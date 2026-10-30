//! Microsoft Teams, through the Bot Framework: the Bot Connector service
//! sends what is said to the bridge as webhooks (`--listen`), each signed
//! for the bot, and the bridge replies in the conversation through the
//! Connector API, with a token Microsoft Entra ID gives the bot's app.
//!
//! | Variable | What it is |
//! |---|---|
//! | `TEAMS_APP_ID` | the bot's Microsoft App ID |
//! | `TEAMS_APP_PASSWORD` | the app's client secret |
//! | `TEAMS_TENANT_ID` | the app's tenant, for a single-tenant bot (`botframework.com`, for a multi-tenant one, otherwise) |
//!
//! A webhook is taken only with the Bot Connector's token (a JWT signed
//! with RS256 by a key `login.botframework.com` publishes, issued by
//! `https://api.botframework.com`), for the bot's app ID, within its
//! times, by a key endorsed for the activity's channel, and naming the
//! activity's service URL; the reply goes to that service URL alone, as
//! Microsoft's documentation asks.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::jwt::{Bearer, KeySet};
use crate::live::Live;
use crate::messengers::{segment, Incoming, Messenger, LIVE_ONCE_WAIT};
use crate::webhooks::{client, cut, encode_form, env, inbox, json_of, needed, Answer, Hook, Seen};

/// Who issues the Bot Connector's tokens.
pub const ISSUER: &str = "https://api.botframework.com";

/// The Bot Connector's OpenID configuration, which names its keys.
pub const OPENID: &str = "https://login.botframework.com/v1/.well-known/openidconfiguration";

/// The Connector API's scope.
const SCOPE: &str = "https://api.botframework.com/.default";

/// Where a conversation is answered: its service, its id, and the
/// activity the reply answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conversation {
    /// The service URL the Bot Connector's token named.
    pub service_url: String,
    /// The conversation's id.
    pub id: String,
    /// The activity answered.
    pub reply_to: String,
}

/// The conversations written in, by id.
pub type Conversations = Arc<Mutex<BTreeMap<String, Conversation>>>;

/// What a webhook is checked with: the bot's app ID, and the Bot
/// Connector's keys.
#[derive(Debug)]
pub struct Guard {
    /// The bot's app ID, the tokens' audience.
    pub app_id: String,
    /// The keys.
    pub keys: KeySet,
    /// The client the keys are fetched with.
    pub client: sion_http::Client,
}

/// A bot in Microsoft Teams.
pub struct Teams {
    client: sion_http::Client,
    app_id: String,
    secret: String,
    token_url: String,
    conversations: Conversations,
    me: Arc<Mutex<BTreeSet<String>>>,
    token: Mutex<Bearer>,
    live: Live,
}

impl std::fmt::Debug for Teams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Teams({})", self.app_id)
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// What a Teams message says: the mentions of the bot (`<at>…</at>`) cut,
/// HTML read for its text.
pub fn teams_text(activity: &Value) -> String {
    let mut text = activity["text"].as_str().unwrap_or_default().to_string();
    for entity in activity["entities"].as_array().into_iter().flatten() {
        if entity["type"] == "mention" {
            if let Some(mention) = entity["text"].as_str().filter(|m| !m.is_empty()) {
                text = text.replace(mention, "");
            }
        }
    }
    let html = activity["textFormat"] != "plain"
        && (text.contains("</") || text.contains("<br") || text.contains("&nbsp;"));
    if html {
        crate::email::html_text(&text)
    } else {
        text.trim().to_string()
    }
}

