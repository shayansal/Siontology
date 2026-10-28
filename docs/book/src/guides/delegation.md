# Delegating stake

This guide sets a validator's commission, delegates stake to it, collects
the rewards, and leaves. It needs a devnet whose epochs end, so that
rewards are paid:

```bash
sion devnet --epoch-length 12 --unbonding 6
```

For background, see [Pooled stake](../concepts/validators/pooled-stake.md).
The devnet's genesis validators are operated by the deterministic wallets
`operator-0` to `operator-3`; `alice`, `bob` and `carol` are funded at
genesis.

## 1. The validators and their pools

```bash
sion stake pools
```

```text
epoch 0, ending after Pulse 12; stake asset USD (test stake)
validator 91fa8cbb  operator operator-1  active  own 100,000  delegated 0 (earning 0)  commission 0%  weight 100000
validator 9c813b07  operator operator-3  active  own 100,000  delegated 0 (earning 0)  commission 0%  weight 100000
validator d7c1b77d  operator operator-0  active  own 100,000  delegated 0 (earning 0)  commission 0%  weight 100000
validator f28a5456  operator operator-2  active  own 100,000  delegated 0 (earning 0)  commission 0%  weight 100000
```

Validators are named by a prefix of their identifier; any unique prefix
works in the commands below.

## 2. The operator sets a commission

```bash
sion faucet operator-0 10
sion stake commission operator-0 d7c1b77d 10
```

```text
operator-0 signs: Set validator d7c1b77d's commission to 10% from the epoch's end. Fee 0.0012 USD; valid until Pulse 23.
Pulse 5: Done: validator d7c1b77d's commission becomes 10% at the epoch's end; fee 0.0012 USD.
```

Nobody has delegated to this pool yet, so the commission may start
anywhere up to 25 %. Above that the command refuses (`the commission may
be at most 25%`), and only the operator may set it:

```text
Pulse 8: Failed (unauthorized: only the validator's operator may do this); nothing happened except the fee of 0.0011 USD.
```

## 3. Delegate

```bash
sion faucet alice 20000
sion stake delegate alice d7c1b77d 5000
```

```text
alice signs: Delegate 5,000 USD to validator d7c1b77d's pool; you keep the delegation, which earns from the epoch's end. Fee 0.0011 USD; valid until Pulse 30.
Pulse 13: Done: alice delegated 5,000 USD to validator d7c1b77d as delegation ec4d550d, earning from the epoch's end; fee 0.0011 USD.
delegation ec4d550d3ef56994839befa50558803d3d25a0090c973a45372133969c4ad79d
```

Below the minimum of 1 the delegation fails (`a delegation must be at least
1`). `sion stake show alice` lists alice's delegations:

```json
{
  "delegation": "ec4d550d3ef56994839befa50558803d3d25a0090c973a45372133969c4ad79d",
  "rewards_owed": "0",
  "shares": "5000000000",
  "since": 13,
  "stake": "5,000",
  "state": "pending: earns from the end of this epoch",
  "validator": "d7c1b77db0ae57df54351fc2e4c60d8557ec8a9ba3f01e567431a45fb8198c59"
}
```

After the epoch ends, the pool's stake counts towards the validator's
weight and the commission is in force:

```text
validator d7c1b77d  operator operator-0  active  own 100,000  delegated 5,000 (earning 5,000)  commission 10%  weight 105000
```

## 4. Collect

Rewards come from fees: each epoch, 70 % of the fees go to the validators
by weight, and this validator's part is split with its pool. On an idle
devnet the fees are a few tenths of a cent, so the rewards are tiny, but
they are exact. After a few epochs with some transfers:

```bash
sion stake show alice        # "rewards_owed": "0.00007", "state": "earning"
sion stake claim alice ec4d550d
```

```text
Pulse 72: Done: 0.00007 USD of rewards collected from delegation ec4d550d; fee 0.0011 USD.
```

## 5. Leave

```bash
sion stake undelegate alice ec4d550d
```

```text
Pulse 74: Done: delegation ec4d550d stopped earning (0.000009 USD of rewards paid) and is withdrawable from Pulse 80; fee 0.0011 USD.
```

Undelegating pays what the delegation is owed and starts the unbonding
period, during which the stake stays in the pool and can still be slashed.
Withdrawing early fails:

```text
Pulse 77: Failed (invalid transaction: the delegation is withdrawable from Pulse 80); nothing happened except the fee of 0.0011 USD.
```

From Pulse 80:

```bash
sion stake withdraw alice ec4d550d
```

```text
Pulse 87: Done: 5,000 USD withdrawn from delegation ec4d550d; fee 0.0011 USD.
```

## For agents

The MCP server offers the same operations: `sion_stake_pools`,
`sion_stake_delegate`, `sion_stake_show`, `sion_stake_claim`,
`sion_stake_undelegate` and `sion_stake_withdraw`.
