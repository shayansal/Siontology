//! A profile on Telegram, on a devnet, against stand-ins for Telegram's API
//! and for a speech provider's (OpenAI's shape): a voice note is fetched,
//! heard and answered aloud; someone the bridge does not know is given a
//! pairing code, and answered once the operator approves it. On Matrix,
//! the room a stranger invites the account to is joined, and they are
//! given a code there; Slack, polled channel by channel without an
//! app-level token, refuses pairing.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use sion_agent::Autonomy;
use sion_cli::ledger::LedgerExt;
use sion_cli::profiles::Profile;
use sion_cli::runtime::SharedLedger;
use sion_cli::session::Session;
use sion_core::ids::{AssetId, ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::charter::Caveats;
use sion_exec::execute::Event;
use sion_exec::TxKind;
use sion_node::{Devnet, DevnetConfig};

/// What the stand-ins were asked: Telegram's methods with their bodies, and
/// the speech provider's paths with what they carried.
#[derive(Default)]
struct Seen {
    telegram: Vec<(String, Vec<u8>)>,
    speech: Vec<(String, Vec<u8>)>,
}

/// Telegram's stand-in: `updates` once (then none), a voice file, and every
/// send recorded; the speech stand-in hears "How much budget is left?" and
/// speaks an Ogg file.
fn stand_ins(updates: Vec<Value>, seen: Arc<Mutex<Seen>>) -> (String, String) {
    let telegram = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let telegram_base = format!("http://{}", telegram.server_addr());
    let speech = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let speech_base = format!("http://{}/v1", speech.server_addr());
    let updates = Arc::new(Mutex::new(updates));
    let seen_telegram = seen.clone();
    std::thread::spawn(move || {
        for mut request in telegram.incoming_requests() {
            let url = request.url().to_string();
            let mut body = Vec::new();
            std::io::Read::read_to_end(request.as_reader(), &mut body).unwrap();
            if url.starts_with("/file/botbot-token/voice/") {
                let _ = request.respond(tiny_http::Response::from_data(b"OggS-the-voice".to_vec()));
                continue;
            }
            let method = url.rsplit('/').next().unwrap_or_default().to_string();
            let result = match method.as_str() {
                "getUpdates" => Value::Array(std::mem::take(&mut *updates.lock().unwrap())),
                "getFile" => json!({"file_path": "voice/file_7.oga"}),
                _ => json!({"message_id": 1}),
            };
            seen_telegram.lock().unwrap().telegram.push((method, body));
            let _ = request.respond(tiny_http::Response::from_string(
                json!({"ok": true, "result": result}).to_string(),
            ));
        }
    });
    std::thread::spawn(move || {
        for mut request in speech.incoming_requests() {
            let url = request.url().to_string();
            let mut body = Vec::new();
            std::io::Read::read_to_end(request.as_reader(), &mut body).unwrap();
            let authorized = request
                .headers()
                .iter()
                .any(|h| h.field.equiv("Authorization") && h.value.as_str() == "Bearer sk-speech");
            seen.lock().unwrap().speech.push((url.clone(), body));
            let response = match (authorized, url.as_str()) {
                (false, _) => tiny_http::Response::from_string("{}").with_status_code(401),
                (true, "/v1/audio/transcriptions") => tiny_http::Response::from_string(
                    json!({"text": "How much budget is left?"}).to_string(),
                ),
                (true, "/v1/audio/speech") => {
                    tiny_http::Response::from_data(b"OggS-spoken-reply".to_vec())
                }
                _ => tiny_http::Response::from_string("{}").with_status_code(404),
            };
            let _ = request.respond(response);
        }
    });
    (telegram_base, speech_base)
}

fn register(session: &mut Session<'_>, ledger: &SharedLedger, wallet: &str) -> ObjectId {
    let pulse = ledger.as_ref().pulse().unwrap();
    let account = session.id(wallet);
    let view = session
        .must(
            "alice",
            TxKind::RegisterAgent {
                agent: account,
                card: sion_core::hash::Hash32::ZERO,
                caveats: Caveats {
                    payees: None,
                    max_per_payment: Amount::units(10),
                    burst: Amount::units(50),
                    rate: Amount(50_000),
                    expires_at: Pulse(pulse.0 + 100_000),
                    evidence_above: None,
                    evidence_policy: sion_assurance::Policy::standard(),
                    receipts: true,
                    may_delegate: false,
                },
                funding: Amount::units(100),
                agent_bond: Amount::ZERO,
            },
            Vec::new(),
        )
        .unwrap();
    view.effects
        .events
        .iter()
        .find_map(|e| match e {
            Event::AgentRegistered { agent, .. } => Some(*agent),
            _ => None,
        })
        .unwrap()
}

/// The multipart part named `name`, in `body`.
fn part<'a>(body: &'a [u8], name: &str) -> Option<&'a [u8]> {
    let marker = format!("name=\"{name}\"");
    let at = body
        .windows(marker.len())
        .position(|w| w == marker.as_bytes())?;
    let rest = &body[at..];
    let start = rest.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    let end = rest[start..].windows(4).position(|w| w == b"\r\n--")?;
    Some(&rest[start..start + end])
}

