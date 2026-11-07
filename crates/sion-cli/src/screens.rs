//! What an agent sees and acts on besides files and the ledger: a browser
//! (`--browser`), a desktop of its own (`--computer`), and a canvas it
//! shows a person things on (`--canvas`, [`sion_agent::canvas`]).
//!
//! The browser is Chromium (or Chrome) with a profile made for the session,
//! in bubblewrap where that works, reaching public addresses only (or only
//! the hosts the operator names) through the egress proxy; it fills the
//! operator's logins the session is given, never showing them. The desktop
//! is an X server with the programs the operator allows, in bubblewrap or
//! a Docker image.
//!
//! All live as long as the process that runs the session, between its
//! runs too where the process keeps them ([`keep_between_runs`]: a server,
//! which hears the approvals of its sessions itself). A session continued
//! by a process of its own gets a browser that goes back to the page it
//! was on, its references kept only where they still name the elements
//! they named; and a new desktop.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use sion_agent::browser::{self, Credential, Credentials, Restore, Shared};
use sion_agent::canvas::Canvas;
use sion_agent::desktop::{self, Computer};
use sion_agent::permissions::{HostRule, ScriptNetwork};
use sion_agent::sandbox::{Backend, Sandbox};
use sion_agent::tools::Tool;

use crate::error::CliError;

/// The tools of a browser and a desktop that act outside. Held to hosts the
/// operator named, they act after the session read pages
/// ([`sion_agent::Policy::contained`]).
pub const ACTING: [&str; 10] = [
    "browser_open",
    "browser_click",
    "browser_click_at",
    "browser_type",
    "browser_type_focused",
    "browser_press",
    "browser_scroll",
    "browser_back",
    "browser_fill_login",
    "computer",
];

/// A session's browser and desktop, as the operator gave them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Screen {
    /// Whether it has a browser.
    #[serde(default)]
    pub browser: bool,
    /// The hosts its browser, and its desktop's programs, may reach; none
    /// named: any public host. `*`: any public host, named.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
    /// The operator's logins its browser may fill, by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logins: Vec<String>,
    /// The browser profile a person signed in with that its browser uses,
    /// by name ([`crate::browser_profiles`]), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Its desktop's sandbox, if it has a desktop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub computer: Option<Sandbox>,
    /// The programs it may start on its desktop, each a command line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub programs: Vec<Vec<String>>,
    /// Its desktop's size, in pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<(u32, u32)>,
    /// Whether its desktop is used only with the pointer and the keyboard,
    /// without code (`run_code`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_code: bool,
    /// Whether it has a canvas to show a person things on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub canvas: bool,
}

impl Screen {
    /// Whether it gives neither a browser, a desktop nor a canvas.
    pub fn is_empty(&self) -> bool {
        !self.browser && self.computer.is_none() && !self.canvas
    }

    /// Where the browser and the desktop's programs may connect.
    pub fn network(&self) -> Result<ScriptNetwork, CliError> {
        if self.hosts.is_empty() || self.hosts.iter().any(|h| h == "*") {
            return Ok(ScriptNetwork::Any);
        }
        self.hosts
            .iter()
            .map(|h| HostRule::parse(h).map_err(CliError::Usage))
            .collect::<Result<Vec<_>, _>>()
            .map(ScriptNetwork::Hosts)
    }

    /// Whether they act on their own after the session read pages: when
    /// the operator named where they reach.
    pub fn contained(&self) -> bool {
        !self.hosts.is_empty()
    }

    /// Checks what can be checked before a session starts.
    pub fn check(&self, workspace: Option<&Path>) -> Result<(), CliError> {
        self.network()?;
        if self.browser && browser::find_program().is_none() {
            return Err(CliError::Usage(
                "no Chromium or Chrome found on this machine: install one, or set SION_BROWSER to it"
                    .into(),
            ));
        }
        if !self.logins.is_empty() {
            let kept = crate::logins::Logins::here().load()?;
            for name in &self.logins {
                if !kept.logins.contains_key(name) {
                    return Err(CliError::Usage(format!(
                        "there is no login {name} (sion agent login add {name} --origin … --username …)"
                    )));
                }
            }
        }
        if self.computer.is_some() && workspace.is_none() {
            return Err(CliError::Usage(
                "a desktop keeps its programs' files in the workspace: give --workspace".into(),
            ));
        }
        if let Some(name) = &self.profile {
            if !crate::browser_profiles::dir(name).is_dir() {
                return Err(CliError::Usage(format!(
                    "there is no browser profile {name}: sign in first (sion agent browser login {name})"
                )));
            }
        }
        Ok(())
    }
}

