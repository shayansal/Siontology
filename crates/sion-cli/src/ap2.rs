//! AP2, the Agent Payments Protocol (v0.2, of 2026-04-28): checking the
//! mandates a shopping agent presents, and making them.
//!
//! [AP2](https://ap2-protocol.org) secures a purchase an agent makes with
//! two *mandates*, each an SD-JWT ([RFC 9901]) signed with ES256:
//!
//! - A **checkout mandate** (`vct` `mandate.checkout.1`) carries the
//!   merchant-signed checkout (`checkout_jwt`) and its hash
//!   (`checkout_hash`): proof, for the merchant, that the agent may buy
//!   exactly that.
//! - A **payment mandate** (`mandate.payment.1`) names the payee, the amount
//!   in minor units, the instrument and, as `transaction_id`, the same
//!   checkout hash: proof, for whoever moves the money, that the agent may
//!   pay for exactly that checkout.
//!
//! With the user there, the user's trusted surface signs these *closed*
//! mandates. Without the user, it signs *open* ones
//! (`mandate.checkout.open.1`, `mandate.payment.open.1`): constraints on
//! what a closed one may say, and the agent's key (`cnf`). The agent then
//! closes each itself, signing with that key, and presents the chain: the
//! open token, then the closed one bound to it by its hash (`sd_hash`), as
//! the Delegate SD-JWT draft has it, the tokens joined by `~`.
//!
//! What is here:
//!
//! - [`verify_chain`]: a chain checked hop by hop, from a root signed by a
//!   key the verifier trusts ([`Trust`], by `kid`) to the closed mandate:
//!   each signature, each disclosure ([RFC 9901] §7.1), each binding hash,
//!   the closing hop's `aud` and `nonce`, and every `iat` and `exp`.
//! - [`verify_checkout`] and [`verify_payment`]: the merchant's and the
//!   payment side's checks of AP2 §Verification, with every constraint
//!   AP2 defines evaluated, and an unknown one failing, as AP2 requires
//!   (`unresolved_constraint`). Two need a history of earlier payments
//!   under the same open mandate: `payment.budget` is evaluated against
//!   what the caller says was spent; `payment.agent_recurrence` is not
//!   evaluated here and fails as unresolved.
//! - [`receipt`]: a signed checkout or payment receipt, whose `reference`
//!   is the hash of the mandate it answers.
//! - [`issue`], [`close`] and [`chain`]: the shopping agent's side, to make
//!   mandates (and to test the checks above against something real).
//!
//! What is not: an `x5c` certificate chain as the root's key (a root is
//! trusted by `kid` only), a KB-JWT at the end of a token (AP2's flow uses
//! none), OpenID4VP delegation from a wallet's user credential, and any
//! digest algorithm but `sha-256`, `sha-384` and `sha-512`.
//!
//! [RFC 9901]: https://datatracker.ietf.org/doc/rfc9901/

use std::collections::{BTreeMap, BTreeSet};

use aws_lc_rs::rand::{SecureRandom, SystemRandom};
use aws_lc_rs::signature::{
    EcdsaKeyPair, KeyPair, UnparsedPublicKey, ECDSA_P256_SHA256_FIXED,
    ECDSA_P256_SHA256_FIXED_SIGNING,
};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::{json, Map, Value};

/// A closed checkout mandate's type.
pub const CHECKOUT: &str = "mandate.checkout.1";
/// An open checkout mandate's type.
pub const CHECKOUT_OPEN: &str = "mandate.checkout.open.1";
/// A closed payment mandate's type.
pub const PAYMENT: &str = "mandate.payment.1";
/// An open payment mandate's type.
pub const PAYMENT_OPEN: &str = "mandate.payment.open.1";

/// The clock skew allowed around `iat` and `exp`, in seconds.
pub const SKEW: u64 = 300;

/// The `typ` of a closing hop (no `cnf`: nothing is delegated past it).
const TYP_TERMINAL: [&str; 2] = ["kb+sd-jwt", "kb-sd-jwt"];
/// The `typ` of a hop that delegates on (it carries the next key's `cnf`).
const TYP_INTERMEDIATE: [&str; 2] = ["kb+sd-jwt+kb", "kb-sd-jwt+kb"];

/// Why a mandate was refused, as AP2's error codes name it.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// It failed verification: a signature, a hash, a time, its form.
    #[error("invalid_credential: {0}")]
    InvalidCredential(String),
    /// A constraint is unknown, or could not be shown to hold.
    #[error("unresolved_constraint: {0}")]
    UnresolvedConstraint(String),
    /// It verified, but does not approve this checkout or payment.
    #[error("invalid_mandate: {0}")]
    InvalidMandate(String),
}

impl Refusal {
    /// The error code AP2 gives it, for a receipt's `error`.
    pub fn code(&self) -> &'static str {
        match self {
            Refusal::InvalidCredential(_) => "invalid_credential",
            Refusal::UnresolvedConstraint(_) => "unresolved_constraint",
            Refusal::InvalidMandate(_) => "invalid_mandate",
        }
    }

    /// What it says, for a receipt's `error_description`.
    pub fn description(&self) -> &str {
        match self {
            Refusal::InvalidCredential(why)
            | Refusal::UnresolvedConstraint(why)
            | Refusal::InvalidMandate(why) => why,
        }
    }
}

fn bad(why: impl Into<String>) -> Refusal {
    Refusal::InvalidCredential(why.into())
}

// ----- Keys and JWS -------------------------------------------------------

/// A P-256 public key, as a JWK's `x` and `y`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicKey {
    x: [u8; 32],
    y: [u8; 32],
}

impl PublicKey {
    /// One read from a JWK (`kty` `EC`, `crv` `P-256`).
    pub fn from_jwk(jwk: &Value) -> Result<PublicKey, Refusal> {
        if jwk["kty"] != "EC" || jwk["crv"] != "P-256" {
            return Err(bad("a key here is an EC key on P-256"));
        }
        let coordinate = |name: &str| -> Result<[u8; 32], Refusal> {
            jwk[name]
                .as_str()
                .and_then(unbase64)
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or_else(|| bad(format!("the key's {name} is not 32 bytes of base64url")))
        };
        Ok(PublicKey {
            x: coordinate("x")?,
            y: coordinate("y")?,
        })
    }

