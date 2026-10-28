# Networking between machines

*Research note, 28 October 2026. This is a design and a plan. Nothing in it
is built.*

Today every Siontology validator is a thread of one process, and validators
exchange real signed messages over in-memory channels. The implementation
status calls this "Networking between machines: Simulated (in-process
channels)" ([`docs/implementation-status.md`](https://github.com/ColdAI-org/siontology/blob/main/docs/implementation-status.md)).
This note maps every message that crosses between validators, or between
the gateway and validators; recommends a transport and a topology; works out
what has to change once messages can be lost or delayed; and lays out a
phased plan that keeps the in-process devnet working at every step.

When its phases land, three trust atoms go: `impl:in-process-network`,
`impl:reliable-handover` and `impl:lock-handover`. The plan adds new ones,
listed under [Trust atoms](#trust-atoms-this-plan-adds-and-retires).

**How numbers are labelled.**

| Label | Meaning |
|---|---|
| Measured | From a run; the run is named |
| Computed | From the Borsh encoding in the code and the standard key and signature sizes; exact up to a few bytes |
| Estimate | A model; its inputs are stated next to it |
| Target | A goal from the architecture paper or proposed here; nothing meets it yet |

KB is 1,000 bytes, KiB 1,024 bytes, MB 10⁶ bytes, TB 10¹² bytes. *n* is the
size of the consensus committee and *q* = ⌊2n/3⌋ + 1 its quorum with equal
stake (*q* = 3 at *n* = 4). *s* is the number of seats in a lane committee,
*q*ₛ its quorum, and *δ* a one-way network delay. Code references are to
`crates/` in this repository at commit `dafc6de`.

## Contents

- [Summary](#summary)
- [1. How the devnet's validators talk today](#1-how-the-devnets-validators-talk-today)
- [2. Message inventory](#2-message-inventory)
- [3. Transport and topology](#3-transport-and-topology)
- [4. Committee changes over a lossy network](#4-committee-changes-over-a-lossy-network)
- [5. Data availability over the network](#5-data-availability-over-the-network)
- [6. Implementation plan](#6-implementation-plan)
- [7. Risks and open questions](#7-risks-and-open-questions)
- [Sources](#sources)

## Summary

- **Transport.** Plain QUIC between validators: `quinn` 0.11 with `rustls`
  0.23 and its `aws-lc-rs` provider. Validators form a full mesh over the
  consensus committee, which the devnet caps at 100 validators
  (`max_active`). Membership comes from the validator set in the state.
  Consensus traffic uses direct streams, not a gossip layer. Sui's and
  Aptos's validator networks have the same shape: direct, authenticated,
  all-to-all connections among a committee known from the chain. Sui uses
  gRPC over TCP, and Aptos uses Noise over TCP. rust-libp2p brings a
  classical-only identity and a 64 KiB default gossip message limit, and
  its peer discovery is not needed among validators that the state already
  lists. It stays an option for a later public layer of full nodes and
  light clients.
- **Authentication.** TLS 1.3 with the hybrid key exchange
  X25519MLKEM768 (RFC 10024, August 2026) and self-signed ML-DSA-44
  certificates (RFC 9881). Each validator has a separate ML-DSA-44 network
  key. A record signed with its dual key (ML-DSA-44 and
  SLH-DSA-SHA2-128f) binds that network key to its validator identity.
  Authentication at the transport layer then rests on ML-DSA-44 alone. That
  is a new atom. Consensus messages keep their dual signatures.
- **Cost.** The 19.5 KB dual signature dominates the wire. With the
  certified DAG as built, an idle validator sends about
  (*n* − 1)(*q* + 3) × 19.5 KB per round. That comes to 0.35 MB at 4
  validators, 1.8 MB at 10 and 135 MB at 100 (computed). Signing and
  checking take about 0.27 s of one core per round at 10 validators and
  10 s at 100 (an estimate from measured single-signature costs). A
  networked testnet can run the certified DAG with up to about 10 to 20
  validators. Beyond that it needs the uncertified DAG (Mysticeti class)
  that the architecture already targets, and payloads carried by digest.
- **What breaks first on a real network.** Two shortcuts break. First, the
  epoch hand-over: a validator that misses messages of the old session
  cannot fetch them once its peers have moved on. Second, restarts: the
  devnet starts a new consensus session at every restart, so that no
  validator ever signs a round twice. One machine that restarts alone
  cannot do that. The designs below fix both: a drain window, Pulse sync
  from certified headers, and a persisted signing log.
- **Lane locks across committee changes.** These need a protocol change.
  Incoming members read the locks from a quorum of the outgoing committee
  before they approve a version that existed before the change. Old
  certificates stay executable for a bounded window, which needs a cap on
  how far ahead a transaction may expire.
- **Data availability.** Very little dissemination is needed, because
  every validator derives the whole extended square from the Pulse it
  executed. The network work is serving. Full nodes subscribe to Pulse
  data, encode it, check the root and serve shares. Light clients sample
  shares with proofs to the certified data root, from several nodes.
- **Plan.** Five phases, each keeping `sion devnet` working through a
  `Transport` trait with in-memory, fault-injecting and QUIC
  implementations. The first three steps:
  1. the transport seam, with a memory transport;
  2. loss tolerance in the protocol (session-tagged messages and retries);
  3. a seeded fault-injecting transport and its tests.

  A testnet of seven validators in four AWS regions costs about 7,000 to
  8,000 USD a month on demand, including a few hours of load tests a week
  (estimate, [section 6](#costs)).

## 1. How the devnet's validators talk today

`Devnet::start` (`sion-node/src/devnet.rs`) creates one `std::sync::mpsc`
channel per validator thread and one for the gateway thread. It hands every
thread a clone of `Bus` (`sion-node/src/validator.rs`), which holds three
things: the senders in index order, an index from `ValidatorId` to
position, and the gateway's sender. `Bus::send` and `Bus::broadcast` push
Rust values into channels, and nothing is encoded. The consensus core
(`sion_consensus::Core`) is a pure state machine. `Validator::apply` turns
its `Action::Broadcast` and `Action::Send` into channel sends, and its
`Action::SetTimer` into entries in a local timer heap.

The in-process bus gives several things for free. A network has to supply
each of them some other way:

| The bus today | What a network has to provide |
|---|---|
| Every message arrives, exactly once, unless the receiver has stopped | Loss, duplication and reordering. A reconnection loses whatever was in flight |
| The receiver trusts the `from` field the sending thread wrote (`Input::Consensus`, `Input::PulseSignature`, `GatewayInput::LaneRejected`) | The receiver takes the sender's identity from the authenticated connection, never from the message |
| Values move by pointer: no encoding, no size limit | One canonical encoding, and a size limit per message class checked before decoding |
| A broadcast costs nothing | Every byte crosses a link, and signatures of 19.5 KB dominate |
| The gateway reads committees and keys from `Shared::view`, a lock the validators write | A remote gateway learns committees from the chain |
| The faucet and status calls ask validator 0 through a reply channel | Remote callers use the HTTP API |
| All validators restart together and agree on a new restart generation (`Shared::generation`), which starts a new consensus session | Machines restart one at a time |

These devnet parameters set the message rates:

| Parameter | Devnet value | Where |
|---|---|---|
| Validators, standby validators | 4, 0 | `DevnetConfig` |
| Lane shards; seats per lane committee | 2; 4 | `DevnetConfig` |
| Consensus rounds per Pulse | 4 | `DevnetConfig::rounds_per_pulse` |
| Lane batch interval | 25 ms | `DevnetConfig::batch_interval_ms` |
| Gateway flush interval; largest group | 20 ms; 512 transactions | `gateway.rs` |
| Gateway re-sends a certified, unexecuted transaction after | 4 s | `gateway.rs` |
| Leader timeout; resend interval | 400 ms; 600 ms | `CoreConfig` in `config.rs` |
| Header payload limit | 512 KiB and 2,000 items | `CoreConfig` |
| Rounds ahead accepted | 64 | `CoreConfig` |
| Messages kept for a future session | 20,000 | `MAX_FUTURE_MESSAGES` |
| Early Pulse signatures kept | 8 Pulses ahead, one per sender | `EARLY_SIGNATURE_WINDOW` |
| Pulses of history; of published data; extended squares cached | 256; 64; 4 | `config.rs`, `validator.rs` |

One measured run gives the rates. It ran four validators with dual
signatures on a shared 2-core virtual machine, with a load average of about
6, in a release build of an earlier version
([benchmarks](../guides/benchmarks.md)). The devnet closed one
Pulse every 3.9 s, which is about one consensus round a second. It moved
15 transfers a second, with a median of 7.1 s from submission to
execution. One machine's CPU set those rates, not a network.

## 2. Message inventory

### Sizes that drive everything

- **A dual signature** is 19,518 bytes encoded, about 19.5 KB. It
  consists of ML-DSA-44 at 2,420 bytes and SLH-DSA-SHA2-128f at 17,088
  bytes, the sizes fixed by FIPS 204 and FIPS 205 and asserted in
  `sion-crypto/src/scheme.rs` and `dual.rs`.
- **A signed one-input transfer** is about 19.7 KB (computed): a body of
  about 194 bytes plus the sender's 19.5 KB authorization. An account's
  first transaction also carries its genesis policy, about 1.4 KB more.
- **Making one dual signature costs 17.1 ms, and checking one costs
  1.26 ms** (measured, same run). The parts are ML-DSA-44 at 0.69 ms to
  sign and 0.20 ms to verify, and SLH-DSA-SHA2-128f at 16.4 ms to sign and
  1.06 ms to verify.

### Between validators, and between the gateway and validators

Frequencies are at the devnet defaults. The last column also says what
repairs a lost message today, if anything does.

| Message (code) | From → to | Purpose | Size (computed) | Frequency | Ordering and reliability needed |
|---|---|---|---|---|---|
| `Message::Header` (`SignedHeader`) | Proposer → every other validator, observers included | Propose a round's payload | 90 B + 32 B per parent + payload + 19.5 KB. The payload is at most 512 KiB and 2,000 items | 1 per validator per round. Re-broadcast every 600 ms while uncertified | No ordering. Duplicates are harmless (deduplicated by digest). The resend timer repairs loss. Latency-critical |
| `Message::Vote` | Voter → the header's author | Vote for a header | 104 B + 19.5 KB ≈ 19.6 KB | *n* − 1 per validator per round | No ordering. When the author re-broadcasts its header, the voter re-sends the same vote, which repairs loss. On the critical path of every round |
| `Message::Certificate` | Author → every other validator; also the reply to a sync request | A header with a quorum of votes | Header with its payload + (*q* + 1) × 19.5 KB: 78 KB + payload at *n* = 4, 1.3 MB + payload at *n* = 100 | 1 per validator per round. The latest one is re-broadcast every 600 ms | No ordering: it waits until its parents arrive. Resend and sync repair loss. Bulk, but on the critical path of the next round |
| `Message::SyncRequest` | Validator → the peer that sent an orphan, or to all on resend | Fetch missing parents | ≤ 64 digests, ≤ 2.1 KB | On missing parents; every 600 ms while any are missing | Request and response, idempotent. It carries no session, so after an epoch boundary it reaches the wrong core (section 4) |
| `Input::Submit` with `PayloadItem::Certified` | Gateway → *f* + 1 consensus members, rotating | Hand lane-certified transactions to consensus | 19.6 KB per distinct batch header + per transaction (19.7 KB + *q*ₛ × (16 + 32·⌈log₂ batch⌉) B). One transfer alone with 3 batch headers: 78.7 KB | Every 20 ms when certificates are waiting, up to 512 transactions. Re-sent after 4 s if not executed, and at every new session | At least once to an honest proposer (*f* + 1 copies). Deduplicated by item hash. Must fit in 512 KiB, or `Core::submit` drops it ([risks](#risks)) |
| `Input::Submit` with `PayloadItem::Forced` | API → every validator (`Devnet::force`) | Post a forced intent | 19.7 KB | Per forced intent, rare | At least once to *f* + 1 validators, so that one honest proposer has it. The client retries |
| Beacon reveal (`PayloadItem::Reveal`) | Inside the proposer's own header | Reveal for the next Pulse | 73 B | 1 per validator per Pulse | Travels with the header. Not a message of its own |
| `Input::Lane` (`LaneRequest`) | Gateway → each member of the sender's lane committee | Lock owned inputs | 42 B + 40 B per input + the signed transaction (19.7 KB for a one-input transfer, at most 256 KiB) | *s* per transaction (*s* = 4), sent once | Request and response. Idempotent at the validator: approving the same transaction again returns the same proof. **No retry today:** a lost request or approval leaves the transaction `certifying`, and resubmitting it while pending changes nothing |
| `GatewayInput::Approval` (`ApprovalProof`) | Lane member → the gateway | One approval | 19.7 KB, because the full batch signature travels with every approval | *s* per transaction, sent when the batch seals (every 25 ms) | Response, idempotent. On a network the signed batch header should travel once per batch, with each approval as a reference (16 + 32·⌈log₂ batch⌉ B) |
| `GatewayInput::LaneRejected` | Lane member → the gateway | A refusal and its reason | About 0.1 to 0.2 KB | Per refusal | Best effort. The gateway rejects once refusals make a quorum impossible |
| `Input::ForcedApproval` | Lane member → every validator | Approval of a forced intent, so a certificate forms without the gateway | 19.7 KB | Per forced intent per lane member; again after a rotation | At least once to every validator. **No retry today** |
| `Input::PulseSignature` | Every validator → every other validator | Signature on the executed Pulse header | 72 B + 19.5 KB | 1 per validator per Pulse (every 4 rounds; 3.9 s measured) | At least once, idempotent. Signatures up to 8 Pulses early are kept, one per sender. **No retry today:** if more than *f* signatures to a validator are lost, that validator never certifies the Pulse |
| `GatewayInput::Executed` | Validator 0 → the gateway | Which transactions executed | 12 B + 32 B per transaction | 1 per Pulse | Best effort. The gateway re-sends certificates after 4 s anyway |

Market key shares for Verifiable Clearing travel inside ordinary
transactions (`TxKind` carries `shares`), so they add no message type.

### Between clients, light clients and nodes

Clients reach a node through the HTTP API (`sion-node/src/api.rs`: JSON
over HTTP/1.1, plaintext, four worker threads). Binary values travel as
hexadecimal inside JSON, which doubles them.

| Route | Purpose | Size (computed) |
|---|---|---|
| `POST /v1/transactions`, `POST /v1/forced` | Submit a transaction or a forced intent | 2 × 19.7 KB as hex; at most 256 KiB before hex |
| `GET /v1/pulses/{n}` | Header and certificate | 0.4 KB header + *q* × 19.6 KB: 59 KB at *n* = 4, 98 KB at *n* = 7, 1.3 MB at *n* = 100 |
| `GET /v1/pulses/{n}/data` | Data-availability header (axis roots) | 44 B + 128 B × *k*: 2.1 KB at *k* = 16, 66 KB at *k* = 512, 262 KB at *k* = 2,048 |
| `GET /v1/pulses/{n}/data/{row}/{column}` | One share with a proof to its row root | 12 B + 32 B × log₂(2*k*): 172 B at *k* = 16, 332 B at *k* = 512 |
| `GET /v1/epochs/{e}` | Certificate of the epoch's boundary and a proof of the validator set | One certificate + about 1.5 KB per registered validator + a proof path |
| `GET /v1/pulses/{n}/world` | World Proof | Grows with witnesses: 1.7 MB of witnesses for 512 transfers (measured, [benchmarks](../guides/benchmarks.md)) |

### Messages that stay inside one process

These carry reply channels or control a local thread. They do not become
network messages: `Input::Query` and its reply, `Input::Adversary`,
`Input::Shutdown`, `GatewayInput::Submit`, `Faucet`, `Status`, `Censor` and
`Shutdown`, and the prover's bounded job channel. The HTTP API stays the
way remote callers reach them.

### Messages a network adds

| Message | Purpose | Phase |
|---|---|---|
| Hello | Exchange chain identity, protocol version and network records after the handshake | 2 |
| Session envelope | Tag every consensus message, votes and sync requests included, with its session | 1 |
| Fetch by round range | Certificates of rounds *a* to *b* of a session, for a peer that fell behind by more than a few rounds (like Sui's `fetch_blocks` and `subscribe_blocks`) | 1 |
| Batch announcement | A lane member's signed batch header, sent once per batch to each gateway | 1 |
| Pulse signature pull | "Send me your signature for Pulse *p*" | 1 |
| Pulse sync | Certified headers with their `PulseRecord`s for a range of Pulses | 1 |
| Snapshot chunks | A snapshot of the state at a certified Pulse | 2 |
| Lock read | "Which owner-signed transaction, if any, holds this object version?", sent to the outgoing lane committee (section 4) | 1 |
| Pulse data stream | Published data of each executed Pulse, for full nodes that serve shares | 3 |
| Sample batch | Several shares with proofs to the data root in one request | 3 |

### What a transfer costs on the wire

For one one-input transfer in the protocol as built, the bytes on the wire
are as follows (computed). *c* = *f* + 1 is the number of copies the
gateway sends to consensus, and every proposer that receives the group puts
it in its header and then in its certificate:

```text
bytes ≈ 19.8 KB × (s lane requests + s approvals + c submissions + 2·c·(n − 1) header and certificate copies)
```

| Case | Per transfer (computed) |
|---|---|
| *n* = 4, *s* = 4, *c* = 2, transfers sharing batches and a group | 0.44 MB |
| *n* = 4, a transfer travelling alone (3 batch headers in its group) | 1.9 MB |
| *n* = 7, *s* = 4, *c* = 3, sharing | 0.93 MB |
| *n* = 100, *s* = 100, *c* = 34, sharing | 138 MB |
| Floor: one copy of the signed transaction per validator, *n* = 4 | 0.08 MB |

All figures are before the periodic re-broadcasts, which add more. Three
changes bring *n* = 7 from 0.93 MB to about 0.22 to 0.26 MB per transfer
(estimate: four lane requests, batch references, one to three
submissions, and one copy to each of the six other validators). First,
carry payloads by digest and disseminate each group once. Second, compact
certificates whose header the peer already has. Third, announce batch
signatures once. Section 6 schedules these changes.

### Consensus traffic and CPU as the committee grows

Each round, a validator of the certified DAG sends its header to *n* − 1
peers, one vote to each of *n* − 1 authors, and its certificate (*q* + 1
signatures) to *n* − 1 peers. It signs one header and *n* − 1 votes. It
checks *n* − 1 headers, the votes for its own header and *q* votes in each
of *n* − 1 certificates; its signature cache skips only repeats. An
uncertified DAG (Mysticeti class) has no votes and no certificates: each
validator signs one block per round and checks *n* − 1.

```text
certified, egress per round ≈ (n − 1)(q + 3) × 19.5 KB
certified, CPU per round    ≈ n × 17.1 ms + (n − 1)(q + 2) × 1.26 ms
uncertified, egress         ≈ (n − 1)(19.5 KB + 32 B × q)
uncertified, CPU            ≈ 17.1 ms + (n − 1) × 1.26 ms
```

| *n* | *q* | Certified: egress per round | Certified: CPU per round | Uncertified: egress per round | Uncertified: CPU per round |
|---|---|---|---|---|---|
| 4 | 3 | 0.35 MB | 0.09 s | 0.06 MB | 0.02 s |
| 7 | 5 | 0.94 MB | 0.17 s | 0.12 MB | 0.02 s |
| 10 | 7 | 1.8 MB | 0.27 s | 0.18 MB | 0.03 s |
| 20 | 14 | 6.3 MB | 0.73 s | 0.38 MB | 0.04 s |
| 50 | 34 | 35 MB | 3.1 s | 1.0 MB | 0.08 s |
| 100 | 67 | 135 MB | 10.3 s | 2.1 MB | 0.14 s |

These are estimates for an idle committee with empty payloads. The
signature costs come from the measured 2-core machine, and all signing
runs on the validator's single thread today. The table counts per round,
and uncertified rounds take about one message delay where certified ones
take three. Per second, then, an uncertified DAG runs about three rounds
for each certified one. Even so it sends about 20 times fewer bytes at
*n* = 100, and about 2.6 times fewer at *n* = 7. Two consequences follow:

- The certified DAG is affordable on 1 Gbit/s links and ordinary servers
  up to about 10 to 20 validators. At 20 validators, one round a second
  is 6.3 MB/s of egress and 0.73 s of one core.
- To scale past that, the uncertified DAG and parallel signature checking
  are prerequisites. The architecture already plans the uncertified DAG
  ([Contention Consensus](../concepts/contention-consensus.md)).

Idle traffic matters because the core never paces itself. A validator
proposes the next round as soon as a quorum of certificates and the
Bullshark wait allow (`Core::try_advance`). It also re-broadcasts its latest
certificate every 600 ms whether or not its peers have it (`Core::resend`).
At *n* = 7 that re-broadcast adds about 6 × 6 × 19.5 KB / 0.6 s ≈ 1.2 MB/s
per validator (computed). The plan adds a minimum round interval (250 ms
proposed) and replaces the periodic re-broadcast with acknowledged
delivery.

## 3. Transport and topology

### The options

| Question | rust-libp2p | QUIC (`quinn`) and a membership layer of our own | What production BFT validator networks do |
|---|---|---|---|
| Fit for a known committee talking all to all | Gossipsub meshes peers at degree 6 (default `mesh_n`, between 5 and 12) and adds hops and duplicate copies. Direct streams are possible (request-response), but then most of the stack goes unused | Direct connections to every committee member. Membership is the validator set in the state | Sui: each validator streams its blocks to the others over gRPC (`subscribe_blocks`, `fetch_blocks`, `fetch_commits`). Aptos: "each validator connects directly to all other validators in order to maintain a full-mesh network", which "should scale up to a few hundred validators" |
| Message sizes | Gossipsub's default `max_transmit_size` is 65,536 bytes. A header can be 512 KiB and a certificate 1.3 MB | QUIC streams carry any length, with flow control per stream and per connection | Sui caps a fetch response at 4 MiB and sets HTTP/2 windows of 64 MiB per connection and 32 MiB per stream |
| Peer identity | The TLS spec allows RSA, Ed25519, Secp256k1 and ECDSA keys. Noise uses X25519. Neither has a post-quantum option | Any rustls verifier: pinned ML-DSA-44 certificates | Sui's consensus: mutual TLS 1.3 through its `sui-tls` crate, with self-signed Ed25519 certificates checked against an allowlist of the committee's network keys; the server maps the key in a client's certificate to an authority. Aptos: Noise IK with x25519 keys from on-chain configuration. Both are classical |
| Post-quantum key exchange | Not in the Noise or TLS specs | rustls offers X25519MLKEM768 with the aws-lc-rs provider | Neither Sui's nor Aptos's code cited here uses it |
| Discovery and NAT traversal | Kademlia, AutoNAT, DCUtR and relays | None. Validators publish their addresses | Addresses from the chain: Aptos's `ValidatorConfig`, Sui's committee |
| Denial of service | Gossipsub scoring (P₁ to P₇, IP colocation, backoff) | Ours to build: quotas per peer and class, admission by key | Solana gives 80 % of a leader's transaction capacity to staked peers in proportion to stake. Sui-derived nodes block IPs that exceed request thresholds |
| Maturity in 2026 | 0.57.1 released 15 October 2026 (docs.rs failed to build it). Remote-panic advisories this year: gossipsub PRUNE backoff overflow (CVE-2026-33040, fixed in 0.49.3) and a QUIC certificate-expiry race (CVE-2026-61544, fixed in libp2p-quic 0.13.1) | `quinn` 0.11.12, released 14 September 2026, minimum Rust 1.80 (the workspace's). `rustls` 0.23 (0.23.45 on docs.rs) | Sui runs consensus over TCP port 8081 and state sync over UDP port 8084. Solana spreads blocks over UDP (Turbine, fanout 200, 32 data and 32 coding shreds per set). Alpenglow's Rotor replaces Turbine's tree with a single layer of relays |

Narwhal and Aptos's Quorum Store both separate data dissemination from
ordering. Validators broadcast batches to each other, and consensus orders
digests and signed proofs that the data is stored. Narwhal reports over
130,000 transactions a second at under 2 s of WAN latency with HotStuff
ordering, and linear growth to 600,000 with more workers (its abstract).
Aptos's Baby Raptr (June 2025) then folded batch availability into the
consensus vote and went from six network hops to four. The common lesson:
consensus should not carry large payloads inline.

### Recommendation

1. **QUIC with `quinn`**, one connection per pair of committee members. The
   validator with the smaller identifier dials, and both sides retry with
   exponential backoff from 0.1 s to 10 s (proposed). A TCP fallback using
   the same framing over TLS 1.3 serves networks that block UDP.
2. **A full mesh over the consensus committee.** That is at most 100
   validators today. Connections also go to the next epoch's members once
   the state names them, and stay open to the previous epoch's members for
   the drain window. Peers come from the validator set in the state plus a
   network record per validator (below), not from discovery.
3. **No gossip for consensus.** Direct streams carry consensus traffic. If
   measurements show the proposer's egress binding, add erasure-coded relay
   of large payloads through one layer of relays, as Alpenglow's Rotor
   does.
4. **A separate public tier.** Full nodes are validators running as
   observers, the mode standby validators already use. They serve the HTTP
   API, Pulse data, proofs and shares to wallets, agents and light clients,
   so validators expose only the mesh port and the ingress port. libp2p
   becomes worth its weight only if light clients are to serve shares to
   each other from behind NAT, which the architecture asks for
   ("Capacity grows with the crowd") and which is not built.

### Topology

```text
 wallets, agents, light clients
            │  HTTPS: TLS 1.3, X25519MLKEM768
            ▼
 full nodes (observers) ◄──── observer stream: certificates, Pulse data ────┐
                                                                            │
 gateways (anyone) ──── lane requests, certified groups ────► validators ───┘
            ▲                                                 full QUIC mesh:
            └──────────── approvals, batch announcements ──── consensus, Pulse
                                                              signatures, forced
                                                              approvals, sync
```

| Port (proposed) | Protocol | Who may connect |
|---|---|---|
| Mesh | QUIC over UDP, mutual TLS | Current committee, next committee, previous committee during the drain |
| Ingress | QUIC over UDP, mutual TLS | Gateways and observers with a network key; quotas per key and per IP |
| API | HTTPS over TCP | Anyone. Served by full nodes; on validators it listens on localhost only |

### Peer authentication with post-quantum keys

- **Network key.** Each validator gets an ML-DSA-44 key pair for the
  transport, separate from its dual consensus keys.
- **Network record.** A record binds the network key to the validator:
  `NetworkRecord { chain, validator, network_key, addresses,
  valid_from_epoch, sequence }`. The validator signs it with its dual key
  under a new domain, `sion/net/record`. The record weighs about 1.4 KB
  plus the 19.5 KB signature, and checking it costs 1.26 ms once per
  record. In phases 2 and 3 the records sit in a testnet peer file. Each
  record authenticates itself, so whoever hosts the file cannot forge
  one. In phase 4 they move into the state through a new transaction kind,
  as Aptos keeps addresses in `ValidatorConfig`.
- **Handshake.** TLS 1.3 only, with ALPN `sion/1`:
  - key exchange X25519MLKEM768 only;
  - signature scheme `mldsa44` (0x0904, draft-ietf-tls-mldsa) only;
  - a self-signed X.509 certificate for the network key (RFC 9881);
  - mutual authentication on the mesh and ingress ports.
- **Verification.** A custom rustls verifier, like Sui's `AllowPublicKeys`
  but with post-quantum keys:
  1. it takes the ML-DSA-44 public key from the peer's certificate;
  2. it requires that key to equal the network key in the peer's current
     record;
  3. it lets `rustls-webpki` check the self-signature.

  The transport hands the protocol the validator identity that the pinned
  key belongs to, as Sui's consensus server maps a client's certificate key
  to an authority. The TLS server name carries the chain identity and the
  epoch; Sui's consensus derives its server name from the epoch too
  (`consensus_epoch_{epoch}`). Certificates are valid for the record's
  lifetime, not minutes, which avoids the class of bug behind
  CVE-2026-61544.
- **Cost.** A certificate is about 3.9 KB: a 1,312-byte key, a 2,420-byte
  signature and the encoding (estimate). The server's first flight is
  about 7.6 KB: a 1,120-byte key share, the certificate, a 2,420-byte
  CertificateVerify and Finished (estimate). The client's 1,216-byte key
  share makes its ClientHello span two Initial datagrams, about 2,400
  bytes. A server may send three times what it received before it
  validates the client's address (RFC 9000 §8.1), about 7,200 bytes, so a
  handshake may take one extra round trip. That does not matter for
  connections that stay open for days.
- **New atom.** Transport authentication rests on ML-DSA-44 alone
  (`net:transport-ml-dsa`). Whoever can forge ML-DSA-44 can impersonate a
  peer at the transport layer: delay, drop, eclipse. That attacker still
  cannot sign consensus messages, which need both signatures.

Reusing the validator's ML-DSA-44 consensus key in TLS was rejected.
`sion-crypto` passes its domain as the FIPS 204 context string and TLS
signs with an empty context, so the signatures could not be confused. But
the key would then sit in a network-facing TLS stack, and stealing it would
yield half of the validator's dual signature. A separate network key keeps
such a compromise inside the transport. Putting both signatures in every
handshake was rejected too. It would mean a 17 KB SLH-DSA signature and
16.4 ms of signing per handshake, and the TLS signature schemes this note
found define ML-DSA alone (draft-ietf-tls-mldsa), not a pair with
SLH-DSA.

### Transport encryption

- **X25519MLKEM768** is a Proposed Standard (RFC 10024, August 2026). Its
  client key share is 1,216 bytes and its server share 1,120 bytes. In
  rustls it is available with the aws-lc-rs provider and becomes the first
  choice with the `prefer-post-quantum` feature. The rustls manual states
  that ML-KEM-768 alone "is not currently enabled by default out of
  conservatism". Validators configure both ends, so they offer only the
  hybrid.
- **ML-DSA certificates** are enabled by default in the aws-lc-rs provider
  since rustls 0.23.44 (7 September 2026). The rustls notes add that they
  are "not supported in the public web PKI, but they can be used with
  private certificate hierarchies", which is what this design uses.
  `rustls-webpki` 0.103.16 provides `ML_DSA_44`, `ML_DSA_65` and
  `ML_DSA_87`.
- **The aws-lc-rs provider builds C code.** The rest of the workspace's
  post-quantum code is pure Rust (`fips203`, `fips204`, `fips205`). The
  alternative is a rustls `CryptoProvider` that wraps the workspace's own
  ML-KEM and ML-DSA crates. That keeps the stack pure Rust but adds
  unaudited glue on top of unaudited crates (`impl:unaudited-pq-crates`).
  The plan starts with aws-lc-rs and records the choice as an atom
  (`net:aws-lc`). The new network crate keeps `unsafe_code = "forbid"`;
  its dependencies (tokio, quinn, rustls, aws-lc-rs) contain unsafe code,
  as current dependencies such as `zeroize` already do.
- **Client HTTPS.** The API needs HTTPS with the same hybrid key exchange,
  and `ureq` in the CLI needs its TLS feature, which is off today.
  Certificates for public endpoints come from the web PKI, which has no
  ML-DSA certificates yet. Light clients check everything a node serves
  against certified headers anyway, so for them TLS protects privacy and
  resists censorship; it does not decide validity.

### Framing, streams and flow control

- **Frames.** A one-byte class, a four-byte length and a Borsh body. The
  receiver checks the length against the class's limit before reading the
  body, and it stamps the sender's identity from the connection.

  | Class | Messages | Frame limit (proposed) | Stream |
  |---|---|---|---|
  | Control (priority 3) | Votes, Pulse signatures, sync requests, lock reads, batch announcements, approvals, rejections | 64 KiB | One long-lived ordered stream per peer |
  | Consensus (priority 2) | Headers, certificates | `max_payload_bytes` + (*n* + 2) × 20 KB | One unidirectional stream per message, so a 1.3 MB certificate never delays a vote |
  | Ingress (priority 1) | Lane requests, payload submissions | 300 KiB | One stream per request, with the response on the same stream |
  | Bulk (priority 0) | Pulse sync, snapshots, fetches by round | 8 MiB per chunk | One stream per request |

  `quinn`'s `SendStream::set_priority` sends higher-priority buffered data
  first. Its documentation warns that many distinct levels can cost
  performance, so the plan uses four.
- **Windows.** `quinn`'s defaults are "tuned for a 100Mbps link with a 100ms
  round trip time". The mesh runs 1 Gbit/s links with round trips up to
  about 237 ms (Frankfurt to Tokyo, measured below), a bandwidth-delay
  product of about 30 MB. So the plan raises the receive windows to about
  32 MB per connection, and a send window bounds memory per peer.
  Keep-alive runs every 5 s (proposed) against `quinn`'s 30 s idle
  timeout.
- **Queues.** Outbound queues are bounded in bytes per peer and per class.
  When one is full, the class's policy decides: drop the oldest re-sendable
  message (headers, certificates) or apply backpressure to the caller
  (sync, ingress).
- **Compression.** Not at first. Signatures look random and dominate the
  bytes. Sui compresses its consensus traffic with Zstd, which is worth
  measuring once payloads travel by digest.

### Denial-of-service protection

Validator mesh:

- **Admission.** Admission happens at the handshake. Only keys in the
  current, next or draining committee's records may connect to the mesh
  port. Anything else fails in TLS before the node decodes a byte.
- **Quotas per peer and class.** Token buckets sized from the protocol's
  own bounds:
  - one header per author and round, within 64 rounds ahead;
  - at most *n* votes and *n* certificates per round, plus sync;
  - sync requests of at most 64 digests, one per resend interval;
  - one Pulse signature per Pulse.

  The node drops and counts traffic beyond a bucket, and disconnects a
  peer that keeps exceeding it for a period. Equivocation remains
  evidence, never a reason to stop listening.
- **Stake-weighted bandwidth under congestion.** Inbound capacity is shared
  in proportion to stake, as Solana's stake-weighted quality of service
  does for transactions, with a floor for every member. Every member's
  headers must be heard for the protocol to stay live.
- **Cheap checks before signatures.** Order the checks as: frame length,
  decode bounds, committee membership, round and session window, already
  seen by digest, and only then the signature (1.26 ms). The node already
  orders lane checks this way: `StateOwners::check` runs the cheap checks
  first and the signature last.
- **Fix the unbounded buffers before going live:**
  - `MAX_FUTURE_MESSAGES` counts messages, not bytes, and keeps them
    unverified. That allows about 20,000 × 0.6 MB ≈ 12 GB at *n* = 4 from a
    single committee member (computed upper bound). Bound it per sender in
    bytes, and accept only the next session.
  - `early_signatures` keeps the first signature per claimed `from`. With
    a spoofable `from`, a peer could take another validator's slot and get
    its real signature dropped. Stamping `from` in the transport closes
    this.
  - The gateway's `pending` map and the validator's `parked` list have no
    count limit.
  - `Core::submit` keeps any item up to 512 KiB in the mempool; bound the
    mempool per submitter.

Gateways and clients:

- **A separate port with its own quotas.** Admission works per network key
  and per IP. Registered gateways get a larger share (a record in the
  state, perhaps bonded). Anonymous gateways and clients share the rest.
  Sui-derived nodes do something similar at their API: per-client request
  thresholds over a burst window, block lists with a time to live, and a
  dry-run mode to tune them. IOTA's production recommendation is a
  threshold of 50 a second per client, a 30 s burst window and a 300 s
  block.
- **Limits per sender.** Cap the lane requests a validator holds per
  sending account, and the forced intents it proposes per account and
  Pulse.
- **Light-client sampling** goes to full nodes, never to validators'
  consensus machines. Its responses can be cached and rate-limited per IP.

### Membership and discovery

- **The state names the peers.** After every Pulse a node derives the
  consensus committee and the lane committees from its state, as it does
  today (`Committees::derive`). It then calls `Transport::set_peers` with
  three sets:
  - the current committee;
  - members of the next active set, once their status says they join at
    the boundary;
  - the previous committee until its drain window closes.
- **A health check.** Each node watches its own peers: QUIC keep-alive,
  plus the round each peer last proposed. It shares no health data, the
  model Aptos describes.
- **Observers** (standby validators and full nodes) subscribe to
  certificates and Pulse data on the ingress port. Today `Bus::broadcast`
  reaches every validator thread, standby ones included. On a network,
  observers pull or subscribe, so committee members never wait on them.

## 4. Committee changes over a lossy network

### Epoch hand-over: the drain period (`impl:reliable-handover`)

**Today.** At the epoch's last Pulse each validator replaces its core with
a new one for the next session (`Validator::reconfigure`). It keeps only
the old mempool and the old builder's leftovers, which it proposes again.
After that the validator handles old-session messages in three ways:

- headers and certificates of older sessions are dropped (`session <
  self.committees.session`);
- messages of future sessions are buffered;
- votes and sync requests carry no session at all (`message_session`
  returns `None`), so they go to the new core.

A validator that has not yet executed the last Pulse still needs the old
session's certificates up to the anchor that closes it. On a network that
lost some of them, it asks its peers. Its peers have dropped the old core,
and their new core does not know those digests, so nobody answers. If more
than *f* validators get stuck this way, the new session lacks a quorum too,
and the chain stops. The in-process bus never loses a message, which is the
only reason this works today.

**Design.**

1. **Tag every consensus message with its session** in the envelope. The
   receiver routes votes and sync requests by it. The tag needs no
   signature: a misrouted vote fails verification, because the header
   digest it covers names the epoch.
2. **Keep the previous session's core for a drain window, serving only.**
   It answers sync requests and fetches by round range for the old
   session, but it proposes and votes no more. It needs no new rounds:
   every certificate a laggard needs to commit the closing anchor already
   exists, because honest validators committed it. Bullshark's commit rule
   then gives the laggard the same anchor and the same order.
3. **Close the window late.** The window closes when the old session's
   last Pulse is certified and every member of the new committee has
   proposed in the new session, or after 30 s, whichever comes later,
   with a cap of 120 s (targets, to tune with measurements).
4. **Pulse sync as the fallback.** It needs no drain window. A laggard
   fetches the certified header of the last Pulse and its `PulseRecord`
   from any peer. It checks the certificate against the epoch's signers,
   which its own state names. It executes the record and requires the
   result to match the header's `after`, `order_digest`, `receipts_root`,
   `data_root` and `beacon`. A Byzantine peer can withhold or serve stale
   data, but cannot make it accept a wrong Pulse. This turns the drain into
   a latency optimisation; safety and liveness no longer depend on it.
5. **Do not buffer the future.** Replace the 20,000-message future buffer
   with a small cap per sender (4 MiB, proposed), plus a fetch by round
   range once the node reaches the new session.

`impl:reliable-handover` retires when the boundary tests in the
[fault matrix](#fault-injection-matrix) pass. One of them delays one
validator by 10 s at a boundary under 10 % loss. In another, a validator
misses the whole drain window and catches up by Pulse sync.

### Restarts without equivocation

The devnet bumps a restart generation shared by every validator.
`committees::session` puts it in the top 32 bits of the session number. So
after a restart every validator starts round 1 of a session nobody has
signed in. One machine restarting alone must rejoin the current session
instead. If it signs a different header for a round it already signed
before the crash, its whole stake is slashable as header equivocation
([equivocation slashing](../concepts/validators/equivocation-slashing.md)).
Votes are safe only as long as it never votes for a second header in a
slot.

The design adds a **consensus signing log**, written and synced before any
header or vote leaves the node. The lane log already works this way for
approvals: `seal_lane_batches` appends before sending. The log holds the
highest round proposed with that header's digest, and the digest voted for
in each (round, author) slot above the garbage-collection round. On
restart the core reloads the log and refuses to sign anything that
conflicts with it. Signing is deterministic (`sign_deterministic`), so
re-sending an identical header or vote produces the same bytes. The
session becomes the epoch alone, and the restart generation goes away from
networked deployments. The devnet may keep the generation.

### Lane lock hand-over (`impl:lock-handover`)

**Today.** A lane certificate is checked against the committee in force
when its transaction executes. When a shard's lane committee changes,
members who stay keep their locks, and new members start without any. The
gateway re-certifies pending transactions with the new committee. An owner
can exploit this in three steps:

1. it signs spend A of an object version;
2. the old committee certifies A, and the certificate is withheld or slow;
3. the owner signs spend B of the same version and sends it to a fully
   replaced committee, which certifies it.

B executes and A never does. A's certificate finality fails.

Sui Lutris ends an epoch by draining through consensus. Once the new
committee is ready, the old one "stops signing new transactions or locking
objects", makes sure every certificate it processed is sequenced, and
sends End-of-Epoch messages. The new committee takes over after 2*f* + 1
of them are ordered, and locks are dropped at the change. That fits a
design where validators hold the certificates they processed. Here the
certificate exists at the gateway, and no member knows whether a quorum
approved. So this plan reads the locks instead.

**Design (needs a specification version and both proof stacks).**

1. **Lock reads.** For *H* Pulses after a change, a new member checks
   every lane request that spends a version created before the change. It
   first asks the outgoing committee: "is this version locked, and to
   which owner-signed transaction?" It waits for answers from a quorum of
   the outgoing committee's stake.
2. **Why the reads are safe.** If A was certified, at least *q* outgoing
   members locked the version to A. Any quorum of answers overlaps them in
   at least *q* + *q* − *n* ≥ *f* + 1 members, so at least one honest
   member reports A. A Byzantine member cannot invent a lock, because a
   lock report carries the owner's signed transaction.
3. **Adopting reported locks.** The new member locks the version to the
   reported transaction and refuses the conflicting request. The refusal
   is owner-equivocation evidence. A version reported locked to two
   different owner-signed transactions stays frozen until *H* Pulses have
   passed.
4. **A grace window.** For the same *H* Pulses, the state accepts a lane
   certificate formed by the shard's previous committee. A certified A then
   executes even when a frozen version stops it from being certified
   again. The state keeps the previous lane committees for *H* Pulses, and
   the World Proof's validator witness covers them.
5. **A bounded horizon.** *H* is finite only if expiries are. Today
   `expires_at` has no cap (`owners.rs` checks only that it is in the
   future). The specification gains one: at lock time, `expires_at` ≤
   current Pulse + *H* (*H* = 16 Pulses proposed, at most the unbonding
   period). Leaving validators must answer lock reads until *H* has passed
   after their last change. Unbonding already keeps their stake at risk
   for 20 Pulses on the devnet.

Loss only delays this. Lock reads are idempotent and retried. A new member
that cannot reach a quorum of the outgoing committee does not approve
older versions until *H* passes. That costs liveness for those objects,
never safety. A forced-inclusion rotation is a committee change too, and
it uses the same path. `impl:lock-handover` retires when a test passes:
an owner double-spends across a full committee replacement, the first spend
executes and the second is refused, under 10 % loss.

### Carried Pulse-builder state: a gap found while planning

The Pulse builder keeps valid beacon reveals that consensus committed ahead
of their Pulse, up to 8 Pulses (`REVEAL_WINDOW`, `PulseBuilder::reveals`).
`next_session` carries them across a session change. They are not in the
state, in `PulseRecord` or in `Snapshot`. On the devnet every validator
restarts together and loses them equally. On a network, a validator that
restarts or catches up alone would lack them. It would compute a different
beacon for up to 8 Pulses, and so execute a different state. Honest
validators reveal only for the next Pulse, after executing the previous
one. But the rules accept early reveals, so any validator can create this
state on purpose.

The fix is to carry pending reveals in `PulseRecord` and snapshots. Pulse
headers should also commit to them, so that a validator syncing from
certified headers derives the same beacon. This needs a specification
version.

The first half is done. Each closed Pulse now hands over a `Carry` (the
beacon and the reveals waiting for later Pulses) as of its own close, and
records and snapshots store it (storage format 4). A validator resumes
from it with `PulseBuilder::resume`. Taking the carry at the close, not
when the record is written, also fixes a second gap: one commit can close
two Pulses before the first executes, and the record of the first then
held the beacon after the second. The test
`a_builder_resumed_from_a_carry_closes_pulses_as_if_it_never_stopped`
shows a builder resumed with the beacon alone diverging, and one resumed
from the carry closing the next Pulses with identical beacons. The header
commitment is still to do.

## 5. Data availability over the network

### What validators already have

Every validator executes every Pulse and builds its published data itself:
transaction bodies in execution order, without signatures, plus the forced
intents the Pulse posted. It then
computes the 2D Reed–Solomon square and signs a header that commits to the
data root (`Validator::execute`). So, unlike Celestia or Ethereum, where
the producer of a block holds its data first, no validator needs shares
from another. Dissemination is about getting data to the nodes that serve
light clients, archivers and reconstructors.

### Serving shares

1. **Validators stream Pulse data to full nodes.** Each validator streams
   every executed Pulse's published data to the full nodes subscribed on
   its ingress port: the blob, a quarter of the extended square.
2. **Full nodes check before they serve.** A full node encodes the blob,
   checks that it reproduces the certified data root and `k`, and only
   then serves. Encoding 2 MiB took 0.24 s and committing to its rows and
   columns 8.0 s on one core (measured,
   [benchmarks](../guides/benchmarks.md)). Full nodes need
   parallel encoding, as the validators do. This cost is already an open
   issue.
3. **Full nodes serve in containers, not only single shares.** Celestia's
   Shwap upgrade addressed data in containers (sample, row, share range,
   blob). It cut a light node's sample from seven round trips to one for
   a 128 × 128 square and made sampling 12 times faster. Full nodes here
   serve single shares, batches of samples, whole rows and columns, and
   whole blobs.
4. **Retention.** Validators keep published data in memory for 64 Pulses,
   as today. Older data exists only in the write-ahead log, from which the
   node serves no shares today (`data pruned`). A full node could re-encode
   older Pulses from its log on demand (not built). Archival remains not
   built.

### Sampling for light clients

- **Prove to the data root, not to the axis-root list.** Today a light
  client downloads the data-availability header, 2*k* row roots and 2*k*
  column roots, and checks each share against its row root. For large
  squares the header outweighs the samples. The data root is a Merkle
  root over the tagged axis roots (`DaHeader::compute_data_root`). So a
  sample can carry two proofs instead: one from the share to its row
  root, and one from the row root to the data root, 32 × log₂(4*k*) bytes.

  | *k* | Header and 16 samples (today) | 16 samples with proofs to the data root |
  |---|---|---|
  | 16 | 4.8 KB | 5.8 KB |
  | 512 | 70.9 KB | 10.9 KB |
  | 2,048 | 268 KB | 13.0 KB |

  The sizes are computed for Borsh; JSON with hex doubles them. Proofs to
  the root win from *k* = 32 at 16 samples. Clients use the header for small
  squares and proofs above that.
- **Sample from many sources.** A light client draws its positions from
  fresh randomness, as `sion verify` already does. It sends each sample, or
  each small batch, to a different full node or validator, chosen at
  random from a published list, over separate connections. Asking a single
  node, as today, tells the client only whether that node serves.
- **Certificates.** A client that samples a Pulse also needs that Pulse's
  certificate: 59 KB at *n* = 4 and 1.3 MB at *n* = 100 (computed). That
  is more than the samples at any committee size above a handful. Two
  responses:
  - a client checks certificates only for the Pulses it uses, plus epoch
    boundaries;
  - aggregation of signatures inside proofs, an open research problem in
    the architecture paper.

### Unlinkability

The guarantee holds only "provided requests are unlinkable so that an
adversary cannot answer only the samplers it chooses" (architecture).
Separate connections to random nodes still reveal the client's IP address
to each server. This plan does not solve that; section 7 lists it as an
open question. Options include a relay or mixnet layer for sample requests,
and light clients that exchange samples with each other. The second
requires peer-to-peer serving from behind NAT, the one place where libp2p's
relays and hole punching would earn their weight.

### Reconstruction and fraud proofs

A node that suspects withholding asks several full nodes for rows and
columns, and decodes with the existing iterative decoder
(`sion_da::reconstruct`). One honest server that holds the whole square is
enough. Across several servers, decoding always succeeds while fewer than
(*k* + 1)² of the 4*k*² shares are missing (`sion_da::min_withholding`),
that is, while more than about three quarters are served. The World Proof checks the
encoding, and bad-encoding fraud proofs exist as a library. On the
network, a fraud proof travels on the control class to every validator and
on the observer stream to full nodes. Like any evidence, it is checked
before it is forwarded.

Ethereum's PeerDAS (EIP-7594, final) takes a different route. It extends
blobs in one dimension, spreads columns over gossip subnets, gives each
node a deterministic set of columns to keep by its node ID, and lets a
node holding half the columns reconstruct the rest. It is the reference
point if Siontology's serving tier ever has to split custody of squares
too large for one full node.

## 6. Implementation plan

### Rules for every phase

- `sion devnet` keeps working, in process, at every step. The existing
  tests pass unchanged.
- One protocol, three carriers. Validators and gateways call a `Transport`
  trait. A memory transport reproduces today's bus, a fault-injecting
  transport wraps any other, and a QUIC transport carries messages between
  machines.
- The receiver takes the sender's identity from the transport, never from
  the message.
- Seeded randomness (`sion_core::rng`) for every fault injection, so that
  failures reproduce.
- Every change that introduces a shortcut names it as an atom in
  `docs/implementation-status.md`.

### The `Transport` trait (sketch)

```rust
/// A node: a validator, or another node (gateway, full node) named by the
/// digest of its network key.
pub enum NodeId { Validator(ValidatorId), Node(Hash32) }

/// What one node sends another. Borsh on the wire; each variant belongs to
/// a class with its own size limit and priority.
pub enum PeerMessage {
    Consensus { session: u64, message: sion_consensus::Message },
    Payload { item: Vec<u8> },
    Lane { shard: ShardId, request: LaneRequest },
    BatchSealed { shard: ShardId, header: BatchHeader, signature: Vec<u8> },
    Approved { shard: ShardId, tx: TxDigest, batch: Hash32, leaf_index: u64, path: Vec<Hash32> },
    LaneRejected { tx: TxDigest, reason: String },
    PulseSignature { pulse: Pulse, header: Hash32, signature: Vec<u8> },
    ForcedApproval { shard: ShardId, tx: TxDigest, proof: ApprovalProof },
    Executed { pulse: Pulse, digests: Vec<TxDigest> },
    // Phase 1 adds: fetch by round range, Pulse sync, signature pulls, lock reads.
}

/// Carries peer messages. On the receiving side the transport delivers
/// `Input::Peer { from, message }` into the node's existing inbox, with
/// `from` taken from the connection.
pub trait Transport: Send + Sync + std::fmt::Debug {
    /// This node.
    fn local(&self) -> NodeId;
    /// Queues a message for one node. Never blocks the protocol thread.
    fn send(&self, to: NodeId, message: PeerMessage);
    /// Queues a message for several nodes.
    fn send_all(&self, to: &[NodeId], message: &PeerMessage);
    /// Replaces the nodes this one keeps connections to, with their
    /// network records (after every committee change).
    fn set_peers(&self, peers: &PeerSet);
}
```

- **Typed messages.** `PeerMessage`, the trait and the memory and faulty
  transports live in `sion-node`. A new `sion-net` crate holds the
  byte-level machinery: the `quinn` endpoint on a small tokio runtime,
  TLS, framing, connection management and quotas. It depends on
  `sion-core` and `sion-crypto`, and `sion-node` adapts it.
- **The protocol thread does not change.** The validator thread keeps its
  `mpsc` loop, and so does the gateway thread. A tokio runtime on separate
  threads bridges the network into the same inbox.
- **Local control stays local.** The variants that carry reply channels
  stay as they are.

### Phase 0: the seam, in process

Milestones:

1. Add `PeerMessage`, `NodeId`, the `Transport` trait and `MemoryTransport`,
   and route every validator-to-validator and gateway-to-validator send
   through it. The transport stamps `from`. `MemoryTransport` can also
   encode and decode every message, which tests the size limits.
2. Add `FaultyTransport<T>`. It injects seeded loss, delay with jitter,
   duplication, reordering and partitions, per message class, and a timer
   thread releases delayed messages. A scenario script changes the faults
   over time.
3. Count messages and bytes per class and per peer, and report them in
   `sion status` and `sion bench`. This gives measured values to replace
   this note's computed ones.

Tests: the existing suite, unchanged. A property test that every
`PeerMessage` round-trips through Borsh within its class limit. A devnet
test with `FaultyTransport` at zero faults that matches the memory
transport's results.

Exit criteria: all tests pass. The devnet's measured Pulse interval stays
within 10 % of today's on the same machine (target).

### Phase 1: a protocol that survives loss, still in process

Milestones:

1. Session-tagged envelopes. The previous session's core serves for the
   drain window. Add fetch by round range.
2. Retries:
   - the gateway retries lane requests whose approvals are late (missing
     after two batch intervals plus one round trip, proposed);
   - validators re-send their Pulse signatures and forced approvals until
     the Pulse or the intent is certified, and answer signature pulls.
3. Batch announcements: approvals carry references, and each batch
   signature travels once per gateway.
4. ~~The gateway caps a group by bytes below `max_payload_bytes`, not
   only at 512 transactions.~~ Done.
5. Pulse sync from certified headers and `PulseRecord`s. Pending beacon
   reveals are carried in records and snapshots (done); Pulse headers
   commit to them (specification change, to do).
6. A consensus signing log, so a single validator can restart into the
   current session. Sessions no longer include a restart generation in
   networked mode.
7. Lane lock hand-over: lock reads, the grace window and the expiry cap.
   This is a specification version, implemented by both proof stacks and
   covered by the differential tests.
8. A deterministic node simulator. It abstracts the clock and drives
   `Validator` and `Gateway` state machines from one event queue with
   seeded faults, the method `sion_consensus::sim` already uses for the
   core. It covers lanes, Pulses, epochs and hand-overs, which the
   consensus simulator does not reach.

Tests: the rows of the [fault matrix](#fault-injection-matrix) marked
"in process", run both on real threads with `FaultyTransport` and in the
deterministic simulator. Property tests over random fault schedules (loss
up to 10 %, one-way delays up to 250 ms, partitions of up to *f*
validators) check two things. Safety: no two certified headers for one
Pulse, no conflicting lane certificates, and no equivocation evidence
against an honest validator. Liveness: progress resumes after the faults
stop.

Exit criteria (targets):

- Four validators run 2 hours at 5 % loss and 20 to 200 ms delay with
  safety intact.
- Every submitted transaction executes or is rejected within 30 s.
- Certified Pulses arrive at 80 % or more of the no-fault rate.
- An epoch boundary under 10 % loss completes without a stall, and a
  validator kept out for its whole drain window catches up by Pulse sync.

`impl:reliable-handover` and `impl:lock-handover` retire at the end of
this phase.

### Phase 2: QUIC between processes

Milestones:

1. `sion-net`: the `quinn` endpoint, rustls with the aws-lc-rs provider
   (X25519MLKEM768 only, `mldsa44` only, TLS 1.3 only), the pinned-key
   verifier, framing with class limits and priorities, connection
   management, keep-alive and quotas.
2. `sion keys`: generate a validator's dual keys and its network key
   locally, and write its signed network record. A genesis file collects
   public keys, possession proofs and records. Devnet keys stay for
   `sion devnet` only (`impl:devnet-keys`).
3. `sion node`: one validator per process, with its own data directory,
   HTTP API on localhost, optional embedded gateway, and a peer file.
   `sion gateway`: a gateway as its own process. It learns committees
   from a node, or as a light client does from epoch proofs, and it
   receives approvals as responses on its own connections.
4. A TCP fallback using the same frames.

Tests:

- Four processes on one machine, and four machines on one LAN, run the
  phase 1 suite over QUIC.
- The same faults, now injected with `tc netem` (delay with jitter and
  distributions, random and Gilbert–Elliot loss, duplication, reordering,
  rate limits) and with process kills.
- Fuzzing of the frame decoder and of every message decoder.
- Handshake tests:
  - an unknown key is refused;
  - a key from a superseded record is refused;
  - a peer that offers only classical groups fails;
  - a handshake that needs the extra round trip of the amplification
    limit still completes.

Exit criteria (targets):

- 24 hours on a LAN with one validator `kill -9`'d every 10 minutes:
  - zero safety violations, and zero evidence against any validator;
  - every restart rejoins within 30 s.
- Bytes per message class within 20 % of the computed values in
  section 2.

### Phase 3: a multi-region testnet

Milestones:

1. Phase 3a: seven validators in four regions (below), two gateways, two
   full nodes serving the API and shares, a prover node, load and
   light-client generators, and monitoring.
2. Add the minimum round interval and replace periodic re-broadcasts with
   acknowledged delivery.
3. DoS controls on all ports. Leader timeout starts at 1,000 ms: the
   longest one-way delay, Frankfurt to Tokyo, is about 115 to 120 ms, and
   the core's own comment asks for more than three network delays. It is
   then tuned down from measurements.
4. Phase 3b: a scaling run with 16 to 20 validators on flat-rate hosts in
   one region, with WAN delays added by `netem`, to test the model in
   section 2 where it predicts the certified DAG's limit.

Tests: a chaos day each week, with these faults:

- kill two of seven validators;
- cut a whole region, which holds at most two validators;
- partition the Tokyo pair for 10 minutes;
- push 100 transfers a second for an hour.

Exit criteria (targets):

- Seven days without a stall.
- Owned-lane certificate latency at its 50th and 95th percentiles is
  measured and compared with the architecture's target of 0.1 to 0.5 s
  plus one batch interval. Contended commit latency and the Pulse interval
  are measured too.
- Bytes per validator within 30 % of the model.
- Pulses keep closing while any one region is cut off, and that region's
  validators catch up within 60 s of reconnecting.
- A light client verifies receipts and samples data from full nodes only.

### Phase 4: scale, driven by what phase 3 measures

- Payloads by digest: groups disseminated once, headers carry digests, and
  validators fetch what they miss. This is Narwhal's and Quorum Store's
  split.
- An uncertified DAG (Mysticeti class). It is already the architecture's
  production target and on the roadmap.
- Parallel signature checking.
- Network records in the state.
- A serving tier where light clients sample across many full nodes, and
  research into unlinkable sampling.
- An erasure-coded relay for large payloads, only if the proposer's egress
  binds.

### Fault-injection matrix

| Scenario | Where | Parameters | Expected |
|---|---|---|---|
| Delay | In process; netem | 20 to 250 ms one way, 20 % jitter | Safe and live; Pulse interval as the model predicts |
| Random loss | In process; netem | 1 %, 5 %, 10 % | Safe; live through retries; no transaction stuck `certifying` |
| Burst loss | netem (Gilbert–Elliot) | Bursts of 1 to 5 s | Safe; live after each burst |
| Minority partition | In process; netem | 1 of 4, or 2 of 7, cut off for 60 s | The majority continues; the minority catches up by sync |
| Even split | In process; netem | 2 against 2 for 60 s | No progress and no safety violation; resumes after healing |
| One-way link | netem | A → B dropped, B → A intact | Live |
| Leader isolation | In process | The anchor of each even round cut off | Timeouts fire; commits continue |
| Crash and restart | Processes | `kill -9` every 5 to 10 minutes | No evidence against the restarted validator; it rejoins |
| Epoch boundary under loss | In process; netem | 10 % loss; one validator delayed 10 s at the boundary | Every validator reaches the new session; no stall |
| Missed drain window | In process | One validator offline across the boundary for 3 minutes | It catches up by Pulse sync |
| Double spend across a committee change | In process | Owner signs A (certified, withheld) and B after a full replacement | A executes; B is refused |
| Gateway loss | In process | 20 % of lane requests and approvals dropped | Every transaction certifies or is rejected within 30 s |
| Byzantine peer | Processes | Oversized frames, junk, future-session floods, spoofed senders | Memory stays bounded; honest throughput within 10 % |
| Clock skew | Processes | ±30 s on one machine | No effect (no wall-clock rules; long-lived certificates) |
| Bandwidth cap | netem | 100 Mbit/s per validator | Throughput and latency as the model predicts |

### Multi-machine testnet plan

Regions and placement for phase 3a: seven validators tolerate *f* = 2,
with *q* = 5. With at most two validators per region, losing any one
region leaves five, still a quorum.

| Region (AWS) | Validators | Other machines |
|---|---|---|
| us-east-1 (N. Virginia) | 2 | Gateway, full node, prover, load generator |
| eu-central-1 (Frankfurt) | 2 | Gateway, light-client generator, monitoring |
| ap-northeast-1 (Tokyo) | 2 | Full node |
| us-west-2 (Oregon) | 1 | none |

Measured round trips between the nearest cities (WonderNetwork, hourly
averages over two weeks in October 2026): New York to Frankfurt 86 to
91 ms, New York to Tokyo 173 to 185 ms, Frankfurt to Tokyo 227 to 237 ms.
Oregon's links were not measured for this note.

A certified round needs three message delays to a quorum, plus signing.
With one-way delays of 45 to 120 ms and 0.1 to 0.2 s of signing on one
core, that means about 2 to 5 rounds a second (estimate). The proposed
minimum round interval caps it at 4, so a Pulse of 4 rounds lands every 1
to 2 s.

#### Costs

On-demand prices from instances.vantage.sh and AWS, October 2026. The
listed instance prices may be for us-east-1; other regions differ. A month
is 730 hours.

| Item | Count | Unit | Unit price | Per month |
|---|---|---|---|---|
| Validator: c7i.4xlarge (16 vCPU, 32 GiB, up to 12.5 Gbit/s) | 7 | hour | 0.714 USD | 3,649 USD |
| Validator storage: 500 GB gp3 | 7 | GB-month | 0.08 USD | 280 USD |
| Prover: c7i.4xlarge | 1 | hour | 0.714 USD | 521 USD |
| Gateway: c7i.2xlarge (8 vCPU, 16 GiB) | 2 | hour | 0.357 USD | 521 USD |
| Full node: m7i.2xlarge (8 vCPU, 32 GiB) | 2 | hour | 0.4032 USD | 589 USD |
| Load and light-client generator: c7i.2xlarge | 2 | hour | 0.357 USD | 521 USD |
| Monitoring: m7i.xlarge (4 vCPU, 16 GiB) | 1 | hour | 0.2016 USD | 147 USD |
| Validator mesh traffic, idle | 35 to 69 TB | GB | about 0.02 USD between regions, 0.01 USD each way between zones | about 700 to 1,400 USD |
| Internet egress (API, light clients) | about 1 TB | GB | about 0.09 USD after 100 GB free | about 90 USD |
| **Total, idle** | | | | **about 7,000 to 7,700 USD** |

The idle mesh traffic is an estimate. Each validator sends 0.94 MB per
round at 2 to 4 rounds a second, with no periodic re-broadcasts: 1.9 to
3.7 MB/s, or 4.9 to 9.8 TB per validator a month. 18 of the 21 validator
pairs cross a region, and the rest are assumed to cross zones.
Re-broadcasts as built would add about a third.

Load adds about 0.9 MB per transfer at *n* = 7 as built (section 2). An
hour at 100 transfers a second therefore moves about 330 GB, about 7 USD of
transfer (estimate). The summary's range of 7,000 to 8,000 USD a month
spans 2 to 4 idle rounds a second plus a few load hours a week.

Two ways to spend less:

- **Phase 2 on flat-rate hosts.** Phase 2 needs four machines in one place,
  for example four Hetzner AX42 (8 cores, 64 GB) at 79 to 99 EUR a month
  each, plus a 49 EUR setup fee.
- **Phase 3b off AWS.** On AWS, 20 validators would send about 6.3 MB per
  round each, so the traffic between regions alone would cost tens of
  thousands of USD a month (estimate). Phase 3b therefore runs in one
  place on flat-rate hosts, for example 20 Hetzner AX102 (16 cores,
  128 GB, traffic listed as unlimited) at 159 to 259 EUR a month each plus
  129 EUR setup: about 3,200 to 5,200 EUR a month. `netem` adds the WAN
  delays. Hetzner lists data centres in Nuremberg, Falkenstein and
  Helsinki, so real geography stays the job of the seven-validator AWS
  testnet. Its page does not state the AX102's port speed; confirm
  1 Gbit/s before ordering.

For scale, Sui suggests 24 physical cores, 128 GB of memory, 4 TB of NVMe
and 1 Gbit/s per validator, and Aptos 48 threads, 128 GB, 3 TB and
1 Gbit/s; both are sized for their main networks. This testnet's machines
are sized for devnet-scale state and a few hundred transfers a second.

## 7. Risks and open questions

### Risks

| Risk | Signal to watch | Mitigation |
|---|---|---|
| Post-quantum signatures make the certified DAG too costly beyond about 10 to 20 validators (section 2) | Bytes and CPU per round in phase 3b against the model | Uncertified DAG, payloads by digest, parallel checking, round pacing. Do not plan a public testnet larger than 20 validators before these land |
| **Fixed.** A certified group larger than `max_payload_bytes` was dropped without a reply: the gateway limited groups to 512 transactions, and about 20 dual-signed transfers fill 512 KiB, so a burst was re-sent every 4 s at the same size until it expired. The gateway now splits groups by encoded bytes, at most half the limit (`PayloadItem::groups`), and refuses a transaction whose certificate cannot fit alone. `a_burst_larger_than_a_consensus_payload_still_executes` reproduced the loss and passes with the fix | Transactions stuck in `sequencing` under load | Count dropped submissions in consensus, so a new limit mismatch shows up as a number |
| A validator that restarts alone equivocates and is slashed | Evidence against an honest validator in the restart tests | Consensus signing log before any send (phase 1); restart tests in every phase |
| Pending beacon reveals were lost by a validator that restarts or syncs alone (section 4). **Half fixed:** records and snapshots now carry them (storage format 4) | A restarted validator's headers differ from the certified ones | Commit to them in Pulse headers, so a validator syncing from certified headers derives the same beacon (specification) |
| Lock hand-over needs specification changes: expiry cap, grace window, lock reads. The cap restricts what wallets may sign | Differential-test failures between the stacks; wallet complaints about short expiries | One specification version with all three; both stacks; tests in the fault matrix |
| C code in the TLS provider (aws-lc-rs) in a workspace whose post-quantum code is pure Rust | Build failures on some targets; advisories | Pin versions; track advisories; evaluate a provider built on `fips203` and `fips204` |
| UDP blocked or throttled on some operators' networks | Connection failures by network | TCP fallback with the same frames |
| Flow control mis-sized for large messages over long links | Rounds slower than three delays; stalls on 1.3 MB certificates | Windows of about 32 MB; four priority classes; measure |
| Unbounded or count-only buffers become memory exhaustion once peers are remote (section 3) | Memory per validator under the Byzantine-peer test | Bounds in bytes per sender before phase 2 |
| Light-client sampling against one node proves nothing about withholding by that node | The share of light clients that use more than one source | Sampling across many full nodes (phase 4); research on unlinkability |
| Certificates of 1.3 MB per Pulse at 100 validators are heavy for phones | Light-client download per Pulse | Check certificates only where needed; aggregation inside proofs (research) |
| Traffic between regions costs more as *n*² × signature size grows | The monthly bill against the estimate | Round pacing, acknowledged delivery, flat-rate hosts for scale tests |
| A large networking stack has a large attack surface. rust-libp2p had two remote-panic advisories in 2026 | Advisories for `quinn`, `rustls`, `aws-lc-rs` | Keep the stack small; fuzz every decoder; no panics on peer input |

### Open questions

1. Where do network records live? A testnet file is enough for phases 2
   and 3. Light clients and full nodes finding validators on their own
   needs them in the state, which is a specification change.
2. Should gateways register, or bond? Registration buys them a quota.
   Bonding would make their misbehaviour cost something, but a gateway can
   only drop or delay, which forced inclusion already counters.
3. Should the consensus committee be sampled, as the architecture plans,
   rather than the whole active set? A sample of 31 to 50 would bound the
   mesh and the per-round cost independently of the number of validators.
4. Is a drain window of 30 to 120 s right? It depends on how long
   validators take to execute the last Pulse of an epoch under load.
   Phase 1 measures it.
5. Is *H* = 16 Pulses the right bound for lock hand-over and certificate
   grace? A shorter *H* restricts expiries; a longer one keeps leaving
   validators online longer.
6. Should Pulse header certificates be aggregated, or replaced inside the
   World Proof by a proof that a quorum signed, for light clients?
7. Should light clients serve shares to each other? That needs NAT
   traversal. It is the case for adopting libp2p, or something like it,
   in the public tier only.
8. Can `quinn` run under a deterministic simulator? `turmoil` simulates
   hosts, latency, drops, partitions and crashes in one thread, and
   `turmoil-net` stands in for `tokio::net`. `quinn` has its own UDP layer,
   so it may need an adapter. Until then, the transport is tested with
   `netem` and process kills, and the protocol with the deterministic node
   simulator.

### Trust atoms this plan adds and retires

| Atom | Status | Meaning |
|---|---|---|
| `impl:in-process-network` | Retired in phase 2 for networked deployments; kept for `sion devnet` | Validators share a process; network faults are not exercised |
| `impl:reliable-handover` | Retired in phase 1 | The epoch hand-over relies on reliable delivery |
| `impl:lock-handover` | Retired in phase 1 (specification version) | Certificates are not drained when a lane committee changes |
| `net:transport-ml-dsa` | Added in phase 2 | Transport authentication rests on ML-DSA-44 alone; a forger could impersonate peers at the transport layer, not sign consensus messages |
| `net:aws-lc` | Added in phase 2 | The TLS provider is aws-lc-rs, C code outside the workspace's audit scope |
| `net:static-peers` | Added in phase 2; retired when records move into the state | Peers come from a testnet file of signed records |
| `net:partial-synchrony` | Assumed by the architecture's security model; named as an atom here because a real network now exercises it | Liveness needs message delays to become bounded after some time |

## Sources

Opened while writing this note, October 2026.

**This repository**

- [`docs/implementation-status.md`](https://github.com/ColdAI-org/siontology/blob/main/docs/implementation-status.md): status words, trust atoms, the review findings
- [`docs/book/src/papers/architecture.md`](../papers/architecture.md): ordering, committees and sharding, data availability, capacity model, latency targets
- [`docs/book/src/guides/benchmarks.md`](../guides/benchmarks.md): measured signature, encoding and devnet costs
- [`docs/book/src/project/operations.md`](../project/operations.md), [`architecture/validator.md`](../architecture/validator.md), [`architecture/gateway.md`](../architecture/gateway.md), [`concepts/validators/epochs.md`](../concepts/validators/epochs.md), [`concepts/data-availability.md`](../concepts/data-availability.md)
- Code: `sion-node` (`devnet.rs`, `validator.rs`, `gateway.rs`, `payload.rs`, `pulses.rs`, `storage.rs`, `owners.rs`, `api.rs`, `config.rs`, `committees.rs`, `keys.rs`), `sion-consensus` (`core.rs`, `dag.rs`, `lanes.rs`, `committee.rs`, `beacon.rs`, `sim.rs`), `sion-da`, `sion-light`, `sion-crypto`

**Standards**

- [RFC 10024: PQ/T hybrid key agreement for TLS 1.3](https://www.rfc-editor.org/info/rfc10024) (August 2026) and [draft-ietf-tls-ecdhe-mlkem](https://datatracker.ietf.org/doc/draft-ietf-tls-ecdhe-mlkem/) (codepoints and key-share sizes)
- [draft-ietf-tls-mldsa-05: ML-DSA in TLS 1.3](https://www.ietf.org/archive/id/draft-ietf-tls-mldsa-05.html) (6 July 2026)
- [RFC 9881: ML-DSA in X.509 certificates](https://www.rfc-editor.org/rfc/rfc9881.html) (October 2025)
- [RFC 9000: QUIC](https://www.rfc-editor.org/rfc/rfc9000.txt), section 8.1, the amplification limit
- [EIP-7594: PeerDAS](https://eips.ethereum.org/EIPS/eip-7594)

**Rust libraries**

- [rustls manual: defaults](https://doc.servo.org/rustls/manual/_05_defaults/index.html) (0.23.46), [rustls releases](https://github.com/rustls/rustls/releases), [`DEFAULT_KX_GROUPS`](https://docs.rs/rustls/latest/rustls/crypto/aws_lc_rs/static.DEFAULT_KX_GROUPS.html), [Phoronix on rustls 0.23.44](https://phoronix.com/news/Rustls-0.23.44-Released)
- [rustls-webpki `aws_lc_rs` algorithms](https://docs.rs/rustls-webpki/latest/webpki/aws_lc_rs/index.html) (0.103.16)
- [quinn](https://github.com/quinn-rs/quinn), [quinn on crates.io](https://crates.io/api/v1/crates/quinn), [`SendStream`](https://docs.rs/quinn/latest/quinn/struct.SendStream.html), [`TransportConfig`](https://docs.rs/quinn/latest/quinn/struct.TransportConfig.html)
- [libp2p crate](https://docs.rs/libp2p), [rust-libp2p releases](https://github.com/libp2p/rust-libp2p/releases), [libp2p TLS spec](https://github.com/libp2p/specs/blob/master/tls/tls.md), [gossipsub v1.1 spec](https://github.com/libp2p/specs/blob/master/pubsub/gossipsub/gossipsub-v1.1.md), [rust-libp2p gossipsub config](https://raw.githubusercontent.com/libp2p/rust-libp2p/master/protocols/gossipsub/src/config.rs)
- [CVE-2026-61544 (libp2p-quic)](https://osv.dev/vulnerability/CVE-2026-61544), [CVE-2026-33040 (gossipsub)](https://ubuntu.com/security/CVE-2026-33040)
- [turmoil](https://github.com/tokio-rs/turmoil), [tc-netem(8)](https://man7.org/linux/man-pages/man8/tc-netem.8.html)

**Production systems**

- Sui: [consensus](https://docs.sui.io/concepts/sui-architecture/consensus), [consensus network traits](https://raw.githubusercontent.com/MystenLabs/sui/main/consensus/core/src/network/mod.rs), [tonic network](https://raw.githubusercontent.com/MystenLabs/sui/main/consensus/core/src/network/tonic_network.rs), [TLS server name](https://raw.githubusercontent.com/MystenLabs/sui/main/consensus/core/src/network/tonic_tls.rs), [`sui-tls`](https://raw.githubusercontent.com/MystenLabs/sui/main/crates/sui-tls/src/lib.rs) and [its verifier](https://raw.githubusercontent.com/MystenLabs/sui/main/crates/sui-tls/src/verifier.rs), [validator configuration](https://docs.sui.io/guides/operator/validator-config)
- [Mysticeti paper](https://arxiv.org/abs/2310.14821) (v6, November 2025); [Sui Lutris paper](https://arxiv.org/abs/2310.18042) and [its reconfiguration section](https://arxiv.org/html/2310.18042v5)
- Aptos: [network README](https://raw.githubusercontent.com/aptos-labs/aptos-core/main/network/README.md), [Noise handshake](https://raw.githubusercontent.com/aptos-labs/aptos-core/main/network/framework/src/noise/mod.rs), [node requirements](https://aptos.dev/network/nodes/validator-node/node-requirements), [Quorum Store](https://forum.aptosfoundation.org/t/aptos-quorum-store-breaking-the-bottleneck/16671), [Baby Raptr](https://aptosnetwork.com/currents/baby-raptr-lands-on-mainnet)
- Solana: [Turbine (Helius, October 2023)](https://www.helius.dev/blog/turbine-block-propagation-on-solana), [stake-weighted QoS](https://solana.com/docs/defi/stake-weighted-qos), [Alpenglow (Anza, May 2025)](https://www.anza.xyz/blog/alpenglow-a-new-consensus-for-solana)
- [Narwhal and Tusk](https://arxiv.org/abs/2105.11827)
- [Celestia: the Shwap upgrade](https://blog.celestia.org/shwap/)
- [IOTA: DoS protection for validators](https://docs.iota.org/operator/activate_DoS_protection_for_validators) (Sui-derived traffic controller)

**Latency and prices**

- WonderNetwork pings: [New York to Frankfurt](https://wondernetwork.com/pings/New%20York/Frankfurt), [New York to Tokyo](https://wondernetwork.com/pings/New%20York/Tokyo), [Frankfurt to Tokyo](https://wondernetwork.com/pings/Frankfurt/Tokyo)
- instances.vantage.sh: [c7i.4xlarge](https://instances.vantage.sh/aws/ec2/c7i.4xlarge), [c7i.2xlarge](https://instances.vantage.sh/aws/ec2/c7i.2xlarge), [m7i.2xlarge](https://instances.vantage.sh/aws/ec2/m7i.2xlarge), [m7i.xlarge](https://instances.vantage.sh/aws/ec2/m7i.xlarge)
- [AWS EC2 on-demand pricing](https://aws.amazon.com/ec2/pricing/on-demand/) (100 GB of free egress), [AWS EBS pricing](https://aws.amazon.com/ebs/pricing), [beSharp: AWS data transfer costs](https://blog.besharp.it/aws-data-transfer-costs-in-a-nutshell/) (March 2026, updated June 2026)
- [Hetzner AX dedicated servers](https://www.hetzner.com/dedicated-rootserver/matrix-ax/)
