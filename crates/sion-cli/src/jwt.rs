//! JSON Web Tokens signed with RS256: checked against a JSON Web Key Set
//! (the keys Microsoft's Bot Framework and Google sign their chats'
//! requests with), and made with an RSA key (a Google service account's),
//! with the TLS stack's cryptography ([`sion_http::rsa_sha256_verify`],
//! [`sion_http::rsa_sha256_sign`]).

use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::{json, Value};

/// How long a key set is kept before it is fetched again.
const KEPT: Duration = Duration::from_secs(24 * 3600);

/// The least time between two fetches of a key set for a key it lacks.
const AGAIN: Duration = Duration::from_secs(60);

/// The clock skew allowed around a token's times, in seconds (the
/// industry's five minutes).
pub const SKEW: u64 = 300;

/// An RSA key of a key set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    /// Its `kid`.
    pub id: Option<String>,
    n: Vec<u8>,
    e: Vec<u8>,
    /// The channels it is endorsed for (the Bot Framework's
    /// `endorsements`).
    pub endorsements: Vec<String>,
}

fn base64url(text: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(text.trim_end_matches('=')).ok()
}

/// The RSA signing keys of a JSON Web Key Set (`{"keys": […]}`).
pub fn keys(set: &Value) -> Vec<Key> {
    set["keys"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|k| k["kty"] == "RSA" && k["use"].as_str().map_or(true, |u| u == "sig"))
        .filter_map(|k| {
            Some(Key {
                id: k["kid"].as_str().map(str::to_string),
                n: base64url(k["n"].as_str()?)?,
                e: base64url(k["e"].as_str()?)?,
                endorsements: k["endorsements"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e.as_str().map(str::to_string))
                    .collect(),
            })
        })
        .collect()
}

/// A token's header and claims, what is signed, and the signature.
fn parts(token: &str) -> Result<(Value, Value, &str, Vec<u8>), String> {
    let mut pieces = token.split('.');
    let (Some(header), Some(claims), Some(signature), None) =
        (pieces.next(), pieces.next(), pieces.next(), pieces.next())
    else {
        return Err("not a signed token".into());
    };
    let json = |piece: &str| -> Result<Value, String> {
        let bytes = base64url(piece).ok_or("a token not in base64url")?;
        serde_json::from_slice(&bytes).map_err(|_| "a token not made of JSON".to_string())
    };
    let signed = &token[..header.len() + 1 + claims.len()];
    let signature = base64url(signature).ok_or("a signature not in base64url")?;
    Ok((json(header)?, json(claims)?, signed, signature))
}

/// The key a token's header names (`kid`), if it names one.
fn key_id(token: &str) -> Option<String> {
    let (header, _, _, _) = parts(token).ok()?;
    header["kid"].as_str().map(str::to_string)
}

/// A token's claims, once its RS256 signature is found to be by a key of
/// `keys` (the one its header names, if it names one), and that key. The
/// claims are the caller's to check ([`check`]).
pub fn verified<'k>(token: &str, keys: &'k [Key]) -> Result<(Value, &'k Key), String> {
    let (header, claims, signed, signature) = parts(token)?;
    if header["alg"] != "RS256" {
        return Err(format!(
            "a token signed with {} (only RS256 is taken)",
            header["alg"]
        ));
    }
    let kid = header["kid"].as_str();
    keys.iter()
        .filter(|k| kid.is_none() || k.id.as_deref() == kid)
        .find(|k| sion_http::rsa_sha256_verify(&k.n, &k.e, signed.as_bytes(), &signature))
        .map(|key| (claims, key))
        .ok_or_else(|| "a token not signed by the service's keys".to_string())
}

fn seconds(value: &Value) -> Option<u64> {
    value.as_u64()
}

