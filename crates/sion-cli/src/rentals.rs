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
use sion_crypto::kem::{seal, KemKeypair, KemPublicKey, SealedEnvelope};
use sion_crypto::keys::{self, Keypair, PublicKey, Signature};
use sion_crypto::scheme::Scheme;
use sion_enclave::Attestation;
use sion_exec::claim::{claim_id, formulas, ClaimBody, Payload};
use sion_exec::object::ObjectData;
use sion_exec::stream::Jump;
use sion_exec::TxKind;

use crate::error::CliError;
use crate::ledger::{Ledger, LedgerExt};
use crate::session::Session;
use crate::stream::Payment;

/// The domain a request is sealed in.
const REQUEST_DOMAIN: &str = "sion/rental/request/v1";
/// The domain a reply is sealed in.
const REPLY_DOMAIN: &str = "sion/rental/reply/v1";
/// The domain the enclave signs its count of usage in.
const USAGE_DOMAIN: &str = "sion/rental/usage/v1";
/// The domain the enclave signs its receipts in.
pub const RECEIPTS_DOMAIN: &str = "sion/rental/receipts/v1";

/// What a renter sends the enclave, sealed to its ML-KEM key: a message,
/// the key the reply is sealed back to, and the stream payment that is to
/// cover the running total of what the session has used.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// What the renter asks.
    pub message: String,
    /// The renter's key, which the reply is sealed to.
    pub reply_key: KemPublicKey,
    /// The payment covering the total used so far, after this request.
    pub payment: Payment,
    /// A jump proving the payment's position, if the stream has
    /// checkpoints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jump: Option<Jump>,
}

/// The enclave's signed count of what a session used, which a renter pays
/// for: the enclave vouches for it, not the owner, so the owner cannot
/// inflate it. Signed with the key the attestation binds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedUsage {
    /// The session it is for.
    pub session: String,
    /// The tokens used, in total, up to and including this request.
    pub usage: Usage,
    /// The USD that comes to, at the listing's prices.
    pub price: Amount,
    /// The stream position that price is, in whole steps.
    pub position: u64,
    /// The enclave's signature over the above.
    pub signature: Signature,
}

/// What the enclave signs for [`SignedUsage`].
fn usage_message(session: &str, usage: &Usage, price: Amount, position: u64) -> Vec<u8> {
    borsh::to_vec(&(
        session.as_bytes(),
        usage.input,
        usage.output,
        price.micros().to_le_bytes(),
        position,
    ))
    .unwrap_or_default()
}

impl SignedUsage {
    /// Checks the enclave's signature under the key `signing_key` the
    /// attestation bound.
    pub fn verify(&self, signing_key: &[u8]) -> Result<(), CliError> {
        let key = PublicKey {
            scheme: self.signature.scheme,
            bytes: signing_key.to_vec(),
        };
        keys::verify(
            &key,
            USAGE_DOMAIN,
            &usage_message(&self.session, &self.usage, self.price, self.position),
            &self.signature,
        )
        .map_err(|e| CliError::Verification(format!("the enclave's count of usage: {e}")))
    }
}

/// The enclave's signed word on which models answered a rental's
/// requests: the receipts its runtime kept of them
/// ([`sion_agent::receipt`]), as a chain over the whole rental. Signed
/// with the key the attestation binds, so it is the measured code that
/// says which address it asked and which model it was told answered, not
/// the owner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedReceipts {
    /// The rental it is for: its stream.
    pub of: String,
    /// How many answers models have given in it.
    pub count: u64,
    /// The head of their chain.
    pub head: String,
    /// Every answer, by the model asked, the address and the model that
    /// answered.
    pub tallies: Vec<sion_agent::receipt::Tally>,
    /// The receipts of this request's answers: the end of the chain.
    pub new: Vec<sion_agent::receipt::Receipt>,
    /// Whether the runtime trusted certificate roots beyond the public
    /// ones (`SSL_CERT_FILE`): an address's name then does not show who
    /// answered there.
    #[serde(default)]
    pub extra_roots: bool,
    /// The enclave's signature over the above, less the receipts
    /// themselves, which the head stands for.
    pub signature: Signature,
}

