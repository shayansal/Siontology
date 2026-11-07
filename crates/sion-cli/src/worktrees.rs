//! Git worktrees for subagents that change code. A subagent asked to work
//! in a worktree gets a branch of its parent's repository of its own,
//! checked out where nothing else works: it edits and runs commands there,
//! in its own sandbox, and subagents spawned together work side by side
//! without touching each other's files or the parent's. When it is done
//! its changes are committed on its branch, the worktree is removed and
//! the branch kept, for the parent to look at and merge.
//!
//! Git runs on this machine, not in the sandbox, so it never trusts what
//! the subagent could change: its `.git` file is not read (the worktree's
//! git directory, read when the worktree was made, is named outright), and
//! hooks, file-system monitors and signing are off for its commits.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The prefix of a subagent's branch.
pub const BRANCH_PREFIX: &str = "sion/";

/// A subagent's worktree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    /// The repository it is of: the parent's workspace.
    pub repo: PathBuf,
    /// Where it is checked out.
    pub dir: PathBuf,
    /// Its git directory, inside the repository's.
    pub git_dir: PathBuf,
    /// Its branch.
    pub branch: String,
    /// The commit it began from.
    pub base: String,
}

/// What a subagent's worktree came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finished {
    /// The branch its work is on, if it changed anything.
    pub branch: Option<String>,
    /// What it changed, as `git diff --stat` says (its last line).
    pub stat: String,
}

/// Whether `dir` is the root of a git repository of its own (its `.git` a
/// directory), as a parent's workspace must be for worktrees.
pub fn is_repo_root(dir: &Path) -> bool {
    dir.join(".git").is_dir()
}

