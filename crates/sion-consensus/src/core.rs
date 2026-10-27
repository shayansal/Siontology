//! Contention Consensus: a certified-DAG BFT protocol in the Bullshark
//! family (partially synchronous version).
//!
//! Only contended objects (pools, auctions, shared registries) and the
//! Pulse's set of lane certificates need a total order, so only they pass
//! through here. The protocol has two layers:
//!
//! 1. **The DAG.** Every round, every validator proposes a header that
//!    references certificates from more than two thirds of the previous
//!    round's stake. Because *every* validator proposes every round, a single
//!    honest validator suffices to get an intent included.
//! 2. **Commit rule.** Even rounds have a deterministic leader (the anchor).
//!    An anchor commits once round `r + 1` certificates holding at least the
//!    validity stake (more than one third) reference it. That threshold is
//!    enough because any quorum of round `r + 1` intersects that set, so
//!    every certificate of round `r + 2` and later has a path to the anchor:
//!    every honest validator will order it, either directly or when it
//!    commits a later anchor and walks back along DAG paths.
//!
//! Committing an anchor orders its not-yet-delivered causal history
//! deterministically (by round, then author). Payload items are
//! deduplicated, because a client may hand the same intent to several
//! validators to resist censorship.
//!
//! **Liveness** (after the global stabilization time) comes from Bullshark's
//! waits: in an even round a validator waits for the anchor or a timeout
//! before advancing; in an odd round it waits until more than one third of
//! the stake voted for the previous anchor, or two thirds did not, or a
//! timeout. Headers and certificates are re-broadcast periodically and
//! missing parents are fetched on demand, so messages lost before
//! stabilization do not stall the protocol.
//!
//! The core is a pure, deterministic state machine: `handle(event)` returns
//! actions for the driver (the simulator or a node) to carry out.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sion_core::{hash_domain, hash_parts, BlockDigest, Hash32, Round, ValidatorId};

use crate::auth::{Authenticator, SignatureVerifier};
use crate::committee::Committee;
use crate::dag::{Certificate, DagEvidence, Header, Message, SignedHeader, Vote};

/// Identifies a payload item for deduplication.
pub fn item_hash(item: &[u8]) -> Hash32 {
    hash_domain("sion/consensus/item", item)
}

/// How anchors (leaders of even rounds) are chosen.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum LeaderSchedule {
    /// Members in committee order, one per even round. Appropriate for
    /// sampled committees where every member holds one seat.
    RoundRobin,
    /// A stake-weighted pseudo-random member per even round, derived from a
    /// seed (for example the epoch's beacon value).
    StakeWeighted {
        /// The seed.
        seed: Hash32,
    },
}

/// Protocol parameters.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct CoreConfig {
    /// How long to wait for an anchor (even rounds) or its votes (odd
    /// rounds) before advancing anyway. Should exceed three network delays.
    pub leader_timeout_ms: u64,
    /// How often to re-broadcast uncertified headers and the latest
    /// certificate, and re-request missing parents.
    pub resend_interval_ms: u64,
    /// Rounds of history kept below the last committed anchor.
    pub gc_depth: u64,
    /// Maximum payload items per header.
    pub max_items_per_header: usize,
    /// Maximum payload bytes per header. Honest validators propose no more
    /// and vote for no header with more, so certified rounds, and therefore
    /// Pulses, have bounded size.
    pub max_payload_bytes: usize,
    /// Headers for rounds further ahead than this are ignored (they could
    /// only be retained, never voted for).
    pub max_rounds_ahead: u64,
    /// Headers waiting for missing parents, at most.
    pub max_pending_headers: usize,
    /// Re-propose an item not delivered this many rounds after it was
    /// proposed (its header may have been orphaned).
    pub repropose_after_rounds: u64,
    /// Anchor selection.
    pub leader_schedule: LeaderSchedule,
}

impl Default for CoreConfig {
    fn default() -> Self {
        CoreConfig {
            leader_timeout_ms: 200,
            resend_interval_ms: 400,
            gc_depth: 50,
            max_items_per_header: 1_000,
            max_payload_bytes: 256 * 1024,
            max_rounds_ahead: 64,
            max_pending_headers: 4_096,
            repropose_after_rounds: 8,
            leader_schedule: LeaderSchedule::RoundRobin,
        }
    }
}

/// Timers the core asks its driver to set.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub enum TimerKind {
    /// The Bullshark wait for a round.
    Round(Round),
    /// Periodic re-broadcast and re-sync.
    Resend,
}

/// Inputs to the core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Begin proposing.
    Start,
    /// A protocol message from another validator.
    Message {
        /// The sender.
        from: ValidatorId,
        /// The message.
        message: Message,
    },
    /// A timer set earlier fired.
    Timer(TimerKind),
    /// A client submitted a payload item to this validator.
    Submit(Vec<u8>),
}

