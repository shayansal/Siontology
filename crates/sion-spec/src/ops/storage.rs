//! Storage deals: PostDeal (§5.50), AcceptDeal (§5.51), ProveStorage
//! (§5.52), SettleDeal (§5.53) and CloseDeal (§5.54).
//!
//! Written from the specification. A round's seed and challenges are the
//! formulas of §5.52 (the seed from the beacon of the round's first Pulse,
//! read from its beacon record, §6.3), computed with the shared hashing
//! primitive (`sion_core::hash::hash_parts`); a challenged leaf is checked
//! with the shared Merkle primitive (`sion_core::merkle::verify_inclusion`,
//! RFC 9162 shape); rounds, a deal's end and what closing pays are computed
//! here. This crate never calls the `sion_exec::storage` or
//! `sion_exec::beacons` helpers: only the types and the §5.50 constants are
//! shared.
//!
//! Reads follow the specification's steps (§2.1): SettleDeal checks the
//! sender before it loads the custody record, CloseDeal loads it first, and
//! ProveStorage loads each custody record only once the proofs before it
//! have passed.

use sion_core::hash::{hash_parts, Hash32};
use sion_core::ids::{AccountId, ObjectId, Pulse};
use sion_core::{merkle, Amount};
use sion_exec::beacons::{BEACON_INTERVAL, BEACON_SLOTS};
use sion_exec::execute::{Deferred, Event};
use sion_exec::market::MAX_PERIOD;
use sion_exec::object::{well_known, Object, ObjectData, Owner};
use sion_exec::storage::{
    Custody, CustodyProof, Deal, DealState, ABANDON_ROUNDS, CHALLENGE_DOMAIN, MAX_CHALLENGES,
    MAX_DEAL_LEAVES, MAX_LEAF_BYTES, MAX_LOCATOR, MAX_PATH, MAX_PROOF_LEAVES, MAX_ROUNDS,
    MAX_ROUND_LENGTH, MIN_ROUND_LENGTH, SEED_DOMAIN,
};
use sion_exec::tx::object_domain;

use crate::error::{fail, Step};
use crate::tx::{usd, TxRun};

/// The terms of §5.50.
pub(super) struct Terms<'a> {
    pub(super) root: Hash32,
    pub(super) size: u64,
    pub(super) challenges: u8,
    pub(super) price: Amount,
    pub(super) rounds: u32,
    pub(super) round_length: u64,
    pub(super) collateral: Amount,
    pub(super) provider: Option<AccountId>,
    pub(super) accept_by: Pulse,
    pub(super) locator: &'a str,
}

/// `count × price`, or `None` if it overflows.
fn times(count: u32, price: Amount) -> Option<Amount> {
    price.0.checked_mul(u128::from(count)).map(Amount)
}

/// A round's seed (§5.52): `hash_parts(SEED_DOMAIN, [beacon_at(b), c.deal])`,
/// each part its 32 bytes, where `beacon` is `beacon_at(b)`.
fn seed_from(beacon: &Hash32, deal: &ObjectId) -> Hash32 {
    hash_parts(SEED_DOMAIN, &[beacon.as_bytes(), deal.as_hash().as_bytes()])
}

/// `beacon_at(b)` (§5.52), for `b ≤ ctx.pulse`: the current Pulse's beacon
/// if `b = ctx.pulse`; otherwise the beacon of the record in slot `(b /
/// BEACON_INTERVAL) mod BEACON_SLOTS`, which must be a beacon record of `b`.
fn beacon_at(run: &mut TxRun<'_>, b: Pulse) -> Step<Hash32> {
    if b == run.ctx.pulse {
        return Ok(run.ctx.beacon);
    }
    if b.0 % BEACON_INTERVAL != 0 || b > run.ctx.pulse {
        return Err(fail(format!("Pulse {b} has no beacon record")));
    }
    let slot = u32::try_from((b.0 / BEACON_INTERVAL) % BEACON_SLOTS)
        .map_err(|_| fail("beacon slot out of range"))?;
    match run.load(&well_known::beacon(slot))?.data {
        ObjectData::Beacon(record) if record.pulse == b => Ok(record.beacon),
        _ => Err(fail(format!("the beacon of Pulse {b} is not recorded"))),
    }
}

/// `challenge(seed, i, k)` (§5.52): the first 8 bytes, read big-endian, of
/// `hash_parts(CHALLENGE_DOMAIN, [seed (32 bytes), i (4 bytes, big-endian),
/// k (1 byte)])`.
fn challenge(seed: &Hash32, round: u32, k: u8) -> u64 {
    hash_parts(
        CHALLENGE_DOMAIN,
        &[seed.as_bytes(), &round.to_be_bytes(), &[k]],
    )
    .prefix_u64()
}

/// The leaf challenge `k` of round `round` asks for (§5.52):
/// `challenge(seed, round, k) mod size`. A tree of no leaves has nothing to
/// challenge (§5.50 gives every deal at least one leaf).
fn challenged_leaf(seed: &Hash32, round: u32, k: u8, size: u64) -> Step<u64> {
    challenge(seed, round, k)
        .checked_rem(size)
        .ok_or_else(|| fail("a deal of no leaves cannot be challenged"))
}

/// The round under way at `pulse` for rounds of `round_length` Pulses from
/// `start` (§5.52, §5.54): `(pulse − start) / round_length`. `None` before
/// `start`, and for rounds of no Pulses (which §5.50 rules out).
fn round_at(start: Pulse, round_length: u64, pulse: Pulse) -> Option<u64> {
    pulse.0.checked_sub(start.0)?.checked_div(round_length)
}

/// The Pulse a deal ends, the first after its last round (§5.51, §5.54):
/// `start + rounds × round_length`, saturating.
fn deal_end(start: Pulse, rounds: u32, round_length: u64) -> Pulse {
    Pulse(
        start
            .0
            .saturating_add(u64::from(rounds).saturating_mul(round_length)),
    )
}

/// Loads `id`, which must be a deal.
fn deal_object(run: &mut TxRun<'_>, id: &ObjectId) -> Step<(Object, Deal)> {
    let object = run.load(id)?;
    let deal = match &object.data {
        ObjectData::Deal(deal) => (**deal).clone(),
        _ => return Err(fail(format!("{id} is not a deal"))),
    };
    Ok((object, deal))
}

/// Loads `id`, which must be the custody record of deal `deal`.
fn custody_record(run: &mut TxRun<'_>, id: &ObjectId, deal: &ObjectId) -> Step<Custody> {
    match run.load(id)?.data {
        ObjectData::Custody(custody) if custody.deal == *deal => Ok(custody),
        _ => Err(fail(format!("{id} is not the deal's custody record"))),
    }
}

/// Writes `deal` back as `object`'s data.
fn put_deal(run: &mut TxRun<'_>, mut object: Object, deal: Deal) -> Step {
    object.data = ObjectData::Deal(Box::new(deal));
    run.store(object)
}

/// §5.50 PostDeal.
pub(super) fn post(run: &mut TxRun<'_>, terms: Terms<'_>) -> Step {
    let pulse = run.ctx.pulse;
    let client = run.sender();
    // Step 1.
    if !(1..=MAX_DEAL_LEAVES).contains(&terms.size) {
        return Err(fail(format!("a deal covers 1 to {MAX_DEAL_LEAVES} leaves")));
    }
    if !(1..=MAX_CHALLENGES).contains(&terms.challenges) {
        return Err(fail(format!(
            "a round challenges 1 to {MAX_CHALLENGES} leaves"
        )));
    }
    if terms.price == Amount::ZERO {
        return Err(fail("the price must be positive"));
    }
    if !(1..=MAX_ROUNDS).contains(&terms.rounds) {
        return Err(fail(format!("a deal has 1 to {MAX_ROUNDS} rounds")));
    }
    if !(MIN_ROUND_LENGTH..=MAX_ROUND_LENGTH).contains(&terms.round_length) {
        return Err(fail(format!(
            "a round lasts {MIN_ROUND_LENGTH} to {MAX_ROUND_LENGTH} Pulses"
        )));
    }
    if terms.round_length % BEACON_INTERVAL != 0 {
        return Err(fail(format!(
            "a round lasts a multiple of {BEACON_INTERVAL} Pulses"
        )));
    }
    if terms.provider.is_none() && terms.collateral == Amount::ZERO {
        return Err(fail("an open deal must ask for collateral"));
    }
    let latest = pulse.0.saturating_add(MAX_PERIOD);
    if !(pulse.0 < terms.accept_by.0 && terms.accept_by.0 <= latest) {
        return Err(fail("accept_by is out of range"));
    }
    if terms.locator.len() > MAX_LOCATOR {
        return Err(fail(format!(
            "the locator is longer than {MAX_LOCATOR} bytes"
        )));
    }
    // Step 2.
    let escrow =
        times(terms.rounds, terms.price).ok_or_else(|| fail("price × rounds overflows"))?;
    run.draw(usd(), escrow)?;
    // Step 3.
    let deal = run.next_created_id();
    run.create(
        Owner::Shared(object_domain("deal", &deal)),
        ObjectData::Deal(Box::new(Deal {
            client,
            provider: terms.provider,
            root: terms.root,
            size: terms.size,
            challenges: terms.challenges,
            price: terms.price,
            rounds: terms.rounds,
            round_length: terms.round_length,
            collateral: terms.collateral,
            accept_by: terms.accept_by,
            locator: terms.locator.to_owned(),
            escrow,
            posted_at: pulse,
            state: DealState::Open,
        })),
    )?;
    // Step 4.
    run.event(Event::DealPosted {
        deal,
        client,
        provider: terms.provider,
        size: terms.size,
        price: terms.price,
        rounds: terms.rounds,
        collateral: terms.collateral,
    });
    Ok(())
}

