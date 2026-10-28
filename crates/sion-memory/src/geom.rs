//! Integer space and time.
//!
//! Space is in millimetres (`i64`) and time in nanoseconds since the Unix
//! epoch (`u64`). Nothing here uses floating point: boxes, intervals and
//! every comparison the index makes are exact.

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

/// A closed axis-aligned box in a frame's coordinates, in millimetres. Both
/// corners are inclusive, so a point is a box with `min == max`.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub struct Box3 {
    /// The lower corner (x, y, z), inclusive.
    pub min: [i64; 3],
    /// The upper corner (x, y, z), inclusive.
    pub max: [i64; 3],
}

impl Box3 {
    /// The degenerate box at the origin, used for atoms without space.
    pub const NULL: Box3 = Box3 {
        min: [0; 3],
        max: [0; 3],
    };

    /// The box covering every representable coordinate.
    pub const ALL: Box3 = Box3 {
        min: [i64::MIN; 3],
        max: [i64::MAX; 3],
    };

    /// A box from its corners (not checked; see [`Box3::is_valid`]).
    pub const fn new(min: [i64; 3], max: [i64; 3]) -> Box3 {
        Box3 { min, max }
    }

    /// A point.
    pub const fn point(point: [i64; 3]) -> Box3 {
        Box3 {
            min: point,
            max: point,
        }
    }

    /// The box `centre ± half` on each axis, saturating at the coordinate limits.
    pub fn around(centre: [i64; 3], half: [i64; 3]) -> Box3 {
        let mut min = [0i64; 3];
        let mut max = [0i64; 3];
        for axis in 0..3 {
            let half = half[axis].max(0);
            min[axis] = centre[axis].saturating_sub(half);
            max[axis] = centre[axis].saturating_add(half);
        }
        Box3 { min, max }
    }

    /// Whether `min <= max` on every axis.
    pub fn is_valid(&self) -> bool {
        (0..3).all(|axis| self.min[axis] <= self.max[axis])
    }

    /// Whether two closed boxes share at least one point.
    pub fn intersects(&self, other: &Box3) -> bool {
        (0..3).all(|axis| self.min[axis] <= other.max[axis] && other.min[axis] <= self.max[axis])
    }

    /// Whether `other` lies wholly inside this box.
    pub fn contains(&self, other: &Box3) -> bool {
        (0..3).all(|axis| self.min[axis] <= other.min[axis] && other.max[axis] <= self.max[axis])
    }

    /// The smallest box covering both.
    pub fn union(&self, other: &Box3) -> Box3 {
        let mut out = *self;
        for axis in 0..3 {
            out.min[axis] = out.min[axis].min(other.min[axis]);
            out.max[axis] = out.max[axis].max(other.max[axis]);
        }
        out
    }

    /// Twice the centre on `axis` (`min + max`), exact in `i128`; the index
    /// sorts on it, so no division or rounding is involved.
    pub(crate) fn centre2(&self, axis: usize) -> i128 {
        i128::from(self.min[axis]) + i128::from(self.max[axis])
    }
}

/// When an atom held: the half-open interval `[start, end)` in nanoseconds
/// since the Unix epoch, open-ended when `end` is `None`.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Default,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub struct Interval {
    /// First instant at which the atom held.
    pub start: u64,
    /// First instant at which it no longer held, or `None` if it still holds.
    pub end: Option<u64>,
}

impl Interval {
    /// The closed-ended interval `[start, end)` (not checked; see [`Interval::is_valid`]).
    pub const fn closed(start: u64, end: u64) -> Interval {
        Interval {
            start,
            end: Some(end),
        }
    }

    /// The open-ended interval `[start, ∞)`.
    pub const fn starting(start: u64) -> Interval {
        Interval { start, end: None }
    }

    /// Whether the interval is non-empty (`end > start` when it has an end).
    pub fn is_valid(&self) -> bool {
        match self.end {
            Some(end) => end > self.start,
            None => true,
        }
    }

    /// Whether `instant` lies in the interval.
    pub fn contains(&self, instant: u64) -> bool {
        instant >= self.start
            && match self.end {
                Some(end) => instant < end,
                None => true,
            }
    }

    /// The interval as an index time range; an open end becomes `u64::MAX`.
    pub fn range(&self) -> TimeRange {
        TimeRange {
            start: self.start,
            end: self.end.unwrap_or(u64::MAX),
        }
    }
}

