//! A serializable exact rational number.
//!
//! [`Q`] wraps `num_rational::BigRational`. Values are always kept in lowest
//! terms with a positive denominator, so every number has exactly one Borsh
//! encoding: a certificate hashed by one node hashes identically everywhere.
//!
//! Borsh layout: `sign: u8` (0 zero, 1 positive, 2 negative), then the
//! numerator magnitude and the denominator as minimal big-endian byte strings
//! (`Vec<u8>`). Zero is `(0, [], [1])`. Decoding rejects any other encoding of
//! the same value (leading zero bytes, unreduced fractions, zero denominators).

use std::cmp::Ordering;
use std::fmt;
use std::io;
use std::ops::{Add, AddAssign, Div, Mul, MulAssign, Neg, Sub, SubAssign};
use std::str::FromStr;

use borsh::{BorshDeserialize, BorshSerialize};
use num_bigint::{BigInt, Sign};
use num_integer::Integer;
use num_rational::BigRational;
use num_traits::{One, Signed, ToPrimitive, Zero};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An exact rational number in lowest terms.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Q(BigRational);

impl Default for Q {
    fn default() -> Self {
        Q::zero()
    }
}

impl Q {
    /// Zero.
    pub fn zero() -> Q {
        Q(BigRational::zero())
    }

    /// One.
    pub fn one() -> Q {
        Q(BigRational::one())
    }

    /// An integer.
    pub fn from_integer(value: impl Into<BigInt>) -> Q {
        Q(BigRational::from_integer(value.into()))
    }

    /// `numerator / denominator`, reduced. Panics if `denominator` is zero.
    pub fn new(numerator: impl Into<BigInt>, denominator: impl Into<BigInt>) -> Q {
        let denominator = denominator.into();
        assert!(!denominator.is_zero(), "rational with zero denominator");
        Q(BigRational::new(numerator.into(), denominator))
    }

    /// Wraps a `BigRational`, normalizing it.
    pub fn from_ratio(value: BigRational) -> Q {
        let (numer, denom) = value.into_raw();
        Q(BigRational::new(numer, denom))
    }

    /// The underlying ratio.
    pub fn as_ratio(&self) -> &BigRational {
        &self.0
    }

    /// Consumes the wrapper.
    pub fn into_ratio(self) -> BigRational {
        self.0
    }

    /// The numerator (sign included).
    pub fn numer(&self) -> &BigInt {
        self.0.numer()
    }

    /// The denominator (always positive).
    pub fn denom(&self) -> &BigInt {
        self.0.denom()
    }

    /// Whether the value is zero.
    pub fn is_zero(&self) -> bool {
        self.0.is_zero()
    }

    /// Whether the value is strictly negative.
    pub fn is_negative(&self) -> bool {
        self.0.is_negative()
    }

    /// Whether the value is strictly positive.
    pub fn is_positive(&self) -> bool {
        self.0.is_positive()
    }

    /// Whether the value is an integer.
    pub fn is_integer(&self) -> bool {
        self.0.is_integer()
    }

    /// The largest integer not greater than the value.
    pub fn floor(&self) -> BigInt {
        self.0.floor().to_integer()
    }

    /// The floor as a `u128`, if it is non-negative and fits.
    pub fn floor_u128(&self) -> Option<u128> {
        self.floor().to_u128()
    }

    /// The value as a `u64` if it is a non-negative integer that fits.
    pub fn to_u64_exact(&self) -> Option<u64> {
        if self.is_integer() {
            self.numer().to_u64()
        } else {
            None
        }
    }

    /// Absolute value.
    pub fn abs(&self) -> Q {
        Q(self.0.abs())
    }

    /// The larger of two values.
    pub fn max(self, other: Q) -> Q {
        if self >= other {
            self
        } else {
            other
        }
    }

    /// The smaller of two values.
    pub fn min(self, other: Q) -> Q {
        if self <= other {
            self
        } else {
            other
        }
    }

    /// Reciprocal; panics on zero.
    pub fn recip(&self) -> Q {
        assert!(!self.is_zero(), "reciprocal of zero");
        Q(self.0.recip())
    }

    /// A lossy approximation for display and statistics only. Never use
    /// floating point in consensus-critical logic.
    pub fn to_f64_lossy(&self) -> f64 {
        self.0.to_f64().unwrap_or(f64::NAN)
    }
}

