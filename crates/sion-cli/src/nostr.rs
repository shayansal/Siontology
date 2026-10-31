//! Nostr's direct messages: private messages as NIP-17 has them (NIP-44
//! encryption, sealed and gift-wrapped as NIP-59 says) and the older
//! encrypted ones of NIP-04, over the relays in `NOSTR_RELAYS`, signed
//! with the key in `NOSTR_PRIVATE_KEY` (`nsec1…` or hex).
//!
//! A chat is the writer's public key as an `npub1…`; a reply goes the way
//! the message came (`NOSTR_DM`, `nip17` by default, for a chat that has
//! not written yet). If the account names no relays for its private
//! messages, the bridge names its own (a kind 10050 event); it sends a
//! gift-wrapped reply to the relays its reader names for theirs (kind
//! 10050) as well as its own, an older one to the relays the reader reads
//! (kind 10002). Every event is checked against its signature, and a
//! sealed message against who sealed it: a relay can withhold messages,
//! not forge them.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sion_http::ws::WebSocket;

use crate::live::{Link, Live};
use crate::messengers::{Incoming, Messenger, LIVE_ONCE_WAIT};

/// The relays, unless `NOSTR_RELAYS` names others.
pub const NOSTR_RELAYS: &str = "wss://relay.damus.io,wss://nos.lol";

/// How far in the past NIP-59 puts a seal's and a gift wrap's time.
const TWO_DAYS: u64 = 2 * 24 * 60 * 60;

/// The most characters of one message: a gift wrap holds its seal, which
/// holds the message, each encrypted within NIP-44's 65,535 bytes.
const NOSTR_TEXT: usize = 8_000;

/// The longest event content read: anything longer is dropped unread.
const MAX_CONTENT: usize = 256 << 10;

/// How long a reply waits for a relay to take it.
const PUBLISH_WAIT: Duration = Duration::from_secs(15);

/// How long the relays a reader names are kept.
const INBOXES_KEPT: Duration = Duration::from_secs(60 * 60);

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn random<const N: usize>() -> Result<[u8; N], String> {
    let mut bytes = [0u8; N];
    sion_http::random(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes)
}

// ---------------------------------------------------------------- NIP-19

const CHARSET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";

fn polymod(values: &[u8]) -> u32 {
    const GEN: [u32; 5] = [
        0x3b6a_57b2,
        0x2650_8e6d,
        0x1ea1_19fa,
        0x3d42_33dd,
        0x2a14_62b3,
    ];
    let mut chk: u32 = 1;
    for v in values {
        let top = chk >> 25;
        chk = ((chk & 0x01ff_ffff) << 5) ^ u32::from(*v);
        for (i, g) in GEN.iter().enumerate() {
            if (top >> i) & 1 == 1 {
                chk ^= g;
            }
        }
    }
    chk
}

fn hrp_expanded(hrp: &str) -> Vec<u8> {
    let mut out: Vec<u8> = hrp.bytes().map(|b| b >> 5).collect();
    out.push(0);
    out.extend(hrp.bytes().map(|b| b & 31));
    out
}

fn regroup(data: &[u8], from: u32, to: u32, pad: bool) -> Option<Vec<u8>> {
    let (mut acc, mut bits, mut out) = (0u32, 0u32, Vec::new());
    let max = (1u32 << to) - 1;
    for value in data {
        let value = u32::from(*value);
        if value >> from != 0 {
            return None;
        }
        acc = (acc << from) | value;
        bits += from;
        while bits >= to {
            bits -= to;
            out.push(((acc >> bits) & max) as u8);
        }
    }
    if pad {
        if bits > 0 {
            out.push(((acc << (to - bits)) & max) as u8);
        }
    } else if bits >= from || ((acc << (to - bits)) & max) != 0 {
        return None;
    }
    Some(out)
}

/// `data` in bech32 under `hrp` (NIP-19's `npub`, `nsec`).
pub fn bech32_encode(hrp: &str, data: &[u8]) -> String {
    let five = regroup(data, 8, 5, true).unwrap_or_default();
    let mut values = hrp_expanded(hrp);
    values.extend(&five);
    values.extend([0u8; 6]);
    let check = polymod(&values) ^ 1;
    let mut out = format!("{hrp}1");
    for v in five
        .iter()
        .copied()
        .chain((0..6).map(|i| ((check >> (5 * (5 - i))) & 31) as u8))
    {
        out.push(CHARSET[v as usize] as char);
    }
    out
}

/// The part before `1` and the bytes of a bech32 string, if its checksum
/// holds.
pub fn bech32_decode(text: &str) -> Result<(String, Vec<u8>), String> {
    let lower = text.to_ascii_lowercase();
    if lower != text && text.to_ascii_uppercase() != text {
        return Err("mixed case".into());
    }
    let at = lower.rfind('1').ok_or("no separator")?;
    let (hrp, rest) = (&lower[..at], &lower[at + 1..]);
    if hrp.is_empty() || rest.len() < 6 {
        return Err("too short".into());
    }
    let values: Vec<u8> = rest
        .bytes()
        .map(|b| CHARSET.iter().position(|c| *c == b).map(|p| p as u8))
        .collect::<Option<_>>()
        .ok_or("not bech32")?;
    let mut checked = hrp_expanded(hrp);
    checked.extend(&values);
    if polymod(&checked) != 1 {
        return Err("its checksum does not hold".into());
    }
    let data = regroup(&values[..values.len() - 6], 5, 8, false).ok_or("bad padding")?;
    Ok((hrp.to_string(), data))
}

fn key_of(text: &str, hrp: &str) -> Result<[u8; 32], String> {
    let text = text.trim();
    let text = text.strip_prefix("nostr:").unwrap_or(text);
    let bytes = if text.starts_with(&format!("{hrp}1")) {
        let (found, bytes) = bech32_decode(text).map_err(|e| format!("{text}: {e}"))?;
        if found != hrp {
            return Err(format!("{text} is not an {hrp}"));
        }
        bytes
    } else {
        hex::decode(text).map_err(|_| format!("{text} is neither an {hrp}1… nor hex"))?
    };
    bytes
        .try_into()
        .map_err(|_| format!("{text} is not 32 bytes"))
}

/// A public key from an `npub1…` or 64 hex digits.
pub fn public_key_of(text: &str) -> Result<[u8; 32], String> {
    key_of(text, "npub")
}

/// A public key as an `npub1…`.
pub fn npub(public: &[u8; 32]) -> String {
    bech32_encode("npub", public)
}

// ---------------------------------------------------------------- NIP-01

