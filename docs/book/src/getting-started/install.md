# Install from source

Releases are made by the release workflow on version tags
(`scripts/release/README.md`); none had been published when this was
written. Until there is one, you build `sion` from source: by hand, with
the installer, in a container image, or with Homebrew or Nix. For one
person's agent on one machine, [Run it on your computer](your-computer.md)
is the short way.

## Requirements

- **Rust 1.80 or later** (the workspace's `rust-version`). Install it with
  [rustup](https://rustup.rs).
- Git.
- A C compiler: aws-lc, the cryptography under the HTTP client's TLS, is
  built from its C sources. No system library is linked: the ledger's
  cryptography (ML-DSA, SLH-DSA, ML-KEM, ChaCha20-Poly1305, SHA3) is pure
  Rust.

The code builds and runs on Linux. Other platforms supported by Rust should
work but have not been tested.

## Build

```bash
git clone https://github.com/ColdAI-org/siontology
cd siontology
cargo build --release -p sion-cli
./target/release/sion --version
```

```text
sion 0.1.0
```

A cold release build took about ten minutes on the 2-core machine used for
this book. The workspace profiles optimize dependencies even in debug
builds, because post-quantum signing is slow without optimization, and
release builds keep integer overflow checks on.

To put `sion` on your `PATH`:

```bash
cargo install --locked --path crates/sion-cli
```

The rest of this book calls the binary `sion`.

## The installer

`scripts/install.sh` installs `sion` into `~/.local/bin`, or into the
`bin` of the directory `--prefix` names: from the checkout it is in; from
the latest release on GitHub with `--latest`, which is what it does when it
is not in a checkout (`curl … | sh`), building from the repository when no
release can be fetched; from the repository with `--git` (and `--rev` for
a tag or commit); or from a release with `--release` and its base URL. A
release archive is checked against its `SHA256SUMS`, and `SHA256SUMS`
against its SSH signature by the release key (the repository's
`scripts/release/allowed_signers` for `--latest`; with `--release`, the
file `--allowed-signers` names), before anything is installed. A private
repository is read with the GitHub CLI, signed in. `--dry-run` says what it
would do. It never uses root.

```bash
sh scripts/install.sh --dry-run
```

```text
building sion from /home/claude/fold
+ cargo install --locked --path /home/claude/fold/crates/sion-cli --root /home/you/.local
(a dry run: nothing was installed)
```

The release path was run here against a release made for the purpose
and served on this machine: a signed release installed; one checked
against another signer's key, and an archive changed by one byte, were
refused, and nothing was installed. `--latest` was run the same way, with
a stand-in for the GitHub CLI giving it such a release. `sion update`
keeps an installed `sion` current the same way
([Update](your-computer.md#update)).

## A container image

The `Dockerfile` builds `sion` and puts it in a small Debian image,
running as a user of its own, with its home a volume:

```bash
docker build -t sion .
docker run --rm -v sion-home:/home/sion/.sion sion doctor
```

`--build-arg EXTRAS="chromium xvfb xdotool imagemagick"` adds the browser
and the desktop the agents can drive. Bubblewrap is in the image, but it
makes sandboxes only where the container may create user namespaces. The
image has not been built here: this environment has no Docker daemon.

## Homebrew and Nix

`brew install --HEAD ./packaging/homebrew/sion.rb` builds the
repository's head (a release's `url` and `sha256` go in the formula once
there is one). `nix build` and `nix run . -- doctor` use `flake.nix`;
`nix develop` gives the toolchain and the agents' tools. Neither has been
run here: the formula and the flake are checked only for their syntax.

## Services

A bridge, the HTTP channel, the scheduler or a host keeps running as a
service of your own: a systemd user unit on Linux, a launchd agent on
macOS. `--env` copies the variables it needs (a bot's token) into a file
only your user reads (`agent:service-tokens`):

```text
$ TELEGRAM_BOT_TOKEN=… sion agent service install support --env TELEGRAM_BOT_TOKEN -- agent telegram support --pairing
wrote /home/you/.config/systemd/user/sion-support.service
wrote /home/you/.config/sion/services/support.env
start it with: systemctl --user enable --now sion-support.service
$ sion agent service list
support: sion agent telegram support --pairing
```

The unit runs this `sion` with those arguments and this `SION_HOME`,
starts again five seconds after a failure, and holds no token:

```text
[Service]
ExecStart="/home/you/.local/bin/sion" "agent" "telegram" "support" "--pairing"
EnvironmentFile="/home/you/.config/sion/services/support.env"
Restart=on-failure
RestartSec=5
NoNewPrivileges=yes
```

`--start` starts it at once and at each login; `sion agent service remove
support` stops it and takes its files away. Commands that need no node
(pairing, secrets, logins, schedules, services) run without one.

## What is here

`sion doctor` says what this machine has for Siontology and its agents,
and what is missing, naming the variables that are set without showing
them. Here, in the environment this book was made in:

```text
$ sion doctor
ok sion        0.1.0 on linux x86_64
ok home        /home/you/.sion (SION_HOME)
-- node        http://127.0.0.1:8730 does not answer: start one (`sion setup` makes a chain of your own; `sion devnet`) or name another (--node, SION_NODE); only the ledger's commands need it
ok bubblewrap  runs commands in a sandbox of their own
-- docker      installed, but its daemon does not answer: Docker sandboxes need it
ok browser     /opt/pw-browsers/chromium-1243/chrome-linux64/chrome (--browser)
ok desktop     Xvfb, xdotool and ImageMagick are here (--computer)
-- models      no model's key set or kept, and no ChatGPT sign-in: agents run only scripted models (sion setup)
ok voice       no speech service's key set (--hear and --speak can also run a command)
ok messengers  none has its credentials set (see `sion agent bridge --help`)
ok profiles    none yet (`sion setup` makes one; `sion agent profile add`)
ok proxy       HTTPS_PROXY, https_proxy set: requests go through it, its roots from SSL_CERT_FILE
```

## Where things live

| What | Where |
|---|---|
| Wallet seeds | `$SION_HOME/wallets/<name>.seed`, by default `~/.sion/wallets` |
| The node the CLI talks to | `--node` or `$SION_NODE`, by default `http://127.0.0.1:8730` |
| The chain identity you pin | `--chain` or `$SION_CHAIN` |
| A devnet's data | the directory you pass to `sion devnet --data-dir`; without it, nothing is written to disk |
| The chain `sion setup` makes | `$SION_HOME/chain`, and what setup made in `$SION_HOME/setup.json` |
| Models' keys `sion` keeps | `$SION_HOME/agents/credentials.bin`, sealed (`sion agent credential list`) |

## Run the tests

```bash
cargo test --workspace
```

The suite includes property tests for the Assurance Algebra, the
linear-programming certificates and the state machine, a deterministic
consensus simulator with adversarial network phases, scenario tests for
every transaction kind, differential tests of the two proof stacks,
end-to-end devnet tests with dual post-quantum signatures, and every demo.
The demo runs take minutes, so they are skipped unless you ask for them:

```bash
cargo test --workspace -- --include-ignored
```

See `CONTRIBUTING.md` for the full checklist.

## Next

- [Run the demos](demos.md): fourteen scripted scenarios, no setup.
- [Run a devnet](devnet.md) and use it from another terminal.
