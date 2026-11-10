//! What model calls cost, in US dollars: list prices per million tokens,
//! and the cost of a call from the tokens it used.
//!
//! Costs are kept in millionths of a dollar (micro-dollars), whole numbers,
//! so that adding many calls loses nothing. A price is per million tokens,
//! in micro-dollars: $3 a million tokens is `3_000_000`.
//!
//! The prices built in ([`Prices::builtin`]) are Anthropic's list prices for
//! Claude models as its pricing page gave them on 11 November 2026, with
//! prompt caching's five-minute writes (what the runtime asks for). Other
//! models are priced from a file ([`Prices::from_json`]); a model with no
//! price is counted apart ([`Cost::unpriced`]), never guessed.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::Usage;

/// A model's prices, in micro-dollars per million tokens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Price {
    /// Input tokens neither read from nor written to the cache.
    pub input: u64,
    /// Output tokens.
    pub output: u64,
    /// Input tokens read from the provider's cache.
    #[serde(default)]
    pub cache_read: u64,
    /// Input tokens written to the provider's cache.
    #[serde(default)]
    pub cache_write: u64,
}

/// A model's pricing: one price, or a second for long prompts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pricing {
    /// The price of a call.
    #[serde(flatten)]
    pub price: Price,
    /// A call whose prompt is longer than this many tokens (all its input,
    /// cache reads and writes included) pays [`Pricing::long`] instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_over: Option<u64>,
    /// The price of a call with a long prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long: Option<Price>,
}

impl Pricing {
    /// One price for every call.
    pub const fn flat(price: Price) -> Pricing {
        Pricing {
            price,
            long_over: None,
            long: None,
        }
    }

    /// The price a call with a prompt of `prompt` tokens pays.
    pub fn for_prompt(&self, prompt: u64) -> &Price {
        match (self.long_over, &self.long) {
            (Some(over), Some(long)) if prompt > over => long,
            _ => &self.price,
        }
    }

    /// What a call that used `usage` costs, in micro-dollars.
    pub fn cost(&self, usage: &Usage) -> u64 {
        let price = self.for_prompt(usage.input);
        let plain = usage
            .input
            .saturating_sub(usage.cache_read)
            .saturating_sub(usage.cache_write);
        let parts = [
            (plain, price.input),
            (usage.cache_read, price.cache_read),
            (usage.cache_write, price.cache_write),
            (usage.output, price.output),
        ];
        let total: u128 = parts
            .iter()
            .map(|(tokens, per_million)| u128::from(*tokens) * u128::from(*per_million))
            .sum();
        // Rounded up: a call never costs less than it is shown to.
        u64::try_from(total.div_ceil(1_000_000)).unwrap_or(u64::MAX)
    }

    /// The most a call may cost that sends about `input` tokens, none of
    /// them cached, and writes at most `output`.
    pub fn most(&self, input: u64, output: u64) -> u64 {
        self.cost(&Usage {
            input,
            output,
            cache_read: 0,
            cache_write: 0,
        })
    }

    /// Tokens of output that `micros` pays for: what a last answer may
    /// write within what is left.
    pub fn output_tokens_for(&self, micros: u64, prompt: u64) -> u64 {
        let per_million = self.for_prompt(prompt).output.max(1);
        u64::try_from(u128::from(micros) * 1_000_000 / u128::from(per_million)).unwrap_or(u64::MAX)
    }
}

/// Prices by model.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prices {
    /// Pricing by model name, as [`normalized`] gives it.
    pub models: BTreeMap<String, Pricing>,
}

/// $`dollars` and `cents` a million tokens, in micro-dollars.
const fn usd(dollars: u64, cents: u64) -> u64 {
    dollars * 1_000_000 + cents * 10_000
}

/// A price from base input and output prices, with five-minute cache
/// writes at 1.25 times the input price and reads at `read`.
const fn claude(input: u64, output: u64, read: u64) -> Price {
    Price {
        input,
        output,
        cache_read: read,
        cache_write: input + input / 4,
    }
}

/// The date of the prices built in.
pub const BUILTIN_AS_OF: &str = "2026-11-11";

