//! End-to-end scenarios for the state machine.
//!
//! Every Pulse here runs through [`Chain`], which verifies signatures,
//! replays every chunk statelessly from its witness and checks the
//! conservation invariants, so each scenario also exercises the Fold's
//! re-execution path.

use std::collections::BTreeSet;

use sion_assurance::{AtomClass, ClaimStatus, Formula, Policy};
use sion_core::hash::hash_domain;
use sion_core::ids::{AccountId, AssetId, ClaimId, ObjectId, Pulse, Version};
use sion_core::Amount;
use sion_exec::charter::Caveats;
use sion_exec::claim::{claim_id, formulas, ClaimBody, Payload};
use sion_exec::execute::bond_atom;
use sion_exec::market::{sealing_aad, BatchStatus, OrderTerms, Submission, SEALED_ORDER_DOMAIN};
use sion_exec::object::{created_id, ObjectRef};
use sion_exec::pulse::{execute_pulse, PulseInput};
use sion_exec::sim::{sim_beacon, Chain};
use sion_exec::{
    Event, ExecStatus, ObjectData, PulseOutput, SignedTransaction, Transaction, TxKind,
};

fn usd() -> AssetId {
    AssetId::usd()
}

fn units(n: u64) -> Amount {
    Amount::units(n)
}

fn minus(a: Amount, b: Amount) -> Amount {
    a.checked_sub(b).unwrap()
}

fn plus(a: Amount, b: Amount) -> Amount {
    a.checked_add(b).unwrap()
}

fn all_events(out: &PulseOutput) -> Vec<Event> {
    out.effects
        .iter()
        .flat_map(|e| e.events.iter().cloned())
        .chain(out.deferred_events.iter().cloned())
        .collect()
}

fn status_of(out: &PulseOutput, tx: &SignedTransaction) -> ExecStatus {
    out.effects
        .iter()
        .find(|e| e.digest == tx.digest())
        .map(|e| e.status.clone())
        .expect("transaction executed")
}

fn assert_success(out: &PulseOutput, tx: &SignedTransaction) {
    assert_eq!(status_of(out, tx), ExecStatus::Success, "{:?}", tx.tx.kind);
}

fn assert_failed(out: &PulseOutput, tx: &SignedTransaction, needle: &str) {
    match status_of(out, tx) {
        ExecStatus::Failed(reason) => assert!(
            reason.contains(needle),
            "failed for {reason:?}, expected {needle:?}"
        ),
        other => panic!("expected failure containing {needle:?}, got {other:?}"),
    }
}

fn note(subject: &str, formula: Formula) -> ClaimBody {
    ClaimBody {
        schema: "test.note/v1".into(),
        subject: subject.into(),
        payload: Payload::Public(subject.as_bytes().to_vec()),
        formula,
        adjudicator: None,
        detection_permille: 1000,
        valid_for: 100,
        challenge_period: 20,
    }
}

fn reference(chain: &Chain, id: &ObjectId) -> ObjectRef {
    chain.state.objects[id].reference()
}

#[test]
fn transfer_moves_coins_and_pays_the_fee() {
    let mut chain = Chain::new(&["alice", "bob"], 1_000, 2);
    let bob = chain.id("bob");
    let tx = chain
        .tx(
            "alice",
            TxKind::Transfer {
                asset: usd(),
                outputs: vec![(bob, units(100))],
            },
        )
        .unwrap();
    let fee = tx.tx.fee;
    let out = chain.pulse(vec![tx.clone()]).unwrap();
    assert_success(&out, &tx);
    assert_eq!(chain.balance("bob", &usd()), units(1_100));
    assert_eq!(chain.balance("alice", &usd()), minus(units(900), fee));
    assert_eq!(chain.state.treasury().balances[&usd()], fee);
    // Replaying the same certified transaction later changes nothing.
    let root = chain.state.state_root();
    let again = chain.pulse(vec![tx.clone()]).unwrap();
    assert!(matches!(status_of(&again, &tx), ExecStatus::Rejected(_)));
    assert_eq!(chain.state.state_root(), root);
}

#[test]
fn failed_operations_charge_the_fee_and_release_inputs() {
    let mut chain = Chain::new(&["alice", "bob"], 1_000, 1);
    let bob = chain.id("bob");
    let tx = chain
        .tx(
            "alice",
            TxKind::Transfer {
                asset: usd(),
                outputs: vec![(bob, Amount::ZERO)],
            },
        )
        .unwrap();
    let gas = tx.tx.inputs[0];
    let out = chain.pulse(vec![tx.clone()]).unwrap();
    assert_failed(&out, &tx, "zero output");
    assert_eq!(
        chain.balance("alice", &usd()),
        minus(units(1_000), tx.tx.fee)
    );
    assert_eq!(chain.balance("bob", &usd()), units(1_000));
    let coin = &chain.state.objects[&gas.id];
    assert_eq!(
        coin.version,
        Version(gas.version.0 + 1),
        "the gas coin's lock is released"
    );
}

#[test]
fn rejected_transactions_change_nothing() {
    let mut chain = Chain::new(&["alice", "bob"], 1_000, 1);
    let bob = chain.id("bob");
    let mut tx = chain
        .tx(
            "alice",
            TxKind::Transfer {
                asset: usd(),
                outputs: vec![(bob, units(5))],
            },
        )
        .unwrap();
    tx.auth.signatures[0].signature.bytes[7] ^= 1;
    let root = chain.state.state_root();
    let out = chain.pulse(vec![tx.clone()]).unwrap();
    assert!(
        matches!(status_of(&out, &tx), ExecStatus::Rejected(ref r) if r.contains("unauthorized"))
    );
    assert_eq!(chain.state.state_root(), root);
    assert_eq!(out.effects[0].fee, Amount::ZERO);
}

#[test]
fn outputs_can_be_spent_in_the_pulse_that_creates_them() {
    let mut chain = Chain::new(&["alice", "bob", "carol"], 1_000, 1);
    let (bob, carol) = (chain.id("bob"), chain.id("carol"));
    let first = chain
        .tx(
            "alice",
            TxKind::Transfer {
                asset: usd(),
                outputs: vec![(bob, units(300))],
            },
        )
        .unwrap();
    // Bob's new coin is the transfer's second created object (after Alice's change).
    let new_coin = created_id(&first.digest(), 1);
    let config = chain.state.config.clone();
    let mut second = Transaction {
        sender: bob,
        genesis_policy: None,
        inputs: vec![ObjectRef {
            id: new_coin,
            version: Version(0),
        }],
        fee: Amount::ZERO,
        kind: TxKind::Transfer {
            asset: usd(),
            outputs: vec![(carol, units(100))],
        },
        expires_at: Pulse(50),
        nonce: 0,
    };
    second.fee = config.min_fee(second.size());
    let second = second
        .sign(&chain.wallet("bob").keys, &chain.state.config)
        .unwrap();
    // Order of submission does not matter: the canonical order runs the creator first.
    let out = chain.pulse(vec![second.clone(), first.clone()]).unwrap();
    assert_success(&out, &first);
    assert_success(&out, &second);
    assert_eq!(chain.balance("carol", &usd()), units(1_100));
}

#[test]
fn execution_is_deterministic_whatever_the_submission_order() {
    let build = || {
        let mut chain = Chain::new(&["a", "b", "c", "d"], 1_000, 3);
        let ids: Vec<AccountId> = ["a", "b", "c", "d"].iter().map(|n| chain.id(n)).collect();
        let mut txs = Vec::new();
        for (i, name) in ["a", "b", "c", "d"].iter().enumerate() {
            txs.push(
                chain
                    .tx(
                        name,
                        TxKind::Transfer {
                            asset: usd(),
                            outputs: vec![(ids[(i + 1) % 4], units(10 + i as u64))],
                        },
                    )
                    .unwrap(),
            );
            txs.push(
                chain
                    .tx(
                        name,
                        TxKind::IssueClaim {
                            claim: note(&format!("note from {name}"), formulas::dual_signature()),
                        },
                    )
                    .unwrap(),
            );
        }
        (chain, txs)
    };
    let (mut one, txs) = build();
    let (mut two, mut shuffled) = build();
    shuffled.reverse();
    shuffled.swap(1, 5);
    let a = one.pulse(txs).unwrap();
    let b = two.pulse(shuffled).unwrap();
    assert_eq!(a.after, b.after);
    assert_eq!(a.receipts_root, b.receipts_root);
    assert_eq!(a.order, b.order);
    assert_eq!(one.state.knowledge.len(), 4);
}

