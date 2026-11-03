//! GitHub's webhooks for the work runner: what happens on a watched
//! repository reaches the runner as it happens, instead of at its next
//! read (`sion agent work run --listen`).
//!
//! The runner listens on an address of its own ([`LISTEN`] by default),
//! which the operator exposes to GitHub behind TLS and names in the
//! repository's (or app's) webhook settings, `application/json`, with the
//! secret in `GITHUB_WEBHOOK_SECRET`. Every delivery is checked against
//! its `X-Hub-Signature-256` before anything else is read of it; one
//! unsigned or wrongly signed is refused (401), and one GitHub sends again
//! (the same `X-GitHub-Delivery`) is taken once. A delivery for a
//! repository the profile does not watch is answered and dropped.
//!
//! | Event | Becomes |
//! |---|---|
//! | `issues` (`opened`), `pull_request` (`opened`) | the issue or pull request, as the runner's read finds it |
//! | `issue_comment` (`created`) | the comment, in the conversation's thread |
//! | `pull_request_review_comment` (`created`) | the review comment, in its review thread |
//! | `pull_request_review` (`submitted`, with words) | the review, in the pull request's thread |
//! | `check_run` (`completed`, failed) | the failure, in each of its pull requests' threads, said by the check's app |
//!
//! Each becomes the trigger the runner's own read would make of it, with
//! the same id, so that what both bring is taken once. The runner still
//! reads the repositories, as often as `--poll` says, for what a webhook
//! missed.

use std::collections::{BTreeMap, VecDeque};
use std::io::Read;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use sion_agent::work::{Addressed, Place, Trigger};

use crate::github::{check_triggers, comment_trigger, issue_trigger, review_comment_trigger};

/// Where the runner listens for webhooks unless told otherwise.
pub const LISTEN: &str = "127.0.0.1:8794";

/// The largest delivery taken, in bytes.
pub const MAX_BODY: u64 = 5 << 20;

/// How many deliveries are remembered, so that one sent again is taken once.
pub const REMEMBERED: usize = 1_000;

/// The most deliveries read at once: each is read in a thread of its own,
/// so that a connection that stalls holds up no other.
pub const AT_ONCE: usize = 16;

/// What webhooks brought, until the runner takes it.
#[derive(Debug, Default)]
pub struct Inbox {
    triggers: Mutex<Vec<Trigger>>,
    ready: Condvar,
}

impl Inbox {
    /// What came, taken.
    pub fn take(&self) -> Vec<Trigger> {
        std::mem::take(&mut *self.triggers.lock().unwrap_or_else(|p| p.into_inner()))
    }

    /// Waits at most `wait` for something to come; whether something has.
    pub fn wait(&self, wait: Duration) -> bool {
        let held = self.triggers.lock().unwrap_or_else(|p| p.into_inner());
        if !held.is_empty() {
            return true;
        }
        let (held, _) = self
            .ready
            .wait_timeout(held, wait)
            .unwrap_or_else(|p| p.into_inner());
        !held.is_empty()
    }

    /// Adds what a delivery brought.
    pub fn add(&self, triggers: Vec<Trigger>) {
        if triggers.is_empty() {
            return;
        }
        self.triggers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .extend(triggers);
        self.ready.notify_all();
    }
}

/// Whether `signature` (`X-Hub-Signature-256`: `sha256=<hex>`) is the
/// signature of `body` with `secret`.
pub fn signed(secret: &[u8], body: &[u8], signature: Option<&str>) -> bool {
    let Some(given) = signature
        .and_then(|s| s.strip_prefix("sha256="))
        .and_then(|hex_part| hex::decode(hex_part.trim()).ok())
    else {
        return false;
    };
    !secret.is_empty() && sion_http::same_bytes(&sion_http::hmac_sha256(secret, body), &given)
}