/// A signed event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Its id: SHA-256 of its serialization, hex.
    pub id: String,
    /// Who signed it, hex (x only).
    pub pubkey: String,
    /// When, in seconds since 1970.
    pub created_at: u64,
    /// What kind of event.
    pub kind: u64,
    /// Its tags.
    pub tags: Vec<Vec<String>>,
    /// What it says.
    pub content: String,
    /// The signature (BIP-340), hex.
    pub sig: String,
}

/// `text` as NIP-01 serializes a string: only `\n`, `"`, `\`, `\r`,
/// `\t`, backspace and form feed escaped.
fn push_string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// The serialization an event's id is the hash of.
pub fn serialized(
    pubkey: &str,
    created_at: u64,
    kind: u64,
    tags: &[Vec<String>],
    content: &str,
) -> String {
    let mut out = String::from("[0,");
    push_string(&mut out, pubkey);
    out.push_str(&format!(",{created_at},{kind},["));
    for (i, tag) in tags.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('[');
        for (j, part) in tag.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            push_string(&mut out, part);
        }
        out.push(']');
    }
    out.push_str("],");
    push_string(&mut out, content);
    out.push(']');
    out
}

fn id_of(
    pubkey: &str,
    created_at: u64,
    kind: u64,
    tags: &[Vec<String>],
    content: &str,
) -> [u8; 32] {
    sion_http::sha256(serialized(pubkey, created_at, kind, tags, content).as_bytes())
}

/// Whether `event` is what its id says and signed by its key.
pub fn verify(event: &Event) -> bool {
    let id = id_of(
        &event.pubkey,
        event.created_at,
        event.kind,
        &event.tags,
        &event.content,
    );
    let (Ok(given), Ok(public), Ok(sig)) = (
        hex::decode(&event.id),
        hex::decode(&event.pubkey),
        hex::decode(&event.sig),
    ) else {
        return false;
    };
    if given != id {
        return false;
    }
    let (Ok(key), Ok(sig)) = (
        k256::schnorr::VerifyingKey::from_bytes(&public),
        k256::schnorr::Signature::try_from(sig.as_slice()),
    ) else {
        return false;
    };
    key.verify_raw(&id, &sig).is_ok()
}

/// A secret key and its public key.
pub struct Keys {
    signing: k256::schnorr::SigningKey,
    /// The public key, x only.
    pub public: [u8; 32],
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Keys({})", npub(&self.public))
    }
}

impl Keys {
    /// The keys of `secret`: an `nsec1…` or 64 hex digits.
    pub fn parse(secret: &str) -> Result<Keys, String> {
        Keys::from_secret(&key_of(secret, "nsec")?)
    }

    /// The keys of the 32 bytes `secret`.
    pub fn from_secret(secret: &[u8; 32]) -> Result<Keys, String> {
        let signing = k256::schnorr::SigningKey::from_bytes(secret)
            .map_err(|_| "not a secp256k1 secret key".to_string())?;
        let public: [u8; 32] = signing.verifying_key().to_bytes().into();
        Ok(Keys { signing, public })
    }

    /// New keys, at random (a gift wrap's own).
    pub fn generate() -> Result<Keys, String> {
        loop {
            if let Ok(keys) = Keys::from_secret(&random::<32>()?) {
                return Ok(keys);
            }
        }
    }

    /// The public key, hex.
    pub fn public_hex(&self) -> String {
        hex::encode(self.public)
    }

    /// An event of `kind` signed with these keys.
    pub fn sign(
        &self,
        kind: u64,
        tags: Vec<Vec<String>>,
        content: String,
        created_at: u64,
    ) -> Result<Event, String> {
        let pubkey = self.public_hex();
        let id = id_of(&pubkey, created_at, kind, &tags, &content);
        let sig = self
            .signing
            .sign_raw(&id, &random::<32>()?)
            .map_err(|_| "could not sign".to_string())?;
        Ok(Event {
            id: hex::encode(id),
            pubkey,
            created_at,
            kind,
            tags,
            content,
            sig: hex::encode(sig.to_bytes()),
        })
    }

    /// The x-coordinate of the point these keys share with `public`
    /// (ECDH, unhashed), as NIP-04 and NIP-44 take it.
    fn shared_x(&self, public: &[u8; 32]) -> Result<[u8; 32], String> {
        let mut sec1 = [2u8; 33];
        sec1[1..].copy_from_slice(public);
        let point = k256::PublicKey::from_sec1_bytes(&sec1)
            .map_err(|_| format!("{} is not a public key", hex::encode(public)))?;
        let shared =
            k256::ecdh::diffie_hellman(self.signing.as_nonzero_scalar(), point.as_affine());
        Ok((*shared.raw_secret_bytes()).into())
    }

    /// NIP-44's conversation key with `public`.
    pub fn conversation_key(&self, public: &[u8; 32]) -> Result<[u8; 32], String> {
        Ok(sion_http::hmac_sha256(b"nip44-v2", &self.shared_x(public)?))
    }
}

// ---------------------------------------------------------------- NIP-44

/// HKDF-SHA256's expansion of `prk` with `info`, `N` bytes (N ≤ 255 × 32).
fn hkdf_expand<const N: usize>(prk: &[u8; 32], info: &[u8]) -> [u8; N] {
    let (mut out, mut last, mut counter) = ([0u8; N], Vec::new(), 1u8);
    let mut filled = 0;
    while filled < N {
        let mut input = last.clone();
        input.extend_from_slice(info);
        input.push(counter);
        let block = sion_http::hmac_sha256(prk, &input);
        let take = (N - filled).min(32);
        out[filled..filled + take].copy_from_slice(&block[..take]);
        filled += take;
        last = block.to_vec();
        counter += 1;
    }
    out
}

/// The ChaCha20 key and nonce and the HMAC key of one message.
pub fn message_keys(
    conversation_key: &[u8; 32],
    nonce: &[u8; 32],
) -> ([u8; 32], [u8; 12], [u8; 32]) {
    let keys: [u8; 76] = hkdf_expand(conversation_key, nonce);
    let mut chacha_key = [0u8; 32];
    let mut chacha_nonce = [0u8; 12];
    let mut hmac_key = [0u8; 32];
    chacha_key.copy_from_slice(&keys[..32]);
    chacha_nonce.copy_from_slice(&keys[32..44]);
    hmac_key.copy_from_slice(&keys[44..]);
    (chacha_key, chacha_nonce, hmac_key)
}

/// How long NIP-44 pads a plaintext of `len` bytes.
pub fn padded_len(len: usize) -> usize {
    if len <= 32 {
        return 32;
    }
    let next_power = 1usize << (usize::BITS - (len - 1).leading_zeros());
    let chunk = if next_power <= 256 {
        32
    } else {
        next_power / 8
    };
    chunk * ((len - 1) / chunk + 1)
}

