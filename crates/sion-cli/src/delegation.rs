//! Handing work to other agents from inside a session (part 2 of the
//! agents plan): an agent spawns a subagent with a budget carved from its
//! own and gives it a task, or hires another agent through a Task Charter,
//! paid only for an accepted result.
//!
//! - `agent_spawn` carves a budget from the agent's allotment into a new
//!   keyless subagent identity (`SpawnAgents`, under caveats that attenuate
//!   the parent's, as the ledger requires; spec v1.13): no key is made for
//!   it, and the wallet that signs for the parent signs for it, in batches
//!   of one. That wallet gives it fee money, and the subagent runs the task
//!   in a session of its own, with the models the person chose for
//!   subagents and the same autonomy. Its answer comes back as the call's
//!   result. Subagents may spawn their own down to a depth of
//!   [`MAX_DEPTH`]; suspending the parent stops the tree.
//! - `task_post` escrows money from the agent's budget for work by another
//!   agent (`PostTask` with the agent's charter): its spec, worker,
//!   verifiers and adjudicator are fixed when it is posted.
//! - `task_status` reads a task; `task_submit` is the worker's side: a
//!   result resting on its task bond, and the settlement.

use std::path::PathBuf;

use serde_json::{json, Value};
use sion_agent::tools::{Effect, Outcome, Tool};
use sion_agent::{Policy, ToolSpec};
use sion_assurance::Policy as Assurance;
use sion_core::hash::Hash32;
use sion_core::ids::{AssetId, AtomId, ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::charter::Spawn;
use sion_exec::execute::Event;
use sion_exec::object::ObjectData;
use sion_exec::task::Acceptance;
use sion_exec::{ExecStatus, TxKind};

use crate::ledger::{Ledger, LedgerExt};
use crate::runtime::{Acting, Equipment};
use crate::session::{wallet_dir, Session};

/// How deep subagents may go: an agent run by a person is at depth 0.
pub const MAX_DEPTH: u32 = 2;

/// Fee money a parent gives each subagent from its own account (USD).
pub const FEE_GRANT: &str = "0.05";

/// What a session's delegation tools work with.
#[derive(Clone)]
pub struct Delegation {
    /// The agent acting.
    pub acting: Acting,
    /// The models subagents run with.
    pub models: Vec<String>,
    /// The rules subagents run under (the parent's).
    pub policy: Policy,
    /// How deep this agent is.
    pub depth: u32,
    /// The adjudicator of the tasks it posts, if the person named one.
    pub adjudicator: Option<AtomId>,
}

impl Delegation {
    /// The tools: spawn, post a task, a task's status, submit a result.
    pub fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![
            Box::new(SpawnTool(self.clone())),
            Box::new(TaskPostTool(self.clone())),
            Box::new(TaskStatusTool(self.acting.clone())),
            Box::new(TaskSubmitTool(self.acting.clone())),
        ]
    }
}

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": properties, "required": required})
}

fn text<'a>(input: &'a Value, key: &str) -> &'a str {
    input[key].as_str().unwrap_or_default().trim()
}

/// Where tasks' specs and outputs are kept: `$SION_HOME/agents/tasks`.
pub fn tasks_dir() -> PathBuf {
    wallet_dir()
        .parent()
        .map(|home| home.join("agents").join("tasks"))
        .unwrap_or_else(|| PathBuf::from(".sion/agents/tasks"))
}

fn keep(bytes: &[u8], commitment: &Hash32) -> Result<PathBuf, String> {
    let dir = tasks_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(commitment.to_hex());
    std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// One of the agent's allotments that holds at least `amount`.
fn allotment_holding(
    ledger: &dyn Ledger,
    agent: ObjectId,
    amount: Amount,
) -> Result<(ObjectId, sion_exec::charter::Charter), String> {
    let charter = crate::agents::identity(ledger, agent).map_err(|e| e.to_string())?;
    for id in &charter.allotments {
        if let Ok(Some(object)) = ledger.object(*id) {
            if let ObjectData::Allotment(a) = object.data {
                if a.remaining >= amount {
                    return Ok((*id, charter));
                }
            }
        }
    }
    Err(format!("no part of your budget holds {amount} USD"))
}

/// `base`, or `base-2`, `base-3`, … if a wallet of that name exists.
fn fresh_name(base: &str) -> String {
    let dir = wallet_dir();
    let taken = |name: &str| {
        dir.join(format!("{name}.seed")).exists() || dir.join(format!("{name}.keyless")).exists()
    };
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|name| !taken(name))
        .expect("some name is free")
}

