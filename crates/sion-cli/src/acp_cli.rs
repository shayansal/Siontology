//! `sion agent sell`, `sion agent buy` and `sion agent commerce`: an agent's
//! work sold through ACP's checkout ([`crate::acp`]), paid in USD on
//! Siontology or by card, and, with AP2 ([`crate::ap2`]), against the
//! mandates the buyer's user signed; and the services around a checkout
//! (a PSP's delegate payment vault, a 3-D Secure provider, an agent's Feed
//! API, a webhook receiver).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use aws_lc_rs::signature::Ed25519KeyPair;
use clap::{Args, Subcommand};
use serde_json::{json, Value};
use sion_core::hash::Hash32;

use crate::acp::card::{self, CardHandler, PspProcessor, Stripe};
use crate::acp::http::{self, Request, Response, Service};
use crate::acp::signing::{Signer, Verifier};
use crate::acp::state::Store;
use crate::acp::{self, buyer, feeds, psp, three_ds, webhooks, Buyer, Catalog, Merchant};
use crate::ap2::{SigningKey, Trust};
use crate::error::CliError;
use crate::runtime::SharedLedger;
use crate::session::Session;
use crate::x402::{Facilitator, Payer};

/// Selling through ACP.
#[derive(Debug, Subcommand)]
pub enum SellCommand {
    /// Serve ACP's checkout and cart API for a catalog (agent work at fixed
    /// prices), paid to a wallet in USD on this chain or by card: other
    /// agents make a checkout session, pay it, and get an order. Sessions,
    /// carts and orders are kept across restarts.
    Serve(Box<ServeArgs>),
    /// List the orders a merchant made (its admin API).
    Orders {
        /// The merchant's API base URL.
        url: String,
        /// The environment variable holding the merchant's admin token.
        #[arg(long, default_value = "SION_ACP_ADMIN_TOKEN", value_name = "NAME")]
        admin_token_env: String,
    },
    /// Mark an order delivered: the buyer's platform is told by webhook.
    Fulfill {
        /// The merchant's API base URL.
        url: String,
        /// The order's id.
        order: String,
        /// Where the buyer gets what was bought.
        #[arg(long, value_name = "URL")]
        access_url: Option<String>,
        /// A license key, if what was bought has one.
        #[arg(long, value_name = "KEY")]
        license_key: Option<String>,
        /// What was delivered, in a few words.
        #[arg(long, value_name = "TEXT")]
        description: Option<String>,
        /// The environment variable holding the merchant's admin token.
        #[arg(long, default_value = "SION_ACP_ADMIN_TOKEN", value_name = "NAME")]
        admin_token_env: String,
    },
    /// Refund an order, all of it or some cents of it: back to the payer
    /// on the ledger, or at the PSP for a card.
    Refund {
        /// The merchant's API base URL.
        url: String,
        /// The order's id.
        order: String,
        /// How much, in cents (what is left of the order if not given).
        #[arg(long, value_name = "CENTS")]
        cents: Option<u64>,
        /// Why.
        #[arg(long, value_name = "TEXT")]
        reason: Option<String>,
        /// The environment variable holding the merchant's admin token.
        #[arg(long, default_value = "SION_ACP_ADMIN_TOKEN", value_name = "NAME")]
        admin_token_env: String,
    },
    /// Publish a catalog as an ACP product feed: to an agent's Feed API,
    /// or as the offline snapshot (`metadata.json`, `products.jsonl`).
    Feed {
        /// The catalog, as `sell serve` takes it.
        catalog: PathBuf,
        /// The merchant's name, as the feed shows the seller.
        #[arg(long)]
        name: String,
        /// The merchant's storefront (its checkout API's base URL).
        #[arg(long, value_name = "URL")]
        public_url: String,
        /// The agent's Feed API to publish to.
        #[arg(long, value_name = "URL", required_unless_present = "export")]
        to: Option<String>,
        /// The environment variable holding the token the agent gave.
        #[arg(long, default_value = "SION_ACP_FEED_TOKEN", value_name = "NAME")]
        token_env: String,
        /// The feed to fill (made if not given).
        #[arg(long, value_name = "ID")]
        feed: Option<String>,
        /// The country the feed is for (ISO 3166-1 alpha-2).
        #[arg(long, value_name = "CODE")]
        country: Option<String>,
        /// Write the offline snapshot to this directory instead.
        #[arg(long, value_name = "DIR")]
        export: Option<PathBuf>,
    },
}