fn chacha(key: &[u8; 32], nonce: &[u8; 12], data: &mut [u8]) {
    use chacha20::cipher::{KeyIvInit, StreamCipher};
    let mut cipher = chacha20::ChaCha20::new(key.into(), nonce.into());
    cipher.apply_keystream(data);
}

/// `plaintext` encrypted as NIP-44 (version 2) has it, with `nonce`.
pub fn nip44_encrypt(
    conversation_key: &[u8; 32],
    plaintext: &str,
    nonce: &[u8; 32],
) -> Result<String, String> {
    let bytes = plaintext.as_bytes();
    if bytes.is_empty() || bytes.len() > 65_535 {
        return Err("a NIP-44 message holds 1 to 65,535 bytes".into());
    }
    let (chacha_key, chacha_nonce, hmac_key) = message_keys(conversation_key, nonce);
    let mut padded = Vec::with_capacity(2 + padded_len(bytes.len()));
    padded.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    padded.extend_from_slice(bytes);
    padded.resize(2 + padded_len(bytes.len()), 0);
    chacha(&chacha_key, &chacha_nonce, &mut padded);
    let mut maced = nonce.to_vec();
    maced.extend_from_slice(&padded);
    let mac = sion_http::hmac_sha256(&hmac_key, &maced);
    let mut payload = vec![2u8];
    payload.extend_from_slice(&maced);
    payload.extend_from_slice(&mac);
    Ok(base64::engine::general_purpose::STANDARD.encode(payload))
}

/// A NIP-44 (version 2) payload decrypted, its MAC and padding checked.
pub fn nip44_decrypt(conversation_key: &[u8; 32], payload: &str) -> Result<String, String> {
    if payload.starts_with('#') {
        return Err("an encryption version the bridge does not know".into());
    }
    if payload.len() < 132 || payload.len() > 87_472 {
        return Err("not a NIP-44 payload's length".into());
    }
    let data = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|_| "not base64".to_string())?;
    if data.len() < 99 || data.len() > 65_603 || data[0] != 2 {
        return Err("not a NIP-44 version 2 payload".into());
    }
    let nonce: [u8; 32] = data[1..33].try_into().map_err(|_| "no nonce")?;
    let (ciphertext, mac) = data[33..].split_at(data.len() - 33 - 32);
    let (chacha_key, chacha_nonce, hmac_key) = message_keys(conversation_key, &nonce);
    if !sion_http::same_bytes(
        &sion_http::hmac_sha256(&hmac_key, &data[1..data.len() - 32]),
        mac,
    ) {
        return Err("its MAC does not hold".into());
    }
    let mut padded = ciphertext.to_vec();
    chacha(&chacha_key, &chacha_nonce, &mut padded);
    let len = usize::from(u16::from_be_bytes([padded[0], padded[1]]));
    if len == 0 || padded.len() != 2 + padded_len(len) {
        return Err("its padding does not hold".into());
    }
    String::from_utf8(padded[2..2 + len].to_vec()).map_err(|_| "not UTF-8".into())
}

// ---------------------------------------------------------------- NIP-04

/// `plaintext` encrypted for `public` as NIP-04 has it (AES-256-CBC under
/// the shared x-coordinate), with `iv`.
pub fn nip04_encrypt(
    keys: &Keys,
    public: &[u8; 32],
    plaintext: &str,
    iv: &[u8; 16],
) -> Result<String, String> {
    let key = keys.shared_x(public)?;
    let padded = sion_http::cipher::pkcs7_pad(plaintext.as_bytes(), 16);
    let encrypted =
        sion_http::cipher::aes_cbc_encrypt(&key, iv, &padded).map_err(|e| e.to_string())?;
    let b64 = base64::engine::general_purpose::STANDARD;
    Ok(format!("{}?iv={}", b64.encode(encrypted), b64.encode(iv)))
}

/// A NIP-04 message from `public` decrypted.
pub fn nip04_decrypt(keys: &Keys, public: &[u8; 32], content: &str) -> Result<String, String> {
    let (data, iv) = content.split_once("?iv=").ok_or("not a NIP-04 message")?;
    let b64 = base64::engine::general_purpose::STANDARD;
    let data = b64.decode(data).map_err(|_| "not base64")?;
    let iv: [u8; 16] = b64
        .decode(iv)
        .ok()
        .and_then(|iv| iv.try_into().ok())
        .ok_or("not a NIP-04 IV")?;
    let key = keys.shared_x(public)?;
    let padded = sion_http::cipher::aes_cbc_decrypt(&key, &iv, &data).map_err(|e| e.to_string())?;
    let plain = sion_http::cipher::pkcs7_unpad(&padded, 16).map_err(|e| e.to_string())?;
    String::from_utf8(plain.to_vec()).map_err(|_| "not UTF-8".into())
}

// ---------------------------------------------------------------- NIP-59

/// A private message (kind 14) from `keys` to `recipient`, sealed (kind
/// 13) and gift-wrapped (kind 1059) for them under a key of its own, the
/// seal's and the wrap's times put up to two days back.
pub fn gift_wrap(
    keys: &Keys,
    recipient: &[u8; 32],
    text: &str,
    created_at: u64,
) -> Result<Event, String> {
    let to = hex::encode(recipient);
    let tags = vec![vec!["p".to_string(), to.clone()]];
    let pubkey = keys.public_hex();
    let rumor = json!({
        "id": hex::encode(id_of(&pubkey, created_at, 14, &tags, text)),
        "pubkey": pubkey,
        "created_at": created_at,
        "kind": 14,
        "tags": tags,
        "content": text,
    });
    let back = |bytes: [u8; 4]| u64::from(u32::from_be_bytes(bytes)) % TWO_DAYS;
    let sealed = nip44_encrypt(
        &keys.conversation_key(recipient)?,
        &rumor.to_string(),
        &random()?,
    )?;
    let seal = keys.sign(
        13,
        Vec::new(),
        sealed,
        created_at.saturating_sub(back(random()?)),
    )?;
    let wrapper = Keys::generate()?;
    let wrapped = nip44_encrypt(
        &wrapper.conversation_key(recipient)?,
        &serde_json::to_string(&seal).map_err(|e| e.to_string())?,
        &random()?,
    )?;
    wrapper.sign(
        1059,
        vec![vec!["p".to_string(), to]],
        wrapped,
        created_at.saturating_sub(back(random()?)),
    )
}

