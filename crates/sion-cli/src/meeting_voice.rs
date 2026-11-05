//! A meeting guest that speaks when it is spoken to.
//!
//! The guest of [`crate::meeting_bot`] listens and says nothing: its
//! microphone plays a second of silence. This gives it a voice. A
//! microphone of its own is made out of PulseAudio parts ([`Microphone`]):
//! a sink nothing listens to, whose monitor is remapped into a *source*,
//! which the browser is told to use as its microphone. Whatever is played
//! into that sink is what the meeting hears the guest say.
//!
//! It speaks only when spoken to. A line of the transcript that names it
//! ([`crate::listening::spoken_to`], which finds the name anywhere in the
//! line, not only at the start) is a question for it; the rest of the
//! meeting goes by unanswered. What it is asked goes to the profile's agent
//! in a session with none of the owner's reach, tainted as a meeting's
//! words always are, and what the agent says is spoken into the meeting and
//! written into the transcript as the guest's own line.
//!
//! The parts here that need no sound card are tested; making the
//! microphone, and the browser taking it, need PulseAudio or PipeWire and a
//! meeting, and were not run where these pages were made.

use std::process::{Command, Stdio};

/// What the guest's sink and source are called, the process that made them
/// after it.
const MIC_PREFIX: &str = "sion-guest-mic-";

/// A microphone the guest speaks through: a null sink, and its monitor
/// remapped into a source the browser uses. Both go when it is dropped.
#[derive(Debug)]
pub struct Microphone {
    sink: String,
    source: String,
    /// The modules loaded, newest first, to unload in that order.
    modules: Vec<String>,
}

/// Runs `pactl` with `args`: what it printed, or why not.
fn pactl(args: &[&str]) -> Result<String, String> {
    let output = Command::new("pactl")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("a guest that speaks needs PulseAudio or PipeWire (pactl): {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "pactl {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

impl Microphone {
    /// Makes one: the sink, then the source remapped from its monitor.
    pub fn make() -> Result<Microphone, String> {
        let name = format!("{MIC_PREFIX}{}", std::process::id());
        let source = format!("{name}-src");
        let sink_module = pactl(&[
            "load-module",
            "module-null-sink",
            &format!("sink_name={name}"),
            "sink_properties=device.description=sion-guest-mic",
        ])?;
        let mut microphone = Microphone {
            sink: name.clone(),
            source: source.clone(),
            modules: vec![sink_module],
        };
        // The browser's microphone: the sink's monitor, as a source.
        let source_module = pactl(&[
            "load-module",
            "module-remap-source",
            &format!("master={name}.monitor"),
            &format!("source_name={source}"),
            "source_properties=device.description=sion-guest-mic",
        ])?;
        // Dropped before this, the sink would go on its own.
        microphone.modules.insert(0, source_module);
        Ok(microphone)
    }

    /// The source the browser uses as its microphone (`PULSE_SOURCE`).
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The sink the guest's voice is played into.
    pub fn sink(&self) -> &str {
        &self.sink
    }

    /// Plays `samples` (16-bit mono at `rate`) into the meeting, as the
    /// guest speaking.
    pub fn say(&self, samples: &[i16], rate: u32) -> Result<(), String> {
        if samples.is_empty() {
            return Ok(());
        }
        let mut child = Command::new("pacat")
            .args([
                "--playback",
                "--format=s16le",
                "--channels=1",
                &format!("--rate={rate}"),
                &format!("--device={}", self.sink),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("pacat: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write as _;
            let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
            // Written beside the wait: a full pipe does not hold this up.
            std::thread::spawn(move || {
                let _ = stdin.write_all(&bytes);
            });
        }
        let status = child.wait().map_err(|e| e.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("pacat could not play the guest's voice ({status})"))
        }
    }
}

impl Drop for Microphone {
    fn drop(&mut self) {
        for module in &self.modules {
            let _ = pactl(&["unload-module", module]);
        }
    }
}

/// What a line of a meeting's transcript asks of the guest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Heard {
    /// Nothing for the guest: the meeting talking among itself.
    NotForIt,
    /// It was named but asked nothing ("thanks, Notes").
    NamedOnly,
    /// It was asked this.
    Asked(String),
}

/// The least a question must be, in characters, to be worth answering:
/// below it the guest was named and little else.
const SHORTEST: usize = 3;

/// What `line` asks of a guest known by `phrases`
/// ([`crate::listening::phrases_of`]).
pub fn heard(phrases: &[Vec<String>], line: &str) -> Heard {
    match crate::listening::spoken_to(phrases, line) {
        None => Heard::NotForIt,
        Some(rest) => {
            let asked = rest.trim().trim_start_matches([',', ':', '-']).trim();
            if asked.chars().count() < SHORTEST {
                Heard::NamedOnly
            } else {
                Heard::Asked(asked.to_string())
            }
        }
    }
}

/// The arguments a browser is given to use `microphone` as its microphone
/// rather than a file of silence: the fake device is left off, so the real
/// PulseAudio source is taken, and permission is granted as before.
pub fn browser_arguments(microphone: &Microphone) -> Vec<String> {
    vec![
        "--use-fake-ui-for-media-stream".to_string(),
        format!("--alsa-input-device=pulse:{}", microphone.source()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listening::phrases_of;

    fn names() -> Vec<Vec<String>> {
        phrases_of("notes|hey notes").unwrap()
    }

    #[test]
    fn the_guest_answers_only_what_names_it() {
        let names = names();
        // Named at the start, in the middle, and at the end.
        assert_eq!(
            heard(&names, "Notes, could you summarize the decision?"),
            Heard::Asked("could you summarize the decision?".into())
        );
        assert_eq!(
            heard(&names, "So, hey notes, what did Ana commit to?"),
            Heard::Asked("what did Ana commit to?".into())
        );
        assert_eq!(heard(&names, "Thanks, notes"), Heard::NamedOnly);
        // The meeting talking among itself.
        assert_eq!(
            heard(&names, "Shall we ship it on Friday?"),
            Heard::NotForIt
        );
        assert_eq!(
            heard(&names, "Ana will write up the notes later"),
            // "notes" names it, and what follows is a question for it:
            // a name that is also an ordinary word is answered, which is
            // why a guest is better given a name of its own.
            Heard::Asked("later".into())
        );
    }

    #[test]
    fn a_name_a_transcriber_spells_a_letter_off_is_still_understood() {
        let names = phrases_of("sion").unwrap();
        assert_eq!(
            heard(&names, "Zion, how much is left?"),
            Heard::Asked("how much is left?".into())
        );
    }

    #[test]
    fn what_follows_the_name_loses_the_punctuation_that_joined_it() {
        let names = names();
        for line in [
            "notes: summarize it",
            "notes - summarize it",
            "notes, summarize it",
        ] {
            assert_eq!(
                heard(&names, line),
                Heard::Asked("summarize it".into()),
                "{line}"
            );
        }
    }

    #[test]
    fn without_pulseaudio_a_speaking_guest_says_what_is_missing() {
        // No sound card here: making the microphone says what it needs.
        match Microphone::make() {
            Err(why) => assert!(why.contains("PulseAudio") || why.contains("pactl"), "{why}"),
            // Where PulseAudio is there, it is made and removed again.
            Ok(microphone) => {
                assert!(microphone.source().starts_with(MIC_PREFIX));
                assert!(browser_arguments(&microphone)
                    .iter()
                    .any(|a| a.contains(microphone.source())));
            }
        }
    }
}
