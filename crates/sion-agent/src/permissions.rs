//! What a skill may do beyond being read, as its `SKILL.md` declares it,
//! and how the runtime holds it to that.
//!
//! A skill declares, in its front matter:
//!
//! - **`allowed-tools`** (Agent Skills, experimental): tools pre-approved
//!   while the skill is in use. Once a session has read the skill, a call
//!   that one of its rules matches runs without asking a person, where the
//!   autonomy level would have asked. It never runs a call the level
//!   refuses, the ledger still enforces the agent's charter, and a session
//!   that read content from outside asks again ([`ToolRule`]).
//! - **`sion-network`** (in `metadata`): whether its scripts reach the
//!   network: `true` for any host the sandbox's network reaches, or the
//!   hosts they may connect to (`api.github.com *.example.org:8443`), which
//!   the runtime enforces through a filtering proxy ([`crate::egress`]).
//! - **`sion-secrets`** (in `metadata`): the secrets its scripts receive as
//!   environment variables, by name; the operator's store holds them, the
//!   model never sees them, and they are cut out of what the scripts print.
//! - **`sion-timeout`** (in `metadata`): the seconds a script may run, at
//!   most the sandbox's limit.
//!
//! The agent cannot grant any of this: `skill_write` writes only a name, a
//! description and instructions. Permissions come from the operator, who
//! placed the skill, or from a release, whose bundle (front matter
//! included) its claim commits to and its bonds back, and which is
//! installed only with the operator's consent to what it declares.

use std::collections::BTreeSet;

use serde_json::Value;

/// The metadata key naming the secrets a skill's scripts receive.
pub const SECRETS_KEY: &str = "sion-secrets";

/// The metadata key giving the seconds a skill's script may run.
pub const TIMEOUT_KEY: &str = "sion-timeout";

/// The most rules an `allowed-tools` field may hold.
pub const MAX_RULES: usize = 64;

/// The most hosts a skill may declare.
pub const MAX_HOSTS: usize = 64;

/// The most secrets a skill may declare.
pub const MAX_SECRETS: usize = 16;

/// Tool names other runtimes use in `allowed-tools`, and this runtime's
/// tool for each.
const ALIASES: [(&str, &str); 9] = [
    ("Bash", "shell"),
    ("Read", "read_file"),
    ("Write", "write_file"),
    ("Edit", "write_file"),
    ("MultiEdit", "write_file"),
    ("Glob", "list_files"),
    ("LS", "list_files"),
    ("WebFetch", "fetch_url"),
    ("WebSearch", "web_search"),
];

/// Characters that let a shell command run more than the one it starts
/// with: a command that has any is never pre-approved by a prefix.
const SHELL_META: [&str; 11] = [";", "&", "|", "`", "$(", "${", ">", "<", "\n", "\r", "\\"];

/// One rule of `allowed-tools`: a tool, and optionally which of its calls.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct ToolRule {
    /// The tool, by this runtime's name.
    pub tool: String,
    /// Which calls: `prefix:*` for a command or path that starts with
    /// `prefix` (a shell command's first words, a script's path), an exact
    /// command or path, or `domain:host` for a URL's host; none for every
    /// call.
    pub pattern: Option<String>,
}

impl ToolRule {
    /// Parses one rule: `tool` or `tool(pattern)`, with other runtimes'
    /// names for the same tools (`Bash(git:*)` is `shell(git:*)`).
    pub fn parse(text: &str) -> Result<ToolRule, String> {
        let text = text.trim();
        let (name, pattern) = match text.split_once('(') {
            Some((name, rest)) => {
                let pattern = rest
                    .strip_suffix(')')
                    .ok_or_else(|| format!("{text:?} opens a parenthesis it does not close"))?;
                (name.trim(), Some(pattern.trim().to_string()))
            }
            None => (text, None),
        };
        let valid = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
        if name.is_empty() || !name.chars().all(valid) || name.len() > 128 {
            return Err(format!("{text:?} does not name a tool"));
        }
        let tool = ALIASES
            .iter()
            .find(|(other, _)| *other == name)
            .map(|(_, ours)| ours.to_string())
            .unwrap_or_else(|| name.to_string());
        Ok(ToolRule {
            tool,
            pattern: pattern.filter(|p| !p.is_empty() && p != "*"),
        })
    }

