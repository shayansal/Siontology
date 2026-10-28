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

## 5. Inspect

```bash
sion agent show <researcher id>
```

prints the agent's account, principal, backer, card, depth, status,
remaining budget and lineage up to the root.

## 6. Suspend, resume, revoke

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
