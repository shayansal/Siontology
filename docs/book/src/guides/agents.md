# Agent identities from the command line

This guide registers an agent, launches subagents with their own
identities, limits how fast the tree spends, narrows it, commits to a
hundred thousand subagents in one transaction, stops and resumes the
tree, and checks an agent's standing as a light client. It assumes a
devnet is running (`sion devnet`) and uses its deterministic wallets
`alice` and `carol`. For background, see [Agent
identities and subagents](../concepts/agent-identities.md).

Every output below is from one run on a local devnet, in order, except
section 10, which is from a later run, on specification v1.15.

## 1. Write a card

```text
$ sion agent card-template "research orchestrator" --description "plans literature reviews and pays for tools" --url "https://agents.example.org/a2a" > orchestrator.json
$ cat orchestrator.json
{
  "capabilities": {
    "pushNotifications": false,
    "streaming": false
  },
  "defaultInputModes": [
    "text/plain"
  ],
  "defaultOutputModes": [
    "text/plain"
  ],
  "description": "plans literature reviews and pays for tools",
  "name": "research orchestrator",
  "provider": {
    "organization": "",
    "url": ""
  },
  "sion": {
    "account": "<the account that signs for the agent>",
    "agent": "did:sion:<chain identity>:<agent id, filled in after registration>",
    "parent": null,
    "principal": "<the account that answers for it>"
  },
  "skills": [],
  "url": "https://agents.example.org/a2a",
  "version": "1.0.0"
}
```

Edit the file as you like. The ledger stores only a commitment to its exact
bytes, so publish the file unchanged wherever your agent is reached.

## 2. Register the agent

```text
$ sion keys new orchestrator
created orchestrator: account 6193f13a6a7737d74427d41da00fcfed4551eedab7ce5fb3be772f5bd61a5b4e
seed stored in $SION_HOME/wallets/orchestrator.seed
$ sion faucet orchestrator 5
funded orchestrator with 5
$ sion agent register alice orchestrator --card orchestrator.json --payees carol --max-payment 25 --burst 500 --rate 0.5 --funding 1000
alice signs: Register orchestrator as an agent with card 13baff0d, funded with 1,000 USD: only to carol, at most 25 USD per payment, 500 USD at once, refilled by 0.5 USD a Pulse, until Pulse 100033, a receipt claim for every payment, may delegate narrower limits. Fee 0.0011 USD; valid until Pulse 53.
Pulse 36: Done: agent 0df519ab registered, signing as orchestrator; fee 0.0011 USD.
agent 0df519abdf18a35258c4047d5a6cbc36f60edf0b137b8c96236e60dbe0ac2b98
allotment d64089c3f8c1dc547c19b7669afbf9eb9f3bed3b9cf88edfbce2c39fe1150663
did did:sion:94425d7211b21492c4a9147ba3b67dc08bf05d315c8f89dc4c2a299d5d596e5a:0df519abdf18a35258c4047d5a6cbc36f60edf0b137b8c96236e60dbe0ac2b98
```

The command prints the agent's id (also its `did:sion:` identifier) and
its allotment. `alice` is the principal and the backer: she answers for the
agent and everything it launches.

The agent's limits are its caveats: it may pay only `carol`, at most 25 USD
a payment, from a budget of 1,000 USD. How fast it may spend is a **token
bucket**: the bucket holds at most the **burst**, 500 USD, and starts full;
it refills by the **rate**, 0.5 USD, every Pulse, never past the burst.
Every payment takes from the bucket as well as from the budget, and a
payment the bucket does not cover is refused.

## 3. Launch subagents

```text
$ sion agent spawn orchestrator 0df519ab…2b98 d64089c3…0663 --count 3 --budget 40 --max-payment 10 --burst 20 --prefix researcher
orchestrator signs: As agent 0df519ab, launch 3 subagents with their own identities, 120 USD in budgets and 60 USD at once, refilled by 0.06 USD a Pulse in all. Fee 0.0013 USD; valid until Pulse 56.
Pulse 39: Done: subagent c6c9fa40 launched by 0df519ab, signing as researcher-1; subagent 548eae09 launched by 0df519ab, signing as researcher-2; subagent d45b06d9 launched by 0df519ab, signing as researcher-3; fee 0.0013 USD.
researcher-1: agent c6c9fa40c6b708d3e2ee3037c44b32551d0ae70ddd5c4ae28432883e4fd20b4a allotment 976fb51d7db0ff339c0ce47fe92706a0d3b4259264a1fc534a03baad1294f23d
researcher-2: agent 548eae09f300c66beefd8bacff46b25cba36c46bb5566f8f1e283ad43eda57ce allotment db64fa6ffac10c268ef09686a1f6574f20920ed0831af82dd61881d0841746e7
researcher-3: agent d45b06d90a21efe876e3c423a252c4c2f6fffaef31ea4ad096f036fb2bc93673 allotment f5f91883bebf80b85ca7eb44dfa67a8b4d3179d8cdddfe642071873e8dca5a1c
$ sion faucet researcher-1 1
funded researcher-1 with 1
$ sion faucet researcher-2 1
funded researcher-2 with 1
```

