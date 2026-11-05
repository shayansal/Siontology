//! A spoken conversation that listens all the time (`sion agent talk
//! --voice --live`): the microphone is read without pause, what is said is
//! found in it by its loudness and heard, and the reply is spoken a
//! sentence at a time as the model writes it. Speaking over the agent, while
//! it works or speaks, stops it: it falls silent at once, its run stops
//! before its next step, and what was said goes to it next.
//!
//! With a wake word (`--wake "hey sion"`), only what begins with it is
//! taken, and what is said before it is heard by a transcriber on this
//! machine unless the person lets one elsewhere hear it (`--wake-cloud`):
//! everything said near the microphone is heard to find the word. It stays
//! awake for a follow-up for a while after it speaks, and is then
//! interrupted by its name ("hey sion, stop").
//!
//! Speech is told from other sound by RNNoise's network (`--detector
//! model`, [`crate::sound::SpeechModel`]) together with its loudness, or
//! by its loudness alone (`--detector loudness`: an energy detector over
//! 20 ms frames, against a noise floor it follows). While the agent speaks
//! through loudspeakers, its own voice is taken out of what the microphone
//! hears ([`crate::sound::Canceller`]): the reply is asked of the speech
//! service as raw samples and played through a program that reads them on
//! its standard input (`--play-raw`: aplay, paplay, sox's play or ffplay),
//! at the pace it is spoken, so that what the loudspeaker was given is
//! known. `--no-echo-cancel` plays files as before; then use headphones,
//! or `--no-interrupt`.

use std::collections::VecDeque;
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::error::CliError;
use crate::profiles::Profile;
use crate::runtime::SharedLedger;
use crate::talk::Talk;
use crate::voice::{Audio, Hearing, Speaking};

/// Samples a second, asked of the microphone.
pub const RATE: u32 = 16_000;

/// The microphone's mode: listened to as usual.
pub(crate) const LISTENING: u8 = 0;
/// The agent works or speaks: what is said must be louder, and longer, to
/// interrupt it.
pub(crate) const OVER: u8 = 1;
/// The agent works or speaks, and is not to be interrupted: not listened
/// to.
pub(crate) const DEAF: u8 = 2;

/// The programs tried for the microphone, in order, when none is named:
/// each writes raw 16-bit mono samples at 16 kHz to its standard output.
pub fn microphones() -> Vec<&'static [&'static str]> {
    vec![
        &[
            "rec",
            "-q",
            "-t",
            "raw",
            "-r",
            "16000",
            "-e",
            "signed-integer",
            "-b",
            "16",
            "-c",
            "1",
            "-",
        ],
        &[
            "arecord", "-q", "-t", "raw", "-f", "S16_LE", "-r", "16000", "-c", "1",
        ],
        &["parec", "--format=s16le", "--rate=16000", "--channels=1"],
        if cfg!(target_os = "macos") {
            &[
                "ffmpeg",
                "-loglevel",
                "quiet",
                "-f",
                "avfoundation",
                "-i",
                ":0",
                "-ac",
                "1",
                "-ar",
                "16000",
                "-f",
                "s16le",
                "-",
            ]
        } else {
            &[
                "ffmpeg",
                "-loglevel",
                "quiet",
                "-f",
                "pulse",
                "-i",
                "default",
                "-ac",
                "1",
                "-ar",
                "16000",
                "-f",
                "s16le",
                "-",
            ]
        },
    ]
}

/// The programs tried, in order, to play raw 16-bit mono samples at 16 kHz
/// read on their standard input, when echo is cancelled.
pub const RAW_PLAYERS: &[&[&str]] = &[
    &[
        "aplay", "-q", "-t", "raw", "-f", "S16_LE", "-r", "16000", "-c", "1",
    ],
    &[
        "paplay",
        "--raw",
        "--format=s16le",
        "--rate=16000",
        "--channels=1",
    ],
    &[
        "play",
        "-q",
        "-t",
        "raw",
        "-r",
        "16000",
        "-e",
        "signed-integer",
        "-b",
        "16",
        "-c",
        "1",
        "-",
    ],
    &[
        "ffplay",
        "-nodisp",
        "-autoexit",
        "-loglevel",
        "quiet",
        "-f",
        "s16le",
        "-ar",
        "16000",
        "-i",
        "-",
    ],
];

/// How speech is told from silence. Levels are root mean squares of 16-bit
/// samples (of 32768).
#[derive(Clone, Debug, PartialEq)]
pub struct Detection {
    /// Samples a second.
    pub rate: u32,
    /// A frame, in samples (20 ms).
    pub frame: usize,
    /// Loud frames in a row that begin speech.
    pub start: usize,
    /// Quiet frames in a row that end it.
    pub end: usize,
    /// Frames kept from before speech began, and after it ended.
    pub around: usize,
    /// The most frames an utterance runs to.
    pub most: usize,
    /// Fewer loud frames than this are a noise, not speech.
    pub least: usize,
    /// The quietest level taken for speech.
    pub level: f64,
    /// How many times louder than the noise floor speech is.
    pub over_floor: f64,
    /// While the agent works or speaks: how many times louder speech must
    /// be to interrupt it,
    pub over_factor: f64,
    /// and for how many frames in a row.
    pub over_frames: usize,
    /// Whether RNNoise's network says which frames are speech, their
    /// loudness only keeping out what is near silence (and, over the
    /// agent, what is not loud enough to interrupt it).
    pub model: bool,
}

impl Default for Detection {
    fn default() -> Detection {
        Detection {
            rate: RATE,
            frame: (RATE / 50) as usize,
            start: 3,
            end: 40,
            around: 10,
            most: 1500,
            least: 8,
            level: 400.0,
            over_floor: 3.0,
            over_factor: 2.0,
            over_frames: 12,
            model: true,
        }
    }
}

/// How likely a frame must be speech, by the model, to begin an utterance
/// (noise that starts suddenly scores up to about 0.8 while the network
/// settles; speech, above 0.9), and to go on with one.
const MODEL_BEGINS: f32 = 0.85;
const MODEL_GOES_ON: f32 = 0.5;

/// The root mean square of `samples`.
pub fn level(samples: &[i16]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    (sum / samples.len() as f64).sqrt()
}

/// What a frame of sound made of what is being said.
#[derive(Debug, PartialEq)]
pub enum Sound {
    /// Nothing new.
    Nothing,
    /// Speech began.
    Began,
    /// An utterance ended: its samples, with a little of what came before
    /// and after.
    Utterance(Vec<i16>),
}

/// Finds utterances in the microphone's sound, a frame at a time.
#[derive(Debug)]
pub struct Detector {
    detection: Detection,
    /// The level of what is heard between utterances.
    floor: Option<f64>,
    /// The frames before speech began.
    before: VecDeque<Vec<i16>>,
    speech: Vec<i16>,
    speaking: bool,
    /// Loud frames in a row, before speech began.
    loud_run: usize,
    /// Quiet frames in a row, in speech.
    quiet_run: usize,
    /// Loud frames in the utterance.
    loud: usize,
    /// Its frames.
    frames: usize,
    /// The level speech goes on above.
    keep_above: f64,
    /// The model of speech, if it is asked.
    model: Option<crate::sound::SpeechModel>,
}

impl Detector {
    /// A detector that tells speech by `detection`.
    pub fn new(detection: Detection) -> Detector {
        Detector {
            model: detection.model.then(crate::sound::SpeechModel::new),
            detection,
            floor: None,
            before: VecDeque::new(),
            speech: Vec::new(),
            speaking: false,
            loud_run: 0,
            quiet_run: 0,
            loud: 0,
            frames: 0,
            keep_above: 0.0,
        }
    }

    /// The level speech begins at: a few times the noise floor, within
    /// bounds; `over` the agent, `over_factor` times that.
    fn threshold(&self, over: bool) -> f64 {
        let d = &self.detection;
        let base = (self.floor.unwrap_or(0.0) * d.over_floor).clamp(d.level, d.level * 8.0);
        if over {
            base * d.over_factor
        } else {
            base
        }
    }

    /// Follows the noise floor: quickly down, slowly up.
    fn adapt(&mut self, level: f64) {
        self.floor = Some(match self.floor {
            None => level,
            Some(floor) if level < floor => floor + (level - floor) * 0.2,
            Some(floor) => floor + (level - floor) * 0.02,
        });
    }

    /// Takes the next frame, heard `over` the agent if it works or speaks.
    pub fn frame(&mut self, frame: &[i16], over: bool) -> Sound {
        let level = level(frame);
        let speech = self.model.as_mut().map(|model| model.probability(frame));
        if !self.speaking {
            let threshold = self.threshold(over);
            let needed = if over {
                self.detection.over_frames
            } else {
                self.detection.start
            };
            let loud = match speech {
                // Over the agent, it must still be loud enough to interrupt
                // it; else only not near silence.
                Some(p) if over => p >= MODEL_BEGINS && level >= threshold,
                Some(p) => p >= MODEL_BEGINS && level >= self.detection.level / 2.0,
                None => level >= threshold,
            };
            if loud {
                self.loud_run += 1;
            } else {
                self.loud_run = 0;
                self.adapt(level);
            }
            self.before.push_back(frame.to_vec());
            while self.before.len() > self.detection.around + needed {
                self.before.pop_front();
            }
            if self.loud_run < needed {
                return Sound::Nothing;
            }
            self.speaking = true;
            self.frames = self.before.len();
            self.speech = self.before.drain(..).flatten().collect();
            self.loud = self.loud_run;
            self.quiet_run = 0;
            self.keep_above = threshold / 2.0;
            return Sound::Began;
        }
        self.speech.extend_from_slice(frame);
        self.frames += 1;
        let going = match speech {
            // A hum the model hears as a voice does not keep an utterance
            // going once it is near silence.
            Some(p) => p >= MODEL_GOES_ON && level >= self.keep_above / 2.0,
            None => level >= self.keep_above,
        };
        if going {
            self.quiet_run = 0;
            self.loud += 1;
        } else {
            self.quiet_run += 1;
        }
        if self.quiet_run < self.detection.end && self.frames < self.detection.most {
            return Sound::Nothing;
        }
        self.end()
    }

    /// Ends the utterance under way: only what kept a little of the
    /// silence after it.
    fn end(&mut self) -> Sound {
        let d = &self.detection;
        let extra = self.quiet_run.saturating_sub(d.around) * d.frame;
        let keep = self.speech.len().saturating_sub(extra);
        self.speech.truncate(keep);
        self.speaking = false;
        self.loud_run = 0;
        self.quiet_run = 0;
        let speech = std::mem::take(&mut self.speech);
        if self.loud < d.least {
            Sound::Nothing
        } else {
            Sound::Utterance(speech)
        }
    }

    /// What was being said when the sound stopped, if it was speech.
    pub fn flush(&mut self) -> Option<Vec<i16>> {
        if !self.speaking {
            return None;
        }
        match self.end() {
            Sound::Utterance(speech) => Some(speech),
            _ => None,
        }
    }

