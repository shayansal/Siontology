//! Tools for working on code in the workspace: changing a file where it
//! needs changing instead of writing it again, patches, finding files and
//! searching their text, and the project's own checker and tests, run in
//! the sandbox, their results read for the model.
//!
//! - [`EditFile`] (`edit_file`) replaces text the model quotes exactly,
//!   once, or everywhere it occurs when asked; text that occurs more than
//!   once or not at all is refused, with where it nearly occurs.
//! - [`ApplyPatch`] (`apply_patch`) applies a patch to several files at
//!   once, or to none: a unified diff (`git diff`'s) or the
//!   `*** Begin Patch` format. Hunks are found by their lines, not by the
//!   numbers in their headers, which models often get wrong; a hunk whose
//!   lines are not in the file fails the whole patch.
//! - [`FindFiles`] (`find_files`) lists the files a glob matches, and
//!   [`SearchFiles`] (`search_files`) the lines a regular expression
//!   matches, without looking into `.git`, `node_modules`, `target` and the
//!   like unless asked to.
//! - [`Diagnostics`] (`diagnostics`) runs the project's checker (`cargo
//!   check`, `tsc`, `ruff` or Python's compiler, `go vet`) and
//!   [`RunTests`] (`run_tests`) its tests (`cargo test`, `npm test`,
//!   `pytest`, `go test`, `make test`), found from the files the project
//!   has, in the sandbox the shell uses, and give back what they found:
//!   each problem as `file:line:column`, the tests that failed and the
//!   counts, and the end of the output.
//!
//! Paths stay inside the workspace ([`crate::builtin::inside`]); what is
//! read from a confidential path is held as the workspace's other tools
//! hold it.

use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::{json, Value};

use crate::builtin::{confidential_in, inside};
use crate::model::ToolSpec;
use crate::sandbox::{Mount, Sandbox, WORKSPACE};
use crate::tools::{Effect, Outcome, Tool};

/// The largest file these tools change or search, in bytes.
pub const MAX_FILE: u64 = 4 << 20;
/// The largest patch, in bytes.
pub const MAX_PATCH: usize = 1 << 20;
/// The most files a search or a listing looks at.
pub const MAX_VISITED: usize = 50_000;
/// The most paths a listing gives.
pub const MAX_FOUND: usize = 500;
/// The most matching lines a search gives.
pub const MAX_MATCHES: usize = 500;
/// The most characters of a line a search shows.
pub const MAX_LINE: usize = 300;
/// The most problems a check lists.
pub const MAX_FINDINGS: usize = 200;

/// Directories a listing or a search does not go into unless its path is
/// inside one: version control, dependencies and build output.
pub const SKIPPED: [&str; 11] = [
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    ".tox",
];

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": properties, "required": required})
}

/// The path of `path` in the workspace `root`, `/` between its parts.
fn relative_name(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Reads a text file of at most [`MAX_FILE`] bytes.
fn read_text(path: &Path, shown: &str) -> Result<String, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{shown}: {e}"))?;
    if meta.file_type().is_symlink() {
        // `inside` checked where it leads; read what it leads to.
        let real = std::fs::metadata(path).map_err(|e| format!("{shown}: {e}"))?;
        if !real.is_file() {
            return Err(format!("{shown} is not a regular file"));
        }
    } else if !meta.is_file() {
        return Err(format!("{shown} is not a regular file"));
    }
    if meta.len() > MAX_FILE {
        return Err(format!("{shown} is more than {MAX_FILE} bytes"));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{shown}: {e}"))?;
    String::from_utf8(bytes).map_err(|_| format!("{shown} is not UTF-8 text"))
}

/// Writes `content` to `path` through a sibling, so that no reader sees
/// half a file.
fn write_text(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    crate::builtin::write_aside(path, content.as_bytes()).map_err(|e| e.to_string())
}

/// Lines `first..=last` (counted from 1) of `text`, numbered.
fn numbered(text: &str, first: usize, last: usize) -> String {
    text.split('\n')
        .enumerate()
        .skip(first.saturating_sub(1))
        .take(last.saturating_sub(first) + 1)
        .map(|(i, line)| format!("{:>5}  {}", i + 1, line.trim_end_matches('\r')))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Changes a file by replacing text quoted exactly.
#[derive(Debug, Clone, Default)]
pub struct EditFile {
    /// The workspace.
    pub root: PathBuf,
    /// The workspace's confidential paths ([`crate::builtin::confidential_path`]).
    pub confidential: Vec<String>,
}

impl Tool for EditFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit_file".into(),
            description: "Change a text file in your workspace by replacing old_string, quoted \
                          exactly as the file has it (spaces and indentation included), with \
                          new_string. old_string must occur once: quote enough of the lines \
                          around it to make it unique, or set replace_all to replace every \
                          occurrence. Read the file first. Returns the changed lines."
                .into(),
            input_schema: schema(
                json!({
                    "path": {"type": "string"},
                    "old_string": {"type": "string"},
                    "new_string": {"type": "string"},
                    "replace_all": {"type": "boolean"}
                }),
                &["path", "old_string", "new_string"],
            ),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Write
    }

    fn journaled(&self) -> bool {
        // An edit made again finds nothing to replace and says so.
        false
    }

    fn resource(&self, input: &Value) -> Option<String> {
        let path = input["path"]
            .as_str()
            .unwrap_or(".")
            .trim_start_matches("./");
        Some(format!("file:{path}"))
    }

    fn call(&self, input: &Value, _key: &str) -> Outcome {
        let relative = input["path"].as_str().unwrap_or_default();
        let old = input["old_string"].as_str().unwrap_or_default();
        let new = input["new_string"].as_str().unwrap_or_default();
        let all = input["replace_all"].as_bool().unwrap_or(false);
        let held = confidential_in(&self.confidential, &self.root, relative);
        // The lines it shows of a file from outside are from outside.
        let outside = crate::provenance::is_outside(
            &self.root,
            &crate::builtin::resolved_name(&self.root, relative),
            std::fs::read(self.root.join(relative)).ok().as_deref(),
        ) || crate::provenance::is_outside(&self.root, relative, None);
        let outcome = match edit(&self.root, relative, old, new, all) {
            Ok(text) => Outcome::ok(text),
            Err(e) => Outcome::error(e),
        };
        let outcome = if outside {
            outcome.from_outside()
        } else {
            outcome
        };
        if held {
            outcome.held_confidential()
        } else {
            outcome
        }
    }
}

/// [`EditFile`]'s work: the changed lines, or why nothing changed.
pub fn edit(
    root: &Path,
    relative: &str,
    old: &str,
    new: &str,
    all: bool,
) -> Result<String, String> {
    if relative.is_empty() {
        return Err("give a file path".into());
    }
    if old.is_empty() {
        return Err("give old_string, the text to replace (write_file makes a new file)".into());
    }
    if old == new {
        return Err("old_string and new_string are the same: nothing would change".into());
    }
    let path = inside(root, relative)?;
    let text = read_text(&path, relative)?;
    // A file written with CRLF line ends is matched as the model sees it.
    let crlf = text.contains("\r\n");
    let (old, new) = if crlf && !old.contains("\r\n") {
        (old.replace('\n', "\r\n"), new.replace('\n', "\r\n"))
    } else {
        (old.to_string(), new.to_string())
    };
    let count = text.matches(old.as_str()).count();
    if count == 0 {
        return Err(not_found(&text, &old, relative));
    }
    if count > 1 && !all {
        let lines: Vec<String> = text
            .match_indices(old.as_str())
            .take(5)
            .map(|(at, _)| (text[..at].matches('\n').count() + 1).to_string())
            .collect();
        return Err(format!(
            "old_string occurs {count} times in {relative} (from lines {}): quote more of the \
             lines around the one to change, or set replace_all",
            lines.join(", ")
        ));
    }
    let first_at = text.find(old.as_str()).unwrap_or(0);
    let changed = if all {
        text.replace(old.as_str(), &new)
    } else {
        text.replacen(old.as_str(), &new, 1)
    };
    write_text(&path, &changed).map_err(|e| format!("{relative}: {e}"))?;
    let first = changed[..first_at].matches('\n').count() + 1;
    let last = first + new.matches('\n').count();
    let shown = numbered(
        &changed,
        first.saturating_sub(2).max(1),
        (last + 2).min(first + 20),
    );
    Ok(format!(
        "edited {relative}: {} replaced\n{shown}",
        if count == 1 {
            "1 occurrence".to_string()
        } else {
            format!("{count} occurrences")
        }
    ))
}

/// Why `old` is not in `text`, with where it nearly is.
fn not_found(text: &str, old: &str, relative: &str) -> String {
    let squeeze = |s: &str| {
        s.lines()
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let wanted = squeeze(old);
    let lines: Vec<&str> = text.lines().collect();
    let height = old.lines().count().max(1);
    for start in 0..lines.len() {
        let end = (start + height).min(lines.len());
        if squeeze(&lines[start..end].join("\n")) == wanted {
            return format!(
                "old_string is not in {relative} as quoted, but it is at line {} with other \
                 spacing: quote the file's text exactly as read_file shows it",
                start + 1
            );
        }
    }
    let first = old
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    if !first.is_empty() {
        if let Some(at) = lines.iter().position(|l| l.trim() == first) {
            return format!(
                "old_string is not in {relative}; its first line is at line {}, but the lines \
                 after it differ: read the file and quote them as they are",
                at + 1
            );
        }
    }
    format!("old_string is not in {relative}: read the file and quote its text exactly")
}

/// One change a patch makes to one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// A new file, with these lines.
    Add {
        /// Its path.
        path: String,
        /// Its lines.
        lines: Vec<String>,
    },
    /// A file removed.
    Delete {
        /// Its path.
        path: String,
    },
    /// A file changed (and moved, if `to` is given).
    Update {
        /// Its path.
        path: String,
        /// Where it moves to.
        to: Option<String>,
        /// What changes in it, in order.
        hunks: Vec<Hunk>,
    },
}

