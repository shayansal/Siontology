# Command line

Every `sion` command and flag, as `--help` prints them, with notes. The
guides show the commands in use.

## Conventions

- **The node.** Every command talks to the node at `--node`, or
  `SION_NODE`, or `http://127.0.0.1:8730`. `sion devnet` serves it; `sion
  demo --local` and `sion bench` start their own in-process devnet.
- **Wallets.** Arguments named `FROM`, `PRINCIPAL`, `OPERATOR`, `SOLVER` or
  `--wallet` are wallet names: a seed file in `$SION_HOME/wallets` (default
  `~/.sion/wallets`), or else the deterministic devnet wallet of that name.
  Arguments that only name an account (`TO`, `ACCOUNT`, `PAYEE`) also take a
  64-digit hex account identifier. See
  [Wallets and keys](../guides/wallets.md).
- **Amounts** are decimal with at most six places (`12.5`, `0.000001`).
  JSON output shows micro-units (`12500000`).
- **Identifiers** are 64 hex digits. Receipts and sentences show the first
  8.
- **What you sign.** Every command that signs prints the transaction as a
  sentence first (`dana signs: …`), then waits up to 90 seconds for it to
  execute and prints the outcome (`Pulse 5: Done: …`) and the digest.
- **Pinning.** `--chain` (or `SION_CHAIN`) on `verify`, `receipt`, `x402
  serve` and `x402 fetch` pins the chain identity. Without it, the node
  vouches for itself, and the output says so.
- **Exit status.** 0 on success, 1 on any error, with the message on
  standard error as `error: …`. A transaction that executes but fails
  (`Failed (…)`) still exits 0: the transaction was processed, and you paid
  its fee. One exception: `sion tx` with an unknown digest prints `unknown
  transaction` and exits 0.

`sion --version` prints `sion 0.1.0`.

## Commands

| Area | Commands |
|---|---|
| Network | `devnet`, `status`, `pulse`, `events`, `forced`, `censor` |
| Wallets and money | `keys new`, `keys show`, `balance`, `faucet`, `send` |
| Knowledge | `bond`, `claim issue`, `claim show`, `claim decide`, `accept`, `challenge`, `rule` |
| Agents | `charter create`, `charter pay`, `charter revoke`, `mcp`, `x402 serve`, `x402 fetch` |
| Markets | `market create`, `market order`, `market clear`, `market show` |
| Verification | `tx`, `receipt`, `verify` |
| Other | `demo`, `bench` |

There is no command yet for staking, unstaking, withdrawing stake,
claiming rewards, reporting equivocation, withdrawing a bond, releasing a
reservation, rotating keys, delegating or reclaiming a charter, escrow
lanes, or sealed market orders. Those transactions are available through
the Rust API ([Transaction kinds](transactions.md)).

## `sion`

```text
Siontology: a neo-blockchain devnet and client

Usage: sion [OPTIONS] <COMMAND>

Commands:
  devnet     Run a devnet and serve its API
  status     Chain status
  keys       Wallet keys
  balance    An account's balances and coins
  faucet     Devnet faucet
  send       Transfer
  forced     Forced-inclusion obligations: intents posted directly to consensus that their shard has not yet executed, and any that are overdue
  censor     ADVERSARY SIMULATION (devnet only): censor accounts at the gateway and/or at the first N validators. No accounts: stop censoring
  bond       Post a bond behind your bonded atom `bond:<you>:<label>`
  claim      Claims
  accept     Rely on a claim with insurance: reserve κ·V of its backing
  challenge  Challenge a bonded atom a claim rests on
  rule       Rule on a challenge as the claim's adjudicator
  charter    Agent Charters
  market     Markets
  tx         A transaction's status and effects
  receipt    A transaction's receipt, verified as a light client would
  pulse      A Pulse's header, certificate and summary
  events     Recent events
  verify     Verify a Pulse as a light client
  demo       Run a scripted demo: invoice, agents, market, receipts, censorship, light or all
  mcp        Serve the Model Context Protocol on stdio (for AI agents)
  x402       Sell or buy HTTP resources with x402 (HTTP 402 payments settled on Siontology)
  bench      Measure primitives, execution, proving, clearing and a devnet
  help       Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
  -V, --version      Print version
```

## `sion devnet`

