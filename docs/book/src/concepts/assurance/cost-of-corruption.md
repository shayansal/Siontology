# Cost of corruption

The **cost of corruption** of a claim is the cheapest total cost of a set
of atom failures that makes its formula false. It is relative to the
consumer's **policy**: the set of assumptions the consumer is willing to
trust.

## The cost of one atom

Under a policy, an atom costs:

| Situation | Cost |
|---|---|
| The atom is broken (proven false), or the policy assumes it broken | 0 |
| The atom is not registered | 0 (an unknown atom backs nothing) |
| The policy trusts the atom, or its class, and does not distrust it | ∞ |
| Otherwise | its unslashed bond (if bonded) plus its known attack cost (if any) |

The **standard policy** (`Policy::standard()`) trusts the `Cryptographic`
and `Specification` classes and nothing else, with κ = 2. The **paranoid
policy** trusts nothing. A consumer can also trust individual atoms,
distrust individual atoms (even if their class is trusted), and assume
atoms already broken, for example a TEE after a published physical attack,
before any adjudicator rules.

| Atom class | Under the standard policy |
|---|---|
| Cryptographic (hashes, lattices) | ∞ |
| Specification | ∞ |
| Code (circuits, proof stacks) | 0, unless the policy trusts it |
| Hardware (a TEE) | Its attack cost, for example 1,000 USD; never ∞ by default |
| Institutional | Its bond, if posted |
| Economic | Its bond |
| Adjudicator | Its bond |

## Combining

The cost is computed bottom-up:

```text
CoC(atom)    = cost(atom)
CoC(φ1 ∧ φ2) = min(CoC(φ1), CoC(φ2))     breaking any input breaks the claim
CoC(φ1 ∨ φ2) = CoC(φ1) + CoC(φ2)         every alternative must be broken
```

The sum rule is exact when the alternatives share no priced atom. When they
do, the exact value is a minimum-weight hitting-set problem, NP-hard in
general. The implementation then reports a **sound interval**:

- **low**: the maximum of the alternatives' costs (breaking `φ1 ∨ φ2`
  means breaking both, which costs at least the dearer one);
- **high**: the sum.

Every value is reported as `{ low, high, exact }`. The decision rule always
uses the low end. `sion_assurance::exact_coc` computes the exact value by
branching over shared atoms when there are few of them. Schemas can require
atom-disjoint alternatives, which restores the exact sum.

## Two consequences

- **Independent evidence adds up.** Three disjoint witnesses bonded at
  20,000 USD each give a cost of corruption of 60,000 USD. Truth gets harder
  to fake as participation grows.
- **Sybils gain nothing.** One party running all three witnesses still
  forfeits all three bonds once the lie is proven.

## What it does not count

Cost of corruption counts stake that is slashable **once a falsehood is
proven**. A lie nobody can disprove costs nothing to tell. Claims that are
rarely disprovable carry a **detection factor** below 1, which the
[decision rule](decision-rule.md) applies, and agents should treat them as
weak.

An attack cost (such as a TEE's) deters but compensates nobody: only bonds
can be slashed.

## On the devnet

The `receipts` demo prices one AI inference receipt,
`dual-signature ∧ (TEE quote ∨ bond)`, under three policies:

```text
Pulse  80  receipt issued  [dual-signature ∧ (TEE quote: nvidia-h100 ∨ bond:inference-co)]
           standard policy: cost to forge = 6,000 USD  (TEE attack ≈ 1,000 USD + 5,000 USD bond)
           the TEE's attack cost deters forgery but cannot repay anyone; only the bond is slashable
           standard           relying on 2000 USD → ACCEPT: cost of corruption 6,000 covers κ·V = 4,000
           standard           relying on 3000 USD → REJECT: only 5,000 of backing is unreserved; κ·V = 6,000 is needed
           assumes TEE broken relying on 2000 USD → ACCEPT: cost of corruption 5,000 covers κ·V = 4,000
           assumes TEE broken relying on 3000 USD → REJECT: cost of corruption 5,000 (after detection discount 1000/1000) is below κ·V = 6,000
           trusts the TEE     relying on 2000 USD → ACCEPT: cost of corruption ∞ covers κ·V = 4,000
           trusts the TEE     relying on 3000 USD → ACCEPT: cost of corruption ∞ covers κ·V = 6,000
```

Under the standard policy, the cost of corruption (6,000 USD) covers
κ·V = 6,000 USD in the 3,000 USD case, but the claim is still refused: only
the 5,000 USD bond can be reserved, and the TEE's attack cost cannot (see
[Reservations](reservations.md)).

`GET /v1/claims/{id}` reports the cost of corruption under the standard
policy as `coc_standard_policy`; `POST /v1/claims/{id}/decide` takes any
policy.
