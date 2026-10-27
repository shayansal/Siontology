//! Linear Lanes: certificates, batch-root voting and equivocation.

use fold_consensus::{
    find_conflicting_approvals, insecure_sim_keyring, verify_lane_certificate, ApprovalProof,
    ApprovalStatus, Authenticator, CertificateBuilder, Committee, ConsensusError,
    InsecureSimAuthenticator, InsecureSimVerifier, LaneCertificate, LaneRequest, LaneValidator,
    ObjectRef, OwnerAuthorization,
};
use fold_core::{hash_parts, Epoch, ObjectId, ShardId, TxDigest, Version};

/// Test owners: a request is authorized when its proof is the owner's tag
/// over the transaction digest.
struct TestOwners;

fn owner_tag(tx: &TxDigest) -> Vec<u8> {
    hash_parts("test/owner", &[&tx.0 .0]).0.to_vec()
}

impl OwnerAuthorization for TestOwners {
    fn authorizes(&self, request: &LaneRequest) -> bool {
        request.owner_proof == owner_tag(&request.tx)
    }
}

fn coin(i: u8) -> ObjectRef {
    ObjectRef {
        id: ObjectId::derive(&[i]),
        version: Version(0),
    }
}

fn request(label: &str, inputs: &[ObjectRef]) -> LaneRequest {
    let tx = TxDigest::derive(label.as_bytes());
    LaneRequest {
        tx,
        inputs: inputs.to_vec(),
        owner_proof: owner_tag(&tx),
    }
}

fn setup(
    n: usize,
) -> (
    Committee,
    Vec<InsecureSimAuthenticator>,
    InsecureSimVerifier,
) {
    let (keys, verifier) = insecure_sim_keyring(n, 42);
    let committee =
        Committee::equal_weight(Epoch(1), ShardId(0), keys.iter().map(|k| k.validator())).unwrap();
    (committee, keys, verifier)
}

/// Approves `request` at `validator`, seals, and returns the proof.
fn approve_and_seal(
    validator: &mut LaneValidator<InsecureSimAuthenticator>,
    request: &LaneRequest,
) -> Option<ApprovalProof> {
    validator.approve(request.clone(), &TestOwners).ok()?;
    validator.seal_batch();
    validator.approval_proof(&request.tx).cloned()
}

fn certificate_from(
    committee: &Committee,
    verifier: &InsecureSimVerifier,
    request: &LaneRequest,
    proofs: &[ApprovalProof],
) -> Option<LaneCertificate> {
    let mut builder = CertificateBuilder::new(request.tx, &request.inputs);
    let mut certificate = None;
    for proof in proofs {
        if let Some(c) = builder.add(committee, proof.clone(), verifier).unwrap() {
            certificate = Some(c);
        }
    }
    certificate
}

#[test]
fn honest_certificate_forms_at_quorum_and_verifies() {
    let (committee, keys, verifier) = setup(4);
    let mut validators: Vec<_> = keys
        .iter()
        .map(|k| LaneValidator::new(&committee, k.clone()))
        .collect();
    let pay = request("pay-alice", &[coin(1), coin(2)]);
    let proofs: Vec<ApprovalProof> = validators
        .iter_mut()
        .map(|v| approve_and_seal(v, &pay).unwrap())
        .collect();

    // Two approvals are not enough; the third completes the quorum.
    assert!(certificate_from(&committee, &verifier, &pay, &proofs[..2]).is_none());
    let certificate = certificate_from(&committee, &verifier, &pay, &proofs[..3]).unwrap();
    assert_eq!(
        verify_lane_certificate(&committee, &pay.tx, &pay.inputs, &certificate, &verifier),
        Ok(3)
    );

    // Wrong inputs, wrong transaction, duplicate signers and forged
    // signatures are all rejected.
    assert!(matches!(
        verify_lane_certificate(&committee, &pay.tx, &[coin(1)], &certificate, &verifier),
        Err(ConsensusError::Mismatch(_))
    ));
    let other = request("pay-bob", &[coin(1), coin(2)]);
    assert!(verify_lane_certificate(
        &committee,
        &other.tx,
        &other.inputs,
        &certificate,
        &verifier
    )
    .is_err());
    let mut duplicated = certificate.clone();
    duplicated.approvals.push(duplicated.approvals[0].clone());
    assert!(matches!(
        verify_lane_certificate(&committee, &pay.tx, &pay.inputs, &duplicated, &verifier),
        Err(ConsensusError::DuplicateSigner(_))
    ));
    let mut forged = certificate.clone();
    forged.approvals[1].signature[0] ^= 1;
    assert!(matches!(
        verify_lane_certificate(&committee, &pay.tx, &pay.inputs, &forged, &verifier),
        Err(ConsensusError::InvalidSignature(_))
    ));
    let mut thin = certificate.clone();
    thin.approvals.truncate(2);
    assert!(matches!(
        verify_lane_certificate(&committee, &pay.tx, &pay.inputs, &thin, &verifier),
        Err(ConsensusError::InsufficientStake { have: 2, need: 3 })
    ));
}

