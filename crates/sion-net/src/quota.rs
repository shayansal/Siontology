//! Inbound quotas: a token bucket of bytes per peer and message class.
//!
//! What a peer sends beyond its bucket is dropped and counted; a peer that
//! keeps exceeding it is disconnected. The defaults are far above what an
//! honest validator sends (the protocol's own bounds: one header per author
//! and round, a vote per header, one Pulse signature per Pulse) and serve to
//! bound memory, not to shape traffic.

use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::class::MessageClass;

/// How much each class may carry from one peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quotas {
    /// Sustained bytes per second, per class (in [`MessageClass::ALL`]
    /// order).
    pub rate: [u64; 4],
    /// Burst: bytes a peer may send at once, per class.
    pub burst: [u64; 4],
    /// Frames dropped over quota within a minute before the peer is
    /// disconnected.
    pub disconnect_after: u64,
}

impl Default for Quotas {
    fn default() -> Self {
        const MIB: u64 = 1024 * 1024;
        Quotas {
            rate: [32 * MIB, 256 * MIB, 128 * MIB, 256 * MIB],
            burst: [64 * MIB, 512 * MIB, 256 * MIB, 512 * MIB],
            disconnect_after: 1_000,
        }
    }
}

/// One peer's buckets.
#[derive(Clone, Debug)]
pub struct Buckets {
    quotas: Quotas,
    tokens: [u64; 4],
    refilled: Instant,
    dropped: u64,
    window: Instant,
}

/// What the buckets say about a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    /// Deliver it.
    Admit,
    /// Drop it.
    Drop,
    /// Drop it and disconnect the peer.
    Disconnect,
}

impl Buckets {
    /// Full buckets.
    pub fn new(quotas: Quotas, now: Instant) -> Buckets {
        Buckets {
            quotas,
            tokens: quotas.burst,
            refilled: now,
            dropped: 0,
            window: now,
        }
    }

    /// Takes `bytes` of `class` from the bucket at time `now`.
    pub fn admit(&mut self, class: MessageClass, bytes: usize, now: Instant) -> Admission {
        let elapsed = now.saturating_duration_since(self.refilled).as_millis() as u64;
        if elapsed > 0 {
            for i in 0..4 {
                let refill = self.quotas.rate[i].saturating_mul(elapsed) / 1_000;
                self.tokens[i] = self.tokens[i]
                    .saturating_add(refill)
                    .min(self.quotas.burst[i]);
            }
            self.refilled = now;
        }
        if now.saturating_duration_since(self.window).as_secs() >= 60 {
            self.window = now;
            self.dropped = 0;
        }
        let i = class.index();
        let bytes = bytes as u64;
        if self.tokens[i] >= bytes {
            self.tokens[i] -= bytes;
            return Admission::Admit;
        }
        self.dropped += 1;
        if self.dropped > self.quotas.disconnect_after {
            self.dropped = 0;
            Admission::Disconnect
        } else {
            Admission::Drop
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn buckets_admit_a_burst_refill_over_time_and_disconnect_a_flood() {
        let quotas = Quotas {
            rate: [1_000; 4],
            burst: [2_000; 4],
            disconnect_after: 3,
        };
        let start = Instant::now();
        let mut buckets = Buckets::new(quotas, start);
        let control = MessageClass::Control;
        assert_eq!(buckets.admit(control, 1_500, start), Admission::Admit);
        assert_eq!(buckets.admit(control, 1_000, start), Admission::Drop);
        // Other classes have their own buckets.
        assert_eq!(
            buckets.admit(MessageClass::Bulk, 2_000, start),
            Admission::Admit
        );
        // Half a second refills 500 bytes.
        let later = start + Duration::from_millis(500);
        assert_eq!(buckets.admit(control, 1_000, later), Admission::Admit);
        assert_eq!(buckets.admit(control, 1, later), Admission::Drop);
        assert_eq!(buckets.admit(control, 1, later), Admission::Drop);
        assert_eq!(buckets.admit(control, 1, later), Admission::Disconnect);
        // A refill never exceeds the burst.
        let much_later = later + Duration::from_secs(100);
        assert_eq!(buckets.admit(control, 2_000, much_later), Admission::Admit);
        assert_eq!(buckets.admit(control, 1, much_later), Admission::Drop);
    }
}
