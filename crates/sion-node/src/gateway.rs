//! The client gateway: routes transactions by coordination class.
//!
//! Every transaction goes to its sender's shard committee, which locks its
//! owned inputs; the gateway collects batch-signed approvals until they form
//! a certificate (more than two thirds of the committee). An owned-path
//! transaction is then final, and the certificate goes to consensus only so
//! a Pulse executes it; a contended transaction is then ordered by
//! consensus. Because contended transactions lock their owned inputs too, a
//! consensus-ordered spend can never overtake a certified one. The devnet
//! faucet batches its grants into one transaction per Pulse.
//!
//! Committees come from the state and change: lane committees at epoch
//! boundaries and rotations, the consensus committee (and its session) at
//! epoch boundaries. The gateway follows the latest committees: approvals
//! from a lane committee that has been replaced no longer count, so it
//! certifies those transactions again with the new committee; and
//! certified transactions that a new session dropped before ordering are
//! handed to consensus again, until they execute.

use std::collections::{BTreeMap, VecDeque};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use sion_consensus::{ApprovalProof, CertificateBuilder, Committee, LaneCertificate, LaneRequest};
use sion_core::encode::encode;
use sion_core::ids::{AccountId, AssetId, Epoch, Pulse, ShardId, TxDigest, ValidatorId};
use sion_core::Amount;
use sion_exec::object::well_known;
use sion_exec::{ObjectData, SignedTransaction, Transaction, TxKind, Wallet};

use crate::payload::{lane_inputs, PayloadItem};
use crate::transport::{LaneMember, NodeId, PeerMessage, Transport};
use crate::validator::Shared;
use crate::view::{Query, SubmitReceipt, TxStatus};

/// Hands the members of one batch to a gateway and waits for its receipts.
pub fn submit_batch_to(
    gateway: &std::sync::mpsc::Sender<GatewayInput>,
    members: Vec<SignedTransaction>,
    timeout: Duration,
) -> Result<Vec<SubmitReceipt>, String> {
    let (reply, answer) = std::sync::mpsc::channel();
    gateway
        .send(GatewayInput::SubmitBatch { members, reply })
        .map_err(|_| "gateway stopped".to_string())?;
    answer
        .recv_timeout(timeout)
        .map_err(|_| "gateway did not answer".to_string())?
}

/// How a gateway asks a validator about the state (the faucet's account,
/// whether its grants executed): a channel to a validator thread in the same
/// process, or a node's HTTP API.
pub type Querier = Box<dyn Fn(Query) -> Result<Value, String> + Send>;

/// Messages to the gateway.
#[derive(Debug)]
pub enum GatewayInput {
    /// A client submits a transaction.
    Submit {
        /// The transaction.
        tx: Box<SignedTransaction>,
        /// Where to answer.
        reply: Sender<Result<SubmitReceipt, String>>,
    },
    /// A client submits the members of one batch (spec v1.13): each
    /// shard's committee gets their lane requests in one message, the
    /// batch's signatures once.
    SubmitBatch {
        /// The members, all of one batch.
        members: Vec<SignedTransaction>,
        /// Where to answer, a receipt per member.
        reply: Sender<Result<Vec<SubmitReceipt>, String>>,
    },
    /// A message from a validator (an approval, a refusal or an execution
    /// report), with the sender as the transport stamped it.
    Peer {
        /// Sender.
        from: NodeId,
        /// Message.
        message: Box<PeerMessage>,
    },
    /// Faucet grants (devnet only).
    Faucet {
        /// Recipients.
        grants: Vec<(AccountId, AssetId, Amount)>,
        /// Acknowledgement.
        reply: Sender<Result<(), String>>,
    },
    /// **Adversary simulation (devnet only).** Silently drop transactions
    /// from these accounts (empty: censor nobody).
    Censor {
        /// Accounts to censor.
        accounts: std::collections::BTreeSet<AccountId>,
    },
    /// The gateway's view of a transaction.
    Status {
        /// The transaction.
        tx: TxDigest,
        /// Where to answer.
        reply: Sender<Option<TxStatus>>,
    },
    /// Stop.
    Shutdown,
}

