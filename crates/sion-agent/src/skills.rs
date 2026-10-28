//! Skills in the Agent Skills format: a directory with a `SKILL.md` whose
//! front matter gives a `name` (1 to 64 lowercase letters, digits and
//! single hyphens) and a `description` (what it does and when to use it),
//! followed by instructions, and any other files the instructions refer to.
//!
//! Skills are disclosed progressively: the names and descriptions of every
//! skill go into the system prompt ([`catalog`]); the model reads a skill's
//! instructions when it decides to use it ([`SkillRead`]), and its other
//! files only when the instructions call for them ([`SkillFile`]). An agent
//! writes its own skills with [`SkillWrite`]; the versions it replaces are
//! kept, so a skill's history can be followed.
//!
//! A skill's [`bundle`] is a canonical encoding of its files, sorted by
//! path: the same files always give the same bytes, so a bundle can be
//! committed to (a release claim names its root) and kept by a storage deal.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::builtin::inside;
use crate::model::ToolSpec;
use crate::tools::{Effect, Outcome, Tool};

/// The most bytes of a `SKILL.md`.
pub const MAX_SKILL_MD: usize = 64 << 10;
/// The most bytes of any other file of a skill.
pub const MAX_SKILL_FILE: u64 = 256 << 10;
/// The most files in a skill's bundle.
pub const MAX_FILES: usize = 64;
/// The most bytes of a bundle.
pub const MAX_BUNDLE: usize = 4 << 20;
/// The most characters of a description.
pub const MAX_DESCRIPTION: usize = 1024;
/// Where replaced versions of a skill's files are kept, inside the skill.
pub const HISTORY: &str = ".history";

const BUNDLE_MAGIC: &[u8] = b"sion-skill-bundle/v1\n";

/// A skill found on disk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skill {
    /// Its name.
    pub name: String,
    /// What it does and when to use it.
    pub description: String,
    /// Its directory.
    pub dir: PathBuf,
}

/// Whether `name` is a valid skill name: 1 to 64 lowercase ASCII letters,
/// digits and hyphens, not starting or ending with a hyphen, with no two in
/// a row.
pub fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
}

/// Reads the front matter of a `SKILL.md`: its name, its description and
/// the body after the front matter.
pub fn parse(text: &str) -> Result<(String, String, String), String> {
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
        .ok_or("SKILL.md must start with front matter between --- lines")?;
    let end = rest
        .find("\n---")
        .ok_or("the front matter of SKILL.md has no closing ---")?;
    let front = &rest[..end];
    let body = rest[end + 4..].trim_start_matches(['\r', '\n']).to_string();
    let mut name = None;
    let mut description = None;
    let lines: Vec<&str> = front.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some((key, value)) = line.split_once(':') {
            if !line.starts_with(' ') {
                let key = key.trim();
                let mut value = value.trim().to_string();
                // A folded or literal block: the indented lines that follow.
                if value == ">" || value == "|" || value == ">-" || value == "|-" {
                    let mut parts = Vec::new();
                    while i + 1 < lines.len() && lines[i + 1].starts_with(' ') {
                        i += 1;
                        parts.push(lines[i].trim());
                    }
                    value = parts.join(" ");
                }
                let value = if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
                    serde_json::from_str::<String>(&value)
                        .unwrap_or_else(|_| value[1..value.len() - 1].to_string())
                } else {
                    value.trim_matches('\'').to_string()
                };
                match key {
                    "name" => name = Some(value),
                    "description" => description = Some(value),
                    _ => {}
                }
            }
        }
        i += 1;
    }
    let name = name.ok_or("the front matter has no name")?;
    let description = description.ok_or("the front matter has no description")?;
    if !valid_name(&name) {
        return Err(format!(
            "{name:?} is not a valid skill name: 1 to 64 lowercase letters, digits and single hyphens"
        ));
    }
    if description.is_empty() || description.chars().count() > MAX_DESCRIPTION {
        return Err(format!(
            "a skill's description has 1 to {MAX_DESCRIPTION} characters"
        ));
    }
    Ok((name, description, body))
}

/// The text of a `SKILL.md`.
pub fn render(name: &str, description: &str, body: &str) -> String {
    let description = description.replace('\n', " ");
    format!(
        "---\nname: {name}\ndescription: {}\n---\n\n{}\n",
        serde_json::to_string(&description).unwrap_or_default(),
        body.trim_end()
    )
}

