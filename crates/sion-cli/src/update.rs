//! `sion update`: this program, replaced by the latest release, once the
//! release is shown to be one.
//!
//! A release (`.github/workflows/release.yml`, made on a `v*` tag) holds an
//! archive per platform, `sion-<target>.tar.gz` (`x86_64-unknown-linux-gnu`,
//! `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`,
//! `aarch64-apple-darwin`), the archives' SHA-256 sums (`SHA256SUMS`), and
//! the sums' SSH signature (`SHA256SUMS.sig`, namespace `file`) by the
//! release key, `releases@siontology.org`. The key's public half is built
//! into `sion` from `scripts/release/allowed_signers`. `sion update` takes
//! the latest release from GitHub, checks the signature against the key
//! built in (with `ssh-keygen`), then the archive against its signed sum,
//! then that what it holds runs and is the release's version, and only
//! then puts it in place of the program it runs as. A build with no key in
//! that file refuses to update: it would have nothing to check against.
//!
//! The repository may be private: a token in `GH_TOKEN` or `GITHUB_TOKEN`,
//! or the GitHub CLI's (`gh auth token`), is sent to GitHub's API, and to
//! no one else.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::Value;

use crate::error::CliError;

/// Where releases come from.
pub const REPO: &str = "ColdAI-org/siontology";
/// GitHub's API.
pub const API: &str = "https://api.github.com";
/// Who signs releases, as `ssh-keygen -Y` names them.
pub const SIGNER: &str = "releases@siontology.org";
/// The release key's public half, as `ssh-keygen` reads allowed signers.
pub const ALLOWED_SIGNERS: &str = include_str!("../../../scripts/release/allowed_signers");

fn usage(e: impl std::fmt::Display) -> CliError {
    CliError::Usage(e.to_string())
}

/// `sion update`'s options.
#[derive(Clone, Debug, Default, clap::Args)]
pub struct UpdateArgs {
    /// Only say whether there is a newer release.
    #[arg(long)]
    pub check: bool,
    /// This release rather than the latest (its tag, `v0.2.0`).
    #[arg(long, value_name = "TAG")]
    pub version: Option<String>,
    /// Install it even if it is not newer.
    #[arg(long)]
    pub force: bool,
}

/// Where a release is taken from, and what it is checked against.
#[derive(Clone, Debug)]
pub struct Source {
    /// GitHub's API (or a server that answers like it).
    pub api: String,
    /// `owner/name`.
    pub repo: String,
    /// A token for a private repository.
    pub token: Option<String>,
    /// Allowed signers, as `ssh-keygen` reads them.
    pub allowed_signers: String,
    /// The archive this machine runs: `sion-<target>.tar.gz`.
    pub archive: String,
    /// The version running.
    pub running: String,
}

impl Source {
    /// This build's: `SION_RELEASES_REPO` and `SION_RELEASES_API` may name
    /// others.
    pub fn here() -> Result<Source, CliError> {
        let env = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        Ok(Source {
            api: env("SION_RELEASES_API").unwrap_or_else(|| API.to_string()),
            repo: env("SION_RELEASES_REPO").unwrap_or_else(|| REPO.to_string()),
            token: env("GH_TOKEN")
                .or_else(|| env("GITHUB_TOKEN"))
                .or_else(gh_token),
            allowed_signers: ALLOWED_SIGNERS.to_string(),
            archive: archive_name(std::env::consts::ARCH, std::env::consts::OS)?,
            running: env!("CARGO_PKG_VERSION").to_string(),
        })
    }
}

/// The GitHub CLI's token, if it is installed and signed in.
fn gh_token() -> Option<String> {
    let out = Command::new("gh")
        .args(["auth", "token"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let token = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !token.is_empty()).then_some(token)
}

/// The release archive for a machine (`std::env::consts`' names).
pub fn archive_name(arch: &str, os: &str) -> Result<String, CliError> {
    let os = match os {
        "linux" => "unknown-linux-gnu",
        "macos" => "apple-darwin",
        other => {
            return Err(usage(format!(
                "no release is made for {other}: build from source"
            )))
        }
    };
    if !matches!(arch, "x86_64" | "aarch64") {
        return Err(usage(format!(
            "no release is made for {arch}: build from source"
        )));
    }
    Ok(format!("sion-{arch}-{os}.tar.gz"))
}

/// Whether the allowed signers name a key (not only comments).
pub fn has_key(allowed_signers: &str) -> bool {
    allowed_signers
        .lines()
        .map(str::trim)
        .any(|line| !line.is_empty() && !line.starts_with('#'))
}

/// A version's numbers: `v0.2.10` → (0, 2, 10); what follows a `-` (a
/// pre-release) is not compared.
pub fn version(tag: &str) -> Option<(u64, u64, u64)> {
    let core = tag.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let found = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(found)
}

/// The SHA-256 `SHA256SUMS` gives for `name`, in hex.
pub fn sum_for<'a>(sums: &'a str, name: &str) -> Option<&'a str> {
    sums.lines().find_map(|line| {
        let (sum, file) = line.split_once(char::is_whitespace)?;
        let file = file.trim_start().trim_start_matches('*');
        (file == name && sum.len() == 64 && sum.bytes().all(|b| b.is_ascii_hexdigit()))
            .then_some(sum)
    })
}

