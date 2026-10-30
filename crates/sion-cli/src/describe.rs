//! Transactions and receipts as sentences.
//!
//! People should read what they sign and what happened as plain language
//! ("Pay 12.5 USD to alice"), not as a hexadecimal payload. Every
//! transaction kind and every event has a sentence. Wallets show the
//! sentence before signing, the CLI prints it with every receipt, and the
//! MCP tools return it to agents.
//!
//! A sentence is a rendering of the signed bytes, not a substitute for them:
//! it names accounts and assets through a local address book ([`Names`]),
//! falling back to short identifiers for anything it does not know.

use std::collections::BTreeMap;

use sion_core::ids::{AccountId, AssetId, AtomId, ClaimId, ObjectId};
use sion_core::Amount;
use sion_exec::charter::Caveats;
use sion_exec::market::Submission;
use sion_exec::{Event, ExecStatus, Transaction, TxEffects, TxKind, Wallet};

/// A local address book: names for accounts and assets.
#[derive(Clone, Debug, Default)]
pub struct Names {
    accounts: BTreeMap<AccountId, String>,
    assets: BTreeMap<AssetId, String>,
}

impl Names {
    /// An address book knowing the devnet wallets of these names and the
    /// usual assets.
    pub fn devnet<'a>(names: impl IntoIterator<Item = &'a str>) -> Names {
        let mut book = Names::default();
        for name in names {
            book.add_account(Wallet::named(name).id, name);
        }
        book.add_asset(AssetId::usd(), "USD");
        book.add_asset(AssetId::derive(b"gpu-hour"), "GPU-hours");
        book
    }

    /// Names an account.
    pub fn add_account(&mut self, id: AccountId, name: &str) {
        self.accounts.insert(id, name.to_string());
    }

    /// Names an asset.
    pub fn add_asset(&mut self, id: AssetId, name: &str) {
        self.assets.insert(id, name.to_string());
    }

    /// An account's name, or its short identifier.
    pub fn account(&self, id: &AccountId) -> String {
        self.accounts
            .get(id)
            .cloned()
            .unwrap_or_else(|| format!("account {}", id.short()))
    }

    /// An asset's name, or its short identifier.
    pub fn asset(&self, id: &AssetId) -> String {
        self.assets
            .get(id)
            .cloned()
            .unwrap_or_else(|| format!("asset {}", id.short()))
    }

    /// `amount asset`, for example "12.5 USD".
    pub fn money(&self, amount: Amount, asset: &AssetId) -> String {
        format!("{amount} {}", self.asset(asset))
    }

    /// An amount of USD, as sentences show it.
    pub fn usd(&self, amount: Amount) -> String {
        self.money(amount, &AssetId::usd())
    }
}

fn list(items: Vec<String>) -> String {
    match items.len() {
        0 => "nothing".into(),
        1 => items[0].clone(),
        n => format!("{} and {}", items[..n - 1].join(", "), items[n - 1]),
    }
}

/// " with card <short>" for a set card commitment, nothing for none.
fn card_phrase(card: &sion_core::hash::Hash32) -> String {
    if *card == sion_core::hash::Hash32::ZERO {
        String::new()
    } else {
        format!(" with card {}", card.short())
    }
}

fn object(id: &ObjectId) -> String {
    id.short()
}

/// What a task's rule asks of attestations (spec v1.15), as a clause after
/// the required claims: empty when it asks nothing.
fn attestation_terms(names: &Names, acceptance: &sion_exec::task::Acceptance) -> String {
    let must: Vec<String> = acceptance
        .must_attest
        .iter()
        .map(|v| names.account(v))
        .collect();
    let at_least = usize::from(acceptance.min_attestations);
    let count = |n: usize| {
        if n == 1 {
            "1 attestation".to_string()
        } else {
            format!("{n} attestations")
        }
    };
    match (must.is_empty(), at_least > must.len()) {
        (true, false) => String::new(),
        (true, true) => format!(", with at least {}", count(at_least)),
        (false, false) => format!(", attested by {}", join_and(&must)),
        (false, true) => format!(
            ", attested by {}, with at least {}",
            join_and(&must),
            count(at_least)
        ),
    }
}

fn join_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// `text` with anything but printable ASCII shown as `?`.
fn printable(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                '?'
            }
        })
        .collect()
}

fn claim(id: &ClaimId) -> String {
    format!("claim {}", id.short())
}

fn atom(id: &AtomId) -> String {
    format!("assumption {}", id.short())
}

