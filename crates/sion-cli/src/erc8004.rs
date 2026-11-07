//! ERC-8004, Trustless Agents: a Siontology agent as an agent in
//! Ethereum's registries (<https://eips.ethereum.org/EIPS/eip-8004>, and its
//! reference contracts at github.com/erc-8004/erc-8004-contracts).
//!
//! ERC-8004 has three registries on EVM chains. The Identity Registry is an
//! ERC-721 whose token URI names an agent's registration file: its name, its
//! services (an A2A card, an MCP server, a DID, …) and the trust it supports.
//! The Reputation Registry keeps feedback that clients give an agent, and the
//! Validation Registry the answers validators give to requests to check its
//! work. What this module makes is everything those take that is the agent's
//! to write:
//!
//! - **The registration file** ([`registration`]), which names the agent's
//!   Siontology DID among its services, so that whoever finds it on Ethereum
//!   finds its charter, its record and its bonds here.
//! - **The proof that a wallet is the agent's** ([`agent_wallet_digest`]),
//!   the EIP-712 signature `setAgentWallet` checks.
//! - **The calls** ([`Tx`]) that register it, bind its wallet, give feedback
//!   and answer a validation, encoded as the contracts take them, with the
//!   feedback's file and its digest. They are written out, never sent: a
//!   wallet of the owner's, or its RPC provider, sends them.
//!
//! Nothing here reads a chain, so nothing here knows what a registry holds:
//! a registration file says what its writer says, and its agent id is the
//! one it is given.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha3::{Digest, Keccak256};

use crate::error::CliError;

/// The registration file's type.
pub const REGISTRATION_TYPE: &str = "https://eips.ethereum.org/EIPS/eip-8004#registration-v1";

/// The Identity Registry on the mainnets it is deployed to.
pub const IDENTITY_MAINNET: &str = "0x8004A169FB4a3325136EB29fA0ceB6D2e539a432";

/// The Identity Registry on the testnets.
pub const IDENTITY_TESTNET: &str = "0x8004A818BFB912233c491871b3d84c89A494BD9e";

/// The Reputation Registry on the mainnets.
pub const REPUTATION_MAINNET: &str = "0x8004BAa17C55a88189AE136b182e5fdA19dE9b63";

/// The Reputation Registry on the testnets.
pub const REPUTATION_TESTNET: &str = "0x8004B663056A597Dffe9eCcC1965A193B7388713";

/// The testnets the registries are on at their testnet addresses: Sepolia,
/// Base Sepolia, Optimism Sepolia, Arbitrum Sepolia and Polygon Amoy.
pub const TESTNETS: [u64; 5] = [11_155_111, 84_532, 11_155_420, 421_614, 80_002];

/// The EIP-712 domain of the Identity Registry.
pub const IDENTITY_DOMAIN: (&str, &str) = ("ERC8004IdentityRegistry", "1");

/// The most seconds ahead a wallet's signature may be good for, as the
/// registry allows.
pub const MAX_DEADLINE: u64 = 5 * 60;

/// Keccak-256.
pub fn keccak256(bytes: &[u8]) -> [u8; 32] {
    Keccak256::digest(bytes).into()
}

/// An EVM address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Address(pub [u8; 20]);

impl Address {
    /// Reads `0x` and 40 hex digits; a mixed-case address must carry a
    /// right EIP-55 checksum.
    pub fn parse(text: &str) -> Result<Address, CliError> {
        let hex_part = text.trim().strip_prefix("0x").ok_or_else(|| {
            CliError::Usage(format!("{text:?}: an address is 0x and 40 hex digits"))
        })?;
        let bytes = hex::decode(hex_part)
            .ok()
            .and_then(|b| <[u8; 20]>::try_from(b).ok())
            .ok_or_else(|| {
                CliError::Usage(format!("{text:?}: an address is 0x and 40 hex digits"))
            })?;
        let address = Address(bytes);
        let mixed = hex_part.chars().any(|c| c.is_ascii_lowercase())
            && hex_part.chars().any(|c| c.is_ascii_uppercase());
        if mixed && address.to_string() != text.trim() {
            return Err(CliError::Usage(format!(
                "{text:?}: its EIP-55 checksum is wrong"
            )));
        }
        Ok(address)
    }

    /// The address of a public key.
    pub fn of(key: &k256::ecdsa::VerifyingKey) -> Address {
        let point = key.to_encoded_point(false);
        let hash = keccak256(&point.as_bytes()[1..]);
        let mut out = [0u8; 20];
        out.copy_from_slice(&hash[12..]);
        Address(out)
    }
}

impl std::fmt::Display for Address {
    /// The address with its EIP-55 checksum.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let lower = hex::encode(self.0);
        let hash = keccak256(lower.as_bytes());
        let mut out = String::with_capacity(42);
        out.push_str("0x");
        for (i, c) in lower.chars().enumerate() {
            let nibble = (hash[i / 2] >> if i % 2 == 0 { 4 } else { 0 }) & 0xf;
            out.push(if c.is_ascii_alphabetic() && nibble >= 8 {
                c.to_ascii_uppercase()
            } else {
                c
            });
        }
        f.write_str(&out)
    }
}

/// A key that signs for an EVM wallet.
pub struct WalletKey(k256::ecdsa::SigningKey);