/// Answers one of the Bot Connector's webhooks: refused unless its token
/// is the Bot Connector's for the bot (see the module's documentation);
/// a message, passed on, its conversation kept for the reply.
pub fn teams_hook(
    hook: &Hook,
    guard: &mut Guard,
    conversations: &Conversations,
    me: &Mutex<BTreeSet<String>>,
    seen: &mut Seen,
    now: u64,
) -> (Answer, Vec<Incoming>) {
    let refused = |status: u16, why: &str| (Answer::text(status, why), Vec::new());
    if hook.method != "POST" {
        return refused(405, "only POST");
    }
    let token = hook
        .header("authorization")
        .and_then(|a| {
            a.strip_prefix("Bearer ")
                .or_else(|| a.strip_prefix("bearer "))
        })
        .unwrap_or_default()
        .trim();
    if token.is_empty() {
        return refused(401, "no token");
    }
    let Ok(activity) = serde_json::from_slice::<Value>(&hook.body) else {
        return refused(400, "not JSON");
    };
    let (claims, key) = match guard.keys.verify(&guard.client, token) {
        Ok(verified) => verified,
        Err(why) => return refused(401, &why),
    };
    if let Err(why) = crate::jwt::check(&claims, &[ISSUER], &guard.app_id, now) {
        return refused(401, &why);
    }
    let service_url = activity["serviceUrl"].as_str().unwrap_or_default();
    if service_url.is_empty() || claims["serviceUrl"].as_str() != Some(service_url) {
        return refused(401, "the token names another service URL");
    }
    let channel = activity["channelId"].as_str().unwrap_or_default();
    if !key.endorsements.iter().any(|e| e == channel) {
        return refused(
            403,
            "the token's key is not endorsed for the activity's channel",
        );
    }
    // Only messages are talk; other activities are taken and let be.
    if activity["type"] != "message" || activity["from"]["role"] == "bot" {
        return (Answer::ok(), Vec::new());
    }
    let id = activity["id"].as_str().unwrap_or_default();
    let conversation = activity["conversation"]["id"].as_str().unwrap_or_default();
    if id.is_empty() || conversation.is_empty() || !seen.first(&format!("{conversation} {id}")) {
        return (Answer::ok(), Vec::new());
    }
    let text = teams_text(&activity);
    if text.is_empty() {
        return (Answer::ok(), Vec::new());
    }
    if let Some(bot) = activity["recipient"]["id"].as_str() {
        me.lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(bot.to_string());
    }
    conversations
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            conversation.to_string(),
            Conversation {
                service_url: service_url.to_string(),
                id: conversation.to_string(),
                reply_to: id.to_string(),
            },
        );
    let mentions = activity["entities"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["type"] == "mention")
        .filter_map(|e| e["mentioned"]["id"].as_str().map(str::to_string))
        .collect();
    let from = activity["from"]["aadObjectId"]
        .as_str()
        .or(activity["from"]["id"].as_str())
        .unwrap_or_default();
    (
        Answer::ok(),
        vec![Incoming {
            chat: conversation.to_string(),
            from: from.to_string(),
            text,
            voice: None,
            direct: activity["conversation"]["conversationType"] == "personal",
            mentions,
        }],
    )
}

impl Teams {
    /// The bot `TEAMS_APP_ID` with the secret `TEAMS_APP_PASSWORD` (of the
    /// tenant `TEAMS_TENANT_ID`), listening on `listen`; `TEAMS_OPENID_URL`
    /// and `TEAMS_LOGIN_URL` point it at other servers (stand-ins).
    pub fn from_env(listen: &str) -> Result<Teams, String> {
        let app_id = needed("TEAMS_APP_ID", "the bot's Microsoft App ID")?;
        let secret = needed("TEAMS_APP_PASSWORD", "the app's client secret")?;
        let tenant = env("TEAMS_TENANT_ID").unwrap_or_else(|| "botframework.com".into());
        let login = env("TEAMS_LOGIN_URL")
            .unwrap_or_else(|| "https://login.microsoftonline.com".into())
            .trim_end_matches('/')
            .to_string();
        let openid = env("TEAMS_OPENID_URL").unwrap_or_else(|| OPENID.into());
        let client = client()?;
        let conversations: Conversations = Arc::new(Mutex::new(BTreeMap::new()));
        let me = Arc::new(Mutex::new(BTreeSet::new()));
        let mut guard = Guard {
            app_id: app_id.clone(),
            keys: KeySet::discovered(&openid),
            client: client.clone(),
        };
        let (kept, known) = (conversations.clone(), me.clone());
        let mut seen = Seen::default();
        let live = inbox("teams", listen, move |hook| {
            teams_hook(hook, &mut guard, &kept, &known, &mut seen, now())
        })?;
        Ok(Teams {
            client,
            token_url: format!("{login}/{}/oauth2/v2.0/token", segment(&tenant)),
            app_id,
            secret,
            conversations,
            me,
            token: Mutex::new(Bearer::default()),
            live,
        })
    }

