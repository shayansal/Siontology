# Reservations

Cost of corruption is per claim, but reliance adds up across consumers.
Three agents each risking 25,000 USD on a claim backed by 60,000 USD would
all pass the decision rule at κ = 2, and a fraud would net 75,000 USD for
60,000 USD forfeited. So every acceptance **reserves** κ · V of the claim's
backing, and no claim is relied on beyond what slashing can repay.

## Pushing a reservation down the formula

A reservation of κ · V is pushed down the claim's formula:

- **every `∧` input carries the full reservation**, because breaking any
  one input breaks the claim;
- **the `∨` alternatives share it**, because all of them must be broken;
- **an atom carries the sum** of every reservation that reaches it, and no
  bond can be reserved beyond its unslashed amount.

Atoms the consumer's policy trusts absorb reservations without limit: the
consumer accepts the risk that they break. Atoms that are neither trusted
nor bonded (a TEE's attack cost, for example) have no capacity: they may
deter, but they cannot compensate.

Then, for any set of broken atoms, the value relying on the falsified claims
is at most the slashed bonds divided by κ, so victims can be paid in full.

When an atom occurs several times in a formula (claims sharing an input),
the implementation splits the atom's capacity evenly across its
occurrences. That is sound, but it can refuse a reservation that an exact
linear program would admit.

## The transaction

`Accept { claim, value_at_risk, policy, until, nonce }` applies the decision
rule with the consumer's policy and, if it passes, records a reservation:
the claim, the consumer, V, κ, κ · V, the end of reliance (`until`), the
amount reserved at each bonded atom, and a digest of the policy. The
reservation's identifier hashes the claim, the consumer and the nonce, so a
consumer can hold several reservations on one claim.

- `Release { reservation }` ends a reservation early. Only its consumer
  may release it.
- At `until`, the registry releases the reservation automatically.

`Accept` and `Release` are contended transactions in the assurance domain.

## On the devnet

```text
$ sion accept erin b677314a931e86fee2403fe437822ec7d087f0df37dbd9452735b16177c41634 10 --pulses 500
erin signs: Rely on claim b677314a for up to 10 USD until Pulse 548, reserving its backing. Fee 0.0012 USD; valid until Pulse 68.
Pulse 51: Done: reserved 20 USD of claim b677314a's backing; fee 0.0012 USD.
  digest 0a062fa05c2005e87f84aa876d586c31930544f852d40e9d83a86e65be5ae02d
```

After it, the same claim's decision reports `"reservable": {"Finite":
30000000}`: 30 of its 50 USD of backing remain.

`sion accept` uses the standard policy with `--kappa` (default 2) and
`--pulses` of reliance (default 1,000). There is no CLI command for
`Release`.

## Uninsured reliance

Relying on a claim without a reservation is allowed, but uninsured: if the
claim is falsified, only reservations are compensated.

## In this implementation

- **Built**: reservations, their allocation over bonded atoms, release,
  expiry, compensation of reservations on falsified claims, and *degraded*
  reservations (an atom carrying part of the reservation broke, but the
  claim still holds).
- **Different from the architecture**: the paper runs reservations against
  a bond's capacity on [Escrow Lanes](../escrow-lanes.md), so that most
  reservations avoid consensus. Here every reservation is a contended
  transaction on the single assurance registry.
