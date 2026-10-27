//! The Goldilocks prime field, `p = 2^64 − 2^32 + 1`.
//!
//! Goldilocks is used for the erasure code because its multiplicative group
//! has a subgroup of order `2^32`: radix-2 NTTs of any power-of-two size up to
//! `2^32` exist, so extending a row of `k` symbols to `2k` costs
//! `O(k log k)`. The same field is common in hash-based (STARK) provers, so
//! a future succinct proof of correct encoding can work over these symbols
//! natively.

use std::fmt;
use std::ops::{Add, AddAssign, Mul, MulAssign, Neg, Sub, SubAssign};

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::DaError;

/// The field modulus `2^64 − 2^32 + 1`.
pub const P: u64 = 0xFFFF_FFFF_0000_0001;

/// `2^64 − p = 2^32 − 1`, the correction applied when a 64-bit word wraps.
const EPSILON: u64 = 0xFFFF_FFFF;

/// A field element in canonical form (`< p`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Fp(u64);

impl Fp {
    /// Zero.
    pub const ZERO: Fp = Fp(0);
    /// One.
    pub const ONE: Fp = Fp(1);
    /// A generator of the full multiplicative group.
    pub const MULTIPLICATIVE_GENERATOR: Fp = Fp(7);
    /// The largest `n` with `2^n | p − 1`.
    pub const TWO_ADICITY: u32 = 32;

    /// Reduces any `u64` into the field.
    pub const fn new(value: u64) -> Fp {
        if value >= P {
            Fp(value - P)
        } else {
            Fp(value)
        }
    }

    /// Accepts only canonical values (`< p`).
    pub const fn from_canonical(value: u64) -> Option<Fp> {
        if value < P {
            Some(Fp(value))
        } else {
            None
        }
    }

    /// The canonical integer representative.
    pub const fn value(self) -> u64 {
        self.0
    }

    /// Whether this is zero.
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// Big-endian canonical encoding.
    pub const fn to_bytes(self) -> [u8; 8] {
        self.0.to_be_bytes()
    }

    /// Decodes a big-endian canonical encoding.
    pub fn from_bytes(bytes: [u8; 8]) -> Result<Fp, DaError> {
        let value = u64::from_be_bytes(bytes);
        Fp::from_canonical(value).ok_or(DaError::NonCanonicalField(value))
    }

    /// `self^exponent` by square-and-multiply.
    pub fn pow(self, mut exponent: u64) -> Fp {
        let mut base = self;
        let mut acc = Fp::ONE;
        while exponent > 0 {
            if exponent & 1 == 1 {
                acc *= base;
            }
            base *= base;
            exponent >>= 1;
        }
        acc
    }

    /// The multiplicative inverse, or `None` for zero (Fermat: `x^(p−2)`).
    pub fn inverse(self) -> Option<Fp> {
        if self.is_zero() {
            None
        } else {
            Some(self.pow(P - 2))
        }
    }

    /// A primitive `2^32`-th root of unity: `7^((p − 1) / 2^32)`.
    pub fn two_adic_root() -> Fp {
        Fp::MULTIPLICATIVE_GENERATOR.pow((P - 1) >> Fp::TWO_ADICITY)
    }

    /// A primitive `2^log_n`-th root of unity.
    ///
    /// # Panics
    /// Panics if `log_n > 32`.
    pub fn root_of_unity(log_n: u32) -> Fp {
        assert!(log_n <= Fp::TWO_ADICITY, "Goldilocks has 2-adicity 32");
        Fp::two_adic_root().pow(1u64 << (Fp::TWO_ADICITY - log_n))
    }
}

/// Reduces a 128-bit product modulo `p`, using `2^64 ≡ 2^32 − 1` and
/// `2^96 ≡ −1 (mod p)`.
fn reduce128(x: u128) -> u64 {
    let lo = x as u64;
    let hi = (x >> 64) as u64;
    let hi_hi = hi >> 32;
    let hi_lo = hi & EPSILON;
    let (mut t0, borrow) = lo.overflowing_sub(hi_hi);
    if borrow {
        // Wrapped by 2^64; subtracting EPSILON turns that into + p.
        t0 = t0.wrapping_sub(EPSILON);
    }
    let t1 = hi_lo * EPSILON;
    let (sum, carry) = t0.overflowing_add(t1);
    let mut result = if carry {
        sum.wrapping_add(EPSILON)
    } else {
        sum
    };
    if result >= P {
        result -= P;
    }
    result
}

