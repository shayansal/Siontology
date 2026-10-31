//! WeChat (微信), through Tencent's iLink bot protocol, the one its
//! official ClawBot plugin speaks: a WeChat account signs in by scanning a
//! QR code (`sion agent weixin login`), and its bot then long polls
//! `getupdates` for messages and answers with `sendmessage`, each reply
//! carrying the context token of what it answers.
//!
//! The bot's token is kept encrypted beside the secrets (`weixin.bin`), or
//! given in `WEIXIN_BOT_TOKEN`; `WEIXIN_API_BASE` names another server than
//! `https://ilinkai.weixin.qq.com`. Text is taken and sent, and a voice
//! message as WeChat transcribed it; pictures and files, which iLink keeps
//! encrypted on its CDN, are named to the agent, not fetched.
//!
//! The protocol is Tencent's, published only as its plugin's code and as
//! others' descriptions of it; it has been run here only against a
//! stand-in built from them.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::CliError;
use crate::messengers::{Incoming, Messenger};

/// iLink's server, unless `WEIXIN_API_BASE` names another.
pub const WEIXIN_API: &str = "https://ilinkai.weixin.qq.com";

/// The version of the channel the bridge says it is, as iLink asks.
const CHANNEL_VERSION: &str = "1.0.2";

/// Where the sign-in is kept, beside the secrets.
const FILE: &str = "weixin.bin";

/// What the sealed file starts with.
const MAGIC: &[u8] = b"sion-weixin-v1";

/// The longest a sign-in waits for the code to be scanned.
const LOGIN_SECS: u64 = 300;

/// The most characters of one message.
const WEIXIN_TEXT: usize = 2_000;

/// A signed-in bot: its token, and the server it was given.
#[derive(Clone, Serialize, Deserialize)]
pub struct Bot {
    /// The bot's token.
    pub token: String,
    /// The server to use, as the sign-in said.
    pub base: String,
    /// The bot's id, and the account's who signed in.
    pub bot_id: String,
    /// The WeChat user who signed in.
    pub user_id: String,
}

/// What is kept on this machine: the bot signed in, if one is.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Store {
    bot: Option<Bot>,
}

fn load() -> Result<Store, CliError> {
    crate::secrets::open_json(&crate::secrets::dir(), FILE, MAGIC)
}

fn save(store: &Store) -> Result<(), CliError> {
    crate::secrets::seal_json(&crate::secrets::dir(), FILE, MAGIC, store)
}

impl std::fmt::Debug for Bot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Bot({}, at {})", self.bot_id, self.base)
    }
}

fn base() -> String {
    std::env::var("WEIXIN_API_BASE")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| WEIXIN_API.into())
        .trim_end_matches('/')
        .to_string()
}

fn client(timeout: u64) -> Result<sion_http::Client, String> {
    sion_http::Client::new(sion_http::Options {
        timeout: Duration::from_secs(timeout),
        // The operator names the server.
        allow_private: true,
        ..sion_http::Options::default()
    })
    .map_err(|e| e.to_string())
}

/// The headers iLink takes: the bot's token, and a random account number
/// each request (`X-WECHAT-UIN`, base64 of its decimal digits).
fn headers(token: &str) -> Vec<(String, String)> {
    let mut random = [0u8; 4];
    let _ = sion_http::random(&mut random);
    let uin = u32::from_le_bytes(random).to_string();
    vec![
        ("Content-Type".into(), "application/json".into()),
        ("AuthorizationType".into(), "ilink_bot_token".into()),
        ("Authorization".into(), format!("Bearer {token}")),
        ("X-WECHAT-UIN".into(), sion_http::base64(uin.as_bytes())),
    ]
}

