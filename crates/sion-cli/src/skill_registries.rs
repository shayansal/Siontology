//! Skills from public registries: ClawHub (OpenClaw's registry), sites that
//! publish skills at `/.well-known/agent-skills/index.json` (the Agent
//! Skills discovery format, v0.2), and skills.sh's index of skills in
//! GitHub repositories. `sion skill search` lists what a registry has;
//! `sion skill import` fetches a skill's files and hands them to
//! [`crate::skill_import`], which reports on them and installs a skill only
//! as the operator's review, or an audit on a bond, vouches for its exact
//! bytes. Nothing fetched is run.
//!
//! What a registry says of a skill is reported, never relied on. ClawHub's
//! scanners' verdicts (VirusTotal, SkillSpector, its own model's review)
//! and moderation flags are shown, each scanner on its own, since one may
//! call a version suspicious that the overall status calls clean; a
//! version ClawHub blocks as malware, or any scanner calls malicious, is
//! refused. Every file fetched is checked against the SHA-256 the registry
//! lists for it, and a file it does not list is refused (but for the
//! `_meta.json` ClawHub's archives add). A site's index names a digest for
//! each skill, and the artifact must match it before it is unpacked; an
//! index in the older layout, which names no digests, is refused. Archives
//! are unpacked by name into a folder of their own: no path out of it, no
//! links, within the sizes an import allows.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::CliError;

/// ClawHub's address (`CLAWHUB_URL` in the environment for another).
pub const CLAWHUB: &str = "https://clawhub.ai";

/// skills.sh's address (`SKILLS_SH_URL` for another).
pub const SKILLS_SH: &str = "https://skills.sh";

/// The largest archive fetched, in bytes.
pub const MAX_ARCHIVE: u64 = 25 << 20;

/// The most files a fetched skill has.
pub const MAX_FILES: usize = 200;

/// The largest answer of a registry's API read, in bytes.
const MAX_ANSWER: u64 = 4 << 20;

/// The file ClawHub adds to its archives, which no manifest lists.
const CLAWHUB_META: &str = "_meta.json";

fn http(base: &str) -> Result<sion_http::Client, CliError> {
    sion_http::Client::new(sion_http::Options {
        timeout: std::time::Duration::from_secs(60),
        // A registry on this machine is reached (a mirror, or a stand-in
        // in tests); no other local address.
        allow_private: sion_agent::oauth::is_loopback(base),
        ..sion_http::Options::default()
    })
    .map_err(|e| CliError::Usage(e.to_string()))
}

/// ClawHub's address.
pub fn clawhub() -> String {
    std::env::var("CLAWHUB_URL")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| CLAWHUB.into())
        .trim_end_matches('/')
        .to_string()
}

fn skills_sh() -> String {
    std::env::var("SKILLS_SH_URL")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| SKILLS_SH.into())
        .trim_end_matches('/')
        .to_string()
}

/// `text` with every character a URL's query cannot carry as is escaped.
pub fn escape(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn get_json(client: &sion_http::Client, url: &str) -> Result<Value, CliError> {
    let fetched = client
        .request(
            "GET",
            url,
            &[("Accept", "application/json")],
            None,
            MAX_ANSWER,
        )
        .map_err(|e| registry_error(url, e))?;
    serde_json::from_slice(&fetched.body)
        .map_err(|e| CliError::Usage(format!("{url} did not answer JSON: {e}")))
}

fn registry_error(url: &str, e: sion_http::HttpError) -> CliError {
    if let sion_http::HttpError::Status { status, body, .. } = &e {
        // ClawHub names the skills a slug could mean.
        if let Ok(answer) = serde_json::from_str::<Value>(body) {
            if answer["code"] == "AMBIGUOUS_SKILL_SLUG" {
                let refs: Vec<String> = answer["matches"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|m| {
                        Some(format!(
                            "clawhub:{}/{}",
                            m["ownerHandle"].as_str()?,
                            m["slug"].as_str()?
                        ))
                    })
                    .collect();
                return CliError::Usage(format!(
                    "more than one skill on ClawHub has that name: say whose ({})",
                    refs.join(", ")
                ));
            }
        }
        if *status == 404 {
            return CliError::Usage(format!("not found: {url}"));
        }
    }
    CliError::Usage(format!("{url}: {e}"))
}

/// A skill a registry lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// What `sion skill import` takes to fetch it.
    pub reference: String,
    /// What it says of itself.
    pub summary: String,
    /// Its downloads or installs, as the registry counts them.
    pub count: Option<u64>,
    /// What the registry says of it, in words.
    pub marks: Vec<String>,
}

impl Found {
    /// One line about it.
    pub fn line(&self) -> String {
        let mut line = self.reference.clone();
        if let Some(count) = self.count {
            line.push_str(&format!(" ({count} installs)"));
        }
        if !self.marks.is_empty() {
            line.push_str(&format!(" [{}]", self.marks.join(", ")));
        }
        // On one line, and short: what a registry says of a skill may run
        // on, over lines, and hold what a terminal would act on.
        let flat = self
            .summary
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .replace(|c: char| c.is_control(), "");
        let mut summary: String = flat.chars().take(160).collect();
        if flat.chars().count() > 160 {
            summary.push('…');
        }
        if !summary.is_empty() {
            line.push_str(&format!(": {summary}"));
        }
        line
    }
}

