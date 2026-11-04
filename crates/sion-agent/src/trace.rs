//! Sessions as trajectories: for review, evaluation and training.
//!
//! [`chat_line`] writes a session as one JSON line in the chat format with
//! tool calls that fine-tuning and reinforcement-learning tools read
//! (`{"messages": [...]}`), with the session's outcome alongside, so that
//! trajectories can be filtered by how they ended. [`digest`] commits to a
//! trajectory, so that it can be recorded where it cannot be rewritten, such
//! as the agent's memory log. [`otel`] writes a session's steps as
//! OpenTelemetry spans (OTLP's JSON), named as OpenTelemetry's
//! generative-AI conventions name them: those conventions are still at
//! their Development stage, so the names may change, and what they have no
//! name for (a call's effect and key, taint, the session's journal) is
//! given under `sion.`.

use serde_json::{json, Value};

use crate::model::Message;
use crate::providers::openai_messages;
use crate::session::{Ended, State};

/// The session as one JSON line: its messages in chat format, and metadata.
pub fn chat_line(state: &State, system: &str, ended: Option<&Ended>) -> String {
    let outcome = ended.map(|e| json!(e)).unwrap_or(Value::Null);
    let calls: usize = state.messages.iter().map(|m| m.tool_uses().count()).sum();
    json!({
        "messages": openai_messages(system, &state.messages, &state.images, ""),
        "metadata": {
            "session": state.id,
            "outcome": outcome,
            "turns": state.turns_taken,
            "tool_calls": calls,
            "compactions": state.compactions,
            "usage": {
                "input_tokens": state.usage.input,
                "output_tokens": state.usage.output,
                "cache_read_input_tokens": state.usage.cache_read,
                "cache_write_input_tokens": state.usage.cache_write,
            },
            "journal": state.journal.entries().len(),
        }
    })
    .to_string()
}

/// A commitment to a session's conversation (SHA3-256, domain-separated):
/// the same conversation always gives the same digest.
pub fn digest(state: &State) -> sion_core::hash::Hash32 {
    digest_of(&state.id, &state.messages)
}

/// The digest of the conversation `messages` of session `id`: a prefix of
/// a session's messages has the digest the session had when it held just
/// those.
pub fn digest_of(id: &str, messages: &[Message]) -> sion_core::hash::Hash32 {
    let body = serde_json::to_vec(messages).unwrap_or_default();
    sion_core::hash::hash_parts("sion-agent/trajectory/v1", &[id.as_bytes(), &body])
}

/// A span's or a trace's identifier, from the session and a label: hex, of
/// `bytes` bytes.
fn otel_id(session: &str, label: &str, bytes: usize) -> String {
    let digest = sion_core::hash::hash_parts(
        "sion-agent/otel-id/v1",
        &[session.as_bytes(), label.as_bytes()],
    );
    digest.to_hex()[..bytes * 2].to_string()
}

fn attribute(key: &str, value: Value) -> Value {
    let value = match value {
        Value::String(text) => json!({"stringValue": text}),
        Value::Bool(flag) => json!({"boolValue": flag}),
        Value::Number(n) if n.is_u64() || n.is_i64() => json!({"intValue": n.to_string()}),
        other => json!({"stringValue": other.to_string()}),
    };
    json!({"key": key, "value": value})
}

