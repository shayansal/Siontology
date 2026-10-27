# Atoms and formulas

## Atoms

An **atom** is one irreducible assumption: a hash function's collision
resistance, the hardness of a lattice problem, a TEE resisting its own host,
a circuit meeting its specification, an attester's honesty.

An atom has:

| Field | Meaning |
|---|---|
| `id` | The hash of its class and label; two atoms with the same class and label are the same atom |
| `class` | What kind of assumption it is (below); policies can trust whole classes |
| `label` | Human-readable, for example `sha3-256` or `bond:<account>:deliveries` |
| `attack_cost` | What an attacker must spend to break it, if known (for example the cheapest known physical attack on a TEE). It deters but is not slashable, so it compensates nobody |
| `bonded` | Whether a slashable bond backs it |

### Classes

| Class | Examples |
|---|---|
| `Cryptographic` | SHA3-256, SHA-256, module-lattice hardness |
| `Code` | A program, circuit or proof stack meets its specification |
| `Specification` | The protocol specification itself is correct |
| `Hardware` | A TEE resisting its own host; a sensor's secure element |
| `Institutional` | A bank's KYC record; a public registry |
| `Economic` | A bonded party's honesty, backed by its slashable bond |
| `Committee` | Fewer than *t* members of a committee collude |
| `Adjudicator` | An adjudicator rules honestly |

### Standard atoms

Every devnet registers these at genesis:

| Label | Class | Notes |
|---|---|---|
| `sha3-256` | Cryptographic | |
| `sha2-256` | Cryptographic | SLH-DSA-SHA2 rests on it |
| `module-lattice` | Cryptographic | ML-DSA and ML-KEM rest on it |
| the specification | Specification | The state transition specification is correct |
| `proof-stack:A`, `proof-stack:B` | Code | Each proof stack is sound |
| `tee-resists-host:nvidia-h100`, `tee-resists-host:intel-tdx`, `tee-resists-host:amd-sev-snp` | Hardware | Attack cost 1,000 USD. Memory-bus attacks costing under 1,000 USD broke TDX, SEV-SNP and SGX attestation in 2025 and 2026, so a policy should never trust a TEE |

### Atoms anyone can create

- **Bonded atoms.** `PostBond { class, label, amount }` posts a bond behind
  the atom `bond:<sender>:<label>`, of class `Economic`, `Institutional` or
  `Adjudicator`, creating it if needed. The sender's account is part of the
  label, so nobody can post the first bond behind someone else's atom.
  `sion bond` sends this transaction and prints the atom's identifier.
- **Registered atoms.** `RegisterAtom { class, label }` registers an
  unbonded atom of class `Code` or `Hardware`, labelled
  `registered:<sender>:<label>`, for example a circuit. Attack costs cannot
  be set this way; they are set at genesis.

## Formulas

A **trust formula** is a monotone Boolean expression:

```text
φ ::= True | False | atom | claim | φ ∧ φ ∧ … | φ ∨ φ ∨ …
```

- **Joint use (∧).** A claim derived from inputs *c1* and *c2* by a proven
  computation *p* gets `φ(c1) ∧ φ(c2) ∧ φ(p)`. It fails if any part fails.
- **Alternatives (∨).** A claim backed by independent routes (two proof
  systems, two signature schemes, several witnesses) gets the `∨` of the
  routes. It fails only if every route fails.
- **Claim references.** A formula may reference an earlier claim; that
  claim's own formula is substituted when the formula is evaluated, and the
  reference holds only while the claim is active.

These are the two operations of provenance semirings from database theory
([Green, Karvounarakis and Tannen, 2007](https://doi.org/10.1145/1265530.1265535)).

Formulas are normalized when built: nested `∧` and `∨` are flattened,
`True` and `False` are absorbed, duplicates are removed.

### Three patterns

- **Dual-origin signature.** ML-DSA-44 and a hash-based signature are both
  required, so forging needs both schemes broken: `module-lattice ∨ sha2-256`.
- **Dual proof.** Two independent proof stacks must both verify:
  `(stack A ∧ circuit A) ∨ (stack B ∧ circuit B)`.
- **Witness quorum.** Three bonded witnesses sign *shipment received*. An
  honest witness signs only true statements, so `W1 ∨ W2 ∨ W3`.

### What `sion claim show` prints

A claim issued with `sion claim issue dana "Shipment 7 arrived at the
buyer's dock" --bond deliveries` rests on
`(module-lattice ∨ sha2-256) ∧ bond:dana:deliveries`. The API returns it as
JSON:

```json
"formula": {
  "And": [
    {
      "Or": [
        { "Atom": "e3dbbfda0d9ee89121eae204845d238794d5deb9b6bd1976bd1af37ffd5f0d1d" },
        { "Atom": "0d0e4fff3d2ddf3afdaf406be3960a6fe406d300a8e4f61fb09a8c72f999c69b" }
      ]
    },
    { "Atom": "05ab20b32354fee47b00f925cbe55bee5f3c5d80f248e7f9d7f159ac9168b90b" }
  ]
}
```

## Rules the ledger enforces

- Every atom a claim's formula names directly must be registered.
- A claim may rest **directly** only on bonds of its own backer (the
  issuer, or the charter's principal for agent receipts). To rely on
  someone else's bond, reference a claim of theirs.
- Referenced claims must be active and issued in an earlier Pulse.
- A formula may nest at most 32 levels when a claim is issued; one nested
  deeper than 64 levels does not even decode. A chain of claim references
  may be at most 16 claims long. The formula may have at most 256 nodes
  (`max_formula_size`).
