//! `sion agent web-identity`: the key this machine's agents sign their web
//! requests with, and where it is published.
//!
//! With a web identity made, what an agent fetches from the web
//! (`fetch_url`, `fetch_paid`) is signed as the IETF's Web Bot Auth draft
//! has it ([`sion_agent::webbotauth`]), so that a site can tell whose
//! agent asks and treat it as it treats that operator. The key is an
//! Ed25519 key made here, kept sealed beside the other secrets
//! ([`crate::secrets`]); what is published is its public half, as a JWK
//! Set, at an HTTPS address of the operator's. Without a web identity,
//! nothing is signed.

use serde::{Deserialize, Serialize};
use sion_agent::webbotauth::{Signer, DIRECTORY_PATH, DIRECTORY_TYPE};
use zeroize::Zeroizing;

use crate::error::CliError;

/// The file the identity is kept in, in the secrets' directory.
const FILE: &str = "web-identity.bin";

/// What the file starts with.
const MAGIC: &[u8] = b"sion-web-identity-v1\n";

/// The identity as kept.
#[derive(Default, Serialize, Deserialize)]
struct Kept {
    /// The key's seed, in hex; empty when there is no identity.
    #[serde(default)]
    seed: String,
    /// Where the key is published.
    #[serde(default)]
    published_at: String,
}

impl Drop for Kept {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.seed);
    }
}

/// `sion agent web-identity …`
#[derive(Debug, clap::Subcommand)]
pub enum WebIdentityCommand {
    /// Make this machine's web identity: an Ed25519 key, kept sealed here,
    /// that your agents' web requests (fetch_url, fetch_paid) are signed
    /// with from then on, as Web Bot Auth has it (HTTP Message Signatures,
    /// RFC 9421). Prints the key directory to publish.
    New {
        /// The HTTPS address you will publish the key directory at: sites
        /// fetch it to check a signature, and it is what names you to
        /// them. The well-known place is
        /// https://<your host>/.well-known/http-message-signatures-directory.
        #[arg(long = "published-at", value_name = "URL")]
        published_at: String,
        /// Replace the identity there is: its key is gone for good, and
        /// sites that knew it no longer know your agents.
        #[arg(long)]
        replace: bool,
    },
    /// The identity: its key's id, where it is published, and the key
    /// directory to publish there.
    Show,
    /// Remove the identity: requests are no longer signed, and its key is
    /// gone for good.
    Remove,
}

/// The seed kept as `hex`, if it is one.
fn seed_of(hex: &str) -> Option<Zeroizing<[u8; 32]>> {
    if hex.len() != 64 {
        return None;
    }
    let mut seed = Zeroizing::new([0u8; 32]);
    for (i, byte) in seed.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(seed)
}

/// The signer of the identity kept in `dir`, if there is one.
pub fn signer_in(dir: &std::path::Path) -> Result<Option<Signer>, CliError> {
    let kept: Kept = crate::secrets::open_json(dir, FILE, MAGIC)?;
    if kept.seed.is_empty() {
        return Ok(None);
    }
    let seed = seed_of(&kept.seed)
        .ok_or_else(|| CliError::Usage("the web identity kept here holds no key".into()))?;
    Signer::new(*seed, &kept.published_at)
        .map(Some)
        .map_err(CliError::Usage)
}

/// This machine's signer, if it has a web identity.
pub fn signer() -> Result<Option<Signer>, CliError> {
    signer_in(&crate::secrets::dir())
}

/// Makes an identity in `dir`, published at `published_at`.
pub fn make_in(
    dir: &std::path::Path,
    published_at: &str,
    replace: bool,
) -> Result<Signer, CliError> {
    sion_agent::webbotauth::check_agent(published_at)
        .map_err(|e| CliError::Usage(format!("--published-at {published_at}: {e}")))?;
    if !replace && signer_in(dir)?.is_some() {
        return Err(CliError::Usage(
            "there is a web identity already (sion agent web-identity show): --replace makes \
             another, and sites that knew this one no longer know your agents"
                .into(),
        ));
    }
    let mut seed = Zeroizing::new([0u8; 32]);
    sion_http::random(seed.as_mut()).map_err(|e| CliError::Usage(e.to_string()))?;
    let signer = Signer::new(*seed, published_at).map_err(CliError::Usage)?;
    std::fs::create_dir_all(dir).map_err(|e| CliError::Usage(format!("{}: {e}", dir.display())))?;
    crate::secrets::seal_json(
        dir,
        FILE,
        MAGIC,
        &Kept {
            seed: sion_agent::encoding::hex(seed.as_slice()),
            published_at: published_at.to_string(),
        },
    )?;
    Ok(signer)
}

