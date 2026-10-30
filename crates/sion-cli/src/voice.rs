//! Voice in channels: what people say in voice notes, heard (speech to
//! text), and replies spoken back (text to speech), with the providers the
//! operator names.
//!
//! | `--hear` | What hears |
//! |---|---|
//! | `openai:<model>` | OpenAI's transcriptions (`OPENAI_API_KEY`; `OPENAI_BASE_URL` for a compatible server, a local one included) |
//! | `groq:<model>` | Groq's (`GROQ_API_KEY`) |
//! | `command:<program …>` | A program on this machine, given the audio's file as its last argument, which prints the text |
//!
//! | `--speak` | What speaks |
//! |---|---|
//! | `openai:<model>:<voice>` | OpenAI's speech (`OPENAI_API_KEY`, `OPENAI_BASE_URL`), as Ogg Opus |
//! | `elevenlabs:<voice id>[:<model>]` | ElevenLabs (`ELEVENLABS_API_KEY`), as MP3 |
//! | `command:<program …>` | A program given the text on its standard input, which writes Ogg Opus or MP3 to its standard output |
//!
//! What a voice note says is what its sender says: it goes to the agent as
//! their message, marked as heard, so that a mishearing can be told apart.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};

/// The longest voice note heard, in bytes (Whisper's limit is 25 MB).
pub const MAX_HEARD: usize = 25 << 20;

/// The longest reply spoken, in characters (longer ones are sent as text).
pub const MAX_SPOKEN: usize = 4000;

/// How long a provider has to answer.
const PROVIDER_SECS: u64 = 120;

/// Audio, with its media type and a file name for it.
#[derive(Clone, PartialEq, Eq)]
pub struct Audio {
    /// Its media type (`audio/ogg`, `audio/mpeg`).
    pub media_type: String,
    /// A file name, its extension saying what it is (`voice.ogg`).
    pub name: String,
    /// Its bytes.
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for Audio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Audio({}, {} bytes)", self.media_type, self.bytes.len())
    }
}

impl Audio {
    /// Audio of `media_type`, named after it.
    pub fn new(media_type: &str, bytes: Vec<u8>) -> Audio {
        let media_type = media_type
            .split(';')
            .next()
            .unwrap_or(media_type)
            .trim()
            .to_ascii_lowercase();
        let extension = match media_type.as_str() {
            "audio/ogg" | "audio/opus" | "application/ogg" => "ogg",
            "audio/mpeg" | "audio/mp3" => "mp3",
            "audio/mp4" | "audio/m4a" | "audio/x-m4a" | "audio/aac" => "m4a",
            "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
            "audio/webm" => "webm",
            "audio/amr" => "amr",
            "audio/flac" => "flac",
            _ => "bin",
        };
        Audio {
            name: format!("voice.{extension}"),
            media_type,
            bytes,
        }
    }

    /// The media type its bytes show: Ogg (`OggS`), MP3 (`ID3` or a frame
    /// sync), WAV (`RIFF`), or `application/octet-stream`.
    pub fn sniff(bytes: &[u8]) -> &'static str {
        if bytes.starts_with(b"OggS") {
            "audio/ogg"
        } else if bytes.starts_with(b"ID3")
            || (bytes.len() > 1 && bytes[0] == 0xFF && bytes[1] & 0xE0 == 0xE0)
        {
            "audio/mpeg"
        } else if bytes.starts_with(b"RIFF") {
            "audio/wav"
        } else {
            "application/octet-stream"
        }
    }
}

fn client() -> Result<sion_http::Client, String> {
    sion_http::Client::new(sion_http::Options {
        timeout: Duration::from_secs(PROVIDER_SECS),
        // The operator names the provider: a server on this machine too.
        allow_private: true,
        ..sion_http::Options::default()
    })
    .map_err(|e| e.to_string())
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// A command line split on whitespace.
fn command_line(text: &str) -> Result<Vec<String>, String> {
    let argv: Vec<String> = text.split_whitespace().map(str::to_string).collect();
    if argv.is_empty() {
        return Err("command: names no program".into());
    }
    Ok(argv)
}

/// What hears voice notes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hearing {
    /// An OpenAI-compatible transcription API.
    Api {
        /// Its base URL (`https://api.openai.com/v1`).
        base: String,
        /// The environment variable holding its key, if it takes one.
        key: Option<String>,
        /// The model.
        model: String,
    },
    /// A program given the audio's file as its last argument, which prints
    /// the text.
    Command(Vec<String>),
}

