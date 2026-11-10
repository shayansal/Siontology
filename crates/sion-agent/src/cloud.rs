//! Models on the cloud platforms, and the providers that speak OpenAI's
//! API under their own names.
//!
//! - **AWS Bedrock** ([`Bedrock`]): the Converse API, `POST
//!   /model/<model>/converse`, signed with Signature Version 4
//!   ([`SigV4`]: an access key and its secret, with a session token if the
//!   credentials are temporary) or carrying a Bedrock API key as a bearer.
//!   Text, pictures, tool calls and their results (screenshots among
//!   them), and Claude's thinking, signed, go both ways; Claude's and
//!   Nova's prompts are cached at two points (after the system prompt and
//!   at the end of the conversation). Bedrock's own framing of a streamed
//!   answer is not read: an answer comes whole.
//! - **Google Vertex AI** ([`Vertex`]): Gemini through `generateContent`
//!   and Claude through `rawPredict`, in the project and location given,
//!   both streamed as Server-Sent Events. A request carries a bearer
//!   token, given or minted ([`GoogleCredentials`]: a service account's
//!   key, a JWT signed with RS256, or a person's own sign-in kept by
//!   `gcloud auth application-default login`, its refresh token) and kept
//!   until shortly before it lapses; or, for Gemini in Vertex AI's express
//!   mode, an API key.
//! - **Azure OpenAI** is OpenAI's API at a resource's `/openai/v1`, its key
//!   in an `api-key` header
//!   ([`crate::providers::OpenAiCompatible::key_header`]).
//! - **Named providers** ([`NAMED`]): OpenRouter, DeepSeek, Moonshot's
//!   Kimi, xAI, Groq, Together, Fireworks, Mistral and Cerebras, each with
//!   its own base URL and key variable, so that several can be mixed in
//!   one agent's models.

use std::sync::Mutex;

use aws_lc_rs::{digest, hmac, rand, signature};
use serde_json::{json, Value};

use crate::encoding::{base64_url, from_base64, hex};
use crate::mcp_http::Events;
use crate::model::{
    most_tokens, Block, Delta, Image, Message, Model, ModelError, Request, Response, Role, Stop,
    Thinking, Usage,
};
use crate::providers::{
    anthropic_response, anthropic_stream, claude_thinking, claude_thinks, gemini_response,
    gemini_stream, http_error, kept_or_not, post_stream, streams, whole, Anthropic, Gemini, Keys,
    Pictures, IMAGE_NOT_SHOWN, MAX_ANSWER, MAX_EVENT,
};

/// A provider that speaks OpenAI's chat completions under its own name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Named {
    /// The name a model is written with: `<name>:<model>`.
    pub name: &'static str,
    /// Its API's base URL.
    pub base: &'static str,
    /// The variable its key is in.
    pub key: &'static str,
    /// The variable that changes its base URL.
    pub base_variable: &'static str,
}

/// The providers known by name.
pub const NAMED: &[Named] = &[
    Named {
        name: "openrouter",
        base: "https://openrouter.ai/api/v1",
        key: "OPENROUTER_API_KEY",
        base_variable: "OPENROUTER_BASE_URL",
    },
    Named {
        name: "deepseek",
        base: "https://api.deepseek.com/v1",
        key: "DEEPSEEK_API_KEY",
        base_variable: "DEEPSEEK_BASE_URL",
    },
    Named {
        name: "moonshot",
        base: "https://api.moonshot.ai/v1",
        key: "MOONSHOT_API_KEY",
        base_variable: "MOONSHOT_BASE_URL",
    },
    Named {
        name: "xai",
        base: "https://api.x.ai/v1",
        key: "XAI_API_KEY",
        base_variable: "XAI_BASE_URL",
    },
    Named {
        name: "groq",
        base: "https://api.groq.com/openai/v1",
        key: "GROQ_API_KEY",
        base_variable: "GROQ_BASE_URL",
    },
    Named {
        name: "together",
        base: "https://api.together.xyz/v1",
        key: "TOGETHER_API_KEY",
        base_variable: "TOGETHER_BASE_URL",
    },
    Named {
        name: "fireworks",
        base: "https://api.fireworks.ai/inference/v1",
        key: "FIREWORKS_API_KEY",
        base_variable: "FIREWORKS_BASE_URL",
    },
    Named {
        name: "mistral",
        base: "https://api.mistral.ai/v1",
        key: "MISTRAL_API_KEY",
        base_variable: "MISTRAL_BASE_URL",
    },
    Named {
        name: "cerebras",
        base: "https://api.cerebras.ai/v1",
        key: "CEREBRAS_API_KEY",
        base_variable: "CEREBRAS_BASE_URL",
    },
];

/// The provider named `name`, if one is.
pub fn named(name: &str) -> Option<&'static Named> {
    NAMED.iter().find(|n| n.name == name)
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex(digest::digest(&digest::SHA256, bytes).as_ref())
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let key = hmac::Key::new(hmac::HMAC_SHA256, key);
    hmac::sign(&key, data).as_ref().to_vec()
}

/// `text` percent-encoded as AWS's canonical requests and HTML forms take
/// it: every byte but the unreserved ones (letters, digits, `-`, `_`, `.`,
/// `~`) and, when `slash`, `/`.
pub fn percent_encode(text: &str, slash: bool) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~')
            || (slash && byte == b'/');
        if keep {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// AWS's Signature Version 4, for one service in one region.
#[derive(Clone, PartialEq, Eq)]
pub struct SigV4 {
    /// The access key's id.
    pub access_key: String,
    /// Its secret.
    pub secret_key: String,
    /// The session token of temporary credentials.
    pub session_token: Option<String>,
    /// The region, `us-east-1`.
    pub region: String,
    /// The service, `bedrock` for Bedrock's runtime.
    pub service: String,
}

impl std::fmt::Debug for SigV4 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SigV4({} in {})", self.service, self.region)
    }
}

/// `t` (seconds since the epoch) as SigV4 dates it: `20261107T040030Z`.
pub fn amz_date(t: u64) -> String {
    crate::schedule::rfc3339(t).replace(['-', ':'], "")
}