impl Prices {
    /// Anthropic's list prices for Claude models (see the module's notes).
    pub fn builtin() -> Prices {
        let mut models = BTreeMap::new();
        let mut put = |names: &[&str], pricing: Pricing| {
            for name in names {
                models.insert((*name).to_string(), pricing);
            }
        };
        put(
            &["claude-fable-5-2", "claude-mythos-5-2"],
            Pricing::flat(claude(usd(8, 0), usd(40, 0), usd(0, 20))),
        );
        put(
            &["claude-opus-5-5"],
            Pricing::flat(claude(usd(4, 0), usd(20, 0), usd(0, 20))),
        );
        put(
            &["claude-sonnet-5-5"],
            Pricing::flat(claude(usd(2, 0), usd(10, 0), usd(0, 10))),
        );
        put(
            &["claude-haiku-5-5"],
            Pricing {
                price: claude(usd(0, 10), usd(0, 50), usd(0, 1)),
                long_over: Some(100_000),
                long: Some(claude(usd(0, 50), usd(2, 50), usd(0, 5))),
            },
        );
        put(
            &["claude-fable-5-1", "claude-mythos-5-1"],
            Pricing::flat(claude(usd(10, 0), usd(50, 0), usd(0, 25))),
        );
        put(
            &["claude-fable-5", "claude-mythos-5"],
            Pricing::flat(claude(usd(10, 0), usd(50, 0), usd(1, 0))),
        );
        put(
            &[
                "claude-opus-5",
                "claude-opus-4-8",
                "claude-opus-4-7",
                "claude-opus-4-6",
                "claude-opus-4-5",
            ],
            Pricing::flat(claude(usd(5, 0), usd(25, 0), usd(0, 50))),
        );
        put(
            &["claude-opus-4-1", "claude-opus-4", "claude-opus-4-0"],
            Pricing::flat(claude(usd(15, 0), usd(75, 0), usd(1, 50))),
        );
        put(
            &["claude-sonnet-5"],
            Pricing::flat(claude(usd(2, 0), usd(10, 0), usd(0, 20))),
        );
        put(
            &[
                "claude-sonnet-4-6",
                "claude-sonnet-4-5",
                "claude-sonnet-4",
                "claude-sonnet-4-0",
            ],
            Pricing::flat(claude(usd(3, 0), usd(15, 0), usd(0, 30))),
        );
        put(
            &["claude-haiku-4-5"],
            Pricing::flat(claude(usd(1, 0), usd(5, 0), usd(0, 10))),
        );
        Prices { models }
    }

    /// Prices from a JSON file: an object of model names, each with
    /// `input`, `output` and optionally `cache_read`, `cache_write`,
    /// `long_over` and `long`, in dollars a million tokens:
    ///
    /// ```json
    /// {"gpt-5.5": {"input": 1.25, "output": 10, "cache_read": 0.125}}
    /// ```
    pub fn from_json(text: &str) -> Result<Prices, String> {
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
        let object = value
            .as_object()
            .ok_or("prices are an object of model names")?;
        let mut models = BTreeMap::new();
        for (name, entry) in object {
            let price = |entry: &serde_json::Value| -> Result<Price, String> {
                let field = |key: &str, required: bool| -> Result<u64, String> {
                    match entry.get(key) {
                        None | Some(serde_json::Value::Null) if !required => Ok(0),
                        None | Some(serde_json::Value::Null) => {
                            Err(format!("{name}: no {key} price"))
                        }
                        Some(value) => dollars_to_micros(value)
                            .ok_or_else(|| format!("{name}: {key} is not a price in dollars")),
                    }
                };
                Ok(Price {
                    input: field("input", true)?,
                    output: field("output", true)?,
                    cache_read: field("cache_read", false)?,
                    cache_write: field("cache_write", false)?,
                })
            };
            let long_over = match entry.get("long_over") {
                None | Some(serde_json::Value::Null) => None,
                Some(n) => Some(
                    n.as_u64()
                        .ok_or_else(|| format!("{name}: long_over is a number of tokens"))?,
                ),
            };
            let long = match entry.get("long") {
                None | Some(serde_json::Value::Null) => None,
                Some(long) => Some(price(long)?),
            };
            if long_over.is_some() != long.is_some() {
                return Err(format!("{name}: long_over and long go together"));
            }
            models.insert(
                normalized(name),
                Pricing {
                    price: price(entry)?,
                    long_over,
                    long,
                },
            );
        }
        Ok(Prices { models })
    }

    /// These prices, with `other`'s in place of any for the same model.
    pub fn with(mut self, other: Prices) -> Prices {
        self.models.extend(other.models);
        self
    }

    /// The pricing of `model` (a provider's name for it, or a model's id),
    /// if there is one.
    pub fn of(&self, model: &str) -> Option<&Pricing> {
        self.models.get(&normalized(model))
    }
}

/// A price in dollars (a JSON number or a decimal string), in micro-dollars.
fn dollars_to_micros(value: &serde_json::Value) -> Option<u64> {
    let text = match value {
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.trim().trim_start_matches('$').to_string(),
        _ => return None,
    };
    parse_dollars(&text)
}