/// Outputs of the core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Send to every other committee member.
    Broadcast(Message),
    /// Send to one validator.
    Send {
        /// The recipient.
        to: ValidatorId,
        /// The message.
        message: Message,
    },
    /// Deliver `Event::Timer(timer)` after `after_ms`.
    SetTimer {
        /// The timer.
        timer: TimerKind,
        /// Delay in milliseconds.
        after_ms: u64,
    },
    /// An anchor and its causal history were committed.
    Commit(CommittedSubDag),
    /// Byzantine behavior was detected.
    Evidence(DagEvidence),
}

/// One committed anchor and the certificates it orders.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct CommittedSubDag {
    /// Position in this validator's commit sequence (0, 1, 2, ...).
    pub sequence: u64,
    /// The anchor's digest.
    pub anchor: BlockDigest,
    /// The anchor's author.
    pub anchor_author: ValidatorId,
    /// The anchor's round.
    pub round: Round,
    /// Newly ordered certificates, by round then author.
    pub certificates: Vec<Certificate>,
    /// Newly ordered payload items, first occurrences only.
    pub items: Vec<Vec<u8>>,
}

#[derive(Clone, Debug)]
struct MyHeader {
    digest: BlockDigest,
    signed: SignedHeader,
    votes: BTreeMap<ValidatorId, Vec<u8>>,
    certified: bool,
}

/// One validator's consensus state machine.
pub struct Core<A: Authenticator, V: SignatureVerifier> {
    committee: Committee,
    authenticator: A,
    verifier: V,
    config: CoreConfig,
    me: ValidatorId,
    started: bool,
    // The DAG: certified, causally complete above the GC round.
    dag: BTreeMap<Round, BTreeMap<ValidatorId, (BlockDigest, Certificate)>>,
    index: BTreeMap<BlockDigest, (Round, ValidatorId)>,
    pending_certs: BTreeMap<BlockDigest, (ValidatorId, Certificate)>,
    pending_headers: BTreeMap<BlockDigest, (ValidatorId, SignedHeader)>,
    seen_headers: BTreeMap<(Round, ValidatorId), (BlockDigest, SignedHeader)>,
    // Proposing.
    round: Round,
    my_headers: BTreeMap<Round, MyHeader>,
    mempool: BTreeMap<u64, Vec<u8>>,
    mempool_index: BTreeMap<Hash32, u64>,
    next_item_seq: u64,
    in_flight: BTreeMap<Hash32, Round>,
    // Voting: (round, author) -> digest voted for.
    voted: BTreeMap<(Round, ValidatorId), BlockDigest>,
    timeouts: BTreeSet<Round>,
    // Commit.
    last_committed_round: Round,
    next_sequence: u64,
    delivered: BTreeSet<BlockDigest>,
    delivered_by_round: BTreeMap<Round, Vec<BlockDigest>>,
    delivered_items: BTreeSet<Hash32>,
    gc_round: Round,
    evidence: Vec<DagEvidence>,
    reported: BTreeSet<(u8, Round, ValidatorId)>,
}

impl<A: Authenticator, V: SignatureVerifier> fmt::Debug for Core<A, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Core")
            .field("validator", &self.me)
            .field("round", &self.round)
            .field("last_committed_round", &self.last_committed_round)
            .field("committed", &self.next_sequence)
            .field("dag_rounds", &self.dag.len())
            .field("mempool", &self.mempool.len())
            .finish()
    }
}

impl<A: Authenticator, V: SignatureVerifier> Core<A, V> {
    /// Creates a validator's core with the committee's genesis certificates.
    pub fn new(committee: Committee, authenticator: A, verifier: V, config: CoreConfig) -> Self {
        let me = authenticator.validator();
        let mut core = Core {
            committee,
            authenticator,
            verifier,
            config,
            me,
            started: false,
            dag: BTreeMap::new(),
            index: BTreeMap::new(),
            pending_certs: BTreeMap::new(),
            pending_headers: BTreeMap::new(),
            seen_headers: BTreeMap::new(),
            round: Round(1),
            my_headers: BTreeMap::new(),
            mempool: BTreeMap::new(),
            mempool_index: BTreeMap::new(),
            next_item_seq: 0,
            in_flight: BTreeMap::new(),
            voted: BTreeMap::new(),
            timeouts: BTreeSet::new(),
            last_committed_round: Round(0),
            next_sequence: 0,
            delivered: BTreeSet::new(),
            delivered_by_round: BTreeMap::new(),
            delivered_items: BTreeSet::new(),
            gc_round: Round(0),
            evidence: Vec::new(),
            reported: BTreeSet::new(),
        };
        for certificate in Certificate::genesis(&core.committee) {
            let digest = certificate.digest();
            core.index.insert(digest, (Round(0), certificate.author()));
            core.dag
                .entry(Round(0))
                .or_default()
                .insert(certificate.author(), (digest, certificate));
        }
        core
    }

