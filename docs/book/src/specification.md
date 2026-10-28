# The state transition specification

The state transition is specified in one normative document in the
repository:
[`docs/spec/state-transition.md`](https://github.com/ColdAI-org/siontology/blob/main/docs/spec/state-transition.md).
This page says what it covers and how to read it; it does not copy it.

The current version is **v1.10**.

## What it specifies

The function a proof stack evaluates for every chunk of every Pulse:

```text
apply_chunk(statement, witness, transactions | deferred_items) -> accept | reject
```

and through it the whole state transition: preparing and executing one
transaction, the three phases of a Pulse, and the commitments a chunk
statement carries. It is written for implementers: the key words MUST,
MUST NOT and MAY are used as in RFC 2119, and every step names the objects
it reads, because a read outside the chunk's witness rejects the chunk.

| Section | Content |
|---|---|
| §0 Scope | What is specified, what is not (data layouts and shared primitives), and what an independent implementation may and may not call |
| §1 Conventions | Hashing, encoding, amounts, ordered maps, the Pulse context |
| §2 State | Objects, tombstones, the knowledge log, the well-known objects, the validator set; reads and the witness |
| §3 Bookkeeping | Loading, storing, creating and consuming objects; drawing coins; fees; errors |
| §4 Outcome | Preparing a transaction (expiry, inputs, fee, signatures) and executing it: Success, Failed, Rejected |
| §5 Operations | Every transaction kind, §5.1 `Transfer` to §5.34 `CloseStream` |
| §6 The Pulse | The order, phases and chunks, and the deferred phase: canonical sorting, forced-intent posting, due events, epoch ends |
| §7 Commitments | World commitments, effects leaves, accumulators, steps |
| §8 Verifying a chunk | What a proof stack checks for one chunk |
| §9 Folding a Pulse | When a World Proof is valid, and what it binds |
| Changes | What each version changed and why |

## Who implements it

Two implementations exist: the reference executor (`sion-exec`, proof stack
A) and an independent one written from the document alone (`sion-spec`,
proof stack B). Where they disagree, one of them is wrong; the
differential tests exist to find out which. See
[Keeping proof stack B independent](architecture/independence.md).

## Versions

| Version | What changed |
|---|---|
| v1.1 | From the differential testing of stack B against stack A: faucet transactions pay no fee and grants are capped; market periods are bounded; charter windows, the clearing check, sealing and deadlines saturate; the deferred phase and the World Proof's bindings are specified. It fixed a market deadline that could halt a debug build and a charter window that made the reference executor diverge |
| v1.2 | From two independent reviews: transaction signatures are bound to the chain configuration; at most 256 inputs, 256 allotments and 1,024 orders per batch; bounded claim-reference and formula depth; bounded certificate values; exact reservation splits |
| v1.3 | Validators, stake, epochs and the censorship response join the state transition: staking, unstaking, withdrawing and rewards (§5.26–5.29); slashing for equivocation (§5.30); forced intents posted as obligations, deadlines and committee rotation (§6.1, §6.3); committees derived from the state and covered by the World Proof (§9) |
| v1.4 | Validators are paid from fees only: each fee is credited to the treasury as its own deferred item and added to a running total of fees, and an epoch's reward pool is a share of the fees collected since the last epoch end. Slashing remainders and released bond pools stay in the treasury and never become validator income |
| v1.5 | From an independent review of the validator machinery: slashed validators lose their seats at the epoch boundary even if nobody else is eligible, so a chain whose every eligible validator was slashed halts instead of running on them; the reasoning behind approval evidence is spelled out (an approval holds its inputs until its transaction succeeds, fails or expires; a rejection is none of these) |
| v1.6 | Payment streams (§5.31–5.34): a payer escrows funds for one payee and pays it off the ledger by revealing a hash chain's elements, one hash to check per payment; the payee settles many payments in one transaction, which walks one hash per step and pays a fee per hash |
| v1.7 | Stream checkpoints: a payer may commit to a Merkle tree over every `spacing`-th element of its chain, and the payee may settle by a jump to a checkpoint, for at most `spacing + 21` hashes however many payments it covers ([Payment streams](concepts/payment-streams.md)) |
| v1.8 | Agent identities (§5.35–5.39): registered agents and subagents launched 256 at a time, each with its own account, card commitment, caveats and budget; suspension and resumption of whole subtrees; revocation by the agent or any overseer; window limits carved from the parent's like budgets ([Agent identities and subagents](concepts/agent-identities.md)) |
| v1.9 | Memory logs (§5.40–5.41): logs of memory atoms kept off the ledger, whose heads the ledger moves only along RFC 9162 consistency proofs, so a log only grows and cannot fork; the Pulse of a commit is the transaction time of the atoms it adds ([Memory for agents and robots](concepts/memory.md)) |
| v1.10 | Task Charters (§5.42–5.44): escrowed tasks whose acceptance rule is fixed at posting; results bound to their task, with exactly the formula the task requires; settlement once, with κ times the payment of backing reserved for the payer, so a result later proven false compensates it; bounties, cancellation, and posting from an agent's budget ([Task Charters](concepts/task-charters.md)) |

The **Changes** section of the document has the full entries.

## When to read it

- Before changing what any transaction or Pulse does: the specification
  changes first, then both stacks ([Changing the state transition](architecture/independence.md#changing-the-state-transition)).
- When you need the exact rule behind a behaviour this book describes in
  words, such as which inputs a forced intent must still own at its
  deadline, or how rewards are rounded.
- When writing another implementation, or another proof stack.
