//! `sion agent rent`: listing an agent out by the token, serving renters,
//! and renting someone else's.
//!
//! The owner's side attests the enclave it runs in, publishes the listing
//! ([`crate::rentals`]) and serves sealed requests over HTTP, running the
//! profile's agent with none of the owner's memory, files or reach. The
//! renter's side reads a listing, checks its attestation, opens a payment
//! stream and sends sealed requests, paying by the token.

use std::io::Read as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use clap::Subcommand;
use sion_agent::model::Usage;
use sion_core::ids::{AccountId, AssetId, ObjectId, Pulse};
use sion_core::Amount;
use sion_crypto::kem::SealedEnvelope;
use sion_enclave::simulated::SimulatedPlatform;
use sion_exec::execute::Event;
use sion_exec::TxKind;

use crate::error::CliError;
use crate::ledger::{Ledger, LedgerExt};
use crate::rentals::{self, EnclaveKeys, Listing, ModelAccount, RentalServer, RenterKey};
use crate::runtime::SharedLedger;
use crate::session::Session;
use crate::stream::{PayerBook, PayerRecord, StreamDesk};

/// The path sealed requests are posted to.
pub const PATH: &str = "/v1/rent";

/// How long a listing is published for, in Pulses.
const LISTING_PULSES: u64 = 100_000;

/// The most bytes a sealed request may be.
const MAX_REQUEST: usize = 256 * 1024;

/// `sion agent rent …`
#[derive(Debug, Subcommand)]
pub enum RentCommand {
    /// Rent a profile's agent out by the token: attest the enclave it runs
    /// in, publish the listing, and serve sealed requests. Renters' sessions
    /// get none of this profile's memory, files or reach, and the model runs
    /// on your own account, priced into what you charge.
    Serve {
        /// The profile whose agent is rented out.
        profile: String,
        /// The listing's name, yours to choose (`rental:<name>` in your
        /// index).
        #[arg(long)]
        name: String,
        /// USD per thousand input tokens (as a decimal, 0.003).
        #[arg(long, value_name = "USD")]
        per_1k_input: String,
        /// USD per thousand output tokens.
        #[arg(long, value_name = "USD")]
        per_1k_output: String,
        /// One step of a renter's payment stream, in USD: prices are
        /// rounded up to a whole number of these.
        #[arg(long, default_value = "0.001", value_name = "USD")]
        unit: String,
        /// The least a renter must escrow to start, in USD.
        #[arg(long, default_value = "1", value_name = "USD")]
        min_deposit: String,
        /// What the agent is for, in a line, which renters search.
        #[arg(long, default_value = "")]
        description: String,
        /// Where to listen (127.0.0.1:0 picks a free port).
        #[arg(long, default_value = "127.0.0.1:8795", value_name = "ADDRESS")]
        listen_at: String,
        /// Serve this many requests, then stop (for trying it out).
        #[arg(long, value_name = "N")]
        requests: Option<u64>,
    },
    /// Rent an owner's agent: check its listing's attestation, escrow a
    /// deposit in a payment stream, ask it, and pay for the tokens it used.
    Use {
        /// The owner's account (64 hex digits, or a devnet wallet's name).
        owner: String,
        /// The listing's name.
        name: String,
        /// What to ask it.
        message: String,
        /// Where its enclave serves (http://host:port).
        #[arg(long, value_name = "URL")]
        at: String,
        /// Your wallet, which pays.
        #[arg(long, default_value = "alice")]
        wallet: String,
        /// What to escrow, in USD.
        #[arg(long, default_value = "1", value_name = "USD")]
        deposit: String,
        /// How long the stream stays open, in Pulses.
        #[arg(long, default_value_t = 10_000)]
        pulses: u64,
        /// The measurement of the enclave you trust (48 bytes in hex).
        #[arg(long, value_name = "HEX")]
        measurement: Option<String>,
        /// Take a simulated enclave with this platform key (hex).
        #[arg(long, value_name = "HEX")]
        simulated_platform: Option<String>,
    },
    /// Show an owner's listing and check its attestation: what it costs,
    /// what it runs, and whether its enclave checks out.
    Show {
        /// The owner's account (64 hex digits, or a devnet wallet's name).
        owner: String,
        /// The listing's name.
        name: String,
        /// The measurement of the enclave you trust (48 bytes in hex);
        /// without it the attestation's own measurement is shown, unchecked.
        #[arg(long, value_name = "HEX")]
        measurement: Option<String>,
        /// Take a simulated enclave with this platform key (hex): for
        /// devnets, where there is no SEV-SNP hardware.
        #[arg(long, value_name = "HEX")]
        simulated_platform: Option<String>,
    },
}

