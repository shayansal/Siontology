//! Light-client verification of a Pulse.

use serde_json::Value;
use sion_core::encode::decode;
use sion_core::hash::Hash32;
use sion_core::ids::{Pulse, TxDigest};
use sion_exec::TxEffects;
use sion_light::{
    verify_object, verify_receipt, ChainSpec, EpochChain, EpochProof, ObjectProof, PulseCertificate,
};
use sion_node::Query;
use sion_prover::{verify_world_proof, ProofStack, ReplayStack, SpecStack, WorldProof};

use crate::error::CliError;
use crate::ledger::{Ledger, LedgerExt};

fn parse<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, CliError> {
    serde_json::from_value(value).map_err(|e| CliError::Protocol(e.to_string()))
}

/// Verifies the committees from the pinned epoch 0 up to `epoch`, one
/// boundary at a time, with proofs fetched from the ledger.
pub fn epoch_chain(
    ledger: &dyn Ledger,
    chain: &ChainSpec,
    epoch: u64,
) -> Result<EpochChain, CliError> {
    let mut epochs = EpochChain::new(chain).map_err(|e| CliError::Verification(e.to_string()))?;
    extend_epoch_chain(ledger, &mut epochs, epoch)?;
    Ok(epochs)
}

/// Extends `epochs` up to `epoch`, one boundary at a time, with proofs
/// fetched from the ledger; returns how many boundaries it followed (none
/// if `epochs` already reaches `epoch`).
pub fn extend_epoch_chain(
    ledger: &dyn Ledger,
    epochs: &mut EpochChain,
    epoch: u64,
) -> Result<u64, CliError> {
    let mut followed = 0;
    while epochs.latest() < epoch {
        let next = epochs.latest() + 1;
        // A boundary's certificate can complete after those of the Pulses
        // that follow it: give it a moment before giving up.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let proof: EpochProof = loop {
            match ledger.query(Query::Epoch(next)) {
                Ok(value) => break parse(value)?,
                Err(error) if std::time::Instant::now() > deadline => return Err(error),
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
            }
        };
        epochs
            .extend(&proof)
            .map_err(|e| CliError::Verification(format!("epoch {next}: {e}")))?;
        followed += 1;
    }
    Ok(followed)
}

/// Checks a Pulse certificate against the committee of its epoch, following
/// the committees from the pinned identity; returns (signing stake, quorum),
/// or `None` for a devnet with insecure simulation signatures.
pub fn verify_header_certificate(
    ledger: &dyn Ledger,
    chain: &ChainSpec,
    certificate: &PulseCertificate,
) -> Result<Option<(u64, u64)>, CliError> {
    let epochs = epoch_chain(ledger, chain, certificate.header.epoch)?;
    epochs
        .verify_header(certificate)
        .map_err(|e| CliError::Verification(e.to_string()))
}

/// Verifies a Pulse (the latest certified one by default) the way a light
/// client would: the chain identity, the committee certificate, receipts, a
/// state proof, data availability sampling and, if `world`, the World Proof
/// and lane certificates. Returns a report.
///
/// `pinned` is the chain identity the client trusts, obtained out of band.
/// Without it the client can only check that the node is self-consistent:
/// it learns the identity from the node it is checking.
pub fn verify_pulse(
    ledger: &dyn Ledger,
    pinned: Option<Hash32>,
    pulse: Option<Pulse>,
    samples: usize,
    world: bool,
) -> Result<Vec<String>, CliError> {
    Ok(verify_pulse_report(ledger, pinned, pulse, samples, world)?.lines)
}

/// What [`verify_pulse_report`] established.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PulseReport {
    /// The chain identity every check ran against: the pinned one, or, if
    /// none was pinned, the one the node served for this verification.
    pub chain: Hash32,
    /// The Pulse verified.
    pub pulse: Pulse,
    /// The checks, one line each.
    pub lines: Vec<String>,
}

