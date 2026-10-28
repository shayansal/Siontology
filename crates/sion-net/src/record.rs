//! Network records: a validator's network key and addresses, signed with its
//! dual consensus keys.
//!
//! A record authenticates itself: whoever hosts a peer file cannot forge
//! one, because checking it needs both of the validator's signatures
//! (ML-DSA-44 and SLH-DSA-SHA2-128f) under the record domain, and the
//! validator's identifier is the hash of the two public keys the record
//! carries. The same signature is the validator's proof that it holds both
//! keys, which a genesis file needs from every validator it lists.
//!
//! A record with a higher `sequence` supersedes a lower one for the same
//! validator.

use std::net::{SocketAddr, ToSocketAddrs};

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::encode::encode;
use sion_core::hash::{hash_domain, Hash32};
use sion_core::ids::ValidatorId;
use sion_crypto::{validator_id, DualKeypair, DualSignature, PublicKey};

use crate::error::NetError;
use crate::id::NodeId;
use crate::key::NetworkPublicKey;

/// The signing domain (FIPS context string) of network records.
pub const RECORD_DOMAIN: &str = "sion/net/record";

/// The most addresses a record lists.
pub const MAX_ADDRESSES: usize = 8;

/// What a validator publishes about how to reach it.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct NetworkRecord {
    /// The chain's name (`ChainConfig::chain_id`), so a record for one
    /// network never counts on another.
    pub chain: String,
    /// The validator.
    pub validator: ValidatorId,
    /// Its network key.
    pub network_key: NetworkPublicKey,
    /// Where it accepts connections, as `host:port`.
    pub addresses: Vec<String>,
    /// The first epoch the record is valid in.
    pub valid_from_epoch: u64,
    /// Orders the records of one validator: the highest wins.
    pub sequence: u64,
}

impl NetworkRecord {
    /// The digest the validator signs.
    pub fn digest(&self) -> Hash32 {
        hash_domain(RECORD_DOMAIN, &encode(self))
    }
}

/// A record with the validator's public keys and dual signature.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct SignedRecord {
    /// The record.
    pub record: NetworkRecord,
    /// The validator's ML-DSA-44 consensus key.
    pub lattice: PublicKey,
    /// The validator's SLH-DSA consensus key.
    pub hash: PublicKey,
    /// Both keys' signatures over the record's digest.
    pub signature: DualSignature,
}

/// A node a transport connects to: who it is, the network key it must
/// present, and where to reach it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    /// The node.
    pub node: NodeId,
    /// The network key it authenticates with.
    pub key: NetworkPublicKey,
    /// Where it accepts connections (none: it only dials, as gateways do).
    pub addresses: Vec<SocketAddr>,
}

/// Resolves `host:port` addresses (names resolve through the system).
pub fn resolve(addresses: &[String]) -> Result<Vec<SocketAddr>, NetError> {
    let mut out = Vec::new();
    for address in addresses {
        let resolved = address
            .to_socket_addrs()
            .map_err(|e| NetError::Parse(format!("address {address:?}: {e}")))?;
        out.extend(resolved);
    }
    Ok(out)
}

impl SignedRecord {
    /// Signs `record` with the validator's dual keys, which must be the
    /// keys its identifier derives from.
    pub fn sign(record: NetworkRecord, keys: &DualKeypair) -> Result<SignedRecord, NetError> {
        let (lattice, hash) = keys.public_keys();
        if validator_id(lattice, hash) != record.validator {
            return Err(NetError::Record("the keys are not the validator's".into()));
        }
        let signature = keys
            .sign_deterministic(RECORD_DOMAIN, &record.digest().0)
            .map_err(|e| NetError::Record(e.to_string()))?;
        Ok(SignedRecord {
            record,
            lattice: lattice.clone(),
            hash: hash.clone(),
            signature,
        })
    }

    /// Checks the record: for chain `chain`, a well-formed network key, at
    /// most [`MAX_ADDRESSES`] addresses, the validator's identifier derived
    /// from the two keys, and both signatures.
    pub fn verify(&self, chain: &str) -> Result<(), NetError> {
        let record = &self.record;
        if record.chain != chain {
            return Err(NetError::Record(format!(
                "the record is for chain {:?}, not {chain:?}",
                record.chain
            )));
        }
        record.network_key.check()?;
        if record.addresses.len() > MAX_ADDRESSES
            || record
                .addresses
                .iter()
                .any(|a| a.is_empty() || a.len() > 255)
        {
            return Err(NetError::Record("malformed addresses".into()));
        }
        if validator_id(&self.lattice, &self.hash) != record.validator {
            return Err(NetError::Record(format!(
                "{} is not derived from the record's keys",
                record.validator.short()
            )));
        }
        sion_crypto::dual::verify_dual(
            &self.lattice,
            &self.hash,
            RECORD_DOMAIN,
            &record.digest().0,
            &self.signature,
        )
        .map_err(|e| NetError::Record(format!("signature: {e}")))
    }

    /// The validator as a peer, its addresses resolved.
    pub fn peer(&self) -> Result<Peer, NetError> {
        Ok(Peer {
            node: NodeId::Validator(self.record.validator),
            key: self.record.network_key.clone(),
            addresses: resolve(&self.record.addresses)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use sion_crypto::Scheme;

    use super::*;
    use crate::key::NetworkKey;

    fn signed() -> (SignedRecord, DualKeypair) {
        let keys = DualKeypair::from_seed(&[5; 32], Scheme::SlhDsaSha2_128f);
        let (lattice, hash) = keys.public_keys();
        let record = NetworkRecord {
            chain: "sion-test".into(),
            validator: validator_id(lattice, hash),
            network_key: NetworkKey::from_seed(&[6; 32]).public().clone(),
            addresses: vec!["127.0.0.1:9100".into()],
            valid_from_epoch: 0,
            sequence: 1,
        };
        (SignedRecord::sign(record, &keys).unwrap(), keys)
    }

    #[test]
    fn a_signed_record_verifies_and_names_its_peer() {
        let (record, _) = signed();
        record.verify("sion-test").unwrap();
        let peer = record.peer().unwrap();
        assert_eq!(peer.node, NodeId::Validator(record.record.validator));
        assert_eq!(peer.addresses, vec!["127.0.0.1:9100".parse().unwrap()]);
        let json = serde_json::to_string(&record).unwrap();
        assert_eq!(serde_json::from_str::<SignedRecord>(&json).unwrap(), record);
    }

    #[test]
    fn forged_or_misplaced_records_are_refused() {
        let (record, keys) = signed();
        assert!(record.verify("another-chain").is_err());

        let mut moved = record.clone();
        moved.record.addresses = vec!["10.0.0.1:9100".into()];
        assert!(
            moved.verify("sion-test").is_err(),
            "the signature covers the addresses"
        );

        let mut stolen = record.clone();
        stolen.record.network_key = NetworkKey::from_seed(&[7; 32]).public().clone();
        assert!(stolen.verify("sion-test").is_err());

        // Another validator's keys cannot sign for this one.
        let other = DualKeypair::from_seed(&[8; 32], Scheme::SlhDsaSha2_128f);
        assert!(SignedRecord::sign(record.record.clone(), &other).is_err());
        let mut swapped = record.clone();
        swapped.lattice = other.lattice.public().clone();
        assert!(swapped.verify("sion-test").is_err());

        let mut crowded = record.record.clone();
        crowded.addresses = vec!["127.0.0.1:1".into(); MAX_ADDRESSES + 1];
        let crowded = SignedRecord::sign(crowded, &keys).unwrap();
        assert!(crowded.verify("sion-test").is_err());
    }
}
