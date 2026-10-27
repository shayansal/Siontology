# Reviews and fixes

The code has had no external security audit. It has had three
independent internal reviews, run by agents that had not written the code,
and its two proof stacks test each other. This chapter lists what they found. Every
item is fixed, with a regression test, unless marked **open**. The source
is the repository's `docs/implementation-status.md`.

## Review of the state machine and libraries

- An unbounded chain of claim references made cost-of-corruption and
  reservation traversals recurse once per link until a validator's stack
  overflowed. Fixed: depth limits at issuance (32 levels of formula, 16
  claim references), and 64 MiB thread stacks.
- A reservation split overflowed 128 bits for large capacities. Fixed:
  exact arithmetic.
- A clearing settlement used an unchecked multiplication. Fixed.
- Bounded totals and batches had no size limits, and certificate values
  had none. Fixed in specification v1.2 (256 allotments, 1,024 orders per
  batch, bounded certificate values).

## Review of the node, consensus and light client

- The signature pre-verification cache keyed on the transaction body but
  not its signatures, so a copy with stripped signatures could split
  replicas. Fixed: the key covers the signatures.
- Expanding a certificate group copied attacker-sized signatures per
  reference before any check. Fixed: groups are shape-checked first, and
  header payloads are bounded.
- Lane locks kept a copy of the whole request per input and were never
  released, which also froze the inputs of lagging, rejected or expired
  transactions. Fixed: one shared copy per request; locks are released
  when the transaction succeeds, fails or expires (a rejected one keeps
  them until it expires: see the third review below); requests from lagging
  validators wait for them to catch up; the gateway accepts resubmission.
- Lane validation skipped the checks execution makes before charging a
  fee, so zero-fee transactions were published for free. Fixed. Pulse data
  was unbounded. Fixed: header payload limits; validators keep the
  published data rather than the extended square, and never sign a header
  without an availability commitment.
- Certificate finality did not survive a restart. Fixed: approvals are
  logged before they leave a validator, and locks are restored after a
  restart, so no conflicting spend can be certified and the original can
  be certified again.
- Light-client sampling used a fixed seed. Fixed: fresh randomness. The
  client did not check the node's transaction list against the certified
  order, or a receipt's position. Fixed. A missing World Proof did not fail
  verification. Fixed.
- Transactions and validator messages were not bound to a chain. Fixed:
  transaction signatures cover the chain configuration (specification
  v1.2); lane batches and consensus headers name the chain identity, and
  votes cover it through the header digest.
- Certified groups went to a single validator. Fixed: f + 1 validators.
- Far-future headers, header signatures and beacon reveals were retained
  without bound. Fixed: windows and caps.
- Equivocation evidence was accepted across epochs. Fixed.
- A re-included certificate's later rejection could hide the real receipt.
  Fixed: the first execution is the receipt.
- Forged Pulse numbers could make an unpinned client overflow. Fixed.
- The prover's job queue and the faucet queue were unbounded. Fixed.
- **Open:** fee-paying transactions with large bodies can still make
  Pulses slow, since data-availability encoding of the largest allowed
  Pulse takes seconds on one core.
- **Open:** an account query scans every object.

## Review of staking, epochs and committees

The third review read the validator machinery of specification v1.4
(staking, rewards, slashing, epoch boundaries, committee sampling and
rotation) in the executor, the node and the light client, and confirmed
two defects with tests that failed until they were fixed
(`crates/sion-exec/tests/staking_review.rs`).

- **An honest validator could be slashed** (high). The `Approvals`
  evidence treats two approvals by one validator of transactions that
  share an input version, both unexpired, as proof of equivocation. The
  node released a transaction's lane locks as soon as it executed,
  rejected ones included. A rejected transaction writes nothing, so its
  inputs kept their versions, and the validator then approved a second
  spend of a version its first approval still covered. An owner could
  frame every validator that approved one of its transactions: have it
  rejected by spending one of its other inputs elsewhere, spend the rest
  again, and report the two approvals. Fixed in the node: a rejected
  transaction keeps its locks until it expires, which narrows the lane-lock
  fix of the node review above (such inputs stay locked until the
  transaction expires, no longer). Specification v1.5 spells out why.
- **Slashed validators could keep their seats** (medium). If every active
  validator was slashed within one epoch, the boundary found nobody
  eligible and kept the old active set, so validators proven to equivocate,
  with nothing left at stake, certified the next epoch. Fixed in
  specification v1.5: they lose their seats regardless, the next active
  set is empty, and the chain halts.
- **The active set can shrink to one validator** (low, **open**). The last
  validator staying cannot leave, but nothing keeps more than one, so
  operators can leave until a single validator forms every quorum. Named
  as the trust atom `impl:active-floor`; a production network would set a
  minimum.
- A simulator default paid a reporter half of a slashed stake, while the
  devnet and these pages say 5 %. Aligned.

The review found the rest sound: reward and slashing arithmetic is exact
and cannot overflow, and the treasury never pays out more than it holds;
stake, slashing and withdrawal conserve value; a slashed or withdrawn
identity cannot register again; proofs of possession bind the chain, the
operator and both keys; double slashing, claiming and withdrawing are
refused; approval evidence lapses with the transactions and stays within
one lane epoch; and the executor, the node and the light client derive the
same committees, the light client only from proven state, one boundary at
a time.

## Found by the two proof stacks

The differential tests of stack B against stack A found:

- a market clearing deadline near `u64::MAX` that overflowed when its batch
  sealed, so any account could halt the chain (fixed in specification
  v1.1);
- a charter window near `u64::MAX` that made the reference executor
  disagree with the specification (fixed in v1.1);
- when the validator set joined the state transition, one divergence on
  whether an epoch's end writes the treasury's USD entry; the
  specification now says it always does (v1.3).

See [Keeping proof stack B independent](../architecture/independence.md).

## Found by the demos

The staking demo found that a consensus commit far beyond an epoch's last
Pulse also closed the Pulses after it, so the old session executed a Pulse
that the next session then closed again. Fixed: a session closes no Pulse
after its epoch's last one, and what it committed later is proposed again
in the next session (a regression test in `sion_node::pulses`).

## Reporting a problem

See [Responsible disclosure](disclosure.md).