    /// Forgets what was being said (while the microphone is not listened
    /// to).
    pub fn reset(&mut self) {
        self.speaking = false;
        self.loud_run = 0;
        self.quiet_run = 0;
        self.speech.clear();
        self.before.clear();
    }
}

/// `samples` as a WAV file: 16-bit mono at `rate`.
pub fn wav(samples: &[i16], rate: u32) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    // PCM, one channel.
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

/// A wake word: what is said must begin with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wake {
    /// The phrases, each its words in lower case: any one wakes it.
    pub phrases: Vec<Vec<String>>,
    /// What hears what is said before it wakes.
    pub hearing: Hearing,
    /// Seconds it stays awake for a follow-up after it speaks.
    pub awake_secs: u64,
}

/// The words of `text`, in lower case, with where each ends.
fn words(text: &str) -> Vec<(String, usize)> {
    let mut found = Vec::new();
    let mut word = String::new();
    for (at, c) in text.char_indices() {
        if c.is_alphanumeric() {
            word.extend(c.to_lowercase());
        } else if matches!(c, '\'' | '’') && !word.is_empty() {
            continue;
        } else if !word.is_empty() {
            found.push((std::mem::take(&mut word), at));
        }
    }
    if !word.is_empty() {
        found.push((word, text.len()));
    }
    found
}

/// The edits that turn `a` into `b`.
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut last = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let here = row[j + 1];
            row[j + 1] = (last + usize::from(ca != cb)).min(row[j] + 1).min(here + 1);
            last = here;
        }
    }
    row[b.len()]
}

/// Whether a heard word is the wake word's: the same, or, for a word of
/// three letters or more, one letter off (transcribers spell a name as
/// they hear it: "zion", "scion").
fn close(heard: &str, wanted: &str) -> bool {
    heard == wanted
        || (heard.chars().count().min(wanted.chars().count()) >= 3 && distance(heard, wanted) <= 1)
}

/// The words heard before the wake phrase that are let pass ("okay, hey
/// sion").
const LEADING: usize = 2;

/// The phrases `text` names, split by `|` (`"hey sion|ok sion"`), each as
/// its words.
pub fn phrases_of(text: &str) -> Result<Vec<Vec<String>>, String> {
    let phrases: Vec<Vec<String>> = text
        .split('|')
        .map(|p| words(p).into_iter().map(|(w, _)| w).collect::<Vec<_>>())
        .filter(|p| !p.is_empty())
        .collect();
    if phrases.is_empty() {
        return Err("give the phrase: \"hey sion\"".into());
    }
    Ok(phrases)
}

/// What follows one of `phrases` in `heard`, wherever it falls rather than
/// only at the start: how a meeting's guest knows it was spoken to in the
/// middle of a sentence ("so, Notes, could you summarize?"). The words are
/// matched as leniently as a wake phrase's, so a transcriber that spells a
/// name a letter off is still understood. `None` if no phrase is there, and
/// an empty string if the phrase ends the line (it was named, but asked
/// nothing).
pub fn spoken_to(phrases: &[Vec<String>], heard: &str) -> Option<String> {
    let said = words(heard);
    for at in 0..said.len() {
        for phrase in phrases {
            let n = phrase.len();
            if at + n <= said.len()
                && phrase
                    .iter()
                    .zip(&said[at..at + n])
                    .all(|(wanted, (word, _))| close(word, wanted))
            {
                return Some(rest_after(heard, said[at + n - 1].1));
            }
            // Its words run together ("heysion").
            if n > 1 && close(&said[at].0, &phrase.concat()) {
                return Some(rest_after(heard, said[at].1));
            }
        }
    }
    None
}

impl Wake {
    /// Phrases split by `|` (`"hey sion|ok sion"`), heard by `hearing`.
    pub fn parse(phrases: &str, hearing: Hearing, awake_secs: u64) -> Result<Wake, String> {
        let phrases = phrases_of(phrases)
            .map_err(|_| "give the wake phrase: --wake \"hey sion\"".to_string())?;
        Ok(Wake {
            phrases,
            hearing,
            awake_secs,
        })
    }

    /// Where the wake phrase ends at the start of `heard`, and the words it
    /// took: `None` if it is not there.
    fn find(&self, heard: &str) -> Option<(usize, usize)> {
        let said = words(heard);
        for skip in 0..=LEADING {
            for phrase in &self.phrases {
                // Its words, one by one…
                let n = phrase.len();
                if skip + n <= said.len()
                    && phrase
                        .iter()
                        .zip(&said[skip..skip + n])
                        .all(|(wanted, (word, _))| close(word, wanted))
                {
                    return Some((said[skip + n - 1].1, skip + n));
                }
                // …or run together ("heysion").
                if n > 1 && skip < said.len() && close(&said[skip].0, &phrase.concat()) {
                    return Some((said[skip].1, skip + 1));
                }
            }
        }
        None
    }

    /// What follows the wake phrase at the start of `heard`, if it is
    /// there.
    pub fn strip(&self, heard: &str) -> Option<String> {
        self.find(heard).map(|(end, _)| rest_after(heard, end))
    }
}

/// `text` after byte `end`, without the marks that followed the word there.
fn rest_after(text: &str, end: usize) -> String {
    text[end..]
        .trim_start_matches(|c: char| {
            c.is_whitespace() || matches!(c, ',' | '.' | '!' | '?' | ':' | ';' | '-' | '—' | '–')
        })
        .trim()
        .to_string()
}

/// `text` without its first `n` words.
fn without_words(text: &str, n: usize) -> String {
    match words(text).get(n.wrapping_sub(1)) {
        Some((_, end)) if n > 0 => rest_after(text, *end),
        _ => text.trim().to_string(),
    }
}

/// Whether a transcript says nothing: empty, or only what a transcriber
/// writes for silence (`[BLANK_AUDIO]`, `(silence)`).
fn nothing_said(text: &str) -> bool {
    let mut depth = 0usize;
    let mut said = false;
    for c in text.chars() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = depth.saturating_sub(1),
            c if depth == 0 && c.is_alphanumeric() => said = true,
            _ => {}
        }
    }
    !said
}

/// What was heard of an utterance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Heard {
    /// Nothing that was speech.
    Nothing,
    /// Said while it slept, without the wake word: what it heard.
    NotWoken(String),
    /// Said to it: the words (without the wake phrase), and whether the
    /// wake phrase began them.
    Said {
        /// The words.
        text: String,
        /// Whether it was woken by them.
        woke: bool,
    },
}

/// Hears an utterance: with a wake word and `asleep`, first by the wake
/// word's transcriber, and, if it was woken, again by the conversation's.
pub fn hear(
    hearing: &Hearing,
    wake: Option<&Wake>,
    audio: &Audio,
    asleep: bool,
) -> Result<Heard, String> {
    match wake {
        Some(wake) if asleep => {
            let first = wake.hearing.hear(audio)?;
            if nothing_said(&first) {
                return Ok(Heard::Nothing);
            }
            let Some((end, taken)) = wake.find(&first) else {
                return Ok(Heard::NotWoken(first.trim().to_string()));
            };
            let text = if wake.hearing == *hearing {
                rest_after(&first, end)
            } else {
                // Woken: what was said is the conversation's, heard by its
                // transcriber.
                match hearing.hear(audio) {
                    Ok(again) if !nothing_said(&again) => wake
                        .strip(&again)
                        .unwrap_or_else(|| without_words(&again, taken)),
                    _ => rest_after(&first, end),
                }
            };
            Ok(Heard::Said { text, woke: true })
        }
        _ => {
            let text = hearing.hear(audio)?;
            if nothing_said(&text) {
                return Ok(Heard::Nothing);
            }
            Ok(match wake.and_then(|w| w.strip(&text)) {
                Some(text) => Heard::Said { text, woke: true },
                None => Heard::Said {
                    text: text.trim().to_string(),
                    woke: false,
                },
            })
        }
    }
}

/// The longest stretch of a reply spoken without a sentence's end.
const LONGEST: usize = 300;

/// Abbreviations a full stop after does not end a sentence.
const ABBREVIATIONS: &[&str] = &[
    "e.g", "i.e", "vs", "mr", "mrs", "ms", "dr", "st", "cf", "approx", "eg", "ie",
];

/// Whether the text before a full stop ends in an abbreviation (or an
/// initial).
fn abbreviation(before: &str) -> bool {
    let word = before
        .rsplit(|c: char| c.is_whitespace() || c == '(')
        .next()
        .unwrap_or_default()
        .to_lowercase();
    ABBREVIATIONS.contains(&word.as_str())
        || (word.chars().count() == 1 && word.chars().all(char::is_alphabetic))
}

/// Where the first sentence of `text` ends, if it is complete: after its
/// mark (and the quotes or brackets that close it), once what follows is
/// seen to be a space; or at a line's end.
fn boundary(text: &str) -> Option<usize> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (i, &(at, c)) in chars.iter().enumerate() {
        if c == '\n' {
            if text[..at].trim().is_empty() {
                continue;
            }
            return Some(at + 1);
        }
        if !matches!(c, '.' | '!' | '?' | '…' | '。' | '！' | '？') {
            continue;
        }
        let mut j = i + 1;
        while j < chars.len() && matches!(chars[j].1, '"' | '\'' | ')' | ']' | '”' | '’' | '»')
        {
            j += 1;
        }
        let end = chars.get(j).map(|&(at, _)| at).unwrap_or(text.len());
        // These need no space after them.
        if matches!(c, '。' | '！' | '？') {
            return Some(end);
        }
        let Some(&(_, next)) = chars.get(j) else {
            // What follows is not written yet.
            return None;
        };
        if !next.is_whitespace() || (c == '.' && abbreviation(&text[..at])) {
            continue;
        }
        return Some(end);
    }
    None
}

/// Text as it is said: links by their words, addresses as on the screen,
/// and no markdown's marks. `None` if nothing is left to say.
pub fn speakable(text: &str) -> Option<String> {
    static LINK: OnceLock<regex::Regex> = OnceLock::new();
    static URL: OnceLock<regex::Regex> = OnceLock::new();
    let link = LINK.get_or_init(|| regex::Regex::new(r"\[([^\]]*)\]\([^)]*\)").expect("a pattern"));
    let url = URL.get_or_init(|| regex::Regex::new(r"https?://[^\s)\]]+").expect("a pattern"));
    let text = link.replace_all(text, "$1");
    let text = url.replace_all(&text, "the link on the screen");
    let mut said = Vec::new();
    for line in text.lines() {
        let line = line
            .trim_start()
            .trim_start_matches(['#', '>'])
            .trim_start();
        let line = ["- ", "* ", "• ", "+ "]
            .iter()
            .find_map(|bullet| line.strip_prefix(bullet))
            .unwrap_or(line);
        let line = line
            .replace("**", "")
            .replace("__", "")
            .replace("~~", "")
            .replace(['*', '`'], "")
            .replace('|', " ");
        let line = line.trim();
        if line.is_empty()
            || line
                .chars()
                .all(|c| matches!(c, '-' | '=' | '_' | ':' | ' '))
        {
            continue;
        }
        said.push(line.to_string());
    }
    let said = said
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    said.chars().any(char::is_alphanumeric).then_some(said)
}

