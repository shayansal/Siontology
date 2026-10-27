# Insurance

The formula that prices a claim's cost of corruption also prices insurance
against it. Given a failure probability for each atom, and assuming the
atoms fail independently:

```text
P(φ1 ∧ φ2 fails) = 1 − (1 − p1)(1 − p2)
P(φ1 ∨ φ2 fails) = p1 · p2
```

A broken atom fails with probability 1; an atom without a given probability
is treated as never failing.

When an atom occurs in several parts of a formula, the parts are not
independent. `sion_assurance::insurance::failure_probability` handles that
exactly by conditioning on each shared atom (up to a caller-chosen limit,
and never more than 30 shared atoms). Correlated atoms, such as two TEEs
from one vendor, need an explicit correlation model, which this module does
not have.

Probabilities are floating point. That is allowed here because premiums are
reports, not consensus values; no consensus-critical code uses floats.

## Two limits of the model

- **Undetectable lies stay cheap.** Insurance and cost of corruption both
  depend on falsehoods being provable. Claims that are rarely disprovable
  carry a detection factor below 1.
- **Prices come from outside.** The ledger proves formulas and bonds. The
  atom probabilities an insurer uses are its own judgment.

## In this implementation

- **Built**: `failure_probability` as a library function, with tests.
- **Not built**: any interface to it (no CLI command, HTTP route or MCP
  tool), insurance premiums or an assurance marketplace.
