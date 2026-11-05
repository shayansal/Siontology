//! The trust layer other agents plug into.
//!
//! An agent built on another framework already reaches a profile here: it
//! talks to it over A2A ([`crate::a2a`]), calls its tools over MCP
//! ([`crate::mcp`]), and pays for what it fetches with x402
//! ([`crate::x402`]). What this adds is the three facts such an agent needs
//! about *anyone* on the chain, served to it without an account or a key,
//! because each is a public fact of the ledger's:
//!
//! - **A record** ([`record_of`]): what an agent has delivered, what it
//!   earned, what it gave back, and what of its work was later proven
//!   false, read from the ledger's own events ([`crate::reputation`]).
//! - **A receipt** ([`receipt_of`]): a transaction's outcome with the proof
//!   a light client checks it by, so an outside agent can show elsewhere
//!   that it was paid, or that it paid.
//! - **A listing** ([`listing_of`]): an agent offered for rent, with
//!   whether its enclave's attestation checks out
//!   ([`crate::rentals`]).
//!
//! These are reads of public state, so they need no token: anyone may ask,
//! and nothing is taken on trust from the asker. What the answers are worth
//! is in their proofs and in `docs/implementation-status.md`'s trust
//! atoms, not in this server's word: an answer from a node is that node's
//! until the asker checks the proof it carries.

use serde_json::{json, Value};
use sion_core::ids::{AccountId, Pulse, TxDigest};

use crate::error::CliError;
use crate::ledger::{Ledger, LedgerExt};
use crate::reputation::{self, Record};

/// How far back a record is read, in Pulses, when the asker says nothing:
/// a window, because the whole chain's events are not read for one answer.
pub const WINDOW: u64 = 100_000;

/// The most events a record is read from.
pub const MAX_EVENTS: usize = 20_000;

/// An agent's record, as an outside agent is told it.
pub fn record_of(
    ledger: &dyn Ledger,
    account: AccountId,
    from: Option<Pulse>,
) -> Result<Value, CliError> {
    let now = ledger.pulse()?;
    let from = from.unwrap_or(Pulse(now.0.saturating_sub(WINDOW)));
    let records = reputation::records(ledger, from, MAX_EVENTS)?;
    let record = records.get(&account).cloned().unwrap_or_default();
    Ok(json!({
        "agent": account.to_string(),
        "from_pulse": from.0,
        "to_pulse": now.0,
        "delivered": record.delivered,
        "earned_usd": record.earned.to_string(),
        "released": record.released,
        "failed": record.failed,
        "trust": record.trust(),
        "read_from": "the ledger's events; see the trust atom agent:agent-record",
    }))
}

/// A record as the number [`Record::trust`] gives it, for a caller that
/// wants only that.
pub fn trust_of(ledger: &dyn Ledger, account: AccountId) -> Result<f64, CliError> {
    let now = ledger.pulse()?;
    let from = Pulse(now.0.saturating_sub(WINDOW));
    let records = reputation::records(ledger, from, MAX_EVENTS)?;
    Ok(records.get(&account).map(Record::trust).unwrap_or_default())
}

/// A transaction's receipt and whether it checks out as a light client
/// would check it.
pub fn receipt_of(ledger: &dyn Ledger, digest: TxDigest) -> Result<Value, CliError> {
    let state = ledger.tx(digest)?;
    let view = match state {
        crate::ledger::TxState::Executed(view) => view,
        other => {
            return Ok(json!({
                "transaction": digest.to_string(),
                "state": format!("{other:?}").split('(').next().unwrap_or("unknown").to_lowercase(),
                "receipt": Value::Null,
            }))
        }
    };
    Ok(json!({
        "transaction": digest.to_string(),
        "state": "executed",
        "pulse": view.pulse.0,
        "outcome": match &view.effects.status {
            sion_exec::ExecStatus::Success => "success",
            sion_exec::ExecStatus::Failed(_) => "failed",
            sion_exec::ExecStatus::Rejected(_) => "rejected",
        },
        "why": match &view.effects.status {
            sion_exec::ExecStatus::Success => Value::Null,
            sion_exec::ExecStatus::Failed(why) | sion_exec::ExecStatus::Rejected(why) => {
                json!(why)
            }
        },
        "events": view.effects.events.len(),
        "verify_with": format!("sion receipt {digest} --chain <the chain identity you pin>"),
    }))
}

/// A rental listing, with whether its enclave's attestation checks out
/// against the measurements the asker trusts (none: everything but the
/// code is checked, and what it runs is its own word).
pub fn listing_of(
    ledger: &dyn Ledger,
    owner: AccountId,
    name: &str,
    trusted: &[[u8; 48]],
) -> Result<Value, CliError> {
    let (listing, claim) = crate::rentals::latest(ledger, None, None, owner, name)?;
    let measurements = if trusted.is_empty() {
        listing
            .attestation
            .measurement()
            .map(|m| vec![m])
            .unwrap_or_default()
    } else {
        trusted.to_vec()
    };
    let expected = sion_enclave::Expectation {
        measurements,
        simulated_platforms: Vec::new(),
        now: ledger.pulse()?.0,
    };
    let checked = sion_enclave::verify(&listing.attestation, &expected);
    Ok(json!({
        "owner": owner.to_string(),
        "name": listing.name,
        "agent": listing.agent.to_string(),
        "listing_claim": claim.to_string(),
        "description": listing.description,
        "per_1k_input_usd": listing.per_1k_input.to_string(),
        "per_1k_output_usd": listing.per_1k_output.to_string(),
        "unit_usd": listing.unit.to_string(),
        "min_deposit_usd": listing.min_deposit.to_string(),
        "enclave": match &checked {
            Ok(verified) => json!({
                "checks_out": true,
                "platform": verified.platform.describe(),
                "measurement": hex::encode(verified.measurement),
                "measurement_trusted_by_asker": !trusted.is_empty(),
            }),
            Err(why) => json!({"checks_out": false, "why": why.to_string()}),
        },
    }))
}

