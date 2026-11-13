//! Receipts: which model answered each of a session's calls.
//!
//! Whoever pays for an agent's work pays, among other things, for the
//! model behind it, and a model that costs less can be put in its place
//! without the answer saying so. A runtime keeps a **receipt** of every
//! answer a model gives it ([`Receipt`]): the model it asked for, the
//! address it asked ([`crate::model::Model::endpoint`]), the model the
//! provider said had answered, the digests of what it sent and of what
//! came back, and the tokens counted. Each receipt holds the digest of
//! the one before, so the receipts of a session are a chain with one head
//! ([`Receipts`]): once the head is signed or committed to, no receipt can
//! be changed, dropped or put in without the head changing.
//!
//! What a receipt is worth is what its runtime's word is worth. Kept by a
//! runtime on its operator's own machine it is the operator's word, fixed
//! at the time. Signed by a runtime in an enclave with a key its
//! attestation binds, it is the word of the code that was measured: that
//! code asked that address, and was told that model answered. It is never
//! more than that: the provider's name for its model is the provider's to
//! give, and nothing here looks inside the model.

use serde::{Deserialize, Serialize};

use crate::model::{Request, Response, Usage};

/// The domain a receipt's digest is taken in.
pub const DOMAIN: &str = "sion-agent/receipt/v1";

/// The domain a request's digest is taken in.
pub const REQUEST_DOMAIN: &str = "sion-agent/receipt/request/v1";

/// The domain an answer's digest is taken in.
pub const RESPONSE_DOMAIN: &str = "sion-agent/receipt/response/v1";

/// The receipts a session keeps whole: the last of them. The count, the
/// head and the tallies cover every one.
pub const KEPT: usize = 200;

/// What stands for the receipt before the first.
pub fn origin() -> String {
    "0".repeat(64)
}

/// What a call was for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    /// A turn of the conversation: what the agent says or calls next.
    #[default]
    Turn,
    /// Checking a call against the operator's rules.
    Rules,
    /// Reading content from outside without tools.
    Reader,
    /// Summarising the conversation so far.
    Summary,
}

impl Purpose {
    fn is_turn(&self) -> bool {
        *self == Purpose::Turn
    }

    /// A word for it.
    pub fn word(&self) -> &'static str {
        match self {
            Purpose::Turn => "turn",
            Purpose::Rules => "rules",
            Purpose::Reader => "reader",
            Purpose::Summary => "summary",
        }
    }
}

/// A model's answer, as the runtime that asked for it kept it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    /// Its place in the session's receipts, from 0.
    pub seq: u64,
    /// The digest of the receipt before it ([`origin`] for the first).
    pub previous: String,
    /// What the call was for.
    #[serde(default, skip_serializing_if = "Purpose::is_turn")]
    pub purpose: Purpose,
    /// The model the runtime asked for, as it is configured.
    pub asked: String,
    /// The address it asked: scheme and host. Empty for a model that is
    /// not asked over a network.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub endpoint: String,
    /// The model the provider said answered, or the one asked for where it
    /// did not say.
    pub answered: String,
    /// The digest of what was sent ([`request_digest`]).
    pub request: String,
    /// The digest of what came back ([`response_digest`]).
    pub response: String,
    /// The tokens the provider counted.
    pub usage: Usage,
}

impl Receipt {
    /// Its digest: of its JSON, as this type writes it.
    pub fn digest(&self) -> String {
        let bytes = serde_json::to_vec(self).unwrap_or_default();
        sion_core::hash::hash_domain(DOMAIN, &bytes).to_hex()
    }

    /// The receipt in a line.
    pub fn line(&self) -> String {
        let model = if self.answered == self.asked {
            self.asked.clone()
        } else {
            format!("{} (asked for {})", self.answered, self.asked)
        };
        let at = if self.endpoint.is_empty() {
            String::new()
        } else {
            format!(" at {}", self.endpoint)
        };
        let purpose = if self.purpose.is_turn() {
            String::new()
        } else {
            format!(" ({})", self.purpose.word())
        };
        format!(
            "{}: {model}{at}{purpose}, {} in and {} out",
            self.seq, self.usage.input, self.usage.output
        )
    }
}

