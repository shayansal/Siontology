//! One validator: lane approvals, consensus, Pulse execution, header
//! certificates, data availability and storage, on its own thread.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, VecDeque};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fold_assurance::coc::claim_coc;
use fold_assurance::{decide, Policy};
use fold_consensus::lanes::verify_lane_certificate;
use fold_consensus::LaneCertificate;
use fold_consensus::{
    Action, ApprovalStatus, Authenticator, Committee, Core, HashChain, LaneRequest, LaneValidator,
    Message, SignatureVerifier, TimerKind,
};
use fold_core::hash::Hash32;
use fold_core::ids::{AssetId, Pulse, ShardId, TxDigest, ValidatorId};
use fold_core::merkle;
use fold_core::smt::SmtSnapshot;
use fold_core::Amount;
use fold_da::{DaHeader, ExtendedSquare};
use fold_exec::pulse::{
    effects_leaf, execute_pulse, execute_pulse_traced, ChunkTrace, PulseInput, PulseOutput,
};
use fold_exec::{GenesisSpec, ObjectData, Owner, State};
use fold_light::{ClaimProof, ObjectProof, PulseCertificate, PulseHeader, ReceiptProof};
use fold_prover::PulseData;
use serde_json::{json, Value};

use crate::config::DevnetConfig;
use crate::gateway::GatewayInput;
use crate::keys::{NodeAuthenticator, NodeVerifier};
use crate::owners::StateOwners;
use crate::payload::{expand, lane_inputs, PayloadItem};
use crate::pulses::{ClosedPulse, PulseBuilder};
use crate::storage::{PulseRecord, Snapshot, Wal};
use crate::view::{AccountView, EventView, PulseView, Query, StatusView, TxView};

/// Everything validators share, fixed for a run.
#[derive(Debug)]
pub struct Shared {
    /// Configuration.
    pub config: DevnetConfig,
    /// Genesis.
    pub genesis: GenesisSpec,
    /// Genesis digest (the chain's identity).
    pub genesis_digest: Hash32,
    /// Consensus epoch of this run.
    pub epoch: u64,
    /// The consensus committee (every validator).
    pub committee: Committee,
    /// Lane committees by shard.
    pub shard_committees: BTreeMap<ShardId, Committee>,
    /// Signature verification for every validator.
    pub verifier: NodeVerifier,
    /// Validators in index order.
    pub validators: Vec<ValidatorId>,
    /// Each validator's beacon hash-chain tip.
    pub beacon_tips: Vec<Hash32>,
}

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
}

#[derive(Debug)]
enum Timer {
    Core(TimerKind),
    LaneBatch,
}

#[derive(Debug)]
struct Collector {
    header: PulseHeader,
    signatures: BTreeMap<ValidatorId, Vec<u8>>,
    certificate: Option<PulseCertificate>,
}

#[derive(Debug)]
struct Executed {
    output: PulseOutput,
    square: Option<(ExtendedSquare, DaHeader)>,
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
    prover: Option<Sender<ProverJob>>,
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
    pub prover: Option<Sender<ProverJob>>,
    /// Verified World Proofs (written by the prover).
    pub proven: Arc<std::sync::Mutex<BTreeMap<Pulse, crate::prover::Proven>>>,
}

