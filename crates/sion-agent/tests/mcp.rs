//! Tools from an MCP server: a fake one, in Python, that lists tools on two
//! pages, asks the client something mid-call, prints noise, hangs and dies.
//! Skipped where there is no python3.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use serde_json::{json, Value};
use sion_agent::mcp::{self, Connection, Server};
use sion_agent::sandbox::{Backend, Sandbox};
use sion_agent::{Effect, Tool};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_mcp.py")
}

fn python() -> bool {
    let found = Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !found {
        eprintln!("skipped: no python3 here");
    }
    found
}

fn server(args: &[&str]) -> Server {
    let mut command = vec![
        "python3".to_string(),
        fixture().to_string_lossy().into_owned(),
    ];
    command.extend(args.iter().map(|a| a.to_string()));
    Server {
        name: "fake".into(),
        command,
        url: None,
        headers: Vec::new(),
        oauth: None,
        legacy_sse: false,
        env: Vec::new(),
        sandbox: None,
        dir: None,
        tools: BTreeMap::new(),
        timeout_secs: 5,
        protocol: None,
    }
}

/// `server` with every tool it lists now reviewed, and the protocol it
/// spoke.
fn reviewed(mut server: Server) -> Server {
    let mut connection = Connection::start(&server).unwrap();
    let listed = connection.tools().unwrap();
    server.tools = listed
        .iter()
        .map(|t| (t.name.clone(), t.pin(None)))
        .collect();
    server.protocol = Some(connection.protocol());
    server
}

fn tool<'a>(mounted: &'a mcp::Mounted, name: &str) -> &'a dyn Tool {
    mounted
        .tools
        .iter()
        .find(|t| t.spec().name == name)
        .unwrap_or_else(|| panic!("no tool {name}"))
        .as_ref()
}

#[test]
fn a_reviewed_server_offers_its_tools_with_their_effects() {
    if !python() {
        return;
    }
    let server = reviewed(server(&[]));
    let mut connection = Connection::start(&server).unwrap();
    assert_eq!(connection.info["name"], "fake");
    // Both pages.
    assert_eq!(connection.tools().unwrap().len(), 7);
    drop(connection);

    let mounted = mcp::mount(&server).unwrap();
    assert!(mounted.notes.is_empty(), "{:?}", mounted.notes);
    let echo = tool(&mounted, "fake_echo");
    assert_eq!(echo.effect(), Effect::Read);
    assert!(echo
        .spec()
        .description
        .contains("(from the MCP server fake)"));
    let out = echo.call(&json!({"text": "hello"}), "k");
    assert_eq!(
        (out.content.as_str(), out.is_error, out.untrusted),
        ("hello", false, false)
    );

    // A tool that says nothing of itself reaches the open world: what it
    // returns is from outside. The server pings the client mid-call.
    let send = tool(&mounted, "fake_send");
    assert_eq!(send.effect(), Effect::External);
    assert!(send.journaled());
    let sent = send.call(&json!({"text": "hi"}), "k");
    assert!(!sent.is_error && sent.untrusted, "{sent:?}");
    assert!(sent.content.starts_with("sent: hi"));

    let save = tool(&mounted, "fake_save");
    assert_eq!(save.effect(), Effect::Write);
    assert!(save.journaled());
    assert_eq!(
        save.call(&json!({"text": "a"}), "k").content,
        "1 notes saved"
    );
    assert_eq!(
        save.call(&json!({"text": "b"}), "k").content,
        "2 notes saved"
    );

    let failed = tool(&mounted, "fake_fail").call(&json!({}), "k");
    assert!(failed.is_error && failed.certain && failed.content == "no such note");
}

#[test]
fn a_modern_server_is_found_by_its_probe() {
    if !python() {
        return;
    }
    let modern = reviewed(server(&["modern"]));
    let connection = Connection::start(&modern).unwrap();
    assert_eq!(connection.era, mcp::Era::Modern);
    assert_eq!(connection.info["name"], "fake-modern");
    drop(connection);
    let mounted = mcp::mount(&modern).unwrap();
    assert!(mounted.notes.is_empty(), "{:?}", mounted.notes);
    let out = tool(&mounted, "fake_echo").call(&json!({"text": "hello"}), "k");
    assert_eq!(out.content, "hello", "{out:?}");
    // An earlier server is introduced with the handshake.
    let legacy = Connection::start(&server(&[])).unwrap();
    assert_eq!(legacy.era, mcp::Era::Legacy(mcp::LEGACY[0].into()));
    // The protocol recorded at the review is tried first; a server that no
    // longer speaks it is probed again.
    let mut moved = reviewed(server(&[]));
    assert_eq!(moved.protocol.as_deref(), Some(mcp::LEGACY[0]));
    moved.command.push("modern".into());
    let found = Connection::start(&moved).unwrap();
    assert_eq!(found.era, mcp::Era::Modern);
}

