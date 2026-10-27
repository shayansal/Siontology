# Implementation plan

How the architecture document ("Neo-Blockchains: The Fold Architecture")
maps onto crates. Each crate depends on `fold-core` and as few siblings as
possible. What is built, simplified, simulated or missing is tracked in
[`implementation-status.md`](implementation-status.md).

| Crate | Architecture section | Responsibility |
|---|---|---|
| `fold-core` | Foundations | Domain-separated SHA3-256, canonical Borsh encoding, newtype IDs, `Amount`/`Cost`, RFC 9162 append log (knowledge root, consistency proofs), sparse Merkle tree (state root, stateless updates) |
| `fold-crypto` | Identity layer | ML-DSA-44 (FIPS 204), SLH-DSA-SHA2-128s/128f (FIPS 205), ML-KEM-768 (FIPS 203), dual-origin signatures, account authorization policies with stable IDs, sealed envelopes (ML-KEM + ChaCha20-Poly1305 + Shamir) |
| `fold-assurance` | Knowledge layer | Atoms, trust formulas over claim DAGs, policy-relative cost-of-corruption interval, exact solver for shared atoms, reservations, decision rule, slashing and compensation, retraction propagation, failure probabilities for insurance |
| `fold-lp` | Verifiable Clearing | Exact rational two-phase simplex returning primal and dual; linear-time certificate verifier |
| `fold-clearing` | Verifiable Clearing | Sealed double auctions with canonical tie-breaking and a price band, assignment markets, settlement instructions, certificate checks |
| `fold-da` | Data availability | 2D Reed–Solomon over the Goldilocks field, row/column Merkle commitments, sampling, reconstruction, detection probability, bad-encoding fraud proofs |
| `fold-consensus` | Ordering | Owned-object lane certificates with batch-root voting, certified-DAG BFT (Bullshark-style) for contention domains, beacon, committee sampling, forced inclusion, deterministic network simulator |
| `fold-exec` | Foundations, Identity, Knowledge, Ordering | The state machine: sorted objects, accounts, transactions and their coordination classes, claims and bonds, Agent Charters, escrow allotments, markets, state and knowledge roots, Pulse execution in three phases with chunk statements, traces and witnesses, stateless chunk replay |
| `fold-spec` | Execution and proving | A second implementation of the state transition, written only from `docs/spec/state-transition.md`, used as proof stack B (N-version proving) |
| `fold-prover` | Execution and proving, the Fold | Proof-stack interface, stack A (replay with the reference executor), World Proofs that fold every chunk and bind receipts, published data and lane certificates |
| `fold-light` | The Fold, DA | Light client: chain identity, header certificates, state, knowledge and receipt proofs, DA sampling |
| `fold-node` | All | Devnet orchestrator: validators, gateway, prover; WAL persistence and snapshots; HTTP API |
| `fold-cli` | Interfaces | `fold` command line, demos, light verification, MCP server, benchmarks |

## Decisions that differ from the first plan

- **Global execution instead of cross-shard messages.** Shards exist for
  lane certification only: each shard's committee locks the owned inputs of
  its accounts' transactions. Every validator executes every Pulse in full,
  so there are no cross-shard messages and no cross-shard atomicity
  protocol. Execution cost per Pulse is what bounds throughput; proving,
  not sharded execution, is how the architecture scales it.
- **Every transaction is lane-certified.** Contended transactions also lock
  their owned inputs (their gas coin, for example) with a lane certificate
  before consensus orders them, so a consensus-ordered spend can never
  conflict with a certified one.
- **Two proof stacks are two implementations, not two provers.** Stack A
  replays chunks with the reference executor; stack B replays them with an
  independent implementation of the specification. Both are replay proofs
  (stateless, not succinct). Succinct STARK stacks plug into the same
  `ProofStack` trait.
- **The chain identity covers the validators.** Pulse headers carry a
  digest of the genesis specification, the validators with their keys and
  stake, and the shard layout; light clients pin it.

## Not yet built

Personhood credentials, the proving market, staking and reconfiguration,
confidential amounts and compliance proofs, forced inclusion in the
running node, and the x402, AP2, ERC-8004 and C2PA adapters. See
[`implementation-status.md`](implementation-status.md).
