//! Sign in with ChatGPT: `sion agent chatgpt login`, `status`, `models` and
//! `logout`, and the models `chatgpt:<model>`, which run on the ChatGPT
//! plan of the person who signed in, through OpenAI's Responses API.
//!
//! This is OpenAI's flow for open-source apps
//! (<https://developers.openai.com/siwc>): an OpenID Connect sign-in in the
//! person's browser, with PKCE, back to a loopback address on 127.0.0.1;
//! this app registered with OpenAI on its first sign-in on a machine
//! (`dynamic_agent_client`), with no client secret. The ID token is checked
//! against OpenAI's published keys, with its issuer, audience, expiry and
//! nonce; the plan is used only if the person shared it
//! (`chatgpt.tokens.use.direct`). The tokens are kept encrypted beside the
//! secrets ([`crate::secrets`]), and renewed before they lapse, the refresh
//! token with them. Inference goes to `POST /v1/responses`, streamed and
//! not stored, as the flow requires.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sion_agent::model::ModelError;

use crate::error::CliError;

/// OpenAI's sign-in.
pub const ISSUER: &str = "https://auth.openai.com";

/// What the tokens are for: OpenAI's API.
pub const RESOURCE: &str = "https://api.openai.com/v1";

/// The scopes asked for: who the person is, and their plan's use.
pub const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";

/// The scope that shares the person's plan with this app.
pub const PLAN: &str = "chatgpt.tokens.use.direct";

/// The loopback port the sign-in comes back to, if it is free (another
/// one otherwise: only the port may change).
pub const PORT: u16 = 1455;

/// The path it comes back to.
pub const CALLBACK: &str = "/auth/callback";

/// This app's name, as OpenAI's sign-in shows it.
pub const NAME: &str = "Siontology";

/// What the store's file starts with, and what its encryption is bound to.
pub const MAGIC: &[u8] = b"sion-agent-chatgpt/v1\n";

/// The file in the store's directory.
pub const FILE: &str = "chatgpt.bin";

/// How long a person has to sign in, in seconds.
pub const LOGIN_SECS: u64 = 600;

/// How long before it lapses an access token is renewed, in seconds.
pub const RENEW_MARGIN_SECS: u64 = 120;

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Where the sign-in is: OpenAI's, or `CHATGPT_AUTH_URL`'s (a stand-in's).
pub fn issuer() -> String {
    env("CHATGPT_AUTH_URL").unwrap_or_else(|| ISSUER.to_string())
}

/// Where the models are: OpenAI's API, or `CHATGPT_API_URL`'s.
pub fn api() -> String {
    env("CHATGPT_API_URL").unwrap_or_else(|| RESOURCE.to_string())
}

/// What signing in left on this machine.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Store {
    /// This machine, to OpenAI's sign-in (`ext_agent_host_id`), made on its
    /// first sign-in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// The client OpenAI registered for this app on this machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// Who is signed in, if anyone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<Account>,
}

/// A person signed in, and their tokens.
#[derive(Clone, Serialize, Deserialize)]
pub struct Account {
    /// Who they are to OpenAI: the ID token's subject.
    pub subject: String,
    /// Their email, if the ID token gives it.
    #[serde(default)]
    pub email: Option<String>,
    id_token: String,
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    /// When the access token lapses, in seconds since the epoch.
    #[serde(default)]
    pub expires_at: Option<u64>,
    /// The scopes granted.
    pub scope: String,
}

impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Account")
            .field("subject", &self.subject)
            .field("email", &self.email)
            .field("expires_at", &self.expires_at)
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

impl Account {
    /// Whether they shared their plan with this app.
    pub fn shares_plan(&self) -> bool {
        self.scope.split_whitespace().any(|scope| scope == PLAN)
    }
}

/// What is kept on this machine.
pub fn load() -> Result<Store, CliError> {
    crate::secrets::open_json(&crate::secrets::dir(), FILE, MAGIC)
}

