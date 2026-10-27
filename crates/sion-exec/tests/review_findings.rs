//! Independent security review: failing tests that demonstrate defects in the
//! validator staking / epoch / committee machinery (spec v1.3 / v1.4).
//!
//! These tests assert the *intended* guarantee, so they FAIL on the current
//! code, pinning the defect. Each test's doc comment names the finding.

use sion_consensus::{
    Authenticator, Committee, Header, LaneRequest, LaneValidator, OwnerAuthorization, SignedHeader,
};
use sion_core::encode::encode;
use sion_core::hash::{hash_domain, Hash32};
use sion_core::ids::{AssetId, Epoch, ObjectId, Pulse, Round, ShardId, ValidatorId, Version};
use sion_core::Amount;
use sion_crypto::{validator_id, DualKeypair, VALIDATOR_DOMAIN};
use sion_exec::object::ObjectRef;
use sion_exec::sim::Chain;
use sion_exec::validators::{
    Equivocation, EvidenceApproval, ForcedParams, ValidatorParams, ValidatorStatus,
};
use sion_exec::{Event, ExecStatus, PulseOutput, SignedTransaction, TxKind};

fn usd(n: u64) -> Amount {
    Amount::units(n)
}

fn params() -> ValidatorParams {
    ValidatorParams {
        stake_asset: AssetId::usd(),
        epoch_length: 10,
        min_stake: usd(1_000),
        unbonding: 5,
        seats: 2,
        max_active: 10,
        reward_permille: 500,
        reporter_permille: 500,
    }
}

fn forced(window: u64) -> ForcedParams {
    ForcedParams {
        window,
        max_pending: 16,
    }
}

fn status(out: &PulseOutput, tx: &SignedTransaction) -> ExecStatus {
    out.effects
        .iter()
        .find(|e| e.digest == tx.digest())
        .map(|e| e.status.clone())
        .expect("executed")
}

/// Signs as a validator holding `keys` (the node's dual scheme).
#[derive(Clone)]
struct KeyAuth(std::sync::Arc<DualKeypair>);

impl Authenticator for KeyAuth {
    fn validator(&self) -> ValidatorId {
        validator_id(self.0.lattice.public(), self.0.hash.public())
    }
    fn sign(&self, message: &[u8]) -> Vec<u8> {
        encode(
            &self
                .0
                .sign_deterministic(VALIDATOR_DOMAIN, message)
                .unwrap(),
        )
    }
}

/// Owner authorization stub: every lane request is authorized (the owner's
/// signature is checked elsewhere; lanes only need to know it is authentic).
struct AnyOwner;
impl OwnerAuthorization for AnyOwner {
    fn authorizes(&self, _: &LaneRequest) -> bool {
        true
    }
}

fn lane_request(tx: &SignedTransaction) -> LaneRequest {
    LaneRequest {
        tx: tx.digest(),
        inputs: tx
            .tx
            .inputs
            .iter()
            .map(|r| sion_consensus::ObjectRef {
                id: r.id,
                version: r.version,
            })
            .collect(),
        owner_proof: Vec::new(),
    }
}

