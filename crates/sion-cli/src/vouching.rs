//! Memory shared between owners: who vouched for a fact, and pulling one
//! back.
//!
//! An agent can keep another owner's curated log in mind
//! (`--shared-memory`), and every memory atom already carries where it came
//! from (`sion_memory`'s provenance: its sources, the spans in them, the
//! model that read them). What this adds is the other two halves of
//! trusting someone else's facts:
//!
//! - **Vouching.** An account publishes a claim that it stands behind an
//!   atom ([`vouch_body`]), on a bond of its own, with an adjudicator who
//!   can rule against it. So "who says so" is on the ledger and costs
//!   something, and a reader weighs a fact by who vouched rather than by
//!   whose log it sits in.
//! - **Revocation.** The writer, or a voucher, publishes that an atom is
//!   not to be relied on any more ([`revoke_body`]), named in its index so
//!   a reader proves against a certified header whether a revocation
//!   stands.
//!
//! A revoked fact does not go away — nothing on the ledger does — and
//! neither do the facts drawn from it. [`pulled`] finds those: an atom that
//! was derived from, or assumes, a revoked atom rests on it, and so does
//! anything resting on *those*, however deep. A reader leaves all of them
//! out, which is why a poisoned fact can be traced and pulled rather than
//! only marked.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sion_assurance::{AtomClass, Formula};
use sion_core::ids::{AccountId, AtomId, ClaimId};
use sion_exec::claim::{claim_id, formulas, ClaimBody, Payload};
use sion_exec::execute::bond_atom;
use sion_exec::TxKind;
use sion_memory::atom::{LinkKind, MemoryAtom, MemoryAtomId};

use crate::error::CliError;
use crate::ledger::{Ledger, LedgerExt};
use crate::session::Session;

/// The schema of a claim vouching for an atom.
pub const VOUCH_SCHEMA: &str = "memory.vouch/v1";

/// The schema of a claim revoking an atom.
pub const REVOKE_SCHEMA: &str = "memory.revocation/v1";

/// The bond a voucher stands behind its vouching with.
pub const VOUCHER_BOND: &str = "memory-voucher";

/// What a vouching or a revocation says, in its claim's payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct About {
    /// The atom it is about.
    pub atom: MemoryAtomId,
    /// Whose log holds it.
    pub writer: AccountId,
    /// Its place in that log.
    pub seq: u64,
    /// What the voucher or revoker says, in a line.
    pub note: String,
}

/// The index key a revocation is named under, so a reader can prove
/// whether one stands: `memory-revoked:<atom>`.
pub fn revoked_key(atom: &MemoryAtomId) -> String {
    format!("memory-revoked:{}", hex::encode(&atom.as_hash().0[..16]))
}

/// The body of a claim vouching for `about`, on the voucher's bond, which
/// `adjudicator` can rule against.
pub fn vouch_body(
    voucher: &AccountId,
    about: &About,
    adjudicator: AtomId,
    valid_for: u64,
) -> Result<ClaimBody, CliError> {
    let payload = serde_json::to_vec(about)
        .map_err(|e| CliError::Usage(format!("the vouching could not be written: {e}")))?;
    Ok(ClaimBody {
        schema: VOUCH_SCHEMA.into(),
        subject: format!("vouches for memory atom {}", about.atom),
        payload: Payload::Public(payload),
        formula: formulas::bonded_statement(
            bond_atom(AtomClass::Economic, voucher, VOUCHER_BOND).id,
        ),
        adjudicator: Some(adjudicator),
        detection_permille: 1_000,
        valid_for,
        challenge_period: 100,
    })
}

/// The body of a claim revoking `about`: that it is not to be relied on
/// any more. It rests on the revoker's signature alone — withdrawing a
/// fact is not a statement that can be proven false.
pub fn revoke_body(about: &About, valid_for: u64) -> Result<ClaimBody, CliError> {
    let payload = serde_json::to_vec(about)
        .map_err(|e| CliError::Usage(format!("the revocation could not be written: {e}")))?;
    Ok(ClaimBody {
        schema: REVOKE_SCHEMA.into(),
        subject: format!("revokes memory atom {}", about.atom),
        payload: Payload::Public(payload),
        formula: Formula::and(vec![formulas::dual_signature()]),
        adjudicator: None,
        detection_permille: 0,
        valid_for,
        challenge_period: 0,
    })
}