impl Add for Fp {
    type Output = Fp;
    fn add(self, other: Fp) -> Fp {
        let (sum, carry) = self.0.overflowing_add(other.0);
        let mut result = if carry {
            sum.wrapping_add(EPSILON)
        } else {
            sum
        };
        if result >= P {
            result -= P;
        }
        Fp(result)
    }
}

impl Sub for Fp {
    type Output = Fp;
    fn sub(self, other: Fp) -> Fp {
        let (difference, borrow) = self.0.overflowing_sub(other.0);
        Fp(if borrow {
            difference.wrapping_sub(EPSILON)
        } else {
            difference
        })
    }
}

impl Mul for Fp {
    type Output = Fp;
    fn mul(self, other: Fp) -> Fp {
        Fp(reduce128(u128::from(self.0) * u128::from(other.0)))
    }
}

impl Neg for Fp {
    type Output = Fp;
    fn neg(self) -> Fp {
        if self.0 == 0 {
            self
        } else {
            Fp(P - self.0)
        }
    }
}

impl AddAssign for Fp {
    fn add_assign(&mut self, other: Fp) {
        *self = *self + other;
    }
}

impl SubAssign for Fp {
    fn sub_assign(&mut self, other: Fp) {
        *self = *self - other;
    }
}

impl MulAssign for Fp {
    fn mul_assign(&mut self, other: Fp) {
        *self = *self * other;
    }
}

impl fmt::Debug for Fp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fp({})", self.0)
    }
}

impl fmt::Display for Fp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl BorshSerialize for Fp {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        BorshSerialize::serialize(&self.0, writer)
    }
}

impl BorshDeserialize for Fp {
    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
        let value = u64::deserialize_reader(reader)?;
        Fp::from_canonical(value).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "non-canonical field element",
            )
        })
    }
}

impl Serialize for Fp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(self.0)
    }
}

impl<'de> Deserialize<'de> for Fp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = <u64 as Deserialize>::deserialize(deserializer)?;
        Fp::from_canonical(value)
            .ok_or_else(|| serde::de::Error::custom("non-canonical field element"))
    }
}

/// Inverts every element with one field inversion (Montgomery's trick).
///
/// # Panics
/// Panics if any element is zero.
pub fn batch_inverse(values: &[Fp]) -> Vec<Fp> {
    if values.is_empty() {
        return Vec::new();
    }
    let mut prefix = Vec::with_capacity(values.len());
    let mut acc = Fp::ONE;
    for value in values {
        assert!(!value.is_zero(), "batch_inverse of zero");
        prefix.push(acc);
        acc *= *value;
    }
    let mut inverse = acc.inverse().expect("product of non-zero elements");
    let mut out = vec![Fp::ZERO; values.len()];
    for i in (0..values.len()).rev() {
        out[i] = inverse * prefix[i];
        inverse *= values[i];
    }
    out
}

fn bit_reverse_permute(values: &mut [Fp]) {
    let n = values.len();
    let bits = n.trailing_zeros();
    if bits == 0 {
        return;
    }
    for i in 0..n {
        let j = i.reverse_bits() >> (usize::BITS - bits);
        if i < j {
            values.swap(i, j);
        }
    }
}

/// In-place radix-2 NTT: replaces coefficients `a` with evaluations
/// `A[t] = Σ_j a[j] · root^(j·t)`, where `root` is a primitive `n`-th root
/// of unity and `n = values.len()` is a power of two.
///
/// # Panics
/// Panics if the length is not a power of two.
pub fn ntt(values: &mut [Fp], root: Fp) {
    let n = values.len();
    assert!(n.is_power_of_two(), "NTT size must be a power of two");
    bit_reverse_permute(values);
    let mut len = 2;
    while len <= n {
        let step = root.pow((n / len) as u64);
        let half = len / 2;
        let mut twiddles = Vec::with_capacity(half);
        let mut w = Fp::ONE;
        for _ in 0..half {
            twiddles.push(w);
            w *= step;
        }
        for start in (0..n).step_by(len) {
            for j in 0..half {
                let u = values[start + j];
                let v = values[start + j + half] * twiddles[j];
                values[start + j] = u + v;
                values[start + j + half] = u - v;
            }
        }
        len <<= 1;
    }
}

/// Inverse of [`ntt`] for the same `root`.
///
/// # Panics
/// Panics if the length is not a power of two.
pub fn intt(values: &mut [Fp], root: Fp) {
    let n = values.len();
    ntt(values, root.inverse().expect("roots of unity are non-zero"));
    let n_inverse = Fp::new(n as u64).inverse().expect("n < p");
    for value in values.iter_mut() {
        *value *= n_inverse;
    }
}