/// The triggers a delivery of `event` with `payload` makes for the watch
/// `watch` of an agent whose login is `login`.
pub fn triggers(event: &str, payload: &Value, watch: &str, login: &str, now: u64) -> Vec<Trigger> {
    let repo = payload["repository"]["full_name"]
        .as_str()
        .unwrap_or_default();
    let action = payload["action"].as_str().unwrap_or_default();
    match (event, action) {
        ("issues", "opened") => issue_trigger(repo, watch, login, &payload["issue"], now)
            .into_iter()
            .collect(),
        ("pull_request", "opened") => {
            // As the runner reads it: a pull request among the issues.
            let mut issue = payload["pull_request"].clone();
            issue["pull_request"] = json!({"url": payload["pull_request"]["url"]});
            issue_trigger(repo, watch, login, &issue, now)
                .into_iter()
                .collect()
        }
        ("issue_comment", "created") => comment_trigger(
            repo,
            watch,
            login,
            &payload["comment"],
            &payload["issue"],
            now,
        )
        .into_iter()
        .collect(),
        ("pull_request_review_comment", "created") => {
            let mut pull = payload["pull_request"].clone();
            pull["pull_request"] = json!({"url": payload["pull_request"]["url"]});
            review_comment_trigger(repo, watch, login, &payload["comment"], &pull, now)
                .into_iter()
                .collect()
        }
        ("pull_request_review", "submitted") => {
            let review = &payload["review"];
            let body = review["body"].as_str().unwrap_or_default();
            let by = review["user"]["login"].as_str().unwrap_or_default();
            let number = payload["pull_request"]["number"].as_u64().unwrap_or(0);
            if body.trim().is_empty()
                || by.is_empty()
                || by.eq_ignore_ascii_case(login)
                || number == 0
            {
                return Vec::new();
            }
            let state = review["state"]
                .as_str()
                .unwrap_or_default()
                .replace('_', " ");
            let mut pull = payload["pull_request"].clone();
            pull["pull_request"] = json!({"url": payload["pull_request"]["url"]});
            vec![Trigger {
                id: format!(
                    "{watch}/review/{}",
                    review["id"].as_u64().unwrap_or_default()
                ),
                watch: watch.to_string(),
                place: Place {
                    chat: repo.to_string(),
                    thread: number.to_string(),
                },
                actor: by.to_string(),
                bot: review["user"]["type"].as_str() == Some("Bot") || by.ends_with("[bot]"),
                addressed: if crate::github::names(body, login) {
                    Addressed::Mention
                } else {
                    Addressed::No
                },
                text: format!("(a review: {state}) {body}"),
                at: crate::github::parse_time(review["submitted_at"].as_str().unwrap_or_default())
                    .unwrap_or(now),
                url: review["html_url"].as_str().map(str::to_string),
                subject: crate::github::subject(&pull),
                ..Trigger::default()
            }]
        }
        ("check_run", "completed") => {
            let run = &payload["check_run"];
            let numbers: Vec<u64> = run["pull_requests"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p["number"].as_u64())
                .collect();
            check_triggers(repo, watch, run, &numbers, now)
        }
        _ => Vec::new(),
    }
}

/// Who the agent is on each watched repository: its watch's name and its
/// login there, if the repository (`owner/name`) is watched.
pub type Watched = dyn Fn(&str) -> Option<(String, String)> + Send + Sync;

/// Listens at `address` for GitHub's deliveries signed with `secret`;
/// what they bring goes to the inbox returned, with the address listened
/// at. `watched` says whether a repository is watched, and by which login.
pub fn listen(
    address: &str,
    secret: String,
    watched: Arc<Watched>,
) -> Result<(Arc<Inbox>, String), String> {
    let server = tiny_http::Server::http(address)
        .map_err(|e| format!("cannot listen for webhooks at {address}: {e}"))?;
    let bound = server
        .server_addr()
        .to_ip()
        .map(|a| a.to_string())
        .unwrap_or_else(|| address.to_string());
    let inbox = Arc::new(Inbox::default());
    let filled = inbox.clone();
    let secret = Arc::new(secret);
    let seen: Arc<Mutex<VecDeque<String>>> = Arc::default();
    let reading = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            use std::sync::atomic::Ordering;
            if reading.fetch_add(1, Ordering::SeqCst) >= AT_ONCE {
                reading.fetch_sub(1, Ordering::SeqCst);
                respond(request, 503, "too many deliveries at once: send it again");
                continue;
            }
            let (secret, watched, filled, seen, reading) = (
                secret.clone(),
                watched.clone(),
                filled.clone(),
                seen.clone(),
                reading.clone(),
            );
            std::thread::spawn(move || {
                let (status, said) = receive(request, &secret, &watched, &filled, &seen);
                reading.fetch_sub(1, Ordering::SeqCst);
                if status >= 400 {
                    eprintln!("a GitHub webhook was refused: {said}");
                }
            });
        }
    });
    Ok((inbox, bound))
}

fn respond(request: tiny_http::Request, status: u16, text: &str) -> (u16, String) {
    let body = json!({"said": text}).to_string();
    let _ = request.respond(
        tiny_http::Response::from_string(body)
            .with_status_code(status)
            .with_header(
                "Content-Type: application/json"
                    .parse::<tiny_http::Header>()
                    .expect("a valid header"),
            ),
    );
    (status, text.to_string())
}