/// The skills in `root`: every directory whose `SKILL.md` parses and whose
/// name matches the directory's, sorted by name.
pub fn list(root: &Path) -> Vec<Skill> {
    let mut skills = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return skills;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        let Ok(text) = std::fs::read_to_string(dir.join("SKILL.md")) else {
            continue;
        };
        if let Ok((name, description, _)) = parse(&text) {
            if entry.file_name().to_string_lossy() == name {
                skills.push(Skill {
                    name,
                    description,
                    dir,
                });
            }
        }
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// What the system prompt says about the skills: one line each.
pub fn catalog(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut text = String::from(
        "Skills you have (read one with skill_read before using it; its other files with skill_file):\n",
    );
    for skill in skills {
        text.push_str(&format!("- {}: {}\n", skill.name, skill.description));
    }
    text
}

/// The files of a skill directory, relative and sorted, without the kept
/// history.
fn files(dir: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries =
            std::fs::read_dir(&current).map_err(|e| format!("{}: {e}", current.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_symlink() {
                return Err(format!("{}: a skill may not contain links", path.display()));
            }
            let relative = path
                .strip_prefix(dir)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            if relative == HISTORY || relative.starts_with(&format!("{HISTORY}/")) {
                continue;
            }
            if kind.is_dir() {
                stack.push(path);
            } else {
                found.push((relative, path));
            }
        }
    }
    found.sort();
    if found.len() > MAX_FILES {
        return Err(format!("a skill has at most {MAX_FILES} files"));
    }
    Ok(found)
}

/// The canonical bytes of a skill directory: a magic line, then for each
/// file sorted by path, the path's length (4 bytes, big-endian), the path,
/// the content's length (8 bytes, big-endian) and the content.
pub fn bundle(dir: &Path) -> Result<Vec<u8>, String> {
    let mut bytes = BUNDLE_MAGIC.to_vec();
    for (relative, path) in files(dir)? {
        let content = std::fs::read(&path).map_err(|e| format!("{relative}: {e}"))?;
        let path_len = u32::try_from(relative.len()).map_err(|_| "a path is too long")?;
        bytes.extend_from_slice(&path_len.to_be_bytes());
        bytes.extend_from_slice(relative.as_bytes());
        bytes.extend_from_slice(&(content.len() as u64).to_be_bytes());
        bytes.extend_from_slice(&content);
        if bytes.len() > MAX_BUNDLE {
            return Err(format!("a skill's bundle is at most {MAX_BUNDLE} bytes"));
        }
    }
    Ok(bytes)
}

/// The files of a bundle, in order, checked: sorted, unique, inside the
/// skill, within the limits, and with a valid `SKILL.md`.
pub fn unbundle(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    if bytes.len() > MAX_BUNDLE {
        return Err(format!("a skill's bundle is at most {MAX_BUNDLE} bytes"));
    }
    let mut rest = bytes
        .strip_prefix(BUNDLE_MAGIC)
        .ok_or("not a skill bundle")?;
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    fn take(n: usize, rest: &mut &[u8]) -> Result<Vec<u8>, String> {
        if rest.len() < n {
            return Err("the bundle is cut short".to_string());
        }
        let (head, tail) = rest.split_at(n);
        *rest = tail;
        Ok(head.to_vec())
    }
    while !rest.is_empty() {
        let len = u32::from_be_bytes(take(4, &mut rest)?.try_into().expect("4 bytes")) as usize;
        let path = String::from_utf8(take(len, &mut rest)?).map_err(|_| "a path is not UTF-8")?;
        let size = u64::from_be_bytes(take(8, &mut rest)?.try_into().expect("8 bytes"));
        let size = usize::try_from(size).map_err(|_| "a file is too large")?;
        let content = take(size, &mut rest)?;
        if path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
            || path.starts_with(HISTORY)
        {
            return Err(format!("{path}: not a path inside a skill"));
        }
        if let Some((last, _)) = out.last() {
            if *last >= path {
                return Err("the bundle's files are not sorted and unique".into());
            }
        }
        out.push((path, content));
        if out.len() > MAX_FILES {
            return Err(format!("a skill has at most {MAX_FILES} files"));
        }
    }
    let skill_md = out
        .iter()
        .find(|(path, _)| path == "SKILL.md")
        .ok_or("the bundle has no SKILL.md")?;
    let text = std::str::from_utf8(&skill_md.1).map_err(|_| "SKILL.md is not UTF-8")?;
    parse(text)?;
    Ok(out)
}

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": properties, "required": required})
}

fn skill_dir(root: &Path, name: &str) -> Result<PathBuf, String> {
    if !valid_name(name) {
        return Err(format!("{name:?} is not a skill name"));
    }
    Ok(root.join(name))
}

/// Reads a skill's instructions.
#[derive(Debug, Clone)]
pub struct SkillRead {
    /// The skills' directory.
    pub root: PathBuf,
}