impl SignedReceipts {
    /// Checks that the receipts given are the end of the chain the enclave
    /// signed, counted by its tallies, and the enclave's signature under
    /// the key `signing_key` the attestation bound.
    pub fn verify(&self, signing_key: &[u8]) -> Result<(), CliError> {
        sion_agent::receipt::check(self.count, &self.head, &self.tallies, &self.new)
            .map_err(|why| CliError::Verification(format!("the enclave's receipts: {why}")))?;
        let key = PublicKey {
            scheme: self.signature.scheme,
            bytes: signing_key.to_vec(),
        };
        let statement = crate::receipts::statement(
            &self.of,
            self.count,
            &self.head,
            &self.tallies,
            self.extra_roots,
        );
        keys::verify(&key, RECEIPTS_DOMAIN, &statement.0, &self.signature)
            .map_err(|e| CliError::Verification(format!("the enclave's receipts: {e}")))
    }
}

/// The enclave's reply, sealed to the renter's reply key: what the agent
/// said, the signed count of usage, and the signed receipts of which
/// models answered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    /// What the agent said.
    pub text: String,
    /// What the session has used, signed by the enclave.
    pub usage: SignedUsage,
    /// Which models answered, signed by the enclave. None from a server
    /// that keeps no receipts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipts: Option<SignedReceipts>,
}

/// What running the rented agent on one request gave.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Turn {
    /// What the agent said.
    pub text: String,
    /// The tokens it used.
    pub usage: Usage,
    /// The receipts its runtime kept of the models' answers.
    pub receipts: Vec<sion_agent::receipt::Receipt>,
}

/// The keys an enclave session holds, made inside it and never leaving:
/// the ML-KEM key requests are sealed to, and the key it signs usage with.
/// Their public parts are what an attestation binds ([`binding`]).
pub struct EnclaveKeys {
    /// Seals and opens requests.
    pub sealing: KemKeypair,
    /// Signs counts of usage.
    pub signing: Keypair,
}

impl std::fmt::Debug for EnclaveKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EnclaveKeys({:?})", self.sealing.public())
    }
}

impl EnclaveKeys {
    /// Fresh keys, made here (inside the enclave, in use).
    pub fn generate() -> EnclaveKeys {
        let seed = keys::random_seed();
        EnclaveKeys {
            sealing: KemKeypair::from_seed(&seed),
            signing: Keypair::generate(Scheme::MlDsa44),
        }
    }

    /// The binding an attestation makes over these keys, for `agent`, the
    /// listing's `terms`, until `expires_at`.
    pub fn binding(
        &self,
        agent: &ObjectId,
        terms: [u8; 32],
        expires_at: u64,
    ) -> sion_enclave::Binding {
        sion_enclave::Binding {
            kem_public_key: self.sealing.public().bytes.clone(),
            signing_public_key: self.signing.public().bytes.clone(),
            agent: agent.as_hash().0,
            terms,
            expires_at,
        }
    }

    /// Opens a request a renter sealed to this enclave.
    pub fn open(&self, sealed: &SealedEnvelope) -> Result<Request, CliError> {
        let bytes = self
            .sealing
            .open(REQUEST_DOMAIN, &[], sealed)
            .map_err(|e| CliError::Verification(format!("the sealed request: {e}")))?;
        serde_json::from_slice(&bytes)
            .map_err(|e| CliError::Protocol(format!("the request's contents: {e}")))
    }

    /// Signs what `session` has used: `usage` tokens, `price` USD, at stream
    /// `position`.
    pub fn sign_usage(
        &self,
        session: &str,
        usage: &Usage,
        price: Amount,
        position: u64,
    ) -> Result<SignedUsage, CliError> {
        let signature = self
            .signing
            .sign(
                USAGE_DOMAIN,
                &usage_message(session, usage, price, position),
            )
            .map_err(|e| CliError::Usage(format!("the count could not be signed: {e}")))?;
        Ok(SignedUsage {
            session: session.to_string(),
            usage: *usage,
            price,
            position,
            signature,
        })
    }

