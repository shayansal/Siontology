//! `sion bench`: measured costs of the primitives, execution, proving,
//! clearing and an end-to-end devnet.
//!
//! Every number is measured on the machine running the command; nothing is
//! extrapolated. Single-threaded figures are per core.

use std::time::{Duration, Instant};

use serde::Serialize;
use sion_assurance::{
    atom::standard, Atom, AtomClass, ClaimRecord, ClaimStatus, Formula, Policy, Ratio, Registry,
};
use sion_clearing::{clear, verify_clearing, Batch, Order, Side};
use sion_core::encode::encode;
use sion_core::hash::{hash_domain, Hash32};
use sion_core::ids::{AccountId, AssetId, ClaimId, DomainId, Pulse};
use sion_core::smt::SparseMerkleTree;
use sion_core::Amount;
use sion_crypto::{verify, Keypair, Scheme};
use sion_da::ExtendedSquare;
use sion_exec::pulse::{execute_pulse, execute_pulse_traced, PulseInput};
use sion_exec::sim::{sim_beacon, Chain};
use sion_exec::{replay_chunk, SignedTransaction, TxKind, Wallet};
use sion_node::transport::TrafficStats;
use sion_node::{Devnet, DevnetConfig};

use crate::error::CliError;
use crate::ledger::{Ledger, LedgerExt, TxState};

/// One measurement.
#[derive(Clone, Debug, Serialize)]
pub struct Measurement {
    /// Group.
    pub group: String,
    /// What was measured.
    pub name: String,
    /// The value.
    pub value: f64,
    /// Its unit.
    pub unit: String,
    /// Context (sizes, counts).
    pub note: String,
}

fn per_op(total: Duration, ops: usize) -> f64 {
    total.as_secs_f64() * 1e6 / ops.max(1) as f64
}

struct Recorder {
    results: Vec<Measurement>,
    print: bool,
}

impl Recorder {
    fn add(&mut self, group: &str, name: &str, value: f64, unit: &str, note: impl Into<String>) {
        let m = Measurement {
            group: group.into(),
            name: name.into(),
            value,
            unit: unit.into(),
            note: note.into(),
        };
        if self.print {
            println!(
                "{:<12} {:<52} {:>14.2} {:<8} {}",
                m.group, m.name, m.value, m.unit, m.note
            );
        }
        self.results.push(m);
    }
}

fn signatures(r: &mut Recorder, quick: bool) {
    for (scheme, n) in [
        (Scheme::MlDsa44, if quick { 20 } else { 100 }),
        (Scheme::SlhDsaSha2_128f, if quick { 5 } else { 20 }),
        (Scheme::SlhDsaSha2_128s, if quick { 1 } else { 3 }),
    ] {
        let keys = Keypair::from_seed(scheme, &[3u8; 32]);
        let message = [7u8; 32];
        let start = Instant::now();
        let mut signature = None;
        for _ in 0..n {
            signature = Some(keys.sign_deterministic("bench", &message).expect("sign"));
        }
        let sign = per_op(start.elapsed(), n);
        let signature = signature.expect("signed");
        let start = Instant::now();
        for _ in 0..n {
            verify(keys.public(), "bench", &message, &signature).expect("verify");
        }
        let verify_us = per_op(start.elapsed(), n);
        let name = scheme.name();
        r.add("signatures", &format!("{name} sign"), sign, "µs", "");
        r.add(
            "signatures",
            &format!("{name} verify"),
            verify_us,
            "µs",
            format!("{} B signature", signature.bytes.len()),
        );
    }
}

fn hashing_and_trees(r: &mut Recorder, quick: bool) {
    let n = if quick { 20_000 } else { 200_000 };
    let start = Instant::now();
    let mut h = Hash32::ZERO;
    for _ in 0..n {
        h = hash_domain("bench", &h.0);
    }
    r.add(
        "state",
        "SHA3-256, 32-byte input",
        per_op(start.elapsed(), n),
        "µs",
        "",
    );
    for objects in [1_000usize, if quick { 10_000 } else { 100_000 }] {
        let mut tree = SparseMerkleTree::new();
        for i in 0..objects {
            let key = hash_domain("bench/key", &(i as u64).to_be_bytes());
            tree.insert(key, key);
        }
        let start = Instant::now();
        let root = tree.root();
        r.add(
            "state",
            &format!("state root of {objects} objects"),
            start.elapsed().as_secs_f64() * 1e3,
            "ms",
            format!("root {}", root.short()),
        );
    }
}

fn availability(r: &mut Recorder, quick: bool) {
    let sizes: &[usize] = if quick {
        &[16 * 1024, 256 * 1024]
    } else {
        &[16 * 1024, 256 * 1024, 2 * 1024 * 1024]
    };
    for &size in sizes {
        let blob: Vec<u8> = (0..size).map(|i| (i * 31 % 251) as u8).collect();
        let start = Instant::now();
        let square = ExtendedSquare::encode(&blob).expect("encode");
        let encode = start.elapsed();
        let start = Instant::now();
        let header = square.header();
        let commit = start.elapsed();
        r.add(
            "data",
            &format!("2D Reed–Solomon extend, {} KiB", size / 1024),
            encode.as_secs_f64() * 1e3,
            "ms",
            format!("{0}×{0} extended square", 2 * header.k),
        );
        r.add(
            "data",
            &format!("row/column commitments, {} KiB", size / 1024),
            commit.as_secs_f64() * 1e3,
            "ms",
            "",
        );
    }
}

