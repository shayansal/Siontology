//! Joining a meeting by its link, as a guest that listens
//! (`sion agent meeting <profile> --join <link>`): Chromium opens the link
//! with a microphone and a camera of its own that give silence and
//! nothing, types the guest's name where the page asks for one, turns its
//! microphone and camera off, and asks to join (Google Meet's "Ask to
//! join" or "Join now", Zoom's "Join from your browser" and "Join", Teams'
//! "Continue on this browser" and "Join now"). What the meeting plays goes
//! to a PulseAudio (or PipeWire) sink of its own, whose monitor is heard
//! and transcribed as a meeting heard here is. It leaves when the meeting
//! ends for it, or when the notes are asked for.
//!
//! The steps are found by what the page's buttons and fields say, as a
//! person would find them, not by the pages' code: a page that says them
//! otherwise, or asks to sign in first, is not joined, and the reason the
//! page gives is said.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sion_agent::cdp::{Cdp, PIPE_SHELL};

/// What a page is asked, once a second, until it is joined: one step of
/// joining, given the guest's name. It answers what it did and where it
/// stands: `joined`, `waiting` (to be let in), `refused` (with the page's
/// words) or `trying`.
pub const JOIN_STEP: &str = r#"(name) => {
  // What a reader of the screen would call it: its label first.
  const said = (el) => ((el.getAttribute && el.getAttribute('aria-label')) || el.innerText || el.value
    || el.title || (el.getAttribute && el.getAttribute('placeholder')) || '').trim();
  const shown = (el) => { const r = el.getBoundingClientRect(); const s = getComputedStyle(el);
    return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none' && !el.disabled; };
  const all = (selector) => Array.from(document.querySelectorAll(selector)).filter(shown);
  const buttons = all('button, [role="button"], a[role="button"], input[type="submit"], input[type="button"]');
  const text = (document.body ? document.body.innerText : '').slice(0, 20000);
  const did = [];
  if (buttons.some(b => /^(leave( call| meeting)?|hang up)$/i.test(said(b)))) {
    return {state: 'joined', did};
  }
  const refused = text.match(/(you can'?t join this (call|meeting)|you have been removed|your request to join was (denied|declined)|this meeting has ended|meeting (id )?(is )?invalid|sign in to join)/i);
  if (refused) { return {state: 'refused', why: refused[0], did}; }
  const waiting = /(waiting for (the )?host|someone (in the meeting )?will let you in|asking to (be let in|join)|you'?ll join when someone lets you in|waiting to be admitted|in the lobby)/i.test(text);
  for (const field of all('input:not([type]), input[type="text"]')) {
    const labels = Array.from(field.labels || []).map(l => l.innerText).join(' ');
    if (!field.value && /name/i.test([said(field), field.placeholder, labels, field.id, field.name].join(' '))) {
      const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      field.focus(); set.call(field, name);
      field.dispatchEvent(new Event('input', {bubbles: true}));
      field.dispatchEvent(new Event('change', {bubbles: true}));
      did.push('named');
    }
  }
  for (const b of buttons) {
    if (/^turn off (the )?(microphone|camera|video|mic)/i.test(said(b))) { b.click(); did.push('off: ' + said(b)); }
  }
  if (!waiting) {
    const order = [/^got it$|^dismiss$|^close$/i, /^continue on this browser$/i, /^join from (your|this) browser$/i,
      /^continue without (audio|video|microphone|camera)/i, /^ask to join$/i, /^join now$/i, /^join( meeting)?$/i];
    for (const wanted of order) {
      const b = buttons.find(b => wanted.test(said(b)));
      if (b) { b.click(); did.push('clicked: ' + said(b)); break; }
    }
  }
  return {state: waiting ? 'waiting' : 'trying', did};
}"#;

/// What a page is asked to leave the meeting.
pub const LEAVE: &str = r#"(() => {
  const said = (el) => (el.getAttribute('aria-label') || el.innerText || el.title || '').trim();
  const b = Array.from(document.querySelectorAll('button, [role="button"]')).find(b => /^(leave( call| meeting)?|hang up)$/i.test(said(b)));
  if (b) { b.click(); return true; }
  return false;
})()"#;

/// How long joining is tried, a guest let in included.
pub const JOIN_SECS: u64 = 300;

/// Where a page stands in joining.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Standing {
    /// In the meeting.
    Joined,
    /// Waiting to be let in.
    Waiting,
    /// Refused, with what the page said.
    Refused(String),
    /// Neither yet.
    Trying,
}

