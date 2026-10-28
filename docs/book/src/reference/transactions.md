# Transaction kinds

Every transaction has the same envelope and one of 30 kinds. This page
lists the kinds, what each does, its coordination class, the sentence
wallets show before signing it, and what sends it today. The rules each
kind follows are in §5 of the
[specification](../specification.md).

## The envelope

```rust
pub struct Transaction {
    pub sender: AccountId,                  // the account that signs
    pub genesis_policy: Option<AuthPolicy>, // required on the sender's first transaction
    pub inputs: Vec<ObjectRef>,             // owned inputs at exact versions; inputs[0] pays the fee
    pub fee: Amount,                        // in USD
    pub kind: TxKind,                       // what it does (below)
    pub expires_at: Pulse,                  // last Pulse in which it may execute
    pub nonce: u64,                         // makes otherwise identical transactions distinct
}

pub struct SignedTransaction {
    pub tx: Transaction,
    pub auth: Authorization, // signatures satisfying the sender's policy, for this chain
}
```

- The **digest** is the hash of the Borsh encoding of `Transaction`; it is
  what the sender signs, under a signing context bound to the chain's
  configuration.
- A transaction names **1 to 256 distinct inputs**. The first is a USD coin
  that pays the fee (the faucet's transaction uses its account object
  instead).
- The **minimum fee** is `base_fee + fee_per_kib × ⌈size / 1024⌉`, where
  `size` is the encoded body without signatures (0.001 and 0.0001 USD on
  the devnet).
- The **coordination class** is derived from the kind, never declared.
  Kinds that touch a contention domain are *contended*; `IssueClaim`,
  `RegisterAtom` and `PostBond` are *knowledge*; `AgentPay`, `Delegate`,
  `SpendAllotment` and `ReturnAllotment` are *bounded*; everything else is
  *owned-linear*. See [Two-sorted state](../concepts/two-sorted-state.md).

The **sentence** column shows what `sion_cli::describe::sentence` renders,
with values in angle brackets. Every sentence ends with
`. Fee <fee> USD; valid until Pulse <expires_at>.` For example:

```text
dana signs: Pay 12.5 USD to alice. Fee 0.0012 USD; valid until Pulse 22.
```

