# Error messages

The messages you are most likely to meet, what they mean, and what to do.
Each was produced by a real command unless the section says it comes from
the code.

## Two kinds of failure

- **The command fails** before anything reaches the ledger, or the ledger
  refuses to certify it. The CLI prints `error: …` on standard error and
  exits with status 1. Nothing was charged.
- **The transaction executes and fails.** The receipt reads `Failed (…);
  nothing happened except the fee of … USD.` and the CLI exits with status
  0: the transaction was processed and its fee was charged. A transaction
  that fails preparation (expired, wrong input version, bad signature, fee
  too low) is `Rejected (…); nothing changed and no fee was charged.`

## Connecting and arguments

| Message | Meaning |
|---|---|
| `error: cannot reach the node: http://127.0.0.1:9999/v1/genesis: …: Connection Failed: Connect error: Connection refused (os error 111)` | No node at `--node`; start `sion devnet` or set `SION_NODE` |
| `error: "1.1234567" is not an amount with at most 6 decimals` | Amounts have at most six decimals |
| `error: "abc" is not an amount` | Not a decimal number |
| `error: claim must be a 64-digit hex ID` | Identifiers are 64 hex digits |
| `error: digest must be a 64-digit hex ID` | The same, for a transaction digest |
| `error: --chain takes 64 hex digits` | A chain identity is 64 hex digits |
| `error: side must be buy or sell, not hold` | Market orders are `buy` or `sell` |
| `error: unknown bond class foo` | `--class` is `economic`, `adjudicator` or `institutional` |
| `error: --charter and --allotment go together` | Paying under a charter needs both |
| `error: unknown demo "nope"; try invoice, agents, market, receipts, x402, censorship, light, staking or all` | A demo name |
| `error: unknown benchmark group "foo"; groups: signatures, state, data, clearing, assurance, execution, devnet` | A benchmark group |

## Wallets and money

| Message | Meaning |
|---|---|
| `error: insufficient funds: need 5.001, have 0` | The wallet's coins do not cover the amount plus the fee. `need` includes the fee |
| `error: $HOME/.sion/wallets/dana.seed already exists` | `sion keys new` never overwrites a seed |
| `error: amount must be between 1 micro-unit and one million units` | The faucet's limits |
| `error: <digest> did not execute in time` | The CLI waited 90 seconds. The gateway may be censoring you: try `--forced` |
| `error: not certified: the transaction has expired` | The transaction's 20-Pulse validity passed before it was certified (seen on a devnet with `--insecure-fast-signatures`) |
| `rejected: object ObjectRef { id: ObjectId(27657e3b), version: Version(0) } is locked to transaction TxDigest(93d6ac50)` | Another transaction already locked that coin: a double spend |

## Looking things up

| Message | Meaning |
|---|---|
| `unknown transaction` (from `sion tx`, exit status 0) | The node has never seen this digest |
| `error: verification failed: unknown transaction` | The same, from `sion receipt` |
| `error: verification failed: never certified: …` | The transaction was rejected; there is no receipt |
| `error: unknown claim` | No such claim in the log, or it was rejected when it was to join it |
| `error: unknown or pruned Pulse` | The Pulse is in the future or older than the node's 256-Pulse history |
| `error: no such market` | No market with that identifier |
| `error: no sealed batch to clear` | The latest batch has not sealed, or was already cleared or refunded |

## Verification

| Message | Meaning |
|---|---|
| `error: verification failed: header chain: the node serves chain 6345655c, not the pinned chain 00000000` | The node serves another chain than the one you pinned. Do not trust it |
| `error: verification failed: no certified Pulse yet` | Also printed when the devnet has no prover: use `--no-world` |
| `error: the epoch's boundary Pulse is not certified yet` | An epoch just began; retry in a few seconds |

## Transactions that fail on the ledger

These are the `Failed (…)` reasons of common operations:

| Reason | Operation |
|---|---|
| `missing object 0000…` | The transaction names an object that does not exist (here, an unknown market) |
| `assurance: challenge: claim names no adjudicator; only cryptographic evidence can break its atoms` | Challenging a claim that names no adjudicator |
| `invalid transaction: claim not accepted: cost of corruption 10 (after detection discount 1000/1000) is below κ·V = 200; only 10 of backing is unreserved; κ·V = 200 is needed` | `sion accept` for more than the claim's backing covers |
| `invalid transaction: unknown challenge` | Ruling on a challenge that does not exist |
| `charter caveat violated: payee not allowed` | An agent paying someone its charter does not list |
| `charter caveat violated: payment 60 above per-payment limit 50` | Over the per-payment limit |
| `charter caveat violated: window total 85 above limit 80` | Over the per-window budget |
| `charter caveat violated: charter revoked` | The charter, or one it was delegated from, was revoked |

A claim whose formula fails its checks is not a failed transaction: the
transaction succeeds and the claim is rejected in the deferred phase, with
an event such as
`{"ClaimRejected":{"claim":"746b…","reason":"unknown atom b758c281…"}}`.

## Validator transactions

From the reference executor (no CLI command sends these yet); see
[Staking](../concepts/validators/staking.md#when-a-step-is-refused) and
[Slashing for equivocation](../concepts/validators/equivocation-slashing.md#when-a-report-is-refused).

## Forced intents

A forced intent that cannot be posted is dismissed with a reason in its
`ForcedDismissed` event. From the reference executor:

| Reason | Meaning |
|---|---|
| `already pending` | The same intent is already owed |
| `too many intents pending` | 4,096 obligations are pending |
| `faucet transactions cannot be forced` | |
| `it expires before its shard could include it` | Its expiry is at or before this Pulse |
| `it needs 1 to 256 inputs` | |
| `duplicate inputs` | |
| `fee below the minimum` | |
| `unknown account and no genesis policy` | A new sender must carry its genesis policy |
| `invalid genesis policy` | |
| `its authorization does not verify` | The signatures do not satisfy the sender's policy |
| `input … does not exist` | |
| `input … was spent or moved (executed or invalid)` | The input is no longer at that version, often because the intent already executed |
| `input … is not the sender's` | |
| `inputs[0] must be a USD coin covering the fee` | |

## x402

| Message | Meaning |
|---|---|
| `error: the server asks 0.25 USD, more than the 0.1 USD this client may pay` | Raise `--max` (or `max_usd`) or do not buy |
| `402: unknown quote, or one already used` (server answer) | A replayed or expired payment |

## HTTP API

| Status | Body |
|---|---|
| 404 | `{"error":"no route for /v1/nothing"}` |
| 400 | `{"error":"\"xyz\" is not a 64-digit hex identifier"}` |
| 400 | ``{"error":"bad JSON body: invalid type: integer `5`, expected a string at line 1 column 8"}`` |
| 400 | `{"error":"tx is not a Borsh SignedTransaction"}` |
| 400 | `{"error":"the epoch must be a number"}` |
| 404 | `{"error":"unknown transaction"}` |
| 404 | `{"error":"no proof for that epoch (it has not begun, or began before this validator kept proofs)"}` |

## MCP

| Message | Meaning |
|---|---|
| `{"error":{"code":-32601,"message":"method not found: resources/list"},…}` | The server offers tools only |
| `unknown tool sion_teleport` (`isError: true`) | No such tool |
