//! The market for proofs on a devnet that proves its Pulses: a requester
//! escrows a payment for the World Proof of a Pulse, a prover delivers it,
//! two verifiers check it under both proof stacks and attest it on their
//! bonds, and the prover is paid. A proof of another Pulse, a changed
//! one, an attestation by someone who is no verifier and a delivery with
//! too few attestations get nowhere.

use std::sync::Arc;

use sion_assurance::AtomClass;
use sion_cli::ledger::{Ledger, LedgerExt};
use sion_cli::proving::{self, Checker, Terms, SYSTEM};
use sion_cli::session::Session;
use sion_core::ids::{AssetId, Pulse};
use sion_core::Amount;
use sion_exec::execute::bond_atom;
use sion_exec::TxKind;
use sion_node::{Devnet, DevnetConfig};

fn usd(n: u64) -> Amount {
    Amount::units(n)
}

#[test]
fn a_pulses_proof_is_asked_for_delivered_attested_under_both_stacks_and_paid() {
    let home = std::env::temp_dir().join(format!("proving-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("SION_HOME", &home);
    // A devnet that proves its Pulses, as the default one does.
    let devnet = Arc::new(Devnet::start(DevnetConfig::default()).unwrap());
    let ledger: &dyn Ledger = devnet.as_ref();
    let mut session = Session::open(ledger).unwrap();
    for who in ["rita", "pat", "vera", "vic", "carol", "mal"] {
        session.fund(who, AssetId::usd(), usd(500)).unwrap();
    }
    let bond = |session: &mut Session<'_>, who: &str, class: AtomClass, label: &str| {
        session
            .must(
                who,
                TxKind::PostBond {
                    class,
                    label: label.into(),
                    amount: usd(100),
                },
                Vec::new(),
            )
            .unwrap();
    };
    let started = ledger.pulse().unwrap().0;
    bond(&mut session, "carol", AtomClass::Adjudicator, "adjudicator");
    bond(
        &mut session,
        "pat",
        AtomClass::Economic,
        sion_exec::task::WORKER_BOND_LABEL,
    );
    for verifier in ["vera", "vic"] {
        bond(
            &mut session,
            verifier,
            AtomClass::Economic,
            sion_cli::tasks::VERIFIER_BOND_LABEL,
        );
    }
    let adjudicator = bond_atom(AtomClass::Adjudicator, &session.id("carol"), "adjudicator").id;
    let verifiers = [session.id("vera"), session.id("vic")];

    // Rita asks for the proof of a Pulse to come, 3 USD in escrow: far
    // enough ahead for the request's three transactions to land first on
    // this machine.
    let now = ledger.pulse().unwrap().0;
    let per_tx = (now - started).div_ceil(4).max(1);
    let target = now + 6 * per_tx + 4;
    let terms = Terms {
        pulse: target,
        amount: usd(3),
        verifiers: verifiers.to_vec(),
        prover: None,
        adjudicator: Some(adjudicator),
        deadline: 500,
    };
    let nobody = Terms {
        verifiers: Vec::new(),
        ..terms.clone()
    };
    assert!(proving::request(&mut session, "rita", &nobody).is_err());
    let task = proving::request(&mut session, "rita", &terms).unwrap();
    let (wanted, state) = proving::wanted(ledger, task).unwrap();
    assert_eq!((wanted.pulse, wanted.system.as_str()), (target, SYSTEM));
    assert_eq!(state.amount, usd(3));
    assert_eq!(state.acceptance.must_attest.len(), 2);
    assert_eq!(state.worker, None);
    // There is no proof of a Pulse that has not come.
    assert!(proving::fetch(ledger, target + 100_000).is_err());

    // The Pulse comes and is proven. Pat fetches its proof from its node.
    session.wait_until(Pulse(target + 1)).unwrap();
    let mut proof = proving::fetch(ledger, target);
    for _ in 0..200 {
        if proof.is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        proof = proving::fetch(ledger, target);
    }
    let proof = proof.unwrap();
    let other = {
        let mut other = proving::fetch(ledger, target + 1);
        for _ in 0..200 {
            if other.is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            other = proving::fetch(ledger, target + 1);
        }
        other.unwrap()
    };
    let verifier = Checker::Verifier { pinned: None };
    let lines = proving::checked(ledger, &wanted, &proof, verifier)
        .unwrap()
        .join("\n");
    assert!(lines.contains("world proof:"), "{lines}");
    // A verifier that pinned no chain is told whose word the chain is.
    assert!(lines.contains("NOT PINNED"), "{lines}");
    // One that pinned the chain the request names checks against it, and
    // one that pinned another is told the request is not for its chain.
    let chain = sion_core::hash::Hash32::from_hex(&wanted.chain).unwrap();
    let pinned = proving::checked(
        ledger,
        &wanted,
        &proof,
        Checker::Verifier {
            pinned: Some(chain),
        },
    )
    .unwrap()
    .join("\n");
    assert!(pinned.contains("matches the pinned identity"), "{pinned}");
    let elsewhere = proving::checked(
        ledger,
        &wanted,
        &proof,
        Checker::Verifier {
            pinned: Some(sion_core::hash::Hash32([9; 32])),
        },
    )
    .unwrap_err()
    .to_string();
    assert!(
        elsewhere.contains("not the chain you pinned"),
        "{elsewhere}"
    );
    assert!(
        lines.contains("ReplayStack") || lines.contains("replay"),
        "{lines}"
    );

    // What is not the proof asked for: that of another Pulse, one with a
    // byte changed, and bytes that are no proof.
    let wrong = proving::checked(ledger, &wanted, &other, verifier)
        .unwrap_err()
        .to_string();
    assert!(
        wrong.contains(&format!("the request is for Pulse {target}")),
        "{wrong}"
    );
    // A byte changed anywhere: a verifier's check does not pass it.
    let mut changed = proof.clone();
    let middle = changed.len() / 2;
    changed[middle] ^= 0x01;
    assert!(proving::checked(ledger, &wanted, &changed, verifier).is_err());
    // What a leaf proves changed: no check passes it, the prover's own
    // included.
    let mut claimed: sion_prover::WorldProof = sion_core::encode::decode(&proof).unwrap();
    claimed.receipts_root = sion_core::hash::Hash32([7; 32]);
    let claimed = sion_core::encode::encode(&claimed);
    assert!(proving::checked(ledger, &wanted, &claimed, Checker::Prover).is_err());
    assert!(proving::checked(ledger, &wanted, b"not a proof", Checker::Prover).is_err());

    // Only the request's verifiers attest, and only what they checked.
    assert!(proving::attest(&mut session, "mal", task, &proof, None)
        .unwrap_err()
        .to_string()
        .contains("not a verifier"));
    assert!(proving::attest(&mut session, "vera", task, &other, None).is_err());
    let (first, checks) = proving::attest(&mut session, "vera", task, &proof, Some(chain)).unwrap();
    assert!(
        checks.iter().any(|line| line.contains("lane certificates")),
        "{checks:?}"
    );
    let (second, _) = proving::attest(&mut session, "vic", task, &proof, None).unwrap();

    // With one attestation the ledger does not settle; with both, pat is
    // paid what rita escrowed.
    let before = session.balance("pat", AssetId::usd()).unwrap();
    assert!(proving::deliver(&mut session, "pat", task, &proof, &[first]).is_err());
    let paid = proving::deliver(&mut session, "pat", task, &proof, &[first, second]).unwrap();
    assert_eq!(paid, usd(3));
    let gained =
        session.balance("pat", AssetId::usd()).unwrap().micros() + 100_000 - before.micros();
    assert!(
        gained > usd(3).micros() && gained <= usd(3).micros() + 100_000,
        "{gained}"
    );
    let shown = proving::lines(ledger, task).unwrap().join("\n");
    assert!(
        shown.contains(&format!(
            "the World Proof of Pulse {target} ({SYSTEM}), 3 USD escrowed"
        )),
        "{shown}"
    );
    assert!(
        shown.contains("to be delivered by anyone and attested by:")
            && shown.contains("delivered by")
            && shown.contains("and paid at Pulse"),
        "{shown}"
    );
    // It is paid once.
    assert!(proving::deliver(&mut session, "pat", task, &proof, &[first, second]).is_err());
    // A request can name its prover: the task is then that account's to
    // settle, and says so.
    let named = Terms {
        pulse: target + 1,
        prover: Some(session.id("pat")),
        ..terms
    };
    let for_pat = proving::request(&mut session, "rita", &named).unwrap();
    let (_, state) = proving::wanted(ledger, for_pat).unwrap();
    assert_eq!(state.worker, Some(session.id("pat")));
    let shown = proving::lines(ledger, for_pat).unwrap().join("\n");
    assert!(
        shown.contains(&format!(
            "to be delivered by {} and attested by:",
            session.id("pat").short()
        )),
        "{shown}"
    );
    // A task that is no request for a proof is not taken for one.
    assert!(proving::wanted(ledger, sion_core::ids::ObjectId::derive(b"nothing")).is_err());
}
