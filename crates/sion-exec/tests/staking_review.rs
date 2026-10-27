//! Regression tests from the independent review of the validator staking,
//! epoch and committee machinery (spec v1.4). Each test first failed,
//! pinning a defect, and now guards its fix (the node's lock release and
//! spec v1.5). See `docs/book/src/security/reviews.md`.

use sion_consensus::{
    ApprovalProof, Authenticator, Committee, Header, LaneRequest, LaneValidator,
    OwnerAuthorization, SignedHeader,
};
use sion_core::encode::encode;
use sion_core::hash::Hash32;
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

/// One validator's lane state for shard 0 of epoch 0, the validator alone.
fn lane_of(auth: &KeyAuth) -> LaneValidator<KeyAuth> {
    let committee = Committee::equal_weight(Epoch(0), ShardId(0), [auth.validator()]).unwrap();
    LaneValidator::new(&committee, auth.clone())
}

/// Approves `tx` in `lane`, seals the batch and returns the approval.
fn approve(lane: &mut LaneValidator<KeyAuth>, tx: &SignedTransaction) -> ApprovalProof {
    lane.approve(lane_request(tx), &AnyOwner).unwrap();
    lane.seal_batch();
    lane.approval_proof(&tx.digest()).unwrap().clone()
}

/// Reports two approvals as equivocation in the next Pulse; returns the
/// report's outcome.
fn report_approvals(
    chain: &mut Chain,
    first: (&SignedTransaction, ApprovalProof),
    second: (&SignedTransaction, ApprovalProof),
) -> ExecStatus {
    let evidence = Equivocation::Approvals {
        first: Box::new(EvidenceApproval {
            tx: first.0.tx.clone(),
            proof: first.1,
        }),
        second: Box::new(EvidenceApproval {
            tx: second.0.tx.clone(),
            proof: second.1,
        }),
    };
    let report = chain
        .tx(
            "bob",
            TxKind::ReportEquivocation {
                evidence: Box::new(evidence),
            },
        )
        .unwrap();
    let out = chain.pulse(vec![report.clone()]).unwrap();
    status(&out, &report)
}

/// FINDING (high), fixed in the node: `ReportEquivocation`'s `Approvals`
/// evidence (spec §5.30) could slash an honest validator.
///
/// The rule treats two approvals by one validator of transactions sharing an
/// input version, both unexpired, as proof of equivocation, because an
/// honest validator approves a version again only once the first
/// transaction can no longer execute. The node used to release a
/// transaction's lane locks as soon as it appeared in a Pulse's effects,
/// rejected transactions included. A rejected transaction wrote nothing: its
/// inputs keep their versions, so the honest validator then approved a
/// second spend of a version its first approval still covered, and the two
/// approvals slashed it. (The review's scenario: an owner double-spends a
/// sibling input `y` of `A = {x, y}` so that `A` is rejected, then spends `x`
/// in `B`, and reports every validator that approved both.)
///
/// The node now keeps a rejected transaction's locks until it expires
/// (`released_locks` in `sion-node`'s `validator.rs`, with its own unit test).
/// This test runs the evidence rule against both behaviours: a validator
/// that releases early and approves `B` while `A` is live is slashed, and one
/// that waits for `A` to expire before approving `B` is not.
#[test]
fn approving_again_before_a_rejected_spend_expires_is_slashable_and_waiting_is_safe() {
    let mut chain = Chain::with_params(&["alice", "bob"], 50_000, 4, params(), forced(8));
    let honest = KeyAuth(std::sync::Arc::new(chain.validator_keys[0].clone()));
    let hasty = KeyAuth(std::sync::Arc::new(chain.validator_keys[1].clone()));
    for auth in [&honest, &hasty] {
        assert_eq!(
            chain.state.validators().validators[&auth.validator()].status,
            ValidatorStatus::Active
        );
    }

    // A spends a shared coin x and a sibling y and expires at Pulse 3; B
    // spends only x. (The Approvals evidence checks the validators'
    // approvals over the transactions' digests and inputs, not the owner's
    // signature, so the inputs are set directly.)
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
    a.tx.expires_at = Pulse(3);
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

    // Both validators approve A, locking x and y; while the lock holds, a
    // second spend of x is refused.
    let mut honest_lane = lane_of(&honest);
    let mut hasty_lane = lane_of(&hasty);
    let honest_a = approve(&mut honest_lane, &a);
    let hasty_a = approve(&mut hasty_lane, &a);
    assert!(honest_lane.approve(lane_request(&b), &AnyOwner).is_err());

    // A is rejected at execution (y was consumed elsewhere), so x keeps its
    // version. The hasty validator releases A at once, as the node used to,
    // and approves B; reported at Pulse 1, while A is live, that is
    // equivocation.
    hasty_lane.release(&a.digest());
    let hasty_b = approve(&mut hasty_lane, &b);
    assert_eq!(
        report_approvals(&mut chain, (&a, hasty_a), (&b, hasty_b)),
        ExecStatus::Success
    );
    assert!(matches!(
        chain.state.validators().validators[&hasty.validator()].status,
        ValidatorStatus::Slashed { .. }
    ));

    // The honest validator keeps A's locks until A expires: B is still
    // refused after the rejection.
    assert!(honest_lane.approve(lane_request(&b), &AnyOwner).is_err());

    // At Pulse 3 A can no longer execute; the node releases its locks and
    // the validator approves B. Reported afterwards, the two approvals are
    // not evidence: A has expired.
    chain.advance_to(Pulse(3)).unwrap();
    honest_lane.release(&a.digest());
    let honest_b = approve(&mut honest_lane, &b);
    assert!(matches!(
        report_approvals(&mut chain, (&a, honest_a), (&b, honest_b)),
        ExecStatus::Failed(_)
    ));
    assert_eq!(
        chain.state.validators().validators[&honest.validator()].status,
        ValidatorStatus::Active,
        "an honest validator is not slashed"
    );
}

/// FINDING (medium), fixed in spec v1.5: at an epoch boundary with no
/// eligible validator, the active set used to be kept unchanged, so a
/// committee made entirely of **slashed** validators carried on into the
/// next epoch, contrary to the epochs chapter ("Slashed validators are not
/// Active, so they lose their seats here") and to `impl:slashed-seat` (a
/// slashed validator keeps its seat "until the epoch ends").
///
/// Since v1.5 (spec §6.3, EpochEnd step 3) the next active set is the
/// `Active` records even if there are none: the chain halts rather than run
/// on proven equivocators with nothing at stake.
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

    // No slashed validator keeps a seat past the boundary.
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
    assert!(set.active.is_empty(), "nobody is left to certify a Pulse");
    let changed = chain
        .history
        .iter()
        .flat_map(|record| record.output.deferred_events.iter())
        .find(|event| matches!(event, Event::EpochChanged { epoch: 1, .. }))
        .cloned();
    assert_eq!(
        changed,
        Some(Event::EpochChanged {
            epoch: 1,
            active: 0,
            rewards: Amount::ZERO
        })
    );
}