fn save(store: &Store) -> Result<(), CliError> {
    crate::secrets::seal_json(&crate::secrets::dir(), FILE, MAGIC, store)
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn usage(e: impl std::fmt::Display) -> CliError {
    CliError::Usage(e.to_string())
}

/// Random text, URL-safe, of `bytes` random bytes.
fn random(bytes: usize) -> Result<String, CliError> {
    let mut buffer = vec![0u8; bytes];
    sion_http::random(&mut buffer).map_err(usage)?;
    Ok(sion_agent::encoding::base64_url(&buffer))
}

/// The client the sign-in is reached with: a stand-in's on this machine
/// may be plain HTTP.
fn client() -> Result<sion_http::Client, CliError> {
    let local = sion_agent::oauth::is_loopback(&issuer());
    sion_http::Client::new(sion_http::Options {
        allow_private: local,
        timeout: Duration::from_secs(30),
        ..sion_http::Options::default()
    })
    .map_err(usage)
}

/// The query of `pairs`, each part encoded.
fn form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(name, value)| {
            format!(
                "{}={}",
                sion_agent::oauth::encode(name),
                sion_agent::oauth::encode(value)
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// Asks the sign-in's token endpoint, with `pairs` as the form: its JSON
/// answer, or why there is none.
fn token_request(pairs: &[(&str, &str)]) -> Result<Value, String> {
    let body = form(pairs);
    let answered = client().map_err(|e| e.to_string())?.request(
        "POST",
        &format!("{}/api/accounts/oauth/token", issuer()),
        &[("Content-Type", "application/x-www-form-urlencoded")],
        Some(body.as_bytes()),
        1 << 20,
    );
    match answered {
        Ok(fetched) => serde_json::from_slice(&fetched.body)
            .map_err(|e| format!("the sign-in answered with something other than JSON: {e}")),
        Err(sion_http::HttpError::Status { status, body, .. }) => {
            let reason = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| v["error"].as_str().map(str::to_string))
                .unwrap_or(body);
            Err(format!("the sign-in refused (HTTP {status}): {reason}"))
        }
        Err(e) => Err(format!("the sign-in could not be reached: {e}")),
    }
}

/// The claims of `id_token`, once it is shown to be OpenAI's (signed with
/// one of the keys its discovery document names), for this app's
/// `client_id`, not expired, and carrying `nonce`.
fn checked(id_token: &str, client_id: &str, nonce: &str) -> Result<Value, String> {
    let client = client().map_err(|e| e.to_string())?;
    let issuer = issuer();
    let get = |url: &str| -> Result<Value, String> {
        let fetched = client.get(url, 1 << 20).map_err(|e| e.to_string())?;
        serde_json::from_slice(&fetched.body).map_err(|e| format!("{url}: {e}"))
    };
    let discovery = get(&format!("{issuer}/.well-known/openid-configuration"))?;
    let jwks = discovery["jwks_uri"]
        .as_str()
        .ok_or("the sign-in's discovery document names no keys")?;
    let keys = crate::jwt::keys(&get(jwks)?);
    let (claims, _) = crate::jwt::verified(id_token, &keys)?;
    crate::jwt::check(&claims, &[issuer.as_str()], client_id, now())?;
    if claims["nonce"].as_str() != Some(nonce) {
        return Err("the ID token is not for this sign-in (its nonce differs)".into());
    }
    Ok(claims)
}

/// Waits on `listener` for the sign-in to come back with `state`: its
/// answer's query, parsed.
fn wait(listener: &tiny_http::Server, state: &str) -> Result<BTreeMap<String, String>, String> {
    let deadline = Instant::now() + Duration::from_secs(LOGIN_SECS);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(format!(
                "no sign-in came back within {} minutes: run it again",
                LOGIN_SECS / 60
            ));
        }
        let Ok(Some(request)) = listener.recv_timeout(left) else {
            continue;
        };
        let target = request.url().to_string();
        let (path, query) = target.split_once('?').unwrap_or((&target, ""));
        if path != CALLBACK {
            let _ = request
                .respond(tiny_http::Response::from_string("not found").with_status_code(404));
            continue;
        }
        let answer = sion_agent::oauth::query(query);
        if answer.get("state").map(String::as_str) != Some(state) {
            let _ = request.respond(
                tiny_http::Response::from_string("This is not the sign-in under way.")
                    .with_status_code(400),
            );
            continue;
        }
        let page = if answer.contains_key("code") {
            "Signed in. You can close this tab and go back to the terminal."
        } else {
            "The sign-in was not completed. You can close this tab."
        };
        let _ = request.respond(
            tiny_http::Response::from_string(page).with_header(
                "Content-Type: text/plain; charset=utf-8"
                    .parse::<tiny_http::Header>()
                    .expect("a valid header"),
            ),
        );
        return Ok(answer);
    }
}

