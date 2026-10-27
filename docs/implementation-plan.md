# Implementation plan

How the architecture document ("Neo-Blockchains: The Fold Architecture")
maps onto crates. Each crate depends on `fold-core` and as few siblings as
possible.

| Crate | Architecture section | Responsibility |
|---|---|---|
| `fold-core` | Foundations | Domain-separated SHA3-256, canonical Borsh encoding, newtype IDs, `Amount`/`Cost`, RFC 9162 append log (knowledge root, consistency proofs), sparse Merkle tree (state root, stateless updates) |
| `fold-crypto` | Identity layer | ML-DSA-44 (FIPS 204), SLH-DSA-SHA2-128s/128f (FIPS 205), ML-KEM-768 (FIPS 203), dual-origin signatures, account authorization policies with stable IDs, sealed envelopes (ML-KEM + ChaCha20-Poly1305 + Shamir) |
| `fold-assurance` | Knowledge layer | Atoms, trust formulas over claim DAGs, policy-relative cost-of-corruption interval, exact solver for shared atoms, reservations, decision rule, slashing and compensation, retraction propagation |
| `fold-lp` | Verifiable Clearing | Exact rational two-phase simplex returning primal and dual; linear-time certificate verifier |
| `fold-clearing` | Verifiable Clearing | Sealed double auctions with canonical tie-breaking and a price band, assignment markets, settlement instructions, certificate checks |
| `fold-da` | Data availability | 2D Reed-Solomon over the Goldilocks field, row/column Merkle commitments, sampling, reconstruction, detection probability |
| `fold-consensus` | Ordering | Owned-object lane certificates with batch-root voting, certified-DAG BFT (Bullshark-style) for contention domains, beacon, committee sampling, forced inclusion, deterministic network simulator |
| `fold-exec` | Foundations, Identity, Knowledge, Ordering | The state machine: sorted objects, accounts, transactions, coordination classes, claims and bonds, Agent Charters, escrow allotments, markets, cross-shard messages, state and knowledge roots, Pulse witnesses |
| `fold-prover` | Execution and proving, the Fold | Proof-stack interface, two independent re-execution stacks, proving market, aggregation tree, World Proof pairs |
| `fold-light` | The Fold, DA | Light client: World Proof checks, state and knowledge proofs, DA sampling |
| `fold-node` | All | Devnet orchestrator, node roles, WAL persistence, HTTP API |
| `fold-cli` | Interfaces | `fold` command line and MCP server |

## Simplifications in the reference implementation

These are deliberate and named, per rule 5 in `AGENTS.md`; the full list
lives in `docs/implementation-status.md`.

- Proofs: the two proof stacks re-execute from witnesses (stateless but not
  succinct). Succinct STARK stacks plug into the same `ProofStack` trait.
- Privacy: payloads can be encrypted to viewing keys, but transfer amounts
  are public; zero-knowledge transfer proofs need a succinct stack.
- Personhood: issuer-signed credentials with per-application nullifiers;
  unlinkability needs zero-knowledge proofs.
- Sealed batches: intents are encrypted to the committee with ML-KEM and
  Shamir sharing (t-of-n trust, post-quantum); this is practical for small
  committees only.
- Consensus: a certified-DAG protocol in the Bullshark family; the
  architecture's production target is an uncertified Mysticeti-class DAG.
