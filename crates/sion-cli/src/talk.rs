//! A conversation with a profile in the terminal (`sion agent talk`): what
//! you type goes to the agent, and its reply comes as the model writes it,
//! the tools it runs shown as it runs them; a conversation goes on in its
//! session, as on any channel, its commands included (`/new`, `/status`,
//! `/model`, `/approve`…).
//!
//! With `--voice` it is spoken: you are heard through a program that
//! records what you say until you stop (`--record`, sox's `rec` if it is
//! there), transcribed with `--hear`, and the reply is spoken with
//! `--speak`, played through a program (`--play`: mpv, ffplay, afplay or
//! paplay, the first there). Each program is given the audio file's path as
//! its last argument.

use std::io::{BufRead, Write};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use serde_json::Value;

use crate::channels::converse_events;
use crate::error::CliError;
use crate::profiles::Profile;
use crate::runtime::SharedLedger;
use crate::voice::{Audio, Hearing, Speaking};

/// What records what is said: sox's `rec`, stopping after a second and a
/// half of silence. The file's path is put last.
pub const RECORD: &[&str] = &[
    "rec", "-q", "-c", "1", "-r", "16000", "-b", "16", "--", "silence", "1", "0.1", "1%", "1",
    "1.5", "1%",
];

/// The players tried, in order, when none is named.
pub const PLAYERS: &[&[&str]] = &[
    &["mpv", "--no-video", "--really-quiet"],
    &["ffplay", "-nodisp", "-autoexit", "-loglevel", "quiet"],
    &["afplay"],
    &["paplay"],
];

/// How a terminal conversation is had.
#[derive(Clone, Debug, Default)]
pub struct Talk {
    /// The conversation's name: each is a session of its own.
    pub conversation: String,
    /// What hears you, in voice mode.
    pub hearing: Option<Hearing>,
    /// What speaks the replies, in voice mode.
    pub speaking: Option<Speaking>,
    /// What records you: a program and its arguments, the file put last.
    pub record: Vec<String>,
    /// What plays the replies, the file put last.
    pub play: Vec<String>,
}

/// Whether `program` is on the `PATH`.
fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

/// A command line from `text` split on whitespace, or the first of
/// `defaults` whose program is on the `PATH`.
pub fn program(text: Option<&str>, defaults: &[&[&str]]) -> Option<Vec<String>> {
    match text {
        Some(text) => {
            let argv: Vec<String> = text.split_whitespace().map(str::to_string).collect();
            (!argv.is_empty()).then_some(argv)
        }
        None => defaults
            .iter()
            .find(|argv| on_path(argv[0]))
            .map(|argv| argv.iter().map(|a| a.to_string()).collect()),
    }
}

/// Runs `argv` with `file` put after its program's first arguments (sox
/// takes its effects after the file: they follow a `--`).
fn run_with_file(argv: &[String], file: &std::path::Path) -> Result<(), String> {
    let (before, after) = match argv.iter().position(|a| a == "--") {
        Some(at) => (&argv[..at], &argv[at + 1..]),
        None => (argv, &[][..]),
    };
    let status = Command::new(&before[0])
        .args(&before[1..])
        .arg(file)
        .args(after)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .map_err(|e| format!("{}: {e}", before[0]))?;
    if !status.success() {
        return Err(format!("{} failed ({status})", before[0]));
    }
    Ok(())
}

/// A file of its own, under the system's temporary directory.
fn scratch(extension: &str) -> Result<std::path::PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("sion-talk-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join(format!("{}.{extension}", crate::runtime::new_id())))
}

impl Talk {
    /// What you say next: a line typed, or, in voice mode, what is heard
    /// after you press Enter. `None` at the end of the input.
    fn next(&self, input: &mut dyn BufRead, out: &mut dyn Write) -> Result<Option<String>, String> {
        let voiced = self.hearing.is_some();
        let _ = write!(
            out,
            "{}",
            if voiced { "(Enter to speak) " } else { "you> " }
        );
        let _ = out.flush();
        let mut line = String::new();
        if input.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return Ok(None);
        }
        let Some(hearing) = &self.hearing else {
            return Ok(Some(line.trim().to_string()));
        };
        // Typed after all: taken as it is.
        if !line.trim().is_empty() {
            return Ok(Some(line.trim().to_string()));
        }
        let _ = writeln!(out, "(listening…)");
        let file = scratch("wav")?;
        run_with_file(&self.record, &file)?;
        let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(&file);
        let heard = hearing.transcribe(&Audio::new(Audio::sniff(&bytes), bytes))?;
        let _ = writeln!(out, "you (heard): {heard}");
        Ok(Some(heard))
    }

    /// Speaks `reply`, where replies are spoken.
    fn speak(&self, reply: &str) -> Result<(), String> {
        let Some(speaking) = &self.speaking else {
            return Ok(());
        };
        if reply.trim().is_empty() {
            return Ok(());
        }
        let audio = speaking.speak(reply)?;
        let extension = audio.name.rsplit('.').next().unwrap_or("audio").to_string();
        let file = scratch(&extension)?;
        std::fs::write(&file, &audio.bytes).map_err(|e| e.to_string())?;
        let played = run_with_file(&self.play, &file);
        let _ = std::fs::remove_file(&file);
        played
    }
}

