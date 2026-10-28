//! Message classes: what kind of traffic a message is, how large it may be
//! and how urgently it travels.
//!
//! | Class | Carries | Limit | Priority |
//! |---|---|---|---|
//! | Control | votes, sync requests, Pulse signatures, approvals, rejections | 64 KiB | 3 |
//! | Consensus | headers and certificates | the largest certificate the committee can form | 2 |
//! | Ingress | lane requests and payload submissions | a payload item or two transactions, plus 64 KiB | 1 |
//! | Bulk | execution reports, later Pulse sync and snapshots | 8 MiB | 0 |
//!
//! A receiver checks a frame's length against its class's limit before it
//! reads the body, so no peer can make it buffer more than the protocol
//! allows. Higher priorities go first when a link is busy, so a 2 MB
//! certificate never holds up a vote.

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

/// The encoded size of a dual post-quantum validator signature: ML-DSA-44
/// (2,420 bytes) and SLH-DSA-SHA2-128f (17,088 bytes), each with a one-byte
/// scheme tag and a four-byte length.
pub const DUAL_SIGNATURE_BYTES: usize = 19_518;

/// The largest signed transaction a gateway accepts (its Borsh encoding).
pub const MAX_TRANSACTION_BYTES: usize = 256 * 1024;

/// A message class.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum MessageClass {
    /// Small, latency-critical messages.
    Control,
    /// Headers and certificates.
    Consensus,
    /// What gateways hand validators.
    Ingress,
    /// Large transfers that can wait.
    Bulk,
}

impl MessageClass {
    /// Every class, in tag order.
    pub const ALL: [MessageClass; 4] = [
        MessageClass::Control,
        MessageClass::Consensus,
        MessageClass::Ingress,
        MessageClass::Bulk,
    ];

    /// The class's byte on the wire.
    pub fn tag(self) -> u8 {
        match self {
            MessageClass::Control => 0,
            MessageClass::Consensus => 1,
            MessageClass::Ingress => 2,
            MessageClass::Bulk => 3,
        }
    }

    /// The class with this wire byte.
    pub fn from_tag(tag: u8) -> Option<MessageClass> {
        MessageClass::ALL.get(usize::from(tag)).copied()
    }

    /// Send priority: higher goes first.
    pub fn priority(self) -> i32 {
        match self {
            MessageClass::Control => 3,
            MessageClass::Consensus => 2,
            MessageClass::Ingress => 1,
            MessageClass::Bulk => 0,
        }
    }

    /// Its name, as status reports show it.
    pub fn name(self) -> &'static str {
        match self {
            MessageClass::Control => "control",
            MessageClass::Consensus => "consensus",
            MessageClass::Ingress => "ingress",
            MessageClass::Bulk => "bulk",
        }
    }

    /// The index of the class in [`MessageClass::ALL`].
    pub fn index(self) -> usize {
        usize::from(self.tag())
    }
}

/// The largest body a frame of each class may carry, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassLimits {
    /// Control frames.
    pub control: usize,
    /// Consensus frames.
    pub consensus: usize,
    /// Ingress frames.
    pub ingress: usize,
    /// Bulk frames.
    pub bulk: usize,
}

impl ClassLimits {
    /// The limit of `class`.
    pub fn get(&self, class: MessageClass) -> usize {
        match class {
            MessageClass::Control => self.control,
            MessageClass::Consensus => self.consensus,
            MessageClass::Ingress => self.ingress,
            MessageClass::Bulk => self.bulk,
        }
    }

    /// The limits for a protocol whose consensus headers carry at most
    /// `max_payload_bytes` of payload in at most `max_items` items, and
    /// whose committees have at most `max_validators` members.
    ///
    /// The consensus limit fits the largest certificate: a header with every
    /// parent and the full payload, its author's signature and a vote from
    /// every member, plus a kilobyte for the envelope. Ingress fits a whole
    /// payload item, or a lane request (a signed transaction and its inputs
    /// again, each at most [`MAX_TRANSACTION_BYTES`]), plus 64 KiB.
    pub fn for_protocol(
        max_payload_bytes: usize,
        max_items: usize,
        max_validators: usize,
    ) -> ClassLimits {
        let header = 32 + 32 + 8 + 8 + 2 + (4 + 32 * max_validators) + (4 + 4 * max_items);
        let votes = 4 + max_validators * (32 + 4 + DUAL_SIGNATURE_BYTES);
        let consensus = 1024 + header + max_payload_bytes + (4 + DUAL_SIGNATURE_BYTES) + votes;
        ClassLimits {
            control: 64 * 1024,
            consensus,
            ingress: max_payload_bytes.max(2 * MAX_TRANSACTION_BYTES) + 64 * 1024,
            bulk: 8 * 1024 * 1024,
        }
    }
}

impl Default for ClassLimits {
    /// The devnet's protocol: 512 KiB and 2,000 items per header, at most
    /// 100 validators in a committee.
    fn default() -> Self {
        ClassLimits::for_protocol(512 * 1024, 2_000, 100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_round_trip_and_priorities_follow_urgency() {
        for class in MessageClass::ALL {
            assert_eq!(MessageClass::from_tag(class.tag()), Some(class));
            assert_eq!(MessageClass::ALL[class.index()], class);
        }
        assert_eq!(MessageClass::from_tag(4), None);
        let priorities: Vec<i32> = MessageClass::ALL.iter().map(|c| c.priority()).collect();
        assert_eq!(priorities, vec![3, 2, 1, 0]);
    }

    #[test]
    fn the_dual_signature_size_matches_its_encoding() {
        let keys =
            sion_crypto::DualKeypair::from_seed(&[1; 32], sion_crypto::Scheme::SlhDsaSha2_128f);
        let signature = keys.sign_deterministic("sion/test", b"m").unwrap();
        assert_eq!(
            sion_core::encode::encode(&signature).len(),
            DUAL_SIGNATURE_BYTES
        );
    }

    #[test]
    fn the_default_limits_fit_a_full_certificate_of_a_hundred_validators() {
        let limits = ClassLimits::default();
        // About 2.4 MiB: the payload, 2,000 item lengths, 100 parents and
        // 101 dual signatures.
        assert!(limits.consensus > 512 * 1024 + 101 * DUAL_SIGNATURE_BYTES);
        assert!(limits.consensus < 3 * 1024 * 1024);
        assert_eq!(limits.ingress, 576 * 1024);
        assert_eq!(limits.get(MessageClass::Control), 64 * 1024);
        assert_eq!(limits.get(MessageClass::Bulk), 8 * 1024 * 1024);
        // Larger payloads raise the limits that carry them.
        let big = ClassLimits::for_protocol(4 * 1024 * 1024, 2_000, 100);
        assert!(big.consensus > 4 * 1024 * 1024 && big.ingress > 4 * 1024 * 1024);
    }
}
