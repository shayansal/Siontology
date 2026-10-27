//! Scripted demonstrations against a live ledger.
//!
//! Each demo drives real transactions through the full protocol — lane
//! certificates, consensus, Pulse execution, header certificates — and
//! narrates what the ledger established and why it can be trusted.

use std::time::{Duration, Instant};

use fold_assurance::{atom::standard, AtomClass, Formula, Policy};
use fold_clearing::Side;
use fold_core::hash::hash_domain;
use fold_core::ids::{AssetId, ClaimId, ObjectId, Pulse};
use fold_core::{Amount, Cost};
use fold_exec::charter::Caveats;
use fold_exec::claim::{claim_id, formulas, ClaimBody, Payload};
use fold_exec::execute::bond_atom;
use fold_exec::market::{BatchStatus, OrderTerms, Submission};
use fold_exec::{Event, ExecStatus, ObjectData, TxKind};
use fold_node::view::TxView;
use serde_json::Value;

use crate::error::CliError;
use crate::ledger::LedgerExt;
use crate::session::Session;

/// Collects or prints a demo's narration.
#[derive(Debug, Default)]
pub struct Narrator {
    /// Lines so far.
    pub lines: Vec<String>,
    /// Whether to print as it goes.
    pub print: bool,
}

impl Narrator {
    /// A narrator that prints.
    pub fn printing() -> Self {
        Narrator {
            lines: Vec::new(),
            print: true,
        }
    }

    /// Says something.
    pub fn say(&mut self, line: impl Into<String>) {
        let line = line.into();
        if self.print {
            println!("{line}");
        }
        self.lines.push(line);
    }
}

fn usd(n: u64) -> Amount {
    Amount::units(n)
}

fn events(view: &TxView) -> &[Event] {
    &view.effects.events
}

fn cost(c: &Cost) -> String {
    match c {
        Cost::Infinite => "∞ (trusted)".into(),
        Cost::Finite(a) => format!("{a} USD"),
    }
}

fn decision_line(value: &Value) -> String {
    let accept = value["accept"].as_bool().unwrap_or(false);
    let reason = value["reasons"][0].as_str().unwrap_or("");
    format!("{}: {reason}", if accept { "ACCEPT" } else { "REJECT" })
}