fn valid_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

/// Spawns subagents and runs their tasks in sessions of their own. The
/// spawns of one turn go together: one `SpawnAgents` transaction, one
/// transfer of their fee money, and their sessions run at once, so that
/// the transactions they make meanwhile are signed in batches.
struct SpawnTool(Delegation);

impl Tool for SpawnTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "agent_spawn".into(),
            description: "Hand part of the work to a subagent of your own. It gets a budget \
                          carved from yours (it can never spend more than you give it), works on \
                          the task you give it in a session of its own, under the same rules, \
                          and reports back. Give it a short name (lowercase letters, digits, \
                          hyphens), a budget in USD and the task, in full: it sees nothing else. \
                          Subagents you spawn in the same turn work at the same time."
                .into(),
            input_schema: schema(
                json!({
                    "name": {"type": "string"},
                    "budget": {"type": "string"},
                    "task": {"type": "string"}
                }),
                &["name", "budget", "task"],
            ),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Spend
    }

    fn batches(&self) -> bool {
        true
    }

    fn call(&self, input: &Value, key: &str) -> Outcome {
        self.call_batch(&[(input, key)]).remove(0)
    }

    fn call_tainted(&self, input: &Value, key: &str) -> Outcome {
        self.call_batch_tainted(&[(input, key)]).remove(0)
    }

    fn call_batch(&self, calls: &[(&Value, &str)]) -> Vec<Outcome> {
        self.call_batch_budgeted(calls, false, None)
    }

    fn call_batch_tainted(&self, calls: &[(&Value, &str)]) -> Vec<Outcome> {
        self.call_batch_budgeted(calls, true, None)
    }

    /// The subagents of a session that read content from outside start
    /// tainted, since the tasks it writes them may carry what it read; in a
    /// run with a budget, they share what is left of it, and what they used
    /// is counted in it.
    fn call_batch_budgeted(
        &self,
        calls: &[(&Value, &str)],
        tainted: bool,
        budget: Option<u64>,
    ) -> Vec<Outcome> {
        let inputs = calls.iter().map(|(input, _)| *input).collect();
        spawn_all(&self.0, inputs, tainted, budget)
            .into_iter()
            .map(|result| match result {
                Ok(report) => {
                    let outcome = Outcome {
                        spent: Some(report.spent),
                        ..Outcome::ok(report.text)
                    };
                    // What a subagent that read content from outside reports
                    // may carry it: it taints this session too.
                    if report.tainted {
                        outcome.from_outside()
                    } else {
                        outcome
                    }
                }
                Err(e) => Outcome::error(format!("no subagent: {e}")),
            })
            .collect()
    }
}

/// What a subagent's session came to: its report, whether it read content
/// from outside, and the tokens it used, its own subagents' included.
struct Report {
    text: String,
    tainted: bool,
    spent: sion_agent::model::Usage,
}

/// A subagent asked for.
struct Order {
    name: String,
    task: String,
    budget: Amount,
}

