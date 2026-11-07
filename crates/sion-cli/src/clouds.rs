//! The models of the cloud platforms and of the providers known by name,
//! as `sion` reaches them from their variables ([`crate::runtime::model`]):
//!
//! - `bedrock:<model>`: AWS Bedrock in `AWS_REGION` (or
//!   `AWS_DEFAULT_REGION`, or the profile's region, or `us-east-1`), with a
//!   Bedrock API key in `AWS_BEARER_TOKEN_BEDROCK`, or an access key:
//!   `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` and `AWS_SESSION_TOKEN`,
//!   or the profile's (`AWS_PROFILE`, by default `default`) in AWS's own
//!   files (`~/.aws/credentials`, `~/.aws/config`). The endpoint is
//!   `AWS_ENDPOINT_URL_BEDROCK_RUNTIME` if set.
//! - `vertex:<model>`: Google Vertex AI in the project
//!   `GOOGLE_CLOUD_PROJECT` (or `ANTHROPIC_VERTEX_PROJECT_ID`) and the
//!   location `GOOGLE_CLOUD_LOCATION` (or `CLOUD_ML_REGION`, by default
//!   `global`), with a token in `GOOGLE_OAUTH_ACCESS_TOKEN`, or minted with
//!   the credentials at `GOOGLE_APPLICATION_CREDENTIALS` or those `gcloud
//!   auth application-default login` keeps. Without a project, Gemini in
//!   Vertex AI's express mode, its key in `GOOGLE_API_KEY`.
//! - `azure:<deployment>`: Azure OpenAI at `AZURE_OPENAI_ENDPOINT`, its key
//!   in `AZURE_OPENAI_API_KEY`.
//! - `<name>:<model>` for each provider known by name
//!   ([`sion_agent::cloud::NAMED`]): its key in `<NAME>_API_KEY`, its base
//!   URL changed by `<NAME>_BASE_URL`.

use std::path::{Path, PathBuf};

use sion_agent::cloud::{
    Bedrock, BedrockAuth, GoogleCredentials, Named, SigV4, Vertex, VertexToken,
};
use sion_agent::providers::{Keys, OpenAiCompatible};
use sion_agent::Thinking;

use crate::error::CliError;

/// Where a variable is looked up: the environment, or what this machine
/// keeps ([`crate::credentials::var`]).
pub type Lookup<'a> = &'a dyn Fn(&str) -> Option<String>;

fn usage(why: impl Into<String>) -> CliError {
    CliError::Usage(why.into())
}

/// The value of `key` in `section` of an INI file's text, as AWS's
/// credentials and config files are written.
pub fn ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            inside = name.trim() == section;
            continue;
        }
        if let (true, Some((name, value))) = (inside, line.split_once('=')) {
            if name.trim() == key {
                return Some(value.trim().to_string()).filter(|v| !v.is_empty());
            }
        }
    }
    None
}

/// AWS's own files, as one profile reads them.
struct AwsFiles {
    profile: String,
    credentials: String,
    config: String,
}

impl AwsFiles {
    fn read(env: Lookup, home: Option<&Path>) -> AwsFiles {
        let file = |variable: &str, name: &str| {
            env(variable)
                .map(PathBuf::from)
                .or_else(|| home.map(|home| home.join(".aws").join(name)))
                .and_then(|path| std::fs::read_to_string(path).ok())
                .unwrap_or_default()
        };
        AwsFiles {
            profile: env("AWS_PROFILE").unwrap_or_else(|| "default".into()),
            credentials: file("AWS_SHARED_CREDENTIALS_FILE", "credentials"),
            config: file("AWS_CONFIG_FILE", "config"),
        }
    }

    /// `key` of the profile: in the credentials file, or in the config
    /// file (whose sections are `[profile <name>]`, but for `[default]`).
    fn value(&self, key: &str) -> Option<String> {
        let section = if self.profile == "default" {
            "default".to_string()
        } else {
            format!("profile {}", self.profile)
        };
        ini_value(&self.credentials, &self.profile, key)
            .or_else(|| ini_value(&self.config, &section, key))
    }
}

/// The region Bedrock is reached in: `AWS_REGION`, `AWS_DEFAULT_REGION`,
/// the profile's, or `us-east-1`.
pub fn aws_region(env: Lookup, home: Option<&Path>) -> String {
    env("AWS_REGION")
        .or_else(|| env("AWS_DEFAULT_REGION"))
        .or_else(|| AwsFiles::read(env, home).value("region"))
        .unwrap_or_else(|| "us-east-1".into())
}

