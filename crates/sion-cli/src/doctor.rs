//! `sion doctor`: what this machine has for Siontology and its agents, and
//! what is missing, one line each, with what to do about it. It reads; it
//! changes nothing, and names the variables that are set without showing
//! their values.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::error::CliError;

/// One finding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// Whether it is as it should be.
    pub ok: bool,
    /// What was looked at.
    pub what: String,
    /// What was found, and what to do if anything.
    pub found: String,
}

fn finding(ok: bool, what: &str, found: impl Into<String>) -> Finding {
    Finding {
        ok,
        what: what.to_string(),
        found: found.into(),
    }
}

/// Whether `program` runs (`args` given), quietly.
fn runs(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Where `program` is on `PATH`, if it is.
fn on_path(program: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(program))
            .find(|candidate| candidate.is_file())
    })
}

/// The variables of `names` that are set (their names only).
/// The variables of `names` that are set here, those this machine keeps
/// ([`crate::credentials`]) marked so.
fn set(names: &[&str]) -> Vec<String> {
    names
        .iter()
        .filter_map(|n| {
            if std::env::var(n).is_ok_and(|v| !v.is_empty()) {
                Some(n.to_string())
            } else if crate::credentials::is_kept(n) {
                Some(format!("{n} (kept)"))
            } else {
                None
            }
        })
        .collect()
}

/// The machine's findings, with its home (`SION_HOME`) and the node at
/// `node`.
pub fn findings(node: &str) -> Vec<Finding> {
    let home = crate::session::wallet_dir()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    findings_at(&home, node)
}