/// `--browser`, `--computer` and what goes with them.
#[derive(Clone, Debug, Default, clap::Args)]
pub struct ScreenArgs {
    /// Give it a browser (Chromium or Chrome, found on this machine or at
    /// SION_BROWSER): it opens pages, reads them as their accessibility
    /// tree, clicks, types and takes screenshots. It runs in bubblewrap
    /// where that works, and reaches public addresses only.
    #[arg(long)]
    pub browser: bool,
    /// A host its browser, and its desktop's programs, may reach (default:
    /// any public host); `*` names any public host. Where you name them, it
    /// acts on their pages on its own, as its autonomy allows, even after
    /// reading them (`agent:browser-hosts`). Repeat for more.
    #[arg(long = "browser-host", value_name = "HOST")]
    pub browser_hosts: Vec<String>,
    /// One of your logins its browser may fill (`sion agent login add`):
    /// only into a field of the login's origin, and never shown to it.
    /// Repeat for more.
    #[arg(long = "login", value_name = "NAME", requires = "browser")]
    pub logins: Vec<String>,
    /// A browser profile you signed in with (`sion agent browser login`)
    /// for its browser to use: your cookies and sessions, so that it acts
    /// on those sites as you (`agent:signed-in-browser`).
    #[arg(long = "browser-profile", value_name = "NAME", requires = "browser")]
    pub browser_profile: Option<String>,
    /// Give it a desktop of its own, seen in screenshots and used with a
    /// pointer and a keyboard (Xvfb, xdotool and ImageMagick): in
    /// bubblewrap (`bwrap`, the default) or `docker:<image>@sha256:<digest>`,
    /// an image with those programs. Needs --workspace.
    #[arg(long, num_args = 0..=1, default_missing_value = "bwrap", value_name = "SANDBOX")]
    pub computer: Option<String>,
    /// A program it may start on its desktop, as a command line
    /// (`firefox --new-instance`). Repeat for more.
    #[arg(
        long = "computer-program",
        value_name = "COMMAND",
        requires = "computer"
    )]
    pub computer_programs: Vec<String>,
    /// Its desktop's size, WIDTHxHEIGHT.
    #[arg(long, default_value = "1280x800", value_name = "SIZE")]
    pub computer_size: String,
    /// Use its desktop only with the pointer and the keyboard: no programs
    /// of its own run there (`run_code`, Python or bash, on the desktop's
    /// display, in its sandbox).
    #[arg(long, requires = "computer")]
    pub computer_no_code: bool,
    /// Give it a canvas: a page on this machine where it shows you what it
    /// works on (A2UI's components: text, pictures, lists, cards, tabs,
    /// buttons, fields) as it goes, and hears what you press there. The
    /// page's address is in its first canvas_show.
    #[arg(long)]
    pub canvas: bool,
    /// With a Docker desktop, the Docker network its programs reach: one
    /// you set up (default: none).
    #[arg(
        long,
        default_value = "none",
        value_name = "NETWORK",
        requires = "computer"
    )]
    pub computer_network: String,
}

impl ScreenArgs {
    /// The screen the flags describe.
    pub fn screen(self) -> Result<Screen, CliError> {
        let computer = match &self.computer {
            None => None,
            Some(backend) => {
                let network = if backend.starts_with("docker:") {
                    self.computer_network.as_str()
                } else {
                    "none"
                };
                Some(crate::runtime::sandbox_from(
                    backend,
                    network,
                    Vec::new(),
                    120,
                )?)
            }
        };
        let size = match &self.computer {
            None => None,
            Some(_) => Some(parse_size(&self.computer_size)?),
        };
        if let Some(name) = &self.browser_profile {
            crate::browser_profiles::check_name(name).map_err(CliError::Usage)?;
        }
        let screen = Screen {
            browser: self.browser,
            hosts: self.browser_hosts,
            logins: self.logins,
            profile: self.browser_profile,
            computer,
            programs: self
                .computer_programs
                .iter()
                .map(|line| line.split_whitespace().map(str::to_string).collect())
                .filter(|argv: &Vec<String>| !argv.is_empty())
                .collect(),
            size,
            no_code: self.computer_no_code,
            canvas: self.canvas,
        };
        if !screen.browser && screen.computer.is_none() && !screen.hosts.is_empty() {
            return Err(CliError::Usage(
                "--browser-host goes with --browser or --computer".into(),
            ));
        }
        screen.network()?;
        Ok(screen)
    }
}