```text
Run a devnet and serve its API

Usage: sion devnet [OPTIONS]

Options:
      --node <NODE>
          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --validators <VALIDATORS>
          Genesis validators (the first active set) [default: 4]
      --standby <STANDBY>
          Standby validators: they run from the start, follow the chain without a seat, and join the active set when their operator stakes for them (`operator-<index>`) [default: 0]
      --epoch-length <EPOCH_LENGTH>
          Pulses per epoch: committees change at epoch boundaries (0: one epoch that never ends) [default: 0]
      --unbonding <UNBONDING>
          Pulses between leaving the active set and withdrawing stake [default: 20]
      --forced-window <FORCED_WINDOW>
          Pulses a shard has to execute a forced intent before its committee is replaced [default: 8]
      --shards <SHARDS>
          Lane shards [default: 2]
      --seats <SEATS>
          Seats per shard committee [default: 4]
      --rounds-per-pulse <ROUNDS_PER_PULSE>
          Consensus rounds per Pulse [default: 4]
      --data-dir <DATA_DIR>
          Keep logs and snapshots here (restarts resume from them)
      --api <API>
          Where to serve the API [default: 127.0.0.1:8730]
      --insecure-fast-signatures
          INSECURE: replace dual post-quantum validator signatures with a fast simulation MAC (load experiments only)
      --no-prove
          Do not run the prover
      --chunk <CHUNK>
          Transactions per proof chunk [default: 16]
      --accounts <ACCOUNTS>
          Named accounts funded at genesis [default: alice,bob,carol]
      --duration <DURATION>
          Stop after this many seconds (0: run until interrupted) [default: 0]
      --report <REPORT>
          Print status every this many seconds (0: silent) [default: 5]
  -h, --help
          Print help
```

See [Running a devnet](../guides/devnet.md).

## `sion status`

```text
Chain status

Usage: sion status [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

Prints `GET /v1/status` as JSON; see [HTTP API](http-api.md#get-v1status).

## `sion keys`

```text
Wallet keys

Usage: sion keys [OPTIONS] <COMMAND>

Commands:
  new   Create a wallet with a random seed
  show  Show a wallet's account and public keys
  help  Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion keys new`

```text
Create a wallet with a random seed

Usage: sion keys new [OPTIONS] <NAME>

Arguments:
  <NAME>  Name

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

Refuses to overwrite an existing seed file. See [Wallets and keys](../guides/wallets.md).

## `sion keys show`

```text
Show a wallet's account and public keys

Usage: sion keys show [OPTIONS] <NAME>

Arguments:
  <NAME>  Name

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

Works for deterministic devnet wallets too: any name without a seed file.

## `sion balance`

```text
An account's balances and coins

Usage: sion balance [OPTIONS] <ACCOUNT>

Arguments:
  <ACCOUNT>  Wallet name or account ID

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

Assets other than USD are shown by their short identifier.

## `sion faucet`

```text
Devnet faucet

Usage: sion faucet [OPTIONS] <ACCOUNT> <AMOUNT>

Arguments:
  <ACCOUNT>  Wallet name or account ID
  <AMOUNT>   Amount (decimal)

Options:
      --asset <ASSET>  Asset: usd, a name such as gpu-hour, or a hex ID [default: usd]
      --node <NODE>    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help           Print help
```

Devnet only. 1 micro-unit to 1,000,000 units per request.

## `sion send`

```text
Transfer

Usage: sion send [OPTIONS] <FROM> <TO> <AMOUNT>

Arguments:
  <FROM>    Sending wallet
  <TO>      Recipient (wallet name or account ID)
  <AMOUNT>  Amount (decimal)

Options:
      --asset <ASSET>  Asset [default: usd]
      --node <NODE>    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --forced         Post it as a forced intent, straight to consensus, bypassing the gateway (for when your transactions are being ignored)
  -h, --help           Print help
```

See [Transfers](../guides/transfers.md).

## `sion forced`

```text
Forced-inclusion obligations: intents posted directly to consensus that their shard has not yet executed, and any that are overdue

Usage: sion forced [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

Prints `GET /v1/forced` as JSON. An obligation stays listed until its deadline, even if the intent executed earlier.

## `sion censor`

```text
ADVERSARY SIMULATION (devnet only): censor accounts at the gateway and/or at the first N validators. No accounts: stop censoring

