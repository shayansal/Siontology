# Rewards

Validators are paid from fees, and from fees only. At the end of each
epoch, a share of the fees collected during the epoch is paid to that
epoch's validators, in proportion to their weight. Rewards are paid in
USD.

## The rule

Every transaction's fee is credited to the treasury, which keeps a running
total of the fees it has ever collected (specification v1.4). In the
deferred phase of an epoch's last Pulse:

1. The epoch's fees are the treasury's running fee total now, minus
   `fees_mark`, the total when the last epoch ended.
2. The pool is `reward_permille` thousandths of the epoch's fees, rounded
   down. On the devnet `reward_permille` is 700, so 70 %.
3. Each validator in the epoch's active set whose record is Active or
   Leaving receives `pool × weight / total`, rounded down, where `total` is
   the weight of the whole active set.
4. What is not paid stays in the treasury: the other 30 % of the fees,
   rounding, and the shares of validators slashed during the epoch.
5. `fees_mark` becomes the current fee total, so the next epoch counts only
   new fees.

Other money reaches the treasury too: the treasury's share of a slashed
bond, the remainder of a slashed stake after the reporter's share, a
slashed pool's tenth, forfeited storage collateral, and
slashed-bond funds no longer needed to compensate anyone. It stays there.
It is never paid out as rewards, so slashing a validator does not enrich
the others.

**Pooled stake** (specification v1.11). A validator with delegators
shares its reward with its pool, in proportion to their earning stake
against its own, and keeps a commission on the pool's part; see
[Pooled stake](pooled-stake.md). The total paid is unchanged: the pool's
part comes out of the validator's share.

Rewards accrue on the validator's record. The operator collects them with
`ClaimRewards { validator }`, or receives them with the stake on
`WithdrawStake`. Unclaimed rewards are forfeited if the validator is
slashed. Claiming with nothing accrued fails with `no rewards to claim`.

## An example from the devnet

From one run of `sion demo staking --local` (24-Pulse epochs, four genesis
validators of weight 100,000):

```text
Pulse  24  epoch 1 begins: 5 validators hold seats, the newcomer among them; 0.00224 USD of fees paid as rewards, split by stake
…
Pulse  29  operator-2 collects: Done: 0.00056 USD of rewards claimed for validator 04dde303; fee 0.0012 USD.
```

During epoch 0 the only fee was the registration's, 0.0032 USD. The pool
was 70 % of it, 0.00224 USD, shared by the four genesis validators: 0.00056
USD each. At Pulse 29, `operator-2` claimed its validator's share.

In epoch 1 of the same run, the newcomer was slashed for 50,000 USD:
2,500 USD went to the reporter and 47,500 USD to the treasury. None of that
is a fee. The fees collected in epoch 1 were 0.0062 USD (the reward claim
and the report), so the pool at the epoch's end is 0.00434 USD; each genesis
validator, at 100,000 of the active set's 450,000 weight, receives 0.000964
USD, and the slashed validator's share stays in the treasury.

The devnet's fees are tiny because the devnet is idle. What matters is the
shape: validator income is a fixed share of what users pay to use the
network.

## What is not built

- **Payments to provers.** The architecture pays provers from fees too.
  The devnet runs one prover thread and pays it nothing.
