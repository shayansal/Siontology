# Glossary

**Active set.** The validators that hold seats in the current epoch, each
with a weight equal to its own plus its pool's earning stake, in whole
units. See [Validators, staking and epochs](../concepts/committees.md).

**Adjudicator.** A bonded party a claim names to rule on challenges. A
ruling cannot be trusted beyond the adjudicator's own bond.

**Agent Charter.** A grant that lets an AI agent spend a principal's money
within caveats the protocol enforces on every payment. See
[Agent Charters](../concepts/agent-charters.md).

**Agent identity.** A charter with an identity record: a card commitment
and a depth in its tree. Its id is the agent's id, written
`did:sion:<chain>:<id>`. See [Agent identities and
subagents](../concepts/agent-identities.md).

**Allotment.** A single-writer share of a bounded total, spent without
consensus. See [Escrow Lanes](../concepts/escrow-lanes.md).

**Anchor.** A leader's header in the consensus DAG; committing it orders
its causal history.

**Assurance Algebra.** The rules for formulas over assumptions, their cost
of corruption, the decision rule, reservations and slashing. See
[The Assurance Algebra](../concepts/assurance/index.md).

**Atom.** One assumption a claim rests on: a cryptographic primitive, a
bond, a piece of hardware, an institution. Each has a class and, if
bonded, a bond.

**Attestation.** A verifier's claim (`task.attestation/v1`) that a task's
output passes, resting on the verifier's bond. A result may rest on the
attestations of verifiers the task names.

**Beacon.** Public randomness for each Pulse, from validators' hash-chain
reveals. It seals market batches, breaks clearing ties and seeds lane
committees.

**Bond.** Money posted behind an atom, slashed if a challenge against it
is upheld.

**Bounty.** A task that names no worker: anyone may settle it with an
acceptable result, and its poster may withdraw it until then.

**Certificate.** Signatures from more than two thirds of a committee's
weight: on a transaction's inputs (a lane certificate), or on a Pulse
header (a Pulse certificate).

**Chain identity.** The digest a light client pins: genesis, genesis
validators with stake and keys, shard layout. See
[The chain identity](../reference/chain-identity.md).

**Challenge.** A deposit-backed assertion that a bonded atom of a claim is
broken, ruled on by the claim's adjudicator.

**Checkpoint.** A commitment, made when a payment stream opens, to every
`spacing`-th element of the payer's chain, as leaves of a Merkle tree. It
lets the payee settle by a jump. See
[Payment streams](../concepts/payment-streams.md).

**Chunk.** A run of transactions (16 by default), or the deferred phase,
with a statement and a witness; the unit a proof stack proves.

**Claim.** A statement with a formula of the atoms and other claims it
rests on, appended to the knowledge log. See
[Claims and the knowledge log](../concepts/claims.md).

**Commission.** The share of its pool's rewards a validator's operator
keeps: at most 25 %, effective at the epoch's end. See
[Pooled stake](../concepts/validators/pooled-stake.md).

**Committee.** The validators that certify something: the consensus
committee (the active set) certifies Pulse headers and orders contended
transactions; each shard's lane committee certifies its transactions'
inputs.

**Contention domain.** A shared object, or a group of them, whose updates
conflict and are ordered by consensus: the assurance registry, a market, a
charter, a bounded total, the validator set.

**Coordination class.** How much ordering a transaction needs, derived
from what it touches: knowledge, owned-linear, bounded or contended. See
[Two-sorted state](../concepts/two-sorted-state.md).

**Cost of corruption.** What an attacker would have to spend or forfeit to
make a claim false, under a consumer's policy. See
[Cost of corruption](../concepts/assurance/cost-of-corruption.md).

**Data-availability sampling.** Checking random shares of a Pulse's
erasure-coded data so withholding is detected with high probability. See
[Data availability](../concepts/data-availability.md).

**Decision rule.** Rely on a claim for value V only if its cost of
corruption, discounted for detection and capped by the adjudicator, is at
least κ·V, and that much backing is unreserved. See
[The decision rule](../concepts/assurance/decision-rule.md).

**Delegation.** An object its delegator owns that holds shares of a
validator's pool: stake added to the validator's weight without handing
it over. See [Pooled stake](../concepts/validators/pooled-stake.md).

**Deferred phase.** The last part of a Pulse: commutative effects in
canonical order, forced-intent posting, and scheduled events such as batch
sealing and epoch ends.

**Devnet.** A whole network in one process, for development and
demonstration (`sion devnet`).

**Epoch.** A run of Pulses with one validator set. Committees change at
epoch boundaries. See [Epochs and committees](../concepts/validators/epochs.md).

**Equivocation.** Signing two conflicting messages: two consensus headers
for one round, or two live lane approvals of one object version. Reporting
it slashes the signer's stake.

**Escrow Lanes.** Splitting a shared total into single-writer allotments
so most updates need no consensus.

**Forced intent.** A transaction posted straight to consensus, bypassing
the gateway. Once committed, it is an obligation of its sender's shard.

**Gateway.** The devnet component that routes transactions to lane
committees and hands certificates to consensus.

**Genesis.** The initial state and parameters; its digest is part of the
chain identity.

**Jump.** A settlement's proof by checkpoint: the checkpoint just below
the settled position and its Merkle path. It costs at most `spacing + 21`
hashes, however many payments it covers.