/// What `sion agent sell serve` takes.
#[derive(Debug, Args)]
pub struct ServeArgs {
    /// The catalog: `{"offers": [{"id", "name", "description",
    /// "price_usd"}]}`, prices in whole cents.
    pub catalog: PathBuf,
    /// Who is paid on the ledger: a wallet's name, or an account (64 hex
    /// digits).
    #[arg(long)]
    pub pay_to: String,
    /// The merchant's name, as buyers and their mandates see it.
    #[arg(long)]
    pub name: String,
    /// Where to listen.
    #[arg(long, default_value = "127.0.0.1:8796", value_name = "ADDRESS")]
    pub listen_at: String,
    /// The URL buyers reach it at (its API base and order links);
    /// http://<listen-at> if not given.
    #[arg(long, value_name = "URL")]
    pub public_url: Option<String>,
    /// The environment variable holding the buyers' bearer tokens,
    /// comma-separated, each `name=token` or a bare token, at least 16
    /// characters.
    #[arg(long, default_value = "SION_ACP_TOKENS", value_name = "NAME")]
    pub tokens_env: String,
    /// The buyers' request-signing keys: `{"<buyer's name>": <JWK>}` (EC
    /// P-256 for ES256, OKP Ed25519, or oct for HMAC-SHA256).
    #[arg(long, value_name = "FILE")]
    pub client_keys: Option<PathBuf>,
    /// Refuse a request from a buyer with a key that is not signed.
    #[arg(long, requires = "client_keys")]
    pub require_signatures: bool,
    /// The chain identity payments are verified against (64 hex digits),
    /// which the node must serve; the node's word if not given.
    #[arg(long, value_name = "HEX")]
    pub chain: Option<String>,
    /// Where the merchant's state is kept (sessions, carts, orders, keys,
    /// the webhook outbox); `acp-merchant-state.json` in the configuration
    /// directory if not given.
    #[arg(long, value_name = "FILE")]
    pub state: Option<PathBuf>,
    /// Take AP2 mandates whose roots these keys signed (a JWK Set file,
    /// `{"keys": [{"kid", "kty": "EC", "crv": "P-256", …}]}`).
    #[arg(long, value_name = "FILE")]
    pub ap2_trust: Option<PathBuf>,
    /// Take AP2 mandates whose roots carry a certificate chain (`x5c`) to
    /// one of these certificates (PEM).
    #[arg(long, value_name = "FILE")]
    pub ap2_roots: Option<PathBuf>,
    /// Complete a session only with an AP2 checkout mandate.
    #[arg(long)]
    pub require_mandate: bool,
    /// Send order webhooks here (the agent platform's
    /// `/agentic_checkout/webhooks/order_events`).
    #[arg(long, value_name = "URL")]
    pub webhook_url: Option<String>,
    /// The environment variable holding the webhook signing secret.
    #[arg(long, default_value = "SION_ACP_WEBHOOK_SECRET", value_name = "NAME")]
    pub webhook_secret_env: String,
    /// The environment variable holding the admin token (fulfill, refund,
    /// list orders); no admin API without it.
    #[arg(long, default_value = "SION_ACP_ADMIN_TOKEN", value_name = "NAME")]
    pub admin_token_env: String,
    /// Refund ledger payments from this wallet (the payee's).
    #[arg(long, value_name = "WALLET")]
    pub refunds_from: Option<String>,
    /// Take cards delegated to this ACP PSP (its base URL).
    #[arg(long, value_name = "URL", requires = "card_merchant_id")]
    pub card_psp: Option<String>,
    /// The environment variable holding this merchant's token at the PSP.
    #[arg(long, default_value = "SION_ACP_PSP_TOKEN", value_name = "NAME")]
    pub card_psp_token_env: String,
    /// Take Stripe shared payment tokens, charged with the secret key in
    /// this environment variable.
    #[arg(
        long,
        value_name = "NAME",
        conflicts_with = "card_psp",
        requires = "card_merchant_id"
    )]
    pub stripe_key_env: Option<String>,
    /// Stripe's API (a stand-in's, for tests).
    #[arg(long, value_name = "URL", hide = true)]
    pub stripe_base: Option<String>,
    /// The merchant's id at its PSP (delegated cards are scoped to it).
    #[arg(long, value_name = "ID")]
    pub card_merchant_id: Option<String>,
    /// The card brands taken.
    #[arg(
        long,
        value_delimiter = ',',
        default_value = "visa,mastercard,amex,discover"
    )]
    pub card_brands: Vec<String>,
    /// Ask for 3-D Secure on card payments: ACP's `AuthenticationMetadata`
    /// (acquirer details, directory server, flow preference) as JSON.
    #[arg(long, value_name = "FILE")]
    pub three_ds: Option<PathBuf>,
    /// Serve this many requests, then stop (for trying it out).
    #[arg(long, value_name = "N")]
    pub requests: Option<usize>,
}