fn caveats(names: &Names, k: &Caveats) -> String {
    let payees = match &k.payees {
        None => "any payee".to_string(),
        Some(set) => format!(
            "only to {}",
            list(set.iter().map(|a| names.account(a)).collect())
        ),
    };
    let mut parts = vec![
        payees,
        format!("at most {} per payment", names.usd(k.max_per_payment)),
        bucket(names, k.burst, k.rate),
        format!("until Pulse {}", k.expires_at),
    ];
    if let Some(above) = k.evidence_above {
        parts.push(format!(
            "with evidence for payments above {}",
            names.usd(above)
        ));
    }
    if k.receipts {
        parts.push("a receipt claim for every payment".into());
    }
    parts.push(if k.may_delegate {
        "may delegate narrower limits".into()
    } else {
        "may not delegate".into()
    });
    parts.join(", ")
}

/// A token bucket, in words.
pub fn bucket(names: &Names, burst: Amount, rate: Amount) -> String {
    if rate == Amount::ZERO {
        format!("{} at once and in all, never refilled", names.usd(burst))
    } else {
        format!(
            "{} at once, refilled by {} a Pulse",
            names.usd(burst),
            names.usd(rate)
        )
    }
}

/// A narrowing, in words.
fn narrowing(names: &Names, n: &sion_exec::charter::Narrowing) -> String {
    let payees = match &n.payees {
        None => "any payee its own limits allow".to_string(),
        Some(set) => format!(
            "only to {}",
            list(set.iter().map(|a| names.account(a)).collect())
        ),
    };
    format!(
        "{payees}, at most {} per payment, until Pulse {}{}",
        names.usd(n.max_per_payment),
        n.expires_at,
        if n.may_delegate {
            ""
        } else {
            ", no new subagents"
        }
    )
}

