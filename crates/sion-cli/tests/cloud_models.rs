//! The cloud platforms' models and the providers known by name, as `sion`
//! makes them from a model's name and its variables, answering through
//! stand-ins of the platforms' APIs: Bedrock's Converse with a signed
//! request, Vertex AI with a token minted from a service account's key,
//! Azure OpenAI with its `api-key` header, and OpenRouter.
//!
//! One test, alone in its binary: it sets the process's variables.

use serde_json::{json, Value};
use sion_agent::model::{Block, Message, Request, Role};

type Seen = Vec<(String, Vec<(String, String)>, String)>;

/// A stand-in API: answers each request in turn with `answers`, and gives
/// back what it was sent (path, headers, body).
fn stand_in(answers: Vec<Value>) -> (String, std::thread::JoinHandle<Seen>) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    let handle = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for answer in answers {
            let mut request = server.recv().unwrap();
            let mut body = String::new();
            request.as_reader().read_to_string(&mut body).unwrap();
            let headers = request
                .headers()
                .iter()
                .map(|h| (h.field.to_string().to_lowercase(), h.value.to_string()))
                .collect();
            seen.push((request.url().to_string(), headers, body));
            let header =
                tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                    .unwrap();
            request
                .respond(tiny_http::Response::from_string(answer.to_string()).with_header(header))
                .unwrap();
        }
        seen
    });
    (base, handle)
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> &'a str {
    headers
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
        .unwrap_or_default()
}

fn asked() -> Request {
    Request {
        system: "Answer briefly.".into(),
        messages: vec![Message {
            role: Role::User,
            content: vec![Block::text("Say hello.")],
        }],
        tools: Vec::new(),
        max_tokens: 64,
        images: Default::default(),
    }
}

fn chat_completion(text: &str) -> Value {
    json!({
        "id": "c1", "object": "chat.completion", "model": "m",
        "choices": [{"index": 0, "finish_reason": "stop",
            "message": {"role": "assistant", "content": text}}],
        "usage": {"prompt_tokens": 5, "completion_tokens": 2}
    })
}

