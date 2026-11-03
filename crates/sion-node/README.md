# sion-node

`sion-node` is the Siontology node: the code that runs validators, a client
gateway and a prover, and serves the HTTP API that wallets, the command line
and light clients use. It runs in two shapes with the same validator and
gateway code. A **devnet** runs every validator of a committee in one
process, each on its own thread, joined by an in-memory network. A
**network** runs one validator per process, joined by QUIC with keys pinned
by a genesis file. Protocol messages never touch a socket directly: they go
through the `Transport` seam, which the in-memory carrier, a fault injector
and the QUIC carrier (over `sion-net`) all implement.

Part of [Siontology](../../README.md) · [Architecture](../../ARCHITECTURE.md)
· Status: **Simplified** (consensus is a certified DAG, the World Proof is a
replay stand-in, and networking is QUIC between processes with peers from
files and no rejoin of a stopped validator; see
[implementation status](../../docs/implementation-status.md)).

## What is in it

| Module | What it holds |
|---|---|
| `devnet` | Assembling and running a devnet: validators, gateway and prover in one process; genesis and chain specifications |
| `config` | `DevnetConfig` |
| `validator` | One validator on its own thread: lane approvals, consensus, Pulse execution, header certificates, data availability, storage |
| `gateway` | The client gateway: routes transactions by coordination class and collects approvals into certificates |
| `keys` | Validator identities and signatures (dual ML-DSA-44 and SLH-DSA-SHA2-128f; a MAC mode for quick experiments) |
| `committees` | Committees derived from the state after every Pulse |
| `forced` | Forced inclusion: censorship becomes visible, then costly |
| `payload` | What consensus orders |
| `pulses` | Cutting the commit sequence into Pulses |
| `owners` | What a validator checks before locking owned inputs |
| `prover` | The prover thread that folds each Pulse into a World Proof |
| `storage` | A segmented write-ahead log of executed Pulses plus snapshots |
| `view` | Queries a validator answers and the views it returns |
| `api` | JSON over HTTP (`Backend`, `serve`); the route table is in the module docs |
| `transport` | The seam (`Transport`, `PeerMessage`), the in-memory carrier, the seeded fault injector, the QUIC carrier and message statistics |
| `network` | A network of processes: genesis file, peer file, key files, `generate_validator` |
| `node` | One node of a network in its own process: `Node`, `GatewayNode`, `claim_session` |
| `error` | `NodeError` |

## Main types and functions

- [`Devnet`](src/devnet.rs) with [`DevnetConfig`](src/config.rs): starts a whole committee in one process.
- [`Node`](src/node.rs): one validator in its own process, optionally hosting a gateway, on a QUIC transport.
- [`GatewayNode`](src/node.rs): a gateway alone; it follows the committees by asking one node it trusts.
- [`Validator`](src/validator.rs): the validator thread both shapes run.
- [`Gateway`](src/gateway.rs): routes a transaction to its shard's committee and gathers a certificate.
- [`Transport`](src/transport/mod.rs): the seam between the protocol and whatever carries its messages.
- [`FaultNet`](src/transport/faulty.rs): seeded loss, delay, duplication, reordering and partitions per message class.
- [`NetworkGenesis`](src/network.rs) and [`PeerFile`](src/network.rs): the files that fix a network's chain and its accepted gateways.
- [`serve`](src/api.rs) with [`Backend`](src/api.rs): the HTTP API over any of the above.
- [`Query`](src/view.rs), [`TxView`](src/view.rs) and [`TxStatus`](src/view.rs): what a caller can ask a validator, and the status of a transaction.

## How it fits

Depends on: `sion-core`, `sion-crypto`, `sion-assurance`, `sion-clearing`,
`sion-consensus`, `sion-exec`, `sion-da`, `sion-light`, `sion-net` and
`sion-prover`; outside the workspace, `tiny_http` serves the API. Used by:
`sion-cli` (`sion node`, `sion gateway` and the devnet commands); `sion-app-core`
uses it in its tests only.

The node holds the protocol, not the user interface: wallets, the agent
runtime and the app live in other crates and reach the node over HTTP.
Simplifications this crate carries, from the status file: a validator that
stops cannot rejoin (`net:no-rejoin`; `claim_session` refuses to start a
session it has already signed in); a standalone gateway trusts the node it
follows (`net:gateway-trusts-node`); there is no drain window at epoch
boundaries and no lane-lock hand-over; the API has no HTTPS; the prover
checks Pulses by replay, a stand-in for succinct proofs
(`impl:replay-proofs`). The devnet's `/v1/devnet/censor` route is an
adversary simulation.

The crate is 11,330 lines of Rust under `src/` at the time of writing.

## Tests

42 tests in `src/` and `tests/` (counted with `grep -c "#\[test\]"`).

- Unit tests in `src/`: 34, among them `src/transport/tests.rs` (6: what travels encodes canonically within its class's limit and the in-memory carrier delivers it), `src/transport/faulty.rs` (5), `src/payload.rs` (4), and `network`, `owners`, `pulses`, `committees`, `storage` and others. `src/transport/tests.rs` has property tests (proptest).
- `tests/devnet.rs` (4): real devnets with dual post-quantum signatures. A transfer is certified, ordered, executed, proven and light-verified; contended and knowledge transactions flow and state survives restarts; a burst larger than one consensus payload still executes; keyless subagents pay in one batch.
- `tests/transport.rs` (4): a devnet whose every message passes the fault injector at zero faults behaves like one on the plain transport, and the protocol's retries carry it through loss. These devnets sign with the insecure simulation MAC to run in seconds.
- The multi-process network (four `sion node` processes and a gateway) is tested in `crates/sion-cli/tests/network.rs`, not here.

No test in this crate is ignored. Devnet tests take turns (they hold a lock)
because two at once starve each other's validators on a small machine.

```text
cargo test -p sion-node
```

## Read more

- [Crates](../../docs/book/src/architecture/crates.md)
- [Validator](../../docs/book/src/architecture/validator.md)
- [Gateway](../../docs/book/src/architecture/gateway.md)
- [Storage](../../docs/book/src/architecture/storage.md)
- [Transaction flow](../../docs/book/src/architecture/transaction-flow.md)
- [Prover](../../docs/book/src/architecture/prover.md)
- [Running a devnet](../../docs/book/src/guides/devnet.md)
- [Running a network](../../docs/book/src/guides/network.md)
- [HTTP API reference](../../docs/book/src/reference/http-api.md)
- [Networking plan](../../docs/book/src/research/networking.md)
