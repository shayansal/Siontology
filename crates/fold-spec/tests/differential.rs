//! Differential tests for proof stack B.
//!
//! The reference executor (stack A, `fold_exec::sim::Chain`) runs workloads
//! that exercise every transaction kind and the deferred phase, recording
//! the honest chunk traces of every Pulse. Stack B
//! ([`fold_spec::verify_chunk`]), written independently from the
//! specification, must accept every one of them, and must reject traces
//! whose statements, witnesses, transactions or deferred items were
//! tampered with.
//!
//! When stack B rejects an honest chunk, the failure message diffs stack
//! B's execution ([`fold_spec::execute_chunk`]) against the effects and
//! writes stack A recorded, transaction by transaction and object by object.

use std::collections::{BTreeMap, BTreeSet};

use fold_assurance::{AtomClass, Formula, Policy};
use fold_clearing::Side;
use fold_core::hash::{hash_domain, Hash32};
use fold_core::ids::{AccountId, AssetId, ClaimId, ObjectId, Pulse, Version};
use fold_core::Amount;
use fold_crypto::{CommitteeEnvelope, KemKeypair};
use fold_exec::charter::Caveats;
use fold_exec::claim::{claim_id, formulas, ClaimBody, Payload};
use fold_exec::execute::{bond_atom, registered_atom, Deferred, Event, ExecStatus};
use fold_exec::market::{BatchStatus, Market, OrderTerms, Submission, SEALED_ORDER_DOMAIN};
use fold_exec::object::{created_id, well_known, Object, ObjectData, ObjectRef, ScheduledEvent};
use fold_exec::pulse::ChunkTrace;
use fold_exec::sim::Chain;
use fold_exec::{PulseOutput, SignedTransaction, Transaction, TxKind};
use fold_spec::commit::{chain_deferred, deferred_coin_id, effects_leaf};
use fold_spec::{execute_chunk, verify_chunk, ChunkReport, SpecError};
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn usd() -> AssetId {
    AssetId::usd()
}

fn gpu() -> AssetId {
    AssetId::derive(b"gpu-hour")
}

fn units(n: u64) -> Amount {
    Amount::units(n)
}

fn cents(c: u128) -> Amount {
    Amount(c * 10_000)
}

/// The outcome class of a transaction (what effects commit to).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Success,
    Failed,
    Rejected,
}

fn outcome_of(status: &ExecStatus) -> Outcome {
    match status {
        ExecStatus::Success => Outcome::Success,
        ExecStatus::Failed(_) => Outcome::Failed,
        ExecStatus::Rejected(_) => Outcome::Rejected,
    }
}

fn kind_name(kind: &TxKind) -> &'static str {
    match kind {
        TxKind::Transfer { .. } => "Transfer",
        TxKind::Faucet { .. } => "Faucet",
        TxKind::RotatePolicy { .. } => "RotatePolicy",
        TxKind::IssueClaim { .. } => "IssueClaim",
        TxKind::RegisterAtom { .. } => "RegisterAtom",
        TxKind::PostBond { .. } => "PostBond",
        TxKind::Accept { .. } => "Accept",
        TxKind::Release { .. } => "Release",
        TxKind::Challenge { .. } => "Challenge",
        TxKind::Rule { .. } => "Rule",
        TxKind::WithdrawBond { .. } => "WithdrawBond",
        TxKind::CreateCharter { .. } => "CreateCharter",
        TxKind::Delegate { .. } => "Delegate",
        TxKind::AgentPay { .. } => "AgentPay",
        TxKind::RevokeCharter { .. } => "RevokeCharter",
        TxKind::ReclaimCharter { .. } => "ReclaimCharter",
        TxKind::CreateBounded { .. } => "CreateBounded",
        TxKind::SpendAllotment { .. } => "SpendAllotment",
        TxKind::ReturnAllotment => "ReturnAllotment",
        TxKind::Rebalance { .. } => "Rebalance",
        TxKind::CreateMarket { .. } => "CreateMarket",
        TxKind::SubmitOrder { .. } => "SubmitOrder",
        TxKind::RevealBatch { .. } => "RevealBatch",
        TxKind::ClearBatch { .. } => "ClearBatch",
    }
}

fn outcome_tag(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Success => "ok",
        Outcome::Failed => "failed",
        Outcome::Rejected => "rejected",
    }
}

fn deferred_name(item: &Deferred) -> &'static str {
    match item {
        Deferred::IssueClaim { .. } => "IssueClaim",
        Deferred::RegisterAtom { .. } => "RegisterAtom",
        Deferred::PostBond { .. } => "PostBond",
        Deferred::AddOrder { .. } => "AddOrder",
        Deferred::ReturnToBounded { .. } => "ReturnToBounded",
        Deferred::Treasury { .. } => "Treasury",
        Deferred::Minted { .. } => "Minted",
        Deferred::Schedule { .. } => "Schedule",
    }
}

fn event_name(event: &Event) -> String {
    let text = format!("{event:?}");
    text.split([' ', '{', '(']).next().unwrap_or("").to_owned()
}

fn scheduled_name(event: &ScheduledEvent) -> &'static str {
    match event {
        ScheduledEvent::SealBatch { .. } => "SealBatch",
        ScheduledEvent::BatchDeadline { .. } => "BatchDeadline",
        ScheduledEvent::RegistryTick => "RegistryTick",
    }
}

/// Explains why stack B rejected an honest chunk, by diffing its execution
/// against what stack A recorded.
fn diagnose(trace: &ChunkTrace, error: &SpecError) -> String {
    let statement = &trace.statement;
    let mut out = format!(
        "stack B rejected an honest chunk (pulse {}, index {}, deferred phase {}, {} transactions, {} witness objects): {error}\n",
        statement.pulse.0,
        statement.index,
        statement.deferred_phase,
        trace.transactions.len(),
        trace.witness.objects.len(),
    );
    match execute_chunk(trace) {
        Err(stop) => {
            out.push_str(&format!("execution stopped: {stop}\nstack A wrote:\n"));
            for object in &trace.writes {
                out.push_str(&format!(
                    "  {} v{}: {}\n",
                    object.id,
                    object.version.0,
                    describe(object)
                ));
            }
        }
        Ok(execution) => {
            for (position, (mine, theirs)) in
                execution.effects.iter().zip(&trace.effects).enumerate()
            {
                if effects_leaf(mine) != effects_leaf(theirs) {
                    out.push_str(&format!(
                        "transaction {position} ({}) effects differ:\n  stack B: {mine:#?}\n  stack A: {theirs:#?}\n",
                        kind_name(&trace.transactions[position].tx.kind)
                    ));
                }
            }
            let theirs: BTreeMap<ObjectId, Object> =
                trace.writes.iter().map(|o| (o.id, o.clone())).collect();
            let ids: BTreeSet<ObjectId> = execution
                .writes
                .keys()
                .chain(theirs.keys())
                .copied()
                .collect();
            for id in ids {
                let mine = execution.writes.get(&id);
                let other = theirs.get(&id);
                if mine != other {
                    out.push_str(&format!(
                        "write {id} differs:\n  stack B: {mine:#?}\n  stack A: {other:#?}\n"
                    ));
                }
            }
        }
    }
    out
}

/// A one-line description of an object (schedules and markets in detail).
fn describe(object: &Object) -> String {
    match &object.data {
        ObjectData::Schedule(schedule) => format!("schedule {:?}", schedule.due),
        ObjectData::Market(market) => {
            let batches: Vec<String> = market
                .batches
                .values()
                .map(|batch| format!("batch {} {:?}", batch.index, batch.status))
                .collect();
            format!("market {batches:?}")
        }
        other => {
            let text = format!("{other:?}");
            text.split([' ', '(', '{']).next().unwrap_or("").to_owned()
        }
    }
}

/// Verifies one honest chunk with stack B, panicking with a diagnosis if it
/// is rejected.
fn accept(trace: &ChunkTrace) -> ChunkReport {
    match verify_chunk(trace) {
        Ok(report) => {
            assert_eq!(report.transactions, trace.transactions.len());
            assert_eq!(report.statement_digest, trace.statement.digest());
            report
        }
        Err(error) => panic!("{}", diagnose(trace, &error)),
    }
}

/// A reference chain whose every chunk stack B verifies.
struct Harness {
    chain: Chain,
    /// History records already verified.
    verified: usize,
    /// Chunks verified.
    chunks: usize,
    /// Transactions verified.
    transactions: usize,
    /// What the verified chunks exercised.
    coverage: BTreeSet<String>,
    /// Witness entries stack A included that stack B neither read nor
    /// wrote (spec §2.1: an implementation must not read more than the
    /// specification requires).
    unread: Vec<String>,
}

impl Harness {
    fn new(names: &[&str], usd: u64, coins: u32, chunk_size: usize) -> Harness {
        let mut chain = Chain::new(names, usd, coins);
        chain.chunk_size = chunk_size;
        Harness {
            chain,
            verified: 0,
            chunks: 0,
            transactions: 0,
            coverage: BTreeSet::new(),
            unread: Vec::new(),
        }
    }

    fn id(&self, name: &str) -> AccountId {
        self.chain.id(name)
    }

    fn tx(&mut self, name: &str, kind: TxKind) -> SignedTransaction {
        self.chain
            .tx(name, kind)
            .unwrap_or_else(|e| panic!("{name} cannot build the transaction: {e}"))
    }

    fn tx_with(&mut self, name: &str, kind: TxKind, extra: Vec<ObjectRef>) -> SignedTransaction {
        self.chain
            .tx_with(name, kind, extra)
            .unwrap_or_else(|e| panic!("{name} cannot build the transaction: {e}"))
    }

    /// Signs an edited transaction body with `name`'s keys, for this chain.
    fn resign(&self, name: &str, body: Transaction) -> SignedTransaction {
        body.sign(&self.chain.wallet(name).keys, &self.chain.state.config)
            .expect("signing")
    }

    fn reference(&self, id: &ObjectId) -> ObjectRef {
        self.chain.state.objects[id].reference()
    }

    fn pulse_number(&self) -> u64 {
        self.chain.state.pulse.0
    }

    fn pulse(&mut self, txs: Vec<SignedTransaction>) -> PulseOutput {
        let out = self
            .chain
            .pulse(txs)
            .expect("the reference executes the Pulse");
        self.verify_new();
        out
    }

    fn advance_to(&mut self, pulse: u64) {
        self.chain.advance_to(Pulse(pulse)).expect("empty Pulses");
        self.verify_new();
    }

    fn advance(&mut self, by: u64) {
        let target = self.pulse_number() + by;
        self.advance_to(target);
    }

    fn fund(&mut self, grants: &[(&str, AssetId, Amount)]) -> PulseOutput {
        let out = self.chain.fund(grants).expect("faucet Pulse");
        self.verify_new();
        out
    }

    fn balance(&self, name: &str, asset: &AssetId) -> Amount {
        self.chain.balance(name, asset)
    }

    /// Verifies every chunk of every Pulse not yet verified.
    fn verify_new(&mut self) {
        let records = &self.chain.history[self.verified..];
        for record in records {
            assert!(!record.traces.is_empty());
            assert!(record.traces.last().unwrap().statement.deferred_phase);
            for trace in &record.traces {
                let report = accept(trace);
                self.chunks += 1;
                self.transactions += report.transactions;
                for (signed, effects) in trace.transactions.iter().zip(&trace.effects) {
                    self.coverage.insert(format!(
                        "{}/{}",
                        kind_name(&signed.tx.kind),
                        outcome_tag(outcome_of(&effects.status))
                    ));
                }
                if trace.statement.deferred_phase {
                    record_deferred_coverage(trace, &mut self.coverage);
                }
                self.unread.extend(unread_witness_entries(trace));
            }
        }
        self.verified = self.chain.history.len();
    }

    /// Prints what was verified (visible with `--nocapture`).
    fn summary(&self, name: &str) {
        eprintln!(
            "{name}: stack B verified {} chunks of {} Pulses ({} transactions); coverage: {:?}; unread witness entries: {}",
            self.chunks,
            self.verified,
            self.transactions,
            self.coverage,
            self.unread.len()
        );
        for entry in &self.unread {
            eprintln!("  unread: {entry}");
        }
    }

    /// Asserts stack A's witnesses held nothing stack B did not need.
    fn assert_minimal_witnesses(&self) {
        assert!(
            self.unread.is_empty(),
            "witness entries stack B never touched:\n{}",
            self.unread.join("\n")
        );
    }

    fn assert_covered(&self, expected: &[&str]) {
        let missing: Vec<&&str> = expected
            .iter()
            .filter(|key| !self.coverage.contains(**key))
            .collect();
        assert!(
            missing.is_empty(),
            "not exercised: {missing:?}\ncovered: {:?}",
            self.coverage
        );
    }
}

/// Witness entries of an honest chunk that stack B's execution neither read
/// nor wrote, described for a failure message.
fn unread_witness_entries(trace: &ChunkTrace) -> Vec<String> {
    let execution = execute_chunk(trace).expect("verified chunks execute");
    let statement = &trace.statement;
    let mut unread = Vec::new();
    for (position, (id, object, _)) in trace.witness.objects.iter().enumerate() {
        if execution.reads.contains(id) || execution.writes.contains_key(id) {
            continue;
        }
        let what = match object {
            None => "absent object".to_owned(),
            Some(object) => {
                let text = format!("{:?}", object.data);
                text.split([' ', '(', '{']).next().unwrap_or("").to_owned()
            }
        };
        let kinds: Vec<&str> = trace
            .transactions
            .iter()
            .map(|t| kind_name(&t.tx.kind))
            .collect();
        unread.push(format!(
            "pulse {} chunk {} (deferred {}, transactions {kinds:?}): entry {position} {id} ({what})",
            statement.pulse.0, statement.index, statement.deferred_phase
        ));
    }
    unread
}

/// Records which deferred items, due events and deferred-phase events a
/// deferred chunk exercised.
fn record_deferred_coverage(trace: &ChunkTrace, coverage: &mut BTreeSet<String>) {
    let pulse = trace.statement.pulse;
    for item in &trace.deferred_items {
        coverage.insert(format!("deferred/{}", deferred_name(item)));
        if let Deferred::Schedule { at, event } = item {
            if *at <= pulse {
                coverage.insert(format!("due/{}", scheduled_name(event)));
            }
        }
    }
    for (id, object, _) in &trace.witness.objects {
        if *id != well_known::schedule() {
            continue;
        }
        if let Some(Object {
            data: ObjectData::Schedule(schedule),
            ..
        }) = object
        {
            for (_, events) in schedule.due.range(..=pulse.0) {
                for event in events {
                    coverage.insert(format!("due/{}", scheduled_name(event)));
                }
            }
        }
    }
    let execution = execute_chunk(trace).expect("verified chunks execute");
    for event in &execution.deferred_events {
        coverage.insert(format!("deferred-event/{}", event_name(event)));
    }
    if execution.writes.contains_key(&deferred_coin_id(pulse, 0)) {
        coverage.insert("deferred/payout".into());
    }
    // Treasury growth beyond the phase's own credits came from registry
    // ticks releasing slashing pools.
    let usd_balance = |object: Option<&Object>| match object.map(|o| &o.data) {
        Some(ObjectData::Treasury(treasury)) => treasury
            .balances
            .get(&usd())
            .copied()
            .unwrap_or(Amount::ZERO),
        _ => Amount::ZERO,
    };
    let before = trace
        .witness
        .objects
        .iter()
        .find(|(id, _, _)| *id == well_known::treasury())
        .and_then(|(_, object, _)| object.as_ref());
    if let Some(after) = execution.writes.get(&well_known::treasury()) {
        let credited: u128 = trace
            .deferred_items
            .iter()
            .filter_map(|item| match item {
                Deferred::Treasury { asset, amount } if *asset == usd() => Some(amount.0),
                _ => None,
            })
            .sum();
        if usd_balance(Some(after)).0 > usd_balance(before).0 + credited {
            coverage.insert("due/RegistryTick-releases-pools".into());
        }
    }
}

fn status_of(out: &PulseOutput, tx: &SignedTransaction) -> ExecStatus {
    out.effects
        .iter()
        .find(|e| e.digest == tx.digest())
        .map(|e| e.status.clone())
        .expect("transaction executed")
}

/// Checks what stack A decided, so each scenario provably exercises the
/// path it is meant to.
fn expect(out: &PulseOutput, tx: &SignedTransaction, outcome: Outcome) {
    let status = status_of(out, tx);
    assert_eq!(
        outcome_of(&status),
        outcome,
        "{} ended {status:?}",
        kind_name(&tx.tx.kind)
    );
}

fn all_events(out: &PulseOutput) -> Vec<Event> {
    out.effects
        .iter()
        .flat_map(|e| e.events.iter().cloned())
        .chain(out.deferred_events.iter().cloned())
        .collect()
}

fn note(subject: &str, formula: Formula) -> ClaimBody {
    ClaimBody {
        schema: "test.note/v1".into(),
        subject: subject.into(),
        payload: Payload::Public(subject.as_bytes().to_vec()),
        formula,
        adjudicator: None,
        detection_permille: 1000,
        valid_for: 100,
        challenge_period: 20,
    }
}

// ---------------------------------------------------------------------------
// Scenario: coins, accounts and every way preparation can reject
// ---------------------------------------------------------------------------