/// The services around a checkout.
#[derive(Debug, Subcommand)]
pub enum CommerceCommand {
    /// Serve a PSP's delegate payment API: agents vault a card for one
    /// checkout, merchants redeem the token once. Its acquirer is
    /// simulated: no card network is reached and no money moves.
    Psp {
        /// Where to listen.
        #[arg(long, default_value = "127.0.0.1:8797", value_name = "ADDRESS")]
        listen_at: String,
        /// The URL it is reached at.
        #[arg(long, value_name = "URL")]
        public_url: Option<String>,
        /// The environment variable holding the agents' tokens.
        #[arg(long, default_value = "SION_PSP_AGENT_TOKENS", value_name = "NAME")]
        agents_env: String,
        /// The environment variable holding the merchants' tokens, each
        /// `merchant_id=token`.
        #[arg(long, default_value = "SION_PSP_MERCHANT_TOKENS", value_name = "NAME")]
        merchants_env: String,
        /// The agents' request-signing keys (`{"<name>": <JWK>}`).
        #[arg(long, value_name = "FILE")]
        client_keys: Option<PathBuf>,
        /// Refuse an agent's request with a key that is not signed.
        #[arg(long, requires = "client_keys")]
        require_signatures: bool,
        /// Serve this many requests, then stop.
        #[arg(long, value_name = "N")]
        requests: Option<usize>,
    },
    /// Serve ACP's Delegate Authentication API (3-D Secure). Its issuer is
    /// simulated: for devnets and tests, its results say so.
    ThreeDs {
        /// Where to listen.
        #[arg(long, default_value = "127.0.0.1:8798", value_name = "ADDRESS")]
        listen_at: String,
        /// The URL it is reached at (its issuer pages are under it).
        #[arg(long, value_name = "URL")]
        public_url: Option<String>,
        /// The environment variable holding the agents' tokens.
        #[arg(long, default_value = "SION_3DS_TOKENS", value_name = "NAME")]
        tokens_env: String,
        /// The agents' request-signing keys (`{"<name>": <JWK>}`).
        #[arg(long, value_name = "FILE")]
        client_keys: Option<PathBuf>,
        /// Refuse an agent's request with a key that is not signed.
        #[arg(long, requires = "client_keys")]
        require_signatures: bool,
        /// Serve this many requests, then stop.
        #[arg(long, value_name = "N")]
        requests: Option<usize>,
    },
    /// Serve an agent's Feed API: merchants publish their products to it.
    Feeds {
        /// Where to listen.
        #[arg(long, default_value = "127.0.0.1:8799", value_name = "ADDRESS")]
        listen_at: String,
        /// The environment variable holding the merchants' tokens.
        #[arg(long, default_value = "SION_ACP_FEED_TOKENS", value_name = "NAME")]
        tokens_env: String,
        /// Where the feeds are kept; `acp-feeds.json` in the configuration
        /// directory if not given.
        #[arg(long, value_name = "FILE")]
        state: Option<PathBuf>,
        /// Serve this many requests, then stop.
        #[arg(long, value_name = "N")]
        requests: Option<usize>,
    },
    /// Receive a merchant's order webhooks: each verified event is printed,
    /// one JSON line each.
    Webhooks {
        /// Where to listen.
        #[arg(long, default_value = "127.0.0.1:8800", value_name = "ADDRESS")]
        listen_at: String,
        /// The environment variable holding the signing secret.
        #[arg(long, default_value = "SION_ACP_WEBHOOK_SECRET", value_name = "NAME")]
        secret_env: String,
        /// Serve this many requests, then stop.
        #[arg(long, value_name = "N")]
        requests: Option<usize>,
    },
    /// Make a request-signing key: written to a file (PKCS#8, readable only
    /// by its owner), its public JWK printed for the services to be given.
    Keygen {
        /// Where to write it.
        out: PathBuf,
        /// Ed25519 instead of ECDSA P-256 (ES256).
        #[arg(long)]
        ed25519: bool,
    },
}