macro_rules! binary_op {
    ($trait:ident, $method:ident) => {
        impl $trait for Q {
            type Output = Q;
            fn $method(self, rhs: Q) -> Q {
                Q(self.0.$method(rhs.0))
            }
        }

        impl<'b> $trait<&'b Q> for Q {
            type Output = Q;
            fn $method(self, rhs: &'b Q) -> Q {
                Q(self.0.$method(&rhs.0))
            }
        }

        impl<'a, 'b> $trait<&'b Q> for &'a Q {
            type Output = Q;
            fn $method(self, rhs: &'b Q) -> Q {
                Q((&self.0).$method(&rhs.0))
            }
        }

        impl<'a> $trait<Q> for &'a Q {
            type Output = Q;
            fn $method(self, rhs: Q) -> Q {
                Q((&self.0).$method(rhs.0))
            }
        }
    };
}

binary_op!(Add, add);
binary_op!(Sub, sub);
binary_op!(Mul, mul);
binary_op!(Div, div);

impl AddAssign<&Q> for Q {
    fn add_assign(&mut self, rhs: &Q) {
        self.0 += &rhs.0;
    }
}

impl AddAssign for Q {
    fn add_assign(&mut self, rhs: Q) {
        self.0 += rhs.0;
    }
}

impl SubAssign<&Q> for Q {
    fn sub_assign(&mut self, rhs: &Q) {
        self.0 -= &rhs.0;
    }
}

impl SubAssign for Q {
    fn sub_assign(&mut self, rhs: Q) {
        self.0 -= rhs.0;
    }
}

impl MulAssign<&Q> for Q {
    fn mul_assign(&mut self, rhs: &Q) {
        self.0 *= &rhs.0;
    }
}

impl Neg for Q {
    type Output = Q;
    fn neg(self) -> Q {
        Q(-self.0)
    }
}

impl Neg for &Q {
    type Output = Q;
    fn neg(self) -> Q {
        Q(-&self.0)
    }
}

impl std::iter::Sum for Q {
    fn sum<I: Iterator<Item = Q>>(iter: I) -> Q {
        iter.fold(Q::zero(), |acc, value| acc + value)
    }
}

impl<'a> std::iter::Sum<&'a Q> for Q {
    fn sum<I: Iterator<Item = &'a Q>>(iter: I) -> Q {
        iter.fold(Q::zero(), |acc, value| acc + value)
    }
}

macro_rules! from_integer {
    ($($ty:ty),*) => {
        $(
            impl From<$ty> for Q {
                fn from(value: $ty) -> Q {
                    Q::from_integer(BigInt::from(value))
                }
            }
        )*
    };
}

from_integer!(i32, i64, i128, u32, u64, u128, usize);

impl From<BigInt> for Q {
    fn from(value: BigInt) -> Q {
        Q::from_integer(value)
    }
}

impl From<BigRational> for Q {
    fn from(value: BigRational) -> Q {
        Q::from_ratio(value)
    }
}

impl PartialEq<i64> for Q {
    fn eq(&self, other: &i64) -> bool {
        self.0 == BigRational::from_integer(BigInt::from(*other))
    }
}

impl PartialOrd<i64> for Q {
    fn partial_cmp(&self, other: &i64) -> Option<Ordering> {
        Some(self.0.cmp(&BigRational::from_integer(BigInt::from(*other))))
    }
}

impl fmt::Display for Q {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.denom().is_one() {
            write!(f, "{}", self.numer())
        } else {
            write!(f, "{}/{}", self.numer(), self.denom())
        }
    }
}

impl fmt::Debug for Q {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Q({self})")
    }
}

/// Errors parsing a rational from text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid rational: {0}")]
pub struct ParseQError(String);

impl FromStr for Q {
    type Err = ParseQError;

    fn from_str(text: &str) -> Result<Q, ParseQError> {
        let text = text.trim();
        let (numer_text, denom_text) = match text.split_once('/') {
            Some((n, d)) => (n.trim(), d.trim()),
            None => (text, "1"),
        };
        let numer =
            BigInt::from_str(numer_text).map_err(|e| ParseQError(format!("{text}: {e}")))?;
        let denom =
            BigInt::from_str(denom_text).map_err(|e| ParseQError(format!("{text}: {e}")))?;
        if denom.is_zero() {
            return Err(ParseQError(format!("{text}: zero denominator")));
        }
        Ok(Q::new(numer, denom))
    }
}

impl Serialize for Q {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Q {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <String as Deserialize>::deserialize(deserializer)?;
        Q::from_str(&text).map_err(serde::de::Error::custom)
    }
}

