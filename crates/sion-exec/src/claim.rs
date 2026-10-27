//! Claims: the network's atomic unit of knowledge.

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_assurance::atom::standard;
use sion_assurance::Formula;
use sion_core::encode::encode;
use sion_core::hash::{hash_domain, Hash32};
use sion_core::ids::{AccountId, AtomId, ClaimId, Pulse};
use sion_core::merkle;
use sion_crypto::SealedEnvelope;

/// A claim's payload: public, encrypted to a viewing key, or committed only.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum Payload {
    /// Public bytes.
    Public(Vec<u8>),
    /// Encrypted to an ML-KEM viewing key (private by default, provable on demand).
    Encrypted(SealedEnvelope),
    /// Only a commitment is published; the owner holds the data.
    Commitment(Hash32),
}

impl Payload {
    /// Size in bytes as published.
    pub fn size(&self) -> usize {
        encode(self).len()
    }
}

/// What an issuer publishes.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct ClaimBody {
    /// Schema identifier, for example `receipt.ai.inference/v1`.
    pub schema: String,
    /// One-line human-readable statement.
    pub subject: String,
    /// Payload.
    pub payload: Payload,
    /// The assumptions the statement rests on.
    pub formula: Formula,
    /// The adjudicator atom that settles disputes, if any.
    pub adjudicator: Option<AtomId>,
    /// Detection factor in thousandths (1000 = always provable when false).
    pub detection_permille: u16,
    /// Validity window length in Pulses.
    pub valid_for: u64,
    /// Challenge period after the window, in Pulses.
    pub challenge_period: u64,
}

/// A claim as stored on the ledger.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct StoredClaim {
    /// Identifier.
    pub id: ClaimId,
    /// Issuer.
    pub issuer: AccountId,
    /// Body.
    pub body: ClaimBody,
    /// Pulse of issue.
    pub issued_at: Pulse,
    /// Index in the knowledge log.
    pub log_index: u64,
}

/// The claim identifier for `body` issued by `issuer`.
pub fn claim_id(issuer: &AccountId, body: &ClaimBody) -> ClaimId {
    ClaimId::derive(&encode(&(issuer, body)))
}

/// The knowledge-log leaf for a stored claim.
pub fn knowledge_leaf(claim: &StoredClaim) -> Hash32 {
    let body_hash = hash_domain("sion/exec/claim-body", &encode(&claim.body));
    merkle::leaf_hash(&encode(&(
        claim.id,
        claim.issuer,
        body_hash,
        claim.issued_at,
    )))
}

/// Standard formulas from the architecture.
pub mod formulas {
    use super::*;

    /// A dual-origin signature: forging needs both families broken.
    pub fn dual_signature() -> Formula {
        Formula::or(vec![
            Formula::atom(standard::module_lattice().id),
            Formula::atom(standard::sha2().id),
        ])
    }

    /// A statement backed by an issuer's bond and authenticated by its
    /// dual signature.
    pub fn bonded_statement(bond_atom: AtomId) -> Formula {
        Formula::and(vec![dual_signature(), Formula::atom(bond_atom)])
    }

    /// A computation proven by two independent proof stacks (each with its
    /// own circuit atom), under the shared specification and hash function.
    pub fn dual_proof(circuit_a: AtomId, circuit_b: AtomId) -> Formula {
        Formula::and(vec![
            Formula::atom(standard::specification().id),
            Formula::atom(standard::sha3().id),
            Formula::or(vec![
                Formula::and(vec![
                    Formula::atom(standard::proof_stack("A").id),
                    Formula::atom(circuit_a),
                ]),
                Formula::and(vec![
                    Formula::atom(standard::proof_stack("B").id),
                    Formula::atom(circuit_b),
                ]),
            ]),
        ])
    }

    /// A witness quorum: any one honest bonded witness suffices.
    pub fn witness_quorum(witnesses: &[AtomId]) -> Formula {
        Formula::or(witnesses.iter().map(|w| Formula::atom(*w)))
    }

    /// An AI inference receipt: a TEE quote or the operator's bond.
    pub fn inference_receipt(tee: AtomId, operator_bond: AtomId) -> Formula {
        Formula::and(vec![
            dual_signature(),
            Formula::or(vec![Formula::atom(tee), Formula::atom(operator_bond)]),
        ])
    }
}
