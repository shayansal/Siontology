//! What a model thought, kept and given back to it alone, and how hard it is
//! asked to think: each provider against a local stand-in for its API.

use std::sync::mpsc;
use std::thread;

use serde_json::{json, Value};
use sion_agent::model::{
    most_tokens, thinking_of, Block, Effort, Message, Model, ModelError, Request, Response, Role,
    Stop, Thinking, ToolSpec, Usage,
};
use sion_agent::providers::{
    gemini_body, gemini_response, openai_messages, openai_response, Anthropic, Gemini,
    OpenAiCompatible, OpenAiResponses,
};

fn client() -> sion_http::Client {
    sion_http::Client::new(sion_http::Options {
        allow_private: true,
        proxy_from_env: false,
        ..sion_http::Options::default()
    })
    .unwrap()
}

/// Serves one request after another, each answered with its `(status,
/// media type, body)`, and sends each request's JSON body back.
fn serve_each(answers: Vec<(u16, &'static str, String)>) -> (String, mpsc::Receiver<Value>) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for (status, media, answer) in answers {
            let mut request = server.recv().unwrap();
            let mut body = String::new();
            request.as_reader().read_to_string(&mut body).unwrap();
            tx.send(serde_json::from_str(&body).unwrap()).unwrap();
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

/// Server-Sent Events, each `(name, data)` an event.
fn events(events: &[(&str, Value)]) -> String {
    events
        .iter()
        .map(|(name, data)| format!("event: {name}\ndata: {data}\n\n"))
        .collect()
}

fn balance_tool() -> ToolSpec {
    ToolSpec {
        name: "balance".into(),
        description: "The account's balance".into(),
        input_schema: json!({"type": "object"}),
    }
}

/// A question, the model's answer with `blocks`, and the result of its
/// call `toolu_1`.
fn after(blocks: Vec<Block>) -> Request {
    Request {
        system: "You keep the books.".into(),
        messages: vec![
            Message::user("What is my balance?"),
            Message {
                role: Role::Assistant,
                content: blocks,
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
        tools: vec![balance_tool()],
        max_tokens: 512,
        images: Default::default(),
    }
}

fn call() -> Block {
    Block::ToolUse {
        id: "toolu_1".into(),
        name: "balance".into(),
        input: json!({}),
        signature: None,
    }
}

fn kept(from: &str, data: Value) -> Block {
    Block::Reasoning {
        from: from.into(),
        data,
    }
}

#[test]
fn anthropic_gives_its_signed_thinking_back_as_it_came_and_no_one_else_s() {
    let answer = json!({
        "model": "claude-opus-5-5",
        "content": [
            {"type": "thinking", "thinking": "", "signature": "c2lnMQ=="},
            {"type": "redacted_thinking", "data": "cmVkYWN0ZWQ="},
            // Cut off before it was signed: it could not be given back.
            {"type": "thinking", "thinking": "half a tho", "signature": ""},
            {"type": "text", "text": "Checking."},
            {"type": "tool_use", "id": "toolu_1", "name": "balance", "input": {}}
        ],
        "stop_reason": "tool_use",
        "usage": {"input_tokens": 10, "output_tokens": 20}
    });
    let (base, seen) = serve_each(vec![(200, "application/json", answer.to_string())]);
    let mut provider = Anthropic::new(client(), "claude-opus-5-5", "k");
    provider.base = base.clone();
    let mut first = after(Vec::new());
    first.messages.truncate(1);
    let response = provider.complete(&first).unwrap();
    let from = format!("anthropic {base} claude-opus-5-5");
    assert_eq!(provider.source(), from);
    assert_eq!(
        response.content,
        vec![
            kept(
                &from,
                json!({"type": "thinking", "thinking": "", "signature": "c2lnMQ=="})
            ),
            kept(
                &from,
                json!({"type": "redacted_thinking", "data": "cmVkYWN0ZWQ="})
            ),
            Block::text("Checking."),
            call(),
        ],
        "signed and redacted thinking kept, as its provider's, in order"
    );
    seen.recv().unwrap();
    // The next call gives it back as it came, before the call it led to;
    // reasoning another provider kept is not shown.
    let mut next = after(response.content.clone());
    next.messages[0].content.push(kept(
        "gemini https://g gemini-3-pro",
        json!({"thoughtSignature": "Z2VtaW5p"}),
    ));
    let body = provider.body(&next);
    assert_eq!(
        body["messages"][1]["content"],
        json!([
            {"type": "thinking", "thinking": "", "signature": "c2lnMQ=="},
            {"type": "redacted_thinking", "data": "cmVkYWN0ZWQ="},
            {"type": "text", "text": "Checking."},
            {"type": "tool_use", "id": "toolu_1", "name": "balance", "input": {}}
        ])
    );
    assert_eq!(
        body["messages"][0]["content"],
        json!([{"type": "text", "text": "What is my balance?"}])
    );
    // Another model of Anthropic's, or another base, is not given it either.
    let mut other = Anthropic::new(client(), "claude-sonnet-5-5", "k");
    other.base = base;
    assert!(!other.body(&next).to_string().contains("c2lnMQ=="));
    // Nor are the other providers.
    let gemini = gemini_body(&next, "gemini https://elsewhere gemini-3-pro");
    let chat = openai_messages("", &next.messages, &next.images, "openai-chat https://o m");
    for shown in [gemini.to_string(), json!(chat).to_string()] {
        assert!(!shown.contains("c2lnMQ==") && !shown.contains("cmVkYWN0ZWQ="));
    }
}

#[test]
fn a_streamed_anthropic_answer_keeps_its_thinking_signed_and_drops_it_unsigned() {
    let sse = events(&[
        (
            "message_start",
            json!({"type": "message_start", "message": {"model": "claude-opus-5-5", "usage": {}}}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 0,
                "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "thinking_delta", "thinking": "Balance, "}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "thinking_delta", "thinking": "then pay."}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "signature_delta", "signature": "c2ln"}}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 1,
                "content_block": {"type": "redacted_thinking", "data": "cmVk"}}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 2,
                "content_block": {"type": "tool_use", "id": "toolu_1", "name": "balance", "input": {}}}),
        ),
        (
            "message_delta",
            json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 5}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ]);
    let cut = events(&[
        (
            "message_start",
            json!({"type": "message_start", "message": {"model": "claude-opus-5-5", "usage": {}}}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 0,
                "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "thinking_delta", "thinking": "Out of room bef"}}),
        ),
        (
            "message_delta",
            json!({"type": "message_delta", "delta": {"stop_reason": "max_tokens"}, "usage": {"output_tokens": 512}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ]);
    let (base, _seen) = serve_each(vec![
        (200, "text/event-stream", sse),
        (200, "text/event-stream", cut),
    ]);
    let mut provider = Anthropic::new(client(), "claude-opus-5-5", "k");
    provider.base = base;
    let from = provider.source();
    let mut told = Vec::new();
    let mut question = after(Vec::new());
    question.messages.truncate(1);
    let answer = provider
        .stream(&question, &mut |delta| {
            told.push(delta);
            true
        })
        .unwrap();
    assert_eq!(
        told,
        vec![sion_agent::Delta::Call("balance".into())],
        "thinking is not told"
    );
    assert_eq!(
        answer.content,
        vec![
            kept(
                &from,
                json!({"type": "thinking", "thinking": "Balance, then pay.", "signature": "c2ln"})
            ),
            kept(&from, json!({"type": "redacted_thinking", "data": "cmVk"})),
            call(),
        ]
    );
    // Thinking that ran out of room before it was signed is not kept.
    let answer = provider.stream(&question, &mut |_| true).unwrap();
    assert_eq!(answer.stop, Stop::MaxTokens);
    assert!(answer.content.is_empty());
}

#[test]
fn reasoning_that_no_longer_checks_is_let_go_rather_than_failing_the_call() {
    let refusal = json!({"type": "error", "error": {"type": "invalid_request_error",
        "message": "messages.1.content.0: Invalid `signature` in `thinking` block"}});
    let answer = json!({"model": "claude-opus-5-5", "content": [{"type": "text", "text": "12.5 USD."}],
        "stop_reason": "end_turn", "usage": {"input_tokens": 1, "output_tokens": 1}});
    let (base, seen) = serve_each(vec![
        (400, "application/json", refusal.to_string()),
        (200, "application/json", answer.to_string()),
        (400, "application/json", refusal.to_string()),
    ]);
    let mut provider = Anthropic::new(client(), "claude-opus-5-5", "k");
    provider.base = base;
    let from = provider.source();
    let request = after(vec![
        kept(
            &from,
            json!({"type": "thinking", "thinking": "", "signature": "b2xk"}),
        ),
        call(),
    ]);
    let response = provider.complete(&request).unwrap();
    assert_eq!(response.content, vec![Block::text("12.5 USD.")]);
    let first = seen.recv().unwrap();
    let second = seen.recv().unwrap();
    assert_eq!(first["messages"][1]["content"][0]["type"], "thinking");
    assert_eq!(
        second["messages"][1]["content"],
        json!([{"type": "tool_use", "id": "toolu_1", "name": "balance", "input": {}}]),
        "asked again without the reasoning, all else as it was"
    );
    // A request that keeps no reasoning is refused once, as before.
    let plain = after(vec![call()]);
    assert!(
        matches!(provider.complete(&plain), Err(ModelError::Refused(why)) if why.starts_with("HTTP 400"))
    );
    assert!(seen.recv().is_ok());
    assert!(seen.try_recv().is_err(), "not asked again");
}

#[test]
fn how_hard_each_provider_is_asked_to_think() {
    let question = after(vec![call()]);
    let mut anthropic = Anthropic::new(client(), "claude-opus-5-5", "k");
    let body = anthropic.body(&question);
    assert!(body.get("thinking").is_none() && body.get("output_config").is_none());
    assert_eq!(
        body["max_tokens"], 512,
        "the provider's default, and no more room"
    );

    anthropic.thinking = Some(Thinking::Effort(Effort::High));
    let body = anthropic.body(&question);
    assert_eq!(body["thinking"], json!({"type": "adaptive"}));
    assert_eq!(body["output_config"], json!({"effort": "high"}));
    assert_eq!(
        body["max_tokens"],
        512 + 16_384,
        "room to think besides the answer"
    );

    anthropic.thinking = Some(Thinking::Effort(Effort::Max));
    let body = anthropic.body(&question);
    assert_eq!(body["output_config"], json!({"effort": "max"}));
    assert_eq!(body["max_tokens"], 60_512);

    anthropic.thinking = Some(Thinking::Effort(Effort::None));
    let body = anthropic.body(&question);
    assert_eq!(body["thinking"], json!({"type": "disabled"}));
    assert!(body.get("output_config").is_none());
    assert_eq!(body["max_tokens"], 512);

    anthropic.thinking = Some(Thinking::Budget(8_000));
    let body = anthropic.body(&question);
    assert_eq!(
        body["thinking"],
        json!({"type": "enabled", "budget_tokens": 8_000})
    );
    assert_eq!(body["max_tokens"], 8_512);
    // The API's least budget, and less than all it may write.
    anthropic.thinking = Some(Thinking::Budget(100));
    let body = anthropic.body(&question);
    assert_eq!(body["thinking"]["budget_tokens"], 1_024);
    assert!(body["max_tokens"].as_u64().unwrap() > 1_024);

    let mut chat = OpenAiCompatible::new(client(), "http://127.0.0.1:1/v1", "gpt-test", None);
    assert!(chat.body(&question).get("reasoning_effort").is_none());
    chat.thinking = Some(Thinking::Effort(Effort::Low));
    let body = chat.body(&question);
    assert_eq!(body["reasoning_effort"], "low");
    assert_eq!(body["max_tokens"], 512 + 4_096);

    let mut gemini = Gemini::new(client(), "gemini-3-pro-preview", "g");
    assert!(gemini.body(&question)["generationConfig"]
        .get("thinkingConfig")
        .is_none());
    gemini.thinking = Some(Thinking::Effort(Effort::Medium));
    let body = gemini.body(&question);
    assert_eq!(
        body["generationConfig"]["thinkingConfig"],
        json!({"thinkingLevel": "medium"})
    );
    assert_eq!(body["generationConfig"]["maxOutputTokens"], 512 + 8_192);
    gemini.thinking = Some(Thinking::Budget(2_048));
    let body = gemini.body(&question);
    assert_eq!(
        body["generationConfig"]["thinkingConfig"],
        json!({"thinkingBudget": 2_048})
    );
    assert_eq!(body["generationConfig"]["maxOutputTokens"], 2_560);

    let mut responses = OpenAiResponses::new(
        client(),
        "gpt-test",
        sion_agent::providers::Bearer::Keys(sion_agent::providers::Keys::parse("k")),
    );
    let body = responses.body(&question);
    assert!(body.get("reasoning").is_none());
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    responses.thinking = Some(Thinking::Effort(Effort::Max));
    let body = responses.body(&question);
    assert_eq!(
        body["reasoning"],
        json!({"effort": "xhigh"}),
        "OpenAI's most"
    );
    assert_eq!(body["max_output_tokens"], 60_512);
}

#[test]
fn thinking_is_written_after_a_model_s_name() {
    assert_eq!(
        thinking_of("anthropic:claude-opus-5-5#high").unwrap(),
        (
            "anthropic:claude-opus-5-5",
            Some(Thinking::Effort(Effort::High))
        )
    );
    assert_eq!(
        thinking_of("gemini:gemini-2.5-flash#0").unwrap(),
        ("gemini:gemini-2.5-flash", Some(Thinking::Budget(0)))
    );
    assert_eq!(
        thinking_of("local:qwen3:32b").unwrap(),
        ("local:qwen3:32b", None)
    );
    for level in ["none", "minimal", "low", "medium", "high", "xhigh", "max"] {
        let thinking = Thinking::parse(level).unwrap();
        assert_eq!(thinking.name(), level);
    }
    assert_eq!(
        Thinking::parse("off").unwrap(),
        Thinking::Effort(Effort::None)
    );
    assert!(thinking_of("anthropic:claude-opus-5-5#hihg").is_err());
    assert!(Thinking::parse("70000").is_err(), "a budget past the most");
    assert_eq!(most_tokens(None, 4_096), 4_096);
    assert_eq!(
        most_tokens(Some(Thinking::Effort(Effort::Medium)), 4_096),
        12_288
    );
    assert_eq!(
        most_tokens(Some(Thinking::Effort(Effort::Max)), 30_000),
        64_000,
        "at most the most any of them writes"
    );
    assert_eq!(
        most_tokens(Some(Thinking::Effort(Effort::Max)), 70_000),
        70_000,
        "never less than was asked for"
    );
}

#[test]
fn the_responses_api_keeps_its_reasoning_encrypted_and_gives_it_back_before_what_followed() {
    let reasoning =
        json!({"type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "ZW5j"});
    let function_call = json!({"type": "function_call", "id": "fc_1", "call_id": "toolu_1",
        "name": "balance", "arguments": "{}"});
    // The answer's items, given as the stream goes; the terminal event, as
    // a sign-in's backend gives it, lists none.
    let sse = events(&[
        (
            "response.created",
            json!({"type": "response.created", "response": {"model": "gpt-test"}}),
        ),
        (
            "response.output_item.done",
            json!({"type": "response.output_item.done", "output_index": 0, "item": reasoning}),
        ),
        (
            "response.output_item.added",
            json!({"type": "response.output_item.added", "output_index": 1, "item": function_call}),
        ),
        (
            "response.output_item.done",
            json!({"type": "response.output_item.done", "output_index": 1, "item": function_call}),
        ),
        (
            "response.completed",
            json!({"type": "response.completed", "response": {"model": "gpt-test",
                "status": "completed", "output": [],
                "usage": {"input_tokens": 9, "output_tokens": 30}}}),
        ),
    ]);
    let (base, seen) = serve_each(vec![(200, "text/event-stream", sse)]);
    let mut provider = OpenAiResponses::new(
        client(),
        "gpt-test",
        sion_agent::providers::Bearer::Keys(sion_agent::providers::Keys::parse("k")),
    );
    provider.base = base.clone();
    let from = provider.source();
    assert_eq!(from, format!("openai-responses {base} gpt-test"));
    let mut question = after(Vec::new());
    question.messages.truncate(1);
    let answer = provider.complete(&question).unwrap();
    assert_eq!(answer.stop, Stop::ToolUse);
    assert_eq!(
        answer.content,
        vec![
            kept(&from, json!({"summary": [], "encrypted_content": "ZW5j"})),
            call(),
        ]
    );
    let asked = seen.recv().unwrap();
    assert_eq!(asked["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(asked["store"], false);
    // Given back, without its id, before the call it led to; reasoning
    // with nothing after it is not.
    let mut next = after(answer.content.clone());
    next.messages.push(Message {
        role: Role::Assistant,
        content: vec![
            Block::text("12.5 USD."),
            kept(
                &from,
                json!({"summary": [], "encrypted_content": "bGFzdA=="}),
            ),
        ],
    });
    let body = provider.body(&next);
    assert_eq!(
        body["input"],
        json!([
            {"role": "user", "content": [{"type": "input_text", "text": "What is my balance?"}]},
            {"type": "reasoning", "summary": [], "encrypted_content": "ZW5j"},
            {"type": "function_call", "call_id": "toolu_1", "name": "balance", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "toolu_1", "output": "12.5 USD"},
            {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "12.5 USD."}]}
        ])
    );
}

#[test]
fn a_responses_model_that_will_not_give_its_reasoning_is_not_asked_for_it_again() {
    let refusal = json!({"error": {"message": "Encrypted content is not supported with this model.",
        "type": "invalid_request_error", "param": "include", "code": "unsupported_parameter"}});
    let done = || {
        events(&[(
            "response.completed",
            json!({"type": "response.completed", "response": {"model": "gpt-test", "status": "completed",
                "output": [{"type": "message", "content": [{"type": "output_text", "text": "Done."}]}],
                "usage": {"input_tokens": 1, "output_tokens": 1}}}),
        )])
    };
    let (base, seen) = serve_each(vec![
        (400, "application/json", refusal.to_string()),
        (200, "text/event-stream", done()),
        (200, "text/event-stream", done()),
    ]);
    let mut provider = OpenAiResponses::new(
        client(),
        "gpt-test",
        sion_agent::providers::Bearer::Keys(sion_agent::providers::Keys::parse("k")),
    );
    provider.base = base;
    let mut question = after(Vec::new());
    question.messages.truncate(1);
    assert_eq!(
        provider.complete(&question).unwrap().content,
        vec![Block::text("Done.")]
    );
    assert!(seen.recv().unwrap().get("include").is_some());
    assert!(seen.recv().unwrap().get("include").is_none());
    provider.complete(&question).unwrap();
    assert!(
        seen.recv().unwrap().get("include").is_none(),
        "not asked for again"
    );
}

#[test]
fn a_chat_server_s_reasoning_goes_back_within_the_task_it_was_for() {
    let whole = openai_response(&json!({
        "model": "deepseek-test",
        "choices": [{"index": 0, "finish_reason": "tool_calls", "message": {
            "role": "assistant", "content": "", "reasoning_content": "Look it up first.",
            "tool_calls": [{"id": "toolu_1", "type": "function",
                "function": {"name": "balance", "arguments": "{}"}}]}}],
        "usage": {"prompt_tokens": 3, "completion_tokens": 4}
    }))
    .unwrap();
    assert_eq!(
        whole.content,
        vec![
            kept("", json!({"reasoning_content": "Look it up first."})),
            call(),
        ]
    );
    let from = "openai-chat http://127.0.0.1:1/v1 deepseek-test";
    let earlier = Message {
        role: Role::Assistant,
        content: vec![
            kept(from, json!({"reasoning": "An old task's."})),
            Block::text("Hello."),
        ],
    };
    let mut request = after(vec![
        kept(from, json!({"reasoning_content": "Look it up first."})),
        call(),
    ]);
    request
        .messages
        .splice(0..0, [Message::user("Hi"), earlier]);
    let messages = openai_messages("", &request.messages, &request.images, from);
    // The system prompt, the old task, then this one.
    assert!(
        messages[2].get("reasoning").is_none(),
        "an old task's is not given back"
    );
    assert_eq!(messages[4]["reasoning_content"], "Look it up first.");
    assert_eq!(messages[4]["tool_calls"][0]["id"], "toolu_1");
    // Another server's is not.
    let other = openai_messages(
        "",
        &request.messages,
        &request.images,
        "openai-chat http://x m",
    );
    assert!(other[4].get("reasoning_content").is_none());

    // Streamed, its reasoning comes in pieces.
    let sse: String = [
        json!({"model": "local", "choices": [{"index": 0, "delta": {"reasoning": "Look "}}]}),
        json!({"model": "local", "choices": [{"index": 0, "delta": {"reasoning": "it up."}}]}),
        json!({"model": "local", "choices": [{"index": 0, "delta": {"content": "Checking."}, "finish_reason": "stop"}]}),
    ]
    .iter()
    .map(|chunk| format!("data: {chunk}\n\n"))
    .chain(["data: [DONE]\n\n".to_string()])
    .collect();
    let (base, _seen) = serve_each(vec![(200, "text/event-stream", sse)]);
    let provider = OpenAiCompatible::new(client(), &format!("{base}/v1"), "local", None);
    let answer = provider.stream(&request, &mut |_| true).unwrap();
    assert_eq!(
        answer.content,
        vec![
            kept(&provider.source(), json!({"reasoning": "Look it up."})),
            Block::text("Checking."),
        ]
    );
}

#[test]
fn a_chat_server_that_wants_max_completion_tokens_is_sent_that_from_then_on() {
    let refusal = json!({"error": {"message": "Unsupported parameter: 'max_tokens' is not supported with this model. Use 'max_completion_tokens' instead.",
        "type": "invalid_request_error", "param": "max_tokens", "code": "unsupported_parameter"}});
    let answer = json!({"model": "gpt-test", "choices": [{"index": 0, "finish_reason": "stop",
        "message": {"role": "assistant", "content": "Done."}}], "usage": {}});
    let (base, seen) = serve_each(vec![
        (400, "application/json", refusal.to_string()),
        (200, "application/json", answer.to_string()),
        (200, "application/json", answer.to_string()),
    ]);
    let mut provider = OpenAiCompatible::new(client(), &format!("{base}/v1"), "gpt-test", None);
    provider.thinking = Some(Thinking::Effort(Effort::High));
    let question = after(vec![call()]);
    assert_eq!(
        provider.complete(&question).unwrap().content,
        vec![Block::text("Done.")]
    );
    let first = seen.recv().unwrap();
    let second = seen.recv().unwrap();
    assert_eq!(first["max_tokens"], 512 + 16_384);
    assert!(second.get("max_tokens").is_none());
    assert_eq!(second["max_completion_tokens"], 512 + 16_384);
    assert_eq!(second["reasoning_effort"], "high");
    provider.complete(&question).unwrap();
    assert_eq!(seen.recv().unwrap()["max_completion_tokens"], 512 + 16_384);
}

#[test]
fn gemini_gives_back_the_signature_on_its_text_on_the_text() {
    let answer = gemini_response(&json!({
        "modelVersion": "gemini-3-pro-preview",
        "candidates": [{"content": {"role": "model", "parts": [
            {"text": "12.5 "},
            {"text": "USD.", "thoughtSignature": "dGV4dA=="}
        ]}, "finishReason": "STOP"}],
        "usageMetadata": {"promptTokenCount": 3, "candidatesTokenCount": 2}
    }))
    .unwrap();
    assert_eq!(
        answer.content,
        vec![
            Block::text("12.5 USD."),
            kept("", json!({"thoughtSignature": "dGV4dA=="}))
        ]
    );
    // Streamed, the signature may come on a part with no text.
    let sse: String = [
        json!({"modelVersion": "gemini-3-pro-preview", "candidates": [{"content": {"parts": [{"text": "12.5 USD."}]}}]}),
        json!({"candidates": [{"content": {"parts": [{"text": "", "thoughtSignature": "dGV4dA=="}]}, "finishReason": "STOP"}]}),
    ]
    .iter()
    .map(|chunk| format!("data: {chunk}\n\n"))
    .collect();
    let (base, _seen) = serve_each(vec![(200, "text/event-stream", sse)]);
    let mut provider = Gemini::new(client(), "gemini-3-pro-preview", "g");
    provider.base = base;
    let from = provider.source();
    let mut question = after(Vec::new());
    question.messages.truncate(1);
    let streamed = provider.stream(&question, &mut |_| true).unwrap();
    assert_eq!(
        streamed.content,
        vec![
            Block::text("12.5 USD."),
            kept(&from, json!({"thoughtSignature": "dGV4dA=="}))
        ]
    );
    // Given back on the text, and only to it.
    let mut next = question.clone();
    next.messages.push(Message {
        role: Role::Assistant,
        content: streamed.content,
    });
    next.messages.push(Message::user("Thanks."));
    let body = provider.body(&next);
    assert_eq!(
        body["contents"][1],
        json!({"role": "model", "parts": [{"text": "12.5 USD.", "thoughtSignature": "dGV4dA=="}]})
    );
    let other = gemini_body(&next, "gemini https://elsewhere gemini-3-pro-preview");
    assert_eq!(
        other["contents"][1],
        json!({"role": "model", "parts": [{"text": "12.5 USD."}]})
    );
}

/// A model that thinks with `room` tokens besides what it answers.
struct Thinks {
    room: u32,
    asked: std::sync::Mutex<usize>,
}

impl Model for Thinks {
    fn id(&self) -> &str {
        "thinks"
    }

    fn complete(&self, _request: &Request) -> Result<Response, ModelError> {
        *self.asked.lock().unwrap() += 1;
        Ok(Response {
            content: vec![Block::text("Done.")],
            stop: Stop::EndTurn,
            usage: Usage::default(),
            model: "thinks".into(),
        })
    }

    fn most_tokens(&self, max_tokens: u32) -> u32 {
        max_tokens + self.room
    }
}

#[test]
fn a_run_s_budget_counts_the_room_its_model_thinks_in() {
    use sion_agent::{Autonomy, Ended, Limits, Policy, Runtime, State, Toolbox};
    fn now() -> u64 {
        1_000
    }
    fn never() -> bool {
        false
    }
    let tools = Toolbox::new();
    let run = |model: &Thinks| {
        let rt = Runtime {
            models: vec![model],
            tools: &tools,
            policy: Policy {
                autonomy: Autonomy::L3,
                ..Policy::default()
            },
            limits: Limits {
                budget_tokens: Some(3_000),
                max_tokens: 1_000,
                reserve_tokens: 500,
                ..Limits::default()
            },
            system: "You are a test agent.".into(),
            now: &now,
            stopped: &never,
            watch: None,
            steer: None,
            checkpoint: None,
            verify: None,
            code: None,
            hooks: None,
        };
        let mut state = State::start("s-room", "Hello", &rt.limits);
        rt.run(&mut state)
    };
    let quick = Thinks {
        room: 0,
        asked: Default::default(),
    };
    assert!(matches!(run(&quick), Ended::Done { .. }));
    let deep = Thinks {
        room: 3_000,
        asked: Default::default(),
    };
    assert_eq!(
        run(&deep),
        Ended::OutOfBudget {
            text: String::new()
        },
        "the room it would think in does not fit what is left"
    );
    assert_eq!(*deep.asked.lock().unwrap(), 0);
}