/// The digest of a request as the runtime made it: its system prompt,
/// conversation, tools and limit.
pub fn request_digest(request: &Request) -> String {
    let bytes = serde_json::to_vec(request).unwrap_or_default();
    sion_core::hash::hash_domain(REQUEST_DOMAIN, &bytes).to_hex()
}

/// The digest of an answer: what the model said and why it stopped.
pub fn response_digest(response: &Response) -> String {
    let bytes = serde_json::to_vec(&(&response.content, &response.stop)).unwrap_or_default();
    sion_core::hash::hash_domain(RESPONSE_DOMAIN, &bytes).to_hex()
}

/// The scheme and host (with its port, if any) of `base`, in lower case:
/// what a receipt says of where a model was asked.
pub fn endpoint_of(base: &str) -> Option<String> {
    let (scheme, rest) = base.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    (!host.is_empty() && !scheme.is_empty()).then(|| {
        format!(
            "{}://{}",
            scheme.to_ascii_lowercase(),
            host.to_ascii_lowercase()
        )
    })
}

/// How often a model answered for a model asked at an address, and the
/// tokens that came to.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tally {
    /// The model asked for.
    pub asked: String,
    /// The address asked.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub endpoint: String,
    /// The model that answered.
    pub answered: String,
    /// The answers.
    pub calls: u64,
    /// Their tokens.
    pub usage: Usage,
}

impl Tally {
    /// The tally in a line.
    pub fn line(&self) -> String {
        let model = if self.answered == self.asked {
            self.asked.clone()
        } else {
            format!("{} (asked for {})", self.answered, self.asked)
        };
        let at = if self.endpoint.is_empty() {
            String::new()
        } else {
            format!(" at {}", self.endpoint)
        };
        format!(
            "{model}{at}: {} answer(s), {} tokens in and {} out",
            self.calls, self.usage.input, self.usage.output
        )
    }
}

/// Adds `receipt` to `tallies`.
fn tally(tallies: &mut Vec<Tally>, receipt: &Receipt) {
    let found = tallies.iter_mut().find(|t| {
        t.asked == receipt.asked && t.endpoint == receipt.endpoint && t.answered == receipt.answered
    });
    let entry = match found {
        Some(entry) => entry,
        None => {
            tallies.push(Tally {
                asked: receipt.asked.clone(),
                endpoint: receipt.endpoint.clone(),
                answered: receipt.answered.clone(),
                ..Tally::default()
            });
            tallies.last_mut().expect("just pushed")
        }
    };
    entry.calls += 1;
    entry.usage.add(&receipt.usage);
}

/// A session's receipts: a chain with one head.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipts {
    /// How many there are.
    #[serde(default)]
    pub count: u64,
    /// The digest of the last ([`origin`] with none; empty reads as that).
    #[serde(default)]
    pub head: String,
    /// Every answer, by the model asked, the address and the model that
    /// answered.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tallies: Vec<Tally>,
    /// The last [`KEPT`] receipts, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept: Vec<Receipt>,
}

impl Receipts {
    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The digest of the last receipt, or [`origin`] with none.
    pub fn head(&self) -> String {
        if self.head.is_empty() {
            origin()
        } else {
            self.head.clone()
        }
    }

    /// Keeps the receipt of `response`, the answer to `request` of the
    /// model asked for as `asked` at `endpoint`. Returns it.
    pub fn add(
        &mut self,
        purpose: Purpose,
        asked: &str,
        endpoint: Option<String>,
        request: &Request,
        response: &Response,
    ) -> &Receipt {
        let answered = if response.model.is_empty() {
            asked.to_string()
        } else {
            response.model.clone()
        };
        self.push(Receipt {
            seq: 0,
            previous: String::new(),
            purpose,
            asked: asked.to_string(),
            endpoint: endpoint.unwrap_or_default(),
            answered,
            request: request_digest(request),
            response: response_digest(response),
            usage: response.usage,
        })
    }

