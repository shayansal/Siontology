//! Renting an agent out to other people, by the token.
//!
//! An owner lists an agent ([`Listing`]): a price for each thousand input
//! and output tokens, in USD, and the attestation ([`sion_enclave`]) that
//! it runs in an enclave the owner cannot read. The listing is a claim,
//! named in the owner's index under `rental:<name>` so a renter finds the
//! latest ([`publish`], [`latest`]). A renter checks the attestation,
//! opens a payment stream to the owner, seals each request to the enclave's
//! key, and advances the stream by what the tokens cost ([`price_of`]); the
//! enclave meters the tokens and signs the count, and the renter pays only
//! for as many as it signed for. The rented session runs with none of the
//! owner's memory, files or spending (the server's part, built on this).
//!
//! The model the agent runs on is the owner's account, priced into the
//! listing; a listing may instead say the renter brings their own model
//! key, which the server seals to the enclave, and that is not served yet.

use serde::{Deserialize, Serialize};
use sion_agent::model::Usage;
use sion_assurance::Formula;
use sion_core::ids::{AccountId, ClaimId, ObjectId, Pulse};
use sion_core::Amount;
use sion_enclave::Attestation;
use sion_exec::claim::{claim_id, formulas, ClaimBody, Payload};
use sion_exec::object::ObjectData;
use sion_exec::TxKind;

use crate::error::CliError;
use crate::ledger::Ledger;
use crate::session::Session;

/// The schema of a listing claim.
pub const LISTING_SCHEMA: &str = "agent.listing/v1";

/// Whose model account a rented session runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelAccount {
    /// The owner's, priced into the listing; the renter brings no key.
    Owner,
    /// The renter's own key, sealed to the enclave (not served yet).
    Renter,
}

/// What an owner offers: an agent, by the token, in an enclave.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    /// A short name, unique to the owner; the index key is `rental:<name>`.
    pub name: String,
    /// The agent rented.
    pub agent: ObjectId,
    /// The account a renter pays (the owner's).
    pub payee: AccountId,
    /// USD per thousand input tokens.
    pub per_1k_input: Amount,
    /// USD per thousand output tokens.
    pub per_1k_output: Amount,
    /// Whose model account the session runs on.
    pub model_account: ModelAccount,
    /// One step of the payment stream, in USD: the price is rounded up to a
    /// whole number of these.
    pub unit: Amount,
    /// The least a renter must escrow to start.
    pub min_deposit: Amount,
    /// What the agent is for, in a line.
    pub description: String,
    /// The attestation that it runs in an enclave, binding the enclave's
    /// keys and this listing's terms.
    pub attestation: Attestation,
}

/// The index key an owner names its latest listing of `name` under.
pub fn index_key(name: &str) -> String {
    format!("rental:{name}")
}

/// The digest of a listing's terms, which its attestation's binding commits
/// to, so that the enclave's keys are bound to these very terms: a listing
/// whose price or payee was changed no longer matches its attestation.
pub fn terms_digest(
    agent: &ObjectId,
    payee: &AccountId,
    per_1k_input: Amount,
    per_1k_output: Amount,
    model_account: ModelAccount,
) -> [u8; 32] {
    let encoded = borsh::to_vec(&(
        agent,
        payee,
        per_1k_input.micros(),
        per_1k_output.micros(),
        format!("{model_account:?}"),
    ))
    .unwrap_or_default();
    sion_core::hash::hash_domain("sion/rental/terms/v1", &encoded).0
}

impl Listing {
    /// Whether the attestation's binding commits to this listing's terms.
    pub fn terms_match(&self) -> bool {
        self.attestation.binding.terms
            == terms_digest(
                &self.agent,
                &self.payee,
                self.per_1k_input,
                self.per_1k_output,
                self.model_account,
            )
    }