impl Tool for SkillRead {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "skill_read".into(),
            description: "Read a skill's instructions (its SKILL.md) before using it.".into(),
            input_schema: schema(json!({"name": {"type": "string"}}), &["name"]),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn call(&self, input: &Value, _key: &str) -> Outcome {
        let name = input["name"].as_str().unwrap_or_default();
        let dir = match skill_dir(&self.root, name) {
            Ok(dir) => dir,
            Err(e) => return Outcome::error(e),
        };
        match std::fs::read_to_string(dir.join("SKILL.md")) {
            Ok(text) => Outcome::ok(text),
            Err(_) => Outcome::error(format!("there is no skill {name}")),
        }
    }
}

/// Reads one of a skill's other files.
#[derive(Debug, Clone)]
pub struct SkillFile {
    /// The skills' directory.
    pub root: PathBuf,
}

impl Tool for SkillFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "skill_file".into(),
            description: "Read a file that a skill's instructions refer to: the skill's name and \
                          the file's path inside it."
                .into(),
            input_schema: schema(
                json!({"name": {"type": "string"}, "path": {"type": "string"}}),
                &["name", "path"],
            ),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn call(&self, input: &Value, _key: &str) -> Outcome {
        let name = input["name"].as_str().unwrap_or_default();
        let relative = input["path"].as_str().unwrap_or_default();
        let dir = match skill_dir(&self.root, name) {
            Ok(dir) => dir,
            Err(e) => return Outcome::error(e),
        };
        if !dir.is_dir() {
            return Outcome::error(format!("there is no skill {name}"));
        }
        let path = match inside(&dir, relative) {
            Ok(path) => path,
            Err(e) => return Outcome::error(e),
        };
        match std::fs::metadata(&path) {
            Ok(meta) if meta.len() > MAX_SKILL_FILE => {
                return Outcome::error(format!("{relative} is larger than {MAX_SKILL_FILE} bytes"))
            }
            Ok(meta) if !meta.is_file() => {
                return Outcome::error(format!("{relative} is not a file"))
            }
            Err(_) => return Outcome::error(format!("{name} has no file {relative}")),
            Ok(_) => {}
        }
        match std::fs::read(&path).map(String::from_utf8) {
            Ok(Ok(text)) => Outcome::ok(text),
            Ok(Err(_)) => Outcome::error(format!("{relative} is not text")),
            Err(e) => Outcome::error(format!("{relative}: {e}")),
        }
    }
}

/// Writes a skill of the agent's own: creates it, or replaces its
/// instructions and keeps the version it replaces.
#[derive(Debug, Clone)]
pub struct SkillWrite {
    /// The skills' directory.
    pub root: PathBuf,
}

impl Tool for SkillWrite {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "skill_write".into(),
            description: "Write down how to do something you did well, as a skill, or improve a \
                          skill you have: its name (lowercase letters, digits and hyphens), a \
                          description of what it does and when to use it, and the instructions. \
                          The version replaced is kept."
                .into(),
            input_schema: schema(
                json!({
                    "name": {"type": "string"},
                    "description": {"type": "string"},
                    "instructions": {"type": "string"}
                }),
                &["name", "description", "instructions"],
            ),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Write
    }

    fn call(&self, input: &Value, _key: &str) -> Outcome {
        let name = input["name"].as_str().unwrap_or_default();
        let description = input["description"].as_str().unwrap_or_default().trim();
        let instructions = input["instructions"].as_str().unwrap_or_default();
        let dir = match skill_dir(&self.root, name) {
            Ok(dir) => dir,
            Err(e) => return Outcome::error(e),
        };
        let text = render(name, description, instructions);
        if let Err(e) = parse(&text) {
            return Outcome::error(e);
        }
        if text.len() > MAX_SKILL_MD {
            return Outcome::error(format!("SKILL.md is at most {MAX_SKILL_MD} bytes"));
        }
        let path = dir.join("SKILL.md");
        let previous = std::fs::read_to_string(&path).ok();
        if previous.as_deref() == Some(text.as_str()) {
            return Outcome::ok(format!("skill {name} is unchanged"));
        }
        let write = || -> std::io::Result<usize> {
            std::fs::create_dir_all(&dir)?;
            let mut version = 0;
            if let Some(previous) = &previous {
                let history = dir.join(HISTORY);
                std::fs::create_dir_all(&history)?;
                version = std::fs::read_dir(&history)?.count() + 1;
                std::fs::write(history.join(format!("{version}.md")), previous)?;
            }
            let partial = dir.join("SKILL.md.partial");
            std::fs::write(&partial, &text)?;
            std::fs::rename(&partial, &path)?;
            Ok(version)
        };
        match write() {
            Ok(0) => Outcome::ok(format!("wrote skill {name}")),
            Ok(version) => Outcome::ok(format!(
                "improved skill {name}; the previous version is kept as {HISTORY}/{version}.md"
            )),
            Err(e) => Outcome::error(format!("{name}: {e}")),
        }
    }
}