impl Hearing {
    /// From `openai:<model>`, `groq:<model>` or `command:<program …>`.
    pub fn parse(spec: &str) -> Result<Hearing, String> {
        let (kind, rest) = spec.split_once(':').ok_or_else(|| {
            format!("{spec:?}: give openai:<model>, groq:<model> or command:<program>")
        })?;
        match kind {
            "openai" if !rest.is_empty() => Ok(Hearing::Api {
                base: env("OPENAI_BASE_URL").unwrap_or_else(|| "https://api.openai.com/v1".into()),
                key: Some("OPENAI_API_KEY".into()),
                model: rest.to_string(),
            }),
            "groq" if !rest.is_empty() => Ok(Hearing::Api {
                base: env("GROQ_BASE_URL")
                    .unwrap_or_else(|| "https://api.groq.com/openai/v1".into()),
                key: Some("GROQ_API_KEY".into()),
                model: rest.to_string(),
            }),
            "command" => Ok(Hearing::Command(command_line(rest)?)),
            _ => Err(format!(
                "{spec:?}: give openai:<model>, groq:<model> or command:<program>"
            )),
        }
    }

    /// What `audio` says.
    pub fn transcribe(&self, audio: &Audio) -> Result<String, String> {
        if audio.bytes.len() > MAX_HEARD {
            return Err(format!(
                "the voice note is longer than {} MB",
                MAX_HEARD >> 20
            ));
        }
        let text = match self {
            Hearing::Api { base, key, model } => {
                let boundary = format!("sion-{}", crate::runtime::new_id());
                let mut body = Vec::new();
                for (name, value) in [("model", model.as_str()), ("response_format", "json")] {
                    write!(
                        body,
                        "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
                    )
                    .map_err(|e| e.to_string())?;
                }
                write!(
                    body,
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
                    audio.name, audio.media_type
                )
                .map_err(|e| e.to_string())?;
                body.extend_from_slice(&audio.bytes);
                write!(body, "\r\n--{boundary}--\r\n").map_err(|e| e.to_string())?;
                let content_type = format!("multipart/form-data; boundary={boundary}");
                let authorization = match key {
                    Some(var) => Some(format!(
                        "Bearer {}",
                        env(var).ok_or_else(|| format!("set {var} to hear voice notes"))?
                    )),
                    None => None,
                };
                let mut headers = vec![("Content-Type", content_type.as_str())];
                if let Some(auth) = &authorization {
                    headers.push(("Authorization", auth.as_str()));
                }
                let url = format!("{}/audio/transcriptions", base.trim_end_matches('/'));
                let response = client()?
                    .send("POST", &url, &headers, Some(&body))
                    .map_err(|e| format!("{url}: {e}"))?;
                let status = response.status;
                let bytes = response.read_all(1 << 20).map_err(|e| e.to_string())?;
                let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                if status != 200 {
                    return Err(format!(
                        "the transcription service answered HTTP {status}: {}",
                        value["error"]["message"]
                            .as_str()
                            .unwrap_or("no reason given")
                    ));
                }
                value["text"]
                    .as_str()
                    .ok_or("the transcription service gave no text")?
                    .to_string()
            }
            Hearing::Command(argv) => {
                let dir =
                    std::env::temp_dir().join(format!("sion-heard-{}", crate::runtime::new_id()));
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                let file = dir.join(&audio.name);
                std::fs::write(&file, &audio.bytes).map_err(|e| e.to_string())?;
                let output = Command::new(&argv[0])
                    .args(&argv[1..])
                    .arg(&file)
                    .stdin(Stdio::null())
                    .output();
                let _ = std::fs::remove_dir_all(&dir);
                let output = output.map_err(|e| format!("{}: {e}", argv[0]))?;
                if !output.status.success() {
                    return Err(format!("{} failed ({})", argv[0], output.status));
                }
                String::from_utf8_lossy(&output.stdout).into_owned()
            }
        };
        let text = text.trim().to_string();
        if text.is_empty() {
            return Err("nothing was heard in the voice note".into());
        }
        Ok(text)
    }
}