Kinds that no CLI command, HTTP route or MCP tool builds yet ("Rust API
only" below) can be built and signed with the Rust API
(`sion_exec::TxKind`) and submitted with
[`POST /v1/transactions`](http-api.md#post-v1transactions).

## Money and accounts

| Kind | What it does | Class | Sentence | Sent by |
|---|---|---|---|---|
| `Transfer { asset, outputs }` | Pays coins of one asset to one or more accounts; change returns to the sender | owned-linear | `Pay <amount> <asset> to <account>[, …]` | `sion send`, `sion x402 fetch`, MCP `sion_pay` |
| `Faucet { grants }` | Devnet only: mints grants from the faucet account; pays no fee; grants are capped | owned-linear | `Mint <amount> <asset> for <account> (devnet faucet)` | The node, for `POST /v1/faucet` and `sion faucet` |
| `RotatePolicy { policy, viewing_key }` | Replaces the account's authorization policy (and optionally its ML-KEM viewing key); the identifier stays | owned-linear | `Replace this account's keys[ and viewing key] (the account keeps its identifier)` | Rust API only |

## Knowledge and assurance

| Kind | What it does | Class | Sentence | Sent by |
|---|---|---|---|---|
| `IssueClaim { claim }` | Submits a claim; it joins the knowledge log in the deferred phase, or is rejected there | knowledge | `Publish the claim "<subject>" (<schema>), valid for <n> Pulses, resting on <a> assumptions and <c> other claims` | `sion claim issue`, MCP `sion_issue_claim` |
| `RegisterAtom { class, label }` | Registers an unbonded assumption `registered:<sender>:<label>` | knowledge | `Register the assumption "<label>" (<class>)` | Rust API only |
| `PostBond { class, label, amount }` | Posts a bond behind the sender's assumption `bond:<sender>:<label>`, registering it if needed | knowledge | `Post a <amount> USD bond behind your <class> assumption "<label>"` | `sion bond` |
| `Accept { claim, value_at_risk, policy, until, nonce }` | Relies on a claim: applies the decision rule under the policy and reserves κ·V of its backing until a Pulse | contended (assurance) | `Rely on claim <id> for up to <V> USD until Pulse <until>, reserving its backing` | `sion accept` |
| `Release { reservation }` | Releases a reservation early | contended (assurance) | `Release reservation <id>` | Rust API only |
| `Challenge { claim, target, evidence, deposit }` | Challenges one bonded atom of a claim with a deposit; needs an adjudicator | contended (assurance) | `Challenge assumption <atom> of claim <id>, depositing <deposit> USD` | `sion challenge` |
| `Rule { challenge, upheld }` | The adjudicator's bond owner rules; upheld slashes, falsifies, compensates | contended (assurance) | `As adjudicator, uphold\|reject challenge <id>` | `sion rule` |
| `WithdrawBond { atom }` | Withdraws the unlocked part of a bond | contended (assurance) | `Withdraw the free part of the bond behind assumption <atom>` | Rust API only |

## Agent Charters and Escrow Lanes

| Kind | What it does | Class | Sentence | Sent by |
|---|---|---|---|---|
| `CreateCharter { agent, caveats, funding, agent_bond }` | Gives an agent a charter with caveats, an allotment funded by the principal, and optionally a bond behind its receipts | owned-linear | `Give <agent> an Agent Charter funded with <funding> USD[ and a <bond> USD bond behind its receipts]: <caveats>` | `sion charter create` |
| `Delegate { charter, sub_agent, caveats, amount }` | The agent gives a sub-agent strictly narrower caveats and part of its allotment | bounded | `Delegate <amount> USD from charter <id> to <sub-agent>: <caveats>` | `sion demo agents` |
| `AgentPay { charter, payee, amount, purpose, evidence }` | The agent pays from its allotment, within every caveat of its charter chain | bounded | `As an agent under charter <id>, pay <amount> USD to <payee> for "<purpose>"[, citing claim <id>]` | `sion charter pay`, `sion x402 fetch --charter`, MCP `sion_agent_pay` |
| `RevokeCharter { charter }` | The principal revokes a charter and everything delegated from it | contended (charter) | `Revoke charter <id> and every charter delegated from it` | `sion charter revoke` |
| `ReclaimCharter { charter }` | Returns what is left in a dead charter's allotments to its root principal | contended (charter) | `Return what is left in dead charter <id>'s allotments to its backer` | Rust API only |
| `CreateBounded { label, asset, total, allotments }` | Locks a shared total and splits it into single-writer allotments | owned-linear | `Lock <total> <asset> as the shared total "<label>", split into <n> single-writer allotments` | Rust API only |
| `SpendAllotment { to, amount }` | The allotment's writer spends from it | bounded | `Spend <amount> USD from your allotment, to <account>` | Rust API only |
| `ReturnAllotment` | The writer returns the rest of its allotment to the total | bounded | `Return the rest of your allotment to its total` | Rust API only |
| `Rebalance { bounded, holder, amount }` | The manager issues a new allotment from the unallocated amount | contended (bounded total) | `Allot <amount> USD of shared total <id> to <account>` | Rust API only |

## Markets

| Kind | What it does | Class | Sentence | Sent by |
|---|---|---|---|---|
| `CreateMarket { label, good, numeraire, batch_length, clear_deadline, committee, threshold }` | Opens a batch market; with a committee, orders may be sealed | owned-linear | `Open the market "<label>" for <good> priced in <numeraire>, clearing every <n> Pulses[ with sealed orders]` | `sion market create` |
| `SubmitOrder { market, submission }` | Enters a plain or sealed order; the escrow is drawn from the sender's coins | owned-linear | `Bid for <q> units at at most <p> <numeraire> each in market <id>` (or `Offer … at at least …`), or `Submit a sealed buy\|sell order to market <id>, escrowing <escrow>` | `sion market order` |
| `RevealBatch { market, batch, shares }` | Reveals sealed orders of a sealed batch with committee key shares | contended (market) | `Reveal <n> sealed orders of batch <b> in market <id>` | Rust API only |
| `ClearBatch { market, batch, result }` | A solver settles a sealed batch with a clearing and its optimality certificate | contended (market) | `Settle batch <b> of market <id> with a certified-optimal clearing` | `sion market clear` |

## Validators

| Kind | What it does | Class | Sentence | Sent by |
|---|---|---|---|---|
| `RegisterValidator { lattice, hash, beacon_tip, stake, possession }` | Stakes for a validator, with a proof that it holds its keys; it joins at the next epoch boundary | contended (validators) | `Register a validator staking <stake> USD; it joins the active set at the next epoch boundary` | `sion demo staking` |
| `Unstake { validator }` | The operator takes its validator out at the next epoch boundary | contended (validators) | `Take validator <id> out of the active set at the next epoch boundary` | Rust API only |
| `WithdrawStake { validator }` | The operator withdraws stake and unclaimed rewards after unbonding | contended (validators) | `Withdraw validator <id>'s stake and unclaimed rewards after unbonding` | Rust API only |
| `ClaimRewards { validator }` | The operator collects its validator's rewards | contended (validators) | `Collect validator <id>'s rewards` | `sion demo staking` |
| `ReportEquivocation { evidence }` | Reports two conflicting signed headers or approvals; the offender's stake is slashed, with a share to the sender | contended (validators) | `Report that a validator signed two conflicting consensus headers\|lane approvals; its stake is slashed and you receive a share` | `sion demo staking` |

See [Validators, staking and epochs](../concepts/committees.md).

## Payment streams

| Kind | What it does | Class | Sentence | Sent by |
|---|---|---|---|---|
| `OpenStream { payee, asset, unit, length, anchor, deposit, until, checkpoints }` | Escrows `deposit` for `payee`, paid off the ledger `unit` per chain element; optionally commits to a checkpoint every `spacing` steps | owned-linear | `Open a payment stream to <payee>: escrow <deposit>, paid off the ledger at <unit> a step for up to <length> steps, settled by <payee> until Pulse <until>[, with a checkpoint every <n> steps]` | `sion stream open` |
| `SettleStream { stream, position, value, jump }` | The payee collects the steps up to `position`: by walking from the head (at most 65,536 steps), or by a `jump` to a checkpoint; pays a fee per hash checked | contended (stream) | `Settle stream <id> up to step <p>[, proven by checkpoint <j>]` | `sion stream settle`, an x402 server's stream desk |
| `TopUpStream { stream, deposit, until }` | The payer adds escrow and may extend the last Pulse | contended (stream) | `Add <deposit> USD to stream <id> and keep it open until Pulse <until>` | `sion stream top-up` |
| `CloseStream { stream }` | Closes the stream and returns the escrow to the payer: by the payee at any time, by the payer after the last Pulse | contended (stream) | `Close stream <id> and return what is left in escrow to its payer` | `sion stream close` |

See [Payment streams](../concepts/payment-streams.md).

## Outcomes

Each transaction ends as **Success**, **Failed** or **Rejected**
([Pulses](../concepts/pulses.md#three-outcomes)). The CLI prints the
outcome as a sentence:

| Outcome | Sentence |
|---|---|
| Success | `Done: <event>; <event>; fee <fee> USD.` (or `Done: accepted; …` with no events) |
| Failed | `Failed (<reason>); nothing happened except the fee of <fee> USD.` |
| Rejected | `Rejected (<reason>); nothing changed and no fee was charged.` |

The events and their sentences are listed in [Events](events.md); the
reasons in [Error messages](errors.md).
