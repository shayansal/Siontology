//! Who is on the other end.

use std::fmt;
use std::str::FromStr;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::hash::Hash32;
use sion_core::ids::ValidatorId;

use crate::error::NetError;

/// A node: a validator, named by its validator identifier (the hash of its
/// dual consensus keys), or another node, such as a gateway, named by the
/// digest of its network key.
///
/// A transport stamps every message it delivers with the identity the
/// sender's connection authenticated, so a node can only ever speak as
/// itself, whatever a message claims.
#[derive(
    Clone,
    Copy,
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
pub enum NodeId {
    /// A validator.
    Validator(ValidatorId),
    /// Any other node.
    Node(Hash32),
}

impl NodeId {
    /// The validator, if this node is one.
    pub fn validator(&self) -> Option<ValidatorId> {
        match self {
            NodeId::Validator(validator) => Some(*validator),
            NodeId::Node(_) => None,
        }
    }

    /// Whether this node is a validator.
    pub fn is_validator(&self) -> bool {
        matches!(self, NodeId::Validator(_))
    }

    /// The digest that names the node.
    pub fn digest(&self) -> Hash32 {
        match self {
            NodeId::Validator(validator) => validator.0,
            NodeId::Node(digest) => *digest,
        }
    }

    /// A short form for logs: `validator:1a2b3c4d` or `node:1a2b3c4d`.
    pub fn short(&self) -> String {
        format!("{}:{}", self.kind(), self.digest().short())
    }

    fn kind(&self) -> &'static str {
        match self {
            NodeId::Validator(_) => "validator",
            NodeId::Node(_) => "node",
        }
    }
}

impl fmt::Debug for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.short())
    }
}

/// `validator:<64 hex digits>` or `node:<64 hex digits>`.
impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind(), self.digest().to_hex())
    }
}

impl FromStr for NodeId {
    type Err = NetError;

    /// Parses the [`Display`](fmt::Display) form.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let bad = || NetError::Parse(format!("{text:?} is not validator:<hex> or node:<hex>"));
        let (kind, hex) = text.split_once(':').ok_or_else(bad)?;
        let digest = Hash32::from_hex(hex).map_err(|_| bad())?;
        match kind {
            "validator" => Ok(NodeId::Validator(ValidatorId::from_hash(digest))),
            "node" => Ok(NodeId::Node(digest)),
            _ => Err(bad()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_ids_print_and_parse_and_order_validators_first() {
        let validator = NodeId::Validator(ValidatorId::derive(b"v"));
        let node = NodeId::Node(sion_core::hash::hash_domain("test/node", b"n"));
        for id in [validator, node] {
            let text = id.to_string();
            assert_eq!(text.parse::<NodeId>().unwrap(), id);
            assert!(text.starts_with(&id.short()[..id.short().len() - 8]));
        }
        assert!(validator < node);
        assert_eq!(validator.validator(), Some(ValidatorId::derive(b"v")));
        assert_eq!(node.validator(), None);
        assert!("gateway:00".parse::<NodeId>().is_err());
        assert!("node:zz".parse::<NodeId>().is_err());
        let encoded = sion_core::encode::encode(&node);
        assert_eq!(sion_core::encode::decode::<NodeId>(&encoded).unwrap(), node);
    }
}
