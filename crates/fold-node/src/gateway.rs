//! The client gateway: routes transactions by coordination class.
//!
//! An owned-path transaction goes to its shard's lane committee; the
//! gateway collects batch-signed approvals until they form a certificate
//! (more than two thirds of the committee), at which point the transaction
//! is final, and hands the certificate to consensus so a Pulse executes it.
//! A contended transaction goes straight to consensus. The devnet faucet
//! batches its grants into one transaction per Pulse.

use std::collections::{BTreeMap, VecDeque};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fold_consensus::{ApprovalProof, CertificateBuilder, LaneRequest};
use fold_core::encode::encode;
use fold_core::ids::{AccountId, AssetId, Pulse, ShardId, TxDigest, ValidatorId};
use fold_core::Amount;
use fold_exec::object::well_known;
use fold_exec::{ObjectData, SignedTransaction, Transaction, TxKind, Wallet};
use serde_json::Value;

use crate::payload::{lane_inputs, PayloadItem};
use crate::validator::{Bus, Input, Shared};
use crate::view::{Query, SubmitReceipt, TxStatus};

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
    /// A validator's sealed approval.
    Approval {
        /// The shard.
        shard: ShardId,
        /// The transaction.
        tx: TxDigest,
        /// The approval.
        proof: ApprovalProof,
    },
    /// A validator refused to approve.
    LaneRejected {
        /// The transaction.
        tx: TxDigest,
        /// The validator.
        validator: ValidatorId,
        /// Why.
        reason: String,
    },
    /// Faucet grants (devnet only).
    Faucet {
        /// Recipients.
        grants: Vec<(AccountId, AssetId, Amount)>,
        /// Acknowledgement.
        reply: Sender<Result<(), String>>,
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
}

/// The gateway's runtime.
#[derive(Debug)]
pub struct Gateway {
    shared: Arc<Shared>,
    bus: Bus,
    inbox: Receiver<GatewayInput>,
    pending: BTreeMap<TxDigest, Pending>,
    statuses: BTreeMap<TxDigest, TxStatus>,
    order: VecDeque<TxDigest>,
    next: usize,
    faucet: Wallet,
    faucet_queue: Vec<(AccountId, AssetId, Amount)>,
    faucet_inflight: Option<(TxDigest, Vec<Grant>, Instant)>,
    faucet_nonce: u64,
}

const STATUS_LIMIT: usize = 100_000;

/// One faucet grant: recipient, asset, amount.
type Grant = (AccountId, AssetId, Amount);

impl Gateway {
    /// A gateway.
    pub fn new(
        shared: Arc<Shared>,
        bus: Bus,
        inbox: Receiver<GatewayInput>,
        faucet: Wallet,
    ) -> Gateway {
        Gateway {
            shared,
            bus,
            inbox,
            pending: BTreeMap::new(),
            statuses: BTreeMap::new(),
            order: VecDeque::new(),
            next: 0,
            faucet,
            faucet_queue: Vec::new(),
            faucet_inflight: None,
            faucet_nonce: 0,
        }
    }

