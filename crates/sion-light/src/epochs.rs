//! Following the committee from epoch to epoch.
//!
//! Epoch 0's committee is fixed by the chain identity a client pins. Every
//! later committee is in the state: the validator set after the last Pulse
//! of an epoch holds the next epoch's active set. So a client that trusts
//! epoch `e − 1` checks one certified header (the last of that epoch) and
//! one state proof (the validator set after it), and from then on trusts
//! epoch `e`. The chain of signers is valid from genesis, link by link, and
//! a client that keeps the links never needs them again.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_consensus::{Committee, SignatureVerifier};
use sion_core::hash::Hash32;
use sion_core::ids::ValidatorId;
use sion_exec::object::well_known;
use sion_exec::validators::ValidatorSet as StateValidators;
use sion_exec::ObjectData;

use crate::chain::ChainSpec;
use crate::error::LightError;
use crate::header::{verify_certificate, PulseCertificate, ValidatorSet};
use crate::proofs::{verify_object, ObjectProof};

/// What links epoch `epoch − 1` to epoch `epoch`.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct EpochProof {
    /// The epoch this proof establishes.
    pub epoch: u64,
    /// The certificate of the last Pulse of the previous epoch.
    pub boundary: PulseCertificate,
    /// The validator set after that Pulse, proven against its header.
    pub validators: ObjectProof,
}

enum Keys {
    Spec(ValidatorSet),
    State(Box<StateValidators>),
}

impl SignatureVerifier for Keys {
    fn verify(&self, validator: &ValidatorId, message: &[u8], signature: &[u8]) -> bool {
        match self {
            Keys::Spec(set) => set.verify(validator, message, signature),
            Keys::State(set) => set.verifier().verify(validator, message, signature),
        }
    }
}

/// The committees a client has verified, from epoch 0 up.
pub struct EpochChain {
    chain: Hash32,
    insecure: bool,
    epochs: BTreeMap<u64, (Committee, Keys)>,
}

impl std::fmt::Debug for EpochChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EpochChain({} epochs)", self.epochs.len())
    }
}

impl EpochChain {
    /// Starts from a chain specification the client has checked against its
    /// pinned identity: epoch 0's committee and keys.
    pub fn new(spec: &ChainSpec) -> Result<EpochChain, LightError> {
        let mut epochs = BTreeMap::new();
        epochs.insert(0, (spec.committee(0)?, Keys::Spec(spec.keys.clone())));
        Ok(EpochChain {
            chain: spec.id(),
            insecure: spec.insecure_signatures,
            epochs,
        })
    }

    /// The highest epoch verified so far.
    pub fn latest(&self) -> u64 {
        *self
            .epochs
            .keys()
            .next_back()
            .expect("epoch 0 is always known")
    }

    /// The committee that signs the headers of `epoch`, if verified.
    pub fn committee(&self, epoch: u64) -> Option<&Committee> {
        self.epochs.get(&epoch).map(|(c, _)| c)
    }

    /// Checks a certificate of a header in a verified epoch; returns the
    /// signing stake and the quorum, or `None` for a devnet that signs with
    /// the insecure simulation scheme (nothing to check).
    pub fn verify_header(
        &self,
        certificate: &PulseCertificate,
    ) -> Result<Option<(u64, u64)>, LightError> {
        let header = &certificate.header;
        if header.chain != self.chain {
            return Err(LightError::Certificate(
                "the header is for another chain".into(),
            ));
        }
        let (committee, keys) = self.epochs.get(&header.epoch).ok_or_else(|| {
            LightError::Certificate(format!("epoch {} is not verified yet", header.epoch))
        })?;
        if self.insecure {
            return Ok(None);
        }
        let stake = verify_certificate(certificate, committee, keys)?;
        Ok(Some((stake, committee.quorum_threshold())))
    }

    /// Verifies the link to the next epoch and adds it.
    pub fn extend(&mut self, proof: &EpochProof) -> Result<(), LightError> {
        let latest = self.latest();
        if proof.epoch != latest + 1 {
            return Err(LightError::Chain(format!(
                "expected the proof for epoch {}, got {}",
                latest + 1,
                proof.epoch
            )));
        }
        let header = &proof.boundary.header;
        if header.epoch != latest {
            return Err(LightError::Chain(
                "the boundary header is not from the previous epoch".into(),
            ));
        }
        self.verify_header(&proof.boundary)?;
        if proof.validators.id != well_known::validators() {
            return Err(LightError::Proof("not the validator set".into()));
        }
        verify_object(header, &proof.validators)?;
        let set = match proof.validators.object.as_ref().map(|o| &o.data) {
            Some(ObjectData::Validators(set)) => (**set).clone(),
            _ => return Err(LightError::Proof("the validator set is missing".into())),
        };
        if set.epoch != proof.epoch || set.epoch_start != header.pulse.next() {
            return Err(LightError::Chain(
                "the boundary header is not the last Pulse of its epoch".into(),
            ));
        }
        let committee = set
            .consensus_committee(self.chain)
            .map_err(|e| LightError::Chain(e.to_string()))?;
        self.epochs
            .insert(proof.epoch, (committee, Keys::State(Box::new(set))));
        Ok(())
    }
}
