//! The devnet's prover: folds every Pulse into a World Proof.
//!
//! Validator 0 executes with tracing and hands each Pulse's chunk traces,
//! published data and lane certificates to this thread, which proves every
//! chunk with every proof stack, folds the leaves into a World Proof that
//! also binds the data's availability root and the certificates, verifies it
//! and publishes it.

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use fold_core::encode::encode;
use fold_core::ids::Pulse;
use fold_prover::{prove_pulse, verify_world_proof, ProofStack, ReplayStack, SpecStack};
use serde_json::{json, Value};

use crate::validator::{ProverJob, Shared};

/// A running prover.
#[derive(Debug)]
pub struct ProverHandle {
    /// Where validators send jobs.
    pub jobs: Sender<ProverJob>,
    stop: Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl ProverHandle {
    /// Asks the prover to stop after its current job.
    pub fn stop(&self) {
        let _ = self.stop.send(());
    }

    /// Waits for the prover thread.
    pub fn join(mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A Pulse's World Proof as published: a summary and the Borsh bytes.
#[derive(Clone, Debug)]
pub struct Proven {
    /// What was proven (or the error).
    pub summary: Value,
    /// The proof's canonical encoding, if proving succeeded.
    pub bytes: Option<Arc<Vec<u8>>>,
}

/// World Proofs kept in memory (they include every witness).
pub const PROOF_HISTORY: usize = 32;

/// The proof stacks the devnet uses: the reference executor (A) and the
/// independent implementation of the specification (B). A World Proof
/// verifies only if both accept every chunk.
pub fn stacks() -> Vec<Box<dyn ProofStack>> {
    vec![Box::new(ReplayStack), Box::new(SpecStack)]
}

/// Starts the prover thread.
pub fn spawn(shared: Arc<Shared>, proven: Arc<Mutex<BTreeMap<Pulse, Proven>>>) -> ProverHandle {
    let (jobs, inbox): (Sender<ProverJob>, Receiver<ProverJob>) = mpsc::channel();
    let (stop, stopped) = mpsc::channel::<()>();
    let history = shared.config.history.clamp(1, PROOF_HISTORY);
    let thread = thread::Builder::new()
        .name("prover".into())
        .spawn(move || {
            let stacks = stacks();
            let refs: Vec<&dyn ProofStack> = stacks.iter().map(|s| s.as_ref()).collect();
            while stopped.try_recv().is_err() {
                let Ok(job) = inbox.recv_timeout(std::time::Duration::from_millis(100)) else { continue };
                let started = std::time::Instant::now();
                let shard_count = shared.config.shards;
                let mut bytes: Option<Arc<Vec<u8>>> = None;
                let entry = match prove_pulse(&job.output, &job.traces, &refs, job.data, job.certificates)
                    .and_then(|proof| {
                        fold_prover::verify_certificates(
                            &proof,
                            &shared.shard_committees,
                            shard_count,
                            &shared.verifier,
                        )?;
                        verify_world_proof(&proof, &refs).map(|report| (proof, report))
                    }) {
                    Ok((proof, report)) => {
                        bytes = Some(Arc::new(encode(&proof)));
                        if proof.after != job.header.after
                            || proof.before != job.header.before
                            || proof.receipts_root != job.header.receipts_root
                            || proof.data_root != job.header.data_root
                            || proof.beacon() != job.header.beacon
                        {
                            json!({ "pulse": job.header.pulse, "error": "proof does not match the header" })
                        } else {
                            json!({
                                "pulse": job.header.pulse,
                                "header": job.header.digest(),
                                "claim": proof.claim_digest(),
                                "statements_root": proof.statements_root,
                                "data_root": proof.data_root,
                                "leaves": report.leaves,
                                "transactions": report.transactions,
                                "certificates": proof.certificates.len(),
                                "stacks": report.stacks,
                                "simulated": report.simulated,
                                "bytes": report.bytes,
                                "proving_ms": started.elapsed().as_millis() as u64,
                            })
                        }
                    }
                    Err(error) => json!({ "pulse": job.header.pulse, "error": error.to_string() }),
                };
                if let Ok(mut proven) = proven.lock() {
                    proven.insert(
                        job.header.pulse,
                        Proven {
                            summary: entry,
                            bytes,
                        },
                    );
                    while proven.len() > history {
                        let oldest = *proven.keys().next().expect("non-empty");
                        proven.remove(&oldest);
                    }
                }
            }
        })
        .expect("spawn prover thread");
    ProverHandle {
        jobs,
        stop,
        thread: Some(thread),
    }
}