impl std::fmt::Debug for WalletKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WalletKey({})", self.address())
    }
}

impl WalletKey {
    /// The key of 64 hex digits (with or without `0x`).
    pub fn parse(text: &str) -> Result<WalletKey, CliError> {
        let bytes = hex::decode(text.trim().trim_start_matches("0x"))
            .ok()
            .filter(|b| b.len() == 32)
            .ok_or_else(|| CliError::Usage("a wallet's key is 64 hex digits".into()))?;
        k256::ecdsa::SigningKey::from_slice(&bytes)
            .map(WalletKey)
            .map_err(|_| CliError::Usage("not a secp256k1 key".into()))
    }

    /// Its wallet's address.
    pub fn address(&self) -> Address {
        Address::of(self.0.verifying_key())
    }

    /// Signs `digest`: `r`, `s` (the lower of the two) and `v` (27 or 28),
    /// as Ethereum's `ecrecover` takes them.
    pub fn sign(&self, digest: &[u8; 32]) -> Result<[u8; 65], CliError> {
        let (signature, recovery) = self
            .0
            .sign_prehash_recoverable(digest)
            .map_err(|e| CliError::Usage(format!("the digest could not be signed: {e}")))?;
        let mut out = [0u8; 65];
        out[..64].copy_from_slice(&signature.to_bytes());
        out[64] = 27 + recovery.to_byte();
        Ok(out)
    }
}

/// The wallet that signed `digest` with `signature` (65 bytes, `v` 27 or
/// 28).
pub fn recover(digest: &[u8; 32], signature: &[u8]) -> Result<Address, CliError> {
    let bad = || CliError::Usage("not a recoverable signature".into());
    if signature.len() != 65 {
        return Err(bad());
    }
    let sig = k256::ecdsa::Signature::from_slice(&signature[..64]).map_err(|_| bad())?;
    let v = signature[64];
    let recovery =
        k256::ecdsa::RecoveryId::from_byte(v.checked_sub(27).unwrap_or(v)).ok_or_else(bad)?;
    let key = k256::ecdsa::VerifyingKey::recover_from_prehash(digest, &sig, recovery)
        .map_err(|_| bad())?;
    Ok(Address::of(&key))
}

/// A value as a contract takes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    /// An unsigned integer (`uint256`, `uint64`, `uint8`, …).
    Uint(u128),
    /// A signed integer (`int128`).
    Int(i128),
    /// An address.
    Address(Address),
    /// 32 bytes.
    Bytes32([u8; 32]),
    /// A string.
    String(String),
    /// Bytes.
    Bytes(Vec<u8>),
}

fn word_of_uint(n: u128) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[16..].copy_from_slice(&n.to_be_bytes());
    word
}

fn tail_of(bytes: &[u8]) -> Vec<u8> {
    let mut out = word_of_uint(bytes.len() as u128).to_vec();
    out.extend_from_slice(bytes);
    out.resize(32 + bytes.len().div_ceil(32) * 32, 0);
    out
}

/// `tokens` in the contract ABI's encoding: a head of words, the dynamic
/// values after it.
pub fn encode(tokens: &[Token]) -> Vec<u8> {
    let mut head = Vec::new();
    let mut tail = Vec::new();
    let head_len = 32 * tokens.len();
    for token in tokens {
        match token {
            Token::Uint(n) => head.extend_from_slice(&word_of_uint(*n)),
            Token::Int(n) => {
                let fill = if *n < 0 { 0xff } else { 0 };
                let mut word = [fill; 32];
                word[16..].copy_from_slice(&n.to_be_bytes());
                head.extend_from_slice(&word);
            }
            Token::Address(a) => {
                let mut word = [0u8; 32];
                word[12..].copy_from_slice(&a.0);
                head.extend_from_slice(&word);
            }
            Token::Bytes32(b) => head.extend_from_slice(b),
            Token::String(s) => {
                head.extend_from_slice(&word_of_uint((head_len + tail.len()) as u128));
                tail.extend(tail_of(s.as_bytes()));
            }
            Token::Bytes(b) => {
                head.extend_from_slice(&word_of_uint((head_len + tail.len()) as u128));
                tail.extend(tail_of(b));
            }
        }
    }
    head.extend(tail);
    head
}

/// A call of the function `signature` (`register(string)`) with `tokens`:
/// its selector and its arguments.
pub fn call(signature: &str, tokens: &[Token]) -> Vec<u8> {
    let mut data = keccak256(signature.as_bytes())[..4].to_vec();
    data.extend(encode(tokens));
    data
}

/// The EIP-712 domain separator of a contract named `name`, `version`, on
/// `chain`, at `contract`.
pub fn domain_separator(name: &str, version: &str, chain: u64, contract: &Address) -> [u8; 32] {
    let type_hash = keccak256(
        b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
    );
    keccak256(&encode(&[
        Token::Bytes32(type_hash),
        Token::Bytes32(keccak256(name.as_bytes())),
        Token::Bytes32(keccak256(version.as_bytes())),
        Token::Uint(u128::from(chain)),
        Token::Address(*contract),
    ]))
}

