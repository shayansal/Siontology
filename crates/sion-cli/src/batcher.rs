//! Signing for keyless subagents in batches (spec v1.13, §4.3): the
//! transactions that subagents of one signer make at about the same time
//! go out under one signature.
//!
//! A keyless subagent's transactions are signed by the wallet of its
//! nearest keyed ancestor. When several of its subagents run at once (the
//! spawns of one turn run in parallel), their transactions reach the
//! batcher within moments of each other. The first to arrive waits for
//! others while they keep coming ([`QUIET`] after the last, at most
//! [`WINDOW`] in all), then signs every transaction waiting, up to
//! [`MAX_MEMBERS`], as one batch, submits the batch in one request and
//! hands each caller its digest. Each member pays the fee floor of a batch
//! its size. If the batch cannot be built (one member's inputs are gone,
//! say), each transaction is signed by itself instead, so that one bad
//! member does not hold up the others.
//!
//! The batcher only signs what callers built and the ledger checks it: a
//! member must still be within its subagent's charter, and its signer must
//! be the subagent's nearest keyed ancestor (`agent:keyless-signer`).

use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use sion_core::ids::TxDigest;
use sion_exec::object::ObjectRef;
use sion_exec::TxKind;

use crate::error::CliError;
use crate::runtime::SharedLedger;
use crate::session::{Actor, Session};

/// How long a batch waits for another member after the last one came:
/// longer than the jitter of a few requests to a node, so that subagents
/// started together are signed together.
pub const QUIET: Duration = Duration::from_millis(100);

/// The longest the first member of a batch waits for others.
pub const WINDOW: Duration = Duration::from_millis(500);

/// The most members of a batch (spec §4.3).
pub const MAX_MEMBERS: usize = 256;

type Answer = Arc<(Mutex<Option<Result<TxDigest, CliError>>>, Condvar)>;

struct Request {
    actor: Actor,
    kind: TxKind,
    extra: Vec<ObjectRef>,
    answer: Answer,
}

#[derive(Default)]
struct Queue {
    pending: Vec<Request>,
    leading: bool,
}

/// Requests signed together, and what signing them gave.
type Group<'a> = (
    Vec<&'a Request>,
    Result<Vec<sion_exec::SignedTransaction>, CliError>,
);

/// How many batches a signer's batcher signed, and how many transactions
/// were in them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Batches signed (one signature each).
    pub batches: u64,
    /// Transactions in them.
    pub members: u64,
}

/// The batcher of one signing wallet.
pub struct Batcher {
    signer: String,
    queue: Mutex<Queue>,
    arrived: Condvar,
    stats: Mutex<Stats>,
}

impl std::fmt::Debug for Batcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Batcher")
            .field("signer", &self.signer)
            .field("stats", &self.stats())
            .finish()
    }
}

fn batchers() -> &'static Mutex<BTreeMap<String, Arc<Batcher>>> {
    static BATCHERS: OnceLock<Mutex<BTreeMap<String, Arc<Batcher>>>> = OnceLock::new();
    BATCHERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// The batcher of the wallet `signer`, in this process.
pub fn of(signer: &str) -> Arc<Batcher> {
    let mut all = batchers().lock().unwrap_or_else(|p| p.into_inner());
    all.entry(signer.to_string())
        .or_insert_with(|| {
            Arc::new(Batcher {
                signer: signer.to_string(),
                queue: Mutex::new(Queue::default()),
                arrived: Condvar::new(),
                stats: Mutex::new(Stats::default()),
            })
        })
        .clone()
}

/// The same error again (errors are not `Clone`).
fn again(error: &CliError) -> CliError {
    match error {
        CliError::Connection(e) => CliError::Connection(e.clone()),
        CliError::Node(e) => CliError::Node(e.clone()),
        CliError::Protocol(e) => CliError::Protocol(e.clone()),
        CliError::Build(e) => CliError::Build(e.clone()),
        CliError::Transaction(e) => CliError::Transaction(e.clone()),
        CliError::Usage(e) => CliError::Usage(e.clone()),
        CliError::Verification(e) => CliError::Verification(e.clone()),
    }
}

fn answer(to: &Answer, result: Result<TxDigest, CliError>) {
    let (slot, ready) = &**to;
    *slot.lock().unwrap_or_else(|p| p.into_inner()) = Some(result);
    ready.notify_all();
}

