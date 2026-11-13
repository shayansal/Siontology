//! Proof that payments kept the rules, without showing the payments.
//!
//! An operator holds a list of payments it will not show: to whom, and how
//! much. An auditor, an insurer or a counterparty wants to know that every
//! one of them was within a limit, that they came to no more than a total,
//! and that each went to one of a list of payees. [`certify`] commits to
//! each payment's amount and payee and proves exactly that
//! ([`sion_zk`]). [`check`] verifies the certificate from the commitments
//! alone: it learns how many payments there were and that they kept the
//! rules, and nothing else about them.
//!
//! A certificate is about the payments its maker committed to. That those
//! are all the payments there were is not something the proof shows. What
//! can be shown is *when* the list was fixed: a claim that commits to the
//! certificate ([`commit`]) puts that on the ledger, and a list fixed
//! before anyone asked is harder to have chosen for the asking.
//!
//! This ledger's own payments are in the clear, so nothing here is needed
//! to audit them: an agent's record adds those up for anyone
//! ([`crate::record`]). The certificate is for payments the ledger does
//! not show, made by card or on another rail, and it is the proof a ledger
//! with confidential amounts would check; that ledger is not built, and it
//! would need proofs whose soundness does not rest on discrete logarithms
//! ([`sion_zk`]).

use std::path::PathBuf;

use clap::Subcommand;
use serde::{Deserialize, Serialize};
use sion_core::ids::ClaimId;
use sion_core::Amount;
use sion_exec::claim::{formulas, ClaimBody, Payload};
use sion_zk::{Commitment, MemberProof, Name, NameOpening, Opening, Proof, Statement};

use crate::error::CliError;
use crate::ledger::Ledger;
use crate::session::Session;

/// The schema of a claim committing to a certificate.
pub const CERTIFICATE_SCHEMA: &str = "agent.compliance/v1";

/// The most payments one certificate covers.
pub const MAX_PAYMENTS: usize = 1_000;

/// A payment, as its maker holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payment {
    /// The amount, in USD.
    pub amount: String,
    /// Whom it went to: an account's id or any name the rules' list uses.
    pub to: String,
}

/// The rules a certificate shows were kept. Each is public.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rules {
    /// No payment was above this many millionths of a USD.
    pub max_payment_micros: u64,
    /// All of them came to at most this many.
    pub total_micros: u64,
    /// Each went to one of these, if a list is given.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub payees: Vec<String>,
}

/// A certificate: commitments to payments and the proof that they keep
/// the rules.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Certificate {
    /// What it was made for, in its maker's words: the proof holds for
    /// this and nothing else.
    pub purpose: String,
    /// The rules.
    pub rules: Rules,
    /// A commitment to each payment's amount, in millionths of a USD.
    pub amounts: Vec<Commitment>,
    /// A commitment to each payment's payee, if the rules list payees.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub payees: Vec<Commitment>,
    /// The proof that each amount is within the limit and that they come
    /// to no more than the total.
    pub proof: Proof,
    /// For each payment, the proof that its payee is on the list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<MemberProof>,
}

/// What opens a certificate's commitments: kept by its maker, never shown
/// with it. Whoever holds this file knows the payments.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Openings {
    /// The amounts' openings.
    pub amounts: Vec<Opening>,
    /// The payees' openings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub payees: Vec<NameOpening>,
}

fn micros(amount: Amount) -> Result<u64, CliError> {
    u64::try_from(amount.0).map_err(|_| CliError::Usage("an amount is too large".into()))
}

/// What a proof is bound to: the purpose, the rules and the commitments.
fn context(purpose: &str, rules: &Rules, amounts: &[Commitment], payees: &[Commitment]) -> Vec<u8> {
    let bound = serde_json::json!({
        "purpose": purpose, "rules": rules, "amounts": amounts, "payees": payees,
    });
    sion_core::hash::hash_domain("sion-cli/compliance/v1", bound.to_string().as_bytes())
        .0
        .to_vec()
}

/// The rules' statements over `count` committed amounts.
fn statements(rules: &Rules, count: usize) -> Vec<Statement> {
    let mut statements: Vec<Statement> = (0..count)
        .map(|at| Statement::at_most(at, rules.max_payment_micros))
        .collect();
    statements.push(Statement::sum_at_most(0..count, rules.total_micros));
    statements
}

fn names(rules: &Rules) -> Vec<Name> {
    rules
        .payees
        .iter()
        .map(|p| sion_zk::name(p.trim().as_bytes()))
        .collect()
}