/// Publishes `body` as `wallet`'s claim; with `key`, also names it the
/// latest under that index key.
pub(crate) fn publish(
    session: &mut Session<'_>,
    wallet: &str,
    body: ClaimBody,
    key: Option<String>,
) -> Result<ClaimId, CliError> {
    let id = claim_id(&session.id(wallet), &body);
    // Each must succeed: a claim the ledger refused, or one it never
    // named, is no vouching or revocation for a reader to find.
    let view = session.must(wallet, TxKind::IssueClaim { claim: body }, Vec::new())?;
    issued(session.ledger, view.pulse, id)?;
    if let Some(key) = key {
        session.must(wallet, TxKind::IndexClaim { key, claim: id }, Vec::new())?;
    }
    Ok(id)
}

/// Whether the ledger issued claim `id`, submitted in the Pulse `at`:
/// claims are issued at the end of their Pulse, and one it rejects (its
/// adjudicator or a bond it rests on unknown) is said with the reason.
fn issued(ledger: &dyn Ledger, at: sion_core::ids::Pulse, id: ClaimId) -> Result<(), CliError> {
    for view in ledger.events(at, 1_000)? {
        if view.pulse > at {
            break;
        }
        if let sion_exec::execute::Event::ClaimRejected { claim, reason } = &view.event {
            if *claim == id {
                return Err(CliError::Transaction(format!(
                    "the ledger did not issue claim {}: {reason}",
                    id.short()
                )));
            }
        }
    }
    Ok(())
}

/// The index key a vouching is named under, so a reader finds who
/// vouched: `memory-vouched:<atom>`.
pub fn vouched_key(atom: &MemoryAtomId) -> String {
    format!("memory-vouched:{}", hex::encode(&atom.as_hash().0[..16]))
}

/// Vouches for an atom as `wallet`: a claim on the voucher's bond, named
/// in the voucher's index so a reader finds it.
pub fn vouch(
    session: &mut Session<'_>,
    wallet: &str,
    about: &About,
    adjudicator: AtomId,
    valid_for: u64,
) -> Result<ClaimId, CliError> {
    let voucher = session.id(wallet);
    let body = vouch_body(&voucher, about, adjudicator, valid_for)?;
    publish(session, wallet, body, Some(vouched_key(&about.atom)))
}

/// A revocation a reader found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revocation {
    /// The atom revoked.
    pub atom: MemoryAtomId,
    /// Who revoked it.
    pub by: AccountId,
    /// What they said.
    pub why: String,
    /// The claim.
    pub claim: ClaimId,
}

/// A vouching a reader found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vouching {
    /// The atom vouched for.
    pub atom: MemoryAtomId,
    /// Who vouched.
    pub by: AccountId,
    /// The claim.
    pub claim: ClaimId,
    /// Whether the claim stood when last looked at: a vouching that
    /// expired or was shown false vouches for nothing.
    #[serde(default = "standing_by_default")]
    pub active: bool,
}

fn standing_by_default() -> bool {
    true
}

/// What a reader has found of the vouchings and revocations of one
/// shared log's atoms, kept beside its copy of the log and brought up to
/// date from the ledger's events ([`refresh`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Found {
    /// Whose log it is.
    pub writer: Option<AccountId>,
    /// The next Pulse whose events are to be read.
    pub next: u64,
    /// The revocations found, whoever made them.
    pub revocations: Vec<Revocation>,
    /// The vouchings found.
    pub vouchings: Vec<Vouching>,
    /// Pulses with more events than a node answers at once, read only as
    /// far as one page of them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub partial: Vec<u64>,
}