/// Funds several accounts from the faucet and waits until all are funded.
pub fn fund_all(
    session: &mut Session<'_>,
    grants: &[(&str, AssetId, Amount)],
) -> Result<(), CliError> {
    let mut targets = Vec::new();
    for (name, asset, amount) in grants {
        let before = session.balance(name, *asset)?;
        let id = session.id(name);
        session.ledger.faucet(id, *asset, *amount)?;
        targets.push((name.to_string(), *asset, Amount(before.0 + amount.0)));
    }
    let deadline = Instant::now() + session.timeout;
    for (name, asset, target) in targets {
        while session.balance(&name, asset)? < target {
            if Instant::now() > deadline {
                return Err(CliError::Transaction(format!(
                    "faucet grant to {name} did not arrive"
                )));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    Ok(())
}

fn find<T>(view: &TxView, f: impl Fn(&Event) -> Option<T>) -> Result<T, CliError> {
    events(view)
        .iter()
        .find_map(f)
        .ok_or_else(|| CliError::Protocol("expected event missing".into()))
}

/// Invoice #1042: insured reliance on attested knowledge, end to end.
pub fn invoice(session: &mut Session<'_>, out: &mut Narrator) -> Result<(), CliError> {
    out.say("== Invoice #1042: trade finance on insured knowledge ==");
    let names = ["supplier", "shipper", "judge", "financier", "buyer"];
    fund_all(
        session,
        &names
            .iter()
            .map(|n| (*n, AssetId::usd(), usd(200_000)))
            .collect::<Vec<_>>(),
    )?;
    let (supplier, shipper, judge) = (
        session.id("supplier"),
        session.id("shipper"),
        session.id("judge"),
    );
    let s_atom = bond_atom(AtomClass::Economic, &supplier, "invoices");
    let w_atom = bond_atom(AtomClass::Economic, &shipper, "logistics");
    let j_atom = bond_atom(AtomClass::Adjudicator, &judge, "arbitration");

    for (who, class, label, amount) in [
        ("supplier", AtomClass::Economic, "invoices", 30_000),
        ("shipper", AtomClass::Economic, "logistics", 20_000),
        ("judge", AtomClass::Adjudicator, "arbitration", 100_000),
    ] {
        let view = session.must(
            who,
            TxKind::PostBond {
                class,
                label: label.into(),
                amount: usd(amount),
            },
            Vec::new(),
        )?;
        out.say(format!(
            "Pulse {:>3}  {who} bonds {} USD behind \"{label}\"",
            view.pulse,
            usd(amount)
        ));
    }

    let delivery = ClaimBody {
        schema: "trade.delivery/v1".into(),
        subject: "Shipment for invoice #1042 delivered to the buyer".into(),
        payload: Payload::Commitment(hash_domain("demo/pod", b"proof-of-delivery-1042")),
        formula: formulas::bonded_statement(w_atom.id),
        adjudicator: Some(j_atom.id),
        detection_permille: 1000,
        valid_for: 2_000,
        challenge_period: 500,
    };
    let delivery_id = claim_id(&shipper, &delivery);
    let view = session.must(
        "shipper",
        TxKind::IssueClaim { claim: delivery },
        Vec::new(),
    )?;
    out.say(format!(
        "Pulse {:>3}  shipper attests \"Shipment for invoice #1042 delivered\"  [dual-signature ∧ bond:shipper]",
        view.pulse
    ));
    session.wait_pulses(1)?;

    let invoice = ClaimBody {
        schema: "trade.invoice/v1".into(),
        subject: "Invoice #1042: 10,000 USD payable by the buyer".into(),
        payload: Payload::Public(b"invoice-1042".to_vec()),
        formula: Formula::and(vec![
            formulas::dual_signature(),
            Formula::or(vec![Formula::atom(s_atom.id), Formula::claim(delivery_id)]),
        ]),
        adjudicator: Some(j_atom.id),
        detection_permille: 1000,
        valid_for: 2_000,
        challenge_period: 500,
    };
    let invoice_id = claim_id(&supplier, &invoice);
    let view = session.must(
        "supplier",
        TxKind::IssueClaim { claim: invoice },
        Vec::new(),
    )?;
    out.say(format!(
        "Pulse {:>3}  supplier issues \"Invoice #1042: 10,000 USD payable\"  [dual-signature ∧ (bond:supplier ∨ delivery)]",
        view.pulse
    ));
    session.wait_pulses(1)?;
    let record = session.ledger.claim(invoice_id)?;
    let coc: fold_assurance::CocInterval =
        serde_json::from_value(record["coc_standard_policy"].clone())
            .map_err(|e| CliError::Protocol(e.to_string()))?;
    out.say(format!(
        "           cost of corruption (standard policy): {}{}",
        cost(&coc.low),
        if coc.exact { ", exact" } else { "" }
    ));
    out.say("           = 30,000 (supplier's bond) + 20,000 (shipper's bond): a forger must corrupt both routes");
    for value in [10_000u64, 30_000] {
        let decision = session
            .ledger
            .decide(invoice_id, usd(value), Policy::standard())?;
        out.say(format!(
            "           rely on {value:>6} USD at κ = 2 → {}",
            decision_line(&decision)
        ));
    }

    let until = Pulse(session.ledger.pulse()?.0 + 1_000);
    let view = session.must(
        "financier",
        TxKind::Accept {
            claim: invoice_id,
            value_at_risk: usd(10_000),
            policy: Policy::standard(),
            until,
            nonce: 0,
        },
        Vec::new(),
    )?;
    let reserved = find(&view, |e| match e {
        Event::Reserved { amount, .. } => Some(*amount),
        _ => None,
    })?;
    out.say(format!(
        "Pulse {:>3}  financier funds the invoice and reserves {reserved} USD of its backing (κ·V): \
         nobody else can rely on the same bonds beyond what slashing can repay",
        view.pulse
    ));
    let financier_before = session.balance("financier", AssetId::usd())?;

    let challenge = |session: &mut Session<'_>, claim: ClaimId, target, tag: &str| {
        session.must(
            "buyer",
            TxKind::Challenge {
                claim,
                target,
                evidence: hash_domain("demo/evidence", tag.as_bytes()),
                deposit: usd(1_000),
            },
            Vec::new(),
        )
    };
    let opened = |view: &TxView| {
        find(view, |e| match e {
            Event::ChallengeOpened { challenge, .. } => Some(*challenge),
            _ => None,
        })
    };
    let view = challenge(
        session,
        invoice_id,
        s_atom.id,
        "goods never left the warehouse",
    )?;
    let first = opened(&view)?;
    out.say(format!(
        "Pulse {:>3}  buyer challenges the supplier's attestation (deposit 1,000 USD)",
        view.pulse
    ));
    let view = session.must(
        "judge",
        TxKind::Rule {
            challenge: first,
            upheld: true,
        },
        Vec::new(),
    )?;
    let slashed = find(&view, |e| match e {
        Event::Slashed { amount, .. } => Some(*amount),
        _ => None,
    })?;
    out.say(format!(
        "Pulse {:>3}  judge upholds: supplier's bond slashed ({slashed} USD); the invoice still holds through the delivery route",
        view.pulse
    ));
    let view = challenge(
        session,
        delivery_id,
        w_atom.id,
        "the bill of lading is forged",
    )?;
    let second = opened(&view)?;
    out.say(format!(
        "Pulse {:>3}  buyer challenges the shipper's delivery attestation, under the shipper's own adjudicator",
        view.pulse
    ));
    let view = session.must(
        "judge",
        TxKind::Rule {
            challenge: second,
            upheld: true,
        },
        Vec::new(),
    )?;
    let falsified = events(&view)
        .iter()
        .filter(|e| matches!(e, Event::Falsified { .. }))
        .count();
    let paid = find(&view, |e| match e {
        Event::Compensated { amount, .. } => Some(*amount),
        _ => None,
    })?;
    out.say(format!(
        "Pulse {:>3}  judge upholds: {falsified} claims falsified (the delivery, and the invoice resting on it)",
        view.pulse
    ));
    let financier_after = session.balance("financier", AssetId::usd())?;
    out.say(format!(
        "           financier compensated {paid} USD automatically from the slashed bonds (balance {} → {})",
        financier_before, financier_after
    ));
    out.say("Result: reliance on a false invoice was repaid in full, in Pulses, with no lawsuit and no adjuster.");
    Ok(())
}

fn charter_caveats(vendor: fold_core::ids::AccountId) -> Caveats {
    Caveats {
        payees: Some([vendor].into_iter().collect()),
        max_per_payment: usd(100),
        max_per_window: usd(250),
        window: 10_000,
        expires_at: Pulse(1_000_000),
        evidence_above: None,
        evidence_policy: Policy::standard(),
        receipts: true,
        may_delegate: true,
    }
}

/// Agent Charters: protocol-enforced limits for an AI agent.
pub fn agents(session: &mut Session<'_>, out: &mut Narrator) -> Result<(), CliError> {
    out.say("== Agent Charters: an AI agent with a budget the ledger enforces ==");
    fund_all(
        session,
        &[
            ("acme", AssetId::usd(), usd(20_000)),
            ("acme-agent", AssetId::usd(), usd(100)),
            ("sub-agent", AssetId::usd(), usd(100)),
        ],
    )?;
    let (agent, vendor, mallory, sub) = (
        session.id("acme-agent"),
        session.id("gpu-cloud"),
        session.id("mallory"),
        session.id("sub-agent"),
    );
    let view = session.must(
        "acme",
        TxKind::CreateCharter {
            agent,
            caveats: charter_caveats(vendor),
            funding: usd(1_000),
            agent_bond: usd(500),
        },
        Vec::new(),
    )?;
    let (charter, allotment) = find(&view, |e| match e {
        Event::CharterCreated { charter, allotment } => Some((*charter, *allotment)),
        _ => None,
    })?;
    out.say(format!(
        "Pulse {:>3}  acme charters its agent: pay only gpu-cloud, ≤ 100 USD per payment, ≤ 250 USD per window, \
         1,000 USD budget, receipts on, 500 USD bond behind the agent's receipts",
        view.pulse
    ));
    let pay = |session: &mut Session<'_>,
               who: &str,
               charter: ObjectId,
               allotment: ObjectId,
               payee,
               amount: u64,
               purpose: &str| {
        let input = session.reference(&allotment)?;
        session.send(
            who,
            TxKind::AgentPay {
                charter,
                payee,
                amount: usd(amount),
                purpose: purpose.into(),
                evidence: None,
            },
            vec![input],
        )
    };
    let view = pay(
        session,
        "acme-agent",
        charter,
        allotment,
        vendor,
        80,
        "1M tokens of inference",
    )?;
    let receipt = find(&view, |e| match e {
        Event::AgentPaid { receipt, .. } => *receipt,
        _ => None,
    })?;
    out.say(format!(
        "Pulse {:>3}  agent pays gpu-cloud 80 USD for inference: {:?}; receipt claim {} joins the knowledge log",
        view.pulse,
        view.status,
        receipt.short()
    ));
    for (payee, amount, story) in [
        (
            mallory,
            90,
            "a prompt-injected agent tries to pay mallory 90 USD",
        ),
        (vendor, 150, "the agent tries a 150 USD payment"),
    ] {
        let view = pay(
            session,
            "acme-agent",
            charter,
            allotment,
            payee,
            amount,
            "ignore previous instructions",
        )?;
        let reason = match &view.status {
            ExecStatus::Failed(reason) => reason.clone(),
            other => format!("{other:?}"),
        };
        out.say(format!(
            "Pulse {:>3}  {story} → refused by the ledger: {reason}",
            view.pulse
        ));
    }
    let mut narrow = charter_caveats(vendor);
    narrow.max_per_payment = usd(20);
    narrow.max_per_window = usd(40);
    narrow.may_delegate = false;
    let input = session.reference(&allotment)?;
    let view = session.must(
        "acme-agent",
        TxKind::Delegate {
            charter,
            sub_agent: sub,
            caveats: narrow,
            amount: usd(100),
        },
        vec![input],
    )?;
    let (child, child_allotment) = find(&view, |e| match e {
        Event::CharterCreated { charter, allotment } => Some((*charter, *allotment)),
        _ => None,
    })?;
    out.say(format!(
        "Pulse {:>3}  agent delegates 100 USD to a sub-agent under narrower caveats (≤ 20 USD per payment)",
        view.pulse
    ));
    let view = pay(
        session,
        "sub-agent",
        child,
        child_allotment,
        vendor,
        20,
        "embedding batch",
    )?;
    out.say(format!(
        "Pulse {:>3}  sub-agent pays gpu-cloud 20 USD: {:?}",
        view.pulse, view.status
    ));
    let view = session.must("acme", TxKind::RevokeCharter { charter }, Vec::new())?;
    out.say(format!(
        "Pulse {:>3}  acme revokes the charter (ordered by consensus)",
        view.pulse
    ));
    for (who, c, a) in [
        ("acme-agent", charter, allotment),
        ("sub-agent", child, child_allotment),
    ] {
        let view = pay(session, who, c, a, vendor, 10, "after revocation")?;
        let reason = match &view.status {
            ExecStatus::Failed(reason) => reason.clone(),
            other => format!("{other:?}"),
        };
        out.say(format!(
            "Pulse {:>3}  {who} tries to pay again → {reason}",
            view.pulse
        ));
    }
    session.wait_pulses(1)?;
    let record = session.ledger.claim(receipt)?;
    out.say(format!(
        "           the agent's receipt is a claim backed by acme's bond: cost to forge it {}",
        cost(
            &serde_json::from_value::<fold_assurance::CocInterval>(
                record["coc_standard_policy"].clone()
            )
            .map(|c| c.low)
            .unwrap_or(Cost::ZERO)
        )
    ));
    let vendor_balance = session.balance("gpu-cloud", AssetId::usd())?;
    out.say(format!(
        "Result: gpu-cloud received {vendor_balance} USD; mallory received nothing; the limits lived in the ledger, not the agent."
    ));
    Ok(())
}

/// The GPU-hour asset used in demos.
pub fn gpu() -> AssetId {
    AssetId::derive(b"gpu-hour")
}

fn cents(c: u128) -> Amount {
    Amount(c * 10_000)
}

/// Verifiable Clearing: a sealed-batch GPU-hour market that proves its own optimality.
pub fn market(session: &mut Session<'_>, out: &mut Narrator) -> Result<(), CliError> {
    out.say("== Verifiable Clearing: a GPU-hour market that proves it is optimal ==");
    fund_all(
        session,
        &[
            ("operator", AssetId::usd(), usd(1_000)),
            ("lab-a", AssetId::usd(), usd(100)),
            ("lab-b", AssetId::usd(), usd(100)),
            ("agent-1", AssetId::usd(), usd(1_000)),
            ("agent-2", AssetId::usd(), usd(1_000)),
            ("solver", AssetId::usd(), usd(100)),
            ("lab-a", gpu(), usd(5)),
            ("lab-b", gpu(), usd(6)),
        ],
    )?;
    // Long enough that all four orders land in one batch on a slow machine.
    let batch_length = 12;
    let view = session.must(
        "operator",
        TxKind::CreateMarket {
            label: "gpu-hours before 18:00".into(),
            good: gpu(),
            numeraire: AssetId::usd(),
            batch_length,
            // Generous, so a slow machine still clears in time.
            clear_deadline: 30,
            committee: Vec::new(),
            threshold: 0,
        },
        Vec::new(),
    )?;
    let market = find(&view, |e| match e {
        Event::MarketCreated { market } => Some(*market),
        _ => None,
    })?;
    out.say(format!(
        "Pulse {:>3}  operator opens a market: batches of {batch_length} Pulses, sealed with the beacon, cleared by anyone with a certificate",
        view.pulse
    ));
    // Start at the beginning of a batch window so every order lands in one batch.
    loop {
        let now = session.ledger.pulse()?.0;
        if (now + 1) % batch_length == 0 {
            break;
        }
        session.wait_pulses(1)?;
    }
    let orders = [
        ("agent-1", Side::Buy, 5, 210),
        ("agent-2", Side::Buy, 3, 190),
        ("lab-a", Side::Sell, 5, 180),
        ("lab-b", Side::Sell, 6, 200),
    ];
    let mut digests = Vec::new();
    for (who, side, quantity, price) in orders {
        digests.push(session.submit(
            who,
            TxKind::SubmitOrder {
                market,
                submission: Submission::Plain(OrderTerms {
                    side,
                    quantity,
                    limit_price: cents(price),
                }),
            },
            Vec::new(),
        )?);
    }
    for ((who, side, quantity, price), digest) in orders.iter().zip(digests) {
        let view = session.wait(digest)?;
        out.say(format!(
            "Pulse {:>3}  {who} {} {quantity} GPU-hours at ≤ {}.{:02} USD: {:?}",
            view.pulse,
            match side {
                Side::Buy => "bids for",
                Side::Sell => "offers",
            },
            price / 100,
            price % 100,
            view.status
        ));
    }
    session.settle_all();
    // Wait for the batch to seal.
    let (index, batch) = loop {
        let object = session
            .ledger
            .object(market)?
            .ok_or_else(|| CliError::Protocol("market vanished".into()))?;
        let ObjectData::Market(state) = object.data else {
            return Err(CliError::Protocol("not a market".into()));
        };
        if let Some((index, entry)) = state
            .batches
            .iter()
            .find(|(_, b)| matches!(b.status, BatchStatus::Sealed { .. }))
        {
            let BatchStatus::Sealed { beacon, at } = entry.status else {
                unreachable!()
            };
            break (*index, state.clearing_batch(entry, beacon, at));
        }
        session.wait_pulses(1)?;
    };
    out.say(format!(
        "           batch {index} sealed with the Pulse beacon {} — {} orders, nobody can reorder them now",
        batch.beacon.short(),
        batch.orders.len()
    ));
    let result = fold_clearing::clear(&batch).map_err(|e| CliError::Build(e.to_string()))?;
    let band = result.band.expect("trade happens");
    out.say(format!(
        "           solver computes the welfare-maximizing allocation: {} GPU-hours at one price {} USD (supporting band {}–{}), \
         welfare {} USD, with an LP optimality certificate",
        result.volume,
        result.price.unwrap_or(Amount::ZERO),
        band.low,
        band.high,
        result.welfare
    ));
    let view = session.must(
        "solver",
        TxKind::ClearBatch {
            market,
            batch: index,
            result,
        },
        Vec::new(),
    )?;
    out.say(format!(
        "Pulse {:>3}  the ledger checks the certificate in linear time and settles everyone at that price",
        view.pulse
    ));
    for who in ["agent-1", "agent-2", "lab-a", "lab-b"] {
        out.say(format!(
            "           {who:<8} now holds {} GPU-hours and {} USD",
            session.balance(who, gpu())?,
            session.balance(who, AssetId::usd())?
        ));
    }
    out.say("Result: one uniform, certified-optimal price; a place in the batch is worth nothing, so there is nothing to front-run.");
    Ok(())
}

/// AI inference receipts priced by the Assurance Algebra.
pub fn receipts(session: &mut Session<'_>, out: &mut Narrator) -> Result<(), CliError> {
    out.say("== Inference receipts: what would it cost to forge this answer? ==");
    fund_all(session, &[("inference-co", AssetId::usd(), usd(10_000))])?;
    let provider = session.id("inference-co");
    let bond = bond_atom(AtomClass::Economic, &provider, "inference");
    let view = session.must(
        "inference-co",
        TxKind::PostBond {
            class: AtomClass::Economic,
            label: "inference".into(),
            amount: usd(5_000),
        },
        Vec::new(),
    )?;
    out.say(format!(
        "Pulse {:>3}  inference-co bonds 5,000 USD behind its receipts",
        view.pulse
    ));
    let tee = standard::tee_resists_host("nvidia-h100");
    let body = ClaimBody {
        schema: "receipt.ai.inference/v1".into(),
        subject: "model fold-llm-7b answered prompt H(p) with answer H(a)".into(),
        payload: Payload::Commitment(hash_domain("demo/inference", b"prompt|answer|model")),
        formula: formulas::inference_receipt(tee.id, bond.id),
        adjudicator: None,
        detection_permille: 1000,
        valid_for: 10_000,
        challenge_period: 100,
    };
    let receipt = claim_id(&provider, &body);
    let view = session.must(
        "inference-co",
        TxKind::IssueClaim { claim: body },
        Vec::new(),
    )?;
    out.say(format!(
        "Pulse {:>3}  receipt issued  [dual-signature ∧ (TEE quote: nvidia-h100 ∨ bond:inference-co)]",
        view.pulse
    ));
    session.wait_pulses(1)?;
    let record = session.ledger.claim(receipt)?;
    let coc: fold_assurance::CocInterval =
        serde_json::from_value(record["coc_standard_policy"].clone())
            .map_err(|e| CliError::Protocol(e.to_string()))?;
    out.say(format!(
        "           standard policy: cost to forge = {}  (TEE attack ≈ 1,000 USD + 5,000 USD bond)",
        cost(&coc.low)
    ));
    out.say("           the TEE's attack cost deters forgery but cannot repay anyone; only the bond is slashable");
    let skeptic = Policy::standard().assuming_broken([tee.id]);
    let trusting = Policy::standard().trusting([tee.id]);
    for (label, policy) in [
        ("standard", Policy::standard()),
        ("assumes TEE broken", skeptic),
        ("trusts the TEE", trusting),
    ] {
        for value in [2_000u64, 3_000] {
            let decision = session.ledger.decide(receipt, usd(value), policy.clone())?;
            out.say(format!(
                "           {label:<18} relying on {value} USD → {}",
                decision_line(&decision)
            ));
        }
    }
    out.say("Result: every consumer prices the same receipt under its own assumptions; trust is a number, not a brand.");
    Ok(())
}

/// Light-client verification of the latest certified Pulse.
pub fn light(session: &mut Session<'_>, out: &mut Narrator) -> Result<(), CliError> {
    out.say("== Light client: verify without running a node ==");
    // A real client pins the chain identity from a source it trusts (a
    // release, a block explorer, a friend). Here it comes from the devnet's
    // own status, which is what `fold devnet` prints at startup.
    let validators = session.ledger.query(fold_node::Query::Validators)?;
    let chain: fold_light::ChainSpec = serde_json::from_value(validators["chain"].clone())
        .map_err(|e| CliError::Protocol(e.to_string()))?;
    let pinned = chain.id();
    out.say(format!(
        "           pinned chain identity {} (genesis, validators, keys and shard layout)",
        pinned.short()
    ));
    let report = crate::verify::verify_pulse(session.ledger, Some(pinned), None, 16, true)?;
    for line in report {
        out.say(format!("           {line}"));
    }
    if !chain.insecure_signatures {
        // What a lying node would have to do to fool the client: serve its
        // own keys under a real validator's identifier. The pin catches it.
        let mut forged = chain.clone();
        let attacker =
            fold_crypto::DualKeypair::from_seed(&[0xBA; 32], fold_crypto::Scheme::SlhDsaSha2_128f);
        if let Some((victim, _)) = forged.validators.first().copied() {
            forged.keys.keys.insert(
                victim,
                (
                    attacker.lattice.public().clone(),
                    attacker.hash.public().clone(),
                ),
            );
        }
        match forged.verify(Some(&pinned)) {
            Err(e) => out.say(format!(
                "           a node serving substituted validator keys is caught: {e} ✗"
            )),
            Ok(_) => {
                return Err(CliError::Verification(
                    "a forged validator set passed the pin".into(),
                ))
            }
        }
    }
    Ok(())
}

/// Censorship resistance: a censored user gets through with a forced
/// intent; when too many validators censor, certified headers show it.
pub fn censorship(session: &mut Session<'_>, out: &mut Narrator) -> Result<(), CliError> {
    out.say("== Censorship resistance: forced inclusion makes censorship visible ==");
    fund_all(session, &[("dissident", AssetId::usd(), usd(100))])?;
    let dissident = session.id("dissident");
    session.wallet("dissident").validity = 200;
    let alice = session.id("alice");
    let names = session.names();
    let window = session.ledger.query(fold_node::Query::Forced)?["window"]
        .as_u64()
        .unwrap_or(8);
    let build = |session: &mut Session<'_>, amount: u64| {
        let view = crate::session::RemoteView::new(session.ledger, session.config.clone());
        session.wallet("dissident").build(
            &view,
            TxKind::Transfer {
                asset: AssetId::usd(),
                outputs: vec![(alice, usd(amount))],
            },
            Vec::new(),
        )
    };

    // 1. The gateway censors the dissident.
    session.ledger.censor(&[dissident], true, 0)?;
    out.say("           ADVERSARY SIMULATION: the gateway now drops every transaction from dissident, silently");
    let tx = build(session, 5)?;
    let receipt = session.ledger.submit(&tx)?;
    out.say(format!(
        "           dissident signs: {}",
        crate::describe::sentence(&names, &tx.tx)
    ));
    session.wait_pulses(3)?;
    let ignored = !matches!(
        session.ledger.tx(receipt.digest)?,
        crate::ledger::TxState::Executed(_)
    );
    out.say(format!(
        "           3 Pulses later: {}",
        if ignored {
            "not executed; the gateway acknowledged it and did nothing"
        } else {
            "executed (the gateway did not censor after all)"
        }
    ));
    // 2. The dissident posts it straight to consensus.
    session.ledger.force(&tx)?;
    let view = session.wait(receipt.digest)?;
    session.wallet("dissident").settle();
    out.say(format!(
        "Pulse {:>3}  posted as a forced intent: one honest proposer ordered it, the shard's validators certified it among themselves",
        view.pulse
    ));
    out.say(format!(
        "           {}",
        crate::describe::receipt(&names, &view.effects)
    ));

    // 3. Now more than a third of the committee censors too.
    session.ledger.censor(&[dissident], true, 2)?;
    out.say("           ADVERSARY SIMULATION: 2 of the 4 validators now censor dissident as well (a quorum needs 3)");
    let tx = build(session, 7)?;
    let receipt = session.ledger.force(&tx)?;
    let posted = session.ledger.pulse()?;
    // Wait until the shard is overdue on it, then until a certified header
    // says so.
    let deadline = Instant::now() + session.timeout;
    let overdue_at: Pulse = loop {
        let forced = session.ledger.query(fold_node::Query::Forced)?;
        if forced["summary"]["overdue"].as_u64().unwrap_or(0) > 0 {
            break serde_json::from_value(forced["pulse"].clone())
                .map_err(|e| CliError::Protocol(e.to_string()))?;
        }
        if Instant::now() > deadline {
            return Err(CliError::Transaction(
                "the forced intent never fell due".into(),
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    out.say(format!(
        "           forced intent posted around Pulse {posted}; by Pulse {overdue_at} it is OVERDUE (window {window} Pulses): the shard owes it and cannot certify it"
    ));
    loop {
        let certified: Pulse =
            serde_json::from_value(session.ledger.status()?["certified_pulse"].clone())
                .map_err(|e| CliError::Protocol(e.to_string()))?;
        if certified >= overdue_at {
            break;
        }
        if Instant::now() > deadline {
            return Err(CliError::Transaction("no certified header".into()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let report = crate::verify::verify_pulse(
        session.ledger,
        Some(session_chain(session)?),
        Some(overdue_at),
        8,
        false,
    )?;
    if let Some(line) = report.iter().find(|l| l.starts_with("forced inclusion")) {
        out.say(format!("           light client: {line}"));
    }

    // 4. The censors stop; the overdue intent executes, late.
    session.ledger.censor(&[], false, 0)?;
    let view = session.wait(receipt.digest)?;
    session.wallet("dissident").settle();
    out.say(format!(
        "Pulse {:>3}  censorship lifted: the intent executes late, and the record shows it: {}",
        view.pulse,
        crate::describe::receipt(&names, &view.effects)
    ));
    out.say("Result: a censoring gateway is routed around; a censoring third of a committee cannot hide, because every certified header counts what it owes.");
    Ok(())
}

/// x402: an AI agent pays an API per request, inside limits the ledger
/// enforces.
pub fn x402(session: &mut Session<'_>, out: &mut Narrator) -> Result<(), CliError> {
    use std::sync::atomic::{AtomicBool, Ordering};

    use crate::x402::{fetch, network, replay, Facilitator, PaidRoute, PaidServer, Payer};

    out.say("== x402: an AI agent pays an API per request, inside limits the ledger enforces ==");
    fund_all(
        session,
        &[
            ("acme", AssetId::usd(), usd(1_000)),
            ("acme-agent", AssetId::usd(), usd(10)),
            ("bob", AssetId::usd(), usd(10)),
        ],
    )?;
    let chain = session_chain(session)?;
    let (agent, carol) = (session.id("acme-agent"), session.id("carol"));
    let caveats = Caveats {
        payees: Some([carol].into_iter().collect()),
        max_per_payment: usd(1),
        max_per_window: usd(2),
        window: 10_000,
        expires_at: Pulse(1_000_000),
        evidence_above: None,
        evidence_policy: Policy::standard(),
        receipts: true,
        may_delegate: false,
    };
    let view = session.must(
        "acme",
        TxKind::CreateCharter {
            agent,
            caveats,
            funding: usd(20),
            agent_bond: Amount::ZERO,
        },
        Vec::new(),
    )?;
    let (charter, allotment) = find(&view, |e| match e {
        Event::CharterCreated { charter, allotment } => Some((*charter, *allotment)),
        _ => None,
    })?;
    out.say(format!(
        "Pulse {:>3}  acme charters its agent: pay only carol, ≤ 1 USD per payment, ≤ 2 USD per window, 20 USD budget",
        view.pulse
    ));

    // carol's API: no accounts and no API keys, just prices.
    let facilitator = Facilitator::new(session.ledger, chain)?;
    let server = PaidServer::bind("127.0.0.1:0")?;
    let base = format!("http://{}", server.address);
    let routes = vec![
        PaidRoute {
            path: "/v1/forecast".into(),
            price: Amount(250_000),
            description: "GPU demand forecast, next 24 hours".into(),
            mime_type: "application/json".into(),
            body: Box::new(|_| {
                r#"{"region":"eu-west","gpu_hours":18250,"confidence":0.82}"#.to_string()
            }),
        },
        PaidRoute {
            path: "/v1/report".into(),
            price: usd(5),
            description: "Full market report".into(),
            mime_type: "text/plain".into(),
            body: Box::new(|_| "the full report".to_string()),
        },
    ];
    out.say(format!(
        "           carol serves a forecast API at {base}: 0.25 USD per call, settled on {}",
        network(&chain)
    ));
    let stop = AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        let serving = scope.spawn(|| {
            server.serve(
                &facilitator,
                carol,
                &routes,
                Duration::from_secs(120),
                &stop,
            )
        });
        let result = (|| -> Result<(), CliError> {
            let payer = Payer::Charter {
                agent: "acme-agent".into(),
                charter,
                allotment,
            };
            let forecast = format!("{base}/v1/forecast");
            let fetched = fetch(session, &payer, &forecast, &chain, usd(1))?;
            let quote = fetched
                .quoted
                .as_ref()
                .and_then(|q| q.accepts.first())
                .ok_or_else(|| CliError::Protocol("the API did not ask for payment".into()))?;
            out.say(format!(
                "           agent: GET /v1/forecast → 402 Payment Required: {} USD to carol (quote {})",
                Amount(quote.amount.parse().unwrap_or(0)),
                quote.extra["nonce"].as_str().unwrap_or("?")
            ));
            let settlement = fetched
                .settlement
                .as_ref()
                .filter(|s| s.success && fetched.status == 200)
                .ok_or_else(|| {
                    CliError::Transaction(format!(
                        "the paid request failed: {} {:?}",
                        fetched.status,
                        fetched
                            .settlement
                            .as_ref()
                            .and_then(|s| s.error_reason.clone())
                    ))
                })?;
            let pulse = settlement
                .extensions
                .as_ref()
                .map(|e| e["fold"]["pulse"].clone());
            out.say(format!(
                "Pulse {:>3}  agent pays under its charter (purpose x402:<quote>) and retries → 200 OK {}",
                pulse.and_then(|p| p.as_u64()).unwrap_or(0),
                fetched.body
            ));
            out.say("           carol's server answered only after verifying the receipt as a light client: committee certificate, certified order, inclusion proof");
            let replayed = replay(&forecast, fetched.payment.as_deref().unwrap_or_default())?;
            out.say(format!(
                "           replaying the same PAYMENT-SIGNATURE → {}: {}",
                replayed.status,
                replayed
                    .quoted
                    .as_ref()
                    .and_then(|q| q.error.clone())
                    .unwrap_or_default()
            ));
            let fetched = fetch(
                session,
                &payer,
                &format!("{base}/v1/report"),
                &chain,
                usd(10),
            )?;
            let refused = fetched
                .settlement
                .as_ref()
                .filter(|s| !s.success)
                .and_then(|s| s.error_reason.clone())
                .ok_or_else(|| {
                    CliError::Protocol("a 5 USD charter payment should have been refused".into())
                })?;
            out.say(format!(
                "           agent: GET /v1/report (5 USD) → pays → {}; carol's API answers {} and serves nothing",
                refused, fetched.status
            ));
            let fetched = fetch(
                session,
                &Payer::Wallet("bob".into()),
                &forecast,
                &chain,
                usd(1),
            )?;
            let settled = fetched
                .settlement
                .as_ref()
                .filter(|s| s.success && fetched.status == 200)
                .ok_or_else(|| CliError::Transaction("bob's paid request failed".into()))?;
            out.say(format!(
                "Pulse {:>3}  bob, a person without a charter, pays the same API with a plain transfer → 200 OK",
                settled
                    .extensions
                    .as_ref()
                    .and_then(|e| e["fold"]["pulse"].as_u64())
                    .unwrap_or(0)
            ));
            Ok(())
        })();
        stop.store(true, Ordering::Relaxed);
        let _ = serving.join();
        result
    });
    result?;
    out.say("Result: no accounts, no API keys; every request is paid and proven before it is served, and the agent's limits live in the ledger, not in its prompt.");
    Ok(())
}

fn session_chain(session: &Session<'_>) -> Result<fold_core::hash::Hash32, CliError> {
    crate::verify::served_chain(session.ledger)
}

/// Runs a named demo (`invoice`, `agents`, `market`, `receipts`, `x402`,
/// `light`, `censorship`, `all`).
pub fn run(name: &str, session: &mut Session<'_>, out: &mut Narrator) -> Result<(), CliError> {
    match name {
        "invoice" => invoice(session, out),
        "agents" => agents(session, out),
        "market" => market(session, out),
        "receipts" => receipts(session, out),
        "light" => light(session, out),
        "censorship" => censorship(session, out),
        "x402" => x402(session, out),
        "all" => {
            invoice(session, out)?;
            out.say("");
            agents(session, out)?;
            out.say("");
            market(session, out)?;
            out.say("");
            receipts(session, out)?;
            out.say("");
            x402(session, out)?;
            out.say("");
            censorship(session, out)?;
            out.say("");
            light(session, out)
        }
        other => Err(CliError::Usage(format!(
            "unknown demo {other:?}; try invoice, agents, market, receipts, x402, censorship, light or all"
        ))),
    }
}