impl Validator {
    /// Builds a validator.
    pub fn new(shared: Arc<Shared>, setup: ValidatorSetup) -> Validator {
        let id = setup.auth.validator();
        let core = Core::new(
            shared.committee.clone(),
            setup.auth.clone(),
            shared.verifier.clone(),
            shared.config.core.clone(),
        );
        let lanes = shared
            .shard_committees
            .iter()
            .filter(|(_, c)| c.contains(&id))
            .map(|(shard, c)| (*shard, LaneValidator::new(c, setup.auth.clone())))
            .collect();
        let builder = PulseBuilder::new(
            shared.config.rounds_per_pulse,
            setup.state.pulse,
            setup.beacon,
        );
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
            verifier: shared.verifier.clone(),
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

    /// Runs until shut down.
    pub fn run(mut self) {
        let mut actions: VecDeque<Action> = self.core.handle(fold_consensus::Event::Start).into();
        let first = self.builder.next_pulse();
        if let Some(value) = self.chain.reveal_for(first) {
            let reveal = PayloadItem::Reveal {
                validator: self.id,
                pulse: first,
                value,
            };
            actions.extend(
                self.core
                    .handle(fold_consensus::Event::Submit(reveal.to_bytes())),
            );
        }
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
            let _ = wal.snapshot(&Snapshot {
                state: self.state.clone(),
                beacon: self.builder.beacon().clone(),
                epoch: self.shared.epoch,
            });
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
                Some(Timer::Core(kind)) => {
                    let actions = self.core.handle(fold_consensus::Event::Timer(kind));
                    self.apply(actions.into());
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
                let actions = self
                    .core
                    .handle(fold_consensus::Event::Message { from, message });
                self.apply(actions.into());
            }
            Input::Submit(item) => {
                let actions = self.core.handle(fold_consensus::Event::Submit(item));
                self.apply(actions.into());
            }
            Input::Lane { shard, request } => self.on_lane_request(shard, request),
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
                Action::SetTimer { timer, after_ms } => {
                    self.schedule(Duration::from_millis(after_ms), Timer::Core(timer))
                }
                Action::Commit(commit) => {
                    self.anchors += 1;
                    for closed in self.builder.on_commit(&commit) {
                        actions.extend(self.execute(closed));
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
        if let Err(reason) = owners.check(&request) {
            let _ = self.bus.gateway.send(GatewayInput::LaneRejected {
                tx,
                validator: self.id,
                reason,
            });
            return;
        }
        match lane.approve(request, &owners) {
            Ok(ApprovalStatus::Pending) => {
                self.lane_waiters.insert(tx);
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

    fn seal_lane_batches(&mut self) {
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
            }
        }
    }

    // ----- Execution -------------------------------------------------------

    fn execute(&mut self, closed: ClosedPulse) -> Vec<Action> {
        let started = Instant::now();
        let shards = self.shared.config.shards;
        let mut owned = Vec::new();
        let mut contended = Vec::new();
        let mut certificates: BTreeMap<TxDigest, LaneCertificate> = BTreeMap::new();
        for item in closed.items {
            let PayloadItem::Certified { batches, txs } = item else {
                continue;
            };
            for grouped in &txs {
                // The certificate must lock exactly the transaction's owned
                // inputs, with a quorum of its shard's committee.
                let Some(certificate) = expand(&batches, grouped) else {
                    continue;
                };
                let tx = &grouped.tx;
                let shard = ShardId::for_account(&tx.tx.sender, shards);
                let Some(committee) = self.shared.shard_committees.get(&shard) else {
                    continue;
                };
                let inputs = lane_inputs(&tx.tx.inputs);
                if verify_lane_certificate(
                    committee,
                    &tx.digest(),
                    &inputs,
                    &certificate,
                    &self.verifier,
                )
                .is_err()
                {
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
        };
        let traced = self.prover.is_some();
        let result = match (traced, self.shared.config.prove_chunk) {
            (true, Some(chunk)) => execute_pulse_traced(&mut self.state, &input, true, chunk),
            _ => execute_pulse(&mut self.state, &input, true).map(|output| (output, Vec::new())),
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

        let header = PulseHeader {
            chain: self.shared.genesis_digest,
            epoch: self.shared.epoch,
            pulse: input.pulse,
            beacon: input.beacon,
            before: output.before,
            after: output.after,
            order_digest: output.order_digest,
            receipts_root: output.receipts_root,
            data_root,
            data_k,
        };

        if let Some(wal) = &mut self.wal {
            let record = PulseRecord {
                input: input.clone(),
                after: output.after,
                beacon: self.builder.beacon().clone(),
            };
            if let Err(e) = wal.append(&record) {
                eprintln!("validator {}: log append failed: {e}", self.index);
            }
            let every = self.shared.config.snapshot_every.max(1);
            if input.pulse.0 % every == 0 {
                if let Err(e) = wal.snapshot(&Snapshot {
                    state: self.state.clone(),
                    beacon: self.builder.beacon().clone(),
                    epoch: self.shared.epoch,
                }) {
                    eprintln!("validator {}: snapshot failed: {e}", self.index);
                }
            }
        }

        for (index, effects) in output.effects.iter().enumerate() {
            self.tx_index
                .insert(effects.digest, (input.pulse, index as u64));
        }

        // Sign the header and share the signature.
        let signature = self.auth.sign(&header.signing_digest().0);
        let digest = header.digest();
        let mut collector = Collector {
            header: header.clone(),
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

        if let Some(prover) = &self.prover {
            let _ = prover.send(ProverJob {
                header,
                output: output.clone(),
                traces,
                data,
                certificates: ordered_certificates,
            });
        }

        self.history.insert(
            input.pulse,
            Executed {
                output,
                square,
                anchors: closed.anchors,
                revealed: closed.beacon.revealed.len(),
                data_bytes: blob.len(),
                execution_ms: started.elapsed().as_millis() as u64,
            },
        );
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

        // Reveal for the next Pulse.
        let next = input.pulse.next();
        match self.chain.reveal_for(next) {
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

    // ----- Header certificates --------------------------------------------

    fn on_pulse_signature(
        &mut self,
        from: ValidatorId,
        pulse: Pulse,
        header: Hash32,
        signature: Vec<u8>,
    ) {
        let Some(collector) = self.collectors.get_mut(&pulse) else {
            if pulse > self.state.pulse {
                self.early_signatures
                    .entry(pulse)
                    .or_default()
                    .push((from, header, signature));
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
        let committee = &self.shared.committee;
        let Some(collector) = self.collectors.get_mut(&pulse) else {
            return;
        };
        if collector.certificate.is_some() {
            return;
        }
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
                let status = StatusView {
                    chain_id: self.shared.config.chain_id.clone(),
                    genesis: self.shared.genesis_digest,
                    validator: self.id,
                    epoch: self.shared.epoch,
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
                let (_, header) = executed.square.as_ref().ok_or("no data square")?;
                Ok(to_json(header))
            }
            Query::Share { pulse, row, column } => {
                let executed = self.history.get(&pulse).ok_or("unknown or pruned Pulse")?;
                let (square, _) = executed.square.as_ref().ok_or("no data square")?;
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
            Query::Validators => Ok(json!({
                "committee": self.shared.committee,
                "shards": self.shared.shard_committees,
                "keys": self.shared.verifier.set,
                "insecure_signatures": self.shared.config.signatures == crate::keys::SignatureMode::InsecureFast,
                "genesis": self.shared.genesis_digest,
                "epoch": self.shared.epoch,
            })),
            Query::Genesis => Ok(json!({
                "digest": self.shared.genesis_digest,
                "spec": self.shared.genesis,
            })),
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