/// Commits to `payments` and proves that they keep `rules`, for
/// `purpose`. An error says which rule a payment breaks: there is no
/// certificate of what is not so.
pub fn certify(
    purpose: &str,
    payments: &[Payment],
    rules: &Rules,
) -> Result<(Certificate, Openings), CliError> {
    if payments.is_empty() || payments.len() > MAX_PAYMENTS {
        return Err(CliError::Usage(format!(
            "a certificate covers 1 to {MAX_PAYMENTS} payments: these are {}",
            payments.len()
        )));
    }
    let mut amounts = Vec::new();
    let mut openings = Vec::new();
    for payment in payments {
        let (commitment, opening) =
            sion_zk::commit(micros(crate::agents::parse_amount(&payment.amount)?)?);
        amounts.push(commitment);
        openings.push(opening);
    }
    let list = names(rules);
    let (mut payees, mut payee_openings) = (Vec::new(), Vec::new());
    if !list.is_empty() {
        for payment in payments {
            let (commitment, opening) =
                sion_zk::commit_name(&sion_zk::name(payment.to.trim().as_bytes()));
            payees.push(commitment);
            payee_openings.push(opening);
        }
    }
    let context = context(purpose, rules, &amounts, &payees);
    let proof = sion_zk::prove(&context, &openings, &statements(rules, payments.len())).map_err(
        |e| match e {
            sion_zk::Error::False(at) if at < payments.len() => CliError::Usage(format!(
                "payment {} is above the limit of {} USD: there is no certificate to make",
                at + 1,
                Amount(u128::from(rules.max_payment_micros))
            )),
            sion_zk::Error::False(_) => CliError::Usage(format!(
                "the payments come to more than {} USD: there is no certificate to make",
                Amount(u128::from(rules.total_micros))
            )),
            other => CliError::Usage(format!("the proof could not be made: {other}")),
        },
    )?;
    let mut members = Vec::new();
    for (at, opening) in payee_openings.iter().enumerate() {
        members.push(
            sion_zk::prove_member(&context, opening, &list).map_err(|e| match e {
                sion_zk::Error::NotAMember => CliError::Usage(format!(
                    "payment {} went to someone who is not on the list: there is no certificate \
                     to make",
                    at + 1
                )),
                other => CliError::Usage(format!("the proof could not be made: {other}")),
            })?,
        );
    }
    Ok((
        Certificate {
            purpose: purpose.to_string(),
            rules: rules.clone(),
            amounts,
            payees,
            proof,
            members,
        },
        Openings {
            amounts: openings,
            payees: payee_openings,
        },
    ))
}

/// Checks `certificate` from what it holds alone: a line for each rule it
/// shows was kept, or why it does not hold.
pub fn check(certificate: &Certificate) -> Result<Vec<String>, CliError> {
    let rules = &certificate.rules;
    let count = certificate.amounts.len();
    let fails = |why: String| CliError::Usage(format!("the certificate does not hold: {why}"));
    if count == 0 || count > MAX_PAYMENTS {
        return Err(fails("it covers no payment, or too many".into()));
    }
    let list = names(rules);
    if list.is_empty() != certificate.payees.is_empty()
        || certificate.payees.len() != certificate.members.len()
        || (!list.is_empty() && certificate.payees.len() != count)
    {
        return Err(fails(
            "its payees and their proofs do not match its payments".into(),
        ));
    }
    let context = context(
        &certificate.purpose,
        rules,
        &certificate.amounts,
        &certificate.payees,
    );
    sion_zk::verify(
        &context,
        &certificate.amounts,
        &statements(rules, count),
        &certificate.proof,
    )
    .map_err(|e| fails(format!("the proof about the amounts: {e}")))?;
    for (at, (payee, member)) in certificate
        .payees
        .iter()
        .zip(&certificate.members)
        .enumerate()
    {
        sion_zk::verify_member(&context, payee, &list, member)
            .map_err(|e| fails(format!("the proof about payment {}'s payee: {e}", at + 1)))?;
    }
    let mut lines = vec![
        format!("for: {}", certificate.purpose),
        format!(
            "holds: {count} payment(s), each a whole amount of at most {} USD",
            Amount(u128::from(rules.max_payment_micros))
        ),
        format!(
            "holds: they come to at most {} USD in all",
            Amount(u128::from(rules.total_micros))
        ),
    ];
    if !list.is_empty() {
        lines.push(format!(
            "holds: each went to one of {} payee(s): {}",
            rules.payees.len(),
            rules.payees.join(", ")
        ));
    }
    lines.push(
        "shown: how many payments there were, and that they keep these rules; not the amounts, \
         nor who was paid what"
            .to_string(),
    );
    lines.push(
        "not shown: that these are all the payments there were. That is their maker's word, \
         unless the list was fixed elsewhere when they were made"
            .to_string(),
    );
    Ok(lines)
}

