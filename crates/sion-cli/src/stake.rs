//! Pooled stake (spec v1.11): delegating to a validator without handing the
//! stake over, collecting rewards, leaving, and the operator's commission.
//!
//! A delegation is an object its delegator owns: it holds shares of the
//! validator's pool, which counts towards the validator's weight and earns a
//! share of its rewards from the end of the epoch it was made in. These
//! commands read the validator set and the delegation book from the node and
//! show what each delegation is worth and is owed.

use clap::Subcommand;
use serde_json::{json, Value};
use sion_core::hash::Hash32;
use sion_core::ids::{AccountId, ObjectId, ValidatorId};
use sion_exec::delegation::{
    resolve, Delegation, DelegationBook, DelegationState, Pool, Standing,
    COMMISSION_CEILING_PERMILLE,
};
use sion_exec::execute::Event;
use sion_exec::object::{well_known, ObjectData};
use sion_exec::validators::{ValidatorSet, ValidatorStatus};
use sion_exec::TxKind;

use crate::describe::{percent, Names};
use crate::error::CliError;
use crate::ledger::{Ledger, LedgerExt};
use crate::session::Session;

/// `sion stake …`
#[derive(Debug, Subcommand)]
pub enum StakeCommand {
    /// Every validator: its own stake, its pool, its commission and its
    /// weight in the active set.
    Pools,
    /// Delegate stake to a validator's pool. You keep the delegation: the
    /// stake counts towards the validator's weight and earns from the end of
    /// this epoch, and you can leave whenever you like.
    Delegate {
        /// Your wallet.
        from: String,
        /// The validator (its id, or a unique prefix of it).
        validator: String,
        /// Stake (decimal, in the stake asset), at least 1.
        amount: String,
    },
    /// An account's delegations: shares, the stake behind them, their state
    /// and the rewards they are owed.
    Show {
        /// Wallet name or account ID.
        account: String,
    },
    /// Collect a delegation's rewards.
    Claim {
        /// Your wallet.
        from: String,
        /// The delegation (its id, or a unique prefix of it).
        delegation: String,
    },
    /// Stop a delegation earning: collect its rewards and start unbonding.
    /// The stake stays slashable until you withdraw it.
    Undelegate {
        /// Your wallet.
        from: String,
        /// The delegation (its id, or a unique prefix of it).
        delegation: String,
    },
    /// Withdraw an unbonded delegation's stake.
    Withdraw {
        /// Your wallet.
        from: String,
        /// The delegation (its id, or a unique prefix of it).
        delegation: String,
    },
    /// Set your validator's commission on its pool's rewards, in percent
    /// (at most 25; once anyone has delegated, a raise of at most 2 points
    /// at a time). It takes effect at the epoch's end.
    Commission {
        /// The operator's wallet.
        from: String,
        /// The validator (its id, or a unique prefix of it).
        validator: String,
        /// The commission in percent, with at most one decimal (for
        /// example 10 or 12.5).
        percent: String,
    },
}

/// Runs a stake command.
pub fn run(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    command: StakeCommand,
) -> Result<(), CliError> {
    match command {
        StakeCommand::Pools => {
            for line in pool_lines(ledger, &operator_names(session))? {
                println!("{line}");
            }
            Ok(())
        }
        StakeCommand::Delegate {
            from,
            validator,
            amount,
        } => {
            let set = validator_set(ledger)?;
            let validator = find_validator(&set, &validator)?;
            let amount = crate::agents::parse_amount(&amount)?;
            let view = session.send(
                &from,
                TxKind::DelegateStake { validator, amount },
                Vec::new(),
            )?;
            crate::agents::print_receipt(session, &view);
            for event in &view.effects.events {
                if let Event::Delegated { delegation, .. } = event {
                    println!("delegation {delegation}");
                }
            }
            Ok(())
        }
        StakeCommand::Show { account } => {
            let id = session.id(&account);
            println!(
                "{}",
                serde_json::to_string_pretty(&delegations_of(ledger, id)?).unwrap_or_default()
            );
            Ok(())
        }
        StakeCommand::Claim { from, delegation } => on_delegation(
            session,
            ledger,
            &from,
            &delegation,
            TxKind::ClaimDelegatorRewards,
        ),
        StakeCommand::Undelegate { from, delegation } => {
            on_delegation(session, ledger, &from, &delegation, TxKind::Undelegate)
        }
        StakeCommand::Withdraw { from, delegation } => on_delegation(
            session,
            ledger,
            &from,
            &delegation,
            TxKind::WithdrawDelegation,
        ),
        StakeCommand::Commission {
            from,
            validator,
            percent,
        } => {
            let set = validator_set(ledger)?;
            let validator = find_validator(&set, &validator)?;
            let permille = parse_percent(&percent)?;
            let view = session.send(
                &from,
                TxKind::SetCommission {
                    validator,
                    permille,
                },
                Vec::new(),
            )?;
            crate::agents::print_receipt(session, &view);
            Ok(())
        }
    }
}