#[test]
fn the_cloud_platforms_models_answer_through_their_own_apis() {
    let home = std::env::temp_dir().join(format!("sion-cloud-models-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("SION_HOME", home.join("sion"));
    std::env::set_var("HOME", &home);
    for name in [
        "AWS_BEARER_TOKEN_BEDROCK",
        "AWS_PROFILE",
        "GOOGLE_OAUTH_ACCESS_TOKEN",
        "GOOGLE_API_KEY",
        "ANTHROPIC_VERTEX_PROJECT_ID",
        "CLOUD_ML_REGION",
        "OPENROUTER_BASE_URL",
    ] {
        std::env::remove_var(name);
    }

    // Bedrock, signed with an access key.
    let (bedrock, bedrock_seen) = stand_in(vec![json!({
        "output": {"message": {"role": "assistant", "content": [{"text": "Hello from Bedrock."}]}},
        "stopReason": "end_turn",
        "usage": {"inputTokens": 7, "outputTokens": 4}
    })]);
    std::env::set_var("AWS_ENDPOINT_URL_BEDROCK_RUNTIME", &bedrock);
    std::env::set_var("AWS_REGION", "eu-west-1");
    std::env::set_var("AWS_ACCESS_KEY_ID", "AKIDEXAMPLE");
    std::env::set_var("AWS_SECRET_ACCESS_KEY", "secret");
    std::env::set_var("AWS_SESSION_TOKEN", "session");
    let model =
        sion_cli::runtime::model("bedrock:anthropic.claude-sonnet-4-5-20250929-v1:0#2048", 0)
            .unwrap();
    let answer = model.complete(&asked()).unwrap();
    assert_eq!(answer.content, vec![Block::text("Hello from Bedrock.")]);
    let seen = bedrock_seen.join().unwrap();
    let (path, headers, body) = &seen[0];
    assert_eq!(
        path,
        "/model/anthropic.claude-sonnet-4-5-20250929-v1%3A0/converse"
    );
    let authorization = header(headers, "authorization");
    assert!(
        authorization.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/")
            && authorization.contains("/eu-west-1/bedrock/aws4_request")
            && authorization
                .contains("SignedHeaders=content-type;host;x-amz-date;x-amz-security-token"),
        "{authorization}"
    );
    assert_eq!(header(headers, "x-amz-security-token"), "session");
    let body: Value = serde_json::from_str(body).unwrap();
    assert_eq!(
        body["additionalModelRequestFields"]["thinking"]["budget_tokens"],
        2048
    );

    // Vertex AI, with a token minted from a service account's key.
    let (vertex, vertex_seen) = stand_in(vec![
        json!({"access_token": "ya29.minted", "expires_in": 3599}),
        json!({"candidates": [{"content": {"role": "model", "parts": [{"text": "Hello from Vertex."}]}, "finishReason": "STOP"}],
               "usageMetadata": {"promptTokenCount": 4, "candidatesTokenCount": 3}}),
    ]);
    let key = {
        use aws_lc_rs::encoding::AsDer;
        let pair =
            aws_lc_rs::signature::RsaKeyPair::generate(aws_lc_rs::rsa::KeySize::Rsa2048).unwrap();
        sion_agent::encoding::base64(pair.as_der().unwrap().as_ref())
    };
    let account = home.join("account.json");
    std::fs::write(
        &account,
        json!({
            "type": "service_account",
            "client_email": "agent@proj.iam.gserviceaccount.com",
            "private_key_id": "k1",
            "private_key": format!("-----BEGIN PRIVATE KEY-----\n{key}\n-----END PRIVATE KEY-----\n"),
            "token_uri": format!("{vertex}/token"),
        })
        .to_string(),
    )
    .unwrap();
    std::env::set_var("GOOGLE_CLOUD_PROJECT", "proj");
    std::env::set_var("GOOGLE_CLOUD_LOCATION", "europe-west4");
    std::env::set_var("GOOGLE_APPLICATION_CREDENTIALS", &account);
    std::env::set_var("VERTEX_BASE_URL", &vertex);
    let model = sion_cli::runtime::model("vertex:gemini-2.5-pro", 0).unwrap();
    let answer = model.complete(&asked()).unwrap();
    assert_eq!(answer.content, vec![Block::text("Hello from Vertex.")]);
    let seen = vertex_seen.join().unwrap();
    assert_eq!(seen[0].0, "/token");
    assert!(seen[0].2.contains("assertion="), "{}", seen[0].2);
    assert_eq!(
        seen[1].0,
        "/v1/projects/proj/locations/europe-west4/publishers/google/models/gemini-2.5-pro:generateContent"
    );
    assert_eq!(header(&seen[1].1, "authorization"), "Bearer ya29.minted");

    // Azure OpenAI, its key in its own header.
    let (azure, azure_seen) = stand_in(vec![chat_completion("Hello from Azure.")]);
    std::env::set_var("AZURE_OPENAI_ENDPOINT", &azure);
    std::env::set_var("AZURE_OPENAI_API_KEY", "azure-key");
    let model = sion_cli::runtime::model("azure:my-gpt", 0).unwrap();
    let answer = model.complete(&asked()).unwrap();
    assert_eq!(answer.content, vec![Block::text("Hello from Azure.")]);
    let seen = azure_seen.join().unwrap();
    assert_eq!(seen[0].0, "/openai/v1/chat/completions");
    assert_eq!(header(&seen[0].1, "api-key"), "azure-key");
    assert_eq!(header(&seen[0].1, "authorization"), "");
    let body: Value = serde_json::from_str(&seen[0].2).unwrap();
    assert_eq!(body["model"], "my-gpt");

    // A provider known by name.
    let (router, router_seen) = stand_in(vec![chat_completion("Hello from OpenRouter.")]);
    std::env::set_var("OPENROUTER_API_KEY", "or-key");
    std::env::set_var("OPENROUTER_BASE_URL", format!("{router}/api/v1"));
    let model = sion_cli::runtime::model("openrouter:moonshotai/kimi-k2.5", 0).unwrap();
    let answer = model.complete(&asked()).unwrap();
    assert_eq!(answer.content, vec![Block::text("Hello from OpenRouter.")]);
    let seen = router_seen.join().unwrap();
    assert_eq!(seen[0].0, "/api/v1/chat/completions");
    assert_eq!(header(&seen[0].1, "authorization"), "Bearer or-key");

    // What each needs, said when it is missing.
    std::env::remove_var("OPENROUTER_API_KEY");
    let refused = sion_cli::runtime::model("openrouter:x", 0)
        .err()
        .unwrap()
        .to_string();
    assert!(refused.contains("OPENROUTER_API_KEY"), "{refused}");
    let refused = sion_cli::runtime::model("deepseek:deepseek-chat#2048", 0)
        .err()
        .unwrap()
        .to_string();
    assert!(refused.contains("effort level"), "{refused}");
    let refused = sion_cli::runtime::model("nobody:x", 0)
        .err()
        .unwrap()
        .to_string();
    assert!(refused.contains("openrouter"), "{refused}");
    let _ = std::fs::remove_dir_all(home);
}