/// Checks `signature` on `message` with `ssh-keygen`, against `allowed`,
/// for [`SIGNER`] in namespace `file`.
pub fn check_signature(
    work: &Path,
    allowed: &str,
    message: &[u8],
    signature: &[u8],
) -> Result<(), CliError> {
    if !has_key(allowed) {
        return Err(usage(
            "this sion was built with no release key to check updates against \
             (scripts/release/allowed_signers): install a release with the installer instead",
        ));
    }
    let signers = work.join("allowed_signers");
    let sig = work.join("SHA256SUMS.sig");
    std::fs::write(&signers, allowed).map_err(usage)?;
    std::fs::write(&sig, signature).map_err(usage)?;
    let mut child = Command::new("ssh-keygen")
        .args(["-Y", "verify", "-f"])
        .arg(&signers)
        .args(["-I", SIGNER, "-n", "file", "-s"])
        .arg(&sig)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            usage(format!(
                "ssh-keygen, which checks a release's signature: {e}"
            ))
        })?;
    child
        .stdin
        .take()
        .expect("piped")
        .write_all(message)
        .map_err(usage)?;
    let out = child.wait_with_output().map_err(usage)?;
    if !out.status.success() {
        return Err(usage(format!(
            "SHA256SUMS is not signed by {SIGNER}'s key: not installed ({})",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// A release, as GitHub describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// Its tag.
    pub tag: String,
    /// Its assets: each one's name and the API address that gives its bytes.
    pub assets: Vec<(String, String)>,
}

impl Release {
    /// From GitHub's JSON.
    pub fn from_json(value: &Value) -> Result<Release, CliError> {
        let tag = value["tag_name"]
            .as_str()
            .ok_or_else(|| usage("the release has no tag"))?
            .to_string();
        let assets = value["assets"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| {
                Some((
                    a["name"].as_str()?.to_string(),
                    a["url"].as_str()?.to_string(),
                ))
            })
            .collect();
        Ok(Release { tag, assets })
    }

    fn asset(&self, name: &str) -> Result<&str, CliError> {
        self.assets
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, url)| url.as_str())
            .ok_or_else(|| usage(format!("release {} has no {name}", self.tag)))
    }
}

fn client(source: &Source) -> Result<sion_http::Client, CliError> {
    sion_http::Client::new(sion_http::Options {
        timeout: Duration::from_secs(600),
        // An asset's bytes are a redirect away, on GitHub's storage.
        redirects: 5,
        allow_private: sion_agent::oauth::is_loopback(&source.api),
        ..sion_http::Options::default()
    })
    .map_err(usage)
}

fn fetch(
    client: &sion_http::Client,
    source: &Source,
    url: &str,
    accept: &str,
    max: u64,
) -> Result<Vec<u8>, CliError> {
    let bearer = source.token.as_ref().map(|t| format!("Bearer {t}"));
    let mut headers = vec![("Accept", accept), ("X-GitHub-Api-Version", "2022-11-28")];
    // The token goes to the API only (a redirect drops it).
    if let Some(bearer) = bearer.as_deref().filter(|_| url.starts_with(&source.api)) {
        headers.push(("Authorization", bearer));
    }
    client
        .request("GET", url, &headers, None, max)
        .map(|f| f.body)
        .map_err(|e| usage(format!("{url}: {e}")))
}

/// The release `tag`, or the latest.
pub fn release(source: &Source, tag: Option<&str>) -> Result<Release, CliError> {
    let client = client(source)?;
    let url = match tag {
        Some(tag) => format!("{}/repos/{}/releases/tags/{tag}", source.api, source.repo),
        None => format!("{}/repos/{}/releases/latest", source.api, source.repo),
    };
    let body = fetch(&client, source, &url, "application/vnd.github+json", 4 << 20).map_err(
        |e| {
            usage(format!(
                "{e}\n(no release yet, or a private repository: give a token in GH_TOKEN, or sign in with gh auth login)"
            ))
        },
    )?;
    let value: Value = serde_json::from_slice(&body).map_err(usage)?;
    Release::from_json(&value)
}

/// What an update did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Updated {
    /// Nothing: the release is not newer.
    Current(String),
    /// A newer release is out (only checked).
    Available(String),
    /// The program was replaced by this release.
    Installed(String),
}

