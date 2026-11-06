//! ACP's request signing: the `Signature` and `Timestamp` headers.
//!
//! ACP says a client signs its request "over canonical JSON" with a
//! detached signature, base64url, the algorithms advertised out of band
//! (Ed25519 and ES256 are its examples), and sends the time in `Timestamp`
//! (RFC 3339), which the server checks within a bounded clock skew. It
//! names no canonical form; RFC 8785 (JCS) is the one the delegate payment
//! RFC points to for comparing bodies, and is the one used here
//! ([`canonical`]). Two profiles are checked and made:
//!
//! - **ES256 and Ed25519** ([`Verifier::Es256`], [`Verifier::Ed25519`]):
//!   the signature is over the body's canonical JSON (an empty body signs
//!   the empty string); the timestamp is checked for freshness, as ACP has
//!   it, and is not covered by the signature (ACP's text covers the body
//!   only).
//! - **HMAC-SHA256** ([`Verifier::Hmac`]): the hex HMAC of
//!   `timestamp.body` with a shared secret, the timestamp in seconds: what
//!   deployed ACP handlers verify (Vercel's `acp-handler`, for one), and
//!   which covers the timestamp.
//!
//! Either way the timestamp must be within [`SKEW`] seconds of now.

use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use serde_json::{Number, Value};

use super::http;
use crate::ap2::{PublicKey, SigningKey};

/// How far a request's `Timestamp` may be from now, in seconds.
pub const SKEW: u64 = 300;

/// The RFC 8785 canonical form of a JSON value: object keys sorted by their
/// UTF-16 code units, no whitespace, numbers as ECMAScript writes them,
/// strings escaped as `JSON.stringify` escapes them.
pub fn canonical(value: &Value) -> String {
    let mut out = String::new();
    write(value, &mut out);
    out
}

fn write(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&number(n)),
        Value::String(s) => string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write(item, out);
            }
            out.push(']');
        }
        Value::Object(object) => {
            let mut keys: Vec<&String> = object.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                string(key, out);
                out.push(':');
                write(&object[key], out);
            }
            out.push('}');
        }
    }
}

fn string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// A JSON number as ECMAScript's `Number.prototype.toString` writes it (an
/// integer beyond 2^53 is a double, as it is in JavaScript).
fn number(n: &Number) -> String {
    const EXACT: u64 = 1 << 53;
    if let Some(u) = n.as_u64() {
        if u <= EXACT {
            return u.to_string();
        }
        return ecmascript(u as f64);
    }
    if let Some(i) = n.as_i64() {
        if i.unsigned_abs() <= EXACT {
            return i.to_string();
        }
        return ecmascript(i as f64);
    }
    ecmascript(n.as_f64().unwrap_or(0.0))
}

/// ECMAScript's shortest round-trip form of a finite double.
fn ecmascript(f: f64) -> String {
    if f == 0.0 {
        return "0".into();
    }
    if f < 0.0 {
        return format!("-{}", ecmascript(-f));
    }
    // Rust writes the shortest digits that round-trip.
    let scientific = format!("{f:e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i64 = exponent.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let k = digits.len() as i64;
    // The value is 0.d₁d₂…dₖ × 10ⁿ.
    let n = exponent + 1;
    if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let sign = if n > 0 { "+" } else { "-" };
        let rest = if k > 1 {
            format!(".{}", &digits[1..])
        } else {
            String::new()
        };
        format!("{}{rest}e{sign}{}", &digits[..1], (n - 1).abs())
    }
}

/// What a body signs: its canonical JSON (none for an empty body).
fn signed_part(body: &str) -> Result<String, String> {
    if body.trim().is_empty() {
        return Ok(String::new());
    }
    let value: Value =
        serde_json::from_str(body).map_err(|e| format!("a signed body is JSON: {e}"))?;
    Ok(canonical(&value))
}

fn hmac(secret: &[u8], data: &[u8]) -> [u8; 32] {
    let key = aws_lc_rs::hmac::Key::new(aws_lc_rs::hmac::HMAC_SHA256, secret);
    let tag = aws_lc_rs::hmac::sign(&key, data);
    let mut out = [0u8; 32];
    out.copy_from_slice(tag.as_ref());
    out
}

fn decode_signature(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    URL_SAFE_NO_PAD
        .decode(text.trim_end_matches('='))
        .ok()
        .or_else(|| STANDARD.decode(text).ok())
}

/// How a client's `Signature` is checked.
#[derive(Clone)]
pub enum Verifier {
    /// ECDSA P-256 with SHA-256 over the canonical body.
    Es256(PublicKey),
    /// Ed25519 over the canonical body (the 32-byte public key).
    Ed25519(Vec<u8>),
    /// HMAC-SHA256 of `timestamp.body` with a shared secret.
    Hmac(Vec<u8>),
}

