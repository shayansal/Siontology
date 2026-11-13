# sion-zk

Zero-knowledge proofs about committed numbers. Someone who holds numbers it
will not show commits to them, and proves to anyone holding only the
commitments that the numbers keep a rule. The proof shows nothing else about
them. It is what lets an operator show that its agent's payments stayed
within their limits, or that its record's figures meet a threshold, without
handing over the payments or the figures.

Part of [Siontology](../../README.md) · [Architecture](../../ARCHITECTURE.md)
· Status: **Built** (commitments, proofs of linear rules and of membership,
with tests; used off the ledger for certificates and sealed records, not by
consensus; see [implementation status](../../docs/implementation-status.md)).

## What can be proven

| Proof | Functions | What it shows |
|---|---|---|
| Linear rules over amounts | `commit`, `prove`, `verify` | Each committed amount is a whole number from 0 to 2⁶⁴ − 1, and every statement `a₁·v₁ + a₂·v₂ + … ≤ b` holds, with whole coefficients that may be negative |
| Membership | `name`, `commit_name`, `prove_member`, `verify_member` | A committed name is one of a public list, without showing which |

"No payment was above 10 USD", "they came to at most 500 USD" and "at most
one call in fifty failed" are linear rules. "Each payment went to a payee on
the list" is membership.

## How

- Commitments are Pedersen commitments on the Ristretto group
  (`curve25519-dalek`).
- A linear rule is turned into a commitment to its slack, `b − Σ aᵢ·vᵢ`,
  which anyone computes from the commitments. The slack and every amount are
  shown to be whole numbers below 2⁶⁴ by aggregated Bulletproofs range proofs
  (the `bulletproofs` crate), 64 numbers to a proof.
- Membership is a one-of-many Schnorr proof (Cramer, Damgård and
  Schoenmakers), made non-interactive with a Merlin transcript.
- Every proof is bound to a context the caller gives: it does not hold for
  another.
- There is no trusted setup.

## What it rests on

- **Hiding rests on nothing.** A commitment says nothing about its number,
  whatever computer looks at it.
- **Soundness rests on discrete logarithms** in the Ristretto group being
  hard. Someone who can compute them, as a large quantum computer could,
  can prove a false statement. These proofs are for showing third parties
  that rules were kept. They are not used by consensus, and are not fit for
  anything that would create or move money on a ledger with post-quantum
  accounts.
- **A proof is about the numbers committed to.** That those are the numbers
  of what really happened has to come from when and by whom the commitments
  were made.

## Tests

`cargo test -p sion-zk`: rules that hold are proven and verified; rules that
do not hold have no proof; a proof does not pass for another rule, other
commitments or another context; a number that is not a whole amount cannot
be slipped in; ratios; more numbers than one range proof takes; membership,
and its failures.
