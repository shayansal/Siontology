//! A deterministic network simulator for the consensus core.
//!
//! Validators run the real [`Core`] state machine; the simulator owns a
//! message queue in simulated time with seeded random delays. Before the
//! global stabilization time (GST) the network is adversarial: delays are
//! long and messages may be dropped. After GST every message arrives within
//! `max_delay_ms`. Byzantine behaviors are injected around honest cores:
//! crashing, staying silent when voting, sending invalid signatures, and
//! equivocating (two different headers per round, each to half the peers,
//! optionally with double voting).
//!
//! Signatures use the insecure simulation MAC from [`crate::auth`]; this
//! module never runs in production.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

use fold_core::{rng, BlockDigest, Epoch, Hash32, Round, ShardId, ValidatorId};
use rand_chacha::ChaCha20Rng;
use rand_core::RngCore;

use crate::auth::{
    insecure_sim_keyring, Authenticator, InsecureSimAuthenticator, InsecureSimVerifier,
    SignatureVerifier,
};
use crate::committee::Committee;
use crate::core::{item_hash, Action, CommittedSubDag, Core, CoreConfig, Event};
use crate::dag::{Certificate, DagEvidence, Header, Message, SignedHeader, Vote};

/// How a simulated validator behaves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Behavior {
    /// Follows the protocol.
    Honest,
    /// Follows the protocol until `at_ms`, then stops.
    Crash {
        /// Crash time.
        at_ms: u64,
    },
    /// Sends one header to half of its peers and a different header to the
    /// other half every round, collects votes for both, and reveals each
    /// certificate only to its half. With `double_vote` it also votes for
    /// every header it sees, including conflicting ones.
    EquivocatingProposer {
        /// Whether it also double-votes.
        double_vote: bool,
    },
    /// Never sends votes.
    SilentVoter,
    /// Corrupts every signature it sends.
    InvalidSignatures,
}

/// Simulation parameters.
#[derive(Clone, Debug)]
pub struct SimConfig {
    /// Number of validators.
    pub validators: usize,
    /// Per-validator stake (defaults to one each).
    pub stakes: Option<Vec<u64>>,
    /// Seed for keys, delays and drops.
    pub seed: u64,
    /// Global stabilization time.
    pub gst_ms: u64,
    /// Minimum message delay.
    pub min_delay_ms: u64,
    /// Maximum message delay after GST (the network bound Δ).
    pub max_delay_ms: u64,
    /// Maximum message delay before GST.
    pub pre_gst_max_delay_ms: u64,
    /// Drop probability before GST, per mille.
    pub pre_gst_drop_per_mille: u32,
    /// Simulated duration.
    pub duration_ms: u64,
    /// One payload item is submitted every this many milliseconds, to
    /// validators in rotation.
    pub payload_interval_ms: u64,
    /// No payload is submitted after this time, so it can settle.
    pub payload_stop_ms: u64,
    /// Protocol parameters.
    pub core: CoreConfig,
    /// Non-honest behaviors by validator index.
    pub behaviors: BTreeMap<usize, Behavior>,
}

impl SimConfig {
    /// A synchronous network of `validators` honest validators.
    pub fn new(validators: usize, seed: u64) -> Self {
        SimConfig {
            validators,
            stakes: None,
            seed,
            gst_ms: 0,
            min_delay_ms: 5,
            max_delay_ms: 50,
            pre_gst_max_delay_ms: 1_000,
            pre_gst_drop_per_mille: 0,
            duration_ms: 12_000,
            payload_interval_ms: 50,
            payload_stop_ms: 8_000,
            core: CoreConfig::default(),
            behaviors: BTreeMap::new(),
        }
    }

    /// Makes the network adversarial until `gst_ms`.
    pub fn with_gst(mut self, gst_ms: u64, drop_per_mille: u32) -> Self {
        self.gst_ms = gst_ms;
        self.pre_gst_drop_per_mille = drop_per_mille;
        self
    }

    /// Gives validator `index` a behavior.
    pub fn with_behavior(mut self, index: usize, behavior: Behavior) -> Self {
        self.behaviors.insert(index, behavior);
        self
    }

    /// Sets per-validator stakes.
    pub fn with_stakes(mut self, stakes: Vec<u64>) -> Self {
        self.stakes = Some(stakes);
        self
    }
}

