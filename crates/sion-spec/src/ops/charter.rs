//! Agent Charters: CreateCharter (§5.12), the charter checks (§5.13),
//! Delegate (§5.14), AgentPay (§5.15), RevokeCharter (§5.16) and
//! ReclaimCharter (§5.17); agent identities: RegisterAgent (§5.35),
//! SpawnAgents (§5.36), SetAgentCard (§5.37), SuspendAgent (§5.38) and
//! ResumeAgent (§5.39); cohorts and narrowing: SpawnCohort (§5.55),
//! MaterializeAgents (§5.56) and NarrowAgent (§5.57).
//!
//! `attenuates`, `narrows`, `refill`, `carve`, `spend`, the ancestry and
//! the cohort proofs are written from §5.13 and §5.55; this crate never
//! calls `Caveats::attenuates`, `Narrowing::attenuates`, `Allotment::spend`,
//! the `Bucket` methods, `Lineage::ancestry` or the `sion_exec::cohort`
//! helpers. It calls `lineage_id` and `overlay_id`, which only derive ids.

use std::collections::BTreeSet;

use sion_assurance::{decide, AtomClass, ClaimStatus};
use sion_core::encode::encode;
use sion_core::hash::{hash_domain, Hash32};
use sion_core::ids::Version;
use sion_core::ids::{AccountId, ClaimId, ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::charter::{
    lineage_id, overlay_id, AgentIdentity, AgentOverlay, Allotment, Bucket, Caveats, Charter,
    Lineage, Narrowing, Spawn, MAX_LINEAGE_DEPTH, MAX_SPAWN_BATCH,
};
use sion_exec::claim::{claim_id, formulas, ClaimBody, Payload};
use sion_exec::cohort::{Cohort, CohortMember, Materialize, SumNode, MAX_COHORT, MAX_MATERIALIZE};
use sion_exec::execute::{bond_atom, Deferred, Event};
use sion_exec::object::{Object, ObjectData, Owner};
use sion_exec::tx::object_domain;

use crate::authorize::keyless;
use crate::error::{fail, Abort, Step};
use crate::ops::assurance::load_assurance;
use crate::tx::{usd, TxRun};

/// Receipt claims are valid for at most this many Pulses.
const RECEIPT_VALIDITY: u64 = 604_800;
/// Receipt claims can be challenged for this many Pulses after their window.
const RECEIPT_CHALLENGE_PERIOD: u64 = 86_400;
/// The schema of agent payment receipts.
const RECEIPT_SCHEMA: &str = "receipt.agent.payment/v1";

/// Whether `child` is at least as strict as `parent` (§5.13): delegation
/// only narrows.
pub(crate) fn attenuates(child: &Caveats, parent: &Caveats) -> bool {
    let payees = match (&parent.payees, &child.payees) {
        (None, _) => true,
        (Some(parent), Some(child)) => child.is_subset(parent),
        (Some(_), None) => false,
    };
    // Where the parent requires evidence, the child judges it by the same
    // policy (§5.13, v1.14).
    let evidence = match (parent.evidence_above, child.evidence_above) {
        (None, _) => true,
        (Some(limit), Some(threshold)) => {
            threshold <= limit && child.evidence_policy == parent.evidence_policy
        }
        (Some(_), None) => false,
    };
    payees
        && evidence
        && child.max_per_payment <= parent.max_per_payment
        && child.burst <= parent.burst
        && child.rate <= parent.rate
        && child.expires_at <= parent.expires_at
        && (!parent.receipts || child.receipts)
        && (!child.may_delegate || parent.may_delegate)
}

/// **narrows(n, prev)** (§5.13): a narrowing that is at least as strict
/// as the one it replaces.
pub(crate) fn narrows(new: &Narrowing, previous: &Narrowing) -> bool {
    let payees = match (&previous.payees, &new.payees) {
        (None, _) => true,
        (Some(old), Some(new)) => new.is_subset(old),
        (Some(_), None) => false,
    };
    payees
        && new.max_per_payment <= previous.max_per_payment
        && new.expires_at <= previous.expires_at
        && (!new.may_delegate || previous.may_delegate)
}

/// **refill(b)** (§5.13).
pub(crate) fn refill(bucket: &mut Bucket, pulse: Pulse) {
    if pulse <= bucket.refilled_at {
        return;
    }
    let pulses = u128::from(pulse.0 - bucket.refilled_at.0);
    let topped = bucket
        .tokens
        .0
        .saturating_add(bucket.rate.0.saturating_mul(pulses));
    bucket.tokens = Amount(topped.min(bucket.burst.0));
    bucket.refilled_at = pulse;
}

/// **carve(b, burst, rate, take)** (§5.13).
pub(crate) fn carve(bucket: &mut Bucket, burst: Amount, rate: Amount, take: Amount) -> Step {
    if burst > bucket.burst {
        return Err(fail("burst exhausted"));
    }
    if rate > bucket.rate {
        return Err(fail("rate exhausted"));
    }
    if take > bucket.tokens {
        return Err(fail("bucket too low"));
    }
    bucket.burst = bucket.burst.checked_sub(burst)?;
    bucket.rate = bucket.rate.checked_sub(rate)?;
    let left = bucket.tokens.checked_sub(take)?;
    bucket.tokens = if left > bucket.burst {
        bucket.burst
    } else {
        left
    };
    Ok(())
}

/// **spend** (§5.13): checks a payment against the balance, the caveats
/// and the narrowings `found` above the agent, and records it, in the order
/// the specification gives.
pub(crate) fn spend(
    allotment: &mut Allotment,
    pulse: Pulse,
    payee: &AccountId,
    amount: Amount,
    evidence_ok: Option<bool>,
    found: &[Narrowing],
) -> Step {
    if amount > allotment.remaining {
        return Err(fail("insufficient allotment"));
    }
    if let Some(caveats) = &allotment.caveats {
        if pulse > caveats.expires_at {
            return Err(fail("charter caveat violated: charter expired"));
        }
        if let Some(payees) = &caveats.payees {
            if !payees.contains(payee) {
                return Err(fail("charter caveat violated: payee not allowed"));
            }
        }
        if amount > caveats.max_per_payment {
            return Err(fail("charter caveat violated: per-payment limit"));
        }
        for narrowing in found {
            if pulse > narrowing.expires_at {
                return Err(fail("narrowed: expired"));
            }
            if let Some(payees) = &narrowing.payees {
                if !payees.contains(payee) {
                    return Err(fail("narrowed: payee not allowed"));
                }
            }
            if amount > narrowing.max_per_payment {
                return Err(fail("narrowed: per-payment limit"));
            }
        }
        refill(&mut allotment.bucket, pulse);
        if amount > allotment.bucket.tokens {
            return Err(fail("charter caveat violated: bucket too low"));
        }
        if let Some(threshold) = caveats.evidence_above {
            if amount > threshold && evidence_ok != Some(true) {
                return Err(fail("charter caveat violated: evidence required"));
            }
        }
        allotment.bucket.tokens = allotment.bucket.tokens.checked_sub(amount)?;
    }
    allotment.remaining = allotment.remaining.checked_sub(amount)?;
    Ok(())
}

/// **ancestry(c)** (§5.13): the ids from the root down to the charter,
/// computed by hashing.
fn ancestry(lineage: &Lineage) -> Vec<ObjectId> {
    let mut ids = vec![lineage.root];
    for index in &lineage.path {
        let above = ids[ids.len() - 1];
        ids.push(lineage_id(&above, *index));
    }
    ids
}

/// What a passing standing check yields.
pub(crate) struct Standing {
    /// The charter's depth.
    pub(crate) depth: usize,
    /// The narrowings found, in order.
    pub(crate) narrowings: Vec<Narrowing>,
}

/// **standing(c)** (§5.13): reads the overlays of the charter and its
/// ancestors, from the charter up to the root, stopping at the first that
/// fails.
fn standing_with(
    run: &mut TxRun<'_>,
    id: ObjectId,
    charter: &Charter,
    ignoring_suspension: bool,
) -> Step<Standing> {
    let ids = ancestry(&charter.lineage);
    if ids.last() != Some(&id) {
        return Err(fail("the charter's lineage does not lead to it"));
    }
    let mut narrowings = Vec::new();
    for agent in ids.iter().rev() {
        let Some(object) = run.try_load(&overlay_id(agent))? else {
            continue;
        };
        let ObjectData::AgentOverlay(overlay) = &object.data else {
            return Err(fail(format!("{} is not an overlay", object.id)));
        };
        if overlay.revoked_at.is_some() {
            return Err(fail("charter revoked"));
        }
        if !ignoring_suspension && overlay.suspended_by.is_some() {
            return Err(fail("charter suspended"));
        }
        if let Some(narrowing) = &overlay.narrowing {
            narrowings.push(narrowing.clone());
        }
    }
    Ok(Standing {
        depth: charter.lineage.path.len(),
        narrowings,
    })
}

/// The standing check (§5.13), suspension included.
pub(super) fn standing(run: &mut TxRun<'_>, id: ObjectId, charter: &Charter) -> Step<Standing> {
    standing_with(run, id, charter, false)
}

/// **may_spawn(k, N)** (§5.13): spawning allowed by the caveats and every
/// narrowing found, none of which has ended.
fn may_spawn(caveats: &Caveats, found: &[Narrowing], pulse: Pulse) -> Step {
    if !caveats.may_delegate {
        return Err(fail("delegation not allowed"));
    }
    if found.iter().any(|n| !n.may_delegate) {
        return Err(fail("narrowed: delegation not allowed"));
    }
    if pulse > caveats.expires_at {
        return Err(fail("charter expired"));
    }
    if found.iter().any(|n| pulse > n.expires_at) {
        return Err(fail("narrowed: expired"));
    }
    Ok(())
}

/// **born(k)** (§5.13): a charter is not spawned already expired.
fn born(caveats: &Caveats, pulse: Pulse) -> Step {
    if caveats.expires_at <= pulse {
        return Err(fail("charter would already be expired"));
    }
    Ok(())
}

/// Loads `id`, which must be a charter with an identity record (§5.35).
pub(super) fn load_identity(run: &mut TxRun<'_>, id: ObjectId) -> Step<Charter> {
    let charter = as_charter(&run.load(&id)?)?;
    if charter.identity.is_none() {
        return Err(fail("the charter is not an agent identity"));
    }
    Ok(charter)
}

/// **position(c, s)** (§5.13): 0 for the backer, `j + 1` for the agent of
/// the highest charter above `charter`, `a_j`, that has `account` as its
/// agent (loading `a₀`, `a₁`, … and stopping at the first), or nothing.
fn position(run: &mut TxRun<'_>, charter: &Charter, account: AccountId) -> Step<Option<usize>> {
    if account == charter.backer {
        return Ok(Some(0));
    }
    let ids = ancestry(&charter.lineage);
    for (j, id) in ids.iter().take(ids.len() - 1).enumerate() {
        if j == MAX_LINEAGE_DEPTH {
            break;
        }
        let object = run.load(id)?;
        let ObjectData::Charter(above) = &object.data else {
            return Err(fail(format!("{id} above the charter is not a charter")));
        };
        if above.agent == account {
            return Ok(Some(j + 1));
        }
    }
    Ok(None)
}

/// **Oversight** (§5.13): the position of `sender` if it is an overseer of
/// `charter` in good standing: the backer, or the agent of `a_j` when no
/// overlay of `a₀ … a_j` is revoked or suspended.
fn oversight(run: &mut TxRun<'_>, charter: &Charter, sender: AccountId) -> Step<Option<usize>> {
    let Some(rank) = position(run, charter, sender)? else {
        return Ok(None);
    };
    let ids = ancestry(&charter.lineage);
    for id in ids.iter().take(rank) {
        if let Some(object) = run.try_load(&overlay_id(id))? {
            let ObjectData::AgentOverlay(overlay) = &object.data else {
                return Err(fail(format!("{} is not an overlay", object.id)));
            };
            if overlay.revoked_at.is_some() || overlay.suspended_by.is_some() {
                return Ok(None);
            }
        }
    }
    Ok(Some(rank))
}

/// The overlay of charter `id` (§5.16): the one it has, or a new one.
fn overlay_of(run: &mut TxRun<'_>, id: ObjectId) -> Step<AgentOverlay> {
    match run.try_load(&overlay_id(&id))? {
        None => Ok(AgentOverlay {
            agent: id,
            revoked_at: None,
            suspended_by: None,
            narrowing: None,
        }),
        Some(object) => match object.data {
            ObjectData::AgentOverlay(overlay) => Ok(overlay),
            _ => Err(fail(format!("{} is not an overlay", object.id))),
        },
    }
}

/// Stores charter `id`'s overlay (store assigns the version, §3.1).
fn put_overlay(run: &mut TxRun<'_>, id: ObjectId, overlay: AgentOverlay) -> Step {
    run.store(Object {
        id: overlay_id(&id),
        version: Version(0),
        owner: Owner::Shared(object_domain("charter", &id)),
        data: ObjectData::AgentOverlay(overlay),
    })
}

/// Splits a loaded object into its charter.
pub(super) fn as_charter(object: &Object) -> Step<Charter> {
    match &object.data {
        ObjectData::Charter(charter) => Ok(charter.clone()),
        _ => Err(fail(format!("{} is not a charter", object.id))),
    }
}

/// Splits a loaded object into its allotment.
pub(super) fn as_allotment(object: &Object) -> Step<Allotment> {
    match &object.data {
        ObjectData::Allotment(allotment) => Ok(allotment.clone()),
        _ => Err(fail(format!("{} is not an allotment", object.id))),
    }
}

/// The allotment a charter operation spends from: `tx.inputs[1]`.
pub(super) fn allotment_input(run: &TxRun<'_>) -> Step<ObjectId> {
    run.tx
        .inputs
        .get(1)
        .map(|input| input.id)
        .ok_or_else(|| fail("the allotment must be the second input"))
}

/// Loads a charter, inserts `allotment` into its allotments and stores it.
fn attach_allotment(run: &mut TxRun<'_>, charter: ObjectId, allotment: ObjectId) -> Step {
    let mut object = run.load(&charter)?;
    let ObjectData::Charter(state) = &mut object.data else {
        return Err(fail("the new charter is not a charter"));
    };
    state.allotments.insert(allotment);
    run.store(object)
}

/// A charter allotment's starting bucket: full (§5.12).
fn full_bucket(caveats: &Caveats, pulse: Pulse) -> Bucket {
    Bucket {
        tokens: caveats.burst,
        refilled_at: pulse,
        burst: caveats.burst,
        rate: caveats.rate,
    }
}

/// **create_at(id, owner, data)** (§3.1).
fn create_at(run: &mut TxRun<'_>, id: ObjectId, owner: Owner, data: ObjectData) -> Step {
    run.store(Object {
        id,
        version: Version(0),
        owner,
        data,
    })
}

/// The next lineage index range: `count` indices from `a.spawned` (§5.14,
/// §5.36, §5.55).
fn take_indices(allotment: &mut Allotment, count: u64) -> Step<u64> {
    let first = allotment.spawned;
    allotment.spawned = first
        .checked_add(count)
        .ok_or_else(|| fail("lineage index overflow"))?;
    Ok(first)
}

/// §5.12 steps 1 to 7, shared by CreateCharter and RegisterAgent (§5.35):
/// a root charter with `identity`, its allotment, the agent atom and its
/// bond. Returns the charter and the allotment.
fn root_charter(
    run: &mut TxRun<'_>,
    agent: AccountId,
    caveats: &Caveats,
    funding: Amount,
    agent_bond: Amount,
    identity: Option<AgentIdentity>,
) -> Step<(ObjectId, ObjectId)> {
    let pulse = run.ctx.pulse;
    if caveats.expires_at <= pulse {
        return Err(fail("the caveats have already expired"));
    }
    run.draw(usd(), funding.checked_add(agent_bond)?)?;
    let sender = run.sender();
    let charter_id = run.next_created_id();
    let agent_atom = bond_atom(
        AtomClass::Economic,
        &sender,
        &format!("charter:{charter_id}"),
    );
    let created = run.create(
        Owner::Shared(object_domain("charter", &charter_id)),
        ObjectData::Charter(Charter {
            principal: sender,
            agent,
            caveats: caveats.clone(),
            agent_atom: agent_atom.id,
            backer: sender,
            parent: None,
            lineage: Lineage {
                root: charter_id,
                path: Vec::new(),
            },
            allotments: BTreeSet::new(),
            created_at: pulse,
            identity,
        }),
    )?;
    debug_assert_eq!(created, charter_id);
    let allotment = run.create(
        Owner::Account(agent),
        ObjectData::Allotment(Allotment {
            source: charter_id,
            writer: agent,
            asset: usd(),
            remaining: funding,
            caveats: Some(caveats.clone()),
            bucket: full_bucket(caveats, pulse),
            spawned: 0,
        }),
    )?;
    attach_allotment(run, charter_id, allotment)?;
    run.defer(Deferred::RegisterAtom {
        atom: agent_atom.clone(),
    });
    if agent_bond > Amount::ZERO {
        run.defer(Deferred::PostBond {
            atom: agent_atom,
            owner: sender,
            amount: agent_bond,
        });
    }
    Ok((charter_id, allotment))
}

/// §5.12 CreateCharter: funds an agent's allotment under a new charter and
/// defers the agent atom and its bond.
pub(super) fn create_charter(
    run: &mut TxRun<'_>,
    agent: AccountId,
    caveats: &Caveats,
    funding: Amount,
    agent_bond: Amount,
) -> Step {
    let (charter, allotment) = root_charter(run, agent, caveats, funding, agent_bond, None)?;
    run.event(Event::CharterCreated { charter, allotment });
    Ok(())
}

/// §5.35 RegisterAgent: CreateCharter with an identity record of depth 0.
pub(super) fn register_agent(
    run: &mut TxRun<'_>,
    agent: AccountId,
    card: Hash32,
    caveats: &Caveats,
    funding: Amount,
    agent_bond: Amount,
) -> Step {
    let identity = AgentIdentity { card, depth: 0 };
    let (charter, allotment) =
        root_charter(run, agent, caveats, funding, agent_bond, Some(identity))?;
    run.event(Event::AgentRegistered {
        agent: charter,
        account: agent,
        allotment,
    });
    Ok(())
}

/// The part of a spawning operation's checks that §5.14, §5.36 and §5.55
/// share: the allotment input and the parent charter `charter_id`, its
/// standing, depth, ownership and permission to spawn. Returns the
/// allotment object, the allotment, the parent and its standing.
fn spawning_parent(
    run: &mut TxRun<'_>,
    charter_id: ObjectId,
) -> Step<(Object, Allotment, Charter, Standing)> {
    let sender = run.sender();
    let allotment_id = allotment_input(run)?;
    let allotment_object = run.load(&allotment_id)?;
    let allotment = as_allotment(&allotment_object)?;
    let parent = as_charter(&run.load(&charter_id)?)?;
    let standing = standing(run, charter_id, &parent)?;
    if standing.depth >= MAX_LINEAGE_DEPTH {
        return Err(fail("too deep: no room for another generation"));
    }
    if allotment.source != charter_id {
        return Err(fail("the allotment is not funded by this charter"));
    }
    if allotment.writer != sender || parent.agent != sender {
        return Err(fail("only the charter's agent may spawn"));
    }
    let Some(own) = &allotment.caveats else {
        return Err(fail("the allotment has no caveats"));
    };
    may_spawn(own, &standing.narrowings, run.ctx.pulse)?;
    Ok((allotment_object, allotment, parent, standing))
}

/// §5.14 Delegate: the agent moves part of its allotment to a sub-agent
/// under narrower caveats.
pub(super) fn delegate(
    run: &mut TxRun<'_>,
    charter_id: ObjectId,
    sub_agent: AccountId,
    caveats: &Caveats,
    amount: Amount,
) -> Step {
    let pulse = run.ctx.pulse;
    let sender = run.sender();
    let (mut allotment_object, mut allotment, parent, _) = spawning_parent(run, charter_id)?;
    let own = allotment
        .caveats
        .clone()
        .ok_or_else(|| fail("no caveats"))?;
    if !attenuates(caveats, &own) {
        return Err(fail("the delegated caveats must be at least as strict"));
    }
    born(caveats, pulse)?;
    refill(&mut allotment.bucket, pulse);
    carve(
        &mut allotment.bucket,
        caveats.burst,
        caveats.rate,
        caveats.burst,
    )?;
    allotment.remaining = allotment
        .remaining
        .checked_sub(amount)
        .map_err(|_| fail("insufficient allotment"))?;
    let index = take_indices(&mut allotment, 1)?;
    allotment_object.data = ObjectData::Allotment(allotment);
    run.store(allotment_object)?;
    let child_id = lineage_id(&charter_id, index);
    let mut path = parent.lineage.path.clone();
    path.push(index);
    create_at(
        run,
        child_id,
        Owner::Shared(object_domain("charter", &child_id)),
        ObjectData::Charter(Charter {
            principal: sender,
            agent: sub_agent,
            caveats: caveats.clone(),
            agent_atom: parent.agent_atom,
            backer: parent.backer,
            parent: Some(charter_id),
            lineage: Lineage {
                root: parent.lineage.root,
                path,
            },
            allotments: BTreeSet::new(),
            created_at: pulse,
            identity: None,
        }),
    )?;
    let child_allotment = run.create(
        Owner::Account(sub_agent),
        ObjectData::Allotment(Allotment {
            source: child_id,
            writer: sub_agent,
            asset: usd(),
            remaining: amount,
            caveats: Some(caveats.clone()),
            bucket: full_bucket(caveats, pulse),
            spawned: 0,
        }),
    )?;
    attach_allotment(run, child_id, child_allotment)?;
    run.event(Event::CharterCreated {
        charter: child_id,
        allotment: child_allotment,
    });
    Ok(())
}

/// A cohort member's node (§5.55).
fn member_node(member: &CohortMember) -> SumNode {
    SumNode {
        hash: hash_domain("sion/cohort/member", &encode(member)),
        burst: member.burst,
        rate: member.rate,
    }
}

/// The inner node over `left` and `right` (§5.55); `None` if a sum
/// overflows.
fn inner(left: &SumNode, right: &SumNode) -> Option<SumNode> {
    Some(SumNode {
        hash: hash_domain(
            "sion/cohort/node",
            &encode(&(
                left.hash,
                left.burst,
                left.rate,
                right.hash,
                right.burst,
                right.rate,
            )),
        ),
        burst: left.burst.checked_add(right.burst).ok()?,
        rate: left.rate.checked_add(right.rate).ok()?,
    })
}

/// The height of a cohort's tree (§5.55): ⌈log₂ count⌉.
fn tree_height(count: u32) -> usize {
    let mut height = 0usize;
    while (1u64 << height) < u64::from(count) {
        height += 1;
    }
    height
}

/// **verify(cohort, index, member, siblings)** (§5.55).
fn verify(cohort: &Cohort, index: u32, member: &CohortMember, siblings: &[SumNode]) -> bool {
    if index >= cohort.count || siblings.len() != tree_height(cohort.count) {
        return false;
    }
    let mut node = member_node(member);
    for (level, sibling) in siblings.iter().enumerate() {
        let right = (index >> level) & 1 == 1;
        let next = if right {
            inner(sibling, &node)
        } else {
            inner(&node, sibling)
        };
        let Some(next) = next else {
            return false;
        };
        node = next;
    }
    node.hash == cohort.root && node.burst == cohort.burst && node.rate == cohort.rate
}

/// The receipt claim of an agent payment (§5.15 step 4).
#[allow(clippy::too_many_arguments)]
fn receipt_body(
    charter_id: ObjectId,
    charter: &Charter,
    payee: AccountId,
    amount: Amount,
    purpose: &str,
    evidence: Option<ClaimId>,
    run: &TxRun<'_>,
) -> ClaimBody {
    let payload = encode(&(
        charter_id,
        payee,
        amount,
        purpose.to_owned(),
        evidence,
        run.digest,
    ));
    ClaimBody {
        schema: RECEIPT_SCHEMA.into(),
        subject: format!("agent paid {amount} USD for: {purpose}"),
        payload: Payload::Public(payload),
        formula: formulas::bonded_statement(charter.agent_atom),
        adjudicator: None,
        detection_permille: 1000,
        valid_for: RECEIPT_VALIDITY.min(run.ctx.config.max_claim_window),
        challenge_period: RECEIPT_CHALLENGE_PERIOD,
    }
}

/// §5.15 AgentPay: the agent pays from its allotment within the caveats
/// and the narrowings above it, optionally citing an evidence claim, and
/// emits a receipt claim.
pub(super) fn agent_pay(
    run: &mut TxRun<'_>,
    charter_id: ObjectId,
    payee: AccountId,
    amount: Amount,
    purpose: &str,
    evidence: Option<ClaimId>,
) -> Step {
    let sender = run.sender();
    let allotment_id = allotment_input(run)?;
    let mut allotment_object = run.load(&allotment_id)?;
    let mut allotment = as_allotment(&allotment_object)?;
    let charter = as_charter(&run.load(&charter_id)?)?;
    let found = standing(run, charter_id, &charter)?.narrowings;
    if allotment.source != charter_id {
        return Err(fail("the allotment is not funded by this charter"));
    }
    if allotment.writer != sender {
        return Err(fail("only the allotment's writer may pay from it"));
    }
    if charter.agent != sender {
        return Err(fail("only the charter's agent may pay"));
    }
    let caveats = allotment
        .caveats
        .clone()
        .ok_or_else(|| fail("the allotment has no caveats"))?;

    let evidence_ok = match (evidence, caveats.evidence_above) {
        (Some(claim), Some(threshold)) if amount > threshold => {
            let loaded = load_assurance(run)?;
            let registry = &loaded.domain.registry;
            let passes = match registry.claim(&claim) {
                Some(record) if record.status == ClaimStatus::Active => {
                    decide(registry, &caveats.evidence_policy, &claim, amount, false)?.accept
                }
                _ => false,
            };
            Some(passes)
        }
        _ => None,
    };

    spend(
        &mut allotment,
        run.ctx.pulse,
        &payee,
        amount,
        evidence_ok,
        &found,
    )?;
    allotment_object.data = ObjectData::Allotment(allotment);
    run.store(allotment_object)?;
    run.mint(payee, usd(), amount)?;

    let receipt = if caveats.receipts {
        let body = receipt_body(charter_id, &charter, payee, amount, purpose, evidence, run);
        let id = claim_id(&sender, &body);
        run.defer(Deferred::IssueClaim {
            issuer: sender,
            backer: charter.backer,
            body,
        });
        Some(id)
    } else {
        None
    };
    run.event(Event::AgentPaid {
        charter: charter_id,
        payee,
        amount,
        receipt,
    });
    Ok(())
}

/// §5.16 RevokeCharter: the agent itself or an overseer in good standing
/// revokes a charter in its overlay (the first revocation's Pulse is kept).
pub(super) fn revoke_charter(run: &mut TxRun<'_>, charter_id: ObjectId) -> Step {
    let charter = as_charter(&run.load(&charter_id)?)?;
    let sender = run.sender();
    if charter.agent != sender && oversight(run, &charter, sender)?.is_none() {
        return Err(fail(
            "only the agent or an overseer in good standing (the backer, an ancestor's agent) may revoke",
        ));
    }
    let mut overlay = overlay_of(run, charter_id)?;
    if overlay.revoked_at.is_none() {
        overlay.revoked_at = Some(run.ctx.pulse);
    }
    put_overlay(run, charter_id, overlay)?;
    run.event(Event::CharterRevoked {
        charter: charter_id,
    });
    Ok(())
}

/// §5.17 ReclaimCharter: returns what is left in a dead charter's
/// allotments to its backer.
pub(super) fn reclaim_charter(run: &mut TxRun<'_>, charter_id: ObjectId) -> Step {
    let charter = as_charter(&run.load(&charter_id)?)?;
    let sender = run.sender();
    if sender != charter.principal && sender != charter.backer {
        return Err(fail("only the principal or the backer may reclaim"));
    }
    let dead = run.ctx.pulse > charter.caveats.expires_at
        || match standing_with(run, charter_id, &charter, true) {
            Ok(_) => false,
            Err(Abort::Op(_)) => true,
            Err(chunk) => return Err(chunk),
        };
    if !dead {
        return Err(fail("the charter is still live"));
    }
    let mut total = Amount::ZERO;
    for id in &charter.allotments {
        let object = run.load(id)?;
        if let ObjectData::Allotment(allotment) = &object.data {
            total = total.checked_add(allotment.remaining)?;
            run.consume(id)?;
        }
    }
    run.mint(charter.backer, usd(), total)?;
    run.event(Event::CharterReclaimed {
        charter: charter_id,
        amount: total,
    });
    Ok(())
}

/// §5.36 step 8 and §5.56 step 8: a spawned charter at `child_id` under
/// `parent` (id `parent_id`) with lineage index `index`, its allotment, and
/// the allotment attached. Returns the account and the allotment.
#[allow(clippy::too_many_arguments)]
fn new_child(
    run: &mut TxRun<'_>,
    parent_id: ObjectId,
    parent: &Charter,
    child_id: ObjectId,
    index: u64,
    agent: Option<AccountId>,
    card: Hash32,
    caveats: Caveats,
    budget: Amount,
    bucket: Bucket,
    depth: usize,
) -> Step<(AccountId, ObjectId)> {
    let pulse = run.ctx.pulse;
    let sender = run.sender();
    let account = match agent {
        Some(agent) => agent,
        None => keyless(&child_id),
    };
    let mut path = parent.lineage.path.clone();
    path.push(index);
    let depth = u8::try_from(depth).map_err(|_| fail("too deep"))?;
    create_at(
        run,
        child_id,
        Owner::Shared(object_domain("charter", &child_id)),
        ObjectData::Charter(Charter {
            principal: sender,
            agent: account,
            caveats: caveats.clone(),
            agent_atom: parent.agent_atom,
            backer: parent.backer,
            parent: Some(parent_id),
            lineage: Lineage {
                root: parent.lineage.root,
                path,
            },
            allotments: BTreeSet::new(),
            created_at: pulse,
            identity: Some(AgentIdentity { card, depth }),
        }),
    )?;
    let allotment = run.create(
        Owner::Account(account),
        ObjectData::Allotment(Allotment {
            source: child_id,
            writer: account,
            asset: usd(),
            remaining: budget,
            caveats: Some(caveats),
            bucket,
            spawned: 0,
        }),
    )?;
    attach_allotment(run, child_id, allotment)?;
    Ok((account, allotment))
}

/// §5.36 SpawnAgents: the agent launches subagents, each with its own
/// identity, account, attenuated caveats, a budget carved from its
/// allotment and a bucket carved from its bucket.
pub(super) fn spawn_agents(run: &mut TxRun<'_>, charter_id: ObjectId, children: &[Spawn]) -> Step {
    // Step 1.
    if children.is_empty() || children.len() > MAX_SPAWN_BATCH {
        return Err(fail(format!(
            "a spawn launches between 1 and {MAX_SPAWN_BATCH} subagents"
        )));
    }
    let pulse = run.ctx.pulse;
    // Steps 2 to 5.
    let (mut allotment_object, mut allotment, parent, standing) = spawning_parent(run, charter_id)?;
    let own = allotment
        .caveats
        .clone()
        .ok_or_else(|| fail("no caveats"))?;
    // Step 6.
    refill(&mut allotment.bucket, pulse);
    for child in children {
        if !attenuates(&child.caveats, &own) {
            return Err(fail("a subagent's caveats must be at least as strict"));
        }
        born(&child.caveats, pulse)?;
        carve(
            &mut allotment.bucket,
            child.caveats.burst,
            child.caveats.rate,
            child.caveats.burst,
        )?;
        allotment.remaining = allotment
            .remaining
            .checked_sub(child.budget)
            .map_err(|_| fail("insufficient allotment for the budgets"))?;
    }
    // Step 7.
    let first = take_indices(&mut allotment, children.len() as u64)?;
    allotment_object.data = ObjectData::Allotment(allotment);
    run.store(allotment_object)?;
    // Step 8.
    for (j, child) in children.iter().enumerate() {
        let index = first + j as u64;
        let child_id = lineage_id(&charter_id, index);
        let (account, child_allotment) = new_child(
            run,
            charter_id,
            &parent,
            child_id,
            index,
            child.agent,
            child.card,
            child.caveats.clone(),
            child.budget,
            full_bucket(&child.caveats, pulse),
            standing.depth + 1,
        )?;
        run.event(Event::AgentSpawned {
            agent: child_id,
            parent: charter_id,
            account,
            allotment: child_allotment,
        });
    }
    Ok(())
}

/// §5.37 SetAgentCard: the agent replaces its card commitment.
pub(super) fn set_agent_card(run: &mut TxRun<'_>, charter_id: ObjectId, card: Hash32) -> Step {
    let mut object = run.load(&charter_id)?;
    let charter = as_charter(&object)?;
    if charter.identity.is_none() {
        return Err(fail("the charter is not an agent identity"));
    }
    if charter.agent != run.sender() {
        return Err(fail("only the agent may set its card"));
    }
    standing(run, charter_id, &charter)?;
    if let ObjectData::Charter(state) = &mut object.data {
        if let Some(identity) = &mut state.identity {
            identity.card = card;
        }
    }
    run.store(object)?;
    run.event(Event::AgentCardSet {
        agent: charter_id,
        card,
    });
    Ok(())
}

/// §5.38 SuspendAgent: an overseer in good standing suspends a charter in
/// its overlay; the highest suspender is kept.
pub(super) fn suspend_agent(run: &mut TxRun<'_>, charter_id: ObjectId) -> Step {
    let charter = as_charter(&run.load(&charter_id)?)?;
    let sender = run.sender();
    let Some(rank) = oversight(run, &charter, sender)? else {
        return Err(fail(
            "only an overseer in good standing (the backer, an ancestor's agent) may suspend",
        ));
    };
    let mut overlay = overlay_of(run, charter_id)?;
    let by = match overlay.suspended_by {
        None => sender,
        Some(previous) => {
            let higher =
                matches!(position(run, &charter, previous)?, Some(theirs) if rank < theirs);
            if higher {
                sender
            } else {
                previous
            }
        }
    };
    overlay.suspended_by = Some(by);
    put_overlay(run, charter_id, overlay)?;
    run.event(Event::AgentSuspended {
        agent: charter_id,
        by,
    });
    Ok(())
}

/// §5.39 ResumeAgent: the suspender, in good standing, or the backer lifts
/// a suspension.
pub(super) fn resume_agent(run: &mut TxRun<'_>, charter_id: ObjectId) -> Step {
    let charter = as_charter(&run.load(&charter_id)?)?;
    let Some(object) = run.try_load(&overlay_id(&charter_id))? else {
        return Err(fail("not suspended"));
    };
    let ObjectData::AgentOverlay(mut overlay) = object.data else {
        return Err(fail("the overlay id holds something else"));
    };
    let Some(suspender) = overlay.suspended_by else {
        return Err(fail("not suspended"));
    };
    let sender = run.sender();
    if sender != suspender && sender != charter.backer {
        return Err(fail("only the suspender or the backer may resume"));
    }
    if oversight(run, &charter, sender)?.is_none() {
        return Err(fail("the suspender is no longer in good standing"));
    }
    overlay.suspended_by = None;
    put_overlay(run, charter_id, overlay)?;
    run.event(Event::AgentResumed { agent: charter_id });
    Ok(())
}

/// §5.55 SpawnCohort: commits to up to 2²⁰ subagents by the root of their
/// sum tree, carving the totals from the parent's bucket.
#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_cohort(
    run: &mut TxRun<'_>,
    charter_id: ObjectId,
    root: Hash32,
    count: u32,
    burst: Amount,
    rate: Amount,
    full: bool,
    caveats: &Caveats,
) -> Step {
    // Step 1.
    if count == 0 || count > MAX_COHORT {
        return Err(fail(format!("a cohort has 1 to {MAX_COHORT} members")));
    }
    let pulse = run.ctx.pulse;
    // Steps 2 to 5.
    let (mut allotment_object, mut allotment, _, _) = spawning_parent(run, charter_id)?;
    let own = allotment
        .caveats
        .clone()
        .ok_or_else(|| fail("no caveats"))?;
    if !attenuates(caveats, &own) {
        return Err(fail("the cohort's caveats must be at least as strict"));
    }
    born(caveats, pulse)?;
    // Step 6.
    refill(&mut allotment.bucket, pulse);
    let take = if full { burst } else { Amount::ZERO };
    carve(&mut allotment.bucket, burst, rate, take)?;
    // Step 7.
    let first = take_indices(&mut allotment, u64::from(count))?;
    allotment_object.data = ObjectData::Allotment(allotment);
    run.store(allotment_object)?;
    // Steps 8 and 9.
    let cohort = run.next_created_id();
    run.create(
        Owner::Shared(object_domain("cohort", &cohort)),
        ObjectData::Cohort(Box::new(Cohort {
            parent: charter_id,
            first,
            count,
            root,
            burst,
            rate,
            full,
            caveats: caveats.clone(),
            created_at: pulse,
        })),
    )?;
    run.event(Event::CohortSpawned {
        cohort,
        parent: charter_id,
        first,
        count,
    });
    Ok(())
}