/// Updates the program at `exe` from `source`: checks, then replaces it.
pub fn update_at(
    exe: &Path,
    source: &Source,
    args: &UpdateArgs,
    work: &Path,
) -> Result<Updated, CliError> {
    let release = release(source, args.version.as_deref())?;
    let newer = match (version(&release.tag), version(&source.running)) {
        (Some(theirs), Some(ours)) => theirs > ours,
        _ => return Err(usage(format!("{} is not a version", release.tag))),
    };
    if !newer && !args.force {
        return Ok(Updated::Current(release.tag));
    }
    if args.check {
        return Ok(Updated::Available(release.tag));
    }
    std::fs::create_dir_all(work).map_err(usage)?;
    let client = client(source)?;
    let octets = "application/octet-stream";
    let sums = fetch(
        &client,
        source,
        release.asset("SHA256SUMS")?,
        octets,
        1 << 20,
    )?;
    let signature = fetch(
        &client,
        source,
        release.asset("SHA256SUMS.sig")?,
        octets,
        64 << 10,
    )?;
    check_signature(work, &source.allowed_signers, &sums, &signature)?;
    let sums = String::from_utf8(sums).map_err(usage)?;
    let expected = sum_for(&sums, &source.archive)
        .ok_or_else(|| usage(format!("SHA256SUMS names no {}", source.archive)))?
        .to_lowercase();
    let archive = fetch(
        &client,
        source,
        release.asset(&source.archive)?,
        octets,
        512 << 20,
    )?;
    if hex::encode(sion_http::sha256(&archive)) != expected {
        return Err(usage(format!(
            "{} does not match its SHA-256 in SHA256SUMS: not installed",
            source.archive
        )));
    }
    let unpacked = work.join("unpacked");
    let _ = std::fs::remove_dir_all(&unpacked);
    std::fs::create_dir_all(&unpacked).map_err(usage)?;
    let file = work.join(&source.archive);
    std::fs::write(&file, &archive).map_err(usage)?;
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&file)
        .arg("-C")
        .arg(&unpacked)
        .status()
        .map_err(|e| usage(format!("tar: {e}")))?;
    let new = unpacked.join("sion");
    if !status.success() || !new.is_file() {
        return Err(usage(format!("{} holds no sion", source.archive)));
    }
    // What it holds runs, and is the release's version.
    let said = Command::new(&new)
        .arg("--version")
        .output()
        .map_err(|e| usage(format!("the new sion does not run: {e}")))?;
    let said = String::from_utf8_lossy(&said.stdout).trim().to_string();
    if version(said.trim_start_matches("sion ")) != version(&release.tag) {
        return Err(usage(format!(
            "the new sion says {said:?}, not release {}: not installed",
            release.tag
        )));
    }
    replace(exe, &new)?;
    Ok(Updated::Installed(release.tag))
}

/// Puts `new` in place of `exe`, atomically: copied beside it, then
/// renamed over it.
fn replace(exe: &Path, new: &Path) -> Result<(), CliError> {
    let dir = exe
        .parent()
        .ok_or_else(|| usage(format!("{} has no directory", exe.display())))?;
    let staged = dir.join(".sion.new");
    std::fs::copy(new, &staged).map_err(|e| {
        usage(format!(
            "{} cannot be written ({e}): install the release where you may write, or run the installer",
            dir.display()
        ))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).map_err(usage)?;
    }
    std::fs::rename(&staged, exe).map_err(usage)
}

/// The program this is, unless something else manages it.
fn this_program() -> Result<PathBuf, CliError> {
    let exe = std::env::current_exe().map_err(usage)?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let shown = exe.display().to_string();
    if shown.contains("/Cellar/") || shown.starts_with("/nix/store/") {
        return Err(usage(format!(
            "{shown} is managed by a package manager: update it there (brew upgrade sion; or your Nix flake)"
        )));
    }
    if shown.contains("/target/debug/") || shown.contains("/target/release/") {
        return Err(usage(format!(
            "{shown} is a build of a checkout: update the checkout and build it again"
        )));
    }
    Ok(exe)
}

