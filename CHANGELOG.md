# Changelog

Notable changes to Siontology, newest first. There are no releases yet:
everything below is on the default branch, unreleased. Once releases begin,
they will follow [Semantic Versioning](https://semver.org/). The state
transition specification (`docs/spec/state-transition.md`) has its own
version, given below where it changed.

Commit messages before the rename use the working name, Fold.

## Unreleased

### Agent identities (specification v1.8)

- An agent identity is a charter with an identity record: a commitment to
  the agent's card and its depth in the tree. The charter's id is the
  agent's id (`did:sion:<chain>:<id>`).
- `RegisterAgent` registers one. `SpawnAgents` launches up to 256
  subagents in one transaction, each with its own account, card, caveats
  at least as strict as its parent's, and a budget carved from the parent's
  allotment. `SetAgentCard` replaces a card.
- `SuspendAgent` and `ResumeAgent` pause and resume a charter and
  everything below it; any overseer (the backer, the principal or an
  ancestor's agent) may suspend, the suspender or the backer may resume.
  `RevokeCharter` may now be sent by the agent itself or any overseer.
- Window limits are carved like budgets, for `Delegate` too: allotments
  gain `carved_window`, and an agent's own limit is what it has not handed
  down. Until v1.7 each child could spend its parent's whole window limit,
  so many children multiplied the parent's rate. An independent design
  review found this.
- Both proof stacks implement v1.8; the differential tests run every
  operation and its failure cases through both. New `sion agent` commands
  (`register`, `spawn`, `show`, `card`, `card-template`, `suspend`,
  `resume`, `revoke`, `reclaim`) and the `swarm` demo.
- The wallet now puts an operation's extra inputs (an allotment, the
  account object) right after the gas coin, so `inputs[1]` holds them
  however many coins pay the fee.

### Continuous integration

- Pull requests and pushes run the fast suite: formatting, lints, the tests
  not marked `#[ignore]`, the independence check, and now also a check that
  the generated website pages match their sources and a build of the book.
- The demo runs (about three minutes on two cores) are `#[ignore]`d and run
  every night with every other test (`cargo test --workspace --
  --include-ignored`), next to the quick benchmarks, whose JSON is kept for
  90 days.
- The website deploys from `master` as well as `main`.

### Node fixes found by the networking review

- The gateway handed consensus every certificate of a 20 ms flush as one
  group, capped at 512 transactions but not in bytes. Consensus drops an
  item above `max_payload_bytes` (512 KiB) without a reply, and about 20
  dual-signed transfers fill it, so a burst never executed: the gateway
  re-sent the same oversized group every 4 s until it expired. Groups are
  now split by encoded size, at most half the payload limit
  (`PayloadItem::groups`), in both the flush and the re-send path. A
  transaction whose certificate cannot fit even alone is refused with a
  reason instead of retried forever. A devnet test sends 40 dual-signed
  transfers at once under a 128 KiB limit; it fails without the fix.
- Beacon reveals committed ahead of their Pulse lived only in the Pulse
  builder, so a validator restarting alone would compute a different beacon
  for up to 8 Pulses. Log records and snapshots now carry them with the
  beacon (storage format 4, `SIONWAL4` and `SIONSNP4`), taken as of each
  Pulse's own close: one commit can close two Pulses, and the first one's
  record used to hold the beacon after the second. A directory in the older
  format is refused with a clear error instead of being truncated as a torn
  tail. Pulse headers do not yet commit to the pending reveals; that needs a
  specification version.

### Payment streams (specification v1.6 and v1.7)

- Payment streams: a payer escrows funds for one payee and pays it off the
  ledger by revealing the elements of a SHA3-256 hash chain, one per
  payment. The payee checks a payment with one hash and settles many in one
  transaction (`OpenStream`, `SettleStream`, `TopUpStream`,
  `CloseStream`, §5.31 to §5.34). A settlement walks from the head, one
  hash per step, at most 65,536 steps, and pays 2 micro-USD per hash.
- Specification v1.7, checkpoints: a payer may commit to a Merkle tree over
  every `spacing`-th chain element, and the payee may settle by a jump to
  the checkpoint below its position, for at most `spacing + 21` hashes
  however many payments the settlement covers.
- Both proof stacks implement the rules; differential tests, a randomized
  property test and executor scenarios cover them.
- x402 gains a `stream` scheme: a request is paid with the next chain
  element, checked by the server with one hash, and settled in batches by
  the server's stream desk. A refused payment names the server's position
  so an honest client resynchronizes. The x402 client reuses HTTP
  connections.
- New commands `sion stream open|pay|jump|check|settle|top-up|close|show`;
  `sion x402 serve --streams` and `sion x402 fetch --stream --count`; the
  `streams` demo; the `streams` benchmark group.
- Measured on one core of a 2-core sandbox: a payee checks a payment in
  0.42 µs; settlements by checkpoint covering about 9 × 10⁸ payments a
  second executed with every signature checked. See the research note
  [Billions of payments a second](docs/book/src/research/billion-scale.md).
- New research notes in the book: the billion-payment note, a plan for
  succinct validity proofs and a plan for networking between machines.

### Review of the validator machinery (specification v1.5)

- An independent review of staking, rewards, slashing and committees
  found that an honest validator could be slashed: the node released a
  rejected transaction's lane locks at once, although its inputs kept
  their versions, and its next approval of those inputs was equivocation
  evidence until the first transaction expired. A rejected transaction now
  keeps its locks until it expires.
- Specification v1.5: slashed validators lose their seats at the epoch
  boundary even if nobody else is eligible. A chain whose every eligible
  validator was slashed halts instead of running on them.
- The simulator's default validator parameters match the devnet: 70 % of
  fees to validators, 5 % of a slashed stake to the reporter.
- New trust atom `impl:active-floor`: the active set can shrink to a single
  validator.

### Validators, staking and epochs (specification v1.3 and v1.4)

- The validator set is part of the state. A validator joins by staking,
  with a proof that it holds its keys; it leaves, and withdraws its stake
  after an unbonding period.
- Epochs. At each epoch's end, joining validators become active, leaving
  validators start unbonding, and the next active set is chosen by stake.
  Consensus is weighted by stake, and lane committees are sampled from the
  active set with the beacon. The World Proof covers every committee
  change.
- Light clients follow the committee from the genesis they pinned, with
  one boundary proof per epoch (`GET /v1/epochs/{e}`, `sion verify`).
- Equivocation, two consensus headers for one round or two live approvals
  of one object version, is slashed on signed evidence. The validator
  forfeits its stake and unclaimed rewards; the reporter gets a share (5 %
  on the devnet).
- Rewards come from fees only (v1.4). Fees are credited to the treasury,
  which keeps a running total of them. At each epoch's end, a share of the
  epoch's fees (70 % on the devnet) is paid to the epoch's validators by
  weight. Slashed stake and released bond pools go to the treasury and are
  never paid out as rewards.
- Forced inclusion answers censorship. A forced intent becomes an
  obligation of its sender's shard; if it could still execute at its
  deadline, it is overdue and the shard's committee is replaced by
  validators from outside it.
- Devnet: `--standby`, `--epoch-length` and `--unbonding`;
  `GET /v1/validators`; `sion demo staking`.

### Renamed to Siontology

- Crates are `sion-*`, the binary is `sion`, and the environment variables
  are `SION_NODE`, `SION_HOME` (default `~/.sion`) and `SION_CHAIN`.
- Hash domain tags start with `sion/`, so digests, identifiers and roots
  differ from earlier builds. The default devnet chain id is
  `sion-devnet-1`, the x402 network is `sion:<chain>`, and the MCP tools
  are named `sion_*`.
- "The Fold" remains the name of the proof-folding mechanism.

### Licence, documentation and community

- Licensed under Apache-2.0 or MIT, at your option (`LICENSE-APACHE`,
  `LICENSE-MIT`).
- The documentation book (`docs/book`), published at
  [siontology.org/docs](https://siontology.org/docs/).
- Contributing guide, security policy, code of conduct, governance,
  trademark policy, citation file, and issue and pull request templates.

## 2026-10-28

- **Validator messages are bound to the chain** (`de39a21`). Lane batches
  and consensus headers name the chain identity; certificates, approvals
  and equivocation evidence from another chain never count, even when its
  validators reuse the same keys.
- **x402** (`de39a21`). An HTTP API can charge per request with HTTP 402
  and settle on the chain (x402 version 2, `exact` scheme). `sion x402
  serve|fetch`, the MCP tool `sion_fetch_paid` and `sion demo x402`.
- **Forced inclusion** (`b0f5c55`). A user whose transactions are ignored
  posts them straight to consensus (`POST /v1/forced`, `sion send
  --forced`). Pulse headers commit to the pending and overdue counts, so
  censorship by more than a third of a committee is visible to light
  clients. `sion demo censorship`.
- **Specification v1.2** (`b0f5c55`), from two independent reviews.
  Transaction signatures are bound to the chain configuration; inputs,
  allotments, orders per batch, formula depth and certificate values are
  bounded; reservation splits are exact. The node and the light client
  received fixes from the same reviews.
- **Proof stack B and specification v1.1** (`0bbf3ef`). `sion-spec`
  implements the state transition from the specification alone. Every
  World Proof carries leaf proofs from both stacks and verifies only if
  both accept every chunk; `scripts/check-independence.sh` checks that
  stack B never calls the reference executor. The differential tests found
  defects in the reference executor, fixed by v1.1. Transactions and
  receipts read as sentences in the CLI.

## 2026-10-27

- **Chain identity** (`4bc64bb`). Pulse headers carry a digest of the
  genesis specification, the validators and the shard layout. A light
  client pins it and rebuilds every committee itself. Fixed: approvals
  from the second shard were attached to the first shard's batches.
- **Certified inputs and a binding World Proof** (`b4c17cf`). Every
  transaction, contended ones included, locks its owned inputs with a
  lane certificate. The World Proof carries the Pulse's published data and
  its lane certificates and checks both. `sion bench`; CI.
- **The state transition specification, v1.0** (`c12fc18`). Receipts
  commit to an outcome code; a dead charter's allotments can be reclaimed.
- **The CLI** (`0af09fe`): a devnet, wallets, every transaction kind over
  HTTP, light-client verification, the MCP server for agents, and the
  first scripted demos.
- **The devnet node, light client and prover** (`1431cd3`). Validators on
  their own threads with their own state; lane committees, certified-DAG
  consensus, Pulses, data availability, a write-ahead log with snapshots,
  and the HTTP API. The prover folds per-chunk proofs from each proof stack
  into a World Proof per Pulse.
- **Execution** (`9046b14`): the two-sorted state machine, coordination
  classes, the Pulse's phases, and stateless re-execution of every chunk
  from its witness.
- **Consensus** (`398cafb`): Linear Lanes, certified-DAG BFT, committees,
  the beacon, forced inclusion and a simulator.
- **Assurance, clearing and data availability** (`7fe243b`): the Assurance
  Algebra with cost-of-corruption intervals, an exact linear-programming
  solver, Verifiable Clearing, and 2D Reed-Solomon data availability with
  sampling.
- **Foundations** (`58fe022`): canonical hashing, Merkle structures and
  post-quantum identities.
