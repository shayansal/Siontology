//! A live conversation through a realtime speech model, on a devnet,
//! against a stand-in for OpenAI's Realtime API: the session is set up
//! with one tool and no turn detection of the model's; an utterance found
//! on this machine is sent and committed; the model's voice is played as
//! raw samples; and what it asks of `ask_agent` is answered by the
//! profile's agent, whose answer goes back to it.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{json, Value};
use sion_agent::Autonomy;
use sion_cli::ledger::LedgerExt;
use sion_cli::listening::{Detection, Listening};
use sion_cli::profiles::Profile;
use sion_cli::runtime::SharedLedger;
use sion_cli::session::Session;
use sion_core::ids::{AssetId, ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::charter::Caveats;
use sion_exec::execute::Event;
use sion_exec::TxKind;
use sion_node::{Devnet, DevnetConfig};

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

/// A terminal no one types at: its input ends when the sender is dropped.
struct Untyped(std::sync::mpsc::Receiver<()>);

impl std::io::Read for Untyped {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        let _ = self.0.recv();
        Ok(0)
    }
}

/// What the stand-in was sent, kept as it comes.
type Kept = Arc<Mutex<Vec<Value>>>;

/// Receives from the client until `wanted` holds of a message, keeping
/// them all; that message.
fn until(
    socket: &mut sion_http::ws::WebSocket<std::net::TcpStream>,
    kept: &Kept,
    wanted: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(
            Instant::now() < deadline,
            "the client sent nothing that was wanted: {:?}",
            kept.lock()
                .unwrap()
                .iter()
                .map(|v| format!("{} {}", v["type"], v["item"]["call_id"]))
                .collect::<Vec<_>>()
        );
        if let Ok(Some(sion_http::ws::Message::Text(text))) =
            socket.receive(Duration::from_millis(200))
        {
            let value: Value = serde_json::from_str(&text).unwrap();
            kept.lock().unwrap().push(value.clone());
            if wanted(&value) {
                return value;
            }
        }
    }
}

fn audio(seconds: f32) -> String {
    let samples: Vec<u8> = (0..(24_000.0 * seconds) as usize)
        .flat_map(|i| (((i % 48) as i16 - 24) * 200).to_le_bytes())
        .collect();
    base64::engine::general_purpose::STANDARD.encode(samples)
}