/// What `newWallet` signs for the Identity Registry at `registry` on
/// `chain` to become agent `agent`'s wallet, as `owner` (the agent's owner)
/// asks before `deadline` (seconds since the epoch): the EIP-712 digest of
/// `AgentWalletSet`.
pub fn agent_wallet_digest(
    chain: u64,
    registry: &Address,
    agent: u128,
    new_wallet: &Address,
    owner: &Address,
    deadline: u64,
) -> [u8; 32] {
    let type_hash = keccak256(
        b"AgentWalletSet(uint256 agentId,address newWallet,address owner,uint256 deadline)",
    );
    let structure = keccak256(&encode(&[
        Token::Bytes32(type_hash),
        Token::Uint(agent),
        Token::Address(*new_wallet),
        Token::Address(*owner),
        Token::Uint(u128::from(deadline)),
    ]));
    let mut message = vec![0x19, 0x01];
    message.extend_from_slice(&domain_separator(
        IDENTITY_DOMAIN.0,
        IDENTITY_DOMAIN.1,
        chain,
        registry,
    ));
    message.extend_from_slice(&structure);
    keccak256(&message)
}

/// The Identity Registry on `chain`: the testnet address on a testnet, the
/// mainnet one elsewhere, unless `given`.
pub fn identity_registry(chain: u64, given: Option<&str>) -> Result<Address, CliError> {
    match given {
        Some(given) => Address::parse(given),
        None if TESTNETS.contains(&chain) => Address::parse(IDENTITY_TESTNET),
        None => Address::parse(IDENTITY_MAINNET),
    }
}

/// The Reputation Registry on `chain`, as [`identity_registry`].
pub fn reputation_registry(chain: u64, given: Option<&str>) -> Result<Address, CliError> {
    match given {
        Some(given) => Address::parse(given),
        None if TESTNETS.contains(&chain) => Address::parse(REPUTATION_TESTNET),
        None => Address::parse(REPUTATION_MAINNET),
    }
}

/// A transaction to send, as a wallet takes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tx {
    /// The chain.
    #[serde(rename = "chainId")]
    pub chain: u64,
    /// The contract.
    pub to: String,
    /// The call, `0x` and hex.
    pub data: String,
    /// What it does, in words.
    pub what: String,
}

impl Tx {
    fn new(chain: u64, to: &Address, data: &[u8], what: String) -> Tx {
        Tx {
            chain,
            to: to.to_string(),
            data: format!("0x{}", hex::encode(data)),
            what,
        }
    }
}

/// `register(agentURI)` on `registry`: a new agent, its registration file
/// at `uri`.
pub fn register_tx(chain: u64, registry: &Address, uri: &str) -> Tx {
    Tx::new(
        chain,
        registry,
        &call("register(string)", &[Token::String(uri.to_string())]),
        format!(
            "registers an agent whose registration file is at {}",
            clip(uri)
        ),
    )
}

/// `setAgentWallet` on `registry`: `wallet` becomes agent `agent`'s, with
/// its signature over [`agent_wallet_digest`]. Sent by the agent's owner.
pub fn agent_wallet_tx(
    chain: u64,
    registry: &Address,
    agent: u128,
    wallet: &Address,
    deadline: u64,
    signature: &[u8],
) -> Tx {
    Tx::new(
        chain,
        registry,
        &call(
            "setAgentWallet(uint256,address,uint256,bytes)",
            &[
                Token::Uint(agent),
                Token::Address(*wallet),
                Token::Uint(u128::from(deadline)),
                Token::Bytes(signature.to_vec()),
            ],
        ),
        format!("makes {wallet} agent {agent}'s wallet"),
    )
}

/// Feedback a client gives an agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Feedback {
    /// The agent.
    pub agent: u128,
    /// The value, with `decimals` of it after the point.
    pub value: i128,
    /// How many of its digits are after the point (0 to 18).
    pub decimals: u8,
    /// Tags that say what it is about.
    pub tags: (String, String),
    /// The endpoint it is about.
    pub endpoint: String,
}

/// The feedback's own file ([the spec's off-chain feedback file]), which
/// `giveFeedback` names by its URI and Keccak digest: as `client` on
/// `chain` gives it at `at` (RFC 3339), with `extra` fields beside.
pub fn feedback_file(
    feedback: &Feedback,
    chain: u64,
    registry: &Address,
    client: &Address,
    at: &str,
    extra: &serde_json::Map<String, Value>,
) -> Value {
    let mut file = json!({
        "agentRegistry": format!("eip155:{chain}:{registry}"),
        "agentId": feedback.agent,
        "clientAddress": format!("eip155:{chain}:{client}"),
        "createdAt": at,
        "value": feedback.value,
        "valueDecimals": feedback.decimals,
    });
    let object = file.as_object_mut().expect("an object");
    for (key, value) in [
        ("tag1", &feedback.tags.0),
        ("tag2", &feedback.tags.1),
        ("endpoint", &feedback.endpoint),
    ] {
        if !value.is_empty() {
            object.insert(key.into(), json!(value));
        }
    }
    for (key, value) in extra {
        object.entry(key.clone()).or_insert_with(|| value.clone());
    }
    file
}

