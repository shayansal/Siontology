//! End-to-end tests of the data-availability layer: encode, commit, sample,
//! withhold, reconstruct, detect bad encodings.

use std::collections::BTreeMap;
use std::time::Instant;

use proptest::prelude::*;
use rand_core::RngCore;
use sion_core::rng::seeded;
use sion_da::{
    reconstruct_blob, verify_bad_encoding_proof, verify_extended_square, Availability, DaError,
    DaHeader, ExtendedSquare, Fp, LightSampler, Share,
};

fn blob(len: usize, seed: u64) -> Vec<u8> {
    let mut rng = seeded(seed);
    let mut out = vec![0u8; len];
    rng.fill_bytes(&mut out);
    out
}

fn verified_shares(
    square: &ExtendedSquare,
    header: &DaHeader,
    keep: impl Fn(usize, usize) -> bool,
) -> BTreeMap<(usize, usize), Fp> {
    let width = square.width();
    let mut shares = BTreeMap::new();
    for r in 0..width {
        for c in 0..width {
            if keep(r, c) {
                let share = square.share(r, c).unwrap();
                header.verify_share(r, c, &share).unwrap();
                shares.insert((r, c), share.value);
            }
        }
    }
    shares
}

#[test]
fn round_trips_through_partial_shares_for_many_sizes() {
    for len in [
        0usize, 1, 2, 6, 7, 8, 13, 14, 15, 20, 21, 22, 63, 64, 65, 200, 777, 1500, 5000,
    ] {
        let data = blob(len, len as u64);
        let square = ExtendedSquare::encode(&data).unwrap();
        let header = square.header();
        header.verify_self_consistent().unwrap();
        verify_extended_square(&square).unwrap();
        assert_eq!(
            square.decode_blob().unwrap(),
            data,
            "direct decode, len {len}"
        );
        // Keep only the parity quadrants: no original symbol is served.
        let k = square.k();
        let shares = verified_shares(&square, &header, |r, c| r >= k || c >= k);
        assert_eq!(
            reconstruct_blob(&header, &shares).unwrap(),
            data,
            "len {len}"
        );
    }
}

#[test]
fn modified_share_fails_its_proof() {
    let square = ExtendedSquare::encode(&blob(500, 3)).unwrap();
    let header = square.header();
    let width = square.width();
    for (r, c) in [
        (0, 0),
        (1, width - 1),
        (width - 1, 2),
        (width / 2, width / 2),
    ] {
        let mut share = square.share(r, c).unwrap();
        share.value += Fp::ONE;
        assert_eq!(
            header.verify_share(r, c, &share),
            Err(DaError::InvalidShare { row: r, column: c })
        );
        let mut share = square.share(r, c).unwrap();
        share.proof[0].0[0] ^= 1;
        assert!(header.verify_share(r, c, &share).is_err());
        // Right share, wrong position.
        let share = square.share(r, c).unwrap();
        let other = (c + 1) % width;
        if square.get(r, other) != share.value {
            assert!(header.verify_share(r, other, &share).is_err());
        }
    }
}

#[test]
fn headers_and_shares_serialize() {
    let square = ExtendedSquare::encode(b"serialize me").unwrap();
    let header = square.header();
    let share = square.share(1, 2).unwrap();

    let bytes = borsh::to_vec(&header).unwrap();
    assert_eq!(borsh::from_slice::<DaHeader>(&bytes).unwrap(), header);
    let json = serde_json::to_string(&share).unwrap();
    let back: Share = serde_json::from_str(&json).unwrap();
    assert_eq!(back, share);
    header.verify_share(1, 2, &back).unwrap();
}

#[test]
fn producer_cannot_hide_bad_encoding_from_full_nodes() {
    let mut square = ExtendedSquare::encode_with_k(&blob(300, 9), 8).unwrap();
    let value = square.get(10, 3);
    square.set(10, 3, value + Fp::new(5));
    let header = square.header();
    // Light clients sampling this square see only valid shares...
    let report = LightSampler::new(30, 1)
        .run(&header, |r, c| square.share(r, c).ok())
        .unwrap();
    assert_eq!(report.outcome, Availability::Available);
    // ...but anyone with the square can prove the encoding is bad.
    assert!(verify_extended_square(&square).is_err());
    let proof = square.find_bad_encoding().unwrap();
    verify_bad_encoding_proof(&header, &proof).unwrap();
}

#[test]
#[ignore = "timing; run with --release -- --ignored --nocapture"]
fn encode_one_megabyte() {
    let data = blob(1 << 20, 1);
    let start = Instant::now();
    let square = ExtendedSquare::encode(&data).unwrap();
    let extend = start.elapsed();
    let header = square.header();
    let total = start.elapsed();
    println!(
        "1 MiB blob: k = {}, {} shares, extension {:?}, extension + commitments {:?}",
        square.k(),
        square.width() * square.width(),
        extend,
        total
    );
    assert_eq!(header.width(), square.width());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// Any pattern that leaves each row with at least k symbols recovers.
    #[test]
    fn rows_with_k_symbols_always_recover(len in 0usize..2500, seed in any::<u64>()) {
        let data = blob(len, seed);
        let square = ExtendedSquare::encode(&data).unwrap();
        let header = square.header();
        let k = square.k();
        let width = square.width();
        let mut rng = seeded(seed ^ 0x5eed);
        let mut shares = BTreeMap::new();
        for r in 0..width {
            // Drop a random subset of at most k positions from this row.
            let mut columns: Vec<usize> = (0..width).collect();
            for i in (1..columns.len()).rev() {
                let j = (rng.next_u64() % (i as u64 + 1)) as usize;
                columns.swap(i, j);
            }
            let drop = (rng.next_u64() % (k as u64 + 1)) as usize;
            for &c in &columns[drop..] {
                shares.insert((r, c), square.get(r, c));
            }
        }
        prop_assert_eq!(reconstruct_blob(&header, &shares).unwrap(), data);
    }

    /// Arbitrary random erasure: reconstruction either returns the exact blob
    /// or reports the data as unrecoverable — never a wrong blob.
    #[test]
    fn random_erasure_never_yields_wrong_data(len in 0usize..1500, seed in any::<u64>(), keep_percent in 20u64..90) {
        let data = blob(len, seed);
        let square = ExtendedSquare::encode(&data).unwrap();
        let header = square.header();
        let width = square.width();
        let mut rng = seeded(seed.wrapping_add(17));
        let mut shares = BTreeMap::new();
        for r in 0..width {
            for c in 0..width {
                if rng.next_u64() % 100 < keep_percent {
                    shares.insert((r, c), square.get(r, c));
                }
            }
        }
        match reconstruct_blob(&header, &shares) {
            Ok(recovered) => prop_assert_eq!(recovered, data),
            Err(DaError::Unrecoverable { .. }) => {}
            Err(other) => prop_assert!(false, "unexpected error {other}"),
        }
    }
}
