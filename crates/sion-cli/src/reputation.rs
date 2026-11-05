//! What an agent's record is, and finding the best agent for a task.
//!
//! Every agent's work leaves a trail on the ledger: tasks it was paid for
//! ([`sion_exec`]'s `TaskSettled`), tasks it was named for and released
//! (`TaskCancelled`), and delivered results later proven false
//! (`Falsified`). [`records`] reads the events and adds them up per agent,
//! so a record is the ledger's own account of what an agent has done, not
//! anyone's say-so. [`Record::trust`] turns it into one number.
//!
//! [`rank`] finds the best agent for a task among candidates, each with a
//! line of what it offers (a rental listing's description, say): it weighs
//! how well the words fit the task (BM25, as a tool search does) against
//! the agent's record and price, so a cheap agent with no record does not
//! outrank a proven one on words alone.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sion_agent::tools::search_words;
use sion_core::ids::{AccountId, ObjectId};
use sion_core::Amount;
use sion_exec::execute::Event;

use crate::error::CliError;
use crate::ledger::{Ledger, LedgerExt};

/// What the ledger shows an agent has done.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// Tasks it was paid for.
    pub delivered: u64,
    /// The USD those came to.
    pub earned: Amount,
    /// Delivered results later proven false.
    pub failed: u64,
    /// Tasks it was named for and gave back before delivering.
    pub released: u64,
}

impl Record {
    /// A number from 0 for how much to trust it: deliveries raise it, a
    /// failed result or a release lowers it, and it climbs towards 1 the
    /// more it rests on. A record of nothing is 0.
    ///
    /// Not consensus-critical: a reader's own weighing, in floating point.
    pub fn trust(&self) -> f64 {
        let delivered = self.delivered as f64;
        let failed = self.failed as f64;
        let released = self.released as f64;
        // A failure counts several deliveries against it; a release, less.
        let good = delivered;
        let bad = failed * 4.0 + released * 0.5;
        if good + bad == 0.0 {
            return 0.0;
        }
        let rate = (good - bad).max(0.0) / (good + bad);
        // Towards `rate` as the record grows (a dozen tasks is most of it).
        let weight = delivered / (delivered + 12.0);
        rate * weight
    }
}

/// Every agent's record, read from the events from Pulse `from` on (up to
/// `limit` of them). A task's worker is learned when it is posted, so that
/// a cancelled task counts against the worker it named.
pub fn records(
    ledger: &dyn Ledger,
    from: sion_core::ids::Pulse,
    limit: usize,
) -> Result<BTreeMap<AccountId, Record>, CliError> {
    let events = ledger.events(from, limit)?;
    Ok(records_from(events.iter().map(|view| &view.event)))
}

/// Every agent's record, from `events` in the order they happened: the
/// aggregation [`records`] does, over whatever stream of events.
pub fn records_from<'a>(events: impl Iterator<Item = &'a Event>) -> BTreeMap<AccountId, Record> {
    let mut records: BTreeMap<AccountId, Record> = BTreeMap::new();
    // task → its named worker, and the result it settled with.
    let mut worker_of: BTreeMap<ObjectId, AccountId> = BTreeMap::new();
    let mut settled_result: BTreeMap<sion_core::ids::ClaimId, AccountId> = BTreeMap::new();
    for event in events {
        match event {
            Event::TaskPosted {
                task,
                worker: Some(worker),
                ..
            } => {
                worker_of.insert(*task, *worker);
            }
            Event::TaskSettled {
                worker,
                amount,
                result,
                ..
            } => {
                let record = records.entry(*worker).or_default();
                record.delivered += 1;
                record.earned = record.earned.checked_add(*amount).unwrap_or(record.earned);
                settled_result.insert(*result, *worker);
            }
            Event::TaskCancelled { task, to, .. } => {
                // A worker that released the task it was named for (the
                // refund goes to the poster or funder, not the worker).
                if let Some(worker) = worker_of.get(task) {
                    if worker != to {
                        records.entry(*worker).or_default().released += 1;
                    }
                }
            }
            Event::Falsified { claim, .. } => {
                if let Some(worker) = settled_result.get(claim) {
                    records.entry(*worker).or_default().failed += 1;
                }
            }
            _ => {}
        }
    }
    records
}

/// An agent a search may return: what it is, what it offers in words, its
/// price if it has one, and whether it runs in an enclave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// The agent.
    pub agent: ObjectId,
    /// The account behind it (the owner, for a rental).
    pub account: AccountId,
    /// A line of what it offers, matched against the task.
    pub offer: String,
    /// What it costs, if it is for rent: USD per thousand output tokens.
    pub per_1k_output: Option<Amount>,
    /// Whether it runs in an enclave whose attestation checked.
    pub attested: bool,
}

