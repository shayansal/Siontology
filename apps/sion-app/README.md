# The Siontology app

One Rust core, [`sion-app-core`](../../crates/sion-app-core), with a
[Tauri 2](https://v2.tauri.app) shell for desktop and mobile. Every
install is a **light node**: it keeps the device's own key and verifies
the latest certified Pulse every eight seconds (the committee certificate
with every signature, the receipts root, a state proof and random shares
of the Pulse's data square) against a chain identity pinned on first use
or by its user. It pays and pools stake with validators, and proves the
receipt of every transaction it signs.

The book's page, [The app](../../docs/book/src/getting-started/app.md),
covers what it checks, what it does not, and where its key lives.

| Layer | Where | What |
|---|---|---|
| Core | `crates/sion-app-core` (in the workspace, tested against a devnet over HTTP) | The key (`key.seed`, mode 0600), settings, light-client verification, payments, pooled stake |
| Shell | `apps/sion-app/src-tauri` (its own Cargo workspace, so GUI dependencies stay out of the protocol's) | Tauri commands that forward to the core; the app-data directory |
| Interface | `apps/sion-app/ui` | Static HTML, CSS and JavaScript in the brand's tokens; it holds no key and makes no network call |

## Desktop

Linux needs WebKitGTK 4.1 and its companions:

```bash
sudo apt-get install libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libayatana-appindicator3-dev
cd apps/sion-app/src-tauri
cargo build --release
SION_NODE=http://127.0.0.1:8730 ./target/release/sion-app
```

macOS and Windows need no extra libraries. `SION_NODE` sets the node on
start; the Settings tab changes it later. `SION_APP_DIR` overrides where
the key and settings live (by default the platform's app-data directory,
for example `~/.local/share/org.siontology.app` on Linux).

Installers (`.deb`, `.AppImage`, `.dmg`, `.msi`) come from the Tauri CLI:

```bash
cargo install tauri-cli --version "^2" --locked
cargo tauri build
```

## Mobile

The same project builds for Android and iOS with the Tauri CLI, given the
Android SDK and NDK, or Xcode:

```bash
cargo tauri android init && cargo tauri android build
cargo tauri ios init && cargo tauri ios build
```

**Not built or tested here**: the environment this was written in has no
Android SDK or Xcode. The shell's entry point is already the mobile one
(`#[cfg_attr(mobile, tauri::mobile_entry_point)]`), and the interface is
laid out for phones first.

## What a device does, and what it does not

- **It verifies**: a server cannot pass off a Pulse the chain's committee
  did not certify, or a receipt for one of the device's own transactions
  that is not in one, and its random samples of each Pulse's data help
  catch a producer that withholds data. The balances it shows come from
  the node, unproven.
- **It does not validate or store the chain.** Validators run on servers
  (`sion node`, see the book's *Running a network of processes*); a phone
  earns nothing by running the app. Its owner earns by pooling stake with a
  validator from the Stake tab. Paying desktops to store and serve data is
  designed, not built.
