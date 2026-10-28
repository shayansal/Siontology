# Research

Siontology's code implements one architecture. Its research asks where that
architecture breaks, and what would have to be true for it to go much
further. This part of the book collects the notes. Each says what is built,
what is measured, and what is only a plan.

## The bet: count payments, not transactions

A general-purpose ledger is bounded by what its validators can check and
prove each second. Our [succinct-proof plan](succinct-proofs.md) puts
numbers on that bound: 100,000 operations a second, the Gate 3 target,
needs about 500 to 2,000 prover machines even with the signature changes
it proposes; a billion operations a second would need millions. No amount
of engineering on one design closes that gap.

Most of the traffic a ledger would carry at that scale is not general,
though. It is machines paying machines, a little at a time, over and over:
an agent paying an API per call, a device paying a network per megabyte,
a service paying a model per token. For that traffic we separate the two
things a ledger does for a payment:

- **Verification**, when the payment is made. In a payment stream, each
  payment is one element of the payer's hash chain, and the payee checks
  it with one hash. Nothing reaches the ledger.
- **Settlement**, when money moves. With checkpoints, the ledger settles
  any number of payments for a bounded number of hashes and one signature,
  and anyone can check any payment against what it settled.

On one core of a small cloud machine, settlements covering about
**9 × 10⁸ payments a second** executed with every signature checked. The
note [Billions of payments a second](billion-scale.md) has the
construction, the safety argument, the capacity law, the measurements and
what remains unknown.

**What this is, honestly.** It is a measured separation and a design that
exploits it, not a theorem, and not new cryptography: hash-chain payments
date from 1996. We have not found the combination elsewhere, and we
publish it so that others can check, break or improve it. A discovery is
something the field agrees on after scrutiny; this is a claim, ready for
it.

## The notes

| Note | Question | Status |
|---|---|---|
| [Billions of payments a second](billion-scale.md) | Can a ledger settle billions of verifiable payments a second without validating billions of anything? | Built and measured on one machine (spec v1.6 and v1.7) |
| [Succinct validity proofs](succinct-proofs.md) | How do replay proofs become real succinct proofs, in two independent stacks, without giving up post-quantum security? | Plan; nothing built |
| [Networking between machines](networking.md) | How do validators on separate machines talk, and how far does the current consensus scale? | Plan; two node fixes it found are built |

## Open problems we would like help with

1. A lower bound on the cost of publicly verifiable, post-quantum payments
   without a signature each.
2. Netting: paying many payees from one escrow, with settlement cost
   independent of the number of payees.
3. Proving SHA3-256 hash walks cheaply inside hash-based proof systems.
4. Pricing proving work in fees, so that what is cheap to execute but
   expensive to prove cannot be used to stall the prover.
5. A composition theorem for the Assurance Algebra: when two claims each
   cost at least `C` to corrupt, what does a claim built on both cost?

Write to the maintainers (see [Responsible disclosure](../security/disclosure.md)
for the address) or open an issue once the repository is public.
