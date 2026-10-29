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
| `impl:in-process-network` | `sion devnet` only: validators share a machine and a process; network faults are exercised only through the in-process fault injector and the consensus simulator. A network of `sion node` processes does not carry this atom |
| `impl:reliable-handover` | At an epoch boundary each validator starts a new consensus session; a validator that has not reached the boundary finishes the old session from messages already sent, which the in-process network always delivers. A real network needs a drain period: on a network of processes, a validator that misses old-session messages at a boundary can stall |
| `net:transport-ml-dsa` | Networks: transport authentication rests on ML-DSA-44 alone (each node's network key, presented as an RFC 7250 raw public key and pinned at both ends). A forger could impersonate a peer at the transport layer (delay, drop, eclipse), not sign consensus messages, which carry dual signatures |
| `net:aws-lc` | Networks: the TLS provider is aws-lc-rs, C code outside the workspace's audit scope (the workspace's own post-quantum code is pure Rust) |
| `net:raw-public-keys` | Networks: TLS authenticates nodes with RFC 7250 raw public keys (an ML-DSA-44 SubjectPublicKeyInfo, signed for in the handshake's CertificateVerify), not the self-signed ML-DSA X.509 certificates the design proposed, and with no channel-binding fallback. Both ends pin the exact key, so no certificate path is validated; the handshake relies on rustls's raw-public-key support, newer and less used than its certificate path |
| `net:static-peers` | Networks: peers come from a genesis file and a peer file of signed records, read when a node starts, not from the state. A validator that joins by staking is reachable only by nodes started with its record in their peer file |
| `net:partial-synchrony` | Networks: liveness needs message delays to become bounded after some time (assumed by the architecture; named because a real network now exercises it) |
| `net:gateway-trusts-node` | Networks: a gateway running alone learns committees and answers state queries from one node it trusts. A lying node can make it ask the wrong validators or show wrong balances; it cannot get a transaction certified that a committee did not approve |
| `net:no-rejoin` | Networks: a validator that stops cannot rejoin. Restarting in a session it signed in needs the consensus signing log, and catching up to a later session needs Pulse sync; neither is built, so it refuses to start rather than risk equivocation. Each stopped validator is a permanent fault; more than a third of the stake stopped halts the chain |
| `impl:slashed-seat` | A slashed validator keeps its seat, and its weight in quorums, until the epoch ends |
| `impl:active-floor` | Validators can leave until one remains: nothing keeps the active set large enough to tolerate a faulty member, so the operators of the other validators can leave a single validator forming every quorum |
| `impl:evidence-scope` | Equivocation evidence is not bound to this chain: a validator's keys must not be used on another network |
| `impl:lock-handover` | Certificates are not drained when a lane committee changes: certificate finality for owned transactions holds within one committee's term (an owner who double-signs could get the second spend certified by a fully replaced committee) |
| `impl:hash-chain-beacon` | The beacon is a hash-chain commit-reveal; the last validator to reveal can bias it by withholding (at the cost of being seen) |
| `impl:public-amounts` | Amounts and balances are visible to everyone |
| `impl:memory-index` | The ledger stores a memory log's index root without checking it: a memory query proof is complete relative to that root, and nothing on the ledger yet acts on evidence that an index leaves an atom out |
| `impl:unaudited-pq-crates` | The FIPS 203/204/205 implementations are unaudited pure-Rust crates |
| `storage:no-replication` | A storage proof shows that the provider could produce the challenged leaves during the round, not that it keeps a copy of its own: it could fetch them from another copy (or from the client) when challenged. Two deals with two providers are not proof of two copies |
| `storage:raw-proofs` | Every storage proof carries its challenged leaves and paths on the ledger, about 17 to 22 KiB a deal a round with 4 challenges. Proofs are not aggregated across deals or rounds, so ledger bandwidth bounds how many deals a network can keep |
| `storage:no-retrieval` | Getting data back is not part of the protocol: a provider proves custody, and returning the data to its client is up to them |
| `app:first-use-pin` | The app pins the chain identity of the first node it verifies (trust on first use) unless its user pins one first. A first use against a lying node pins the liar's chain |
| `app:plain-key` | The app keeps the device's key unencrypted in its directory (mode 0600 on Unix), not in the platform's keystore. Anything that can read the directory can spend |
| `app:unproven-balances` | The balances the app shows come from the node unproven; its own receipts and the Pulses it verifies are proven |
| `agent:local-policy` | An agent runtime's autonomy level, approvals, grants and taint are enforced by the runtime on the machine it runs on. Only the agent's charter (payees, limits, budget, expiry, suspension) is enforced by the ledger |
| `agent:provider-sees-context` | A model provider receives the conversation it is asked to continue, tool results included |
| `agent:plain-sessions` | Agent sessions, with their conversations and journals, are stored unencrypted under `$SION_HOME/agents/sessions` |
| `agent:local-skill-checks` | The runtime on the agent's machine checks a skill's bundle against its release when installing it, and takes a skill out of use when its release is no longer active at the start of each run. A session already running keeps the skill until it ends, and installed files changed on disk afterwards are not checked again |
| `agent:eval-runs` | Evaluation runs are not proven. A verifier's attestation that a version passes an evaluation set rests on its bond and the bounty's adjudicator; with a model that varies between runs, another run can differ. That a verifier did not write the version is checked by the commands, not by the ledger |
| `agent:channel-token` | Whoever holds the HTTP channel's bearer token can start and continue the sessions of every profile on the machine, approvals included, within each profile's autonomy and the agent's charter. The channel speaks plain HTTP |
| `agent:telegram-chats` | A Telegram bot trusts Telegram for who writes in the chats it answers; in a group chat, every member speaks for its owner, approvals included |
| `agent:local-scheduler` | Scheduled jobs run only while `sion agent scheduler` runs on the machine (or a system timer calls it); a missed run happens once, late |
| `agent:keyless-signer` | A keyless subagent is contained by the runtime that holds its nearest keyed ancestor's keys: the ledger lets only that ancestor sign for it, and holds it to its own caveats, but a subagent that could make that runtime sign something else as the ancestor would act within the ancestor's caveats, not its own. The runtime signs only for the session's own tool calls |
| `impl:batch-size-fee` | A batch member's fee floor splits the cost of checking signatures by the size its batch declares. A signer that declares 256 members and sends one pays about a fifth of the base fee for a signature check; validators bound this by checking cheap things first and taking batches whole |
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