/// A half-open time range `[start, end)` in nanoseconds, as the index stores
/// it: an open-ended interval ends at `u64::MAX`.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub struct TimeRange {
    /// First instant covered.
    pub start: u64,
    /// First instant not covered.
    pub end: u64,
}

impl TimeRange {
    /// Every instant.
    pub const ALL: TimeRange = TimeRange {
        start: 0,
        end: u64::MAX,
    };

    /// The range `[start, end)` (not checked; see [`TimeRange::is_valid`]).
    pub const fn new(start: u64, end: u64) -> TimeRange {
        TimeRange { start, end }
    }

    /// Whether the range is non-empty.
    pub fn is_valid(&self) -> bool {
        self.start < self.end
    }

    /// Whether two half-open ranges share an instant.
    pub fn intersects(&self, other: &TimeRange) -> bool {
        self.start < other.end && other.start < self.end
    }

    /// Whether `instant` lies in the range.
    pub fn contains(&self, instant: u64) -> bool {
        self.start <= instant && instant < self.end
    }

    /// The smallest range covering both.
    pub fn union(&self, other: &TimeRange) -> TimeRange {
        TimeRange {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn boxes_intersect_on_shared_faces_and_points() {
        let a = Box3::new([0, 0, 0], [10, 10, 10]);
        let b = Box3::new([10, 10, 10], [20, 20, 20]);
        let c = Box3::new([11, 0, 0], [20, 10, 10]);
        assert!(a.intersects(&b), "closed boxes touching at a corner meet");
        assert!(!a.intersects(&c));
        assert!(Box3::point([5, 5, 5]).intersects(&a));
        assert!(a.contains(&Box3::point([5, 5, 5])));
        assert!(!a.contains(&b));
        assert_eq!(a.union(&c), Box3::new([0, 0, 0], [20, 10, 10]));
        assert!(a.is_valid());
        assert!(!Box3::new([1, 0, 0], [0, 0, 0]).is_valid());
        assert!(Box3::ALL.contains(&a));
        assert_eq!(Box3::NULL, Box3::point([0, 0, 0]));
    }

    #[test]
    fn around_saturates_and_centres_are_exact() {
        let b = Box3::around([i64::MAX - 1, 0, i64::MIN + 1], [5, 2, 5]);
        assert_eq!(b.max[0], i64::MAX);
        assert_eq!(b.min[2], i64::MIN);
        assert_eq!(b.min[1], -2);
        assert_eq!(Box3::ALL.centre2(0), -1);
        assert_eq!(Box3::new([3, 0, 0], [4, 0, 0]).centre2(0), 7);
        assert_eq!(Box3::around([0, 0, 0], [-3, 0, 0]), Box3::NULL);
    }

    #[test]
    fn intervals_are_half_open() {
        let i = Interval::closed(10, 20);
        assert!(i.is_valid());
        assert!(i.contains(10) && i.contains(19) && !i.contains(20));
        assert!(!Interval::closed(5, 5).is_valid());
        let open = Interval::starting(7);
        assert!(open.is_valid() && open.contains(u64::MAX));
        assert_eq!(open.range(), TimeRange::new(7, u64::MAX));
        assert_eq!(i.range(), TimeRange::new(10, 20));
        assert_eq!(Interval::default(), Interval::starting(0));
    }

    #[test]
    fn time_ranges_intersect_half_open() {
        let a = TimeRange::new(0, 10);
        assert!(a.intersects(&TimeRange::new(9, 11)));
        assert!(!a.intersects(&TimeRange::new(10, 11)));
        assert!(a.contains(0) && !a.contains(10));
        assert_eq!(a.union(&TimeRange::new(20, 30)), TimeRange::new(0, 30));
        assert!(!TimeRange::new(3, 3).is_valid());
        assert!(TimeRange::ALL.intersects(&a));
    }

    proptest! {
        #![proptest_config(crate::testing::config(256, 0x5eed_6e03))]

        #[test]
        fn union_covers_both(a in any::<[i64; 6]>(), b in any::<[i64; 6]>()) {
            let make = |v: [i64; 6]| {
                let mut min = [0; 3];
                let mut max = [0; 3];
                for axis in 0..3 {
                    min[axis] = v[axis].min(v[axis + 3]);
                    max[axis] = v[axis].max(v[axis + 3]);
                }
                Box3::new(min, max)
            };
            let (a, b) = (make(a), make(b));
            let u = a.union(&b);
            prop_assert!(u.contains(&a) && u.contains(&b));
            prop_assert_eq!(a.intersects(&b), b.intersects(&a));
        }
    }
}