**κ (kappa).** The safety factor of the decision rule; 2 by default.

**Knowledge log.** The grow-only Merkle log of claims (RFC 9162).

**Lane epoch.** The identifier of a lane committee: the validator-set
epoch and the shard's rotations in it. Approvals from one lane epoch never
count in another.

**Light client.** A client that checks headers, receipts, state and data
with proofs against a pinned chain identity, without executing the chain.

**Linear Lanes.** Certifying owned-object transactions with one committee
certificate and no consensus. See [Linear Lanes](../concepts/linear-lanes.md).

**Memory atom.** One statement in a memory log: what was observed or
asserted, when it held, where, with what confidence, and what it rests on.
See [Memory for agents and robots](../concepts/memory.md).

**Memory log.** A log of memory atoms that only its writer appends to,
kept off the ledger. The ledger keeps its head and moves it only along a
consistency proof, so the log only grows (specification v1.9).

**Micro-unit.** 10⁻⁶ of an asset; the unit amounts are stored in.

**Neo-blockchain.** A network whose product is verified knowledge, not
ordered transactions. See [Neo-blockchains](../concepts/neo-blockchains.md).

**Obligation.** A forced intent a shard owes, held in the state with a
deadline.

**Operator.** The account that stakes for a validator and receives its
rewards and withdrawals.

**Overseer.** For an agent identity: its backer, its principal, or the
agent of any charter above it. Any overseer may suspend or revoke it.

**Payment stream.** Escrow from one payer for one payee, paid off the
ledger by revealing a hash chain's elements, one per payment, and settled
by the payee in batches. See [Payment streams](../concepts/payment-streams.md).

**Pool.** A validator's delegated stake, divided into shares, in the
delegation book. Earning shares share the validator's rewards; all shares
share its slashing.

**Policy.** A consumer's view of which atoms and classes to trust,
distrust or assume broken, and its κ.

**Proof stack.** An implementation of chunk proving and verification. Stack
A replays with the reference executor; stack B replays with an independent
implementation of the specification.

**Pulse.** The network's heartbeat: a batch of execution with a certified
header, published data and a World Proof. See [Pulses](../concepts/pulses.md).

**Quorum.** More than two thirds of a committee's weight: `⌊2·total/3⌋ + 1`.

**Receipt.** A transaction's effects, proven against a certified header's
receipts root; for agent payments, also a receipt claim.

**Reservation.** The share of a claim's backing (κ·V) a consumer reserves
when it relies on the claim, so no claim is relied on beyond its backing.
See [Reservations](../concepts/assurance/reservations.md).

**Result claim.** A worker's claim (`task.result/v1`) that commits to a
task and its output, with exactly the formula the task requires.

**Rotation.** Replacing a shard's lane committee after it misses a
forced-inclusion deadline.

**Session.** One consensus DAG. A new session starts at every epoch
boundary and every restart.

**Shard.** A partition of accounts, each with its own lane committee.

**Slashing.** Forfeiting a bond (for a false claim) or a validator's stake
(for equivocation).

**Specification.** `docs/spec/state-transition.md`, the normative
description of the state transition. See
[The state transition specification](../specification.md).

**Standby validator.** A devnet validator that runs without a seat until
its operator stakes for it.

**Stake.** What a validator deposits to join; its weight in committees and
what it forfeits if it equivocates. Delegators add stake to a validator's
pool (see Delegation).

**State root.** The root of the sparse Merkle tree over every object.

**Stream desk.** The payee's side of payment streams in `sion_cli`: it
checks each payment with one hash, decides when to settle, and settles by
walk or jump.

**Subagent.** An agent launched by another with `SpawnAgents`: its own
identity, account, card and budget, with caveats and a window limit carved
from its parent's.

**Task.** Escrow for a piece of work, with the rule a result must pass
fixed when it is posted (specification v1.10). See [Task
Charters](../concepts/task-charters.md).

**The Fold.** Folding every chunk proof of a Pulse into one World Proof.
See [The Fold and World Proofs](../concepts/the-fold.md).

**Tombstone.** What a consumed object becomes: it keeps its identifier,
owner and version, so history stays auditable.

**Transaction time.** For a memory atom: the Pulse of the commit that
added it to its log, assigned by the ledger, not by the writer. Its valid
time, when the statement held, is the writer's.

**Two-sorted state.** Linear objects that are consumed at most once, and
knowledge that is only appended.

**Unbonding.** The Pulses an exited validator's stake, or an undelegated
delegation's, stays locked, and slashable, before it can be withdrawn.

**Validator set.** The state object holding every validator's record, the
active set, the epoch and its parameters.

**Verifiable Clearing.** Settling a market batch only with a certificate
that no better allocation exists. See
[Verifiable Clearing](../concepts/verifiable-clearing.md).

**Weight.** A validator's own plus its pool's earning stake in whole
units of the stake asset: its voting weight in the consensus committee.

**Witness.** The objects a chunk touches, with state proofs, plus the
knowledge log's frontier: what a stateless verifier needs to re-execute it.

**World Proof.** The proof that one Pulse's transition is valid, binding
its receipts, data and lane certificates. See
[The World Proof format](../reference/world-proof.md).

**x402.** HTTP 402 Payment Required, used to sell an API per request. See
[x402](../guides/x402.md).