/// The certificate's digest: what a claim commits to.
pub fn digest(certificate: &Certificate) -> Result<sion_core::hash::Hash32, CliError> {
    let bytes = serde_json::to_vec(certificate)
        .map_err(|e| CliError::Usage(format!("the certificate could not be written: {e}")))?;
    Ok(sion_core::hash::hash_domain(
        "sion-cli/compliance/certificate/v1",
        &bytes,
    ))
}

/// Publishes a commitment to `certificate` as `wallet`'s claim: when the
/// list of payments was fixed.
pub fn commit(
    session: &mut Session<'_>,
    wallet: &str,
    certificate: &Certificate,
) -> Result<ClaimId, CliError> {
    let digest = digest(certificate)?.to_hex();
    let body = ClaimBody {
        schema: CERTIFICATE_SCHEMA.into(),
        subject: format!(
            "{} payment(s) kept their rules: {}",
            certificate.amounts.len(),
            certificate.purpose.chars().take(80).collect::<String>()
        ),
        payload: Payload::Public(digest.clone().into_bytes()),
        formula: sion_assurance::Formula::and(vec![formulas::dual_signature()]),
        adjudicator: None,
        detection_permille: 0,
        valid_for: 1_000_000,
        challenge_period: 0,
    };
    crate::vouching::publish(
        session,
        wallet,
        body,
        Some(format!("compliance:{}", &digest[..32])),
    )
}

/// `sion agent comply`.
#[derive(Clone, Debug, Subcommand)]
pub enum ComplyCommand {
    /// Prove that payments you will not show kept your rules: none above a
    /// limit, no more than a total in all, each to one of a list of payees
    /// if you give one. Writes a certificate anyone can check, and the
    /// openings, which you keep to yourself.
    Prove {
        /// The payments: a JSON file, a list of {"amount": "<USD>", "to":
        /// "<payee>"}.
        #[arg(long, value_name = "FILE")]
        payments: PathBuf,
        /// What no payment may be above, in USD.
        #[arg(long = "max-payment", value_name = "USD")]
        max_payment: String,
        /// What they may come to in all, in USD.
        #[arg(long, value_name = "USD")]
        total: String,
        /// A payee payments may go to (repeat for more). With none, payees
        /// are neither committed to nor proven.
        #[arg(long = "payee", value_name = "NAME")]
        payees: Vec<String>,
        /// What the certificate is for: it holds for this and nothing else.
        #[arg(long = "for", value_name = "TEXT")]
        purpose: String,
        /// Where to write the certificate.
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
        /// Commit to the certificate in a claim of this wallet's, so that
        /// when the list was fixed is on the ledger.
        #[arg(long)]
        wallet: Option<String>,
    },
    /// Check a certificate someone gave you. Needs nothing but the file.
    Check {
        /// The certificate.
        file: PathBuf,
    },
}

impl ComplyCommand {
    /// Whether it needs a node.
    pub fn needs_ledger(&self) -> bool {
        matches!(
            self,
            ComplyCommand::Prove {
                wallet: Some(_),
                ..
            }
        )
    }
}

fn read<T: serde::de::DeserializeOwned>(path: &std::path::Path, what: &str) -> Result<T, CliError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
    serde_json::from_str(&text)
        .map_err(|e| CliError::Usage(format!("{} is not {what}: {e}", path.display())))
}