Each subagent gets a new wallet with a random seed (`researcher-1`,
`researcher-2`, `researcher-3` under `$SION_HOME/wallets`), its own
identity, caveats at least as strict as the parent's, and 40 USD carved
from the orchestrator's allotment. Add `--may-spawn` to let them launch
subagents of their own. Subagents pay their own transaction fees, so fund
their wallets (`sion faucet researcher-1 1`).

Each also gets a bucket carved out of the orchestrator's. By default its
burst is its budget, or an equal share of what the parent's bucket holds if
that is less, and its rate is the share of the parent's rate that its
budget is of the parent's remaining budget: here 40/1,000 of 0.5 USD, 0.02
USD a Pulse. `--burst` and `--rate` set them. The parent keeps only what it
did not hand down, tokens included, so the whole tree never pays faster
than the orchestrator's bucket allows.

A subagent's id is its **lineage id**: a hash of its parent's id and an
index that only the parent writes, a counter in its allotment. Anyone who
holds an agent's lineage (its tree's root and the path of indices down to
it) computes the ids of all its ancestors without reading them.

## 4. Pay as a subagent

```text
$ sion charter pay researcher-1 c6c9fa40…0b4a 976fb51d…f23d carol 8 --purpose 'a tool call'
researcher-1 signs: As an agent under charter c6c9fa40, pay 8 USD to carol for "a tool call". Fee 0.0012 USD; valid until Pulse 64.
Pulse 47: Done: agent paid 8 USD to carol (receipt claim a98a73cf); fee 0.0012 USD.
  digest dcbc22c7be37514d2237b0b11117fe64228283f1d0980676a8bd33a9dae11b9a
$ sion charter pay researcher-1 c6c9fa40…0b4a 976fb51d…f23d carol 12 --purpose 'a bigger tool call'
researcher-1 signs: As an agent under charter c6c9fa40, pay 12 USD to carol for "a bigger tool call". Fee 0.0011 USD; valid until Pulse 67.
Pulse 49: Failed (charter caveat violated: payment 12 above per-payment limit 10); nothing happened except the fee of 0.0011 USD.
  digest 041455b2cb13008c5c5c706937e8727e6d050c58ea0dd2a4ca85a6fe7b60219a
```

A payment above the subagent's limit, or to a payee not on its list, is
refused by the ledger, and so is one its bucket does not cover.
researcher-1 may spend at most 20 USD at once, and its bucket refills by
0.02 USD a Pulse: after paying 8 USD, it can pay 12.06 USD in the next
Pulse. A first 10 USD payment goes through; a second, three Pulses later,
finds 2.16 USD in the bucket:

```text
$ sion agent show c6c9fa40…0b4a
{
  …
  "agent": "c6c9fa40c6b708d3e2ee3037c44b32551d0ae70ddd5c4ae28432883e4fd20b4a",
  "budget_remaining_usd": "32",
  "burst_usd": "20",
  "can_spend_next_pulse_usd": "12.06",
  "depth": 1,
  "kept_after_children": {
    "burst_usd": "20",
    "refill_usd_per_pulse": "0.02"
  },
  "lineage_path": [
    0
  ],
  "lineage_root": "0df519abdf18a35258c4047d5a6cbc36f60edf0b137b8c96236e60dbe0ac2b98",
  "max_per_payment_usd": "10",
  "refill_usd_per_pulse": "0.02",
  "status": "active"
}
$ sion charter pay researcher-1 c6c9fa40…0b4a 976fb51d…f23d carol 10 --purpose 'a batch of lookups'
researcher-1 signs: As an agent under charter c6c9fa40, pay 10 USD to carol for "a batch of lookups". Fee 0.0011 USD; valid until Pulse 70.
Pulse 52: Done: agent paid 10 USD to carol (receipt claim 44e15262); fee 0.0011 USD.
  digest 1c608f7b011399e99dd02efdb610fef9125161182c13e46b67269f26f47d55e1
$ sion charter pay researcher-1 c6c9fa40…0b4a 976fb51d…f23d carol 10 --purpose 'another batch'
researcher-1 signs: As an agent under charter c6c9fa40, pay 10 USD to carol for "another batch". Fee 0.0011 USD; valid until Pulse 72.
Pulse 55: Failed (charter caveat violated: the bucket holds 2.16, below the payment 10); nothing happened except the fee of 0.0011 USD.
  digest 85e5d444decc8e309497e561f03f90b5b753e936791717d9f0436c109dff6083
```

