# Crate map

The repository is one Cargo workspace of nineteen crates. Lower crates know
nothing of higher ones, and every crate depends on `sion-core` rather than
on its neighbours where it can (a rule in `AGENTS.md`). The app's GUI shell,
`apps/sion-app/src-tauri`, is a separate Cargo workspace on top of
`sion-app-core`, so that GUI dependencies stay out of the protocol's.

Each row depends only on rows below it (the table lists the exact
dependencies):

```text
sion-app-core
sion-cli
sion-node        sion-agent
sion-prover
sion-spec        sion-light      sion-net
sion-exec        sion-memory
sion-consensus   sion-clearing   sion-assurance   sion-crypto   sion-da   sion-enclave
sion-core        sion-lp         sion-http
```

| Crate | What it holds | Lines | Depends on |
|---|---|---|---|
| `sion-core` | Domain-separated SHA3-256, canonical Borsh encoding, typed identifiers, fixed-point amounts, the RFC 9162 append-only log, the sparse Merkle state tree, seeded randomness for tests | 1,952 | none |
| `sion-crypto` | ML-DSA-44, SLH-DSA-SHA2-128s/128f, ML-KEM-768, dual-origin signatures, account policies, validator identities and signatures, sealed committee envelopes, Shamir sharing | 1,801 | core |
| `sion-assurance` | The Assurance Algebra: atoms, formulas, cost of corruption, decision rule, reservations, challenges, slashing, insurance | 3,780 | core |
| `sion-lp` | Exact rational simplex and linear-time verification of optimality certificates | 1,360 | none |
| `sion-clearing` | Verifiable Clearing: sealed double auctions and assignment markets with certificates, beacon priorities | 1,402 | core, lp |
| `sion-da` | 2D Reed–Solomon over the Goldilocks field, row and column commitments, sampling, reconstruction, fraud proofs | 2,203 | core |
| `sion-consensus` | Linear Lanes (lane validators, batch-signed approvals, certificates), the certified-DAG BFT core, committees and sampling, the beacon, a deterministic network simulator | 4,509 | core |
| `sion-exec` | The state machine: objects, accounts, claims, bonds, charters, agent identities (lineage ids, token buckets, overlays, cohorts), escrow, markets, payment streams, memory log heads, tasks, the validator set, pooled stake, storage deals, beacon records and forced-inclusion obligations; transactions and batched authorization (batches, fee floors, keyless subagents); Pulse execution with chunk traces and witnesses; stateless replay; wallets | 13,750 | core, crypto, assurance, clearing, lp, consensus |
| `sion-spec` | Proof stack B: an independent implementation of the state transition, written from the specification | 10,842 | core, crypto, assurance, clearing, consensus, exec (types only) |
| `sion-prover` | The `ProofStack` trait, stacks A and B, World Proofs, certificate checks | 1,047 | core, assurance, exec, spec, da, consensus |
| `sion-light` | The light client: chain identity, header certificates, epoch-to-epoch committee following, state and claim proofs, availability sampling | 816 | core, crypto, consensus, exec, da |
| `sion-memory` | Memory for agents and robots, off the ledger: memory atoms with space and two kinds of time, single-writer logs, the canonical 4D index with range, subject, link and omission proofs, belief as a fold, sealing and forgetting | 9,156 | core, crypto |
| `sion-net` | Networking between processes: QUIC with TLS 1.3, X25519MLKEM768 and pinned ML-DSA-44 raw public keys, one stream per message class, bounded queues and quotas | 2,958 | core, crypto |
| `sion-http` | Outbound HTTPS for clients: TLS 1.3 or 1.2, with the hybrid X25519MLKEM768 key exchange offered first where TLS 1.3 is spoken, web PKI roots, a resolver that refuses loopback, private and other non-public addresses, proxy support, bounded reads, timeouts, no redirects | 2,476 | none |
| `sion-agent` | The agent runtime: the session loop; models (Anthropic, OpenAI-compatible, Gemini, scripted); tools with effects, autonomy levels, approvals, taint, quarantine and the journal of side effects; budgets, compaction and sessions that pause and resume; workspace, code, shell and web tools, web search, a browser and a desktop; a sandbox (bubblewrap, Docker) and an egress proxy; an MCP client over standard input and output or HTTP, with OAuth, elicitation and sampling; skills in the Agent Skills format, evaluation sets and skill evolution; graphs of agents; schedules; work taken on under mandates | 37,522 | core, http |
| `sion-enclave` | Attestation for agents in enclaves: AMD SEV-SNP reports checked against AMD's chain for Milan, Genoa and Turin, the chip's and firmware's versions matched, debuggable guests refused, the session's keys and an expiry bound in the report; a simulated platform for tests | 1,003 | core |
| `sion-node` | The devnet and `sion node`: validator threads, gateway, prover thread, committees from the state, forced-inclusion bookkeeping, write-ahead log and snapshots, batches checked once and carried once, HTTP API, the transport seam | 11,330 | core, crypto, assurance, clearing, consensus, exec, da, light, net, prover |
| `sion-cli` | The `sion` command: the devnet, nodes and gateways, wallets and every transaction, payment streams, x402, light verification, the MCP server and demos; for the agent, the ledger as tools, profiles, sessions, skills from others and their import, subagents and cohorts, schedules, the HTTP channel, the messengers behind one bridge, voice in the terminal, work from GitHub and chats, sion's own mail, calendar and documents MCP servers, setup, update, running on a server (`sion agent up`, `sion deploy`); renting an agent out, agents' records and the public reads for other frameworks; ACP and AP2; benchmarks | 105,661 | core, crypto, assurance, clearing, consensus, exec, da, light, prover, spec, node, memory, http, agent, enclave; net for tests |
| `sion-app-core` | The app's core: the device's key, a light client that verifies the latest Pulse and the device's own receipts, payments, pooled stake and storage provided from the device ([The app](../getting-started/app.md)) | 1,025 | core, crypto, exec, light, cli |

