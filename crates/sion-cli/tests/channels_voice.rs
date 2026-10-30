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

mod common;
use common::part;

/// The tests here set the environment: one at a time.
static ENVIRONMENT: Mutex<()> = Mutex::new(());

fn one_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner())
}

/// A home of its own, a devnet, and the profile `lead`, whose scripted
/// model says the budget left.
fn setup(name: &str) -> (std::path::PathBuf, Arc<Devnet>, SharedLedger, Profile) {
    let home = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
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
    (home, devnet, ledger, profile)
}

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

#[test]
fn a_voice_note_is_heard_and_answered_aloud_and_a_stranger_pairs() {
    let _environment = one_at_a_time();
    let (home, devnet, ledger, profile) = setup("sion-channels-voice");

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

#[test]
fn voice_notes_are_heard_and_spoken_on_every_messenger() {
    let _environment = one_at_a_time();
    let (home, devnet, ledger, profile) = setup("sion-voice-everywhere");
    use common::{http_stand_in, live_stand_in, sent_to};
    use std::sync::OnceLock;

    // A speech service in OpenAI's shape: it hears "How much budget is
    // left?" and speaks an Ogg file.
    let (speech, heard) = http_stand_in(|sent| match sent.url.as_str() {
        "/v1/audio/transcriptions" => (
            200,
            json!({"text": "How much budget is left?"})
                .to_string()
                .into_bytes(),
        ),
        "/v1/audio/speech" => (200, b"OggS-spoken-reply".to_vec()),
        _ => (404, b"{}".to_vec()),
    });
    std::env::set_var("OPENAI_BASE_URL", format!("{speech}/v1"));
    std::env::set_var("OPENAI_API_KEY", "sk-speech");
    let voiced = |chat: &str| sion_cli::messengers::Options {
        allow: vec![chat.into()],
        once: true,
        hearing: Some(sion_cli::voice::Hearing::parse("openai:whisper-1").unwrap()),
        speaking: Some(sion_cli::voice::Speaking::parse("openai:tts-1:alloy").unwrap()),
        ..Default::default()
    };
    let last_heard = || {
        let file = sent_to(&heard, "POST", "/v1/audio/transcriptions")
            .pop()
            .expect("a voice note was heard");
        part(&file.body, "file").unwrap().to_vec()
    };
    let spoken = b"OggS-spoken-reply".to_vec();
    let reply = "85 USD left in the budget.";

    // Discord: an audio attachment, through the gateway; the reply posted
    // with the audio attached.
    let gateway = Arc::new(OnceLock::new());
    let url = gateway.clone();
    let (discord, discord_seen) =
        http_stand_in(
            move |sent| match (sent.method.as_str(), sent.url.as_str()) {
                ("GET", "/gateway/bot") => (
                    200,
                    json!({"url": url.get().unwrap()}).to_string().into_bytes(),
                ),
                ("GET", "/attachments/note.ogg") => (200, b"OggS-discord-note".to_vec()),
                ("POST", "/channels/900/messages") => (200, b"{\"id\": \"1\"}".to_vec()),
                _ => (404, b"{}".to_vec()),
            },
        );
    let note = format!("{discord}/attachments/note.ogg");
    let (socket, _) = live_stand_in(move |peer| {
        peer.send(json!({"op": 10, "d": {"heartbeat_interval": 45000}}));
        peer.expect(1);
        peer.send(
            json!({"op": 0, "s": 1, "t": "READY", "d": {"session_id": "s",
            "resume_gateway_url": "ws://127.0.0.1:9", "user": {"id": "B7"}}}),
        );
        peer.send(
            json!({"op": 0, "s": 2, "t": "MESSAGE_CREATE", "d": {"channel_id": "900",
            "guild_id": "1", "author": {"id": "A1"}, "content": "",
            "attachments": [{"url": note, "content_type": "audio/ogg"}]}}),
        );
    });
    gateway.set(socket).unwrap();
    std::env::set_var("DISCORD_BOT_TOKEN", "discord-voice");
    std::env::set_var("DISCORD_API_BASE", &discord);
    let options = voiced("900");
    let messenger = sion_cli::messengers::for_bridge("discord", &options).unwrap();
    sion_cli::messengers::bridge_with(ledger.clone(), &profile, messenger.as_ref(), &options)
        .unwrap();
    drop(messenger);
    assert_eq!(last_heard(), b"OggS-discord-note");
    let posted = sent_to(&discord_seen, "POST", "/channels/900/messages").remove(0);
    let payload: Value =
        serde_json::from_slice(part(&posted.body, "payload_json").unwrap()).unwrap();
    assert_eq!(payload["content"], reply);
    assert_eq!(part(&posted.body, "files[0]").unwrap(), spoken);

    // Slack in Socket Mode: an audio clip, fetched with the bot's token;
    // the reply uploaded and shared with the text.
    let (socket_at, base_at) = (
        Arc::new(OnceLock::new()),
        Arc::new(OnceLock::<String>::new()),
    );
    let (socket_url, slack_base) = (socket_at.clone(), base_at.clone());
    let (slack, slack_seen) = http_stand_in(move |sent| {
        let answer = match sent.url.as_str() {
            "/apps.connections.open" => json!({"ok": true, "url": socket_url.get().unwrap()}),
            "/auth.test" => json!({"ok": true, "user_id": "UBOT"}),
            "/files/clip.webm" if sent.header("authorization") == Some("Bearer xoxb-voice") => {
                return (200, b"slack-webm-clip".to_vec())
            }
            "/files/clip.webm" => return (403, b"no token".to_vec()),
            "/files.getUploadURLExternal" => json!({"ok": true, "file_id": "F1",
                "upload_url": format!("{}/upload/F1", slack_base.get().unwrap())}),
            "/upload/F1" => return (200, b"OK".to_vec()),
            "/files.completeUploadExternal" => json!({"ok": true}),
            _ => return (404, b"{}".to_vec()),
        };
        (200, answer.to_string().into_bytes())
    });
    base_at.set(slack.clone()).unwrap();
    let clip = format!("{slack}/files/clip.webm");
    let (socket, acks) = live_stand_in(move |peer| {
        peer.send(json!({"type": "hello"}));
        peer.send(
            json!({"envelope_id": "v1", "type": "events_api", "payload": {"event": {
            "type": "message", "subtype": "file_share", "channel": "C3", "user": "U9",
            "text": "", "ts": "400.1", "channel_type": "channel",
            "files": [{"id": "F0", "subtype": "slack_audio", "mimetype": "audio/webm",
                "url_private_download": clip}]}}}),
        );
    });
    socket_at.set(socket).unwrap();
    std::env::set_var("SLACK_BOT_TOKEN", "xoxb-voice");
    std::env::set_var("SLACK_APP_TOKEN", "xapp-voice");
    std::env::set_var("SLACK_API_BASE", &slack);
    let options = voiced("C3");
    let messenger = sion_cli::messengers::for_bridge("slack", &options).unwrap();
    sion_cli::messengers::bridge_with(ledger.clone(), &profile, messenger.as_ref(), &options)
        .unwrap();
    drop(messenger);
    std::env::remove_var("SLACK_APP_TOKEN");
    assert_eq!(last_heard(), b"slack-webm-clip");
    assert_eq!(common::wait_for(&acks, 1), [json!({"envelope_id": "v1"})]);
    let asked = sent_to(&slack_seen, "POST", "/files.getUploadURLExternal").remove(0);
    assert_eq!(
        common::form(&asked.body),
        [
            ("filename".to_string(), "voice.ogg".to_string()),
            ("length".to_string(), spoken.len().to_string())
        ]
    );
    let uploaded = sent_to(&slack_seen, "POST", "/upload/F1").remove(0);
    assert_eq!(uploaded.body, spoken);
    assert!(
        uploaded.header("authorization").is_none(),
        "the upload takes no token"
    );
    let shared = common::form(
        &sent_to(&slack_seen, "POST", "/files.completeUploadExternal")
            .remove(0)
            .body,
    );
    assert!(
        shared.contains(&("channel_id".to_string(), "C3".to_string())),
        "{shared:?}"
    );
    assert!(
        shared.contains(&("initial_comment".to_string(), reply.to_string())),
        "{shared:?}"
    );

    // Matrix: an m.audio message, fetched as authenticated media; the
    // reply uploaded and sent as a voice message.
    let (matrix, matrix_seen) = http_stand_in(|sent| {
        let answer = match (sent.method.as_str(), sent.url.as_str()) {
            ("GET", "/_matrix/client/v3/account/whoami") => json!({"user_id": "@lead:example.org"}),
            ("GET", "/_matrix/client/v3/sync?timeout=0") => json!({"next_batch": "m1"}),
            ("GET", "/_matrix/client/v3/sync?timeout=0&since=m1") => json!({"next_batch": "m2",
                "rooms": {"join": {"!ops:example.org": {"summary": {"m.joined_member_count": 3},
                    "timeline": {"events": [{"type": "m.room.message", "sender": "@ana:example.org",
                        "content": {"msgtype": "m.audio", "body": "voice-message.ogg",
                            "url": "mxc://example.org/note1", "info": {"mimetype": "audio/ogg"}}}]}}}}}),
            ("GET", "/_matrix/client/v1/media/download/example.org/note1") => {
                return (200, b"OggS-matrix-note".to_vec())
            }
            ("POST", u) if u.starts_with("/_matrix/media/v3/upload?filename=voice.ogg") => {
                json!({"content_uri": "mxc://example.org/reply1"})
            }
            ("PUT", u) if u.starts_with("/_matrix/client/v3/rooms/%21ops%3Aexample.org/send/") => {
                json!({"event_id": "$1"})
            }
            _ => return (404, b"{}".to_vec()),
        };
        (200, answer.to_string().into_bytes())
    });
    std::env::set_var("MATRIX_HOMESERVER", &matrix);
    std::env::set_var("MATRIX_ACCESS_TOKEN", "matrix-voice");
    let options = voiced("!ops:example.org");
    let room = sion_cli::messengers::for_bridge("matrix", &options).unwrap();
    for _ in 0..2 {
        sion_cli::messengers::bridge_with(ledger.clone(), &profile, room.as_ref(), &options)
            .unwrap();
    }
    assert_eq!(last_heard(), b"OggS-matrix-note");
    let upload = sent_to(&matrix_seen, "POST", "/_matrix/media/v3/upload").remove(0);
    assert_eq!(upload.body, spoken);
    let message = sent_to(&matrix_seen, "PUT", "/_matrix/client/v3/rooms/")
        .remove(0)
        .json();
    assert_eq!(
        (&message["msgtype"], &message["url"], &message["body"]),
        (
            &json!("m.audio"),
            &json!("mxc://example.org/reply1"),
            &json!(reply)
        )
    );
    assert!(message["org.matrix.msc3245.voice"].is_object(), "{message}");

    // Signal: an audio attachment; the reply sent with the audio attached.
    let (signal, signal_seen) =
        http_stand_in(|sent| match (sent.method.as_str(), sent.url.as_str()) {
            ("GET", "/v1/receive/%2B15559990000") => (
                200,
                json!([{"envelope": {"sourceNumber": "+15550001111", "dataMessage": {
                    "attachments": [{"id": "att1", "contentType": "audio/aac", "size": 16}]}}}])
                .to_string()
                .into_bytes(),
            ),
            ("GET", "/v1/attachments/att1") => (200, b"signal-aac-note".to_vec()),
            ("POST", "/v2/send") => (201, b"{\"timestamp\": \"1\"}".to_vec()),
            _ => (404, b"{}".to_vec()),
        });
    std::env::set_var("SIGNAL_API_BASE", &signal);
    std::env::set_var("SIGNAL_NUMBER", "+15559990000");
    let options = voiced("+15550001111");
    let number = sion_cli::messengers::for_bridge("signal", &options).unwrap();
    sion_cli::messengers::bridge_with(ledger, &profile, number.as_ref(), &options).unwrap();
    assert_eq!(last_heard(), b"signal-aac-note");
    let sent = sent_to(&signal_seen, "POST", "/v2/send").remove(0).json();
    assert_eq!(sent["message"], reply);
    assert_eq!(sent["recipients"], json!(["+15550001111"]));
    assert_eq!(
        sent["base64_attachments"],
        json!([format!(
            "data:audio/ogg;filename=voice.ogg;base64,{}",
            sion_http::base64(&spoken)
        )])
    );
    drop(devnet);
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn webhook_messengers_answer_only_what_is_signed() {
    let _environment = one_at_a_time();
    let (home, devnet, ledger, profile) = setup("sion-webhooks");
    use common::{http_stand_in, sent_to};
    let poster = sion_http::Client::new(sion_http::Options {
        allow_private: true,
        proxy_from_env: false,
        ..sion_http::Options::default()
    })
    .unwrap();
    let post = |url: &str, headers: &[(&str, &str)], body: &[u8]| {
        poster
            .send("POST", url, headers, Some(body))
            .unwrap()
            .status
    };
    let free = || {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("127.0.0.1:{}", probe.local_addr().unwrap().port())
    };
    let reply = "85 USD left in the budget.";
    let options = |chat: &str, listen: &str| sion_cli::messengers::Options {
        allow: vec![chat.into()],
        once: true,
        listen: listen.into(),
        ..Default::default()
    };

    // WhatsApp: Meta's check of the address answered; an unsigned message
    // refused; a signed one answered through the Cloud API.
    let (meta, meta_seen) = http_stand_in(|_| {
        (
            200,
            json!({"messages": [{"id": "wamid.out"}]})
                .to_string()
                .into_bytes(),
        )
    });
    std::env::set_var("WHATSAPP_TOKEN", "wa-token");
    std::env::set_var("WHATSAPP_PHONE_NUMBER_ID", "106");
    std::env::set_var("WHATSAPP_APP_SECRET", "wa-secret");
    std::env::set_var("WHATSAPP_VERIFY_TOKEN", "wa-verify");
    std::env::set_var("WHATSAPP_API_BASE", &meta);
    let listen = free();
    let at = format!("http://{listen}/");
    let whatsapp_options = options("15550001111", &listen);
    let whatsapp = sion_cli::messengers::for_bridge("whatsapp", &whatsapp_options).unwrap();
    let check = poster
        .send(
            "GET",
            &format!("{at}?hub.mode=subscribe&hub.verify_token=wa-verify&hub.challenge=4242"),
            &[],
            None,
        )
        .unwrap();
    assert_eq!(check.status, 200);
    assert_eq!(check.read_all(100).unwrap(), b"4242");
    let body = json!({"entry": [{"changes": [{"value": {"metadata": {"phone_number_id": "106"},
        "messages": [{"id": "wamid.in", "from": "15550001111", "type": "text",
            "text": {"body": "How much budget is left?"}}]}}]}]})
    .to_string();
    assert_eq!(post(&at, &[], body.as_bytes()), 401, "unsigned");
    let signature = format!(
        "sha256={}",
        hex::encode(sion_http::hmac_sha256(b"wa-secret", body.as_bytes()))
    );
    assert_eq!(
        post(&at, &[("X-Hub-Signature-256", &signature)], body.as_bytes()),
        200
    );
    sion_cli::messengers::bridge_with(
        ledger.clone(),
        &profile,
        whatsapp.as_ref(),
        &whatsapp_options,
    )
    .unwrap();
    drop(whatsapp);
    let sent = sent_to(&meta_seen, "POST", "/106/messages").remove(0);
    assert_eq!(sent.header("authorization"), Some("Bearer wa-token"));
    assert_eq!(
        sent.json(),
        json!({"messaging_product": "whatsapp", "to": "15550001111", "type": "text",
            "text": {"body": reply}})
    );

    // LINE: answered with the message's reply token.
    let (line, line_seen) = http_stand_in(|_| (200, b"{}".to_vec()));
    std::env::set_var("LINE_CHANNEL_ACCESS_TOKEN", "line-token");
    std::env::set_var("LINE_CHANNEL_SECRET", "line-secret");
    std::env::set_var("LINE_API_BASE", &line);
    std::env::set_var("LINE_DATA_API_BASE", &line);
    let listen = free();
    let at = format!("http://{listen}/");
    let line_options = options("U1", &listen);
    let channel = sion_cli::messengers::for_bridge("line", &line_options).unwrap();
    let body = json!({"events": [{"type": "message", "webhookEventId": "ev1", "replyToken": "r1",
        "source": {"type": "user", "userId": "U1"},
        "message": {"id": "1", "type": "text", "text": "How much budget is left?"}}]})
    .to_string();
    let forged = sion_http::base64(&sion_http::hmac_sha256(b"not-the-secret", body.as_bytes()));
    assert_eq!(
        post(&at, &[("X-Line-Signature", &forged)], body.as_bytes()),
        401
    );
    let signature = sion_http::base64(&sion_http::hmac_sha256(b"line-secret", body.as_bytes()));
    assert_eq!(
        post(&at, &[("X-Line-Signature", &signature)], body.as_bytes()),
        200
    );
    sion_cli::messengers::bridge_with(ledger.clone(), &profile, channel.as_ref(), &line_options)
        .unwrap();
    drop(channel);
    assert_eq!(
        sent_to(&line_seen, "POST", "/v2/bot/message/reply")
            .remove(0)
            .json(),
        json!({"replyToken": "r1", "messages": [{"type": "text", "text": reply}]})
    );

    // SMS through Twilio: the signature over the public address Twilio was
    // given; the reply sent through Twilio's API.
    let (twilio, twilio_seen) = http_stand_in(|_| (201, b"{\"sid\": \"SM9\"}".to_vec()));
    std::env::set_var("TWILIO_ACCOUNT_SID", "AC1");
    std::env::set_var("TWILIO_AUTH_TOKEN", "tw-token");
    std::env::set_var("TWILIO_NUMBER", "+15559990000");
    std::env::set_var("TWILIO_WEBHOOK_URL", "https://bot.example.org/sms");
    std::env::set_var("TWILIO_API_BASE", &twilio);
    let listen = free();
    let sms_options = options("+15550001111", &listen);
    let number = sion_cli::messengers::for_bridge("sms", &sms_options).unwrap();
    let body = b"MessageSid=SM1&From=%2B15550001111&To=%2B15559990000&Body=How+much+budget+is+left%3F&NumMedia=0";
    let signature = sion_cli::webhooks::twilio_signature(
        "tw-token",
        "https://bot.example.org/sms",
        &sion_cli::webhooks::decode_form(body),
    );
    let form = [("Content-Type", "application/x-www-form-urlencoded")];
    let at = format!("http://{listen}/sms");
    assert_eq!(post(&at, &form, body), 401, "unsigned");
    assert_eq!(
        post(&at, &[form[0], ("X-Twilio-Signature", &signature)], body),
        200
    );
    sion_cli::messengers::bridge_with(ledger, &profile, number.as_ref(), &sms_options).unwrap();
    drop(number);
    let sent = sent_to(
        &twilio_seen,
        "POST",
        "/2010-04-01/Accounts/AC1/Messages.json",
    )
    .remove(0);
    assert_eq!(
        sent.header("authorization").map(str::to_string),
        Some(format!("Basic {}", sion_http::base64(b"AC1:tw-token")))
    );
    assert_eq!(
        common::form(&sent.body),
        [
            ("To".to_string(), "+15550001111".to_string()),
            ("From".to_string(), "+15559990000".to_string()),
            ("Body".to_string(), reply.to_string())
        ]
    );
    drop(devnet);
    std::fs::remove_dir_all(&home).unwrap();
}