    /// Whether this rule pre-approves calling `tool` with `input`.
    pub fn matches(&self, tool: &str, input: &Value) -> bool {
        if self.tool != tool {
            return false;
        }
        let Some(pattern) = &self.pattern else {
            return true;
        };
        match tool {
            "shell" => {
                let command = input["command"].as_str().unwrap_or_default().trim();
                !SHELL_META.iter().any(|m| command.contains(m)) && starts(pattern, command, ' ')
            }
            "skill_run" => {
                let script = input["script"].as_str().unwrap_or_default();
                !script.contains("..") && starts(pattern, script.trim_start_matches("./"), '/')
            }
            "fetch_url" | "web_search" => match pattern.strip_prefix("domain:") {
                Some(domain) => input["url"]
                    .as_str()
                    .and_then(host_of)
                    .is_some_and(|host| HostRule::host_matches(domain, &host)),
                None => false,
            },
            "read_file" | "write_file" | "list_files" => {
                let path = input["path"].as_str().unwrap_or_default();
                !path.contains("..") && starts(pattern, path.trim_start_matches("./"), '/')
            }
            // A pattern for a tool whose calls this runtime cannot tell
            // apart pre-approves nothing.
            _ => false,
        }
    }

    /// The rule as it is written.
    pub fn describe(&self) -> String {
        match &self.pattern {
            Some(pattern) => format!("{}({pattern})", self.tool),
            None => self.tool.clone(),
        }
    }
}

/// Whether `text` is what `pattern` names: `prefix:*` (or `prefix*`) for
/// `prefix` followed by nothing or by `separator`, else exactly the
/// pattern.
fn starts(pattern: &str, text: &str, separator: char) -> bool {
    let prefix = pattern
        .strip_suffix(":*")
        .or_else(|| pattern.strip_suffix('*'));
    match prefix {
        Some(prefix) => {
            let prefix = prefix.trim_end();
            if prefix.ends_with(separator) {
                text.starts_with(prefix)
            } else {
                text == prefix
                    || text
                        .strip_prefix(prefix)
                        .is_some_and(|rest| rest.starts_with(separator))
            }
        }
        None => text == pattern,
    }
}

/// The host of a URL, lowercased, without its port or user.
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if let Some(v6) = host.strip_prefix('[') {
        v6.split(']').next()?
    } else {
        host.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Splits an `allowed-tools` field into its rules: by white space, except
/// inside parentheses (`Bash(git commit:*)` is one rule).
pub fn split_rules(field: &str) -> Vec<String> {
    let mut rules = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    for c in field.chars() {
        match c {
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(c);
            }
            c if c.is_whitespace() && depth == 0 => {
                if !current.is_empty() {
                    rules.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        rules.push(current);
    }
    rules
}

/// A host a skill's scripts may connect to: exactly one, or every
/// subdomain of one (`*.example.org`), on a port (443 and 80 by default).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct HostRule {
    /// The host, lowercased, with a leading `*.` for its subdomains.
    pub host: String,
    /// The port, if one is named.
    pub port: Option<u16>,
}

impl HostRule {
    /// Parses `host`, `*.domain` or either with `:port`.
    pub fn parse(text: &str) -> Result<HostRule, String> {
        let text = text.trim().trim_end_matches('/').to_ascii_lowercase();
        let text = text
            .strip_prefix("https://")
            .or_else(|| text.strip_prefix("http://"))
            .unwrap_or(&text)
            .to_string();
        let (host, port) = match text.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') => (
                host.to_string(),
                Some(
                    port.parse::<u16>()
                        .map_err(|_| format!("{text:?}: the port is not a number"))?,
                ),
            ),
            _ => (text.clone(), None),
        };
        let bare = host.strip_prefix("*.").unwrap_or(&host);
        let label_ok = |l: &str| {
            !l.is_empty()
                && l.len() <= 63
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !l.starts_with('-')
                && !l.ends_with('-')
        };
        if bare.len() > 253 || !bare.contains('.') || !bare.split('.').all(label_ok) {
            return Err(format!(
                "{text:?} is not a host name (example.org, or *.example.org for its subdomains)"
            ));
        }
        Ok(HostRule { host, port })
    }

    /// Whether `host` is `pattern` or, for `*.domain`, one of its
    /// subdomains (not `domain` itself).
    pub fn host_matches(pattern: &str, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        match pattern.strip_prefix("*.") {
            Some(domain) => host.ends_with(&format!(".{domain}")),
            None => host == pattern,
        }
    }

    /// Whether a connection to `host` on `port` is allowed by this rule.
    pub fn allows(&self, host: &str, port: u16) -> bool {
        let port_ok = match self.port {
            Some(p) => p == port,
            None => port == 443 || port == 80,
        };
        port_ok && HostRule::host_matches(&self.host, host)
    }

    /// The rule as it is written.
    pub fn describe(&self) -> String {
        match self.port {
            Some(port) => format!("{}:{port}", self.host),
            None => self.host.clone(),
        }
    }
}