#[derive(Debug)]
struct Pending {
    tx: SignedTransaction,
    shard: ShardId,
    builder: CertificateBuilder,
    approvals: usize,
    rejections: BTreeMap<ValidatorId, String>,
    /// The lane committee it is being certified by.
    lane_epoch: Epoch,
    /// The members whose approvals arrived.
    approved: std::collections::BTreeSet<ValidatorId>,
    /// When to ask the members that stayed silent again.
    retry_at: Instant,
    /// How many times it has.
    retries: u32,
}

/// When to ask again after `retries` retries: two batch intervals and a
/// generous second for the answer, then twice as long each time, at most
/// eight seconds. Approvals are idempotent, so asking twice is harmless.
fn lane_retry_delay(batch_interval_ms: u64, retries: u32) -> Duration {
    let first = Duration::from_millis(2 * batch_interval_ms + 1_000);
    (first * 2u32.saturating_pow(retries.min(4))).min(Duration::from_secs(8))
}

/// A certified transaction handed to consensus, kept until it executes.
#[derive(Debug)]
struct Sequenced {
    tx: SignedTransaction,
    certificate: LaneCertificate,
    shard: ShardId,
    lane_epoch: Epoch,
    sent: Instant,
}

/// The gateway's runtime.
pub struct Gateway {
    shared: Arc<Shared>,
    net: Arc<dyn Transport>,
    query: Querier,
    inbox: Receiver<GatewayInput>,
    pending: BTreeMap<TxDigest, Pending>,
    statuses: BTreeMap<TxDigest, TxStatus>,
    order: VecDeque<TxDigest>,
    next: usize,
    faucet: Wallet,
    faucet_queue: Vec<(AccountId, AssetId, Amount)>,
    faucet_inflight: Option<(TxDigest, Vec<Grant>, Instant)>,
    faucet_nonce: u64,
    verifier: crate::keys::NodeVerifier,
    certified: Vec<(SignedTransaction, LaneCertificate)>,
    censored: std::collections::BTreeSet<AccountId>,
    sequencing: BTreeMap<TxDigest, Sequenced>,
    session: u64,
    lane_epochs: BTreeMap<ShardId, Epoch>,
}

impl std::fmt::Debug for Gateway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gateway")
            .field("node", &self.net.local())
            .field("pending", &self.pending.len())
            .field("sequencing", &self.sequencing.len())
            .finish()
    }
}

/// A certified transaction not executed after this long is handed to
/// consensus again (duplicates execute once).
const RESEND_AFTER: Duration = Duration::from_secs(4);

const STATUS_LIMIT: usize = 100_000;

/// Transactions being certified at most; beyond it the gateway refuses new
/// ones until some certify, are refused or expire.
const PENDING_LIMIT: usize = 100_000;

/// Certified transactions are handed to consensus in groups at most this
/// often, sharing their batch headers.
const GROUP_INTERVAL_MS: u64 = 20;

/// The most transactions in one group (groups are also bounded in bytes,
/// see [`Gateway::send_certified`]).
const GROUP_LIMIT: usize = 512;

/// Faucet grants waiting at most.
const FAUCET_QUEUE_LIMIT: usize = 10_000;

/// One faucet grant: recipient, asset, amount.
type Grant = (AccountId, AssetId, Amount);

/// Whom to ask for a transaction's lane approvals: its shard and the
/// members of the shard's committee.
type Ask = (ShardId, Vec<NodeId>);

impl Gateway {
    /// A gateway that reaches validators through `net`, receives what they
    /// answer in `inbox`, and asks a validator about the state with `query`.
    pub fn new(
        shared: Arc<Shared>,
        net: Arc<dyn Transport>,
        query: Querier,
        inbox: Receiver<GatewayInput>,
        faucet: Wallet,
    ) -> Gateway {
        let verifier = shared.verifier.clone();
        Gateway {
            shared,
            net,
            query,
            inbox,
            pending: BTreeMap::new(),
            statuses: BTreeMap::new(),
            order: VecDeque::new(),
            next: 0,
            faucet,
            faucet_queue: Vec::new(),
            faucet_inflight: None,
            faucet_nonce: 0,
            verifier,
            certified: Vec::new(),
            censored: Default::default(),
            sequencing: BTreeMap::new(),
            session: 0,
            lane_epochs: BTreeMap::new(),
        }
    }

