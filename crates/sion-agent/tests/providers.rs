//! The providers against local stand-ins for the APIs: what is sent, what is
//! read back, and how errors map.

use std::sync::mpsc;
use std::thread;

use serde_json::{json, Value};
use sion_agent::model::{Block, Message, Model, ModelError, Request, Role, Stop, ToolSpec};
use sion_agent::providers::{Anthropic, OpenAiCompatible};

fn client() -> sion_http::Client {
    sion_http::Client::new(sion_http::Options {
        allow_private: true,
        proxy_from_env: false,
        ..sion_http::Options::default()
    })
    .unwrap()
}

/// A request as the stand-in saw it: its headers and its JSON body.
type Seen = (Vec<(String, String)>, Value);

/// Serves one request: sends its headers and body back over the channel and
/// answers with `status` and `answer`.
fn serve_once(status: u16, answer: Value) -> (String, mpsc::Receiver<Seen>) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut request = server.recv().unwrap();
        let headers = request
            .headers()
            .iter()
            .map(|h| {
                (
                    h.field.as_str().as_str().to_ascii_lowercase(),
                    h.value.as_str().to_string(),
                )
            })
            .collect();
        let mut body = String::new();
        request.as_reader().read_to_string(&mut body).unwrap();
        tx.send((headers, serde_json::from_str(&body).unwrap()))
            .unwrap();
        let response = tiny_http::Response::from_string(answer.to_string())
            .with_status_code(status)
            .with_header(
                "Content-Type: application/json"
                    .parse::<tiny_http::Header>()
                    .unwrap(),
            );
        request.respond(response).unwrap();
    });
    (base, rx)
}

fn conversation() -> Request {
    Request {
        system: "You keep the books.".into(),
        messages: vec![
            Message::user("What is my balance?"),
            Message {
                role: Role::Assistant,
                content: vec![
                    Block::text("Checking."),
                    Block::ToolUse {
                        id: "toolu_1".into(),
                        name: "balance".into(),
                        input: json!({"asset": "USD"}),
                    },
                ],
            },
            Message {
                role: Role::User,
                content: vec![Block::ToolResult {
                    id: "toolu_1".into(),
                    content: "12.5 USD".into(),
                    is_error: false,
                    images: Vec::new(),
                }],
            },
        ],
        tools: vec![ToolSpec {
            name: "balance".into(),
            description: "The account's balance".into(),
            input_schema: json!({"type": "object", "properties": {"asset": {"type": "string"}}}),
        }],
        max_tokens: 512,
        images: Default::default(),
    }
}

/// A conversation of four screenshots, the bytes of all of them kept.
fn screenshots() -> Request {
    let mut request = conversation();
    request.messages.truncate(1);
    for i in 0..4 {
        let data = format!("cGljdHVyZS{i}=");
        let digest = sion_agent::model::image_digest("image/png", &data);
        request.images.insert(digest.clone(), data);
        request.messages.push(Message {
            role: Role::Assistant,
            content: vec![Block::ToolUse {
                id: format!("toolu_{i}"),
                name: "screenshot".into(),
                input: json!({}),
            }],
        });
        request.messages.push(Message {
            role: Role::User,
            content: vec![Block::ToolResult {
                id: format!("toolu_{i}"),
                content: format!("screen {i}"),
                is_error: false,
                images: vec![sion_agent::model::Image {
                    media_type: "image/png".into(),
                    digest,
                }],
            }],
        });
    }
    request
}