#[test]
fn batch_root_voting_needs_one_signature_per_batch() {
    let (committee, keys, verifier) = setup(4);
    let mut validator = LaneValidator::new(&committee, keys[0].clone());
    let requests: Vec<LaneRequest> = (0..100u8)
        .map(|i| request(&format!("tx-{i}"), &[coin(i)]))
        .collect();
    for r in &requests {
        assert_eq!(
            validator.approve(r.clone(), &TestOwners).unwrap(),
            ApprovalStatus::Pending
        );
    }
    let batch = validator.seal_batch().unwrap();
    assert_eq!(batch.header.size, 100);
    assert!(validator.seal_batch().is_none(), "nothing left to seal");
    for r in &requests {
        let proof = validator.approval_proof(&r.tx).unwrap();
        assert_eq!(
            proof.signature, batch.signature,
            "one signature covers the batch"
        );
        proof
            .verify(&committee, &r.tx, &r.inputs, &verifier)
            .unwrap();
    }
    // Re-submitting an approved transaction returns its sealed proof.
    assert!(matches!(
        validator.approve(requests[5].clone(), &TestOwners).unwrap(),
        ApprovalStatus::Sealed(_)
    ));
}

#[test]
fn locks_are_atomic_and_owner_equivocation_is_evidence() {
    let (committee, keys, _) = setup(4);
    let mut validator = LaneValidator::new(&committee, keys[0].clone());
    let first = request("first", &[coin(1), coin(2)]);
    let second = request("second", &[coin(2), coin(3)]);
    validator.approve(first.clone(), &TestOwners).unwrap();
    let rejection = validator.approve(second.clone(), &TestOwners).unwrap_err();
    assert_eq!(
        rejection.error,
        ConsensusError::LockConflict {
            input: coin(2),
            existing: first.tx
        }
    );
    let evidence = rejection.evidence.unwrap();
    assert!(evidence.verify(&TestOwners));
    // The rejected request locked nothing, so coin 3 stays free.
    assert_eq!(validator.lock_holder(&coin(3)), None);
    assert_eq!(validator.locked_count(), 2);
    // Unauthorized requests are refused outright.
    let mut forged = request("forged", &[coin(9)]);
    forged.owner_proof = vec![0; 32];
    assert!(validator.approve(forged, &TestOwners).is_err());
}