    /// This validator.
    pub fn validator(&self) -> ValidatorId {
        self.me
    }

    /// Whether this validator is a member of the committee. A non-member is
    /// an **observer**: it never proposes or votes, but it accepts
    /// certificates and commits exactly what members commit, so it can
    /// follow the chain (for example while waiting to join at an epoch
    /// boundary).
    pub fn is_member(&self) -> bool {
        self.committee.contains(&self.me)
    }

    /// The committee.
    pub fn committee(&self) -> &Committee {
        &self.committee
    }

    /// The round this validator is proposing in.
    pub fn round(&self) -> Round {
        self.round
    }

    /// The round of the last committed anchor.
    pub fn last_committed_round(&self) -> Round {
        self.last_committed_round
    }

    /// Number of anchors committed so far.
    pub fn committed_count(&self) -> u64 {
        self.next_sequence
    }

    /// The garbage-collection round: history at or below it is discarded.
    pub fn gc_round(&self) -> Round {
        self.gc_round
    }

    /// Number of rounds held in the DAG.
    pub fn dag_rounds(&self) -> usize {
        self.dag.len()
    }

    /// Items waiting to be committed.
    pub fn mempool_len(&self) -> usize {
        self.mempool.len()
    }

    /// Removes and returns every item not committed yet, in submission
    /// order: what a validator proposes again when this session ends.
    pub fn take_mempool(&mut self) -> Vec<Vec<u8>> {
        self.mempool_index.clear();
        self.in_flight.clear();
        std::mem::take(&mut self.mempool).into_values().collect()
    }

    /// Evidence detected so far.
    pub fn evidence(&self) -> &[DagEvidence] {
        &self.evidence
    }

    /// The anchor (leader) of an even round.
    pub fn leader(&self, round: Round) -> ValidatorId {
        match &self.config.leader_schedule {
            LeaderSchedule::RoundRobin => self.committee.member_at((round.0 / 2) as usize),
            LeaderSchedule::StakeWeighted { seed } => {
                let digest =
                    hash_parts("sion/consensus/leader", &[&seed.0, &round.0.to_be_bytes()]);
                let mut target = digest.prefix_u64() % self.committee.total_stake();
                for (validator, stake) in self.committee.members() {
                    if target < *stake {
                        return *validator;
                    }
                    target -= stake;
                }
                self.committee.member_at(0)
            }
        }
    }

    /// Whether a certificate is in this validator's DAG.
    pub fn has_certificate(&self, digest: &BlockDigest) -> bool {
        self.index.contains_key(digest)
    }

    /// Processes one event and returns the resulting actions.
    pub fn handle(&mut self, event: Event) -> Vec<Action> {
        let mut out = Vec::new();
        match event {
            Event::Start => {
                if !self.started && self.is_member() {
                    self.started = true;
                    self.propose(&mut out);
                    out.push(Action::SetTimer {
                        timer: TimerKind::Round(self.round),
                        after_ms: self.config.leader_timeout_ms,
                    });
                    out.push(Action::SetTimer {
                        timer: TimerKind::Resend,
                        after_ms: self.config.resend_interval_ms,
                    });
                }
            }
            Event::Submit(item) => self.submit(item),
            Event::Message { from, message } => match message {
                Message::Header(signed) => self.on_header(from, signed, &mut out),
                Message::Vote(vote) => self.on_vote(vote, &mut out),
                Message::Certificate(certificate) => {
                    self.on_certificate(from, certificate, &mut out)
                }
                Message::SyncRequest(digests) => self.on_sync_request(from, &digests, &mut out),
            },
            Event::Timer(TimerKind::Round(round)) => {
                if round == self.round {
                    self.timeouts.insert(round);
                }
            }
            Event::Timer(TimerKind::Resend) => {
                self.resend(&mut out);
                out.push(Action::SetTimer {
                    timer: TimerKind::Resend,
                    after_ms: self.config.resend_interval_ms,
                });
            }
        }
        self.progress(&mut out);
        out
    }

    fn progress(&mut self, out: &mut Vec<Action>) {
        self.vote_on_ready_headers(out);
        self.try_commit(out);
        if self.started {
            self.try_advance(out);
        }
    }

    // ----- Payload -----------------------------------------------------

    fn submit(&mut self, item: Vec<u8>) {
        // An item that cannot fit in any header would never be proposed.
        if item.len() > self.config.max_payload_bytes {
            return;
        }
        let hash = item_hash(&item);
        if self.delivered_items.contains(&hash) || self.mempool_index.contains_key(&hash) {
            return;
        }
        let seq = self.next_item_seq;
        self.next_item_seq += 1;
        self.mempool.insert(seq, item);
        self.mempool_index.insert(hash, seq);
    }