/// What a transaction asks for, as one sentence.
pub fn sentence(names: &Names, tx: &Transaction) -> String {
    let body = match &tx.kind {
        TxKind::Transfer { asset, outputs } => format!(
            "Pay {}",
            list(
                outputs
                    .iter()
                    .map(|(to, amount)| format!(
                        "{} to {}",
                        names.money(*amount, asset),
                        names.account(to)
                    ))
                    .collect()
            )
        ),
        TxKind::Faucet { grants } => format!(
            "Mint {} (devnet faucet)",
            list(
                grants
                    .iter()
                    .map(|(to, asset, amount)| format!(
                        "{} for {}",
                        names.money(*amount, asset),
                        names.account(to)
                    ))
                    .collect()
            )
        ),
        TxKind::RotatePolicy { viewing_key, .. } => format!(
            "Replace this account's keys{} (the account keeps its identifier)",
            if viewing_key.is_some() {
                " and viewing key"
            } else {
                ""
            }
        ),
        TxKind::IssueClaim { claim: body } => format!(
            "Publish the claim \"{}\" ({}), valid for {} Pulses, resting on {}",
            body.subject,
            body.schema,
            body.valid_for,
            if body.formula.direct_atoms().is_empty() && body.formula.claim_refs().is_empty() {
                "nothing".to_string()
            } else {
                format!(
                    "{} assumptions and {} other claims",
                    body.formula.direct_atoms().len(),
                    body.formula.claim_refs().len()
                )
            }
        ),
        TxKind::RegisterAtom { class, label } => {
            format!("Register the assumption \"{label}\" ({class:?})")
        }
        TxKind::PostBond {
            class,
            label,
            amount,
        } => format!(
            "Post a {} bond behind your {class:?} assumption \"{label}\"",
            names.usd(*amount)
        ),
        TxKind::Accept {
            claim: c,
            value_at_risk,
            until,
            ..
        } => format!(
            "Rely on {} for up to {} until Pulse {until}, reserving its backing",
            claim(c),
            names.usd(*value_at_risk)
        ),
        TxKind::Release { reservation } => {
            format!("Release reservation {}", reservation.short())
        }
        TxKind::Challenge {
            claim: c,
            target,
            deposit,
            ..
        } => format!(
            "Challenge {} of {}, depositing {}",
            atom(target),
            claim(c),
            names.usd(*deposit)
        ),
        TxKind::Rule { challenge, upheld } => format!(
            "As adjudicator, {} challenge {}",
            if *upheld { "uphold" } else { "reject" },
            challenge.short()
        ),
        TxKind::WithdrawBond { atom: a } => {
            format!("Withdraw the free part of the bond behind {}", atom(a))
        }
        TxKind::CreateCharter {
            agent,
            caveats: k,
            funding,
            agent_bond,
        } => format!(
            "Give {} an Agent Charter funded with {}{}: {}",
            names.account(agent),
            names.usd(*funding),
            if *agent_bond > Amount::ZERO {
                format!(" and a {} bond behind its receipts", names.usd(*agent_bond))
            } else {
                String::new()
            },
            caveats(names, k)
        ),
        TxKind::Delegate {
            charter,
            sub_agent,
            caveats: k,
            amount,
        } => format!(
            "Delegate {} from charter {} to {}: {}",
            names.usd(*amount),
            object(charter),
            names.account(sub_agent),
            caveats(names, k)
        ),
        TxKind::AgentPay {
            charter,
            payee,
            amount,
            purpose,
            evidence,
        } => format!(
            "As an agent under charter {}, pay {} to {} for \"{purpose}\"{}",
            object(charter),
            names.usd(*amount),
            names.account(payee),
            match evidence {
                Some(c) => format!(", citing {}", claim(c)),
                None => String::new(),
            }
        ),
        TxKind::RevokeCharter { charter } => format!(
            "Revoke charter {} and every charter delegated from it",
            object(charter)
        ),
        TxKind::ReclaimCharter { charter } => format!(
            "Return what is left in dead charter {}'s allotments to its backer",
            object(charter)
        ),
        TxKind::RegisterAgent {
            agent,
            card,
            caveats: k,
            funding,
            agent_bond,
        } => format!(
            "Register {} as an agent{}, funded with {}{}: {}",
            names.account(agent),
            card_phrase(card),
            names.usd(*funding),
            if *agent_bond > Amount::ZERO {
                format!(" and a {} bond behind its receipts", names.usd(*agent_bond))
            } else {
                String::new()
            },
            caveats(names, k)
        ),
        TxKind::SpawnAgents { charter, children } => {
            let sum = |f: fn(&sion_exec::charter::Spawn) -> u128| {
                Amount(children.iter().map(f).fold(0u128, u128::saturating_add))
            };
            let n = children.len();
            format!(
                "As agent {}, launch {n} {}, {} in budgets and {} in all",
                object(charter),
                if n == 1 {
                    "subagent with its own identity"
                } else {
                    "subagents with their own identities"
                },
                names.usd(sum(|c| c.budget.0)),
                bucket(
                    names,
                    sum(|c| c.caveats.burst.0),
                    sum(|c| c.caveats.rate.0)
                )
            )
        }
        TxKind::SpawnCohort {
            charter,
            count,
            burst,
            rate,
            full,
            ..
        } => format!(
            "As agent {}, commit to a cohort of {count} {}, with {} in all{}",
            object(charter),
            if *count == 1 { "subagent" } else { "subagents" },
            bucket(names, *burst, *rate),
            if *full {
                ", their buckets full from now"
            } else {
                ", their buckets filling from now"
            }
        ),
        TxKind::MaterializeAgents { cohort, members } => {
            let budget = Amount(members.iter().map(|m| m.member.budget.0).sum());
            let n = members.len();
            format!(
                "Materialize {n} {} of cohort {}, with {} in budgets in all",
                if n == 1 { "member" } else { "members" },
                object(cohort),
                names.usd(budget)
            )
        }
        TxKind::NarrowAgent {
            charter,
            narrowing: Some(n),
        } => format!(
            "Narrow agent {} and every agent below it: {}",
            object(charter),
            narrowing(names, n)
        ),
        TxKind::NarrowAgent {
            charter,
            narrowing: None,
        } => format!("Lift the narrowing of agent {}", object(charter)),
        TxKind::SetAgentCard { charter, card } => format!(
            "Set agent {}'s card to {}",
            object(charter),
            card.short()
        ),
        TxKind::SuspendAgent { charter } => format!(
            "Suspend agent {} and every agent below it",
            object(charter)
        ),
        TxKind::ResumeAgent { charter } => {
            format!("Lift the suspension of agent {}", object(charter))
        }
        TxKind::CreateBounded {
            label,
            asset,
            total,
            allotments,
        } => format!(
            "Lock {} as the shared total \"{label}\", split into {} single-writer allotments",
            names.money(*total, asset),
            allotments.len()
        ),
        TxKind::SpendAllotment { to, amount } => format!(
            "Spend {} from your allotment, to {}",
            names.usd(*amount),
            names.account(to)
        ),
        TxKind::ReturnAllotment => "Return the rest of your allotment to its total".into(),
        TxKind::Rebalance {
            bounded,
            holder,
            amount,
        } => format!(
            "Allot {} of shared total {} to {}",
            names.usd(*amount),
            object(bounded),
            names.account(holder)
        ),
        TxKind::CreateMarket {
            label,
            good,
            numeraire,
            batch_length,
            committee,
            ..
        } => format!(
            "Open the market \"{label}\" for {} priced in {}, clearing every {batch_length} Pulses{}",
            names.asset(good),
            names.asset(numeraire),
            if committee.is_empty() {
                ""
            } else {
                " with sealed orders"
            }
        ),
        TxKind::SubmitOrder { market, submission } => match submission {
            Submission::Plain(terms) => format!(
                "{} {} units, {} {} each, in market {}",
                match terms.side {
                    sion_clearing::Side::Buy => "Bid for",
                    sion_clearing::Side::Sell => "Offer",
                },
                terms.quantity,
                match terms.side {
                    sion_clearing::Side::Buy => "paying at most",
                    sion_clearing::Side::Sell => "accepting at least",
                },
                terms.limit_price,
                object(market)
            ),
            Submission::Sealed { side, escrow, .. } => format!(
                "Submit a sealed {} order to market {}, escrowing {escrow}",
                match side {
                    sion_clearing::Side::Buy => "buy",
                    sion_clearing::Side::Sell => "sell",
                },
                object(market)
            ),
        },
        TxKind::RevealBatch {
            market,
            batch,
            shares,
        } => format!(
            "Reveal {} sealed orders of batch {batch} in market {}",
            shares.len(),
            object(market)
        ),
        TxKind::ClearBatch { market, batch, .. } => format!(
            "Settle batch {batch} of market {} with a certified-optimal clearing",
            object(market)
        ),
        TxKind::RegisterValidator { stake, .. } => format!(
            "Register a validator staking {}; it joins the active set at the next epoch boundary",
            names.usd(*stake)
        ),
        TxKind::Unstake { validator } => format!(
            "Take validator {} out of the active set at the next epoch boundary",
            validator.short()
        ),
        TxKind::WithdrawStake { validator } => format!(
            "Withdraw validator {}'s stake and unclaimed rewards after unbonding",
            validator.short()
        ),
        TxKind::ClaimRewards { validator } => {
            format!("Collect validator {}'s rewards", validator.short())
        }
        TxKind::ReportEquivocation { evidence } => format!(
            "Report that a validator signed two conflicting {}; its stake is slashed and you receive a share",
            match evidence.as_ref() {
                sion_exec::validators::Equivocation::Headers { .. } => "consensus headers",
                sion_exec::validators::Equivocation::Approvals { .. } => "lane approvals",
            }
        ),
        TxKind::OpenStream {
            payee,
            asset,
            unit,
            length,
            deposit,
            until,
            checkpoints,
            ..
        } => format!(
            "Open a payment stream to {}: escrow {}, paid off the ledger at {} a step for up to {} steps, settled by {} until Pulse {until}{}",
            names.account(payee),
            names.money(*deposit, asset),
            names.money(*unit, asset),
            sion_core::Amount::units(*length),
            names.account(payee),
            match checkpoints {
                Some(c) => format!(
                    ", with a checkpoint every {} steps",
                    sion_core::Amount::units(c.spacing)
                ),
                None => String::new(),
            }
        ),
        TxKind::SettleStream {
            stream,
            position,
            jump,
            ..
        } => format!(
            "Settle stream {} up to step {position}{}",
            stream.short(),
            match jump {
                Some(jump) => format!(", proven by checkpoint {}", jump.index),
                None => String::new(),
            }
        ),
        TxKind::TopUpStream {
            stream,
            deposit,
            until,
        } => format!(
            "Add {} to stream {} and keep it open until Pulse {until}",
            names.usd(*deposit),
            stream.short()
        ),
        TxKind::CloseStream { stream } => format!(
            "Close stream {} and return what is left in escrow to its payer",
            stream.short()
        ),
        TxKind::OpenMemory { agent, .. } => match agent {
            Some(agent) => format!(
                "Open a memory log for agent {}, which only it can append to",
                object(agent)
            ),
            None => "Open a memory log that only the sender can append to".to_string(),
        },
        TxKind::CommitMemory { size, seal, .. } => {
            let memory_log = tx
                .inputs
                .get(1)
                .map(|input| format!("memory log {}", object(&input.id)))
                .unwrap_or_else(|| "the memory log".to_string());
            format!(
                "Commit {memory_log} at {size} {}, proving it only grew{}",
                if *size == 1 { "atom" } else { "atoms" },
                if *seal { ", and seal it" } else { "" }
            )
        }
        TxKind::PostTask {
            acceptance,
            worker,
            amount,
            deadline,
            charter,
            ..
        } => format!(
            "Escrow {} for a task {}, paid {} for a result resting on {} required {}{} and accepted under your policy, settled by Pulse {deadline}{}",
            names.usd(*amount),
            match worker {
                Some(worker) => format!("for {}", names.account(worker)),
                None => "open to anyone".to_string(),
            },
            if worker.is_some() { "to them" } else { "to the first worker" },
            acceptance.requires.len(),
            if acceptance.requires.len() == 1 { "claim" } else { "claims" },
            attestation_terms(names, acceptance),
            match charter {
                Some(charter) => format!(", from agent {}'s budget", object(charter)),
                None => String::new(),
            }
        ),
        TxKind::SettleTask {
            task, attestations, ..
        } => format!(
            "Settle task {} with your result{}",
            object(task),
            match attestations.len() {
                0 => String::new(),
                1 => " and 1 attestation".to_string(),
                n => format!(" and {n} attestations"),
            }
        ),
        TxKind::CancelTask { task } => format!(
            "Cancel task {} and refund its escrow",
            object(task)
        ),
        TxKind::IndexClaim { key, claim: c } => format!(
            "Name {} as your latest claim under \"{}\" (your index entry moves only to a later claim of yours)",
            claim(c),
            printable(key)
        ),
        TxKind::DelegateStake { validator, amount } => format!(
            "Delegate {} to validator {}'s pool; you keep the delegation, which earns from the epoch's end",
            names.usd(*amount),
            validator.short()
        ),
        TxKind::Undelegate => {
            "Stop a delegation earning, collect its rewards and start unbonding".into()
        }
        TxKind::WithdrawDelegation => "Withdraw an unbonded delegation's stake".into(),
        TxKind::ClaimDelegatorRewards => "Collect a delegation's rewards".into(),
        TxKind::SetCommission {
            validator,
            permille,
        } => format!(
            "Set validator {}'s commission to {} from the epoch's end",
            validator.short(),
            percent(*permille)
        ),
        TxKind::PostDeal {
            root,
            size,
            challenges,
            price,
            rounds,
            round_length,
            collateral,
            provider,
            ..
        } => format!(
            "Escrow {} to have data {} ({size} leaves) kept by {} for {rounds} rounds of {round_length} Pulses: {} per round proven by {challenges} random leaves, with {} of collateral at stake",
            names.usd(Amount(price.0.saturating_mul(u128::from(*rounds)))),
            root.short(),
            match provider {
                Some(provider) => names.account(provider),
                None => "any provider".to_string(),
            },
            names.usd(*price),
            names.usd(*collateral)
        ),
        TxKind::AcceptDeal { deal } => format!(
            "Accept storage deal {}, posting its collateral; prove each round or lose it",
            object(deal)
        ),
        TxKind::ProveStorage { proofs } => {
            let leaves: usize = proofs.iter().map(|p| p.leaves.len()).sum();
            format!(
                "Prove custody of {} storage deal(s) for the round under way with {leaves} challenged leaves",
                proofs.len()
            )
        }
        TxKind::SettleDeal { deal } => format!(
            "Collect for the proven rounds of storage deal {}",
            object(deal)
        ),
        TxKind::CloseDeal { deal } => format!(
            "Close storage deal {}: pay what is due, refund the escrow of unproven rounds, settle the collateral",
            object(deal)
        ),
    };
    format!(
        "{body}. Fee {}; valid until Pulse {}.",
        names.usd(tx.fee),
        tx.expires_at
    )
}