/// Signs in: shows `show` the address of the QR code to scan with WeChat,
/// and waits until it is scanned and confirmed.
pub fn login_with(show: &mut dyn FnMut(&str)) -> Result<String, CliError> {
    let base = base();
    let failed = |e: String| CliError::Usage(format!("WeChat's sign-in: {e}"));
    let client = client(40).map_err(failed)?;
    let fetched = client
        .request(
            "GET",
            &format!("{base}/ilink/bot/get_bot_qrcode?bot_type=3"),
            &[],
            None,
            1 << 20,
        )
        .map_err(|e| failed(e.to_string()))?;
    let code: Value = serde_json::from_slice(&fetched.body).map_err(|e| failed(e.to_string()))?;
    let qrcode = code["qrcode"]
        .as_str()
        .ok_or_else(|| failed("no code was given".into()))?
        .to_string();
    show(
        code["qrcode_img_content"]
            .as_str()
            .or(code["url"].as_str())
            .unwrap_or(&qrcode),
    );
    let until = Instant::now() + Duration::from_secs(LOGIN_SECS);
    while Instant::now() < until {
        let fetched = client
            .request(
                "GET",
                &format!(
                    "{base}/ilink/bot/get_qrcode_status?qrcode={}",
                    crate::webhooks::encode_form(&[("q", &qrcode)]).trim_start_matches("q=")
                ),
                &[("iLink-App-ClientVersion", "1")],
                None,
                1 << 20,
            )
            .map_err(|e| failed(e.to_string()))?;
        let status: Value =
            serde_json::from_slice(&fetched.body).map_err(|e| failed(e.to_string()))?;
        match status["status"].as_str() {
            Some("confirmed") => {
                let bot = Bot {
                    token: status["bot_token"]
                        .as_str()
                        .ok_or_else(|| failed("no token was given".into()))?
                        .to_string(),
                    base: status["baseurl"]
                        .as_str()
                        .filter(|b| b.starts_with("https://") || b.starts_with("http://"))
                        .map(|b| b.trim_end_matches('/').to_string())
                        .unwrap_or_else(|| base.clone()),
                    bot_id: status["ilink_bot_id"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    user_id: status["ilink_user_id"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                };
                let said = format!(
                    "signed in to WeChat: bot {}; `sion agent bridge <profile> --via weixin` answers it",
                    bot.bot_id
                );
                save(&Store { bot: Some(bot) })?;
                return Ok(said);
            }
            Some("expired") => return Err(failed("the code expired: sign in again".into())),
            _ => std::thread::sleep(Duration::from_secs(1)),
        }
    }
    Err(failed("the code was not scanned in time".into()))
}

/// The bot signed in on this machine, or the one `WEIXIN_BOT_TOKEN` gives.
pub fn bot() -> Result<Bot, String> {
    if let Ok(token) = std::env::var("WEIXIN_BOT_TOKEN") {
        if !token.is_empty() {
            return Ok(Bot {
                token,
                base: base(),
                bot_id: String::new(),
                user_id: String::new(),
            });
        }
    }
    load()
        .map_err(|e| e.to_string())?
        .bot
        .ok_or_else(|| "sign in first: sion agent weixin login (or set WEIXIN_BOT_TOKEN)".into())
}

/// Forgets the sign-in: whether there was one.
pub fn logout() -> Result<bool, CliError> {
    let mut store = load()?;
    let was = store.bot.take().is_some();
    if was {
        save(&store)?;
    }
    Ok(was)
}

/// The messages of a `getupdates` answer, from people (not the bot's own),
/// with the context token each reply needs.
pub fn weixin_messages(answer: &Value) -> Vec<(Incoming, String)> {
    answer["msgs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["message_type"] == 1)
        .filter_map(|m| {
            let from = m["from_user_id"].as_str().filter(|f| !f.is_empty())?;
            let context = m["context_token"].as_str().unwrap_or_default().to_string();
            let mut lines = Vec::new();
            for item in m["item_list"].as_array().into_iter().flatten() {
                match item["type"].as_i64() {
                    Some(1) => {
                        if let Some(text) = item["text_item"]["text"].as_str() {
                            lines.push(text.trim().to_string());
                        }
                    }
                    // WeChat's own transcription of a voice message.
                    Some(3) => match item["voice_item"]["text"]
                        .as_str()
                        .filter(|t| !t.trim().is_empty())
                    {
                        Some(heard) => {
                            lines.push(format!("(voice note, as WeChat heard it) {}", heard.trim()))
                        }
                        None => lines.push("(a voice message, not heard)".into()),
                    },
                    Some(2) => lines.push("(a picture, not fetched)".into()),
                    Some(4) => lines.push(format!(
                        "(a file, not fetched: {})",
                        item["file_item"]["file_name"].as_str().unwrap_or("unnamed")
                    )),
                    Some(5) => lines.push("(a video, not fetched)".into()),
                    _ => {}
                }
            }
            let text = lines.join("\n").trim().to_string();
            if text.is_empty() {
                return None;
            }
            Some((
                Incoming {
                    id: m["message_id"]
                        .as_i64()
                        .map(|n| n.to_string())
                        .or(m["message_id"].as_str().map(str::to_string))
                        .unwrap_or_default(),
                    thread: String::new(),
                    chat: from.to_string(),
                    from: from.to_string(),
                    text,
                    voice: None,
                    direct: true,
                    mentions: Vec::new(),
                    files: Vec::new(),
                },
                context,
            ))
        })
        .collect()
}

/// A WeChat bot.
pub struct Weixin {
    client: sion_http::Client,
    bot: Bot,
    /// The context token of each chat's last message, which its reply
    /// carries.
    contexts: Arc<Mutex<BTreeMap<String, String>>>,
}

impl std::fmt::Debug for Weixin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Weixin({:?})", self.bot)
    }
}

impl Weixin {
    /// The bot signed in on this machine (see [`bot`]).
    pub fn from_env() -> Result<Weixin, String> {
        Ok(Weixin {
            client: client(60)?,
            bot: bot()?,
            contexts: Arc::default(),
        })
    }

    fn post(&self, path: &str, mut body: Value) -> Result<Value, String> {
        body["base_info"] = json!({"channel_version": CHANNEL_VERSION});
        let headers = headers(&self.bot.token);
        let headers: Vec<(&str, &str)> = headers
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        let fetched = self
            .client
            .request(
                "POST",
                &format!("{}{path}", self.bot.base),
                &headers,
                Some(body.to_string().as_bytes()),
                4 << 20,
            )
            .map_err(|e| e.to_string().replace(&self.bot.token, "<token>"))?;
        let answer: Value = serde_json::from_slice(&fetched.body)
            .map_err(|e| format!("WeChat answered with something other than JSON: {e}"))?;
        let code = answer["errcode"]
            .as_i64()
            .or(answer["ret"].as_i64())
            .unwrap_or(0);
        match code {
            0 => Ok(answer),
            -14 => Err("WeChat's sign-in lapsed: sign in again (sion agent weixin login)".into()),
            code => Err(format!(
                "WeChat refused {path} ({code}): {}",
                answer["errmsg"].as_str().unwrap_or("no reason given")
            )),
        }
    }
}

impl Messenger for Weixin {
    fn name(&self) -> &'static str {
        "weixin"
    }

    fn receive(&self, cursor: &str, _wait: u64) -> Result<(Vec<Incoming>, String), String> {
        let answer = self.post("/ilink/bot/getupdates", json!({"get_updates_buf": cursor}))?;
        let mut contexts = self.contexts.lock().unwrap_or_else(|e| e.into_inner());
        let messages = weixin_messages(&answer)
            .into_iter()
            .map(|(message, context)| {
                if !context.is_empty() {
                    contexts.insert(message.chat.clone(), context);
                }
                message
            })
            .collect();
        let next = answer["get_updates_buf"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| cursor.to_string());
        Ok((messages, next))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        let context = self
            .contexts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(chat)
            .cloned()
            .ok_or_else(|| {
                format!("{chat} has written nothing to answer: WeChat lets a bot only reply")
            })?;
        let text: String = text.chars().take(WEIXIN_TEXT).collect();
        self.post(
            "/ilink/bot/sendmessage",
            json!({"msg": {"from_user_id": "", "to_user_id": chat,
                "client_id": format!("sion-{}", crate::runtime::new_id()),
                "message_type": 2, "message_state": 2, "context_token": context,
                "item_list": [{"type": 1, "text_item": {"text": if text.is_empty() { "(nothing)".to_string() } else { text }}}]}}),
        )
        .map(|_| ())
    }

    fn room(&self) -> usize {
        WEIXIN_TEXT
    }

    fn long_polls(&self) -> bool {
        true
    }

    fn sees_strangers(&self) -> bool {
        true
    }
}