impl BorshSerialize for Q {
    fn serialize<W: io::Write>(&self, writer: &mut W) -> io::Result<()> {
        let (sign, magnitude) = self.numer().to_bytes_be();
        let (sign_byte, magnitude) = match sign {
            Sign::NoSign => (0u8, Vec::new()),
            Sign::Plus => (1u8, magnitude),
            Sign::Minus => (2u8, magnitude),
        };
        let (_, denominator) = self.denom().to_bytes_be();
        BorshSerialize::serialize(&sign_byte, writer)?;
        BorshSerialize::serialize(&magnitude, writer)?;
        BorshSerialize::serialize(&denominator, writer)
    }
}

impl BorshDeserialize for Q {
    fn deserialize_reader<R: io::Read>(reader: &mut R) -> io::Result<Self> {
        fn invalid(message: &str) -> io::Error {
            io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
        }
        let sign_byte = u8::deserialize_reader(reader)?;
        let magnitude = Vec::<u8>::deserialize_reader(reader)?;
        let denominator = Vec::<u8>::deserialize_reader(reader)?;
        if denominator.is_empty() || denominator[0] == 0 {
            return Err(invalid("non-canonical rational denominator"));
        }
        let sign = match sign_byte {
            0 => Sign::NoSign,
            1 => Sign::Plus,
            2 => Sign::Minus,
            _ => return Err(invalid("invalid rational sign")),
        };
        match sign {
            Sign::NoSign => {
                if !magnitude.is_empty() || denominator != [1u8] {
                    return Err(invalid("non-canonical rational zero"));
                }
                Ok(Q::zero())
            }
            Sign::Plus | Sign::Minus => {
                if magnitude.is_empty() || magnitude[0] == 0 {
                    return Err(invalid("non-canonical rational numerator"));
                }
                let numer = BigInt::from_bytes_be(sign, &magnitude);
                let denom = BigInt::from_bytes_be(Sign::Plus, &denominator);
                if !numer.gcd(&denom).is_one() {
                    return Err(invalid("rational not in lowest terms"));
                }
                Ok(Q(BigRational::new_raw(numer, denom)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_is_exact() {
        let third = Q::new(1, 3);
        let sum = &third + &third + &third;
        assert_eq!(sum, Q::one());
        assert_eq!(Q::new(2, 4), Q::new(1, 2));
        assert_eq!(Q::new(-3, 2).floor(), BigInt::from(-2));
        assert_eq!(Q::new(7, 2).floor_u128(), Some(3));
        assert!(Q::new(-1, 3).is_negative());
    }

    #[test]
    fn borsh_round_trip_and_canonical() {
        for value in [
            Q::zero(),
            Q::one(),
            Q::new(-7, 3),
            Q::new(1, u128::MAX),
            Q::from(u128::MAX) * Q::from(u128::MAX),
        ] {
            let bytes = borsh::to_vec(&value).unwrap();
            let back: Q = borsh::from_slice(&bytes).unwrap();
            assert_eq!(back, value);
        }
        // An unreduced fraction (2/4) must be rejected.
        let mut bytes = Vec::new();
        BorshSerialize::serialize(&1u8, &mut bytes).unwrap();
        BorshSerialize::serialize(&vec![2u8], &mut bytes).unwrap();
        BorshSerialize::serialize(&vec![4u8], &mut bytes).unwrap();
        assert!(borsh::from_slice::<Q>(&bytes).is_err());
        // A leading zero byte must be rejected.
        let mut bytes = Vec::new();
        BorshSerialize::serialize(&1u8, &mut bytes).unwrap();
        BorshSerialize::serialize(&vec![0u8, 3u8], &mut bytes).unwrap();
        BorshSerialize::serialize(&vec![1u8], &mut bytes).unwrap();
        assert!(borsh::from_slice::<Q>(&bytes).is_err());
    }

    #[test]
    fn text_round_trip() {
        let value = Q::new(-22, 7);
        assert_eq!(value.to_string(), "-22/7");
        assert_eq!(Q::from_str("-22/7").unwrap(), value);
        assert_eq!(Q::from_str("4/2").unwrap(), Q::from(2u64));
        assert!(Q::from_str("1/0").is_err());
        let json = serde_json::to_string(&value).unwrap();
        let back: Q = serde_json::from_str(&json).unwrap();
        assert_eq!(back, value);
    }
}