#[test]
fn pictures_go_to_both_providers_the_last_three_shown() {
    let request = screenshots();
    let anthropic = Anthropic::new(client(), "claude-opus-5-5", "k").body(&request);
    let results: Vec<&Value> = anthropic["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["content"][0]["type"] == "tool_result")
        .map(|m| &m["content"][0])
        .collect();
    assert_eq!(results.len(), 4);
    // The first picture is a line; the last three are pictures.
    assert_eq!(results[0]["content"][0]["text"], "screen 0");
    assert_eq!(results[0]["content"][1]["type"], "text");
    assert_eq!(
        results[0]["content"][1]["text"],
        sion_agent::providers::IMAGE_NOT_SHOWN
    );
    for (i, result) in results.iter().enumerate().skip(1) {
        let picture = &result["content"][1];
        assert_eq!(picture["type"], "image");
        assert_eq!(picture["source"]["media_type"], "image/png");
        assert_eq!(picture["source"]["data"], format!("cGljdHVyZS{i}="));
    }

    let openai = OpenAiCompatible::new(client(), "http://127.0.0.1:1/v1", "m", None).body(&request);
    let messages = openai["messages"].as_array().unwrap();
    let tools: Vec<&Value> = messages.iter().filter(|m| m["role"] == "tool").collect();
    assert!(tools[0]["content"]
        .as_str()
        .unwrap()
        .ends_with(sion_agent::providers::IMAGE_NOT_SHOWN));
    // A tool message holds text: each shown picture follows its result in a
    // user message.
    let pictures: Vec<&Value> = messages
        .iter()
        .filter(|m| m["role"] == "user" && m["content"].is_array())
        .collect();
    assert_eq!(pictures.len(), 3);
    assert_eq!(
        pictures[2]["content"][1]["image_url"]["url"],
        "data:image/png;base64,cGljdHVyZS3="
    );

    // A picture whose bytes are let go is a line too, and the last three of
    // those kept are shown.
    let mut fewer = screenshots();
    let last = sion_agent::model::image_digest("image/png", "cGljdHVyZS3=");
    fewer.images.remove(&last);
    let body = Anthropic::new(client(), "claude-opus-5-5", "k").body(&fewer);
    let kinds: Vec<String> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["content"][0]["type"] == "tool_result")
        .map(|m| {
            m["content"][0]["content"][1]["type"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(kinds, ["image", "image", "image", "text"]);
}

#[test]
fn anthropic_sends_the_current_api_version_and_reads_tool_calls() {
    let (base, seen) = serve_once(
        200,
        json!({
            "id": "msg_1", "type": "message", "role": "assistant", "model": "claude-opus-5-5",
            "content": [
                {"type": "text", "text": "Paying now."},
                {"type": "tool_use", "id": "toolu_2", "name": "pay", "input": {"to": "bob", "amount": "1"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 321, "output_tokens": 45}
        }),
    );
    let mut provider = Anthropic::new(client(), "claude-opus-5-5", "test-key");
    provider.base = base;
    let response = provider.complete(&conversation()).unwrap();
    assert_eq!(response.stop, Stop::ToolUse);
    assert_eq!(response.usage.input, 321);
    assert!(
        matches!(&response.content[1], Block::ToolUse { name, input, .. }
        if name == "pay" && input["to"] == "bob")
    );

    let (headers, body) = seen.recv().unwrap();
    let header = |name: &str| {
        headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
    };
    assert_eq!(header("x-api-key").as_deref(), Some("test-key"));
    assert_eq!(header("anthropic-version").as_deref(), Some("2026-10-21"));
    assert_eq!(body["model"], "claude-opus-5-5");
    assert_eq!(body["system"], "You keep the books.");
    assert_eq!(body["cache_control"], json!({"type": "ephemeral"}));
    assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
    assert_eq!(body["messages"][1]["content"][1]["type"], "tool_use");
    assert_eq!(body["messages"][2]["content"][0]["tool_use_id"], "toolu_1");
    // Version 2026-10-21 refuses sampling parameters and forced tool choice
    // is refused by current models: neither is sent.
    for absent in ["temperature", "top_p", "top_k", "tool_choice"] {
        assert!(body.get(absent).is_none(), "{absent} was sent");
    }
}

#[test]
fn anthropic_reads_refusals_and_pauses() {
    for (reason, stop) in [
        ("refusal", Stop::Refusal),
        ("pause_turn", Stop::PauseTurn),
        ("max_tokens", Stop::MaxTokens),
        ("end_turn", Stop::EndTurn),
    ] {
        let answer = json!({"content": [], "stop_reason": reason, "usage": {}});
        assert_eq!(
            sion_agent::providers::anthropic_response(&answer)
                .unwrap()
                .stop,
            stop
        );
    }
}

#[test]
fn busy_answers_are_transient_and_bad_requests_are_not() {
    for (status, transient) in [
        (429, true),
        (529, true),
        (503, true),
        (400, false),
        (401, false),
    ] {
        let (base, _seen) = serve_once(
            status,
            json!({"type": "error", "error": {"type": "x", "message": "m"}}),
        );
        let mut provider = Anthropic::new(client(), "claude-opus-5-5", "k");
        provider.base = base;
        let error = provider.complete(&conversation()).unwrap_err();
        assert_eq!(error.transient(), transient, "HTTP {status}: {error}");
        if !transient {
            assert!(matches!(error, ModelError::Refused(_)));
        }
    }
}

#[test]
fn openai_compatible_maps_tool_calls_both_ways() {
    let (base, seen) = serve_once(
        200,
        json!({
            "model": "local-model",
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {"role": "assistant", "content": null, "tool_calls": [
                    {"id": "call_9", "type": "function", "function": {"name": "pay", "arguments": "{\"to\":\"bob\",\"amount\":\"2\"}"}}
                ]}
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 3}
        }),
    );
    let provider = OpenAiCompatible::new(
        client(),
        &format!("{base}/v1"),
        "local-model",
        Some("sk-local"),
    );
    let response = provider.complete(&conversation()).unwrap();
    assert_eq!(response.stop, Stop::ToolUse);
    assert!(
        matches!(&response.content[0], Block::ToolUse { id, input, .. }
        if id == "call_9" && input["amount"] == "2")
    );

    let (headers, body) = seen.recv().unwrap();
    assert!(headers
        .iter()
        .any(|(n, v)| n == "authorization" && v == "Bearer sk-local"));
    let roles: Vec<&str> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, vec!["system", "user", "assistant", "tool"]);
    assert_eq!(
        body["messages"][2]["tool_calls"][0]["function"]["arguments"],
        "{\"asset\":\"USD\"}"
    );
    assert_eq!(body["messages"][3]["tool_call_id"], "toolu_1");
    assert_eq!(body["tools"][0]["function"]["name"], "balance");
}
