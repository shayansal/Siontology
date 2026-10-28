//! Randomized workloads: whatever mix of operations runs, supply is
//! conserved, the vault balances, every chunk replays from its witness and
//! two nodes executing the same Pulses agree. The mix includes payment
//! streams settled, topped up and closed in any order.

use proptest::prelude::*;
use sion_assurance::AtomClass;
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, AssetId, ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::claim::{formulas, ClaimBody, Payload};
use sion_exec::execute::{bond_atom, Event};
use sion_exec::sim::Chain;
use sion_exec::stream::PayerChain;
use sion_exec::{ObjectData, SignedTransaction, TxKind};

const NAMES: [&str; 4] = ["a", "b", "c", "d"];

#[derive(Clone, Debug)]
enum Op {
    Transfer {
        from: usize,
        to: usize,
        amount: u64,
    },
    Claim {
        from: usize,
        tag: u8,
        bonded: bool,
    },
    Bond {
        from: usize,
        amount: u64,
    },
    Withdraw {
        from: usize,
    },
    Mint {
        to: usize,
        amount: u64,
    },
    /// The payee of stream `stream` settles `steps` more steps, by a jump
    /// to a checkpoint if `jump` (only stream 1 has checkpoints; a jump on
    /// stream 0 fails).
    Settle {
        stream: usize,
        steps: u64,
        jump: bool,
    },
    /// The payer of stream `stream` adds funds and time.
    TopUp {
        stream: usize,
        amount: u64,
        extend: u64,
    },
    /// The payee, or the payer, closes stream `stream`.
    Close {
        stream: usize,
        by_payee: bool,
    },
}

/// Two streams, a → b and c → d, opened before the random Pulses.
const STREAMS: [(usize, usize); 2] = [(0, 1), (2, 3)];
const STREAM_LENGTH: u64 = 3_000;

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (0..4usize, 0..4usize, 1..400u64).prop_map(|(from, to, amount)| Op::Transfer { from, to, amount }),
        2 => (0..4usize, any::<u8>(), any::<bool>()).prop_map(|(from, tag, bonded)| Op::Claim { from, tag, bonded }),
        1 => (0..4usize, 1..300u64).prop_map(|(from, amount)| Op::Bond { from, amount }),
        1 => (0..4usize).prop_map(|from| Op::Withdraw { from }),
        1 => (0..4usize, 1..1_000u64).prop_map(|(to, amount)| Op::Mint { to, amount }),
        3 => (0..2usize, 1..2_000u64, any::<bool>()).prop_map(|(stream, steps, jump)| Op::Settle { stream, steps, jump }),
        1 => (0..2usize, 0..30u64, 0..20u64).prop_map(|(stream, amount, extend)| Op::TopUp { stream, amount, extend }),
        1 => (0..2usize, any::<bool>()).prop_map(|(stream, by_payee)| Op::Close { stream, by_payee }),
    ]
}

/// The streams' IDs and the payers' chains.
struct Streams {
    ids: Vec<ObjectId>,
    chains: Vec<PayerChain>,
}

fn stream_of(chain: &Chain, id: &ObjectId) -> sion_exec::stream::Stream {
    match &chain.state.objects[id].data {
        ObjectData::Stream(stream) => stream.clone(),
        other => panic!("not a stream: {other:?}"),
    }
}

/// Opens the two streams on both chains: 40 USD each, one cent a step,
/// lasting 12 Pulses (so some random workloads outlive them). The second
/// commits to a checkpoint every 50 steps.
fn open_streams(one: &mut Chain, two: &mut Chain) -> Streams {
    let mut chains = Vec::new();
    let mut txs = Vec::new();
    let until = Pulse(one.state.pulse.0 + 12);
    for (index, (payer, payee)) in STREAMS.iter().enumerate() {
        let seed = Hash32([index as u8 + 1; 32]);
        let chain = match index {
            0 => PayerChain::new(seed, STREAM_LENGTH),
            _ => PayerChain::with_checkpoints(seed, STREAM_LENGTH, 50),
        };
        let payee = one.id(NAMES[*payee]);
        txs.push(
            one.tx(
                NAMES[*payer],
                TxKind::OpenStream {
                    payee,
                    asset: AssetId::usd(),
                    unit: Amount(10_000),
                    length: STREAM_LENGTH,
                    anchor: chain.anchor(),
                    deposit: Amount::units(40),
                    until,
                    checkpoints: chain.checkpoints(),
                },
            )
            .expect("open"),
        );
        chains.push(chain);
    }
    let out = one.pulse(txs.clone()).expect("pulse");
    two.pulse(txs.clone()).expect("pulse");
    let ids = txs
        .iter()
        .map(|tx| {
            out.effects
                .iter()
                .find(|e| e.digest == tx.digest())
                .and_then(|e| {
                    e.events.iter().find_map(|event| match event {
                        Event::StreamOpened { stream, .. } => Some(*stream),
                        _ => None,
                    })
                })
                .expect("stream opened")
        })
        .collect();
    Streams { ids, chains }
}