/// Settings that keep git from running anything a worktree's files could
/// name: no hooks, no file-system monitor, no signing, no prompts.
const SAFE: [&str; 8] = [
    "-c",
    "core.hooksPath=/dev/null",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "commit.gpgsign=false",
    "-c",
    "protocol.allow=never",
];

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(SAFE)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("git could not run: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(format!(
            "git {}: {}",
            args.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

impl Worktree {
    /// Git in the worktree, its git directory and files named outright.
    fn git(&self, args: &[&str]) -> Result<String, String> {
        let output = Command::new("git")
            .arg("--git-dir")
            .arg(&self.git_dir)
            .arg("--work-tree")
            .arg(&self.dir)
            .args(SAFE)
            .args(args)
            .current_dir(&self.repo)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .map_err(|e| format!("git could not run: {e}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(format!(
                "git {}: {}",
                args.first().copied().unwrap_or_default(),
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }
}

/// A random name: 8 random bytes in hex.
fn random_name() -> Result<String, String> {
    let mut bytes = [0u8; 8];
    sion_http::random(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Where worktrees are made: `sion-worktrees` in this user's cache
/// (`XDG_CACHE_HOME`, or `~/.cache`), readable by them alone, away from the
/// Siontology home and from the repository, and not in a directory others
/// share, where another user could have made it first.
fn worktrees_dir() -> Result<PathBuf, String> {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .ok_or("there is no home to keep worktrees in: set HOME")?;
    let dir = cache.join("sion-worktrees");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let made = std::fs::symlink_metadata(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if made.file_type().is_symlink() || !made.is_dir() {
        return Err(format!(
            "{} is not a directory of its own: remove it",
            dir.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let this = dir.join(random_name()?);
    std::fs::create_dir(&this).map_err(|e| format!("{}: {e}", this.display()))?;
    Ok(this)
}

/// Makes a worktree of `repo` for the subagent `name`, on a new branch
/// `sion/<name>` (with a number after it if that is taken) from the
/// repository's HEAD.
pub fn create(repo: &Path, name: &str) -> Result<Worktree, String> {
    if !is_repo_root(repo) {
        return Err(format!(
            "{} is not the root of a git repository",
            repo.display()
        ));
    }
    let base = git(repo, &["rev-parse", "--verify", "HEAD"])
        .map_err(|_| "the repository has no commit to branch from".to_string())?;
    let mut branch = format!("{BRANCH_PREFIX}{name}");
    let mut n = 1;
    while git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok()
    {
        n += 1;
        branch = format!("{BRANCH_PREFIX}{name}-{n}");
    }
    let dir = worktrees_dir()?.join(name);
    let path = dir.to_string_lossy().to_string();
    git(
        repo,
        &["worktree", "add", "--quiet", "-b", &branch, &path, &base],
    )?;
    // Its git directory, read now, before anything else can write there.
    let pointer = std::fs::read_to_string(dir.join(".git"))
        .map_err(|e| format!("the worktree has no .git file: {e}"))?;
    let git_dir = pointer
        .trim()
        .strip_prefix("gitdir:")
        .map(|path| PathBuf::from(path.trim()))
        .ok_or("the worktree's .git file names no git directory")?;
    let common = std::fs::canonicalize(repo.join(".git")).map_err(|e| e.to_string())?;
    let git_dir = std::fs::canonicalize(&git_dir).map_err(|e| e.to_string())?;
    if !git_dir.starts_with(&common) {
        return Err("the worktree's git directory is not the repository's".into());
    }
    Ok(Worktree {
        repo: repo.to_path_buf(),
        dir,
        git_dir,
        branch,
        base,
    })
}

impl Worktree {
    /// Commits what was changed in it on its branch, with `message`, by
    /// `author` (a name), and removes it: the branch stays if it changed
    /// anything, and goes if not.
    pub fn finish(&self, message: &str, author: &str) -> Result<Finished, String> {
        self.git(&["add", "--all"])?;
        let changed = self.git(&["diff", "--cached", "--quiet"]).is_err();
        if changed {
            let name = format!("user.name=sion subagent {author}");
            let email = format!("user.email={author}@subagents.sion.invalid");
            self.git(&[
                "-c",
                &name,
                "-c",
                &email,
                "commit",
                "--quiet",
                "--no-verify",
                "-m",
                message,
            ])?;
        }
        let stat = if changed {
            let stat = self.git(&["diff", "--stat", &self.base, "HEAD"])?;
            stat.lines().last().unwrap_or_default().trim().to_string()
        } else {
            String::new()
        };
        self.remove();
        if !changed {
            let _ = git(&self.repo, &["branch", "-D", &self.branch]);
        }
        Ok(Finished {
            branch: changed.then(|| self.branch.clone()),
            stat,
        })
    }

    /// Removes it, its branch kept.
    pub fn remove(&self) {
        let path = self.dir.to_string_lossy().to_string();
        let _ = git(&self.repo, &["worktree", "remove", "--force", &path]);
        if self.dir.exists() {
            let _ = std::fs::remove_dir_all(&self.dir);
            let _ = git(&self.repo, &["worktree", "prune"]);
        }
        if let Some(parent) = self.dir.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
}

/// What `branch` changed since `base`, in `repo`, as `git diff` shows it,
/// cut to about `max` characters.
pub fn diff(repo: &Path, base: &str, branch: &str, max: usize) -> Result<String, String> {
    let diff = git(repo, &["diff", "--no-color", "--no-ext-diff", base, branch])?;
    let count = diff.chars().count();
    if count <= max {
        return Ok(diff);
    }
    let kept: String = diff.chars().take(max).collect();
    Ok(format!(
        "{kept}\n[… {} characters more: git diff {base} {branch}]",
        count - max
    ))
}

/// Merges `branch` into what `repo` has checked out (fast-forward when it
/// can), as `author`'s merge: a merge that does not go through is undone,
/// and why is said.
pub fn merge(repo: &Path, branch: &str, author: &str) -> Result<String, String> {
    if !branch.starts_with(BRANCH_PREFIX) {
        return Err(format!("{branch} is not a branch sion made"));
    }
    let name = format!("user.name=sion {author}");
    let email = format!("user.email={author}@agents.sion.invalid");
    let merged = git(
        repo,
        &[
            "-c",
            &name,
            "-c",
            &email,
            "merge",
            "--no-edit",
            "--no-verify",
            "--quiet",
            branch,
        ],
    );
    match merged {
        Ok(_) => Ok(format!("merged branch {branch} into the workspace")),
        Err(why) => {
            let _ = git(repo, &["merge", "--abort"]);
            Err(format!("{why} (the merge was undone; the branch stays)"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sion-repo-{}", random_name().unwrap()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "--quiet", "-b", "main"]).unwrap();
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git(&dir, &["add", "a.txt"]).unwrap();
        git(
            &dir,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "--quiet",
                "-m",
                "first",
            ],
        )
        .unwrap();
        dir
    }

    #[test]
    fn an_attempt_s_branch_is_shown_as_a_diff_and_merged_or_its_merge_undone() {
        let repo = repo();
        let first = create(&repo, "graph-x-attempt-1").unwrap();
        let second = create(&repo, "graph-x-attempt-2").unwrap();
        std::fs::write(first.dir.join("a.txt"), "two\n").unwrap();
        std::fs::write(second.dir.join("a.txt"), "three\n").unwrap();
        let one = first.finish("Attempt 1", "team").unwrap().branch.unwrap();
        let two = second.finish("Attempt 2", "team").unwrap().branch.unwrap();
        let shown = diff(&repo, &first.base, &one, 10_000).unwrap();
        assert!(shown.contains("-one\n+two"), "{shown}");
        assert!(diff(&repo, &first.base, &one, 10)
            .unwrap()
            .contains("characters more"));
        assert_eq!(
            merge(&repo, &one, "team").unwrap(),
            format!("merged branch {one} into the workspace")
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("a.txt")).unwrap(),
            "two\n"
        );
        // The second changed the same line: its merge is undone.
        let refused = merge(&repo, &two, "team").unwrap_err();
        assert!(refused.contains("the merge was undone"), "{refused}");
        assert_eq!(
            std::fs::read_to_string(repo.join("a.txt")).unwrap(),
            "two\n"
        );
        assert_eq!(git(&repo, &["status", "--porcelain"]).unwrap(), "");
        // Only a branch sion made is merged.
        assert!(merge(&repo, "main", "team").is_err());
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn a_subagent_s_work_is_committed_on_its_branch_and_its_worktree_removed() {
        let repo = repo();
        let first = create(&repo, "fixer").unwrap();
        let second = create(&repo, "fixer").unwrap();
        assert_eq!(first.branch, "sion/fixer");
        assert_eq!(second.branch, "sion/fixer-2", "a branch of its own");
        assert_ne!(first.dir, second.dir);
        // Each works apart: the parent's files are as they were.
        std::fs::write(first.dir.join("a.txt"), "two\n").unwrap();
        std::fs::write(first.dir.join("b.txt"), "new\n").unwrap();
        // What the subagent could write cannot steer git on this machine.
        std::fs::write(first.dir.join(".git"), "gitdir: /nowhere\n").unwrap();
        std::fs::create_dir_all(first.dir.join(".husky")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let hook = first.dir.join(".husky/pre-commit");
            std::fs::write(
                &hook,
                format!("#!/bin/sh\ntouch {}\n", repo.join("hook-ran").display()),
            )
            .unwrap();
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // As husky sets it: the repository's hooks are its files'.
        git(&repo, &["config", "core.hooksPath", ".husky"]).unwrap();
        assert_eq!(
            std::fs::read_to_string(repo.join("a.txt")).unwrap(),
            "one\n"
        );
        let done = first.finish("Fix a", "fixer").unwrap();
        assert!(
            !repo.join("hook-ran").exists(),
            "no hook of the worktree's ran"
        );
        assert_eq!(done.branch.as_deref(), Some("sion/fixer"));
        assert!(done.stat.contains("files changed"), "{}", done.stat);
        assert!(!first.dir.exists(), "its worktree is removed");
        let log = git(&repo, &["log", "--format=%an|%s", "sion/fixer", "-1"]).unwrap();
        assert_eq!(log, "sion subagent fixer|Fix a");
        assert_eq!(
            git(&repo, &["show", "sion/fixer:a.txt"]).unwrap(),
            "two",
            "its work is on its branch"
        );
        // One that changed nothing leaves no branch.
        let done = second.finish("Nothing", "fixer").unwrap();
        assert_eq!(done.branch, None);
        assert!(git(
            &repo,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                "refs/heads/sion/fixer-2"
            ]
        )
        .is_err());
        assert!(create(&repo.join("missing"), "x").is_err());
        let _ = std::fs::remove_dir_all(&repo);
    }
}