/// Checks a token's claims: issued by one of `issuers`, for `audience`,
/// and within its times at `now` (seconds since the epoch), give or take
/// [`SKEW`].
pub fn check(claims: &Value, issuers: &[&str], audience: &str, now: u64) -> Result<(), String> {
    let issuer = claims["iss"].as_str().unwrap_or_default();
    if !issuers.contains(&issuer) {
        return Err(format!(
            "a token issued by {issuer:?}, not {}",
            issuers.join(" or ")
        ));
    }
    let for_us = match &claims["aud"] {
        Value::String(aud) => aud == audience,
        Value::Array(auds) => auds.iter().any(|a| a == audience),
        _ => false,
    };
    if !for_us {
        return Err(format!("a token for another audience ({})", claims["aud"]));
    }
    let expires = seconds(&claims["exp"]).ok_or("a token that names no expiry")?;
    if expires.saturating_add(SKEW) < now {
        return Err("an expired token".into());
    }
    if seconds(&claims["nbf"]).is_some_and(|nbf| nbf > now.saturating_add(SKEW)) {
        return Err("a token not valid yet".into());
    }
    Ok(())
}

/// A token of `claims`, signed with the RSA key in `pem`, its header
/// naming `kid`.
pub fn signed(claims: &Value, pem: &str, kid: Option<&str>) -> Result<String, String> {
    let mut header = json!({"alg": "RS256", "typ": "JWT"});
    if let Some(kid) = kid {
        header["kid"] = json!(kid);
    }
    let signing = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header.to_string()),
        URL_SAFE_NO_PAD.encode(claims.to_string())
    );
    let signature =
        sion_http::rsa_sha256_sign(pem, signing.as_bytes()).map_err(|e| e.to_string())?;
    Ok(format!("{signing}.{}", URL_SAFE_NO_PAD.encode(signature)))
}

/// A key set as its service publishes it: fetched from `url` (or from the
/// `jwks_uri` of the OpenID configuration there), kept a day, and fetched
/// again sooner, at most once a minute, for a token whose key it lacks.
#[derive(Debug)]
pub struct KeySet {
    url: String,
    discovered: bool,
    keys: Vec<Key>,
    fetched: Option<Instant>,
}

impl KeySet {
    /// The key set at `url`.
    pub fn at(url: &str) -> KeySet {
        KeySet {
            url: url.to_string(),
            discovered: false,
            keys: Vec::new(),
            fetched: None,
        }
    }

    /// The key set the OpenID configuration at `url` names.
    pub fn discovered(url: &str) -> KeySet {
        KeySet {
            discovered: true,
            ..KeySet::at(url)
        }
    }

    fn fetch(&mut self, client: &sion_http::Client) -> Result<(), String> {
        let get = |url: &str| -> Result<Value, String> {
            let fetched = client
                .get(url, 1 << 20)
                .map_err(|e| format!("the signing keys ({url}): {e}"))?;
            serde_json::from_slice(&fetched.body)
                .map_err(|e| format!("the signing keys ({url}): not JSON: {e}"))
        };
        let url = if self.discovered {
            let configuration = get(&self.url)?;
            configuration["jwks_uri"]
                .as_str()
                .ok_or_else(|| format!("{} names no jwks_uri", self.url))?
                .to_string()
        } else {
            self.url.clone()
        };
        let keys = keys(&get(&url)?);
        if keys.is_empty() {
            return Err(format!("{url} gives no RSA signing keys"));
        }
        self.keys = keys;
        self.fetched = Some(Instant::now());
        Ok(())
    }

    /// A token's claims and the key that signed it ([`verified`]).
    pub fn verify(
        &mut self,
        client: &sion_http::Client,
        token: &str,
    ) -> Result<(Value, Key), String> {
        if self.fetched.map_or(true, |t| t.elapsed() > KEPT) {
            self.fetch(client)?;
        }
        if let Ok((claims, key)) = verified(token, &self.keys) {
            return Ok((claims, key.clone()));
        }
        // A key published since: fetched again, not too often.
        let lacking = key_id(token)
            .is_some_and(|kid| !self.keys.iter().any(|k| k.id.as_deref() == Some(&kid)));
        if lacking && self.fetched.map_or(true, |t| t.elapsed() > AGAIN) {
            self.fetch(client)?;
        }
        verified(token, &self.keys).map(|(claims, key)| (claims, key.clone()))
    }
}

