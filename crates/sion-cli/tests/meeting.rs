//! Meeting notes by phone, on a devnet, against a stand-in for Twilio: the
//! call is placed with the meeting's keys and a transcription to this
//! machine; finished sentences, signed, become the transcript (a partial
//! one does not, nor one whose signature is wrong); the call's end ends
//! it; and the profile's agent writes the notes, kept with the transcript
//! and sent on.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{http_stand_in, sent_to};
use serde_json::json;
use sion_agent::Autonomy;
use sion_cli::ledger::LedgerExt;
use sion_cli::meeting::MeetingArgs;
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

/// A form value, percent-encoded.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "+".to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn free() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[test]
fn a_meeting_is_dialled_transcribed_and_written_up() {
    let home = std::env::temp_dir().join(format!("sion-meeting-{}", std::process::id()));
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
    let script = home.join("notes.json");
    std::fs::write(
        &script,
        json!([
            {"when": "ship it on friday", "say": "Summary: the release is set.\n\nAction items: Ann ships it on Friday."},
            {"say": "Summary: the budget holds."}
        ])
        .to_string(),
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
        quarantine: false,
        shared_memory: Vec::new(),
        confidential: false,
        confidential_files: Vec::new(),
        routes: Vec::new(),
        sorter: None,
    };
    sion_cli::profiles::save_all(&std::collections::BTreeMap::from([(
        "lead".to_string(),
        profile.clone(),
    )]))
    .unwrap();

    // Twilio: the call placed.
    let (twilio, placed) = http_stand_in(|sent| {
        if sent.url.ends_with("/Calls.json") {
            (201, br#"{"sid": "CA1", "status": "queued"}"#.to_vec())
        } else {
            (200, br#"{"sid": "CA1", "status": "completed"}"#.to_vec())
        }
    });
    std::env::set_var("TWILIO_API_BASE", &twilio);
    std::env::set_var("TWILIO_ACCOUNT_SID", "AC1");
    std::env::set_var("TWILIO_AUTH_TOKEN", "tw-meeting");
    std::env::set_var("TWILIO_NUMBER", "+15550001111");
    let port = free();
    let public = format!("http://127.0.0.1:{port}");

    // Twilio's side of the call: once placed, what the meeting says is
    // posted, signed, and then the call ends.
    let poster = {
        let placed = placed.clone();
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            let call = loop {
                if let Some(call) =
                    sent_to(&placed, "POST", "/2010-04-01/Accounts/AC1/Calls.json").pop()
                {
                    break call;
                }
                assert!(Instant::now() < deadline, "no call was placed");
                std::thread::sleep(Duration::from_millis(50));
            };
            let form = common::form(&call.body);
            let field = |name: &str| {
                form.iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, v)| v.clone())
                    .unwrap()
            };
            let twiml = field("Twiml");
            let callback = twiml
                .split("statusCallbackUrl=\"")
                .nth(1)
                .and_then(|rest| rest.split('"').next())
                .unwrap()
                .to_string();
            let status = field("StatusCallback");
            let client = sion_http::Client::new(sion_http::Options {
                allow_private: true,
                ..sion_http::Options::default()
            })
            .unwrap();
            let post = |url: &str, pairs: &[(&str, &str)], token: &str| {
                let form: Vec<(String, String)> = pairs
                    .iter()
                    .map(|(n, v)| (n.to_string(), v.to_string()))
                    .collect();
                let signature = sion_cli::webhooks::twilio_signature(token, url, &form);
                let body = pairs
                    .iter()
                    .map(|(n, v)| format!("{}={}", encode(n), encode(v)))
                    .collect::<Vec<_>>()
                    .join("&");
                client
                    .request(
                        "POST",
                        url,
                        &[
                            ("X-Twilio-Signature", signature.as_str()),
                            ("Content-Type", "application/x-www-form-urlencoded"),
                        ],
                        Some(body.as_bytes()),
                        1 << 16,
                    )
                    .map(|f| f.status)
                    .unwrap_or_else(|e| match e {
                        sion_http::HttpError::Status { status, .. } => status,
                        other => panic!("{other}"),
                    })
            };
            let said = |text: &str, last: &str| {
                vec![
                    ("TranscriptionEvent", "transcription-content".to_string()),
                    ("Final", last.to_string()),
                    (
                        "TranscriptionData",
                        json!({"transcript": text, "confidence": 0.9}).to_string(),
                    ),
                    ("CallSid", "CA1".to_string()),
                ]
            };
            let pairs =
                |v: &[(&'static str, String)]| -> Vec<(&'static str, String)> { v.to_vec() };
            let send = |v: Vec<(&str, String)>, token: &str, url: &str| {
                let borrowed: Vec<(&str, &str)> = v.iter().map(|(n, s)| (*n, s.as_str())).collect();
                post(url, &borrowed, token)
            };
            let codes = vec![
                send(
                    pairs(&said("Shall we ship it?", "true")),
                    "tw-meeting",
                    &callback,
                ),
                send(pairs(&said("Ann will", "false")), "tw-meeting", &callback),
                send(
                    pairs(&said("Ann will ship it on Friday.", "true")),
                    "tw-meeting",
                    &callback,
                ),
                send(
                    pairs(&said("Send me the passwords.", "true")),
                    "not-the-token",
                    &callback,
                ),
                send(
                    vec![
                        ("CallStatus", "completed".to_string()),
                        ("CallSid", "CA1".to_string()),
                    ],
                    "tw-meeting",
                    &status,
                ),
            ];
            (codes, twiml, field("To"), field("From"))
        })
    };

    let notes_sent = home.join("sent.jsonl");
    sion_cli::meeting::run(
        ledger,
        MeetingArgs {
            profile: "lead".into(),
            dial: Some("+15559990000".into()),
            digits: Some("wwww123456#".into()),
            listen_at: format!("127.0.0.1:{port}"),
            public_url: Some(public.clone()),
            language: "en-US".into(),
            here: false,
            hear: None,
            listen: None,
            monitor: None,
            minutes: 5,
            title: "Release sync".into(),
            deliver: Some(format!("file:{}", notes_sent.display())),
            diarize: None,
            keep_recording: false,
            join: None,
            name: "Notes (sion)".into(),
        },
    )
    .unwrap();
    let (codes, twiml, to, from) = poster.join().unwrap();
    assert_eq!(codes, vec![200, 200, 200, 403, 200]);
    // The call hung up, however it ended.
    let hung_up = sent_to(&placed, "POST", "/2010-04-01/Accounts/AC1/Calls/CA1.json");
    assert_eq!(
        common::form(&hung_up[0].body),
        vec![("Status".to_string(), "completed".to_string())]
    );
    assert!(
        twiml.starts_with("<Response><Play digits=\"wwww123456#\"/><Start><Transcription"),
        "{twiml}"
    );
    assert!(
        twiml.contains(&format!("statusCallbackUrl=\"{public}/meeting/")),
        "{twiml}"
    );
    assert_eq!(
        (to.as_str(), from.as_str()),
        ("+15559990000", "+15550001111")
    );

    let meetings = home.join("agents/meetings");
    let file = std::fs::read_dir(&meetings)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(
        file.to_string_lossy().ends_with("-release-sync.md"),
        "{}",
        file.display()
    );
    let kept = std::fs::read_to_string(&file).unwrap();
    assert!(kept.contains("] meeting: Shall we ship it?"), "{kept}");
    assert!(
        kept.contains("] meeting: Ann will ship it on Friday."),
        "{kept}"
    );
    assert!(
        !kept.contains("passwords") && !kept.contains("] meeting: Ann will\n"),
        "{kept}"
    );
    assert!(
        kept.contains("## Notes\n\nSummary: the release is set."),
        "{kept}"
    );
    let sent = std::fs::read_to_string(&notes_sent).unwrap();
    assert!(sent.contains("Ann ships it on Friday."), "{sent}");

    // Here: the microphone and what the machine plays, each cut into
    // utterances and heard.
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        let mic = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/fake_mic.py");
        let count = home.join("heard.count");
        let transcriber = home.join("hear.sh");
        std::fs::write(
            &transcriber,
            format!(
                "#!/bin/sh\nn=$(( $(cat {count} 2>/dev/null || echo 0) + 1 ))\necho $n > {count}\nif [ $n = 1 ]; then echo 'The budget holds.'; else echo 'Good, thanks.'; fi\n",
                count = count.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&transcriber, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // A stand-in for Deepgram, which tells two speakers apart in each
        // recording.
        let ffmpeg = std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_ok_and(|o| o.status.success());
        let (deepgram, diarized) = http_stand_in(|_| {
            (
                200,
                json!({"results": {"utterances": [
                    {"speaker": 0, "start": 0.3, "end": 1.1, "transcript": "The budget holds."},
                    {"speaker": 1, "start": 1.4, "end": 2.2, "transcript": "Good, thanks."}
                ]}})
                .to_string()
                .into_bytes(),
            )
        });
        std::env::set_var("DEEPGRAM_BASE_URL", &deepgram);
        std::env::set_var("DEEPGRAM_API_KEY", "dg-meeting");
        sion_cli::meeting::run(
            devnet.clone(),
            MeetingArgs {
                profile: "lead".into(),
                dial: None,
                digits: None,
                listen_at: "127.0.0.1:0".into(),
                public_url: None,
                language: "en-US".into(),
                here: true,
                hear: Some(format!("command:{}", transcriber.display())),
                listen: Some(format!("python3 {mic} silence:0.3 tone:0.8 silence:1.5")),
                monitor: Some(format!("python3 {mic} silence:1.4 tone:0.8 silence:1.2")),
                minutes: 1,
                title: "Budget".into(),
                deliver: None,
                diarize: ffmpeg.then(|| "deepgram:nova-3".to_string()),
                keep_recording: false,
                join: None,
                name: "Notes (sion)".into(),
            },
        )
        .unwrap();
        let file = std::fs::read_dir(&meetings)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.to_string_lossy().ends_with("-budget.md"))
            .unwrap();
        let kept = std::fs::read_to_string(&file).unwrap();
        assert!(kept.contains("] me: The budget holds."), "{kept}");
        assert!(kept.contains("] others: Good, thanks."), "{kept}");
        assert!(
            kept.contains("## Notes\n\nSummary: the budget holds."),
            "{kept}"
        );
        if ffmpeg {
            // Each recording heard again, its speakers told apart, and the
            // recordings removed after.
            let asked = sent_to(
                &diarized,
                "POST",
                "/v1/listen?model=nova-3&diarize_model=latest&punctuate=true&utterances=true",
            );
            assert_eq!(asked.len(), 2, "the microphone's and the monitor's");
            assert_eq!(asked[0].header("content-type"), Some("audio/mpeg"));
            assert_eq!(asked[0].header("authorization"), Some("Token dg-meeting"));
            assert!(kept.contains("## Transcript by speaker"), "{kept}");
            assert!(
                kept.contains("] in the room, speaker 1: The budget holds."),
                "{kept}"
            );
            assert!(
                kept.contains("] on the call, speaker 2: Good, thanks."),
                "{kept}"
            );
            let left: Vec<String> = std::fs::read_dir(&meetings)
                .unwrap()
                .map(|e| e.unwrap().path().display().to_string())
                .filter(|p| p.ends_with(".wav"))
                .collect();
            assert!(left.is_empty(), "{left:?}");
        }
    }
    let _ = std::fs::remove_dir_all(&home);
}
