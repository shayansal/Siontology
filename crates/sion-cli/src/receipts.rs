//! Receipts for which model answered, for someone else to check.
//!
//! A runtime keeps a receipt of every answer a model gives it
//! ([`sion_agent::receipt`]): the model asked for, the address asked, the
//! model the provider said answered, and the tokens, in a chain with one
//! head. This is what is done with them:
//!
//! - `sion agent receipts show` reads a session's receipts on this
//!   machine.
//! - `sion agent receipts write` writes them to a file ([`ReceiptsFile`]);
//!   with a wallet it commits to them in a claim, which fixes them as of
//!   that Pulse. That is the operator's word about its own machine.
//! - An agent rented from an enclave signs its receipts with the key its
//!   attestation binds ([`crate::rentals::SignedReceipts`]), and the
//!   renter keeps them in a file with the attestation. That is the word of
//!   the code that was measured, on the chip that signed the report.
//! - `sion agent receipts check` checks a file of either kind.
//!
//! A receipt says which address a runtime asked and which model it was
//! told answered. It does not look inside the model: the name is the
//! provider's to give.

use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde::{Deserialize, Serialize};
use sion_agent::receipt::{self, Receipt, Receipts, Tally};
use sion_core::hash::Hash32;
use sion_core::ids::ClaimId;
use sion_crypto::keys::{self, PublicKey, Signature};
use sion_enclave::Attestation;
use sion_exec::claim::{formulas, ClaimBody, Payload};

use crate::error::CliError;
use crate::feeds;
use crate::ledger::{Ledger, LedgerExt};
use crate::rentals::{SignedReceipts, RECEIPTS_DOMAIN};
use crate::session::Session;

/// The schema of a claim committing to a session's receipts.
pub const SCHEMA: &str = "agent.receipts/v1";

/// What is signed or committed to of a chain of receipts: what they are
/// of, how many, their head, their tallies, and whether the runtime
/// trusted certificate roots beyond the public ones.
pub fn statement(of: &str, count: u64, head: &str, tallies: &[Tally], extra_roots: bool) -> Hash32 {
    let said = serde_json::json!({
        "of": of,
        "count": count,
        "head": head,
        "tallies": tallies,
        "extra_roots": extra_roots,
    });
    sion_core::hash::hash_domain(
        "sion-cli/receipts/statement/v1",
        said.to_string().as_bytes(),
    )
}

/// Who stands behind a file of receipts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum By {
    /// No one: a file anyone could have written.
    #[default]
    NoOne,
    /// Its operator, who committed to it in a claim.
    Operator {
        /// The chain the claim is on (its genesis digest).
        chain: String,
        /// The claim.
        claim: String,
    },
    /// An enclave, with the key its attestation binds.
    Enclave {
        /// The enclave's evidence and the keys it binds.
        attestation: Box<Attestation>,
        /// The signature of the session's key over the [`statement`].
        signature: Signature,
    },
}

/// A chain of receipts as a file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptsFile {
    /// The format's version.
    pub version: u32,
    /// What they are of: a session's id, or a rental's stream.
    pub of: String,
    /// How many answers there were.
    pub count: u64,
    /// The head of their chain.
    pub head: String,
    /// Every answer, by the model asked, the address and the model that
    /// answered.
    pub tallies: Vec<Tally>,
    /// The receipts given whole: the end of the chain.
    pub receipts: Vec<Receipt>,
    /// Whether the runtime that kept them trusted certificate roots beyond
    /// the public ones.
    #[serde(default)]
    pub extra_roots: bool,
    /// Who stands behind them.
    #[serde(default)]
    pub by: By,
}

impl ReceiptsFile {
    /// The receipts of a session on this machine, with no one behind them
    /// yet.
    pub fn of_session(session: &str, receipts: &Receipts) -> ReceiptsFile {
        ReceiptsFile {
            version: 1,
            of: session.to_string(),
            count: receipts.count,
            head: receipts.head(),
            tallies: receipts.tallies.clone(),
            receipts: receipts.kept.clone(),
            extra_roots: std::env::var_os("SSL_CERT_FILE").is_some(),
            by: By::NoOne,
        }
    }