/// Talks with `profile` in conversation `talk.conversation`, reading from
/// `input` and writing to `out` until the input ends.
pub fn run(
    ledger: SharedLedger,
    profile: &Profile,
    talk: &Talk,
    input: &mut dyn BufRead,
    out: &mut (dyn Write + Send),
) -> Result<(), CliError> {
    let key = format!("terminal:{}:{}", profile.name, talk.conversation);
    let out = Mutex::new(out);
    loop {
        let said = {
            let mut out = out.lock().unwrap_or_else(|e| e.into_inner());
            match talk.next(input, &mut **out) {
                Ok(Some(said)) => said,
                Ok(None) => return Ok(()),
                Err(why) => {
                    let _ = writeln!(out, "(not heard: {why})");
                    continue;
                }
            }
        };
        if said.is_empty() {
            continue;
        }
        // The reply as it is written; a tool on a line of its own.
        let mid_line = Mutex::new(false);
        let tell = |event: &str, data: Value| {
            let mut out = out.lock().unwrap_or_else(|e| e.into_inner());
            let mut mid = mid_line.lock().unwrap_or_else(|e| e.into_inner());
            match event {
                "delta" => {
                    let text = data["text"].as_str().unwrap_or_default();
                    if !*mid {
                        let _ = write!(out, "agent> ");
                    }
                    let _ = write!(out, "{text}");
                    *mid = !text.is_empty() || *mid;
                }
                "tool" if data["state"] == "running" => {
                    if *mid {
                        let _ = writeln!(out);
                        *mid = false;
                    }
                    let _ = writeln!(out, "  · {}", data["tool"].as_str().unwrap_or_default());
                }
                _ => {}
            }
            let _ = out.flush();
        };
        let answer = converse_events(ledger.clone(), profile, &key, &said, &[], &tell);
        let mut out_now = out.lock().unwrap_or_else(|e| e.into_inner());
        let mid = *mid_line.lock().unwrap_or_else(|e| e.into_inner());
        match answer {
            Ok(answer) => {
                // A command's answer comes whole.
                if !mid && !answer.reply.is_empty() {
                    let _ = write!(out_now, "agent> {}", answer.reply);
                }
                let _ = writeln!(out_now);
                drop(out_now);
                if let Err(why) = talk.speak(&answer.reply) {
                    let mut out = out.lock().unwrap_or_else(|e| e.into_inner());
                    let _ = writeln!(out, "(not spoken: {why})");
                }
            }
            Err(e) => {
                if mid {
                    let _ = writeln!(out_now);
                }
                let _ = writeln!(out_now, "(that did not work: {e})");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programs_are_named_or_found() {
        assert_eq!(
            program(Some("  sox -d  "), &[]),
            Some(vec!["sox".to_string(), "-d".to_string()])
        );
        assert_eq!(program(Some(" "), &[]), None);
        assert_eq!(program(None, &[&["no-such-program-here"]]), None);
        assert_eq!(
            program(None, &[&["no-such-program-here"], &["sh", "-c"]]).unwrap()[0],
            "sh"
        );
    }

    #[test]
    fn a_file_goes_before_the_effects_that_follow_a_double_dash() {
        let file = scratch("txt").unwrap();
        let argv: Vec<String> = [
            "sh",
            "-c",
            "printf '%s' \"$0 $1\" > \"$0.out\"",
            "--",
            "after",
        ]
        .iter()
        .map(|a| a.to_string())
        .collect();
        run_with_file(&argv, &file).unwrap();
        let written = std::fs::read_to_string(file.with_extension("txt.out")).unwrap();
        assert_eq!(written, format!("{} after", file.display()));
    }
}