/// What the trust layer answers, by path: `GET` only, no token, all public
/// reads. `None` if the path is not one of these.
pub fn answer(ledger: &dyn Ledger, method: &str, path: &str) -> Option<Result<Value, CliError>> {
    if method != "GET" {
        return None;
    }
    let rest = path.strip_prefix("/v1/trust/")?;
    let (kind, rest) = rest.split_once('/')?;
    match kind {
        "agents" => {
            let account = match account_of(rest) {
                Ok(account) => account,
                Err(e) => return Some(Err(e)),
            };
            Some(record_of(ledger, account, None))
        }
        "receipts" => {
            let digest = match hash_of(rest, "the transaction") {
                Ok(hash) => TxDigest::from_hash(hash),
                Err(e) => return Some(Err(e)),
            };
            Some(receipt_of(ledger, digest))
        }
        "listings" => {
            let (owner, name) = rest.split_once('/')?;
            let owner = match account_of(owner) {
                Ok(owner) => owner,
                Err(e) => return Some(Err(e)),
            };
            Some(listing_of(ledger, owner, name, &[]))
        }
        _ => None,
    }
}

/// What the trust layer serves, for an outside agent to discover.
pub fn describe() -> Value {
    json!({
        "serves": [
            {"path": "/v1/trust/agents/{account}", "gives": "what an agent has delivered, released and had proven false, and a number to trust it by"},
            {"path": "/v1/trust/receipts/{digest}", "gives": "a transaction's outcome, and how to verify its receipt as a light client"},
            {"path": "/v1/trust/listings/{owner}/{name}", "gives": "an agent offered for rent, and whether its enclave's attestation checks out"},
        ],
        "reads": "public ledger facts; no token, and nothing is taken on trust from the asker",
        "pay_with": ["x402"],
        "talk_with": ["a2a", "mcp"],
        "trust_atoms": ["agent:agent-record", "agent:rented-enclave", "agent:metered-tokens"],
    })
}

fn hash_of(text: &str, what: &str) -> Result<sion_core::hash::Hash32, CliError> {
    let bytes = hex::decode(text).map_err(|e| CliError::Usage(format!("{what}: {e}")))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| CliError::Usage(format!("{what} is 32 bytes (64 hex digits)")))?;
    Ok(sion_core::hash::Hash32(bytes))
}

fn account_of(text: &str) -> Result<AccountId, CliError> {
    hash_of(text, "the account").map(AccountId::from_hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_its_own_public_reads_are_answered() {
        // Not a path it serves.
        assert!(answer_is_none("GET", "/v1/profiles/lead/a2a"));
        assert!(answer_is_none("GET", "/v1/trust/"));
        assert!(answer_is_none("GET", "/v1/trust/nothing/x"));
        // Reads only: nothing is changed through it.
        assert!(answer_is_none("POST", "/v1/trust/agents/aa"));
        assert!(answer_is_none("DELETE", "/v1/trust/agents/aa"));
    }

    /// Whether the path is one the trust layer does not serve, without
    /// needing a ledger (the path is read before anything else).
    fn answer_is_none(method: &str, path: &str) -> bool {
        if method != "GET" {
            return true;
        }
        match path
            .strip_prefix("/v1/trust/")
            .and_then(|r| r.split_once('/'))
        {
            None => true,
            Some((kind, rest)) => {
                !matches!(kind, "agents" | "receipts" | "listings") || rest.is_empty()
            }
        }
    }

    #[test]
    fn what_it_serves_is_said_for_an_outside_agent_to_read() {
        let described = describe();
        let paths: Vec<&str> = described["serves"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["path"].as_str().unwrap())
            .collect();
        assert!(paths.iter().any(|p| p.contains("/agents/")));
        assert!(paths.iter().any(|p| p.contains("/receipts/")));
        assert!(paths.iter().any(|p| p.contains("/listings/")));
        // It names the trust atoms its answers rest on, not a promise.
        assert!(described["trust_atoms"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a == "agent:agent-record"));
    }

    #[test]
    fn an_account_or_a_digest_is_read_only_as_64_hex_digits() {
        assert!(account_of(&"a".repeat(64)).is_ok());
        assert!(account_of("too-short").is_err());
        assert!(account_of(&"a".repeat(63)).is_err());
        assert!(hash_of(&"f".repeat(64), "x").is_ok());
        assert!(hash_of("zz", "x").is_err());
    }
}
