//! Light-client verification of a Pulse.

use fold_consensus::Committee;
use fold_core::encode::decode;
use fold_core::hash::Hash32;
use fold_core::ids::Pulse;
use fold_light::{verify_certificate, verify_object, verify_receipt, ObjectProof, ValidatorSet};
use fold_node::Query;
use fold_prover::{verify_world_proof, ProofStack, ReplayStack, WorldProof};
use serde_json::Value;

use crate::error::CliError;
use crate::ledger::{Ledger, LedgerExt};

fn parse<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, CliError> {
    serde_json::from_value(value).map_err(|e| CliError::Protocol(e.to_string()))
}

/// Verifies a Pulse (the latest certified one by default) the way a light
/// client would: the committee certificate, receipts, a state proof, data
/// availability sampling and, if `world`, the World Proof. Returns a report.
pub fn verify_pulse(
    ledger: &dyn Ledger,
    pulse: Option<Pulse>,
    samples: usize,
    world: bool,
) -> Result<Vec<String>, CliError> {
    let mut report = Vec::new();
    let status = ledger.status()?;
    let genesis: Hash32 = parse(status["genesis"].clone())?;
    let insecure = status["insecure_signatures"].as_bool().unwrap_or(false);
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
    if header.chain != genesis {
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

    // 1. The committee certificate.
    let validators = ledger.query(Query::Validators)?;
    let committee: Committee = parse(validators["committee"].clone())?;
    let keys: ValidatorSet = parse(validators["keys"].clone())?;
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
        let stake = verify_certificate(&certificate, &committee, &keys)
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
                if proof.before != header.before
                    || proof.after != header.after
                    || proof.receipts_root != header.receipts_root
                    || proof.data_root != header.data_root
                {
                    return Err(CliError::Verification(
                        "World Proof does not match the certified header".into(),
                    ));
                }
                let stacks: Vec<&dyn ProofStack> = vec![&ReplayStack];
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
                    let shards: std::collections::BTreeMap<fold_core::ids::ShardId, Committee> =
                        parse(validators["shards"].clone())?;
                    let shard_count = ledger.chain_config()?.shard_count;
                    fold_prover::verify_certificates(&proof, &shards, shard_count, &keys)
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
