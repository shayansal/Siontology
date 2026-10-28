//! What consensus orders.
//!
//! Consensus never orders the owned-path traffic itself: an owned
//! transaction is final once its lane certificate exists. Consensus only
//! fixes *which Pulse executes it*, alongside the contended transactions it
//! does order, and the beacon reveals. Every transaction, contended or not,
//! first locks its owned inputs with a lane certificate.
//!
//! **Batch-root voting.** A validator signs one Merkle root per batch of
//! approvals, so a certificate is three (or more) references into signed
//! batches. Certified transactions travel in groups that carry each signed
//! batch header once and each approval as a reference (batch, position,
//! path): a group of a hundred transactions costs a handful of dual
//! signatures instead of three hundred.

use std::collections::{BTreeMap, BTreeSet};

use borsh::{BorshDeserialize, BorshSerialize};
use sion_consensus::lanes::canonical_inputs;
use sion_consensus::{ApprovalProof, BatchHeader, LaneCertificate, ObjectRef as LaneRef};
use sion_core::encode::{decode, encode};
use sion_core::hash::Hash32;
use sion_core::ids::{Pulse, ValidatorId};
use sion_exec::{ObjectRef, SignedTransaction};

/// A validator's signed batch header.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SignedBatchHeader {
    /// The header.
    pub header: BatchHeader,
    /// The validator's signature over its signing digest.
    pub signature: Vec<u8>,
}

/// One approval, by reference into a group's batch headers.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ApprovalRef {
    /// Index of the batch header in the group.
    pub batch: u32,
    /// Position of the approval in the batch.
    pub leaf_index: u64,
    /// Inclusion path to the batch root.
    pub path: Vec<Hash32>,
}

/// A certified transaction inside a group.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct GroupedTx {
    /// The transaction.
    pub tx: SignedTransaction,
    /// Its approvals.
    pub approvals: Vec<ApprovalRef>,
}

/// One consensus payload item.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum PayloadItem {
    /// Certified transactions (owned-path or contended) sharing batch headers.
    Certified {
        /// Signed batch headers, each once.
        batches: Vec<SignedBatchHeader>,
        /// Transactions with their approvals.
        txs: Vec<GroupedTx>,
    },
    /// A validator's beacon reveal for a Pulse.
    Reveal {
        /// The validator.
        validator: ValidatorId,
        /// The Pulse.
        pulse: Pulse,
        /// The hash-chain preimage.
        value: Hash32,
    },
    /// A forced intent: a transaction its sender posted directly to
    /// consensus, which its shard must now execute or show invalid within
    /// the forced-inclusion window (see [`crate::forced`]).
    Forced {
        /// The transaction.
        tx: Box<SignedTransaction>,
    },
}