/// `giveFeedback` on the Reputation Registry `registry`, the feedback's
/// file at `uri` with digest `hash`. Sent by the client; never the agent's
/// own owner, whom the registry refuses.
pub fn feedback_tx(
    chain: u64,
    registry: &Address,
    feedback: &Feedback,
    uri: &str,
    hash: [u8; 32],
) -> Result<Tx, CliError> {
    if feedback.decimals > 18 {
        return Err(CliError::Usage("feedback has 0 to 18 decimals".into()));
    }
    Ok(Tx::new(
        chain,
        registry,
        &call(
            "giveFeedback(uint256,int128,uint8,string,string,string,string,bytes32)",
            &[
                Token::Uint(feedback.agent),
                Token::Int(feedback.value),
                Token::Uint(u128::from(feedback.decimals)),
                Token::String(feedback.tags.0.clone()),
                Token::String(feedback.tags.1.clone()),
                Token::String(feedback.endpoint.clone()),
                Token::String(uri.to_string()),
                Token::Bytes32(hash),
            ],
        ),
        format!(
            "gives agent {} feedback of {}",
            feedback.agent,
            decimal(feedback.value, feedback.decimals)
        ),
    ))
}

/// `validationResponse` on the Validation Registry `registry`: the answer
/// (0 to 100) to the request of digest `request`, what was found at `uri`
/// with digest `hash`. Sent by the validator the request named.
pub fn validation_tx(
    chain: u64,
    registry: &Address,
    request: [u8; 32],
    response: u8,
    uri: &str,
    hash: [u8; 32],
    tag: &str,
) -> Result<Tx, CliError> {
    if response > 100 {
        return Err(CliError::Usage("a validation's answer is 0 to 100".into()));
    }
    Ok(Tx::new(
        chain,
        registry,
        &call(
            "validationResponse(bytes32,uint8,string,bytes32,string)",
            &[
                Token::Bytes32(request),
                Token::Uint(u128::from(response)),
                Token::String(uri.to_string()),
                Token::Bytes32(hash),
                Token::String(tag.to_string()),
            ],
        ),
        format!(
            "answers validation request 0x{}… with {response}",
            &hex::encode(request)[..12]
        ),
    ))
}

fn decimal(value: i128, decimals: u8) -> String {
    if decimals == 0 {
        return value.to_string();
    }
    let scale = 10i128.pow(u32::from(decimals));
    let sign = if value < 0 { "-" } else { "" };
    format!(
        "{sign}{}.{:0width$}",
        (value / scale).abs(),
        (value % scale).abs(),
        width = usize::from(decimals)
    )
}

fn clip(text: &str) -> String {
    if text.len() <= 64 {
        text.to_string()
    } else {
        let mut end = 60;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &text[..end])
    }
}

/// A service a registration file names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Service {
    /// What it is: `A2A`, `MCP`, `web`, `DID`, `email`, …
    pub name: String,
    /// Where.
    pub endpoint: String,
    /// The version of its protocol, if it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// An agent's entry in a registry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registered {
    /// Its id there.
    #[serde(rename = "agentId")]
    pub agent: u128,
    /// The registry: `eip155:<chain>:<address>`.
    #[serde(rename = "agentRegistry")]
    pub registry: String,
}

/// An agent's registration file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registration {
    /// [`REGISTRATION_TYPE`].
    #[serde(rename = "type")]
    pub kind: String,
    /// Its name.
    pub name: String,
    /// What it does, in words.
    pub description: String,
    /// A picture of it.
    #[serde(default)]
    pub image: String,
    /// Where it is reached.
    pub services: Vec<Service>,
    /// Whether it takes x402 payments.
    #[serde(rename = "x402Support", default)]
    pub x402: bool,
    /// Whether it is at work.
    #[serde(default = "yes")]
    pub active: bool,
    /// Its entries in registries.
    #[serde(default)]
    pub registrations: Vec<Registered>,
    /// The trust it supports.
    #[serde(rename = "supportedTrust", default)]
    pub trust: Vec<String>,
}

fn yes() -> bool {
    true
}

/// What a registration file is made from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Card {
    /// The agent's name.
    pub name: String,
    /// What it does.
    pub description: String,
    /// Its Siontology DID.
    pub did: String,
    /// A picture of it.
    pub image: String,
    /// Its A2A card's address.
    pub a2a: Option<String>,
    /// Its MCP server's address.
    pub mcp: Option<String>,
    /// Its web page.
    pub web: Option<String>,
    /// Whether it takes x402 payments.
    pub x402: bool,
    /// Its entry in a registry: its id, the chain and the registry.
    pub registered: Option<(u128, u64, Address)>,
}

/// The registration file of the agent `card` describes. Its trust is the
/// Siontology record its DID leads to: what it was paid for and delivered
/// (`reputation`), and the bonds that stand behind its claims, which an
/// adjudicator can rule against (`crypto-economic`).
pub fn registration(card: &Card) -> Registration {
    let mut services = Vec::new();
    if let Some(web) = &card.web {
        services.push(Service {
            name: "web".into(),
            endpoint: web.clone(),
            version: None,
        });
    }
    if let Some(a2a) = &card.a2a {
        services.push(Service {
            name: "A2A".into(),
            endpoint: a2a.clone(),
            version: Some("0.3.0".into()),
        });
    }
    if let Some(mcp) = &card.mcp {
        services.push(Service {
            name: "MCP".into(),
            endpoint: mcp.clone(),
            version: Some("2025-06-18".into()),
        });
    }
    services.push(Service {
        name: "DID".into(),
        endpoint: card.did.clone(),
        version: Some("v1".into()),
    });
    Registration {
        kind: REGISTRATION_TYPE.into(),
        name: card.name.clone(),
        description: card.description.clone(),
        image: card.image.clone(),
        services,
        x402: card.x402,
        active: true,
        registrations: card
            .registered
            .iter()
            .map(|(agent, chain, registry)| Registered {
                agent: *agent,
                registry: format!("eip155:{chain}:{registry}"),
            })
            .collect(),
        trust: vec!["reputation".into(), "crypto-economic".into()],
    }
}