/// §5.51 AcceptDeal.
pub(super) fn accept(run: &mut TxRun<'_>, deal_id: ObjectId) -> Step {
    let pulse = run.ctx.pulse;
    let sender = run.sender();
    // Step 1.
    let (object, mut deal) = deal_object(run, &deal_id)?;
    if deal.state != DealState::Open {
        return Err(fail("the deal is not open"));
    }
    if pulse > deal.accept_by {
        return Err(fail("the deal can no longer be accepted"));
    }
    if let Some(provider) = deal.provider {
        if provider != sender {
            return Err(fail("only the provider the deal names accepts it"));
        }
    }
    // Step 2.
    run.draw(usd(), deal.collateral)?;
    // Step 3: the first multiple of BEACON_INTERVAL after this Pulse.
    let start = Pulse(
        (pulse.0 / BEACON_INTERVAL)
            .saturating_add(1)
            .saturating_mul(BEACON_INTERVAL),
    );
    // Step 4.
    let custody = run.create(
        Owner::Account(sender),
        ObjectData::Custody(Custody {
            deal: deal_id,
            root: deal.root,
            size: deal.size,
            challenges: deal.challenges,
            start,
            round_length: deal.round_length,
            rounds: deal.rounds,
            next: 0,
            proven: 0,
        }),
    )?;
    // Step 5.
    deal.state = DealState::Active {
        provider: sender,
        custody,
        start,
        bond: deal.collateral,
        paid: 0,
    };
    put_deal(run, object, deal)?;
    // Step 6.
    run.event(Event::DealAccepted {
        deal: deal_id,
        provider: sender,
        custody,
        start,
    });
    Ok(())
}

/// The checks of §5.52 step 2 for proof `p` of custody record `c`, executed
/// at `pulse`, up to the beacon: the round under way, not proven yet, with
/// an answer per challenge. Returns the round it proves.
fn check_round(c: &Custody, p: &CustodyProof, pulse: Pulse) -> Step<u32> {
    let i = p.round;
    let under_way = round_at(c.start, c.round_length, pulse)
        .ok_or_else(|| fail("the deal's first round has not begun"))?;
    if u64::from(i) != under_way {
        return Err(fail(format!(
            "round {i} is not the round under way ({under_way})"
        )));
    }
    if i >= c.rounds {
        return Err(fail("the deal has no such round"));
    }
    if i < c.next {
        return Err(fail(format!("round {i} is already proven or passed over")));
    }
    if p.leaves.len() != usize::from(c.challenges) {
        return Err(fail(format!(
            "a proof answers each of its round's {} challenges",
            c.challenges
        )));
    }
    Ok(i)
}

/// The rest of §5.52 step 2: every challenged leaf of round `i`, drawn from
/// `seed`, is answered with a leaf of the deal's tree.
fn check_leaves(c: &Custody, p: &CustodyProof, i: u32, seed: &Hash32) -> Step {
    for (k, leaf) in (0..c.challenges).zip(&p.leaves) {
        let index = challenged_leaf(seed, i, k, c.size)?;
        if leaf.data.len() > MAX_LEAF_BYTES {
            return Err(fail(format!(
                "a leaf is longer than {MAX_LEAF_BYTES} bytes"
            )));
        }
        if leaf.path.len() > MAX_PATH {
            return Err(fail(format!("a path is longer than {MAX_PATH} hashes")));
        }
        let hash = merkle::leaf_hash(&leaf.data);
        if merkle::verify_inclusion(&c.root, c.size, index, &hash, &leaf.path).is_err() {
            return Err(fail(format!("leaf {index} is not in the deal's tree")));
        }
    }
    Ok(())
}

/// §5.52 ProveStorage: `proofs[j]` proves the custody record `tx.inputs[j +
/// 1]`.
pub(super) fn prove(run: &mut TxRun<'_>, proofs: &[CustodyProof]) -> Step {
    let tx = run.tx;
    let pulse = run.ctx.pulse;
    // Step 1.
    if proofs.is_empty() {
        return Err(fail("no proofs"));
    }
    if tx.inputs.len() <= proofs.len() {
        return Err(fail("every proof needs its custody record as an input"));
    }
    let mut leaves: usize = 0;
    for proof in proofs {
        leaves = leaves
            .checked_add(proof.leaves.len())
            .ok_or_else(|| fail("too many leaves"))?;
    }
    if leaves > MAX_PROOF_LEAVES {
        return Err(fail(format!(
            "one transaction proves at most {MAX_PROOF_LEAVES} leaves"
        )));
    }
    // Step 2: each record is loaded only once every proof before it passed.
    for (input, proof) in tx.inputs[1..].iter().zip(proofs) {
        let id = input.id;
        let mut object = run.load(&id)?;
        let ObjectData::Custody(record) = &object.data else {
            return Err(fail(format!("{id} is not a custody record")));
        };
        let round = check_round(record, proof, pulse)?;
        // The round's first Pulse.
        let first = record
            .start
            .0
            .checked_add(
                u64::from(round)
                    .checked_mul(record.round_length)
                    .ok_or_else(|| fail("the round's first Pulse overflows"))?,
            )
            .map(Pulse)
            .ok_or_else(|| fail("the round's first Pulse overflows"))?;
        let deal = record.deal;
        let beacon = beacon_at(run, first)?;
        let seed = seed_from(&beacon, &deal);
        let ObjectData::Custody(custody) = &mut object.data else {
            return Err(fail(format!("{id} is not a custody record")));
        };
        check_leaves(custody, proof, round, &seed)?;
        custody.proven = custody
            .proven
            .checked_add(1)
            .ok_or_else(|| fail("proven rounds overflow"))?;
        custody.next = round
            .checked_add(1)
            .ok_or_else(|| fail("the next round overflows"))?;
        run.store(object)?;
        run.event(Event::StorageProven {
            deal,
            custody: id,
            round,
        });
    }
    Ok(())
}

/// §5.53 SettleDeal.
pub(super) fn settle(run: &mut TxRun<'_>, deal_id: ObjectId) -> Step {
    // Step 1.
    let (object, mut deal) = deal_object(run, &deal_id)?;
    let DealState::Active {
        provider,
        custody,
        start,
        bond,
        paid,
    } = deal.state
    else {
        return Err(fail("the deal is not active"));
    };
    if run.sender() != provider {
        return Err(fail("only the deal's provider settles it"));
    }
    // Step 2.
    let c = custody_record(run, &custody, &deal_id)?;
    // Step 3.
    let due = c
        .proven
        .checked_sub(paid)
        .filter(|due| *due > 0)
        .ok_or_else(|| fail("no proven round is left to pay"))?;
    let amount = times(due, deal.price).ok_or_else(|| fail("rounds × price overflows"))?;
    deal.escrow = deal
        .escrow
        .checked_sub(amount)
        .map_err(|_| fail("the escrow does not cover the rounds due"))?;
    deal.state = DealState::Active {
        provider,
        custody,
        start,
        bond,
        paid: c.proven,
    };
    put_deal(run, object, deal)?;
    run.mint(provider, usd(), amount)?;
    // Step 4.
    run.event(Event::DealSettled {
        deal: deal_id,
        rounds: due,
        amount,
    });
    Ok(())
}

/// What closing a deal settles (§5.54 steps 2 and 3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Closing {
    /// Rounds proven.
    proven: u32,
    /// Rounds missed.
    missed: u32,
    /// Paid to the provider now, for proven rounds not yet paid.
    paid_now: Amount,
    /// Collateral returned to the provider.
    returned: Amount,
    /// Collateral forfeited to the treasury.
    forfeited: Amount,
}

/// The fields of `DealState::Active` that closing uses.
#[derive(Clone, Copy, Debug)]
struct Accepted {
    provider: AccountId,
    start: Pulse,
    bond: Amount,
    paid: u32,
}

/// §5.54 step 3 for the accepted deal `d`, once its custody record `c` is
/// loaded: whether `sender` may close it at `pulse`, and what closing pays,
/// returns and forfeits. The caller moves the escrow and mints the coins.
fn close_accepted(
    d: &Deal,
    c: &Custody,
    a: &Accepted,
    pulse: Pulse,
    sender: &AccountId,
) -> Step<Closing> {
    let proven = c.proven;
    let end = deal_end(a.start, d.rounds, d.round_length);
    let (missed, forfeited, returned) = if pulse >= end {
        // At the end.
        if *sender != d.client && *sender != a.provider {
            return Err(fail(
                "only the client or the provider closes a deal at its end",
            ));
        }
        let missed = d
            .rounds
            .checked_sub(proven)
            .ok_or_else(|| fail("more rounds proven than the deal has"))?;
        // ⌊bond × missed / rounds⌋, exactly (§5.45's mul_div).
        let forfeited =
            super::delegation::floor_mul_div(a.bond.0, u128::from(missed), u128::from(d.rounds))
                .map(Amount)
                .ok_or_else(|| fail("bond × missed / rounds overflows or a deal of no rounds"))?;
        let returned = a.bond.checked_sub(forfeited)?;
        (missed, forfeited, returned)
    } else {
        // Abandoned.
        if *sender != d.client {
            return Err(fail("only the client closes a deal before its end"));
        }
        let r = round_at(a.start, d.round_length, pulse)
            .ok_or_else(|| fail("the deal's first round has not begun"))?;
        if r < u64::from(c.next) + u64::from(ABANDON_ROUNDS) {
            return Err(fail(format!(
                "the provider has proven one of the last {ABANDON_ROUNDS} rounds"
            )));
        }
        let missed = r
            .checked_sub(u64::from(proven))
            .and_then(|missed| u32::try_from(missed).ok())
            .ok_or_else(|| fail("the missed rounds are out of range"))?;
        (missed, a.bond, Amount::ZERO)
    };
    let due = proven
        .checked_sub(a.paid)
        .ok_or_else(|| fail("more rounds paid than proven"))?;
    let paid_now = times(due, d.price).ok_or_else(|| fail("rounds × price overflows"))?;
    Ok(Closing {
        proven,
        missed,
        paid_now,
        returned,
        forfeited,
    })
}