Usage: sion censor [OPTIONS] [ACCOUNTS]...

Arguments:
  [ACCOUNTS]...  Wallet names or account IDs to censor

Options:
      --gateway                  The gateway drops their transactions silently
      --node <NODE>              The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --validators <VALIDATORS>  This many validators ignore them too (more than a third of a committee can stall certification) [default: 0]
  -h, --help                     Print help
```

**Adversary simulation**, devnet only. See [Forced inclusion](../guides/forced-inclusion.md).

## `sion bond`

```text
Post a bond behind your bonded atom `bond:<you>:<label>`

Usage: sion bond [OPTIONS] <FROM> <LABEL> <AMOUNT>

Arguments:
  <FROM>    Wallet
  <LABEL>   Label
  <AMOUNT>  USD

Options:
      --class <CLASS>  economic, adjudicator or institutional [default: economic]
      --node <NODE>    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help           Print help
```

See [Bonds and challenges](../guides/bonds-and-challenges.md).

## `sion claim`

```text
Claims

Usage: sion claim [OPTIONS] <COMMAND>

Commands:
  issue   Issue a claim
  show    Show a claim, its status and cost of corruption
  decide  Apply the decision rule
  help    Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion claim issue`

```text
Issue a claim

Usage: sion claim issue [OPTIONS] <FROM> <SUBJECT>

Arguments:
  <FROM>     Issuer's wallet
  <SUBJECT>  One-line statement

Options:
      --node <NODE>                The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --schema <SCHEMA>            Schema [default: note/v1]
      --bond <BOND>                Rest it on your bond of this label
      --adjudicator <ADJUDICATOR>  Adjudicator atom ID
      --valid-for <VALID_FOR>      Validity in Pulses [default: 10000]
  -h, --help                       Print help
```

See [Issuing and assessing claims](../guides/claims.md).

## `sion claim show`

```text
Show a claim, its status and cost of corruption

Usage: sion claim show [OPTIONS] <CLAIM>

Arguments:
  <CLAIM>  Claim ID

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

Prints `GET /v1/claims/{id}` as JSON.

## `sion claim decide`

```text
Apply the decision rule

Usage: sion claim decide [OPTIONS] <CLAIM> <VALUE>

Arguments:
  <CLAIM>  Claim ID
  <VALUE>  Value at risk (USD)

Options:
      --kappa <KAPPA>        κ [default: 2]
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --distrust <DISTRUST>  Atoms to distrust (hex IDs)
  -h, --help                 Print help
```

Prints `POST /v1/claims/{id}/decide` as JSON, under the standard policy with your κ and distrusted atoms.

## `sion accept`

```text
Rely on a claim with insurance: reserve κ·V of its backing

Usage: sion accept [OPTIONS] <FROM> <CLAIM> <VALUE>

Arguments:
  <FROM>   Wallet
  <CLAIM>  Claim ID
  <VALUE>  Value at risk (USD)

Options:
      --node <NODE>      The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --pulses <PULSES>  Pulses of reliance [default: 1000]
      --kappa <KAPPA>    Safety factor κ [default: 2]
  -h, --help             Print help
```

Sends `Accept`: the decision rule runs on the ledger, and the reservation is made only if it passes.

## `sion challenge`

```text
Challenge a bonded atom a claim rests on

Usage: sion challenge [OPTIONS] <FROM> <CLAIM> <ATOM> <DEPOSIT>

Arguments:
  <FROM>     Wallet
  <CLAIM>    Claim ID
  <ATOM>     Atom ID
  <DEPOSIT>  Deposit (USD)

Options:
      --evidence <EVIDENCE>  Evidence (hashed and recorded) [default: ""]
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help                 Print help
```

The evidence text is hashed; only its hash is recorded.

## `sion rule`

```text
Rule on a challenge as the claim's adjudicator

Usage: sion rule [OPTIONS] <FROM> <CHALLENGE>

Arguments:
  <FROM>       Adjudicator's wallet
  <CHALLENGE>  Challenge ID

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --upheld       Whether the challenged atom is broken
  -h, --help         Print help
```

Only the owner of the claim's adjudicator bond can rule.

## `sion charter`

