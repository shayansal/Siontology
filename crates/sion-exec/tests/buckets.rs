//! Token buckets add up (spec v1.14, §5.13): however a tree of agents is
//! shaped and whatever it spawns, cohorts and delegations included, every
//! subtree whose root has caveats with burst `σ` and rate `ρ` pays at most
//! `σ + ρ·τ` in any `τ + 1` consecutive Pulses. Checked on the executor,
//! with random trees, payments and gaps between Pulses, for the whole tree
//! and for every subtree in it.

use std::collections::BTreeMap;

use proptest::prelude::*;
use sion_assurance::Policy;
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::charter::{Caveats, Spawn};
use sion_exec::cohort::{CohortMember, CohortTree, Materialize};
use sion_exec::sim::Chain;
use sion_exec::{Event, SignedTransaction, TxKind};

/// The wallets agents act through: an agent's account may act for several
/// charters.
const WALLETS: [&str; 6] = ["w0", "w1", "w2", "w3", "w4", "w5"];

#[derive(Clone, Debug)]
enum Op {
    /// Node `parent` spawns a child acting through wallet `wallet`, with a
    /// burst of `burst` tenths and a rate of `rate` hundredths a Pulse.
    Spawn {
        parent: usize,
        wallet: usize,
        burst: u64,
        rate: u64,
    },
    /// Node `parent` delegates to a sub-agent acting through wallet
    /// `wallet` (`Delegate`, §5.14), with a burst of `burst` tenths and a
    /// rate of `rate` hundredths a Pulse.
    Delegate {
        parent: usize,
        wallet: usize,
        burst: u64,
        rate: u64,
    },
    /// Node `node` pays `amount` tenths.
    Pay { node: usize, amount: u64 },
    /// Node `parent` commits to a cohort of `count` members, each with a
    /// burst of `burst` tenths and a rate of `rate` hundredths a Pulse.
    Cohort {
        parent: usize,
        count: u32,
        burst: u64,
        rate: u64,
        full: bool,
    },
    /// The parent of cohort `cohort` materializes member `member`.
    Materialize { cohort: usize, member: u32 },
    /// The Pulse ends, and `gap` empty Pulses follow.
    Next { gap: u64 },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (0usize..8, 0usize..6, 0u64..400, 0u64..60).prop_map(|(parent, wallet, burst, rate)| {
            Op::Spawn { parent, wallet, burst, rate }
        }),
        1 => (0usize..8, 0usize..6, 0u64..400, 0u64..60).prop_map(|(parent, wallet, burst, rate)| {
            Op::Delegate { parent, wallet, burst, rate }
        }),
        6 => (0usize..16, 1u64..300).prop_map(|(node, amount)| Op::Pay { node, amount }),
        1 => (0usize..8, 1u32..6, 0u64..100, 0u64..40, any::<bool>()).prop_map(
            |(parent, count, burst, rate, full)| Op::Cohort { parent, count, burst, rate, full }
        ),
        2 => (0usize..4, 0u32..6).prop_map(|(cohort, member)| Op::Materialize { cohort, member }),
        3 => (0u64..6).prop_map(|gap| Op::Next { gap }),
    ]
}

fn tenths(n: u64) -> Amount {
    Amount(u128::from(n) * 100_000)
}

fn hundredths(n: u64) -> Amount {
    Amount(u128::from(n) * 10_000)
}

fn caveats(burst: Amount, rate: Amount) -> Caveats {
    Caveats {
        payees: None,
        max_per_payment: Amount::units(1_000),
        burst,
        rate,
        expires_at: Pulse(1_000_000),
        evidence_above: None,
        evidence_policy: Policy::standard(),
        receipts: false,
        may_delegate: true,
    }
}

/// A charter of the tree: its id, its allotment, the wallet that acts for
/// it, the node above it and its caveats' burst and rate.
#[derive(Clone, Copy, Debug)]
struct Node {
    charter: ObjectId,
    allotment: ObjectId,
    wallet: &'static str,
    parent: Option<usize>,
    bound: (Amount, Amount),
}

/// What a run paid: by the charter that paid, at each Pulse.
type Paid = BTreeMap<ObjectId, BTreeMap<u64, u128>>;

