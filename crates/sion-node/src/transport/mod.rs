//! The seam between the protocol and whatever carries its messages.
//!
//! Validators and gateways never touch a channel or a socket: they hand a
//! [`PeerMessage`] for a [`NodeId`] to a [`Transport`], and the transport
//! delivers what arrives for them into their inbox, stamped with the node
//! it came from. The sender's identity comes from the transport (the
//! endpoint a message was sent through, or the authenticated connection it
//! arrived on), never from the message, so a node can only ever speak as
//! itself.
//!
//! Two carriers implement the trait in process:
//!
//! - [`memory::MemoryTransport`]: the in-process devnet's channels, exactly
//!   as before; it can also encode and decode every message, which checks
//!   the class size limits.
//! - [`faulty::FaultyTransport`]: wraps any transport and injects seeded
//!   loss, delay, duplication, reordering and partitions per message class.
//!
//! Every transport counts what it carries per message class, per kind and
//! per peer ([`TrafficStats`]).

pub mod faulty;
pub mod memory;
mod stats;

use std::fmt;
use std::sync::Arc;

use borsh::{BorshDeserialize, BorshSerialize};
use sion_consensus::{ApprovalProof, LaneRequest, Message};
use sion_core::hash::Hash32;
use sion_core::ids::{Pulse, ShardId, TxDigest};
pub use sion_net::{ClassLimits, MessageClass, NodeId};

pub use stats::{Counter, Traffic, TrafficCounter, TrafficStats};

/// The longest refusal reason a validator sends; longer ones are cut.
pub const MAX_REASON_BYTES: usize = 1024;

/// The most transaction digests in one [`PeerMessage::Executed`]; a Pulse
/// that executed more is reported in several messages.
pub const EXECUTED_CHUNK: usize = 16_384;

/// What one node sends another. Borsh on the wire; each variant belongs to a
/// [`MessageClass`] with its own size limit and priority.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum PeerMessage {
    /// A consensus message, in the envelope of the consensus session it
    /// belongs to. The receiver routes it by that session: a vote or a sync
    /// request names none of its own.
    Consensus {
        /// The session.
        session: u64,
        /// The message.
        message: Message,
    },
    /// A payload item for a validator to propose: a group of certified
    /// transactions from a gateway, or a forced intent.
    Payload {
        /// The item's canonical bytes.
        item: Vec<u8>,
    },
    /// A request to lock owned inputs, to a member of the shard's lane
    /// committee. The member answers the node that asked.
    Lane {
        /// The shard.
        shard: ShardId,
        /// The request.
        request: LaneRequest,
    },
    /// A sealed lane approval, to the node that asked for it.
    Approval {
        /// The shard.
        shard: ShardId,
        /// The transaction.
        tx: TxDigest,
        /// The approval.
        proof: ApprovalProof,
    },
    /// A lane member's refusal, to the node that asked. The refusing member
    /// is the sender, as the transport stamped it.
    LaneRejected {
        /// The transaction.
        tx: TxDigest,
        /// Why (at most [`MAX_REASON_BYTES`]).
        reason: String,
    },
    /// The sender's signature on the header of a Pulse it executed.
    PulseSignature {
        /// The Pulse.
        pulse: Pulse,
        /// The digest of the header it signed.
        header: Hash32,
        /// The signature over the header's signing digest.
        signature: Vec<u8>,
    },
    /// "Send me your signature for this Pulse": what a validator still
    /// waiting for a Pulse's certificate asks the members it lacks.
    PulseSignatureRequest {
        /// The Pulse.
        pulse: Pulse,
    },
    /// A lane member's approval of a forced intent, to every validator, so
    /// a certificate forms without the gateway.
    ForcedApproval {
        /// The shard.
        shard: ShardId,
        /// The intent.
        tx: TxDigest,
        /// The approval.
        proof: ApprovalProof,
    },
    /// The transactions a Pulse executed (some of them, at most
    /// [`EXECUTED_CHUNK`]), to the gateways a validator reports to.
    Executed {
        /// The Pulse.
        pulse: Pulse,
        /// The executed transactions.
        digests: Vec<TxDigest>,
    },
}