    /// What an enclave signed of a rental, with the attestation that binds
    /// its key. `kept` are the receipts the renter was given so far.
    pub fn of_rental(
        signed: &SignedReceipts,
        kept: Vec<Receipt>,
        attestation: &Attestation,
    ) -> ReceiptsFile {
        ReceiptsFile {
            version: 1,
            of: signed.of.clone(),
            count: signed.count,
            head: signed.head.clone(),
            tallies: signed.tallies.clone(),
            receipts: kept,
            extra_roots: signed.extra_roots,
            by: By::Enclave {
                attestation: Box::new(attestation.clone()),
                signature: signed.signature.clone(),
            },
        }
    }

    /// Its [`statement`].
    pub fn statement(&self) -> Hash32 {
        statement(
            &self.of,
            self.count,
            &self.head,
            &self.tallies,
            self.extra_roots,
        )
    }
}

/// What a claim committing to receipts holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Committed {
    /// What the receipts are of.
    pub of: String,
    /// How many answers.
    pub count: u64,
    /// The head of their chain.
    pub head: String,
    /// The [`statement`] committed to.
    pub statement: String,
}

fn key_of(of: &str) -> String {
    let digest = sion_core::hash::hash_domain("sion-cli/receipts/of/v1", of.as_bytes());
    format!("receipts:{}", feeds::short_key(&digest))
}

/// Commits to `file` in a claim of `wallet`'s.
pub fn commit(
    session: &mut Session<'_>,
    wallet: &str,
    file: &ReceiptsFile,
) -> Result<ClaimId, CliError> {
    let committed = Committed {
        of: file.of.clone(),
        count: file.count,
        head: file.head.clone(),
        statement: file.statement().to_hex(),
    };
    let payload = serde_json::to_vec(&committed)
        .map_err(|e| CliError::Usage(format!("the commitment could not be written: {e}")))?;
    let body = ClaimBody {
        schema: SCHEMA.into(),
        subject: format!("the receipts of {} model answer(s)", file.count),
        payload: Payload::Public(payload),
        formula: sion_assurance::Formula::and(vec![formulas::dual_signature()]),
        adjudicator: None,
        detection_permille: 0,
        valid_for: 1_000_000,
        challenge_period: 0,
    };
    crate::vouching::publish(session, wallet, body, Some(key_of(&file.of)))
}

/// What a checker of an enclave's receipts trusts.
#[derive(Clone, Debug, Default)]
pub struct Trusted {
    /// The measurement of the code it trusts, in hex; without one the
    /// attestation's own is taken, and what the enclave ran is its word.
    pub measurement: Option<String>,
    /// A simulated platform it takes, by its key in hex (tests, devnets).
    pub simulated_platform: Option<String>,
}

