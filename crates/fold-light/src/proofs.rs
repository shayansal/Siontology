//! State, knowledge and receipt proofs against a certified header.

use borsh::{BorshDeserialize, BorshSerialize};
use fold_core::ids::ObjectId;
use fold_core::merkle;
use fold_core::smt::{verify_proof, SmtProof};
use fold_exec::claim::{knowledge_leaf, StoredClaim};
use fold_exec::pulse::effects_leaf;
use fold_exec::{Object, TxEffects};
use serde::{Deserialize, Serialize};

use crate::error::LightError;
use crate::header::PulseHeader;

/// An object (or its absence) with a sparse-Merkle proof.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct ObjectProof {
    /// The object identifier.
    pub id: ObjectId,
    /// The object, or `None` if it does not exist.
    pub object: Option<Object>,
    /// Proof against the header's post-state root.
    pub proof: SmtProof,
}

/// Checks an object proof against the state after the header's Pulse.
pub fn verify_object(header: &PulseHeader, proof: &ObjectProof) -> Result<(), LightError> {
    if let Some(object) = &proof.object {
        if object.id != proof.id {
            return Err(LightError::Proof(
                "object does not have the proven identifier".into(),
            ));
        }
    }
    let value = proof.object.as_ref().map(Object::digest);
    verify_proof(
        &header.after.state_root,
        &proof.id.0,
        value.as_ref(),
        &proof.proof,
    )
    .map_err(|e| LightError::Proof(format!("object {}: {e}", proof.id.short())))
}

/// A claim with its position in the knowledge log.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct ClaimProof {
    /// The claim.
    pub claim: StoredClaim,
    /// Inclusion path in the knowledge log at the header's size.
    pub path: Vec<fold_core::hash::Hash32>,
}

/// Checks that a claim is in the knowledge log committed by the header.
pub fn verify_claim(header: &PulseHeader, proof: &ClaimProof) -> Result<(), LightError> {
    let leaf = knowledge_leaf(&proof.claim);
    merkle::verify_inclusion(
        &header.after.knowledge_root,
        header.after.knowledge_size,
        proof.claim.log_index,
        &leaf,
        &proof.path,
    )
    .map_err(|e| LightError::Proof(format!("claim {}: {e}", proof.claim.id.short())))
}

/// A transaction's effects with its position among the Pulse's receipts.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct ReceiptProof {
    /// The effects.
    pub effects: TxEffects,
    /// Position in the Pulse's execution order.
    pub index: u64,
    /// Number of transactions the Pulse executed.
    pub count: u64,
    /// Inclusion path to the receipts root.
    pub path: Vec<fold_core::hash::Hash32>,
}

/// Checks that a transaction executed in the header's Pulse with these effects.
pub fn verify_receipt(header: &PulseHeader, proof: &ReceiptProof) -> Result<(), LightError> {
    merkle::verify_inclusion(
        &header.receipts_root,
        proof.count,
        proof.index,
        &effects_leaf(&proof.effects),
        &proof.path,
    )
    .map_err(|e| LightError::Proof(format!("receipt {}: {e}", proof.effects.digest)))
}