#[test]
fn a_tool_changed_since_its_review_is_not_offered() {
    if !python() {
        return;
    }
    let mut changed = reviewed(server(&[]));
    changed.command.push("changed".into());
    let mounted = mcp::mount(&changed).unwrap();
    let names: Vec<String> = mounted.tools.iter().map(|t| t.spec().name).collect();
    assert!(!names.contains(&"fake_echo".to_string()), "{names:?}");
    assert!(names.contains(&"fake_send".to_string()));
    assert!(!names.contains(&"fake_extra".to_string()));
    let notes = mounted.notes.join("\n");
    assert!(
        notes.contains("echo changed since it was reviewed"),
        "{notes}"
    );
    assert!(
        notes.contains("extra is new and was not reviewed"),
        "{notes}"
    );
}

#[test]
fn a_call_that_gets_no_answer_may_have_taken_effect() {
    if !python() {
        return;
    }
    let mut server = reviewed(server(&[]));
    server.timeout_secs = 1;
    let mounted = mcp::mount(&server).unwrap();
    let hang = tool(&mounted, "fake_hang").call(&json!({}), "k");
    assert!(hang.is_error && !hang.certain, "{hang:?}");
    assert!(hang.content.contains("did not answer within 1 seconds"));
    let died = tool(&mounted, "fake_die").call(&json!({}), "k");
    assert!(!died.certain, "{died:?}");
    // A read whose answer was lost changed nothing.
    let echo = tool(&mounted, "fake_echo").call(&json!({"text": "x"}), "k");
    assert!(echo.is_error && echo.certain, "{echo:?}");
}

#[test]
fn a_server_gets_only_the_variables_it_names() {
    if !python() {
        return;
    }
    // Cargo sets these for the tests; only the named one reaches the server.
    let names = json!({"names": ["CARGO_PKG_NAME", "CARGO_PKG_VERSION", "CARGO_MANIFEST_DIR"]});
    let mut named = reviewed(server(&[]));
    named.env = vec!["CARGO_PKG_NAME".into()];
    let mounted = mcp::mount(&named).unwrap();
    let env: Value =
        serde_json::from_str(&tool(&mounted, "fake_env").call(&names, "k").content).unwrap();
    assert_eq!(env["CARGO_PKG_NAME"], "sion-agent");
    assert_eq!(env["CARGO_PKG_VERSION"], Value::Null);
    assert_eq!(env["CARGO_MANIFEST_DIR"], Value::Null);
    let mut missing = named.clone();
    missing.env = vec!["SION_TEST_VARIABLE_NOBODY_SETS".into()];
    assert!(mcp::mount(&missing)
        .unwrap_err()
        .contains("SION_TEST_VARIABLE_NOBODY_SETS"));
}

#[test]
fn a_server_runs_in_a_sandbox_with_its_own_directory() {
    if !python() {
        return;
    }
    let sandbox = Sandbox::new(Backend::Bubblewrap);
    if let Err(e) = sandbox.probe() {
        eprintln!("skipped: bubblewrap does not work here: {e}");
        return;
    }
    let dir = std::env::temp_dir().join(format!("sion-mcp-sandboxed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(fixture(), dir.join("fake_mcp.py")).unwrap();
    let server = reviewed(Server {
        command: vec!["python3".into(), "/workspace/fake_mcp.py".into()],
        url: None,
        headers: Vec::new(),
        oauth: None,
        legacy_sse: false,
        env: vec!["CARGO_PKG_NAME".into()],
        sandbox: Some(sandbox),
        dir: Some(dir.clone()),
        ..server(&[])
    });
    let mounted = mcp::mount(&server).unwrap();
    let names = json!({"names": ["CARGO_PKG_NAME", "CARGO_PKG_VERSION", "HOME"]});
    let env: Value =
        serde_json::from_str(&tool(&mounted, "fake_env").call(&names, "k").content).unwrap();
    assert_eq!(env["CARGO_PKG_NAME"], "sion-agent");
    assert_eq!(env["CARGO_PKG_VERSION"], Value::Null);
    assert_eq!(env["HOME"], "/workspace");
    assert_eq!(env["cwd"], "/workspace");
    drop(mounted);
    std::fs::remove_dir_all(&dir).unwrap();
}