```text
Agent Charters

Usage: sion charter [OPTIONS] <COMMAND>

Commands:
  create  Charter an agent
  pay     Pay as the agent
  revoke  Revoke
  help    Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion charter create`

```text
Charter an agent

Usage: sion charter create [OPTIONS] <PRINCIPAL> <AGENT>

Arguments:
  <PRINCIPAL>  Principal's wallet
  <AGENT>      Agent (wallet or account)

Options:
      --node <NODE>                The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --payees <PAYEES>            Allowed payees (comma-separated; empty: any)
      --max-payment <MAX_PAYMENT>  Largest payment (USD) [default: 100]
      --max-window <MAX_WINDOW>    Largest total per window (USD) [default: 500]
      --window <WINDOW>            Window (Pulses) [default: 1000]
      --funding <FUNDING>          Budget (USD) [default: 1000]
      --bond <BOND>                Bond behind the agent's receipts (USD) [default: 0]
      --lifetime <LIFETIME>        Charter lifetime (Pulses) [default: 100000]
  -h, --help                       Print help
```

Charters created here always issue receipt claims and allow narrower delegation. See [Charters for AI agents](../guides/charters.md).

## `sion charter pay`

```text
Pay as the agent

Usage: sion charter pay [OPTIONS] <AGENT> <CHARTER> <ALLOTMENT> <PAYEE> <AMOUNT>

Arguments:
  <AGENT>      Agent's wallet
  <CHARTER>    Charter ID
  <ALLOTMENT>  Allotment ID
  <PAYEE>      Payee
  <AMOUNT>     USD

Options:
      --node <NODE>        The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --purpose <PURPOSE>  Purpose [default: ""]
  -h, --help               Print help
```

## `sion charter revoke`

```text
Revoke

Usage: sion charter revoke [OPTIONS] <PRINCIPAL> <CHARTER>

Arguments:
  <PRINCIPAL>  Principal's wallet
  <CHARTER>    Charter ID

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion market`

```text
Markets

Usage: sion market [OPTIONS] <COMMAND>

Commands:
  create  Open a market
  order   Submit a plain order
  clear   Clear the latest sealed batch with a certificate
  show    Show a market
  help    Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion market create`

```text
Open a market

Usage: sion market create [OPTIONS] <OPERATOR> <LABEL>

Arguments:
  <OPERATOR>  Operator's wallet
  <LABEL>     Name

Options:
      --good <GOOD>          The good (asset name or ID) [default: gpu-hour]
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --batch <BATCH>        Pulses per batch [default: 6]
      --deadline <DEADLINE>  Pulses allowed for clearing [default: 4]
  -h, --help                 Print help
```

See [Markets and clearing](../guides/markets.md).

## `sion market order`

```text
Submit a plain order

Usage: sion market order [OPTIONS] <FROM> <MARKET> <SIDE> <QUANTITY> <PRICE>

Arguments:
  <FROM>      Wallet
  <MARKET>    Market ID
  <SIDE>      buy or sell
  <QUANTITY>  Units
  <PRICE>     Limit price per unit (USD)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion market clear`

```text
Clear the latest sealed batch with a certificate

Usage: sion market clear [OPTIONS] <SOLVER> <MARKET>

Arguments:
  <SOLVER>  Solver's wallet
  <MARKET>  Market ID

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

Solves the latest sealed batch exactly, builds the optimality certificate and submits `ClearBatch`.

## `sion market show`

```text
Show a market

Usage: sion market show [OPTIONS] <MARKET>

Arguments:
  <MARKET>  Market ID

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion tx`

```text
A transaction's status and effects

Usage: sion tx [OPTIONS] <DIGEST>

Arguments:
  <DIGEST>  Digest

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

For an unknown digest it prints `unknown transaction` and exits with status 0.

## `sion receipt`

```text
A transaction's receipt, verified as a light client would

Usage: sion receipt [OPTIONS] <DIGEST>

Arguments:
  <DIGEST>  Digest

Options:
      --chain <CHAIN>  The chain identity you trust. Without it the node vouches for itself [env: SION_CHAIN=]
      --node <NODE>    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help           Print help
```

Verifies as a light client; see [Light-client verification](../guides/light-client.md).

## `sion pulse`