impl SigV4 {
    /// The headers that sign `POST https://<host><path>` with `body` of
    /// `content_type` at time `t`: `X-Amz-Date`, the session token if
    /// there is one, and `Authorization`. `path` is as it is sent, already
    /// percent-encoded; the canonical request encodes it once more, as AWS
    /// asks of every service but S3.
    pub fn sign(
        &self,
        host: &str,
        path: &str,
        content_type: &str,
        body: &[u8],
        t: u64,
    ) -> Vec<(String, String)> {
        let stamp = amz_date(t);
        let day = &stamp[..8];
        let mut signed: Vec<(&str, String)> = vec![
            ("content-type", content_type.trim().to_string()),
            ("host", host.trim().to_lowercase()),
            ("x-amz-date", stamp.clone()),
        ];
        if let Some(token) = &self.session_token {
            signed.push(("x-amz-security-token", token.trim().to_string()));
        }
        signed.sort_by(|a, b| a.0.cmp(b.0));
        let names = signed.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(";");
        let headers: String = signed.iter().map(|(n, v)| format!("{n}:{v}\n")).collect();
        let canonical = format!(
            "POST\n{}\n\n{headers}\n{names}\n{}",
            percent_encode(path, true),
            sha256_hex(body)
        );
        let scope = format!("{day}/{}/{}/aws4_request", self.region, self.service);
        let to_sign = format!(
            "AWS4-HMAC-SHA256\n{stamp}\n{scope}\n{}",
            sha256_hex(canonical.as_bytes())
        );
        let mut key = hmac_sha256(
            format!("AWS4{}", self.secret_key).as_bytes(),
            day.as_bytes(),
        );
        for part in [self.region.as_str(), self.service.as_str(), "aws4_request"] {
            key = hmac_sha256(&key, part.as_bytes());
        }
        let signature = hex(&hmac_sha256(&key, to_sign.as_bytes()));
        let mut out = vec![("X-Amz-Date".to_string(), stamp)];
        if let Some(token) = &self.session_token {
            out.push(("X-Amz-Security-Token".to_string(), token.clone()));
        }
        out.push((
            "Authorization".to_string(),
            format!(
                "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={names}, Signature={signature}",
                self.access_key
            ),
        ));
        out
    }
}

/// How a request to Bedrock is authorized.
#[derive(Debug)]
pub enum BedrockAuth {
    /// A Bedrock API key, as a bearer (several, comma-separated, in turn).
    Key(Keys),
    /// An access key, signed with SigV4.
    Signed(SigV4),
}

/// AWS Bedrock's Converse API.
#[derive(Debug)]
pub struct Bedrock {
    client: sion_http::Client,
    /// The region.
    pub region: String,
    /// The model, an inference profile or its ARN:
    /// `anthropic.claude-sonnet-4-5-20250929-v1:0`,
    /// `us.anthropic.claude-sonnet-4-5-20250929-v1:0`.
    pub model: String,
    /// The endpoint, `https://bedrock-runtime.<region>.amazonaws.com` by
    /// default.
    pub base: String,
    auth: BedrockAuth,
    /// How hard Claude is asked to think, if said; other models are not
    /// asked.
    pub thinking: Option<Thinking>,
    /// Whether the prompt is asked to be cached: by default for Claude's
    /// and Nova's models, which cache.
    pub cache: bool,
    /// The time, for signatures.
    pub now: fn() -> u64,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// What a picture's media type is in Converse's `format`.
fn image_format(media_type: &str) -> &'static str {
    match media_type {
        "image/jpeg" | "image/jpg" => "jpeg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "png",
    }
}

/// Whether `model` on Bedrock is one of Anthropic's.
fn bedrock_claude(model: &str) -> bool {
    model.contains("anthropic.claude")
}

impl Bedrock {
    /// A provider for `model` in `region`, authorized by `auth`.
    pub fn new(client: sion_http::Client, region: &str, model: &str, auth: BedrockAuth) -> Bedrock {
        Bedrock {
            client,
            region: region.into(),
            model: model.into(),
            base: format!("https://bedrock-runtime.{region}.amazonaws.com"),
            auth,
            thinking: None,
            cache: bedrock_claude(model) || model.contains("amazon.nova"),
            now,
        }
    }

    /// Where its reasoning comes from, and the only reasoning it is given
    /// back: its kind, endpoint and model.
    pub fn source(&self) -> String {
        format!("bedrock {} {}", self.base.trim_end_matches('/'), self.model)
    }

    /// The path of a Converse request for its model, as sent.
    pub fn path(&self) -> String {
        format!("/model/{}/converse", percent_encode(&self.model, false))
    }

    /// How hard it thinks: as asked, for Claude.
    fn thinks(&self) -> Option<Thinking> {
        if bedrock_claude(&self.model) {
            claude_thinks(self.thinking)
        } else {
            None
        }
    }

    /// The Converse request for `request`.
    pub fn body(&self, request: &Request) -> Value {
        let from = self.source();
        let mut pictures = Pictures::of(&request.messages, &request.images);
        let mut messages: Vec<Value> = request
            .messages
            .iter()
            .map(|m| converse_message(m, &mut pictures, &from))
            .collect();
        let most = self.most_tokens(request.max_tokens);
        let mut system = Vec::new();
        if !request.system.trim().is_empty() {
            system.push(json!({"text": request.system}));
        }
        if self.cache {
            let point = json!({"cachePoint": {"type": "default"}});
            if !system.is_empty() {
                system.push(point.clone());
            }
            if let Some(content) = messages
                .last_mut()
                .and_then(|m| m["content"].as_array_mut())
            {
                content.push(point);
            }
        }
        let mut body = json!({
            "messages": messages,
            "inferenceConfig": {"maxTokens": most},
        });
        if !system.is_empty() {
            body["system"] = json!(system);
        }
        if !request.tools.is_empty() {
            let tools: Vec<Value> = request
                .tools
                .iter()
                .map(|t| {
                    json!({"toolSpec": {
                        "name": t.name,
                        "description": t.description,
                        "inputSchema": {"json": t.input_schema},
                    }})
                })
                .collect();
            body["toolConfig"] = json!({ "tools": tools });
        }
        let fields: serde_json::Map<String, Value> =
            claude_thinking(&self.model, self.thinks(), most, false)
                .into_iter()
                .map(|(field, value)| (field.to_string(), value))
                .collect();
        if !fields.is_empty() {
            body["additionalModelRequestFields"] = Value::Object(fields);
        }
        body
    }