    /// The latest committees (validators publish them after every Pulse).
    fn committees(&mut self) -> Option<crate::committees::Committees> {
        let view = self.shared.view.read().ok()?;
        self.verifier.extend_keys(&view.keys);
        view.committees.clone()
    }

    fn lane_committee(&mut self, shard: ShardId) -> Option<Committee> {
        self.committees()?.lanes.get(&shard).cloned()
    }

    /// Follows committee changes: re-certifies transactions whose lane
    /// committee was replaced, and re-sends certified ones a new session
    /// dropped or that are taking too long.
    fn follow_committees(&mut self) {
        let Some(committees) = self.committees() else {
            return;
        };
        let lanes: BTreeMap<ShardId, Epoch> = committees
            .lanes
            .iter()
            .map(|(s, c)| (*s, c.epoch()))
            .collect();
        let new_session = committees.session != self.session;
        if lanes != self.lane_epochs {
            // Certified under a replaced committee: certify again.
            let stale: Vec<TxDigest> = self
                .sequencing
                .iter()
                .filter(|(_, s)| lanes.get(&s.shard) != Some(&s.lane_epoch))
                .map(|(d, _)| *d)
                .collect();
            for digest in stale {
                if let Some(sequenced) = self.sequencing.remove(&digest) {
                    self.statuses.remove(&digest);
                    let _ = self.submit(sequenced.tx);
                }
            }
            let restart: Vec<TxDigest> = self
                .pending
                .iter()
                .filter(|(_, p)| lanes.get(&p.shard) != Some(&p.lane_epoch))
                .map(|(d, _)| *d)
                .collect();
            for digest in restart {
                if let Some(pending) = self.pending.remove(&digest) {
                    self.statuses.remove(&digest);
                    let _ = self.submit(pending.tx);
                }
            }
            self.lane_epochs = lanes;
        }
        let now = Instant::now();
        let resend: Vec<TxDigest> = self
            .sequencing
            .iter()
            .filter(|(_, s)| new_session || now.duration_since(s.sent) > RESEND_AFTER)
            .map(|(d, _)| *d)
            .collect();
        self.session = committees.session;
        let mut again = Vec::new();
        for digest in resend {
            if let Some(sequenced) = self.sequencing.get_mut(&digest) {
                sequenced.sent = now;
                again.push((sequenced.tx.clone(), sequenced.certificate.clone()));
            }
        }
        self.send_certified(again);
    }

