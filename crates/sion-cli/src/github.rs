//! A GitHub repository as a watch for work ([`crate::work`]): its new
//! issues and pull requests and the comments on them, read through
//! GitHub's REST API; and comments posted on them in answer.
//!
//! The token is `GH_TOKEN` or `GITHUB_TOKEN`, else the GitHub CLI's
//! (`gh auth token`); the API is `SION_GITHUB_API` if set (a stand-in, or
//! GitHub Enterprise), else `https://api.github.com`. The agent's own
//! login, which mentions name and whose comments are its own, is the
//! token's (`GET /user`), or `SION_GITHUB_LOGIN` for a token that has
//! none (an app's).
//!
//! Comments on a pull request's conversation are read, and review
//! comments on its code: each review thread (a comment on a line and the
//! replies to it) is a thread of its own, `<number>:review:<first
//! comment>`, answered with a reply in that thread. A pull request that
//! has work open on it is read for failed checks too ([`Repo::failed_checks`]):
//! a failure is said by the check's app (`github-actions[bot]`), a bot, so
//! it counts only where the watch lists that bot. Webhooks bring the same
//! triggers as they happen ([`crate::github_hooks`]); the tools a work
//! session has on its repository are in [`crate::github_tools`].

use std::collections::BTreeMap;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};
use sion_agent::work::{Addressed, Place, Trigger};

/// GitHub's API.
pub const API: &str = "https://api.github.com";

/// The most bytes one answer of the API may hold.
const MAX_BYTES: u64 = 8 << 20;

/// What one page of a listing holds: GitHub's most.
const PER_PAGE: usize = 100;

/// The most pages one read takes of a listing: the rest is read the next
/// time.
const PAGES: usize = 20;

/// Where reading one listing got to: the time of the latest change read,
/// and the time from which what was made counts as new. The second stays
/// behind while a read was cut short, so that what was made meanwhile is
/// not taken as old.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Position {
    /// The `updated_at` of the latest change read.
    pub read: String,
    /// What was made at this time or later is new.
    pub new_from: String,
}

impl Position {
    /// A position as [`Position::text`] keeps it.
    pub fn parse(text: &str) -> Position {
        match text.split_once(' ') {
            Some((read, new_from)) => Position {
                read: read.to_string(),
                new_from: new_from.to_string(),
            },
            None => Position {
                read: text.to_string(),
                new_from: text.to_string(),
            },
        }
    }

    /// The position as kept: `read new_from`.
    pub fn text(&self) -> String {
        format!("{} {}", self.read, self.new_from)
    }

    /// Where reading starts at `now`.
    fn start(now: u64) -> Position {
        let at = rfc3339(now);
        Position {
            read: at.clone(),
            new_from: at,
        }
    }
}

/// Why a comment was not posted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unsent {
    /// The issue is gone, or the agent may not see it: trying again does
    /// not help.
    Gone(String),
    /// Something that may pass (the network, a rate limit).
    Failed(String),
}

/// What one read of a repository found, and where each listing got to.
#[derive(Debug, Default)]
pub struct Polled {
    /// What was said, oldest first.
    pub triggers: Vec<Trigger>,
    /// Where the issues and pull requests got to.
    pub issues: Position,
    /// Where the comments on conversations got to.
    pub comments: Position,
    /// Where the review comments on code got to.
    pub reviews: Position,
    /// What could not be read, which the rest did not wait for (review
    /// comments a token may not read).
    pub notes: Vec<String>,
}

