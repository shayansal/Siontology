//! X's API v2 as sion's X tools reach it: a user's OAuth 2.0 access token
//! (`X_ACCESS_TOKEN`), renewed with `X_REFRESH_TOKEN` and `X_CLIENT_ID`
//! (and `X_CLIENT_SECRET` for a confidential app) when X refuses it.
//!
//! X rotates refresh tokens: each renewal gives a new one and ends the one
//! used. So the pair a renewal gives is kept sealed on this machine
//! (`agents/x-tokens.bin`, as the other stores), named by the digest of the
//! refresh token it descends from, and used again after a restart for as
//! long as `X_REFRESH_TOKEN` is the one it descends from; a new
//! `X_REFRESH_TOKEN` (the account authorized again) starts afresh. Direct
//! messages ([`crate::x_dm`]), mentions and search
//! ([`crate::x_mentions`]) share it.

use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// X's API, unless `X_API_BASE` names another.
pub const X_API: &str = "https://api.x.com";

/// The file the renewed tokens are kept in.
const KEPT: &str = "x-tokens.bin";

/// What the file starts with.
const MAGIC: &[u8] = b"sion-x-tokens/v1\n";

/// The access and refresh tokens, as they last were.
#[derive(Clone, Default, Serialize, Deserialize)]
struct Tokens {
    access: String,
    refresh: Option<String>,
    /// The digest of the refresh token these descend from.
    #[serde(default)]
    origin: String,
}

impl Drop for Tokens {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.access);
        if let Some(refresh) = &mut self.refresh {
            zeroize::Zeroize::zeroize(refresh);
        }
    }
}

fn digest(token: &str) -> String {
    sion_http::sha256(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// An X account, as its token reaches it.
pub struct XApi {
    client: sion_http::Client,
    base: String,
    tokens: Mutex<Tokens>,
    client_id: Option<String>,
    client_secret: Option<String>,
    me: Mutex<Option<(String, String)>>,
}

impl std::fmt::Debug for XApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "XApi({})", self.base)
    }
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

impl XApi {
    /// The account whose token is in `X_ACCESS_TOKEN`, with the tokens a
    /// renewal kept if they descend from `X_REFRESH_TOKEN`.
    pub fn from_env(what: &str) -> Result<XApi, String> {
        let access = env("X_ACCESS_TOKEN")
            .ok_or_else(|| format!("set X_ACCESS_TOKEN (a user's OAuth 2.0 token, with {what})"))?;
        let refresh = env("X_REFRESH_TOKEN");
        let origin = refresh.as_deref().map(digest).unwrap_or_default();
        let kept: Tokens =
            crate::secrets::open_json(&crate::secrets::dir(), KEPT, MAGIC).unwrap_or_default();
        let tokens = if !origin.is_empty() && kept.origin == origin && !kept.access.is_empty() {
            kept
        } else {
            Tokens {
                access,
                refresh,
                origin,
            }
        };
        Ok(XApi {
            client: sion_http::Client::new(sion_http::Options {
                timeout: Duration::from_secs(30),
                allow_private: true,
                ..sion_http::Options::default()
            })
            .map_err(|e| e.to_string())?,
            base: env("X_API_BASE")
                .unwrap_or_else(|| X_API.into())
                .trim_end_matches('/')
                .to_string(),
            tokens: Mutex::new(tokens),
            client_id: env("X_CLIENT_ID"),
            client_secret: env("X_CLIENT_SECRET"),
            me: Mutex::new(None),
        })
    }

    /// Whether the environment gives X's tools a token.
    pub fn configured() -> bool {
        env("X_ACCESS_TOKEN").is_some()
    }

    /// A new access token from the refresh token, kept sealed with the new
    /// refresh token.
    fn renew(&self) -> Result<(), String> {
        let mut tokens = self.tokens.lock().unwrap_or_else(|e| e.into_inner());
        let (Some(refresh), Some(client_id)) = (tokens.refresh.clone(), self.client_id.clone())
        else {
            return Err(
                "X refused the access token, and there is no refresh token to renew it".into(),
            );
        };
        let body = crate::webhooks::encode_form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh),
            ("client_id", &client_id),
        ]);
        let basic = self.client_secret.as_ref().map(|secret| {
            format!(
                "Basic {}",
                sion_http::base64(format!("{client_id}:{secret}").as_bytes())
            )
        });
        let mut headers = vec![("Content-Type", "application/x-www-form-urlencoded")];
        if let Some(basic) = &basic {
            headers.push(("Authorization", basic));
        }
        let fetched = self
            .client
            .request(
                "POST",
                &format!("{}/2/oauth2/token", self.base),
                &headers,
                Some(body.as_bytes()),
                1 << 16,
            )
            .map_err(|e| e.to_string().replace(&refresh, "<token>"))?;
        let answer: Value = serde_json::from_slice(&fetched.body).map_err(|e| e.to_string())?;
        tokens.access = answer["access_token"]
            .as_str()
            .ok_or("X gave no access token")?
            .to_string();
        if let Some(next) = answer["refresh_token"].as_str() {
            tokens.refresh = Some(next.to_string());
        }
        // Kept, so that a restart does not use the refresh token X ended.
        if let Err(e) = crate::secrets::seal_json(&crate::secrets::dir(), KEPT, MAGIC, &*tokens) {
            eprintln!("x: the renewed tokens could not be kept: {e}");
        }
        Ok(())
    }

    /// `method path` (a path with its query) with the access token, renewed
    /// once if X refuses it; the answer as JSON.
    pub fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, XError> {
        for renewed in [false, true] {
            let access = self
                .tokens
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .access
                .clone();
            let auth = format!("Bearer {access}");
            let bytes = body.map(|b| b.to_string().into_bytes());
            let mut headers = vec![("Authorization", auth.as_str())];
            if bytes.is_some() {
                headers.push(("Content-Type", "application/json"));
            }
            match self.client.request(
                method,
                &format!("{}{path}", self.base),
                &headers,
                bytes.as_deref(),
                4 << 20,
            ) {
                Ok(fetched) => {
                    return serde_json::from_slice(&fetched.body).map_err(|e| {
                        XError::Other(format!("X answered with something other than JSON: {e}"))
                    })
                }
                Err(sion_http::HttpError::Status { status: 401, .. }) if !renewed => {
                    self.renew().map_err(XError::Other)?
                }
                Err(sion_http::HttpError::Status { status, body, .. }) => {
                    return Err(XError::Status(status, body.replace(&access, "<token>")))
                }
                Err(e) => return Err(XError::Other(e.to_string().replace(&access, "<token>"))),
            }
        }
        Err(XError::Other("X refused the access token".into()))
    }

    /// The account's id and username.
    pub fn me(&self) -> Result<(String, String), String> {
        if let Some(me) = self.me.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return Ok(me);
        }
        let me = self
            .call("GET", "/2/users/me", None)
            .map_err(|e| e.to_string())?;
        let id = me["data"]["id"]
            .as_str()
            .ok_or("X did not say who the token is for")?
            .to_string();
        let name = me["data"]["username"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        *self.me.lock().unwrap_or_else(|e| e.into_inner()) = Some((id.clone(), name.clone()));
        Ok((id, name))
    }
}

/// How a call to X failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum XError {
    /// X answered with this status, and the start of its body.
    Status(u16, String),
    /// Anything else.
    Other(String),
}

impl std::fmt::Display for XError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            XError::Status(status, body) => write!(f, "X answered {status}: {body}"),
            XError::Other(why) => write!(f, "{why}"),
        }
    }
}
