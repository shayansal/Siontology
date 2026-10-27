//! One validator: lane approvals, consensus, Pulse execution, header
//! certificates, data availability and storage, on its own thread.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use fold_assurance::coc::claim_coc;
use fold_assurance::{decide, Policy};
use fold_consensus::lanes::verify_lane_certificate;
use fold_consensus::LaneCertificate;
use fold_consensus::{
    Action, ApprovalProof, ApprovalStatus, Authenticator, Beacon, CertificateBuilder, Committee,
    Core, HashChain, LaneRequest, LaneValidator, Message, SignatureVerifier, TimerKind,
};
use fold_core::hash::Hash32;
use fold_core::ids::{AccountId, AssetId, Pulse, ShardId, TxDigest, ValidatorId};
use fold_core::merkle;
use fold_core::smt::SmtSnapshot;
use fold_core::Amount;
use fold_da::{DaHeader, ExtendedSquare};
use fold_exec::object::well_known;
use fold_exec::pulse::{
    effects_leaf, execute_pulse, execute_pulse_traced, posted_intents, ChunkTrace, PulseInput,
    PulseOutput,
};
use fold_exec::validators::ValidatorSet as StateValidators;
use fold_exec::{GenesisSpec, ObjectData, Owner, SignedTransaction, State};
use fold_light::{
    ChainSpec, ClaimProof, EpochProof, ObjectProof, PulseCertificate, PulseHeader, ReceiptProof,
};
use fold_prover::{PulseData, ValidatorsWitness};
use serde_json::{json, Value};

use crate::committees::{CommitteeView, Committees};
use crate::config::DevnetConfig;
use crate::gateway::GatewayInput;
use crate::keys::{NodeAuthenticator, NodeVerifier};
use crate::owners::{AlreadyChecked, Authorized, StateOwners};
use crate::payload::{expand, lane_inputs, PayloadItem};
use crate::pulses::{ClosedPulse, PulseBuilder};
use crate::storage::{LaneLog, PulseRecord, Snapshot, Wal};
use crate::view::{AccountView, EventView, PulseView, Query, StatusView, TxView};

/// Everything validators share, fixed for a run.
#[derive(Debug)]
pub struct Shared {
    /// Configuration.
    pub config: DevnetConfig,
    /// Genesis.
    pub genesis: GenesisSpec,
    /// Genesis digest (of the state specification).
    pub genesis_digest: Hash32,
    /// What fixes who certifies this chain: genesis, validators, keys, shards.
    pub chain: ChainSpec,
    /// The chain identity ([`ChainSpec::id`]) that headers carry and light
    /// clients pin.
    pub chain_id: Hash32,
    /// How many times this devnet has been restarted (consensus sessions
    /// never repeat across restarts).
    pub generation: u64,
    /// Signature verification for every devnet validator, genesis and
    /// standby (validators also learn keys registered in the state).
    pub verifier: NodeVerifier,
    /// Validator threads in index order (genesis validators, then standby
    /// validators that can join by staking).
    pub validators: Vec<ValidatorId>,
    /// The latest committees and keys, for the gateway and the API.
    pub view: RwLock<CommitteeView>,
}

impl Shared {
    /// The latest committees any validator derived.
    pub fn committees(&self) -> Option<Committees> {
        self.view
            .read()
            .ok()
            .and_then(|view| view.committees.clone())
    }
}

/// Batch headers one certificate group may carry.
const MAX_GROUP_BATCHES: usize = 4_096;

/// The largest validator signature accepted in a certificate group (a dual
/// post-quantum signature is about 19.5 KB).
const MAX_SIGNATURE_BYTES: usize = 32 * 1024;

/// How many Pulses a lane request may wait for this validator to catch up.
const PARK_PULSES: u64 = 4;

/// How many Pulses ahead a header signature may arrive and wait.
const EARLY_SIGNATURE_WINDOW: u64 = 8;

/// Extended data squares kept in memory (for serving shares).
const SQUARE_CACHE: usize = 4;

/// Pulses whose published data is kept for re-deriving shares.
const DA_HISTORY: u64 = 64;

/// Consensus messages for a session this validator has not reached yet,
/// kept until it gets there (at most).
const MAX_FUTURE_MESSAGES: usize = 20_000;

/// Messages to a validator.
#[derive(Debug)]
pub enum Input {
    /// A consensus message from another validator.
    Consensus {
        /// Sender.
        from: ValidatorId,
        /// Message.
        message: Message,
    },
    /// A payload item to propose.
    Submit(Vec<u8>),
    /// A lane approval request.
    Lane {
        /// The shard.
        shard: ShardId,
        /// The request.
        request: LaneRequest,
    },
    /// Another validator's signature on its Pulse header.
    PulseSignature {
        /// Signer.
        from: ValidatorId,
        /// Pulse.
        pulse: Pulse,
        /// Digest of the header it signed.
        header: Hash32,
        /// Signature over the header's signing digest.
        signature: Vec<u8>,
    },
    /// Another validator's approval of a forced intent (validators collect
    /// these themselves, without the gateway).
    ForcedApproval {
        /// The shard.
        shard: ShardId,
        /// The intent.
        tx: TxDigest,
        /// The sealed approval.
        proof: ApprovalProof,
    },
    /// **Adversary simulation (devnet only).** Makes this validator censor
    /// (or stop censoring) these accounts: it ignores their lane requests
    /// and drops their transactions from its proposals.
    Adversary {
        /// Accounts to censor.
        censored: BTreeSet<AccountId>,
        /// Whether this validator takes part.
        censor: bool,
    },
    /// A query.
    Query(Query, Sender<Result<Value, String>>),
    /// Stop.
    Shutdown,
}

/// The in-process network.
#[derive(Clone, Debug)]
pub struct Bus {
    /// Validator inboxes by index.
    pub validators: Vec<Sender<Input>>,
    /// Validator indices.
    pub index: BTreeMap<ValidatorId, usize>,
    /// The gateway's inbox.
    pub gateway: Sender<GatewayInput>,
}

impl Bus {
    /// Sends to one validator (dropped if it has stopped).
    pub fn send(&self, to: &ValidatorId, input: Input) {
        if let Some(index) = self.index.get(to) {
            let _ = self.validators[*index].send(input);
        }
    }

    /// Sends to every validator except `from`.
    pub fn broadcast(&self, from: usize, make: impl Fn() -> Input) {
        for (index, sender) in self.validators.iter().enumerate() {
            if index != from {
                let _ = sender.send(make());
            }
        }
    }
}

/// A job for the prover.
#[derive(Debug)]
pub struct ProverJob {
    /// The Pulse header.
    pub header: PulseHeader,
    /// The Pulse's output.
    pub output: PulseOutput,
    /// Chunk traces.
    pub traces: Vec<ChunkTrace>,
    /// The Pulse's published data.
    pub data: PulseData,
    /// Lane certificates of the executed transactions, in execution order.
    pub certificates: Vec<LaneCertificate>,
    /// The validator set when the Pulse began, with its state proof.
    pub validators: ValidatorsWitness,
}

#[derive(Debug)]
enum Timer {
    /// A consensus timer of a session.
    Core(u64, TimerKind),
    LaneBatch,
}

#[derive(Debug)]
struct Collector {
    header: PulseHeader,
    /// Who signs it: the active set of the header's epoch.
    committee: Committee,
    signatures: BTreeMap<ValidatorId, Vec<u8>>,
    certificate: Option<PulseCertificate>,
}

/// An epoch boundary: its last Pulse, the validator set after it and, once
/// the Pulse is certified, the certificate.
#[derive(Clone, Debug)]
struct Boundary {
    pulse: Pulse,
    validators: ObjectProof,
    certificate: Option<PulseCertificate>,
}

