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
    pub auth: Authorization,          // signatures satisfying the authorizing policy, for this chain
    pub batch: Option<BatchMember>,   // for a batch member: its batch and its place in it
}

pub struct BatchMember {
    pub batch: TxBatch,               // what the one authorization signs
    pub index: u64,                   // the member's position
    pub path: Vec<Hash32>,            // its inclusion path to the batch's root
    pub charter: Option<ObjectId>,    // for a keyless sender, its charter
}

pub struct TxBatch {
    pub signer: AccountId,            // the account whose policy the authorization satisfies
    pub size: u64,                    // 1 to 256 members
    pub root: Hash32,                 // RFC 9162 Merkle root over the members' digests
}
```

- The **digest** is the hash of the Borsh encoding of `Transaction`. A
  transaction on its own signs its digest, under a signing context bound
  to the chain's configuration. A **batch member** is signed for once with
  the rest of its batch: the authorization signs the batch's message,
  under a separate batch context, and the member carries its path to the
  batch's root. See [Batched authorization](../concepts/batched-authorization.md).
- The **authorizing policy** is the sender's own, or, for a
  [keyless subagent](../concepts/agent-identities.md#keyless-subagents),
  the policy of its nearest keyed ancestor, which must be the batch's
  signer.
- A transaction names **1 to 256 distinct inputs**. The first is a USD coin
  that pays the fee (the faucet's transaction uses its account object
  instead).
- The **minimum fee** is `base_fee + fee_per_kib × ⌈size / 1024⌉`, where
  `size` is the encoded body without signatures (0.001 and 0.0001 USD on
  the devnet). A batch member pays a share of the signature check instead
  of the whole of it: 800‰ of `base_fee` is split between the batch's
  members, rounded up. On the devnet a small transfer's floor is
  0.0011 USD alone and 0.000304 USD as a member of a batch of 256.
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
| `RevokeCharter { charter }` | The agent itself or an overseer (backer, principal, an ancestor's agent) revokes a charter and everything below it | contended (charter) | `Revoke charter <id> and every charter delegated from it` | `sion charter revoke`, `sion agent revoke` |
| `ReclaimCharter { charter }` | Returns what is left in a dead charter's allotments to its root principal | contended (charter) | `Return what is left in dead charter <id>'s allotments to its backer` | `sion agent reclaim` |
| `CreateBounded { label, asset, total, allotments }` | Locks a shared total and splits it into single-writer allotments | owned-linear | `Lock <total> <asset> as the shared total "<label>", split into <n> single-writer allotments` | Rust API only |
| `SpendAllotment { to, amount }` | The allotment's writer spends from it | bounded | `Spend <amount> USD from your allotment, to <account>` | Rust API only |
| `ReturnAllotment` | The writer returns the rest of its allotment to the total | bounded | `Return the rest of your allotment to its total` | Rust API only |
| `Rebalance { bounded, holder, amount }` | The manager issues a new allotment from the unallocated amount | contended (bounded total) | `Allot <amount> USD of shared total <id> to <account>` | Rust API only |

## Agent identities

| Kind | What it does | Class | Sentence | Sent by |
|---|---|---|---|---|
| `RegisterAgent { agent, card, caveats, funding, agent_bond }` | Registers an agent identity: a charter with a card commitment, at depth 0 | owned-linear | `Register <agent> as an agent[ with card <hash>], funded with <funding> USD[ and a <bond> USD bond behind its receipts]: <caveats>` | `sion agent register` |
| `SpawnAgents { charter, children }` | The agent launches up to 256 subagents, each with its own account, card, attenuated caveats, carved window limit and budget | bounded | `As agent <id>, launch <n> subagents with their own identities, <total> USD in budgets in all` | `sion agent spawn` |
| `SetAgentCard { charter, card }` | The agent replaces its card commitment | contended (charter) | `Set agent <id>'s card to <hash>` | `sion agent card` |
| `SuspendAgent { charter }` | An overseer suspends a charter and everything below it | contended (charter) | `Suspend agent <id> and every agent below it` | `sion agent suspend` |
| `ResumeAgent { charter }` | The suspender or the backer lifts a suspension | contended (charter) | `Lift the suspension of agent <id>` | `sion agent resume` |

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
| `ReportEquivocation { evidence }` | Reports two conflicting signed headers or approvals; the offender's stake is slashed, and a tenth of its pool, with a share of the offender's own stake (never of its pool) to the sender | contended (validators) | `Report that a validator signed two conflicting consensus headers\|lane approvals; its stake is slashed and you receive a share` | `sion demo staking` |

