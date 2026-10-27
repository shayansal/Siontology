//! DAG vertices: headers, votes, certificates and equivocation evidence.
//!
//! A header is a validator's proposal for one round: some payload and
//! references to certificates of the previous round. A certificate is a
//! header plus votes from more than two thirds of the stake. Honest
//! validators vote for at most one header per (author, round), so by quorum
//! intersection at most one certificate can exist per (author, round) while
//! Byzantine stake is within tolerance. This is what lets every honest
//! validator converge on the same DAG.

use std::collections::BTreeSet;

use borsh::{BorshDeserialize, BorshSerialize};
use fold_core::{encode, BlockDigest, Epoch, Hash32, Round, ShardId, ValidatorId};
use serde::{Deserialize, Serialize};

use crate::auth::{signing_digest, SignatureVerifier};
use crate::committee::Committee;
use crate::error::ConsensusError;

/// A validator's proposal for one round.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Header {
    /// The proposer.
    pub author: ValidatorId,
    /// The round.
    pub round: Round,
    /// The committee epoch.
    pub epoch: Epoch,
    /// The shard.
    pub shard: ShardId,
    /// Digests of certificates from the previous round, sorted.
    pub parents: Vec<BlockDigest>,
    /// Opaque payload items (transactions, certificate digests, intents).
    pub payload: Vec<Vec<u8>>,
}

impl Header {
    /// The header's digest, which also identifies its certificate.
    pub fn digest(&self) -> BlockDigest {
        BlockDigest::derive(&encode(self))
    }

    /// The digest the author signs.
    pub fn signing_digest(digest: &BlockDigest) -> Hash32 {
        signing_digest("fold/consensus/header", &[&digest.0 .0])
    }
}

/// A header with its author's signature.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct SignedHeader {
    /// The header.
    pub header: Header,
    /// The author's signature over [`Header::signing_digest`].
    pub signature: Vec<u8>,
}

impl SignedHeader {
    /// Checks the author's signature.
    pub fn verify_signature<V: SignatureVerifier>(&self, verifier: &V) -> bool {
        let digest = self.header.digest();
        verifier.verify(
            &self.header.author,
            &Header::signing_digest(&digest).0,
            &self.signature,
        )
    }
}

/// A vote for a header.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Vote {
    /// The header voted for.
    pub header: BlockDigest,
    /// The header's author.
    pub author: ValidatorId,
    /// The header's round.
    pub round: Round,
    /// The voter.
    pub voter: ValidatorId,
    /// The voter's signature over [`Vote::signing_digest`].
    pub signature: Vec<u8>,
}

impl Vote {
    /// The digest a voter signs.
    pub fn signing_digest(header: &BlockDigest) -> Hash32 {
        signing_digest("fold/consensus/vote", &[&header.0 .0])
    }
}

/// A header certified by votes from more than two thirds of the stake.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Certificate {
    /// The certified header.
    pub header: Header,
    /// The author's header signature (kept as equivocation evidence).
    pub header_signature: Vec<u8>,
    /// Votes: (voter, signature), sorted by voter.
    pub votes: Vec<(ValidatorId, Vec<u8>)>,
}

impl Certificate {
    /// The certificate's digest (its header's digest).
    pub fn digest(&self) -> BlockDigest {
        self.header.digest()
    }

    /// The round.
    pub fn round(&self) -> Round {
        self.header.round
    }

    /// The author.
    pub fn author(&self) -> ValidatorId {
        self.header.author
    }

    /// The genesis certificates for a committee: one empty round-0 vertex
    /// per member, certified by definition.
    pub fn genesis(committee: &Committee) -> Vec<Certificate> {
        committee
            .members()
            .iter()
            .map(|(author, _)| Certificate {
                header: Header {
                    author: *author,
                    round: Round(0),
                    epoch: committee.epoch(),
                    shard: committee.shard(),
                    parents: Vec::new(),
                    payload: Vec::new(),
                },
                header_signature: Vec::new(),
                votes: Vec::new(),
            })
            .collect()
    }

    /// Whether this is a canonical genesis certificate.
    pub fn is_genesis(&self) -> bool {
        self.header.round == Round(0)
            && self.header.parents.is_empty()
            && self.header.payload.is_empty()
            && self.header_signature.is_empty()
            && self.votes.is_empty()
    }