/// What to publish, and how, for `signer`.
fn publish_lines(signer: &Signer) -> Vec<String> {
    let mut lines = vec![
        format!("key {}", signer.key_id()),
        format!("published at {}", signer.agent()),
        format!("Publish this there, as {DIRECTORY_TYPE}, with no redirect on the way to it:"),
        signer.directory().to_string(),
    ];
    if !signer.agent().ends_with(DIRECTORY_PATH) {
        lines.push(format!(
            "(The well-known place for it is https://<your host>{DIRECTORY_PATH}; any HTTPS \
             address that answers with it will do.)"
        ));
    }
    lines
}

/// Runs a web identity command.
pub fn run(command: WebIdentityCommand) -> Result<(), CliError> {
    let dir = crate::secrets::dir();
    match command {
        WebIdentityCommand::New {
            published_at,
            replace,
        } => {
            let signer = make_in(&dir, &published_at, replace)?;
            println!(
                "web identity made, kept sealed in {}",
                dir.join(FILE).display()
            );
            for line in publish_lines(&signer) {
                println!("{line}");
            }
            println!(
                "From now on what your agents fetch from the web is signed with it. Until the \
                 directory is published, sites cannot check the signature."
            );
            Ok(())
        }
        WebIdentityCommand::Show => match signer_in(&dir)? {
            Some(signer) => {
                for line in publish_lines(&signer) {
                    println!("{line}");
                }
                Ok(())
            }
            None => {
                println!(
                    "no web identity: requests are not signed (sion agent web-identity new \
                     --published-at <URL> makes one)"
                );
                Ok(())
            }
        },
        WebIdentityCommand::Remove => {
            let path = dir.join(FILE);
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    println!("web identity removed: requests are no longer signed");
                    Ok(())
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    println!("no web identity");
                    Ok(())
                }
                Err(e) => Err(CliError::Usage(format!("{}: {e}", path.display()))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identity_is_made_once_kept_sealed_and_signs() {
        let dir = std::env::temp_dir().join(format!("sion-web-identity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(signer_in(&dir).unwrap().is_none());
        // Where it is published is an HTTPS address.
        assert!(make_in(&dir, "http://agents.example.org/keys", false).is_err());
        let at = "https://agents.example.org/.well-known/http-message-signatures-directory";
        let made = make_in(&dir, at, false).unwrap();
        let kept = signer_in(&dir).unwrap().unwrap();
        assert_eq!(kept.key_id(), made.key_id());
        assert_eq!(kept.agent(), at);
        // The file is sealed: neither the address nor the key's id reads
        // in it.
        let bytes = std::fs::read(dir.join(FILE)).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.starts_with("sion-web-identity-v1\n"));
        assert!(!text.contains("agents.example.org"));
        // A second is refused unless asked for, and is another key.
        assert!(make_in(&dir, at, false).is_err());
        let other = make_in(&dir, "https://example.org/keys.json", true).unwrap();
        assert_ne!(other.key_id(), made.key_id());
        assert_eq!(
            signer_in(&dir).unwrap().unwrap().agent(),
            "https://example.org/keys.json"
        );
        // What it signs names the key the directory publishes.
        let headers = other.headers("https://shop.example.com/x").unwrap();
        assert!(headers[1].1.contains(&other.key_id()));
        assert_eq!(other.directory()["keys"][0]["kid"], other.key_id());
        let lines = publish_lines(&other);
        assert!(lines.iter().any(|l| l.contains("well-known place")));
        assert!(!publish_lines(&made)
            .iter()
            .any(|l| l.contains("well-known place")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