/// Parses a decimal USD amount.
fn usd(text: &str) -> Result<Amount, CliError> {
    crate::agents::parse_amount(text)
}

/// Parses 64 hex digits.
fn parse_hash(text: &str, what: &str) -> Result<sion_core::hash::Hash32, CliError> {
    let bytes = hex::decode(text).map_err(|e| CliError::Usage(format!("{what}: {e}")))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| CliError::Usage(format!("{what} is 32 bytes (64 hex digits)")))?;
    Ok(sion_core::hash::Hash32(bytes))
}

/// Runs `sion agent rent …`.
pub fn run(ledger: SharedLedger, command: RentCommand) -> Result<(), CliError> {
    match command {
        RentCommand::Serve {
            profile,
            name,
            per_1k_input,
            per_1k_output,
            unit,
            min_deposit,
            description,
            listen_at,
            requests,
        } => serve(
            ledger,
            &profile,
            Terms {
                name,
                per_1k_input: usd(&per_1k_input)?,
                per_1k_output: usd(&per_1k_output)?,
                unit: usd(&unit)?,
                min_deposit: usd(&min_deposit)?,
                description,
            },
            &listen_at,
            requests,
        ),
        RentCommand::Use {
            owner,
            name,
            message,
            at,
            wallet,
            deposit,
            pulses,
            measurement,
            simulated_platform,
        } => rent(
            ledger,
            Rent {
                owner,
                name,
                message,
                at,
                wallet,
                deposit: usd(&deposit)?,
                pulses,
                measurement,
                simulated_platform,
            },
        ),
        RentCommand::Show {
            owner,
            name,
            measurement,
            simulated_platform,
        } => show(
            ledger.as_ref(),
            &owner,
            &name,
            measurement.as_deref(),
            simulated_platform.as_deref(),
        ),
    }
}

/// What an owner charges and offers.
#[derive(Clone, Debug)]
pub struct Terms {
    /// The listing's name.
    pub name: String,
    /// USD per thousand input tokens.
    pub per_1k_input: Amount,
    /// USD per thousand output tokens.
    pub per_1k_output: Amount,
    /// One stream step, in USD.
    pub unit: Amount,
    /// The least a renter escrows.
    pub min_deposit: Amount,
    /// What the agent is for.
    pub description: String,
}

/// Attests the enclave this runs in, binding `keys` to `terms`: a real
/// SEV-SNP report where there is one, else a simulated platform kept beside
/// the agent's own files, which [`crate::rentals`] marks as such.
pub fn attest(
    keys: &EnclaveKeys,
    agent: &ObjectId,
    terms: [u8; 32],
    expires_at: u64,
) -> Result<(sion_enclave::Attestation, Option<Vec<u8>>), CliError> {
    let binding = keys.binding(agent, terms, expires_at);
    let fetch = |url: &str| -> Result<Vec<u8>, String> {
        let client =
            sion_http::Client::new(sion_http::Options::default()).map_err(|e| e.to_string())?;
        client
            .get(url, 64 * 1024)
            .map(|fetched| fetched.body)
            .map_err(|e| e.to_string())
    };
    match sion_enclave::collect(binding.clone(), &fetch) {
        Ok(attestation) => Ok((attestation, None)),
        Err(why) => {
            // No confidential VM here: a simulated platform, said so.
            eprintln!("(not in an enclave this attests: {why})");
            eprintln!("(using a SIMULATED platform: nothing stands behind its reports, and a renter takes one only with --simulated-platform)");
            let platform =
                SimulatedPlatform::generate().map_err(|e| CliError::Usage(e.to_string()))?;
            let measurement = measurement_of_simulated();
            let attestation = platform
                .attest(binding, measurement)
                .map_err(|e| CliError::Usage(e.to_string()))?;
            Ok((attestation, Some(platform.public_key())))
        }
    }
}