fn build(
    chain: &mut Chain,
    op: &Op,
    ids: &[AccountId],
    streams: &mut Streams,
) -> Option<SignedTransaction> {
    match op {
        Op::Transfer { from, to, amount } => chain
            .tx(
                NAMES[*from],
                TxKind::Transfer {
                    asset: AssetId::usd(),
                    outputs: vec![(ids[*to], Amount::units(*amount))],
                },
            )
            .ok(),
        Op::Claim { from, tag, bonded } => {
            let formula = if *bonded {
                formulas::bonded_statement(bond_atom(AtomClass::Economic, &ids[*from], "stake").id)
            } else {
                formulas::dual_signature()
            };
            chain
                .tx(
                    NAMES[*from],
                    TxKind::IssueClaim {
                        claim: ClaimBody {
                            schema: "prop.note/v1".into(),
                            subject: format!("note {tag}"),
                            payload: Payload::Public(vec![*tag]),
                            formula,
                            adjudicator: None,
                            detection_permille: 1000,
                            valid_for: u64::from(*tag % 5) + 1,
                            challenge_period: 2,
                        },
                    },
                )
                .ok()
        }
        Op::Bond { from, amount } => chain
            .tx(
                NAMES[*from],
                TxKind::PostBond {
                    class: AtomClass::Economic,
                    label: "stake".into(),
                    amount: Amount::units(*amount),
                },
            )
            .ok(),
        Op::Withdraw { from } => chain
            .tx(
                NAMES[*from],
                TxKind::WithdrawBond {
                    atom: bond_atom(AtomClass::Economic, &ids[*from], "stake").id,
                },
            )
            .ok(),
        Op::Mint { to, amount } => {
            let grants = vec![(ids[*to], AssetId::usd(), Amount::units(*amount))];
            let faucet = chain.wallets.get_mut(sion_exec::sim::FAUCET).unwrap();
            faucet.faucet(&chain.state, grants).ok()
        }
        Op::Settle {
            stream,
            steps,
            jump,
        } => {
            let id = streams.ids[*stream];
            let at = stream_of(chain, &id).position;
            // Positions past the chain's length are refused by the ledger;
            // the payer cannot reveal them, so the payee asks for the top.
            let position = (at + steps).min(STREAM_LENGTH);
            let value = streams.chains[*stream].element(position)?;
            // Stream 0 has no checkpoints: its "jump" is a made-up one.
            let jump = jump.then(|| {
                streams.chains[*stream]
                    .jump(position / 50)
                    .unwrap_or(sion_exec::stream::Jump {
                        index: position / 50,
                        path: Vec::new(),
                    })
            });
            chain
                .tx(
                    NAMES[STREAMS[*stream].1],
                    TxKind::SettleStream {
                        stream: id,
                        position,
                        value,
                        jump,
                    },
                )
                .ok()
        }
        Op::TopUp {
            stream,
            amount,
            extend,
        } => {
            let id = streams.ids[*stream];
            let until = Pulse(stream_of(chain, &id).until.0 + extend);
            chain
                .tx(
                    NAMES[STREAMS[*stream].0],
                    TxKind::TopUpStream {
                        stream: id,
                        deposit: Amount::units(*amount),
                        until,
                    },
                )
                .ok()
        }
        Op::Close { stream, by_payee } => {
            let (payer, payee) = STREAMS[*stream];
            let who = if *by_payee { payee } else { payer };
            chain
                .tx(
                    NAMES[who],
                    TxKind::CloseStream {
                        stream: streams.ids[*stream],
                    },
                )
                .ok()
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]
    #[test]
    fn random_workloads_conserve_and_replay(
        pulses in proptest::collection::vec(proptest::collection::vec(op(), 0..7), 1..6)
    ) {
        let mut one = Chain::new(&NAMES, 2_000, 3);
        let mut two = Chain::new(&NAMES, 2_000, 3);
        two.replay = false;
        let ids: Vec<AccountId> = NAMES.iter().map(|n| one.id(n)).collect();
        let mut streams = open_streams(&mut one, &mut two);
        for ops in &pulses {
            let mut txs = Vec::new();
            let mut minted = false;
            for op in ops {
                if matches!(op, Op::Mint { .. }) {
                    // The faucet's account object admits one transaction per Pulse.
                    if minted { continue; }
                    minted = true;
                }
                if let Some(tx) = build(&mut one, op, &ids, &mut streams) {
                    txs.push(tx);
                }
            }
            // Chain::pulse replays every chunk and checks the invariants.
            let a = one.pulse(txs.clone()).map_err(|e| TestCaseError::fail(e.to_string()))?;
            // The second node receives the owned set in another order (it is
            // a set); consensus fixes the contended order for both.
            let (contended, mut owned): (Vec<_>, Vec<_>) = txs.into_iter().partition(|t| t.tx.domain().is_some());
            owned.reverse();
            let b = two.pulse_with(owned, contended).map_err(|e| TestCaseError::fail(e.to_string()))?;
            prop_assert_eq!(a.after, b.after);
            prop_assert_eq!(a.receipts_root, b.receipts_root);
            // Whatever ran, a stream never pays out more than was put in,
            // and what it paid matches the steps it settled.
            for (index, id) in streams.ids.iter().enumerate() {
                let stream = stream_of(&one, id);
                prop_assert!(stream.position <= STREAM_LENGTH);
                prop_assert_eq!(
                    sion_exec::stream::walk(&stream.head, stream.position),
                    streams.chains[index].anchor()
                );
                if !stream.open {
                    prop_assert_eq!(stream.escrow, Amount::ZERO);
                }
            }
        }
    }
}

/// The stream operations in the mix do run: a fixed workload settles both
/// streams twice in one Pulse, tops one up and closes the other.
#[test]
fn the_stream_operations_in_the_mix_take_effect() {
    let mut one = Chain::new(&NAMES, 2_000, 3);
    let mut two = Chain::new(&NAMES, 2_000, 3);
    let ids: Vec<AccountId> = NAMES.iter().map(|n| one.id(n)).collect();
    let mut streams = open_streams(&mut one, &mut two);
    let ops = [
        Op::Settle {
            stream: 0,
            steps: 10,
            jump: false,
        },
        Op::Settle {
            stream: 0,
            steps: 25,
            jump: false,
        },
        Op::Settle {
            stream: 1,
            steps: 400,
            jump: true,
        },
        Op::TopUp {
            stream: 0,
            amount: 5,
            extend: 3,
        },
    ];
    let txs: Vec<_> = ops
        .iter()
        .filter_map(|op| build(&mut one, op, &ids, &mut streams))
        .collect();
    assert_eq!(txs.len(), 4);
    let out = one.pulse(txs).unwrap();
    let settled = out
        .effects
        .iter()
        .flat_map(|e| &e.events)
        .filter(|e| matches!(e, Event::StreamSettled { .. }))
        .count();
    // Both settlements of stream 0 succeed in the order consensus gave them
    // (10 steps, then 15 more); stream 1 settles 400.
    assert_eq!(settled, 3);
    assert_eq!(stream_of(&one, &streams.ids[0]).position, 25);
    assert_eq!(stream_of(&one, &streams.ids[1]).position, 400);
    let payer_before = one.balance("c", &AssetId::usd());
    let close = build(
        &mut one,
        &Op::Close {
            stream: 1,
            by_payee: true,
        },
        &ids,
        &mut streams,
    )
    .unwrap();
    one.pulse(vec![close]).unwrap();
    let closed = stream_of(&one, &streams.ids[1]);
    assert!(!closed.open);
    assert_eq!(closed.escrow, Amount::ZERO);
    // The payer gets back what the 400 settled steps left: 40 − 4 USD.
    assert_eq!(
        one.balance("c", &AssetId::usd()).0,
        payer_before.0 + Amount::units(36).0
    );
}