    /// It as a JWK.
    pub fn jwk(&self) -> Value {
        json!({
            "kty": "EC",
            "crv": "P-256",
            "x": URL_SAFE_NO_PAD.encode(self.x),
            "y": URL_SAFE_NO_PAD.encode(self.y),
        })
    }

    fn uncompressed(&self) -> [u8; 65] {
        let mut point = [0u8; 65];
        point[0] = 4;
        point[1..33].copy_from_slice(&self.x);
        point[33..].copy_from_slice(&self.y);
        point
    }

    /// Whether `signature` (r ‖ s, 64 bytes) signs `message` under it.
    pub fn verifies(&self, message: &[u8], signature: &[u8]) -> bool {
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, self.uncompressed())
            .verify(message, signature)
            .is_ok()
    }
}

/// A P-256 signing key, for ES256.
pub struct SigningKey {
    key: EcdsaKeyPair,
}

impl std::fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SigningKey({})", URL_SAFE_NO_PAD.encode(self.public().x))
    }
}

impl SigningKey {
    /// A new one.
    pub fn generate() -> SigningKey {
        SigningKey {
            key: EcdsaKeyPair::generate(&ECDSA_P256_SHA256_FIXED_SIGNING)
                .expect("a P-256 key can be made"),
        }
    }

    /// One kept as PKCS#8 ([`SigningKey::pkcs8`]).
    pub fn from_pkcs8(pkcs8: &[u8]) -> Result<SigningKey, String> {
        EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8)
            .map(|key| SigningKey { key })
            .map_err(|_| "not a P-256 key in PKCS#8".to_string())
    }

    /// It as PKCS#8, to keep.
    pub fn pkcs8(&self) -> Vec<u8> {
        self.key
            .to_pkcs8v1()
            .map(|document| document.as_ref().to_vec())
            .expect("a key can be written")
    }

    /// Its public key.
    pub fn public(&self) -> PublicKey {
        let point = self.key.public_key().as_ref();
        let mut x = [0u8; 32];
        let mut y = [0u8; 32];
        x.copy_from_slice(&point[1..33]);
        y.copy_from_slice(&point[33..65]);
        PublicKey { x, y }
    }

    fn sign(&self, message: &[u8]) -> Vec<u8> {
        self.key
            .sign(&SystemRandom::new(), message)
            .expect("ES256 signs")
            .as_ref()
            .to_vec()
    }
}

fn unbase64(text: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(text.trim_end_matches('=')).ok()
}

fn segment(text: &str, what: &str) -> Result<Value, Refusal> {
    let bytes = unbase64(text).ok_or_else(|| bad(format!("{what} is not base64url")))?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|e| bad(format!("{what} is not JSON: {e}")))?;
    if value.is_object() {
        Ok(value)
    } else {
        Err(bad(format!("{what} is not a JSON object")))
    }
}

/// A compact JWS signed with ES256 by `key`.
pub fn sign_jws(header: &Value, payload: &Value, key: &SigningKey) -> String {
    let mut header = header.clone();
    header["alg"] = json!("ES256");
    let input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header.to_string()),
        URL_SAFE_NO_PAD.encode(payload.to_string())
    );
    let signature = key.sign(input.as_bytes());
    format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature))
}

/// A compact JWS's header and payload, without checking its signature.
pub fn read_jws(token: &str) -> Result<(Value, Value), Refusal> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(bad("a JWT has three parts"));
    }
    Ok((
        segment(parts[0], "a JWT header")?,
        segment(parts[1], "a JWT payload")?,
    ))
}

/// A compact JWS's header and payload, if `key` signed it with ES256.
pub fn verify_jws(token: &str, key: &PublicKey) -> Result<(Value, Value), Refusal> {
    let (header, payload) = read_jws(token)?;
    if header["alg"] != "ES256" {
        return Err(bad(format!(
            "only ES256 is accepted, not {}",
            header["alg"]
        )));
    }
    let (input, signature) = token.rsplit_once('.').expect("three parts");
    let signature = unbase64(signature).ok_or_else(|| bad("a signature is not base64url"))?;
    if !key.verifies(input.as_bytes(), &signature) {
        return Err(bad("a signature does not verify"));
    }
    Ok((header, payload))
}

// ----- Hashes -------------------------------------------------------------

/// The digest algorithms a token may name (`_sd_alg`).
fn digest_of(alg: &str, text: &str) -> Result<String, Refusal> {
    let algorithm = match alg {
        "sha-256" => &aws_lc_rs::digest::SHA256,
        "sha-384" => &aws_lc_rs::digest::SHA384,
        "sha-512" => &aws_lc_rs::digest::SHA512,
        other => return Err(bad(format!("the digest {other} is not supported"))),
    };
    Ok(URL_SAFE_NO_PAD.encode(aws_lc_rs::digest::digest(algorithm, text.as_bytes())))
}

/// The base64url SHA-256 of `text`, as AP2 hashes a checkout JWT or a
/// token when a token names no `_sd_alg`.
pub fn hash(text: &str) -> String {
    digest_of("sha-256", text).expect("sha-256 is supported")
}

// ----- SD-JWT -------------------------------------------------------------

/// One SD-JWT of a chain: its issuer-signed JWT and its disclosures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    /// The issuer-signed JWT.
    pub jwt: String,
    /// The disclosures presented with it.
    pub disclosures: Vec<String>,
}

impl Token {
    /// Reads one SD-JWT: `<jwt>~<disclosure>~…~`, with no KB-JWT.
    pub fn parse(text: &str) -> Result<Token, Refusal> {
        let text = text.trim();
        let Some(body) = text.strip_suffix('~') else {
            return Err(bad(
                "an SD-JWT here ends with ~ (AP2 uses no KB-JWT after it)",
            ));
        };
        let mut parts = body.split('~');
        let jwt = parts.next().unwrap_or_default().to_string();
        if jwt.split('.').count() != 3 {
            return Err(bad("an SD-JWT starts with a JWT"));
        }
        let disclosures: Vec<String> = parts.map(str::to_string).collect();
        if disclosures.iter().any(String::is_empty) {
            return Err(bad("an SD-JWT has an empty disclosure"));
        }
        Ok(Token { jwt, disclosures })
    }