/// Signs in with ChatGPT: shows the address to open (and opens it, with
/// `open`), waits for the sign-in to come back, takes and checks its
/// tokens, and keeps them. What it says of who signed in.
pub fn login(open: bool) -> Result<String, CliError> {
    login_with(&mut |url| {
        println!("Continue with ChatGPT: sign in at\n\n  {url}\n");
        if open && crate::mcp_auth::open_browser(url) {
            println!("(opened in your browser)");
        }
    })
}

/// [`login`], the address to sign in at given to `show`.
pub fn login_with(show: &mut dyn FnMut(&str)) -> Result<String, CliError> {
    let mut store = load()?;
    let host = match &store.host {
        Some(host) => host.clone(),
        None => format!("sion-{}", random(16)?),
    };
    let listener = tiny_http::Server::http(("127.0.0.1", PORT))
        .or_else(|_| tiny_http::Server::http(("127.0.0.1", 0)))
        .map_err(|e| usage(format!("cannot listen on 127.0.0.1 for the sign-in: {e}")))?;
    let port = listener
        .server_addr()
        .to_ip()
        .map(|address| address.port())
        .ok_or_else(|| usage("the sign-in's listener has no port"))?;
    let redirect = format!("http://127.0.0.1:{port}{CALLBACK}");
    let (state, nonce, verifier) = (random(24)?, random(24)?, random(32)?);
    let challenge = sion_agent::oauth::challenge_of(&verifier);
    let client_id = store
        .client_id
        .clone()
        .unwrap_or_else(|| "dynamic_agent_client".to_string());
    let mut pairs: Vec<(&str, &str)> = vec![
        ("response_type", "code"),
        ("client_id", &client_id),
        ("ext_agent_host_id", &host),
        ("redirect_uri", &redirect),
        ("scope", SCOPES),
        ("resource", RESOURCE),
        ("state", &state),
        ("nonce", &nonce),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
    ];
    // Registered on the first sign-in, under this app's name.
    if store.client_id.is_none() {
        pairs.push(("agent_name_hint", NAME));
    }
    let hints: Option<(String, Option<String>)> = store
        .account
        .as_ref()
        .map(|a| (a.id_token.clone(), a.email.clone()));
    if let Some((id_token, email)) = &hints {
        pairs.push(("id_token_hint", id_token));
        if let Some(email) = email {
            pairs.push(("login_hint", email));
        }
    }
    let url = format!("{}/api/accounts/authorize?{}", issuer(), form(&pairs));
    show(&url);
    let answer = wait(&listener, &state).map_err(usage)?;
    if let Some(error) = answer.get("error") {
        return Err(usage(format!(
            "the sign-in was not completed: {error}{}",
            answer
                .get("error_description")
                .map(|d| format!(" ({d})"))
                .unwrap_or_default()
        )));
    }
    let code = answer
        .get("code")
        .filter(|code| !code.is_empty())
        .ok_or_else(|| usage("the sign-in came back without a code"))?;
    let client_id = match (answer.get("client_id"), &store.client_id) {
        (Some(issued), _) if !issued.is_empty() => issued.clone(),
        (_, Some(kept)) => kept.clone(),
        _ => {
            return Err(usage(
                "the sign-in did not register this app: no client id came back",
            ))
        }
    };
    let tokens = token_request(&[
        ("grant_type", "authorization_code"),
        ("client_id", &client_id),
        ("code", code),
        ("code_verifier", &verifier),
        ("redirect_uri", &redirect),
        ("resource", RESOURCE),
    ])
    .map_err(usage)?;
    let id_token = tokens["id_token"]
        .as_str()
        .ok_or_else(|| usage("the sign-in gave no ID token"))?;
    let claims = checked(id_token, &client_id, &nonce).map_err(usage)?;
    let access_token = tokens["access_token"]
        .as_str()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| usage("the sign-in gave no access token"))?;
    let account = Account {
        subject: claims["sub"]
            .as_str()
            .ok_or_else(|| usage("the ID token names no one"))?
            .to_string(),
        email: claims["email"].as_str().map(str::to_string),
        id_token: id_token.to_string(),
        access_token: access_token.to_string(),
        refresh_token: tokens["refresh_token"].as_str().map(str::to_string),
        expires_at: tokens["expires_in"]
            .as_u64()
            .map(|secs| now().saturating_add(secs)),
        // Granted as asked, unless the answer says otherwise.
        scope: tokens["scope"].as_str().unwrap_or(SCOPES).to_string(),
    };
    let said = describe(&account);
    store.host = Some(host);
    store.client_id = Some(client_id);
    store.account = Some(account);
    save(&store)?;
    Ok(said)
}