/// §5.54 CloseDeal.
pub(super) fn close(run: &mut TxRun<'_>, deal_id: ObjectId) -> Step {
    let pulse = run.ctx.pulse;
    let sender = run.sender();
    // Step 1.
    let (object, mut deal) = deal_object(run, &deal_id)?;
    let closing = match deal.state {
        // Step 2.
        DealState::Open => {
            if sender != deal.client {
                return Err(fail("only the client closes an open deal"));
            }
            Closing::default()
        }
        // Step 3: the custody record is loaded before the sender is checked.
        DealState::Active {
            provider,
            custody,
            start,
            bond,
            paid,
        } => {
            let c = custody_record(run, &custody, &deal_id)?;
            let accepted = Accepted {
                provider,
                start,
                bond,
                paid,
            };
            let closing = close_accepted(&deal, &c, &accepted, pulse, &sender)?;
            deal.escrow = deal
                .escrow
                .checked_sub(closing.paid_now)
                .map_err(|_| fail("the escrow does not cover the rounds due"))?;
            let to_provider = closing.paid_now.checked_add(closing.returned)?;
            run.mint(provider, usd(), to_provider)?;
            closing
        }
        // Step 4.
        DealState::Closed { .. } => return Err(fail("the deal is closed")),
    };
    // Step 5.
    let refund = deal.escrow;
    run.mint(deal.client, usd(), refund)?;
    if closing.forfeited > Amount::ZERO {
        run.defer(Deferred::Treasury {
            asset: usd(),
            amount: closing.forfeited,
        });
    }
    // Step 6.
    deal.escrow = Amount::ZERO;
    deal.state = DealState::Closed {
        at: pulse,
        proven: closing.proven,
        missed: closing.missed,
        refund,
        forfeited: closing.forfeited,
    };
    put_deal(run, object, deal)?;
    // Step 7.
    run.event(Event::DealClosed {
        deal: deal_id,
        refund,
        paid: closing.paid_now,
        returned: closing.returned,
        forfeited: closing.forfeited,
        missed: closing.missed,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeSet;

    use proptest::prelude::*;
    use sion_core::ids::Version;
    use sion_crypto::Authorization;
    use sion_exec::object::{created_id, Coin, ObjectRef};
    use sion_exec::state::ChainConfig;
    use sion_exec::storage::LeafProof;
    use sion_exec::tx::{SignedTransaction, Transaction, TxKind};

    use crate::context::SpecContext;
    use crate::error::{Abort, SpecError};
    use crate::tx::RunOutcome;
    use crate::view::ChunkView;

    /// The beacon of every test Pulse.
    const BEACON: Hash32 = Hash32([0x5a; 32]);
    /// Every test transaction's fee.
    const FEE: Amount = Amount(10_000);
    /// What a round of the test deals pays.
    const PRICE: Amount = Amount(1_000);

    fn account(tag: &[u8]) -> AccountId {
        AccountId::derive(tag)
    }

    fn context(pulse: u64) -> SpecContext {
        SpecContext {
            pulse: Pulse(pulse),
            beacon: BEACON,
            config: ChainConfig {
                chain_id: "test".into(),
                shard_count: 1,
                base_fee: Amount(1_000),
                fee_per_kib: Amount(100),
                faucet: None,
                max_claim_window: 1_000,
                max_formula_size: 64,
                max_payload_bytes: 1_024,
                challenger_share_permille: 500,
            },
        }
    }

    /// A USD coin `owner` holds.
    fn coin(tag: &[u8], owner: AccountId, amount: u128) -> Object {
        Object {
            id: ObjectId::derive(tag),
            version: Version(1),
            owner: Owner::Account(owner),
            data: ObjectData::Coin(Coin {
                asset: usd(),
                amount: Amount(amount),
            }),
        }
    }

    /// `sender`'s transaction of `kind` naming `inputs`. Operations never
    /// look at its authorization (prepare does), so it carries none.
    fn signed(sender: AccountId, inputs: &[&Object], kind: TxKind) -> SignedTransaction {
        SignedTransaction {
            tx: Transaction {
                sender,
                genesis_policy: None,
                inputs: inputs
                    .iter()
                    .map(|object| ObjectRef {
                        id: object.id,
                        version: object.version,
                    })
                    .collect(),
                fee: FEE,
                kind,
                expires_at: Pulse(u64::MAX),
                nonce: 0,
            },
            auth: Authorization {
                signatures: Vec::new(),
            },
            batch: None,
        }
    }

    /// Runs the operation of `signed` (§5, through the dispatch of
    /// `ops::apply`) over a witness holding exactly `witness`.
    fn apply_over(
        ctx: &SpecContext,
        witness: &[&Object],
        signed: &SignedTransaction,
    ) -> (Step, RunOutcome) {
        let view = ChunkView::new(
            witness
                .iter()
                .map(|object| (object.id, Some((*object).clone())))
                .collect(),
        );
        let mut run = TxRun::new(ctx, &view, signed);
        let result = crate::ops::apply(&mut run);
        (result, run.finish())
    }

    /// The object `signed` created with index `index`.
    fn created<'o>(outcome: &'o RunOutcome, signed: &SignedTransaction, index: u32) -> &'o Object {
        &outcome.parts.writes[&created_id(&signed.digest(), index)]
    }

    /// The owner and amount of a USD coin.
    fn coin_of(object: &Object) -> (Owner, Amount) {
        match &object.data {
            ObjectData::Coin(coin) if coin.asset == usd() => (object.owner, coin.amount),
            other => panic!("not a USD coin: {other:?}"),
        }
    }

    fn deal_in(outcome: &RunOutcome, id: &ObjectId) -> Deal {
        match &outcome.parts.writes[id].data {
            ObjectData::Deal(deal) => (**deal).clone(),
            other => panic!("not a deal: {other:?}"),
        }
    }

    fn custody_in(outcome: &RunOutcome, id: &ObjectId) -> Custody {
        match &outcome.parts.writes[id].data {
            ObjectData::Custody(custody) => custody.clone(),
            other => panic!("not a custody record: {other:?}"),
        }
    }

    fn ids(list: &[ObjectId]) -> BTreeSet<ObjectId> {
        list.iter().copied().collect()
    }

    /// An error of the operation (the transaction fails), as opposed to a
    /// read outside the witness (the chunk is rejected).
    fn fails(result: &Step) -> bool {
        matches!(result, Err(Abort::Op(_)))
    }

    /// Test data: its leaves, their leaf hashes and the tree's root.
    struct Data {
        leaves: Vec<Vec<u8>>,
        hashes: Vec<Hash32>,
        root: Hash32,
    }

    /// `n` leaves of test data.
    fn data(n: usize) -> Data {
        let leaves: Vec<Vec<u8>> = (0..n).map(|i| format!("leaf {i}").into_bytes()).collect();
        let hashes: Vec<Hash32> = leaves.iter().map(|leaf| merkle::leaf_hash(leaf)).collect();
        let root = merkle::root(&hashes);
        Data {
            leaves,
            hashes,
            root,
        }
    }

    impl Data {
        fn size(&self) -> u64 {
            self.leaves.len() as u64
        }

        /// What a provider holding the data answers for round `round` of
        /// custody record `c` (§5.52), when the round's first Pulse had the
        /// tests' beacon.
        fn answer(&self, c: &Custody, round: u32) -> CustodyProof {
            self.answer_to(c, round, &seed_from(&BEACON, &c.deal))
        }

        /// The answer to round `round`'s challenges drawn from `seed`.
        fn answer_to(&self, c: &Custody, round: u32, seed: &Hash32) -> CustodyProof {
            let leaves = (0..c.challenges)
                .map(|k| {
                    let index = challenged_leaf(seed, round, k, c.size).unwrap();
                    let index = usize::try_from(index).unwrap();
                    LeafProof {
                        data: self.leaves[index].clone(),
                        path: merkle::inclusion_proof(&self.hashes, index).unwrap(),
                    }
                })
                .collect();
            CustodyProof { round, leaves }
        }
    }

    /// §5.52 step 2 for one proof, when the round's first Pulse had the
    /// tests' beacon.
    fn check_proof(c: &Custody, p: &CustodyProof, pulse: Pulse) -> Step<u32> {
        check_proof_with(c, p, pulse, &seed_from(&BEACON, &c.deal))
    }

    /// §5.52 step 2 for one proof, with the round's seed given.
    fn check_proof_with(c: &Custody, p: &CustodyProof, pulse: Pulse, seed: &Hash32) -> Step<u32> {
        let i = check_round(c, p, pulse)?;
        check_leaves(c, p, i, seed)?;
        Ok(i)
    }

    /// The beacon record of `pulse` (§6.3), as the witness holds it.
    fn beacon_object(pulse: u64, beacon: Hash32) -> Object {
        let slot = u32::try_from((pulse / BEACON_INTERVAL) % BEACON_SLOTS).unwrap();
        Object {
            id: well_known::beacon(slot),
            version: Version(4),
            owner: Owner::Shared(well_known::beacons_domain()),
            data: ObjectData::Beacon(sion_exec::beacons::BeaconRecord {
                pulse: Pulse(pulse),
                beacon,
            }),
        }
    }

    /// The root of a perfect tree (a power of two leaves) holding `leaf` at
    /// `index`, with `path` its siblings from the leaf up.
    fn perfect_root(leaf: Hash32, index: u64, path: &[Hash32]) -> Hash32 {
        let mut acc = leaf;
        for (level, sibling) in path.iter().enumerate() {
            acc = if (index >> level) & 1 == 1 {
                merkle::node_hash(sibling, &acc)
            } else {
                merkle::node_hash(&acc, sibling)
            };
        }
        acc
    }

    /// A fresh custody record of `data` for deal `deal`: 5 rounds of 8
    /// Pulses from Pulse 11, three challenges a round. (AcceptDeal starts
    /// rounds on multiples of BEACON_INTERVAL; the rules these tests check
    /// do not depend on it, and a proof in a round's first Pulse uses that
    /// Pulse's beacon, so no beacon record is needed.)
    fn custody(deal: ObjectId, data: &Data) -> Custody {
        Custody {
            deal,
            root: data.root,
            size: data.size(),
            challenges: 3,
            start: Pulse(11),
            round_length: 8,
            rounds: 5,
            next: 0,
            proven: 0,
        }
    }

    /// An open deal of `data` posted at Pulse 2: 5 rounds of 8 Pulses at
    /// `PRICE`, a collateral of 5,003, open for acceptance until Pulse 20.
    fn posted(client: AccountId, provider: Option<AccountId>, data: &Data) -> Deal {
        Deal {
            client,
            provider,
            root: data.root,
            size: data.size(),
            challenges: 3,
            price: PRICE,
            rounds: 5,
            round_length: 8,
            collateral: Amount(5_003),
            accept_by: Pulse(20),
            locator: "sion://data".into(),
            escrow: Amount(5_000),
            posted_at: Pulse(2),
            state: DealState::Open,
        }
    }

    fn deal_at(id: ObjectId, deal: Deal) -> Object {
        Object {
            id,
            version: Version(3),
            owner: Owner::Shared(object_domain("deal", &id)),
            data: ObjectData::Deal(Box::new(deal)),
        }
    }

    fn custody_at(id: ObjectId, owner: AccountId, custody: Custody) -> Object {
        Object {
            id,
            version: Version(2),
            owner: Owner::Account(owner),
            data: ObjectData::Custody(custody),
        }
    }

    /// An accepted deal and its custody record.
    struct Fixture {
        client: AccountId,
        provider: AccountId,
        deal_id: ObjectId,
        custody_id: ObjectId,
        deal: Object,
        custody: Object,
    }

    /// The deal of [`posted`], accepted by its provider at Pulse 10 with a
    /// bond of 5,003: round 0 is Pulses 11 to 18 and the deal ends at Pulse
    /// 51. The provider has proven `proven` rounds, the last of them round
    /// `next − 1`, and has been paid for `paid`.
    fn accepted(proven: u32, next: u32, paid: u32) -> Fixture {
        let client = account(b"client");
        let provider = account(b"provider");
        let deal_id = ObjectId::derive(b"deal");
        let custody_id = ObjectId::derive(b"custody");
        let data = data(10);
        let mut record = custody(deal_id, &data);
        record.proven = proven;
        record.next = next;
        let mut deal = posted(client, Some(provider), &data);
        deal.escrow = times(deal.rounds - paid, PRICE).unwrap();
        deal.state = DealState::Active {
            provider,
            custody: custody_id,
            start: Pulse(11),
            bond: deal.collateral,
            paid,
        };
        Fixture {
            client,
            provider,
            deal_id,
            custody_id,
            deal: deal_at(deal_id, deal),
            custody: custody_at(custody_id, provider, record),
        }
    }

    /// `sender` closes the fixture's deal at `pulse`, over `witness`.
    fn close_at(
        f: &Fixture,
        sender: AccountId,
        pulse: u64,
        witness: &[&Object],
    ) -> (Step, RunOutcome, SignedTransaction) {
        let gas = coin(b"gas", sender, 100_000);
        let tx = signed(sender, &[&gas], TxKind::CloseDeal { deal: f.deal_id });
        let (result, outcome) = apply_over(&context(pulse), witness, &tx);
        (result, outcome, tx)
    }

    // -- The formulas of §5.51 and §5.52 ----------------------------------

    #[test]
    fn challenges_match_values_computed_by_hand() {
        // Computed outside this code base, with Python's hashlib: SHA3-256 of
        // u16_be(22) ‖ "sion/storage/challenge" ‖ u64_be(32) ‖ seed ‖
        // u64_be(4) ‖ be32(i) ‖ u64_be(1) ‖ k, its first 8 bytes read
        // big-endian, then reduced modulo the size.
        let counting = Hash32(std::array::from_fn(|i| i as u8));
        assert_eq!(challenge(&counting, 7, 3), 16_212_771_450_120_386_125);
        assert_eq!(challenged_leaf(&counting, 7, 3, 1_000).unwrap(), 125);
        let flat = Hash32([0xab; 32]);
        assert_eq!(challenge(&flat, 0, 0), 4_398_885_085_296_536_428);
        assert_eq!(
            challenged_leaf(&flat, 0, 0, MAX_DEAL_LEAVES).unwrap(),
            746_332_171_116
        );
        assert_eq!(challenge(&flat, 999_999, 15), 9_713_013_666_938_509_021);
        assert_eq!(challenged_leaf(&flat, 999_999, 15, 37).unwrap(), 20);
        assert_eq!(challenged_leaf(&flat, 999_999, 15, 1).unwrap(), 0);
        // A tree of no leaves has nothing to challenge: an error, not a panic.
        assert!(fails(&challenged_leaf(&flat, 0, 0, 0).map(|_| ())));
    }

    #[test]
    fn seeds_match_values_computed_by_hand() {
        // SHA3-256 of u16_be(17) ‖ "sion/storage/seed" ‖ then u64_be(32) ‖
        // part for the beacon of the round's first Pulse and the deal,
        // computed outside this code base with Python's hashlib.
        let beacon = Hash32([0x11; 32]);
        let deal = ObjectId::from_hash(Hash32([0x22; 32]));
        assert_eq!(
            seed_from(&beacon, &deal).to_hex(),
            "4a29b4e2272e9bbb9e94564a8cb0411b189c4c858a1289941d24504a913501b8"
        );
    }

    #[test]
    fn rounds_count_from_the_start_and_deals_end_after_the_last() {
        assert_eq!(round_at(Pulse(11), 8, Pulse(10)), None);
        assert_eq!(round_at(Pulse(11), 8, Pulse(11)), Some(0));
        assert_eq!(round_at(Pulse(11), 8, Pulse(18)), Some(0));
        assert_eq!(round_at(Pulse(11), 8, Pulse(19)), Some(1));
        assert_eq!(round_at(Pulse(11), 8, Pulse(50)), Some(4));
        assert_eq!(round_at(Pulse(0), 8, Pulse(u64::MAX)), Some(u64::MAX / 8));
        // Rounds of no Pulses (§5.50 rules them out) have no round under way.
        assert_eq!(round_at(Pulse(0), 0, Pulse(5)), None);
        // Round i is Pulses start + i × length to start + (i + 1) × length −
        // 1; the deal ends the Pulse after its last round, saturating.
        assert_eq!(deal_end(Pulse(11), 5, 8), Pulse(51));
        assert_eq!(
            deal_end(Pulse(u64::MAX - 5), MAX_ROUNDS, MAX_ROUND_LENGTH),
            Pulse(u64::MAX)
        );
        assert_eq!(deal_end(Pulse(1), u32::MAX, u64::MAX), Pulse(u64::MAX));
    }

    #[test]
    fn the_longest_path_reaches_the_largest_deal() {
        // A tree of MAX_DEAL_LEAVES leaves is exactly MAX_PATH levels deep.
        assert_eq!(1u64 << MAX_PATH, MAX_DEAL_LEAVES);
    }

    proptest! {
        /// Cross-checks against the reference executor's helpers (allowed
        /// in unit tests; the library never calls them).
        #[test]
        fn challenges_and_seeds_agree_with_the_reference(
            seed in any::<[u8; 32]>(),
            beacon in any::<[u8; 32]>(),
            deal in any::<[u8; 32]>(),
            round in any::<u32>(),
            k in any::<u8>(),
            size in 1..=MAX_DEAL_LEAVES,
            small in 1u64..=64,
        ) {
            let (seed, beacon) = (Hash32(seed), Hash32(beacon));
            let deal = ObjectId::from_hash(Hash32(deal));
            for size in [size, small] {
                prop_assert_eq!(
                    challenged_leaf(&seed, round, k, size).unwrap(),
                    sion_exec::storage::challenge_index(&seed, round, k, size)
                );
            }
            prop_assert_eq!(
                seed_from(&beacon, &deal),
                sion_exec::storage::round_seed(&beacon, &deal)
            );
        }

        #[test]
        fn rounds_and_ends_agree_with_the_reference(
            // Half the starts lie where a deal's end saturates.
            start in prop_oneof![any::<u64>(), (u64::MAX - 2_000_000_000_000)..=u64::MAX],
            pulse in prop_oneof![any::<u64>(), (u64::MAX - 2_000_000_000_000)..=u64::MAX],
            round_length in MIN_ROUND_LENGTH..=MAX_ROUND_LENGTH,
            rounds in 1..=MAX_ROUNDS,
        ) {
            prop_assert_eq!(
                round_at(Pulse(start), round_length, Pulse(pulse)),
                sion_exec::storage::current_round(Pulse(start), round_length, Pulse(pulse))
            );
            let mut deal = posted(account(b"client"), None, &data(1));
            deal.rounds = rounds;
            deal.round_length = round_length;
            deal.state = DealState::Active {
                provider: account(b"provider"),
                custody: ObjectId::derive(b"custody"),
                start: Pulse(start),
                bond: Amount::ZERO,
                paid: 0,
            };
            prop_assert_eq!(deal.end(), Some(deal_end(Pulse(start), rounds, round_length)));
        }
    }

    // -- Checking one proof (§5.52 step 2) ---------------------------------

    #[test]
    fn a_proof_verifies_only_during_its_round() {
        let data = data(1_000);
        let deal = ObjectId::derive(b"deal");
        let mut c = custody(deal, &data);
        let proof = data.answer(&c, 0);
        assert!(
            fails(&check_proof(&c, &proof, Pulse(10)).map(|_| ())),
            "before the start"
        );
        for pulse in 11..=18 {
            assert_eq!(check_proof(&c, &proof, Pulse(pulse)).unwrap(), 0);
        }
        assert!(
            fails(&check_proof(&c, &proof, Pulse(19)).map(|_| ())),
            "round 0 is over"
        );
        // Proving round 2 passes over round 1.
        let proof = data.answer(&c, 2);
        assert_eq!(check_proof(&c, &proof, Pulse(27)).unwrap(), 2);
        c.proven = 1;
        c.next = 3;
        assert!(
            fails(&check_proof(&c, &proof, Pulse(27)).map(|_| ())),
            "round 2 is proven"
        );
        // Each round draws its own challenges, and only from its seed.
        let fresh = data.answer(&c, 3);
        assert_eq!(check_proof(&c, &fresh, Pulse(35)).unwrap(), 3);
        let other = seed_from(&Hash32([0x77; 32]), &deal);
        let elsewhere = data.answer_to(&c, 3, &other);
        assert_ne!(elsewhere, fresh);
        assert!(
            fails(&check_proof(&c, &elsewhere, Pulse(35)).map(|_| ())),
            "an answer to another beacon's challenge"
        );
        assert_eq!(
            check_proof_with(&c, &elsewhere, Pulse(35), &other).unwrap(),
            3
        );
        // Nothing can be proven once the deal's rounds are over.
        let after = data.answer(&c, 5);
        assert!(fails(&check_proof(&c, &after, Pulse(51)).map(|_| ())));
    }

    #[test]
    fn a_proof_must_answer_every_challenge_with_the_challenged_leaf() {
        let data = data(1_000);
        let c = custody(ObjectId::derive(b"deal"), &data);
        let pulse = Pulse(19);
        let good = data.answer(&c, 1);
        assert_eq!(check_proof(&c, &good, pulse).unwrap(), 1);
        let rejected = |proof: &CustodyProof| fails(&check_proof(&c, proof, pulse).map(|_| ()));
        // Another round's number.
        for round in [0, 2, u32::MAX] {
            assert!(rejected(&CustodyProof {
                round,
                ..good.clone()
            }));
        }
        // Too few or too many leaves.
        let mut short = good.clone();
        short.leaves.pop();
        assert!(rejected(&short));
        let mut long = good.clone();
        long.leaves.push(good.leaves[0].clone());
        assert!(rejected(&long));
        // Bytes that are not the leaf.
        let mut altered = good.clone();
        altered.leaves[1].data.push(0);
        assert!(rejected(&altered));
        // Another leaf, with its own valid path.
        let seed = seed_from(&BEACON, &c.deal);
        let challenged = challenged_leaf(&seed, 1, 1, c.size).unwrap() as usize;
        let other = (challenged + 1) % data.leaves.len();
        let mut misplaced = good.clone();
        misplaced.leaves[1] = LeafProof {
            data: data.leaves[other].clone(),
            path: merkle::inclusion_proof(&data.hashes, other).unwrap(),
        };
        assert!(rejected(&misplaced));
        // A path that does not lead to the root.
        let mut lost = good.clone();
        lost.leaves[2].path[0] = Hash32([1; 32]);
        assert!(rejected(&lost));
        // Answers in another order (the challenged leaves differ here).
        let mut swapped = good.clone();
        swapped.leaves.swap(0, 2);
        assert_ne!(swapped.leaves[0], good.leaves[0]);
        assert!(rejected(&swapped));
    }

    #[test]
    fn leaves_and_paths_are_bounded() {
        let deal = ObjectId::derive(b"deal");
        let seed = seed_from(&BEACON, &deal);
        // One leaf, so every challenge asks for it and only the bound can
        // fail.
        for (length, ok) in [(MAX_LEAF_BYTES, true), (MAX_LEAF_BYTES + 1, false)] {
            let leaf = vec![7u8; length];
            let c = Custody {
                deal,
                root: merkle::leaf_hash(&leaf),
                size: 1,
                challenges: 2,
                start: Pulse(11),
                round_length: 8,
                rounds: 5,
                next: 0,
                proven: 0,
            };
            let answer = LeafProof {
                data: leaf,
                path: Vec::new(),
            };
            let proof = CustodyProof {
                round: 0,
                leaves: vec![answer; 2],
            };
            assert_eq!(
                check_proof(&c, &proof, Pulse(11)).is_ok(),
                ok,
                "{length} bytes"
            );
        }
        // A perfect tree MAX_PATH levels deep verifies; one level deeper (a
        // size §5.50 rules out) verifies as a tree but fails the bound.
        for (depth, ok) in [(MAX_PATH, true), (MAX_PATH + 1, false)] {
            let size = 1u64 << depth;
            let index = challenged_leaf(&seed, 0, 0, size).unwrap();
            let leaf = b"deep".to_vec();
            let path: Vec<Hash32> = (0..depth).map(|level| Hash32([level as u8; 32])).collect();
            let root = perfect_root(merkle::leaf_hash(&leaf), index, &path);
            assert!(
                merkle::verify_inclusion(&root, size, index, &merkle::leaf_hash(&leaf), &path)
                    .is_ok()
            );
            let c = Custody {
                deal,
                root,
                size,
                challenges: 1,
                start: Pulse(11),
                round_length: 8,
                rounds: 5,
                next: 0,
                proven: 0,
            };
            let proof = CustodyProof {
                round: 0,
                leaves: vec![LeafProof { data: leaf, path }],
            };
            assert_eq!(
                check_proof(&c, &proof, Pulse(11)).is_ok(),
                ok,
                "{depth} levels"
            );
        }
    }

    #[test]
    fn the_reference_providers_answers_verify() {
        // Cross-check: a provider using the reference executor's client-side
        // helpers answers challenges this crate accepts.
        use sion_exec::storage::DataTree;
        let bytes: Vec<u8> = (0..5 * MAX_LEAF_BYTES + 123)
            .map(|i| (i % 251) as u8)
            .collect();
        let tree = DataTree::of_data(&bytes);
        let deal = ObjectId::derive(b"deal");
        let mut c = Custody {
            deal,
            root: tree.root(),
            size: tree.size(),
            challenges: MAX_CHALLENGES,
            start: Pulse(11),
            round_length: 8,
            rounds: 6,
            next: 0,
            proven: 0,
        };
        for round in 0..c.rounds {
            let pulse = Pulse(11 + u64::from(round) * 8 + 5);
            let beacon = Hash32([round as u8; 32]);
            let proof = tree.answer(&bytes, &c, round, &beacon).expect("an answer");
            assert_eq!(
                check_proof_with(&c, &proof, pulse, &seed_from(&beacon, &deal)).unwrap(),
                round
            );
            c.proven += 1;
            c.next = round + 1;
        }
    }

    proptest! {
        #[test]
        fn honest_answers_verify_and_altered_leaves_do_not(
            n in 1usize..200,
            challenges in 1..=MAX_CHALLENGES,
            seed in any::<[u8; 32]>(),
            which in any::<prop::sample::Index>(),
            flip in any::<u8>(),
        ) {
            let data = data(n);
            let mut c = custody(ObjectId::derive(b"deal"), &data);
            c.challenges = challenges;
            let seed = Hash32(seed);
            let good = data.answer_to(&c, 0, &seed);
            prop_assert_eq!(check_proof_with(&c, &good, Pulse(11), &seed).unwrap(), 0);
            let mut bad = good;
            let leaf = &mut bad.leaves[which.index(usize::from(challenges))];
            leaf.data.push(flip);
            prop_assert!(fails(&check_proof_with(&c, &bad, Pulse(11), &seed).map(|_| ())));
        }
    }

    // -- §5.50 PostDeal ----------------------------------------------------

    /// PostDeal's terms, editable.
    struct Posting {
        size: u64,
        challenges: u8,
        price: Amount,
        rounds: u32,
        round_length: u64,
        collateral: Amount,
        provider: Option<AccountId>,
        accept_by: Pulse,
        locator: String,
    }

    impl Posting {
        fn kind(&self) -> TxKind {
            TxKind::PostDeal {
                root: Hash32([9; 32]),
                size: self.size,
                challenges: self.challenges,
                price: self.price,
                rounds: self.rounds,
                round_length: self.round_length,
                collateral: self.collateral,
                provider: self.provider,
                accept_by: self.accept_by,
                locator: self.locator.clone(),
            }
        }
    }

    fn posting() -> Posting {
        Posting {
            size: 10,
            challenges: 3,
            price: PRICE,
            rounds: 5,
            round_length: 8,
            collateral: Amount(5_003),
            provider: Some(account(b"provider")),
            accept_by: Pulse(20),
            locator: "sion://data".into(),
        }
    }

    #[test]
    fn posting_draws_the_escrow_then_creates_the_deal() {
        let client = account(b"client");
        let provider = account(b"provider");
        let gas = coin(b"gas", client, 1_000_000);
        let tx = signed(client, &[&gas], posting().kind());
        let (result, outcome) = apply_over(&context(10), &[&gas], &tx);
        result.unwrap();
        // draw (§3.2) consumes the coin, charges the fee and mints the change
        // before the deal is created.
        assert_eq!(
            coin_of(created(&outcome, &tx, 0)),
            (Owner::Account(client), Amount(1_000_000 - 5_000 - 10_000))
        );
        let deal_id = created_id(&tx.digest(), 1);
        let object = created(&outcome, &tx, 1);
        assert_eq!(object.owner, Owner::Shared(object_domain("deal", &deal_id)));
        assert_eq!(object.version, Version(0));
        assert_eq!(
            deal_in(&outcome, &deal_id),
            Deal {
                client,
                provider: Some(provider),
                root: Hash32([9; 32]),
                size: 10,
                challenges: 3,
                price: PRICE,
                rounds: 5,
                round_length: 8,
                collateral: Amount(5_003),
                accept_by: Pulse(20),
                locator: "sion://data".into(),
                escrow: Amount(5_000),
                posted_at: Pulse(10),
                state: DealState::Open,
            }
        );
        assert_eq!(outcome.parts.consumed, ids(&[gas.id]));
        assert_eq!(
            outcome.parts.created,
            ids(&[created_id(&tx.digest(), 0), deal_id])
        );
        assert_eq!(
            outcome.deferred,
            vec![Deferred::Fee {
                asset: usd(),
                amount: FEE
            }]
        );
        assert_eq!(
            outcome.events,
            vec![Event::DealPosted {
                deal: deal_id,
                client,
                provider: Some(provider),
                size: 10,
                price: PRICE,
                rounds: 5,
                collateral: Amount(5_003),
            }]
        );
    }

    #[test]
    fn posting_checks_every_term_before_reading_anything() {
        let client = account(b"client");
        let gas = coin(b"gas", client, 1_000_000);
        let pulse = 10;
        let with = |edit: &dyn Fn(&mut Posting)| {
            let mut terms = posting();
            edit(&mut terms);
            terms
        };
        let refused = [
            ("no leaves", with(&|t| t.size = 0)),
            ("too many leaves", with(&|t| t.size = MAX_DEAL_LEAVES + 1)),
            ("no challenges", with(&|t| t.challenges = 0)),
            (
                "too many challenges",
                with(&|t| t.challenges = MAX_CHALLENGES + 1),
            ),
            ("free", with(&|t| t.price = Amount::ZERO)),
            ("no rounds", with(&|t| t.rounds = 0)),
            ("too many rounds", with(&|t| t.rounds = MAX_ROUNDS + 1)),
            (
                "short rounds",
                with(&|t| t.round_length = MIN_ROUND_LENGTH - 1),
            ),
            (
                "long rounds",
                with(&|t| t.round_length = MAX_ROUND_LENGTH + BEACON_INTERVAL),
            ),
            (
                "rounds off the beacon interval",
                with(&|t| t.round_length = MIN_ROUND_LENGTH + 1),
            ),
            (
                "open without collateral",
                with(&|t| {
                    t.provider = None;
                    t.collateral = Amount::ZERO;
                }),
            ),
            ("accept_by now", with(&|t| t.accept_by = Pulse(pulse))),
            (
                "accept_by too late",
                with(&|t| t.accept_by = Pulse(pulse + MAX_PERIOD + 1)),
            ),
            // 129 two-byte characters: 258 bytes.
            ("long locator", with(&|t| t.locator = "é".repeat(129))),
            (
                "escrow overflows",
                with(&|t| {
                    t.price = Amount(u128::MAX / 2 + 1);
                    t.rounds = 2;
                }),
            ),
        ];
        for (what, terms) in refused {
            // An empty witness: any read would reject the chunk instead.
            let tx = signed(client, &[&gas], terms.kind());
            let (result, outcome) = apply_over(&context(pulse), &[], &tx);
            assert!(fails(&result), "{what}: {result:?}");
            assert!(outcome.parts.reads.is_empty(), "{what}");
        }
        let limits = [
            ("most leaves", with(&|t| t.size = MAX_DEAL_LEAVES)),
            ("most challenges", with(&|t| t.challenges = MAX_CHALLENGES)),
            ("most rounds", with(&|t| t.rounds = MAX_ROUNDS)),
            (
                "shortest rounds",
                with(&|t| t.round_length = MIN_ROUND_LENGTH),
            ),
            (
                "longest rounds",
                with(&|t| t.round_length = MAX_ROUND_LENGTH),
            ),
            (
                "accept_by soonest",
                with(&|t| t.accept_by = Pulse(pulse + 1)),
            ),
            (
                "accept_by latest",
                with(&|t| t.accept_by = Pulse(pulse + MAX_PERIOD)),
            ),
            // 128 two-byte characters: 256 bytes.
            ("longest locator", with(&|t| t.locator = "é".repeat(128))),
            // A named provider may be asked for no collateral.
            (
                "named, no collateral",
                with(&|t| t.collateral = Amount::ZERO),
            ),
            ("open with collateral", with(&|t| t.provider = None)),
        ];
        for (what, terms) in limits {
            // These pass step 1, so draw reads the input.
            let tx = signed(client, &[&gas], terms.kind());
            let (result, _) = apply_over(&context(pulse), &[], &tx);
            let expected = Err(Abort::Chunk(SpecError::Unwitnessed(gas.id)));
            assert_eq!(result, expected, "{what}");
        }
        // Near the end of time the acceptance window saturates.
        let late = with(&|t| t.accept_by = Pulse(u64::MAX));
        let tx = signed(client, &[&gas], late.kind());
        let (result, _) = apply_over(&context(u64::MAX - 1), &[&gas], &tx);
        result.unwrap();
    }

    #[test]
    fn posting_fails_without_the_escrow() {
        let client = account(b"client");
        // The escrow (5,000) and the fee (10,000) need 15,000.
        let gas = coin(b"gas", client, 14_999);
        let tx = signed(client, &[&gas], posting().kind());
        let (result, _) = apply_over(&context(10), &[&gas], &tx);
        assert!(fails(&result));
    }

    // -- §5.51 AcceptDeal --------------------------------------------------

    #[test]
    fn accepting_posts_the_collateral_and_creates_the_custody_record() {
        let client = account(b"client");
        let provider = account(b"provider");
        let data = data(10);
        let deal_id = ObjectId::derive(b"deal");
        let deal = deal_at(deal_id, posted(client, Some(provider), &data));
        let gas = coin(b"gas", provider, 100_000);
        let tx = signed(provider, &[&gas], TxKind::AcceptDeal { deal: deal_id });
        let (result, outcome) = apply_over(&context(10), &[&deal, &gas], &tx);
        result.unwrap();
        assert_eq!(
            coin_of(created(&outcome, &tx, 0)),
            (Owner::Account(provider), Amount(100_000 - 5_003 - 10_000))
        );
        let custody_id = created_id(&tx.digest(), 1);
        assert_eq!(created(&outcome, &tx, 1).owner, Owner::Account(provider));
        assert_eq!(
            custody_in(&outcome, &custody_id),
            Custody {
                deal: deal_id,
                root: data.root,
                size: 10,
                challenges: 3,
                start: Pulse(16),
                round_length: 8,
                rounds: 5,
                next: 0,
                proven: 0,
            }
        );
        assert_eq!(outcome.parts.writes[&deal_id].version, Version(4));
        assert_eq!(
            deal_in(&outcome, &deal_id).state,
            DealState::Active {
                provider,
                custody: custody_id,
                start: Pulse(16),
                bond: Amount(5_003),
                paid: 0,
            }
        );
        assert!(outcome.parts.mutated.contains(&deal_id));
        assert_eq!(
            outcome.events,
            vec![Event::DealAccepted {
                deal: deal_id,
                provider,
                custody: custody_id,
                start: Pulse(16),
            }]
        );
        // Accepted on a multiple of the interval, rounds start on the next.
        let gas = coin(b"gas", provider, 100_000);
        let tx = signed(provider, &[&gas], TxKind::AcceptDeal { deal: deal_id });
        let (result, outcome) = apply_over(&context(16), &[&deal, &gas], &tx);
        result.unwrap();
        assert_eq!(
            custody_in(&outcome, &created_id(&tx.digest(), 1)).start,
            Pulse(24)
        );
    }

    #[test]
    fn only_the_named_provider_accepts_and_only_in_time() {
        let client = account(b"client");
        let provider = account(b"provider");
        let stranger = account(b"stranger");
        let data = data(10);
        let deal_id = ObjectId::derive(b"deal");
        let named = deal_at(deal_id, posted(client, Some(provider), &data));
        let open = deal_at(deal_id, posted(client, None, &data));
        let accept = |deal: &Object, sender: AccountId, pulse: u64| {
            let gas = coin(b"gas", sender, 100_000);
            let tx = signed(sender, &[&gas], TxKind::AcceptDeal { deal: deal_id });
            apply_over(&context(pulse), &[deal, &gas], &tx).0
        };
        assert!(fails(&accept(&named, stranger, 10)));
        assert!(fails(&accept(&named, provider, 21)), "after accept_by");
        accept(&named, provider, 20).unwrap();
        accept(&open, stranger, 20).unwrap();
        let mut taken = posted(client, None, &data);
        taken.state = DealState::Active {
            provider,
            custody: ObjectId::derive(b"custody"),
            start: Pulse(5),
            bond: Amount(5_003),
            paid: 0,
        };
        assert!(fails(&accept(&deal_at(deal_id, taken), stranger, 10)));
        // A coin is not a deal.
        let not_a_deal = Object {
            id: deal_id,
            ..coin(b"x", client, 1)
        };
        assert!(fails(&accept(&not_a_deal, provider, 10)));
    }

    // -- §5.52 ProveStorage ------------------------------------------------

    /// Two custody records of `provider`, for deals of 10 and 1,000 leaves:
    /// the first has proven round 0, the second nothing.
    fn two_records(provider: AccountId) -> [(Data, Object); 2] {
        let one = data(10);
        let two = data(1_000);
        let mut first = custody(ObjectId::derive(b"deal one"), &one);
        first.proven = 1;
        first.next = 1;
        let second = custody(ObjectId::derive(b"deal two"), &two);
        [
            (
                one,
                custody_at(ObjectId::derive(b"custody one"), provider, first),
            ),
            (
                two,
                custody_at(ObjectId::derive(b"custody two"), provider, second),
            ),
        ]
    }

    fn record(object: &Object) -> Custody {
        match &object.data {
            ObjectData::Custody(custody) => custody.clone(),
            other => panic!("not a custody record: {other:?}"),
        }
    }

    #[test]
    fn proving_moves_each_custody_record_to_its_next_round() {
        let provider = account(b"provider");
        let [(one, first), (two, second)] = two_records(provider);
        let gas = coin(b"gas", provider, 100_000);
        let proofs = vec![
            one.answer(&record(&first), 2),
            two.answer(&record(&second), 2),
        ];
        let tx = signed(
            provider,
            &[&gas, &first, &second],
            TxKind::ProveStorage { proofs },
        );
        // Round 2 is under way at Pulse 27. The gas coin is not witnessed:
        // proving does not read it.
        let (result, outcome) = apply_over(&context(27), &[&first, &second], &tx);
        result.unwrap();
        let before = record(&first);
        assert_eq!(
            custody_in(&outcome, &first.id),
            Custody {
                proven: 2,
                next: 3,
                ..before.clone()
            }
        );
        let before_second = record(&second);
        assert_eq!(
            custody_in(&outcome, &second.id),
            Custody {
                proven: 1,
                next: 3,
                ..before_second.clone()
            }
        );
        assert_eq!(outcome.parts.mutated, ids(&[first.id, second.id]));
        assert!(outcome.parts.created.is_empty());
        assert_eq!(
            outcome.events,
            vec![
                Event::StorageProven {
                    deal: before.deal,
                    custody: first.id,
                    round: 2,
                },
                Event::StorageProven {
                    deal: before_second.deal,
                    custody: second.id,
                    round: 2,
                },
            ]
        );
    }

    #[test]
    fn a_proof_after_its_rounds_first_pulse_reads_that_pulses_beacon_record() {
        // Rounds of 8 from Pulse 16; round 1 is Pulses 24 to 31, and the
        // chain recorded Pulse 24's beacon.
        let provider = account(b"provider");
        let data = data(100);
        let deal = ObjectId::derive(b"deal");
        let mut c = custody(deal, &data);
        c.start = Pulse(16);
        c.proven = 1;
        c.next = 1;
        let record = custody_at(ObjectId::derive(b"custody"), provider, c.clone());
        let recorded = Hash32([0x24; 32]);
        let beacon = beacon_object(24, recorded);
        let gas = coin(b"gas", provider, 100_000);
        let prove = |pulse: u64, proof: CustodyProof, witness: &[&Object]| {
            let tx = signed(
                provider,
                &[&gas, &record],
                TxKind::ProveStorage {
                    proofs: vec![proof],
                },
            );
            apply_over(&context(pulse), witness, &tx)
        };
        let honest = data.answer_to(&c, 1, &seed_from(&recorded, &deal));
        // Late in the round: the record is read and the answer verifies.
        let (result, outcome) = prove(31, honest.clone(), &[&record, &beacon]);
        result.unwrap();
        assert_eq!(outcome.parts.reads, ids(&[record.id, beacon.id]));
        assert_eq!(custody_in(&outcome, &record.id).next, 2);
        // Without the record in the witness, the chunk is rejected.
        let (result, _) = prove(27, honest.clone(), &[&record]);
        assert_eq!(result, Err(Abort::Chunk(SpecError::Unwitnessed(beacon.id))));
        // A slot that holds another Pulse's record fails the proof.
        let stale = beacon_object(24 + BEACON_INTERVAL * BEACON_SLOTS, recorded);
        assert_eq!(stale.id, beacon.id);
        let (result, _) = prove(27, honest.clone(), &[&record, &stale]);
        assert!(fails(&result), "{result:?}");
        // An answer to the challenge of the proof's own Pulse fails.
        let steered = data.answer_to(&c, 1, &seed_from(&BEACON, &deal));
        assert_ne!(steered, honest);
        let (result, _) = prove(27, steered, &[&record, &beacon]);
        assert!(fails(&result), "{result:?}");
        // In the round's first Pulse, the Pulse's own beacon is the round's:
        // nothing else is read.
        let first = data.answer_to(&c, 1, &seed_from(&BEACON, &deal));
        let (result, outcome) = prove(24, first, &[&record]);
        result.unwrap();
        assert_eq!(outcome.parts.reads, ids(&[record.id]));
    }

    #[test]
    fn proving_reads_each_record_only_after_the_proofs_before_it_pass() {
        let provider = account(b"provider");
        let [(one, first), (two, second)] = two_records(provider);
        let gas = coin(b"gas", provider, 100_000);
        // The first proof names the wrong round: the second record is never
        // read, so its absence from the witness does not matter.
        let proofs = vec![
            one.answer(&record(&first), 1),
            two.answer(&record(&second), 2),
        ];
        let tx = signed(
            provider,
            &[&gas, &first, &second],
            TxKind::ProveStorage { proofs },
        );
        let (result, outcome) = apply_over(&context(27), &[&first], &tx);
        assert!(fails(&result), "{result:?}");
        assert_eq!(outcome.parts.reads, ids(&[first.id]));
        // Once the first passes, the second is read.
        let proofs = vec![
            one.answer(&record(&first), 2),
            two.answer(&record(&second), 2),
        ];
        let tx = signed(
            provider,
            &[&gas, &first, &second],
            TxKind::ProveStorage { proofs },
        );
        let (result, _) = apply_over(&context(27), &[&first], &tx);
        assert_eq!(result, Err(Abort::Chunk(SpecError::Unwitnessed(second.id))));
    }

    #[test]
    fn proving_checks_the_batch_before_reading_any_record() {
        let provider = account(b"provider");
        let gas = coin(b"gas", provider, 100_000);
        let records: Vec<Object> = (0..5u8).map(|i| coin(&[b'r', i], provider, 1)).collect();
        let inputs: Vec<&Object> = std::iter::once(&gas).chain(records.iter()).collect();
        let proofs_of = |sizes: &[usize]| -> Vec<CustodyProof> {
            sizes
                .iter()
                .map(|&n| CustodyProof {
                    round: 0,
                    leaves: vec![
                        LeafProof {
                            data: Vec::new(),
                            path: Vec::new(),
                        };
                        n
                    ],
                })
                .collect()
        };
        let prove = |inputs: &[&Object], proofs: Vec<CustodyProof>| {
            let tx = signed(provider, inputs, TxKind::ProveStorage { proofs });
            apply_over(&context(27), &[], &tx)
        };
        // No proofs; more proofs than records; more than MAX_PROOF_LEAVES
        // leaves in all. Nothing is read.
        for (inputs, proofs) in [
            (&inputs[..], Vec::new()),
            (&inputs[..3], proofs_of(&[1, 1, 1])),
            (&inputs[..], proofs_of(&[13, 13, 13, 13, 13])),
        ] {
            let (result, outcome) = prove(inputs, proofs);
            assert!(fails(&result), "{result:?}");
            assert!(outcome.parts.reads.is_empty());
        }
        // Exactly MAX_PROOF_LEAVES leaves pass step 1.
        let (result, _) = prove(&inputs, proofs_of(&[16, 16, 16, 16]));
        assert_eq!(
            result,
            Err(Abort::Chunk(SpecError::Unwitnessed(records[0].id)))
        );
        // A record that is not a custody record fails.
        let tx = signed(
            provider,
            &inputs,
            TxKind::ProveStorage {
                proofs: proofs_of(&[1]),
            },
        );
        let (result, _) = apply_over(&context(27), &[&records[0]], &tx);
        assert!(fails(&result));
    }

    // -- §5.53 SettleDeal --------------------------------------------------

    fn settle_at(
        f: &Fixture,
        sender: AccountId,
        witness: &[&Object],
    ) -> (Step, RunOutcome, SignedTransaction) {
        let gas = coin(b"gas", sender, 100_000);
        let tx = signed(sender, &[&gas], TxKind::SettleDeal { deal: f.deal_id });
        let (result, outcome) = apply_over(&context(40), witness, &tx);
        (result, outcome, tx)
    }

    #[test]
    fn settling_pays_the_rounds_proven_since_the_last_settlement() {
        let f = accepted(3, 3, 1);
        let (result, outcome, tx) = settle_at(&f, f.provider, &[&f.deal, &f.custody]);
        result.unwrap();
        let deal = deal_in(&outcome, &f.deal_id);
        assert_eq!(deal.escrow, Amount(4_000 - 2_000));
        assert_eq!(
            deal.state,
            DealState::Active {
                provider: f.provider,
                custody: f.custody_id,
                start: Pulse(11),
                bond: Amount(5_003),
                paid: 3,
            }
        );
        assert_eq!(
            coin_of(created(&outcome, &tx, 0)),
            (Owner::Account(f.provider), Amount(2_000))
        );
        assert_eq!(
            outcome.events,
            vec![Event::DealSettled {
                deal: f.deal_id,
                rounds: 2,
                amount: Amount(2_000),
            }]
        );
        // The custody record is read, not written.
        assert!(outcome.parts.reads.contains(&f.custody_id));
        assert!(!outcome.parts.writes.contains_key(&f.custody_id));
    }

    #[test]
    fn settling_needs_unpaid_proven_rounds_of_an_accepted_deal() {
        let f = accepted(3, 3, 3);
        let (result, _, _) = settle_at(&f, f.provider, &[&f.deal, &f.custody]);
        assert!(fails(&result), "nothing is due");
        let data = data(10);
        let open = deal_at(f.deal_id, posted(f.client, Some(f.provider), &data));
        let (result, _, _) = settle_at(&f, f.provider, &[&open]);
        assert!(fails(&result), "not accepted");
        let f = accepted(3, 3, 1);
        let mut foreign = record(&f.custody);
        foreign.deal = ObjectId::derive(b"another deal");
        let foreign = custody_at(f.custody_id, f.provider, foreign);
        let (result, _, _) = settle_at(&f, f.provider, &[&f.deal, &foreign]);
        assert!(fails(&result), "another deal's record");
    }

    #[test]
    fn settling_checks_the_sender_before_reading_the_custody_record() {
        let f = accepted(3, 3, 1);
        // Only the deal is witnessed: a stranger fails without reading the
        // record...
        let (result, outcome, _) = settle_at(&f, account(b"stranger"), &[&f.deal]);
        assert!(fails(&result), "{result:?}");
        assert_eq!(outcome.parts.reads, ids(&[f.deal_id]));
        // ...the client too...
        let (result, _, _) = settle_at(&f, f.client, &[&f.deal]);
        assert!(fails(&result), "{result:?}");
        // ...while the provider reads it.
        let (result, _, _) = settle_at(&f, f.provider, &[&f.deal]);
        assert_eq!(
            result,
            Err(Abort::Chunk(SpecError::Unwitnessed(f.custody_id)))
        );
    }

    // -- §5.54 CloseDeal ---------------------------------------------------

    #[test]
    fn closing_an_open_deal_refunds_its_client() {
        let f = accepted(0, 0, 0);
        let data = data(10);
        let open = deal_at(f.deal_id, posted(f.client, None, &data));
        let (result, outcome, tx) = close_at(&f, f.client, 30, &[&open]);
        result.unwrap();
        assert_eq!(
            coin_of(created(&outcome, &tx, 0)),
            (Owner::Account(f.client), Amount(5_000))
        );
        let closed = deal_in(&outcome, &f.deal_id);
        assert_eq!(closed.escrow, Amount::ZERO);
        assert_eq!(
            closed.state,
            DealState::Closed {
                at: Pulse(30),
                proven: 0,
                missed: 0,
                refund: Amount(5_000),
                forfeited: Amount::ZERO,
            }
        );
        assert!(outcome.deferred.is_empty());
        assert_eq!(
            outcome.events,
            vec![Event::DealClosed {
                deal: f.deal_id,
                refund: Amount(5_000),
                paid: Amount::ZERO,
                returned: Amount::ZERO,
                forfeited: Amount::ZERO,
                missed: 0,
            }]
        );
        // Nobody else closes an open deal, and a closed deal stays closed.
        let (result, _, _) = close_at(&f, f.provider, 30, &[&open]);
        assert!(fails(&result));
        let closed = deal_at(f.deal_id, closed);
        let (result, _, _) = close_at(&f, f.client, 31, &[&closed]);
        assert!(fails(&result));
    }

    #[test]
    fn closing_reads_the_custody_record_before_checking_the_sender() {
        let f = accepted(3, 3, 1);
        let stranger = account(b"stranger");
        let (result, _, _) = close_at(&f, stranger, 51, &[&f.deal]);
        assert_eq!(
            result,
            Err(Abort::Chunk(SpecError::Unwitnessed(f.custody_id)))
        );
        let (result, _, _) = close_at(&f, stranger, 51, &[&f.deal, &f.custody]);
        assert!(fails(&result));
    }

    #[test]
    fn closing_at_the_end_returns_the_bond_less_each_missed_round() {
        // Proven rounds 0, 2 and 4 (1 and 3 missed); paid for one of them.
        let f = accepted(3, 5, 1);
        for closer in [f.client, f.provider] {
            let (result, outcome, tx) = close_at(&f, closer, 51, &[&f.deal, &f.custody]);
            result.unwrap();
            // The provider is paid for the two unpaid proven rounds and gets
            // its bond back less ⌊5,003 × 2 / 5⌋ = 2,001, in one coin minted
            // before the client's.
            assert_eq!(
                coin_of(created(&outcome, &tx, 0)),
                (Owner::Account(f.provider), Amount(2_000 + 3_002))
            );
            // The client gets back the escrow of the rounds not proven.
            assert_eq!(
                coin_of(created(&outcome, &tx, 1)),
                (Owner::Account(f.client), Amount(2_000))
            );
            assert_eq!(
                outcome.deferred,
                vec![Deferred::Treasury {
                    asset: usd(),
                    amount: Amount(2_001)
                }]
            );
            let closed = deal_in(&outcome, &f.deal_id);
            assert_eq!(closed.escrow, Amount::ZERO);
            assert_eq!(
                closed.state,
                DealState::Closed {
                    at: Pulse(51),
                    proven: 3,
                    missed: 2,
                    refund: Amount(2_000),
                    forfeited: Amount(2_001),
                }
            );
            assert_eq!(
                outcome.events,
                vec![Event::DealClosed {
                    deal: f.deal_id,
                    refund: Amount(2_000),
                    paid: Amount(2_000),
                    returned: Amount(3_002),
                    forfeited: Amount(2_001),
                    missed: 2,
                }]
            );
        }
        // A provider that proved and collected every round gets its whole
        // bond back; the client has nothing left to refund.
        let f = accepted(5, 5, 5);
        let (result, outcome, tx) = close_at(&f, f.provider, 60, &[&f.deal, &f.custody]);
        result.unwrap();
        assert_eq!(
            coin_of(created(&outcome, &tx, 0)),
            (Owner::Account(f.provider), Amount(5_003))
        );
        assert_eq!(outcome.parts.created, ids(&[created_id(&tx.digest(), 0)]));
        assert!(outcome.deferred.is_empty());
        // Nobody else closes it.
        let (result, _, _) = close_at(&f, account(b"stranger"), 60, &[&f.deal, &f.custody]);
        assert!(fails(&result));
    }

    #[test]
    fn a_client_closes_a_deal_its_provider_abandoned() {
        // Proven round 0 only, not paid for. At Pulse 42 round 3 is under way:
        // rounds 1 and 2 passed unproven, fewer than ABANDON_ROUNDS.
        let f = accepted(1, 1, 0);
        let (result, _, _) = close_at(&f, f.client, 42, &[&f.deal, &f.custody]);
        assert!(fails(&result));
        // From round 4 (Pulse 43) the client may close, and the bond is
        // forfeited.
        let (result, outcome, tx) = close_at(&f, f.client, 43, &[&f.deal, &f.custody]);
        result.unwrap();
        assert_eq!(
            coin_of(created(&outcome, &tx, 0)),
            (Owner::Account(f.provider), Amount(1_000))
        );
        assert_eq!(
            coin_of(created(&outcome, &tx, 1)),
            (Owner::Account(f.client), Amount(4_000))
        );
        assert_eq!(
            outcome.deferred,
            vec![Deferred::Treasury {
                asset: usd(),
                amount: Amount(5_003)
            }]
        );
        assert_eq!(
            deal_in(&outcome, &f.deal_id).state,
            DealState::Closed {
                at: Pulse(43),
                proven: 1,
                missed: 3,
                refund: Amount(4_000),
                forfeited: Amount(5_003),
            }
        );
        assert_eq!(
            outcome.events,
            vec![Event::DealClosed {
                deal: f.deal_id,
                refund: Amount(4_000),
                paid: Amount(1_000),
                returned: Amount::ZERO,
                forfeited: Amount(5_003),
                missed: 3,
            }]
        );
        // Before the end, the provider cannot close it.
        let (result, _, _) = close_at(&f, f.provider, 43, &[&f.deal, &f.custody]);
        assert!(fails(&result));
        // A provider that never proved owes nothing and is paid nothing: the
        // client's refund is the first coin.
        let f = accepted(0, 0, 0);
        let (result, outcome, tx) = close_at(&f, f.client, 35, &[&f.deal, &f.custody]);
        result.unwrap();
        assert_eq!(
            coin_of(created(&outcome, &tx, 0)),
            (Owner::Account(f.client), Amount(5_000))
        );
        assert_eq!(outcome.parts.created, ids(&[created_id(&tx.digest(), 0)]));
        // Nothing can be abandoned before its first round.
        let (result, _, _) = close_at(&f, f.client, 10, &[&f.deal, &f.custody]);
        assert!(fails(&result));
    }

    proptest! {
        /// Closing pays out exactly what the deal holds: the escrow left and
        /// the bond go to the provider, the client and the treasury.
        #[test]
        fn closing_conserves_the_escrow_and_the_bond(
            (rounds, proven, next, paid) in (1u32..=40).prop_flat_map(|rounds| {
                (0..=rounds).prop_flat_map(move |proven| {
                    (proven..=rounds).prop_flat_map(move |next| {
                        (0..=proven).prop_map(move |paid| (rounds, proven, next, paid))
                    })
                })
            }),
            price in 1u128..=1_000_000_000_000,
            bond in 0u128..=1_000_000_000_000_000,
            round_length in MIN_ROUND_LENGTH..=20,
            pulse in 0u64..1_000,
            by_client in any::<bool>(),
        ) {
            let client = account(b"client");
            let provider = account(b"provider");
            let mut deal = posted(client, Some(provider), &data(1));
            deal.rounds = rounds;
            deal.round_length = round_length;
            deal.price = Amount(price);
            let escrow = Amount(price * u128::from(rounds - paid));
            deal.escrow = escrow;
            let mut c = custody(ObjectId::derive(b"deal"), &data(1));
            c.proven = proven;
            c.next = next;
            let start = Pulse(11);
            let accepted = Accepted { provider, start, bond: Amount(bond), paid };
            let sender = if by_client { client } else { provider };
            let end = deal_end(start, rounds, round_length);
            match close_accepted(&deal, &c, &accepted, Pulse(pulse), &sender) {
                Ok(closing) => {
                    let refund = escrow.checked_sub(closing.paid_now).unwrap();
                    prop_assert_eq!(
                        closing.paid_now.0 + closing.returned.0 + refund.0 + closing.forfeited.0,
                        escrow.0 + bond
                    );
                    prop_assert_eq!(closing.proven, proven);
                    prop_assert_eq!(closing.paid_now.0, price * u128::from(proven - paid));
                    if Pulse(pulse) >= end {
                        prop_assert_eq!(closing.missed, rounds - proven);
                        prop_assert_eq!(
                            closing.forfeited.0,
                            bond * u128::from(rounds - proven) / u128::from(rounds)
                        );
                    } else {
                        prop_assert!(by_client);
                        prop_assert_eq!(closing.forfeited.0, bond);
                        prop_assert_eq!(closing.returned, Amount::ZERO);
                    }
                }
                Err(_) => {
                    // Refused only before the end, for the provider, or
                    // before the provider has missed ABANDON_ROUNDS rounds.
                    prop_assert!(Pulse(pulse) < end);
                    let abandoned = round_at(start, round_length, Pulse(pulse))
                        .is_some_and(|r| r >= u64::from(next) + u64::from(ABANDON_ROUNDS));
                    prop_assert!(!by_client || !abandoned);
                }
            }
        }
    }
}