fn coins_accounts_and_outcomes(chunk_size: usize) -> Harness {
    let mut h = Harness::new(&["alice", "bob", "carol", "lab"], 1_000, 4, chunk_size);
    let (alice, bob, carol) = (h.id("alice"), h.id("bob"), h.id("carol"));

    // The faucet mints a non-USD asset.
    let out = h.fund(&[("lab", gpu(), units(50)), ("alice", gpu(), units(5))]);
    assert!(out.effects.iter().all(|e| e.succeeded()));

    // Multi-output transfers, a non-USD transfer (fee from the gas coin),
    // a zero output and an overdraft (both failed), a dependent spend of a
    // coin created earlier in the same Pulse.
    let multi = h.tx(
        "alice",
        TxKind::Transfer {
            asset: usd(),
            outputs: vec![(bob, units(10)), (carol, units(20)), (bob, units(5))],
        },
    );
    let goods = h.tx(
        "lab",
        TxKind::Transfer {
            asset: gpu(),
            outputs: vec![(alice, units(7)), (carol, units(3))],
        },
    );
    let zero = h.tx(
        "bob",
        TxKind::Transfer {
            asset: usd(),
            outputs: vec![(carol, Amount::ZERO)],
        },
    );
    let mut overdraft = h
        .tx(
            "carol",
            TxKind::Transfer {
                asset: usd(),
                outputs: vec![(alice, units(1))],
            },
        )
        .tx;
    overdraft.kind = TxKind::Transfer {
        asset: usd(),
        outputs: vec![(alice, units(900_000))],
    };
    let overdraft = h.resign("carol", overdraft);
    // Carol spends the coin Alice's transfer creates for her (index 2: change, bob, carol).
    let carol_coin = created_id(&multi.digest(), 2);
    let mut dependent = Transaction {
        sender: carol,
        genesis_policy: None,
        inputs: vec![ObjectRef {
            id: carol_coin,
            version: Version(0),
        }],
        fee: Amount::ZERO,
        kind: TxKind::Transfer {
            asset: usd(),
            outputs: vec![(bob, units(4))],
        },
        expires_at: Pulse(h.pulse_number() + 10),
        nonce: 7,
    };
    dependent.fee = h.chain.state.config.min_fee(dependent.size());
    let dependent = h.resign("carol", dependent);
    let out = h.pulse(vec![
        dependent.clone(),
        multi.clone(),
        goods.clone(),
        zero.clone(),
        overdraft.clone(),
    ]);
    expect(&out, &multi, Outcome::Success);
    expect(&out, &goods, Outcome::Success);
    expect(&out, &dependent, Outcome::Success);
    expect(&out, &zero, Outcome::Failed);
    expect(&out, &overdraft, Outcome::Failed);
    assert_eq!(h.balance("carol", &gpu()), units(3));

    // Preparation rejects: replay, bad signature, expiry, low fee, faucet
    // impostor, non-USD gas, duplicate input, someone else's coin.
    let replay = multi.clone();
    let mut forged = h.tx(
        "bob",
        TxKind::Transfer {
            asset: usd(),
            outputs: vec![(alice, units(1))],
        },
    );
    forged.auth.signatures[0].signature.bytes[7] ^= 1;
    let mut expired = h
        .tx(
            "alice",
            TxKind::Transfer {
                asset: usd(),
                outputs: vec![(bob, units(1))],
            },
        )
        .tx;
    expired.expires_at = Pulse(h.pulse_number());
    let expired = h.resign("alice", expired);
    let mut cheap = h
        .tx(
            "carol",
            TxKind::Transfer {
                asset: usd(),
                outputs: vec![(bob, units(1))],
            },
        )
        .tx;
    cheap.fee = Amount(1);
    let cheap = h.resign("carol", cheap);
    let mut impostor = h
        .tx(
            "bob",
            TxKind::Transfer {
                asset: usd(),
                outputs: vec![(alice, units(2))],
            },
        )
        .tx;
    impostor.kind = TxKind::Faucet {
        grants: vec![(bob, usd(), units(1_000))],
    };
    let impostor = h.resign("bob", impostor);
    let mut goods_gas = h
        .tx(
            "lab",
            TxKind::Transfer {
                asset: gpu(),
                outputs: vec![(bob, units(1))],
            },
        )
        .tx;
    let gpu_position = goods_gas
        .inputs
        .iter()
        .position(|input| {
            h.chain.state.objects[&input.id]
                .as_coin()
                .is_some_and(|coin| coin.asset == gpu())
        })
        .expect("a gpu coin input");
    goods_gas.inputs.swap(0, gpu_position);
    let goods_gas = h.resign("lab", goods_gas);
    let mut duplicate = h
        .tx(
            "alice",
            TxKind::Transfer {
                asset: usd(),
                outputs: vec![(carol, units(1))],
            },
        )
        .tx;
    duplicate.inputs.push(duplicate.inputs[0]);
    let duplicate = h.resign("alice", duplicate);
    let bobs_coin = h.chain.state.coins_of(&bob)[0].reference();
    let mut theft = h
        .tx(
            "carol",
            TxKind::Transfer {
                asset: usd(),
                outputs: vec![(carol, units(1))],
            },
        )
        .tx;
    theft.inputs.push(bobs_coin);
    let theft = h.resign("carol", theft);
    let rejected = vec![
        replay, forged, expired, cheap, impostor, goods_gas, duplicate, theft,
    ];
    let out = h.pulse(rejected.clone());
    for tx in &rejected {
        expect(&out, tx, Outcome::Rejected);
    }

    // Policy rotation: an invalid policy fails, a valid one succeeds, the
    // old keys are then rejected and the new keys accepted.
    let mut weak = fold_exec::Wallet::named("alice-weak").policy;
    weak.min_families = 1;
    let invalid = h.tx(
        "alice",
        TxKind::RotatePolicy {
            policy: weak,
            viewing_key: None,
        },
    );
    let out = h.pulse(vec![invalid.clone()]);
    expect(&out, &invalid, Outcome::Failed);
    let fresh = fold_exec::Wallet::named("alice-rotated");
    let rotate = h.tx(
        "alice",
        TxKind::RotatePolicy {
            policy: fresh.policy.clone(),
            viewing_key: Some(KemKeypair::from_seed(&[42; 32]).public().clone()),
        },
    );
    let out = h.pulse(vec![rotate.clone()]);
    expect(&out, &rotate, Outcome::Success);
    let stale = h.tx(
        "alice",
        TxKind::Transfer {
            asset: usd(),
            outputs: vec![(bob, units(1))],
        },
    );
    let out = h.pulse(vec![stale.clone()]);
    expect(&out, &stale, Outcome::Rejected);
    let mut rotated = fold_exec::Wallet::with_keys(fresh.keys.clone());
    rotated.id = alice;
    h.chain.wallets.insert("alice".into(), rotated);
    let ok = h.tx(
        "alice",
        TxKind::Transfer {
            asset: usd(),
            outputs: vec![(bob, units(1))],
        },
    );
    let out = h.pulse(vec![ok.clone()]);
    expect(&out, &ok, Outcome::Success);

    // New accounts come into existence with their first transaction, even
    // a failed one; without a genesis policy they are rejected.
    let dave = h.chain.add_wallet("dave");
    let erin = h.chain.add_wallet("erin");
    let frank = h.chain.add_wallet("frank");
    let gift = h.tx(
        "bob",
        TxKind::Transfer {
            asset: usd(),
            outputs: vec![(dave, units(50)), (erin, units(50)), (frank, units(50))],
        },
    );
    h.pulse(vec![gift]);
    let first = h.tx(
        "dave",
        TxKind::Transfer {
            asset: usd(),
            outputs: vec![(alice, units(5))],
        },
    );
    let failing_first = h.tx(
        "erin",
        TxKind::Transfer {
            asset: usd(),
            outputs: vec![(alice, Amount::ZERO)],
        },
    );
    let mut anonymous = h
        .tx(
            "frank",
            TxKind::Transfer {
                asset: usd(),
                outputs: vec![(alice, units(5))],
            },
        )
        .tx;
    assert!(anonymous.genesis_policy.is_some());
    anonymous.genesis_policy = None;
    let anonymous = h.resign("frank", anonymous);
    let out = h.pulse(vec![
        first.clone(),
        failing_first.clone(),
        anonymous.clone(),
    ]);
    expect(&out, &first, Outcome::Success);
    expect(&out, &failing_first, Outcome::Failed);
    expect(&out, &anonymous, Outcome::Rejected);
    let erin_account = well_known::account(&erin);
    assert!(status_of(&out, &failing_first) != ExecStatus::Success);
    assert!(out
        .effects
        .iter()
        .any(|e| e.digest == failing_first.digest() && e.created.contains(&erin_account)));

    h.assert_covered(&[
        "Faucet/ok",
        "Faucet/rejected",
        "Transfer/ok",
        "Transfer/failed",
        "Transfer/rejected",
        "RotatePolicy/ok",
        "RotatePolicy/failed",
        "deferred/Treasury",
        "deferred/Minted",
    ]);
    h.assert_minimal_witnesses();
    h
}

#[test]
fn coins_accounts_and_outcomes_one_per_chunk() {
    coins_accounts_and_outcomes(1).summary("coins (1 per chunk)");
}

#[test]
fn coins_accounts_and_outcomes_many_per_chunk() {
    coins_accounts_and_outcomes(5).summary("coins (5 per chunk)");
}

// ---------------------------------------------------------------------------
// Scenario: claims, atoms, bonds and expiry
// ---------------------------------------------------------------------------

fn knowledge(chunk_size: usize) -> Harness {
    let mut h = Harness::new(&["alice", "bob", "mallory"], 10_000, 8, chunk_size);
    let (alice, bob, mallory) = (h.id("alice"), h.id("bob"), h.id("mallory"));
    let stake = bond_atom(AtomClass::Economic, &alice, "stake");
    let circuit = registered_atom(AtomClass::Code, &bob, "circuit-v1");

    let base = note("the sky is blue", formulas::dual_signature());
    let base_id = claim_id(&alice, &base);
    let derived = note(
        "so is the sea",
        Formula::and(vec![Formula::claim(base_id), formulas::dual_signature()]),
    );
    let derived_id = claim_id(&bob, &derived);
    let mut short = note("brief", formulas::dual_signature());
    short.valid_for = 2;
    short.challenge_period = 1;
    let short_id = claim_id(&alice, &short);

    // Pulse 1: a bond, a code atom, claims (one referencing a claim of the
    // same Pulse, one resting on someone else's bond), invalid operations.
    let bond = h.tx(
        "alice",
        TxKind::PostBond {
            class: AtomClass::Economic,
            label: "stake".into(),
            amount: units(1_000),
        },
    );
    let atom = h.tx(
        "bob",
        TxKind::RegisterAtom {
            class: AtomClass::Code,
            label: "circuit-v1".into(),
        },
    );
    let issue_base = h.tx(
        "alice",
        TxKind::IssueClaim {
            claim: base.clone(),
        },
    );
    let issue_short = h.tx(
        "alice",
        TxKind::IssueClaim {
            claim: short.clone(),
        },
    );
    let too_early = h.tx(
        "bob",
        TxKind::IssueClaim {
            claim: derived.clone(),
        },
    );
    let impersonation = h.tx(
        "mallory",
        TxKind::IssueClaim {
            claim: note("alice vouches for me", formulas::bonded_statement(stake.id)),
        },
    );
    let wrong_class = h.tx(
        "alice",
        TxKind::RegisterAtom {
            class: AtomClass::Economic,
            label: "nope".into(),
        },
    );
    let empty_label = h.tx(
        "bob",
        TxKind::RegisterAtom {
            class: AtomClass::Hardware,
            label: String::new(),
        },
    );
    let zero_bond = h.tx(
        "mallory",
        TxKind::PostBond {
            class: AtomClass::Economic,
            label: "nothing".into(),
            amount: Amount::ZERO,
        },
    );
    let unbondable = h.tx(
        "mallory",
        TxKind::PostBond {
            class: AtomClass::Code,
            label: "x".into(),
            amount: units(1),
        },
    );
    let mut bad_schema = note("no schema", formulas::dual_signature());
    bad_schema.schema = String::new();
    let bad_schema = h.tx("bob", TxKind::IssueClaim { claim: bad_schema });
    let mut falsum = note("never", Formula::False);
    falsum.valid_for = 1;
    let falsum = h.tx("bob", TxKind::IssueClaim { claim: falsum });
    let mut forever = note("forever", formulas::dual_signature());
    forever.valid_for = u64::MAX;
    let forever = h.tx("mallory", TxKind::IssueClaim { claim: forever });
    let out = h.pulse(vec![
        bond.clone(),
        atom.clone(),
        issue_base.clone(),
        issue_short.clone(),
        too_early.clone(),
        impersonation.clone(),
        wrong_class.clone(),
        empty_label.clone(),
        zero_bond.clone(),
        unbondable.clone(),
        bad_schema.clone(),
        falsum.clone(),
        forever.clone(),
    ]);
    for tx in [
        &bond,
        &atom,
        &issue_base,
        &issue_short,
        &too_early,
        &impersonation,
    ] {
        expect(&out, tx, Outcome::Success);
    }
    for tx in [
        &wrong_class,
        &empty_label,
        &zero_bond,
        &unbondable,
        &bad_schema,
        &falsum,
        &forever,
    ] {
        expect(&out, tx, Outcome::Failed);
    }
    let rejected: BTreeSet<ClaimId> = all_events(&out)
        .into_iter()
        .filter_map(|e| match e {
            Event::ClaimRejected { claim, .. } => Some(claim),
            _ => None,
        })
        .collect();
    assert!(rejected.contains(&derived_id), "same-Pulse reference");
    assert_eq!(rejected.len(), 2, "and the impersonation");
    assert_eq!(h.chain.state.knowledge.len(), 2);

    // Pulse 2: the reference is fine now; a bonded claim on one's own bond,
    // a claim on a registered code atom, claims on unknown things.
    let reissue = h.tx(
        "bob",
        TxKind::IssueClaim {
            claim: derived.clone(),
        },
    );
    let bonded = h.tx(
        "alice",
        TxKind::IssueClaim {
            claim: note("my stake backs this", formulas::bonded_statement(stake.id)),
        },
    );
    let on_circuit = h.tx(
        "bob",
        TxKind::IssueClaim {
            claim: note(
                "proven by my circuit",
                Formula::and(vec![formulas::dual_signature(), Formula::atom(circuit.id)]),
            ),
        },
    );
    let dangling = h.tx(
        "mallory",
        TxKind::IssueClaim {
            claim: note(
                "rests on nothing",
                Formula::claim(ClaimId::derive(b"nowhere")),
            ),
        },
    );
    let unknown_atom = h.tx(
        "mallory",
        TxKind::IssueClaim {
            claim: note(
                "rests on an unknown atom",
                Formula::atom(registered_atom(AtomClass::Hardware, &mallory, "tee").id),
            ),
        },
    );
    let out = h.pulse(vec![
        reissue.clone(),
        bonded.clone(),
        on_circuit.clone(),
        dangling.clone(),
        unknown_atom.clone(),
    ]);
    let issued: BTreeSet<ClaimId> = out.new_claims.iter().map(|claim| claim.id).collect();
    assert!(issued.contains(&derived_id));
    assert_eq!(issued.len(), 3);
    assert_eq!(h.chain.state.knowledge.len(), 5);
    // The bonded claim now locks the bond through its challenge period.
    let early_withdraw = h.tx("alice", TxKind::WithdrawBond { atom: stake.id });
    let out = h.pulse(vec![early_withdraw.clone()]);
    expect(&out, &early_withdraw, Outcome::Failed);

    // The short claim expires at the end of its window (a registry tick);
    // a later reference to it is rejected.
    h.advance(3);
    assert_eq!(
        h.chain.state.registry().claim(&short_id).unwrap().status,
        fold_assurance::ClaimStatus::Expired
    );
    let on_expired = h.tx(
        "bob",
        TxKind::IssueClaim {
            claim: note("rests on an expired claim", Formula::claim(short_id)),
        },
    );
    let out = h.pulse(vec![on_expired]);
    assert!(all_events(&out)
        .iter()
        .any(|e| matches!(e, Event::ClaimRejected { .. })));

    // The bond is locked through the challenge period of claims on it.
    let locked_until = h
        .chain
        .state
        .registry()
        .bond(&stake.id)
        .unwrap()
        .locked_until;
    h.advance_to(locked_until.0 + 1);
    let withdraw = h.tx("alice", TxKind::WithdrawBond { atom: stake.id });
    let top_up = h.tx(
        "alice",
        TxKind::PostBond {
            class: AtomClass::Economic,
            label: "stake".into(),
            amount: units(5),
        },
    );
    let out = h.pulse(vec![withdraw.clone()]);
    expect(&out, &withdraw, Outcome::Success);
    let out = h.pulse(vec![top_up.clone()]);
    expect(&out, &top_up, Outcome::Success);
    let _ = (alice, bob);

    h.assert_covered(&[
        "IssueClaim/ok",
        "IssueClaim/failed",
        "RegisterAtom/ok",
        "RegisterAtom/failed",
        "PostBond/ok",
        "PostBond/failed",
        "WithdrawBond/ok",
        "WithdrawBond/failed",
        "deferred/IssueClaim",
        "deferred/RegisterAtom",
        "deferred/PostBond",
        "deferred-event/ClaimIssued",
        "deferred-event/ClaimRejected",
        "deferred-event/AtomRegistered",
        "deferred-event/BondPosted",
        "due/RegistryTick",
    ]);
    h.assert_minimal_witnesses();
    h
}

#[test]
fn knowledge_claims_atoms_bonds_and_expiry() {
    knowledge(2).summary("knowledge");
}

// ---------------------------------------------------------------------------
// Scenario: reliance, challenges, slashing and compensation
// ---------------------------------------------------------------------------