/// §5.56 MaterializeAgents: creates cohort members at their lineage ids,
/// each with a proof of its leaf, its budget from the parent's allotment.
pub(super) fn materialize_agents(
    run: &mut TxRun<'_>,
    cohort_id: ObjectId,
    members: &[Materialize],
) -> Step {
    // Step 1.
    if members.is_empty() || members.len() > MAX_MATERIALIZE {
        return Err(fail(format!(
            "between 1 and {MAX_MATERIALIZE} members per transaction"
        )));
    }
    for pair in members.windows(2) {
        if pair[1].index <= pair[0].index {
            return Err(fail("members must come in increasing order"));
        }
    }
    let pulse = run.ctx.pulse;
    let sender = run.sender();
    // Step 2.
    let allotment_id = allotment_input(run)?;
    let mut allotment_object = run.load(&allotment_id)?;
    let mut allotment = as_allotment(&allotment_object)?;
    // Step 3.
    let cohort_object = run.load(&cohort_id)?;
    let ObjectData::Cohort(cohort) = &cohort_object.data else {
        return Err(fail(format!("{cohort_id} is not a cohort")));
    };
    let cohort = (**cohort).clone();
    let parent = as_charter(&run.load(&cohort.parent)?)?;
    let found = standing(run, cohort.parent, &parent)?;
    // Step 4.
    if allotment.source != cohort.parent {
        return Err(fail("the allotment is not the cohort parent's"));
    }
    if allotment.writer != sender || parent.agent != sender {
        return Err(fail("only the cohort parent's agent may materialize"));
    }
    // Step 5.
    let Some(own) = &allotment.caveats else {
        return Err(fail("the allotment has no caveats"));
    };
    may_spawn(own, &found.narrowings, pulse)?;
    born(&cohort.caveats, pulse)?;
    // Step 6.
    let mut ids = Vec::with_capacity(members.len());
    for m in members {
        if !verify(&cohort, m.index, &m.member, &m.siblings) {
            return Err(fail(format!("member {} is not in the cohort", m.index)));
        }
        if m.member.burst > cohort.caveats.burst || m.member.rate > cohort.caveats.rate {
            return Err(fail("a member's bucket exceeds the cohort's caveats"));
        }
        let index = cohort
            .first
            .checked_add(u64::from(m.index))
            .ok_or_else(|| fail("lineage index overflow"))?;
        let id = lineage_id(&cohort.parent, index);
        if run.try_load(&id)?.is_some() {
            return Err(fail(format!("member {} is already materialized", m.index)));
        }
        allotment.remaining = allotment
            .remaining
            .checked_sub(m.member.budget)
            .map_err(|_| fail("insufficient allotment for the budgets"))?;
        ids.push((id, index));
    }
    // Step 7.
    allotment_object.data = ObjectData::Allotment(allotment);
    run.store(allotment_object)?;
    // Step 8.
    let elapsed = u128::from(pulse.0.saturating_sub(cohort.created_at.0));
    for (m, (id, index)) in members.iter().zip(ids) {
        let mut caveats = cohort.caveats.clone();
        caveats.burst = m.member.burst;
        caveats.rate = m.member.rate;
        let start = if cohort.full { m.member.burst.0 } else { 0 };
        let filled = start.saturating_add(m.member.rate.0.saturating_mul(elapsed));
        let bucket = Bucket {
            tokens: Amount(filled.min(m.member.burst.0)),
            refilled_at: pulse,
            burst: m.member.burst,
            rate: m.member.rate,
        };
        let (account, member_allotment) = new_child(
            run,
            cohort.parent,
            &parent,
            id,
            index,
            m.member.agent,
            m.member.card,
            caveats,
            m.member.budget,
            bucket,
            found.depth + 1,
        )?;
        run.event(Event::AgentMaterialized {
            agent: id,
            cohort: cohort_id,
            index: m.index,
            account,
            allotment: member_allotment,
        });
    }
    Ok(())
}

