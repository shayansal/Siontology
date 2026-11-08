//! An agent takes work on by itself, on a devnet, against a stand-in for
//! GitHub's API. In observe mode, what it would have taken on is only
//! recorded; in auto mode it takes on what a rule covers, and what people
//! ask it for. Each piece of work has a mandate kept on the ledger, which
//! its payments name as their evidence. It says once that it took the work
//! on, asks one question at a time, says nothing while it works, and posts
//! its result. A person's request runs before work nobody asked for, in
//! the slot kept for people. A person stops one item; a change of the
//! rules revokes another. In a Telegram group, what is said to the agent
//! starts work and goes to it, and the chatter between goes to no one.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use sion_agent::work::{How, Mode, Stage, Strength};
use sion_agent::Autonomy;
use sion_cli::ledger::LedgerExt;
use sion_cli::profiles::Profile;
use sion_cli::runtime::SharedLedger;
use sion_cli::session::Session;
use sion_cli::work::{self, RunOptions};
use sion_core::ids::{AccountId, AssetId, ClaimId, ObjectId, Pulse, TxDigest};
use sion_core::Amount;
use sion_exec::charter::Caveats;
use sion_exec::claim::Payload;
use sion_exec::execute::Event;
use sion_exec::TxKind;
use sion_node::{Devnet, DevnetConfig};

/// What the stand-in for GitHub holds.
#[derive(Default)]
struct Hub {
    issues: Vec<Value>,
    comments: Vec<Value>,
    /// Review comments on pull requests' code.
    reviews: Vec<Value>,
    /// The agent's replies in review threads: the thread's first comment,
    /// and what.
    replies: Vec<(u64, String)>,
    /// The check runs of the pull request's commit.
    checks: Vec<Value>,
    /// The comments the agent posted: on which issue, and what.
    posted: Vec<(u64, String)>,
    /// The latest time anything was said, as GitHub writes times.
    latest: String,
    next_comment: u64,
}

impl Hub {
    fn issue(&mut self, number: u64, by: &str, title: &str, body: &str, at: u64) {
        let at = sion_cli::github::rfc3339(at);
        self.issues.push(json!({
            "number": number, "title": title, "body": body,
            "user": {"login": by, "type": "User"},
            "html_url": format!("https://github.example/acme/app/issues/{number}"),
            "created_at": at, "updated_at": at, "assignees": []
        }));
        self.latest = at;
    }

    fn comment(&mut self, number: u64, by: &str, body: &str, at: u64) {
        let at = sion_cli::github::rfc3339(at);
        self.next_comment += 1;
        self.comments.push(json!({
            "id": self.next_comment, "body": body,
            "user": {"login": by, "type": "User"},
            "issue_url": format!("https://api.github.example/repos/acme/app/issues/{number}"),
            "html_url": format!("https://github.example/acme/app/issues/{number}#c{}", self.next_comment),
            "created_at": at, "updated_at": at
        }));
        self.latest = at;
    }

    fn posted_on(&self, number: u64) -> Vec<String> {
        self.posted
            .iter()
            .filter(|(n, _)| *n == number)
            .map(|(_, text)| text.clone())
            .collect()
    }
}