fn assurance(chunk_size: usize) -> Harness {
    let mut h = Harness::new(
        &[
            "supplier",
            "shipper",
            "judge",
            "financier",
            "lender",
            "buyer",
            "mallory",
        ],
        200_000,
        4,
        chunk_size,
    );
    let (supplier, shipper, judge) = (h.id("supplier"), h.id("shipper"), h.id("judge"));
    let s_atom = bond_atom(AtomClass::Economic, &supplier, "invoices");
    let w_atom = bond_atom(AtomClass::Economic, &shipper, "logistics");
    let j_atom = bond_atom(AtomClass::Adjudicator, &judge, "arbitration");

    let bonds = vec![
        h.tx(
            "supplier",
            TxKind::PostBond {
                class: AtomClass::Economic,
                label: "invoices".into(),
                amount: units(30_000),
            },
        ),
        h.tx(
            "shipper",
            TxKind::PostBond {
                class: AtomClass::Economic,
                label: "logistics".into(),
                amount: units(20_000),
            },
        ),
        h.tx(
            "judge",
            TxKind::PostBond {
                class: AtomClass::Adjudicator,
                label: "arbitration".into(),
                amount: units(100_000),
            },
        ),
    ];
    h.pulse(bonds);

    let delivery = ClaimBody {
        schema: "trade.delivery/v1".into(),
        subject: "Shipment for invoice #1042 delivered to the buyer".into(),
        payload: Payload::Commitment(hash_domain("test/pod", b"proof-of-delivery-1042")),
        formula: formulas::bonded_statement(w_atom.id),
        adjudicator: Some(j_atom.id),
        detection_permille: 1000,
        valid_for: 200,
        challenge_period: 50,
    };
    let delivery_id = claim_id(&shipper, &delivery);
    let tx = h.tx("shipper", TxKind::IssueClaim { claim: delivery });
    h.pulse(vec![tx]);

    let invoice = ClaimBody {
        schema: "trade.invoice/v1".into(),
        subject: "Invoice #1042: 10,000 USD payable by the buyer".into(),
        payload: Payload::Public(b"invoice-1042".to_vec()),
        formula: Formula::and(vec![
            formulas::dual_signature(),
            Formula::or(vec![Formula::atom(s_atom.id), Formula::claim(delivery_id)]),
        ]),
        adjudicator: Some(j_atom.id),
        detection_permille: 1000,
        valid_for: 200,
        challenge_period: 50,
    };
    let invoice_id = claim_id(&supplier, &invoice);
    let tx = h.tx("supplier", TxKind::IssueClaim { claim: invoice });
    h.pulse(vec![tx]);

    // Reliance: accepted, too greedy (failed), reliance ending now (failed),
    // a second consumer who will release early.
    let until = Pulse(h.pulse_number() + 100);
    let accept_tx = h.tx(
        "financier",
        TxKind::Accept {
            claim: invoice_id,
            value_at_risk: units(10_000),
            policy: Policy::standard(),
            until,
            nonce: 0,
        },
    );
    let greedy = h.tx(
        "financier",
        TxKind::Accept {
            claim: invoice_id,
            value_at_risk: units(30_000),
            policy: Policy::standard(),
            until,
            nonce: 1,
        },
    );
    let instant = h.tx(
        "financier",
        TxKind::Accept {
            claim: invoice_id,
            value_at_risk: units(1),
            policy: Policy::standard(),
            until: Pulse(h.pulse_number() + 1),
            nonce: 2,
        },
    );
    let lend = h.tx(
        "lender",
        TxKind::Accept {
            claim: invoice_id,
            value_at_risk: units(1_000),
            policy: Policy::standard(),
            until,
            nonce: 0,
        },
    );
    let out = h.pulse(vec![
        accept_tx.clone(),
        greedy.clone(),
        instant.clone(),
        lend.clone(),
    ]);
    expect(&out, &accept_tx, Outcome::Success);
    expect(&out, &greedy, Outcome::Failed);
    expect(&out, &instant, Outcome::Failed);
    expect(&out, &lend, Outcome::Success);
    let lender_reservation = out
        .effects
        .iter()
        .find(|e| e.digest == lend.digest())
        .and_then(|e| {
            e.events.iter().find_map(|event| match event {
                Event::Reserved { reservation, .. } => Some(*reservation),
                _ => None,
            })
        })
        .expect("reserved");

    let challenge = |h: &mut Harness, claim: ClaimId, target, tag: &str| {
        h.tx(
            "buyer",
            TxKind::Challenge {
                claim,
                target,
                evidence: hash_domain("test/evidence", tag.as_bytes()),
                deposit: units(1_000),
            },
        )
    };
    let opened = |out: &PulseOutput, tx: &SignedTransaction| {
        out.effects
            .iter()
            .find(|e| e.digest == tx.digest())
            .and_then(|e| {
                e.events.iter().find_map(|event| match event {
                    Event::ChallengeOpened { challenge, .. } => Some(*challenge),
                    _ => None,
                })
            })
            .expect("challenge opened")
    };
    // Two challenges: one against the supplier (will be upheld), one
    // frivolous one against the shipper (will be rejected).
    let c1 = challenge(&mut h, invoice_id, s_atom.id, "goods never shipped");
    let c2 = challenge(&mut h, delivery_id, w_atom.id, "frivolous");
    let unknown_claim = challenge(&mut h, ClaimId::derive(b"nope"), s_atom.id, "x");
    let out = h.pulse(vec![c1.clone(), c2.clone(), unknown_claim.clone()]);
    expect(&out, &unknown_claim, Outcome::Failed);
    let (first, frivolous) = (opened(&out, &c1), opened(&out, &c2));

    let impostor = h.tx(
        "mallory",
        TxKind::Rule {
            challenge: first,
            upheld: true,
        },
    );
    let upheld = h.tx(
        "judge",
        TxKind::Rule {
            challenge: first,
            upheld: true,
        },
    );
    let dismissed = h.tx(
        "judge",
        TxKind::Rule {
            challenge: frivolous,
            upheld: false,
        },
    );
    let shipper_before = h.balance("shipper", &usd());
    let out = h.pulse(vec![impostor.clone(), upheld.clone(), dismissed.clone()]);
    expect(&out, &impostor, Outcome::Failed);
    expect(&out, &upheld, Outcome::Success);
    expect(&out, &dismissed, Outcome::Success);
    assert!(all_events(&out)
        .iter()
        .any(|e| matches!(e, Event::Slashed { amount, .. } if *amount == units(30_000))));
    assert_eq!(
        h.balance("shipper", &usd()),
        shipper_before.checked_add(units(1_000)).unwrap(),
        "the frivolous deposit goes to the challenged bond's owner"
    );

    // An early release; a bond behind a broken atom is refunded at the end
    // of the Pulse; a ruling twice on the same challenge fails.
    let release = h.tx(
        "lender",
        TxKind::Release {
            reservation: lender_reservation,
        },
    );
    let foreign_release = h.tx(
        "mallory",
        TxKind::Release {
            reservation: lender_reservation,
        },
    );
    let broken_bond = h.tx(
        "supplier",
        TxKind::PostBond {
            class: AtomClass::Economic,
            label: "invoices".into(),
            amount: units(500),
        },
    );
    let twice = h.tx(
        "judge",
        TxKind::Rule {
            challenge: first,
            upheld: true,
        },
    );
    let supplier_before = h.balance("supplier", &usd());
    let out = h.pulse(vec![
        foreign_release.clone(),
        release.clone(),
        broken_bond.clone(),
        twice.clone(),
    ]);
    expect(&out, &foreign_release, Outcome::Failed);
    expect(&out, &release, Outcome::Success);
    expect(&out, &broken_bond, Outcome::Success);
    expect(&out, &twice, Outcome::Failed);
    let fee = broken_bond.tx.fee;
    assert_eq!(
        h.balance("supplier", &usd()),
        supplier_before.checked_sub(fee).unwrap(),
        "the bond on a broken atom was paid back"
    );

    // The shipper's bond can only be challenged through its own claim.
    let indirect = challenge(&mut h, invoice_id, w_atom.id, "no delivery");
    let out = h.pulse(vec![indirect.clone()]);
    expect(&out, &indirect, Outcome::Failed);
    let c3 = challenge(&mut h, delivery_id, w_atom.id, "no delivery");
    let out = h.pulse(vec![c3.clone()]);
    let second = opened(&out, &c3);
    let rule = h.tx(
        "judge",
        TxKind::Rule {
            challenge: second,
            upheld: true,
        },
    );
    let financier_before = h.balance("financier", &usd());
    let out = h.pulse(vec![rule]);
    let events = all_events(&out);
    assert!(events.iter().any(|e| matches!(e, Event::Falsified { .. })));
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::Compensated { amount, .. } if *amount == units(10_000))));
    assert_eq!(
        h.balance("financier", &usd()),
        financier_before.checked_add(units(10_000)).unwrap()
    );

    // Reliance ends; registry ticks release what pools still hold. The
    // judge's bond unlocks after the challenge periods of its claims.
    h.advance_to(until.0 + 1);
    let locked_until = h
        .chain
        .state
        .registry()
        .bond(&j_atom.id)
        .unwrap()
        .locked_until;
    h.advance_to(locked_until.0 + 1);
    let withdraw = h.tx("judge", TxKind::WithdrawBond { atom: j_atom.id });
    let none = h.tx("mallory", TxKind::WithdrawBond { atom: j_atom.id });
    let out = h.pulse(vec![withdraw.clone(), none.clone()]);
    expect(&out, &withdraw, Outcome::Success);
    expect(&out, &none, Outcome::Failed);

    h.assert_covered(&[
        "Accept/ok",
        "Accept/failed",
        "Release/ok",
        "Release/failed",
        "Challenge/ok",
        "Challenge/failed",
        "Rule/ok",
        "Rule/failed",
        "WithdrawBond/ok",
        "WithdrawBond/failed",
        "deferred/Schedule",
        "deferred/payout",
        "due/RegistryTick",
        "due/RegistryTick-releases-pools",
    ]);
    h.assert_minimal_witnesses();
    h
}

#[test]
fn assurance_reliance_challenges_slashing_and_compensation() {
    assurance(3).summary("assurance");
}

// ---------------------------------------------------------------------------
// Scenario: Agent Charters
// ---------------------------------------------------------------------------

fn charter_caveats(vendor: AccountId) -> Caveats {
    Caveats {
        payees: Some([vendor].into_iter().collect()),
        max_per_payment: units(100),
        max_per_window: units(250),
        window: 1_000,
        expires_at: Pulse(10_000),
        evidence_above: None,
        evidence_policy: Policy::standard(),
        receipts: true,
        may_delegate: true,
    }
}

fn charter_created(out: &PulseOutput, tx: &SignedTransaction) -> (ObjectId, ObjectId) {
    out.effects
        .iter()
        .find(|e| e.digest == tx.digest())
        .and_then(|e| {
            e.events.iter().find_map(|event| match event {
                Event::CharterCreated { charter, allotment } => Some((*charter, *allotment)),
                _ => None,
            })
        })
        .expect("charter created")
}

fn charters(chunk_size: usize) -> Harness {
    let mut h = Harness::new(
        &["principal", "agent", "helper", "vendor", "stranger"],
        10_000,
        4,
        chunk_size,
    );
    let (agent, helper, vendor, stranger) = (
        h.id("agent"),
        h.id("helper"),
        h.id("vendor"),
        h.id("stranger"),
    );

    // An evidence claim for large payments (cryptographic atoms only, so the
    // standard policy trusts it).
    let evidence_body = note("the vendor delivered", formulas::dual_signature());
    let evidence_id = claim_id(&vendor, &evidence_body);
    let evidence = h.tx(
        "vendor",
        TxKind::IssueClaim {
            claim: evidence_body,
        },
    );
    let create = h.tx(
        "principal",
        TxKind::CreateCharter {
            agent,
            caveats: charter_caveats(vendor),
            funding: units(1_000),
            agent_bond: units(500),
        },
    );
    let mut expired_caveats = charter_caveats(vendor);
    expired_caveats.expires_at = Pulse(0);
    let stillborn = h.tx(
        "principal",
        TxKind::CreateCharter {
            agent,
            caveats: expired_caveats,
            funding: units(10),
            agent_bond: Amount::ZERO,
        },
    );
    let mut careful = charter_caveats(vendor);
    careful.evidence_above = Some(units(50));
    careful.receipts = false;
    careful.may_delegate = false;
    careful.expires_at = Pulse(h.pulse_number() + 30);
    let create_careful = h.tx(
        "principal",
        TxKind::CreateCharter {
            agent,
            caveats: careful,
            funding: units(300),
            agent_bond: Amount::ZERO,
        },
    );
    let out = h.pulse(vec![
        evidence,
        create.clone(),
        stillborn.clone(),
        create_careful.clone(),
    ]);
    expect(&out, &create, Outcome::Success);
    expect(&out, &stillborn, Outcome::Failed);
    expect(&out, &create_careful, Outcome::Success);
    let (charter, allotment) = charter_created(&out, &create);
    let (careful_charter, careful_allotment) = charter_created(&out, &create_careful);

    let pay = |h: &mut Harness,
               who: &str,
               charter: ObjectId,
               allotment: ObjectId,
               payee: AccountId,
               amount: Amount,
               evidence: Option<ClaimId>| {
        let input = h.reference(&allotment);
        h.tx_with(
            who,
            TxKind::AgentPay {
                charter,
                payee,
                amount,
                purpose: "inference credits".into(),
                evidence,
            },
            vec![input],
        )
    };

    let ok = pay(
        &mut h,
        "agent",
        charter,
        allotment,
        vendor,
        units(100),
        None,
    );
    let out = h.pulse(vec![ok.clone()]);
    expect(&out, &ok, Outcome::Success);
    for (payee, amount) in [(stranger, units(10)), (vendor, units(150))] {
        let bad = pay(&mut h, "agent", charter, allotment, payee, amount, None);
        let out = h.pulse(vec![bad.clone()]);
        expect(&out, &bad, Outcome::Failed);
    }
    let ok = pay(
        &mut h,
        "agent",
        charter,
        allotment,
        vendor,
        units(100),
        None,
    );
    let out = h.pulse(vec![ok.clone()]);
    expect(&out, &ok, Outcome::Success);
    let over = pay(
        &mut h,
        "agent",
        charter,
        allotment,
        vendor,
        units(100),
        None,
    );
    let out = h.pulse(vec![over.clone()]);
    expect(&out, &over, Outcome::Failed);

    // Evidence: none, unknown, and a valid evidence claim above the threshold.
    let bare = pay(
        &mut h,
        "agent",
        careful_charter,
        careful_allotment,
        vendor,
        units(60),
        None,
    );
    let out = h.pulse(vec![bare.clone()]);
    expect(&out, &bare, Outcome::Failed);
    let unknown = pay(
        &mut h,
        "agent",
        careful_charter,
        careful_allotment,
        vendor,
        units(60),
        Some(ClaimId::derive(b"no such claim")),
    );
    let out = h.pulse(vec![unknown.clone()]);
    expect(&out, &unknown, Outcome::Failed);
    let backed = pay(
        &mut h,
        "agent",
        careful_charter,
        careful_allotment,
        vendor,
        units(60),
        Some(evidence_id),
    );
    let out = h.pulse(vec![backed.clone()]);
    expect(&out, &backed, Outcome::Success);
    // A stranger cannot even name the agent's allotment as an input.
    let theft = {
        let input = h.reference(&allotment);
        let mut body = h
            .tx(
                "stranger",
                TxKind::Transfer {
                    asset: usd(),
                    outputs: vec![(vendor, units(1))],
                },
            )
            .tx;
        body.kind = TxKind::AgentPay {
            charter,
            payee: stranger,
            amount: units(1),
            purpose: "theft".into(),
            evidence: None,
        };
        body.inputs.push(input);
        h.resign("stranger", body)
    };
    // The careful allotment is not a bounded allotment.
    let misuse = {
        let input = h.reference(&careful_allotment);
        h.tx_with(
            "agent",
            TxKind::SpendAllotment {
                to: vendor,
                amount: units(1),
            },
            vec![input],
        )
    };
    let out = h.pulse(vec![theft.clone(), misuse.clone()]);
    expect(&out, &theft, Outcome::Rejected);
    expect(&out, &misuse, Outcome::Failed);

    // Delegation only narrows.
    let mut wide = charter_caveats(vendor);
    wide.max_per_payment = units(500);
    let input = h.reference(&allotment);
    let bad = h.tx_with(
        "agent",
        TxKind::Delegate {
            charter,
            sub_agent: helper,
            caveats: wide,
            amount: units(100),
        },
        vec![input],
    );
    let out = h.pulse(vec![bad.clone()]);
    expect(&out, &bad, Outcome::Failed);
    let mut narrow = charter_caveats(vendor);
    narrow.max_per_payment = units(20);
    narrow.max_per_window = units(40);
    narrow.may_delegate = false;
    let input = h.reference(&allotment);
    let delegate = h.tx_with(
        "agent",
        TxKind::Delegate {
            charter,
            sub_agent: helper,
            caveats: narrow.clone(),
            amount: units(100),
        },
        vec![input],
    );
    let out = h.pulse(vec![delegate.clone()]);
    expect(&out, &delegate, Outcome::Success);
    let (child, child_allotment) = charter_created(&out, &delegate);
    let sub = pay(
        &mut h,
        "helper",
        child,
        child_allotment,
        vendor,
        units(20),
        None,
    );
    let out = h.pulse(vec![sub.clone()]);
    expect(&out, &sub, Outcome::Success);
    let input = h.reference(&child_allotment);
    let no_further = h.tx_with(
        "helper",
        TxKind::Delegate {
            charter: child,
            sub_agent: stranger,
            caveats: narrow,
            amount: units(1),
        },
        vec![input],
    );
    let out = h.pulse(vec![no_further.clone()]);
    expect(&out, &no_further, Outcome::Failed);

    // Revocation: a racing payment in the same Pulse still executes; then the
    // charter and its child stop. Only the principal may revoke.
    h.advance(1_000);
    let revoke = h.tx("principal", TxKind::RevokeCharter { charter });
    let not_yours = h.tx("stranger", TxKind::RevokeCharter { charter });
    let racing = pay(&mut h, "agent", charter, allotment, vendor, units(50), None);
    let out = h.pulse(vec![not_yours.clone(), revoke.clone(), racing.clone()]);
    expect(&out, &not_yours, Outcome::Failed);
    expect(&out, &revoke, Outcome::Success);
    expect(&out, &racing, Outcome::Success);
    let late = pay(&mut h, "agent", charter, allotment, vendor, units(10), None);
    let late_sub = pay(
        &mut h,
        "helper",
        child,
        child_allotment,
        vendor,
        units(10),
        None,
    );
    let out = h.pulse(vec![late.clone(), late_sub.clone()]);
    expect(&out, &late, Outcome::Failed);
    expect(&out, &late_sub, Outcome::Failed);

    // Reclaiming: the root (revoked), the child (dead through its chain),
    // the careful charter (expired); a stranger may not reclaim.
    let reclaim_root = h.tx("principal", TxKind::ReclaimCharter { charter });
    let reclaim_child = h.tx("principal", TxKind::ReclaimCharter { charter: child });
    let reclaim_careful = h.tx(
        "principal",
        TxKind::ReclaimCharter {
            charter: careful_charter,
        },
    );
    let not_backer = h.tx("stranger", TxKind::ReclaimCharter { charter });
    let principal_before = h.balance("principal", &usd());
    let fees = [&reclaim_root, &reclaim_child, &reclaim_careful]
        .iter()
        .try_fold(Amount::ZERO, |total, tx| total.checked_add(tx.tx.fee))
        .unwrap();
    let out = h.pulse(vec![
        not_backer.clone(),
        reclaim_root.clone(),
        reclaim_child.clone(),
        reclaim_careful.clone(),
    ]);
    expect(&out, &not_backer, Outcome::Failed);
    expect(&out, &reclaim_root, Outcome::Success);
    expect(&out, &reclaim_child, Outcome::Success);
    expect(&out, &reclaim_careful, Outcome::Success);
    // 1,000 − 250 paid − 100 delegated = 650 at the root, 80 below, 240 careful.
    assert_eq!(
        h.balance("principal", &usd()),
        principal_before
            .checked_add(units(970))
            .and_then(|a| a.checked_sub(fees))
            .unwrap()
    );
    // A live charter cannot be reclaimed.
    let live = h.tx(
        "principal",
        TxKind::CreateCharter {
            agent,
            caveats: charter_caveats(vendor),
            funding: units(10),
            agent_bond: Amount::ZERO,
        },
    );
    let out = h.pulse(vec![live.clone()]);
    let (live_charter, _) = charter_created(&out, &live);
    let premature = h.tx(
        "principal",
        TxKind::ReclaimCharter {
            charter: live_charter,
        },
    );
    let out = h.pulse(vec![premature.clone()]);
    expect(&out, &premature, Outcome::Failed);

    h.assert_covered(&[
        "CreateCharter/ok",
        "CreateCharter/failed",
        "AgentPay/ok",
        "AgentPay/failed",
        "AgentPay/rejected",
        "Delegate/ok",
        "Delegate/failed",
        "RevokeCharter/ok",
        "RevokeCharter/failed",
        "ReclaimCharter/ok",
        "ReclaimCharter/failed",
        "SpendAllotment/failed",
    ]);
    h.assert_minimal_witnesses();
    h
}

#[test]
fn charters_create_pay_delegate_revoke_reclaim() {
    charters(1).summary("charters");
}

// ---------------------------------------------------------------------------
// Scenario: bounded totals
// ---------------------------------------------------------------------------

fn bounded(chunk_size: usize) -> Harness {
    let mut h = Harness::new(
        &["manager", "w1", "w2", "shop", "outsider"],
        10_000,
        4,
        chunk_size,
    );
    let (w1, w2, shop) = (h.id("w1"), h.id("w2"), h.id("shop"));
    h.fund(&[("manager", gpu(), units(40))]);
    let create = h.tx(
        "manager",
        TxKind::CreateBounded {
            label: "team budget".into(),
            asset: usd(),
            total: units(1_000),
            allotments: vec![(w1, units(400)), (w2, units(400))],
        },
    );
    let goods = h.tx(
        "manager",
        TxKind::CreateBounded {
            label: "gpu pool".into(),
            asset: gpu(),
            total: units(30),
            allotments: vec![(w1, units(10))],
        },
    );
    let overcommitted = h.tx(
        "manager",
        TxKind::CreateBounded {
            label: "too much".into(),
            asset: usd(),
            total: units(10),
            allotments: vec![(w1, units(6)), (w2, units(6))],
        },
    );
    let out = h.pulse(vec![create.clone(), goods.clone(), overcommitted.clone()]);
    expect(&out, &create, Outcome::Success);
    expect(&out, &goods, Outcome::Success);
    expect(&out, &overcommitted, Outcome::Failed);
    let created = |out: &PulseOutput, tx: &SignedTransaction| {
        out.effects
            .iter()
            .find(|e| e.digest == tx.digest())
            .and_then(|e| {
                e.events.iter().find_map(|event| match event {
                    Event::BoundedCreated {
                        bounded,
                        allotments,
                    } => Some((*bounded, allotments.clone())),
                    _ => None,
                })
            })
            .expect("bounded total created")
    };
    let (budget, allotments) = created(&out, &create);
    let (pool, pool_allotments) = created(&out, &goods);

    let spend = |h: &mut Harness, who: &str, allotment: ObjectId, amount: Amount| {
        let input = h.reference(&allotment);
        h.tx_with(
            who,
            TxKind::SpendAllotment { to: shop, amount },
            vec![input],
        )
    };
    let s1 = spend(&mut h, "w1", allotments[0], units(300));
    let s2 = spend(&mut h, "w2", allotments[1], units(400));
    let s3 = spend(&mut h, "w1", pool_allotments[0], units(4));
    let out = h.pulse(vec![s1.clone(), s2.clone(), s3.clone()]);
    for tx in [&s1, &s2, &s3] {
        expect(&out, tx, Outcome::Success);
    }
    let over = spend(&mut h, "w1", allotments[0], units(101));
    let out = h.pulse(vec![over.clone()]);
    expect(&out, &over, Outcome::Failed);
    let back = {
        let input = h.reference(&allotments[0]);
        h.tx_with("w1", TxKind::ReturnAllotment, vec![input])
    };
    let back_goods = {
        let input = h.reference(&pool_allotments[0]);
        h.tx_with("w1", TxKind::ReturnAllotment, vec![input])
    };
    let out = h.pulse(vec![back.clone(), back_goods.clone()]);
    expect(&out, &back, Outcome::Success);
    expect(&out, &back_goods, Outcome::Success);
    let rebalance = h.tx(
        "manager",
        TxKind::Rebalance {
            bounded: budget,
            holder: w2,
            amount: units(300),
        },
    );
    let outsider = h.tx(
        "outsider",
        TxKind::Rebalance {
            bounded: budget,
            holder: w2,
            amount: units(1),
        },
    );
    let out = h.pulse(vec![outsider.clone(), rebalance.clone()]);
    expect(&out, &outsider, Outcome::Failed);
    expect(&out, &rebalance, Outcome::Success);
    let exhausted = h.tx(
        "manager",
        TxKind::Rebalance {
            bounded: budget,
            holder: w1,
            amount: units(1),
        },
    );
    let goods_rebalance = h.tx(
        "manager",
        TxKind::Rebalance {
            bounded: pool,
            holder: w2,
            amount: units(26),
        },
    );
    let out = h.pulse(vec![exhausted.clone(), goods_rebalance.clone()]);
    expect(&out, &exhausted, Outcome::Failed);
    expect(&out, &goods_rebalance, Outcome::Success);
    let ObjectData::Bounded(state) = &h.chain.state.objects[&budget].data else {
        panic!("bounded total")
    };
    assert_eq!(state.unallocated, Amount::ZERO);
    assert_eq!(h.balance("shop", &usd()), units(10_700));
    assert_eq!(h.balance("shop", &gpu()), units(4));

    h.assert_covered(&[
        "CreateBounded/ok",
        "CreateBounded/failed",
        "SpendAllotment/ok",
        "SpendAllotment/failed",
        "ReturnAllotment/ok",
        "Rebalance/ok",
        "Rebalance/failed",
        "deferred/ReturnToBounded",
    ]);
    h.assert_minimal_witnesses();
    h
}