/// Searches `registry` (`clawhub` or `skills.sh`) for `query`.
pub fn search(registry: &str, query: &str, limit: usize) -> Result<Vec<Found>, CliError> {
    let limit = limit.clamp(1, 50);
    match registry {
        "clawhub" => {
            let base = clawhub();
            let answer = get_json(
                &http(&base)?,
                &format!("{base}/api/v1/search?q={}&limit={limit}", escape(query)),
            )?;
            Ok(clawhub_found(&answer))
        }
        "skills.sh" => {
            let base = skills_sh();
            let answer = get_json(
                &http(&base)?,
                &format!("{base}/api/search?q={}&limit={limit}", escape(query)),
            )?;
            Ok(answer["skills"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|skill| {
                    let id = skill["id"].as_str()?;
                    (id.split('/').count() == 3).then(|| Found {
                        reference: format!("skills.sh:{id}"),
                        summary: skill["name"].as_str().unwrap_or_default().to_string(),
                        count: skill["installs"].as_u64(),
                        marks: Vec::new(),
                    })
                })
                .collect())
        }
        other => Err(CliError::Usage(format!(
            "{other} is not a registry sion searches: clawhub or skills.sh"
        ))),
    }
}

/// What ClawHub's search answered, read leniently: its documented shape and
/// the richer one it gives.
pub fn clawhub_found(answer: &Value) -> Vec<Found> {
    answer["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let slug = r["slug"].as_str()?;
            let owner = r["ownerHandle"]
                .as_str()
                .or_else(|| r["owner"]["handle"].as_str())?;
            let mut marks = Vec::new();
            if r["official"] == true || r["publisher"]["official"] == true {
                marks.push("official".to_string());
            }
            if r["native"]["skill"]["isSuspicious"] == true || r["isSuspicious"] == true {
                marks.push("ClawHub calls it suspicious".to_string());
            }
            if let Some(installability) = r["trust"]["installability"].as_str() {
                if installability != "installable" {
                    marks.push(format!("ClawHub: {installability}"));
                }
            }
            Some(Found {
                reference: format!("clawhub:{owner}/{slug}"),
                summary: r["summary"].as_str().unwrap_or_default().to_string(),
                count: r["downloads"]
                    .as_u64()
                    .or_else(|| r["native"]["skill"]["stats"]["downloads"].as_u64()),
                marks,
            })
        })
        .collect()
}

/// A skill fetched into a folder of its own, and what the registry said
/// of it.
#[derive(Debug)]
pub struct Fetched {
    /// The folder its files are in.
    pub dir: PathBuf,
    /// Where it came from, as said.
    pub from: String,
    /// What the registry says of it, one line each, for the report.
    pub lines: Vec<String>,
    /// What is kept with the skill of where it came from.
    pub kept: Value,
}

/// A fetched skill whose files are in a repository instead: ClawHub hands
/// a skill that lives on GitHub over to it.
#[derive(Debug)]
pub enum Got {
    /// Its files, here.
    Files(Fetched),
    /// A repository, the commit to read it at, the skill's folder in it,
    /// and what the registry said of it.
    Repository {
        /// The repository's address.
        url: String,
        /// The commit.
        commit: String,
        /// The skill's folder in it.
        path: String,
        /// What the registry says of it.
        lines: Vec<String>,
    },
}

/// A ClawHub reference: `owner/slug`, or `slug` alone, with `@version` or
/// not.
pub fn clawhub_ref(text: &str) -> Result<(Option<String>, String, Option<String>), String> {
    // ClawHub writes a reference `@owner/slug`.
    let bare = text.strip_prefix('@').unwrap_or(text);
    let (name, version) = match bare.split_once('@') {
        Some((name, version)) if !version.is_empty() => (name, Some(version.to_string())),
        Some(_) => return Err(format!("clawhub:{text}: a version after @")),
        None => (text, None),
    };
    let (owner, slug) = match name.split_once('/') {
        Some((owner, slug)) => (
            Some(owner.trim_start_matches('@').to_ascii_lowercase()),
            slug,
        ),
        None => (None, name),
    };
    // Letters, digits, `-`, `_` and `.`, starting with a letter or digit:
    // nothing that reads as a path on the registry.
    let fine = |s: &str| {
        s.len() <= 100
            && s.starts_with(|c: char| c.is_ascii_alphanumeric())
            && !s.contains("..")
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    };
    if !fine(slug) || !owner.as_deref().map_or(true, fine) {
        return Err(format!(
            "clawhub:{text}: give clawhub:owner/slug, with @version or not"
        ));
    }
    if version.as_deref().is_some_and(|v| {
        !v.chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-+".contains(c))
    }) {
        return Err(format!("clawhub:{text}: that is not a version"));
    }
    Ok((owner, slug.to_string(), version))
}