impl PayloadItem {
    /// Canonical bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        encode(self)
    }

    /// Parses canonical bytes.
    pub fn from_bytes(bytes: &[u8]) -> Option<PayloadItem> {
        decode(bytes).ok()
    }

    /// The senders of the transactions this item carries.
    pub fn senders(&self) -> Vec<sion_core::ids::AccountId> {
        match self {
            PayloadItem::Certified { txs, .. } => txs.iter().map(|t| t.tx.tx.sender).collect(),
            PayloadItem::Forced { tx } => vec![tx.tx.sender],
            PayloadItem::Reveal { .. } => Vec::new(),
        }
    }

    /// Groups certified transactions, sharing batch headers.
    ///
    /// A batch is identified by its whole header (validator, epoch, shard,
    /// sequence number, root and size): a validator numbers its batches per
    /// shard, so the same sequence number recurs across shards and epochs.
    pub fn group(certified: Vec<(SignedTransaction, LaneCertificate)>) -> PayloadItem {
        let mut index: BTreeMap<Hash32, u32> = BTreeMap::new();
        let mut batches = Vec::new();
        let mut txs = Vec::with_capacity(certified.len());
        for (tx, certificate) in certified {
            let mut approvals = Vec::with_capacity(certificate.approvals.len());
            for proof in certificate.approvals {
                let key = proof.batch.signing_digest();
                let position = *index.entry(key).or_insert_with(|| {
                    batches.push(SignedBatchHeader {
                        header: proof.batch.clone(),
                        signature: proof.signature.clone(),
                    });
                    (batches.len() - 1) as u32
                });
                approvals.push(ApprovalRef {
                    batch: position,
                    leaf_index: proof.leaf_index,
                    path: proof.path,
                });
            }
            txs.push(GroupedTx { tx, approvals });
        }
        PayloadItem::Certified { batches, txs }
    }

    /// Splits certified transactions into groups that each fit a consensus
    /// payload: at most `max_bytes` encoded bytes, `max_txs` transactions
    /// and [`MAX_GROUP_BATCHES`] batch headers, sharing headers within each
    /// group. Order is kept.
    ///
    /// A transaction that does not fit even in an empty group goes alone:
    /// the caller checks such a group against the hard limit it enforces.
    pub fn groups(
        certified: Vec<(SignedTransaction, LaneCertificate)>,
        max_bytes: usize,
        max_txs: usize,
    ) -> Vec<PayloadItem> {
        let mut out = Vec::new();
        let mut current: Vec<(SignedTransaction, LaneCertificate)> = Vec::new();
        let mut headers: BTreeSet<Hash32> = BTreeSet::new();
        let mut bytes = EMPTY_GROUP_LEN;
        for (tx, certificate) in certified {
            let added = |headers: &BTreeSet<Hash32>| {
                let mut fresh = BTreeSet::new();
                let mut len = grouped_len(&tx, &certificate);
                for proof in &certificate.approvals {
                    let key = proof.batch.signing_digest();
                    if !headers.contains(&key) && fresh.insert(key) {
                        len += header_len(proof);
                    }
                }
                (len, fresh)
            };
            let (mut len, mut fresh) = added(&headers);
            let full = bytes + len > max_bytes
                || current.len() >= max_txs
                || headers.len() + fresh.len() > MAX_GROUP_BATCHES;
            if full && !current.is_empty() {
                out.push(PayloadItem::group(std::mem::take(&mut current)));
                headers.clear();
                bytes = EMPTY_GROUP_LEN;
                (len, fresh) = added(&headers);
            }
            bytes += len;
            headers.extend(fresh);
            current.push((tx, certificate));
        }
        if !current.is_empty() {
            out.push(PayloadItem::group(current));
        }
        out
    }
}

/// The most batch headers one group may carry; validators refuse larger
/// groups before expanding anything.
pub const MAX_GROUP_BATCHES: usize = 4_096;

/// Encoded length of an empty certified group: the variant tag and two
/// vector lengths.
const EMPTY_GROUP_LEN: usize = 1 + 4 + 4;

/// Encoded length of a transaction inside a group: the transaction, then
/// each approval as a reference (batch index, leaf index, path).
fn grouped_len(tx: &SignedTransaction, certificate: &LaneCertificate) -> usize {
    let approvals: usize = certificate
        .approvals
        .iter()
        .map(|proof| 4 + 8 + 4 + 32 * proof.path.len())
        .sum();
    encoded_len(tx) + 4 + approvals
}

/// Encoded length of the signed batch header an approval references.
fn header_len(proof: &ApprovalProof) -> usize {
    encoded_len(&proof.batch) + 4 + proof.signature.len()
}

fn encoded_len<T: BorshSerialize>(value: &T) -> usize {
    borsh::object_length(value).expect("values in memory encode")
}

