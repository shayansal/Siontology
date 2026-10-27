//! `fold bench`: measured costs of the primitives, execution, proving,
//! clearing and an end-to-end devnet.
//!
//! Every number is measured on the machine running the command; nothing is
//! extrapolated. Single-threaded figures are per core.

use std::time::{Duration, Instant};

use fold_assurance::{
    atom::standard, Atom, AtomClass, ClaimRecord, ClaimStatus, Formula, Policy, Ratio, Registry,
};
use fold_clearing::{clear, verify_clearing, Batch, Order, Side};
use fold_core::hash::{hash_domain, Hash32};
use fold_core::ids::{AccountId, AssetId, ClaimId, DomainId, Pulse};
use fold_core::smt::SparseMerkleTree;
use fold_core::Amount;
use fold_crypto::{verify, Keypair, Scheme};
use fold_da::ExtendedSquare;
use fold_exec::pulse::{execute_pulse, execute_pulse_traced, PulseInput};
use fold_exec::sim::{sim_beacon, Chain};
use fold_exec::{replay_chunk, SignedTransaction, TxKind, Wallet};
use fold_node::{Devnet, DevnetConfig};
use serde::Serialize;

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
        "hashing",
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
        let _ = fold_assurance::coc::claim_coc(&registry, &policy, &delivery);
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
        let _ = fold_assurance::decide(&registry, &policy, &delivery, Amount::units(10_000), true);
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
        "proving",
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
        "proving",
        "stateless replay of every chunk (stack A)",
        replay.as_secs_f64() * 1e3,
        "ms",
        format!("{} KiB of witnesses", witness / 1024),
    );
    let start = Instant::now();
    for trace in &traces {
        fold_spec::verify_chunk(trace).expect("stack B accepts the chunk");
    }
    let spec = start.elapsed();
    r.add(
        "proving",
        "stateless replay of every chunk (stack B)",
        spec.as_secs_f64() * 1e3,
        "ms",
        "independent implementation of the specification",
    );
    let _ = output;
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
    devnet.shutdown();
    Ok(())
}

/// A state view fetched once.
struct Prefetched {
    config: fold_exec::ChainConfig,
    pulse: Pulse,
    coins: std::collections::BTreeMap<AccountId, Vec<fold_exec::Object>>,
    objects: std::collections::BTreeMap<fold_core::ids::ObjectId, fold_exec::Object>,
}

impl Prefetched {
    fn fetch(
        ledger: &dyn Ledger,
        config: fold_exec::ChainConfig,
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

impl fold_exec::StateView for Prefetched {
    fn config(&self) -> &fold_exec::ChainConfig {
        &self.config
    }
    fn current_pulse(&self) -> Pulse {
        self.pulse
    }
    fn object(&self, id: &fold_core::ids::ObjectId) -> Option<fold_exec::Object> {
        self.objects.get(id).cloned()
    }
    fn coins(&self, account: &AccountId) -> Vec<fold_exec::Object> {
        self.coins.get(account).cloned().unwrap_or_default()
    }
}

/// The benchmark groups, in the order they run.
pub const GROUPS: [&str; 7] = [
    "signatures",
    "state",
    "data",
    "clearing",
    "assurance",
    "execution",
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
    if with_devnet && wanted("devnet") {
        devnet(&mut r, quick)?;
    }
    Ok(r.results)
}