/// One place a file changes: the lines there (`old`, context and what is
/// taken out) and what they become (`new`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hunk {
    /// A line the hunk comes after (the `*** Begin Patch` format's `@@
    /// line`).
    pub anchor: Option<String>,
    /// Where the header says the hunk starts (a line index from 0).
    pub hint: Option<usize>,
    /// The lines as they are.
    pub old: Vec<String>,
    /// The lines as they become.
    pub new: Vec<String>,
}

/// Parses a patch: a unified diff, or the `*** Begin Patch` format.
pub fn parse_patch(patch: &str) -> Result<Vec<Change>, String> {
    let changes = if patch.lines().any(|l| l.trim_end() == "*** Begin Patch") {
        parse_begin_patch(patch)?
    } else {
        parse_unified(patch)?
    };
    if changes.is_empty() {
        return Err(
            "the patch changes no file: give a unified diff (--- a/path, +++ b/path, @@ hunks) \
             or *** Begin Patch … *** End Patch"
                .into(),
        );
    }
    Ok(changes)
}

/// A diff header's path: `a/` or `b/` taken off, a timestamp after a tab
/// dropped; none for `/dev/null`.
fn diff_path(text: &str) -> Option<String> {
    let text = text.split('\t').next().unwrap_or("").trim();
    let text = text.trim_matches('"');
    if text == "/dev/null" || text.is_empty() {
        return None;
    }
    let text = text
        .strip_prefix("a/")
        .or_else(|| text.strip_prefix("b/"))
        .unwrap_or(text);
    Some(text.to_string())
}

fn parse_unified(patch: &str) -> Result<Vec<Change>, String> {
    let header = Regex::new(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,\d+)? @@").expect("a valid pattern");
    let lines: Vec<&str> = patch.lines().collect();
    let mut changes = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let is_file = lines[i].starts_with("--- ")
            && lines
                .get(i + 1)
                .is_some_and(|next| next.starts_with("+++ "));
        if !is_file {
            i += 1;
            continue;
        }
        let from = diff_path(&lines[i][4..]);
        let to = diff_path(&lines[i + 1][4..]);
        i += 2;
        let mut hunks = Vec::new();
        while i < lines.len() && lines[i].starts_with("@@") {
            // A range of no lines (`-5,0`) is the place after its line.
            let hint = header.captures(lines[i]).and_then(|c| {
                let start = c[1].parse::<usize>().ok()?;
                let empty = c.get(2).is_some_and(|n| n.as_str() == "0");
                Some(if empty {
                    start
                } else {
                    start.saturating_sub(1)
                })
            });
            i += 1;
            let mut hunk = Hunk {
                hint,
                ..Hunk::default()
            };
            while i < lines.len() {
                let line = lines[i];
                if line.starts_with("@@")
                    || (line.starts_with("--- ")
                        && lines.get(i + 1).is_some_and(|n| n.starts_with("+++ ")))
                    || line.starts_with("diff --git ")
                {
                    break;
                }
                if let Some(rest) = line.strip_prefix('+') {
                    hunk.new.push(rest.to_string());
                } else if let Some(rest) = line.strip_prefix('-') {
                    hunk.old.push(rest.to_string());
                } else if let Some(rest) = line.strip_prefix(' ') {
                    hunk.old.push(rest.to_string());
                    hunk.new.push(rest.to_string());
                } else if line.is_empty() {
                    // A blank context line whose space was lost.
                    hunk.old.push(String::new());
                    hunk.new.push(String::new());
                } else if line.starts_with('\\') {
                    // "\ No newline at end of file".
                } else {
                    break;
                }
                i += 1;
            }
            hunks.push(hunk);
        }
        match (from, to) {
            (None, Some(path)) => changes.push(Change::Add {
                path,
                lines: hunks.into_iter().flat_map(|h| h.new).collect(),
            }),
            (Some(path), None) => changes.push(Change::Delete { path }),
            (Some(path), Some(to)) => changes.push(Change::Update {
                to: (to != path).then_some(to),
                path,
                hunks,
            }),
            (None, None) => return Err("a file in the patch is /dev/null on both sides".into()),
        }
    }
    Ok(changes)
}

fn parse_begin_patch(patch: &str) -> Result<Vec<Change>, String> {
    let mut changes: Vec<Change> = Vec::new();
    let mut lines = patch.lines().peekable();
    // Up to the start.
    for line in lines.by_ref() {
        if line.trim_end() == "*** Begin Patch" {
            break;
        }
    }
    let mut ended = false;
    while let Some(line) = lines.next() {
        let line = line.trim_end_matches('\r');
        if line.trim_end() == "*** End Patch" {
            ended = true;
            break;
        }
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            let mut added = Vec::new();
            while let Some(next) = lines.peek() {
                if next.starts_with("*** ") {
                    break;
                }
                let next = lines.next().unwrap_or_default();
                added.push(next.strip_prefix('+').unwrap_or(next).to_string());
            }
            changes.push(Change::Add {
                path: path.trim().to_string(),
                lines: added,
            });
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            changes.push(Change::Delete {
                path: path.trim().to_string(),
            });
        } else if let Some(path) = line.strip_prefix("*** Update File: ") {
            let mut to = None;
            let mut hunks: Vec<Hunk> = Vec::new();
            while let Some(next) = lines.peek() {
                if let Some(target) = next.strip_prefix("*** Move to: ") {
                    to = Some(target.trim().to_string());
                    lines.next();
                    continue;
                }
                if next.trim_end() == "*** End of File" {
                    lines.next();
                    continue;
                }
                if next.starts_with("*** ") {
                    break;
                }
                let next = lines.next().unwrap_or_default().trim_end_matches('\r');
                if let Some(anchor) = next.strip_prefix("@@") {
                    let anchor = anchor.trim();
                    hunks.push(Hunk {
                        anchor: (!anchor.is_empty()).then(|| anchor.to_string()),
                        ..Hunk::default()
                    });
                    continue;
                }
                if hunks.is_empty() {
                    hunks.push(Hunk::default());
                }
                let hunk = hunks.last_mut().expect("a hunk was pushed");
                if let Some(rest) = next.strip_prefix('+') {
                    hunk.new.push(rest.to_string());
                } else if let Some(rest) = next.strip_prefix('-') {
                    hunk.old.push(rest.to_string());
                } else {
                    let rest = next.strip_prefix(' ').unwrap_or(next);
                    hunk.old.push(rest.to_string());
                    hunk.new.push(rest.to_string());
                }
            }
            hunks.retain(|h| !(h.old.is_empty() && h.new.is_empty() && h.anchor.is_none()));
            changes.push(Change::Update {
                path: path.trim().to_string(),
                to,
                hunks,
            });
        } else if !line.trim().is_empty() {
            return Err(format!(
                "the patch has a line outside any file: {line:?} (each file starts with *** Add \
                 File:, *** Update File: or *** Delete File:)"
            ));
        }
    }
    if !ended {
        return Err("the patch has no *** End Patch".into());
    }
    Ok(changes)
}

/// A file's text as lines, with how it ends them.
#[derive(Clone, Debug)]
struct Lines {
    lines: Vec<String>,
    crlf: bool,
    newline_at_end: bool,
}

impl Lines {
    fn of(text: &str) -> Lines {
        let crlf = text.contains("\r\n");
        let newline_at_end = text.ends_with('\n') || text.is_empty();
        let mut lines: Vec<String> = text
            .split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
            .collect();
        if text.ends_with('\n') {
            lines.pop();
        }
        if text.is_empty() {
            lines.clear();
        }
        Lines {
            lines,
            crlf,
            newline_at_end,
        }
    }

    fn text(&self) -> String {
        let end = if self.crlf { "\r\n" } else { "\n" };
        let mut text = self.lines.join(end);
        if self.newline_at_end && !self.lines.is_empty() {
            text.push_str(end);
        }
        text
    }
}

