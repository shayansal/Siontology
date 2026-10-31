//! Phone calls, through Twilio Voice: Twilio answers a call to the
//! number, hears what the caller says (`<Gather input="speech">`) and
//! speaks the agent's reply (`<Say>`), each step a webhook to the bridge,
//! signed with the account's auth token. While the agent works, the call
//! waits on short pauses, the bridge asked again each time
//! (`<Redirect>`); then the reply is said, and the caller heard again.
//!
//! | Variable | What |
//! |---|---|
//! | `TWILIO_AUTH_TOKEN` | the account's auth token, which signs the webhooks |
//! | `TWILIO_VOICE_URL` | the public address Twilio is given for the number's calls (`https://…/phone`); the steps of a call are under it |
//! | `PHONE_LANGUAGE` | the language heard and spoken (`en-US` by default) |
//! | `PHONE_VOICE` | Twilio's voice for `<Say>` (`Polly.Joanna`, say), its default otherwise |
//! | `PHONE_GREETING` | what a call is greeted with |
//!
//! A caller is named by their number, as Twilio gives it; a number can be
//! forged (`agent:phone-callers`). Only the numbers in `--allow` are
//! answered: others are told so and hung up on.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::live::Live;
use crate::messengers::{Incoming, Messenger};
use crate::webhooks::{decode_form, inbox, needed, twilio_signature, Answer, Hook};

/// How long a call waits on one pause for the agent, in seconds.
const PAUSE_SECS: u64 = 2;

/// How long a call waits for the agent's reply in all, before it is told
/// none came.
const WAIT_MOST: Duration = Duration::from_secs(300);

/// The calls under way: each call's caller, and the replies waiting for
/// them.
#[derive(Debug, Default)]
pub struct Calls {
    /// The caller of each call, by its id (`CallSid`), and when it began
    /// waiting for a reply, if it is.
    callers: BTreeMap<String, (String, Option<Instant>)>,
    /// What waits to be said to each caller.
    replies: BTreeMap<String, VecDeque<String>>,
}

/// Text written into TwiML.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// How a call is set up: where its steps are, and how it speaks.
#[derive(Clone, Debug)]
pub struct Line {
    /// The public address of the number's calls.
    pub url: String,
    /// The language heard and spoken.
    pub language: String,
    /// The voice spoken in, if not Twilio's default.
    pub voice: Option<String>,
    /// What a call is greeted with.
    pub greeting: String,
    /// The numbers answered.
    pub allow: Vec<String>,
}

impl Line {
    fn say(&self, text: &str) -> String {
        let voice = self
            .voice
            .as_deref()
            .map(|v| format!(" voice=\"{}\"", escape(v)))
            .unwrap_or_default();
        format!(
            "<Say language=\"{}\"{voice}>{}</Say>",
            escape(&self.language),
            escape(text)
        )
    }

    /// What has the caller heard: `said` spoken first, if anything.
    fn listen(&self, said: &str) -> String {
        format!(
            "<Response><Gather input=\"speech\" action=\"{}/heard\" method=\"POST\" speechTimeout=\"auto\" language=\"{}\">{}</Gather><Redirect method=\"POST\">{}/listen</Redirect></Response>",
            escape(&self.url),
            escape(&self.language),
            if said.is_empty() { String::new() } else { self.say(said) },
            escape(&self.url)
        )
    }

    fn wait(&self) -> String {
        format!(
            "<Response><Pause length=\"{PAUSE_SECS}\"/><Redirect method=\"POST\">{}/wait</Redirect></Response>",
            escape(&self.url)
        )
    }

    fn hang_up(&self, said: &str) -> String {
        format!("<Response>{}<Hangup/></Response>", self.say(said))
    }
}

fn twiml(body: String) -> Answer {
    Answer {
        status: 200,
        content_type: "text/xml",
        body: format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>{body}").into_bytes(),
    }
}