    fn select_items(&mut self, round: Round) -> Vec<Vec<u8>> {
        let mut chosen = Vec::new();
        let mut bytes = 0usize;
        for item in self.mempool.values() {
            if chosen.len() >= self.config.max_items_per_header {
                break;
            }
            if bytes + item.len() > self.config.max_payload_bytes {
                continue;
            }
            let hash = item_hash(item);
            if let Some(proposed) = self.in_flight.get(&hash) {
                if round.0 < proposed.0 + self.config.repropose_after_rounds {
                    continue;
                }
            }
            bytes += item.len();
            chosen.push((hash, item.clone()));
        }
        for (hash, _) in &chosen {
            self.in_flight.insert(*hash, round);
        }
        chosen.into_iter().map(|(_, item)| item).collect()
    }

    // ----- Proposing and voting -----------------------------------------

    fn propose(&mut self, out: &mut Vec<Action>) {
        let round = self.round;
        let mut parents: Vec<BlockDigest> = self
            .dag
            .get(&Round(round.0 - 1))
            .map(|certs| certs.values().map(|(digest, _)| *digest).collect())
            .unwrap_or_default();
        parents.sort();
        let payload = self.select_items(round);
        let header = Header {
            author: self.me,
            chain: self.committee.chain(),
            round,
            epoch: self.committee.epoch(),
            shard: self.committee.shard(),
            parents,
            payload,
        };
        let digest = header.digest();
        let signature = self.authenticator.sign(&Header::signing_digest(&digest).0);
        let signed = SignedHeader { header, signature };
        self.seen_headers
            .insert((round, self.me), (digest, signed.clone()));
        self.voted.insert((round, self.me), digest);
        let mut votes = BTreeMap::new();
        votes.insert(
            self.me,
            self.authenticator.sign(&Vote::signing_digest(&digest).0),
        );
        self.my_headers.insert(
            round,
            MyHeader {
                digest,
                signed: signed.clone(),
                votes,
                certified: false,
            },
        );
        out.push(Action::Broadcast(Message::Header(signed)));
        self.maybe_certify(round, out);
    }

    fn maybe_certify(&mut self, round: Round, out: &mut Vec<Action>) {
        let Some(mine) = self.my_headers.get_mut(&round) else {
            return;
        };
        if mine.certified
            || !self
                .committee
                .is_quorum(self.committee.stake_of_set(mine.votes.keys()))
        {
            return;
        }
        mine.certified = true;
        let certificate = Certificate {
            header: mine.signed.header.clone(),
            header_signature: mine.signed.signature.clone(),
            votes: mine.votes.iter().map(|(v, s)| (*v, s.clone())).collect(),
        };
        let digest = mine.digest;
        self.accept_certificate(self.me, certificate.clone(), digest, out);
        out.push(Action::Broadcast(Message::Certificate(certificate)));
    }

    fn make_vote(&self, header: &Header, digest: BlockDigest) -> Vote {
        Vote {
            header: digest,
            author: header.author,
            round: header.round,
            voter: self.me,
            signature: self.authenticator.sign(&Vote::signing_digest(&digest).0),
        }
    }

    fn on_header(&mut self, from: ValidatorId, signed: SignedHeader, out: &mut Vec<Action>) {
        if !self.is_member() {
            // Observers neither vote nor gather votes; certificates suffice.
            return;
        }
        let header = &signed.header;
        if header.chain != self.committee.chain()
            || header.epoch != self.committee.epoch()
            || header.shard != self.committee.shard()
            || !self.committee.contains(&header.author)
            || header.round == Round(0)
            || header.round <= self.gc_round
            || header.round.0 > self.round.0.saturating_add(self.config.max_rounds_ahead)
            || header.author == self.me
            || header.payload.len() > self.config.max_items_per_header
            || header.payload.iter().map(Vec::len).sum::<usize>() > self.config.max_payload_bytes
        {
            return;
        }
        if !signed.verify_signature(&self.verifier) {
            return;
        }
        let digest = header.digest();
        let slot = (header.round, header.author);
        let seen = self.seen_headers.get(&slot).cloned();
        match seen {
            Some((seen_digest, seen)) if seen_digest != digest => {
                let evidence = DagEvidence::HeaderEquivocation {
                    first: seen,
                    second: signed.clone(),
                };
                self.report(0, slot, evidence, out);
                return;
            }
            Some(_) => {}
            None => {
                self.seen_headers.insert(slot, (digest, signed.clone()));
            }
        }
        if let Some(voted) = self.voted.get(&slot) {
            if *voted == digest {
                let vote = self.make_vote(header, digest);
                out.push(Action::Send {
                    to: header.author,
                    message: Message::Vote(vote),
                });
            }
            return;
        }
        let missing: Vec<BlockDigest> = header
            .parents
            .iter()
            .filter(|p| !self.index.contains_key(p))
            .copied()
            .collect();
        if missing.is_empty() {
            self.vote_for(digest, &signed, out);
        } else if self.pending_headers.len() < self.config.max_pending_headers {
            self.pending_headers.insert(digest, (from, signed.clone()));
            out.push(Action::Send {
                to: from,
                message: Message::SyncRequest(missing),
            });
        }
    }