/// [`verify_pulse`], also returning the chain identity and the Pulse it
/// checked, so that a client pinning on first use pins exactly the identity
/// it verified against rather than asking the node a second time.
pub fn verify_pulse_report(
    ledger: &dyn Ledger,
    pinned: Option<Hash32>,
    pulse: Option<Pulse>,
    samples: usize,
    world: bool,
) -> Result<PulseReport, CliError> {
    verify_pulse_cached(ledger, pinned, pulse, samples, world, &mut None)
}

/// [`verify_pulse_report`] for a client that verifies again and again: the
/// committees it followed last time come in `epochs` and go back out
/// there, extended to the verified header's epoch, so that each
/// verification only follows the boundaries since the last one. Only the
/// committees of the verified epoch and the one before are kept. A cache
/// for another chain, or one that has forgotten the header's epoch, is
/// replaced by one followed from the pinned epoch 0.
pub fn verify_pulse_cached(
    ledger: &dyn Ledger,
    pinned: Option<Hash32>,
    pulse: Option<Pulse>,
    samples: usize,
    world: bool,
    epochs: &mut Option<EpochChain>,
) -> Result<PulseReport, CliError> {
    let mut report = Vec::new();

    // 0. The chain identity: every committee below is derived from it.
    let validators = ledger.query(Query::Validators)?;
    let chain: ChainSpec = parse(validators["chain"].clone())?;
    let chain_id = chain
        .verify(pinned.as_ref())
        .map_err(|e| CliError::Verification(e.to_string()))?;
    let insecure = chain.insecure_signatures;
    report.push(match pinned {
        Some(_) => format!(
            "chain: {} matches the pinned identity; {} validators, keys derive their identifiers ✓",
            chain_id.short(),
            chain.validators.len()
        ),
        None => format!(
            "chain: {} (NOT PINNED: learned from the node itself; pass --chain {} to pin it)",
            chain_id.short(),
            chain_id.to_hex()
        ),
    });

    let status = ledger.status()?;
    let pulse = match pulse {
        Some(p) => p,
        None => {
            // The latest certified Pulse (and proven, if the World Proof is
            // wanted), or a recent one with transactions.
            let mut certified: Pulse = parse(status["certified_pulse"].clone())?;
            if world {
                let proven: Option<Pulse> =
                    serde_json::from_value(status["proven_pulse"].clone()).unwrap_or(None);
                certified = certified.min(proven.unwrap_or(Pulse(0)));
            }
            let mut chosen = certified;
            for back in 0..32u64 {
                let Some(candidate) = certified.0.checked_sub(back) else {
                    break;
                };
                match ledger.pulse_view(Some(Pulse(candidate))) {
                    Ok(view) if !view.transactions.is_empty() && view.certificate.is_some() => {
                        chosen = Pulse(candidate);
                        break;
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            chosen
        }
    };
    if pulse == Pulse(0) {
        return Err(CliError::Verification("no certified Pulse yet".into()));
    }
    let view = ledger.pulse_view(Some(pulse))?;
    let header = view.header.clone();
    if header.chain != chain_id {
        return Err(CliError::Verification(
            "the header is for another chain".into(),
        ));
    }
    report.push(format!(
        "Pulse {} header {} (state root {}, knowledge root {}, {} claims)",
        header.pulse,
        header.digest().short(),
        header.after.state_root.short(),
        header.after.knowledge_root.short(),
        header.after.knowledge_size
    ));

    // 1. The committee certificate, against the committee of the header's
    // epoch: epoch 0's is fixed by the chain identity, each later one is
    // proven from the validator set at the end of the epoch before.
    let certificate = view
        .certificate
        .clone()
        .ok_or_else(|| CliError::Verification("the Pulse is not certified yet".into()))?;
    if certificate.header != header {
        return Err(CliError::Verification(
            "certificate is for another header".into(),
        ));
    }
    let reusable = epochs.as_ref().is_some_and(|cached| {
        cached.chain() == chain_id
            && (cached.committee(header.epoch).is_some() || cached.latest() < header.epoch)
    });
    let kept = if reusable { epochs.take() } else { None };
    let reused = kept.is_some();
    let mut followed_epochs = match kept {
        Some(cached) => cached,
        None => EpochChain::new(&chain).map_err(|e| CliError::Verification(e.to_string()))?,
    };
    let followed = match extend_epoch_chain(ledger, &mut followed_epochs, header.epoch) {
        Ok(followed) => followed,
        Err(error) => {
            // Every committee followed so far was proven: keep them.
            *epochs = Some(followed_epochs);
            return Err(error);
        }
    };
    if header.epoch > 0 {
        report.push(if reused {
            format!(
                "committees: followed from the pinned epoch 0 to epoch {}; {} new certified boundary and validator-set proof(s) checked, the earlier ones kept from previous verifications ✓",
                header.epoch, followed
            )
        } else {
            format!(
                "committees: followed from the pinned epoch 0 to epoch {} through {} certified boundaries and validator-set proofs ✓",
                header.epoch, followed
            )
        });
    }
    let verified_header = followed_epochs.verify_header(&certificate);
    let committee_size = followed_epochs
        .committee(header.epoch)
        .map(|c| (c.size(), c.total_stake()));
    followed_epochs.forget_before(header.epoch.saturating_sub(1));
    *epochs = Some(followed_epochs);
    match verified_header.map_err(|e| CliError::Verification(e.to_string()))? {
        None => report
            .push("certificate: SKIPPED — this devnet runs insecure simulation signatures".into()),
        Some((stake, quorum)) => {
            let (size, total) = committee_size.expect("verified above");
            report.push(format!(
                "certificate: {} of {} validators signed, {} of {} stake (quorum {}), each signature ML-DSA-44 + SLH-DSA-SHA2-128f ✓",
                certificate.signers().len(),
                size,
                stake,
                total,
                quorum
            ));
        }
    }
    record_seen(&chain_id, header.pulse);

    // Forced inclusion, as the certified header states it.
    report.push(if header.forced.overdue > 0 {
        format!(
            "forced inclusion: {} intent(s) OVERDUE, censorship detected and certified by the committee ({} pending)",
            header.forced.overdue, header.forced.pending
        )
    } else {
        format!(
            "forced inclusion: no overdue intents ({} pending within their deadline) ✓",
            header.forced.pending
        )
    });

    // 2. The executed order, then receipts. The node's list of executed
    // transactions must be the one the certified header committed to; each
    // receipt must sit at its transaction's position in a tree of exactly
    // that many receipts.
    if sion_exec::pulse::order_digest(header.pulse, &header.beacon, &view.transactions)
        != header.order_digest
    {
        return Err(CliError::Verification(
            "the node's transaction list is not the certified order".into(),
        ));
    }
    let mut checked = 0;
    for (index, digest) in view.transactions.iter().enumerate().take(3) {
        let crate::ledger::TxState::Executed(record) = ledger.tx(*digest)? else {
            return Err(CliError::Verification(format!(
                "the node has no receipt for certified transaction {}",
                digest.short()
            )));
        };
        check_receipt_position(&record.receipt, *digest, index, view.transactions.len())?;
        verify_receipt(&header, &record.receipt)
            .map_err(|e| CliError::Verification(e.to_string()))?;
        checked += 1;
    }
    report.push(format!(
        "receipts: {checked} of {} transactions proven against the receipts root, in the certified order ✓",
        view.transactions.len()
    ));

    // 3. A state proof (when the node's latest state is this Pulse).
    let faucet = sion_exec::object::well_known::account(&sion_exec::Wallet::named("faucet").id);
    let value = ledger.query(Query::Object(faucet))?;
    let at: Pulse = parse(value["pulse"].clone())?;
    if at == header.pulse {
        let proof: ObjectProof = parse(value["proof"].clone())?;
        verify_object(&header, &proof).map_err(|e| CliError::Verification(e.to_string()))?;
        report.push("state: an account object proven against the state root ✓".into());
    } else {
        report.push(format!("state: proof skipped (node already at Pulse {at})"));
    }

    // 4. Data availability.
    let da = ledger.data(pulse)?;
    // Sample positions must be unpredictable to the node: fresh randomness
    // from the operating system, mixed with the data root.
    let mut fresh = [0u8; 32];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut fresh);
    let seed =
        sion_core::hash::hash_parts("sion/light/sample-seed", &[&header.data_root.0, &fresh]);
    let seed = u64::from_le_bytes(seed.0[..8].try_into().expect("8 bytes"));
    let sampled = sion_light::availability::sample(&header, &da, samples, seed, |row, column| {
        ledger.share(pulse, row, column).ok()
    })
    .map_err(|e| CliError::Verification(e.to_string()))?;
    report.push(format!(
        "availability: {} of {} random shares of the {}×{} square verified; a withholding would go unnoticed with probability < {:.1e} ✓",
        sampled.verified,
        sampled.positions.len(),
        2 * da.k,
        2 * da.k,
        sion_da::undetected_probability(da.k as usize, samples)
    ));

    // 5. The World Proof.
    if world {
        match ledger.query(Query::World(pulse)) {
            Ok(entry) if entry.get("proof").is_some() => {
                let bytes = hex::decode(entry["proof"].as_str().unwrap_or(""))
                    .map_err(|e| CliError::Protocol(e.to_string()))?;
                let proof: WorldProof =
                    decode(&bytes).map_err(|e| CliError::Protocol(e.to_string()))?;
                if proof.pulse != header.pulse
                    || proof.before != header.before
                    || proof.after != header.after
                    || proof.receipts_root != header.receipts_root
                    || proof.data_root != header.data_root
                    || proof.data_k != header.data_k
                    || proof.beacon() != header.beacon
                {
                    return Err(CliError::Verification(
                        "World Proof does not match the certified header".into(),
                    ));
                }
                let stacks: Vec<&dyn ProofStack> = vec![&ReplayStack, &SpecStack];
                let world_report = verify_world_proof(&proof, &stacks)
                    .map_err(|e| CliError::Verification(e.to_string()))?;
                report.push(format!(
                    "world proof: {} leaves re-verified by stack {:?}; the published data is what executed and \
                     encodes to the certified data root ({} KB{}) ✓",
                    world_report.leaves,
                    world_report.stacks,
                    world_report.bytes / 1024,
                    if world_report.simulated {
                        "; replay proofs simulate succinct ones"
                    } else {
                        ""
                    }
                ));
                if insecure {
                    report
                        .push("lane certificates: SKIPPED — insecure simulation signatures".into());
                } else {
                    // The committees come from the validator set the proof
                    // carries, proven against the Pulse's starting state.
                    sion_prover::verify_certificates(&proof, chain_id, chain.shards, None)
                        .map_err(|e| CliError::Verification(e.to_string()))?;
                    report.push(format!(
                        "lane certificates: all {} executed transactions locked their inputs with a quorum of their shard's committee, taken from the proven validator set ✓",
                        proof.certificates.len()
                    ));
                }
            }
            Ok(entry) => {
                return Err(CliError::Verification(format!(
                    "World Proof of Pulse {pulse} unavailable: {}",
                    entry["error"]
                )))
            }
            Err(e) => {
                return Err(CliError::Verification(format!(
                    "World Proof of Pulse {pulse} unavailable: {e}"
                )))
            }
        }
    }
    Ok(PulseReport {
        chain: chain_id,
        pulse,
        lines: report,
    })
}

/// A transaction's receipt as a light client established it.
#[derive(Clone, Debug)]
pub struct VerifiedReceipt {
    /// The Pulse it executed in.
    pub pulse: Pulse,
    /// The proven effects (the outcome class is proven; a failure's reason
    /// text is not committed).
    pub effects: TxEffects,
    /// Stake that signed the Pulse header, and the quorum it needed
    /// (`None` if the devnet runs insecure simulation signatures).
    pub signed: Option<(u64, u64)>,
    /// The chain identity checked, and whether it was pinned.
    pub chain: (Hash32, bool),
}

/// Objects as a light client established them: state proofs checked
/// against a certified header.
#[derive(Clone, Debug)]
pub struct VerifiedState {
    /// The Pulse whose header the proofs were checked against: the state
    /// after it.
    pub pulse: Pulse,
    /// The objects proven, in the order given; `None` where the proof shows
    /// that no object has that id.
    pub objects: Vec<(sion_core::ids::ObjectId, Option<sion_exec::object::Object>)>,
    /// Stake that signed the Pulse header, and the quorum it needed
    /// (`None` if the devnet runs insecure simulation signatures).
    pub signed: Option<(u64, u64)>,
    /// The chain identity checked, and whether it was pinned.
    pub chain: (Hash32, bool),
}

impl VerifiedState {
    /// One line on how the state was proven.
    pub fn line(&self) -> String {
        let certificate = match self.signed {
            Some((stake, quorum)) => {
                format!("certified by {stake} of the committee's stake (quorum {quorum})")
            }
            None => "its certificate not checked: the devnet runs insecure simulation signatures"
                .to_string(),
        };
        format!(
            "{} proven against Pulse {}'s header, {certificate}, chain {}{}",
            if self.objects.len() == 1 {
                "state".to_string()
            } else {
                format!("{} objects", self.objects.len())
            },
            self.pulse,
            self.chain.0.short(),
            if self.chain.1 {
                ""
            } else {
                " (not pinned: the node vouched for it)"
            }
        )
    }
}

/// How many Pulses older than the latest certified Pulse a client knows of
/// a state proof may be. Older answers are refused: the node is behind, or
/// it is answering from an old state to hide a newer one.
pub const STALE_TOLERANCE: u64 = 16;

/// The latest certified Pulse of each chain this process has verified.
static SEEN: std::sync::Mutex<std::collections::BTreeMap<Hash32, Pulse>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// The latest certified Pulse of `chain` this process has verified, if any.
pub fn seen_pulse(chain: &Hash32) -> Option<Pulse> {
    SEEN.lock().ok()?.get(chain).copied()
}

/// Remembers that this process verified a certified header of `chain` at
/// `pulse`. A long-running client (the app, a gateway, an agent's runtime)
/// thereby refuses a node that later answers from an older state. It is
/// kept in memory only: a devnet restarted from the same genesis has the
/// same chain identity and starts again from Pulse 0.
pub fn record_seen(chain: &Hash32, pulse: Pulse) {
    if let Ok(mut seen) = SEEN.lock() {
        let entry = seen.entry(*chain).or_insert(pulse);
        if *entry < pulse {
            *entry = pulse;
        }
    }
}

/// How long to wait for the certificate of the Pulse a state proof is from.
const STATE_CERTIFICATE_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// Checks `proofs`, all of the state after Pulse `pulse`, against that
/// Pulse's certified header the way a light client would: the chain
/// identity (against `pinned`, if given), the committee certificate, then
/// each proof against the header's state root. Waits up to 20 seconds for
/// the Pulse to be certified. The proofs must be fresh: from no more than
/// [`STALE_TOLERANCE`] Pulses before the latest certified Pulse the client
/// knows of as it begins checking them, whether this process verified it,
/// the node says it has certified it, or the caller knows it from
/// elsewhere (`min_pulse`).
pub fn verify_state(
    ledger: &dyn Ledger,
    pinned: Option<Hash32>,
    pulse: Pulse,
    proofs: &[ObjectProof],
    min_pulse: Option<Pulse>,
) -> Result<VerifiedState, CliError> {
    let validators = ledger.query(Query::Validators)?;
    let chain: ChainSpec = parse(validators["chain"].clone())?;
    let chain_id = chain
        .verify(pinned.as_ref())
        .map_err(|e| CliError::Verification(e.to_string()))?;
    // Fresh enough: not far behind a later certified Pulse this process
    // verified, nor behind the node's own account of its certified Pulse
    // (a mirror that serves an old state while it follows the chain). Both
    // are taken now, as the answer is checked: Pulses certified while this
    // client waits for and checks the certificate came after the node
    // answered, and are not held against it.
    let claimed: Option<Pulse> = ledger
        .status()
        .ok()
        .and_then(|status| serde_json::from_value(status["certified_pulse"].clone()).ok());
    let known = [seen_pulse(&chain_id), claimed, min_pulse]
        .into_iter()
        .flatten()
        .max();
    let deadline = std::time::Instant::now() + STATE_CERTIFICATE_WAIT;
    let certificate = loop {
        let view = ledger.pulse_view(Some(pulse))?;
        match view.certificate {
            Some(certificate) => break certificate,
            None if std::time::Instant::now() >= deadline => {
                return Err(CliError::Verification(format!(
                    "Pulse {pulse} is not certified yet"
                )))
            }
            None => std::thread::sleep(std::time::Duration::from_millis(200)),
        }
    };
    let header = &certificate.header;
    if header.chain != chain_id || header.pulse != pulse {
        return Err(CliError::Verification(
            "the certificate is for another chain or Pulse".into(),
        ));
    }
    let signed = verify_header_certificate(ledger, &chain, &certificate)?;
    if let Some(known) = known {
        if pulse.0.saturating_add(STALE_TOLERANCE) < known.0 {
            return Err(CliError::Verification(format!(
                "the node answered from Pulse {pulse}, more than {STALE_TOLERANCE} Pulses before Pulse {known}, which is certified: it is behind, or it is hiding a newer state"
            )));
        }
    }
    record_seen(&chain_id, pulse);
    let mut objects = Vec::with_capacity(proofs.len());
    for proof in proofs {
        verify_object(header, proof).map_err(|e| CliError::Verification(e.to_string()))?;
        objects.push((proof.id, proof.object.clone()));
    }
    Ok(VerifiedState {
        pulse,
        objects,
        signed,
        chain: (chain_id, pinned.is_some()),
    })
}

/// Fetches object `id` with its state proof and checks it as
/// [`verify_state`] does.
pub fn verify_object_now(
    ledger: &dyn Ledger,
    pinned: Option<Hash32>,
    id: sion_core::ids::ObjectId,
    min_pulse: Option<Pulse>,
) -> Result<VerifiedState, CliError> {
    let value = ledger.query(Query::Object(id))?;
    let pulse: Pulse = parse(value["pulse"].clone())?;
    let proof: ObjectProof = parse(value["proof"].clone())?;
    if proof.id != id {
        return Err(CliError::Verification(
            "the node proved another object".into(),
        ));
    }
    verify_state(ledger, pinned, pulse, &[proof], min_pulse)
}

/// The chain identity the ledger serves, unpinned: the node vouches for
/// itself.
pub fn served_chain(ledger: &dyn Ledger) -> Result<Hash32, CliError> {
    let validators = ledger.query(Query::Validators)?;
    let chain: ChainSpec = parse(validators["chain"].clone())?;
    Ok(chain.id())
}

/// Fetches a transaction's receipt and verifies it the way a light client
/// would: the chain identity (against `pinned`, if given), the committee
/// certificate of the Pulse it executed in, then the receipt's inclusion
/// proof against that certified header.
pub fn verify_receipt_of(
    ledger: &dyn Ledger,
    pinned: Option<Hash32>,
    digest: TxDigest,
) -> Result<VerifiedReceipt, CliError> {
    let record = match ledger.tx(digest)? {
        crate::ledger::TxState::Executed(record) => record,
        crate::ledger::TxState::Pending(state) => {
            return Err(CliError::Verification(format!(
                "not executed yet ({state})"
            )))
        }
        crate::ledger::TxState::Rejected(reason) => {
            return Err(CliError::Verification(format!("never certified: {reason}")))
        }
        crate::ledger::TxState::Unknown => {
            return Err(CliError::Verification("unknown transaction".into()))
        }
    };
    if record.receipt.effects.digest != digest {
        return Err(CliError::Verification(
            "the node served another transaction's receipt".into(),
        ));
    }
    let validators = ledger.query(Query::Validators)?;
    let chain: ChainSpec = parse(validators["chain"].clone())?;
    let chain_id = chain
        .verify(pinned.as_ref())
        .map_err(|e| CliError::Verification(e.to_string()))?;
    let view = ledger.pulse_view(Some(record.pulse))?;
    let certificate = view.certificate.ok_or_else(|| {
        CliError::Verification(format!("Pulse {} is not certified yet", record.pulse))
    })?;
    let header = &certificate.header;
    if header.chain != chain_id || header.pulse != record.pulse {
        return Err(CliError::Verification(
            "the certificate is for another chain or Pulse".into(),
        ));
    }
    let signed = verify_header_certificate(ledger, &chain, &certificate)?;
    record_seen(&chain_id, header.pulse);
    if sion_exec::pulse::order_digest(header.pulse, &header.beacon, &view.transactions)
        != header.order_digest
    {
        return Err(CliError::Verification(
            "the node's transaction list is not the certified order".into(),
        ));
    }
    let index = view
        .transactions
        .iter()
        .position(|d| *d == digest)
        .ok_or_else(|| {
            CliError::Verification("the transaction is not in the certified order".into())
        })?;
    check_receipt_position(&record.receipt, digest, index, view.transactions.len())?;
    verify_receipt(header, &record.receipt).map_err(|e| CliError::Verification(e.to_string()))?;
    Ok(VerifiedReceipt {
        pulse: record.pulse,
        effects: record.receipt.effects.clone(),
        signed,
        chain: (chain_id, pinned.is_some()),
    })
}

/// Checks that a receipt proof is for `digest` at position `index` in a
/// Pulse of `count` transactions (the certified order fixes both).
fn check_receipt_position(
    proof: &sion_light::ReceiptProof,
    digest: TxDigest,
    index: usize,
    count: usize,
) -> Result<(), CliError> {
    if proof.effects.digest != digest || proof.index != index as u64 || proof.count != count as u64
    {
        return Err(CliError::Verification(format!(
            "the receipt for {} is not at its certified position",
            digest.short()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seen_pulse_only_moves_forward_per_chain() {
        let (a, b) = (Hash32([1; 32]), Hash32([2; 32]));
        assert_eq!(seen_pulse(&a), None);
        record_seen(&a, Pulse(40));
        record_seen(&a, Pulse(30));
        record_seen(&b, Pulse(5));
        assert_eq!(seen_pulse(&a), Some(Pulse(40)));
        assert_eq!(seen_pulse(&b), Some(Pulse(5)));
        record_seen(&a, Pulse(41));
        assert_eq!(seen_pulse(&a), Some(Pulse(41)));
    }
}

/// An agent's standing, checked as a light client would: its charter and
/// the overlays of every charter of its lineage proven against one
/// certified header, its status worked out here from them (as the ledger
/// works it out for a payment in the next Pulse), and its allotment proven
/// against another.
#[derive(Clone, Debug)]
pub struct Standing {
    /// The agent.
    pub agent: sion_core::ids::ObjectId,
    /// `active`, `active, narrowed`, `suspended`, `revoked`, `stopped by an
    /// ancestor` or `expired`.
    pub status: &'static str,
    /// Its charter, as proven.
    pub charter: sion_exec::charter::Charter,
    /// What its allotments have left, and what they can spend in the next
    /// Pulse (their buckets' levels), as proven; `None` without one.
    pub budget: Option<(sion_core::Amount, sion_core::Amount)>,
    /// The verifications: the charter and overlays', then the allotment's.
    pub lines: Vec<String>,
    /// The Pulse the status holds after.
    pub pulse: Pulse,
}

impl Standing {
    /// Whether the agent can act: active, narrowed or not.
    pub fn in_good_standing(&self) -> bool {
        self.status.starts_with("active")
    }
}

/// `agent`'s status after `pulse` (for a transaction in the next Pulse)
/// from its proven `charter` and the proven overlays of its lineage
/// (`None` where none exists), by the rule the node's agent query uses
/// ([`sion_exec::charter::agent_status`]).
pub fn status_of(
    agent: &sion_core::ids::ObjectId,
    charter: &sion_exec::charter::Charter,
    overlays: &[(
        sion_core::ids::ObjectId,
        Option<sion_exec::charter::AgentOverlay>,
    )],
    pulse: Pulse,
) -> &'static str {
    sion_exec::charter::agent_status(
        agent,
        charter,
        overlays
            .iter()
            .filter_map(|(id, overlay)| overlay.as_ref().map(|o| (id, o))),
        pulse.next(),
    )
    .label()
}

/// `agent`'s standing, checked against certified headers of the chain
/// `pinned` (or the one the node serves), refusing proofs older than
/// `min_pulse` allows ([`verify_state`]).
pub fn agent_standing(
    ledger: &dyn Ledger,
    pinned: Option<Hash32>,
    agent: sion_core::ids::ObjectId,
    min_pulse: Option<Pulse>,
) -> Result<Standing, CliError> {
    use sion_exec::object::ObjectData;
    let value = ledger.query(Query::Agent(agent))?;
    let pulse: Pulse = parse(value["pulse"].clone())?;
    let charter_proof: ObjectProof = parse(value["charter"].clone())?;
    if charter_proof.id != agent {
        return Err(CliError::Verification(
            "the node proved another charter".into(),
        ));
    }
    let mut proofs = vec![charter_proof];
    for entry in value["overlays"].as_array().into_iter().flatten() {
        proofs.push(parse(entry["overlay"].clone())?);
    }
    let verified = verify_state(ledger, pinned, pulse, &proofs, min_pulse)?;
    let charter = match &verified.objects[0].1 {
        Some(object) => match &object.data {
            ObjectData::Charter(charter) => charter.clone(),
            _ => {
                return Err(CliError::Verification(format!(
                    "{} is not an agent's charter",
                    agent.short()
                )))
            }
        },
        None => {
            return Err(CliError::Verification(format!(
                "the state holds no agent {}",
                agent.short()
            )))
        }
    };
    // The overlays proven must be those of the lineage, every one of them:
    // a node that leaves out an ancestor's could hide its suspension.
    let ancestry = charter.lineage.ancestry();
    if ancestry.last() != Some(&agent) {
        return Err(CliError::Verification(
            "the charter's lineage does not lead to the agent".into(),
        ));
    }
    let wanted: std::collections::BTreeSet<sion_core::ids::ObjectId> = ancestry
        .iter()
        .map(sion_exec::charter::overlay_id)
        .collect();
    let proven: std::collections::BTreeSet<sion_core::ids::ObjectId> =
        verified.objects[1..].iter().map(|(id, _)| *id).collect();
    if wanted != proven {
        return Err(CliError::Verification(
            "the node did not prove the overlay of every charter of the agent's lineage".into(),
        ));
    }
    let mut overlays = Vec::new();
    for id in &ancestry {
        let overlay_id = sion_exec::charter::overlay_id(id);
        let overlay = verified
            .objects
            .iter()
            .find(|(proven_id, _)| *proven_id == overlay_id)
            .and_then(|(_, object)| match object.as_ref().map(|o| &o.data) {
                Some(ObjectData::AgentOverlay(o)) => Some(o.clone()),
                _ => None,
            });
        overlays.push((*id, overlay));
    }
    let status = status_of(&agent, &charter, &overlays, verified.pulse);
    let mut lines = vec![verified.line()];
    // Its budget: every allotment of the charter, each proven.
    let mut budget: Option<(sion_core::Amount, sion_core::Amount)> = None;
    for allotment in &charter.allotments {
        let proven = verify_object_now(ledger, pinned, *allotment, Some(verified.pulse))?;
        if let Some(Some(sion_exec::object::Object {
            data: ObjectData::Allotment(a),
            ..
        })) = proven.objects.first().map(|(_, o)| o.clone())
        {
            let next = proven.pulse.next();
            let (left, now) = budget.unwrap_or((sion_core::Amount(0), sion_core::Amount(0)));
            budget = Some((
                sion_core::Amount(left.0.saturating_add(a.remaining.0)),
                sion_core::Amount(
                    now.0
                        .saturating_add(a.bucket.level(next).min(a.remaining).0),
                ),
            ));
        }
        lines.push(format!("budget: {}", proven.line()));
    }
    Ok(Standing {
        agent,
        status,
        charter,
        budget,
        lines,
        pulse: verified.pulse,
    })
}