/// The measurement a simulated platform reports: this build of `sion`, so
/// that a renter can at least pin what it was told it is running.
fn measurement_of_simulated() -> [u8; 48] {
    let mut measurement = [0u8; 48];
    let digest = sion_core::hash::hash_domain(
        "sion/enclave/simulated-measurement",
        env!("CARGO_PKG_VERSION").as_bytes(),
    );
    measurement[..32].copy_from_slice(&digest.0);
    measurement
}

/// Serves renters of `profile`'s agent.
fn serve(
    ledger: SharedLedger,
    profile: &str,
    terms: Terms,
    listen_at: &str,
    requests: Option<u64>,
) -> Result<(), CliError> {
    let profile = crate::profiles::get(profile)?;
    if terms.unit.micros() == 0 {
        return Err(CliError::Usage("a stream step is more than nothing".into()));
    }
    let mut session = Session::open(ledger.as_ref())?;
    let owner = session.id(&profile.wallet);
    let keys = EnclaveKeys::generate();
    let digest = rentals::terms_digest(
        &profile.agent,
        &owner,
        terms.per_1k_input,
        terms.per_1k_output,
        ModelAccount::Owner,
    );
    let until = ledger.pulse()?.0.saturating_add(LISTING_PULSES);
    let (attestation, simulated) = attest(&keys, &profile.agent, digest, until)?;
    let listing = Listing {
        name: terms.name.clone(),
        agent: profile.agent,
        payee: owner,
        per_1k_input: terms.per_1k_input,
        per_1k_output: terms.per_1k_output,
        model_account: ModelAccount::Owner,
        unit: terms.unit,
        min_deposit: terms.min_deposit,
        description: terms.description.clone(),
        attestation,
    };
    let claim = rentals::publish(&mut session, &profile.wallet, &listing, LISTING_PULSES)?;
    println!(
        "listed {} as {}: {} USD per 1k input, {} USD per 1k output, steps of {} USD",
        profile.name, terms.name, terms.per_1k_input, terms.per_1k_output, terms.unit
    );
    println!("its listing is claim {claim}");
    if let Some(key) = &simulated {
        println!(
            "its enclave is SIMULATED; renters need --simulated-platform {}",
            hex::encode(key)
        );
    }
    println!(
        "renters find it with: sion agent rent show {} {}",
        owner, terms.name
    );

    let desk = StreamDesk::new(crate::session::wallet_for(&profile.wallet));
    let server = RentalServer::new(keys, listing, desk);
    let http = tiny_http::Server::http(listen_at)
        .map_err(|e| CliError::Usage(format!("cannot listen on {listen_at}: {e}")))?;
    let address = http
        .server_addr()
        .to_ip()
        .map(|a| a.to_string())
        .unwrap_or_else(|| listen_at.to_string());
    println!("serving sealed requests at http://{address}{PATH} (Ctrl-C ends it)");
    let stop = Arc::new(AtomicBool::new(false));
    let mut served = 0u64;
    for mut request in http.incoming_requests() {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        if request.url() != PATH || request.method().as_str() != "POST" {
            let _ =
                request.respond(tiny_http::Response::from_string("not here").with_status_code(404));
            continue;
        }
        let mut body = Vec::new();
        let read =
            std::io::Read::take(request.as_reader(), MAX_REQUEST as u64 + 1).read_to_end(&mut body);
        if read.is_err() || body.len() > MAX_REQUEST {
            let _ =
                request.respond(tiny_http::Response::from_string("too long").with_status_code(413));
            continue;
        }
        let answer = serve_body(ledger.clone(), &server, &profile, &body);
        let response = match answer {
            Ok(sealed) => tiny_http::Response::from_data(sealed).with_status_code(200),
            Err(why) => {
                eprintln!("(a request was refused: {why})");
                tiny_http::Response::from_string(why.to_string()).with_status_code(402)
            }
        };
        let _ = request.respond(response);
        served += 1;
        // Settle what has been taken, now and then.
        if let Ok(now) = ledger.pulse() {
            let _ = server.settle(ledger.as_ref(), now);
        }
        if requests.is_some_and(|most| served >= most) {
            break;
        }
    }
    println!("({served} request(s) served; settling what is owed)");
    server.close_out(ledger.as_ref(), std::time::Duration::from_secs(60))?;
    Ok(())
}

