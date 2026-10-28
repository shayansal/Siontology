//! Robots sharing a warehouse, end to end: two robots record what they see,
//! one finds an aisle empty where the other saw a pallet, a curator records
//! the conflict and decides it, consumers with different policies fold
//! different beliefs from the same evidence, a curator's cell digest is
//! convicted of leaving an atom out, and a sealed camera frame is forgotten.
//! Everything is checked from heads alone, as a light client would.

use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, Pulse};
use sion_memory::{
    belief_at, prove_belief, prove_cell_omission, prove_omission, prove_range, select_atoms,
    verify_belief, verify_cell_omission, verify_omission, verify_range, AtomClass, BeliefPolicy,
    BeliefState, Box3, CellKey, Commit, CommitHistory, ConflictKind, CuratedCell, FrameId,
    Interval, LinkKind, Memory, MemoryAtom, MemoryIndex, MemoryKind, PayloadRequest, RangeQuery,
    RecallRequest, RememberRequest, ReviseRequest, ReviserHead, ReviserLog, Space, Statement,
    TimeRange, Value, WriterLog,
};
use std::collections::BTreeSet;

const SECOND: u64 = 1_000_000_000;

fn warehouse() -> FrameId {
    FrameId::named("warehouse-3")
}

fn observe(
    memory: &mut Memory,
    kind: MemoryKind,
    subject: &str,
    value: Value,
    at: u64,
    bbox: Box3,
) -> MemoryAtom {
    let mut request = RememberRequest::new(
        kind,
        AtomClass::Observation,
        Statement::claim(subject, "at", value),
        Interval::closed(at, at + 30 * SECOND),
        at,
    );
    request.space = Some(Space {
        frame: warehouse(),
        bbox,
        uncertainty_mm: 50,
    });
    request.confidence = 900;
    let response = memory
        .remember(request, &mut sion_core::rng::seeded(at))
        .unwrap();
    memory.log().atom(response.seq).unwrap().clone()
}

fn commit(history: &mut CommitHistory, memory: &Memory, pulse: u64) {
    history
        .push(Commit {
            pulse: Pulse(pulse),
            size: memory.log().len(),
        })
        .unwrap();
}

