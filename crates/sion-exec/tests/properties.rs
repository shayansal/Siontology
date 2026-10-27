//! Randomized workloads: whatever mix of operations runs, supply is
//! conserved, the vault balances, every chunk replays from its witness and
//! two nodes executing the same Pulses agree.

use proptest::prelude::*;
use sion_assurance::AtomClass;
use sion_core::ids::{AccountId, AssetId};
use sion_core::Amount;
use sion_exec::claim::{formulas, ClaimBody, Payload};
use sion_exec::execute::bond_atom;
use sion_exec::sim::Chain;
use sion_exec::{SignedTransaction, TxKind};

const NAMES: [&str; 4] = ["a", "b", "c", "d"];

#[derive(Clone, Debug)]
enum Op {
    Transfer { from: usize, to: usize, amount: u64 },
    Claim { from: usize, tag: u8, bonded: bool },
    Bond { from: usize, amount: u64 },
    Withdraw { from: usize },
    Mint { to: usize, amount: u64 },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (0..4usize, 0..4usize, 1..400u64).prop_map(|(from, to, amount)| Op::Transfer { from, to, amount }),
        2 => (0..4usize, any::<u8>(), any::<bool>()).prop_map(|(from, tag, bonded)| Op::Claim { from, tag, bonded }),
        1 => (0..4usize, 1..300u64).prop_map(|(from, amount)| Op::Bond { from, amount }),
        1 => (0..4usize).prop_map(|from| Op::Withdraw { from }),
        1 => (0..4usize, 1..1_000u64).prop_map(|(to, amount)| Op::Mint { to, amount }),
    ]
}

fn build(chain: &mut Chain, op: &Op, ids: &[AccountId]) -> Option<SignedTransaction> {
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
        for ops in &pulses {
            let mut txs = Vec::new();
            let mut minted = false;
            for op in ops {
                if matches!(op, Op::Mint { .. }) {
                    // The faucet's account object admits one transaction per Pulse.
                    if minted { continue; }
                    minted = true;
                }
                if let Some(tx) = build(&mut one, op, &ids) {
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
        }
    }
}
