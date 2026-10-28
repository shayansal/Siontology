//! Networking between Siontology nodes.
//!
//! The protocol (`sion-node`) sends typed messages through a `Transport`;
//! this crate holds what every carrier of those messages shares, down to the
//! bytes, and the carrier between machines:
//!
//! - [`id`]: who a node is. A validator is named by its validator
//!   identifier, any other node (a gateway) by the digest of its network
//!   key. The receiver always takes the sender's identity from the
//!   authenticated connection, never from the message.
//! - [`class`]: the four message classes (control, consensus, ingress,
//!   bulk), each with a size limit and a priority.
//! - [`frame`]: one class byte, a four-byte length and a Borsh body; the
//!   length is checked against the class's limit before the body is read.
//! - [`key`] and [`record`]: a node's ML-DSA-44 network key, and the record,
//!   signed with a validator's dual consensus keys, that binds the network
//!   key and addresses to the validator.
//! - [`tls`]: TLS 1.3 with the hybrid key exchange X25519MLKEM768 only and
//!   ML-DSA-44 raw public keys only, both ends pinned.
//! - [`endpoint`]: the QUIC endpoint (`quinn`) on a small tokio runtime of
//!   its own: connection management, keep-alive, a stream per class and
//!   direction with priorities, bounded queues and [`quota`]s.
//!
//! The crate forbids unsafe code; its dependencies (tokio, quinn, rustls,
//! aws-lc-rs) contain some, and aws-lc-rs is C (`net:aws-lc`).

pub mod class;
pub mod endpoint;
pub mod error;
pub mod frame;
pub mod id;
pub mod key;
pub mod quota;
pub mod record;
pub mod tls;

pub use class::{ClassLimits, MessageClass};
pub use endpoint::{
    probe, Deliver, Endpoint, EndpointConfig, EndpointStats, KeyExchange, LinkStats, SendError,
};
pub use error::NetError;
pub use frame::{FrameError, FrameHeader};
pub use id::NodeId;
pub use key::{NetworkKey, NetworkPublicKey};
pub use quota::Quotas;
pub use record::{NetworkRecord, Peer, SignedRecord};