/// Checks `file`: a line for each thing looked at, and whether everything
/// holds. `ledger` is needed for receipts an operator committed to.
pub fn check(
    ledger: Option<&dyn Ledger>,
    file: &ReceiptsFile,
    trusted: &Trusted,
) -> Result<(Vec<String>, bool), CliError> {
    let mut lines = Vec::new();
    let mut holds = true;
    match receipt::check(file.count, &file.head, &file.tallies, &file.receipts) {
        Ok(()) if file.receipts.len() as u64 == file.count => lines.push(format!(
            "holds: {} receipt(s) in a chain that ends at the head, each holding the digest of the \
             one before",
            file.count
        )),
        Ok(()) => lines.push(format!(
            "holds: the last {} of {} receipt(s) are the end of the chain; the tallies count the \
             rest, which are not shown",
            file.receipts.len(),
            file.count
        )),
        Err(why) => {
            holds = false;
            lines.push(format!("differs: {why}"));
        }
    }
    match &file.by {
        By::NoOne => {
            holds = false;
            lines.push(
                "no one stands behind these receipts: the file shows only that it agrees with \
                 itself"
                    .into(),
            );
        }
        By::Operator { chain, claim } => {
            let Some(ledger) = ledger else {
                return Err(CliError::Usage(
                    "these receipts were committed to in a claim: a node is needed to read it"
                        .into(),
                ));
            };
            let here = ledger.genesis_digest()?.to_hex();
            let id = Hash32::from_hex(claim)
                .map(ClaimId::from_hash)
                .map_err(|e| CliError::Usage(format!("the file's claim: {e}")))?;
            if *chain != here {
                holds = false;
                lines.push(format!(
                    "differs: the claim is on another chain ({}…), not this one ({}…)",
                    &chain[..chain.len().min(8)],
                    &here[..8]
                ));
            } else {
                match feeds::read::<Committed>(ledger, id, SCHEMA)? {
                    Some(read) if read.says.statement == file.statement().to_hex() => {
                        lines.push(format!(
                            "holds: account {} committed to this head and these tallies in claim \
                             {} at Pulse {}",
                            read.issuer.short(),
                            id.short(),
                            read.issued_at
                        ));
                        lines.push(
                            "  this is its operator's word about its own machine, fixed as of \
                             that Pulse"
                                .into(),
                        );
                    }
                    Some(_) => {
                        holds = false;
                        lines.push(format!(
                            "differs: claim {} commits to other receipts than these",
                            id.short()
                        ));
                    }
                    None => {
                        holds = false;
                        lines.push(format!(
                            "differs: claim {} is not a commitment to receipts on this ledger",
                            id.short()
                        ));
                    }
                }
            }
        }
        By::Enclave {
            attestation,
            signature,
        } => {
            let expected = sion_enclave::Expectation {
                measurements: match &trusted.measurement {
                    Some(hex) => vec![crate::rent_cli::measurement_bytes(hex)?],
                    None => attestation
                        .measurement()
                        .map(|m| vec![m])
                        .unwrap_or_default(),
                },
                simulated_platforms: match &trusted.simulated_platform {
                    Some(text) => vec![hex::decode(text)
                        .map_err(|e| CliError::Usage(format!("the platform key: {e}")))?],
                    None => Vec::new(),
                },
                // A signature made while the key was bound stands after the
                // binding has run out.
                now: 0,
            };
            match sion_enclave::verify(attestation, &expected) {
                Ok(verified) => {
                    lines.push(format!(
                        "holds: the key is bound to {}, running code of measurement {}",
                        verified.platform.describe(),
                        hex::encode(verified.measurement)
                    ));
                    if trusted.measurement.is_none() {
                        lines.push(
                            "  (you did not say which code you trust: give --measurement <hex>)"
                                .into(),
                        );
                    }
                    let key = PublicKey {
                        scheme: signature.scheme,
                        bytes: verified.binding.signing_public_key.clone(),
                    };
                    match keys::verify(&key, RECEIPTS_DOMAIN, &file.statement().0, signature) {
                        Ok(()) => lines.push(
                            "holds: that key signed this head and these tallies: it is the \
                             measured code that says where it asked and which model it was told \
                             answered"
                                .into(),
                        ),
                        Err(why) => {
                            holds = false;
                            lines.push(format!(
                                "differs: the enclave's key did not sign these receipts: {why}"
                            ));
                        }
                    }
                }
                Err(why) => {
                    holds = false;
                    lines.push(format!("differs: the enclave does not check out: {why}"));
                }
            }
        }
    }
    if file.extra_roots {
        lines.push(
            "note: the runtime trusted certificate roots beyond the public ones \
             (SSL_CERT_FILE): an address's name does not show who answered there"
                .into(),
        );
    }
    for tally in &file.tallies {
        lines.push(format!("  {}", tally.line()));
    }
    Ok((lines, holds))
}

/// A session's receipts in lines: how many, their head, who answered, and
/// the last `last` of them.
pub fn lines(session: &str, receipts: &Receipts, last: usize) -> Vec<String> {
    if receipts.is_empty() {
        return vec![format!(
            "session {session} has no receipts: no model answered in it"
        )];
    }
    let mut lines = vec![format!(
        "session {session}: {} answer(s) from models, head {}…",
        receipts.count,
        &receipts.head()[..16]
    )];
    for tally in &receipts.tallies {
        lines.push(format!("  {}", tally.line()));
    }
    let shown = receipts.kept.len().min(last);
    if shown > 0 {
        lines.push(format!("the last {shown}:"));
        for receipt in &receipts.kept[receipts.kept.len() - shown..] {
            lines.push(format!("  {}", receipt.line()));
        }
    }
    lines
}

