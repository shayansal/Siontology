//! WeCom (企业微信), through a self-built app: its callbacks are encrypted
//! (AES-256-CBC, as WeCom's `WXBizMsgCrypt` has them) and signed with the
//! callback's token, and the bridge answers through the app's API
//! (`message/send`) with an access token from the corp id and the app's
//! secret.
//!
//! | Variable | What |
//! |---|---|
//! | `WECOM_CORP_ID` | the company's id (`ww…`), which every callback names |
//! | `WECOM_AGENT_ID` | the app's id |
//! | `WECOM_SECRET` | the app's secret, for the access token |
//! | `WECOM_TOKEN` | the callback's token, which signs it |
//! | `WECOM_AES_KEY` | the callback's `EncodingAESKey` (43 characters) |
//! | `WECOM_API_BASE` | another server than `https://qyapi.weixin.qq.com` |
//!
//! A callback is taken only when its signature is the token's over what it
//! carries, and what it decrypts to names the company: a callback that is
//! not WeCom's cannot be read, let alone answered (`agent:wecom-callbacks`).
//! Pictures, voice messages and videos are fetched through the app's API
//! (`media/get`); a file the agent shares is uploaded (`media/upload`) and
//! sent as a picture or a file.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{json, Value};

use crate::live::Live;
use crate::messengers::{Incoming, Messenger, Note, LIVE_ONCE_WAIT};
use crate::voice::{Audio, Form};
use crate::webhooks::{client, inbox, json_of, needed, Answer, Hook, Seen};

/// WeCom's API, unless `WECOM_API_BASE` names another.
pub const WECOM_API: &str = "https://qyapi.weixin.qq.com";

/// The most bytes of text WeCom takes in one message.
pub const WECOM_TEXT_BYTES: usize = 2_048;

/// The callbacks' cipher: WeCom's and WeChat's (`WXBizMsgCrypt`). A message
/// is sixteen random bytes, its length (four bytes, big-endian), the
/// message and the id of whom it is for, padded as PKCS#7 has it to 32
/// bytes, encrypted with AES-256-CBC under the `EncodingAESKey` (base64,
/// 43 characters), the key's first sixteen bytes the IV; and signed with
/// SHA-1 over the token, the time, the nonce and the ciphertext, sorted.
#[derive(Clone)]
pub struct MsgCrypt {
    token: String,
    key: [u8; 32],
    receiver: String,
}

impl std::fmt::Debug for MsgCrypt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MsgCrypt(for {})", self.receiver)
    }
}