/// What ClawHub's scanners and moderators say of a version, in lines, and
/// why it is refused, if it is.
pub fn verdicts(security: &Value, moderation: &Value) -> (Vec<String>, Option<String>) {
    let mut lines = Vec::new();
    let mut refused = None;
    if moderation["isMalwareBlocked"] == true {
        refused = Some("ClawHub blocks it as malware".to_string());
    }
    if moderation["isSuspicious"] == true {
        lines.push(format!(
            "ClawHub's moderation calls it suspicious{}",
            moderation["summary"]
                .as_str()
                .map(|s| format!(": {}", s.chars().take(200).collect::<String>()))
                .unwrap_or_default()
        ));
    }
    let word = |v: &Value| -> Option<String> {
        ["normalizedStatus", "verdict", "status"]
            .iter()
            .find_map(|k| v[*k].as_str())
            .map(str::to_lowercase)
    };
    if let Some(status) = security["status"].as_str() {
        lines.push(format!(
            "ClawHub's scan: {status}{}",
            if security["hasWarnings"] == true {
                ", with warnings"
            } else {
                ""
            }
        ));
        if status.eq_ignore_ascii_case("malicious") {
            refused = Some("ClawHub's scan calls it malicious".to_string());
        }
    } else {
        lines.push("ClawHub has not scanned it".to_string());
    }
    for (key, name) in [
        ("vt", "VirusTotal"),
        ("skillspector", "SkillSpector"),
        ("llm", "ClawHub's model review"),
    ] {
        let scanner = &security["scanners"][key];
        if scanner.is_null() {
            continue;
        }
        let said = word(scanner).unwrap_or_else(|| "no verdict".into());
        let mut line = format!("  {name}: {said}");
        if let Some(severity) = scanner["severity"].as_str() {
            line.push_str(&format!(", {}", severity.to_lowercase()));
        }
        if let Some(summary) = scanner["summary"].as_str() {
            line.push_str(&format!(
                " ({})",
                summary.chars().take(160).collect::<String>()
            ));
        }
        lines.push(line);
        if said == "malicious" {
            refused = Some(format!("{name} calls it malicious"));
        }
    }
    (lines, refused)
}