See [Validators, staking and epochs](../concepts/committees.md).

## Pooled stake

| Kind | What it does | Class | Sentence | Sent by |
|---|---|---|---|---|
| `DelegateStake { validator, amount }` | Adds stake to a joining or active validator's pool without handing it over: the sender keeps a delegation holding pool shares, which earns and counts towards the validator's weight from the epoch's end; refused above the weight cap | contended (validators) | `Delegate <amount> USD to validator <id>'s pool; you keep the delegation, which earns from the epoch's end` | `sion stake delegate`, MCP `sion_stake_delegate` |
| `Undelegate` | Stops the delegation at `inputs[1]` earning, pays its rewards and starts its unbonding | contended (validators) | `Stop a delegation earning, collect its rewards and start unbonding` | `sion stake undelegate`, MCP `sion_stake_undelegate` |
| `WithdrawDelegation` | Returns an unbonded delegation's stake (`inputs[1]`), slashes included | contended (validators) | `Withdraw an unbonded delegation's stake` | `sion stake withdraw`, MCP `sion_stake_withdraw` |
| `ClaimDelegatorRewards` | Pays the rewards credited to the delegation at `inputs[1]` | contended (validators) | `Collect a delegation's rewards` | `sion stake claim`, MCP `sion_stake_claim` |
| `SetCommission { validator, permille }` | The operator sets its pool's commission (at most 250 ‰; once anyone has delegated, a raise of at most 20 ‰ at a time), from the epoch's end | contended (validators) | `Set validator <id>'s commission to <p>% from the epoch's end` | `sion stake commission` |

See [Pooled stake](../concepts/validators/pooled-stake.md).

## Payment streams

| Kind | What it does | Class | Sentence | Sent by |
|---|---|---|---|---|
| `OpenStream { payee, asset, unit, length, anchor, deposit, until, checkpoints }` | Escrows `deposit` for `payee`, paid off the ledger `unit` per chain element; optionally commits to a checkpoint every `spacing` steps | owned-linear | `Open a payment stream to <payee>: escrow <deposit>, paid off the ledger at <unit> a step for up to <length> steps, settled by <payee> until Pulse <until>[, with a checkpoint every <n> steps]` | `sion stream open` |
| `SettleStream { stream, position, value, jump }` | The payee collects the steps up to `position`: by walking from the head (at most 65,536 steps), or by a `jump` to a checkpoint; pays a fee per hash checked | contended (stream) | `Settle stream <id> up to step <p>[, proven by checkpoint <j>]` | `sion stream settle`, an x402 server's stream desk |
| `TopUpStream { stream, deposit, until }` | The payer adds escrow and may extend the last Pulse | contended (stream) | `Add <deposit> USD to stream <id> and keep it open until Pulse <until>` | `sion stream top-up` |
| `CloseStream { stream }` | Closes the stream and returns the escrow to the payer: by the payee at any time, by the payer after the last Pulse | contended (stream) | `Close stream <id> and return what is left in escrow to its payer` | `sion stream close` |

See [Payment streams](../concepts/payment-streams.md).

## Memory logs