    /// Runs until shut down.
    pub fn run(mut self) {
        let mut last_tick = Instant::now();
        let mut last_flush = Instant::now();
        loop {
            match self
                .inbox
                .recv_timeout(Duration::from_millis(GROUP_INTERVAL_MS))
            {
                Ok(GatewayInput::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(input) => self.handle(input),
                Err(RecvTimeoutError::Timeout) => {}
            }
            if last_flush.elapsed() >= Duration::from_millis(GROUP_INTERVAL_MS) {
                last_flush = Instant::now();
                self.flush_certified();
            }
            if last_tick.elapsed() >= Duration::from_millis(100) {
                last_tick = Instant::now();
                self.follow_committees();
                self.retry_lane_requests();
                self.tick_faucet();
            }
        }
    }

    /// Asks the lane committee members whose answer is late again (a request
    /// or its approval may have been lost), and gives up on transactions
    /// that expired before they were certified.
    fn retry_lane_requests(&mut self) {
        let now = Instant::now();
        let pulse = self.shared.view.read().map(|v| v.pulse).unwrap_or_default();
        let expired: Vec<TxDigest> = self
            .pending
            .iter()
            .filter(|(_, p)| p.tx.tx.expires_at <= pulse)
            .map(|(d, _)| *d)
            .collect();
        for digest in expired {
            self.pending.remove(&digest);
            self.set_status(
                digest,
                TxStatus::Rejected("it expired before a quorum approved it".into()),
            );
        }
        let due: Vec<TxDigest> = self
            .pending
            .iter()
            .filter(|(_, p)| p.retry_at <= now)
            .map(|(d, _)| *d)
            .collect();
        let batch_interval = self.shared.config.batch_interval_ms;
        for digest in due {
            let Some((shard, lane_epoch)) = self
                .pending
                .get(&digest)
                .map(|pending| (pending.shard, pending.lane_epoch))
            else {
                continue;
            };
            let committee = match self.lane_committee(shard) {
                // A replaced committee is handled by `follow_committees`.
                Some(committee) if committee.epoch() == lane_epoch => committee,
                _ => continue,
            };
            let Some(pending) = self.pending.get_mut(&digest) else {
                continue;
            };
            let silent: Vec<NodeId> = committee
                .members()
                .iter()
                .map(|(member, _)| *member)
                .filter(|m| !pending.approved.contains(m) && !pending.rejections.contains_key(m))
                .map(NodeId::Validator)
                .collect();
            pending.retries += 1;
            pending.retry_at = now + lane_retry_delay(batch_interval, pending.retries);
            let request = LaneRequest {
                tx: digest,
                inputs: lane_inputs(&pending.tx.tx.inputs),
                owner_proof: encode(&pending.tx),
            };
            self.net
                .send_all(&silent, &PeerMessage::Lane { shard, request });
        }
    }

    fn set_status(&mut self, tx: TxDigest, status: TxStatus) {
        if self.statuses.insert(tx, status).is_none() {
            self.order.push_back(tx);
            while self.order.len() > STATUS_LIMIT {
                if let Some(old) = self.order.pop_front() {
                    self.statuses.remove(&old);
                }
            }
        }
    }

    fn handle(&mut self, input: GatewayInput) {
        match input {
            GatewayInput::Submit { tx, reply } => {
                let _ = reply.send(self.submit(*tx));
            }
            GatewayInput::SubmitBatch { members, reply } => {
                let _ = reply.send(self.submit_batch(members));
            }
            GatewayInput::Peer { from, message } => self.on_peer(from, *message),
            GatewayInput::Faucet { grants, reply } => {
                if grants.iter().any(|(_, _, a)| *a == Amount::ZERO) {
                    let _ = reply.send(Err("zero grant".into()));
                } else if self.faucet_queue.len() + grants.len() > FAUCET_QUEUE_LIMIT {
                    let _ = reply.send(Err("the faucet is busy; try again shortly".into()));
                } else {
                    self.faucet_queue.extend(grants);
                    let _ = reply.send(Ok(()));
                }
            }
            GatewayInput::Status { tx, reply } => {
                let _ = reply.send(self.statuses.get(&tx).cloned());
            }
            GatewayInput::Censor { accounts } => self.censored = accounts,
            GatewayInput::Shutdown => {}
        }
    }

    /// What a validator sent. Only validators' messages count, and a
    /// refusal counts as the refusal of the validator that sent it.
    fn on_peer(&mut self, from: NodeId, message: PeerMessage) {
        let Some(validator) = from.validator() else {
            return;
        };
        match message {
            PeerMessage::Approval { shard, tx, proof } => self.on_approval(shard, tx, proof),
            PeerMessage::LaneRejected { tx, reason } => self.on_rejection(tx, validator, reason),
            PeerMessage::Executed { digests, .. } => {
                for digest in digests {
                    self.sequencing.remove(&digest);
                }
            }
            _ => {}
        }
    }

    fn submit(&mut self, signed: SignedTransaction) -> Result<SubmitReceipt, String> {
        let request = LaneRequest {
            tx: signed.digest(),
            inputs: lane_inputs(&signed.tx.inputs),
            owner_proof: encode(&signed),
        };
        let (receipt, ask) = self.admit(signed)?;
        if let Some((shard, members)) = ask {
            self.net
                .send_all(&members, &PeerMessage::Lane { shard, request });
        }
        Ok(receipt)
    }

    /// Submits the members of one batch: each is admitted as
    /// [`Gateway::submit`] admits a transaction, and each shard's committee
    /// gets the lane requests of its members in one message, with the
    /// batch's signatures once. Members of different batches go one by one.
    fn submit_batch(
        &mut self,
        members: Vec<SignedTransaction>,
    ) -> Result<Vec<SubmitReceipt>, String> {
        let shared = members.first().and_then(|first| {
            let batch = first.batch.as_ref()?.batch;
            members
                .iter()
                .all(|m| m.auth == first.auth && m.batch.as_ref().map(|b| b.batch) == Some(batch))
                .then(|| (batch, first.auth.clone()))
        });
        let Some((batch, auth)) = shared else {
            return members.into_iter().map(|m| self.submit(m)).collect();
        };
        if members.len() as u64 > sion_exec::tx::MAX_BATCH {
            return Err("a batch holds at most 256 transactions".into());
        }
        let mut receipts = Vec::with_capacity(members.len());
        let mut asks: BTreeMap<ShardId, (Vec<NodeId>, Vec<LaneMember>)> = BTreeMap::new();
        for signed in members {
            let place = signed.batch.clone().expect("a batch member");
            let tx = signed.tx.clone();
            let (receipt, ask) = self.admit(signed)?;
            if let Some((shard, nodes)) = ask {
                asks.entry(shard)
                    .or_insert_with(|| (nodes, Vec::new()))
                    .1
                    .push(LaneMember {
                        tx,
                        index: place.index,
                        path: place.path,
                        charter: place.charter,
                    });
            }
            receipts.push(receipt);
        }
        for (shard, (nodes, members)) in asks {
            self.net.send_all(
                &nodes,
                &PeerMessage::LaneBatch {
                    shard,
                    batch,
                    auth: auth.clone(),
                    members,
                },
            );
        }
        Ok(receipts)
    }

    /// Admits a transaction for certification: its receipt, and the shard
    /// and committee members to ask for lane approvals, if they are to be
    /// asked (not when it is already known, or censored).
    fn admit(&mut self, signed: SignedTransaction) -> Result<(SubmitReceipt, Option<Ask>), String> {
        let digest = signed.digest();
        if signed.size() > 256 * 1024 {
            return Err("transaction too large".into());
        }
        // Every transaction locks its owned inputs with a lane certificate
        // first; a contended one then goes through consensus.
        let shard = ShardId::for_account(&signed.tx.sender, self.shared.config.shards);
        let receipt = SubmitReceipt {
            digest,
            route: if signed.tx.domain().is_some() {
                "consensus"
            } else {
                "lane"
            }
            .into(),
            shard: Some(shard.0),
        };
        // A transaction refused before may be submitted again (validators may
        // have caught up, or its inputs been released).
        if matches!(self.statuses.get(&digest), Some(TxStatus::Rejected(_))) {
            self.statuses.remove(&digest);
        }
        if self.pending.contains_key(&digest) || self.statuses.contains_key(&digest) {
            return Ok((receipt, None));
        }
        if self.pending.len() >= PENDING_LIMIT {
            return Err("the gateway is busy; try again shortly".into());
        }
        if self.censored.contains(&signed.tx.sender) {
            // A censoring gateway acknowledges and then does nothing.
            self.set_status(
                digest,
                TxStatus::Certifying {
                    shard: shard.0,
                    approvals: 0,
                },
            );
            return Ok((receipt, None));
        }
        let committee = self
            .lane_committee(shard)
            .ok_or("no committee for the shard yet")?;
        let inputs = lane_inputs(&signed.tx.inputs);
        let members: Vec<NodeId> = committee
            .members()
            .iter()
            .map(|(member, _)| NodeId::Validator(*member))
            .collect();
        self.pending.insert(
            digest,
            Pending {
                tx: signed,
                shard,
                builder: CertificateBuilder::new(digest, &inputs),
                approvals: 0,
                rejections: BTreeMap::new(),
                lane_epoch: committee.epoch(),
                approved: Default::default(),
                retry_at: Instant::now()
                    + lane_retry_delay(self.shared.config.batch_interval_ms, 0),
                retries: 0,
            },
        );
        self.set_status(
            digest,
            TxStatus::Certifying {
                shard: shard.0,
                approvals: 0,
            },
        );
        Ok((receipt, Some((shard, members))))
    }

    /// Hands every certificate collected since the last flush to consensus,
    /// in groups sharing their batch headers, and keeps them until they
    /// execute.
    fn flush_certified(&mut self) {
        if self.certified.is_empty() {
            return;
        }
        let certified = std::mem::take(&mut self.certified);
        let now = Instant::now();
        for (tx, certificate) in &certified {
            let shard = ShardId::for_account(&tx.tx.sender, self.shared.config.shards.max(1));
            let lane_epoch = self.lane_epochs.get(&shard).copied().unwrap_or_default();
            self.sequencing.insert(
                tx.digest(),
                Sequenced {
                    tx: tx.clone(),
                    certificate: certificate.clone(),
                    shard,
                    lane_epoch: certificate
                        .approvals
                        .first()
                        .map(|a| a.batch.epoch)
                        .unwrap_or(lane_epoch),
                    sent: now,
                },
            );
        }
        self.send_certified(certified);
    }

    /// Hands certified transactions to consensus in groups that fit a
    /// consensus payload, sharing batch headers within each group.
    ///
    /// Consensus drops an item larger than its payload limit, so a group
    /// that big would be re-sent and dropped forever. Groups aim at half the
    /// limit, so a header can carry two groups and the small items (beacon
    /// reveals, forced intents) never wait behind a full one. A transaction
    /// whose certificate cannot fit even alone is refused.
    fn send_certified(&mut self, certified: Vec<(SignedTransaction, LaneCertificate)>) {
        let limit = self.shared.config.core.max_payload_bytes;
        for item in PayloadItem::groups(certified, limit / 2, GROUP_LIMIT) {
            let bytes = item.to_bytes();
            if bytes.len() <= limit {
                self.send_to_consensus(bytes);
                continue;
            }
            let PayloadItem::Certified { txs, .. } = item else {
                continue;
            };
            for grouped in txs {
                let digest = grouped.tx.digest();
                eprintln!("gateway: {digest} has a certificate too large for a consensus payload");
                self.sequencing.remove(&digest);
                self.set_status(
                    digest,
                    TxStatus::Rejected(format!(
                        "its certificate does not fit in a consensus payload of {limit} bytes"
                    )),
                );
            }
        }
    }

    /// Hands an item to f + 1 members of the consensus committee, so at
    /// least one honest proposer includes it (duplicates are executed once).
    /// Validators outside the active set only observe, so they are skipped.
    fn send_to_consensus(&mut self, bytes: Vec<u8>) {
        let members: Vec<NodeId> = match self.committees() {
            Some(committees) => committees
                .consensus
                .members()
                .iter()
                .map(|(v, _)| NodeId::Validator(*v))
                .filter(|node| self.net.knows(node))
                .collect(),
            None => self
                .shared
                .validators
                .iter()
                .map(|v| NodeId::Validator(*v))
                .filter(|node| self.net.knows(node))
                .collect(),
        };
        if members.is_empty() {
            return;
        }
        let copies = (members.len() - 1) / 3 + 1;
        let chosen: Vec<NodeId> = (0..copies.min(members.len()))
            .map(|k| members[(self.next + k) % members.len()])
            .collect();
        self.net
            .send_all(&chosen, &PeerMessage::Payload { item: bytes });
        self.next = self.next.wrapping_add(1);
    }

    fn on_approval(&mut self, shard: ShardId, tx: TxDigest, proof: ApprovalProof) {
        let Some(pending) = self.pending.get_mut(&tx) else {
            return;
        };
        if pending.shard != shard || proof.batch.epoch != pending.lane_epoch {
            return;
        }
        let Some(committee) = self
            .shared
            .committees()
            .and_then(|c| c.lanes.get(&shard).cloned())
        else {
            return;
        };
        let Some(pending) = self.pending.get_mut(&tx) else {
            return;
        };
        let approver = proof.batch.validator;
        let added = pending.builder.add(&committee, proof, &self.verifier);
        if added.is_ok() {
            pending.approved.insert(approver);
        }
        match added {
            Ok(Some(certificate)) => {
                let pending = self.pending.remove(&tx).expect("present");
                self.certified.push((pending.tx, certificate));
                self.set_status(tx, TxStatus::Sequencing);
                if self.certified.len() >= GROUP_LIMIT {
                    self.flush_certified();
                }
            }
            Ok(None) => {
                // An approval asked for twice may arrive twice.
                pending.approvals = pending.approved.len();
                let approvals = pending.approvals;
                self.set_status(
                    tx,
                    TxStatus::Certifying {
                        shard: shard.0,
                        approvals,
                    },
                );
            }
            Err(error) => eprintln!("gateway: bad approval for {}: {error}", tx),
        }
    }

    fn on_rejection(&mut self, tx: TxDigest, validator: ValidatorId, reason: String) {
        let Some(pending) = self.pending.get_mut(&tx) else {
            return;
        };
        pending.rejections.insert(validator, reason.clone());
        let shard = pending.shard;
        let Some(committee) = self
            .shared
            .committees()
            .and_then(|c| c.lanes.get(&shard).cloned())
        else {
            return;
        };
        let Some(pending) = self.pending.get_mut(&tx) else {
            return;
        };
        let rejected = committee.stake_of_set(pending.rejections.keys());
        if rejected > committee.total_stake() - committee.quorum_threshold() {
            self.pending.remove(&tx);
            self.set_status(tx, TxStatus::Rejected(reason));
        }
    }

    fn query(&self, query: Query) -> Result<Value, String> {
        (self.query)(query)
    }

    fn tick_faucet(&mut self) {
        if let Some((digest, grants, since)) = &self.faucet_inflight {
            let executed = self.query(Query::Tx(*digest)).ok();
            let rejected = matches!(self.statuses.get(digest), Some(TxStatus::Rejected(_)));
            let succeeded = executed
                .as_ref()
                .and_then(|v| v.get("status"))
                .map(|s| s == "Success")
                .unwrap_or(false);
            if executed.is_some() || rejected || since.elapsed() > Duration::from_secs(30) {
                if !succeeded {
                    self.faucet_queue.extend(grants.iter().copied());
                }
                self.faucet_inflight = None;
            } else {
                return;
            }
        }
        if self.faucet_queue.is_empty() {
            return;
        }
        let take = self.faucet_queue.len().min(256);
        let grants: Vec<_> = self.faucet_queue.drain(..take).collect();
        match self.build_faucet(grants.clone()) {
            Ok(tx) => {
                let digest = tx.digest();
                if self.submit(tx).is_ok() {
                    self.faucet_inflight = Some((digest, grants, Instant::now()));
                } else {
                    self.faucet_queue.extend(grants);
                }
            }
            Err(error) => {
                eprintln!("gateway: faucet: {error}");
                self.faucet_queue.extend(grants);
            }
        }
    }

    fn build_faucet(
        &mut self,
        grants: Vec<(AccountId, AssetId, Amount)>,
    ) -> Result<SignedTransaction, String> {
        let value = self.query(Query::Object(well_known::account(&self.faucet.id)))?;
        let pulse: Pulse =
            serde_json::from_value(value["pulse"].clone()).map_err(|e| e.to_string())?;
        let object: sion_exec::Object = serde_json::from_value(value["proof"]["object"].clone())
            .map_err(|_| "faucet account missing".to_string())?;
        if !matches!(object.data, ObjectData::Account(_)) {
            return Err("faucet account object corrupted".into());
        }
        let tx = Transaction {
            sender: self.faucet.id,
            genesis_policy: None,
            inputs: vec![object.reference()],
            fee: Amount::ZERO,
            kind: TxKind::Faucet { grants },
            expires_at: Pulse(pulse.0 + 30),
            nonce: self.faucet_nonce,
        };
        self.faucet_nonce += 1;
        tx.sign(&self.faucet.keys, &self.shared.genesis.config)
            .map_err(|e| e.to_string())
    }
}