/// `sion agent weixin`.
#[derive(Debug, clap::Subcommand)]
pub enum WeixinCommand {
    /// Sign a WeChat account in: scan the QR code shown with WeChat, and
    /// confirm.
    Login,
    /// Forget the sign-in.
    Logout,
}

/// `text` as a QR code drawn with block characters, two rows of modules
/// to a line, its quiet zone around it; dark modules dark on light ones
/// whatever the terminal's colours when `colours` (ANSI black on white),
/// else as the characters' own (for a light page). Empty when `text`
/// does not fit in a QR code.
pub fn terminal_qr(text: &str, colours: bool) -> String {
    use qrcode::render::unicode::Dense1x2;
    let Ok(code) = qrcode::QrCode::new(text.as_bytes()) else {
        return String::new();
    };
    let drawn = code
        .render::<Dense1x2>()
        .dark_color(Dense1x2::Dark)
        .light_color(Dense1x2::Light)
        .build();
    if !colours {
        return drawn;
    }
    drawn
        .lines()
        .map(|line| format!("\x1b[30;107m{line}\x1b[0m"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Runs `sion agent weixin …`.
pub fn run(command: WeixinCommand) -> Result<(), CliError> {
    match command {
        WeixinCommand::Login => {
            let said = login_with(&mut |url| {
                use std::io::IsTerminal;
                println!(
                    "Scan this code with WeChat, then confirm on your phone:\n\n{}\n({url})",
                    terminal_qr(url, std::io::stdout().is_terminal())
                )
            })?;
            println!("{said}");
            Ok(())
        }
        WeixinCommand::Logout => {
            println!(
                "{}",
                if logout()? {
                    "signed out of WeChat"
                } else {
                    "no WeChat sign-in on this machine"
                }
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sign_in_is_drawn_as_a_qr_code() {
        let url = "https://liteapp.weixin.qq.com/q/7GiQu1?qrcode=0123456789abcdef&bot_type=3";
        let plain = terminal_qr(url, false);
        let lines: Vec<&str> = plain.lines().collect();
        // Two rows of modules to a line, the quiet zone included: square.
        let width = lines[0].chars().count();
        assert!(width >= 21 + 8, "{width}");
        assert!(lines.iter().all(|l| l.chars().count() == width));
        assert_eq!(lines.len(), width.div_ceil(2));
        assert!(plain.contains('\u{2588}') && plain.contains(' '));
        // The same code, black on white for any terminal.
        let coloured = terminal_qr(url, true);
        assert!(coloured
            .lines()
            .all(|l| l.starts_with("\x1b[30;107m") && l.ends_with("\x1b[0m")));
        assert_eq!(coloured.lines().count(), lines.len());
        assert_eq!(terminal_qr(&"x".repeat(8000), false), "");
    }

    #[test]
    fn updates_give_peoples_text_and_the_context_to_answer_in() {
        let answer = json!({"ret": 0, "get_updates_buf": "c2", "msgs": [
            {"message_id": 11, "from_user_id": "wxid_ann", "to_user_id": "bot", "message_type": 1,
             "message_state": 2, "context_token": "ctx-1",
             "item_list": [{"type": 1, "text_item": {"text": "How much is left?"}}]},
            {"message_id": 12, "from_user_id": "bot", "message_type": 2, "context_token": "ctx-2",
             "item_list": [{"type": 1, "text_item": {"text": "mine"}}]},
            {"message_id": 13, "from_user_id": "wxid_bo", "message_type": 1, "context_token": "ctx-3",
             "item_list": [{"type": 3, "voice_item": {"text": "Hello"}}, {"type": 2, "image_item": {}}]}
        ]});
        let messages = weixin_messages(&answer);
        assert_eq!(messages.len(), 2, "the bot's own left out");
        assert_eq!(
            (
                messages[0].0.chat.as_str(),
                messages[0].0.text.as_str(),
                messages[0].1.as_str()
            ),
            ("wxid_ann", "How much is left?", "ctx-1")
        );
        assert_eq!(
            messages[1].0.text,
            "(voice note, as WeChat heard it) Hello\n(a picture, not fetched)"
        );
        let uin = headers("t")
            .into_iter()
            .find(|(n, _)| n == "X-WECHAT-UIN")
            .unwrap()
            .1;
        use base64::Engine;
        let digits = base64::engine::general_purpose::STANDARD
            .decode(uin)
            .unwrap();
        assert!(digits.iter().all(u8::is_ascii_digit));
    }
}