At its rate, the bucket holds 10 USD again about 400 Pulses later.

## 5. Keyless subagents

A subagent does not need keys of its own. Launch keyless ones and the
orchestrator's wallet signs for all of them:

```text
$ sion agent spawn orchestrator 0df519ab…2b98 d64089c3…0663 --keyless --count 3 --budget 40 --max-payment 10 --prefix worker --gas 0.01
orchestrator signs: As agent 0df519ab, launch 3 subagents with their own identities, 120 USD in budgets and 120 USD at once, refilled by 0.06 USD a Pulse in all. Fee 0.0011 USD; valid until Pulse 75.
Pulse 58: Done: subagent 3b1a55d0 launched by 0df519ab, keyless: its nearest keyed ancestor signs for it; subagent 931f6c9f launched by 0df519ab, keyless: its nearest keyed ancestor signs for it; subagent 2bcbbc42 launched by 0df519ab, keyless: its nearest keyed ancestor signs for it; fee 0.0011 USD.
worker-1: agent 3b1a55d031c6d1c49cb65d0351fce152568e947a6b2244b11dd5abc63260d6a4 account 3243d25b1e721efc26d77167867e5b4911ebf3f8323216583d704c104f6e081a allotment 37d22cc2e04b393246da5d30424792181c4bd2e23fd640a040a6452f74f5929e (keyless; orchestrator signs for it)
worker-2: agent 931f6c9f85bf83dc2e6bb4953b557f44ec9944380f854fe97e66c0f38948c802 account 09574967fa78aba491bc9e9a4206554a6f7d42c60d7a2495ef3529ac568c2ce6 allotment cad6415470d8bf85488f680c56d2a2fa15a4efc613a44ce44b70567eb7066d9d (keyless; orchestrator signs for it)
worker-3: agent 2bcbbc4298a2640c086b72e7b888cc53f5b6cc91463e22f52b972b662f5e5906 account b54c44a831dbe9ce6b0ccc94af23b0868e1402175d287f863cb88bbf00366d41 allotment a0da37ae85fb879f6ba0c2eb5c10566cdaff7bde356cd535886c3a453f58dd4e (keyless; orchestrator signs for it)
orchestrator signs: Pay 0.01 USD to account 3243d25b, 0.01 USD to account 09574967 and 0.01 USD to account b54c44a8. Fee 0.0011 USD; valid until Pulse 78.
Pulse 60: Done: paid 0.01 USD to account 3243d25b, 0.01 USD to account 09574967 and 0.01 USD to account b54c44a8; fee 0.0011 USD.
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
  "charter": "3b1a55d031c6d1c49cb65d0351fce152568e947a6b2244b11dd5abc63260d6a4",
  "signer": "orchestrator"
}
```

`--gas 0.01` sends each subagent 0.01 USD from the orchestrator's account
for its fees, in one transfer. Every command that takes a wallet takes a
keyless name, and signs with the orchestrator's keys:

```text
$ sion charter pay worker-1 3b1a55d0…d6a4 37d22cc2…929e carol 8 --purpose 'a tool call'
orchestrator signs, as a batch of one: As an agent under charter 3b1a55d0, pay 8 USD to carol for "a tool call". Fee 0.0011 USD; valid until Pulse 80.
Pulse 63: Done: agent paid 8 USD to carol (receipt claim f843beff); fee 0.0011 USD.
  digest b8782d90bce61eb30c6faa9695a682fc478051fd0f9cba3e59903b3f68ba30c5
```

To pay from many subagents at once, sign one batch for all of them:

```text
$ sion agent pay-batch orchestrator carol 1 --agents 3b1a55d0…d6a4,931f6c9f…c802,2bcbbc42…5906 --purpose 'tool calls'
orchestrator signs one batch of 3 transactions:
  As an agent under charter 3b1a55d0, pay 1 USD to carol for "tool calls". Fee 0.000567 USD; valid until Pulse 83.
  As an agent under charter 931f6c9f, pay 1 USD to carol for "tool calls". Fee 0.000567 USD; valid until Pulse 84.
  As an agent under charter 2bcbbc42, pay 1 USD to carol for "tool calls". Fee 0.000567 USD; valid until Pulse 84.
Pulse 66: Done: agent paid 1 USD to carol (receipt claim 7992a920); fee 0.000567 USD.
Pulse 66: Done: agent paid 1 USD to carol (receipt claim a92dc177); fee 0.000567 USD.
Pulse 66: Done: agent paid 1 USD to carol (receipt claim 843ab721); fee 0.000567 USD.
```

Each payment is a transaction of its own, with its own receipt and fee;
the three share one authorization. A member of a batch of three pays a fee
floor of 0.000567 USD on the devnet, against 0.0011 USD for a transaction
signed by itself (see [Batched authorization](../concepts/batched-authorization.md)).
`sion agent show` marks a keyless subagent with `"keyless": true` and names
the account that signs for it in `signed_for_by`.

## 6. Narrow a subtree

`sion agent narrow` adds limits to an agent and to everything below it,
with one transaction: the payees it may still pay (`--payees`), a smaller
largest payment (`--max-payment`), an earlier end (`--expires-in`), or no
more spawning (`--no-spawn`). What is not given stays as it is. The agent
itself or any overseer in good standing may narrow; only the backer may
widen a narrowing or lift it.

```text
$ sion agent narrow orchestrator 0df519ab…2b98 --max-payment 2 --no-spawn
orchestrator signs: Narrow agent 0df519ab and every agent below it: only to carol, at most 2 USD per payment, until Pulse 100033, no new subagents. Fee 0.0011 USD; valid until Pulse 86.
Pulse 69: Done: agent 0df519ab narrowed by orchestrator, with everything below it; fee 0.0011 USD.
$ sion charter pay researcher-2 548eae09…57ce db64fa6f…46e7 carol 3 --purpose 'above the narrowing'
researcher-2 signs: As an agent under charter 548eae09, pay 3 USD to carol for "above the narrowing". Fee 0.0012 USD; valid until Pulse 89.
Pulse 72: Failed (charter caveat violated: narrowed: payment 3 above 2); nothing happened except the fee of 0.0012 USD.
  digest f369e33062f67a8f9865b19daf254eaeec3184f9abb98a2b0cbe7ad5f2f91e01
$ sion charter pay researcher-2 548eae09…57ce db64fa6f…46e7 carol 2 --purpose 'within it'
researcher-2 signs: As an agent under charter 548eae09, pay 2 USD to carol for "within it". Fee 0.0011 USD; valid until Pulse 92.
Pulse 75: Done: agent paid 2 USD to carol (receipt claim f3228afe); fee 0.0011 USD.
  digest 8bd04db6513d713821887d8847f3bd760f5e5ec1039323443b56cd209f47efed
$ sion agent spawn orchestrator 0df519ab…2b98 d64089c3…0663 --count 1 --budget 1 --prefix intern
orchestrator signs: As agent 0df519ab, launch 1 subagent with its own identity, 1 USD in budgets and 1 USD at once, refilled by 0.0005 USD a Pulse in all. Fee 0.0011 USD; valid until Pulse 95.
Pulse 77: Failed (charter caveat violated: narrowed: delegation not allowed); nothing happened except the fee of 0.0011 USD.
$ sion agent narrow orchestrator 0df519ab…2b98 --max-payment 5
orchestrator signs: Narrow agent 0df519ab and every agent below it: only to carol, at most 5 USD per payment, until Pulse 100033, no new subagents. Fee 0.0011 USD; valid until Pulse 97.
Pulse 79: Failed (unauthorized: only the backer may widen or lift a narrowing); nothing happened except the fee of 0.0011 USD.
$ sion agent show 548eae09…57ce
{
  …
  "agent": "548eae09f300c66beefd8bacff46b25cba36c46bb5566f8f1e283ad43eda57ce",
  "lineage": [
    {
      "account": "6193f13a6a7737d74427d41da00fcfed4551eedab7ce5fb3be772f5bd61a5b4e",
      "agent": "0df519abdf18a35258c4047d5a6cbc36f60edf0b137b8c96236e60dbe0ac2b98",
      "narrowing": {
        "expires_at": 100033,
        "max_per_payment_usd": "2",
        "may_spawn": false,
        "payees": [
          "31338b143b630889b7c189bdb6aeef9683bba23d992bf8f66d6f22bc364f2353"
        ]
      },
      "revoked_at": null,
      "suspended_by": null
    }
  ],
  "narrowing": null,
  "status": "active, narrowed"
}
$ sion agent narrow alice 0df519ab…2b98 --lift
alice signs: Lift the narrowing of agent 0df519ab. Fee 0.0011 USD; valid until Pulse 99.
Pulse 82: Done: the narrowing of agent 0df519ab lifted by alice; fee 0.0011 USD.
```