impl std::fmt::Debug for Verifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Verifier::Es256(_) => "Verifier(ES256)",
            Verifier::Ed25519(_) => "Verifier(Ed25519)",
            // The secret is never printed.
            Verifier::Hmac(_) => "Verifier(HMAC-SHA256)",
        })
    }
}

impl Verifier {
    /// One read from a JWK: `EC`/`P-256` for ES256, `OKP`/`Ed25519`, or
    /// `oct` (an HMAC secret, `k`).
    pub fn from_jwk(jwk: &Value) -> Result<Verifier, String> {
        match (jwk["kty"].as_str(), jwk["crv"].as_str()) {
            (Some("EC"), Some("P-256")) => PublicKey::from_jwk(jwk)
                .map(Verifier::Es256)
                .map_err(|e| e.to_string()),
            (Some("OKP"), Some("Ed25519")) => {
                let x = jwk["x"]
                    .as_str()
                    .and_then(|x| URL_SAFE_NO_PAD.decode(x).ok())
                    .filter(|x| x.len() == 32)
                    .ok_or("an Ed25519 JWK's x is 32 bytes of base64url")?;
                Ok(Verifier::Ed25519(x))
            }
            (Some("oct"), _) => {
                let k = jwk["k"]
                    .as_str()
                    .and_then(|k| URL_SAFE_NO_PAD.decode(k).ok())
                    .filter(|k| k.len() >= 16)
                    .ok_or("an oct JWK's k is 16 bytes or more of base64url")?;
                Ok(Verifier::Hmac(k))
            }
            _ => Err("a signing key here is EC P-256, OKP Ed25519 or oct".into()),
        }
    }

    /// Checks `signature` and `timestamp` over `body`, at `now`.
    pub fn check(
        &self,
        signature: &str,
        timestamp: &str,
        body: &str,
        now: u64,
    ) -> Result<(), String> {
        let at =
            http::seconds_of(timestamp).ok_or("Timestamp is RFC 3339, or a number of seconds")?;
        if at.abs_diff(now) > SKEW {
            return Err("Timestamp outside allowed window".into());
        }
        let ok = match self {
            Verifier::Es256(key) => {
                let sig = decode_signature(signature).ok_or("Signature is base64url")?;
                key.verifies(signed_part(body)?.as_bytes(), &sig)
            }
            Verifier::Ed25519(public) => {
                let sig = decode_signature(signature).ok_or("Signature is base64url")?;
                UnparsedPublicKey::new(&ED25519, public)
                    .verify(signed_part(body)?.as_bytes(), &sig)
                    .is_ok()
            }
            Verifier::Hmac(secret) => {
                let expected = hmac(secret, format!("{timestamp}.{body}").as_bytes());
                let given = hex::decode(signature.trim())
                    .ok()
                    .or_else(|| decode_signature(signature));
                given.is_some_and(|given| sion_http::same_bytes(&given, &expected))
            }
        };
        if ok {
            Ok(())
        } else {
            Err("Signature does not verify".into())
        }
    }
}

/// How a client signs its requests.
pub enum Signer {
    /// ECDSA P-256 with SHA-256.
    Es256(SigningKey),
    /// Ed25519.
    Ed25519(Ed25519KeyPair),
    /// HMAC-SHA256 with a shared secret.
    Hmac(Vec<u8>),
}

impl std::fmt::Debug for Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Signer({:?})", self.verifier())
    }
}

impl Signer {
    /// An HMAC signer with `secret`.
    pub fn hmac(secret: &[u8]) -> Signer {
        Signer::Hmac(secret.to_vec())
    }

    /// A new Ed25519 key.
    pub fn ed25519() -> Signer {
        let rng = aws_lc_rs::rand::SystemRandom::new();
        let document = Ed25519KeyPair::generate_pkcs8(&rng).expect("an Ed25519 key can be made");
        Signer::Ed25519(
            Ed25519KeyPair::from_pkcs8(document.as_ref()).expect("a key just made reads back"),
        )
    }

    /// What checks its signatures.
    pub fn verifier(&self) -> Verifier {
        match self {
            Signer::Es256(key) => Verifier::Es256(key.public()),
            Signer::Ed25519(key) => Verifier::Ed25519(key.public_key().as_ref().to_vec()),
            Signer::Hmac(secret) => Verifier::Hmac(secret.clone()),
        }
    }

    /// Its public key as a JWK, for the server to be told (none for HMAC).
    pub fn jwk(&self) -> Option<Value> {
        match self {
            Signer::Es256(key) => Some(key.public().jwk()),
            Signer::Ed25519(key) => Some(serde_json::json!({
                "kty": "OKP",
                "crv": "Ed25519",
                "x": URL_SAFE_NO_PAD.encode(key.public_key().as_ref()),
            })),
            Signer::Hmac(_) => None,
        }
    }

