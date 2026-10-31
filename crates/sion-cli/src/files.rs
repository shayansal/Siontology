//! Files between people and agents: what a person sends in a chat (the
//! pictures shown to the model, other files put in the agent's workspace),
//! what the agent shares with its reply (`share_file`), and the pictures it
//! makes (`image_generate`, through OpenAI's Images API).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use sion_agent::tools::{Effect, Outcome, Tool};
use sion_agent::{Picture, ToolSpec};

use crate::error::CliError;

/// The most bytes of a file a person sends that the bridge takes.
pub const MAX_RECEIVED: u64 = 25 << 20;

/// The most bytes of a picture shown to a model as it is: past that, it
/// goes to the workspace as any file.
pub const MAX_SHOWN: usize = 5 << 20;

/// The most bytes of a file the agent shares.
pub const MAX_SHARED: u64 = 25 << 20;

/// Where the files people send are put, in the agent's workspace.
pub const INBOX: &str = "inbox";

/// Where the pictures the agent makes are put, in its workspace.
pub const MADE: &str = "images";

/// A file: what it is, a name, its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    /// Its media type (`image/png`, `application/pdf`).
    pub media_type: String,
    /// A name, its extension saying what it is.
    pub name: String,
    /// Its bytes.
    pub bytes: Vec<u8>,
}

impl From<crate::voice::Audio> for File {
    fn from(audio: crate::voice::Audio) -> File {
        File {
            media_type: audio.media_type,
            name: audio.name,
            bytes: audio.bytes,
        }
    }
}

impl File {
    /// Whether models are shown it as a picture: a PNG, JPEG, GIF or WebP
    /// by its bytes, whatever its name or sender says, and small enough.
    pub fn is_picture(&self) -> bool {
        sniff_picture(&self.bytes).is_some() && self.bytes.len() <= MAX_SHOWN
    }

    /// It as a picture for a model, if it is one ([`File::is_picture`]),
    /// of the type its bytes are.
    pub fn picture(&self) -> Option<Picture> {
        let media_type = sniff_picture(&self.bytes)?;
        (self.bytes.len() <= MAX_SHOWN).then(|| Picture {
            media_type: media_type.into(),
            base64: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &self.bytes),
        })
    }

    /// It as JSON, for the HTTP channel: its `name`, `media_type`, `size`
    /// and its bytes in `base64`.
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "media_type": self.media_type,
            "size": self.bytes.len(),
            "base64": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &self.bytes),
        })
    }

    /// A file sent to the HTTP channel as JSON: its bytes in `base64`, and
    /// a `name` and a `media_type` if the sender gives them (else what its
    /// bytes or its name show); `index` names one that came without a name.
    pub fn from_json(value: &Value, index: usize) -> Result<File, String> {
        let data = value["base64"]
            .as_str()
            .ok_or("a file is sent as {\"base64\": …}, with a name and a media_type if you like")?;
        let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data)
            .map_err(|_| "a file's base64 is not base64".to_string())?;
        let name = value["name"].as_str().filter(|n| !n.is_empty());
        let media_type = value["media_type"]
            .as_str()
            .filter(|t| !t.is_empty() && *t != "application/octet-stream")
            .or(sniff_picture(&bytes))
            .or(name.map(media_type_of))
            .unwrap_or("application/octet-stream")
            .to_string();
        let name = match name {
            Some(name) => name.to_string(),
            None => format!("file-{index}.{}", extension_of(&media_type)),
        };
        Ok(File {
            media_type,
            name,
            bytes,
        })
    }

    /// The file of the workspace at `relative`.
    pub fn read(root: &Path, relative: &str) -> Result<File, String> {
        let path = sion_agent::builtin::inside(root, relative)?;
        let size = std::fs::metadata(&path)
            .map_err(|e| format!("{relative}: {e}"))?
            .len();
        if size > MAX_SHARED {
            return Err(format!("{relative} is larger than {} MB", MAX_SHARED >> 20));
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("{relative}: {e}"))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        Ok(File {
            media_type: media_type_of(&name).to_string(),
            name,
            bytes,
        })
    }
}

/// The media type a file's name says it has.
pub fn media_type_of(name: &str) -> &'static str {
    let extension = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" | "log" => "text/plain",
        "md" => "text/markdown",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "zip" => "application/zip",
        "mp3" => "audio/mpeg",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "mp4" => "video/mp4",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
}

