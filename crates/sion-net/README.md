# sion-net

`sion-net` carries Siontology's protocol messages between nodes in separate
processes. It holds what every carrier of those messages shares (node
identities, four message classes with size limits, a framing format, network
keys and the signed records that bind them to validators) and the carrier
itself, a QUIC endpoint. Two ideas matter. The receiver takes the sender's
identity from the authenticated connection, never from the message. And a
connection is made only with post-quantum TLS 1.3 (the hybrid key exchange
X25519MLKEM768, ML-DSA-44 raw public keys) with both ends pinned.

Part of [Siontology](../../README.md) · [Architecture](../../ARCHITECTURE.md)
· Status: **Simplified** (QUIC between processes, peers from files, a
stopped validator cannot rejoin; see
[implementation status](../../docs/implementation-status.md)).

## What is in it

| Module | What it holds |
|---|---|
| `id` | `NodeId`: a validator is named by its validator identifier, any other node by the digest of its network key |
| `class` | The four message classes (control, consensus, ingress, bulk), each with a size limit and a priority |
| `frame` | One class byte, a four-byte length and a Borsh body; the length is checked against the class limit before the body is read |
| `key` | A node's ML-DSA-44 network key and its public half |
| `record` | The record, signed with a validator's dual consensus keys, that binds a network key and addresses to the validator; `SignedRecord::peer` turns a verified record into a `Peer` |
| `tls` | TLS 1.3 with X25519MLKEM768 only and ML-DSA-44 raw public keys only, both ends pinned |
| `endpoint` | The QUIC endpoint (`quinn`) on a small tokio runtime of its own: connections, keep-alive, a stream per class and direction, bounded queues |
| `quota` | Per-peer quotas that drop floods |
| `error` | `NetError` |

## Main types and functions

- [`Endpoint`](src/endpoint.rs): starts with `Endpoint::start`, sends a body to a node in a class (`send`), reports who is connected, takes a new peer list (`set_peers`) and shuts down.
- [`EndpointConfig`](src/endpoint.rs): the chain identity, the local node, its key, the listen address (none for a dial-only gateway), the peers, class limits, queue limits and quotas.
- [`Deliver`](src/endpoint.rs): the callback an endpoint calls with the authenticated sender, the class and the body of each arriving frame.
- [`probe`](src/endpoint.rs): dials a peer once with a chosen key exchange (`KeyExchange`) and exchanges hellos; an operator checks reachability with it, and tests check that nodes refuse classical key exchange.
- [`MessageClass`](src/class.rs) and [`ClassLimits`](src/class.rs): the four classes and their size limits.
- [`FrameHeader`](src/frame.rs): the class and length that open every frame.
- [`NodeId`](src/id.rs): a node's identity on the network.
- [`NetworkKey`](src/key.rs) and [`NetworkPublicKey`](src/key.rs): the ML-DSA-44 key a node presents.
- [`NetworkRecord`](src/record.rs), [`SignedRecord`](src/record.rs) and [`Peer`](src/record.rs): what a validator publishes and what an endpoint pins.
- [`Quotas`](src/quota.rs): flood limits per peer and class.

## How it fits

Depends on: `sion-core` and `sion-crypto`, and on `quinn`, `rustls` and
`aws-lc-rs` for QUIC and TLS (aws-lc-rs is C code, atom `net:aws-lc`).
Used by: `sion-node` (its QUIC transport) and `sion-cli`.

The crate knows nothing about the protocol: it moves bytes in classes and
does not decode consensus messages, which is `sion-node`'s job. Status
limits that apply here: no TCP fallback, peers do not follow committee
changes, and the code has not been tested across machines, under `netem`,
or by fuzzing the decoders. Raw public keys (RFC 7250) stand in for X.509
certificates (`net:raw-public-keys`); keys are pinned from files
(`net:static-peers`).

The crate is 2,958 lines of Rust under `src/` at the time of writing.

## Tests

26 unit tests (counted with `grep -c "#\[test\]"`), all inside `src/`; there
is no `tests/` directory. `src/endpoint/tests.rs` (9 tests) runs real
endpoints on one machine: pinned validators connect and carry every class in
order, an unknown key is refused, a node presenting another key is refused,
a peer offering only classical key exchange cannot connect, a peer on another
chain is refused, frames queued for a down peer arrive when it returns, and
oversized frames and floods are refused or dropped. `src/frame.rs` has
property tests (proptest). The other modules test their own encodings and
limits. No test is ignored.

```text
cargo test -p sion-net
```

## Read more

- [Crates](../../docs/book/src/architecture/crates.md)
- [Networking plan](../../docs/book/src/research/networking.md)
- [Running a network](../../docs/book/src/guides/network.md)
- [Post-quantum signatures](../../docs/book/src/concepts/post-quantum.md)
- [Implementation status: node and operations](../../docs/implementation-status.md)
