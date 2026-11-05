//! `sion agent sell` and `sion agent buy`: an agent's work sold through
//! ACP's checkout ([`crate::acp`]), paid in USD on Siontology, and, with
//! AP2 ([`crate::ap2`]), against the mandates the buyer's user signed.

use std::path::PathBuf;

use clap::Subcommand;
use serde_json::Value;
use sion_core::hash::Hash32;

use crate::acp::{self, Buyer, Catalog, Merchant};
use crate::ap2::{SigningKey, Trust};
use crate::error::CliError;
use crate::runtime::SharedLedger;
use crate::session::Session;
use crate::x402::{Facilitator, Payer};

/// Selling through ACP.
#[derive(Debug, Subcommand)]
pub enum SellCommand {
    /// Serve ACP's checkout API for a catalog (agent work at fixed prices),
    /// paid to a wallet in USD on this chain: other agents make a checkout
    /// session, pay it with a signed transaction, and get an order.
    Serve {
        /// The catalog: `{"offers": [{"id", "name", "description",
        /// "price_usd"}]}`, prices in whole cents.
        catalog: PathBuf,
        /// Who is paid: a wallet's name, or an account (64 hex digits).
        #[arg(long)]
        pay_to: String,
        /// The merchant's name, as buyers and their mandates see it.
        #[arg(long)]
        name: String,
        /// Where to listen.
        #[arg(long, default_value = "127.0.0.1:8796", value_name = "ADDRESS")]
        listen_at: String,
        /// The URL buyers reach it at (its API base and order links);
        /// http://<listen-at> if not given.
        #[arg(long, value_name = "URL")]
        public_url: Option<String>,
        /// The environment variable holding the bearer tokens buyers are
        /// given, comma-separated, each at least 16 characters.
        #[arg(long, default_value = "SION_ACP_TOKENS", value_name = "NAME")]
        tokens_env: String,
        /// The chain identity payments are verified against (64 hex
        /// digits); the node's word if not given.
        #[arg(long, value_name = "HEX")]
        chain: Option<String>,
        /// Take AP2 mandates whose roots these keys signed (a JWK Set
        /// file, `{"keys": [{"kid", "kty": "EC", "crv": "P-256", …}]}`).
        #[arg(long, value_name = "FILE")]
        ap2_trust: Option<PathBuf>,
        /// Complete a session only with an AP2 checkout mandate.
        #[arg(long, requires = "ap2_trust")]
        require_mandate: bool,
        /// Serve this many requests, then stop (for trying it out).
        #[arg(long, value_name = "N")]
        requests: Option<usize>,
    },
}

/// The merchant's AP2 signing key, kept between runs so that its
/// checkouts and receipts stay checkable: made on first use, readable only
/// by its owner.
fn merchant_key() -> Result<SigningKey, CliError> {
    let path = crate::model_routes::dir().join("acp-merchant-key.p8");
    if let Ok(bytes) = std::fs::read(&path) {
        return SigningKey::from_pkcs8(&bytes).map_err(CliError::Usage);
    }
    let key = SigningKey::generate();
    let io = |e: std::io::Error| CliError::Usage(format!("{}: {e}", path.display()));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    std::fs::write(&path, key.pkcs8()).map_err(io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(io)?;
    }
    Ok(key)
}

fn chain_of(ledger: &dyn crate::ledger::Ledger, chain: Option<&str>) -> Result<Hash32, CliError> {
    match crate::skills::pinned_chain(chain)? {
        Some(chain) => Ok(chain),
        None => {
            let served = crate::verify::served_chain(ledger)?;
            eprintln!(
                "warning: chain {} is the node's word; pass --chain to pin it",
                served.short()
            );
            Ok(served)
        }
    }
}