/// A guest in a meeting: its browser.
pub struct Bot {
    child: Child,
    cdp: Cdp,
    session: String,
    profile: PathBuf,
}

impl std::fmt::Debug for Bot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Bot({})", self.profile.display())
    }
}

impl Drop for Bot {
    /// Closes the browser, however the guest ends, and removes its profile.
    fn drop(&mut self) {
        let _ = self
            .cdp
            .send_waiting(None, "Browser.close", json!({}), Duration::from_secs(2));
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.profile);
    }
}

/// A WAV of a second of silence: what the guest's microphone says.
fn silence(dir: &std::path::Path) -> Result<PathBuf, String> {
    let path = dir.join("silence.wav");
    std::fs::write(&path, crate::listening::wav(&[0; 16_000], 16_000))
        .map_err(|e| e.to_string())?;
    Ok(path)
}

/// A picture of black, as a one-frame Y4M video: what the guest's camera
/// shows where the page does not let it be turned off.
fn black(dir: &std::path::Path) -> Result<PathBuf, String> {
    let (width, height) = (640usize, 360usize);
    let mut video =
        format!("YUV4MPEG2 W{width} H{height} F1:1 Ip A1:1 C420jpeg\nFRAME\n").into_bytes();
    // No brightness, and no color.
    video.resize(video.len() + width * height, 0);
    video.resize(video.len() + width * height / 2, 128);
    let path = dir.join("black.y4m");
    std::fs::write(&path, video).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Whether this process runs as root.
fn running_as_root() -> bool {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata("/proc/self").is_ok_and(|m| m.uid() == 0)
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

impl Bot {
    /// Starts Chromium (`program`) for a guest, its sound to the PulseAudio
    /// sink `sink` if given, and opens `link` in it. Chromium keeps its own
    /// sandbox (`own_sandbox`, refused as root, where it cannot start one);
    /// only tests turn it off.
    pub fn open(
        program: &std::path::Path,
        link: &str,
        sink: Option<&str>,
        own_sandbox: bool,
    ) -> Result<Bot, String> {
        if !(link.starts_with("https://") || link.starts_with("http://")) {
            return Err(format!("{link:?} is not a meeting's link (https://…)"));
        }
        if own_sandbox && running_as_root() {
            return Err(
                "Chromium does not start its sandbox as root: join as an ordinary user".into(),
            );
        }
        let mut name = [0u8; 8];
        sion_http::random(&mut name).map_err(|e| e.to_string())?;
        let profile = std::env::temp_dir().join(format!(
            "sion-meeting-{}",
            name.iter().map(|b| format!("{b:02x}")).collect::<String>()
        ));
        std::fs::create_dir(&profile).map_err(|e| format!("{}: {e}", profile.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&profile, std::fs::Permissions::from_mode(0o700));
        }
        // However the start fails, the browser and its profile go.
        Bot::start(program, link, sink, own_sandbox, &profile).inspect_err(|_| {
            let _ = std::fs::remove_dir_all(&profile);
        })
    }

    fn start(
        program: &std::path::Path,
        link: &str,
        sink: Option<&str>,
        own_sandbox: bool,
        profile: &std::path::Path,
    ) -> Result<Bot, String> {
        let quiet = silence(profile)?;
        let dark = black(profile)?;
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(PIPE_SHELL)
            .arg(program)
            .args([
                "--headless=new",
                "--remote-debugging-pipe",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-sync",
                "--disable-extensions",
                "--password-store=basic",
                "--use-mock-keychain",
                "--use-fake-ui-for-media-stream",
                "--use-fake-device-for-media-stream",
                "--autoplay-policy=no-user-gesture-required",
                "--window-size=1280,800",
            ])
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg(format!(
                "--use-file-for-fake-audio-capture={}",
                quiet.display()
            ))
            .arg(format!(
                "--use-file-for-fake-video-capture={}",
                dark.display()
            ))
            .arg("about:blank")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(sink) = sink {
            command.env("PULSE_SINK", sink);
        }
        if !own_sandbox {
            command.arg("--no-sandbox");
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("{}: {e}", program.display()))?;
        let (Some(writer), Some(reader)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("no pipe to the browser".into());
        };
        // No one is there to answer a page's dialogs: they are dismissed.
        let cdp = Cdp::new(writer, reader, Duration::from_secs(30));
        let mut bot = Bot {
            child,
            cdp,
            session: String::new(),
            profile: profile.to_path_buf(),
        };
        // From here, a failure drops the bot, which closes the browser.
        bot.session = bot.attach(link)?;
        Ok(bot)
    }

    /// Opens a page of the browser's own, and `link` in it: its session.
    fn attach(&self, link: &str) -> Result<String, String> {
        let cdp = &self.cdp;
        // Once it answers, a page of its own: the one it opened with may
        // not be listed yet.
        cdp.send_waiting(
            None,
            "Browser.getVersion",
            json!({}),
            Duration::from_secs(30),
        )
        .map_err(|e| format!("the browser did not start: {e}"))?;
        let target = cdp.send(None, "Target.createTarget", json!({"url": "about:blank"}))?;
        let page = target["targetId"]
            .as_str()
            .ok_or("the browser opened no page")?
            .to_string();
        let attached = cdp.send(
            None,
            "Target.attachToTarget",
            json!({"targetId": page, "flatten": true}),
        )?;
        let session = attached["sessionId"]
            .as_str()
            .ok_or("the browser's page could not be reached")?
            .to_string();
        cdp.send(Some(&session), "Page.enable", json!({}))?;
        // Its camera and microphone are allowed, as a person would allow
        // them; they show nothing and say nothing.
        let _ = cdp.send(
            None,
            "Browser.grantPermissions",
            json!({"permissions": ["audioCapture", "videoCapture"]}),
        );
        cdp.send(Some(&session), "Page.navigate", json!({"url": link}))?;
        Ok(session)
    }

    fn evaluate(&self, expression: &str) -> Result<Value, String> {
        let answer = self.cdp.send(
            Some(&self.session),
            "Runtime.evaluate",
            json!({"expression": expression, "returnByValue": true, "awaitPromise": true}),
        )?;
        if let Some(thrown) = answer.get("exceptionDetails") {
            return Err(format!(
                "the page's script failed: {}",
                thrown["exception"]["description"]
                    .as_str()
                    .or(thrown["text"].as_str())
                    .unwrap_or("no reason given")
            ));
        }
        Ok(answer["result"]["value"].clone())
    }

    /// One step of joining, as the guest `name`.
    pub fn step(&self, name: &str) -> Result<(Standing, Vec<String>), String> {
        let answer = self.evaluate(&format!("({JOIN_STEP})({})", json!(name)))?;
        let did = answer["did"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|d| d.as_str().map(str::to_string))
            .collect();
        let standing = match answer["state"].as_str() {
            Some("joined") => Standing::Joined,
            Some("waiting") => Standing::Waiting,
            Some("refused") => Standing::Refused(
                answer["why"]
                    .as_str()
                    .unwrap_or("the page refused")
                    .to_string(),
            ),
            _ => Standing::Trying,
        };
        Ok((standing, did))
    }

    /// Joins as `name`, a step a second, telling `said` what it does, for at
    /// most `within`.
    pub fn join(
        &self,
        name: &str,
        within: Duration,
        said: &mut dyn FnMut(&str),
    ) -> Result<(), String> {
        let deadline = Instant::now() + within;
        let mut waiting = false;
        while Instant::now() < deadline {
            // A page still loading has no script to run yet.
            match self.step(name) {
                Ok((Standing::Joined, _)) => return Ok(()),
                Ok((Standing::Refused(why), _)) => return Err(format!("not let in: {why}")),
                Ok((standing, did)) => {
                    for done in did {
                        said(&done);
                    }
                    if standing == Standing::Waiting && !waiting {
                        said("waiting to be let in");
                        waiting = true;
                    }
                }
                Err(_) => {}
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        Err(format!(
            "not in the meeting after {} seconds: it may ask to sign in, or no one let the guest in",
            within.as_secs()
        ))
    }

    /// Whether it is still in the meeting.
    pub fn in_meeting(&self, name: &str) -> bool {
        matches!(self.step(name), Ok((Standing::Joined, _)))
    }

    /// Leaves the meeting, and closes its browser.
    pub fn leave(self) {
        if !self.session.is_empty() {
            let _ = self.evaluate(LEAVE);
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}

/// A PulseAudio (or PipeWire) sink of the guest's own, made with `pactl`:
/// what the meeting plays goes there, and its monitor is heard.
#[derive(Debug)]
pub struct Sink {
    /// Its name.
    pub name: String,
    module: String,
}

/// What the sinks' names begin with, the process that made each after it.
const SINK_PREFIX: &str = "sion-meeting-";

/// The PulseAudio modules `pactl list short modules` lists in `listed`
/// that are sinks a process made which no longer runs (one stopped
/// before it could remove its sink): their numbers.
fn left_behind(listed: &str, runs: impl Fn(u32) -> bool) -> Vec<String> {
    listed
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let module = fields.next()?.trim();
            if fields.next()?.trim() != "module-null-sink" {
                return None;
            }
            let arguments = fields.next().unwrap_or_default();
            let pid = arguments
                .split_whitespace()
                .find_map(|a| a.strip_prefix("sink_name="))?
                .strip_prefix(SINK_PREFIX)?
                .parse::<u32>()
                .ok()?;
            (!runs(pid)).then(|| module.to_string())
        })
        .collect()
}

impl Sink {
    /// Makes one, having removed those that guests before it, stopped
    /// before they could, left behind.
    pub fn make() -> Result<Sink, String> {
        if let Ok(listed) = Command::new("pactl")
            .args(["list", "short", "modules"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
        {
            let listed = String::from_utf8_lossy(&listed.stdout);
            for module in left_behind(&listed, sion_agent::browser::process_runs) {
                let _ = Command::new("pactl")
                    .args(["unload-module", &module])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
        let name = format!("{SINK_PREFIX}{}", std::process::id());
        let output = Command::new("pactl")
            .args(["load-module", "module-null-sink"])
            .arg(format!("sink_name={name}"))
            .arg("sink_properties=device.description=sion-meeting")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| {
                format!(
                    "hearing a meeting joined by its link needs PulseAudio or PipeWire (pactl): {e}"
                )
            })?;
        if !output.status.success() {
            return Err(format!(
                "pactl could not make a sink: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(Sink {
            name,
            module: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        })
    }

    /// The program that gives what it plays: raw 16-bit mono at 16 kHz.
    pub fn monitor(&self) -> Vec<String> {
        vec![
            "parec".into(),
            "-d".into(),
            format!("{}.monitor", self.name),
            "--format=s16le".into(),
            "--rate=16000".into(),
            "--channels=1".into(),
        ]
    }
}

impl Drop for Sink {
    fn drop(&mut self) {
        let _ = Command::new("pactl")
            .args(["unload-module", &self.module])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sinks_of_guests_that_stopped_are_found_and_no_others() {
        let listed = "536870912\tmodule-null-sink\tsink_name=sion-meeting-41 sink_properties=device.description=sion-meeting\t\n\
                      536870913\tmodule-null-sink\tsink_name=sion-meeting-42 sink_properties=device.description=sion-meeting\t\n\
                      536870914\tmodule-null-sink\tsink_name=music\t\n\
                      536870915\tmodule-native-protocol-unix\t\t\n";
        assert_eq!(left_behind(listed, |pid| pid == 42), ["536870912"]);
        assert!(left_behind("", |_| false).is_empty());
    }
}