/// Where a skill's scripts may connect.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ScriptNetwork {
    /// Nowhere: the default.
    #[default]
    None,
    /// Anywhere the sandbox's network reaches.
    Any,
    /// Only these hosts, through the runtime's filtering proxy.
    Hosts(Vec<HostRule>),
}

/// Everything a skill declares beyond its instructions.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Permissions {
    /// The calls pre-approved while the skill is in use.
    pub tools: Vec<ToolRule>,
    /// Where its scripts may connect.
    pub network: ScriptNetwork,
    /// The secrets its scripts receive, by name.
    pub secrets: Vec<String>,
    /// The seconds a script may run, if the skill says.
    pub timeout_secs: Option<u64>,
}

/// Whether `name` can name a secret: an environment variable's name, of
/// capital letters, digits and underscores, not starting with a digit, and
/// not one the sandbox sets or that changes how programs are found and
/// loaded (`PATH`, `HOME`, `LD_…`, the proxy variables, `SION_…`).
pub fn valid_secret_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && !matches!(
            name,
            "PATH" | "HOME" | "USER" | "SHELL" | "TMPDIR" | "NO_PROXY"
        )
        && !name.ends_with("_PROXY")
        && !["LD_", "DYLD_", "SION_", "NODE_", "PYTHON", "BASH_", "ENV"]
            .iter()
            .any(|prefix| name.starts_with(prefix))
}

impl Permissions {
    /// What `allowed_tools` and the `metadata` of a skill's front matter
    /// declare. `network` is the value of `sion-network`, if any.
    pub fn declared(
        allowed_tools: Option<&str>,
        metadata: &std::collections::BTreeMap<String, String>,
        network_key: &str,
    ) -> Result<Permissions, String> {
        let mut tools = Vec::new();
        if let Some(field) = allowed_tools {
            let rules = split_rules(field);
            if rules.len() > MAX_RULES {
                return Err(format!("allowed-tools holds at most {MAX_RULES} rules"));
            }
            let mut seen = BTreeSet::new();
            for rule in rules {
                let rule = ToolRule::parse(&rule)?;
                if seen.insert(rule.clone()) {
                    tools.push(rule);
                }
            }
        }
        let network = match metadata.get(network_key).map(|v| v.trim()) {
            None | Some("") => ScriptNetwork::None,
            Some(v) if matches!(v.to_ascii_lowercase().as_str(), "false" | "no" | "none") => {
                ScriptNetwork::None
            }
            Some(v) if matches!(v.to_ascii_lowercase().as_str(), "true" | "yes" | "required") => {
                ScriptNetwork::Any
            }
            Some(v) => {
                let hosts = v
                    .split([' ', ','])
                    .filter(|h| !h.is_empty())
                    .map(HostRule::parse)
                    .collect::<Result<Vec<_>, _>>()?;
                if hosts.len() > MAX_HOSTS {
                    return Err(format!("{network_key} names at most {MAX_HOSTS} hosts"));
                }
                ScriptNetwork::Hosts(hosts)
            }
        };
        let mut secrets = Vec::new();
        if let Some(v) = metadata.get(SECRETS_KEY) {
            for name in v.split([' ', ',']).filter(|n| !n.is_empty()) {
                if !valid_secret_name(name) {
                    return Err(format!(
                        "{SECRETS_KEY}: {name:?} is not a secret's name (capital letters, digits and underscores)"
                    ));
                }
                if !secrets.iter().any(|s| s == name) {
                    secrets.push(name.to_string());
                }
            }
            if secrets.len() > MAX_SECRETS {
                return Err(format!("{SECRETS_KEY} names at most {MAX_SECRETS} secrets"));
            }
        }
        let timeout_secs = match metadata.get(TIMEOUT_KEY) {
            Some(v) => Some(
                v.trim()
                    .parse::<u64>()
                    .ok()
                    .filter(|s| *s > 0)
                    .ok_or_else(|| format!("{TIMEOUT_KEY}: {v:?} is not a number of seconds"))?,
            ),
            None => None,
        };
        Ok(Permissions {
            tools,
            network,
            secrets,
            timeout_secs,
        })
    }