/// Where `hunk`'s lines are in `lines`, looking from `from` on.
fn locate(lines: &[String], hunk: &Hunk, from: usize) -> Option<usize> {
    let mut from = from.min(lines.len());
    if let Some(anchor) = &hunk.anchor {
        let at = (from..lines.len()).find(|&i| lines[i].trim() == anchor.trim())?;
        from = at;
        if hunk.old.is_empty() {
            return Some(at + 1);
        }
    }
    if hunk.old.is_empty() {
        return Some(
            hunk.hint
                .filter(|&h| h >= from && h <= lines.len())
                .unwrap_or(lines.len()),
        );
    }
    let fits = |at: usize, same: &dyn Fn(&str, &str) -> bool| {
        at + hunk.old.len() <= lines.len()
            && hunk
                .old
                .iter()
                .zip(&lines[at..at + hunk.old.len()])
                .all(|(want, have)| same(want, have))
    };
    let exact = |a: &str, b: &str| a == b;
    let trailing = |a: &str, b: &str| a.trim_end() == b.trim_end();
    let spaces = |a: &str, b: &str| a.trim() == b.trim();
    if let Some(hint) = hunk.hint {
        if hint >= from && fits(hint, &exact) {
            return Some(hint);
        }
    }
    for same in [&exact as &dyn Fn(&str, &str) -> bool, &trailing, &spaces] {
        if let Some(at) = (from..lines.len()).find(|&at| fits(at, same)) {
            return Some(at);
        }
    }
    // Hunks out of order: anywhere in the file, if only once.
    let everywhere: Vec<usize> = (0..lines.len()).filter(|&at| fits(at, &trailing)).collect();
    match everywhere.as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

/// `hunks` applied to `text`, or which hunk does not fit.
pub fn apply_hunks(text: &str, hunks: &[Hunk]) -> Result<String, String> {
    let mut lines = Lines::of(text);
    let mut from = 0;
    for (n, hunk) in hunks.iter().enumerate() {
        let mut tried = hunk.clone();
        let mut at = locate(&lines.lines, &tried, from);
        // A blank line that only separated the patch's files is not context.
        while at.is_none()
            && tried.old.last().is_some_and(|l| l.is_empty())
            && tried.new.last().is_some_and(|l| l.is_empty())
        {
            tried.old.pop();
            tried.new.pop();
            at = locate(&lines.lines, &tried, from);
        }
        let Some(at) = at else {
            let shown: Vec<String> = hunk.old.iter().take(8).map(|l| format!("  {l}")).collect();
            return Err(format!(
                "hunk {} of {} does not match the file{}: these lines are not in it:\n{}",
                n + 1,
                hunks.len(),
                hunk.anchor
                    .as_ref()
                    .map(|a| format!(" after the line {a:?}"))
                    .unwrap_or_default(),
                shown.join("\n")
            ));
        };
        let end = at + tried.old.len();
        lines.lines.splice(at..end, tried.new.iter().cloned());
        from = at + tried.new.len();
    }
    Ok(lines.text())
}

/// Applies patches to the workspace: all of a patch's changes, or none.
#[derive(Debug, Clone, Default)]
pub struct ApplyPatch {
    /// The workspace.
    pub root: PathBuf,
    /// The workspace's confidential paths.
    pub confidential: Vec<String>,
}

impl Tool for ApplyPatch {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "apply_patch".into(),
            description: "Apply a patch to files in your workspace: a unified diff (--- a/path, \
                          +++ b/path, @@ hunks with ' ' context, '-' and '+' lines; /dev/null for \
                          a new or removed file), or *** Begin Patch / *** Update File: path / \
                          @@ / lines / *** End Patch (also *** Add File:, *** Delete File:, \
                          *** Move to:). Hunks are found by their lines, so give a few \
                          unchanged lines around each change. The whole patch is checked, and \
                          every new text written, before any file changes."
                .into(),
            input_schema: schema(json!({"patch": {"type": "string"}}), &["patch"]),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Write
    }

    fn journaled(&self) -> bool {
        false
    }

    fn call(&self, input: &Value, _key: &str) -> Outcome {
        let patch = input["patch"].as_str().unwrap_or_default();
        let changes = match parse_patch(patch) {
            Ok(changes) if patch.len() <= MAX_PATCH => changes,
            Ok(_) => return Outcome::error(format!("a patch is at most {MAX_PATCH} bytes")),
            Err(e) => return Outcome::error(e),
        };
        let held = changes.iter().any(|change| {
            let (path, to) = match change {
                Change::Add { path, .. } | Change::Delete { path } => (path, None),
                Change::Update { path, to, .. } => (path, to.as_ref()),
            };
            confidential_in(&self.confidential, &self.root, path)
                || to.is_some_and(|to| confidential_in(&self.confidential, &self.root, to))
        });
        // A confidential file moved is confidential where it went.
        let moved: Vec<&String> = changes
            .iter()
            .filter_map(|change| match change {
                Change::Update {
                    path, to: Some(to), ..
                } if confidential_in(&self.confidential, &self.root, path) => Some(to),
                _ => None,
            })
            .collect();
        let outcome = match apply(&self.root, &changes) {
            Ok(summary) => {
                for to in moved {
                    let _ = crate::provenance::keep_confidential(&self.root, to);
                }
                Outcome::ok(summary)
            }
            Err(e) => Outcome::error(e),
        };
        if held {
            outcome.held_confidential()
        } else {
            outcome
        }
    }
}

/// Applies `changes` to the workspace `root`: each is worked out first, and
/// files are written only if every one fits.
pub fn apply(root: &Path, changes: &[Change]) -> Result<String, String> {
    enum Step {
        Write(PathBuf, String),
        Remove(PathBuf),
    }
    let mut steps = Vec::new();
    let mut summary = Vec::new();
    let mut touched = std::collections::BTreeSet::new();
    for change in changes {
        match change {
            Change::Add { path, lines } => {
                let target = inside(root, path)?;
                if target.exists() {
                    return Err(format!(
                        "{path} exists already: update it instead of adding it"
                    ));
                }
                if !touched.insert(target.clone()) {
                    return Err(format!("the patch changes {path} twice"));
                }
                let mut text = lines.join("\n");
                text.push('\n');
                summary.push(format!("A {path} ({} lines)", lines.len()));
                steps.push(Step::Write(target, text));
            }
            Change::Delete { path } => {
                let target = inside(root, path)?;
                if !target.is_file() {
                    return Err(format!("{path} is not a file of the workspace, to remove"));
                }
                if !touched.insert(target.clone()) {
                    return Err(format!("the patch changes {path} twice"));
                }
                summary.push(format!("D {path}"));
                steps.push(Step::Remove(target));
            }
            Change::Update { path, to, hunks } => {
                let target = inside(root, path)?;
                if !touched.insert(target.clone()) {
                    return Err(format!("the patch changes {path} twice"));
                }
                let text = read_text(&target, path)?;
                let changed = apply_hunks(&text, hunks).map_err(|e| format!("{path}: {e}"))?;
                let added: usize = hunks
                    .iter()
                    .map(|h| h.new.len().saturating_sub(common(h)))
                    .sum();
                let removed: usize = hunks
                    .iter()
                    .map(|h| h.old.len().saturating_sub(common(h)))
                    .sum();
                match to {
                    Some(to) => {
                        let moved = inside(root, to)?;
                        if moved.exists() || !touched.insert(moved.clone()) {
                            return Err(format!("{to} exists already: {path} cannot move there"));
                        }
                        summary.push(format!("R {path} → {to} (+{added} −{removed})"));
                        steps.push(Step::Write(moved, changed));
                        steps.push(Step::Remove(target));
                    }
                    None => {
                        summary.push(format!(
                            "M {path} ({} hunk{}, +{added} −{removed})",
                            hunks.len(),
                            if hunks.len() == 1 { "" } else { "s" }
                        ));
                        steps.push(Step::Write(target, changed));
                    }
                }
            }
        }
    }
    // Every new text is written beside its file before any file changes:
    // a failure here changes nothing.
    let mut ready: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut removed = Vec::new();
    for step in steps {
        let made = match step {
            Step::Write(path, text) => path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| crate::builtin::write_beside(&path, text.as_bytes()))
                .map(|beside| ready.push(beside))
                .map_err(|e| format!("{}: {e}", relative_name(root, &path))),
            Step::Remove(path) => {
                removed.push(path);
                Ok(())
            }
        };
        if let Err(e) = made {
            for (_, aside) in &ready {
                let _ = std::fs::remove_file(aside);
            }
            return Err(e);
        }
    }
    // Then each is put in its file's place, and what goes is removed.
    for (at, aside) in &ready {
        crate::builtin::put_in_place(aside, at)
            .map_err(|e| format!("{}: {e}", relative_name(root, at)))?;
    }
    for path in removed {
        std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", relative_name(root, &path)))?;
    }
    Ok(format!("applied the patch:\n{}", summary.join("\n")))
}

/// The lines a hunk keeps as they are (its context), counted as the
/// shorter side's lines that both sides share in order.
fn common(hunk: &Hunk) -> usize {
    // Longest common subsequence, bounded: hunks are short.
    let (a, b) = (&hunk.old, &hunk.new);
    if a.len() * b.len() > 1_000_000 {
        return 0;
    }
    let mut table = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            table[i][j] = if a[i] == b[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    table[0][0]
}

/// A glob as a regular expression over paths relative to where a listing
/// starts: `*` is any characters but `/`, `**` any directories, `?` one
/// character, `[…]` a class (`[!…]` its complement) and `{a,b}` either. A
/// glob without `/` matches a file's name at any depth.
pub fn glob(pattern: &str) -> Result<Regex, String> {
    let pattern = pattern.trim().trim_start_matches("./");
    if pattern.is_empty() {
        return Err("give a glob, such as src/**/*.rs or *.py".into());
    }
    let anywhere = !pattern.contains('/');
    let mut out = String::from(if anywhere { "^(?:.*/)?" } else { "^" });
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    let mut braces = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '*' if chars.get(i + 1) == Some(&'*') => {
                if chars.get(i + 2) == Some(&'/') {
                    out.push_str("(?:.*/)?");
                    i += 3;
                } else {
                    out.push_str(".*");
                    i += 2;
                }
                continue;
            }
            '*' => out.push_str("[^/]*"),
            '?' => out.push_str("[^/]"),
            '[' => {
                let close = chars[i + 1..].iter().position(|&c| c == ']');
                match close {
                    Some(len) if len > 0 => {
                        let class: String = chars[i + 1..i + 1 + len].iter().collect();
                        let class = class
                            .strip_prefix('!')
                            .map(|rest| format!("^{rest}"))
                            .unwrap_or(class);
                        out.push('[');
                        out.push_str(&class.replace('\\', "\\\\"));
                        out.push(']');
                        i += len + 2;
                        continue;
                    }
                    _ => out.push_str("\\["),
                }
            }
            '{' => {
                braces += 1;
                out.push_str("(?:");
            }
            '}' if braces > 0 => {
                braces -= 1;
                out.push(')');
            }
            ',' if braces > 0 => out.push('|'),
            _ => out.push_str(&regex::escape(&c.to_string())),
        }
        i += 1;
    }
    if braces > 0 {
        return Err(format!("{pattern}: a {{ is not closed"));
    }
    out.push('$');
    Regex::new(&out).map_err(|e| format!("{pattern}: {e}"))
}