Lines are lines of Rust under each crate's `src/`, unit tests included, at
the time of writing.

## Where to look

| To understand… | Read |
|---|---|
| How a transaction executes | `sion_exec::execute` (one transaction), `sion_exec::pulse` (a Pulse and its phases) |
| What a validator does | `sion_node::validator` ([The validator thread](validator.md)) |
| How committees are derived | `sion_exec::validators`, `sion_node::committees` |
| How a light client decides | `sion_light::{chain, epochs, header, proofs}`, `sion_cli::verify` |
| The rules both proof stacks implement | `docs/spec/state-transition.md` ([Specification](../specification.md)) |
| What the CLI prints | `sion_cli::describe` |

## Tests

| Where | What |
|---|---|
| Unit tests in every crate | Encodings, hashes, Merkle structures, signatures, sampling, the beacon |
| `sion-assurance`, `sion-lp`, `sion-exec/tests/properties.rs` | Property tests (`proptest`) of the algebra, the certificates and the state machine |
| `sion-exec/tests/scenarios.rs`, `sion-exec/tests/validators.rs` | Scenario tests for every transaction kind, staking, forced inclusion and payment streams |
| `sion-exec/tests/storage.rs`, `sion-exec/tests/pool_slashing.rs` | Beacon records, storage rounds and challenges, proportional forfeits; slashing pooled stake and the reporter's share |
| `sion-exec/tests/buckets.rs` | A property test of the token buckets: random trees of spawns, cohorts, materializations and payments never pay more than σ + ρ·τ over τ + 1 Pulses, and a greedy spender reaches most of that bound |
| `sion-consensus/tests` | The consensus simulator with adversarial network phases; lanes; observers |
| `sion-clearing/tests`, `sion-da/tests` | Auctions and assignments; encoding, sampling, reconstruction |
| `sion-spec/tests/differential.rs` | Stack A against stack B on random workloads, boundary probes and tampering |
| `sion-node/tests/devnet.rs` | End-to-end devnets with dual post-quantum signatures |
| `sion-app-core/tests/devnet.rs` | The app's core against a devnet over HTTP: pinning on first use, a payment with its receipt proven, pooled stake, a wrong pin, following new epochs without starting over, providing storage and proving its rounds |
| `sion-cli/tests/demos.rs` | The demos (`#[ignore]`d: they take minutes) and an MCP session |
| `sion-agent/tests` | Sessions with a scripted model (approvals across a pause, the journal, calls that run again, a turn's calls run together, taint, budgets, fallback, compaction, the kill switch, traces), the providers against stand-ins for their APIs, and MCP servers of both protocol eras (a fake one in Python: pages, pins, hangs, deaths, a sandbox) |
| `sion-cli/tests/agent_work.rs` | Agents working with others on a devnet: a subagent with a budget carved from its parent's cannot spend more; a hired agent delivers and is paid from the poster's escrow; a scheduled job runs and reports to a file; the HTTP channel needs its token; a Telegram bot answers only its chats (against a stand-in for the API) |
| `sion-cli/tests/channels_across.rs` | Channels on a devnet: a profile's tokens reach only it, and only a token made for approvals approves; an agent gateway passes messages to the host that proved the agent's key, refuses a host without it, and stops them once the agent is suspended; Slack through the bridge (against a stand-in) skips the history and bots, and only an approver decides; Discord, Matrix and Signal, against stand-ins, start from the latest message, skip bots and their own messages (Signal, group messages) and answer in the right place |
| `sion-cli/tests/agent_recall.rs` | An agent's past on a devnet: it finds its earlier sessions, an edited one shows as changed, consolidation keeps grounded notes and supersedes a replaced one, and a note from outside content stays out of the prompt |
| `sion-cli/tests/memory_archive.rs` | A memory log archived with a storage provider on a devnet: retrieved only by the deal's client, restored, and refused when altered, stale or another log's |
| `sion-cli/tests/agent_memory.rs` | An agent's memory and skills on a devnet: what it records is committed when its session ends, with an episode; its preferences and skills are in the next prompt; recall is proved complete; a retraction hides an atom; after a long task, it writes down how it did it |
| `sion-cli/tests/agent_tools.rs` | An agent's tools on a devnet: a shell and a skill's script in bubblewrap, an MCP server whose changed tool is not offered, storage deals, and three subagents spawned in one turn whose payments go out in one batch |
| `sion-cli/tests/agent_runtime.rs` | An agent at work on a devnet: payments the charter allows go through, the ledger refuses the rest even at L6, a payment waits for approval at L5 and survives a restart |
| `sion-cli/tests/skill_supply.rs` | Skills from others on a devnet: a release on one bond does not cover the agent's budget and is refused; with an audit it does and is installed; a changed bundle is refused; the agent cannot rewrite the skill; when both bonds fall, the agent is compensated and the skill is taken out of use |
| `sion-cli/tests/skill_improve.rs` | Skills that get better on a devnet: the released version fails a case of its evaluation set; a bounty fixes the set, the cases and the verifiers; an evolution finds a version that passes; only named verifiers attest, and only to a new version; the worker is paid; the next release must name the release it improves and be of the result's bytes, rests on the result and shows it in its lineage |
| `sion-cli/tests/streams.rs` | An x402 server's stream desk on a devnet: refusals, recovery, top-ups and the close-out |
| `sion-cli/tests/agent_takes_work.rs` | Work the agent takes on by itself, on a devnet against a stand-in for GitHub's API: observe and auto modes, mandates on the ledger that its payments name, one question at a time, people's requests first |
| `sion-cli/tests/rentals.rs` | Renting an agent out on a devnet: requests sealed to a simulated enclave, paid by the token through a payment stream, the owner paid what the enclave signed for |
| `sion-cli/tests/commerce.rs`, `commerce_cards.rs` | ACP and AP2 end to end: a merchant's checkout and carts paid on the ledger, with AP2 mandates checked before money moves, state kept across restarts, signed webhooks and request signatures; cards delegated to a PSP or charged through a stand-in for Stripe, with 3-D Secure (the acquirer and the issuer simulated); a catalog published to a Feed API |
| `sion-cli/tests/commerce_properties.rs`, `ap2_vectors.rs`, `interop.rs` | Property tests of canonical JSON, fingerprints, both kinds of signature, recurrence periods and exact amounts; AP2's own example tokens; requests and answers written out for ACP's published JSON Schemas (`scripts/interop`) |

`cargo test --workspace` runs the fast suite, as CI does on every push.
`cargo test --workspace -- --include-ignored` adds the demo runs, as CI
does every night.