/// Serves one sealed request: the agent runs with none of the owner's
/// memory, files or reach, and its tokens are what the renter pays for.
fn serve_body(
    ledger: SharedLedger,
    server: &RentalServer,
    profile: &crate::profiles::Profile,
    body: &[u8],
) -> Result<Vec<u8>, CliError> {
    let sealed: SealedEnvelope = serde_json::from_slice(body)
        .map_err(|e| CliError::Protocol(format!("the sealed request: {e}")))?;
    let now = ledger.pulse()?;
    let running = ledger.clone();
    let answer = server.serve(ledger.as_ref(), now, &sealed, |message| {
        run_rented(running, profile, message)
    })?;
    serde_json::to_vec(&answer).map_err(|e| CliError::Usage(format!("the sealed reply: {e}")))
}

/// Runs the rented agent once: what it said, and the tokens it used.
fn run_rented(
    ledger: SharedLedger,
    profile: &crate::profiles::Profile,
    message: &str,
) -> Result<(String, Usage), CliError> {
    // A renter's words come from outside, and the session gets none of the
    // owner's memory, files, skills or reach.
    let hooks = crate::runtime::Hooks {
        outside: true,
        ..Default::default()
    };
    let settings = crate::channels::ChatSettings::default();
    let (saved, _) = profile
        .without_reach()
        .start_in(ledger, message, &[], hooks, &settings)?;
    let said = match &saved.ended {
        Some(sion_agent::Ended::Done { text }) => text.clone(),
        other => format!(
            "(the session ended without an answer: {})",
            other
                .as_ref()
                .map(crate::model_routes::ended_kind)
                .unwrap_or("running")
        ),
    };
    Ok((said, saved.state.usage))
}

/// Shows a listing and checks its attestation.
fn show(
    ledger: &dyn Ledger,
    owner: &str,
    name: &str,
    measurement: Option<&str>,
    simulated_platform: Option<&str>,
) -> Result<(), CliError> {
    let owner = account_of(ledger, owner)?;
    let (listing, claim) = rentals::latest(ledger, None, None, owner, name)?;
    println!("{} by {}", listing.name, owner);
    println!("  {}", listing.description);
    println!(
        "  {} USD per 1k input, {} USD per 1k output, steps of {} USD, at least {} USD escrowed",
        listing.per_1k_input, listing.per_1k_output, listing.unit, listing.min_deposit
    );
    println!("  its agent is {}", listing.agent);
    println!("  its listing is claim {claim}");
    let expected =
        sion_enclave::Expectation {
            // Without a measurement to trust, the attestation's own is used:
            // everything else is checked, and what it runs is only its word.
            measurements: match measurement {
                Some(hex) => vec![measurement_bytes(hex)?],
                None => listing
                    .attestation
                    .measurement()
                    .map(|m| vec![m])
                    .unwrap_or_default(),
            },
            simulated_platforms: match simulated_platform {
                Some(text) => vec![hex::decode(text)
                    .map_err(|e| CliError::Usage(format!("the platform key: {e}")))?],
                None => Vec::new(),
            },
            now: ledger.pulse()?.0,
        };
    match sion_enclave::verify(&listing.attestation, &expected) {
        Ok(verified) => {
            println!("  its enclave checks out: {}", verified.platform.describe());
            println!(
                "  running code of measurement {}",
                hex::encode(verified.measurement)
            );
        }
        Err(why) => {
            println!("  its enclave does NOT check out: {why}");
            if measurement.is_none() {
                println!("  (give --measurement <hex> to say which code you trust)");
            }
        }
    }
    Ok(())
}