/// What each input asks for, checked.
fn orders(delegation: &Delegation, inputs: &[&Value]) -> Vec<Result<Order, String>> {
    let mut seen = std::collections::BTreeSet::new();
    inputs
        .iter()
        .map(|input| {
            if delegation.depth >= MAX_DEPTH {
                return Err(format!(
                    "a subagent at depth {} cannot spawn another (at most {MAX_DEPTH})",
                    delegation.depth
                ));
            }
            let name = text(input, "name");
            let task = text(input, "task");
            if !valid_name(name) {
                return Err(format!(
                    "{name:?} is not a subagent name: 1 to 32 lowercase letters, digits and hyphens"
                ));
            }
            if !seen.insert(name.to_string()) {
                return Err(format!("{name} is asked for twice in this turn"));
            }
            if task.is_empty() {
                return Err("give the subagent its task".into());
            }
            let budget =
                crate::agents::parse_amount(text(input, "budget")).map_err(|e| e.to_string())?;
            if budget == Amount::ZERO {
                return Err("give the subagent a budget".into());
            }
            Ok(Order {
                name: name.to_string(),
                task: task.to_string(),
                budget,
            })
        })
        .collect()
}

/// A subagent launched: its wallet name and its agent.
struct Launched {
    wallet: String,
    child: ObjectId,
}