/// What `sion agent buy` takes.
#[derive(Debug, Args)]
pub struct BuyArgs {
    /// The merchant's API base URL.
    pub url: String,
    /// The offers bought, by id (an id twice buys two).
    #[arg(required = true)]
    pub items: Vec<String>,
    /// The wallet that pays (the agent's, with --charter).
    #[arg(long, default_value = "alice")]
    pub wallet: String,
    /// Pay under this Agent Charter, from --allotment.
    #[arg(long, value_name = "ID")]
    pub charter: Option<String>,
    /// The agent's allotment under --charter.
    #[arg(long, value_name = "ID")]
    pub allotment: Option<String>,
    /// The most to pay, in USD.
    #[arg(long, value_name = "USD")]
    pub max_usd: String,
    /// The chain identity payments are checked against (64 hex digits),
    /// which the node must serve; the node's word if not given.
    #[arg(long, value_name = "HEX")]
    pub chain: Option<String>,
    /// The environment variable holding the merchant's token.
    #[arg(long, default_value = "SION_ACP_TOKEN", value_name = "NAME")]
    pub token_env: String,
    /// Sign requests with this key (PKCS#8, PEM or DER: P-256 for ES256,
    /// or Ed25519).
    #[arg(long, value_name = "FILE")]
    pub sign_key: Option<PathBuf>,
    /// Sign requests with HMAC-SHA256, the secret in this environment
    /// variable.
    #[arg(long, value_name = "NAME", conflicts_with = "sign_key")]
    pub sign_secret_env: Option<String>,
    /// Pay by card instead: ACP's `PaymentMethodCard` as JSON (it is sent
    /// to the PSP, never kept).
    #[arg(long, value_name = "FILE", requires = "psp")]
    pub card: Option<PathBuf>,
    /// The PSP the card is delegated to (its base URL).
    #[arg(long, value_name = "URL")]
    pub psp: Option<String>,
    /// The environment variable holding the PSP's token.
    #[arg(long, default_value = "SION_PSP_TOKEN", value_name = "NAME")]
    pub psp_token_env: String,
    /// The 3-D Secure provider, when the merchant or the issuer asks.
    #[arg(long, value_name = "URL")]
    pub three_ds: Option<String>,
    /// The environment variable holding the 3-D Secure provider's token.
    #[arg(long, default_value = "SION_3DS_TOKEN", value_name = "NAME")]
    pub three_ds_token_env: String,
    /// Where the issuer posts back.
    #[arg(
        long,
        value_name = "URL",
        default_value = "https://localhost/3ds/notify"
    )]
    pub notification_url: String,
    /// Answer a simulated issuer's challenge with this code (devnets only:
    /// a real issuer's challenge is the buyer's to answer).
    #[arg(long, value_name = "CODE")]
    pub simulated_otp: Option<String>,
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
    write_private(&path, &key.pkcs8())?;
    Ok(key)
}

/// Writes `bytes` to `path`, readable only by its owner.
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), CliError> {
    let io = |e: std::io::Error| CliError::Usage(format!("{}: {e}", path.display()));
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    std::fs::write(path, bytes).map_err(io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(io)?;
    }
    Ok(())
}

/// The chain payments are checked against: the one pinned, which the node
/// must be serving (a payment made on another chain than the one pinned
/// would move money that no receipt here could ever show), or the node's
/// word if none is pinned.
fn chain_of(ledger: &dyn crate::ledger::Ledger, chain: Option<&str>) -> Result<Hash32, CliError> {
    match crate::skills::pinned_chain(chain)? {
        Some(chain) => {
            let served = crate::verify::served_chain(ledger)?;
            if served != chain {
                return Err(CliError::Usage(format!(
                    "the node serves chain {}, not the {} pinned: check --chain (sion status names it as \"chain\")",
                    served.to_hex(),
                    chain.to_hex()
                )));
            }
            Ok(chain)
        }
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

fn read_json(path: &Path) -> Result<Value, CliError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
    serde_json::from_str(&text).map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))
}

fn secret_of(env: &str, what: &str) -> Result<String, CliError> {
    let secret = std::env::var(env).unwrap_or_default();
    if secret.len() < 16 {
        return Err(CliError::Usage(format!(
            "set {env} to {what} (at least 16 characters)"
        )));
    }
    Ok(secret)
}

/// The clients in `env` (`name=token`, comma-separated), signing with the
/// keys in `keys` if given.
fn clients_from(
    env: &str,
    keys: Option<&Path>,
    require: bool,
) -> Result<Vec<http::Client>, CliError> {
    let mut clients = http::clients_of(&std::env::var(env).unwrap_or_default())?;
    if clients.is_empty() {
        return Err(CliError::Usage(format!(
            "set {env} to the bearer tokens clients are given (comma-separated, name=token)"
        )));
    }
    if let Some(path) = keys {
        let set = read_json(path)?;
        let named = set
            .as_object()
            .ok_or_else(|| CliError::Usage(format!("{}: {{\"<name>\": <JWK>}}", path.display())))?;
        for (name, jwk) in named {
            let verifier = Verifier::from_jwk(jwk)
                .map_err(|e| CliError::Usage(format!("{}: {name}: {e}", path.display())))?;
            let client = clients
                .iter_mut()
                .find(|c| &c.name == name)
                .ok_or_else(|| {
                    CliError::Usage(format!("{}: no client is called {name}", path.display()))
                })?;
            *client = client.clone().signing(verifier, require);
        }
        if require {
            if let Some(unkeyed) = clients.iter().find(|c| c.verifier.is_none()) {
                return Err(CliError::Usage(format!(
                    "signatures are required, and {} has no key in {}",
                    unkeyed.name,
                    path.display()
                )));
            }
        }
    }
    Ok(clients)
}