/// `"12.5"` dollars as 12,500,000 micro-dollars: at most six decimals,
/// never negative.
pub fn parse_dollars(text: &str) -> Option<u64> {
    let text = text.trim().trim_start_matches('$');
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    if !whole.chars().all(|c| c.is_ascii_digit()) || !fraction.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if fraction.len() > 6 {
        return None;
    }
    let whole: u64 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let fraction: u64 = if fraction.is_empty() {
        0
    } else {
        format!("{fraction:0<6}").parse().ok()?
    };
    whole.checked_mul(1_000_000)?.checked_add(fraction)
}

/// A model's name as prices are kept: without the provider before a colon
/// or a slash (`anthropic:`, `anthropic/`), the cloud's prefix
/// (`us.anthropic.`), a version after `@` (Vertex AI), a Bedrock version
/// (`-v1:0`) or a date (`-20261001`), in lower case.
pub fn normalized(model: &str) -> String {
    let mut name = model.trim().to_ascii_lowercase();
    if let Some((_, rest)) = name.rsplit_once('/') {
        name = rest.to_string();
    }
    if let Some((before, after)) = name.split_once(':') {
        // `provider:model`, or Bedrock's `model-v1:0`.
        name = if before.ends_with(|c: char| c.is_ascii_digit())
            && after.chars().all(|c| c.is_ascii_digit())
        {
            before.to_string()
        } else {
            after.to_string()
        };
    }
    if let Some(at) = name.find('@') {
        name.truncate(at);
    }
    if let Some(dot) = name.rfind("anthropic.") {
        name = name[dot + "anthropic.".len()..].to_string();
    }
    // A Bedrock version, `-v1` (its `:0` went above).
    if let Some(v) = name.rfind("-v") {
        if name[v + 2..].chars().all(|c| c.is_ascii_digit()) && v + 2 < name.len() {
            name.truncate(v);
        }
    }
    // A date: eight digits at the end.
    if let Some(dash) = name.rfind('-') {
        let tail = &name[dash + 1..];
        if tail.len() == 8 && tail.chars().all(|c| c.is_ascii_digit()) {
            name.truncate(dash);
        }
    }
    // Routers write Claude's versions with dots (`claude-opus-5.5`).
    if name.starts_with("claude-") {
        name = name.replace('.', "-");
    }
    name
}

/// What calls cost, those without a price apart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cost {
    /// Micro-dollars, for the calls with a price.
    pub micros: u64,
    /// The tokens of calls to models without a price.
    #[serde(default)]
    pub unpriced: Usage,
}

impl Cost {
    /// Adds a call to `model` that used `usage`, at `prices`.
    pub fn add_call(&mut self, prices: &Prices, model: &str, usage: &Usage) {
        match prices.of(model) {
            Some(pricing) => self.micros = self.micros.saturating_add(pricing.cost(usage)),
            None => self.unpriced.add(usage),
        }
    }

    /// Adds what `other` cost.
    pub fn add(&mut self, other: &Cost) {
        self.micros = self.micros.saturating_add(other.micros);
        self.unpriced.add(&other.unpriced);
    }

    /// Whether every call had a price.
    pub fn complete(&self) -> bool {
        self.unpriced.total() == 0
    }

    /// In words: `$0.37`, and the tokens without a price if any.
    pub fn shown(&self) -> String {
        let dollars = dollars(self.micros);
        match self.unpriced.total() {
            0 => dollars,
            n if self.micros == 0 => format!("unknown ({n} tokens of models without a price)"),
            n => format!("{dollars}, besides {n} tokens of models without a price"),
        }
    }
}