/// The skill tools for `root`: read, read a file, write.
pub fn tools(root: &Path) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(SkillRead {
            root: root.to_path_buf(),
        }),
        Box::new(SkillFile {
            root: root.to_path_buf(),
        }),
        Box::new(SkillWrite {
            root: root.to_path_buf(),
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sion-skills-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn names_follow_the_agent_skills_rules() {
        for good in ["pdf", "pay-invoices", "a1", &"x".repeat(64)] {
            assert!(valid_name(good), "{good}");
        }
        for bad in [
            "",
            "Pdf",
            "-pdf",
            "pdf-",
            "pay--x",
            "pay_x",
            "pay x",
            &"x".repeat(65),
        ] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn front_matter_is_read_in_its_common_forms() {
        let (name, description, body) =
            parse("---\nname: invoices\ndescription: \"Pay invoices: check, then pay.\"\n---\n\n# Steps\n1. Check.\n").unwrap();
        assert_eq!(
            (name.as_str(), description.as_str()),
            ("invoices", "Pay invoices: check, then pay.")
        );
        assert_eq!(body, "# Steps\n1. Check.\n");
        let (_, folded, _) =
            parse("---\nname: x\ndescription: >\n  Several\n  lines.\nlicense: MIT\n---\nbody")
                .unwrap();
        assert_eq!(folded, "Several lines.");
        assert!(parse("no front matter").is_err());
        assert!(parse("---\nname: Bad Name\ndescription: d\n---\n").is_err());
        assert!(parse("---\nname: x\n---\n").is_err());
        let text = render("x", "Does a thing: well.", "Body.");
        assert_eq!(parse(&text).unwrap().1, "Does a thing: well.");
    }

    #[test]
    fn an_agent_writes_reads_and_improves_its_skills() {
        let root = temp("write");
        let write = SkillWrite { root: root.clone() };
        let read = SkillRead { root: root.clone() };
        let out = write.call(
            &json!({"name": "pay-invoices", "description": "Pay a supplier's invoice after checking it.", "instructions": "1. Check the invoice.\n2. Pay it."}),
            "k",
        );
        assert!(!out.is_error, "{out:?}");
        assert!(read
            .call(&json!({"name": "pay-invoices"}), "k")
            .content
            .contains("2. Pay it."));
        let listed = list(&root);
        assert_eq!(listed.len(), 1);
        assert!(catalog(&listed)
            .contains("- pay-invoices: Pay a supplier's invoice after checking it."));
        let improved = write.call(
            &json!({"name": "pay-invoices", "description": "Pay a supplier's invoice after checking it.", "instructions": "1. Check the invoice against the order.\n2. Pay it."}),
            "k2",
        );
        assert!(improved.content.contains(".history/1.md"), "{improved:?}");
        assert!(root.join("pay-invoices/.history/1.md").exists());
        assert!(
            write
                .call(
                    &json!({"name": "../x", "description": "d", "instructions": "i"}),
                    "k"
                )
                .is_error
        );
        assert!(read.call(&json!({"name": "missing"}), "k").is_error);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_bundle_is_canonical_and_checked_when_read() {
        let root = temp("bundle");
        let dir = root.join("report");
        std::fs::create_dir_all(dir.join("templates")).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            render("report", "Write a report.", "See templates/a.md."),
        )
        .unwrap();
        std::fs::write(dir.join("templates/a.md"), "# A").unwrap();
        std::fs::create_dir_all(dir.join(HISTORY)).unwrap();
        std::fs::write(dir.join(HISTORY).join("1.md"), "old").unwrap();
        let bytes = bundle(&dir).unwrap();
        assert_eq!(bytes, bundle(&dir).unwrap());
        let files = unbundle(&bytes).unwrap();
        let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            paths,
            vec!["SKILL.md", "templates/a.md"],
            "history is not bundled"
        );
        let file = SkillFile { root: root.clone() };
        assert_eq!(
            file.call(&json!({"name": "report", "path": "templates/a.md"}), "k")
                .content,
            "# A"
        );
        assert!(
            file.call(
                &json!({"name": "report", "path": "../report/SKILL.md"}),
                "k"
            )
            .is_error
        );
        // Tampered or malformed bundles are refused.
        let mut cut = bytes.clone();
        cut.truncate(bytes.len() - 1);
        assert!(unbundle(&cut).is_err());
        assert!(unbundle(b"not a bundle").is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