/// FINDING (high): `ReportEquivocation`'s `Approvals` evidence (spec §5.30,
/// `Equivocation::offender`) slashes an honest validator.
///
/// The spec justifies the `Approvals` rule with: "an honest validator approves
/// a version again only once the first transaction can no longer execute (it
/// executed, or it expired)." But a transaction also stops being executable
/// when one of its *other* inputs is consumed by a competing certified
/// transaction. The node recognises exactly this: after a Pulse, it releases
/// the lane lock of every transaction that appears in the Pulse's effects,
/// including those **rejected** at execution (`sion-node/src/validator.rs`
/// ~1233: `executed_digests = output.effects.iter().map(|e| e.digest)` — and
/// `output.effects` carries `Rejected` outcomes). Having released the lock, an
/// honest validator legitimately approves a second spend of the still-live,
/// still-unexpired shared input. The evidence check only guards against
/// expiry, so it accepts the two honest approvals and the validator forfeits
/// its whole stake.
///
/// This test reproduces one honest validator's lane behaviour exactly:
///   1. approve A, which spends {x, y};
///   2. while A holds the locks, a second spend of x (B) is correctly REFUSED;
///   3. A is rejected at execution because its sibling input y was consumed
///      elsewhere, so the node releases A's locks;
///   4. the validator then approves B, which spends {x}.
/// The two approvals A and B are produced by an honest validator, yet they are
/// accepted as slashing evidence. The final assertion (the validator must not
/// be slashed) FAILS on the current code.
#[test]
fn honest_validator_is_not_slashed_after_a_rejected_sibling_spend() {
    let mut chain = Chain::with_params(&["alice", "bob"], 50_000, 4, params(), forced(8));
    let auth = KeyAuth(std::sync::Arc::new(chain.validator_keys[0].clone()));
    let victim = auth.validator();
    assert_eq!(
        chain.state.validators().validators[&victim].status,
        ValidatorStatus::Active,
        "the victim is an honest, active genesis validator"
    );

    // Two transactions of the owner. A spends a shared coin x and a sibling y;
    // B spends only x. (Inputs are set explicitly; the owner's own signature is
    // irrelevant to the Approvals evidence, which checks only the validator's
    // approval over the transaction digest and its inputs.)
    let x = ObjectRef {
        id: ObjectId::derive(b"review/shared-coin-x"),
        version: Version(0),
    };
    let y = ObjectRef {
        id: ObjectId::derive(b"review/sibling-coin-y"),
        version: Version(0),
    };
    let mut a = chain
        .tx(
            "alice",
            TxKind::Transfer {
                asset: AssetId::usd(),
                outputs: vec![(chain.id("bob"), usd(1))],
            },
        )
        .unwrap();
    a.tx.inputs = vec![x, y];
    a.tx.expires_at = Pulse(1000);
    let mut b = chain
        .tx(
            "alice",
            TxKind::Transfer {
                asset: AssetId::usd(),
                outputs: vec![(chain.id("alice"), usd(1))],
            },
        )
        .unwrap();
    b.tx.inputs = vec![x];
    b.tx.expires_at = Pulse(1000);
    assert_ne!(a.digest(), b.digest());

    // One honest validator's lane state for shard 0, epoch 0.
    let committee = Committee::equal_weight(Epoch(0), ShardId(0), [victim]).unwrap();
    let mut lane = LaneValidator::new(&committee, auth.clone());

    // 1. Approve A (locks x and y to A); seal; keep A's approval proof.
    lane.approve(lane_request(&a), &AnyOwner).unwrap();
    lane.seal_batch();
    let proof_a = lane.approval_proof(&a.digest()).unwrap().clone();

    // 2. While A holds x, the honest validator REFUSES a second spend of x.
    assert!(
        lane.approve(lane_request(&b), &AnyOwner).is_err(),
        "the lock on x is the mechanism that stops honest double approval"
    );

    // 3. A is rejected at execution (its sibling input y was consumed by a
    //    competing certified transaction), so the node releases A's locks.
    //    This is precisely what `validator.rs` does for any digest in the
    //    Pulse's effects — rejected transactions included.
    lane.release(&a.digest());

    // 4. With x free again, the honest validator approves B.
    lane.approve(lane_request(&b), &AnyOwner).unwrap();
    lane.seal_batch();
    let proof_b = lane.approval_proof(&b.digest()).unwrap().clone();

    let evidence = Equivocation::Approvals {
        first: Box::new(EvidenceApproval {
            tx: a.tx.clone(),
            proof: proof_a,
        }),
        second: Box::new(EvidenceApproval {
            tx: b.tx.clone(),
            proof: proof_b,
        }),
    };

    // Both transactions are still unexpired, so the evidence is accepted and
    // names the honest validator as the offender.
    let report = chain
        .tx(
            "bob",
            TxKind::ReportEquivocation {
                evidence: Box::new(evidence),
            },
        )
        .unwrap();
    let out = chain.pulse(vec![report.clone()]).unwrap();

    // The intended guarantee: an honest validator cannot be slashed. This
    // FAILS on the current code — the report succeeds and the stake is gone.
    assert!(
        !matches!(status(&out, &report), ExecStatus::Success),
        "honest validator framed: the equivocation report was accepted"
    );
    assert!(
        !matches!(
            chain.state.validators().validators[&victim].status,
            ValidatorStatus::Slashed { .. }
        ),
        "honest validator was slashed for a rejected sibling spend"
    );
}

/// FINDING (medium): at an epoch boundary with no eligible active validator,
/// the active set is kept unchanged, so a committee made entirely of **slashed**
/// validators persists into the next epoch.
///
/// `docs/.../epochs.md` step 3 states "Slashed validators are not Active, so
/// they lose their seats here", and the trust atom `impl:slashed-seat` says a
/// slashed validator keeps its seat only "until the epoch ends". But
/// `end_epoch` (spec §6.3 step 3; `sion-exec/src/pulse.rs`) keeps the old
/// active set whenever no `Active` record remains. If every active validator is
/// slashed in an epoch, the next epoch's consensus committee is exactly those
/// slashed, zero-stake validators — they keep their seats *past* the boundary,
/// contrary to both the chapter and the documented simplification.
#[test]
fn slashed_validators_lose_their_seats_at_the_boundary() {
    let mut chain = Chain::with_params(&["alice", "bob"], 50_000, 4, params(), forced(8));
    let reporters = ["alice", "bob", "alice", "bob"];

    // Slash all four genesis validators for header equivocation, one per Pulse.
    for (i, keys) in chain.validator_keys.clone().into_iter().enumerate() {
        let auth = KeyAuth(std::sync::Arc::new(keys));
        let offender = auth.validator();
        let header = |payload: &[u8]| {
            let header = Header {
                author: offender,
                chain: Hash32([1; 32]),
                round: Round(7),
                epoch: Epoch(0),
                shard: ShardId(0),
                parents: Vec::new(),
                payload: vec![payload.to_vec()],
            };
            SignedHeader {
                signature: auth.sign(&Header::signing_digest(&header.digest()).0),
                header,
            }
        };
        let evidence = Equivocation::Headers {
            first: header(b"a"),
            second: header(b"b"),
        };
        let report = chain
            .tx(
                reporters[i],
                TxKind::ReportEquivocation {
                    evidence: Box::new(evidence),
                },
            )
            .unwrap();
        let out = chain.pulse(vec![report.clone()]).unwrap();
        assert_eq!(status(&out, &report), ExecStatus::Success);
    }

    // Every validator is now slashed.
    assert!(chain
        .state
        .validators()
        .validators
        .values()
        .all(|r| matches!(r.status, ValidatorStatus::Slashed { .. })));

    // Cross the epoch boundary (epoch_length = 10).
    chain.advance_to(Pulse(10)).unwrap();
    let set = chain.state.validators();
    assert_eq!(set.epoch, 1, "the epoch advanced");

    // The intended guarantee: no slashed validator keeps a seat past the
    // boundary. This FAILS — the whole stale active set is retained.
    let slashed_seated: Vec<_> = set
        .active
        .iter()
        .filter(|(v, _)| matches!(set.validators[v].status, ValidatorStatus::Slashed { .. }))
        .collect();
    assert!(
        slashed_seated.is_empty(),
        "a committee of {} slashed, zero-stake validators seats the next epoch: {:?}",
        slashed_seated.len(),
        slashed_seated
    );
    let _ = &chain;
}