/// One delivery: checked, read, and answered.
fn receive(
    mut request: tiny_http::Request,
    secret: &str,
    watched: &Arc<Watched>,
    inbox: &Inbox,
    seen: &Mutex<VecDeque<String>>,
) -> (u16, String) {
    if request.method() != &tiny_http::Method::Post {
        return respond(request, 405, "webhooks are POSTed");
    }
    let header = |request: &tiny_http::Request, name: &str| {
        request
            .headers()
            .iter()
            .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str().to_string())
    };
    let signature = header(&request, "X-Hub-Signature-256");
    let event = header(&request, "X-GitHub-Event").unwrap_or_default();
    let delivery = header(&request, "X-GitHub-Delivery").unwrap_or_default();
    let mut body = Vec::new();
    if request
        .as_reader()
        .take(MAX_BODY + 1)
        .read_to_end(&mut body)
        .is_err()
        || body.len() as u64 > MAX_BODY
    {
        return respond(request, 413, "the delivery is too large");
    }
    if !signed(secret.as_bytes(), &body, signature.as_deref()) {
        return respond(request, 401, "the signature is missing or wrong");
    }
    let taken_already = || {
        seen.lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&delivery)
    };
    if !delivery.is_empty() && taken_already() {
        return respond(request, 200, "taken already");
    }
    // Remembered once it is taken, so that one sent again after a failure
    // here is taken then.
    let remember = || {
        if delivery.is_empty() {
            return;
        }
        let mut seen = seen.lock().unwrap_or_else(|e| e.into_inner());
        seen.push_back(delivery.clone());
        if seen.len() > REMEMBERED {
            seen.pop_front();
        }
    };
    if event == "ping" {
        return respond(request, 200, "pong");
    }
    let Ok(payload) = serde_json::from_slice::<Value>(&body) else {
        return respond(request, 400, "the delivery is not JSON");
    };
    let repo = payload["repository"]["full_name"]
        .as_str()
        .unwrap_or_default();
    let Some((watch, login)) = watched(repo) else {
        return respond(request, 202, "that repository is not watched");
    };
    let found = triggers(&event, &payload, &watch, &login, crate::work::now());
    let count = found.len();
    inbox.add(found);
    remember();
    respond(request, 202, &format!("{count} taken"))
}

/// The logins of watched repositories, found once each.
#[derive(Debug, Default)]
pub struct Logins(Mutex<BTreeMap<String, String>>);