A narrowing does not change any charter: it is written to the agent's
**overlay**, an object at a hash of its id that exists only once something
has happened to the agent. Every check of a payment reads the overlays of
the agent and its ancestors, at most nine, so one write narrows the whole
subtree.

## 7. A hundred thousand subagents in one transaction

`sion agent cohort` commits to up to 1,048,576 keyless subagents at once.
It builds a Merkle sum tree over the members, each a budget, a burst, a
rate and a card, and sends only its root and the sums of the members'
bursts and rates, which are carved out of the parent's bucket:

```text
$ sion agent cohort orchestrator 0df519ab…2b98 d64089c3…0663 --count 100000 --budget 0.05 --burst 0.001 --rate 0.000001 --full --prefix member
sum tree over 100000 members built in 478 ms: root aace5228, 100 of burst and 0.1 a Pulse of refill in all
orchestrator signs: As agent 0df519ab, commit to a cohort of 100000 subagents, with 100 USD at once, refilled by 0.1 USD a Pulse in all, their buckets full from now. Fee 0.0011 USD; valid until Pulse 102.
Pulse 85: Done: cohort 01822874 of 100000 committed under 0df519ab (lineage indices 6 to 100005); fee 0.0011 USD.
cohort 0182287460cd12a89569c79c897d39fe0962b42b7b24f20fae2da933d363e9b9
members: lineage indices 6 to 100005 under agent 0df519ab, kept in $SION_HOME/agents/cohorts/0182287460cd12a89569c79c897d39fe0962b42b7b24f20fae2da933d363e9b9.json
```

A member costs nothing on the ledger until it is needed. `sion agent
materialize` creates members, up to 128 a transaction, each with a proof
of its leaf: its charter and allotment are created at its lineage id, and
its budget is paid from the parent's allotment then. Each member is
keyless and gets a `.keyless` wallet file, `member-<index>`, so it pays
through the orchestrator's signature once it has a little USD for fees.
Here member-0's bucket holds 0.001 USD and refills by a millionth of a
USD each Pulse, so a second reading right after the first is refused:

```text
$ sion agent materialize orchestrator 0182287460cd --next 3
orchestrator signs: Materialize 3 members of cohort 01822874, with 0.15 USD in budgets in all. Fee 0.0014 USD; valid until Pulse 105.
Pulse 88: Done: member 0 of cohort 01822874 materialized as agent 01fc0eac, keyless; member 1 of cohort 01822874 materialized as agent e1fc482c, keyless; member 2 of cohort 01822874 materialized as agent 86bfed80, keyless; fee 0.0014 USD.
member-0: agent 01fc0eac8f1b1b0ca007fa20a0037f814f3d6dcf9224d6aa00d78bee51a35aa2 account dbbdbdb38c32f6565ae709ce3b917792773a18228865fdeea09f1568ce400d0a allotment 3b828b4f52ec474c3ce31d94d35c0c49970f1d0c82094665cd17934ca4026d46 (keyless; orchestrator signs for it)
member-1: agent e1fc482c5e5dd9d557d1641ec8e3e71838f722f11ae7ba971b4572e3d40a8dee account dbe373e8baec6927d877eef931d1530f6fea7a9f6455dc28131e6df9c3e7e72a allotment a9164c09de70bf5dc8476ef536ea1891f2aa52a4442b5baf2880a1ee2b81f0a6 (keyless; orchestrator signs for it)
member-2: agent 86bfed80c65a0beebc20e47ed8a63962c54d3128b248f1827be06484d9cb570e account 2a2b17d01e9951506401bfaf3d3f447f9023f685fc657686759c5331c79b5ae6 allotment b8a59997e9d5d590aa4cdcb9bc5291eecacb65603af64f85b29f5d4afe4f993a (keyless; orchestrator signs for it)
$ sion send orchestrator member-0 0.01
orchestrator signs: Pay 0.01 USD to member-0. Fee 0.0011 USD; valid until Pulse 108.
Pulse 90: Done: paid 0.01 USD to member-0; fee 0.0011 USD.
  digest b3cba090186b783cb8d04b1b828eb288116971e66601e42f84e3768478498eee
$ sion charter pay member-0 01fc0eac…5aa2 3b828b4f…6d46 carol 0.001 --purpose 'a sensor reading'
orchestrator signs, as a batch of one: As an agent under charter 01fc0eac, pay 0.001 USD to carol for "a sensor reading". Fee 0.0011 USD; valid until Pulse 110.
Pulse 93: Done: agent paid 0.001 USD to carol (receipt claim 87b480e0); fee 0.0011 USD.
  digest a614c131b856a5ed8433af2cb6e93f21beed0589e3f79c98c2d619aa72004734
$ sion charter pay member-0 01fc0eac…5aa2 3b828b4f…6d46 carol 0.001 --purpose 'another reading'
orchestrator signs, as a batch of one: As an agent under charter 01fc0eac, pay 0.001 USD to carol for "another reading". Fee 0.0011 USD; valid until Pulse 113.
Pulse 95: Failed (charter caveat violated: the bucket holds 0.000002, below the payment 0.001); nothing happened except the fee of 0.0011 USD.
  digest bacbf8c01e2b0643627a496382fb866c837391f1c6e40a10725327ca19f7a5b7
```

With `--full`, the members' buckets start full and the parent gives up
those tokens when the cohort is created; without it, each member's bucket
starts empty at the cohort's creation and fills at its rate, as if the
member had existed since. A member is materialized once:

```text
$ sion agent materialize orchestrator 0182287460cd --index 0
orchestrator signs: Materialize 1 member of cohort 01822874, with 0.05 USD in budgets in all. Fee 0.0012 USD; valid until Pulse 115.
Pulse 98: Failed (invalid transaction: member 0 is already materialized); nothing happened except the fee of 0.0012 USD.
```

## 8. Inspect

`sion agent show` prints the agent's account, principal, backer, card,
lineage, status, caveats, what its bucket lets it pay in the next Pulse
and what it keeps after its children's shares, and its remaining budget,
amounts in USD:

```text
$ sion agent show 01fc0eac…5aa2
{
  "account": "dbbdbdb38c32f6565ae709ce3b917792773a18228865fdeea09f1568ce400d0a",
  "agent": "01fc0eac8f1b1b0ca007fa20a0037f814f3d6dcf9224d6aa00d78bee51a35aa2",
  "allotments": [
    "3b828b4f52ec474c3ce31d94d35c0c49970f1d0c82094665cd17934ca4026d46"
  ],
  "backer": "42424444042f6eae3f1f69e9d99e163216bd317aed703c8eb19a98346b1eeb23",
  "budget_remaining_usd": "0.049",
  "burst_usd": "0.001",
  "can_spend_next_pulse_usd": "0.000006",
  "card": "0000000000000000000000000000000000000000000000000000000000000000",
  "created_at": 88,
  "depth": 1,
  "did": "did:sion:94425d7211b21492c4a9147ba3b67dc08bf05d315c8f89dc4c2a299d5d596e5a:01fc0eac8f1b1b0ca007fa20a0037f814f3d6dcf9224d6aa00d78bee51a35aa2",
  "expires_at": 100033,
  "kept_after_children": {
    "burst_usd": "0.001",
    "refill_usd_per_pulse": "0.000001"
  },
  "keyless": true,
  "lineage": [
    {
      "account": "6193f13a6a7737d74427d41da00fcfed4551eedab7ce5fb3be772f5bd61a5b4e",
      "agent": "0df519abdf18a35258c4047d5a6cbc36f60edf0b137b8c96236e60dbe0ac2b98",
      "narrowing": null,
      "revoked_at": null,
      "suspended_by": null
    }
  ],
  "lineage_path": [
    6
  ],
  "lineage_root": "0df519abdf18a35258c4047d5a6cbc36f60edf0b137b8c96236e60dbe0ac2b98",
  "max_per_payment_usd": "25",
  "may_spawn": false,
  "narrowing": null,
  "principal": "6193f13a6a7737d74427d41da00fcfed4551eedab7ce5fb3be772f5bd61a5b4e",
  "refill_usd_per_pulse": "0.000001",
  "signed_for_by": "6193f13a6a7737d74427d41da00fcfed4551eedab7ce5fb3be772f5bd61a5b4e",
  "status": "active"
}
```

