//! Differential tests for proof stack B.
//!
//! The reference executor (stack A, `sion_exec::sim::Chain`) runs workloads
//! that exercise every transaction kind and the deferred phase, recording
//! the honest chunk traces of every Pulse. Stack B
//! ([`sion_spec::verify_chunk`]), written independently from the
//! specification, must accept every one of them, and must reject traces
//! whose statements, witnesses, transactions or deferred items were
//! tampered with.
//!
//! When stack B rejects an honest chunk, the failure message diffs stack
//! B's execution ([`sion_spec::execute_chunk`]) against the effects and
//! writes stack A recorded, transaction by transaction and object by object.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

use proptest::prelude::*;
use sion_assurance::{AtomClass, Formula, Policy};
use sion_clearing::Side;
use sion_consensus::{
    sample_committee, Authenticator, Committee, Header, LaneRequest, LaneValidator,
    OwnerAuthorization, SignedHeader,
};
use sion_core::encode::encode;
use sion_core::hash::{hash_domain, Hash32};
use sion_core::ids::{
    AccountId, AssetId, ClaimId, Epoch, ObjectId, Pulse, Round, ShardId, TxDigest, ValidatorId,
    Version,
};
use sion_core::Amount;
use sion_crypto::{
    validator_id, CommitteeEnvelope, DualKeypair, KemKeypair, Scheme, VALIDATOR_DOMAIN,
};
use sion_exec::charter::{card_commitment, Caveats, Spawn, MAX_CHARTER_CHAIN, MAX_SPAWN_BATCH};
use sion_exec::claim::{claim_id, formulas, ClaimBody, Payload};
use sion_exec::execute::{bond_atom, registered_atom, Deferred, Event, ExecStatus};
use sion_exec::market::{BatchStatus, Market, OrderTerms, Submission, SEALED_ORDER_DOMAIN};
use sion_exec::object::{
    created_id, well_known, CoordinationClass, Object, ObjectData, ObjectRef, Owner, ScheduledEvent,
};
use sion_exec::pulse::ChunkTrace;
use sion_exec::sim::Chain;
use sion_exec::validators::{
    possession_digest, Equivocation, EvidenceApproval, ForcedObligations, ForcedParams,
    ValidatorParams, ValidatorRecord, ValidatorSet, ValidatorStatus,
};
use sion_exec::{PulseOutput, SignedTransaction, Transaction, TxKind};
use sion_spec::commit::{chain_deferred, deferred_coin_id, effects_leaf};
use sion_spec::{execute_chunk, verify_chunk, ChunkReport, SpecError};

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
        TxKind::RegisterValidator { .. } => "RegisterValidator",
        TxKind::Unstake { .. } => "Unstake",
        TxKind::WithdrawStake { .. } => "WithdrawStake",
        TxKind::ClaimRewards { .. } => "ClaimRewards",
        TxKind::ReportEquivocation { .. } => "ReportEquivocation",
        TxKind::OpenStream {
            checkpoints: Some(_),
            ..
        } => "OpenStream(checkpoints)",
        TxKind::OpenStream { .. } => "OpenStream",
        TxKind::SettleStream { jump: Some(_), .. } => "SettleStream(jump)",
        TxKind::SettleStream { .. } => "SettleStream",
        TxKind::TopUpStream { .. } => "TopUpStream",
        TxKind::CloseStream { .. } => "CloseStream",
        TxKind::RegisterAgent { .. } => "RegisterAgent",
        TxKind::SpawnAgents { .. } => "SpawnAgents",
        TxKind::SetAgentCard { .. } => "SetAgentCard",
        TxKind::SuspendAgent { .. } => "SuspendAgent",
        TxKind::ResumeAgent { .. } => "ResumeAgent",
        TxKind::OpenMemory { agent: Some(_), .. } => "OpenMemory(agent)",
        TxKind::OpenMemory { .. } => "OpenMemory",
        TxKind::CommitMemory { .. } => "CommitMemory",
        TxKind::PostTask {
            charter: Some(_),
            worker: Some(_),
            ..
        } => "PostTask(agent)",
        TxKind::PostTask {
            charter: Some(_), ..
        } => "PostTask(agent, open)",
        TxKind::PostTask { worker: None, .. } => "PostTask(open)",
        TxKind::PostTask { .. } => "PostTask",
        TxKind::SettleTask { .. } => "SettleTask",
        TxKind::CancelTask { .. } => "CancelTask",
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
        Deferred::Fee { .. } => "Fee",
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
        ScheduledEvent::ForcedDeadline { .. } => "ForcedDeadline",
        ScheduledEvent::EpochEnd => "EpochEnd",
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
            assert_eq!(
                report.transactions + report.intents,
                trace.transactions.len()
            );
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
    /// Forced intents posted in verified deferred phases.
    intents: usize,
    /// What the verified chunks exercised.
    coverage: BTreeSet<String>,
    /// Witness entries stack A included that stack B neither read nor
    /// wrote (spec §2.1: an implementation must not read more than the
    /// specification requires).
    unread: Vec<String>,
    /// The treasury's USD balance after the last epoch end (at genesis, its
    /// genesis balance), for the epoch-reward oracle.
    epoch_balance: Amount,
    /// Epoch ends checked against the v1.4 reward rule.
    epochs: usize,
}

impl Harness {
    fn new(names: &[&str], usd: u64, coins: u32, chunk_size: usize) -> Harness {
        Harness::from_chain(Chain::new(names, usd, coins), chunk_size)
    }

    /// A chain with these validator and forced-inclusion parameters (spec
    /// v1.3); its genesis has the simulation's four validators, operated by
    /// the faucet account.
    fn with_params(
        names: &[&str],
        usd: u64,
        coins: u32,
        chunk_size: usize,
        validators: ValidatorParams,
        forced: ForcedParams,
    ) -> Harness {
        Harness::from_chain(
            Chain::with_params(names, usd, coins, validators, forced),
            chunk_size,
        )
    }

    fn from_chain(mut chain: Chain, chunk_size: usize) -> Harness {
        chain.chunk_size = chunk_size;
        let epoch_balance = match &chain.state.objects[&well_known::treasury()].data {
            ObjectData::Treasury(treasury) => treasury
                .balances
                .get(&usd())
                .copied()
                .unwrap_or(Amount::ZERO),
            other => panic!("the treasury object holds {other:?}"),
        };
        Harness {
            chain,
            verified: 0,
            chunks: 0,
            transactions: 0,
            intents: 0,
            coverage: BTreeSet::new(),
            unread: Vec::new(),
            epoch_balance,
            epochs: 0,
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

    /// Executes one Pulse that also posts `forced` intents; the transactions
    /// are split by coordination class, as consensus would order them.
    fn pulse_forced(
        &mut self,
        txs: Vec<SignedTransaction>,
        forced: Vec<SignedTransaction>,
    ) -> PulseOutput {
        let (contended, owned): (Vec<_>, Vec<_>) = txs
            .into_iter()
            .partition(|tx| tx.tx.class() == CoordinationClass::Contended);
        let out = self
            .chain
            .pulse_forced(owned, contended, forced)
            .expect("the reference executes the Pulse");
        self.verify_new();
        out
    }

    /// Releases every wallet's in-flight coins (after building intents that
    /// are posted rather than executed).
    fn settle(&mut self) {
        for wallet in self.chain.wallets.values_mut() {
            wallet.settle();
        }
    }

    /// The validator set as the reference chain holds it.
    fn validator_set(&self) -> ValidatorSet {
        match &self.chain.state.objects[&well_known::validators()].data {
            ObjectData::Validators(set) => (**set).clone(),
            other => panic!("the validator set object holds {other:?}"),
        }
    }

    fn record(&self, validator: &ValidatorId) -> ValidatorRecord {
        self.validator_set().validators[validator].clone()
    }

    /// The forced-inclusion obligations as the reference chain holds them.
    fn obligations(&self) -> ForcedObligations {
        match &self.chain.state.objects[&well_known::forced()].data {
            ObjectData::Forced(obligations) => (**obligations).clone(),
            other => panic!("the obligations object holds {other:?}"),
        }
    }

    /// The genesis validators (keys and identifiers), operated by the
    /// faucet account.
    fn genesis_validators(&self) -> Vec<(DualKeypair, ValidatorId)> {
        self.chain
            .validator_keys
            .iter()
            .map(|keys| (keys.clone(), key_id(keys)))
            .collect()
    }

    /// The shard that owes `name`'s forced intents.
    fn shard_of(&self, name: &str) -> u16 {
        ShardId::for_account(&self.id(name), self.chain.state.config.shard_count).0
    }

    /// The forced intents the last Pulse's deferred phase posted.
    fn last_posted(&self) -> Vec<SignedTransaction> {
        let record = self.chain.history.last().expect("a Pulse");
        let phase = record.traces.last().expect("a deferred phase");
        assert!(phase.statement.deferred_phase);
        phase.transactions.clone()
    }

    /// The coins `name` holds of `asset`, as references at their current
    /// versions.
    fn coins(&self, name: &str, asset: &AssetId) -> Vec<ObjectRef> {
        let owner = Owner::Account(self.id(name));
        self.chain
            .state
            .objects
            .values()
            .filter(|object| object.owner == owner)
            .filter(|object| object.as_coin().is_some_and(|coin| coin.asset == *asset))
            .map(Object::reference)
            .collect()
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
                self.intents += report.intents;
                for (signed, effects) in trace.transactions.iter().zip(&trace.effects) {
                    self.coverage.insert(format!(
                        "{}/{}",
                        kind_name(&signed.tx.kind),
                        outcome_tag(outcome_of(&effects.status))
                    ));
                }
                if trace.statement.deferred_phase {
                    record_deferred_coverage(trace, &mut self.coverage);
                    let rewards =
                        record
                            .output
                            .deferred_events
                            .iter()
                            .find_map(|event| match event {
                                Event::EpochChanged { rewards, .. } => Some(*rewards),
                                _ => None,
                            });
                    if let Some(rewards) = rewards {
                        let check = check_epoch_rewards(trace, rewards, self.epoch_balance);
                        self.epoch_balance = check.balance_after;
                        self.epochs += 1;
                        self.coverage.insert("epoch/rewards-from-fees".into());
                        if check.gain > Amount::ZERO && rewards > Amount::ZERO {
                            self.coverage.insert("epoch/fees-paid-out".into());
                        }
                        if check.other_income > Amount::ZERO {
                            self.coverage.insert("epoch/non-fee-income-kept".into());
                        }
                    }
                }
                self.unread.extend(unread_witness_entries(trace));
            }
        }
        self.verified = self.chain.history.len();
    }

    /// Prints what was verified (visible with `--nocapture`).
    fn summary(&self, name: &str) {
        eprintln!(
            "{name}: stack B verified {} chunks of {} Pulses ({} transactions, {} forced intents, {} epoch ends); coverage: {:?}; unread witness entries: {}",
            self.chunks,
            self.verified,
            self.transactions,
            self.intents,
            self.epochs,
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
    // Treasury growth beyond the phase's own credits (treasury credits and
    // fees), net of the epoch rewards it paid, came from registry ticks
    // releasing slashing pools.
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
                Deferred::Treasury { asset, amount } | Deferred::Fee { asset, amount }
                    if *asset == usd() =>
                {
                    Some(amount.0)
                }
                _ => None,
            })
            .sum();
        let paid: u128 = execution
            .deferred_events
            .iter()
            .filter_map(|event| match event {
                Event::EpochChanged { rewards, .. } => Some(rewards.0),
                _ => None,
            })
            .sum();
        if usd_balance(Some(after)).0 + paid > usd_balance(before).0 + credited {
            coverage.insert("due/RegistryTick-releases-pools".into());
        }
    }
}

/// What an epoch end did with the treasury.
struct EpochCheck {
    /// The treasury's USD balance after it.
    balance_after: Amount,
    /// The fees collected during the epoch.
    gain: Amount,
    /// What else the treasury received during the epoch (slashing
    /// remainders, released pools).
    other_income: Amount,
}