    fn complete_once(&self, request: &Request) -> Result<Response, ModelError> {
        let body = serde_json::to_vec(&self.body(request))
            .map_err(|e| ModelError::Protocol(e.to_string()))?;
        let url = format!("{}{}", self.base.trim_end_matches('/'), self.path());
        let answer = match &self.auth {
            BedrockAuth::Key(keys) => keys.send(|key| {
                let key = key.ok_or_else(|| ModelError::Refused("no Bedrock API key".into()))?;
                let bearer = format!("Bearer {key}");
                self.client
                    .post_json(
                        &url,
                        &[("Authorization", bearer.as_str())],
                        &body,
                        MAX_ANSWER,
                    )
                    .map_err(http_error)
            })?,
            BedrockAuth::Signed(signer) => {
                let headers = signer.sign(
                    &host_of(&self.base),
                    &self.path(),
                    "application/json",
                    &body,
                    (self.now)(),
                );
                let headers: Vec<(&str, &str)> = headers
                    .iter()
                    .map(|(n, v)| (n.as_str(), v.as_str()))
                    .collect();
                self.client
                    .post_json(&url, &headers, &body, MAX_ANSWER)
                    .map_err(http_error)?
            }
        };
        let value: Value = serde_json::from_slice(&answer.body)
            .map_err(|e| ModelError::Protocol(format!("answer is not JSON: {e}")))?;
        converse_response(&value, &self.model)
    }
}

/// The host (with its port, if any) of `base`.
fn host_of(base: &str) -> String {
    let rest = base.split_once("://").map_or(base, |(_, rest)| rest);
    rest.split('/').next().unwrap_or_default().to_string()
}

fn converse_image(image: &Image, data: &str) -> Value {
    json!({"image": {
        "format": image_format(&image.media_type),
        "source": {"bytes": data},
    }})
}

/// A message as Converse takes it. Converse takes no empty text, so text
/// with nothing in it is left out, and a message or a tool's result that
/// would be empty says so.
fn converse_message(message: &Message, pictures: &mut Pictures<'_>, from: &str) -> Value {
    let mut content = Vec::new();
    for block in &message.content {
        match block {
            Block::Text { text } => {
                if !text.trim().is_empty() {
                    content.push(json!({ "text": text }));
                }
            }
            Block::ToolUse {
                id, name, input, ..
            } => content.push(json!({
                "toolUse": {"toolUseId": id, "name": name, "input": input}
            })),
            Block::ToolResult {
                id,
                content: text,
                is_error,
                images,
            } => {
                let mut parts = Vec::new();
                if !text.trim().is_empty() {
                    parts.push(json!({ "text": text }));
                }
                for image in images {
                    parts.push(match pictures.next(image) {
                        Some(data) => converse_image(image, data),
                        None => json!({ "text": IMAGE_NOT_SHOWN }),
                    });
                }
                if parts.is_empty() {
                    parts.push(json!({"text": "(nothing)"}));
                }
                let mut result = json!({"toolUseId": id, "content": parts});
                if *is_error {
                    result["status"] = json!("error");
                }
                content.push(json!({ "toolResult": result }));
            }
            Block::Image { image } => content.push(match pictures.next(image) {
                Some(data) => converse_image(image, data),
                None => json!({ "text": IMAGE_NOT_SHOWN }),
            }),
            Block::Reasoning { from: kept, data } if kept == from => content.push(data.clone()),
            Block::Reasoning { .. } => {}
        }
    }
    if content.is_empty() {
        content.push(json!({"text": "(nothing)"}));
    }
    json!({
        "role": match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        },
        "content": content,
    })
}

/// A reasoning block of Converse's, kept if it can be given back: signed
/// reasoning, or redacted. Marked as its provider's by [`kept_or_not`].
fn converse_reasoning(block: &Value) -> Option<Block> {
    let reasoning = &block["reasoningContent"];
    let signed = reasoning["reasoningText"]["signature"]
        .as_str()
        .is_some_and(|s| !s.is_empty());
    let redacted = reasoning["redactedContent"]
        .as_str()
        .is_some_and(|r| !r.is_empty());
    (signed || redacted).then(|| Block::Reasoning {
        from: String::new(),
        data: block.clone(),
    })
}

/// Converse's answer, for `model`, as a [`Response`].
pub fn converse_response(answer: &Value, model: &str) -> Result<Response, ModelError> {
    let blocks = answer["output"]["message"]["content"]
        .as_array()
        .ok_or_else(|| ModelError::Protocol(format!("no message in the answer: {answer}")))?;
    let mut content = Vec::new();
    for block in blocks {
        if let Some(text) = block["text"].as_str() {
            content.push(Block::text(text));
        } else if let Some(call) = block.get("toolUse") {
            content.push(Block::ToolUse {
                id: call["toolUseId"].as_str().unwrap_or_default().to_string(),
                name: call["name"].as_str().unwrap_or_default().to_string(),
                input: call.get("input").cloned().unwrap_or_else(|| json!({})),
                signature: None,
            });
        } else if let Some(reasoning) = converse_reasoning(block) {
            content.push(reasoning);
        }
    }
    let usage = &answer["usage"];
    let count = |key: &str| usage[key].as_u64().unwrap_or(0);
    let (read, written) = (
        count("cacheReadInputTokens"),
        count("cacheWriteInputTokens"),
    );
    let stop = match answer["stopReason"].as_str() {
        Some("end_turn" | "stop_sequence") | None => Stop::EndTurn,
        Some("tool_use") => Stop::ToolUse,
        Some("max_tokens" | "model_context_window_exceeded") => Stop::MaxTokens,
        Some("guardrail_intervened" | "content_filtered") => Stop::Refusal,
        Some(other) => Stop::Other(other.to_string()),
    };
    Ok(Response {
        content,
        stop,
        usage: Usage {
            // Converse counts the cached tokens apart from the rest.
            input: count("inputTokens") + read + written,
            output: count("outputTokens"),
            cache_read: read,
            cache_write: written,
        },
        model: model.to_string(),
        forgot_reasoning: false,
    })
}

impl Model for Bedrock {
    fn id(&self) -> &str {
        &self.model
    }

    fn complete(&self, request: &Request) -> Result<Response, ModelError> {
        kept_or_not(request, &self.source(), |request| {
            self.complete_once(request)
        })
    }

    fn most_tokens(&self, max_tokens: u32) -> u32 {
        if bedrock_claude(&self.model) {
            crate::providers::claude_most(&self.model, self.thinking, max_tokens)
        } else {
            most_tokens(self.thinks(), max_tokens)
        }
    }
}

/// What mints Google's tokens.
enum Grant {
    /// A service account's key: a JWT it signs is exchanged for a token.
    Account {
        email: String,
        key_id: String,
        /// The private key, PKCS #8 DER.
        key: Vec<u8>,
    },
    /// A person's own sign-in: its refresh token is exchanged.
    User {
        client_id: String,
        client_secret: String,
        refresh_token: String,
    },
}

