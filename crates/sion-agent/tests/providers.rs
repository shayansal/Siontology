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
                        signature: None,
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
                signature: None,
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

/// Serves one request per answer in `answers` (a status, a media type and a
/// body), in order: sends each request's headers and body back.
fn serve_each(answers: Vec<(u16, &'static str, String)>) -> (String, mpsc::Receiver<Seen>) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for (status, media, answer) in answers {
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
            let response = tiny_http::Response::from_string(answer)
                .with_status_code(status)
                .with_header(
                    format!("Content-Type: {media}")
                        .parse::<tiny_http::Header>()
                        .unwrap(),
                );
            request.respond(response).unwrap();
        }
    });
    (base, rx)
}

/// Server-Sent Events, each `(name, data)` an event: the name as its type
/// when it has one.
fn events(events: &[(&str, Value)]) -> String {
    events
        .iter()
        .map(|(name, data)| {
            if name.is_empty() {
                format!("data: {data}\n\n")
            } else {
                format!("event: {name}\ndata: {data}\n\n")
            }
        })
        .collect()
}

/// What a stream told, and the answer it made.
fn streamed(
    model: &dyn Model,
) -> (
    Vec<sion_agent::Delta>,
    Result<sion_agent::Response, ModelError>,
) {
    let mut told = Vec::new();
    let answer = model.stream(&conversation(), &mut |delta| {
        told.push(delta);
        true
    });
    (told, answer)
}

#[test]
fn anthropic_streams_the_text_and_calls_as_they_come_and_answers_as_whole() {
    use sion_agent::Delta;
    let sse = events(&[
        (
            "message_start",
            json!({"type": "message_start", "message": {"id": "msg_9", "model": "claude-opus-5-5",
                "content": [], "usage": {"input_tokens": 321, "output_tokens": 1}}}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": ""}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "The balance first."}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "c2ln"}}),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 0}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}),
        ),
        ("ping", json!({"type": "ping"})),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "Paying "}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "now."}}),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 1}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use",
                "id": "toolu_2", "name": "pay", "input": {}}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "{\"to\": \"bo"}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "b\", \"amount\": \"1\"}"}}),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 2}),
        ),
        // What a later version adds is let be.
        ("a_new_event", json!({"type": "a_new_event"})),
        (
            "message_delta",
            json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 45}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ]);
    let (base, seen) = serve_each(vec![(200, "text/event-stream", sse)]);
    let mut provider = Anthropic::new(client(), "claude-opus-5-5", "test-key");
    provider.base = base;
    let (told, answer) = streamed(&provider);
    assert_eq!(
        told,
        vec![
            Delta::Text("Paying ".into()),
            Delta::Text("now.".into()),
            Delta::Call("pay".into()),
        ],
        "the text and the call, as they came; thinking is not told"
    );
    let answer = answer.unwrap();
    // What the same answer whole reads as.
    let whole = sion_agent::providers::anthropic_response(&json!({
        "model": "claude-opus-5-5",
        "content": [
            {"type": "thinking", "thinking": "The balance first.", "signature": "c2ln"},
            {"type": "text", "text": "Paying now."},
            {"type": "tool_use", "id": "toolu_2", "name": "pay", "input": {"to": "bob", "amount": "1"}}
        ],
        "stop_reason": "tool_use",
        "usage": {"input_tokens": 321, "output_tokens": 45}
    }))
    .unwrap();
    assert_eq!(answer, whole);
    let (headers, body) = seen.recv().unwrap();
    assert_eq!(body["stream"], true);
    assert!(headers
        .iter()
        .any(|(n, v)| n == "accept" && v == "text/event-stream"));
}