    fn parents_valid(&self, header: &Header) -> bool {
        if !header.parents.windows(2).all(|w| w[0] < w[1]) {
            return false;
        }
        let expected = Round(header.round.0 - 1);
        let mut authors = BTreeSet::new();
        for parent in &header.parents {
            match self.index.get(parent) {
                Some((round, author)) if *round == expected => {
                    if !authors.insert(*author) {
                        return false;
                    }
                }
                _ => return false,
            }
        }
        self.committee
            .is_quorum(self.committee.stake_of_set(authors.iter()))
    }

    fn vote_for(&mut self, digest: BlockDigest, signed: &SignedHeader, out: &mut Vec<Action>) {
        let header = &signed.header;
        let slot = (header.round, header.author);
        if self.voted.contains_key(&slot) {
            return;
        }
        // Parents must be known certificates of the previous round holding a
        // quorum. Parents already garbage-collected fail this check, which
        // only means we skip voting for a header that old.
        if !self.parents_valid(header) {
            return;
        }
        self.voted.insert(slot, digest);
        let vote = self.make_vote(header, digest);
        out.push(Action::Send {
            to: header.author,
            message: Message::Vote(vote),
        });
    }

    fn vote_on_ready_headers(&mut self, out: &mut Vec<Action>) {
        let ready: Vec<BlockDigest> = self
            .pending_headers
            .iter()
            .filter(|(_, (_, signed))| {
                signed
                    .header
                    .parents
                    .iter()
                    .all(|p| self.index.contains_key(p))
            })
            .map(|(digest, _)| *digest)
            .collect();
        for digest in ready {
            if let Some((_, signed)) = self.pending_headers.remove(&digest) {
                self.vote_for(digest, &signed, out);
            }
        }
    }

    fn on_vote(&mut self, vote: Vote, out: &mut Vec<Action>) {
        if vote.author != self.me || !self.committee.contains(&vote.voter) {
            return;
        }
        let Some(mine) = self.my_headers.get_mut(&vote.round) else {
            return;
        };
        if mine.digest != vote.header || mine.certified {
            return;
        }
        if !self.verifier.verify(
            &vote.voter,
            &Vote::signing_digest(&vote.header).0,
            &vote.signature,
        ) {
            return;
        }
        mine.votes.insert(vote.voter, vote.signature);
        self.maybe_certify(vote.round, out);
    }

    // ----- Certificates and the DAG -------------------------------------

    fn on_certificate(
        &mut self,
        from: ValidatorId,
        certificate: Certificate,
        out: &mut Vec<Action>,
    ) {
        let round = certificate.round();
        if round == Round(0) || round <= self.gc_round {
            return;
        }
        let digest = certificate.digest();
        if self.index.contains_key(&digest) || self.pending_certs.contains_key(&digest) {
            return;
        }
        if certificate.verify(&self.committee, &self.verifier).is_err() {
            return;
        }
        self.accept_certificate(from, certificate, digest, out);
    }

    fn accept_certificate(
        &mut self,
        from: ValidatorId,
        certificate: Certificate,
        digest: BlockDigest,
        out: &mut Vec<Action>,
    ) {
        let slot = (certificate.round(), certificate.author());
        let existing = self
            .dag
            .get(&slot.0)
            .and_then(|certs| certs.get(&slot.1))
            .cloned();
        if let Some((existing_digest, existing)) = existing {
            if existing_digest != digest {
                let evidence = DagEvidence::ConflictingCertificates {
                    first: existing,
                    second: certificate,
                };
                self.report(1, slot, evidence, out);
            }
            return;
        }
        let parents_collected = certificate.round().0.saturating_sub(1) <= self.gc_round.0;
        let missing: Vec<BlockDigest> = certificate
            .header
            .parents
            .iter()
            .filter(|p| !self.index.contains_key(p))
            .copied()
            .collect();
        if missing.is_empty() || parents_collected {
            self.insert_into_dag(digest, certificate);
            self.drain_pending_certificates(out);
        } else {
            self.pending_certs.insert(digest, (from, certificate));
            out.push(Action::Send {
                to: from,
                message: Message::SyncRequest(missing),
            });
        }
    }

    fn insert_into_dag(&mut self, digest: BlockDigest, certificate: Certificate) {
        let round = certificate.round();
        let author = certificate.author();
        self.index.insert(digest, (round, author));
        self.dag
            .entry(round)
            .or_default()
            .insert(author, (digest, certificate));
    }