    /// Signs the receipts of the rental `of`: the chain `all` as it now
    /// stands, with the receipts from place `from` on given whole.
    pub fn sign_receipts(
        &self,
        of: &str,
        all: &sion_agent::receipt::Receipts,
        from: u64,
    ) -> Result<SignedReceipts, CliError> {
        let extra_roots = std::env::var_os("SSL_CERT_FILE").is_some();
        let statement =
            crate::receipts::statement(of, all.count, &all.head(), &all.tallies, extra_roots);
        let signature = self
            .signing
            .sign(RECEIPTS_DOMAIN, &statement.0)
            .map_err(|e| CliError::Usage(format!("the receipts could not be signed: {e}")))?;
        Ok(SignedReceipts {
            of: of.to_string(),
            count: all.count,
            head: all.head(),
            tallies: all.tallies.clone(),
            new: all.since(from).map(<[_]>::to_vec).unwrap_or_default(),
            extra_roots,
            signature,
        })
    }

    /// Seals `reply` to the renter's `reply_key`.
    pub fn seal_reply(
        &self,
        reply: &Reply,
        reply_key: &KemPublicKey,
    ) -> Result<SealedEnvelope, CliError> {
        let bytes = serde_json::to_vec(reply)
            .map_err(|e| CliError::Usage(format!("the reply could not be written: {e}")))?;
        seal(reply_key, REPLY_DOMAIN, &[], &bytes, &mut rand_core::OsRng)
            .map_err(|e| CliError::Usage(format!("the reply could not be sealed: {e}")))
    }
}

/// What a stream has run up so far on the server's side.
#[derive(Clone, Debug, Default)]
struct Running {
    usage: Usage,
    /// The receipts of the models' answers over the whole rental.
    receipts: sion_agent::receipt::Receipts,
    /// The total USD signed for so far.
    owed: Amount,
    /// The stream position `owed` comes to.
    position: u64,
    /// The USD the desk has been given a payment for.
    covered: Amount,
}

/// The enclave's side of renting: it opens each sealed request, runs the
/// rented agent, meters the tokens, takes the stream payment that covers
/// what was signed for before, signs the new total and seals the reply.
///
/// Payment is one request behind: a request carries the payment for the
/// total signed through the request before it (the first owes nothing), so
/// the owner is ever owed at most one request's tokens, and a renter that
/// stops paying is served no more. A request with an empty message runs
/// nothing and only settles what is outstanding, to close the tab.
pub struct RentalServer {
    keys: EnclaveKeys,
    listing: Listing,
    desk: crate::stream::StreamDesk,
    running: std::sync::Mutex<std::collections::BTreeMap<ObjectId, Running>>,
}

impl std::fmt::Debug for RentalServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RentalServer({})", self.listing.name)
    }
}

impl RentalServer {
    /// A server for `listing`, paid into `desk` (the owner's), its requests
    /// opened and signed with `keys` (whose public parts the listing's
    /// attestation binds).
    pub fn new(
        keys: EnclaveKeys,
        listing: Listing,
        desk: crate::stream::StreamDesk,
    ) -> RentalServer {
        RentalServer {
            keys,
            listing,
            desk,
            running: std::sync::Mutex::new(std::collections::BTreeMap::new()),
        }
    }

    /// The listing it serves.
    pub fn listing(&self) -> &Listing {
        &self.listing
    }

    /// The enclave's ML-KEM key, which requests are sealed to.
    pub fn sealing_key(&self) -> &KemPublicKey {
        self.keys.sealing.public()
    }

    /// Serves one sealed request at Pulse `now`, running the agent with
    /// `run` (given the message, it returns what the agent said and the
    /// tokens that turn used). Returns the sealed reply, or why the request
    /// was refused (an unsealed error; a refused payment names the position
    /// the renter should resynchronize to).
    pub fn serve(
        &self,
        ledger: &dyn Ledger,
        now: Pulse,
        sealed: &SealedEnvelope,
        run: impl FnOnce(&str) -> Result<(String, Usage), CliError>,
    ) -> Result<SealedEnvelope, CliError> {
        self.serve_turn(ledger, now, sealed, |message| {
            let (text, usage) = run(message)?;
            Ok(Turn {
                text,
                usage,
                receipts: Vec::new(),
            })
        })
    }

