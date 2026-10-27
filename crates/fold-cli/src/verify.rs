//! Light-client verification of a Pulse.

use fold_core::encode::decode;
use fold_core::hash::Hash32;
use fold_core::ids::{Pulse, TxDigest};
use fold_exec::TxEffects;
use fold_light::{verify_certificate, verify_object, verify_receipt, ChainSpec, ObjectProof};
use fold_node::Query;
use fold_prover::{verify_world_proof, ProofStack, ReplayStack, SpecStack, WorldProof};
use serde_json::Value;

use crate::error::CliError;
use crate::ledger::{Ledger, LedgerExt};

fn parse<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, CliError> {
    serde_json::from_value(value).map_err(|e| CliError::Protocol(e.to_string()))
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
            // The latest certified Pulse, or a recent one with transactions.
            let certified: Pulse = parse(status["certified_pulse"].clone())?;
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

    // 1. The committee certificate, against the committee the chain
    // identity fixes for the header's epoch.
    let committee = chain
        .committee(header.epoch)
        .map_err(|e| CliError::Verification(e.to_string()))?;
    let keys = &chain.keys;
    let certificate = view
        .certificate
        .clone()
        .ok_or_else(|| CliError::Verification("the Pulse is not certified yet".into()))?;
    if certificate.header != header {
        return Err(CliError::Verification(
            "certificate is for another header".into(),
        ));
    }
    if insecure {
        report
            .push("certificate: SKIPPED — this devnet runs insecure simulation signatures".into());
    } else {
        let stake = verify_certificate(&certificate, &committee, keys)
            .map_err(|e| CliError::Verification(e.to_string()))?;
        report.push(format!(
            "certificate: {} of {} validators signed (quorum {}), each with ML-DSA-44 + SLH-DSA-SHA2-128f ✓",
            stake,
            committee.total_stake(),
            committee.quorum_threshold()
        ));
    }

    // 2. Receipts.
    let mut checked = 0;
    for digest in view.transactions.iter().take(3) {
        if let crate::ledger::TxState::Executed(record) = ledger.tx(*digest)? {
            verify_receipt(&header, &record.receipt)
                .map_err(|e| CliError::Verification(e.to_string()))?;
            checked += 1;
        }
    }
    report.push(format!(
        "receipts: {checked} of {} transactions proven against the receipts root ✓",
        view.transactions.len()
    ));

    // 3. A state proof (when the node's latest state is this Pulse).
    let faucet = fold_exec::object::well_known::account(&fold_exec::Wallet::named("faucet").id);
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
    let sampled = fold_light::availability::sample(&header, &da, samples, 0xF01D, |row, column| {
        ledger.share(pulse, row, column).ok()
    })
    .map_err(|e| CliError::Verification(e.to_string()))?;
    report.push(format!(
        "availability: {} of {} random shares of the {}×{} square verified; a withholding would go unnoticed with probability < {:.1e} ✓",
        sampled.verified,
        sampled.positions.len(),
        2 * da.k,
        2 * da.k,
        fold_da::undetected_probability(da.k as usize, samples)
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
                    let shards = chain
                        .shard_committees(header.epoch)
                        .map_err(|e| CliError::Verification(e.to_string()))?;
                    fold_prover::verify_certificates(&proof, &shards, chain.shards, keys)
                        .map_err(|e| CliError::Verification(e.to_string()))?;
                    report.push(format!(
                        "lane certificates: all {} executed transactions locked their inputs with a shard quorum ✓",
                        proof.certificates.len()
                    ));
                }
            }
            Ok(entry) => report.push(format!("world proof: unavailable ({})", entry["error"])),
            Err(e) => report.push(format!("world proof: unavailable ({e})")),
        }
    }
    Ok(report)
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
    let signed = if chain.insecure_signatures {
        None
    } else {
        let committee = chain
            .committee(header.epoch)
            .map_err(|e| CliError::Verification(e.to_string()))?;
        let stake = verify_certificate(&certificate, &committee, &chain.keys)
            .map_err(|e| CliError::Verification(e.to_string()))?;
        Some((stake, committee.quorum_threshold()))
    };
    verify_receipt(header, &record.receipt).map_err(|e| CliError::Verification(e.to_string()))?;
    Ok(VerifiedReceipt {
        pulse: record.pulse,
        effects: record.receipt.effects.clone(),
        signed,
        chain: (chain_id, pinned.is_some()),
    })
}
