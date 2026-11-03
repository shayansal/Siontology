//! The bridge between a messenger and a profile, on a devnet, against a
//! messenger kept in memory: a burst is one turn; what is said while the
//! agent works is told to it, queued, collected, or interrupts it; `/stop`
//! stops it while it writes; chats are answered side by side; a reply that
//! fails is sent again, or kept in an outbox and sent when the bridge
//! starts again; a chat whose answer a restart cut off is told so;
//! messages are marked; the chat commands work through it; and the files a
//! person sends reach the agent, and those it shares reach them.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sion_agent::Autonomy;
use sion_cli::ledger::LedgerExt;
use sion_cli::messengers::{Ack, Busy, Incoming, Messenger, Note, Options};
use sion_cli::profiles::Profile;
use sion_cli::runtime::SharedLedger;
use sion_cli::session::Session;
use sion_cli::showing::{Draft, Drafting, Typist};
use sion_core::ids::{AssetId, ObjectId, Pulse};
use sion_core::Amount;
use sion_exec::charter::Caveats;
use sion_exec::execute::Event;
use sion_exec::TxKind;
use sion_node::{Devnet, DevnetConfig};

mod common;

/// When a batch of messages comes.
#[derive(Debug)]
enum When {
    /// At once.
    Now,
    /// Once the chat named is shown typing.
    Typing(&'static str),
    /// Once a draft in the chat named shows text.
    Drafted(&'static str),
}

/// What the messenger was asked to do.
#[derive(Debug, Default)]
struct Asked {
    /// Replies that went through: chat, text, when.
    sent: Vec<(String, String, Instant)>,
    /// Sends tried, through or not.
    tries: usize,
    /// Marks: the message's id, and what.
    marks: Vec<(String, Ack)>,
    /// Typing shown: chat, when.
    typing: Vec<(String, Instant)>,
    /// Drafts shown: chat, text.
    drafts: Vec<(String, String)>,
    /// What each draft became: chat, text.
    finished: Vec<(String, String)>,
    /// Files sent: chat, name, media type, bytes.
    files: Vec<(String, String, String, usize)>,
}

/// A messenger in memory: it gives its batches as their time comes, then
/// stops the bridge; it fails the sends it is told to fail, and keeps
/// everything else it is asked to do.
#[derive(Debug)]
struct Fake {
    batches: Mutex<VecDeque<(When, Vec<Incoming>)>>,
    asked: Arc<Mutex<Asked>>,
    /// How the next sends fail, one each.
    failing: Mutex<VecDeque<String>>,
    /// Whether it shows replies as they are written.
    drafts: bool,
    /// The files people sent, by note id.
    held: BTreeMap<String, sion_cli::files::File>,
    /// Whether it sends files.
    sends_files: bool,
    stop: AtomicBool,
}

impl Fake {
    fn new(batches: Vec<(When, Vec<Incoming>)>) -> Fake {
        Fake {
            batches: Mutex::new(batches.into()),
            asked: Arc::default(),
            failing: Mutex::default(),
            drafts: false,
            held: BTreeMap::new(),
            sends_files: true,
            stop: AtomicBool::new(false),
        }
    }

    fn holding(self, held: &[(&str, &sion_cli::files::File)]) -> Fake {
        Fake {
            held: held
                .iter()
                .map(|(id, file)| (id.to_string(), (*file).clone()))
                .collect(),
            ..self
        }
    }

    fn sending_no_files(self) -> Fake {
        Fake {
            sends_files: false,
            ..self
        }
    }

    fn drafting(self) -> Fake {
        Fake {
            drafts: true,
            ..self
        }
    }

    fn failing(self, failures: &[&str]) -> Fake {
        *self.failing.lock().unwrap() = failures.iter().map(|f| f.to_string()).collect();
        self
    }

    /// The replies that went to `chat`.
    fn sent(&self, chat: &str) -> Vec<String> {
        self.asked
            .lock()
            .unwrap()
            .sent
            .iter()
            .filter(|(c, _, _)| c == chat)
            .map(|(_, text, _)| text.clone())
            .collect()
    }

    /// What the drafts in `chat` became.
    fn finished(&self, chat: &str) -> Vec<String> {
        self.asked
            .lock()
            .unwrap()
            .finished
            .iter()
            .filter(|(c, _)| c == chat)
            .map(|(_, text)| text.clone())
            .collect()
    }

    fn first_typing(&self, chat: &str) -> Instant {
        self.asked
            .lock()
            .unwrap()
            .typing
            .iter()
            .find(|(c, _)| c == chat)
            .map(|(_, at)| *at)
            .expect("typing shown")
    }

    fn first_sent(&self, chat: &str) -> Instant {
        self.asked
            .lock()
            .unwrap()
            .sent
            .iter()
            .find(|(c, _, _)| c == chat)
            .map(|(_, _, at)| *at)
            .expect("a reply sent")
    }
}

struct Typing {
    chat: String,
    asked: Arc<Mutex<Asked>>,
}

impl Typist for Typing {
    fn show(&mut self) -> Result<Duration, String> {
        self.asked
            .lock()
            .unwrap()
            .typing
            .push((self.chat.clone(), Instant::now()));
        Ok(Duration::from_secs(5))
    }
}

struct Drafted {
    chat: String,
    asked: Arc<Mutex<Asked>>,
}

impl Draft for Drafted {
    fn show(&mut self, text: &str, _status: Option<&str>) -> Result<(), String> {
        self.asked
            .lock()
            .unwrap()
            .drafts
            .push((self.chat.clone(), text.to_string()));
        Ok(())
    }

    fn finish(&mut self, text: &str) -> Result<(), String> {
        self.asked
            .lock()
            .unwrap()
            .finished
            .push((self.chat.clone(), text.to_string()));
        Ok(())
    }
}

impl Messenger for Fake {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn receive(&self, cursor: &str, _wait: u64) -> Result<(Vec<Incoming>, String), String> {
        let mut batches = self.batches.lock().unwrap();
        let ready = match batches.front() {
            // All given: the bridge may stop.
            None => {
                self.stop.store(true, Ordering::SeqCst);
                false
            }
            Some((When::Now, _)) => true,
            Some((When::Typing(chat), _)) => {
                let asked = self.asked.lock().unwrap();
                asked.typing.iter().any(|(c, _)| c == chat)
            }
            Some((When::Drafted(chat), _)) => {
                let asked = self.asked.lock().unwrap();
                asked
                    .drafts
                    .iter()
                    .any(|(c, text)| c == chat && !text.is_empty())
            }
        };
        if ready {
            let (_, messages) = batches.pop_front().unwrap();
            return Ok((messages, cursor.to_string()));
        }
        drop(batches);
        std::thread::sleep(Duration::from_millis(5));
        Ok((Vec::new(), cursor.to_string()))
    }

    fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        let mut asked = self.asked.lock().unwrap();
        asked.tries += 1;
        if let Some(why) = self.failing.lock().unwrap().pop_front() {
            return Err(why);
        }
        asked
            .sent
            .push((chat.to_string(), text.to_string(), Instant::now()));
        Ok(())
    }

    fn long_polls(&self) -> bool {
        true
    }

    fn typist(&self, message: &Incoming, _aloud: bool) -> Option<Box<dyn Typist>> {
        Some(Box::new(Typing {
            chat: message.chat.clone(),
            asked: self.asked.clone(),
        }))
    }

    fn drafting(&self, message: &Incoming) -> Option<Drafting> {
        if !self.drafts {
            return None;
        }
        let (chat, asked) = (message.chat.clone(), self.asked.clone());
        Some(Drafting {
            open: Box::new(move || {
                Box::new(Drafted {
                    chat: chat.clone(),
                    asked: asked.clone(),
                })
            }),
            every: Duration::from_millis(30),
            most: None,
            keep: None,
            first: 0,
        })
    }

    fn acknowledge(&self, message: &Incoming, ack: Ack) -> Result<(), String> {
        self.asked
            .lock()
            .unwrap()
            .marks
            .push((message.id.clone(), ack));
        Ok(())
    }

    fn fetch_file(&self, note: &Note) -> Result<sion_cli::files::File, String> {
        self.held
            .get(&note.id)
            .cloned()
            .ok_or_else(|| "gone".to_string())
    }

    fn send_file(
        &self,
        message: &Incoming,
        file: &sion_cli::files::File,
        _caption: &str,
    ) -> Result<bool, String> {
        if !self.sends_files {
            return Ok(false);
        }
        self.asked.lock().unwrap().files.push((
            message.chat.clone(),
            file.name.clone(),
            file.media_type.clone(),
            file.bytes.len(),
        ));
        Ok(true)
    }
}

/// A message from `from` in `chat`, named `id`.
fn said(chat: &str, id: &str, from: &str, text: &str) -> Incoming {
    Incoming {
        id: id.into(),
        thread: String::new(),
        chat: chat.into(),
        from: from.into(),
        text: text.into(),
        voice: None,
        direct: true,
        mentions: Vec::new(),
        files: Vec::new(),
    }
}

/// A profile `name` of the agent, whose models play `scripts`, one each.
fn profile(home: &Path, name: &str, agent: ObjectId, scripts: &[Value]) -> Profile {
    let models = scripts
        .iter()
        .enumerate()
        .map(|(at, steps)| {
            let path = home.join(format!("{name}-{at}.json"));
            std::fs::write(&path, steps.to_string()).unwrap();
            format!("script:{}", path.display())
        })
        .collect();
    Profile {
        name: name.into(),
        wallet: "lead".into(),
        agent,
        models,
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
    }
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

/// Runs a bridge until `fake` has given all its batches and what they
/// asked is answered.
fn bridged(ledger: &SharedLedger, profile: &Profile, fake: &Fake, options: Options) {
    let options = Options {
        allow: if options.allow.is_empty() {
            vec![
                "c-burst",
                "c-steer",
                "c-stop",
                "c-queue",
                "c-collect",
                "c-int",
                "c-a",
                "c-b",
                "c-c",
                "c-d",
                "c-deliver",
                "c-restart",
                "c-talk",
                "c-files",
            ]
            .into_iter()
            .map(String::from)
            .collect()
        } else {
            options.allow
        },
        ..options
    };
    sion_cli::bridge::run_until(ledger.clone(), profile, fake, &options, &fake.stop).unwrap();
}

/// The session conversation `key` is in.
fn session_of(key: &str) -> sion_cli::runtime::Saved {
    let id = sion_cli::channels::state(key)
        .unwrap()
        .unwrap_or_else(|| panic!("no session for {key}"));
    sion_cli::runtime::Saved::load(&id).unwrap()
}

/// What the person said in a session, message by message.
fn their_words(saved: &sion_cli::runtime::Saved) -> Vec<String> {
    saved
        .state
        .messages
        .iter()
        .filter(|m| m.role == sion_agent::Role::User)
        .map(|m| m.text())
        .filter(|text| !text.is_empty())
        .collect()
}

/// A text of `words` words, the first "Once".
fn story(words: usize) -> String {
    let mut text = vec!["Once".to_string()];
    text.extend((1..words).map(|n| format!("w{n}")));
    text.join(" ") + "."
}

#[test]
fn a_bridge_answers_chats_side_by_side_and_as_they_go_on() {
    let home: PathBuf = std::env::temp_dir().join(format!("sion-bridging-{}", std::process::id()));
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

    // A burst is one turn: two messages, gathered, one answer.
    let burst = profile(
        &home,
        "burst",
        agent,
        &[json!([{"say": "Hello, Ann: 100 USD left."}])],
    );
    let fake = Fake::new(vec![(
        When::Now,
        vec![
            said("c-burst", "b1", "ann", "Hi."),
            said("c-burst", "b2", "ann", "How much budget is left?"),
        ],
    )]);
    bridged(
        &ledger,
        &burst,
        &fake,
        Options {
            debounce_ms: 300,
            ..Options::default()
        },
    );
    assert_eq!(fake.sent("c-burst"), ["Hello, Ann: 100 USD left."]);
    assert_eq!(
        their_words(&session_of("fake:burst:c-burst")),
        ["Hi.\nHow much budget is left?"]
    );

    // Steered: what is said while the agent works reaches it before its
    // next step, and is answered with the rest; each message is marked
    // taken, then answered.
    // Twelve looks at the budget, each a call of its own: the same call
    // giving the same result again and again would be taken for going round
    // in circles, and stopped.
    let mut steps: Vec<Value> = (0..12)
        .map(|look| json!({"call": "ledger_balance", "input": {"look": look}}))
        .collect();
    steps.push(json!({"when": "in euros", "say": "About 92 EUR left."}));
    steps.push(json!({"say": "100 USD left."}));
    let steer = profile(&home, "steer", agent, &[Value::Array(steps)]);
    let fake = Fake::new(vec![
        (
            When::Now,
            vec![said("c-steer", "s1", "ann", "How much budget is left?")],
        ),
        (
            When::Typing("c-steer"),
            vec![said("c-steer", "s2", "ann", "In euros, please.")],
        ),
    ]);
    bridged(
        &ledger,
        &steer,
        &fake,
        Options {
            busy: Busy::Steer,
            acknowledge: true,
            ..Options::default()
        },
    );
    assert_eq!(fake.sent("c-steer"), ["About 92 EUR left."]);
    let steered = session_of("fake:steer:c-steer");
    assert!(
        steered.state.messages.iter().any(|m| m
            .text()
            .contains("(The person said, while you worked:) In euros, please.")),
        "{:?}",
        steered.state.messages
    );
    assert_eq!(
        fake.asked.lock().unwrap().marks,
        [
            ("s1".to_string(), Ack::Seen),
            ("s2".to_string(), Ack::Seen),
            ("s1".to_string(), Ack::Done),
            ("s2".to_string(), Ack::Done),
        ]
    );

    // /stop, while the agent writes: it stops there, what it wrote kept.
    let told = story(30);
    let stopper = profile(
        &home,
        "stopper",
        agent,
        &[json!([{"say": told, "pace_ms": 100}])],
    );
    let fake = Fake::new(vec![
        (
            When::Now,
            vec![said("c-stop", "t1", "ann", "Tell me a story.")],
        ),
        (
            When::Drafted("c-stop"),
            vec![said("c-stop", "t2", "ann", "/stop")],
        ),
    ])
    .drafting();
    bridged(
        &ledger,
        &stopper,
        &fake,
        Options {
            busy: Busy::Steer,
            ..Options::default()
        },
    );
    let finished = fake.finished("c-stop");
    assert_eq!(finished.len(), 1, "{finished:?}");
    let (written, ending) = finished[0]
        .rsplit_once("\n\n")
        .expect("what it wrote, then that it stopped");
    assert_eq!(ending, "Stopped.");
    assert!(
        told.starts_with(written.trim_end()) && written.len() < told.len() && !written.is_empty(),
        "stopped before the end: {written:?}"
    );
    let stopped = session_of("fake:stopper:c-stop");
    let last = stopped.state.messages.last().unwrap();
    assert_eq!(last.role, sion_agent::Role::Assistant);
    assert!(told.starts_with(last.text().trim_end()));

    // Queued, what came meanwhile is answered after, one at a time;
    // collected, together.
    let slow = json!([
        {"say": "First answer, written slowly so that more comes meanwhile.", "pace_ms": 60},
        {"say": "Answer two."},
        {"say": "Answer three."}
    ]);
    for (name, chat, busy, answers, words) in [
        (
            "queue",
            "c-queue",
            Busy::Queue,
            vec!["Answer two.", "Answer three."],
            vec!["First?", "Second?", "Third?"],
        ),
        (
            "collect",
            "c-collect",
            Busy::Collect,
            vec!["Answer two."],
            vec!["First?", "Second?\nThird?"],
        ),
    ] {
        let queued = profile(&home, name, agent, std::slice::from_ref(&slow));
        let gate = if name == "queue" {
            When::Typing("c-queue")
        } else {
            When::Typing("c-collect")
        };
        let fake = Fake::new(vec![
            (When::Now, vec![said(chat, "q1", "ann", "First?")]),
            (
                gate,
                vec![
                    said(chat, "q2", "ann", "Second?"),
                    said(chat, "q3", "ann", "Third?"),
                ],
            ),
        ]);
        bridged(
            &ledger,
            &queued,
            &fake,
            Options {
                busy,
                ..Options::default()
            },
        );
        let mut expected = vec!["First answer, written slowly so that more comes meanwhile."];
        expected.extend(answers);
        assert_eq!(fake.sent(chat), expected, "{name}");
        assert_eq!(
            their_words(&session_of(&format!("fake:{name}:{chat}"))),
            words,
            "{name}"
        );
    }

    // Interrupted: the agent stops writing, and takes up what was said.
    let long = story(40);
    let interrupted = profile(
        &home,
        "interrupt",
        agent,
        &[json!([
            {"say": long, "pace_ms": 100},
            {"when": "just the total", "say": "100 USD."}
        ])],
    );
    let fake = Fake::new(vec![
        (
            When::Now,
            vec![said(
                "c-int",
                "i1",
                "ann",
                "Tell me everything about the budget.",
            )],
        ),
        (
            When::Drafted("c-int"),
            vec![said("c-int", "i2", "ann", "Actually, just the total.")],
        ),
    ])
    .drafting();
    bridged(
        &ledger,
        &interrupted,
        &fake,
        Options {
            busy: Busy::Interrupt,
            ..Options::default()
        },
    );
    let finished = fake.finished("c-int");
    assert_eq!(finished.len(), 2, "{finished:?}");
    assert!(
        long.starts_with(finished[0].trim_end()) && finished[0].len() < long.len(),
        "cut off, without a word of it: {:?}",
        finished[0]
    );
    assert_eq!(finished[1], "100 USD.");

    // Side by side: the second chat is at work before the first is
    // answered; one at a time, only after.
    for (parallel, first, second) in [(2, "c-a", "c-b"), (1, "c-c", "c-d")] {
        let side = profile(
            &home,
            &format!("side-{parallel}"),
            agent,
            &[json!([{"say": story(10), "pace_ms": 50}])],
        );
        let fake = Fake::new(vec![(
            When::Now,
            vec![
                said(first, "p1", "ann", "Hi."),
                said(second, "p2", "bo", "Hi."),
            ],
        )]);
        bridged(
            &ledger,
            &side,
            &fake,
            Options {
                parallel,
                ..Options::default()
            },
        );
        let before = fake.first_typing(second) < fake.first_sent(first);
        assert_eq!(before, parallel > 1, "parallel {parallel}");
    }

    // A reply that fails is sent again; one that fails each time waits in
    // the outbox, and goes once the messenger takes it; one refused is
    // not tried again, and its message is marked failed.
    let deliver = profile(
        &home,
        "deliver",
        agent,
        &[json!([
            {"say": "Sent on the third try."},
            {"say": "Kept for later."},
            {"say": "Refused."}
        ])],
    );
    let once = || Options {
        once: true,
        acknowledge: true,
        ..Options::default()
    };
    let fake = Fake::new(vec![(
        When::Now,
        vec![said("c-deliver", "d1", "ann", "One?")],
    )])
    .failing(&["HTTP 503: unavailable", "HTTP 503: unavailable"]);
    bridged(&ledger, &deliver, &fake, once());
    assert_eq!(fake.sent("c-deliver"), ["Sent on the third try."]);
    assert_eq!(fake.asked.lock().unwrap().tries, 3);
    let failing = ["HTTP 502: bad gateway"; sion_cli::bridge::ATTEMPTS as usize];
    let fake = Fake::new(vec![(
        When::Now,
        vec![said("c-deliver", "d2", "ann", "Two?")],
    )])
    .failing(&failing);
    bridged(&ledger, &deliver, &fake, once());
    assert!(fake.sent("c-deliver").is_empty());
    let outbox = sion_cli::channels::state("fake-cursor:deliver:outbox").unwrap();
    assert!(
        outbox
            .as_deref()
            .is_some_and(|kept| kept.contains("Kept for later.")),
        "{outbox:?}"
    );
    let fake = Fake::new(Vec::new());
    bridged(&ledger, &deliver, &fake, once());
    assert_eq!(fake.sent("c-deliver"), ["Kept for later."]);
    assert_eq!(
        sion_cli::channels::state("fake-cursor:deliver:outbox").unwrap(),
        None
    );
    let fake = Fake::new(vec![(
        When::Now,
        vec![said("c-deliver", "d3", "ann", "Three?")],
    )])
    .failing(&["HTTP 403: Forbidden"]);
    bridged(&ledger, &deliver, &fake, once());
    let asked = fake.asked.lock().unwrap();
    assert_eq!(asked.tries, 1, "not tried again");
    assert_eq!(asked.marks.last(), Some(&("d3".to_string(), Ack::Failed)));
    drop(asked);

    // What was taken before a restart is answered after it, and the chat
    // whose answer was cut off is told so.
    let restart = profile(
        &home,
        "restart",
        agent,
        &[json!([{"say": "100 USD left."}])],
    );
    sion_cli::channels::set_state(
        "fake-cursor:restart:pending",
        Some(
            &serde_json::to_string(&[said("c-restart", "r1", "ann", "How much is left?")]).unwrap(),
        ),
    )
    .unwrap();
    sion_cli::channels::set_state(
        "fake-cursor:restart:answering",
        Some(&json!(["fake:restart:c-restart"]).to_string()),
    )
    .unwrap();
    let fake = Fake::new(Vec::new());
    bridged(&ledger, &restart, &fake, once());
    assert_eq!(
        fake.sent("c-restart"),
        [sion_cli::bridge::RESTARTED, "100 USD left."]
    );
    for left in ["pending", "answering"] {
        assert_eq!(
            sion_cli::channels::state(&format!("fake-cursor:restart:{left}")).unwrap(),
            None
        );
    }

    // The commands, each a turn of its own: the session named, its usage,
    // its last answer again, undone, the model chosen by whoever may, the
    // sessions listed, who one is.
    let talk = profile(
        &home,
        "talk",
        agent,
        &[
            json!([{"say": "One."}, {"say": "Two."}, {"say": "Three."}]),
            json!([{"say": "From the second model."}]),
        ],
    );
    let second = talk.models[1].clone();
    let messages: Vec<Incoming> = [
        ("ann", "/help"),
        ("ann", "Hi"),
        ("ann", "/title Budget talk"),
        ("ann", "/usage"),
        ("ann", "/retry"),
        ("ann", "/undo"),
        ("ann", "Hello again"),
        ("ann", "/model 2"),
        ("boss", "/model 2"),
        ("ann", "Go on"),
        ("ann", "/sessions"),
        ("ann", "/whoami"),
        ("ann", "/new"),
        ("ann", "/compact"),
    ]
    .iter()
    .enumerate()
    .map(|(at, (from, text))| said("c-talk", &format!("k{at}"), from, text))
    .collect();
    let fake = Fake::new(vec![(When::Now, messages)]);
    bridged(
        &ledger,
        &talk,
        &fake,
        Options {
            once: true,
            approvers: vec!["boss".into()],
            ..Options::default()
        },
    );
    let replies = fake.sent("c-talk");
    assert_eq!(replies.len(), 14, "{replies:#?}");
    assert_eq!(replies[0], sion_cli::channels::COMMANDS);
    assert_eq!(replies[1], "One.");
    assert_eq!(replies[2], "The session is called Budget talk.");
    assert!(
        replies[3].starts_with("This session: 1 turns"),
        "{}",
        replies[3]
    );
    assert_eq!(replies[4], "Two.", "the last message answered again");
    assert_eq!(
        replies[5],
        "Undone: nothing is left, so the next message begins a new session."
    );
    assert_eq!(replies[6], "One.");
    assert_eq!(replies[7], "Only boss may choose the model here.");
    assert_eq!(replies[8], format!("This session uses {second} now."));
    assert_eq!(replies[9], "From the second model.");
    assert!(
        replies[10].starts_with("now: ") && replies[10].contains("Budget talk"),
        "{}",
        replies[10]
    );
    assert_eq!(
        replies[11],
        "You are ann on fake in chat c-talk, with talk. You may talk with it; only boss may approve or decline what it waits on, or choose its model."
    );
    assert_eq!(
        replies[12],
        "Starting over: the next message begins a new session."
    );
    assert_eq!(replies[13], "No session yet: send a task.");
    // Back to the first session, by the start of its id.
    let history: Vec<String> = serde_json::from_str(
        &sion_cli::channels::state("fake:talk:c-talk:history")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(history.len(), 2, "{history:?}");
    let fake = Fake::new(vec![(
        When::Now,
        vec![said(
            "c-talk",
            "k20",
            "ann",
            &format!("/resume {}", &history[0][..8]),
        )],
    )]);
    bridged(
        &ledger,
        &talk,
        &fake,
        Options {
            once: true,
            ..Options::default()
        },
    );
    assert!(
        fake.sent("c-talk")[0].starts_with("Back in session ")
            && fake.sent("c-talk")[0].contains("Budget talk"),
        "{:?}",
        fake.sent("c-talk")
    );
    assert_eq!(
        sion_cli::channels::state("fake:talk:c-talk").unwrap(),
        Some(history[0].clone())
    );

    // What a chat asks of the agent, and the tools it may use there, hold
    // for its sessions; another chat, through the same messenger, is
    // answered by another profile.
    let ops = profile(
        &home,
        "ops",
        agent,
        &[json!([
            {"say": "Bonjour."},
            {"call": "ledger_pay", "input": {"to": "bob", "amount": "1", "purpose": "a test"}},
            {"say": "I may not pay here."}
        ])],
    );
    let pay = profile(&home, "pay", agent, &[json!([{"say": "Pay here."}])]);
    let fake = Fake::new(vec![(
        When::Now,
        vec![
            said("c-ops", "o1", "ann", "/system Answer in French."),
            said("c-ops", "o2", "ann", "/tools ledger_balance"),
            said("c-ops", "o3", "ann", "Hello"),
            said("c-ops", "o4", "ann", "Pay Bob 1 USD."),
            said("c-pay", "o5", "ann", "Hi"),
            said("c-pay", "o6", "ann", "/whoami"),
        ],
    )]);
    bridged(
        &ledger,
        &ops,
        &fake,
        Options {
            once: true,
            allow: vec!["c-ops".into()],
            routes: [("c-pay".to_string(), pay)].into(),
            ..Options::default()
        },
    );
    assert_eq!(
        fake.sent("c-ops"),
        [
            "Asked: this session and the next ones here take it after the operator's instructions.",
            "Only these, of the tools the profile gives, here: ledger_balance.",
            "Bonjour.",
            "I may not pay here.",
        ]
    );
    let asked = session_of("fake:ops:c-ops");
    assert!(
        asked
            .prompt
            .contains("What the chat you answer in asks of you:\nAnswer in French."),
        "{}",
        asked.prompt
    );
    assert!(
        asked
            .state
            .messages
            .iter()
            .any(|m| m.content.iter().any(|b| matches!(
                b,
                sion_agent::Block::ToolResult { content, is_error: true, .. }
                    if content.contains("there is no tool named ledger_pay")
            ))),
        "a tool the chat did not give is not there"
    );
    assert_eq!(
        fake.sent("c-pay"),
        [
            "Pay here.",
            "You are ann on fake in chat c-pay, with pay. You may talk with it, approve or decline what it waits on, and choose its model.",
        ]
    );
    assert!(sion_cli::channels::state("fake:pay:c-pay")
        .unwrap()
        .is_some());

    // Files: a picture is shown to the agent and every file put in its
    // workspace, it told where; the picture it makes and the file it shares
    // go back after its reply.
    let png = sion_cli::files::File {
        media_type: "image/png".into(),
        name: "receipt.png".into(),
        bytes: b"\x89PNG\r\n\x1a\nreceipt".to_vec(),
    };
    let pdf = sion_cli::files::File {
        media_type: "application/pdf".into(),
        name: "report.pdf".into(),
        bytes: b"%PDF-1.7 report".to_vec(),
    };
    let made = b"\x89PNG\r\n\x1a\nchart".to_vec();
    let (images, asked_images) = {
        let made = made.clone();
        common::http_stand_in(move |_| {
            (
                200,
                json!({"data": [{"b64_json": sion_http::base64(&made)}], "output_format": "png"})
                    .to_string()
                    .into_bytes(),
            )
        })
    };
    std::env::set_var("OPENAI_BASE_URL", &images);
    std::env::set_var("OPENAI_API_KEY", "sk-test");
    // A workspace apart from the home, as the runtime asks.
    let workspace =
        std::env::temp_dir().join(format!("sion-bridging-files-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&workspace);
    std::fs::create_dir_all(&workspace).unwrap();
    let files = Profile {
        workspace: Some(workspace.clone()),
        image_model: Some("openai:gpt-image-1".into()),
        ..profile(
            &home,
            "files",
            agent,
            &[json!([
                {"call": "image_generate", "input": {"prompt": "a bar chart of the receipt", "name": "chart"}},
                {"call": "share_file", "input": {"path": "images/chart.png"}},
                {"call": "share_file", "input": {"path": "inbox/report.pdf"}},
                {"say": "Here is a chart of it, and your report back."}
            ])],
        )
    };
    let mut sent = said("c-files", "f1", "ann", "Chart this receipt, please.");
    sent.files = vec![
        Note {
            id: "n-receipt".into(),
            media_type: Some("image/png".into()),
            name: Some("receipt.png".into()),
        },
        Note {
            id: "n-report".into(),
            media_type: Some("application/pdf".into()),
            name: Some("report.pdf".into()),
        },
    ];
    let fake = Fake::new(vec![(When::Now, vec![sent])])
        .holding(&[("n-receipt", &png), ("n-report", &pdf)]);
    bridged(&ledger, &files, &fake, once());
    assert_eq!(
        fake.sent("c-files"),
        ["Here is a chart of it, and your report back."]
    );
    assert_eq!(
        fake.asked.lock().unwrap().files,
        [
            (
                "c-files".to_string(),
                "chart.png".to_string(),
                "image/png".to_string(),
                made.len()
            ),
            (
                "c-files".to_string(),
                "report.pdf".to_string(),
                "application/pdf".to_string(),
                pdf.bytes.len()
            ),
        ]
    );
    assert_eq!(
        std::fs::read(workspace.join("inbox/receipt.png")).unwrap(),
        png.bytes
    );
    assert_eq!(
        std::fs::read(workspace.join("inbox/report.pdf")).unwrap(),
        pdf.bytes
    );
    assert_eq!(
        std::fs::read(workspace.join("images/chart.png")).unwrap(),
        made
    );
    let asked = session_of("fake:files:c-files");
    let first = &asked.state.messages[0];
    assert!(
        first.content.iter().any(|b| matches!(
            b,
            sion_agent::Block::Image { image } if image.media_type == "image/png"
        )),
        "the picture is shown: {first:?}"
    );
    assert_eq!(
        first.text(),
        format!(
            "Chart this receipt, please.\n(A picture was sent, put in your workspace as inbox/receipt.png: image/png, {} bytes.)\n(A file was sent, put in your workspace as inbox/report.pdf: application/pdf, {} bytes.)",
            png.bytes.len(),
            pdf.bytes.len()
        )
    );
    let generated = common::sent_to(&asked_images, "POST", "/images/generations");
    assert_eq!(generated.len(), 1);
    let generated = generated[0].json();
    assert_eq!(
        (generated["model"].as_str(), generated["prompt"].as_str()),
        (Some("gpt-image-1"), Some("a bar chart of the receipt"))
    );
    // Where the messenger sends no files, the person is told; a file that
    // cannot be fetched, the agent is told.
    let again = Profile {
        workspace: Some(workspace.clone()),
        ..profile(
            &home,
            "files-again",
            agent,
            &[json!([
                {"call": "share_file", "input": {"path": "inbox/report.pdf"}},
                {"say": "Sent."}
            ])],
        )
    };
    let mut sent = said("c-files", "f2", "ann", "And the report?");
    sent.files = vec![Note {
        id: "n-gone".into(),
        media_type: None,
        name: None,
    }];
    let fake = Fake::new(vec![(When::Now, vec![sent])]).sending_no_files();
    bridged(&ledger, &again, &fake, once());
    assert_eq!(
        fake.sent("c-files"),
        [
            "Sent.",
            "(I shared report.pdf with you, but fake does not take files from me.)"
        ]
    );
    assert_eq!(
        their_words(&session_of("fake:files-again:c-files")),
        ["And the report?\n(A file was sent that could not be fetched: gone.)"]
    );

    // A2A: the card other agents read, and a task in each wire version,
    // its conversation going on in its context; streamed, the reply grows.
    let a2a = profile(
        &home,
        "a2a",
        agent,
        &[json!([
            {"say": "85 USD left."},
            {"say": "Still 85 USD."},
            {"say": "Streamed, 85 USD."}
        ])],
    );
    let mut profiles = sion_cli::profiles::all().unwrap();
    profiles.insert("a2a".into(), a2a.clone());
    sion_cli::profiles::save_all(&profiles).unwrap();
    sion_cli::a2a::publish(sion_cli::a2a::Published {
        profiles: vec!["a2a".into()],
        base: Some("https://agents.example.org".into()),
        ..Default::default()
    });
    let listen = {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("127.0.0.1:{}", probe.local_addr().unwrap().port())
    };
    let admin = "a2a-admin-token-0123456789";
    let serving = {
        let (ledger, listen) = (ledger.clone(), listen.clone());
        std::thread::spawn(move || {
            sion_cli::channels::serve_with(ledger, &listen, Some(admin), Some(6)).unwrap()
        })
    };
    let client = sion_http::Client::new(sion_http::Options {
        allow_private: true,
        proxy_from_env: false,
        timeout: Duration::from_secs(120),
        ..sion_http::Options::default()
    })
    .unwrap();
    let get = |path: &str| -> Value {
        for _ in 0..100 {
            if let Ok(fetched) = client.get(&format!("http://{listen}{path}"), 1 << 20) {
                return serde_json::from_slice(&fetched.body).unwrap();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the channel did not answer");
    };
    let card = get("/.well-known/agent-card.json");
    assert_eq!(card["name"], "a2a");
    assert_eq!(
        card["url"],
        "https://agents.example.org/v1/profiles/a2a/a2a"
    );
    assert_eq!(card["supportedInterfaces"][0]["protocolVersion"], "1.0");
    assert_eq!(card["capabilities"]["streaming"], true);
    let auth = format!("Bearer {admin}");
    let rpc = |body: Value, version: Option<&str>| -> Vec<u8> {
        let mut headers = vec![
            ("Authorization", auth.as_str()),
            ("Content-Type", "application/json"),
        ];
        if let Some(version) = version {
            headers.push(("A2A-Version", version));
        }
        client
            .request(
                "POST",
                &format!("http://{listen}/v1/profiles/a2a/a2a"),
                &headers,
                Some(body.to_string().as_bytes()),
                1 << 20,
            )
            .unwrap()
            .body
    };
    let sent: Value = serde_json::from_slice(&rpc(
        json!({"jsonrpc": "2.0", "id": 1, "method": "message/send", "params": {"message": {
            "kind": "message", "role": "user", "messageId": "m1", "contextId": "ctx-1",
            "parts": [{"kind": "text", "text": "How much budget is left?"}]}}}),
        None,
    ))
    .unwrap();
    let task = &sent["result"];
    assert_eq!(
        (
            task["kind"].as_str(),
            task["contextId"].as_str(),
            task["status"]["state"].as_str()
        ),
        (Some("task"), Some("ctx-1"), Some("completed")),
        "{sent}"
    );
    assert_eq!(task["artifacts"][0]["parts"][0]["text"], "85 USD left.");
    let again: Value = serde_json::from_slice(&rpc(
        json!({"jsonrpc": "2.0", "id": 2, "method": "SendMessage", "params": {"message": {
            "role": "ROLE_USER", "messageId": "m2", "contextId": "ctx-1",
            "parts": [{"text": "And now?"}]}}}),
        Some("1.0"),
    ))
    .unwrap();
    let task = &again["result"]["task"];
    assert_eq!(task["status"]["state"], "TASK_STATE_COMPLETED", "{again}");
    assert_eq!(task["artifacts"][0]["parts"][0]["text"], "Still 85 USD.");
    let asked = session_of("a2a:a2a:ctx-1");
    assert_eq!(
        their_words(&asked),
        ["How much budget is left?", "And now?"]
    );
    // Another agent's messages are content from outside: the session is
    // tainted, so its external calls and payments would wait for a person.
    assert!(asked.state.standing.tainted_until.is_some());
    let found: Value = serde_json::from_slice(&rpc(
        json!({"jsonrpc": "2.0", "id": 3, "method": "GetTask", "params": {"id": task["id"]}}),
        None,
    ))
    .unwrap();
    assert_eq!(found["result"]["id"], task["id"]);
    let missing: Value = serde_json::from_slice(&rpc(
        json!({"jsonrpc": "2.0", "id": 4, "method": "tasks/get", "params": {"id": "nope"}}),
        None,
    ))
    .unwrap();
    assert_eq!(missing["error"]["code"], -32001);
    let streamed = String::from_utf8(rpc(
        json!({"jsonrpc": "2.0", "id": 5, "method": "message/stream", "params": {"message": {
            "kind": "message", "role": "user", "messageId": "m3",
            "parts": [{"kind": "text", "text": "Stream it."}]}}}),
        None,
    ))
    .unwrap();
    let events: Vec<Value> = streamed
        .lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(events[0]["result"]["kind"], "task");
    assert!(events
        .iter()
        .any(|e| e["result"]["kind"] == "artifact-update"));
    let last = events.last().unwrap();
    assert_eq!(
        (
            last["result"]["kind"].as_str(),
            last["result"]["final"].as_bool()
        ),
        (Some("status-update"), Some(true))
    );
    serving.join().unwrap();

    // In the terminal: what is typed, answered as it is written; by voice,
    // recorded, heard and spoken, through programs given the files.
    let talker = profile(
        &home,
        "talker",
        agent,
        &[json!([{"say": "85 USD left."}, {"say": "Spoken: 85 USD."}])],
    );
    let mut out = Vec::new();
    sion_cli::talk::run(
        ledger.clone(),
        &talker,
        &sion_cli::talk::Talk {
            conversation: "desk".into(),
            ..Default::default()
        },
        &mut "How much budget is left?\n".as_bytes(),
        &mut out,
    )
    .unwrap();
    let shown = String::from_utf8(out).unwrap();
    assert!(shown.contains("you> agent> 85 USD left.\n"), "{shown:?}");
    let played = home.join("played");
    let sh = |script: &str| vec!["sh".to_string(), "-c".to_string(), script.to_string()];
    let mut out = Vec::new();
    sion_cli::talk::run(
        ledger.clone(),
        &talker,
        &sion_cli::talk::Talk {
            conversation: "desk".into(),
            hearing: Some(sion_cli::voice::Hearing::Command(sh("echo Say it aloud."))),
            speaking: Some(sion_cli::voice::Speaking::Command(sh("printf OggS; cat"))),
            record: sh("printf 'RIFF....WAVE' > \"$0\""),
            play: sh(&format!("cp \"$0\" {}", played.display())),
        },
        &mut "\n".as_bytes(),
        &mut out,
    )
    .unwrap();
    let shown = String::from_utf8(out).unwrap();
    assert!(shown.contains("you (heard): Say it aloud."), "{shown:?}");
    assert!(shown.contains("agent> Spoken: 85 USD."), "{shown:?}");
    assert_eq!(std::fs::read(&played).unwrap(), b"OggSSpoken: 85 USD.");

    drop(devnet);
    std::fs::remove_dir_all(&workspace).unwrap();
    std::fs::remove_dir_all(&home).unwrap();
}