/// A repository the agent watches.
#[derive(Debug)]
pub struct Repo {
    pub(crate) client: sion_http::Client,
    pub(crate) api: String,
    pub(crate) token: String,
    /// `owner/name`.
    pub repo: String,
    /// The agent's own login.
    pub login: String,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// The token to call GitHub with: `GH_TOKEN`, `GITHUB_TOKEN`, or the
/// GitHub CLI's.
pub fn token() -> Option<String> {
    env("GH_TOKEN").or_else(|| env("GITHUB_TOKEN")).or_else(|| {
        let out = Command::new("gh")
            .args(["auth", "token"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let token = String::from_utf8(out.stdout).ok()?.trim().to_string();
        (out.status.success() && !token.is_empty()).then_some(token)
    })
}

/// Whether `repo` names a repository: `owner/name`.
pub fn valid_repo(repo: &str) -> bool {
    let ok = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    };
    matches!(repo.split_once('/'), Some((owner, name)) if ok(owner) && ok(name))
}

impl Repo {
    /// The repository `repo` (`owner/name`), with the token and the API the
    /// environment names.
    pub fn open(repo: &str) -> Result<Repo, String> {
        let mut opened = Repo::connect(repo)?;
        opened.login = match env("SION_GITHUB_LOGIN") {
            Some(login) => login,
            None => opened.get("/user")?["login"]
                .as_str()
                .map(str::to_string)
                .ok_or("GitHub did not say whose the token is: set SION_GITHUB_LOGIN")?,
        };
        Ok(opened)
    }

    /// The repository `repo`, as [`Repo::open`] opens it but without
    /// asking whose the token is: for reading, not watching.
    pub fn connect(repo: &str) -> Result<Repo, String> {
        if !valid_repo(repo) {
            return Err(format!("{repo:?} is not a repository: owner/name"));
        }
        let token = token().ok_or(
            "no GitHub token: set GH_TOKEN or GITHUB_TOKEN, or sign the GitHub CLI in (gh auth login)",
        )?;
        let client = sion_http::Client::new(sion_http::Options {
            timeout: Duration::from_secs(30),
            // The operator names the API (a stand-in, an Enterprise server).
            allow_private: true,
            ..sion_http::Options::default()
        })
        .map_err(|e| e.to_string())?;
        let api = env("SION_GITHUB_API")
            .unwrap_or_else(|| API.to_string())
            .trim_end_matches('/')
            .to_string();
        Ok(Repo {
            client,
            api,
            token,
            repo: repo.to_string(),
            login: String::new(),
        })
    }

    pub(crate) fn headers(&self) -> [(String, String); 3] {
        [
            ("Authorization".into(), format!("Bearer {}", self.token)),
            ("Accept".into(), "application/vnd.github+json".into()),
            ("X-GitHub-Api-Version".into(), "2022-11-28".into()),
        ]
    }

    pub(crate) fn hide(&self, text: String) -> String {
        text.replace(&self.token, "<token>")
    }

    pub(crate) fn get(&self, path: &str) -> Result<Value, String> {
        let headers = self.headers();
        let headers: Vec<(&str, &str)> = headers
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        let fetched = self
            .client
            .request(
                "GET",
                &format!("{}{path}", self.api),
                &headers,
                None,
                MAX_BYTES,
            )
            .map_err(|e| self.hide(format!("GitHub: {e}")))?;
        serde_json::from_slice(&fetched.body)
            .map_err(|e| format!("GitHub answered with something other than JSON: {e}"))
    }

    /// What a listing (`path`, sorted by the time of change, oldest first)
    /// changed since `from`: read a page at a time, each page from the time
    /// of the last change the one before it held (so that a change made
    /// while it reads moves on ahead of it, and is not skipped), up to
    /// [`PAGES`] pages; and where reading got to.
    fn changed(&self, path: &str, from: &Position) -> Result<(Vec<Value>, Position), String> {
        let mut latest = from.read.clone();
        let mut taken = std::collections::BTreeSet::new();
        let mut all = Vec::new();
        let mut complete = false;
        for _ in 0..PAGES {
            // From a second before: what changed in the second read last
            // is read again, and taken once.
            let since = parse_time(&latest)
                .map(|at| rfc3339(at.saturating_sub(1)))
                .unwrap_or_else(|| latest.clone());
            let listed = self.get(&format!("{path}&per_page={PER_PAGE}&since={since}"))?;
            let listed = listed
                .as_array()
                .cloned()
                .ok_or("GitHub answered a listing with something other than a list")?;
            let full = listed.len() >= PER_PAGE;
            let mut new = false;
            for entry in listed {
                let updated = entry["updated_at"].as_str().unwrap_or_default();
                if updated > latest.as_str() {
                    latest = updated.to_string();
                }
                let id = match &entry["id"] {
                    Value::Null => entry["number"].to_string(),
                    id => id.to_string(),
                };
                if taken.insert(id) {
                    all.push(entry);
                    new = true;
                }
            }
            // A page that is not full is the last; a full page that adds
            // nothing (all of one second) cannot be read past.
            if !full || !new {
                complete = true;
                break;
            }
        }
        let new_from = if complete {
            latest.clone()
        } else {
            from.new_from.clone()
        };
        Ok((
            all,
            Position {
                read: latest,
                new_from,
            },
        ))
    }

    /// What was said on the repository since `issues`, `comments` and
    /// `reviews` (the first time, with nothing read yet, nothing older is
    /// taken and reading starts now): the issues and pull requests opened,
    /// the comments made on their conversations and the review comments on
    /// their code, by others than the agent, oldest first; and where
    /// reading got to.
    pub fn poll(
        &self,
        watch: &str,
        issues: &Position,
        comments: &Position,
        reviews: &Position,
        now: u64,
    ) -> Result<Polled, String> {
        if issues.read.is_empty() || comments.read.is_empty() {
            return Ok(Polled {
                triggers: Vec::new(),
                issues: Position::start(now),
                comments: Position::start(now),
                reviews: Position::start(now),
                notes: Vec::new(),
            });
        }
        let mut triggers = Vec::new();
        let mut known: BTreeMap<String, Value> = BTreeMap::new();
        let (opened, issues_to) = self.changed(
            &format!(
                "/repos/{}/issues?state=all&sort=updated&direction=asc",
                self.repo
            ),
            issues,
        )?;
        for issue in &opened {
            let number = issue["number"].as_u64().unwrap_or(0).to_string();
            known.insert(number.clone(), issue.clone());
            let created = issue["created_at"].as_str().unwrap_or_default();
            if created < issues.new_from.as_str() {
                continue;
            }
            triggers.extend(issue_trigger(&self.repo, watch, &self.login, issue, now));
        }
        let (said, comments_to) = self.changed(
            &format!(
                "/repos/{}/issues/comments?sort=updated&direction=asc",
                self.repo
            ),
            comments,
        )?;
        for comment in &said {
            let created = comment["created_at"].as_str().unwrap_or_default();
            if created < comments.new_from.as_str() {
                continue;
            }
            let Some(number) = number_at_end(comment["issue_url"].as_str()) else {
                continue;
            };
            if !known.contains_key(&number) {
                let issue = self.get(&format!("/repos/{}/issues/{number}", self.repo))?;
                known.insert(number.clone(), issue);
            }
            triggers.extend(comment_trigger(
                &self.repo,
                watch,
                &self.login,
                comment,
                &known[&number],
                now,
            ));
        }
        // Review comments on code: read from where they were, or from now
        // if they never were (a watch kept from before they were read).
        let reviews = if reviews.read.is_empty() {
            Position::start(now)
        } else {
            reviews.clone()
        };
        let mut notes = Vec::new();
        let (reviewed, reviews_to) = match self.changed(
            &format!(
                "/repos/{}/pulls/comments?sort=updated&direction=asc",
                self.repo
            ),
            &reviews,
        ) {
            Ok(read) => read,
            Err(e) => {
                notes.push(format!("review comments could not be read: {e}"));
                (Vec::new(), reviews.clone())
            }
        };
        for comment in &reviewed {
            let created = comment["created_at"].as_str().unwrap_or_default();
            if created < reviews.new_from.as_str() {
                continue;
            }
            let Some(number) = number_at_end(comment["pull_request_url"].as_str()) else {
                continue;
            };
            if !known.contains_key(&number) {
                let issue = self.get(&format!("/repos/{}/issues/{number}", self.repo))?;
                known.insert(number.clone(), issue);
            }
            triggers.extend(review_comment_trigger(
                &self.repo,
                watch,
                &self.login,
                comment,
                &known[&number],
                now,
            ));
        }
        triggers.sort_by_key(|t| t.at);
        Ok(Polled {
            triggers,
            issues: issues_to,
            comments: comments_to,
            reviews: reviews_to,
            notes,
        })
    }

    /// The checks that failed on pull request `number`'s latest commit, as
    /// triggers in its thread, said by each check's app.
    pub fn failed_checks(
        &self,
        watch: &str,
        number: &str,
        now: u64,
    ) -> Result<Vec<Trigger>, String> {
        let pull = self.get(&format!("/repos/{}/pulls/{number}", self.repo))?;
        let Some(sha) = pull["head"]["sha"].as_str() else {
            return Ok(Vec::new());
        };
        let runs = self.get(&format!(
            "/repos/{}/commits/{sha}/check-runs?filter=latest&per_page=100",
            self.repo
        ))?;
        let numbers = [number.parse::<u64>().unwrap_or(0)];
        Ok(runs["check_runs"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|run| check_triggers(&self.repo, watch, run, &numbers, now))
            .collect())
    }

    /// Posts `text` as a comment on issue (or pull request) `thread`, or,
    /// for a review thread (`<number>:review:<comment>`), as a reply in it.
    pub fn comment(&self, thread: &str, text: &str) -> Result<(), Unsent> {
        let headers = self.headers();
        let headers: Vec<(&str, &str)> = headers
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        let body = json!({ "body": text }).to_string();
        let path = match thread.split_once(":review:") {
            Some((number, root)) => format!(
                "{}/repos/{}/pulls/{number}/comments/{root}/replies",
                self.api, self.repo
            ),
            None => format!("{}/repos/{}/issues/{thread}/comments", self.api, self.repo),
        };
        match self
            .client
            .post_json(&path, &headers, body.as_bytes(), MAX_BYTES)
        {
            Ok(_) => Ok(()),
            Err(sion_http::HttpError::Status { status, body, .. })
                if status == 404 || status == 410 =>
            {
                Err(Unsent::Gone(self.hide(format!(
                    "GitHub: HTTP {status}: {}",
                    body.chars().take(200).collect::<String>()
                ))))
            }
            Err(e) => Err(Unsent::Failed(self.hide(format!("GitHub: {e}")))),
        }
    }
}

/// The number at the end of a URL (`…/issues/12`), if there is one.
fn number_at_end(url: Option<&str>) -> Option<String> {
    url.and_then(|url| url.rsplit('/').next())
        .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        .map(str::to_string)
}

/// The trigger for an issue or pull request opened by someone other than
/// `login`.
pub(crate) fn issue_trigger(
    repo: &str,
    watch: &str,
    login: &str,
    issue: &Value,
    now: u64,
) -> Option<Trigger> {
    let number = issue["number"].as_u64().unwrap_or(0).to_string();
    let by = issue["user"]["login"].as_str().unwrap_or_default();
    if by.is_empty() || by.eq_ignore_ascii_case(login) {
        return None;
    }
    let title = issue["title"].as_str().unwrap_or_default();
    let body = issue["body"].as_str().unwrap_or_default();
    let assigned = issue["assignees"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|a| a["login"].as_str() == Some(login));
    let addressed = if assigned || names(title, login) || names(body, login) {
        Addressed::Mention
    } else {
        Addressed::No
    };
    Some(Trigger {
        id: format!("{watch}/issue/{number}"),
        watch: watch.to_string(),
        place: Place {
            chat: repo.to_string(),
            thread: number,
        },
        actor: by.to_string(),
        bot: is_bot(&issue["user"]),
        addressed,
        text: if body.trim().is_empty() {
            title.to_string()
        } else {
            body.to_string()
        },
        at: parse_time(issue["created_at"].as_str().unwrap_or_default()).unwrap_or(now),
        url: issue["html_url"].as_str().map(str::to_string),
        subject: subject(issue),
        ..Trigger::default()
    })
}

/// The trigger for a comment on `issue`'s conversation, by someone other
/// than `login`.
pub(crate) fn comment_trigger(
    repo: &str,
    watch: &str,
    login: &str,
    comment: &Value,
    issue: &Value,
    now: u64,
) -> Option<Trigger> {
    let by = comment["user"]["login"].as_str().unwrap_or_default();
    if by.is_empty() || by.eq_ignore_ascii_case(login) {
        return None;
    }
    let number = issue["number"].as_u64()?.to_string();
    let body = comment["body"].as_str().unwrap_or_default();
    Some(Trigger {
        id: format!(
            "{watch}/comment/{}",
            comment["id"].as_u64().unwrap_or_default()
        ),
        watch: watch.to_string(),
        place: Place {
            chat: repo.to_string(),
            thread: number,
        },
        actor: by.to_string(),
        bot: is_bot(&comment["user"]),
        addressed: if names(body, login) {
            Addressed::Mention
        } else {
            Addressed::No
        },
        text: body.to_string(),
        at: parse_time(comment["created_at"].as_str().unwrap_or_default()).unwrap_or(now),
        url: comment["html_url"].as_str().map(str::to_string),
        subject: subject(issue),
        context: issue["body"].as_str().unwrap_or_default().to_string(),
        context_by: issue["user"]["login"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        chat_wide: false,
    })
}

/// The most characters of a diff hunk a review comment's trigger carries.
const MAX_HUNK: usize = 1_500;

/// The trigger for a review comment on a pull request's code, by someone
/// other than `login`: in the thread of the review comments it replies to.
pub(crate) fn review_comment_trigger(
    repo: &str,
    watch: &str,
    login: &str,
    comment: &Value,
    pull: &Value,
    now: u64,
) -> Option<Trigger> {
    let by = comment["user"]["login"].as_str().unwrap_or_default();
    if by.is_empty() || by.eq_ignore_ascii_case(login) {
        return None;
    }
    let number = pull["number"]
        .as_u64()
        .map(|n| n.to_string())
        .or_else(|| number_at_end(comment["pull_request_url"].as_str()))?;
    let id = comment["id"].as_u64()?;
    let root = comment["in_reply_to_id"].as_u64().unwrap_or(id);
    let body = comment["body"].as_str().unwrap_or_default();
    let path = comment["path"].as_str().unwrap_or_default();
    let line = comment["line"]
        .as_u64()
        .or(comment["original_line"].as_u64())
        .map(|l| format!(", line {l}"))
        .unwrap_or_default();
    let hunk: String = comment["diff_hunk"]
        .as_str()
        .unwrap_or_default()
        .chars()
        .take(MAX_HUNK)
        .collect();
    Some(Trigger {
        id: format!("{watch}/review-comment/{id}"),
        watch: watch.to_string(),
        place: Place {
            chat: repo.to_string(),
            thread: format!("{number}:review:{root}"),
        },
        actor: by.to_string(),
        bot: is_bot(&comment["user"]),
        addressed: if names(body, login) {
            Addressed::Mention
        } else {
            Addressed::No
        },
        text: body.to_string(),
        at: parse_time(comment["created_at"].as_str().unwrap_or_default()).unwrap_or(now),
        url: comment["html_url"].as_str().map(str::to_string),
        subject: format!("{} (a review comment on {path}{line})", subject(pull)),
        context: format!("On {path}{line}:\n{hunk}"),
        context_by: String::new(),
        chat_wide: false,
    })
}

/// Whether a check run that ended so failed.
fn failed(conclusion: &str) -> bool {
    matches!(conclusion, "failure" | "timed_out")
}

/// The triggers for a check run that failed, one in the thread of each
/// pull request `numbers` names, said by the check's app.
pub(crate) fn check_triggers(
    repo: &str,
    watch: &str,
    run: &Value,
    numbers: &[u64],
    now: u64,
) -> Vec<Trigger> {
    let conclusion = run["conclusion"].as_str().unwrap_or_default();
    if run["status"].as_str() != Some("completed") || !failed(conclusion) {
        return Vec::new();
    }
    let id = run["id"].as_u64().unwrap_or_default();
    let app = run["app"]["slug"].as_str().unwrap_or("checks");
    let name = run["name"].as_str().unwrap_or("a check");
    let sha: String = run["head_sha"]
        .as_str()
        .unwrap_or_default()
        .chars()
        .take(7)
        .collect();
    let title = run["output"]["title"].as_str().unwrap_or_default();
    let summary: String = run["output"]["summary"]
        .as_str()
        .unwrap_or_default()
        .chars()
        .take(600)
        .collect();
    let mut text = format!("The check \"{name}\" ended in {conclusion} on {sha}");
    if !title.is_empty() {
        text.push_str(&format!(": {title}"));
    }
    if !summary.trim().is_empty() {
        text.push_str(&format!("\n{}", summary.trim()));
    }
    let at = parse_time(run["completed_at"].as_str().unwrap_or_default()).unwrap_or(now);
    numbers
        .iter()
        .filter(|n| **n > 0)
        .map(|number| Trigger {
            id: format!("{watch}/check/{id}/{number}"),
            watch: watch.to_string(),
            place: Place {
                chat: repo.to_string(),
                thread: number.to_string(),
            },
            actor: format!("{app}[bot]"),
            bot: true,
            addressed: Addressed::No,
            text: text.clone(),
            at,
            url: run["html_url"].as_str().map(str::to_string),
            subject: format!("Pull request #{number}"),
            ..Trigger::default()
        })
        .collect()
}

/// What a trigger says its thread is: `Issue #12: title`, or `Pull request
/// #13: title`.
pub(crate) fn subject(issue: &Value) -> String {
    let kind = if issue.get("pull_request").is_some_and(|p| !p.is_null()) {
        "Pull request"
    } else {
        "Issue"
    };
    format!(
        "{kind} #{}: {}",
        issue["number"].as_u64().unwrap_or(0),
        issue["title"].as_str().unwrap_or_default()
    )
}

/// Whether a GitHub user is a bot: an app, or a login ending in `[bot]`.
fn is_bot(user: &Value) -> bool {
    user["type"].as_str() == Some("Bot")
        || user["login"]
            .as_str()
            .is_some_and(|login| login.ends_with("[bot]"))
}

/// Whether `text` mentions `login` (`@login`, not part of a longer name).
pub fn names(text: &str, login: &str) -> bool {
    if login.is_empty() {
        return false;
    }
    let text = text.to_lowercase();
    let needle = format!("@{}", login.to_lowercase());
    text.match_indices(&needle).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + needle.len()..].chars().next();
        let part = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '-' || c == '_');
        !part(before) && !part(after)
    })
}