```text
A Pulse's header, certificate and summary

Usage: sion pulse [OPTIONS] [PULSE]

Arguments:
  [PULSE]  Pulse (default: latest)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

Prints `GET /v1/pulses/{n}` as JSON; signatures appear as arrays of bytes, so the output is long.

## `sion events`

```text
Recent events

Usage: sion events [OPTIONS]

Options:
      --from <FROM>    From Pulse [default: 0]
      --node <NODE>    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --limit <LIMIT>  Most events [default: 50]
  -h, --help           Print help
```

One line per event: the Pulse and the event as JSON. See [Events](events.md).

## `sion verify`

```text
Verify a Pulse as a light client

Usage: sion verify [OPTIONS]

Options:
      --chain <CHAIN>      The chain identity you trust (from `sion devnet`'s banner or another source you trust). Without it the node vouches for itself [env: SION_CHAIN=]
      --node <NODE>        The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --pulse <PULSE>      Pulse (default: latest certified)
      --samples <SAMPLES>  Data-availability samples [default: 16]
      --no-world           Skip the World Proof
  -h, --help               Print help
```

See [Verify as a light client](../getting-started/light-client.md).

## `sion demo`

```text
Run a scripted demo: invoice, agents, market, receipts, censorship, light or all

Usage: sion demo [OPTIONS] [NAME]

Arguments:
  [NAME]  Which demo [default: all]

Options:
      --local        Start an in-process devnet instead of using --node
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

Demos: `invoice`, `agents`, `market`, `receipts`, `x402`, `censorship`,
`light`, `staking`, and `all`, which runs them in that order. With
`--local`, the staking demo gets its own devnet with a standby validator
and 24-Pulse epochs; the others share a default devnet. `sion demo light
--local` on its own fails, because its fresh devnet has no proven Pulse
yet. See [Run the demos](../getting-started/demos.md).

## `sion mcp`

```text
Serve the Model Context Protocol on stdio (for AI agents)

Usage: sion mcp [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

See [MCP tools](mcp.md).

## `sion x402`

```text
Sell or buy HTTP resources with x402 (HTTP 402 payments settled on Siontology)

Usage: sion x402 [OPTIONS] <COMMAND>

Commands:
  serve  Serve one resource for a price per request, paid to PAY_TO
  fetch  Fetch a URL, paying through x402 if the server asks
  help   Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion x402 serve`

```text
Serve one resource for a price per request, paid to PAY_TO

Usage: sion x402 serve [OPTIONS] <PAY_TO>

Arguments:
  <PAY_TO>  Who gets paid (wallet name or account ID)

Options:
      --listen <LISTEN>  Listen address [default: 127.0.0.1:8402]
      --node <NODE>      The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --path <PATH>      The resource's path [default: /v1/resource]
      --price <PRICE>    Price per request (USD) [default: 0.25]
      --body <BODY>      What every paid request returns [default: {"paid":true}]
      --chain <CHAIN>    The chain identity receipts must verify against. Without it the node vouches for itself [env: SION_CHAIN=]
  -h, --help             Print help
```

See [x402](../guides/x402.md).

## `sion x402 fetch`

```text
Fetch a URL, paying through x402 if the server asks

Usage: sion x402 fetch [OPTIONS] --wallet <WALLET> <URL>

Arguments:
  <URL>  The URL

Options:
      --node <NODE>            The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --wallet <WALLET>        The paying wallet (the agent's, when paying under a charter)
      --charter <CHARTER>      Pay under this Agent Charter (ID) instead of by transfer
      --allotment <ALLOTMENT>  The agent's allotment (ID), with --charter
      --max <MAX>              The most to pay (USD) [default: 1]
      --chain <CHAIN>          The chain identity to pay on. Without it the node vouches for itself [env: SION_CHAIN=]
  -h, --help                   Print help
```

## `sion bench`

```text
Measure primitives, execution, proving, clearing and a devnet

Usage: sion bench [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --quick        Smaller sizes
      --no-devnet    Skip the end-to-end devnet measurement
      --only <ONLY>  Run only these groups: signatures, state, data, clearing, assurance, execution, devnet (comma-separated)
      --json <JSON>  Write the measurements as JSON to this file
  -h, --help         Print help
```

See [Benchmarks](../guides/benchmarks.md).

