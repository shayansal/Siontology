//! Hearing what a file of the workspace says (`transcribe`): an audio
//! file's words, or a video's, its sound taken out of it first in the
//! agent's sandbox, written down by the speech-to-text the operator named
//! (`--hear`, [`crate::voice::Hearing`]).
//!
//! A video is never opened on this machine: ffmpeg runs in the sandbox,
//! where the agent's commands run, and writes the sound beside the video,
//! which is read and removed.

use std::path::PathBuf;

use serde_json::{json, Value};
use sion_agent::model::ToolSpec;
use sion_agent::sandbox::{Mount, Sandbox, WORKSPACE};
use sion_agent::{Effect, Outcome, Tool};

use crate::voice::{Audio, Hearing, MAX_HEARD};

/// The tool's name.
pub const TRANSCRIBE: &str = "transcribe";

/// Audio files heard as they are, by extension, with their media types.
const AUDIO: [(&str, &str); 10] = [
    ("mp3", "audio/mpeg"),
    ("m4a", "audio/mp4"),
    ("aac", "audio/aac"),
    ("wav", "audio/wav"),
    ("ogg", "audio/ogg"),
    ("oga", "audio/ogg"),
    ("opus", "audio/ogg"),
    ("flac", "audio/flac"),
    ("weba", "audio/webm"),
    ("amr", "audio/amr"),
];

/// Videos, whose sound is taken out of them first.
const VIDEO: [&str; 7] = ["mp4", "m4v", "mov", "mkv", "webm", "avi", "mpeg"];

/// Writes down what an audio or video file of the workspace says.
#[derive(Debug)]
pub struct Transcribe {
    /// The workspace.
    pub root: PathBuf,
    /// What hears.
    pub hearing: Hearing,
    /// What hears, as the operator named it (`openai:whisper-1`).
    pub spec: String,
    /// Where a video's sound is taken out of it, if the agent has a shell.
    pub sandbox: Option<Sandbox>,
    /// The paths of the workspace whose content is confidential.
    pub confidential: Vec<String>,
}

/// `text` in single quotes, for `/bin/sh`.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

impl Tool for Transcribe {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: TRANSCRIBE.into(),
            description: format!(
                "Write down what is said in an audio file of your workspace ({}) or a video ({}): \
                 its words, as text. A video's sound is taken out of it first, in your \
                 sandbox. Audio of up to {} MB.",
                AUDIO.map(|(x, _)| x).join(", "),
                VIDEO.join(", "),
                MAX_HEARD >> 20
            ),
            input_schema: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }),
        }
    }

    fn effect(&self) -> Effect {
        // Heard elsewhere, the sound leaves this machine.
        if self.hearing.is_local() {
            Effect::Read
        } else {
            Effect::External
        }
    }

    fn resource(&self, input: &Value) -> Option<String> {
        let path = input["path"].as_str()?.trim_start_matches("./");
        Some(format!("file:{path}"))
    }

    fn call(&self, input: &Value, _key: &str) -> Outcome {
        let relative = input["path"].as_str().unwrap_or_default();
        let path = match sion_agent::builtin::inside(&self.root, relative) {
            Ok(path) => path,
            Err(e) => return Outcome::error(e),
        };
        let held = sion_agent::builtin::confidential_in(&self.confidential, &self.root, relative);
        // What a recording from outside says is from outside.
        let outside = sion_agent::provenance::is_outside(
            &self.root,
            relative,
            std::fs::metadata(&path)
                .ok()
                .filter(|meta| meta.len() <= 64 << 20)
                .and_then(|_| std::fs::read(&path).ok())
                .as_deref(),
        );
        let extension = path
            .extension()
            .map(|x| x.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let audio = if let Some((_, media)) = AUDIO.iter().find(|(x, _)| *x == extension) {
            match std::fs::metadata(&path) {
                Ok(meta) if meta.len() > MAX_HEARD as u64 => {
                    return Outcome::error(format!(
                        "{relative} is {} bytes, more than the {} MB heard at once: cut it \
                         into parts with the shell",
                        meta.len(),
                        MAX_HEARD >> 20
                    ))
                }
                Ok(meta) if !meta.is_file() => {
                    return Outcome::error(format!("{relative} is not a file"))
                }
                Err(e) => return Outcome::error(format!("{relative}: {e}")),
                Ok(_) => {}
            }
            match std::fs::read(&path) {
                Ok(bytes) => Audio::new(media, bytes),
                Err(e) => return Outcome::error(format!("{relative}: {e}")),
            }
        } else if VIDEO.contains(&extension.as_str()) {
            match self.sound_of(relative) {
                Ok(audio) => audio,
                Err(why) => return Outcome::error(why),
            }
        } else {
            return Outcome::error(format!(
                "{relative} is not an audio file or a video this hears ({}, {})",
                AUDIO.map(|(x, _)| x).join(", "),
                VIDEO.join(", ")
            ));
        };
        let outcome = match self.hearing.transcribe(&audio) {
            Ok(text) => Outcome::ok(format!(
                "What {relative} says, as {} heard it:\n{}",
                self.spec,
                text.trim()
            )),
            Err(why) => Outcome::error(format!("{relative} could not be heard: {why}")),
        };
        let outcome = if outside {
            outcome.from_outside()
        } else {
            outcome
        };
        if held {
            outcome.held_confidential()
        } else {
            outcome
        }
    }
}