/// The files under `start` (a directory of the workspace `root`), as paths
/// relative to `start`, sorted, skipping [`SKIPPED`] directories below it
/// and never following a symbolic link; at most [`MAX_VISITED`].
pub(crate) fn walk(start: &Path) -> (Vec<(String, PathBuf)>, bool) {
    let mut found = Vec::new();
    let mut stack = vec![start.to_path_buf()];
    let mut cut = false;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries.into_iter().rev() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !SKIPPED.contains(&name.as_str()) {
                    stack.push(path);
                }
            } else if kind.is_file() {
                if found.len() >= MAX_VISITED {
                    cut = true;
                    break;
                }
                found.push((relative_name(start, &path), path));
            }
        }
    }
    found.sort();
    (found, cut)
}

/// Lists the workspace's files a glob matches.
#[derive(Debug, Clone, Default)]
pub struct FindFiles {
    /// The workspace.
    pub root: PathBuf,
}

impl Tool for FindFiles {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "find_files".into(),
            description: format!(
                "List the files in your workspace whose paths match a glob: * is any characters \
                 but /, ** any directories, ? one character, [abc] a class, {{a,b}} either; a \
                 glob without / matches file names at any depth (*.rs). Looks under `path` (the \
                 workspace's top by default), not into {}, unless `path` is in one. At most \
                 {MAX_FOUND} paths.",
                SKIPPED[..6].join(", ")
            ),
            input_schema: schema(
                json!({"pattern": {"type": "string"}, "path": {"type": "string"}}),
                &["pattern"],
            ),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn call(&self, input: &Value, _key: &str) -> Outcome {
        let pattern = match glob(input["pattern"].as_str().unwrap_or_default()) {
            Ok(pattern) => pattern,
            Err(e) => return Outcome::error(e),
        };
        let relative = input["path"].as_str().unwrap_or_default();
        let start = match inside(&self.root, relative) {
            Ok(start) if start.is_dir() => start,
            Ok(_) => {
                return Outcome::error(format!("{relative} is not a directory of the workspace"))
            }
            Err(e) => return Outcome::error(e),
        };
        let (files, cut) = walk(&start);
        let prefix = relative.trim_matches('/');
        let matched: Vec<String> = files
            .iter()
            .filter(|(name, _)| pattern.is_match(name))
            .map(|(name, _)| {
                if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}/{name}")
                }
            })
            .collect();
        if matched.is_empty() {
            return Outcome::ok("no file matches");
        }
        let mut text = matched
            .iter()
            .take(MAX_FOUND)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        if matched.len() > MAX_FOUND {
            text.push_str(&format!(
                "\n[{} more: give a narrower glob or path]",
                matched.len() - MAX_FOUND
            ));
        }
        if cut {
            text.push_str(&format!(
                "\n[only the first {MAX_VISITED} files were looked at]"
            ));
        }
        Outcome::ok(text)
    }
}

/// Searches the text of the workspace's files with a regular expression.
#[derive(Debug, Clone, Default)]
pub struct SearchFiles {
    /// The workspace.
    pub root: PathBuf,
    /// The workspace's confidential paths: a search that shows a line of
    /// one is held.
    pub confidential: Vec<String>,
}

impl Tool for SearchFiles {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "search_files".into(),
            description: format!(
                "Search the text of the files in your workspace with a regular expression (Rust \
                 regex syntax: \\s, \\b, (a|b), …). Gives each matching line as path:line: text, \
                 `context` lines around it if asked (at most 5), or only the files with \
                 files_only. Narrow it with `path` (a directory or a file) and `glob` (*.rs). \
                 Binary files and {} are left out. At most {MAX_MATCHES} lines.",
                SKIPPED[..6].join(", ")
            ),
            input_schema: schema(
                json!({
                    "pattern": {"type": "string"},
                    "path": {"type": "string"},
                    "glob": {"type": "string"},
                    "ignore_case": {"type": "boolean"},
                    "context": {"type": "integer", "minimum": 0, "maximum": 5},
                    "files_only": {"type": "boolean"},
                    "max_results": {"type": "integer", "minimum": 1, "maximum": MAX_MATCHES}
                }),
                &["pattern"],
            ),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn call(&self, input: &Value, _key: &str) -> Outcome {
        match search(&self.root, input) {
            Ok((text, names)) => {
                let held = names
                    .iter()
                    .any(|name| confidential_in(&self.confidential, &self.root, name));
                // Lines of a file from outside are from outside.
                let outside = names.iter().any(|name| {
                    crate::provenance::is_outside(
                        &self.root,
                        name,
                        std::fs::read(self.root.join(name)).ok().as_deref(),
                    )
                });
                let outcome = if outside {
                    Outcome::ok(text).from_outside()
                } else {
                    Outcome::ok(text)
                };
                if held {
                    outcome.held_confidential()
                } else {
                    outcome
                }
            }
            Err(e) => Outcome::error(e),
        }
    }
}

/// [`SearchFiles`]'s work: what it shows, and the files it showed lines of.
fn search(root: &Path, input: &Value) -> Result<(String, Vec<String>), String> {
    let pattern = input["pattern"].as_str().unwrap_or_default();
    if pattern.is_empty() {
        return Err("give a pattern".into());
    }
    let regex = regex::RegexBuilder::new(pattern)
        .case_insensitive(input["ignore_case"].as_bool().unwrap_or(false))
        .size_limit(1 << 22)
        .build()
        .map_err(|e| format!("the pattern: {e}"))?;
    let only = match input["glob"].as_str() {
        Some(g) if !g.trim().is_empty() => Some(glob(g)?),
        _ => None,
    };
    let context = input["context"].as_u64().unwrap_or(0).min(5) as usize;
    let files_only = input["files_only"].as_bool().unwrap_or(false);
    let most = input["max_results"]
        .as_u64()
        .map(|n| n as usize)
        .unwrap_or(MAX_MATCHES)
        .clamp(1, MAX_MATCHES);
    let relative = input["path"].as_str().unwrap_or_default().trim_matches('/');
    let start = inside(root, relative)?;
    let (files, cut) = if start.is_file() {
        let name = start
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        (vec![(name, start.clone())], false)
    } else if start.is_dir() {
        walk(&start)
    } else {
        return Err(format!("{relative} is not in the workspace"));
    };
    let name_of = |name: &str| {
        if start.is_file() || relative.is_empty() {
            if start.is_file() {
                relative.to_string()
            } else {
                name.to_string()
            }
        } else {
            format!("{relative}/{name}")
        }
    };
    let mut shown = Vec::new();
    let mut names = Vec::new();
    let mut matches = 0usize;
    let mut more = false;
    'files: for (name, path) in &files {
        if let Some(only) = &only {
            if !only.is_match(name) {
                continue;
            }
        }
        let Ok(meta) = std::fs::metadata(path) else {
            continue;
        };
        if meta.len() > MAX_FILE {
            continue;
        }
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        if bytes.iter().take(8192).any(|&b| b == 0) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        let lines: Vec<&str> = text.lines().collect();
        let hits: Vec<usize> = (0..lines.len())
            .filter(|&i| regex.is_match(lines[i]))
            .collect();
        if hits.is_empty() {
            continue;
        }
        let shown_name = name_of(name);
        names.push(shown_name.clone());
        if files_only {
            shown.push(shown_name);
            matches += 1;
            if matches >= most {
                more = true;
                break;
            }
            continue;
        }
        let mut last_shown: Option<usize> = None;
        for &hit in &hits {
            if matches >= most {
                more = true;
                break 'files;
            }
            let first = hit.saturating_sub(context);
            let last = (hit + context).min(lines.len() - 1);
            if context > 0 && last_shown.is_some_and(|l| first > l + 1) {
                shown.push("--".into());
            }
            for (i, text) in lines.iter().enumerate().take(last + 1).skip(first) {
                if last_shown.is_some_and(|l| i <= l) {
                    continue;
                }
                let mark = if i == hit { ':' } else { '-' };
                let line: String = text.chars().take(MAX_LINE).collect();
                shown.push(format!("{shown_name}{mark}{}{mark} {line}", i + 1));
                last_shown = Some(i);
            }
            matches += 1;
        }
    }
    if shown.is_empty() {
        return Ok(("no match".into(), names));
    }
    let mut text = shown.join("\n");
    if more {
        text.push_str(&format!("\n[stopped at {most} matches: narrow the search]"));
    }
    if cut {
        text.push_str(&format!(
            "\n[only the first {MAX_VISITED} files were looked at]"
        ));
    }
    Ok((text, names))
}

