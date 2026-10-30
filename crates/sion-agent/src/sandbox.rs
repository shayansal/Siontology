//! Running commands where they can do little harm: the agent's workspace is
//! the only place they can write, they reach no network unless the operator
//! allows it, and they are stopped when they run too long.
//!
//! Two backends:
//!
//! - **Bubblewrap** (`bwrap`, Linux). The command runs in new user, PID,
//!   IPC, UTS, cgroup and network namespaces, as an unprivileged user with
//!   no capabilities. It sees the system's programs and libraries
//!   read-only, a few files of `/etc` (certificates, the dynamic linker's
//!   cache, a user database naming its one user), any directories the
//!   operator adds read-only, a private `/tmp`, and the workspace
//!   read-write at `/workspace`; nothing else of the file system, the home
//!   directory included. Without network it has only a loopback interface.
//! - **Docker.** The command runs in a container of an image pinned by
//!   digest (`name@sha256:…`) that is already on the machine: it is never
//!   pulled during a call. The root file system is read-only, every
//!   capability is dropped, new privileges are refused, processes, memory
//!   and CPUs are bounded, and it runs as the workspace's owner, so that
//!   what it writes stays theirs. Without network it runs with
//!   `--network none`; with network, only on a Docker network the operator
//!   names (one that blocks private and internal addresses): Docker's
//!   `host` and default `bridge` networks are refused.
//!
//! Both run `/bin/sh -c <command>` with a clean environment, a CPU-time
//! limit, a file-size limit, a wall-clock limit and bounded output. What a
//! sandbox does not stop is named in the trust model (`agent:sandbox`): a
//! kernel or container escape; with bubblewrap, a command that starts
//! processes without end until the wall-clock limit stops it; and, with
//! the network allowed, whatever the network lets it reach.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// The user a command runs as under bubblewrap. Inside the sandbox it is
/// this number; outside, the user running the runtime.
pub const SANDBOX_UID: u32 = 10001;

/// Where the workspace is inside a sandbox.
pub const WORKSPACE: &str = "/workspace";

/// The label every sandbox container carries (`docker ps --filter
/// label=sion-sandbox`).
pub const DOCKER_LABEL: &str = "sion-sandbox";

/// What runs the commands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    /// Linux namespaces through bubblewrap.
    Bubblewrap,
    /// A Docker container of `image`, pinned by digest.
    Docker {
        /// The image, `name@sha256:<64 hex digits>`.
        image: String,
    },
}

/// Whether, and how, a sandboxed command reaches the network.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Network {
    /// No network: only a loopback interface. The default.
    #[default]
    None,
    /// The machine's network as it is (bubblewrap): nothing filters where
    /// the command connects, local services and a cloud's metadata address
    /// included.
    Host,
    /// A Docker network the operator set up; `host` and `bridge` are
    /// refused.
    Docker(String),
    /// Only what skills declare (bubblewrap): commands get no network, and
    /// the scripts of a skill that declares a network reach it through the
    /// egress proxy ([`crate::egress`]), which refuses private, loopback
    /// and link-local addresses: the hosts the skill names, or any public
    /// host if it declares `true`.
    Declared,
}

impl Network {
    /// Whether commands reach anything outside the sandbox directly.
    pub fn reaches_out(&self) -> bool {
        matches!(self, Network::Host | Network::Docker(_))
    }

    /// Whether the scripts of a skill that declares a network reach
    /// outside the sandbox, directly or through the egress proxy.
    pub fn scripts_reach_out(&self) -> bool {
        !matches!(self, Network::None)
    }

    /// The network in a few words.
    pub fn describe(&self) -> String {
        match self {
            Network::None => "no network".into(),
            Network::Host => "the machine's network, unfiltered".into(),
            Network::Docker(name) => format!("the Docker network {name}"),
            Network::Declared => {
                "no network for commands; skills' scripts reach what their skills declare, \
                 through a filtering proxy"
                    .into()
            }
        }
    }
}

/// How far a command may go.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Limits {
    /// Seconds of wall-clock time before the command is stopped.
    pub timeout_secs: u64,
    /// CPU seconds (each process).
    pub cpu_seconds: u64,
    /// Bytes of each of standard output and standard error kept.
    pub output_bytes: usize,
    /// Memory, in MiB: each process's address space with bubblewrap, the
    /// container's memory with Docker. 0 sets none: for a browser, which
    /// reserves far more address space than it uses.
    pub memory_mib: u64,
    /// The largest file a command may write, in MiB.
    pub file_mib: u64,
    /// Processes at once (Docker).
    pub processes: u32,
    /// CPUs (Docker).
    pub cpus: u32,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            timeout_secs: 120,
            cpu_seconds: 120,
            output_bytes: 64 << 10,
            memory_mib: 4096,
            file_mib: 512,
            processes: 256,
            cpus: 2,
        }
    }
}

/// A directory made visible inside the sandbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    /// The directory on the host.
    pub host: PathBuf,
    /// Where it appears inside.
    pub inside: String,
    /// Whether the command may change it.
    pub writable: bool,
}