| Kind | What it does | Class | Sentence | Sent by |
|---|---|---|---|---|
| `OpenMemory { agent, policy }` | Opens a memory log owned by the sender, optionally for an agent identity whose agent is the sender | owned-linear | `Open a memory log that only the sender can append to` or `Open a memory log for agent <id>, which only it can append to` | `sion memory open`, MCP `sion_memory_open` |
| `CommitMemory { size, root, index, manifest, proof, seal }` | Moves the log at `inputs[1]` to a new head; the ledger checks an RFC 9162 consistency proof from the current head, so the log only grows | owned-linear | `Commit memory log <id> at <n> atoms, proving it only grew[, and seal it]` | `sion memory commit`, MCP `sion_memory_commit` |

See [Memory for agents and robots](../concepts/memory.md).

## Task Charters

| Kind | What it does | Class | Sentence | Sent by |
|---|---|---|---|---|
| `PostTask { spec, acceptance, worker, amount, deadline, charter }` | Escrows `amount` for a task whose acceptance rule is fixed now; with `charter`, from that agent's allotment (`inputs[1]`) under its caveats | owned-linear, or bounded with `charter` | `Escrow <amount> USD for a task for <worker>\|open to anyone, paid … for a result resting on <n> required claims and accepted under your policy, settled by Pulse <deadline>[, from agent <id>'s budget]` | `sion task post` |
| `SettleTask { task, result, attestations }` | Pays the worker once a result claim with exactly the required formula passes the decision rule; reserves κ times the amount of its backing for the payer | contended (assurance) | `Settle task <id> with your result[ and <n> attestations]` | `sion task settle` |
| `CancelTask { task }` | Refunds a task: released by its worker, or withdrawn by its poster when open to all or expired | contended (task) | `Cancel task <id> and refund its escrow` | `sion task cancel` |

Results and attestations are claims (`IssueClaim`, schemas `task.result/v1`
and `task.attestation/v1`), issued with `sion task submit` and
`sion task attest`. See [Task Charters](../concepts/task-charters.md).

## Storage deals

| Kind | What it does | Class | Sentence | Sent by |
|---|---|---|---|---|
| `PostDeal { root, size, challenges, price, rounds, round_length, collateral, provider, accept_by, locator }` | Escrows `price × rounds` for keeping `size` leaves whose Merkle root is `root`, for `provider` or, without one, for anyone (then with collateral); `round_length` is a multiple of 8, from 8 to 100,000 | owned-linear | `Escrow <price × rounds> USD to have data <root> (<n> leaves) kept by <provider>\|any provider for <r> rounds of <l> Pulses: <price> USD per round proven by <c> random leaves, with <collateral> USD of collateral at stake` | `sion storage post`, MCP `sion_storage_post` |
| `AcceptDeal { deal }` | The provider posts the collateral and gets a custody record it owns; rounds start on the next multiple of 8 Pulses | contended (deal) | `Accept storage deal <id>, posting its collateral; prove each round or lose it` | `sion storage accept`, `sion storage serve` |
| `ProveStorage { proofs }` | Proves the round under way for the custody records at `inputs[1..]`, one proof each: the leaves the beacon of the round's first Pulse challenges, with their Merkle paths (at most 64 leaves in all) | owned-linear | `Prove custody of <n> storage deal(s) for the round under way with <k> challenged leaves` | `sion storage prove`, `sion storage serve` |
| `SettleDeal { deal }` | The provider is paid for the rounds it proved since the last payment | contended (deal) | `Collect for the proven rounds of storage deal <id>` | `sion storage settle`, `sion storage serve` |
| `CloseDeal { deal }` | Refunds an open deal (client); ends an accepted one at its end (client or provider) or once abandoned for three rounds (client): pays what is due, refunds unproven rounds, returns the collateral less `⌊bond × missed / rounds⌋`, or forfeits it if abandoned | contended (deal) | `Close storage deal <id>: pay what is due, refund the escrow of unproven rounds, settle the collateral` | `sion storage close`, MCP `sion_storage_close` |

See [Storage deals](../concepts/storage.md).

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