fn clearing(r: &mut Recorder, quick: bool) {
    for n in [10usize, 100, if quick { 400 } else { 2_000 }] {
        let orders: Vec<Order> = (0..n)
            .map(|i| Order {
                id: hash_domain("bench/order", &(i as u64).to_be_bytes()),
                owner: AccountId::derive(&(i as u64).to_be_bytes()),
                side: if i % 2 == 0 { Side::Buy } else { Side::Sell },
                quantity: 1 + (i as u64 * 7) % 20,
                limit_price: Amount(1_500_000 + ((i as u128 * 7919) % 1_000_000)),
            })
            .collect();
        let batch = Batch {
            domain: DomainId::derive(b"bench"),
            pulse: Pulse(1),
            beacon: hash_domain("bench/beacon", b"1"),
            orders,
        };
        let start = Instant::now();
        let result = clear(&batch).expect("clear");
        let solve = start.elapsed();
        let start = Instant::now();
        verify_clearing(&batch, &result).expect("verify");
        let check = start.elapsed();
        r.add(
            "clearing",
            &format!("solve batch of {n} orders"),
            solve.as_secs_f64() * 1e3,
            "ms",
            format!("{} units traded", result.volume),
        );
        r.add(
            "clearing",
            &format!("verify certificate, {n} orders"),
            check.as_secs_f64() * 1e3,
            "ms",
            "exact rational arithmetic",
        );
    }
}

fn assurance(r: &mut Recorder) {
    let mut registry = Registry::with_standard_atoms();
    let mut atoms = Vec::new();
    for i in 0..64u64 {
        let atom = Atom::bonded(AtomClass::Economic, format!("w{i}"));
        registry.register_atom(atom.clone()).expect("register");
        registry
            .post_bond(
                atom.id,
                AccountId::derive(&i.to_be_bytes()),
                Amount::units(1_000 + i),
                Pulse(0),
            )
            .expect("bond");
        atoms.push(atom.id);
    }
    // A delivery claim backed by any of 64 witnesses, and an invoice on it.
    let delivery = ClaimId::derive(b"bench/delivery");
    registry
        .issue_claim(ClaimRecord {
            id: delivery,
            issuer: AccountId::derive(b"issuer"),
            formula: Formula::and(vec![
                Formula::atom(standard::sha3().id),
                Formula::or(atoms.iter().map(|a| Formula::atom(*a))),
            ]),
            adjudicator: None,
            detection: Ratio::ONE,
            issued_at: Pulse(0),
            valid_until: Pulse(100),
            challenge_until: Pulse(110),
            status: ClaimStatus::Active,
            sequence: 0,
        })
        .expect("issue");
    let policy = Policy::standard();
    let n = 2_000;
    let start = Instant::now();
    for _ in 0..n {
        let _ = sion_assurance::coc::claim_coc(&registry, &policy, &delivery);
    }
    r.add(
        "assurance",
        "cost of corruption, 64-witness claim",
        per_op(start.elapsed(), n),
        "µs",
        "",
    );
    let start = Instant::now();
    for _ in 0..n {
        let _ = sion_assurance::decide(&registry, &policy, &delivery, Amount::units(10_000), true);
    }
    r.add(
        "assurance",
        "decision rule with reservation check",
        per_op(start.elapsed(), n),
        "µs",
        "",
    );
}

fn transfers(chain: &mut Chain, names: &[String]) -> Vec<SignedTransaction> {
    let ids: Vec<AccountId> = names.iter().map(|n| chain.id(n)).collect();
    names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            chain
                .tx(
                    name,
                    TxKind::Transfer {
                        asset: AssetId::usd(),
                        outputs: vec![(ids[(i + 1) % ids.len()], Amount::units(1))],
                    },
                )
                .expect("build")
        })
        .collect()
}