    /// It as presented: `<jwt>~<disclosure>~…~`.
    pub fn text(&self) -> String {
        let mut text = format!("{}~", self.jwt);
        for disclosure in &self.disclosures {
            text.push_str(disclosure);
            text.push('~');
        }
        text
    }

    fn sd_alg(&self) -> Result<String, Refusal> {
        let (_, payload) = read_jws(&self.jwt)?;
        Ok(payload["_sd_alg"].as_str().unwrap_or("sha-256").to_string())
    }

    /// The hash a next hop binds it by (`sd_hash`), and a receipt names it
    /// by (`reference`).
    pub fn sd_hash(&self) -> Result<String, Refusal> {
        digest_of(&self.sd_alg()?, &self.text())
    }

    fn issuer_jwt_hash(&self) -> Result<String, Refusal> {
        digest_of(&self.sd_alg()?, &self.jwt)
    }
}

/// The tokens of a chain: SD-JWTs joined by `~` (so `~~` between two).
pub fn split(chain: &str) -> Result<Vec<Token>, Refusal> {
    let chain = chain.trim();
    let pieces: Vec<&str> = chain.split("~~").collect();
    let last = pieces.len() - 1;
    pieces
        .iter()
        .enumerate()
        .map(|(index, piece)| {
            if index == last {
                Token::parse(piece)
            } else {
                Token::parse(&format!("{piece}~"))
            }
        })
        .collect()
}

/// Tokens joined into a chain.
pub fn chain(tokens: &[&str]) -> String {
    let mut chain = String::new();
    for (index, token) in tokens.iter().enumerate() {
        let token = token.trim();
        if index + 1 < tokens.len() {
            // Its own closing ~, and one more between it and the next.
            chain.push_str(token);
            if !token.ends_with('~') {
                chain.push('~');
            }
            chain.push('~');
        } else {
            chain.push_str(token);
        }
    }
    chain
}

/// A payload with its disclosures put in place (RFC 9901 §7.1): each
/// disclosure used once, each digest once, none left over.
fn disclose(token: &Token, payload: &Value) -> Result<Value, Refusal> {
    let alg = payload["_sd_alg"].as_str().unwrap_or("sha-256");
    let mut by_digest: BTreeMap<String, Value> = BTreeMap::new();
    for disclosure in &token.disclosures {
        let bytes = unbase64(disclosure).ok_or_else(|| bad("a disclosure is not base64url"))?;
        let decoded: Value = serde_json::from_slice(&bytes)
            .map_err(|e| bad(format!("a disclosure is not JSON: {e}")))?;
        let well_formed = decoded
            .as_array()
            .is_some_and(|a| (a.len() == 2 || a.len() == 3) && a[0].is_string());
        if !well_formed {
            return Err(bad("a disclosure is [salt, value] or [salt, name, value]"));
        }
        if by_digest
            .insert(digest_of(alg, disclosure)?, decoded)
            .is_some()
        {
            return Err(bad("a disclosure is presented twice"));
        }
    }
    let mut seen = BTreeSet::new();
    let mut used = BTreeSet::new();
    let mut resolved = resolve(payload, &by_digest, &mut seen, &mut used)?;
    if used.len() != by_digest.len() {
        return Err(bad("a disclosure is presented that nothing refers to"));
    }
    if let Some(object) = resolved.as_object_mut() {
        object.remove("_sd_alg");
    }
    Ok(resolved)
}

fn resolve(
    value: &Value,
    by_digest: &BTreeMap<String, Value>,
    seen: &mut BTreeSet<String>,
    used: &mut BTreeSet<String>,
) -> Result<Value, Refusal> {
    let note = |digest: &str, seen: &mut BTreeSet<String>| -> Result<(), Refusal> {
        if seen.insert(digest.to_string()) {
            Ok(())
        } else {
            Err(bad("a digest appears twice"))
        }
    };
    match value {
        Value::Object(object) => {
            let mut out = Map::new();
            for (name, inner) in object {
                if name != "_sd" {
                    out.insert(name.clone(), resolve(inner, by_digest, seen, used)?);
                }
            }
            if let Some(digests) = object.get("_sd") {
                let digests = digests
                    .as_array()
                    .ok_or_else(|| bad("_sd is an array of digests"))?;
                for digest in digests {
                    let digest = digest.as_str().ok_or_else(|| bad("a digest is a string"))?;
                    note(digest, seen)?;
                    // A digest nothing discloses is a decoy, or withheld.
                    let Some(disclosed) = by_digest.get(digest) else {
                        continue;
                    };
                    let parts = disclosed.as_array().expect("checked");
                    if parts.len() != 3 {
                        return Err(bad("an object's disclosure is [salt, name, value]"));
                    }
                    let name = parts[1]
                        .as_str()
                        .ok_or_else(|| bad("a disclosed name is a string"))?;
                    if name == "_sd" || name == "..." || out.contains_key(name) {
                        return Err(bad(format!("the disclosed name {name:?} is not allowed")));
                    }
                    used.insert(digest.to_string());
                    let inner = resolve(&parts[2], by_digest, seen, used)?;
                    out.insert(name.to_string(), inner);
                }
            }
            Ok(Value::Object(out))
        }
        Value::Array(items) => {
            let mut out = Vec::new();
            for item in items {
                let reference = item
                    .as_object()
                    .filter(|o| o.len() == 1)
                    .and_then(|o| o.get("..."));
                match reference {
                    Some(digest) => {
                        let digest = digest.as_str().ok_or_else(|| bad("a digest is a string"))?;
                        note(digest, seen)?;
                        if let Some(disclosed) = by_digest.get(digest) {
                            let parts = disclosed.as_array().expect("checked");
                            if parts.len() != 2 {
                                return Err(bad("an array's disclosure is [salt, value]"));
                            }
                            used.insert(digest.to_string());
                            out.push(resolve(&parts[1], by_digest, seen, used)?);
                        }
                    }
                    None => out.push(resolve(item, by_digest, seen, used)?),
                }
            }
            Ok(Value::Array(out))
        }
        other => Ok(other.clone()),
    }
}

// ----- Chains -------------------------------------------------------------

/// The keys a verifier trusts to sign a chain's root, by `kid`: the agent
/// providers whose trusted surfaces it relies on (AP2's Trusted Agent
/// Provider model), or users' own keys.
#[derive(Clone, Debug, Default)]
pub struct Trust {
    keys: BTreeMap<String, PublicKey>,
}