/// What is said instead of a block of code.
const CODE: &str = "The code is on the screen.";

/// Splits a reply, as it is written, into what to say a sentence at a time.
#[derive(Debug, Default)]
pub struct Sentences {
    pending: String,
    in_code: bool,
}

impl Sentences {
    /// Takes more of the reply: the sentences it completes, as said.
    pub fn push(&mut self, more: &str) -> Vec<String> {
        self.pending.push_str(more);
        let mut said = Vec::new();
        loop {
            if self.in_code {
                match self.pending.find("```") {
                    Some(at) => {
                        self.pending.drain(..at + 3);
                        self.in_code = false;
                        continue;
                    }
                    None => {
                        // A fence may be coming, in pieces.
                        let mut keep = self.pending.len().saturating_sub(2);
                        while !self.pending.is_char_boundary(keep) {
                            keep -= 1;
                        }
                        self.pending.drain(..keep);
                        break;
                    }
                }
            }
            let fence = self.pending.find("```");
            let end = boundary(&self.pending);
            let code_first = match (fence, end) {
                (Some(fence), Some(end)) => fence < end,
                (Some(_), None) => true,
                (None, _) => false,
            };
            match (fence, end) {
                (Some(fence), _) if code_first => {
                    if !self.open_code(fence, &mut said) {
                        break;
                    }
                }
                (_, Some(end)) => {
                    said.extend(speakable(&self.pending[..end]));
                    self.pending.drain(..end);
                }
                (_, None) => {
                    if self.pending.chars().count() > LONGEST {
                        let cut = self
                            .pending
                            .char_indices()
                            .take(LONGEST)
                            .filter(|(_, c)| c.is_whitespace())
                            .map(|(at, _)| at)
                            .last();
                        if let Some(cut) = cut.filter(|&cut| cut > 0) {
                            said.extend(speakable(&self.pending[..cut]));
                            self.pending.drain(..cut);
                            continue;
                        }
                    }
                    break;
                }
            }
        }
        said
    }

    /// Says what comes before the fence at `fence`, and passes over the
    /// code it opens: `false` if its line is not written whole yet.
    fn open_code(&mut self, fence: usize, said: &mut Vec<String>) -> bool {
        said.extend(speakable(&self.pending[..fence]));
        // The fence's line (its language) goes with it.
        match self.pending[fence + 3..].find('\n') {
            Some(n) => {
                self.pending.drain(..fence + 3 + n + 1);
                self.in_code = true;
                said.push(CODE.to_string());
                true
            }
            None => {
                self.pending.drain(..fence);
                false
            }
        }
    }

    /// What is left at the reply's end, as said.
    pub fn finish(&mut self) -> Option<String> {
        let rest = std::mem::take(&mut self.pending);
        if std::mem::replace(&mut self.in_code, false) {
            return None;
        }
        speakable(&rest)
    }
}

/// What happens in a live conversation: what the microphone, the keyboard,
/// the transcriber, the run and the voice tell the conversation.
#[derive(Debug)]
pub enum Event {
    /// Speech began over the agent.
    Began,
    /// An utterance ended.
    Utterance(Vec<i16>),
    /// The microphone gave no more sound (why, if it failed).
    MicrophoneStopped(String),
    /// A line was typed.
    Typed(String),
    /// The input ended.
    InputEnded,
    /// What an utterance was heard to say.
    Heard(Result<Heard, String>),
    /// More of the reply.
    Delta(String),
    /// A tool runs.
    Tool(String),
    /// An MCP server asks the person something; the reply goes back here.
    Question(String, Sender<Option<String>>),
    /// An MCP server tells the person something.
    Told(String),
    /// The run ended: its reply, or why it failed.
    RunEnded(Result<String, String>),
    /// A command typed while the run went on was answered.
    Commanded(Result<String, String>),
    /// A sentence was spoken (said while the voice was at `epoch`).
    Played(u64),
    /// A sentence could not be spoken.
    NotSpoken(u64, String),
    /// The wait for the rest of what the person was saying, held as
    /// unfinished ([`Do::Wait`]), ended.
    HoldEnded(u64),
}

/// What the conversation does next, as [`State`] decides.
#[derive(Debug, PartialEq)]
enum Do {
    /// Writes to the terminal.
    Print(String),
    /// Speaks.
    Say(String),
    /// Falls silent, at once, and forgets what it was to say.
    Hush,
    /// Stops the run before its next step.
    Stop,
    /// Runs a message.
    Run(String),
    /// Runs a command while the run goes on.
    Command(String),
    /// Answers the question asked.
    Answer(Option<String>),
    /// Hears an utterance (`true`: asleep, so only what begins with the
    /// wake word).
    Hear(Vec<i16>, bool),
    /// Listens to the microphone so.
    Listen(u8),
    /// Waits as long for the rest of what is held as unfinished, then
    /// says so ([`Event::HoldEnded`] with the hold's number).
    Wait(u64, Duration),
    /// Ends the conversation.
    End,
}

/// What a live conversation is doing, and what it does next for each
/// event: kept apart from the threads, the programs and the clock.
#[derive(Debug)]
struct State {
    /// With a wake word: how long it stays awake after speaking.
    wake_secs: Option<u64>,
    /// Whether speaking over it stops it.
    interrupt: bool,
    /// Whether replies are spoken.
    voiced: bool,
    running: bool,
    /// A question is asked, and waits for its reply.
    asked: bool,
    /// The reply under way was interrupted.
    interrupted: bool,
    /// Sentences given to the voice and not yet said.
    unsaid: usize,
    /// Times the voice was silenced: what it says after is told apart.
    epoch: u64,
    /// What was said while the agent was at work, for it next.
    pending: Vec<String>,
    /// Utterances being heard.
    hearing: usize,
    awake_until: Option<Instant>,
    mid_line: bool,
    /// Some of the reply was written.
    printed: bool,
    sentences: Sentences,
    ending: bool,
    ended: bool,
    mode: u8,
    /// With turns told by what was said: how long what sounds unfinished
    /// is held for the rest.
    hold: Option<Duration>,
    /// What the person said that sounds unfinished, held for the rest.
    held: Option<String>,
    /// The number of the hold under way.
    held_id: u64,
    /// The hold under way ran out while an utterance was being heard.
    hold_ran_out: bool,
}

impl State {
    fn new(wake_secs: Option<u64>, interrupt: bool, voiced: bool) -> State {
        State {
            wake_secs,
            interrupt,
            voiced,
            running: false,
            asked: false,
            interrupted: false,
            unsaid: 0,
            epoch: 0,
            pending: Vec::new(),
            hearing: 0,
            awake_until: None,
            mid_line: false,
            printed: false,
            sentences: Sentences::default(),
            ending: false,
            ended: false,
            mode: LISTENING,
            hold: None,
            held: None,
            held_id: 0,
            hold_ran_out: false,
        }
    }

    /// Tells turns by what was said: what sounds unfinished is held as long
    /// as `hold` for the rest.
    fn holding(mut self, hold: Option<Duration>) -> State {
        self.hold = hold;
        self
    }

    /// What the person was heard to say: taken, or, if it sounds unfinished
    /// ([`finished`]) and turns are told by what was said, held for the
    /// rest, what came before it in front.
    fn heard(&mut self, text: String, todo: &mut Vec<Do>) {
        let words = match self.held.take() {
            Some(held) => format!("{held} {text}"),
            None => text,
        };
        match self.hold {
            Some(hold) if !finished(&words) => {
                self.held_id += 1;
                self.hold_ran_out = false;
                self.held = Some(words);
                todo.push(Do::Wait(self.held_id, hold));
            }
            _ => self.take(words, todo),
        }
    }

    /// Takes what is held, its hold over.
    fn let_go(&mut self, todo: &mut Vec<Do>) {
        self.hold_ran_out = false;
        if let Some(words) = self.held.take() {
            self.take(words, todo);
        }
    }

    fn speaking(&self) -> bool {
        self.unsaid > 0
    }

    /// Whether the agent works (and is not waiting on a reply) or speaks.
    fn busy(&self) -> bool {
        (self.running && !self.asked && !self.interrupted) || self.speaking()
    }

    fn idle(&self) -> bool {
        !self.running
            && !self.speaking()
            && !self.asked
            && self.pending.is_empty()
            && self.hearing == 0
            && self.held.is_none()
    }

    /// `text` on a line of its own.
    fn line(&mut self, text: &str, todo: &mut Vec<Do>) {
        let mut shown = String::new();
        if self.mid_line {
            shown.push('\n');
            self.mid_line = false;
        }
        shown.push_str(text);
        shown.push('\n');
        todo.push(Do::Print(shown));
    }

    fn say(&mut self, text: String, todo: &mut Vec<Do>) {
        self.unsaid += 1;
        todo.push(Do::Say(text));
    }

    /// Awake, after speaking, for a follow-up.
    fn awake(&mut self, now: Instant) {
        if let Some(secs) = self.wake_secs {
            self.awake_until = Some(now + Duration::from_secs(secs));
        }
    }

    fn start(&mut self, text: String, todo: &mut Vec<Do>) {
        self.running = true;
        self.interrupted = false;
        self.printed = false;
        self.sentences = Sentences::default();
        todo.push(Do::Run(text));
    }

    /// What the person said or typed: the reply to a question asked, a
    /// message for after the run, or one to run.
    fn take(&mut self, text: String, todo: &mut Vec<Do>) {
        if self.asked {
            self.asked = false;
            todo.push(Do::Answer(Some(text)));
        } else if self.running {
            self.pending.push(text);
        } else {
            self.start(text, todo);
        }
    }

    /// Silences it, and stops its run: a question asked stays asked.
    fn interrupt_now(&mut self, todo: &mut Vec<Do>) {
        if self.speaking() {
            self.unsaid = 0;
            self.epoch += 1;
            todo.push(Do::Hush);
        }
        if self.asked || self.interrupted {
            return;
        }
        if self.running {
            todo.push(Do::Stop);
        }
        self.interrupted = true;
        self.line("(interrupted)", todo);
    }

    /// A sentence said (or not, and why) by the voice at `epoch`: one less
    /// to say, if it is not one silenced since.
    fn said_one(&mut self, epoch: u64, failed: Option<String>, now: Instant, todo: &mut Vec<Do>) {
        if epoch != self.epoch {
            return;
        }
        if let Some(why) = failed {
            self.line(&format!("(not spoken: {why})"), todo);
        }
        self.unsaid = self.unsaid.saturating_sub(1);
        if !self.speaking() && (!self.running || self.asked) {
            self.awake(now);
        }
    }