#[test]
fn claims_join_the_knowledge_log_and_reference_only_the_past() {
    let mut chain = Chain::new(&["alice", "bob"], 1_000, 2);
    let alice = chain.id("alice");
    let base = note("the sky is blue", formulas::dual_signature());
    let base_id = claim_id(&alice, &base);
    let derived = note(
        "so is the sea",
        Formula::and(vec![Formula::claim(base_id), formulas::dual_signature()]),
    );
    let t1 = chain
        .tx(
            "alice",
            TxKind::IssueClaim {
                claim: base.clone(),
            },
        )
        .unwrap();
    let t2 = chain
        .tx(
            "bob",
            TxKind::IssueClaim {
                claim: derived.clone(),
            },
        )
        .unwrap();
    let out = chain.pulse(vec![t1, t2]).unwrap();
    let events = all_events(&out);
    let derived_id = claim_id(&chain.id("bob"), &derived);
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::ClaimIssued { claim, .. } if *claim == base_id)));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::ClaimRejected { claim, .. } if *claim == derived_id)),
        "same-Pulse references are rejected: {events:?}"
    );
    assert_eq!(chain.state.knowledge.len(), 1);
    // In a later Pulse the reference is fine.
    let t3 = chain
        .tx("bob", TxKind::IssueClaim { claim: derived })
        .unwrap();
    chain.pulse(vec![t3]).unwrap();
    assert_eq!(chain.state.knowledge.len(), 2);
    // Claims expire at the end of their window.
    chain.advance_to(Pulse(105)).unwrap();
    assert_eq!(
        chain.state.registry().claim(&base_id).unwrap().status,
        ClaimStatus::Expired
    );
}

#[test]
fn nobody_can_put_another_accounts_bond_at_stake() {
    let mut chain = Chain::new(&["honest", "mallory"], 100_000, 2);
    let honest = chain.id("honest");
    let bond = chain
        .tx(
            "honest",
            TxKind::PostBond {
                class: AtomClass::Economic,
                label: "attestations".into(),
                amount: units(10_000),
            },
        )
        .unwrap();
    chain.pulse(vec![bond]).unwrap();
    let atom = bond_atom(AtomClass::Economic, &honest, "attestations");
    let forged = chain
        .tx(
            "mallory",
            TxKind::IssueClaim {
                claim: note("honest vouches for me", formulas::bonded_statement(atom.id)),
            },
        )
        .unwrap();
    let out = chain.pulse(vec![forged]).unwrap();
    assert!(all_events(&out).iter().any(
        |e| matches!(e, Event::ClaimRejected { reason, .. } if reason.contains("not the issuer's"))
    ));
    assert_eq!(chain.state.knowledge.len(), 0);
}

/// Invoice #1042: a supplier's invoice backed by its own bond or the
/// shipper's delivery attestation; a financier relies on it with insurance;
/// both attestations turn out false and the financier is repaid in full.
#[test]
fn invoice_1042_end_to_end() {
    let mut chain = Chain::new(
        &["supplier", "shipper", "judge", "financier", "buyer"],
        200_000,
        4,
    );
    let (supplier, shipper, judge) = (chain.id("supplier"), chain.id("shipper"), chain.id("judge"));
    let s_atom = bond_atom(AtomClass::Economic, &supplier, "invoices");
    let w_atom = bond_atom(AtomClass::Economic, &shipper, "logistics");
    let j_atom = bond_atom(AtomClass::Adjudicator, &judge, "arbitration");

    // Pulse 1: bonds.
    let bonds = vec![
        chain
            .tx(
                "supplier",
                TxKind::PostBond {
                    class: AtomClass::Economic,
                    label: "invoices".into(),
                    amount: units(30_000),
                },
            )
            .unwrap(),
        chain
            .tx(
                "shipper",
                TxKind::PostBond {
                    class: AtomClass::Economic,
                    label: "logistics".into(),
                    amount: units(20_000),
                },
            )
            .unwrap(),
        chain
            .tx(
                "judge",
                TxKind::PostBond {
                    class: AtomClass::Adjudicator,
                    label: "arbitration".into(),
                    amount: units(100_000),
                },
            )
            .unwrap(),
    ];
    chain.pulse(bonds).unwrap();
    assert_eq!(chain.state.assurance().vault, units(150_000));

    // Pulse 2: the shipper attests delivery.
    let delivery = ClaimBody {
        schema: "trade.delivery/v1".into(),
        subject: "Shipment for invoice #1042 delivered to the buyer".into(),
        payload: Payload::Commitment(hash_domain("test/pod", b"proof-of-delivery-1042")),
        formula: formulas::bonded_statement(w_atom.id),
        adjudicator: Some(j_atom.id),
        detection_permille: 1000,
        valid_for: 200,
        challenge_period: 50,
    };
    let delivery_id = claim_id(&shipper, &delivery);
    let tx = chain
        .tx("shipper", TxKind::IssueClaim { claim: delivery })
        .unwrap();
    chain.pulse(vec![tx]).unwrap();

    // Pulse 3: the supplier issues the invoice claim: its own bond, or the delivery.
    let invoice = ClaimBody {
        schema: "trade.invoice/v1".into(),
        subject: "Invoice #1042: 10,000 USD payable by the buyer".into(),
        payload: Payload::Public(b"invoice-1042".to_vec()),
        formula: Formula::and(vec![
            formulas::dual_signature(),
            Formula::or(vec![Formula::atom(s_atom.id), Formula::claim(delivery_id)]),
        ]),
        adjudicator: Some(j_atom.id),
        detection_permille: 1000,
        valid_for: 200,
        challenge_period: 50,
    };
    let invoice_id = claim_id(&supplier, &invoice);
    let tx = chain
        .tx("supplier", TxKind::IssueClaim { claim: invoice })
        .unwrap();
    chain.pulse(vec![tx]).unwrap();
    let coc =
        sion_assurance::coc::claim_coc(chain.state.registry(), &Policy::standard(), &invoice_id);
    assert_eq!(
        coc.low,
        sion_core::Cost::Finite(units(50_000)),
        "30k + 20k across independent routes"
    );

    // Pulse 4: the financier relies on 10,000 USD (κ = 2 reserves 20,000);
    // relying on 30,000 would need 60,000 and is refused.
    let until = Pulse(chain.state.pulse.0 + 100);
    let accept = chain
        .tx(
            "financier",
            TxKind::Accept {
                claim: invoice_id,
                value_at_risk: units(10_000),
                policy: Policy::standard(),
                until,
                nonce: 0,
            },
        )
        .unwrap();
    let greedy = chain
        .tx(
            "financier",
            TxKind::Accept {
                claim: invoice_id,
                value_at_risk: units(30_000),
                policy: Policy::standard(),
                until,
                nonce: 1,
            },
        )
        .unwrap();
    let out = chain.pulse(vec![accept.clone(), greedy.clone()]).unwrap();
    assert_success(&out, &accept);
    assert_failed(&out, &greedy, "not accepted");
    let reserved: Vec<Amount> = all_events(&out)
        .iter()
        .filter_map(|e| match e {
            Event::Reserved { amount, .. } => Some(*amount),
            _ => None,
        })
        .collect();
    assert_eq!(reserved, vec![units(20_000)]);
    let financier_before = chain.balance("financier", &usd());

    // Pulses 5-6: the buyer shows the supplier's own attestation false.
    let challenge =
        |chain: &mut Chain, claim: ClaimId, target: sion_core::ids::AtomId, tag: &str| {
            chain
                .tx(
                    "buyer",
                    TxKind::Challenge {
                        claim,
                        target,
                        evidence: hash_domain("test/evidence", tag.as_bytes()),
                        deposit: units(1_000),
                    },
                )
                .unwrap()
        };
    let opened = |out: &PulseOutput| {
        all_events(out)
            .into_iter()
            .find_map(|e| match e {
                Event::ChallengeOpened { challenge, .. } => Some(challenge),
                _ => None,
            })
            .expect("challenge opened")
    };
    let c1 = challenge(&mut chain, invoice_id, s_atom.id, "goods never shipped");
    let out = chain.pulse(vec![c1]).unwrap();
    let first = opened(&out);
    let rule = chain
        .tx(
            "judge",
            TxKind::Rule {
                challenge: first,
                upheld: true,
            },
        )
        .unwrap();
    let out = chain.pulse(vec![rule]).unwrap();
    let events = all_events(&out);
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::Slashed { amount, .. } if *amount == units(30_000))));
    assert!(
        !events.iter().any(|e| matches!(e, Event::Falsified { .. })),
        "the delivery route still holds"
    );
    assert_eq!(
        chain.state.registry().claim(&invoice_id).unwrap().status,
        ClaimStatus::Active
    );

    // Challenging the shipper through the invoice is not possible: its bond is
    // at stake only in its own claim, under its own adjudicator.
    let indirect = challenge(&mut chain, invoice_id, w_atom.id, "no delivery");
    let out = chain.pulse(vec![indirect.clone()]).unwrap();
    assert_failed(&out, &indirect, "direct assumption");

    // Pulses 8-9: the delivery attestation is false too.
    let c2 = challenge(&mut chain, delivery_id, w_atom.id, "no delivery");
    let out = chain.pulse(vec![c2]).unwrap();
    let second = opened(&out);
    let rule = chain
        .tx(
            "judge",
            TxKind::Rule {
                challenge: second,
                upheld: true,
            },
        )
        .unwrap();
    let out = chain.pulse(vec![rule]).unwrap();
    let events = all_events(&out);
    let falsified: BTreeSet<ClaimId> = events
        .iter()
        .filter_map(|e| match e {
            Event::Falsified { claim, .. } => Some(*claim),
            _ => None,
        })
        .collect();
    assert_eq!(falsified, [delivery_id, invoice_id].into_iter().collect());
    let compensated: Vec<Amount> = events
        .iter()
        .filter_map(|e| match e {
            Event::Compensated { amount, .. } => Some(*amount),
            _ => None,
        })
        .collect();
    assert_eq!(
        compensated,
        vec![units(10_000)],
        "paid the full value at risk"
    );
    assert_eq!(
        chain.balance("financier", &usd()),
        plus(financier_before, units(10_000))
    );
    // The buyer got both deposits back plus half of every released pool.
    assert!(chain.balance("buyer", &usd()) > units(200_000));
}

