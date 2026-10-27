//! Fixed-point money and costs with an explicit infinity.

use std::cmp::Ordering;
use std::fmt;
use std::ops::{Add, AddAssign};

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// An amount in micro-units (six decimal places), for example micro-USD.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
#[serde(transparent)]
pub struct Amount(pub u128);

impl Amount {
    /// Zero.
    pub const ZERO: Amount = Amount(0);
    /// Micro-units per whole unit.
    pub const MICROS_PER_UNIT: u128 = 1_000_000;

    /// Whole units, for example `Amount::units(25_000)` is 25,000 USD.
    pub const fn units(units: u64) -> Amount {
        Amount(units as u128 * Self::MICROS_PER_UNIT)
    }

    /// Raw micro-units.
    pub const fn micros(self) -> u128 {
        self.0
    }

    /// Checked addition.
    pub fn checked_add(self, other: Amount) -> Result<Amount, CoreError> {
        self.0
            .checked_add(other.0)
            .map(Amount)
            .ok_or(CoreError::Overflow("amount addition"))
    }

    /// Checked subtraction.
    pub fn checked_sub(self, other: Amount) -> Result<Amount, CoreError> {
        self.0
            .checked_sub(other.0)
            .map(Amount)
            .ok_or(CoreError::Overflow("amount subtraction"))
    }

    /// Saturating subtraction.
    pub fn saturating_sub(self, other: Amount) -> Amount {
        Amount(self.0.saturating_sub(other.0))
    }

    /// Multiplies by a ratio `numerator / denominator`, rounding down.
    pub fn mul_ratio(self, numerator: u128, denominator: u128) -> Result<Amount, CoreError> {
        if denominator == 0 {
            return Err(CoreError::Overflow("division by zero"));
        }
        self.0
            .checked_mul(numerator)
            .map(|product| Amount(product / denominator))
            .ok_or(CoreError::Overflow("amount multiplication"))
    }

    /// Sums amounts with overflow checking.
    pub fn sum<'a>(amounts: impl IntoIterator<Item = &'a Amount>) -> Result<Amount, CoreError> {
        amounts
            .into_iter()
            .try_fold(Amount::ZERO, |total, next| total.checked_add(*next))
    }
}

impl fmt::Debug for Amount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Amount({self})")
    }
}

impl fmt::Display for Amount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let whole = self.0 / Self::MICROS_PER_UNIT;
        let frac = self.0 % Self::MICROS_PER_UNIT;
        let mut whole_text = whole.to_string();
        let mut grouped = String::new();
        while whole_text.len() > 3 {
            let tail = whole_text.split_off(whole_text.len() - 3);
            grouped = format!(",{tail}{grouped}");
        }
        grouped = format!("{whole_text}{grouped}");
        if frac == 0 {
            write!(f, "{grouped}")
        } else {
            let frac_text = format!("{frac:06}");
            write!(f, "{grouped}.{}", frac_text.trim_end_matches('0'))
        }
    }
}

/// A cost that may be infinite: the cost of breaking an assumption a
/// consumer's policy trusts is infinite.
#[derive(
    Clone, Copy, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub enum Cost {
    /// A finite cost.
    Finite(Amount),
    /// Unbounded: the assumption is trusted.
    Infinite,
}

impl Cost {
    /// Zero cost.
    pub const ZERO: Cost = Cost::Finite(Amount::ZERO);

    /// Whether the cost is finite.
    pub fn is_finite(&self) -> bool {
        matches!(self, Cost::Finite(_))
    }

    /// The finite amount, if any.
    pub fn finite(&self) -> Option<Amount> {
        match self {
            Cost::Finite(amount) => Some(*amount),
            Cost::Infinite => None,
        }
    }

    /// Saturating addition: overflow and infinity both yield infinity.
    pub fn saturating_add(self, other: Cost) -> Cost {
        match (self, other) {
            (Cost::Finite(a), Cost::Finite(b)) => match a.0.checked_add(b.0) {
                Some(sum) => Cost::Finite(Amount(sum)),
                None => Cost::Infinite,
            },
            _ => Cost::Infinite,
        }
    }

    /// The smaller of two costs.
    pub fn min(self, other: Cost) -> Cost {
        if self <= other {
            self
        } else {
            other
        }
    }

    /// The larger of two costs.
    pub fn max(self, other: Cost) -> Cost {
        if self >= other {
            self
        } else {
            other
        }
    }

    /// Scales a finite cost by `numerator / denominator` (rounding down);
    /// infinity stays infinite unless the factor is zero.
    pub fn scale(self, numerator: u128, denominator: u128) -> Cost {
        match self {
            Cost::Finite(amount) => amount
                .mul_ratio(numerator, denominator)
                .map(Cost::Finite)
                .unwrap_or(Cost::Infinite),
            Cost::Infinite if numerator == 0 => Cost::ZERO,
            Cost::Infinite => Cost::Infinite,
        }
    }
}

impl PartialOrd for Cost {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Cost {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Cost::Finite(a), Cost::Finite(b)) => a.cmp(b),
            (Cost::Finite(_), Cost::Infinite) => Ordering::Less,
            (Cost::Infinite, Cost::Finite(_)) => Ordering::Greater,
            (Cost::Infinite, Cost::Infinite) => Ordering::Equal,
        }
    }
}

impl Add for Cost {
    type Output = Cost;
    fn add(self, other: Cost) -> Cost {
        self.saturating_add(other)
    }
}

impl AddAssign for Cost {
    fn add_assign(&mut self, other: Cost) {
        *self = self.saturating_add(other);
    }
}

impl From<Amount> for Cost {
    fn from(amount: Amount) -> Self {
        Cost::Finite(amount)
    }
}

impl fmt::Debug for Cost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Cost({self})")
    }
}

impl fmt::Display for Cost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Cost::Finite(amount) => write!(f, "{amount}"),
            Cost::Infinite => f.write_str("∞"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_groups_thousands() {
        assert_eq!(Amount::units(60_000).to_string(), "60,000");
        assert_eq!(Amount::units(1_000_000).to_string(), "1,000,000");
        assert_eq!(Amount(1_500_000).to_string(), "1.5");
        assert_eq!(Amount(1).to_string(), "0.000001");
    }

    #[test]
    fn checked_arithmetic() {
        assert!(Amount(u128::MAX).checked_add(Amount(1)).is_err());
        assert!(Amount(0).checked_sub(Amount(1)).is_err());
        assert_eq!(
            Amount::units(60_000).mul_ratio(1, 3).unwrap(),
            Amount::units(20_000)
        );
    }

    #[test]
    fn cost_ordering_and_infinity() {
        let finite = Cost::Finite(Amount::units(5));
        assert!(finite < Cost::Infinite);
        assert_eq!(finite.min(Cost::Infinite), finite);
        assert_eq!(finite + Cost::Infinite, Cost::Infinite);
        assert_eq!(Cost::Finite(Amount(u128::MAX)) + finite, Cost::Infinite);
        assert_eq!(Cost::Infinite.scale(0, 1), Cost::ZERO);
        assert_eq!(Cost::Infinite.to_string(), "∞");
    }
}
