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

/// Where worktrees are made: a directory of this user's in the system's
/// temporary directory, readable by them alone, away from the
/// Siontology home and from the repository.
fn worktrees_dir() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("sion-worktrees");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
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