fn charter_caveats(vendor: AccountId) -> Caveats {
    Caveats {
        payees: Some([vendor].into_iter().collect()),
        max_per_payment: units(100),
        // 250 USD at once, refilled at 250 USD per 1,000 Pulses.
        burst: units(250),
        rate: Amount(250_000),
        expires_at: Pulse(10_000),
        evidence_above: None,
        evidence_policy: Policy::standard(),
        receipts: true,
        may_delegate: true,
    }
}

#[test]
fn agent_charters_enforce_caveats_delegation_and_revocation() {
    let mut chain = Chain::new(
        &["principal", "agent", "helper", "vendor", "stranger"],
        10_000,
        4,
    );
    let (agent, helper, vendor, stranger) = (
        chain.id("agent"),
        chain.id("helper"),
        chain.id("vendor"),
        chain.id("stranger"),
    );
    let create = chain
        .tx(
            "principal",
            TxKind::CreateCharter {
                agent,
                caveats: charter_caveats(vendor),
                funding: units(1_000),
                agent_bond: units(500),
            },
        )
        .unwrap();
    let out = chain.pulse(vec![create.clone()]).unwrap();
    assert_success(&out, &create);
    let (charter, allotment) = all_events(&out)
        .into_iter()
        .find_map(|e| match e {
            Event::CharterCreated { charter, allotment } => Some((charter, allotment)),
            _ => None,
        })
        .unwrap();

    let pay = |chain: &mut Chain,
               who: &str,
               charter: ObjectId,
               allotment: ObjectId,
               payee: AccountId,
               amount: u64| {
        let input = reference(chain, &allotment);
        chain
            .tx_with(
                who,
                TxKind::AgentPay {
                    charter,
                    payee,
                    amount: units(amount),
                    purpose: "inference credits".into(),
                    evidence: None,
                },
                vec![input],
            )
            .unwrap()
    };
    let vendor_start = chain.balance("vendor", &usd());

    let ok = pay(&mut chain, "agent", charter, allotment, vendor, 100);
    let out = chain.pulse(vec![ok.clone()]).unwrap();
    assert_success(&out, &ok);
    let receipt = all_events(&out)
        .into_iter()
        .find_map(|e| match e {
            Event::AgentPaid { receipt, .. } => receipt,
            _ => None,
        })
        .expect("receipts are on");
    assert!(
        chain.state.claim(&receipt).is_some(),
        "the receipt joined the knowledge log"
    );

    for (payee, amount, why) in [
        (stranger, 10, "payee not allowed"),
        (vendor, 150, "per-payment limit"),
    ] {
        let bad = pay(&mut chain, "agent", charter, allotment, payee, amount);
        let out = chain.pulse(vec![bad.clone()]).unwrap();
        assert_failed(&out, &bad, why);
    }
    let ok = pay(&mut chain, "agent", charter, allotment, vendor, 100);
    let out = chain.pulse(vec![ok.clone()]).unwrap();
    assert_success(&out, &ok);
    // The bucket held 250; two payments of 100 a few Pulses apart leave
    // about 51 in it.
    let over = pay(&mut chain, "agent", charter, allotment, vendor, 100);
    let out = chain.pulse(vec![over.clone()]).unwrap();
    assert_failed(&out, &over, "the bucket holds");

    // Delegation only narrows.
    let mut wide = charter_caveats(vendor);
    wide.max_per_payment = units(500);
    let input = reference(&chain, &allotment);
    let bad = chain
        .tx_with(
            "agent",
            TxKind::Delegate {
                charter,
                sub_agent: helper,
                caveats: wide,
                amount: units(100),
            },
            vec![input],
        )
        .unwrap();
    let out = chain.pulse(vec![bad.clone()]).unwrap();
    assert_failed(&out, &bad, "at least as strict");
    let mut narrow = charter_caveats(vendor);
    narrow.max_per_payment = units(20);
    narrow.burst = units(40);
    narrow.rate = Amount(40_000);
    narrow.may_delegate = false;
    let input = reference(&chain, &allotment);
    let delegate = chain
        .tx_with(
            "agent",
            TxKind::Delegate {
                charter,
                sub_agent: helper,
                caveats: narrow,
                amount: units(100),
            },
            vec![input],
        )
        .unwrap();
    let out = chain.pulse(vec![delegate.clone()]).unwrap();
    assert_success(&out, &delegate);
    let (child, child_allotment) = all_events(&out)
        .into_iter()
        .find_map(|e| match e {
            Event::CharterCreated { charter, allotment } => Some((charter, allotment)),
            _ => None,
        })
        .unwrap();
    let sub = pay(&mut chain, "helper", child, child_allotment, vendor, 20);
    let out = chain.pulse(vec![sub.clone()]).unwrap();
    assert_success(&out, &sub);

    // Revocation takes effect when it commits: a payment certified in the
    // same Pulse still executes; afterwards the charter and its children stop.
    chain
        .advance_to(Pulse(chain.state.pulse.0 + 1_000))
        .unwrap(); // the bucket refills
    let revoke = chain
        .tx("principal", TxKind::RevokeCharter { charter })
        .unwrap();
    let racing = pay(&mut chain, "agent", charter, allotment, vendor, 50);
    let out = chain.pulse(vec![revoke.clone(), racing.clone()]).unwrap();
    assert_success(&out, &revoke);
    assert_success(&out, &racing);
    let late = pay(&mut chain, "agent", charter, allotment, vendor, 10);
    let late_sub = pay(&mut chain, "helper", child, child_allotment, vendor, 10);
    let out = chain.pulse(vec![late.clone(), late_sub.clone()]).unwrap();
    assert_failed(&out, &late, "revoked");
    assert_failed(&out, &late_sub, "revoked");
    assert_eq!(
        chain.balance("vendor", &usd()),
        plus(vendor_start, units(270))
    );

    // What is left goes back to the root principal, whose funds it was:
    // 1,000 − 250 paid − 100 delegated = 650 at the root, 100 − 20 = 80 below.
    let principal_before = chain.balance("principal", &usd());
    let reclaim_root = chain
        .tx("principal", TxKind::ReclaimCharter { charter })
        .unwrap();
    let reclaim_child = chain
        .tx("principal", TxKind::ReclaimCharter { charter: child })
        .unwrap();
    let fees = plus(reclaim_root.tx.fee, reclaim_child.tx.fee);
    let out = chain
        .pulse(vec![reclaim_root.clone(), reclaim_child.clone()])
        .unwrap();
    assert_success(&out, &reclaim_root);
    assert_success(&out, &reclaim_child);
    assert_eq!(
        chain.balance("principal", &usd()),
        minus(plus(principal_before, units(730)), fees)
    );
    // A live charter cannot be reclaimed.
    assert!(chain.state.objects[&allotment].is_tombstone());
}

