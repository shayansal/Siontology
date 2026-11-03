//! Voice in channels: what people say in voice notes, heard (speech to
//! text), and replies spoken back (text to speech), with the providers the
//! operator names.
//!
//! | `--hear` | What hears |
//! |---|---|
//! | `openai:<model>` | OpenAI's transcriptions (`OPENAI_API_KEY`; `OPENAI_BASE_URL` for a compatible server, a local one included) |
//! | `groq:<model>` | Groq's (`GROQ_API_KEY`) |
//! | `deepgram:<model>` | Deepgram's (`DEEPGRAM_API_KEY`): `nova-3`, say |
//! | `elevenlabs:<model>` | ElevenLabs' Scribe (`ELEVENLABS_API_KEY`): `scribe_v1`, say |
//! | `command:<program …>` | A program on this machine, given the audio's file as its last argument, which prints the text |
//!
//! | `--speak` | What speaks |
//! |---|---|
//! | `openai:<model>:<voice>` | OpenAI's speech (`OPENAI_API_KEY`, `OPENAI_BASE_URL`), as Ogg Opus |
//! | `elevenlabs:<voice id>[:<model>]` | ElevenLabs (`ELEVENLABS_API_KEY`), as MP3 |
//! | `deepgram:<voice model>` | Deepgram's Aura (`DEEPGRAM_API_KEY`), as MP3: `aura-2-thalia-en`, say |
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

/// A `multipart/form-data` body, for the APIs that take files so.
#[derive(Debug)]
pub struct Form {
    boundary: String,
    body: Vec<u8>,
}

impl Default for Form {
    fn default() -> Form {
        Form::new()
    }
}

impl Form {
    /// An empty form, with a boundary of its own.
    pub fn new() -> Form {
        Form {
            boundary: format!("sion-{}", crate::runtime::new_id()),
            body: Vec::new(),
        }
    }

    /// The form with a field `name` holding `value`.
    pub fn text(mut self, name: &str, value: &str) -> Form {
        self.body.extend_from_slice(
            format!(
                "--{}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n",
                self.boundary
            )
            .as_bytes(),
        );
        self
    }

    /// The form with a field `name` holding `audio` as a file.
    pub fn file(self, name: &str, audio: &Audio) -> Form {
        self.bytes(name, &audio.name, &audio.media_type, &audio.bytes)
    }

    /// The form with a field `name` holding `bytes` as the file
    /// `file_name`, of `media_type`. The name keeps to characters that
    /// cannot break the part's header.
    pub fn bytes(mut self, name: &str, file_name: &str, media_type: &str, bytes: &[u8]) -> Form {
        let file_name: String = file_name
            .chars()
            .map(|c| {
                if c == '"' || c == '\\' || c.is_control() {
                    '_'
                } else {
                    c
                }
            })
            .collect();
        let media_type: String = media_type.chars().filter(|c| !c.is_control()).collect();
        self.body.extend_from_slice(
            format!(
                "--{}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{file_name}\"\r\nContent-Type: {media_type}\r\n\r\n",
                self.boundary
            )
            .as_bytes(),
        );
        self.body.extend_from_slice(bytes);
        self.body.extend_from_slice(b"\r\n");
        self
    }

