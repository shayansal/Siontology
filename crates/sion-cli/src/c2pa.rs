//! Content Credentials (C2PA): what the agent makes, signed as made by it.
//!
//! A C2PA manifest store is a JUMBF box ([ISO/IEC 19566-5]) holding
//! manifests. Each manifest holds assertions (CBOR), a claim that lists each
//! assertion by the SHA-256 of its box, and the claim's signature, a
//! COSE_Sign1 whose protected header carries the signer's X.509 chain
//! (`x5chain`). A hard binding, the `c2pa.hash.data` assertion, holds the
//! SHA-256 of the asset itself, so the claim binds the very bytes signed.
//! What [`sign`] writes is the manifest the C2PA specification (2.x) calls
//! a sidecar: kept beside the asset (`<asset>.c2pa`), not inside it, its
//! data hash over all of the asset's bytes. Its assertions:
//!
//! - `c2pa.actions.v2`: the asset was created (or edited) by trained
//!   algorithmic media, the IPTC digital source type for what a model
//!   makes, by Siontology;
//! - `org.siontology.agent`: the agent that made it, by its DID and name;
//! - `c2pa.hash.data`: the hard binding.
//!
//! The claim is a `c2pa.claim.v2`, signed ES256 with a P-256 key whose
//! certificate is the chain's first. [`verify`] checks a manifest store
//! against an asset: the signature, every assertion's hash, and the data
//! hash, the chain to a root given as trusted when one is. Whether a root is
//! one C2PA's trust list names is for a validator that keeps the list; a
//! certificate of the user's own CA makes manifests that validate, signed
//! by a credential no one else has reason to trust.

use base64::Engine;
use k256::sha2::{Digest, Sha256};

use crate::ap2::{PublicKey, SigningKey};
use crate::error::CliError;

/// The JUMBF type of each C2PA box, a UUID.
const C2PA: [u8; 16] = uuid(*b"c2pa");
const MANIFEST: [u8; 16] = uuid(*b"c2ma");
const ASSERTIONS: [u8; 16] = uuid(*b"c2as");
const CLAIM: [u8; 16] = uuid(*b"c2cl");
const SIGNATURE: [u8; 16] = uuid(*b"c2cs");
const CBOR: [u8; 16] = uuid(*b"cbor");

/// The IPTC digital source type of what a trained model makes.
pub const TRAINED_ALGORITHMIC_MEDIA: &str =
    "http://cv.iptc.org/newscodes/digitalsourcetype/trainedAlgorithmicMedia";

/// The label of the agent's own assertion.
pub const AGENT_ASSERTION: &str = "org.siontology.agent";

/// A UUID of ISO's JUMBF form: four characters, then 0011-0010-8000-00AA00389B71.
const fn uuid(code: [u8; 4]) -> [u8; 16] {
    [
        code[0], code[1], code[2], code[3], 0x00, 0x11, 0x00, 0x10, 0x80, 0x00, 0x00, 0xaa, 0x00,
        0x38, 0x9b, 0x71,
    ]
}

/// A CBOR value: as much of RFC 8949 as C2PA's claims and COSE use.
#[derive(Clone, Debug, PartialEq)]
pub enum Cbor {
    /// An integer.
    Int(i128),
    /// A byte string.
    Bytes(Vec<u8>),
    /// A text string.
    Text(String),
    /// An array.
    Array(Vec<Cbor>),
    /// A map, its entries in the order written.
    Map(Vec<(Cbor, Cbor)>),
    /// A tagged value.
    Tag(u64, Box<Cbor>),
    /// `false` or `true`.
    Bool(bool),
    /// `null` (or `undefined`).
    Null,
    /// A float, kept as its bits.
    Float(f64),
}

impl Cbor {
    fn text(text: &str) -> Cbor {
        Cbor::Text(text.to_string())
    }

    /// A map of text keys.
    fn map(entries: Vec<(&str, Cbor)>) -> Cbor {
        Cbor::Map(
            entries
                .into_iter()
                .map(|(k, v)| (Cbor::text(k), v))
                .collect(),
        )
    }

    /// The value under the text key `key`, in a map.
    pub fn get(&self, key: &str) -> Option<&Cbor> {
        match self {
            Cbor::Map(entries) => entries
                .iter()
                .find(|(k, _)| matches!(k, Cbor::Text(t) if t == key))
                .map(|(_, v)| v),
            _ => None,
        }
    }

    /// The value under the integer key `key`, in a map.
    fn get_int(&self, key: i128) -> Option<&Cbor> {
        match self {
            Cbor::Map(entries) => entries
                .iter()
                .find(|(k, _)| matches!(k, Cbor::Int(n) if *n == key))
                .map(|(_, v)| v),
            _ => None,
        }
    }