/// An access key for `region`: the environment's, or the profile's.
pub fn aws_signer(env: Lookup, home: Option<&Path>, region: &str) -> Result<SigV4, String> {
    let signer = |access_key: String, secret_key: String, session_token: Option<String>| SigV4 {
        access_key,
        secret_key,
        session_token,
        region: region.to_string(),
        service: "bedrock".into(),
    };
    if let (Some(id), Some(secret)) = (env("AWS_ACCESS_KEY_ID"), env("AWS_SECRET_ACCESS_KEY")) {
        return Ok(signer(id, secret, env("AWS_SESSION_TOKEN")));
    }
    let files = AwsFiles::read(env, home);
    match (
        files.value("aws_access_key_id"),
        files.value("aws_secret_access_key"),
    ) {
        (Some(id), Some(secret)) => Ok(signer(id, secret, files.value("aws_session_token"))),
        _ if [
            "sso_session",
            "sso_start_url",
            "credential_process",
            "role_arn",
        ]
        .iter()
        .any(|key| files.value(key).is_some()) =>
        {
            Err(format!(
                "AWS's profile {} signs in another way (single sign-on, a role or a process): \
                 put its credentials in the environment first \
                 (eval \"$(aws configure export-credentials --profile {} --format env)\")",
                files.profile, files.profile
            ))
        }
        _ => Err(
            "set AWS_BEARER_TOKEN_BEDROCK (a Bedrock API key), or AWS_ACCESS_KEY_ID and \
             AWS_SECRET_ACCESS_KEY, or give AWS's profile its keys (aws configure)"
                .into(),
        ),
    }
}

/// `bedrock:<model>`.
pub fn bedrock(
    model: &str,
    thinking: Option<Thinking>,
    env: Lookup,
    home: Option<&Path>,
    client: sion_http::Client,
) -> Result<Bedrock, CliError> {
    let region = aws_region(env, home);
    let auth = match env("AWS_BEARER_TOKEN_BEDROCK") {
        Some(key) => BedrockAuth::Key(Keys::parse(&key)),
        None => BedrockAuth::Signed(aws_signer(env, home, &region).map_err(usage)?),
    };
    let mut provider = Bedrock::new(client, &region, model, auth);
    if let Some(base) = env("AWS_ENDPOINT_URL_BEDROCK_RUNTIME") {
        provider.base = base;
    }
    provider.thinking = thinking;
    Ok(provider)
}

/// Where `gcloud auth application-default login` keeps a person's
/// credentials.
pub fn application_default(home: &Path) -> PathBuf {
    home.join(".config")
        .join("gcloud")
        .join("application_default_credentials.json")
}

/// The token a request to Vertex AI in a project carries.
fn google_token(
    env: Lookup,
    home: Option<&Path>,
    client: sion_http::Client,
) -> Result<VertexToken, CliError> {
    if let Some(token) = env("GOOGLE_OAUTH_ACCESS_TOKEN") {
        return Ok(VertexToken::Given(token));
    }
    let path = env("GOOGLE_APPLICATION_CREDENTIALS")
        .map(PathBuf::from)
        .or_else(|| home.map(application_default).filter(|path| path.exists()))
        .ok_or_else(|| {
            usage(
                "set GOOGLE_OAUTH_ACCESS_TOKEN (gcloud auth print-access-token), or \
                 GOOGLE_APPLICATION_CREDENTIALS to a service account's key, or sign in with \
                 gcloud auth application-default login",
            )
        })?;
    let text =
        std::fs::read_to_string(&path).map_err(|e| usage(format!("{}: {e}", path.display())))?;
    let credentials = GoogleCredentials::from_json(client, &text)
        .map_err(|e| usage(format!("{}: {e}", path.display())))?;
    Ok(VertexToken::Minted(Box::new(credentials)))
}

/// `vertex:<model>`.
pub fn vertex(
    model: &str,
    thinking: Option<Thinking>,
    env: Lookup,
    home: Option<&Path>,
    client: sion_http::Client,
) -> Result<Vertex, CliError> {
    let project = env("GOOGLE_CLOUD_PROJECT").or_else(|| env("ANTHROPIC_VERTEX_PROJECT_ID"));
    let location = env("GOOGLE_CLOUD_LOCATION")
        .or_else(|| env("CLOUD_ML_REGION"))
        .unwrap_or_else(|| "global".into());
    let token = match &project {
        Some(_) => google_token(env, home, client.clone())?,
        None if model.starts_with("claude") => {
            return Err(usage(
                "Claude on Vertex AI is reached in a project: set GOOGLE_CLOUD_PROJECT",
            ))
        }
        None => VertexToken::Key(env("GOOGLE_API_KEY").ok_or_else(|| {
            usage(
                "set GOOGLE_CLOUD_PROJECT (the project Vertex AI bills), or GOOGLE_API_KEY for \
                 Vertex AI's express mode",
            )
        })?),
    };
    let mut provider = Vertex::new(
        client,
        project.as_deref().unwrap_or_default(),
        &location,
        model,
        token,
    );
    if let Some(base) = env("VERTEX_BASE_URL") {
        provider.base = base;
    }
    provider.thinking = thinking;
    Ok(provider)
}