/// The files of a ZIP archive, by name, checked: names within the folder,
/// sizes within an import's.
pub fn unzip(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, CliError> {
    let zip = crate::documents::Zip::open(bytes)
        .map_err(|e| CliError::Usage(format!("the archive: {e}")))?;
    let mut files = Vec::new();
    let mut total = 0u64;
    for name in zip.names() {
        if name.ends_with('/') {
            continue;
        }
        let path = safe_path(name)?;
        let left =
            (crate::skill_import::MAX_SKILL - total.min(crate::skill_import::MAX_SKILL)) as usize;
        let content = zip
            .file(name, left)
            .map_err(|e| CliError::Usage(format!("the archive: {e}")))?;
        total += content.len() as u64;
        files.push((path, content));
        if files.len() > MAX_FILES {
            return Err(CliError::Usage(format!(
                "the archive holds more than {MAX_FILES} files"
            )));
        }
    }
    Ok(files)
}

/// `name` as a path within a skill's folder: relative, forward slashes, no
/// part that is empty, `.` or `..`.
pub fn safe_path(name: &str) -> Result<String, CliError> {
    let refused = || {
        CliError::Usage(format!(
            "the archive names a path out of the skill: {name:?}"
        ))
    };
    if name.is_empty() || name.starts_with('/') || name.contains('\\') || name.contains(':') {
        return Err(refused());
    }
    let parts: Vec<&str> = name.trim_end_matches('/').split('/').collect();
    if parts
        .iter()
        .any(|p| p.is_empty() || *p == "." || *p == "..")
    {
        return Err(refused());
    }
    Ok(parts.join("/"))
}

/// The files of a gzipped tar archive: regular files only, checked as a
/// ZIP's are.
pub fn untar_gz(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, CliError> {
    let damaged = || CliError::Usage("the archive is damaged".into());
    // RFC 1952: a 10-byte header, then optional fields its flags name.
    if bytes.len() < 18 || bytes[0] != 0x1F || bytes[1] != 0x8B || bytes[2] != 8 {
        return Err(damaged());
    }
    let flags = bytes[3];
    let mut at = 10usize;
    if flags & 0x04 != 0 {
        let extra =
            u16::from_le_bytes([bytes[at], *bytes.get(at + 1).ok_or_else(damaged)?]) as usize;
        at += 2 + extra;
    }
    for flag in [0x08, 0x10] {
        if flags & flag != 0 {
            let end = bytes
                .get(at..)
                .ok_or_else(damaged)?
                .iter()
                .position(|b| *b == 0)
                .ok_or_else(damaged)?;
            at += end + 1;
        }
    }
    if flags & 0x02 != 0 {
        at += 2;
    }
    let packed = bytes
        .get(at..bytes.len().saturating_sub(8))
        .ok_or_else(damaged)?;
    let tar = miniz_oxide::inflate::decompress_to_vec_with_limit(
        packed,
        crate::skill_import::MAX_SKILL as usize + (1 << 20),
    )
    .map_err(|_| {
        CliError::Usage("the archive could not be unpacked within an import's size".into())
    })?;
    let mut files = Vec::new();
    let mut at = 0usize;
    while at + 512 <= tar.len() {
        let header = &tar[at..at + 512];
        if header.iter().all(|b| *b == 0) {
            break;
        }
        let field = |from: usize, len: usize| {
            let raw = &header[from..from + len];
            let end = raw.iter().position(|b| *b == 0).unwrap_or(len);
            String::from_utf8_lossy(&raw[..end]).trim().to_string()
        };
        let size = usize::from_str_radix(&field(124, 12), 8).map_err(|_| damaged())?;
        let kind = header[156];
        let prefix = field(345, 155);
        let name = field(0, 100);
        let name = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let body = tar.get(at + 512..at + 512 + size).ok_or_else(damaged)?;
        match kind {
            b'0' | 0 => {
                let path = safe_path(name.trim_start_matches("./"))?;
                files.push((path, body.to_vec()));
            }
            b'5' => {}
            // Links and anything else are not part of a skill.
            _ => {
                return Err(CliError::Usage(format!(
                    "the archive holds {name:?}, which is not a file or a folder"
                )))
            }
        }
        if files.len() > MAX_FILES {
            return Err(CliError::Usage(format!(
                "the archive holds more than {MAX_FILES} files"
            )));
        }
        at += 512 + size.div_ceil(512) * 512;
    }
    Ok(files)
}

/// Writes `files` into a folder of their own.
fn place(files: &[(String, Vec<u8>)]) -> Result<PathBuf, CliError> {
    let dir = std::env::temp_dir().join(format!("sion-skill-fetch-{}", crate::runtime::new_id()));
    for (path, content) in files {
        let target = dir.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CliError::Usage(e.to_string()))?;
        }
        std::fs::write(&target, content).map_err(|e| CliError::Usage(e.to_string()))?;
    }
    Ok(dir)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Fetches `owner`'s skill `slug` from ClawHub, at `version` (its latest
/// if none), checked against what ClawHub lists for that version.
pub fn fetch_clawhub(
    owner: Option<&str>,
    slug: &str,
    version: Option<&str>,
) -> Result<Got, CliError> {
    let base = clawhub();
    let client = http(&base)?;
    let whose = |owner: &str| format!("ownerHandle={}", escape(owner));
    let detail = get_json(
        &client,
        &format!(
            "{base}/api/v1/skills/{}{}",
            escape(slug),
            owner.map(|o| format!("?{}", whose(o))).unwrap_or_default()
        ),
    )?;
    let owner = owner
        .map(str::to_string)
        .or_else(|| detail["owner"]["handle"].as_str().map(str::to_lowercase))
        .ok_or_else(|| CliError::Usage(format!("ClawHub names no owner for {slug}")))?;
    let version = version
        .map(str::to_string)
        .or_else(|| {
            detail["latestVersion"]["version"]
                .as_str()
                .map(String::from)
        })
        .or_else(|| detail["skill"]["tags"]["latest"].as_str().map(String::from))
        .ok_or_else(|| CliError::Usage(format!("clawhub:{owner}/{slug} has no public version")))?;
    let listed = get_json(
        &client,
        &format!(
            "{base}/api/v1/skills/{}/versions/{}?{}",
            escape(slug),
            escape(&version),
            whose(&owner)
        ),
    )?;
    let reference = format!("clawhub:{owner}/{slug}@{version}");
    let mut lines = vec![format!(
        "ClawHub: {reference}{}",
        listed["version"]["license"]
            .as_str()
            .or_else(|| detail["latestVersion"]["license"].as_str())
            .map(|l| format!(", licence {l}"))
            .unwrap_or_default()
    )];
    let (said, refused) = verdicts(&listed["version"]["security"], &detail["moderation"]);
    lines.extend(said);
    if let Some(why) = refused {
        return Err(CliError::Verification(format!(
            "not fetched: {reference}: {why}"
        )));
    }
    let manifest: Vec<(String, String)> = listed["version"]["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| {
            Some((
                f["path"].as_str()?.to_string(),
                f["sha256"].as_str()?.to_ascii_lowercase(),
            ))
        })
        .collect();
    let url = format!(
        "{base}/api/v1/download?slug={}&{}&version={}",
        escape(slug),
        whose(&owner),
        escape(&version)
    );
    let fetched = client
        .request("GET", &url, &[], None, MAX_ARCHIVE)
        .map_err(|e| registry_error(&url, e))?;
    // An archive, or a note that the skill lives on GitHub (as JSON).
    let json = !fetched.body.starts_with(b"PK\x03\x04") && fetched.body.first() == Some(&b'{');
    if json {
        // A skill that lives on GitHub: ClawHub names where, and it is
        // read there, at that commit.
        let handed: Value = serde_json::from_slice(&fetched.body)
            .map_err(|e| CliError::Usage(format!("{url}: {e}")))?;
        let repo = handed["repo"].as_str().unwrap_or_default();
        let commit = handed["commit"].as_str().unwrap_or_default();
        if !crate::github::valid_repo(repo)
            || commit.len() != 40
            || !commit.chars().all(|c| c.is_ascii_hexdigit())
        {
            return Err(CliError::Usage(format!(
                "{reference}: ClawHub hands it to a repository it does not name properly"
            )));
        }
        lines.push(format!(
            "ClawHub keeps it in github.com/{repo}, read there at commit {}",
            &commit[..12]
        ));
        return Ok(Got::Repository {
            url: format!("https://github.com/{repo}.git"),
            commit: commit.to_ascii_lowercase(),
            path: handed["path"].as_str().unwrap_or_default().to_string(),
            lines,
        });
    }
    let mut files = unzip(&fetched.body)?;
    files.retain(|(path, _)| path != CLAWHUB_META);
    if manifest.is_empty() {
        return Err(CliError::Usage(format!(
            "{reference}: ClawHub lists no files for it, so nothing can be checked"
        )));
    }
    for (path, content) in &files {
        let Some((_, digest)) = manifest.iter().find(|(p, _)| p == path) else {
            return Err(CliError::Verification(format!(
                "not fetched: {reference}: the archive holds {path}, which ClawHub does not list"
            )));
        };
        if hex(&sion_http::sha256(content)) != *digest {
            return Err(CliError::Verification(format!(
                "not fetched: {reference}: {path} is not the file ClawHub lists (its SHA-256 differs)"
            )));
        }
    }
    for (path, _) in &manifest {
        if !files.iter().any(|(p, _)| p == path) {
            return Err(CliError::Verification(format!(
                "not fetched: {reference}: the archive lacks {path}, which ClawHub lists"
            )));
        }
    }
    lines.push(format!(
        "every file is the one ClawHub lists ({} checked by SHA-256)",
        files.len()
    ));
    Ok(Got::Files(Fetched {
        dir: place(&files)?,
        from: reference.clone(),
        lines,
        kept: serde_json::json!({
            "registry": "clawhub",
            "owner": owner,
            "slug": slug,
            "version": version,
        }),
    }))
}