fn listen(address: &str) -> Result<(tiny_http::Server, String), CliError> {
    let server = tiny_http::Server::http(address)
        .map_err(|e| CliError::Usage(format!("cannot listen on {address}: {e}")))?;
    let bound = server
        .server_addr()
        .to_ip()
        .map(|a| a.to_string())
        .unwrap_or_else(|| address.to_string());
    Ok((server, bound))
}

/// A request-signing key read from `path`: PKCS#8, PEM or DER, P-256 or
/// Ed25519.
pub fn signer_from_file(path: &Path) -> Result<Signer, CliError> {
    let bytes =
        std::fs::read(path).map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
    let der = match std::str::from_utf8(&bytes) {
        Ok(text) if text.contains("-----BEGIN") => {
            use base64::Engine;
            let body: String = text
                .lines()
                .filter(|line| !line.starts_with("-----"))
                .collect();
            base64::engine::general_purpose::STANDARD
                .decode(body.trim())
                .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?
        }
        _ => bytes,
    };
    if let Ok(key) = SigningKey::from_pkcs8(&der) {
        return Ok(Signer::Es256(key));
    }
    Ed25519KeyPair::from_pkcs8(&der)
        .or_else(|_| Ed25519KeyPair::from_pkcs8_maybe_unchecked(&der))
        .map(Signer::Ed25519)
        .map_err(|_| {
            CliError::Usage(format!(
                "{} is neither a P-256 nor an Ed25519 PKCS#8 key",
                path.display()
            ))
        })
}

/// Runs `sion agent sell`.
pub fn sell(ledger: SharedLedger, command: SellCommand) -> Result<(), CliError> {
    match command {
        SellCommand::Serve(args) => serve(ledger, *args),
        SellCommand::Orders {
            url,
            admin_token_env,
        } => {
            let token = secret_of(&admin_token_env, "the merchant's admin token")?;
            let orders = admin(&url, &token, "GET", "/admin/orders", None)?;
            for order in orders["orders"].as_array().into_iter().flatten() {
                let total = order["totals"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|t| t["type"] == "total")
                    .and_then(|t| t["amount"].as_u64())
                    .unwrap_or_default();
                println!(
                    "{}  {}  {}.{:02} USD  session {}",
                    order["id"].as_str().unwrap_or_default(),
                    order["status"].as_str().unwrap_or_default(),
                    total / 100,
                    total % 100,
                    order["checkout_session_id"].as_str().unwrap_or_default(),
                );
            }
            Ok(())
        }
        SellCommand::Fulfill {
            url,
            order,
            access_url,
            license_key,
            description,
            admin_token_env,
        } => {
            let token = secret_of(&admin_token_env, "the merchant's admin token")?;
            let mut body = json!({});
            for (field, value) in [
                ("access_url", access_url),
                ("license_key", license_key),
                ("description", description),
            ] {
                if let Some(value) = value {
                    body[field] = json!(value);
                }
            }
            let done = admin(
                &url,
                &token,
                "POST",
                &format!("/admin/orders/{order}/fulfill"),
                Some(&body),
            )?;
            println!(
                "order {order} is {}",
                done["status"].as_str().unwrap_or_default()
            );
            Ok(())
        }
        SellCommand::Refund {
            url,
            order,
            cents,
            reason,
            admin_token_env,
        } => {
            let token = secret_of(&admin_token_env, "the merchant's admin token")?;
            let mut body = json!({});
            if let Some(cents) = cents {
                body["amount"] = json!(cents);
            }
            if let Some(reason) = reason {
                body["reason"] = json!(reason);
            }
            let done = admin(
                &url,
                &token,
                "POST",
                &format!("/admin/orders/{order}/refund"),
                Some(&body),
            )?;
            let refund = done["adjustments"]
                .as_array()
                .and_then(|a| a.last())
                .cloned()
                .unwrap_or_default();
            println!(
                "refunded {} cents of order {order}: {}",
                refund["amount"],
                refund["description"].as_str().unwrap_or_default()
            );
            Ok(())
        }
        SellCommand::Feed {
            catalog,
            name,
            public_url,
            to,
            token_env,
            feed,
            country,
            export,
        } => {
            let catalog = Catalog::from_json(&read_json(&catalog)?)?;
            let products = feeds::products_of(&catalog, &name, &public_url);
            if let Some(dir) = export {
                let id = feed.clone().unwrap_or_else(|| http::fresh_id("feed"));
                feeds::export(&dir, &id, country.as_deref(), &products)?;
                println!(
                    "wrote feed {id} ({} product(s)) to {}",
                    products.len(),
                    dir.display()
                );
            }
            if let Some(api) = to {
                let token = secret_of(&token_env, "the token the agent's Feed API gave")?;
                let id =
                    feeds::publish(&api, &token, feed.as_deref(), country.as_deref(), &products)?;
                println!(
                    "published {} product(s) to feed {id} at {api}",
                    products.len()
                );
            }
            Ok(())
        }
    }
}

