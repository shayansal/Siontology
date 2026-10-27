//! The devnet's prover: folds every Pulse into a World Proof.
//!
//! Validator 0 executes with tracing and hands each Pulse's chunk traces to
//! this thread, which proves every chunk with every proof stack, folds the
//! leaves into a World Proof, verifies it and publishes it.

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use fold_core::encode::encode;
use fold_core::ids::Pulse;
use fold_prover::{prove_pulse, verify_world_proof, ProofStack, ReplayStack};
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

/// The proof stacks the devnet uses.
pub fn stacks() -> Vec<Box<dyn ProofStack>> {
    vec![Box::new(ReplayStack)]
}

/// Starts the prover thread.
pub fn spawn(shared: Arc<Shared>, proven: Arc<Mutex<BTreeMap<Pulse, Value>>>) -> ProverHandle {
    let (jobs, inbox): (Sender<ProverJob>, Receiver<ProverJob>) = mpsc::channel();
    let (stop, stopped) = mpsc::channel::<()>();
    let history = shared.config.history.max(1);
    let thread = thread::Builder::new()
        .name("prover".into())
        .spawn(move || {
            let stacks = stacks();
            let refs: Vec<&dyn ProofStack> = stacks.iter().map(|s| s.as_ref()).collect();
            while stopped.try_recv().is_err() {
                let Ok(job) = inbox.recv_timeout(std::time::Duration::from_millis(100)) else { continue };
                let started = std::time::Instant::now();
                let entry = match prove_pulse(&job.output, &job.traces, &refs)
                    .and_then(|proof| verify_world_proof(&proof, &refs).map(|report| (proof, report)))
                {
                    Ok((proof, report)) => {
                        if proof.after != job.header.after || proof.before != job.header.before {
                            json!({ "pulse": job.header.pulse, "error": "proof does not match the header" })
                        } else {
                            json!({
                                "pulse": job.header.pulse,
                                "header": job.header.digest(),
                                "claim": proof.claim_digest(),
                                "statements_root": proof.statements_root,
                                "leaves": report.leaves,
                                "stacks": report.stacks,
                                "simulated": report.simulated,
                                "bytes": report.bytes,
                                "proving_ms": started.elapsed().as_millis() as u64,
                                "proof": hex::encode(encode(&proof)),
                            })
                        }
                    }
                    Err(error) => json!({ "pulse": job.header.pulse, "error": error.to_string() }),
                };
                if let Ok(mut proven) = proven.lock() {
                    proven.insert(job.header.pulse, entry);
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