impl Mount {
    /// The workspace `dir`, read-write at [`WORKSPACE`].
    pub fn workspace(dir: &Path) -> Mount {
        Mount {
            host: dir.to_path_buf(),
            inside: WORKSPACE.into(),
            writable: true,
        }
    }
}

/// A network that reaches only what a filtering proxy lets through: the
/// sandbox has none of its own, and a bridge inside it hands connections
/// to the egress proxy outside ([`crate::egress`]). Bubblewrap only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filtered {
    /// The directory holding the egress proxy's socket.
    pub socket_dir: PathBuf,
    /// The bridge program, then the arguments before its own (the `sion`
    /// command and `sandbox-bridge`, or the `sion-sandbox-bridge` program).
    pub bridge: Vec<String>,
}

/// A sandbox: a backend, a network and limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sandbox {
    /// What runs the commands.
    pub backend: Backend,
    /// The network.
    #[serde(default)]
    pub network: Network,
    /// The limits.
    #[serde(default)]
    pub limits: Limits,
    /// Directories shown read-only at their own paths (a toolchain under
    /// `/opt`, say). With bubblewrap, each one's `bin` directory goes on
    /// the `PATH`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub readable: Vec<PathBuf>,
}

/// What a command did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// Its exit status, if it exited (none when it was stopped or killed).
    pub exit: Option<i32>,
    /// What it wrote to standard output (lossy UTF-8, cut at the limit).
    pub stdout: String,
    /// What it wrote to standard error.
    pub stderr: String,
    /// Whether it was stopped for running too long.
    pub timed_out: bool,
    /// Whether output was cut.
    pub truncated: bool,
}

impl Run {
    /// Whether it exited with status 0.
    pub fn succeeded(&self) -> bool {
        self.exit == Some(0) && !self.timed_out
    }

    /// The run as the model sees it: its status, then its output.
    pub fn render(&self) -> String {
        let mut text = match (self.exit, self.timed_out) {
            (_, true) => "stopped: it ran longer than its time limit".to_string(),
            (Some(code), false) => format!("exit status {code}"),
            (None, false) => "killed by a signal".to_string(),
        };
        if !self.stdout.is_empty() {
            text.push_str("\n--- stdout\n");
            text.push_str(&self.stdout);
        }
        if !self.stderr.is_empty() {
            text.push_str("\n--- stderr\n");
            text.push_str(&self.stderr);
        }
        if self.truncated {
            text.push_str("\n(output cut at the sandbox's limit)");
        }
        text
    }
}

/// Whether `image` is pinned by digest: a name, `@sha256:` and 64
/// lowercase hex digits.
pub fn pinned(image: &str) -> bool {
    match image.split_once("@sha256:") {
        Some((name, digest)) => {
            !name.is_empty()
                && digest.len() == 64
                && digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }
        None => false,
    }
}

/// `text` quoted for a POSIX shell: in single quotes, each single quote
/// closed, escaped and reopened.
pub fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The directories of the system's programs and libraries, bound read-only
/// (or recreated as the links they are, where `/bin` points into `/usr`).
const SYSTEM_DIRS: [&str; 7] = [
    "/usr", "/bin", "/sbin", "/lib", "/lib64", "/lib32", "/libx32",
];

/// Files of `/etc` a command may need: certificates, the dynamic linker's
/// cache, the alternatives programs link through, the time zone.
const ETC_FILES: [&str; 7] = [
    "/etc/ssl",
    "/etc/ca-certificates",
    "/etc/ld.so.cache",
    "/etc/alternatives",
    "/etc/localtime",
    "/etc/mime.types",
    "/etc/fonts",
];

/// Files of `/etc` a command needs to resolve names, with network.
const ETC_NETWORK: [&str; 3] = ["/etc/resolv.conf", "/etc/hosts", "/etc/nsswitch.conf"];

/// The base `PATH` under bubblewrap.
const PATH: &str = "/usr/local/bin:/usr/bin:/bin";

static RUNS: AtomicU64 = AtomicU64::new(0);

/// A user database naming the sandbox's one user, so that programs that
/// look their user up (`whoami`, `ssh`, `git`) find it. Written once per
/// process, in the temporary directory.
fn user_database() -> Option<(PathBuf, PathBuf)> {
    let dir = std::env::temp_dir().join(format!("sion-sandbox-etc-{}", std::process::id()));
    let passwd = dir.join("passwd");
    let group = dir.join("group");
    if !passwd.exists() || !group.exists() {
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(
            &passwd,
            format!("sandbox:x:{SANDBOX_UID}:{SANDBOX_UID}:sandbox:{WORKSPACE}:/bin/sh\n"),
        )
        .ok()?;
        std::fs::write(&group, format!("sandbox:x:{SANDBOX_UID}:\n")).ok()?;
    }
    Some((passwd, group))
}

/// The owner of `dir` (its user and group), for a container to run as.
#[cfg(unix)]
pub(crate) fn owner(dir: &Path) -> Option<(u32, u32)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(dir).ok().map(|m| (m.uid(), m.gid()))
}