/// What a gift wrap for `keys` holds: who sealed it, and the message (its
/// `kind`, `content`, `tags`, `created_at`), checked to be theirs.
pub fn open_gift_wrap(keys: &Keys, wrap: &Event) -> Result<([u8; 32], Value), String> {
    if wrap.kind != 1059 || !verify(wrap) {
        return Err("not a signed gift wrap".into());
    }
    let wrapper = public_key_of(&wrap.pubkey)?;
    let seal: Event = serde_json::from_str(&nip44_decrypt(
        &keys.conversation_key(&wrapper)?,
        &wrap.content,
    )?)
    .map_err(|_| "the gift wrap holds no seal".to_string())?;
    if seal.kind != 13 || !verify(&seal) {
        return Err("the seal is not signed".into());
    }
    let sender = public_key_of(&seal.pubkey)?;
    let rumor: Value = serde_json::from_str(&nip44_decrypt(
        &keys.conversation_key(&sender)?,
        &seal.content,
    )?)
    .map_err(|_| "the seal holds no message".to_string())?;
    // Whoever sealed it is who it is from: a message naming another is
    // not taken.
    if rumor["pubkey"] != seal.pubkey.as_str() {
        return Err("the message names another than who sealed it".into());
    }
    Ok((sender, rumor))
}

// ---------------------------------------------------------------- relays

/// How a chat is written to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scheme {
    /// NIP-17's private messages, gift-wrapped.
    Private,
    /// NIP-04's encrypted messages (kind 4).
    Encrypted,
}

/// An event to publish, and where the first relay's answer goes.
struct Publish {
    event: Event,
    answer: Sender<Result<(), String>>,
}

/// The relays readers name, by reader: when asked, those for private
/// messages (kind 10050) and those they read (kind 10002).
type Inboxes = BTreeMap<String, (Instant, Vec<String>, Vec<String>)>;

/// Events published and not yet taken, by id: who waits, how many relays
/// may still answer, and what those that refused said.
type Waiting = BTreeMap<String, (Sender<Result<(), String>>, usize, Vec<String>)>;

/// What the bridge asks of its relays, and what they give back.
#[derive(Default)]
struct Shared {
    outbox: Mutex<VecDeque<Publish>>,
    /// The relays readers name (kinds 10050 and 10002), with when.
    inboxes: Mutex<Inboxes>,
    /// Readers whose relays are asked for, and who waits for them.
    asked: Mutex<VecDeque<(String, Sender<()>)>>,
    /// The newest message's time: the cursor.
    newest: AtomicU64,
    /// The schemes chats last wrote in.
    schemes: Mutex<BTreeMap<String, Scheme>>,
}

/// One relay's connection.
struct Relay {
    url: String,
    socket: Option<WebSocket>,
    failures: u32,
    retry_at: Instant,
    /// Events sent and not yet answered, by id.
    pending: BTreeMap<String, Event>,
    /// Lookups sent: subscription, reader.
    lookups: BTreeMap<String, String>,
}

/// The messages of `event` for `keys` from others, since `since`: a gift
/// wrap or an encrypted message, checked and decrypted, and its scheme.
fn message_of(keys: &Keys, event: &Event, since: u64) -> Option<(Incoming, Scheme, u64)> {
    if event.content.len() > MAX_CONTENT {
        return None;
    }
    let me = keys.public_hex();
    let for_me = event
        .tags
        .iter()
        .any(|t| t.len() >= 2 && t[0] == "p" && t[1] == me);
    if !for_me {
        return None;
    }
    let (sender, text, at, id, scheme) = match event.kind {
        4 => {
            if !verify(event) || event.pubkey == me || event.created_at < since {
                return None;
            }
            let sender = public_key_of(&event.pubkey).ok()?;
            let text = nip04_decrypt(keys, &sender, &event.content).ok()?;
            (
                sender,
                text,
                event.created_at,
                event.id.clone(),
                Scheme::Encrypted,
            )
        }
        1059 => {
            let (sender, rumor) = open_gift_wrap(keys, event).ok()?;
            let at = rumor["created_at"].as_u64()?;
            if sender == keys.public || at < since {
                return None;
            }
            // Only one person's messages: a group's are not answered.
            let others = rumor["tags"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|t| t[0] == "p" && t[1] != me.as_str())
                .count();
            if others > 0 {
                return None;
            }
            let text = match rumor["kind"].as_u64()? {
                14 => rumor["content"].as_str()?.to_string(),
                15 => "(a file, not fetched)".to_string(),
                _ => return None,
            };
            let id = rumor["id"].as_str().unwrap_or(&event.id).to_string();
            (sender, text, at, id, Scheme::Private)
        }
        _ => return None,
    };
    let text = text.trim().to_string();
    if text.is_empty() {
        return None;
    }
    let chat = npub(&sender);
    Some((
        Incoming {
            id,
            thread: String::new(),
            chat: chat.clone(),
            from: chat,
            text,
            voice: None,
            direct: true,
            mentions: Vec::new(),
            files: Vec::new(),
        },
        scheme,
        at,
    ))
}

/// The relay URLs of a relay list: kind 10050's `relay` tags, kind
/// 10002's `r` tags that are read (unmarked or marked `read`).
fn relays_of(event: &Event) -> Vec<String> {
    event
        .tags
        .iter()
        .filter(|t| match (event.kind, t.first().map(String::as_str)) {
            (10050, Some("relay")) => t.len() >= 2,
            (10002, Some("r")) => t.len() == 2 || t.get(2).map(String::as_str) == Some("read"),
            _ => false,
        })
        .map(|t| t[1].trim_end_matches('/').to_string())
        .filter(|url| url.starts_with("wss://"))
        .take(4)
        .collect()
}

fn send_json(socket: &mut WebSocket, message: &Value) -> Result<(), String> {
    socket
        .send_text(&message.to_string())
        .map_err(|e| e.to_string())
}