/// Spawns `orders` in one transaction from one part of the budget, and
/// gives them their fee money in one transfer.
fn launch(delegation: &Delegation, orders: &[&Order]) -> Result<Vec<Launched>, String> {
    let acting = &delegation.acting;
    let ledger = acting.ledger.as_ref();
    let total = Amount(orders.iter().map(|o| o.budget.0).sum());
    let (allotment, charter) = allotment_holding(ledger, acting.agent, total)?;
    if !charter.caveats.may_delegate {
        return Err("your charter does not let you spawn subagents".into());
    }
    // What the parent's bucket keeps (spec v1.14): its capacity and refill,
    // and what it holds now. Children start with full buckets, taken from
    // what it holds.
    let pulse = ledger.pulse().map_err(|e| e.to_string())?;
    let (kept, remaining) = match ledger.object(allotment).map_err(|e| e.to_string())? {
        Some(object) => match object.data {
            ObjectData::Allotment(a) => (a.bucket, a.remaining),
            _ => return Err("the budget is not an allotment".into()),
        },
        None => return Err("the budget is gone".into()),
    };
    let holds = kept.level(pulse).min(kept.burst);
    // Each child may spend its whole budget at once if the bucket holds
    // that much for all of them; otherwise they share what it holds, in
    // proportion to their budgets. Each gets the share of the parent's
    // refill that its budget is of the parent's remaining budget.
    let children: Vec<Spawn> = orders
        .iter()
        .map(|order| {
            let mut caveats = charter.caveats.clone();
            let burst = if total.0 <= holds.0 {
                order.budget.0
            } else {
                holds.0.saturating_mul(order.budget.0) / total.0.max(1)
            };
            caveats.burst = Amount(burst.min(order.budget.0));
            caveats.rate = Amount(kept.rate.0.saturating_mul(order.budget.0) / remaining.0.max(1));
            caveats.max_per_payment = Amount(caveats.max_per_payment.0.min(order.budget.0));
            caveats.may_delegate = caveats.may_delegate && delegation.depth + 1 < MAX_DEPTH;
            // Keyless (spec v1.13): no key is made or kept for it; the wallet
            // that signs for this agent signs for it too.
            Spawn {
                agent: None,
                card: Hash32::ZERO,
                caveats,
                budget: order.budget,
            }
        })
        .collect();
    let mut session = Session::open(ledger).map_err(|e| e.to_string())?;
    let signer = session.signer_of(&acting.wallet);
    let reference = session.reference(&allotment).map_err(|e| e.to_string())?;
    let view = session
        .send(
            &acting.wallet,
            TxKind::SpawnAgents {
                charter: acting.agent,
                children,
            },
            vec![reference],
        )
        .map_err(|e| e.to_string())?;
    let names = session.names();
    if view.status != ExecStatus::Success {
        return Err(format!(
            "refused by the ledger: {}",
            crate::describe::receipt(&names, &view.effects)
        ));
    }
    let spawned: Vec<ObjectId> = view
        .effects
        .events
        .iter()
        .filter_map(|e| match e {
            Event::AgentSpawned { agent, .. } => Some(*agent),
            _ => None,
        })
        .collect();
    if spawned.len() != orders.len() {
        return Err(format!(
            "the ledger spawned {} subagents of {}",
            spawned.len(),
            orders.len()
        ));
    }
    let mut launched = Vec::new();
    for (order, child) in orders.iter().zip(spawned) {
        let wallet = fresh_name(&format!("{}-{}", acting.wallet, order.name));
        crate::session::create_keyless_alias(
            &wallet,
            &crate::session::KeylessAlias {
                charter: child,
                signer: signer.clone(),
            },
        )
        .map_err(|e| e.to_string())?;
        launched.push(Launched { wallet, child });
    }
    // Each subagent pays its transactions' fees from its own account; the
    // keyed account that signs for it gives it the money, all in one
    // transfer (a keyless parent has only fee money of its own).
    let grant = crate::agents::parse_amount(FEE_GRANT).map_err(|e| e.to_string())?;
    let outputs = launched
        .iter()
        .map(|l| (sion_exec::charter::keyless_account(&l.child), grant))
        .collect();
    let funded = session
        .send(
            &signer,
            TxKind::Transfer {
                asset: AssetId::usd(),
                outputs,
            },
            Vec::new(),
        )
        .map(|v| v.status == ExecStatus::Success)
        .unwrap_or(false);
    if !funded {
        return Err(format!(
            "spawned {}, but {signer} could not give them {FEE_GRANT} USD each for fees",
            launched
                .iter()
                .map(|l| l.wallet.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(launched)
}

/// Spawns the subagents `inputs` ask for and runs them at once, tainted
/// from the start if `tainted`, each with an even share of `budget` tokens
/// if there is one, and returns each one's report.
fn spawn_all(
    delegation: &Delegation,
    inputs: Vec<&Value>,
    tainted: bool,
    budget: Option<u64>,
) -> Vec<Result<Report, String>> {
    let checked = orders(delegation, &inputs);
    let valid: Vec<&Order> = checked.iter().filter_map(|o| o.as_ref().ok()).collect();
    let share = budget.map(|budget| budget / valid.len().max(1) as u64);
    let launched = if valid.is_empty() {
        Ok(Vec::new())
    } else {
        launch(delegation, &valid)
    };
    let launched = match launched {
        Ok(launched) => launched,
        Err(e) => {
            return checked
                .into_iter()
                .map(|o| o.and_then(|_| Err(e.clone())))
                .collect()
        }
    };
    let signer = Session::open(delegation.acting.ledger.as_ref())
        .map(|mut session| session.signer_of(&delegation.acting.wallet))
        .unwrap_or_else(|_| delegation.acting.wallet.clone());
    let before = crate::batcher::of(&signer).stats();
    let runs: Vec<Result<Report, String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = valid
            .iter()
            .zip(&launched)
            .map(|(order, launched)| {
                scope.spawn(move || work(delegation, order, launched, tainted, share))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err("the subagent's session failed".into()))
            })
            .collect()
    });
    // How the transactions signed for them meanwhile went out.
    let after = crate::batcher::of(&signer).stats();
    let (members, batches) = (
        after.members.saturating_sub(before.members),
        after.batches.saturating_sub(before.batches),
    );
    let signed = if members > 0 && launched.len() > 1 {
        format!(
            " While the {} subagents worked, {signer} signed {members} of their transactions in \
             {batches} {}.",
            launched.len(),
            if batches == 1 { "batch" } else { "batches" }
        )
    } else {
        String::new()
    };
    let mut runs = runs.into_iter();
    checked
        .into_iter()
        .map(|o| {
            o.and_then(|_| runs.next().unwrap_or_else(|| Err("no run".into())))
                .map(|report| Report {
                    text: format!("{}{signed}", report.text),
                    ..report
                })
        })
        .collect()
}

/// Runs a launched subagent's task in a session of its own, tainted from
/// the start if `tainted`, within `budget` tokens if there is one, and
/// reports how it went.
fn work(
    delegation: &Delegation,
    order: &Order,
    launched: &Launched,
    tainted: bool,
    budget: Option<u64>,
) -> Result<Report, String> {
    let acting = &delegation.acting;
    let Launched { wallet, child } = launched;
    let (saved, _) = crate::runtime::start_watched(
        acting.ledger.clone(),
        wallet,
        *child,
        &order.task,
        &[],
        delegation.models.clone(),
        delegation.policy.clone(),
        // A subagent gets no workspace, shell or web of its parent's: only
        // the ledger tools and what its charter allows.
        Equipment {
            depth: delegation.depth + 1,
            subagent_models: delegation.models.clone(),
            adjudicator: delegation.adjudicator,
            budget_tokens: budget,
            ..Equipment::default()
        },
        crate::runtime::Hooks {
            outside: tainted,
            ..crate::runtime::Hooks::default()
        },
    )
    .map_err(|e| {
        format!(
            "spawned agent {}, but its session failed: {e}",
            child.short()
        )
    })?;
    let ended = match &saved.ended {
        Some(sion_agent::Ended::Done { text }) => {
            format!("done; it said: {}", text.trim_end_matches('.'))
        }
        Some(sion_agent::Ended::NeedsApproval { tool, .. }) => format!(
            "waiting for a person's approval of {tool} (`sion agent continue {}`)",
            saved.state.id
        ),
        Some(sion_agent::Ended::Stuck { why }) => {
            format!("stopped going round in circles ({why})")
        }
        Some(sion_agent::Ended::OutOfBudget { text }) if text.trim().is_empty() => {
            "its token budget ran out before it could answer".to_string()
        }
        Some(sion_agent::Ended::OutOfBudget { text }) => format!(
            "its token budget ran out; it said: {}",
            text.trim_end_matches('.')
        ),
        Some(sion_agent::Ended::Unverified { text, .. }) => format!(
            "its answer did not pass its check; its best: {}",
            text.trim_end_matches('.')
        ),
        Some(sion_agent::Ended::CannotComplete { why }) => {
            format!("it cannot complete the task: {why}")
        }
        Some(other) => format!("ended: {other:?}"),
        None => "did not run".into(),
    };
    // What it called, and how each call went, in a few words.
    let mut names = std::collections::BTreeMap::new();
    let mut calls = Vec::new();
    for message in &saved.state.messages {
        for block in &message.content {
            match block {
                sion_agent::Block::ToolUse { id, name, .. } => {
                    names.insert(id.clone(), name.clone());
                }
                sion_agent::Block::ToolResult {
                    id,
                    content,
                    is_error,
                    ..
                } => {
                    let name = names.get(id).cloned().unwrap_or_default();
                    calls.push(if *is_error {
                        let reason: String = content.chars().take(80).collect();
                        format!("{name} failed ({reason})")
                    } else {
                        format!("{name} ok")
                    });
                }
                _ => {}
            }
        }
    }
    let tainted = saved.state.standing.tainted_until.is_some();
    let mut spent = saved.state.usage;
    spent.add(&saved.state.delegated);
    Ok(Report {
        text: format!(
            "subagent {wallet} (agent {}, budget {} USD) worked in session {}: {ended}. Its calls: {}.",
            child.short(),
            order.budget,
            saved.state.id,
            if calls.is_empty() {
                "none".to_string()
            } else {
                calls.join(", ")
            }
        ),
        tainted,
        spent,
    })
}

/// Posts a Task Charter from the agent's budget.
struct TaskPostTool(Delegation);

impl Tool for TaskPostTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "task_post".into(),
            description: "Hire another agent: escrow money from your budget for a piece of work, \
                          paid only for a result that passes the rule fixed now. Give the amount \
                          in USD, the work's description in full, and optionally the worker (a \
                          wallet name or account), verifiers whose attestations count, those of \
                          them who must attest every result, how many attestations a result \
                          needs at least, and the deadline in Pulses (default 1000)."
                .into(),
            input_schema: schema(
                json!({
                    "amount": {"type": "string"},
                    "description": {"type": "string"},
                    "worker": {"type": "string"},
                    "verifiers": {"type": "array", "items": {"type": "string"}},
                    "must_attest": {"type": "array", "items": {"type": "string"}},
                    "min_attestations": {"type": "integer"},
                    "deadline": {"type": "integer"}
                }),
                &["amount", "description"],
            ),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Spend
    }

    fn call(&self, input: &Value, _key: &str) -> Outcome {
        match post(&self.0, input) {
            Ok(text) => Outcome::ok(text),
            Err(e) => Outcome::error(format!("not posted: {e}")),
        }
    }
}