/// A project's kind, from the files it has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `Cargo.toml`.
    Rust,
    /// `tsconfig.json`, or a `package.json`.
    Node,
    /// `pyproject.toml`, `setup.py`, `setup.cfg`, `requirements.txt`,
    /// `pytest.ini`, `tox.ini`.
    Python,
    /// `go.mod`.
    Go,
    /// A `Makefile` with a `test` target (tests only).
    Make,
}

/// The kind of project at `dir`, first match in [`Kind`]'s order.
pub fn kind(dir: &Path) -> Option<Kind> {
    let has = |name: &str| dir.join(name).is_file();
    if has("Cargo.toml") {
        Some(Kind::Rust)
    } else if has("tsconfig.json") || has("package.json") {
        Some(Kind::Node)
    } else if [
        "pyproject.toml",
        "setup.py",
        "setup.cfg",
        "requirements.txt",
        "pytest.ini",
        "tox.ini",
    ]
    .iter()
    .any(|n| has(n))
    {
        Some(Kind::Python)
    } else if has("go.mod") {
        Some(Kind::Go)
    } else if std::fs::read_to_string(dir.join("Makefile"))
        .map(|m| m.lines().any(|l| l.starts_with("test:")))
        .unwrap_or(false)
    {
        Some(Kind::Make)
    } else {
        None
    }
}

/// The command that checks a project of `kind` (without network, cargo
/// is told to stay offline), and what it is called.
pub fn check_command(kind: Kind, offline: bool) -> Option<(&'static str, String)> {
    let offline = if offline { " --offline" } else { "" };
    match kind {
        Kind::Rust => Some((
            "cargo check",
            format!("cargo check --all-targets --message-format=short --color=never{offline}"),
        )),
        Kind::Node => Some((
            "tsc",
            "if [ -x node_modules/.bin/tsc ]; then node_modules/.bin/tsc --noEmit --pretty false; \
             else tsc --noEmit --pretty false; fi"
                .into(),
        )),
        Kind::Python => Some((
            "ruff, or Python's compiler",
            "if command -v ruff >/dev/null 2>&1; then ruff check --output-format=concise --no-fix .; \
             else python3 -m compileall -q -x '(^|/)(\\.git|\\.venv|venv|node_modules)(/|$)' .; fi"
                .into(),
        )),
        Kind::Go => Some(("go vet", "go vet ./...".into())),
        Kind::Make => None,
    }
}

/// The command that runs a project's tests, `filter` narrowing them where
/// the runner can.
pub fn test_command(
    kind: Kind,
    dir: &Path,
    filter: Option<&str>,
    offline: bool,
) -> Option<(&'static str, String)> {
    let quoted = filter.map(shell_quote);
    let offline = if offline { " --offline" } else { "" };
    match kind {
        Kind::Rust => Some((
            "cargo test",
            format!(
                "cargo test --color=never{offline}{}",
                quoted.map(|f| format!(" {f}")).unwrap_or_default()
            ),
        )),
        Kind::Node => {
            let manifest: Value = std::fs::read_to_string(dir.join("package.json"))
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or(Value::Null);
            let script = manifest["scripts"]["test"].as_str().unwrap_or_default();
            if script.is_empty() || script.contains("no test specified") {
                return None;
            }
            Some((
                "npm test",
                format!(
                    "npm test --silent{}",
                    quoted.map(|f| format!(" -- {f}")).unwrap_or_default()
                ),
            ))
        }
        Kind::Python => Some((
            "pytest",
            format!(
                "python3 -m pytest -q -rfE --color=no{}",
                quoted.map(|f| format!(" -k {f}")).unwrap_or_default()
            ),
        )),
        Kind::Go => Some((
            "go test",
            format!(
                "go test ./...{}",
                quoted.map(|f| format!(" -run {f}")).unwrap_or_default()
            ),
        )),
        Kind::Make => Some(("make test", "make test".into())),
    }
}

/// `text` quoted for `/bin/sh`.
pub fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// One problem a checker found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// The file, as the checker named it.
    pub file: String,
    /// Its line.
    pub line: u32,
    /// Its column, if the checker gave one.
    pub column: Option<u32>,
    /// `error`, `warning`, `note` or `lint`.
    pub severity: String,
    /// The checker's code for it (`E0308`, `TS2322`, `F401`), if any.
    pub code: Option<String>,
    /// What it says.
    pub message: String,
}

/// The problems in a checker's output: rustc's and cargo's short format,
/// tsc's, mypy's, ruff's concise one, Go's, and Python's compile errors.
pub fn findings(output: &str) -> Vec<Finding> {
    let patterns = [
        // rustc --message-format=short
        (
            r"^(?P<file>[^\s:][^:]*):(?P<line>\d+):(?P<col>\d+): (?P<sev>error|warning|note|help)(?:\[(?P<code>[^\]]+)\])?: (?P<msg>.+)$",
            None,
        ),
        // tsc --pretty false
        (
            r"^(?P<file>[^\s(][^(]*)\((?P<line>\d+),(?P<col>\d+)\): (?P<sev>error|warning) (?P<code>TS\d+): (?P<msg>.+)$",
            None,
        ),
        // mypy
        (
            r"^(?P<file>[^\s:][^:]*\.pyi?):(?P<line>\d+):(?:(?P<col>\d+):)? (?P<sev>error|warning|note): (?P<msg>.+?)(?:  \[(?P<code>[a-z-]+)\])?$",
            None,
        ),
        // ruff --output-format=concise
        (
            r"^(?P<file>[^\s:][^:]*):(?P<line>\d+):(?P<col>\d+): (?P<code>[A-Z]+[0-9]+) (?:\[\*\] )?(?P<msg>.+)$",
            Some("lint"),
        ),
        // go vet, go build
        (
            r"^(?:vet: )?(?P<file>[^\s:][^:]*\.go):(?P<line>\d+):(?P<col>\d+): (?P<msg>.+)$",
            Some("error"),
        ),
    ];
    let compiled: Vec<(Regex, Option<&str>)> = patterns
        .iter()
        .map(|(p, s)| (Regex::new(p).expect("a valid pattern"), *s))
        .collect();
    let python_file =
        Regex::new(r#"^\s*File "(?P<file>[^"]+)", line (?P<line>\d+)"#).expect("a valid pattern");
    let python_error =
        Regex::new(r"^(?P<kind>[A-Za-z]+Error): (?P<msg>.+)$").expect("a valid pattern");
    let mut found = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let lines: Vec<&str> = output.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let line = line.trim_end();
        let mut finding = None;
        for (regex, severity) in &compiled {
            if let Some(c) = regex.captures(line) {
                finding = Some(Finding {
                    file: c["file"].trim().to_string(),
                    line: c["line"].parse().unwrap_or(0),
                    column: c.name("col").and_then(|m| m.as_str().parse().ok()),
                    severity: c
                        .name("sev")
                        .map(|m| m.as_str().to_string())
                        .or_else(|| severity.map(str::to_string))
                        .unwrap_or_else(|| "error".into()),
                    code: c.name("code").map(|m| m.as_str().to_string()),
                    message: c["msg"].trim().to_string(),
                });
                break;
            }
        }
        if finding.is_none() {
            if let Some(c) = python_file.captures(line) {
                // The error follows a few lines on.
                if let Some(error) = lines[i + 1..]
                    .iter()
                    .take(4)
                    .find_map(|l| python_error.captures(l.trim()))
                {
                    finding = Some(Finding {
                        file: c["file"].trim_start_matches("./").to_string(),
                        line: c["line"].parse().unwrap_or(0),
                        column: None,
                        severity: "error".into(),
                        code: Some(error["kind"].to_string()),
                        message: error["msg"].trim().to_string(),
                    });
                }
            }
        }
        if let Some(finding) = finding {
            let key = format!(
                "{}:{}:{:?}:{}",
                finding.file, finding.line, finding.column, finding.message
            );
            if seen.insert(key) {
                found.push(finding);
            }
        }
    }
    found
}

/// The problems as the model reads them, with counts.
pub fn render_findings(
    tool: &str,
    findings: &[Finding],
    exit: Option<i32>,
    output: &str,
) -> String {
    let count = |severity: &str| findings.iter().filter(|f| f.severity == severity).count();
    let (errors, warnings, lints) = (count("error"), count("warning"), count("lint"));
    if findings.is_empty() {
        return if exit == Some(0) {
            format!("{tool} found no problems.")
        } else {
            format!(
                "{tool} failed ({}) and named no problem the tool could read; the end of what it \
                 printed:\n{}",
                exit.map(|c| format!("exit status {c}"))
                    .unwrap_or_else(|| "stopped".into()),
                tail(output, 40)
            )
        };
    }
    let mut counts = Vec::new();
    for (n, word) in [
        (errors, "error"),
        (warnings, "warning"),
        (lints, "lint finding"),
    ] {
        if n > 0 {
            counts.push(format!("{n} {word}{}", if n == 1 { "" } else { "s" }));
        }
    }
    let notes = findings.len() - errors - warnings - lints;
    if notes > 0 {
        counts.push(format!("{notes} note{}", if notes == 1 { "" } else { "s" }));
    }
    let mut text = format!("{tool}: {}\n", counts.join(", "));
    for f in findings.iter().take(MAX_FINDINGS) {
        text.push_str(&format!(
            "{}:{}{}: {}{}: {}\n",
            f.file,
            f.line,
            f.column.map(|c| format!(":{c}")).unwrap_or_default(),
            f.severity,
            f.code
                .as_ref()
                .map(|c| format!("[{c}]"))
                .unwrap_or_default(),
            f.message
        ));
    }
    if findings.len() > MAX_FINDINGS {
        text.push_str(&format!("[{} more]\n", findings.len() - MAX_FINDINGS));
    }
    text
}

