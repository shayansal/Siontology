# Changelog

Notable changes to Siontology, newest first. There are no releases yet:
everything below is on the `main` branch, unreleased. Once releases begin,
they will follow [Semantic Versioning](https://semver.org/). The state
transition specification (`docs/spec/state-transition.md`) has its own
version, given below where it changed.

Commit messages before the rename use the working name, Fold.

## Unreleased

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