#[test]
fn bounded_totals_create_spend_return_rebalance() {
    bounded(2).summary("bounded");
}

// ---------------------------------------------------------------------------
// Scenario: markets
// ---------------------------------------------------------------------------

fn create_market(
    h: &mut Harness,
    committee: Vec<fold_crypto::KemPublicKey>,
    threshold: u8,
) -> ObjectId {
    let tx = h.tx(
        "operator",
        TxKind::CreateMarket {
            label: "gpu-hours".into(),
            good: gpu(),
            numeraire: usd(),
            batch_length: 4,
            clear_deadline: 3,
            committee,
            threshold,
        },
    );
    let out = h.pulse(vec![tx.clone()]);
    expect(&out, &tx, Outcome::Success);
    all_events(&out)
        .into_iter()
        .find_map(|e| match e {
            Event::MarketCreated { market } => Some(market),
            _ => None,
        })
        .expect("market created")
}

fn order(
    h: &mut Harness,
    who: &str,
    market: ObjectId,
    side: Side,
    quantity: u64,
    price: u128,
) -> SignedTransaction {
    h.tx(
        who,
        TxKind::SubmitOrder {
            market,
            submission: Submission::Plain(OrderTerms {
                side,
                quantity,
                limit_price: cents(price),
            }),
        },
    )
}

fn market_state(h: &Harness, market: &ObjectId) -> Market {
    match &h.chain.state.objects[market].data {
        ObjectData::Market(state) => (**state).clone(),
        other => panic!("not a market: {other:?}"),
    }
}

/// The clearing batch of the latest sealed batch, built as a solver would
/// (§5.25 step 3), without `Market::clearing_batch`.
fn sealed_batch(h: &Harness, market: &ObjectId) -> (u64, fold_clearing::Batch) {
    let state = market_state(h, market);
    let (index, batch) = state
        .batches
        .iter()
        .rev()
        .find(|(_, b)| matches!(b.status, BatchStatus::Sealed { .. }))
        .expect("a sealed batch");
    let BatchStatus::Sealed { beacon, at } = batch.status else {
        unreachable!()
    };
    let orders = batch
        .orders
        .iter()
        .filter_map(|pending| {
            pending.terms.map(|terms| fold_clearing::Order {
                id: pending.id,
                owner: pending.owner,
                side: terms.side,
                quantity: terms.quantity,
                limit_price: terms.limit_price,
            })
        })
        .collect();
    (
        *index,
        fold_clearing::Batch {
            domain: state.domain,
            pulse: at,
            beacon,
            orders,
        },
    )
}

/// Runs empty Pulses until the next Pulse starts a batch window of length 4.
fn align_to_window(h: &mut Harness) {
    while (h.pulse_number() + 1) % 4 != 0 {
        h.pulse(Vec::new());
    }
}

fn markets(chunk_size: usize) -> Harness {
    let mut h = Harness::new(
        &["operator", "agent", "lab-a", "lab-b", "solver", "relay"],
        10_000,
        4,
        chunk_size,
    );
    h.fund(&[("lab-a", gpu(), units(20)), ("lab-b", gpu(), units(6))]);
    let invalid = h.tx(
        "operator",
        TxKind::CreateMarket {
            label: "self-trade".into(),
            good: usd(),
            numeraire: usd(),
            batch_length: 4,
            clear_deadline: 3,
            committee: Vec::new(),
            threshold: 0,
        },
    );
    let out = h.pulse(vec![invalid.clone()]);
    expect(&out, &invalid, Outcome::Failed);
    let market = create_market(&mut h, Vec::new(), 0);

    // Plain orders clear at one price.
    align_to_window(&mut h);
    let orders = vec![
        order(&mut h, "agent", market, Side::Buy, 8, 210),
        order(&mut h, "lab-a", market, Side::Sell, 5, 180),
        order(&mut h, "lab-b", market, Side::Sell, 6, 200),
    ];
    let zero = h.tx(
        "agent",
        TxKind::SubmitOrder {
            market,
            submission: Submission::Plain(OrderTerms {
                side: Side::Buy,
                quantity: 0,
                limit_price: cents(100),
            }),
        },
    );
    let mut all = orders.clone();
    all.push(zero.clone());
    let out = h.pulse(all);
    expect(&out, &zero, Outcome::Failed);
    for tx in &orders {
        expect(&out, tx, Outcome::Success);
    }
    let agent_usd = h.balance("agent", &usd());
    h.advance(3);
    let (index, batch) = sealed_batch(&h, &market);
    let result = fold_clearing::clear(&batch).expect("clears");
    assert_eq!(result.price, Some(cents(200)));
    let mut wrong = result.clone();
    wrong.price = Some(cents(201));
    let forged = h.tx(
        "solver",
        TxKind::ClearBatch {
            market,
            batch: index,
            result: wrong,
        },
    );
    let clear = h.tx(
        "solver",
        TxKind::ClearBatch {
            market,
            batch: index,
            result: result.clone(),
        },
    );
    let again = h.tx(
        "solver",
        TxKind::ClearBatch {
            market,
            batch: index,
            result,
        },
    );
    let out = h.pulse(vec![forged.clone(), clear.clone(), again.clone()]);
    expect(&out, &forged, Outcome::Failed);
    expect(&out, &clear, Outcome::Success);
    expect(&out, &again, Outcome::Failed);
    assert_eq!(h.balance("agent", &gpu()), units(8));
    assert_eq!(
        h.balance("agent", &usd()),
        agent_usd.checked_add(cents(80)).unwrap()
    );

    // Uncleared batches are refunded at the deadline.
    let refund_orders = vec![
        order(&mut h, "agent", market, Side::Buy, 2, 300),
        order(&mut h, "lab-a", market, Side::Sell, 5, 100),
    ];
    h.pulse(refund_orders);
    h.advance(10);
    let state = market_state(&h, &market);
    assert!(state
        .batches
        .values()
        .any(|b| b.status == BatchStatus::Refunded));

    // Sealed orders are revealed by the committee and cleared.
    let committee: Vec<KemKeypair> = (1..=3u8).map(|i| KemKeypair::from_seed(&[i; 32])).collect();
    let sealed_market = create_market(
        &mut h,
        committee.iter().map(|k| k.public().clone()).collect(),
        2,
    );
    align_to_window(&mut h);
    let state = market_state(&h, &sealed_market);
    let next = h.pulse_number() + 1;
    let index = next / state.batch_length;
    let aad = fold_exec::market::sealing_aad(&state.domain, index);
    let mut rng = fold_core::rng::seeded(7);
    let keys: Vec<_> = committee.iter().map(|k| k.public().clone()).collect();
    let terms = OrderTerms {
        side: Side::Buy,
        quantity: 4,
        limit_price: cents(250),
    };
    let envelope = CommitteeEnvelope::seal(
        &keys,
        2,
        SEALED_ORDER_DOMAIN,
        &aad,
        &fold_core::encode(&terms),
        &mut rng,
    )
    .expect("seal");
    let sealed = h.tx(
        "agent",
        TxKind::SubmitOrder {
            market: sealed_market,
            submission: Submission::Sealed {
                envelope: envelope.clone(),
                side: Side::Buy,
                escrow: units(12),
            },
        },
    );
    let outsiders: Vec<_> = (7..=9u8)
        .map(|i| KemKeypair::from_seed(&[i; 32]).public().clone())
        .collect();
    let own = CommitteeEnvelope::seal(
        &outsiders,
        2,
        SEALED_ORDER_DOMAIN,
        &aad,
        &fold_core::encode(&terms),
        &mut rng,
    )
    .expect("seal");
    let selective = h.tx(
        "agent",
        TxKind::SubmitOrder {
            market: sealed_market,
            submission: Submission::Sealed {
                envelope: own,
                side: Side::Buy,
                escrow: units(12),
            },
        },
    );
    let to_plain = h.tx(
        "agent",
        TxKind::SubmitOrder {
            market,
            submission: Submission::Sealed {
                envelope: envelope.clone(),
                side: Side::Buy,
                escrow: units(12),
            },
        },
    );
    let ask = order(&mut h, "lab-a", sealed_market, Side::Sell, 5, 150);
    let out = h.pulse(vec![
        sealed.clone(),
        selective.clone(),
        to_plain.clone(),
        ask.clone(),
    ]);
    expect(&out, &sealed, Outcome::Success);
    expect(&out, &selective, Outcome::Failed);
    expect(&out, &to_plain, Outcome::Failed);
    expect(&out, &ask, Outcome::Success);
    // Revealing an open batch fails.
    let order_id = {
        let state = market_state(&h, &sealed_market);
        state.batches[&index]
            .orders
            .iter()
            .find(|o| o.sealed.is_some())
            .expect("sealed order")
            .id
    };
    let shares: Vec<_> = [0u8, 2]
        .iter()
        .map(|i| {
            envelope
                .open_share(*i, &committee[usize::from(*i)], SEALED_ORDER_DOMAIN, &aad)
                .expect("share")
        })
        .collect();
    let early_reveal = h.tx(
        "relay",
        TxKind::RevealBatch {
            market: sealed_market,
            batch: index,
            shares: vec![(order_id, shares.clone())],
        },
    );
    let out = h.pulse(vec![early_reveal.clone()]);
    expect(&out, &early_reveal, Outcome::Failed);
    h.advance_to(index * 4 + 3);
    let (sealed_index, batch) = sealed_batch(&h, &sealed_market);
    assert_eq!(sealed_index, index);
    assert_eq!(batch.orders.len(), 1, "only the plain ask is visible");
    let early = fold_clearing::clear(&batch).expect("clears");
    let premature = h.tx(
        "solver",
        TxKind::ClearBatch {
            market: sealed_market,
            batch: index,
            result: early,
        },
    );
    let too_few = h.tx(
        "relay",
        TxKind::RevealBatch {
            market: sealed_market,
            batch: index,
            shares: vec![(order_id, shares[..1].to_vec())],
        },
    );
    let reveal = h.tx(
        "relay",
        TxKind::RevealBatch {
            market: sealed_market,
            batch: index,
            shares: vec![(order_id, shares)],
        },
    );
    let out = h.pulse(vec![premature.clone(), too_few.clone(), reveal.clone()]);
    expect(&out, &premature, Outcome::Failed);
    expect(&out, &too_few, Outcome::Success);
    expect(&out, &reveal, Outcome::Success);
    let (_, batch) = sealed_batch(&h, &sealed_market);
    assert_eq!(batch.orders.len(), 2);
    let result = fold_clearing::clear(&batch).expect("clears");
    assert_eq!(result.volume, 4);
    let clear = h.tx(
        "solver",
        TxKind::ClearBatch {
            market: sealed_market,
            batch: index,
            result,
        },
    );
    let out = h.pulse(vec![clear.clone()]);
    expect(&out, &clear, Outcome::Success);
    assert_eq!(h.balance("agent", &gpu()), units(12));

    h.assert_covered(&[
        "CreateMarket/ok",
        "CreateMarket/failed",
        "SubmitOrder/ok",
        "SubmitOrder/failed",
        "RevealBatch/ok",
        "RevealBatch/failed",
        "ClearBatch/ok",
        "ClearBatch/failed",
        "deferred/AddOrder",
        "due/SealBatch",
        "due/BatchDeadline",
        "deferred-event/BatchSealed",
        "deferred-event/BatchRefunded",
        "deferred/payout",
    ]);
    h.assert_minimal_witnesses();
    h
}

#[test]
fn markets_plain_sealed_cleared_and_refunded() {
    markets(3).summary("markets");
}

// ---------------------------------------------------------------------------
// Negative tests: tampered chunks must be rejected
// ---------------------------------------------------------------------------

/// A small honest Pulse to tamper with: a transfer, a claim and a bond in
/// two transaction chunks, then the deferred phase.
fn honest_pulse() -> (Harness, Vec<ChunkTrace>) {
    let mut h = Harness::new(&["alice", "bob"], 1_000, 4, 2);
    let bob = h.id("bob");
    let transfer = h.tx(
        "alice",
        TxKind::Transfer {
            asset: usd(),
            outputs: vec![(bob, units(10)), (bob, units(3))],
        },
    );
    let claim = h.tx(
        "bob",
        TxKind::IssueClaim {
            claim: note("tamper with me", formulas::dual_signature()),
        },
    );
    let bond = h.tx(
        "alice",
        TxKind::PostBond {
            class: AtomClass::Economic,
            label: "stake".into(),
            amount: units(5),
        },
    );
    h.pulse(vec![transfer, claim, bond]);
    let traces = h.chain.history.last().expect("a Pulse").traces.clone();
    assert_eq!(
        traces.len(),
        3,
        "two transaction chunks and the deferred phase"
    );
    assert!(traces[2].statement.deferred_phase);
    assert_eq!(
        traces[2].statement.after.knowledge_size,
        traces[2].statement.before.knowledge_size + 1
    );
    (h, traces)
}

fn flip(hash: &mut Hash32) {
    hash.0[0] ^= 0x80;
}

fn rejected(trace: &ChunkTrace, what: &str) -> SpecError {
    match verify_chunk(trace) {
        Ok(report) => panic!("stack B accepted a chunk with {what}: {report:?}"),
        Err(error) => error,
    }
}