    /// Its body as a claim.
    fn body(&self, valid_for: u64) -> Result<ClaimBody, CliError> {
        let payload = serde_json::to_vec(self)
            .map_err(|e| CliError::Usage(format!("the listing could not be written: {e}")))?;
        Ok(ClaimBody {
            schema: LISTING_SCHEMA.into(),
            subject: format!(
                "{} rents out {} by the token",
                self.payee.short(),
                self.name
            ),
            payload: Payload::Public(payload),
            // A listing rests only on the owner's signature: it is an
            // offer, not a statement that can be proven false.
            formula: Formula::and(vec![formulas::dual_signature()]),
            adjudicator: None,
            detection_permille: 0,
            valid_for,
            challenge_period: 0,
        })
    }
}

/// The USD a session that used `usage` tokens owes under `listing`, rounded
/// up to a whole number of the listing's stream steps. Done in u128
/// throughout, saturating rather than overflowing.
pub fn price_of(listing: &Listing, usage: &Usage) -> Amount {
    let part = |tokens: u64, per_1k: Amount| -> u128 {
        u128::from(tokens)
            .saturating_mul(per_1k.micros())
            .div_ceil(1000)
    };
    let raw = part(usage.input, listing.per_1k_input)
        .saturating_add(part(usage.output, listing.per_1k_output));
    let unit = listing.unit.micros().max(1);
    // Up to a whole number of steps.
    Amount(raw.div_ceil(unit).saturating_mul(unit))
}

/// How many stream steps `price` is, given a listing's `unit`.
pub fn steps_of(price: Amount, unit: Amount) -> u64 {
    let unit = unit.micros().max(1);
    u64::try_from(price.micros().div_ceil(unit)).unwrap_or(u64::MAX)
}

/// Publishes `listing` as a claim of `wallet`'s and names it the latest
/// under `rental:<name>` in its index, valid for `valid_for` Pulses. The
/// listing's terms must match its attestation's binding.
pub fn publish(
    session: &mut Session<'_>,
    wallet: &str,
    listing: &Listing,
    valid_for: u64,
) -> Result<ClaimId, CliError> {
    if !listing.terms_match() {
        return Err(CliError::Usage(
            "the listing's terms do not match the attestation's binding: attest the enclave for \
             these very terms"
                .into(),
        ));
    }
    let body = listing.body(valid_for)?;
    let id = claim_id(&session.id(wallet), &body);
    session.send(wallet, TxKind::IssueClaim { claim: body }, Vec::new())?;
    session.send(
        wallet,
        TxKind::IndexClaim {
            key: index_key(&listing.name),
            claim: id,
        },
        Vec::new(),
    )?;
    Ok(id)
}

/// Reads the listing a claim holds.
pub fn read(ledger: &dyn Ledger, id: &str) -> Result<Listing, CliError> {
    let stored = crate::tasks::stored_claim(ledger, id)?;
    if stored.body.schema != LISTING_SCHEMA {
        return Err(CliError::Usage(format!(
            "claim {} is a {}, not an agent listing",
            stored.id.short(),
            stored.body.schema
        )));
    }
    let Payload::Public(bytes) = &stored.body.payload else {
        return Err(CliError::Usage("a listing's payload is public".into()));
    };
    let listing: Listing = serde_json::from_slice(bytes)
        .map_err(|e| CliError::Protocol(format!("listing payload: {e}")))?;
    if listing.payee != stored.issuer {
        return Err(CliError::Verification(
            "the listing names another payee than the owner who published it".into(),
        ));
    }
    Ok(listing)
}

