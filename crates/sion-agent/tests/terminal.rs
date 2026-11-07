//! Terminals that stay open between calls, in bubblewrap: the directory and
//! variables carry over, background jobs keep running and can be read, a
//! command that runs too long is stopped without its terminal, and a
//! closed terminal opens again.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;
use sion_agent::sandbox::{Backend, Sandbox};
use sion_agent::terminal::{self, Terminals, TERMINAL_CLOSE, TERMINAL_READ, TERMINAL_RUN};
use sion_agent::tools::Tool;

fn workspace(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sion-terminal-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    dir
}

/// The terminal tools over a fresh workspace, if bubblewrap works here.
fn terminals(name: &str) -> Option<(Vec<Box<dyn Tool>>, PathBuf)> {
    let mut sandbox = Sandbox::new(Backend::Bubblewrap);
    sandbox.limits.timeout_secs = 10;
    sandbox.limits.terminal_secs = 30;
    if let Err(e) = sandbox.probe() {
        eprintln!("skipped: bubblewrap does not work here: {e}");
        return None;
    }
    let root = workspace(name);
    let tools = terminal::tools(Arc::new(Terminals::new(root.clone(), sandbox)));
    Some((tools, root))
}

fn tool<'a>(tools: &'a [Box<dyn Tool>], name: &str) -> &'a dyn Tool {
    tools
        .iter()
        .find(|t| t.spec().name == name)
        .map(|t| t.as_ref())
        .unwrap()
}

#[test]
fn a_terminal_keeps_its_directory_variables_and_background_jobs() {
    let Some((tools, root)) = terminals("keeps") else {
        return;
    };
    let run = tool(&tools, TERMINAL_RUN);
    let read = tool(&tools, TERMINAL_READ);
    let close = tool(&tools, TERMINAL_CLOSE);

    let first = run.call(&json!({"command": "cd sub && export GREETING=hello"}), "k1");
    assert!(!first.is_error, "{}", first.content);
    assert!(
        first.content.contains("[a new terminal, main"),
        "{}",
        first.content
    );
    assert!(
        first.content.contains("[in /workspace/sub]"),
        "{}",
        first.content
    );

    // The next command starts where the last left off, with its variable.
    let second = run.call(
        &json!({"command": "pwd; echo \"$GREETING\" > note.txt; cat note.txt"}),
        "k2",
    );
    assert!(!second.is_error, "{}", second.content);
    assert!(
        second.content.contains("/workspace/sub\nhello\n"),
        "{}",
        second.content
    );
    assert!(
        second.content.contains("[exit status 0]"),
        "{}",
        second.content
    );
    assert_eq!(
        std::fs::read_to_string(root.join("sub/note.txt")).unwrap(),
        "hello\n"
    );

    // A failing command says its status; the terminal goes on.
    let failed = run.call(&json!({"command": "false"}), "k3");
    assert!(!failed.is_error);
    assert!(
        failed.content.contains("[exit status 1]"),
        "{}",
        failed.content
    );

    // A job left in the background keeps running and is read later.
    let job = run.call(
        &json!({"command": "(sleep 1; echo 'built at last') & echo started"}),
        "k4",
    );
    assert!(job.content.contains("started"), "{}", job.content);
    let later = read.call(&json!({"wait": 10}), "k5");
    assert!(later.content.contains("built at last"), "{}", later.content);
    let nothing = read.call(&json!({"wait": 0}), "k6");
    assert!(
        nothing.content.contains("[nothing new]"),
        "{}",
        nothing.content
    );

    // A command that runs too long is stopped; the terminal stays.
    let slow = run.call(&json!({"command": "sleep 20", "timeout": 1}), "k7");
    assert!(slow.is_error, "{}", slow.content);
    assert!(
        slow.content.contains("stopped after 1 seconds"),
        "{}",
        slow.content
    );
    let still = run.call(&json!({"command": "echo \"$GREETING again\""}), "k8");
    assert!(still.content.contains("hello again"), "{}", still.content);

    // Another terminal is apart from the first.
    let other = run.call(
        &json!({"command": "pwd; echo \"[$GREETING]\"", "terminal": "two"}),
        "k9",
    );
    assert!(
        other.content.contains("/workspace\n[]"),
        "{}",
        other.content
    );

    // Closed, it opens anew.
    let closed = close.call(&json!({}), "k10");
    assert!(!closed.is_error, "{}", closed.content);
    let fresh = run.call(&json!({"command": "pwd"}), "k11");
    assert!(
        fresh.content.contains("[a new terminal, main"),
        "{}",
        fresh.content
    );
    assert!(fresh.content.contains("/workspace\n"), "{}", fresh.content);
    assert!(close.call(&json!({"terminal": "nope"}), "k12").is_error);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_server_started_in_the_background_answers_later_commands() {
    let Some((tools, root)) = terminals("server") else {
        return;
    };
    if !std::path::Path::new("/usr/bin/python3").exists() {
        eprintln!("skipped: no python3");
        return;
    }
    let run = tool(&tools, TERMINAL_RUN);
    std::fs::write(root.join("index.html"), "served from the workspace\n").unwrap();
    let started = run.call(
        &json!({"command": "nohup python3 -m http.server 8931 --bind 127.0.0.1 > /tmp/server.log 2>&1 & echo $! > /tmp/server.pid; echo up"}),
        "s1",
    );
    assert!(started.content.contains("up"), "{}", started.content);
    let fetched = run.call(
        &json!({"command": "for i in 1 2 3 4 5 6 7 8 9 10; do python3 -c \"import urllib.request;print(urllib.request.urlopen('http://127.0.0.1:8931/index.html').read().decode(), end='')\" 2>/dev/null && break; sleep 0.5; done", "timeout": 20}),
        "s2",
    );
    assert!(
        fetched.content.contains("served from the workspace"),
        "{}",
        fetched.content
    );
    let _ = std::fs::remove_dir_all(root);
}