impl MsgCrypt {
    /// The cipher of a callback with `token` and `aes_key` (the
    /// `EncodingAESKey`), for messages to `receiver` (the corp id, or an
    /// official account's app id).
    pub fn new(token: &str, aes_key: &str, receiver: &str) -> Result<MsgCrypt, String> {
        // The key's last character carries bits past its 32 bytes, which
        // WeCom does not keep at zero (its own sample key does not): they
        // are ignored, as WeCom's libraries ignore them.
        let lenient = base64::engine::GeneralPurpose::new(
            &base64::alphabet::STANDARD,
            base64::engine::GeneralPurposeConfig::new()
                .with_decode_allow_trailing_bits(true)
                .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent),
        );
        let key = lenient
            .decode(aes_key.trim())
            .ok()
            .and_then(|key| <[u8; 32]>::try_from(key).ok())
            .ok_or("the EncodingAESKey is 43 characters of base64")?;
        Ok(MsgCrypt {
            token: token.to_string(),
            key,
            receiver: receiver.to_string(),
        })
    }

    /// The signature over `timestamp`, `nonce` and `encrypted`.
    pub fn signature(&self, timestamp: &str, nonce: &str, encrypted: &str) -> String {
        let mut parts = [self.token.as_str(), timestamp, nonce, encrypted];
        parts.sort_unstable();
        hex::encode(sion_http::sha1(parts.concat().as_bytes()))
    }

    /// Whether `signature` is the token's over the rest.
    pub fn signed(&self, signature: &str, timestamp: &str, nonce: &str, encrypted: &str) -> bool {
        let expected = self.signature(timestamp, nonce, encrypted);
        sion_http::same_bytes(signature.as_bytes(), expected.as_bytes())
    }

    /// What `encrypted` (base64) says, if it decrypts and is for the
    /// receiver.
    pub fn decrypt(&self, encrypted: &str) -> Result<String, String> {
        let unreadable = || "the message does not decrypt with the EncodingAESKey".to_string();
        let data = base64::engine::general_purpose::STANDARD
            .decode(encrypted.trim())
            .map_err(|_| unreadable())?;
        let iv: [u8; 16] = self.key[..16].try_into().expect("16 bytes");
        let plain =
            sion_http::cipher::aes_cbc_decrypt(&self.key, &iv, &data).map_err(|_| unreadable())?;
        let plain = sion_http::cipher::pkcs7_unpad(&plain, 32).map_err(|_| unreadable())?;
        if plain.len() < 20 {
            return Err(unreadable());
        }
        let length = u32::from_be_bytes(plain[16..20].try_into().expect("4 bytes")) as usize;
        let (message, receiver) = plain[20..]
            .split_at_checked(length)
            .ok_or_else(unreadable)?;
        if receiver != self.receiver.as_bytes() {
            return Err("the message is for another company or account".into());
        }
        String::from_utf8(message.to_vec()).map_err(|_| unreadable())
    }

    /// `message` encrypted for the receiver, after `random` (sixteen bytes:
    /// fresh ones for each message).
    pub fn encrypt(&self, message: &str, random: &[u8; 16]) -> Result<String, String> {
        let mut plain = random.to_vec();
        plain.extend((message.len() as u32).to_be_bytes());
        plain.extend(message.as_bytes());
        plain.extend(self.receiver.as_bytes());
        let iv: [u8; 16] = self.key[..16].try_into().expect("16 bytes");
        let encrypted = sion_http::cipher::aes_cbc_encrypt(
            &self.key,
            &iv,
            &sion_http::cipher::pkcs7_pad(&plain, 32),
        )
        .map_err(|e| e.to_string())?;
        Ok(base64::engine::general_purpose::STANDARD.encode(encrypted))
    }
}

/// The text of the element `name` of a flat XML message, as WeCom and
/// WeChat write them (`<Name><![CDATA[…]]></Name>` or `<Name>…</Name>`).
pub fn xml_field(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml.find(&open)? + open.len();
    let end = start + xml[start..].find(&close)?;
    let inner = xml[start..end].trim();
    if let Some(data) = inner
        .strip_prefix("<![CDATA[")
        .and_then(|rest| rest.strip_suffix("]]>"))
    {
        return Some(data.to_string());
    }
    Some(
        inner
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&"),
    )
}

/// The message a decrypted callback carries, unless it is an event or says
/// nothing.
pub fn wecom_message(xml: &str) -> Option<Incoming> {
    let from = xml_field(xml, "FromUserName")?;
    let id = xml_field(xml, "MsgId").unwrap_or_default();
    let kind = xml_field(xml, "MsgType")?;
    let media = || xml_field(xml, "MediaId");
    let (text, voice, files) = match kind.as_str() {
        "text" => (xml_field(xml, "Content")?, None, Vec::new()),
        // WeCom's own transcription, where the app asks for one.
        "voice" => match xml_field(xml, "Recognition").filter(|r| !r.trim().is_empty()) {
            Some(heard) => (
                format!("(voice note, as WeCom heard it) {}", heard.trim()),
                None,
                Vec::new(),
            ),
            None => (
                String::new(),
                media().map(|id| Note {
                    id,
                    media_type: Some(format!(
                        "audio/{}",
                        xml_field(xml, "Format")
                            .unwrap_or_else(|| "amr".into())
                            .to_ascii_lowercase()
                    )),
                    name: None,
                }),
                Vec::new(),
            ),
        },
        "image" | "video" | "file" => (
            String::new(),
            None,
            media()
                .map(|id| Note {
                    id,
                    media_type: None,
                    name: xml_field(xml, "FileName").or(xml_field(xml, "Title")),
                })
                .into_iter()
                .collect(),
        ),
        _ => return None,
    };
    if text.trim().is_empty() && voice.is_none() && files.is_empty() {
        return None;
    }
    Some(Incoming {
        id,
        thread: String::new(),
        chat: from.clone(),
        from,
        text,
        voice,
        direct: true,
        mentions: Vec::new(),
        files,
    })
}

