# Validators, staking and epochs

Validity comes from proofs. Ordering, liveness and censorship resistance
still rest on stake: fewer than a third of each committee's stake may be
Byzantine. This chapter explains who the validators are, how they join and
leave, and how committees follow from the state.

## In the architecture

- **Sampled committees.** Committees of 301 seats are sampled by stake. If
  an adversary holds a fifth of all stake, the chance that a committee has
  a Byzantine third is about 2.5 × 10⁻⁸; if it holds a quarter, about
  5.5 × 10⁻⁴. Stake concentration is therefore monitored, and committees
  grow when it rises.
- **Stake buys seats, misbehaviour costs stake.** Validators stake to join,
  earn a share of fees, and forfeit their stake on signed evidence of
  equivocation.
- **Value in flight.** Before a validity proof lands, two thirds of a
  committee could certify an invalid light operation (the Fold would fail
  to prove it and the signers would be slashed), and more than a third can
  certify two conflicting spends. So the network caps value in flight,
  certified but not yet proven, at the committee's slashable stake.
- **Throughput** grows with the number of shards until data availability
  or signature bandwidth binds.

## In this implementation

Who certifies is part of the state transition. The **validator set** is an
object in the state, next to the treasury and the assurance registry
(specification §2):

| Field | What it holds |
|---|---|
| `params` | Fixed at genesis: `stake_asset`, `epoch_length`, `min_stake`, `unbonding`, `seats`, `max_active`, `reward_permille`, `reporter_permille` |
| `epoch`, `epoch_start` | The current epoch and its first Pulse |
| `seed` | The randomness this epoch's lane committees are sampled with |
| `active` | The active set: (validator, weight) pairs |
| `validators` | Every validator ever registered: keys, operator, stake, status, unclaimed rewards, beacon chain tip |
| `rotations` | Shards whose lane committee was replaced this epoch |
| `fees_mark` | The treasury's running total of fees when the last epoch ended |

Every validator derives the committees from this object after every
Pulse. Nothing about committees is configured:

- The **consensus committee** is the active set. Each validator's weight is
  its stake in whole units of the stake asset, its own plus its pool's
  earning stake ([Pooled stake](validators/pooled-stake.md)): 100,000 USD
  of its own and 5,000 USD delegated is weight 105,000.
- Each shard's **lane committee** is `seats` validators drawn, by stake and
  without replacement, from the active set with the epoch's seed. Each
  member holds one seat. A shard that misses a forced-inclusion deadline
  gets a new lane committee; see
  [Censorship resistance](censorship-resistance.md).
- A **quorum** is more than two thirds of the committee's weight:
  `⌊2·total/3⌋ + 1`. With four validators of 100,000 each, that is 266,667,
  so any three of the four.

The set changes only through transactions and the deferred phase of a
Pulse, by the rules of the specification (§5.26–5.30, §6.3). Both proof
stacks check those rules, so the World Proof covers every change of who
certifies.

### A validator's life

| Status | How it gets there | Holds a seat |
|---|---|---|
| Joining | `RegisterValidator`: stake plus a proof that the operator's validator holds its keys | No; at the next epoch boundary it becomes Active |
| Active | The epoch boundary after joining (genesis validators start Active) | Yes, if its own plus pooled earning stake ranks among the top `max_active` |
| Leaving | `Unstake` while Active | Yes, until the next epoch boundary |
| Exited | The boundary after Leaving (withdrawable `unbonding` Pulses later), or `Unstake` while Joining (withdrawable at once) | No |
| Withdrawn | `WithdrawStake` after unbonding: stake and unclaimed rewards return to the operator | No |
| Slashed | `ReportEquivocation` with two conflicting signed messages, in any status except Slashed and Withdrawn | Until the next epoch boundary |

The sub-chapters cover each step:

- [Staking](validators/staking.md): joining, leaving and withdrawing.
- [Epochs and committees](validators/epochs.md): what happens at a
  boundary, consensus sessions, and how light clients follow the
  committee.
- [Rewards](validators/rewards.md): how fees reach validators.
- [Slashing for equivocation](validators/equivocation-slashing.md): the
  evidence, the penalty and the reporter's share.
- [Pooled stake](validators/pooled-stake.md): delegating to a validator
  without handing the stake over, the reward split, the commission and the
  shared risk.

### The devnet's values

| Parameter | Devnet value | Set with |
|---|---|---|
| Genesis validators | 4, each staking 100,000 USD | `sion devnet --validators` |
| Standby validators (run from the start, join by staking) | 0 | `--standby` |
| Epoch length | 0: one epoch that never ends | `--epoch-length` |
| Unbonding | 20 Pulses | `--unbonding` |
| Seats per lane committee | 4 | `--seats` |
| Minimum stake | 10,000 USD | fixed |
| Most validators in the active set | 100 | fixed |
| Rewards | 70 % of the fees collected in an epoch | fixed |
| Reporter's share of a slash | 5 % | fixed |
| Stake asset | USD | fixed |

With the defaults, the validator set never changes. To see it change, start
a devnet with a standby validator and short epochs, or let the staking demo
start one:

```bash
sion demo staking --local                       # its own devnet: 1 standby, 24-Pulse epochs
sion devnet --standby 1 --epoch-length 24       # or a devnet you keep using
```

The guide [Staking and epochs](../guides/staking.md) walks through both.

## What is simplified or missing

- **The consensus committee is the whole active set** (at most 100
  validators), not a sample. Lane committees are sampled.
- **A slashed validator keeps its seat** and its weight in quorums until
  the epoch ends (trust atom `impl:slashed-seat`).
- **Epoch hand-over relies on reliable delivery.** At a boundary each
  validator starts a new consensus session; one that has not reached the
  boundary finishes the old session from messages already sent, which the
  in-process network always delivers. A real network needs a drain period
  (`impl:reliable-handover`).
- **Evidence is not bound to a chain or a time.** An operator must not use
  a validator's keys on more than one network (`impl:evidence-scope`).
- **No command registers a validator yet.** The CLI, the HTTP API and the
  MCP server have no command for `RegisterValidator`, `Unstake`,
  `WithdrawStake`, `ClaimRewards` or `ReportEquivocation`. They are
  transaction kinds in the Rust API (`sion_exec::TxKind`), and `sion demo
  staking` uses them. Delegating has commands: `sion stake …`
  ([Delegating stake](../guides/delegation.md)).
- **Equivocation is reported by hand.** A validator that sees two headers
  from one author for one round logs it but does not submit the report;
  anyone holding the two signed messages can.
- **Not built**: the value-in-flight cap and payments to provers by the
  protocol. A requester can pay for a Pulse's proof from its own escrow
  ([A market for proofs](../guides/proving.md)).

`sion_consensus::capture_probability(byzantine_fraction, seats)` computes
the chance that a sampled committee is captured, as a binomial tail. It is
a reporting statistic.