/// The session a consensus message belongs to, if it says.
fn message_session(message: &Message) -> Option<u64> {
    match message {
        Message::Header(signed) => Some(signed.header.epoch.0),
        Message::Certificate(certificate) => Some(certificate.header.epoch.0),
        Message::Vote(_) | Message::SyncRequest(_) => None,
    }
}

/// Makes the beacon's participants the active set: departed validators stop
/// counting, newly active ones reveal from the next Pulse (their registered
/// tip is the first commitment).
fn sync_beacon(beacon: &mut Beacon, set: &StateValidators) {
    let active: BTreeSet<ValidatorId> = set.active.iter().map(|(v, _)| *v).collect();
    for participant in beacon.participants() {
        if !active.contains(&participant) {
            beacon.deregister(&participant);
        }
    }
    for (validator, _) in &set.active {
        if let Some(record) = set.validators.get(validator) {
            beacon.register(*validator, record.beacon_tip);
        }
    }
}

#[derive(Debug)]
struct Executed {
    output: PulseOutput,
    /// The data-availability header (`None` if encoding failed).
    da: Option<DaHeader>,
    /// The published data, from which shares are re-derived on demand (the
    /// extended square itself is large).
    blob: Option<Arc<Vec<u8>>>,
    anchors: u64,
    revealed: usize,
    data_bytes: usize,
    execution_ms: u64,
}

/// The validator's runtime.
pub struct Validator {
    index: usize,
    id: ValidatorId,
    shared: Arc<Shared>,
    auth: NodeAuthenticator,
    core: Core<NodeAuthenticator, NodeVerifier>,
    lanes: BTreeMap<ShardId, LaneValidator<NodeAuthenticator>>,
    lane_waiters: BTreeSet<TxDigest>,
    state: State,
    builder: PulseBuilder,
    chain: HashChain,
    wal: Option<Wal>,
    bus: Bus,
    inbox: Receiver<Input>,
    timers: BinaryHeap<Reverse<(Instant, u64)>>,
    timer_kinds: BTreeMap<u64, Timer>,
    timer_seq: u64,
    history: BTreeMap<Pulse, Executed>,
    tx_index: BTreeMap<TxDigest, (Pulse, u64)>,
    collectors: BTreeMap<Pulse, Collector>,
    early_signatures: BTreeMap<Pulse, Vec<(ValidatorId, Hash32, Vec<u8>)>>,
    certified: Pulse,
    executed: (u64, u64, u64),
    anchors: u64,
    proof_cache: Option<(Pulse, SmtSnapshot)>,
    verifier: NodeVerifier,
    authorized: Authorized,
    squares: VecDeque<(Pulse, ExtendedSquare)>,
    parked: Vec<(ShardId, LaneRequest, u64)>,
    lock_expiry: BTreeMap<TxDigest, (ShardId, Pulse)>,
    lane_log: Option<LaneLog>,
    lane_log_pending: Vec<(ShardId, LaneRequest)>,
    committees: Committees,
    future: Vec<(ValidatorId, Message)>,
    boundaries: BTreeMap<u64, Boundary>,
    boundary_dir: Option<PathBuf>,
    forced_txs: BTreeMap<TxDigest, SignedTransaction>,
    forced_waiters: BTreeSet<TxDigest>,
    forced_builders: BTreeMap<TxDigest, CertificateBuilder>,
    forced_submitted: BTreeSet<TxDigest>,
    censored: BTreeSet<AccountId>,
    censoring: bool,
    prover: Option<std::sync::mpsc::SyncSender<ProverJob>>,
    proven: Arc<std::sync::Mutex<BTreeMap<Pulse, crate::prover::Proven>>>,
}

impl std::fmt::Debug for Validator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Validator")
            .field("index", &self.index)
            .field("id", &self.id.short())
            .field("pulse", &self.state.pulse)
            .finish()
    }
}

/// What a validator starts from.
#[derive(Debug)]
pub struct ValidatorSetup {
    /// Index.
    pub index: usize,
    /// Signing.
    pub auth: NodeAuthenticator,
    /// Recovered or genesis state.
    pub state: State,
    /// Recovered or genesis beacon.
    pub beacon: fold_consensus::Beacon,
    /// Its beacon hash chain.
    pub chain: HashChain,
    /// Its log, if persistent.
    pub wal: Option<Wal>,
    /// Its inbox.
    pub inbox: Receiver<Input>,
    /// The network.
    pub bus: Bus,
    /// Where to send traces, if this validator feeds the prover.
    pub prover: Option<std::sync::mpsc::SyncSender<ProverJob>>,
    /// Verified World Proofs (written by the prover).
    pub proven: Arc<std::sync::Mutex<BTreeMap<Pulse, crate::prover::Proven>>>,
    /// Signed forced intents its state still holds obligations for.
    pub forced_txs: Vec<SignedTransaction>,
    /// Where it keeps epoch-boundary proofs, if persistent.
    pub boundary_dir: Option<PathBuf>,
    /// Its durable lane-lock log, if persistent.
    pub lane_log: Option<LaneLog>,
    /// Locks to restore (approved before a restart, not yet executed).
    pub lane_locks: Vec<(ShardId, LaneRequest)>,
}

impl Validator {
    /// Builds a validator.
    pub fn new(shared: Arc<Shared>, setup: ValidatorSetup) -> Validator {
        let id = setup.auth.validator();
        let mut verifier = shared.verifier.clone();
        verifier.extend_from(setup.state.validators());
        let committees = Committees::derive(
            &setup.state,
            shared.chain_id,
            shared.generation,
            shared.config.shards,
        )
        .expect("a valid state always has committees");
        let core = Core::new(
            committees.consensus.clone(),
            setup.auth.clone(),
            verifier.clone(),
            shared.config.core.clone(),
        );
        let mut lanes: BTreeMap<ShardId, LaneValidator<NodeAuthenticator>> = committees
            .lanes
            .iter()
            .filter(|(_, c)| c.contains(&id))
            .map(|(shard, c)| (*shard, LaneValidator::new(c, setup.auth.clone())))
            .collect();
        let mut lock_expiry = BTreeMap::new();
        for (shard, request) in setup.lane_locks {
            if let Some(lane) = lanes.get_mut(&shard) {
                if let Ok(signed) =
                    fold_core::encode::decode::<fold_exec::SignedTransaction>(&request.owner_proof)
                {
                    lock_expiry.insert(request.tx, (shard, signed.tx.expires_at));
                }
                lane.restore(request);
            }
        }
        // The beacon follows the active set (a restart may come right after
        // an epoch boundary).
        let mut beacon = setup.beacon;
        sync_beacon(&mut beacon, setup.state.validators());
        let mut builder =
            PulseBuilder::new(shared.config.rounds_per_pulse, setup.state.pulse, beacon);
        builder.end_after(setup.state.validators().epoch_end());
        let boundaries = setup
            .boundary_dir
            .as_deref()
            .map(load_boundaries)
            .unwrap_or_default();
        if let Ok(mut view) = shared.view.write() {
            view.update(setup.state.pulse, &committees, &setup.state);
        }
        let forced_txs = setup
            .forced_txs
            .into_iter()
            .map(|tx| (tx.digest(), tx))
            .collect();
        Validator {
            index: setup.index,
            id,
            auth: setup.auth,
            core,
            lanes,
            lane_waiters: BTreeSet::new(),
            state: setup.state,
            builder,
            chain: setup.chain,
            wal: setup.wal,
            bus: setup.bus,
            inbox: setup.inbox,
            timers: BinaryHeap::new(),
            timer_kinds: BTreeMap::new(),
            timer_seq: 0,
            history: BTreeMap::new(),
            tx_index: BTreeMap::new(),
            collectors: BTreeMap::new(),
            early_signatures: BTreeMap::new(),
            certified: Pulse(0),
            executed: (0, 0, 0),
            anchors: 0,
            proof_cache: None,
            verifier,
            authorized: Authorized::default(),
            squares: VecDeque::new(),
            parked: Vec::new(),
            lock_expiry,
            lane_log: setup.lane_log,
            lane_log_pending: Vec::new(),
            committees,
            future: Vec::new(),
            boundaries,
            boundary_dir: setup.boundary_dir,
            forced_txs,
            forced_waiters: BTreeSet::new(),
            forced_builders: BTreeMap::new(),
            forced_submitted: BTreeSet::new(),
            censored: BTreeSet::new(),
            censoring: false,
            prover: setup.prover,
            proven: setup.proven,
            shared,
        }
    }

