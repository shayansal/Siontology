//! Storage deals in the reference executor (specification §5.50 to §5.54,
//! the beacon records of §6.3): the rules end to end on a simulated chain,
//! whose chunks are re-executed from their witnesses after every Pulse.

use sion_core::ids::{ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::beacons::{slot_of, BEACON_INTERVAL};
use sion_exec::execute::{Event, ExecStatus};
use sion_exec::object::{well_known, ObjectData};
use sion_exec::pulse::PulseOutput;
use sion_exec::sim::{sim_beacon, Chain};
use sion_exec::storage::{
    challenge_index, round_seed, Custody, DataTree, Deal, DealState, MAX_LEAF_BYTES,
};
use sion_exec::TxKind;

fn data(leaves: usize, tag: u8) -> Vec<u8> {
    (0..leaves * MAX_LEAF_BYTES)
        .map(|i| (i as u8).wrapping_mul(29).wrapping_add(tag))
        .collect()
}

fn events(out: &PulseOutput) -> Vec<Event> {
    out.effects.iter().flat_map(|e| e.events.clone()).collect()
}

fn statuses(out: &PulseOutput) -> Vec<ExecStatus> {
    out.effects.iter().map(|e| e.status.clone()).collect()
}

fn succeeded(out: &PulseOutput) -> bool {
    out.effects.iter().all(|e| e.succeeded())
}

fn deal_of(chain: &Chain, id: &ObjectId) -> Deal {
    match &chain.state.objects[id].data {
        ObjectData::Deal(d) => (**d).clone(),
        other => panic!("{other:?}"),
    }
}

fn custody_of(chain: &Chain, id: &ObjectId) -> Custody {
    match &chain.state.objects[id].data {
        ObjectData::Custody(c) => c.clone(),
        other => panic!("{other:?}"),
    }
}

struct Setup {
    chain: Chain,
    deal: ObjectId,
    data: Vec<u8>,
    tree: DataTree,
}

fn post_kind(
    tree: &DataTree,
    challenges: u8,
    rounds: u32,
    round_length: u64,
    collateral: Amount,
    provider: Option<sion_core::ids::AccountId>,
    accept_by: Pulse,
) -> TxKind {
    TxKind::PostDeal {
        root: tree.root(),
        size: tree.size(),
        challenges,
        price: Amount::units(1),
        rounds,
        round_length,
        collateral,
        provider,
        accept_by,
        locator: "https://example.org/data".into(),
    }
}

fn post(
    leaves: usize,
    challenges: u8,
    rounds: u32,
    round_length: u64,
    collateral: Amount,
) -> Setup {
    let mut chain = Chain::new(&["client", "provider", "stranger"], 100_000, 8);
    let data = data(leaves, 7);
    let tree = DataTree::of_data(&data);
    let provider = chain.id("provider");
    let accept_by = Pulse(chain.state.pulse.0 + 10_000);
    let kind = post_kind(
        &tree,
        challenges,
        rounds,
        round_length,
        collateral,
        Some(provider),
        accept_by,
    );
    let tx = chain.tx("client", kind).unwrap();
    let out = chain.pulse(vec![tx]).unwrap();
    assert!(succeeded(&out), "{:?}", statuses(&out));
    let deal = events(&out)
        .into_iter()
        .find_map(|e| match e {
            Event::DealPosted { deal, .. } => Some(deal),
            _ => None,
        })
        .unwrap();
    Setup {
        chain,
        deal,
        data,
        tree,
    }
}

/// Accepts in Pulse `pulse` (advancing to just before it); returns the
/// custody record.
fn accept_at(s: &mut Setup, pulse: u64) -> ObjectId {
    if s.chain.state.pulse.0 + 1 < pulse {
        s.chain.advance_to(Pulse(pulse - 1)).unwrap();
    }
    let tx = s
        .chain
        .tx("provider", TxKind::AcceptDeal { deal: s.deal })
        .unwrap();
    let out = s.chain.pulse(vec![tx]).unwrap();
    assert!(succeeded(&out), "{:?}", statuses(&out));
    match deal_of(&s.chain, &s.deal).state {
        DealState::Active { custody, .. } => custody,
        other => panic!("{other:?}"),
    }
}

/// The leaves round `round` of the deal in `record` challenges.
fn challenged(record: &Custody, round: u32) -> Vec<u64> {
    let beacon = sim_beacon(record.round_start(round));
    let seed = round_seed(&beacon, &record.deal);
    (0..record.challenges)
        .map(|k| challenge_index(&seed, round, k, record.size))
        .collect()
}

/// Proves `round` in Pulse `pulse` (advancing to just before it).
fn prove_in(s: &mut Setup, custody: ObjectId, round: u32, pulse: u64) -> PulseOutput {
    if s.chain.state.pulse.0 + 1 < pulse {
        s.chain.advance_to(Pulse(pulse - 1)).unwrap();
    }
    assert_eq!(s.chain.state.pulse.0 + 1, pulse);
    let record = custody_of(&s.chain, &custody);
    let beacon = sim_beacon(record.round_start(round));
    let proof = s.tree.answer(&s.data, &record, round, &beacon).unwrap();
    let reference = s.chain.state.objects[&custody].reference();
    let tx = s
        .chain
        .tx_with(
            "provider",
            TxKind::ProveStorage {
                proofs: vec![proof],
            },
            vec![reference],
        )
        .unwrap();
    s.chain.pulse(vec![tx]).unwrap()
}

fn close_at_end(s: &mut Setup) -> (Amount, Amount, Amount, u32) {
    let end = deal_of(&s.chain, &s.deal).end().unwrap();
    if s.chain.state.pulse < end {
        s.chain.advance_to(end).unwrap();
    }
    let tx = s
        .chain
        .tx("provider", TxKind::CloseDeal { deal: s.deal })
        .unwrap();
    let out = s.chain.pulse(vec![tx]).unwrap();
    assert!(succeeded(&out), "{:?}", statuses(&out));
    events(&out)
        .into_iter()
        .find_map(|e| match e {
            Event::DealClosed {
                paid,
                returned,
                forfeited,
                missed,
                ..
            } => Some((paid, returned, forfeited, missed)),
            _ => None,
        })
        .unwrap()
}

#[test]
fn every_eighth_pulse_records_its_beacon() {
    let mut chain = Chain::new(&["client"], 10, 1);
    chain.advance_to(Pulse(40)).unwrap();
    for pulse in (8..=40).step_by(BEACON_INTERVAL as usize) {
        let slot = slot_of(Pulse(pulse)).unwrap();
        match &chain.state.objects[&well_known::beacon(slot)].data {
            ObjectData::Beacon(record) => {
                assert_eq!(record.pulse, Pulse(pulse));
                assert_eq!(record.beacon, sim_beacon(Pulse(pulse)));
            }
            other => panic!("{other:?}"),
        }
    }
    assert!(!chain
        .state
        .objects
        .contains_key(&well_known::beacon(slot_of(Pulse(48)).unwrap())));
}

#[test]
fn rounds_start_on_recorded_pulses() {
    let mut s = post(4, 2, 3, 16, Amount::units(2));
    let accepted = s.chain.state.pulse.0 + 1;
    let custody = accept_at(&mut s, accepted);
    let record = custody_of(&s.chain, &custody);
    assert!(record.start.0 > accepted && record.start.0 <= accepted + BEACON_INTERVAL);
    assert_eq!(record.start.0 % BEACON_INTERVAL, 0);
    // Accepting on a boundary starts at the next one.
    let mut s = post(4, 2, 3, 16, Amount::units(2));
    let custody = accept_at(&mut s, 16);
    assert_eq!(custody_of(&s.chain, &custody).start, Pulse(24));
}

#[test]
fn deal_terms_are_checked() {
    let mut chain = Chain::new(&["client", "provider"], 1_000, 4);
    let tree = DataTree::of_data(&data(4, 1));
    let provider = Some(chain.id("provider"));
    let by = Pulse(500);
    let cases = [
        // Not a multiple of the beacon interval.
        post_kind(&tree, 2, 3, 12, Amount::units(1), provider, by),
        // Too long: beacon records would not last.
        post_kind(&tree, 2, 3, 100_008, Amount::units(1), provider, by),
        // Open to anyone without collateral.
        post_kind(&tree, 2, 3, 16, Amount::ZERO, None, by),
    ];
    for kind in cases {
        let tx = chain.tx("client", kind).unwrap();
        let out = chain.pulse(vec![tx]).unwrap();
        assert!(
            matches!(statuses(&out)[0], ExecStatus::Failed(_)),
            "{:?}",
            statuses(&out)
        );
    }
    // A named provider may be trusted without collateral; open deals with
    // collateral are fine.
    for kind in [
        post_kind(&tree, 2, 3, 16, Amount::ZERO, provider, by),
        post_kind(&tree, 2, 3, 100_000, Amount::units(1), None, by),
    ] {
        let tx = chain.tx("client", kind).unwrap();
        let out = chain.pulse(vec![tx]).unwrap();
        assert!(succeeded(&out), "{:?}", statuses(&out));
    }
}

#[test]
fn the_challenge_is_the_same_whenever_the_proof_executes() {
    // Prove round 0 at its first Pulse (the beacon is the Pulse's own) and
    // round 1 at its last (the beacon comes from its record): the same
    // answer, built from the round's first Pulse, is accepted both times.
    let mut s = post(16, 4, 3, 16, Amount::units(3));
    let accepted = s.chain.state.pulse.0 + 1;
    let custody = accept_at(&mut s, accepted);
    let start = custody_of(&s.chain, &custody).start.0;
    let out = prove_in(&mut s, custody, 0, start);
    assert!(succeeded(&out), "{:?}", statuses(&out));
    let out = prove_in(&mut s, custody, 1, start + 31);
    assert!(succeeded(&out), "{:?}", statuses(&out));
    assert!(events(&out)
        .iter()
        .any(|e| matches!(e, Event::StorageProven { round: 1, .. })));
    // An answer to another beacon's challenge is refused.
    let record = custody_of(&s.chain, &custody);
    let wrong = s
        .tree
        .answer(&s.data, &record, 2, &sim_beacon(Pulse(start + 40)))
        .unwrap();
    if wrong.leaves
        != s.tree
            .answer(&s.data, &record, 2, &sim_beacon(record.round_start(2)))
            .unwrap()
            .leaves
    {
        s.chain.advance_to(Pulse(start + 40)).unwrap();
        let reference = s.chain.state.objects[&custody].reference();
        let tx = s
            .chain
            .tx_with(
                "provider",
                TxKind::ProveStorage {
                    proofs: vec![wrong],
                },
                vec![reference],
            )
            .unwrap();
        let out = s.chain.pulse(vec![tx]).unwrap();
        assert!(matches!(statuses(&out)[0], ExecStatus::Failed(_)));
    }
}

/// A provider that kept half the leaves, and could try every Pulse of every
/// round, answers only the rounds whose challenge falls in its half: the
/// Pulse it proves in changes nothing. (Before beacon records, it could
/// choose the next round's challenge through its proof's Pulse, and passed
/// every round.)
#[test]
fn a_provider_with_half_the_data_cannot_choose_its_challenges() {
    let rounds = 20;
    let round_length = 64;
    let mut s = post(64, 4, rounds, round_length, Amount::units(20));
    let size = s.tree.size();
    let kept = size / 2;
    let accepted = s.chain.state.pulse.0 + 1;
    let custody = accept_at(&mut s, accepted);
    let mut proven = 0;
    for round in 0..rounds {
        let record = custody_of(&s.chain, &custody);
        if challenged(&record, round).iter().all(|&i| i < kept) {
            // It proves at the round's last Pulse: as good as any.
            let last = record.round_start(round).0 + round_length - 1;
            let out = prove_in(&mut s, custody, round, last);
            assert!(succeeded(&out), "{:?}", statuses(&out));
            proven += 1;
        }
    }
    let expected: u32 = (0..rounds)
        .filter(|&r| {
            challenged(&custody_of(&s.chain, &custody), r)
                .iter()
                .all(|&i| i < kept)
        })
        .count() as u32;
    assert_eq!(proven, expected);
    assert!(proven <= rounds / 2, "proved {proven} of {rounds}");
    let (_, returned, forfeited, missed) = close_at_end(&mut s);
    assert_eq!(missed, rounds - proven);
    assert_eq!(
        forfeited,
        Amount(Amount::units(20).0 * u128::from(missed) / u128::from(rounds))
    );
    assert_eq!(returned.checked_add(forfeited).unwrap(), Amount::units(20));
}

#[test]
fn forfeits_are_the_missed_share_of_the_collateral_rounded_down_once() {
    // 10 micro-USD over 4 rounds, 2 missed: 5 forfeited, 5 returned.
    let mut s = post(3, 1, 4, 8, Amount(10));
    let accepted = s.chain.state.pulse.0 + 1;
    let custody = accept_at(&mut s, accepted);
    let start = custody_of(&s.chain, &custody).start.0;
    assert!(succeeded(&prove_in(&mut s, custody, 0, start)));
    let tx = s
        .chain
        .tx("provider", TxKind::SettleDeal { deal: s.deal })
        .unwrap();
    assert!(succeeded(&s.chain.pulse(vec![tx]).unwrap()));
    assert!(succeeded(&prove_in(&mut s, custody, 2, start + 16)));
    let (paid, returned, forfeited, missed) = close_at_end(&mut s);
    assert_eq!(
        (paid, returned, forfeited, missed),
        (Amount::units(1), Amount(5), Amount(5), 2)
    );
    // A collateral smaller than the number of rounds no longer escapes:
    // 3 micro-USD over 4 rounds, nothing proven, all of it forfeited.
    let mut s = post(3, 1, 4, 8, Amount(3));
    let accepted = s.chain.state.pulse.0 + 1;
    accept_at(&mut s, accepted);
    let (paid, returned, forfeited, missed) = close_at_end(&mut s);
    assert_eq!(
        (paid, returned, forfeited, missed),
        (Amount::ZERO, Amount::ZERO, Amount(3), 4)
    );
}