/// §5.57 NarrowAgent: the agent or an overseer adds limits to the agent's
/// whole subtree; only the backer widens or lifts them.
pub(super) fn narrow_agent(
    run: &mut TxRun<'_>,
    charter_id: ObjectId,
    narrowing: &Option<Narrowing>,
) -> Step {
    let charter = as_charter(&run.load(&charter_id)?)?;
    let sender = run.sender();
    if charter.agent != sender && oversight(run, &charter, sender)?.is_none() {
        return Err(fail(
            "only the agent or an overseer in good standing (the backer, an ancestor's agent) may narrow",
        ));
    }
    let mut overlay = overlay_of(run, charter_id)?;
    if sender != charter.backer {
        let allowed = match (narrowing, &overlay.narrowing) {
            (None, _) => false,
            (Some(_), None) => true,
            (Some(new), Some(previous)) => narrows(new, previous),
        };
        if !allowed {
            return Err(fail("only the backer may widen or lift a narrowing"));
        }
    }
    overlay.narrowing = narrowing.clone();
    put_overlay(run, charter_id, overlay)?;
    run.event(Event::AgentNarrowed {
        agent: charter_id,
        by: sender,
        narrowing: narrowing.clone(),
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sion_assurance::Policy;
    use sion_exec::cohort::CohortTree;

    fn account(tag: &[u8]) -> AccountId {
        AccountId::derive(tag)
    }

    fn caveats() -> Caveats {
        Caveats {
            payees: Some([account(b"v1"), account(b"v2")].into_iter().collect()),
            max_per_payment: Amount(100),
            burst: Amount(250),
            rate: Amount(10),
            expires_at: Pulse(1_000),
            evidence_above: Some(Amount(80)),
            evidence_policy: Policy::standard(),
            receipts: true,
            may_delegate: true,
        }
    }

    fn allotment(caveats: Option<Caveats>) -> Allotment {
        let bucket = match &caveats {
            Some(k) => full_bucket(k, Pulse(0)),
            None => Bucket {
                tokens: Amount::ZERO,
                refilled_at: Pulse(0),
                burst: Amount::ZERO,
                rate: Amount::ZERO,
            },
        };
        Allotment {
            source: ObjectId::derive(b"charter"),
            writer: account(b"agent"),
            asset: usd(),
            remaining: Amount(1_000),
            caveats,
            bucket,
            spawned: 0,
        }
    }

    #[test]
    fn attenuation_only_narrows() {
        let parent = caveats();
        assert!(attenuates(&parent, &parent));
        let mut child = parent.clone();
        child.payees = Some([account(b"v1")].into_iter().collect());
        child.max_per_payment = Amount(50);
        child.burst = Amount(20);
        child.rate = Amount(1);
        child.receipts = true;
        child.may_delegate = false;
        assert!(attenuates(&child, &parent));
        // Any widening fails.
        let mut wide = parent.clone();
        wide.payees = None;
        assert!(!attenuates(&wide, &parent));
        let mut wide = parent.clone();
        wide.evidence_above = None;
        assert!(!attenuates(&wide, &parent));
        let mut wide = parent.clone();
        wide.evidence_above = Some(Amount(81));
        assert!(!attenuates(&wide, &parent));
        // A laxer policy for the evidence the parent requires fails.
        let mut lax = parent.clone();
        lax.evidence_policy
            .trusted_classes
            .insert(AtomClass::Economic);
        assert!(!attenuates(&lax, &parent));
        // Where the parent requires none, the child may add a requirement
        // under any policy.
        let mut open = parent.clone();
        open.evidence_above = None;
        let mut strict = open.clone();
        strict.evidence_above = Some(Amount(5));
        strict
            .evidence_policy
            .trusted_classes
            .insert(AtomClass::Economic);
        assert!(attenuates(&strict, &open));
        let mut wide = parent.clone();
        wide.burst = Amount(251);
        assert!(!attenuates(&wide, &parent));
        let mut wide = parent.clone();
        wide.rate = Amount(11);
        assert!(!attenuates(&wide, &parent));
        let mut wide = parent.clone();
        wide.expires_at = Pulse(1_001);
        assert!(!attenuates(&wide, &parent));
        let mut wide = parent.clone();
        wide.receipts = false;
        assert!(!attenuates(&wide, &parent));
        let mut no_delegation = parent.clone();
        no_delegation.may_delegate = false;
        let mut child = parent.clone();
        child.may_delegate = true;
        assert!(!attenuates(&child, &no_delegation));
        // A parent without a payee list or threshold allows anything there.
        let mut open = parent.clone();
        open.payees = None;
        open.evidence_above = None;
        let mut child = parent;
        child.payees = None;
        assert!(attenuates(&child, &open));
    }

    #[test]
    fn spend_checks_caveats_in_order() {
        let v1 = account(b"v1");
        let mut a = allotment(Some(caveats()));
        spend(&mut a, Pulse(1), &v1, Amount(70), None, &[]).unwrap();
        assert_eq!(a.remaining, Amount(930));
        // The bucket refilled 10 at Pulse 1 (up to its 250), then gave 70.
        assert_eq!(a.bucket.tokens, Amount(180));
        assert!(spend(&mut a, Pulse(1), &account(b"x"), Amount(1), None, &[]).is_err());
        assert!(spend(&mut a, Pulse(1), &v1, Amount(101), None, &[]).is_err());
        assert!(
            spend(&mut a, Pulse(1), &v1, Amount(90), None, &[]).is_err(),
            "needs evidence"
        );
        spend(&mut a, Pulse(1), &v1, Amount(90), Some(true), &[]).unwrap();
        spend(&mut a, Pulse(2), &v1, Amount(80), None, &[]).unwrap();
        // 180 − 90 + 10 − 80 = 20 left; 30 is too much a Pulse later.
        assert!(
            spend(&mut a, Pulse(3), &v1, Amount(31), None, &[]).is_err(),
            "the bucket holds 30"
        );
        spend(&mut a, Pulse(3), &v1, Amount(30), None, &[]).unwrap();
        // Twenty-five Pulses refill all of it, and no more.
        spend(&mut a, Pulse(28), &v1, Amount(80), None, &[]).unwrap();
        assert_eq!(a.bucket.tokens, Amount(170));
        assert!(
            spend(&mut a, Pulse(1_001), &v1, Amount(1), None, &[]).is_err(),
            "expired"
        );
        assert!(spend(&mut a, Pulse(28), &v1, Amount(10_000), None, &[]).is_err());
    }

    #[test]
    fn spend_without_caveats_checks_only_the_balance() {
        let mut a = allotment(None);
        spend(
            &mut a,
            Pulse(5),
            &account(b"anyone"),
            Amount(1_000),
            None,
            &[],
        )
        .unwrap();
        assert_eq!(a.remaining, Amount::ZERO);
        assert!(spend(&mut a, Pulse(5), &account(b"anyone"), Amount(1), None, &[]).is_err());
    }

    #[test]
    fn narrowings_bind_every_payment_below_them() {
        let v1 = account(b"v1");
        let n = Narrowing {
            payees: Some([v1].into_iter().collect()),
            max_per_payment: Amount(10),
            expires_at: Pulse(50),
            may_delegate: false,
        };
        let mut a = allotment(Some(caveats()));
        let found = std::slice::from_ref(&n);
        spend(&mut a, Pulse(1), &v1, Amount(10), None, found).unwrap();
        assert!(spend(&mut a, Pulse(1), &v1, Amount(11), None, found).is_err());
        assert!(spend(&mut a, Pulse(1), &account(b"v2"), Amount(1), None, found).is_err());
        assert!(spend(&mut a, Pulse(51), &v1, Amount(1), None, found).is_err());
        let mut looser = n.clone();
        looser.max_per_payment = Amount(11);
        assert!(!narrows(&looser, &n));
        assert!(narrows(&n, &looser));
        let mut open = n.clone();
        open.payees = None;
        assert!(!narrows(&open, &n));
        assert!(may_spawn(&caveats(), std::slice::from_ref(&n), Pulse(1)).is_err());
        assert!(may_spawn(&caveats(), &[], Pulse(1)).is_ok());
        // Nothing spawns once the charter, or a narrowing above it, ends.
        let mut spawning = n.clone();
        spawning.may_delegate = true;
        let found = std::slice::from_ref(&spawning);
        assert!(may_spawn(&caveats(), found, Pulse(50)).is_ok());
        assert!(may_spawn(&caveats(), found, Pulse(51)).is_err());
        assert!(may_spawn(&caveats(), &[], Pulse(1_001)).is_err());
        assert!(born(&caveats(), Pulse(999)).is_ok());
        assert!(born(&caveats(), Pulse(1_000)).is_err());
    }

    #[test]
    fn buckets_refill_and_carve_as_specified() {
        let mut b = Bucket {
            tokens: Amount(4),
            refilled_at: Pulse(10),
            burst: Amount(100),
            rate: Amount(7),
        };
        refill(&mut b, Pulse(9));
        assert_eq!((b.tokens, b.refilled_at), (Amount(4), Pulse(10)));
        refill(&mut b, Pulse(13));
        assert_eq!((b.tokens, b.refilled_at), (Amount(25), Pulse(13)));
        refill(&mut b, Pulse(u64::MAX));
        assert_eq!(b.tokens, Amount(100));
        assert!(carve(&mut b, Amount(101), Amount(0), Amount(0)).is_err());
        assert!(carve(&mut b, Amount(1), Amount(8), Amount(0)).is_err());
        carve(&mut b, Amount(30), Amount(2), Amount(30)).unwrap();
        assert_eq!(
            (b.burst, b.rate, b.tokens),
            (Amount(70), Amount(5), Amount(70))
        );
        // Taking no tokens still leaves the parent no more than it keeps.
        carve(&mut b, Amount(60), Amount(0), Amount(0)).unwrap();
        assert_eq!((b.burst, b.tokens), (Amount(10), Amount(10)));
        assert!(carve(&mut b, Amount(5), Amount(0), Amount(11)).is_err());
    }

    #[test]
    fn ancestry_follows_the_lineage() {
        let root = ObjectId::derive(b"root");
        let lineage = Lineage {
            root,
            path: vec![4, 0, 9],
        };
        let ids = ancestry(&lineage);
        assert_eq!(ids.len(), 4);
        assert_eq!(ids[0], root);
        assert_eq!(ids[3], lineage_id(&lineage_id(&lineage_id(&root, 4), 0), 9));
    }

    #[test]
    fn cohort_proofs_are_checked_from_the_specification() {
        // The tree is built by sion-exec's client helper; the check is this
        // crate's own (§5.55).
        let members: Vec<CohortMember> = (0..5u64)
            .map(|n| CohortMember {
                agent: None,
                card: Hash32::ZERO,
                budget: Amount(u128::from(n)),
                burst: Amount(u128::from(n) + 1),
                rate: Amount(2 * u128::from(n)),
            })
            .collect();
        let tree = CohortTree::build(&members).unwrap();
        let root = tree.root();
        let cohort = Cohort {
            parent: ObjectId::derive(b"parent"),
            first: 3,
            count: 5,
            root: root.hash,
            burst: root.burst,
            rate: root.rate,
            full: false,
            caveats: caveats(),
            created_at: Pulse(1),
        };
        assert_eq!(tree_height(5), 3);
        assert_eq!(tree_height(1), 0);
        for (i, m) in members.iter().enumerate() {
            let proof = tree.prove(i as u32).unwrap();
            assert!(verify(&cohort, i as u32, m, &proof));
            assert!(!verify(&cohort, (i as u32 + 1) % 5, m, &proof));
        }
        let mut lying = cohort.clone();
        lying.burst = Amount(root.burst.0 + 1);
        assert!(!verify(&lying, 0, &members[0], &tree.prove(0).unwrap()));
        assert!(!verify(&cohort, 5, &members[0], &tree.prove(0).unwrap()));
    }

    #[test]
    fn the_lineage_limit_is_the_specified_eight() {
        assert_eq!(MAX_LINEAGE_DEPTH, 8);
        assert_eq!(MAX_COHORT, 1 << 20);
        assert_eq!(MAX_MATERIALIZE, 128);
    }
}