/// Keeps the relays: subscribed to the bridge's messages since `since`,
/// signing in where a relay asks (NIP-42), publishing what the outbox
/// holds and asking for readers' relays.
fn keep_relays(
    urls: Vec<String>,
    keys: Arc<Keys>,
    shared: Arc<Shared>,
    since: u64,
    client: sion_http::Client,
    link: Link,
) {
    let me = keys.public_hex();
    let filters = json!([
        {"kinds": [4], "#p": [me], "since": since},
        {"kinds": [1059], "#p": [me], "since": since.saturating_sub(TWO_DAYS)}
    ]);
    let subscribe = |socket: &mut WebSocket| {
        let mut request = vec![json!("REQ"), json!("sion-dms")];
        request.extend(filters.as_array().cloned().unwrap_or_default());
        send_json(socket, &Value::Array(request))
    };
    let mut relays: Vec<Relay> = urls
        .into_iter()
        .map(|url| Relay {
            url,
            socket: None,
            failures: 0,
            retry_at: Instant::now(),
            pending: BTreeMap::new(),
            lookups: BTreeMap::new(),
        })
        .collect();
    let mut seen: VecDeque<String> = VecDeque::new();
    let mut waiting: Waiting = BTreeMap::new();
    let mut lookups: BTreeMap<String, (String, Sender<()>, usize, Instant)> = BTreeMap::new();
    // Whether the account names relays for its private messages, asked
    // once a relay is there: if it names none, the bridge's are named
    // (kind 10050).
    let mut list_asked: Option<mpsc::Receiver<()>> = None;
    let mut list_done = false;
    let mut asked = 0u64;
    loop {
        if link.stopped() {
            for relay in relays.iter_mut() {
                if let Some(socket) = relay.socket.as_mut() {
                    socket.close(1000, "stopped");
                }
            }
            return;
        }
        // Connect the relays that are due.
        for relay in relays
            .iter_mut()
            .filter(|r| r.socket.is_none() && Instant::now() >= r.retry_at)
        {
            match WebSocket::connect(&client, &relay.url, &[], 1 << 20)
                .map_err(|e| e.to_string())
                .and_then(|mut socket| subscribe(&mut socket).map(|()| socket))
            {
                Ok(mut socket) => {
                    relay.failures = 0;
                    // What was not answered goes again.
                    for event in relay.pending.values() {
                        let _ = send_json(&mut socket, &json!(["EVENT", event]));
                    }
                    relay.socket = Some(socket);
                }
                Err(e) => {
                    relay.failures += 1;
                    let pause = Duration::from_secs(1u64 << relay.failures.min(6))
                        .min(Duration::from_secs(60));
                    println!(
                        "nostr: {}: {e}; connecting again in {} s",
                        relay.url,
                        pause.as_secs()
                    );
                    relay.retry_at = Instant::now() + pause;
                }
            }
        }
        let connected = relays.iter().filter(|r| r.socket.is_some()).count();
        // Where the bridge's private messages go: its relays (NIP-17), if
        // the account names none of its own.
        let named = || {
            shared
                .inboxes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&me)
                .is_some_and(|(_, private, _)| !private.is_empty())
        };
        if !list_done && connected > 0 {
            match &list_asked {
                None => {
                    let (answer, answered) = mpsc::channel();
                    shared
                        .asked
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push_back((me.clone(), answer));
                    list_asked = Some(answered);
                }
                Some(answered) if answered.try_recv().is_ok() => {
                    list_done = true;
                    if !named() {
                        let tags = relays
                            .iter()
                            .map(|r| vec!["relay".to_string(), r.url.clone()])
                            .collect();
                        if let Ok(list) = keys.sign(10050, tags, String::new(), now()) {
                            let (answer, _) = mpsc::channel();
                            shared
                                .outbox
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .push_back(Publish {
                                    event: list,
                                    answer,
                                });
                        }
                    }
                }
                Some(_) => {}
            }
        }
        let outbox: Vec<Publish> = shared
            .outbox
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
            .collect();
        for publish in outbox {
            let mut sent = 0;
            for relay in relays.iter_mut() {
                relay
                    .pending
                    .insert(publish.event.id.clone(), publish.event.clone());
                if let Some(socket) = relay.socket.as_mut() {
                    if send_json(socket, &json!(["EVENT", publish.event])).is_ok() {
                        sent += 1;
                    }
                }
            }
            // Answered by the relays it went to (or, sent later, by one).
            waiting.insert(
                publish.event.id.clone(),
                (publish.answer, sent.max(1), Vec::new()),
            );
        }
        let questions: Vec<(String, Sender<()>)> = shared
            .asked
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
            .collect();
        for (reader, answer) in questions {
            asked += 1;
            let sub = format!("sion-look-{asked}");
            let mut sent = 0;
            for relay in relays.iter_mut() {
                if let Some(socket) = relay.socket.as_mut() {
                    let filter = json!({"kinds": [10050, 10002], "authors": [reader], "limit": 4});
                    if send_json(socket, &json!(["REQ", sub, filter])).is_ok() {
                        relay.lookups.insert(sub.clone(), reader.clone());
                        sent += 1;
                    }
                }
            }
            lookups.insert(
                sub,
                (
                    reader,
                    answer,
                    sent,
                    Instant::now() + Duration::from_secs(4),
                ),
            );
        }
        lookups.retain(|_, (_, answer, left, until)| {
            let done = *left == 0 || Instant::now() >= *until;
            if done {
                let _ = answer.send(());
            }
            !done
        });
        if connected == 0 {
            link.rest(Duration::from_millis(200));
            continue;
        }
        let wait = Duration::from_millis((200 / connected as u64).max(20));
        for relay in relays.iter_mut() {
            let Some(socket) = relay.socket.as_mut() else {
                continue;
            };
            let message: Value = match socket.receive(wait) {
                Ok(Some(sion_http::ws::Message::Text(text))) => match serde_json::from_str(&text) {
                    Ok(message) => message,
                    Err(_) => continue,
                },
                Ok(_) => continue,
                Err(e) => {
                    println!("nostr: {}: {e}; connecting again", relay.url);
                    relay.socket = None;
                    relay.retry_at = Instant::now() + Duration::from_secs(1);
                    continue;
                }
            };
            match message[0].as_str() {
                Some("EVENT") => {
                    let Ok(event) = serde_json::from_value::<Event>(message[2].clone()) else {
                        continue;
                    };
                    if let Some(reader) = relay.lookups.get(message[1].as_str().unwrap_or_default())
                    {
                        if event.pubkey == *reader && verify(&event) {
                            let mut inboxes =
                                shared.inboxes.lock().unwrap_or_else(|e| e.into_inner());
                            let entry = inboxes
                                .entry(reader.clone())
                                .or_insert_with(|| (Instant::now(), Vec::new(), Vec::new()));
                            entry.0 = Instant::now();
                            match event.kind {
                                10050 => entry.1 = relays_of(&event),
                                10002 => entry.2 = relays_of(&event),
                                _ => {}
                            }
                        }
                        continue;
                    }
                    if seen.contains(&event.id) {
                        continue;
                    }
                    seen.push_back(event.id.clone());
                    if seen.len() > 4096 {
                        seen.pop_front();
                    }
                    let Some((incoming, scheme, at)) = message_of(&keys, &event, since) else {
                        continue;
                    };
                    // The same message, wrapped again or on another relay.
                    if seen.contains(&incoming.id) && incoming.id != event.id {
                        continue;
                    }
                    seen.push_back(incoming.id.clone());
                    shared
                        .schemes
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(incoming.chat.clone(), scheme);
                    shared.newest.fetch_max(at, Ordering::SeqCst);
                    if !link.pass(incoming) {
                        return;
                    }
                }
                Some("EOSE") => {
                    let sub = message[1].as_str().unwrap_or_default().to_string();
                    if relay.lookups.remove(&sub).is_some() {
                        let _ = send_json(socket, &json!(["CLOSE", sub]));
                        if let Some(lookup) = lookups.get_mut(&sub) {
                            lookup.2 = lookup.2.saturating_sub(1);
                        }
                    }
                }
                Some("OK") => {
                    let id = message[1].as_str().unwrap_or_default().to_string();
                    let accepted = message[2] == true;
                    let said = message[3].as_str().unwrap_or_default().to_string();
                    if said.starts_with("auth-required") {
                        // Sent again once signed in.
                        continue;
                    }
                    relay.pending.remove(&id);
                    if let Some((answer, left, refusals)) = waiting.get_mut(&id) {
                        if accepted || said.starts_with("duplicate") {
                            let _ = answer.send(Ok(()));
                            waiting.remove(&id);
                        } else {
                            refusals.push(format!("{}: {said}", relay.url));
                            *left = left.saturating_sub(1);
                            if *left == 0 {
                                let _ = answer.send(Err(format!(
                                    "no relay took it ({})",
                                    refusals.join("; ")
                                )));
                                waiting.remove(&id);
                            }
                        }
                    }
                }
                Some("AUTH") => {
                    let Some(challenge) = message[1].as_str() else {
                        continue;
                    };
                    let tags = vec![
                        vec!["relay".to_string(), relay.url.clone()],
                        vec!["challenge".to_string(), challenge.to_string()],
                    ];
                    if let Ok(auth) = keys.sign(22242, tags, String::new(), now()) {
                        let _ = send_json(socket, &json!(["AUTH", auth]));
                        // What the relay held back for it, asked again.
                        let _ = subscribe(socket);
                        for event in relay.pending.values() {
                            let _ = send_json(socket, &json!(["EVENT", event]));
                        }
                    }
                }
                Some("CLOSED") => {
                    let said = message[2].as_str().unwrap_or_default();
                    if !said.starts_with("auth-required") {
                        println!("nostr: {} closed the subscription: {said}", relay.url);
                    }
                }
                Some("NOTICE") => {
                    println!(
                        "nostr: {}: {}",
                        relay.url,
                        message[1].as_str().unwrap_or_default()
                    );
                }
                _ => {}
            }
        }
    }
}