/// What speaks replies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Speaking {
    /// An OpenAI-compatible speech API, asked for Ogg Opus.
    Api {
        /// Its base URL.
        base: String,
        /// The environment variable holding its key.
        key: Option<String>,
        /// The model.
        model: String,
        /// The voice.
        voice: String,
    },
    /// ElevenLabs, asked for MP3.
    ElevenLabs {
        /// Its base URL (`https://api.elevenlabs.io`).
        base: String,
        /// The voice's id.
        voice: String,
        /// The model.
        model: String,
    },
    /// A program given the text on its standard input, which writes the
    /// audio (Ogg Opus or MP3) to its standard output.
    Command(Vec<String>),
}

impl Speaking {
    /// From `openai:<model>:<voice>`, `elevenlabs:<voice id>[:<model>]` or
    /// `command:<program …>`.
    pub fn parse(spec: &str) -> Result<Speaking, String> {
        let bad = || {
            format!(
                "{spec:?}: give openai:<model>:<voice>, elevenlabs:<voice id>[:<model>] or command:<program>"
            )
        };
        let (kind, rest) = spec.split_once(':').ok_or_else(bad)?;
        match kind {
            "openai" => {
                let (model, voice) = rest.split_once(':').ok_or_else(bad)?;
                if model.is_empty() || voice.is_empty() {
                    return Err(bad());
                }
                Ok(Speaking::Api {
                    base: env("OPENAI_BASE_URL")
                        .unwrap_or_else(|| "https://api.openai.com/v1".into()),
                    key: Some("OPENAI_API_KEY".into()),
                    model: model.to_string(),
                    voice: voice.to_string(),
                })
            }
            "elevenlabs" => {
                let (voice, model) = rest
                    .split_once(':')
                    .unwrap_or((rest, "eleven_multilingual_v2"));
                if voice.is_empty() {
                    return Err(bad());
                }
                Ok(Speaking::ElevenLabs {
                    base: env("ELEVENLABS_API_BASE")
                        .unwrap_or_else(|| "https://api.elevenlabs.io".into()),
                    voice: voice.to_string(),
                    model: model.to_string(),
                })
            }
            "command" => Ok(Speaking::Command(command_line(rest)?)),
            _ => Err(bad()),
        }
    }

    /// `text`, spoken.
    pub fn speak(&self, text: &str) -> Result<Audio, String> {
        let text: String = text.chars().take(MAX_SPOKEN).collect();
        let (bytes, declared) = match self {
            Speaking::Api {
                base,
                key,
                model,
                voice,
            } => {
                let authorization = match key {
                    Some(var) => Some(format!(
                        "Bearer {}",
                        env(var).ok_or_else(|| format!("set {var} to speak replies"))?
                    )),
                    None => None,
                };
                let mut headers = vec![("Content-Type", "application/json")];
                if let Some(auth) = &authorization {
                    headers.push(("Authorization", auth.as_str()));
                }
                let url = format!("{}/audio/speech", base.trim_end_matches('/'));
                let body = json!({"model": model, "voice": voice, "input": text, "response_format": "opus"});
                fetch_audio(&url, &headers, &body)?
            }
            Speaking::ElevenLabs { base, voice, model } => {
                let key =
                    env("ELEVENLABS_API_KEY").ok_or("set ELEVENLABS_API_KEY to speak replies")?;
                let headers = [
                    ("Content-Type", "application/json"),
                    ("xi-api-key", key.as_str()),
                ];
                let url = format!(
                    "{}/v1/text-to-speech/{voice}?output_format=mp3_44100_128",
                    base.trim_end_matches('/')
                );
                let body = json!({"text": text, "model_id": model});
                fetch_audio(&url, &headers, &body)?
            }
            Speaking::Command(argv) => {
                let mut child = Command::new(&argv[0])
                    .args(&argv[1..])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|e| format!("{}: {e}", argv[0]))?;
                if let Some(mut stdin) = child.stdin.take() {
                    stdin
                        .write_all(text.as_bytes())
                        .map_err(|e| e.to_string())?;
                }
                let output = child.wait_with_output().map_err(|e| e.to_string())?;
                if !output.status.success() {
                    return Err(format!("{} failed ({})", argv[0], output.status));
                }
                (output.stdout, None)
            }
        };
        if bytes.is_empty() {
            return Err("the speech service gave no audio".into());
        }
        let media_type = match Audio::sniff(&bytes) {
            "application/octet-stream" => {
                declared.unwrap_or_else(|| "application/octet-stream".into())
            }
            sniffed => sniffed.to_string(),
        };
        if !matches!(media_type.as_str(), "audio/ogg" | "audio/mpeg") {
            return Err(format!(
                "the speech service gave {media_type}, not Ogg Opus or MP3"
            ));
        }
        Ok(Audio::new(&media_type, bytes))
    }
}

