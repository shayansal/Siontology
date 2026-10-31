//! IRC, and Twitch's chat (IRC over TLS): a connection kept by a thread of
//! its own ([`crate::live`]), joined to the channels the bridge answers.
//! What is said there, and privately to the bot, is passed on; replies go
//! back as `PRIVMSG`, a line at a time and no faster than servers allow,
//! long ones cut to a few lines.
//!
//! | `--via` | Credentials | Server |
//! |---|---|---|
//! | `irc` | `IRC_NICK`, and `IRC_PASSWORD` for a SASL login (as `IRC_ACCOUNT`, the nick by default) | `IRC_SERVER` (`ircs://host:6697`) |
//! | `twitch` | `TWITCH_NICK`, `TWITCH_TOKEN` (OAuth, with chat scopes) | `TWITCH_IRC_SERVER` (`ircs://irc.chat.twitch.tv:6697`) |

use std::io::{Read, Write};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::live::{again, keep, Fault, Link, Live, Next};
use crate::messengers::{Incoming, Messenger, LIVE_ONCE_WAIT};

/// The longest line of text sent, in bytes, before a line is cut (IRC's
/// lines are 512 bytes, the sender's prefix included).
const LINE: usize = 400;

/// The most lines one reply takes.
const MOST_LINES: usize = 8;

/// How long the server may stay silent before the connection is taken
/// for lost.
const SILENCE: Duration = Duration::from_secs(300);

/// How the bot logs in and where.
#[derive(Clone, Debug)]
struct Config {
    server: String,
    nick: String,
    /// A SASL PLAIN login: the account and its password.
    sasl: Option<(String, String)>,
    /// Twitch's OAuth token, sent as the server password.
    pass: Option<String>,
    channels: Vec<String>,
    /// Lines a second, at most, once logged in.
    pause: Duration,
}

/// An IRC or Twitch bot.
pub struct Irc {
    name: &'static str,
    nick: String,
    live: Live,
    out: Mutex<Sender<String>>,
}

impl std::fmt::Debug for Irc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Irc({}, {})", self.name, self.nick)
    }
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// An IRC line's parts: who sent it, its command, and its parameters (the
/// last one's text whole).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// The sender's nick, if the line names one.
    pub from: Option<String>,
    /// The command (`PRIVMSG`, `001`).
    pub command: String,
    /// The parameters.
    pub params: Vec<String>,
}

/// Reads a line as IRC (and Twitch, whose tags come first) writes it.
pub fn parse(line: &str) -> Option<Line> {
    let mut rest = line.trim_end_matches(['\r', '\n']);
    if rest.starts_with('@') {
        rest = rest.split_once(' ')?.1;
    }
    let mut from = None;
    if let Some(prefixed) = rest.strip_prefix(':') {
        let (prefix, after) = prefixed.split_once(' ')?;
        from = Some(prefix.split('!').next().unwrap_or(prefix).to_string());
        rest = after;
    }
    let (head, trailing) = match rest.split_once(" :") {
        Some((head, trailing)) => (head, Some(trailing)),
        None => (rest, None),
    };
    let mut words = head.split(' ').filter(|w| !w.is_empty());
    let command = words.next()?.to_ascii_uppercase();
    let mut params: Vec<String> = words.map(str::to_string).collect();
    if let Some(trailing) = trailing {
        params.push(trailing.to_string());
    }
    Some(Line {
        from,
        command,
        params,
    })
}

