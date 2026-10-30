//! One validator: lane approvals, consensus, Pulse execution, header
//! certificates, data availability and storage, on its own thread.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sion_assurance::coc::claim_coc;
use sion_assurance::{decide, Policy};
use sion_consensus::lanes::verify_lane_certificate;
use sion_consensus::LaneCertificate;
use sion_consensus::{
    Action, ApprovalProof, ApprovalStatus, Authenticator, Beacon, CertificateBuilder, Committee,
    Core, HashChain, LaneRequest, LaneValidator, Message, SignatureVerifier, TimerKind,
};
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, AssetId, Pulse, ShardId, TxDigest, ValidatorId};
use sion_core::merkle;
use sion_core::smt::SmtSnapshot;
use sion_core::Amount;
use sion_da::{DaHeader, ExtendedSquare};
use sion_exec::object::well_known;
use sion_exec::pulse::{
    effects_leaf, execute_pulse, execute_pulse_traced, posted_intents, ChunkTrace, PulseInput,
    PulseOutput,
};
use sion_exec::validators::ValidatorSet as StateValidators;
use sion_exec::{GenesisSpec, ObjectData, Owner, SignedTransaction, State};
use sion_light::{
    ChainSpec, ClaimProof, EpochProof, ObjectProof, PulseCertificate, PulseHeader, ReceiptProof,
};
use sion_prover::{PulseData, ValidatorsWitness};

use crate::committees::{CommitteeView, Committees};
use crate::config::DevnetConfig;
use crate::keys::{NodeAuthenticator, NodeVerifier};
use crate::owners::{AlreadyChecked, Authorized, StateOwners};
use crate::payload::{expand, lane_inputs, restore, PayloadItem, MAX_GROUP_BATCHES};
use crate::pulses::{Carry, ClosedPulse, PulseBuilder};
use crate::storage::{LaneLog, PulseRecord, Snapshot, Wal};
use crate::transport::{NodeId, PeerMessage, Transport, EXECUTED_CHUNK};
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

/// Bytes of future-session messages kept per sender: they are kept
/// unverified, so a single peer must not fill memory with them.
const MAX_FUTURE_BYTES_PER_SENDER: usize = 32 * 1024 * 1024;

/// Lane requests waiting for this validator to catch up, at most.
const MAX_PARKED: usize = 16_384;

/// A Pulse still uncertified this long after it executed: ask the members
/// whose signatures are missing for them.
const SIGNATURE_PULL_AFTER: Duration = Duration::from_secs(2);

/// How often to look for Pulses to pull signatures for.
const SIGNATURE_PULL_INTERVAL: Duration = Duration::from_millis(500);

/// Uncertified Pulses asked about per look, newest first (any Pulse still
/// in the history qualifies).
const SIGNATURE_PULLS_PER_TICK: usize = 64;

/// Gateways a validator reports executions to, at most, and how long one
/// stays without sending anything.
const MAX_SUBSCRIBERS: usize = 64;
const SUBSCRIBER_TTL: Duration = Duration::from_secs(600);

/// Messages to a validator.
#[derive(Debug)]
pub enum Input {
    /// A message from another node, with the sender as the transport
    /// stamped it.
    Peer {
        /// Sender.
        from: NodeId,
        /// Message.
        message: PeerMessage,
    },
    /// A payload item to propose, from this process (the devnet's API posts
    /// forced intents this way).
    Submit(Vec<u8>),
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

/// Whom a validator tells which transactions a Pulse executed, so gateways
/// stop handing them to consensus again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutedReports {
    /// These nodes (the devnet: validator 0 reports to its gateway, the
    /// others to nobody).
    To(Vec<NodeId>),
    /// Every node that sent this validator a lane request or a payload in
    /// the last ten minutes: the gateways that use it (between machines).
    Subscribers,
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
    /// Ask for signatures of Pulses that are slow to certify.
    SignaturePull,
}

/// A lane request waiting for this validator to catch up.
#[derive(Debug)]
struct Parked {
    shard: ShardId,
    request: LaneRequest,
    /// Pulses it has waited.
    waited: u64,
    /// The node that asked, which gets the answer.
    requester: NodeId,
}