/// What a test run says: its counts and the tests that failed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tested {
    /// Tests that passed.
    pub passed: u64,
    /// Tests that failed (errors included).
    pub failed: u64,
    /// Tests skipped or ignored.
    pub skipped: u64,
    /// The names of those that failed.
    pub failures: Vec<String>,
    /// Whether the output had counts this could read.
    pub counted: bool,
}

/// What a test runner's output says: cargo test's, pytest's, go test's,
/// Jest's and Vitest's summaries.
pub fn tested(output: &str) -> Tested {
    let mut t = Tested::default();
    let cargo =
        Regex::new(r"^test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored")
            .expect("a valid pattern");
    let cargo_failed = Regex::new(r"^test (\S+) \.\.\. FAILED$").expect("a valid pattern");
    let cargo_section = Regex::new(r"^---- (\S+) stdout ----$").expect("a valid pattern");
    let pytest =
        Regex::new(r"^=*\s*(?P<summary>(?:\d+ \w+(?:, )?)+) in [\d.]+s").expect("a valid pattern");
    let pytest_failed = Regex::new(r"^(?:FAILED|ERROR) (\S+)").expect("a valid pattern");
    let counted = Regex::new(
        r"(\d+) (passed|failed|skipped|errors?|xfailed|xpassed|todo|ignored|deselected)",
    )
    .expect("a valid pattern");
    let go_failed = Regex::new(r"^\s*--- FAIL: (\S+)").expect("a valid pattern");
    let go_passed = Regex::new(r"^\s*--- PASS: ").expect("a valid pattern");
    let go_package = Regex::new(r"^(ok|FAIL)\s+\S+").expect("a valid pattern");
    let jest = Regex::new(r"^Tests:\s+(.+)$").expect("a valid pattern");
    let vitest = Regex::new(r"^\s*Tests\s+(.+)\(\d+\)").expect("a valid pattern");
    let jest_failed = Regex::new(r"^\s*● (.+)$").expect("a valid pattern");
    let vitest_failed =
        Regex::new(r"^\s*(?:×|✗|FAIL)\s+(.+?)(?:\s+\d+ms)?$").expect("a valid pattern");
    let mut go_counts = (0u64, 0u64);
    let mut go_seen = false;
    let add = |t: &mut Tested, summary: &str| {
        for c in counted.captures_iter(summary) {
            let n: u64 = c[1].parse().unwrap_or(0);
            match &c[2] {
                "passed" | "xpassed" => t.passed += n,
                "failed" | "error" | "errors" => t.failed += n,
                _ => t.skipped += n,
            }
        }
        t.counted = true;
    };
    for line in output.lines() {
        let line = line.trim_end();
        if let Some(c) = cargo.captures(line) {
            t.passed += c[1].parse::<u64>().unwrap_or(0);
            t.failed += c[2].parse::<u64>().unwrap_or(0);
            t.skipped += c[3].parse::<u64>().unwrap_or(0);
            t.counted = true;
        } else if let Some(c) = cargo_failed
            .captures(line)
            .or_else(|| cargo_section.captures(line))
        {
            t.failures.push(c[1].to_string());
        } else if let Some(c) = pytest.captures(line) {
            add(&mut t, &c["summary"]);
        } else if let Some(c) = pytest_failed.captures(line) {
            t.failures.push(c[1].to_string());
        } else if let Some(c) = go_failed.captures(line) {
            t.failures.push(c[1].to_string());
            go_counts.1 += 1;
            go_seen = true;
        } else if go_passed.is_match(line) {
            go_counts.0 += 1;
            go_seen = true;
        } else if go_package.is_match(line) {
            go_seen = true;
        } else if let Some(c) = jest.captures(line).or_else(|| vitest.captures(line)) {
            add(&mut t, &c[1]);
        } else if let Some(c) = jest_failed
            .captures(line)
            .or_else(|| vitest_failed.captures(line))
        {
            t.failures.push(c[1].trim().to_string());
        }
    }
    if go_seen && !t.counted {
        t.passed = go_counts.0;
        t.failed = go_counts.1;
        t.counted = go_counts.0 + go_counts.1 > 0;
    }
    let mut seen = std::collections::BTreeSet::new();
    t.failures.retain(|name| seen.insert(name.clone()));
    t
}

/// The last `n` lines of `text`.
fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    let mut shown = lines[start..].join("\n");
    if start > 0 {
        shown = format!("[… {start} lines before]\n{shown}");
    }
    shown
}

/// The script that runs `command` keeping only the end of what it prints
/// (`bytes` of it), so that a long run's summary is not cut off.
fn keeping_the_end(command: &str, bytes: usize) -> String {
    format!(
        "log=$(mktemp 2>/dev/null) || log=.sion-run.log\n\
         {{ {command}\n}} > \"$log\" 2>&1\n\
         status=$?\n\
         tail -c {bytes} \"$log\"\n\
         rm -f \"$log\"\n\
         exit $status\n"
    )
}

/// Where in the sandbox the directory `relative` of the workspace is.
fn sandbox_dir(relative: &str) -> String {
    let relative = relative.trim_matches('/');
    if relative.is_empty() {
        WORKSPACE.to_string()
    } else {
        format!("{WORKSPACE}/{relative}")
    }
}

/// Runs the project's checker in the sandbox and lists the problems it found.
#[derive(Debug, Clone)]
pub struct Diagnostics {
    /// The workspace.
    pub root: PathBuf,
    /// Where it runs: the shell's sandbox.
    pub sandbox: Sandbox,
}

/// Runs the project's tests in the sandbox and says what failed.
#[derive(Debug, Clone)]
pub struct RunTests {
    /// The workspace.
    pub root: PathBuf,
    /// Where they run: the shell's sandbox.
    pub sandbox: Sandbox,
}

/// The directory of the workspace a call names, checked.
fn project_dir(root: &Path, input: &Value) -> Result<(String, PathBuf), String> {
    let relative = input["path"]
        .as_str()
        .unwrap_or_default()
        .trim_matches('/')
        .to_string();
    let dir = inside(root, &relative)?;
    if !dir.is_dir() {
        return Err(format!("{relative} is not a directory of the workspace"));
    }
    Ok((relative, dir))
}

fn run_in(
    sandbox: &Sandbox,
    root: &Path,
    relative: &str,
    command: &str,
) -> Result<crate::sandbox::Run, String> {
    let keep = sandbox.limits.output_bytes.saturating_sub(512).max(4096);
    sandbox
        .run(
            &[Mount::workspace(root)],
            &sandbox_dir(relative),
            &keeping_the_end(command, keep),
        )
        .map_err(|e| format!("the sandbox could not run it: {e}"))
}

impl Tool for Diagnostics {
    fn irreversible(&self, input: &Value) -> Option<String> {
        // A command given in place of the project's own runs as the shell's.
        crate::policy::irreversible_command(input["command"].as_str()?)
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "diagnostics".into(),
            description: "Check the project in your workspace (or in `path`, a directory of it) \
                          with its own checker, found from its files: cargo check (Cargo.toml), \
                          tsc (tsconfig.json), ruff or Python's compiler (pyproject.toml and the \
                          like), go vet (go.mod). Runs in your sandbox and lists each problem as \
                          file:line:column: severity[code]: message. `command` runs another \
                          checker instead, its output read the same way."
                .into(),
            input_schema: schema(
                json!({"path": {"type": "string"}, "command": {"type": "string"}}),
                &[],
            ),
        }
    }

    fn effect(&self) -> Effect {
        if self.sandbox.network.reaches_out() {
            Effect::External
        } else {
            Effect::Write
        }
    }

    fn journaled(&self) -> bool {
        false
    }

    fn quarantinable(&self) -> bool {
        false
    }

    fn call(&self, input: &Value, _key: &str) -> Outcome {
        let (relative, dir) = match project_dir(&self.root, input) {
            Ok(found) => found,
            Err(e) => return Outcome::error(e),
        };
        let offline = !self.sandbox.network.reaches_out();
        let (tool, command) = match input["command"].as_str().filter(|c| !c.trim().is_empty()) {
            Some(command) if command.len() <= crate::builtin::MAX_COMMAND => {
                ("the checker", command.to_string())
            }
            Some(_) => return Outcome::error("the command is too long"),
            None => match kind(&dir).and_then(|k| check_command(k, offline)) {
                Some((tool, command)) => (tool, command),
                None => {
                    return Outcome::error(
                        "no checker found for this directory (no Cargo.toml, tsconfig.json, \
                         package.json, pyproject.toml or go.mod): give `command`",
                    )
                }
            },
        };
        match run_in(&self.sandbox, &self.root, &relative, &command) {
            Ok(run) => {
                let output = format!("{}{}", run.stdout, run.stderr);
                let text = render_findings(
                    tool,
                    &findings(&output),
                    if run.timed_out { None } else { run.exit },
                    &output,
                );
                let outcome = if run.timed_out {
                    Outcome::error(format!("{tool} ran longer than its time limit\n{text}"))
                } else {
                    Outcome::ok(text)
                };
                let outcome = if self.sandbox.network.reaches_out() {
                    outcome.from_outside()
                } else {
                    outcome
                };
                // What it prints of a file from outside or a confidential
                // one is so.
                crate::provenance::marked_after(&self.root, &run.displaced, outcome)
            }
            Err(e) => Outcome::error(e),
        }
    }
}

