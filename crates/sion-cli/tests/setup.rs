//! One person on one machine: `sion setup` against a chain of their own
//! (`sion devnet` with one validator, pacing itself when idle), then bare
//! `sion` talking with the agent it made, `sion -c` going on with the same
//! conversation, and setup run again changing nothing it need not.
//!
//! The model is a scripted one. About half a minute on two cores.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use sion_cli::{HttpLedger, LedgerExt};

const SION: &str = env!("CARGO_BIN_EXE_sion");

/// A chain killed when it goes out of scope.
struct Chain(Child);

impl Drop for Chain {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// `sion` with `args`, as the person `dir` holds the home of, with `input`.
fn sion(dir: &Path, node: &str, args: &[&str], input: &str) -> Output {
    let mut child = Command::new(SION)
        .args(["--node", node])
        .args(args)
        .env("SION_HOME", dir.join("home"))
        .env("HOME", dir.join("user"))
        .env("USER", "ana")
        .env_remove("SION_NODE")
        .env_remove("ANTHROPIC_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn one_person_sets_up_a_chain_and_an_agent_and_talks_with_it() {
    let dir: PathBuf = std::env::temp_dir().join(format!("sion-setup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("user")).unwrap();
    let port = free_port();
    let node = format!("http://127.0.0.1:{port}");

    // Nothing is set up: bare `sion` says what to do.
    let out = sion(&dir, &node, &[], "");
    assert!(!out.status.success());
    assert!(text(&out).contains("run `sion setup`"), "{}", text(&out));

    // The chain setup would ask for, started as a person would.
    let args =
        sion_cli::setup::chain_args(&format!("127.0.0.1:{port}"), &dir.join("home/chain"), 500);
    let _chain = Chain(
        Command::new(SION)
            .args(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let ledger = HttpLedger::new(&node);
    let started = Instant::now();
    while ledger.status().is_err() {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the chain did not answer"
        );
        std::thread::sleep(Duration::from_millis(200));
    }

    let model = dir.join("assistant.json");
    std::fs::write(
        &model,
        serde_json::json!([
            {"when": "say hello", "say": "Hello. I am ready."},
            {"when": "workspace", "call": "list_files", "input": {}},
            {"say": "Your workspace is empty so far."},
            {"when": "notes", "call": "write_file", "input": {"path": "notes.md", "content": "# Notes\n"}},
            {"say": "I wrote notes.md."}
        ])
        .to_string(),
    )
    .unwrap();
    let spec = format!("script:{}", model.display());

    // No service is installed: where a service manager runs (some CI
    // machines), this test's home is not the one it reads.
    let out = sion(
        &dir,
        &node,
        &["setup", "--yes", "--no-scheduler", "--model", &spec],
        "",
    );
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    for expected in [
        "uses it",
        "$ sion keys new ana",
        "$ sion faucet ana 10000",
        "$ sion keys new ana-agent",
        "$ sion agent register ana ana-agent --lifetime 1000000000",
        "$ sion memory open ana-agent --agent",
        "it said: Hello. I am ready.",
        "profile assistant: agent",
        "Talk with your agent:",
    ] {
        assert!(said.contains(expected), "{expected:?} not in:\n{said}");
    }
    let made: sion_cli::setup::Made =
        serde_json::from_str(&std::fs::read_to_string(dir.join("home/setup.json")).unwrap())
            .unwrap();
    assert_eq!(made.profile.as_deref(), Some("assistant"));
    assert_eq!(made.wallet.as_deref(), Some("ana"));
    assert!(made.agent.is_some() && made.memory.is_some());
    // A wallet's seed is readable by its owner only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let seed = dir.join("home/wallets/ana.seed");
        let mode = std::fs::metadata(&seed).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "{}", seed.display());
    }
    assert_eq!(made.chain, None, "setup used a chain it did not make");
    let workspace = dir.join("user/sion-workspace");
    assert!(workspace.is_dir());

    // Bare `sion`: a new conversation with the profile setup made.
    let out = sion(&dir, &node, &[], "What is in your workspace?\n");
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    assert!(said.contains("Talking with assistant"), "{said}");
    assert!(said.contains("· list_files"), "{said}");
    assert!(
        said.contains("agent> Your workspace is empty so far."),
        "{said}"
    );

    // `sion -c`: the same conversation, its script where it left off.
    let out = sion(&dir, &node, &["-c"], "Write my notes down.\n");
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    assert!(said.contains("Continuing with assistant"), "{said}");
    assert!(said.contains("agent> I wrote notes.md."), "{said}");
    assert_eq!(
        std::fs::read_to_string(workspace.join("notes.md")).unwrap(),
        "# Notes\n"
    );
    let out = sion(&dir, &node, &["-c", "status"], "");
    assert!(!out.status.success());
    assert!(
        text(&out).contains("give it with no command"),
        "{}",
        text(&out)
    );

    // Again: nothing made twice, nothing funded twice.
    let out = sion(
        &dir,
        &node,
        &["setup", "--yes", "--no-check", "--no-scheduler"],
        "",
    );
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    assert!(said.contains("registered already"), "{said}");
    assert!(said.contains("opened already"), "{said}");
    assert!(!said.contains("$ sion faucet"), "{said}");
    assert!(!said.contains("$ sion keys new"), "{said}");
}