/// A message from a `PRIVMSG` to `me`, or to a channel `me` is in: a
/// channel's is its channel's; one to `me` alone is a direct message.
pub fn message_of(line: &Line, me: &str) -> Option<Incoming> {
    if line.command != "PRIVMSG" || line.params.len() < 2 {
        return None;
    }
    let from = line.from.clone()?;
    if from.eq_ignore_ascii_case(me) {
        return None;
    }
    let target = &line.params[0];
    let mut text = line.params[1].clone();
    // A CTCP ACTION (`/me …`) is said as itself; other CTCP is not talk.
    if let Some(ctcp) = text.strip_prefix('\u{1}') {
        let ctcp = ctcp.trim_end_matches('\u{1}');
        text = ctcp.strip_prefix("ACTION ")?.to_string();
    }
    let direct = !target.starts_with(['#', '&']);
    let named = text
        .split(|c: char| !(c.is_alphanumeric() || "_-[]\\^{}|`".contains(c)))
        .any(|word| word.eq_ignore_ascii_case(me));
    // "nick: …" or "nick, …" is said to the bot.
    let lower = text.to_ascii_lowercase();
    let me_lower = me.to_ascii_lowercase();
    for lead in [format!("{me_lower}:"), format!("{me_lower},")] {
        if lower.starts_with(&lead) {
            text = text[lead.len()..].trim().to_string();
            break;
        }
    }
    if text.trim().is_empty() {
        return None;
    }
    Some(Incoming {
        id: String::new(),
        thread: String::new(),
        chat: if direct { from.clone() } else { target.clone() },
        from,
        text,
        voice: None,
        direct,
        mentions: if named {
            vec![me.to_string()]
        } else {
            Vec::new()
        },
        files: Vec::new(),
    })
}

/// `text` as the lines a reply to `chat` takes: each `PRIVMSG`, cut at
/// character boundaries, a few at most.
pub fn reply_lines(chat: &str, text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.lines().filter(|l| !l.trim().is_empty()) {
        let mut rest = paragraph.trim();
        while !rest.is_empty() {
            let mut end = rest.len().min(LINE);
            while !rest.is_char_boundary(end) {
                end -= 1;
            }
            // Cut at a space where one is near.
            if end < rest.len() {
                if let Some(space) = rest[..end].rfind(' ').filter(|s| *s > LINE / 2) {
                    end = space;
                }
            }
            lines.push(format!("PRIVMSG {chat} :{}", rest[..end].trim()));
            rest = rest[end..].trim_start();
        }
    }
    if lines.is_empty() {
        lines.push(format!("PRIVMSG {chat} :(nothing)"));
    }
    if lines.len() > MOST_LINES {
        lines.truncate(MOST_LINES);
        if let Some(last) = lines.last_mut() {
            last.push_str(" …");
        }
    }
    lines
}

fn write_line(stream: &mut sion_http::Stream, line: &str) -> Result<(), Fault> {
    // No line break may come from what is sent: it would be a command.
    let line = line.replace(['\r', '\n'], " ");
    stream
        .write_all(format!("{line}\r\n").as_bytes())
        .and_then(|()| stream.flush())
        .map_err(again)
}

