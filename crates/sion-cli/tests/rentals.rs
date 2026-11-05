//! Renting an agent out, end to end on a devnet: a renter seals requests
//! to a simulated enclave, pays by the token through a payment stream, and
//! the owner is paid what the enclave signed for. No agent model or
//! hardware: the "agent" is a closure that returns a reply and a count of
//! tokens, and the enclave is simulated.

use std::sync::Arc;

use sion_agent::model::Usage;
use sion_cli::ledger::{Ledger, LedgerExt};
use sion_cli::rentals::{self, EnclaveKeys, Listing, ModelAccount, RentalServer, RenterKey};
use sion_cli::session::{wallet_for, Session};
use sion_cli::stream::{PayerBook, PayerRecord, Payment, StreamDesk};
use sion_core::ids::{AssetId, ObjectId, Pulse};
use sion_core::Amount;
use sion_enclave::simulated::SimulatedPlatform;
use sion_exec::stream::Jump;
use sion_exec::TxKind;
use sion_node::{Devnet, DevnetConfig};

fn usd(n: u64) -> Amount {
    Amount::units(n)
}

#[test]
fn an_agent_is_rented_by_the_token_and_the_owner_is_paid_what_the_enclave_signs() {
    let devnet = Arc::new(Devnet::start(DevnetConfig::default()).unwrap());
    let ledger: &dyn Ledger = devnet.as_ref();
    let mut session = Session::open(ledger).unwrap();
    session.fund("owner", AssetId::usd(), usd(1)).unwrap();
    session.fund("renter", AssetId::usd(), usd(100)).unwrap();
    let owner = session.id("owner");

    // The owner lists an agent: 3000 USD-micros per 1k input tokens, 15000
    // per 1k output, in 1000-micro steps. The enclave is simulated; its
    // keys are bound to the listing's terms.
    let agent = ObjectId::derive(b"rented-agent");
    let per_in = Amount(3_000);
    let per_out = Amount(15_000);
    let unit = Amount(1_000);
    let keys = EnclaveKeys::generate();
    let terms = rentals::terms_digest(&agent, &owner, per_in, per_out, ModelAccount::Owner);
    let platform = SimulatedPlatform::generate().unwrap();
    let attestation = platform
        .attest(keys.binding(&agent, terms, 100_000), [0x5A; 48])
        .unwrap();
    let listing = Listing {
        name: "summarizer".into(),
        agent,
        payee: owner,
        per_1k_input: per_in,
        per_1k_output: per_out,
        model_account: ModelAccount::Owner,
        unit,
        min_deposit: usd(1),
        description: "summarizes what you send".into(),
        attestation,
    };
    assert!(listing.terms_match());

    // The renter opens a payment stream to the owner.
    let length = 1_000_000u64;
    let (seed, chain) = PayerBook::new_chain(length, 0);
    let until = Pulse(ledger.pulse().unwrap().0 + 10_000);
    let view = session
        .send(
            "renter",
            TxKind::OpenStream {
                payee: owner,
                asset: AssetId::usd(),
                unit,
                length,
                anchor: chain.anchor(),
                deposit: usd(10),
                until,
                checkpoints: chain.checkpoints(),
            },
            Vec::new(),
        )
        .unwrap();
    let stream = view
        .effects
        .events
        .iter()
        .find_map(|e| match e {
            sion_exec::execute::Event::StreamOpened { stream, .. } => Some(*stream),
            _ => None,
        })
        .unwrap();
    session
        .streams
        .add(
            PayerRecord {
                stream,
                wallet: "renter".into(),
                payee: owner,
                unit,
                length,
                seed,
                revealed: 0,
                spacing: 0,
                announced: None,
            },
            chain,
        )
        .unwrap();

    // The server: the simulated enclave, the listing, the owner's desk.
    let desk = StreamDesk::new(wallet_for("owner"));
    let server = RentalServer::new(keys, listing.clone(), desk);
    let renter = RenterKey::generate();

    // Each request's "agent" returns a reply and the tokens it used.
    let turns = [(2_000u64, 500u64), (1_000, 300), (4_000, 1_000)];
    let run = |tokens: (u64, u64)| {
        move |message: &str| -> Result<(String, Usage), sion_cli::error::CliError> {
            Ok((
                format!("summary of {message}"),
                Usage {
                    input: tokens.0,
                    output: tokens.1,
                    ..Usage::default()
                },
            ))
        }
    };

    let now = || ledger.pulse().unwrap();
    let mut cumulative = Usage::default();
    let mut owed_before = Amount::ZERO;

    // The payment a request carries covers the total signed before it.
    let pay = |session: &mut Session, owed: Amount| -> (Payment, Option<Jump>) {
        if owed.micros() == 0 {
            // Nothing owed yet: a placeholder payment at position 0.
            return (
                Payment {
                    stream,
                    position: 0,
                    value: chain_anchor(session, &stream),
                },
                None,
            );
        }
        let position = rentals::steps_of(owed, unit);
        let record_position = session.streams.record(&stream).unwrap().revealed;
        let steps = position - record_position;
        let payment = session.streams.pay(&stream, steps).unwrap();
        (payment, None)
    };

    for (i, tokens) in turns.iter().enumerate() {
        let (payment, jump) = pay(&mut session, owed_before);
        let sealed = renter
            .seal_request(&listing, &format!("request {i}"), payment, jump)
            .unwrap();
        let reply_sealed = server.serve(ledger, now(), &sealed, run(*tokens)).unwrap();
        let reply = renter.open_reply(&reply_sealed).unwrap();
        assert_eq!(reply.text, format!("summary of request {i}"));
        // The enclave's count checks under the bound signing key.
        reply
            .usage
            .verify(&listing.attestation.binding.signing_public_key)
            .unwrap();
        // It is the price of the running total.
        cumulative.add(&Usage {
            input: tokens.0,
            output: tokens.1,
            ..Usage::default()
        });
        assert_eq!(reply.usage.price, rentals::price_of(&listing, &cumulative));
        owed_before = reply.usage.price;
    }

    // A final settling request pays the last total, running nothing.
    let (payment, jump) = pay(&mut session, owed_before);
    let sealed = renter.seal_request(&listing, "", payment, jump).unwrap();
    let closing = renter
        .open_reply(&server.serve(ledger, now(), &sealed, run((0, 0))).unwrap())
        .unwrap();
    assert_eq!(closing.text, "");
    assert_eq!(
        closing.usage.price,
        rentals::price_of(&listing, &cumulative)
    );

    // The owner is paid what was signed for, no more.
    server
        .close_out(ledger, std::time::Duration::from_secs(30))
        .unwrap();
    let total = rentals::price_of(&listing, &cumulative);
    assert!(total.micros() > 0);
    // On the ledger, the stream paid out exactly the signed total: its
    // position is the total in steps, and that much escrow was consumed.
    let settled = sion_cli::stream::load_stream(ledger, &stream).unwrap();
    assert_eq!(settled.position, rentals::steps_of(total, unit));
    assert_eq!(
        usd(10).micros() - settled.escrow.micros(),
        total.micros(),
        "the renter's escrow fell by exactly the signed total"
    );
    // The owner received the total, less the small fee it pays to settle.
    let owner_gain = session.balance("owner", AssetId::usd()).unwrap().micros() - usd(1).micros();
    assert!(
        owner_gain <= total.micros() && owner_gain + 10_000 >= total.micros(),
        "the owner received the total minus a small fee: {owner_gain} of {}",
        total.micros()
    );
}

/// The chain's anchor, read from the payer book.
fn chain_anchor(session: &mut Session, stream: &ObjectId) -> sion_core::hash::Hash32 {
    // Position 0 is the anchor; paying 0 steps is not allowed, so read the
    // stream's head from the ledger.
    let _ = session;
    sion_cli::stream::load_stream(session.ledger, stream)
        .map(|s| s.head)
        .unwrap()
}