/// Calls a merchant's admin API.
fn admin(
    url: &str,
    token: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<Value, CliError> {
    let client = http::outbound(true)?;
    let bearer = format!("Bearer {token}");
    let text = body.map(Value::to_string);
    let (status, answer) = http::call(
        &client,
        method,
        &format!("{}{path}", url.trim_end_matches('/')),
        &[
            ("Authorization", bearer.as_str()),
            ("Content-Type", "application/json"),
        ],
        text.as_deref().map(str::as_bytes),
    )?;
    if status != 200 {
        return Err(CliError::Protocol(format!(
            "the merchant refused ({status}): {}",
            answer["message"].as_str().unwrap_or_default()
        )));
    }
    Ok(answer)
}

fn serve(ledger: SharedLedger, args: ServeArgs) -> Result<(), CliError> {
    let catalog = Catalog::from_json(&read_json(&args.catalog)?)?;
    let clients = clients_from(
        &args.tokens_env,
        args.client_keys.as_deref(),
        args.require_signatures,
    )?;
    let ledger = ledger.as_ref();
    let chain = chain_of(ledger, args.chain.as_deref())?;
    let mut session = Session::open(ledger)?;
    let pay_to = session.id(&args.pay_to);
    let (server, address) = listen(&args.listen_at)?;
    let base = args
        .public_url
        .clone()
        .unwrap_or_else(|| format!("http://{address}"));
    let orders = crate::model_routes::dir().join("acp-orders.jsonl");
    let state = args
        .state
        .clone()
        .unwrap_or_else(|| crate::model_routes::dir().join("acp-merchant-state.json"));
    let mut merchant = Merchant::new(
        Facilitator::new(ledger, chain)?,
        pay_to,
        &args.name,
        &base,
        catalog.clone(),
        clients,
    )?
    .with_orders(orders.clone());
    let mut said = Vec::new();
    if args.ap2_trust.is_some() || args.ap2_roots.is_some() {
        let mut trust = match &args.ap2_trust {
            Some(path) => {
                Trust::from_jwks(&read_json(path)?).map_err(|e| CliError::Usage(e.to_string()))?
            }
            None => Trust::new(),
        };
        if let Some(path) = &args.ap2_roots {
            let pem = std::fs::read_to_string(path)
                .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
            trust = trust
                .with_roots_pem(&pem)
                .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
        }
        if trust.is_empty() {
            return Err(CliError::Usage(
                "the AP2 trust names no key and no root".into(),
            ));
        }
        merchant = merchant.with_ap2(merchant_key()?, trust, args.require_mandate);
    } else if args.require_mandate {
        return Err(CliError::Usage(
            "--require-mandate needs --ap2-trust or --ap2-roots".into(),
        ));
    }
    let processor: Option<Box<dyn card::Processor>> = match (&args.card_psp, &args.stripe_key_env) {
        (Some(url), _) => {
            let token = secret_of(&args.card_psp_token_env, "this merchant's token at the PSP")?;
            said.push(format!("cards delegated to the PSP at {url}"));
            Some(Box::new(PspProcessor::new("acp-psp", url, &token)?))
        }
        (None, Some(env)) => {
            let key = std::env::var(env)
                .map_err(|_| CliError::Usage(format!("set {env} to the Stripe secret key")))?;
            said.push("Stripe shared payment tokens".to_string());
            Some(Box::new(Stripe::new(&key, args.stripe_base.as_deref())?))
        }
        (None, None) => None,
    };
    match processor {
        Some(processor) => {
            let three_ds = match &args.three_ds {
                Some(path) => {
                    said.push("3-D Secure asked for".to_string());
                    Some(read_json(path)?)
                }
                None => None,
            };
            merchant = merchant.with_card(CardHandler {
                processor,
                merchant_id: args.card_merchant_id.clone().unwrap_or_default(),
                accepted_brands: args.card_brands.clone(),
                three_ds,
            });
        }
        None if args.three_ds.is_some() => {
            return Err(CliError::Usage(
                "--three-ds needs a card backend (--card-psp or --stripe-key-env)".into(),
            ))
        }
        None => {}
    }
    if let Some(url) = &args.webhook_url {
        let secret = secret_of(&args.webhook_secret_env, "the webhook signing secret")?;
        merchant = merchant.with_webhooks(webhooks::Hook::new(url, secret.as_bytes())?);
        said.push(format!("order webhooks to {url}"));
    }
    if let Some(token) = std::env::var(&args.admin_token_env)
        .ok()
        .filter(|t| !t.is_empty())
    {
        merchant = merchant.with_admin(&token)?;
        said.push("admin API on".to_string());
    }
    if let Some(wallet) = &args.refunds_from {
        merchant = merchant.with_refunds(Session::open(ledger)?, wallet);
        said.push(format!("ledger refunds from {wallet}"));
    }
    merchant = merchant.with_state(Store::new(state.clone()))?;
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
            if args.require_mandate {
                " and required"
            } else {
                ""
            },
            key.jwk()
        );
    }
    for line in said {
        println!("{line}");
    }
    println!(
        "state is kept in {}; orders are logged to {}",
        state.display(),
        orders.display()
    );
    acp::serve(&merchant, &server, args.requests);
    Ok(())
}