    fn drain_pending_certificates(&mut self, out: &mut Vec<Action>) {
        loop {
            let gc = self.gc_round;
            let ready: Vec<BlockDigest> = self
                .pending_certs
                .iter()
                .filter(|(_, (_, cert))| {
                    cert.round().0.saturating_sub(1) <= gc.0
                        || cert
                            .header
                            .parents
                            .iter()
                            .all(|p| self.index.contains_key(p))
                })
                .map(|(digest, _)| *digest)
                .collect();
            if ready.is_empty() {
                return;
            }
            for digest in ready {
                if let Some((from, certificate)) = self.pending_certs.remove(&digest) {
                    let slot = (certificate.round(), certificate.author());
                    let existing = self
                        .dag
                        .get(&slot.0)
                        .and_then(|certs| certs.get(&slot.1))
                        .cloned();
                    match existing {
                        Some((existing_digest, existing)) => {
                            if existing_digest != digest {
                                let evidence = DagEvidence::ConflictingCertificates {
                                    first: existing,
                                    second: certificate,
                                };
                                self.report(1, slot, evidence, out);
                            }
                        }
                        None => {
                            let _ = from;
                            self.insert_into_dag(digest, certificate);
                        }
                    }
                }
            }
        }
    }

    fn on_sync_request(
        &mut self,
        from: ValidatorId,
        digests: &[BlockDigest],
        out: &mut Vec<Action>,
    ) {
        for digest in digests.iter().take(64) {
            if let Some((round, author)) = self.index.get(digest) {
                if *round == Round(0) {
                    continue;
                }
                if let Some((_, certificate)) = self.dag.get(round).and_then(|c| c.get(author)) {
                    out.push(Action::Send {
                        to: from,
                        message: Message::Certificate(certificate.clone()),
                    });
                }
            }
        }
    }

    fn resend(&mut self, out: &mut Vec<Action>) {
        let recent = Round(self.round.0.saturating_sub(1));
        for mine in self.my_headers.range(recent..).map(|(_, m)| m) {
            if !mine.certified {
                out.push(Action::Broadcast(Message::Header(mine.signed.clone())));
            }
        }
        let latest = self
            .dag
            .iter()
            .rev()
            .find_map(|(_, certs)| certs.get(&self.me).map(|(_, c)| c.clone()));
        if let Some(certificate) = latest {
            if certificate.round() > Round(0) {
                out.push(Action::Broadcast(Message::Certificate(certificate)));
            }
        }
        let mut missing = BTreeSet::new();
        for (_, certificate) in self.pending_certs.values() {
            missing.extend(
                certificate
                    .header
                    .parents
                    .iter()
                    .filter(|p| !self.index.contains_key(p))
                    .copied(),
            );
        }
        for (_, signed) in self.pending_headers.values() {
            missing.extend(
                signed
                    .header
                    .parents
                    .iter()
                    .filter(|p| !self.index.contains_key(p))
                    .copied(),
            );
        }
        if !missing.is_empty() {
            out.push(Action::Broadcast(Message::SyncRequest(
                missing.into_iter().take(64).collect(),
            )));
        }
    }

    // ----- Round advancement ---------------------------------------------

    fn round_stake(&self, round: Round) -> u64 {
        self.dag
            .get(&round)
            .map(|certs| self.committee.stake_of_set(certs.keys()))
            .unwrap_or(0)
    }

    fn highest_quorum_round(&self) -> Round {
        self.dag
            .keys()
            .rev()
            .find(|round| self.committee.is_quorum(self.round_stake(**round)))
            .copied()
            .unwrap_or(Round(0))
    }

    fn wait_satisfied(&self, round: Round) -> bool {
        if self.timeouts.contains(&round) {
            return true;
        }
        if round.0 % 2 == 0 {
            // Anchor round: wait for the leader's certificate.
            self.dag
                .get(&round)
                .is_some_and(|certs| certs.contains_key(&self.leader(round)))
        } else {
            // Vote round: wait until the previous anchor is decided either way.
            let previous = Round(round.0 - 1);
            if previous.0 == 0 {
                return true;
            }
            let Some(anchor) = self
                .dag
                .get(&previous)
                .and_then(|certs| certs.get(&self.leader(previous)))
                .map(|(digest, _)| *digest)
            else {
                return true;
            };
            let (mut votes, mut non_votes) = (0u64, 0u64);
            if let Some(certs) = self.dag.get(&round) {
                for (author, (_, certificate)) in certs {
                    if certificate.header.parents.contains(&anchor) {
                        votes += self.committee.stake_of(author);
                    } else {
                        non_votes += self.committee.stake_of(author);
                    }
                }
            }
            self.committee.is_valid_stake(votes) || self.committee.is_quorum(non_votes)
        }
    }