#[cfg(not(unix))]
pub(crate) fn owner(_dir: &Path) -> Option<(u32, u32)> {
    None
}

impl Sandbox {
    /// A sandbox on `backend` with no network and the default limits.
    pub fn new(backend: Backend) -> Sandbox {
        Sandbox {
            backend,
            network: Network::None,
            limits: Limits::default(),
            readable: Vec::new(),
        }
    }

    /// The sandbox in a few words, for the model and for people.
    pub fn describe(&self) -> String {
        let backend = match &self.backend {
            Backend::Bubblewrap => "bubblewrap".to_string(),
            Backend::Docker { image } => format!("Docker ({image})"),
        };
        format!(
            "{backend}, {}, stopped after {} s",
            self.network.describe(),
            self.limits.timeout_secs
        )
    }

    /// Why this sandbox cannot run commands, if it cannot.
    pub fn check(&self) -> Result<(), String> {
        match (&self.backend, &self.network) {
            (Backend::Bubblewrap, Network::Docker(_)) => {
                return Err("a Docker network needs the Docker backend".into())
            }
            (Backend::Docker { .. }, Network::Declared) => {
                return Err(
                    "the declared network is filtered for bubblewrap; with Docker, name a network \
                     you set up to restrict where commands connect"
                        .into(),
                )
            }
            (Backend::Docker { .. }, Network::Host) => {
                return Err(
                    "with Docker, name a network you set up to restrict where commands connect, \
                     not the host's"
                        .into(),
                )
            }
            (Backend::Docker { .. }, Network::Docker(name))
                if matches!(name.as_str(), "host" | "bridge" | "default" | "") =>
            {
                return Err(format!(
                    "Docker's {name:?} network does not restrict where a command connects: \
                     name a network you set up"
                ))
            }
            _ => {}
        }
        if let Backend::Docker { image } = &self.backend {
            if !pinned(image) {
                return Err(format!(
                    "{image} is not pinned by digest: give name@sha256:<64 hex digits>"
                ));
            }
        }
        if self.limits.timeout_secs == 0 || self.limits.cpu_seconds == 0 {
            return Err("a sandbox's time limits must be at least one second".into());
        }
        for dir in &self.readable {
            if !dir.is_absolute() || !dir.is_dir() {
                return Err(format!(
                    "{} is not an absolute path to a directory",
                    dir.display()
                ));
            }
        }
        Ok(())
    }

    /// The process that runs `script` with `mounts`, starting in `workdir`
    /// (a path inside the sandbox); `name` names a container.
    pub fn command(&self, mounts: &[Mount], workdir: &str, script: &str, name: &str) -> Command {
        self.build(mounts, workdir, script, name, false, &[])
    }

    /// The process for `script`; one that `serves` lives as long as its
    /// caller wants (a server): no CPU-time limit, and an open input.
    fn build(
        &self,
        mounts: &[Mount],
        workdir: &str,
        script: &str,
        name: &str,
        serves: bool,
        files: &[(PathBuf, String)],
    ) -> Command {
        // Limits a shell sets on itself, before running the script in a
        // child shell under them; `$1` keeps the script out of any quoting.
        // `ulimit -f` counts blocks of 512 bytes.
        let memory = match self.backend {
            // Docker bounds the container's memory; bubblewrap, each
            // process's address space.
            Backend::Bubblewrap if self.limits.memory_mib > 0 => {
                format!("ulimit -v {}; ", self.limits.memory_mib * 1024)
            }
            _ => String::new(),
        };
        let cpu = if serves {
            String::new()
        } else {
            format!("ulimit -t {}; ", self.limits.cpu_seconds)
        };
        let limits = format!(
            "ulimit -c 0; {cpu}ulimit -f {}; {memory}exec /bin/sh -c \"$1\"",
            self.limits.file_mib * 2048,
        );
        match &self.backend {
            Backend::Bubblewrap => self.bubblewrap(mounts, workdir, script, &limits, files),
            Backend::Docker { image } => {
                let mut cmd = self.docker(image, mounts, workdir, script, &limits, name);
                if serves {
                    // `docker run -i`: the server reads its requests there.
                    let mut args: Vec<std::ffi::OsString> =
                        cmd.get_args().map(Into::into).collect();
                    args.insert(1, "-i".into());
                    cmd = Command::new("docker");
                    cmd.args(args);
                }
                cmd
            }
        }
    }