#[test]
fn tampered_statement_roots_and_accumulators_are_rejected() {
    use fold_exec::pulse::ChunkStatement;
    let (_, traces) = honest_pulse();
    type Mutation = (&'static str, fn(&mut ChunkStatement));
    let mutations: Vec<Mutation> = vec![
        ("before.state_root", |s| flip(&mut s.before.state_root)),
        ("after.state_root", |s| flip(&mut s.after.state_root)),
        ("before.knowledge_root", |s| {
            flip(&mut s.before.knowledge_root)
        }),
        ("after.knowledge_root", |s| {
            flip(&mut s.after.knowledge_root)
        }),
        ("before.knowledge_size", |s| s.before.knowledge_size += 1),
        ("after.knowledge_size", |s| s.after.knowledge_size += 1),
        ("before.config", |s| flip(&mut s.before.config)),
        ("after.config", |s| flip(&mut s.after.config)),
        ("deferred_in", |s| flip(&mut s.deferred_in)),
        ("deferred_out", |s| flip(&mut s.deferred_out)),
        ("effects_in", |s| flip(&mut s.effects_in)),
        ("effects_out", |s| flip(&mut s.effects_out)),
        ("pulse", |s| s.pulse = s.pulse.next()),
        ("after.pulse", |s| s.after.pulse = s.after.pulse.next()),
        ("before.pulse", |s| s.before.pulse = s.pulse.next()),
        ("deferred_phase", |s| s.deferred_phase = !s.deferred_phase),
    ];
    for trace in &traces {
        accept(trace);
        for (what, mutate) in &mutations {
            let mut tampered = trace.clone();
            mutate(&mut tampered.statement);
            rejected(&tampered, what);
        }
        // §8.5: transaction chunks bind their transactions; the deferred
        // phase (v1.1) binds `steps([])`.
        let mut tampered = trace.clone();
        flip(&mut tampered.statement.steps);
        assert_eq!(rejected(&tampered, "steps"), SpecError::StepsMismatch);
    }
}

#[test]
fn tampered_deferred_steps_are_rejected() {
    let (_, traces) = honest_pulse();
    let phase = traces.last().expect("deferred phase");
    assert!(phase.statement.deferred_phase);
    assert_eq!(
        phase.statement.steps,
        fold_spec::commit::steps_digest(&[]),
        "stack A binds the empty step list"
    );
    // Claiming the Pulse's transactions as the phase's steps ...
    let all: Vec<SignedTransaction> = traces
        .iter()
        .flat_map(|trace| trace.transactions.iter().cloned())
        .collect();
    let mut claimed = phase.clone();
    claimed.statement.steps = fold_spec::commit::steps_digest(&all);
    assert_eq!(
        rejected(&claimed, "deferred steps naming transactions"),
        SpecError::StepsMismatch
    );
    // ... or anything else.
    let mut zero = phase.clone();
    zero.statement.steps = Hash32::ZERO;
    assert_eq!(
        rejected(&zero, "zero deferred steps"),
        SpecError::StepsMismatch
    );
}

#[test]
fn dropped_witness_objects_are_rejected() {
    let (_, traces) = honest_pulse();
    let mut absent_droppable = 0usize;
    for trace in &traces {
        for position in 0..trace.witness.objects.len() {
            let mut dropped = trace.clone();
            let (id, object, _) = dropped.witness.objects.remove(position);
            match (&object, verify_chunk(&dropped)) {
                (Some(_), Ok(_)) => {
                    panic!("stack B accepted a chunk without the witnessed object {id}")
                }
                (Some(_), Err(error)) => assert!(
                    matches!(
                        error,
                        SpecError::Unwitnessed(_) | SpecError::UncoveredWrite { .. }
                    ),
                    "dropping {id}: {error}"
                ),
                // A created object's absence may also be proven by another
                // entry's proof along the same path.
                (None, Ok(_)) => absent_droppable += 1,
                (None, Err(_)) => {}
            }
        }
    }
    eprintln!("absent entries whose absence another proof also covered: {absent_droppable}");
}

#[test]
fn altered_transactions_are_rejected() {
    let (h, traces) = honest_pulse();
    let first = &traces[0];
    assert_eq!(first.transactions.len(), 2);

    // Any change to a transaction changes the steps it is bound to.
    let mut altered = first.clone();
    altered.transactions[0].tx.nonce += 1;
    assert_eq!(
        rejected(&altered, "an altered transaction"),
        SpecError::StepsMismatch
    );

    // A re-signed, different transaction under matching steps executes
    // differently from what the statement claims.
    let fix_steps = |trace: &mut ChunkTrace| {
        trace.statement.steps = fold_spec::commit::steps_digest(&trace.transactions);
    };
    for position in 0..first.transactions.len() {
        let mut altered = first.clone();
        let original = &altered.transactions[position].tx;
        let name = if original.sender == h.id("alice") {
            "alice"
        } else {
            "bob"
        };
        let mut body = original.clone();
        body.nonce += 1;
        if let TxKind::Transfer { outputs, .. } = &mut body.kind {
            outputs[0].1 = units(11);
        }
        altered.transactions[position] = h.resign(name, body);
        fix_steps(&mut altered);
        rejected(&altered, "a re-signed different transaction");
    }

    // Dropping, reordering or repeating transactions.
    let mut dropped = first.clone();
    dropped.transactions.remove(0);
    fix_steps(&mut dropped);
    rejected(&dropped, "a dropped transaction");
    let mut swapped = first.clone();
    swapped.transactions.swap(0, 1);
    fix_steps(&mut swapped);
    rejected(&swapped, "reordered transactions");
    let mut repeated = first.clone();
    let copy = repeated.transactions[0].clone();
    repeated.transactions.push(copy);
    fix_steps(&mut repeated);
    rejected(&repeated, "a repeated transaction");

    // A broken signature.
    let mut forged = first.clone();
    forged.transactions[0].auth.signatures[0].signature.bytes[3] ^= 1;
    rejected(&forged, "a forged signature");
}

#[test]
fn altered_deferred_items_are_rejected() {
    let (_, traces) = honest_pulse();
    let phase = traces.last().expect("deferred phase");
    assert!(!phase.deferred_items.is_empty());
    let refit = |trace: &mut ChunkTrace| {
        let acc = chain_deferred(&Hash32::ZERO, &trace.deferred_items);
        trace.statement.deferred_in = acc;
        trace.statement.deferred_out = acc;
    };

    // Changed amounts break the accumulator; with the accumulator refitted
    // they break the state root.
    let treasury = phase
        .deferred_items
        .iter()
        .position(|item| matches!(item, Deferred::Treasury { .. }))
        .expect("a fee");
    let mut altered = phase.clone();
    if let Deferred::Treasury { amount, .. } = &mut altered.deferred_items[treasury] {
        *amount = amount.checked_add(Amount(1)).unwrap();
    }
    assert_eq!(
        rejected(&altered, "an altered item"),
        SpecError::DeferredMismatch
    );
    refit(&mut altered);
    assert!(matches!(
        rejected(&altered, "an altered, refitted item"),
        SpecError::StateRootMismatch { .. }
    ));

    // Dropping each item (refitted) changes the outcome.
    for position in 0..phase.deferred_items.len() {
        let mut dropped = phase.clone();
        let item = dropped.deferred_items.remove(position);
        rejected(&dropped, "a dropped item");
        refit(&mut dropped);
        rejected(
            &dropped,
            &format!("a dropped, refitted {}", deferred_name(&item)),
        );
    }

    // An injected mint.
    let mut injected = phase.clone();
    injected.deferred_items.push(Deferred::Minted {
        asset: usd(),
        amount: units(1_000_000),
    });
    refit(&mut injected);
    rejected(&injected, "an injected item");

    // The phase sorts its items, so their order only matters to the
    // accumulator, which binds this chunk to the transaction chunks.
    let mut reordered = phase.clone();
    reordered.deferred_items.reverse();
    rejected(&reordered, "reordered items");
    refit(&mut reordered);
    let report = verify_chunk(&reordered).expect("the deferred phase is order-independent");
    assert_eq!(report.written, accept(phase).written);
}

#[test]
fn forged_witness_objects_are_rejected() {
    let (_, traces) = honest_pulse();
    let trace = &traces[0];
    let coin = trace
        .witness
        .objects
        .iter()
        .position(|(_, o, _)| o.as_ref().is_some_and(|o| o.as_coin().is_some()))
        .expect("a witnessed coin");
    let absent = trace
        .witness
        .objects
        .iter()
        .position(|(_, o, _)| o.is_none())
        .expect("a witnessed absence");

    let mut richer = trace.clone();
    if let Some(Object {
        data: ObjectData::Coin(coin),
        ..
    }) = &mut richer.witness.objects[coin].1
    {
        coin.amount = units(1_000_000);
    }
    assert!(matches!(
        rejected(&richer, "a forged coin"),
        SpecError::WitnessProof { .. }
    ));

    let mut hidden = trace.clone();
    hidden.witness.objects[coin].1 = None;
    assert!(matches!(
        rejected(&hidden, "a hidden object"),
        SpecError::WitnessProof { .. }
    ));

    let mut invented = trace.clone();
    let mut fake = trace.witness.objects[coin].1.clone().expect("coin");
    fake.id = invented.witness.objects[absent].0;
    invented.witness.objects[absent].1 = Some(fake);
    assert!(matches!(
        rejected(&invented, "an invented object"),
        SpecError::WitnessProof { .. }
    ));

    let mut renamed = trace.clone();
    if let Some(object) = &mut renamed.witness.objects[coin].1 {
        object.id = ObjectId::derive(b"elsewhere");
    }
    assert!(matches!(
        rejected(&renamed, "a renamed object"),
        SpecError::WitnessIdMismatch { .. }
    ));

    let mut doubled = trace.clone();
    let entry = doubled.witness.objects[coin].clone();
    doubled.witness.objects.push(entry);
    assert!(matches!(
        rejected(&doubled, "a duplicated entry"),
        SpecError::DuplicateWitness(_)
    ));

    let mut swapped = trace.clone();
    let other = swapped.witness.objects[absent].2.clone();
    swapped.witness.objects[coin].2 = other;
    rejected(&swapped, "a proof for another key");
}

#[test]
fn misplaced_payloads_frontiers_and_configurations_are_rejected() {
    let (_, traces) = honest_pulse();
    let (chunk, phase) = (&traces[0], &traces[2]);

    let mut with_items = chunk.clone();
    with_items.deferred_items = phase.deferred_items.clone();
    assert!(matches!(
        rejected(&with_items, "deferred items in a transaction chunk"),
        SpecError::UnexpectedDeferredItems(_)
    ));
    let mut with_transactions = phase.clone();
    with_transactions.transactions = chunk.transactions.clone();
    assert!(matches!(
        rejected(&with_transactions, "transactions in the deferred phase"),
        SpecError::UnexpectedTransactions(_)
    ));

    for trace in [chunk, phase] {
        let mut bigger = trace.clone();
        bigger.witness.frontier.size += 1;
        assert_eq!(
            rejected(&bigger, "a resized frontier"),
            SpecError::FrontierMismatch
        );
        let mut altered = trace.clone();
        if let Some(peak) = altered.witness.frontier.peaks.first_mut() {
            flip(peak);
            assert_eq!(
                rejected(&altered, "an altered frontier"),
                SpecError::FrontierMismatch
            );
        }
    }

    // Another configuration: caught by its digest; and even with matching
    // digests, executing under a higher fee floor changes the outcome.
    let mut config = chunk.clone();
    config.witness.config.base_fee = units(1);
    assert!(matches!(
        rejected(&config, "another configuration"),
        SpecError::ConfigMismatch { .. }
    ));
    let digest = config.witness.config.digest();
    config.statement.before.config = digest;
    config.statement.after.config = digest;
    rejected(&config, "execution under another configuration");
}

// ---------------------------------------------------------------------------
// Property test: random workloads
// ---------------------------------------------------------------------------

const NAMES: [&str; 4] = ["a", "b", "c", "d"];

#[derive(Clone, Debug)]
enum Op {
    Transfer {
        from: usize,
        to: Vec<usize>,
        amount: u64,
        goods: bool,
    },
    Claim {
        from: usize,
        tag: u8,
        bonded: bool,
        refers: Option<usize>,
    },
    Bond {
        from: usize,
        amount: u64,
    },
    Withdraw {
        from: usize,
    },
    Mint {
        to: usize,
        amount: u64,
    },
    Atom {
        from: usize,
        tag: u8,
    },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (0..4usize, proptest::collection::vec(0..4usize, 1..4), 0..400u64, any::<bool>())
            .prop_map(|(from, to, amount, goods)| Op::Transfer { from, to, amount, goods }),
        3 => (0..4usize, any::<u8>(), any::<bool>(), proptest::option::of(0..64usize))
            .prop_map(|(from, tag, bonded, refers)| Op::Claim { from, tag, bonded, refers }),
        1 => (0..4usize, 1..300u64).prop_map(|(from, amount)| Op::Bond { from, amount }),
        1 => (0..4usize).prop_map(|from| Op::Withdraw { from }),
        1 => (0..4usize, 1..1_000u64).prop_map(|(to, amount)| Op::Mint { to, amount }),
        1 => (0..4usize, any::<u8>()).prop_map(|(from, tag)| Op::Atom { from, tag }),
    ]
}

fn build(
    h: &mut Harness,
    op: &Op,
    ids: &[AccountId],
    claims: &mut Vec<ClaimId>,
) -> Option<SignedTransaction> {
    match op {
        Op::Transfer {
            from,
            to,
            amount,
            goods,
        } => {
            let asset = if *goods { gpu() } else { usd() };
            let outputs = to.iter().map(|t| (ids[*t], units(*amount))).collect();
            h.chain
                .tx(NAMES[*from], TxKind::Transfer { asset, outputs })
                .ok()
        }
        Op::Claim {
            from,
            tag,
            bonded,
            refers,
        } => {
            let mut formula = if *bonded {
                formulas::bonded_statement(bond_atom(AtomClass::Economic, &ids[*from], "stake").id)
            } else {
                formulas::dual_signature()
            };
            if let Some(index) = refers {
                if !claims.is_empty() {
                    let referenced = claims[index % claims.len()];
                    formula = Formula::and(vec![formula, Formula::claim(referenced)]);
                }
            }
            let body = ClaimBody {
                schema: "prop.note/v1".into(),
                subject: format!("note {tag}"),
                payload: Payload::Public(vec![*tag]),
                formula,
                adjudicator: None,
                detection_permille: 1000,
                valid_for: u64::from(*tag % 5) + 1,
                challenge_period: 2,
            };
            claims.push(claim_id(&ids[*from], &body));
            h.chain
                .tx(NAMES[*from], TxKind::IssueClaim { claim: body })
                .ok()
        }
        Op::Bond { from, amount } => h
            .chain
            .tx(
                NAMES[*from],
                TxKind::PostBond {
                    class: AtomClass::Economic,
                    label: "stake".into(),
                    amount: units(*amount),
                },
            )
            .ok(),
        Op::Withdraw { from } => h
            .chain
            .tx(
                NAMES[*from],
                TxKind::WithdrawBond {
                    atom: bond_atom(AtomClass::Economic, &ids[*from], "stake").id,
                },
            )
            .ok(),
        Op::Mint { to, amount } => {
            let grants = vec![(ids[*to], usd(), units(*amount))];
            let faucet = h.chain.wallets.get_mut(fold_exec::sim::FAUCET)?;
            faucet.faucet(&h.chain.state, grants).ok()
        }
        Op::Atom { from, tag } => h
            .chain
            .tx(
                NAMES[*from],
                TxKind::RegisterAtom {
                    class: AtomClass::Code,
                    label: format!("circuit-{}", tag % 4),
                },
            )
            .ok(),
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 24,
        failure_persistence: None,
        // Deterministic, as AGENTS.md asks of every RNG.
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x5eed_f01d),
        ..ProptestConfig::default()
    })]
    #[test]
    fn random_workloads_are_verified_chunk_by_chunk(
        chunk_size in 1usize..5,
        pulses in proptest::collection::vec(proptest::collection::vec(op(), 0..8), 1..6),
    ) {
        let mut h = Harness::new(&NAMES, 2_000, 3, chunk_size);
        let ids: Vec<AccountId> = NAMES.iter().map(|n| h.id(n)).collect();
        let goods: Vec<(&str, AssetId, Amount)> =
            NAMES.iter().map(|n| (*n, gpu(), units(500))).collect();
        h.fund(&goods);
        let mut claims = Vec::new();
        for ops in &pulses {
            let mut txs = Vec::new();
            let mut minted = false;
            for op in ops {
                if matches!(op, Op::Mint { .. }) {
                    // The faucet's account object admits one transaction per Pulse.
                    if minted {
                        continue;
                    }
                    minted = true;
                }
                if let Some(tx) = build(&mut h, op, &ids, &mut claims) {
                    txs.push(tx);
                }
            }
            h.pulse(txs);
        }
        // Let claims expire and registry ticks run.
        h.advance(8);
        prop_assert!(h.chunks >= h.verified);
        prop_assert!(h.unread.is_empty(), "{:?}", h.unread);
        h.summary(&format!("random workload (chunk size {chunk_size})"));
    }
}

// ---------------------------------------------------------------------------
// Edge-case probes: boundaries the specification fixes exactly
// ---------------------------------------------------------------------------

#[test]
fn probe_delegation_chain_depth_limit() {
    // One agent delegates to itself, building a chain of charters one link
    // deeper per Pulse. §5.13 (v1.1): at most MAX_CHARTER_CHAIN (16)
    // charters are loaded; if the 16th still has a parent, the check fails
    // without loading it.
    let mut h = Harness::new(&["principal", "agent", "vendor"], 10_000, 4, 1);
    let (agent, vendor) = (h.id("agent"), h.id("vendor"));
    let mut caveats = charter_caveats(vendor);
    caveats.receipts = false;
    let create = h.tx(
        "principal",
        TxKind::CreateCharter {
            agent,
            caveats: caveats.clone(),
            funding: units(1_000),
            agent_bond: Amount::ZERO,
        },
    );
    let out = h.pulse(vec![create.clone()]);
    let mut chain = vec![charter_created(&out, &create)];
    let mut outcomes = Vec::new();
    for depth in 1..=18u64 {
        let (charter, allotment) = *chain.last().unwrap();
        let input = h.reference(&allotment);
        let delegate = h.tx_with(
            "agent",
            TxKind::Delegate {
                charter,
                sub_agent: agent,
                caveats: caveats.clone(),
                amount: units(1_000 - 50 * depth),
            },
            vec![input],
        );
        let out = h.pulse(vec![delegate.clone()]);
        let outcome = outcome_of(&status_of(&out, &delegate));
        outcomes.push((depth, outcome));
        if outcome != Outcome::Success {
            break;
        }
        chain.push(charter_created(&out, &delegate));
    }
    // Pay from the deepest charters.
    let mut pays = Vec::new();
    for (depth, (charter, allotment)) in chain.iter().enumerate().rev().take(3) {
        let input = h.reference(allotment);
        let pay = h.tx_with(
            "agent",
            TxKind::AgentPay {
                charter: *charter,
                payee: vendor,
                amount: units(1),
                purpose: "deep".into(),
                evidence: None,
            },
            vec![input],
        );
        let out = h.pulse(vec![pay.clone()]);
        pays.push((depth + 1, outcome_of(&status_of(&out, &pay))));
    }
    eprintln!("delegations from depth: {outcomes:?}; payments at depth: {pays:?}");
    // A chain of 16 charters passes, one of 17 fails ...
    let limit = fold_exec::charter::MAX_CHARTER_CHAIN;
    assert_eq!(
        chain.len(),
        limit + 1,
        "delegating from a 17-charter chain fails"
    );
    assert_eq!(
        pays,
        vec![
            (limit + 1, Outcome::Failed),
            (limit, Outcome::Success),
            (limit - 1, Outcome::Success)
        ]
    );
    // ... and neither stack loads the 17th charter: stack A's witnesses for
    // the 17-charter checks hold nothing stack B does not read. (Under v1.0
    // stack A also loaded the root.)
    h.assert_minimal_witnesses();
    h.summary("chain depth");
}

#[test]
fn probe_saturating_charter_windows() {
    // §5.13 (v1.1): `window_start + k.window` saturates, so a window that
    // would end after u64::MAX never resets and its per-window limit becomes
    // a lifetime limit. (Under v1.0 the sum overflowed into an error; stack
    // A already saturated.)
    let mut h = Harness::new(&["principal", "agent", "helper", "vendor"], 10_000, 4, 1);
    let (agent, helper, vendor) = (h.id("agent"), h.id("helper"), h.id("vendor"));
    let mut caveats = charter_caveats(vendor);
    caveats.window = u64::MAX;
    caveats.max_per_window = units(150);
    caveats.receipts = false;
    let create = h.tx(
        "principal",
        TxKind::CreateCharter {
            agent,
            caveats: caveats.clone(),
            funding: units(1_000),
            agent_bond: Amount::ZERO,
        },
    );
    let out = h.pulse(vec![create.clone()]);
    expect(&out, &create, Outcome::Success);
    let (charter, allotment) = charter_created(&out, &create);
    let pay =
        |h: &mut Harness, charter: ObjectId, allotment: ObjectId, who: &str, amount: Amount| {
            let input = h.reference(&allotment);
            let tx = h.tx_with(
                who,
                TxKind::AgentPay {
                    charter,
                    payee: vendor,
                    amount,
                    purpose: "lifetime".into(),
                    evidence: None,
                },
                vec![input],
            );
            let out = h.pulse(vec![tx.clone()]);
            outcome_of(&status_of(&out, &tx))
        };
    assert_eq!(
        pay(&mut h, charter, allotment, "agent", units(100)),
        Outcome::Success
    );
    h.advance(50);
    assert_eq!(
        pay(&mut h, charter, allotment, "agent", units(100)),
        Outcome::Failed,
        "no new window"
    );
    assert_eq!(
        pay(&mut h, charter, allotment, "agent", units(50)),
        Outcome::Success
    );
    assert_eq!(
        pay(&mut h, charter, allotment, "agent", Amount(1)),
        Outcome::Failed
    );
    // A sub-charter must keep a window at least as long: the same lifetime
    // rule, with a window one short of u64::MAX (also saturating, since its
    // window starts after Pulse 1).
    let mut child_caveats = caveats.clone();
    child_caveats.window = u64::MAX - 1;
    let input = h.reference(&allotment);
    let narrower = h.tx_with(
        "agent",
        TxKind::Delegate {
            charter,
            sub_agent: helper,
            caveats: child_caveats.clone(),
            amount: units(10),
        },
        vec![input],
    );
    let out = h.pulse(vec![narrower.clone()]);
    expect(&out, &narrower, Outcome::Failed);
    let mut child_caveats = caveats;
    child_caveats.max_per_window = units(40);
    let input = h.reference(&allotment);
    let delegate = h.tx_with(
        "agent",
        TxKind::Delegate {
            charter,
            sub_agent: helper,
            caveats: child_caveats,
            amount: units(100),
        },
        vec![input],
    );
    let out = h.pulse(vec![delegate.clone()]);
    expect(&out, &delegate, Outcome::Success);
    let (child, child_allotment) = charter_created(&out, &delegate);
    assert_eq!(
        pay(&mut h, child, child_allotment, "helper", units(40)),
        Outcome::Success
    );
    h.advance(10);
    assert_eq!(
        pay(&mut h, child, child_allotment, "helper", units(1)),
        Outcome::Failed
    );
    h.assert_minimal_witnesses();
    h.summary("saturating windows");
}

