//! Signed web requests, so that a site can tell whose agent asks.
//!
//! This is HTTP Message Signatures (RFC 9421) as the IETF's Web Bot Auth
//! working group profiles them for automated clients
//! (`draft-ietf-webbotauth-httpsig-protocol-01`, 29 October 2026). A
//! signed request carries three headers:
//!
//! - `Signature-Agent`, a dictionary whose member says where the signer's
//!   keys are published: an HTTPS address that answers with a JWK Set
//!   ([`directory`]);
//! - `Signature-Input`, what was signed and how: the request's authority
//!   and that member, when the signature was made and when it expires
//!   ([`VALID_SECS`] later), the key's id (its JWK thumbprint, RFC 7638),
//!   a nonce, and the tag `web-bot-auth`;
//! - `Signature`, an Ed25519 signature of those.
//!
//! A signature says who asks, and that the request was made for this
//! host within these minutes. It does not cover the path or the body, so
//! whoever sees a signed request can send another to the same host with
//! it until it expires; it does not make a site answer; and it is worth
//! what the site makes of the address the keys are published at. The
//! profile is a draft and may change: what is built is the revision named
//! above, checked against its test vector.

use crate::encoding::{base64, base64_url};

/// The tag of a Web Bot Auth signature.
pub const TAG: &str = "web-bot-auth";

/// The label a request's signature goes under.
pub const LABEL: &str = "sig1";

/// How long a signature is good for, in seconds: long enough for a slow
/// connection and a clock a little off, short enough that one that was
/// seen is of little use.
pub const VALID_SECS: u64 = 300;

/// The media type of a key directory.
pub const DIRECTORY_TYPE: &str = "application/http-message-signatures-directory+json";

/// The well-known path of a key directory.
pub const DIRECTORY_PATH: &str = "/.well-known/http-message-signatures-directory";

/// The longest address of a key directory taken.
pub const MAX_AGENT: usize = 512;

/// Signs requests with one Ed25519 key, as one signature agent.
#[derive(Clone)]
pub struct Signer {
    seed: [u8; 32],
    public: [u8; 32],
    agent: String,
}

impl std::fmt::Debug for Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The key is not shown.
        f.debug_struct("Signer")
            .field("key_id", &self.key_id())
            .field("agent", &self.agent)
            .finish()
    }
}

impl Signer {
    /// A signer with the key of `seed`, whose keys are published at
    /// `agent`.
    pub fn new(seed: [u8; 32], agent: &str) -> Result<Signer, String> {
        check_agent(agent)?;
        let public = sion_http::cipher::ed25519_public(&seed).map_err(|e| e.to_string())?;
        Ok(Signer {
            seed,
            public,
            agent: agent.to_string(),
        })
    }

    /// The key's id: the thumbprint of its public JWK.
    pub fn key_id(&self) -> String {
        thumbprint(&self.public)
    }

    /// Where its keys are published.
    pub fn agent(&self) -> &str {
        &self.agent
    }

    /// The public key.
    pub fn public(&self) -> &[u8; 32] {
        &self.public
    }

    /// The JWK Set to publish at [`Signer::agent`].
    pub fn directory(&self) -> serde_json::Value {
        directory(&self.public)
    }

    /// The headers that sign a request to `url` made at `now` (seconds
    /// since 1970), with `nonce`, under `label`.
    pub fn headers_at(
        &self,
        url: &str,
        label: &str,
        now: u64,
        nonce: &[u8],
    ) -> Result<Vec<(&'static str, String)>, String> {
        let authority = authority(url)?;
        let params = params(
            label,
            now,
            now.saturating_add(VALID_SECS),
            &self.key_id(),
            &base64(nonce),
        );
        let base = signature_base(&authority, label, &self.agent, &params);
        let signature = sion_http::cipher::ed25519_sign(&self.seed, base.as_bytes())
            .map_err(|e| e.to_string())?;
        Ok(vec![
            ("Signature-Agent", format!("{label}=\"{}\"", self.agent)),
            ("Signature-Input", format!("{label}={params}")),
            ("Signature", format!("{label}=:{}:", base64(&signature))),
        ])
    }

