//! Proof sizes of the 4D index, reproducing the simulation in section 4.6 of
//! the research note "Billions of agents" deterministically.
//!
//! One writer's log of N atoms (default 1,000,000) over 30 days in
//! 1 km × 1 km × 30 m, uniformly spread, 5 % long-lived, and the note's four
//! queries. Every answer is verified and checked against a brute-force
//! scan.
//!
//! ```text
//! cargo run --release -p sion-memory --example proof_sizes -- [atoms] [queries per row] [seed] [log]
//! ```
//!
//! With `log`, the atoms are also kept in a `WriterLog` and its head (log
//! root and index root) is computed the way the command-line tool will,
//! to measure that path's time and memory.
//!
//! The simulated atoms: every twentieth atom is long-lived, the rest
//! short-lived. Positions are points, uniform in the volume. A short-lived
//! atom starts uniformly in the 30 days and lasts 1 s to 10 min, cut at the
//! end of its hour so it stays in one bucket. A long-lived atom starts
//! uniformly; one in ten is open-ended (a map or a standing fact), the rest
//! last 2 h to 10 days. Each atom is an observation claim about one of
//! 10,000 objects. Payloads are excluded, as in the note.

use std::time::{Duration, Instant};

use rand_core::RngCore;
use sion_core::encode::encode;
use sion_core::ids::AccountId;
use sion_core::merkle::Frontier;
use sion_memory::{
    prove_range, verify_range, AtomClass, Box3, FrameId, IndexBuilder, Interval, MemoryAtom,
    MemoryAtomId, MemoryKind, RangeQuery, Statement, TimeRange, Value, WriterLog, BUCKET_NS,
};

const SECOND_NS: u64 = 1_000_000_000;
const HOUR_NS: u64 = 3_600 * SECOND_NS;
const DAY_NS: u64 = 24 * HOUR_NS;
const SPAN_NS: u64 = 30 * DAY_NS;
/// 2026-10-01T00:00:00Z, an hour boundary.
const T0: u64 = 1_790_812_800 * SECOND_NS;
const SIDE_MM: i64 = 1_000_000;
const HEIGHT_MM: i64 = 30_000;

struct Row {
    label: &'static str,
    size_mm: [i64; 3],
    duration_ns: u64,
}

const ROWS: [Row; 4] = [
    Row {
        label: "10 x 10 x 3 m, 1 hour",
        size_mm: [10_000, 10_000, 3_000],
        duration_ns: HOUR_NS,
    },
    Row {
        label: "50 x 50 x 10 m, 1 hour",
        size_mm: [50_000, 50_000, 10_000],
        duration_ns: HOUR_NS,
    },
    Row {
        label: "100 x 100 x 30 m, 6 hours",
        size_mm: [100_000, 100_000, 30_000],
        duration_ns: 6 * HOUR_NS,
    },
    Row {
        label: "200 x 200 x 30 m, 1 day",
        size_mm: [200_000, 200_000, 30_000],
        duration_ns: DAY_NS,
    },
];

fn uniform(rng: &mut impl RngCore, below: u64) -> u64 {
    // Rejection sampling keeps the draw exactly uniform.
    let zone = u64::MAX - u64::MAX % below;
    loop {
        let x = rng.next_u64();
        if x < zone {
            return x % below;
        }
    }
}

fn make_atom(writer: AccountId, frame: FrameId, seq: u64, rng: &mut impl RngCore) -> MemoryAtom {
    let point = [
        uniform(rng, SIDE_MM as u64) as i64,
        uniform(rng, SIDE_MM as u64) as i64,
        uniform(rng, HEIGHT_MM as u64) as i64,
    ];
    let start = T0 + uniform(rng, SPAN_NS);
    let valid = if seq % 20 == 19 {
        if uniform(rng, 10) == 0 {
            Interval::starting(start)
        } else {
            Interval::closed(
                start,
                start + 2 * HOUR_NS + uniform(rng, 10 * DAY_NS - 2 * HOUR_NS),
            )
        }
    } else {
        let bucket_end = (start / BUCKET_NS + 1) * BUCKET_NS;
        let duration = SECOND_NS + uniform(rng, 600 * SECOND_NS);
        Interval::closed(start, (start + duration).min(bucket_end))
    };
    MemoryAtom::builder(
        writer,
        seq,
        MemoryKind::Observation,
        AtomClass::Observation,
        Statement::claim(
            format!("object-{}", seq % 10_000),
            "seen",
            Value::Integer(i128::from(seq)),
        ),
    )
    .valid(valid)
    .observed_at(valid.start)
    .at(frame, point)
    .confidence(900)
    .build()
    .expect("simulated atoms are valid")
}

fn peak_memory_mib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib / 1024)
}