#[test]
fn probe_market_period_bounds() {
    // §5.22 (v1.1): 1 ≤ batch_length ≤ MAX_PERIOD and 1 ≤ clear_deadline ≤
    // MAX_PERIOD. (Under v1.0 a clearing deadline of u64::MAX overflowed
    // when the batch sealed: a panic in stack A's debug builds.)
    use fold_exec::market::MAX_PERIOD;
    let mut h = Harness::new(&["operator", "agent", "lab", "solver"], 10_000, 10, 2);
    h.fund(&[("lab", gpu(), units(10))]);
    let create = |h: &mut Harness, batch_length: u64, clear_deadline: u64| {
        h.tx(
            "operator",
            TxKind::CreateMarket {
                label: format!("periods {batch_length}/{clear_deadline}"),
                good: gpu(),
                numeraire: usd(),
                batch_length,
                clear_deadline,
                committee: Vec::new(),
                threshold: 0,
            },
        )
    };
    let cases = [
        (2, u64::MAX, Outcome::Failed),
        (2, MAX_PERIOD + 1, Outcome::Failed),
        (2, MAX_PERIOD, Outcome::Success),
        (2, 0, Outcome::Failed),
        (u64::MAX, 3, Outcome::Failed),
        (MAX_PERIOD + 1, 3, Outcome::Failed),
        (MAX_PERIOD, 3, Outcome::Success),
        (0, 3, Outcome::Failed),
    ];
    let txs: Vec<SignedTransaction> = cases
        .iter()
        .map(|(length, deadline, _)| create(&mut h, *length, *deadline))
        .collect();
    let out = h.pulse(txs.clone());
    let mut markets = BTreeMap::new();
    for ((length, deadline, expected), tx) in cases.iter().zip(&txs) {
        assert_eq!(
            outcome_of(&status_of(&out, tx)),
            *expected,
            "batch length {length}, clearing deadline {deadline}"
        );
        if *expected == Outcome::Success {
            let id = out
                .effects
                .iter()
                .find(|e| e.digest == tx.digest())
                .and_then(|e| {
                    e.events.iter().find_map(|event| match event {
                        Event::MarketCreated { market } => Some(*market),
                        _ => None,
                    })
                })
                .expect("market");
            markets.insert((*length, *deadline), id);
        }
    }
    let longest_deadline = markets[&(2, MAX_PERIOD)];
    let longest_window = markets[&(MAX_PERIOD, 3)];

    // The longest clearing deadline: the batch seals at the end of its
    // window and its deadline lands MAX_PERIOD Pulses later; it still
    // clears normally.
    while (h.pulse_number() + 1) % 2 != 0 {
        h.pulse(Vec::new());
    }
    let orders = vec![
        order(&mut h, "agent", longest_deadline, Side::Buy, 3, 200),
        order(&mut h, "lab", longest_deadline, Side::Sell, 3, 100),
        order(&mut h, "agent", longest_window, Side::Buy, 1, 100),
    ];
    h.pulse(orders);
    let opened = h.pulse_number();
    h.advance(1);
    let (index, batch) = sealed_batch(&h, &longest_deadline);
    let ObjectData::Schedule(schedule) = &h.chain.state.objects[&well_known::schedule()].data
    else {
        panic!("schedule")
    };
    let sealed_at = opened + 1;
    assert!(
        schedule.due[&(sealed_at + MAX_PERIOD)].contains(&ScheduledEvent::BatchDeadline {
            market: longest_deadline,
            batch: index
        })
    );
    // The longest batch window seals at MAX_PERIOD − 1 (batch 0).
    assert!(
        schedule.due[&(MAX_PERIOD - 1)].contains(&ScheduledEvent::SealBatch {
            market: longest_window,
            batch: 0
        })
    );
    let result = fold_clearing::clear(&batch).expect("clears");
    let clear = h.tx(
        "solver",
        TxKind::ClearBatch {
            market: longest_deadline,
            batch: index,
            result,
        },
    );
    let out = h.pulse(vec![clear.clone()]);
    expect(&out, &clear, Outcome::Success);
    h.assert_minimal_witnesses();
    h.summary("market periods");
}

#[test]
fn probe_faucet_edges() {
    use fold_exec::tx::MAX_GRANT;
    let mut h = Harness::new(&["alice", "bob"], 1_000, 4, 1);
    let (alice, bob) = (h.id("alice"), h.id("bob"));
    let faucet = |h: &mut Harness, grants: Vec<(AccountId, AssetId, Amount)>| {
        let wallet = h
            .chain
            .wallets
            .get_mut(fold_exec::sim::FAUCET)
            .expect("faucet");
        wallet
            .faucet(&h.chain.state, grants)
            .expect("faucet transaction")
    };
    // §4.1 step 3 (v1.1): a faucet transaction with a fee is rejected before
    // anything is read (one transaction per chunk here, so its chunk's
    // witness must be empty of anything stack B does not read).
    let mut paying = faucet(&mut h, vec![(alice, usd(), units(1))]).tx;
    paying.fee = units(1);
    let paying = h.resign(fold_exec::sim::FAUCET, paying);
    let out = h.pulse(vec![paying.clone()]);
    expect(&out, &paying, Outcome::Rejected);
    // Zero grants, too many grants, the most grants.
    let zero = faucet(
        &mut h,
        vec![(alice, usd(), units(1)), (bob, usd(), Amount::ZERO)],
    );
    let out = h.pulse(vec![zero.clone()]);
    expect(&out, &zero, Outcome::Failed);
    let many = faucet(&mut h, (0..257).map(|_| (bob, usd(), Amount(1))).collect());
    let out = h.pulse(vec![many.clone()]);
    expect(&out, &many, Outcome::Failed);
    let max = faucet(&mut h, (0..256).map(|_| (bob, gpu(), Amount(1))).collect());
    let out = h.pulse(vec![max.clone()]);
    expect(&out, &max, Outcome::Success);
    // §5.2 (v1.1): a grant may mint at most MAX_GRANT.
    let largest = faucet(&mut h, vec![(alice, gpu(), MAX_GRANT)]);
    let out = h.pulse(vec![largest.clone()]);
    expect(&out, &largest, Outcome::Success);
    let too_large = faucet(
        &mut h,
        vec![
            (alice, gpu(), units(1)),
            (bob, gpu(), MAX_GRANT.checked_add(Amount(1)).unwrap()),
        ],
    );
    let out = h.pulse(vec![too_large.clone()]);
    expect(&out, &too_large, Outcome::Failed);
    assert_eq!(h.balance("alice", &gpu()), MAX_GRANT);
    h.assert_minimal_witnesses();
    h.summary("faucet edges");
}

#[test]
fn probe_grants_keep_the_supply_far_from_overflow() {
    // Under v1.0 two grants of u128::MAX / 2 + 1 overflowed the minted
    // supply in the deferred phase (stack A refused the Pulse). With grants
    // capped at MAX_GRANT (§5.2) the transaction fails instead, and the
    // largest faucet transaction mints 256 × MAX_GRANT.
    use fold_exec::tx::MAX_GRANT;
    let mut h = Harness::new(&["alice"], 1_000, 2, 1);
    let alice = h.id("alice");
    let half = Amount(u128::MAX / 2 + 1);
    let wallet = h
        .chain
        .wallets
        .get_mut(fold_exec::sim::FAUCET)
        .expect("faucet");
    let huge = wallet
        .faucet(
            &h.chain.state,
            vec![(alice, gpu(), half), (alice, gpu(), half)],
        )
        .expect("faucet transaction");
    let out = h.pulse(vec![huge.clone()]);
    expect(&out, &huge, Outcome::Failed);
    let wallet = h
        .chain
        .wallets
        .get_mut(fold_exec::sim::FAUCET)
        .expect("faucet");
    let most = wallet
        .faucet(
            &h.chain.state,
            (0..256).map(|_| (alice, gpu(), MAX_GRANT)).collect(),
        )
        .expect("faucet transaction");
    let out = h.pulse(vec![most.clone()]);
    expect(&out, &most, Outcome::Success);
    assert_eq!(
        h.chain.state.minted(&gpu()),
        MAX_GRANT.mul_ratio(256, 1).unwrap()
    );
    h.assert_minimal_witnesses();
    h.summary("bounded grants");
}

#[test]
fn probe_transfer_and_input_edges() {
    let mut h = Harness::new(&["alice", "bob"], 1_000, 4, 3);
    let (alice, bob) = (h.id("alice"), h.id("bob"));
    let many = h.tx(
        "alice",
        TxKind::Transfer {
            asset: usd(),
            outputs: (0..257).map(|_| (bob, Amount(1))).collect(),
        },
    );
    let most = h.tx(
        "alice",
        TxKind::Transfer {
            asset: usd(),
            outputs: (0..256).map(|_| (bob, Amount(1))).collect(),
        },
    );
    let none = h.tx(
        "bob",
        TxKind::Transfer {
            asset: usd(),
            outputs: Vec::new(),
        },
    );
    // The sender's account object as an extra input: unchanged, its
    // version is bumped by finalize_inputs.
    let account = h.reference(&well_known::account(&alice));
    let with_account = h.tx_with(
        "alice",
        TxKind::Transfer {
            asset: usd(),
            outputs: vec![(bob, units(1))],
        },
        vec![account],
    );
    // Valid in exactly the Pulse it expires in.
    let mut last_chance = h
        .tx(
            "bob",
            TxKind::Transfer {
                asset: usd(),
                outputs: vec![(alice, units(1))],
            },
        )
        .tx;
    last_chance.expires_at = Pulse(h.pulse_number() + 1);
    let last_chance = h.resign("bob", last_chance);
    let out = h.pulse(vec![
        many.clone(),
        most.clone(),
        none.clone(),
        with_account.clone(),
        last_chance.clone(),
    ]);
    expect(&out, &many, Outcome::Failed);
    expect(&out, &most, Outcome::Success);
    expect(&out, &none, Outcome::Failed);
    expect(&out, &with_account, Outcome::Success);
    expect(&out, &last_chance, Outcome::Success);
    // RotatePolicy without the account object among the inputs fails.
    let mut rotate = h
        .tx(
            "bob",
            TxKind::RotatePolicy {
                policy: fold_exec::Wallet::named("bob-2").policy,
                viewing_key: None,
            },
        )
        .tx;
    let account = well_known::account(&bob);
    rotate.inputs.retain(|input| input.id != account);
    let rotate = h.resign("bob", rotate);
    let out = h.pulse(vec![rotate.clone()]);
    expect(&out, &rotate, Outcome::Failed);
    h.assert_minimal_witnesses();
    h.summary("transfer edges");
}

#[test]
fn probe_knowledge_edges() {
    let mut h = Harness::new(&["alice", "bob"], 10_000, 8, 2);
    let alice = h.id("alice");
    let circuit = registered_atom(AtomClass::Code, &alice, "fresh");
    let stake = bond_atom(AtomClass::Economic, &alice, "stake");
    let same = note("said twice", formulas::dual_signature());
    let twice_a = h.tx(
        "alice",
        TxKind::IssueClaim {
            claim: same.clone(),
        },
    );
    let twice_b = h.tx("alice", TxKind::IssueClaim { claim: same });
    // An atom registered and a bond posted in the same Pulse are visible to
    // claims of that Pulse (atoms and bonds sort before claims).
    let register = h.tx(
        "alice",
        TxKind::RegisterAtom {
            class: AtomClass::Code,
            label: "fresh".into(),
        },
    );
    let on_fresh = h.tx(
        "alice",
        TxKind::IssueClaim {
            claim: note("fresh circuit", Formula::atom(circuit.id)),
        },
    );
    let bond_a = h.tx(
        "alice",
        TxKind::PostBond {
            class: AtomClass::Economic,
            label: "stake".into(),
            amount: units(10),
        },
    );
    let bond_b = h.tx(
        "alice",
        TxKind::PostBond {
            class: AtomClass::Economic,
            label: "stake".into(),
            amount: units(20),
        },
    );
    let on_stake = h.tx(
        "alice",
        TxKind::IssueClaim {
            claim: note("staked", formulas::bonded_statement(stake.id)),
        },
    );
    let out = h.pulse(vec![
        twice_a, twice_b, register, on_fresh, bond_a, bond_b, on_stake,
    ]);
    assert_eq!(out.new_claims.len(), 3, "the duplicate is rejected");
    assert_eq!(
        h.chain.state.registry().bond(&stake.id).unwrap().amount,
        units(30)
    );
    // Accepting unknown or expired claims fails; challenging a claim without
    // an adjudicator fails; withdrawing a bond nobody posted fails.
    let mut short = note("brief", formulas::bonded_statement(stake.id));
    short.valid_for = 1;
    let short_id = claim_id(&alice, &short);
    let tx = h.tx("alice", TxKind::IssueClaim { claim: short });
    h.pulse(vec![tx]);
    h.advance(2);
    let until = Pulse(h.pulse_number() + 10);
    let unknown = h.tx(
        "bob",
        TxKind::Accept {
            claim: ClaimId::derive(b"nothing"),
            value_at_risk: units(1),
            policy: Policy::standard(),
            until,
            nonce: 0,
        },
    );
    let expired = h.tx(
        "bob",
        TxKind::Accept {
            claim: short_id,
            value_at_risk: units(1),
            policy: Policy::standard(),
            until,
            nonce: 1,
        },
    );
    let no_adjudicator = h.tx(
        "bob",
        TxKind::Challenge {
            claim: short_id,
            target: stake.id,
            evidence: Hash32::ZERO,
            deposit: Amount::ZERO,
        },
    );
    let unbonded = h.tx(
        "bob",
        TxKind::WithdrawBond {
            atom: bond_atom(AtomClass::Economic, &h.id("bob"), "never").id,
        },
    );
    let out = h.pulse(vec![
        unknown.clone(),
        expired.clone(),
        no_adjudicator.clone(),
        unbonded.clone(),
    ]);
    for tx in [&unknown, &expired, &no_adjudicator, &unbonded] {
        expect(&out, tx, Outcome::Failed);
    }
    h.assert_minimal_witnesses();
    h.summary("knowledge edges");
}

#[test]
fn probe_zero_amounts_and_missing_inputs() {
    let mut h = Harness::new(
        &[
            "principal",
            "agent",
            "vendor",
            "manager",
            "w1",
            "operator",
            "buyer",
        ],
        10_000,
        6,
        2,
    );
    let (agent, vendor, w1) = (h.id("agent"), h.id("vendor"), h.id("w1"));
    let create = h.tx(
        "principal",
        TxKind::CreateCharter {
            agent,
            caveats: charter_caveats(vendor),
            funding: Amount::ZERO,
            agent_bond: Amount::ZERO,
        },
    );
    let bounded = h.tx(
        "manager",
        TxKind::CreateBounded {
            label: "empty".into(),
            asset: usd(),
            total: Amount::ZERO,
            allotments: Vec::new(),
        },
    );
    let zero_bounded = h.tx(
        "manager",
        TxKind::CreateBounded {
            label: "zero allotment".into(),
            asset: usd(),
            total: units(5),
            allotments: vec![(w1, Amount::ZERO)],
        },
    );
    let market = h.tx(
        "operator",
        TxKind::CreateMarket {
            label: "m".into(),
            good: gpu(),
            numeraire: usd(),
            batch_length: 1,
            clear_deadline: 1,
            committee: Vec::new(),
            threshold: 9,
        },
    );
    let out = h.pulse(vec![
        create.clone(),
        bounded.clone(),
        zero_bounded.clone(),
        market.clone(),
    ]);
    for tx in [&create, &bounded, &zero_bounded, &market] {
        expect(&out, tx, Outcome::Success);
    }
    let (charter, allotment) = charter_created(&out, &create);
    let (budget, budget_allotments) = out
        .effects
        .iter()
        .find(|e| e.digest == zero_bounded.digest())
        .and_then(|e| {
            e.events.iter().find_map(|event| match event {
                Event::BoundedCreated {
                    bounded,
                    allotments,
                } => Some((*bounded, allotments.clone())),
                _ => None,
            })
        })
        .expect("bounded");
    let market_id = all_events(&out)
        .into_iter()
        .find_map(|e| match e {
            Event::MarketCreated { market } => Some(market),
            _ => None,
        })
        .expect("market");

    // Zero-amount payments, delegations, spends and rebalances; operations
    // missing their allotment input; a free buy order.
    let input = h.reference(&allotment);
    let zero_pay = h.tx_with(
        "agent",
        TxKind::AgentPay {
            charter,
            payee: vendor,
            amount: Amount::ZERO,
            purpose: "nothing".into(),
            evidence: None,
        },
        vec![input],
    );
    let no_input_pay = h.tx(
        "agent",
        TxKind::AgentPay {
            charter,
            payee: vendor,
            amount: Amount::ZERO,
            purpose: "nothing".into(),
            evidence: None,
        },
    );
    let no_input_spend = h.tx(
        "w1",
        TxKind::SpendAllotment {
            to: vendor,
            amount: Amount::ZERO,
        },
    );
    let no_input_return = h.tx("w1", TxKind::ReturnAllotment);
    let input = h.reference(&budget_allotments[0]);
    let zero_spend = h.tx_with(
        "w1",
        TxKind::SpendAllotment {
            to: vendor,
            amount: Amount::ZERO,
        },
        vec![input],
    );
    let zero_rebalance = h.tx(
        "manager",
        TxKind::Rebalance {
            bounded: budget,
            holder: w1,
            amount: Amount::ZERO,
        },
    );
    let free = h.tx(
        "buyer",
        TxKind::SubmitOrder {
            market: market_id,
            submission: Submission::Plain(OrderTerms {
                side: Side::Buy,
                quantity: 3,
                limit_price: Amount::ZERO,
            }),
        },
    );
    let out = h.pulse(vec![
        zero_pay.clone(),
        no_input_pay.clone(),
        no_input_spend.clone(),
        no_input_return.clone(),
        zero_spend.clone(),
        zero_rebalance.clone(),
        free.clone(),
    ]);
    for (name, tx) in [
        ("zero pay", &zero_pay),
        ("pay without input", &no_input_pay),
        ("spend without input", &no_input_spend),
        ("return without input", &no_input_return),
        ("zero spend", &zero_spend),
        ("zero rebalance", &zero_rebalance),
        ("free order", &free),
    ] {
        eprintln!("{name}: {:?}", status_of(&out, tx));
    }
    let input = h.reference(&allotment);
    let zero_delegate = h.tx_with(
        "agent",
        TxKind::Delegate {
            charter,
            sub_agent: vendor,
            caveats: charter_caveats(vendor),
            amount: Amount::ZERO,
        },
        vec![input],
    );
    let out = h.pulse(vec![zero_delegate.clone()]);
    eprintln!("zero delegation: {:?}", status_of(&out, &zero_delegate));
    // Let the free order's batch seal and expire.
    h.advance(4);
    h.assert_minimal_witnesses();
    h.summary("zero amounts");
}