/// The v1.4 rule, checked against stack A's `EpochChanged` from the
/// deferred phase's witness and items: the rewards are exactly the shares
/// of `reward_permille` of the fees collected since `fees_mark`, paid to the
/// active set's Active and Leaving records; the mark moves to the fee
/// total; the treasury's other income stays in its balance.
fn check_epoch_rewards(trace: &ChunkTrace, rewards: Amount, last_balance: Amount) -> EpochCheck {
    let pulse = trace.statement.pulse.0;
    let witnessed = |id: ObjectId| {
        trace
            .witness
            .objects
            .iter()
            .find(|(entry, _, _)| *entry == id)
            .and_then(|(_, object, _)| object.clone())
            .map(|object| object.data)
    };
    let written = |id: ObjectId| {
        trace
            .writes
            .iter()
            .find(|object| object.id == id)
            .map(|object| object.data.clone())
    };
    let Some(ObjectData::Validators(set)) = witnessed(well_known::validators()) else {
        panic!("pulse {pulse}: an epoch end reads the validator set");
    };
    let Some(ObjectData::Treasury(treasury)) = witnessed(well_known::treasury()) else {
        panic!("pulse {pulse}: an epoch end reads the treasury");
    };
    let fees_in_phase: u128 = trace
        .deferred_items
        .iter()
        .filter_map(|item| match item {
            Deferred::Fee { asset, amount } if *asset == usd() => Some(amount.0),
            _ => None,
        })
        .sum();
    let collected =
        Amount(treasury.fees.get(&usd()).copied().unwrap_or(Amount::ZERO).0 + fees_in_phase);
    let gain = Amount(collected.0.saturating_sub(set.fees_mark.0));
    let pool = sion_exec::validators::permille_of(gain, set.params.reward_permille);
    let total: u64 = set.active.iter().map(|(_, weight)| *weight).sum();
    let expected: u128 = set
        .active
        .iter()
        .filter(|(validator, _)| {
            matches!(
                set.validators.get(validator).map(|record| record.status),
                Some(ValidatorStatus::Active | ValidatorStatus::Leaving)
            )
        })
        .map(|(_, weight)| sion_exec::validators::share_of(pool, *weight, total).0)
        .sum();
    assert_eq!(
        rewards,
        Amount(expected),
        "pulse {pulse}: the rewards are the shares of {} permille of the {gain} in fees collected",
        set.params.reward_permille
    );
    let Some(ObjectData::Validators(after)) = written(well_known::validators()) else {
        panic!("pulse {pulse}: an epoch end writes the validator set");
    };
    assert_eq!(
        after.fees_mark, collected,
        "pulse {pulse}: the mark is the fee total"
    );
    let Some(ObjectData::Treasury(after)) = written(well_known::treasury()) else {
        panic!("pulse {pulse}: an epoch end writes the treasury");
    };
    assert_eq!(
        after.fees.get(&usd()).copied().unwrap_or(Amount::ZERO),
        collected
    );
    let balance_after = *after
        .balances
        .get(&usd())
        .expect("the epoch end writes the USD balance");
    let income = Amount(
        (balance_after.0 + rewards.0)
            .checked_sub(last_balance.0)
            .unwrap_or_else(|| {
                panic!("pulse {pulse}: the treasury lost USD other than through rewards")
            }),
    );
    assert!(
        income >= gain,
        "pulse {pulse}: the treasury received {income} in the epoch, less than its {gain} in fees"
    );
    EpochCheck {
        balance_after,
        gain,
        other_income: Amount(income.0 - gain.0),
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
    let mut weak = sion_exec::Wallet::named("alice-weak").policy;
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
    let fresh = sion_exec::Wallet::named("alice-rotated");
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
    let mut rotated = sion_exec::Wallet::with_keys(fresh.keys.clone());
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
        "deferred/Fee",
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
        sion_assurance::ClaimStatus::Expired
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

const ASSURANCE_NAMES: [&str; 7] = [
    "supplier",
    "shipper",
    "judge",
    "financier",
    "lender",
    "buyer",
    "mallory",
];

fn assurance(chunk_size: usize) -> Harness {
    assurance_on(Harness::new(&ASSURANCE_NAMES, 200_000, 4, chunk_size))
}

/// The assurance scenario on `h`, a chain with the `ASSURANCE_NAMES`
/// accounts.
fn assurance_on(mut h: Harness) -> Harness {
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

/// v1.4: the assurance scenario on a chain whose epochs end every four
/// Pulses. Its slashings send remainders to the treasury and its registry
/// ticks release pools into it, in epochs that also collect fees; every
/// epoch end pays exactly its share of the fees and keeps the rest (the
/// harness checks each one against the rule).
#[test]
fn rewards_come_from_fees_while_slashings_and_released_pools_fill_the_treasury() {
    let h = assurance_on(Harness::with_params(
        &ASSURANCE_NAMES,
        200_000,
        4,
        3,
        validator_params(4),
        forced_params(3, 8),
    ));
    h.assert_covered(&[
        "deferred/Treasury",
        "deferred/Fee",
        "due/RegistryTick-releases-pools",
        "epoch/rewards-from-fees",
        "epoch/fees-paid-out",
        "epoch/non-fee-income-kept",
    ]);
    h.summary("assurance with epochs");
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
// Scenario: agent identities (spec v1.8)
// ---------------------------------------------------------------------------

fn agent_registered(out: &PulseOutput, tx: &SignedTransaction) -> (ObjectId, ObjectId) {
    out.effects
        .iter()
        .find(|e| e.digest == tx.digest())
        .and_then(|e| {
            e.events.iter().find_map(|event| match event {
                Event::AgentRegistered {
                    agent, allotment, ..
                } => Some((*agent, *allotment)),
                _ => None,
            })
        })
        .expect("agent registered")
}

/// The (agent, allotment) pairs a spawn created, in order.
fn agents_spawned(out: &PulseOutput, tx: &SignedTransaction) -> Vec<(ObjectId, ObjectId)> {
    out.effects
        .iter()
        .find(|e| e.digest == tx.digest())
        .map(|e| {
            e.events
                .iter()
                .filter_map(|event| match event {
                    Event::AgentSpawned {
                        agent, allotment, ..
                    } => Some((*agent, *allotment)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn charter_state(h: &Harness, id: &ObjectId) -> sion_exec::charter::Charter {
    match &h.chain.state.objects[id].data {
        ObjectData::Charter(charter) => charter.clone(),
        other => panic!("{id} holds {other:?}"),
    }
}

fn spawn_child(agent: AccountId, caveats: Caveats, budget: Amount, card: &[u8]) -> Spawn {
    Spawn {
        agent,
        card: card_commitment(card),
        caveats,
        budget,
    }
}

fn spawn(
    h: &mut Harness,
    who: &str,
    charter: ObjectId,
    allotment: ObjectId,
    children: Vec<Spawn>,
) -> SignedTransaction {
    let input = h.reference(&allotment);
    h.tx_with(who, TxKind::SpawnAgents { charter, children }, vec![input])
}

fn agent_pay(
    h: &mut Harness,
    who: &str,
    charter: ObjectId,
    allotment: ObjectId,
    payee: AccountId,
    amount: Amount,
) -> SignedTransaction {
    let input = h.reference(&allotment);
    h.tx_with(
        who,
        TxKind::AgentPay {
            charter,
            payee,
            amount,
            purpose: "a tool call".into(),
            evidence: None,
        },
        vec![input],
    )
}

fn agents(chunk_size: usize) -> Harness {
    let mut h = Harness::new(
        &[
            "principal",
            "orchestrator",
            "worker-a",
            "worker-b",
            "sub-a",
            "vendor",
            "stranger",
            "deep",
        ],
        10_000,
        4,
        chunk_size,
    );
    let (orchestrator, worker_a, worker_b, sub_a, vendor, deep) = (
        h.id("orchestrator"),
        h.id("worker-a"),
        h.id("worker-b"),
        h.id("sub-a"),
        h.id("vendor"),
        h.id("deep"),
    );

    // Registration, and a registration whose caveats have already expired.
    let register = h.tx(
        "principal",
        TxKind::RegisterAgent {
            agent: orchestrator,
            card: card_commitment(b"orchestrator card v1"),
            caveats: charter_caveats(vendor),
            funding: units(1_000),
            agent_bond: units(100),
        },
    );
    let mut expired = charter_caveats(vendor);
    expired.expires_at = Pulse(0);
    let stillborn = h.tx(
        "principal",
        TxKind::RegisterAgent {
            agent: orchestrator,
            card: Hash32::ZERO,
            caveats: expired,
            funding: units(1),
            agent_bond: Amount::ZERO,
        },
    );
    let out = h.pulse(vec![register.clone(), stillborn.clone()]);
    expect(&out, &register, Outcome::Success);
    expect(&out, &stillborn, Outcome::Failed);
    let (root, root_allotment) = agent_registered(&out, &register);
    let identity = charter_state(&h, &root).identity.expect("an identity");
    assert_eq!(identity.depth, 0);
    assert_eq!(identity.card, card_commitment(b"orchestrator card v1"));

    // Spawning: widening caveats, budgets above the allotment, an empty and
    // an oversized batch all fail; then two workers launch.
    let mut wide = charter_caveats(vendor);
    wide.max_per_payment = units(500);
    let widening = spawn(
        &mut h,
        "orchestrator",
        root,
        root_allotment,
        vec![spawn_child(worker_a, wide, units(10), b"a")],
    );
    let out = h.pulse(vec![widening.clone()]);
    expect(&out, &widening, Outcome::Failed);
    let greedy = spawn(
        &mut h,
        "orchestrator",
        root,
        root_allotment,
        vec![
            spawn_child(worker_a, charter_caveats(vendor), units(600), b"a"),
            spawn_child(worker_b, charter_caveats(vendor), units(600), b"b"),
        ],
    );
    let out = h.pulse(vec![greedy.clone()]);
    expect(&out, &greedy, Outcome::Failed);
    let empty = spawn(&mut h, "orchestrator", root, root_allotment, Vec::new());
    let out = h.pulse(vec![empty.clone()]);
    expect(&out, &empty, Outcome::Failed);
    let oversized: Vec<Spawn> = (0..=MAX_SPAWN_BATCH)
        .map(|i| {
            spawn_child(
                AccountId::derive(format!("swarm-{i}").as_bytes()),
                charter_caveats(vendor),
                Amount::ZERO,
                b"swarm",
            )
        })
        .collect();
    let too_many = spawn(&mut h, "orchestrator", root, root_allotment, oversized);
    let out = h.pulse(vec![too_many.clone()]);
    expect(&out, &too_many, Outcome::Failed);

    let mut leaf = charter_caveats(vendor);
    leaf.max_per_payment = units(20);
    leaf.max_per_window = units(40);
    leaf.may_delegate = false;
    let mut lead = charter_caveats(vendor);
    lead.max_per_window = units(100);
    // Window limits are carved too: two children asking for 200 of a
    // 250 USD window between them fail, whatever their budgets.
    let mut half = charter_caveats(vendor);
    half.max_per_window = units(200);
    let too_fast = spawn(
        &mut h,
        "orchestrator",
        root,
        root_allotment,
        vec![
            spawn_child(worker_a, half.clone(), units(1), b"a"),
            spawn_child(worker_b, half, units(1), b"b"),
        ],
    );
    let out = h.pulse(vec![too_fast.clone()]);
    expect(&out, &too_fast, Outcome::Failed);
    let workers = spawn(
        &mut h,
        "orchestrator",
        root,
        root_allotment,
        vec![
            spawn_child(worker_a, lead.clone(), units(200), b"worker-a card"),
            spawn_child(worker_b, leaf.clone(), units(100), b"worker-b card"),
        ],
    );
    let out = h.pulse(vec![workers.clone()]);
    expect(&out, &workers, Outcome::Success);
    let spawned = agents_spawned(&out, &workers);
    assert_eq!(spawned.len(), 2);
    let ((a, a_allotment), (b, b_allotment)) = (spawned[0], spawned[1]);
    assert_eq!(charter_state(&h, &a).identity.unwrap().depth, 1);
    assert_eq!(charter_state(&h, &a).parent, Some(root));

    // A worker that may delegate launches its own subagent; one that may
    // not, fails.
    let subs = spawn(
        &mut h,
        "worker-a",
        a,
        a_allotment,
        vec![spawn_child(sub_a, leaf.clone(), units(50), b"sub-a card")],
    );
    let barred = spawn(
        &mut h,
        "worker-b",
        b,
        b_allotment,
        vec![spawn_child(sub_a, leaf.clone(), units(5), b"x")],
    );
    let out = h.pulse(vec![subs.clone(), barred.clone()]);
    expect(&out, &subs, Outcome::Success);
    expect(&out, &barred, Outcome::Failed);
    let (sub, sub_allotment) = agents_spawned(&out, &subs)[0];
    assert_eq!(charter_state(&h, &sub).identity.unwrap().depth, 2);
    let paid = agent_pay(&mut h, "sub-a", sub, sub_allotment, vendor, units(10));
    let out = h.pulse(vec![paid.clone()]);
    expect(&out, &paid, Outcome::Success);

    // Cards: only the agent sets its own.
    let card = h.tx(
        "orchestrator",
        TxKind::SetAgentCard {
            charter: root,
            card: card_commitment(b"orchestrator card v2"),
        },
    );
    let forged_card = h.tx(
        "stranger",
        TxKind::SetAgentCard {
            charter: root,
            card: Hash32::ZERO,
        },
    );
    let out = h.pulse(vec![card.clone(), forged_card.clone()]);
    expect(&out, &card, Outcome::Success);
    expect(&out, &forged_card, Outcome::Failed);
    assert_eq!(
        charter_state(&h, &root).identity.unwrap().card,
        card_commitment(b"orchestrator card v2")
    );

    // Suspension: a stranger may not; the orchestrator (worker-a's
    // principal) may, and the whole subtree stops; the backer's second
    // suspension keeps the first suspender; a suspended charter is not dead.
    let stranger_suspends = h.tx("stranger", TxKind::SuspendAgent { charter: a });
    let suspend = h.tx("orchestrator", TxKind::SuspendAgent { charter: a });
    let out = h.pulse(vec![stranger_suspends.clone(), suspend.clone()]);
    expect(&out, &stranger_suspends, Outcome::Failed);
    expect(&out, &suspend, Outcome::Success);
    let halted = agent_pay(&mut h, "sub-a", sub, sub_allotment, vendor, units(1));
    let halted_spawn = spawn(
        &mut h,
        "worker-a",
        a,
        a_allotment,
        vec![spawn_child(sub_a, leaf.clone(), units(1), b"y")],
    );
    let again = h.tx("principal", TxKind::SuspendAgent { charter: a });
    let reclaim_suspended = h.tx("orchestrator", TxKind::ReclaimCharter { charter: a });
    let out = h.pulse(vec![
        halted.clone(),
        halted_spawn.clone(),
        again.clone(),
        reclaim_suspended.clone(),
    ]);
    expect(&out, &halted, Outcome::Failed);
    expect(&out, &halted_spawn, Outcome::Failed);
    expect(&out, &again, Outcome::Success);
    expect(&out, &reclaim_suspended, Outcome::Failed);
    assert_eq!(charter_state(&h, &a).suspended_by, Some(orchestrator));

    // Resumption: neither the subagent nor a sibling may lift it; the backer
    // may; resuming what is not suspended fails.
    let by_sub = h.tx("sub-a", TxKind::ResumeAgent { charter: a });
    let by_sibling = h.tx("worker-b", TxKind::ResumeAgent { charter: a });
    let resume = h.tx("principal", TxKind::ResumeAgent { charter: a });
    let out = h.pulse(vec![by_sub.clone(), by_sibling.clone(), resume.clone()]);
    expect(&out, &by_sub, Outcome::Failed);
    expect(&out, &by_sibling, Outcome::Failed);
    expect(&out, &resume, Outcome::Success);
    let twice = h.tx("principal", TxKind::ResumeAgent { charter: a });
    let pays_again = agent_pay(&mut h, "sub-a", sub, sub_allotment, vendor, units(1));
    let out = h.pulse(vec![twice.clone(), pays_again.clone()]);
    expect(&out, &twice, Outcome::Failed);
    expect(&out, &pays_again, Outcome::Success);

    // An ancestor two levels up suspends the grandchild directly; a
    // suspension held by the backer is lifted only by the backer.
    let by_grandparent = h.tx("orchestrator", TxKind::SuspendAgent { charter: sub });
    let out = h.pulse(vec![by_grandparent.clone()]);
    expect(&out, &by_grandparent, Outcome::Success);
    let resume_own = h.tx("orchestrator", TxKind::ResumeAgent { charter: sub });
    let out = h.pulse(vec![resume_own.clone()]);
    expect(&out, &resume_own, Outcome::Success);
    let by_backer = h.tx("principal", TxKind::SuspendAgent { charter: sub });
    let out = h.pulse(vec![by_backer.clone()]);
    expect(&out, &by_backer, Outcome::Success);
    let not_theirs = h.tx("orchestrator", TxKind::ResumeAgent { charter: sub });
    let backer_resumes = h.tx("principal", TxKind::ResumeAgent { charter: sub });
    let out = h.pulse(vec![not_theirs.clone(), backer_resumes.clone()]);
    expect(&out, &not_theirs, Outcome::Failed);
    expect(&out, &backer_resumes, Outcome::Success);

    // Revocation: the subagent retires itself; the backer revokes a worker
    // it is not the principal of; a stranger may not revoke; a revoked
    // charter's funds go back to the backer.
    let retire = h.tx("sub-a", TxKind::RevokeCharter { charter: sub });
    let stranger_revokes = h.tx("stranger", TxKind::RevokeCharter { charter: b });
    let backer_revokes = h.tx("principal", TxKind::RevokeCharter { charter: b });
    let out = h.pulse(vec![
        retire.clone(),
        stranger_revokes.clone(),
        backer_revokes.clone(),
    ]);
    expect(&out, &retire, Outcome::Success);
    expect(&out, &stranger_revokes, Outcome::Failed);
    expect(&out, &backer_revokes, Outcome::Success);
    let retired_pays = agent_pay(&mut h, "sub-a", sub, sub_allotment, vendor, units(1));
    let revoked_card = h.tx(
        "worker-b",
        TxKind::SetAgentCard {
            charter: b,
            card: Hash32::ZERO,
        },
    );
    let out = h.pulse(vec![retired_pays.clone(), revoked_card.clone()]);
    expect(&out, &retired_pays, Outcome::Failed);
    expect(&out, &revoked_card, Outcome::Failed);
    let before = h.balance("principal", &usd());
    let reclaim = h.tx("orchestrator", TxKind::ReclaimCharter { charter: b });
    let out = h.pulse(vec![reclaim.clone()]);
    expect(&out, &reclaim, Outcome::Success);
    assert_eq!(
        h.balance("principal", &usd()),
        before.checked_add(units(100)).unwrap(),
        "worker-b's whole budget goes back to the backer"
    );

    // A full batch of 64 subagents in one transaction.
    let mut quiet = leaf.clone();
    quiet.max_per_window = Amount::ZERO;
    let swarm: Vec<Spawn> = (0..MAX_SPAWN_BATCH)
        .map(|i| {
            spawn_child(
                AccountId::derive(format!("swarm-{i}").as_bytes()),
                quiet.clone(),
                cents(1),
                b"swarm card",
            )
        })
        .collect();
    let launch = spawn(&mut h, "orchestrator", root, root_allotment, swarm);
    let out = h.pulse(vec![launch.clone()]);
    expect(&out, &launch, Outcome::Success);
    assert_eq!(agents_spawned(&out, &launch).len(), MAX_SPAWN_BATCH);

    // Depth: one account spawns itself generation after generation; the
    // charter that is the sixteenth of its chain cannot spawn.
    let register_deep = h.tx(
        "principal",
        TxKind::RegisterAgent {
            agent: deep,
            card: Hash32::ZERO,
            caveats: charter_caveats(vendor),
            funding: units(100),
            agent_bond: Amount::ZERO,
        },
    );
    let out = h.pulse(vec![register_deep.clone()]);
    let (mut current, mut current_allotment) = agent_registered(&out, &register_deep);
    for generation in 1..MAX_CHARTER_CHAIN {
        let tx = spawn(
            &mut h,
            "deep",
            current,
            current_allotment,
            vec![spawn_child(
                deep,
                charter_caveats(vendor),
                units(1),
                b"deep",
            )],
        );
        let out = h.pulse(vec![tx.clone()]);
        expect(&out, &tx, Outcome::Success);
        (current, current_allotment) = agents_spawned(&out, &tx)[0];
        assert_eq!(
            usize::from(charter_state(&h, &current).identity.unwrap().depth),
            generation
        );
    }
    let too_deep = spawn(
        &mut h,
        "deep",
        current,
        current_allotment,
        vec![spawn_child(
            deep,
            charter_caveats(vendor),
            units(1),
            b"deep",
        )],
    );
    let out = h.pulse(vec![too_deep.clone()]);
    expect(&out, &too_deep, Outcome::Failed);

    h.assert_covered(&[
        "RegisterAgent/ok",
        "RegisterAgent/failed",
        "SpawnAgents/ok",
        "SpawnAgents/failed",
        "SetAgentCard/ok",
        "SetAgentCard/failed",
        "SuspendAgent/ok",
        "SuspendAgent/failed",
        "ResumeAgent/ok",
        "ResumeAgent/failed",
        "RevokeCharter/ok",
        "RevokeCharter/failed",
        "ReclaimCharter/ok",
        "ReclaimCharter/failed",
        "AgentPay/ok",
        "AgentPay/failed",
    ]);
    h.assert_minimal_witnesses();
    h
}

#[test]
fn agents_register_spawn_suspend_resume_and_revoke() {
    agents(1).summary("agents");
}

#[test]
fn agents_many_transactions_per_chunk() {
    agents(5).summary("agents (chunks of 5)");
}

// ---------------------------------------------------------------------------
// Scenario: memory logs (spec v1.9)
// ---------------------------------------------------------------------------

/// The leaf hashes of a writer's first `n` atoms (the ledger sees only
/// their Merkle roots, so any distinct hashes do).
fn memory_leaves(writer: &str, n: u64) -> Vec<Hash32> {
    (0..n)
        .map(|i| sion_core::merkle::leaf_hash(format!("{writer} atom {i}").as_bytes()))
        .collect()
}

fn memory_opened(out: &PulseOutput, tx: &SignedTransaction) -> ObjectId {
    out.effects
        .iter()
        .find(|e| e.digest == tx.digest())
        .and_then(|e| {
            e.events.iter().find_map(|event| match event {
                Event::MemoryOpened { log, .. } => Some(*log),
                _ => None,
            })
        })
        .expect("memory log opened")
}

fn memory_log(h: &Harness, id: &ObjectId) -> sion_exec::memory::MemoryLog {
    match &h.chain.state.objects[id].data {
        ObjectData::MemoryLog(log) => log.clone(),
        other => panic!("{id} holds {other:?}"),
    }
}

/// A commit of `log` from its current size to `size`, over `leaves`, with
/// an honest consistency proof.
fn commit_memory(
    h: &mut Harness,
    who: &str,
    log: ObjectId,
    leaves: &[Hash32],
    size: u64,
    seal: bool,
) -> SignedTransaction {
    let old = memory_log(h, &log).size;
    let new = &leaves[..size as usize];
    let proof = sion_core::merkle::consistency_proof(new, old as usize).expect("a prefix");
    let kind = TxKind::CommitMemory {
        size,
        root: sion_core::merkle::root(new),
        index: hash_domain("test/index", &size.to_be_bytes()),
        manifest: hash_domain("test/manifest", &size.to_be_bytes()),
        proof,
        seal,
    };
    let input = h.reference(&log);
    h.tx_with(who, kind, vec![input])
}

/// A commit with an explicit head, for the failure cases.
fn commit_head(
    h: &mut Harness,
    who: &str,
    log: ObjectId,
    size: u64,
    root: Hash32,
    proof: Vec<Hash32>,
) -> SignedTransaction {
    let input = h.reference(&log);
    h.tx_with(
        who,
        TxKind::CommitMemory {
            size,
            root,
            index: Hash32::ZERO,
            manifest: Hash32::ZERO,
            proof,
            seal: false,
        },
        vec![input],
    )
}

fn memory(chunk_size: usize) -> Harness {
    let mut h = Harness::new(
        &["robot", "principal", "agent", "stranger", "vendor"],
        10_000,
        4,
        chunk_size,
    );
    let (agent, vendor) = (h.id("agent"), h.id("vendor"));
    let leaves = memory_leaves("robot", 16);

    // Opening: a plain log, and a second one to probe the empty head.
    let open = h.tx(
        "robot",
        TxKind::OpenMemory {
            agent: None,
            policy: hash_domain("test/policy", b"warehouse"),
        },
    );
    let open_second = h.tx(
        "robot",
        TxKind::OpenMemory {
            agent: None,
            policy: Hash32::ZERO,
        },
    );
    let out = h.pulse(vec![open.clone(), open_second.clone()]);
    expect(&out, &open, Outcome::Success);
    expect(&out, &open_second, Outcome::Success);
    let log = memory_opened(&out, &open);
    let second = memory_opened(&out, &open_second);
    let opened = memory_log(&h, &log);
    assert_eq!(opened.size, 0);
    assert_eq!(opened.root, sion_core::merkle::empty_root());
    assert_eq!(
        h.chain.state.objects[&log].owner,
        Owner::Account(h.id("robot"))
    );

    // Growing: 0 → 3 → 7, each with a consistency proof.
    let first = commit_memory(&mut h, "robot", log, &leaves, 3, false);
    let out = h.pulse(vec![first.clone()]);
    expect(&out, &first, Outcome::Success);
    let committed = memory_log(&h, &log);
    assert_eq!((committed.size, committed.commits), (3, 1));
    assert_eq!(committed.committed_at, Pulse(h.pulse_number()));
    let grow = commit_memory(&mut h, "robot", log, &leaves, 7, false);
    let out = h.pulse(vec![grow.clone()]);
    expect(&out, &grow, Outcome::Success);
    assert_eq!(
        memory_log(&h, &log).root,
        sion_core::merkle::root(&leaves[..7])
    );

    // A log only grows, along a proof: shrinking, rewriting at the same
    // size, a proof for another log, an overlong proof, a missing input and
    // an input that is not a log all fail.
    let shrink = commit_head(
        &mut h,
        "robot",
        log,
        5,
        sion_core::merkle::root(&leaves[..5]),
        Vec::new(),
    );
    let out = h.pulse(vec![shrink.clone()]);
    expect(&out, &shrink, Outcome::Failed);
    let rewrite = commit_head(&mut h, "robot", log, 7, Hash32::ZERO, Vec::new());
    let out = h.pulse(vec![rewrite.clone()]);
    expect(&out, &rewrite, Outcome::Failed);
    let other = memory_leaves("impostor", 9);
    let forged = commit_head(
        &mut h,
        "robot",
        log,
        9,
        sion_core::merkle::root(&other),
        sion_core::merkle::consistency_proof(&other, 7).unwrap(),
    );
    let out = h.pulse(vec![forged.clone()]);
    expect(&out, &forged, Outcome::Failed);
    let overlong = commit_head(
        &mut h,
        "robot",
        log,
        9,
        sion_core::merkle::root(&leaves[..9]),
        vec![Hash32::ZERO; sion_exec::memory::MAX_CONSISTENCY_PROOF + 1],
    );
    let out = h.pulse(vec![overlong.clone()]);
    expect(&out, &overlong, Outcome::Failed);
    let no_log = h.tx(
        "robot",
        TxKind::CommitMemory {
            size: 1,
            root: leaves[0],
            index: Hash32::ZERO,
            manifest: Hash32::ZERO,
            proof: Vec::new(),
            seal: false,
        },
    );
    let account = h.reference(&well_known::account(&h.id("robot")));
    let not_a_log = h.tx_with(
        "robot",
        TxKind::CommitMemory {
            size: 1,
            root: leaves[0],
            index: Hash32::ZERO,
            manifest: Hash32::ZERO,
            proof: Vec::new(),
            seal: false,
        },
        vec![account],
    );
    let out = h.pulse(vec![no_log.clone(), not_a_log.clone()]);
    expect(&out, &no_log, Outcome::Failed);
    expect(&out, &not_a_log, Outcome::Failed);

    // Only the writer commits: anyone else naming the log is rejected,
    // because the log is the writer's owned object.
    let foreign = commit_head(
        &mut h,
        "stranger",
        log,
        8,
        sion_core::merkle::root(&leaves[..8]),
        sion_core::merkle::consistency_proof(&leaves[..8], 7).unwrap(),
    );
    let out = h.pulse(vec![foreign.clone()]);
    expect(&out, &foreign, Outcome::Rejected);

    // Two heads for the same version, one Pulse: one executes, the other
    // finds the version consumed. A log cannot fork.
    let left = commit_memory(&mut h, "robot", log, &leaves, 8, false);
    let right = commit_head(
        &mut h,
        "robot",
        log,
        8,
        sion_core::merkle::root(&memory_leaves("fork", 8)),
        Vec::new(),
    );
    let out = h.pulse(vec![left.clone(), right.clone()]);
    let outcomes = [
        outcome_of(&status_of(&out, &left)),
        outcome_of(&status_of(&out, &right)),
    ];
    assert!(
        outcomes.contains(&Outcome::Rejected),
        "one of two forks must be rejected: {outcomes:?}"
    );
    h.chain.wallets.get_mut("robot").unwrap().settle();
    let size_now = memory_log(&h, &log).size;
    if size_now == 7 {
        // The forged fork ran first and failed its proof: grow honestly.
        let again = commit_memory(&mut h, "robot", log, &leaves, 8, false);
        let out = h.pulse(vec![again.clone()]);
        expect(&out, &again, Outcome::Success);
    }
    assert_eq!(
        memory_log(&h, &log).root,
        sion_core::merkle::root(&leaves[..8])
    );

    // A commit at the same size may move the index and manifest roots.
    let reindex = commit_memory(&mut h, "robot", log, &leaves, 8, false);
    let out = h.pulse(vec![reindex.clone()]);
    expect(&out, &reindex, Outcome::Success);
    assert_eq!(memory_log(&h, &log).size, 8);

    // The empty head: an empty commit with the empty root succeeds, one
    // with any other root fails.
    let empty_wrong = commit_head(&mut h, "robot", second, 0, Hash32::ZERO, Vec::new());
    let out = h.pulse(vec![empty_wrong.clone()]);
    expect(&out, &empty_wrong, Outcome::Failed);
    let empty_ok = commit_head(
        &mut h,
        "robot",
        second,
        0,
        sion_core::merkle::empty_root(),
        Vec::new(),
    );
    let out = h.pulse(vec![empty_ok.clone()]);
    expect(&out, &empty_ok, Outcome::Success);
    assert_eq!(memory_log(&h, &second).commits, 1);

    // Sealing: the last commit seals the log, and nothing follows it.
    let seal = commit_memory(&mut h, "robot", log, &leaves, 10, true);
    let out = h.pulse(vec![seal.clone()]);
    expect(&out, &seal, Outcome::Success);
    assert!(memory_log(&h, &log).sealed);
    let after_seal = commit_memory(&mut h, "robot", log, &leaves, 12, false);
    let out = h.pulse(vec![after_seal.clone()]);
    expect(&out, &after_seal, Outcome::Failed);

    // An agent's log: only the agent opens it, only for an identity, and
    // it stops growing while the agent is suspended or revoked.
    let register = h.tx(
        "principal",
        TxKind::RegisterAgent {
            agent,
            card: card_commitment(b"warehouse robot"),
            caveats: charter_caveats(vendor),
            funding: units(10),
            agent_bond: Amount::ZERO,
        },
    );
    let plain = h.tx(
        "principal",
        TxKind::CreateCharter {
            agent,
            caveats: charter_caveats(vendor),
            funding: units(10),
            agent_bond: Amount::ZERO,
        },
    );
    let out = h.pulse(vec![register.clone(), plain.clone()]);
    expect(&out, &register, Outcome::Success);
    expect(&out, &plain, Outcome::Success);
    let (identity, _) = agent_registered(&out, &register);
    let (no_identity, _) = charter_created(&out, &plain);
    let open_agent = h.tx(
        "agent",
        TxKind::OpenMemory {
            agent: Some(identity),
            policy: Hash32::ZERO,
        },
    );
    let open_for_other = h.tx(
        "stranger",
        TxKind::OpenMemory {
            agent: Some(identity),
            policy: Hash32::ZERO,
        },
    );
    let open_plain_charter = h.tx(
        "agent",
        TxKind::OpenMemory {
            agent: Some(no_identity),
            policy: Hash32::ZERO,
        },
    );
    let open_not_charter = h.tx(
        "agent",
        TxKind::OpenMemory {
            agent: Some(log),
            policy: Hash32::ZERO,
        },
    );
    let out = h.pulse(vec![
        open_agent.clone(),
        open_for_other.clone(),
        open_plain_charter.clone(),
        open_not_charter.clone(),
    ]);
    expect(&out, &open_agent, Outcome::Success);
    expect(&out, &open_for_other, Outcome::Failed);
    expect(&out, &open_plain_charter, Outcome::Failed);
    expect(&out, &open_not_charter, Outcome::Failed);
    let agent_log = memory_opened(&out, &open_agent);
    assert_eq!(memory_log(&h, &agent_log).agent, Some(identity));
    let agent_leaves = memory_leaves("agent", 6);
    let first = commit_memory(&mut h, "agent", agent_log, &agent_leaves, 2, false);
    let out = h.pulse(vec![first.clone()]);
    expect(&out, &first, Outcome::Success);

    let suspend = h.tx("principal", TxKind::SuspendAgent { charter: identity });
    let out = h.pulse(vec![suspend.clone()]);
    expect(&out, &suspend, Outcome::Success);
    let while_suspended = commit_memory(&mut h, "agent", agent_log, &agent_leaves, 4, false);
    let out = h.pulse(vec![while_suspended.clone()]);
    expect(&out, &while_suspended, Outcome::Failed);
    let resume = h.tx("principal", TxKind::ResumeAgent { charter: identity });
    let out = h.pulse(vec![resume.clone()]);
    expect(&out, &resume, Outcome::Success);
    let resumed = commit_memory(&mut h, "agent", agent_log, &agent_leaves, 4, false);
    let out = h.pulse(vec![resumed.clone()]);
    expect(&out, &resumed, Outcome::Success);

    let revoke = h.tx("principal", TxKind::RevokeCharter { charter: identity });
    let out = h.pulse(vec![revoke.clone()]);
    expect(&out, &revoke, Outcome::Success);
    let after_revoke = commit_memory(&mut h, "agent", agent_log, &agent_leaves, 6, false);
    let reopen = h.tx(
        "agent",
        TxKind::OpenMemory {
            agent: Some(identity),
            policy: Hash32::ZERO,
        },
    );
    let out = h.pulse(vec![after_revoke.clone(), reopen.clone()]);
    expect(&out, &after_revoke, Outcome::Failed);
    expect(&out, &reopen, Outcome::Failed);
    assert_eq!(memory_log(&h, &agent_log).size, 4);

    h.assert_covered(&[
        "OpenMemory/ok",
        "OpenMemory(agent)/ok",
        "OpenMemory(agent)/failed",
        "CommitMemory/ok",
        "CommitMemory/failed",
        "CommitMemory/rejected",
    ]);
    h.assert_minimal_witnesses();
    h
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 12,
        ..ProptestConfig::default()
    })]

    /// Random commits by two writers, each from its own leaves or the other
    /// writer's, to random sizes: both stacks agree on every chunk, and a
    /// commit succeeds exactly when it extends what the log already holds.
    #[test]
    fn random_memory_commits_are_verified_and_only_grow(
        chunk_size in 1usize..4,
        steps in proptest::collection::vec((0usize..2, 0u64..24, any::<bool>()), 1..10),
    ) {
        let mut h = Harness::new(&["writer-0", "writer-1"], 1_000, 2, chunk_size);
        let leaves = [memory_leaves("writer-0", 24), memory_leaves("writer-1", 24)];
        let opens: Vec<SignedTransaction> = (0..2)
            .map(|w| {
                h.tx(
                    &format!("writer-{w}"),
                    TxKind::OpenMemory { agent: None, policy: Hash32::ZERO },
                )
            })
            .collect();
        let out = h.pulse(opens.clone());
        let logs: Vec<ObjectId> = opens.iter().map(|tx| memory_opened(&out, tx)).collect();
        // What each log holds, as the model sees it.
        let mut held: [Vec<Hash32>; 2] = [Vec::new(), Vec::new()];
        for (writer, size, own) in steps {
            let who = format!("writer-{writer}");
            let log = logs[writer];
            let current = held[writer].len();
            let source = &leaves[if own { writer } else { 1 - writer }][..size as usize];
            let proof = if size as usize >= current {
                sion_core::merkle::consistency_proof(source, current).unwrap()
            } else {
                Vec::new()
            };
            let tx = commit_head(
                &mut h,
                &who,
                log,
                size,
                sion_core::merkle::root(source),
                proof,
            );
            let extends = size as usize >= current && source[..current] == held[writer][..];
            let out = h.pulse(vec![tx.clone()]);
            let outcome = outcome_of(&status_of(&out, &tx));
            prop_assert_eq!(
                outcome,
                if extends { Outcome::Success } else { Outcome::Failed },
                "{} from {} to {} ({})",
                who,
                current,
                size,
                if own { "own leaves" } else { "the other writer's" }
            );
            if extends {
                held[writer] = source.to_vec();
            }
            let state = memory_log(&h, &log);
            prop_assert_eq!(state.size, held[writer].len() as u64);
            prop_assert_eq!(state.root, sion_core::merkle::root(&held[writer]));
        }
        prop_assert!(h.unread.is_empty(), "{:?}", h.unread);
    }
}

#[test]
fn memory_logs_open_grow_seal_and_follow_their_agent() {
    memory(1).summary("memory");
}

#[test]
fn memory_logs_many_transactions_per_chunk() {
    memory(4).summary("memory (chunks of 4)");
}

// ---------------------------------------------------------------------------
// Scenario: Task Charters (spec v1.10)
// ---------------------------------------------------------------------------

fn task_posted(out: &PulseOutput, tx: &SignedTransaction) -> ObjectId {
    out.effects
        .iter()
        .find(|e| e.digest == tx.digest())
        .and_then(|e| {
            e.events.iter().find_map(|event| match event {
                Event::TaskPosted { task, .. } => Some(*task),
                _ => None,
            })
        })
        .expect("task posted")
}

fn task_state(h: &Harness, id: &ObjectId) -> sion_exec::task::Task {
    match &h.chain.state.objects[id].data {
        ObjectData::Task(task) => (**task).clone(),
        other => panic!("{id} holds {other:?}"),
    }
}

fn acceptance(
    requires: &[ClaimId],
    verifiers: &[AccountId],
    adjudicator: sion_core::ids::AtomId,
) -> sion_exec::task::Acceptance {
    sion_exec::task::Acceptance {
        policy: Policy::standard(),
        requires: requires.iter().copied().collect(),
        verifiers: verifiers.iter().copied().collect(),
        adjudicator: Some(adjudicator),
        detection_permille: 900,
        reliance: 100,
    }
}

/// A result or attestation body committing to `task` and `output`.
fn task_claim(
    schema: &str,
    task: ObjectId,
    output: &[u8],
    formula: Formula,
    adjudicator: sion_core::ids::AtomId,
    detection_permille: u16,
) -> ClaimBody {
    ClaimBody {
        schema: schema.into(),
        subject: format!("work for task {}", task.0.short()),
        payload: sion_exec::task::TaskResult {
            task,
            output: hash_domain("test/output", output),
        }
        .payload(),
        formula,
        adjudicator: Some(adjudicator),
        detection_permille,
        valid_for: 500,
        challenge_period: 200,
    }
}

#[allow(clippy::too_many_arguments)]
fn post_task(
    h: &mut Harness,
    who: &str,
    acceptance: sion_exec::task::Acceptance,
    worker: Option<AccountId>,
    amount: Amount,
    deadline: Pulse,
    charter: Option<(ObjectId, ObjectId)>,
) -> SignedTransaction {
    let kind = TxKind::PostTask {
        spec: hash_domain("test/spec", who.as_bytes()),
        acceptance,
        worker,
        amount,
        deadline,
        charter: charter.map(|(c, _)| c),
    };
    match charter {
        Some((_, allotment)) => {
            let input = h.reference(&allotment);
            h.tx_with(who, kind, vec![input])
        }
        None => h.tx(who, kind),
    }
}

fn settle_task(
    h: &mut Harness,
    who: &str,
    task: ObjectId,
    result: &ClaimBody,
    attestations: &[(&str, &ClaimBody)],
) -> SignedTransaction {
    let attestations = attestations
        .iter()
        .map(|(name, body)| (h.id(name), (*body).clone()))
        .collect();
    h.tx(
        who,
        TxKind::SettleTask {
            task,
            result: result.clone(),
            attestations,
        },
    )
}

const TASK_NAMES: [&str; 11] = [
    "lab",
    "orchestrator",
    "narrow",
    "vendor",
    "worker",
    "hunter",
    "verifier-1",
    "verifier-2",
    "judge",
    "challenger",
    "stranger",
];

fn tasks(chunk_size: usize) -> Harness {
    use sion_exec::task::{result_formula, worker_bond, ATTESTATION_SCHEMA, RESULT_SCHEMA};

    let mut h = Harness::new(&TASK_NAMES, 100_000, 8, chunk_size);
    let id = |h: &Harness, name: &str| h.id(name);
    let (orchestrator, narrow, worker, hunter) = (
        id(&h, "orchestrator"),
        id(&h, "narrow"),
        id(&h, "worker"),
        id(&h, "hunter"),
    );
    let (v1, v2) = (id(&h, "verifier-1"), id(&h, "verifier-2"));
    let env_atom = bond_atom(AtomClass::Economic, &id(&h, "vendor"), "environment");
    let judge_atom = bond_atom(AtomClass::Adjudicator, &id(&h, "judge"), "arbitration");
    let verifier_atom = |who: &AccountId| bond_atom(AtomClass::Economic, who, "verifier");

    // Bonds, and two agents: an orchestrator that may pay anyone, and one
    // limited to paying the worker.
    let bond = |h: &mut Harness, who: &str, class, label: &str, amount| {
        h.tx(
            who,
            TxKind::PostBond {
                class,
                label: label.into(),
                amount: units(amount),
            },
        )
    };
    let mut wide = charter_caveats(worker);
    wide.payees = None;
    wide.max_per_payment = units(500);
    let register = |h: &mut Harness, agent: AccountId, caveats: Caveats| {
        h.tx(
            "lab",
            TxKind::RegisterAgent {
                agent,
                card: card_commitment(b"agent"),
                caveats,
                funding: units(1_000),
                agent_bond: Amount::ZERO,
            },
        )
    };
    let txs = vec![
        bond(&mut h, "vendor", AtomClass::Economic, "environment", 5_000),
        bond(
            &mut h,
            "judge",
            AtomClass::Adjudicator,
            "arbitration",
            3_000,
        ),
        bond(&mut h, "worker", AtomClass::Economic, "task-worker", 100),
        // The bounty hunter's bond is nominal: its result rests on an
        // attestation.
        h.tx(
            "hunter",
            TxKind::PostBond {
                class: AtomClass::Economic,
                label: "task-worker".into(),
                amount: Amount(1),
            },
        ),
        bond(&mut h, "verifier-1", AtomClass::Economic, "verifier", 1_000),
        bond(&mut h, "verifier-2", AtomClass::Economic, "verifier", 1_000),
    ];
    let reg_orchestrator = register(&mut h, orchestrator, wide);
    let reg_narrow = register(&mut h, narrow, charter_caveats(worker));
    let mut all = txs;
    all.push(reg_orchestrator.clone());
    all.push(reg_narrow.clone());
    let out = h.pulse(all);
    let orchestrator_agent = agent_registered(&out, &reg_orchestrator);
    let narrow_agent = agent_registered(&out, &reg_narrow);

    // The environment vendor's bonded claim that its environment has no
    // known exploit passing its grader.
    let environment = ClaimBody {
        schema: "environment.no-known-exploit/v1".into(),
        subject: "environment sandbox-7 has no known exploit passing grader g-3".into(),
        payload: Payload::Commitment(hash_domain("test/environment", b"sandbox-7")),
        formula: formulas::bonded_statement(env_atom.id),
        adjudicator: Some(judge_atom.id),
        detection_permille: 1000,
        valid_for: 500,
        challenge_period: 200,
    };
    let env_claim = claim_id(&id(&h, "vendor"), &environment);
    let tx = h.tx("vendor", TxKind::IssueClaim { claim: environment });
    h.pulse(vec![tx]);

    let terms = |verifiers: &[AccountId]| acceptance(&[env_claim], verifiers, judge_atom.id);

    // Posting: a direct task from the orchestrator's budget, and failures
    // that change nothing but the fee.
    let base = h.pulse_number();
    let t1_tx = post_task(
        &mut h,
        "orchestrator",
        terms(&[v1, v2]),
        Some(worker),
        units(150),
        Pulse(base + 60),
        Some(orchestrator_agent),
    );
    let zero = post_task(
        &mut h,
        "lab",
        terms(&[]),
        Some(worker),
        Amount::ZERO,
        Pulse(base + 60),
        None,
    );
    let past = post_task(
        &mut h,
        "lab",
        terms(&[]),
        Some(worker),
        units(1),
        Pulse(0),
        None,
    );
    let mut reckless = terms(&[]);
    reckless.reliance = 0;
    let no_reliance = post_task(
        &mut h,
        "lab",
        reckless,
        Some(worker),
        units(1),
        Pulse(base + 9),
        None,
    );
    let narrow_open = post_task(
        &mut h,
        "narrow",
        terms(&[]),
        None,
        units(10),
        Pulse(base + 9),
        Some(narrow_agent),
    );
    let out = h.pulse(vec![
        t1_tx.clone(),
        zero.clone(),
        past.clone(),
        no_reliance.clone(),
        narrow_open.clone(),
    ]);
    expect(&out, &t1_tx, Outcome::Success);
    expect(&out, &zero, Outcome::Failed);
    expect(&out, &past, Outcome::Failed);
    expect(&out, &no_reliance, Outcome::Failed);
    expect(&out, &narrow_open, Outcome::Failed);
    let t1 = task_posted(&out, &t1_tx);
    assert_eq!(task_state(&h, &t1).refund_to, id(&h, "lab"));
    assert_eq!(task_state(&h, &t1).funder, Some(orchestrator_agent.0));

    // An open bounty from the budget; one above the per-payment limit; one
    // from an allotment that is not the poster's charter's.
    let base = h.pulse_number();
    let t2_tx = post_task(
        &mut h,
        "orchestrator",
        terms(&[v2]),
        None,
        units(40),
        Pulse(base + 60),
        Some(orchestrator_agent),
    );
    let mismatched = post_task(
        &mut h,
        "narrow",
        terms(&[]),
        Some(worker),
        units(5),
        Pulse(base + 9),
        Some((orchestrator_agent.0, narrow_agent.1)),
    );
    let out = h.pulse(vec![t2_tx.clone(), mismatched.clone()]);
    expect(&out, &t2_tx, Outcome::Success);
    expect(&out, &mismatched, Outcome::Failed);
    let t2 = task_posted(&out, &t2_tx);
    let base = h.pulse_number();
    let over_limit = post_task(
        &mut h,
        "orchestrator",
        terms(&[]),
        Some(worker),
        units(600),
        Pulse(base + 9),
        Some(orchestrator_agent),
    );
    let out = h.pulse(vec![over_limit.clone()]);
    expect(&out, &over_limit, Outcome::Failed);

    // Attestations of the outputs, then results.
    let attest = |task, output: &[u8], who: &AccountId| {
        task_claim(
            ATTESTATION_SCHEMA,
            task,
            output,
            formulas::bonded_statement(verifier_atom(who).id),
            judge_atom.id,
            1000,
        )
    };
    let a1 = attest(t1, b"t1 output", &v1);
    let a2 = attest(t1, b"t1 output", &v2);
    let a3 = attest(t2, b"t2 output", &v2);
    let unnamed = attest(t2, b"t2 output", &v1);
    let txs = vec![
        h.tx("verifier-1", TxKind::IssueClaim { claim: a1.clone() }),
        h.tx("verifier-2", TxKind::IssueClaim { claim: a2.clone() }),
        h.tx("verifier-2", TxKind::IssueClaim { claim: a3.clone() }),
        h.tx(
            "verifier-1",
            TxKind::IssueClaim {
                claim: unnamed.clone(),
            },
        ),
    ];
    h.pulse(txs);
    let (a1_id, a2_id, a3_id) = (claim_id(&v1, &a1), claim_id(&v2, &a2), claim_id(&v2, &a3));

    let requires = task_state(&h, &t1).acceptance.requires.clone();
    let result = |task, output: &[u8], formula| {
        task_claim(RESULT_SCHEMA, task, output, formula, judge_atom.id, 900)
    };
    let good = result(
        t1,
        b"t1 output",
        result_formula(&worker, &[a1_id, a2_id], &requires),
    );
    let without_environment = result(
        t1,
        b"t1 output",
        result_formula(&worker, &[a1_id, a2_id], &BTreeSet::new()),
    );
    let bond_only = result(t1, b"t1 output", result_formula(&worker, &[], &requires));
    let elsewhere = result(
        t2,
        b"t1 output",
        result_formula(&worker, &[a1_id, a2_id], &requires),
    );
    let bounty = result(
        t2,
        b"t2 output",
        result_formula(&hunter, &[a3_id], &requires),
    );
    assert!(h
        .chain
        .state
        .registry()
        .atom(&worker_bond(&hunter))
        .is_some());
    let txs = vec![
        h.tx(
            "worker",
            TxKind::IssueClaim {
                claim: good.clone(),
            },
        ),
        h.tx(
            "worker",
            TxKind::IssueClaim {
                claim: without_environment.clone(),
            },
        ),
        h.tx(
            "worker",
            TxKind::IssueClaim {
                claim: bond_only.clone(),
            },
        ),
        h.tx(
            "worker",
            TxKind::IssueClaim {
                claim: elsewhere.clone(),
            },
        ),
        h.tx(
            "hunter",
            TxKind::IssueClaim {
                claim: bounty.clone(),
            },
        ),
    ];
    h.pulse(txs);

    // Settling: every way a result can fail the rule fixed at posting.
    let wrong_worker = settle_task(
        &mut h,
        "stranger",
        t1,
        &good,
        &[("verifier-1", &a1), ("verifier-2", &a2)],
    );
    let missing_environment = settle_task(
        &mut h,
        "worker",
        t1,
        &without_environment,
        &[("verifier-1", &a1), ("verifier-2", &a2)],
    );
    let too_weak = settle_task(&mut h, "worker", t1, &bond_only, &[]);
    let other_task = settle_task(
        &mut h,
        "worker",
        t1,
        &elsewhere,
        &[("verifier-1", &a1), ("verifier-2", &a2)],
    );
    let unnamed_verifier = settle_task(&mut h, "hunter", t2, &bounty, &[("verifier-1", &unnamed)]);
    let missing_attestation = settle_task(&mut h, "worker", t1, &good, &[("verifier-1", &a1)]);
    let twice = settle_task(
        &mut h,
        "worker",
        t1,
        &good,
        &[("verifier-1", &a1), ("verifier-1", &a1)],
    );
    let out = h.pulse(vec![
        wrong_worker.clone(),
        missing_environment.clone(),
        too_weak.clone(),
        other_task.clone(),
        unnamed_verifier.clone(),
        missing_attestation.clone(),
        twice.clone(),
    ]);
    for (tx, why) in [
        (&wrong_worker, "only the task's worker"),
        (
            &missing_environment,
            "formula is not the one the task requires",
        ),
        (&too_weak, "not accepted"),
        (&other_task, "does not commit to this task"),
        (&unnamed_verifier, "verifier the task does not name"),
        (
            &missing_attestation,
            "formula is not the one the task requires",
        ),
        (&twice, "one attestation per verifier"),
    ] {
        match status_of(&out, tx) {
            ExecStatus::Failed(reason) => assert!(reason.contains(why), "{reason}"),
            other => panic!("expected a failure ({why}), got {other:?}"),
        }
    }

    // The honest settlement pays the worker once; a second one fails.
    let before = h.balance("worker", &usd());
    let settle = settle_task(
        &mut h,
        "worker",
        t1,
        &good,
        &[("verifier-1", &a1), ("verifier-2", &a2)],
    );
    let out = h.pulse(vec![settle.clone()]);
    expect(&out, &settle, Outcome::Success);
    let fee = out
        .effects
        .iter()
        .find(|e| e.digest == settle.digest())
        .map(|e| e.fee)
        .unwrap();
    assert_eq!(
        h.balance("worker", &usd()),
        before
            .checked_add(units(150))
            .unwrap()
            .checked_sub(fee)
            .unwrap()
    );
    assert!(matches!(
        task_state(&h, &t1).state,
        sion_exec::task::TaskState::Settled { .. }
    ));
    let again = settle_task(
        &mut h,
        "worker",
        t1,
        &good,
        &[("verifier-1", &a1), ("verifier-2", &a2)],
    );
    let out = h.pulse(vec![again.clone()]);
    expect(&out, &again, Outcome::Failed);

    // An open bounty: anyone may settle it with an acceptable result, here
    // a hunter with a nominal bond whose output a named verifier attested.
    let hunted = settle_task(&mut h, "hunter", t2, &bounty, &[("verifier-2", &a3)]);
    let out = h.pulse(vec![hunted.clone()]);
    expect(&out, &hunted, Outcome::Success);

    // Cancelling: a named worker can count on its task until the deadline;
    // only it can release it. Open tasks can be withdrawn; expired ones
    // refunded.
    let base = h.pulse_number();
    let t3_tx = post_task(
        &mut h,
        "lab",
        terms(&[]),
        Some(worker),
        units(100),
        Pulse(base + 30),
        None,
    );
    let t4_tx = post_task(
        &mut h,
        "orchestrator",
        terms(&[]),
        None,
        units(30),
        Pulse(base + 30),
        Some(orchestrator_agent),
    );
    let out = h.pulse(vec![t3_tx.clone(), t4_tx.clone()]);
    let (t3, t4) = (task_posted(&out, &t3_tx), task_posted(&out, &t4_tx));
    let base = h.pulse_number();
    let t5_tx = post_task(
        &mut h,
        "orchestrator",
        terms(&[]),
        Some(worker),
        units(20),
        Pulse(base + 2),
        Some(orchestrator_agent),
    );
    let out = h.pulse(vec![t5_tx.clone()]);
    let t5 = task_posted(&out, &t5_tx);
    let lab_cancels = h.tx("lab", TxKind::CancelTask { task: t3 });
    let stranger_cancels = h.tx("stranger", TxKind::CancelTask { task: t4 });
    let out = h.pulse(vec![lab_cancels.clone(), stranger_cancels.clone()]);
    expect(&out, &lab_cancels, Outcome::Failed);
    expect(&out, &stranger_cancels, Outcome::Failed);
    let lab_before = h.balance("lab", &usd());
    let release = h.tx("worker", TxKind::CancelTask { task: t3 });
    let withdraw = h.tx("orchestrator", TxKind::CancelTask { task: t4 });
    let out = h.pulse(vec![release.clone(), withdraw.clone()]);
    expect(&out, &release, Outcome::Success);
    expect(&out, &withdraw, Outcome::Success);
    assert_eq!(
        h.balance("lab", &usd()),
        lab_before.checked_add(units(130)).unwrap()
    );
    h.advance(3);
    let late = settle_task(&mut h, "worker", t5, &bond_only, &[]);
    let expire = h.tx("orchestrator", TxKind::CancelTask { task: t5 });
    let settled_cancel = h.tx("orchestrator", TxKind::CancelTask { task: t1 });
    let out = h.pulse(vec![late.clone(), expire.clone(), settled_cancel.clone()]);
    expect(&out, &late, Outcome::Failed);
    expect(&out, &expire, Outcome::Success);
    expect(&out, &settled_cancel, Outcome::Failed);

    // An exploit in the environment is proven: the vendor's bond is
    // slashed, the environment claim and every result resting on it fall,
    // and the lab, which relied on the paid result, is compensated.
    let challenge = h.tx(
        "challenger",
        TxKind::Challenge {
            claim: env_claim,
            target: env_atom.id,
            evidence: hash_domain("test/exploit", b"grader g-3 accepts an empty patch"),
            deposit: units(100),
        },
    );
    let out = h.pulse(vec![challenge.clone()]);
    expect(&out, &challenge, Outcome::Success);
    let opened = all_events(&out)
        .into_iter()
        .find_map(|event| match event {
            Event::ChallengeOpened { challenge, .. } => Some(challenge),
            _ => None,
        })
        .expect("challenge opened");
    let lab_before = h.balance("lab", &usd());
    let rule = h.tx(
        "judge",
        TxKind::Rule {
            challenge: opened,
            upheld: true,
        },
    );
    let out = h.pulse(vec![rule.clone()]);
    expect(&out, &rule, Outcome::Success);
    let events = all_events(&out);
    let good_id = claim_id(&worker, &good);
    assert!(events.iter().any(|e| matches!(
        e,
        Event::Falsified { claim, .. } if *claim == good_id
    )));
    let compensated: Amount = Amount(
        events
            .iter()
            .filter_map(|e| match e {
                Event::Compensated {
                    consumer, amount, ..
                } if *consumer == id(&h, "lab") => Some(amount.0),
                _ => None,
            })
            .sum(),
    );
    assert_eq!(compensated, units(190));
    assert_eq!(
        h.balance("lab", &usd()),
        lab_before.checked_add(compensated).unwrap()
    );

    h.assert_covered(&[
        "PostTask/ok",
        "PostTask/failed",
        "PostTask(agent)/ok",
        "PostTask(agent)/failed",
        "PostTask(agent, open)/ok",
        "PostTask(agent, open)/failed",
        "SettleTask/ok",
        "SettleTask/failed",
        "CancelTask/ok",
        "CancelTask/failed",
    ]);
    h.assert_minimal_witnesses();
    h
}

#[test]
fn tasks_post_settle_once_cancel_and_compensate_an_exploit() {
    tasks(1).summary("tasks");
}

#[test]
fn tasks_many_transactions_per_chunk() {
    tasks(5).summary("tasks (chunks of 5)");
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
// Scenario (v1.6): payment streams
// ---------------------------------------------------------------------------

/// The stream a successful OpenStream created.
fn opened_stream(out: &PulseOutput, tx: &SignedTransaction) -> ObjectId {
    out.effects
        .iter()
        .find(|e| e.digest == tx.digest())
        .and_then(|e| {
            e.events.iter().find_map(|event| match event {
                Event::StreamOpened { stream, .. } => Some(*stream),
                _ => None,
            })
        })
        .expect("stream opened")
}

fn stream_state(h: &Harness, id: &ObjectId) -> sion_exec::stream::Stream {
    match &h.chain.state.objects[id].data {
        ObjectData::Stream(stream) => stream.clone(),
        other => panic!("not a stream: {other:?}"),
    }
}

fn open_stream(
    h: &mut Harness,
    payer: &str,
    payee: &str,
    chain: &sion_exec::stream::PayerChain,
    deposit: Amount,
    until: u64,
) -> SignedTransaction {
    let payee = h.id(payee);
    h.tx(
        payer,
        TxKind::OpenStream {
            payee,
            asset: usd(),
            unit: cents(1),
            length: chain.length(),
            anchor: chain.anchor(),
            deposit,
            until: Pulse(until),
            checkpoints: chain.checkpoints(),
        },
    )
}

fn settle_stream(
    h: &mut Harness,
    who: &str,
    stream: ObjectId,
    position: u64,
    value: Hash32,
) -> SignedTransaction {
    h.tx(
        who,
        TxKind::SettleStream {
            stream,
            position,
            value,
            jump: None,
        },
    )
}

fn jump_stream(
    h: &mut Harness,
    who: &str,
    stream: ObjectId,
    position: u64,
    value: Hash32,
    jump: sion_exec::stream::Jump,
) -> SignedTransaction {
    h.tx(
        who,
        TxKind::SettleStream {
            stream,
            position,
            value,
            jump: Some(jump),
        },
    )
}

fn streams(chunk_size: usize) -> Harness {
    use sion_exec::stream::{walk, PayerChain};
    let mut h = Harness::new(&["payer", "api", "outsider"], 10_000, 8, chunk_size);
    let mut chain = PayerChain::new(Hash32([0x5a; 32]), 100);
    let mut long = PayerChain::new(Hash32([0x5b; 32]), 70_000);
    let now = h.pulse_number();

    // Pulse: one stream of 100 steps of 0.01 USD with 0.50 USD in escrow,
    // one long stream, and opens that break §5.31 step 1.
    let s1 = open_stream(&mut h, "payer", "api", &chain, cents(50), now + 30);
    let s2 = open_stream(&mut h, "payer", "api", &long, units(1_000), now + 30);
    let anchor = chain.anchor();
    let api = h.id("api");
    let bad = |h: &mut Harness, unit: Amount, length: u64, deposit: Amount, until: u64| {
        h.tx(
            "outsider",
            TxKind::OpenStream {
                payee: api,
                asset: usd(),
                unit,
                length,
                anchor,
                deposit,
                until: Pulse(until),
                checkpoints: None,
            },
        )
    };
    let zero_unit = bad(&mut h, Amount::ZERO, 10, cents(1), now + 5);
    let zero_length = bad(&mut h, cents(1), 0, cents(1), now + 5);
    let past = bad(&mut h, cents(1), 10, cents(1), now + 1);
    let too_long = bad(&mut h, cents(1), 10, cents(1), now + 2 + 1_000_001);
    let out = h.pulse(vec![
        s1.clone(),
        s2.clone(),
        zero_unit.clone(),
        zero_length.clone(),
        past.clone(),
        too_long.clone(),
    ]);
    expect(&out, &s1, Outcome::Success);
    expect(&out, &s2, Outcome::Success);
    for tx in [&zero_unit, &zero_length, &past, &too_long] {
        expect(&out, tx, Outcome::Failed);
    }
    let (one, two) = (opened_stream(&out, &s1), opened_stream(&out, &s2));
    let before = h.balance("api", &usd());

    // Off the ledger the payer reveals elements 1..=10; the api checks each
    // with one hash. It settles 10 steps: 0.10 USD.
    let mut last = anchor;
    for position in 1..=10 {
        let value = chain.element(position).unwrap();
        assert_eq!(walk(&value, 1), last);
        last = value;
    }
    let w10 = chain.element(10).unwrap();
    let ok = settle_stream(&mut h, "api", one, 10, w10);
    let thief = settle_stream(&mut h, "outsider", one, 10, w10);
    let wrong = settle_stream(&mut h, "api", two, 3, w10);
    let out = h.pulse(vec![ok.clone(), thief.clone(), wrong.clone()]);
    expect(&out, &ok, Outcome::Success);
    expect(&out, &thief, Outcome::Failed);
    expect(&out, &wrong, Outcome::Failed);
    let fee = |out: &PulseOutput, tx: &SignedTransaction| {
        out.effects
            .iter()
            .find(|e| e.digest == tx.digest())
            .unwrap()
            .fee
    };
    // The api paid a fee for both of its settlements, the failed one too.
    assert_eq!(
        h.balance("api", &usd()).0,
        before.0 + cents(10).0 - fee(&out, &ok).0 - fee(&out, &wrong).0
    );
    let state = stream_state(&h, &one);
    assert_eq!(
        (state.position, state.head, state.escrow),
        (10, w10, cents(40))
    );
    // A third party checks any paid element against the settled head.
    let w4 = chain.element(4).unwrap();
    assert_eq!(walk(&state.head, 10 - 4), w4);

    // §5.32 steps 2, 3 and 5: not past the settled position, not beyond
    // the length, not more than the escrow, not more than 65,536 steps,
    // and a fee per step.
    let behind = settle_stream(&mut h, "api", one, 10, w10);
    let beyond = settle_stream(&mut h, "api", one, 101, w10);
    let w60 = chain.element(60).unwrap();
    let over_escrow = settle_stream(&mut h, "api", one, 60, w60);
    let out = h.pulse(vec![behind.clone(), beyond.clone(), over_escrow.clone()]);
    for tx in [&behind, &beyond, &over_escrow] {
        expect(&out, tx, Outcome::Failed);
    }
    let w65537 = long.element(65_537).unwrap();
    let too_many = settle_stream(&mut h, "api", two, 65_537, w65537);
    let w600 = long.element(600).unwrap();
    let cheap = by_hand(
        &mut h,
        "api",
        TxKind::SettleStream {
            stream: two,
            position: 600,
            value: w600,
            jump: None,
        },
        Vec::new(),
    );
    let out = h.pulse(vec![too_many.clone(), cheap.clone()]);
    expect(&out, &too_many, Outcome::Failed);
    expect(&out, &cheap, Outcome::Failed);
    let w65536 = long.element(65_536).unwrap();
    let most = settle_stream(&mut h, "api", two, 65_536, w65536);
    let out = h.pulse(vec![most.clone()]);
    expect(&out, &most, Outcome::Success);
    assert_eq!(stream_state(&h, &two).position, 65_536);

    // §5.33: the payer adds 1 USD and extends the stream; the payee, a
    // shorter last Pulse and a top-up that changes nothing are refused.
    let until = stream_state(&h, &one).until.0;
    let top = h.tx(
        "payer",
        TxKind::TopUpStream {
            stream: one,
            deposit: units(1),
            until: Pulse(until + 10),
        },
    );
    let by_payee = h.tx(
        "api",
        TxKind::TopUpStream {
            stream: one,
            deposit: cents(1),
            until: Pulse(until),
        },
    );
    let out = h.pulse(vec![top.clone(), by_payee.clone()]);
    expect(&out, &top, Outcome::Success);
    expect(&out, &by_payee, Outcome::Failed);
    let shorter = h.tx(
        "payer",
        TxKind::TopUpStream {
            stream: one,
            deposit: cents(1),
            until: Pulse(until),
        },
    );
    let nothing = h.tx(
        "payer",
        TxKind::TopUpStream {
            stream: one,
            deposit: Amount::ZERO,
            until: Pulse(until + 10),
        },
    );
    let out = h.pulse(vec![shorter.clone(), nothing.clone()]);
    expect(&out, &shorter, Outcome::Failed);
    expect(&out, &nothing, Outcome::Failed);
    assert_eq!(stream_state(&h, &one).escrow, cents(140));

    // Now 50 more steps fit in the escrow.
    let late = settle_stream(&mut h, "api", one, 60, w60);
    let early_close = h.tx("payer", TxKind::CloseStream { stream: one });
    let out = h.pulse(vec![late.clone(), early_close.clone()]);
    expect(&out, &late, Outcome::Success);
    expect(&out, &early_close, Outcome::Failed);

    // §5.34: the payee closes; the payer gets the rest back; a closed
    // stream refuses everything.
    let payer_before = h.balance("payer", &usd());
    let close = h.tx("api", TxKind::CloseStream { stream: one });
    let out = h.pulse(vec![close.clone()]);
    expect(&out, &close, Outcome::Success);
    assert_eq!(
        h.balance("payer", &usd()),
        payer_before.checked_add(cents(90)).unwrap()
    );
    let w70 = chain.element(70).unwrap();
    let after_close = settle_stream(&mut h, "api", one, 70, w70);
    let top_closed = h.tx(
        "payer",
        TxKind::TopUpStream {
            stream: one,
            deposit: cents(1),
            until: Pulse(until + 20),
        },
    );
    let close_again = h.tx("api", TxKind::CloseStream { stream: one });
    let out = h.pulse(vec![
        after_close.clone(),
        top_closed.clone(),
        close_again.clone(),
    ]);
    for tx in [&after_close, &top_closed, &close_again] {
        expect(&out, tx, Outcome::Failed);
    }

    // The long stream expires: the payee can no longer settle, an outsider
    // cannot close it, and the payer takes back the rest.
    let end = stream_state(&h, &two).until.0;
    h.advance_to(end + 1);
    let w70000 = long.element(70_000).unwrap();
    let expired = settle_stream(&mut h, "api", two, 70_000, w70000);
    let stranger = h.tx("outsider", TxKind::CloseStream { stream: two });
    let reclaim = h.tx("payer", TxKind::CloseStream { stream: two });
    let out = h.pulse(vec![expired.clone(), stranger.clone(), reclaim.clone()]);
    expect(&out, &expired, Outcome::Failed);
    expect(&out, &stranger, Outcome::Failed);
    expect(&out, &reclaim, Outcome::Success);
    assert_eq!(stream_state(&h, &two).escrow, Amount::ZERO);
    assert!(!stream_state(&h, &two).open);

    h.assert_covered(&[
        "OpenStream/ok",
        "OpenStream/failed",
        "SettleStream/ok",
        "SettleStream/failed",
        "TopUpStream/ok",
        "TopUpStream/failed",
        "CloseStream/ok",
        "CloseStream/failed",
    ]);
    h.assert_minimal_witnesses();
    h
}

#[test]
fn streams_open_settle_top_up_and_close() {
    streams(2).summary("streams");
}

#[test]
fn streams_one_transaction_per_chunk() {
    streams(1).summary("streams (1 per chunk)");
}

// ---------------------------------------------------------------------------
// Scenario: stream checkpoints (spec v1.7)
// ---------------------------------------------------------------------------

/// §5.31 and §5.32 with checkpoints: opening with a commitment to every
/// `spacing`-th element, settling by a jump to a checkpoint (any number of
/// steps for a bounded number of hashes), and every way a jump can be wrong.
fn stream_checkpoints(chunk_size: usize) -> Harness {
    use sion_exec::stream::{walk, Checkpoints, Jump, PayerChain, MAX_CHECKPOINTS};
    let mut h = Harness::new(&["payer", "api", "outsider"], 10_000, 8, chunk_size);
    let mut chain = PayerChain::with_checkpoints(Hash32([0x6a; 32]), 10_000, 64);
    let mut plain = PayerChain::new(Hash32([0x6b; 32]), 1_000);
    // Longer than one walked settlement may cover: 200,000 steps.
    let mut long = PayerChain::with_checkpoints(Hash32([0x6c; 32]), 200_000, 1_024);
    let now = h.pulse_number();
    let api = h.id("api");
    let s1 = open_stream(&mut h, "payer", "api", &chain, units(90), now + 40);
    let s2 = open_stream(&mut h, "payer", "api", &plain, units(10), now + 40);
    let s3 = open_stream(&mut h, "payer", "api", &long, units(2_100), now + 40);
    // §5.31 step 1: a spacing of 0, longer than the chain, or more than
    // 2^20 checkpoints.
    let anchor = chain.anchor();
    let bad = |h: &mut Harness, length: u64, spacing: u64| {
        h.tx(
            "outsider",
            TxKind::OpenStream {
                payee: api,
                asset: usd(),
                unit: cents(1),
                length,
                anchor,
                deposit: cents(1),
                until: Pulse(now + 5),
                checkpoints: Some(Checkpoints {
                    spacing,
                    root: Hash32([1; 32]),
                }),
            },
        )
    };
    let no_spacing = bad(&mut h, 100, 0);
    let wide = bad(&mut h, 100, 101);
    let crowded = bad(&mut h, MAX_CHECKPOINTS * 2, 2);
    let most = bad(&mut h, MAX_CHECKPOINTS * 2 - 1, 2);
    let out = h.pulse(vec![
        s1.clone(),
        s2.clone(),
        s3.clone(),
        no_spacing.clone(),
        wide.clone(),
        crowded.clone(),
        most.clone(),
    ]);
    for tx in [&s1, &s2, &s3, &most] {
        expect(&out, tx, Outcome::Success);
    }
    for tx in [&no_spacing, &wide, &crowded] {
        expect(&out, tx, Outcome::Failed);
    }
    let (one, two, three) = (
        opened_stream(&out, &s1),
        opened_stream(&out, &s2),
        opened_stream(&out, &s3),
    );
    let state = stream_state(&h, &one);
    assert_eq!(state.spacing, 64);
    assert_eq!(state.checkpoints, chain.checkpoints().unwrap().root);
    assert_eq!(state.checkpoint_count(), 157);

    // A jump to position 5,000: checkpoint 78 (5,000 = 78 × 64 + 8).
    let w5000 = chain.element(5_000).unwrap();
    let j78 = chain.jump(78).unwrap();
    let ok = jump_stream(&mut h, "api", one, 5_000, w5000, j78.clone());
    let fee_paid = ok.tx.fee;
    // The wrong checkpoint (more than one spacing below), a checkpoint
    // above the position, a bent path, a path that is too long, and a
    // stream without checkpoints.
    let low = jump_stream(&mut h, "api", one, 5_000, w5000, chain.jump(77).unwrap());
    let high = jump_stream(&mut h, "api", one, 5_000, w5000, chain.jump(79).unwrap());
    let mut bent = j78.clone();
    bent.path[1] = walk(&bent.path[1], 1);
    let bent = jump_stream(&mut h, "api", one, 5_000, w5000, bent);
    let mut padded = j78.clone();
    padded
        .path
        .extend(std::iter::repeat(Hash32([2; 32])).take(21 - padded.path.len()));
    let padded = jump_stream(&mut h, "api", one, 5_000, w5000, padded);
    let w500 = plain.element(500).unwrap();
    let no_tree = jump_stream(
        &mut h,
        "api",
        two,
        500,
        w500,
        Jump {
            index: 7,
            path: Vec::new(),
        },
    );
    // Consensus orders the failures first, then the good jump.
    let out = h.pulse(vec![
        low.clone(),
        high.clone(),
        bent.clone(),
        padded.clone(),
        no_tree.clone(),
        ok.clone(),
    ]);
    for tx in [&low, &high, &bent, &padded, &no_tree] {
        expect(&out, tx, Outcome::Failed);
    }
    expect(&out, &ok, Outcome::Success);
    let state = stream_state(&h, &one);
    assert_eq!((state.position, state.head), (5_000, w5000));
    assert_eq!(state.escrow, units(40));
    // The jump cost 8 steps, the leaf and the path in hashes, not 5,000.
    let hashes = 8 + 1 + j78.path.len() as u128;
    let floor = h.chain.state.config.min_fee(ok.tx.size()).0 + 2 * hashes;
    assert_eq!(fee_paid.0, floor);

    // §5.32 step 4: a jump pays for its hashes. One fee short fails.
    let w5100 = chain.element(5_100).unwrap();
    let j79 = chain.jump(79).unwrap();
    let mut short = jump_stream(&mut h, "api", one, 5_100, w5100, j79.clone()).tx;
    short.fee = Amount(short.fee.0 - 1);
    let short = h.resign("api", short);
    let out = h.pulse(vec![short.clone()]);
    expect(&out, &short, Outcome::Failed);

    // A walk still works after a jump, and a jump may start below the
    // settled position: walk 30 steps to 5,030, then jump to 5,100 from
    // checkpoint 79 (5,056), which lies between them.
    let w5030 = chain.element(5_030).unwrap();
    let walked = settle_stream(&mut h, "api", one, 5_030, w5030);
    let out = h.pulse(vec![walked.clone()]);
    expect(&out, &walked, Outcome::Success);
    let w5060 = chain.element(5_060).unwrap();
    let j79b = chain.jump(79).unwrap();
    let across = jump_stream(&mut h, "api", one, 5_060, w5060, j79b);
    let out = h.pulse(vec![across.clone()]);
    expect(&out, &across, Outcome::Success);
    // And one whose checkpoint lies below the settled position: from
    // 5,060, jump to 5,100 by checkpoint 79 again.
    let again = jump_stream(&mut h, "api", one, 5_100, w5100, j79);
    let out = h.pulse(vec![again.clone()]);
    expect(&out, &again, Outcome::Success);

    // The escrow still bounds a jump: the 39 USD left cover 3,900 steps,
    // not the 4,900 up to 10,000.
    let w10000 = chain.element(10_000).unwrap();
    let j156 = chain.jump(156).unwrap();
    let too_much = jump_stream(&mut h, "api", one, 10_000, w10000, j156.clone());
    let out = h.pulse(vec![too_much.clone()]);
    expect(&out, &too_much, Outcome::Failed);

    // The long stream: 199,999 steps in one settlement, beyond what a walk
    // may cover (65,536 steps), for about 1,040 hashes.
    let w199999 = long.element(199_999).unwrap();
    let walk_far = settle_stream(&mut h, "api", three, 199_999, w199999);
    let jump_far = jump_stream(
        &mut h,
        "api",
        three,
        199_999,
        w199999,
        long.jump(195).unwrap(),
    );
    let out = h.pulse(vec![walk_far.clone(), jump_far.clone()]);
    expect(&out, &walk_far, Outcome::Failed);
    expect(&out, &jump_far, Outcome::Success);
    let state = stream_state(&h, &three);
    assert_eq!(state.position, 199_999);
    assert_eq!(
        state.escrow,
        units(2_100).checked_sub(Amount(199_999 * 10_000)).unwrap()
    );
    // The last step, by a walk of one hash.
    let top = long.element(200_000).unwrap();
    let last = settle_stream(&mut h, "api", three, 200_000, top);
    let out = h.pulse(vec![last.clone()]);
    expect(&out, &last, Outcome::Success);
    assert_eq!(stream_state(&h, &three).position, 200_000);

    h.assert_covered(&[
        "OpenStream(checkpoints)/ok",
        "OpenStream(checkpoints)/failed",
        "OpenStream/ok",
        "SettleStream(jump)/ok",
        "SettleStream(jump)/failed",
        "SettleStream/ok",
        "SettleStream/failed",
    ]);
    h.assert_minimal_witnesses();
    h
}

#[test]
fn stream_checkpoints_settle_by_jump() {
    stream_checkpoints(2).summary("stream checkpoints");
}

#[test]
fn stream_checkpoints_one_transaction_per_chunk() {
    stream_checkpoints(1).summary("stream checkpoints (1 per chunk)");
}

// ---------------------------------------------------------------------------
// Scenario: markets
// ---------------------------------------------------------------------------

fn create_market(
    h: &mut Harness,
    committee: Vec<sion_crypto::KemPublicKey>,
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
fn sealed_batch(h: &Harness, market: &ObjectId) -> (u64, sion_clearing::Batch) {
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
            pending.terms.map(|terms| sion_clearing::Order {
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
        sion_clearing::Batch {
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
    let result = sion_clearing::clear(&batch).expect("clears");
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
    let aad = sion_exec::market::sealing_aad(&state.domain, index);
    let mut rng = sion_core::rng::seeded(7);
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
        &sion_core::encode(&terms),
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
        &sion_core::encode(&terms),
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
    let early = sion_clearing::clear(&batch).expect("clears");
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
    let result = sion_clearing::clear(&batch).expect("clears");
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
    use sion_exec::pulse::ChunkStatement;
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
        sion_spec::commit::steps_digest(&[]),
        "stack A binds the empty step list"
    );
    // Claiming the Pulse's transactions as the phase's steps ...
    let all: Vec<SignedTransaction> = traces
        .iter()
        .flat_map(|trace| trace.transactions.iter().cloned())
        .collect();
    let mut claimed = phase.clone();
    claimed.statement.steps = sion_spec::commit::steps_digest(&all);
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
        trace.statement.steps = sion_spec::commit::steps_digest(&trace.transactions);
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
    let fee = phase
        .deferred_items
        .iter()
        .position(|item| matches!(item, Deferred::Fee { .. }))
        .expect("a fee");
    let mut altered = phase.clone();
    if let Deferred::Fee { amount, .. } = &mut altered.deferred_items[fee] {
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

    // v1.4: an injected fee, or a fee relabelled as a treasury credit (the
    // same balance, but not a fee validators are paid from), changes the
    // treasury.
    let mut injected = phase.clone();
    injected.deferred_items.push(Deferred::Fee {
        asset: usd(),
        amount: units(1),
    });
    refit(&mut injected);
    assert!(matches!(
        rejected(&injected, "an injected fee"),
        SpecError::StateRootMismatch { .. }
    ));
    let mut relabelled = phase.clone();
    if let Deferred::Fee { asset, amount } = relabelled.deferred_items[fee].clone() {
        relabelled.deferred_items[fee] = Deferred::Treasury { asset, amount };
    }
    refit(&mut relabelled);
    assert!(matches!(
        rejected(&relabelled, "a fee relabelled as a treasury credit"),
        SpecError::StateRootMismatch { .. }
    ));

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
    // v1.3: the deferred phase's transactions are its posted forced
    // intents, bound by its steps.
    let mut with_transactions = phase.clone();
    with_transactions.transactions = chunk.transactions.clone();
    assert_eq!(
        rejected(&with_transactions, "unbound intents in the deferred phase"),
        SpecError::StepsMismatch
    );

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
            let faucet = h.chain.wallets.get_mut(sion_exec::sim::FAUCET)?;
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
        // Each level keeps 5 USD of its window limit (carved like budgets).
        let mut narrower = caveats.clone();
        narrower.max_per_window = Amount(caveats.max_per_window.0 - units(5 * depth).0);
        let delegate = h.tx_with(
            "agent",
            TxKind::Delegate {
                charter,
                sub_agent: agent,
                caveats: narrower,
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
    let limit = sion_exec::charter::MAX_CHARTER_CHAIN;
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
    use sion_exec::market::MAX_PERIOD;
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
    let result = sion_clearing::clear(&batch).expect("clears");
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
    use sion_exec::tx::MAX_GRANT;
    let mut h = Harness::new(&["alice", "bob"], 1_000, 4, 1);
    let (alice, bob) = (h.id("alice"), h.id("bob"));
    let faucet = |h: &mut Harness, grants: Vec<(AccountId, AssetId, Amount)>| {
        let wallet = h
            .chain
            .wallets
            .get_mut(sion_exec::sim::FAUCET)
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
    let paying = h.resign(sion_exec::sim::FAUCET, paying);
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
    use sion_exec::tx::MAX_GRANT;
    let mut h = Harness::new(&["alice"], 1_000, 2, 1);
    let alice = h.id("alice");
    let half = Amount(u128::MAX / 2 + 1);
    let wallet = h
        .chain
        .wallets
        .get_mut(sion_exec::sim::FAUCET)
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
        .get_mut(sion_exec::sim::FAUCET)
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
                policy: sion_exec::Wallet::named("bob-2").policy,
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
    let aad = sion_exec::market::sealing_aad(&state.domain, index);
    let mut rng = sion_core::rng::seeded(11);
    let seal = |terms: &OrderTerms, threshold: u8, rng: &mut _| {
        CommitteeEnvelope::seal(
            &keys,
            threshold,
            SEALED_ORDER_DOMAIN,
            &aad,
            &sion_core::encode(terms),
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
        sion_core::hash::hash_parts("sion/market/order", &[&submitted.digest().0 .0])
    };
    assert!(state.batches[&index]
        .orders
        .iter()
        .any(|o| o.id == order_id(&s_kept)));
    let shares = |envelope: &CommitteeEnvelope| -> Vec<sion_crypto::KeyShare> {
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
    let result = sion_clearing::clear(&batch).expect("clears");
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
                .map(|i| Formula::Atom(sion_core::ids::AtomId::derive(&i.to_be_bytes())))
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
    // Window limits are carved out of the parent's (v1.8), so the probes
    // that should pass ask for none of it; "identical" takes all of it last.
    let narrower = |change: fn(&mut Caveats)| {
        let mut caveats = parent.clone();
        caveats.max_per_window = Amount::ZERO;
        change(&mut caveats);
        caveats
    };
    let delegations: Vec<(&str, Caveats, Outcome)> = vec![
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
        ("identical", parent.clone(), Outcome::Success),
        (
            "window allowance exhausted",
            narrower(|c| c.max_per_window = Amount(1)),
            Outcome::Failed,
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
    orphan.adjudicator = Some(sion_core::ids::AtomId::derive(b"no court"));
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
    let result = sion_clearing::clear(&batch).expect("clears");
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
    let fresh = sion_exec::Wallet::named("gina-rotated");
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
/// (`crates/sion-exec/tests/scenarios.rs`), replayed through the harness so
/// stack B verifies every chunk it produces and every witness is minimal.
#[test]
fn stack_a_bounds_scenario_verifies() {
    use sion_exec::charter::MAX_CHARTER_CHAIN;
    use sion_exec::market::MAX_PERIOD;
    use sion_exec::sim::FAUCET;
    use sion_exec::tx::MAX_GRANT;

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
        // Each level keeps 5 USD of its window limit (carved like budgets).
        let mut narrower = caveats.clone();
        narrower.max_per_window = Amount(caveats.max_per_window.0 - units(5 * level).0);
        let input = h.reference(&allotment);
        let delegate = h.tx_with(
            "agent",
            TxKind::Delegate {
                charter,
                sub_agent: agent,
                caveats: narrower,
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
            .authorize(sion_exec::tx::TX_DOMAIN, &body.digest().0 .0)
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
    use sion_exec::tx::MAX_INPUTS;
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
    use sion_exec::escrow::MAX_ALLOTMENTS;
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
    use sion_exec::market::MAX_BATCH_ORDERS;
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
        .map(|tx| sion_core::hash::hash_parts("sion/market/order", &[&tx.digest().0 .0]))
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

// ---------------------------------------------------------------------------
// Spec v1.3: validators, stake, epochs and forced inclusion — helpers
// ---------------------------------------------------------------------------

/// Validator parameters for the v1.3 tests: epochs of `epoch_length`
/// Pulses and two seats per shard, so that with the four genesis
/// validators a rotation replaces a shard's whole committee.
fn validator_params(epoch_length: u64) -> ValidatorParams {
    ValidatorParams {
        stake_asset: usd(),
        epoch_length,
        min_stake: units(1_000),
        unbonding: 3,
        seats: 2,
        max_active: 10,
        reward_permille: 500,
        reporter_permille: 500,
    }
}

/// The faucet's wallet, which also operates the genesis validators.
const FAUCET_NAME: &str = sion_exec::sim::FAUCET;

fn forced_params(window: u64, max_pending: u32) -> ForcedParams {
    ForcedParams {
        window,
        max_pending,
    }
}

/// The identifier of the validator holding `keys`.
fn key_id(keys: &DualKeypair) -> ValidatorId {
    validator_id(keys.lattice.public(), keys.hash.public())
}

/// Deterministic validator keys for the validators the tests register (key
/// generation and signing are slow, so they are made once).
fn pool_keys(index: usize) -> &'static DualKeypair {
    static POOL: OnceLock<Vec<DualKeypair>> = OnceLock::new();
    &POOL.get_or_init(|| {
        (0..8u8)
            .map(|i| DualKeypair::from_seed(&[0xb0 + i; 32], Scheme::SlhDsaSha2_128f))
            .collect()
    })[index]
}

/// A RegisterValidator kind for `keys`, with a proof of possession that
/// names `operator` and this chain.
fn registration(h: &Harness, operator: &str, keys: &DualKeypair, stake: Amount) -> TxKind {
    let (lattice, hash) = (keys.lattice.public().clone(), keys.hash.public().clone());
    let id = validator_id(&lattice, &hash);
    let message = possession_digest(&h.chain.state.config, &h.id(operator), &id);
    let possession = encode(
        &keys
            .sign_deterministic(VALIDATOR_DOMAIN, &message.0)
            .expect("signing"),
    );
    TxKind::RegisterValidator {
        lattice,
        hash,
        beacon_tip: hash_domain("test/beacon", &id.0 .0),
        stake,
        possession,
    }
}

/// Signs as the validator holding `keys`; with `claims`, it claims to be
/// another validator (forged evidence).
#[derive(Clone)]
struct KeyAuth {
    keys: Arc<DualKeypair>,
    claims: Option<ValidatorId>,
}

impl KeyAuth {
    fn of(keys: &DualKeypair) -> KeyAuth {
        KeyAuth {
            keys: Arc::new(keys.clone()),
            claims: None,
        }
    }

    fn claiming(keys: &DualKeypair, claims: ValidatorId) -> KeyAuth {
        KeyAuth {
            keys: Arc::new(keys.clone()),
            claims: Some(claims),
        }
    }
}

impl Authenticator for KeyAuth {
    fn validator(&self) -> ValidatorId {
        self.claims.unwrap_or_else(|| key_id(&self.keys))
    }

    fn sign(&self, message: &[u8]) -> Vec<u8> {
        encode(
            &self
                .keys
                .sign_deterministic(VALIDATOR_DOMAIN, message)
                .expect("signing"),
        )
    }
}

/// A consensus header signed by `auth`.
fn signed_header(auth: &KeyAuth, round: u64, shard: u16, payload: &[u8]) -> SignedHeader {
    let header = Header {
        author: auth.validator(),
        chain: Hash32([1; 32]),
        round: Round(round),
        epoch: Epoch(0),
        shard: ShardId(shard),
        parents: Vec::new(),
        payload: vec![payload.to_vec()],
    };
    SignedHeader {
        signature: auth.sign(&Header::signing_digest(&header.digest()).0),
        header,
    }
}

fn report_headers(first: SignedHeader, second: SignedHeader) -> TxKind {
    TxKind::ReportEquivocation {
        evidence: Box::new(Equivocation::Headers { first, second }),
    }
}

/// Lane owners for evidence: every request is authorized.
struct AnyOwner;

impl OwnerAuthorization for AnyOwner {
    fn authorizes(&self, _: &LaneRequest) -> bool {
        true
    }
}

/// `auth`'s signed lane approval of `tx`.
fn approval(auth: &KeyAuth, tx: &Transaction) -> EvidenceApproval {
    let committee = Committee::equal_weight(Epoch(0), ShardId(0), [auth.validator()])
        .expect("a one-member committee");
    let mut lane = LaneValidator::new(&committee, auth.clone());
    let digest = tx.digest();
    let request = LaneRequest {
        tx: digest,
        inputs: tx
            .inputs
            .iter()
            .map(|input| sion_consensus::ObjectRef {
                id: input.id,
                version: input.version,
            })
            .collect(),
        owner_proof: Vec::new(),
    };
    lane.approve(request, &AnyOwner).expect("approved");
    lane.seal_batch();
    EvidenceApproval {
        tx: tx.clone(),
        proof: lane.approval_proof(&digest).expect("sealed").clone(),
    }
}

fn report_approvals(first: EvidenceApproval, second: EvidenceApproval) -> TxKind {
    TxKind::ReportEquivocation {
        evidence: Box::new(Equivocation::Approvals {
            first: Box::new(first),
            second: Box::new(second),
        }),
    }
}

/// Two transfers by `name` that spend the same coin (bodies only: they
/// serve as approved transactions in evidence).
fn double_spend(h: &mut Harness, name: &str) -> (Transaction, Transaction) {
    let to = h.id(name);
    let one = h.tx(
        name,
        TxKind::Transfer {
            asset: usd(),
            outputs: vec![(to, units(1))],
        },
    );
    h.settle();
    let two = h.tx(
        name,
        TxKind::Transfer {
            asset: usd(),
            outputs: vec![(to, units(2))],
        },
    );
    h.settle();
    assert_eq!(one.tx.inputs, two.tx.inputs, "both spend the same coin");
    (one.tx, two.tx)
}

fn transfer(h: &mut Harness, from: &str, to: &str, amount: Amount) -> SignedTransaction {
    let to = h.id(to);
    h.tx(
        from,
        TxKind::Transfer {
            asset: usd(),
            outputs: vec![(to, amount)],
        },
    )
}

/// Transfers whose fees flow into the treasury.
fn fee_traffic(h: &mut Harness, from: &str, to: &str, count: usize) -> Vec<SignedTransaction> {
    (0..count)
        .map(|n| transfer(h, from, to, units(n as u64 + 1)))
        .collect()
}

/// Re-signs `name`'s transaction after `edit` changes its body.
fn edited(
    h: &Harness,
    name: &str,
    signed: &SignedTransaction,
    edit: impl FnOnce(&mut Transaction),
) -> SignedTransaction {
    let mut body = signed.tx.clone();
    edit(&mut body);
    h.resign(name, body)
}

fn count_events(out: &PulseOutput, name: &str) -> usize {
    all_events(out)
        .iter()
        .filter(|event| event_name(event) == name)
        .count()
}

fn posted_events(out: &PulseOutput) -> BTreeSet<TxDigest> {
    all_events(out)
        .into_iter()
        .filter_map(|event| match event {
            Event::ForcedPosted { intent, .. } => Some(intent),
            _ => None,
        })
        .collect()
}

fn dismissed_events(out: &PulseOutput) -> BTreeSet<TxDigest> {
    all_events(out)
        .into_iter()
        .filter_map(|event| match event {
            Event::ForcedDismissed { intent, .. } => Some(intent),
            _ => None,
        })
        .collect()
}

/// The members `sample_committee` gives shard `shard` under the set's
/// current seed (a shard's lane members before any rotation).
fn sampled_members(set: &ValidatorSet, shard: u16) -> BTreeSet<ValidatorId> {
    sample_committee(
        &set.active,
        usize::from(set.params.seats),
        &set.seed,
        Epoch(set.epoch),
        ShardId(shard),
    )
    .expect("a committee")
    .members()
    .iter()
    .map(|(validator, _)| *validator)
    .collect()
}

/// Names of fresh wallets (not yet on chain) whose account falls on
/// `shard`, or off it.
fn wallet_name_on(h: &Harness, prefix: &str, shard: u16, on: bool) -> String {
    (0..64)
        .map(|n| format!("{prefix}{n}"))
        .find(|name| {
            let account = sion_exec::Wallet::named(name).policy.account_id();
            let lands = ShardId::for_account(&account, h.chain.state.config.shard_count).0 == shard;
            lands == on && !h.chain.wallets.contains_key(name)
        })
        .expect("a wallet name on the shard")
}

// ---------------------------------------------------------------------------
// Scenario (v1.3): validators register, earn, leave and withdraw by epoch
// ---------------------------------------------------------------------------

fn validator_lifecycle(chunk_size: usize) -> Harness {
    let mut h = Harness::with_params(
        &["alice", "bob", "carol", "dave"],
        50_000,
        8,
        chunk_size,
        validator_params(4),
        forced_params(3, 8),
    );
    let (k1, k2, k3) = (pool_keys(0), pool_keys(1), pool_keys(2));
    let (v1, v2, v3) = (key_id(k1), key_id(k2), key_id(k3));
    let genesis: Vec<ValidatorId> = h.genesis_validators().iter().map(|(_, id)| *id).collect();
    let (g0, g3) = (genesis[0], genesis[3]);

    // Pulse 1: the faucet, which operates the genesis validators, gets gas.
    h.fund(&[
        (FAUCET_NAME, usd(), units(1_000)),
        (FAUCET_NAME, usd(), units(1_000)),
        (FAUCET_NAME, usd(), units(1_000)),
    ]);

    // Pulse 2: one registration succeeds; a replayed proof of possession, a
    // stake below the minimum, an unaffordable stake and a second
    // registration of the same keys fail.
    let kind = registration(&h, "alice", k1, units(2_000));
    let register = h.tx("alice", kind.clone());
    let again = h.tx("alice", kind.clone());
    let stolen = h.tx("bob", kind);
    let low = {
        let kind = registration(&h, "carol", k2, Amount(units(1_000).0 - 1));
        h.tx("carol", kind)
    };
    let rich = {
        let affordable = registration(&h, "dave", k3, units(1_000));
        let signed = h.tx("dave", affordable);
        edited(&h, "dave", &signed, |body| {
            if let TxKind::RegisterValidator { stake, .. } = &mut body.kind {
                *stake = units(10_000_000);
            }
        })
    };
    let out = h.pulse(vec![
        register.clone(),
        again.clone(),
        stolen.clone(),
        low.clone(),
        rich.clone(),
    ]);
    let succeeded = [&register, &again]
        .iter()
        .filter(|tx| status_of(&out, tx) == ExecStatus::Success)
        .count();
    assert_eq!(succeeded, 1, "one of two identical registrations succeeds");
    expect(&out, &stolen, Outcome::Failed);
    expect(&out, &low, Outcome::Failed);
    expect(&out, &rich, Outcome::Failed);
    assert_eq!(h.record(&v1).status, ValidatorStatus::Joining);
    assert!(!h.validator_set().validators.contains_key(&v2));

    // Pulse 3: fees.
    let txs = fee_traffic(&mut h, "bob", "alice", 2);
    h.pulse(txs);

    // Pulse 4, epoch 0's last: carol registers at exactly the minimum, and
    // the epoch ends in the same Pulse. Both new validators become active;
    // neither is paid for epoch 0.
    let late = {
        let kind = registration(&h, "carol", k2, units(1_000));
        h.tx("carol", kind)
    };
    let mut txs = fee_traffic(&mut h, "bob", "alice", 2);
    txs.push(late.clone());
    let out = h.pulse(txs);
    expect(&out, &late, Outcome::Success);
    assert_eq!(count_events(&out, "EpochChanged"), 1);
    let set = h.validator_set();
    assert_eq!((set.epoch, set.epoch_start), (1, Pulse(5)));
    for v in [v1, v2] {
        assert_eq!(set.validators[&v].status, ValidatorStatus::Active);
        assert_eq!(set.validators[&v].rewards, Amount::ZERO);
    }
    assert!(set.validators[&g0].rewards > Amount::ZERO);
    assert_eq!(set.active.len(), 6);

    // Pulses 5–7: dave registers, unstakes while still joining (he exits at
    // once) and withdraws.
    let join = {
        let kind = registration(&h, "dave", k3, units(1_500));
        h.tx("dave", kind)
    };
    let out = h.pulse(vec![join.clone()]);
    expect(&out, &join, Outcome::Success);
    let quit = h.tx("dave", TxKind::Unstake { validator: v3 });
    let out = h.pulse(vec![quit.clone()]);
    expect(&out, &quit, Outcome::Success);
    assert_eq!(
        h.record(&v3).status,
        ValidatorStatus::Exited {
            withdrawable_at: Pulse(6)
        }
    );
    let back = h.tx("dave", TxKind::WithdrawStake { validator: v3 });
    let mut txs = fee_traffic(&mut h, "bob", "carol", 2);
    txs.push(back.clone());
    let out = h.pulse(txs);
    expect(&out, &back, Outcome::Success);
    assert_eq!(h.record(&v3).status, ValidatorStatus::Withdrawn);

    // Pulse 8, epoch 1's last: the new validators are paid by weight.
    let txs = fee_traffic(&mut h, "bob", "alice", 2);
    h.pulse(txs);
    let earned = h.record(&v1).rewards;
    assert!(earned > Amount::ZERO);
    assert!(h.record(&v2).rewards > Amount::ZERO);

    // Pulse 9: only the operator claims.
    let theft = h.tx("bob", TxKind::ClaimRewards { validator: v1 });
    let claim = h.tx("alice", TxKind::ClaimRewards { validator: v1 });
    let out = h.pulse(vec![theft.clone(), claim.clone()]);
    expect(&out, &theft, Outcome::Failed);
    expect(&out, &claim, Outcome::Success);
    assert!(all_events(&out).contains(&Event::RewardsClaimed {
        validator: v1,
        amount: earned
    }));

    // Pulse 10: nothing left to claim; alice takes v1 out, bob cannot.
    let empty = h.tx("alice", TxKind::ClaimRewards { validator: v1 });
    let leave = h.tx("alice", TxKind::Unstake { validator: v1 });
    let not_yours = h.tx("bob", TxKind::Unstake { validator: v1 });
    let out = h.pulse(vec![empty.clone(), leave.clone(), not_yours.clone()]);
    expect(&out, &empty, Outcome::Failed);
    expect(&out, &leave, Outcome::Success);
    expect(&out, &not_yours, Outcome::Failed);
    assert_eq!(h.record(&v1).status, ValidatorStatus::Leaving);

    // Pulse 11: leaving twice or withdrawing while leaving fails; the
    // faucet claims a genesis validator's rewards and takes another out.
    let twice = h.tx("alice", TxKind::Unstake { validator: v1 });
    let early = h.tx("alice", TxKind::WithdrawStake { validator: v1 });
    let genesis_claim = h.tx(FAUCET_NAME, TxKind::ClaimRewards { validator: g0 });
    let genesis_leave = h.tx(FAUCET_NAME, TxKind::Unstake { validator: g3 });
    let out = h.pulse(vec![
        twice.clone(),
        early.clone(),
        genesis_claim.clone(),
        genesis_leave.clone(),
    ]);
    expect(&out, &twice, Outcome::Failed);
    expect(&out, &early, Outcome::Failed);
    expect(&out, &genesis_claim, Outcome::Success);
    expect(&out, &genesis_leave, Outcome::Success);

    // Pulse 12, epoch 2's last: leaving validators are paid once more and
    // exit; they can withdraw after the unbonding period.
    let txs = fee_traffic(&mut h, "bob", "alice", 2);
    h.pulse(txs);
    let exited = ValidatorStatus::Exited {
        withdrawable_at: Pulse(15),
    };
    assert_eq!(h.record(&v1).status, exited);
    assert_eq!(h.record(&g3).status, exited);
    assert!(h.record(&v1).rewards > Amount::ZERO);
    assert!(!h.validator_set().active.iter().any(|(v, _)| *v == v1));

    // Pulses 13–14: too early.
    for _ in 0..2 {
        let early = h.tx("alice", TxKind::WithdrawStake { validator: v1 });
        let out = h.pulse(vec![early.clone()]);
        expect(&out, &early, Outcome::Failed);
    }
    // Pulse 15: exactly at withdrawable_at.
    let before = h.balance("alice", &usd());
    let stake = h.record(&v1).stake;
    let rewards = h.record(&v1).rewards;
    let withdraw = h.tx("alice", TxKind::WithdrawStake { validator: v1 });
    let genesis_withdraw = h.tx(FAUCET_NAME, TxKind::WithdrawStake { validator: g3 });
    let out = h.pulse(vec![withdraw.clone(), genesis_withdraw.clone()]);
    expect(&out, &withdraw, Outcome::Success);
    expect(&out, &genesis_withdraw, Outcome::Success);
    let fee = out
        .effects
        .iter()
        .find(|e| e.digest == withdraw.digest())
        .map(|e| e.fee)
        .unwrap();
    assert_eq!(
        h.balance("alice", &usd()).0,
        before.0 + stake.0 + rewards.0 - fee.0,
        "stake and unclaimed rewards return"
    );

    // Pulse 16: a withdrawn validator has nothing left.
    let again = h.tx("alice", TxKind::WithdrawStake { validator: v1 });
    let leave = h.tx("alice", TxKind::Unstake { validator: v1 });
    let claim = h.tx("alice", TxKind::ClaimRewards { validator: v1 });
    let out = h.pulse(vec![again.clone(), leave.clone(), claim.clone()]);
    for tx in [&again, &leave, &claim] {
        expect(&out, tx, Outcome::Failed);
    }

    // More epochs, with and without fees.
    let txs = fee_traffic(&mut h, "carol", "bob", 3);
    h.pulse(txs);
    h.advance_to(25);
    assert_eq!(h.validator_set().epoch, 6);

    h.assert_covered(&[
        "RegisterValidator/ok",
        "RegisterValidator/failed",
        "Unstake/ok",
        "Unstake/failed",
        "WithdrawStake/ok",
        "WithdrawStake/failed",
        "ClaimRewards/ok",
        "ClaimRewards/failed",
        "due/EpochEnd",
        "deferred-event/EpochChanged",
    ]);
    h.assert_minimal_witnesses();
    h
}

#[test]
fn validators_register_earn_leave_and_withdraw_one_per_chunk() {
    validator_lifecycle(1).summary("validator lifecycle (1 per chunk)");
}

#[test]
fn validators_register_earn_leave_and_withdraw_many_per_chunk() {
    validator_lifecycle(4).summary("validator lifecycle (4 per chunk)");
}

// ---------------------------------------------------------------------------
// Scenario (v1.3): equivocation evidence and slashing
// ---------------------------------------------------------------------------

fn equivocation(chunk_size: usize) -> Harness {
    let mut h = Harness::with_params(
        &["alice", "bob", "carol", "dave"],
        50_000,
        8,
        chunk_size,
        validator_params(5),
        forced_params(3, 8),
    );
    let genesis = h.genesis_validators();
    let auth: Vec<KeyAuth> = genesis.iter().map(|(keys, _)| KeyAuth::of(keys)).collect();
    let ids: Vec<ValidatorId> = genesis.iter().map(|(_, id)| *id).collect();

    // Pulses 1–5 (epoch 0): fees, so the genesis validators earn rewards.
    for _ in 0..4 {
        let txs = fee_traffic(&mut h, "dave", "alice", 2);
        h.pulse(txs);
    }
    h.advance_to(5);
    let rewards = h.record(&ids[0]).rewards;
    assert!(rewards > Amount::ZERO);
    let stake = h.record(&ids[0]).stake;

    // Pulse 6: two headers for one round slash g0. The same header twice,
    // headers of different rounds or shards, a signer claiming another's
    // identity and an unregistered author are not evidence.
    let real = h.tx(
        "bob",
        report_headers(
            signed_header(&auth[0], 7, 0, b"a"),
            signed_header(&auth[0], 7, 0, b"b"),
        ),
    );
    let same = h.tx(
        "carol",
        report_headers(
            signed_header(&auth[1], 7, 0, b"a"),
            signed_header(&auth[1], 7, 0, b"a"),
        ),
    );
    let rounds = h.tx(
        "carol",
        report_headers(
            signed_header(&auth[1], 7, 0, b"a"),
            signed_header(&auth[1], 8, 0, b"b"),
        ),
    );
    let shards = h.tx(
        "dave",
        report_headers(
            signed_header(&auth[1], 7, 0, b"a"),
            signed_header(&auth[1], 7, 1, b"b"),
        ),
    );
    let impostor = KeyAuth::claiming(&genesis[2].0, ids[1]);
    let forged = h.tx(
        "dave",
        report_headers(
            signed_header(&impostor, 7, 0, b"a"),
            signed_header(&impostor, 7, 0, b"b"),
        ),
    );
    let outsider = KeyAuth::of(pool_keys(7));
    let stranger = h.tx(
        "alice",
        report_headers(
            signed_header(&outsider, 7, 0, b"a"),
            signed_header(&outsider, 7, 0, b"b"),
        ),
    );
    let before = h.balance("bob", &usd());
    let out = h.pulse(vec![
        real.clone(),
        same.clone(),
        rounds.clone(),
        shards.clone(),
        forged.clone(),
        stranger.clone(),
    ]);
    expect(&out, &real, Outcome::Success);
    for tx in [&same, &rounds, &shards, &forged, &stranger] {
        expect(&out, tx, Outcome::Failed);
    }
    let record = h.record(&ids[0]);
    assert_eq!(record.status, ValidatorStatus::Slashed { at: Pulse(6) });
    assert_eq!((record.stake, record.rewards), (Amount::ZERO, Amount::ZERO));
    let fee = out
        .effects
        .iter()
        .find(|e| e.digest == real.digest())
        .map(|e| e.fee)
        .unwrap();
    assert_eq!(
        h.balance("bob", &usd()).0,
        before.0 + stake.0 / 2 + rewards.0 / 2 - fee.0,
        "the reporter receives half of the stake and of the rewards"
    );
    let deferred: Vec<Deferred> = out
        .effects
        .iter()
        .find(|e| e.digest == real.digest())
        .map(|e| e.deferred.clone())
        .unwrap();
    assert!(
        matches!(
            deferred.as_slice(),
            [
                Deferred::Treasury { .. },
                Deferred::Treasury { .. },
                Deferred::Fee { .. }
            ]
        ),
        "the rest of the stake and of the rewards go to the treasury, then the fee: {deferred:?}"
    );

    // Pulse 7: g0 cannot be slashed twice; two live approvals of one coin
    // version slash g1; alice registers a validator.
    let (one, two) = double_spend(&mut h, "carol");
    let again = h.tx(
        "alice",
        report_headers(
            signed_header(&auth[0], 9, 0, b"c"),
            signed_header(&auth[0], 9, 0, b"d"),
        ),
    );
    let approvals = h.tx(
        "bob",
        report_approvals(approval(&auth[1], &one), approval(&auth[1], &two)),
    );
    let k1 = pool_keys(0);
    let join = {
        let kind = registration(&h, "alice", k1, units(3_000));
        h.tx("alice", kind)
    };
    let out = h.pulse(vec![again.clone(), approvals.clone(), join.clone()]);
    expect(&out, &again, Outcome::Failed);
    expect(&out, &approvals, Outcome::Success);
    expect(&out, &join, Outcome::Success);
    assert!(matches!(
        h.record(&ids[1]).status,
        ValidatorStatus::Slashed { .. }
    ));

    // Pulse 8: approvals that expired, of the same transaction, without a
    // common input, or signed by an impostor are not evidence; a joining
    // validator's double header is.
    let mut old_one = one.clone();
    old_one.expires_at = Pulse(7);
    let mut old_two = two.clone();
    old_two.expires_at = Pulse(7);
    let expired = h.tx(
        "dave",
        report_approvals(approval(&auth[2], &old_one), approval(&auth[2], &old_two)),
    );
    let same_tx = h.tx(
        "dave",
        report_approvals(approval(&auth[2], &one), approval(&auth[2], &one)),
    );
    let mut other = two.clone();
    other.inputs = vec![h.coins("dave", &usd())[0]];
    let disjoint = h.tx(
        "carol",
        report_approvals(approval(&auth[2], &one), approval(&auth[2], &other)),
    );
    let impostor = KeyAuth::claiming(&genesis[3].0, ids[2]);
    let forged = h.tx(
        "carol",
        report_approvals(approval(&impostor, &one), approval(&impostor, &two)),
    );
    let joining = KeyAuth::of(k1);
    let newcomer = h.tx(
        "bob",
        report_headers(
            signed_header(&joining, 3, 1, b"x"),
            signed_header(&joining, 3, 1, b"y"),
        ),
    );
    let out = h.pulse(vec![
        expired.clone(),
        same_tx.clone(),
        disjoint.clone(),
        forged.clone(),
        newcomer.clone(),
    ]);
    for tx in [&expired, &same_tx, &disjoint, &forged] {
        expect(&out, tx, Outcome::Failed);
    }
    expect(&out, &newcomer, Outcome::Success);
    assert!(matches!(
        h.record(&key_id(k1)).status,
        ValidatorStatus::Slashed { .. }
    ));

    // Pulse 10, epoch 1's last: the slashed validators' weight still counts,
    // but they are not paid, and they leave the active set.
    let txs = fee_traffic(&mut h, "dave", "alice", 2);
    h.pulse(txs);
    h.advance_to(10);
    let set = h.validator_set();
    let mut expected = vec![ids[2], ids[3]];
    expected.sort();
    assert_eq!(
        set.active.iter().map(|(v, _)| *v).collect::<Vec<_>>(),
        expected
    );
    assert_eq!(set.validators[&ids[0]].rewards, Amount::ZERO);
    assert!(set.validators[&ids[2]].rewards > Amount::ZERO);

    // Pulse 11: slashing a third leaves one validator for the next epoch.
    let third = h.tx(
        "carol",
        report_headers(
            signed_header(&auth[2], 1, 0, b"p"),
            signed_header(&auth[2], 1, 0, b"q"),
        ),
    );
    let out = h.pulse(vec![third.clone()]);
    expect(&out, &third, Outcome::Success);
    h.advance_to(16);
    assert_eq!(h.validator_set().active.len(), 1);

    // Pulse 17 (v1.5): slashing the last one leaves nobody eligible. At the
    // boundary (Pulse 20) the slashed validators lose their seats anyway, so
    // the next epoch has no active set and the chain halts; both stacks
    // agree on that state and on Pulses executed after it.
    let last = h.tx(
        "dave",
        report_headers(
            signed_header(&auth[3], 2, 0, b"r"),
            signed_header(&auth[3], 2, 0, b"s"),
        ),
    );
    let out = h.pulse(vec![last.clone()]);
    expect(&out, &last, Outcome::Success);
    h.advance_to(20);
    let set = h.validator_set();
    assert_eq!(set.epoch, 4);
    assert!(set.active.is_empty(), "{:?}", set.active);
    h.advance_to(22);

    h.assert_covered(&[
        "ReportEquivocation/ok",
        "ReportEquivocation/failed",
        "RegisterValidator/ok",
        "due/EpochEnd",
    ]);
    h.assert_minimal_witnesses();
    h
}

#[test]
fn equivocation_is_slashed_on_valid_evidence_only_one_per_chunk() {
    equivocation(1).summary("equivocation (1 per chunk)");
}

#[test]
fn equivocation_is_slashed_on_valid_evidence_many_per_chunk() {
    equivocation(3).summary("equivocation (3 per chunk)");
}

// ---------------------------------------------------------------------------
// Scenario (v1.3): forced intents are posted, resolved, overdue, rotated
// ---------------------------------------------------------------------------

fn forced_inclusion(chunk_size: usize) -> Harness {
    let mut h = Harness::with_params(
        &["alice", "bob", "carol", "dave"],
        50_000,
        8,
        chunk_size,
        validator_params(8),
        forced_params(3, 4),
    );
    let window = 3;
    let bob_shard = h.shard_of("bob");
    // A new account on bob's shard (its intents are owed by the same
    // committee) and one on the other shard.
    let fresh = wallet_name_on(&h, "fresh", bob_shard, true);
    let erin = wallet_name_on(&h, "erin", bob_shard, false);
    h.chain.add_wallet(&fresh);
    h.chain.add_wallet(&erin);

    // Pulse 1: funds for the new accounts, and a non-USD coin for carol.
    h.fund(&[
        (&fresh, usd(), units(100)),
        (&erin, usd(), units(100)),
        ("carol", gpu(), units(5)),
        (FAUCET_NAME, usd(), units(100)),
    ]);

    // Pulse 2: two admissible intents (one listed twice: it is posted once),
    // a faucet intent and an intent that expires in this very Pulse.
    let p2 = h.pulse_number() + 1;
    let resolved = transfer(&mut h, "alice", "bob", units(5));
    let censored = transfer(&mut h, "bob", "alice", units(5));
    let faucet = {
        let grants = vec![(h.id("bob"), usd(), units(1))];
        let wallet = h.chain.wallets.get_mut(FAUCET_NAME).expect("faucet");
        wallet
            .faucet(&h.chain.state, grants)
            .expect("a faucet intent")
    };
    let expiring = {
        let signed = transfer(&mut h, "carol", "alice", units(1));
        edited(&h, "carol", &signed, |body| body.expires_at = Pulse(p2))
    };
    h.settle();
    let out = h.pulse_forced(
        Vec::new(),
        vec![
            resolved.clone(),
            censored.clone(),
            resolved.clone(),
            faucet.clone(),
            expiring.clone(),
        ],
    );
    assert_eq!(h.last_posted().len(), 4, "a digest is posted once");
    assert_eq!(
        posted_events(&out),
        [resolved.digest(), censored.digest()].into()
    );
    assert_eq!(
        dismissed_events(&out),
        [faucet.digest(), expiring.digest()].into()
    );
    let pending = h.obligations().pending;
    assert_eq!(pending[&censored.digest()].deadline, Pulse(p2 + window));
    assert_eq!(pending[&censored.digest()].shard, bob_shard);

    // Pulse 3: alice's intent executes (her shard did its job); bob's is
    // already pending; malformed intents are dismissed; an intent spending
    // a coin a transaction of the same Pulse spends is stale by the time
    // intents are posted.
    let base = transfer(&mut h, "carol", "alice", units(1));
    let carol_pay = transfer(&mut h, "carol", "bob", units(2));
    let conflict = edited(&h, "carol", &carol_pay, |body| body.nonce += 1);
    let original_auth = base.auth.clone();
    let malformed = [
        edited(&h, "carol", &base, |body| body.fee = Amount::ZERO),
        edited(&h, "carol", &base, |body| body.inputs.clear()),
        edited(&h, "carol", &base, |body| {
            let first = body.inputs[0];
            body.inputs.push(first);
        }),
        edited(&h, "carol", &base, |body| {
            body.inputs = (0..=sion_exec::tx::MAX_INPUTS as u32)
                .map(|n| ObjectRef {
                    id: ObjectId::derive(&n.to_be_bytes()),
                    version: Version(0),
                })
                .collect();
        }),
        {
            let mut forged = base.clone();
            forged.tx.nonce += 1;
            forged.auth = original_auth;
            forged
        },
    ];
    let mut forced = vec![censored.clone(), conflict.clone()];
    forced.extend(malformed.iter().cloned());
    let out = h.pulse_forced(vec![resolved.clone(), carol_pay.clone()], forced);
    expect(&out, &resolved, Outcome::Success);
    expect(&out, &carol_pay, Outcome::Success);
    assert!(posted_events(&out).is_empty());
    assert_eq!(dismissed_events(&out).len(), 7);

    // Pulse 4: intents whose sender or inputs disqualify them, and one from
    // an account that does not exist yet (its genesis policy admits it).
    let fresh_intent = transfer(&mut h, &fresh, "alice", units(1));
    assert!(fresh_intent.tx.genesis_policy.is_some());
    let no_policy = edited(&h, &fresh, &fresh_intent, |body| {
        body.genesis_policy = None;
        body.nonce += 1;
    });
    let alice_policy = h.chain.wallet("alice").policy.clone();
    let wrong_policy = edited(&h, &fresh, &fresh_intent, |body| {
        body.genesis_policy = Some(alice_policy);
        body.nonce += 2;
    });
    let base = transfer(&mut h, "carol", "alice", units(1));
    let gas = base.tx.inputs[0];
    let gas_amount = h.chain.state.objects[&gas.id]
        .as_coin()
        .expect("a coin")
        .amount;
    let tombstone = h.reference(&carol_pay.tx.inputs[0].id);
    assert!(h.chain.state.objects[&tombstone.id].is_tombstone());
    let foreign = h.coins("bob", &usd())[0];
    let goods = h.coins("carol", &gpu())[0];
    let with_first = |first: ObjectRef| {
        move |body: &mut Transaction| {
            body.inputs[0] = first;
        }
    };
    let disqualified = [
        no_policy,
        wrong_policy,
        edited(
            &h,
            "carol",
            &base,
            with_first(ObjectRef {
                id: ObjectId::derive(b"nowhere"),
                version: Version(0),
            }),
        ),
        edited(
            &h,
            "carol",
            &base,
            with_first(ObjectRef {
                id: gas.id,
                version: Version(gas.version.0 + 1),
            }),
        ),
        edited(&h, "carol", &base, with_first(tombstone)),
        edited(&h, "carol", &base, with_first(foreign)),
        edited(&h, "carol", &base, with_first(goods)),
        edited(&h, "carol", &base, |body| {
            body.fee = Amount(gas_amount.0 + 1);
        }),
    ];
    let mut forced = vec![fresh_intent.clone()];
    forced.extend(disqualified.iter().cloned());
    h.settle();
    let out = h.pulse_forced(Vec::new(), forced);
    assert_eq!(posted_events(&out), [fresh_intent.digest()].into());
    assert_eq!(dismissed_events(&out).len(), disqualified.len());

    // Pulse 5, the first deadline: alice's intent can no longer execute
    // (resolved); bob's still can (overdue), and bob's shard is rotated to
    // the two validators outside its committee. Intents posted in this
    // Pulse see the obligations due in it still pending: dave's is the
    // fourth, so carol's is dismissed.
    let dave_intent = transfer(&mut h, "dave", "alice", units(1));
    let dave_policy = h.chain.wallet("dave").policy.clone();
    let dave_rotate = h.tx(
        "dave",
        TxKind::RotatePolicy {
            policy: dave_policy,
            viewing_key: None,
        },
    );
    assert_ne!(dave_rotate.tx.inputs[0], dave_intent.tx.inputs[0]);
    let carol_intent = transfer(&mut h, "carol", "dave", units(1));
    let previous = sampled_members(&h.validator_set(), bob_shard);
    let out = h.pulse_forced(Vec::new(), vec![dave_intent.clone(), carol_intent.clone()]);
    assert_eq!(posted_events(&out), [dave_intent.digest()].into());
    assert_eq!(dismissed_events(&out), [carol_intent.digest()].into());
    let events = all_events(&out);
    assert!(events.contains(&Event::ForcedResolved {
        intent: resolved.digest()
    }));
    assert!(events.contains(&Event::ForcedOverdue {
        intent: censored.digest(),
        shard: bob_shard
    }));
    assert!(events.contains(&Event::ShardRotated {
        shard: bob_shard,
        rotation: 1
    }));
    let rotation = h.validator_set().rotations[&bob_shard].clone();
    let members: BTreeSet<ValidatorId> = rotation.members.iter().copied().collect();
    assert_eq!(members.len(), 2);
    assert!(
        members.is_disjoint(&previous),
        "an entirely fresh committee"
    );
    let obligation = h.obligations().pending[&censored.digest()].clone();
    assert_eq!(
        (obligation.missed, obligation.deadline),
        (1, Pulse(p2 + 2 * window))
    );

    // Pulse 6: dave rotates his policy (to the same keys), so his intent
    // can no longer execute although its coin is untouched; erin's intent
    // expires before its window ends, so its deadline is its expiry.
    let p6 = h.pulse_number() + 1;
    let erin_intent = {
        let signed = transfer(&mut h, &erin, "alice", units(1));
        edited(&h, &erin, &signed, |body| body.expires_at = Pulse(p6 + 1))
    };
    let out = h.pulse_forced(vec![dave_rotate.clone()], vec![erin_intent.clone()]);
    expect(&out, &dave_rotate, Outcome::Success);
    assert_eq!(posted_events(&out), [erin_intent.digest()].into());
    assert_eq!(
        h.obligations().pending[&erin_intent.digest()].deadline,
        Pulse(p6 + 1)
    );

    // Pulse 7: the new account's intent is overdue, but bob's shard rotated
    // less than a window ago (no rotation); erin's is overdue at its expiry:
    // the other shard rotates and the obligation ends.
    let out = h.pulse(Vec::new());
    let events = all_events(&out);
    assert!(events.contains(&Event::ForcedOverdue {
        intent: fresh_intent.digest(),
        shard: bob_shard
    }));
    let erin_shard = 1 - bob_shard;
    assert!(events.contains(&Event::ForcedOverdue {
        intent: erin_intent.digest(),
        shard: erin_shard
    }));
    assert_eq!(count_events(&out, "ShardRotated"), 1);
    assert!(events.contains(&Event::ShardRotated {
        shard: erin_shard,
        rotation: 1
    }));
    let pending = h.obligations().pending;
    assert!(!pending.contains_key(&erin_intent.digest()));
    assert_eq!(pending[&fresh_intent.digest()].missed, 1);

    // Pulse 8, epoch 0's last: bob's intent is overdue again a window after
    // the rotation (rotation 2, from the members now serving); dave's is
    // resolved by his policy rotation; then the epoch ends and the
    // rotations are cleared.
    let out = h.pulse(Vec::new());
    let events = all_events(&out);
    assert!(events.contains(&Event::ShardRotated {
        shard: bob_shard,
        rotation: 2
    }));
    assert!(events.contains(&Event::ForcedResolved {
        intent: dave_intent.digest()
    }));
    assert_eq!(count_events(&out, "EpochChanged"), 1);
    assert!(h.validator_set().rotations.is_empty());

    // Pulse 9: bob's intent finally executes.
    let out = h.pulse(vec![censored.clone()]);
    expect(&out, &censored, Outcome::Success);
    // Pulse 10: the new account's intent is overdue again; in the new epoch
    // bob's shard rotates afresh (rotation 1).
    let out = h.pulse(Vec::new());
    assert!(all_events(&out).contains(&Event::ShardRotated {
        shard: bob_shard,
        rotation: 1
    }));
    // Pulse 11: bob's obligation is resolved; the new account's intent
    // executes, creating its account.
    let out = h.pulse(vec![fresh_intent.clone()]);
    expect(&out, &fresh_intent, Outcome::Success);
    assert!(all_events(&out).contains(&Event::ForcedResolved {
        intent: censored.digest()
    }));
    h.advance_to(17);
    assert!(h.obligations().pending.is_empty());

    h.assert_covered(&[
        "due/ForcedDeadline",
        "due/EpochEnd",
        "deferred-event/ForcedPosted",
        "deferred-event/ForcedDismissed",
        "deferred-event/ForcedResolved",
        "deferred-event/ForcedOverdue",
        "deferred-event/ShardRotated",
        "RotatePolicy/ok",
    ]);
    h.assert_minimal_witnesses();
    h
}

#[test]
fn forced_intents_post_resolve_and_rotate_one_per_chunk() {
    forced_inclusion(1).summary("forced inclusion (1 per chunk)");
}

#[test]
fn forced_intents_post_resolve_and_rotate_many_per_chunk() {
    forced_inclusion(4).summary("forced inclusion (4 per chunk)");
}

// ---------------------------------------------------------------------------
// Property test (v1.3): random validator, epoch and forced-intent workloads
// ---------------------------------------------------------------------------

/// The accounts of the random workloads; index 4 is the faucet, which
/// operates the genesis validators.
const ACTORS: [&str; 5] = ["a", "b", "c", "d", FAUCET_NAME];

#[derive(Clone, Copy, Debug)]
enum StakeKind {
    Valid,
    Minimum,
    Low,
    Replayed,
}

#[derive(Clone, Copy, Debug)]
enum EvidenceKind {
    Headers,
    SameHeader,
    WrongSigner,
    Approvals,
    StaleApprovals,
}

#[derive(Clone, Copy, Debug)]
enum IntentKind {
    Transfer,
    Faucet,
    Expired,
    BadAuth,
    Stale,
    Twice,
}

/// One step of a random workload. Validator targets 0–3 are the genesis
/// validators, 4–7 the pool keys; `as_operator` sends the transaction from
/// the target's operator when it has one.
#[derive(Clone, Debug)]
enum ChainOp {
    Pay {
        from: usize,
        to: usize,
    },
    Register {
        who: usize,
        key: usize,
        stake: StakeKind,
    },
    Unstake {
        who: usize,
        target: usize,
        as_operator: bool,
    },
    Withdraw {
        who: usize,
        target: usize,
        as_operator: bool,
    },
    Claim {
        who: usize,
        target: usize,
        as_operator: bool,
    },
    Report {
        who: usize,
        target: usize,
        evidence: EvidenceKind,
    },
    Force {
        who: usize,
        intent: IntentKind,
    },
    Execute {
        which: usize,
    },
}

#[derive(Clone, Debug)]
struct ChainParams {
    chunk_size: usize,
    epoch_length: u64,
    unbonding: u64,
    max_active: u16,
    reward_permille: u16,
    reporter_permille: u16,
    window: u64,
    max_pending: u32,
}

fn chain_params() -> impl Strategy<Value = ChainParams> {
    (
        1usize..5,
        1u64..7,
        0u64..5,
        2u16..7,
        prop_oneof![Just(0u16), Just(333), Just(500), Just(1000)],
        prop_oneof![Just(0u16), Just(250), Just(500), Just(1000)],
        1u64..5,
        1u32..6,
    )
        .prop_map(
            |(
                chunk_size,
                epoch_length,
                unbonding,
                max_active,
                reward_permille,
                reporter_permille,
                window,
                max_pending,
            )| ChainParams {
                chunk_size,
                epoch_length,
                unbonding,
                max_active,
                reward_permille,
                reporter_permille,
                window,
                max_pending,
            },
        )
}

fn chain_op() -> impl Strategy<Value = ChainOp> {
    let stake = prop_oneof![
        3 => Just(StakeKind::Valid),
        1 => Just(StakeKind::Minimum),
        1 => Just(StakeKind::Low),
        1 => Just(StakeKind::Replayed),
    ];
    let evidence = prop_oneof![
        2 => Just(EvidenceKind::Headers),
        1 => Just(EvidenceKind::SameHeader),
        1 => Just(EvidenceKind::WrongSigner),
        2 => Just(EvidenceKind::Approvals),
        1 => Just(EvidenceKind::StaleApprovals),
    ];
    let intent = prop_oneof![
        4 => Just(IntentKind::Transfer),
        1 => Just(IntentKind::Faucet),
        1 => Just(IntentKind::Expired),
        1 => Just(IntentKind::BadAuth),
        1 => Just(IntentKind::Stale),
        1 => Just(IntentKind::Twice),
    ];
    prop_oneof![
        3 => (0..4usize, 0..4usize).prop_map(|(from, to)| ChainOp::Pay { from, to }),
        3 => (0..4usize, 0..4usize, stake)
            .prop_map(|(who, key, stake)| ChainOp::Register { who, key, stake }),
        2 => (0..5usize, 0..8usize, any::<bool>())
            .prop_map(|(who, target, as_operator)| ChainOp::Unstake { who, target, as_operator }),
        2 => (0..5usize, 0..8usize, any::<bool>())
            .prop_map(|(who, target, as_operator)| ChainOp::Withdraw { who, target, as_operator }),
        2 => (0..5usize, 0..8usize, any::<bool>())
            .prop_map(|(who, target, as_operator)| ChainOp::Claim { who, target, as_operator }),
        2 => (0..4usize, 0..8usize, evidence)
            .prop_map(|(who, target, evidence)| ChainOp::Report { who, target, evidence }),
        4 => (0..4usize, intent).prop_map(|(who, intent)| ChainOp::Force { who, intent }),
        2 => (0..16usize).prop_map(|which| ChainOp::Execute { which }),
    ]
}

/// The keys and identifier of validator target `target`.
fn target_keys(
    genesis: &[(DualKeypair, ValidatorId)],
    target: usize,
) -> (DualKeypair, ValidatorId) {
    if target < 4 {
        genesis[target].clone()
    } else {
        let keys = pool_keys(target - 4);
        (keys.clone(), key_id(keys))
    }
}

/// The workload account that operates `validator`, if any.
fn operator_of(h: &Harness, validator: &ValidatorId) -> Option<&'static str> {
    let operator = h.validator_set().validators.get(validator)?.operator;
    ACTORS.iter().copied().find(|name| h.id(name) == operator)
}

/// Runs one random workload; returns what it exercised.
fn run_chain_workload(params: &ChainParams, pulses: &[Vec<ChainOp>]) -> BTreeSet<String> {
    let mut validators = validator_params(params.epoch_length);
    validators.unbonding = params.unbonding;
    validators.max_active = params.max_active;
    validators.reward_permille = params.reward_permille;
    validators.reporter_permille = params.reporter_permille;
    let mut h = Harness::with_params(
        &ACTORS[..4],
        20_000,
        6,
        params.chunk_size,
        validators,
        forced_params(params.window, params.max_pending),
    );
    h.fund(&[
        (FAUCET_NAME, usd(), units(1_000)),
        (FAUCET_NAME, usd(), units(1_000)),
        (FAUCET_NAME, usd(), units(1_000)),
    ]);
    let genesis = h.genesis_validators();
    let mut posted: Vec<SignedTransaction> = Vec::new();
    let mut round = 0u64;
    let payment = |h: &Harness, to: usize| TxKind::Transfer {
        asset: usd(),
        outputs: vec![(h.id(ACTORS[to]), units(1))],
    };
    for ops in pulses {
        let mut txs: Vec<SignedTransaction> = Vec::new();
        let mut forced: Vec<SignedTransaction> = Vec::new();
        let now = h.pulse_number() + 1;
        for op in ops {
            match *op {
                ChainOp::Pay { from, to } => {
                    let kind = payment(&h, to);
                    txs.extend(h.chain.tx(ACTORS[from], kind).ok());
                }
                ChainOp::Register { who, key, stake } => {
                    let keys = pool_keys(key);
                    let minimum = units(1_000);
                    let kind = match stake {
                        StakeKind::Valid => {
                            registration(&h, ACTORS[who], keys, units(1_000 + 250 * key as u64))
                        }
                        StakeKind::Minimum => registration(&h, ACTORS[who], keys, minimum),
                        StakeKind::Low => {
                            registration(&h, ACTORS[who], keys, Amount(minimum.0 - 1))
                        }
                        StakeKind::Replayed => {
                            registration(&h, ACTORS[(who + 1) % 4], keys, units(1_500))
                        }
                    };
                    txs.extend(h.chain.tx(ACTORS[who], kind).ok());
                }
                ChainOp::Unstake {
                    who,
                    target,
                    as_operator,
                }
                | ChainOp::Withdraw {
                    who,
                    target,
                    as_operator,
                }
                | ChainOp::Claim {
                    who,
                    target,
                    as_operator,
                } => {
                    let validator = target_keys(&genesis, target).1;
                    let actor = if as_operator {
                        operator_of(&h, &validator).unwrap_or(ACTORS[who])
                    } else {
                        ACTORS[who]
                    };
                    let kind = match op {
                        ChainOp::Unstake { .. } => TxKind::Unstake { validator },
                        ChainOp::Withdraw { .. } => TxKind::WithdrawStake { validator },
                        _ => TxKind::ClaimRewards { validator },
                    };
                    txs.extend(h.chain.tx(actor, kind).ok());
                }
                ChainOp::Report {
                    who,
                    target,
                    evidence,
                } => {
                    let (keys, id) = target_keys(&genesis, target);
                    let auth = KeyAuth::of(&keys);
                    round += 1;
                    let kind = match evidence {
                        EvidenceKind::Headers => report_headers(
                            signed_header(&auth, round, 0, b"x"),
                            signed_header(&auth, round, 0, b"y"),
                        ),
                        EvidenceKind::SameHeader => {
                            let header = signed_header(&auth, round, 1, b"x");
                            report_headers(header.clone(), header)
                        }
                        EvidenceKind::WrongSigner => {
                            let other = target_keys(&genesis, (target + 1) % 8).0;
                            let impostor = KeyAuth::claiming(&other, id);
                            report_headers(
                                signed_header(&impostor, round, 0, b"x"),
                                signed_header(&impostor, round, 0, b"y"),
                            )
                        }
                        EvidenceKind::Approvals | EvidenceKind::StaleApprovals => {
                            let kind = payment(&h, who);
                            let Ok(base) = h.chain.tx(ACTORS[who], kind) else {
                                continue;
                            };
                            let mut one = base.tx;
                            if matches!(evidence, EvidenceKind::StaleApprovals) {
                                one.expires_at = Pulse(now - 1);
                            }
                            let mut two = one.clone();
                            two.nonce += 1;
                            report_approvals(approval(&auth, &one), approval(&auth, &two))
                        }
                    };
                    txs.extend(h.chain.tx(ACTORS[who], kind).ok());
                }
                ChainOp::Force { who, intent } => {
                    let name = ACTORS[who];
                    if let IntentKind::Faucet = intent {
                        let grants = vec![(h.id(name), usd(), units(1))];
                        let wallet = h.chain.wallets.get_mut(FAUCET_NAME).expect("faucet");
                        forced.extend(wallet.faucet(&h.chain.state, grants).ok());
                        continue;
                    }
                    let kind = payment(&h, (who + 1) % 4);
                    let Ok(signed) = h.chain.tx(name, kind) else {
                        continue;
                    };
                    match intent {
                        IntentKind::Transfer => {
                            posted.push(signed.clone());
                            forced.push(signed);
                        }
                        IntentKind::Twice => {
                            posted.push(signed.clone());
                            forced.push(signed.clone());
                            forced.push(signed);
                        }
                        IntentKind::Expired => forced.push(edited(&h, name, &signed, |body| {
                            body.expires_at = Pulse(now)
                        })),
                        IntentKind::BadAuth => {
                            let mut forged = signed;
                            forged.tx.nonce += 1;
                            forced.push(forged);
                        }
                        IntentKind::Stale => forced.push(edited(&h, name, &signed, |body| {
                            body.inputs[0].version = Version(body.inputs[0].version.0 + 1)
                        })),
                        IntentKind::Faucet => unreachable!(),
                    }
                }
                ChainOp::Execute { which } => {
                    if let Some(intent) = posted.get(which % posted.len().max(1)) {
                        if !txs.iter().any(|tx| tx.digest() == intent.digest()) {
                            txs.push(intent.clone());
                        }
                    }
                }
            }
        }
        h.pulse_forced(txs, forced);
    }
    // Let deadlines pass and epochs end.
    h.advance(2 * params.window + params.epoch_length + 1);
    assert!(
        h.unread.is_empty(),
        "witness entries stack B never touched:\n{}",
        h.unread.join("\n")
    );
    h.summary(&format!("random chain workload {params:?}"));
    h.coverage
}

#[test]
fn random_validator_and_forced_workloads_are_verified_chunk_by_chunk() {
    use proptest::test_runner::{Config, RngSeed, TestRunner};
    let mut runner = TestRunner::new(Config {
        cases: 32,
        failure_persistence: None,
        // Deterministic, as AGENTS.md asks of every RNG.
        rng_seed: RngSeed::Fixed(0x5eed_0013),
        ..Config::default()
    });
    let coverage = std::cell::RefCell::new(BTreeSet::new());
    let strategy = (
        chain_params(),
        proptest::collection::vec(proptest::collection::vec(chain_op(), 0..7), 3..10),
    );
    runner
        .run(&strategy, |(params, pulses)| {
            let covered = run_chain_workload(&params, &pulses);
            coverage.borrow_mut().extend(covered);
            Ok(())
        })
        .unwrap();
    let coverage = coverage.into_inner();
    let expected = [
        "RegisterValidator/ok",
        "RegisterValidator/failed",
        "Unstake/ok",
        "Unstake/failed",
        "WithdrawStake/ok",
        "WithdrawStake/failed",
        "ClaimRewards/ok",
        "ClaimRewards/failed",
        "ReportEquivocation/ok",
        "ReportEquivocation/failed",
        "due/ForcedDeadline",
        "due/EpochEnd",
        "deferred-event/ForcedPosted",
        "deferred-event/ForcedDismissed",
        "deferred-event/ForcedResolved",
        "deferred-event/ForcedOverdue",
        "deferred-event/ShardRotated",
        "deferred-event/EpochChanged",
        "deferred/Fee",
        "deferred/Treasury",
        "epoch/rewards-from-fees",
        "epoch/fees-paid-out",
        "epoch/non-fee-income-kept",
    ];
    let missing: Vec<&str> = expected
        .iter()
        .copied()
        .filter(|key| !coverage.contains(*key))
        .collect();
    assert!(
        missing.is_empty(),
        "random workloads never exercised {missing:?}; covered {coverage:?}"
    );
    eprintln!("random chain workloads covered: {coverage:?}");
}

// ---------------------------------------------------------------------------
// Negative tests (v1.3): tampered intents, beacons, witnesses and evidence
// ---------------------------------------------------------------------------

/// Honest v1.3 traces to tamper with.
struct V13Traces {
    h: Harness,
    /// A deferred phase posting two admissible intents and a faucet intent.
    posting: ChunkTrace,
    /// The intents it posted, in order.
    intents: Vec<SignedTransaction>,
    /// A deferred phase where both intents are overdue (rotating both
    /// shards) and the epoch ends.
    rotation: ChunkTrace,
    /// A transaction chunk whose only transaction slashes a validator.
    report: ChunkTrace,
}

fn v13_honest_traces() -> V13Traces {
    let mut h = Harness::with_params(
        &["alice", "bob", "carol"],
        50_000,
        8,
        2,
        validator_params(4),
        forced_params(2, 8),
    );
    // Intents from both shards: alice's, and a new account's on the other.
    let other = wallet_name_on(&h, "other", h.shard_of("alice"), false);
    h.chain.add_wallet(&other);
    // Pulse 1: funds.
    h.fund(&[(&other, usd(), units(500))]);
    // Pulse 2: two admissible intents and a faucet intent.
    let alice = transfer(&mut h, "alice", "carol", units(3));
    let bob = transfer(&mut h, &other, "carol", units(4));
    let faucet = {
        let grants = vec![(h.id("bob"), usd(), units(1))];
        let wallet = h.chain.wallets.get_mut(FAUCET_NAME).expect("faucet");
        wallet
            .faucet(&h.chain.state, grants)
            .expect("a faucet intent")
    };
    h.settle();
    let intents = vec![alice, bob, faucet];
    let out = h.pulse_forced(Vec::new(), intents.clone());
    assert_eq!(count_events(&out, "ForcedPosted"), 2);
    let posting = h
        .chain
        .history
        .last()
        .unwrap()
        .traces
        .last()
        .unwrap()
        .clone();
    // Pulse 3: fees; Pulse 4: both deadlines, both shards rotate, the epoch
    // ends.
    let txs = fee_traffic(&mut h, "carol", "bob", 2);
    h.pulse(txs);
    let out = h.pulse(Vec::new());
    assert_eq!(count_events(&out, "ShardRotated"), 2);
    assert_eq!(count_events(&out, "EpochChanged"), 1);
    let rotation = h
        .chain
        .history
        .last()
        .unwrap()
        .traces
        .last()
        .unwrap()
        .clone();
    // Pulse 5: carol reports genesis validator 0.
    let auth = KeyAuth::of(&h.chain.validator_keys[0].clone());
    let report = h.tx(
        "carol",
        report_headers(
            signed_header(&auth, 2, 0, b"m"),
            signed_header(&auth, 2, 0, b"n"),
        ),
    );
    let out = h.pulse(vec![report.clone()]);
    expect(&out, &report, Outcome::Success);
    let report = h.chain.history.last().unwrap().traces[0].clone();
    assert_eq!(report.transactions.len(), 1);
    V13Traces {
        h,
        posting,
        intents,
        rotation,
        report,
    }
}

/// Fits `S.steps` to the trace's transactions (or posted intents).
fn refit_steps(trace: &mut ChunkTrace) {
    trace.statement.steps = sion_spec::commit::steps_digest(&trace.transactions);
}

#[test]
fn tampered_posted_intents_are_rejected() {
    let V13Traces {
        h,
        posting,
        intents,
        ..
    } = v13_honest_traces();
    accept(&posting);
    assert_eq!(posting.transactions, intents);
    assert_eq!(
        posting.statement.steps,
        sion_spec::commit::steps_digest(&intents),
        "the deferred phase binds the posted intents"
    );

    // Any change to the list breaks the steps.
    let mut dropped = posting.clone();
    dropped.transactions.remove(0);
    assert_eq!(
        rejected(&dropped, "a dropped intent"),
        SpecError::StepsMismatch
    );
    // With the steps refitted, the obligations differ.
    refit_steps(&mut dropped);
    assert!(matches!(
        rejected(&dropped, "a dropped, refitted intent"),
        SpecError::StateRootMismatch { .. }
    ));
    let mut none = posting.clone();
    none.transactions.clear();
    refit_steps(&mut none);
    assert!(matches!(
        rejected(&none, "no intents"),
        SpecError::StateRootMismatch { .. }
    ));
    let mut altered = posting.clone();
    altered.transactions[0] = edited(&h, "alice", &intents[0], |body| body.nonce += 1);
    refit_steps(&mut altered);
    assert!(matches!(
        rejected(&altered, "an altered, re-signed intent"),
        SpecError::StateRootMismatch { .. }
    ));
    let mut forged = posting.clone();
    forged.transactions[0].tx.nonce += 1;
    refit_steps(&mut forged);
    assert!(matches!(
        rejected(&forged, "an intent whose signature no longer verifies"),
        SpecError::StateRootMismatch { .. }
    ));
    let mut extra = posting.clone();
    extra
        .transactions
        .push(edited(&h, "alice", &intents[0], |body| body.nonce += 2));
    refit_steps(&mut extra);
    assert!(matches!(
        rejected(&extra, "an extra admissible intent"),
        SpecError::StateRootMismatch { .. }
    ));

    // The list itself is bound only by the steps (§8.5) and, through the
    // World Proof, by the published intents (§9.5): with the steps
    // refitted, an order that changes nothing, or an extra intent dismissed
    // before anything is read, executes to the same state.
    let mut reordered = posting.clone();
    reordered.transactions.swap(0, 1);
    assert_eq!(
        rejected(&reordered, "reordered intents"),
        SpecError::StepsMismatch
    );
    refit_steps(&mut reordered);
    accept(&reordered);
    let mut padded = posting.clone();
    let duplicate_faucet = edited(&h, FAUCET_NAME, &intents[2], |body| body.nonce += 1);
    padded.transactions.push(duplicate_faucet);
    refit_steps(&mut padded);
    accept(&padded);
}

#[test]
fn tampered_beacons_change_rotations_and_epochs() {
    let V13Traces {
        posting, rotation, ..
    } = v13_honest_traces();
    accept(&rotation);
    let mut tampered = rotation.clone();
    flip(&mut tampered.statement.beacon);
    assert!(matches!(
        rejected(&tampered, "another beacon for rotations and the epoch seed"),
        SpecError::StateRootMismatch { .. }
    ));
    // Where nothing draws on the beacon, the chunk cannot tell (§8: the
    // World Proof binds the beacon).
    let mut unused = posting.clone();
    flip(&mut unused.statement.beacon);
    accept(&unused);
}

#[test]
fn forged_validator_and_obligation_witnesses_are_rejected() {
    let V13Traces {
        h,
        posting,
        rotation,
        intents,
        ..
    } = v13_honest_traces();
    let position = |trace: &ChunkTrace, id: &ObjectId| {
        trace
            .witness
            .objects
            .iter()
            .position(|(entry, _, _)| entry == id)
            .unwrap_or_else(|| panic!("{id} is witnessed"))
    };

    // Altered protocol objects fail their proofs.
    let mut richer = rotation.clone();
    let at = position(&richer, &well_known::validators());
    if let Some(Object {
        data: ObjectData::Validators(set),
        ..
    }) = &mut richer.witness.objects[at].1
    {
        let record = set.validators.values_mut().next().expect("a validator");
        record.stake = units(1);
    }
    assert!(matches!(
        rejected(&richer, "a forged validator set"),
        SpecError::WitnessProof { .. }
    ));
    let mut forgiven = rotation.clone();
    let at = position(&forgiven, &well_known::forced());
    if let Some(Object {
        data: ObjectData::Forced(obligations),
        ..
    }) = &mut forgiven.witness.objects[at].1
    {
        obligations.pending.clear();
    }
    assert!(matches!(
        rejected(&forgiven, "forged obligations"),
        SpecError::WitnessProof { .. }
    ));

    // Every object the new steps read must be witnessed.
    let gas = intents[0].tx.inputs[0].id;
    let account = well_known::account(&h.id("alice"));
    for (trace, id, what) in [
        (&rotation, well_known::validators(), "the validator set"),
        (&rotation, well_known::forced(), "the obligations"),
        (
            &rotation,
            well_known::treasury(),
            "the treasury at the epoch's end",
        ),
        (&rotation, gas, "an overdue intent's input"),
        (&rotation, account, "an overdue intent's sender"),
        (
            &posting,
            well_known::forced(),
            "the obligations when posting",
        ),
        (&posting, gas, "a posted intent's input"),
        (&posting, account, "a posted intent's sender"),
    ] {
        let mut dropped = trace.clone();
        let at = position(&dropped, &id);
        dropped.witness.objects.remove(at);
        assert_eq!(
            rejected(&dropped, &format!("without {what}")),
            SpecError::Unwitnessed(id),
            "{what}"
        );
    }
}

#[test]
fn altered_equivocation_reports_are_rejected() {
    let V13Traces { h, report, .. } = v13_honest_traces();
    accept(&report);
    let signed = report.transactions[0].clone();
    let TxKind::ReportEquivocation { evidence } = &signed.tx.kind else {
        panic!("a report");
    };
    let Equivocation::Headers { first, second } = (**evidence).clone() else {
        panic!("header evidence");
    };

    // The same evidence, swapped: still valid, but another transaction.
    let mut swapped = report.clone();
    swapped.transactions[0] = edited(&h, "carol", &signed, |body| {
        body.kind = report_headers(second.clone(), first.clone());
    });
    assert_eq!(
        rejected(&swapped, "swapped evidence"),
        SpecError::StepsMismatch
    );
    refit_steps(&mut swapped);
    // The reporter's reward coins get identifiers derived from the new
    // digest, which the witness does not cover.
    assert!(matches!(
        rejected(&swapped, "swapped, refitted evidence"),
        SpecError::UncoveredWrite { .. } | SpecError::EffectsMismatch
    ));

    // Evidence that is no longer evidence: the report fails instead.
    let mut same = report.clone();
    same.transactions[0] = edited(&h, "carol", &signed, |body| {
        body.kind = report_headers(first.clone(), first.clone());
    });
    refit_steps(&mut same);
    rejected(&same, "the same header twice");

    // A corrupted signature inside the evidence (the report re-signed).
    let mut corrupted = report.clone();
    corrupted.transactions[0] = edited(&h, "carol", &signed, |body| {
        let mut broken = second.clone();
        broken.signature[10] ^= 1;
        body.kind = report_headers(first.clone(), broken);
    });
    refit_steps(&mut corrupted);
    rejected(&corrupted, "a corrupted evidence signature");
}

// ---------------------------------------------------------------------------
// Edge-case probes (v1.3)
// ---------------------------------------------------------------------------

fn treasury_balances(h: &Harness) -> BTreeMap<AssetId, Amount> {
    match &h.chain.state.objects[&well_known::treasury()].data {
        ObjectData::Treasury(treasury) => treasury.balances.clone(),
        other => panic!("the treasury object holds {other:?}"),
    }
}

/// §6.3 EpochEnd step 5, `balances[USD] −= paid`, when the treasury never
/// held USD: the entry is written (as 0) even though nothing is paid.
#[test]
fn probe_epoch_end_writes_the_usd_entry_even_when_nothing_is_paid() {
    let mut h = Harness::with_params(
        &["alice"],
        1_000,
        2,
        1,
        validator_params(1),
        forced_params(2, 4),
    );
    assert_eq!(treasury_balances(&h).get(&usd()), None);
    // Pulse 1 carries no fee and is epoch 0's last.
    let out = h.pulse(Vec::new());
    assert!(all_events(&out).contains(&Event::EpochChanged {
        epoch: 1,
        active: 4,
        rewards: Amount::ZERO
    }));
    assert_eq!(
        treasury_balances(&h).get(&usd()),
        Some(&Amount::ZERO),
        "stack A writes balances[USD] = 0"
    );
    // Epochs of one Pulse, with and without fees.
    let txs = fee_traffic(&mut h, "alice", "alice", 2);
    h.pulse(txs);
    h.advance(3);
    assert_eq!(h.validator_set().epoch, 5);
    h.assert_minimal_witnesses();
}

/// §6.3 posting: `ctx.pulse < tx.expires_at` is strict, and the deadline is
/// `min(ctx.pulse + window, expires_at)`; an intent that can still execute
/// at its deadline is overdue even in its last executable Pulse.
#[test]
fn probe_posting_expiry_and_deadline_boundaries() {
    let mut h = Harness::with_params(
        &["alice", "bob"],
        50_000,
        8,
        1,
        validator_params(50),
        forced_params(4, 8),
    );
    let p = h.pulse_number() + 1;
    let base_a = transfer(&mut h, "alice", "bob", units(1));
    let base_b = transfer(&mut h, "bob", "alice", units(1));
    let now = edited(&h, "alice", &base_a, |body| body.expires_at = Pulse(p));
    let next = edited(&h, "bob", &base_b, |body| body.expires_at = Pulse(p + 1));
    h.settle();
    let out = h.pulse_forced(Vec::new(), vec![now.clone(), next.clone()]);
    assert_eq!(dismissed_events(&out), [now.digest()].into());
    assert_eq!(posted_events(&out), [next.digest()].into());
    assert_eq!(
        h.obligations().pending[&next.digest()].deadline,
        Pulse(p + 1)
    );
    let out = h.pulse(Vec::new());
    assert!(all_events(&out).contains(&Event::ForcedOverdue {
        intent: next.digest(),
        shard: h.shard_of("bob")
    }));
    assert_eq!(count_events(&out, "ShardRotated"), 1);
    assert!(
        h.obligations().pending.is_empty(),
        "it ends with its expiry"
    );

    // Executed in its deadline Pulse (transactions run before the deferred
    // phase): resolved.
    let p = h.pulse_number() + 1;
    let base = transfer(&mut h, "alice", "bob", units(2));
    let soon = edited(&h, "alice", &base, |body| body.expires_at = Pulse(p + 1));
    h.settle();
    h.pulse_forced(Vec::new(), vec![soon.clone()]);
    let out = h.pulse(vec![soon.clone()]);
    expect(&out, &soon, Outcome::Success);
    assert!(all_events(&out).contains(&Event::ForcedResolved {
        intent: soon.digest()
    }));
    h.assert_minimal_witnesses();
}

/// §6.3: intents are posted before due events, so an obligation resolved
/// in this Pulse still counts toward `max_pending`.
#[test]
fn probe_max_pending_counts_obligations_due_in_the_same_pulse() {
    let mut h = Harness::with_params(
        &["alice", "bob", "carol"],
        50_000,
        8,
        1,
        validator_params(50),
        forced_params(2, 1),
    );
    let first = transfer(&mut h, "alice", "bob", units(1));
    h.settle();
    let out = h.pulse_forced(Vec::new(), vec![first.clone()]);
    assert_eq!(posted_events(&out), [first.digest()].into());
    let second = transfer(&mut h, "bob", "carol", units(1));
    let out = h.pulse_forced(vec![first.clone()], vec![second.clone()]);
    expect(&out, &first, Outcome::Success);
    assert_eq!(dismissed_events(&out), [second.digest()].into());
    let third = transfer(&mut h, "carol", "alice", units(1));
    let out = h.pulse_forced(Vec::new(), vec![third.clone()]);
    assert_eq!(dismissed_events(&out), [third.digest()].into());
    assert!(all_events(&out).contains(&Event::ForcedResolved {
        intent: first.digest()
    }));
    let fourth = transfer(&mut h, "carol", "alice", units(2));
    let out = h.pulse_forced(Vec::new(), vec![fourth.clone()]);
    assert_eq!(posted_events(&out), [fourth.digest()].into());
    h.assert_minimal_witnesses();
}

/// §6.3 Rotate: at most once per window per shard; a later rotation
/// replaces the members now serving (rotation 1's), not the sampled ones.
#[test]
fn probe_rotation_window_and_previous_members() {
    let mut h = Harness::with_params(
        &["alice", "bob", "carol", "dave"],
        50_000,
        8,
        2,
        validator_params(100),
        forced_params(3, 8),
    );
    let shard = h.shard_of("alice");
    let same: Vec<&str> = ["bob", "carol", "dave"]
        .into_iter()
        .filter(|name| h.shard_of(name) == shard)
        .collect();
    assert!(same.len() >= 2, "three accounts on one shard");
    let original = sampled_members(&h.validator_set(), shard);
    let i1 = transfer(&mut h, "alice", "bob", units(1));
    h.settle();
    h.pulse_forced(Vec::new(), vec![i1.clone()]); // Pulse 1, deadline 4
    let i2 = transfer(&mut h, same[0], "alice", units(1));
    h.settle();
    h.pulse_forced(Vec::new(), vec![i2.clone()]); // Pulse 2, deadline 5
    h.pulse(Vec::new()); // Pulse 3
    let i3 = transfer(&mut h, same[1], "alice", units(1));
    h.settle();
    let out = h.pulse_forced(Vec::new(), vec![i3.clone()]); // Pulse 4, deadline 7
    assert!(all_events(&out).contains(&Event::ShardRotated { shard, rotation: 1 }));
    let first: BTreeSet<ValidatorId> = h.validator_set().rotations[&shard]
        .members
        .iter()
        .copied()
        .collect();
    assert!(first.is_disjoint(&original));
    let out = h.pulse(Vec::new()); // Pulse 5: i2 overdue within the window
    assert!(all_events(&out).contains(&Event::ForcedOverdue {
        intent: i2.digest(),
        shard
    }));
    assert_eq!(count_events(&out, "ShardRotated"), 0);
    h.pulse(Vec::new()); // Pulse 6
    let out = h.pulse(Vec::new()); // Pulse 7: i1 and i3 overdue; one rotation
    assert_eq!(count_events(&out, "ForcedOverdue"), 2);
    assert_eq!(count_events(&out, "ShardRotated"), 1);
    let rotation = h.validator_set().rotations[&shard].clone();
    assert_eq!((rotation.count, rotation.at), (2, Pulse(7)));
    let second: BTreeSet<ValidatorId> = rotation.members.iter().copied().collect();
    assert!(
        second.is_disjoint(&first),
        "replaces the members now serving"
    );
    assert_eq!(second, original);
    h.advance(6);
    h.assert_minimal_witnesses();
}

/// §5.27 (`staying` counts joining validators), §6.3 EpochEnd step 3
/// (v1.5: slashed validators lose their seats even if nobody else is
/// eligible, and the chain halts), rotations drawing from fewer validators
/// than seats, and a halted chain rotating no shard.
#[test]
fn probe_last_validator_cannot_leave_and_slashed_validators_lose_their_seats() {
    let mut h = Harness::with_params(
        &["alice", "bob", "carol"],
        50_000,
        8,
        2,
        validator_params(4),
        forced_params(2, 8),
    );
    let genesis = h.genesis_validators();
    let (k1, k2) = (pool_keys(3), pool_keys(4));
    let (v1, v2) = (key_id(k1), key_id(k2));
    let join = {
        let kind = registration(&h, "alice", k1, units(2_000));
        h.tx("alice", kind)
    };
    let out = h.pulse(vec![join.clone()]);
    expect(&out, &join, Outcome::Success);
    h.advance_to(4);
    assert_eq!(h.record(&v1).status, ValidatorStatus::Active);
    // Pulse 5: every genesis validator is slashed.
    let reports: Vec<SignedTransaction> = genesis
        .iter()
        .enumerate()
        .map(|(n, (keys, _))| {
            let auth = KeyAuth::of(keys);
            h.tx(
                "bob",
                report_headers(
                    signed_header(&auth, n as u64, 0, b"1"),
                    signed_header(&auth, n as u64, 0, b"2"),
                ),
            )
        })
        .collect();
    let out = h.pulse(reports.clone());
    for report in &reports {
        expect(&out, report, Outcome::Success);
    }
    // Pulse 6: v1 is the last validator active or joining.
    let leave = h.tx("alice", TxKind::Unstake { validator: v1 });
    let out = h.pulse(vec![leave.clone()]);
    expect(&out, &leave, Outcome::Failed);
    // Pulse 7: with carol's validator joining, v1 may leave.
    let join = {
        let kind = registration(&h, "carol", k2, units(1_000));
        h.tx("carol", kind)
    };
    let leave = h.tx("alice", TxKind::Unstake { validator: v1 });
    let out = h.pulse(vec![join.clone(), leave.clone()]);
    expect(&out, &join, Outcome::Success);
    expect(&out, &leave, Outcome::Success);
    // Pulse 8: the epoch ends; v2 is the whole active set.
    h.advance_to(8);
    assert_eq!(
        h.validator_set().active,
        vec![(v2, sion_exec::validators::weight(units(1_000)))]
    );
    // Pulse 9: an intent is posted; Pulse 10: v2 is slashed; Pulse 11: the
    // intent is overdue and the shard rotates with one validator for two
    // seats; Pulse 12: no record is active, so the next active set is empty
    // and the chain halts.
    let intent = transfer(&mut h, "alice", "bob", units(1));
    h.settle();
    h.pulse_forced(Vec::new(), vec![intent.clone()]);
    let auth = KeyAuth::of(k2);
    let report = h.tx(
        "bob",
        report_headers(
            signed_header(&auth, 1, 1, b"3"),
            signed_header(&auth, 1, 1, b"4"),
        ),
    );
    let out = h.pulse(vec![report.clone()]);
    expect(&out, &report, Outcome::Success);
    let out = h.pulse(Vec::new());
    assert_eq!(count_events(&out, "ShardRotated"), 1);
    let shard = h.shard_of("alice");
    assert_eq!(h.validator_set().rotations[&shard].members, vec![v2]);
    let out = h.pulse(Vec::new());
    assert_eq!(count_events(&out, "EpochChanged"), 1);
    assert!(
        h.validator_set().active.is_empty(),
        "slashed validators lose their seats"
    );
    // Pulse 13: the intent is overdue again, and a halted chain has no
    // validator to rotate in.
    let out = h.pulse(Vec::new());
    assert_eq!(count_events(&out, "ForcedOverdue"), 1);
    assert_eq!(count_events(&out, "ShardRotated"), 0);
    h.advance(4);
    h.assert_minimal_witnesses();
}

/// §6.3 EpochEnd step 3: the largest stakes, ties broken by identifier,
/// at most `max_active`.
#[test]
fn probe_active_set_ranking_ties_and_cap() {
    let mut params = validator_params(3);
    params.max_active = 4;
    let mut h = Harness::with_params(
        &["alice", "bob"],
        400_000,
        8,
        2,
        params,
        forced_params(2, 8),
    );
    let genesis = h.genesis_validators();
    let stake = h.record(&genesis[0].1).stake;
    let (tie, top) = (pool_keys(5), pool_keys(6));
    let txs = vec![
        {
            let kind = registration(&h, "alice", tie, stake);
            h.tx("alice", kind)
        },
        {
            let kind = registration(&h, "bob", top, stake.checked_add(units(1)).unwrap());
            h.tx("bob", kind)
        },
    ];
    let out = h.pulse(txs.clone());
    for tx in &txs {
        expect(&out, tx, Outcome::Success);
    }
    h.advance_to(3);
    let set = h.validator_set();
    let mut tied: Vec<ValidatorId> = genesis.iter().map(|(_, id)| *id).collect();
    tied.push(key_id(tie));
    tied.sort();
    let mut expected: Vec<ValidatorId> = tied[..3].to_vec();
    expected.push(key_id(top));
    expected.sort();
    assert_eq!(
        set.active.iter().map(|(v, _)| *v).collect::<Vec<_>>(),
        expected,
        "the tied validator with the largest identifier is left out"
    );
    assert!(set.active.iter().all(|(_, w)| *w >= 100_000));
    h.advance(4);
    h.assert_minimal_witnesses();
}

/// §5.30 step 4 and §6.3 EpochEnd step 1 at the extremes of the shares.
#[test]
fn probe_reporter_and_reward_share_extremes() {
    for (reporter_permille, reward_permille) in [(1000, 1000), (0, 0), (1, 999)] {
        let mut params = validator_params(3);
        params.reporter_permille = reporter_permille;
        params.reward_permille = reward_permille;
        let mut h =
            Harness::with_params(&["alice", "bob"], 50_000, 8, 2, params, forced_params(2, 8));
        let genesis = h.genesis_validators();
        for _ in 0..3 {
            let txs = fee_traffic(&mut h, "alice", "bob", 2);
            h.pulse(txs);
        }
        let record = h.record(&genesis[0].1);
        assert_eq!(record.rewards == Amount::ZERO, reward_permille == 0);
        let auth = KeyAuth::of(&genesis[0].0);
        let report = h.tx(
            "bob",
            report_headers(
                signed_header(&auth, 4, 0, b"e"),
                signed_header(&auth, 4, 0, b"f"),
            ),
        );
        let out = h.pulse(vec![report.clone()]);
        expect(&out, &report, Outcome::Success);
        let effects = out
            .effects
            .iter()
            .find(|e| e.digest == report.digest())
            .unwrap();
        let count = |fee: bool| {
            effects
                .deferred
                .iter()
                .filter(|item| match item {
                    Deferred::Fee { .. } => fee,
                    Deferred::Treasury { .. } => !fee,
                    _ => false,
                })
                .count()
        };
        let paid = |amount: Amount| sion_exec::validators::permille_of(amount, reporter_permille);
        let rests = [record.stake, record.rewards]
            .into_iter()
            .filter(|amount| amount.0 > paid(*amount).0)
            .count();
        assert_eq!(
            count(false),
            rests,
            "every positive rest is a treasury credit"
        );
        assert_eq!(count(true), 1, "the fee is a fee");
        h.advance(4);
        h.assert_minimal_witnesses();
    }
}

/// §5.30 Approvals: `ctx.pulse` at most both expiries.
#[test]
fn probe_approvals_count_until_their_expiry() {
    let mut h = Harness::with_params(
        &["alice", "bob"],
        50_000,
        8,
        1,
        validator_params(50),
        forced_params(2, 8),
    );
    let genesis = h.genesis_validators();
    h.pulse(Vec::new());
    let (one, two) = double_spend(&mut h, "alice");
    let p = h.pulse_number() + 1;
    let with_expiry = |tx: &Transaction, at: u64| {
        let mut tx = tx.clone();
        tx.expires_at = Pulse(at);
        tx
    };
    let auth = |n: usize| KeyAuth::of(&genesis[n].0);
    let live = h.tx(
        "bob",
        report_approvals(
            approval(&auth(0), &with_expiry(&one, p)),
            approval(&auth(0), &with_expiry(&two, p)),
        ),
    );
    let dead = h.tx(
        "bob",
        report_approvals(
            approval(&auth(1), &with_expiry(&one, p - 1)),
            approval(&auth(1), &with_expiry(&two, p - 1)),
        ),
    );
    let mixed = h.tx(
        "alice",
        report_approvals(
            approval(&auth(2), &with_expiry(&one, p)),
            approval(&auth(2), &with_expiry(&two, p - 1)),
        ),
    );
    // The same coin at another version is not a common input.
    let mut shifted = with_expiry(&two, p);
    shifted.inputs[0].version = Version(shifted.inputs[0].version.0 + 1);
    let versions = h.tx(
        "alice",
        report_approvals(
            approval(&auth(3), &with_expiry(&one, p)),
            approval(&auth(3), &shifted),
        ),
    );
    let out = h.pulse(vec![
        live.clone(),
        dead.clone(),
        mixed.clone(),
        versions.clone(),
    ]);
    expect(&out, &live, Outcome::Success);
    expect(&out, &dead, Outcome::Failed);
    expect(&out, &mixed, Outcome::Failed);
    expect(&out, &versions, Outcome::Failed);
    h.assert_minimal_witnesses();
}

/// `name`'s transaction of `kind`, spending a USD gas coin and then
/// `extra`, built by hand (for stakes the wallet cannot fund).
fn by_hand(h: &mut Harness, name: &str, kind: TxKind, extra: Vec<ObjectRef>) -> SignedTransaction {
    let template = transfer(h, name, name, units(1));
    let mut body = template.tx;
    body.kind = kind;
    body.inputs.truncate(1);
    body.inputs.extend(extra);
    body.fee = h
        .chain
        .state
        .config
        .min_fee(body.size())
        .checked_add(Amount(1_000))
        .unwrap();
    h.resign(name, body)
}

/// §5.26–§5.30 with a stake asset other than USD: stake is drawn, returned
/// and forfeited in that asset, the fee still in USD, rewards in USD.
#[test]
fn probe_stake_in_another_asset() {
    let mut params = validator_params(3);
    params.stake_asset = gpu();
    let mut h = Harness::with_params(&["alice", "bob"], 50_000, 8, 2, params, forced_params(2, 8));
    h.fund(&[("alice", gpu(), units(5_000)), ("bob", gpu(), units(5_000))]);
    let (k1, k2) = (pool_keys(1), pool_keys(2));
    let (v1, v2) = (key_id(k1), key_id(k2));
    // The wallet funds stakes in USD only, so these are built by hand: a
    // USD gas coin, then the goods.
    let alice_goods = h.coins("alice", &gpu());
    let bob_goods = h.coins("bob", &gpu());
    let usd_only = {
        let kind = registration(&h, "alice", k1, units(2_000));
        by_hand(&mut h, "alice", kind, Vec::new())
    };
    let a = {
        let kind = registration(&h, "alice", k1, units(2_000));
        by_hand(&mut h, "alice", kind, alice_goods)
    };
    let b = {
        let kind = registration(&h, "bob", k2, units(2_000));
        by_hand(&mut h, "bob", kind, bob_goods)
    };
    let out = h.pulse(vec![usd_only.clone(), a.clone(), b.clone()]);
    expect(&out, &usd_only, Outcome::Failed);
    expect(&out, &a, Outcome::Success);
    expect(&out, &b, Outcome::Success);
    // Pulse 3: bob leaves while joining and withdraws his goods next Pulse;
    // the epoch ends with alice's validator active.
    let leave = h.tx("bob", TxKind::Unstake { validator: v2 });
    let txs = vec![leave.clone(), transfer(&mut h, "alice", "bob", units(1))];
    let out = h.pulse(txs);
    expect(&out, &leave, Outcome::Success);
    let before = h.balance("bob", &gpu());
    let withdraw = h.tx("bob", TxKind::WithdrawStake { validator: v2 });
    let out = h.pulse(vec![withdraw.clone()]);
    expect(&out, &withdraw, Outcome::Success);
    assert_eq!(
        h.balance("bob", &gpu()),
        before.checked_add(units(2_000)).unwrap()
    );
    // Slashing alice's validator pays the reporter in goods and credits the
    // treasury in goods.
    let auth = KeyAuth::of(k1);
    let report = h.tx(
        "bob",
        report_headers(
            signed_header(&auth, 5, 1, b"g"),
            signed_header(&auth, 5, 1, b"h"),
        ),
    );
    let out = h.pulse(vec![report.clone()]);
    expect(&out, &report, Outcome::Success);
    assert!(matches!(
        h.record(&v1).status,
        ValidatorStatus::Slashed { .. }
    ));
    assert!(treasury_balances(&h)
        .get(&gpu())
        .is_some_and(|a| *a > Amount::ZERO));
    h.advance(4);
    h.assert_minimal_witnesses();
}

/// §5.27/§5.28: a joining validator exits at once, so it can withdraw in
/// the same Pulse if the withdrawal is ordered after the unstake.
#[test]
fn probe_unstake_then_withdraw_in_one_pulse() {
    let mut h = Harness::with_params(
        &["alice"],
        50_000,
        8,
        1,
        validator_params(10),
        forced_params(2, 8),
    );
    let k = pool_keys(3);
    let v = key_id(k);
    let join = {
        let kind = registration(&h, "alice", k, units(1_000));
        h.tx("alice", kind)
    };
    h.pulse(vec![join]);
    let leave = h.tx("alice", TxKind::Unstake { validator: v });
    let withdraw = h.tx("alice", TxKind::WithdrawStake { validator: v });
    let out = h.pulse(vec![leave.clone(), withdraw.clone()]);
    expect(&out, &leave, Outcome::Success);
    let order: Vec<TxDigest> = out.effects.iter().map(|e| e.digest).collect();
    let after = order.iter().position(|d| *d == withdraw.digest())
        > order.iter().position(|d| *d == leave.digest());
    let expected = if after {
        Outcome::Success
    } else {
        Outcome::Failed
    };
    expect(&out, &withdraw, expected);
    h.assert_minimal_witnesses();
}

/// §5.30 step 2: leaving and exited validators can still be slashed; a
/// slashed validator neither exits nor withdraws.
#[test]
fn probe_slashing_leaving_and_exited_validators() {
    let mut params = validator_params(3);
    params.unbonding = 5;
    let mut h = Harness::with_params(&["alice", "bob"], 50_000, 8, 2, params, forced_params(2, 8));
    let genesis = h.genesis_validators();
    h.fund(&[
        (FAUCET_NAME, usd(), units(100)),
        (FAUCET_NAME, usd(), units(100)),
    ]);
    // Pulse 2: g1, g2 and g3 leave.
    let leaving: Vec<SignedTransaction> = genesis[1..]
        .iter()
        .map(|(_, validator)| {
            h.tx(
                FAUCET_NAME,
                TxKind::Unstake {
                    validator: *validator,
                },
            )
        })
        .collect();
    let out = h.pulse(leaving.clone());
    for leave in &leaving {
        expect(&out, leave, Outcome::Success);
    }
    // Pulse 3, epoch 0's last: g1 is slashed while leaving; g2 exits.
    let auth1 = KeyAuth::of(&genesis[1].0);
    let report = h.tx(
        "bob",
        report_headers(
            signed_header(&auth1, 1, 0, b"i"),
            signed_header(&auth1, 1, 0, b"j"),
        ),
    );
    let out = h.pulse(vec![report.clone()]);
    expect(&out, &report, Outcome::Success);
    assert_eq!(
        h.record(&genesis[1].1).status,
        ValidatorStatus::Slashed { at: Pulse(3) }
    );
    assert_eq!(
        h.record(&genesis[2].1).status,
        ValidatorStatus::Exited {
            withdrawable_at: Pulse(8)
        }
    );
    // Pulse 4: the exited g2 is slashed too; it can never withdraw.
    let auth2 = KeyAuth::of(&genesis[2].0);
    let report = h.tx(
        "alice",
        report_headers(
            signed_header(&auth2, 1, 0, b"k"),
            signed_header(&auth2, 1, 0, b"l"),
        ),
    );
    let out = h.pulse(vec![report.clone()]);
    expect(&out, &report, Outcome::Success);
    h.advance_to(8);
    let withdraw = h.tx(
        FAUCET_NAME,
        TxKind::WithdrawStake {
            validator: genesis[2].1,
        },
    );
    let honest = h.tx(
        FAUCET_NAME,
        TxKind::WithdrawStake {
            validator: genesis[3].1,
        },
    );
    let out = h.pulse(vec![withdraw.clone(), honest.clone()]);
    expect(&out, &withdraw, Outcome::Failed);
    expect(&out, &honest, Outcome::Success);
    // Pulse 10: a withdrawn validator cannot be slashed.
    let auth3 = KeyAuth::of(&genesis[3].0);
    let report = h.tx(
        "bob",
        report_headers(
            signed_header(&auth3, 1, 0, b"m"),
            signed_header(&auth3, 1, 0, b"n"),
        ),
    );
    let out = h.pulse(vec![report.clone()]);
    expect(&out, &report, Outcome::Failed);
    h.assert_minimal_witnesses();
}

/// §6.3: an intent may spend its sender's account object; it stays
/// executable until the account's version or rotations change.
#[test]
fn probe_intent_spending_its_senders_account_object() {
    let mut h = Harness::with_params(
        &["alice", "bob"],
        50_000,
        8,
        1,
        validator_params(50),
        forced_params(2, 8),
    );
    let account = well_known::account(&h.id("alice"));
    let policy = h.chain.wallet("alice").policy.clone();
    let intent = h.tx(
        "alice",
        TxKind::RotatePolicy {
            policy,
            viewing_key: None,
        },
    );
    assert!(intent.tx.inputs.iter().any(|input| input.id == account));
    h.settle();
    let out = h.pulse_forced(Vec::new(), vec![intent.clone()]);
    assert_eq!(posted_events(&out), [intent.digest()].into());
    h.pulse(Vec::new());
    let out = h.pulse(Vec::new());
    assert!(all_events(&out).contains(&Event::ForcedOverdue {
        intent: intent.digest(),
        shard: h.shard_of("alice")
    }));
    let out = h.pulse(vec![intent.clone()]);
    expect(&out, &intent, Outcome::Success);
    let out = h.pulse(Vec::new());
    assert!(all_events(&out).contains(&Event::ForcedResolved {
        intent: intent.digest()
    }));
    h.assert_minimal_witnesses();
}

/// Degenerate parameters the specification allows: with a window of 1 a
/// shard may rotate every Pulse, a zero stake weighs one (and forfeits
/// nothing), and with `max_pending` 0 every intent is dismissed. Intents of
/// contended kinds are owed like any other. (A zero window, which §6.3
/// clamps to 1, is refused at genesis.)
#[test]
fn probe_degenerate_forced_and_stake_parameters() {
    let mut params = validator_params(4);
    params.min_stake = Amount::ZERO;
    let mut h = Harness::with_params(&["alice", "bob"], 50_000, 8, 1, params, forced_params(1, 4));
    let shard = h.shard_of("alice");
    let p = h.pulse_number() + 1;
    let intent = transfer(&mut h, "alice", "bob", units(1));
    let claim = h.tx(
        FAUCET_NAME,
        TxKind::ClaimRewards {
            validator: h.genesis_validators()[0].1,
        },
    );
    h.settle();
    let out = h.pulse_forced(Vec::new(), vec![intent.clone(), claim.clone()]);
    assert_eq!(
        posted_events(&out),
        [intent.digest(), claim.digest()].into(),
        "a contended kind is owed by its sender's shard too"
    );
    assert_eq!(
        h.obligations().pending[&intent.digest()].deadline,
        Pulse(p + 1)
    );
    // A validator with nothing staked.
    let k = pool_keys(6);
    let zero = {
        let kind = registration(&h, "bob", k, Amount::ZERO);
        h.tx("bob", kind)
    };
    let out = h.pulse(vec![zero.clone()]);
    expect(&out, &zero, Outcome::Success);
    assert!(all_events(&out).contains(&Event::ShardRotated { shard, rotation: 1 }));
    let out = h.pulse(Vec::new());
    assert!(
        all_events(&out).contains(&Event::ShardRotated { shard, rotation: 2 }),
        "one rotation per Pulse with a window of 1"
    );
    h.advance_to(4);
    let v = key_id(k);
    assert_eq!(
        h.validator_set()
            .active
            .iter()
            .find(|(id, _)| *id == v)
            .map(|(_, weight)| *weight),
        Some(1)
    );
    let auth = KeyAuth::of(k);
    let report = h.tx(
        "alice",
        report_headers(
            signed_header(&auth, 6, 0, b"o"),
            signed_header(&auth, 6, 0, b"p"),
        ),
    );
    let out = h.pulse(vec![report.clone()]);
    expect(&out, &report, Outcome::Success);
    assert!(all_events(&out).contains(&Event::ValidatorSlashed {
        validator: v,
        amount: Amount::ZERO,
        reporter: h.id("alice"),
        reward: Amount::ZERO
    }));
    h.advance(4);
    h.assert_minimal_witnesses();

    // No obligation may be pending.
    let mut h = Harness::with_params(
        &["alice", "bob"],
        50_000,
        8,
        1,
        validator_params(4),
        forced_params(2, 0),
    );
    let intent = transfer(&mut h, "alice", "bob", units(1));
    h.settle();
    let out = h.pulse_forced(Vec::new(), vec![intent.clone()]);
    assert_eq!(dismissed_events(&out), [intent.digest()].into());
    h.assert_minimal_witnesses();
}

// ---------------------------------------------------------------------------
// Spec v1.4: validators are paid from fees only
// ---------------------------------------------------------------------------

/// The rewards an epoch end pays for `fees` collected, by the rule of
/// §6.3 EpochEnd step 1, from the validator set it starts from.
fn rewards_for(set: &ValidatorSet, fees: Amount) -> Amount {
    let pool = sion_exec::validators::permille_of(fees, set.params.reward_permille);
    let total: u64 = set.active.iter().map(|(_, weight)| *weight).sum();
    Amount(
        set.active
            .iter()
            .filter(|(validator, _)| {
                matches!(
                    set.validators[validator].status,
                    ValidatorStatus::Active | ValidatorStatus::Leaving
                )
            })
            .map(|(_, weight)| sion_exec::validators::share_of(pool, *weight, total).0)
            .sum(),
    )
}

fn fees_of(out: &PulseOutput) -> Amount {
    Amount(out.effects.iter().map(|effects| effects.fee.0).sum())
}

fn epoch_rewards(out: &PulseOutput) -> Option<Amount> {
    out.deferred_events.iter().find_map(|event| match event {
        Event::EpochChanged { rewards, .. } => Some(*rewards),
        _ => None,
    })
}

fn treasury_fees(h: &Harness) -> Amount {
    match &h.chain.state.objects[&well_known::treasury()].data {
        ObjectData::Treasury(treasury) => {
            treasury.fees.get(&usd()).copied().unwrap_or(Amount::ZERO)
        }
        other => panic!("the treasury object holds {other:?}"),
    }
}

/// v1.4: a validator slashed during an epoch sends half its stake to the
/// treasury; the epoch's rewards are still the share of its fees alone,
/// and the remainder stays in the treasury.
#[test]
fn probe_rewards_ignore_slashing_remainders() {
    let mut h = Harness::with_params(
        &["alice", "bob"],
        50_000,
        8,
        2,
        validator_params(4),
        forced_params(2, 8),
    );
    let genesis = h.genesis_validators();
    let stake = h.record(&genesis[0].1).stake;
    let txs = fee_traffic(&mut h, "alice", "bob", 2);
    let first = h.pulse(txs);
    let auth = KeyAuth::of(&genesis[0].0);
    let report = h.tx(
        "bob",
        report_headers(
            signed_header(&auth, 1, 0, b"r"),
            signed_header(&auth, 1, 0, b"s"),
        ),
    );
    let second = h.pulse(vec![report.clone()]);
    expect(&second, &report, Outcome::Success);
    let txs = fee_traffic(&mut h, "alice", "bob", 2);
    let third = h.pulse(txs);
    let fees = Amount(fees_of(&first).0 + fees_of(&second).0 + fees_of(&third).0);
    let remainder = Amount(stake.0 - sion_exec::validators::permille_of(stake, 500).0);
    let before = treasury_balances(&h)[&usd()];
    assert_eq!(before, Amount(fees.0 + remainder.0));
    let set = h.validator_set();
    // Pulse 4 ends the epoch: g0's weight still counts, unpaid.
    let out = h.pulse(Vec::new());
    let paid = epoch_rewards(&out).expect("the epoch ended");
    assert_eq!(paid, rewards_for(&set, fees));
    let share = sion_exec::validators::share_of(
        sion_exec::validators::permille_of(fees, 500),
        100_000,
        400_000,
    );
    assert_eq!(paid, Amount(3 * share.0), "three of four equal shares");
    assert_eq!(treasury_balances(&h)[&usd()], Amount(before.0 - paid.0));
    assert!(
        treasury_balances(&h)[&usd()] > remainder,
        "the remainder stays"
    );
    assert_eq!(h.validator_set().fees_mark, fees);
    assert_eq!(treasury_fees(&h), fees);
    h.advance(5);
    h.assert_covered(&["epoch/non-fee-income-kept", "epoch/fees-paid-out"]);
    h.assert_minimal_witnesses();
}

/// v1.4 across epochs: each epoch's rewards are the share of the fees it
/// collected (the mark follows the running total); an epoch whose only fee
/// is a slashing report's pays a share of that fee, not of the forfeited
/// stake; and an epoch without fees pays nothing, however large the
/// treasury.
#[test]
fn probe_fees_across_epochs() {
    let mut params = validator_params(3);
    params.reward_permille = 1000;
    let mut h = Harness::with_params(&["alice", "bob"], 50_000, 8, 2, params, forced_params(2, 8));
    let genesis = h.genesis_validators();
    let mut total = Amount::ZERO;
    for epoch in 0..6u64 {
        let mut collected = Amount::ZERO;
        let mut set = h.validator_set();
        for step in 0..3 {
            let txs = match (epoch, step) {
                // Epoch 2: one slashing report, nothing else.
                (2, 0) => {
                    let auth = KeyAuth::of(&genesis[1].0);
                    vec![h.tx(
                        "alice",
                        report_headers(
                            signed_header(&auth, 9, 1, b"t"),
                            signed_header(&auth, 9, 1, b"u"),
                        ),
                    )]
                }
                (2, _) | (4, _) => Vec::new(),
                _ => fee_traffic(&mut h, "alice", "bob", epoch as usize + 1),
            };
            if step == 2 {
                set = h.validator_set();
            }
            let out = h.pulse(txs);
            collected = Amount(collected.0 + fees_of(&out).0);
            if step == 2 {
                let paid = epoch_rewards(&out).expect("the epoch ended");
                assert_eq!(paid, rewards_for(&set, collected), "epoch {epoch}");
                match epoch {
                    2 => assert!(paid <= collected, "a share of the report's fee"),
                    4 => assert_eq!(paid, Amount::ZERO),
                    _ => assert!(paid > Amount::ZERO),
                }
            }
        }
        total = Amount(total.0 + collected.0);
        assert_eq!(h.validator_set().fees_mark, total, "epoch {epoch}");
        assert_eq!(treasury_fees(&h), total, "epoch {epoch}");
        assert_eq!(h.validator_set().epoch, epoch + 1);
    }
    assert!(
        treasury_balances(&h)[&usd()].0 > 40_000_000_000,
        "the forfeited stake is still in the treasury"
    );
    h.assert_covered(&[
        "epoch/rewards-from-fees",
        "epoch/fees-paid-out",
        "epoch/non-fee-income-kept",
    ]);
    h.assert_minimal_witnesses();
}