    fn schedule(&mut self, after: Duration, timer: Timer) {
        let seq = self.timer_seq;
        self.timer_seq += 1;
        self.timers.push(Reverse((Instant::now() + after, seq)));
        self.timer_kinds.insert(seq, timer);
    }

    /// This validator's beacon reveal for `pulse`, if it is a beacon
    /// participant (its hash chain is indexed from its registration).
    fn reveal(&self, pulse: Pulse) -> Option<Hash32> {
        let base = self.builder.beacon().base_of(&self.id)?;
        self.chain.reveal_after(base, pulse)
    }

    /// Proposes this validator's reveal for the next Pulse.
    fn submit_reveal(&mut self) -> Vec<Action> {
        let next = self.builder.next_pulse();
        match self.reveal(next) {
            Some(value) => self.core.handle(fold_consensus::Event::Submit(
                PayloadItem::Reveal {
                    validator: self.id,
                    pulse: next,
                    value,
                }
                .to_bytes(),
            )),
            None => Vec::new(),
        }
    }

    /// Runs until shut down.
    pub fn run(mut self) {
        let mut actions: VecDeque<Action> = self.core.handle(fold_consensus::Event::Start).into();
        actions.extend(self.submit_reveal());
        self.apply(actions);
        self.schedule(
            Duration::from_millis(self.shared.config.batch_interval_ms),
            Timer::LaneBatch,
        );
        loop {
            let timeout = self
                .timers
                .peek()
                .map(|Reverse((at, _))| at.saturating_duration_since(Instant::now()))
                .unwrap_or(Duration::from_millis(100));
            match self.inbox.recv_timeout(timeout) {
                Ok(Input::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(input) => self.handle(input),
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.fire_timers();
        }
        if let Some(wal) = &self.wal {
            let _ = wal.snapshot(&self.snapshot_record());
        }
    }

    fn snapshot_record(&self) -> Snapshot {
        Snapshot {
            state: self.state.clone(),
            beacon: self.builder.beacon().clone(),
            generation: self.shared.generation,
            forced: self.forced_txs.values().cloned().collect(),
        }
    }

    fn fire_timers(&mut self) {
        let now = Instant::now();
        while let Some(Reverse((at, seq))) = self.timers.peek().copied() {
            if at > now {
                break;
            }
            self.timers.pop();
            match self.timer_kinds.remove(&seq) {
                Some(Timer::Core(session, kind)) => {
                    // Timers of an earlier session's DAG are stale.
                    if session == self.committees.session {
                        let actions = self.core.handle(fold_consensus::Event::Timer(kind));
                        self.apply(actions.into());
                    }
                }
                Some(Timer::LaneBatch) => {
                    self.seal_lane_batches();
                    self.schedule(
                        Duration::from_millis(self.shared.config.batch_interval_ms),
                        Timer::LaneBatch,
                    );
                }
                None => {}
            }
        }
    }

    fn handle(&mut self, input: Input) {
        match input {
            Input::Consensus { from, message } => {
                match message_session(&message) {
                    Some(session) if session > self.committees.session => {
                        // From a session this validator reaches at its next
                        // epoch boundary: keep it until then.
                        if self.future.len() < MAX_FUTURE_MESSAGES {
                            self.future.push((from, message));
                        }
                        return;
                    }
                    Some(session) if session < self.committees.session => return,
                    _ => {}
                }
                let actions = self
                    .core
                    .handle(fold_consensus::Event::Message { from, message });
                self.apply(actions.into());
            }
            Input::Submit(item) => {
                if self.censors(&item) {
                    return;
                }
                let actions = self.core.handle(fold_consensus::Event::Submit(item));
                self.apply(actions.into());
            }
            Input::Lane { shard, request } => self.on_lane_request(shard, request),
            Input::ForcedApproval { shard, tx, proof } => {
                let actions = self.on_forced_approval(shard, tx, proof);
                self.apply(actions.into());
            }
            Input::Adversary { censored, censor } => {
                self.censored = censored;
                self.censoring = censor;
            }
            Input::PulseSignature {
                from,
                pulse,
                header,
                signature,
            } => self.on_pulse_signature(from, pulse, header, signature),
            Input::Query(query, reply) => {
                let _ = reply.send(self.answer(query));
            }
            Input::Shutdown => {}
        }
    }

    fn apply(&mut self, mut actions: VecDeque<Action>) {
        while let Some(action) = actions.pop_front() {
            match action {
                Action::Broadcast(message) => {
                    let from = self.id;
                    self.bus.broadcast(self.index, || Input::Consensus {
                        from,
                        message: message.clone(),
                    });
                }
                Action::Send { to, message } => self.bus.send(
                    &to,
                    Input::Consensus {
                        from: self.id,
                        message,
                    },
                ),
                Action::SetTimer { timer, after_ms } => self.schedule(
                    Duration::from_millis(after_ms),
                    Timer::Core(self.committees.session, timer),
                ),
                Action::Commit(commit) => {
                    self.anchors += 1;
                    let session = self.committees.session;
                    for closed in self.builder.on_commit(&commit) {
                        let next = self.execute(closed);
                        if self.committees.session != session {
                            // The epoch's last Pulse executed and a new
                            // consensus session began. What the old session's
                            // core still asked for is stale: its later commits
                            // are proposed again in the new session, and its
                            // timers are dropped. Its messages still go out,
                            // for validators that have not reached the
                            // boundary yet.
                            let mut carried = Vec::new();
                            for action in std::mem::take(&mut actions) {
                                match action {
                                    Action::Commit(late) => carried.extend(late.items),
                                    Action::SetTimer { .. } => {}
                                    other => actions.push_back(other),
                                }
                            }
                            actions.extend(self.propose_again(carried));
                        }
                        actions.extend(next);
                    }
                }
                Action::Evidence(evidence) => {
                    eprintln!(
                        "validator {} detected Byzantine behavior by {:?}",
                        self.index,
                        evidence
                            .offenders()
                            .iter()
                            .map(|v| v.short())
                            .collect::<Vec<_>>()
                    );
                }
            }
        }
    }

    // ----- Lanes ----------------------------------------------------------

    fn on_lane_request(&mut self, shard: ShardId, request: LaneRequest) {
        self.handle_lane_request(shard, request, 0);
    }

    /// Checks a lane request and approves it. A request that fails only
    /// because this validator is behind (an input it has not seen created
    /// yet) waits up to [`PARK_PULSES`] Pulses instead of being refused, so
    /// lagging validators do not split an object's locks.
    fn handle_lane_request(&mut self, shard: ShardId, request: LaneRequest, parked_for: u64) {
        let tx = request.tx;
        let Some(lane) = self.lanes.get_mut(&shard) else {
            let _ = self.bus.gateway.send(GatewayInput::LaneRejected {
                tx,
                validator: self.id,
                reason: "not a member of this shard's committee".into(),
            });
            return;
        };
        let owners = StateOwners { state: &self.state };
        let expires_at = match owners.check(&request) {
            Ok((signed, _)) if self.censoring && self.censored.contains(&signed.tx.sender) => {
                // Censorship is silent: no approval, no rejection.
                return;
            }
            Ok((signed, key)) => {
                self.authorized.insert(key);
                signed.tx.expires_at
            }
            Err(reason) => {
                let behind =
                    fold_core::encode::decode::<fold_exec::SignedTransaction>(&request.owner_proof)
                        .map(|signed| owners.is_behind(&signed))
                        .unwrap_or(false);
                if behind && parked_for < PARK_PULSES {
                    self.parked.push((shard, request, parked_for + 1));
                } else {
                    let _ = self.bus.gateway.send(GatewayInput::LaneRejected {
                        tx,
                        validator: self.id,
                        reason,
                    });
                }
                return;
            }
        };
        self.lock_expiry.insert(tx, (shard, expires_at));
        let logged = request.clone();
        match lane.approve(request, &AlreadyChecked) {
            Ok(ApprovalStatus::Pending) => {
                self.lane_waiters.insert(tx);
                self.lane_log_pending.push((shard, logged));
            }
            Ok(ApprovalStatus::Sealed(proof)) => {
                let _ = self
                    .bus
                    .gateway
                    .send(GatewayInput::Approval { shard, tx, proof });
            }
            Err(rejection) => {
                let _ = self.bus.gateway.send(GatewayInput::LaneRejected {
                    tx,
                    validator: self.id,
                    reason: rejection.error.to_string(),
                });
            }
        }
    }

    /// A consensus-ordered transaction whose lane certificate does not
    /// verify is not executed. Honest gateways never produce one, so it is
    /// worth a line in the log (once, from the first validator).
    fn drop_uncertified(&self, tx: &fold_exec::SignedTransaction, why: &str) {
        if self.index == 0 {
            eprintln!(
                "validator: dropping {} ordered without a valid lane certificate: {why}",
                tx.digest()
            );
        }
    }

    fn seal_lane_batches(&mut self) {
        // Approvals leave this validator only once their locks are durable.
        if let Some(log) = &mut self.lane_log {
            if let Err(e) = log.append(&self.lane_log_pending) {
                eprintln!("validator {}: lane log append failed: {e}", self.index);
                return;
            }
        }
        self.lane_log_pending.clear();
        let mut forced = Vec::new();
        for (shard, lane) in self.lanes.iter_mut() {
            let Some(batch) = lane.seal_batch() else {
                continue;
            };
            for (tx, _) in &batch.approvals {
                if self.lane_waiters.remove(tx) {
                    if let Some(proof) = lane.approval_proof(tx) {
                        let _ = self.bus.gateway.send(GatewayInput::Approval {
                            shard: *shard,
                            tx: *tx,
                            proof: proof.clone(),
                        });
                    }
                }
                if self.forced_waiters.remove(tx) {
                    if let Some(proof) = lane.approval_proof(tx) {
                        forced.push((*shard, *tx, proof.clone()));
                    }
                }
            }
        }
        let mut actions = Vec::new();
        for (shard, tx, proof) in forced {
            actions.extend(self.share_forced_approval(shard, tx, proof));
        }
        self.apply(actions.into());
    }

    // ----- Forced inclusion ------------------------------------------------

    /// Sends this validator's approval of a forced intent to every
    /// validator, itself included.
    fn share_forced_approval(
        &mut self,
        shard: ShardId,
        tx: TxDigest,
        proof: ApprovalProof,
    ) -> Vec<Action> {
        self.bus.broadcast(self.index, || Input::ForcedApproval {
            shard,
            tx,
            proof: proof.clone(),
        });
        self.on_forced_approval(shard, tx, proof)
    }

    /// Approves the forced intents the state says this validator's shards
    /// owe, if they can still execute.
    fn approve_forced(&mut self) -> Vec<Action> {
        let owed: Vec<(TxDigest, ShardId)> = self
            .state
            .forced()
            .pending
            .iter()
            .map(|(digest, o)| (*digest, ShardId(o.shard)))
            .collect();
        let mut sealed = Vec::new();
        for (digest, shard) in owed {
            let Some(tx) = self.forced_txs.get(&digest).cloned() else {
                continue;
            };
            if self.censoring && self.censored.contains(&tx.tx.sender) {
                continue;
            }
            if !self.lanes.contains_key(&shard)
                || (StateOwners { state: &self.state })
                    .admissible(&tx, &mut self.authorized)
                    .is_err()
            {
                continue;
            }
            let Some(lane) = self.lanes.get_mut(&shard) else {
                continue;
            };
            let request = LaneRequest {
                tx: digest,
                inputs: lane_inputs(&tx.tx.inputs),
                owner_proof: fold_core::encode::encode(&tx),
            };
            // Admissible in this state; a lock held by another transaction
            // still refuses.
            self.lock_expiry.insert(digest, (shard, tx.tx.expires_at));
            let logged = request.clone();
            match lane.approve(request, &AlreadyChecked) {
                Ok(ApprovalStatus::Pending) => {
                    self.forced_waiters.insert(digest);
                    self.lane_log_pending.push((shard, logged));
                }
                Ok(ApprovalStatus::Sealed(proof)) => {
                    if !self.forced_submitted.contains(&digest) {
                        sealed.push((shard, digest, proof));
                    }
                }
                Err(_) => {}
            }
        }
        let mut actions = Vec::new();
        for (shard, tx, proof) in sealed {
            actions.extend(self.share_forced_approval(shard, tx, proof));
        }
        actions
    }

    /// Collects approvals of a forced intent; once they form a certificate,
    /// proposes the certified transaction itself. Returns the consensus
    /// actions to apply.
    fn on_forced_approval(
        &mut self,
        shard: ShardId,
        tx: TxDigest,
        proof: ApprovalProof,
    ) -> Vec<Action> {
        if self.forced_submitted.contains(&tx) {
            return Vec::new();
        }
        let Some(signed) = self.forced_txs.get(&tx).cloned() else {
            return Vec::new();
        };
        if self.censoring && self.censored.contains(&signed.tx.sender) {
            return Vec::new();
        }
        let Some(committee) = self.committees.lanes.get(&shard) else {
            return Vec::new();
        };
        let builder = self
            .forced_builders
            .entry(tx)
            .or_insert_with(|| CertificateBuilder::new(tx, &lane_inputs(&signed.tx.inputs)));
        match builder.add(committee, proof, &self.verifier) {
            Ok(Some(certificate)) => {
                self.forced_submitted.insert(tx);
                self.forced_builders.remove(&tx);
                let group = PayloadItem::group(vec![(signed, certificate)]);
                self.core
                    .handle(fold_consensus::Event::Submit(group.to_bytes()))
            }
            _ => Vec::new(),
        }
    }

    // ----- Execution -------------------------------------------------------

    fn execute(&mut self, closed: ClosedPulse) -> Vec<Action> {
        let started = Instant::now();
        let shards = self.shared.config.shards;
        let mut owned = Vec::new();
        let mut contended = Vec::new();
        let mut certificates: BTreeMap<TxDigest, LaneCertificate> = BTreeMap::new();
        let mut posted = Vec::new();
        for item in closed.items {
            let (batches, txs) = match item {
                PayloadItem::Certified { batches, txs } => (batches, txs),
                PayloadItem::Forced { tx } => {
                    posted.push(*tx);
                    continue;
                }
                PayloadItem::Reveal { .. } => continue,
            };
            // Check the group's shape before expanding anything: expansion
            // copies a batch header per approval, so an oversized signature
            // or more approvals than a committee has members must be refused
            // first.
            let seats = self.shared.config.shard_seats.max(1);
            if batches.len() > MAX_GROUP_BATCHES
                || batches
                    .iter()
                    .any(|b| b.signature.len() > MAX_SIGNATURE_BYTES)
                || txs.iter().any(|t| {
                    t.approvals.len() > seats || t.approvals.iter().any(|a| a.path.len() > 64)
                })
            {
                if self.index == 0 {
                    eprintln!("validator: dropping a malformed certificate group");
                }
                continue;
            }
            for grouped in &txs {
                // The certificate must lock exactly the transaction's owned
                // inputs, with a quorum of its shard's committee.
                let tx = &grouped.tx;
                let Some(certificate) = expand(&batches, grouped) else {
                    self.drop_uncertified(tx, "references a batch the group does not carry");
                    continue;
                };
                let shard = ShardId::for_account(&tx.tx.sender, shards);
                let Some(committee) = self.committees.lanes.get(&shard) else {
                    self.drop_uncertified(tx, "no committee for its shard");
                    continue;
                };
                let inputs = lane_inputs(&tx.tx.inputs);
                if let Err(error) = verify_lane_certificate(
                    committee,
                    &tx.digest(),
                    &inputs,
                    &certificate,
                    &self.verifier,
                ) {
                    self.drop_uncertified(tx, &error.to_string());
                    continue;
                }
                if certificates.contains_key(&tx.digest()) {
                    continue;
                }
                certificates.insert(tx.digest(), certificate);
                if tx.tx.domain().is_some() {
                    contended.push(tx.clone());
                } else {
                    owned.push(tx.clone());
                }
            }
        }
        let input = PulseInput {
            pulse: closed.pulse,
            beacon: closed.beacon.value,
            owned,
            contended,
            forced: posted,
        };
        // The validator set the certificates had to come from, for the prover.
        let validators_witness = self
            .prover
            .as_ref()
            .and_then(|_| ValidatorsWitness::of(&self.state));
        let epoch = self.committees.epoch;
        // Signatures checked when the inputs were locked (or now, in
        // parallel) need not be checked again during execution; the result
        // is identical. Proof stacks always check them.
        let verify = !self
            .authorized
            .preverify(&self.state, input.owned.iter().chain(&input.contended));
        let traced = self.prover.is_some();
        let result = match (traced, self.shared.config.prove_chunk) {
            (true, Some(chunk)) => execute_pulse_traced(&mut self.state, &input, verify, chunk),
            _ => execute_pulse(&mut self.state, &input, verify).map(|output| (output, Vec::new())),
        };
        let (output, traces) = match result {
            Ok(result) => result,
            Err(error) => {
                // Inputs are filtered above, so this is a bug; stop rather than diverge.
                panic!(
                    "validator {} cannot execute Pulse {}: {error}",
                    self.index, input.pulse
                );
            }
        };
        let (s, f, r) = output.status_counts();
        self.executed = (
            self.executed.0 + s as u64,
            self.executed.1 + f as u64,
            self.executed.2 + r as u64,
        );

        // Forced inclusion: the state now holds what the shards owe. Keep the
        // signed intents it still holds obligations for.
        let executed_digests: Vec<TxDigest> = output.effects.iter().map(|e| e.digest).collect();
        for tx in &input.forced {
            self.forced_txs
                .entry(tx.digest())
                .or_insert_with(|| tx.clone());
        }
        let owed = &self.state.forced().pending;
        self.forced_txs
            .retain(|digest, _| owed.contains_key(digest));
        let forced_summary = crate::forced::summary(self.state.forced());
        if self.index == 0 {
            for event in &output.deferred_events {
                if let fold_exec::Event::ForcedOverdue { intent, shard } = event {
                    eprintln!(
                        "validator: Pulse {}: forced intent {} overdue in shard {shard}: censorship detected",
                        input.pulse,
                        intent.short()
                    );
                }
                if let fold_exec::Event::ShardRotated { shard, rotation } = event {
                    eprintln!(
                        "validator: Pulse {}: shard {shard}'s committee rotated (rotation {rotation})",
                        input.pulse
                    );
                }
            }
        }

        // Data availability: the transaction bodies in execution order,
        // without signatures.
        let bodies: BTreeMap<TxDigest, &fold_exec::Transaction> = input
            .owned
            .iter()
            .chain(&input.contended)
            .map(|t| (t.digest(), &t.tx))
            .collect();
        let data = PulseData {
            pulse: input.pulse,
            beacon: input.beacon,
            transactions: output
                .order
                .iter()
                .filter_map(|d| bodies.get(d).map(|t| (*t).clone()))
                .collect(),
            forced: posted_intents(&input.forced)
                .into_iter()
                .map(|t| t.tx)
                .collect(),
        };
        let ordered_certificates: Vec<LaneCertificate> = output
            .order
            .iter()
            .filter_map(|d| certificates.get(d).cloned())
            .collect();
        let blob = data.blob();
        let square = data.availability().ok();
        let (data_root, data_k) = square
            .as_ref()
            .map(|(_, h)| (h.data_root, h.k))
            .unwrap_or((Hash32::ZERO, 0));
        let da = square.as_ref().map(|(_, h)| h.clone());
        if let Some((square, _)) = square {
            self.squares.push_back((input.pulse, square));
            while self.squares.len() > SQUARE_CACHE {
                self.squares.pop_front();
            }
        }

        let header = PulseHeader {
            chain: self.shared.chain_id,
            epoch,
            pulse: input.pulse,
            beacon: input.beacon,
            before: output.before,
            after: output.after,
            order_digest: output.order_digest,
            receipts_root: output.receipts_root,
            data_root,
            data_k,
            forced: forced_summary,
        };

        if self.wal.is_some() {
            let record = PulseRecord {
                input: input.clone(),
                after: output.after,
                beacon: self.builder.beacon().clone(),
            };
            let snapshot = (input.pulse.0 % self.shared.config.snapshot_every.max(1) == 0)
                .then(|| self.snapshot_record());
            let Some(wal) = self.wal.as_mut() else {
                unreachable!("checked above")
            };
            if let Err(e) = wal.append(&record) {
                eprintln!("validator {}: log append failed: {e}", self.index);
            }
            if let Some(snapshot) = snapshot {
                if let Err(e) = wal.snapshot(&snapshot) {
                    eprintln!("validator {}: snapshot failed: {e}", self.index);
                }
                // Keep the lane log to the locks that still matter.
                if let Some(log) = &mut self.lane_log {
                    let state = &self.state;
                    let live: Vec<(ShardId, LaneRequest)> = self
                        .lanes
                        .iter()
                        .flat_map(|(shard, lane)| {
                            lane.locked_requests().into_iter().map(move |r| (*shard, r))
                        })
                        .filter(|(_, request)| crate::owners::lock_is_live(state, request))
                        .collect();
                    if let Err(e) = log.compact(&live) {
                        eprintln!("validator {}: lane log compaction failed: {e}", self.index);
                    }
                }
            }
        }

        // A transaction's receipt is its first execution: a certificate
        // re-included later is rejected (its inputs are gone), and that
        // later receipt must not hide the real one.
        for (index, effects) in output.effects.iter().enumerate() {
            self.tx_index
                .entry(effects.digest)
                .or_insert((input.pulse, index as u64));
        }

        // Sign the header and share the signature, but never certify a Pulse
        // whose data has no availability commitment.
        if data_k > 0 {
            let signature = self.auth.sign(&header.signing_digest().0);
            let digest = header.digest();
            let mut collector = Collector {
                header: header.clone(),
                committee: self.committees.signers.clone(),
                signatures: BTreeMap::new(),
                certificate: None,
            };
            collector.signatures.insert(self.id, signature.clone());
            self.collectors.insert(input.pulse, collector);
            let (from, pulse) = (self.id, input.pulse);
            self.bus.broadcast(self.index, || Input::PulseSignature {
                from,
                pulse,
                header: digest,
                signature: signature.clone(),
            });
            for (from, header, signature) in self
                .early_signatures
                .remove(&input.pulse)
                .unwrap_or_default()
            {
                self.on_pulse_signature(from, input.pulse, header, signature);
            }
            self.try_certify(input.pulse);
        } else {
            eprintln!(
                "validator {}: Pulse {} has no data-availability commitment; not signing it",
                self.index, input.pulse
            );
        }

        if let (Some(prover), Some(validators)) = (&self.prover, validators_witness) {
            // A full backlog means the prover is behind: this Pulse goes
            // unproven rather than queueing without bound.
            let _ = prover.try_send(ProverJob {
                header,
                output: output.clone(),
                traces,
                data,
                certificates: ordered_certificates,
                validators,
            });
        }
        if self.index == 0 {
            let _ = self.bus.gateway.send(GatewayInput::Executed {
                pulse: input.pulse,
                digests: executed_digests.clone(),
            });
        }

        self.history.insert(
            input.pulse,
            Executed {
                output,
                da,
                data_bytes: blob.len(),
                blob: Some(Arc::new(blob)),
                anchors: closed.anchors,
                revealed: closed.beacon.revealed.len(),
                execution_ms: started.elapsed().as_millis() as u64,
            },
        );
        if let Some(old) = input
            .pulse
            .0
            .checked_sub(DA_HISTORY)
            .and_then(|p| self.history.get_mut(&Pulse(p)))
        {
            old.blob = None;
        }
        while self.history.len() > self.shared.config.history.max(1) {
            let oldest = *self.history.keys().next().expect("non-empty");
            if let Some(old) = self.history.remove(&oldest) {
                for effects in &old.output.effects {
                    self.tx_index.remove(&effects.digest);
                }
            }
            self.collectors.remove(&oldest);
        }
        self.proof_cache = None;

        // Lane locks of transactions that executed (their input versions are
        // gone) or can no longer execute (expired) are released, so their
        // inputs never stay frozen.
        let pulse_now = self.state.pulse;
        let mut released: Vec<(ShardId, TxDigest)> = executed_digests
            .iter()
            .filter_map(|d| self.lock_expiry.get(d).map(|(s, _)| (*s, *d)))
            .collect();
        released.extend(
            self.lock_expiry
                .iter()
                .filter(|(_, (_, expires_at))| *expires_at <= pulse_now)
                .map(|(d, (s, _))| (*s, *d)),
        );
        for (shard, digest) in released {
            if let Some(lane) = self.lanes.get_mut(&shard) {
                lane.release(&digest);
            }
            self.lock_expiry.remove(&digest);
            self.lane_waiters.remove(&digest);
        }
        // Requests that waited for this validator to catch up get another try.
        for (shard, request, parked_for) in std::mem::take(&mut self.parked) {
            self.handle_lane_request(shard, request, parked_for);
        }

        // Committees follow the state: lane committees may have rotated, and
        // at an epoch boundary consensus starts a new session.
        let mut actions = self.reconfigure();

        // Forced intents still owed: approve those of this validator's
        // shards, and forget bookkeeping for resolved ones.
        let owed = &self.state.forced().pending;
        self.forced_waiters.retain(|d| owed.contains_key(d));
        self.forced_builders.retain(|d, _| owed.contains_key(d));
        self.forced_submitted.retain(|d| owed.contains_key(d));
        actions.extend(self.approve_forced());

        // Reveal for the next Pulse.
        actions.extend(self.submit_reveal());
        actions
    }

    // ----- Committees and epochs ---------------------------------------------

    /// Derives the committees from the state after a Pulse and adopts them:
    /// lane validators for shards whose committee changed are rebuilt
    /// (keeping this validator's locks, so it never approves a conflicting
    /// spend), and at an epoch boundary consensus starts a new session with
    /// the new active set (this validator observes if it is not in it).
    fn reconfigure(&mut self) -> Vec<Action> {
        let mut actions = Vec::new();
        self.verifier.extend_from(self.state.validators());
        let next = match Committees::derive(
            &self.state,
            self.shared.chain_id,
            self.shared.generation,
            self.shared.config.shards,
        ) {
            Ok(next) => next,
            Err(error) => {
                eprintln!("validator {}: no committees: {error}", self.index);
                return actions;
            }
        };
        for (shard, committee) in &next.lanes {
            if self.committees.lanes.get(shard) == Some(committee) {
                continue;
            }
            let old = self.lanes.remove(shard);
            if committee.contains(&self.id) {
                let mut lane = LaneValidator::new(committee, self.auth.clone());
                if let Some(old) = &old {
                    for request in old.locked_requests() {
                        lane.restore(request);
                    }
                }
                self.lanes.insert(*shard, lane);
            }
            // Approvals of forced intents under the old committee no longer
            // count: collect them again.
            let owed = &self.state.forced().pending;
            let stale: Vec<TxDigest> = owed
                .iter()
                .filter(|(_, o)| o.shard == shard.0)
                .map(|(d, _)| *d)
                .collect();
            for digest in stale {
                self.forced_builders.remove(&digest);
                self.forced_submitted.remove(&digest);
                self.forced_waiters.remove(&digest);
            }
        }
        if next.epoch != self.committees.epoch {
            if self.index == 0 {
                eprintln!(
                    "validator: Pulse {}: epoch {} begins with {} validators (session {})",
                    self.state.pulse,
                    next.epoch,
                    next.consensus.members().len(),
                    next.session
                );
            }
            let mut beacon = self.builder.beacon().clone();
            sync_beacon(&mut beacon, self.state.validators());
            let mut builder = self.builder.next_session(self.state.pulse, beacon);
            builder.end_after(self.state.validators().epoch_end());
            let mut old_builder = std::mem::replace(&mut self.builder, builder);
            let mut old_core = std::mem::replace(
                &mut self.core,
                Core::new(
                    next.consensus.clone(),
                    self.auth.clone(),
                    self.verifier.clone(),
                    self.shared.config.core.clone(),
                ),
            );
            self.committees = next.clone();
            actions.extend(self.core.handle(fold_consensus::Event::Start));
            // Nothing is lost at the boundary: what the old session committed
            // after its last Pulse, and what it never committed, is proposed
            // again in the new one.
            let mut carried = old_builder.take_leftover();
            carried.extend(old_core.take_mempool());
            actions.extend(self.propose_again(carried));
            for (from, message) in std::mem::take(&mut self.future) {
                match message_session(&message) {
                    Some(session) if session == self.committees.session => actions.extend(
                        self.core
                            .handle(fold_consensus::Event::Message { from, message }),
                    ),
                    Some(session) if session > self.committees.session => {
                        self.future.push((from, message))
                    }
                    _ => {}
                }
            }
            // Light clients cross the boundary with the certificate of this
            // Pulse and the validator set after it.
            let pulse = self.state.pulse;
            let object = self.state.objects.get(&well_known::validators()).cloned();
            let proof = self.snapshot().prove(&well_known::validators().0);
            self.boundaries.insert(
                next.epoch,
                Boundary {
                    pulse,
                    validators: ObjectProof {
                        id: well_known::validators(),
                        object,
                        proof,
                    },
                    certificate: None,
                },
            );
            self.attach_boundary_certificate(pulse);
        }
        self.committees = next;
        if let Ok(mut view) = self.shared.view.write() {
            view.update(self.state.pulse, &self.committees, &self.state);
        }
        actions
    }

    /// Whether this validator, as an adversary simulation, drops `item`.
    fn censors(&self, item: &[u8]) -> bool {
        self.censoring
            && PayloadItem::from_bytes(item)
                .map(|i| i.senders().iter().any(|s| self.censored.contains(s)))
                .unwrap_or(false)
    }

    /// Submits items an earlier session did not execute to the current
    /// core. Beacon reveals are not carried: each validator reveals afresh
    /// for the Pulses ahead.
    fn propose_again(&mut self, items: Vec<Vec<u8>>) -> Vec<Action> {
        let mut actions = Vec::new();
        for item in items {
            if self.censors(&item)
                || matches!(
                    PayloadItem::from_bytes(&item),
                    Some(PayloadItem::Reveal { .. })
                )
            {
                continue;
            }
            actions.extend(self.core.handle(fold_consensus::Event::Submit(item)));
        }
        actions
    }

    /// Records the certificate of an epoch's last Pulse, once there is one,
    /// with the proof that crosses into the next epoch.
    fn attach_boundary_certificate(&mut self, pulse: Pulse) {
        let Some(certificate) = self
            .collectors
            .get(&pulse)
            .and_then(|c| c.certificate.clone())
        else {
            return;
        };
        let dir = self.boundary_dir.clone();
        for (epoch, boundary) in self.boundaries.iter_mut() {
            if boundary.pulse == pulse && boundary.certificate.is_none() {
                boundary.certificate = Some(certificate.clone());
                if let Some(dir) = &dir {
                    let proof = EpochProof {
                        epoch: *epoch,
                        boundary: certificate.clone(),
                        validators: boundary.validators.clone(),
                    };
                    if let Err(e) = store_boundary(dir, &proof) {
                        eprintln!("validator {}: epoch proof not stored: {e}", self.index);
                    }
                }
            }
        }
    }

    // ----- Header certificates --------------------------------------------

    fn on_pulse_signature(
        &mut self,
        from: ValidatorId,
        pulse: Pulse,
        header: Hash32,
        signature: Vec<u8>,
    ) {
        let Some(collector) = self.collectors.get_mut(&pulse) else {
            // Signatures for a Pulse not executed yet wait, but only a few
            // Pulses ahead and one per committee member.
            let ahead = pulse.0.saturating_sub(self.state.pulse.0);
            if pulse > self.state.pulse
                && ahead <= EARLY_SIGNATURE_WINDOW
                && self.verifier.knows(&from)
            {
                let waiting = self.early_signatures.entry(pulse).or_default();
                if !waiting.iter().any(|(v, _, _)| *v == from) {
                    waiting.push((from, header, signature));
                }
            }
            return;
        };
        if collector.header.digest() != header {
            eprintln!(
                "validator {}: {} signed a different header for Pulse {pulse} (state divergence)",
                self.index,
                from.short()
            );
            return;
        }
        if !self
            .verifier
            .verify(&from, &collector.header.signing_digest().0, &signature)
        {
            return;
        }
        collector.signatures.insert(from, signature);
        self.try_certify(pulse);
    }

    fn try_certify(&mut self, pulse: Pulse) {
        let Some(collector) = self.collectors.get_mut(&pulse) else {
            return;
        };
        if collector.certificate.is_some() {
            return;
        }
        let committee = &collector.committee;
        let signed: BTreeMap<ValidatorId, Vec<u8>> = collector
            .signatures
            .iter()
            .filter(|(v, _)| committee.contains(v))
            .map(|(v, s)| (*v, s.clone()))
            .collect();
        collector.signatures = signed;
        if committee.is_quorum(committee.stake_of_set(collector.signatures.keys())) {
            collector.certificate = Some(PulseCertificate {
                header: collector.header.clone(),
                signatures: collector
                    .signatures
                    .iter()
                    .map(|(v, s)| (*v, s.clone()))
                    .collect(),
            });
            self.certified = self.certified.max(pulse);
            self.attach_boundary_certificate(pulse);
        }
    }

    // ----- Queries ---------------------------------------------------------

    fn snapshot(&mut self) -> &SmtSnapshot {
        let pulse = self.state.pulse;
        if self.proof_cache.as_ref().map(|(p, _)| *p) != Some(pulse) {
            self.proof_cache = Some((pulse, self.state.tree.snapshot()));
        }
        &self.proof_cache.as_ref().expect("set above").1
    }

    fn answer(&mut self, query: Query) -> Result<Value, String> {
        match query {
            Query::Status => {
                let set = self.state.validators();
                let summary = crate::forced::summary(self.state.forced());
                let status = StatusView {
                    chain_id: self.shared.config.chain_id.clone(),
                    genesis: self.shared.genesis_digest,
                    chain: self.shared.chain_id,
                    validator: self.id,
                    epoch: set.epoch,
                    epoch_end: set.epoch_end(),
                    session: self.committees.session,
                    active_validators: set.active.len(),
                    active: set.active.iter().any(|(v, _)| *v == self.id),
                    pulse: self.state.pulse,
                    certified_pulse: self.certified,
                    commitment: self.state.commitment(),
                    round: self.core.round().0,
                    anchors: self.anchors,
                    executed: self.executed,
                    claims: self.state.knowledge.len(),
                    objects: self.state.objects.len() as u64,
                    validators: self.shared.validators.len(),
                    shards: self.shared.config.shards,
                    insecure_signatures: self.shared.config.signatures
                        == crate::keys::SignatureMode::InsecureFast,
                    beacon: self.builder.beacon().latest(),
                    proven_pulse: self
                        .proven
                        .lock()
                        .ok()
                        .and_then(|p| p.keys().next_back().copied()),
                    forced_pending: summary.pending,
                    forced_overdue: summary.overdue,
                };
                Ok(to_json(&status))
            }
            Query::Account(account) => {
                let object = self
                    .state
                    .objects
                    .get(&fold_exec::object::well_known::account(&account))
                    .cloned();
                let coins: Vec<_> = self.state.coins_of(&account).into_iter().cloned().collect();
                let mut balances: BTreeMap<AssetId, u128> = BTreeMap::new();
                for coin in coins.iter().filter_map(|o| o.as_coin()) {
                    *balances.entry(coin.asset).or_default() += coin.amount.0;
                }
                let allotments = self
                    .state
                    .objects
                    .values()
                    .filter(|o| {
                        o.owner == Owner::Account(account)
                            && matches!(o.data, ObjectData::Allotment(_))
                    })
                    .cloned()
                    .collect();
                let view = AccountView {
                    account,
                    object,
                    coins,
                    balances: balances.into_iter().map(|(a, v)| (a, Amount(v))).collect(),
                    allotments,
                    pulse: self.state.pulse,
                };
                Ok(to_json(&view))
            }
            Query::Object(id) => {
                let object = self.state.objects.get(&id).cloned();
                let proof = self.snapshot().prove(&id.0);
                let proof = ObjectProof { id, object, proof };
                Ok(json!({ "pulse": self.state.pulse, "proof": proof }))
            }
            Query::Claim(id) => {
                let stored = self.state.claim(&id).cloned().ok_or("unknown claim")?;
                let registry = self.state.registry();
                let record = registry.claim(&id).cloned();
                let coc = claim_coc(registry, &Policy::standard(), &id);
                let path = self
                    .state
                    .knowledge
                    .inclusion_proof(stored.log_index)
                    .map_err(|e| e.to_string())?;
                let atoms: Vec<Value> = stored
                    .body
                    .formula
                    .direct_atoms()
                    .iter()
                    .filter_map(|a| registry.atom(a))
                    .map(|atom| {
                        json!({
                            "atom": atom,
                            "bond": registry.bond(&atom.id),
                            "broken": registry.is_broken(&atom.id),
                        })
                    })
                    .collect();
                Ok(json!({
                    "pulse": self.state.pulse,
                    "claim": stored.clone(),
                    "status": record.map(|r| format!("{:?}", r.status)),
                    "coc_standard_policy": coc,
                    "atoms": atoms,
                    "proof": ClaimProof { claim: stored, path },
                }))
            }
            Query::Decide {
                claim,
                value,
                policy,
            } => {
                let decision = decide(self.state.registry(), &policy, &claim, value, true)
                    .map_err(|e| e.to_string())?;
                Ok(to_json(&decision))
            }
            Query::Tx(digest) => {
                let (pulse, index) = *self
                    .tx_index
                    .get(&digest)
                    .ok_or("not executed (or pruned)")?;
                let executed = self.history.get(&pulse).ok_or("pruned")?;
                let effects = executed.output.effects[index as usize].clone();
                let leaves: Vec<Hash32> =
                    executed.output.effects.iter().map(effects_leaf).collect();
                let path =
                    merkle::inclusion_proof(&leaves, index as usize).map_err(|e| e.to_string())?;
                let view = TxView {
                    pulse,
                    index,
                    status: effects.status.clone(),
                    receipt: ReceiptProof {
                        effects: effects.clone(),
                        index,
                        count: leaves.len() as u64,
                        path,
                    },
                    effects,
                };
                Ok(to_json(&view))
            }
            Query::Pulse(pulse) => {
                let pulse = pulse.unwrap_or(self.state.pulse);
                let executed = self.history.get(&pulse).ok_or("unknown or pruned Pulse")?;
                let collector = self.collectors.get(&pulse).ok_or("unknown Pulse")?;
                let view = PulseView {
                    header: collector.header.clone(),
                    certificate: collector.certificate.clone(),
                    transactions: executed.output.order.clone(),
                    counts: executed.output.status_counts(),
                    deferred_events: executed.output.deferred_events.clone(),
                    new_claims: executed.output.new_claims.iter().map(|c| c.id).collect(),
                    anchors: executed.anchors,
                    revealed: executed.revealed,
                    data_bytes: executed.data_bytes,
                    execution_ms: executed.execution_ms,
                };
                Ok(to_json(&view))
            }
            Query::Data(pulse) => {
                let executed = self.history.get(&pulse).ok_or("unknown or pruned Pulse")?;
                let header = executed.da.as_ref().ok_or("no data square")?;
                Ok(to_json(header))
            }
            Query::Share { pulse, row, column } => {
                if !self.squares.iter().any(|(p, _)| *p == pulse) {
                    let executed = self.history.get(&pulse).ok_or("unknown or pruned Pulse")?;
                    let blob = executed.blob.as_ref().ok_or("data pruned")?;
                    let square = ExtendedSquare::encode(blob).map_err(|e| e.to_string())?;
                    self.squares.push_back((pulse, square));
                    while self.squares.len() > SQUARE_CACHE {
                        self.squares.pop_front();
                    }
                }
                let (_, square) = self
                    .squares
                    .iter()
                    .find(|(p, _)| *p == pulse)
                    .expect("cached above");
                let share = square.share(row, column).map_err(|e| e.to_string())?;
                Ok(to_json(&share))
            }
            Query::Events { from, limit } => {
                let mut events = Vec::new();
                'outer: for (pulse, executed) in self.history.range(from..) {
                    for effects in &executed.output.effects {
                        for event in &effects.events {
                            events.push(EventView {
                                pulse: *pulse,
                                tx: Some(effects.digest),
                                event: event.clone(),
                            });
                        }
                    }
                    for event in &executed.output.deferred_events {
                        events.push(EventView {
                            pulse: *pulse,
                            tx: None,
                            event: event.clone(),
                        });
                    }
                    if events.len() >= limit {
                        break 'outer;
                    }
                }
                events.truncate(limit);
                Ok(to_json(&events))
            }
            Query::Validators => {
                let set = self.state.validators();
                let records: Vec<Value> = set
                    .validators
                    .values()
                    .map(|r| {
                        json!({
                            "validator": r.id,
                            "operator": r.operator,
                            "stake": r.stake,
                            "rewards": r.rewards,
                            "status": r.status,
                            "registered_at": r.registered_at,
                            "active": set.active.iter().any(|(v, _)| *v == r.id),
                        })
                    })
                    .collect();
                Ok(json!({
                    "chain": self.shared.chain,
                    "chain_id": self.shared.chain_id,
                    "genesis": self.shared.genesis_digest,
                    "insecure_signatures": self.shared.config.signatures == crate::keys::SignatureMode::InsecureFast,
                    "pulse": self.state.pulse,
                    "epoch": set.epoch,
                    "epoch_start": set.epoch_start,
                    "epoch_end": set.epoch_end(),
                    "session": self.committees.session,
                    "params": set.params,
                    "committee": self.committees.signers,
                    "keys": crate::committees::registered_keys(set),
                    "shards": self.committees.lanes,
                    "rotations": set.rotations,
                    "validators": records,
                }))
            }
            Query::Epoch(epoch) => {
                let boundary = self
                    .boundaries
                    .get(&epoch)
                    .ok_or("no proof for that epoch (it has not begun, or began before this validator kept proofs)")?;
                let certificate = boundary
                    .certificate
                    .clone()
                    .ok_or("the epoch's boundary Pulse is not certified yet")?;
                Ok(to_json(&EpochProof {
                    epoch,
                    boundary: certificate,
                    validators: boundary.validators.clone(),
                }))
            }
            Query::Genesis => Ok(json!({
                "digest": self.shared.genesis_digest,
                "spec": self.shared.genesis,
            })),
            Query::Forced => {
                let obligations = self.state.forced();
                Ok(json!({
                    "pulse": self.state.pulse,
                    "window": obligations.params.window,
                    "summary": crate::forced::summary(obligations),
                    "pending": crate::forced::pending(obligations),
                    "rotations": self.state.validators().rotations,
                }))
            }
            Query::World(pulse) => {
                let entry = self
                    .proven
                    .lock()
                    .map_err(|_| "prover state poisoned".to_string())?
                    .get(&pulse)
                    .cloned()
                    .ok_or_else(|| "no World Proof for that Pulse (yet)".to_string())?;
                let mut summary = entry.summary;
                if let (Some(bytes), Value::Object(map)) = (&entry.bytes, &mut summary) {
                    map.insert("proof".into(), Value::String(hex::encode(bytes.as_slice())));
                }
                Ok(summary)
            }
        }
    }
}

fn to_json<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// Loads the epoch-boundary proofs a validator stored.
fn load_boundaries(dir: &std::path::Path) -> BTreeMap<u64, Boundary> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let Ok(bytes) = std::fs::read(entry.path()) else {
            continue;
        };
        if let Ok(proof) = fold_core::encode::decode::<EpochProof>(&bytes) {
            out.insert(
                proof.epoch,
                Boundary {
                    pulse: proof.boundary.header.pulse,
                    validators: proof.validators,
                    certificate: Some(proof.boundary),
                },
            );
        }
    }
    out
}

/// Stores one epoch-boundary proof (write and rename, so it is whole or
/// absent).
fn store_boundary(dir: &std::path::Path, proof: &EpochProof) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("epoch-{:010}.bin", proof.epoch));
    let temp = dir.join(format!("epoch-{:010}.tmp", proof.epoch));
    std::fs::write(&temp, fold_core::encode::encode(proof))?;
    std::fs::rename(temp, path)
}
