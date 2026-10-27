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

use fold_core::ids::{AccountId, AssetId, AtomId, ClaimId, ObjectId};
use fold_core::Amount;
use fold_exec::charter::Caveats;
use fold_exec::market::Submission;
use fold_exec::{Event, ExecStatus, Transaction, TxEffects, TxKind, Wallet};

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

    fn usd(&self, amount: Amount) -> String {
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

fn object(id: &ObjectId) -> String {
    id.short()
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
        format!(
            "{} per {} Pulses",
            names.usd(k.max_per_window),
            if k.window == u64::MAX {
                "lifetime of the charter".to_string()
            } else {
                k.window.to_string()
            }
        ),
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
                "{} {} units at {} {} each in market {}",
                match terms.side {
                    fold_clearing::Side::Buy => "Bid for",
                    fold_clearing::Side::Sell => "Offer",
                },
                terms.quantity,
                match terms.side {
                    fold_clearing::Side::Buy => "at most",
                    fold_clearing::Side::Sell => "at least",
                },
                terms.limit_price,
                object(market)
            ),
            Submission::Sealed { side, escrow, .. } => format!(
                "Submit a sealed {} order to market {}, escrowing {escrow}",
                match side {
                    fold_clearing::Side::Buy => "buy",
                    fold_clearing::Side::Sell => "sell",
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
                fold_exec::validators::Equivocation::Headers { .. } => "consensus headers",
                fold_exec::validators::Equivocation::Approvals { .. } => "lane approvals",
            }
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
    use fold_core::ids::Pulse;

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