/// Azure OpenAI's base URL for the resource at `endpoint`: its `/openai/v1`.
pub fn azure_base(endpoint: &str) -> String {
    let endpoint = endpoint.trim_end_matches('/');
    if endpoint.ends_with("/openai/v1") {
        endpoint.to_string()
    } else {
        format!("{endpoint}/openai/v1")
    }
}

/// `azure:<deployment>`.
pub fn azure(
    deployment: &str,
    thinking: Option<Thinking>,
    env: Lookup,
    client: sion_http::Client,
) -> Result<OpenAiCompatible, CliError> {
    let (Some(endpoint), Some(key)) = (env("AZURE_OPENAI_ENDPOINT"), env("AZURE_OPENAI_API_KEY"))
    else {
        return Err(usage(
            "set AZURE_OPENAI_ENDPOINT (https://<resource>.openai.azure.com) and \
             AZURE_OPENAI_API_KEY (or keep them: sion agent credential add)",
        ));
    };
    let mut provider =
        OpenAiCompatible::new(client, &azure_base(&endpoint), deployment, Some(&key));
    provider.key_header = Some("api-key".into());
    provider.thinking = thinking;
    Ok(provider)
}

/// The base URL of a provider known by name: `<NAME>_BASE_URL`, or its own.
pub fn named_base(provider: &Named, env: Lookup) -> String {
    env(provider.base_variable).unwrap_or_else(|| provider.base.to_string())
}