    /// The voters.
    pub fn voters(&self) -> BTreeSet<ValidatorId> {
        self.votes.iter().map(|(v, _)| *v).collect()
    }

    /// Checks the author signature, every vote, and the quorum.
    pub fn verify<V: SignatureVerifier>(
        &self,
        committee: &Committee,
        verifier: &V,
    ) -> Result<(), ConsensusError> {
        let header = &self.header;
        if header.epoch != committee.epoch() || header.shard != committee.shard() {
            return Err(ConsensusError::Mismatch(
                "certificate is for another committee",
            ));
        }
        if !committee.contains(&header.author) {
            return Err(ConsensusError::UnknownValidator(header.author));
        }
        if header.round == Round(0) {
            return if self.is_genesis() {
                Ok(())
            } else {
                Err(ConsensusError::Malformed(
                    "non-canonical genesis certificate",
                ))
            };
        }
        let digest = header.digest();
        if !verifier.verify(
            &header.author,
            &Header::signing_digest(&digest).0,
            &self.header_signature,
        ) {
            return Err(ConsensusError::InvalidSignature(header.author));
        }
        let message = Vote::signing_digest(&digest);
        let mut seen = BTreeSet::new();
        let mut stake = 0u64;
        for (voter, signature) in &self.votes {
            if !seen.insert(*voter) {
                return Err(ConsensusError::DuplicateSigner(*voter));
            }
            if !committee.contains(voter) {
                return Err(ConsensusError::UnknownValidator(*voter));
            }
            if !verifier.verify(voter, &message.0, signature) {
                return Err(ConsensusError::InvalidSignature(*voter));
            }
            stake += committee.stake_of(voter);
        }
        if !committee.is_quorum(stake) {
            return Err(ConsensusError::InsufficientStake {
                have: stake,
                need: committee.quorum_threshold(),
            });
        }
        Ok(())
    }
}

/// Protocol messages.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum Message {
    /// A proposal, broadcast by its author.
    Header(SignedHeader),
    /// A vote, sent to the header's author.
    Vote(Vote),
    /// A certificate, broadcast by its author or forwarded on request.
    Certificate(Certificate),
    /// A request for certificates the sender is missing.
    SyncRequest(Vec<BlockDigest>),
}

/// Verifiable evidence of Byzantine behavior in the DAG protocol.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum DagEvidence {
    /// The author signed two different headers for the same round.
    HeaderEquivocation {
        /// One signed header.
        first: SignedHeader,
        /// A conflicting signed header.
        second: SignedHeader,
    },
    /// Two valid certificates exist for the same (author, round). Every
    /// voter in both double-voted; with Byzantine stake within tolerance
    /// this cannot happen.
    ConflictingCertificates {
        /// One certificate.
        first: Certificate,
        /// The conflicting certificate.
        second: Certificate,
    },
}

impl DagEvidence {
    /// Checks the evidence against the committee.
    pub fn verify<V: SignatureVerifier>(&self, committee: &Committee, verifier: &V) -> bool {
        match self {
            DagEvidence::HeaderEquivocation { first, second } => {
                first.header.author == second.header.author
                    && first.header.round == second.header.round
                    && first.header.digest() != second.header.digest()
                    && committee.contains(&first.header.author)
                    && first.verify_signature(verifier)
                    && second.verify_signature(verifier)
            }
            DagEvidence::ConflictingCertificates { first, second } => {
                first.author() == second.author()
                    && first.round() == second.round()
                    && first.digest() != second.digest()
                    && first.verify(committee, verifier).is_ok()
                    && second.verify(committee, verifier).is_ok()
            }
        }
    }

    /// The validators this evidence incriminates.
    pub fn offenders(&self) -> BTreeSet<ValidatorId> {
        match self {
            DagEvidence::HeaderEquivocation { first, .. } => {
                std::iter::once(first.header.author).collect()
            }
            DagEvidence::ConflictingCertificates { first, second } => {
                let mut offenders: BTreeSet<ValidatorId> = first
                    .voters()
                    .intersection(&second.voters())
                    .copied()
                    .collect();
                offenders.insert(first.author());
                offenders
            }
        }
    }
}