/// One commit as observed by one validator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitRecord {
    /// Commit sequence number.
    pub sequence: u64,
    /// Anchor digest.
    pub anchor: BlockDigest,
    /// Anchor round.
    pub round: Round,
    /// Simulated time of the commit.
    pub time_ms: u64,
    /// Hashes of the items it delivered.
    pub items: Vec<Hash32>,
}

/// The outcome of a simulation.
#[derive(Clone, Debug)]
pub struct SimReport {
    /// The committee.
    pub committee: Committee,
    /// Validators that followed the protocol for the whole run.
    pub honest: BTreeSet<ValidatorId>,
    /// Validators with any other behavior.
    pub faulty: BTreeSet<ValidatorId>,
    /// Global stabilization time.
    pub gst_ms: u64,
    /// Commits per validator, in order.
    pub commits: BTreeMap<ValidatorId, Vec<CommitRecord>>,
    /// Submitted items: (hash, validator, time).
    pub submitted: Vec<(Hash32, ValidatorId, u64)>,
    /// Evidence reported, with the reporting validator.
    pub evidence: Vec<(ValidatorId, DagEvidence)>,
    /// Messages delivered.
    pub delivered_messages: u64,
    /// Messages dropped before GST.
    pub dropped_messages: u64,
    /// Highest round reached by any honest validator.
    pub max_round: Round,
    /// Rounds held in each validator's DAG at the end (bounded by garbage
    /// collection).
    pub dag_rounds: BTreeMap<ValidatorId, usize>,
    verifier: InsecureSimVerifier,
}