#[test]
fn robots_share_a_warehouse() {
    let t0 = 1_790_812_800 * SECOND;
    let alpha = AccountId::derive(b"robot-alpha");
    let beta = AccountId::derive(b"robot-beta");
    let curator = AccountId::derive(b"curator");
    let mut a = Memory::in_memory(alpha);
    let mut b = Memory::in_memory(beta);
    let mut c = Memory::in_memory(curator);
    let (mut ha, mut hb, mut hc) = (
        CommitHistory::default(),
        CommitHistory::default(),
        CommitHistory::default(),
    );

    // Alpha sees pallet 17 in aisle 4; Beta later scans the same volume and sees nothing.
    let aisle = Box3::new([40_000, 10_000, 0], [42_000, 12_000, 3_000]);
    let pallet = observe(
        &mut a,
        MemoryKind::Observation,
        "pallet-17",
        Value::Entity("aisle-4".into()),
        t0,
        Box3::around([41_000, 11_000, 500], [600, 400, 500]),
    );
    let empty = observe(
        &mut b,
        MemoryKind::Coverage,
        "aisle-4",
        Value::Boolean(false),
        t0 + 10 * SECOND,
        aisle,
    );
    commit(&mut ha, &a, 100);
    commit(&mut hb, &b, 101);

    // Anyone can ask Alpha what it recorded in the aisle, and check it against Alpha's head.
    let head_a = a.head();
    let query = RangeQuery::new(warehouse(), aisle, TimeRange::new(t0, t0 + 60 * SECOND));
    let request = RecallRequest::Range(query.clone());
    let answer = a.recall(&request).unwrap();
    let atoms = Memory::verify(&head_a, &request, &answer).unwrap();
    assert_eq!(atoms, vec![pallet.clone()]);
    // And prove that Beta recorded no pallet observation there: the complete
    // answer holds only its coverage atom, which the kind filter drops.
    let index_b = b.log().index(b.log().len()).unwrap();
    let observations = query.clone().with_kinds([MemoryKind::Observation]);
    let (results, proof) = prove_range(&index_b, &observations).unwrap();
    verify_range(&b.head().index_root, &observations, &results, &proof).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, empty.id());
    assert!(
        select_atoms(&observations, &results, std::slice::from_ref(&empty))
            .unwrap()
            .is_empty()
    );

    // The curator records the disagreement as a conflict, then decides for Beta.
    let mut conflict = RememberRequest::new(
        MemoryKind::WorldState,
        AtomClass::Conflict,
        Statement::conflict([pallet.id(), empty.id()], ConflictKind::Existence),
        Interval::starting(t0 + 20 * SECOND),
        t0 + 20 * SECOND,
    );
    conflict.links.push(sion_memory::Link {
        kind: LinkKind::Supports,
        target: empty.id(),
        strength: 700,
    });
    c.remember(conflict, &mut sion_core::rng::seeded(1))
        .unwrap();
    commit(&mut hc, &c, 102);
    let both = BeliefPolicy::new([alpha, beta, curator]);
    let logs =
        |a: &Memory, b: &Memory, c: &Memory, ha, hb, hc| -> Vec<(WriterLog, CommitHistory)> {
            vec![
                (a.log().clone(), ha),
                (b.log().clone(), hb),
                (c.log().clone(), hc),
            ]
        };
    let snapshot = logs(&a, &b, &c, ha.clone(), hb.clone(), hc.clone());
    let revisers: Vec<ReviserLog<'_>> = snapshot
        .iter()
        .map(|(log, commits)| ReviserLog { log, commits })
        .collect();
    assert_eq!(
        belief_at(&pallet, &both, &revisers).unwrap().state,
        BeliefState::Disputed
    );

    c.revise(ReviseRequest {
        targets: vec![pallet.id()],
        state: BeliefState::Refuted,
        end_valid: None,
        reason: "beta's scan is newer and attested".into(),
        at: t0 + 40 * SECOND,
    })
    .unwrap();
    commit(&mut hc, &c, 110);
    let snapshot = logs(&a, &b, &c, ha.clone(), hb.clone(), hc.clone());
    let revisers: Vec<ReviserLog<'_>> = snapshot
        .iter()
        .map(|(log, commits)| ReviserLog { log, commits })
        .collect();

    // A consumer trusting the curator believes the pallet refuted; one trusting
    // only Alpha still believes it. The same evidence, two beliefs.
    let curated = BeliefPolicy::new([alpha, curator]);
    let alpha_only = BeliefPolicy::only(alpha);
    assert_eq!(
        belief_at(&pallet, &curated, &revisers).unwrap().state,
        BeliefState::Refuted
    );
    assert_eq!(
        belief_at(&pallet, &alpha_only, &revisers).unwrap().state,
        BeliefState::Accepted
    );

    // A light client checks the curated belief from heads alone.
    let heads: Vec<ReviserHead> = snapshot
        .iter()
        .map(|(log, commits)| ReviserHead {
            writer: log.writer(),
            head: log.head(commits.committed_size()).unwrap(),
            commits: commits.clone(),
        })
        .collect();
    let bundle = prove_belief(&pallet, &curated, &revisers).unwrap();
    assert_eq!(
        verify_belief(&pallet, &curated, &heads, &bundle)
            .unwrap()
            .state,
        BeliefState::Refuted
    );
    // As of Pulse 105 the refutation had not happened: the bundle from that
    // prefix proves no authorized retraction existed, only the dispute.
    let early: Vec<(WriterLog, CommitHistory)> = snapshot
        .iter()
        .map(|(log, commits)| (log.clone(), commits.as_of(Pulse(105))))
        .collect();
    let early_revisers: Vec<ReviserLog<'_>> = early
        .iter()
        .map(|(log, commits)| ReviserLog { log, commits })
        .collect();
    let early_heads: Vec<ReviserHead> = early
        .iter()
        .filter(|(_, commits)| !commits.commits().is_empty())
        .map(|(log, commits)| ReviserHead {
            writer: log.writer(),
            head: log.head(commits.committed_size()).unwrap(),
            commits: commits.clone(),
        })
        .collect();
    let early_bundle = prove_belief(&pallet, &curated, &early_revisers).unwrap();
    assert_eq!(
        verify_belief(&pallet, &curated, &early_heads, &early_bundle)
            .unwrap()
            .state,
        BeliefState::Disputed
    );

    // A curator's cell digest that drops Alpha's pallet is convicted.
    let cell = CellKey::containing(warehouse(), 0, [41_000, 11_000, 0]).unwrap();
    let honest = CuratedCell::build(cell, Pulse(110), &[(a.log(), 1), (b.log(), 1)]).unwrap();
    assert_eq!(honest.members().len(), 2);
    let mut members: BTreeSet<_> = honest.members().iter().copied().collect();
    members.remove(&(alpha, pallet.id()));
    let lazy = CuratedCell::from_members(cell, Pulse(110), honest.digest().heads.clone(), members)
        .unwrap();
    let proof = prove_cell_omission(&lazy, a.log(), 0)
        .unwrap()
        .expect("omission");
    verify_cell_omission(lazy.digest(), &proof).unwrap();
    assert!(verify_cell_omission(honest.digest(), &proof).is_err());

    // A robot whose committed index leaves an atom out is convicted too.
    let forged = MemoryIndex::forge_omitting(b.log().atoms(), &BTreeSet::from([0])).unwrap();
    let forged_head = sion_memory::Head {
        size: 1,
        root: b.log().root(),
        index_root: forged.root(),
    };
    let fraud = prove_omission(b.log(), 1, &forged, 0)
        .unwrap()
        .expect("omission");
    verify_omission(&forged_head, &fraud).unwrap();
    assert!(verify_omission(&b.head(), &fraud).is_err());
}

#[test]
fn camera_frames_are_sealed_shared_and_forgotten() {
    let alpha = AccountId::derive(b"robot-alpha");
    let mut a = Memory::in_memory(alpha);
    let mut rng = sion_core::rng::seeded(2026);
    let space = Hash32([3; 32]);
    let key = a.new_epoch_key(space, 0, &mut rng).unwrap();
    let mut frame = RememberRequest::new(
        MemoryKind::Observation,
        AtomClass::Observation,
        Statement::Content(Hash32([9; 32])),
        Interval::closed(0, 1),
        0,
    );
    frame.payload = PayloadRequest::Sealed {
        key,
        plaintext: b"jpeg bytes".to_vec(),
    };
    let stored = a.remember(frame, &mut rng).unwrap();
    let atom = a.log().atom(stored.seq).unwrap().clone();
    assert_eq!(a.open_payload(&atom).unwrap(), b"jpeg bytes");
    // Its id commits to the ciphertext: the log and index roots never change on forgetting.
    let before = a.head();
    a.forget(sion_memory::ForgetRequest { key, at: 5 }).unwrap();
    assert!(a.open_payload(&atom).is_err());
    let audit = a
        .audit(&sion_memory::AuditRequest {
            old_size: before.size,
            new_size: None,
        })
        .unwrap();
    Memory::verify_audit(&audit).unwrap();
    assert_eq!(audit.old_root, before.root);
}