    /// Adds `receipt` at the end of the chain, with the place and link it
    /// has there, whatever it had where it came from: how one chain takes
    /// the receipts of another's turn.
    pub fn push(&mut self, mut receipt: Receipt) -> &Receipt {
        receipt.seq = self.count;
        receipt.previous = self.head();
        self.count += 1;
        self.head = receipt.digest();
        tally(&mut self.tallies, &receipt);
        self.kept.push(receipt);
        if self.kept.len() > KEPT {
            let over = self.kept.len() - KEPT;
            self.kept.drain(..over);
        }
        self.kept.last().expect("just pushed")
    }

    /// The receipts from place `from` on, if they are all kept.
    pub fn since(&self, from: u64) -> Option<&[Receipt]> {
        let first = self.kept.first().map_or(self.count, |r| r.seq);
        if from < first || from > self.count {
            return None;
        }
        Some(&self.kept[(from - first) as usize..])
    }

    /// Checks that the receipts kept are the end of a chain of `count`
    /// with this head: in order, each holding the digest of the one before,
    /// the last one's digest the head; and that the tallies count them,
    /// exactly where every receipt is kept and at least where older ones
    /// were let go. An error says what does not hold.
    pub fn check(&self) -> Result<(), String> {
        check(self.count, &self.head(), &self.tallies, &self.kept)
    }
}