/// The findings, with `home` as the home.
pub fn findings_at(home: &Path, node: &str) -> Vec<Finding> {
    let mut out = vec![finding(
        true,
        "sion",
        format!(
            "{} on {} {}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
    )];

    // Where keys, profiles and sessions are kept.
    let writable = std::fs::create_dir_all(home).is_ok() && tempfile_in(home);
    out.push(finding(
        writable,
        "home",
        if writable {
            format!("{} (SION_HOME)", home.display())
        } else {
            format!(
                "{} cannot be written: set SION_HOME to a directory of yours",
                home.display()
            )
        },
    ));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let key = home.join("agents").join("secrets.key");
        if let Ok(meta) = std::fs::metadata(&key) {
            let mode = meta.permissions().mode() & 0o777;
            out.push(finding(
                mode & 0o077 == 0,
                "secrets",
                if mode & 0o077 == 0 {
                    "the secrets' key is readable only by you".to_string()
                } else {
                    format!(
                        "{} is readable by others (mode {mode:o}): chmod 600 it",
                        key.display()
                    )
                },
            ));
        }
    }

    // The node the ledger commands talk to.
    let client = sion_http::Client::new(sion_http::Options {
        allow_private: true,
        timeout: Duration::from_secs(5),
        ..sion_http::Options::default()
    });
    let reached = client.ok().and_then(|c| {
        c.get(
            &format!("{}/v1/genesis", node.trim_end_matches('/')),
            1 << 20,
        )
        .ok()
    });
    out.push(finding(
        reached.is_some(),
        "node",
        match reached {
            Some(_) => format!("{node} answers"),
            None => format!(
                "{node} does not answer: start one (`sion setup` makes a chain of your own; `sion devnet`) or name another (--node, SION_NODE); only the ledger's commands need it"
            ),
        },
    ));

    // What sandboxes commands, skills and MCP servers.
    let bwrap = on_path("bwrap").is_some();
    let userns = bwrap
        && runs(
            "bwrap",
            &[
                "--ro-bind",
                "/",
                "/",
                "--dev",
                "/dev",
                "--unshare-all",
                "--die-with-parent",
                "true",
            ],
        );
    out.push(finding(
        userns,
        "bubblewrap",
        if userns {
            "runs commands in a sandbox of their own".to_string()
        } else if bwrap {
            "installed, but cannot make a sandbox here (user namespaces may be off): use --shell docker, or allow them".to_string()
        } else {
            "not installed (apt install bubblewrap): the shell, skills' scripts and the browser use it on Linux".to_string()
        },
    ));
    let docker = on_path("docker").is_some();
    let daemon = docker && runs("docker", &["version", "--format", "{{.Server.Version}}"]);
    out.push(finding(
        daemon || !docker,
        "docker",
        if daemon {
            "its daemon answers: Docker sandboxes are open to you".to_string()
        } else if docker {
            "installed, but its daemon does not answer: Docker sandboxes need it".to_string()
        } else {
            "not installed (only the Docker sandboxes need it)".to_string()
        },
    ));

    // The browser and the desktop.
    let browser = sion_agent::browser::find_program();
    out.push(finding(
        browser.is_some(),
        "browser",
        match browser {
            Some(path) => format!("{} (--browser)", path.display()),
            None => "no Chromium or Chrome found: install one, or set SION_BROWSER, for --browser"
                .into(),
        },
    ));
    let desktop: Vec<&str> = ["Xvfb", "xdotool", "import"]
        .into_iter()
        .filter(|p| on_path(p).is_none())
        .collect();
    out.push(finding(
        desktop.is_empty(),
        "desktop",
        if desktop.is_empty() {
            "Xvfb, xdotool and ImageMagick are here (--computer)".to_string()
        } else {
            format!(
                "missing {} (only --computer needs them)",
                desktop.join(", ")
            )
        },
    ));

    // Models, speech and messengers: which credentials are set.
    let models = set(&[
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
    ]);
    let chatgpt = crate::chatgpt::load()
        .map(|store| store.account.is_some())
        .unwrap_or(false);
    out.push(finding(
        !models.is_empty() || chatgpt,
        "models",
        match (models.is_empty(), chatgpt) {
            (true, false) => "no model's key set or kept, and no ChatGPT sign-in: agents run only scripted models (sion setup)".to_string(),
            (true, true) => "signed in with ChatGPT (chatgpt:<model>)".to_string(),
            (false, false) => format!("{} set", models.join(", ")),
            (false, true) => format!("{} set; signed in with ChatGPT", models.join(", ")),
        },
    ));
    let speech = set(&[
        "OPENAI_API_KEY",
        "GROQ_API_KEY",
        "ELEVENLABS_API_KEY",
        "DEEPGRAM_API_KEY",
    ]);
    out.push(finding(
        true,
        "voice",
        if speech.is_empty() {
            "no speech service's key set (--hear and --speak can also run a command)".to_string()
        } else {
            format!("{} set", speech.join(", "))
        },
    ));
    let messengers = [
        ("telegram", &["TELEGRAM_BOT_TOKEN"][..]),
        ("slack", &["SLACK_BOT_TOKEN"]),
        ("discord", &["DISCORD_BOT_TOKEN"]),
        ("matrix", &["MATRIX_ACCESS_TOKEN", "MATRIX_HOMESERVER"]),
        ("signal", &["SIGNAL_API_BASE", "SIGNAL_NUMBER"]),
        ("mattermost", &["MATTERMOST_URL", "MATTERMOST_TOKEN"]),
        ("irc", &["IRC_SERVER", "IRC_NICK"]),
        ("twitch", &["TWITCH_NICK", "TWITCH_TOKEN"]),
        (
            "whatsapp",
            &[
                "WHATSAPP_TOKEN",
                "WHATSAPP_PHONE_NUMBER_ID",
                "WHATSAPP_APP_SECRET",
                "WHATSAPP_VERIFY_TOKEN",
            ],
        ),
        (
            "line",
            &["LINE_CHANNEL_ACCESS_TOKEN", "LINE_CHANNEL_SECRET"],
        ),
        (
            "sms",
            &[
                "TWILIO_ACCOUNT_SID",
                "TWILIO_AUTH_TOKEN",
                "TWILIO_NUMBER",
                "TWILIO_WEBHOOK_URL",
            ],
        ),
        ("teams", &["TEAMS_APP_ID", "TEAMS_APP_PASSWORD"]),
        ("google-chat", &["GOOGLE_CHAT_AUDIENCE"]),
        (
            "dingtalk",
            &["DINGTALK_CLIENT_ID", "DINGTALK_CLIENT_SECRET"],
        ),
        (
            "feishu",
            &[
                "FEISHU_APP_ID",
                "FEISHU_APP_SECRET",
                "FEISHU_VERIFICATION_TOKEN",
            ],
        ),
        (
            "nextcloud-talk",
            &["NEXTCLOUD_URL", "NEXTCLOUD_TALK_SECRET"],
        ),
        (
            "wecom",
            &[
                "WECOM_CORP_ID",
                "WECOM_AGENT_ID",
                "WECOM_SECRET",
                "WECOM_TOKEN",
                "WECOM_AES_KEY",
            ],
        ),
        ("qq", &["QQ_APP_ID", "QQ_APP_SECRET"]),
        ("zalo", &["ZALO_BOT_TOKEN"]),
        ("ntfy", &["NTFY_URL"]),
        ("phone", &["TWILIO_AUTH_TOKEN", "TWILIO_VOICE_URL"]),
        ("x", &["X_ACCESS_TOKEN"]),
        (
            "synology-chat",
            &["SYNOLOGY_CHAT_INCOMING_URL", "SYNOLOGY_CHAT_TOKEN"],
        ),
        ("nostr", &["NOSTR_PRIVATE_KEY"]),
    ];
    let mut ready: Vec<&str> = messengers
        .iter()
        .filter(|(_, names)| set(names).len() == names.len())
        .map(|(name, _)| *name)
        .collect();
    // iMessage needs imsg, on this Mac or reached by a command of its own.
    if on_path("imsg").is_some() || std::env::var_os("IMSG_COMMAND").is_some() {
        ready.push("imessage");
    }
    // SimpleX talks to its terminal client, here or at SIMPLEX_URL.
    if on_path("simplex-chat").is_some() || std::env::var_os("SIMPLEX_URL").is_some() {
        ready.push("simplex");
    }
    // WeChat, once signed in (or given a token).
    if crate::weixin::bot().is_ok() {
        ready.push("weixin");
    }
    // Email logs in with a password or with a command's token.
    if set(&["EMAIL_ADDRESS", "EMAIL_IMAP_URL", "EMAIL_SMTP_URL"]).len() == 3
        && !set(&["EMAIL_PASSWORD", "EMAIL_TOKEN_COMMAND"]).is_empty()
    {
        ready.push("email");
    }
    out.push(finding(
        true,
        "messengers",
        if ready.is_empty() {
            "none has its credentials set (see `sion agent bridge --help`)".to_string()
        } else {
            format!("credentials set for {}", ready.join(", "))
        },
    ));
    if std::env::var_os("SLACK_BOT_TOKEN").is_some()
        && std::env::var_os("SLACK_APP_TOKEN").is_none()
    {
        out.push(finding(
            true,
            "slack",
            "no SLACK_APP_TOKEN: Slack is polled channel by channel, without direct messages or pairing",
        ));
    }

    // Profiles.
    match crate::profiles::all() {
        Ok(profiles) => out.push(finding(
            true,
            "profiles",
            if profiles.is_empty() {
                "none yet (`sion setup` makes one; `sion agent profile add`)".to_string()
            } else {
                profiles
                    .iter()
                    .map(|(name, p)| format!("{name} ({})", p.models.join(", ")))
                    .collect::<Vec<_>>()
                    .join("; ")
            },
        )),
        Err(e) => out.push(finding(false, "profiles", e.to_string())),
    }

    // The network's way out.
    let proxy = set(&["HTTPS_PROXY", "https_proxy", "ALL_PROXY"]);
    if !proxy.is_empty() {
        out.push(finding(
            true,
            "proxy",
            format!(
                "{} set: requests go through it{}",
                proxy.join(", "),
                if std::env::var_os("SSL_CERT_FILE").is_some() {
                    ", its roots from SSL_CERT_FILE"
                } else {
                    ""
                }
            ),
        ));
    }
    out
}