/// Google credentials, as their JSON file holds them, and the token last
/// minted with them: a service account's key (`"type":
/// "service_account"`), or what `gcloud auth application-default login`
/// keeps (`"type": "authorized_user"`).
pub struct GoogleCredentials {
    grant: Grant,
    /// Where tokens are minted.
    pub token_uri: String,
    client: sion_http::Client,
    minted: Mutex<Option<(String, u64)>>,
    /// The time, for the claims and the token's life.
    pub now: fn() -> u64,
}

impl std::fmt::Debug for GoogleCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GoogleCredentials({})", self.who())
    }
}

/// The scope Vertex AI's tokens are minted for.
pub const CLOUD_PLATFORM: &str = "https://www.googleapis.com/auth/cloud-platform";

/// Where Google mints tokens, unless the credentials say.
pub const GOOGLE_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";

/// The bytes of a PEM block's body.
pub fn pem_body(pem: &str) -> Result<Vec<u8>, String> {
    let body: String = pem
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("-----"))
        .collect();
    from_base64(&body).ok_or_else(|| "the key is not PEM".to_string())
}

impl GoogleCredentials {
    /// The credentials a file's JSON holds.
    pub fn from_json(client: sion_http::Client, text: &str) -> Result<GoogleCredentials, String> {
        let file: Value =
            serde_json::from_str(text).map_err(|e| format!("not Google's credentials: {e}"))?;
        let field = |name: &str| {
            file[name]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("the credentials have no {name}"))
        };
        let grant = match file["type"].as_str() {
            Some("service_account") => {
                let key = pem_body(&field("private_key")?)?;
                signature::RsaKeyPair::from_pkcs8(&key)
                    .map_err(|_| "the service account's private key is not an RSA key")?;
                Grant::Account {
                    email: field("client_email")?,
                    key_id: file["private_key_id"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    key,
                }
            }
            Some("authorized_user") => Grant::User {
                client_id: field("client_id")?,
                client_secret: field("client_secret")?,
                refresh_token: field("refresh_token")?,
            },
            Some(other) => {
                return Err(format!(
                    "credentials of type {other} are not read here: give a service account's \
                     key, or sign in with gcloud auth application-default login"
                ))
            }
            None => return Err("not Google's credentials: they say no type".into()),
        };
        Ok(GoogleCredentials {
            grant,
            token_uri: file["token_uri"]
                .as_str()
                .unwrap_or(GOOGLE_TOKEN_URI)
                .to_string(),
            client,
            minted: Mutex::new(None),
            now,
        })
    }

    /// Whose they are: the service account's address, or a person's.
    pub fn who(&self) -> String {
        match &self.grant {
            Grant::Account { email, .. } => email.clone(),
            Grant::User { .. } => "a person's sign-in".into(),
        }
    }

    /// A JWT asking for a token for `scope` at time `t`, signed with the
    /// service account's key (RS256): none for a person's sign-in.
    pub fn assertion(&self, scope: &str, t: u64) -> Result<String, String> {
        let Grant::Account { email, key_id, key } = &self.grant else {
            return Err("a person's sign-in signs no assertion".into());
        };
        let header = json!({"alg": "RS256", "typ": "JWT", "kid": key_id});
        let claims = json!({
            "iss": email,
            "scope": scope,
            "aud": self.token_uri,
            "iat": t,
            "exp": t + 3_600,
        });
        let signing = format!(
            "{}.{}",
            base64_url(header.to_string().as_bytes()),
            base64_url(claims.to_string().as_bytes())
        );
        let pair = signature::RsaKeyPair::from_pkcs8(key)
            .map_err(|_| "the private key is not an RSA key".to_string())?;
        let mut signed = vec![0u8; pair.public_modulus_len()];
        pair.sign(
            &signature::RSA_PKCS1_SHA256,
            &rand::SystemRandom::new(),
            signing.as_bytes(),
            &mut signed,
        )
        .map_err(|_| "the assertion could not be signed".to_string())?;
        Ok(format!("{signing}.{}", base64_url(&signed)))
    }

    /// The form that asks for a token at time `t`.
    fn form(&self, t: u64) -> Result<String, String> {
        Ok(match &self.grant {
            Grant::Account { .. } => format!(
                "grant_type={}&assertion={}",
                percent_encode("urn:ietf:params:oauth:grant-type:jwt-bearer", false),
                percent_encode(&self.assertion(CLOUD_PLATFORM, t)?, false)
            ),
            Grant::User {
                client_id,
                client_secret,
                refresh_token,
            } => format!(
                "grant_type=refresh_token&client_id={}&client_secret={}&refresh_token={}",
                percent_encode(client_id, false),
                percent_encode(client_secret, false),
                percent_encode(refresh_token, false)
            ),
        })
    }

    /// A token for Vertex AI: the one minted last if it has more than a
    /// minute left, or a new one.
    pub fn token(&self) -> Result<String, String> {
        let t = (self.now)();
        let kept = self
            .minted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some((token, until)) = kept {
            if until > t + 60 {
                return Ok(token);
            }
        }
        let form = self.form(t)?;
        let answer = self
            .client
            .request(
                "POST",
                &self.token_uri,
                &[("Content-Type", "application/x-www-form-urlencoded")],
                Some(form.as_bytes()),
                1 << 20,
            )
            .map_err(|e| format!("Google's token endpoint: {e}"))?;
        let value: Value = serde_json::from_slice(&answer.body)
            .map_err(|e| format!("Google's token endpoint answered what is not JSON: {e}"))?;
        let token = value["access_token"]
            .as_str()
            .filter(|t| !t.is_empty())
            .ok_or_else(|| "Google's token endpoint gave no token".to_string())?
            .to_string();
        let lasts = value["expires_in"].as_u64().unwrap_or(3_600);
        *self.minted.lock().unwrap_or_else(|e| e.into_inner()) = Some((token.clone(), t + lasts));
        Ok(token)
    }
}

/// How a request to Vertex AI is authorized.
pub enum VertexToken {
    /// A token given (`gcloud auth print-access-token`).
    Given(String),
    /// Tokens minted with credentials, renewed as they lapse.
    Minted(Box<GoogleCredentials>),
    /// An API key of Vertex AI's express mode, for Gemini.
    Key(String),
}

impl std::fmt::Debug for VertexToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VertexToken::Given(_) => f.write_str("VertexToken::Given"),
            VertexToken::Minted(credentials) => {
                write!(f, "VertexToken::Minted({})", credentials.who())
            }
            VertexToken::Key(_) => f.write_str("VertexToken::Key"),
        }
    }
}