impl PeerMessage {
    /// The class the message travels in.
    pub fn class(&self) -> MessageClass {
        match self {
            PeerMessage::Consensus {
                message: Message::Header(_) | Message::Certificate(_),
                ..
            } => MessageClass::Consensus,
            PeerMessage::Consensus {
                message: Message::Vote(_) | Message::SyncRequest(_),
                ..
            } => MessageClass::Control,
            PeerMessage::Payload { .. } | PeerMessage::Lane { .. } => MessageClass::Ingress,
            PeerMessage::Approval { .. }
            | PeerMessage::LaneRejected { .. }
            | PeerMessage::PulseSignature { .. }
            | PeerMessage::PulseSignatureRequest { .. }
            | PeerMessage::ForcedApproval { .. } => MessageClass::Control,
            PeerMessage::Executed { .. } => MessageClass::Bulk,
        }
    }

    /// The message's kind, as traffic counters name it.
    pub fn kind(&self) -> &'static str {
        match self {
            PeerMessage::Consensus { message, .. } => match message {
                Message::Header(_) => "header",
                Message::Vote(_) => "vote",
                Message::Certificate(_) => "certificate",
                Message::SyncRequest(_) => "sync_request",
            },
            PeerMessage::Payload { .. } => "payload",
            PeerMessage::Lane { .. } => "lane_request",
            PeerMessage::Approval { .. } => "approval",
            PeerMessage::LaneRejected { .. } => "lane_rejected",
            PeerMessage::PulseSignature { .. } => "pulse_signature",
            PeerMessage::PulseSignatureRequest { .. } => "pulse_signature_request",
            PeerMessage::ForcedApproval { .. } => "forced_approval",
            PeerMessage::Executed { .. } => "executed",
        }
    }

    /// Whether a gateway handles the message (everything else is for
    /// validators). A node that hosts both routes by this.
    pub fn for_gateway(&self) -> bool {
        matches!(
            self,
            PeerMessage::Approval { .. }
                | PeerMessage::LaneRejected { .. }
                | PeerMessage::Executed { .. }
        )
    }

    /// The canonical encoding's length, without encoding.
    pub fn encoded_len(&self) -> usize {
        borsh::object_length(self).expect("values in memory encode")
    }

    /// A refusal, with the reason cut to [`MAX_REASON_BYTES`] on a
    /// character boundary.
    pub fn rejected(tx: TxDigest, reason: impl Into<String>) -> PeerMessage {
        let mut reason = reason.into();
        if reason.len() > MAX_REASON_BYTES {
            let mut cut = MAX_REASON_BYTES;
            while !reason.is_char_boundary(cut) {
                cut -= 1;
            }
            reason.truncate(cut);
        }
        PeerMessage::LaneRejected { tx, reason }
    }
}

/// Where a transport delivers what arrives for a node: the sender (as the
/// transport authenticated it) and the message. It must not block: nodes
/// push into their inbox.
pub type Delivery = Arc<dyn Fn(NodeId, PeerMessage) + Send + Sync>;

/// Carries peer messages. On the receiving side the transport delivers into
/// the node's existing inbox, with the sender taken from the transport.
pub trait Transport: Send + Sync + fmt::Debug {
    /// This node.
    fn local(&self) -> NodeId;

    /// Queues a message for one node. Never blocks the protocol thread; a
    /// message for a node the transport cannot reach is dropped (and
    /// counted).
    fn send(&self, to: NodeId, message: PeerMessage);

    /// Queues a message for several nodes.
    fn send_all(&self, to: &[NodeId], message: &PeerMessage) {
        for node in to {
            self.send(*node, message.clone());
        }
    }

    /// Whether the transport can reach `node` at all.
    fn knows(&self, node: &NodeId) -> bool;

    /// What the transport has carried so far for this node.
    fn stats(&self) -> TrafficStats;
}

#[cfg(test)]
mod tests;