/// What a search requires.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Need {
    /// Only agents in an attested enclave.
    pub attested_only: bool,
    /// At most this USD per thousand output tokens.
    pub max_per_1k_output: Option<Amount>,
}

/// An agent a search returned, with why.
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    /// The candidate.
    pub candidate: Candidate,
    /// How well its offer fit the task (BM25, higher is better).
    pub fit: f64,
    /// Its trust, from its record ([`Record::trust`]).
    pub trust: f64,
    /// The score it was ranked by.
    pub score: f64,
}

/// How much the fit of the words counts next to the record; the rest is
/// the record. A proven agent is preferred, but not so far that words
/// stop mattering.
const FIT_WEIGHT: f64 = 0.6;

/// Ranks `candidates` for `task`, best first: the fit of each one's offer
/// to the task (by [`fit`]) weighed against its `records` trust, dropping
/// any that `need` rules out. A candidate with no fit at all is left out.
pub fn rank(
    task: &str,
    candidates: &[Candidate],
    records: &BTreeMap<AccountId, Record>,
    need: &Need,
) -> Vec<Match> {
    let wanted = search_words(task);
    // How rare each word is among the offers, for BM25.
    let offers: Vec<Vec<String>> = candidates.iter().map(|c| search_words(&c.offer)).collect();
    let mut matches: Vec<Match> = candidates
        .iter()
        .zip(&offers)
        .filter(|(c, _)| !need.attested_only || c.attested)
        .filter(|(c, _)| match (need.max_per_1k_output, c.per_1k_output) {
            (Some(most), Some(price)) => price.micros() <= most.micros(),
            (Some(_), None) => false,
            (None, _) => true,
        })
        .filter_map(|(candidate, words)| {
            let fit = fit(&wanted, words, &offers);
            if fit <= 0.0 {
                return None;
            }
            let trust = records.get(&candidate.account).map_or(0.0, Record::trust);
            let score = FIT_WEIGHT * fit + (1.0 - FIT_WEIGHT) * trust;
            Some(Match {
                candidate: candidate.clone(),
                fit,
                trust,
                score,
            })
        })
        .collect();
    matches.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                a.candidate
                    .agent
                    .to_string()
                    .cmp(&b.candidate.agent.to_string())
            })
    });
    matches
}