/// POSTs `body` and reads the audio back, with its declared media type.
fn fetch_audio(
    url: &str,
    headers: &[(&str, &str)],
    body: &Value,
) -> Result<(Vec<u8>, Option<String>), String> {
    let response = client()?
        .send("POST", url, headers, Some(body.to_string().as_bytes()))
        .map_err(|e| format!("{url}: {e}"))?;
    let status = response.status;
    let declared = response.media_type();
    let bytes = response
        .read_all(MAX_HEARD as u64)
        .map_err(|e| e.to_string())?;
    if status != 200 {
        let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        let reason = value["error"]["message"]
            .as_str()
            .or_else(|| value["detail"]["message"].as_str())
            .unwrap_or("no reason given");
        return Err(format!(
            "the speech service answered HTTP {status}: {reason}"
        ));
    }
    Ok((bytes, declared))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_are_named_as_the_flags_name_them() {
        assert!(
            matches!(Hearing::parse("openai:gpt-4o-mini-transcribe").unwrap(),
            Hearing::Api { model, .. } if model == "gpt-4o-mini-transcribe")
        );
        assert!(
            matches!(Hearing::parse("groq:whisper-large-v3-turbo").unwrap(),
            Hearing::Api { base, .. } if base.contains("groq"))
        );
        assert_eq!(
            Hearing::parse("command:/usr/bin/hear --fast").unwrap(),
            Hearing::Command(vec!["/usr/bin/hear".into(), "--fast".into()])
        );
        assert!(Hearing::parse("whisper").is_err());
        assert!(Hearing::parse("openai:").is_err());
        assert!(
            matches!(Speaking::parse("openai:gpt-4o-mini-tts:alloy").unwrap(),
            Speaking::Api { voice, .. } if voice == "alloy")
        );
        assert!(
            matches!(Speaking::parse("elevenlabs:21m00Tcm4TlvDq8ikWAM").unwrap(),
            Speaking::ElevenLabs { model, .. } if model == "eleven_multilingual_v2")
        );
        assert!(Speaking::parse("openai:gpt-4o-mini-tts").is_err());
    }

    #[test]
    fn audio_is_told_by_its_bytes() {
        assert_eq!(Audio::sniff(b"OggS\0\x02"), "audio/ogg");
        assert_eq!(Audio::sniff(b"ID3\x04"), "audio/mpeg");
        assert_eq!(Audio::sniff(&[0xFF, 0xFB, 0x90]), "audio/mpeg");
        assert_eq!(Audio::sniff(b"RIFF....WAVE"), "audio/wav");
        assert_eq!(
            Audio::new("audio/ogg; codecs=opus", vec![]).name,
            "voice.ogg"
        );
    }

    #[test]
    fn a_command_hears_and_speaks() {
        // A program that "hears" by printing the file's first line, and one
        // that "speaks" by writing an Ogg header before the text.
        let hearing = Hearing::Command(vec!["head".into(), "-n1".into()]);
        let audio = Audio::new("audio/ogg", b"hello there\nmore".to_vec());
        assert_eq!(hearing.transcribe(&audio).unwrap(), "hello there");
        let speaking = Speaking::Command(vec!["sh".into(), "-c".into(), "printf OggS; cat".into()]);
        let spoken = speaking.speak("hi").unwrap();
        assert_eq!(spoken.media_type, "audio/ogg");
        assert_eq!(spoken.bytes, b"OggShi");
    }
}
