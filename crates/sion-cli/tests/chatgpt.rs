//! Sign in with ChatGPT, against a stand-in for OpenAI's sign-in and API:
//! the browser's part played here; the app registered on its first
//! sign-in; the ID token checked against the stand-in's keys, for its
//! nonce; a model of the plan asked through the Responses API, streamed
//! and not stored; the access token renewed when the API refuses it, the
//! refresh token rotating; and a sign-in that did not share the plan is not
//! used.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use sion_agent::model::{Message, Request};

mod common;

const TEST_KEY: &str = include_str!("fixtures/rsa-2048.pem");
const TEST_N: &str = include_str!("fixtures/rsa-2048.n");

/// What the stand-in needs from the sign-in under way: its nonce, and the
/// scopes it grants.
#[derive(Default)]
struct Signing {
    nonce: String,
    scope: String,
}

#[test]
fn a_chatgpt_sign_in_runs_models_on_the_plan_and_renews_its_tokens() {
    let home = std::env::temp_dir().join(format!("sion-chatgpt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    // The only test in this binary, so the environment is its own.
    std::env::set_var("SION_HOME", &home);

    let signing = Arc::new(Mutex::new(Signing {
        scope: sion_cli::chatgpt::SCOPES.into(),
        ..Signing::default()
    }));
    let base_cell: Arc<Mutex<String>> = Arc::default();
    let (base, seen) = {
        let (signing, base_cell) = (signing.clone(), base_cell.clone());
        common::http_stand_in(move |sent| {
            let base = base_cell.lock().unwrap().clone();
            let path = sent.url.split('?').next().unwrap_or_default();
            match (sent.method.as_str(), path) {
                ("GET", "/.well-known/openid-configuration") => (
                    200,
                    json!({"issuer": base, "jwks_uri": format!("{base}/jwks")})
                        .to_string()
                        .into_bytes(),
                ),
                ("GET", "/jwks") => (
                    200,
                    json!({"keys": [{"kty": "RSA", "use": "sig", "kid": "k1",
                        "n": TEST_N.trim(), "e": "AQAB"}]})
                    .to_string()
                    .into_bytes(),
                ),
                ("POST", "/api/accounts/oauth/token") => {
                    let form: std::collections::BTreeMap<String, String> =
                        common::form(&sent.body).into_iter().collect();
                    let signing = signing.lock().unwrap();
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs();
                    let id_token = sion_cli::jwt::signed(
                        &json!({"iss": base, "aud": "oaiapp_123", "sub": "user-1",
                            "email": "ann@example.com", "iat": now, "exp": now + 3600,
                            "nonce": signing.nonce}),
                        TEST_KEY,
                        Some("k1"),
                    )
                    .unwrap();
                    match form.get("grant_type").map(String::as_str) {
                        Some("authorization_code") => (
                            200,
                            json!({"id_token": id_token, "access_token": "at-1",
                                "refresh_token": "rt-1", "token_type": "Bearer",
                                "expires_in": 3600, "scope": signing.scope})
                            .to_string()
                            .into_bytes(),
                        ),
                        Some("refresh_token") => {
                            let next = match form.get("refresh_token").map(String::as_str) {
                                Some("rt-1") => ("at-2", "rt-2"),
                                Some("rt-2") => ("at-3", "rt-3"),
                                _ => return (400, br#"{"error": "invalid_grant"}"#.to_vec()),
                            };
                            (
                                200,
                                json!({"access_token": next.0, "refresh_token": next.1,
                                    "token_type": "Bearer", "expires_in": 3600})
                                .to_string()
                                .into_bytes(),
                            )
                        }
                        _ => (400, br#"{"error": "unsupported_grant_type"}"#.to_vec()),
                    }
                }
                // The API: models, and responses, refusing the first token.
                ("GET", "/v1/models") => (
                    200,
                    json!({"models": [
                        {"slug": "gpt-plan", "display_name": "GPT Plan", "visibility": "list"},
                        {"slug": "gpt-hidden", "display_name": "Hidden", "visibility": "hide"}
                    ]})
                    .to_string()
                    .into_bytes(),
                ),
                ("POST", "/v1/responses")
                    if sent.header("authorization") == Some("Bearer at-1") =>
                {
                    (401, br#"{"error": {"message": "expired"}}"#.to_vec())
                }
                ("POST", "/v1/responses") => {
                    let completed = json!({"type": "response.completed", "response": {
                        "model": "gpt-plan", "status": "completed",
                        "output": [{"type": "message", "role": "assistant",
                            "content": [{"type": "output_text", "text": "Hello from the plan."}]}],
                        "usage": {"input_tokens": 5, "output_tokens": 4}}});
                    (
                        200,
                        format!(
                            "event: response.output_text.delta\ndata: {}\n\nevent: response.completed\ndata: {completed}\n\n",
                            json!({"type": "response.output_text.delta", "delta": "Hello from the plan."})
                        )
                        .into_bytes(),
                    )
                }
                _ => (404, b"{}".to_vec()),
            }
        })
    };
    *base_cell.lock().unwrap() = base.clone();
    std::env::set_var("CHATGPT_AUTH_URL", &base);
    std::env::set_var("CHATGPT_API_URL", format!("{base}/v1"));

    // The browser's part: open the address, sign in, come back with a code
    // and, the first time, the client OpenAI registered.
    let browse = |signing: Arc<Mutex<Signing>>| {
        move |url: &str| {
            let query = url.split_once('?').unwrap().1;
            let asked = sion_agent::oauth::query(query);
            signing.lock().unwrap().nonce = asked["nonce"].clone();
            let mut back = format!(
                "{}?code=c1&state={}",
                asked["redirect_uri"],
                sion_agent::oauth::encode(&asked["state"])
            );
            if asked["client_id"] == "dynamic_agent_client" {
                back.push_str("&client_id=oaiapp_123");
            }
            std::thread::spawn(move || {
                let client = sion_http::Client::new(sion_http::Options {
                    allow_private: true,
                    ..sion_http::Options::default()
                })
                .unwrap();
                for _ in 0..50 {
                    if client.get(&back, 1 << 16).is_ok() {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            });
        }
    };
    let mut first_url = String::new();
    let mut show = {
        let browse = browse(signing.clone());
        let first_url = &mut first_url;
        move |url: &str| {
            *first_url = url.to_string();
            browse(url)
        }
    };
    let said = sion_cli::chatgpt::login_with(&mut show).unwrap();
    assert!(
        said.contains("ann@example.com") && said.contains("runs on that plan"),
        "{said}"
    );
    // The first sign-in registered the app, under its name, for this
    // machine, with PKCE, asking for the plan.
    let asked = sion_agent::oauth::query(first_url.split_once('?').unwrap().1);
    assert_eq!(asked["client_id"], "dynamic_agent_client");
    assert_eq!(asked["agent_name_hint"], "Siontology");
    assert!(asked["ext_agent_host_id"].starts_with("sion-"));
    assert_eq!(asked["code_challenge_method"], "S256");
    assert_eq!(asked["resource"], "https://api.openai.com/v1");
    assert_eq!(asked["scope"], sion_cli::chatgpt::SCOPES);
    assert!(asked["redirect_uri"].starts_with("http://127.0.0.1:"));
    assert!(asked["redirect_uri"].ends_with("/auth/callback"));
    let exchanged = common::sent_to(&seen, "POST", "/api/accounts/oauth/token").remove(0);
    let form: std::collections::BTreeMap<String, String> =
        common::form(&exchanged.body).into_iter().collect();
    assert_eq!(form["client_id"], "oaiapp_123");
    assert_eq!(
        sion_agent::oauth::challenge_of(&form["code_verifier"]),
        asked["code_challenge"]
    );
    assert!(sion_cli::chatgpt::status()
        .unwrap()
        .contains("ann@example.com"));

    // The plan's models, and one of them asked: the first token refused,
    // a new one is had from the refresh token, which rotates.
    assert_eq!(
        sion_cli::chatgpt::models().unwrap(),
        [("gpt-plan".to_string(), "GPT Plan".to_string())]
    );
    let model = sion_cli::runtime::model("chatgpt:gpt-plan", 0).unwrap();
    let request = Request {
        system: "Be brief.".into(),
        messages: vec![Message::user("Hello?")],
        tools: Vec::new(),
        max_tokens: 100,
        images: Default::default(),
    };
    let answer = model.complete(&request).unwrap();
    assert_eq!(
        answer.content[0].clone(),
        sion_agent::model::Block::text("Hello from the plan.")
    );
    let asked: Vec<Value> = common::sent_to(&seen, "POST", "/v1/responses")
        .iter()
        .map(common::Sent::json)
        .collect();
    assert_eq!(asked.len(), 2, "refused once, then answered");
    assert!(asked
        .iter()
        .all(|a| a["store"] == false && a["stream"] == true));
    let renewed = common::sent_to(&seen, "POST", "/api/accounts/oauth/token");
    let renewal: std::collections::BTreeMap<String, String> =
        common::form(&renewed[1].body).into_iter().collect();
    assert_eq!(
        (
            renewal["grant_type"].as_str(),
            renewal["refresh_token"].as_str()
        ),
        ("refresh_token", "rt-1")
    );
    // The next renewal uses the refresh token that came back.
    assert_eq!(sion_cli::chatgpt::token(true).unwrap(), "at-3");

    // Signed in again: the app is registered already, and the person's
    // last sign-in is the hint.
    let mut again_url = String::new();
    let mut show = {
        let browse = browse(signing.clone());
        let again_url = &mut again_url;
        move |url: &str| {
            *again_url = url.to_string();
            browse(url)
        }
    };
    signing.lock().unwrap().scope = "openid profile email".into();
    let said = sion_cli::chatgpt::login_with(&mut show).unwrap();
    let asked = sion_agent::oauth::query(again_url.split_once('?').unwrap().1);
    assert_eq!(asked["client_id"], "oaiapp_123");
    assert!(!asked.contains_key("agent_name_hint"));
    assert_eq!(asked["login_hint"], "ann@example.com");
    // That sign-in did not share the plan: no model runs on it.
    assert!(said.contains("not shared"), "{said}");
    assert!(matches!(
        sion_cli::chatgpt::token(false),
        Err(sion_agent::model::ModelError::Refused(_))
    ));
    assert!(sion_cli::chatgpt::logout().unwrap());
    assert!(!sion_cli::chatgpt::logout().unwrap());
    std::fs::remove_dir_all(&home).unwrap();
}