/// `url` read against `base` (RFC 3986, for the forms an index uses):
/// absolute https, from the host's root, or beside the index.
pub fn resolve(base: &str, url: &str) -> Result<String, CliError> {
    if url.starts_with("https://") {
        return Ok(url.to_string());
    }
    if url.contains("://") || url.starts_with("//") {
        return Err(CliError::Usage(format!(
            "{url}: an index's skills are fetched over https only"
        )));
    }
    let rest = base
        .strip_prefix("https://")
        .or_else(|| base.strip_prefix("http://"))
        .ok_or_else(|| CliError::Usage(format!("{base} is not an https address")))?;
    let scheme = &base[..base.len() - rest.len()];
    let host = rest.split('/').next().unwrap_or(rest);
    if let Some(path) = url.strip_prefix('/') {
        return Ok(format!("{scheme}{host}/{path}"));
    }
    let dir = match base.rfind('/') {
        Some(at) if at + 1 > scheme.len() + host.len() => &base[..at + 1],
        _ => return Ok(format!("{scheme}{host}/{url}")),
    };
    Ok(format!("{dir}{url}"))
}

/// The index of skills a site publishes, from `text`: an index's own
/// address, or `host` for `https://host/.well-known/agent-skills/index.json`.
pub fn index_url(text: &str) -> Result<String, CliError> {
    if text.starts_with("https://")
        || text.starts_with("http://127.0.0.1")
        || text.starts_with("http://localhost")
    {
        return Ok(text.to_string());
    }
    if text.contains("://") {
        return Err(CliError::Usage(format!(
            "{text}: a site's index is read over https"
        )));
    }
    Ok(format!(
        "https://{}/.well-known/agent-skills/index.json",
        text.trim_end_matches('/')
    ))
}

/// Fetches the skill `name` from the index at `index`, its artifact checked
/// against the index's digest before it is unpacked.
pub fn fetch_well_known(index: &str, name: &str) -> Result<Got, CliError> {
    let client = http(index)?;
    let listed = get_json(&client, index)?;
    match listed["$schema"].as_str() {
        Some(schema) if schema.contains("/discovery/0.2") => {}
        Some(schema) => {
            return Err(CliError::Usage(format!(
                "{index}: an index of a version sion does not read ({schema})"
            )))
        }
        None => {
            return Err(CliError::Usage(format!(
                "{index}: an index in the older layout, which names no digests: not fetched"
            )))
        }
    }
    let skill = listed["skills"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|s| s["name"] == name)
        .ok_or_else(|| CliError::Usage(format!("{index} lists no skill {name}")))?;
    let digest = skill["digest"]
        .as_str()
        .and_then(|d| d.strip_prefix("sha256:"))
        .filter(|d| d.len() == 64 && d.chars().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(|| CliError::Usage(format!("{index}: skill {name} names no SHA-256 digest")))?
        .to_ascii_lowercase();
    let url = resolve(index, skill["url"].as_str().unwrap_or_default())?;
    let artifact = client
        .request("GET", &url, &[], None, MAX_ARCHIVE)
        .map_err(|e| registry_error(&url, e))?;
    if hex(&sion_http::sha256(&artifact.body)) != digest {
        return Err(CliError::Verification(format!(
            "not fetched: {url} is not what {index} names (its SHA-256 differs)"
        )));
    }
    let files = match skill["type"].as_str() {
        Some("skill-md") => vec![("SKILL.md".to_string(), artifact.body.clone())],
        Some("archive") if artifact.body.starts_with(b"PK\x03\x04") => unzip(&artifact.body)?,
        Some("archive") if artifact.body.starts_with(&[0x1F, 0x8B]) => untar_gz(&artifact.body)?,
        Some("archive") => {
            return Err(CliError::Usage(format!(
                "{url}: an archive neither ZIP nor gzipped tar"
            )))
        }
        other => {
            return Err(CliError::Usage(format!(
                "{index}: skill {name} is of a type sion does not read ({})",
                other.unwrap_or("none")
            )))
        }
    };
    if !files.iter().any(|(p, _)| p == "SKILL.md") {
        return Err(CliError::Usage(format!(
            "{url}: no SKILL.md at the archive's root"
        )));
    }
    Ok(Got::Files(Fetched {
        dir: place(&files)?,
        from: format!("{index} ({name})"),
        lines: vec![format!(
            "{index} lists it with digest sha256:{}…, and the artifact matches it",
            &digest[..16]
        )],
        kept: serde_json::json!({
            "registry": "well-known",
            "index": index,
            "name": name,
            "digest": format!("sha256:{digest}"),
        }),
    }))
}

/// The folder in a repository a skills.sh skill named `skill` is in: where
/// skills usually are, else the first folder whose `SKILL.md` names it.
pub fn find_in(repo: &Path, skill: &str) -> Option<PathBuf> {
    for candidate in [
        format!("skills/{skill}"),
        format!(".agents/skills/{skill}"),
        format!(".claude/skills/{skill}"),
        skill.to_string(),
    ] {
        let dir = repo.join(&candidate);
        if dir.join("SKILL.md").is_file() {
            return Some(dir);
        }
    }
    let mut stack = vec![(repo.to_path_buf(), 0usize)];
    let mut seen = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        seen += 1;
        if seen > 2_000 {
            return None;
        }
        let skill_md = dir.join("SKILL.md");
        if let Ok(text) = std::fs::read_to_string(&skill_md) {
            if sion_agent::skills::front_matter(&text).is_ok_and(|f| f.name == skill) {
                return Some(dir);
            }
        }
        if depth >= 6 {
            continue;
        }
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let walked = !name.starts_with('.') && name != "node_modules"
                || name == ".agents"
                || name == ".claude";
            if walked && entry.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push((entry.path(), depth + 1));
            }
        }
    }
    None
}