impl Trust {
    /// None.
    pub fn new() -> Trust {
        Trust::default()
    }

    /// Also trusts `key` as `kid`.
    pub fn with(mut self, kid: &str, key: PublicKey) -> Trust {
        self.keys.insert(kid.to_string(), key);
        self
    }

    /// Reads a JWK Set (`{"keys": [{"kid": …, …}]}`).
    pub fn from_jwks(set: &Value) -> Result<Trust, Refusal> {
        let mut trust = Trust::new();
        for jwk in set["keys"].as_array().into_iter().flatten() {
            let kid = jwk["kid"]
                .as_str()
                .ok_or_else(|| bad("a trusted key has a kid"))?;
            trust = trust.with(kid, PublicKey::from_jwk(jwk)?);
        }
        Ok(trust)
    }

    /// How many keys it trusts.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether it trusts none.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

/// One verified hop of a chain.
#[derive(Clone, Debug, PartialEq)]
pub struct Hop {
    /// Its payload, its disclosures in place.
    pub payload: Value,
    /// What it delegates: the items of its `delegate_payload`.
    pub items: Vec<Value>,
    /// Its hash, as the next hop or a receipt binds it.
    pub sd_hash: String,
    /// The digest algorithm it names (`_sd_alg`, `sha-256` if none).
    pub sd_alg: String,
}

/// A chain that verified, root first.
#[derive(Clone, Debug, PartialEq)]
pub struct Verified {
    /// Its hops.
    pub hops: Vec<Hop>,
}

impl Verified {
    /// The closed mandate: the last hop's one item.
    pub fn closed(&self) -> Result<&Value, Refusal> {
        let last = self.hops.last().expect("a chain has a hop");
        match last.items.as_slice() {
            [item] => Ok(item),
            _ => Err(bad("the closing hop delegates exactly one mandate")),
        }
    }

    /// The open mandates before it, with the hash of the token each is in.
    pub fn open(&self) -> Vec<(&Value, &str)> {
        let count = self.hops.len();
        self.hops[..count - 1]
            .iter()
            .flat_map(|hop| hop.items.iter().map(|item| (item, hop.sd_hash.as_str())))
            .collect()
    }

    /// What a receipt for it names it by: the hash of its last token.
    pub fn reference(&self) -> &str {
        &self.hops.last().expect("a chain has a hop").sd_hash
    }
}

/// What the closing hop must say: whom it is for and what it answers.
#[derive(Clone, Copy, Debug)]
pub struct Expect<'a> {
    /// The verifier, as `aud`.
    pub audience: &'a str,
    /// The verifier's challenge, as `nonce`.
    pub nonce: &'a str,
    /// Now, in seconds since 1970.
    pub now: u64,
}

fn items_of(payload: &Value) -> Vec<Value> {
    payload["delegate_payload"]
        .as_array()
        .map(|items| items.iter().filter(|i| i.is_object()).cloned().collect())
        .unwrap_or_default()
}

fn times(value: &Value, now: u64, what: &str) -> Result<(), Refusal> {
    if let Some(exp) = value.get("exp") {
        let exp = exp
            .as_u64()
            .ok_or_else(|| bad(format!("{what}'s exp is a number of seconds")))?;
        if now > exp.saturating_add(SKEW) {
            return Err(bad(format!("{what} expired at {exp}")));
        }
    }
    if let Some(iat) = value.get("iat") {
        let iat = iat
            .as_u64()
            .ok_or_else(|| bad(format!("{what}'s iat is a number of seconds")))?;
        if iat > now.saturating_add(SKEW) {
            return Err(bad(format!("{what} was issued in the future ({iat})")));
        }
    }
    Ok(())
}

fn cnf_of(items: &[Value]) -> Option<&Value> {
    items
        .iter()
        .find_map(|item| item.get("cnf").and_then(|cnf| cnf.get("jwk")))
}

/// Verifies a chain from a root `trust` signed to its closing hop, which
/// must be for `expect`. A chain of one token is a closed mandate the
/// root's key signed itself (the user there, AP2's direct mode).
pub fn verify_chain(text: &str, trust: &Trust, expect: Expect<'_>) -> Result<Verified, Refusal> {
    let tokens = split(text)?;
    let mut hops: Vec<Hop> = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        let last = index + 1 == tokens.len();
        let (header, _) = read_jws(&token.jwt)?;
        let key = if index == 0 {
            if header.get("x5c").is_some() && header.get("kid").is_none() {
                return Err(bad("a root is trusted by kid here, not by an x5c chain"));
            }
            let kid = header["kid"]
                .as_str()
                .ok_or_else(|| bad("the root names its key (kid)"))?;
            trust
                .keys
                .get(kid)
                .cloned()
                .ok_or_else(|| bad(format!("the root's key {kid:?} is not trusted")))?
        } else {
            let typ = header["typ"].as_str().unwrap_or_default();
            if !TYP_TERMINAL.contains(&typ) && !TYP_INTERMEDIATE.contains(&typ) {
                return Err(bad(format!("hop {index} has the typ {typ:?}")));
            }
            let previous = &hops[index - 1];
            let jwk = cnf_of(&previous.items)
                .ok_or_else(|| bad(format!("hop {} names no key to close it", index - 1)))?;
            PublicKey::from_jwk(jwk)?
        };
        let (_, signed) = verify_jws(&token.jwt, &key)?;
        let payload = disclose(token, &signed)?;
        let items = items_of(&payload);
        if index > 0 {
            let typ = header["typ"].as_str().unwrap_or_default();
            let previous = &tokens[index - 1];
            match (payload.get("sd_hash"), payload.get("issuer_jwt_hash")) {
                (Some(bound), None) => {
                    if bound.as_str() != Some(previous.sd_hash()?.as_str()) {
                        return Err(bad(format!("hop {index} is bound to another token")));
                    }
                }
                (None, Some(bound)) => {
                    if bound.as_str() != Some(previous.issuer_jwt_hash()?.as_str()) {
                        return Err(bad(format!("hop {index} is bound to another token")));
                    }
                }
                _ => {
                    return Err(bad(format!(
                        "hop {index} has exactly one of sd_hash and issuer_jwt_hash"
                    )))
                }
            }
            if payload.get("iat").is_none() {
                return Err(bad(format!("hop {index} says when it was made (iat)")));
            }
            let terminal = TYP_TERMINAL.contains(&typ);
            if terminal == cnf_of(&items).is_some() {
                return Err(bad(format!(
                    "hop {index}: a closing hop names no next key, and only it"
                )));
            }
            if terminal != last {
                return Err(bad("only the last hop closes the chain"));
            }
            if last {
                if payload["aud"] != expect.audience {
                    return Err(bad(format!(
                        "the mandate is for {}, not {}",
                        payload["aud"], expect.audience
                    )));
                }
                if payload["nonce"] != expect.nonce {
                    return Err(bad("the mandate answers another challenge (nonce)"));
                }
            }
        }
        if !last && items.len() != 1 {
            return Err(bad(format!("hop {index} delegates exactly one mandate")));
        }
        times(&payload, expect.now, &format!("hop {index}"))?;
        for item in &items {
            times(item, expect.now, &format!("a mandate of hop {index}"))?;
        }
        hops.push(Hop {
            sd_hash: token.sd_hash()?,
            sd_alg: token.sd_alg()?,
            payload,
            items,
        });
    }
    Ok(Verified { hops })
}

