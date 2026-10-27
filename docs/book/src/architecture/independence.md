# Keeping proof stack B independent

A World Proof verifies only if two proof stacks accept every chunk. That is
worth something only if the stacks can fail independently: a bug in one
must not be a bug in the other. Stack A is the reference executor
(`sion-exec`) replayed statelessly. Stack B (`sion-spec`) is a second
implementation written only from the
[specification](../specification.md). This chapter describes how the
repository keeps B independent, and what the two still share.

## The rule

§0 of the specification says what B may and may not use.

**May not call** the reference executor's logic: `sion_exec::execute`,
`sion_exec::pulse` (beyond its type definitions and the commitment helpers
of §7), `sion_exec::stateless`, `sion_exec::wallet`, `sion_exec::sim`,
`sion_exec::genesis`, nor the methods `Allotment::spend`,
`Caveats::attenuates`, `OrderTerms::escrow`, `Market::clearing_batch`,
`Market::batch_index`, `Equivocation::offender` and the `ValidatorSet`
methods.

**May use** the data layouts (the Rust type definitions, whose canonical
encoding is Borsh, are normative), a short list of type helpers
(`Transaction::digest`, `ChainConfig::min_fee`, `claim_id`,
`possession_digest`, `rotation_seed`, `weight`, `permille_of`, `share_of`
and others named in §0), and the shared primitives below.

## The lint

`scripts/check-independence.sh` searches B's library sources (comments and
unit-test modules excluded, since unit tests cross-check helpers against
the reference on purpose) for the forbidden paths and method calls, and
for any item of `sion_exec::pulse` other than `ChunkStatement`,
`ChunkTrace`, `Witness` and `PulseInput`. CI runs it on every push:

```text
$ ./scripts/check-independence.sh
Proof stack B is independent of the reference executor (21 files checked).
```

It is a lint, not a proof. Review still matters: a function copied into B
under another name would pass it.

## What the stacks share

Both stacks use, and the World Proof's formula trusts as separate atoms
(`impl:shared-primitives`):

- hashing and Merkle structures (`sion-core`);
- signature verification, validator identities and signatures
  (`sion-crypto`);
- the Assurance Algebra: registry operations, cost of corruption,
  reservations, the decision rule, slashing (`sion-assurance`);
- clearing-certificate verification (`sion-clearing`);
- committee envelopes, committee sampling and the consensus message formats
  (`sion-consensus`).

A bug in any of these is common-mode: both stacks would accept the same
wrong answer. Two proof stacks with different arithmetizations, as the
architecture intends, would share less.

## The differential tests

`crates/sion-spec/tests/differential.rs` runs the same chunks through both
stacks: every transaction kind, random workloads, boundary probes (amounts,
windows and deadlines near `u64::MAX`, maximal batches, deep formulas) and
tampered traces that both must reject. A disagreement means one of them is
wrong, and the test says which chunk and why.

They have found real defects:

- **v1.1**: a market clearing deadline near `u64::MAX` overflowed when its
  batch sealed (any account could halt the chain), and a charter window
  near `u64::MAX` made the reference executor disagree with the
  specification. Both were fixed in the reference executor, and several
  gaps in the specification were closed.
- **v1.3**: when B implemented validators and epochs, the stacks disagreed
  on whether an epoch's end writes the treasury's USD entry when the
  treasury had none and nothing is paid. The specification now says it
  always does, as the reference executor did.

## Changing the state transition

Because two implementations must agree, a change to what a transaction or
a Pulse does follows one order:

1. change the specification, `docs/spec/state-transition.md`, and add an
   entry to its **Changes** section;
2. change the reference executor (`sion-exec`);
3. change stack B (`sion-spec`) from the specification text alone;
4. extend the differential tests until both agree, and run
   `./scripts/check-independence.sh`.

`CONTRIBUTING.md` in the repository states the same rule.