/// The extension a media type is written with, for a file that came
/// without a name.
pub fn extension_of(media_type: &str) -> &'static str {
    match media_type.split(';').next().unwrap_or_default().trim() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "application/pdf" => "pdf",
        "text/plain" => "txt",
        "text/csv" => "csv",
        "application/json" => "json",
        "audio/mpeg" => "mp3",
        "audio/ogg" => "ogg",
        "video/mp4" => "mp4",
        _ => "bin",
    }
}

/// The picture type `bytes` are by how they begin: PNG, JPEG, GIF or WebP,
/// the types models take; `None` for anything else.
pub fn sniff_picture(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// A name for a file a person sent, safe to write in a workspace: its own
/// name's plain characters, or one made of `id` and its media type.
pub fn safe_name(name: Option<&str>, id: &str, media_type: &str) -> String {
    let plain = |text: &str| -> String {
        text.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>()
            .trim_matches(['.', '_'])
            .chars()
            .take(80)
            .collect()
    };
    match name.map(plain).filter(|n| !n.is_empty()) {
        Some(name) => name,
        None => {
            let id = plain(id);
            let id = if id.is_empty() {
                "file".to_string()
            } else {
                id
            };
            format!(
                "{}.{}",
                id.chars()
                    .rev()
                    .take(24)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<String>(),
                extension_of(media_type)
            )
        }
    }
}

/// The picture a `data:` URL holds (`data:image/png;base64,…`), as OpenAI's
/// chat API sends them: `None` for another URL, or for what is no picture a
/// model takes ([`File::picture`]).
pub fn picture_of_data_url(url: &str) -> Option<Picture> {
    let (head, data) = url.strip_prefix("data:")?.split_once(',')?;
    if !head.ends_with(";base64") {
        return None;
    }
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data).ok()?;
    File {
        media_type: String::new(),
        name: String::new(),
        bytes,
    }
    .picture()
}

/// What `files` a person sent come to for the agent: the pictures, shown
/// to it, and a line for each file that tells it where the file was put
/// in its `workspace` (in [`INBOX`]), or why it was not kept. `id` (the
/// message's) names files that came without a name.
pub fn take(files: &[File], workspace: Option<&Path>, id: &str) -> (Vec<Picture>, Vec<String>) {
    let mut pictures = Vec::new();
    let mut told = Vec::new();
    for file in files {
        let size = file.bytes.len();
        if size as u64 > MAX_RECEIVED {
            told.push(format!(
                "({} was sent, but it is larger than {} MB, so it was not taken.)",
                file.name,
                MAX_RECEIVED >> 20
            ));
            continue;
        }
        let picture = file.picture();
        let what = if picture.is_some() {
            "A picture"
        } else {
            "A file"
        };
        told.push(match workspace.map(|root| receive(root, file, id)) {
            Some(Ok(path)) => format!(
                "({what} was sent, put in your workspace as {path}: {}, {size} bytes.)",
                file.media_type
            ),
            Some(Err(why)) => format!(
                "({what} was sent, {}, that could not be kept: {why}.)",
                file.name
            ),
            None if picture.is_some() => format!("({what} was sent: {}.)", file.name),
            None => format!(
                "({what} was sent, {} ({}, {size} bytes), but there is no workspace here to keep it in.)",
                file.name, file.media_type
            ),
        });
        pictures.extend(picture);
    }
    (pictures, told)
}

/// Puts `file` in the workspace at `root`, in [`INBOX`], under a name not
/// taken yet: its path in the workspace.
pub fn receive(root: &Path, file: &File, id: &str) -> Result<String, String> {
    let name = safe_name(Some(&file.name), id, &file.media_type);
    put(root, INBOX, &name, &file.bytes)
}