    fn bubblewrap(
        &self,
        mounts: &[Mount],
        workdir: &str,
        script: &str,
        limits: &str,
        files: &[(PathBuf, String)],
    ) -> Command {
        let mut cmd = Command::new("bwrap");
        cmd.arg("--unshare-all");
        if self.network.reaches_out() {
            cmd.arg("--share-net");
        }
        cmd.args(["--die-with-parent", "--new-session", "--cap-drop", "ALL"]);
        cmd.args(["--uid", &SANDBOX_UID.to_string()]);
        cmd.args(["--gid", &SANDBOX_UID.to_string()]);
        for dir in SYSTEM_DIRS {
            let path = Path::new(dir);
            match std::fs::symlink_metadata(path) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    if let Ok(target) = std::fs::read_link(path) {
                        cmd.arg("--symlink").arg(target).arg(dir);
                    }
                }
                Ok(_) => {
                    cmd.args(["--ro-bind", dir, dir]);
                }
                Err(_) => {}
            }
        }
        for file in ETC_FILES {
            cmd.args(["--ro-bind-try", file, file]);
        }
        if self.network.reaches_out() {
            for file in ETC_NETWORK {
                cmd.args(["--ro-bind-try", file, file]);
            }
        }
        if let Some((passwd, group)) = user_database() {
            cmd.arg("--ro-bind").arg(passwd).arg("/etc/passwd");
            cmd.arg("--ro-bind").arg(group).arg("/etc/group");
        }
        cmd.args(["--proc", "/proc", "--dev", "/dev"]);
        cmd.args(["--tmpfs", "/tmp", "--tmpfs", "/var/tmp"]);
        let mut path = String::new();
        for dir in &self.readable {
            cmd.arg("--ro-bind").arg(dir).arg(dir);
            let bin = dir.join("bin");
            if bin.is_dir() {
                path.push_str(&bin.to_string_lossy());
                path.push(':');
            }
        }
        path.push_str(PATH);
        for mount in mounts {
            let flag = if mount.writable {
                "--bind"
            } else {
                "--ro-bind"
            };
            cmd.arg(flag).arg(&mount.host).arg(&mount.inside);
        }
        // Single files, read-only (the bridge program).
        for (host, inside) in files {
            cmd.arg("--ro-bind").arg(host).arg(inside);
        }
        cmd.args(["--chdir", workdir]);
        // Nothing of the runtime's environment reaches the command: the
        // sandbox starts from bubblewrap's, which is empty but for what a
        // caller passes on (as variables, never as arguments that other
        // users could read), and the settings below.
        cmd.env_clear();
        for (key, value) in [
            ("PATH", path.as_str()),
            ("HOME", WORKSPACE),
            ("USER", "sandbox"),
            ("LOGNAME", "sandbox"),
            ("TMPDIR", "/tmp"),
            ("LANG", "C.UTF-8"),
        ] {
            cmd.args(["--setenv", key, value]);
        }
        cmd.args(["--", "/bin/sh", "-c", limits, "sandbox", script]);
        cmd
    }

    fn docker(
        &self,
        image: &str,
        mounts: &[Mount],
        workdir: &str,
        script: &str,
        limits: &str,
        name: &str,
    ) -> Command {
        let mut cmd = Command::new("docker");
        cmd.args(["run", "--rm", "--pull", "never", "--name", name]);
        cmd.args(["--label", DOCKER_LABEL]);
        match &self.network {
            Network::Docker(network) => cmd.args(["--network", network]),
            _ => cmd.args(["--network", "none"]),
        };
        cmd.args([
            "--read-only",
            "--tmpfs",
            "/tmp:rw,size=256m",
            "--cap-drop",
            "ALL",
            "--security-opt",
            "no-new-privileges",
        ]);
        cmd.arg("--pids-limit")
            .arg(self.limits.processes.to_string());
        if self.limits.memory_mib > 0 {
            cmd.arg("--memory")
                .arg(format!("{}m", self.limits.memory_mib));
            cmd.arg("--memory-swap")
                .arg(format!("{}m", self.limits.memory_mib));
        }
        cmd.arg("--cpus").arg(self.limits.cpus.to_string());
        // The workspace's owner, so that the files it writes are theirs and
        // it can write the files they own.
        let (uid, gid) = mounts
            .iter()
            .find(|m| m.writable)
            .and_then(|m| owner(&m.host))
            .unwrap_or((SANDBOX_UID, SANDBOX_UID));
        cmd.arg("--user").arg(format!("{uid}:{gid}"));
        cmd.args(["--env", "HOME=/workspace", "--env", "LANG=C.UTF-8"]);
        for dir in &self.readable {
            cmd.arg("--volume")
                .arg(format!("{}:{}:ro", dir.display(), dir.display()));
        }
        for mount in mounts {
            let mode = if mount.writable { "rw" } else { "ro" };
            cmd.arg("--volume")
                .arg(format!("{}:{}:{mode}", mount.host.display(), mount.inside));
        }
        cmd.args(["--workdir", workdir, image]);
        cmd.args(["/bin/sh", "-c", limits, "sandbox", script]);
        cmd
    }

    /// The mounts with their directories made absolute (Docker takes a
    /// name without a slash for a volume of its own, not a directory), and
    /// a fresh name for a container.
    fn ready(&self, mounts: &[Mount]) -> Result<(Vec<Mount>, String), String> {
        self.check()?;
        let mut absolute = Vec::with_capacity(mounts.len());
        for mount in mounts {
            if !mount.host.is_dir() {
                return Err(format!("{} is not a directory", mount.host.display()));
            }
            if !mount.inside.starts_with('/') || mount.inside == "/" {
                return Err(format!("{} is not a place to mount", mount.inside));
            }
            let host = mount
                .host
                .canonicalize()
                .map_err(|e| format!("{}: {e}", mount.host.display()))?;
            absolute.push(Mount {
                host,
                ..mount.clone()
            });
        }
        let name = format!(
            "sion-sandbox-{}-{}",
            std::process::id(),
            RUNS.fetch_add(1, Ordering::Relaxed)
        );
        Ok((absolute, name))
    }

    /// Starts `argv` with `mounts` in `workdir` and leaves it running, its
    /// standard input and output piped (for a server that speaks over
    /// them) and its standard error discarded. `env` is passed on as
    /// variables. Returns the process and its container's name; the
    /// caller stops both ([`Sandbox::stop`]).
    pub fn spawn(
        &self,
        mounts: &[Mount],
        workdir: &str,
        argv: &[String],
        env: &[(String, String)],
    ) -> Result<(Child, String), String> {
        self.spawn_with(mounts, workdir, argv, env, None)
    }

    /// Starts `argv` as [`Sandbox::spawn`] does, with, given `filtered`, a
    /// network only through the egress proxy (as [`Sandbox::run_with`]).
    pub fn spawn_with(
        &self,
        mounts: &[Mount],
        workdir: &str,
        argv: &[String],
        env: &[(String, String)],
        filtered: Option<&Filtered>,
    ) -> Result<(Child, String), String> {
        if argv.is_empty() {
            return Err("no command to start".into());
        }
        let mut env = env.to_vec();
        let mut files = Vec::new();
        let mut mounts = mounts.to_vec();
        let mut script = format!(
            "exec {}",
            argv.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")
        );
        let this = match filtered {
            Some(filtered) => {
                self.filter(filtered, &mut mounts, &mut files, &mut script, &mut env)?
            }
            None => self.clone(),
        };
        let env = env.as_slice();
        let (mounts, name) = this.ready(&mounts)?;
        let mounts = mounts.as_slice();
        let mut command = this.build(mounts, workdir, &script, &name, true, &files);
        if let Backend::Docker { .. } = self.backend {
            // `--env NAME` takes the value from the client's environment.
            let mut args: Vec<std::ffi::OsString> = command.get_args().map(Into::into).collect();
            let at = args
                .iter()
                .position(|a| a == "--workdir")
                .unwrap_or(args.len());
            for (key, _) in env {
                args.insert(at, key.into());
                args.insert(at, "--env".into());
            }
            let mut docker = Command::new("docker");
            docker.args(args);
            command = docker;
        }
        command
            .envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let child = command.spawn().map_err(|e| match &self.backend {
            Backend::Bubblewrap => format!("bubblewrap (bwrap) could not start: {e}"),
            Backend::Docker { .. } => format!("docker could not start: {e}"),
        })?;
        Ok((child, name))
    }

    /// Stops a process [`Sandbox::spawn`] started, and its container.
    pub fn stop(&self, child: &mut Child, name: &str) {
        if let Backend::Docker { .. } = self.backend {
            let _ = Command::new("docker")
                .args(["kill", name])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = child.kill();
        let _ = child.wait();
    }

    /// Runs `script` with `mounts`, starting in `workdir`, and waits for it
    /// within the time limit.
    pub fn run(&self, mounts: &[Mount], workdir: &str, script: &str) -> Result<Run, String> {
        self.run_with(mounts, workdir, script, &[], None)
    }

    /// Runs `script` as [`Sandbox::run`] does, with `env` passed on as
    /// variables (never as arguments, which other users of the machine can
    /// read) and, with `filtered`, a network only through the egress proxy:
    /// the sandbox itself gets none, whatever its own network says.
    pub fn run_with(
        &self,
        mounts: &[Mount],
        workdir: &str,
        script: &str,
        env: &[(String, String)],
        filtered: Option<&Filtered>,
    ) -> Result<Run, String> {
        let mut env = env.to_vec();
        let mut files = Vec::new();
        let mut mounts = mounts.to_vec();
        let mut script = script.to_string();
        let this = match filtered {
            Some(filtered) => {
                self.filter(filtered, &mut mounts, &mut files, &mut script, &mut env)?
            }
            None => self.clone(),
        };
        let (mounts, name) = this.ready(&mounts)?;
        let mounts = mounts.as_slice();
        let mut command = this.build(mounts, workdir, &script, &name, false, &files);
        if let Backend::Docker { .. } = this.backend {
            // `--env NAME` takes the value from the client's environment.
            let mut args: Vec<std::ffi::OsString> = command.get_args().map(Into::into).collect();
            let at = args
                .iter()
                .position(|a| a == "--workdir")
                .unwrap_or(args.len());
            for (key, _) in &env {
                args.insert(at, key.into());
                args.insert(at, "--env".into());
            }
            let mut docker = Command::new("docker");
            docker.args(args);
            command = docker;
        }
        command
            .envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|e| match &self.backend {
            Backend::Bubblewrap => format!("bubblewrap (bwrap) could not start: {e}"),
            Backend::Docker { .. } => format!("docker could not start: {e}"),
        })?;
        let stdout = capture(child.stdout.take(), self.limits.output_bytes);
        let stderr = capture(child.stderr.take(), self.limits.output_bytes);
        let (exit, timed_out) = this.wait(&mut child, &name)?;
        let (stdout, cut_out) = stdout.join().unwrap_or_default();
        let (stderr, cut_err) = stderr.join().unwrap_or_default();
        Ok(Run {
            exit,
            stdout,
            stderr,
            timed_out,
            truncated: cut_out || cut_err,
        })
    }

    /// What a command with a `filtered` network needs: no network of the
    /// sandbox's own, the proxy's socket and the bridge program bound in,
    /// `script` started by the bridge, and the proxy's variables in `env`.
    /// Returns the sandbox to run it in.
    fn filter(
        &self,
        filtered: &Filtered,
        mounts: &mut Vec<Mount>,
        files: &mut Vec<(PathBuf, String)>,
        script: &mut String,
        env: &mut Vec<(String, String)>,
    ) -> Result<Sandbox, String> {
        if !matches!(self.backend, Backend::Bubblewrap) {
            return Err(
                "a network filtered by host needs the bubblewrap backend; with Docker, the operator's network decides"
                    .into(),
            );
        }
        let Some((program, before)) = filtered.bridge.split_first() else {
            return Err("no bridge program for the filtered network".into());
        };
        let mut this = self.clone();
        this.network = Network::None;
        mounts.push(Mount {
            host: filtered.socket_dir.clone(),
            inside: crate::egress::SOCKET_DIR_INSIDE.into(),
            writable: true,
        });
        files.push((
            PathBuf::from(program),
            crate::egress::BRIDGE_INSIDE.to_string(),
        ));
        let bridge = std::iter::once(crate::egress::BRIDGE_INSIDE.to_string())
            .chain(before.iter().cloned())
            .chain([
                crate::egress::LISTEN.to_string(),
                format!(
                    "{}/{}",
                    crate::egress::SOCKET_DIR_INSIDE,
                    crate::egress::SOCKET_NAME
                ),
                "/bin/sh".into(),
                "-c".into(),
                script.clone(),
            ])
            .map(|a| quote(&a))
            .collect::<Vec<_>>()
            .join(" ");
        *script = format!("exec {bridge}");
        env.extend(crate::egress::proxy_env());
        Ok(this)
    }

    /// Waits for `child` within the time limit; stops it, and its
    /// container, when it runs over.
    fn wait(&self, child: &mut Child, name: &str) -> Result<(Option<i32>, bool), String> {
        let deadline = Instant::now() + Duration::from_secs(self.limits.timeout_secs);
        let mut pause = Duration::from_millis(1);
        loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                return Ok((status.code(), false));
            }
            if Instant::now() >= deadline {
                self.stop(child, name);
                return Ok((None, true));
            }
            std::thread::sleep(pause);
            pause = (pause * 2).min(Duration::from_millis(20));
        }
    }

    /// Whether this sandbox can run a trivial command here: its backend is
    /// installed and allowed to make its namespaces or containers.
    pub fn probe(&self) -> Result<(), String> {
        let dir = std::env::temp_dir().join(format!(
            "sion-sandbox-probe-{}-{}",
            std::process::id(),
            RUNS.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let result = self.run(&[Mount::workspace(&dir)], WORKSPACE, "true");
        let _ = std::fs::remove_dir_all(&dir);
        match result {
            Ok(run) if run.succeeded() => Ok(()),
            Ok(run) => Err(format!("the sandbox does not work here: {}", run.render())),
            Err(e) => Err(e),
        }
    }
}

/// Reads a child's stream on a thread of its own, keeping at most `limit`
/// bytes and draining the rest, so that a chatty command never blocks on a
/// full pipe.
fn capture<R: Read + Send + 'static>(
    stream: Option<R>,
    limit: usize,
) -> JoinHandle<(String, bool)> {
    std::thread::spawn(move || {
        let Some(mut stream) = stream else {
            return (String::new(), false);
        };
        let mut kept = Vec::new();
        let mut cut = false;
        let mut buffer = [0u8; 8192];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let room = limit.saturating_sub(kept.len());
                    if n > room {
                        cut = true;
                    }
                    kept.extend_from_slice(&buffer[..n.min(room)]);
                }
            }
        }
        (String::from_utf8_lossy(&kept).into_owned(), cut)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sion-sandbox-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A bubblewrap sandbox, if bubblewrap works on this machine.
    fn bubblewrap() -> Option<Sandbox> {
        let sandbox = Sandbox::new(Backend::Bubblewrap);
        match sandbox.probe() {
            Ok(()) => Some(sandbox),
            Err(e) => {
                eprintln!("skipped: bubblewrap does not work here: {e}");
                None
            }
        }
    }

    #[test]
    fn images_must_be_pinned_by_digest() {
        let digest = "a".repeat(64);
        assert!(pinned(&format!("python@sha256:{digest}")));
        assert!(pinned(&format!(
            "registry:5000/team/python@sha256:{digest}"
        )));
        assert!(!pinned("python:3.13"));
        assert!(!pinned(&format!("@sha256:{digest}")));
        assert!(!pinned(&format!("python@sha256:{}", "A".repeat(64))));
        assert!(!pinned("python@sha256:abc"));
        let sandbox = Sandbox::new(Backend::Docker {
            image: "python:3.13".into(),
        });
        assert!(sandbox.check().unwrap_err().contains("pinned by digest"));
    }

    #[test]
    fn quoting_survives_any_text() {
        assert_eq!(quote("plain"), "'plain'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        let tricky = "a'b\"c$(rm -rf x)`d`\\e\n";
        let run = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("printf %s {}", quote(tricky)))
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(run.stdout).unwrap(), tricky);
    }

    #[test]
    fn docker_runs_offline_from_a_local_image_without_privileges() {
        let image = format!("python@sha256:{}", "b".repeat(64));
        let sandbox = Sandbox::new(Backend::Docker {
            image: image.clone(),
        });
        let ws = temp("docker-args");
        let mounts = [Mount::workspace(&ws)];
        let cmd = sandbox.command(&mounts, WORKSPACE, "echo hi", "sion-sandbox-test");
        let given = args(&cmd);
        let joined = given.join(" ");
        for expected in [
            "--pull never",
            "--network none",
            "--read-only",
            "--cap-drop ALL",
            "--security-opt no-new-privileges",
            "--pids-limit 256",
            "--memory 4096m",
            "--cpus 2",
            &format!("--volume {}:/workspace:rw", ws.display()),
        ] {
            assert!(
                joined.contains(expected),
                "{expected} missing from {joined}"
            );
        }
        #[cfg(unix)]
        {
            let (uid, gid) = owner(&ws).unwrap();
            assert!(joined.contains(&format!("--user {uid}:{gid}")), "{joined}");
        }
        assert!(given.contains(&image));
        assert_eq!(given.last().map(String::as_str), Some("echo hi"));
        // Docker's host network and default bridge are refused, and so is
        // the host's network.
        for network in ["host", "bridge"] {
            let open = Sandbox {
                network: Network::Docker(network.into()),
                ..sandbox.clone()
            };
            assert!(open.check().is_err());
        }
        let host = Sandbox {
            network: Network::Host,
            ..sandbox.clone()
        };
        assert!(host.check().is_err());
        let filtered = Sandbox {
            network: Network::Docker("sion-egress".into()),
            ..sandbox
        };
        assert!(filtered.check().is_ok());
        assert!(args(&filtered.command(&mounts, WORKSPACE, "true", "n"))
            .join(" ")
            .contains("--network sion-egress"));
        std::fs::remove_dir_all(&ws).unwrap();
    }

    #[test]
    fn bubblewrap_unshares_everything_and_shares_the_network_only_when_allowed() {
        let sandbox = Sandbox::new(Backend::Bubblewrap);
        let mounts = [Mount {
            host: PathBuf::from("/tmp/ws"),
            inside: WORKSPACE.into(),
            writable: true,
        }];
        let command = sandbox.command(&mounts, WORKSPACE, "true", "n");
        // bubblewrap starts from an empty environment.
        assert!(command.get_envs().all(|(_, value)| value.is_none()));
        let offline = args(&command).join(" ");
        assert!(offline.contains("--unshare-all"));
        assert!(!offline.contains("--share-net"));
        assert!(offline.contains("--cap-drop ALL"));
        assert!(offline.contains("--bind /tmp/ws /workspace"));
        assert!(!offline.contains("/etc/resolv.conf"));
        let online = Sandbox {
            network: Network::Host,
            ..sandbox
        };
        let online = args(&online.command(&mounts, WORKSPACE, "true", "n")).join(" ");
        assert!(online.contains("--share-net"));
        assert!(online.contains("/etc/resolv.conf"));
    }

    #[test]
    fn mounts_are_given_as_absolute_directories() {
        // Docker would take a relative path for a volume of its own name.
        let sandbox = Sandbox::new(Backend::Docker {
            image: format!("x@sha256:{}", "c".repeat(64)),
        });
        let (mounts, _) = sandbox.ready(&[Mount::workspace(Path::new("."))]).unwrap();
        assert!(mounts[0].host.is_absolute(), "{:?}", mounts[0].host);
        assert!(sandbox
            .ready(&[Mount::workspace(Path::new("no-such-directory"))])
            .is_err());
    }

    #[test]
    fn output_is_cut_at_the_limit_and_the_rest_drained() {
        let text = vec![b'x'; 100_000];
        let (kept, cut) = capture(Some(std::io::Cursor::new(text)), 1000)
            .join()
            .unwrap();
        assert_eq!(kept.len(), 1000);
        assert!(cut);
    }

    #[test]
    fn a_command_writes_only_its_workspace_and_sees_no_home() {
        let Some(sandbox) = bubblewrap() else { return };
        let ws = temp("bwrap-ws");
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
        let script = format!(
            "echo made > made.txt; id -u; whoami; \
             touch /usr/escape 2>/dev/null && echo wrote-usr; \
             ls {} 2>/dev/null && echo saw-home; \
             echo $HOME",
            quote(&home)
        );
        let run = sandbox
            .run(&[Mount::workspace(&ws)], WORKSPACE, &script)
            .unwrap();
        assert!(run.succeeded(), "{}", run.render());
        assert_eq!(
            run.stdout,
            format!("{SANDBOX_UID}\nsandbox\n{WORKSPACE}\n"),
            "{}",
            run.render()
        );
        assert_eq!(
            std::fs::read_to_string(ws.join("made.txt")).unwrap(),
            "made\n"
        );
        std::fs::remove_dir_all(&ws).unwrap();
    }

    #[test]
    fn a_command_without_network_cannot_connect() {
        let Some(sandbox) = bubblewrap() else { return };
        let ws = temp("bwrap-net");
        // Only the loopback interface: no route anywhere.
        let run = sandbox
            .run(
                &[Mount::workspace(&ws)],
                WORKSPACE,
                "cat /proc/net/route | wc -l; ls /sys/class/net 2>/dev/null || cat /proc/net/dev | tail -n +3 | cut -d: -f1",
            )
            .unwrap();
        assert!(run.succeeded(), "{}", run.render());
        let lines: Vec<&str> = run.stdout.lines().map(str::trim).collect();
        // The route table has only its header line.
        assert_eq!(lines[0], "1", "{}", run.render());
        assert!(lines[1..].iter().all(|l| *l == "lo"), "{}", run.render());
        std::fs::remove_dir_all(&ws).unwrap();
    }

    #[test]
    fn a_command_that_runs_too_long_is_stopped_and_output_is_bounded() {
        let Some(mut sandbox) = bubblewrap() else {
            return;
        };
        sandbox.limits.timeout_secs = 1;
        sandbox.limits.output_bytes = 100;
        let ws = temp("bwrap-limits");
        let started = Instant::now();
        let run = sandbox
            .run(
                &[Mount::workspace(&ws)],
                WORKSPACE,
                "yes | head -c 5000; sleep 30",
            )
            .unwrap();
        assert!(run.timed_out, "{}", run.render());
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(run.stdout.len(), 100);
        assert!(run.truncated);
        assert!(run.render().starts_with("stopped"));
        // A file larger than the limit is cut off.
        sandbox.limits.timeout_secs = 30;
        sandbox.limits.file_mib = 1;
        let run = sandbox
            .run(
                &[Mount::workspace(&ws)],
                WORKSPACE,
                "head -c 3000000 /dev/zero > big; echo $?",
            )
            .unwrap();
        assert_ne!(run.stdout.trim(), "0", "{}", run.render());
        assert!(std::fs::metadata(ws.join("big")).unwrap().len() <= 1 << 20);
        std::fs::remove_dir_all(&ws).unwrap();
    }

    /// A Docker sandbox of the image `SION_TEST_DOCKER_IMAGE` names (pinned
    /// by digest and already on the machine), if Docker works here.
    fn docker() -> Option<Sandbox> {
        let Ok(image) = std::env::var("SION_TEST_DOCKER_IMAGE") else {
            eprintln!("skipped: SION_TEST_DOCKER_IMAGE names no image");
            return None;
        };
        let sandbox = Sandbox::new(Backend::Docker { image });
        match sandbox.probe() {
            Ok(()) => Some(sandbox),
            Err(e) => {
                eprintln!("skipped: Docker does not work here: {e}");
                None
            }
        }
    }

    #[test]
    fn a_container_writes_as_the_workspace_owner_and_serves_over_its_input() {
        use std::io::{BufRead, Write};
        let Some(sandbox) = docker() else { return };
        let ws = temp("docker-ws");
        let run = sandbox
            .run(
                &[Mount::workspace(&ws)],
                WORKSPACE,
                "echo made > made.txt; id -u; touch /etc/x 2>/dev/null && echo wrote-etc; ip route 2>/dev/null | wc -l",
            )
            .unwrap();
        assert!(run.succeeded(), "{}", run.render());
        #[cfg(unix)]
        {
            let (uid, _) = owner(&ws).unwrap();
            assert_eq!(run.stdout, format!("{uid}\n0\n"), "{}", run.render());
        }
        assert_eq!(
            std::fs::read_to_string(ws.join("made.txt")).unwrap(),
            "made\n"
        );
        // A server: `cat` answers each line it reads, for as long as it runs.
        let (mut child, name) = sandbox
            .spawn(
                &[Mount::workspace(&ws)],
                WORKSPACE,
                &["cat".to_string()],
                &[],
            )
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
        for line in ["one", "two"] {
            writeln!(input, "{line}").unwrap();
            input.flush().unwrap();
            let mut answer = String::new();
            output.read_line(&mut answer).unwrap();
            assert_eq!(answer.trim_end(), line);
        }
        sandbox.stop(&mut child, &name);
        std::fs::remove_dir_all(&ws).unwrap();
    }
}