    /// The headers that sign a request to `url` made now, with a nonce
    /// of its own. Only a request over HTTPS is signed, or one to this
    /// machine itself: a signature sent in the clear could be used, while
    /// it holds, by whoever saw it pass.
    pub fn headers(&self, url: &str) -> Result<Vec<(&'static str, String)>, String> {
        let secure = url
            .get(..8)
            .is_some_and(|start| start.eq_ignore_ascii_case("https://"));
        if !secure && !loopback(&authority(url)?) {
            return Err("only requests over HTTPS are signed".into());
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs();
        let mut nonce = [0u8; 32];
        sion_http::random(&mut nonce).map_err(|e| e.to_string())?;
        self.headers_at(url, LABEL, now, &nonce)
    }
}

/// Checks that `agent` can be where a signer's keys are published: an
/// absolute HTTPS address, written in a header as it is.
pub fn check_agent(agent: &str) -> Result<(), String> {
    let rest = agent
        .strip_prefix("https://")
        .ok_or("the address the keys are published at must begin with https://")?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if host.is_empty() || host.contains('@') {
        return Err("the address the keys are published at names no host".into());
    }
    if agent.len() > MAX_AGENT {
        return Err(format!(
            "the address the keys are published at is longer than {MAX_AGENT} characters"
        ));
    }
    // A string of a structured field: printable ASCII, and here nothing
    // that would need escaping either.
    if !agent
        .bytes()
        .all(|b| (0x21..=0x7e).contains(&b) && b != b'"' && b != b'\\')
    {
        return Err(
            "the address the keys are published at may hold only printable ASCII, without \
             spaces, quotes or backslashes"
                .into(),
        );
    }
    if agent.contains('#') {
        return Err("the address the keys are published at may not have a fragment".into());
    }
    Ok(())
}

/// The thumbprint of the Ed25519 public key `public` as a JWK (RFC 7638,
/// with the members RFC 8037 names): what names the key in a signature
/// and in a directory.
pub fn thumbprint(public: &[u8; 32]) -> String {
    let jwk = format!(
        r#"{{"crv":"Ed25519","kty":"OKP","x":"{}"}}"#,
        base64_url(public)
    );
    base64_url(&sion_http::sha256(jwk.as_bytes()))
}

/// The JWK Set that publishes `public`: what the address a signer names
/// answers with, as [`DIRECTORY_TYPE`].
pub fn directory(public: &[u8; 32]) -> serde_json::Value {
    serde_json::json!({
        "keys": [{
            "kty": "OKP",
            "crv": "Ed25519",
            "x": base64_url(public),
            "kid": thumbprint(public),
            "use": "sig",
        }]
    })
}

/// The authority of `url` as a signature covers it (RFC 9421, 2.2.3): the
/// host in lower case, and the port unless it is the scheme's own. It is
/// read by the parser the request is made with ([`sion_http::destination`]),
/// so that what is signed is the host the request goes to and no other: a
/// URL two parsers read differently (`https://a.example\@b.example/`)
/// would otherwise be signed for one host and sent to another, which
/// could then use the signature at the first.
pub fn authority(url: &str) -> Result<String, String> {
    sion_http::destination(url)
        .map(|to| to.authority())
        .map_err(|e| e.to_string())
}

/// Whether `authority` is this machine itself: `localhost`, or a loopback
/// address written out.
fn loopback(authority: &str) -> bool {
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => authority.split(':').next().unwrap_or_default(),
    };
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// The parameters of a signature: what it covers, when it was made
/// (`created`) and until when it holds (`expires`), the key's id, a nonce
/// and the tag.
pub fn params(label: &str, created: u64, expires: u64, key_id: &str, nonce: &str) -> String {
    format!(
        "(\"@authority\" \"signature-agent\";key=\"{label}\");created={created};\
         keyid=\"{key_id}\";alg=\"ed25519\";expires={expires};nonce=\"{nonce}\";tag=\"{TAG}\""
    )
}

/// What is signed (RFC 9421, 2.5): the request's authority, the member of
/// `Signature-Agent` under `label`, and the signature's parameters.
pub fn signature_base(authority: &str, label: &str, agent: &str, params: &str) -> String {
    format!(
        "\"@authority\": {authority}\n\"signature-agent\";key=\"{label}\": \"{agent}\"\n\
         \"@signature-params\": {params}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The test key of RFC 9421 (B.1.4), which the draft's vectors use.
    fn test_seed() -> [u8; 32] {
        let der = crate::encoding::from_base64(
            "MC4CAQAwBQYDK2VwBCIEIJ+DYvh6SEqVTm50DFtMDoQikTmiCqirVv9mWG9qfSnF",
        )
        .unwrap();
        der[16..].try_into().unwrap()
    }

    #[test]
    fn a_request_is_signed_as_the_drafts_test_vector_is() {
        // draft-ietf-webbotauth-httpsig-protocol-01, the Ed25519 vector
        // with a Signature-Agent header.
        let signer = Signer::new(test_seed(), "https://signature-agent.test").unwrap();
        assert_eq!(
            base64_url(signer.public()),
            "JrQLj5P_89iXES9-vFgrIy29clF9CC_oPPsw3c5D0bs"
        );
        assert_eq!(
            signer.key_id(),
            "poqkLGiymh_W0uP6PZFw-dvez3QJT5SolqXBCW38r0U"
        );
        let nonce = crate::encoding::from_base64(
            "n9p433xm+NJ3ph3upfBIGmsuwHw387YV7Q/F+6BSpGCVjYCqQw6rznNA8PVVLySrAWsv0hQtFioQb6E1YsauiA==",
        )
        .unwrap();
        // The vector's signature holds for a long time: made with its own
        // expiry rather than ours.
        let params = params(
            "sig2",
            1_735_689_600,
            4_889_289_600,
            &signer.key_id(),
            &base64(&nonce),
        );
        assert_eq!(
            params,
            "(\"@authority\" \"signature-agent\";key=\"sig2\");created=1735689600;\
             keyid=\"poqkLGiymh_W0uP6PZFw-dvez3QJT5SolqXBCW38r0U\";alg=\"ed25519\";\
             expires=4889289600;nonce=\"n9p433xm+NJ3ph3upfBIGmsuwHw387YV7Q/F+6BSpGCVjYCqQw6rznNA8PVVLySrAWsv0hQtFioQb6E1YsauiA==\";\
             tag=\"web-bot-auth\""
        );
        let base = signature_base("example.com", "sig2", signer.agent(), &params);
        let signature = sion_http::cipher::ed25519_sign(&test_seed(), base.as_bytes()).unwrap();
        assert_eq!(
            base64(&signature),
            "CGokAoY/FrPpyuzTe1Fb2kLTAH9Idw6qSFd1+D77r1firsMRCxeuLu5e6O+ek3zQgDMAzjiXtLVfqE00xnABCQ=="
        );
    }

    #[test]
    fn a_requests_headers_verify_against_the_published_key() {
        let signer = Signer::new([7u8; 32], "https://agents.example.org/keys.json").unwrap();
        let headers = signer
            .headers_at(
                "https://Shop.Example.com:8443/a?b=c",
                LABEL,
                1_800_000_000,
                &[9u8; 32],
            )
            .unwrap();
        let get = |name: &str| {
            headers
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        assert_eq!(
            get("Signature-Agent"),
            "sig1=\"https://agents.example.org/keys.json\""
        );
        let input = get("Signature-Input");
        let params = input.strip_prefix("sig1=").unwrap();
        assert!(params.contains(";created=1800000000;"));
        assert!(params.contains(";expires=1800000300;"));
        assert!(params.ends_with(";tag=\"web-bot-auth\""));
        // A verifier rebuilds the base from the request and the headers,
        // and checks it with the key the directory publishes.
        let directory = signer.directory();
        let key = &directory["keys"][0];
        assert_eq!(key["kid"], signer.key_id());
        assert!(params.contains(&format!(";keyid=\"{}\";", signer.key_id())));
        let x = key["x"]
            .as_str()
            .unwrap()
            .replace('-', "+")
            .replace('_', "/");
        let public: [u8; 32] = crate::encoding::from_base64(&x)
            .unwrap()
            .try_into()
            .unwrap();
        let base = signature_base(
            "shop.example.com:8443",
            "sig1",
            "https://agents.example.org/keys.json",
            params,
        );
        let signature = get("Signature");
        let signature = crate::encoding::from_base64(
            signature
                .strip_prefix("sig1=:")
                .and_then(|s| s.strip_suffix(':'))
                .unwrap(),
        )
        .unwrap();
        assert!(sion_http::cipher::ed25519_verify(
            &public,
            base.as_bytes(),
            &signature
        ));
        // Not for another host.
        let other = signature_base("evil.example", "sig1", signer.agent(), params);
        assert!(!sion_http::cipher::ed25519_verify(
            &public,
            other.as_bytes(),
            &signature
        ));
        // Two requests do not share a nonce.
        let (a, b) = (
            signer.headers("https://example.com/").unwrap(),
            signer.headers("https://example.com/").unwrap(),
        );
        assert_ne!(a[1].1, b[1].1);
        // Nothing is signed that would go in the clear, but to this
        // machine itself.
        assert!(signer.headers("http://example.com/").is_err());
        assert!(signer.headers("http://127.0.0.1:8080/x").is_ok());
        assert!(signer.headers("http://localhost/x").is_ok());
        assert!(signer.headers("http://[::1]:9/x").is_ok());
        assert!(signer.headers("HTTPS://example.com/").is_ok());
        // The key is not in what a signer prints of itself.
        assert!(!format!("{signer:?}").contains("7, 7"));
    }

    #[test]
    fn an_authority_is_the_host_in_lower_case_and_a_port_that_is_not_the_default() {
        for (url, expected) in [
            ("https://Example.COM/path?q=1", "example.com"),
            ("https://example.com:443/", "example.com"),
            ("https://example.com:8443", "example.com:8443"),
            ("http://example.com:80/x", "example.com"),
            ("http://example.com:8080/x", "example.com:8080"),
            ("https://user:pw@example.com/x", "example.com"),
            ("https://[2001:DB8::1]:8443/x", "[2001:db8::1]:8443"),
            ("https://[2001:db8::1]/x", "[2001:db8::1]"),
            ("https://example.com?x=1", "example.com"),
            // As the client reads them, which is where the request goes: a
            // backslash ends the host, so what follows the @ is a path.
            ("https://evil.example\\@good.example/x", "evil.example"),
            ("https://good.example/@evil.example", "good.example"),
            ("https://good.example?@evil.example", "good.example"),
            ("http://0x7f.1:8080/", "127.0.0.1:8080"),
        ] {
            assert_eq!(authority(url).as_deref(), Ok(expected), "{url}");
        }
        for bad in [
            "example.com",
            "ftp://example.com/",
            "https://example.com:x/",
            "https://[::1/x",
        ] {
            assert!(authority(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn where_the_keys_are_published_is_an_https_address_a_header_can_carry() {
        for good in [
            "https://agents.example.org",
            "https://agents.example.org/.well-known/http-message-signatures-directory",
            "https://example.org/keys.json?v=2",
        ] {
            assert!(check_agent(good).is_ok(), "{good}");
        }
        for bad in [
            "http://agents.example.org",
            "agents.example.org",
            "https://",
            "https://user@example.org/",
            "https://example.org/a b",
            "https://example.org/a\"b",
            "https://example.org/a\\b",
            "https://example.org/é",
            "https://example.org/#keys",
        ] {
            assert!(check_agent(bad).is_err(), "{bad}");
        }
        assert!(check_agent(&format!("https://example.org/{}", "a".repeat(600))).is_err());
        assert!(Signer::new([1u8; 32], "http://example.org").is_err());
    }
}