fn post(delegation: &Delegation, input: &Value) -> Result<String, String> {
    let acting = &delegation.acting;
    let amount = crate::agents::parse_amount(text(input, "amount")).map_err(|e| e.to_string())?;
    let description = text(input, "description");
    if description.is_empty() {
        return Err("describe the work".into());
    }
    let ledger = acting.ledger.as_ref();
    let (allotment, _) = allotment_holding(ledger, acting.agent, amount)?;
    let mut session = Session::open(ledger).map_err(|e| e.to_string())?;
    let spec = crate::tasks::spec_commitment(description.as_bytes());
    keep(description.as_bytes(), &spec)?;
    let worker = match text(input, "worker") {
        "" => None,
        w => Some(session.id(w)),
    };
    let verifiers = input["verifiers"]
        .as_array()
        .map(|v| {
            v.iter()
                .filter_map(|x| x.as_str())
                .map(|x| session.id(x))
                .collect()
        })
        .unwrap_or_default();
    let must_attest = input["must_attest"]
        .as_array()
        .map(|v| {
            v.iter()
                .filter_map(|x| x.as_str())
                .map(|x| session.id(x))
                .collect()
        })
        .unwrap_or_default();
    let min_attestations = input["min_attestations"]
        .as_u64()
        .unwrap_or(0)
        .min(u64::from(u8::MAX)) as u8;
    let deadline = input["deadline"].as_u64().unwrap_or(1_000).max(1);
    let now = ledger.pulse().map_err(|e| e.to_string())?;
    let reference = session.reference(&allotment).map_err(|e| e.to_string())?;
    let view = session
        .send(
            &acting.wallet,
            TxKind::PostTask {
                spec,
                acceptance: Acceptance {
                    policy: Assurance::standard(),
                    requires: Default::default(),
                    verifiers,
                    must_attest,
                    min_attestations,
                    adjudicator: delegation.adjudicator,
                    detection_permille: 900,
                    reliance: 1_000,
                },
                worker,
                amount,
                deadline: Pulse(now.0 + deadline),
                charter: Some(acting.agent),
            },
            vec![reference],
        )
        .map_err(|e| e.to_string())?;
    let names = session.names();
    let receipt = crate::describe::receipt(&names, &view.effects);
    if view.status != ExecStatus::Success {
        return Err(format!("refused by the ledger: {receipt}"));
    }
    let task = view
        .effects
        .events
        .iter()
        .find_map(|e| match e {
            Event::TaskPosted { task, .. } => Some(*task),
            _ => None,
        })
        .ok_or("the ledger posted no task")?;
    Ok(format!(
        "Pulse {}: {receipt} Task {task}{}.",
        view.pulse.0,
        if delegation.adjudicator.is_none() {
            "; it names no adjudicator, so its results cannot be challenged"
        } else {
            ""
        }
    ))
}