#[test]
fn a_stream_that_fails_or_stops_partway_is_a_failure_that_may_pass() {
    let start = (
        "message_start",
        json!({"type": "message_start", "message": {"model": "m", "usage": {}}}),
    );
    let text = (
        "content_block_start",
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": "Half"}}),
    );
    let overloaded = (
        "error",
        json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}),
    );
    let refused = (
        "error",
        json!({"type": "error", "error": {"type": "invalid_request_error", "message": "no"}}),
    );
    let (base, _seen) = serve_each(vec![
        (
            200,
            "text/event-stream",
            events(&[start.clone(), text.clone(), overloaded]),
        ),
        (
            200,
            "text/event-stream",
            events(&[start.clone(), text.clone()]),
        ),
        (200, "text/event-stream", events(&[start, text, refused])),
    ]);
    let mut provider = Anthropic::new(client(), "m", "k");
    provider.base = base;
    let (told, answer) = streamed(&provider);
    assert_eq!(told, vec![sion_agent::Delta::Text("Half".into())]);
    assert!(
        matches!(answer, Err(ModelError::Busy(ref why)) if why.contains("overloaded_error")),
        "{answer:?}"
    );
    let (_, cut) = streamed(&provider);
    assert!(matches!(cut, Err(ModelError::Transport(_))), "{cut:?}");
    let (_, refused) = streamed(&provider);
    assert!(
        matches!(refused, Err(ModelError::Refused(_))),
        "{refused:?}"
    );
}

#[test]
fn openai_compatible_streams_the_text_and_calls_and_asks_for_the_usage() {
    use sion_agent::Delta;
    let chunk = |delta: Value, finish: Value| {
        json!({"id": "c1", "object": "chat.completion.chunk", "model": "local-model",
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]})
    };
    let sse = events(&[
        (
            "",
            chunk(json!({"role": "assistant", "content": ""}), Value::Null),
        ),
        ("", chunk(json!({"content": "Paying "}), Value::Null)),
        ("", chunk(json!({"content": "now."}), Value::Null)),
        (
            "",
            chunk(
                json!({"tool_calls": [{"index": 0, "id": "call_9", "type": "function",
                "function": {"name": "pay", "arguments": ""}}]}),
                Value::Null,
            ),
        ),
        (
            "",
            chunk(
                json!({"tool_calls": [{"index": 0, "function": {"arguments": "{\"to\":\"bob\","}}]}),
                Value::Null,
            ),
        ),
        (
            "",
            chunk(
                json!({"tool_calls": [{"index": 0, "function": {"arguments": "\"amount\":\"2\"}"}}]}),
                Value::Null,
            ),
        ),
        ("", chunk(json!({}), json!("tool_calls"))),
        (
            "",
            json!({"id": "c1", "object": "chat.completion.chunk", "model": "local-model", "choices": [],
                "usage": {"prompt_tokens": 10, "completion_tokens": 3}}),
        ),
    ]) + "data: [DONE]\n\n";
    let (base, seen) = serve_each(vec![(200, "text/event-stream", sse)]);
    let provider = OpenAiCompatible::new(
        client(),
        &format!("{base}/v1"),
        "local-model",
        Some("sk-local"),
    );
    let (told, answer) = streamed(&provider);
    assert_eq!(
        told,
        vec![
            Delta::Text("Paying ".into()),
            Delta::Text("now.".into()),
            Delta::Call("pay".into()),
        ]
    );
    let whole = sion_agent::providers::openai_response(&json!({
        "model": "local-model",
        "choices": [{"finish_reason": "tool_calls", "message": {"role": "assistant",
            "content": "Paying now.", "tool_calls": [{"id": "call_9", "type": "function",
            "function": {"name": "pay", "arguments": "{\"to\":\"bob\",\"amount\":\"2\"}"}}]}}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 3}
    }))
    .unwrap();
    assert_eq!(answer.unwrap(), whole);
    let (_, body) = seen.recv().unwrap();
    assert_eq!(body["stream"], true);
    assert_eq!(body["stream_options"], json!({"include_usage": true}));
}

#[test]
fn a_server_that_will_not_stream_is_read_whole_and_one_without_stream_options_is_asked_again() {
    // Refuses stream_options, then answers whole.
    let (base, seen) = serve_each(vec![
        (
            400,
            "application/json",
            json!({"error": {"message": "unknown field: stream_options"}}).to_string(),
        ),
        (
            200,
            "application/json",
            json!({"model": "local-model", "choices": [{"finish_reason": "stop",
                "message": {"role": "assistant", "content": "Whole."}}]})
            .to_string(),
        ),
    ]);
    let provider = OpenAiCompatible::new(client(), &format!("{base}/v1"), "local-model", None);
    let (told, answer) = streamed(&provider);
    assert_eq!(told, vec![sion_agent::Delta::Text("Whole.".into())]);
    assert_eq!(answer.unwrap().content, vec![Block::text("Whole.")]);
    let (_, first) = seen.recv().unwrap();
    let (_, second) = seen.recv().unwrap();
    assert!(first.get("stream_options").is_some());
    assert!(second.get("stream_options").is_none() && second["stream"] == true);
}