/// Runs `sion agent sell`.
pub fn sell(ledger: SharedLedger, command: SellCommand) -> Result<(), CliError> {
    let SellCommand::Serve {
        catalog,
        pay_to,
        name,
        listen_at,
        public_url,
        tokens_env,
        chain,
        ap2_trust,
        require_mandate,
        requests,
    } = command;
    let text = std::fs::read_to_string(&catalog)
        .map_err(|e| CliError::Usage(format!("{}: {e}", catalog.display())))?;
    let parsed: Value = serde_json::from_str(&text)
        .map_err(|e| CliError::Usage(format!("{}: {e}", catalog.display())))?;
    let catalog = Catalog::from_json(&parsed)?;
    let tokens: Vec<String> = std::env::var(&tokens_env)
        .unwrap_or_default()
        .split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.is_empty() {
        return Err(CliError::Usage(format!(
            "set {tokens_env} to the bearer tokens buyers are given (comma-separated)"
        )));
    }
    let ledger = ledger.as_ref();
    let chain = chain_of(ledger, chain.as_deref())?;
    let mut session = Session::open(ledger)?;
    let pay_to = session.id(&pay_to);
    let server = tiny_http::Server::http(&listen_at)
        .map_err(|e| CliError::Usage(format!("cannot listen on {listen_at}: {e}")))?;
    let address = server
        .server_addr()
        .to_ip()
        .map(|a| a.to_string())
        .unwrap_or(listen_at);
    let base = public_url.unwrap_or_else(|| format!("http://{address}"));
    let orders = crate::model_routes::dir().join("acp-orders.jsonl");
    let mut merchant = Merchant::new(
        Facilitator::new(ledger, chain)?,
        pay_to,
        &name,
        &base,
        catalog.clone(),
        tokens,
    )?
    .with_orders(orders.clone());
    if let Some(path) = ap2_trust {
        let text = std::fs::read_to_string(&path)
            .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
        let set: Value = serde_json::from_str(&text)
            .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
        let trust = Trust::from_jwks(&set).map_err(|e| CliError::Usage(e.to_string()))?;
        if trust.is_empty() {
            return Err(CliError::Usage(format!("{} trusts no key", path.display())));
        }
        merchant = merchant.with_ap2(merchant_key()?, trust, require_mandate);
    }
    println!(
        "selling {} offer(s) through ACP {} at {base}, paid to {} on {}",
        catalog.offers.len(),
        acp::API_VERSION,
        pay_to.short(),
        crate::x402::network(&chain),
    );
    for offer in catalog.offers.values() {
        println!("  {}: {} ({} USD)", offer.id, offer.name, offer.price);
    }
    if let Some(key) = merchant.ap2_key() {
        println!(
            "AP2 mandates taken{}; checkouts and receipts are signed by {}",
            if require_mandate { " and required" } else { "" },
            key.jwk()
        );
    }
    println!("orders are written to {}", orders.display());
    acp::serve(&merchant, &server, requests);
    Ok(())
}

/// Runs `sion agent buy`: buys `items` from the ACP merchant at `url`.
#[allow(clippy::too_many_arguments)]
pub fn buy(
    ledger: SharedLedger,
    url: &str,
    items: &[String],
    wallet: &str,
    charter: Option<&str>,
    allotment: Option<&str>,
    max_usd: &str,
    chain: Option<&str>,
    token_env: &str,
) -> Result<(), CliError> {
    let token = std::env::var(token_env).map_err(|_| {
        CliError::Usage(format!(
            "set {token_env} to the token the merchant gave you"
        ))
    })?;
    let ledger = ledger.as_ref();
    let chain = chain_of(ledger, chain)?;
    let payer = match (charter, allotment) {
        (Some(charter), Some(allotment)) => Payer::Charter {
            agent: wallet.to_string(),
            charter: crate::agents::parse_object(charter, "the charter")?,
            allotment: crate::agents::parse_object(allotment, "the allotment")?,
        },
        (None, None) => Payer::Wallet(wallet.to_string()),
        _ => {
            return Err(CliError::Usage(
                "a charter payment names both --charter and --allotment".into(),
            ))
        }
    };
    let mut session = Session::open(ledger)?;
    let buyer = Buyer {
        base_url: url,
        token: &token,
        chain,
        max_price: crate::agents::parse_amount(max_usd)?,
    };
    let names: Vec<&str> = items.iter().map(String::as_str).collect();
    let done = acp::buy(&mut session, &payer, &buyer, &names, |_| Ok(None))?;
    let total = done["totals"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|t| t["type"] == "total")
        .and_then(|t| t["amount"].as_u64())
        .unwrap_or_default();
    println!(
        "bought {} for {}.{:02} USD: order {} ({})",
        names.join(", "),
        total / 100,
        total % 100,
        done["order"]["id"].as_str().unwrap_or_default(),
        done["order"]["permalink_url"].as_str().unwrap_or_default(),
    );
    Ok(())
}