impl Found {
    /// Who vouches for each atom: whoever's vouching still stands and who
    /// has not withdrawn it (a voucher's revocation withdraws its own
    /// vouching, and only that).
    pub fn vouches(&self) -> BTreeMap<MemoryAtomId, Vec<AccountId>> {
        let mut vouches: BTreeMap<MemoryAtomId, Vec<AccountId>> = BTreeMap::new();
        for vouching in self.vouchings.iter().filter(|v| v.active) {
            let withdrawn = self
                .revocations
                .iter()
                .any(|r| r.atom == vouching.atom && r.by == vouching.by);
            if withdrawn {
                continue;
            }
            let by = vouches.entry(vouching.atom).or_default();
            if !by.contains(&vouching.by) {
                by.push(vouching.by);
            }
        }
        vouches
    }

    /// The revocations that pull a fact, by atom: the log's writer's.
    /// Anyone may publish a revocation; a voucher's withdraws its own
    /// vouching ([`Found::vouches`]), and anyone else's counts for nothing.
    pub fn revoked(&self) -> BTreeMap<MemoryAtomId, (AccountId, String)> {
        let mut revoked = BTreeMap::new();
        for revocation in &self.revocations {
            if Some(revocation.by) == self.writer {
                revoked
                    .entry(revocation.atom)
                    .or_insert((revocation.by, revocation.why.clone()));
            }
        }
        revoked
    }

    /// Where each of `atoms` stands by what was found.
    pub fn standings(&self, atoms: &[MemoryAtom]) -> Vec<Standing> {
        standings(atoms, &self.revoked(), &self.vouches())
    }

    /// Takes in the claim `claim`, which `owner` named under `key`: kept
    /// if it is a vouching or revocation of an atom of this writer's log,
    /// by that owner, named under the key that atom's vouching or
    /// revocation is named under.
    /// A claim that is not one is passed over; one that could not be read
    /// (the node out of reach) is an error, so that nothing is skipped.
    fn take(
        &mut self,
        ledger: &dyn Ledger,
        owner: AccountId,
        key: &str,
        claim: ClaimId,
    ) -> Result<(), CliError> {
        let (about, schema, issuer) = match read_about(ledger, &claim.to_string()) {
            Ok(read) => read,
            Err(CliError::Usage(_) | CliError::Protocol(_)) => return Ok(()),
            Err(e) => return Err(e),
        };
        if issuer != owner || Some(about.writer) != self.writer {
            return Ok(());
        }
        if schema == REVOKE_SCHEMA && key == revoked_key(&about.atom) {
            let revocation = Revocation {
                atom: about.atom,
                by: owner,
                why: about.note,
                claim,
            };
            if !self.revocations.contains(&revocation) {
                self.revocations.push(revocation);
            }
        } else if schema == VOUCH_SCHEMA
            && key == vouched_key(&about.atom)
            && !self.vouchings.iter().any(|v| v.claim == claim)
        {
            self.vouchings.push(Vouching {
                atom: about.atom,
                by: owner,
                claim,
                active: true,
            });
        }
        Ok(())
    }

    /// Looks again at whether each vouching's claim still stands.
    pub fn recheck(&mut self, ledger: &dyn Ledger) -> Result<(), CliError> {
        for vouching in &mut self.vouchings {
            let value = ledger.claim(vouching.claim)?;
            vouching.active = value["status"].as_str() == Some("Active");
        }
        Ok(())
    }

