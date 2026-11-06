//! A root's key from an X.509 certificate chain: the `x5c` header (RFC 7515
//! §4.1.6), for AP2 chains whose first hop an agent provider signs with a
//! certified key rather than one the verifier knows by `kid`.
//!
//! The chain is the header's certificates, the signer's first, each signed
//! by the next; the last is a root the verifier trusts, or is signed by one.
//! Checked here, as RFC 5280's path validation has it for what AP2 needs:
//! every signature (ECDSA P-256 and P-384 with SHA-256 or SHA-384, RSA
//! PKCS #1 v1.5 with SHA-256, -384 or -512, Ed25519); that each
//! certificate names the next as its issuer; that every issuer is a CA
//! (basic constraints), may sign certificates (key usage, when stated) and
//! allows as many CAs below it as there are (path length); that every
//! certificate, the trusted root's included, is valid now; that no
//! critical extension is one this does not check; and that the signer's
//! key is a P-256 key that may sign (ES256). Not checked: name
//! constraints and certificate policies (a chain that marks either critical
//! is refused), revocation (no CRL or OCSP is fetched), and RSA-PSS
//! signatures.
//!
//! AP2's reference SDK writes the certificates in base64url; RFC 7515 says
//! base64. Both are read.

use aws_lc_rs::signature::{self, UnparsedPublicKey, VerificationAlgorithm};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use serde_json::Value;
use x509_cert::der::asn1::ObjectIdentifier;
use x509_cert::der::{Decode, Encode};
use x509_cert::ext::pkix::{BasicConstraints, KeyUsage, KeyUsages};
use x509_cert::Certificate;

use super::{bad, PublicKey, Refusal};

/// The most certificates a chain may hold.
pub const MAX_CHAIN: usize = 8;

const EC_KEY: &str = "1.2.840.10045.2.1";
const P256: &str = "1.2.840.10045.3.1.7";
const P384: &str = "1.3.132.0.34";
const RSA_KEY: &str = "1.2.840.113549.1.1.1";
const ED25519: &str = "1.3.101.112";
const ECDSA_SHA256: &str = "1.2.840.10045.4.3.2";
const ECDSA_SHA384: &str = "1.2.840.10045.4.3.3";
const RSA_SHA256: &str = "1.2.840.113549.1.1.11";
const RSA_SHA384: &str = "1.2.840.113549.1.1.12";
const RSA_SHA512: &str = "1.2.840.113549.1.1.13";
const BASIC_CONSTRAINTS: &str = "2.5.29.19";
const KEY_USAGE: &str = "2.5.29.15";
/// Extensions read as informative: allowed critical, not checked further.
const INFORMATIVE: [&str; 4] = [
    "2.5.29.14", // subject key identifier
    "2.5.29.35", // authority key identifier
    "2.5.29.17", // subject alternative name
    "2.5.29.37", // extended key usage
];

#[derive(Clone, Debug, PartialEq, Eq)]
enum Key {
    P256(Vec<u8>),
    P384(Vec<u8>),
    Rsa(Vec<u8>),
    Ed25519(Vec<u8>),
}

impl Key {
    fn bytes(&self) -> &[u8] {
        match self {
            Key::P256(b) | Key::P384(b) | Key::Rsa(b) | Key::Ed25519(b) => b,
        }
    }
}

/// A certificate, as what is checked of it.
#[derive(Clone, Debug)]
struct Cert {
    der: Vec<u8>,
    tbs: Vec<u8>,
    signature: Vec<u8>,
    signature_algorithm: String,
    subject: Vec<u8>,
    issuer: Vec<u8>,
    key: Key,
    not_before: u64,
    not_after: u64,
    /// `Some(path length)` if it is a CA.
    ca: Option<Option<u8>>,
    key_usage: Option<KeyUsage>,
}

fn oid(text: &str) -> ObjectIdentifier {
    ObjectIdentifier::new(text).expect("a constant OID")
}