/// Runs a comply command; `session` is there when a claim is to be made.
pub fn run(
    session: Option<(&mut Session<'_>, &dyn Ledger)>,
    command: ComplyCommand,
) -> Result<(), CliError> {
    match command {
        ComplyCommand::Prove {
            payments,
            max_payment,
            total,
            payees,
            purpose,
            out,
            wallet,
        } => {
            let payments: Vec<Payment> = read(&payments, "a list of payments")?;
            let rules = Rules {
                max_payment_micros: micros(crate::agents::parse_amount(&max_payment)?)?,
                total_micros: micros(crate::agents::parse_amount(&total)?)?,
                payees,
            };
            let (certificate, openings) = certify(&purpose, &payments, &rules)?;
            crate::profiles::write_json(&out, &certificate)?;
            let kept = out.with_extension("openings.json");
            crate::profiles::write_json(&kept, &openings)?;
            println!("certified {} payment(s): {}", payments.len(), out.display());
            println!(
                "the openings are in {}: keep them to yourself, whoever holds them knows the \
                 payments",
                kept.display()
            );
            if let (Some(wallet), Some((session, _))) = (wallet, session) {
                let claim = commit(session, &wallet, &certificate)?;
                println!("committed to in claim {}", claim.as_hash().to_hex());
            }
            Ok(())
        }
        ComplyCommand::Check { file } => {
            let certificate: Certificate = read(&file, "a certificate")?;
            for line in check(&certificate)? {
                println!("{line}");
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paid(amount: &str, to: &str) -> Payment {
        Payment {
            amount: amount.into(),
            to: to.into(),
        }
    }

    fn rules(max: u64, total: u64, payees: &[&str]) -> Rules {
        Rules {
            max_payment_micros: max * 1_000_000,
            total_micros: total * 1_000_000,
            payees: payees.iter().map(|p| p.to_string()).collect(),
        }
    }

    #[test]
    fn payments_that_kept_the_rules_are_certified_without_being_shown() {
        let payments = [
            paid("5", "acme-ltd"),
            paid("8.50", "birch"),
            paid("2", "acme-ltd"),
        ];
        let rules = rules(10, 20, &["acme-ltd", "birch", "cedar"]);
        let (certificate, openings) = certify("audit of May", &payments, &rules).unwrap();
        let shown = check(&certificate).unwrap().join("\n");
        assert!(shown.contains("holds: 3 payment(s), each a whole amount of at most 10 USD"));
        assert!(shown.contains("holds: they come to at most 20 USD in all"));
        assert!(shown.contains("holds: each went to one of 3 payee(s)"));
        // The certificate holds no amount and no payee's place in the list;
        // the openings do, and are another file.
        let text = serde_json::to_string(&certificate).unwrap();
        assert!(
            !text.contains("8500000") && !text.contains("8.50"),
            "{text}"
        );
        assert_eq!(openings.amounts[1].value, 8_500_000);
        // It goes through a file as it is.
        let again: Certificate = serde_json::from_str(&text).unwrap();
        assert!(check(&again).is_ok());
        // Without a list of payees, payees are neither committed to nor
        // proven.
        let (open, _) = certify(
            "x",
            &payments,
            &super::Rules {
                payees: Vec::new(),
                ..rules.clone()
            },
        )
        .unwrap();
        assert!(open.payees.is_empty() && open.members.is_empty());
        assert!(!check(&open).unwrap().join("\n").contains("payee(s)"));
    }

    #[test]
    fn there_is_no_certificate_of_payments_that_broke_a_rule() {
        let payments = [paid("5", "acme-ltd"), paid("12", "birch")];
        let over = certify("a", &payments, &rules(10, 100, &[]))
            .unwrap_err()
            .to_string();
        assert!(
            over.contains("payment 2 is above the limit of 10 USD"),
            "{over}"
        );
        let total = certify("a", &payments, &rules(20, 16, &[]))
            .unwrap_err()
            .to_string();
        assert!(total.contains("come to more than 16 USD"), "{total}");
        let stranger = certify("a", &payments, &rules(20, 100, &["acme-ltd", "cedar"]))
            .unwrap_err()
            .to_string();
        assert!(
            stranger.contains("payment 2 went to someone who is not on the list"),
            "{stranger}"
        );
    }

    #[test]
    fn a_certificate_altered_or_used_for_something_else_does_not_hold() {
        let payments = [paid("5", "acme-ltd"), paid("9", "birch")];
        let (certificate, _) = certify(
            "audit of May",
            &payments,
            &rules(10, 20, &["acme-ltd", "birch"]),
        )
        .unwrap();
        assert!(check(&certificate).is_ok());
        let stricter = Certificate {
            rules: rules(8, 20, &["acme-ltd", "birch"]),
            ..certificate.clone()
        };
        assert!(check(&stricter).is_err());
        let fewer_payees = Certificate {
            rules: rules(10, 20, &["acme-ltd"]),
            ..certificate.clone()
        };
        assert!(check(&fewer_payees).is_err());
        let other_purpose = Certificate {
            purpose: "audit of June".into(),
            ..certificate.clone()
        };
        assert!(check(&other_purpose).is_err());
        let mut dropped = certificate.clone();
        dropped.amounts.pop();
        dropped.payees.pop();
        dropped.members.pop();
        assert!(check(&dropped).is_err());
        // Another list's commitments under this proof.
        let (other, _) = certify(
            "audit of May",
            &payments,
            &rules(10, 20, &["acme-ltd", "birch"]),
        )
        .unwrap();
        let mixed = Certificate {
            amounts: other.amounts,
            ..certificate
        };
        assert!(check(&mixed).is_err());
    }
}