/// Serves GitHub's API from `hub` until `done`.
fn serve(hub: Arc<Mutex<Hub>>, done: Arc<std::sync::atomic::AtomicBool>) -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    let here = base.clone();
    std::thread::spawn(move || {
        while !done.load(std::sync::atomic::Ordering::SeqCst) {
            let Ok(Some(mut request)) = server.recv_timeout(std::time::Duration::from_millis(20))
            else {
                continue;
            };
            let authorized = request
                .headers()
                .iter()
                .any(|h| h.field.equiv("Authorization") && h.value.as_str() == "Bearer hub-token");
            let mut body = String::new();
            std::io::Read::read_to_string(request.as_reader(), &mut body).unwrap();
            let url = request.url().to_string();
            let path = url.split('?').next().unwrap_or_default().to_string();
            let mut hub = hub.lock().unwrap();
            // A job's log: the API sends the client to a signed address,
            // which takes no token.
            if path == "/repos/acme/app/actions/jobs/77/logs" && authorized {
                let moved = tiny_http::Response::from_string("")
                    .with_status_code(302)
                    .with_header(
                        format!("Location: {here}/signed/logs/77")
                            .parse::<tiny_http::Header>()
                            .unwrap(),
                    );
                request.respond(moved).unwrap();
                continue;
            }
            if path == "/signed/logs/77" {
                assert!(!authorized, "the token does not go to the log's address");
                let log = "2026-11-03T01:02:03.1234567Z running 1 test\n2026-11-03T01:02:04.1234567Z test login::rename ... FAILED\n2026-11-03T01:02:05.1234567Z ##[error]Process completed with exit code 101.\n";
                request
                    .respond(tiny_http::Response::from_string(log))
                    .unwrap();
                continue;
            }
            let (status, answer) = if !authorized {
                (401, json!({"message": "Bad credentials"}))
            } else {
                match (request.method().as_str(), path.as_str()) {
                    ("GET", "/user") => (200, json!({"login": "sion-helper"})),
                    ("GET", "/repos/acme/app/issues") => (200, Value::from(hub.issues.clone())),
                    ("GET", "/repos/acme/app/issues/comments") => {
                        (200, Value::from(hub.comments.clone()))
                    }
                    ("GET", "/repos/acme/app/pulls/comments") => {
                        (200, Value::from(hub.reviews.clone()))
                    }
                    ("GET", "/repos/acme/app/pulls/20") => (
                        200,
                        json!({"number": 20, "title": "Read the renamed cookie", "state": "open",
                               "user": {"login": "dana"}, "body": "Fixes the login.",
                               "head": {"label": "dana:cookie", "sha": "abc1234567"},
                               "base": {"ref": "main"}, "mergeable_state": "clean"}),
                    ),
                    ("GET", "/repos/acme/app/pulls/20/files") => (
                        200,
                        json!([{"filename": "src/login.rs", "status": "modified", "additions": 2, "deletions": 1,
                                "patch": "@@ -1,3 +1,4 @@\n-let name = \"sid\";\n+let name = \"session\";\n+let old = \"sid\";"}]),
                    ),
                    ("GET", "/repos/acme/app/commits/abc1234567/check-runs") => {
                        (200, json!({"check_runs": hub.checks.clone()}))
                    }
                    ("GET", "/repos/acme/app/commits/abc1234567/status") => {
                        (200, json!({"statuses": []}))
                    }
                    ("GET", "/repos/acme/app/check-runs/77/annotations") => (
                        200,
                        json!([{"path": "src/login.rs", "start_line": 3, "annotation_level": "failure",
                                "message": "assertion failed: signed_in(old_cookie)"}]),
                    ),
                    ("POST", "/repos/acme/app/pulls/20/comments/300/replies") => {
                        let text = serde_json::from_str::<Value>(&body).unwrap()["body"]
                            .as_str()
                            .unwrap()
                            .to_string();
                        hub.replies.push((300, text));
                        (201, json!({"id": 301}))
                    }
                    ("GET", other) if other.starts_with("/repos/acme/app/issues/") => {
                        let number: u64 = other.rsplit('/').next().unwrap().parse().unwrap();
                        match hub.issues.iter().find(|i| i["number"] == number) {
                            Some(issue) => (200, issue.clone()),
                            None => (404, json!({"message": "Not Found"})),
                        }
                    }
                    ("POST", other) if other.ends_with("/comments") => {
                        let number: u64 = other.split('/').nth(5).unwrap().parse().unwrap();
                        let text = serde_json::from_str::<Value>(&body).unwrap()["body"]
                            .as_str()
                            .unwrap()
                            .to_string();
                        hub.posted.push((number, text.clone()));
                        // GitHub lists the agent's own comments with the rest.
                        let latest = hub.latest.clone();
                        hub.next_comment += 1;
                        let id = hub.next_comment;
                        hub.comments.push(json!({
                            "id": id, "body": text,
                            "user": {"login": "sion-helper", "type": "User"},
                            "issue_url": format!("https://api.github.example/repos/acme/app/issues/{number}"),
                            "created_at": latest, "updated_at": latest
                        }));
                        (201, json!({"id": id}))
                    }
                    _ => (404, json!({"message": "Not Found"})),
                }
            };
            request
                .respond(
                    tiny_http::Response::from_string(answer.to_string()).with_status_code(status),
                )
                .unwrap();
        }
    });
    base
}

/// What the stand-in for Telegram's Bot API holds: the updates it gives,
/// and the messages the bot sent.
#[derive(Default)]
struct Bot {
    updates: Vec<Value>,
    sent: Vec<Value>,
}