/// The claims an open mandate fixes, which the closed one must repeat.
fn fixed_claims(open: &Value) -> impl Iterator<Item = (&String, &Value)> {
    open.as_object()
        .into_iter()
        .flatten()
        .filter(|(name, _)| !matches!(name.as_str(), "vct" | "constraints" | "cnf" | "iat" | "exp"))
}

fn constraints_of(open: &Value) -> Vec<&Value> {
    open["constraints"]
        .as_array()
        .map(|c| c.iter().collect())
        .unwrap_or_default()
}

/// Whether every field `allowed` names has the same value in `actual`
/// (an allowed entry may name less than the actual party).
fn names(allowed: &Value, actual: &Value) -> bool {
    match allowed.as_object() {
        Some(fields) if !fields.is_empty() => fields.iter().all(|(k, v)| actual.get(k) == Some(v)),
        _ => false,
    }
}

// ----- Checkout -----------------------------------------------------------

/// A checkout mandate that approves a checkout.
#[derive(Clone, Debug, PartialEq)]
pub struct CheckoutApproved {
    /// The checkout it approves (the checkout JWT's payload).
    pub checkout: Value,
    /// The checkout's hash: what a payment mandate names as
    /// `transaction_id`.
    pub checkout_hash: String,
    /// The hashes of the open checkout mandates it was closed under, for a
    /// payment mandate's `payment.reference`.
    pub open: Vec<String>,
    /// What its receipt names it by.
    pub reference: String,
}

/// The merchant's checks: the chain, then that it approves exactly the
/// checkout `checkout_jwt` (the one the merchant signed with `merchant_key`
/// and gave the agent) for `merchant`, under every constraint of every open
/// mandate before it.
pub fn verify_checkout(
    chain: &str,
    trust: &Trust,
    expect: Expect<'_>,
    checkout_jwt: &str,
    merchant_key: &PublicKey,
) -> Result<CheckoutApproved, Refusal> {
    let verified = verify_chain(chain, trust, expect)?;
    let closed = verified.closed()?;
    if closed["vct"] != CHECKOUT {
        return Err(bad(format!("the closed mandate is a {}", closed["vct"])));
    }
    let presented = closed["checkout_jwt"]
        .as_str()
        .ok_or_else(|| bad("the mandate carries the checkout JWT"))?;
    if presented != checkout_jwt {
        return Err(Refusal::InvalidMandate(
            "the mandate is for another checkout".into(),
        ));
    }
    let alg = &verified.hops.last().expect("a chain has a hop").sd_alg;
    let checkout_hash = digest_of(alg, presented)?;
    if closed["checkout_hash"] != checkout_hash.as_str() {
        return Err(bad("checkout_hash is not the checkout JWT's hash"));
    }
    let (_, checkout) = verify_jws(presented, merchant_key).map_err(|_| {
        Refusal::InvalidMandate("the checkout is not one this merchant signed".into())
    })?;
    let mut open_hashes = Vec::new();
    for (open, hash) in verified.open() {
        if open["vct"] != CHECKOUT_OPEN {
            return Err(Refusal::InvalidMandate(format!(
                "a checkout is closed under a {}",
                open["vct"]
            )));
        }
        for (name, value) in fixed_claims(open) {
            if closed.get(name) != Some(value) {
                return Err(Refusal::InvalidMandate(format!(
                    "the closed mandate changes {name}"
                )));
            }
        }
        for constraint in constraints_of(open) {
            checkout_constraint(constraint, &checkout)?;
        }
        open_hashes.push(hash.to_string());
    }
    Ok(CheckoutApproved {
        checkout,
        checkout_hash,
        open: open_hashes,
        reference: verified.reference().to_string(),
    })
}

/// An item's identifier in a checkout's line: `item.id` (UCP), or
/// `product.id`.
fn item_id(line: &Value) -> Option<&str> {
    line["item"]["id"]
        .as_str()
        .or_else(|| line["product"]["id"].as_str())
}