    fn try_advance(&mut self, out: &mut Vec<Action>) {
        // Bound the loop by rounds already present, so a committee whose own
        // certificate forms instantly (a single validator) still advances one
        // round per event rather than looping forever.
        let target = self.highest_quorum_round();
        while self.round <= target {
            let round = self.round;
            if !self.committee.is_quorum(self.round_stake(round)) || !self.wait_satisfied(round) {
                break;
            }
            self.round = Round(round.0 + 1);
            self.propose(out);
            out.push(Action::SetTimer {
                timer: TimerKind::Round(self.round),
                after_ms: self.config.leader_timeout_ms,
            });
        }
    }

    // ----- Commit rule -----------------------------------------------------

    fn anchor_at(&self, round: Round) -> Option<BlockDigest> {
        self.dag
            .get(&round)
            .and_then(|certs| certs.get(&self.leader(round)))
            .map(|(digest, _)| *digest)
    }

    fn support(&self, anchor: &BlockDigest, round: Round) -> u64 {
        self.dag
            .get(&Round(round.0 + 1))
            .map(|certs| {
                certs
                    .iter()
                    .filter(|(_, (_, c))| c.header.parents.contains(anchor))
                    .map(|(author, _)| self.committee.stake_of(author))
                    .sum()
            })
            .unwrap_or(0)
    }

    fn try_commit(&mut self, out: &mut Vec<Action>) {
        loop {
            let max_round = self.dag.keys().next_back().copied().unwrap_or(Round(0));
            let mut candidate = Round(self.last_committed_round.0 + 2);
            let mut committed = false;
            while candidate.0 < max_round.0 {
                if let Some(anchor) = self.anchor_at(candidate) {
                    if self
                        .committee
                        .is_valid_stake(self.support(&anchor, candidate))
                    {
                        self.commit_anchor(candidate, anchor, out);
                        committed = true;
                        break;
                    }
                }
                candidate = Round(candidate.0 + 2);
            }
            if !committed {
                return;
            }
        }
    }

    fn path_exists(&self, from: &BlockDigest, to: &BlockDigest) -> bool {
        let Some((to_round, _)) = self.index.get(to).copied() else {
            return false;
        };
        let mut stack = vec![*from];
        let mut seen = BTreeSet::new();
        while let Some(digest) = stack.pop() {
            if digest == *to {
                return true;
            }
            if !seen.insert(digest) {
                continue;
            }
            let Some((round, author)) = self.index.get(&digest).copied() else {
                continue;
            };
            if round <= to_round {
                continue;
            }
            if let Some((_, certificate)) = self.dag.get(&round).and_then(|c| c.get(&author)) {
                stack.extend(certificate.header.parents.iter().copied());
            }
        }
        false
    }

    fn commit_anchor(&mut self, round: Round, anchor: BlockDigest, out: &mut Vec<Action>) {
        // Walk back to earlier, uncommitted anchors linked by DAG paths.
        let mut chain = vec![(round, anchor)];
        let mut current = anchor;
        let mut candidate = round.0;
        while candidate >= self.last_committed_round.0 + 4 {
            candidate -= 2;
            let previous_round = Round(candidate);
            if let Some(previous) = self.anchor_at(previous_round) {
                if self.path_exists(&current, &previous) {
                    chain.push((previous_round, previous));
                    current = previous;
                }
            }
        }
        for (anchor_round, anchor_digest) in chain.into_iter().rev() {
            let certificates = self.order_causal_history(&anchor_digest, anchor_round);
            let mut items = Vec::new();
            for certificate in &certificates {
                let digest = certificate.digest();
                self.delivered.insert(digest);
                self.delivered_by_round
                    .entry(certificate.round())
                    .or_default()
                    .push(digest);
                for item in &certificate.header.payload {
                    let hash = item_hash(item);
                    if self.delivered_items.insert(hash) {
                        items.push(item.clone());
                    }
                    if let Some(seq) = self.mempool_index.remove(&hash) {
                        self.mempool.remove(&seq);
                    }
                    self.in_flight.remove(&hash);
                }
            }
            let anchor_author = self
                .index
                .get(&anchor_digest)
                .map(|(_, author)| *author)
                .unwrap_or_default();
            out.push(Action::Commit(CommittedSubDag {
                sequence: self.next_sequence,
                anchor: anchor_digest,
                anchor_author,
                round: anchor_round,
                certificates,
                items,
            }));
            self.next_sequence += 1;
        }
        self.last_committed_round = round;
        let gc = Round(round.0.saturating_sub(self.config.gc_depth));
        if gc > self.gc_round {
            self.gc_round = gc;
            self.garbage_collect();
        }
    }