/// `WIDTHxHEIGHT`, each between 320 and 4096.
fn parse_size(text: &str) -> Result<(u32, u32), CliError> {
    let bad = || {
        CliError::Usage(format!(
            "{text:?} is not a size: give WIDTHxHEIGHT, like 1280x800"
        ))
    };
    let (w, h) = text.split_once(['x', 'X']).ok_or_else(bad)?;
    let (w, h): (u32, u32) = (w.parse().map_err(|_| bad())?, h.parse().map_err(|_| bad())?);
    if !(320..=4096).contains(&w) || !(240..=4096).contains(&h) {
        return Err(bad());
    }
    Ok((w, h))
}

/// The logins a session was given, from the store as it is now.
#[derive(Debug)]
struct Given {
    logins: BTreeMap<String, Credential>,
}

impl Credentials for Given {
    fn get(&self, name: &str) -> Option<Credential> {
        self.logins.get(name).cloned()
    }

    fn list(&self) -> Vec<(String, String)> {
        self.logins
            .iter()
            .map(|(name, c)| (name.clone(), c.origin.clone()))
            .collect()
    }
}

/// A session's browser, desktop and canvas while they run.
#[derive(Clone, Default)]
struct Open {
    browser: Option<Shared>,
    computer: Option<Computer>,
    canvas: Option<Canvas>,
}

static OPEN: OnceLock<Mutex<BTreeMap<String, Open>>> = OnceLock::new();
static KEEP: AtomicBool = AtomicBool::new(false);

fn open() -> std::sync::MutexGuard<'static, BTreeMap<String, Open>> {
    OPEN.get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// Keeps sessions' browsers and desktops between their runs in this
/// process: for a server, where a session that waits for an approval is
/// run again by the same process once it is answered.
pub fn keep_between_runs() {
    KEEP.store(true, Ordering::SeqCst);
}

/// Whether this machine runs the browser in bubblewrap: where bubblewrap
/// works (Linux).
fn browser_sandbox() -> Option<Sandbox> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let sandbox = Sandbox::new(Backend::Bubblewrap);
    sandbox.probe().ok().map(|()| sandbox)
}

/// The tools of session `session`'s screen: its browser and desktop, as
/// they run in this process, or started for it (the browser going back to
/// `restore`).
pub fn tools(
    screen: &Screen,
    session: &str,
    workspace: Option<&Path>,
    confidential: &[String],
    restore: Option<Restore>,
    asker: Option<Arc<dyn sion_agent::mcp_input::Asker>>,
) -> Result<Vec<Box<dyn Tool>>, CliError> {
    screen.check(workspace)?;
    let network = screen.network()?;
    let egress = sion_agent::egress::Options {
        allow_private: false,
        upstream: sion_agent::egress::Upstream::from_env(),
    };
    let mut running = open();
    let entry = running.entry(session.to_string()).or_default();
    let mut tools: Vec<Box<dyn Tool>> = Vec::new();
    if screen.browser {
        if entry.browser.is_none() {
            let program = browser::find_program().ok_or_else(|| {
                CliError::Usage("no Chromium or Chrome found: set SION_BROWSER".into())
            })?;
            let credentials: Option<Arc<dyn Credentials>> = if screen.logins.is_empty() {
                None
            } else {
                let snapshot = crate::logins::Snapshot::of(&crate::logins::Logins::here());
                let logins = screen
                    .logins
                    .iter()
                    .filter_map(|name| snapshot.get(name).map(|c| (name.clone(), c)))
                    .collect();
                Some(Arc::new(Given { logins }))
            };
            let shared = Shared::new(
                browser::Config {
                    program,
                    sandbox: browser_sandbox(),
                    bridge: crate::secrets::bridge(),
                    network: network.clone(),
                    egress: egress.clone(),
                    size: (1280, 800),
                    workspace: workspace.map(Path::to_path_buf),
                    confidential: confidential.to_vec(),
                    profile: screen.profile.as_deref().map(crate::browser_profiles::dir),
                },
                credentials,
            )
            .restoring(restore);
            entry.browser = Some(shared);
        }
        // The person this run can hand the page to, if any.
        let shared = entry.browser.take().expect("set above").asking(asker);
        tools.extend(shared.tools());
        entry.browser = Some(shared);
    }
    if let Some(sandbox) = &screen.computer {
        if entry.computer.is_none() {
            let workspace: PathBuf = workspace
                .ok_or_else(|| CliError::Usage("a desktop needs --workspace".into()))?
                .to_path_buf();
            entry.computer = Some(Computer::new(desktop::Config {
                sandbox: sandbox.clone(),
                size: screen.size.unwrap_or((1280, 800)),
                programs: screen.programs.clone(),
                bridge: crate::secrets::bridge(),
                network: network.clone(),
                egress: egress.clone(),
                workspace,
                code: !screen.no_code,
            }));
        }
        tools.push(Box::new(entry.computer.clone().expect("set above")));
    }
    if screen.canvas {
        if entry.canvas.is_none() {
            // Served from the start, and said at once: what a run says
            // comes when it ends, and the person watches while it works.
            let canvas = Canvas::new();
            let url = canvas.url().map_err(CliError::Usage)?;
            eprintln!("the agent's canvas is at {url}: open it on this machine to watch it");
            entry.canvas = Some(canvas);
        }
        tools.extend(entry.canvas.as_ref().expect("set above").tools());
    }
    Ok(tools)
}