/// Whether a file can be made in `dir`.
fn tempfile_in(dir: &Path) -> bool {
    let probe = dir.join(format!(".doctor-{}", crate::runtime::new_id()));
    let made = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    made
}

/// Prints the findings for the node at `node`; fails only if the home
/// cannot be written.
pub fn run(node: &str) -> Result<(), CliError> {
    let findings = findings(node);
    let width = findings.iter().map(|f| f.what.len()).max().unwrap_or(0);
    for f in &findings {
        println!(
            "{} {:width$}  {}",
            if f.ok { "ok" } else { "--" },
            f.what,
            f.found
        );
    }
    if findings.iter().any(|f| f.what == "home" && !f.ok) {
        return Err(CliError::Usage("the home cannot be written".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn findings_name_what_is_set_and_not_its_value() {
        let home = std::env::temp_dir().join(format!("sion-doctor-{}", crate::runtime::new_id()));
        let findings = findings_at(&home, "http://127.0.0.1:9");
        assert!(findings.iter().any(|f| f.what == "home" && f.ok));
        let node = findings.iter().find(|f| f.what == "node").unwrap();
        assert!(
            !node.ok && node.found.contains("does not answer"),
            "{node:?}"
        );
        assert!(findings.iter().any(|f| f.what == "bubblewrap"));
        assert!(findings.iter().any(|f| f.what == "browser"));
        // No value of any variable is shown.
        for (name, value) in std::env::vars() {
            let secret = ["TOKEN", "KEY", "SECRET"].iter().any(|s| name.ends_with(s));
            if secret && value.len() >= 8 {
                assert!(findings.iter().all(|f| !f.found.contains(&value)), "{name}");
            }
        }
        std::fs::remove_dir_all(&home).unwrap();
    }
}
