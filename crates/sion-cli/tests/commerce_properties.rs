//! Properties of the commerce code that must hold for every input, not
//! only the examples: canonical JSON, idempotency fingerprints, webhook
//! signatures, request signatures, AP2's recurrence periods and its exact
//! amounts.

use proptest::prelude::*;
use serde_json::{json, Map, Number, Value};
use sion_cli::acp::http;
use sion_cli::acp::signing::{self, Signer};
use sion_cli::acp::webhooks;
use sion_cli::ap2;

/// JSON values: nulls, booleans, integers, finite doubles, strings with
/// characters that need escaping and ones outside the BMP, arrays and
/// objects, nested.
fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|i| json!(i)),
        any::<u64>().prop_map(|u| json!(u)),
        any::<f64>()
            .prop_filter("finite", |f| f.is_finite())
            .prop_map(|f| Value::Number(Number::from_f64(f).expect("finite"))),
        "[a-z\"\\\\\\x00-\\x1fé€😀]{0,8}".prop_map(Value::String),
    ];
    leaf.prop_recursive(3, 24, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..5).prop_map(Value::Array),
            prop::collection::btree_map("[a-zA-Z0-9€😀\\x{80}\\r]{0,6}", inner, 0..5)
                .prop_map(|m| Value::Object(m.into_iter().collect::<Map<String, Value>>())),
        ]
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// The canonical form is JSON that reads back as the same value, and is
    /// its own canonical form.
    #[test]
    fn canonical_json_reads_back_and_is_a_fixed_point(value in json_value()) {
        let canonical = signing::canonical(&value);
        let read: Value = serde_json::from_str(&canonical).expect("canonical JSON parses");
        prop_assert_eq!(signing::canonical(&read), canonical.clone());
        // A value without doubles, or integers beyond 2^53 (which RFC 8785
        // writes as doubles, as JavaScript does), reads back as itself.
        if !has_double(&value) && !has_big_integer(&value) {
            prop_assert_eq!(&read, &value);
        }
        prop_assert!(!canonical.contains('\n'), "no whitespace outside strings");
    }

    /// Bodies that are the same JSON value have the same idempotency
    /// fingerprint, however they were written: `1.0` is `1`.
    #[test]
    fn a_fingerprint_is_of_the_value_not_its_text(value in json_value()) {
        let pretty: Value = serde_json::from_str(&serde_json::to_string_pretty(&value).unwrap()).unwrap();
        prop_assert_eq!(http::fingerprint(&value), http::fingerprint(&pretty));
        let canonical: Value = serde_json::from_str(&signing::canonical(&value)).unwrap();
        prop_assert_eq!(http::fingerprint(&value), http::fingerprint(&canonical));
    }

    /// A webhook verifies with its secret, at its time, for its body, and
    /// for no other body, secret or time far from it.
    #[test]
    fn a_webhook_signature_binds_body_secret_and_time(
        body in "[ -~]{0,64}",
        other in "[ -~]{0,64}",
        secret in prop::collection::vec(any::<u8>(), 16..48),
        at in 1_000_000_000u64..4_000_000_000,
        late in 0u64..2 * webhooks::TOLERANCE,
    ) {
        let header = webhooks::signature(&secret, at, &body);
        let checked = webhooks::verify(&header, &body, &secret, at + late);
        prop_assert_eq!(checked.is_ok(), late <= webhooks::TOLERANCE);
        if other != body {
            prop_assert!(webhooks::verify(&header, &other, &secret, at).is_err());
        }
        let mut wrong = secret.clone();
        wrong[0] ^= 1;
        prop_assert!(webhooks::verify(&header, &body, &wrong, at).is_err());
    }

    /// A request signed by one profile verifies for the same value written
    /// any way (ES256 over its canonical form), and not for another value.
    #[test]
    fn a_request_signature_is_over_the_canonical_body(
        value in json_value(),
        changed in json_value(),
        at in 1_000_000_000u64..4_000_000_000,
    ) {
        let signer = Signer::Es256(ap2::SigningKey::generate());
        let body = value.to_string();
        let (signature, timestamp) = signer.sign(&body, at);
        let verifier = signer.verifier();
        let pretty = serde_json::to_string_pretty(&value).unwrap();
        prop_assert!(verifier.check(&signature, &timestamp, &pretty, at).is_ok());
        if signing::canonical(&changed) != signing::canonical(&value) {
            prop_assert!(verifier.check(&signature, &timestamp, &changed.to_string(), at).is_err());
        }
        let hmac = Signer::hmac(b"a shared signing secret, 32 bytes");
        let (tag, seconds) = hmac.sign(&body, at);
        prop_assert!(hmac.verifier().check(&tag, &seconds, &body, at).is_ok());
        prop_assert!(hmac.verifier().check(&tag, &seconds, &body, at + signing::SKEW + 1).is_err());
    }

    /// A recurring mandate's next payment is never before its last, and a
    /// later last payment never allows an earlier next one.
    #[test]
    fn recurrence_periods_move_forward(
        last in 0u64..4_000_000_000,
        later in 0u64..100_000_000,
        frequency in prop::sample::select(vec!["ON_DEMAND", "DAILY", "WEEKLY", "BIWEEKLY", "MONTHLY", "QUARTERLY", "ANNUALLY"]),
    ) {
        let next = ap2::next_allowed(last, frequency).expect("a known frequency");
        prop_assert!(next >= last);
        if frequency != "ON_DEMAND" {
            prop_assert!(next > last);
        }
        let next_later = ap2::next_allowed(last + later, frequency).expect("a known frequency");
        prop_assert!(next_later >= next);
        // Calendar periods are at least as long as their shortest month,
        // quarter or year.
        let shortest = match frequency {
            "DAILY" => 86_400,
            "WEEKLY" => 7 * 86_400,
            "BIWEEKLY" => 14 * 86_400,
            "MONTHLY" => 28 * 86_400,
            "QUARTERLY" => 89 * 86_400,
            "ANNUALLY" => 365 * 86_400,
            _ => 0,
        };
        prop_assert!(next - last >= shortest);
    }

    /// An amount in major units reads as exactly its minor units, written
    /// as a string or a number; one finer than the currency's minor unit is
    /// refused.
    #[test]
    fn major_units_read_exactly(cents in 0u64..10_000_000_000) {
        let text = format!("{}.{:02}", cents / 100, cents % 100);
        prop_assert_eq!(ap2::minor_units(&json!(text), "USD"), Some(cents));
        let number: Value = serde_json::from_str(&text).unwrap();
        prop_assert_eq!(ap2::minor_units(&number, "USD"), Some(cents));
        let finer = format!("{text}1");
        prop_assert_eq!(ap2::minor_units(&json!(finer), "USD"), None);
    }
}

fn has_double(value: &Value) -> bool {
    match value {
        Value::Number(n) => n.is_f64(),
        Value::Array(items) => items.iter().any(has_double),
        Value::Object(map) => map.values().any(has_double),
        _ => false,
    }
}

fn has_big_integer(value: &Value) -> bool {
    match value {
        Value::Number(n) => {
            n.as_u64().is_some_and(|u| u > 1 << 53)
                || n.as_i64().is_some_and(|i| i.unsigned_abs() > 1 << 53)
        }
        Value::Array(items) => items.iter().any(has_big_integer),
        Value::Object(map) => map.values().any(has_big_integer),
        _ => false,
    }
}
