//! Scripted demonstrations against a live ledger.
//!
//! Each demo drives real transactions through the full protocol — lane
//! certificates, consensus, Pulse execution, header certificates — and
//! narrates what the ledger established and why it can be trusted.

use std::time::{Duration, Instant};

use serde_json::Value;
use sion_assurance::{atom::standard, AtomClass, Formula, Policy};
use sion_clearing::Side;
use sion_core::hash::hash_domain;
use sion_core::ids::{AssetId, ClaimId, ObjectId, Pulse};
use sion_core::{Amount, Cost};
use sion_exec::charter::Caveats;
use sion_exec::claim::{claim_id, formulas, ClaimBody, Payload};
use sion_exec::execute::bond_atom;
use sion_exec::market::{BatchStatus, OrderTerms, Submission};
use sion_exec::{Event, ExecStatus, ObjectData, TxKind};
use sion_node::view::TxView;

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

/// `n` with thousands separators.
fn grouped(n: u64) -> String {
    Amount::units(n).to_string()
}

/// A transaction's outcome in a few words.
fn outcome(status: &ExecStatus) -> String {
    match status {
        ExecStatus::Success => "accepted".into(),
        ExecStatus::Failed(reason) => format!("failed ({reason})"),
        ExecStatus::Rejected(reason) => format!("rejected ({reason})"),
    }
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
    let coc: sion_assurance::CocInterval =
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

fn charter_caveats(vendor: sion_core::ids::AccountId) -> Caveats {
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
        "Pulse {:>3}  agent pays gpu-cloud 80 USD for inference: {}; receipt claim {} joins the knowledge log",
        view.pulse,
        outcome(&view.status),
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
        "Pulse {:>3}  sub-agent pays gpu-cloud 20 USD: {}",
        view.pulse,
        outcome(&view.status)
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
            &serde_json::from_value::<sion_assurance::CocInterval>(
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
            "Pulse {:>3}  {who} {} {quantity} GPU-hours at {} {}.{:02} USD: {}",
            view.pulse,
            match side {
                Side::Buy => "bids for",
                Side::Sell => "offers",
            },
            match side {
                Side::Buy => "≤",
                Side::Sell => "≥",
            },
            price / 100,
            price % 100,
            outcome(&view.status)
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
    let result = sion_clearing::clear(&batch).map_err(|e| CliError::Build(e.to_string()))?;
    let Some(band) = result.band else {
        return Err(CliError::Transaction(format!(
            "batch {index} sealed with {} of the 4 orders and nothing crosses (the orders straddled a batch boundary)",
            batch.orders.len()
        )));
    };
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
        subject: "model open-llm-7b answered prompt H(p) with answer H(a)".into(),
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
    let coc: sion_assurance::CocInterval =
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
    // own status, which is what `sion devnet` prints at startup.
    let validators = session.ledger.query(sion_node::Query::Validators)?;
    let chain: sion_light::ChainSpec = serde_json::from_value(validators["chain"].clone())
        .map_err(|e| CliError::Protocol(e.to_string()))?;
    let pinned = chain.id();
    out.say(format!(
        "           pinned chain identity {} (genesis, validators, keys and shard layout)",
        pinned.short()
    ));
    // Something to verify, even on a fresh devnet: one transfer, in a Pulse
    // that is certified and, if the devnet proves, proven.
    fund_all(session, &[("light-user", AssetId::usd(), usd(10))])?;
    let alice = session.id("alice");
    let view = session.must(
        "light-user",
        TxKind::Transfer {
            asset: AssetId::usd(),
            outputs: vec![(alice, usd(1))],
        },
        Vec::new(),
    )?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let proven = loop {
        let status = session.ledger.status()?;
        let certified: Pulse = json(&status["certified_pulse"]).unwrap_or_default();
        let proven: Option<Pulse> = json(&status["proven_pulse"]).unwrap_or(None);
        if certified >= view.pulse && proven.is_some_and(|p| p >= view.pulse) {
            break true;
        }
        if Instant::now() > deadline {
            if certified < view.pulse {
                return Err(CliError::Transaction(format!(
                    "Pulse {} was not certified in time",
                    view.pulse
                )));
            }
            break false;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    if !proven {
        out.say(
            "           (this devnet does not prove, or not yet: the World Proof check is skipped)",
        );
    }
    let report =
        crate::verify::verify_pulse(session.ledger, Some(pinned), Some(view.pulse), 16, proven)?;
    for line in report {
        out.say(format!("           {line}"));
    }
    if !chain.insecure_signatures {
        // What a lying node would have to do to fool the client: serve its
        // own keys under a real validator's identifier. The pin catches it.
        let mut forged = chain.clone();
        let attacker =
            sion_crypto::DualKeypair::from_seed(&[0xBA; 32], sion_crypto::Scheme::SlhDsaSha2_128f);
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
    let window = session.ledger.query(sion_node::Query::Forced)?["window"]
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
        let forced = session.ledger.query(sion_node::Query::Forced)?;
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
    // The state's response: the shard's committee is replaced.
    let rotated = session
        .ledger
        .events(posted, 10_000)?
        .into_iter()
        .find_map(|view| match view.event {
            Event::ShardRotated { shard, rotation } => Some((view.pulse, shard, rotation)),
            _ => None,
        });
    if let Some((pulse, shard, rotation)) = rotated {
        let validators = session.ledger.query(sion_node::Query::Validators)?;
        let active = validators["committee"]["members"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0);
        let seats = validators["params"]["seats"].as_u64().unwrap_or(0) as usize;
        out.say(format!(
            "Pulse {pulse:>3}  the state replaces shard {shard}'s committee (rotation {rotation}), drawing from validators outside it first{}",
            if active <= seats {
                format!("; this devnet has {active} validators for {seats} seats, so the same members return and the intent stays owed")
            } else {
                String::new()
            }
        ));
    }
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
                .map(|e| e["sion"]["pulse"].clone());
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
                    .and_then(|e| e["sion"]["pulse"].as_u64())
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

/// Streams: an agent pays an API a tenth of a cent per call from a payment
/// stream. Each payment is one hash-chain element that the API checks with
/// one hash; the API settles thousands in a few transactions, which every
/// validator checks with one hash per payment; and anyone holding a call's
/// receipt can check it against the settled head.
pub fn streams(session: &mut Session<'_>, out: &mut Narrator) -> Result<(), CliError> {
    use std::sync::atomic::{AtomicBool, Ordering};

    use crate::stream::{
        load_stream, standing, standing_by_jump, PayerBook, PayerRecord, Payment, Standing,
        StreamDesk,
    };
    use crate::x402::{
        decode_header, fetch, fetch_with, Facilitator, PaidRoute, PaidServer, Payer, PaymentPayload,
    };

    const CALLS: u64 = 2_000;
    const STEPS: u64 = 100_000;
    out.say("== Streams: an agent pays an API a tenth of a cent per call, one hash per payment ==");
    fund_all(
        session,
        &[
            ("stream-agent", AssetId::usd(), usd(30)),
            ("carol", AssetId::usd(), usd(10)),
        ],
    )?;
    let chain = session_chain(session)?;
    let carol = session.id("carol");
    let unit = Amount(1_000);
    let spacing = 100;
    let (seed, payer_chain) = PayerBook::new_chain(STEPS, spacing);
    let until = Pulse(session.ledger.pulse()?.0 + 2_000);
    let view = session.must(
        "stream-agent",
        TxKind::OpenStream {
            payee: carol,
            asset: AssetId::usd(),
            unit,
            length: STEPS,
            anchor: payer_chain.anchor(),
            deposit: usd(20),
            until,
            checkpoints: payer_chain.checkpoints(),
        },
        Vec::new(),
    )?;
    let stream = find(&view, |e| match e {
        Event::StreamOpened { stream, .. } => Some(*stream),
        _ => None,
    })?;
    session.streams.add(
        PayerRecord {
            stream,
            wallet: "stream-agent".into(),
            payee: carol,
            unit,
            length: STEPS,
            seed,
            revealed: 0,
            spacing,
            announced: None,
        },
        payer_chain,
    )?;
    out.say(format!(
        "Pulse {:>3}  the agent opens a stream to carol: 20 USD in escrow, {unit} USD a step, {} steps; carol may settle until Pulse {until}",
        view.pulse,
        grouped(STEPS)
    ));

    let mut desk = StreamDesk::new(crate::session::wallet_for("carol"));
    desk.settle_every = 500;
    let facilitator = Facilitator::new(session.ledger, chain)?.with_streams(desk);
    let server = PaidServer::bind("127.0.0.1:0")?;
    let url = format!("http://{}/v1/price", server.address);
    let routes = vec![PaidRoute {
        path: "/v1/price".into(),
        price: unit,
        description: "Spot price of a GPU-hour".into(),
        mime_type: "application/json".into(),
        body: Box::new(|_| r#"{"good":"gpu-hour","usd":1.95}"#.to_string()),
    }];
    out.say(format!(
        "           carol's API at {url}: {unit} USD per call, paid by `exact` (a transaction per call) or `stream` (a hash per call)"
    ));
    let stop = AtomicBool::new(false);
    let (receipt, per_call_fee, elapsed) = std::thread::scope(|scope| {
        let serving = scope.spawn(|| {
            server.serve(
                &facilitator,
                carol,
                &routes,
                Duration::from_secs(600),
                &stop,
            )
        });
        let result = (|| -> Result<(Payment, Amount, Duration), CliError> {
            // One call paid the usual way, for comparison.
            let fetched = fetch(
                session,
                &Payer::Wallet("stream-agent".into()),
                &url,
                &chain,
                usd(1),
            )?;
            let settled = fetched
                .settlement
                .as_ref()
                .filter(|s| s.success && fetched.status == 200)
                .ok_or_else(|| CliError::Transaction("the exact payment failed".into()))?;
            let digest = sion_core::ids::TxDigest::from_hash(
                sion_core::hash::Hash32::from_hex(&settled.transaction)
                    .map_err(|_| CliError::Protocol("bad digest".into()))?,
            );
            let per_call_fee = match session.ledger.tx(digest)? {
                crate::ledger::TxState::Executed(view) => view.effects.fee,
                _ => return Err(CliError::Protocol("the exact payment vanished".into())),
            };
            out.say(format!(
                "           one call paid with a transfer: a transaction, certified and proven, {per_call_fee} USD in fees"
            ));
            // Then the stream: the first call learns the price, the rest pay
            // up front.
            let started = Instant::now();
            let mut known = None;
            let mut receipt = None;
            for call in 1..=CALLS {
                let fetched = fetch_with(
                    session,
                    &Payer::Stream(stream),
                    &url,
                    &chain,
                    usd(1),
                    known.as_ref(),
                )?;
                if fetched.status != 200 {
                    return Err(CliError::Transaction(format!(
                        "call {call} was refused: {:?}",
                        fetched.settlement.and_then(|s| s.error_reason)
                    )));
                }
                if call == 1_234 {
                    let header = fetched.payment.as_deref().unwrap_or_default();
                    let paid: PaymentPayload = decode_header(header)?;
                    receipt = Some(
                        serde_json::from_value::<Payment>(paid.payload)
                            .map_err(|e| CliError::Protocol(e.to_string()))?,
                    );
                }
                known = fetched.accepted;
            }
            let receipt = receipt.ok_or_else(|| CliError::Protocol("no receipt kept".into()))?;
            Ok((receipt, per_call_fee, started.elapsed()))
        })();
        // The server settles whatever is left before it stops.
        stop.store(true, Ordering::Relaxed);
        let _ = serving.join();
        result
    })?;
    let report = facilitator
        .streams()
        .map(|desk| desk.report())
        .unwrap_or_default();
    out.say(format!(
        "           {} calls paid from the stream in {:.1} s: carol's server checked {} hashes and sent no transaction per call",
        grouped(CALLS),
        elapsed.as_secs_f64(),
        grouped(report.hashes)
    ));
    let per_payment = report.fees.0 as f64 / report.settled_steps.max(1) as f64;
    out.say(format!(
        "           carol settled {} payments in {} transactions: {} USD received, {} USD in fees ({per_payment:.2} micro-USD per payment, against {} for a transaction per call)",
        grouped(report.settled_steps),
        report.settlements,
        report.settled_value,
        report.fees,
        grouped(u64::try_from(per_call_fee.0).unwrap_or(u64::MAX))
    ));
    let state = load_stream(session.ledger, &stream)?;
    out.say(format!(
        "           the validators checked {} hashes for those settlements ({} proven by a jump to a checkpoint), not one per payment; the stream is at position {} with {} USD left in escrow",
        grouped(report.ledger_hashes),
        report.jumps,
        grouped(state.position),
        state.escrow
    ));
    let walked = match standing(&state, &receipt) {
        Standing::Settled { hashes } => hashes,
        other => {
            return Err(CliError::Protocol(format!(
                "the receipt should be settled: {other:?}"
            )))
        }
    };
    let jump = session
        .streams
        .jump(&stream, receipt.position / spacing)?
        .ok_or_else(|| CliError::Protocol("no checkpoint for the receipt".into()))?;
    let jumped = match standing_by_jump(&state, &receipt, &jump) {
        Standing::Settled { hashes } => hashes,
        other => {
            return Err(CliError::Protocol(format!(
                "the receipt should be settled by its checkpoint too: {other:?}"
            )))
        }
    };
    out.say(format!(
        "           an auditor checks call {}'s receipt: genuine and settled, in {walked} hashes from the settled head, or {jumped} from the checkpoint below it",
        grouped(receipt.position)
    ));
    let forged = Payment {
        value: hash_domain("demo/forged", b"receipt"),
        ..receipt
    };
    if standing(&state, &forged) != Standing::Forged {
        return Err(CliError::Protocol("a forged receipt passed".into()));
    }
    out.say("           a forged receipt for the same call → not an element of the stream's chain");
    let view = session.must("carol", TxKind::CloseStream { stream }, Vec::new())?;
    let refund = find(&view, |e| match e {
        Event::StreamClosed { refund, .. } => Some(*refund),
        _ => None,
    })?;
    out.say(format!(
        "Pulse {:>3}  carol closes the stream; the agent gets {refund} USD back",
        view.pulse
    ));
    out.say("Result: each payment costs the server one hash, not a signature; a settlement costs the ledger the same however many payments it covers; and every payment stays checkable by anyone.");
    Ok(())
}

fn json<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T, CliError> {
    serde_json::from_value(value.clone()).map_err(|e| CliError::Protocol(e.to_string()))
}

/// Waits until the state reaches `epoch`; returns the validator set then.
/// An epoch can be many Pulses away on a slow machine, so this gives up only
/// when the chain stops making progress for the session's timeout.
fn wait_for_epoch(session: &Session<'_>, epoch: u64) -> Result<Value, CliError> {
    let mut deadline = Instant::now() + session.timeout;
    let mut last = Pulse(0);
    loop {
        let validators = session.ledger.query(sion_node::Query::Validators)?;
        if validators["epoch"].as_u64().unwrap_or(0) >= epoch {
            return Ok(validators);
        }
        let pulse: Pulse = json(&validators["pulse"]).unwrap_or(last);
        if pulse > last {
            last = pulse;
            deadline = Instant::now() + session.timeout;
        }
        if Instant::now() > deadline {
            return Err(CliError::Transaction(format!(
                "epoch {epoch} did not begin: the chain stalled at Pulse {last}"
            )));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The `EpochChanged` event that began `epoch`, with its Pulse.
fn epoch_event(
    session: &Session<'_>,
    from: Pulse,
    epoch: u64,
) -> Result<(Pulse, u32, Amount), CliError> {
    for view in session.ledger.events(from, 10_000)? {
        if let Event::EpochChanged {
            epoch: e,
            active,
            rewards,
        } = view.event
        {
            if e == epoch {
                return Ok((view.pulse, active, rewards));
            }
        }
    }
    Err(CliError::Protocol(format!("no event began epoch {epoch}")))
}

/// The first certified Pulse at or after `from`.
fn certified_from(
    session: &Session<'_>,
    from: Pulse,
) -> Result<sion_light::PulseCertificate, CliError> {
    let deadline = Instant::now() + session.timeout;
    let mut pulse = from;
    loop {
        if let Ok(view) = session.ledger.pulse_view(Some(pulse)) {
            if let Some(certificate) = view.certificate {
                return Ok(certificate);
            }
        }
        if session.ledger.pulse()? > pulse.next() {
            // Certificates complete in order; try the next one.
            pulse = pulse.next();
        }
        if Instant::now() > deadline {
            return Err(CliError::Transaction(format!(
                "no certified Pulse from {from} on"
            )));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Staking: a standby validator stakes and joins the active set at the next
/// epoch boundary, every validator derives the new committees from the
/// state, rewards are paid by stake, an equivocating validator is slashed
/// and loses its seat, and a light client follows the committee from the
/// pinned genesis through every change.
pub fn staking(session: &mut Session<'_>, out: &mut Narrator) -> Result<(), CliError> {
    out.say("== Staking: committees come from the state, epoch by epoch ==");
    let threads = session.ledger.status()?["validators"].as_u64().unwrap_or(0) as usize;
    let validators = session.ledger.query(sion_node::Query::Validators)?;
    let params: sion_exec::validators::ValidatorParams = json(&validators["params"])?;
    let chain_id = session.config.chain_id.clone();
    let registered: Vec<sion_core::ids::ValidatorId> = validators["validators"]
        .as_array()
        .map(|records| {
            records
                .iter()
                .filter_map(|r| json(&r["validator"]).ok())
                .collect()
        })
        .unwrap_or_default();
    let standby = (0..threads)
        .find(|index| !registered.contains(&sion_node::devnet::validator_id(&chain_id, *index)));
    let Some(index) = standby.filter(|_| params.epoch_length <= 10_000) else {
        out.say(
            "           this devnet has no standby validator, or runs one epoch that never ends;",
        );
        out.say("           start one with `sion devnet --standby 1 --epoch-length 24` to watch validators join and leave");
        return Ok(());
    };
    let spec: sion_light::ChainSpec = json(&validators["chain"])?;
    let epoch = validators["epoch"].as_u64().unwrap_or(0);
    let epoch_end: Pulse = json(&validators["epoch_end"])?;
    let active = validators["committee"]["members"]
        .as_array()
        .map(Vec::len)
        .unwrap_or(0);
    let operator = sion_node::devnet::operator_name(index);
    let newcomer = sion_node::devnet::validator_id(&chain_id, index);
    let genesis_stake: Amount = json(&validators["validators"][0]["stake"])?;
    out.say(format!(
        "           epoch {epoch}: {active} validators hold seats, each staking {genesis_stake} USD; validator {} runs on standby, following the chain without a seat",
        newcomer.short()
    ));
    out.say(format!(
        "           epochs last {} Pulses; this one ends after Pulse {epoch_end}",
        params.epoch_length
    ));

    // 1. The standby validator's operator stakes for it.
    let stake = usd(50_000);
    fund_all(
        session,
        &[
            (operator.as_str(), AssetId::usd(), usd(50_010)),
            ("watchdog", AssetId::usd(), usd(10)),
        ],
    )?;
    let operator_id = session.id(&operator);
    let kind = sion_node::devnet::registration(&session.config, index, &operator_id, stake);
    let view = session.must(&operator, kind, Vec::new())?;
    let names = session.names();
    out.say(format!(
        "Pulse {:>3}  {operator} stakes {} USD for validator {}, with a proof that it holds the validator's keys",
        view.pulse,
        stake,
        newcomer.short()
    ));
    out.say(format!(
        "           {}",
        crate::describe::receipt(&names, &view.effects)
    ));

    // 2. The boundary: rewards, the new active set, new committees.
    let mut next = wait_for_epoch(session, epoch + 1)?;
    let mut current = next["epoch"].as_u64().unwrap_or(epoch + 1);
    // If the registration landed after the boundary, it joins at the next.
    if !next["committee"]["members"].as_array().is_some_and(|m| {
        m.iter()
            .any(|v| json::<sion_core::ids::ValidatorId>(&v[0]).ok() == Some(newcomer))
    }) {
        next = wait_for_epoch(session, current + 1)?;
        current = next["epoch"].as_u64().unwrap_or(current + 1);
    }
    let (boundary, seats, rewards) =
        epoch_event(session, Pulse(epoch_end.0.saturating_sub(1)), current)?;
    out.say(format!(
        "Pulse {:>3}  epoch {current} begins: {seats} validators hold seats, the newcomer among them; {} USD of fees paid as rewards, split by stake",
        boundary, rewards
    ));
    let lanes = next["shards"].as_object().map(|m| m.len()).unwrap_or(0);
    out.say(format!(
        "           every validator derived the same committees from the state: consensus (weighted by stake) and {lanes} lane committees sampled with the beacon"
    ));

    // 3. A light client follows the change from the pinned genesis.
    let certificate = certified_from(session, boundary.next())?;
    let followed = crate::verify::epoch_chain(session.ledger, &spec, certificate.header.epoch)?;
    let signers = certificate.signers();
    let committee = followed
        .committee(certificate.header.epoch)
        .ok_or_else(|| CliError::Verification("the client lost the epoch".into()))?;
    match followed
        .verify_header(&certificate)
        .map_err(|e| CliError::Verification(e.to_string()))?
    {
        Some((signed, quorum)) => out.say(format!(
            "           light client: followed the committee from genesis through {} boundary proof(s); Pulse {}'s certificate has {} of {} validators, {signed} of {} stake (quorum {quorum}) ✓",
            followed.latest(),
            certificate.header.pulse,
            signers.len(),
            committee.size(),
            committee.total_stake()
        )),
        None => out.say(format!(
            "           light client: followed the committee through {} boundary proof(s) (insecure simulation signatures: not checked)",
            followed.latest()
        )),
    }

    // 4. Rewards are claimed by the operator.
    let rewarded = next["validators"].as_array().and_then(|records| {
        records.iter().position(|r| {
            json::<Amount>(&r["rewards"]).is_ok_and(|a| a > Amount::ZERO)
                && json::<sion_core::ids::ValidatorId>(&r["validator"]).ok() != Some(newcomer)
        })
    });
    if let Some(position) = rewarded {
        let record = &next["validators"][position];
        let validator: sion_core::ids::ValidatorId = json(&record["validator"])?;
        let owner: sion_core::ids::AccountId = json(&record["operator"])?;
        let owner_name = (0..threads)
            .map(sion_node::devnet::operator_name)
            .find(|name| sion_exec::Wallet::named(name).id == owner);
        if let Some(owner_name) = owner_name {
            fund_all(session, &[(owner_name.as_str(), AssetId::usd(), usd(1))])?;
            let view = session.must(&owner_name, TxKind::ClaimRewards { validator }, Vec::new())?;
            out.say(format!(
                "Pulse {:>3}  {owner_name} collects: {}",
                view.pulse,
                crate::describe::receipt(&session.names(), &view.effects)
            ));
        }
    }

    // 5. The newcomer equivocates and is slashed.
    let session_number = next["session"].as_u64().unwrap_or(0);
    let evidence = sion_node::devnet::equivocation(
        &chain_id,
        spec.id(),
        index,
        session_number,
        1_000_000 + current,
    );
    out.say(format!(
        "           ADVERSARY SIMULATION: validator {} signs two different consensus headers for the same round",
        newcomer.short()
    ));
    let view = session.must(
        "watchdog",
        TxKind::ReportEquivocation {
            evidence: Box::new(evidence),
        },
        Vec::new(),
    )?;
    out.say(format!(
        "Pulse {:>3}  watchdog reports both signed headers: {}",
        view.pulse,
        crate::describe::receipt(&session.names(), &view.effects)
    ));
    let after = wait_for_epoch(session, current + 1)?;
    let still = after["committee"]["members"].as_array().is_some_and(|m| {
        m.iter()
            .any(|v| json::<sion_core::ids::ValidatorId>(&v[0]).ok() == Some(newcomer))
    });
    let seats = after["committee"]["members"]
        .as_array()
        .map(Vec::len)
        .unwrap_or(0);
    out.say(format!(
        "           epoch {} begins with {seats} validators: the slashed validator {} its seat",
        after["epoch"],
        if still { "kept" } else { "lost" }
    ));
    if still {
        return Err(CliError::Verification(
            "a slashed validator kept its seat".into(),
        ));
    }
    let followed =
        crate::verify::epoch_chain(session.ledger, &spec, after["epoch"].as_u64().unwrap_or(0))?;
    out.say(format!(
        "           light client: still following, {} boundary proof(s) from the pinned genesis ✓",
        followed.latest()
    ));
    out.say("Result: who certifies is part of the state transition, proven like any balance; stake buys seats, misbehaviour is evidence anyone can submit, and a client needs nothing but the genesis it pinned.");
    Ok(())
}

fn session_chain(session: &Session<'_>) -> Result<sion_core::hash::Hash32, CliError> {
    crate::verify::served_chain(session.ledger)
}

/// Whether a demo needs a devnet with a standby validator and epochs that
/// end.
pub fn needs_epochs(name: &str) -> bool {
    name == "staking"
}

/// The in-process devnet `demo --local` starts for a demo.
pub fn local_config(name: &str) -> sion_node::DevnetConfig {
    let base = sion_node::DevnetConfig::default();
    if needs_epochs(name) {
        sion_node::DevnetConfig {
            standby: 1,
            epoch_length: 24,
            unbonding: 8,
            ..base
        }
    } else {
        base
    }
}

/// Every demo, in the order `all` runs them.
/// Agent identities (spec v1.8): an orchestrator launches a swarm of
/// subagents, each with its own identity, account and budget; the ledger
/// contains them, and one transaction from an overseer stops or resumes a
/// whole subtree.
pub fn swarm(session: &mut Session<'_>, out: &mut Narrator) -> Result<(), CliError> {
    use sion_exec::charter::{card_commitment, Spawn};
    const RESEARCHERS: usize = 12;
    const SUBS: usize = 3;
    out.say("== Agent identities: a swarm of subagents, each with its own identity ==");
    let researchers: Vec<String> = (1..=RESEARCHERS)
        .map(|i| format!("researcher-{i}"))
        .collect();
    let subs: Vec<String> = (1..=SUBS).map(|i| format!("sub-researcher-{i}")).collect();
    let mut grants = vec![
        ("acme", AssetId::usd(), usd(5_000)),
        ("orchestrator", AssetId::usd(), usd(10)),
    ];
    for name in researchers.iter().chain(&subs) {
        grants.push((name.as_str(), AssetId::usd(), usd(1)));
    }
    fund_all(session, &grants)?;
    let vendor = session.id("gpu-cloud");
    let orchestrator = session.id("orchestrator");
    let mut caveats = charter_caveats(vendor);
    caveats.max_per_payment = usd(25);
    caveats.max_per_window = usd(500);
    let card = card_commitment(
        br#"{"name":"acme research orchestrator","url":"https://agents.acme.example/a2a"}"#,
    );
    let view = session.must(
        "acme",
        TxKind::RegisterAgent {
            agent: orchestrator,
            card,
            caveats: caveats.clone(),
            funding: usd(1_000),
            agent_bond: usd(200),
        },
        Vec::new(),
    )?;
    let (root, root_allotment) = find(&view, |e| match e {
        Event::AgentRegistered {
            agent, allotment, ..
        } => Some((*agent, *allotment)),
        _ => None,
    })?;
    out.say(format!(
        "Pulse {:>3}  acme registers its orchestrator: agent {}, card {}, 1,000 USD budget, may launch subagents",
        view.pulse,
        root.short(),
        card.short()
    ));

    let mut worker = caveats.clone();
    worker.max_per_payment = usd(10);
    worker.max_per_window = usd(40);
    let children: Vec<Spawn> = researchers
        .iter()
        .map(|name| Spawn {
            agent: session.id(name),
            card: card_commitment(format!("{{\"name\":\"{name}\"}}").as_bytes()),
            caveats: worker.clone(),
            budget: usd(40),
        })
        .collect();
    let input = session.reference(&root_allotment)?;
    let view = session.must(
        "orchestrator",
        TxKind::SpawnAgents {
            charter: root,
            children,
        },
        vec![input],
    )?;
    let spawned: Vec<(ObjectId, ObjectId)> = events(&view)
        .iter()
        .filter_map(|e| match e {
            Event::AgentSpawned {
                agent, allotment, ..
            } => Some((*agent, *allotment)),
            _ => None,
        })
        .collect();
    out.say(format!(
        "Pulse {:>3}  the orchestrator launches {} researchers in one transaction: {} identities, {} accounts, 40 USD each, ≤ 10 USD per payment",
        view.pulse,
        spawned.len(),
        spawned.len(),
        spawned.len()
    ));

    let (lead, lead_allotment) = spawned[0];
    let mut leaf = worker.clone();
    leaf.max_per_payment = usd(5);
    leaf.max_per_window = usd(10);
    leaf.may_delegate = false;
    let children: Vec<Spawn> = subs
        .iter()
        .map(|name| Spawn {
            agent: session.id(name),
            card: card_commitment(format!("{{\"name\":\"{name}\"}}").as_bytes()),
            caveats: leaf.clone(),
            budget: usd(10),
        })
        .collect();
    let input = session.reference(&lead_allotment)?;
    let view = session.must(
        &researchers[0],
        TxKind::SpawnAgents {
            charter: lead,
            children,
        },
        vec![input],
    )?;
    let sub_agents: Vec<(ObjectId, ObjectId)> = events(&view)
        .iter()
        .filter_map(|e| match e {
            Event::AgentSpawned {
                agent, allotment, ..
            } => Some((*agent, *allotment)),
            _ => None,
        })
        .collect();
    out.say(format!(
        "Pulse {:>3}  researcher-1 launches {} sub-researchers of its own (depth 2), 10 USD each, ≤ 5 USD per payment",
        view.pulse,
        sub_agents.len()
    ));

    // Every subagent pays per tool call from its own allotment, all at once.
    let mut pending = Vec::new();
    for (name, (charter, allotment)) in researchers
        .iter()
        .zip(&spawned)
        .chain(subs.iter().zip(&sub_agents))
    {
        let input = session.reference(allotment)?;
        let amount = if name.starts_with("sub") {
            usd(4)
        } else {
            usd(8)
        };
        pending.push(session.submit(
            name,
            TxKind::AgentPay {
                charter: *charter,
                payee: vendor,
                amount,
                purpose: "a tool call".into(),
                evidence: None,
            },
            vec![input],
        )?);
    }
    let mut paid = 0;
    let mut last = Pulse(0);
    for digest in pending {
        let view = session.wait(digest)?;
        last = last.max(view.pulse);
        if view.status == ExecStatus::Success {
            paid += 1;
        }
    }
    session.settle_all();
    out.say(format!(
        "Pulse {:>3}  {paid} subagents each pay gpu-cloud for a tool call, all at once: no consensus, each spends only its own allotment",
        last.0
    ));

    let (sub, sub_allotment) = sub_agents[2];
    let pay = |session: &mut Session<'_>, amount: u64| {
        let input = session.reference(&sub_allotment)?;
        session.send(
            &subs[2],
            TxKind::AgentPay {
                charter: sub,
                payee: vendor,
                amount: usd(amount),
                purpose: "a bigger tool call".into(),
                evidence: None,
            },
            vec![input],
        )
    };
    let view = pay(session, 6)?;
    out.say(format!(
        "Pulse {:>3}  sub-researcher-3 tries to pay 6 USD, above its own limit → {}",
        view.pulse,
        refusal(&view)
    ));

    let view = session.must("acme", TxKind::SuspendAgent { charter: root }, Vec::new())?;
    out.say(format!(
        "Pulse {:>3}  acme suspends the orchestrator: one transaction",
        view.pulse
    ));
    let view = pay(session, 3)?;
    out.say(format!(
        "Pulse {:>3}  sub-researcher-3, two levels down, tries to pay 3 USD → {}",
        view.pulse,
        refusal(&view)
    ));
    let view = session.must("acme", TxKind::ResumeAgent { charter: root }, Vec::new())?;
    out.say(format!(
        "Pulse {:>3}  acme lifts the suspension",
        view.pulse
    ));
    let view = pay(session, 3)?;
    out.say(format!(
        "Pulse {:>3}  sub-researcher-3 pays 3 USD: {}",
        view.pulse,
        outcome(&view.status)
    ));

    let (second, _) = spawned[1];
    let view = session.must(
        "orchestrator",
        TxKind::RevokeCharter { charter: second },
        Vec::new(),
    )?;
    out.say(format!(
        "Pulse {:>3}  the orchestrator revokes researcher-2 for good",
        view.pulse
    ));
    let view = session.must(
        "acme",
        TxKind::ReclaimCharter { charter: second },
        Vec::new(),
    )?;
    let reclaimed = find(&view, |e| match e {
        Event::CharterReclaimed { amount, .. } => Some(*amount),
        _ => None,
    })?;
    out.say(format!(
        "Pulse {:>3}  acme reclaims researcher-2's unspent budget: {}",
        view.pulse,
        session.names().money(reclaimed, &AssetId::usd())
    ));

    let lineage = crate::agents::describe(session.ledger, sub)?;
    let mut chain = vec![sub.short()];
    if let Some(above) = lineage["lineage"].as_array() {
        for entry in above {
            if let Some(id) = entry["agent"].as_str() {
                chain.push(id.chars().take(8).collect());
            }
        }
    }
    out.say(format!(
        "            sub-researcher-3's lineage: {} → acme (depth {}, {})",
        chain.join(" → "),
        lineage["depth"],
        lineage["status"].as_str().unwrap_or("")
    ));
    out.say(format!(
        "            {} identities created in 3 transactions; every limit was checked by the ledger, not by the agents",
        1 + spawned.len() + sub_agents.len()
    ));
    Ok(())
}

/// Task Charters (spec v1.10): a lab buys evaluation work from others,
/// pays only for results that pass the rule it fixed when it posted the
/// work, and is compensated when the environment turns out to be
/// exploitable.
pub fn tasks(session: &mut Session<'_>, out: &mut Narrator) -> Result<(), CliError> {
    use sion_exec::task::{
        result_formula, Acceptance, TaskResult, ATTESTATION_SCHEMA, RESULT_SCHEMA,
        WORKER_BOND_LABEL,
    };
    out.say(
        "== Task Charters: paying for work only when the result passes a rule fixed in advance ==",
    );
    fund_all(
        session,
        &[
            ("lab", AssetId::usd(), usd(10_000)),
            ("eval-orchestrator", AssetId::usd(), usd(10)),
            ("sandbox-vendor", AssetId::usd(), usd(6_000)),
            ("coder", AssetId::usd(), usd(200)),
            ("bounty-hunter", AssetId::usd(), usd(10)),
            ("reviewer-1", AssetId::usd(), usd(1_100)),
            ("reviewer-2", AssetId::usd(), usd(1_100)),
            ("arbiter", AssetId::usd(), usd(3_100)),
            ("red-team", AssetId::usd(), usd(200)),
        ],
    )?;
    let (lab, orchestrator, vendor) = (
        session.id("lab"),
        session.id("eval-orchestrator"),
        session.id("sandbox-vendor"),
    );
    let (coder, hunter) = (session.id("coder"), session.id("bounty-hunter"));
    let (r1, r2) = (session.id("reviewer-1"), session.id("reviewer-2"));
    let env_atom = bond_atom(AtomClass::Economic, &vendor, "environment");
    let arbiter_atom = bond_atom(
        AtomClass::Adjudicator,
        &session.id("arbiter"),
        "arbitration",
    );

    for (who, class, label, amount) in [
        (
            "sandbox-vendor",
            AtomClass::Economic,
            "environment",
            usd(5_000),
        ),
        ("arbiter", AtomClass::Adjudicator, "arbitration", usd(3_000)),
        ("coder", AtomClass::Economic, WORKER_BOND_LABEL, usd(100)),
        (
            "bounty-hunter",
            AtomClass::Economic,
            WORKER_BOND_LABEL,
            Amount(1),
        ),
        ("reviewer-1", AtomClass::Economic, "verifier", usd(1_000)),
        ("reviewer-2", AtomClass::Economic, "verifier", usd(1_000)),
    ] {
        session.must(
            who,
            TxKind::PostBond {
                class,
                label: label.into(),
                amount,
            },
            Vec::new(),
        )?;
    }
    out.say("           bonds: sandbox-vendor 5,000 USD behind its environment, arbiter 3,000, reviewers 1,000 each, coder 100");

    let environment = ClaimBody {
        schema: "environment.no-known-exploit/v1".into(),
        subject:
            "swe-sandbox-7 has no known exploit that passes grader g-3 without solving the task"
                .into(),
        payload: Payload::Commitment(hash_domain("demo/environment", b"swe-sandbox-7")),
        formula: formulas::bonded_statement(env_atom.id),
        adjudicator: Some(arbiter_atom.id),
        detection_permille: 1000,
        valid_for: 5_000,
        challenge_period: 2_000,
    };
    let env_claim = claim_id(&vendor, &environment);
    let view = session.must(
        "sandbox-vendor",
        TxKind::IssueClaim { claim: environment },
        Vec::new(),
    )?;
    out.say(format!(
        "Pulse {:>3}  sandbox-vendor claims \"swe-sandbox-7 has no known exploit that passes grader g-3\"  [backed by its 5,000 USD bond]",
        view.pulse
    ));

    let mut caveats = charter_caveats(lab);
    caveats.payees = None;
    caveats.max_per_payment = usd(500);
    caveats.max_per_window = usd(1_000);
    let view = session.must(
        "lab",
        TxKind::RegisterAgent {
            agent: orchestrator,
            card: sion_exec::charter::card_commitment(br#"{"name":"lab eval orchestrator"}"#),
            caveats,
            funding: usd(1_000),
            agent_bond: Amount::ZERO,
        },
        Vec::new(),
    )?;
    let (agent, allotment) = find(&view, |e| match e {
        Event::AgentRegistered {
            agent, allotment, ..
        } => Some((*agent, *allotment)),
        _ => None,
    })?;
    out.say(format!(
        "Pulse {:>3}  lab registers its eval orchestrator with a 1,000 USD budget, at most 500 USD per task",
        view.pulse
    ));
    session.wait_pulses(1)?;

    let acceptance = |verifiers: &[sion_core::ids::AccountId]| Acceptance {
        policy: Policy::standard(),
        requires: [env_claim].into_iter().collect(),
        verifiers: verifiers.iter().copied().collect(),
        adjudicator: Some(arbiter_atom.id),
        detection_permille: 900,
        reliance: 2_000,
    };
    let deadline = Pulse(session.ledger.pulse()?.0 + 2_000);
    let post = |session: &mut Session<'_>,
                acceptance: Acceptance,
                worker: Option<sion_core::ids::AccountId>,
                amount: Amount|
     -> Result<(ObjectId, Pulse), CliError> {
        let input = session.reference(&allotment)?;
        let view = session.must(
            "eval-orchestrator",
            TxKind::PostTask {
                spec: hash_domain("demo/task", b"fix issue #4711 in the parser"),
                acceptance,
                worker,
                amount,
                deadline,
                charter: Some(agent),
            },
            vec![input],
        )?;
        let task = find(&view, |e| match e {
            Event::TaskPosted { task, .. } => Some(*task),
            _ => None,
        })?;
        Ok((task, view.pulse))
    };
    let (t1, pulse) = post(session, acceptance(&[r1, r2]), Some(coder), usd(150))?;
    out.say(format!(
        "Pulse {pulse:>3}  the orchestrator posts task {}: 150 USD escrowed for coder, from its budget. Fixed now: a result must rest on",
        t1.short()
    ));
    out.say("           sandbox-vendor's claim, may be attested by reviewer-1 or reviewer-2, and must pass the lab's policy at κ = 2");
    let (t2, pulse) = post(session, acceptance(&[r2]), None, usd(40))?;
    out.say(format!(
        "Pulse {pulse:>3}  …and task {}: a 40 USD bounty open to anyone, on the same terms",
        t2.short()
    ));

    // The work, and what it rests on.
    let payload = |task, output: &[u8]| {
        TaskResult {
            task,
            output: hash_domain("demo/output", output),
        }
        .payload()
    };
    let claim = |schema: &str, payload: Payload, formula, detection| ClaimBody {
        schema: schema.into(),
        subject: "patch passes grader g-3".into(),
        payload,
        formula,
        adjudicator: Some(arbiter_atom.id),
        detection_permille: detection,
        valid_for: 5_000,
        challenge_period: 2_000,
    };
    let verifier_formula = |who: &sion_core::ids::AccountId| {
        formulas::bonded_statement(bond_atom(AtomClass::Economic, who, "verifier").id)
    };
    let a1 = claim(
        ATTESTATION_SCHEMA,
        payload(t1, b"patch-4711"),
        verifier_formula(&r1),
        1000,
    );
    let a2 = claim(
        ATTESTATION_SCHEMA,
        payload(t1, b"patch-4711"),
        verifier_formula(&r2),
        1000,
    );
    let a3 = claim(
        ATTESTATION_SCHEMA,
        payload(t2, b"patch-88"),
        verifier_formula(&r2),
        1000,
    );
    let (a1_id, a2_id, a3_id) = (claim_id(&r1, &a1), claim_id(&r2, &a2), claim_id(&r2, &a3));
    let requires: std::collections::BTreeSet<ClaimId> = [env_claim].into_iter().collect();
    let alone = claim(
        RESULT_SCHEMA,
        payload(t1, b"patch-4711"),
        result_formula(&coder, &[], &requires),
        900,
    );
    let attested = claim(
        RESULT_SCHEMA,
        payload(t1, b"patch-4711"),
        result_formula(&coder, &[a1_id, a2_id], &requires),
        900,
    );
    let bounty = claim(
        RESULT_SCHEMA,
        payload(t2, b"patch-88"),
        result_formula(&hunter, &[a3_id], &requires),
        900,
    );
    for (who, body) in [
        ("reviewer-1", a1.clone()),
        ("reviewer-2", a2.clone()),
        ("reviewer-2", a3.clone()),
        ("coder", alone.clone()),
    ] {
        session.must(who, TxKind::IssueClaim { claim: body }, Vec::new())?;
    }
    session.wait_pulses(1)?;
    for (who, body) in [
        ("coder", attested.clone()),
        ("bounty-hunter", bounty.clone()),
    ] {
        session.must(who, TxKind::IssueClaim { claim: body }, Vec::new())?;
    }
    session.wait_pulses(1)?;
    out.say("           coder submits patch-4711; reviewer-1 and reviewer-2 each attest that it passes, on their own bonds");

    let settle = |session: &mut Session<'_>,
                  who: &str,
                  task,
                  result: &ClaimBody,
                  attestations: Vec<(sion_core::ids::AccountId, ClaimBody)>| {
        session.send(
            who,
            TxKind::SettleTask {
                task,
                result: result.clone(),
                attestations,
            },
            Vec::new(),
        )
    };
    let view = settle(session, "coder", t1, &alone, Vec::new())?;
    out.say(format!(
        "Pulse {:>3}  coder claims the 150 USD on its own 100 USD bond → {}",
        view.pulse,
        refusal(&view)
    ));
    let before = session.balance("coder", AssetId::usd())?;
    let view = settle(
        session,
        "coder",
        t1,
        &attested,
        vec![(r1, a1.clone()), (r2, a2.clone())],
    )?;
    let paid = session.balance("coder", AssetId::usd())?;
    out.say(format!(
        "Pulse {:>3}  coder settles with both attestations: cost of corruption 2,100 USD (100 + 1,000 + 1,000) covers κ·V = 300 → paid {} USD; 300 USD of that backing reserved for the lab",
        view.pulse,
        Amount(paid.0.saturating_sub(before.0).saturating_add(view.effects.fee.0))
    ));
    let view = settle(
        session,
        "coder",
        t1,
        &attested,
        vec![(r1, a1.clone()), (r2, a2.clone())],
    )?;
    out.say(format!(
        "Pulse {:>3}  coder settles again → {}: a task is paid once",
        view.pulse,
        refusal(&view)
    ));
    let view = settle(
        session,
        "bounty-hunter",
        t2,
        &bounty,
        vec![(r2, a3.clone())],
    )?;
    out.say(format!(
        "Pulse {:>3}  bounty-hunter settles the open bounty with reviewer-2's attestation → {}",
        view.pulse,
        match &view.status {
            ExecStatus::Success => "paid 40 USD".to_string(),
            _ => refusal(&view),
        }
    ));

    // A bounty nobody took is withdrawn.
    let (t3, _) = post(session, acceptance(&[]), None, usd(30))?;
    let view = session.must(
        "eval-orchestrator",
        TxKind::CancelTask { task: t3 },
        Vec::new(),
    )?;
    let refund = find(&view, |e| match e {
        Event::TaskCancelled { refund, .. } => Some(*refund),
        _ => None,
    })?;
    out.say(format!(
        "Pulse {:>3}  the orchestrator withdraws an unclaimed bounty: {refund} USD back to the lab",
        view.pulse
    ));

    // The exploit.
    let view = session.must(
        "red-team",
        TxKind::Challenge {
            claim: env_claim,
            target: env_atom.id,
            evidence: hash_domain(
                "demo/exploit",
                b"grader g-3 accepts any patch that deletes the failing test",
            ),
            deposit: usd(100),
        },
        Vec::new(),
    )?;
    let challenge = find(&view, |e| match e {
        Event::ChallengeOpened { challenge, .. } => Some(*challenge),
        _ => None,
    })?;
    out.say(format!(
        "Pulse {:>3}  red-team shows that grader g-3 accepts any patch that deletes the failing test, and challenges the vendor's claim",
        view.pulse
    ));
    let lab_before = session.balance("lab", AssetId::usd())?;
    let red_before = session.balance("red-team", AssetId::usd())?;
    let view = session.must(
        "arbiter",
        TxKind::Rule {
            challenge,
            upheld: true,
        },
        Vec::new(),
    )?;
    let falsified = events(&view)
        .iter()
        .filter(|e| matches!(e, Event::Falsified { .. }))
        .count();
    let compensated: u128 = events(&view)
        .iter()
        .filter_map(|e| match e {
            Event::Compensated {
                consumer, amount, ..
            } if *consumer == lab => Some(amount.0),
            _ => None,
        })
        .sum();
    session.wait_pulses(1)?;
    let lab_after = session.balance("lab", AssetId::usd())?;
    let red_after = session.balance("red-team", AssetId::usd())?;
    out.say(format!(
        "Pulse {:>3}  arbiter upholds it: the vendor's bond is slashed, and {falsified} claims fall: the environment's, and the {} results resting on it, paid or not",
        view.pulse,
        falsified.saturating_sub(1)
    ));
    out.say(format!(
        "           the lab is compensated {} USD from the vendor's bond for the two results it paid for (lab +{} USD)",
        Amount(compensated),
        Amount(lab_after.0.saturating_sub(lab_before.0))
    ));
    out.say(format!(
        "           red-team gets its deposit back and a share of the slashed bond: +{} USD, a bounty for the reward hack paid by the vendor",
        Amount(red_after.0.saturating_sub(red_before.0))
    ));
    Ok(())
}

/// Commits `memory`'s whole log to its ledger object `log`, with a proof
/// that it extends the head the ledger holds; records the commit.
fn commit_memory(
    session: &mut Session<'_>,
    writer: &str,
    log: ObjectId,
    memory: &mut sion_memory::Memory,
    history: &mut sion_memory::CommitHistory,
    index: Option<sion_core::hash::Hash32>,
) -> Result<TxView, CliError> {
    let on_chain = crate::memory::committed(session.ledger, &log)?;
    let head = memory.head();
    let proof = memory
        .log()
        .consistency_proof(on_chain.size)
        .map_err(|e| CliError::Protocol(e.to_string()))?;
    let reference = session.reference(&log)?;
    let view = session.send(
        writer,
        TxKind::CommitMemory {
            size: head.size,
            root: head.root,
            index: index.unwrap_or(head.index_root),
            manifest: sion_core::hash::Hash32::ZERO,
            proof,
            seal: false,
        },
        vec![reference],
    )?;
    if view.status == ExecStatus::Success {
        history
            .push(sion_memory::Commit {
                pulse: view.pulse,
                size: head.size,
            })
            .map_err(|e| CliError::Protocol(e.to_string()))?;
    }
    Ok(view)
}

/// The head the ledger holds for a memory log, as a reader sees it.
fn ledger_head(session: &Session<'_>, log: &ObjectId) -> Result<sion_memory::Head, CliError> {
    let state = crate::memory::committed(session.ledger, log)?;
    Ok(sion_memory::Head {
        size: state.size,
        root: state.root,
        index_root: state.index,
    })
}

/// Memory for robots: two robots share a warehouse, each with its own
/// identity and memory log; the ledger keeps the logs append-only and
/// attests when each atom was recorded; readers check complete answers,
/// fold beliefs under their own policies, convict an index that leaves an
/// atom out, and see a camera frame forgotten.
pub fn robots(session: &mut Session<'_>, out: &mut Narrator) -> Result<(), CliError> {
    use sion_memory::{
        prove_belief, prove_omission, verify_belief, verify_omission, AtomClass as MemoryClass,
        BeliefPolicy, BeliefState, Box3, CommitHistory, ConflictKind, ForgetRequest, FrameId,
        Interval, Memory, MemoryIndex, MemoryKind, PayloadRequest, RangeQuery, RecallRequest,
        RememberRequest, ReviseRequest, ReviserHead, ReviserLog, Space, Statement, TimeRange,
        Value as Fact,
    };
    const SECOND: u64 = 1_000_000_000;
    let fail = |e: sion_memory::MemoryError| CliError::Protocol(e.to_string());
    out.say(
        "== Memory for robots: a warehouse two robots share, kept append-only by the ledger ==",
    );
    fund_all(
        session,
        &[
            ("fleet", AssetId::usd(), usd(50)),
            ("robot-alpha", AssetId::usd(), usd(5)),
            ("robot-beta", AssetId::usd(), usd(5)),
            ("curator", AssetId::usd(), usd(5)),
        ],
    )?;
    let fleet = session.id("fleet");
    let (alpha, beta, curator) = (
        session.id("robot-alpha"),
        session.id("robot-beta"),
        session.id("curator"),
    );

    // 1. Each robot gets an agent identity of its own (v1.8).
    let mut charters = Vec::new();
    for (name, robot) in [("robot-alpha", alpha), ("robot-beta", beta)] {
        let mut caveats = charter_caveats(fleet);
        caveats.payees = None;
        let view = session.must(
            "fleet",
            TxKind::RegisterAgent {
                agent: robot,
                card: sion_exec::charter::card_commitment(
                    format!(r#"{{"name":"{name}","role":"warehouse robot"}}"#).as_bytes(),
                ),
                caveats,
                funding: usd(1),
                agent_bond: Amount::ZERO,
            },
            Vec::new(),
        )?;
        let charter = find(&view, |e| match e {
            Event::AgentRegistered { agent, .. } => Some(*agent),
            _ => None,
        })?;
        charters.push(charter);
    }
    out.say("           fleet gives robot-alpha and robot-beta agent identities of their own");

    // 2. Each opens a memory log tied to its identity; the curator a plain one.
    let mut logs = Vec::new();
    for (name, agent) in [
        ("robot-alpha", Some(charters[0])),
        ("robot-beta", Some(charters[1])),
        ("curator", None),
    ] {
        let view = session.must(
            name,
            TxKind::OpenMemory {
                agent,
                policy: sion_core::hash::Hash32::ZERO,
            },
            Vec::new(),
        )?;
        logs.push(find(&view, |e| match e {
            Event::MemoryOpened { log, .. } => Some(*log),
            _ => None,
        })?);
    }
    let (log_a, log_b, log_c) = (logs[0], logs[1], logs[2]);
    out.say("           each robot opens a memory log only it can append to, tied to its identity; the curator opens one too");

    // 3. Alpha sees pallet 17 in aisle 4; Beta scans the aisle later and finds it empty.
    let warehouse = FrameId::named("warehouse-3");
    let aisle = Box3::new([40_000, 10_000, 0], [42_000, 12_000, 3_000]);
    let t0 = 1_790_812_800 * SECOND;
    let mut rng = sion_core::rng::seeded(2026);
    let (mut a, mut b, mut c) = (
        Memory::in_memory(alpha),
        Memory::in_memory(beta),
        Memory::in_memory(curator),
    );
    let (mut ha, mut hb, mut hc) = (
        CommitHistory::default(),
        CommitHistory::default(),
        CommitHistory::default(),
    );
    let mut seen = RememberRequest::new(
        MemoryKind::Observation,
        MemoryClass::Observation,
        Statement::claim("pallet-17", "at", Fact::Entity("aisle-4".into())),
        Interval::closed(t0, t0 + 30 * SECOND),
        t0,
    );
    seen.space = Some(Space {
        frame: warehouse,
        bbox: Box3::around([41_000, 11_000, 500], [600, 400, 500]),
        uncertainty_mm: 50,
    });
    seen.confidence = 900;
    let pallet_seq = a.remember(seen, &mut rng).map_err(fail)?.seq;
    let pallet = a
        .log()
        .atom(pallet_seq)
        .cloned()
        .ok_or_else(|| CliError::Protocol("the observation is missing".into()))?;
    let mut scan = RememberRequest::new(
        MemoryKind::Coverage,
        MemoryClass::Observation,
        Statement::claim("aisle-4", "occupied", Fact::Boolean(false)),
        Interval::closed(t0 + 10 * SECOND, t0 + 40 * SECOND),
        t0 + 10 * SECOND,
    );
    scan.space = Some(Space {
        frame: warehouse,
        bbox: aisle,
        uncertainty_mm: 50,
    });
    let empty_seq = b.remember(scan, &mut rng).map_err(fail)?.seq;
    let empty = b
        .log()
        .atom(empty_seq)
        .cloned()
        .ok_or_else(|| CliError::Protocol("the scan is missing".into()))?;
    let view = commit_memory(session, "robot-alpha", log_a, &mut a, &mut ha, None)?;
    out.say(format!(
        "Pulse {:>3}  robot-alpha commits its log: pallet-17 seen in aisle 4. This Pulse is the observation's transaction time; the robot cannot backdate it",
        view.pulse
    ));
    let view = commit_memory(session, "robot-beta", log_b, &mut b, &mut hb, None)?;
    out.say(format!(
        "Pulse {:>3}  robot-beta commits its log: aisle 4 scanned 10 s later, empty",
        view.pulse
    ));

    // 4. A reader asks Alpha, and checks the answer against the head on the ledger.
    let head_a = ledger_head(session, &log_a)?;
    let request = RecallRequest::Range(RangeQuery::new(
        warehouse,
        aisle,
        TimeRange::new(t0, t0 + 60 * SECOND),
    ));
    let answer = a.recall(&request).map_err(fail)?;
    let atoms = Memory::verify(&head_a, &request, &answer).map_err(fail)?;
    let proof_bytes = match &answer.proof {
        sion_memory::RecallProof::Range { proof, .. } => sion_core::encode::encode(proof).len(),
        _ => 0,
    };
    out.say(format!(
        "           a reader asks robot-alpha for everything it recorded in aisle 4 during that minute: {} atom, with a {proof_bytes}-byte proof checked against the head on the ledger",
        atoms.len()
    ));
    let elsewhere = RecallRequest::Range(RangeQuery::new(
        warehouse,
        Box3::new([80_000, 10_000, 0], [82_000, 12_000, 3_000]),
        TimeRange::new(t0, t0 + 60 * SECOND),
    ));
    let nothing = a.recall(&elsewhere).map_err(fail)?;
    let none = Memory::verify(&head_a, &elsewhere, &nothing).map_err(fail)?;
    out.say(format!(
        "           and for aisle 9: {} atoms, with a proof that it recorded nothing there",
        none.len()
    ));

    // 5. Alpha tries to rewrite what it committed.
    let mut rewritten = Memory::in_memory(alpha);
    let mut denial = RememberRequest::new(
        MemoryKind::Observation,
        MemoryClass::Observation,
        Statement::claim("pallet-17", "at", Fact::Entity("aisle-5".into())),
        Interval::closed(t0, t0 + 30 * SECOND),
        t0,
    );
    denial.space = Some(Space {
        frame: warehouse,
        bbox: Box3::around([51_000, 11_000, 500], [600, 400, 500]),
        uncertainty_mm: 50,
    });
    rewritten.remember(denial, &mut rng).map_err(fail)?;
    let forged_head = rewritten.head();
    let reference = session.reference(&log_a)?;
    let view = session.send(
        "robot-alpha",
        TxKind::CommitMemory {
            size: forged_head.size,
            root: forged_head.root,
            index: forged_head.index_root,
            manifest: sion_core::hash::Hash32::ZERO,
            proof: Vec::new(),
            seal: false,
        },
        vec![reference],
    )?;
    out.say(format!(
        "Pulse {:>3}  robot-alpha tries to replace its observation with one in aisle 5: {}",
        view.pulse,
        refusal(&view)
    ));

    // 6. The curator records the disagreement, then decides for Beta.
    let mut conflict = RememberRequest::new(
        MemoryKind::WorldState,
        MemoryClass::Conflict,
        Statement::conflict([pallet.id(), empty.id()], ConflictKind::Existence),
        Interval::starting(t0 + 20 * SECOND),
        t0 + 20 * SECOND,
    );
    conflict.confidence = 1_000;
    c.remember(conflict, &mut rng).map_err(fail)?;
    commit_memory(session, "curator", log_c, &mut c, &mut hc, None)?;
    c.revise(ReviseRequest {
        targets: vec![pallet.id()],
        state: BeliefState::Refuted,
        end_valid: None,
        reason: "robot-beta's scan is later and covers the whole aisle".into(),
        at: t0 + 40 * SECOND,
    })
    .map_err(fail)?;
    let view = commit_memory(session, "curator", log_c, &mut c, &mut hc, None)?;
    out.say(format!(
        "Pulse {:>3}  the curator records that the two robots disagree, then refutes robot-alpha's observation",
        view.pulse
    ));
    let curated = BeliefPolicy::new([alpha, beta, curator]);
    let alpha_only = BeliefPolicy::only(alpha);
    let snapshot = [
        (a.log().clone(), ha.clone(), log_a),
        (b.log().clone(), hb.clone(), log_b),
        (c.log().clone(), hc.clone(), log_c),
    ];
    let revisers: Vec<ReviserLog<'_>> = snapshot
        .iter()
        .map(|(log, commits, _)| ReviserLog { log, commits })
        .collect();
    let mut heads = Vec::new();
    for (log, commits, object) in &snapshot {
        heads.push(ReviserHead {
            writer: log.writer(),
            head: ledger_head(session, object)?,
            commits: commits.clone(),
        });
    }
    let mut beliefs = Vec::new();
    for policy in [&curated, &alpha_only] {
        let bundle = prove_belief(&pallet, policy, &revisers).map_err(fail)?;
        let belief = verify_belief(&pallet, policy, &heads, &bundle).map_err(fail)?;
        beliefs.push(format!("{:?}", belief.state).to_lowercase());
    }
    out.say(format!(
        "           a reader trusting the curator believes the observation {}; one trusting only robot-alpha believes it {}. Same evidence, two beliefs, each checked against the heads on the ledger",
        beliefs[0], beliefs[1]
    ));

    // 7. Beta commits a head whose index leaves its scan out.
    for (n, aisle_x) in [(1u64, 60_000i64), (2, 70_000)] {
        let mut more = RememberRequest::new(
            MemoryKind::Coverage,
            MemoryClass::Observation,
            Statement::claim(format!("aisle-{}", 5 + n), "occupied", Fact::Boolean(false)),
            Interval::closed(t0 + (40 + 10 * n) * SECOND, t0 + (70 + 10 * n) * SECOND),
            t0 + (40 + 10 * n) * SECOND,
        );
        more.space = Some(Space {
            frame: warehouse,
            bbox: Box3::new([aisle_x, 10_000, 0], [aisle_x + 2_000, 12_000, 3_000]),
            uncertainty_mm: 50,
        });
        b.remember(more, &mut rng).map_err(fail)?;
    }
    let forged = MemoryIndex::forge_omitting(b.log().atoms(), &[empty_seq].into_iter().collect())
        .map_err(fail)?;
    let view = commit_memory(
        session,
        "robot-beta",
        log_b,
        &mut b,
        &mut hb,
        Some(forged.root()),
    )?;
    out.say(format!(
        "Pulse {:>3}  robot-beta commits 2 more scans, with an index that leaves out its scan of aisle 4 (the ledger stores index roots without checking them)",
        view.pulse
    ));
    let head_b = ledger_head(session, &log_b)?;
    let fraud = prove_omission(b.log(), head_b.size, &forged, empty_seq)
        .map_err(fail)?
        .ok_or_else(|| CliError::Protocol("no omission found".into()))?;
    verify_omission(&head_b, &fraud).map_err(fail)?;
    out.say(format!(
        "           anyone holding that scan proves the omission from the head on the ledger alone, with a {}-byte fraud proof",
        sion_core::encode::encode(&fraud).len()
    ));

    // 8. A camera frame, sealed, then forgotten.
    let key = a
        .new_epoch_key(
            hash_domain("demo/space", b"warehouse-3 cameras"),
            0,
            &mut rng,
        )
        .map_err(fail)?;
    let mut frame = RememberRequest::new(
        MemoryKind::Observation,
        MemoryClass::Observation,
        Statement::Content(hash_domain("demo/frame", b"camera 2, frame 4411")),
        Interval::closed(t0 + 50 * SECOND, t0 + 50 * SECOND + 1),
        t0 + 50 * SECOND,
    );
    frame.payload = PayloadRequest::Sealed {
        key,
        plaintext: b"jpeg bytes of a face".to_vec(),
    };
    let frame_seq = a.remember(frame, &mut rng).map_err(fail)?.seq;
    let frame_atom = a
        .log()
        .atom(frame_seq)
        .cloned()
        .ok_or_else(|| CliError::Protocol("the frame is missing".into()))?;
    let readable = a.open_payload(&frame_atom).is_ok();
    a.forget(ForgetRequest {
        key,
        at: t0 + 60 * SECOND,
    })
    .map_err(fail)?;
    let view = commit_memory(session, "robot-alpha", log_a, &mut a, &mut ha, None)?;
    out.say(format!(
        "Pulse {:>3}  robot-alpha stores a camera frame encrypted under its memory space's key (readable: {readable}), then forgets it by destroying the key (readable: {}); the log still proves the frame existed",
        view.pulse,
        a.open_payload(&frame_atom).is_ok()
    ));

    // 9. Containment reaches memory.
    let view = session.must(
        "fleet",
        TxKind::SuspendAgent {
            charter: charters[0],
        },
        Vec::new(),
    )?;
    let mut note = RememberRequest::new(
        MemoryKind::Episodic,
        MemoryClass::AgentMemory,
        Statement::claim("robot-alpha", "status", Fact::Text("suspended".into())),
        Interval::starting(t0 + 70 * SECOND),
        t0 + 70 * SECOND,
    );
    note.confidence = 1_000;
    a.remember(note, &mut rng).map_err(fail)?;
    let after = commit_memory(session, "robot-alpha", log_a, &mut a, &mut ha, None)?;
    out.say(format!(
        "Pulse {:>3}  fleet suspends robot-alpha; at Pulse {} its next commit is {}",
        view.pulse,
        after.pulse,
        refusal(&after)
    ));
    Ok(())
}

/// Storage for agents (spec v1.12): a robot pays devices to keep the
/// archive of its memory log and its camera footage, and to prove every
/// round that they still hold them; a provider that kept only half of what
/// it promised is caught by the challenges and loses its collateral.
pub fn storage(session: &mut Session<'_>, out: &mut Narrator) -> Result<(), CliError> {
    use crate::storage::{deal, storage_dir, Provider, ProviderPolicy, Store};
    use sion_exec::storage::{
        challenge_index, current_round, round_seed, DataTree, DealState, ABANDON_ROUNDS,
    };
    use sion_memory::{
        AtomClass as MemoryClass, Box3, FrameId, Interval, Memory, MemoryKind, RememberRequest,
        Space, Statement, Value as Fact,
    };
    const SECOND: u64 = 1_000_000_000;
    const ROUNDS: u32 = 6;
    const ROUND_LENGTH: u64 = 16;
    let fail = |e: sion_memory::MemoryError| CliError::Protocol(e.to_string());
    let io = |e: std::io::Error| CliError::Usage(e.to_string());
    let ledger = session.ledger;
    out.say(
        "== Storage for agents: a robot's memory kept by devices it pays, proven every round ==",
    );
    fund_all(
        session,
        &[
            ("robot-gamma", AssetId::usd(), usd(40)),
            ("laptop", AssetId::usd(), usd(10)),
            ("phone", AssetId::usd(), usd(10)),
        ],
    )?;
    let robot = session.id("robot-gamma");
    let phone_account = session.id("phone");

    // 1. A patrol's worth of memory, committed with its archive's root as
    //    the manifest.
    let mut memory = Memory::in_memory(robot);
    let mut rng = sion_core::rng::seeded(12);
    let t0 = 1_790_812_800 * SECOND;
    for i in 0..400u64 {
        let mut seen = RememberRequest::new(
            MemoryKind::Observation,
            MemoryClass::Observation,
            Statement::claim(
                format!("shelf-{}", i % 40),
                "stock",
                Fact::Integer(i128::from(i * 7 % 100)),
            ),
            Interval::closed(t0 + i * SECOND, t0 + (i + 1) * SECOND),
            t0 + i * SECOND,
        );
        seen.space = Some(Space {
            frame: FrameId::named("warehouse-3"),
            bbox: Box3::around([1_000 * (i % 40) as i64, 5_000, 1_000], [300, 300, 300]),
            uncertainty_mm: 20,
        });
        memory.remember(seen, &mut rng).map_err(fail)?;
    }
    let archive = sion_core::encode::encode(&memory.log().atoms().to_vec());
    let archive_tree = DataTree::of_data(&archive);
    let dir = storage_dir().join("robot-gamma");
    std::fs::create_dir_all(&dir).map_err(io)?;
    let archive_path = dir.join("patrol-archive.bin");
    std::fs::write(&archive_path, &archive).map_err(io)?;
    let opened = session.must(
        "robot-gamma",
        TxKind::OpenMemory {
            agent: None,
            policy: sion_core::hash::Hash32::ZERO,
        },
        Vec::new(),
    )?;
    let log = find(&opened, |e| match e {
        Event::MemoryOpened { log, .. } => Some(*log),
        _ => None,
    })?;
    let head = memory.head();
    let proof = memory
        .log()
        .consistency_proof(0)
        .map_err(|e| CliError::Protocol(e.to_string()))?;
    let reference = session.reference(&log)?;
    let view = session.must(
        "robot-gamma",
        TxKind::CommitMemory {
            size: head.size,
            root: head.root,
            index: head.index_root,
            manifest: archive_tree.root(),
            proof,
            seal: false,
        },
        vec![reference],
    )?;
    out.say(format!(
        "Pulse {:>3}  robot-gamma commits 400 observations of a patrol; the head's manifest is the root of their archive ({} KiB, {} leaves of at most 4 KiB)",
        view.pulse,
        archive.len() / 1024,
        archive_tree.size()
    ));

    // 2. Two deals: the archive for any provider, the camera footage for
    //    the phone.
    let camera: Vec<u8> = {
        let mut bytes = vec![0u8; 64 * 1024];
        rand_core::RngCore::fill_bytes(&mut rng, &mut bytes);
        bytes
    };
    let camera_tree = DataTree::of_data(&camera);
    let camera_path = dir.join("camera.bin");
    std::fs::write(&camera_path, &camera).map_err(io)?;
    let accept_by = Pulse(ledger.pulse()?.0 + 60);
    let locator = |path: &std::path::Path| -> Result<String, CliError> {
        Ok(format!(
            "file://{}",
            std::fs::canonicalize(path).map_err(io)?.display()
        ))
    };
    let view = session.must(
        "robot-gamma",
        TxKind::PostDeal {
            root: archive_tree.root(),
            size: archive_tree.size(),
            challenges: 4,
            price: Amount(500_000),
            rounds: ROUNDS,
            round_length: ROUND_LENGTH,
            collateral: usd(3),
            provider: None,
            accept_by,
            locator: locator(&archive_path)?,
        },
        Vec::new(),
    )?;
    let archive_deal = find(&view, |e| match e {
        Event::DealPosted { deal, .. } => Some(*deal),
        _ => None,
    })?;
    out.say(format!(
        "Pulse {:>3}  robot-gamma posts an open deal for the archive: 6 rounds of 16 Pulses at 0.5 USD a proven round (3 USD in escrow), 4 leaves challenged a round, 3 USD of collateral",
        view.pulse
    ));
    let view = session.must(
        "robot-gamma",
        TxKind::PostDeal {
            root: camera_tree.root(),
            size: camera_tree.size(),
            challenges: 4,
            price: Amount(500_000),
            rounds: ROUNDS,
            round_length: ROUND_LENGTH,
            collateral: usd(3),
            provider: Some(phone_account),
            accept_by,
            locator: locator(&camera_path)?,
        },
        Vec::new(),
    )?;
    let camera_deal = find(&view, |e| match e {
        Event::DealPosted { deal, .. } => Some(*deal),
        _ => None,
    })?;
    out.say(format!(
        "Pulse {:>3}  and a deal for its camera footage ({} leaves) with the phone, on the same terms",
        view.pulse,
        camera_tree.size()
    ));

    // 3. The devices: the laptop takes open deals; the phone only its own.
    //    They run on the robot's machine here, so they may read its files
    //    (providers taking strangers' deals refuse file:// locators).
    let mut laptop = Provider::new(
        "laptop",
        Store::new(storage_dir().join("laptop")),
        ProviderPolicy {
            allow_local: true,
            ..ProviderPolicy::default()
        },
    );
    let mut phone = Provider::new(
        "phone",
        Store::new(storage_dir().join("phone")),
        ProviderPolicy {
            accept_open: false,
            allow_local: true,
            ..ProviderPolicy::default()
        },
    );
    out.say("           a laptop and a phone run providers: each fetches a deal's data from its locator in the background, checks it hashes to the deal's root, posts the collateral, then proves every round");
    let accepted = |ledger: &dyn crate::ledger::Ledger| -> Result<bool, CliError> {
        Ok(
            matches!(deal(ledger, archive_deal)?.state, DealState::Active { .. })
                && matches!(deal(ledger, camera_deal)?.state, DealState::Active { .. }),
        )
    };
    for _ in 0..200 {
        for line in laptop.tick(session, ledger)? {
            out.say(format!("           laptop: {line}"));
        }
        for line in phone.tick(session, ledger)? {
            out.say(format!("           phone:  {line}"));
        }
        if accepted(ledger)? {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !accepted(ledger)? {
        return Err(CliError::Protocol("the providers did not accept".into()));
    }
    // The phone keeps only the first half of the footage, to save space.
    let half = camera_tree.size() / 2;
    std::fs::write(
        storage_dir()
            .join("phone")
            .join(format!("{}.bin", camera_deal.0.to_hex())),
        &camera[..(half as usize) * sion_exec::storage::MAX_LEAF_BYTES],
    )
    .map_err(io)?;
    out.say(format!(
        "           the phone then quietly keeps only leaves 0 to {} of the footage, to save space: the deal still pays it, as long as nobody asks for the rest",
        half - 1
    ));

    // 4. Rounds: both providers prove when they can; the robot closes a
    //    deal abandoned for three rounds.
    let custody_of = |id: ObjectId| -> Result<Option<sion_exec::storage::Custody>, CliError> {
        let DealState::Active { custody, .. } = deal(ledger, id)?.state else {
            return Ok(None);
        };
        Ok(match ledger.object(custody)?.map(|o| o.data) {
            Some(ObjectData::Custody(record)) => Some(record),
            _ => None,
        })
    };
    let mut announced = None;
    let deadline = ledger.pulse()?.0 + u64::from(ROUNDS) * ROUND_LENGTH + 40;
    loop {
        if let Some(record) = custody_of(camera_deal)? {
            let latest = ledger.pulse()?;
            let beacon = match current_round(record.start, record.round_length, latest.next()) {
                Some(round) if round < u64::from(ROUNDS) => {
                    crate::storage::recorded_beacon(ledger, record.round_start(round as u32))?
                        .map(|beacon| (round, beacon))
                }
                _ => None,
            };
            if let Some((round, beacon)) = beacon {
                if announced != Some(round) && round >= u64::from(record.next) {
                    announced = Some(round);
                    let seed = round_seed(&beacon, &record.deal);
                    let picks: Vec<u64> = (0..record.challenges)
                        .map(|k| challenge_index(&seed, round as u32, k, record.size))
                        .collect();
                    let missing = picks.iter().filter(|&&i| i >= half).count();
                    out.say(format!(
                        "           round {round}: the phone is asked for leaves {picks:?}; {}",
                        if missing == 0 {
                            "all in the half it kept, so it answers this time".to_string()
                        } else {
                            format!("it lost {missing} of them and cannot answer")
                        }
                    ));
                }
            }
        }
        for line in laptop.tick(session, ledger)? {
            out.say(format!("           laptop: {line}"));
        }
        for line in phone.tick(session, ledger)? {
            out.say(format!("           phone:  {line}"));
        }
        let pulse = ledger.pulse()?;
        let camera_state = deal(ledger, camera_deal)?;
        if let (DealState::Active { start, .. }, Some(record)) =
            (&camera_state.state, custody_of(camera_deal)?)
        {
            let round = current_round(*start, camera_state.round_length, Pulse(pulse.0 + 1));
            let ended = camera_state.end().is_some_and(|end| pulse.0 + 1 >= end.0);
            let abandoned = round.is_some_and(|r| {
                r < u64::from(ROUNDS) && r >= u64::from(record.next) + u64::from(ABANDON_ROUNDS)
            });
            if ended || abandoned {
                let view = session.must(
                    "robot-gamma",
                    TxKind::CloseDeal { deal: camera_deal },
                    Vec::new(),
                )?;
                let (refund, paid, forfeited, missed) = find(&view, |e| match e {
                    Event::DealClosed {
                        refund,
                        paid,
                        forfeited,
                        missed,
                        ..
                    } => Some((*refund, *paid, *forfeited, *missed)),
                    _ => None,
                })?;
                out.say(format!(
                    "Pulse {:>3}  robot-gamma closes the footage deal ({}): {missed} round(s) missed; {refund} USD of escrow back, {paid} USD paid for what the phone proved, its collateral of {forfeited} USD forfeited to the treasury",
                    view.pulse,
                    if abandoned { "abandoned" } else { "ended" }
                ));
            }
        }
        let archive_closed = matches!(deal(ledger, archive_deal)?.state, DealState::Closed { .. });
        let camera_closed = matches!(deal(ledger, camera_deal)?.state, DealState::Closed { .. });
        if archive_closed && camera_closed {
            break;
        }
        if pulse.0 > deadline {
            return Err(CliError::Protocol("the deals did not close in time".into()));
        }
        session.wait_pulses(1)?;
    }

    // 5. What it came to.
    let DealState::Closed { proven, missed, .. } = deal(ledger, archive_deal)?.state else {
        return Err(CliError::Protocol("the archive deal is not closed".into()));
    };
    out.say(format!(
        "           the laptop proved {proven} of {ROUNDS} rounds ({missed} missed): paid {} USD and its collateral returned; it kept the archive the whole time",
        Amount(500_000 * u128::from(proven))
    ));
    let state = crate::memory::committed(ledger, &log)?;
    out.say(format!(
        "           a reader checks two objects on the ledger: robot-gamma's memory head names manifest {}, and the deal the laptop proved commits to root {}: {}",
        state.manifest.short(),
        archive_tree.root().short(),
        if state.manifest == archive_tree.root() {
            "the memory the robot committed is the data that was kept"
        } else {
            "they differ"
        }
    ));
    out.say("           what a proof shows: the provider could produce the challenged leaves during the round; not that it stores its own copy (no proof of replication)");
    Ok(())
}

fn refusal(view: &TxView) -> String {
    match &view.status {
        ExecStatus::Failed(reason) => format!("refused by the ledger: {reason}"),
        other => format!("{other:?}"),
    }
}

pub const ALL: [&str; 13] = [
    "invoice",
    "agents",
    "swarm",
    "tasks",
    "robots",
    "storage",
    "market",
    "receipts",
    "x402",
    "streams",
    "censorship",
    "light",
    "staking",
];

/// Runs a named demo (`invoice`, `agents`, `swarm`, `tasks`, `robots`, `storage`, `market`,
/// `receipts`, `x402`, `streams`, `light`, `censorship`, `staking`, `all`).
pub fn run(name: &str, session: &mut Session<'_>, out: &mut Narrator) -> Result<(), CliError> {
    match name {
        "invoice" => invoice(session, out),
        "agents" => agents(session, out),
        "swarm" => swarm(session, out),
        "tasks" => tasks(session, out),
        "robots" => robots(session, out),
        "storage" => storage(session, out),
        "market" => market(session, out),
        "receipts" => receipts(session, out),
        "light" => light(session, out),
        "censorship" => censorship(session, out),
        "x402" => x402(session, out),
        "streams" => streams(session, out),
        "staking" => staking(session, out),
        "all" => {
            for (index, demo) in ALL.iter().enumerate() {
                if index > 0 {
                    out.say("");
                }
                run(demo, session, out)?;
            }
            Ok(())
        }
        other => Err(CliError::Usage(format!(
            "unknown demo {other:?}; try invoice, agents, swarm, tasks, robots, storage, market, receipts, x402, streams, censorship, light, staking or all"
        ))),
    }
}