    /// What to do for `event`, at `now`.
    fn on(&mut self, event: Event, now: Instant) -> Vec<Do> {
        let mut todo = Vec::new();
        match event {
            Event::Began => {
                if self.wake_secs.is_none() && self.interrupt && self.busy() {
                    self.interrupt_now(&mut todo);
                }
            }
            Event::Utterance(speech) => {
                // A question's answer too: awake for it a while after it
                // is asked, then only with the wake word.
                let asleep = self.wake_secs.is_some()
                    && (self.busy() || self.awake_until.map_or(true, |until| now >= until));
                self.hearing += 1;
                todo.push(Do::Hear(speech, asleep));
            }
            Event::Heard(heard) => {
                self.hearing = self.hearing.saturating_sub(1);
                let mut said = false;
                match heard {
                    Err(why) => self.line(&format!("(not heard: {why})"), &mut todo),
                    Ok(Heard::Nothing) => {}
                    Ok(Heard::NotWoken(text)) => {
                        let shown: String = text.chars().take(60).collect();
                        let more = if text.chars().count() > 60 { "…" } else { "" };
                        self.line(&format!("(not woken: heard \"{shown}{more}\")"), &mut todo);
                    }
                    Ok(Heard::Said { text, woke }) => {
                        if woke {
                            // Called by its name, over its reply.
                            if self.busy() {
                                self.interrupt_now(&mut todo);
                            }
                            self.awake(now);
                        }
                        if !text.is_empty() {
                            said = true;
                            self.line(&format!("you (heard): {text}"), &mut todo);
                            self.heard(text, &mut todo);
                        } else if woke {
                            self.line("(awake: listening)", &mut todo);
                        }
                    }
                }
                // A hold that ran out while this was heard, which added
                // nothing to it, is over.
                if !said && self.hold_ran_out && self.hearing == 0 {
                    self.let_go(&mut todo);
                }
            }
            Event::HoldEnded(id) => {
                if id == self.held_id && self.held.is_some() {
                    if self.hearing == 0 {
                        self.let_go(&mut todo);
                    } else {
                        // What is being heard may be the rest.
                        self.hold_ran_out = true;
                    }
                }
            }
            Event::Typed(line) => {
                let line = line.trim().to_string();
                if line.is_empty() {
                } else if self.asked {
                    self.take(line, &mut todo);
                } else if line == "/stop" && self.busy() {
                    self.interrupt_now(&mut todo);
                } else if self.running && line.starts_with('/') {
                    todo.push(Do::Command(line));
                } else {
                    self.take(line, &mut todo);
                }
            }
            Event::InputEnded => {
                self.ending = true;
                if self.asked {
                    self.asked = false;
                    todo.push(Do::Answer(None));
                }
            }
            Event::MicrophoneStopped(why) => {
                let shown = if why.is_empty() {
                    "(the microphone stopped)".to_string()
                } else {
                    format!("(the microphone stopped: {why})")
                };
                self.line(&shown, &mut todo);
                self.ending = true;
            }
            Event::Delta(text) => {
                if !self.interrupted && !text.is_empty() {
                    let mut shown = String::new();
                    if !self.mid_line {
                        shown.push_str("agent> ");
                    }
                    shown.push_str(&text);
                    self.mid_line = true;
                    self.printed = true;
                    todo.push(Do::Print(shown));
                    if self.voiced {
                        for sentence in self.sentences.push(&text) {
                            self.say(sentence, &mut todo);
                        }
                    }
                }
            }
            Event::Tool(tool) => {
                if !self.interrupted {
                    self.line(&format!("  · {tool}"), &mut todo);
                }
            }
            Event::Question(question, _) => {
                self.line(&question, &mut todo);
                self.asked = true;
                match speakable(&question).filter(|_| self.voiced) {
                    Some(said) => self.say(said, &mut todo),
                    // Awake for the answer from when it is shown.
                    None => self.awake(now),
                }
            }
            Event::Told(text) => self.line(&text, &mut todo),
            Event::RunEnded(result) => {
                self.running = false;
                match result {
                    Ok(reply) => {
                        if !self.printed && !self.interrupted && !reply.is_empty() {
                            // A command's answer comes whole.
                            self.line(&format!("agent> {reply}"), &mut todo);
                            if self.voiced {
                                for sentence in self.sentences.push(&reply) {
                                    self.say(sentence, &mut todo);
                                }
                            }
                        } else if self.mid_line {
                            self.mid_line = false;
                            todo.push(Do::Print("\n".into()));
                        }
                        if self.voiced && !self.interrupted {
                            if let Some(rest) = self.sentences.finish() {
                                self.say(rest, &mut todo);
                            }
                        }
                    }
                    Err(why) => self.line(&format!("(that did not work: {why})"), &mut todo),
                }
                if !self.pending.is_empty() {
                    let next = std::mem::take(&mut self.pending).join("\n");
                    self.start(next, &mut todo);
                } else if !self.speaking() {
                    self.awake(now);
                }
            }
            Event::Commanded(result) => {
                let shown = match result {
                    Ok(reply) => format!("agent> {reply}"),
                    Err(why) => format!("(that did not work: {why})"),
                };
                self.line(&shown, &mut todo);
            }
            Event::Played(epoch) => self.said_one(epoch, None, now, &mut todo),
            Event::NotSpoken(epoch, why) => self.said_one(epoch, Some(why), now, &mut todo),
        }
        let mode = if !self.busy() {
            LISTENING
        } else if self.interrupt {
            OVER
        } else {
            DEAF
        };
        if mode != self.mode {
            self.mode = mode;
            todo.push(Do::Listen(mode));
        }
        if self.ending && self.hearing == 0 && self.held.is_some() {
            self.let_go(&mut todo);
        }
        if self.ending && !self.ended && self.idle() {
            self.ended = true;
            todo.push(Do::End);
        }
        todo
    }
}

/// Words that what is said rarely ends on: what ends on one sounds
/// unfinished, the speaker pausing before the rest.
const GOES_ON: &[&str] = &[
    "and", "but", "or", "nor", "so", "because", "cause", "if", "then", "than", "to", "of", "the",
    "a", "an", "with", "without", "for", "at", "from", "into", "onto", "about", "by", "via", "as",
    "which", "who", "whose", "where", "whereas", "while", "although", "though", "unless", "until",
    "since", "whether", "either", "neither", "my", "your", "our", "their", "his", "her", "its",
    "um", "uh", "er", "erm", "uhm", "hmm", "mm",
];

/// Whether `said` sounds finished: it asks or exclaims, or ends on
/// anything but a comma, a dash, an ellipsis or a word that goes on
/// ([`GOES_ON`]). Transcribers put in the punctuation; a word that goes on
/// sounds unfinished even with a full stop after it.
pub fn finished(said: &str) -> bool {
    let said = said.trim();
    if said.is_empty() || said.ends_with(['?', '!']) {
        return true;
    }
    if said.ends_with([',', ';', ':', '-', '–', '—', '…']) || said.ends_with("...") {
        return false;
    }
    let last: String = said
        .rsplit(char::is_whitespace)
        .next()
        .unwrap_or_default()
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
        .to_lowercase();
    !GOES_ON.contains(&last.as_str())
}

/// How a live conversation is had.
#[derive(Clone, Debug, PartialEq)]
pub struct Listening {
    /// The program that gives the microphone's sound: raw 16-bit mono
    /// samples, at the detection's rate, on its standard output.
    pub listen: Vec<String>,
    /// A wake word, if what is said must begin with one.
    pub wake: Option<Wake>,
    /// Whether speaking over the agent stops it.
    pub interrupt: bool,
    /// How speech is told from silence.
    pub detection: Detection,
    /// With echo cancelled: the program that plays raw samples at 16 kHz
    /// on its standard input.
    pub play_raw: Option<Vec<String>>,
    /// With turns told by what was said ([`Listening::semantic`]): how
    /// long what sounds unfinished is held for the rest.
    pub hold: Option<Duration>,
}

/// How long what sounds unfinished is held for the rest of it.
pub const HOLD: Duration = Duration::from_millis(1_500);

/// The quiet frames that end an utterance when turns are told by what was
/// said: 500 ms, what sounds unfinished being held besides.
pub const SEMANTIC_END: usize = 25;

impl Listening {
    /// Turns told by what was said, not by silence alone: an utterance
    /// ends after a shorter pause, and what sounds unfinished ([`finished`])
    /// is held for the rest, as long as [`HOLD`].
    pub fn semantic(mut self) -> Listening {
        self.hold = Some(HOLD);
        self.detection.end = SEMANTIC_END;
        self
    }
}

/// Where a transcriber that is not on this machine sends what it hears.
fn elsewhere(hearing: &Hearing) -> String {
    match hearing {
        Hearing::Api { base, .. } => base.clone(),
        Hearing::Deepgram { .. } => "Deepgram".into(),
        Hearing::ElevenLabs { .. } => "ElevenLabs".into(),
        Hearing::Command(_) | Hearing::Whisper { .. } => "this machine".into(),
    }
}

impl Listening {
    /// From `sion agent talk`'s flags: the microphone program named, or the
    /// first of [`microphones`] there; the wake word, heard by a
    /// transcriber on this machine unless `wake_cloud` lets one elsewhere
    /// hear everything said near the microphone.
    #[allow(clippy::too_many_arguments)]
    pub fn from_flags(
        hearing: Option<&Hearing>,
        listen: Option<&str>,
        wake: Option<&str>,
        wake_hear: Option<&str>,
        wake_cloud: bool,
        awake_secs: u64,
        interrupt: bool,
        speech_level: f64,
        detector: &str,
        echo_cancel: Option<Option<&str>>,
    ) -> Result<Listening, String> {
        let model = match detector {
            "model" => true,
            "loudness" => false,
            other => return Err(format!("--detector is model or loudness, not {other}")),
        };
        let play_raw = match echo_cancel {
            None => None,
            Some(named) => Some(crate::talk::program(named, RAW_PLAYERS).ok_or_else(|| {
                "cancelling the loudspeaker's echo plays raw samples: install alsa-utils \
                 (aplay), pulseaudio-utils (paplay), sox or ffmpeg, give --play-raw, or \
                 --no-echo-cancel (then use headphones)"
                    .to_string()
            })?),
        };
        if !(1.0..=32_767.0).contains(&speech_level) {
            return Err(format!(
                "--speech-level is a level of 1 to 32767 (the default is {})",
                Detection::default().level
            ));
        }
        let listen = crate::talk::program(listen, &microphones()).ok_or_else(|| {
            "nothing gives the microphone's sound: install sox (rec), alsa-utils (arecord), \
             pulseaudio-utils (parec) or ffmpeg, or give --listen"
                .to_string()
        })?;
        let wake = match wake {
            None => None,
            Some(phrases) => {
                let heard_by = match wake_hear {
                    Some(spec) => Hearing::parse(spec)?,
                    None => hearing.cloned().ok_or("a wake word needs --hear")?,
                };
                if !heard_by.is_local() && !wake_cloud {
                    return Err(format!(
                        "a wake word is found by hearing everything said near the microphone: \
                         give a transcriber on this machine (--wake-hear whisper:<model file>, \
                         local:<model> or command:<program>), or --wake-cloud to let {} hear it all",
                        elsewhere(&heard_by)
                    ));
                }
                Some(Wake::parse(phrases, heard_by, awake_secs)?)
            }
        };
        Ok(Listening {
            listen,
            wake,
            interrupt,
            detection: Detection {
                level: speech_level,
                model,
                ..Detection::default()
            },
            play_raw,
            hold: None,
        })
    }
}