impl SimReport {
    /// Every pair of honest validators committed prefix-consistent
    /// sequences: the same anchors and the same items in the same order.
    pub fn check_safety(&self) -> Result<(), String> {
        let honest: Vec<&ValidatorId> = self.honest.iter().collect();
        for i in 0..honest.len() {
            for j in i + 1..honest.len() {
                let a = self
                    .commits
                    .get(honest[i])
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let b = self
                    .commits
                    .get(honest[j])
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                for (x, y) in a.iter().zip(b.iter()) {
                    if x.anchor != y.anchor || x.items != y.items {
                        return Err(format!(
                            "validators {} and {} diverge at commit {}",
                            honest[i].short(),
                            honest[j].short(),
                            x.sequence
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// After GST every honest validator committed, and every item submitted
    /// to an honest validator after GST was delivered by every honest
    /// validator.
    pub fn check_liveness(&self) -> Result<(), String> {
        let mut delivered: BTreeMap<ValidatorId, BTreeSet<Hash32>> = BTreeMap::new();
        for validator in &self.honest {
            let commits = self
                .commits
                .get(validator)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            if !commits.iter().any(|c| c.time_ms >= self.gst_ms) {
                return Err(format!(
                    "validator {} never committed after GST",
                    validator.short()
                ));
            }
            delivered.insert(
                *validator,
                commits
                    .iter()
                    .flat_map(|c| c.items.iter().copied())
                    .collect(),
            );
        }
        for (hash, target, time) in &self.submitted {
            if *time < self.gst_ms || !self.honest.contains(target) {
                continue;
            }
            for (validator, items) in &delivered {
                if !items.contains(hash) {
                    return Err(format!(
                        "item {} submitted at {time} ms to {} not delivered by {}",
                        hash.short(),
                        target.short(),
                        validator.short()
                    ));
                }
            }
        }
        Ok(())
    }

    /// Number of commits by a validator.
    pub fn commit_count(&self, validator: &ValidatorId) -> usize {
        self.commits.get(validator).map(Vec::len).unwrap_or(0)
    }

    /// Validators incriminated by any reported evidence.
    pub fn offenders(&self) -> BTreeSet<ValidatorId> {
        self.evidence
            .iter()
            .flat_map(|(_, evidence)| evidence.offenders())
            .collect()
    }

    /// Whether every reported piece of evidence verifies.
    pub fn evidence_verifies(&self) -> bool {
        self.evidence
            .iter()
            .all(|(_, evidence)| evidence.verify(&self.committee, &self.verifier))
    }
}

struct Node {
    id: ValidatorId,
    core: Core<InsecureSimAuthenticator, InsecureSimVerifier>,
    authenticator: InsecureSimAuthenticator,
    behavior: Behavior,
    // Equivocation bookkeeping: original header digest -> alternate header.
    alternates: BTreeMap<BlockDigest, SignedHeader>,
    // Alternate digest -> votes collected for it.
    alternate_votes: BTreeMap<BlockDigest, BTreeMap<ValidatorId, Vec<u8>>>,
    alternate_certified: BTreeSet<BlockDigest>,
    max_round: Round,
}

/// The simulator.
pub struct Simulation {
    config: SimConfig,
    committee: Committee,
    verifier: InsecureSimVerifier,
    nodes: Vec<Node>,
    index_of: BTreeMap<ValidatorId, usize>,
    queue: BinaryHeap<Reverse<(u64, u64)>>,
    events: BTreeMap<u64, (usize, Event)>,
    next_seq: u64,
    now: u64,
    rng: ChaCha20Rng,
    commits: BTreeMap<ValidatorId, Vec<CommitRecord>>,
    submitted: Vec<(Hash32, ValidatorId, u64)>,
    evidence: Vec<(ValidatorId, DagEvidence)>,
    delivered: u64,
    dropped: u64,
}

impl std::fmt::Debug for Simulation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Simulation")
            .field("validators", &self.nodes.len())
            .field("now", &self.now)
            .finish()
    }
}

impl Simulation {
    /// Builds a simulation.
    pub fn new(config: SimConfig) -> Self {
        let (authenticators, verifier) = insecure_sim_keyring(config.validators, config.seed);
        let stakes = config
            .stakes
            .clone()
            .unwrap_or_else(|| vec![1; config.validators]);
        let committee = Committee::new(
            Epoch(0),
            ShardId(0),
            authenticators
                .iter()
                .zip(stakes)
                .map(|(a, s)| (a.validator(), s))
                .collect(),
        )
        .expect("valid simulated committee");
        let nodes: Vec<Node> = authenticators
            .into_iter()
            .enumerate()
            .map(|(index, authenticator)| Node {
                id: authenticator.validator(),
                core: Core::new(
                    committee.clone(),
                    authenticator.clone(),
                    verifier.clone(),
                    config.core.clone(),
                ),
                authenticator,
                behavior: config
                    .behaviors
                    .get(&index)
                    .cloned()
                    .unwrap_or(Behavior::Honest),
                alternates: BTreeMap::new(),
                alternate_votes: BTreeMap::new(),
                alternate_certified: BTreeSet::new(),
                max_round: Round(0),
            })
            .collect();
        let index_of = nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect();
        let rng = rng::labeled(config.seed, "fold-consensus/sim/network");
        Simulation {
            config,
            committee,
            verifier,
            nodes,
            index_of,
            queue: BinaryHeap::new(),
            events: BTreeMap::new(),
            next_seq: 0,
            now: 0,
            rng,
            commits: BTreeMap::new(),
            submitted: Vec::new(),
            evidence: Vec::new(),
            delivered: 0,
            dropped: 0,
        }
    }

    /// Runs a configuration to completion.
    pub fn run_config(config: SimConfig) -> SimReport {
        Simulation::new(config).run()
    }

    fn push(&mut self, time: u64, target: usize, event: Event) {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.queue.push(Reverse((time, seq)));
        self.events.insert(seq, (target, event));
    }

    fn uniform(&mut self, low: u64, high: u64) -> u64 {
        if high <= low {
            return low;
        }
        low + self.rng.next_u64() % (high - low + 1)
    }

    fn is_down(&self, index: usize) -> bool {
        matches!(self.nodes[index].behavior, Behavior::Crash { at_ms } if self.now >= at_ms)
    }

    /// Runs the simulation and returns the report.
    pub fn run(mut self) -> SimReport {
        let n = self.nodes.len();
        for index in 0..n {
            self.push(0, index, Event::Start);
        }
        let mut k = 1u64;
        while self.config.payload_interval_ms > 0
            && k * self.config.payload_interval_ms <= self.config.payload_stop_ms
        {
            let time = k * self.config.payload_interval_ms;
            let target = (k as usize) % n;
            let item = format!("item-{k}").into_bytes();
            self.submitted
                .push((item_hash(&item), self.nodes[target].id, time));
            self.push(time, target, Event::Submit(item));
            k += 1;
        }
        while let Some(Reverse((time, seq))) = self.queue.pop() {
            if time > self.config.duration_ms {
                break;
            }
            self.now = time;
            let (target, event) = self.events.remove(&seq).expect("scheduled event");
            if self.is_down(target) {
                continue;
            }
            if matches!(event, Event::Message { .. }) {
                self.delivered += 1;
            }
            if let Some(event) = self.intercept(target, event) {
                let actions = self.nodes[target].core.handle(event);
                let round = self.nodes[target].core.round();
                if round > self.nodes[target].max_round {
                    self.nodes[target].max_round = round;
                }
                self.apply(target, actions);
            }
        }
        self.report()
    }

    fn report(self) -> SimReport {
        let honest: BTreeSet<ValidatorId> = self
            .nodes
            .iter()
            .filter(|n| n.behavior == Behavior::Honest)
            .map(|n| n.id)
            .collect();
        let faulty = self
            .nodes
            .iter()
            .filter(|n| n.behavior != Behavior::Honest)
            .map(|n| n.id)
            .collect();
        let max_round = self
            .nodes
            .iter()
            .filter(|n| honest.contains(&n.id))
            .map(|n| n.max_round)
            .max()
            .unwrap_or(Round(0));
        let dag_rounds = self
            .nodes
            .iter()
            .map(|n| (n.id, n.core.dag_rounds()))
            .collect();
        SimReport {
            committee: self.committee,
            honest,
            faulty,
            gst_ms: self.config.gst_ms,
            commits: self.commits,
            submitted: self.submitted,
            evidence: self.evidence,
            delivered_messages: self.delivered,
            dropped_messages: self.dropped,
            max_round,
            dag_rounds,
            verifier: self.verifier,
        }
    }

    fn apply(&mut self, from: usize, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Broadcast(message) => {
                    for to in 0..self.nodes.len() {
                        if to != from {
                            self.transmit(from, to, message.clone());
                        }
                    }
                }
                Action::Send { to, message } => {
                    if let Some(index) = self.index_of.get(&to).copied() {
                        if index != from {
                            self.transmit(from, index, message);
                        }
                    }
                }
                Action::SetTimer { timer, after_ms } => {
                    self.push(self.now + after_ms.max(1), from, Event::Timer(timer));
                }
                Action::Commit(sub) => self.record_commit(from, sub),
                Action::Evidence(evidence) => {
                    let reporter = self.nodes[from].id;
                    self.evidence.push((reporter, evidence));
                }
            }
        }
    }

    fn record_commit(&mut self, node: usize, sub: CommittedSubDag) {
        let record = CommitRecord {
            sequence: sub.sequence,
            anchor: sub.anchor,
            round: sub.round,
            time_ms: self.now,
            items: sub.items.iter().map(|item| item_hash(item)).collect(),
        };
        self.commits
            .entry(self.nodes[node].id)
            .or_default()
            .push(record);
    }

    /// Whether `to` is in the second half of `from`'s peers (the half that
    /// an equivocator feeds its alternate headers).
    fn in_second_half(&self, from: usize, to: usize) -> bool {
        let peers: Vec<usize> = (0..self.nodes.len()).filter(|i| *i != from).collect();
        let position = peers.iter().position(|p| *p == to).unwrap_or(0);
        position >= peers.len() / 2
    }

    fn transmit(&mut self, from: usize, to: usize, message: Message) {
        let author = self.nodes[from].id;
        let message = match self.nodes[from].behavior.clone() {
            Behavior::SilentVoter if matches!(message, Message::Vote(_)) => return,
            Behavior::InvalidSignatures => corrupt(message),
            Behavior::EquivocatingProposer { .. } => match message {
                Message::Header(signed) if signed.header.author == author => {
                    let alternate = self.alternate_for(from, &signed);
                    let colluder = matches!(
                        self.nodes[to].behavior,
                        Behavior::EquivocatingProposer { .. }
                    );
                    if colluder {
                        // Colluding equivocators see (and can vote for) both.
                        self.schedule_delivery(from, to, Message::Header(alternate));
                        Message::Header(signed)
                    } else if self.in_second_half(from, to) {
                        Message::Header(alternate)
                    } else {
                        Message::Header(signed)
                    }
                }
                Message::Certificate(certificate)
                    if certificate.author() == author
                        && self.nodes[from]
                            .alternates
                            .contains_key(&certificate.digest()) =>
                {
                    // Only the first half learns the original certificate.
                    if self.in_second_half(from, to) {
                        return;
                    }
                    Message::Certificate(certificate)
                }
                other => other,
            },
            _ => message,
        };
        self.schedule_delivery(from, to, message);
    }

    fn schedule_delivery(&mut self, from: usize, to: usize, message: Message) {
        let delay = if self.now < self.config.gst_ms {
            if self.config.pre_gst_drop_per_mille > 0
                && (self.rng.next_u32() % 1_000) < self.config.pre_gst_drop_per_mille
            {
                self.dropped += 1;
                return;
            }
            let high = self
                .config
                .pre_gst_max_delay_ms
                .max(self.config.min_delay_ms);
            self.uniform(self.config.min_delay_ms, high)
        } else {
            self.uniform(self.config.min_delay_ms, self.config.max_delay_ms)
        };
        let sender = self.nodes[from].id;
        self.push(
            self.now + delay.max(1),
            to,
            Event::Message {
                from: sender,
                message,
            },
        );
    }

    fn alternate_for(&mut self, from: usize, original: &SignedHeader) -> SignedHeader {
        let original_digest = original.header.digest();
        if let Some(alternate) = self.nodes[from].alternates.get(&original_digest) {
            return alternate.clone();
        }
        let mut header: Header = original.header.clone();
        header
            .payload
            .push(format!("equivocation-{}", header.round.0).into_bytes());
        let digest = header.digest();
        let node = &mut self.nodes[from];
        let signature = node.authenticator.sign(&Header::signing_digest(&digest).0);
        let alternate = SignedHeader { header, signature };
        let own_vote = node.authenticator.sign(&Vote::signing_digest(&digest).0);
        node.alternates.insert(original_digest, alternate.clone());
        node.alternate_votes
            .entry(digest)
            .or_default()
            .insert(node.id, own_vote);
        alternate
    }

    /// Byzantine interception of incoming events. Returns the event to pass
    /// to the core, if any.
    fn intercept(&mut self, target: usize, event: Event) -> Option<Event> {
        let Behavior::EquivocatingProposer { double_vote } = self.nodes[target].behavior else {
            return Some(event);
        };
        match &event {
            Event::Message {
                message: Message::Vote(vote),
                ..
            } if self.nodes[target]
                .alternate_votes
                .contains_key(&vote.header) =>
            {
                self.collect_alternate_vote(target, vote.clone());
                None
            }
            Event::Message {
                from,
                message: Message::Header(signed),
            } if double_vote && signed.header.author != self.nodes[target].id => {
                // Vote for every header seen, conflicting or not.
                let digest = signed.header.digest();
                let vote = Vote {
                    header: digest,
                    author: signed.header.author,
                    round: signed.header.round,
                    voter: self.nodes[target].id,
                    signature: self.nodes[target]
                        .authenticator
                        .sign(&Vote::signing_digest(&digest).0),
                };
                if let Some(index) = self.index_of.get(from).copied() {
                    self.schedule_delivery(target, index, Message::Vote(vote));
                }
                Some(event)
            }
            _ => Some(event),
        }
    }

    fn collect_alternate_vote(&mut self, target: usize, vote: Vote) {
        if !self.verifier.verify(
            &vote.voter,
            &Vote::signing_digest(&vote.header).0,
            &vote.signature,
        ) {
            return;
        }
        let digest = vote.header;
        let node = &mut self.nodes[target];
        if node.alternate_certified.contains(&digest) {
            return;
        }
        let votes = node.alternate_votes.entry(digest).or_default();
        votes.insert(vote.voter, vote.signature);
        if !self
            .committee
            .is_quorum(self.committee.stake_of_set(votes.keys()))
        {
            return;
        }
        let Some(alternate) = node
            .alternates
            .values()
            .find(|a| a.header.digest() == digest)
            .cloned()
        else {
            return;
        };
        node.alternate_certified.insert(digest);
        let certificate = Certificate {
            header: alternate.header,
            header_signature: alternate.signature,
            votes: votes.iter().map(|(v, s)| (*v, s.clone())).collect(),
        };
        for to in 0..self.nodes.len() {
            if to != target && self.in_second_half(target, to) {
                self.schedule_delivery(target, to, Message::Certificate(certificate.clone()));
            }
        }
    }
}

fn flip(signature: &mut Vec<u8>) {
    match signature.first_mut() {
        Some(byte) => *byte ^= 0xff,
        None => signature.push(1),
    }
}

fn corrupt(message: Message) -> Message {
    match message {
        Message::Header(mut signed) => {
            flip(&mut signed.signature);
            Message::Header(signed)
        }
        Message::Vote(mut vote) => {
            flip(&mut vote.signature);
            Message::Vote(vote)
        }
        Message::Certificate(mut certificate) => {
            flip(&mut certificate.header_signature);
            Message::Certificate(certificate)
        }
        other => other,
    }
}