/// How well `words` fit `wanted`, by BM25 over the `offers` as the corpus,
/// normalised to roughly 0..1 by the best a perfect match of the query
/// would score.
fn fit(wanted: &[String], words: &[String], offers: &[Vec<String>]) -> f64 {
    if wanted.is_empty() || words.is_empty() {
        return 0.0;
    }
    let count = offers.len().max(1) as f64;
    let avg_len = offers.iter().map(|o| o.len()).sum::<usize>() as f64 / count;
    let length = words.len() as f64 / avg_len.max(1.0);
    const K1: f64 = 1.2;
    const B: f64 = 0.75;
    let mut score = 0.0;
    let mut best = 0.0;
    for word in wanted {
        let have = offers.iter().filter(|o| o.contains(word)).count() as f64;
        let rarity = (1.0 + (count - have + 0.5) / (have + 0.5)).ln();
        let frequency = words.iter().filter(|w| *w == word).count() as f64;
        best += rarity;
        if frequency > 0.0 {
            score += rarity * frequency * (K1 + 1.0) / (frequency + K1 * (1.0 - B + B * length));
        }
    }
    if best <= 0.0 {
        0.0
    } else {
        score / best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(seed: &[u8]) -> ObjectId {
        ObjectId::from_hash(sion_core::hash::hash_domain("test/agent", seed))
    }

    fn account(seed: &[u8]) -> AccountId {
        AccountId::from_hash(sion_core::hash::hash_domain("test/account", seed))
    }

    #[test]
    fn trust_rises_with_deliveries_and_falls_with_failures() {
        let nothing = Record::default();
        assert_eq!(nothing.trust(), 0.0);
        let proven = Record {
            delivered: 20,
            earned: Amount::units(100),
            failed: 0,
            released: 0,
        };
        let shaky = Record {
            delivered: 20,
            earned: Amount::units(100),
            failed: 3,
            released: 2,
        };
        let new = Record {
            delivered: 1,
            earned: Amount::units(5),
            failed: 0,
            released: 0,
        };
        assert!(proven.trust() > shaky.trust());
        assert!(
            proven.trust() > new.trust(),
            "a long clean record beats one task"
        );
        assert!(proven.trust() < 1.0 && proven.trust() > 0.5);
        // All failures: trust floors at zero, never negative.
        let bad = Record {
            delivered: 1,
            earned: Amount::ZERO,
            failed: 5,
            released: 0,
        };
        assert_eq!(bad.trust(), 0.0);
    }

    fn candidate(
        seed: &[u8],
        offer: &str,
        attested: bool,
        per_1k_output: Option<u64>,
    ) -> Candidate {
        Candidate {
            agent: agent(seed),
            account: account(seed),
            offer: offer.into(),
            per_1k_output: per_1k_output.map(|m| Amount(m.into())),
            attested,
        }
    }

    #[test]
    fn the_best_agent_fits_the_task_and_has_the_record() {
        let candidates = vec![
            candidate(
                b"a",
                "summarizes legal contracts and documents",
                true,
                Some(15_000),
            ),
            candidate(b"b", "writes and reviews rust code", true, Some(20_000)),
            candidate(b"c", "summarizes documents cheaply", true, Some(5_000)),
        ];
        let mut records = BTreeMap::new();
        // The first agent is proven; the third is new.
        records.insert(
            account(b"a"),
            Record {
                delivered: 30,
                earned: Amount::units(300),
                failed: 0,
                released: 0,
            },
        );
        let found = rank(
            "summarize this contract document",
            &candidates,
            &records,
            &Need::default(),
        );
        // The code writer does not fit and is dropped.
        assert!(
            found.iter().all(|m| m.candidate.agent != agent(b"b")),
            "{found:?}"
        );
        // The proven summarizer outranks the new cheap one.
        assert_eq!(found[0].candidate.agent, agent(b"a"));
        assert_eq!(found[1].candidate.agent, agent(b"c"));
        assert!(found[0].trust > found[1].trust);
    }

    #[test]
    fn records_add_up_deliveries_releases_and_failures_from_events() {
        use sion_core::ids::{ClaimId, Pulse};
        use sion_core::Amount;
        let coder = account(b"coder");
        let task = |seed: &[u8]| ObjectId::from_hash(sion_core::hash::hash_domain("t", seed));
        let result = ClaimId::from_hash(sion_core::hash::hash_domain("r", b"1"));
        let _ = Pulse(0);
        let events = [
            // Two tasks named for coder.
            Event::TaskPosted {
                task: task(b"1"),
                poster: account(b"lab"),
                worker: Some(coder),
                amount: Amount::units(5),
                deadline: Pulse(100),
            },
            Event::TaskPosted {
                task: task(b"2"),
                poster: account(b"lab"),
                worker: Some(coder),
                amount: Amount::units(3),
                deadline: Pulse(100),
            },
            // The first it delivered, the second it released.
            Event::TaskSettled {
                task: task(b"1"),
                worker: coder,
                result,
                amount: Amount::units(5),
                reservation: sion_core::hash::Hash32::ZERO,
            },
            Event::TaskCancelled {
                task: task(b"2"),
                refund: Amount::units(3),
                to: account(b"lab"),
            },
            // The delivered result was later proven false.
            Event::Falsified {
                claim: result,
                causes: Vec::new(),
            },
        ];
        let records = records_from(events.iter());
        let record = &records[&coder];
        assert_eq!(record.delivered, 1);
        assert_eq!(record.earned, Amount::units(5));
        assert_eq!(record.released, 1);
        assert_eq!(record.failed, 1);
        // A task cancelled with the refund going to the worker itself is
        // not a release.
        let events = [
            Event::TaskPosted {
                task: task(b"3"),
                poster: account(b"lab"),
                worker: Some(coder),
                amount: Amount::units(1),
                deadline: Pulse(100),
            },
            Event::TaskCancelled {
                task: task(b"3"),
                refund: Amount::units(1),
                to: coder,
            },
        ];
        assert_eq!(records_from(events.iter()).get(&coder), None);
    }

    #[test]
    fn a_search_keeps_to_what_is_needed() {
        let candidates = vec![
            candidate(b"a", "summarizes documents", true, Some(15_000)),
            candidate(b"b", "summarizes documents", false, Some(2_000)),
            candidate(b"c", "summarizes documents", true, Some(90_000)),
        ];
        let records = BTreeMap::new();
        // Only enclaves, at most 20000 per 1k output.
        let found = rank(
            "summarize documents",
            &candidates,
            &records,
            &Need {
                attested_only: true,
                max_per_1k_output: Some(Amount(20_000)),
            },
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].candidate.agent, agent(b"a"));
    }
}