impl Transcribe {
    /// The sound of the video at `relative`, taken out of it by ffmpeg in
    /// the sandbox, as Ogg Opus at 16 kHz, one channel.
    fn sound_of(&self, relative: &str) -> Result<Audio, String> {
        let sandbox = self.sandbox.as_ref().ok_or(
            "taking the sound out of a video needs the agent's sandbox: give it a shell, or the \
             sound as an audio file",
        )?;
        let mut nonce = [0u8; 6];
        sion_http::random(&mut nonce).map_err(|e| e.to_string())?;
        let name = format!(
            ".sion-heard-{}.ogg",
            nonce.iter().map(|b| format!("{b:02x}")).collect::<String>()
        );
        let script = format!(
            "ffmpeg -nostdin -hide_banner -loglevel error -y -i {} -vn -ac 1 -ar 16000 -c:a \
             libopus -b:a 24k {}",
            quoted(&format!(
                "{WORKSPACE}/{}",
                relative.trim_start_matches("./")
            )),
            quoted(&format!("{WORKSPACE}/{name}"))
        );
        let ran = sandbox.run(&[Mount::workspace(&self.root)], WORKSPACE, &script)?;
        let out = self.root.join(&name);
        let read = std::fs::read(&out);
        let _ = std::fs::remove_file(&out);
        if !ran.succeeded() {
            let why = if ran.stderr.contains("not found") {
                "ffmpeg is not in the sandbox".to_string()
            } else {
                ran.render()
            };
            return Err(format!(
                "the sound could not be taken out of {relative}: {why}"
            ));
        }
        let bytes = read.map_err(|e| format!("the sound taken out of {relative}: {e}"))?;
        if bytes.len() > MAX_HEARD {
            return Err(format!(
                "the sound of {relative} is {} bytes, more than the {} MB heard at once: cut \
                 the video into parts with the shell",
                bytes.len(),
                MAX_HEARD >> 20
            ));
        }
        Ok(Audio::new("audio/ogg", bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sion-hear-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Hears with a program that prints how many bytes it was given.
    fn counting(root: PathBuf, sandbox: Option<Sandbox>) -> Transcribe {
        let spec = "command:wc -c".to_string();
        Transcribe {
            root,
            hearing: Hearing::parse(&spec).unwrap(),
            spec,
            sandbox,
            confidential: vec!["private/".into()],
        }
    }

    #[test]
    fn an_audio_file_is_heard_and_others_refused() {
        let root = workspace("audio");
        std::fs::create_dir_all(root.join("private")).unwrap();
        std::fs::write(root.join("note.ogg"), b"OggS fake sound").unwrap();
        std::fs::write(root.join("private/memo.mp3"), b"ID3 fake").unwrap();
        std::fs::write(root.join("notes.txt"), b"text").unwrap();
        let tool = counting(root.clone(), None);
        assert_eq!(tool.effect(), Effect::Read, "a program here hears");
        let heard = tool.call(&json!({"path": "note.ogg"}), "k");
        assert!(!heard.is_error, "{}", heard.content);
        assert!(
            heard
                .content
                .starts_with("What note.ogg says, as command:wc -c heard it:\n15"),
            "{}",
            heard.content
        );
        assert!(!heard.confidential);
        let held = tool.call(&json!({"path": "private/memo.mp3"}), "k");
        assert!(held.confidential, "{held:?}");
        let text = tool.call(&json!({"path": "notes.txt"}), "k");
        assert!(text.is_error && text.content.contains("not an audio file"));
        let outside = tool.call(&json!({"path": "../x.ogg"}), "k");
        assert!(outside.is_error);
        // A video needs the sandbox, to take its sound out.
        std::fs::write(root.join("talk.mp4"), b"not a video").unwrap();
        let video = tool.call(&json!({"path": "talk.mp4"}), "k");
        assert!(
            video.is_error && video.content.contains("needs the agent's sandbox"),
            "{}",
            video.content
        );
        // Heard by a service, the sound leaves this machine.
        let away = Transcribe {
            hearing: Hearing::parse("openai:whisper-1").unwrap(),
            ..counting(root.clone(), None)
        };
        assert_eq!(away.effect(), Effect::External);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_videos_sound_is_taken_out_in_the_sandbox() {
        let sandbox = Sandbox::new(sion_agent::sandbox::Backend::Bubblewrap);
        if sandbox.probe().is_err() || !std::path::Path::new("/usr/local/bin/ffmpeg").exists() {
            eprintln!("skipped: no bubblewrap or no ffmpeg here");
            return;
        }
        let root = workspace("video");
        // A second of a test picture and a tone, made in the sandbox.
        let made = sandbox
            .run(
                &[Mount::workspace(&root)],
                WORKSPACE,
                "ffmpeg -nostdin -loglevel error -f lavfi -i testsrc=duration=1:size=64x64 -f lavfi -i sine=duration=1 -shortest clip.mp4",
            )
            .unwrap();
        if !made.succeeded() {
            eprintln!(
                "skipped: ffmpeg cannot make a clip in the sandbox: {}",
                made.render()
            );
            return;
        }
        let tool = counting(root.clone(), Some(sandbox));
        let heard = tool.call(&json!({"path": "clip.mp4"}), "k");
        assert!(!heard.is_error, "{}", heard.content);
        assert!(
            heard.content.starts_with("What clip.mp4 says"),
            "{}",
            heard.content
        );
        // What was taken out is gone again.
        let left: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, vec!["clip.mp4".to_string()]);
        let _ = std::fs::remove_dir_all(root);
    }
}