/// Writes `bytes` in the workspace at `root`, in `dir`, as `name` or, if
/// that is taken, `name-2`, `name-3`…: its path in the workspace.
fn put(root: &Path, dir: &str, name: &str, bytes: &[u8]) -> Result<String, String> {
    let at = root.join(dir);
    std::fs::create_dir_all(&at).map_err(|e| format!("{}: {e}", at.display()))?;
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) => (stem.to_string(), format!(".{extension}")),
        None => (name.to_string(), String::new()),
    };
    let mut candidate = name.to_string();
    let mut n = 1;
    while at.join(&candidate).exists() {
        n += 1;
        candidate = format!("{stem}-{n}{extension}");
    }
    std::fs::write(at.join(&candidate), bytes).map_err(|e| format!("{candidate}: {e}"))?;
    Ok(format!("{dir}/{candidate}"))
}

/// `share_file`: a file of the workspace goes to the person the agent
/// answers, with its reply, where the channel takes files.
#[derive(Debug)]
pub struct ShareFile {
    /// The workspace.
    pub root: PathBuf,
    /// What was shared this run, by path in the workspace.
    pub shared: Arc<Mutex<Vec<String>>>,
}

impl Tool for ShareFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "share_file".into(),
            description: format!(
                "Send a file of your workspace (a picture, a document) to the person you are answering, with your reply. At most {} MB; the path is in the workspace.",
                MAX_SHARED >> 20
            ),
            input_schema: json!({"type": "object",
                "properties": {"path": {"type": "string", "description": "the file's path in the workspace"}},
                "required": ["path"]}),
        }
    }

    /// It goes where the reply goes, to whoever the agent answers: no
    /// further than what the agent says.
    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn journaled(&self) -> bool {
        false
    }

    fn call(&self, input: &Value, _key: &str) -> Outcome {
        let Some(relative) = input["path"].as_str() else {
            return Outcome::error("give the file's path in the workspace");
        };
        match File::read(&self.root, relative) {
            Ok(file) => {
                if let Ok(mut shared) = self.shared.lock() {
                    if !shared.iter().any(|s| s == relative) {
                        shared.push(relative.to_string());
                    }
                }
                Outcome::ok(format!(
                    "{relative} ({}, {} bytes) goes with your reply",
                    file.media_type,
                    file.bytes.len()
                ))
            }
            Err(why) => Outcome::error(why),
        }
    }
}

/// The provider and the model an image model's spec names
/// (`openai:<model>`).
pub fn image_model(spec: &str) -> Result<(&str, &str), CliError> {
    match spec.split_once(':') {
        Some(("openai", model)) if !model.is_empty() => Ok(("openai", model)),
        _ => Err(CliError::Usage(format!(
            "an image model is openai:<model>, such as openai:gpt-image-1, not {spec}"
        ))),
    }
}

/// The sizes `image_generate` offers.
const SIZES: [&str; 4] = ["auto", "1024x1024", "1536x1024", "1024x1536"];

/// `image_generate`: a picture made from a description, through OpenAI's
/// Images API (`POST /images/generations`), put in the workspace and shown
/// to the model.
pub struct ImageMaker {
    client: sion_http::Client,
    base: String,
    model: String,
    key: Option<String>,
    root: PathBuf,
}

impl std::fmt::Debug for ImageMaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ImageMaker({} at {})", self.model, self.base)
    }
}

impl ImageMaker {
    /// The tool for `spec` (`openai:<model>`: `OPENAI_BASE_URL`,
    /// `OPENAI_API_KEY`), its pictures put in the workspace at `root`.
    pub fn new(spec: &str, root: &Path) -> Result<ImageMaker, CliError> {
        let (_, model) = image_model(spec)?;
        let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        let base = env("OPENAI_BASE_URL").unwrap_or_else(|| "https://api.openai.com/v1".into());
        let client = sion_http::Client::new(sion_http::Options {
            timeout: std::time::Duration::from_secs(300),
            allow_private: sion_agent::oauth::is_loopback(&base),
            ..sion_http::Options::default()
        })
        .map_err(|e| CliError::Usage(e.to_string()))?;
        Ok(ImageMaker {
            client,
            base,
            model: model.to_string(),
            key: env("OPENAI_API_KEY"),
            root: root.to_path_buf(),
        })
    }
}