    /// [`RentalServer::serve`], with a `run` that also returns the
    /// receipts its runtime kept of the models' answers: they join the
    /// rental's chain, and the reply carries the enclave's signature over
    /// it.
    pub fn serve_turn(
        &self,
        ledger: &dyn Ledger,
        now: Pulse,
        sealed: &SealedEnvelope,
        run: impl FnOnce(&str) -> Result<Turn, CliError>,
    ) -> Result<SealedEnvelope, CliError> {
        let request = self.keys.open(sealed)?;
        let stream = request.payment.stream;
        let mut running = self.running.lock().expect("running lock");
        let state = running.entry(stream).or_default();
        // Take the payment for what was signed before this request.
        if state.owed.micros() > state.covered.micros() {
            let delta = Amount(state.owed.micros() - state.covered.micros());
            self.desk
                .accept(ledger, now, &request.payment, delta, request.jump.as_ref())
                .map_err(|refused| {
                    CliError::Usage(match refused.position {
                        Some(p) => format!(
                            "payment refused ({}); resynchronize to position {p}",
                            refused.reason
                        ),
                        None => format!("payment refused: {}", refused.reason),
                    })
                })?;
            state.covered = state.owed;
        }
        // An empty message only settles; otherwise run and meter.
        if !request.message.trim().is_empty() {
            let turn = run(&request.message)?;
            state.usage.add(&turn.usage);
            state.owed = price_of(&self.listing, &state.usage);
            state.position = steps_of(state.owed, self.listing.unit);
            let before = state.receipts.count;
            for receipt in turn.receipts {
                state.receipts.push(receipt);
            }
            let receipts = if state.receipts.is_empty() {
                None
            } else {
                Some(
                    self.keys
                        .sign_receipts(&stream.to_string(), &state.receipts, before)?,
                )
            };
            let reply = Reply {
                text: turn.text,
                usage: self.keys.sign_usage(
                    &stream.to_string(),
                    &state.usage,
                    state.owed,
                    state.position,
                )?,
                receipts,
            };
            return self.keys.seal_reply(&reply, &request.reply_key);
        }
        // A settling request: acknowledge the total, nothing run.
        let reply = Reply {
            text: String::new(),
            usage: self.keys.sign_usage(
                &stream.to_string(),
                &state.usage,
                state.owed,
                state.position,
            )?,
            receipts: None,
        };
        self.keys.seal_reply(&reply, &request.reply_key)
    }

    /// Settles the payments taken so far without waiting (the desk sends a
    /// settlement and goes on); call it from time to time.
    pub fn settle(&self, ledger: &dyn Ledger, now: Pulse) -> Result<(), CliError> {
        let config = ledger.chain_config()?;
        self.desk.refresh(ledger)?;
        self.desk.settle(ledger, &config, now, true)?;
        Ok(())
    }

    /// Settles every payment taken and waits for the settlements to be
    /// certified, up to `timeout`: what the owner collects when the renting
    /// is done.
    pub fn close_out(
        &self,
        ledger: &dyn Ledger,
        timeout: std::time::Duration,
    ) -> Result<(), CliError> {
        let config = ledger.chain_config()?;
        self.desk.close_out(ledger, &config, timeout)
    }
}

/// A renter's key for sealing a request and opening the reply.
#[derive(Debug)]
pub struct RenterKey {
    keypair: KemKeypair,
}

impl Default for RenterKey {
    fn default() -> RenterKey {
        RenterKey::generate()
    }
}

impl RenterKey {
    /// A fresh key.
    pub fn generate() -> RenterKey {
        RenterKey {
            keypair: KemKeypair::from_seed(&keys::random_seed()),
        }
    }

    /// The public part, which a reply is sealed to.
    pub fn public(&self) -> &KemPublicKey {
        self.keypair.public()
    }