/// Every way three honest validators can see two conflicting spends first,
/// with one Byzantine validator approving both: at most one certificate.
#[test]
fn conflicting_certificates_impossible_below_a_third_byzantine() {
    let (committee, keys, verifier) = setup(4);
    let spend_a = request("spend-a", &[coin(7)]);
    let spend_b = request("spend-b", &[coin(7)]);
    for assignment in 0u8..8 {
        let mut proofs_a = Vec::new();
        let mut proofs_b = Vec::new();
        for (index, key) in keys.iter().enumerate().take(3) {
            let mut honest = LaneValidator::new(&committee, key.clone());
            let (first, second) = if assignment & (1 << index) == 0 {
                (&spend_a, &spend_b)
            } else {
                (&spend_b, &spend_a)
            };
            let proof = approve_and_seal(&mut honest, first).unwrap();
            let rejection = honest.approve(second.clone(), &TestOwners).unwrap_err();
            assert!(rejection.evidence.unwrap().verify(&TestOwners));
            if first.tx == spend_a.tx {
                proofs_a.push(proof);
            } else {
                proofs_b.push(proof);
            }
        }
        // The Byzantine validator approves both, from two separate states.
        let mut byzantine_a = LaneValidator::new(&committee, keys[3].clone());
        let mut byzantine_b = LaneValidator::new(&committee, keys[3].clone());
        proofs_a.push(approve_and_seal(&mut byzantine_a, &spend_a).unwrap());
        proofs_b.push(approve_and_seal(&mut byzantine_b, &spend_b).unwrap());
        let a = certificate_from(&committee, &verifier, &spend_a, &proofs_a);
        let b = certificate_from(&committee, &verifier, &spend_b, &proofs_b);
        assert!(
            !(a.is_some() && b.is_some()),
            "two conflicting certificates under assignment {assignment:03b}"
        );
    }
}

/// With a third of the stake equivocating, both certificates can form, and
/// the overlap identifies exactly the equivocators.
#[test]
fn conflicting_certificates_at_a_third_byzantine_yield_slashable_evidence() {
    let (committee, keys, verifier) = setup(4);
    let spend_a = request("spend-a", &[coin(7)]);
    let spend_b = request("spend-b", &[coin(7)]);
    let mut honest_0 = LaneValidator::new(&committee, keys[0].clone());
    let mut honest_1 = LaneValidator::new(&committee, keys[1].clone());
    let mut proofs_a = vec![approve_and_seal(&mut honest_0, &spend_a).unwrap()];
    let mut proofs_b = vec![approve_and_seal(&mut honest_1, &spend_b).unwrap()];
    for key in &keys[2..] {
        let mut for_a = LaneValidator::new(&committee, key.clone());
        let mut for_b = LaneValidator::new(&committee, key.clone());
        proofs_a.push(approve_and_seal(&mut for_a, &spend_a).unwrap());
        proofs_b.push(approve_and_seal(&mut for_b, &spend_b).unwrap());
    }
    let a = certificate_from(&committee, &verifier, &spend_a, &proofs_a).unwrap();
    let b = certificate_from(&committee, &verifier, &spend_b, &proofs_b).unwrap();
    verify_lane_certificate(&committee, &spend_a.tx, &spend_a.inputs, &a, &verifier).unwrap();
    verify_lane_certificate(&committee, &spend_b.tx, &spend_b.inputs, &b, &verifier).unwrap();

    let evidence = find_conflicting_approvals(&a, &b);
    let offenders: std::collections::BTreeSet<_> = evidence.iter().map(|e| e.validator).collect();
    let byzantine: std::collections::BTreeSet<_> =
        keys[2..].iter().map(|k| k.validator()).collect();
    assert_eq!(offenders, byzantine);
    assert!(evidence.iter().all(|e| e.verify(&committee, &verifier)));
}

#[test]
fn weighted_committees_count_stake_not_heads() {
    let (keys, verifier) = insecure_sim_keyring(3, 5);
    let committee = Committee::new(
        Epoch(0),
        ShardId(3),
        vec![
            (keys[0].validator(), 50),
            (keys[1].validator(), 30),
            (keys[2].validator(), 20),
        ],
    )
    .unwrap();
    let pay = request("weighted", &[coin(4)]);
    let mut proofs = Vec::new();
    for key in [&keys[0], &keys[2]] {
        let mut validator = LaneValidator::new(&committee, key.clone());
        proofs.push(approve_and_seal(&mut validator, &pay).unwrap());
    }
    let certificate = certificate_from(&committee, &verifier, &pay, &proofs).unwrap();
    assert_eq!(
        verify_lane_certificate(&committee, &pay.tx, &pay.inputs, &certificate, &verifier),
        Ok(70)
    );
    // An approval for another committee (shard) is rejected.
    let other_shard =
        Committee::equal_weight(Epoch(0), ShardId(4), keys.iter().map(|k| k.validator())).unwrap();
    assert!(
        verify_lane_certificate(&other_shard, &pay.tx, &pay.inputs, &certificate, &verifier)
            .is_err()
    );
}