impl Tool for ImageMaker {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "image_generate".into(),
            description: format!(
                "Make a picture from a description ({}). It is put in your workspace, in {MADE}/, and shown to you; share_file sends it to the person.",
                self.model
            ),
            input_schema: json!({"type": "object",
                "properties": {
                    "prompt": {"type": "string", "description": "what the picture shows, in detail"},
                    "size": {"type": "string", "enum": SIZES},
                    "name": {"type": "string", "description": "a file name for it (letters, digits, - and _), else one is made"}
                },
                "required": ["prompt"]}),
        }
    }

    /// It writes a picture in the workspace. The description goes only to
    /// the image provider the operator chose (as the conversation goes to
    /// the model's), and spends the operator's credit there as a model call
    /// does.
    fn effect(&self) -> Effect {
        Effect::Write
    }

    fn call(&self, input: &Value, _key: &str) -> Outcome {
        let Some(prompt) = input["prompt"].as_str().filter(|p| !p.trim().is_empty()) else {
            return Outcome::error("describe the picture: give a prompt");
        };
        let size = input["size"]
            .as_str()
            .filter(|s| SIZES.contains(s))
            .unwrap_or("auto");
        let body = json!({"model": self.model, "prompt": prompt, "size": size, "n": 1});
        let bearer = self.key.as_ref().map(|k| format!("Bearer {k}"));
        let headers: Vec<(&str, &str)> = bearer
            .as_deref()
            .map(|b| vec![("Authorization", b)])
            .unwrap_or_default();
        let answer = match self.client.post_json(
            &format!("{}/images/generations", self.base.trim_end_matches('/')),
            &headers,
            body.to_string().as_bytes(),
            64 << 20,
        ) {
            Ok(answer) => answer,
            Err(e) => return Outcome::error(format!("no picture was made: {e}")),
        };
        let value: Value = match serde_json::from_slice(&answer.body) {
            Ok(value) => value,
            Err(e) => return Outcome::error(format!("the answer is not JSON: {e}")),
        };
        let Some(data) = value["data"][0]["b64_json"].as_str() else {
            return Outcome::error("the answer has no picture");
        };
        let Ok(bytes) = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data)
        else {
            return Outcome::error("the picture's bytes are not base64");
        };
        let media_type = match value["output_format"].as_str() {
            Some("jpeg") => "image/jpeg",
            Some("webp") => "image/webp",
            _ => "image/png",
        };
        // The name asked for, as a plain name of the picture's type.
        let extension = extension_of(media_type);
        let name = match input["name"].as_str().filter(|n| !n.trim().is_empty()) {
            Some(asked) => {
                let plain = safe_name(Some(asked), "", media_type);
                let stem = plain
                    .rsplit_once('.')
                    .map_or(plain.as_str(), |(stem, _)| stem)
                    .to_string();
                format!("{stem}.{extension}")
            }
            None => format!("{}.{extension}", crate::runtime::new_id()),
        };
        let made = match put(&self.root, MADE, &name, &bytes) {
            Ok(made) => made,
            Err(why) => return Outcome::error(why),
        };
        let mut outcome = Outcome::ok(format!(
            "made {made} ({} bytes): share_file sends it",
            bytes.len()
        ));
        outcome.pictures.push(Picture {
            media_type: media_type.into(),
            base64: data.to_string(),
        });
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_s_name_says_what_it_is_and_is_made_safe() {
        assert_eq!(media_type_of("Receipt.JPG"), "image/jpeg");
        assert_eq!(media_type_of("notes"), "application/octet-stream");
        assert_eq!(
            safe_name(Some("../../etc/passwd"), "x", "text/plain"),
            "etc_passwd"
        );
        assert_eq!(
            safe_name(Some("report 2026.pdf"), "x", "application/pdf"),
            "report_2026.pdf"
        );
        assert_eq!(
            safe_name(None, "AgACAgIAAxk", "image/jpeg"),
            "AgACAgIAAxk.jpg"
        );
        assert_eq!(safe_name(Some("..."), "", "image/png"), "file.png");
    }

    #[test]
    fn a_received_file_never_takes_another_s_place_and_a_shared_one_stays_inside() {
        let root = std::env::temp_dir().join(format!("sion-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let file = File {
            media_type: "text/plain".into(),
            name: "notes.txt".into(),
            bytes: b"hello".to_vec(),
        };
        assert_eq!(receive(&root, &file, "m1").unwrap(), "inbox/notes.txt");
        assert_eq!(receive(&root, &file, "m2").unwrap(), "inbox/notes-2.txt");
        let shared = Arc::new(Mutex::new(Vec::new()));
        let tool = ShareFile {
            root: root.clone(),
            shared: shared.clone(),
        };
        assert!(!tool.call(&json!({"path": "inbox/notes.txt"}), "k").is_error);
        assert!(tool.call(&json!({"path": "../outside.txt"}), "k").is_error);
        assert!(
            tool.call(&json!({"path": "inbox/missing.txt"}), "k")
                .is_error
        );
        assert_eq!(*shared.lock().unwrap(), ["inbox/notes.txt"]);
        // A picture is one by its bytes: a PNG named as a JPEG is a PNG,
        // and bytes called a PNG that are not one are no picture.
        let picture = File {
            media_type: "image/jpeg".into(),
            name: "p.jpg".into(),
            bytes: b"\x89PNG\r\n\x1a\n\0\0".to_vec(),
        };
        assert!(picture.is_picture() && !file.is_picture());
        let shown = picture.picture().unwrap();
        assert_eq!(shown.media_type, "image/png");
        assert_eq!(shown.base64, "iVBORw0KGgoAAA==");
        let pretend = File {
            media_type: "image/png".into(),
            name: "p.png".into(),
            bytes: vec![0; 10],
        };
        assert!(pretend.picture().is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn files_sent_as_json_or_data_urls_are_what_their_bytes_are() {
        let png = b"\x89PNG\r\n\x1a\n\0\0";
        let file = File::from_json(&json!({"base64": "iVBORw0KGgoAAA=="}), 2).unwrap();
        assert_eq!(
            (file.name.as_str(), file.media_type.as_str()),
            ("file-2.png", "image/png")
        );
        assert_eq!(file.bytes, png);
        assert_eq!(File::from_json(&file.to_json(), 0).unwrap(), file);
        let named = File::from_json(&json!({"base64": "aGk=", "name": "a.csv"}), 0).unwrap();
        assert_eq!(named.media_type, "text/csv");
        assert!(File::from_json(&json!({"base64": "not base64!"}), 0).is_err());
        assert!(File::from_json(&json!({"name": "a.txt"}), 0).is_err());
        let shown = picture_of_data_url("data:image/jpeg;base64,iVBORw0KGgoAAA==").unwrap();
        assert_eq!(shown.media_type, "image/png");
        assert!(picture_of_data_url("https://example.org/a.png").is_none());
        assert!(picture_of_data_url("data:text/plain;base64,aGk=").is_none());
        assert!(picture_of_data_url("data:image/png,raw").is_none());
    }

    #[test]
    fn files_taken_are_shown_or_kept_and_the_agent_told_where() {
        let root = std::env::temp_dir().join(format!("sion-take-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let picture = File {
            media_type: "image/png".into(),
            name: "plan.png".into(),
            bytes: b"\x89PNG\r\n\x1a\n".to_vec(),
        };
        let report = File {
            media_type: "application/pdf".into(),
            name: "report.pdf".into(),
            bytes: b"%PDF-1.7".to_vec(),
        };
        let both = [picture.clone(), report.clone()];
        let (shown, told) = take(&both, Some(&root), "m1");
        assert_eq!(shown.len(), 1);
        assert_eq!(
            told,
            [
                "(A picture was sent, put in your workspace as inbox/plan.png: image/png, 8 bytes.)",
                "(A file was sent, put in your workspace as inbox/report.pdf: application/pdf, 8 bytes.)"
            ]
        );
        assert_eq!(
            std::fs::read(root.join("inbox/report.pdf")).unwrap(),
            b"%PDF-1.7"
        );
        // Without a workspace, a picture is still shown; a file is not kept.
        let (shown, told) = take(&both, None, "m2");
        assert_eq!(shown.len(), 1);
        assert_eq!(told[0], "(A picture was sent: plan.png.)");
        assert!(told[1].contains("no workspace here"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn pictures_are_known_by_their_first_bytes() {
        assert_eq!(sniff_picture(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(sniff_picture(b"GIF89a.."), Some("image/gif"));
        assert_eq!(sniff_picture(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff_picture(b"RIFF\0\0\0\0WAVEfmt "), None);
        assert_eq!(sniff_picture(b"%PDF-1.7"), None);
        assert_eq!(sniff_picture(b""), None);
    }
}