/// Answers one of WeCom's callbacks: the check of the address (its
/// `echostr` decrypted), or a message, taken only when signed with the
/// token and decrypting to one for the company.
pub fn wecom_hook(hook: &Hook, crypt: &MsgCrypt, seen: &mut Seen) -> (Answer, Vec<Incoming>) {
    let refused = |status: u16, why: &str| (Answer::text(status, why), Vec::new());
    let (Some(signature), Some(timestamp), Some(nonce)) = (
        hook.query("msg_signature"),
        hook.query("timestamp"),
        hook.query("nonce"),
    ) else {
        return refused(400, "not a WeCom callback: no signature");
    };
    if hook.method == "GET" {
        let Some(echo) = hook.query("echostr") else {
            return refused(400, "no echostr");
        };
        if !crypt.signed(&signature, &timestamp, &nonce, &echo) {
            return refused(401, "not signed with the callback's token");
        }
        return match crypt.decrypt(&echo) {
            Ok(plain) => (Answer::text(200, &plain), Vec::new()),
            Err(why) => refused(401, &why),
        };
    }
    let body = String::from_utf8_lossy(&hook.body);
    let Some(encrypted) = xml_field(&body, "Encrypt") else {
        return refused(400, "no Encrypt element");
    };
    if !crypt.signed(&signature, &timestamp, &nonce, &encrypted) {
        return refused(401, "not signed with the callback's token");
    }
    let xml = match crypt.decrypt(&encrypted) {
        Ok(xml) => xml,
        Err(why) => return refused(401, &why),
    };
    // Answered at once; the reply goes through the API.
    let ok = Answer::text(200, "success");
    let key = xml_field(&xml, "MsgId").unwrap_or_else(|| format!("{timestamp}:{nonce}"));
    if !seen.first(&key) {
        return (ok, Vec::new());
    }
    (ok, wecom_message(&xml).into_iter().collect())
}

/// An access token and when it lapses.
type Token = Option<(String, Instant)>;

/// A WeCom app.
pub struct WeCom {
    client: sion_http::Client,
    base: String,
    corp: String,
    agent: String,
    secret: String,
    token: Mutex<Token>,
    live: Live,
}

impl std::fmt::Debug for WeCom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WeCom({}, app {})", self.corp, self.agent)
    }
}

