//! Keeping agents running: `sion agent up` runs its list and starts again
//! what stops, and what it runs stops when it does, however it stops;
//! `sion deploy` puts sion and a home on a server, through a stand-in for
//! SSH that runs the server's commands in a home of its own on this
//! machine (no server, and no service manager, is reached).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

fn sion() -> &'static str {
    env!("CARGO_BIN_EXE_sion")
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sion-always-on-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(home: &Path, args: &[&str]) -> String {
    let out = Command::new(sion())
        .args(args)
        .env("SION_HOME", home)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "sion {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn alive(pid: u64) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
        && !std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .map(|s| s.split_whitespace().nth(2) == Some("Z"))
            .unwrap_or(false)
}

fn status(home: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(home.join("up.status.json")).unwrap()).unwrap()
}

#[test]
fn up_keeps_its_list_running_and_takes_it_down_with_it() {
    if !Path::new("/proc/self").exists() {
        eprintln!("skipped: no /proc here to see processes in");
        return;
    }
    let home = temp("up");
    let docs = home.join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    // One that runs until it is stopped, one that stops at once.
    run(
        &home,
        &[
            "agent",
            "up",
            "add",
            "docs",
            "--",
            "mcp",
            "documents",
            "--dir",
            docs.to_str().unwrap(),
        ],
    );
    run(
        &home,
        &["agent", "up", "add", "flap", "--", "agent", "waiting"],
    );
    let listed = run(&home, &["agent", "up", "list"]);
    assert!(
        listed.contains("docs: sion mcp documents --dir") && listed.contains("not started"),
        "{listed}"
    );

    let out = Command::new(sion())
        .args(["agent", "up", "--for-secs", "7"])
        .env("SION_HOME", &home)
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{said}");
    assert!(said.contains("flap: stopped (exit status: 0)"), "{said}");
    assert!(
        said.contains("starting again in 2 s"),
        "the pause doubles: {said}"
    );
    let after = status(&home);
    assert!(
        after["entries"]["flap"]["restarts"].as_u64().unwrap() >= 1,
        "{after}"
    );
    assert!(
        after["entries"]["docs"]["pid"].is_null(),
        "stopped at the end: {after}"
    );
    let log = std::fs::read_to_string(home.join("logs/flap.log")).unwrap();
    assert!(log.matches("Nothing needs you.").count() >= 2, "{log}");

    // Killed outright: what it ran stops by itself.
    let mut up = Command::new(sion())
        .args(["agent", "up"])
        .env("SION_HOME", &home)
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_secs(2));
    let pid = status(&home)["entries"]["docs"]["pid"].as_u64().unwrap();
    assert!(alive(pid), "the documents server runs");
    up.kill().unwrap();
    up.wait().unwrap();
    let mut gone = false;
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(100));
        if !alive(pid) {
            gone = true;
            break;
        }
    }
    assert!(gone, "process {pid} outlived sion agent up");
    let listed = run(&home, &["agent", "up", "remove", "flap"]);
    assert_eq!(listed.trim(), "removed flap");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn deploy_puts_sion_and_a_home_on_a_server_and_runs_it_as_a_service() {
    if std::env::consts::OS != "linux" {
        eprintln!("skipped: the stand-in server is this Linux machine");
        return;
    }
    let base = temp("deploy");
    let (here, server) = (base.join("here"), base.join("server"));
    let local_sion = here.join(".sion");
    std::fs::create_dir_all(local_sion.join("agents")).unwrap();
    std::fs::create_dir_all(&server).unwrap();
    // What setup made here, and a file that holds a path of this home.
    std::fs::write(
        local_sion.join("setup.json"),
        r#"{"chain": "127.0.0.1:8730", "service": true, "profile": "assistant", "scheduler": true, "telegram": false}"#,
    )
    .unwrap();
    std::fs::write(
        local_sion.join("agents/paths.json"),
        format!(r#"{{"workspace": "{}/sion-workspace"}}"#, here.display()),
    )
    .unwrap();
    // SSH's stand-in: the server's commands run here, in its own home.
    let ssh = base.join("ssh");
    std::fs::write(
        &ssh,
        format!(
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do case \"$1\" in -p|-i) shift 2;; -t) shift;; --) shift; break;; *) break;; esac; done\nshift\nexec env -i HOME={} PATH=\"$PATH\" /bin/sh -c \"$1\"\n",
            server.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let out = Command::new(sion())
        .args(["deploy", "ana@server.example", "--copy-home", "--no-start"])
        .env("HOME", &here)
        .env("SION_HOME", &local_sion)
        .env("SION_SSH", &ssh)
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{said}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(said.contains("put sion there"), "{said}");
    assert!(said.contains("its keys with it"), "{said}");
    // sion is there, and runs.
    let there = server.join(".local/bin/sion");
    let version = Command::new(&there).arg("--version").output().unwrap();
    assert!(version.status.success());
    // The home is there, its paths the server's.
    let paths = std::fs::read_to_string(server.join(".sion/agents/paths.json")).unwrap();
    assert!(
        paths.contains(&format!("{}/sion-workspace", server.display())),
        "{paths}"
    );
    // Its keys are the user's alone there.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&server.join(".sion")), 0o700);
        assert_eq!(mode(&server.join(".sion/agents/paths.json")) & 0o077, 0);
    }
    // What runs there: the chain, the scheduler, the work runner, by one
    // service of the user's.
    let list = std::fs::read_to_string(server.join(".sion/up.json")).unwrap();
    for name in ["\"chain\"", "\"scheduler\"", "\"work\""] {
        assert!(list.contains(name), "{list}");
    }
    let unit =
        std::fs::read_to_string(server.join(".config/systemd/user/sion-up.service")).unwrap();
    assert!(unit.contains("agent up"), "{unit}");
    assert!(said.contains("stop the agents on this machine"), "{said}");
    let _ = std::fs::remove_dir_all(&base);
}