/// `seconds` (since the Unix epoch) as GitHub writes times:
/// `2026-11-02T10:00:00Z`.
pub fn rfc3339(seconds: u64) -> String {
    crate::runtime::utc(seconds)
        .trim_end_matches(" UTC")
        .replacen(' ', "T", 1)
        + "Z"
}

/// The seconds since the Unix epoch of a time as GitHub writes it
/// (`2026-11-02T10:00:00Z`).
pub fn parse_time(text: &str) -> Option<u64> {
    let text = text.strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let mut date = date.split('-').map(|p| p.parse::<i64>().ok());
    let (year, month, day) = (date.next()??, date.next()??, date.next()??);
    let mut time = time.split(':').map(|p| p.parse::<i64>().ok());
    let (hour, minute, second) = (time.next()??, time.next()??, time.next()??);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=9999).contains(&year)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=60).contains(&second)
    {
        return None;
    }
    // Days from civil (proleptic Gregorian), after Howard Hinnant.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second;
    u64::try_from(seconds).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_go_both_ways() {
        let at = 1_793_616_000; // 2026-11-02 10:40:00 UTC
        assert_eq!(rfc3339(at), "2026-11-02T10:40:00Z");
        assert_eq!(parse_time("2026-11-02T10:40:00Z"), Some(at));
        assert_eq!(parse_time("99999999999999999-01-01T00:00:00Z"), None);
        assert_eq!(parse_time("2026-11-02T25:00:00Z"), None);
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_time("2024-02-29T23:59:59Z").map(rfc3339),
            Some("2024-02-29T23:59:59Z".into())
        );
        assert_eq!(parse_time("yesterday"), None);
        // GitHub's times compare as text in the order of time.
        assert!(rfc3339(at) < rfc3339(at + 1));
    }

    #[test]
    fn a_position_is_kept_as_text() {
        let position = Position {
            read: "2026-11-02T10:40:00Z".into(),
            new_from: "2026-11-02T10:00:00Z".into(),
        };
        assert_eq!(Position::parse(&position.text()), position);
        let old = Position::parse("2026-11-02T10:40:00Z");
        assert_eq!(old.read, old.new_from);
        assert_eq!(Position::start(1_793_616_000).read, "2026-11-02T10:40:00Z");
    }

    #[test]
    fn a_mention_names_the_login_alone() {
        assert!(names("@sion-bot please look", "sion-bot"));
        assert!(names("cc @Sion-Bot.", "sion-bot"));
        assert!(!names("@sion-bot2 please", "sion-bot"));
        assert!(!names("mail dana@sion-bot", "sion-bot"));
        assert!(!names("no mention", "sion-bot"));
        assert!(valid_repo("acme/app") && !valid_repo("acme") && !valid_repo("a/b/c"));
        assert!(is_bot(&json!({"login": "dependabot[bot]", "type": "Bot"})));
        assert!(!is_bot(&json!({"login": "dana", "type": "User"})));
        assert_eq!(
            subject(&json!({"number": 13, "title": "Fix it", "pull_request": {"url": "x"}})),
            "Pull request #13: Fix it"
        );
    }
}