/// Serves Telegram's Bot API from `bot` until `done`.
fn serve_telegram(bot: Arc<Mutex<Bot>>, done: Arc<std::sync::atomic::AtomicBool>) -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    std::thread::spawn(move || {
        while !done.load(std::sync::atomic::Ordering::SeqCst) {
            let Ok(Some(mut request)) = server.recv_timeout(std::time::Duration::from_millis(20))
            else {
                continue;
            };
            let mut body = String::new();
            std::io::Read::read_to_string(request.as_reader(), &mut body).unwrap();
            let url = request.url().to_string();
            assert!(url.starts_with("/botbot-token/"), "{url}");
            let asked: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            let mut bot = bot.lock().unwrap();
            let result = match url.rsplit('/').next().unwrap_or_default() {
                "getMe" => json!({"id": 999, "is_bot": true, "username": "sion_helper_bot"}),
                "getUpdates" => {
                    let offset = asked["offset"].as_i64().unwrap_or(0);
                    Value::from(
                        bot.updates
                            .iter()
                            .filter(|u| u["update_id"].as_i64().unwrap_or(0) >= offset)
                            .cloned()
                            .collect::<Vec<_>>(),
                    )
                }
                "sendMessage" => {
                    bot.sent.push(asked.clone());
                    json!({"message_id": 500 + bot.sent.len(), "chat": {"id": asked["chat_id"]}})
                }
                _ => json!(true),
            };
            request
                .respond(tiny_http::Response::from_string(
                    json!({"ok": true, "result": result}).to_string(),
                ))
                .unwrap();
        }
    });
    base
}

fn script(home: &std::path::Path, name: &str, steps: Value) -> String {
    let path = home.join(name);
    std::fs::write(&path, steps.to_string()).unwrap();
    format!("script:{}", path.display())
}