/// What an event established, as one sentence.
pub fn event(names: &Names, event: &Event) -> String {
    match event {
        Event::Transferred { asset, outputs } => format!(
            "paid {}",
            list(
                outputs
                    .iter()
                    .map(|(to, a)| format!("{} to {}", names.money(*a, asset), names.account(to)))
                    .collect()
            )
        ),
        Event::Minted { to, asset, amount } => {
            format!(
                "minted {} for {}",
                names.money(*amount, asset),
                names.account(to)
            )
        }
        Event::PolicyRotated { account } => format!("{}'s keys rotated", names.account(account)),
        Event::ClaimSubmitted { claim: c } => format!("{} submitted", claim(c)),
        Event::ClaimIssued {
            claim: c,
            log_index,
        } => {
            format!(
                "{} joined the knowledge log at position {log_index}",
                claim(c)
            )
        }
        Event::ClaimRejected { claim: c, reason } => format!("{} rejected: {reason}", claim(c)),
        Event::AtomRegistered { atom: a } => format!("{} registered", atom(a)),
        Event::BondPosted {
            atom: a,
            owner,
            amount,
        } => format!(
            "{} posted a {} bond behind {}",
            names.account(owner),
            names.usd(*amount),
            atom(a)
        ),
        Event::Reserved {
            claim: c, amount, ..
        } => format!("reserved {} of {}'s backing", names.usd(*amount), claim(c)),
        Event::Released { reservation } => format!("reservation {} released", reservation.short()),
        Event::ChallengeOpened {
            claim: c, target, ..
        } => format!("challenge opened against {} of {}", atom(target), claim(c)),
        Event::ChallengeRuled { upheld, .. } => {
            format!("challenge {}", if *upheld { "upheld" } else { "rejected" })
        }
        Event::Slashed { atom: a, amount } => {
            format!("{} slashed from {}", names.usd(*amount), atom(a))
        }
        Event::Falsified { claim: c, .. } => format!("{} falsified", claim(c)),
        Event::Compensated {
            consumer, amount, ..
        } => format!(
            "{} compensated {}",
            names.account(consumer),
            names.usd(*amount)
        ),
        Event::BondWithdrawn { atom: a, amount } => {
            format!(
                "{} withdrawn from the bond behind {}",
                names.usd(*amount),
                atom(a)
            )
        }
        Event::CharterCreated { charter, .. } => format!("charter {} created", object(charter)),
        Event::CharterRevoked { charter } => format!("charter {} revoked", object(charter)),
        Event::AgentRegistered { agent, account, .. } => format!(
            "agent {} registered, signing as {}",
            object(agent),
            names.account(account)
        ),
        Event::AgentSpawned {
            agent,
            parent,
            account,
            ..
        } if *account == sion_exec::charter::keyless_account(agent) => format!(
            "subagent {} launched by {}, keyless: its nearest keyed ancestor signs for it",
            object(agent),
            object(parent)
        ),
        Event::AgentSpawned {
            agent,
            parent,
            account,
            ..
        } => format!(
            "subagent {} launched by {}, signing as {}",
            object(agent),
            object(parent),
            names.account(account)
        ),
        Event::AgentCardSet { agent, card } => {
            format!("agent {}'s card is now {}", object(agent), card.short())
        }
        Event::AgentSuspended { agent, by } => format!(
            "agent {} suspended by {}, with everything below it",
            object(agent),
            names.account(by)
        ),
        Event::AgentResumed { agent } => format!("agent {} resumed", object(agent)),
        Event::CohortSpawned {
            cohort,
            parent,
            first,
            count,
        } => format!(
            "cohort {} of {count} committed under {} (lineage indices {first} to {})",
            object(cohort),
            object(parent),
            first + u64::from(*count).saturating_sub(1)
        ),
        Event::AgentMaterialized {
            agent,
            cohort,
            index,
            account,
            ..
        } if *account == sion_exec::charter::keyless_account(agent) => format!(
            "member {index} of cohort {} materialized as agent {}, keyless",
            object(cohort),
            object(agent)
        ),
        Event::AgentMaterialized {
            agent,
            cohort,
            index,
            account,
            ..
        } => format!(
            "member {index} of cohort {} materialized as agent {}, signing as {}",
            object(cohort),
            object(agent),
            names.account(account)
        ),
        Event::AgentNarrowed {
            agent,
            by,
            narrowing: Some(_),
        } => format!(
            "agent {} narrowed by {}, with everything below it",
            object(agent),
            names.account(by)
        ),
        Event::AgentNarrowed {
            agent,
            by,
            narrowing: None,
        } => format!(
            "the narrowing of agent {} lifted by {}",
            object(agent),
            names.account(by)
        ),
        Event::ClaimIndexed {
            owner,
            key,
            claim: c,
            count,
            ..
        } => format!(
            "{}'s latest claim under \"{}\" is {} ({} named so far)",
            names.account(owner),
            printable(key),
            claim(c),
            count
        ),
        Event::CharterReclaimed { charter, amount } => format!(
            "{} returned from charter {}",
            names.usd(*amount),
            object(charter)
        ),
        Event::AgentPaid {
            payee,
            amount,
            receipt,
            ..
        } => format!(
            "agent paid {} to {}{}",
            names.usd(*amount),
            names.account(payee),
            match receipt {
                Some(c) => format!(" (receipt {})", claim(c)),
                None => String::new(),
            }
        ),
        Event::BoundedCreated { allotments, .. } => {
            format!("shared total created with {} allotments", allotments.len())
        }
        Event::AllotmentUsed { amount, .. } => {
            format!("{} used from an allotment", names.usd(*amount))
        }
        Event::MarketCreated { market } => format!("market {} opened", object(market)),
        Event::OrderSubmitted { market, .. } => format!("order entered market {}", object(market)),
        Event::BatchSealed { batch, orders, .. } => {
            format!("batch {batch} sealed with {orders} orders")
        }
        Event::BatchRevealed {
            batch, revealed, ..
        } => {
            format!("{revealed} sealed orders of batch {batch} revealed")
        }
        Event::BatchCleared {
            batch,
            price,
            volume,
            ..
        } => match price {
            Some(price) => format!("batch {batch} cleared: {volume} units at one price {price}"),
            None => format!("batch {batch} cleared with no trade"),
        },
        Event::BatchRefunded { batch, .. } => format!("batch {batch} refunded"),
        Event::ValidatorRegistered {
            validator,
            operator,
            stake,
        } => format!(
            "validator {} registered by {} with {} at stake",
            validator.short(),
            names.account(operator),
            names.usd(*stake)
        ),
        Event::ValidatorLeaving { validator } => {
            format!("validator {} is leaving the active set", validator.short())
        }
        Event::StakeWithdrawn { validator, amount } => format!(
            "{} of stake withdrawn from validator {}",
            names.usd(*amount),
            validator.short()
        ),
        Event::RewardsClaimed { validator, amount } => format!(
            "{} of rewards claimed for validator {}",
            names.usd(*amount),
            validator.short()
        ),
        Event::ValidatorSlashed {
            validator,
            amount,
            reporter,
            reward,
        } => format!(
            "validator {} slashed: {} forfeited, {} of it to {} for the report",
            validator.short(),
            names.usd(*amount),
            names.usd(*reward),
            names.account(reporter)
        ),
        Event::EpochChanged {
            epoch,
            active,
            rewards,
        } => format!(
            "epoch {epoch} began with {active} active validators; {} paid in rewards",
            names.usd(*rewards)
        ),
        Event::ForcedPosted {
            intent,
            shard,
            deadline,
        } => format!(
            "forced intent {} is now owed by shard {shard}, due by Pulse {deadline}",
            intent.short()
        ),
        Event::ForcedDismissed { intent, reason } => {
            format!("forced intent {} dismissed: {reason}", intent.short())
        }
        Event::ForcedResolved { intent } => {
            format!("forced intent {} settled", intent.short())
        }
        Event::ForcedOverdue { intent, shard } => format!(
            "shard {shard} missed its deadline for forced intent {}",
            intent.short()
        ),
        Event::ShardRotated { shard, rotation } => {
            format!("shard {shard}'s committee was replaced (rotation {rotation} this epoch)")
        }
        Event::StreamOpened {
            stream,
            payer,
            payee,
            deposit,
        } => format!(
            "{} opened stream {} to {} with {} in escrow",
            names.account(payer),
            stream.short(),
            names.account(payee),
            names.usd(*deposit)
        ),
        Event::StreamSettled {
            stream,
            position,
            amount,
            ..
        } => format!(
            "stream {} settled to step {position}: {} paid",
            stream.short(),
            names.usd(*amount)
        ),
        Event::StreamToppedUp {
            stream,
            deposit,
            until,
        } => format!(
            "stream {} topped up by {}, open until Pulse {until}",
            stream.short(),
            names.usd(*deposit)
        ),
        Event::StreamClosed { stream, refund } => format!(
            "stream {} closed: {} returned to its payer",
            stream.short(),
            names.usd(*refund)
        ),
        Event::MemoryOpened { log, agent, .. } => match agent {
            Some(agent) => format!(
                "memory log {} opened for agent {}",
                object(log),
                object(agent)
            ),
            None => format!("memory log {} opened", object(log)),
        },
        Event::MemoryCommitted {
            log,
            size,
            commits,
            sealed,
            ..
        } => format!(
            "memory log {} now holds {size} {} (head {commits}){}",
            object(log),
            if *size == 1 { "atom" } else { "atoms" },
            if *sealed { ", sealed" } else { "" }
        ),
        Event::TaskPosted {
            task,
            worker,
            amount,
            deadline,
            ..
        } => format!(
            "task {} posted with {} in escrow, {}, until Pulse {deadline}",
            object(task),
            names.usd(*amount),
            match worker {
                Some(worker) => format!("for {}", names.account(worker)),
                None => "open to anyone".to_string(),
            }
        ),
        Event::TaskSettled {
            task,
            worker,
            result,
            amount,
            ..
        } => format!(
            "task {} settled: {} paid to {} for result {}, its backing reserved",
            object(task),
            names.usd(*amount),
            names.account(worker),
            result.short()
        ),
        Event::TaskCancelled { task, refund, to } => format!(
            "task {} cancelled: {} refunded to {}",
            object(task),
            names.usd(*refund),
            names.account(to)
        ),
        Event::Delegated {
            delegation,
            validator,
            owner,
            amount,
            ..
        } => format!(
            "{} delegated {} to validator {} as delegation {}, earning from the epoch's end",
            names.account(owner),
            names.usd(*amount),
            validator.short(),
            object(delegation)
        ),
        Event::Undelegated {
            delegation,
            rewards,
            withdrawable_at,
        } => format!(
            "delegation {} stopped earning ({} of rewards paid) and is withdrawable from Pulse {}",
            object(delegation),
            names.usd(*rewards),
            withdrawable_at
        ),
        Event::DelegationWithdrawn { delegation, amount } => format!(
            "{} withdrawn from delegation {}",
            names.usd(*amount),
            object(delegation)
        ),
        Event::DelegatorRewardsClaimed { delegation, amount } => format!(
            "{} of rewards collected from delegation {}",
            names.usd(*amount),
            object(delegation)
        ),
        Event::CommissionSet {
            validator,
            permille,
        } => format!(
            "validator {}'s commission becomes {} at the epoch's end",
            validator.short(),
            percent(*permille)
        ),
        Event::PoolSlashed { validator, amount } => format!(
            "validator {}'s pool lost {} of its delegated stake",
            validator.short(),
            names.usd(*amount)
        ),
        Event::DealPosted {
            deal,
            provider,
            size,
            price,
            rounds,
            collateral,
            ..
        } => format!(
            "storage deal {} posted: {size} leaves, {rounds} rounds at {} each, {} of collateral, {}",
            object(deal),
            names.usd(*price),
            names.usd(*collateral),
            match provider {
                Some(provider) => format!("for {}", names.account(provider)),
                None => "open to any provider".to_string(),
            }
        ),
        Event::DealAccepted {
            deal,
            provider,
            custody,
            start,
        } => format!(
            "{} accepted storage deal {} (custody record {}); rounds start at Pulse {start}",
            names.account(provider),
            object(deal),
            object(custody)
        ),
        Event::StorageProven { deal, round, .. } => format!(
            "round {round} of storage deal {} proven",
            object(deal)
        ),
        Event::DealSettled {
            deal,
            rounds,
            amount,
        } => format!(
            "{} paid for {rounds} proven round(s) of storage deal {}",
            names.usd(*amount),
            object(deal)
        ),
        Event::DealClosed {
            deal,
            refund,
            paid,
            returned,
            forfeited,
            missed,
        } => format!(
            "storage deal {} closed: {} refunded, {} paid, {} of collateral returned, {} forfeited, {missed} round(s) missed",
            object(deal),
            names.usd(*refund),
            names.usd(*paid),
            names.usd(*returned),
            names.usd(*forfeited)
        ),
    }
}