#[test]
fn probe_sealed_order_edges() {
    let mut h = Harness::new(
        &["operator", "agent", "lab", "solver", "relay"],
        10_000,
        6,
        2,
    );
    h.fund(&[("lab", gpu(), units(20))]);
    let committee: Vec<KemKeypair> = (1..=3u8).map(|i| KemKeypair::from_seed(&[i; 32])).collect();
    let keys: Vec<_> = committee.iter().map(|k| k.public().clone()).collect();
    for (threshold, committee) in [(0u8, keys.clone()), (4, keys.clone())] {
        let tx = h.tx(
            "operator",
            TxKind::CreateMarket {
                label: "bad threshold".into(),
                good: gpu(),
                numeraire: usd(),
                batch_length: 4,
                clear_deadline: 3,
                committee,
                threshold,
            },
        );
        let out = h.pulse(vec![tx.clone()]);
        expect(&out, &tx, Outcome::Failed);
    }
    let market = create_market(&mut h, keys.clone(), 2);
    align_to_window(&mut h);
    let state = market_state(&h, &market);
    let index = (h.pulse_number() + 1) / state.batch_length;
    let aad = fold_exec::market::sealing_aad(&state.domain, index);
    let mut rng = fold_core::rng::seeded(11);
    let seal = |terms: &OrderTerms, threshold: u8, rng: &mut _| {
        CommitteeEnvelope::seal(
            &keys,
            threshold,
            SEALED_ORDER_DOMAIN,
            &aad,
            &fold_core::encode(terms),
            rng,
        )
        .expect("seal")
    };
    let buy = OrderTerms {
        side: Side::Buy,
        quantity: 2,
        limit_price: cents(300),
    };
    // Revealed with duplicate share lists (later ones replace earlier
    // ones); revealed but needing more than its escrow; never revealed; an
    // envelope with the wrong threshold; a zero escrow.
    let kept = seal(&buy, 2, &mut rng);
    let greedy = seal(
        &OrderTerms {
            quantity: 50,
            ..buy
        },
        2,
        &mut rng,
    );
    let silent = seal(&buy, 2, &mut rng);
    let wrong_threshold = seal(&buy, 3, &mut rng);
    let submit = |h: &mut Harness, envelope: &CommitteeEnvelope, escrow: Amount| {
        h.tx(
            "agent",
            TxKind::SubmitOrder {
                market,
                submission: Submission::Sealed {
                    envelope: envelope.clone(),
                    side: Side::Buy,
                    escrow,
                },
            },
        )
    };
    let s_kept = submit(&mut h, &kept, units(7));
    let s_greedy = submit(&mut h, &greedy, units(7));
    let s_silent = submit(&mut h, &silent, units(7));
    let s_wrong = submit(&mut h, &wrong_threshold, units(7));
    let s_zero = submit(&mut h, &kept, Amount::ZERO);
    let ask = order(&mut h, "lab", market, Side::Sell, 5, 150);
    let out = h.pulse(vec![
        s_kept.clone(),
        s_greedy.clone(),
        s_silent.clone(),
        s_wrong.clone(),
        s_zero.clone(),
        ask.clone(),
    ]);
    for tx in [&s_kept, &s_greedy, &s_silent, &ask] {
        expect(&out, tx, Outcome::Success);
    }
    expect(&out, &s_wrong, Outcome::Failed);
    expect(&out, &s_zero, Outcome::Failed);
    h.advance_to(index * 4 + 3);
    let state = market_state(&h, &market);
    let order_id = |submitted: &SignedTransaction| {
        fold_core::hash::hash_parts("fold/market/order", &[&submitted.digest().0 .0])
    };
    assert!(state.batches[&index]
        .orders
        .iter()
        .any(|o| o.id == order_id(&s_kept)));
    let shares = |envelope: &CommitteeEnvelope| -> Vec<fold_crypto::KeyShare> {
        [0u8, 1]
            .iter()
            .map(|i| {
                envelope
                    .open_share(*i, &committee[usize::from(*i)], SEALED_ORDER_DOMAIN, &aad)
                    .expect("share")
            })
            .collect()
    };
    let reveal = h.tx(
        "relay",
        TxKind::RevealBatch {
            market,
            batch: index,
            shares: vec![
                (order_id(&s_kept), shares(&silent)),
                (order_id(&s_kept), shares(&kept)),
                (order_id(&s_greedy), shares(&greedy)),
                (order_id(&s_silent), shares(&silent)[..1].to_vec()),
            ],
        },
    );
    let out = h.pulse(vec![reveal.clone()]);
    expect(&out, &reveal, Outcome::Success);
    let revealed = all_events(&out)
        .into_iter()
        .find_map(|e| match e {
            Event::BatchRevealed { revealed, .. } => Some(revealed),
            _ => None,
        })
        .expect("revealed");
    assert_eq!(revealed, 1, "only the order with enough escrow");
    // At the reveal deadline, clearing settles the known orders and refunds
    // the unknown ones in full.
    let (_, batch) = sealed_batch(&h, &market);
    let result = fold_clearing::clear(&batch).expect("clears");
    let clear = h.tx(
        "solver",
        TxKind::ClearBatch {
            market,
            batch: index,
            result,
        },
    );
    let out = h.pulse(vec![clear.clone()]);
    expect(&out, &clear, Outcome::Success);
    h.advance(4);
    h.assert_minimal_witnesses();
    h.summary("sealed edges");
}

#[test]
fn probe_claim_validation_boundaries() {
    let mut h = Harness::new(&["alice"], 100_000, 24, 4);
    let config = h.chain.state.config.clone();
    let base = || note("boundary", formulas::dual_signature());
    let mut cases: Vec<(&str, ClaimBody, Outcome)> = Vec::new();
    let mut body = base();
    body.schema = "s".repeat(128);
    cases.push(("schema 128", body, Outcome::Success));
    let mut body = base();
    body.schema = "s".repeat(129);
    cases.push(("schema 129", body, Outcome::Failed));
    let mut body = base();
    body.subject = "x".repeat(512);
    cases.push(("subject 512", body, Outcome::Success));
    let mut body = base();
    body.subject = "x".repeat(513);
    cases.push(("subject 513", body, Outcome::Failed));
    // A public payload's encoding is a 1-byte tag, a 4-byte length and the bytes.
    let mut body = base();
    body.payload = Payload::Public(vec![7; config.max_payload_bytes - 5]);
    assert_eq!(body.payload.size(), config.max_payload_bytes);
    cases.push(("payload at the limit", body, Outcome::Success));
    let mut body = base();
    body.payload = Payload::Public(vec![7; config.max_payload_bytes - 4]);
    cases.push(("payload over the limit", body, Outcome::Failed));
    let atoms = |n: usize| -> Formula {
        Formula::Or(
            (0..n)
                .map(|i| Formula::Atom(fold_core::ids::AtomId::derive(&i.to_be_bytes())))
                .collect(),
        )
    };
    let mut body = base();
    body.formula = atoms(config.max_formula_size - 1);
    assert_eq!(body.formula.size(), config.max_formula_size);
    cases.push(("formula at the limit", body, Outcome::Success));
    let mut body = base();
    body.formula = atoms(config.max_formula_size);
    cases.push(("formula over the limit", body, Outcome::Failed));
    let mut body = base();
    body.detection_permille = 1000;
    body.subject = "detection 1000".into();
    cases.push(("detection 1000", body, Outcome::Success));
    let mut body = base();
    body.detection_permille = 1001;
    cases.push(("detection 1001", body, Outcome::Failed));
    let mut body = base();
    body.valid_for = config.max_claim_window;
    body.subject = "longest".into();
    cases.push(("window at the limit", body, Outcome::Success));
    let mut body = base();
    body.valid_for = config.max_claim_window + 1;
    cases.push(("window over the limit", body, Outcome::Failed));
    let mut body = base();
    body.valid_for = 0;
    cases.push(("empty window", body, Outcome::Failed));
    let mut txs = Vec::new();
    for (_, body, _) in &cases {
        txs.push(h.tx(
            "alice",
            TxKind::IssueClaim {
                claim: body.clone(),
            },
        ));
    }
    for (label, outcome) in [
        ("c".repeat(128), Outcome::Success),
        ("c".repeat(129), Outcome::Failed),
    ] {
        let tx = h.tx(
            "alice",
            TxKind::RegisterAtom {
                class: AtomClass::Hardware,
                label,
            },
        );
        cases.push(("atom label", base(), outcome));
        txs.push(tx);
    }
    let out = h.pulse(txs.clone());
    for ((what, _, outcome), tx) in cases.iter().zip(&txs) {
        assert_eq!(
            outcome_of(&status_of(&out, tx)),
            *outcome,
            "{what}: {:?}",
            status_of(&out, tx)
        );
    }
    h.assert_minimal_witnesses();
    h.summary("claim boundaries");
}

#[test]
fn probe_charter_boundaries_and_attenuation() {
    let mut h = Harness::new(&["principal", "agent", "vendor", "other"], 100_000, 6, 1);
    let (agent, vendor, other) = (h.id("agent"), h.id("vendor"), h.id("other"));
    let now = h.pulse_number() + 1;
    let mut base = charter_caveats(vendor);
    base.receipts = false;
    base.window = 10;
    base.expires_at = Pulse(now + 60);
    base.evidence_above = Some(units(80));
    let create = |h: &mut Harness, caveats: Caveats| {
        h.tx(
            "principal",
            TxKind::CreateCharter {
                agent,
                caveats,
                funding: units(5_000),
                agent_bond: Amount::ZERO,
            },
        )
    };
    let mut at_now = base.clone();
    at_now.expires_at = Pulse(now);
    let mut zero_window = base.clone();
    zero_window.window = 0;
    let expiring = create(&mut h, at_now);
    let windowless = create(&mut h, zero_window);
    let main = create(&mut h, base.clone());
    let out = h.pulse(vec![expiring.clone(), windowless.clone(), main.clone()]);
    expect(&out, &expiring, Outcome::Failed);
    expect(&out, &windowless, Outcome::Failed);
    expect(&out, &main, Outcome::Success);
    let (charter, allotment) = charter_created(&out, &main);
    let created_at = h.pulse_number();

    let pay = |h: &mut Harness, amount: Amount| {
        let input = h.reference(&allotment);
        let tx = h.tx_with(
            "agent",
            TxKind::AgentPay {
                charter,
                payee: vendor,
                amount,
                purpose: "boundary".into(),
                evidence: Some(ClaimId::derive(b"never needed")),
            },
            vec![input],
        );
        let out = h.pulse(vec![tx.clone()]);
        outcome_of(&status_of(&out, &tx))
    };
    // Each payment runs in its own Pulse; the window (10 Pulses) started at
    // the creation Pulse C.
    // C+1: exactly the evidence threshold needs no evidence (an unneeded
    // evidence claim is never looked up); C+2: one micro-unit more does.
    assert_eq!(pay(&mut h, units(80)), Outcome::Success);
    assert_eq!(pay(&mut h, Amount(units(80).0 + 1)), Outcome::Failed);
    // C+3..C+5: filling the window exactly (250) works.
    assert_eq!(pay(&mut h, units(80)), Outcome::Success);
    assert_eq!(pay(&mut h, units(80)), Outcome::Success);
    assert_eq!(pay(&mut h, units(10)), Outcome::Success);
    assert_eq!(h.pulse_number(), created_at + 5);
    // C+6 and C+9: one more micro-unit fails within the window.
    assert_eq!(pay(&mut h, Amount(1)), Outcome::Failed, "a full window");
    h.advance_to(created_at + 8);
    assert_eq!(
        pay(&mut h, Amount(1)),
        Outcome::Failed,
        "the last Pulse of the window"
    );
    // C+10: `ctx.pulse ≥ window_start + window` starts a new window.
    assert_eq!(pay(&mut h, units(10)), Outcome::Success, "a new window");

    // Attenuation, one dimension at a time.
    let parent = base.clone();
    let narrower = |change: fn(&mut Caveats)| {
        let mut caveats = parent.clone();
        change(&mut caveats);
        caveats
    };
    let delegations: Vec<(&str, Caveats, Outcome)> = vec![
        ("identical", parent.clone(), Outcome::Success),
        ("any payee", narrower(|c| c.payees = None), Outcome::Failed),
        (
            "more payees",
            narrower(|c| {
                c.payees
                    .get_or_insert_with(BTreeSet::new)
                    .insert(AccountId::derive(b"extra"));
            }),
            Outcome::Failed,
        ),
        (
            "no payee",
            narrower(|c| c.payees = Some(BTreeSet::new())),
            Outcome::Success,
        ),
        (
            "no evidence rule",
            narrower(|c| c.evidence_above = None),
            Outcome::Failed,
        ),
        (
            "higher evidence threshold",
            narrower(|c| c.evidence_above = Some(units(81))),
            Outcome::Failed,
        ),
        (
            "lower evidence threshold",
            narrower(|c| c.evidence_above = Some(units(1))),
            Outcome::Success,
        ),
        (
            "larger payments",
            narrower(|c| c.max_per_payment = units(101)),
            Outcome::Failed,
        ),
        (
            "larger windows",
            narrower(|c| c.max_per_window = units(251)),
            Outcome::Failed,
        ),
        (
            "shorter window",
            narrower(|c| c.window = 9),
            Outcome::Failed,
        ),
        (
            "longer window",
            narrower(|c| c.window = 11),
            Outcome::Success,
        ),
        (
            "later expiry",
            narrower(|c| c.expires_at = c.expires_at.next()),
            Outcome::Failed,
        ),
        (
            "receipts on",
            narrower(|c| c.receipts = true),
            Outcome::Success,
        ),
        (
            "no delegation",
            narrower(|c| c.may_delegate = false),
            Outcome::Success,
        ),
    ];
    let mut parent_with_receipts = parent.clone();
    parent_with_receipts.receipts = true;
    let mut results = Vec::new();
    for (what, caveats, expected) in delegations {
        let input = h.reference(&allotment);
        let delegate = h.tx_with(
            "agent",
            TxKind::Delegate {
                charter,
                sub_agent: other,
                caveats,
                amount: units(1),
            },
            vec![input],
        );
        let out = h.pulse(vec![delegate.clone()]);
        let outcome = outcome_of(&status_of(&out, &delegate));
        results.push((what, outcome));
        assert_eq!(outcome, expected, "{what}");
    }
    eprintln!("attenuation: {results:?}");

    // Revoking twice keeps the first revocation; reclaiming twice returns
    // nothing the second time.
    let revoke = h.tx("principal", TxKind::RevokeCharter { charter });
    h.pulse(vec![revoke]);
    let revoked_at = h.pulse_number();
    let again = h.tx("principal", TxKind::RevokeCharter { charter });
    let out = h.pulse(vec![again.clone()]);
    expect(&out, &again, Outcome::Success);
    let ObjectData::Charter(state) = &h.chain.state.objects[&charter].data else {
        panic!("charter")
    };
    assert_eq!(state.revoked_at, Some(Pulse(revoked_at)));
    let reclaim = h.tx("principal", TxKind::ReclaimCharter { charter });
    let out = h.pulse(vec![reclaim.clone()]);
    expect(&out, &reclaim, Outcome::Success);
    let reclaim = h.tx("principal", TxKind::ReclaimCharter { charter });
    let out = h.pulse(vec![reclaim.clone()]);
    expect(&out, &reclaim, Outcome::Success);
    assert!(all_events(&out)
        .iter()
        .any(|e| matches!(e, Event::CharterReclaimed { amount, .. } if *amount == Amount::ZERO)));

    // Paying in the Pulse the caveats expire in works; one Pulse later not.
    let mut short = base.clone();
    short.expires_at = Pulse(h.pulse_number() + 2);
    short.evidence_above = None;
    let create = create(&mut h, short);
    let out = h.pulse(vec![create.clone()]);
    let (charter, allotment) = charter_created(&out, &create);
    let mut outcomes = Vec::new();
    for _ in 0..2 {
        let input = h.reference(&allotment);
        let tx = h.tx_with(
            "agent",
            TxKind::AgentPay {
                charter,
                payee: vendor,
                amount: units(1),
                purpose: "last call".into(),
                evidence: None,
            },
            vec![input],
        );
        let out = h.pulse(vec![tx.clone()]);
        outcomes.push(outcome_of(&status_of(&out, &tx)));
    }
    assert_eq!(outcomes, vec![Outcome::Success, Outcome::Failed]);
    h.assert_minimal_witnesses();
    h.summary("charter boundaries");
}

#[test]
fn probe_schedule_written_back_only_when_changed() {
    // A Schedule item for an event already scheduled at the same Pulse does
    // not change the schedule, so it is not written back (§6.3).
    let mut h = Harness::new(&["issuer", "consumer"], 100_000, 6, 1);
    let issuer = h.id("issuer");
    let stake = bond_atom(AtomClass::Economic, &issuer, "stake");
    let bond = h.tx(
        "issuer",
        TxKind::PostBond {
            class: AtomClass::Economic,
            label: "stake".into(),
            amount: units(10_000),
        },
    );
    h.pulse(vec![bond]);
    let body = note("backed", formulas::bonded_statement(stake.id));
    let claim = claim_id(&issuer, &body);
    let tx = h.tx("issuer", TxKind::IssueClaim { claim: body });
    h.pulse(vec![tx]);
    let until = Pulse(h.pulse_number() + 30);
    let accept = |h: &mut Harness, nonce: u64| {
        h.tx(
            "consumer",
            TxKind::Accept {
                claim,
                value_at_risk: units(10),
                policy: Policy::standard(),
                until,
                nonce,
            },
        )
    };
    let first = accept(&mut h, 0);
    let out = h.pulse(vec![first.clone()]);
    expect(&out, &first, Outcome::Success);
    let schedule_version = h.chain.state.objects[&well_known::schedule()].version;
    let second = accept(&mut h, 1);
    let out = h.pulse(vec![second.clone()]);
    expect(&out, &second, Outcome::Success);
    assert_eq!(
        h.chain.state.objects[&well_known::schedule()].version,
        schedule_version,
        "the schedule did not change"
    );
    h.advance_to(until.0 + 1);
    h.assert_minimal_witnesses();
    h.summary("schedule write-back");
}

#[test]
fn probe_bondless_bonded_atoms_and_no_trade_clearing() {
    let mut h = Harness::new(
        &[
            "principal",
            "agent",
            "judge",
            "challenger",
            "operator",
            "lab",
            "solver",
        ],
        100_000,
        6,
        2,
    );
    let (principal, agent, judge) = (h.id("principal"), h.id("agent"), h.id("judge"));
    let j_atom = bond_atom(AtomClass::Adjudicator, &judge, "court");
    // A charter without an agent bond registers a bonded atom nobody bonds.
    let create = h.tx(
        "principal",
        TxKind::CreateCharter {
            agent,
            caveats: charter_caveats(h.id("lab")),
            funding: units(10),
            agent_bond: Amount::ZERO,
        },
    );
    let court = h.tx(
        "judge",
        TxKind::PostBond {
            class: AtomClass::Adjudicator,
            label: "court".into(),
            amount: units(1_000),
        },
    );
    let out = h.pulse(vec![create.clone(), court]);
    let (charter, _) = charter_created(&out, &create);
    let ObjectData::Charter(state) = &h.chain.state.objects[&charter].data else {
        panic!("charter")
    };
    let agent_atom = state.agent_atom;
    assert!(h.chain.state.registry().bond(&agent_atom).is_none());
    // The principal vouches with the agent's atom under the judge; a claim
    // naming an unregistered adjudicator is rejected.
    let mut vouch = note("my agent behaves", formulas::bonded_statement(agent_atom));
    vouch.adjudicator = Some(j_atom.id);
    let vouch_id = claim_id(&principal, &vouch);
    let mut orphan = note("judged by nobody", formulas::dual_signature());
    orphan.adjudicator = Some(fold_core::ids::AtomId::derive(b"no court"));
    let txs = vec![
        h.tx("principal", TxKind::IssueClaim { claim: vouch }),
        h.tx("principal", TxKind::IssueClaim { claim: orphan }),
    ];
    let out = h.pulse(txs);
    assert_eq!(out.new_claims.len(), 1);
    let challenge = |h: &mut Harness, tag: &str| {
        h.tx(
            "challenger",
            TxKind::Challenge {
                claim: vouch_id,
                target: agent_atom,
                evidence: hash_domain("test/evidence", tag.as_bytes()),
                deposit: units(5),
            },
        )
    };
    let first = challenge(&mut h, "weak");
    let second = challenge(&mut h, "strong");
    let out = h.pulse(vec![first.clone(), second.clone()]);
    let opened: Vec<Hash32> = [&first, &second]
        .iter()
        .map(|tx| {
            out.effects
                .iter()
                .find(|e| e.digest == tx.digest())
                .and_then(|e| {
                    e.events.iter().find_map(|event| match event {
                        Event::ChallengeOpened { challenge, .. } => Some(*challenge),
                        _ => None,
                    })
                })
                .expect("opened")
        })
        .collect();
    // Dismissed: with no bond behind the target, the deposit goes back to
    // the challenger. Upheld: nothing to slash, the claim is falsified.
    let challenger_before = h.balance("challenger", &usd());
    let rulings = vec![
        h.tx(
            "judge",
            TxKind::Rule {
                challenge: opened[0],
                upheld: false,
            },
        ),
        h.tx(
            "judge",
            TxKind::Rule {
                challenge: opened[1],
                upheld: true,
            },
        ),
    ];
    let out = h.pulse(rulings.clone());
    for tx in &rulings {
        expect(&out, tx, Outcome::Success);
    }
    assert_eq!(
        h.balance("challenger", &usd()),
        challenger_before.checked_add(units(10)).unwrap()
    );
    assert!(all_events(&out)
        .iter()
        .any(|e| matches!(e, Event::Slashed { amount, .. } if *amount == Amount::ZERO)));
    assert!(all_events(&out)
        .iter()
        .any(|e| matches!(e, Event::Falsified { claim, .. } if *claim == vouch_id)));

    // A batch with only a sell order clears with no trade: everything is
    // refunded by the clearing itself.
    h.fund(&[("lab", gpu(), units(5))]);
    let tx = h.tx(
        "operator",
        TxKind::CreateMarket {
            label: "quiet".into(),
            good: gpu(),
            numeraire: usd(),
            batch_length: 4,
            clear_deadline: 3,
            committee: Vec::new(),
            threshold: 0,
        },
    );
    let out = h.pulse(vec![tx]);
    let market = all_events(&out)
        .into_iter()
        .find_map(|e| match e {
            Event::MarketCreated { market } => Some(market),
            _ => None,
        })
        .expect("market");
    align_to_window(&mut h);
    let ask = order(&mut h, "lab", market, Side::Sell, 5, 100);
    h.pulse(vec![ask]);
    h.advance(3);
    let (index, batch) = sealed_batch(&h, &market);
    let result = fold_clearing::clear(&batch).expect("clears");
    assert_eq!(result.price, None);
    let clear = h.tx(
        "solver",
        TxKind::ClearBatch {
            market,
            batch: index,
            result,
        },
    );
    let out = h.pulse(vec![clear.clone()]);
    expect(&out, &clear, Outcome::Success);
    assert_eq!(h.balance("lab", &gpu()), units(5));
    h.advance(4);
    h.assert_minimal_witnesses();
    h.summary("bondless atoms");
}