fn parse_task(input: &Value) -> Result<ObjectId, String> {
    Hash32::from_hex(text(input, "task"))
        .map(ObjectId::from_hash)
        .map_err(|_| "give the task's id (64 hex digits)".to_string())
}

/// A task's terms and state.
struct TaskStatusTool(Acting);

impl Tool for TaskStatusTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "task_status".into(),
            description: "A task's terms and state: open, settled (and with which result) or \
                          cancelled. If its output is kept on this machine, it is included."
                .into(),
            input_schema: schema(json!({"task": {"type": "string"}}), &["task"]),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn call(&self, input: &Value, _key: &str) -> Outcome {
        let task = match parse_task(input) {
            Ok(task) => task,
            Err(e) => return Outcome::error(e),
        };
        let ledger = self.0.ledger.as_ref();
        let mut value = match crate::tasks::describe(ledger, task) {
            Ok(value) => value,
            Err(e) => return Outcome::error(e.to_string()),
        };
        // The output of a settled task, if it is kept here.
        if let Some(result) = value["state"]["Settled"]["result"].as_str() {
            if let Ok(stored) = crate::tasks::stored_claim(ledger, result) {
                if let sion_exec::claim::Payload::Public(bytes) = &stored.body.payload {
                    if let Ok(committed) =
                        sion_core::encode::decode::<sion_exec::task::TaskResult>(bytes)
                    {
                        let path = tasks_dir().join(committed.output.to_hex());
                        if let Ok(output) = std::fs::read_to_string(path) {
                            value["output"] = json!(output);
                        }
                    }
                }
            }
        }
        // A task's output is another agent's work: content from outside.
        if value.get("output").is_some() {
            Outcome::ok(value.to_string()).from_outside()
        } else {
            Outcome::ok(value.to_string())
        }
    }
}