impl Tool for RunTests {
    fn irreversible(&self, input: &Value) -> Option<String> {
        // A command given in place of the project's own runs as the shell's.
        crate::policy::irreversible_command(input["command"].as_str()?)
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "run_tests".into(),
            description: "Run the tests of the project in your workspace (or in `path`, a \
                          directory of it) with its own runner, found from its files: cargo test, \
                          npm test, pytest, go test or make test; `filter` runs only the tests \
                          whose names match it. Runs in your sandbox and says how many passed and \
                          failed, which failed, and the end of the output. `command` runs \
                          another test command instead."
                .into(),
            input_schema: schema(
                json!({"path": {"type": "string"}, "filter": {"type": "string"}, "command": {"type": "string"}}),
                &[],
            ),
        }
    }

    fn effect(&self) -> Effect {
        if self.sandbox.network.reaches_out() {
            Effect::External
        } else {
            Effect::Write
        }
    }

    fn journaled(&self) -> bool {
        false
    }

    fn quarantinable(&self) -> bool {
        false
    }

    fn call(&self, input: &Value, _key: &str) -> Outcome {
        let (relative, dir) = match project_dir(&self.root, input) {
            Ok(found) => found,
            Err(e) => return Outcome::error(e),
        };
        let offline = !self.sandbox.network.reaches_out();
        let filter = input["filter"].as_str().filter(|f| !f.trim().is_empty());
        let (runner, command) = match input["command"].as_str().filter(|c| !c.trim().is_empty()) {
            Some(command) if command.len() <= crate::builtin::MAX_COMMAND => {
                ("the test command", command.to_string())
            }
            Some(_) => return Outcome::error("the command is too long"),
            None => match kind(&dir).and_then(|k| test_command(k, &dir, filter, offline)) {
                Some(found) => found,
                None => {
                    return Outcome::error(
                        "no test runner found for this directory (no Cargo.toml, package.json \
                         with a test script, pytest configuration, go.mod or Makefile with a \
                         test target): give `command`",
                    )
                }
            },
        };
        let run = match run_in(&self.sandbox, &self.root, &relative, &command) {
            Ok(run) => run,
            Err(e) => return Outcome::error(e),
        };
        let output = format!("{}{}", run.stdout, run.stderr);
        let t = tested(&output);
        let status = if run.timed_out {
            "stopped: it ran longer than its time limit".to_string()
        } else {
            match run.exit {
                Some(0) => "exit status 0".into(),
                Some(code) => format!("exit status {code}"),
                None => "killed by a signal".into(),
            }
        };
        let mut text = if t.counted {
            format!(
                "{runner}: {} passed, {} failed{} ({status})\n",
                t.passed,
                t.failed,
                if t.skipped > 0 {
                    format!(", {} skipped", t.skipped)
                } else {
                    String::new()
                }
            )
        } else {
            format!("{runner}: {status}; no counts the tool could read\n")
        };
        if !t.failures.is_empty() {
            text.push_str("failed:\n");
            for name in t.failures.iter().take(50) {
                text.push_str(&format!("  {name}\n"));
            }
        }
        let lines = if run.succeeded() { 15 } else { 80 };
        text.push_str(&format!(
            "\nthe end of the output:\n{}",
            tail(&output, lines)
        ));
        let outcome = if run.timed_out {
            Outcome::error(text)
        } else {
            Outcome::ok(text)
        };
        let outcome = if self.sandbox.network.reaches_out() {
            outcome.from_outside()
        } else {
            outcome
        };
        // What it prints of a file from outside or a confidential one is so.
        crate::provenance::marked_after(&self.root, &run.displaced, outcome)
    }
}

/// The editing tools for the workspace `root`: `edit_file`, `apply_patch`,
/// `find_files` and `search_files`.
pub fn editing(root: &Path, confidential: &[String]) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(EditFile {
            root: root.to_path_buf(),
            confidential: confidential.to_vec(),
        }),
        Box::new(ApplyPatch {
            root: root.to_path_buf(),
            confidential: confidential.to_vec(),
        }),
        Box::new(FindFiles {
            root: root.to_path_buf(),
        }),
        Box::new(SearchFiles {
            root: root.to_path_buf(),
            confidential: confidential.to_vec(),
        }),
    ]
}