impl Logins {
    /// The agent's login on `repo`, asked of GitHub the first time.
    pub fn of(&self, repo: &str) -> Option<String> {
        if let Some(login) = self.0.lock().unwrap_or_else(|p| p.into_inner()).get(repo) {
            return Some(login.clone());
        }
        let login = crate::github::Repo::open(repo).ok()?.login;
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(repo.to_string(), login.clone());
        Some(login)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_are_checked() {
        let body = br#"{"zen": "Keep it logically awesome."}"#;
        let signature = format!(
            "sha256={}",
            hex::encode(sion_http::hmac_sha256(b"s3cret", body))
        );
        assert!(signed(b"s3cret", body, Some(&signature)));
        assert!(!signed(b"other", body, Some(&signature)));
        assert!(!signed(b"s3cret", b"changed", Some(&signature)));
        assert!(!signed(b"s3cret", body, None));
        assert!(!signed(b"", body, Some(&signature)));
        assert!(!signed(b"s3cret", body, Some("sha1=abc")));
    }

    #[test]
    fn deliveries_become_the_triggers_a_read_would_make() {
        let repo = json!({"full_name": "acme/app"});
        let user = |login: &str| json!({"login": login, "type": "User"});
        let comment = json!({"action": "created", "repository": repo,
            "issue": {"number": 12, "title": "Crash", "body": "It crashes.", "user": user("dana")},
            "comment": {"id": 77, "body": "@sion-bot can you look?", "user": user("dana"),
                        "created_at": "2026-11-03T01:00:00Z", "html_url": "https://github.com/acme/app/issues/12#c77"}});
        let found = triggers("issue_comment", &comment, "github:acme/app", "sion-bot", 0);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "github:acme/app/comment/77");
        assert_eq!(found[0].addressed, Addressed::Mention);
        assert_eq!(found[0].subject, "Issue #12: Crash");
        assert_eq!(found[0].thread(), "github:acme/app#12");
        // The agent's own comment is not work.
        let mut own = comment.clone();
        own["comment"]["user"] = user("sion-bot");
        assert!(triggers("issue_comment", &own, "github:acme/app", "sion-bot", 0).is_empty());

        let review = json!({"action": "created", "repository": repo,
            "pull_request": {"number": 13, "title": "Fix crash", "url": "https://api.github.com/repos/acme/app/pulls/13"},
            "comment": {"id": 91, "in_reply_to_id": 90, "body": "Why unwrap here?", "user": user("lee"),
                        "path": "src/main.rs", "line": 42, "diff_hunk": "@@ -40,3 +40,3 @@\n-    a\n+    b.unwrap()",
                        "created_at": "2026-11-03T01:05:00Z"}});
        let found = triggers(
            "pull_request_review_comment",
            &review,
            "github:acme/app",
            "sion-bot",
            0,
        );
        assert_eq!(found[0].id, "github:acme/app/review-comment/91");
        assert_eq!(
            found[0].place.thread, "13:review:90",
            "a reply is in its first comment's thread"
        );
        assert_eq!(
            found[0].subject,
            "Pull request #13: Fix crash (a review comment on src/main.rs, line 42)"
        );
        assert!(
            found[0]
                .context
                .starts_with("On src/main.rs, line 42:\n@@ -40,3"),
            "{}",
            found[0].context
        );

        let check = json!({"action": "completed", "repository": repo, "check_run": {
            "id": 555, "name": "test", "status": "completed", "conclusion": "failure", "head_sha": "1a2b3c4d5e",
            "app": {"slug": "github-actions"}, "output": {"title": "2 tests failed", "summary": "parse::empty"},
            "html_url": "https://github.com/acme/app/runs/555", "pull_requests": [{"number": 13}]}});
        let found = triggers("check_run", &check, "github:acme/app", "sion-bot", 0);
        assert_eq!(found[0].id, "github:acme/app/check/555/13");
        assert_eq!(
            (found[0].actor.as_str(), found[0].bot),
            ("github-actions[bot]", true)
        );
        assert_eq!(
            found[0].text,
            "The check \"test\" ended in failure on 1a2b3c4: 2 tests failed\nparse::empty"
        );
        let mut passed = check.clone();
        passed["check_run"]["conclusion"] = json!("success");
        assert!(triggers("check_run", &passed, "github:acme/app", "sion-bot", 0).is_empty());

        let submitted = json!({"action": "submitted", "repository": repo,
            "pull_request": {"number": 13, "title": "Fix crash", "url": "u"},
            "review": {"id": 5, "state": "changes_requested", "body": "Add a test.", "user": user("lee"),
                       "submitted_at": "2026-11-03T01:10:00Z"}});
        let found = triggers(
            "pull_request_review",
            &submitted,
            "github:acme/app",
            "sion-bot",
            0,
        );
        assert_eq!(found[0].text, "(a review: changes requested) Add a test.");
        assert!(triggers("watch", &submitted, "github:acme/app", "sion-bot", 0).is_empty());
    }

    #[test]
    fn a_listener_takes_signed_deliveries_once() {
        let watched: Arc<Watched> = Arc::new(|repo: &str| {
            (repo == "acme/app").then(|| ("github:acme/app".to_string(), "sion-bot".to_string()))
        });
        let (inbox, bound) = listen("127.0.0.1:0", "s3cret".into(), watched).unwrap();
        let client = sion_http::Client::new(sion_http::Options {
            allow_private: true,
            ..sion_http::Options::default()
        })
        .unwrap();
        let body = json!({"action": "opened", "repository": {"full_name": "acme/app"},
            "issue": {"number": 3, "title": "Typo in README", "body": "", "user": {"login": "dana"},
                      "created_at": "2026-11-03T01:00:00Z"}})
        .to_string();
        let signature = format!(
            "sha256={}",
            hex::encode(sion_http::hmac_sha256(b"s3cret", body.as_bytes()))
        );
        let send = |signature: &str, delivery: &str| {
            client
                .send(
                    "POST",
                    &format!("http://{bound}/github"),
                    &[
                        ("Content-Type", "application/json"),
                        ("X-GitHub-Event", "issues"),
                        ("X-GitHub-Delivery", delivery),
                        ("X-Hub-Signature-256", signature),
                    ],
                    Some(body.as_bytes()),
                )
                .unwrap()
                .status
        };
        assert_eq!(send("sha256=00", "d-1"), 401);
        assert_eq!(send(&signature, "d-1"), 202);
        assert_eq!(send(&signature, "d-1"), 200, "sent again: taken once");
        assert!(inbox.wait(Duration::from_secs(5)));
        let taken = inbox.take();
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].id, "github:acme/app/issue/3");
        assert_eq!(taken[0].text, "Typo in README");
        assert!(!inbox.wait(Duration::from_millis(50)));
    }
}