/// `<name>:<model>`, for a provider known by name.
pub fn named(
    provider: &Named,
    model: &str,
    thinking: Option<Thinking>,
    env: Lookup,
    client: sion_http::Client,
) -> Result<OpenAiCompatible, CliError> {
    let key = env(provider.key).ok_or_else(|| {
        usage(format!(
            "set {} (or keep it: sion agent credential add {})",
            provider.key, provider.key
        ))
    })?;
    let mut provider = OpenAiCompatible::new(client, &named_base(provider, env), model, Some(&key));
    provider.thinking = thinking;
    Ok(provider)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name: &str| map.get(name).cloned()
    }

    fn client() -> sion_http::Client {
        sion_http::Client::new(sion_http::Options::default()).unwrap()
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sion-clouds-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".aws")).unwrap();
        dir
    }

    #[test]
    fn ini_sections_are_read_as_aws_writes_them() {
        let text = "# keys\n[default]\naws_access_key_id = AKID\n\n[work]\naws_access_key_id=WORK\nregion = eu-west-1\n";
        assert_eq!(
            ini_value(text, "default", "aws_access_key_id").unwrap(),
            "AKID"
        );
        assert_eq!(
            ini_value(text, "work", "aws_access_key_id").unwrap(),
            "WORK"
        );
        assert_eq!(ini_value(text, "work", "region").unwrap(), "eu-west-1");
        assert!(ini_value(text, "default", "region").is_none());
        assert!(ini_value(text, "other", "aws_access_key_id").is_none());
    }

    #[test]
    fn aws_credentials_come_from_the_environment_or_the_profile() {
        let env = lookup(&[
            ("AWS_ACCESS_KEY_ID", "AKID"),
            ("AWS_SECRET_ACCESS_KEY", "secret"),
            ("AWS_SESSION_TOKEN", "token"),
        ]);
        let signer = aws_signer(&env, None, "us-west-2").unwrap();
        assert_eq!(signer.access_key, "AKID");
        assert_eq!(signer.session_token.as_deref(), Some("token"));
        assert_eq!(signer.region, "us-west-2");

        let home = scratch("profile");
        std::fs::write(
            home.join(".aws").join("credentials"),
            "[default]\naws_access_key_id = DEF\naws_secret_access_key = defsecret\n[work]\naws_access_key_id = WORK\naws_secret_access_key = worksecret\n",
        )
        .unwrap();
        std::fs::write(
            home.join(".aws").join("config"),
            "[default]\nregion = eu-central-1\n[profile work]\nregion = ap-south-1\n[profile sso]\nsso_session = corp\n",
        )
        .unwrap();
        let none = lookup(&[]);
        assert_eq!(aws_region(&none, Some(&home)), "eu-central-1");
        assert_eq!(
            aws_signer(&none, Some(&home), "eu-central-1")
                .unwrap()
                .access_key,
            "DEF"
        );
        let work = lookup(&[("AWS_PROFILE", "work")]);
        assert_eq!(aws_region(&work, Some(&home)), "ap-south-1");
        assert_eq!(
            aws_signer(&work, Some(&home), "ap-south-1")
                .unwrap()
                .secret_key,
            "worksecret"
        );
        let sso = lookup(&[("AWS_PROFILE", "sso")]);
        let refused = aws_signer(&sso, Some(&home), "us-east-1").unwrap_err();
        assert!(refused.contains("export-credentials"), "{refused}");
        assert!(aws_signer(&none, None, "us-east-1").is_err());
        assert_eq!(
            aws_region(&lookup(&[("AWS_REGION", "us-west-1")]), Some(&home)),
            "us-west-1"
        );
        assert_eq!(aws_region(&none, None), "us-east-1");
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn bedrock_takes_a_bedrock_key_before_an_access_key() {
        let env = lookup(&[
            ("AWS_BEARER_TOKEN_BEDROCK", "brk"),
            ("AWS_REGION", "eu-west-3"),
            ("AWS_ENDPOINT_URL_BEDROCK_RUNTIME", "http://127.0.0.1:9"),
        ]);
        let provider = bedrock("amazon.nova-pro-v1:0", None, &env, None, client()).unwrap();
        assert_eq!(provider.region, "eu-west-3");
        assert_eq!(provider.base, "http://127.0.0.1:9");
        assert!(format!("{provider:?}").contains("Keys(1)"));
        let refused = bedrock("amazon.nova-pro-v1:0", None, &lookup(&[]), None, client())
            .unwrap_err()
            .to_string();
        assert!(refused.contains("AWS_BEARER_TOKEN_BEDROCK"), "{refused}");
    }

    #[test]
    fn vertex_reads_its_project_location_and_token() {
        let env = lookup(&[
            ("ANTHROPIC_VERTEX_PROJECT_ID", "proj"),
            ("CLOUD_ML_REGION", "us-east5"),
            ("GOOGLE_OAUTH_ACCESS_TOKEN", "ya29.x"),
        ]);
        let provider = vertex("claude-opus-5-5", None, &env, None, client()).unwrap();
        assert_eq!(provider.project, "proj");
        assert_eq!(provider.base, "https://us-east5-aiplatform.googleapis.com");
        assert!(provider.url(false).ends_with(
            "/projects/proj/locations/us-east5/publishers/anthropic/models/claude-opus-5-5:rawPredict"
        ));
        // Express mode: no project, a key, Gemini only.
        let express = lookup(&[("GOOGLE_API_KEY", "k")]);
        let provider = vertex("gemini-2.5-flash", None, &express, None, client()).unwrap();
        assert!(provider.url(true).ends_with(
            "/v1/publishers/google/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
        ));
        assert!(vertex("claude-opus-5-5", None, &express, None, client()).is_err());
        // A project, but nothing to sign in with.
        let home = scratch("vertex");
        let bare = lookup(&[("GOOGLE_CLOUD_PROJECT", "p")]);
        let refused = vertex("gemini-2.5-flash", None, &bare, Some(&home), client())
            .unwrap_err()
            .to_string();
        assert!(refused.contains("application-default login"), "{refused}");
        // A person's sign-in, where gcloud keeps it.
        let file = application_default(&home);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            r#"{"type": "authorized_user", "client_id": "c", "client_secret": "s", "refresh_token": "r"}"#,
        )
        .unwrap();
        let provider = vertex("gemini-2.5-flash", None, &bare, Some(&home), client()).unwrap();
        assert!(format!("{provider:?}").contains("a person's sign-in"));
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn azure_and_the_named_providers_take_their_own_variables() {
        assert_eq!(
            azure_base("https://res.openai.azure.com/"),
            "https://res.openai.azure.com/openai/v1"
        );
        assert_eq!(
            azure_base("https://res.services.ai.azure.com/openai/v1"),
            "https://res.services.ai.azure.com/openai/v1"
        );
        let env = lookup(&[
            ("AZURE_OPENAI_ENDPOINT", "https://res.openai.azure.com"),
            ("AZURE_OPENAI_API_KEY", "az"),
        ]);
        let provider = azure("my-gpt", None, &env, client()).unwrap();
        assert_eq!(provider.key_header.as_deref(), Some("api-key"));
        assert!(azure("my-gpt", None, &lookup(&[]), client()).is_err());

        let openrouter = sion_agent::cloud::named("openrouter").unwrap();
        let env = lookup(&[("OPENROUTER_API_KEY", "or")]);
        assert_eq!(named_base(openrouter, &env), "https://openrouter.ai/api/v1");
        assert!(named(
            openrouter,
            "anthropic/claude-opus-5.5",
            None,
            &env,
            client()
        )
        .is_ok());
        let moved = lookup(&[("OPENROUTER_BASE_URL", "http://127.0.0.1:8080/v1")]);
        assert_eq!(named_base(openrouter, &moved), "http://127.0.0.1:8080/v1");
        let refused = named(openrouter, "x", None, &lookup(&[]), client())
            .unwrap_err()
            .to_string();
        assert!(refused.contains("OPENROUTER_API_KEY"), "{refused}");
    }
}