/// Spawns, delegations and materializations sent in a Pulse, by digest:
/// the node that sent each, and the wallet and bound of each charter it
/// creates, in order.
type Launches = BTreeMap<sion_core::ids::TxDigest, (usize, Vec<(&'static str, (Amount, Amount))>)>;

/// A cohort: its id, its parent node (and its index), its tree and its
/// members.
struct Group {
    id: ObjectId,
    parent: Node,
    parent_index: usize,
    tree: CohortTree,
    members: Vec<CohortMember>,
}

/// Runs `ops` on a tree whose root has `(σ, ρ)`; returns the tree's nodes
/// and what each paid at each Pulse.
fn run_tree(sigma: Amount, rho: Amount, ops: &[Op]) -> (Vec<Node>, Paid) {
    let mut names = vec!["principal", "vendor"];
    names.extend(WALLETS);
    let mut chain = Chain::new(&names, 100_000, 32);
    let register = chain
        .tx(
            "principal",
            TxKind::RegisterAgent {
                agent: chain.id(WALLETS[0]),
                card: Hash32::ZERO,
                caveats: caveats(sigma, rho),
                funding: Amount::units(50_000),
                agent_bond: Amount::ZERO,
            },
        )
        .expect("register");
    let out = chain.pulse(vec![register]).expect("pulse");
    let (charter, allotment) = out
        .effects
        .iter()
        .flat_map(|e| e.events.iter())
        .find_map(|e| match e {
            Event::AgentRegistered {
                agent, allotment, ..
            } => Some((*agent, *allotment)),
            _ => None,
        })
        .expect("registered");
    let accounts: BTreeMap<AccountId, &'static str> =
        WALLETS.iter().map(|w| (chain.id(w), *w)).collect();
    let vendor = chain.id("vendor");
    let mut nodes = vec![Node {
        charter,
        allotment,
        wallet: WALLETS[0],
        parent: None,
        bound: (sigma, rho),
    }];
    let mut groups: Vec<Group> = Vec::new();
    // Cohorts proposed this Pulse: their trees, until the Pulse says their ids.
    let mut proposed: Vec<(SignedTransaction, usize, CohortTree, Vec<CohortMember>)> = Vec::new();
    // Spawns, delegations and materializations sent this Pulse: who sent
    // them, and the wallet and bound of each child they create, in order.
    let mut launches: Launches = BTreeMap::new();
    let mut paid: Paid = BTreeMap::new();
    let mut pending: Vec<SignedTransaction> = Vec::new();
    // One transaction per allotment per Pulse: two would conflict.
    let mut busy: Vec<ObjectId> = Vec::new();

    let mut ops = ops.to_vec();
    ops.push(Op::Next { gap: 0 });
    for op in &ops {
        match op {
            Op::Spawn {
                parent,
                wallet,
                burst,
                rate,
            } => {
                let parent = *parent % nodes.len();
                let node = nodes[parent];
                if busy.contains(&node.allotment) {
                    continue;
                }
                let child = Spawn {
                    agent: Some(chain.id(WALLETS[*wallet])),
                    card: Hash32::ZERO,
                    caveats: caveats(tenths(*burst), hundredths(*rate)),
                    budget: Amount::units(100),
                };
                let bound = (child.caveats.burst, child.caveats.rate);
                let input = chain.state.objects[&node.allotment].reference();
                let tx = chain
                    .tx_with(
                        node.wallet,
                        TxKind::SpawnAgents {
                            charter: node.charter,
                            children: vec![child],
                        },
                        vec![input],
                    )
                    .expect("sign");
                busy.push(node.allotment);
                launches.insert(tx.digest(), (parent, vec![(WALLETS[*wallet], bound)]));
                pending.push(tx);
            }
            Op::Delegate {
                parent,
                wallet,
                burst,
                rate,
            } => {
                let parent = *parent % nodes.len();
                let node = nodes[parent];
                if busy.contains(&node.allotment) {
                    continue;
                }
                let child = caveats(tenths(*burst), hundredths(*rate));
                let bound = (child.burst, child.rate);
                let input = chain.state.objects[&node.allotment].reference();
                let tx = chain
                    .tx_with(
                        node.wallet,
                        TxKind::Delegate {
                            charter: node.charter,
                            sub_agent: chain.id(WALLETS[*wallet]),
                            caveats: child,
                            amount: Amount::units(100),
                        },
                        vec![input],
                    )
                    .expect("sign");
                busy.push(node.allotment);
                launches.insert(tx.digest(), (parent, vec![(WALLETS[*wallet], bound)]));
                pending.push(tx);
            }
            Op::Pay { node, amount } => {
                let node = nodes[*node % nodes.len()];
                if busy.contains(&node.allotment) {
                    continue;
                }
                let input = chain.state.objects[&node.allotment].reference();
                let tx = chain
                    .tx_with(
                        node.wallet,
                        TxKind::AgentPay {
                            charter: node.charter,
                            payee: vendor,
                            amount: tenths(*amount),
                            purpose: String::new(),
                            evidence: None,
                        },
                        vec![input],
                    )
                    .expect("sign");
                busy.push(node.allotment);
                pending.push(tx);
            }
            Op::Cohort {
                parent,
                count,
                burst,
                rate,
                full,
            } => {
                let parent = *parent % nodes.len();
                let node = nodes[parent];
                if busy.contains(&node.allotment) {
                    continue;
                }
                let members: Vec<CohortMember> = (0..*count as usize)
                    .map(|i| WALLETS[(i + 1) % WALLETS.len()])
                    .map(|w| CohortMember {
                        agent: Some(chain.id(w)),
                        card: Hash32::ZERO,
                        budget: Amount::units(10),
                        burst: tenths(*burst),
                        rate: hundredths(*rate),
                    })
                    .collect();
                let tree = CohortTree::build(&members).expect("a tree");
                let root = tree.root();
                let input = chain.state.objects[&node.allotment].reference();
                let tx = chain
                    .tx_with(
                        node.wallet,
                        TxKind::SpawnCohort {
                            charter: node.charter,
                            root: root.hash,
                            count: *count,
                            burst: root.burst,
                            rate: root.rate,
                            full: *full,
                            caveats: caveats(tenths(*burst), hundredths(*rate)),
                        },
                        vec![input],
                    )
                    .expect("sign");
                busy.push(node.allotment);
                proposed.push((tx.clone(), parent, tree, members));
                pending.push(tx);
            }
            Op::Materialize { cohort, member } => {
                if groups.is_empty() {
                    continue;
                }
                let group = &groups[*cohort % groups.len()];
                let index = *member % group.members.len() as u32;
                if busy.contains(&group.parent.allotment) {
                    continue;
                }
                let input = chain.state.objects[&group.parent.allotment].reference();
                let tx = chain
                    .tx_with(
                        group.parent.wallet,
                        TxKind::MaterializeAgents {
                            cohort: group.id,
                            members: vec![Materialize {
                                index,
                                member: group.members[index as usize].clone(),
                                siblings: group.tree.prove(index).expect("in the tree"),
                            }],
                        },
                        vec![input],
                    )
                    .expect("sign");
                busy.push(group.parent.allotment);
                let member = &group.members[index as usize];
                let wallet = accounts[&member.agent.expect("keyed members")];
                launches.insert(
                    tx.digest(),
                    (
                        group.parent_index,
                        vec![(wallet, (member.burst, member.rate))],
                    ),
                );
                pending.push(tx);
            }
            Op::Next { gap } => {
                let out = chain.pulse(std::mem::take(&mut pending)).expect("pulse");
                busy.clear();
                let pulse = out.pulse.0;
                for effects in &out.effects {
                    let mut born = launches.get(&effects.digest).cloned();
                    let mut next_child = 0;
                    for event in &effects.events {
                        let child = match event {
                            Event::AgentPaid {
                                charter, amount, ..
                            } => {
                                *paid.entry(*charter).or_default().entry(pulse).or_default() +=
                                    amount.0;
                                None
                            }
                            Event::AgentSpawned {
                                agent, allotment, ..
                            }
                            | Event::AgentMaterialized {
                                agent, allotment, ..
                            }
                            | Event::CharterCreated {
                                charter: agent,
                                allotment,
                            } => Some((*agent, *allotment)),
                            Event::CohortSpawned { cohort, .. } => {
                                if let Some(position) = proposed
                                    .iter()
                                    .position(|(tx, ..)| tx.digest() == effects.digest)
                                {
                                    let (_, parent_index, tree, members) =
                                        proposed.remove(position);
                                    groups.push(Group {
                                        id: *cohort,
                                        parent: nodes[parent_index],
                                        parent_index,
                                        tree,
                                        members,
                                    });
                                }
                                None
                            }
                            _ => None,
                        };
                        if let (Some((charter, allotment)), Some((parent, children))) =
                            (child, born.as_mut())
                        {
                            let (wallet, bound) = children[next_child];
                            next_child += 1;
                            nodes.push(Node {
                                charter,
                                allotment,
                                wallet,
                                parent: Some(*parent),
                                bound,
                            });
                        }
                    }
                }
                proposed.clear();
                launches.clear();
                if *gap > 0 {
                    let to = Pulse(chain.state.pulse.0 + gap);
                    chain.advance_to(to).expect("advance");
                }
            }
        }
    }
    (nodes, paid)
}

/// What the whole tree paid at each Pulse.
fn run(sigma: Amount, rho: Amount, ops: &[Op]) -> BTreeMap<u64, u128> {
    let (_, paid) = run_tree(sigma, rho, ops);
    let mut total: BTreeMap<u64, u128> = BTreeMap::new();
    for by_pulse in paid.values() {
        for (pulse, amount) in by_pulse {
            *total.entry(*pulse).or_default() += amount;
        }
    }
    total
}

/// What the subtree under node `top` paid at each Pulse.
fn subtree_paid(nodes: &[Node], paid: &Paid, top: usize) -> BTreeMap<u64, u128> {
    let below = |mut i: usize| loop {
        if i == top {
            return true;
        }
        match nodes[i].parent {
            Some(parent) => i = parent,
            None => return false,
        }
    };
    let mut total: BTreeMap<u64, u128> = BTreeMap::new();
    for (i, node) in nodes.iter().enumerate() {
        if below(i) {
            for (pulse, amount) in paid.get(&node.charter).into_iter().flatten() {
                *total.entry(*pulse).or_default() += amount;
            }
        }
    }
    total
}

/// The largest `σ + ρ·τ` excess: for every window of Pulses `[t₁, t₂]`,
/// what was paid in it less `σ + ρ·(t₂ − t₁)`; the bound holds if it is
/// never positive.
fn worst_excess(paid: &BTreeMap<u64, u128>, sigma: Amount, rho: Amount) -> i128 {
    let pulses: Vec<(u64, u128)> = paid.iter().map(|(p, a)| (*p, *a)).collect();
    let mut worst = i128::MIN;
    for i in 0..pulses.len() {
        let mut total = 0u128;
        for j in i..pulses.len() {
            total += pulses[j].1;
            let span = u128::from(pulses[j].0 - pulses[i].0);
            let bound = sigma.0 + rho.0 * span;
            worst = worst.max(total as i128 - bound as i128);
        }
    }
    worst
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, .. ProptestConfig::default() })]

    #[test]
    fn a_subtree_pays_at_most_its_roots_burst_and_rate(
        burst in 10u64..500,
        rate in 0u64..80,
        ops in prop::collection::vec(op(), 1..60),
    ) {
        let (sigma, rho) = (tenths(burst), hundredths(rate));
        let (nodes, paid) = run_tree(sigma, rho, &ops);
        for (i, node) in nodes.iter().enumerate() {
            let subtree = subtree_paid(&nodes, &paid, i);
            let (sigma, rho) = node.bound;
            let worst = worst_excess(&subtree, sigma, rho);
            prop_assert!(
                worst <= 0,
                "the subtree under node {i} paid {subtree:?}: {worst} µUSD over σ + ρ·τ"
            );
        }
    }
}