    /// Reads the ledger's events from where it left off to the latest,
    /// taking in each claim named under a vouching's or revocation's key.
    /// A Pulse with more events than a node answers at once is read only
    /// as far as one page of it, and noted in [`Found::partial`]. On an
    /// error it stops where the page it was reading began.
    pub fn scan(&mut self, ledger: &dyn Ledger) -> Result<(), CliError> {
        // As many as a node answers at once.
        const PAGE: usize = 5_000;
        loop {
            let mut page = ledger.events(sion_core::ids::Pulse(self.next), PAGE)?;
            let full = page.len() >= PAGE;
            let (Some(first), Some(last)) = (
                page.first().map(|view| view.pulse),
                page.last().map(|view| view.pulse),
            ) else {
                return Ok(());
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
                if let sion_exec::execute::Event::ClaimIndexed {
                    owner, key, claim, ..
                } = &view.event
                {
                    if key.starts_with("memory-revoked:") || key.starts_with("memory-vouched:") {
                        self.take(ledger, *owner, key, *claim)?;
                    }
                }
            }
            if !whole {
                self.next = last.0;
                continue;
            }
            self.next = last.0.saturating_add(1);
            if !full {
                return Ok(());
            }
        }
    }
}

/// Where a reader keeps what it found of `log`'s vouchings and
/// revocations: beside its copy of the log.
pub fn found_path(log: &sion_core::ids::ObjectId) -> std::path::PathBuf {
    crate::memory::store_dir(log).with_extension("standing.json")
}

/// What was found of `log`'s vouchings and revocations when it was last
/// brought up to date (nothing, if never).
pub fn found(log: &sion_core::ids::ObjectId) -> Found {
    std::fs::read_to_string(found_path(log))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Brings what this reader knows of `log`'s vouchings and revocations up
/// to date from the ledger's events, and keeps it.
pub fn refresh(ledger: &dyn Ledger, log: &sion_core::ids::ObjectId) -> Result<Found, CliError> {
    let writer = crate::memory::committed(ledger, log)?.writer;
    let mut kept = found(log);
    if kept.writer != Some(writer) {
        kept = Found {
            writer: Some(writer),
            ..Found::default()
        };
    }
    kept.scan(ledger)?;
    kept.recheck(ledger)?;
    crate::profiles::write_json(&found_path(log), &kept)?;
    Ok(kept)
}

/// The atom at `seq` of `log`, from this machine's copy of it, with whom
/// it is about.
pub fn about_atom(
    ledger: &dyn Ledger,
    log: &sion_core::ids::ObjectId,
    seq: u64,
    note: &str,
) -> Result<About, CliError> {
    let writer = crate::memory::committed(ledger, log)?.writer;
    let store = sion_memory::LogStore::open(crate::memory::store_dir(log)).map_err(|e| {
        CliError::Usage(format!(
            "this machine has no copy of log {}: {e}",
            log.0.short()
        ))
    })?;
    let atom = store
        .log()
        .atoms()
        .iter()
        .find(|atom| atom.seq == seq)
        .ok_or_else(|| {
            CliError::Usage(format!(
                "this machine's copy of log {} has no atom {seq}",
                log.0.short()
            ))
        })?;
    if atom.writer != writer {
        return Err(CliError::Usage(format!(
            "atom {seq} of the copy is not the log's writer's"
        )));
    }
    Ok(About {
        atom: atom.id(),
        writer,
        seq,
        note: note.to_string(),
    })
}

/// Revokes an atom as `wallet`, naming it in the revoker's index so a
/// reader can prove the revocation stands.
pub fn revoke(
    session: &mut Session<'_>,
    wallet: &str,
    about: &About,
    valid_for: u64,
) -> Result<ClaimId, CliError> {
    let body = revoke_body(about, valid_for)?;
    publish(session, wallet, body, Some(revoked_key(&about.atom)))
}

/// Reads the `About` a vouching or revocation claim holds.
pub fn read_about(ledger: &dyn Ledger, id: &str) -> Result<(About, String, AccountId), CliError> {
    let stored = crate::tasks::stored_claim(ledger, id)?;
    if stored.body.schema != VOUCH_SCHEMA && stored.body.schema != REVOKE_SCHEMA {
        return Err(CliError::Usage(format!(
            "claim {} is a {}, not a vouching or a revocation",
            stored.id.short(),
            stored.body.schema
        )));
    }
    let Payload::Public(bytes) = &stored.body.payload else {
        return Err(CliError::Usage("a vouching's payload is public".into()));
    };
    let about: About = serde_json::from_slice(bytes)
        .map_err(|e| CliError::Protocol(format!("its payload: {e}")))?;
    Ok((about, stored.body.schema.clone(), stored.issuer))
}

/// Where a shared fact stands for a reader.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Standing {
    /// Revoked by whoever is named, with what they said: left out, and so
    /// is anything resting on it.
    Revoked {
        /// Who revoked it.
        by: AccountId,
        /// What they said.
        why: String,
    },
    /// Left out because it rests on a revoked atom, named here.
    RestsOnRevoked {
        /// The revoked atom beneath it.
        atom: MemoryAtomId,
    },
    /// Vouched for by these accounts.
    Vouched {
        /// Who vouched.
        by: Vec<AccountId>,
    },
    /// Nobody vouched and nobody revoked: its writer's word alone.
    Plain,
}