/// `sion agent receipts`.
#[derive(Clone, Debug, Subcommand)]
pub enum ReceiptsCommand {
    /// A session's receipts: which model was asked for each answer, at
    /// which address, which model the provider said answered, and the
    /// tokens.
    Show {
        /// The session.
        session: String,
        /// How many of the last receipts to list.
        #[arg(long, default_value_t = 10)]
        last: usize,
    },
    /// Write a session's receipts to a file for someone else to check; with
    /// --wallet, commit to them in a claim, which fixes them as of now.
    Write {
        /// The session.
        session: String,
        /// The file to write.
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
        /// Commit to them in a claim of this wallet's.
        #[arg(long)]
        wallet: Option<String>,
    },
    /// Check a file of receipts: that they are a chain, and who stands
    /// behind them. An operator's are checked against its claim; an
    /// enclave's against its attestation and the key it binds.
    Check {
        /// The file.
        file: PathBuf,
        /// The measurement of the enclave's code you trust (hex).
        #[arg(long)]
        measurement: Option<String>,
        /// SIMULATED: a simulated platform's key to take (hex), for tests
        /// and devnets.
        #[arg(long = "simulated-platform", value_name = "KEY")]
        simulated_platform: Option<String>,
    },
}

fn read(path: &Path) -> Result<ReceiptsFile, CliError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
    serde_json::from_str(&text)
        .map_err(|e| CliError::Usage(format!("{} is not a file of receipts: {e}", path.display())))
}

impl ReceiptsCommand {
    /// Whether the command needs a node: to commit to receipts, or to read
    /// the claim a file names.
    pub fn needs_ledger(&self) -> bool {
        match self {
            ReceiptsCommand::Show { .. } => false,
            ReceiptsCommand::Write { wallet, .. } => wallet.is_some(),
            ReceiptsCommand::Check { file, .. } => {
                matches!(read(file).map(|f| f.by), Ok(By::Operator { .. }))
            }
        }
    }
}

/// Runs a receipts command; `session` is there when a node is needed.
pub fn run(
    session: Option<(&mut Session<'_>, &dyn Ledger)>,
    command: ReceiptsCommand,
) -> Result<(), CliError> {
    match command {
        ReceiptsCommand::Show { session: id, last } => {
            let saved = crate::runtime::Saved::load(&id)?;
            for line in lines(&saved.state.id, &saved.state.receipts, last) {
                println!("{line}");
            }
            Ok(())
        }
        ReceiptsCommand::Write {
            session: id,
            out,
            wallet,
        } => {
            let saved = crate::runtime::Saved::load(&id)?;
            if saved.state.receipts.is_empty() {
                return Err(CliError::Usage(format!(
                    "session {id} has no receipts: no model answered in it"
                )));
            }
            let mut file = ReceiptsFile::of_session(&saved.state.id, &saved.state.receipts);
            if let (Some(wallet), Some((session, ledger))) = (wallet, session) {
                let claim = commit(session, &wallet, &file)?;
                file.by = By::Operator {
                    chain: ledger.genesis_digest()?.to_hex(),
                    claim: claim.as_hash().to_hex(),
                };
                println!(
                    "committed to {} receipt(s) in claim {}",
                    file.count,
                    claim.as_hash().to_hex()
                );
            }
            crate::profiles::write_json(&out, &file)?;
            println!("{} receipt(s) written to {}", file.count, out.display());
            Ok(())
        }
        ReceiptsCommand::Check {
            file,
            measurement,
            simulated_platform,
        } => {
            let file = read(&file)?;
            let trusted = Trusted {
                measurement,
                simulated_platform,
            };
            let (lines, holds) = check(session.map(|(_, ledger)| ledger), &file, &trusted)?;
            for line in lines {
                println!("{line}");
            }
            if holds {
                Ok(())
            } else {
                Err(CliError::Verification(
                    "the receipts do not check out".into(),
                ))
            }
        }
    }
}