/// An access token while it lasts (a service's, for the bridge's calls).
#[derive(Debug, Default)]
pub struct Bearer(Option<(String, Instant)>);

impl Bearer {
    /// The token, made anew by `make` (which gives it and its lifetime in
    /// seconds) once it is within five minutes of its end.
    pub fn get(
        &mut self,
        make: impl FnOnce() -> Result<(String, u64), String>,
    ) -> Result<String, String> {
        if let Some((token, until)) = &self.0 {
            if Instant::now() < *until {
                return Ok(token.clone());
            }
        }
        let (token, lifetime) = make()?;
        let until = Instant::now() + Duration::from_secs(lifetime.saturating_sub(SKEW).max(30));
        self.0 = Some((token.clone(), until));
        Ok(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key made for these tests: it signs nothing else.
    const KEY: &str = include_str!("../tests/fixtures/rsa-2048.pem");
    const N: &str = include_str!("../tests/fixtures/rsa-2048.n");

    fn set() -> Value {
        json!({"keys": [
            {"kty": "RSA", "use": "sig", "kid": "k1", "n": N.trim(), "e": "AQAB", "endorsements": ["msteams"]},
            {"kty": "EC", "kid": "k2", "crv": "P-256", "x": "AA", "y": "AA"}
        ]})
    }

    #[test]
    fn tokens_are_taken_only_signed_by_the_key_they_name() {
        let keys = keys(&set());
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].endorsements, ["msteams"]);
        let claims = json!({"iss": "https://api.botframework.com", "aud": "app-1", "exp": 2_000, "nbf": 1_000});
        let token = signed(&claims, KEY, Some("k1")).unwrap();
        let (read, key) = verified(&token, &keys).unwrap();
        assert_eq!(read, claims);
        assert_eq!(key.id.as_deref(), Some("k1"));
        // Its claims, checked.
        assert_eq!(
            check(&read, &["https://api.botframework.com"], "app-1", 1_500),
            Ok(())
        );
        assert!(check(&read, &["https://api.botframework.com"], "app-2", 1_500).is_err());
        assert!(check(&read, &["https://elsewhere"], "app-1", 1_500).is_err());
        assert!(check(
            &read,
            &["https://api.botframework.com"],
            "app-1",
            2_000 + SKEW + 1
        )
        .is_err());
        assert!(check(
            &read,
            &["https://api.botframework.com"],
            "app-1",
            1_000 - SKEW - 1
        )
        .is_err());
        let listed = json!({"iss": "x", "aud": ["a", "app-1"], "exp": 10});
        assert_eq!(check(&listed, &["x"], "app-1", 10), Ok(()));
        // A key it does not name, changed claims, another algorithm.
        let other = signed(&claims, KEY, Some("k9")).unwrap();
        assert!(verified(&other, &keys).is_err());
        let (head, rest) = token.split_once('.').unwrap();
        let (_, signature) = rest.split_once('.').unwrap();
        let changed = URL_SAFE_NO_PAD.encode(
            json!({"iss": "https://api.botframework.com", "aud": "app-2", "exp": 2_000})
                .to_string(),
        );
        assert!(verified(&format!("{head}.{changed}.{signature}"), &keys).is_err());
        let none = URL_SAFE_NO_PAD.encode(json!({"alg": "none", "kid": "k1"}).to_string());
        let error = verified(&format!("{none}.{changed}."), &keys).unwrap_err();
        assert!(error.contains("only RS256"), "{error}");
        assert!(verified("a.b", &keys).is_err());
    }

    #[test]
    fn a_bearer_token_is_made_again_only_near_its_end() {
        let mut bearer = Bearer::default();
        let mut made = 0;
        for _ in 0..3 {
            let token = bearer
                .get(|| {
                    made += 1;
                    Ok(("t1".into(), 3_600))
                })
                .unwrap();
            assert_eq!(token, "t1");
        }
        assert_eq!(made, 1);
        let mut short = Bearer::default();
        short.get(|| Ok(("t2".into(), 10))).unwrap();
        assert!(short
            .0
            .as_ref()
            .is_some_and(|(_, until)| *until > Instant::now()));
    }
}