    fn as_text(&self) -> Option<&str> {
        match self {
            Cbor::Text(t) => Some(t),
            _ => None,
        }
    }

    fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Cbor::Bytes(b) => Some(b),
            _ => None,
        }
    }

    /// Its encoding, lengths definite.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out);
        out
    }

    fn head(out: &mut Vec<u8>, major: u8, n: u64) {
        let major = major << 5;
        match n {
            0..=23 => out.push(major | n as u8),
            24..=0xff => out.extend([major | 24, n as u8]),
            0x100..=0xffff => {
                out.push(major | 25);
                out.extend((n as u16).to_be_bytes());
            }
            0x1_0000..=0xffff_ffff => {
                out.push(major | 26);
                out.extend((n as u32).to_be_bytes());
            }
            _ => {
                out.push(major | 27);
                out.extend(n.to_be_bytes());
            }
        }
    }

    fn write(&self, out: &mut Vec<u8>) {
        match self {
            Cbor::Int(n) if *n >= 0 => Cbor::head(out, 0, *n as u64),
            Cbor::Int(n) => Cbor::head(out, 1, (-1 - *n) as u64),
            Cbor::Bytes(b) => {
                Cbor::head(out, 2, b.len() as u64);
                out.extend_from_slice(b);
            }
            Cbor::Text(t) => {
                Cbor::head(out, 3, t.len() as u64);
                out.extend_from_slice(t.as_bytes());
            }
            Cbor::Array(items) => {
                Cbor::head(out, 4, items.len() as u64);
                for item in items {
                    item.write(out);
                }
            }
            Cbor::Map(entries) => {
                Cbor::head(out, 5, entries.len() as u64);
                for (k, v) in entries {
                    k.write(out);
                    v.write(out);
                }
            }
            Cbor::Tag(tag, value) => {
                Cbor::head(out, 6, *tag);
                value.write(out);
            }
            Cbor::Bool(b) => out.push(if *b { 0xf5 } else { 0xf4 }),
            Cbor::Null => out.push(0xf6),
            Cbor::Float(f) => {
                out.push(0xfb);
                out.extend(f.to_bits().to_be_bytes());
            }
        }
    }

    /// Reads one value from the whole of `bytes`.
    pub fn decode(bytes: &[u8]) -> Result<Cbor, String> {
        let mut at = 0;
        let value = Cbor::read(bytes, &mut at, 0)?;
        if at != bytes.len() {
            return Err("bytes after the CBOR value".into());
        }
        Ok(value)
    }

    fn read(bytes: &[u8], at: &mut usize, depth: usize) -> Result<Cbor, String> {
        if depth > 64 {
            return Err("CBOR nested too deep".into());
        }
        let take = |at: &mut usize, n: usize| -> Result<&[u8], String> {
            let end = at
                .checked_add(n)
                .filter(|e| *e <= bytes.len())
                .ok_or("CBOR cut short")?;
            let slice = &bytes[*at..end];
            *at = end;
            Ok(slice)
        };
        let first = take(at, 1)?[0];
        let (major, info) = (first >> 5, first & 0x1f);
        let argument = |at: &mut usize| -> Result<u64, String> {
            Ok(match info {
                0..=23 => u64::from(info),
                24 => u64::from(take(at, 1)?[0]),
                25 => u64::from(u16::from_be_bytes(
                    take(at, 2)?.try_into().unwrap_or_default(),
                )),
                26 => u64::from(u32::from_be_bytes(
                    take(at, 4)?.try_into().unwrap_or_default(),
                )),
                27 => u64::from_be_bytes(take(at, 8)?.try_into().unwrap_or_default()),
                _ => return Err("indefinite or reserved CBOR lengths are not read".into()),
            })
        };
        let length = |n: u64| -> Result<usize, String> {
            usize::try_from(n)
                .ok()
                .filter(|n| *n <= bytes.len())
                .ok_or_else(|| "a CBOR length past its bytes".to_string())
        };
        Ok(match major {
            0 => Cbor::Int(i128::from(argument(at)?)),
            1 => Cbor::Int(-1 - i128::from(argument(at)?)),
            2 => {
                let n = length(argument(at)?)?;
                Cbor::Bytes(take(at, n)?.to_vec())
            }
            3 => {
                let n = length(argument(at)?)?;
                Cbor::Text(
                    String::from_utf8(take(at, n)?.to_vec()).map_err(|_| "CBOR text not UTF-8")?,
                )
            }
            4 => {
                let n = length(argument(at)?)?;
                let mut items = Vec::with_capacity(n.min(1024));
                for _ in 0..n {
                    items.push(Cbor::read(bytes, at, depth + 1)?);
                }
                Cbor::Array(items)
            }
            5 => {
                let n = length(argument(at)?)?;
                let mut entries = Vec::with_capacity(n.min(1024));
                for _ in 0..n {
                    let k = Cbor::read(bytes, at, depth + 1)?;
                    let v = Cbor::read(bytes, at, depth + 1)?;
                    entries.push((k, v));
                }
                Cbor::Map(entries)
            }
            6 => {
                let tag = argument(at)?;
                Cbor::Tag(tag, Box::new(Cbor::read(bytes, at, depth + 1)?))
            }
            _ => match info {
                20 => Cbor::Bool(false),
                21 => Cbor::Bool(true),
                22 | 23 => Cbor::Null,
                25 => {
                    let half = u16::from_be_bytes(take(at, 2)?.try_into().unwrap_or_default());
                    Cbor::Float(half_to_f64(half))
                }
                26 => Cbor::Float(f64::from(f32::from_bits(u32::from_be_bytes(
                    take(at, 4)?.try_into().unwrap_or_default(),
                )))),
                27 => Cbor::Float(f64::from_bits(u64::from_be_bytes(
                    take(at, 8)?.try_into().unwrap_or_default(),
                ))),
                _ => return Err("a CBOR simple value this does not read".into()),
            },
        })
    }
}