/// The session's steps as OpenTelemetry spans, OTLP's JSON
/// (`resourceSpans`): one span for the session (`invoke_agent`), and below
/// it one for each model asked (`chat`) and each call (`execute_tool`),
/// with the times, tokens and outcomes its trace kept ([`State::steps`]).
pub fn otel(state: &State, agent: &str, ended: Option<&Ended>) -> Value {
    let trace_id = otel_id(&state.id, "trace", 16);
    let root_id = otel_id(&state.id, "session", 8);
    let nanos = |ms: u64| (u128::from(ms) * 1_000_000).to_string();
    let mut spans = Vec::new();
    for (index, step) in state.steps.iter().enumerate() {
        let mut attributes = vec![attribute("gen_ai.conversation.id", json!(state.id))];
        let name = if step.kind == "model" {
            attributes.push(attribute("gen_ai.operation.name", json!("chat")));
            attributes.push(attribute("gen_ai.request.model", json!(step.name)));
            if let Some((input, output)) = step.tokens {
                attributes.push(attribute("gen_ai.usage.input_tokens", json!(input)));
                attributes.push(attribute("gen_ai.usage.output_tokens", json!(output)));
            }
            // Of the input, what the provider's cache gave and took, when
            // it said: as the GenAI conventions name them, on the call's
            // span only.
            if let Some((read, write)) = step.cached {
                if read > 0 {
                    attributes.push(attribute(
                        "gen_ai.usage.cache_read.input_tokens",
                        json!(read),
                    ));
                }
                if write > 0 {
                    attributes.push(attribute(
                        "gen_ai.usage.cache_write.input_tokens",
                        json!(write),
                    ));
                }
            }
            format!("chat {}", step.name)
        } else {
            attributes.push(attribute("gen_ai.operation.name", json!("execute_tool")));
            attributes.push(attribute("gen_ai.tool.name", json!(step.name)));
            if let Some(key) = &step.key {
                attributes.push(attribute("sion.call.key", json!(key)));
            }
            if let Some(effect) = step.effect {
                attributes.push(attribute("sion.effect", json!(effect)));
            }
            if let Some(outcome) = &step.outcome {
                attributes.push(attribute("sion.outcome", json!(outcome)));
            }
            format!("execute_tool {}", step.name)
        };
        attributes.push(attribute("sion.tainted", json!(step.tainted)));
        let failed = matches!(step.outcome.as_deref(), Some("error" | "unknown"));
        spans.push(json!({
            "traceId": trace_id,
            "spanId": otel_id(&state.id, &format!("step-{index}"), 8),
            "parentSpanId": root_id,
            "name": name,
            "kind": 3,
            "startTimeUnixNano": nanos(step.start_ms),
            "endTimeUnixNano": nanos(step.start_ms + step.ms),
            "attributes": attributes,
            "status": {"code": if failed { 2 } else { 1 }},
        }));
    }
    let start = state.steps.first().map_or(0, |s| s.start_ms);
    let end = state.steps.last().map_or(0, |s| s.start_ms + s.ms);
    let outcome = ended
        .map(|e| json!(e)["ended"].as_str().unwrap_or_default().to_string())
        .unwrap_or_default();
    let unknown = state.journal.unknown().count();
    let root = json!({
        "traceId": trace_id,
        "spanId": root_id,
        "name": format!("invoke_agent {agent}"),
        "kind": 1,
        "startTimeUnixNano": nanos(start),
        "endTimeUnixNano": nanos(end),
        "attributes": [
            attribute("gen_ai.operation.name", json!("invoke_agent")),
            attribute("gen_ai.agent.name", json!(agent)),
            attribute("gen_ai.conversation.id", json!(state.id)),
            attribute("gen_ai.usage.input_tokens", json!(state.usage.input)),
            attribute("gen_ai.usage.output_tokens", json!(state.usage.output)),
            attribute("sion.ended", json!(outcome)),
            attribute("sion.journal.entries", json!(state.journal.entries().len())),
            attribute("sion.journal.unknown", json!(unknown)),
        ],
        "status": {"code": if outcome == "done" { 1 } else { 0 }},
    });
    spans.insert(0, root);
    json!({"resourceSpans": [{
        "resource": {"attributes": [attribute("service.name", json!("sion"))]},
        "scopeSpans": [{
            "scope": {"name": "sion-agent", "version": env!("CARGO_PKG_VERSION")},
            "spans": spans,
        }],
    }]})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Step;

    #[test]
    fn steps_become_spans_under_the_session() {
        let mut state = State {
            id: "s1".into(),
            ..State::default()
        };
        state.steps.push(Step {
            kind: "model".into(),
            name: "claude-x".into(),
            start_ms: 1_000,
            ms: 20,
            tokens: Some((10, 5)),
            cached: Some((6, 0)),
            key: None,
            effect: None,
            outcome: None,
            tainted: false,
        });
        state.steps.push(Step {
            kind: "tool".into(),
            name: "ledger_pay".into(),
            start_ms: 1_030,
            ms: 7,
            tokens: None,
            cached: None,
            key: Some("k1".into()),
            effect: Some(crate::tools::Effect::Spend),
            outcome: Some("unknown".into()),
            tainted: true,
        });
        let ended = Ended::Done { text: "ok".into() };
        let otel = otel(&state, "lead", Some(&ended));
        let spans = &otel["resourceSpans"][0]["scopeSpans"][0]["spans"];
        assert_eq!(spans.as_array().unwrap().len(), 3);
        assert_eq!(spans[0]["name"], "invoke_agent lead");
        assert_eq!(spans[1]["name"], "chat claude-x");
        // What the cache gave is on the call's span; nothing was written.
        let chat = spans[1]["attributes"].as_array().unwrap();
        assert!(
            chat.contains(&json!({"key": "gen_ai.usage.cache_read.input_tokens",
            "value": {"intValue": "6"}}))
        );
        assert!(!chat
            .iter()
            .any(|a| a["key"] == "gen_ai.usage.cache_write.input_tokens"));
        assert_eq!(spans[1]["parentSpanId"], spans[0]["spanId"]);
        assert_eq!(spans[1]["startTimeUnixNano"], "1000000000");
        assert_eq!(spans[1]["endTimeUnixNano"], "1020000000");
        assert_eq!(spans[2]["name"], "execute_tool ledger_pay");
        assert_eq!(
            spans[2]["status"]["code"], 2,
            "an unknown outcome is not a success"
        );
        let attributes = spans[2]["attributes"].as_array().unwrap();
        assert!(
            attributes.contains(&json!({"key": "sion.effect", "value": {"stringValue": "spend"}}))
        );
        assert!(attributes.contains(&json!({"key": "sion.tainted", "value": {"boolValue": true}})));
        assert_eq!(spans[0]["traceId"].as_str().unwrap().len(), 32);
        assert_eq!(spans[0]["spanId"].as_str().unwrap().len(), 16);
    }
}