/// `skill_md` with `network` (hosts) and `secrets` (names) declared in its
/// metadata (`sion-network`, `sion-secrets`), in place of what it declared
/// of either: the operator's grants to an imported skill. Metadata written
/// as one JSON object, as OpenClaw's skills write it, becomes a block of
/// keys, each value as it was.
pub fn grant(skill_md: &str, network: &[String], secrets: &[String]) -> Result<String, String> {
    if network.is_empty() && secrets.is_empty() {
        return Ok(skill_md.to_string());
    }
    let rest = skill_md
        .strip_prefix("---\n")
        .ok_or("SKILL.md must start with front matter between --- lines")?;
    let end = rest
        .find("\n---")
        .ok_or("the front matter of SKILL.md has no closing ---")?;
    let (front, after) = (&rest[..end], &rest[end..]);
    let quoted = |text: &str| serde_json::to_string(text).unwrap_or_default();
    let mut ours = Vec::new();
    if !network.is_empty() {
        ours.push(format!("  sion-network: {}", quoted(&network.join(" "))));
    }
    if !secrets.is_empty() {
        ours.push(format!("  sion-secrets: {}", quoted(&secrets.join(" "))));
    }
    let lines: Vec<&str> = front.lines().collect();
    let indented = |line: &str| line.starts_with(' ') || line.starts_with('\t');
    let mut out: Vec<String> = Vec::new();
    let mut placed = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let value = line.strip_prefix("metadata:");
        match value {
            Some(value) if !placed => {
                placed = true;
                out.push("metadata:".to_string());
                let value = value.trim();
                if value.is_empty() {
                    while i + 1 < lines.len() && indented(lines[i + 1]) {
                        i += 1;
                        let key = lines[i]
                            .trim_start()
                            .split(':')
                            .next()
                            .unwrap_or_default()
                            .trim();
                        if key != "sion-network" && key != sion_agent::permissions::SECRETS_KEY {
                            out.push(lines[i].to_string());
                        }
                    }
                } else {
                    let object: serde_json::Map<String, Value> = serde_json::from_str(value)
                        .map_err(|_| {
                            "its metadata is neither a block of keys nor one JSON object"
                                .to_string()
                        })?;
                    for (key, value) in object {
                        if key == "sion-network" || key == sion_agent::permissions::SECRETS_KEY {
                            continue;
                        }
                        let text = match value {
                            Value::String(text) => text,
                            other => other.to_string(),
                        };
                        out.push(format!("  {key}: {}", quoted(&text)));
                    }
                }
                out.extend(ours.iter().cloned());
            }
            _ => out.push(line.to_string()),
        }
        i += 1;
    }
    if !placed {
        out.push("metadata:".to_string());
        out.extend(ours);
    }
    Ok(format!("---\n{}{after}", out.join("\n")))
}

/// Whether the command `name` is on this machine's `PATH`.
pub fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(name).is_file()))
}

