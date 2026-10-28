# Pooled stake

Running a validator takes a server and a minimum stake (10,000 USD on the
devnet). **Pooled stake** lets anyone add stake to a validator they choose,
from one unit up, without running anything and without handing the stake
over. The stake counts towards the validator's weight, earns a share of
its rewards, and shares its risk.

## A delegation is yours

A delegation is an object its delegator owns, like a coin. It holds
**shares** of the validator's **pool**; the pool holds the stake. The
operator never controls it: only the delegator can collect its rewards,
undelegate or withdraw, and there is no pool token to lose or lend.

| Step | Transaction | What happens |
|---|---|---|
| Delegate | `DelegateStake { validator, amount }` | The stake joins the pool; the delegator gets shares (one per micro-unit while the pool's shares are worth face value). The delegation is **pending**. |
| Join | The end of the epoch (no transaction) | Pending delegations start **earning** and count towards the validator's weight for the next epoch. |
| Collect | `ClaimDelegatorRewards` | Pays the rewards credited since the delegation's last claim, in USD. |
| Leave | `Undelegate` | Pays the rewards owed, stops earning, and starts **unbonding**: the stake stays in the pool, and slashable, for the unbonding period (20 Pulses on the devnet). |
| Withdraw | `WithdrawDelegation` | Returns the stake behind the shares, less any slash, and closes the delegation. |

Delegating just before an epoch ends earns nothing for that epoch: a
delegation earns only for epochs it was in the pool for from start to end.

## How rewards are split

Validators are paid from fees only (see [Rewards](rewards.md)). At the end
of each epoch, each validator's reward is split between its own stake and
its pool's earning stake, in proportion to their sizes. The operator keeps
a commission on the pool's part; the rest is credited to the pool per
earning share, with an exact integer accumulator, so a delegator is owed
exactly its shares' part, rounded down.

An example (our arithmetic): validator `v` stakes 100,000 of its own; its
pool's earning stake is 50,000, alice's 30,000 and bob's 20,000; its
commission is 10 %; its reward for the epoch is 30 USD.

| | Amount |
|---|---|
| The pool's part: 30 × 50,000 / 150,000 | 10 USD |
| The operator's commission on it: 10 % | 1 USD |
| Credited to the pool | 9 USD: alice 5.40, bob 3.60 |
| The operator keeps | 20 + 1 = 21 USD |

Rewards stay credited until collected, whatever happens to the validator
later. Rounding left over by the accumulator stays with the pool's
unclaimed rewards.

## The commission

- At most **25 %**.
- It takes effect at the **end of the epoch** in which it is set, after
  that epoch's rewards: a change never applies to an epoch already under
  way, so delegators see it before it applies, and can leave.
- Once anyone has delegated, it rises by at most **2 percentage points** at
  a time (one change per epoch takes effect). A pool nobody has joined may
  start at any commission up to the ceiling. Lowering is always allowed.

## Concentration

A delegation is refused if it would give its validator more than **twice
an equal share** of the bonded stake of all joining and active validators,
and the cap is never below a tenth:

| Eligible validators | Cap on one validator's share |
|---|---|
| 4 | 50 % |
| 5 | 40 % |
| 10 | 20 % |
| 20 or more | 10 % |

Bonded stake here counts the validator's own stake and its pool's earning
and pending stake. The cap bounds what delegation can concentrate; a
validator's own stake is not capped.

## Shared risk

If a validator equivocates, it loses all of its own stake, as before, and
its pool loses **a tenth** of its stake. Every share loses the same
fraction, pending and unbonding delegations included: undelegating does
not escape a slash until the stake is withdrawn. Evidence is not bound to a
time, so a delegation made after the offence but before the report is
slashed too. The reporter's share (5 % on the devnet) is of the
validator's own stake only; the pool's tenth goes to the treasury whole.
(As first written, v1.11 paid the reporter a share of the pool's tenth
too: an operator whose pool was about 190 times its own stake could then
profit from reporting itself through another account, since header
evidence never expires. A review of the economics found it, and it was
changed before release.)

In the example, if `v` equivocates: its 100,000 is forfeited, the pool
loses 5,000, alice's delegation is worth 27,000 and bob's 18,000, and the
reporter receives 5 % of 100,000, 5,000. Delegators keep the rewards
already credited to them.

Choosing a validator is therefore a real decision: its commission, its
record and how much of its own stake it puts first.

## The weight of a validator

At each epoch's end, validators rank for the active set by their own stake
plus their pool's earning stake, and that sum is their weight in consensus
and in sampling lane committees. Unbonding stake does not count.

## In this implementation

- **Built** (specification v1.11, §5.45 to §5.49 and the epoch end in
  §6.3): delegations, shares, joining at the epoch's end, the reward split
  with an exact per-share accumulator, commission rules, the concentration
  cap, collecting, undelegating, unbonding, withdrawing, and the pool's
  share of a slash; the ranking by own plus earning pooled stake. Both
  proof stacks implement it; the differential tests run every rule and
  every way it can fail through both, check every epoch end's split
  against a third, plain-arithmetic reading, and run randomized workloads
  of delegations, claims, commission changes, fees and slashings, checking
  the pools against the delegations they hold after every Pulse.
- **Commands**: `sion stake pools`, `delegate`, `show`, `claim`,
  `undelegate`, `withdraw` and `commission`, and the MCP tools
  `sion_stake_*`. See [Delegating stake](../../guides/delegation.md).
- **Simplified**: every pool lives in one shared object in the validators'
  domain, so delegation transactions are ordered one after another; a pool
  slash is a flat tenth whatever the offence; the stake asset is fixed at
  genesis (the devnet stakes USD, a test asset).
- **Not built**: moving a delegation to another validator without
  unbonding; delegating from an agent's budget; reward compounding.