/// Sends `kind` with the delegation as its second input.
fn on_delegation(
    session: &mut Session<'_>,
    ledger: &dyn Ledger,
    from: &str,
    delegation: &str,
    kind: TxKind,
) -> Result<(), CliError> {
    let owner = session.id(from);
    let id = find_delegation(ledger, owner, delegation)?;
    let reference = session.reference(&id)?;
    let view = session.send(from, kind, vec![reference])?;
    crate::agents::print_receipt(session, &view);
    Ok(())
}

/// The validator set.
pub fn validator_set(ledger: &dyn Ledger) -> Result<ValidatorSet, CliError> {
    match ledger.object(well_known::validators())?.map(|o| o.data) {
        Some(ObjectData::Validators(set)) => Ok(*set),
        _ => Err(CliError::Usage("the node returned no validator set".into())),
    }
}

/// The delegation book, if any delegation or commission created it.
pub fn delegation_book(ledger: &dyn Ledger) -> Result<Option<DelegationBook>, CliError> {
    match ledger.object(well_known::delegations())?.map(|o| o.data) {
        Some(ObjectData::Delegations(book)) => Ok(Some(*book)),
        None => Ok(None),
        Some(_) => Err(CliError::Usage(
            "the delegation book object holds something else".into(),
        )),
    }
}

/// A validator by full id or unique prefix.
pub fn find_validator(set: &ValidatorSet, text: &str) -> Result<ValidatorId, CliError> {
    let text = text.trim().to_lowercase();
    if let Ok(hash) = Hash32::from_hex(&text) {
        return Ok(ValidatorId::from_hash(hash));
    }
    let matches: Vec<ValidatorId> = set
        .validators
        .keys()
        .filter(|id| id.0.to_hex().starts_with(&text))
        .copied()
        .collect();
    match matches.as_slice() {
        [one] => Ok(*one),
        [] => Err(CliError::Usage(format!(
            "no validator starts with {text:?}"
        ))),
        _ => Err(CliError::Usage(format!(
            "{} validators start with {text:?}; give more digits",
            matches.len()
        ))),
    }
}

/// One of `owner`'s delegations by full id or unique prefix.
pub fn find_delegation(
    ledger: &dyn Ledger,
    owner: AccountId,
    text: &str,
) -> Result<ObjectId, CliError> {
    let text = text.trim().to_lowercase();
    if let Ok(hash) = Hash32::from_hex(&text) {
        return Ok(ObjectId::from_hash(hash));
    }
    let view = ledger.account(owner)?;
    let matches: Vec<ObjectId> = view
        .delegations
        .iter()
        .map(|o| o.id)
        .filter(|id| id.0.to_hex().starts_with(&text))
        .collect();
    match matches.as_slice() {
        [one] => Ok(*one),
        [] => Err(CliError::Usage(format!(
            "the account has no delegation starting with {text:?}"
        ))),
        _ => Err(CliError::Usage(format!(
            "{} delegations start with {text:?}; give more digits",
            matches.len()
        ))),
    }
}

/// `"12.5"` → 125 thousandths.
pub fn parse_percent(text: &str) -> Result<u16, CliError> {
    let text = text.trim().trim_end_matches('%');
    let (whole, tenth) = text.split_once('.').unwrap_or((text, "0"));
    let bad = || {
        CliError::Usage(format!(
            "{text:?} is not a percentage with at most one decimal"
        ))
    };
    if tenth.len() != 1 {
        return Err(bad());
    }
    let whole: u16 = whole.parse().map_err(|_| bad())?;
    let tenth: u16 = tenth.parse().map_err(|_| bad())?;
    let permille = whole
        .checked_mul(10)
        .and_then(|p| p.checked_add(tenth))
        .ok_or_else(bad)?;
    if permille > COMMISSION_CEILING_PERMILLE {
        return Err(CliError::Usage(format!(
            "the commission may be at most {}",
            percent(COMMISSION_CEILING_PERMILLE)
        )));
    }
    Ok(permille)
}