/// A merchant's order webhooks, received and printed.
struct Receiver {
    secret: Vec<u8>,
    seen: Mutex<std::collections::BTreeSet<String>>,
}

impl Service for Receiver {
    fn answer(&self, request: &Request<'_>) -> Response {
        if (request.method, request.path) != ("POST", "/agentic_checkout/webhooks/order_events") {
            return http::not_found("no such endpoint");
        }
        match webhooks::receive(request, &self.secret, http::now()) {
            Ok(event) => {
                // A redelivery (the same Request-Id) is answered, not
                // printed twice.
                let id = request.header("Request-Id").unwrap_or_default().to_string();
                if id.is_empty() || self.seen.lock().expect("seen lock").insert(id) {
                    println!("{event}");
                }
                Response::json(200, json!({"received": true}))
            }
            Err(refused) => refused,
        }
    }
}

/// Runs `sion agent commerce`.
pub fn commerce(command: CommerceCommand) -> Result<(), CliError> {
    match command {
        CommerceCommand::Psp {
            listen_at,
            public_url,
            agents_env,
            merchants_env,
            client_keys,
            require_signatures,
            requests,
        } => {
            let agents = clients_from(&agents_env, client_keys.as_deref(), require_signatures)?;
            // A merchant's name is its merchant id.
            let merchants: Vec<(http::Client, String)> = clients_from(&merchants_env, None, false)?
                .into_iter()
                .map(|client| {
                    let id = client.name.clone();
                    (client, id)
                })
                .collect();
            let (server, address) = listen(&listen_at)?;
            let base = public_url.unwrap_or_else(|| format!("http://{address}"));
            let served = psp::Psp::new(&base, agents, merchants, Box::new(psp::SimulatedAcquirer))?;
            println!(
                "a PSP's delegate payment API at {base}, for {} merchant(s); its acquirer is simulated: no money moves",
                std::env::var(&merchants_env).unwrap_or_default().split(',').filter(|e| !e.trim().is_empty()).count()
            );
            acp::serve(&served, &server, requests);
            Ok(())
        }
        CommerceCommand::ThreeDs {
            listen_at,
            public_url,
            tokens_env,
            client_keys,
            require_signatures,
            requests,
        } => {
            let clients = clients_from(&tokens_env, client_keys.as_deref(), require_signatures)?;
            let (server, address) = listen(&listen_at)?;
            let base = public_url.unwrap_or_else(|| format!("http://{address}"));
            let served = three_ds::ThreeDs::new(&base, clients)?;
            println!(
                "3-D Secure (Delegate Authentication) at {base}; its issuer is simulated (challenge code {})",
                three_ds::SIMULATED_OTP
            );
            acp::serve(&served, &server, requests);
            Ok(())
        }
        CommerceCommand::Feeds {
            listen_at,
            tokens_env,
            state,
            requests,
        } => {
            let clients = clients_from(&tokens_env, None, false)?;
            let state = state.unwrap_or_else(|| crate::model_routes::dir().join("acp-feeds.json"));
            let host = feeds::FeedHost::new(clients, Some(Store::new(state.clone())))?;
            let (server, address) = listen(&listen_at)?;
            println!(
                "a Feed API at http://{address}, kept in {}",
                state.display()
            );
            acp::serve(&host, &server, requests);
            Ok(())
        }
        CommerceCommand::Webhooks {
            listen_at,
            secret_env,
            requests,
        } => {
            let secret = secret_of(&secret_env, "the webhook signing secret")?;
            let (server, address) = listen(&listen_at)?;
            eprintln!("receiving order webhooks at http://{address}/agentic_checkout/webhooks/order_events");
            let receiver = Receiver {
                secret: secret.into_bytes(),
                seen: Mutex::new(Default::default()),
            };
            acp::serve(&receiver, &server, requests);
            Ok(())
        }
        CommerceCommand::Keygen { out, ed25519 } => {
            let (pkcs8, signer) = if ed25519 {
                let rng = aws_lc_rs::rand::SystemRandom::new();
                let document = Ed25519KeyPair::generate_pkcs8(&rng)
                    .map_err(|_| CliError::Usage("an Ed25519 key could not be made".into()))?;
                let bytes = document.as_ref().to_vec();
                let pair = Ed25519KeyPair::from_pkcs8(&bytes)
                    .map_err(|_| CliError::Usage("an Ed25519 key could not be read back".into()))?;
                (bytes, Signer::Ed25519(pair))
            } else {
                let key = SigningKey::generate();
                (key.pkcs8(), Signer::Es256(key))
            };
            write_private(&out, &pkcs8)?;
            println!("{}", signer.jwk().unwrap_or_default());
            Ok(())
        }
    }
}