/// Expands a grouped transaction back into its full lane certificate
/// (`None` if it references a batch the group does not carry).
pub fn expand(batches: &[SignedBatchHeader], grouped: &GroupedTx) -> Option<LaneCertificate> {
    let approvals = grouped
        .approvals
        .iter()
        .map(|r| {
            let batch = batches.get(r.batch as usize)?;
            Some(ApprovalProof {
                batch: batch.header.clone(),
                signature: batch.signature.clone(),
                leaf_index: r.leaf_index,
                path: r.path.clone(),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(LaneCertificate {
        tx: grouped.tx.digest(),
        inputs: canonical_inputs(&lane_inputs(&grouped.tx.tx.inputs)),
        approvals,
    })
}

/// The lane-layer view of a transaction's owned inputs.
pub fn lane_inputs(inputs: &[ObjectRef]) -> Vec<LaneRef> {
    inputs
        .iter()
        .map(|r| LaneRef {
            id: r.id,
            version: r.version,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use sion_consensus::{
        insecure_sim_keyring, verify_lane_certificate, Authenticator, CertificateBuilder,
        Committee, LaneRequest, LaneValidator, OwnerAuthorization,
    };
    use sion_core::ids::{AssetId, Epoch, ObjectId, ShardId, Version};
    use sion_core::Amount;
    use sion_exec::{Transaction, TxKind, Wallet};

    use super::*;

    struct AnyOwner;

    impl OwnerAuthorization for AnyOwner {
        fn authorizes(&self, _: &LaneRequest) -> bool {
            true
        }
    }

    fn transfer(wallet: &Wallet, coin: &[u8], nonce: u64) -> SignedTransaction {
        Transaction {
            sender: wallet.id,
            genesis_policy: None,
            inputs: vec![ObjectRef {
                id: ObjectId::derive(coin),
                version: Version(0),
            }],
            fee: Amount::units(1),
            kind: TxKind::Transfer {
                asset: AssetId::usd(),
                outputs: Vec::new(),
            },
            expires_at: Pulse(100),
            nonce,
        }
        .sign(&wallet.keys, &sion_exec::ChainConfig::devnet(None))
        .expect("sign")
    }

    /// `count` transfers certified by four validators of one shard, each
    /// validator sealing a batch after every `per_batch` approvals.
    fn certified(
        count: u64,
        per_batch: usize,
    ) -> (
        Vec<(SignedTransaction, LaneCertificate)>,
        Committee,
        sion_consensus::InsecureSimVerifier,
    ) {
        let (auths, verifier) = insecure_sim_keyring(4, 7);
        let ids: Vec<_> = auths.iter().map(|a| a.validator()).collect();
        let committee = Committee::equal_weight(Epoch(0), ShardId(0), ids.iter().copied()).unwrap();
        let mut lanes: Vec<LaneValidator<_>> = auths
            .iter()
            .map(|a| LaneValidator::new(&committee, a.clone()))
            .collect();
        let alice = Wallet::named("alice");
        let txs: Vec<SignedTransaction> = (0..count)
            .map(|i| transfer(&alice, &i.to_be_bytes(), i))
            .collect();
        let mut out = Vec::new();
        for chunk in txs.chunks(per_batch) {
            for lane in lanes.iter_mut() {
                for tx in chunk {
                    let request = LaneRequest {
                        tx: tx.digest(),
                        inputs: lane_inputs(&tx.tx.inputs),
                        owner_proof: Vec::new(),
                    };
                    lane.approve(request, &AnyOwner).unwrap();
                }
                lane.seal_batch().unwrap();
            }
            for tx in chunk {
                let mut builder = CertificateBuilder::new(tx.digest(), &lane_inputs(&tx.tx.inputs));
                let mut certificate = None;
                for lane in &lanes {
                    let proof = lane.approval_proof(&tx.digest()).unwrap().clone();
                    if let Some(done) = builder.add(&committee, proof, &verifier).unwrap() {
                        certificate.get_or_insert(done);
                    }
                }
                out.push((tx.clone(), certificate.expect("quorum")));
            }
        }
        (out, committee, verifier)
    }

    fn txs_of(item: &PayloadItem) -> &[GroupedTx] {
        match item {
            PayloadItem::Certified { txs, .. } => txs,
            _ => panic!("certified group expected"),
        }
    }

    #[test]
    fn groups_fit_the_byte_budget_and_keep_every_certificate() {
        let (certified, committee, verifier) = certified(24, 5);
        let whole = PayloadItem::group(certified.clone()).to_bytes().len();

        // A budget the whole group meets exactly gives one group; one byte
        // less gives two. The size estimate is exact.
        assert_eq!(PayloadItem::groups(certified.clone(), whole, 512).len(), 1);
        assert_eq!(
            PayloadItem::groups(certified.clone(), whole, 512)[0],
            PayloadItem::group(certified.clone())
        );
        assert_eq!(
            PayloadItem::groups(certified.clone(), whole - 1, 512).len(),
            2
        );

        // A third of the bytes: several groups, each within budget, in
        // order, every certificate expanding and verifying.
        let budget = whole / 3;
        let groups = PayloadItem::groups(certified.clone(), budget, 512);
        assert!(groups.len() >= 3, "{} groups", groups.len());
        let mut seen = Vec::new();
        for item in &groups {
            assert!(item.to_bytes().len() <= budget);
            let decoded = PayloadItem::from_bytes(&item.to_bytes()).expect("round trip");
            let PayloadItem::Certified { batches, txs } = decoded else {
                panic!("certified group expected");
            };
            for grouped in &txs {
                let expanded = expand(&batches, grouped).expect("expands");
                verify_lane_certificate(
                    &committee,
                    &grouped.tx.digest(),
                    &lane_inputs(&grouped.tx.tx.inputs),
                    &expanded,
                    &verifier,
                )
                .expect("verifies");
                seen.push(grouped.tx.digest());
            }
        }
        let expected: Vec<_> = certified.iter().map(|(tx, _)| tx.digest()).collect();
        assert_eq!(seen, expected);
    }

    #[test]
    fn groups_respect_the_transaction_cap_and_send_giants_alone() {
        let (certified, _, _) = certified(10, 4);
        let capped = PayloadItem::groups(certified.clone(), usize::MAX, 3);
        let sizes: Vec<usize> = capped.iter().map(|g| txs_of(g).len()).collect();
        assert_eq!(sizes, vec![3, 3, 3, 1]);

        // Nothing fits a 10-byte budget: each transaction goes alone, and
        // the caller decides what to do with a group over its hard limit.
        let alone = PayloadItem::groups(certified, 10, 512);
        assert_eq!(alone.len(), 10);
        assert!(alone.iter().all(|g| txs_of(g).len() == 1));
        assert!(PayloadItem::groups(Vec::new(), 10, 512).is_empty());
    }

    #[test]
    fn groups_keep_batches_of_different_shards_apart() {
        // Four validators sit on both shard committees and number their
        // batches per shard, so each shard's first batch has index 0.
        let (auths, verifier) = insecure_sim_keyring(4, 7);
        let ids: Vec<_> = auths.iter().map(|a| a.validator()).collect();
        let committees: Vec<Committee> = (0..2)
            .map(|s| Committee::equal_weight(Epoch(0), ShardId(s), ids.iter().copied()).unwrap())
            .collect();
        let mut lanes: Vec<Vec<LaneValidator<_>>> = committees
            .iter()
            .map(|c| {
                auths
                    .iter()
                    .map(|a| LaneValidator::new(c, a.clone()))
                    .collect()
            })
            .collect();

        let alice = Wallet::named("alice");
        let txs = [
            transfer(&alice, b"coin-a", 1),
            transfer(&alice, b"coin-b", 2),
        ];
        let mut certified = Vec::new();
        for (shard, tx) in txs.iter().enumerate() {
            let inputs = lane_inputs(&tx.tx.inputs);
            let request = LaneRequest {
                tx: tx.digest(),
                inputs: inputs.clone(),
                owner_proof: Vec::new(),
            };
            let mut builder = CertificateBuilder::new(tx.digest(), &inputs);
            let mut certificate = None;
            for lane in lanes[shard].iter_mut() {
                lane.approve(request.clone(), &AnyOwner).unwrap();
                let batch = lane.seal_batch().unwrap();
                assert_eq!(batch.header.index, 0);
                let proof = lane.approval_proof(&tx.digest()).unwrap().clone();
                if let Some(done) = builder.add(&committees[shard], proof, &verifier).unwrap() {
                    certificate.get_or_insert(done);
                }
            }
            certified.push((tx.clone(), certificate.expect("quorum")));
        }

        let item = PayloadItem::group(certified.clone());
        let item = PayloadItem::from_bytes(&item.to_bytes()).expect("round trip");
        let PayloadItem::Certified {
            batches,
            txs: grouped,
        } = item
        else {
            panic!("certified group expected");
        };
        // Same validators and indices in both shards, yet every approval
        // keeps its own shard's header.
        let approvals: usize = certified.iter().map(|(_, c)| c.approvals.len()).sum();
        assert_eq!(approvals, 6, "a quorum of three per certificate");
        assert_eq!(batches.len(), approvals);
        for ((tx, original), (shard, grouped)) in certified.iter().zip(grouped.iter().enumerate()) {
            let expanded = expand(&batches, grouped).expect("expands");
            assert_eq!(&expanded.approvals, &original.approvals);
            verify_lane_certificate(
                &committees[shard],
                &tx.digest(),
                &lane_inputs(&tx.tx.inputs),
                &expanded,
                &verifier,
            )
            .expect("the expanded certificate verifies");
        }
    }
}