fn read(der: &[u8], what: &str) -> Result<Cert, Refusal> {
    let malformed = |e: x509_cert::der::Error| bad(format!("{what} is not a certificate: {e}"));
    let cert = Certificate::from_der(der).map_err(malformed)?;
    let tbs = &cert.tbs_certificate;
    let spki = &tbs.subject_public_key_info;
    let raw_key = spki.subject_public_key.raw_bytes().to_vec();
    let key = match spki.algorithm.oid.to_string().as_str() {
        EC_KEY => {
            let curve = spki
                .algorithm
                .parameters
                .as_ref()
                .and_then(|p| p.decode_as::<ObjectIdentifier>().ok())
                .map(|c| c.to_string());
            match curve.as_deref() {
                Some(P256) => Key::P256(raw_key),
                Some(P384) => Key::P384(raw_key),
                _ => return Err(bad(format!("{what} has a key on a curve not taken here"))),
            }
        }
        RSA_KEY => Key::Rsa(raw_key),
        ED25519 => Key::Ed25519(raw_key),
        other => {
            return Err(bad(format!(
                "{what} has a key of a kind not taken here ({other})"
            )))
        }
    };
    let mut ca = None;
    let mut key_usage = None;
    for extension in tbs.extensions.iter().flatten() {
        let id = extension.extn_id;
        let value = extension.extn_value.as_bytes();
        if id == oid(BASIC_CONSTRAINTS) {
            let constraints = BasicConstraints::from_der(value)
                .map_err(|e| bad(format!("{what}'s basic constraints: {e}")))?;
            if constraints.ca {
                ca = Some(constraints.path_len_constraint);
            }
        } else if id == oid(KEY_USAGE) {
            key_usage = Some(
                KeyUsage::from_der(value).map_err(|e| bad(format!("{what}'s key usage: {e}")))?,
            );
        } else if extension.critical && !INFORMATIVE.iter().any(|known| id == oid(known)) {
            return Err(bad(format!(
                "{what} has a critical extension not checked here ({id})"
            )));
        }
    }
    let seconds = |time: x509_cert::time::Time| time.to_unix_duration().as_secs();
    Ok(Cert {
        der: der.to_vec(),
        tbs: tbs.to_der().map_err(malformed)?,
        signature: cert.signature.raw_bytes().to_vec(),
        signature_algorithm: cert.signature_algorithm.oid.to_string(),
        subject: tbs.subject.to_der().map_err(malformed)?,
        issuer: tbs.issuer.to_der().map_err(malformed)?,
        key,
        not_before: seconds(tbs.validity.not_before),
        not_after: seconds(tbs.validity.not_after),
        ca,
        key_usage,
    })
}

/// Checks that `issuer` signed `cert`, named as its issuer, and may: a CA
/// that may sign certificates, with room below it for `below` more CAs.
fn signed_by(cert: &Cert, issuer: &Cert, below: usize, what: &str) -> Result<(), Refusal> {
    if cert.issuer != issuer.subject {
        return Err(bad(format!("{what} names another issuer")));
    }
    match issuer.ca {
        None => return Err(bad(format!("{what}'s issuer is not a CA"))),
        Some(Some(length)) if usize::from(length) < below => {
            return Err(bad(format!(
                "{what}'s issuer allows {length} CAs below it, and there are {below}"
            )))
        }
        Some(_) => {}
    }
    if let Some(usage) = &issuer.key_usage {
        if !usage.0.contains(KeyUsages::KeyCertSign) {
            return Err(bad(format!("{what}'s issuer may not sign certificates")));
        }
    }
    let algorithm: &'static dyn VerificationAlgorithm =
        match (cert.signature_algorithm.as_str(), &issuer.key) {
            (ECDSA_SHA256, Key::P256(_)) => &signature::ECDSA_P256_SHA256_ASN1,
            (ECDSA_SHA256, Key::P384(_)) => &signature::ECDSA_P384_SHA256_ASN1,
            (ECDSA_SHA384, Key::P256(_)) => &signature::ECDSA_P256_SHA384_ASN1,
            (ECDSA_SHA384, Key::P384(_)) => &signature::ECDSA_P384_SHA384_ASN1,
            (RSA_SHA256, Key::Rsa(_)) => &signature::RSA_PKCS1_2048_8192_SHA256,
            (RSA_SHA384, Key::Rsa(_)) => &signature::RSA_PKCS1_2048_8192_SHA384,
            (RSA_SHA512, Key::Rsa(_)) => &signature::RSA_PKCS1_2048_8192_SHA512,
            (ED25519, Key::Ed25519(_)) => &signature::ED25519,
            (algorithm, _) => {
                return Err(bad(format!(
            "{what} is signed with {algorithm}, which is not checked here with its issuer's key"
        )))
            }
        };
    UnparsedPublicKey::new(algorithm, issuer.key.bytes())
        .verify(&cert.tbs, &cert.signature)
        .map_err(|_| bad(format!("{what}'s signature does not verify")))
}