/// What an account is, in a line or two.
fn describe(account: &Account) -> String {
    let who = account.email.as_deref().unwrap_or(&account.subject);
    if account.shares_plan() {
        format!("Signed in with ChatGPT as {who}: chatgpt:<model> runs on that plan (`sion agent chatgpt models` lists them).")
    } else {
        format!("Signed in with ChatGPT as {who}, but the plan was not shared ({PLAN}): chatgpt:<model> will not run. Sign in again and allow it.")
    }
}

/// Who is signed in, if anyone.
pub fn status() -> Result<String, CliError> {
    Ok(match load()?.account {
        Some(account) => {
            let lasts = match account.expires_at {
                Some(at) if at > now() => {
                    format!(
                        " Its access token lasts {} more minutes, and is renewed before it lapses.",
                        (at - now()) / 60
                    )
                }
                Some(_) => " Its access token has lapsed; it is renewed when next used.".into(),
                None => String::new(),
            };
            format!("{}{lasts}", describe(&account))
        }
        None => "No one is signed in with ChatGPT here: `sion agent chatgpt login`.".into(),
    })
}

/// Forgets who signed in. This machine's registration with OpenAI stays,
/// for the next sign-in. Whether anyone was signed in.
pub fn logout() -> Result<bool, CliError> {
    let mut store = load()?;
    let was = store.account.take().is_some();
    if was {
        save(&store)?;
    }
    Ok(was)
}

/// Held while tokens are renewed: by one thread at a time.
static RENEWING: Mutex<()> = Mutex::new(());

/// The access token to use: the one kept, or, if it is about to lapse or
/// `fresh` asks for it (the API refused the last), a new one from the
/// refresh token, which comes back new too and is kept with it.
pub fn token(fresh: bool) -> Result<String, ModelError> {
    let _renewing = RENEWING.lock().unwrap_or_else(|e| e.into_inner());
    let refused = |why: String| ModelError::Refused(why);
    let mut store = load().map_err(|e| refused(e.to_string()))?;
    let client_id = store.client_id.clone();
    let account = store.account.as_mut().ok_or_else(|| {
        refused("no one is signed in with ChatGPT here: `sion agent chatgpt login`".into())
    })?;
    if !account.shares_plan() {
        return Err(refused(format!(
            "the ChatGPT sign-in did not share the plan ({PLAN}): sign in again and allow it"
        )));
    }
    let lapsing = account
        .expires_at
        .is_some_and(|at| now().saturating_add(RENEW_MARGIN_SECS) >= at);
    if !fresh && !lapsing {
        return Ok(account.access_token.clone());
    }
    let refresh = account.refresh_token.clone().ok_or_else(|| {
        refused(
            "the ChatGPT sign-in gave no refresh token: `sion agent chatgpt login` again".into(),
        )
    })?;
    let client_id = client_id
        .ok_or_else(|| refused("this machine is not registered with ChatGPT's sign-in".into()))?;
    let tokens = token_request(&[
        ("grant_type", "refresh_token"),
        ("client_id", &client_id),
        ("refresh_token", &refresh),
        ("resource", RESOURCE),
    ])
    .map_err(|why| {
        if why.contains("could not be reached") {
            ModelError::Transport(why)
        } else {
            refused(format!("{why}: `sion agent chatgpt login` again"))
        }
    })?;
    let access_token = tokens["access_token"]
        .as_str()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| refused("the sign-in renewed no access token".into()))?
        .to_string();
    // Replaced together: the access token, its expiry, the scopes and the
    // refresh token, which rotates.
    account.access_token = access_token.clone();
    account.expires_at = tokens["expires_in"]
        .as_u64()
        .map(|secs| now().saturating_add(secs));
    if let Some(scope) = tokens["scope"].as_str() {
        account.scope = scope.to_string();
    }
    if let Some(rotated) = tokens["refresh_token"].as_str() {
        account.refresh_token = Some(rotated.to_string());
    }
    if let Some(id_token) = tokens["id_token"].as_str() {
        account.id_token = id_token.to_string();
    }
    save(&store).map_err(|e| refused(e.to_string()))?;
    Ok(access_token)
}