impl WeCom {
    /// The app the variables above name, its callbacks taken on `listen`.
    pub fn from_env(listen: &str) -> Result<WeCom, String> {
        let corp = needed("WECOM_CORP_ID", "the company's id")?;
        let agent = needed("WECOM_AGENT_ID", "the app's id")?;
        let secret = needed("WECOM_SECRET", "the app's secret")?;
        let crypt = MsgCrypt::new(
            &needed("WECOM_TOKEN", "the callback's token")?,
            &needed("WECOM_AES_KEY", "the callback's EncodingAESKey")?,
            &corp,
        )?;
        let base = std::env::var("WECOM_API_BASE")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| WECOM_API.into())
            .trim_end_matches('/')
            .to_string();
        let mut seen = Seen::default();
        let live = inbox("wecom", listen, move |hook| {
            wecom_hook(hook, &crypt, &mut seen)
        })?;
        Ok(WeCom {
            client: client()?,
            base,
            corp,
            agent,
            secret,
            token: Mutex::new(None),
            live,
        })
    }

    /// An access token: the one kept, or a new one.
    fn token(&self, fresh: bool) -> Result<String, String> {
        let mut kept = self.token.lock().unwrap_or_else(|e| e.into_inner());
        if let (false, Some((token, until))) = (fresh, kept.as_ref()) {
            if Instant::now() < *until {
                return Ok(token.clone());
            }
        }
        let url = format!(
            "{}/cgi-bin/gettoken?corpid={}&corpsecret={}",
            self.base,
            crate::webhooks::encode_form(&[("x", &self.corp)]).trim_start_matches("x="),
            crate::webhooks::encode_form(&[("x", &self.secret)]).trim_start_matches("x=")
        );
        let fetched = self
            .client
            .get(&url, 1 << 16)
            .map_err(|e| e.to_string().replace(&self.secret, "<secret>"))?;
        let answer = json_of(&fetched.body, "WeCom")?;
        let token = answer["access_token"]
            .as_str()
            .filter(|_| answer["errcode"].as_i64().unwrap_or(0) == 0)
            .ok_or_else(|| {
                format!(
                    "WeCom gave no access token: {}",
                    answer["errmsg"].as_str().unwrap_or("no reason given")
                )
            })?
            .to_string();
        // Renewed a few minutes before it lapses.
        let lasts = answer["expires_in"]
            .as_u64()
            .unwrap_or(7_200)
            .saturating_sub(300);
        *kept = Some((token.clone(), Instant::now() + Duration::from_secs(lasts)));
        Ok(token)
    }

    /// `POST {path}` with `body` and the access token: WeCom's answer, the
    /// token renewed once if WeCom says it lapsed.
    fn post(&self, path: &str, content_type: &str, body: &[u8]) -> Result<Value, String> {
        for fresh in [false, true] {
            let token = self.token(fresh)?;
            let joiner = if path.contains('?') { '&' } else { '?' };
            let url = format!("{}{path}{joiner}access_token={token}", self.base);
            let fetched = self
                .client
                .request(
                    "POST",
                    &url,
                    &[("Content-Type", content_type)],
                    Some(body),
                    1 << 20,
                )
                .map_err(|e| e.to_string().replace(&token, "<token>"))?;
            let answer = json_of(&fetched.body, "WeCom")?;
            match answer["errcode"].as_i64().unwrap_or(0) {
                0 => return Ok(answer),
                // The token is not valid, or lapsed: a new one, once.
                40014 | 42001 if !fresh => continue,
                code => {
                    return Err(format!(
                        "WeCom refused {path} ({code}): {}",
                        answer["errmsg"].as_str().unwrap_or("no reason given")
                    ))
                }
            }
        }
        Err("WeCom refused the access token".into())
    }

    fn message(&self, mut message: Value) -> Result<(), String> {
        message["agentid"] = json!(self.agent.parse::<u64>().unwrap_or(0));
        self.post(
            "/cgi-bin/message/send",
            "application/json",
            message.to_string().as_bytes(),
        )
        .map(|_| ())
    }

    /// Uploads a file of `kind` (`image`, `voice`, `file`): its media id.
    fn upload(
        &self,
        kind: &str,
        name: &str,
        media_type: &str,
        bytes: &[u8],
    ) -> Result<String, String> {
        let (content_type, body) = Form::new().bytes("media", name, media_type, bytes).finish();
        let answer = self.post(
            &format!("/cgi-bin/media/upload?type={kind}"),
            &content_type,
            &body,
        )?;
        Ok(answer["media_id"]
            .as_str()
            .ok_or("WeCom gave no media id")?
            .to_string())
    }
}

/// `text` cut to what WeCom takes, on a character's edge.
fn fit(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if out.len() + c.len_utf8() > WECOM_TEXT_BYTES {
            break;
        }
        out.push(c);
    }
    if out.is_empty() {
        "(nothing)".into()
    } else {
        out
    }
}

