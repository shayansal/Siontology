# Implementation status

What this repository implements of the architecture ("Neo-Blockchains: The
Fold Architecture"), what it simulates, and what it leaves out. Rule 5 of
`AGENTS.md` asks that every shortcut be named; this file is where the names
live.

**Status words**

- **Built**: implemented as the architecture describes, at devnet scale, with
  tests.
- **Simplified**: implemented, with a named simplification that changes a
  guarantee or a cost.
- **Simulated**: something stands in for a component the architecture
  requires, and is labeled as a stand-in in the types and in the output.
- **Not built**: absent.

## At a glance

| Architecture section | Status |
|---|---|
| Foundations: two-sorted state, derived coordination class | Built |
| Identity: post-quantum accounts | Built |
| Identity: personhood | Not built |
| Identity: Agent Charters | Built |
| Identity: agent identities and subagents | Built (devnet scale) |
| Data availability: 2D Reed–Solomon, sampling, reconstruction, fraud proofs | Built |
| Data availability: retention and archival | Simplified |
| Ordering: Linear Lanes (owned objects) | Built |
| Ordering: Escrow Lanes (bounded shared totals) | Built |
| Ordering: Contention Consensus | Simplified (certified DAG, not uncertified) |
| Ordering: censorship resistance (forced inclusion) | Simplified (obligations and committee rotation; the shard is not frozen) |
| Ordering: committees, epochs and staking | Built (devnet scale) |
| Ordering: two levels of finality | Built |
| Execution: prove heavy, execute light | Simulated (replay proofs) |
| Execution: N-version proving | Built (two independent replay stacks) |
| Execution: proving market | Not built |
| The Fold: World Proof per Pulse | Simulated (replay proofs, real bindings) |
| Knowledge layer: claims and the Assurance Algebra | Built |
| Verifiable Clearing | Built |
| Privacy: encrypted payloads, viewing keys, sealed orders | Built |
| Privacy: confidential amounts, zero-knowledge compliance | Not built |
| Interfaces: CLI, HTTP API, MCP server for agents | Built |
| Interfaces: x402, AP2, ERC-8004, C2PA adapters | Partly built (x402) |
| Payments: streams with checkpoint settlement (beyond the paper) | Built (devnet scale) |
| Memory: logs for agents and robots, kept append-only by the ledger (beyond the paper) | Partly built (the ledger's part) |
| Economics: fees, treasury, slashing with compensation | Built |
| Economics: staking, rewards, equivocation slashing | Built |
| Economics: payments to provers, value-in-flight cap | Not built |
| Networking between machines | Simulated (in-process channels) |

## By section

### Foundations

**Built.** State is two-sorted (`sion-exec`): linear objects with owners
and versions, tombstoned when consumed and never deleted, and a grow-only
knowledge log (an RFC 9162 Merkle log in `sion-core`). A transaction's
coordination class (knowledge, owned-linear, bounded, contended) is derived
from what it touches (`Transaction::class`), never declared by the sender.
Every consensus-critical value has one Borsh encoding and is hashed with a
domain tag. The state root is a sparse Merkle tree with single-leaf
subtrees collapsed (`sion_core::smt`).

### Identity

**Accounts: built.** Accounts are identified by the hash of their genesis
authorization policy, so keys rotate (`RotatePolicy`) without changing the
identifier. The default policy requires two signatures from unrelated
families over the same digest: ML-DSA-44 (FIPS 204, lattice) and
SLH-DSA-SHA2-128f (FIPS 205, hash-based); `SLH-DSA-SHA2-128s` is available
for accounts that sign rarely. ML-KEM-768 (FIPS 203) with
ChaCha20-Poly1305 encrypts payloads to viewing keys. The implementations
are the pure-Rust `fips203`, `fips204` and `fips205` crates; they have not
been audited for side channels.

**Personhood: not built.** The architecture's issuer-signed credentials with
per-application nullifiers can be expressed today as claims, but there is no
nullifier scheme and no unlinkability (that needs zero-knowledge proofs).

**Agent Charters: built.** A charter binds an agent to a principal with
caveats the protocol checks on every payment (`sion_exec::charter`): an
allow-list of payees, a per-payment limit, a budget per window, an expiry,
an evidence requirement above a threshold (a claim that passes the decision
rule under a stated policy), receipts as claims, and whether the agent may
delegate. Delegation must attenuate (every caveat strictly narrower).
Revocation by the principal stops the agent and every sub-agent at once;
`ReclaimCharter` returns unspent allotments. Payments come from an
allotment only the agent can write, so they need no consensus.
Not built: the architecture's model terms (which models and providers may
act) and a bond behind each agent's receipts beyond the agent atom.

**Agent identities and subagents: built (devnet scale, specification
v1.8).** An agent identity is a charter with an identity record (a card
commitment and a depth); its id is the agent's id. `SpawnAgents` launches
up to 256 subagents per transaction, each with its own account, card,
attenuated caveats, a window limit carved from its parent's and a budget
carved from its parent's allotment. Any overseer (the backer, the
principal, an ancestor's agent) can suspend a subtree, the suspender or the
backer can resume it, and the agent or any overseer can revoke it. Both
proof stacks implement it, and the differential tests cover every
operation and its failure cases. The `swarm` demo runs it on a devnet.
Not built, and named in the [design note](book/src/research/agent-scale.md):
cohorts that create a million identities in one transaction and
materialize each on first use, identities without an account of their own
(signed for by an ancestor in batch roots), additive token-bucket rate
limits (window limits are fixed windows, so a subtree can spend up to twice
its root's limit in an interval that straddles two windows), and a rule
that makes a certified agent payment final before it executes (today a
suspension or revocation that executes first makes it fail).

### Data availability

**Built.** Each Pulse's data (transaction bodies in execution order) is laid
out as a `k × k` square of Goldilocks field elements, extended with a 2D
Reed–Solomon code to `2k × 2k`, and committed with row and column Merkle
roots (`sion-da`). Light clients sample random shares; withholding enough to
prevent reconstruction is detected with probability `1 − (3/4)^s` for `s`
samples. Iterative row/column decoding rebuilds the square from any
sufficient subset, and bad-encoding fraud proofs exist as a library. The
World Proof additionally checks that the published data encodes to the
certified data root, so the fraud-proof path is a fallback.

**Retention: simplified.** Validators keep a write-ahead log of every
executed Pulse and periodic snapshots, and serve the last 256 Pulses' data
from memory. There is no archival tier and no incentive for anyone to keep
shares.

### Ordering

**Linear Lanes: built.** An owned object version can be locked to one
transaction digest per validator. A transaction is final when validators
holding more than two thirds of its shard committee's stake have approved
it. Approvals are signed per batch (one Merkle root per batch, one dual
signature per batch), so a certificate is a few references into signed
batches. Every transaction, contended ones included, locks its owned inputs
this way before consensus sees it.

**Escrow Lanes: built.** `CreateBounded` splits a shared total (seats, a
budget, a rate limit) into single-writer allotments; spending an allotment
is an owned-path operation; only `Rebalance` goes through consensus.

**Contention Consensus: simplified.** A certified-DAG BFT protocol in the
Bullshark family, partially synchronous, with leader timeouts and
equivocation evidence (`sion_consensus::core`), tested in a deterministic
network simulator with an adversarial pre-GST phase. The architecture's
production target is an uncertified DAG (Mysticeti class), which commits
in fewer message delays.

**Censorship resistance: simplified.** A user posts a transaction
straight to consensus as a forced intent (`POST /v1/forced`, `sion send
--forced`); every validator proposes every round, so one honest proposer
gets it ordered. The Pulse that commits it **posts** it in the state as an
obligation of the sender's shard (specification §6.3), after checking that
it could execute at all (signature, fee, inputs owned at their versions).
The shard then has a window (8 Pulses by default) to execute it or for it
to become unexecutable. Honest validators of the shard approve forced
intents themselves and exchange the approvals, so a certificate forms
without the gateway. At the deadline, the state checks whether the intent
could still execute; if it could, the shard censored it: the intent is
**overdue** and the shard's lane committee is replaced by validators drawn
from outside it first (at most once per shard per window), and the
obligation stands for another window. Every Pulse header commits to the
number of pending and overdue intents and a root over the overdue ones
(`sion_light::ForcedSummary`), so censorship by more than a third of a
committee appears in certified headers that light clients check (`sion
demo censorship`). Simplified: the architecture also freezes the shard
until the obligation is met; here the shard keeps running under its new
committee. With no more validators than seats (the default devnet: 4 and
4), a rotation can only reshuffle the same members. The header's
forced-inclusion summary is certified by the committee and can be checked
against the obligations in the state, but the World Proof does not
recompute it.

**Committees, epochs and staking: built (devnet scale).** The validator
set is an object in the state (specification §2, §5.26–5.30, §6.3). A
validator joins by staking with a proof that it holds its keys, takes its
seat at the next epoch boundary, leaves at a boundary and withdraws after
unbonding. At each epoch's last Pulse the state pays a share of the fees
collected during the epoch (and nothing else the treasury received) to the
epoch's validators by weight, admits and retires
validators, picks the next active set by stake (at most `max_active`) and
reseeds lane-committee sampling with the beacon. Every validator derives
the consensus committee (the active set, weighted) and the lane committees
(`sample_committee` per shard, with rotations) from its state after every
Pulse; nothing about committees is configured. Consensus runs one DAG per
session: a new session begins at every epoch boundary (and every restart);
what the old session committed after the epoch's last Pulse, or never
committed, is proposed again in the new one. Standby validators run as
observers and start proposing once their stake makes them active. `sion
demo staking` runs the whole cycle: a standby validator stakes and joins,
a light client follows the committee change, rewards are claimed, and the
newcomer is slashed for signing two headers for one round and loses its
seat. Simplified: the consensus committee is the whole active set rather
than a sample; a slashed validator keeps its seat until the epoch ends;
validators log equivocation they observe but do not report it themselves
(any account can submit the evidence); and certificates are not drained at
a committee change. A lane certificate is checked against the committee in
force when its transaction executes, so one formed just before a change is
certified again by the new committee. Members who stay keep their locks;
new members start without them, so an owner who signs two spends of one
object version could get the second certified by a fully replaced
committee, defeating the first certificate's finality (it never executes).
Proof finality is unaffected. The architecture drains certificates at
every change; until then certificate finality holds within one
committee's term (`impl:lock-handover`).

**Two levels of finality: built.** Certificate finality comes from the lane
certificate (owned paths) and the consensus commit (contended paths); every
Pulse header is then signed by the committee, and proof finality comes when
the World Proof for that Pulse verifies.

### Execution and proving

**Prove heavy, execute light: simulated.** Execution is cut into chunks;
each chunk carries a statement (world commitments before and after,
accumulators of effects and deferred items, the steps) and a witness (each
touched object with a sparse-Merkle proof, plus the knowledge frontier).
Proof stack A (`sion_prover::ReplayStack`) "proves" a chunk by including
everything needed to re-execute it statelessly; verification re-executes
it and rejects any read outside the witness. This has the interface and
the binding properties of a succinct proof (stateless, chunk-local,
foldable) but not its cost: verifying is as expensive as executing. The
type is labeled `simulated: true`, and every report that verifies a World
Proof says so. A STARK stack would plug into the same `ProofStack` trait.

**N-version proving: built.** The state transition is specified in
`docs/spec/state-transition.md`. A second implementation written only from
that document (`crates/sion-spec`, proof stack B) checks every chunk
alongside the reference executor (stack A), and a World Proof verifies only
if both accept every chunk. `scripts/check-independence.sh` (run in CI)
checks that stack B never calls the reference executor's logic. Its
differential tests run thousands of chunks, random workloads, boundary
probes and tampering through both stacks. They found two defects in the
reference executor and several gaps in the specification, all fixed in
specification v1.1: a market clearing deadline near `u64::MAX` overflowed
when its batch sealed (any account could halt the chain), and a charter
window near `u64::MAX` made the reference executor disagree with the
specification. Both stacks share the hashing, Merkle, signature, Assurance
Algebra and clearing-certificate code (`impl:shared-primitives`).

**Proving market: not built.** The devnet runs one prover thread.

**Verifiable AI inference: built as claims.** The three tiers (proof,
attestation, bond) are expressed as claims whose formulas name their atoms
(`receipts` demo): a TEE quote is an unbonded atom with a known attack cost,
an operator bond is slashable. Zero-knowledge inference proofs are not
built.

### The Fold

**Simulated, with real bindings.** For every Pulse the devnet's prover folds
all chunk proofs into a World Proof (`sion_prover::world`) that checks the
chain of chunk statements from the Pulse's first commitment to its last,
the receipts root over every effect, the published data (it is exactly what
executed, and it encodes to the certified data-availability root), and every
lane certificate against its shard committee. Folding is a Merkle tree over
leaf statements, not recursive proof composition; the World Proof's size
grows with the Pulse's witnesses. A light client verifies it today by
re-executing every chunk (or a random sample of chunks).

### Knowledge layer

**Built.** Claims carry formulas over atoms and other claims (`sion-assurance`).
Cost of corruption is an interval computed per consumer policy: exact for
formulas whose disjuncts share no atoms, with a lower bound by an exact
solver when they do. The decision rule, reservations of `κ·V` against a
claim's backing, bonds with lock periods, challenges, adjudication,
slashing with compensation to the parties who relied on the claim, and
failure probabilities for insurance are implemented and property-tested.
Policies can trust or distrust atoms and classes, or assume an atom broken.

The assurance registry is a single contended object. Every claim, bond and
reservation operation touches it, so its witness grows with the registry.
That is fine at devnet scale and is the first thing to split (per atom and
per claim objects) for production.

### Verifiable Clearing

**Built.** Markets collect sealed orders during a batch window, seal with
the Pulse's beacon, reveal sealed orders with committee key shares, and
settle the first submitted clearing whose optimality certificate verifies
(`sion-lp`, `sion-clearing`): exact rational arithmetic, a linear-time
certificate check, beacon-derived priorities for canonical tie-breaking,
and a canonical price band. Sealed double auctions and assignment markets
are implemented; the market demo clears at one price.

### Privacy and compliance

**Built:** claim payloads can be public, encrypted to an ML-KEM viewing key,
or replaced by a commitment; orders can be sealed to a market committee
(ML-KEM with Shamir t-of-n sharing, practical for small committees).

**Not built:** transfer amounts and balances are public. Confidential
transfers, private computation and compliance-by-proof (KYC, Travel Rule
and sanctions checks proven without revealing data) need a succinct
zero-knowledge stack. The claim model can carry such attestations today,
but only as issuer-signed claims.

### Interfaces

**Built:** the `sion` CLI (devnet, wallets, transfers, claims, bonds,
challenges, charters, markets, light-client verification, demos,
benchmarks), a JSON HTTP API (`sion_node::api`) and a Model Context
Protocol server (`sion mcp`) with tools to check balances, pay, issue
claims, price a claim with the Assurance Algebra before relying on it, pay
under an Agent Charter, verify a Pulse and read events.

**Built as well:** every transaction kind and every event reads as a
sentence (`sion_cli::describe`); the CLI prints what each command signs and
what happened; the MCP server has `sion_submit_intent` (a transaction the
agent signed itself) and `sion_fetch_receipt` (a receipt verified as a
light client would).

**x402: built.** `sion_cli::x402` implements x402 version 2 (the
`PAYMENT-REQUIRED`, `PAYMENT-SIGNATURE` and `PAYMENT-RESPONSE` headers)
with a Siontology `exact` scheme on network `sion:<chain>`: a resource server
quotes a price with a fresh nonce, the client signs a transfer or a
charter payment whose purpose names the quote, and the server's
facilitator submits it and serves only once the receipt verifies as a
light client would (committee certificate, certified order, inclusion
proof). Each quote and each transaction pays for one request. `sion x402
serve` and `sion x402 fetch` use it from the command line, and the MCP
tool `sion_fetch_paid` lets an agent buy HTTP resources within a cap it
sets and within its charter's limits. The adapter speaks the protocol's
headers and JSON; it has not been tested against third-party x402
clients, and EVM schemes are out of scope.

**x402 `stream` scheme: built.** A client that opened a payment stream to a
server pays each request with the next element of its hash chain instead
of a transaction; the server's `StreamDesk` checks it with one hash per
step, serves, and settles in batches (by checkpoint when it can), and
settles everything before it stops. A refused payment's response names the
server's position, so an honest client resynchronizes. Tested end to end
on a devnet (`crates/sion-cli/tests/streams.rs`): lost payments, escrow
refusals and top-ups, forged and replayed elements, misdirected
requirements, and the close-out. The desk keeps its tabs in memory: a
server that crashes loses what it had not settled. MCP's
`sion_fetch_paid` pays with `exact` only.

**Not built:** adapters for AP2, ERC-8004 and C2PA.

### Payments

**Payment streams: built (specification v1.6 and v1.7).** Not in the
architecture paper; added to test whether a ledger can settle payments at
billions a second without checking each one
([research note](book/src/research/billion-scale.md)). A payer escrows funds for one
payee and pays by revealing a SHA3-256 hash chain's elements off the
ledger; the payee settles with a walk (one hash per step, at most 65,536)
or, with checkpoints, a jump (at most `spacing + 21` hashes, however many
payments). Both proof stacks implement it, with differential tests for
every rule, a randomized property test and executor scenarios; the CLI
has `sion stream …`, the demo `streams`, and the bench group `streams`.
Measured on one core of the 2-core sandbox: about 2.2 million payee
checks a second, and settlements covering about 9 × 10⁸ payments a second
by checkpoint. **Simplified:** the fee of 2 micro-USD per hash prices
execution, not proving; payers build their chain up front (one hash per
step); unsettled payments are the payee's risk if it does not settle by the
stream's last Pulse; a stream pays one payee in one direction.

### Memory

**Memory logs: built on the ledger (specification v1.9).** Not in the
architecture paper; added so that agents' and robots' memories can be
verified ([concept](book/src/concepts/memory.md), [design
note](book/src/research/agent-scale.md)). A memory log is owned by its
writer; `CommitMemory` moves its head to a new size and root only along an
RFC 9162 consistency proof, which the ledger checks, so a log only grows
and cannot fork, and the commit's Pulse is the transaction time of the
atoms it adds. A log can belong to an agent identity, and then stops
growing while the agent, or an agent above it, is suspended or revoked.
Both proof stacks implement it, with differential tests for every rule and
a randomized property test of honest and forged commits. **Simplified:**
the index and manifest roots are stored unchecked (atom
`impl:memory-index`). **Not built:** the off-ledger library (atoms, the 4D
index and its completeness proofs, belief folding, encryption), command-
line and MCP interfaces, aggregators that commit many logs in one claim,
curators of shared world memory, and storage with proofs.

### Economics

**Built:** fees (a base fee plus a fee per KiB) go to the treasury;
slashed bonds compensate the parties who relied on the claim and pay a
share to the challenger, with the rest to the treasury. Validators stake,
earn a share of the fees collected in each epoch by weight (70 % on the
devnet; slashing remainders and released pools are never paid out)
and are slashed on signed evidence of equivocation: two consensus headers
for one round, or two live lane approvals of one object version. The
whole stake and unclaimed rewards are forfeited; the reporter receives a
share (5 % on the devnet, so reporting oneself still costs 95 %) and the
treasury the rest.

**Not built:** payments to provers, a value-in-flight cap.

### Node and operations

**Simulated:** a devnet runs every validator in one process, each on its own
thread with its own state replica, exchanging real signed messages over
in-memory channels. There is no peer-to-peer networking, and the client
gateway is a single component that collects lane approvals.

**Built:** a segmented write-ahead log with checksums and torn-tail
truncation, periodic snapshots, replay that re-checks every commitment,
restart into a new consensus session, state sync for validators that fell
behind, and epoch boundary proofs kept on disk for light clients
(`GET /v1/epochs/{e}`).

## Trust assumptions this implementation adds

These are atoms in the formula of anything the devnet reports, on top of
the architecture's own:

| Atom | What it means |
|---|---|
| `impl:replay-proofs` | World Proofs are checked by re-execution, so "proof finality" costs a verifier as much as executing; succinctness is assumed, not delivered. |
| `impl:shared-primitives` | Both proof stacks use the same hashing, Merkle, signature, Assurance Algebra and clearing-certificate code; a bug there is common-mode. |
| `impl:devnet-keys` | Devnet validator keys are derived from the chain name; anyone can derive them. A devnet secures nothing of value. |
| `impl:in-process-network` | Validators share a machine and a process; network faults are not exercised by the devnet (they are by the consensus simulator). |
| `impl:reliable-handover` | At an epoch boundary each validator starts a new consensus session; a validator that has not reached the boundary finishes the old session from messages already sent, which the in-process network always delivers. A real network needs a drain period. |
| `impl:slashed-seat` | A slashed validator keeps its seat, and its weight in quorums, until the epoch ends. |
| `impl:active-floor` | Validators can leave until one remains: nothing keeps the active set large enough to tolerate a faulty member, so the operators of the other validators can leave a single validator forming every quorum. |
| `impl:evidence-scope` | Equivocation evidence is not bound to this chain: a validator's keys must not be used on another network. |
| `impl:lock-handover` | Certificates are not drained when a lane committee changes: certificate finality for owned transactions holds within one committee's term (an owner who double-signs could get the second spend certified by a fully replaced committee). |
| `impl:hash-chain-beacon` | The beacon is a hash-chain commit-reveal; the last validator to reveal can bias it by withholding (at the cost of being seen). |
| `impl:public-amounts` | Amounts and balances are visible to everyone. |
| `impl:memory-index` | The ledger stores a memory log's index root without checking it: a memory query proof is complete relative to that root, and nothing on the ledger yet acts on evidence that an index leaves an atom out. |
| `impl:unaudited-pq-crates` | The FIPS 203/204/205 implementations are unaudited pure-Rust crates. |
| `impl:insecure-sim-signatures` | Only when a devnet is started with `--insecure-fast-signatures`: validators sign with a simulation MAC. The chain identity records it, and light-client reports say so. |

## Light-client trust

A light client pins one value: the chain identity (`sion_light::ChainSpec::id`),
a digest of the genesis specification, the genesis validators with their
stake and public keys, and the shard layout. That fixes epoch 0's
committee. Every later committee is in the state, and the client follows
it one boundary at a time (`sion_light::EpochChain`): the certificate of an
epoch's last Pulse, checked against the committee it already trusts, and a
state proof of the validator set under that header give it the next
epoch's committee and keys. World Proofs carry the validator set the Pulse
began with, proven against its starting state root, so lane certificates
are checked against committees derived from the state, not from anything a
node serves. Without a pin, `sion verify` says the node is vouching for
itself.

## Reviews and what they found

Three independent reviews, run by agents that had not written the code,
read the consensus-critical libraries, the node, and the validator
machinery, and confirmed their findings with tests. A fourth, the
networking plan, found two more gaps while reading the node. Everything
below is fixed, with a regression test, unless marked otherwise.

- **State machine and libraries.** An unbounded chain of claim references
  made cost-of-corruption and reservation traversals recurse once per link
  until a validator's stack overflowed (fixed: depth limits at issuance,
  64 MiB thread stacks); a reservation split overflowed 128 bits for large
  capacities (fixed: exact arithmetic); a clearing settlement used an
  unchecked multiplication; bounded totals and batches had no size limits;
  certificate values had no size limit (all fixed in specification v1.2).
- **Node, consensus and light client.**
  - The signature pre-verification cache keyed on the transaction body but
    not its signatures, so a stripped-signature copy could split replicas
    (fixed: the key covers the signatures).
  - Expanding a certificate group copied attacker-sized signatures per
    reference before any check (fixed: groups are shape-checked first, and
    header payloads are bounded).
  - Lane locks kept a copy of the whole request per input and were never
    released, which also froze inputs of lagging, rejected or expired
    transactions (fixed: one shared copy per request; locks are released
    when the transaction succeeds, fails or expires, a rejected one keeping
    them until it expires; requests from lagging
    validators wait for them to catch up; the gateway accepts resubmission).
  - Lane validation skipped the checks execution makes before charging a
    fee, so zero-fee transactions were published for free (fixed), and
    Pulse data was unbounded (fixed: header payload limits; validators keep
    the published data rather than the extended square, and never sign a
    header without an availability commitment).
  - Certificate finality did not survive a restart, which starts a new
    epoch (fixed: approvals are logged before they leave a validator and
    locks are restored after a restart, so no conflicting spend can be
    certified and the original can be certified again).
  - Light-client sampling used a fixed seed (fixed: fresh randomness); the
    client did not check the node's transaction list against the certified
    order, or a receipt's position (fixed); a missing World Proof did not
    fail verification (fixed).
  - Transactions and validator messages were not bound to a chain (fixed:
    transaction signatures cover the chain configuration, specification
    v1.2; lane batches and consensus headers name the chain identity, and
    votes cover it through the header digest).
  - Certified groups went to a single validator (fixed: f + 1 validators);
    far-future headers, header signatures and beacon reveals were retained
    without bound (fixed: windows and caps); equivocation evidence was
    accepted across epochs (fixed); a re-included certificate's later
    rejection could hide the real receipt (fixed: the first execution is the
    receipt); forged Pulse numbers could make an unpinned client overflow
    (fixed); the prover's job queue and the faucet queue were unbounded
    (fixed).
  - **Open:** fee-paying transactions with large bodies can still make
    Pulses slow, since data-availability encoding of the largest allowed
    Pulse takes seconds on one core; an account query scans every object.
- **Staking, epochs and committees (specification v1.4).**
  - An honest validator could be slashed: the node released a rejected
    transaction's lane locks at once, although its inputs kept their
    versions, and then approved a second spend that the `Approvals`
    evidence treats as equivocation while the first transaction is
    unexpired. An owner could frame every validator that approved one of
    its transactions (fixed: a rejected transaction keeps its locks until
    it expires; specification v1.5 spells out the rule).
  - If every active validator was slashed within an epoch, the boundary
    kept the old active set, so slashed validators with nothing at stake
    certified the next epoch (fixed in specification v1.5: they lose their
    seats regardless, and the chain halts).
  - A simulator default paid reporters 50 % instead of the documented 5 %
    (fixed).
  - **Open:** the active set can shrink to a single validator
    (`impl:active-floor`).
- **Networking plan** ([research note](book/src/research/networking.md)).
  - The gateway capped certificate groups at 512 transactions but not in
    bytes, and consensus drops an item above its payload limit without a
    reply, so a burst of dual-signed transfers never executed (fixed:
    groups are split by encoded size, at most half the limit; a
    certificate too large to fit alone is refused with a reason).
  - Beacon reveals committed ahead of their Pulse were kept only in
    memory, so a validator restarting alone would compute a different
    beacon (fixed in storage format 4: records and snapshots carry them,
    as of each Pulse's close). **Open:** Pulse headers do not commit to
    them yet, which a validator syncing from certified headers will need
    (a specification version).