    /// Seals `request` to the enclave `listing` attests.
    pub fn seal_request(
        &self,
        listing: &Listing,
        message: &str,
        payment: Payment,
        jump: Option<Jump>,
    ) -> Result<SealedEnvelope, CliError> {
        let enclave = KemPublicKey {
            bytes: listing.attestation.binding.kem_public_key.clone(),
        };
        let request = Request {
            message: message.to_string(),
            reply_key: self.public().clone(),
            payment,
            jump,
        };
        let bytes = serde_json::to_vec(&request)
            .map_err(|e| CliError::Usage(format!("the request could not be written: {e}")))?;
        seal(&enclave, REQUEST_DOMAIN, &[], &bytes, &mut rand_core::OsRng)
            .map_err(|e| CliError::Usage(format!("the request could not be sealed: {e}")))
    }

    /// Opens the enclave's sealed reply.
    pub fn open_reply(&self, sealed: &SealedEnvelope) -> Result<Reply, CliError> {
        let bytes = self
            .keypair
            .open(REPLY_DOMAIN, &[], sealed)
            .map_err(|e| CliError::Verification(format!("the sealed reply: {e}")))?;
        serde_json::from_slice(&bytes)
            .map_err(|e| CliError::Protocol(format!("the reply's contents: {e}")))
    }
}

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

    fn payment(position: u64) -> Payment {
        Payment {
            stream: ObjectId::from_hash(sion_core::hash::hash_domain("test/stream", b"s")),
            position,
            value: sion_core::hash::hash_domain("test/value", &position.to_le_bytes()),
        }
    }

    #[test]
    fn a_request_is_sealed_to_the_enclave_and_opened_only_by_it() {
        let enclave = EnclaveKeys::generate();
        let listing = {
            let mut l = listing(3_000, 15_000, 1_000);
            l.attestation.binding.kem_public_key = enclave.sealing.public().bytes.clone();
            l
        };
        let renter = RenterKey::generate();
        let sealed = renter
            .seal_request(&listing, "summarize this", payment(14), None)
            .unwrap();
        // The enclave opens it; another enclave's keys cannot.
        let opened = enclave.open(&sealed).unwrap();
        assert_eq!(opened.message, "summarize this");
        assert_eq!(opened.payment.position, 14);
        assert_eq!(opened.reply_key.bytes, renter.public().bytes);
        assert!(EnclaveKeys::generate().open(&sealed).is_err());
    }

    #[test]
    fn the_enclave_signs_usage_and_the_renter_checks_it_against_the_binding() {
        let enclave = EnclaveKeys::generate();
        let used = usage(2_000, 500);
        let signed = enclave
            .sign_usage("session-1", &used, Amount(14_000), 14)
            .unwrap();
        // Checks under the key the attestation would bind.
        let key = enclave.signing.public().bytes.clone();
        assert!(signed.verify(&key).is_ok());
        // Not under another key, and not if the count is changed.
        assert!(signed.verify(&[0u8; 1312]).is_err());
        let mut tampered = signed.clone();
        tampered.price = Amount(1);
        assert!(tampered.verify(&key).is_err());
    }

    #[test]
    fn a_reply_is_sealed_to_the_renter_and_carries_the_signed_count() {
        let enclave = EnclaveKeys::generate();
        let renter = RenterKey::generate();
        let signed = enclave
            .sign_usage("session-1", &usage(10, 5), Amount(1_000), 1)
            .unwrap();
        let reply = Reply {
            text: "done".into(),
            usage: signed,
            receipts: None,
        };
        let sealed = enclave.seal_reply(&reply, renter.public()).unwrap();
        let opened = renter.open_reply(&sealed).unwrap();
        assert_eq!(opened.text, "done");
        assert!(opened.usage.verify(&enclave.signing.public().bytes).is_ok());
        // Another renter cannot open it.
        assert!(RenterKey::generate().open_reply(&sealed).is_err());
    }

    #[test]
    fn the_binding_carries_the_enclaves_public_keys() {
        let enclave = EnclaveKeys::generate();
        let agent = ObjectId::from_hash(sion_core::hash::hash_domain("test/agent", b"a"));
        let binding = enclave.binding(&agent, [3u8; 32], 9_000);
        assert_eq!(binding.kem_public_key, enclave.sealing.public().bytes);
        assert_eq!(binding.signing_public_key, enclave.signing.public().bytes);
        assert_eq!(binding.agent, agent.as_hash().0);
        assert_eq!(binding.expires_at, 9_000);
    }
}
