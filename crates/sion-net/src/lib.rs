//! Networking between Siontology nodes.
//!
//! The protocol (`sion-node`) sends typed messages through a `Transport`;
//! this crate holds what every carrier of those messages shares, down to the
//! bytes:
//!
//! - [`id`]: who a node is. A validator is named by its validator
//!   identifier, any other node (a gateway) by the digest of its network
//!   key. The receiver always takes the sender's identity from the
//!   authenticated connection, never from the message.
//! - [`class`]: the four message classes (control, consensus, ingress,
//!   bulk), each with a size limit and a priority.
//! - [`frame`]: one class byte, a four-byte length and a Borsh body; the
//!   length is checked against the class's limit before the body is read.

pub mod class;
pub mod error;
pub mod frame;
pub mod id;

pub use class::{ClassLimits, MessageClass};
pub use error::NetError;
pub use frame::{FrameError, FrameHeader};
pub use id::NodeId;