/// Micro-dollars as dollars: two decimals from a dollar up, four below a
/// cent, so that small runs do not show as nothing.
pub fn dollars(micros: u64) -> String {
    if micros >= 10_000 {
        let cents = micros.div_ceil(10_000);
        format!("${}.{:02}", cents / 100, cents % 100)
    } else {
        let tenths_of_mills = micros.div_ceil(100);
        format!(
            "${}.{:04}",
            tenths_of_mills / 10_000,
            tenths_of_mills % 10_000
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u64, output: u64, cache_read: u64, cache_write: u64) -> Usage {
        Usage {
            input,
            output,
            cache_read,
            cache_write,
        }
    }

    #[test]
    fn a_call_costs_its_tokens_at_their_prices() {
        let prices = Prices::builtin();
        let sonnet = prices.of("claude-sonnet-5-5").unwrap();
        // A million plain input tokens and a million output: $2 + $10.
        assert_eq!(sonnet.cost(&usage(1_000_000, 1_000_000, 0, 0)), 12_000_000);
        // Of 100,000 input tokens, 90,000 read from the cache and 5,000
        // written: 5,000 × $2 + 90,000 × $0.10 + 5,000 × $2.50, per million,
        // and 1,000 output tokens at $10.
        let cost = sonnet.cost(&usage(100_000, 1_000, 90_000, 5_000));
        assert_eq!(cost, 10_000 + 9_000 + 12_500 + 10_000);
        let opus = prices.of("claude-opus-5-5").unwrap();
        assert_eq!(opus.price.cache_write, usd(5, 0));
        assert_eq!(opus.price.cache_read, usd(0, 20));
    }

    #[test]
    fn a_long_prompt_pays_the_long_price() {
        let prices = Prices::builtin();
        let haiku = prices.of("claude-haiku-5-5").unwrap();
        assert_eq!(haiku.cost(&usage(100_000, 0, 0, 0)), 10_000);
        assert_eq!(haiku.cost(&usage(100_001, 0, 0, 0)), 50_001);
    }

    #[test]
    fn names_are_found_however_the_provider_writes_them() {
        for name in [
            "claude-opus-5-5",
            "anthropic:claude-opus-5-5",
            "anthropic/claude-opus-5-5",
            "claude-opus-5-5-20261001",
            "claude-opus-5-5@20261001",
            "us.anthropic.claude-opus-5-5-v1:0",
            "anthropic.claude-opus-5-5-v1:0",
            "Claude-Opus-5-5",
            "openrouter:anthropic/claude-opus-5.5",
        ] {
            assert_eq!(normalized(name), "claude-opus-5-5", "{name}");
        }
        assert_eq!(normalized("claude-opus-5"), "claude-opus-5");
        assert_eq!(normalized("openai:gpt-5.5"), "gpt-5.5");
        assert!(Prices::builtin().of("claude-opus-5-6").is_none());
    }

    #[test]
    fn a_file_adds_and_overrides_prices() {
        let file = Prices::from_json(
            r#"{"openai:gpt-5.5": {"input": 1.25, "output": "10", "cache_read": 0.125},
                "claude-opus-5-5": {"input": 3, "output": 15}}"#,
        )
        .unwrap();
        let prices = Prices::builtin().with(file);
        let gpt = prices.of("gpt-5.5").unwrap();
        assert_eq!(gpt.price.input, 1_250_000);
        assert_eq!(gpt.price.cache_read, 125_000);
        assert_eq!(prices.of("claude-opus-5-5").unwrap().price.input, 3_000_000);
        assert!(Prices::from_json(r#"{"m": {"input": 1}}"#).is_err());
        assert!(Prices::from_json(r#"{"m": {"input": -1, "output": 1}}"#).is_err());
        assert!(Prices::from_json(r#"{"m": {"input": 1, "output": 1, "long_over": 5}}"#).is_err());
    }

    #[test]
    fn dollars_parse_and_show() {
        assert_eq!(parse_dollars("12.5"), Some(12_500_000));
        assert_eq!(parse_dollars("$0.000001"), Some(1));
        assert_eq!(parse_dollars(".5"), Some(500_000));
        assert_eq!(parse_dollars("1.0000001"), None);
        assert_eq!(parse_dollars("-1"), None);
        assert_eq!(parse_dollars(""), None);
        assert_eq!(dollars(370_000), "$0.37");
        assert_eq!(dollars(12_345_678), "$12.35");
        assert_eq!(dollars(4_200), "$0.0042");
        assert_eq!(dollars(0), "$0.0000");
    }

    #[test]
    fn calls_without_a_price_are_counted_apart() {
        let prices = Prices::builtin();
        let mut cost = Cost::default();
        cost.add_call(&prices, "claude-sonnet-5-5", &usage(1_000_000, 0, 0, 0));
        cost.add_call(&prices, "scripted", &usage(10, 5, 0, 0));
        assert_eq!(cost.micros, 2_000_000);
        assert_eq!(cost.unpriced.total(), 15);
        assert!(!cost.complete());
        assert_eq!(
            cost.shown(),
            "$2.00, besides 15 tokens of models without a price"
        );
        let most = prices.of("claude-sonnet-5-5").unwrap().most(1_000, 1_000);
        assert_eq!(most, 2_000 + 10_000);
        assert_eq!(
            prices
                .of("claude-sonnet-5-5")
                .unwrap()
                .output_tokens_for(10_000, 10),
            1_000
        );
    }
}