impl Standing {
    /// Whether a reader keeps the fact in mind.
    pub fn kept(&self) -> bool {
        matches!(self, Standing::Vouched { .. } | Standing::Plain)
    }

    /// A line saying where it stands.
    pub fn describe(&self) -> String {
        match self {
            Standing::Revoked { by, why } => {
                format!("revoked by {}: {why}", by.short())
            }
            Standing::RestsOnRevoked { atom } => {
                format!("rests on the revoked atom {atom}")
            }
            Standing::Vouched { by } => format!(
                "vouched for by {}",
                by.iter()
                    .map(AccountId::short)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Standing::Plain => "its writer's word alone".into(),
        }
    }
}

/// Which of `atoms` rest on a revoked one, by index: those that were
/// derived from or assume a revoked atom, and those resting on *them*, as
/// far as the links go. The walk ends: an atom is settled once.
pub fn pulled(atoms: &[MemoryAtom], revoked: &BTreeSet<MemoryAtomId>) -> BTreeSet<usize> {
    let id_of: Vec<MemoryAtomId> = atoms.iter().map(|atom| atom.id()).collect();
    // Whatever rests on something gone: start from the revoked themselves.
    let mut gone: BTreeSet<MemoryAtomId> = revoked.clone();
    let mut out: BTreeSet<usize> = BTreeSet::new();
    // Each round takes in whatever rests on what is already gone, until a
    // round adds nothing. At most one round per atom.
    for _ in 0..=atoms.len() {
        let mut added = false;
        for (index, atom) in atoms.iter().enumerate() {
            if out.contains(&index) || revoked.contains(&id_of[index]) {
                continue;
            }
            let rests = atom.links.iter().any(|link| {
                matches!(link.kind, LinkKind::DerivedFrom | LinkKind::Assumes)
                    && gone.contains(&link.target)
            });
            if rests {
                out.insert(index);
                gone.insert(id_of[index]);
                added = true;
            }
        }
        if !added {
            break;
        }
    }
    out
}

/// Where each of `atoms` stands, given who revoked what and who vouched
/// for what: the revoked are out, what rests on them is out, and the rest
/// carry who vouched.
pub fn standings(
    atoms: &[MemoryAtom],
    revocations: &BTreeMap<MemoryAtomId, (AccountId, String)>,
    vouches: &BTreeMap<MemoryAtomId, Vec<AccountId>>,
) -> Vec<Standing> {
    let revoked: BTreeSet<MemoryAtomId> = revocations.keys().copied().collect();
    let resting = pulled(atoms, &revoked);
    atoms
        .iter()
        .enumerate()
        .map(|(index, atom)| {
            let id = atom.id();
            if let Some((by, why)) = revocations.get(&id) {
                return Standing::Revoked {
                    by: *by,
                    why: why.clone(),
                };
            }
            if resting.contains(&index) {
                // Name a revoked atom it rests on, for the reader.
                let beneath = atom
                    .links
                    .iter()
                    .find(|link| matches!(link.kind, LinkKind::DerivedFrom | LinkKind::Assumes))
                    .map(|link| link.target)
                    .unwrap_or(id);
                return Standing::RestsOnRevoked { atom: beneath };
            }
            match vouches.get(&id) {
                Some(by) if !by.is_empty() => Standing::Vouched { by: by.clone() },
                _ => Standing::Plain,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sion_memory::atom::{Link, MemoryKind, Statement, Value};
    use sion_memory::geom::Interval;

    fn account(seed: &[u8]) -> AccountId {
        AccountId::from_hash(sion_core::hash::hash_domain("test/account", seed))
    }

    /// An atom of `writer`'s saying `text`, resting on `rests_on`.
    fn atom(writer: AccountId, seq: u64, text: &str, rests_on: &[MemoryAtomId]) -> MemoryAtom {
        let mut built = MemoryAtom::builder(
            writer,
            seq,
            MemoryKind::Observation,
            sion_memory::atom::AtomClass::Observation,
            Statement::claim("note", "says", Value::Entity(text.into())),
        )
        .valid(Interval::closed(1_000, 2_000))
        .observed_at(1_500)
        .build()
        .expect("a well-formed atom");
        built.links = rests_on
            .iter()
            .map(|target| Link {
                kind: LinkKind::DerivedFrom,
                target: *target,
                strength: 1_000,
            })
            .collect();
        built
    }

    #[test]
    fn what_rests_on_a_revoked_fact_is_pulled_however_deep() {
        let writer = account(b"writer");
        let first = atom(writer, 0, "the rate is 4%", &[]);
        let second = atom(writer, 1, "so the cost is 40", &[first.id()]);
        let third = atom(writer, 2, "so we should buy", &[second.id()]);
        let apart = atom(writer, 3, "the office is in Berlin", &[]);
        let atoms = [first.clone(), second, third, apart];
        let revoked: BTreeSet<_> = [first.id()].into_iter().collect();
        let out = pulled(&atoms, &revoked);
        // The two drawn from it, however deep; not the one apart, and the
        // revoked atom itself is not "resting on" anything.
        assert_eq!(out, [1usize, 2].into_iter().collect());
    }

    #[test]
    fn a_fact_that_rests_on_nothing_revoked_is_kept() {
        let writer = account(b"writer");
        let atoms = [atom(writer, 0, "a", &[]), atom(writer, 1, "b", &[])];
        assert!(pulled(&atoms, &BTreeSet::new()).is_empty());
    }

    #[test]
    fn a_long_chain_is_pulled_whatever_order_the_facts_come_in() {
        let writer = account(b"writer");
        // Twenty facts, each drawn from the one before.
        let mut chain = vec![atom(writer, 0, "base", &[])];
        for seq in 1..20u64 {
            let last = chain[chain.len() - 1].id();
            chain.push(atom(writer, seq, &format!("step {seq}"), &[last]));
        }
        let base = chain[0].id();
        let revoked: BTreeSet<_> = [base].into_iter().collect();
        // Every fact but the revoked one rests on it.
        let expected: BTreeSet<usize> = (1..20).collect();
        assert_eq!(pulled(&chain, &revoked), expected);
        // Given in reverse, where one pass over the list would miss most
        // of them, the answer is the same set of facts.
        let mut backwards = chain.clone();
        backwards.reverse();
        let out = pulled(&backwards, &revoked);
        let names: BTreeSet<MemoryAtomId> = out.iter().map(|i| backwards[*i].id()).collect();
        let wanted: BTreeSet<MemoryAtomId> = chain[1..].iter().map(MemoryAtom::id).collect();
        assert_eq!(names, wanted);
        // A content address cannot name a fact that names it back, so the
        // links can hold no circle for the walk to run around.
    }

    #[test]
    fn standings_say_what_is_revoked_what_rests_on_it_and_who_vouched() {
        let writer = account(b"writer");
        let auditor = account(b"auditor");
        let first = atom(writer, 0, "the rate is 4%", &[]);
        let second = atom(writer, 1, "so the cost is 40", &[first.id()]);
        let good = atom(writer, 2, "the office is in Berlin", &[]);
        let atoms = [first.clone(), second, good.clone()];
        let revocations: BTreeMap<_, _> =
            [(first.id(), (writer, "the rate was misread".to_string()))]
                .into_iter()
                .collect();
        let vouches: BTreeMap<_, _> = [(good.id(), vec![auditor])].into_iter().collect();
        let standings = standings(&atoms, &revocations, &vouches);
        assert!(matches!(standings[0], Standing::Revoked { .. }));
        assert!(matches!(standings[1], Standing::RestsOnRevoked { .. }));
        assert_eq!(standings[2], Standing::Vouched { by: vec![auditor] });
        // Only the last is kept in mind.
        assert_eq!(
            standings.iter().map(Standing::kept).collect::<Vec<_>>(),
            [false, false, true]
        );
        assert!(standings[0].describe().contains("the rate was misread"));
        assert!(standings[2].describe().contains("vouched for by"));
    }

    #[test]
    fn a_fact_nobody_vouched_for_is_its_writers_word() {
        let writer = account(b"writer");
        let only = atom(writer, 0, "a", &[]);
        let standings = standings(&[only], &BTreeMap::new(), &BTreeMap::new());
        assert_eq!(standings[0], Standing::Plain);
        assert!(standings[0].kept());
        assert!(standings[0].describe().contains("writer's word alone"));
    }

    #[test]
    fn a_revocation_is_named_under_its_atom_in_the_index() {
        let writer = account(b"writer");
        let one = atom(writer, 0, "a", &[]);
        let key = revoked_key(&one.id());
        assert!(key.starts_with("memory-revoked:"));
        // 16 bytes of the atom, so the key fits the index's 64.
        assert!(key.len() <= 64);
        assert_ne!(key, revoked_key(&atom(writer, 1, "b", &[]).id()));
    }

    #[test]
    fn only_the_writer_pulls_a_fact_and_a_voucher_withdraws_only_its_own() {
        let writer = account(b"writer");
        let dana = account(b"dana");
        let carol = account(b"carol");
        let eve = account(b"eve");
        let first = atom(writer, 0, "a", &[]);
        let drawn = atom(writer, 1, "b", &[first.id()]);
        let own = atom(writer, 2, "c", &[]);
        let atoms = [first.clone(), drawn.clone(), own.clone()];
        let claim = |seed: &[u8]| ClaimId::from_hash(sion_core::hash::hash_domain("c", seed));
        let revocation = |atom: &MemoryAtom, by: AccountId, seed: &[u8]| Revocation {
            atom: atom.id(),
            by,
            why: "no".into(),
            claim: claim(seed),
        };
        let vouching = |atom: &MemoryAtom, by: AccountId, seed: &[u8], active: bool| Vouching {
            atom: atom.id(),
            by,
            claim: claim(seed),
            active,
        };
        let mut found = Found {
            writer: Some(writer),
            // eve, who has a vouching of her own for the first, revokes it:
            // that withdraws her vouching, and pulls nothing.
            revocations: vec![revocation(&first, eve, b"r1")],
            vouchings: vec![
                vouching(&first, eve, b"v1", true),
                vouching(&own, dana, b"v2", true),
                vouching(&drawn, carol, b"v3", false),
            ],
            ..Found::default()
        };
        assert_eq!(
            found.standings(&atoms),
            vec![
                Standing::Plain,
                // carol's vouching no longer stands.
                Standing::Plain,
                Standing::Vouched { by: vec![dana] }
            ]
        );
        // dana withdraws hers: the fact is her writer's word again, still
        // kept in mind.
        found.revocations.push(revocation(&own, dana, b"r2"));
        assert_eq!(found.standings(&atoms)[2], Standing::Plain);
        // The writer revokes the first: it is out, and what was drawn from it.
        found.revocations.push(revocation(&first, writer, b"r3"));
        let standings = found.standings(&atoms);
        assert!(matches!(standings[0], Standing::Revoked { by, .. } if by == writer));
        assert_eq!(standings[1], Standing::RestsOnRevoked { atom: first.id() });
        assert!(standings[2].kept());
    }
}