/// Runs `sion agent buy`: buys `items` from the ACP merchant at `url`.
pub fn buy(ledger: SharedLedger, args: BuyArgs) -> Result<(), CliError> {
    let token = std::env::var(&args.token_env).map_err(|_| {
        CliError::Usage(format!(
            "set {} to the token the merchant gave you",
            args.token_env
        ))
    })?;
    let signer = match (&args.sign_key, &args.sign_secret_env) {
        (Some(path), _) => Some(signer_from_file(path)?),
        (None, Some(env)) => Some(Signer::hmac(
            secret_of(env, "the signing secret")?.as_bytes(),
        )),
        (None, None) => None,
    };
    let names: Vec<&str> = args.items.iter().map(String::as_str).collect();
    let max_price = crate::agents::parse_amount(&args.max_usd)?;
    let done = match &args.card {
        Some(path) => {
            let method = read_json(path)?;
            let psp_url = args.psp.as_deref().unwrap_or_default();
            let psp_token = secret_of(&args.psp_token_env, "the token the PSP gave you")?;
            let card = buyer::Card {
                psp_url,
                psp_token: &psp_token,
                payment_method: &method,
                billing_address: None,
            };
            let buyer = Buyer {
                base_url: &args.url,
                token: &token,
                chain: Hash32::default(),
                max_price,
                signer: signer.as_ref(),
            };
            let otp = args.simulated_otp.clone();
            let challenge = move |challenge: &Value| -> Result<(), CliError> {
                match &otp {
                    Some(code) => buyer::answer_simulated_challenge(challenge, code),
                    None => Err(CliError::Usage(
                        "the issuer asks for a challenge, which only the buyer can answer (--simulated-otp answers a simulated issuer's)".into(),
                    )),
                }
            };
            let three_ds_token = match &args.three_ds {
                Some(_) => Some(secret_of(
                    &args.three_ds_token_env,
                    "the token the 3-D Secure provider gave you",
                )?),
                None => None,
            };
            let authenticator = match (&args.three_ds, &three_ds_token) {
                (Some(url), Some(token)) => Some(buyer::Authenticator {
                    url,
                    token,
                    notification_url: &args.notification_url,
                    channel: None,
                    challenge: &challenge,
                }),
                _ => None,
            };
            acp::buy_by_card(&buyer, &names, &card, authenticator.as_ref())?
        }
        None => {
            let ledger = ledger.as_ref();
            let chain = chain_of(ledger, args.chain.as_deref())?;
            let payer = match (&args.charter, &args.allotment) {
                (Some(charter), Some(allotment)) => Payer::Charter {
                    agent: args.wallet.clone(),
                    charter: crate::agents::parse_object(charter, "the charter")?,
                    allotment: crate::agents::parse_object(allotment, "the allotment")?,
                },
                (None, None) => Payer::Wallet(args.wallet.clone()),
                _ => {
                    return Err(CliError::Usage(
                        "a charter payment names both --charter and --allotment".into(),
                    ))
                }
            };
            let mut session = Session::open(ledger)?;
            let buyer = Buyer {
                base_url: &args.url,
                token: &token,
                chain,
                max_price,
                signer: signer.as_ref(),
            };
            acp::buy(&mut session, &payer, &buyer, &names, |_| Ok(None))?
        }
    };
    let total = buyer::total_of(&done).unwrap_or_default();
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