/// Thousandths as a percentage: 100 → "10%", 125 → "12.5%".
pub fn percent(permille: u16) -> String {
    match permille % 10 {
        0 => format!("{}%", permille / 10),
        tenth => format!("{}.{tenth}%", permille / 10),
    }
}

/// What happened to a transaction, as one sentence.
pub fn receipt(names: &Names, effects: &TxEffects) -> String {
    let events: Vec<String> = effects.events.iter().map(|e| event(names, e)).collect();
    match &effects.status {
        ExecStatus::Success => format!(
            "Done: {}; fee {}.",
            if events.is_empty() {
                "accepted".to_string()
            } else {
                events.join("; ")
            },
            names.usd(effects.fee)
        ),
        ExecStatus::Failed(reason) => format!(
            "Failed ({reason}); nothing happened except the fee of {}.",
            names.usd(effects.fee)
        ),
        ExecStatus::Rejected(reason) => {
            format!("Rejected ({reason}); nothing changed and no fee was charged.")
        }
    }
}

#[cfg(test)]
mod tests {
    use sion_core::ids::Pulse;

    use super::*;

    #[test]
    fn transfers_and_receipts_read_as_sentences() {
        let names = Names::devnet(["alice", "bob"]);
        let bob = Wallet::named("bob").id;
        let tx = Transaction {
            sender: Wallet::named("alice").id,
            genesis_policy: None,
            inputs: Vec::new(),
            fee: Amount(10_000),
            kind: TxKind::Transfer {
                asset: AssetId::usd(),
                outputs: vec![(bob, Amount::units(12).saturating_add(Amount(500_000)))],
            },
            expires_at: Pulse(120),
            nonce: 0,
        };
        assert_eq!(
            sentence(&names, &tx),
            "Pay 12.5 USD to bob. Fee 0.01 USD; valid until Pulse 120."
        );
        let effects = TxEffects {
            digest: tx.digest(),
            sender: tx.sender,
            class: tx.class(),
            status: ExecStatus::Success,
            fee: Amount(10_000),
            created: Default::default(),
            mutated: Default::default(),
            consumed: Default::default(),
            events: vec![Event::Transferred {
                asset: AssetId::usd(),
                outputs: vec![(bob, Amount::units(12))],
            }],
            deferred: Vec::new(),
        };
        assert_eq!(
            receipt(&names, &effects),
            "Done: paid 12 USD to bob; fee 0.01 USD."
        );
        let unknown = Names::default();
        assert!(sentence(&unknown, &tx).contains("to account "));
    }
}