/// Answers one step of a call: a call that comes (greeted and heard), what
/// the caller said (passed on, and the call made to wait), a wait (the
/// reply said, if it came), each only when signed with the auth token
/// for the address Twilio was given.
pub fn phone_hook(
    hook: &Hook,
    token: &str,
    line: &Line,
    calls: &Mutex<Calls>,
) -> (Answer, Vec<Incoming>) {
    let form = decode_form(&hook.body);
    let path = hook.url.split('?').next().unwrap_or("/");
    let step = path.rsplit('/').next().unwrap_or_default();
    let url = match step {
        "heard" | "wait" | "listen" => format!("{}/{step}", line.url),
        _ => line.url.clone(),
    };
    let expected = twilio_signature(token, &url, &form);
    let given = hook.header("x-twilio-signature").unwrap_or_default();
    if !sion_http::same_bytes(given.as_bytes(), expected.as_bytes()) {
        return (
            Answer::text(401, "not signed with the auth token"),
            Vec::new(),
        );
    }
    let field = |name: &str| {
        form.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let (call, caller) = (field("CallSid"), field("From"));
    let mut calls = calls.lock().unwrap_or_else(|e| e.into_inner());
    match step {
        "heard" => {
            let said = field("SpeechResult");
            if said.trim().is_empty() {
                return (twiml(line.listen("")), Vec::new());
            }
            calls
                .callers
                .insert(call.clone(), (caller.clone(), Some(Instant::now())));
            let message = Incoming {
                id: String::new(),
                thread: String::new(),
                chat: caller.clone(),
                from: caller,
                text: said.trim().to_string(),
                voice: None,
                direct: true,
                mentions: Vec::new(),
                files: Vec::new(),
            };
            (twiml(line.wait()), vec![message])
        }
        "wait" => {
            let reply = calls.replies.get_mut(&caller).and_then(VecDeque::pop_front);
            match reply {
                Some(reply) => {
                    if let Some((_, waiting)) = calls.callers.get_mut(&call) {
                        *waiting = None;
                    }
                    (twiml(line.listen(&reply)), Vec::new())
                }
                None => {
                    let waited = calls
                        .callers
                        .get(&call)
                        .and_then(|(_, since)| *since)
                        .is_some_and(|since| since.elapsed() > WAIT_MOST);
                    if waited {
                        (
                            twiml(line.listen("Sorry, no answer came. Say it again?")),
                            Vec::new(),
                        )
                    } else {
                        (twiml(line.wait()), Vec::new())
                    }
                }
            }
        }
        "listen" => (twiml(line.listen("")), Vec::new()),
        // A call that comes.
        _ => {
            if !line.allow.contains(&caller) {
                return (
                    twiml(line.hang_up("Sorry, calls from this number are not answered.")),
                    Vec::new(),
                );
            }
            calls.callers.insert(call, (caller, None));
            (twiml(line.listen(&line.greeting)), Vec::new())
        }
    }
}

/// A phone number's calls, through Twilio.
pub struct Phone {
    calls: Arc<Mutex<Calls>>,
    live: Live,
}

impl std::fmt::Debug for Phone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Phone")
    }
}

impl Phone {
    /// The number the variables above set up, answering `allow`, its
    /// webhooks taken on `listen`.
    pub fn from_env(listen: &str, allow: &[String]) -> Result<Phone, String> {
        let env = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        let token = needed(
            "TWILIO_AUTH_TOKEN",
            "the auth token webhooks are signed with",
        )?;
        let line = Line {
            url: needed(
                "TWILIO_VOICE_URL",
                "the public address Twilio is given for the number's calls",
            )?
            .trim_end_matches('/')
            .to_string(),
            language: env("PHONE_LANGUAGE").unwrap_or_else(|| "en-US".into()),
            voice: env("PHONE_VOICE"),
            greeting: env("PHONE_GREETING")
                .unwrap_or_else(|| "Hello. What can I do for you?".into()),
            allow: allow.to_vec(),
        };
        let calls: Arc<Mutex<Calls>> = Arc::default();
        let kept = calls.clone();
        let live = inbox("phone", listen, move |hook| {
            phone_hook(hook, &token, &line, &kept)
        })?;
        Ok(Phone { calls, live })
    }
}