/// `file` as a `data:` URI, for an agent URI kept wholly on chain.
pub fn data_uri(file: &[u8]) -> String {
    use base64::Engine;
    format!(
        "data:application/json;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(file)
    )
}

/// Reads a registration file and checks what the spec requires of it:
/// its type, a name, and registrations of the form
/// `eip155:<chain>:<address>`.
pub fn read_registration(bytes: &[u8]) -> Result<Registration, CliError> {
    let file: Registration = serde_json::from_slice(bytes)
        .map_err(|e| CliError::Usage(format!("not a registration file: {e}")))?;
    if file.kind != REGISTRATION_TYPE {
        return Err(CliError::Usage(format!(
            "its type is {:?}, not {REGISTRATION_TYPE}",
            file.kind
        )));
    }
    if file.name.trim().is_empty() {
        return Err(CliError::Usage("it has no name".into()));
    }
    for entry in &file.registrations {
        let parts: Vec<&str> = entry.registry.split(':').collect();
        let well_formed = parts.len() == 3
            && parts[0] == "eip155"
            && parts[1].parse::<u64>().is_ok()
            && Address::parse(parts[2]).is_ok();
        if !well_formed {
            return Err(CliError::Usage(format!(
                "registry {:?} is not eip155:<chain>:<address>",
                entry.registry
            )));
        }
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Checked against eth-account 0.13.4 and eth-abi (Python), which
    // computed the same addresses, digests, signatures and calls.
    const KEY: &str = "4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318";

    #[test]
    fn an_address_has_its_checksum_and_a_key_its_address() {
        let key = WalletKey::parse(KEY).unwrap();
        assert_eq!(
            key.address().to_string(),
            "0x2c7536E3605D9C16a7a3D7b1898e529396a65c23"
        );
        assert!(Address::parse("0x2c7536e3605d9c16a7a3d7b1898e529396a65c23").is_ok());
        assert!(Address::parse("0x2C7536e3605d9c16a7a3d7b1898e529396a65c23").is_err());
        assert_eq!(
            Address::parse(IDENTITY_MAINNET).unwrap().to_string(),
            IDENTITY_MAINNET
        );
    }

    #[test]
    fn a_signature_recovers_to_the_wallet_that_made_it() {
        let key = WalletKey::parse(KEY).unwrap();
        let digest = keccak256(b"anything");
        let signature = key.sign(&digest).unwrap();
        assert!(signature[64] == 27 || signature[64] == 28);
        assert_eq!(recover(&digest, &signature).unwrap(), key.address());
        let mut other = signature;
        other[10] ^= 1;
        assert_ne!(recover(&digest, &other).ok(), Some(key.address()));
    }

    #[test]
    fn calls_are_encoded_as_the_contracts_take_them() {
        assert_eq!(
            hex::encode(&call("register(string)", &[])[..4]),
            hex::encode(&keccak256(b"register(string)")[..4])
        );
        let data = encode(&[Token::Uint(22), Token::Int(-5), Token::String("hi".into())]);
        assert_eq!(data.len(), 32 * 5);
        assert_eq!(data[31], 22);
        assert!(data[32..63].iter().all(|b| *b == 0xff) && data[63] == 0xfb);
        assert_eq!(data[95], 96, "the string starts after the head");
        assert_eq!(data[127], 2);
        assert_eq!(&data[128..130], b"hi");
        assert_eq!(decimal(9977, 2), "99.77");
        assert_eq!(decimal(-32, 1), "-3.2");
    }

    #[test]
    fn digests_signatures_and_calls_match_an_independent_implementation() {
        let key = WalletKey::parse(KEY).unwrap();
        let registry = Address::parse(IDENTITY_TESTNET).unwrap();
        let owner = Address::parse("0x1111111111111111111111111111111111111111").unwrap();
        let digest = agent_wallet_digest(
            11_155_111,
            &registry,
            22,
            &key.address(),
            &owner,
            1_800_000_000,
        );
        assert_eq!(
            hex::encode(digest),
            "2340158cfc0c9d159731db1857f749881bd4ae8ff61cc727ed8e3e9baaf1e657"
        );
        let signature = key.sign(&digest).unwrap();
        assert_eq!(
            hex::encode(signature),
            "8a03d3502129b828bc146cc793ab9d8aa1bf6471d1910620c1156d0a2e655b894e8ef8047badb3d6357a8f91f2472fdeb8df7156824ad7c1662853572a9bfc741b"
        );
        let tx = register_tx(11_155_111, &registry, "ipfs://bafyexample");
        assert_eq!(tx.data, "0xf2c298be00000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000012697066733a2f2f626166796578616d706c650000000000000000000000000000");
        let feedback = Feedback {
            agent: 22,
            value: -9977,
            decimals: 2,
            tags: ("starred".into(), String::new()),
            endpoint: "https://agent.example/mcp".into(),
        };
        let tx = feedback_tx(1, &registry, &feedback, "ipfs://feedback", [0x11; 32]).unwrap();
        assert_eq!(tx.data, "0x3c036a7e0000000000000000000000000000000000000000000000000000000000000016ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffd907000000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000000140000000000000000000000000000000000000000000000000000000000000016000000000000000000000000000000000000000000000000000000000000001a01111111111111111111111111111111111111111111111111111111111111111000000000000000000000000000000000000000000000000000000000000000773746172726564000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001968747470733a2f2f6167656e742e6578616d706c652f6d637000000000000000000000000000000000000000000000000000000000000000000000000000000f697066733a2f2f666565646261636b0000000000000000000000000000000000");
        let tx = validation_tx(
            1,
            &registry,
            [0x22; 32],
            100,
            "https://example.org/found.json",
            [0x33; 32],
            "siontology-release",
        )
        .unwrap();
        assert_eq!(tx.data, "0x3d659a962222222222222222222222222222222222222222222222222222222222222222000000000000000000000000000000000000000000000000000000000000006400000000000000000000000000000000000000000000000000000000000000a0333333333333333333333333333333333333333333333333333333333333333300000000000000000000000000000000000000000000000000000000000000e0000000000000000000000000000000000000000000000000000000000000001e68747470733a2f2f6578616d706c652e6f72672f666f756e642e6a736f6e0000000000000000000000000000000000000000000000000000000000000000001273696f6e746f6c6f67792d72656c656173650000000000000000000000000000");
        let tx = agent_wallet_tx(
            11_155_111,
            &registry,
            22,
            &key.address(),
            1_800_000_000,
            &signature,
        );
        assert_eq!(tx.data, "0x2d1ef5ae00000000000000000000000000000000000000000000000000000000000000160000000000000000000000002c7536e3605d9c16a7a3d7b1898e529396a65c23000000000000000000000000000000000000000000000000000000006b49d200000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000000418a03d3502129b828bc146cc793ab9d8aa1bf6471d1910620c1156d0a2e655b894e8ef8047badb3d6357a8f91f2472fdeb8df7156824ad7c1662853572a9bfc741b00000000000000000000000000000000000000000000000000000000000000");
    }

    #[test]
    fn a_registration_file_names_the_agents_did_and_reads_back() {
        let file = registration(&Card {
            name: "lead".into(),
            description: "Pays Acme's invoices.".into(),
            did: "did:sion:devnet:0123".into(),
            a2a: Some("https://agent.example/.well-known/agent-card.json".into()),
            x402: true,
            registered: Some((22, 1, Address::parse(IDENTITY_MAINNET).unwrap())),
            ..Card::default()
        });
        let bytes = serde_json::to_vec_pretty(&file).unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["type"], REGISTRATION_TYPE);
        assert_eq!(value["x402Support"], true);
        assert_eq!(
            value["registrations"][0]["agentRegistry"],
            format!("eip155:1:{IDENTITY_MAINNET}")
        );
        assert_eq!(value["services"][1]["name"], "DID");
        assert_eq!(read_registration(&bytes).unwrap(), file);
        assert!(data_uri(&bytes).starts_with("data:application/json;base64,"));
        let mut wrong = value.clone();
        wrong["registrations"][0]["agentRegistry"] = json!("ethereum:0x1");
        assert!(read_registration(wrong.to_string().as_bytes()).is_err());
    }
}

