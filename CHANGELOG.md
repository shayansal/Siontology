# Changelog

Notable changes to Siontology, newest first. There are no releases yet:
everything below is on the default branch, unreleased. Once releases begin,
they will follow [Semantic Versioning](https://semver.org/). The state
transition specification (`docs/spec/state-transition.md`) has its own
version, given below where it changed.

Commit messages before the rename use the working name, Fold.

## Unreleased

### An agent runtime

- New crate `sion-agent`: runs a language model as an agent. Models: the
  Anthropic Messages API (version 2026-10-21, automatic prompt caching),
  any OpenAI-compatible API, and a scripted model; a busy model cools
  down for 30 seconds while the next is asked. Tools declare an effect
  (read, write, external, spend); autonomy levels L0 to L6 decide what
  runs, what waits for a person's approval (valid 15 minutes) and what is
  refused. Content from outside taints a session for 24 hours, during
  which external calls and spending need approval. A journal returns the
  recorded outcome of a repeated call and never repeats one whose outcome
  is unknown. Runs are bounded (12 turns, 4 more per turn of progress, at
  most 32), compacted when long, stopped by a kill switch, and pause and
  resume from disk. Sessions export as chat-format trajectories.
- `sion agent run | continue | sessions | trace | kill-switch`: an agent
  identity at work, with the ledger as tools (`ledger_balance`,
  `ledger_pay` under its charter, `ledger_transaction`), files in a
  workspace and public web pages. The ledger refuses a payment outside
  the charter whatever the runtime's level. New trust atoms
  `agent:local-policy`, `agent:provider-sees-context` and
  `agent:plain-sessions`. Guide: [Agents at
  work](docs/book/src/guides/agent-runtime.md).
- Memory: `sion agent run --memory <log>|new` gives the agent its memory
  log and the tools `memory_remember`, `memory_recall` (proved complete
  against the ledger for a subject and a relation), `memory_search`,
  `memory_forget` (a retraction; nothing is deleted) and `memory_commit`.
  Its preferences, goals and procedures are in every prompt, and each
  finished session is recorded as an episode with its trajectory's digest.
- Skills in the Agent Skills format (`sion_agent::skills`): the agent's
  skills are listed in every prompt, read on use (`skill_read`,
  `skill_file`), and written or improved by the agent (`skill_write`),
  which keeps the versions it replaces; a skill's bundle is a canonical
  encoding of its files.
- Skills from others: `sion skill pack | audit | publish | show |
  install`. A release (`skill/v1`) is a claim on the publisher's bond or
  on audits of the same bundle (`skill.audit/v1`, each on its auditor's
  bond), which add up. Installing checks the bundle against the release's
  root and accepts the release with the agent's remaining budget at risk;
  a release whose backing does not cover it is refused before anything is
  signed. An installed skill cannot be rewritten by the agent, and one
  whose release is falsified is taken out of use before the next run;
  the agents that relied on it are compensated from the slashed bonds.
  New trust atom `agent:local-skill-checks`. Guide: [Skills from
  others](docs/book/src/guides/skills.md).
- Skills that get better: evaluation sets (`sion_agent::evals`: cases
  with stand-in tools and checks, fixed by the digest of their bytes) and
  evolution (`sion_agent::evolve`: a mutator model proposes variants for
  the cases a version fails; gates for purpose, size, novelty and
  regressions; a Pareto front). `sion skill eval` (with trajectories
  labelled by result), `evolve` (the best version kept as a draft),
  `vouch` (an evaluation set's author stands behind it on a bond,
  `skill.evals/v1`), `bounty` (a Task Charter for a better version, its
  spec, verifiers and required claims fixed first), `verify` (a named
  verifier runs the set and attests), `submit` (result and settlement),
  `publish --evidence` (the release rests on the settled result and
  records it) and `lineage`. New trust atom `agent:eval-runs`. Guide:
  [Skills that get better](docs/book/src/guides/skill-improvement.md).
- After a finished task with at least five successful tool calls and no
  skill written, `sion agent run` asks the agent whether to write down how
  it did it, with only its skill tools (`--no-reflect` to skip).
- Scripted models: a step can play only `when` the conversation contains
  a text, or `unless` it does; a resumed session's script goes on from the
  step it reached.

### Fixes from the review of storage deals, pooled stake and the app

An independent review of storage deals and the app, and a review of the
staking incentives, found defects that were fixed before release ([Reviews and
fixes](docs/book/src/security/reviews.md)):

- **Storage challenges** (spec v1.12): a round's challenge now comes from
  the beacon of its first Pulse, fixed at acceptance, instead of the beacon
  of the previous proof's Pulse, which a provider working with a validator
  could time. The deferred phase records the beacon of every eighth Pulse
  in one of 12,500 beacon records (`sion_exec::beacons`); rounds start and
  last multiples of 8 Pulses; the longest round is 100,000 Pulses. Missed
  rounds forfeit `⌊bond × missed / rounds⌋`; open deals must ask for
  collateral.
- **The reporter's share** (spec v1.11) is of the offender's own stake and
  rewards only: a share of the pool's slashed tenth made reporting
  yourself profitable once your pool was about 190 times your stake.
- **Proof stack B** implements storage deals and beacon records; the
  storage differential tests run again.
- **The provider** proves first and fetches in the background with
  deadlines; refuses `file://` locators unless allowed and private
  addresses always; batches proofs by size; takes modest deals by default;
  writes copies only after acceptance and deletes them at the close. A
  refused submission releases its coins. New crate `sion-http`: bounded,
  guarded outbound HTTP with X25519MLKEM768 first.
- **The app** shows outcomes from proven receipts only, waits for the
  digest it computed, pays 64-digit accounts only, writes settings
  atomically and pins nothing on first use after a reset; test money is
  labelled test USD, and the wording is neutral.
- `GET /v1/deals?open=1`, from an index; `sion storage post` needs
  `--locator` or `--local`.

### Storage deals (specification v1.12)

- `PostDeal` escrows `price × rounds` for keeping data whose Merkle root
  the deal fixes (leaves of at most 4 KiB, RFC 9162 tree), for a named
  provider or for any (then with collateral). `AcceptDeal` posts the
  provider's collateral and creates a custody record the provider owns;
  rounds start on the next multiple of 8 Pulses. `ProveStorage` proves the
  round under way for many deals at once, owned-linear: for each, the
  leaves the beacon of the round's first Pulse picks, with their Merkle
  paths. `SettleDeal` pays the provider for proven rounds; `CloseDeal`
  refunds unproven rounds to the client, returns the collateral less
  `⌊bond × missed / rounds⌋`, or forfeits all of it to the treasury when
  the client closes a deal its provider abandoned for three rounds. The
  supply counts deal escrow and collateral. Both proof stacks implement it.
- `sion storage prepare | post | deals | show | accept | prove | settle |
  close | serve`; `serve` runs a provider that proves every round, collects
  and closes, and accepts deals whose data it fetched (`https://`, or
  `file://` when allowed) and checked against the root. MCP tools
  `sion_storage_post`, `sion_storage_deals` and `sion_storage_close`.
  `GET /v1/deals`; account views list custody records.
- The `storage` demo: a robot commits its memory log with its archive's
  root as the manifest and pays a laptop to keep the archive; a phone that
  kept only half of the camera footage it accepted cannot answer the
  challenges and forfeits its collateral.
- The app's Storage tab turns a device into a provider: it takes open deals
  on modest terms whose data it can fetch, keeps the data, proves every
  round (each proof expiring at the end of its round, so a late one costs
  nothing), collects and closes; turned off, it keeps proving the deals it
  holds.

### The app: a light node on every device

- New crate `sion-app-core`, shared by the app's desktop and mobile
  shells: the device's key, a light client that verifies the latest
  certified Pulse every eight seconds (chain identity pinned on first use
  or by its user, committee certificate, receipts, a state proof, 16 random
  data shares), receipts proven for every transaction the device signs,
  payments and pooled stake, over any node's HTTP API. `Send + Sync`, and
  no lock held while it talks to a node.
- A Tauri 2 shell in `apps/sion-app` (its own Cargo workspace) with four
  tabs (Node, Wallet, Stake, Settings) in the brand's type and colours.
  Built and run on Linux against a devnet; Android and iOS are configured
  but not built yet. See the [app page](docs/book/src/getting-started/app.md).
- `sion_cli::verify::verify_pulse_report` returns the chain identity and
  Pulse a verification checked, so a client pinning on first use pins the
  identity it verified against.

### Networking between machines

- Validators run one per process and talk QUIC: `sion keys validator`
  generates a validator's keys and a network record signed with both of its
  consensus keys, `sion genesis` collects records into a genesis file and a
  peer file, `sion node` runs a validator (with an optional embedded
  gateway) and `sion gateway` a gateway on its own. TLS 1.3 with
  X25519MLKEM768 only and ML-DSA-44 raw public keys only, pinned at both
  ends; one stream per message class, bounded queues, quotas (new crate
  `sion-net`, on quinn 0.11.9 and rustls with aws-lc-rs). Tested with four
  node processes and a gateway on one machine. A stopped validator cannot
  rejoin yet (`net:no-rejoin`); see the
  [guide](docs/book/src/guides/network.md).
- Every message between nodes now goes through a `Transport` seam that
  stamps the sender: the devnet's in-memory transport carries exactly what
  the old bus did, a seeded fault injector adds loss, delay, duplication,
  reordering and partitions per message class, and status and `sion bench`
  report traffic per class, kind and peer.
- Lost messages are asked for again: gateways retry late lane approvals,
  validators pull missing Pulse signatures, and the buffers peers can fill
  are bounded in bytes or count.

### Memory for agents and robots: the library, commands and demo

- New crate `sion-memory`, the off-ledger half of memory logs: integer-only
  memory atoms with space and two kinds of time, single-writer logs with an
  on-disk store, a canonical 4D index (hourly Merkle R-trees per frame, a
  long-lived tree, subject and link maps) specified for independent
  rebuilding, complete range proofs including empty answers, subject and
  link proofs, omission proofs that convict an index leaving an atom out,
  belief as a fold with proofs checked from heads, sealed payloads with
  forgetting by key destruction, and the Reality Memory Protocol. 84
  tests, including a second implementation of the index written from its
  specification. Over 10⁶ atoms, a 50 × 50 × 10 m, one-hour query returns
  9 atoms with a 36 KiB proof, verified in 0.2 ms.
- `sion memory open`, `remember`, `commit`, `recall` and `show`, and the
  MCP tools `sion_memory_open`, `sion_memory_remember`,
  `sion_memory_commit` and `sion_memory_recall`: recall answers are
  checked against the index root the ledger holds.
- The `robots` demo: two robots with their own agent identities and memory
  logs share a warehouse; a rewrite is refused; a curator's refutation
  gives two readers two provable beliefs; an omitting index is convicted; a
  camera frame is forgotten; a suspended robot's memory stops growing.

### Pooled stake (specification v1.11)

- `DelegateStake` adds stake to a validator's pool without handing it
  over: the delegator keeps a delegation object holding pool shares. It
  joins at the epoch's end, then counts towards the validator's weight and
  earns a share of its rewards. `Undelegate` pays what is owed and starts
  unbonding (the stake stays slashable), `WithdrawDelegation` returns the
  stake, and `ClaimDelegatorRewards` pays the rewards credited so far.
- At each epoch's end a validator's reward is split between its own stake
  and its pool's earning stake; the operator keeps a commission on the
  pool's part, set with `SetCommission`: at most 25 %, effective at the
  epoch's end, and, once anyone has delegated, raised by at most 2 points
  at a time. The rest is credited per share with an exact accumulator.
- A delegation is refused if it would give its validator more than twice
  an equal share of the bonded stake (never less than a tenth).
- An equivocation also slashes a tenth of the offender's pool, unbonding
  delegations included; the reporter's share is of the offender's own
  stake only (as first written, of both). Validators rank
  for the active set by their own plus their pool's earning stake.
- Both proof stacks implement v1.11. The differential tests run every rule
  and failure through both, check every epoch end's split against a third,
  plain-arithmetic reading, and run randomized workloads of delegations,
  claims, commission changes, fees and slashings, checking the pools
  against the delegations they hold after every Pulse.
- New `sion stake` commands (`pools`, `delegate`, `show`, `claim`,
  `undelegate`, `withdraw`, `commission`), MCP tools `sion_stake_pools`,
  `sion_stake_delegate`, `sion_stake_show`, `sion_stake_claim`,
  `sion_stake_undelegate` and `sion_stake_withdraw`, and the guide
  *Delegating stake*. The node's account view lists an account's
  delegations. The supply invariant counts pooled stake and unclaimed
  delegator rewards.

### Task Charters (specification v1.10)

- `PostTask` escrows money for a task and fixes, at posting, the rule a
  result must pass: the poster's policy, the claims every result must rest
  on (an environment's bonded "no known exploit" claim, say), the
  verifiers whose attestations count, the adjudicator, the largest
  detection factor and how long the poster relies on a paid result. An
  agent can post from its own budget, under its caveats.
- `SettleTask` pays the worker once, when a result claim that commits to
  the task, with exactly the formula the task requires (the worker's bond
  or a named verifier's attestation of the same output, and every required
  claim), passes the decision rule. It reserves κ times the payment of the
  result's backing for whoever paid, so if a required claim is later
  falsified (an exploit in the environment), that party is compensated
  from the slashed bonds and the challenger is paid a share.
- `CancelTask` refunds a task its worker releases, a bounty its poster
  withdraws, or one past its deadline. A named worker can count on its
  task until the deadline.
- Both proof stacks implement v1.10; the differential tests run every rule
  and failure, posting from an agent's budget, bounties, cancellation and
  an exploit's compensation. New `sion task` commands (`post`, `attest`,
  `submit`, `settle`, `cancel`, `show`), MCP tools `sion_task_post`,
  `sion_task_attest`, `sion_task_submit`, `sion_task_settle` and
  `sion_task_show`, and the `tasks` demo.
- The supply invariant counts open task escrow.

### Memory logs (specification v1.9)

- A memory log is a log of memory atoms that only its writer appends to,
  kept off the ledger. `OpenMemory` creates one, owned by the writer,
  optionally for an agent identity; `CommitMemory` moves its head to a new
  size and root along an RFC 9162 consistency proof that the ledger checks,
  so a log only grows and cannot fork. The Pulse of a commit is the
  transaction time of the atoms it adds.
- An agent's log stops growing while the agent, or any agent above it, is
  suspended or revoked.
- Both proof stacks implement v1.9; the differential tests run every rule
  and its failure cases, and a property test runs random commits, honest
  and forged, by two writers through both.
- The design note "Billions of agents" covers the memory layer's atoms,
  4D index, completeness proofs, belief and privacy.

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