/// The owner's latest listing of `name`, its index entry proven against a
/// certified header as a light client would (against `pinned`, if given,
/// and fresh past `min_pulse`), then the listing read and checked to be
/// that owner's listing of that name, with terms matching its attestation.
pub fn latest(
    ledger: &dyn Ledger,
    pinned: Option<sion_core::hash::Hash32>,
    min_pulse: Option<Pulse>,
    owner: AccountId,
    name: &str,
) -> Result<(Listing, ClaimId), CliError> {
    let id = sion_exec::index::index_id(&owner, &index_key(name));
    let proven = crate::verify::verify_object_now(ledger, pinned, id, min_pulse)?;
    let entry = match proven.objects.first().and_then(|(_, o)| o.as_ref()) {
        Some(object) => match &object.data {
            ObjectData::IndexEntry(entry) => entry.clone(),
            _ => {
                return Err(CliError::Verification(format!(
                    "{id} is not an index entry"
                )))
            }
        },
        None => {
            return Err(CliError::Usage(format!(
                "the owner has listed no rental named {name} (proven absent: {})",
                proven.line()
            )))
        }
    };
    let listing = read(ledger, &entry.claim.to_string())?;
    if listing.payee != owner || listing.name != name {
        return Err(CliError::Verification(
            "the index names a claim that is not this owner's listing of this name".into(),
        ));
    }
    if !listing.terms_match() {
        return Err(CliError::Verification(
            "the listing's terms do not match its attestation's binding".into(),
        ));
    }
    Ok((listing, entry.claim))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sion_enclave::simulated::SimulatedPlatform;
    use sion_enclave::Binding;

    fn listing(per_1k_input: u64, per_1k_output: u64, unit: u64) -> Listing {
        let agent = ObjectId::from_hash(sion_core::hash::hash_domain("test/agent", b"a"));
        let payee = AccountId::from_hash(sion_core::hash::hash_domain("test/payee", b"p"));
        let model_account = ModelAccount::Owner;
        let terms = terms_digest(
            &agent,
            &payee,
            Amount(per_1k_input.into()),
            Amount(per_1k_output.into()),
            model_account,
        );
        let platform = SimulatedPlatform::generate().unwrap();
        let binding = Binding {
            kem_public_key: vec![1u8; 1184],
            signing_public_key: vec![2u8; 1312],
            agent: agent.as_hash().0,
            terms,
            expires_at: 10_000,
        };
        let attestation = platform.attest(binding, [0xAB; 48]).unwrap();
        Listing {
            name: "helper".into(),
            agent,
            payee,
            per_1k_input: Amount(per_1k_input.into()),
            per_1k_output: Amount(per_1k_output.into()),
            model_account,
            unit: Amount(unit.into()),
            min_deposit: Amount::units(1),
            description: "a helper".into(),
            attestation,
        }
    }

    fn usage(input: u64, output: u64) -> Usage {
        Usage {
            input,
            output,
            ..Usage::default()
        }
    }

    #[test]
    fn a_price_is_per_token_rounded_up_to_whole_steps() {
        // 3000 USD micros per 1k input, 15000 per 1k output, 1000-micro steps.
        let listing = listing(3_000, 15_000, 1_000);
        // 2000 input → 6000 micros; 500 output → 7500 micros; 13500 → up to
        // 14000 (14 steps of 1000).
        assert_eq!(price_of(&listing, &usage(2_000, 500)), Amount(14_000));
        // Nothing used, nothing owed.
        assert_eq!(price_of(&listing, &usage(0, 0)), Amount::ZERO);
        // One input token: 3 micros, up to one 1000-micro step.
        assert_eq!(price_of(&listing, &usage(1, 0)), Amount(1_000));
        assert_eq!(steps_of(Amount(14_000), Amount(1_000)), 14);
    }

    #[test]
    fn a_price_does_not_overflow() {
        let listing = listing(u64::MAX, u64::MAX, 1);
        let price = price_of(&listing, &usage(u64::MAX, u64::MAX));
        assert!(price.micros() > 0);
    }

    #[test]
    fn terms_bind_the_listing_to_its_attestation() {
        let mut l = listing(3_000, 15_000, 1_000);
        assert!(l.terms_match());
        // Change the price after attesting: it no longer matches.
        l.per_1k_output = Amount(16_000);
        assert!(!l.terms_match());
    }

    #[test]
    fn a_listing_is_written_and_read_back() {
        let l = listing(3_000, 15_000, 1_000);
        let bytes = serde_json::to_vec(&l).unwrap();
        let back: Listing = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(l, back);
        assert_eq!(index_key("helper"), "rental:helper");
    }
}