impl Messenger for Phone {
    fn name(&self) -> &'static str {
        "phone"
    }

    fn chat_word(&self) -> &'static str {
        "caller"
    }

    fn receive(&self, cursor: &str, wait: u64) -> Result<(Vec<Incoming>, String), String> {
        Ok((self.live.receive(wait)?, cursor.to_string()))
    }

    /// Kept for the call to say when it next asks.
    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        let mut calls = self.calls.lock().unwrap_or_else(|e| e.into_inner());
        if !calls.callers.values().any(|(caller, _)| caller == chat) {
            return Err(format!("{chat} is not on a call"));
        }
        calls
            .replies
            .entry(chat.to_string())
            .or_default()
            .push_back(text.to_string());
        Ok(())
    }

    /// One reply is said at once.
    fn room(&self) -> usize {
        3_000
    }

    fn long_polls(&self) -> bool {
        true
    }

    fn sees_strangers(&self) -> bool {
        false
    }

    fn once_wait(&self) -> u64 {
        crate::messengers::LIVE_ONCE_WAIT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line() -> Line {
        Line {
            url: "https://bot.example.org/phone".into(),
            language: "en-US".into(),
            voice: None,
            greeting: "Hello.".into(),
            allow: vec!["+15550001111".into()],
        }
    }

    fn step(path: &str, form: &[(&str, &str)]) -> Hook {
        let body = crate::webhooks::encode_form(form);
        let url = format!("https://bot.example.org{path}");
        let signature = twilio_signature("tok", &url, &decode_form(body.as_bytes()));
        Hook {
            method: "POST".into(),
            url: path.into(),
            headers: vec![("x-twilio-signature".into(), signature)],
            body: body.into_bytes(),
        }
    }

    #[test]
    fn a_call_is_greeted_heard_kept_waiting_and_answered() {
        let (line, calls) = (line(), Mutex::new(Calls::default()));
        let caller = [("CallSid", "CA1"), ("From", "+15550001111")];
        let (answer, _) = phone_hook(&step("/phone", &caller), "tok", &line, &calls);
        let said = String::from_utf8(answer.body).unwrap();
        assert!(
            said.contains(
                "<Gather input=\"speech\" action=\"https://bot.example.org/phone/heard\""
            ),
            "{said}"
        );
        assert!(
            said.contains("<Say language=\"en-US\">Hello.</Say>"),
            "{said}"
        );
        // What the caller said goes to the agent, and the call waits.
        let heard = [
            ("CallSid", "CA1"),
            ("From", "+15550001111"),
            ("SpeechResult", "How much budget is left?"),
        ];
        let (answer, messages) = phone_hook(&step("/phone/heard", &heard), "tok", &line, &calls);
        assert_eq!(messages[0].text, "How much budget is left?");
        assert!(String::from_utf8(answer.body)
            .unwrap()
            .contains("<Redirect method=\"POST\">https://bot.example.org/phone/wait</Redirect>"));
        let waiting = [("CallSid", "CA1"), ("From", "+15550001111")];
        let (answer, _) = phone_hook(&step("/phone/wait", &waiting), "tok", &line, &calls);
        assert!(String::from_utf8(answer.body).unwrap().contains("<Pause"));
        // The reply, once it comes, is said, and the caller heard again.
        let phone = Phone {
            calls: Arc::new(calls),
            live: Live::start("phone-test", |_| {}),
        };
        phone.send("+15550001111", "85 USD & some left.").unwrap();
        assert!(phone.send("+15559999999", "x").is_err(), "no such call");
        let (answer, _) = phone_hook(&step("/phone/wait", &waiting), "tok", &line, &phone.calls);
        let said = String::from_utf8(answer.body).unwrap();
        assert!(
            said.contains("<Say language=\"en-US\">85 USD &amp; some left.</Say>"),
            "{said}"
        );
        assert!(said.contains("<Gather input=\"speech\""), "{said}");
    }

    #[test]
    fn strangers_are_hung_up_on_and_forgeries_refused() {
        let (line, calls) = (line(), Mutex::new(Calls::default()));
        let stranger = [("CallSid", "CA2"), ("From", "+15559999999")];
        let (answer, _) = phone_hook(&step("/phone", &stranger), "tok", &line, &calls);
        assert!(String::from_utf8(answer.body)
            .unwrap()
            .contains("<Hangup/>"));
        let mut forged = step("/phone", &[("CallSid", "CA3"), ("From", "+15550001111")]);
        forged.headers[0].1 = "AAAA".into();
        assert_eq!(phone_hook(&forged, "tok", &line, &calls).0.status, 401);
        // Signed for another step than it asks for: refused.
        let mut moved = step(
            "/phone/wait",
            &[("CallSid", "CA1"), ("From", "+15550001111")],
        );
        moved.url = "/phone/heard".into();
        assert_eq!(phone_hook(&moved, "tok", &line, &calls).0.status, 401);
    }
}