/// Evaluates a polynomial given by its coefficients (lowest degree first).
pub fn evaluate(coefficients: &[Fp], x: Fp) -> Fp {
    coefficients
        .iter()
        .rev()
        .fold(Fp::ZERO, |acc, coefficient| acc * x + *coefficient)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn modulus_identities() {
        assert_eq!(Fp::new(P), Fp::ZERO);
        assert_eq!(Fp::new(P + 5), Fp::new(5));
        assert_eq!(Fp::new(P - 1) + Fp::ONE, Fp::ZERO);
        assert_eq!(Fp::ZERO - Fp::ONE, Fp::new(P - 1));
        assert_eq!(-Fp::ONE, Fp::new(P - 1));
        assert_eq!(Fp::new(P - 1) * Fp::new(P - 1), Fp::ONE);
        assert!(Fp::from_canonical(P).is_none());
        assert!(Fp::from_bytes(P.to_be_bytes()).is_err());
        assert_eq!(Fp::from_bytes(Fp::new(42).to_bytes()).unwrap(), Fp::new(42));
    }

    #[test]
    fn two_adic_root_has_exact_order() {
        let root = Fp::two_adic_root();
        assert_eq!(root.pow(1u64 << 32), Fp::ONE);
        assert_eq!(root.pow(1u64 << 31), -Fp::ONE);
        for log_n in 0..=12 {
            let w = Fp::root_of_unity(log_n);
            assert_eq!(w.pow(1u64 << log_n), Fp::ONE);
            if log_n > 0 {
                assert_ne!(w.pow(1u64 << (log_n - 1)), Fp::ONE);
            }
        }
    }

    #[test]
    fn ntt_matches_naive_dft_and_inverts() {
        for log_n in 0..=6u32 {
            let n = 1usize << log_n;
            let root = Fp::root_of_unity(log_n);
            let coefficients: Vec<Fp> = (0..n).map(|i| Fp::new((i as u64 + 3) * 977)).collect();
            let mut values = coefficients.clone();
            ntt(&mut values, root);
            for (t, value) in values.iter().enumerate() {
                assert_eq!(*value, evaluate(&coefficients, root.pow(t as u64)));
            }
            intt(&mut values, root);
            assert_eq!(values, coefficients);
        }
    }

    #[test]
    fn batch_inverse_matches_single() {
        let values: Vec<Fp> = (1..50u64).map(|i| Fp::new(i * 1_000_003)).collect();
        let inverses = batch_inverse(&values);
        for (value, inverse) in values.iter().zip(inverses) {
            assert_eq!(*value * inverse, Fp::ONE);
        }
    }

    #[test]
    fn serialization_rejects_non_canonical() {
        let json = serde_json::to_string(&Fp::new(9)).unwrap();
        assert_eq!(json, "9");
        assert!(serde_json::from_str::<Fp>(&P.to_string()).is_err());
        let bytes = borsh::to_vec(&Fp::new(9)).unwrap();
        assert_eq!(borsh::from_slice::<Fp>(&bytes).unwrap(), Fp::new(9));
        let bad = borsh::to_vec(&P).unwrap();
        assert!(borsh::from_slice::<Fp>(&bad).is_err());
    }

    proptest! {
        #[test]
        fn reduction_matches_modulo(a in any::<u64>(), b in any::<u64>()) {
            let x = Fp::new(a);
            let y = Fp::new(b);
            let expected = ((u128::from(x.value()) * u128::from(y.value())) % u128::from(P)) as u64;
            prop_assert_eq!((x * y).value(), expected);
            let sum = ((u128::from(x.value()) + u128::from(y.value())) % u128::from(P)) as u64;
            prop_assert_eq!((x + y).value(), sum);
            let difference = ((u128::from(x.value()) + u128::from(P) - u128::from(y.value())) % u128::from(P)) as u64;
            prop_assert_eq!((x - y).value(), difference);
        }

        #[test]
        fn raw_reduce128_matches(x in any::<u128>()) {
            prop_assert_eq!(reduce128(x), (x % u128::from(P)) as u64);
        }

        #[test]
        fn inverses(a in 1u64..P) {
            let x = Fp::new(a);
            prop_assert_eq!(x * x.inverse().unwrap(), Fp::ONE);
        }
    }
}