impl VertexToken {
    /// The header a request carries.
    fn header(&self) -> Result<(&'static str, String), ModelError> {
        Ok(match self {
            VertexToken::Given(token) => ("Authorization", format!("Bearer {token}")),
            VertexToken::Minted(credentials) => (
                "Authorization",
                format!(
                    "Bearer {}",
                    credentials.token().map_err(ModelError::Refused)?
                ),
            ),
            VertexToken::Key(key) => ("x-goog-api-key", key.clone()),
        })
    }
}

/// A model on Google Vertex AI: Gemini, or Claude.
#[derive(Debug)]
pub struct Vertex {
    client: sion_http::Client,
    /// The project (none in express mode).
    pub project: String,
    /// The location: a region, or `global`.
    pub location: String,
    /// The endpoint: `https://<location>-aiplatform.googleapis.com`, or
    /// `https://aiplatform.googleapis.com` for `global` and express mode.
    pub base: String,
    /// The model, `gemini-3-pro-preview` or `claude-opus-5-5`.
    pub model: String,
    token: VertexToken,
    /// How hard it is asked to think, if said.
    pub thinking: Option<Thinking>,
}

/// The `anthropic_version` Vertex AI's Claude takes.
pub const VERTEX_ANTHROPIC_VERSION: &str = "vertex-2023-10-16";

impl Vertex {
    /// A provider for `model` in `project` at `location`.
    pub fn new(
        client: sion_http::Client,
        project: &str,
        location: &str,
        model: &str,
        token: VertexToken,
    ) -> Vertex {
        let base = if location == "global" || matches!(token, VertexToken::Key(_)) {
            "https://aiplatform.googleapis.com".to_string()
        } else {
            format!("https://{location}-aiplatform.googleapis.com")
        };
        Vertex {
            client,
            project: project.into(),
            location: location.into(),
            base,
            model: model.into(),
            token,
            thinking: None,
        }
    }

    /// Whether the model is Claude (Anthropic's, as Vertex AI publishes
    /// it) rather than Gemini.
    pub fn claude(&self) -> bool {
        self.model.starts_with("claude")
    }

    /// The URL of a request, streamed or whole.
    pub fn url(&self, stream: bool) -> String {
        let (publisher, method) = match (self.claude(), stream) {
            (true, false) => ("anthropic", "rawPredict"),
            (true, true) => ("anthropic", "streamRawPredict"),
            (false, false) => ("google", "generateContent"),
            (false, true) => ("google", "streamGenerateContent?alt=sse"),
        };
        let base = self.base.trim_end_matches('/');
        if matches!(self.token, VertexToken::Key(_)) {
            // Express mode: no project.
            format!(
                "{base}/v1/publishers/{publisher}/models/{}:{method}",
                self.model
            )
        } else {
            format!(
                "{base}/v1/projects/{}/locations/{}/publishers/{publisher}/models/{}:{method}",
                self.project, self.location, self.model
            )
        }
    }

    /// Where its reasoning comes from, and the only reasoning it is given
    /// back: its kind, endpoint, project, location and model.
    pub fn source(&self) -> String {
        format!(
            "vertex {} {} {} {}",
            self.base.trim_end_matches('/'),
            self.project,
            self.location,
            self.model
        )
    }

    /// The request body: Gemini's, or Anthropic's without its model, with
    /// Vertex AI's `anthropic_version`, and its prompt cached at two
    /// breakpoints ([`cache_breakpoints`]).
    pub fn body(&self, request: &Request) -> Value {
        let from = self.source();
        if self.claude() {
            let mut shaped = Anthropic::new(self.client.clone(), &self.model, "").unbound();
            shaped.thinking = self.thinking;
            shaped.cache = false;
            let mut body = shaped.body_from(request, &from);
            if let Some(fields) = body.as_object_mut() {
                fields.remove("model");
                fields.insert("anthropic_version".into(), json!(VERTEX_ANTHROPIC_VERSION));
            }
            cache_breakpoints(&mut body);
            body
        } else {
            let mut shaped = Gemini::new(self.client.clone(), &self.model, "");
            shaped.thinking = self.thinking;
            shaped.body_from(request, &from)
        }
    }

    fn complete_once(&self, request: &Request) -> Result<Response, ModelError> {
        let body = serde_json::to_vec(&self.body(request))
            .map_err(|e| ModelError::Protocol(e.to_string()))?;
        let (name, value) = self.token.header()?;
        let answer = self
            .client
            .post_json(
                &self.url(false),
                &[(name, value.as_str())],
                &body,
                MAX_ANSWER,
            )
            .map_err(http_error)?;
        let value: Value = serde_json::from_slice(&answer.body)
            .map_err(|e| ModelError::Protocol(format!("answer is not JSON: {e}")))?;
        if self.claude() {
            anthropic_response(&value)
        } else {
            gemini_response(&value)
        }
    }

    fn stream_once(
        &self,
        request: &Request,
        seen: &mut dyn FnMut(Delta) -> bool,
    ) -> Result<Response, ModelError> {
        let mut body = self.body(request);
        if self.claude() {
            body["stream"] = json!(true);
        }
        let body = serde_json::to_vec(&body).map_err(|e| ModelError::Protocol(e.to_string()))?;
        let (name, value) = self.token.header()?;
        let mut response = post_stream(
            &self.client,
            &self.url(true),
            &[(name, value.as_str())],
            &body,
        )?;
        if !streams(&response) {
            let read = if self.claude() {
                anthropic_response
            } else {
                gemini_response
            };
            return whole(response, read, seen);
        }
        let mut events = Events::new(response.body(), MAX_EVENT);
        if self.claude() {
            anthropic_stream(&mut events, seen)
        } else {
            gemini_stream(&mut events, seen)
        }
    }
}

/// Claude's prompt cached as every platform takes it: a breakpoint on the
/// system prompt and one on the conversation's last block, when it can
/// take one.
fn cache_breakpoints(body: &mut Value) {
    let ephemeral = json!({"type": "ephemeral"});
    let system = body["system"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string);
    if let Some(system) = system {
        body["system"] = json!([{"type": "text", "text": system, "cache_control": ephemeral}]);
    }
    let last = body["messages"]
        .as_array_mut()
        .and_then(|messages| messages.last_mut())
        .and_then(|message| message["content"].as_array_mut())
        .and_then(|content| content.last_mut());
    if let Some(last) = last {
        let takes = match last["type"].as_str() {
            Some("text") => last["text"].as_str().is_some_and(|t| !t.is_empty()),
            Some("image" | "tool_result" | "tool_use") => true,
            _ => false,
        };
        if takes {
            last["cache_control"] = ephemeral;
        }
    }
}

