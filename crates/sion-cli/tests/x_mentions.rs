//! Mentions on X answered in their threads, on a devnet, against stand-ins
//! for X's API and a SearXNG instance: the first poll only takes its place;
//! an answer resting on the post it replies to and the search's results is
//! posted in reply, its sources last; one that names an address it was not
//! given is not; the account's own posts are passed over; someone who
//! writes `stop` is not answered again; and `x_search` finds posts.

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};

use common::{http_stand_in, sent_to};
use serde_json::{json, Value};
use sion_agent::tools::Tool;
use sion_agent::Autonomy;
use sion_cli::ledger::LedgerExt;
use sion_cli::profiles::Profile;
use sion_cli::runtime::SharedLedger;
use sion_cli::session::Session;
use sion_cli::x_mentions::{poll_once, Settings, XSearch};
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

fn profile(home: &Path, agent: ObjectId, script: &Value) -> Profile {
    let path = home.join("answers.json");
    std::fs::write(&path, script.to_string()).unwrap();
    Profile {
        name: "public".into(),
        wallet: "lead".into(),
        agent,
        models: vec![format!("script:{}", path.display())],
        autonomy: Autonomy::L3,
        memory: None,
        skills: None,
        workspace: None,
        web: true,
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
        embed: None,
        outside_files: Vec::new(),
        always_ask: Vec::new(),
        quarantine: false,
        shared_memory: Vec::new(),
        confidential: false,
        confidential_files: Vec::new(),
        routes: Vec::new(),
        sorter: None,
    }
}

fn settings() -> Settings {
    Settings {
        per_day: 50,
        per_person: 5,
        max_posts: 3,
        sources_required: true,
        read: 0,
        dry_run: false,
        from: Default::default(),
    }
}