    /// Runs until shut down.
    pub fn run(mut self) {
        let mut last_tick = Instant::now();
        loop {
            match self.inbox.recv_timeout(Duration::from_millis(50)) {
                Ok(GatewayInput::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(input) => self.handle(input),
                Err(RecvTimeoutError::Timeout) => {}
            }
            if last_tick.elapsed() >= Duration::from_millis(100) {
                last_tick = Instant::now();
                self.tick_faucet();
            }
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
            GatewayInput::Approval { shard, tx, proof } => self.on_approval(shard, tx, proof),
            GatewayInput::LaneRejected {
                tx,
                validator,
                reason,
            } => self.on_rejection(tx, validator, reason),
            GatewayInput::Faucet { grants, reply } => {
                if grants.iter().any(|(_, _, a)| *a == Amount::ZERO) {
                    let _ = reply.send(Err("zero grant".into()));
                } else {
                    self.faucet_queue.extend(grants);
                    let _ = reply.send(Ok(()));
                }
            }
            GatewayInput::Status { tx, reply } => {
                let _ = reply.send(self.statuses.get(&tx).cloned());
            }
            GatewayInput::Shutdown => {}
        }
    }

    fn submit(&mut self, signed: SignedTransaction) -> Result<SubmitReceipt, String> {
        let digest = signed.digest();
        if signed.size() > 256 * 1024 {
            return Err("transaction too large".into());
        }
        if signed.tx.domain().is_some() {
            if !self.statuses.contains_key(&digest) {
                self.send_to_consensus(PayloadItem::Contended { tx: signed });
                self.set_status(digest, TxStatus::Sequencing);
            }
            return Ok(SubmitReceipt {
                digest,
                route: "consensus".into(),
                shard: None,
            });
        }
        let shard = ShardId::for_account(&signed.tx.sender, self.shared.config.shards);
        let receipt = SubmitReceipt {
            digest,
            route: "lane".into(),
            shard: Some(shard.0),
        };
        if self.pending.contains_key(&digest) || self.statuses.contains_key(&digest) {
            return Ok(receipt);
        }
        let committee = self
            .shared
            .shard_committees
            .get(&shard)
            .ok_or("no committee for the shard")?
            .clone();
        let inputs = lane_inputs(&signed.tx.inputs);
        let request = LaneRequest {
            tx: digest,
            inputs: inputs.clone(),
            owner_proof: encode(&signed),
        };
        for (member, _) in committee.members() {
            self.bus.send(
                member,
                Input::Lane {
                    shard,
                    request: request.clone(),
                },
            );
        }
        self.pending.insert(
            digest,
            Pending {
                tx: signed,
                shard,
                builder: CertificateBuilder::new(digest, &inputs),
                approvals: 0,
                rejections: BTreeMap::new(),
            },
        );
        self.set_status(
            digest,
            TxStatus::Certifying {
                shard: shard.0,
                approvals: 0,
            },
        );
        Ok(receipt)
    }

    fn send_to_consensus(&mut self, item: PayloadItem) {
        let validators = &self.bus.validators;
        if validators.is_empty() {
            return;
        }
        let index = self.next % validators.len();
        self.next = self.next.wrapping_add(1);
        let _ = validators[index].send(Input::Submit(item.to_bytes()));
    }

    fn on_approval(&mut self, shard: ShardId, tx: TxDigest, proof: ApprovalProof) {
        let Some(pending) = self.pending.get_mut(&tx) else {
            return;
        };
        if pending.shard != shard {
            return;
        }
        let Some(committee) = self.shared.shard_committees.get(&shard) else {
            return;
        };
        match pending.builder.add(committee, proof, &self.shared.verifier) {
            Ok(Some(certificate)) => {
                let pending = self.pending.remove(&tx).expect("present");
                self.send_to_consensus(PayloadItem::Owned {
                    tx: pending.tx,
                    certificate,
                });
                self.set_status(tx, TxStatus::Sequencing);
            }
            Ok(None) => {
                pending.approvals += 1;
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
        let Some(committee) = self.shared.shard_committees.get(&pending.shard) else {
            return;
        };
        let rejected = committee.stake_of_set(pending.rejections.keys());
        if rejected > committee.total_stake() - committee.quorum_threshold() {
            self.pending.remove(&tx);
            self.set_status(tx, TxStatus::Rejected(reason));
        }
    }

    fn query(&self, query: Query) -> Result<Value, String> {
        let (reply, answer) = mpsc::channel();
        self.bus.validators[0]
            .send(Input::Query(query, reply))
            .map_err(|_| "validator stopped".to_string())?;
        answer
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| "validator did not answer".to_string())?
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
        let object: fold_exec::Object = serde_json::from_value(value["proof"]["object"].clone())
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
        tx.sign(&self.faucet.keys).map_err(|e| e.to_string())
    }
}