/// [`Receipts::check`], of parts held apart.
pub fn check(count: u64, head: &str, tallies: &[Tally], kept: &[Receipt]) -> Result<(), String> {
    if kept.len() as u64 > count {
        return Err(format!(
            "{} receipts are kept of a chain said to hold {count}",
            kept.len()
        ));
    }
    if count == 0 {
        return if head == origin() && tallies.is_empty() {
            Ok(())
        } else {
            Err("a chain of no receipts has a head or tallies".into())
        };
    }
    let Some(last) = kept.last() else {
        return Err("no receipt is kept, so the head rests on nothing shown".into());
    };
    let first = count - kept.len() as u64;
    let mut previous: Option<String> = (first == 0).then(origin);
    for (at, receipt) in kept.iter().enumerate() {
        if receipt.seq != first + at as u64 {
            return Err(format!(
                "receipt {} stands where receipt {} belongs",
                receipt.seq,
                first + at as u64
            ));
        }
        if previous.as_ref().is_some_and(|p| *p != receipt.previous) {
            return Err(format!(
                "receipt {} does not hold the digest of the one before it",
                receipt.seq
            ));
        }
        previous = Some(receipt.digest());
    }
    if last.digest() != head {
        return Err("the head is not the digest of the last receipt".into());
    }
    let mut counted = Vec::new();
    for receipt in kept {
        tally(&mut counted, receipt);
    }
    let all_kept = first == 0;
    for had in &counted {
        let said = tallies.iter().find(|t| {
            t.asked == had.asked && t.endpoint == had.endpoint && t.answered == had.answered
        });
        let holds = said.is_some_and(|said| {
            if all_kept {
                said.calls == had.calls && said.usage == had.usage
            } else {
                said.calls >= had.calls
                    && said.usage.input >= had.usage.input
                    && said.usage.output >= had.usage.output
            }
        });
        if !holds {
            return Err(format!(
                "the tallies do not count the receipts of {} answering for {}",
                had.answered, had.asked
            ));
        }
    }
    let said: u64 = tallies.iter().map(|t| t.calls).sum();
    if said != count || (all_kept && tallies.len() != counted.len()) {
        return Err(format!(
            "the tallies count {said} answer(s) and the chain holds {count}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Block, Message, Stop};

    fn asked(text: &str) -> Request {
        Request {
            system: "You keep the books.".into(),
            messages: vec![Message::user(text)],
            tools: Vec::new(),
            max_tokens: 100,
            images: Default::default(),
        }
    }

    fn answer(model: &str, text: &str, input: u64, output: u64) -> Response {
        Response {
            content: vec![Block::text(text)],
            stop: Stop::EndTurn,
            usage: Usage {
                input,
                output,
                ..Usage::default()
            },
            model: model.into(),
            forgot_reasoning: false,
        }
    }

    #[test]
    fn receipts_are_a_chain_that_no_change_leaves_whole() {
        let mut receipts = Receipts::default();
        assert!(receipts.is_empty() && receipts.check().is_ok());
        assert_eq!(receipts.head(), origin());
        let at = Some("https://api.example.com".to_string());
        receipts.add(
            Purpose::Turn,
            "big-1",
            at.clone(),
            &asked("one"),
            &answer("big-1-2026", "1", 10, 2),
        );
        receipts.add(
            Purpose::Turn,
            "big-1",
            at.clone(),
            &asked("two"),
            &answer("small-1", "2", 12, 3),
        );
        // A model that does not say which it is counts as the one asked.
        receipts.add(
            Purpose::Summary,
            "big-1",
            None,
            &asked("three"),
            &answer("", "3", 5, 1),
        );
        assert_eq!(receipts.count, 3);
        assert!(receipts.check().is_ok());
        assert_eq!(receipts.kept[1].previous, receipts.kept[0].digest());
        assert_eq!(receipts.head(), receipts.kept[2].digest());
        assert_eq!(
            receipts.kept[1].line(),
            "1: small-1 (asked for big-1) at https://api.example.com, 12 in and 3 out"
        );
        assert_eq!(
            receipts.kept[2].line(),
            "2: big-1 (summary), 5 in and 1 out"
        );
        assert_eq!(receipts.tallies.len(), 3);
        assert_eq!(
            receipts.tallies[1].line(),
            "small-1 (asked for big-1) at https://api.example.com: 1 answer(s), 12 tokens in and 3 \
             out"
        );
        // The same words asked or answered give the same digests, and
        // others give others.
        assert_eq!(request_digest(&asked("one")), receipts.kept[0].request);
        assert_ne!(request_digest(&asked("One")), receipts.kept[0].request);
        assert_eq!(
            response_digest(&answer("anything", "1", 0, 0)),
            receipts.kept[0].response
        );

        // The model that answered changed afterwards: the chain breaks.
        let mut changed = receipts.clone();
        changed.kept[1].answered = "big-1-2026".into();
        assert!(changed.check().is_err());
        // A receipt dropped, or the order changed.
        let mut dropped = receipts.clone();
        dropped.kept.remove(1);
        assert!(dropped.check().is_err());
        let mut swapped = receipts.clone();
        swapped.kept.swap(0, 1);
        assert!(swapped.check().is_err());
        // The tallies say otherwise than the receipts.
        let mut miscounted = receipts.clone();
        miscounted.tallies[0].usage.input = 1;
        assert!(miscounted.check().is_err());
        let mut hidden = receipts.clone();
        hidden.tallies.remove(1);
        assert!(hidden.check().is_err());
    }

    #[test]
    fn a_long_chain_keeps_its_last_receipts_and_counts_them_all() {
        let mut receipts = Receipts::default();
        for n in 0..(KEPT as u64 + 5) {
            receipts.add(
                Purpose::Turn,
                "big-1",
                None,
                &asked(&n.to_string()),
                &answer("big-1", "ok", 1, 1),
            );
        }
        assert_eq!(receipts.count, KEPT as u64 + 5);
        assert_eq!(receipts.kept.len(), KEPT);
        assert_eq!(receipts.kept[0].seq, 5);
        assert_eq!(receipts.tallies[0].calls, KEPT as u64 + 5);
        assert!(receipts.check().is_ok());
        assert!(receipts.since(4).is_none());
        assert_eq!(receipts.since(KEPT as u64 + 3).unwrap().len(), 2);
        assert_eq!(receipts.since(KEPT as u64 + 5).unwrap().len(), 0);
        // One chain takes another's receipts with places and links of its
        // own.
        let mut other = Receipts::default();
        for receipt in receipts.since(KEPT as u64 + 3).unwrap() {
            other.push(receipt.clone());
        }
        assert_eq!((other.count, other.kept[0].seq), (2, 0));
        assert_eq!(other.kept[0].previous, origin());
        assert!(other.check().is_ok());
    }

    #[test]
    fn an_endpoint_is_a_scheme_and_a_host() {
        assert_eq!(
            endpoint_of("https://API.Example.com/v1/messages?x=1").as_deref(),
            Some("https://api.example.com")
        );
        assert_eq!(
            endpoint_of("http://user:key@localhost:8080").as_deref(),
            Some("http://localhost:8080")
        );
        assert_eq!(endpoint_of("api.example.com"), None);
        assert_eq!(endpoint_of("https:///v1"), None);
    }
}