fn half_to_f64(half: u16) -> f64 {
    let exponent = (half >> 10) & 0x1f;
    let mantissa = f64::from(half & 0x3ff);
    let value = match exponent {
        0 => mantissa * 2f64.powi(-24),
        31 => {
            if mantissa == 0.0 {
                f64::INFINITY
            } else {
                f64::NAN
            }
        }
        e => (mantissa + 1024.0) * 2f64.powi(i32::from(e) - 25),
    };
    if half & 0x8000 != 0 {
        -value
    } else {
        value
    }
}

/// A box: its four-character type and its payload.
fn plain_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

/// A JUMBF superbox of type `kind`, labelled, with a salt when given, its
/// content boxes after its description.
fn superbox(kind: [u8; 16], label: &str, salt: Option<&[u8; 16]>, contents: &[Vec<u8>]) -> Vec<u8> {
    let mut description = kind.to_vec();
    // Requestable, labelled, and with a private salt box when salted.
    description.push(if salt.is_some() { 0x13 } else { 0x03 });
    description.extend_from_slice(label.as_bytes());
    description.push(0);
    if let Some(salt) = salt {
        description.extend(plain_box(b"c2sh", salt));
    }
    let mut payload = plain_box(b"jumd", &description);
    for content in contents {
        payload.extend_from_slice(content);
    }
    plain_box(b"jumb", &payload)
}

/// A box read back: its type, where its payload is.
#[derive(Clone, Copy, Debug)]
struct Found {
    kind: [u8; 4],
    start: usize,
    end: usize,
}

fn boxes(bytes: &[u8], mut at: usize, end: usize) -> Result<Vec<Found>, String> {
    let mut found = Vec::new();
    while at < end {
        if end - at < 8 {
            return Err("a JUMBF box cut short".into());
        }
        let mut size =
            u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap_or_default()) as usize;
        let kind: [u8; 4] = bytes[at + 4..at + 8].try_into().unwrap_or_default();
        let mut header = 8;
        if size == 1 {
            if end - at < 16 {
                return Err("a JUMBF box cut short".into());
            }
            size =
                u64::from_be_bytes(bytes[at + 8..at + 16].try_into().unwrap_or_default()) as usize;
            header = 16;
        } else if size == 0 {
            size = end - at;
        }
        if size < header || at.checked_add(size).map_or(true, |e| e > end) {
            return Err("a JUMBF box's size runs past its bytes".into());
        }
        found.push(Found {
            kind,
            start: at + header,
            end: at + size,
        });
        at += size;
    }
    Ok(found)
}

/// A superbox read back.
#[derive(Clone, Debug)]
struct Super {
    kind: [u8; 16],
    label: String,
    /// Its payload, after its header: what its hash is taken over.
    contents: (usize, usize),
    /// The boxes after its description.
    children: Vec<Found>,
}

fn read_super(bytes: &[u8], found: &Found) -> Result<Super, String> {
    if &found.kind != b"jumb" {
        return Err("not a JUMBF superbox".into());
    }
    let inner = boxes(bytes, found.start, found.end)?;
    let description = inner
        .first()
        .filter(|b| &b.kind == b"jumd")
        .ok_or("a JUMBF superbox without its description")?;
    let d = &bytes[description.start..description.end];
    if d.len() < 17 {
        return Err("a JUMBF description cut short".into());
    }
    let kind: [u8; 16] = d[..16].try_into().unwrap_or_default();
    let toggles = d[16];
    let label = if toggles & 0x02 != 0 {
        let rest = &d[17..];
        let end = rest
            .iter()
            .position(|b| *b == 0)
            .ok_or("a JUMBF label unterminated")?;
        String::from_utf8(rest[..end].to_vec()).map_err(|_| "a JUMBF label not UTF-8")?
    } else {
        String::new()
    };
    Ok(Super {
        kind,
        label,
        contents: (found.start, found.end),
        children: inner[1..].to_vec(),
    })
}