fn execution(r: &mut Recorder, quick: bool) {
    let n = if quick { 64 } else { 512 };
    let names: Vec<String> = (0..n).map(|i| format!("bench-{i}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut chain = Chain::new(&refs, 1_000, 1);
    let start = Instant::now();
    let txs = transfers(&mut chain, &names);
    r.add(
        "execution",
        "client signing (ML-DSA-44 + SLH-DSA-128f)",
        per_op(start.elapsed(), n),
        "µs/tx",
        format!("{n} transfers"),
    );
    let input = |pulse: Pulse, txs: Vec<SignedTransaction>| PulseInput {
        pulse,
        beacon: sim_beacon(pulse),
        owned: txs,
        contended: Vec::new(),
        forced: Vec::new(),
    };
    for (label, verify) in [
        ("with signature checks", true),
        ("state machine only", false),
    ] {
        let mut state = chain.state.clone();
        let pulse = state.pulse.next();
        let start = Instant::now();
        let output =
            execute_pulse(&mut state, &input(pulse, txs.clone()), verify).expect("execute");
        let elapsed = start.elapsed();
        assert_eq!(output.status_counts().0, n);
        r.add(
            "execution",
            &format!("Pulse of {n} transfers, {label}"),
            n as f64 / elapsed.as_secs_f64(),
            "tx/s",
            format!("{:.1} ms per Pulse, one core", elapsed.as_secs_f64() * 1e3),
        );
    }
    let mut state = chain.state.clone();
    let pulse = state.pulse.next();
    let start = Instant::now();
    let (output, traces) =
        execute_pulse_traced(&mut state, &input(pulse, txs), true, 64).expect("trace");
    let traced = start.elapsed();
    r.add(
        "execution",
        &format!("traced execution of {n} transfers"),
        traced.as_secs_f64() * 1e3,
        "ms",
        format!("{} chunks with witnesses", traces.len()),
    );
    let start = Instant::now();
    for trace in &traces {
        replay_chunk(trace).expect("replay");
    }
    let replay = start.elapsed();
    let witness: usize = traces.iter().map(|t| t.witness.size()).sum();
    r.add(
        "execution",
        "stateless replay of every chunk (stack A)",
        replay.as_secs_f64() * 1e3,
        "ms",
        format!("{} KiB of witnesses", witness / 1024),
    );
    let start = Instant::now();
    for trace in &traces {
        sion_spec::verify_chunk(trace).expect("stack B accepts the chunk");
    }
    let spec = start.elapsed();
    r.add(
        "execution",
        "stateless replay of every chunk (stack B)",
        spec.as_secs_f64() * 1e3,
        "ms",
        "independent implementation of the specification",
    );
    let _ = output;
}

/// Payment streams: what one payment costs the payee, the payer and the
/// ledger. A payee checks a payment with one hash; the ledger checks it
/// again, one hash per step, when the payee settles a batch.
/// Authorization (spec v1.13): what checking one action's authorization
/// costs signed by itself and as a member of a batch, in time and in bytes,
/// and what a Pulse of agent payments costs with each.
fn authorization(r: &mut Recorder, quick: bool) {
    use sion_exec::auth::check_authorization;
    use sion_exec::tx::{sign_batch, Transaction};

    let config = sion_exec::ChainConfig::devnet(None);
    let wallet = Wallet::named("bench-signer");
    let body = |nonce: u64| Transaction {
        sender: wallet.id,
        genesis_policy: None,
        inputs: vec![sion_exec::ObjectRef {
            id: sion_core::ids::ObjectId::derive(&nonce.to_be_bytes()),
            version: sion_core::ids::Version(0),
        }],
        fee: Amount(1_000),
        kind: TxKind::Transfer {
            asset: AssetId::usd(),
            outputs: vec![(wallet.id, Amount(1))],
        },
        expires_at: Pulse(100),
        nonce,
    };
    let n = if quick { 8 } else { 40 };
    let own: Vec<SignedTransaction> = (0..n)
        .map(|i| body(i).sign(&wallet.keys, &config).expect("sign"))
        .collect();
    let start = Instant::now();
    for tx in &own {
        check_authorization(&config, tx, &wallet.policy).expect("verifies");
    }
    r.add(
        "authorization",
        "signed by itself: check",
        per_op(start.elapsed(), n as usize),
        "µs",
        "ML-DSA-44 + SLH-DSA-SHA2-128f, per transaction",
    );
    let own_bytes = encode(&own[0].auth).len() + 1;
    r.add(
        "authorization",
        "signed by itself: bytes",
        own_bytes as f64,
        "B",
        "signatures, per transaction",
    );
    for size in [16u64, 64, 256] {
        let members = sign_batch(
            &wallet.keys,
            wallet.id,
            &config,
            (0..size).map(|i| (body(10_000 + i), None)).collect(),
        )
        .expect("sign a batch");
        // A fresh thread, so no batch is remembered as checked before.
        let (config2, policy) = (config.clone(), wallet.policy.clone());
        let checked = std::thread::spawn(move || {
            let start = Instant::now();
            for member in &members {
                check_authorization(&config2, member, &policy).expect("verifies");
            }
            let elapsed = start.elapsed();
            (elapsed, members)
        })
        .join()
        .expect("thread");
        let (elapsed, members) = checked;
        r.add(
            "authorization",
            &format!("batch of {size}: check per member"),
            per_op(elapsed, size as usize),
            "µs",
            "one signature check per batch, one inclusion path per member",
        );
        // On the wire the batch's signatures travel once; each member carries
        // its place: the batch, its index, its path, a charter option.
        let shared = encode(&members[0].auth).len() as f64;
        let place = (encode(&members[0].batch).len()) as f64;
        r.add(
            "authorization",
            &format!("batch of {size}: bytes per member"),
            shared / size as f64 + place,
            "B",
            format!("{place} B of place and path, plus a {size}th of the signatures"),
        );
    }
    // A Pulse of agent payments: each signed by its own subagent, or every
    // one a keyless subagent's, in one batch its parent signs.
    let count = if quick { 32 } else { 256 };
    for keyless in [false, true] {
        let (chain, txs) = agent_payments(count, keyless);
        let mut state = chain.state.clone();
        let pulse = state.pulse.next();
        let input = PulseInput {
            pulse,
            beacon: sim_beacon(pulse),
            owned: txs,
            contended: Vec::new(),
            forced: Vec::new(),
        };
        // A fresh thread: nothing is remembered as checked.
        let (elapsed, ok) = std::thread::spawn(move || {
            let start = Instant::now();
            let output = execute_pulse(&mut state, &input, true).expect("execute");
            (start.elapsed(), output.status_counts().0)
        })
        .join()
        .expect("thread");
        assert_eq!(ok, count, "every payment succeeds");
        let label = if keyless {
            "keyless subagents, one batch"
        } else {
            "keyed subagents, each signed by itself"
        };
        r.add(
            "authorization",
            &format!("Pulse of {count} agent payments, {label}"),
            per_op(elapsed, count),
            "µs/tx",
            format!(
                "{:.1} ms per Pulse with signature checks, one core",
                elapsed.as_secs_f64() * 1e3
            ),
        );
    }
}

/// Lineage ids, overlays and cohorts (spec v1.14): what a client spends
/// building a cohort's sum tree, what a cohort and its materializations
/// cost on the wire and in fees, what a materialized agent costs in state,
/// and what an agent's payment reads at the deepest generation.
fn lineage(r: &mut Recorder, quick: bool) {
    use sion_exec::charter::{Caveats, Spawn, MAX_LINEAGE_DEPTH};
    use sion_exec::cohort::{CohortMember, CohortTree, Materialize, MAX_MATERIALIZE};

    let count: u32 = if quick { 1 << 14 } else { 1 << 20 };
    let member = CohortMember {
        agent: None,
        card: Hash32::ZERO,
        budget: Amount::units(1),
        burst: Amount::units(1),
        rate: Amount(100),
    };
    let start = Instant::now();
    let tree = CohortTree::build(&vec![member.clone(); count as usize]).expect("a tree");
    r.add(
        "lineage",
        &format!("cohort of {count}: build its sum tree"),
        start.elapsed().as_secs_f64() * 1e3,
        "ms",
        "every leaf and inner node, one core, by the client",
    );
    let top = tree.root();

    // A root agent, a chain of generations below it, and a cohort.
    let mut chain = Chain::new(&["principal", "agent", "vendor"], 10_000_000, 8);
    chain.replay = false;
    let agent = chain.id("agent");
    let vendor = chain.id("vendor");
    let caveats = Caveats {
        payees: Some([vendor].into_iter().collect()),
        max_per_payment: Amount::units(10),
        burst: Amount::units(1_000_000_000),
        rate: Amount::units(1_000_000),
        expires_at: Pulse(10_000_000),
        evidence_above: None,
        evidence_policy: Policy::standard(),
        receipts: false,
        may_delegate: true,
    };
    let event_pair = |out: &sion_exec::PulseOutput| {
        out.effects
            .iter()
            .flat_map(|e| e.events.iter())
            .find_map(|event| match event {
                sion_exec::Event::AgentRegistered {
                    agent, allotment, ..
                }
                | sion_exec::Event::AgentSpawned {
                    agent, allotment, ..
                } => Some((*agent, *allotment)),
                _ => None,
            })
            .expect("an agent")
    };
    let register = chain
        .tx(
            "principal",
            TxKind::RegisterAgent {
                agent,
                card: Hash32::ZERO,
                caveats: caveats.clone(),
                funding: Amount::units(5_000_000),
                agent_bond: Amount::ZERO,
            },
        )
        .expect("register");
    let out = chain.pulse(vec![register]).expect("pulse");
    let (root, root_allotment) = event_pair(&out);

    let input = chain.state.objects[&root_allotment].reference();
    let mut members_caveats = caveats.clone();
    members_caveats.burst = Amount::units(1);
    members_caveats.rate = Amount(100);
    members_caveats.may_delegate = false;
    let spawn_cohort = chain
        .tx_with(
            "agent",
            TxKind::SpawnCohort {
                charter: root,
                root: top.hash,
                count,
                burst: top.burst,
                rate: top.rate,
                full: false,
                caveats: members_caveats,
            },
            vec![input],
        )
        .expect("cohort");
    r.add(
        "lineage",
        &format!("SpawnCohort of {count}: bytes"),
        spawn_cohort.size() as f64,
        "B",
        "the whole signed transaction",
    );
    r.add(
        "lineage",
        &format!("SpawnCohort of {count}: fee"),
        spawn_cohort.tx.fee.0 as f64,
        "µUSD",
        "devnet prices",
    );
    let identities = 10_000_000_000u64;
    let cohorts = identities.div_ceil(u64::from(sion_exec::cohort::MAX_COHORT));
    r.add(
        "lineage",
        "10^10 identities: SpawnCohort transactions",
        cohorts as f64,
        "tx",
        format!(
            "our arithmetic: cohorts of 2^20; about {:.0} USD in fees at the measured fee per cohort",
            cohorts as f64 * spawn_cohort.tx.fee.0 as f64 / 1e6
        ),
    );
    let out = chain.pulse(vec![spawn_cohort]).expect("pulse");
    let cohort = out
        .effects
        .iter()
        .flat_map(|e| e.events.iter())
        .find_map(|event| match event {
            sion_exec::Event::CohortSpawned { cohort, .. } => Some(*cohort),
            _ => None,
        })
        .expect("a cohort");

    // A full batch of members.
    let batch = MAX_MATERIALIZE.min(count as usize) as u32;
    let members: Vec<Materialize> = (0..batch)
        .map(|index| Materialize {
            index,
            member: member.clone(),
            siblings: tree.prove(index).expect("a proof"),
        })
        .collect();
    let input = chain.state.objects[&root_allotment].reference();
    let materialize = chain
        .tx_with(
            "agent",
            TxKind::MaterializeAgents { cohort, members },
            vec![input],
        )
        .expect("materialize");
    r.add(
        "lineage",
        &format!("MaterializeAgents of {batch}: bytes per member"),
        materialize.size() as f64 / f64::from(batch),
        "B",
        format!(
            "proofs of {} siblings each",
            sion_exec::cohort::height(count)
        ),
    );
    r.add(
        "lineage",
        &format!("MaterializeAgents of {batch}: fee per member"),
        materialize.tx.fee.0 as f64 / f64::from(batch),
        "µUSD",
        "devnet prices",
    );
    let mut state = chain.state.clone();
    let pulse = state.pulse.next();
    let input_pulse = PulseInput {
        pulse,
        beacon: sim_beacon(pulse),
        owned: vec![materialize.clone()],
        contended: Vec::new(),
        forced: Vec::new(),
    };
    let (elapsed, output) = std::thread::spawn(move || {
        let start = Instant::now();
        let output = execute_pulse(&mut state, &input_pulse, true).expect("execute");
        (start.elapsed(), output)
    })
    .join()
    .expect("thread");
    assert_eq!(output.status_counts().0, 1, "the materialization succeeds");
    r.add(
        "lineage",
        &format!("MaterializeAgents of {batch}: execution per member"),
        per_op(elapsed, batch as usize),
        "µs",
        "proof checks, creates and the signature check, one core",
    );
    let out = chain.pulse(vec![materialize]).expect("pulse");
    let (first, first_allotment) = out
        .effects
        .iter()
        .flat_map(|e| e.events.iter())
        .find_map(|event| match event {
            sion_exec::Event::AgentMaterialized {
                agent, allotment, ..
            } => Some((*agent, *allotment)),
            _ => None,
        })
        .expect("a member");
    let state_bytes = encode(&chain.state.objects[&first]).len()
        + encode(&chain.state.objects[&first_allotment]).len();
    r.add(
        "lineage",
        "a materialized member: state",
        state_bytes as f64,
        "B",
        "its charter and allotment, encoded (a keyless member has no account object)",
    );

    // Generations down to the deepest, then a payment from it.
    let (mut charter, mut allotment) = (root, root_allotment);
    for _ in 0..MAX_LINEAGE_DEPTH {
        let input = chain.state.objects[&allotment].reference();
        let mut child = caveats.clone();
        child.burst = Amount::units(1_000);
        child.rate = Amount::units(1);
        let tx = chain
            .tx_with(
                "agent",
                TxKind::SpawnAgents {
                    charter,
                    children: vec![Spawn {
                        agent: Some(agent),
                        card: Hash32::ZERO,
                        caveats: child,
                        budget: Amount::units(100),
                    }],
                },
                vec![input],
            )
            .expect("spawn");
        let out = chain.pulse(vec![tx]).expect("pulse");
        (charter, allotment) = event_pair(&out);
    }
    let input = chain.state.objects[&allotment].reference();
    let pay = chain
        .tx_with(
            "agent",
            TxKind::AgentPay {
                charter,
                payee: vendor,
                amount: Amount::units(1),
                purpose: "bench".into(),
                evidence: None,
            },
            vec![input],
        )
        .expect("pay");
    let mut state = chain.state.clone();
    let pulse = state.pulse.next();
    let input = PulseInput {
        pulse,
        beacon: sim_beacon(pulse),
        owned: vec![pay],
        contended: Vec::new(),
        forced: Vec::new(),
    };
    let (_, traces) = execute_pulse_traced(&mut state, &input, true, 1).expect("execute");
    let chunk = traces
        .iter()
        .find(|t| !t.statement.deferred_phase)
        .expect("a transaction chunk");
    let absent = chunk
        .witness
        .objects
        .iter()
        .filter(|(_, object, _)| object.is_none())
        .count();
    r.add(
        "lineage",
        &format!("a payment at depth {MAX_LINEAGE_DEPTH}: objects read"),
        chunk.witness.objects.len() as f64,
        "objects",
        format!(
            "{absent} of them proven absent (the overlays of the agent and its ancestors, and the coins it creates)"
        ),
    );
    r.add(
        "lineage",
        &format!("a payment at depth {MAX_LINEAGE_DEPTH}: witness"),
        encode(&chunk.witness.objects).len() as f64,
        "B",
        "objects and their state proofs",
    );
}

/// A chain where an agent has `count` subagents with gas and budgets, and
/// one agent payment from each: signed by each keyed subagent, or by the
/// parent in one batch for keyless ones.
fn agent_payments(count: usize, keyless: bool) -> (Chain, Vec<SignedTransaction>) {
    use sion_exec::charter::{Caveats, Spawn};
    use sion_exec::sim::Sender;

    let names: Vec<String> = (0..count).map(|i| format!("sub-{i}")).collect();
    let mut all: Vec<&str> = vec!["principal", "parent", "vendor"];
    if !keyless {
        all.extend(names.iter().map(String::as_str));
    }
    let mut chain = Chain::new(&all, 1_000_000, 4);
    let vendor = chain.id("vendor");
    let caveats = Caveats {
        payees: Some([vendor].into_iter().collect()),
        max_per_payment: Amount::units(10),
        burst: Amount::units(1_000_000),
        rate: Amount::units(1_000),
        expires_at: Pulse(1_000_000),
        evidence_above: None,
        evidence_policy: Policy::standard(),
        receipts: false,
        may_delegate: true,
    };
    let parent = chain.id("parent");
    let register = chain
        .tx(
            "principal",
            TxKind::RegisterAgent {
                agent: parent,
                card: Hash32::ZERO,
                caveats: caveats.clone(),
                funding: Amount::units(100_000),
                agent_bond: Amount::ZERO,
            },
        )
        .expect("register");
    let out = chain.pulse(vec![register]).expect("pulse");
    let (root, root_allotment) = out
        .effects
        .iter()
        .flat_map(|e| e.events.iter())
        .find_map(|event| match event {
            sion_exec::Event::AgentRegistered {
                agent, allotment, ..
            } => Some((*agent, *allotment)),
            _ => None,
        })
        .expect("registered");
    let mut child = caveats.clone();
    child.burst = Amount::units(100);
    child.rate = Amount::units(1);
    let children: Vec<Spawn> = names
        .iter()
        .map(|name| Spawn {
            agent: (!keyless).then(|| chain.id(name)),
            card: Hash32::ZERO,
            caveats: child.clone(),
            budget: Amount::units(10),
        })
        .collect();
    let input = chain.state.objects[&root_allotment].reference();
    let spawn = chain
        .tx_with(
            "parent",
            TxKind::SpawnAgents {
                charter: root,
                children,
            },
            vec![input],
        )
        .expect("spawn");
    let out = chain.pulse(vec![spawn]).expect("pulse");
    let spawned: Vec<(
        sion_core::ids::ObjectId,
        AccountId,
        sion_core::ids::ObjectId,
    )> = out
        .effects
        .iter()
        .flat_map(|e| e.events.iter())
        .filter_map(|event| match event {
            sion_exec::Event::AgentSpawned {
                agent,
                account,
                allotment,
                ..
            } => Some((*agent, *account, *allotment)),
            _ => None,
        })
        .collect();
    if keyless {
        let gas = chain
            .tx(
                "parent",
                TxKind::Transfer {
                    asset: AssetId::usd(),
                    outputs: spawned
                        .iter()
                        .map(|(_, account, _)| (*account, Amount::units(1)))
                        .collect(),
                },
            )
            .expect("gas");
        chain.pulse(vec![gas]).expect("pulse");
    }
    let pay = |charter| TxKind::AgentPay {
        charter,
        payee: vendor,
        amount: Amount::units(1),
        purpose: "bench".into(),
        evidence: None,
    };
    let txs = if keyless {
        let actions = spawned
            .iter()
            .map(|(charter, _, allotment)| {
                let input = chain.state.objects[allotment].reference();
                (Sender::Keyless(*charter), pay(*charter), vec![input])
            })
            .collect();
        chain.batch("parent", actions).expect("batch")
    } else {
        names
            .iter()
            .zip(&spawned)
            .map(|(name, (charter, _, allotment))| {
                let input = chain.state.objects[allotment].reference();
                chain
                    .tx_with(name, pay(*charter), vec![input])
                    .expect("pay")
            })
            .collect()
    };
    (chain, txs)
}

fn streams(r: &mut Recorder, quick: bool) {
    use sion_exec::stream::{step, PayerChain, MAX_SETTLE_STEPS};

    let n = if quick { 200_000 } else { 2_000_000 };
    let mut x = Hash32([1; 32]);
    let start = Instant::now();
    for _ in 0..n {
        x = step(&x);
    }
    let elapsed = start.elapsed();
    std::hint::black_box(x);
    r.add(
        "streams",
        "payee checks a payment (one SHA3-256 hash)",
        per_op(elapsed, n),
        "µs",
        format!(
            "{:.2} million payments/s per core",
            n as f64 / elapsed.as_secs_f64() / 1e6
        ),
    );

    let length = if quick { 100_000 } else { 1_000_000 };
    let start = Instant::now();
    let mut chain = PayerChain::new(Hash32([2; 32]), length);
    r.add(
        "streams",
        &format!("payer builds a chain of {length} steps (once)"),
        start.elapsed().as_secs_f64() * 1e3,
        "ms",
        "one hash per step",
    );
    let start = Instant::now();
    for position in 1..=length {
        std::hint::black_box(chain.element(position));
    }
    r.add(
        "streams",
        "payer reveals the next element",
        per_op(start.elapsed(), length as usize),
        "µs",
        "in order; checkpoints every √n steps",
    );

    // Settlement: k payees each settle s steps in one Pulse, executed with
    // signature checks as a validator executes it: by walking from the head
    // (one hash per step), then by a jump to a checkpoint (a bounded number
    // of hashes, however many steps).
    let (k, walk_steps, jump_steps) = if quick {
        (4usize, 4_096u64, 1u64 << 16)
    } else {
        (16, MAX_SETTLE_STEPS, 1u64 << 20)
    };
    let walked = settlements(k, walk_steps, 0);
    assert_eq!(walked.covered, walk_steps);
    r.add(
        "streams",
        &format!("Pulse of {k} settlements × {walk_steps} steps, walked"),
        walked.payments / walked.execute.as_secs_f64(),
        "payments/s",
        format!(
            "{:.1} ms per Pulse with signature checks, one core; one hash per step",
            walked.execute.as_secs_f64() * 1e3
        ),
    );
    r.add(
        "streams",
        "stateless replay of those settlements (stack A)",
        walked.payments / walked.replay.as_secs_f64(),
        "payments/s",
        format!("{:.1} ms", walked.replay.as_secs_f64() * 1e3),
    );
    r.add(
        "streams",
        "fee per payment, walked settlement",
        walked.fee.0 as f64 / walk_steps as f64,
        "µUSD",
        format!("{} USD for {walk_steps} steps", walked.fee),
    );
    let spacing = 1_024;
    let jumped = settlements(k, jump_steps, spacing);
    r.add(
        "streams",
        &format!(
            "Pulse of {k} settlements × {} steps, by checkpoint",
            jumped.covered
        ),
        jumped.payments / jumped.execute.as_secs_f64(),
        "payments/s",
        format!(
            "{:.1} ms per Pulse with signature checks, one core; {} hashes each",
            jumped.execute.as_secs_f64() * 1e3,
            jumped.hashes
        ),
    );
    r.add(
        "streams",
        "stateless replay of those settlements (stack A)",
        jumped.payments / jumped.replay.as_secs_f64(),
        "payments/s",
        format!("{:.1} ms", jumped.replay.as_secs_f64() * 1e3),
    );
    r.add(
        "streams",
        "fee per 1,000 payments, settlement by checkpoint",
        jumped.fee.0 as f64 * 1_000.0 / jumped.covered as f64,
        "µUSD",
        format!("{} USD for {} steps", jumped.fee, jumped.covered),
    );
    r.add(
        "streams",
        "published bytes per 1,000 payments, by checkpoint",
        jumped.body as f64 * 1_000.0 / jumped.covered as f64,
        "B",
        format!(
            "{} B body; {} B with signatures, which are verified, not published",
            jumped.body, jumped.full
        ),
    );
}

/// A measured Pulse of settlements.
struct Settled {
    /// Steps each settlement covered.
    covered: u64,
    payments: f64,
    execute: Duration,
    replay: Duration,
    fee: Amount,
    hashes: u64,
    body: usize,
    full: usize,
}

/// Opens `k` streams of `steps` steps (with checkpoints every `spacing`
/// steps, if not 0), then measures one Pulse in which every payee settles
/// its stream: to the top by walking, or by a jump to a typical position,
/// half a spacing above a checkpoint (the top of a chain is its last
/// checkpoint, whose path is shortest).
fn settlements(k: usize, steps: u64, spacing: u64) -> Settled {
    use sion_exec::stream::PayerChain;

    let payers: Vec<String> = (0..k).map(|i| format!("payer-{i}")).collect();
    let payees: Vec<String> = (0..k).map(|i| format!("payee-{i}")).collect();
    let names: Vec<&str> = payers.iter().chain(&payees).map(String::as_str).collect();
    let mut sim = Chain::new(&names, 10_000, 1);
    let mut chains: Vec<PayerChain> = (0..k)
        .map(|i| {
            let seed = hash_domain("bench/stream", &(i as u64).to_be_bytes());
            match spacing {
                0 => PayerChain::new(seed, steps),
                spacing => PayerChain::with_checkpoints(seed, steps, spacing),
            }
        })
        .collect();
    let opens: Vec<SignedTransaction> = (0..k)
        .map(|i| {
            let payee = sim.id(&payees[i]);
            sim.tx(
                &payers[i],
                TxKind::OpenStream {
                    payee,
                    asset: AssetId::usd(),
                    unit: Amount(1),
                    length: steps,
                    anchor: chains[i].anchor(),
                    deposit: Amount::units(steps.div_ceil(1_000_000).max(1) + 1),
                    until: Pulse(1_000),
                    checkpoints: chains[i].checkpoints(),
                },
            )
            .expect("build")
        })
        .collect();
    let out = sim.pulse(opens.clone()).expect("open");
    let streams: Vec<sion_core::ids::ObjectId> = opens
        .iter()
        .map(|tx| {
            out.effects
                .iter()
                .find(|e| e.digest == tx.digest())
                .and_then(|e| {
                    e.events.iter().find_map(|event| match event {
                        sion_exec::Event::StreamOpened { stream, .. } => Some(*stream),
                        _ => None,
                    })
                })
                .expect("opened")
        })
        .collect();
    let position = match spacing {
        0 => steps,
        spacing => steps - spacing / 2,
    };
    let settles: Vec<SignedTransaction> = (0..k)
        .map(|i| {
            let jump = (spacing > 0).then(|| chains[i].jump(position / spacing).expect("jump"));
            sim.tx(
                &payees[i],
                TxKind::SettleStream {
                    stream: streams[i],
                    position,
                    value: chains[i].element(position).expect("element"),
                    jump,
                },
            )
            .expect("build")
        })
        .collect();
    let hashes = match &settles[0].tx.kind {
        TxKind::SettleStream {
            jump: Some(jump), ..
        } => position % spacing + 1 + jump.path.len() as u64,
        _ => steps,
    };
    let mut state = sim.state.clone();
    let pulse = state.pulse.next();
    let input = PulseInput {
        pulse,
        beacon: sim_beacon(pulse),
        owned: Vec::new(),
        contended: settles.clone(),
        forced: Vec::new(),
    };
    let start = Instant::now();
    let output = execute_pulse(&mut state, &input, true).expect("execute");
    let execute = start.elapsed();
    assert_eq!(output.status_counts().0, k, "every settlement succeeds");
    let mut state = sim.state.clone();
    let (_, traces) = execute_pulse_traced(&mut state, &input, true, 64).expect("trace");
    let start = Instant::now();
    for trace in &traces {
        replay_chunk(trace).expect("replay");
    }
    let replay = start.elapsed();
    Settled {
        covered: position,
        payments: k as f64 * position as f64,
        execute,
        replay,
        fee: settles[0].tx.fee,
        hashes,
        body: settles.iter().map(|t| t.tx.size()).sum::<usize>() / k,
        full: settles.iter().map(|t| encode(t).len()).sum::<usize>() / k,
    }
}

fn devnet(r: &mut Recorder, quick: bool) -> Result<(), CliError> {
    let accounts = if quick { 8 } else { 32 };
    let names: Vec<String> = (0..accounts).map(|i| format!("load-{i}")).collect();
    let config = DevnetConfig {
        demo_accounts: names.clone(),
        demo_usd: 10_000,
        ..DevnetConfig::default()
    };
    let devnet = Devnet::start(config).map_err(|e| CliError::Node(e.to_string()))?;
    let ledger: &dyn Ledger = &devnet;
    let chain_config = ledger.chain_config()?;
    // Wait for the chain to run, then measure the Pulse cadence.
    let start_pulse = ledger.pulse()?;
    let t0 = Instant::now();
    while ledger.pulse()? < Pulse(start_pulse.0 + 5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let cadence = t0.elapsed().as_secs_f64() / 5.0;
    r.add(
        "devnet",
        "Pulse interval (4 validators, dual PQ signatures)",
        cadence * 1e3,
        "ms",
        "4 consensus rounds per Pulse",
    );
    let (idle_pulse, idle_traffic) = traffic(ledger)?;
    // Each account sends four transfers at once (it holds four coins).
    // Build against one prefetched view, as a wallet with a synced cache would.
    let mut wallets: Vec<Wallet> = names.iter().map(|n| Wallet::named(n)).collect();
    let ids: Vec<AccountId> = wallets.iter().map(|w| w.id).collect();
    let view = Prefetched::fetch(ledger, chain_config, &ids)?;
    let mut txs = Vec::new();
    for (i, wallet) in wallets.iter_mut().enumerate() {
        wallet.validity = 500;
        for k in 0..4 {
            txs.push(
                wallet
                    .build(
                        &view,
                        TxKind::Transfer {
                            asset: AssetId::usd(),
                            outputs: vec![(ids[(i + k + 1) % ids.len()], Amount::units(1))],
                        },
                        Vec::new(),
                    )
                    .map_err(CliError::from)?,
            );
        }
    }
    let total = txs.len();
    let start = Instant::now();
    let mut submitted = Vec::with_capacity(total);
    for tx in &txs {
        let receipt = ledger.submit(tx)?;
        submitted.push((receipt.digest, Instant::now()));
    }
    let mut latencies = Vec::with_capacity(total);
    let mut pending: Vec<_> = submitted;
    let deadline = Instant::now() + Duration::from_secs(120);
    while !pending.is_empty() {
        let mut still = Vec::new();
        for (digest, at) in pending {
            match ledger.tx(digest)? {
                TxState::Executed(_) => latencies.push(at.elapsed()),
                TxState::Rejected(reason) => return Err(CliError::Transaction(reason)),
                _ => still.push((digest, at)),
            }
        }
        pending = still;
        if Instant::now() > deadline {
            return Err(CliError::Transaction("load test did not finish".into()));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let wall = start.elapsed();
    latencies.sort();
    let pct = |p: f64| {
        latencies[((latencies.len() as f64 - 1.0) * p).round() as usize].as_secs_f64() * 1e3
    };
    r.add(
        "devnet",
        &format!("{total} transfers from {accounts} accounts: throughput"),
        total as f64 / wall.as_secs_f64(),
        "tx/s",
        "lane certification + consensus + execution, one machine",
    );
    r.add("devnet", "submit → executed, median", pct(0.5), "ms", "");
    r.add(
        "devnet",
        "submit → executed, 95th percentile",
        pct(0.95),
        "ms",
        "",
    );
    // What validator 0's transport carried during the load, per Pulse and
    // per message class (Borsh bytes, as they would cross a network).
    let (pulse, traffic_after) = traffic(ledger)?;
    let pulses = pulse.0.saturating_sub(idle_pulse.0).max(1) as f64;
    let sent = |stats: &TrafficStats, class: &str| {
        stats.classes.get(class).map(|t| t.sent).unwrap_or_default()
    };
    for class in ["control", "consensus", "ingress", "bulk"] {
        let (before, after) = (sent(&idle_traffic, class), sent(&traffic_after, class));
        r.add(
            "devnet",
            &format!("validator 0 sends per Pulse, {class} class"),
            after.bytes.saturating_sub(before.bytes) as f64 / pulses / 1e3,
            "KB",
            format!(
                "{:.0} messages per Pulse, during the load",
                after.messages.saturating_sub(before.messages) as f64 / pulses
            ),
        );
    }
    let bytes = traffic_after
        .sent()
        .bytes
        .saturating_sub(idle_traffic.sent().bytes);
    r.add(
        "devnet",
        "validator 0 sends per transfer, all classes",
        bytes as f64 / total as f64 / 1e3,
        "KB",
        "consensus traffic of the whole load included",
    );
    devnet.shutdown();
    Ok(())
}

/// The last executed Pulse and what validator 0's transport has carried.
fn traffic(ledger: &dyn Ledger) -> Result<(Pulse, TrafficStats), CliError> {
    let status = ledger.status()?;
    let pulse = serde_json::from_value(status["pulse"].clone())
        .map_err(|e| CliError::Protocol(e.to_string()))?;
    let stats = serde_json::from_value(status["network"].clone()).unwrap_or_default();
    Ok((pulse, stats))
}

/// A state view fetched once.
struct Prefetched {
    config: sion_exec::ChainConfig,
    pulse: Pulse,
    coins: std::collections::BTreeMap<AccountId, Vec<sion_exec::Object>>,
    objects: std::collections::BTreeMap<sion_core::ids::ObjectId, sion_exec::Object>,
}

impl Prefetched {
    fn fetch(
        ledger: &dyn Ledger,
        config: sion_exec::ChainConfig,
        accounts: &[AccountId],
    ) -> Result<Self, CliError> {
        let mut coins = std::collections::BTreeMap::new();
        let mut objects = std::collections::BTreeMap::new();
        for account in accounts {
            let view = ledger.account(*account)?;
            if let Some(object) = view.object {
                objects.insert(object.id, object);
            }
            coins.insert(*account, view.coins);
        }
        Ok(Prefetched {
            config,
            pulse: ledger.pulse()?,
            coins,
            objects,
        })
    }
}

impl sion_exec::StateView for Prefetched {
    fn config(&self) -> &sion_exec::ChainConfig {
        &self.config
    }
    fn current_pulse(&self) -> Pulse {
        self.pulse
    }
    fn object(&self, id: &sion_core::ids::ObjectId) -> Option<sion_exec::Object> {
        self.objects.get(id).cloned()
    }
    fn coins(&self, account: &AccountId) -> Vec<sion_exec::Object> {
        self.coins.get(account).cloned().unwrap_or_default()
    }
}

/// The benchmark groups, in the order they run.
pub const GROUPS: [&str; 10] = [
    "signatures",
    "authorization",
    "state",
    "data",
    "clearing",
    "assurance",
    "execution",
    "streams",
    "lineage",
    "devnet",
];

/// Runs the benchmarks; `quick` uses smaller sizes. `only` selects groups
/// from [`GROUPS`] (all when empty).
pub fn run(
    quick: bool,
    with_devnet: bool,
    only: &[String],
    print: bool,
) -> Result<Vec<Measurement>, CliError> {
    if let Some(unknown) = only.iter().find(|g| !GROUPS.contains(&g.as_str())) {
        return Err(CliError::Usage(format!(
            "unknown benchmark group {unknown:?}; groups: {}",
            GROUPS.join(", ")
        )));
    }
    let wanted = |group: &str| only.is_empty() || only.iter().any(|g| g == group);
    let mut r = Recorder {
        results: Vec::new(),
        print,
    };
    if print {
        println!(
            "{:<12} {:<52} {:>14} {:<8} note",
            "group", "measurement", "value", "unit"
        );
    }
    if wanted("signatures") {
        signatures(&mut r, quick);
    }
    if wanted("authorization") {
        authorization(&mut r, quick);
    }
    if wanted("state") {
        hashing_and_trees(&mut r, quick);
    }
    if wanted("data") {
        availability(&mut r, quick);
    }
    if wanted("clearing") {
        clearing(&mut r, quick);
    }
    if wanted("assurance") {
        assurance(&mut r);
    }
    if wanted("execution") {
        execution(&mut r, quick);
    }
    if wanted("streams") {
        streams(&mut r, quick);
    }
    if wanted("lineage") {
        lineage(&mut r, quick);
    }
    if with_devnet && wanted("devnet") {
        devnet(&mut r, quick)?;
    }
    Ok(r.results)
}