impl Model for Vertex {
    fn id(&self) -> &str {
        &self.model
    }

    fn complete(&self, request: &Request) -> Result<Response, ModelError> {
        kept_or_not(request, &self.source(), |request| {
            self.complete_once(request)
        })
    }

    fn stream(
        &self,
        request: &Request,
        seen: &mut dyn FnMut(Delta) -> bool,
    ) -> Result<Response, ModelError> {
        kept_or_not(request, &self.source(), |request| {
            self.stream_once(request, seen)
        })
    }

    fn most_tokens(&self, max_tokens: u32) -> u32 {
        if self.claude() {
            crate::providers::claude_most(&self.model, self.thinking, max_tokens)
        } else {
            most_tokens(self.thinking, max_tokens)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ToolSpec;

    fn client() -> sion_http::Client {
        sion_http::Client::new(sion_http::Options {
            allow_private: true,
            proxy_from_env: false,
            ..sion_http::Options::default()
        })
        .unwrap()
    }

    #[test]
    fn a_bedrock_request_is_signed_as_aws_signs_it() {
        // The signatures botocore computes for this request at this time.
        let body = br#"{"messages":[{"role":"user","content":[{"text":"Hello"}]}],"inferenceConfig":{"maxTokens":64}}"#;
        let mut signer = SigV4 {
            access_key: "AKIDEXAMPLE".into(),
            secret_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
            session_token: None,
            region: "us-east-1".into(),
            service: "bedrock".into(),
        };
        let path = format!(
            "/model/{}/converse",
            percent_encode("anthropic.claude-sonnet-4-5-20250929-v1:0", false)
        );
        assert_eq!(
            path,
            "/model/anthropic.claude-sonnet-4-5-20250929-v1%3A0/converse"
        );
        let host = "bedrock-runtime.us-east-1.amazonaws.com";
        let headers = signer.sign(host, &path, "application/json", body, 1_794_024_030);
        assert_eq!(headers[0], ("X-Amz-Date".into(), "20261107T040030Z".into()));
        assert_eq!(
            headers.last().unwrap().1,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261107/us-east-1/bedrock/aws4_request, \
             SignedHeaders=content-type;host;x-amz-date, \
             Signature=beb07ec935d679d84223d76f236729ad0f8e39d61ae78ccc8ace61367f9578fb"
        );
        signer.session_token = Some("SESSIONTOKEN123".into());
        let headers = signer.sign(host, &path, "application/json", body, 1_794_024_030);
        assert_eq!(
            headers[1],
            ("X-Amz-Security-Token".into(), "SESSIONTOKEN123".into())
        );
        assert!(headers[2].1.ends_with(
            "SignedHeaders=content-type;host;x-amz-date;x-amz-security-token, \
             Signature=095f66e519f9d1216f2c6514cdaac048fbb6b1b78ed1700b32bddce28c8d6733"
        ));
    }

    fn request() -> Request {
        let mut images = std::collections::BTreeMap::new();
        images.insert("d1".to_string(), "aGk=".to_string());
        images.insert("d2".to_string(), "c2hvdA==".to_string());
        Request {
            system: "Be careful.".into(),
            messages: vec![
                Message {
                    role: Role::User,
                    content: vec![
                        Block::text("Pay Acme."),
                        Block::Image {
                            image: Image {
                                digest: "d1".into(),
                                media_type: "image/jpeg".into(),
                            },
                        },
                    ],
                },
                Message {
                    role: Role::Assistant,
                    content: vec![Block::ToolUse {
                        id: "t1".into(),
                        name: "lookup".into(),
                        input: json!({"supplier": "acme"}),
                        signature: None,
                    }],
                },
                Message {
                    role: Role::User,
                    content: vec![Block::ToolResult {
                        id: "t1".into(),
                        content: "acme-ltd".into(),
                        is_error: false,
                        images: vec![Image {
                            digest: "d2".into(),
                            media_type: "image/png".into(),
                        }],
                    }],
                },
            ],
            tools: vec![ToolSpec {
                name: "lookup".into(),
                description: "Find a payee.".into(),
                input_schema: json!({"type": "object"}),
            }],
            max_tokens: 100,
            images,
        }
    }

    type Seen = Vec<(String, Vec<(String, String)>, String)>;

    /// A stand-in server: answers each request in turn with JSON, and
    /// gives back what it was sent (path, headers, body).
    fn stand_in(answers: Vec<String>) -> (String, std::thread::JoinHandle<Seen>) {
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
                    .respond(tiny_http::Response::from_string(answer).with_header(header))
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

    #[test]
    fn bedrock_takes_the_conversation_and_gives_back_its_calls() {
        let answer = json!({
            "output": {"message": {"role": "assistant", "content": [
                {"text": "Paying."},
                {"toolUse": {"toolUseId": "t2", "name": "pay", "input": {"to": "acme-ltd"}}}
            ]}},
            "stopReason": "tool_use",
            "usage": {"inputTokens": 12, "outputTokens": 5, "cacheReadInputTokens": 3}
        });
        let (base, server) = stand_in(vec![answer.to_string(), answer.to_string()]);
        let mut signed = Bedrock::new(
            client(),
            "us-east-1",
            "anthropic.claude-sonnet-4-5-20250929-v1:0",
            BedrockAuth::Signed(SigV4 {
                access_key: "AKIDEXAMPLE".into(),
                secret_key: "secret".into(),
                session_token: None,
                region: "us-east-1".into(),
                service: "bedrock".into(),
            }),
        );
        signed.base = base.clone();
        signed.now = || 1_794_024_030;
        let response = signed.complete(&request()).unwrap();
        assert_eq!(response.stop, Stop::ToolUse);
        assert_eq!(response.usage.input, 15);
        assert_eq!(response.usage.cache_read, 3);
        assert!(
            matches!(&response.content[1], Block::ToolUse { name, input, .. } if name == "pay" && input["to"] == "acme-ltd")
        );
        let mut keyed = Bedrock::new(
            client(),
            "us-east-1",
            "meta.llama4-maverick-17b-instruct-v1:0",
            BedrockAuth::Key(Keys::parse("bedrock-key")),
        );
        keyed.base = base;
        keyed.complete(&request()).unwrap();
        let seen = server.join().unwrap();
        let (path, headers, body) = &seen[0];
        assert_eq!(
            path,
            "/model/anthropic.claude-sonnet-4-5-20250929-v1%3A0/converse"
        );
        assert!(header(headers, "authorization").starts_with(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261107/us-east-1/bedrock/aws4_request"
        ));
        assert_eq!(header(headers, "x-amz-date"), "20261107T040030Z");
        let body: Value = serde_json::from_str(body).unwrap();
        assert_eq!(body["system"][0]["text"], "Be careful.");
        assert_eq!(body["system"][1]["cachePoint"]["type"], "default");
        let first = &body["messages"][0]["content"];
        assert_eq!(first[1]["image"]["format"], "jpeg");
        assert_eq!(first[1]["image"]["source"]["bytes"], "aGk=");
        assert_eq!(
            body["messages"][1]["content"][0]["toolUse"]["name"],
            "lookup"
        );
        let result = &body["messages"][2]["content"][0]["toolResult"];
        assert_eq!(result["content"][0]["text"], "acme-ltd");
        assert_eq!(result["content"][1]["image"]["source"]["bytes"], "c2hvdA==");
        assert_eq!(
            body["messages"][2]["content"][1]["cachePoint"]["type"],
            "default"
        );
        assert_eq!(
            body["toolConfig"]["tools"][0]["toolSpec"]["inputSchema"]["json"]["type"],
            "object"
        );
        // Another model: a bearer, and no cache points.
        let (path, headers, body) = &seen[1];
        assert_eq!(
            path,
            "/model/meta.llama4-maverick-17b-instruct-v1%3A0/converse"
        );
        assert_eq!(header(headers, "authorization"), "Bearer bedrock-key");
        assert!(!body.contains("cachePoint"), "{body}");
    }

    #[test]
    fn claude_thinking_on_bedrock_is_asked_for_kept_and_given_back() {
        let mut bedrock = Bedrock::new(
            client(),
            "us-west-2",
            "us.anthropic.claude-opus-4-1-20250805-v1:0",
            BedrockAuth::Key(Keys::parse("k")),
        );
        bedrock.thinking = Some(Thinking::Budget(2_048));
        let mut asked = request();
        asked.messages[1].content.insert(
            0,
            Block::Reasoning {
                from: bedrock.source(),
                data: json!({"reasoningContent": {"reasoningText": {"text": "Check first.", "signature": "sig"}}}),
            },
        );
        asked.messages[1].content.insert(
            0,
            Block::Reasoning {
                from: "another".into(),
                data: json!({"x": 1}),
            },
        );
        let body = bedrock.body(&asked);
        let thinking = &body["additionalModelRequestFields"]["thinking"];
        assert_eq!(thinking["type"], "enabled");
        assert_eq!(thinking["budget_tokens"], 2_048);
        assert!(body["inferenceConfig"]["maxTokens"].as_u64().unwrap() > 2_048);
        let given = body["messages"][1]["content"].as_array().unwrap();
        assert_eq!(given.len(), 2, "{given:?}");
        assert_eq!(
            given[0]["reasoningContent"]["reasoningText"]["signature"],
            "sig"
        );
        // Signed and redacted reasoning is kept; unsigned is not.
        let answer = json!({"output": {"message": {"content": [
            {"reasoningContent": {"reasoningText": {"text": "hm", "signature": "s"}}},
            {"reasoningContent": {"redactedContent": "cmVk"}},
            {"reasoningContent": {"reasoningText": {"text": "unsigned"}}},
            {"text": "ok"}
        ]}}, "stopReason": "end_turn", "usage": {}});
        let response = converse_response(&answer, "m").unwrap();
        assert_eq!(response.content.len(), 3);
        assert!(matches!(&response.content[1], Block::Reasoning { .. }));
        assert_eq!(response.content[2], Block::text("ok"));
        // Other models are not asked to think.
        let mut other = Bedrock::new(
            client(),
            "us-west-2",
            "amazon.nova-pro-v1:0",
            BedrockAuth::Key(Keys::parse("k")),
        );
        other.thinking = Some(Thinking::Budget(2_048));
        assert!(other.body(&request())["additionalModelRequestFields"].is_null());
    }

    #[test]
    fn vertex_reaches_gemini_and_claude_with_its_token() {
        let gemini = json!({"candidates": [{"content": {"role": "model", "parts": [{"text": "Hello."}]}, "finishReason": "STOP"}], "usageMetadata": {"promptTokenCount": 4, "candidatesTokenCount": 2}});
        let claude = json!({"id": "m", "type": "message", "role": "assistant", "content": [{"type": "text", "text": "Hi."}], "stop_reason": "end_turn", "usage": {"input_tokens": 3, "output_tokens": 1}});
        let (base, server) = stand_in(vec![
            gemini.to_string(),
            claude.to_string(),
            gemini.to_string(),
        ]);
        let mut on_gemini = Vertex::new(
            client(),
            "my-project",
            "us-central1",
            "gemini-3-pro-preview",
            VertexToken::Given("ya29.token".into()),
        );
        assert_eq!(
            on_gemini.base,
            "https://us-central1-aiplatform.googleapis.com"
        );
        on_gemini.base = base.clone();
        assert_eq!(
            on_gemini.complete(&request()).unwrap().content[0],
            Block::text("Hello.")
        );
        let mut on_claude = Vertex::new(
            client(),
            "my-project",
            "global",
            "claude-opus-5-5",
            VertexToken::Given("ya29.token".into()),
        );
        assert_eq!(on_claude.base, "https://aiplatform.googleapis.com");
        on_claude.base = base.clone();
        assert_eq!(
            on_claude.complete(&request()).unwrap().content[0],
            Block::text("Hi.")
        );
        let mut express = Vertex::new(
            client(),
            "",
            "global",
            "gemini-2.5-flash",
            VertexToken::Key("express-key".into()),
        );
        express.base = base;
        express.complete(&request()).unwrap();
        let seen = server.join().unwrap();
        assert_eq!(seen[0].0, "/v1/projects/my-project/locations/us-central1/publishers/google/models/gemini-3-pro-preview:generateContent");
        assert_eq!(header(&seen[0].1, "authorization"), "Bearer ya29.token");
        assert_eq!(seen[1].0, "/v1/projects/my-project/locations/global/publishers/anthropic/models/claude-opus-5-5:rawPredict");
        let body: Value = serde_json::from_str(&seen[1].2).unwrap();
        assert_eq!(body["anthropic_version"], VERTEX_ANTHROPIC_VERSION);
        assert!(body.get("model").is_none());
        assert!(body.get("cache_control").is_none());
        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
        let last = body["messages"][2]["content"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        assert_eq!(last["type"], "tool_result");
        assert_eq!(last["cache_control"]["type"], "ephemeral");
        assert_eq!(
            seen[2].0,
            "/v1/publishers/google/models/gemini-2.5-flash:generateContent"
        );
        assert_eq!(header(&seen[2].1, "x-goog-api-key"), "express-key");
        assert_eq!(header(&seen[2].1, "authorization"), "");
    }

    #[test]
    fn vertex_gives_back_only_what_it_was_given_itself() {
        let vertex = Vertex::new(
            client(),
            "p",
            "global",
            "claude-opus-5-5",
            VertexToken::Given("t".into()),
        );
        let mut asked = request();
        asked.messages[1].content.insert(
            0,
            Block::Reasoning {
                from: vertex.source(),
                data: json!({"type": "thinking", "thinking": "Check.", "signature": "sig"}),
            },
        );
        asked.messages[1].content.insert(
            0,
            Block::Reasoning {
                from: Anthropic::new(client(), "claude-opus-5-5", "").source(),
                data: json!({"type": "thinking", "thinking": "Elsewhere.", "signature": "other"}),
            },
        );
        let body = vertex.body(&asked);
        let given = body["messages"][1]["content"].as_array().unwrap();
        assert_eq!(given.len(), 2, "{given:?}");
        assert_eq!(given[0]["signature"], "sig");
    }

    fn key_file(base: &str) -> (Vec<u8>, String) {
        use aws_lc_rs::encoding::AsDer;
        let pair = signature::RsaKeyPair::generate(aws_lc_rs::rsa::KeySize::Rsa2048).unwrap();
        let pkcs8 = pair.as_der().unwrap().as_ref().to_vec();
        let pem = format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
            crate::encoding::base64(&pkcs8)
        );
        let file = json!({
            "type": "service_account",
            "project_id": "my-project",
            "client_email": "agent@my-project.iam.gserviceaccount.com",
            "private_key_id": "k1",
            "private_key": pem,
            "token_uri": format!("{base}/token"),
        });
        (pkcs8, file.to_string())
    }

    fn unbase64url(part: &str) -> Vec<u8> {
        let text = part.replace('-', "+").replace('_', "/");
        from_base64(&text).unwrap()
    }

    #[test]
    fn a_service_account_mints_a_token_with_a_signed_assertion_and_keeps_it() {
        let (base, server) = stand_in(vec![json!({
            "access_token": "ya29.minted", "expires_in": 3599, "token_type": "Bearer"
        })
        .to_string()]);
        let (pkcs8, file) = key_file(&base);
        let mut credentials = GoogleCredentials::from_json(client(), &file).unwrap();
        credentials.now = || 1_794_024_030;
        assert_eq!(credentials.token().unwrap(), "ya29.minted");
        // Kept: no second request is made.
        assert_eq!(credentials.token().unwrap(), "ya29.minted");
        let seen = server.join().unwrap();
        let (path, headers, form) = &seen[0];
        assert_eq!(path, "/token");
        assert_eq!(
            header(headers, "content-type"),
            "application/x-www-form-urlencoded"
        );
        let assertion = form
            .strip_prefix(
                "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion=",
            )
            .unwrap();
        // The assertion is signed by the key: its public half verifies it.
        let parts: Vec<&str> = assertion.split('.').collect();
        assert_eq!(parts.len(), 3);
        let claims: Value = serde_json::from_slice(&unbase64url(parts[1])).unwrap();
        assert_eq!(claims["iss"], "agent@my-project.iam.gserviceaccount.com");
        assert_eq!(claims["scope"], CLOUD_PLATFORM);
        assert_eq!(claims["aud"], format!("{base}/token"));
        assert_eq!(claims["exp"], 1_794_024_030u64 + 3_600);
        let pair = signature::RsaKeyPair::from_pkcs8(&pkcs8).unwrap();
        let public = signature::UnparsedPublicKey::new(
            &signature::RSA_PKCS1_2048_8192_SHA256,
            signature::KeyPair::public_key(&pair).as_ref().to_vec(),
        );
        let signed = format!("{}.{}", parts[0], parts[1]);
        assert!(public
            .verify(signed.as_bytes(), &unbase64url(parts[2]))
            .is_ok());
    }

    #[test]
    fn a_persons_sign_in_mints_with_its_refresh_token_and_renews_when_it_lapses() {
        let (base, server) = stand_in(vec![
            json!({"access_token": "ya29.first", "expires_in": 30}).to_string(),
            json!({"access_token": "ya29.second", "expires_in": 3599}).to_string(),
        ]);
        let file = json!({
            "type": "authorized_user",
            "client_id": "id.apps.googleusercontent.com",
            "client_secret": "s3cr/t",
            "refresh_token": "1//refresh",
            "token_uri": format!("{base}/token"),
        });
        let mut credentials = GoogleCredentials::from_json(client(), &file.to_string()).unwrap();
        credentials.now = || 1_794_024_030;
        assert_eq!(credentials.who(), "a person's sign-in");
        assert!(credentials.assertion(CLOUD_PLATFORM, 0).is_err());
        assert_eq!(credentials.token().unwrap(), "ya29.first");
        // It lapses within a minute: a new one.
        assert_eq!(credentials.token().unwrap(), "ya29.second");
        let seen = server.join().unwrap();
        assert_eq!(
            seen[0].2,
            "grant_type=refresh_token&client_id=id.apps.googleusercontent.com&client_secret=s3cr%2Ft&refresh_token=1%2F%2Frefresh"
        );
        let refused =
            GoogleCredentials::from_json(client(), r#"{"type": "external_account"}"#).unwrap_err();
        assert!(refused.contains("external_account"), "{refused}");
    }

    #[test]
    fn the_named_providers_have_their_own_bases_and_keys() {
        assert_eq!(
            named("openrouter").unwrap().base,
            "https://openrouter.ai/api/v1"
        );
        assert_eq!(named("deepseek").unwrap().key, "DEEPSEEK_API_KEY");
        assert!(named("nobody").is_none());
        for provider in NAMED {
            let upper = provider.name.to_uppercase();
            assert_eq!(provider.key, format!("{upper}_API_KEY"));
            assert_eq!(provider.base_variable, format!("{upper}_BASE_URL"));
            assert!(provider.base.starts_with("https://"));
        }
        let names: std::collections::BTreeSet<&str> = NAMED.iter().map(|n| n.name).collect();
        assert_eq!(names.len(), NAMED.len());
        assert_eq!(host_of("https://a.example:8443/x"), "a.example:8443");
        assert_eq!(amz_date(0), "19700101T000000Z");
        assert_eq!(percent_encode("a b/c~", true), "a%20b/c~");
    }
}