/// Reads the microphone's sound a frame at a time, and tells the
/// conversation the utterances found in it (and, over the agent, when
/// speech begins).
pub(crate) fn capture(
    mut sound: impl Read,
    detection: Detection,
    mode: &AtomicU8,
    events: &Sender<Event>,
    mut canceller: Option<crate::sound::Canceller>,
) {
    let size = detection.frame;
    let mut detector = Detector::new(detection);
    let mut bytes = vec![0u8; size * 2];
    let mut frame = vec![0i16; size];
    while sound.read_exact(&mut bytes).is_ok() {
        for (sample, pair) in frame.iter_mut().zip(bytes.chunks_exact(2)) {
            *sample = i16::from_le_bytes([pair[0], pair[1]]);
        }
        // The loudspeaker's echo taken out first, every frame, so that the
        // canceller keeps the microphone's count.
        if let Some(canceller) = canceller.as_mut() {
            frame = canceller.frame(&frame);
        }
        let heard = match mode.load(Ordering::SeqCst) {
            DEAF => {
                detector.reset();
                continue;
            }
            mode => detector.frame(&frame, mode == OVER),
        };
        let event = match heard {
            Sound::Began => Event::Began,
            Sound::Utterance(speech) => Event::Utterance(speech),
            Sound::Nothing => continue,
        };
        if events.send(event).is_err() {
            return;
        }
    }
    if let Some(speech) = detector.flush() {
        let _ = events.send(Event::Utterance(speech));
    }
    let _ = events.send(Event::MicrophoneStopped(String::new()));
}

/// The last line a program wrote to `from` (its standard error).
fn last_line(mut from: impl Read) -> String {
    let mut kept = Vec::new();
    let mut buffer = [0u8; 1024];
    while let Ok(n) = from.read(&mut buffer) {
        if n == 0 {
            break;
        }
        kept.extend_from_slice(&buffer[..n]);
        if kept.len() > 4096 {
            kept.drain(..kept.len() - 2048);
        }
    }
    String::from_utf8_lossy(&kept)
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Why the microphone stopped: nothing if it ended as it should.
fn microphone_ended(microphone: &mut Child, said: Option<JoinHandle<String>>) -> String {
    let deadline = Instant::now() + Duration::from_secs(1);
    let status = loop {
        match microphone.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => break None,
        }
    };
    match status {
        Some(status) if status.success() => String::new(),
        Some(status) => {
            let said = said.and_then(|s| s.join().ok()).unwrap_or_default();
            if said.is_empty() {
                status.to_string()
            } else {
                format!("{status}: {said}")
            }
        }
        None => "it gives no more sound".into(),
    }
}

/// What the voice plays, and what it was at when it began.
pub(crate) struct Playing {
    pub(crate) epoch: AtomicU64,
    pub(crate) child: Mutex<Option<Child>>,
    /// The raw player's input, while it runs.
    pub(crate) input: Mutex<Option<std::process::ChildStdin>>,
}

/// A piece of raw samples handed to the player: 20 ms.
pub(crate) const PIECE: usize = (crate::sound::RATE / 50) as usize;

/// Pieces handed on ahead of their time, so that the player never runs
/// dry: 100 ms.
pub(crate) const AHEAD: usize = 5;

impl Playing {
    /// A voice that plays nothing yet.
    pub(crate) fn new() -> Playing {
        Playing {
            epoch: AtomicU64::new(0),
            child: Mutex::new(None),
            input: Mutex::new(None),
        }
    }

    /// Waits until `until`, or until the voice is silenced since `epoch`:
    /// whether it was not.
    pub(crate) fn wait(&self, until: Instant, epoch: u64) -> bool {
        loop {
            if self.epoch.load(Ordering::SeqCst) != epoch {
                return false;
            }
            let now = Instant::now();
            if now >= until {
                return true;
            }
            std::thread::sleep((until - now).min(Duration::from_millis(10)));
        }
    }

    /// Plays raw `samples` (at 16 kHz) through `play`, a piece every 20
    /// ms, each told to `far` as it is handed on, unless the voice was
    /// silenced since `epoch`; silencing it kills the player, and the next
    /// sentence starts another.
    pub(crate) fn play_raw(
        &self,
        play: &[String],
        samples: &[i16],
        epoch: u64,
        far: &crate::sound::FarLine,
    ) -> Result<(), String> {
        let started = Instant::now();
        let pieces: Vec<&[i16]> = samples.chunks(PIECE).collect();
        let at = |n: usize| started + Duration::from_millis(20 * n as u64);
        for (i, piece) in pieces.iter().enumerate() {
            if i >= AHEAD && !self.wait(at(i - AHEAD), epoch) {
                return Ok(());
            }
            if !self.hand_on(play, piece, epoch, far)? {
                return Ok(());
            }
        }
        // Until what was handed on has been heard.
        self.wait(at(pieces.len()), epoch);
        Ok(())
    }