/// A tree that pays as fast as it can: the root and its children try to
/// pay every Pulse more than their buckets refill, so together they pay
/// close to the bound, and never past it.
#[test]
fn a_greedy_tree_reaches_the_bound_and_stops_there() {
    let (sigma, rho) = (tenths(100), hundredths(50));
    let mut ops = vec![
        Op::Spawn {
            parent: 0,
            wallet: 1,
            burst: 30,
            rate: 20,
        },
        Op::Spawn {
            parent: 0,
            wallet: 2,
            burst: 30,
            rate: 10,
        },
        Op::Next { gap: 0 },
    ];
    for _ in 0..40 {
        for node in 0..3 {
            ops.push(Op::Pay { node, amount: 5 });
        }
        ops.push(Op::Next { gap: 0 });
    }
    let paid = run(sigma, rho, &ops);
    assert!(worst_excess(&paid, sigma, rho) <= 0);
    let (first, last) = (paid.keys().next().copied(), paid.keys().last().copied());
    let (Some(first), Some(last)) = (first, last) else {
        panic!("nothing was paid");
    };
    let total: u128 = paid.values().sum();
    let bound = sigma.0 + rho.0 * u128::from(last - first);
    assert!(
        total * 10 >= bound * 8,
        "the tree paid {total} of a bound of {bound}: it should come close"
    );
}
