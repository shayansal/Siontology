# Events

Events say what a transaction or a Pulse's deferred phase established. A
transaction's events are part of its effects, which the receipts root
commits to. Deferred-phase events (claims joining the log, batches
sealing, epochs changing, …) belong to the Pulse.

`sion events` and `GET /v1/events?from=<pulse>&limit=<n>` list them as JSON
with the Pulse they happened in; receipts print them as sentences
(`sion_cli::describe::event`). Amounts in JSON are micro-units (10⁻⁶ of the
asset).

```text
$ sion events --from 1 --limit 400
…
Pulse    24  {"EpochChanged":{"epoch":1,"active":5,"rewards":2240}}
…
```

The same event as a sentence: `epoch 1 began with 5 active validators;
0.00224 USD paid in rewards`.

## Money and accounts

| Event | Fields | Emitted by | Sentence |
|---|---|---|---|
| `Transferred` | `asset`, `outputs` [(account, amount)] | `Transfer` | `paid <amount> <asset> to <account>[, …]` |
| `Minted` | `to`, `asset`, `amount` | `Faucet` | `minted <amount> <asset> for <account>` |
| `PolicyRotated` | `account` | `RotatePolicy` | `<account>'s keys rotated` |

## Knowledge and assurance

| Event | Fields | Emitted by | Sentence |
|---|---|---|---|
| `ClaimSubmitted` | `claim` | `IssueClaim` | `claim <id> submitted` |
| `ClaimIssued` | `claim`, `log_index` | deferred phase | `claim <id> joined the knowledge log at position <n>` |
| `ClaimRejected` | `claim`, `reason` | deferred phase | `claim <id> rejected: <reason>` |
| `AtomRegistered` | `atom` | deferred phase | `assumption <id> registered` |
| `BondPosted` | `atom`, `owner`, `amount` | deferred phase | `<owner> posted a <amount> USD bond behind assumption <id>` |
| `Reserved` | `reservation`, `claim`, `amount` (κ·V) | `Accept` | `reserved <amount> USD of claim <id>'s backing` |
| `Released` | `reservation` | `Release` (reservations that expire are released without an event) | `reservation <id> released` |
| `ChallengeOpened` | `challenge`, `claim`, `target` | `Challenge` | `challenge opened against assumption <atom> of claim <id>` |
| `ChallengeRuled` | `challenge`, `upheld` | `Rule` | `challenge upheld` or `challenge rejected` |
| `Slashed` | `atom`, `amount` | `Rule` (upheld) | `<amount> USD slashed from assumption <atom>` |
| `Falsified` | `claim`, `causes` [atoms] | `Rule` (upheld) | `claim <id> falsified` |
| `Compensated` | `reservation`, `consumer`, `amount` | `Rule` (upheld) | `<consumer> compensated <amount> USD` |
| `BondWithdrawn` | `atom`, `amount` | `WithdrawBond` | `<amount> USD withdrawn from the bond behind assumption <atom>` |

## Charters and Escrow Lanes

| Event | Fields | Emitted by | Sentence |
|---|---|---|---|
| `CharterCreated` | `charter`, `allotment` | `CreateCharter`, `Delegate` | `charter <id> created` |
| `CharterRevoked` | `charter` | `RevokeCharter` | `charter <id> revoked` |
| `CharterReclaimed` | `charter`, `amount` | `ReclaimCharter` | `<amount> USD returned from charter <id>` |
| `AgentPaid` | `charter`, `payee`, `amount`, `receipt` | `AgentPay` | `agent paid <amount> USD to <payee>[ (receipt claim <id>)]` |
| `AgentRegistered` | `agent`, `account`, `allotment` | `RegisterAgent` | `agent <id> registered, signing as <account>` |
| `AgentSpawned` | `agent`, `parent`, `account`, `allotment` | `SpawnAgents` (one per subagent) | `subagent <id> launched by <parent>, signing as <account>` |
| `AgentCardSet` | `agent`, `card` | `SetAgentCard` | `agent <id>'s card is now <hash>` |
| `AgentSuspended` | `agent`, `by` | `SuspendAgent` | `agent <id> suspended by <account>, with everything below it` |
| `AgentResumed` | `agent` | `ResumeAgent` | `agent <id> resumed` |
| `BoundedCreated` | `bounded`, `allotments` | `CreateBounded` | `shared total created with <n> allotments` |
| `AllotmentUsed` | `allotment`, `amount` | `SpendAllotment`, `ReturnAllotment`, `Rebalance` | `<amount> USD used from an allotment` |

## Markets