fn random16() -> [u8; 16] {
    let mut bytes = [0u8; 16];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut bytes);
    bytes
}

fn uuid_v4() -> String {
    let mut b = random16();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode(b);
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// What made the asset.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Maker {
    /// The agent's DID.
    pub did: String,
    /// Its name.
    pub name: String,
    /// The models it ran on, if said.
    pub models: Vec<String>,
}

/// What was done to the asset: `c2pa.created` or `c2pa.edited`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// It was made from nothing.
    Created,
    /// It was changed.
    Edited,
}

impl Action {
    fn label(self) -> &'static str {
        match self {
            Action::Created => "c2pa.created",
            Action::Edited => "c2pa.edited",
        }
    }
}

fn software() -> Cbor {
    Cbor::map(vec![
        ("name", Cbor::text("Siontology")),
        ("version", Cbor::text(env!("CARGO_PKG_VERSION"))),
    ])
}

/// Signs `asset`, titled `title`, as made by `maker` with `action`: a
/// sidecar manifest store, signed with `key`, whose certificate is the
/// first of `chain` (DER, the signer's first).
pub fn sign(
    asset: &[u8],
    title: &str,
    maker: &Maker,
    action: Action,
    chain: &[Vec<u8>],
    key: &SigningKey,
) -> Result<Vec<u8>, CliError> {
    if chain.is_empty() {
        return Err(CliError::Usage(
            "a manifest is signed with a certificate".into(),
        ));
    }
    let label = format!("urn:c2pa:{}", uuid_v4());
    let mut action_entry = vec![
        ("action", Cbor::text(action.label())),
        ("softwareAgent", software()),
    ];
    if action == Action::Created {
        action_entry.push(("digitalSourceType", Cbor::text(TRAINED_ALGORITHMIC_MEDIA)));
    }
    let actions = Cbor::map(vec![(
        "actions",
        Cbor::Array(vec![Cbor::map(action_entry)]),
    )]);
    let mut agent = vec![
        ("did", Cbor::text(&maker.did)),
        ("name", Cbor::text(&maker.name)),
    ];
    if !maker.models.is_empty() {
        agent.push((
            "models",
            Cbor::Array(maker.models.iter().map(|m| Cbor::text(m)).collect()),
        ));
    }
    let hard = Cbor::map(vec![
        ("name", Cbor::text("jumbf manifest")),
        ("alg", Cbor::text("sha256")),
        ("hash", Cbor::Bytes(Sha256::digest(asset).to_vec())),
        ("pad", Cbor::Bytes(Vec::new())),
    ]);
    let assertions: Vec<(&str, Cbor)> = vec![
        ("c2pa.hash.data", hard),
        ("c2pa.actions.v2", actions),
        (AGENT_ASSERTION, Cbor::map(agent)),
    ];
    let mut boxes_written = Vec::new();
    let mut hashed = Vec::new();
    for (name, value) in &assertions {
        let boxed = superbox(
            CBOR,
            name,
            Some(&random16()),
            &[plain_box(b"cbor", &value.encode())],
        );
        hashed.push(Cbor::map(vec![
            (
                "url",
                Cbor::Text(format!("self#jumbf=c2pa.assertions/{name}")),
            ),
            ("hash", Cbor::Bytes(Sha256::digest(&boxed[8..]).to_vec())),
        ]));
        boxes_written.push(boxed);
    }
    let claim = Cbor::map(vec![
        ("instanceID", Cbor::Text(format!("xmp:iid:{}", uuid_v4()))),
        ("claim_generator_info", software()),
        (
            "signature",
            Cbor::Text(format!("self#jumbf=/c2pa/{label}/c2pa.signature")),
        ),
        ("created_assertions", Cbor::Array(hashed)),
        ("dc:title", Cbor::text(title)),
        ("alg", Cbor::text("sha256")),
    ])
    .encode();
    let x5chain = if chain.len() == 1 {
        Cbor::Bytes(chain[0].clone())
    } else {
        Cbor::Array(chain.iter().map(|c| Cbor::Bytes(c.clone())).collect())
    };
    let protected = Cbor::Map(vec![
        (Cbor::Int(1), Cbor::Int(-7)),
        (Cbor::Int(33), x5chain),
    ])
    .encode();
    let to_sign = Cbor::Array(vec![
        Cbor::text("Signature1"),
        Cbor::Bytes(protected.clone()),
        Cbor::Bytes(Vec::new()),
        Cbor::Bytes(claim.clone()),
    ])
    .encode();
    let signature = key.sign(&to_sign);
    let cose = Cbor::Tag(
        18,
        Box::new(Cbor::Array(vec![
            Cbor::Bytes(protected),
            Cbor::Map(Vec::new()),
            Cbor::Null,
            Cbor::Bytes(signature),
        ])),
    )
    .encode();
    let manifest = superbox(
        MANIFEST,
        &label,
        None,
        &[
            superbox(ASSERTIONS, "c2pa.assertions", None, &boxes_written),
            superbox(CLAIM, "c2pa.claim.v2", None, &[plain_box(b"cbor", &claim)]),
            superbox(
                SIGNATURE,
                "c2pa.signature",
                None,
                &[plain_box(b"cbor", &cose)],
            ),
        ],
    );
    Ok(superbox(C2PA, "c2pa", None, &[manifest]))
}

