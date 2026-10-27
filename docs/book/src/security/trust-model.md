# Trust model

Every assumption the system relies on without proof is named, as an atom in
some trust formula (rule 5 of `AGENTS.md`). This chapter lists what the
architecture assumes, what this implementation adds, and what a light
client trusts when `sion verify` prints a check mark.

## What the architecture assumes

| Property | Comes from | Assumes |
|---|---|---|
| Validity of the state | The World Proof | The proof systems are sound (two independent stacks: either suffices), the specification is right, SHA3-256 is collision resistant |
| Ordering, finality, liveness | Committees | Fewer than a third of each committee's stake is Byzantine; partial synchrony for liveness |
| Censorship resistance | Forced inclusion, rotation, sampling | At least one honest proposer; a committee that censors can be replaced |
| Data availability | Sampling and erasure coding | Enough clients sample; the data root commits to a correct encoding (bound by the World Proof) |
| Signatures | ML-DSA-44 and SLH-DSA together | At least one of the two families is unbroken |
| Knowledge (claims) | The Assurance Algebra | Nothing: a claim states the atoms it rests on, and each consumer prices them under its own policy |

The trust formula of a dual proof, from the architecture:

```text
specification ∧ sha3-256 ∧ ((stack A ∧ circuit A) ∨ (stack B ∧ circuit B))
```

## What this implementation adds

These atoms are part of the formula of anything the devnet reports, on top
of the architecture's own. The list is copied from the repository's
[implementation status](https://github.com/ColdAI-org/siontology/blob/main/docs/implementation-status.md).

| Atom | What it means |
|---|---|
| `impl:replay-proofs` | World Proofs are checked by re-execution, so "proof finality" costs a verifier as much as executing; succinctness is assumed, not delivered |
| `impl:shared-primitives` | Both proof stacks use the same hashing, Merkle, signature, Assurance Algebra and clearing-certificate code; a bug there is common-mode |
| `impl:devnet-keys` | Devnet validator keys are derived from the chain name; anyone can derive them. A devnet secures nothing of value |
| `impl:in-process-network` | Validators share a machine and a process; network faults are not exercised by the devnet (they are by the consensus simulator) |
| `impl:reliable-handover` | At an epoch boundary each validator starts a new consensus session; a validator that has not reached the boundary finishes the old session from messages already sent, which the in-process network always delivers. A real network needs a drain period |
| `impl:slashed-seat` | A slashed validator keeps its seat, and its weight in quorums, until the epoch ends |
| `impl:evidence-scope` | Equivocation evidence is not bound to this chain: a validator's keys must not be used on another network |
| `impl:lock-handover` | Certificates are not drained when a lane committee changes: certificate finality for owned transactions holds within one committee's term (an owner who double-signs could get the second spend certified by a fully replaced committee) |
| `impl:hash-chain-beacon` | The beacon is a hash-chain commit-reveal; the last validator to reveal can bias it by withholding (at the cost of being seen) |
| `impl:public-amounts` | Amounts and balances are visible to everyone |
| `impl:unaudited-pq-crates` | The FIPS 203/204/205 implementations are unaudited pure-Rust crates |
| `impl:insecure-sim-signatures` | Only when a devnet is started with `--insecure-fast-signatures`: validators sign with a simulation MAC. The chain identity records it, and light-client reports say so |

## What a light client trusts

A client pins one value, the [chain identity](../reference/chain-identity.md),
from a source it trusts. Everything below follows from it. Here is what
each line of `sion verify` establishes, and what it still assumes:

| Line | Establishes | Still assumes |
|---|---|---|
| `chain: … matches the pinned identity` | The node serves the chain you pinned; validator keys derive their identifiers | The pin came from a source you trust |
| `committees: followed from the pinned epoch 0 to epoch …` | Each later committee came from the state, certified by the committee before it | Each of those committees had fewer than a third Byzantine stake |
| `certificate: … of … stake signed (quorum …)` | More than two thirds of the epoch's committee signed this header | Fewer than a third of it is Byzantine (else a conflicting header may exist) |
| `forced inclusion: …` | The committee certified how many forced intents are pending and overdue | The summary is certified, not proven: the World Proof proves the obligations but does not check the summary against them |
| `receipts: …` | The node's transactions are the certified order, and each receipt is in the receipts root | As for the certificate |
| `state: …` | An object or claim is in the certified state root | As for the certificate, until the World Proof is checked |
| `availability: … random shares … verified` | Withholding enough data to prevent reconstruction would have been caught with the stated probability | Your samples are private and random |
| `world proof: … re-verified by stack ["A", "B"]` | The transition from the previous state is valid, the data is what executed | `impl:replay-proofs`, `impl:shared-primitives`, the specification |
| `lane certificates: …` | Every executed transaction locked its inputs with a quorum of its shard's committee, from the proven validator set | As for the certificate |

Without `--chain`, the first line reads `NOT PINNED: learned from the node
itself`, and every other line is only as good as the node.

## What the devnet does not protect

A devnet runs every validator in one process with keys anyone can derive.
Whoever runs it can sign anything as any validator. It demonstrates the
protocol; it secures nothing.