fn checkout_constraint(constraint: &Value, checkout: &Value) -> Result<(), Refusal> {
    match constraint["type"].as_str().unwrap_or_default() {
        "checkout.allowed_merchants" => {
            let allowed = constraint["allowed"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if allowed.iter().any(|a| names(a, &checkout["merchant"])) {
                Ok(())
            } else {
                Err(Refusal::InvalidMandate(
                    "this merchant is not one the mandate allows".into(),
                ))
            }
        }
        "checkout.line_items" => line_items(constraint, checkout),
        other => Err(Refusal::UnresolvedConstraint(format!(
            "the constraint {other:?} is not known here"
        ))),
    }
}

/// `checkout.line_items`: every required entry is met by its quantity of
/// acceptable items, and every item in the checkout meets one, none twice.
/// A maximum flow, as AP2 suggests, by augmenting paths.
fn line_items(constraint: &Value, checkout: &Value) -> Result<(), Refusal> {
    let refuse = |why: &str| Err(Refusal::InvalidMandate(why.to_string()));
    let entries = constraint["items"].as_array().cloned().unwrap_or_default();
    let mut wanted: Vec<(BTreeSet<String>, u64)> = Vec::new();
    for entry in &entries {
        let acceptable: BTreeSet<String> = entry["acceptable_items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| item["id"].as_str().map(str::to_string))
            .collect();
        let quantity = entry["quantity"].as_u64().unwrap_or(0);
        if quantity == 0 {
            return Err(bad("a line item requirement has a quantity"));
        }
        wanted.push((acceptable, quantity));
    }
    let mut have: BTreeMap<String, u64> = BTreeMap::new();
    for line in checkout["line_items"].as_array().into_iter().flatten() {
        let Some(id) = item_id(line) else {
            return refuse("a line of the checkout names no item");
        };
        *have.entry(id.to_string()).or_default() += line["quantity"].as_u64().unwrap_or(1);
    }
    let need: u64 = wanted.iter().map(|(_, q)| q).sum();
    let total: u64 = have.values().sum();
    if need != total {
        return refuse("the checkout does not hold exactly the items the mandate allows");
    }
    // Unit by unit: the totals are small (they are what a person buys).
    let items: Vec<(&String, u64)> = have.iter().map(|(k, v)| (k, *v)).collect();
    let mut left: Vec<u64> = items.iter().map(|(_, q)| *q).collect();
    let mut flow: Vec<Vec<u64>> = vec![vec![0; items.len()]; wanted.len()];
    fn augment(
        entry: usize,
        wanted: &[(BTreeSet<String>, u64)],
        items: &[(&String, u64)],
        left: &mut [u64],
        flow: &mut [Vec<u64>],
        visited: &mut [bool],
    ) -> bool {
        for (item, (id, _)) in items.iter().enumerate() {
            if visited[item] || !wanted[entry].0.contains(*id) {
                continue;
            }
            visited[item] = true;
            if left[item] > 0 {
                left[item] -= 1;
                flow[entry][item] += 1;
                return true;
            }
            // Take a unit another entry holds of this item, if that entry
            // can find another.
            for other in 0..wanted.len() {
                if flow[other][item] > 0 && augment(other, wanted, items, left, flow, visited) {
                    flow[other][item] -= 1;
                    flow[entry][item] += 1;
                    return true;
                }
            }
        }
        false
    }
    for entry in 0..wanted.len() {
        for _ in 0..wanted[entry].1 {
            let mut visited = vec![false; items.len()];
            if !augment(entry, &wanted, &items, &mut left, &mut flow, &mut visited) {
                return refuse("the checkout's items do not meet the mandate's line items");
            }
        }
    }
    Ok(())
}

// ----- Payment ------------------------------------------------------------

/// What a payment mandate must approve.
#[derive(Clone, Debug, PartialEq)]
pub struct Payment<'a> {
    /// The checkout's hash (`transaction_id`).
    pub checkout_hash: &'a str,
    /// Who is paid: `{id, name, website?}`.
    pub payee: &'a Value,
    /// How much, in minor units.
    pub amount: u64,
    /// In what (ISO 4217, compared without case).
    pub currency: &'a str,
    /// The open checkout mandates the checkout was closed under
    /// ([`CheckoutApproved::open`]).
    pub checkout_open: &'a [String],
    /// What was paid before under the same open payment mandate, in minor
    /// units, if the caller keeps that: without it `payment.budget` is
    /// unresolved.
    pub spent_before: Option<u64>,
}

/// A payment mandate that approves a payment.
#[derive(Clone, Debug, PartialEq)]
pub struct PaymentApproved {
    /// The closed mandate.
    pub mandate: Value,
    /// The hash of the open payment mandate it was closed under, if any:
    /// the key to keep what was spent under it by.
    pub open: Option<String>,
    /// What its receipt names it by.
    pub reference: String,
}

/// The payment side's checks: the chain, then that it approves exactly
/// `payment`, under every constraint of the open mandates before it.
pub fn verify_payment(
    chain: &str,
    trust: &Trust,
    expect: Expect<'_>,
    payment: &Payment<'_>,
) -> Result<PaymentApproved, Refusal> {
    let verified = verify_chain(chain, trust, expect)?;
    let closed = verified.closed()?.clone();
    if closed["vct"] != PAYMENT {
        return Err(bad(format!("the closed mandate is a {}", closed["vct"])));
    }
    for field in [
        "transaction_id",
        "payee",
        "payment_amount",
        "payment_instrument",
    ] {
        if closed.get(field).is_none() {
            return Err(bad(format!("a payment mandate has {field}")));
        }
    }
    if closed["transaction_id"] != payment.checkout_hash {
        return Err(Refusal::InvalidMandate(
            "the payment is for another checkout".into(),
        ));
    }
    if !names(&closed["payee"], payment.payee) || closed["payee"]["id"] != payment.payee["id"] {
        return Err(Refusal::InvalidMandate(
            "the payment is to someone else".into(),
        ));
    }
    let amount = &closed["payment_amount"];
    let currency_matches = amount["currency"]
        .as_str()
        .is_some_and(|c| c.eq_ignore_ascii_case(payment.currency));
    if amount["amount"].as_u64() != Some(payment.amount) || !currency_matches {
        return Err(Refusal::InvalidMandate(format!(
            "the mandate approves {} {}, not {} {}",
            amount["amount"], amount["currency"], payment.amount, payment.currency
        )));
    }
    let mut open_hash = None;
    for (open, hash) in verified.open() {
        if open["vct"] != PAYMENT_OPEN {
            return Err(Refusal::InvalidMandate(format!(
                "a payment is closed under a {}",
                open["vct"]
            )));
        }
        for (name, value) in fixed_claims(open) {
            if closed.get(name) != Some(value) {
                return Err(Refusal::InvalidMandate(format!(
                    "the closed mandate changes {name}"
                )));
            }
        }
        for constraint in constraints_of(open) {
            payment_constraint(constraint, &closed, payment)?;
        }
        open_hash = Some(hash.to_string());
    }
    Ok(PaymentApproved {
        mandate: closed,
        open: open_hash,
        reference: verified.reference().to_string(),
    })
}

fn payment_constraint(
    constraint: &Value,
    closed: &Value,
    payment: &Payment<'_>,
) -> Result<(), Refusal> {
    let refuse = |why: String| Err(Refusal::InvalidMandate(why));
    let member = |field: &str, of: &Value| {
        constraint["allowed"]
            .as_array()
            .is_some_and(|allowed| allowed.iter().any(|a| names(a, of)))
            .then_some(())
            .ok_or_else(|| Refusal::InvalidMandate(format!("the {field} is not one allowed")))
    };
    let same_currency = |c: &Value| {
        c.as_str()
            .is_some_and(|c| c.eq_ignore_ascii_case(payment.currency))
    };
    match constraint["type"].as_str().unwrap_or_default() {
        "payment.allowed_payees" => member("payee", &closed["payee"]),
        "payment.allowed_payment_instruments" => {
            member("payment instrument", &closed["payment_instrument"])
        }
        "payment.allowed_pisps" => match closed.get("pisp") {
            Some(pisp) => member("payment initiation service provider", pisp),
            None => refuse("the mandate allows only some PISPs, and names none".into()),
        },
        "payment.amount_range" => {
            if !same_currency(&constraint["currency"]) {
                return refuse("the payment is in another currency".into());
            }
            let max = constraint["max"].as_u64();
            let min = constraint["min"].as_u64().unwrap_or(0);
            match max {
                Some(max) if (min..=max).contains(&payment.amount) => Ok(()),
                Some(max) => refuse(format!("{} is outside {min}..={max}", payment.amount)),
                None => Err(Refusal::UnresolvedConstraint(
                    "an amount range's max is a whole number of minor units".into(),
                )),
            }
        }
        "payment.budget" => {
            if !same_currency(&constraint["currency"]) {
                return refuse("the payment is in another currency".into());
            }
            // The schema has `max` as a number; read as minor units, as
            // `amount_range` is, and only when it is a whole number.
            let Some(max) = constraint["max"].as_u64() else {
                return Err(Refusal::UnresolvedConstraint(
                    "a budget is read here only as whole minor units".into(),
                ));
            };
            let Some(spent) = payment.spent_before else {
                return Err(Refusal::UnresolvedConstraint(
                    "what was spent under this mandate before is not kept here".into(),
                ));
            };
            if spent.saturating_add(payment.amount) <= max {
                Ok(())
            } else {
                refuse(format!(
                    "{spent} spent and {} more is over the budget of {max}",
                    payment.amount
                ))
            }
        }
        "payment.reference" => {
            let wanted = constraint["conditional_transaction_id"]
                .as_str()
                .unwrap_or_default();
            if payment.checkout_open.iter().any(|h| h == wanted) {
                Ok(())
            } else {
                refuse("the payment is not for the checkout its mandate references".into())
            }
        }
        "payment.execution_date" => {
            let when = match closed.get("execution_date") {
                Some(date) => date
                    .as_str()
                    .and_then(seconds_of)
                    .ok_or_else(|| bad("execution_date is an ISO 8601 date"))?,
                // Absent: the payment is made now.
                None => i64::try_from(payment_now()).unwrap_or(i64::MAX),
            };
            for (bound, later) in [("not_before", true), ("not_after", false)] {
                if let Some(text) = constraint.get(bound) {
                    let at = text
                        .as_str()
                        .and_then(seconds_of)
                        .ok_or_else(|| bad(format!("{bound} is an ISO 8601 date")))?;
                    if (later && when < at) || (!later && when > at) {
                        return refuse(format!("the payment falls outside its {bound}"));
                    }
                }
            }
            Ok(())
        }
        "payment.agent_recurrence" => Err(Refusal::UnresolvedConstraint(
            "recurring payments under one mandate are not tracked here".into(),
        )),
        other => Err(Refusal::UnresolvedConstraint(format!(
            "the constraint {other:?} is not known here"
        ))),
    }
}

fn payment_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Seconds since 1970 of an RFC 3339 date-time (`2026-04-30T23:59:59Z`,
/// with a fraction or an offset) or of a date (`2026-04-30`, its start).
pub fn seconds_of(text: &str) -> Option<i64> {
    let number = |s: &str| -> Option<i64> {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse().ok())
            .flatten()
    };
    let (date, time) = match text.split_once(['T', 't', ' ']) {
        Some((date, time)) => (date, Some(time)),
        None => (text, None),
    };
    let mut parts = date.splitn(3, '-');
    let (year, month, day) = (
        number(parts.next()?)?,
        number(parts.next()?)?,
        number(parts.next()?)?,
    );
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let Some(time) = time else {
        return Some(days * 86_400);
    };
    let (clock, offset) = if let Some(clock) = time.strip_suffix(['Z', 'z']) {
        (clock, 0)
    } else {
        let at = time.rfind(['+', '-'])?;
        let (clock, zone) = time.split_at(at);
        let sign = if zone.starts_with('-') { -1 } else { 1 };
        let (h, m) = zone[1..].split_once(':')?;
        (clock, sign * (number(h)? * 3600 + number(m)? * 60))
    };
    let clock = clock.split('.').next()?;
    let mut hms = clock.splitn(3, ':');
    let (h, m, s) = (
        number(hms.next()?)?,
        number(hms.next()?)?,
        number(hms.next()?)?,
    );
    if h > 23 || m > 59 || s > 60 {
        return None;
    }
    Some(days * 86_400 + h * 3600 + m * 60 + s - offset)
}

// ----- Receipts -----------------------------------------------------------

/// What a receipt answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A checkout mandate, answered by the merchant.
    Checkout,
    /// A payment mandate, answered by whoever moved the money.
    Payment,
}