    /// The `Content-Type` to send it with, and the body.
    pub fn finish(mut self) -> (String, Vec<u8>) {
        self.body
            .extend_from_slice(format!("--{}--\r\n", self.boundary).as_bytes());
        (
            format!("multipart/form-data; boundary={}", self.boundary),
            self.body,
        )
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
    crate::credentials::var(name)
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
    /// Deepgram's speech to text (`/v1/listen`), the audio as the body.
    Deepgram {
        /// Its base URL (`https://api.deepgram.com`).
        base: String,
        /// The model (`nova-3`).
        model: String,
    },
    /// ElevenLabs' speech to text (Scribe, `/v1/speech-to-text`).
    ElevenLabs {
        /// Its base URL (`https://api.elevenlabs.io`).
        base: String,
        /// The model (`scribe_v1`).
        model: String,
    },
    /// A program given the audio's file as its last argument, which prints
    /// the text.
    Command(Vec<String>),
}

impl Hearing {
    /// From `openai:<model>`, `groq:<model>`, `deepgram:<model>`,
    /// `elevenlabs:<model>` or `command:<program …>`.
    pub fn parse(spec: &str) -> Result<Hearing, String> {
        let (kind, rest) = spec.split_once(':').ok_or_else(|| {
            format!("{spec:?}: give openai:<model>, groq:<model>, deepgram:<model>, elevenlabs:<model> or command:<program>")
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
            "deepgram" if !rest.is_empty() => Ok(Hearing::Deepgram {
                base: env("DEEPGRAM_BASE_URL").unwrap_or_else(|| "https://api.deepgram.com".into()),
                model: rest.to_string(),
            }),
            "elevenlabs" if !rest.is_empty() => Ok(Hearing::ElevenLabs {
                base: env("ELEVENLABS_API_BASE").unwrap_or_else(|| "https://api.elevenlabs.io".into()),
                model: rest.to_string(),
            }),
            "command" => Ok(Hearing::Command(command_line(rest)?)),
            _ => Err(format!(
                "{spec:?}: give openai:<model>, groq:<model>, deepgram:<model>, elevenlabs:<model> or command:<program>"
            )),
        }
    }

    /// What `audio` says: an error if nothing was heard in it.
    pub fn transcribe(&self, audio: &Audio) -> Result<String, String> {
        let text = self.hear(audio)?;
        if text.is_empty() {
            return Err("nothing was heard in the voice note".into());
        }
        Ok(text)
    }

    /// What `audio` says, which may be nothing.
    pub fn hear(&self, audio: &Audio) -> Result<String, String> {
        if audio.bytes.len() > MAX_HEARD {
            return Err(format!(
                "the voice note is longer than {} MB",
                MAX_HEARD >> 20
            ));
        }
        let text = match self {
            Hearing::Api { base, key, model } => {
                let (content_type, body) = Form::new()
                    .text("model", model)
                    .text("response_format", "json")
                    .file("file", audio)
                    .finish();
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
            Hearing::Deepgram { base, model } => {
                let key =
                    env("DEEPGRAM_API_KEY").ok_or("set DEEPGRAM_API_KEY to hear voice notes")?;
                let auth = format!("Token {key}");
                let url = format!(
                    "{}/v1/listen?model={}&smart_format=true",
                    base.trim_end_matches('/'),
                    crate::webhooks::encode_form(&[("m", model)]).trim_start_matches("m=")
                );
                let response = client()?
                    .send(
                        "POST",
                        &url,
                        &[
                            ("Authorization", &auth),
                            ("Content-Type", &audio.media_type),
                        ],
                        Some(&audio.bytes),
                    )
                    .map_err(|e| format!("{url}: {}", e.to_string().replace(&key, "<key>")))?;
                let status = response.status;
                let bytes = response.read_all(1 << 20).map_err(|e| e.to_string())?;
                let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                if status != 200 {
                    return Err(format!(
                        "Deepgram answered HTTP {status}: {}",
                        value["err_msg"].as_str().unwrap_or("no reason given")
                    ));
                }
                value["results"]["channels"][0]["alternatives"][0]["transcript"]
                    .as_str()
                    .ok_or("Deepgram gave no transcript")?
                    .to_string()
            }
            Hearing::ElevenLabs { base, model } => {
                let key = env("ELEVENLABS_API_KEY")
                    .ok_or("set ELEVENLABS_API_KEY to hear voice notes")?;
                let (content_type, body) = Form::new()
                    .text("model_id", model)
                    .file("file", audio)
                    .finish();
                let url = format!("{}/v1/speech-to-text", base.trim_end_matches('/'));
                let response = client()?
                    .send(
                        "POST",
                        &url,
                        &[("xi-api-key", &key), ("Content-Type", &content_type)],
                        Some(&body),
                    )
                    .map_err(|e| format!("{url}: {}", e.to_string().replace(&key, "<key>")))?;
                let status = response.status;
                let bytes = response.read_all(1 << 20).map_err(|e| e.to_string())?;
                let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                if status != 200 {
                    return Err(format!(
                        "ElevenLabs answered HTTP {status}: {}",
                        value["detail"]["message"]
                            .as_str()
                            .or(value["detail"].as_str())
                            .unwrap_or("no reason given")
                    ));
                }
                value["text"]
                    .as_str()
                    .ok_or("ElevenLabs gave no text")?
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
        Ok(text.trim().to_string())
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
    /// Deepgram's Aura (`/v1/speak`), asked for MP3.
    Deepgram {
        /// Its base URL (`https://api.deepgram.com`).
        base: String,
        /// The voice's model (`aura-2-thalia-en`).
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
                "{spec:?}: give openai:<model>:<voice>, elevenlabs:<voice id>[:<model>], deepgram:<voice model> or command:<program>"
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
            "deepgram" if !rest.is_empty() => Ok(Speaking::Deepgram {
                base: env("DEEPGRAM_BASE_URL").unwrap_or_else(|| "https://api.deepgram.com".into()),
                model: rest.to_string(),
            }),
            "command" => Ok(Speaking::Command(command_line(rest)?)),
            _ => Err(bad()),
        }
    }

    /// `text`, spoken.
    pub fn speak(&self, text: &str) -> Result<Audio, String> {
        let (bytes, declared) = self.fetch(text, false)?;
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

    /// `text`, spoken as raw samples at 16 kHz: what a loudspeaker whose
    /// echo is cancelled plays. The services are asked for them (OpenAI's
    /// at 24 kHz, brought down); a program's audio is decoded with ffmpeg.
    pub fn speak_samples(&self, text: &str) -> Result<Vec<i16>, String> {
        let (bytes, _) = self.fetch(text, true)?;
        if bytes.is_empty() {
            return Err("the speech service gave no audio".into());
        }
        let rate = crate::sound::RATE;
        Ok(match self {
            Speaking::Api { .. } => {
                crate::sound::Resampler::once(24_000, rate, &crate::sound::samples_of(&bytes))
            }
            Speaking::ElevenLabs { .. } | Speaking::Deepgram { .. } => {
                crate::sound::samples_of(&bytes)
            }
            Speaking::Command(_) => decoded(&bytes, rate)?,
        })
    }

    /// The service's answer for `text`: raw samples if `raw` (16-bit at
    /// 24 kHz from OpenAI, 16 kHz from the others), else Ogg Opus or MP3;
    /// and the media type it declared.
    fn fetch(&self, text: &str, raw: bool) -> Result<(Vec<u8>, Option<String>), String> {
        let text: String = text.chars().take(MAX_SPOKEN).collect();
        Ok(match self {
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
                let format = if raw { "pcm" } else { "opus" };
                let body = json!({"model": model, "voice": voice, "input": text, "response_format": format});
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
                    "{}/v1/text-to-speech/{voice}?output_format={}",
                    base.trim_end_matches('/'),
                    if raw { "pcm_16000" } else { "mp3_44100_128" }
                );
                let body = json!({"text": text, "model_id": model});
                fetch_audio(&url, &headers, &body)?
            }
            Speaking::Deepgram { base, model } => {
                let key = env("DEEPGRAM_API_KEY").ok_or("set DEEPGRAM_API_KEY to speak replies")?;
                let auth = format!("Token {key}");
                let headers = [
                    ("Content-Type", "application/json"),
                    ("Authorization", auth.as_str()),
                ];
                let url = format!(
                    "{}/v1/speak?model={}&{}",
                    base.trim_end_matches('/'),
                    crate::webhooks::encode_form(&[("m", model)]).trim_start_matches("m="),
                    if raw {
                        "encoding=linear16&sample_rate=16000&container=none"
                    } else {
                        "encoding=mp3"
                    }
                );
                fetch_audio(&url, &headers, &json!({"text": text}))
                    .map_err(|e| e.replace(&key, "<key>"))?
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
        })
    }
}

/// Ogg Opus or MP3 as raw 16-bit mono samples at `rate`, decoded by ffmpeg.
fn decoded(bytes: &[u8], rate: u32) -> Result<Vec<i16>, String> {
    let mut child = Command::new("ffmpeg")
        .args([
            "-loglevel",
            "error",
            "-i",
            "pipe:0",
            "-f",
            "s16le",
            "-ac",
            "1",
            "-ar",
        ])
        .arg(rate.to_string())
        .arg("pipe:1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("decoding the spoken audio needs ffmpeg: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("no pipe to ffmpeg")?;
    let input = bytes.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    let _ = writer.join();
    if !output.status.success() || output.stdout.is_empty() {
        return Err(format!(
            "ffmpeg could not decode the spoken audio ({})",
            output.status
        ));
    }
    Ok(crate::sound::samples_of(&output.stdout))
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