fn valid_at(cert: &Cert, now: u64, what: &str) -> Result<(), Refusal> {
    if now < cert.not_before {
        return Err(bad(format!(
            "{what} is not valid until {}",
            cert.not_before
        )));
    }
    if now > cert.not_after {
        return Err(bad(format!("{what} expired at {}", cert.not_after)));
    }
    Ok(())
}

/// The DER of one `x5c` entry: base64, or base64url as AP2's reference SDK
/// writes it.
fn der_of(text: &str) -> Option<Vec<u8>> {
    STANDARD
        .decode(text)
        .ok()
        .or_else(|| URL_SAFE_NO_PAD.decode(text.trim_end_matches('=')).ok())
}

/// The signer's key from `x5c`, if the chain leads to one of `roots` (DER)
/// and holds at `now` (seconds since 1970).
pub fn key_of(x5c: &Value, roots: &[Vec<u8>], now: u64) -> Result<PublicKey, Refusal> {
    if roots.is_empty() {
        return Err(bad(
            "no root certificate is trusted here, so no x5c chain can be",
        ));
    }
    let entries = x5c
        .as_array()
        .filter(|entries| (1..=MAX_CHAIN).contains(&entries.len()))
        .ok_or_else(|| bad(format!("x5c is a list of 1 to {MAX_CHAIN} certificates")))?;
    let mut chain = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let what = format!("x5c certificate {index}");
        let der = entry
            .as_str()
            .and_then(der_of)
            .ok_or_else(|| bad(format!("{what} is not base64")))?;
        let cert = read(&der, &what)?;
        valid_at(&cert, now, &what)?;
        chain.push(cert);
    }
    let signer = &chain[0];
    if let Some(usage) = &signer.key_usage {
        if !usage.0.contains(KeyUsages::DigitalSignature) {
            return Err(bad("the signer's certificate may not sign (key usage)"));
        }
    }
    for index in 0..chain.len() - 1 {
        signed_by(
            &chain[index],
            &chain[index + 1],
            index,
            &format!("x5c certificate {index}"),
        )?;
    }
    let last = chain.last().expect("one at least");
    let mut found = false;
    let mut last_error = None;
    for (index, root) in roots.iter().enumerate() {
        if *root == last.der {
            found = true;
            break;
        }
        let root = match read(root, &format!("trusted root {index}")) {
            Ok(root) => root,
            Err(e) => {
                last_error = Some(e);
                continue;
            }
        };
        if root.subject != last.issuer {
            continue;
        }
        let checked = valid_at(&root, now, "the trusted root")
            .and_then(|()| signed_by(last, &root, chain.len() - 1, "the last x5c certificate"));
        match checked {
            Ok(()) => {
                found = true;
                break;
            }
            Err(e) => last_error = Some(e),
        }
    }
    if !found {
        return Err(last_error.unwrap_or_else(|| bad("the x5c chain leads to no trusted root")));
    }
    match &signer.key {
        Key::P256(point) if point.len() == 65 && point[0] == 4 => {
            let mut x = [0u8; 32];
            let mut y = [0u8; 32];
            x.copy_from_slice(&point[1..33]);
            y.copy_from_slice(&point[33..]);
            Ok(PublicKey { x, y })
        }
        _ => Err(bad("the signer's certificate holds no P-256 key (ES256)")),
    }
}

/// The DER of each certificate in `pem` (one or more).
pub fn pem_certificates(pem: &str) -> Result<Vec<Vec<u8>>, Refusal> {
    let mut certificates = Vec::new();
    let mut body: Option<String> = None;
    for line in pem.lines().map(str::trim) {
        if line.starts_with("-----BEGIN CERTIFICATE-----") {
            body = Some(String::new());
        } else if line.starts_with("-----END CERTIFICATE-----") {
            let text = body.take().unwrap_or_default();
            certificates.push(
                STANDARD
                    .decode(text)
                    .map_err(|e| bad(format!("a PEM certificate's base64: {e}")))?,
            );
        } else if let Some(text) = body.as_mut() {
            text.push_str(line);
        }
    }
    if certificates.is_empty() {
        return Err(bad("no certificate in the PEM"));
    }
    Ok(certificates)
}