/// Runs `sion update`.
pub fn run(args: &UpdateArgs) -> Result<(), CliError> {
    let source = Source::here()?;
    let exe = if args.check {
        std::env::current_exe().map_err(usage)?
    } else {
        this_program()?
    };
    let work = crate::setup::home().join("updates");
    let updated = update_at(&exe, &source, args, &work);
    let _ = std::fs::remove_dir_all(&work);
    match updated? {
        Updated::Current(tag) => println!(
            "sion {} is up to date (the latest release is {tag})",
            source.running
        ),
        Updated::Available(tag) => println!(
            "sion {tag} is out (this is {}): `sion update` installs it",
            source.running
        ),
        Updated::Installed(tag) => {
            println!(
                "sion {tag} is installed at {} (it was {}): its signature and SHA-256 were checked",
                exe.display(),
                source.running
            );
            if let Ok(manager) = crate::service::Manager::here() {
                let services = crate::service::installed(&manager).unwrap_or_default();
                if !services.is_empty() {
                    println!("Services you run go on with the old one until they restart:");
                    for (name, _) in services {
                        match manager {
                            crate::service::Manager::Systemd { .. } => {
                                println!("  systemctl --user restart sion-{name}")
                            }
                            crate::service::Manager::Launchd { .. } => println!(
                                "  launchctl kickstart -k gui/$(id -u)/org.siontology.sion.{name}"
                            ),
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read};
    use std::net::TcpListener;

    #[test]
    fn archives_versions_and_sums_are_read_as_releases_write_them() {
        assert_eq!(
            archive_name("x86_64", "linux").unwrap(),
            "sion-x86_64-unknown-linux-gnu.tar.gz"
        );
        assert_eq!(
            archive_name("aarch64", "macos").unwrap(),
            "sion-aarch64-apple-darwin.tar.gz"
        );
        assert!(archive_name("x86_64", "windows").is_err());
        assert!(archive_name("riscv64", "linux").is_err());
        assert_eq!(version("v0.2.10"), Some((0, 2, 10)));
        assert_eq!(version("1.0.0-rc.1"), Some((1, 0, 0)));
        assert_eq!(version("v1.2"), None);
        assert_eq!(version("v1.2.3.4"), None);
        assert!(version("v0.10.0") > version("0.9.9"));
        let sums = format!(
            "{}  sion-x86_64-unknown-linux-gnu.tar.gz\n{} *sion-aarch64-apple-darwin.tar.gz\n",
            "a".repeat(64),
            "B".repeat(64)
        );
        assert_eq!(
            sum_for(&sums, "sion-x86_64-unknown-linux-gnu.tar.gz"),
            Some("a".repeat(64).as_str())
        );
        assert_eq!(
            sum_for(&sums, "sion-aarch64-apple-darwin.tar.gz"),
            Some("B".repeat(64).as_str())
        );
        assert_eq!(sum_for(&sums, "sion-x86_64-apple-darwin.tar.gz"), None);
        assert!(!has_key("# no key yet\n\n"));
        assert!(has_key(
            "# the key\nreleases@siontology.org ssh-ed25519 AAAA\n"
        ));
        // This repository's file is read as releases read it.
        let _ = has_key(ALLOWED_SIGNERS);
    }

    /// A server that answers like GitHub's API: `routes(base)` gives each
    /// path and its bytes, `base` being the server's own address. It runs
    /// until the test ends.
    fn serve(routes: impl FnOnce(&str) -> Vec<(String, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let table = routes(&base);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let path = line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_string();
                loop {
                    let mut header = String::new();
                    match reader.read_line(&mut header) {
                        Ok(0) | Err(_) => break,
                        Ok(_) if header == "\r\n" => break,
                        Ok(_) => {}
                    }
                }
                let (status, body) = match table.iter().find(|(p, _)| *p == path) {
                    Some((_, body)) => ("200 OK", body.clone()),
                    None => ("404 Not Found", b"{}".to_vec()),
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        base
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sion-update-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sh(dir: &Path, script: &str) -> bool {
        Command::new("sh")
            .args(["-c", script])
            .current_dir(dir)
            .status()
            .is_ok_and(|s| s.success())
    }

    #[test]
    fn a_release_is_installed_only_once_its_signature_and_sum_hold() {
        let dir = scratch("flow");
        // A release key, another key, and a release.
        if !sh(
            &dir,
            "ssh-keygen -q -t ed25519 -N '' -C test -f key && ssh-keygen -q -t ed25519 -N '' -C other -f other",
        ) {
            eprintln!("no ssh-keygen here: skipped");
            return;
        }
        let archive = "sion-x86_64-unknown-linux-gnu.tar.gz";
        assert!(sh(
            &dir,
            &format!(
                "mkdir -p build && printf '#!/bin/sh\\necho sion 9.9.9\\n' > build/sion \
                 && chmod 755 build/sion && tar -czf {archive} -C build sion"
            )
        ));
        let read = |name: &str| std::fs::read(dir.join(name)).unwrap();
        let sums = format!(
            "{}  {archive}\n",
            hex::encode(sion_http::sha256(&read(archive)))
        );
        std::fs::write(dir.join("SHA256SUMS"), &sums).unwrap();
        assert!(sh(
            &dir,
            "ssh-keygen -q -Y sign -f key -n file SHA256SUMS \
             && cp SHA256SUMS by-other && ssh-keygen -q -Y sign -f other -n file by-other"
        ));
        let public = String::from_utf8(read("key.pub")).unwrap();
        let allowed = format!("# the release key\n{SIGNER} namespaces=\"file\" {public}");
        let run = |signature: Vec<u8>, archive_bytes: Vec<u8>, allowed: &str| {
            let sums = read("SHA256SUMS");
            let base = serve(move |base| {
                let release = serde_json::json!({
                    "tag_name": "v9.9.9",
                    "assets": [
                        {"name": "SHA256SUMS", "url": format!("{base}/a/1")},
                        {"name": "SHA256SUMS.sig", "url": format!("{base}/a/2")},
                        {"name": archive, "url": format!("{base}/a/3")},
                    ]
                });
                vec![
                    (
                        "/repos/o/r/releases/latest".into(),
                        release.to_string().into_bytes(),
                    ),
                    ("/a/1".into(), sums),
                    ("/a/2".into(), signature),
                    ("/a/3".into(), archive_bytes),
                ]
            });
            let source = Source {
                api: base,
                repo: "o/r".into(),
                token: Some("for-the-api-only".into()),
                allowed_signers: allowed.to_string(),
                archive: archive.into(),
                running: "0.1.0".into(),
            };
            std::fs::create_dir_all(dir.join("bin")).unwrap();
            let exe = dir.join("bin/sion");
            std::fs::write(&exe, b"the old one").unwrap();
            let result = update_at(&exe, &source, &UpdateArgs::default(), &dir.join("work"));
            let mut now = Vec::new();
            std::fs::File::open(&exe)
                .unwrap()
                .read_to_end(&mut now)
                .unwrap();
            (result, now)
        };
        // Only checking: nothing is fetched past the release, or replaced.
        let base = serve(|_| {
            vec![(
                "/repos/o/r/releases/latest".into(),
                br#"{"tag_name":"v0.1.0","assets":[]}"#.to_vec(),
            )]
        });
        let source = Source {
            api: base,
            repo: "o/r".into(),
            token: None,
            allowed_signers: allowed.clone(),
            archive: archive.into(),
            running: "0.1.0".into(),
        };
        let check = UpdateArgs {
            check: true,
            ..UpdateArgs::default()
        };
        assert_eq!(
            update_at(&dir.join("bin/none"), &source, &check, &dir.join("work")).unwrap(),
            Updated::Current("v0.1.0".into())
        );
        // Signed by the release key, and as summed: installed.
        let (result, now) = run(read("SHA256SUMS.sig"), read(archive), &allowed);
        assert_eq!(result.unwrap(), Updated::Installed("v9.9.9".into()));
        assert!(String::from_utf8_lossy(&now).contains("echo sion 9.9.9"));
        // Signed by another key: refused, and the program is untouched.
        let (result, now) = run(read("by-other.sig"), read(archive), &allowed);
        assert!(result.unwrap_err().to_string().contains("not signed"));
        assert_eq!(now, b"the old one");
        // An archive changed by one byte: refused.
        let mut changed = read(archive);
        let last = changed.len() - 1;
        changed[last] ^= 1;
        let (result, now) = run(read("SHA256SUMS.sig"), changed, &allowed);
        assert!(result.unwrap_err().to_string().contains("does not match"));
        assert_eq!(now, b"the old one");
        // A build with no key to check against: refused.
        let (result, now) = run(read("SHA256SUMS.sig"), read(archive), "# no key yet\n");
        assert!(result.unwrap_err().to_string().contains("no release key"));
        assert_eq!(now, b"the old one");
    }
}