fn register(session: &mut Session<'_>, ledger: &SharedLedger, wallet: &str) -> ObjectId {
    let pulse = ledger.as_ref().pulse().unwrap();
    let account = session.id(wallet);
    let view = session
        .must(
            "dana",
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
    session
        .fund(wallet, AssetId::usd(), Amount::units(5))
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

fn claim_schema(ledger: &SharedLedger, id: &str) -> String {
    let id = ClaimId::from_hash(sion_core::hash::Hash32::from_hex(id).unwrap());
    let record = ledger.as_ref().claim(id).unwrap();
    record["claim"]["body"]["schema"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[test]
fn an_agent_takes_work_on_under_mandates_and_spends_attention_sparingly() {
    let home = std::env::temp_dir().join(format!("sion-agent-takes-work-{}", std::process::id()));
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
    for (name, amount) in [("dana", 1_000), ("bob", 1)] {
        session
            .fund(name, AssetId::usd(), Amount::units(amount))
            .unwrap();
    }
    let agent = register(&mut session, &ledger, "helper");

    // The agent: one script for every item, each step by the item it is
    // working on; and a model that weighs what nobody asked for.
    let helper = script(
        &home,
        "helper.json",
        json!([
            {"when": "work item w2:", "call": "work_ask",
             "input": {"question": "Which branch should I fix?", "choices": ["main", "release-2.1"]}},
            {"when": "work item w2:", "say": "I asked which branch to fix."},
            {"when": "dana wrote:\n> main", "call": "ledger_pay",
             "input": {"to": "bob", "amount": "2", "purpose": "a test account for the login fix"}},
            {"when": "dana wrote:\n> main", "call": "work_report",
             "input": {"state": "ready", "summary": "Fixed on main: the session check failed after the cookie rename. The fix is in a8f3c2d."}},
            {"when": "dana wrote:\n> main", "say": "Done."},
            {"when": "work item w1:", "call": "work_report",
             "input": {"state": "progress", "summary": "Reproduced on a cold cache."}},
            {"when": "work item w1:", "call": "work_report",
             "input": {"state": "ready", "summary": "Reproduced: signup takes 19.8 s with a cold cache. The slow step is the email check, which calls the mail server once per signup."}},
            {"when": "work item w1:", "say": "Reported."},
            {"when": "work item w3:", "call": "work_report",
             "input": {"state": "progress", "summary": "Looking at the deploy test."}},
            {"when": "work item w3:", "call": "work_report",
             "input": {"state": "blocked", "summary": "I need read access to the deploy logs."}},
            {"when": "work item w3:", "say": "Blocked."},
            {"when": "work item w3:", "call": "work_report",
             "input": {"state": "blocked", "summary": "I need read access to the deploy logs."}},
            {"when": "work item w3:", "say": "Still blocked."},
            {"when": "work item w4:", "call": "work_ask",
             "input": {"question": "Is the search slow on mobile only?"}},
            {"when": "work item w4:", "say": "I asked."},
            {"when": "work item w5:", "unless": "4242 wrote:\n> november", "call": "work_ask",
             "input": {"question": "Which month?", "choices": ["October", "November"]}},
            {"when": "work item w5:", "unless": "4242 wrote:\n> november", "say": "I asked which month."},
            {"when": "4242 wrote:\n> november", "call": "work_report",
             "input": {"state": "ready", "summary": "In November I spent 2 USD, on a test account for the login fix."}},
            {"when": "4242 wrote:\n> november", "say": "Reported."},
            {"when": "work item w6:", "unless": "ended in failure", "call": "github_pull_request", "input": {"number": 20}},
            {"when": "work item w6:", "unless": "ended in failure", "call": "work_report",
             "input": {"state": "waiting", "summary": "Waiting for the checks."}},
            {"when": "work item w6:", "unless": "ended in failure", "say": "I'll look again when the checks end."},
            {"when": "ended in failure", "call": "github_checks", "input": {"pull": 20}},
            {"when": "ended in failure", "call": "github_job_log", "input": {"job": 77}},
            {"when": "ended in failure", "call": "work_report",
             "input": {"state": "ready", "summary": "The check test fails: login::rename finds no session for the old cookie. Reading both names for a release fixes it."}},
            {"when": "ended in failure", "say": "Reported."},
            {"when": "work item w7:", "call": "work_report",
             "input": {"state": "ready", "summary": "Both names are read for one release, so people signed in before the rename stay signed in."}},
            {"when": "work item w7:", "say": "Answered."}
        ]),
    );
    let accept = |title: &str| {
        json!({"accept": true, "rules": ["triage"], "title": title, "why": "a bug report"})
            .to_string()
    };
    let assess = script(
        &home,
        "assess.json",
        json!([
            {"when": "signup", "say": accept("Reproduce the slow signup")},
            {"when": "search page", "say": accept("Reproduce the slow search")},
            {"when": "login", "say": accept("Reproduce the login failure")},
            {"say": json!({"accept": false, "rules": [], "title": "", "why": "not a bug report"}).to_string()}
        ]),
    );
    let profile = Profile {
        name: "helper".into(),
        wallet: "helper".into(),
        agent,
        models: vec![helper],
        autonomy: Autonomy::L6,
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
        no_plan: false,
        no_change_check: false,
        hear: None,
        rules: None,
        rules_model: None,
        always_ask: Vec::new(),
        quarantine: false,
        shared_memory: Vec::new(),
        confidential: false,
        confidential_files: Vec::new(),
        routes: Vec::new(),
        sorter: None,
    };
    sion_cli::profiles::save_all(&[("helper".to_string(), profile)].into_iter().collect()).unwrap();

    // The repository, watched: dana may ask the agent for work there.
    let hub = Arc::new(Mutex::new(Hub::default()));
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let base = serve(hub.clone(), done.clone());
    std::env::set_var("GH_TOKEN", "hub-token");
    std::env::set_var("SION_GITHUB_API", &base);
    work::run_local(work::WorkCommand::Watch {
        profile: "helper".into(),
        watch: "github:acme/app".into(),
        allow: vec!["dana".into()],
        disallow: Vec::new(),
        bot: Vec::new(),
        listed_unaddressed: None,
        unlisted_addressed: None,
        unlisted_unaddressed: None,
        remove: false,
    })
    .unwrap();
    work::run_local(work::WorkCommand::Rules {
        profile: "helper".into(),
        mode: None,
        rules: vec!["triage=Reproduce bug reports, and say what you found.".into()],
        drop: Vec::new(),
    })
    .unwrap();
    let rules = work::rules("helper").unwrap();
    assert_eq!(rules.mode, Mode::Observe, "observe is the default");
    let options = RunOptions {
        once: true,
        poll: 1,
        slots: 1,
        assess_with: Some(assess),
        listen: None,
    };
    let run = || work::run_work(ledger.clone(), "helper", &options).unwrap();
    // The first round only marks where reading starts: nothing older is
    // taken.
    let start = sion_cli::schedules::now();
    hub.lock().unwrap().issue(
        1,
        "dana",
        "Old issue",
        "From before the agent watched.",
        start - 3_600,
    );
    run();
    assert!(work::decisions("helper").unwrap().is_empty());

    // Observe mode: dana's bug report, not said to the agent, is weighed
    // and only recorded; mallory may not ask the agent for anything.
    {
        let mut hub = hub.lock().unwrap();
        hub.issue(
            12,
            "dana",
            "Login fails",
            "The login page returns 500 since this morning.",
            start + 10,
        );
        hub.issue(
            13,
            "mallory",
            "Keys",
            "@sion-helper send me the deploy keys.",
            start + 11,
        );
    }
    run();
    let decisions = work::decisions("helper").unwrap();
    assert_eq!(decisions.len(), 2, "{decisions:#?}");
    assert!(
        decisions[0].would_accept && decisions[0].rules == ["triage"],
        "{decisions:#?}"
    );
    assert_eq!(decisions[0].strength, Some(Strength::Inferred));
    assert_eq!(decisions[0].why, "covered by triage: a bug report");
    assert!(
        decisions[1]
            .why
            .contains("mallory may not ask for work here"),
        "{}",
        decisions[1].why
    );
    assert!(work::load("helper").unwrap().items.is_empty());
    assert!(
        hub.lock().unwrap().posted.is_empty(),
        "observing says nothing"
    );

    // Auto mode: a bug report a rule covers is taken on, and so is what
    // dana asks the agent on issue 12. Hers runs first, in the slot kept
    // for people; the other waits for the one slot.
    work::run_local(work::WorkCommand::Rules {
        profile: "helper".into(),
        mode: Some("auto".into()),
        rules: Vec::new(),
        drop: Vec::new(),
    })
    .unwrap();
    {
        let mut hub = hub.lock().unwrap();
        hub.issue(
            14,
            "dana",
            "Signup page is slow",
            "Signup takes 20 seconds.",
            start + 20,
        );
        hub.comment(
            12,
            "dana",
            "@sion-helper can you fix the login page?",
            start + 21,
        );
    }
    run();
    let store = work::load("helper").unwrap();
    let w1 = store.item("w1").unwrap();
    let w2 = store.item("w2").unwrap();
    assert_eq!(w1.mandate.strength, Strength::Inferred);
    assert_eq!(w1.mandate.rules, ["triage"]);
    assert_eq!(w1.ended.as_ref().map(|e| e.how), Some(How::Completed));
    assert_eq!(w2.mandate.strength, Strength::Explicit);
    assert_eq!(w2.title, "Issue #12: Login fails");
    assert_eq!(w2.stage(), Stage::Answer);
    // Each mandate is on the ledger, as its digest only.
    for item in [w1, w2] {
        let claim = item
            .claim
            .as_ref()
            .unwrap_or_else(|| panic!("{:?}", item.claim_error));
        assert_eq!(claim_schema(&ledger, claim), "work.mandate/v1");
        let id = ClaimId::from_hash(sion_core::hash::Hash32::from_hex(claim).unwrap());
        let record = ledger.as_ref().claim(id).unwrap();
        let payload: Payload =
            serde_json::from_value(record["claim"]["body"]["payload"].clone()).unwrap();
        assert_eq!(payload, Payload::Commitment(item.mandate.digest()));
    }
    {
        let hub = hub.lock().unwrap();
        // Said once that it took the work on, and why, for what nobody
        // asked for; its progress said to no one; then its result.
        assert_eq!(
            hub.posted_on(14),
            [
                "I'm taking this on under my operator's rule triage: Reproduce the slow signup. I'll write here when it's ready, or if I need a decision. Say stop to stop me.",
                "Reproduced: signup takes 19.8 s with a cold cache. The slow step is the email check, which calls the mail server once per signup."
            ]
        );
        // On issue 12: it is on it, and asks one question.
        assert_eq!(
            hub.posted_on(12),
            [
                "On it: Issue #12: Login fails. I'll write here when it's ready, or if I need a decision.",
                "Which branch should I fix? (main / release-2.1)"
            ]
        );
    }
    // One list shows what waits for a person.
    let waiting = work::waiting().unwrap();
    assert_eq!(waiting.len(), 1, "{waiting:#?}");
    assert!(
        waiting[0].starts_with("work helper/w2: waiting for an answer (Issue #12: Login fails). It asks: Which branch should I fix?"),
        "{}",
        waiting[0]
    );

    // dana answers on the thread; the agent pays for what the fix needs,
    // naming the mandate, and posts its result. Two more requests come:
    // one asked for, one not.
    {
        let mut hub = hub.lock().unwrap();
        hub.comment(12, "dana", "main", start + 30);
        hub.issue(
            15,
            "dana",
            "Deploy test is flaky",
            "@sion-helper please look at the flaky deploy test.",
            start + 31,
        );
        hub.issue(
            16,
            "dana",
            "Search page is slow",
            "The search page takes 9 seconds to load.",
            start + 32,
        );
    }
    run();
    let store = work::load("helper").unwrap();
    let w2 = store.item("w2").unwrap();
    assert_eq!(
        w2.ended.as_ref().map(|e| e.how),
        Some(How::Completed),
        "{w2:#?}"
    );
    assert_eq!(
        hub.lock().unwrap().posted_on(12)[2],
        "Fixed on main: the session check failed after the cookie rename. The fix is in a8f3c2d."
    );
    session.wait_pulses(1).unwrap();
    let mandate =
        ClaimId::from_hash(sion_core::hash::Hash32::from_hex(w2.claim.as_ref().unwrap()).unwrap());
    let receipts: Vec<ClaimId> = ledger
        .as_ref()
        .events(Pulse(0), 100_000)
        .unwrap()
        .into_iter()
        .filter_map(|e| match e.event {
            Event::AgentPaid {
                charter, receipt, ..
            } if charter == agent => receipt,
            _ => None,
        })
        .collect();
    assert_eq!(receipts.len(), 1, "one payment");
    let record = ledger.as_ref().claim(receipts[0]).unwrap();
    let payload: Payload =
        serde_json::from_value(record["claim"]["body"]["payload"].clone()).unwrap();
    let Payload::Public(bytes) = payload else {
        panic!("a receipt's payload is public");
    };
    let (_, _, amount, _, evidence, _): (
        ObjectId,
        AccountId,
        Amount,
        String,
        Option<ClaimId>,
        TxDigest,
    ) = sion_core::encode::decode(&bytes).unwrap();
    assert_eq!(amount, Amount::units(2));
    assert_eq!(
        evidence,
        Some(mandate),
        "the payment names the work's mandate"
    );
    let w3 = store.item("w3").unwrap();
    let w4 = store.item("w4").unwrap();
    assert_eq!(w3.stage(), Stage::Blocked, "{w3:#?}");
    assert_eq!(w4.stage(), Stage::Answer, "{w4:#?}");
    {
        let hub = hub.lock().unwrap();
        assert_eq!(
            hub.posted_on(15)[1..],
            ["I need read access to the deploy logs."],
            "its progress said to no one"
        );
        assert_eq!(hub.posted_on(16)[1], "Is the search slow on mobile only?");
    }

    // dana answers both. The operator takes the agent off work nobody
    // asked for, so the item that was not asked for cannot go on: its
    // mandate is revoked. The other is still blocked, and says so again,
    // though in the same words.
    work::run_local(work::WorkCommand::Rules {
        profile: "helper".into(),
        mode: Some("observe".into()),
        rules: Vec::new(),
        drop: Vec::new(),
    })
    .unwrap();
    {
        let mut hub = hub.lock().unwrap();
        hub.comment(15, "dana", "You have access now.", start + 40);
        hub.comment(16, "dana", "Yes, on mobile only.", start + 41);
    }
    run();
    let store = work::load("helper").unwrap();
    let w3 = store.item("w3").unwrap();
    let w4 = store.item("w4").unwrap();
    assert_eq!(
        w3.stage(),
        Stage::Blocked,
        "the same report again is a report: {w3:#?}"
    );
    assert_eq!(
        hub.lock().unwrap().posted_on(15)[2],
        "I need read access to the deploy logs."
    );
    let waiting = work::waiting().unwrap();
    assert_eq!(waiting.len(), 1, "{waiting:#?}");
    assert!(
        waiting[0].starts_with("work helper/w3: blocked"),
        "{}",
        waiting[0]
    );
    // dana stops it on its thread.
    hub.lock()
        .unwrap()
        .comment(15, "dana", "@sion-helper stop", start + 50);
    run();
    let store = work::load("helper").unwrap();
    let w3 = store.item("w3").unwrap();
    let stopped = w3.ended.as_ref().unwrap();
    assert_eq!(stopped.how, How::Stopped);
    assert_eq!(
        claim_schema(&ledger, stopped.claim.as_ref().unwrap()),
        "work.mandate.withdrawn/v1"
    );
    let revoked = w4.ended.as_ref().unwrap();
    assert_eq!(revoked.how, How::Revoked);
    assert!(
        revoked.why.contains("no longer taken on"),
        "{}",
        revoked.why
    );
    assert_eq!(
        claim_schema(&ledger, revoked.claim.as_ref().unwrap()),
        "work.mandate.revoked/v1"
    );
    {
        let hub = hub.lock().unwrap();
        assert_eq!(
            hub.posted_on(15)[3],
            "Stopped: Issue #15: Deploy test is flaky."
        );
        assert!(hub.posted_on(16)[2]
            .starts_with("I've stopped work on this: the rules I work under no longer allow it."));
        // Nothing it only noted went to anyone, and every message keeps to
        // its length.
        for (_, text) in &hub.posted {
            assert_ne!(text, "Reproduced on a cold cache.");
            assert_ne!(text, "Looking at the deploy test.");
            assert!(text.split_whitespace().count() <= sion_agent::work::THREAD_WORDS);
        }
        assert_eq!(hub.posted.len(), 12, "{:#?}", hub.posted);
    }
    // Every trigger was decided once; the agent's own comments, which
    // GitHub lists with the rest, were not taken as anyone's.
    let decisions = work::decisions("helper").unwrap();
    assert_eq!(decisions.len(), 10, "{decisions:#?}");
    assert!(work::waiting().unwrap().is_empty(), "nothing needs anyone");

    // A Telegram group: dana (4242) asks the bot by name; chatter while the
    // work is open goes to no one; her answer, a reply to the bot, goes to
    // the work.
    let bot = Arc::new(Mutex::new(Bot::default()));
    let telegram = serve_telegram(bot.clone(), done.clone());
    std::env::set_var("TELEGRAM_BOT_TOKEN", "bot-token");
    std::env::set_var("TELEGRAM_API_BASE", &telegram);
    work::run_local(work::WorkCommand::Watch {
        profile: "helper".into(),
        watch: "telegram:-100".into(),
        allow: vec!["4242".into()],
        disallow: Vec::new(),
        bot: Vec::new(),
        listed_unaddressed: None,
        unlisted_addressed: None,
        unlisted_unaddressed: None,
        remove: false,
    })
    .unwrap();
    let group = json!({"id": -100, "type": "supergroup"});
    {
        let mut bot = bot.lock().unwrap();
        bot.updates.push(json!({"update_id": 1, "message": {
            "message_id": 10, "chat": group, "from": {"id": 4242},
            "text": "@sion_helper_bot what did you spend?"}}));
        bot.updates.push(json!({"update_id": 2, "message": {
            "message_id": 11, "chat": group, "from": {"id": 4242},
            "text": "Lunch at one?"}}));
    }
    run();
    let store = work::load("helper").unwrap();
    let w5 = store.item("w5").unwrap();
    assert_eq!(
        w5.thread, "telegram:-100#",
        "the group, outside its threads"
    );
    assert_eq!(w5.mandate.strength, Strength::Explicit);
    assert_eq!(w5.stage(), Stage::Answer, "{w5:#?}");
    assert!(w5.inputs.is_empty(), "the chatter went to no one");
    let texts = |bot: &Bot| -> Vec<String> {
        bot.sent
            .iter()
            .map(|m| m["text"].as_str().unwrap_or_default().to_string())
            .collect()
    };
    assert_eq!(
        texts(&bot.lock().unwrap()),
        [
            "On it: what did you spend? I'll write here when it's ready, or if I need a decision.",
            "Which month? (October / November)"
        ]
    );
    assert_eq!(bot.lock().unwrap().sent[0]["chat_id"], -100);
    bot.lock()
        .unwrap()
        .updates
        .push(json!({"update_id": 3, "message": {
        "message_id": 12, "chat": group, "from": {"id": 4242}, "text": "November",
        "reply_to_message": {"message_id": 502, "from": {"id": 999}}}}));
    run();
    let w5 = work::load("helper").unwrap().item("w5").unwrap().clone();
    assert_eq!(
        w5.ended.as_ref().map(|e| e.how),
        Some(How::Completed),
        "{w5:#?}"
    );
    assert_eq!(
        texts(&bot.lock().unwrap())[2],
        "In November I spent 2 USD, on a test account for the login fix."
    );
    let decisions = work::decisions("helper").unwrap();
    assert_eq!(
        decisions.len(),
        12,
        "the chatter is not recorded: {decisions:#?}"
    );

    // A pull request dana asks the agent to keep an eye on: it reads the
    // changes, and waits.
    work::run_local(work::WorkCommand::Watch {
        profile: "helper".into(),
        watch: "github:acme/app".into(),
        allow: Vec::new(),
        disallow: Vec::new(),
        bot: vec!["github-actions[bot]".into()],
        listed_unaddressed: None,
        unlisted_addressed: None,
        unlisted_unaddressed: None,
        remove: false,
    })
    .unwrap();
    {
        let mut hub = hub.lock().unwrap();
        hub.issue(
            20,
            "dana",
            "Read the renamed cookie",
            "@sion-helper keep an eye on the checks of this one.",
            start + 60,
        );
        hub.issues.last_mut().unwrap()["pull_request"] =
            json!({"url": "https://api.github.example/repos/acme/app/pulls/20"});
    }
    run();
    let w6 = work::load("helper").unwrap().item("w6").unwrap().clone();
    assert_eq!(w6.stage(), Stage::Idle, "{w6:#?}");
    assert!(
        w6.request.subject.starts_with("Pull request #20"),
        "{}",
        w6.request.subject
    );
    // Its check fails: GitHub Actions, a bot on the watch's list, says so
    // in the pull request's thread, and the agent reads why.
    hub.lock().unwrap().checks.push(json!({
        "id": 77, "name": "test", "status": "completed", "conclusion": "failure",
        "head_sha": "abc1234567", "app": {"slug": "github-actions"},
        "output": {"title": "1 test failed", "summary": "login::rename"},
        "html_url": "https://github.example/acme/app/runs/77",
        "completed_at": sion_cli::github::rfc3339(start + 70)
    }));
    run();
    let w6 = work::load("helper").unwrap().item("w6").unwrap().clone();
    assert_eq!(
        w6.ended.as_ref().map(|e| e.how),
        Some(How::Completed),
        "{w6:#?}"
    );
    assert!(!w6.inputs[0].person, "a bot's word, not a person's");
    let session = sion_cli::runtime::Saved::load(w6.session.as_ref().unwrap()).unwrap();
    let results: Vec<String> = session
        .state
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            sion_agent::Block::ToolResult { content, .. } => Some(content.clone()),
            _ => None,
        })
        .collect();
    assert!(
        results
            .iter()
            .any(|r| r.contains("--- src/login.rs (modified, +2 −1)")),
        "{results:#?}"
    );
    assert!(
        results.iter().any(|r| r
            .contains("- test: failure — 1 test failed (job 77: github_job_log)")
            && r.contains("src/login.rs:3: failure: assertion failed: signed_in(old_cookie)")),
        "{results:#?}"
    );
    assert!(
        results
            .iter()
            .any(|r| r.starts_with("Errors:\nProcess completed with exit code 101.")),
        "{results:#?}"
    );
    assert!(hub
        .lock()
        .unwrap()
        .posted_on(20)
        .last()
        .unwrap()
        .starts_with("The check test fails"));
    // A review comment on its code: answered in its review thread.
    {
        let mut hub = hub.lock().unwrap();
        let at = sion_cli::github::rfc3339(start + 80);
        hub.reviews.push(json!({
            "id": 300, "body": "@sion-helper why read both names?", "user": {"login": "dana", "type": "User"},
            "path": "src/login.rs", "line": 2, "diff_hunk": "@@ -1,3 +1,4 @@\n+let old = \"sid\";",
            "pull_request_url": "https://api.github.example/repos/acme/app/pulls/20",
            "html_url": "https://github.example/acme/app/pull/20#discussion_r300",
            "created_at": at, "updated_at": at
        }));
        hub.latest = at;
    }
    run();
    let w7 = work::load("helper").unwrap().item("w7").unwrap().clone();
    assert_eq!(w7.thread, "github:acme/app#20:review:300");
    assert_eq!(
        w7.ended.as_ref().map(|e| e.how),
        Some(How::Completed),
        "{w7:#?}"
    );
    let replies = hub.lock().unwrap().replies.clone();
    assert_eq!(
        replies.len(),
        2,
        "the acknowledgement and the answer: {replies:#?}"
    );
    assert!(
        replies[1]
            .1
            .starts_with("Both names are read for one release"),
        "{replies:#?}"
    );
    done.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = std::fs::remove_dir_all(&home);
}