/// What a manifest store says of an asset, once checked.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// The active manifest's label.
    pub manifest: String,
    /// The asset's title, as the claim gives it.
    pub title: String,
    /// What made the claim: its claim generator's name.
    pub generator: String,
    /// The actions, by label, with their digital source types.
    pub actions: Vec<(String, Option<String>)>,
    /// The Siontology agent that made it, if the manifest names one.
    pub agent: Option<Maker>,
    /// The signer's certificate's subject.
    pub signer: String,
    /// Whether its chain leads to a root given as trusted.
    pub trusted: bool,
    /// Every assertion the claim lists, by label.
    pub assertions: Vec<String>,
}

fn bad(why: impl Into<String>) -> CliError {
    CliError::Usage(why.into())
}

/// The P-256 key and subject of the certificate `der`.
fn leaf_key(der: &[u8]) -> Result<(PublicKey, String), CliError> {
    use x509_cert::der::Decode;
    let cert = x509_cert::Certificate::from_der(der)
        .map_err(|e| bad(format!("the signer's certificate: {e}")))?;
    let tbs = &cert.tbs_certificate;
    let point = tbs.subject_public_key_info.subject_public_key.raw_bytes();
    if point.len() != 65 || point[0] != 4 {
        return Err(bad("the signer's certificate holds no P-256 key (ES256)"));
    }
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let key = PublicKey::from_jwk(&serde_json::json!({
        "kty": "EC", "crv": "P-256",
        "x": b64.encode(&point[1..33]), "y": b64.encode(&point[33..65]),
    }))
    .map_err(|e| bad(e.to_string()))?;
    Ok((key, tbs.subject.to_string()))
}