impl Batcher {
    /// What this batcher has signed so far.
    pub fn stats(&self) -> Stats {
        *self.stats.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Signs `kind` for `actor` (with `extra` among its inputs) in a batch
    /// with whatever else arrives within [`WINDOW`], submits it, and returns
    /// its digest once the node acknowledged it.
    pub fn submit(
        &self,
        ledger: &SharedLedger,
        actor: Actor,
        kind: TxKind,
        extra: Vec<ObjectRef>,
    ) -> Result<TxDigest, CliError> {
        let answer: Answer = Arc::new((Mutex::new(None), Condvar::new()));
        let lead = {
            let mut queue = self.queue.lock().unwrap_or_else(|p| p.into_inner());
            queue.pending.push(Request {
                actor,
                kind,
                extra,
                answer: answer.clone(),
            });
            self.arrived.notify_all();
            !std::mem::replace(&mut queue.leading, true)
        };
        if lead {
            self.lead(ledger);
        }
        let (slot, ready) = &*answer;
        let mut result = slot.lock().unwrap_or_else(|p| p.into_inner());
        while result.is_none() {
            result = ready.wait(result).unwrap_or_else(|p| p.into_inner());
        }
        result.take().expect("answered")
    }

    /// Takes batches off the queue and signs them until it is empty.
    fn lead(&self, ledger: &SharedLedger) {
        let deadline = Instant::now() + WINDOW;
        loop {
            let batch: Vec<Request> = {
                let mut queue = self.queue.lock().unwrap_or_else(|p| p.into_inner());
                // Wait while others keep joining, up to the window or a full
                // batch.
                let mut seen = queue.pending.len();
                let mut quiet_until = Instant::now() + QUIET;
                while queue.pending.len() < MAX_MEMBERS {
                    let now = Instant::now();
                    let until = quiet_until.min(deadline);
                    if now >= until {
                        break;
                    }
                    queue = self
                        .arrived
                        .wait_timeout(queue, until - now)
                        .unwrap_or_else(|p| p.into_inner())
                        .0;
                    if queue.pending.len() > seen {
                        seen = queue.pending.len();
                        quiet_until = Instant::now() + QUIET;
                    }
                }
                if queue.pending.is_empty() {
                    queue.leading = false;
                    return;
                }
                let take = queue.pending.len().min(MAX_MEMBERS);
                queue.pending.drain(..take).collect()
            };
            self.sign(ledger, batch);
        }
    }

    /// Signs and submits `batch`, answering every request in it.
    fn sign(&self, ledger: &SharedLedger, batch: Vec<Request>) {
        let mut session = match Session::open(ledger.as_ref()) {
            Ok(session) => session,
            Err(error) => {
                for request in &batch {
                    answer(&request.answer, Err(again(&error)));
                }
                return;
            }
        };
        let actions: Vec<(Actor, TxKind, Vec<ObjectRef>)> = batch
            .iter()
            .map(|r| (r.actor.clone(), r.kind.clone(), r.extra.clone()))
            .collect();
        let together = if batch.len() > 1 {
            session.sign_batch(&self.signer, actions.clone()).ok()
        } else {
            None
        };
        let groups: Vec<Group<'_>> = match together {
            Some(members) => vec![(batch.iter().collect(), Ok(members))],
            // Each by itself: a bad member fails alone.
            None => batch
                .iter()
                .zip(actions)
                .map(|(request, action)| {
                    (
                        vec![request],
                        session.sign_batch(&self.signer, vec![action]),
                    )
                })
                .collect(),
        };
        for (requests, signed) in groups {
            let members = match signed {
                Ok(members) => members,
                Err(error) => {
                    for request in &requests {
                        answer(&request.answer, Err(again(&error)));
                    }
                    continue;
                }
            };
            match ledger.submit_batch(&members) {
                Ok(_) => {
                    let mut stats = self.stats.lock().unwrap_or_else(|p| p.into_inner());
                    stats.batches += 1;
                    stats.members += members.len() as u64;
                    drop(stats);
                    for (request, member) in requests.iter().zip(&members) {
                        answer(&request.answer, Ok(member.digest()));
                    }
                }
                Err(error) => {
                    for request in &requests {
                        answer(&request.answer, Err(again(&error)));
                    }
                }
            }
        }
        session.settle_all();
    }
}
