//! Shamir secret sharing over GF(2^8), byte by byte.
//!
//! Information-theoretically secure: any `t − 1` shares reveal nothing about
//! the secret, with or without a quantum computer.

use borsh::{BorshDeserialize, BorshSerialize};
use rand_core::CryptoRngCore;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::error::CryptoError;

/// Multiplication in GF(2^8) with the AES polynomial x^8 + x^4 + x^3 + x + 1.
fn gf_mul(mut a: u8, mut b: u8) -> u8 {
    let mut product = 0u8;
    while b != 0 {
        if b & 1 == 1 {
            product ^= a;
        }
        let carry = a & 0x80;
        a <<= 1;
        if carry != 0 {
            a ^= 0x1b;
        }
        b >>= 1;
    }
    product
}

/// Multiplicative inverse (a^254) in GF(2^8); `a` must be nonzero.
fn gf_inv(a: u8) -> u8 {
    debug_assert!(a != 0);
    let mut result = 1u8;
    let mut base = a;
    let mut exponent = 254u32;
    while exponent > 0 {
        if exponent & 1 == 1 {
            result = gf_mul(result, base);
        }
        base = gf_mul(base, base);
        exponent >>= 1;
    }
    result
}

/// One share: the evaluation point `x` (1..=255) and one byte per secret byte.
#[derive(
    Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize, Zeroize,
)]
pub struct Share {
    /// Evaluation point, never zero.
    pub x: u8,
    /// Evaluations.
    pub y: Vec<u8>,
}

/// Splits `secret` into `n` shares, any `threshold` of which reconstruct it.
pub fn split(
    secret: &[u8],
    threshold: u8,
    n: u8,
    rng: &mut impl CryptoRngCore,
) -> Result<Vec<Share>, CryptoError> {
    if threshold == 0 || threshold > n {
        return Err(CryptoError::InvalidShares("threshold must be in 1..=n"));
    }
    let mut shares: Vec<Share> = (1..=n)
        .map(|x| Share {
            x,
            y: Vec::with_capacity(secret.len()),
        })
        .collect();
    let mut coefficients = vec![0u8; usize::from(threshold)];
    for &byte in secret {
        coefficients[0] = byte;
        rng.fill_bytes(&mut coefficients[1..]);
        for share in &mut shares {
            // Horner evaluation at x.
            let mut value = 0u8;
            for &coefficient in coefficients.iter().rev() {
                value = gf_mul(value, share.x) ^ coefficient;
            }
            share.y.push(value);
        }
    }
    coefficients.zeroize();
    Ok(shares)
}

/// Reconstructs the secret from at least `threshold` distinct shares
/// (extra shares are ignored; the first `threshold` are used).
pub fn combine(shares: &[Share], threshold: u8) -> Result<Vec<u8>, CryptoError> {
    let t = usize::from(threshold);
    if shares.len() < t || t == 0 {
        return Err(CryptoError::NotEnoughShares {
            needed: t,
            have: shares.len(),
        });
    }
    let used = &shares[..t];
    let length = used[0].y.len();
    for (i, share) in used.iter().enumerate() {
        if share.x == 0 {
            return Err(CryptoError::InvalidShares("zero evaluation point"));
        }
        if share.y.len() != length {
            return Err(CryptoError::InvalidShares("share lengths differ"));
        }
        if used[..i].iter().any(|other| other.x == share.x) {
            return Err(CryptoError::InvalidShares("duplicate evaluation point"));
        }
    }
    // Lagrange basis values at zero: l_i(0) = Π_{j≠i} x_j / (x_j − x_i); in
    // characteristic 2, subtraction is XOR.
    let basis: Vec<u8> = used
        .iter()
        .enumerate()
        .map(|(i, share_i)| {
            let mut numerator = 1u8;
            let mut denominator = 1u8;
            for (j, share_j) in used.iter().enumerate() {
                if i != j {
                    numerator = gf_mul(numerator, share_j.x);
                    denominator = gf_mul(denominator, share_j.x ^ share_i.x);
                }
            }
            gf_mul(numerator, gf_inv(denominator))
        })
        .collect();
    let mut secret = vec![0u8; length];
    for (byte_index, out) in secret.iter_mut().enumerate() {
        let mut value = 0u8;
        for (share, &weight) in used.iter().zip(&basis) {
            value ^= gf_mul(share.y[byte_index], weight);
        }
        *out = value;
    }
    Ok(secret)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn field_inverse() {
        for a in 1..=255u8 {
            assert_eq!(gf_mul(a, gf_inv(a)), 1, "inverse of {a}");
        }
    }

    #[test]
    fn any_threshold_subset_reconstructs() {
        let mut rng = fold_core::rng::seeded(4);
        let secret = b"0123456789abcdef0123456789abcdef";
        let shares = split(secret, 3, 5, &mut rng).unwrap();
        for a in 0..5 {
            for b in (a + 1)..5 {
                for c in (b + 1)..5 {
                    let subset = vec![shares[a].clone(), shares[b].clone(), shares[c].clone()];
                    assert_eq!(combine(&subset, 3).unwrap(), secret);
                }
            }
        }
        assert!(combine(&shares[..2], 3).is_err());
    }

    #[test]
    fn fewer_shares_give_wrong_secret() {
        let mut rng = fold_core::rng::seeded(5);
        let secret = [42u8; 32];
        let shares = split(&secret, 3, 5, &mut rng).unwrap();
        // Combining two shares as if the threshold were 2 does not yield the secret.
        assert_ne!(combine(&shares[..2], 2).unwrap(), secret.to_vec());
    }

    proptest! {
        #[test]
        fn round_trip(secret in proptest::collection::vec(any::<u8>(), 0..64), t in 1u8..6, extra in 0u8..4, seed in any::<u64>()) {
            let n = t + extra;
            let mut rng = fold_core::rng::seeded(seed);
            let shares = split(&secret, t, n, &mut rng).unwrap();
            let start = usize::from(extra);
            prop_assert_eq!(combine(&shares[start..], t).unwrap(), secret);
        }
    }
}