The node answers the same question with proofs: `GET /v1/agents/{id}`
returns the agent's charter and the overlay of the agent and of each
ancestor, each with a state proof (an overlay that does not exist comes
with a proof of its absence), so a client can check an agent's status
against a certified header ([HTTP API](../reference/http-api.md#get-v1agentsid)):

```text
$ curl -s $SION_NODE/v1/agents/01fc0eac…5aa2 | jq '{pulse, status, depth, bucket, overlays: [.overlays[] | {agent, present: (.overlay.object != null)}]}'
{
  "pulse": 98,
  "status": "active",
  "depth": 1,
  "bucket": {
    "allotment": "3b828b4f52ec474c3ce31d94d35c0c49970f1d0c82094665cd17934ca4026d46",
    "burst": 1000,
    "can_spend_next_pulse": 6,
    "rate_per_pulse": 1,
    "remaining": 49000,
    "spawned": 0
  },
  "overlays": [
    {
      "agent": "01fc0eac8f1b1b0ca007fa20a0037f814f3d6dcf9224d6aa00d78bee51a35aa2",
      "present": false
    },
    {
      "agent": "0df519abdf18a35258c4047d5a6cbc36f60edf0b137b8c96236e60dbe0ac2b98",
      "present": true
    }
  ]
}
```

## 9. Suspend, resume, revoke

```text
$ sion agent suspend alice 0df519ab…2b98
alice signs: Suspend agent 0df519ab and every agent below it. Fee 0.0011 USD; valid until Pulse 119.
Pulse 102: Done: agent 0df519ab suspended by alice, with everything below it; fee 0.0011 USD.
$ sion charter pay worker-2 931f6c9f…c802 cad64154…6d9d carol 1 --purpose 'while suspended'
orchestrator signs, as a batch of one: As an agent under charter 931f6c9f, pay 1 USD to carol for "while suspended". Fee 0.0011 USD; valid until Pulse 122.
Pulse 105: Failed (charter caveat violated: charter suspended); nothing happened except the fee of 0.0011 USD.
  digest a04b92c6df8f71766921fb29ebc483362e02c5671d546c823b657274416bbb6d
$ sion agent show 931f6c9f…c802
{
  …
  "agent": "931f6c9f85bf83dc2e6bb4953b557f44ec9944380f854fe97e66c0f38948c802",
  "lineage": [
    {
      "account": "6193f13a6a7737d74427d41da00fcfed4551eedab7ce5fb3be772f5bd61a5b4e",
      "agent": "0df519abdf18a35258c4047d5a6cbc36f60edf0b137b8c96236e60dbe0ac2b98",
      "narrowing": null,
      "revoked_at": null,
      "suspended_by": "42424444042f6eae3f1f69e9d99e163216bd317aed703c8eb19a98346b1eeb23"
    }
  ],
  "narrowing": null,
  "status": "stopped by an ancestor"
}
$ sion agent resume alice 0df519ab…2b98
alice signs: Lift the suspension of agent 0df519ab. Fee 0.0011 USD; valid until Pulse 125.
Pulse 109: Done: agent 0df519ab resumed; fee 0.0011 USD.
$ sion charter pay worker-2 931f6c9f…c802 cad64154…6d9d carol 1 --purpose 'after the resume'
orchestrator signs, as a batch of one: As an agent under charter 931f6c9f, pay 1 USD to carol for "after the resume". Fee 0.0011 USD; valid until Pulse 129.
Pulse 111: Done: agent paid 1 USD to carol (receipt claim a14de858); fee 0.0011 USD.
  digest 1554b99128e924e0d70d85a4e7928db70876c73a7625d74fbb24948d58228102
$ sion agent revoke orchestrator c6c9fa40…0b4a
orchestrator signs: Revoke charter c6c9fa40 and every charter delegated from it. Fee 0.0011 USD; valid until Pulse 131.
Pulse 114: Done: charter c6c9fa40 revoked; fee 0.0011 USD.
$ sion agent reclaim alice c6c9fa40…0b4a
alice signs: Return what is left in dead charter c6c9fa40's allotments to its backer. Fee 0.0011 USD; valid until Pulse 134.
Pulse 116: Done: 22 USD returned from charter c6c9fa40; fee 0.0011 USD.
```

Any overseer in good standing may suspend: the backer, the principal, or
the agent of any charter above, as long as neither its own charter nor
one above it is revoked or suspended. The highest suspender is recorded:
a suspension from higher in the tree replaces one from lower down, never
the reverse, and only the recorded suspender or the backer may resume.
An agent may revoke itself. Only a revoked or expired agent can be
reclaimed, by its principal or its backer, and the funds go to the
backer.

## 10. Check an agent's standing as a light client

`sion agent show` reports what the node says. `sion agent status` checks
it instead, as a light client would: the agent's charter and the
overlays of every charter of its lineage are proven against one
certified header, the status is worked out here from what was proven,
by the same rule the node uses, and the agent's allotments are proven
too, for its budget.
`--chain` names the chain identity the proofs must verify against (or
`SION_CHAIN`); without it the node vouches for itself. `--min-pulse`
refuses proofs from more than 16 Pulses before a Pulse you know of.

lead launches a helper, whose standing depends on lead's:

```text
$ sion agent spawn lead 908f6ddc…7432 e469664a…adb4 --count 1 --budget 20 --max-payment 5 --prefix helper
lead signs: As agent 908f6ddc, launch 1 subagent with its own identity, 20 USD in budgets and 20 USD at once, refilled by 0.1 USD a Pulse in all. Fee 0.0012 USD; valid until Pulse 26.
Pulse 9: Done: subagent 5ff8d547 launched by 908f6ddc, signing as helper-1; fee 0.0012 USD.
helper-1: agent 5ff8d547…3906 allotment b0a68512…e6c7
$ sion agent status 5ff8d547…3906 --chain 94425d72…6e5a
agent 5ff8d547: active after Pulse 9 (depth 1, backed by alice)
budget: 20 USD left, 20 USD can be spent in the next Pulse
3 objects proven against Pulse 9's header, certified by 300000 of the committee's stake (quorum 266667), chain 94425d72
budget: state proven against Pulse 9's header, certified by 300000 of the committee's stake (quorum 266667), chain 94425d72
```

The three objects are the helper's charter and the overlays of lead and
the helper, each proven present or absent: neither has been suspended,
revoked or narrowed yet.

When alice suspends lead, the helper is stopped too, and the check shows
it from the proven overlay of the charter above it. A node that left
that overlay out, to show the helper as active, would fail the check:
every charter of the lineage must be proven.

```text
$ sion agent suspend alice 908f6ddc…7432
alice signs: Suspend agent 908f6ddc and every agent below it. Fee 0.0011 USD; valid until Pulse 29.
Pulse 12: Done: agent 908f6ddc suspended by alice, with everything below it; fee 0.0011 USD.
$ sion agent status 5ff8d547…3906 --chain 94425d72…6e5a
agent 5ff8d547: stopped by an ancestor after Pulse 12 (depth 1, backed by alice)
budget: 20 USD left; nothing can be spent while it is stopped by an ancestor
3 objects proven against Pulse 12's header, certified by 300000 of the committee's stake (quorum 266667), chain 94425d72
budget: state proven against Pulse 12's header, certified by 300000 of the committee's stake (quorum 266667), chain 94425d72
```

Against another chain identity, nothing verifies:

```text
$ sion agent status 5ff8d547…3906 --chain 00000000…0001
error: verification failed: header chain: the node serves chain 94425d72, not the pinned chain 00000000
[exit 1]
```

```text
$ sion agent resume alice 908f6ddc…7432
alice signs: Lift the suspension of agent 908f6ddc. Fee 0.0011 USD; valid until Pulse 32.
Pulse 15: Done: agent 908f6ddc resumed; fee 0.0011 USD.
$ sion agent status 5ff8d547…3906 --chain 94425d72…6e5a
agent 5ff8d547: active after Pulse 15 (depth 1, backed by alice)
budget: 20 USD left, 20 USD can be spent in the next Pulse
3 objects proven against Pulse 15's header, certified by 300000 of the committee's stake (quorum 266667), chain 94425d72
budget: state proven against Pulse 15's header, certified by 300000 of the committee's stake (quorum 266667), chain 94425d72
```

The app's Agents tab makes the same check for the agents a device
watches ([The app](../getting-started/app.md)).