/// What a renter asked for.
#[derive(Clone, Debug)]
struct Rent {
    owner: String,
    name: String,
    message: String,
    at: String,
    wallet: String,
    deposit: Amount,
    pulses: u64,
    measurement: Option<String>,
    simulated_platform: Option<String>,
}

/// Rents an agent: checks the listing, escrows, asks, and pays for the
/// tokens the enclave signed for.
fn rent(ledger: SharedLedger, rent: Rent) -> Result<(), CliError> {
    let owner = account_of(ledger.as_ref(), &rent.owner)?;
    let (listing, _) = rentals::latest(ledger.as_ref(), None, None, owner, &rent.name)?;
    // Check the enclave before anything is sent to it or paid.
    let expected =
        sion_enclave::Expectation {
            measurements: match &rent.measurement {
                Some(hex) => vec![measurement_bytes(hex)?],
                None => listing
                    .attestation
                    .measurement()
                    .map(|m| vec![m])
                    .unwrap_or_default(),
            },
            simulated_platforms: match &rent.simulated_platform {
                Some(text) => vec![hex::decode(text)
                    .map_err(|e| CliError::Usage(format!("the platform key: {e}")))?],
                None => Vec::new(),
            },
            now: ledger.pulse()?.0,
        };
    let verified = sion_enclave::verify(&listing.attestation, &expected)
        .map_err(|why| CliError::Verification(format!("its enclave does not check out: {why}")))?;
    println!("its enclave checks out: {}", verified.platform.describe());
    if rent.measurement.is_none() {
        println!(
            "(you did not say which code you trust: it says it runs {})",
            hex::encode(verified.measurement)
        );
    }

    let mut session = Session::open(ledger.as_ref())?;
    let stream = open_stream(
        &mut session,
        &rent.wallet,
        &listing,
        rent.deposit,
        rent.pulses,
    )?;
    println!(
        "escrowed {} USD in stream {} at {} USD a step",
        rent.deposit, stream, listing.unit
    );

    let key = RenterKey::generate();
    let url = format!("{}{PATH}", rent.at.trim_end_matches('/'));
    // The first request owes nothing yet: the stream's head stands in.
    let head = crate::stream::load_stream(ledger.as_ref(), &stream)?.head;
    let first = crate::stream::Payment {
        stream,
        position: 0,
        value: head,
    };
    let reply = ask(&key, &listing, &url, &rent.message, first, None)?;
    reply
        .usage
        .verify(&listing.attestation.binding.signing_public_key)?;
    println!(
        "
{}
",
        reply.text.trim()
    );
    println!(
        "it used {} input and {} output tokens: {} USD",
        reply.usage.usage.input, reply.usage.usage.output, reply.usage.price
    );

    // Pay what it signed for, and close the tab.
    if reply.usage.price.micros() > 0 {
        let steps = reply.usage.position;
        let payment = session.streams.pay(&stream, steps)?;
        let settling = ask(&key, &listing, &url, "", payment, None)?;
        settling
            .usage
            .verify(&listing.attestation.binding.signing_public_key)?;
        println!("paid {} USD for it", settling.usage.price);
    }
    println!("(the stream stays open for more; `sion stream close {stream}` takes back the rest)");
    Ok(())
}

