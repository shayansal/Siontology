# sion-app-core

`sion-app-core` is the core of the Siontology app, shared by its desktop and
mobile shells. Every install is a light node: it holds the device's own key
and checks what it shows instead of trusting the node it talks to. The
central call is `AppCore::verify_latest`, which verifies the latest certified
Pulse the way `sion verify` does. The core also pays, delegates stake to
validators, can run a storage provider from the device, and watches agents the
device backs. Every method returns JSON that a shell hands to its interface
unchanged.

Part of [Siontology](../../README.md) · [Architecture](../../ARCHITECTURE.md)
· Status: **Partly built** (the desktop shell is built and run; the mobile
builds are configured and not built; see
[implementation status](../../docs/implementation-status.md)).

## What is in it

The crate is a single file, `src/lib.rs`.

| Item | What it holds |
|---|---|
| `AppCore` | The app's state and all its operations; `Send + Sync`, every method takes `&self` |
| `Settings` | What the app keeps in `settings.json` (node address, pinned chain, storage settings, watched agents) |
| `Verified` | The result of one verification: the Pulse, whether every check passed, whether the chain was pinned now, and one line per check |
| `AppError` | Storage, ledger (from `sion-cli`) and input errors |
| `DEFAULT_SAMPLES` | Shares of each data square a phone samples (16) |

## Main types and functions

- [`AppCore::open`](src/lib.rs): opens or creates the app's directory, holding `key.seed` (mode 0600) and `settings.json`.
- [`AppCore::set_node`](src/lib.rs) and [`AppCore::pin_chain`](src/lib.rs): choose the node and pin the chain identity (nothing is pinned on first use without the person's say).
- [`AppCore::verify_latest`](src/lib.rs): verifies the latest certified Pulse; [`history`](src/lib.rs) keeps the last 50 results.
- [`AppCore::balance`](src/lib.rs), [`faucet`](src/lib.rs) and [`send`](src/lib.rs): balance, devnet faucet and payment; each transaction the device signs returns with its receipt proven against a certified header.
- [`AppCore::pools`](src/lib.rs), [`delegate`](src/lib.rs), [`delegations`](src/lib.rs) and [`delegation_action`](src/lib.rs): pooled stake (specification v1.11).
- [`AppCore::set_storage`](src/lib.rs), [`storage_tick`](src/lib.rs) and [`storage_status`](src/lib.rs): run a storage provider from the device (v1.12).
- [`AppCore::agents`](src/lib.rs), [`find_agents`](src/lib.rs), [`watch_agent`](src/lib.rs) and [`agent_action`](src/lib.rs): watch agents, prove their charters, and suspend, resume or revoke those the device oversees.

## How it fits

Depends on: `sion-core`, `sion-crypto`, `sion-exec`, `sion-light` and
`sion-cli` (for the ledger client, sessions and storage provider code the
command line also uses). Dev-dependencies: `sion-node` and `sion-assurance`
for the tests. Used by: the Tauri 2 shell in `apps/sion-app`, which is its own
Cargo workspace and path-depends on this crate.

The core has no user interface and no window code. It talks to a node only
over HTTP, so it works against any node. Not built, from the status file: the
Android and iOS builds, a key in the platform's keystore (`app:plain-key`),
proofs of the balances it shows (`app:unproven-balances`), bandwidth served by
devices, and installers or code signing. The Agents tab of the shell was
type-checked but not run in a window.

The crate is 1,025 lines of Rust under `src/` at the time of writing.

## Tests

9 tests (counted with `grep -c "#\[test\]"`): 5 unit tests in `src/lib.rs` and
4 in `tests/devnet.rs`, which runs the core against a real devnet served over
HTTP, as a shell uses it. The devnet tests cover an install that verifies the
chain, pays and delegates; a device that follows new epochs without starting
over; a device that provides storage and proves its rounds; and an install that
watches the agents it backs and stops them. The tests take turns, because each
runs a whole devnet. No test is ignored.

```text
cargo test -p sion-app-core
```

## Read more

- [The app](../../docs/book/src/getting-started/app.md)
- [Light client](../../docs/book/src/getting-started/light-client.md)
- [Data availability](../../docs/book/src/concepts/data-availability.md)
- [Delegation](../../docs/book/src/guides/delegation.md)
- [Storage](../../docs/book/src/guides/storage.md)
- [The shell's README](../../apps/sion-app/README.md)