/// A receipt: a JWT `key` signs as `issuer`, answering the mandate
/// `reference` names, with `outcome` (on success, `order_id` for a
/// checkout, or `payment_id` and its confirmations for a payment).
pub fn receipt(
    kind: Kind,
    key: &SigningKey,
    issuer: &str,
    reference: &str,
    outcome: Result<&Value, &Refusal>,
    now: u64,
) -> String {
    let mut claims = json!({
        "iss": issuer,
        "iat": now,
        "reference": reference,
    });
    match outcome {
        Ok(details) => {
            claims["status"] = json!("Success");
            for (name, value) in details.as_object().into_iter().flatten() {
                claims[name] = value.clone();
            }
        }
        Err(refusal) => {
            claims["status"] = json!("Error");
            claims["error"] = json!(refusal.code());
            claims["error_description"] = json!(refusal.description());
            if kind == Kind::Payment {
                claims["payment_id"] = json!("");
            }
        }
    }
    sign_jws(&json!({"typ": "JWT"}), &claims, key)
}

// ----- Making mandates ----------------------------------------------------

fn salt() -> String {
    let mut bytes = [0u8; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .expect("the system has randomness");
    URL_SAFE_NO_PAD.encode(bytes)
}

/// A disclosure of an array element, and its digest.
fn element(value: &Value) -> (String, String) {
    let disclosure = URL_SAFE_NO_PAD.encode(json!([salt(), value]).to_string());
    let digest = hash(&disclosure);
    (disclosure, digest)
}

/// A root mandate: `mandate` (an open or closed mandate's content) as the
/// one disclosed item of `delegate_payload`, signed by `key` as `kid`: what
/// a trusted surface hands an agent.
pub fn issue(mandate: &Value, key: &SigningKey, kid: &str) -> String {
    let (disclosure, digest) = element(mandate);
    let jwt = sign_jws(
        &json!({"typ": "dc+sd-jwt", "kid": kid}),
        &json!({"delegate_payload": [{"...": digest}], "_sd_alg": "sha-256"}),
        key,
    );
    format!("{jwt}~{disclosure}~")
}

/// The agent's closing hop over `previous` (the open mandate's token):
/// `mandate` signed with the agent's key (the one the open mandate names),
/// for `audience` and its `nonce`, at `now`.
pub fn close(
    previous: &str,
    mandate: &Value,
    key: &SigningKey,
    audience: &str,
    nonce: &str,
    now: u64,
) -> Result<String, Refusal> {
    let previous = Token::parse(previous)?;
    let (disclosure, digest) = element(mandate);
    let typ = if mandate.get("cnf").is_some() {
        TYP_INTERMEDIATE[0]
    } else {
        TYP_TERMINAL[0]
    };
    let jwt = sign_jws(
        &json!({"typ": typ}),
        &json!({
            "delegate_payload": [{"...": digest}],
            "iat": now,
            "aud": audience,
            "nonce": nonce,
            "sd_hash": previous.sd_hash()?,
            "_sd_alg": "sha-256",
        }),
        key,
    );
    Ok(format!("{jwt}~{disclosure}~"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_read_as_rfc_3339() {
        assert_eq!(seconds_of("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(seconds_of("1970-01-02"), Some(86_400));
        assert_eq!(seconds_of("2026-04-30T23:59:59Z"), Some(1_777_593_599));
        assert_eq!(
            seconds_of("2026-04-30T23:59:59.250+02:00"),
            Some(1_777_593_599 - 7200)
        );
        assert_eq!(seconds_of("2026-13-01"), None);
        assert_eq!(seconds_of("soon"), None);
    }

    #[test]
    fn a_signature_verifies_only_under_its_key() {
        let key = SigningKey::generate();
        let token = sign_jws(&json!({}), &json!({"a": 1}), &key);
        assert!(verify_jws(&token, &key.public()).is_ok());
        assert!(verify_jws(&token, &SigningKey::generate().public()).is_err());
        let again = SigningKey::from_pkcs8(&key.pkcs8()).unwrap();
        assert_eq!(again.public(), key.public());
        assert_eq!(
            PublicKey::from_jwk(&key.public().jwk()).unwrap(),
            key.public()
        );
    }

    #[test]
    fn disclosures_are_each_used_once_and_digests_are_not_repeated() {
        let key = SigningKey::generate();
        let (disclosure, digest) = element(&json!({"vct": CHECKOUT}));
        let payload = json!({"delegate_payload": [{"...": digest}], "_sd_alg": "sha-256"});
        let jwt = sign_jws(&json!({}), &payload, &key);
        let token = Token::parse(&format!("{jwt}~{disclosure}~")).unwrap();
        let disclosed = disclose(&token, &payload).unwrap();
        assert_eq!(disclosed["delegate_payload"][0]["vct"], CHECKOUT);
        // A disclosure nothing refers to is refused.
        let (stray, _) = element(&json!(1));
        let token = Token::parse(&format!("{jwt}~{disclosure}~{stray}~")).unwrap();
        assert!(disclose(&token, &payload).is_err());
        // A digest repeated is refused.
        let twice = json!({"delegate_payload": [{"...": digest}, {"...": digest}]});
        let token = Token::parse(&format!("{jwt}~{disclosure}~")).unwrap();
        assert!(disclose(&token, &twice).is_err());
        // Withheld, it is left out.
        let token = Token::parse(&format!("{jwt}~")).unwrap();
        assert_eq!(
            disclose(&token, &payload).unwrap()["delegate_payload"],
            json!([])
        );
    }

    #[test]
    fn line_items_are_matched_as_a_flow() {
        let constraint = json!({
            "type": "checkout.line_items",
            "items": [
                {"id": "shoes", "quantity": 1, "acceptable_items": [{"id": "red"}, {"id": "blue"}]},
                {"id": "socks", "quantity": 1, "acceptable_items": [{"id": "socks"}]},
            ]
        });
        let checkout = |ids: &[&str]| json!({"line_items": ids.iter().map(|id| json!({"item": {"id": id}, "quantity": 1})).collect::<Vec<_>>()});
        assert!(line_items(&constraint, &checkout(&["red", "socks"])).is_ok());
        assert!(line_items(&constraint, &checkout(&["blue", "socks"])).is_ok());
        for wrong in [
            &["red", "blue"][..],
            &["red"],
            &["socks"],
            &["red", "socks", "socks"],
        ] {
            assert!(
                line_items(&constraint, &checkout(wrong)).is_err(),
                "{wrong:?}"
            );
        }
        // A unit taken by the first entry is given up when the second needs
        // it: "either" then "only blue".
        let constraint = json!({
            "type": "checkout.line_items",
            "items": [
                {"quantity": 1, "acceptable_items": [{"id": "red"}, {"id": "blue"}]},
                {"quantity": 1, "acceptable_items": [{"id": "blue"}]},
            ]
        });
        let both = json!({"line_items": [
            {"item": {"id": "blue"}, "quantity": 1},
            {"item": {"id": "red"}, "quantity": 1},
        ]});
        assert!(line_items(&constraint, &both).is_ok());
    }
}
