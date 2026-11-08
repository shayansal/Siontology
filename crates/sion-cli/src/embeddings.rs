//! What turns texts into vectors for search by meaning, as the operator
//! names it (`--embed`), and where their vectors are kept.
//!
//! `--embed` is `openai:<model>`, `voyage:<model>`, `local:<model>` (a
//! server on this machine that speaks OpenAI's embeddings API, such as
//! Ollama's) or `command:<program>` (a program on this machine given the
//! texts as JSON lines on its standard input, which prints a vector of
//! numbers for each, a line each). Vectors are kept under `$SION_HOME`, by
//! model, so a text is embedded once.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;

use sion_agent::meaning::{self, Cache, Embedder, Found, Purpose};

use crate::error::CliError;

/// How long an embeddings provider has to answer.
const PROVIDER_SECS: u64 = 120;

/// An embedder, and where its vectors are kept.
#[derive(Clone)]
pub struct Meaning {
    /// What embeds.
    pub embedder: Arc<dyn Embedder>,
    /// The directory the vectors are kept in.
    pub dir: PathBuf,
}

impl std::fmt::Debug for Meaning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Meaning({})", self.embedder.id())
    }
}

impl Meaning {
    /// Searches `texts` for `query` by words and meaning fused, at most
    /// `limit`: each text's place, its score and how it was found, and a
    /// note if the meaning could not be searched.
    pub fn search(
        &self,
        texts: &[String],
        query: &str,
        limit: usize,
    ) -> (Vec<(usize, f64, Found)>, Option<String>) {
        let mut cache = Cache::open(Some(self.dir.clone()), self.embedder.id());
        meaning::search(
            texts,
            query,
            limit,
            Some((self.embedder.as_ref(), &mut cache)),
            FLOOR,
        )
    }
}

/// How alike a text's meaning must be to the query's to count.
pub const FLOOR: f64 = 0.35;

/// Where embeddings are kept: `$SION_HOME/agents/embeddings`.
pub fn dir() -> PathBuf {
    crate::session::wallet_dir()
        .parent()
        .map(|home| home.join("agents").join("embeddings"))
        .unwrap_or_else(|| PathBuf::from(".sion/agents/embeddings"))
}

/// The embedder `spec` names, and the directory for its vectors.
pub fn parse(spec: &str) -> Result<Meaning, CliError> {
    let (kind, rest) = spec.split_once(':').ok_or_else(|| {
        CliError::Usage(format!(
            "--embed is openai:<model>, voyage:<model>, local:<model> or command:<program>, not {spec:?}"
        ))
    })?;
    let env = crate::credentials::var;
    let http = |base: Option<&str>| {
        sion_http::Client::new(sion_http::Options {
            timeout: std::time::Duration::from_secs(PROVIDER_SECS),
            allow_private: base.is_some_and(sion_agent::oauth::is_loopback),
            ..sion_http::Options::default()
        })
        .map_err(|e| CliError::Usage(e.to_string()))
    };
    let embedder: Arc<dyn Embedder> = match kind {
        "openai" if !rest.is_empty() => {
            let base = env("OPENAI_BASE_URL").unwrap_or_else(|| "https://api.openai.com/v1".into());
            Arc::new(meaning::Api {
                client: http(Some(&base))?,
                base,
                key: Some(env("OPENAI_API_KEY").ok_or_else(|| {
                    CliError::Usage("set OPENAI_API_KEY to embed with OpenAI".into())
                })?),
                model: rest.to_string(),
                voyage: false,
            })
        }
        "voyage" if !rest.is_empty() => {
            let base =
                env("VOYAGE_BASE_URL").unwrap_or_else(|| "https://api.voyageai.com/v1".into());
            Arc::new(meaning::Api {
                client: http(Some(&base))?,
                base,
                key: Some(env("VOYAGE_API_KEY").ok_or_else(|| {
                    CliError::Usage("set VOYAGE_API_KEY to embed with Voyage".into())
                })?),
                model: rest.to_string(),
                voyage: true,
            })
        }
        "local" if !rest.is_empty() => {
            let base = env("SION_LOCAL_EMBED_URL")
                .unwrap_or_else(|| "http://127.0.0.1:11434/v1".into());
            Arc::new(meaning::Api {
                client: http(Some(&base))?,
                base,
                key: env("SION_LOCAL_EMBED_KEY"),
                model: rest.to_string(),
                voyage: false,
            })
        }
        "command" if !rest.is_empty() => Arc::new(Program {
            argv: crate::voice::command_line(rest).map_err(CliError::Usage)?,
            model: format!("command:{rest}"),
        }),
        _ => {
            return Err(CliError::Usage(format!(
                "--embed is openai:<model>, voyage:<model>, local:<model> or command:<program>, not {spec:?}"
            )))
        }
    };
    Ok(Meaning {
        embedder,
        dir: dir(),
    })
}