    /// Hands `piece` to the raw player `play`, started if it does not run,
    /// and tells `far`: false if the voice was silenced since `epoch`.
    pub(crate) fn hand_on(
        &self,
        play: &[String],
        piece: &[i16],
        epoch: u64,
        far: &crate::sound::FarLine,
    ) -> Result<bool, String> {
        {
            let mut child = self.child.lock().unwrap_or_else(|e| e.into_inner());
            if self.epoch.load(Ordering::SeqCst) != epoch {
                return Ok(false);
            }
            let running = matches!(child.as_mut().map(Child::try_wait), Some(Ok(None)));
            if !running {
                let mut started = std::process::Command::new(&play[0])
                    .args(&play[1..])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|e| format!("{}: {e}", play[0]))?;
                *self.input.lock().unwrap_or_else(|e| e.into_inner()) = started.stdin.take();
                *child = Some(started);
            }
        }
        let bytes: Vec<u8> = piece.iter().flat_map(|s| s.to_le_bytes()).collect();
        let written = self
            .input
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
            .map(|input| input.write_all(&bytes).and_then(|()| input.flush()));
        match written {
            Some(Ok(())) => {
                far.played(piece);
                Ok(true)
            }
            _ if self.epoch.load(Ordering::SeqCst) != epoch => Ok(false),
            _ => Err(format!("{} stopped taking sound", play[0])),
        }
    }

    /// Silences it: what it plays stops, and what it had still to play is
    /// dropped.
    pub(crate) fn hush(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        let mut child = self.child.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(child) = child.as_mut() {
            let _ = child.kill();
        }
    }

    /// Plays `file` with `play`, unless the voice was silenced since
    /// `epoch`; silencing it kills the player.
    fn play(&self, play: &[String], file: &Path, epoch: u64) -> Result<(), String> {
        {
            let mut child = self.child.lock().unwrap_or_else(|e| e.into_inner());
            if self.epoch.load(Ordering::SeqCst) != epoch {
                return Ok(());
            }
            *child = Some(
                crate::talk::with_file(play, file)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|e| format!("{}: {e}", play[0]))?,
            );
        }
        loop {
            {
                let mut child = self.child.lock().unwrap_or_else(|e| e.into_inner());
                match child.as_mut().map(Child::try_wait) {
                    Some(Ok(None)) => {}
                    Some(Ok(Some(status))) => {
                        *child = None;
                        return if status.success() {
                            Ok(())
                        } else {
                            Err(format!("{} failed ({status})", play[0]))
                        };
                    }
                    Some(Err(e)) => {
                        *child = None;
                        return Err(e.to_string());
                    }
                    None => return Ok(()),
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// A sentence's audio, ready to play: a file, or raw samples.
enum Spoken {
    File(PathBuf),
    Samples(Vec<i16>),
}

/// Speaks a sentence at a time: one thread makes each sentence's audio
/// while another plays the one before; silencing it stops the player and
/// drops what it had still to say.
struct Voice {
    playing: Arc<Playing>,
    lines: Sender<(u64, String)>,
}

impl Voice {
    /// The voice: playing files with `play`, or, with `raw`, raw samples
    /// through its program, told to its line as they are handed on.
    fn start(
        speaking: Speaking,
        play: Vec<String>,
        events: Sender<Event>,
        raw: Option<(Vec<String>, Arc<crate::sound::FarLine>)>,
    ) -> Voice {
        let playing = Arc::new(Playing::new());
        let (lines, said) = channel::<(u64, String)>();
        let (ready, to_play) = channel::<(u64, Result<Spoken, String>)>();
        let made = playing.clone();
        let samples = raw.is_some();
        std::thread::spawn(move || {
            for (epoch, text) in said {
                if made.epoch.load(Ordering::SeqCst) != epoch {
                    continue;
                }
                let spoken = if samples {
                    speaking.speak_samples(&text).map(Spoken::Samples)
                } else {
                    speaking.speak(&text).and_then(|audio| {
                        let extension =
                            audio.name.rsplit('.').next().unwrap_or("audio").to_string();
                        let file = crate::talk::scratch(&extension)?;
                        std::fs::write(&file, &audio.bytes).map_err(|e| e.to_string())?;
                        Ok(Spoken::File(file))
                    })
                };
                if ready.send((epoch, spoken)).is_err() {
                    break;
                }
            }
        });
        let player = playing.clone();
        std::thread::spawn(move || {
            for (epoch, spoken) in to_play {
                let played = spoken.and_then(|spoken| match (spoken, &raw) {
                    (Spoken::Samples(samples), Some((program, far))) => {
                        player.play_raw(program, &samples, epoch, far)
                    }
                    (Spoken::Samples(_), None) => Ok(()),
                    (Spoken::File(file), _) => {
                        let played = player.play(&play, &file, epoch);
                        let _ = std::fs::remove_file(&file);
                        played
                    }
                });
                if player.epoch.load(Ordering::SeqCst) != epoch {
                    continue;
                }
                let event = match played {
                    Ok(()) => Event::Played(epoch),
                    Err(why) => Event::NotSpoken(epoch, why),
                };
                if events.send(event).is_err() {
                    break;
                }
            }
        });
        Voice { playing, lines }
    }

    fn say(&self, text: &str) {
        let epoch = self.playing.epoch.load(Ordering::SeqCst);
        let _ = self.lines.send((epoch, text.to_string()));
    }

    fn hush(&self) {
        self.playing.hush();
    }
}

/// Asks the person in the live conversation.
struct Asking {
    events: Sender<Event>,
}

impl sion_agent::mcp_input::Asker for Asking {
    fn ask(&self, question: &str) -> Option<String> {
        let (replied, reply) = channel();
        self.events
            .send(Event::Question(question.to_string(), replied))
            .ok()?;
        reply.recv().ok().flatten()
    }

    fn tell(&self, text: &str) {
        let _ = self.events.send(Event::Told(text.to_string()));
    }
}

/// Runs `text` in conversation `key`, in a thread of its own, telling
/// `events` the reply as it is written and when it ends.
fn run_message(
    ledger: SharedLedger,
    profile: Profile,
    key: String,
    text: String,
    who: String,
    events: Sender<Event>,
) {
    std::thread::spawn(move || {
        let tell = |event: &str, data: Value| {
            let event = match event {
                "delta" => Event::Delta(data["text"].as_str().unwrap_or_default().to_string()),
                "tool" if data["state"] == "running" => {
                    Event::Tool(data["tool"].as_str().unwrap_or_default().to_string())
                }
                _ => return,
            };
            let _ = events.send(event);
        };
        let asker: Arc<dyn sion_agent::mcp_input::Asker> = Arc::new(Asking {
            events: events.clone(),
        });
        let answer = crate::channels::converse_events_asking(
            ledger,
            &profile,
            &key,
            &text,
            &[],
            &tell,
            Some(&who),
            Some(&asker),
        );
        let _ = events.send(Event::RunEnded(
            answer.map(|a| a.reply).map_err(|e| e.to_string()),
        ));
    });
}

/// Talks with `profile` in conversation `talk.conversation`, listening all
/// the time through `listening.listen`; what is typed on `input` is taken
/// too (a command, a message, or the reply to a question). It ends when the
/// input ends or the microphone stops, once the agent is done.
pub fn run(
    ledger: SharedLedger,
    profile: &Profile,
    talk: &Talk,
    listening: &Listening,
    input: Box<dyn BufRead + Send>,
    out: &mut (dyn Write + Send),
) -> Result<(), CliError> {
    let hearing = talk
        .hearing
        .clone()
        .ok_or_else(|| CliError::Usage("a live conversation is heard: give --hear".into()))?;
    let Some(program) = listening.listen.first() else {
        return Err(CliError::Usage(
            "nothing gives the microphone's sound: give --listen".into(),
        ));
    };
    if talk.speaking.is_some() && talk.play.is_empty() && listening.play_raw.is_none() {
        return Err(CliError::Usage(
            "nothing plays the replies: give --play".into(),
        ));
    }
    // With a voice through loudspeakers, its echo is taken out of what the
    // microphone hears.
    let far = listening
        .play_raw
        .as_ref()
        .filter(|_| talk.speaking.is_some())
        .map(|_| crate::sound::FarLine::new());
    let key = format!("terminal:{}:{}", profile.name, talk.conversation);
    let who = crate::runtime::local_person("talk");
    let (events, inbox) = channel::<Event>();

    // The microphone, read in a thread of its own.
    let mut microphone = std::process::Command::new(program)
        .args(&listening.listen[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CliError::Usage(format!("{program}: {e}")))?;
    let sound = microphone.stdout.take().expect("piped");
    let complaints = microphone
        .stderr
        .take()
        .map(|stderr| std::thread::spawn(move || last_line(stderr)));
    let mode = Arc::new(AtomicU8::new(LISTENING));
    {
        let (mode, events, detection) = (mode.clone(), events.clone(), listening.detection.clone());
        let canceller = far.clone().map(crate::sound::Canceller::new);
        std::thread::spawn(move || capture(sound, detection, &mode, &events, canceller));
    }
    // What is typed.
    {
        let events = events.clone();
        std::thread::spawn(move || {
            let mut input = input;
            let mut line = String::new();
            loop {
                line.clear();
                let event = match input.read_line(&mut line) {
                    Ok(0) | Err(_) => Event::InputEnded,
                    Ok(_) => Event::Typed(line.trim_end().to_string()),
                };
                let ended = matches!(event, Event::InputEnded);
                if events.send(event).is_err() || ended {
                    break;
                }
            }
        });
    }
    // What hears, an utterance at a time, in the order they were said.
    let (to_hear, heard) = channel::<(Vec<i16>, bool)>();
    {
        let (events, wake, rate) = (
            events.clone(),
            listening.wake.clone(),
            listening.detection.rate,
        );
        std::thread::spawn(move || {
            for (speech, asleep) in heard {
                let audio = Audio::new("audio/wav", wav(&speech, rate));
                if events
                    .send(Event::Heard(hear(&hearing, wake.as_ref(), &audio, asleep)))
                    .is_err()
                {
                    break;
                }
            }
        });
    }
    // What speaks.
    let voice = talk.speaking.clone().map(|speaking| {
        let raw = listening.play_raw.clone().zip(far.clone());
        Voice::start(speaking, talk.play.clone(), events.clone(), raw)
    });

    let mut state = State::new(
        listening.wake.as_ref().map(|w| w.awake_secs),
        listening.interrupt,
        voice.is_some(),
    )
    .holding(listening.hold);
    let mut question: Option<Sender<Option<String>>> = None;
    let mut complaints = complaints;
    let _ = writeln!(
        out,
        "{}",
        match &listening.wake {
            Some(wake) => format!(
                "(listening for \"{}\"; type if you like; Ctrl-D ends)",
                wake.phrases[0].join(" ")
            ),
            None if listening.interrupt => {
                "(listening: speak when you like, and over the reply to stop it; Ctrl-D ends)"
                    .into()
            }
            None => "(listening: speak when you like; Ctrl-D ends)".into(),
        }
    );
    let _ = out.flush();
    'conversation: while let Ok(event) = inbox.recv() {
        let event = match event {
            Event::Question(text, reply) => {
                question = Some(reply.clone());
                Event::Question(text, reply)
            }
            Event::MicrophoneStopped(_) => {
                Event::MicrophoneStopped(microphone_ended(&mut microphone, complaints.take()))
            }
            event => event,
        };
        for todo in state.on(event, Instant::now()) {
            match todo {
                Do::Print(text) => {
                    let _ = write!(out, "{text}");
                    let _ = out.flush();
                }
                Do::Say(text) => {
                    if let Some(voice) = &voice {
                        voice.say(&text);
                    }
                }
                Do::Hush => {
                    if let Some(voice) = &voice {
                        voice.hush();
                    }
                }
                Do::Stop => {
                    crate::channels::stop(&key);
                }
                Do::Run(text) => run_message(
                    ledger.clone(),
                    profile.clone(),
                    key.clone(),
                    text,
                    who.clone(),
                    events.clone(),
                ),
                Do::Command(text) => {
                    let (ledger, profile, key, events) =
                        (ledger.clone(), profile.clone(), key.clone(), events.clone());
                    std::thread::spawn(move || {
                        let answer = crate::channels::converse(ledger, &profile, &key, &text);
                        let _ = events.send(Event::Commanded(
                            answer.map(|a| a.reply).map_err(|e| e.to_string()),
                        ));
                    });
                }
                Do::Answer(reply) => {
                    if let Some(question) = question.take() {
                        let _ = question.send(reply);
                    }
                }
                Do::Hear(speech, asleep) => {
                    let _ = to_hear.send((speech, asleep));
                }
                Do::Listen(m) => mode.store(m, Ordering::SeqCst),
                Do::Wait(id, hold) => {
                    let events = events.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(hold);
                        let _ = events.send(Event::HoldEnded(id));
                    });
                }
                Do::End => break 'conversation,
            }
        }
    }
    let _ = microphone.kill();
    let _ = microphone.wait();
    if let Some(voice) = &voice {
        voice.hush();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(frames: usize, amplitude: f64) -> Vec<Vec<i16>> {
        (0..frames)
            .map(|f| {
                (0..320)
                    .map(|i| {
                        let t = (f * 320 + i) as f64 / 16_000.0;
                        (amplitude * (2.0 * std::f64::consts::PI * 300.0 * t).sin()) as i16
                    })
                    .collect()
            })
            .collect()
    }

    fn quiet(frames: usize) -> Vec<Vec<i16>> {
        (0..frames)
            .map(|f| {
                (0..320)
                    .map(|i| (((f * 31 + i * 17) % 61) as i16) - 30)
                    .collect()
            })
            .collect()
    }

    /// The sounds `frames` make, and the utterances' lengths in frames.
    fn detect(detector: &mut Detector, frames: &[Vec<i16>], over: bool) -> (usize, Vec<usize>) {
        let mut began = 0;
        let mut ended = Vec::new();
        for frame in frames {
            match detector.frame(frame, over) {
                Sound::Began => began += 1,
                Sound::Utterance(speech) => ended.push(speech.len() / 320),
                Sound::Nothing => {}
            }
        }
        (began, ended)
    }

    #[test]
    fn what_sounds_unfinished_is_told_from_what_is_done() {
        for done in [
            "Book a flight to Paris.",
            "What time is it?",
            "Turn it on.",
            "Thank you",
            "Stop!",
            "",
        ] {
            assert!(finished(done), "{done:?}");
        }
        for going_on in [
            "Book a flight to",
            "I want the,",
            "Send it to Ana and",
            "Because um...",
            "Put it in my",
            "Well, if",
            "The one with the —",
        ] {
            assert!(!finished(going_on), "{going_on:?}");
        }
    }

    #[test]
    fn what_sounds_unfinished_waits_for_the_rest_and_no_longer() {
        let now = Instant::now();
        let heard = |text: &str| {
            Event::Heard(Ok(Heard::Said {
                text: text.into(),
                woke: false,
            }))
        };
        let runs = |todo: &[Do]| -> Vec<String> {
            todo.iter()
                .filter_map(|d| match d {
                    Do::Run(text) => Some(text.clone()),
                    _ => None,
                })
                .collect()
        };
        let mut state = State::new(None, true, false).holding(Some(HOLD));
        // Unfinished: held, and a wait begun.
        state.on(Event::Utterance(vec![0; 320]), now);
        let todo = state.on(heard("Book a flight to"), now);
        assert!(runs(&todo).is_empty());
        assert!(todo.contains(&Do::Wait(1, HOLD)));
        // The rest comes: both, run as one.
        state.on(Event::Utterance(vec![0; 320]), now);
        let todo = state.on(heard("Paris, please."), now);
        assert_eq!(runs(&todo), ["Book a flight to Paris, please."]);
        // An old wait's end changes nothing.
        assert!(runs(&state.on(Event::HoldEnded(1), now)).is_empty());
        state.on(Event::RunEnded(Ok("Booked.".into())), now);

        // Unfinished, and nothing more: run when the wait ends.
        state.on(Event::Utterance(vec![0; 320]), now);
        state.on(heard("Send it to Ana and"), now);
        assert_eq!(
            runs(&state.on(Event::HoldEnded(2), now)),
            ["Send it to Ana and"]
        );
        state.on(Event::RunEnded(Ok("Sent.".into())), now);

        // A wait that ends while more is heard waits for it; noise adds
        // nothing, and lets it go.
        state.on(Event::Utterance(vec![0; 320]), now);
        state.on(heard("Put it in my"), now);
        state.on(Event::Utterance(vec![0; 320]), now);
        assert!(runs(&state.on(Event::HoldEnded(3), now)).is_empty());
        assert_eq!(
            runs(&state.on(Event::Heard(Ok(Heard::Nothing)), now)),
            ["Put it in my"]
        );

        // Without holding, what is heard is run at once.
        let mut plain = State::new(None, true, false);
        plain.on(Event::Utterance(vec![0; 320]), now);
        assert_eq!(
            runs(&plain.on(heard("Book a flight to"), now)),
            ["Book a flight to"]
        );
    }

    #[test]
    fn speech_is_told_from_silence_by_its_loudness() {
        let loudness = Detection {
            model: false,
            ..Detection::default()
        };
        let mut detector = Detector::new(loudness.clone());
        let mut sound = quiet(25);
        sound.extend(tone(50, 8000.0));
        sound.extend(quiet(60));
        // Ten frames before, the second of speech, ten after.
        assert_eq!(detect(&mut detector, &sound, false), (1, vec![70]));
        // A click is not speech; nor is speech too quiet to hear.
        let mut sound = tone(5, 8000.0);
        sound.extend(quiet(60));
        sound.extend(tone(50, 300.0));
        sound.extend(quiet(60));
        assert_eq!(detect(&mut detector, &sound, false).1, Vec::<usize>::new());
        // Over the agent, speech must be louder, and longer.
        let mut sound = tone(50, 700.0);
        sound.extend(quiet(60));
        assert_eq!(detect(&mut detector, &sound, true), (0, vec![]));
        let mut sound = tone(11, 8000.0);
        sound.extend(quiet(60));
        assert_eq!(detect(&mut detector, &sound, true).0, 0);
        let mut sound = tone(30, 8000.0);
        sound.extend(quiet(60));
        assert_eq!(detect(&mut detector, &sound, true), (1, vec![50]));
        // An utterance is cut at its longest; what the sound ends on is
        // kept.
        let mut detector = Detector::new(Detection {
            most: 100,
            ..loudness
        });
        assert_eq!(
            detect(&mut detector, &tone(250, 8000.0), false).1,
            vec![100, 100]
        );
        assert_eq!(detector.flush().map(|s| s.len() / 320), Some(50));
        assert_eq!(detector.flush(), None);
    }

    #[test]
    fn raw_samples_are_played_at_their_pace_and_told_to_the_echo_line() {
        let dir = std::env::temp_dir().join(format!("sion-raw-play-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let heard = dir.join("heard.raw");
        let play = vec![
            "sh".to_string(),
            "-c".into(),
            format!("cat >> {}", heard.display()),
        ];
        let playing = Playing {
            epoch: AtomicU64::new(0),
            child: Mutex::new(None),
            input: Mutex::new(None),
        };
        let far = crate::sound::FarLine::new();
        // Half a second, played in about half a second.
        let samples: Vec<i16> = (0..8_000).map(|i| (i % 400) as i16).collect();
        let started = Instant::now();
        playing.play_raw(&play, &samples, 0, &far).unwrap();
        let took = started.elapsed();
        assert!(took >= Duration::from_millis(400), "{took:?}");
        // The player runs on for the next sentence; what it was handed is
        // what it heard.
        let child = playing.child.lock().unwrap().take();
        drop(playing.input.lock().unwrap().take());
        let _ = child.map(|mut c| c.wait());
        let bytes = std::fs::read(&heard).unwrap();
        assert_eq!(crate::sound::samples_of(&bytes), samples);
        // Silenced: it stops at once.
        let started = Instant::now();
        let long: Vec<i16> = vec![100; 16_000 * 5];
        std::thread::scope(|scope| {
            scope.spawn(|| playing.play_raw(&play, &long, 0, &far));
            std::thread::sleep(Duration::from_millis(300));
            playing.epoch.fetch_add(1, Ordering::SeqCst);
        });
        assert!(started.elapsed() < Duration::from_secs(2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_model_takes_speech_and_leaves_noise_that_is_as_loud() {
        // Random hiss, faint or loud.
        let mut state = 99u64;
        let mut hiss = |frames: usize, amplitude: i32| -> Vec<Vec<i16>> {
            (0..frames)
                .map(|_| {
                    (0..320)
                        .map(|_| {
                            state = state
                                .wrapping_mul(6364136223846793005)
                                .wrapping_add(1442695040888963407);
                            ((state >> 40) as i32 % (2 * amplitude) - amplitude) as i16
                        })
                        .collect()
                })
                .collect()
        };
        // "Can you hear me now?", said by espeak-ng, in a quiet room; then
        // a burst of hiss as loud, which loudness alone takes for speech.
        let said = crate::sound::samples_of(include_bytes!("../tests/fixtures/spoken-16k.raw"));
        let mut sound = hiss(25, 30);
        sound.extend(said.chunks_exact(320).map(<[i16]>::to_vec));
        sound.extend(hiss(60, 30));
        sound.extend(hiss(50, 3000));
        sound.extend(hiss(60, 30));
        let (_, heard) = detect(&mut Detector::new(Detection::default()), &sound, false);
        assert_eq!(heard.len(), 1, "{heard:?}");
        let loudness = Detection {
            model: false,
            ..Detection::default()
        };
        let (_, loud) = detect(&mut Detector::new(loudness), &sound, false);
        assert_eq!(loud.len(), 2, "{loud:?}");
        assert!(Listening::from_flags(
            None,
            Some("cat"),
            None,
            None,
            false,
            15,
            true,
            400.0,
            "both",
            None
        )
        .is_err());
    }

    #[test]
    fn a_wake_word_is_heard_on_this_machine_unless_the_person_says_otherwise() {
        let cloud = Hearing::parse("openai:whisper-1").unwrap();
        let listen = Some("cat /dev/zero");
        let made = Listening::from_flags(
            Some(&cloud),
            listen,
            None,
            None,
            false,
            15,
            true,
            250.0,
            "model",
            None,
        )
        .unwrap();
        assert_eq!(made.listen, vec!["cat", "/dev/zero"]);
        assert_eq!(
            (made.wake, made.interrupt, made.detection.level),
            (None, true, 250.0)
        );
        assert!(Listening::from_flags(
            Some(&cloud),
            listen,
            None,
            None,
            false,
            15,
            true,
            0.0,
            "model",
            None
        )
        .is_err());
        let refused = Listening::from_flags(
            Some(&cloud),
            listen,
            Some("hey sion"),
            None,
            false,
            15,
            true,
            400.0,
            "model",
            None,
        )
        .unwrap_err();
        assert!(
            refused.contains("--wake-hear whisper:")
                && refused.contains("command:<program>")
                && refused.contains("api.openai.com"),
            "{refused}"
        );
        let here = Listening::from_flags(
            Some(&cloud),
            listen,
            Some("hey sion"),
            Some("command:whisper-cli -m tiny.bin"),
            false,
            20,
            false,
            400.0,
            "model",
            None,
        )
        .unwrap();
        let wake = here.wake.unwrap();
        assert_eq!(
            wake.hearing,
            Hearing::Command(vec!["whisper-cli".into(), "-m".into(), "tiny.bin".into()])
        );
        assert_eq!(wake.awake_secs, 20);
        assert!(Listening::from_flags(
            Some(&cloud),
            listen,
            Some("hey sion"),
            None,
            true,
            15,
            true,
            400.0,
            "model",
            None,
        )
        .unwrap()
        .wake
        .is_some_and(|w| w.hearing == cloud));
    }

    #[test]
    fn what_is_heard_goes_as_a_wav_file() {
        let file = wav(&[0, 1, -1, i16::MAX], 16_000);
        assert_eq!(file.len(), 44 + 8);
        assert_eq!(&file[..4], b"RIFF");
        assert_eq!(&file[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(file[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u32::from_le_bytes(file[40..44].try_into().unwrap()), 8);
        assert_eq!(&file[50..], &i16::MAX.to_le_bytes());
        assert_eq!(Audio::sniff(&file), "audio/wav");
    }

    #[test]
    fn a_wake_phrase_is_found_at_the_start_of_what_is_said() {
        let wake = Wake::parse(
            "hey sion|ok sion",
            Hearing::Command(vec!["true".into()]),
            15,
        )
        .unwrap();
        assert_eq!(wake.phrases, vec![vec!["hey", "sion"], vec!["ok", "sion"]]);
        assert_eq!(
            wake.strip("Hey, Sion! What's on today?").as_deref(),
            Some("What's on today?")
        );
        assert_eq!(
            wake.strip("Okay, hey Zion — what's on").as_deref(),
            Some("what's on")
        );
        assert_eq!(wake.strip("hey scion").as_deref(), Some(""));
        assert_eq!(wake.strip("Heysion, stop.").as_deref(), Some("stop."));
        assert_eq!(
            wake.strip("OK Sion, lights off").as_deref(),
            Some("lights off")
        );
        assert_eq!(wake.strip("what a lovely day"), None);
        assert_eq!(wake.strip("I told him that hey sion works"), None);
        assert_eq!(wake.strip("hey"), None);
        assert_eq!(without_words("Hazy on, the weather?", 2), "the weather?");
        assert!(Wake::parse(" | ", Hearing::Command(vec!["true".into()]), 15).is_err());
        assert!(nothing_said(" [BLANK_AUDIO] "));
        assert!(nothing_said("(silence)"));
        assert!(!nothing_said("(laughs) yes"));
    }

    #[test]
    fn a_wake_word_is_heard_here_before_the_conversation_hears_more() {
        let dir = std::env::temp_dir().join(format!("sion-wake-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let said = |name: &str, text: &str| {
            let log = dir.join(format!("{name}.log"));
            Hearing::Command(vec![
                "sh".into(),
                "-c".into(),
                format!("echo heard >> {}; echo \"{text}\"", log.display()),
            ])
        };
        let count = |name: &str| {
            std::fs::read_to_string(dir.join(format!("{name}.log")))
                .map(|t| t.lines().count())
                .unwrap_or(0)
        };
        let audio = Audio::new("audio/wav", wav(&[0; 320], 16_000));
        let here = said("here", "what a lovely day");
        let there = said("there", "Hey Zion, the weather?");
        let wake = Wake::parse("hey sion", here.clone(), 15).unwrap();
        // Asleep: heard here only, and not woken.
        assert_eq!(
            hear(&there, Some(&wake), &audio, true).unwrap(),
            Heard::NotWoken("what a lovely day".into())
        );
        assert_eq!((count("here"), count("there")), (1, 0));
        // Woken: heard again by the conversation's transcriber.
        let woken = Wake {
            hearing: said("here", "hey sion the weather"),
            ..wake.clone()
        };
        assert_eq!(
            hear(&there, Some(&woken), &audio, true).unwrap(),
            Heard::Said {
                text: "the weather?".into(),
                woke: true
            }
        );
        assert_eq!((count("here"), count("there")), (2, 1));
        // Awake: heard by the conversation's alone.
        assert_eq!(
            hear(&said("there", "And tomorrow?"), Some(&wake), &audio, false).unwrap(),
            Heard::Said {
                text: "And tomorrow?".into(),
                woke: false
            }
        );
        assert_eq!((count("here"), count("there")), (2, 2));
        assert_eq!(
            hear(&said("there", "[BLANK_AUDIO]"), None, &audio, false).unwrap(),
            Heard::Nothing
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn replies_are_said_a_sentence_at_a_time_as_they_are_written() {
        let mut sentences = Sentences::default();
        assert!(sentences.push("Three meet").is_empty());
        assert_eq!(
            sentences.push("ings today. The"),
            vec!["Three meetings today."]
        );
        assert!(sentences.push(" first, e.g. at 9.5,").is_empty());
        assert_eq!(
            sentences.push(" is Dr. Ana's. Next"),
            vec!["The first, e.g. at 9.5, is Dr. Ana's."]
        );
        assert_eq!(sentences.finish().as_deref(), Some("Next"));
        // Code is shown, not read out; links by their words.
        let mut sentences = Sentences::default();
        let mut said = Vec::new();
        for word in "Run **this**:\n``".split_inclusive(' ') {
            said.extend(sentences.push(word));
        }
        said.extend(sentences.push("`sh\nls -la. Not this.\n```\nSee [the docs](https://x.example/d) or https://a.example/b now."));
        said.extend(sentences.finish());
        assert_eq!(
            said,
            vec![
                "Run this:",
                CODE,
                "See the docs or the link on the screen now."
            ]
        );
        // A long stretch with no end is said in pieces; marks alone are not.
        let mut sentences = Sentences::default();
        let long = "word ".repeat(80);
        let said = sentences.push(&long);
        assert_eq!(said.len(), 1);
        assert!(said[0].len() <= LONGEST);
        assert_eq!(speakable("---"), None);
        assert_eq!(
            speakable("- **Bold** `code` | cell |").as_deref(),
            Some("Bold code cell")
        );
        let mut sentences = Sentences::default();
        assert_eq!(sentences.push("你好。再见"), vec!["你好。"]);
    }

    fn sender() -> Sender<Option<String>> {
        channel().0
    }

    fn printed(todo: &[Do]) -> String {
        todo.iter()
            .filter_map(|d| match d {
                Do::Print(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn speaking_over_the_agent_silences_and_stops_it() {
        let now = Instant::now();
        let mut state = State::new(None, true, true);
        let todo = state.on(Event::Utterance(vec![1; 320]), now);
        assert_eq!(todo, vec![Do::Hear(vec![1; 320], false)]);
        let todo = state.on(
            Event::Heard(Ok(Heard::Said {
                text: "What is on today?".into(),
                woke: false,
            })),
            now,
        );
        assert_eq!(
            todo[0],
            Do::Print("you (heard): What is on today?\n".into())
        );
        assert_eq!(todo[1], Do::Run("What is on today?".into()));
        assert_eq!(todo[2], Do::Listen(OVER));
        let todo = state.on(Event::Delta("Three meetings. The first".into()), now);
        assert_eq!(
            todo,
            vec![
                Do::Print("agent> Three meetings. The first".into()),
                Do::Say("Three meetings.".into())
            ]
        );
        // Spoken over: silent at once, the run stopped, and listened to as
        // usual while it stops.
        let todo = state.on(Event::Began, now);
        assert_eq!(
            todo,
            vec![
                Do::Hush,
                Do::Stop,
                Do::Print("\n(interrupted)\n".into()),
                Do::Listen(LISTENING)
            ]
        );
        // What it was still writing is not shown or said; what it said
        // before it was silenced is not counted.
        assert!(state
            .on(Event::Delta(" is at nine. ".into()), now)
            .is_empty());
        assert!(state.on(Event::Played(0), now).is_empty());
        state.on(Event::Utterance(vec![2; 320]), now);
        let todo = state.on(
            Event::Heard(Ok(Heard::Said {
                text: "Move the noon one.".into(),
                woke: false,
            })),
            now,
        );
        assert_eq!(
            todo,
            vec![Do::Print("you (heard): Move the noon one.\n".into())]
        );
        // It goes next, once the run has stopped.
        let todo = state.on(
            Event::RunEnded(Ok("Three meetings. The first is at nine.".into())),
            now,
        );
        assert_eq!(
            todo,
            vec![Do::Run("Move the noon one.".into()), Do::Listen(OVER)]
        );
        state.on(Event::Delta("Moved.".into()), now);
        let todo = state.on(Event::RunEnded(Ok("Moved.".into())), now);
        assert_eq!(todo, vec![Do::Print("\n".into()), Do::Say("Moved.".into())]);
        // Spoken: listened to as usual; the input's end ends it.
        assert_eq!(state.on(Event::Played(1), now), vec![Do::Listen(LISTENING)]);
        assert_eq!(state.on(Event::InputEnded, now), vec![Do::End]);
    }

    #[test]
    fn typed_lines_questions_and_commands_have_their_place() {
        let now = Instant::now();
        let mut state = State::new(None, false, true);
        let todo = state.on(Event::Typed("Greet someone.".into()), now);
        assert_eq!(
            todo,
            vec![Do::Run("Greet someone.".into()), Do::Listen(DEAF)]
        );
        // Not to be interrupted: speaking over it does nothing.
        assert!(state.on(Event::Began, now).is_empty());
        // A command typed meanwhile runs alongside; /stop stops it.
        assert_eq!(
            state.on(Event::Typed("/status".into()), now),
            vec![Do::Command("/status".into())]
        );
        // A question is asked aloud, and listened to for its reply once
        // asked.
        let todo = state.on(
            Event::Question("Who should be greeted?".into(), sender()),
            now,
        );
        assert_eq!(
            todo,
            vec![
                Do::Print("Who should be greeted?\n".into()),
                Do::Say("Who should be greeted?".into())
            ]
        );
        assert_eq!(state.on(Event::Played(0), now), vec![Do::Listen(LISTENING)]);
        state.on(Event::Utterance(vec![1; 320]), now);
        let todo = state.on(
            Event::Heard(Ok(Heard::Said {
                text: "Ana".into(),
                woke: false,
            })),
            now,
        );
        assert_eq!(
            todo,
            vec![
                Do::Print("you (heard): Ana\n".into()),
                Do::Answer(Some("Ana".into())),
                Do::Listen(DEAF)
            ]
        );
        let todo = state.on(Event::Typed("/stop".into()), now);
        assert_eq!(
            todo,
            vec![
                Do::Stop,
                Do::Print("(interrupted)\n".into()),
                Do::Listen(LISTENING)
            ]
        );
        // The microphone stopped: it ends once the run has.
        let todo = state.on(
            Event::MicrophoneStopped("exit status: 1: no device".into()),
            now,
        );
        assert_eq!(
            printed(&todo),
            "(the microphone stopped: exit status: 1: no device)\n"
        );
        let todo = state.on(Event::RunEnded(Err("the model failed".into())), now);
        assert_eq!(
            todo,
            vec![
                Do::Print("(that did not work: the model failed)\n".into()),
                Do::End
            ]
        );
    }

    #[test]
    fn with_a_wake_word_a_question_is_answered_while_it_is_awake() {
        let start = Instant::now();
        let mut state = State::new(Some(15), true, true);
        state.on(Event::Typed("Greet someone.".into()), start);
        state.on(Event::Question("Who?".into(), sender()), start);
        // While the question is said, what is heard needs the wake word.
        assert_eq!(
            state.on(Event::Utterance(vec![1; 320]), start),
            vec![Do::Hear(vec![1; 320], true)]
        );
        state.on(Event::Heard(Ok(Heard::Nothing)), start);
        // Once it is said, the answer does not, for a while.
        state.on(Event::Played(0), start);
        let soon = start + Duration::from_secs(5);
        assert_eq!(
            state.on(Event::Utterance(vec![1; 320]), soon),
            vec![Do::Hear(vec![1; 320], false)]
        );
        let later = start + Duration::from_secs(20);
        assert_eq!(
            state.on(Event::Utterance(vec![1; 320]), later),
            vec![Do::Hear(vec![1; 320], true)]
        );
    }

    #[test]
    fn with_a_wake_word_it_hears_only_its_name_and_is_stopped_by_it() {
        let start = Instant::now();
        let mut state = State::new(Some(15), true, true);
        // Asleep: heard for the wake word only.
        assert_eq!(
            state.on(Event::Utterance(vec![1; 320]), start),
            vec![Do::Hear(vec![1; 320], true)]
        );
        let todo = state.on(
            Event::Heard(Ok(Heard::NotWoken("lovely day".into()))),
            start,
        );
        assert_eq!(printed(&todo), "(not woken: heard \"lovely day\")\n");
        state.on(Event::Utterance(vec![1; 320]), start);
        let todo = state.on(
            Event::Heard(Ok(Heard::Said {
                text: "the weather?".into(),
                woke: true,
            })),
            start,
        );
        assert_eq!(todo[1], Do::Run("the weather?".into()));
        state.on(Event::Delta("Sunny. All day.".into()), start);
        // Over its reply, speech begins: with a wake word, only its name
        // stops it.
        assert!(state.on(Event::Began, start).is_empty());
        assert_eq!(
            state.on(Event::Utterance(vec![1; 320]), start),
            vec![Do::Hear(vec![1; 320], true)]
        );
        let todo = state.on(
            Event::Heard(Ok(Heard::Said {
                text: String::new(),
                woke: true,
            })),
            start,
        );
        assert_eq!(
            todo,
            vec![
                Do::Hush,
                Do::Stop,
                Do::Print("\n(interrupted)\n".into()),
                Do::Print("(awake: listening)\n".into()),
                Do::Listen(LISTENING)
            ]
        );
        state.on(Event::RunEnded(Ok("Sunny. All day.".into())), start);
        // Awake for a follow-up, then asleep again.
        let soon = start + Duration::from_secs(10);
        assert_eq!(
            state.on(Event::Utterance(vec![1; 320]), soon),
            vec![Do::Hear(vec![1; 320], false)]
        );
        let later = start + Duration::from_secs(16);
        assert_eq!(
            state.on(Event::Utterance(vec![1; 320]), later),
            vec![Do::Hear(vec![1; 320], true)]
        );
    }
}