/// `sion agent erc8004`: the agent in ERC-8004's registries. Every call is
/// written out for a wallet to send, never sent from here.
#[derive(clap::Subcommand, Debug)]
pub enum Erc8004Command {
    /// Write the agent's registration file: its name, what it does, its
    /// services (an A2A card, an MCP server, a web page) and its Siontology
    /// DID, and its entry in a registry once it has one. With --data-uri,
    /// the file as a data: URI too, to register wholly on chain.
    Card {
        /// The profile.
        profile: String,
        /// What the agent does, in a sentence or two (public).
        #[arg(long)]
        description: Option<String>,
        /// Its A2A card's address (…/.well-known/agent-card.json).
        #[arg(long, value_name = "URL")]
        a2a: Option<String>,
        /// Its MCP server's address.
        #[arg(long, value_name = "URL")]
        mcp: Option<String>,
        /// Its web page.
        #[arg(long, value_name = "URL")]
        web: Option<String>,
        /// A picture of it.
        #[arg(long, value_name = "URL", default_value = "")]
        image: String,
        /// Say that it takes x402 payments.
        #[arg(long)]
        x402: bool,
        /// Its id in the Identity Registry, once registered.
        #[arg(long = "agent-id")]
        agent_id: Option<u128>,
        /// The chain (EIP-155 id).
        #[arg(long, default_value_t = 1)]
        chain: u64,
        /// The Identity Registry (default: the one deployed on that chain).
        #[arg(long)]
        registry: Option<String>,
        /// Where to write it (default: standard output).
        #[arg(long, value_name = "FILE")]
        out: Option<std::path::PathBuf>,
        /// Also print it as a data: URI.
        #[arg(long = "data-uri")]
        data_uri: bool,
    },
    /// The call that registers an agent in the Identity Registry, its
    /// registration file at a URI (https://, ipfs:// or data:), for its
    /// owner to send.
    Register {
        /// The registration file's URI.
        #[arg(long)]
        uri: String,
        /// The chain (EIP-155 id).
        #[arg(long, default_value_t = 1)]
        chain: u64,
        /// The Identity Registry (default: the one deployed on that chain).
        #[arg(long)]
        registry: Option<String>,
    },
    /// Sign, as the wallet whose key is in EVM_PRIVATE_KEY, that it is an
    /// agent's wallet (EIP-712, as setAgentWallet checks), and write the
    /// call the agent's owner sends.
    Wallet {
        /// The agent's id in the Identity Registry.
        agent_id: u128,
        /// The agent's owner, who sends the call.
        #[arg(long)]
        owner: String,
        /// Seconds the signature is good for (at most 300).
        #[arg(long, default_value_t = MAX_DEADLINE)]
        within: u64,
        /// The chain (EIP-155 id).
        #[arg(long, default_value_t = 1)]
        chain: u64,
        /// The Identity Registry (default: the one deployed on that chain).
        #[arg(long)]
        registry: Option<String>,
    },
    /// Feedback for an agent, as a client gives it: its file, to put at a
    /// URI, and the call (giveFeedback) the client sends, naming the file
    /// by that URI and its Keccak digest.
    Feedback {
        /// The agent's id in the Identity Registry.
        agent_id: u128,
        /// The value, a decimal (99.77, -3.2, 1).
        #[arg(long, allow_hyphen_values = true)]
        value: String,
        /// What it is about (`starred`, `uptime`, …).
        #[arg(long, default_value = "")]
        tag1: String,
        /// More of what it is about.
        #[arg(long, default_value = "")]
        tag2: String,
        /// The endpoint it is about.
        #[arg(long, default_value = "")]
        endpoint: String,
        /// The client's address.
        #[arg(long)]
        client: String,
        /// Where the file will be (default: none, and no digest).
        #[arg(long)]
        uri: Option<String>,
        /// The chain (EIP-155 id).
        #[arg(long, default_value_t = 1)]
        chain: u64,
        /// The Identity Registry the agent is in (default: the one
        /// deployed on that chain).
        #[arg(long)]
        registry: Option<String>,
        /// The Reputation Registry (default: the one deployed there).
        #[arg(long)]
        reputation: Option<String>,
        /// Where to write the file.
        #[arg(long, value_name = "FILE")]
        out: Option<std::path::PathBuf>,
    },
    /// A validator's answer to a validation request: the call
    /// (validationResponse) the validator sends, naming what it found by
    /// its URI and Keccak digest.
    Validate {
        /// The request's digest (0x and 64 hex digits).
        #[arg(long)]
        request: String,
        /// The answer, 0 (failed) to 100 (passed).
        #[arg(long)]
        response: u8,
        /// Where what was found is.
        #[arg(long, default_value = "")]
        uri: String,
        /// The file of what was found, whose digest the call names.
        #[arg(long, value_name = "FILE")]
        found: Option<std::path::PathBuf>,
        /// A tag for the answer.
        #[arg(long, default_value = "")]
        tag: String,
        /// The chain (EIP-155 id).
        #[arg(long, default_value_t = 1)]
        chain: u64,
        /// The Validation Registry (no address is listed as deployed yet).
        #[arg(long)]
        registry: String,
    },
    /// Check a registration file: its type, its name and its entries.
    Check {
        /// The file.
        file: std::path::PathBuf,
    },
}

