# Agent identities from the command line

This guide registers an agent, launches subagents with their own
identities, and stops and resumes the tree. It assumes a devnet is running
(`sion devnet`) and uses its deterministic wallets `alice` and `carol`.
For background, see [Agent identities and subagents](../concepts/agent-identities.md).

## 1. Write a card

```bash
sion agent card-template "research orchestrator" \
  --description "plans literature reviews and pays for tools" \
  --url "https://agents.example.org/a2a" > orchestrator.json
```

Edit the file as you like. The ledger stores only a commitment to its exact
bytes, so publish the file unchanged wherever your agent is reached.

## 2. Register the agent

```bash
sion keys new orchestrator
sion faucet orchestrator 5
sion agent register alice orchestrator --card orchestrator.json \
  --payees carol --max-payment 25 --max-window 500 --funding 1000
```

The command prints the agent's id (also its `did:sion:` identifier) and
its allotment. `alice` is the principal and the backer: she answers for the
agent and everything it launches.

## 3. Launch subagents

```bash
sion agent spawn orchestrator <agent id> <allotment> \
  --count 3 --budget 40 --max-payment 10 --prefix researcher
```

Each subagent gets a new wallet with a random seed (`researcher-1`,
`researcher-2`, `researcher-3` under `$SION_HOME/wallets`), its own
identity, caveats at least as strict as the parent's, and 40 USD carved
from the orchestrator's allotment. The command prints each subagent's id
and allotment. Add `--may-spawn` to let them launch subagents of their own.
Subagents pay their own transaction fees, so fund their wallets
(`sion faucet researcher-1 1`).

## 4. Pay as a subagent

```bash
sion charter pay researcher-1 <researcher id> <researcher allotment> carol 8 \
  --purpose "a tool call"
```

A payment above the subagent's limit, or to a payee not on its list, is
refused by the ledger.

## 5. Keyless subagents

A subagent does not need keys of its own. Launch keyless ones and the
orchestrator's wallet signs for all of them:

```bash
sion agent spawn orchestrator <agent id> <allotment> --keyless \
  --count 3 --budget 40 --max-payment 10 --prefix worker --gas 0.01
```

Each keyless subagent's account is derived from its id
(`H("sion/agent/keyless", id)`), which no key controls; the ledger accepts
its transactions only as members of a batch signed by its nearest keyed
ancestor, here the orchestrator. Nothing secret is written: each name gets
a `.keyless` file under `$SION_HOME/wallets` that records the subagent and
the wallet that signs for it:

```text
$ cat $SION_HOME/wallets/worker-1.keyless
{
  "charter": "81fc211e28c4d6df634ee034cfda5a2e130df8d0ca26a82fbe835731e4b75eb7",
  "signer": "orchestrator"
}
```

`--gas 0.01` sends each subagent 0.01 USD from the orchestrator's account
for its fees, in one transfer. Every command that takes a wallet takes a
keyless name, and signs with the orchestrator's keys:

```bash
sion charter pay worker-1 <worker id> <worker allotment> carol 8 \
  --purpose "a tool call"
```

To pay from many subagents at once, sign one batch for all of them:

```bash
sion agent pay-batch orchestrator carol 1 \
  --agents <worker-1 id>,<worker-2 id>,<worker-3 id> --purpose "tool calls"
```

It prints what the orchestrator signs, then each payment's outcome:

```text
orchestrator signs one batch of 3 transactions:
  As an agent under charter 81fc211e, pay 1 USD to carol for "tool calls". Fee 0.000567 USD; valid until Pulse 33.
  As an agent under charter 38731f06, pay 1 USD to carol for "tool calls". Fee 0.000567 USD; valid until Pulse 33.
  As an agent under charter 21e080bf, pay 1 USD to carol for "tool calls". Fee 0.000567 USD; valid until Pulse 33.
Pulse 16: Done: agent paid 1 USD to carol (receipt claim 52480131); fee 0.000567 USD.
Pulse 16: Done: agent paid 1 USD to carol (receipt claim 4a296f0c); fee 0.000567 USD.
Pulse 16: Done: agent paid 1 USD to carol (receipt claim 4de2fe2b); fee 0.000567 USD.
```

Each payment is a transaction of its own, with its own receipt and fee;
the three share one authorization. A member of a batch of three pays a fee
floor of 0.000567 USD on the devnet, against 0.0011 USD for a transaction
signed by itself (see [Batched authorization](../concepts/batched-authorization.md)).
`sion agent show` marks a keyless subagent with `"keyless": true` and names
the account that signs for it in `signed_for_by`.

## 6. Inspect

```bash
sion agent show <researcher id>
```

prints the agent's account, principal, backer, card, depth, status,
remaining budget and lineage up to the root.

## 7. Suspend, resume, revoke

```bash
sion agent suspend alice <agent id>              # the whole tree stops
sion agent resume alice <agent id>               # it runs again
sion agent revoke orchestrator <researcher id>   # for good
sion agent reclaim alice <researcher id>         # its unspent budget returns to alice
```

Any overseer may suspend: the backer, the principal, or the agent of any
charter above. Only the suspender or the backer may resume. An agent may
revoke itself. Only a revoked or expired agent can be reclaimed, by its
principal or its backer, and the funds go to the backer.