/// Checks the manifest store `store` against `asset`: the active manifest's
/// signature (ES256), with its chain to one of `roots` (DER) when any are
/// given; the hash of every assertion its claim lists; and its data hash
/// over the asset. Errors on the first thing that fails.
pub fn verify(asset: &[u8], store: &[u8], roots: &[Vec<u8>], now: u64) -> Result<Report, CliError> {
    let top = boxes(store, 0, store.len()).map_err(bad)?;
    let top = top.first().ok_or_else(|| bad("no manifest store"))?;
    let top = read_super(store, top).map_err(bad)?;
    if top.kind != C2PA {
        return Err(bad("not a C2PA manifest store"));
    }
    let manifests: Vec<Super> = top
        .children
        .iter()
        .filter_map(|b| read_super(store, b).ok())
        .filter(|s| s.kind == MANIFEST || s.kind == uuid(*b"c2um"))
        .collect();
    let active = manifests
        .last()
        .ok_or_else(|| bad("the store holds no manifest"))?;
    let parts: Vec<Super> = active
        .children
        .iter()
        .filter_map(|b| read_super(store, b).ok())
        .collect();
    let part = |kind: [u8; 16]| parts.iter().find(|p| p.kind == kind);
    let content = |s: &Super| -> Result<Vec<u8>, CliError> {
        let first = s
            .children
            .iter()
            .find(|b| &b.kind == b"cbor")
            .ok_or_else(|| bad(format!("{} holds no CBOR", s.label)))?;
        Ok(store[first.start..first.end].to_vec())
    };
    let claim_box = part(CLAIM).ok_or_else(|| bad("the manifest holds no claim"))?;
    let claim_bytes = content(claim_box)?;
    let claim = Cbor::decode(&claim_bytes).map_err(bad)?;
    // The signature.
    let signature_box = part(SIGNATURE).ok_or_else(|| bad("the manifest holds no signature"))?;
    let cose = Cbor::decode(&content(signature_box)?).map_err(bad)?;
    let Cbor::Tag(18, sign1) = cose else {
        return Err(bad("the signature is not a COSE_Sign1"));
    };
    let Cbor::Array(sign1) = *sign1 else {
        return Err(bad("the signature is not a COSE_Sign1"));
    };
    let [Cbor::Bytes(protected), _, _, Cbor::Bytes(signature)] = sign1.as_slice() else {
        return Err(bad("the signature is not a COSE_Sign1"));
    };
    let header = Cbor::decode(protected).map_err(bad)?;
    if header.get_int(1) != Some(&Cbor::Int(-7)) {
        return Err(bad(
            "the signature is not ES256, the only algorithm checked here",
        ));
    }
    let chain: Vec<Vec<u8>> = match header.get_int(33) {
        Some(Cbor::Bytes(one)) => vec![one.clone()],
        Some(Cbor::Array(many)) => many
            .iter()
            .filter_map(|c| c.as_bytes().map(<[u8]>::to_vec))
            .collect(),
        _ => return Err(bad("the signature carries no certificate (x5chain)")),
    };
    let (key, signer) = leaf_key(chain.first().ok_or_else(|| bad("an empty x5chain"))?)?;
    let to_sign = Cbor::Array(vec![
        Cbor::text("Signature1"),
        Cbor::Bytes(protected.clone()),
        Cbor::Bytes(Vec::new()),
        Cbor::Bytes(claim_bytes.clone()),
    ])
    .encode();
    if !key.verifies(&to_sign, signature) {
        return Err(bad("the claim's signature does not verify"));
    }
    let trusted = if roots.is_empty() {
        false
    } else {
        let b64 = base64::engine::general_purpose::STANDARD;
        let x5c = serde_json::Value::Array(
            chain
                .iter()
                .map(|c| serde_json::Value::String(b64.encode(c)))
                .collect(),
        );
        let chained = crate::ap2::x5c::key_of(&x5c, roots, now)
            .map_err(|e| bad(format!("the signer's chain: {e}")))?;
        if chained != key {
            return Err(bad("the chain's signer is not the certificate that signed"));
        }
        true
    };
    // Every assertion the claim lists, by the hash of its box.
    let store_part = part(ASSERTIONS).ok_or_else(|| bad("the manifest holds no assertions"))?;
    let assertions: Vec<Super> = store_part
        .children
        .iter()
        .filter_map(|b| read_super(store, b).ok())
        .collect();
    let mut report = Report {
        manifest: active.label.clone(),
        title: claim
            .get("dc:title")
            .and_then(Cbor::as_text)
            .unwrap_or_default()
            .to_string(),
        generator: claim
            .get("claim_generator_info")
            .and_then(|g| match g {
                Cbor::Array(items) => items.first(),
                other => Some(other),
            })
            .and_then(|g| g.get("name"))
            .and_then(Cbor::as_text)
            .or_else(|| claim.get("claim_generator").and_then(Cbor::as_text))
            .unwrap_or_default()
            .to_string(),
        signer,
        trusted,
        ..Report::default()
    };
    let mut listed = Vec::new();
    for key in ["created_assertions", "gathered_assertions", "assertions"] {
        if let Some(Cbor::Array(items)) = claim.get(key) {
            listed.extend(items.iter().cloned());
        }
    }
    for entry in &listed {
        let url = entry.get("url").and_then(Cbor::as_text).unwrap_or_default();
        let hash = entry
            .get("hash")
            .and_then(Cbor::as_bytes)
            .unwrap_or_default();
        let label = url.rsplit('/').next().unwrap_or_default();
        let found = assertions
            .iter()
            .find(|a| a.label == label)
            .ok_or_else(|| bad(format!("assertion {label} is listed but missing")))?;
        let (start, end) = found.contents;
        if Sha256::digest(&store[start..end]).as_slice() != hash {
            return Err(bad(format!("assertion {label} does not match its hash")));
        }
        report.assertions.push(label.to_string());
        let value = || -> Result<Cbor, CliError> { Cbor::decode(&content(found)?).map_err(bad) };
        if label == "c2pa.hash.data" {
            let value = value()?;
            let mut hasher = Sha256::new();
            let mut at = 0usize;
            let mut exclusions: Vec<(usize, usize)> = match value.get("exclusions") {
                Some(Cbor::Array(items)) => items
                    .iter()
                    .filter_map(|x| match (x.get("start"), x.get("length")) {
                        (Some(Cbor::Int(s)), Some(Cbor::Int(l))) => {
                            Some((usize::try_from(*s).ok()?, usize::try_from(*l).ok()?))
                        }
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            exclusions.sort();
            for (start, length) in exclusions {
                let start = start.min(asset.len());
                hasher.update(&asset[at.min(start)..start]);
                at = start.saturating_add(length).min(asset.len());
            }
            hasher.update(&asset[at.min(asset.len())..]);
            if Some(hasher.finalize().as_slice()) != value.get("hash").and_then(Cbor::as_bytes) {
                return Err(bad(
                    "the asset is not the one signed: its data hash differs",
                ));
            }
        } else if label.starts_with("c2pa.actions") {
            if let Some(Cbor::Array(items)) = value()?.get("actions") {
                for item in items {
                    report.actions.push((
                        item.get("action")
                            .and_then(Cbor::as_text)
                            .unwrap_or_default()
                            .to_string(),
                        item.get("digitalSourceType")
                            .and_then(Cbor::as_text)
                            .map(str::to_string),
                    ));
                }
            }
        } else if label == AGENT_ASSERTION {
            let value = value()?;
            report.agent = Some(Maker {
                did: value
                    .get("did")
                    .and_then(Cbor::as_text)
                    .unwrap_or_default()
                    .to_string(),
                name: value
                    .get("name")
                    .and_then(Cbor::as_text)
                    .unwrap_or_default()
                    .to_string(),
                models: match value.get("models") {
                    Some(Cbor::Array(items)) => items
                        .iter()
                        .filter_map(|m| m.as_text().map(str::to_string))
                        .collect(),
                    _ => Vec::new(),
                },
            });
        }
    }
    if !report.assertions.iter().any(|a| a == "c2pa.hash.data") {
        return Err(bad(
            "the claim holds no hard binding (c2pa.hash.data) checked here",
        ));
    }
    Ok(report)
}

/// The DER of each certificate in `pem`.
pub fn certificates(pem: &str) -> Result<Vec<Vec<u8>>, CliError> {
    crate::ap2::x5c::pem_certificates(pem).map_err(|e| bad(e.to_string()))
}

/// A P-256 key from a PKCS#8 PEM (`BEGIN PRIVATE KEY`).
pub fn private_key(pem: &str) -> Result<SigningKey, CliError> {
    let body: String = pem
        .lines()
        .map(str::trim)
        .skip_while(|l| !l.starts_with("-----BEGIN PRIVATE KEY-----"))
        .skip(1)
        .take_while(|l| !l.starts_with("-----END"))
        .collect();
    let der = base64::engine::general_purpose::STANDARD
        .decode(body)
        .map_err(|_| bad("the key is not a PKCS#8 PEM (BEGIN PRIVATE KEY)"))?;
    SigningKey::from_pkcs8(&der).map_err(bad)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cbor_round_trips_what_claims_use() {
        let value = Cbor::Map(vec![
            (Cbor::text("a"), Cbor::Int(-7)),
            (
                Cbor::Int(33),
                Cbor::Array(vec![Cbor::Bytes(vec![1; 300]), Cbor::Null]),
            ),
            (Cbor::text("t"), Cbor::Tag(18, Box::new(Cbor::Bool(true)))),
            (Cbor::text("big"), Cbor::Int(1 << 40)),
        ]);
        let bytes = value.encode();
        assert_eq!(Cbor::decode(&bytes).unwrap(), value);
        // RFC 8949's examples.
        assert_eq!(Cbor::Int(-1000).encode(), vec![0x39, 0x03, 0xe7]);
        assert_eq!(
            Cbor::Int(1_000_000).encode(),
            vec![0x1a, 0x00, 0x0f, 0x42, 0x40]
        );
        assert_eq!(Cbor::decode(&[0xf9, 0x3c, 0x00]).unwrap(), Cbor::Float(1.0));
        assert!(
            Cbor::decode(&[0x5f]).is_err(),
            "indefinite lengths are refused"
        );
    }

    #[test]
    fn a_superbox_reads_back_with_its_label() {
        let boxed = superbox(
            CBOR,
            "c2pa.actions.v2",
            Some(&[7; 16]),
            &[plain_box(b"cbor", &[0xa0])],
        );
        let found = boxes(&boxed, 0, boxed.len()).unwrap();
        let read = read_super(&boxed, &found[0]).unwrap();
        assert_eq!(read.label, "c2pa.actions.v2");
        assert_eq!(read.kind, CBOR);
        assert_eq!(read.children.len(), 1);
        assert_eq!(&read.children[0].kind, b"cbor");
        assert_eq!(read.contents, (8, boxed.len()));
    }
}

/// `sion agent c2pa`: Content Credentials for what the agent makes.
#[derive(clap::Subcommand, Debug)]
pub enum C2paCommand {
    /// Sign a file as made by the profile's agent: a C2PA manifest
    /// (Content Credentials) kept beside it as <file>.c2pa, binding its
    /// bytes, saying that trained algorithmic media made it (or edited it,
    /// with --edited), by Siontology, and naming the agent by its DID.
    Sign {
        /// The file.
        file: std::path::PathBuf,
        /// The profile whose agent made it.
        #[arg(long)]
        profile: String,
        /// The signer's certificate chain, PEM, the signer's first.
        #[arg(long, value_name = "PEM")]
        cert: std::path::PathBuf,
        /// The signer's P-256 key, PKCS#8 PEM.
        #[arg(long, value_name = "PEM")]
        key: std::path::PathBuf,
        /// It was edited, not made from nothing.
        #[arg(long)]
        edited: bool,
        /// Where to write the manifest (default: <file>.c2pa).
        #[arg(long, value_name = "FILE")]
        out: Option<std::path::PathBuf>,
    },
    /// Check a file's Content Credentials against the file: the claim's
    /// signature, every assertion's hash, and the data hash; with --trust,
    /// that the signer's chain leads to a root you trust.
    Verify {
        /// The file.
        file: std::path::PathBuf,
        /// Its manifest (default: <file>.c2pa).
        #[arg(long, value_name = "FILE")]
        manifest: Option<std::path::PathBuf>,
        /// A root certificate you trust, PEM (repeat).
        #[arg(long, value_name = "PEM")]
        trust: Vec<std::path::PathBuf>,
    },
}

impl C2paCommand {
    /// Whether it needs a node: signing names the agent's DID.
    pub fn needs_ledger(&self) -> bool {
        matches!(self, C2paCommand::Sign { .. })
    }
}

fn read(path: &std::path::Path) -> Result<Vec<u8>, CliError> {
    std::fs::read(path).map_err(|e| bad(format!("{}: {e}", path.display())))
}

fn sidecar(file: &std::path::Path) -> std::path::PathBuf {
    let mut name = file.as_os_str().to_os_string();
    name.push(".c2pa");
    std::path::PathBuf::from(name)
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Runs a C2PA command that needs no node; gives back one that does.
pub fn local(command: C2paCommand) -> Result<Result<(), CliError>, Box<C2paCommand>> {
    match command {
        C2paCommand::Verify {
            file,
            manifest,
            trust,
        } => Ok((|| {
            let manifest = manifest.unwrap_or_else(|| sidecar(&file));
            let mut roots = Vec::new();
            for path in &trust {
                roots.extend(certificates(&String::from_utf8_lossy(&read(path)?))?);
            }
            let report = verify(&read(&file)?, &read(&manifest)?, &roots, now())?;
            println!(
                "{}: Content Credentials hold ({} assertions checked, the data hash among them)",
                file.display(),
                report.assertions.len()
            );
            println!("  title: {}; made with {}", report.title, report.generator);
            for (action, source) in &report.actions {
                match source {
                    Some(source) => println!("  {action}, by {source}"),
                    None => println!("  {action}"),
                }
            }
            if let Some(agent) = &report.agent {
                println!("  by agent {} ({})", agent.name, agent.did);
            }
            println!(
                "  signed by {}: {}",
                report.signer,
                if report.trusted {
                    "its chain leads to a root you trust"
                } else if trust.is_empty() {
                    "no root was given to trust (--trust)"
                } else {
                    "untrusted"
                }
            );
            Ok(())
        })()),
        other => Err(Box::new(other)),
    }
}

/// Runs a C2PA command.
pub fn run(ledger: &dyn crate::ledger::Ledger, command: C2paCommand) -> Result<(), CliError> {
    let C2paCommand::Sign {
        file,
        profile,
        cert,
        key,
        edited,
        out,
    } = command
    else {
        return local(command).unwrap_or_else(|_| Err(bad("this needs a node")));
    };
    let found = crate::profiles::get(&profile)?;
    let maker = Maker {
        did: crate::agents::did(ledger, &found.agent)?,
        name: found.name.clone(),
        models: found.models.clone(),
    };
    let chain = certificates(&String::from_utf8_lossy(&read(&cert)?))?;
    let key = private_key(&String::from_utf8_lossy(&read(&key)?))?;
    let asset = read(&file)?;
    let title = file
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let store = sign(
        &asset,
        &title,
        &maker,
        if edited {
            Action::Edited
        } else {
            Action::Created
        },
        &chain,
        &key,
    )?;
    let out = out.unwrap_or_else(|| sidecar(&file));
    std::fs::write(&out, &store).map_err(|e| bad(format!("{}: {e}", out.display())))?;
    println!(
        "signed {} as made by agent {} ({}): {}",
        file.display(),
        maker.name,
        maker.did,
        out.display()
    );
    Ok(())
}