#[test]
fn a_voice_note_is_heard_and_answered_aloud_and_a_stranger_pairs() {
    let home = std::env::temp_dir().join(format!("sion-channels-voice-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    // The only test in this binary, so the environment is its own.
    std::env::set_var("SION_HOME", &home);
    let devnet = Arc::new(
        Devnet::start(DevnetConfig {
            rounds_per_pulse: 2,
            prove_chunk: None,
            ..DevnetConfig::default()
        })
        .unwrap(),
    );
    let ledger: SharedLedger = devnet.clone();
    let mut session = Session::open(ledger.as_ref()).unwrap();
    session
        .fund("alice", AssetId::usd(), Amount::units(1_000))
        .unwrap();
    let agent = register(&mut session, &ledger, "lead");
    session
        .fund("lead", AssetId::usd(), Amount::units(1))
        .unwrap();
    let script = home.join("say.json");
    std::fs::write(
        &script,
        json!([{"say": "85 USD left in the budget."}]).to_string(),
    )
    .unwrap();
    let profile = Profile {
        name: "lead".into(),
        wallet: "lead".into(),
        agent,
        models: vec![format!("script:{}", script.display())],
        autonomy: Autonomy::L3,
        memory: None,
        skills: None,
        workspace: None,
        web: false,
        delegate: false,
        subagent_models: Vec::new(),
        adjudicator: None,
        reflect: false,
        shell: None,
        mcp: Vec::new(),
        storage: false,
        instructions: None,
        skill_approvals: true,
        screen: Default::default(),
    };

    let seen = Arc::new(Mutex::new(Seen::default()));
    let (telegram, speech) = stand_ins(
        vec![
            json!({"update_id": 7, "message": {"chat": {"id": 42, "type": "private"}, "from": {"id": 5},
                "voice": {"file_id": "voice-7", "duration": 2, "mime_type": "audio/ogg"}}}),
            json!({"update_id": 8, "message": {"chat": {"id": 77, "type": "private"}, "from": {"id": 9}, "text": "Hello?"}}),
        ],
        seen.clone(),
    );
    std::env::set_var("TELEGRAM_BOT_TOKEN", "bot-token");
    std::env::set_var("TELEGRAM_API_BASE", &telegram);
    std::env::set_var("OPENAI_BASE_URL", &speech);
    std::env::set_var("OPENAI_API_KEY", "sk-speech");
    let options = sion_cli::messengers::ChannelArgs {
        pairing: true,
        hear: Some("openai:gpt-4o-mini-transcribe".into()),
        speak: Some("openai:gpt-4o-mini-tts:alloy".into()),
        require_mention: false,
    }
    .options(vec!["42".into()], Vec::new(), true)
    .unwrap();
    sion_cli::channels::telegram_options(ledger.clone(), &profile, &options).unwrap();

    let seen_now = std::mem::take(&mut *seen.lock().unwrap());
    // The voice note was fetched and heard...
    let methods: Vec<&str> = seen_now.telegram.iter().map(|(m, _)| m.as_str()).collect();
    assert!(methods.contains(&"getFile"), "{methods:?}");
    let paths: Vec<&str> = seen_now.speech.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(paths, ["/v1/audio/transcriptions", "/v1/audio/speech"]);
    let (path, heard) = &seen_now.speech[0];
    assert_eq!(path, "/v1/audio/transcriptions");
    assert_eq!(part(heard, "model"), Some(&b"gpt-4o-mini-transcribe"[..]));
    assert_eq!(part(heard, "file"), Some(&b"OggS-the-voice"[..]));
    // ... and answered aloud, the reply as the caption.
    let (path, spoken) = &seen_now.speech[1];
    assert_eq!(path, "/v1/audio/speech");
    let spoken: Value = serde_json::from_slice(spoken).unwrap();
    assert_eq!(spoken["input"], "85 USD left in the budget.");
    assert_eq!(spoken["voice"], "alloy");
    let (_, voice) = seen_now
        .telegram
        .iter()
        .find(|(m, _)| m == "sendVoice")
        .expect("a voice note was sent");
    assert_eq!(part(voice, "chat_id"), Some(&b"42"[..]));
    assert_eq!(
        part(voice, "caption"),
        Some(&b"85 USD left in the budget."[..])
    );
    assert_eq!(part(voice, "voice"), Some(&b"OggS-spoken-reply"[..]));

    // The stranger was given a code, which the operator approves.
    let (_, sent) = seen_now
        .telegram
        .iter()
        .find(|(m, b)| m == "sendMessage" && String::from_utf8_lossy(b).contains("\"chat_id\":77"))
        .expect("the stranger was answered");
    let sent: Value = serde_json::from_slice(sent).unwrap();
    let text = sent["text"].as_str().unwrap();
    let code = text.rsplit(' ').next().unwrap().to_string();
    assert!(text.contains("sion agent pairing approve"), "{text}");
    let pending = sion_cli::pairing::load().unwrap().pending;
    let request = pending.values().next().expect("a code is waiting");
    assert_eq!(pending.len(), 1);
    assert_eq!(
        (
            request.channel.as_str(),
            request.chat.as_str(),
            request.said.as_str()
        ),
        ("telegram:lead", "77", "Hello?")
    );
    sion_cli::pairing::run(sion_cli::pairing::PairingCommand::Approve { code }).unwrap();
    let pairings = sion_cli::pairing::load().unwrap();
    assert!(pairings.pending.is_empty());
    assert!(pairings.allows("telegram:lead", "77"));

    // Now the stranger is answered, in text.
    let seen_again = Arc::new(Mutex::new(Seen::default()));
    let (telegram, _) = stand_ins(
        vec![
            json!({"update_id": 9, "message": {"chat": {"id": 77, "type": "private"}, "from": {"id": 9}, "text": "Hello again"}}),
        ],
        seen_again.clone(),
    );
    std::env::set_var("TELEGRAM_API_BASE", &telegram);
    sion_cli::channels::telegram_options(ledger.clone(), &profile, &options).unwrap();
    let after = std::mem::take(&mut *seen_again.lock().unwrap());
    let (_, answer) = after
        .telegram
        .iter()
        .find(|(m, _)| m == "sendMessage")
        .expect("the paired chat was answered");
    let answer: Value = serde_json::from_slice(answer).unwrap();
    assert_eq!(answer["chat_id"], 77);
    assert_eq!(answer["text"], "85 USD left in the budget.");

    // On Matrix, someone who writes first invites the account to a room:
    // with pairing it joins, and what they say there gets a code.
    let (matrix, requests) = stand_in(|method, url, _| {
        let room = "%21dm%3Aexample.org";
        let answer = match (method, url) {
            ("GET", "/_matrix/client/v3/account/whoami") => json!({"user_id": "@lead:example.org"}),
            ("GET", "/_matrix/client/v3/sync?timeout=0") => json!({"next_batch": "s1",
                "rooms": {"invite": {"!dm:example.org": {"invite_state": {"events": []}}}}}),
            ("GET", "/_matrix/client/v3/sync?timeout=0&since=s1") => json!({"next_batch": "s2",
                "rooms": {"join": {"!dm:example.org": {"summary": {"m.joined_member_count": 2}, "timeline": {"events": [
                    {"type": "m.room.message", "sender": "@ana:example.org",
                     "content": {"msgtype": "m.text", "body": "Hello?"}}]}}}}}),
            ("POST", u) if u == format!("/_matrix/client/v3/join/{room}") => {
                json!({"room_id": "!dm:example.org"})
            }
            ("PUT", u) if u.starts_with(&format!("/_matrix/client/v3/rooms/{room}/send/")) => {
                json!({"event_id": "$1"})
            }
            _ => return (404, b"{}".to_vec()),
        };
        (200, answer.to_string().into_bytes())
    });
    std::env::set_var("MATRIX_HOMESERVER", &matrix);
    std::env::set_var("MATRIX_ACCESS_TOKEN", "matrix-token");
    let pairing_only = sion_cli::messengers::Options {
        pairing: true,
        once: true,
        ..Default::default()
    };
    let room = sion_cli::messengers::for_bridge("matrix", &pairing_only).unwrap();
    for _ in 0..2 {
        sion_cli::messengers::bridge_with(ledger.clone(), &profile, room.as_ref(), &pairing_only)
            .unwrap();
    }
    let requests = requests.lock().unwrap().clone();
    assert!(
        requests
            .iter()
            .any(|(m, u, _)| m == "POST" && u.contains("/join/%21dm%3Aexample.org")),
        "{requests:?}"
    );
    let (_, _, sent) = requests
        .iter()
        .find(|(m, _, _)| m == "PUT")
        .expect("the room was answered");
    let sent: Value = serde_json::from_slice(sent).unwrap();
    assert!(
        sent["body"]
            .as_str()
            .unwrap()
            .contains("sion agent pairing approve"),
        "{sent}"
    );
    let waiting: Vec<_> = sion_cli::pairing::load()
        .unwrap()
        .pending
        .into_values()
        .map(|r| (r.channel, r.chat, r.from))
        .collect();
    assert_eq!(
        waiting,
        [(
            "matrix:lead".to_string(),
            "!dm:example.org".to_string(),
            "@ana:example.org".to_string()
        )]
    );

    // Slack is polled channel by channel here: pairing is refused there.
    std::env::set_var("SLACK_BOT_TOKEN", "xoxb-stand-in");
    let slack = sion_cli::messengers::for_bridge("slack", &pairing_only).unwrap();
    let refused =
        sion_cli::messengers::bridge_with(ledger, &profile, slack.as_ref(), &pairing_only)
            .unwrap_err()
            .to_string();
    assert!(refused.contains("SLACK_APP_TOKEN"), "{refused}");
    drop(devnet);
    std::fs::remove_dir_all(&home).unwrap();
}

type Kept = Arc<Mutex<Vec<(String, String, Vec<u8>)>>>;

/// A stand-in answering each request with the status and body `answer`
/// gives for its method, URL and body, keeping them all.
fn stand_in(
    answer: impl Fn(&str, &str, &[u8]) -> (u16, Vec<u8>) + Send + 'static,
) -> (String, Kept) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    let kept: Kept = Arc::new(Mutex::new(Vec::new()));
    let keep = kept.clone();
    std::thread::spawn(move || {
        for mut request in server.incoming_requests() {
            let mut body = Vec::new();
            std::io::Read::read_to_end(request.as_reader(), &mut body).unwrap();
            let method = request.method().as_str().to_string();
            let url = request.url().to_string();
            let (status, reply) = answer(&method, &url, &body);
            keep.lock().unwrap().push((method, url, body));
            let _ = request.respond(tiny_http::Response::from_data(reply).with_status_code(status));
        }
    });
    (base, kept)
}