fn millis(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let parse = |i: usize, default: u64| -> u64 {
        args.get(i)
            .map(|a| a.replace('_', "").parse().expect("arguments are integers"))
            .unwrap_or(default)
    };
    let n = parse(1, 1_000_000);
    let per_row = parse(2, 25) as usize;
    let seed = parse(3, 2026);
    let with_log = args.get(4).is_some_and(|a| a == "log");
    let cores = std::thread::available_parallelism().map_or(1, |c| c.get());

    println!("sion-memory proof sizes");
    println!(
        "atoms {n}, queries per row {per_row}, seed {seed}, cores available {cores} (the build runs on one)"
    );
    println!(
        "world: 1 km x 1 km x 30 m, 30 days, 5 % long-lived, point atoms, payloads excluded\n"
    );

    let writer = AccountId::derive(b"proof-sizes-robot");
    let frame = FrameId::named("warehouse");
    let mut rng = sion_core::rng::labeled(seed, "atoms");
    let mut builder = IndexBuilder::new();
    let mut frontier = Frontier::default();
    let mut brute: Vec<(MemoryAtomId, Box3, TimeRange)> = Vec::with_capacity(n as usize);
    let mut log = with_log.then(|| WriterLog::new(writer));
    let started = Instant::now();
    for seq in 0..n {
        let atom = make_atom(writer, frame, seq, &mut rng);
        let id = atom.id();
        builder
            .push_with_id(&atom, id)
            .expect("atoms arrive in log order");
        frontier.append(id.leaf());
        brute.push((id, atom.bbox(), atom.time_range()));
        if let Some(log) = &mut log {
            log.append(atom).expect("valid atoms in order");
        }
    }
    let generated = started.elapsed();
    let started = Instant::now();
    let index = builder.finish();
    let built = started.elapsed();
    let long_lived = brute
        .iter()
        .filter(|(_, _, t)| t.end - t.start > HOUR_NS || t.end == u64::MAX)
        .count();
    println!(
        "generated and hashed {n} atoms in {:.2} s; log root {}",
        generated.as_secs_f64(),
        frontier.root().short()
    );
    println!(
        "index built in {:.2} s: {} nodes, {} long-lived atoms, index root {}",
        built.as_secs_f64(),
        index.node_count(),
        long_lived,
        index.root().short()
    );
    if let Some(log) = &log {
        let started = Instant::now();
        let head = log.head(log.len()).expect("the whole log");
        println!(
            "WriterLog head of {} atoms in {:.2} s (index rebuilt from the stored atoms); roots agree: {}",
            head.size,
            started.elapsed().as_secs_f64(),
            head.root == frontier.root() && head.index_root == index.root()
        );
    }
    if let Some(mib) = peak_memory_mib() {
        println!("peak resident memory so far {mib} MiB");
    }
    println!();
    println!(
        "{:<27} {:>8} {:>9} {:>8} {:>8} {:>8} {:>8} {:>8} {:>9} {:>6}",
        "query",
        "results",
        "proof KiB",
        "max KiB",
        "forest",
        "long",
        "B/result",
        "prove ms",
        "verify ms",
        "nodes"
    );

    let mut qrng = sion_core::rng::labeled(seed, "queries");
    for row in &ROWS {
        let mut results_total = 0usize;
        let mut bytes_total = 0usize;
        let mut bytes_max = 0usize;
        let mut prove_time = Duration::ZERO;
        let mut verify_time = Duration::ZERO;
        let mut hashes = 0usize;
        let mut forest_total = 0usize;
        let mut long_total = 0usize;
        for _ in 0..per_row {
            let mut min = [0i64; 3];
            let limits = [SIDE_MM, SIDE_MM, HEIGHT_MM];
            for axis in 0..3 {
                let room = (limits[axis] - row.size_mm[axis]).max(0) as u64;
                min[axis] = uniform(&mut qrng, room + 1) as i64;
            }
            let max = [
                min[0] + row.size_mm[0],
                min[1] + row.size_mm[1],
                min[2] + row.size_mm[2],
            ];
            let t1 = T0 + uniform(&mut qrng, SPAN_NS - row.duration_ns + 1);
            let query = RangeQuery::new(
                frame,
                Box3::new(min, max),
                TimeRange::new(t1, t1 + row.duration_ns),
            );

            let started = Instant::now();
            let (results, proof) = prove_range(&index, &query).expect("valid query");
            prove_time += started.elapsed();
            let bytes = encode(&proof).len();
            let started = Instant::now();
            verify_range(&index.root(), &query, &results, &proof).expect("the proof verifies");
            verify_time += started.elapsed();

            let expected = brute
                .iter()
                .filter(|(_, b, t)| b.intersects(&query.region) && t.intersects(&query.time))
                .count();
            assert_eq!(
                results.len(),
                expected,
                "the answer equals a brute-force scan"
            );
            results_total += results.len();
            bytes_total += bytes;
            bytes_max = bytes_max.max(bytes);
            hashes += proof.hash_count();
            forest_total += encode(&proof.forest).len();
            long_total += encode(&proof.long_lived).len();
        }
        let k = per_row.max(1) as f64;
        let mean_results = results_total as f64 / k;
        let mean_bytes = bytes_total as f64 / k;
        let per_result = if results_total == 0 {
            "-".to_string()
        } else {
            format!("{:.0}", bytes_total as f64 / results_total as f64)
        };
        println!(
            "{:<27} {:>8.1} {:>9.1} {:>8.1} {:>8.1} {:>8.1} {:>8} {:>8.3} {:>9.3} {:>6.0}",
            row.label,
            mean_results,
            mean_bytes / 1024.0,
            bytes_max as f64 / 1024.0,
            forest_total as f64 / k / 1024.0,
            long_total as f64 / k / 1024.0,
            per_result,
            millis(prove_time) / k,
            millis(verify_time) / k,
            hashes as f64 / k,
        );
    }
    println!();
    println!("means over the queries of each row; forest and long are the two trees' shares of the proof (KiB);");
    println!(
        "nodes counts the node digests and map hashes a verifier computes; every answer verified"
    );
    println!("and matched a brute-force scan");
    if let Some(mib) = peak_memory_mib() {
        println!("peak resident memory {mib} MiB (including the brute-force list)");
    }
}