pub(crate) fn measurement_bytes(text: &str) -> Result<[u8; 48], CliError> {
    let bytes = hex::decode(text).map_err(|e| CliError::Usage(format!("the measurement: {e}")))?;
    let mut measurement = [0u8; 48];
    if bytes.len() != 48 {
        return Err(CliError::Usage(
            "a measurement is 48 bytes (96 hex digits)".into(),
        ));
    }
    measurement.copy_from_slice(&bytes);
    Ok(measurement)
}

/// An account from 64 hex digits, or a devnet wallet's name.
pub(crate) fn account_of(ledger: &dyn Ledger, text: &str) -> Result<AccountId, CliError> {
    if text.len() == 64 && text.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(AccountId::from_hash(parse_hash(text, "the owner")?));
    }
    let mut session = Session::open(ledger)?;
    Ok(session.id(text))
}

/// Opens a payment stream to `listing`'s payee and keeps its chain, so the
/// renter can pay by the token: the stream, and the steps its chain holds.
pub fn open_stream(
    session: &mut Session<'_>,
    wallet: &str,
    listing: &Listing,
    deposit: Amount,
    pulses: u64,
) -> Result<ObjectId, CliError> {
    if deposit.micros() < listing.min_deposit.micros() {
        return Err(CliError::Usage(format!(
            "this listing wants at least {} USD escrowed",
            listing.min_deposit
        )));
    }
    let steps = u64::try_from(deposit.micros() / listing.unit.micros().max(1)).unwrap_or(u64::MAX);
    let length = steps.max(1);
    let spacing = crate::stream::default_spacing(length);
    let (seed, chain) = PayerBook::new_chain(length, spacing);
    let until = Pulse(session.ledger.pulse()?.0.saturating_add(pulses));
    let view = session.send(
        wallet,
        TxKind::OpenStream {
            payee: listing.payee,
            asset: AssetId::usd(),
            unit: listing.unit,
            length,
            anchor: chain.anchor(),
            deposit,
            until,
            checkpoints: chain.checkpoints(),
        },
        Vec::new(),
    )?;
    let stream = view
        .effects
        .events
        .iter()
        .find_map(|e| match e {
            Event::StreamOpened { stream, .. } => Some(*stream),
            _ => None,
        })
        .ok_or_else(|| CliError::Transaction("the stream did not open".into()))?;
    session.streams.add(
        PayerRecord {
            stream,
            wallet: wallet.to_string(),
            payee: listing.payee,
            unit: listing.unit,
            length,
            seed,
            revealed: 0,
            spacing,
            announced: None,
        },
        chain,
    )?;
    Ok(stream)
}

/// Sends one sealed request to `url` and opens the reply.
pub fn ask(
    key: &RenterKey,
    listing: &Listing,
    url: &str,
    message: &str,
    payment: crate::stream::Payment,
    jump: Option<sion_exec::stream::Jump>,
) -> Result<crate::rentals::Reply, CliError> {
    let sealed = key.seal_request(listing, message, payment, jump)?;
    let body = serde_json::to_vec(&sealed)
        .map_err(|e| CliError::Usage(format!("the sealed request: {e}")))?;
    let client = sion_http::Client::new(sion_http::Options {
        allow_private: true,
        timeout: std::time::Duration::from_secs(600),
        ..sion_http::Options::default()
    })
    .map_err(|e| CliError::Usage(e.to_string()))?;
    let fetched = client
        .request(
            "POST",
            url,
            &[("Content-Type", "application/json")],
            Some(&body),
            4 << 20,
        )
        .map_err(|e| CliError::Usage(format!("the enclave did not answer: {e}")))?;
    let sealed: SealedEnvelope = serde_json::from_slice(&fetched.body)
        .map_err(|e| CliError::Protocol(format!("the sealed reply: {e}")))?;
    key.open_reply(&sealed)
}