    /// A token for the Connector API (client credentials).
    fn new_token(&self) -> Result<(String, u64), String> {
        let body = encode_form(&[
            ("grant_type", "client_credentials"),
            ("client_id", &self.app_id),
            ("client_secret", &self.secret),
            ("scope", SCOPE),
        ]);
        let fetched = self
            .client
            .request(
                "POST",
                &self.token_url,
                &[("Content-Type", "application/x-www-form-urlencoded")],
                Some(body.as_bytes()),
                1 << 16,
            )
            .map_err(|e| {
                format!(
                    "the bot's token: {}",
                    e.to_string().replace(&self.secret, "<secret>")
                )
            })?;
        let value = json_of(&fetched.body, "Microsoft Entra ID")?;
        let token = value["access_token"]
            .as_str()
            .ok_or("Microsoft Entra ID gave no access token")?;
        Ok((
            token.to_string(),
            value["expires_in"].as_u64().unwrap_or(3600),
        ))
    }
}

impl Messenger for Teams {
    fn name(&self) -> &'static str {
        "teams"
    }

    fn chat_word(&self) -> &'static str {
        "conversation"
    }

    fn receive(&self, cursor: &str, wait: u64) -> Result<(Vec<Incoming>, String), String> {
        Ok((self.live.receive(wait)?, cursor.to_string()))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        let conversation = self
            .conversations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(chat)
            .cloned()
            .ok_or_else(|| {
                format!("{chat}: nothing came from that conversation yet, and Teams is answered where the bot was written to")
            })?;
        let token = self
            .token
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(|| self.new_token())?;
        let url = format!(
            "{}/v3/conversations/{}/activities/{}",
            conversation.service_url.trim_end_matches('/'),
            segment(&conversation.id),
            segment(&conversation.reply_to)
        );
        let body = json!({
            "type": "message",
            "text": cut(text, 25_000),
            "textFormat": "markdown",
            "replyToId": conversation.reply_to,
        })
        .to_string();
        let auth = format!("Bearer {token}");
        self.client
            .request(
                "POST",
                &url,
                &[
                    ("Authorization", &auth),
                    ("Content-Type", "application/json"),
                ],
                Some(body.as_bytes()),
                1 << 20,
            )
            .map_err(|e| e.to_string().replace(&token, "<token>"))?;
        Ok(())
    }

    fn long_polls(&self) -> bool {
        true
    }

    fn once_wait(&self) -> u64 {
        LIVE_ONCE_WAIT
    }

    fn me(&self) -> Result<Vec<String>, String> {
        let mut names = vec![format!("28:{}", self.app_id)];
        names.extend(
            self.me
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .cloned(),
        );
        Ok(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_is_read_without_the_bots_mention() {
        let activity = json!({
            "type": "message",
            "text": "<at>Lead</at> How much budget is left?",
            "textFormat": "plain",
            "entities": [{"type": "mention", "text": "<at>Lead</at>", "mentioned": {"id": "28:app-1"}}]
        });
        assert_eq!(teams_text(&activity), "How much budget is left?");
        let formatted = json!({"text": "<p>Line one</p><p>Line&nbsp;two</p>", "textFormat": "xml"});
        assert_eq!(teams_text(&formatted), "Line one\n\nLine two");
        assert_eq!(
            teams_text(&json!({"text": "a < b", "textFormat": "plain"})),
            "a < b"
        );
    }
}