/// The worker's side: a result on its task bond, and the settlement.
struct TaskSubmitTool(Acting);

impl Tool for TaskSubmitTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "task_submit".into(),
            description: "Deliver the work for a task you were hired for, and be paid if the \
                          result passes the task's rule: give the task's id and the output. The \
                          result rests on your task bond (bond:<you>:task-worker)."
                .into(),
            input_schema: schema(
                json!({"task": {"type": "string"}, "output": {"type": "string"}}),
                &["task", "output"],
            ),
        }
    }

    fn effect(&self) -> Effect {
        Effect::External
    }

    fn call(&self, input: &Value, _key: &str) -> Outcome {
        match submit(&self.0, input) {
            Ok(text) => Outcome::ok(text),
            Err(e) => Outcome::error(format!("not delivered: {e}")),
        }
    }
}

fn submit(acting: &Acting, input: &Value) -> Result<String, String> {
    let task = parse_task(input)?;
    let output = input["output"].as_str().unwrap_or_default();
    if output.trim().is_empty() {
        return Err("give the output".into());
    }
    let ledger = acting.ledger.as_ref();
    let mut session = Session::open(ledger).map_err(|e| e.to_string())?;
    let who = session.id(&acting.wallet);
    let body = crate::tasks::result_body(ledger, task, &who, output.as_bytes(), &[])
        .map_err(|e| e.to_string())?;
    let id = sion_exec::claim::claim_id(&who, &body);
    let view = session
        .send(
            &acting.wallet,
            TxKind::IssueClaim { claim: body },
            Vec::new(),
        )
        .map_err(|e| e.to_string())?;
    if view.status != ExecStatus::Success {
        return Err(format!(
            "the result was refused: {}",
            crate::describe::receipt(&session.names(), &view.effects)
        ));
    }
    let Ok(result) = crate::tasks::stored_claim(ledger, &id.to_string()) else {
        return Err(format!(
            "the result did not join the knowledge log: post a task bond first \
             (bond:{}:task-worker)",
            acting.wallet
        ));
    };
    keep(
        output.as_bytes(),
        &crate::tasks::output_commitment(output.as_bytes()),
    )?;
    let view = session
        .send(
            &acting.wallet,
            TxKind::SettleTask {
                task,
                result: result.body,
                attestations: Vec::new(),
            },
            Vec::new(),
        )
        .map_err(|e| e.to_string())?;
    let receipt = crate::describe::receipt(&session.names(), &view.effects);
    if view.status != ExecStatus::Success {
        return Err(format!("not settled: {receipt}"));
    }
    Ok(format!("Pulse {}: {receipt} Result {id}.", view.pulse.0))
}

impl std::fmt::Debug for Delegation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Delegation")
            .field("acting", &self.acting)
            .field("models", &self.models)
            .field("depth", &self.depth)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subagent_names_are_short_and_plain() {
        assert!(valid_name("researcher-1"));
        assert!(!valid_name("Researcher"));
        assert!(!valid_name("-x"));
        assert!(!valid_name(&"x".repeat(33)));
    }
}