/// What a delegation stands for now: its standing in the current epoch,
/// the stake behind its shares and what it is owed.
fn standing(pool: &Pool, delegation: &Delegation, epoch: u64) -> (String, u128, Option<u128>) {
    let stake = pool.value_of(delegation.shares).map(|a| a.0).unwrap_or(0);
    let (mut pool, mut d) = (pool.clone(), delegation.clone());
    match (delegation.state, resolve(&mut d, &mut pool, epoch)) {
        (DelegationState::Unbonding { at }, _) => (
            format!("unbonding, withdrawable from Pulse {at}"),
            stake,
            None,
        ),
        (_, Ok(Standing::Pending)) => (
            "pending: earns from the end of this epoch".into(),
            stake,
            Some(0),
        ),
        (_, Ok(Standing::Earning { entry })) => (
            "earning".into(),
            stake,
            pool.owed(delegation.shares, entry).map(|a| a.0),
        ),
        (_, Err(reason)) => (reason.to_string(), stake, None),
    }
}

/// An account's delegations, as JSON.
pub fn delegations_of(ledger: &dyn Ledger, account: AccountId) -> Result<Value, CliError> {
    let view = ledger.account(account)?;
    let set = validator_set(ledger)?;
    let book = delegation_book(ledger)?.unwrap_or_default();
    let mut list = Vec::new();
    for object in &view.delegations {
        let ObjectData::Delegation(d) = &object.data else {
            continue;
        };
        let pool = book.pools.get(&d.validator).cloned().unwrap_or_default();
        let (state, stake, owed) = standing(&pool, d, set.epoch);
        list.push(json!({
            "delegation": object.id,
            "validator": d.validator,
            "shares": d.shares.to_string(),
            "stake": sion_core::Amount(stake).to_string(),
            "state": state,
            "rewards_owed": owed.map(|o| sion_core::Amount(o).to_string()),
            "since": d.since,
        }));
    }
    Ok(json!({ "account": account, "epoch": set.epoch, "delegations": list }))
}

/// The session's address book plus the devnet's validator operators
/// (`operator-<index>`).
pub fn operator_names(session: &Session<'_>) -> Names {
    let mut names = session.names();
    for index in 0..64 {
        let name = format!("operator-{index}");
        names.add_account(sion_exec::wallet::Wallet::named(&name).id, &name);
    }
    names
}

/// One line per validator: operator, status, own stake, pool, commission,
/// weight.
pub fn pool_lines(ledger: &dyn Ledger, names: &Names) -> Result<Vec<String>, CliError> {
    let set = validator_set(ledger)?;
    let book = delegation_book(ledger)?.unwrap_or_default();
    let mut lines = vec![format!(
        "epoch {}, ending after Pulse {}; stake asset {}",
        set.epoch,
        set.epoch_start
            .0
            .saturating_add(set.params.epoch_length.max(1) - 1),
        if set.params.stake_asset == sion_core::ids::AssetId::usd() {
            "USD (test stake)".to_string()
        } else {
            set.params.stake_asset.to_string()
        }
    )];
    for (id, record) in &set.validators {
        let pool = book.pools.get(id).cloned().unwrap_or_default();
        let bonded = pool.bonded().unwrap_or_default();
        let earning = pool.earning_value().unwrap_or_default();
        let weight = set
            .active
            .iter()
            .find(|(v, _)| v == id)
            .map(|(_, w)| w.to_string())
            .unwrap_or_else(|| "-".into());
        let status = match record.status {
            ValidatorStatus::Joining => "joining".to_string(),
            ValidatorStatus::Active => "active".to_string(),
            ValidatorStatus::Leaving => "leaving".to_string(),
            ValidatorStatus::Exited { withdrawable_at } => {
                format!("exited (withdrawable from Pulse {withdrawable_at})")
            }
            ValidatorStatus::Withdrawn => "withdrawn".to_string(),
            ValidatorStatus::Slashed { at } => format!("slashed at Pulse {at}"),
        };
        let commission = match pool.next_commission {
            Some(next) if next != pool.commission_permille => format!(
                "{} ({} from the epoch's end)",
                percent(pool.commission_permille),
                percent(next)
            ),
            _ => percent(pool.commission_permille),
        };
        lines.push(format!(
            "validator {}  operator {}  {status}  own {}  delegated {} (earning {})  commission {commission}  weight {weight}",
            id.short(),
            names.account(&record.operator),
            record.stake,
            bonded,
            earning,
        ));
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentages_parse_to_thousandths() {
        assert_eq!(parse_percent("10").unwrap(), 100);
        assert_eq!(parse_percent("12.5").unwrap(), 125);
        assert_eq!(parse_percent("25%").unwrap(), 250);
        assert_eq!(parse_percent("0").unwrap(), 0);
        assert!(parse_percent("25.1").is_err());
        assert!(parse_percent("1.25").is_err());
        assert!(parse_percent("ten").is_err());
    }
}
