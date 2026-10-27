# Claims and the knowledge log

A **claim** is the network's unit of knowledge: a statement with its
evidence, its issuer, the formula of assumptions it rests on, the bonds
behind it and a validity window. Claims never conflict with each other, so
issuing one needs no consensus.

## What a claim holds

The architecture paper lists these fields. The right-hand column shows
where each lives in this implementation (`ClaimBody` in `sion-exec`).

| Field | Holds | In the code |
|---|---|---|
| Statement | A typed predicate over committed data | `schema` (for example `receipt.ai.inference/v1`), `subject` (one line of text) and `payload` |
| Evidence | Proof, attestation or bond reference | Expressed through the formula's atoms; the ledger does not verify proofs or attestations attached to a claim |
| Issuer | An account or an Agent Charter | The transaction's sender; agent receipts are issued by the agent and backed by the charter's principal |
| Trust formula φ | The assumptions the statement rests on | `formula` (see [Atoms and formulas](assurance/atoms-and-formulas.md)) |
| Backing | Bonds locked for the claim's window | Bonded atoms in the formula, which must belong to the claim's backer |
| Dependencies | Claims used as inputs | References to earlier claims inside the formula |
| Window | Validity, then a challenge period | `valid_for` and `challenge_period`, in Pulses |
| Adjudicator | Who settles disputes | `adjudicator`: an atom of class `Adjudicator`, with its own bond |
| Detection factor δ | How reliably a falsehood is provable | `detection_permille` (1000 means always provable) |

A claim's identifier is the hash of its issuer and its canonical body, so
the same body issued by two accounts gives two claims.

## Payloads

A payload is one of three things:

- **Public**: bytes anyone can read.
- **Encrypted**: sealed to an ML-KEM-768 viewing key with ChaCha20-Poly1305.
- **Commitment**: only a hash is published; the owner keeps the data. Its
  availability is then the owner's risk.

## Issuing a claim

An `IssueClaim` transaction travels the owned path like a transfer. During
execution the ledger checks the body against the chain configuration:

- `schema` is 1 to 128 bytes and `subject` at most 512 bytes;
- the payload is at most `max_payload_bytes` (64 KiB on the devnet);
- the formula has at most `max_formula_size` nodes (256) and is not `False`;
- `detection_permille` is at most 1000;
- `valid_for` is between 1 and `max_claim_window` Pulses (1,000,000).

The transaction emits `ClaimSubmitted`. The claim itself joins the
knowledge log at the end of the Pulse, in the deferred phase, in claim
identifier order. There the ledger rejects it (event `ClaimRejected` with a
reason) if:

- an atom in the formula is not registered;
- a bonded atom in the formula belongs to someone other than the claim's
  backer (you cannot rest your claim on someone else's bond);
- a referenced claim is unknown, not active, or was issued in this Pulse or
  later;
- the formula nests deeper than 32 levels, or the chain of claim references
  is longer than 16.

Otherwise the claim is appended to the knowledge log (event `ClaimIssued`
with its position) and the ledger schedules its expiry.

Claims may only reference claims from earlier Pulses, so the knowledge log
never depends on the order of transactions inside a Pulse.

## Status and windows

A claim is **Active**, **Falsified** (with the Pulse and the broken atoms
that caused it) or **Expired**.

- `valid_until` = the Pulse of issue + `valid_for`.
- `challenge_until` = `valid_until` + `challenge_period`.

After `valid_until` the claim expires. Bonds behind it stay locked until the
challenge period of every claim resting on them has ended.

## The knowledge log

The knowledge log is an append-only Merkle log in the style of RFC 9162.
Each leaf hashes the claim's identifier, its issuer, a hash of its body and
its Pulse of issue. The log's root and size are in every world commitment,
so a light client checks that a claim is in the log with one inclusion path
against a certified Pulse header (`verify_claim` in `sion-light`). The
`GET /v1/claims/{id}` route returns that path.

## Standard formulas

`sion_exec::claim::formulas` builds the patterns the architecture uses:

| Formula | Meaning |
|---|---|
| `dual_signature()` | `module-lattice ∨ sha2-256`: forging the issuer's signature needs both families broken |
| `bonded_statement(bond)` | `dual_signature ∧ bond`: the statement is as good as the issuer's bond |
| `dual_proof(circuit_a, circuit_b)` | `specification ∧ sha3-256 ∧ ((stack A ∧ circuit A) ∨ (stack B ∧ circuit B))` |
| `witness_quorum([w1, w2, w3])` | `w1 ∨ w2 ∨ w3`: one honest bonded witness suffices |
| `inference_receipt(tee, bond)` | `dual_signature ∧ (tee ∨ bond)`: an AI receipt backed by a TEE quote or the operator's bond |

`sion claim issue` uses `dual_signature()`, or `bonded_statement` with
`--bond <label>`.

## Receipts are claims

Two kinds of receipts are claims too:

- An agent paying under an Agent Charter with receipts on issues a
  `receipt.agent.payment/v1` claim for every payment, backed by the
  charter's agent bond. See [Agent Charters](agent-charters.md).
- An AI provider can issue `receipt.ai.inference/v1` claims; the `receipts`
  demo shows one priced under three policies.

The architecture paper's example schema for an AI inference receipt:

```json
{
  "claim_id": "hash of the canonical encoding",
  "schema": "receipt.ai.inference/v1",
  "statement": {
    "model": "sha3-256 of the model weights or provider model ID",
    "input_commitment": "hiding commitment to prompt and context",
    "output_hash": "sha3-256 of the output"
  },
  "evidence": [
    { "kind": "tee_quote", "vendor": "GPU vendor", "quote": "..." },
    { "kind": "operator_bond", "bond_ref": "lane object ID", "amount_usd": 50000 }
  ],
  "trust_formula": "OR(atom:tee.resists_host, atom:bond.operator)",
  "issuer": "account ID",
  "charter": "charter ID, if issued by an agent",
  "dependencies": [],
  "adjudicator": "claim ID of the arbitration service and its bond",
  "window": { "valid_until": "2027-01-31", "challenge_days": 14 },
  "detection_factor": 0.5
}
```

This is the design's schema. The implementation's `ClaimBody` carries the
same information with the evidence expressed as atoms and the statement in
`subject` and `payload`.

## In this implementation

- **Built**: claims, payload privacy modes, the knowledge log, inclusion
  proofs, claim references, windows, expiry.
- **Not built**: verification of evidence inside the ledger (TEE quotes,
  zero-knowledge proofs of inference), batch claims that commit to millions
  of statements under one Merkle root (a claim can carry such a root as its
  payload, but nothing checks it), and personhood credentials.