/// The checking tools, run in `sandbox`: `diagnostics` and `run_tests`.
pub fn checking(root: &Path, sandbox: &Sandbox) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(Diagnostics {
            root: root.to_path_buf(),
            sandbox: sandbox.clone(),
        }),
        Box::new(RunTests {
            root: root.to_path_buf(),
            sandbox: sandbox.clone(),
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sion-code-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn edits_replace_what_is_quoted_once() {
        let root = temp("edit");
        std::fs::write(
            root.join("a.rs"),
            "fn main() {\n    let x = 1;\n    let y = 1;\n}\n",
        )
        .unwrap();
        // Twice in the file: refused, with where.
        let twice = edit(&root, "a.rs", " = 1;", " = 2;", false).unwrap_err();
        assert!(
            twice.contains("occurs 2 times") && twice.contains("lines 2, 3"),
            "{twice}"
        );
        let done = edit(&root, "a.rs", "let y = 1;", "let y = 2;", false).unwrap();
        assert!(
            done.contains("1 occurrence") && done.contains("    3      let y = 2;"),
            "{done}"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("a.rs")).unwrap(),
            "fn main() {\n    let x = 1;\n    let y = 2;\n}\n"
        );
        // Everywhere, when asked.
        edit(&root, "a.rs", "let", "const", true).unwrap();
        assert!(
            std::fs::read_to_string(root.join("a.rs"))
                .unwrap()
                .matches("const")
                .count()
                == 2
        );
        // Not there: where it nearly is.
        let spaced = edit(&root, "a.rs", "const x =  1;", "x", false).unwrap_err();
        assert!(spaced.contains("line 2 with other spacing"), "{spaced}");
        assert!(edit(&root, "a.rs", "nothing", "x", false)
            .unwrap_err()
            .contains("not in a.rs"));
        assert!(edit(&root, "../a.rs", "a", "b", false).is_err());
        assert!(edit(&root, "a.rs", "x", "x", false).is_err());
        // CRLF files are matched as the model writes them.
        std::fs::write(root.join("w.txt"), "one\r\ntwo\r\nthree\r\n").unwrap();
        edit(&root, "w.txt", "one\ntwo", "uno\ndos", false).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("w.txt")).unwrap(),
            "uno\r\ndos\r\nthree\r\n"
        );
        // A confidential file's edit is held.
        let tool = EditFile {
            root: root.clone(),
            confidential: vec!["w.txt".into()],
        };
        assert!(
            tool.call(
                &json!({"path": "w.txt", "old_string": "uno", "new_string": "un"}),
                "k"
            )
            .confidential
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_patch_with_no_context_inserts_after_its_line_and_keeps_modes() {
        let root = temp("patch-u0");
        std::fs::write(root.join("x.txt"), "a\nb\nc\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.join("x.txt"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        let changes =
            parse_patch("--- a/x.txt\n+++ b/x.txt\n@@ -2,0 +3,1 @@\n+inserted\n").unwrap();
        apply(&root, &changes).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("x.txt")).unwrap(),
            "a\nb\ninserted\nc\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(root.join("x.txt"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755, "a script stays one");
        }
        // A patch that fails part of the way changes nothing.
        let changes = parse_patch(
            "--- a/x.txt\n+++ b/x.txt\n@@ -1 +1 @@\n-a\n+A\n--- a/y.txt\n+++ b/y.txt\n@@ -1 +1 @@\n-y\n+Y\n",
        )
        .unwrap();
        assert!(apply(&root, &changes).is_err());
        assert!(std::fs::read_to_string(root.join("x.txt"))
            .unwrap()
            .starts_with("a\n"));
        let left: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".partial"))
            .collect();
        assert!(left.is_empty());
    }

    #[test]
    fn unified_diffs_apply_by_their_lines_whatever_their_numbers() {
        let root = temp("diff");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "a\nb\nc\nd\ne\nf\ng\n").unwrap();
        std::fs::write(root.join("old.txt"), "gone\n").unwrap();
        // The header's numbers are wrong; the lines are right.
        let patch = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -40,3 +40,3 @@\n b\n-c\n+C\n d\n@@ -6,2 +6,3 @@ fn x\n f\n+f2\n g\n--- /dev/null\n+++ b/new.md\n@@ -0,0 +1,2 @@\n+# New\n+text\n--- a/old.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-gone\n";
        let tool = ApplyPatch {
            root: root.clone(),
            confidential: vec![],
        };
        let outcome = tool.call(&json!({"patch": patch}), "k");
        assert!(!outcome.is_error, "{}", outcome.content);
        assert!(
            outcome.content.contains("M src/lib.rs (2 hunks, +2 −1)"),
            "{}",
            outcome.content
        );
        assert_eq!(
            std::fs::read_to_string(root.join("src/lib.rs")).unwrap(),
            "a\nb\nC\nd\ne\nf\nf2\ng\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("new.md")).unwrap(),
            "# New\ntext\n"
        );
        assert!(!root.join("old.txt").exists());
        // A hunk that does not fit changes nothing, in any file.
        std::fs::write(root.join("x.txt"), "1\n2\n").unwrap();
        let bad = "--- a/x.txt\n+++ b/x.txt\n@@ -1 +1 @@\n-1\n+one\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-zzz\n+y\n";
        let failed = tool.call(&json!({"patch": bad}), "k");
        assert!(
            failed.is_error
                && failed.content.contains("hunk 1 of 1")
                && failed.content.contains("zzz"),
            "{}",
            failed.content
        );
        assert_eq!(
            std::fs::read_to_string(root.join("x.txt")).unwrap(),
            "1\n2\n"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn begin_patch_format_adds_updates_moves_and_deletes() {
        let root = temp("begin");
        std::fs::write(
            root.join("app.py"),
            "def a():\n    return 1\n\ndef b():\n    return 1\n",
        )
        .unwrap();
        std::fs::write(root.join("rm.txt"), "x\n").unwrap();
        let patch = "*** Begin Patch\n*** Update File: app.py\n*** Move to: main.py\n@@ def b():\n-    return 1\n+    return 2\n*** Add File: notes.txt\n+hello\n*** Delete File: rm.txt\n*** End Patch\n";
        let summary = apply(&root, &parse_patch(patch).unwrap()).unwrap();
        assert!(summary.contains("R app.py → main.py"), "{summary}");
        // The anchor put the change in b, not in a.
        assert_eq!(
            std::fs::read_to_string(root.join("main.py")).unwrap(),
            "def a():\n    return 1\n\ndef b():\n    return 2\n"
        );
        assert!(!root.join("app.py").exists() && !root.join("rm.txt").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("notes.txt")).unwrap(),
            "hello\n"
        );
        assert!(parse_patch("*** Begin Patch\n*** Update File: a\n")
            .unwrap_err()
            .contains("End Patch"));
        assert!(parse_patch("just words").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn globs_find_files_and_searches_find_lines() {
        let root = temp("find");
        for (path, text) in [
            ("src/main.rs", "fn main() {\n    todo!();\n}\n"),
            ("src/deep/x.rs", "// TODO: later\n"),
            ("README.md", "# Todo app\n"),
            ("node_modules/dep/index.js", "TODO\n"),
            ("bin.dat", "TO\0DO"),
        ] {
            std::fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
            std::fs::write(root.join(path), text).unwrap();
        }
        let find = FindFiles { root: root.clone() };
        let rs = find.call(&json!({"pattern": "*.rs"}), "k").content;
        assert_eq!(rs, "src/deep/x.rs\nsrc/main.rs");
        assert_eq!(
            find.call(&json!({"pattern": "src/*.rs"}), "k").content,
            "src/main.rs"
        );
        assert_eq!(
            find.call(&json!({"pattern": "**/*.{md,js}"}), "k").content,
            "README.md"
        );
        // Inside a skipped directory when the path is there.
        assert_eq!(
            find.call(&json!({"pattern": "*.js", "path": "node_modules"}), "k")
                .content,
            "node_modules/dep/index.js"
        );
        let search = SearchFiles {
            root: root.clone(),
            confidential: vec!["README.md".into()],
        };
        let hits = search.call(&json!({"pattern": "todo", "ignore_case": true}), "k");
        assert_eq!(
            hits.content,
            "README.md:1: # Todo app\nsrc/deep/x.rs:1: // TODO: later\nsrc/main.rs:2:     todo!();"
        );
        assert!(hits.confidential, "a line of a confidential file was shown");
        let only = search.call(&json!({"pattern": "TODO", "glob": "*.rs"}), "k");
        assert_eq!(only.content, "src/deep/x.rs:1: // TODO: later");
        assert!(!only.confidential);
        let around = search
            .call(
                &json!({"pattern": "todo!", "path": "src/main.rs", "context": 1}),
                "k",
            )
            .content;
        assert_eq!(
            around,
            "src/main.rs-1- fn main() {\nsrc/main.rs:2:     todo!();\nsrc/main.rs-3- }"
        );
        assert_eq!(
            search
                .call(&json!({"pattern": "fn", "files_only": true}), "k")
                .content,
            "src/main.rs"
        );
        assert!(search.call(&json!({"pattern": "("}), "k").is_error);
        assert!(glob("a{b").is_err());
        assert!(glob("[!a]?.txt").unwrap().is_match("bc.txt"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn checkers_output_becomes_problems() {
        let cargo = "   Compiling x v0.1.0\nsrc/main.rs:2:5: error[E0425]: cannot find value `y` in this scope\nsrc/lib.rs:1:5: warning: unused import: `std::fs`\nerror: could not compile `x` (bin \"x\") due to 1 previous error\n";
        let found = findings(cargo);
        assert_eq!(found.len(), 2);
        assert_eq!(
            found[0],
            Finding {
                file: "src/main.rs".into(),
                line: 2,
                column: Some(5),
                severity: "error".into(),
                code: Some("E0425".into()),
                message: "cannot find value `y` in this scope".into()
            }
        );
        let rendered = render_findings("cargo check", &found, Some(101), cargo);
        assert!(
            rendered.starts_with("cargo check: 1 error, 1 warning\nsrc/main.rs:2:5: error[E0425]:"),
            "{rendered}"
        );
        let tsc = findings(
            "src/a.ts(3,7): error TS2322: Type 'string' is not assignable to type 'number'.\n",
        );
        assert_eq!((tsc[0].line, tsc[0].code.as_deref()), (3, Some("TS2322")));
        let ruff = findings("app.py:1:8: F401 [*] `os` imported but unused\nFound 1 error.\n");
        assert_eq!(
            (ruff[0].severity.as_str(), ruff[0].code.as_deref()),
            ("lint", Some("F401"))
        );
        let mypy = findings("app.py:4: error: Incompatible return value type (got \"str\", expected \"int\")  [return-value]\n");
        assert_eq!(mypy[0].code.as_deref(), Some("return-value"));
        let go = findings("# example.com/x\n./main.go:5:2: undefined: fmt.Printn\n");
        assert_eq!((go[0].file.as_str(), go[0].line), ("./main.go", 5));
        let python = findings("*** Error compiling './bad.py'...\n  File \"./bad.py\", line 2\n    def (\n        ^\nSyntaxError: invalid syntax\n");
        assert_eq!(
            (
                python[0].file.as_str(),
                python[0].line,
                python[0].code.as_deref()
            ),
            ("bad.py", 2, Some("SyntaxError"))
        );
        assert_eq!(
            render_findings("go vet", &[], Some(0), ""),
            "go vet found no problems."
        );
        assert!(render_findings("tsc", &[], Some(2), "Cannot find module")
            .contains("Cannot find module"));
    }

    #[test]
    fn test_runners_output_becomes_counts_and_failures() {
        let cargo = "running 3 tests\ntest a::works ... ok\ntest a::fails ... FAILED\ntest b ... ignored\n\nfailures:\n\n---- a::fails stdout ----\nthread 'a::fails' panicked\n\nfailures:\n    a::fails\n\ntest result: FAILED. 1 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out\n\nrunning 2 tests\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n";
        let t = tested(cargo);
        assert_eq!((t.passed, t.failed, t.skipped, t.counted), (3, 1, 1, true));
        assert_eq!(t.failures, vec!["a::fails".to_string()]);
        let pytest =
            "..F\nFAILED tests/test_x.py::test_two - assert 1 == 2\n1 failed, 2 passed in 0.03s\n";
        let t = tested(pytest);
        assert_eq!((t.passed, t.failed), (2, 1));
        assert_eq!(t.failures, vec!["tests/test_x.py::test_two".to_string()]);
        let go = "--- FAIL: TestAdd (0.00s)\n    add_test.go:9: got 3\nFAIL\nFAIL\texample.com/x\t0.002s\nok  \texample.com/y\t0.001s\n";
        let t = tested(go);
        assert_eq!(
            (t.failed, t.failures.clone()),
            (1, vec!["TestAdd".to_string()])
        );
        let jest = "  ● math › adds\n\nTests:       1 failed, 4 passed, 5 total\n";
        let t = tested(jest);
        assert_eq!(
            (t.passed, t.failed, t.failures.clone()),
            (4, 1, vec!["math › adds".to_string()])
        );
        assert!(!tested("nothing here").counted);
    }

    #[test]
    fn projects_are_known_by_their_files() {
        let root = temp("kinds");
        assert_eq!(kind(&root), None);
        std::fs::write(
            root.join("package.json"),
            r#"{"scripts": {"test": "echo \"Error: no test specified\" && exit 1"}}"#,
        )
        .unwrap();
        assert_eq!(kind(&root), Some(Kind::Node));
        assert_eq!(test_command(Kind::Node, &root, None, true), None);
        std::fs::write(
            root.join("package.json"),
            r#"{"scripts": {"test": "vitest run"}}"#,
        )
        .unwrap();
        assert_eq!(
            test_command(Kind::Node, &root, Some("adds"), true)
                .unwrap()
                .1,
            "npm test --silent -- 'adds'"
        );
        std::fs::write(root.join("Cargo.toml"), "[package]\n").unwrap();
        assert_eq!(kind(&root), Some(Kind::Rust));
        assert_eq!(
            test_command(Kind::Rust, &root, Some("it's"), true)
                .unwrap()
                .1,
            "cargo test --color=never --offline 'it'\\''s'"
        );
        assert!(check_command(Kind::Rust, false)
            .unwrap()
            .1
            .ends_with("--color=never"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
