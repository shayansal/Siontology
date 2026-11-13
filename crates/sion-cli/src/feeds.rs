//! Claims found by the key they were named under.
//!
//! Anyone can publish a claim and name it in their own index
//! (`IndexClaim`). The ledger says so in an event, and that is how a
//! reader who does not know the publisher finds it: by reading the events
//! and keeping the claims named under the keys it cares about. A lesson an
//! agent kept ([`crate::lessons`]) is found this way, like a vouching for a
//! shared fact ([`crate::vouching`]).
//!
//! What is found is only that someone named a claim under a key. Whose it
//! is, whether it still stands, and whether it says what its key suggests
//! are read from the claim itself each time ([`read`]): a key is a name
//! anyone may use.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sion_core::ids::{AccountId, ClaimId, Pulse};
use sion_exec::claim::Payload;

use crate::error::CliError;
use crate::ledger::{Ledger, LedgerExt};

/// As many events as a node answers at once.
const PAGE: usize = 5_000;

/// A claim someone named under a key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Who named it, in their own index.
    pub owner: AccountId,
    /// The key.
    pub key: String,
    /// The claim.
    pub claim: ClaimId,
    /// The Pulse it was named in.
    pub pulse: u64,
}

/// The claims found so far under some keys, and where reading the
/// ledger's events left off.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feed {
    /// The next Pulse whose events are to be read.
    pub next: u64,
    /// What was found, in the order it was named.
    pub entries: Vec<Entry>,
    /// Pulses with more events than a node answers at once, read only as
    /// far as one page of them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub partial: Vec<u64>,
}

impl Feed {
    /// Reads the ledger's events from where it left off to the latest,
    /// keeping each claim named under a key that starts with one of
    /// `prefixes`. Returns how many were new. A Pulse with more events
    /// than a node answers at once is read only as far as one page of it,
    /// and noted in [`Feed::partial`]. On an error it stops where the page
    /// it was reading began.
    pub fn scan(&mut self, ledger: &dyn Ledger, prefixes: &[&str]) -> Result<usize, CliError> {
        let mut new = 0;
        loop {
            let mut page = ledger.events(Pulse(self.next), PAGE)?;
            let full = page.len() >= PAGE;
            let (Some(first), Some(last)) = (
                page.first().map(|view| view.pulse),
                page.last().map(|view| view.pulse),
            ) else {
                return Ok(new);
            };
            // A full page may end inside a Pulse: that Pulse is read again,
            // whole, with the next page.
            let whole = !(full && first != last);
            if !whole {
                page.retain(|view| view.pulse != last);
            } else if full && !self.partial.contains(&last.0) {
                self.partial.push(last.0);
            }
            for view in &page {
                let sion_exec::execute::Event::ClaimIndexed {
                    owner, key, claim, ..
                } = &view.event
                else {
                    continue;
                };
                if !prefixes.iter().any(|prefix| key.starts_with(prefix)) {
                    continue;
                }
                let entry = Entry {
                    owner: owner.to_owned(),
                    key: key.clone(),
                    claim: claim.to_owned(),
                    pulse: view.pulse.0,
                };
                if !self.entries.contains(&entry) {
                    self.entries.push(entry);
                    new += 1;
                }
            }
            if !whole {
                self.next = last.0;
                continue;
            }
            self.next = last.0.saturating_add(1);
            if !full {
                return Ok(new);
            }
        }
    }

    /// The entries named under keys that start with `prefix`, oldest first.
    pub fn under<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = &'a Entry> + 'a {
        self.entries
            .iter()
            .filter(move |entry| entry.key.starts_with(prefix))
    }
}

/// A claim read back from the ledger as what its schema says it holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Read<T> {
    /// What its payload holds.
    pub says: T,
    /// Who issued it.
    pub issuer: AccountId,
    /// Whether it stands: issued and neither expired, withdrawn nor ruled
    /// against.
    pub active: bool,
    /// The Pulse it was issued in.
    pub issued_at: u64,
}

/// Reads claim `id` as a `T`, if it is a claim of `schema` with a public
/// payload that reads as one: `None` where it is something else, so that a
/// key anyone may use names nothing by itself. An error is the node out of
/// reach, never a claim that does not fit.
pub fn read<T: DeserializeOwned>(
    ledger: &dyn Ledger,
    id: ClaimId,
    schema: &str,
) -> Result<Option<Read<T>>, CliError> {
    let value = ledger.claim(id)?;
    let Ok(stored) =
        serde_json::from_value::<sion_exec::claim::StoredClaim>(value["claim"].clone())
    else {
        return Ok(None);
    };
    if stored.body.schema != schema {
        return Ok(None);
    }
    let Payload::Public(bytes) = &stored.body.payload else {
        return Ok(None);
    };
    let Ok(says) = serde_json::from_slice::<T>(bytes) else {
        return Ok(None);
    };
    Ok(Some(Read {
        says,
        issuer: stored.issuer,
        active: value["status"].as_str() == Some("Active"),
        issued_at: stored.issued_at.0,
    }))
}

/// The first 16 bytes of `digest` in hex: what a key names a thing by.
pub fn short_key(digest: &sion_core::hash::Hash32) -> String {
    hex::encode(&digest.0[..16])
}