impl Erc8004Command {
    /// Whether it needs a node: the card names the agent's DID.
    pub fn needs_ledger(&self) -> bool {
        matches!(self, Erc8004Command::Card { .. })
    }
}

fn print_tx(tx: &Tx) -> Result<(), CliError> {
    println!(
        "{}",
        serde_json::to_string_pretty(tx).map_err(|e| CliError::Usage(e.to_string()))?
    );
    Ok(())
}

fn digest_arg(text: &str) -> Result<[u8; 32], CliError> {
    hex::decode(text.trim().trim_start_matches("0x"))
        .ok()
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
        .ok_or_else(|| CliError::Usage(format!("{text:?} is not 0x and 64 hex digits")))
}

/// `value` as an integer and the digits after its point.
pub fn parse_decimal(value: &str) -> Result<(i128, u8), CliError> {
    let bad = || CliError::Usage(format!("{value:?} is not a decimal"));
    let text = value.trim();
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if fraction.len() > 18 || !fraction.chars().all(|c| c.is_ascii_digit()) {
        return Err(bad());
    }
    let digits = format!("{whole}{fraction}");
    let number: i128 = digits.parse().map_err(|_| bad())?;
    Ok((number, fraction.len() as u8))
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Runs an ERC-8004 command that needs no node; gives back one that does.
pub fn local(command: Erc8004Command) -> Result<Result<(), CliError>, Box<Erc8004Command>> {
    if command.needs_ledger() {
        return Err(Box::new(command));
    }
    Ok(run_local(command))
}

fn run_local(command: Erc8004Command) -> Result<(), CliError> {
    match command {
        Erc8004Command::Register {
            uri,
            chain,
            registry,
        } => print_tx(&register_tx(
            chain,
            &identity_registry(chain, registry.as_deref())?,
            &uri,
        )),
        Erc8004Command::Wallet {
            agent_id,
            owner,
            within,
            chain,
            registry,
        } => {
            if within == 0 || within > MAX_DEADLINE {
                return Err(CliError::Usage(format!(
                    "the registry takes a signature good for 1 to {MAX_DEADLINE} seconds"
                )));
            }
            let key = crate::webhooks::env("EVM_PRIVATE_KEY").ok_or_else(|| {
                CliError::Usage("set EVM_PRIVATE_KEY (the wallet's key, 64 hex digits)".into())
            })?;
            let key = WalletKey::parse(&key)?;
            let registry = identity_registry(chain, registry.as_deref())?;
            let owner = Address::parse(&owner)?;
            let deadline = now() + within;
            let digest =
                agent_wallet_digest(chain, &registry, agent_id, &key.address(), &owner, deadline);
            let signature = key.sign(&digest)?;
            eprintln!(
                "{} signs that it is agent {agent_id}'s wallet until {}; {owner} sends this:",
                key.address(),
                sion_agent::schedule::rfc3339(deadline)
            );
            print_tx(&agent_wallet_tx(
                chain,
                &registry,
                agent_id,
                &key.address(),
                deadline,
                &signature,
            ))
        }
        Erc8004Command::Feedback {
            agent_id,
            value,
            tag1,
            tag2,
            endpoint,
            client,
            uri,
            chain,
            registry,
            reputation,
            out,
        } => {
            let (value, decimals) = parse_decimal(&value)?;
            let feedback = Feedback {
                agent: agent_id,
                value,
                decimals,
                tags: (tag1, tag2),
                endpoint,
            };
            let file = feedback_file(
                &feedback,
                chain,
                &identity_registry(chain, registry.as_deref())?,
                &Address::parse(&client)?,
                &sion_agent::schedule::rfc3339(now()),
                &serde_json::Map::new(),
            );
            let bytes =
                serde_json::to_vec_pretty(&file).map_err(|e| CliError::Usage(e.to_string()))?;
            let (uri, hash) = match uri {
                Some(uri) => (uri, keccak256(&bytes)),
                None => (String::new(), [0u8; 32]),
            };
            match &out {
                Some(path) => {
                    std::fs::write(path, &bytes)
                        .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
                    eprintln!(
                        "the feedback's file: {} (Keccak 0x{}); put it at the URI the call names",
                        path.display(),
                        hex::encode(keccak256(&bytes))
                    );
                }
                None => eprintln!("{}", String::from_utf8_lossy(&bytes)),
            }
            print_tx(&feedback_tx(
                chain,
                &reputation_registry(chain, reputation.as_deref())?,
                &feedback,
                &uri,
                hash,
            )?)
        }
        Erc8004Command::Validate {
            request,
            response,
            uri,
            found,
            tag,
            chain,
            registry,
        } => {
            let hash = match &found {
                Some(path) => keccak256(
                    &std::fs::read(path)
                        .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?,
                ),
                None => [0u8; 32],
            };
            print_tx(&validation_tx(
                chain,
                &Address::parse(&registry)?,
                digest_arg(&request)?,
                response,
                &uri,
                hash,
                &tag,
            )?)
        }
        Erc8004Command::Check { file } => {
            let bytes = std::fs::read(&file)
                .map_err(|e| CliError::Usage(format!("{}: {e}", file.display())))?;
            let found = read_registration(&bytes)?;
            println!("{}: a registration file for {}", file.display(), found.name);
            for service in &found.services {
                println!("  {}: {}", service.name, service.endpoint);
            }
            for entry in &found.registrations {
                println!("  agent {} in {}", entry.agent, entry.registry);
            }
            if found.trust.is_empty() {
                println!("  no trust named: for discovery only");
            } else {
                println!("  trust: {}", found.trust.join(", "));
            }
            Ok(())
        }
        Erc8004Command::Card { .. } => Err(CliError::Usage("this needs a node".into())),
    }
}

/// Runs an ERC-8004 command.
pub fn run(ledger: &dyn crate::ledger::Ledger, command: Erc8004Command) -> Result<(), CliError> {
    let Erc8004Command::Card {
        profile,
        description,
        a2a,
        mcp,
        web,
        image,
        x402,
        agent_id,
        chain,
        registry,
        out,
        data_uri: as_uri,
    } = command
    else {
        return run_local(command);
    };
    let found = crate::profiles::get(&profile)?;
    let registered = match agent_id {
        Some(agent) => Some((agent, chain, identity_registry(chain, registry.as_deref())?)),
        None => None,
    };
    let file = registration(&Card {
        name: found.name.clone(),
        description: description.unwrap_or_else(|| {
            "An agent on Siontology: its charter, its record and the bonds behind its claims are at its DID.".into()
        }),
        did: crate::agents::did(ledger, &found.agent)?,
        image,
        a2a,
        mcp,
        web,
        x402,
        registered,
    });
    let bytes = serde_json::to_vec_pretty(&file).map_err(|e| CliError::Usage(e.to_string()))?;
    match &out {
        Some(path) => {
            std::fs::write(path, &bytes)
                .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
            println!(
                "wrote {}'s registration file: {}",
                found.name,
                path.display()
            );
        }
        None => println!("{}", String::from_utf8_lossy(&bytes)),
    }
    if as_uri {
        println!("{}", data_uri(&bytes));
    }
    Ok(())
}