#[test]
fn probe_bonded_means_the_atom_flag_and_windows_saturate() {
    let mut h = Harness::new(&["principal", "agent", "mallory"], 10_000, 6, 1);
    let agent = h.id("agent");
    let create = h.tx(
        "principal",
        TxKind::CreateCharter {
            agent,
            caveats: charter_caveats(h.id("mallory")),
            funding: units(10),
            agent_bond: Amount::ZERO,
        },
    );
    let out = h.pulse(vec![create.clone()]);
    let (charter, _) = charter_created(&out, &create);
    let ObjectData::Charter(state) = &h.chain.state.objects[&charter].data else {
        panic!("charter")
    };
    let agent_atom = state.agent_atom;
    assert!(h.chain.state.registry().atom(&agent_atom).unwrap().bonded);
    assert!(h.chain.state.registry().bond(&agent_atom).is_none());
    // Mallory rests a claim on the principal's bonded (but unfunded) atom:
    // §6.3 rejects it because the atom is bonded and not Mallory's.
    let borrowed = note(
        "vouched by someone else's atom",
        formulas::bonded_statement(agent_atom),
    );
    let borrowed_id = claim_id(&h.id("mallory"), &borrowed);
    // A claim whose challenge period saturates the Pulse counter.
    let mut endless = note("challengeable forever", formulas::dual_signature());
    endless.challenge_period = u64::MAX;
    let txs = vec![
        h.tx("mallory", TxKind::IssueClaim { claim: borrowed }),
        h.tx("principal", TxKind::IssueClaim { claim: endless }),
    ];
    let out = h.pulse(txs);
    let rejected: Vec<ClaimId> = all_events(&out)
        .into_iter()
        .filter_map(|e| match e {
            Event::ClaimRejected { claim, .. } => Some(claim),
            _ => None,
        })
        .collect();
    assert_eq!(rejected, vec![borrowed_id]);
    assert_eq!(out.new_claims.len(), 1);
    h.advance(3);
    h.assert_minimal_witnesses();
    h.summary("bonded flag");
}

#[test]
fn probe_new_account_edges() {
    let mut h = Harness::new(&["alice"], 10_000, 4, 1);
    let alice = h.id("alice");
    let gina = h.chain.add_wallet("gina");
    let hugo = h.chain.add_wallet("hugo");
    let ivan = h.chain.add_wallet("ivan");
    let gift = h.tx(
        "alice",
        TxKind::Transfer {
            asset: usd(),
            outputs: vec![
                (gina, units(50)),
                (gina, units(50)),
                (hugo, units(50)),
                (ivan, units(50)),
                (ivan, units(50)),
            ],
        },
    );
    h.pulse(vec![gift]);
    // Gina's first transaction rotates her policy, naming the account
    // object that its own preparation creates (version 0) as an input.
    let fresh = fold_exec::Wallet::named("gina-rotated");
    let mut rotate = h
        .tx(
            "gina",
            TxKind::Transfer {
                asset: usd(),
                outputs: vec![(alice, units(1))],
            },
        )
        .tx;
    assert!(rotate.genesis_policy.is_some());
    rotate.kind = TxKind::RotatePolicy {
        policy: fresh.policy.clone(),
        viewing_key: None,
    };
    rotate.inputs.push(ObjectRef {
        id: well_known::account(&gina),
        version: Version(0),
    });
    rotate.fee = h.chain.state.config.min_fee(rotate.size());
    let rotate = h.resign("gina", rotate);
    // Hugo's first transaction carries a broken signature: the account it
    // would create is discarded with the rejection.
    let mut forged = h.tx(
        "hugo",
        TxKind::Transfer {
            asset: usd(),
            outputs: vec![(alice, units(1))],
        },
    );
    forged.auth.signatures[1].signature.bytes[5] ^= 1;
    // Ivan sends two first transactions in one Pulse: whichever runs first
    // creates the account, the other finds it.
    let ivan_one = h.tx(
        "ivan",
        TxKind::Transfer {
            asset: usd(),
            outputs: vec![(alice, units(1))],
        },
    );
    let ivan_two = h.tx(
        "ivan",
        TxKind::IssueClaim {
            claim: note("hello from ivan", formulas::dual_signature()),
        },
    );
    assert!(ivan_one.tx.genesis_policy.is_some() && ivan_two.tx.genesis_policy.is_some());
    let out = h.pulse(vec![
        rotate.clone(),
        forged.clone(),
        ivan_one.clone(),
        ivan_two.clone(),
    ]);
    for (name, tx) in [
        ("rotation naming its own new account", &rotate),
        ("forged first transaction", &forged),
        ("ivan one", &ivan_one),
        ("ivan two", &ivan_two),
    ] {
        eprintln!("{name}: {:?}", status_of(&out, tx));
    }
    expect(&out, &rotate, Outcome::Success);
    expect(&out, &forged, Outcome::Rejected);
    assert!(!h
        .chain
        .state
        .objects
        .contains_key(&well_known::account(&hugo)));
    expect(&out, &ivan_one, Outcome::Success);
    expect(&out, &ivan_two, Outcome::Success);
    h.assert_minimal_witnesses();
    h.summary("new accounts");
}

/// Stack A's `bounds_keep_user_chosen_numbers_far_from_overflow` scenario
/// (`crates/fold-exec/tests/scenarios.rs`), replayed through the harness so
/// stack B verifies every chunk it produces and every witness is minimal.
#[test]
fn stack_a_bounds_scenario_verifies() {
    use fold_exec::charter::MAX_CHARTER_CHAIN;
    use fold_exec::market::MAX_PERIOD;
    use fold_exec::sim::FAUCET;
    use fold_exec::tx::MAX_GRANT;

    let mut h = Harness::new(&["operator", "principal", "agent", "vendor"], 10_000, 4, 1);
    let (agent, vendor) = (h.id("agent"), h.id("vendor"));

    // Market periods.
    for (deadline, ok) in [
        (u64::MAX, false),
        (MAX_PERIOD + 1, false),
        (MAX_PERIOD, true),
    ] {
        let tx = h.tx(
            "operator",
            TxKind::CreateMarket {
                label: "periods".into(),
                good: gpu(),
                numeraire: usd(),
                batch_length: 2,
                clear_deadline: deadline,
                committee: Vec::new(),
                threshold: 0,
            },
        );
        let out = h.pulse(vec![tx.clone()]);
        expect(
            &out,
            &tx,
            if ok {
                Outcome::Success
            } else {
                Outcome::Failed
            },
        );
    }

    // Faucet grants are capped, and faucet transactions pay no fee.
    let faucet = h.id(FAUCET);
    let out = h.fund(&[("vendor", usd(), MAX_GRANT.checked_add(Amount(1)).unwrap())]);
    assert_eq!(outcome_of(&out.effects[0].status), Outcome::Failed);
    let paying = Transaction {
        sender: faucet,
        genesis_policy: None,
        inputs: vec![h.reference(&well_known::account(&faucet))],
        fee: units(1),
        kind: TxKind::Faucet {
            grants: vec![(vendor, usd(), units(1))],
        },
        expires_at: Pulse(h.pulse_number() + 10),
        nonce: 99,
    };
    let paying = h.resign(FAUCET, paying);
    let out = h.pulse(vec![paying.clone()]);
    expect(&out, &paying, Outcome::Rejected);

    // A charter window of u64::MAX never resets.
    let mut caveats = charter_caveats(vendor);
    caveats.window = u64::MAX;
    caveats.max_per_window = units(150);
    let create = h.tx(
        "principal",
        TxKind::CreateCharter {
            agent,
            caveats: caveats.clone(),
            funding: units(1_000),
            agent_bond: Amount::ZERO,
        },
    );
    let out = h.pulse(vec![create.clone()]);
    expect(&out, &create, Outcome::Success);
    let (root, root_allotment) = charter_created(&out, &create);
    let pay = |h: &mut Harness, charter: ObjectId, allotment: ObjectId, amount: u64| {
        let input = h.reference(&allotment);
        h.tx_with(
            "agent",
            TxKind::AgentPay {
                charter,
                payee: vendor,
                amount: units(amount),
                purpose: "bounds".into(),
                evidence: None,
            },
            vec![input],
        )
    };
    let first = pay(&mut h, root, root_allotment, 100);
    let out = h.pulse(vec![first.clone()]);
    expect(&out, &first, Outcome::Success);
    h.advance(50);
    let second = pay(&mut h, root, root_allotment, 100);
    let out = h.pulse(vec![second.clone()]);
    expect(&out, &second, Outcome::Failed);

    // Delegation chains up to MAX_CHARTER_CHAIN charters, and one deeper.
    let (mut charter, mut allotment) = (root, root_allotment);
    let mut levels = vec![(charter, allotment)];
    for level in 1..=MAX_CHARTER_CHAIN as u64 {
        let passed = 5 * (MAX_CHARTER_CHAIN as u64 + 1 - level) + 10;
        let input = h.reference(&allotment);
        let delegate = h.tx_with(
            "agent",
            TxKind::Delegate {
                charter,
                sub_agent: agent,
                caveats: caveats.clone(),
                amount: units(passed),
            },
            vec![input],
        );
        let out = h.pulse(vec![delegate.clone()]);
        expect(&out, &delegate, Outcome::Success);
        (charter, allotment) = charter_created(&out, &delegate);
        levels.push((charter, allotment));
    }
    let (at_limit, at_limit_allotment) = levels[MAX_CHARTER_CHAIN - 1];
    let ok = pay(&mut h, at_limit, at_limit_allotment, 5);
    let out = h.pulse(vec![ok.clone()]);
    expect(&out, &ok, Outcome::Success);
    let (beyond, beyond_allotment) = levels[MAX_CHARTER_CHAIN];
    let deep = pay(&mut h, beyond, beyond_allotment, 5);
    let out = h.pulse(vec![deep.clone()]);
    expect(&out, &deep, Outcome::Failed);
    h.assert_minimal_witnesses();
    h.summary("stack A bounds scenario");
}

// ---------------------------------------------------------------------------
// Spec v1.2 boundaries
// ---------------------------------------------------------------------------

#[test]
fn probe_signatures_are_bound_to_the_chain() {
    // §4.1 step 5 (v1.2): signatures verify under `signing_context(config)`,
    // so a transaction signed for another chain, or under the bare
    // transaction domain, is rejected; the same body signed for this chain
    // succeeds.
    let mut h = Harness::new(&["alice", "bob"], 1_000, 4, 1);
    let bob = h.id("bob");
    let body = h
        .tx(
            "alice",
            TxKind::Transfer {
                asset: usd(),
                outputs: vec![(bob, units(3))],
            },
        )
        .tx;
    let mut elsewhere = h.chain.state.config.clone();
    elsewhere.chain_id = "another-chain".into();
    let keys = h.chain.wallet("alice").keys.clone();
    let foreign = body.clone().sign(&keys, &elsewhere).expect("signing");
    let bare = SignedTransaction {
        auth: keys
            .authorize(fold_exec::tx::TX_DOMAIN, &body.digest().0 .0)
            .expect("signing"),
        tx: body.clone(),
    };
    let out = h.pulse(vec![foreign.clone()]);
    expect(&out, &foreign, Outcome::Rejected);
    let out = h.pulse(vec![bare.clone()]);
    expect(&out, &bare, Outcome::Rejected);
    let native = h.resign("alice", body);
    let out = h.pulse(vec![native.clone()]);
    expect(&out, &native, Outcome::Success);
    h.assert_minimal_witnesses();
    h.summary("chain-bound signatures");
}

#[test]
fn probe_input_count_limit() {
    // §4.1 step 2 (v1.2): at most MAX_INPUTS (256) inputs.
    use fold_exec::tx::MAX_INPUTS;
    let mut h = Harness::new(&["whale", "bob"], 300_000, 300, 1);
    let (whale, bob) = (h.id("whale"), h.id("bob"));
    let coins: Vec<ObjectRef> = h
        .chain
        .state
        .coins_of(&whale)
        .iter()
        .map(|coin| coin.reference())
        .collect();
    assert!(coins.len() > MAX_INPUTS);
    let with_inputs = |h: &Harness, inputs: &[ObjectRef], nonce: u64| {
        let mut body = Transaction {
            sender: whale,
            genesis_policy: None,
            inputs: inputs.to_vec(),
            fee: Amount::ZERO,
            kind: TxKind::Transfer {
                asset: usd(),
                outputs: vec![(bob, units(1))],
            },
            expires_at: Pulse(h.pulse_number() + 10),
            nonce,
        };
        body.fee = h.chain.state.config.min_fee(body.size());
        h.resign("whale", body)
    };
    let too_many = with_inputs(&h, &coins[..=MAX_INPUTS], 1);
    let out = h.pulse(vec![too_many.clone()]);
    expect(&out, &too_many, Outcome::Rejected);
    let most = with_inputs(&h, &coins[..MAX_INPUTS], 2);
    let out = h.pulse(vec![most.clone()]);
    expect(&out, &most, Outcome::Success);
    let consumed = out
        .effects
        .iter()
        .find(|e| e.digest == most.digest())
        .map(|e| e.consumed.len())
        .unwrap();
    assert_eq!(consumed, MAX_INPUTS, "every input coin is drawn");
    h.assert_minimal_witnesses();
    h.summary("input limit");
}

#[test]
fn probe_allotment_count_limit() {
    // §5.18 (v1.2): at most MAX_ALLOTMENTS (256) allotments.
    use fold_exec::escrow::MAX_ALLOTMENTS;
    let mut h = Harness::new(&["manager", "writer"], 10_000, 4, 1);
    let writer = h.id("writer");
    let create = |h: &mut Harness, count: usize| {
        h.tx(
            "manager",
            TxKind::CreateBounded {
                label: format!("{count} allotments"),
                asset: usd(),
                total: units(1_000),
                allotments: vec![(writer, Amount(1)); count],
            },
        )
    };
    let most = create(&mut h, MAX_ALLOTMENTS);
    let too_many = create(&mut h, MAX_ALLOTMENTS + 1);
    let out = h.pulse(vec![most.clone(), too_many.clone()]);
    expect(&out, &most, Outcome::Success);
    expect(&out, &too_many, Outcome::Failed);
    let created = out
        .effects
        .iter()
        .find(|e| e.digest == most.digest())
        .map(|e| e.created.len())
        .unwrap();
    // The bounded total and its allotments (plus the change coin).
    assert!(created > MAX_ALLOTMENTS);
    h.assert_minimal_witnesses();
    h.summary("allotment limit");
}

#[test]
fn probe_batch_order_limit() {
    // §6.3 AddOrder (v1.2): a batch holds at most MAX_BATCH_ORDERS (1,024)
    // orders; later orders in its window are refunded at the end of their
    // Pulse.
    use fold_exec::market::MAX_BATCH_ORDERS;
    const TRADERS: [&str; 5] = ["t0", "t1", "t2", "t3", "t4"];
    let mut names = vec!["operator"];
    names.extend(TRADERS);
    let mut h = Harness::new(&names, 10_000, 215, 128);
    let tx = h.tx(
        "operator",
        TxKind::CreateMarket {
            label: "crowded".into(),
            good: gpu(),
            numeraire: usd(),
            batch_length: 4,
            clear_deadline: 2,
            committee: Vec::new(),
            threshold: 0,
        },
    );
    let out = h.pulse(vec![tx]);
    let market = all_events(&out)
        .into_iter()
        .find_map(|e| match e {
            Event::MarketCreated { market } => Some(market),
            _ => None,
        })
        .expect("market");
    align_to_window(&mut h);
    let tiny = |h: &mut Harness, who: &str| {
        h.tx(
            who,
            TxKind::SubmitOrder {
                market,
                submission: Submission::Plain(OrderTerms {
                    side: Side::Buy,
                    quantity: 1,
                    limit_price: Amount(1),
                }),
            },
        )
    };
    // One Pulse with MAX_BATCH_ORDERS + 1 orders: the 1,025th in sort order
    // (by order id) is refunded.
    let orders: Vec<SignedTransaction> = (0..=MAX_BATCH_ORDERS)
        .map(|i| tiny(&mut h, TRADERS[i % TRADERS.len()]))
        .collect();
    let out = h.pulse(orders.clone());
    for tx in &orders {
        expect(&out, tx, Outcome::Success);
    }
    let pulse = h.pulse_number();
    let state = market_state(&h, &market);
    let index = pulse / state.batch_length;
    let batch = &state.batches[&index];
    assert_eq!(batch.orders.len(), MAX_BATCH_ORDERS);
    let highest = orders
        .iter()
        .map(|tx| fold_core::hash::hash_parts("fold/market/order", &[&tx.digest().0 .0]))
        .max()
        .unwrap();
    assert!(batch.orders.iter().all(|order| order.id != highest));
    assert!(h
        .chain
        .state
        .objects
        .contains_key(&deferred_coin_id(Pulse(pulse), 0)));
    // The next Pulse is in the same window: the full batch refunds again.
    let late = tiny(&mut h, "t0");
    let out = h.pulse(vec![late.clone()]);
    expect(&out, &late, Outcome::Success);
    assert!(h
        .chain
        .state
        .objects
        .contains_key(&deferred_coin_id(Pulse(pulse + 1), 0)));
    assert_eq!(
        market_state(&h, &market).batches[&index].orders.len(),
        MAX_BATCH_ORDERS
    );
    // Nobody clears: the deadline refunds all 1,024 orders in one phase.
    h.advance(6);
    assert_eq!(
        market_state(&h, &market).batches[&index].status,
        BatchStatus::Refunded
    );
    h.assert_minimal_witnesses();
    h.summary("batch order limit");
}