fn session(
    client: &sion_http::Client,
    config: &Config,
    outgoing: &Receiver<String>,
    link: &Link,
) -> Result<Next, Fault> {
    let mut stream = client.connect(&config.server).map_err(again)?;
    let mut nick = config.nick.clone();
    if let Some(pass) = &config.pass {
        write_line(&mut stream, &format!("PASS {pass}"))?;
    }
    if config.sasl.is_some() {
        write_line(&mut stream, "CAP REQ :sasl")?;
    }
    write_line(&mut stream, &format!("NICK {nick}"))?;
    write_line(&mut stream, &format!("USER {nick} 0 * :sion"))?;
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .map_err(again)?;
    let mut read: Vec<u8> = Vec::new();
    let mut registered = false;
    let mut sent_at = Instant::now();
    let mut heard_at = Instant::now();
    let mut tries = 0;
    loop {
        if link.stopped() {
            // What was queued goes first: the bridge's last replies.
            for line in outgoing.try_iter().take(MOST_LINES) {
                write_line(&mut stream, &line)?;
                std::thread::sleep(Duration::from_millis(100));
            }
            let _ = write_line(&mut stream, "QUIT :stopped");
            return Ok(Next::Stop);
        }
        if registered && sent_at.elapsed() >= config.pause {
            if let Ok(line) = outgoing.try_recv() {
                write_line(&mut stream, &line)?;
                sent_at = Instant::now();
            }
        }
        if heard_at.elapsed() > SILENCE {
            return Err(again("the server has been silent too long"));
        }
        let mut chunk = [0u8; 4096];
        match stream.read(&mut chunk) {
            Ok(0) => return Err(again("the server closed the connection")),
            Ok(n) => {
                read.extend_from_slice(&chunk[..n]);
                heard_at = Instant::now();
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => return Err(again(e)),
        }
        if read.len() > 64 << 10 {
            return Err(again("a line longer than any IRC line"));
        }
        while let Some(end) = read.iter().position(|b| *b == b'\n') {
            let raw: Vec<u8> = read.drain(..=end).collect();
            let Some(line) = parse(&String::from_utf8_lossy(&raw)) else {
                continue;
            };
            match line.command.as_str() {
                "PING" => write_line(
                    &mut stream,
                    &format!(
                        "PONG :{}",
                        line.params.last().map(String::as_str).unwrap_or("")
                    ),
                )?,
                "001" => {
                    registered = true;
                    for group in config.channels.chunks(10) {
                        write_line(&mut stream, &format!("JOIN {}", group.join(",")))?;
                    }
                }
                "433" if !registered => {
                    tries += 1;
                    if tries > 3 {
                        return Err(Fault::Fatal(format!("the nick {} is taken", config.nick)));
                    }
                    nick.push('_');
                    write_line(&mut stream, &format!("NICK {nick}"))?;
                }
                "CAP" if line.params.get(1).is_some_and(|p| p == "ACK") => {
                    write_line(&mut stream, "AUTHENTICATE PLAIN")?
                }
                "CAP" if line.params.get(1).is_some_and(|p| p == "NAK") => {
                    return Err(Fault::Fatal("the server offers no SASL login".into()));
                }
                "AUTHENTICATE" => {
                    if let Some((account, password)) = &config.sasl {
                        let plain = format!("{account}\0{account}\0{password}");
                        write_line(
                            &mut stream,
                            &format!("AUTHENTICATE {}", sion_http::base64(plain.as_bytes())),
                        )?;
                    }
                }
                "903" => write_line(&mut stream, "CAP END")?,
                "904" | "905" | "906" => {
                    return Err(Fault::Fatal("the server refused the SASL login".into()))
                }
                "NOTICE"
                    if line
                        .params
                        .last()
                        .is_some_and(|t| t.contains("Login authentication failed")) =>
                {
                    return Err(Fault::Fatal("Twitch refused the token".into()))
                }
                "ERROR" => return Ok(Next::Resume),
                "PRIVMSG" => {
                    if let Some(message) = message_of(&line, &nick) {
                        if !link.pass(message) {
                            let _ = write_line(&mut stream, "QUIT :stopped");
                            return Ok(Next::Stop);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

impl Irc {
    /// `--via irc`: `IRC_SERVER`, `IRC_NICK`, and `IRC_PASSWORD` (as
    /// `IRC_ACCOUNT`) for SASL; joined to the `channels` (`#…`) of `chats`.
    pub fn irc_from_env(chats: &[String]) -> Result<Irc, String> {
        let nick = env("IRC_NICK").ok_or("set IRC_NICK")?;
        let sasl = env("IRC_PASSWORD")
            .map(|password| (env("IRC_ACCOUNT").unwrap_or_else(|| nick.clone()), password));
        Irc::start(
            "irc",
            Config {
                server: env("IRC_SERVER").ok_or("set IRC_SERVER (ircs://host:6697)")?,
                nick,
                sasl,
                pass: None,
                channels: channels_in(chats),
                pause: Duration::from_millis(700),
            },
        )
    }

    /// `--via twitch`: `TWITCH_NICK` with the OAuth token in
    /// `TWITCH_TOKEN`, joined to the channels (`#…`) of `chats`.
    pub fn twitch_from_env(chats: &[String]) -> Result<Irc, String> {
        let token =
            env("TWITCH_TOKEN").ok_or("set TWITCH_TOKEN (an OAuth token with chat scopes)")?;
        Irc::start(
            "twitch",
            Config {
                server: env("TWITCH_IRC_SERVER")
                    .unwrap_or_else(|| "ircs://irc.chat.twitch.tv:6697".into()),
                nick: env("TWITCH_NICK")
                    .ok_or("set TWITCH_NICK (the bot's login)")?
                    .to_ascii_lowercase(),
                sasl: None,
                pass: Some(format!(
                    "oauth:{}",
                    token.strip_prefix("oauth:").unwrap_or(&token)
                )),
                channels: channels_in(chats),
                // Twitch takes 20 messages in 30 seconds from most bots.
                pause: Duration::from_millis(1_600),
            },
        )
    }

    fn start(name: &'static str, config: Config) -> Result<Irc, String> {
        let client = sion_http::Client::new(sion_http::Options {
            // The operator names the server.
            allow_private: true,
            ..sion_http::Options::default()
        })
        .map_err(|e| e.to_string())?;
        let (out, outgoing) = channel();
        let nick = config.nick.clone();
        let live = Live::start(name, move |link| {
            link.set_me(vec![config.nick.clone()]);
            keep(name, &link, |link, _| {
                session(&client, &config, &outgoing, link)
            });
        });
        Ok(Irc {
            name,
            nick,
            live,
            out: Mutex::new(out),
        })
    }
}

/// The channels among `chats`.
fn channels_in(chats: &[String]) -> Vec<String> {
    chats
        .iter()
        .filter(|chat| chat.starts_with(['#', '&']))
        .cloned()
        .collect()
}

impl Messenger for Irc {
    fn name(&self) -> &'static str {
        self.name
    }

    fn chat_word(&self) -> &'static str {
        "channel"
    }

    fn receive(&self, cursor: &str, wait: u64) -> Result<(Vec<Incoming>, String), String> {
        Ok((self.live.receive(wait)?, cursor.to_string()))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        if chat.is_empty() || chat.contains([' ', ',', '\r', '\n']) {
            return Err(format!("{chat:?} is not a channel or a nick"));
        }
        let out = self.out.lock().map_err(|_| "poisoned")?;
        for line in reply_lines(chat, text) {
            out.send(line).map_err(|_| "the connection stopped")?;
        }
        Ok(())
    }

    fn long_polls(&self) -> bool {
        true
    }

    fn once_wait(&self) -> u64 {
        LIVE_ONCE_WAIT
    }

    fn me(&self) -> Result<Vec<String>, String> {
        Ok(vec![self.nick.clone()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_read_as_irc_and_twitch_write_them() {
        let line = parse(":ann!a@host PRIVMSG #ops :lead: how much is left?\r\n").unwrap();
        assert_eq!(line.from.as_deref(), Some("ann"));
        assert_eq!(line.params, ["#ops", "lead: how much is left?"]);
        let message = message_of(&line, "lead").unwrap();
        assert_eq!(
            (message.chat.as_str(), message.text.as_str(), message.direct),
            ("#ops", "how much is left?", false)
        );
        assert_eq!(message.mentions, ["lead"]);
        let private = parse(":ann!a@host PRIVMSG lead :hello").unwrap();
        let direct = message_of(&private, "lead").unwrap();
        assert_eq!((direct.chat.as_str(), direct.direct), ("ann", true));
        let tagged = parse(
            "@badge-info=;display-name=Ann :ann!ann@ann.tmi.twitch.tv PRIVMSG #stream :hi chat",
        )
        .unwrap();
        assert_eq!(message_of(&tagged, "bot").unwrap().text, "hi chat");
        let action = parse(":ann!a@h PRIVMSG #ops :\u{1}ACTION waves\u{1}").unwrap();
        assert_eq!(message_of(&action, "lead").unwrap().text, "waves");
        let version = parse(":ann!a@h PRIVMSG lead :\u{1}VERSION\u{1}").unwrap();
        assert!(message_of(&version, "lead").is_none());
        assert_eq!(
            parse("PING :tmi.twitch.tv").unwrap().params,
            ["tmi.twitch.tv"]
        );
    }

    #[test]
    fn replies_are_cut_into_lines() {
        let long = "word ".repeat(200);
        let lines = reply_lines("#ops", &long);
        assert!(lines.len() <= MOST_LINES);
        assert!(lines
            .iter()
            .all(|l| l.len() <= LINE + 20 && l.starts_with("PRIVMSG #ops :")));
        assert_eq!(
            reply_lines("ann", "one\ntwo"),
            ["PRIVMSG ann :one", "PRIVMSG ann :two"]
        );
        assert_eq!(reply_lines("#ops", "é".repeat(300).as_str()).len(), 2);
    }
}