#[test]
fn a_realtime_model_hears_speaks_and_asks_the_agent() {
    if !std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        eprintln!("skipped: no python3 for the stand-in microphone");
        return;
    }
    let home = std::env::temp_dir().join(format!("sion-realtime-{}", std::process::id()));
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
    let script = home.join("agent.json");
    std::fs::write(
        &script,
        json!([{"say": "Two meetings today: at nine and at four."}]).to_string(),
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
        image_model: None,
        home_assistant: false,
        budget_tokens: None,
        budget_micros: None,
        max_turns: None,
        max_tokens: None,
        compact_at: None,
        summary_model: None,
        quarantine: false,
        shared_memory: Vec::new(),
        confidential: false,
        confidential_files: Vec::new(),
        routes: Vec::new(),
        sorter: None,
    };

    // The Realtime API's stand-in.
    let done = home.join("done");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let kept: Kept = Arc::new(Mutex::new(Vec::new()));
    let opened = Arc::new(Mutex::new(None));
    let server = {
        let (kept, opened, done) = (kept.clone(), opened.clone(), done.clone());
        std::thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            let (mut socket, request) = sion_http::ws::WebSocket::accept(tcp, 16 << 20).unwrap();
            *opened.lock().unwrap() = Some(request);
            let send = |socket: &mut sion_http::ws::WebSocket<std::net::TcpStream>, v: Value| {
                socket.send_text(&v.to_string()).unwrap()
            };
            until(&mut socket, &kept, |v| v["type"] == "session.update");
            // The utterance: appended, committed, and a reply asked for.
            until(&mut socket, &kept, |v| v["type"] == "response.create");
            send(&mut socket, json!({"type": "response.created"}));
            send(
                &mut socket,
                json!({"type": "conversation.item.input_audio_transcription.completed", "transcript": "What is on today?"}),
            );
            send(
                &mut socket,
                json!({"type": "response.output_audio_transcript.delta", "delta": "On it."}),
            );
            send(
                &mut socket,
                json!({"type": "response.output_audio.delta", "item_id": "item_1", "delta": audio(0.3)}),
            );
            send(
                &mut socket,
                json!({"type": "response.done", "response": {"status": "completed", "output": [
                {"type": "message", "id": "item_1"},
                {"type": "function_call", "name": "ask_agent", "call_id": "call_0",
                 "arguments": "{\"question\": \"/approve\"}"},
                {"type": "function_call", "name": "ask_agent", "call_id": "call_1",
                 "arguments": "{\"question\": \"What is on today?\"}"}]}}),
            );
            // A command through the voice refused; the agent's answer; then
            // a reply asked for.
            let refused = until(&mut socket, &kept, |v| {
                v["type"] == "conversation.item.create" && v["item"]["call_id"] == "call_0"
            });
            assert!(refused["item"]["output"]
                .as_str()
                .unwrap()
                .contains("the person's to type"));
            let answered = until(&mut socket, &kept, |v| {
                v["type"] == "conversation.item.create" && v["item"]["call_id"] == "call_1"
            });
            until(&mut socket, &kept, |v| v["type"] == "response.create");
            send(&mut socket, json!({"type": "response.created"}));
            send(
                &mut socket,
                json!({"type": "response.output_audio_transcript.delta", "delta": "Two meetings, at nine and four."}),
            );
            send(
                &mut socket,
                json!({"type": "response.output_audio.delta", "item_id": "item_2", "delta": audio(0.2)}),
            );
            send(
                &mut socket,
                json!({"type": "response.done", "response": {"status": "completed", "output": [{"type": "message", "id": "item_2"}]}}),
            );
            std::fs::write(&done, "done").unwrap();
            // What else comes, until the client goes.
            let deadline = Instant::now() + Duration::from_secs(30);
            while Instant::now() < deadline {
                match socket.receive(Duration::from_millis(200)) {
                    Ok(Some(sion_http::ws::Message::Text(text))) => kept
                        .lock()
                        .unwrap()
                        .push(serde_json::from_str(&text).unwrap_or(Value::Null)),
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            answered
        })
    };
    std::env::set_var("OPENAI_BASE_URL", format!("http://127.0.0.1:{port}/v1"));
    std::env::set_var("OPENAI_API_KEY", "sk-realtime-test");
    let realtime = sion_cli::realtime::Realtime::parse("openai:gpt-realtime").unwrap();

    // Something said, then quiet until the conversation is done.
    let heard = home.join("heard.raw");
    let listening = Listening {
        listen: vec![
            "python3".into(),
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/fake_mic.py").into(),
            "silence:0.3".into(),
            "tone:0.8".into(),
            "silence:0.2".into(),
            format!("wait:{}", done.display()),
            "silence:0.5".into(),
        ],
        wake: None,
        interrupt: true,
        // The stand-in microphone's speech is a tone: told by its loudness.
        detection: Detection {
            model: false,
            ..Detection::default()
        },
        play_raw: Some(vec![
            "sh".into(),
            "-c".into(),
            format!("cat >> {}", heard.display()),
        ]),
        hold: None,
    };
    let talk = sion_cli::talk::Talk {
        conversation: "desk".into(),
        ..Default::default()
    };
    let (typing, untyped) = std::sync::mpsc::channel::<()>();
    let input = Box::new(std::io::BufReader::new(Untyped(untyped)));
    let mut out: Vec<u8> = Vec::new();
    sion_cli::realtime::run(
        ledger, &profile, &talk, &listening, &realtime, input, &mut out,
    )
    .unwrap();
    drop(typing);
    let answered = server.join().unwrap();
    let said = String::from_utf8_lossy(&out).to_string();

    let request = opened.lock().unwrap().clone().unwrap();
    assert!(
        request.path.ends_with("/v1/realtime?model=gpt-realtime"),
        "{}",
        request.path
    );
    assert!(request
        .headers
        .iter()
        .any(|(n, v)| n == "authorization" && v == "Bearer sk-realtime-test"));
    let kept = kept.lock().unwrap().clone();
    let update = &kept[0];
    assert_eq!(update["session"]["tools"][0]["name"], "ask_agent");
    assert_eq!(
        update["session"]["audio"]["input"]["turn_detection"],
        Value::Null
    );
    // The utterance went as 24 kHz audio, committed.
    let appended: usize = kept
        .iter()
        .filter(|v| v["type"] == "input_audio_buffer.append")
        .map(|v| {
            base64::engine::general_purpose::STANDARD
                .decode(v["audio"].as_str().unwrap())
                .unwrap()
                .len()
        })
        .sum();
    assert!(
        appended >= 24_000 * 2,
        "only {appended} bytes of the utterance"
    );
    assert!(kept
        .iter()
        .any(|v| v["type"] == "input_audio_buffer.commit"));
    // The agent answered what was asked of it.
    assert_eq!(answered["item"]["call_id"], "call_1");
    assert_eq!(
        answered["item"]["output"],
        "Two meetings today: at nine and at four."
    );
    for line in [
        "you (heard): What is on today?",
        "agent: On it.",
        "(asking the agent: What is on today?)",
        "(not asked of the agent: /approve)",
        "agent: Two meetings, at nine and four.",
    ] {
        assert!(said.contains(line), "{line:?} not in:\n{said}");
    }
    // Its voice was played, at 16 kHz: 0.3 s and 0.2 s of it.
    let played = std::fs::read(&heard).unwrap();
    assert!(
        played.len() >= 2 * 7_000,
        "only {} bytes played",
        played.len()
    );
    let _ = std::fs::remove_dir_all(&home);
}