/// A program on this machine that embeds: it is given the texts as JSON
/// lines on its standard input and prints a vector of numbers for each, a
/// line each (a JSON array, or numbers separated by spaces or commas).
#[derive(Debug)]
struct Program {
    argv: Vec<String>,
    model: String,
}

impl Embedder for Program {
    fn id(&self) -> &str {
        &self.model
    }

    fn is_local(&self) -> bool {
        true
    }

    fn embed(&self, texts: &[String], _: Purpose) -> Result<Vec<Vec<f32>>, String> {
        let mut input = String::new();
        for text in texts {
            let line =
                serde_json::to_string(&text.chars().take(meaning::MAX_CHARS).collect::<String>())
                    .map_err(|e| e.to_string())?;
            input.push_str(&line);
            input.push('\n');
        }
        let mut child = Command::new(&self.argv[0])
            .args(&self.argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("{}: {e}", self.argv[0]))?;
        child
            .stdin
            .take()
            .ok_or("no standard input")?
            .write_all(input.as_bytes())
            .map_err(|e| e.to_string())?;
        let output = child.wait_with_output().map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(format!(
                "the embedding program failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let vectors: Vec<Vec<f32>> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(parse_vector)
            .collect::<Result<_, _>>()?;
        if vectors.len() != texts.len() {
            return Err(format!(
                "the embedding program printed {} vectors for {} texts",
                vectors.len(),
                texts.len()
            ));
        }
        Ok(vectors)
    }
}

/// A vector from a line: a JSON array of numbers, or numbers separated by
/// spaces or commas.
fn parse_vector(line: &str) -> Result<Vec<f32>, String> {
    let line = line.trim();
    if let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(line) {
        return Ok(items
            .iter()
            .map(|v| v.as_f64().unwrap_or(0.0) as f32)
            .collect());
    }
    line.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<f32>().map_err(|e| format!("{s:?}: {e}")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_are_parsed_or_refused() {
        std::env::set_var("OPENAI_API_KEY", "k");
        assert_eq!(
            parse("openai:text-embedding-3-small")
                .unwrap()
                .embedder
                .id(),
            "text-embedding-3-small"
        );
        std::env::remove_var("OPENAI_API_KEY");
        assert!(parse("openai:x").is_err(), "no key");
        assert!(parse("nonsense").is_err());
        assert!(parse("command:").is_err());
        assert_eq!(
            parse("command:/bin/cat").unwrap().embedder.id(),
            "command:/bin/cat"
        );
    }

    #[test]
    fn a_program_embeds_over_standard_input() {
        // A program that prints a two-number vector for each line it reads.
        let program = Program {
            argv: vec![
                "python3".into(),
                "-c".into(),
                "import sys,json\nfor line in sys.stdin:\n line=line.strip()\n if not line: continue\n t=json.loads(line)\n print(len(t), t.count('a'))".into(),
            ],
            model: "command:test".into(),
        };
        if std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("skipped: no python3");
            return;
        }
        let vectors = program
            .embed(&["banana".into(), "xyz".into()], Purpose::Document)
            .unwrap();
        assert_eq!(vectors, vec![vec![6.0, 3.0], vec![3.0, 0.0]]);
        assert_eq!(parse_vector("[1.5, 2.0, 3]").unwrap(), vec![1.5, 2.0, 3.0]);
        assert_eq!(parse_vector("1 2 3").unwrap(), vec![1.0, 2.0, 3.0]);
    }
}
