# Staking

A validator joins by staking, serves from the next epoch boundary, and
leaves the same way it came: at a boundary, with its stake locked for an
unbonding period. This chapter describes the three transactions involved,
specified in §5.26–5.28 of the specification.

Every validator-set transaction touches the validator set, one shared
object, so all of them are **contended**: their inputs are locked by a lane
certificate like any transaction's, and consensus orders them.

## Joining: `RegisterValidator`

`RegisterValidator { lattice, hash, beacon_tip, stake, possession }` is sent
by the validator's **operator**, the account that stakes and later receives
rewards and withdrawals. Execution checks, in order:

1. The validator's identifier is derived from its two public keys
   (ML-DSA-44 and SLH-DSA). It must not be registered already.
2. `stake` is at least `min_stake` (10,000 USD on the devnet).
3. `possession` is a dual signature by the validator's keys over
   `possession_digest(config, operator, validator)`, a hash of the chain
   configuration, the operator and the validator. It proves that the
   operator's validator holds both keys. Because it names the chain and the
   operator, nobody can replay the registration on another chain or claim
   someone else's validator.
4. The stake is drawn from the operator's coins of the stake asset (USD on
   the devnet).

The record is stored with status **Joining**, and the event
`ValidatorRegistered` is emitted. At the next epoch boundary the validator
becomes Active and, if its stake ranks among the top `max_active` (100 on
the devnet), joins the active set. `beacon_tip` is the tip of the
validator's beacon hash chain: once active, it reveals the chain's
preimages one Pulse at a time, as every validator does
([Pulses](../pulses.md)).

## Weight

A validator's consensus weight is its stake in whole units of the stake
asset (10⁶ micro-units), at least 1 and at most 10¹². A validator staking
50,000 USD weighs 50,000; one staking 100,000 USD weighs twice as much.

At each boundary the next active set is chosen from the Active records:
sorted by stake, largest first, ties broken by identifier, the first
`max_active`. If none is eligible, because every validator that could serve
was slashed, the next active set is empty and the chain halts
([Epochs](epochs.md)).

## Leaving: `Unstake`

`Unstake { validator }` must come from the validator's operator.

- A **Joining** validator becomes **Exited**, withdrawable at once: it
  never served.
- An **Active** validator becomes **Leaving**. It keeps its seat, its
  weight and its share of rewards until the epoch ends. At the boundary it
  becomes **Exited**, withdrawable `unbonding` Pulses later (20 on the
  devnet). The last validator staying cannot leave, but nothing keeps more
  than one: operators can leave until a single validator forms every
  quorum (`impl:active-floor`).

The unbonding period keeps the stake slashable after the validator stops
serving: equivocation evidence from its last epoch can still be reported.

## Withdrawing: `WithdrawStake`

`WithdrawStake { validator }` from the operator, once the validator is
Exited and its `withdrawable_at` Pulse has come, pays the stake (in the
stake asset) and any unclaimed rewards (in USD) to the operator as new
coins. The record stays, with status **Withdrawn** and nothing at stake.
A withdrawn validator has nothing left to slash.

## When a step is refused

The reference executor fails the transaction with these reasons:

| Situation | Reason |
|---|---|
| The validator is already registered | `validator already registered` |
| The stake is below the minimum | `stake … below the minimum …` |
| The proof of possession does not verify | `no proof of possession of the validator keys` |
| The sender is not the operator | `only the validator's operator may do this` |
| No such validator | `unknown validator` |
| The last staying validator unstakes | `the last validator cannot leave` |
| Unstaking a validator that is neither Active nor Joining | `the validator is neither active nor joining` |
| Withdrawing before `withdrawable_at` | `stake is withdrawable from Pulse …` |
| Withdrawing a validator that has not exited | `only an exited validator's stake can be withdrawn` |

## On the devnet

A devnet started with `--standby N` runs N extra validators from the
start. They execute every Pulse and follow the chain, but hold no seat. A
devnet validator with index `i` is operated by the deterministic devnet
wallet `operator-<i>`; with four genesis validators, the first standby
validator is index 4, operated by `operator-4`.

`sion demo staking --local` starts a devnet with one standby validator and
24-Pulse epochs, and stakes for it:

```text
== Staking: committees come from the state, epoch by epoch ==
…
           epochs last 24 Pulses; this one ends after Pulse 24
Pulse   6  operator-4 stakes 50,000 USD for validator a6f32863, with a proof that it holds the validator's keys
           Done: validator a6f32863 registered by operator-4 with 50,000 USD at stake; fee 0.0032 USD.
Pulse  24  epoch 1 begins: 5 validators hold seats, the newcomer among them; 0.00224 USD of fees paid as rewards, split by stake
```

There is no CLI command for these transactions yet. The demo builds the
registration with `sion_node::devnet::registration`, which derives the
standby validator's keys from the chain name. That works only because
devnet keys are public (`impl:devnet-keys`). On a real network the
operator would build the proof of possession on the validator's own
machine.