impl Messenger for WeCom {
    fn name(&self) -> &'static str {
        "wecom"
    }

    fn receive(&self, cursor: &str, wait: u64) -> Result<(Vec<Incoming>, String), String> {
        Ok((self.live.receive(wait)?, cursor.to_string()))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        self.message(json!({"touser": chat, "msgtype": "text", "text": {"content": fit(text)}}))
    }

    /// WeCom counts bytes: a part of 680 characters fits whatever script
    /// it is in.
    fn room(&self) -> usize {
        WECOM_TEXT_BYTES / 3
    }

    fn long_polls(&self) -> bool {
        true
    }

    fn once_wait(&self) -> u64 {
        LIVE_ONCE_WAIT
    }

    fn fetch_voice(&self, note: &Note) -> Result<Audio, String> {
        if note.id.is_empty()
            || !note
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_@".contains(&b))
        {
            return Err("not a media id".into());
        }
        for fresh in [false, true] {
            let token = self.token(fresh)?;
            let url = format!(
                "{}/cgi-bin/media/get?access_token={token}&media_id={}",
                self.base, note.id
            );
            let fetched = self
                .client
                .get(&url, crate::files::MAX_RECEIVED)
                .map_err(|e| e.to_string().replace(&token, "<token>"))?;
            // An error comes as JSON; the media as itself.
            let json = fetched
                .content_type
                .as_deref()
                .is_some_and(|t| t.contains("json") || t.starts_with("text/plain"));
            if json {
                let answer = json_of(&fetched.body, "WeCom")?;
                match answer["errcode"].as_i64().unwrap_or(0) {
                    40014 | 42001 if !fresh => continue,
                    code => {
                        return Err(format!(
                            "WeCom gave no media ({code}): {}",
                            answer["errmsg"].as_str().unwrap_or("no reason given")
                        ))
                    }
                }
            }
            let media_type = note
                .media_type
                .clone()
                .or(fetched.content_type.clone())
                .unwrap_or_else(|| Audio::sniff(&fetched.body).to_string());
            return Ok(Audio::new(&media_type, fetched.body));
        }
        Err("WeCom refused the access token".into())
    }

    /// A picture as an image message, anything else as a file.
    fn send_file(
        &self,
        message: &Incoming,
        file: &crate::files::File,
        caption: &str,
    ) -> Result<bool, String> {
        let kind = if file.is_picture() { "image" } else { "file" };
        let media = self.upload(kind, &file.name, &file.media_type, &file.bytes)?;
        self.message(json!({"touser": message.chat, "msgtype": kind, kind: {"media_id": media}}))?;
        if !caption.is_empty() {
            self.send(&message.chat, caption)?;
        }
        Ok(true)
    }

    fn send_voice(&self, chat: &str, audio: &Audio, caption: &str) -> Result<bool, String> {
        // WeCom plays AMR voice messages only: other audio goes as a file.
        let kind = if audio.media_type == "audio/amr" {
            "voice"
        } else {
            "file"
        };
        let media = self.upload(kind, &audio.name, &audio.media_type, &audio.bytes)?;
        self.message(json!({"touser": chat, "msgtype": kind, kind: {"media_id": media}}))?;
        self.send(chat, caption)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "jWmYm7qr5nMoAUwZRjGtBxmz3KA1tkAj3ykkR6q2B2C";

    /// The message below, after sixteen 7s, encrypted by OpenSSL.
    const OPENSSL: &str = "q0ysNi5GBe4lFFBIAtQxbE5p4y43d1ZqL6wxWvb4mY4vtpzmjnr4CceLPWAPbK+NGrDBRYZS2REzre+lYx8OED/2HSomVQy8nIA9gmeKUieLtNZ4F07+sSw5QQYD3l8WA0daEKai39azFRgmVfyfWNztiTBd0C0IFAh3DPMnnpCLkkcs5+A1YLih5ZU/DOILvOAAT7g6w6oTXM1V0hLZOJ9siIvc2CyJWpTQoFqGiUf5wZjTK+0YaAs6RjN3gLldSgujyScBaTzMrQqO73rTTA3S2X0eazeW8l/lYaKIqOuJy7dOykKxGuSejLXNGQV/LdsebJBlRBsXejagGzajCHyNEEI5nmNhb7xaQuL76VaT7oPxy02MXgmceZX0B/f1gRZKIJ3I9C2Me+DsIL9AgKpv2RZ4hYk6h+G9rvIkqQUTesCsfclbcEQgBbk/+HIJ+HdfA1bOwnQ963fm06HQyg==";

    #[test]
    fn callbacks_are_read_as_wecom_writes_them() {
        let crypt = MsgCrypt::new("QDG6eK", KEY, "ww5823bf96d3bd56c7").unwrap();
        let xml = "<xml><ToUserName><![CDATA[ww5823bf96d3bd56c7]]></ToUserName><FromUserName><![CDATA[ann]]></FromUserName><CreateTime>1409659813</CreateTime><MsgType><![CDATA[text]]></MsgType><Content><![CDATA[How much is left?]]></Content><MsgId>4561255354251345929</MsgId><AgentID>218</AgentID></xml>";
        let encrypted = crypt.encrypt(xml, &[7; 16]).unwrap();
        // As OpenSSL makes it (`openssl enc -aes-256-cbc -nopad` of the same
        // bytes, padded to 32).
        assert_eq!(encrypted, OPENSSL);
        assert_eq!(crypt.decrypt(&encrypted).unwrap(), xml);
        // For another company, it is not read.
        let other = MsgCrypt::new("QDG6eK", KEY, "ww0000000000000000").unwrap();
        assert!(other.decrypt(&encrypted).is_err());
        let message = wecom_message(xml).unwrap();
        assert_eq!(
            (
                message.chat.as_str(),
                message.text.as_str(),
                message.id.as_str()
            ),
            ("ann", "How much is left?", "4561255354251345929")
        );
        // Signed: the token's SHA-1 over the four, sorted.
        let signature = crypt.signature("1409659813", "263014780", &encrypted);
        let mut parts = ["QDG6eK", "1409659813", "263014780", encrypted.as_str()];
        parts.sort_unstable();
        assert_eq!(
            signature,
            hex::encode(sion_http::sha1(parts.concat().as_bytes()))
        );
        let query = format!("/?msg_signature={signature}&timestamp=1409659813&nonce=263014780");
        let body = format!("<xml><ToUserName><![CDATA[ww5823bf96d3bd56c7]]></ToUserName><Encrypt><![CDATA[{encrypted}]]></Encrypt><AgentID><![CDATA[218]]></AgentID></xml>");
        let hook = |url: &str, body: &str| Hook {
            method: "POST".into(),
            url: url.into(),
            headers: Vec::new(),
            body: body.as_bytes().to_vec(),
        };
        let mut seen = Seen::default();
        let (answer, messages) = wecom_hook(&hook(&query, &body), &crypt, &mut seen);
        assert_eq!((answer.status, messages.len()), (200, 1));
        // Sent again: passed on once.
        assert!(wecom_hook(&hook(&query, &body), &crypt, &mut seen)
            .1
            .is_empty());
        // Signed otherwise, or not at all: refused.
        let forged = query.replace(&signature, &"0".repeat(40));
        assert_eq!(
            wecom_hook(&hook(&forged, &body), &crypt, &mut seen)
                .0
                .status,
            401
        );
        assert_eq!(
            wecom_hook(&hook("/", &body), &crypt, &mut seen).0.status,
            400
        );
        // The address's check: its echostr decrypted.
        let echo = crypt.encrypt("1616140317555161061", &[1; 16]).unwrap();
        let signature = crypt.signature("1", "2", &echo);
        let check = Hook {
            method: "GET".into(),
            url: format!(
                "/?msg_signature={signature}&timestamp=1&nonce=2&echostr={}",
                crate::webhooks::encode_form(&[("x", &echo)]).trim_start_matches("x=")
            ),
            headers: Vec::new(),
            body: Vec::new(),
        };
        let (answer, _) = wecom_hook(&check, &crypt, &mut seen);
        assert_eq!(
            (answer.status, String::from_utf8(answer.body).unwrap()),
            (200, "1616140317555161061".to_string())
        );
    }

    #[test]
    fn pictures_and_voice_are_media_and_events_are_not_messages() {
        let picture = "<xml><FromUserName><![CDATA[ann]]></FromUserName><MsgType><![CDATA[image]]></MsgType><PicUrl><![CDATA[https://example.org/p]]></PicUrl><MediaId><![CDATA[m-1]]></MediaId><MsgId>1</MsgId></xml>";
        assert_eq!(wecom_message(picture).unwrap().files[0].id, "m-1");
        let voice = "<xml><FromUserName>ann</FromUserName><MsgType>voice</MsgType><MediaId>m-2</MediaId><Format>amr</Format><MsgId>2</MsgId></xml>";
        let note = wecom_message(voice).unwrap().voice.unwrap();
        assert_eq!(
            (note.id.as_str(), note.media_type.as_deref()),
            ("m-2", Some("audio/amr"))
        );
        let heard = "<xml><FromUserName>ann</FromUserName><MsgType>voice</MsgType><MediaId>m-3</MediaId><Recognition><![CDATA[How much?]]></Recognition></xml>";
        assert_eq!(
            wecom_message(heard).unwrap().text,
            "(voice note, as WeCom heard it) How much?"
        );
        let event = "<xml><FromUserName>ann</FromUserName><MsgType>event</MsgType><Event>enter_agent</Event></xml>";
        assert!(wecom_message(event).is_none());
        assert_eq!(xml_field("<a>x &amp; y</a>", "a").as_deref(), Some("x & y"));
        assert_eq!(fit(&"界".repeat(1000)).len(), 2_046);
    }
}