/// After a run of session `session`: where its browser is, to go back to;
/// and its browser and desktop let go, unless it waits and this process
/// keeps them ([`keep_between_runs`]). Its canvas stays while the process
/// runs, at the same address, for the session's next run to show on.
pub fn after_run(session: &str, waits: bool) -> Option<Restore> {
    let mut running = open();
    let state = running
        .get(session)
        .and_then(|open| open.browser.as_ref())
        .and_then(Shared::state);
    if !(waits && KEEP.load(Ordering::SeqCst)) {
        if let Some(open) = running.get_mut(session) {
            open.browser = None;
            open.computer = None;
            if open.canvas.is_none() {
                running.remove(session);
            }
        }
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> ScreenArgs {
        ScreenArgs {
            computer_size: "1280x800".into(),
            computer_network: "none".into(),
            ..ScreenArgs::default()
        }
    }

    #[test]
    fn named_hosts_hold_the_browser_and_let_it_act() {
        let screen = ScreenArgs {
            browser: true,
            browser_hosts: vec!["docs.rs".into(), "*.example.org:8443".into()],
            ..args()
        }
        .screen()
        .unwrap();
        assert!(screen.contained());
        assert!(
            matches!(screen.network().unwrap(), ScriptNetwork::Hosts(rules) if rules.len() == 2)
        );
        // `*` names every public host, and still lets it act.
        let any = ScreenArgs {
            browser: true,
            browser_hosts: vec!["*".into()],
            ..args()
        }
        .screen()
        .unwrap();
        assert!(any.contained() && any.network().unwrap() == ScriptNetwork::Any);
        // None named: any public host, and its actions after a page wait.
        let open = ScreenArgs {
            browser: true,
            ..args()
        }
        .screen()
        .unwrap();
        assert!(!open.contained() && open.network().unwrap() == ScriptNetwork::Any);
        assert!(ScreenArgs {
            browser_hosts: vec!["docs.rs".into()],
            ..args()
        }
        .screen()
        .is_err());
    }

    #[test]
    fn a_desktop_has_a_size_programs_and_a_sandbox() {
        let screen = ScreenArgs {
            computer: Some("bwrap".into()),
            computer_programs: vec!["firefox --new-instance".into(), "  ".into()],
            computer_size: "1024x768".into(),
            ..args()
        }
        .screen()
        .unwrap();
        assert_eq!(screen.size, Some((1024, 768)));
        assert_eq!(
            screen.programs,
            vec![vec!["firefox".to_string(), "--new-instance".into()]]
        );
        assert!(matches!(
            screen.computer.as_ref().map(|s| &s.backend),
            Some(Backend::Bubblewrap)
        ));
        assert!(screen.check(None).is_err(), "a desktop needs a workspace");
        for size in ["1024", "10x10", "99999x800", "wide"] {
            assert!(parse_size(size).is_err(), "{size}");
        }
    }

    #[test]
    fn a_canvas_is_served_once_and_stays_at_its_address_between_runs() {
        let screen = ScreenArgs {
            canvas: true,
            ..args()
        }
        .screen()
        .unwrap();
        assert!(!screen.is_empty() && !screen.contained());
        let session = "canvas-test-session";
        let names = |tools: &[Box<dyn Tool>]| -> Vec<String> {
            tools.iter().map(|t| t.spec().name).collect()
        };
        let given = tools(&screen, session, None, &[], None, None).unwrap();
        assert_eq!(names(&given), ["canvas_show", "canvas_wait"]);
        let url = |session: &str| {
            open()
                .get(session)
                .and_then(|o| o.canvas.as_ref())
                .map(|c| c.url().unwrap())
        };
        let first = url(session).unwrap();
        assert!(first.starts_with("http://127.0.0.1:"), "{first}");
        // The run ends; the next run shows on the same canvas.
        assert!(after_run(session, false).is_none());
        tools(&screen, session, None, &[], None, None).unwrap();
        assert_eq!(url(session).unwrap(), first);
        open().remove(session);
    }
}