    /// Whether the skill declares nothing beyond its instructions.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
            && self.network == ScriptNetwork::None
            && self.secrets.is_empty()
            && self.timeout_secs.is_none()
    }

    /// Whether the skill declares anything a person should consent to:
    /// pre-approvals, the network or secrets.
    pub fn needs_consent(&self) -> bool {
        !self.tools.is_empty() || self.network != ScriptNetwork::None || !self.secrets.is_empty()
    }

    /// The skill's permissions in a few phrases, for people.
    pub fn describe(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.tools.is_empty() {
            let rules: Vec<String> = self.tools.iter().map(ToolRule::describe).collect();
            out.push(format!(
                "once a session has read it, these calls run without asking: {}",
                rules.join(", ")
            ));
        }
        match &self.network {
            ScriptNetwork::None => {}
            ScriptNetwork::Any => out.push(
                "its scripts may connect to any host the sandbox's network reaches (on the \
                 declared network, any public one)"
                    .into(),
            ),
            ScriptNetwork::Hosts(hosts) => {
                let hosts: Vec<String> = hosts.iter().map(HostRule::describe).collect();
                out.push(format!("its scripts may connect to {}", hosts.join(", ")));
            }
        }
        match self.secrets.as_slice() {
            [] => {}
            [one] => out.push(format!("its scripts receive the secret {one}")),
            many => out.push(format!(
                "its scripts receive the secrets {}",
                many.join(", ")
            )),
        }
        if let Some(secs) = self.timeout_secs {
            out.push(format!("its scripts are stopped after at most {secs} s"));
        }
        out
    }

    /// The rule of these permissions that pre-approves calling `tool` with
    /// `input`, if one does.
    pub fn pre_approves(&self, tool: &str, input: &Value) -> Option<&ToolRule> {
        self.tools.iter().find(|r| r.matches(tool, input))
    }
}

/// Where the secrets a skill's scripts declare come from: the operator's
/// store, which the model never reads.
pub trait Secrets: Send + Sync + std::fmt::Debug {
    /// The value of the secret `name`, if the store has it.
    fn get(&self, name: &str) -> Option<String>;
}