    /// The `Signature` and `Timestamp` headers for `body` at `now`.
    pub fn sign(&self, body: &str, now: u64) -> (String, String) {
        match self {
            Signer::Es256(key) => {
                let part = signed_part(body).unwrap_or_default();
                (
                    URL_SAFE_NO_PAD.encode(key.sign(part.as_bytes())),
                    http::rfc3339(now),
                )
            }
            Signer::Ed25519(key) => {
                let part = signed_part(body).unwrap_or_default();
                (
                    URL_SAFE_NO_PAD.encode(key.sign(part.as_bytes()).as_ref()),
                    http::rfc3339(now),
                )
            }
            Signer::Hmac(secret) => {
                let timestamp = now.to_string();
                let tag = hmac(secret, format!("{timestamp}.{body}").as_bytes());
                (hex::encode(tag), timestamp)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_json_is_rfc_8785s() {
        // RFC 8785 §3.2.3's example of sorting by UTF-16 code units.
        let sorted: Value = serde_json::from_str(
            r#"{"€":"Euro Sign","\r":"Carriage Return","דּ":"Hebrew Letter Dalet With Dagesh","1":"One","😀":"Emoji: Grinning Face","\u0080":"Control","ö":"Latin Small Letter O With Diaeresis"}"#,
        )
        .unwrap();
        assert_eq!(
            canonical(&sorted),
            "{\"\\r\":\"Carriage Return\",\"1\":\"One\",\"\u{80}\":\"Control\",\"\u{f6}\":\"Latin Small Letter O With Diaeresis\",\"\u{20ac}\":\"Euro Sign\",\"\u{1f600}\":\"Emoji: Grinning Face\",\"\u{fb33}\":\"Hebrew Letter Dalet With Dagesh\"}"
        );
        // RFC 8785 §3.2.2.3's numbers, as ECMAScript writes them.
        for (input, expected) in [
            ("1.0", "1"),
            ("0.000001", "0.000001"),
            ("1e-7", "1e-7"),
            ("1e21", "1e+21"),
            ("1e20", "100000000000000000000"),
            ("4.5", "4.5"),
            ("-0.0", "0"),
            ("333333333.33333329", "333333333.3333333"),
            ("1E30", "1e+30"),
            ("9007199254740993", "9007199254740992"),
            ("0.002", "0.002"),
        ] {
            let value: Value = serde_json::from_str(input).unwrap();
            assert_eq!(canonical(&value), expected, "{input}");
        }
        assert_eq!(
            canonical(&json!({"b": [true, null], "a": "x\ny\u{1}"})),
            r#"{"a":"x\ny\u0001","b":[true,null]}"#
        );
    }

    #[test]
    fn the_hmac_profile_is_the_hex_hmac_of_timestamp_dot_body() {
        // Known answer, from OpenSSL (`openssl dgst -sha256 -hmac`): what
        // Vercel's acp-handler computes (`computeSignature`), the timestamp
        // in seconds.
        let body = r#"{"currency":"usd","line_items":[{"id":"report"}]}"#;
        let signer = Signer::hmac(b"a shared signing secret, 32 bytes");
        let (signature, timestamp) = signer.sign(body, 1_800_000_000);
        assert_eq!(timestamp, "1800000000");
        assert_eq!(
            signature,
            "378d53e1fa26bf4582a66df07ecad609fa052d383823b1f26bb24cc7a6bb96b5"
        );
    }

    #[test]
    fn each_profile_signs_and_checks() {
        let body = r#"{"b": 2, "a": 1.0}"#;
        let now = 1_800_000_000;
        for signer in [
            Signer::Es256(SigningKey::generate()),
            Signer::ed25519(),
            Signer::hmac(b"a shared secret, 32 bytes long!!"),
        ] {
            let (signature, timestamp) = signer.sign(body, now);
            let verifier = signer.verifier();
            assert!(
                verifier.check(&signature, &timestamp, body, now).is_ok(),
                "{signer:?}"
            );
            // Too old, or too far ahead.
            assert!(verifier
                .check(&signature, &timestamp, body, now + SKEW + 1)
                .is_err());
            // Another body.
            assert!(verifier
                .check(&signature, &timestamp, r#"{"a": 2}"#, now)
                .is_err());
        }
        // The asymmetric profiles sign the canonical form: the same value
        // written another way verifies.
        let signer = Signer::ed25519();
        let (signature, timestamp) = signer.sign(body, now);
        assert!(signer
            .verifier()
            .check(&signature, &timestamp, r#"{"a":1,"b":2}"#, now)
            .is_ok());
        // A JWK reads back as the same verifier.
        let jwk = signer.jwk().unwrap();
        assert!(Verifier::from_jwk(&jwk)
            .unwrap()
            .check(&signature, &timestamp, body, now)
            .is_ok());
    }
}