    fn order_causal_history(&self, anchor: &BlockDigest, anchor_round: Round) -> Vec<Certificate> {
        // The cutoff depends only on the anchor's round, so every honest
        // validator orders exactly the same certificates for this anchor.
        let cutoff = anchor_round.0.saturating_sub(self.config.gc_depth);
        let mut stack = vec![*anchor];
        let mut seen = BTreeSet::new();
        let mut collected: Vec<(Round, ValidatorId, Certificate)> = Vec::new();
        while let Some(digest) = stack.pop() {
            if !seen.insert(digest) || self.delivered.contains(&digest) {
                continue;
            }
            let Some((round, author)) = self.index.get(&digest).copied() else {
                continue;
            };
            if round.0 <= cutoff {
                continue;
            }
            if let Some((_, certificate)) = self.dag.get(&round).and_then(|c| c.get(&author)) {
                stack.extend(certificate.header.parents.iter().copied());
                collected.push((round, author, certificate.clone()));
            }
        }
        collected.sort_by_key(|(round, author, _)| (*round, *author));
        collected.into_iter().map(|(_, _, c)| c).collect()
    }

    fn garbage_collect(&mut self) {
        let gc = self.gc_round;
        let old: Vec<Round> = self.dag.range(..=gc).map(|(round, _)| *round).collect();
        for round in old {
            if let Some(certs) = self.dag.remove(&round) {
                for (digest, _) in certs.values() {
                    self.index.remove(digest);
                }
            }
        }
        let old_delivered: Vec<Round> = self
            .delivered_by_round
            .range(..=gc)
            .map(|(round, _)| *round)
            .collect();
        for round in old_delivered {
            if let Some(digests) = self.delivered_by_round.remove(&round) {
                for digest in digests {
                    self.delivered.remove(&digest);
                }
            }
        }
        self.pending_certs.retain(|_, (_, c)| c.round() > gc);
        self.pending_headers.retain(|_, (_, h)| h.header.round > gc);
        self.seen_headers.retain(|(round, _), _| *round > gc);
        self.voted.retain(|(round, _), _| *round > gc);
        self.my_headers.retain(|round, _| *round > gc);
        self.timeouts.retain(|round| *round > gc);
        self.reported.retain(|(_, round, _)| *round > gc);
    }

    fn report(
        &mut self,
        kind: u8,
        slot: (Round, ValidatorId),
        evidence: DagEvidence,
        out: &mut Vec<Action>,
    ) {
        if self.reported.insert((kind, slot.0, slot.1)) {
            self.evidence.push(evidence.clone());
            out.push(Action::Evidence(evidence));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::insecure_sim_keyring;
    use sion_core::{Epoch, ShardId};

    #[test]
    fn single_validator_commits_by_timeouts() {
        let (keys, verifier) = insecure_sim_keyring(1, 3);
        let committee =
            Committee::equal_weight(Epoch(0), ShardId(0), keys.iter().map(|k| k.validator()))
                .unwrap();
        let mut core = Core::new(committee, keys[0].clone(), verifier, CoreConfig::default());
        core.handle(Event::Submit(b"hello".to_vec()));
        core.handle(Event::Start);
        let mut items = Vec::new();
        for _ in 0..12 {
            let round = core.round();
            for action in core.handle(Event::Timer(TimerKind::Round(round))) {
                if let Action::Commit(sub) = action {
                    items.extend(sub.items);
                }
            }
        }
        assert!(core.committed_count() > 0);
        assert_eq!(items, vec![b"hello".to_vec()]);
    }

    #[test]
    fn leaders_rotate_and_weighted_leaders_follow_stake() {
        let (keys, verifier) = insecure_sim_keyring(4, 9);
        let committee =
            Committee::equal_weight(Epoch(0), ShardId(0), keys.iter().map(|k| k.validator()))
                .unwrap();
        let core = Core::new(
            committee.clone(),
            keys[0].clone(),
            verifier.clone(),
            CoreConfig::default(),
        );
        let leaders: BTreeSet<ValidatorId> = (1..=4).map(|i| core.leader(Round(2 * i))).collect();
        assert_eq!(leaders.len(), 4);

        let heavy = Committee::new(
            Epoch(0),
            ShardId(0),
            vec![
                (keys[0].validator(), 97),
                (keys[1].validator(), 1),
                (keys[2].validator(), 1),
                (keys[3].validator(), 1),
            ],
        )
        .unwrap();
        let config = CoreConfig {
            leader_schedule: LeaderSchedule::StakeWeighted {
                seed: hash_domain("test", b"seed"),
            },
            ..CoreConfig::default()
        };
        let core = Core::new(heavy, keys[0].clone(), verifier, config);
        let heavy_count = (1..=200)
            .filter(|i| core.leader(Round(2 * i)) == keys[0].validator())
            .count();
        assert!(
            heavy_count > 170,
            "stake-weighted leader picked {heavy_count}/200"
        );
    }
}