/// Replaces every occurrence of a secret's value (of at least four
/// characters) in `text` with `[secret NAME]`.
pub fn redact(text: &str, secrets: &[(String, String)]) -> String {
    let mut out = text.to_string();
    for (name, value) in secrets {
        if value.chars().count() >= 4 {
            out = out.replace(value.as_str(), &format!("[secret {name}]"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn rules_parse_with_other_runtimes_names_and_patterns() {
        let rules = split_rules("Bash(git commit:*) Read  shell(ls) skill_run(scripts/*)");
        assert_eq!(
            rules,
            vec![
                "Bash(git commit:*)",
                "Read",
                "shell(ls)",
                "skill_run(scripts/*)"
            ]
        );
        let bash = ToolRule::parse("Bash(git:*)").unwrap();
        assert_eq!(bash.tool, "shell");
        assert_eq!(bash.pattern.as_deref(), Some("git:*"));
        assert_eq!(ToolRule::parse("Read").unwrap().tool, "read_file");
        assert_eq!(ToolRule::parse("Bash(*)").unwrap().pattern, None);
        assert!(ToolRule::parse("Bash(git").is_err());
        assert!(ToolRule::parse("not a tool!").is_err());
    }

    #[test]
    fn a_prefix_rule_matches_one_command_and_never_a_chain() {
        let git = ToolRule::parse("Bash(git:*)").unwrap();
        let run = |command: &str| git.matches("shell", &json!({ "command": command }));
        assert!(run("git status"));
        assert!(run("git"));
        assert!(!run("gitk"), "a prefix is a whole word");
        assert!(!run("git status; rm -rf /workspace"));
        assert!(!run("git log | sh"));
        assert!(!run("git $(curl evil)"));
        assert!(!run("git status && true"));
        assert!(!run("git log > out"));
        assert!(!git.matches("write_file", &json!({"command": "git"})));
        let commit = ToolRule::parse("Bash(git commit:*)").unwrap();
        assert!(commit.matches("shell", &json!({"command": "git commit -m x"})));
        assert!(!commit.matches("shell", &json!({"command": "git push"})));
        let exact = ToolRule::parse("shell(npm test)").unwrap();
        assert!(exact.matches("shell", &json!({"command": "npm test"})));
        assert!(!exact.matches("shell", &json!({"command": "npm test --x"})));
        let scripts = ToolRule::parse("skill_run(scripts/*)").unwrap();
        assert!(scripts.matches("skill_run", &json!({"script": "scripts/a.py"})));
        assert!(!scripts.matches("skill_run", &json!({"script": "scripts/../../x"})));
        assert!(!scripts.matches("skill_run", &json!({"script": "tools/a.py"})));
        let any = ToolRule::parse("ledger_balance").unwrap();
        assert!(any.matches("ledger_balance", &json!({})));
        let opaque = ToolRule::parse("ledger_pay(to:bob)").unwrap();
        assert!(
            !opaque.matches("ledger_pay", &json!({"to": "bob"})),
            "a pattern for a tool whose calls are not told apart pre-approves nothing"
        );
        let fetch = ToolRule::parse("WebFetch(domain:example.org)").unwrap();
        assert!(fetch.matches("fetch_url", &json!({"url": "https://example.org/a"})));
        assert!(!fetch.matches(
            "fetch_url",
            &json!({"url": "https://example.org.evil.com/"})
        ));
        assert!(!fetch.matches("fetch_url", &json!({"url": "https://api.example.org/"})));
    }

    #[test]
    fn hosts_parse_and_match_exactly_or_by_subdomain() {
        let exact = HostRule::parse("api.github.com").unwrap();
        assert!(exact.allows("api.github.com", 443));
        assert!(exact.allows("API.GitHub.com.", 80));
        assert!(
            !exact.allows("api.github.com", 22),
            "only 443 and 80 by default"
        );
        assert!(!exact.allows("evil-api.github.com", 443));
        assert!(!exact.allows("api.github.com.evil.org", 443));
        let wild = HostRule::parse("*.example.org:8443").unwrap();
        assert!(wild.allows("a.example.org", 8443));
        assert!(
            !wild.allows("example.org", 8443),
            "*.domain is its subdomains"
        );
        assert!(!wild.allows("a.example.org", 443));
        assert_eq!(HostRule::parse("https://x.org/").unwrap().host, "x.org");
        assert!(HostRule::parse("localhost").is_err());
        assert!(HostRule::parse("x.org:http").is_err());
        assert!(HostRule::parse("bad_host.org").is_err());
    }

    #[test]
    fn permissions_come_from_the_front_matter_and_describe_themselves() {
        let mut metadata = BTreeMap::new();
        metadata.insert("sion-network".into(), "api.github.com *.example.org".into());
        metadata.insert(SECRETS_KEY.into(), "GITHUB_TOKEN, OTHER".into());
        metadata.insert(TIMEOUT_KEY.into(), "300".into());
        let p = Permissions::declared(Some("Bash(git:*) Read Read"), &metadata, "sion-network")
            .unwrap();
        assert_eq!(p.tools.len(), 2, "duplicates are kept once");
        assert_eq!(p.secrets, vec!["GITHUB_TOKEN", "OTHER"]);
        assert_eq!(p.timeout_secs, Some(300));
        assert!(matches!(&p.network, ScriptNetwork::Hosts(h) if h.len() == 2));
        assert!(p.needs_consent());
        let text = p.describe().join("; ");
        assert!(text.contains("shell(git:*), read_file"), "{text}");
        assert!(text.contains("api.github.com, *.example.org"), "{text}");
        assert!(text.contains("GITHUB_TOKEN, OTHER"), "{text}");
        let none = Permissions::declared(None, &BTreeMap::new(), "sion-network").unwrap();
        assert!(none.is_empty() && !none.needs_consent());
        let mut any = BTreeMap::new();
        any.insert("sion-network".into(), "true".into());
        assert_eq!(
            Permissions::declared(None, &any, "sion-network")
                .unwrap()
                .network,
            ScriptNetwork::Any
        );
        let mut bad = BTreeMap::new();
        bad.insert(SECRETS_KEY.into(), "lower".into());
        assert!(Permissions::declared(None, &bad, "sion-network").is_err());
        let mut bad = BTreeMap::new();
        bad.insert(TIMEOUT_KEY.into(), "soon".into());
        assert!(Permissions::declared(None, &bad, "sion-network").is_err());
    }

    #[test]
    fn secrets_are_cut_out_of_what_scripts_print() {
        let secrets = vec![
            ("TOKEN".to_string(), "ghp_abcdef".to_string()),
            ("PIN".to_string(), "12".to_string()),
        ];
        assert_eq!(
            redact("auth ghp_abcdef ok 12", &secrets),
            "auth [secret TOKEN] ok 12",
            "values under four characters are left: they would cut ordinary text"
        );
        assert!(valid_secret_name("GITHUB_TOKEN") && !valid_secret_name("1X"));
        assert!(!valid_secret_name("lower") && !valid_secret_name(""));
    }

    #[test]
    fn urls_give_their_hosts() {
        assert_eq!(
            host_of("https://user@Example.org:8443/x?y").as_deref(),
            Some("example.org")
        );
        assert_eq!(host_of("http://[::1]:80/").as_deref(), Some("::1"));
        assert_eq!(host_of("not a url"), None);
    }
}