#[test]
fn a_stream_its_watcher_cuts_off_ends_there_with_the_text_so_far() {
    use sion_agent::Delta;
    // Anthropic's events: text, then a call; cut off after the first words.
    let sse = events(&[
        (
            "message_start",
            json!({"type": "message_start", "message": {"id": "msg_1", "model": "claude-opus-5-5",
                "content": [], "usage": {"input_tokens": 12, "output_tokens": 1}}}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Paying "}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Bob now."}}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "t1", "name": "pay", "input": {}}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ]);
    let mut stream =
        sion_agent::mcp_http::Events::new(std::io::Cursor::new(sse.into_bytes()), 1 << 16);
    let mut told = Vec::new();
    let answer = sion_agent::providers::anthropic_stream(&mut stream, &mut |delta| {
        told.push(delta);
        told.is_empty()
    })
    .unwrap();
    assert_eq!(told, vec![Delta::Text("Paying ".into())]);
    assert_eq!(answer.stop, Stop::Cut);
    assert_eq!(answer.content, vec![Block::text("Paying ")]);
    assert_eq!(answer.usage.input, 12);

    // Cut off at a call: the call, not finished, is not part of the answer.
    let mut told = Vec::new();
    let answer = sion_agent::providers::anthropic_stream(
        &mut sion_agent::mcp_http::Events::new(
            std::io::Cursor::new(events_of_a_paying_answer().into_bytes()),
            1 << 16,
        ),
        &mut |delta| {
            let calling = matches!(delta, Delta::Call(_));
            told.push(delta);
            !calling
        },
    )
    .unwrap();
    assert_eq!(answer.stop, Stop::Cut);
    assert_eq!(answer.content, vec![Block::text("Paying Bob now.")]);
    assert_eq!(told.last(), Some(&Delta::Call("pay".into())));

    // An OpenAI-compatible stream, cut off after its first chunk of text.
    let chunk = |delta: Value| {
        json!({"id": "c2", "object": "chat.completion.chunk", "model": "local-model",
            "choices": [{"index": 0, "delta": delta, "finish_reason": null}]})
    };
    let sse = events(&[
        ("", chunk(json!({"content": "Paying "}))),
        ("", chunk(json!({"content": "Bob now."}))),
    ]) + "data: [DONE]\n\n";
    let mut told = Vec::new();
    let answer = sion_agent::providers::openai_stream(
        &mut sion_agent::mcp_http::Events::new(std::io::Cursor::new(sse.into_bytes()), 1 << 16),
        &mut |delta| {
            told.push(delta);
            false
        },
    )
    .unwrap();
    assert_eq!(told.len(), 1);
    assert_eq!(
        (answer.stop, answer.content),
        (Stop::Cut, vec![Block::text("Paying ")])
    );
}

/// Anthropic's events for "Paying Bob now." and a call of `pay`.
fn events_of_a_paying_answer() -> String {
    events(&[
        (
            "message_start",
            json!({"type": "message_start", "message": {"id": "msg_2", "model": "claude-opus-5-5",
                "content": [], "usage": {"input_tokens": 12, "output_tokens": 1}}}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": "Paying Bob now."}}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "t1", "name": "pay", "input": {}}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "{\"to\":"}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ])
}

/// What a stand-in was sent: the path, the headers and the JSON body of each request.
type Requests = mpsc::Receiver<(String, Vec<(String, String)>, Value)>;

/// A stand-in that answers each request with the next of `answers`, and
/// keeps each request's URL, headers and body.
fn serve_urls(answers: Vec<(u16, &'static str, String)>) -> (String, Requests) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for (status, media, answer) in answers {
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
            tx.send((
                request.url().to_string(),
                headers,
                serde_json::from_str(&body).unwrap_or(Value::Null),
            ))
            .unwrap();
            let response = tiny_http::Response::from_string(answer)
                .with_status_code(status)
                .with_header(
                    format!("Content-Type: {media}")
                        .parse::<tiny_http::Header>()
                        .unwrap(),
                );
            request.respond(response).unwrap();
        }
    });
    (base, rx)
}