| Event | Fields | Emitted by | Sentence |
|---|---|---|---|
| `MarketCreated` | `market` | `CreateMarket` | `market <id> opened` |
| `OrderSubmitted` | `market`, `order` | `SubmitOrder` | `order entered market <id>` |
| `BatchSealed` | `market`, `batch`, `orders` | deferred phase | `batch <b> sealed with <n> orders` |
| `BatchRevealed` | `market`, `batch`, `revealed` | `RevealBatch` | `<n> sealed orders of batch <b> revealed` |
| `BatchCleared` | `market`, `batch`, `price`, `volume`, `welfare` | `ClearBatch` | `batch <b> cleared: <volume> units at one price <price>`, or `batch <b> cleared with no trade` |
| `BatchRefunded` | `market`, `batch` | deferred phase (clearing deadline passed) | `batch <b> refunded` |

## Validators and epochs

| Event | Fields | Emitted by | Sentence |
|---|---|---|---|
| `ValidatorRegistered` | `validator`, `operator`, `stake` | `RegisterValidator` | `validator <id> registered by <operator> with <stake> USD at stake` |
| `ValidatorLeaving` | `validator` | `Unstake` | `validator <id> is leaving the active set` |
| `StakeWithdrawn` | `validator`, `amount` (stake returned) | `WithdrawStake` | `<amount> USD of stake withdrawn from validator <id>` |
| `RewardsClaimed` | `validator`, `amount` | `ClaimRewards` | `<amount> USD of rewards claimed for validator <id>` |
| `ValidatorSlashed` | `validator`, `amount` (stake forfeited), `reporter`, `reward` (reporter's share of the stake) | `ReportEquivocation` | `validator <id> slashed: <amount> USD forfeited, <reward> USD of it to <reporter> for the report` |
| `EpochChanged` | `epoch` (the new one), `active` (size of its active set), `rewards` (paid from the fees of the epoch that ended) | deferred phase (epoch's last Pulse) | `epoch <e> began with <n> active validators; <rewards> USD paid in rewards` |

## Forced inclusion

| Event | Fields | Emitted by | Sentence |
|---|---|---|---|
| `ForcedPosted` | `intent`, `shard`, `deadline` | deferred phase (Pulse that committed the intent) | `forced intent <id> is now owed by shard <s>, due by Pulse <p>` |
| `ForcedDismissed` | `intent`, `reason` (free text) | deferred phase | `forced intent <id> dismissed: <reason>` |
| `ForcedResolved` | `intent` | deferred phase (deadline: the intent can no longer execute) | `forced intent <id> settled` |
| `ForcedOverdue` | `intent`, `shard` | deferred phase (deadline: the intent could still execute) | `shard <s> missed its deadline for forced intent <id>` |
| `ShardRotated` | `shard`, `rotation` (count this epoch) | deferred phase, after `ForcedOverdue` | `shard <s>'s committee was replaced (rotation <n> this epoch)` |

See [Censorship resistance](../concepts/censorship-resistance.md) for when
each happens.

## Payment streams

| Event | Fields | Emitted by | Sentence |
|---|---|---|---|
| `StreamOpened` | `stream`, `payer`, `payee`, `deposit` | `OpenStream` | `<payer> opened stream <id> to <payee> with <deposit> USD in escrow` |
| `StreamSettled` | `stream`, `position`, `head` (the element at `position`), `amount` | `SettleStream` | `stream <id> settled to step <p>: <amount> USD paid` |
| `StreamToppedUp` | `stream`, `deposit`, `until` | `TopUpStream` | `stream <id> topped up by <deposit> USD, open until Pulse <until>` |
| `StreamClosed` | `stream`, `refund` | `CloseStream` | `stream <id> closed: <refund> USD returned to its payer` |

## Memory logs

| Event | Fields | Emitted by | Sentence |
|---|---|---|---|
| `MemoryOpened` | `log`, `account` (the writer), `agent` | `OpenMemory` | `memory log <id> opened[ for agent <id>]` |
| `MemoryCommitted` | `log`, `size`, `root`, `commits` (the head's sequence number), `sealed` | `CommitMemory` | `memory log <id> now holds <n> atoms (head <k>)[, sealed]` |

The atoms a commit adds have the commit's Pulse as their transaction time.

## Task Charters

| Event | Fields | Emitted by | Sentence |
|---|---|---|---|
| `TaskPosted` | `task`, `poster`, `worker`, `amount`, `deadline` | `PostTask` | `task <id> posted with <amount> USD in escrow, for <worker>\|open to anyone, until Pulse <deadline>` |
| `TaskSettled` | `task`, `worker`, `result`, `amount`, `reservation` | `SettleTask` | `task <id> settled: <amount> USD paid to <worker> for result <id>, its backing reserved` |
| `TaskCancelled` | `task`, `refund`, `to` | `CancelTask` | `task <id> cancelled: <refund> USD refunded to <account>` |

If a claim a paid result rests on is later falsified, the payer's
compensation appears as `Compensated` events of the `Rule` that broke it.

## Failed transactions

A transaction that fails keeps no events: its receipt reads
`Failed (<reason>); nothing happened except the fee of <fee> USD.` A
rejected transaction has no effects at all.