/// The (agent, allotment) pairs an output's events created, in order.
fn spawned_agents(out: &PulseOutput) -> Vec<(ObjectId, ObjectId)> {
    all_events(out)
        .into_iter()
        .filter_map(|e| match e {
            Event::AgentSpawned {
                agent, allotment, ..
            } => Some((agent, allotment)),
            _ => None,
        })
        .collect()
}

#[test]
fn an_agent_swarm_is_contained_and_stopped_from_any_ancestor() {
    use sion_exec::charter::{card_commitment, Spawn};
    let mut chain = Chain::new(
        &["acme", "orchestrator", "w1", "w2", "s1", "vendor"],
        10_000,
        4,
    );
    let ids: Vec<AccountId> = ["orchestrator", "w1", "w2", "s1", "vendor"]
        .iter()
        .map(|n| chain.id(n))
        .collect();
    let (orchestrator, w1, w2, s1, vendor) = (ids[0], ids[1], ids[2], ids[3], ids[4]);
    let register = chain
        .tx(
            "acme",
            TxKind::RegisterAgent {
                agent: orchestrator,
                card: card_commitment(b"orchestrator"),
                caveats: charter_caveats(vendor),
                funding: units(1_000),
                agent_bond: Amount::ZERO,
            },
        )
        .unwrap();
    let out = chain.pulse(vec![register.clone()]).unwrap();
    assert_success(&out, &register);
    let (root, root_allotment) = all_events(&out)
        .into_iter()
        .find_map(|e| match e {
            Event::AgentRegistered {
                agent, allotment, ..
            } => Some((agent, allotment)),
            _ => None,
        })
        .unwrap();

    let spawn = |chain: &mut Chain, who: &str, parent, allotment, children: Vec<Spawn>| {
        let input = reference(chain, &allotment);
        chain
            .tx_with(
                who,
                TxKind::SpawnAgents {
                    charter: parent,
                    children,
                },
                vec![input],
            )
            .unwrap()
    };
    // Buckets are carved like budgets: bursts of 100 + 50 of the root's 250,
    // then 40 of w1's 100, each with the rate of that much per 1,000 Pulses.
    let child = |agent, budget: u64, burst: u64| {
        let mut caveats = charter_caveats(vendor);
        caveats.burst = units(burst);
        caveats.rate = Amount(u128::from(burst) * 1_000);
        Spawn {
            agent: Some(agent),
            card: card_commitment(b"worker"),
            caveats,
            budget: units(budget),
        }
    };
    let tx = spawn(
        &mut chain,
        "orchestrator",
        root,
        root_allotment,
        vec![child(w1, 200, 100), child(w2, 100, 50)],
    );
    let out = chain.pulse(vec![tx.clone()]).unwrap();
    assert_success(&out, &tx);
    let workers = spawned_agents(&out);
    let ((a, a_allotment), (b, b_allotment)) = (workers[0], workers[1]);
    let tx = spawn(&mut chain, "w1", a, a_allotment, vec![child(s1, 50, 40)]);
    let out = chain.pulse(vec![tx.clone()]).unwrap();
    assert_success(&out, &tx);
    let (s, s_allotment) = spawned_agents(&out)[0];

    let pay = |chain: &mut Chain, who: &str, charter, allotment, amount: u64| {
        let input = reference(chain, &allotment);
        chain
            .tx_with(
                who,
                TxKind::AgentPay {
                    charter,
                    payee: vendor,
                    amount: units(amount),
                    purpose: "a tool call".into(),
                    evidence: None,
                },
                vec![input],
            )
            .unwrap()
    };
    // Siblings and a grandchild pay in the same Pulse: they write only their
    // own allotments, so all of them execute.
    let txs = vec![
        pay(&mut chain, "w1", a, a_allotment, 10),
        pay(&mut chain, "w2", b, b_allotment, 10),
        pay(&mut chain, "s1", s, s_allotment, 10),
    ];
    let out = chain.pulse(txs.clone()).unwrap();
    for tx in &txs {
        assert_success(&out, tx);
    }

    // A suspension takes effect when it executes: a payment in the same
    // Pulse runs first (owned transactions before contended ones); the next
    // one fails, two levels down.
    let suspend = chain
        .tx("acme", TxKind::SuspendAgent { charter: root })
        .unwrap();
    let racing = pay(&mut chain, "s1", s, s_allotment, 1);
    let out = chain.pulse(vec![suspend.clone(), racing.clone()]).unwrap();
    assert_success(&out, &suspend);
    assert_success(&out, &racing);
    let stopped = pay(&mut chain, "s1", s, s_allotment, 1);
    let out = chain.pulse(vec![stopped.clone()]).unwrap();
    assert_failed(&out, &stopped, "suspended");
    let resume = chain
        .tx("acme", TxKind::ResumeAgent { charter: root })
        .unwrap();
    chain.pulse(vec![resume]).unwrap();
    let again = pay(&mut chain, "s1", s, s_allotment, 1);
    let out = chain.pulse(vec![again.clone()]).unwrap();
    assert_success(&out, &again);

    // The backer revokes w2, which it is not the principal of, and takes its
    // unspent budget back.
    let revoke = chain
        .tx("acme", TxKind::RevokeCharter { charter: b })
        .unwrap();
    chain.pulse(vec![revoke]).unwrap();
    let before = chain.balance("acme", &usd());
    let reclaim = chain
        .tx("acme", TxKind::ReclaimCharter { charter: b })
        .unwrap();
    let out = chain.pulse(vec![reclaim.clone()]).unwrap();
    assert_success(&out, &reclaim);
    assert_eq!(
        chain.balance("acme", &usd()),
        minus(plus(before, units(90)), reclaim.tx.fee),
        "w2's 100 USD budget less its 10 USD payment"
    );
    match &chain.state.objects[&s].data {
        ObjectData::Charter(c) => {
            let identity = c.identity.as_ref().expect("an identity");
            assert_eq!(identity.depth, 2);
            assert_eq!(c.parent, Some(a));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn bounded_totals_split_into_single_writer_allotments() {
    let mut chain = Chain::new(&["manager", "w1", "w2", "shop"], 10_000, 2);
    let (w1, w2, shop) = (chain.id("w1"), chain.id("w2"), chain.id("shop"));
    let create = chain
        .tx(
            "manager",
            TxKind::CreateBounded {
                label: "team budget".into(),
                asset: usd(),
                total: units(1_000),
                allotments: vec![(w1, units(400)), (w2, units(400))],
            },
        )
        .unwrap();
    let out = chain.pulse(vec![create]).unwrap();
    let (bounded, allotments) = all_events(&out)
        .into_iter()
        .find_map(|e| match e {
            Event::BoundedCreated {
                bounded,
                allotments,
            } => Some((bounded, allotments)),
            _ => None,
        })
        .unwrap();
    // Both writers spend in the same Pulse without coordinating.
    let spend1 = {
        let input = reference(&chain, &allotments[0]);
        chain
            .tx_with(
                "w1",
                TxKind::SpendAllotment {
                    to: shop,
                    amount: units(300),
                },
                vec![input],
            )
            .unwrap()
    };
    let spend2 = {
        let input = reference(&chain, &allotments[1]);
        chain
            .tx_with(
                "w2",
                TxKind::SpendAllotment {
                    to: shop,
                    amount: units(400),
                },
                vec![input],
            )
            .unwrap()
    };
    let out = chain.pulse(vec![spend1.clone(), spend2.clone()]).unwrap();
    assert_success(&out, &spend1);
    assert_success(&out, &spend2);
    // An allotment cannot be overspent.
    let over = {
        let input = reference(&chain, &allotments[0]);
        chain
            .tx_with(
                "w1",
                TxKind::SpendAllotment {
                    to: shop,
                    amount: units(101),
                },
                vec![input],
            )
            .unwrap()
    };
    let out = chain.pulse(vec![over.clone()]).unwrap();
    assert_failed(&out, &over, "insufficient");
    // Returning the rest, then rebalancing through consensus.
    let back = {
        let input = reference(&chain, &allotments[0]);
        chain
            .tx_with("w1", TxKind::ReturnAllotment, vec![input])
            .unwrap()
    };
    chain.pulse(vec![back]).unwrap();
    let rebalance = chain
        .tx(
            "manager",
            TxKind::Rebalance {
                bounded,
                holder: w2,
                amount: units(300),
            },
        )
        .unwrap();
    let out = chain.pulse(vec![rebalance.clone()]).unwrap();
    assert_success(&out, &rebalance);
    let ObjectData::Bounded(state) = &chain.state.objects[&bounded].data else {
        panic!()
    };
    assert_eq!(
        state.unallocated,
        units(0),
        "200 unallocated + 100 returned - 300 rebalanced"
    );
    assert_eq!(chain.balance("shop", &usd()), units(10_700));
}

fn gpu() -> AssetId {
    AssetId::derive(b"gpu-hour")
}

fn cents(c: u128) -> Amount {
    Amount(c * 10_000)
}

fn create_market(
    chain: &mut Chain,
    committee: Vec<sion_crypto::KemPublicKey>,
    threshold: u8,
) -> ObjectId {
    let tx = chain
        .tx(
            "operator",
            TxKind::CreateMarket {
                label: "gpu-hours".into(),
                good: gpu(),
                numeraire: usd(),
                batch_length: 4,
                clear_deadline: 3,
                committee,
                threshold,
            },
        )
        .unwrap();
    let out = chain.pulse(vec![tx]).unwrap();
    all_events(&out)
        .into_iter()
        .find_map(|e| match e {
            Event::MarketCreated { market } => Some(market),
            _ => None,
        })
        .unwrap()
}

fn order(
    chain: &mut Chain,
    who: &str,
    market: ObjectId,
    side: sion_clearing::Side,
    quantity: u64,
    price: u128,
) -> SignedTransaction {
    chain
        .tx(
            who,
            TxKind::SubmitOrder {
                market,
                submission: Submission::Plain(OrderTerms {
                    side,
                    quantity,
                    limit_price: cents(price),
                }),
            },
        )
        .unwrap()
}

fn sealed_batch(chain: &Chain, market: ObjectId) -> (u64, sion_clearing::Batch) {
    let ObjectData::Market(state) = &chain.state.objects[&market].data else {
        panic!()
    };
    let (index, batch) = state
        .batches
        .iter()
        .rev()
        .find(|(_, b)| matches!(b.status, BatchStatus::Sealed { .. }))
        .expect("a sealed batch");
    let BatchStatus::Sealed { beacon, at } = batch.status else {
        unreachable!()
    };
    (*index, state.clearing_batch(batch, beacon, at))
}

#[test]
fn markets_clear_in_certified_batches_at_one_price() {
    use sion_clearing::Side;
    let mut chain = Chain::new(
        &["operator", "agent", "lab-a", "lab-b", "solver"],
        10_000,
        4,
    );
    chain
        .fund(&[("lab-a", gpu(), units(5)), ("lab-b", gpu(), units(6))])
        .unwrap();
    let market = create_market(&mut chain, Vec::new(), 0);
    // Wait for the start of a batch window so all three orders share one batch.
    while (chain.state.pulse.0 + 1) % 4 != 0 {
        chain.pulse(Vec::new()).unwrap();
    }
    let orders = vec![
        order(&mut chain, "agent", market, Side::Buy, 8, 210),
        order(&mut chain, "lab-a", market, Side::Sell, 5, 180),
        order(&mut chain, "lab-b", market, Side::Sell, 6, 200),
    ];
    chain.pulse(orders).unwrap();
    let agent_usd = chain.balance("agent", &usd());
    let lab_a_usd = chain.balance("lab-a", &usd());
    chain.advance_to(Pulse(chain.state.pulse.0 + 3)).unwrap();
    let (index, batch) = sealed_batch(&chain, market);
    let result = sion_clearing::clear(&batch).unwrap();
    assert_eq!(result.price, Some(cents(200)));
    let clear = chain
        .tx(
            "solver",
            TxKind::ClearBatch {
                market,
                batch: index,
                result,
            },
        )
        .unwrap();
    let out = chain.pulse(vec![clear.clone()]).unwrap();
    assert_success(&out, &clear);
    assert_eq!(chain.balance("agent", &gpu()), units(8));
    // The agent escrowed 8 × 2.10 and paid 8 × 2.00: 0.80 USD came back.
    assert_eq!(chain.balance("agent", &usd()), plus(agent_usd, cents(80)));
    assert_eq!(chain.balance("lab-a", &usd()), plus(lab_a_usd, units(10)));
    assert_eq!(chain.balance("lab-b", &gpu()), units(3));
    // A second, equally valid clearing is refused: the batch is settled.
    let ObjectData::Market(state) = &chain.state.objects[&market].data else {
        panic!()
    };
    assert!(matches!(
        state.batches[&index].status,
        BatchStatus::Cleared { volume: 8, .. }
    ));
}

#[test]
fn uncleared_batches_are_refunded_at_the_deadline() {
    use sion_clearing::Side;
    let mut chain = Chain::new(&["operator", "agent", "lab-a"], 10_000, 4);
    chain.fund(&[("lab-a", gpu(), units(5))]).unwrap();
    let market = create_market(&mut chain, Vec::new(), 0);
    let orders = vec![
        order(&mut chain, "agent", market, Side::Buy, 2, 300),
        order(&mut chain, "lab-a", market, Side::Sell, 5, 100),
    ];
    let fees: Amount = Amount(
        orders
            .iter()
            .filter(|t| t.tx.sender == chain.id("agent"))
            .map(|t| t.tx.fee.0)
            .sum(),
    );
    chain.pulse(orders).unwrap();
    chain.advance_to(Pulse(chain.state.pulse.0 + 10)).unwrap();
    let ObjectData::Market(state) = &chain.state.objects[&market].data else {
        panic!()
    };
    assert!(state
        .batches
        .values()
        .all(|b| b.status == BatchStatus::Refunded));
    assert_eq!(chain.balance("agent", &usd()), minus(units(10_000), fees));
    assert_eq!(chain.balance("lab-a", &gpu()), units(5));
}

#[test]
fn sealed_orders_are_revealed_by_the_committee_and_cleared() {
    use sion_clearing::Side;
    use sion_crypto::{CommitteeEnvelope, KemKeypair};
    let mut chain = Chain::new(
        &["operator", "agent", "lab-a", "solver", "relay"],
        10_000,
        4,
    );
    chain.fund(&[("lab-a", gpu(), units(5))]).unwrap();
    let committee: Vec<KemKeypair> = (1..=3u8).map(|i| KemKeypair::from_seed(&[i; 32])).collect();
    let market = create_market(
        &mut chain,
        committee.iter().map(|k| k.public().clone()).collect(),
        2,
    );
    while (chain.state.pulse.0 + 1) % 4 != 0 {
        chain.pulse(Vec::new()).unwrap();
    }
    let ObjectData::Market(state) = &chain.state.objects[&market].data else {
        panic!()
    };
    let domain = state.domain;
    let index = state.batch_index(chain.state.pulse.next());
    let aad = sealing_aad(&domain, index);
    let mut rng = sion_core::rng::seeded(7);
    let keys: Vec<_> = committee.iter().map(|k| k.public().clone()).collect();
    let terms = OrderTerms {
        side: Side::Buy,
        quantity: 4,
        limit_price: cents(250),
    };
    let envelope = CommitteeEnvelope::seal(
        &keys,
        2,
        SEALED_ORDER_DOMAIN,
        &aad,
        &sion_core::encode(&terms),
        &mut rng,
    )
    .unwrap();
    let sealed = chain
        .tx(
            "agent",
            TxKind::SubmitOrder {
                market,
                submission: Submission::Sealed {
                    envelope: envelope.clone(),
                    side: Side::Buy,
                    escrow: units(12),
                },
            },
        )
        .unwrap();
    // A sealed order to some other committee is refused.
    let mine: Vec<_> = (7..=9u8)
        .map(|i| KemKeypair::from_seed(&[i; 32]).public().clone())
        .collect();
    let own = CommitteeEnvelope::seal(
        &mine,
        2,
        SEALED_ORDER_DOMAIN,
        &aad,
        &sion_core::encode(&terms),
        &mut rng,
    )
    .unwrap();
    let selective = chain
        .tx(
            "agent",
            TxKind::SubmitOrder {
                market,
                submission: Submission::Sealed {
                    envelope: own,
                    side: Side::Buy,
                    escrow: units(12),
                },
            },
        )
        .unwrap();
    let ask = order(&mut chain, "lab-a", market, Side::Sell, 5, 150);
    let out = chain.pulse(vec![sealed, selective.clone(), ask]).unwrap();
    assert_failed(&out, &selective, "market's committee");
    chain.advance_to(Pulse(chain.state.pulse.0 + 3)).unwrap();

    // Before reveal, the price is unknown and clearing early is refused.
    let (index, batch) = sealed_batch(&chain, market);
    assert_eq!(batch.orders.len(), 1, "only the plain ask is visible");
    let early = sion_clearing::clear(&batch).unwrap();
    let premature = chain
        .tx(
            "solver",
            TxKind::ClearBatch {
                market,
                batch: index,
                result: early,
            },
        )
        .unwrap();
    // Two of three members publish their shares; anyone can relay them.
    let order_id = {
        let ObjectData::Market(state) = &chain.state.objects[&market].data else {
            panic!()
        };
        state.batches[&index]
            .orders
            .iter()
            .find(|o| o.sealed.is_some())
            .unwrap()
            .id
    };
    let shares: Vec<_> = [0u8, 2]
        .iter()
        .map(|i| {
            envelope
                .open_share(*i, &committee[usize::from(*i)], SEALED_ORDER_DOMAIN, &aad)
                .unwrap()
        })
        .collect();
    let reveal = chain
        .tx(
            "relay",
            TxKind::RevealBatch {
                market,
                batch: index,
                shares: vec![(order_id, shares)],
            },
        )
        .unwrap();
    let out = chain
        .pulse(vec![premature.clone(), reveal.clone()])
        .unwrap();
    assert_failed(&out, &premature, "still being revealed");
    assert_success(&out, &reveal);
    let (index, batch) = sealed_batch(&chain, market);
    assert_eq!(batch.orders.len(), 2);
    let result = sion_clearing::clear(&batch).unwrap();
    assert_eq!(result.volume, 4);
    let clear = chain
        .tx(
            "solver",
            TxKind::ClearBatch {
                market,
                batch: index,
                result,
            },
        )
        .unwrap();
    let out = chain.pulse(vec![clear.clone()]).unwrap();
    assert_success(&out, &clear);
    assert_eq!(chain.balance("agent", &gpu()), units(4));
}

#[test]
fn policies_rotate_without_changing_the_account() {
    let mut chain = Chain::new(&["alice", "bob"], 1_000, 2);
    let alice = chain.id("alice");
    let fresh = sion_exec::Wallet::named("alice-rotated");
    let rotate = chain
        .tx(
            "alice",
            TxKind::RotatePolicy {
                policy: fresh.policy.clone(),
                viewing_key: None,
            },
        )
        .unwrap();
    let out = chain.pulse(vec![rotate.clone()]).unwrap();
    assert_success(&out, &rotate);
    // Old keys no longer authorize the account.
    let bob = chain.id("bob");
    let stale = chain
        .tx(
            "alice",
            TxKind::Transfer {
                asset: usd(),
                outputs: vec![(bob, units(1))],
            },
        )
        .unwrap();
    let out = chain.pulse(vec![stale.clone()]).unwrap();
    assert!(matches!(status_of(&out, &stale), ExecStatus::Rejected(_)));
    // New keys do, for the same account.
    let mut rotated = sion_exec::Wallet::with_keys(fresh.keys.clone());
    rotated.id = alice;
    chain.wallets.insert("alice".into(), rotated);
    let ok = chain
        .tx(
            "alice",
            TxKind::Transfer {
                asset: usd(),
                outputs: vec![(bob, units(1))],
            },
        )
        .unwrap();
    let out = chain.pulse(vec![ok.clone()]).unwrap();
    assert_success(&out, &ok);
}

#[test]
fn tampered_witnesses_and_statements_are_caught() {
    let mut chain = Chain::new(&["alice", "bob"], 1_000, 2);
    let bob = chain.id("bob");
    let tx = chain
        .tx(
            "alice",
            TxKind::Transfer {
                asset: usd(),
                outputs: vec![(bob, units(10))],
            },
        )
        .unwrap();
    chain.pulse(vec![tx]).unwrap();
    let record = chain.history.last().unwrap().clone();
    let trace = &record.traces[0];
    sion_exec::replay_chunk(trace).unwrap();

    let mut dropped = trace.clone();
    dropped.witness.objects.pop();
    assert!(
        sion_exec::replay_chunk(&dropped).is_err(),
        "an incomplete witness invalidates the chunk"
    );

    let mut wrong_root = trace.clone();
    wrong_root.statement.after.state_root = hash_domain("test", b"forged");
    assert!(sion_exec::replay_chunk(&wrong_root).is_err());

    let mut forged_object = trace.clone();
    if let Some((_, Some(object), _)) = forged_object
        .witness
        .objects
        .iter_mut()
        .find(|(_, o, _)| o.as_ref().is_some_and(|o| o.as_coin().is_some()))
    {
        if let ObjectData::Coin(coin) = &mut object.data {
            coin.amount = units(1_000_000);
        }
    }
    assert!(
        sion_exec::replay_chunk(&forged_object).is_err(),
        "witnessed objects must match the root"
    );

    let mut other_txs = trace.clone();
    other_txs.transactions.clear();
    assert!(sion_exec::replay_chunk(&other_txs).is_err());
}

#[test]
fn contended_transactions_cannot_ride_the_owned_path() {
    let mut chain = Chain::new(&["alice"], 1_000, 2);
    let release = chain
        .tx(
            "alice",
            TxKind::Release {
                reservation: hash_domain("test", b"none"),
            },
        )
        .unwrap();
    let pulse = chain.state.pulse.next();
    let input = PulseInput {
        pulse,
        beacon: sim_beacon(pulse),
        owned: vec![release],
        contended: Vec::new(),
        forced: Vec::new(),
    };
    let before = chain.state.commitment();
    assert!(execute_pulse(&mut chain.state, &input, true).is_err());
    assert_eq!(
        chain.state.commitment(),
        before,
        "a rejected Pulse leaves the state untouched"
    );
}

/// Found by proof stack B (N-version testing): user-chosen periods, rates
/// and grants must never reach an arithmetic overflow, and a lineage grows
/// no deeper than its limit.
#[test]
fn bounds_keep_user_chosen_numbers_far_from_overflow() {
    use sion_exec::charter::MAX_LINEAGE_DEPTH;
    use sion_exec::market::MAX_PERIOD;
    use sion_exec::object::well_known;
    use sion_exec::sim::FAUCET;
    use sion_exec::tx::MAX_GRANT;

    let mut chain = Chain::new(&["operator", "principal", "agent", "vendor"], 10_000, 4);
    let (agent, vendor) = (chain.id("agent"), chain.id("vendor"));

    // Market periods: a clearing deadline of u64::MAX used to overflow when
    // the batch sealed (a panic in debug builds, a wrap in release builds).
    for (deadline, ok) in [
        (u64::MAX, false),
        (MAX_PERIOD + 1, false),
        (MAX_PERIOD, true),
    ] {
        let tx = chain
            .tx(
                "operator",
                TxKind::CreateMarket {
                    label: "periods".into(),
                    good: gpu(),
                    numeraire: usd(),
                    batch_length: 2,
                    clear_deadline: deadline,
                    committee: Vec::new(),
                    threshold: 0,
                },
            )
            .unwrap();
        let out = chain.pulse(vec![tx.clone()]).unwrap();
        if ok {
            assert_success(&out, &tx);
        } else {
            assert_failed(&out, &tx, "between 1 and");
        }
    }

    // Faucet grants are capped, and faucet transactions pay no fee.
    let faucet = chain.id(FAUCET);
    let too_much = chain.fund(&[("vendor", usd(), plus(MAX_GRANT, Amount(1)))]);
    let out = too_much.unwrap();
    assert!(matches!(
        &out.effects[0].status,
        ExecStatus::Failed(reason) if reason.contains("at most")
    ));
    let paying = Transaction {
        sender: faucet,
        genesis_policy: None,
        inputs: vec![reference(&chain, &well_known::account(&faucet))],
        fee: units(1),
        kind: TxKind::Faucet {
            grants: vec![(vendor, usd(), units(1))],
        },
        expires_at: Pulse(chain.state.pulse.0 + 10),
        nonce: 99,
    }
    .sign(&chain.wallet(FAUCET).keys, &chain.state.config)
    .unwrap();
    let out = chain.pulse(vec![paying.clone()]).unwrap();
    assert!(matches!(status_of(&out, &paying), ExecStatus::Rejected(_)));

    // A bucket that never refills is a lifetime limit, and one that refills
    // by u128::MAX a Pulse saturates instead of overflowing.
    let mut caveats = charter_caveats(vendor);
    caveats.rate = Amount::ZERO;
    caveats.burst = units(150);
    let create = chain
        .tx(
            "principal",
            TxKind::CreateCharter {
                agent,
                caveats: caveats.clone(),
                funding: units(1_000),
                agent_bond: Amount::ZERO,
            },
        )
        .unwrap();
    let out = chain.pulse(vec![create.clone()]).unwrap();
    assert_success(&out, &create);
    let (root, root_allotment) = all_events(&out)
        .into_iter()
        .find_map(|e| match e {
            Event::CharterCreated { charter, allotment } => Some((charter, allotment)),
            _ => None,
        })
        .unwrap();
    let pay = |chain: &mut Chain, charter: ObjectId, allotment: ObjectId, amount: u64| {
        let input = reference(chain, &allotment);
        chain
            .tx_with(
                "agent",
                TxKind::AgentPay {
                    charter,
                    payee: vendor,
                    amount: units(amount),
                    purpose: "bounds".into(),
                    evidence: None,
                },
                vec![input],
            )
            .unwrap()
    };
    let first = pay(&mut chain, root, root_allotment, 100);
    let out = chain.pulse(vec![first.clone()]).unwrap();
    assert_success(&out, &first);
    chain.advance_to(Pulse(chain.state.pulse.0 + 50)).unwrap();
    let second = pay(&mut chain, root, root_allotment, 100);
    let out = chain.pulse(vec![second.clone()]).unwrap();
    assert_failed(&out, &second, "the bucket holds");
    let mut torrent = charter_caveats(vendor);
    torrent.burst = units(150);
    torrent.rate = Amount(u128::MAX);
    let create = chain
        .tx(
            "principal",
            TxKind::CreateCharter {
                agent,
                caveats: torrent,
                funding: units(1_000),
                agent_bond: Amount::ZERO,
            },
        )
        .unwrap();
    let out = chain.pulse(vec![create.clone()]).unwrap();
    assert_success(&out, &create);
    let (fast, fast_allotment) = all_events(&out)
        .into_iter()
        .find_map(|e| match e {
            Event::CharterCreated { charter, allotment } => Some((charter, allotment)),
            _ => None,
        })
        .unwrap();
    for _ in 0..2 {
        let tx = pay(&mut chain, fast, fast_allotment, 100);
        let out = chain.pulse(vec![tx.clone()]).unwrap();
        assert_success(&out, &tx);
    }

    // Lineages: the agent delegates to itself until the chain is
    // MAX_LINEAGE_DEPTH generations deep; payments still work at that depth,
    // and the next delegation fails.
    caveats.burst = units(1_000);
    caveats.rate = units(1);
    let create = chain
        .tx(
            "principal",
            TxKind::CreateCharter {
                agent,
                caveats: caveats.clone(),
                funding: units(1_000),
                agent_bond: Amount::ZERO,
            },
        )
        .unwrap();
    let out = chain.pulse(vec![create.clone()]).unwrap();
    assert_success(&out, &create);
    let (root, root_allotment) = all_events(&out)
        .into_iter()
        .find_map(|e| match e {
            Event::CharterCreated { charter, allotment } => Some((charter, allotment)),
            _ => None,
        })
        .unwrap();
    let (mut charter, mut allotment) = (root, root_allotment);
    let mut levels = vec![(charter, allotment)];
    for level in 1..=MAX_LINEAGE_DEPTH as u64 {
        // Each level keeps 5 USD, 5 USD of burst and 0.01 USD of rate (which
        // are carved like the budget), and passes the rest down.
        let passed = 5 * (MAX_LINEAGE_DEPTH as u64 + 1 - level) + 10;
        let mut narrower = caveats.clone();
        narrower.burst = minus(caveats.burst, units(5 * level));
        narrower.rate = minus(caveats.rate, Amount(10_000 * u128::from(level)));
        let input = reference(&chain, &allotment);
        let delegate = chain
            .tx_with(
                "agent",
                TxKind::Delegate {
                    charter,
                    sub_agent: agent,
                    caveats: narrower,
                    amount: units(passed),
                },
                vec![input],
            )
            .unwrap();
        let out = chain.pulse(vec![delegate.clone()]).unwrap();
        assert_success(&out, &delegate);
        (charter, allotment) = all_events(&out)
            .into_iter()
            .find_map(|e| match e {
                Event::CharterCreated { charter, allotment } => Some((charter, allotment)),
                _ => None,
            })
            .unwrap();
        levels.push((charter, allotment));
    }
    let (deepest, deepest_allotment) = levels[MAX_LINEAGE_DEPTH];
    match &chain.state.objects[&deepest].data {
        ObjectData::Charter(c) => assert_eq!(c.lineage.depth(), MAX_LINEAGE_DEPTH),
        other => panic!("{other:?}"),
    }
    let ok = pay(&mut chain, deepest, deepest_allotment, 5);
    let out = chain.pulse(vec![ok.clone()]).unwrap();
    assert_success(&out, &ok);
    let mut deeper = caveats.clone();
    deeper.burst = units(1);
    deeper.rate = Amount::ZERO;
    let input = reference(&chain, &deepest_allotment);
    let too_deep = chain
        .tx_with(
            "agent",
            TxKind::Delegate {
                charter: deepest,
                sub_agent: agent,
                caveats: deeper,
                amount: units(1),
            },
            vec![input],
        )
        .unwrap();
    let out = chain.pulse(vec![too_deep.clone()]).unwrap();
    assert_failed(&out, &too_deep, "too deep");
}

fn stream_state(chain: &Chain, id: &ObjectId) -> sion_exec::stream::Stream {
    match &chain.state.objects[id].data {
        ObjectData::Stream(stream) => stream.clone(),
        other => panic!("not a stream: {other:?}"),
    }
}

/// An agent pays an API one tenth of a cent per request from a stream: each
/// payment is one chain element, checked by the API with one hash, and the
/// API settles thousands of them in a transaction. Anyone holding a request
/// receipt can check it against the settled head later.
#[test]
fn a_stream_pays_an_api_per_request_with_one_hash_each() {
    use sion_exec::stream::{extends, step, walk, PayerChain};

    let mut chain = Chain::new(&["agent", "api", "auditor"], 100, 2);
    let api = chain.id("api");
    let mut payer = PayerChain::new(hash_domain("test/stream/seed", b"agent"), 100_000);
    let unit = Amount(1_000); // 0.001 USD
    let open = chain
        .tx(
            "agent",
            TxKind::OpenStream {
                payee: api,
                asset: usd(),
                unit,
                length: payer.length(),
                anchor: payer.anchor(),
                deposit: units(10),
                until: Pulse(chain.state.pulse.0 + 50),
                checkpoints: None,
            },
        )
        .unwrap();
    let agent_before = chain.balance("agent", &usd());
    let out = chain.pulse(vec![open.clone()]).unwrap();
    assert_success(&out, &open);
    let stream = all_events(&out)
        .iter()
        .find_map(|e| match e {
            Event::StreamOpened { stream, .. } => Some(*stream),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        chain.balance("agent", &usd()),
        minus(minus(agent_before, units(10)), open.tx.fee)
    );

    // Off the ledger: 2,500 requests, each paid by the next element and
    // checked by the API with one hash against the last one it accepted.
    let (mut last, mut hashes) = (payer.anchor(), 0u64);
    let mut receipts = Vec::new();
    for position in 1..=2_500u64 {
        let value = payer.element(position).unwrap();
        hashes += 1;
        assert_eq!(step(&value), last, "payment {position}");
        last = value;
        if position % 500 == 234 {
            receipts.push((position, value));
        }
    }
    assert_eq!(hashes, 2_500);
    // A payment that skips ahead (three requests at once) is checked with
    // three hashes.
    assert!(extends(&last, &payer.element(2_503).unwrap(), 3));
    // A replayed or forged payment does not hash to the last one accepted.
    assert_ne!(step(&payer.element(2_500).unwrap()), last);
    assert_ne!(step(&hash_domain("test/forged", b"x")), last);

    // The API settles twice: 1,000 steps, then 1,500.
    let api_before = chain.balance("api", &usd());
    let first = chain
        .tx(
            "api",
            TxKind::SettleStream {
                stream,
                position: 1_000,
                value: payer.element(1_000).unwrap(),
                jump: None,
            },
        )
        .unwrap();
    let out = chain.pulse(vec![first.clone()]).unwrap();
    assert_success(&out, &first);
    let second = chain
        .tx(
            "api",
            TxKind::SettleStream {
                stream,
                position: 2_500,
                value: last,
                jump: None,
            },
        )
        .unwrap();
    let out = chain.pulse(vec![second.clone()]).unwrap();
    assert_success(&out, &second);
    // The fee pays for each step checked: 2 micro-USD on top of the minimum.
    let config = chain.state.config.clone();
    assert_eq!(
        second.tx.fee,
        plus(config.min_fee(second.tx.size()), Amount(2 * 1_500))
    );
    let fees = plus(first.tx.fee, second.tx.fee);
    assert_eq!(
        chain.balance("api", &usd()),
        minus(plus(api_before, Amount(2_500 * 1_000)), fees)
    );
    let settled = stream_state(&chain, &stream);
    assert_eq!((settled.position, settled.head), (2_500, last));
    assert_eq!(settled.escrow, minus(units(10), Amount(2_500 * 1_000)));
    // 2,500 payments cost the API two transactions: about 2.4 micro-USD
    // each, against 1,000+ for a transfer per request.
    assert!(fees.0 / 2_500 < 5);

    // An auditor checks request receipts against the settled head alone.
    for (position, value) in &receipts {
        assert_eq!(walk(&settled.head, settled.position - position), *value);
    }
    let (position, _) = receipts[0];
    assert_ne!(
        walk(&settled.head, settled.position - position),
        hash_domain("test/forged", b"receipt")
    );

    // The API closes the stream and the agent gets the rest back.
    let agent_before = chain.balance("agent", &usd());
    let close = chain.tx("api", TxKind::CloseStream { stream }).unwrap();
    let out = chain.pulse(vec![close.clone()]).unwrap();
    assert_success(&out, &close);
    assert_eq!(
        chain.balance("agent", &usd()),
        plus(agent_before, Amount(10_000_000 - 2_500_000))
    );
    assert!(!stream_state(&chain, &stream).open);
}

/// With checkpoints, what a settlement costs does not grow with the number
/// of payments it covers: settling 1,000 payments and settling 200,000 cost
/// the same hashes and the same fee.
#[test]
fn a_settlement_by_checkpoint_costs_the_same_however_many_payments_it_covers() {
    use sion_exec::stream::PayerChain;

    let mut chain = Chain::new(&["agent", "api"], 1_000, 4);
    let api = chain.id("api");
    let chains = [
        PayerChain::with_checkpoints(hash_domain("test/stream/seed", b"small"), 1_000, 100),
        PayerChain::with_checkpoints(hash_domain("test/stream/seed", b"large"), 200_000, 100),
    ];
    let opens: Vec<_> = chains
        .iter()
        .map(|payer| {
            chain
                .tx(
                    "agent",
                    TxKind::OpenStream {
                        payee: api,
                        asset: usd(),
                        unit: Amount(1),
                        length: payer.length(),
                        anchor: payer.anchor(),
                        deposit: units(1),
                        until: Pulse(chain.state.pulse.0 + 50),
                        checkpoints: payer.checkpoints(),
                    },
                )
                .unwrap()
        })
        .collect();
    let out = chain.pulse(opens).unwrap();
    let streams: Vec<ObjectId> = all_events(&out)
        .iter()
        .filter_map(|e| match e {
            Event::StreamOpened { stream, .. } => Some(*stream),
            _ => None,
        })
        .collect();
    assert_eq!(streams.len(), 2);
    let mut chains = chains;
    // Settle each stream to its top, by the jump to its last checkpoint.
    let settles: Vec<_> = (0..2)
        .map(|i| {
            let stream = streams
                .iter()
                .copied()
                .find(|id| stream_state(&chain, id).length == chains[i].length())
                .unwrap();
            let top = chains[i].length();
            chain
                .tx(
                    "api",
                    TxKind::SettleStream {
                        stream,
                        position: top,
                        value: chains[i].element(top).unwrap(),
                        jump: chains[i].jump(top / 100),
                    },
                )
                .unwrap()
        })
        .collect();
    let out = chain.pulse(settles.clone()).unwrap();
    for tx in &settles {
        assert_success(&out, tx);
    }
    let paths: Vec<usize> = settles
        .iter()
        .map(|tx| match &tx.tx.kind {
            TxKind::SettleStream {
                jump: Some(jump), ..
            } => jump.path.len(),
            _ => unreachable!(),
        })
        .collect();
    // 11 leaves and 2,001 leaves: paths of at most 4 and 11 hashes (an
    // RFC 9162 tree gives its last leaf a shorter one). The settlement that
    // covers 200 times as many payments costs a few more hashes: 2
    // micro-USD each.
    assert!(paths[0] <= 4 && paths[1] <= 11, "{paths:?}");
    let fees: Vec<u128> = settles.iter().map(|tx| tx.tx.fee.0).collect();
    assert_eq!(fees[1] - fees[0], 2 * (paths[1] - paths[0]) as u128);
    assert_eq!(
        chain.balance("api", &usd()),
        minus(
            plus(units(1_000), Amount(1_000 + 200_000)),
            Amount(fees[0] + fees[1])
        )
    );
}