#[test]
fn gemini_is_asked_natively_and_its_signatures_go_back_with_its_calls() {
    use sion_agent::providers::{gemini_body, gemini_response, Gemini, GEMINI_NO_SIGNATURE};
    // A call Gemini made, with its signature, and one another model made.
    let mut request = conversation();
    request.messages.push(Message {
        role: Role::Assistant,
        content: vec![Block::ToolUse {
            id: "g1".into(),
            name: "balance".into(),
            input: json!({"asset": "EUR"}),
            signature: Some("c2lnbmVk".into()),
        }],
    });
    request.messages.push(Message {
        role: Role::User,
        content: vec![Block::ToolResult {
            id: "g1".into(),
            content: "no EUR account".into(),
            is_error: true,
            images: Vec::new(),
        }],
    });
    let body = gemini_body(&request);
    assert_eq!(
        body["systemInstruction"],
        json!({"parts": [{"text": "You keep the books."}]})
    );
    assert_eq!(
        body["generationConfig"]["maxOutputTokens"],
        request.max_tokens
    );
    assert_eq!(
        body["tools"][0]["functionDeclarations"][0],
        json!({"name": "balance", "description": "The account's balance",
            "parametersJsonSchema": {"type": "object", "properties": {"asset": {"type": "string"}}}})
    );
    let contents = body["contents"].as_array().unwrap();
    let roles: Vec<&str> = contents
        .iter()
        .map(|c| c["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["user", "model", "user", "model", "user"]);
    // Another model's call stands in a signature; Gemini's goes back as it
    // came.
    assert_eq!(
        contents[1]["parts"],
        json!([{"text": "Checking."},
            {"functionCall": {"id": "toolu_1", "name": "balance", "args": {"asset": "USD"}},
             "thoughtSignature": GEMINI_NO_SIGNATURE}])
    );
    assert_eq!(
        contents[2]["parts"],
        json!([{"functionResponse": {"id": "toolu_1", "name": "balance",
            "response": {"output": "12.5 USD"}}}])
    );
    assert_eq!(contents[3]["parts"][0]["thoughtSignature"], "c2lnbmVk");
    assert_eq!(
        contents[4]["parts"][0]["functionResponse"]["response"],
        json!({"error": "no EUR account"})
    );

    // An answer: its thinking left out, its text, then its call with its
    // signature; the thinking paid for as output.
    let answer = json!({
        "candidates": [{"content": {"role": "model", "parts": [
            {"text": "Let me think.", "thought": true},
            {"text": "Checking EUR."},
            {"functionCall": {"name": "balance", "args": {"asset": "EUR"}},
             "thoughtSignature": "bmV4dA=="}
        ]}, "finishReason": "STOP"}],
        "usageMetadata": {"promptTokenCount": 40, "candidatesTokenCount": 12, "thoughtsTokenCount": 30},
        "modelVersion": "gemini-3-pro-preview"
    });
    let read = gemini_response(&answer).unwrap();
    assert_eq!(read.stop, Stop::ToolUse);
    assert_eq!((read.usage.input, read.usage.output), (40, 42));
    assert_eq!(read.content[0], Block::text("Checking EUR."));
    assert!(
        matches!(&read.content[1], Block::ToolUse { name, signature: Some(s), .. }
        if name == "balance" && s == "bmV4dA==")
    );
    let blocked = gemini_response(&json!({"promptFeedback": {"blockReason": "SAFETY"}})).unwrap();
    assert_eq!(blocked.stop, Stop::Refusal);
    assert!(matches!(
        gemini_response(
            &json!({"error": {"code": 429, "status": "RESOURCE_EXHAUSTED", "message": "slow down"}})
        ),
        Err(ModelError::Busy(_))
    ));

    // Over HTTP, streamed: the text as it comes, then the call; the answer
    // what the whole one would have been.
    let chunk = |parts: Value, finish: Value| {
        json!({"candidates": [{"content": {"role": "model", "parts": parts}, "finishReason": finish}],
            "modelVersion": "gemini-3-pro-preview"})
    };
    let sse = events(&[
        ("", chunk(json!([{"text": "Checking "}]), Value::Null)),
        ("", chunk(json!([{"text": "EUR."}]), Value::Null)),
        (
            "",
            json!({"candidates": [{"content": {"role": "model", "parts": [
                {"functionCall": {"name": "balance", "args": {"asset": "EUR"}}, "thoughtSignature": "bmV4dA=="}
            ]}, "finishReason": "STOP"}],
                "usageMetadata": {"promptTokenCount": 40, "candidatesTokenCount": 12, "thoughtsTokenCount": 30}}),
        ),
    ]);
    let (base, seen) = serve_urls(vec![(200, "text/event-stream", sse)]);
    let mut provider = Gemini::new(client(), "gemini-3-pro-preview", "g-key");
    provider.base = base;
    let (told, answer) = streamed(&provider);
    assert_eq!(
        told,
        vec![
            sion_agent::Delta::Text("Checking ".into()),
            sion_agent::Delta::Text("EUR.".into()),
            sion_agent::Delta::Call("balance".into()),
        ]
    );
    let answer = answer.unwrap();
    assert_eq!(answer.stop, Stop::ToolUse);
    assert_eq!(answer.content.len(), 2);
    assert_eq!(answer.content[0], Block::text("Checking EUR."));
    assert_eq!((answer.usage.input, answer.usage.output), (40, 42));
    let (url, headers, sent) = seen.recv().unwrap();
    assert_eq!(
        url,
        "/v1beta/models/gemini-3-pro-preview:streamGenerateContent?alt=sse"
    );
    assert!(headers.contains(&("x-goog-api-key".into(), "g-key".into())));
    assert_eq!(
        sent["contents"][0]["parts"][0]["text"],
        "What is my balance?"
    );
}

#[test]
fn keys_are_used_in_turn_and_one_that_is_limited_or_refused_rests() {
    // The first key is limited, the second answers; then the second is used
    // first, the first resting; a refused key rests too.
    let ok =
        json!({"id": "msg_1", "type": "message", "role": "assistant", "model": "claude-opus-5-5",
        "content": [{"type": "text", "text": "Hi."}], "stop_reason": "end_turn",
        "usage": {"input_tokens": 3, "output_tokens": 1}})
        .to_string();
    let limited =
        json!({"type": "error", "error": {"type": "rate_limit_error", "message": "slow down"}})
            .to_string();
    let (base, seen) = serve_urls(vec![
        (429, "application/json", limited.clone()),
        (200, "application/json", ok.clone()),
        (200, "application/json", ok.clone()),
        (401, "application/json", "{}".into()),
        (200, "application/json", ok),
    ]);
    let mut provider = Anthropic::new(client(), "claude-opus-5-5", "key-a, key-b");
    provider.base = base;
    let key = |seen: &Requests| {
        let (_, headers, _) = seen.recv().unwrap();
        headers
            .into_iter()
            .find(|(name, _)| name == "x-api-key")
            .map(|(_, value)| value)
            .unwrap()
    };
    assert!(provider.complete(&conversation()).is_ok());
    assert_eq!(
        (key(&seen), key(&seen)),
        ("key-a".to_string(), "key-b".to_string())
    );
    // The next call starts with the next key, and key-a rests anyway.
    assert!(provider.complete(&conversation()).is_ok());
    assert_eq!(key(&seen), "key-b");
    // key-b refused: key-a, resting, is tried after it.
    assert!(provider.complete(&conversation()).is_ok());
    assert_eq!(
        (key(&seen), key(&seen)),
        ("key-b".to_string(), "key-a".to_string())
    );

    let keys = sion_agent::providers::Keys::parse(" one ,two,, ");
    assert_eq!(keys.len(), 2);
    assert_eq!(format!("{keys:?}"), "Keys(2)", "the keys are never shown");
}

#[test]
fn the_responses_api_is_streamed_unstored_and_its_token_renewed_when_refused() {
    use sion_agent::providers::{responses_body, Bearer, OpenAiResponses};
    let body = responses_body("gpt-test", &conversation());
    assert_eq!(body["instructions"], "You keep the books.");
    assert_eq!(
        (body["store"].clone(), body["stream"].clone()),
        (json!(false), json!(true))
    );
    assert_eq!(
        body["input"],
        json!([
            {"role": "user", "content": [{"type": "input_text", "text": "What is my balance?"}]},
            {"role": "assistant", "content": [{"type": "output_text", "text": "Checking."}]},
            {"type": "function_call", "call_id": "toolu_1", "name": "balance", "arguments": "{\"asset\":\"USD\"}"},
            {"type": "function_call_output", "call_id": "toolu_1", "output": "12.5 USD"}
        ])
    );
    assert_eq!(
        body["tools"][0],
        json!({"type": "function", "name": "balance", "description": "The account's balance",
            "parameters": {"type": "object", "properties": {"asset": {"type": "string"}}}, "strict": false})
    );

    // A token refused once is asked for anew; then the answer streams.
    let completed = json!({"type": "response.completed", "response": {
        "model": "gpt-test", "status": "completed",
        "output": [
            {"type": "reasoning", "id": "rs_1", "summary": []},
            {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Paying now."}]},
            {"type": "function_call", "call_id": "call_7", "name": "pay", "arguments": "{\"to\":\"bob\"}"}
        ],
        "usage": {"input_tokens": 21, "output_tokens": 9}}});
    let sse = events(&[
        (
            "response.created",
            json!({"type": "response.created", "response": {"model": "gpt-test"}}),
        ),
        (
            "response.output_text.delta",
            json!({"type": "response.output_text.delta", "delta": "Paying "}),
        ),
        (
            "response.output_text.delta",
            json!({"type": "response.output_text.delta", "delta": "now."}),
        ),
        (
            "response.output_item.added",
            json!({"type": "response.output_item.added",
            "item": {"type": "function_call", "name": "pay", "call_id": "call_7"}}),
        ),
        ("response.completed", completed),
    ]);
    let (base, seen) = serve_urls(vec![
        (401, "application/json", "{}".into()),
        (200, "text/event-stream", sse),
    ]);
    let renewed = std::sync::atomic::AtomicBool::new(false);
    let renewed = std::sync::Arc::new(renewed);
    let asked = renewed.clone();
    let mut provider = OpenAiResponses::new(
        client(),
        "gpt-test",
        Bearer::Token(Box::new(move |fresh| {
            if fresh {
                asked.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok("token-2".into())
            } else {
                Ok("token-1".into())
            }
        })),
    );
    provider.base = format!("{base}/v1");
    let (told, answer) = streamed(&provider);
    assert_eq!(
        told,
        vec![
            sion_agent::Delta::Text("Paying ".into()),
            sion_agent::Delta::Text("now.".into()),
            sion_agent::Delta::Call("pay".into()),
        ]
    );
    let answer = answer.unwrap();
    assert_eq!(answer.stop, Stop::ToolUse);
    assert_eq!(answer.content[0], Block::text("Paying now."));
    assert!(
        matches!(&answer.content[1], Block::ToolUse { id, name, input, .. }
        if id == "call_7" && name == "pay" && input["to"] == "bob")
    );
    assert_eq!((answer.usage.input, answer.usage.output), (21, 9));
    assert!(renewed.load(std::sync::atomic::Ordering::SeqCst));
    let bearer = |seen: &Requests| {
        let (url, headers, _) = seen.recv().unwrap();
        assert_eq!(url, "/v1/responses");
        headers
            .into_iter()
            .find(|(name, _)| name == "authorization")
            .map(|(_, value)| value)
            .unwrap()
    };
    assert_eq!(bearer(&seen), "Bearer token-1");
    assert_eq!(bearer(&seen), "Bearer token-2");

    // A plan's limit, reported partway, may pass: another model is asked.
    let failed = events(&[(
        "response.failed",
        json!({"type": "response.failed", "response": {"status": "failed",
            "error": {"code": "subscription_sharing_usage_limit_exceeded", "message": "limit"}}}),
    )]);
    let (base, _) = serve_urls(vec![(200, "text/event-stream", failed)]);
    let mut limited = OpenAiResponses::new(
        client(),
        "gpt-test",
        Bearer::Keys(sion_agent::providers::Keys::parse("sk-a")),
    );
    limited.base = base;
    assert!(matches!(streamed(&limited).1, Err(ModelError::Busy(_))));
}