#[derive(Debug)]
struct Collector {
    header: PulseHeader,
    /// Who signs it: the active set of the header's epoch.
    committee: Committee,
    signatures: BTreeMap<ValidatorId, Vec<u8>>,
    certificate: Option<PulseCertificate>,
    /// When this validator executed the Pulse.
    executed_at: Instant,
    /// When it last asked for the missing signatures.
    pulled_at: Option<Instant>,
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

/// The lane locks to release after a Pulse, as `(shard, transaction)`:
/// those of transactions that can no longer execute. A transaction that
/// succeeded or failed moved every input it named to a new version (spec
/// §4.2, `finalize_inputs`), and one whose `expires_at` is at most `now`
/// can never run again.
///
/// A **rejected** transaction is neither: it wrote nothing, its inputs keep
/// their versions, and it may still execute later (an input can reach the
/// version it names, an account can be created or rotate its policy back).
/// Its locks therefore hold until it expires. Releasing them earlier would
/// let this validator approve a second spend of a version its first approval
/// still covers, and the two approvals are valid equivocation evidence
/// (spec §5.30) until the first transaction expires.
fn released_locks<'a>(
    executed: impl IntoIterator<Item = (&'a TxDigest, &'a sion_exec::ExecStatus)>,
    lock_expiry: &BTreeMap<TxDigest, (ShardId, Pulse)>,
    now: Pulse,
) -> Vec<(ShardId, TxDigest)> {
    let mut released: BTreeMap<TxDigest, ShardId> = BTreeMap::new();
    for (digest, status) in executed {
        if matches!(status, sion_exec::ExecStatus::Rejected(_)) {
            continue;
        }
        if let Some((shard, _)) = lock_expiry.get(digest) {
            released.insert(*digest, *shard);
        }
    }
    for (digest, (shard, expires_at)) in lock_expiry {
        if *expires_at <= now {
            released.insert(*digest, *shard);
        }
    }
    released
        .into_iter()
        .map(|(digest, shard)| (shard, digest))
        .collect()
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
    /// Who asked for an approval that waits for the next sealed batch.
    lane_waiters: BTreeMap<TxDigest, BTreeSet<NodeId>>,
    state: State,
    builder: PulseBuilder,
    /// What the builder carried past the close of the state's Pulse.
    last_carry: Carry,
    /// Oversized payload items earlier sessions' cores refused.
    dropped_items: u64,
    chain: HashChain,
    wal: Option<Wal>,
    net: Arc<dyn Transport>,
    /// Every other validator of the network, in index order: whom a
    /// broadcast reaches.
    peers: Vec<NodeId>,
    executed_to: ExecutedReports,
    /// Nodes that sent lane requests or payloads, and when they last did.
    subscribers: BTreeMap<NodeId, Instant>,
    /// Signature requests answered lately: (Pulse, requester) → when.
    answered: BTreeMap<(Pulse, ValidatorId), Instant>,
    inbox: Receiver<Input>,
    timers: BinaryHeap<Reverse<(Instant, u64)>>,
    timer_kinds: BTreeMap<u64, Timer>,
    timer_seq: u64,
    history: BTreeMap<Pulse, Executed>,
    tx_index: BTreeMap<TxDigest, (Pulse, u64)>,
    /// Every storage deal's id, for `Query::Deals`: built by the first such
    /// query from the state, then kept up to date from each Pulse's
    /// created objects.
    deal_index: Option<std::collections::BTreeSet<sion_core::ids::ObjectId>>,
    collectors: BTreeMap<Pulse, Collector>,
    early_signatures: BTreeMap<Pulse, Vec<(ValidatorId, Hash32, Vec<u8>)>>,
    certified: Pulse,
    executed: (u64, u64, u64),
    anchors: u64,
    proof_cache: Option<(Pulse, SmtSnapshot)>,
    verifier: NodeVerifier,
    authorized: Authorized,
    squares: VecDeque<(Pulse, ExtendedSquare)>,
    parked: Vec<Parked>,
    lock_expiry: BTreeMap<TxDigest, (ShardId, Pulse)>,
    lane_log: Option<LaneLog>,
    lane_log_pending: Vec<(ShardId, LaneRequest)>,
    committees: Committees,
    /// Consensus messages of sessions not reached yet: sender, session,
    /// message.
    future: Vec<(ValidatorId, u64, Message)>,
    /// Their encoded bytes, by sender.
    future_bytes: BTreeMap<ValidatorId, usize>,
    boundaries: BTreeMap<u64, Boundary>,
    boundary_dir: Option<PathBuf>,
    /// Where it records each consensus session before signing in it.
    session_guard: Option<PathBuf>,
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
    /// What the Pulse builder carried past the state's Pulse: the recovered
    /// or genesis beacon, with the reveals committed for later Pulses.
    pub carry: Carry,
    /// Its beacon hash chain.
    pub chain: HashChain,
    /// Its log, if persistent.
    pub wal: Option<Wal>,
    /// Its inbox.
    pub inbox: Receiver<Input>,
    /// What carries its messages to other nodes (and delivers theirs into
    /// its inbox).
    pub transport: Arc<dyn Transport>,
    /// Whom it tells what each Pulse executed.
    pub executed_to: ExecutedReports,
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
    /// A node of a network: the directory where it records each consensus
    /// session before it signs anything in it ([`crate::node::claim_session`];
    /// the session it starts in is claimed before it starts). A devnet,
    /// whose sessions change with every restart, needs none.
    pub session_guard: Option<PathBuf>,
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
        .unwrap_or_else(|error| {
            panic!(
                "no committees in epoch {}: {error} (a chain whose every eligible validator was slashed has halted)",
                setup.state.validators().epoch
            )
        });
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
                    sion_core::encode::decode::<sion_exec::SignedTransaction>(&request.owner_proof)
                {
                    lock_expiry.insert(request.tx, (shard, signed.tx.expires_at));
                }
                lane.restore(request);
            }
        }
        // The beacon follows the active set (a restart may come right after
        // an epoch boundary). Reveals committed before a restart for the
        // Pulses ahead still count.
        let mut carry = setup.carry;
        sync_beacon(&mut carry.beacon, setup.state.validators());
        let last_carry = carry.clone();
        let mut builder =
            PulseBuilder::resume(shared.config.rounds_per_pulse, setup.state.pulse, carry);
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
        let peers = shared
            .validators
            .iter()
            .filter(|validator| **validator != id)
            .map(|validator| NodeId::Validator(*validator))
            .collect();
        Validator {
            index: setup.index,
            id,
            auth: setup.auth,
            core,
            lanes,
            lane_waiters: BTreeMap::new(),
            state: setup.state,
            builder,
            last_carry,
            dropped_items: 0,
            chain: setup.chain,
            wal: setup.wal,
            net: setup.transport,
            peers,
            executed_to: setup.executed_to,
            subscribers: BTreeMap::new(),
            deal_index: None,
            answered: BTreeMap::new(),
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
            future_bytes: BTreeMap::new(),
            boundaries,
            boundary_dir: setup.boundary_dir,
            session_guard: setup.session_guard,
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
            Some(value) => self.core.handle(sion_consensus::Event::Submit(
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
        let mut actions: VecDeque<Action> = self.core.handle(sion_consensus::Event::Start).into();
        actions.extend(self.submit_reveal());
        self.apply(actions);
        self.schedule(
            Duration::from_millis(self.shared.config.batch_interval_ms),
            Timer::LaneBatch,
        );
        self.schedule(SIGNATURE_PULL_INTERVAL, Timer::SignaturePull);
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

    /// A snapshot of the state with what the builder carried past the
    /// state's Pulse (not the builder's state now: one commit can close
    /// several Pulses before the first of them executes).
    fn snapshot_record(&self) -> Snapshot {
        Snapshot {
            state: self.state.clone(),
            carry: self.last_carry.clone(),
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
                        let actions = self.core.handle(sion_consensus::Event::Timer(kind));
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
                Some(Timer::SignaturePull) => {
                    self.pull_signatures();
                    self.schedule(SIGNATURE_PULL_INTERVAL, Timer::SignaturePull);
                }
                None => {}
            }
        }
    }

    fn handle(&mut self, input: Input) {
        match input {
            Input::Peer { from, message } => self.on_peer(from, message),
            Input::Submit(item) => self.submit_item(item),
            Input::Adversary { censored, censor } => {
                self.censored = censored;
                self.censoring = censor;
            }
            Input::Query(query, reply) => {
                let _ = reply.send(self.answer(query));
            }
            Input::Shutdown => {}
        }
    }

    /// A message from another node. Who sent it is the transport's word,
    /// never the message's: consensus messages, Pulse signatures and forced
    /// approvals count only from validators.
    fn on_peer(&mut self, from: NodeId, message: PeerMessage) {
        match message {
            PeerMessage::Consensus { session, message } => {
                if let Some(validator) = from.validator() {
                    self.on_consensus(validator, session, message);
                }
            }
            PeerMessage::Payload { item } => {
                self.subscribe(from);
                self.submit_item(item);
            }
            PeerMessage::Lane { shard, request } => {
                self.subscribe(from);
                self.handle_lane_request(shard, request, 0, from);
            }
            PeerMessage::LaneBatch {
                shard,
                batch,
                auth,
                members,
            } => {
                // Each member is a lane request of its own; the batch's
                // signatures are checked with the first and remembered.
                if members.len() as u64 > sion_exec::tx::MAX_BATCH {
                    return;
                }
                self.subscribe(from);
                for member in &members {
                    let signed = member.signed(batch, &auth);
                    let request = LaneRequest {
                        tx: signed.digest(),
                        inputs: lane_inputs(&signed.tx.inputs),
                        owner_proof: sion_core::encode::encode(&signed),
                    };
                    self.handle_lane_request(shard, request, 0, from);
                }
            }
            PeerMessage::PulseSignature {
                pulse,
                header,
                signature,
            } => {
                if let Some(validator) = from.validator() {
                    self.on_pulse_signature(validator, pulse, header, signature);
                }
            }
            PeerMessage::PulseSignatureRequest { pulse } => {
                if let Some(validator) = from.validator() {
                    self.answer_signature_request(validator, pulse);
                }
            }
            PeerMessage::ForcedApproval { shard, tx, proof } => {
                if from.is_validator() {
                    let actions = self.on_forced_approval(shard, tx, proof);
                    self.apply(actions.into());
                }
            }
            // For gateways.
            PeerMessage::Approval { .. }
            | PeerMessage::LaneRejected { .. }
            | PeerMessage::Executed { .. } => {}
        }
    }

    /// A consensus message in the envelope of `session`. Headers and
    /// certificates name their session themselves, and the envelope must
    /// agree; votes and sync requests are routed by the envelope alone.
    fn on_consensus(&mut self, from: ValidatorId, session: u64, message: Message) {
        if message_session(&message).is_some_and(|own| own != session) {
            return;
        }
        if session > self.committees.session {
            // From a session this validator reaches at its next epoch
            // boundary: keep it until then, within a byte budget per sender
            // (it is not verified yet).
            let bytes = borsh::object_length(&message).unwrap_or(usize::MAX);
            let held = self.future_bytes.entry(from).or_default();
            if self.future.len() < MAX_FUTURE_MESSAGES
                && held.saturating_add(bytes) <= MAX_FUTURE_BYTES_PER_SENDER
            {
                *held += bytes;
                self.future.push((from, session, message));
            }
            return;
        }
        if session < self.committees.session {
            return;
        }
        let actions = self
            .core
            .handle(sion_consensus::Event::Message { from, message });
        self.apply(actions.into());
    }

    /// A payload item to propose, unless this validator censors it.
    fn submit_item(&mut self, item: Vec<u8>) {
        if self.censors(&item) {
            return;
        }
        let actions = self.core.handle(sion_consensus::Event::Submit(item));
        self.apply(actions.into());
    }

    /// Sends to every other validator of the network.
    fn broadcast(&self, message: PeerMessage) {
        self.net.send_all(&self.peers, &message);
    }

    /// Carries out actions of the current session's core.
    fn apply(&mut self, actions: VecDeque<Action>) {
        let session = self.committees.session;
        self.apply_tagged(actions.into_iter().map(|a| (session, a)).collect());
    }

    /// Carries out actions, each tagged with the session of the core that
    /// produced it: a message an old session's core still sends after the
    /// boundary goes out in that session's envelope.
    fn apply_tagged(&mut self, mut actions: VecDeque<(u64, Action)>) {
        while let Some((session, action)) = actions.pop_front() {
            match action {
                Action::Broadcast(message) => {
                    self.broadcast(PeerMessage::Consensus { session, message })
                }
                Action::Send { to, message } => self.net.send(
                    NodeId::Validator(to),
                    PeerMessage::Consensus { session, message },
                ),
                Action::SetTimer { timer, after_ms } => {
                    self.schedule(Duration::from_millis(after_ms), Timer::Core(session, timer))
                }
                Action::Commit(commit) => {
                    self.anchors += 1;
                    for closed in self.builder.on_commit(&commit) {
                        let next = self.execute(closed);
                        let current = self.committees.session;
                        if current != session {
                            // The epoch's last Pulse executed and a new
                            // consensus session began. What the old session's
                            // core still asked for is stale: its later commits
                            // are proposed again in the new session, and its
                            // timers are dropped. Its messages still go out,
                            // for validators that have not reached the
                            // boundary yet.
                            let mut carried = Vec::new();
                            for (tag, action) in std::mem::take(&mut actions) {
                                match action {
                                    Action::Commit(late) => carried.extend(late.items),
                                    Action::SetTimer { .. } => {}
                                    other => actions.push_back((tag, other)),
                                }
                            }
                            let again = self.propose_again(carried);
                            actions.extend(again.into_iter().map(|a| (current, a)));
                        }
                        actions.extend(next.into_iter().map(|a| (current, a)));
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

    /// Checks a lane request and approves it; the answer goes to
    /// `requester`, the node that asked. A request that fails only because
    /// this validator is behind (an input it has not seen created yet) waits
    /// up to [`PARK_PULSES`] Pulses instead of being refused, so lagging
    /// validators do not split an object's locks.
    fn handle_lane_request(
        &mut self,
        shard: ShardId,
        request: LaneRequest,
        parked_for: u64,
        requester: NodeId,
    ) {
        let tx = request.tx;
        let Some(lane) = self.lanes.get_mut(&shard) else {
            self.net.send(
                requester,
                PeerMessage::rejected(tx, "not a member of this shard's committee"),
            );
            return;
        };
        let owners = StateOwners { state: &self.state };
        // A batch's members share one signature check (spec §4.3).
        let expires_at = match owners.check_cached(&request, &mut self.authorized) {
            Ok(signed) if self.censoring && self.censored.contains(&signed.tx.sender) => {
                // Censorship is silent: no approval, no rejection.
                return;
            }
            Ok(signed) => signed.tx.expires_at,
            Err(reason) => {
                let behind =
                    sion_core::encode::decode::<sion_exec::SignedTransaction>(&request.owner_proof)
                        .map(|signed| owners.is_behind(&signed))
                        .unwrap_or(false);
                if behind && parked_for < PARK_PULSES {
                    // A request asked again while it waits is already
                    // waiting.
                    if self
                        .parked
                        .iter()
                        .any(|p| p.request.tx == tx && p.requester == requester)
                    {
                        return;
                    }
                    if self.parked.len() >= MAX_PARKED {
                        self.net.send(
                            requester,
                            PeerMessage::rejected(tx, "this validator is behind; try again"),
                        );
                        return;
                    }
                    self.parked.push(Parked {
                        shard,
                        request,
                        waited: parked_for + 1,
                        requester,
                    });
                } else {
                    self.net.send(requester, PeerMessage::rejected(tx, reason));
                }
                return;
            }
        };
        self.lock_expiry.insert(tx, (shard, expires_at));
        let logged = request.clone();
        match lane.approve(request, &AlreadyChecked) {
            Ok(ApprovalStatus::Pending) => {
                self.lane_waiters.entry(tx).or_default().insert(requester);
                self.lane_log_pending.push((shard, logged));
            }
            Ok(ApprovalStatus::Sealed(proof)) => {
                self.net
                    .send(requester, PeerMessage::Approval { shard, tx, proof });
            }
            Err(rejection) => {
                self.net.send(
                    requester,
                    PeerMessage::rejected(tx, rejection.error.to_string()),
                );
            }
        }
    }

    /// A consensus-ordered transaction whose lane certificate does not
    /// verify is not executed. Honest gateways never produce one, so it is
    /// worth a line in the log (once, from the first validator).
    fn drop_uncertified(&self, tx: &sion_exec::SignedTransaction, why: &str) {
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
                if let Some(requesters) = self.lane_waiters.remove(tx) {
                    if let Some(proof) = lane.approval_proof(tx) {
                        let approval = PeerMessage::Approval {
                            shard: *shard,
                            tx: *tx,
                            proof: proof.clone(),
                        };
                        let requesters: Vec<NodeId> = requesters.into_iter().collect();
                        self.net.send_all(&requesters, &approval);
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
        self.broadcast(PeerMessage::ForcedApproval {
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
                owner_proof: sion_core::encode::encode(&tx),
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
                    .handle(sion_consensus::Event::Submit(group.to_bytes()))
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
            let (batches, authorizations, txs) = match item {
                PayloadItem::Certified {
                    batches,
                    authorizations,
                    txs,
                } => (batches, authorizations, txs),
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
                || authorizations.len() > txs.len()
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
                // A batch member's signatures come from the group's list.
                let Some(tx) = restore(&authorizations, grouped) else {
                    self.drop_uncertified(
                        &grouped.tx,
                        "references an authorization the group does not carry",
                    );
                    continue;
                };
                let tx = &tx;
                // The certificate must lock exactly the transaction's owned
                // inputs, with a quorum of its shard's committee.
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
        self.last_carry = closed.carry;
        if let Some(index) = &mut self.deal_index {
            for id in output.effects.iter().flat_map(|e| e.created.iter()) {
                if matches!(
                    self.state.objects.get(id).map(|o| &o.data),
                    Some(ObjectData::Deal(_))
                ) {
                    index.insert(*id);
                }
            }
        }
        let (s, f, r) = output.status_counts();
        self.executed = (
            self.executed.0 + s as u64,
            self.executed.1 + f as u64,
            self.executed.2 + r as u64,
        );

        // Forced inclusion: the state now holds what the shards owe. Keep the
        // signed intents it still holds obligations for.
        let executed_digests: Vec<TxDigest> = output.effects.iter().map(|e| e.digest).collect();
        let outcomes: Vec<(TxDigest, sion_exec::ExecStatus)> = output
            .effects
            .iter()
            .map(|e| (e.digest, e.status.clone()))
            .collect();
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
                if let sion_exec::Event::ForcedOverdue { intent, shard } = event {
                    eprintln!(
                        "validator: Pulse {}: forced intent {} overdue in shard {shard}: censorship detected",
                        input.pulse,
                        intent.short()
                    );
                }
                if let sion_exec::Event::ShardRotated { shard, rotation } = event {
                    eprintln!(
                        "validator: Pulse {}: shard {shard}'s committee rotated (rotation {rotation})",
                        input.pulse
                    );
                }
            }
        }

        // Data availability: the transaction bodies in execution order,
        // without signatures.
        let bodies: BTreeMap<TxDigest, &sion_exec::Transaction> = input
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
                carry: self.last_carry.clone(),
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
                executed_at: Instant::now(),
                pulled_at: None,
            };
            collector.signatures.insert(self.id, signature.clone());
            self.collectors.insert(input.pulse, collector);
            self.broadcast(PeerMessage::PulseSignature {
                pulse: input.pulse,
                header: digest,
                signature,
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
        self.report_executed(input.pulse, &executed_digests);

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

        // Lane locks are released once their transaction can no longer
        // execute, so its inputs never stay frozen; a rejected one keeps them
        // until it expires (see `released_locks`).
        let released = released_locks(
            outcomes.iter().map(|(digest, status)| (digest, status)),
            &self.lock_expiry,
            self.state.pulse,
        );
        for (shard, digest) in released {
            if let Some(lane) = self.lanes.get_mut(&shard) {
                lane.release(&digest);
            }
            self.lock_expiry.remove(&digest);
            self.lane_waiters.remove(&digest);
        }
        // Whatever executed, rejected or not, no longer waits for this
        // validator's approval.
        for digest in &executed_digests {
            self.lane_waiters.remove(digest);
        }
        // Requests that waited for this validator to catch up get another try.
        for parked in std::mem::take(&mut self.parked) {
            self.handle_lane_request(
                parked.shard,
                parked.request,
                parked.waited,
                parked.requester,
            );
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

    /// Tells the nodes this validator reports to which transactions `pulse`
    /// executed (one message per [`EXECUTED_CHUNK`] digests, and one even
    /// when it executed none).
    fn report_executed(&self, pulse: Pulse, digests: &[TxDigest]) {
        let nodes: Vec<NodeId> = match &self.executed_to {
            ExecutedReports::To(nodes) => nodes.clone(),
            ExecutedReports::Subscribers => self.subscribers.keys().copied().collect(),
        };
        if nodes.is_empty() {
            return;
        }
        if digests.is_empty() {
            self.net.send_all(
                &nodes,
                &PeerMessage::Executed {
                    pulse,
                    digests: Vec::new(),
                },
            );
        }
        for chunk in digests.chunks(EXECUTED_CHUNK) {
            self.net.send_all(
                &nodes,
                &PeerMessage::Executed {
                    pulse,
                    digests: chunk.to_vec(),
                },
            );
        }
    }

    /// Remembers a node that hands this validator work, to report
    /// executions to (only between machines: the devnet names its gateway).
    fn subscribe(&mut self, node: NodeId) {
        if self.executed_to != ExecutedReports::Subscribers {
            return;
        }
        let now = Instant::now();
        self.subscribers.insert(node, now);
        if self.subscribers.len() > MAX_SUBSCRIBERS {
            if let Some(stalest) = self
                .subscribers
                .iter()
                .min_by_key(|(_, at)| **at)
                .map(|(node, _)| *node)
            {
                self.subscribers.remove(&stalest);
            }
        }
    }

    /// Asks the committee members whose signatures are missing for those of
    /// the Pulses that have stayed uncertified a while (newest first, any
    /// Pulse still in the history): a lost signature would otherwise leave
    /// this validator without the certificate.
    fn pull_signatures(&mut self) {
        let now = Instant::now();
        let mut requests = Vec::new();
        let due = self
            .collectors
            .iter_mut()
            .rev()
            .filter(|(_, collector)| {
                collector.certificate.is_none()
                    && now.duration_since(collector.executed_at) >= SIGNATURE_PULL_AFTER
                    && collector
                        .pulled_at
                        .map_or(true, |at| now.duration_since(at) >= SIGNATURE_PULL_AFTER)
            })
            .take(SIGNATURE_PULLS_PER_TICK);
        for (pulse, collector) in due {
            collector.pulled_at = Some(now);
            for (member, _) in collector.committee.members() {
                if *member != self.id && !collector.signatures.contains_key(member) {
                    requests.push((NodeId::Validator(*member), *pulse));
                }
            }
        }
        for (member, pulse) in requests {
            self.net
                .send(member, PeerMessage::PulseSignatureRequest { pulse });
        }
        self.answered
            .retain(|_, at| now.duration_since(*at) < SIGNATURE_PULL_AFTER);
        self.subscribers
            .retain(|_, at| now.duration_since(*at) < SUBSCRIBER_TTL);
    }

    /// Sends `requester` this validator's signature on a Pulse it executed
    /// and still holds, at most once per pull interval.
    fn answer_signature_request(&mut self, requester: ValidatorId, pulse: Pulse) {
        let Some(collector) = self.collectors.get(&pulse) else {
            return;
        };
        let Some(signature) = collector.signatures.get(&self.id) else {
            return;
        };
        if !collector.committee.contains(&requester) {
            return;
        }
        let now = Instant::now();
        if self
            .answered
            .get(&(pulse, requester))
            .is_some_and(|at| now.duration_since(*at) < SIGNATURE_PULL_AFTER)
        {
            return;
        }
        self.answered.insert((pulse, requester), now);
        self.net.send(
            NodeId::Validator(requester),
            PeerMessage::PulseSignature {
                pulse,
                header: collector.header.digest(),
                signature: signature.clone(),
            },
        );
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
                let set = self.state.validators();
                if set.active.is_empty() {
                    // Spec §6.3: every validator that could have served was
                    // slashed, so nobody certifies the next Pulse.
                    eprintln!(
                        "validator {}: the chain has halted after Pulse {}: epoch {} has no eligible validator",
                        self.index, self.state.pulse, set.epoch
                    );
                } else {
                    eprintln!("validator {}: no committees: {error}", self.index);
                }
                return actions;
            }
        };
        if next.session != self.committees.session {
            if let Some(dir) = &self.session_guard {
                // Nothing is signed in a session before it is recorded: a
                // validator that cannot record it stops here.
                if let Err(error) = crate::node::claim_session(dir, next.session) {
                    eprintln!(
                        "validator {}: stops before session {}: {error}",
                        self.index, next.session
                    );
                    return actions;
                }
            }
        }
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
            actions.extend(self.core.handle(sion_consensus::Event::Start));
            // Nothing is lost at the boundary: what the old session committed
            // after its last Pulse, and what it never committed, is proposed
            // again in the new one.
            let mut carried = old_builder.take_leftover();
            carried.extend(old_core.take_mempool());
            self.dropped_items += old_core.oversized_items();
            actions.extend(self.propose_again(carried));
            self.future_bytes.clear();
            for (from, session, message) in std::mem::take(&mut self.future) {
                if session == self.committees.session {
                    actions.extend(
                        self.core
                            .handle(sion_consensus::Event::Message { from, message }),
                    );
                } else if session > self.committees.session {
                    *self.future_bytes.entry(from).or_default() +=
                        borsh::object_length(&message).unwrap_or(0);
                    self.future.push((from, session, message));
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
            actions.extend(self.core.handle(sion_consensus::Event::Submit(item)));
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
                    dropped_items: self.dropped_items + self.core.oversized_items(),
                    network: Some(self.net.stats()),
                };
                Ok(to_json(&status))
            }
            Query::Account(account) => {
                let object = self
                    .state
                    .objects
                    .get(&sion_exec::object::well_known::account(&account))
                    .cloned();
                let coins: Vec<_> = self.state.coins_of(&account).into_iter().cloned().collect();
                let mut balances: BTreeMap<AssetId, u128> = BTreeMap::new();
                for coin in coins.iter().filter_map(|o| o.as_coin()) {
                    *balances.entry(coin.asset).or_default() += coin.amount.0;
                }
                let owned =
                    |object: &&sion_exec::object::Object| object.owner == Owner::Account(account);
                let allotments = self
                    .state
                    .objects
                    .values()
                    .filter(owned)
                    .filter(|o| matches!(o.data, ObjectData::Allotment(_)))
                    .cloned()
                    .collect();
                let delegations = self
                    .state
                    .objects
                    .values()
                    .filter(owned)
                    .filter(|o| matches!(o.data, ObjectData::Delegation(_)))
                    .cloned()
                    .collect();
                let custody = self
                    .state
                    .objects
                    .values()
                    .filter(owned)
                    .filter(|o| matches!(o.data, ObjectData::Custody(_)))
                    .cloned()
                    .collect();
                let view = AccountView {
                    account,
                    object,
                    coins,
                    balances: balances.into_iter().map(|(a, v)| (a, Amount(v))).collect(),
                    allotments,
                    delegations,
                    custody,
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
            Query::Committees => Ok(crate::committees::encode_view(
                self.state.pulse,
                &self.committees,
                &crate::committees::registered_keys(self.state.validators()),
            )),
            Query::Deals { open } => {
                let pulse = self.state.pulse;
                let index = self.deal_index.get_or_insert_with(|| {
                    self.state
                        .objects
                        .values()
                        .filter(|o| matches!(o.data, ObjectData::Deal(_)))
                        .map(|o| o.id)
                        .collect()
                });
                let objects = &self.state.objects;
                let deals: Vec<Value> = index
                    .iter()
                    .filter_map(|id| match objects.get(id).map(|o| &o.data) {
                        Some(ObjectData::Deal(deal))
                            if !open
                                || deal.state == sion_exec::storage::DealState::Open
                                    && deal.accept_by > pulse =>
                        {
                            Some(json!({ "id": id, "deal": deal }))
                        }
                        _ => None,
                    })
                    .collect();
                Ok(json!({ "pulse": pulse, "deals": deals }))
            }
            Query::Agent(id) => {
                let pulse = self.state.pulse;
                // What matters to a client is the next Pulse, in which a
                // payment would execute.
                let next = pulse.next();
                let charter = match self.state.objects.get(&id).map(|o| &o.data) {
                    Some(ObjectData::Charter(charter)) => charter.clone(),
                    _ => return Err(format!("{id} is not an agent or charter")),
                };
                let ids = charter.lineage.ancestry();
                let mut overlays = Vec::with_capacity(ids.len());
                let (mut revoked, mut suspended, mut stopped, mut narrowed) =
                    (false, false, false, false);
                let mut expired = next > charter.caveats.expires_at;
                for agent in ids.iter().rev() {
                    let overlay_id = sion_exec::charter::overlay_id(agent);
                    let object = self.state.objects.get(&overlay_id).cloned();
                    if let Some(ObjectData::AgentOverlay(o)) = object.as_ref().map(|o| &o.data) {
                        let own = agent == &id;
                        revoked |= own && o.revoked_at.is_some();
                        suspended |= own && o.suspended_by.is_some();
                        stopped |= !own && (o.revoked_at.is_some() || o.suspended_by.is_some());
                        if let Some(n) = &o.narrowing {
                            narrowed = true;
                            expired |= next > n.expires_at;
                        }
                    }
                    let proof = self.snapshot().prove(&overlay_id.0);
                    overlays.push(json!({
                        "agent": agent,
                        "overlay": ObjectProof { id: overlay_id, object, proof },
                    }));
                }
                let mut bucket = json!(null);
                for allotment in &charter.allotments {
                    if let Some(ObjectData::Allotment(a)) =
                        self.state.objects.get(allotment).map(|o| &o.data)
                    {
                        bucket = json!({
                            "allotment": allotment,
                            "remaining": a.remaining,
                            "can_spend_next_pulse": a.bucket.level(next).min(a.remaining),
                            "burst": a.bucket.burst,
                            "rate_per_pulse": a.bucket.rate,
                            "spawned": a.spawned,
                        });
                    }
                }
                let status = if revoked {
                    "revoked"
                } else if suspended {
                    "suspended"
                } else if stopped {
                    "stopped by an ancestor"
                } else if expired {
                    "expired"
                } else if narrowed {
                    "active, narrowed"
                } else {
                    "active"
                };
                let object = self.state.objects.get(&id).cloned();
                let proof = self.snapshot().prove(&id.0);
                Ok(json!({
                    "pulse": pulse,
                    "agent": id,
                    "status": status,
                    "depth": charter.lineage.depth(),
                    "lineage": charter.lineage,
                    "charter": ObjectProof { id, object, proof },
                    "overlays": overlays,
                    "bucket": bucket,
                }))
            }
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
        if let Ok(proof) = sion_core::encode::decode::<EpochProof>(&bytes) {
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
    std::fs::write(&temp, sion_core::encode::encode(proof))?;
    std::fs::rename(temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sion_exec::ExecStatus;

    fn digest(name: &[u8]) -> TxDigest {
        TxDigest::derive(name)
    }

    #[test]
    fn a_rejected_transaction_keeps_its_locks_until_it_expires() {
        let (ok, failed, rejected, stale) = (
            digest(b"ok"),
            digest(b"failed"),
            digest(b"rejected"),
            digest(b"stale"),
        );
        let lock_expiry: BTreeMap<TxDigest, (ShardId, Pulse)> = [
            (ok, (ShardId(0), Pulse(50))),
            (failed, (ShardId(1), Pulse(50))),
            (rejected, (ShardId(0), Pulse(50))),
            (stale, (ShardId(1), Pulse(9))),
        ]
        .into_iter()
        .collect();
        let statuses = [
            (ok, ExecStatus::Success),
            (failed, ExecStatus::Failed("aborted".into())),
            (rejected, ExecStatus::Rejected("input consumed".into())),
        ];
        let executed = || statuses.iter().map(|(d, s)| (d, s));

        // At Pulse 10: executed transactions moved their inputs, so their
        // locks go; `stale` expired at 9; the rejected one still holds its
        // inputs, which keep their versions.
        let mut released = released_locks(executed(), &lock_expiry, Pulse(10));
        released.sort();
        let mut expected = vec![(ShardId(0), ok), (ShardId(1), failed), (ShardId(1), stale)];
        expected.sort();
        assert_eq!(released, expected);
        assert!(!released.iter().any(|(_, d)| *d == rejected));

        // Once it expires, nothing can execute it: its locks go too.
        let released = released_locks(std::iter::empty(), &lock_expiry, Pulse(50));
        assert!(released.contains(&(ShardId(0), rejected)));
        assert_eq!(released.len(), 4);

        // A transaction this validator never locked releases nothing.
        let unknown = (digest(b"unknown"), ExecStatus::Success);
        let released = released_locks([(&unknown.0, &unknown.1)], &lock_expiry, Pulse(0));
        assert!(released.is_empty());
    }
}