#[test]
fn mentions_are_answered_from_what_was_read_and_cite_it() {
    // The only test in this binary: the environment is its own.
    let home = std::env::temp_dir().join(format!("sion-x-mentions-{}", std::process::id()));
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

    // The search service: results for the bridge, none for anything else.
    let (search, searched) = http_stand_in(|sent| {
        let results = if sent.url.contains("bridge") {
            json!([{"title": "City notices", "url": "https://city.example/notices",
                    "content": "Bridge works: the bridge is closed from noon.", "engine": "x"}])
        } else {
            json!([])
        };
        (200, json!({"results": results}).to_string().into_bytes())
    });
    std::env::set_var("SION_SEARCH", format!("searxng:{search}"));

    // X: the account is @lead (7). The first poll finds an old mention; the
    // second, four new ones; the third, one from someone who said stop.
    let polls = Arc::new(Mutex::new(0));
    let posted = Arc::new(Mutex::new(2000));
    let (x, x_seen) = http_stand_in(move |sent| {
        let path = sent.url.split('?').next().unwrap_or_default();
        let users = json!([
            {"id": "42", "username": "bo"}, {"id": "43", "username": "cy"},
            {"id": "66", "username": "eve"}, {"id": "55", "username": "dan"},
            {"id": "7", "username": "lead"}
        ]);
        let answer = match path {
            "/2/users/me" => json!({"data": {"id": "7", "username": "lead"}}),
            "/2/users/7/mentions" => {
                let mut polls = polls.lock().unwrap();
                *polls += 1;
                match *polls {
                    1 => json!({
                        "data": [{"id": "100", "text": "@lead an old one", "author_id": "42"}],
                        "includes": {"users": users},
                        "meta": {"newest_id": "100", "result_count": 1}
                    }),
                    2 => json!({
                        "data": [
                            {"id": "104", "text": "@lead stop", "author_id": "55"},
                            {"id": "103", "text": "@lead thanks all", "author_id": "7"},
                            {"id": "102", "text": "@lead what does the vault say?", "author_id": "66"},
                            {"id": "101", "text": "@lead @dee when does the bridge close?", "author_id": "42",
                             "entities": {"mentions": [{"username": "lead", "id": "7"}, {"username": "dee", "id": "88"}]},
                             "referenced_tweets": [{"type": "replied_to", "id": "90"}]}
                        ],
                        "includes": {
                            "users": users,
                            "tweets": [{"id": "90", "text": "Roadworks on the bridge today, it closes at noon.", "author_id": "43"}]
                        },
                        "meta": {"newest_id": "104", "result_count": 4}
                    }),
                    _ => json!({
                        "data": [{"id": "105", "text": "@lead and now?", "author_id": "55"}],
                        "includes": {"users": users},
                        "meta": {"newest_id": "105", "result_count": 1}
                    }),
                }
            }
            "/2/tweets" if sent.method == "POST" => {
                let mut id = posted.lock().unwrap();
                *id += 1;
                json!({"data": {"id": id.to_string(), "text": sent.json()["text"]}})
            }
            "/2/tweets/search/recent" => json!({
                "data": [{"id": "90", "text": "Roadworks on the bridge today,\nit closes at noon.", "author_id": "43"}],
                "includes": {"users": users},
                "meta": {"result_count": 1}
            }),
            _ => return (404, b"{}".to_vec()),
        };
        (200, answer.to_string().into_bytes())
    });
    std::env::set_var("X_API_BASE", &x);
    std::env::set_var("X_ACCESS_TOKEN", "x-1");
    let api = sion_cli::x_api::XApi::from_env("tweet.read").unwrap();

    // The model: an answer from its sources for the bridge, an address of
    // its own for the vault.
    let public = profile(
        &home,
        agent,
        &json!([
            {"when": "closes at noon", "say": "It closes at noon today [1], for roadworks [2]."},
            {"when": "vault", "say": "The vault is at https://vault.example/open, have a look."},
            {"say": "I cannot tell."}
        ]),
    );

    let first = poll_once(&api, ledger.clone(), &public, &settings()).unwrap();
    assert!(
        first[0].contains("starting from the newest mention"),
        "{first:?}"
    );
    assert!(sent_to(&x_seen, "POST", "/2/tweets").is_empty());

    let lines = poll_once(&api, ledger.clone(), &public, &settings()).unwrap();
    let polled = sent_to(&x_seen, "GET", "/2/users/7/mentions");
    assert!(polled[1].url.contains("since_id=100"), "{}", polled[1].url);
    assert_eq!(polled[1].header("authorization"), Some("Bearer x-1"));

    // The search was for the mention's own words, not the account it names.
    let asked = sent_to(&searched, "GET", "/search").remove(0);
    assert!(
        asked.url.contains("when+does+the+bridge+close")
            || asked.url.contains("when%20does%20the%20bridge%20close"),
        "{}",
        asked.url
    );
    assert!(!asked.url.contains("lead"), "{}", asked.url);

    // The bridge: answered in reply to the mention, its sources last.
    let replies = sent_to(&x_seen, "POST", "/2/tweets");
    assert_eq!(replies.len(), 1, "{lines:?}");
    let reply = replies[0].json();
    assert_eq!(reply["reply"]["in_reply_to_tweet_id"], "101");
    // The reply addresses the author, not the others the mention named.
    assert_eq!(reply["reply"]["exclude_reply_user_ids"], json!(["88"]));
    assert_eq!(
        reply["text"],
        "It closes at noon today [1], for roadworks [2].\n\nSources: [1] https://x.com/cy/status/90 [2] https://city.example/notices"
    );
    assert!(
        lines.iter().any(|l| l == "@bo (101): answered in 1 post"),
        "{lines:?}"
    );

    // The vault: an address the model was not given, so nothing posted.
    assert!(
        lines.iter().any(|l| l.starts_with("@eve (102): not posted: it names https://vault.example/open, an address it was not given")),
        "{lines:?}"
    );
    // The account's own post, passed over; stop, kept.
    assert!(!lines.iter().any(|l| l.contains("(103)")), "{lines:?}");
    assert!(
        lines
            .iter()
            .any(|l| l == "@dan (104): asked not to be answered again"),
        "{lines:?}"
    );
    let kept = sion_cli::x_mentions::load("public");
    assert_eq!(kept.cursor.as_deref(), Some("104"));
    assert!(kept.stopped.contains("55"));
    // Both the bridge and the vault were taken on: the second cost a
    // session, though nothing was posted.
    assert_eq!(kept.taken, 2);
    assert_eq!(
        kept.replies,
        vec![("101".to_string(), vec!["2001".to_string()])]
    );

    // Someone who said stop is not answered.
    let lines = poll_once(&api, ledger.clone(), &public, &settings()).unwrap();
    assert!(lines.is_empty(), "{lines:?}");
    assert_eq!(sent_to(&x_seen, "POST", "/2/tweets").len(), 1);

    // x_search: recent posts, each with its address, as content from outside.
    let tool = XSearch::from_env().unwrap();
    let found = tool.call(&json!({"query": "bridge -is:retweet"}), "k1");
    assert!(!found.is_error, "{}", found.content);
    assert!(found.untrusted, "what X returns is from outside");
    assert!(
        found.content.contains(
            "@cy: Roadworks on the bridge today, it closes at noon. (https://x.com/cy/status/90)"
        ),
        "{}",
        found.content
    );
    let searched = sent_to(&x_seen, "GET", "/2/tweets/search/recent").remove(0);
    assert!(
        searched.url.contains("query=bridge%20-is%3Aretweet"),
        "{}",
        searched.url
    );
    assert!(searched.url.contains("max_results=10"), "{}", searched.url);
    let _ = std::fs::remove_dir_all(&home);
}