/// The models the signed-in person's plan offers: each one's name to give
/// as `chatgpt:<name>`, and how ChatGPT shows it.
pub fn models() -> Result<Vec<(String, String)>, CliError> {
    let token = token(false).map_err(|e| usage(e.to_string()))?;
    let bearer = format!("Bearer {token}");
    let local = sion_agent::oauth::is_loopback(&api());
    let client = sion_http::Client::new(sion_http::Options {
        allow_private: local,
        timeout: Duration::from_secs(30),
        ..sion_http::Options::default()
    })
    .map_err(usage)?;
    let fetched = client
        .request(
            "GET",
            &format!("{}/models", api().trim_end_matches('/')),
            &[("Authorization", bearer.as_str())],
            None,
            4 << 20,
        )
        .map_err(|e| usage(format!("the models could not be listed: {e}")))?;
    let listed: Value = serde_json::from_slice(&fetched.body).map_err(usage)?;
    Ok(listed["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["visibility"] == "list")
        .filter_map(|m| {
            let slug = m["slug"].as_str()?.to_string();
            let shown = m["display_name"].as_str().unwrap_or(&slug).to_string();
            Some((slug, shown))
        })
        .collect())
}

/// `sion agent chatgpt …`
#[derive(Debug, clap::Subcommand)]
pub enum ChatgptCommand {
    /// Sign in with your ChatGPT account, in your browser, so that agents
    /// may run on your plan (`--model chatgpt:<model>`).
    Login {
        /// Show the address only: do not open a browser.
        #[arg(long)]
        no_browser: bool,
    },
    /// Who is signed in, and whether the plan is shared.
    Status,
    /// The models your plan offers.
    Models,
    /// Forget who signed in.
    Logout,
}

/// Runs a ChatGPT command.
pub fn run(command: ChatgptCommand) -> Result<(), CliError> {
    match command {
        ChatgptCommand::Login { no_browser } => println!("{}", login(!no_browser)?),
        ChatgptCommand::Status => println!("{}", status()?),
        ChatgptCommand::Models => {
            let models = models()?;
            if models.is_empty() {
                println!("Your plan offers no models to this app.");
            }
            for (slug, shown) in models {
                println!("chatgpt:{slug}  ({shown})");
            }
        }
        ChatgptCommand::Logout => println!(
            "{}",
            if logout()? {
                "Signed out of ChatGPT here."
            } else {
                "No one was signed in with ChatGPT here."
            }
        ),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_is_used_only_if_it_was_shared_and_tokens_are_never_shown() {
        let mut account = Account {
            subject: "user-1".into(),
            email: Some("ann@example.com".into()),
            id_token: "id".into(),
            access_token: "secret-access".into(),
            refresh_token: Some("secret-refresh".into()),
            expires_at: None,
            scope: "openid profile email".into(),
        };
        assert!(!account.shares_plan());
        assert!(describe(&account).contains("not shared"));
        account.scope = SCOPES.into();
        assert!(account.shares_plan());
        let shown = format!("{account:?}");
        assert!(!shown.contains("secret"), "{shown}");
        assert!(describe(&account).contains("ann@example.com"));
    }

    #[test]
    fn a_form_is_encoded_part_by_part() {
        assert_eq!(
            form(&[
                ("scope", "openid email"),
                ("redirect_uri", "http://127.0.0.1:1455/auth/callback")
            ]),
            "scope=openid%20email&redirect_uri=http%3A%2F%2F127.0.0.1%3A1455%2Fauth%2Fcallback"
        );
    }
}