/// Publishes `event` once to each of `urls` (relays a reader named), each
/// connection its own; whether one took it.
fn publish_to(client: &sion_http::Client, urls: &[String], event: &Event) -> bool {
    let mut took = false;
    for url in urls {
        let Ok(mut socket) = WebSocket::connect(client, url, &[], 1 << 16) else {
            continue;
        };
        if send_json(&mut socket, &json!(["EVENT", event])).is_err() {
            continue;
        }
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            match socket.receive(Duration::from_millis(500)) {
                Ok(Some(sion_http::ws::Message::Text(text))) => {
                    let message: Value = serde_json::from_str(&text).unwrap_or_default();
                    if message[0] == "OK" && message[1] == event.id.as_str() {
                        took |= message[2] == true;
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        socket.close(1000, "sent");
    }
    took
}

/// A Nostr account's direct messages.
pub struct Nostr {
    keys: Arc<Keys>,
    relays: Vec<String>,
    first: Scheme,
    shared: Arc<Shared>,
    client: sion_http::Client,
    /// For relays readers name: no private addresses.
    public: sion_http::Client,
    live: OnceLock<Live>,
}

impl std::fmt::Debug for Nostr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Nostr({})", npub(&self.keys.public))
    }
}

impl Nostr {
    /// The account whose key is in `NOSTR_PRIVATE_KEY`, on the relays in
    /// `NOSTR_RELAYS`.
    pub fn from_env() -> Result<Nostr, String> {
        let env = crate::webhooks::env;
        let keys = Keys::parse(
            &env("NOSTR_PRIVATE_KEY")
                .ok_or("set NOSTR_PRIVATE_KEY (the account's nsec1… or hex key)")?,
        )
        .map_err(|e| format!("NOSTR_PRIVATE_KEY: {e}"))?;
        let relays: Vec<String> = env("NOSTR_RELAYS")
            .unwrap_or_else(|| NOSTR_RELAYS.into())
            .split(',')
            .map(|r| r.trim().trim_end_matches('/').to_string())
            .filter(|r| !r.is_empty())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if relays.is_empty()
            || relays
                .iter()
                .any(|r| !r.starts_with("ws://") && !r.starts_with("wss://"))
        {
            return Err("NOSTR_RELAYS: give relays as wss://… addresses, comma-separated".into());
        }
        let first = match env("NOSTR_DM").as_deref() {
            None | Some("nip17") => Scheme::Private,
            Some("nip04") => Scheme::Encrypted,
            Some(other) => return Err(format!("NOSTR_DM is {other}: nip17 or nip04")),
        };
        Ok(Nostr {
            keys: Arc::new(keys),
            relays,
            first,
            shared: Arc::new(Shared::default()),
            client: crate::webhooks::client()?,
            public: sion_http::Client::new(sion_http::Options {
                timeout: Duration::from_secs(15),
                ..sion_http::Options::default()
            })
            .map_err(|e| e.to_string())?,
            live: OnceLock::new(),
        })
    }

    fn live(&self, cursor: &str) -> &Live {
        self.live.get_or_init(|| {
            // From the bridge's place, or from now.
            let since = cursor.parse::<u64>().map_or_else(|_| now(), |at| at + 1);
            self.shared
                .newest
                .fetch_max(since.saturating_sub(1), Ordering::SeqCst);
            let (urls, keys, shared, client) = (
                self.relays.clone(),
                self.keys.clone(),
                self.shared.clone(),
                self.client.clone(),
            );
            Live::start("nostr", move |link| {
                keep_relays(urls, keys, shared, since, client, link)
            })
        })
    }

    /// The relays `reader` names for private messages and for reading,
    /// asked of the bridge's relays and kept an hour.
    fn inboxes(&self, reader: &str) -> (Vec<String>, Vec<String>) {
        if let Some((at, private, read)) = self
            .shared
            .inboxes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(reader)
        {
            if at.elapsed() < INBOXES_KEPT {
                return (private.clone(), read.clone());
            }
        }
        let (answer, answered) = mpsc::channel();
        self.shared
            .asked
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back((reader.to_string(), answer));
        let _ = answered.recv_timeout(Duration::from_secs(6));
        self.shared
            .inboxes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(reader)
            .map(|(_, private, read)| (private.clone(), read.clone()))
            .unwrap_or_default()
    }

    /// Publishes `event` on the bridge's relays, and once on `extra`.
    fn publish(&self, event: Event, extra: &[String]) -> Result<(), String> {
        let extra: Vec<String> = extra
            .iter()
            .filter(|url| !self.relays.contains(url))
            .cloned()
            .collect();
        let (answer, answered) = mpsc::channel();
        self.shared
            .outbox
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(Publish {
                event: event.clone(),
                answer,
            });
        let elsewhere = !extra.is_empty() && publish_to(&self.public, &extra, &event);
        match answered.recv_timeout(PUBLISH_WAIT) {
            Ok(Ok(())) => Ok(()),
            _ if elsewhere => Ok(()),
            Ok(Err(e)) => Err(e),
            Err(RecvTimeoutError::Timeout) => Err("no relay took the message in 15 s".into()),
            Err(RecvTimeoutError::Disconnected) => Err("the relays' connection ended".into()),
        }
    }
}

impl Messenger for Nostr {
    fn name(&self) -> &'static str {
        "nostr"
    }

    fn chat_word(&self) -> &'static str {
        "npub"
    }

    fn receive(&self, cursor: &str, wait: u64) -> Result<(Vec<Incoming>, String), String> {
        let messages = self.live(cursor).receive(wait)?;
        let newest = self.shared.newest.load(Ordering::SeqCst);
        let cursor = match cursor.parse::<u64>() {
            Ok(at) if at >= newest => at.to_string(),
            _ => newest.to_string(),
        };
        Ok((messages, cursor))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        let recipient = public_key_of(chat)?;
        self.live("");
        let mut text: String = text.chars().take(NOSTR_TEXT).collect();
        if text.is_empty() {
            text = "(nothing)".into();
        }
        let scheme = self
            .shared
            .schemes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&npub(&recipient))
            .copied()
            .unwrap_or(self.first);
        let (private, read) = self.inboxes(&hex::encode(recipient));
        match scheme {
            Scheme::Private => {
                let wrap = gift_wrap(&self.keys, &recipient, &text, now())?;
                self.publish(wrap, &private)
            }
            Scheme::Encrypted => {
                let content = nip04_encrypt(&self.keys, &recipient, &text, &random()?)?;
                let event = self.keys.sign(
                    4,
                    vec![vec!["p".to_string(), hex::encode(recipient)]],
                    content,
                    now(),
                )?;
                self.publish(event, &read)
            }
        }
    }

    fn room(&self) -> usize {
        NOSTR_TEXT
    }

    fn long_polls(&self) -> bool {
        true
    }

    fn sees_strangers(&self) -> bool {
        true
    }

    fn once_wait(&self) -> u64 {
        LIVE_ONCE_WAIT
    }

    fn me(&self) -> Result<Vec<String>, String> {
        Ok(vec![npub(&self.keys.public), self.keys.public_hex()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h32(text: &str) -> [u8; 32] {
        hex::decode(text).unwrap().try_into().unwrap()
    }

    #[test]
    fn keys_read_and_write_as_nip19_has_them() {
        // NIP-19's own examples.
        let public = h32("3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d");
        assert_eq!(
            npub(&public),
            "npub180cvv07tjdrrgpa0j7j7tmnyl2yr6yr7l8j4s3evf6u64th6gkwsyjh6w6"
        );
        assert_eq!(
            public_key_of("npub10elfcs4fr0l0r8af98jlmgdh9c8tcxjvz9qkw038js35mp4dma8qzvjptg")
                .unwrap(),
            h32("7e7e9c42a91bfef19fa929e5fda1b72e0ebc1a4c1141673e2794234d86addf4e")
        );
        assert_eq!(
            key_of(
                "nsec1vl029mgpspedva04g90vltkh6fvh240zqtv9k0t9af8935ke9laqsnlfe5",
                "nsec"
            )
            .unwrap(),
            h32("67dea2ed018072d675f5415ecfaed7d2597555e202d85b3d65ea4e58d2d92ffa")
        );
        assert_eq!(public_key_of(&hex::encode(public)).unwrap(), public);
        assert!(
            public_key_of("npub180cvv07tjdrrgpa0j7j7tmnyl2yr6yr7l8j4s3evf6u64th6gkwsyjh6w7")
                .is_err()
        );
        assert!(key_of(
            "npub180cvv07tjdrrgpa0j7j7tmnyl2yr6yr7l8j4s3evf6u64th6gkwsyjh6w6",
            "nsec"
        )
        .is_err());
    }

    #[test]
    fn signatures_are_bip340s() {
        // BIP-340's first test vector: its message signed with its key.
        let keys = Keys::from_secret(&h32(
            "0000000000000000000000000000000000000000000000000000000000000003",
        ))
        .unwrap();
        assert_eq!(
            keys.public_hex(),
            "f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9"
        );
        let sig = keys.signing.sign_raw(&[0u8; 32], &[0u8; 32]).unwrap();
        assert_eq!(
            hex::encode(sig.to_bytes()),
            "e907831f80848d1069a5371b402410364bdf1c5f8307b0084c55f1ce2dca821525f66a4a85ea8b71e482a74f382d2ce5ebeee8fdb2172f477df4900d310536c0"
        );
        let event = keys
            .sign(
                1,
                vec![vec!["p".into(), "ab".into()]],
                "line\none \"quoted\"".into(),
                1_700_000_000,
            )
            .unwrap();
        assert!(verify(&event));
        let mut forged = event.clone();
        forged.content = "other".into();
        assert!(!verify(&forged));
        let mut renamed = event;
        renamed.pubkey = hex::encode([7u8; 32]);
        assert!(!verify(&renamed));
    }

    #[test]
    fn events_serialize_as_nip01_says() {
        let tags = vec![vec!["e".to_string(), "x".to_string()]];
        assert_eq!(
            serialized("ab", 1, 4, &tags, "a\"b\\c\nd\te/\u{1}é"),
            "[0,\"ab\",1,4,[[\"e\",\"x\"]],\"a\\\"b\\\\c\\nd\\te/\u{1}é\"]"
        );
        assert_eq!(serialized("ab", 1, 4, &[], ""), "[0,\"ab\",1,4,[],\"\"]");
    }

    #[test]
    fn nip44_is_its_test_vectors() {
        // From NIP-44's vectors (github.com/paulmillr/nip44).
        let keys = Keys::from_secret(&h32(
            "315e59ff51cb9209768cf7da80791ddcaae56ac9775eb25b6dee1234bc5d2268",
        ))
        .unwrap();
        assert_eq!(
            keys.conversation_key(&h32(
                "c2f9d9948dc8c7c38321e4b85c8558872eafa0641cd269db76848a6073e69133"
            ))
            .unwrap(),
            h32("3dfef0ce2a4d80a25e7a328accf73448ef67096f65f79588e358d9a0eb9013f1")
        );
        let (chacha_key, chacha_nonce, hmac_key) = message_keys(
            &h32("a1a3d60f3470a8612633924e91febf96dc5366ce130f658b1f0fc652c20b3b54"),
            &h32("e1e6f880560d6d149ed83dcc7e5861ee62a5ee051f7fde9975fe5d25d2a02d72"),
        );
        assert_eq!(
            hex::encode(chacha_key),
            "f145f3bed47cb70dbeaac07f3a3fe683e822b3715edb7c4fe310829014ce7d76"
        );
        assert_eq!(hex::encode(chacha_nonce), "c4ad129bb01180c0933a160c");
        assert_eq!(
            hex::encode(hmac_key),
            "027c1db445f05e2eee864a0975b0ddef5b7110583c8c192de3732571ca5838c4"
        );
        for (len, padded) in [
            (16, 32),
            (32, 32),
            (33, 64),
            (37, 64),
            (45, 64),
            (49, 64),
            (64, 64),
            (65, 96),
            (100, 128),
            (111, 128),
            (200, 224),
            (250, 256),
            (320, 320),
            (383, 384),
            (384, 384),
            (400, 448),
            (500, 512),
            (512, 512),
            (515, 640),
            (700, 768),
            (800, 896),
            (900, 1024),
            (1020, 1024),
            (65_536, 65_536),
        ] {
            assert_eq!(padded_len(len), padded, "{len}");
        }
        let one = Keys::from_secret(&h32(
            "0000000000000000000000000000000000000000000000000000000000000001",
        ))
        .unwrap();
        let two = Keys::from_secret(&h32(
            "0000000000000000000000000000000000000000000000000000000000000002",
        ))
        .unwrap();
        let key = one.conversation_key(&two.public).unwrap();
        assert_eq!(
            key,
            h32("c41c775356fd92eadc63ff5a0dc1da211b268cbea22316767095b2871ea1412d")
        );
        assert_eq!(key, two.conversation_key(&one.public).unwrap());
        let payload = "AgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABee0G5VSK0/9YypIObAtDKfYEAjD35uVkHyB0F4DwrcNaCXlCWZKaArsGrY6M9wnuTMxWfp1RTN9Xga8no+kF5Vsb";
        let nonce = h32("0000000000000000000000000000000000000000000000000000000000000001");
        assert_eq!(nip44_encrypt(&key, "a", &nonce).unwrap(), payload);
        assert_eq!(nip44_decrypt(&key, payload).unwrap(), "a");
        let mut tampered = payload.to_string();
        tampered.replace_range(50..51, if &payload[50..51] == "A" { "B" } else { "A" });
        assert!(nip44_decrypt(&key, &tampered).is_err());
        assert!(nip44_decrypt(&key, &format!("#{}", &payload[1..])).is_err());
        let long = "x".repeat(70_000);
        assert!(nip44_encrypt(&key, &long, &nonce).is_err());
    }

    #[test]
    fn nip04_messages_go_both_ways() {
        let ann = Keys::generate().unwrap();
        let bot = Keys::generate().unwrap();
        let sent = nip04_encrypt(&ann, &bot.public, "How much is left?", &[9; 16]).unwrap();
        assert!(sent.ends_with(&format!(
            "?iv={}",
            base64::engine::general_purpose::STANDARD.encode([9u8; 16])
        )));
        assert_eq!(
            nip04_decrypt(&bot, &ann.public, &sent).unwrap(),
            "How much is left?"
        );
        let other = Keys::generate().unwrap();
        assert_ne!(
            nip04_decrypt(&other, &ann.public, &sent).ok().as_deref(),
            Some("How much is left?")
        );
    }

    #[test]
    fn gift_wraps_hold_who_sealed_them() {
        let ann = Keys::generate().unwrap();
        let bot = Keys::generate().unwrap();
        let wrap = gift_wrap(&ann, &bot.public, "How much is left?", 1_800_000_000).unwrap();
        assert_eq!(wrap.kind, 1059);
        assert_ne!(
            wrap.pubkey,
            ann.public_hex(),
            "wrapped under a key of its own"
        );
        assert!(wrap.created_at <= 1_800_000_000 && wrap.created_at > 1_800_000_000 - TWO_DAYS - 1);
        let (sender, rumor) = open_gift_wrap(&bot, &wrap).unwrap();
        assert_eq!(sender, ann.public);
        assert_eq!(
            (rumor["kind"].as_u64(), rumor["content"].as_str()),
            (Some(14), Some("How much is left?"))
        );
        assert_eq!(rumor["created_at"], 1_800_000_000u64);
        // Only its reader opens it.
        assert!(open_gift_wrap(&ann, &wrap).is_err());
        // A seal naming another sender is refused.
        let mallory = Keys::generate().unwrap();
        let rumor = json!({"id": "00", "pubkey": ann.public_hex(), "created_at": 1, "kind": 14, "tags": [], "content": "Pay Mallory."});
        let seal = mallory
            .sign(
                13,
                Vec::new(),
                nip44_encrypt(
                    &mallory.conversation_key(&bot.public).unwrap(),
                    &rumor.to_string(),
                    &[1; 32],
                )
                .unwrap(),
                1,
            )
            .unwrap();
        let wrapper = Keys::generate().unwrap();
        let forged = wrapper
            .sign(
                1059,
                vec![vec!["p".into(), bot.public_hex()]],
                nip44_encrypt(
                    &wrapper.conversation_key(&bot.public).unwrap(),
                    &serde_json::to_string(&seal).unwrap(),
                    &[2; 32],
                )
                .unwrap(),
                1,
            )
            .unwrap();
        assert!(open_gift_wrap(&bot, &forged)
            .unwrap_err()
            .contains("another"));
        // A message for the bot: taken, as a private one, from its sender.
        let (incoming, scheme, at) = message_of(&bot, &wrap, 1_700_000_000).unwrap();
        assert_eq!(
            (incoming.chat, scheme, at),
            (npub(&ann.public), Scheme::Private, 1_800_000_000)
        );
        assert!(
            message_of(&bot, &wrap, 1_900_000_000).is_none(),
            "older than the bridge's place"
        );
    }

    #[test]
    fn relay_lists_name_where_to_write() {
        let keys = Keys::generate().unwrap();
        let inbox = keys
            .sign(
                10050,
                vec![
                    vec!["relay".into(), "wss://inbox.example/".into()],
                    vec!["relay".into(), "http://x".into()],
                ],
                String::new(),
                1,
            )
            .unwrap();
        assert_eq!(relays_of(&inbox), ["wss://inbox.example"]);
        let list = keys
            .sign(
                10002,
                vec![
                    vec!["r".into(), "wss://both.example".into()],
                    vec!["r".into(), "wss://write.example".into(), "write".into()],
                    vec!["r".into(), "wss://read.example".into(), "read".into()],
                ],
                String::new(),
                1,
            )
            .unwrap();
        assert_eq!(
            relays_of(&list),
            ["wss://both.example", "wss://read.example"]
        );
    }
}