/// What an OpenClaw skill's metadata says it needs (`metadata.openclaw`,
/// or its older names `clawdbot` and `clawdis`): the commands, whether they
/// are on this machine, the environment variables it reads, and what it
/// asks to have installed, in lines for the report; and the variables, for
/// the hint to grant them as secrets.
pub fn openclaw_needs(skill_md: &str) -> (Vec<String>, Vec<String>) {
    let Ok(front) = sion_agent::skills::front_matter(skill_md) else {
        return (Vec::new(), Vec::new());
    };
    // The metadata line itself, written as one JSON object (the form
    // OpenClaw's parser reads), or its openclaw key's value.
    let inline = skill_md
        .lines()
        .skip(1)
        .take_while(|l| *l != "---")
        .find_map(|l| l.strip_prefix("metadata:"))
        .map(str::trim)
        .and_then(|v| serde_json::from_str::<Value>(v).ok());
    let found = ["openclaw", "clawdbot", "clawdis"].iter().find_map(|key| {
        inline
            .as_ref()
            .and_then(|m| m.get(*key).cloned())
            .or_else(|| {
                front
                    .metadata
                    .get(*key)
                    .and_then(|v| serde_json::from_str(v).ok())
            })
    });
    let Some(meta) = found else {
        return (Vec::new(), Vec::new());
    };
    let names = |v: &Value| -> Vec<String> {
        v.as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| {
                x.as_str()
                    .map(String::from)
                    .or_else(|| x["name"].as_str().map(String::from))
            })
            .collect()
    };
    let mut lines = Vec::new();
    let mut bins = names(&meta["requires"]["bins"]);
    bins.extend(names(&meta["requires"]["anyBins"]));
    if !bins.is_empty() {
        let (here, missing): (Vec<String>, Vec<String>) =
            bins.iter().cloned().partition(|b| on_path(b));
        lines.push(format!(
            "it says it needs the commands {}{}{}",
            bins.join(", "),
            if here.is_empty() {
                String::new()
            } else {
                format!("; on this machine: {}", here.join(", "))
            },
            if missing.is_empty() {
                String::new()
            } else {
                format!("; missing here: {}", missing.join(", "))
            }
        ));
    }
    let mut env = names(&meta["requires"]["env"]);
    env.extend(names(&meta["envVars"]));
    if let Some(primary) = meta["primaryEnv"].as_str() {
        env.push(primary.to_string());
    }
    env.sort();
    env.dedup();
    env.retain(|name| {
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    });
    if !env.is_empty() {
        lines.push(format!(
            "it reads the environment variables {}: a skill's scripts here receive only the secrets it declares, which it does not",
            env.join(", ")
        ));
    }
    let installs: Vec<String> = meta["install"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| {
            Some(format!(
                "{} {}",
                i["kind"].as_str()?,
                i["formula"]
                    .as_str()
                    .or_else(|| i["package"].as_str())
                    .or_else(|| i["module"].as_str())
                    .unwrap_or("?")
            ))
        })
        .collect();
    if !installs.is_empty() {
        lines.push(format!(
            "it asks OpenClaw to install software for it ({}): sion installs nothing for a skill",
            installs.join(", ")
        ));
    }
    if meta["always"] == true {
        lines.push("it asks OpenClaw to keep it always in use (always): here a skill is read when it fits a task".into());
    }
    (lines, env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn references_are_read() {
        assert_eq!(
            clawhub_ref("steipete/github@1.2.0").unwrap(),
            (
                Some("steipete".into()),
                "github".into(),
                Some("1.2.0".into())
            )
        );
        assert_eq!(
            clawhub_ref("@Steipete/github").unwrap(),
            (Some("steipete".into()), "github".into(), None)
        );
        assert_eq!(
            clawhub_ref("github").unwrap(),
            (None, "github".into(), None)
        );
        assert!(clawhub_ref("a/b c").is_err());
        assert!(clawhub_ref("a/b@").is_err());
        assert!(clawhub_ref("a/b@1;rm").is_err());
        assert!(clawhub_ref("../x").is_err());
    }

    #[test]
    fn paths_stay_within_the_skill() {
        assert_eq!(safe_path("scripts/run.sh").unwrap(), "scripts/run.sh");
        for bad in [
            "/etc/passwd",
            "../x",
            "a/../../x",
            "a\\b",
            "C:/x",
            "a//b",
            "./a",
        ] {
            assert!(safe_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn urls_are_resolved_against_the_index() {
        let index = "https://example.org/.well-known/agent-skills/index.json";
        assert_eq!(
            resolve(index, "/.well-known/agent-skills/pdf/SKILL.md").unwrap(),
            "https://example.org/.well-known/agent-skills/pdf/SKILL.md"
        );
        assert_eq!(
            resolve(index, "pdf/SKILL.md").unwrap(),
            "https://example.org/.well-known/agent-skills/pdf/SKILL.md"
        );
        assert_eq!(
            resolve(index, "https://cdn.example.net/pdf.zip").unwrap(),
            "https://cdn.example.net/pdf.zip"
        );
        assert!(resolve(index, "http://example.org/x").is_err());
        assert!(resolve(index, "//evil.example/x").is_err());
        assert_eq!(
            index_url("example.org").unwrap(),
            "https://example.org/.well-known/agent-skills/index.json"
        );
        assert!(index_url("ftp://example.org").is_err());
    }

    #[test]
    fn every_scanner_is_heard_and_malicious_is_refused() {
        let security = json!({
            "status": "clean", "hasWarnings": true,
            "scanners": {
                "vt": {"status": "done", "verdict": "benign", "normalizedStatus": "clean"},
                "skillspector": {"normalizedStatus": "suspicious", "severity": "MEDIUM"},
                "llm": {"normalizedStatus": "clean", "summary": "Does what it says."}
            }
        });
        let (lines, refused) = verdicts(&security, &Value::Null);
        assert_eq!(refused, None);
        let text = lines.join("\n");
        assert!(
            text.contains("ClawHub's scan: clean, with warnings"),
            "{text}"
        );
        assert!(text.contains("SkillSpector: suspicious, medium"), "{text}");
        assert!(
            text.contains("ClawHub's model review: clean (Does what it says.)"),
            "{text}"
        );
        let mut bad = security.clone();
        bad["scanners"]["llm"]["normalizedStatus"] = json!("malicious");
        assert_eq!(
            verdicts(&bad, &Value::Null).1.as_deref(),
            Some("ClawHub's model review calls it malicious")
        );
        assert_eq!(
            verdicts(&security, &json!({"isMalwareBlocked": true}))
                .1
                .as_deref(),
            Some("ClawHub blocks it as malware")
        );
        assert!(verdicts(&Value::Null, &Value::Null).0[0].contains("not scanned"));
    }

    #[test]
    fn search_answers_are_read_in_either_shape() {
        let found = clawhub_found(&json!({"results": [
            {"slug": "gifgrep", "displayName": "GifGrep", "summary": "Find GIFs.", "ownerHandle": "openclaw"},
            {"slug": "github", "summary": "Use gh.", "ownerHandle": "steipete", "official": true, "downloads": 5,
             "native": {"skill": {"isSuspicious": true}}, "trust": {"installability": "blocked"}},
            {"summary": "no slug"}
        ]}));
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].reference, "clawhub:openclaw/gifgrep");
        assert_eq!(
            found[1].line(),
            "clawhub:steipete/github (5 installs) [official, ClawHub calls it suspicious, ClawHub: blocked]: Use gh."
        );
    }

    /// A gzipped tar archive of `files` (ustar, gzip with no options).
    fn tar_gz(files: &[(&str, &[u8])], link: Option<&str>) -> Vec<u8> {
        let mut tar = Vec::new();
        let mut entry = |name: &str, kind: u8, body: &[u8]| {
            let mut header = [0u8; 512];
            header[..name.len()].copy_from_slice(name.as_bytes());
            let size = format!("{:011o}\0", body.len());
            header[124..136].copy_from_slice(size.as_bytes());
            header[156] = kind;
            header[257..262].copy_from_slice(b"ustar");
            tar.extend_from_slice(&header);
            tar.extend_from_slice(body);
            tar.resize(tar.len().div_ceil(512) * 512, 0);
        };
        for (name, body) in files {
            entry(name, b'0', body);
        }
        if let Some(name) = link {
            entry(name, b'2', b"");
        }
        tar.extend_from_slice(&[0u8; 1024]);
        let mut gz = vec![0x1F, 0x8B, 8, 0, 0, 0, 0, 0, 0, 3];
        gz.extend(miniz_oxide::deflate::compress_to_vec(&tar, 6));
        gz.extend_from_slice(&[0u8; 8]);
        gz
    }

    #[test]
    fn tarballs_are_unpacked_without_links_or_escapes() {
        let files = untar_gz(&tar_gz(
            &[
                ("SKILL.md", b"---\nname: x\n---\n"),
                ("scripts/a.sh", b"echo"),
            ],
            None,
        ))
        .unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[1].0, "scripts/a.sh");
        assert!(untar_gz(&tar_gz(&[("SKILL.md", b"x")], Some("evil"))).is_err());
        assert!(untar_gz(&tar_gz(&[("../SKILL.md", b"x")], None)).is_err());
        assert!(untar_gz(b"not gzip at all, but long enough").is_err());
    }

    #[test]
    fn grants_are_written_into_the_metadata() {
        let block = "---\nname: x\ndescription: d\nmetadata:\n  author: ana\n  sion-network: \"true\"\n---\nbody\n";
        let granted = grant(block, &["api.github.com".into()], &["GH_TOKEN".into()]).unwrap();
        let front = sion_agent::skills::front_matter(&granted).unwrap();
        assert_eq!(front.metadata["sion-network"], "api.github.com");
        assert_eq!(front.metadata["sion-secrets"], "GH_TOKEN");
        assert_eq!(front.metadata["author"], "ana");
        assert!(granted.ends_with("---\nbody\n"));
        // OpenClaw's metadata, one JSON object, becomes a block.
        let openclaw = "---\nname: gh\ndescription: Use gh.\nmetadata: {\"openclaw\": {\"requires\": {\"bins\": [\"gh\"], \"env\": [\"GH_TOKEN\"]}}}\n---\nbody";
        let granted = grant(openclaw, &[], &["GH_TOKEN".into()]).unwrap();
        let front = sion_agent::skills::front_matter(&granted).unwrap();
        assert_eq!(front.metadata["sion-secrets"], "GH_TOKEN");
        let kept: Value = serde_json::from_str(&front.metadata["openclaw"]).unwrap();
        assert_eq!(kept["requires"]["bins"][0], "gh");
        // None at all: a block is added.
        let none = grant(
            "---\nname: x\ndescription: d\n---\nb",
            &["a.example".into()],
            &[],
        )
        .unwrap();
        assert_eq!(
            sion_agent::skills::front_matter(&none).unwrap().metadata["sion-network"],
            "a.example"
        );
        assert_eq!(grant(block, &[], &[]).unwrap(), block);
    }

    #[test]
    fn openclaw_needs_are_reported() {
        let skill = "---\nname: gh\ndescription: Use gh.\nmetadata: {\"openclaw\": {\"requires\": {\"bins\": [\"sh\", \"no-such-command-here\"], \"env\": [\"GH_TOKEN\"]}, \"primaryEnv\": \"GH_TOKEN\", \"install\": [{\"kind\": \"brew\", \"formula\": \"gh\"}], \"always\": true}}\n---\nbody";
        let (lines, env) = openclaw_needs(skill);
        let text = lines.join("\n");
        assert!(
            text.contains("missing here: no-such-command-here"),
            "{text}"
        );
        assert!(text.contains("environment variables GH_TOKEN"), "{text}");
        assert!(text.contains("(brew gh)"), "{text}");
        assert!(text.contains("always"), "{text}");
        assert_eq!(env, vec!["GH_TOKEN".to_string()]);
        assert_eq!(
            openclaw_needs("---\nname: x\ndescription: d\n---\n"),
            (Vec::new(), Vec::new())
        );
    }
}
