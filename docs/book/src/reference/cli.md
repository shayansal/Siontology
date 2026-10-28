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
| Network | `devnet`, `node`, `gateway`, `genesis`, `status`, `pulse`, `events`, `forced`, `censor` |
| Wallets and money | `keys new`, `keys show`, `keys validator`, `keys gateway`, `balance`, `faucet`, `send` |
| Knowledge | `bond`, `claim issue`, `claim show`, `claim decide`, `accept`, `challenge`, `rule` |
| Agents | `charter create`, `charter pay`, `charter revoke`, `agent register`, `agent spawn`, `agent show`, `agent card`, `agent card-template`, `agent suspend`, `agent resume`, `agent revoke`, `agent reclaim`, `mcp`, `x402 serve`, `x402 fetch` |
| Work | `task post`, `task attest`, `task submit`, `task settle`, `task cancel`, `task show` |
| Memory | `memory open`, `memory remember`, `memory commit`, `memory recall`, `memory show` |
| Stake | `stake pools`, `stake delegate`, `stake show`, `stake claim`, `stake undelegate`, `stake withdraw`, `stake commission` |
| Markets | `market create`, `market order`, `market clear`, `market show` |
| Payment streams | `stream open`, `stream pay`, `stream jump`, `stream check`, `stream settle`, `stream top-up`, `stream close`, `stream show` |
| Verification | `tx`, `receipt`, `verify` |
| Other | `demo`, `bench` |

There is no command yet for registering a validator on a running chain,
unstaking, withdrawing stake, claiming a validator's rewards, reporting
equivocation, withdrawing a bond, releasing a reservation, rotating keys,
escrow lanes, sealed market orders, or memory revisions, sealing and
forgetting. Those transactions are available through the Rust API
([Transaction kinds](transactions.md)) and the demos.

## `sion`

```text
Siontology: a neo-blockchain devnet and client

Usage: sion [OPTIONS] <COMMAND>

Commands:
  devnet     Run a devnet and serve its API
  genesis    Collect validators' entries into a network's genesis file
  node       Run one validator of a network, in this process
  gateway    Run a gateway of a network, in this process
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
  agent      Agent identities: register agents, launch subagents with their own identities, suspend, resume and revoke whole subtrees
  task       Task Charters: escrow work whose acceptance rule is fixed when it is posted; attest, submit and settle results; cancel
  stake      Pooled stake: delegate to a validator without handing the stake over, collect rewards, leave; set a validator's commission
  memory     Memory logs for agents and robots: open a log, record atoms locally, commit heads the ledger keeps append-only, and recall with proofs of completeness
  storage    Storage deals: have data kept by a provider who proves every round that it still holds it, or provide storage and be paid for it
  market     Markets
  stream     Payment streams: escrow once, pay per request with one hash each, settle thousands of payments in one transaction
  tx         A transaction's status and effects
  receipt    A transaction's receipt, verified as a light client would
  pulse      A Pulse's header, certificate and summary
  events     Recent events
  verify     Verify a Pulse as a light client
  demo       Run a scripted demo: invoice, agents, swarm, tasks, robots, storage, market, receipts, x402, streams, censorship, light, staking or all
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

## `sion genesis`

```text
Collect validators' entries into a network's genesis file

Usage: sion genesis [OPTIONS] --validator <VALIDATORS>

Options:
      --node <NODE>
          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --validator <VALIDATORS>
          A validator's entry (`validator.json`, written by `sion keys validator`); repeat for each
      --gateway <GATEWAYS>
          A gateway's entry (`gateway.json`, written by `sion keys gateway`); repeat for each
      --chain <CHAIN>
          The chain's name (default: the one the entries name)
      --stake <STAKE>
          Each validator's stake, in USD, minted at genesis [default: 100000]
      --accounts <ACCOUNTS>
          TEST NETWORKS ONLY: named accounts funded at genesis, whose keys anyone can derive from their names
      --demo-usd <DEMO_USD>
          USD each named account starts with [default: 100000]
      --faucet
          TEST NETWORKS ONLY: fund the devnet faucet, whose keys anyone can derive from its name
      --shards <SHARDS>
          Lane shards (default: as `sion devnet`, 2)
      --seats <SEATS>
          Seats per lane committee (default: as `sion devnet`, 4)
      --rounds-per-pulse <ROUNDS_PER_PULSE>
          Consensus rounds per Pulse (default: as `sion devnet`, 4)
      --epoch-length <EPOCH_LENGTH>
          Pulses per epoch (default: as `sion devnet`, 0: one epoch that never ends)
      --out <OUT>
          Where to write `genesis.json` and `peers.json` [default: .]
  -h, --help
          Print help
```

Checks every record's dual signature. See [Running a network of processes](../guides/network.md).

## `sion node`

```text
Run one validator of a network, in this process

Usage: sion node [OPTIONS] --dir <DIR> --genesis <GENESIS>

Options:
      --dir <DIR>            The validator's directory (its keys; its log and snapshots go to `data/` inside)
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --genesis <GENESIS>    The network's genesis file
      --peers <PEERS>        The peer file (default: `peers.json` next to the genesis file, if there is one)
      --listen <LISTEN>      Where to listen for QUIC (default: the first address of its record)
      --api <API>            Where to serve the HTTP API [default: 127.0.0.1:8730]
      --gateway              Host a gateway too: accept transactions on the API and get them certified
      --prove                Run the prover (a World Proof for every Pulse)
      --chunk <CHUNK>        Transactions per proof chunk, with --prove [default: 16]
      --no-fsync             Do not fsync log records (faster, loses the last records on a power failure)
      --duration <DURATION>  Stop after this many seconds (0: run until interrupted) [default: 0]
      --report <REPORT>      Print status every this many seconds (0: silent) [default: 5]
  -h, --help                 Print help
```

One validator per process. A validator that stops cannot rejoin yet (`net:no-rejoin`). See [Running a network of processes](../guides/network.md).

## `sion gateway`

```text
Run a gateway of a network, in this process

Usage: sion gateway [OPTIONS] --dir <DIR> --genesis <GENESIS> --follow <FOLLOW>

Options:
      --dir <DIR>            The gateway's directory (its network key)
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --genesis <GENESIS>    The network's genesis file
      --peers <PEERS>        The peer file, which must list this gateway (default: `peers.json` next to the genesis file)
      --follow <FOLLOW>      The API of the node this gateway trusts for the state and the committees
      --api <API>            Where to serve the HTTP API [default: 127.0.0.1:8740]
      --duration <DURATION>  Stop after this many seconds (0: run until interrupted) [default: 0]
      --report <REPORT>      Print status every this many seconds (0: silent) [default: 5]
  -h, --help                 Print help
```

Trusts the node it follows (`net:gateway-trusts-node`). See [Running a network of processes](../guides/network.md).

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
  new        Create a wallet with a random seed
  show       Show a wallet's account and public keys
  validator  Generate a network validator's keys and its signed network record
  gateway    Generate a network gateway's key
  help       Print this message or the help of the given subcommand(s)

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

## `sion keys validator`

```text
Generate a network validator's keys and its signed network record

Usage: sion keys validator [OPTIONS] --dir <DIR> --address <ADDRESSES> --operator <OPERATOR>

Options:
      --dir <DIR>            The validator's directory (created; it must not hold keys yet)
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --chain <CHAIN>        The chain's name [default: sion-localnet]
      --address <ADDRESSES>  Where other nodes reach it, `host:port` (repeat for several)
      --operator <OPERATOR>  The wallet name or account ID that operates it (stakes for it and receives its rewards)
  -h, --help                 Print help
```

Writes secret keys (mode 0600) and the public `validator.json`; refuses to overwrite. See [Running a network of processes](../guides/network.md).

## `sion keys gateway`

```text
Generate a network gateway's key

Usage: sion keys gateway [OPTIONS] --dir <DIR>

Options:
      --dir <DIR>    The gateway's directory (created; it must not hold a key yet)
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --name <NAME>  A name for people [default: gateway]
  -h, --help         Print help
```

See [Running a network of processes](../guides/network.md).

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

## `sion agent`

```text
Agent identities: register agents, launch subagents with their own identities, suspend, resume and revoke whole subtrees

Usage: sion agent [OPTIONS] <COMMAND>

Commands:
  register       Register an agent identity: a charter with a card, funded by the principal
  spawn          Launch subagents, each with its own identity, account, caveats and budget carved from the parent's allotment
  show           Show an agent: account, principal, lineage, card, status, budget
  card           Replace an agent's card with a file's commitment
  card-template  Print an agent card template (A2A-style JSON with a `sion` section)
  suspend        Suspend an agent and everything below it (an overseer: the backer, the principal or an ancestor's agent)
  resume         Lift a suspension (the suspender or the backer)
  revoke         Revoke an agent and everything below it, for good (the agent itself or an overseer)
  reclaim        Return a revoked or expired agent's unspent budget to its backer (its principal or its backer)
  help           Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

See [Agent identities from the command line](../guides/agents.md).

## `sion agent register`

```text
Register an agent identity: a charter with a card, funded by the principal

Usage: sion agent register [OPTIONS] <PRINCIPAL> <AGENT>

Arguments:
  <PRINCIPAL>  The principal's wallet (who answers for the agent)
  <AGENT>      The agent's wallet or account

Options:
      --card <CARD>                The agent card file (its exact bytes are committed)
      --node <NODE>                The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --payees <PAYEES>            Allowed payees (comma-separated; empty: any)
      --max-payment <MAX_PAYMENT>  Largest payment (USD) [default: 100]
      --max-window <MAX_WINDOW>    Largest total per window (USD) [default: 500]
      --window <WINDOW>            Window (Pulses) [default: 1000]
      --funding <FUNDING>          Budget (USD) [default: 1000]
      --bond <BOND>                Bond behind the agent's receipts (USD) [default: 0]
      --lifetime <LIFETIME>        Lifetime (Pulses) [default: 100000]
      --no-spawn                   The agent may not launch subagents
  -h, --help                       Print help
```

## `sion agent spawn`

```text
Launch subagents, each with its own identity, account, caveats and budget carved from the parent's allotment

Usage: sion agent spawn [OPTIONS] <PARENT> <AGENT> <ALLOTMENT>

Arguments:
  <PARENT>     The parent agent's wallet
  <AGENT>      The parent's agent id (its charter)
  <ALLOTMENT>  The parent's allotment

Options:
      --count <COUNT>              How many subagents [default: 1]
      --node <NODE>                The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --budget <BUDGET>            Budget of each (USD) [default: 10]
      --prefix <PREFIX>            Wallet names are `<prefix>-1`, `<prefix>-2`, …; a wallet that does not exist yet is created with a random seed [default: subagent]
      --card <CARD>                The subagents' card file
      --max-payment <MAX_PAYMENT>  Largest payment of each (USD); default: the parent's
      --max-window <MAX_WINDOW>    Largest total per window of each (USD); default: an equal share of the window allowance the parent has not carved out yet
      --may-spawn                  The subagents may launch subagents of their own
  -h, --help                       Print help
```

## `sion agent show`

```text
Show an agent: account, principal, lineage, card, status, budget

Usage: sion agent show [OPTIONS] <AGENT>

Arguments:
  <AGENT>  Agent id

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent card`

```text
Replace an agent's card with a file's commitment

Usage: sion agent card [OPTIONS] <WALLET> <AGENT> <FILE>

Arguments:
  <WALLET>  The agent's wallet
  <AGENT>   Agent id
  <FILE>    The card file

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent card-template`

```text
Print an agent card template (A2A-style JSON with a `sion` section)

Usage: sion agent card-template [OPTIONS] <NAME>

Arguments:
  <NAME>  The agent's name

Options:
      --description <DESCRIPTION>  What it does [default: ""]
      --node <NODE>                The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --url <URL>                  Where it is served [default: ""]
  -h, --help                       Print help
```

## `sion agent suspend`

```text
Suspend an agent and everything below it (an overseer: the backer, the principal or an ancestor's agent)

Usage: sion agent suspend [OPTIONS] <WALLET> <AGENT>

Arguments:
  <WALLET>  The overseer's wallet
  <AGENT>   Agent id

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent resume`

```text
Lift a suspension (the suspender or the backer)

Usage: sion agent resume [OPTIONS] <WALLET> <AGENT>

Arguments:
  <WALLET>  Wallet
  <AGENT>   Agent id

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent revoke`

```text
Revoke an agent and everything below it, for good (the agent itself or an overseer)

Usage: sion agent revoke [OPTIONS] <WALLET> <AGENT>

Arguments:
  <WALLET>  Wallet
  <AGENT>   Agent id

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent reclaim`

```text
Return a revoked or expired agent's unspent budget to its backer (its principal or its backer)

Usage: sion agent reclaim [OPTIONS] <WALLET> <AGENT>

Arguments:
  <WALLET>  Wallet
  <AGENT>   Agent id

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion task`

```text
Task Charters: escrow work whose acceptance rule is fixed when it is posted; attest, submit and settle results; cancel

Usage: sion task [OPTIONS] <COMMAND>

Commands:
  post    Post a task: escrow money for work whose acceptance rule is fixed now. Without --worker, anyone may settle it (a bounty)
  attest  Attest a task's output as a verifier, on your bond `bond:<you>:verifier`
  submit  Submit a result claim for a task, as its worker, resting on your task bond, the attestations given, and the task's required claims
  settle  Settle a task with your result claim and its attestations
  cancel  Cancel a task: a worker releases it, or the poster withdraws an open or expired one. The escrow is refunded
  show    Show a task: its terms, escrow and state
  help    Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

See [Task Charters from the command line](../guides/tasks.md).

## `sion task post`

```text
Post a task: escrow money for work whose acceptance rule is fixed now. Without --worker, anyone may settle it (a bounty)

Usage: sion task post [OPTIONS] <POSTER> <AMOUNT>

Arguments:
  <POSTER>  The poster's wallet (with --agent, the agent's wallet)
  <AMOUNT>  Escrow (USD), paid in full for an accepted result

Options:
      --node <NODE>                The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --worker <WORKER>            The worker (wallet or account); omit for an open bounty
      --spec <SPEC>                A file describing the task (its exact bytes are committed)
      --requires <REQUIRES>        Claims every result must rest on (comma-separated ids)
      --verifiers <VERIFIERS>      Verifiers whose attestations count (comma-separated wallets or accounts)
      --adjudicator <ADJUDICATOR>  The adjudicator atom results must name (hex)
      --detection <DETECTION>      The largest detection factor results may declare, in permille [default: 900]
      --reliance <RELIANCE>        Pulses of reliance on a paid result (its backing stays reserved) [default: 1000]
      --kappa <KAPPA>              Safety factor κ of the lab's policy [default: 2]
      --deadline <DEADLINE>        Pulses until the deadline [default: 1000]
      --agent <AGENT>              Post from this agent's budget (its id); needs --allotment
      --allotment <ALLOTMENT>      The agent's allotment
  -h, --help                       Print help
```

## `sion task attest`

```text
Attest a task's output as a verifier, on your bond `bond:<you>:verifier`

Usage: sion task attest [OPTIONS] <VERIFIER> <TASK> <OUTPUT>

Arguments:
  <VERIFIER>  The verifier's wallet
  <TASK>      The task
  <OUTPUT>    The output file (its exact bytes are committed)

Options:
      --bond <BOND>  The bond label the attestation rests on [default: verifier]
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion task submit`

```text
Submit a result claim for a task, as its worker, resting on your task bond, the attestations given, and the task's required claims

Usage: sion task submit [OPTIONS] <WORKER> <TASK> <OUTPUT>

Arguments:
  <WORKER>  The worker's wallet
  <TASK>    The task
  <OUTPUT>  The output file (its exact bytes are committed)

Options:
      --attestations <ATTESTATIONS>  Attestation claim ids (comma-separated), in order
      --node <NODE>                  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help                         Print help
```

## `sion task settle`

```text
Settle a task with your result claim and its attestations

Usage: sion task settle [OPTIONS] <WORKER> <TASK> <RESULT>

Arguments:
  <WORKER>  The worker's wallet
  <TASK>    The task
  <RESULT>  The result claim id

Options:
      --attestations <ATTESTATIONS>  Attestation claim ids (comma-separated), in the order the result lists them
      --node <NODE>                  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help                         Print help
```

## `sion task cancel`

```text
Cancel a task: a worker releases it, or the poster withdraws an open or expired one. The escrow is refunded

Usage: sion task cancel [OPTIONS] <WALLET> <TASK>

Arguments:
  <WALLET>  Wallet
  <TASK>    The task

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion task show`

```text
Show a task: its terms, escrow and state

Usage: sion task show [OPTIONS] <TASK>

Arguments:
  <TASK>  The task

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion stake`

```text
Pooled stake: delegate to a validator without handing the stake over, collect rewards, leave; set a validator's commission

Usage: sion stake [OPTIONS] <COMMAND>

Commands:
  pools       Every validator: its own stake, its pool, its commission and its weight in the active set
  delegate    Delegate stake to a validator's pool. You keep the delegation: the stake counts towards the validator's weight and earns from the end of this epoch, and you can leave whenever you like
  show        An account's delegations: shares, the stake behind them, their state and the rewards they are owed
  claim       Collect a delegation's rewards
  undelegate  Stop a delegation earning: collect its rewards and start unbonding. The stake stays slashable until you withdraw it
  withdraw    Withdraw an unbonded delegation's stake
  commission  Set your validator's commission on its pool's rewards, in percent (at most 25; once anyone has delegated, a raise of at most 2 points at a time). It takes effect at the epoch's end
  help        Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

See [Delegating stake](../guides/delegation.md).

## `sion stake pools`

```text
Every validator: its own stake, its pool, its commission and its weight in the active set

Usage: sion stake pools [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion stake delegate`

```text
Delegate stake to a validator's pool. You keep the delegation: the stake counts towards the validator's weight and earns from the end of this epoch, and you can leave whenever you like

Usage: sion stake delegate [OPTIONS] <FROM> <VALIDATOR> <AMOUNT>

Arguments:
  <FROM>       Your wallet
  <VALIDATOR>  The validator (its id, or a unique prefix of it)
  <AMOUNT>     Stake (decimal, in the stake asset), at least 1

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion stake show`

```text
An account's delegations: shares, the stake behind them, their state and the rewards they are owed

Usage: sion stake show [OPTIONS] <ACCOUNT>

Arguments:
  <ACCOUNT>  Wallet name or account ID

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion stake claim`

```text
Collect a delegation's rewards

Usage: sion stake claim [OPTIONS] <FROM> <DELEGATION>

Arguments:
  <FROM>        Your wallet
  <DELEGATION>  The delegation (its id, or a unique prefix of it)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion stake undelegate`

```text
Stop a delegation earning: collect its rewards and start unbonding. The stake stays slashable until you withdraw it

Usage: sion stake undelegate [OPTIONS] <FROM> <DELEGATION>

Arguments:
  <FROM>        Your wallet
  <DELEGATION>  The delegation (its id, or a unique prefix of it)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion stake withdraw`

```text
Withdraw an unbonded delegation's stake

Usage: sion stake withdraw [OPTIONS] <FROM> <DELEGATION>

Arguments:
  <FROM>        Your wallet
  <DELEGATION>  The delegation (its id, or a unique prefix of it)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion stake commission`

```text
Set your validator's commission on its pool's rewards, in percent (at most 25; once anyone has delegated, a raise of at most 2 points at a time). It takes effect at the epoch's end

Usage: sion stake commission [OPTIONS] <FROM> <VALIDATOR> <PERCENT>

Arguments:
  <FROM>       The operator's wallet
  <VALIDATOR>  The validator (its id, or a unique prefix of it)
  <PERCENT>    The commission in percent, with at most one decimal (for example 10 or 12.5)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion memory`

```text
Memory logs for agents and robots: open a log, record atoms locally, commit heads the ledger keeps append-only, and recall with proofs of completeness

Usage: sion memory [OPTIONS] <COMMAND>

Commands:
  open      Open a memory log on the ledger, owned by the wallet (for an agent identity with --agent, whose agent must be the wallet), and a local store for its atoms
  remember  Record an atom in a log's local store. It reaches the ledger's commitment with the next `sion memory commit`
  commit    Commit the local log's head to the ledger, with a proof that it extends the head the ledger holds
  recall    Everything the log had recorded in a box during an interval, as of its committed head, with a proof checked against the index root the ledger holds
  show      A log's head on the ledger and its local store
  help      Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

See [Memory from the command line](../guides/memory.md).

## `sion memory open`

```text
Open a memory log on the ledger, owned by the wallet (for an agent identity with --agent, whose agent must be the wallet), and a local store for its atoms

Usage: sion memory open [OPTIONS] <WALLET>

Arguments:
  <WALLET>  The writer's wallet

Options:
      --agent <AGENT>  The agent identity the log belongs to (its id)
      --node <NODE>    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help           Print help
```

## `sion memory remember`

```text
Record an atom in a log's local store. It reaches the ledger's commitment with the next `sion memory commit`

Usage: sion memory remember [OPTIONS] <WALLET> <LOG> <SUBJECT> <PREDICATE> <VALUE>

Arguments:
  <WALLET>     The writer's wallet
  <LOG>        The log (its id, or a unique prefix of it)
  <SUBJECT>    What the statement is about, for example pallet-17
  <PREDICATE>  The relation, for example at
  <VALUE>      The value: true or false, an integer, @name for an entity, or text

Options:
      --kind <KIND>              observation, coverage, semantic, episodic, procedural, … [default: observation]
      --node <NODE>              The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --frame <FRAME>            The frame its place is in, for example warehouse-3
      --at <AT>                  Its place in the frame, in millimetres: x,y,z
      --size <SIZE>              Half the size of its box around `at`, in millimetres: dx,dy,dz
      --from <FROM>              When it began to hold, in seconds since the Unix epoch (default: now)
      --seconds <SECONDS>        For how many seconds it held (default: open-ended)
      --confidence <CONFIDENCE>  Confidence, in thousandths [default: 1000]
  -h, --help                     Print help
```

## `sion memory commit`

```text
Commit the local log's head to the ledger, with a proof that it extends the head the ledger holds

Usage: sion memory commit [OPTIONS] <WALLET> <LOG>

Arguments:
  <WALLET>  The writer's wallet
  <LOG>     The log (its id, or a unique prefix of it)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --seal         Close the log for good
  -h, --help         Print help
```

## `sion memory recall`

```text
Everything the log had recorded in a box during an interval, as of its committed head, with a proof checked against the index root the ledger holds

Usage: sion memory recall [OPTIONS] --min <MIN> --max <MAX> --from <FROM> --until <UNTIL> <LOG> <FRAME>

Arguments:
  <LOG>    The log (its id, or a unique prefix of it)
  <FRAME>  The frame, for example warehouse-3

Options:
      --min <MIN>      The box's lower corner, in millimetres: x,y,z
      --node <NODE>    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --max <MAX>      The box's upper corner, in millimetres: x,y,z
      --from <FROM>    The interval's start, in seconds since the Unix epoch
      --until <UNTIL>  The interval's end, in seconds since the Unix epoch (exclusive)
  -h, --help           Print help
```

## `sion memory show`

```text
A log's head on the ledger and its local store

Usage: sion memory show [OPTIONS] <LOG>

Arguments:
  <LOG>  The log (its id, or a unique prefix of it)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion storage`

```text
Storage deals: have data kept by a provider who proves every round that it still holds it, or provide storage and be paid for it

Usage: sion storage [OPTIONS] <COMMAND>

Commands:
  prepare  Split a file into leaves and print the root a deal would commit to
  post     Post a deal for a file: escrow PRICE × ROUNDS to have it kept and proven every round
  deals    List storage deals
  show     One deal: its terms, its state and its provider's progress
  accept   Accept a deal as its provider: fetch its data (from --file or its locator), check it hashes to the deal's root, keep a copy and post the collateral
  prove    Prove the round under way for every deal you hold
  settle   Collect for the rounds you proved
  close    Close a deal: refund an open one (client), or end an accepted one at its end (client or provider) or once its provider abandoned it (client)
  serve    Run a provider: accept deals whose data you can fetch, prove every round, collect and close, every second until stopped
  help     Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

See [Storage from the command line](../guides/storage.md).

## `sion storage prepare`

```text
Split a file into leaves and print the root a deal would commit to

Usage: sion storage prepare [OPTIONS] <FILE>

Arguments:
  <FILE>  The file

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion storage post`

```text
Post a deal for a file: escrow PRICE × ROUNDS to have it kept and proven every round

Usage: sion storage post [OPTIONS] --price <PRICE> --rounds <ROUNDS> <FROM> <FILE>

Arguments:
  <FROM>  Your wallet (the client)
  <FILE>  The file to have kept

Options:
      --node <NODE>                    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --price <PRICE>                  Paid per proven round (decimal USD)
      --rounds <ROUNDS>                Rounds
      --round-length <ROUND_LENGTH>    Pulses per round: a multiple of 8, from 8 to 100,000 [default: 16]
      --collateral <COLLATERAL>        Collateral the provider must post (decimal USD). Default: the escrow, for a deal open to anyone (which must ask for some); none for a named provider
      --challenges <CHALLENGES>        Leaves challenged per round (1 to 16) [default: 4]
      --provider <PROVIDER>            The provider (wallet name or account); without it, anyone may accept
      --accept-within <ACCEPT_WITHIN>  How many Pulses the deal stays open for acceptance [default: 200]
      --locator <LOCATOR>              Where providers fetch the data: an https:// URL serving the file
      --local                          Point providers at the file itself (file://<its path>): only a provider on this machine can fetch it, and the deal publishes the path
  -h, --help                           Print help
```

## `sion storage deals`

```text
List storage deals

Usage: sion storage deals [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --open         Only deals still open for acceptance
  -h, --help         Print help
```

## `sion storage show`

```text
One deal: its terms, its state and its provider's progress

Usage: sion storage show [OPTIONS] <DEAL>

Arguments:
  <DEAL>  The deal (its id, or a unique prefix of it)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion storage accept`

```text
Accept a deal as its provider: fetch its data (from --file or its locator), check it hashes to the deal's root, keep a copy and post the collateral

Usage: sion storage accept [OPTIONS] <FROM> <DEAL>

Arguments:
  <FROM>  Your wallet (the provider)
  <DEAL>  The deal (its id, or a unique prefix of it)

Options:
      --file <FILE>  Take the data from this file instead of the locator
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion storage prove`

```text
Prove the round under way for every deal you hold

Usage: sion storage prove [OPTIONS] <FROM>

Arguments:
  <FROM>  Your wallet (the provider)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion storage settle`

```text
Collect for the rounds you proved

Usage: sion storage settle [OPTIONS] <FROM> <DEAL>

Arguments:
  <FROM>  Your wallet (the provider)
  <DEAL>  The deal (its id, or a unique prefix of it)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion storage close`

```text
Close a deal: refund an open one (client), or end an accepted one at its end (client or provider) or once its provider abandoned it (client)

Usage: sion storage close [OPTIONS] <FROM> <DEAL>

Arguments:
  <FROM>  Your wallet
  <DEAL>  The deal (its id, or a unique prefix of it)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion storage serve`

```text
Run a provider: accept deals whose data you can fetch, prove every round, collect and close, every second until stopped

Usage: sion storage serve [OPTIONS] <FROM>

Arguments:
  <FROM>  Your wallet (the provider)

Options:
      --min-price <MIN_PRICE>
          Accept only deals paying at least this per round (decimal USD), and at least twice what proving a round costs in fees [default: 0.01]
      --node <NODE>
          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --max-collateral <MAX_COLLATERAL>
          Accept only deals asking at most this collateral (decimal USD) [default: 10]
      --max-total-collateral <MAX_TOTAL_COLLATERAL>
          The most collateral at stake across all deals (decimal USD) [default: 100]
      --max-mib <MAX_MIB>
          Accept only data of at most this many MiB [default: 64]
      --max-total-mib <MAX_TOTAL_MIB>
          Keep at most this many MiB in all [default: 1024]
      --max-duration <MAX_DURATION>
          Accept only deals lasting at most this many Pulses [default: 100000]
      --allow-local
          Fetch file:// locators (a provider on the clients' machine)
      --allow-private
          Fetch from local and private network addresses
      --pulses <PULSES>
          Stop after this many Pulses
  -h, --help
          Print help
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

## `sion stream`

```text
Payment streams: escrow once, pay per request with one hash each, settle thousands of payments in one transaction

Usage: sion stream [OPTIONS] <COMMAND>

Commands:
  open    Open a stream: escrow funds for one payee and keep the chain that pays it (its secret seed goes under $SION_HOME/streams)
  pay     Pay from a stream: prints the next payment, `<stream>:<position>:<element>`, to give to the payee, and the jump for its checkpoint when it is the first payment past one
  jump    Print the jump for the checkpoint below a position (as the payer)
  check   Check a payment against the stream's settled head (anyone can)
  settle  Settle as the payee: everything up to the latest payment received
  top-up  Add escrow and/or time, as the payer
  close   Close a stream: the payee at any time, the payer after its last Pulse. What is left in escrow goes back to the payer
  show    Show a stream
  help    Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

See [Streams from the command line](../guides/streams.md).

## `sion stream open`

```text
Open a stream: escrow funds for one payee and keep the chain that pays it (its secret seed goes under $SION_HOME/streams)

Usage: sion stream open [OPTIONS] <PAYER> <PAYEE>

Arguments:
  <PAYER>  The paying wallet
  <PAYEE>  Who is paid (wallet name or account ID)

Options:
      --node <NODE>        The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --unit <UNIT>        The value of one step (USD) [default: 0.001]
      --steps <STEPS>      Steps in the chain: the most the stream can ever pay is unit × steps [default: 100000]
      --deposit <DEPOSIT>  Escrow (USD) [default: 10]
      --pulses <PULSES>    Pulses the payee has to settle (after them the payer may close the stream and take back what is left) [default: 10000]
      --spacing <SPACING>  Steps between checkpoints, which let the payee settle any number of steps for a bounded number of hashes (default: every 64 steps, fewer for very long chains; 0: no checkpoints)
  -h, --help               Print help
```

## `sion stream pay`

```text
Pay from a stream: prints the next payment, `<stream>:<position>:<element>`, to give to the payee, and the jump for its checkpoint when it is the first payment past one

Usage: sion stream pay [OPTIONS] <STREAM>

Arguments:
  <STREAM>  The stream

Options:
      --node <NODE>    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --steps <STEPS>  Steps to pay [default: 1]
  -h, --help           Print help
```

## `sion stream jump`

```text
Print the jump for the checkpoint below a position (as the payer)

Usage: sion stream jump [OPTIONS] <STREAM>

Arguments:
  <STREAM>  The stream

Options:
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --position <POSITION>  The position (default: the last one revealed)
  -h, --help                 Print help
```

## `sion stream check`

```text
Check a payment against the stream's settled head (anyone can)

Usage: sion stream check [OPTIONS] <PAYMENT>

Arguments:
  <PAYMENT>  The payment

Options:
      --jump <JUMP>  Check it by this jump instead of walking from the head
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion stream settle`

```text
Settle as the payee: everything up to the latest payment received

Usage: sion stream settle [OPTIONS] <PAYEE> <PAYMENT>

Arguments:
  <PAYEE>    The payee's wallet
  <PAYMENT>  The latest payment received

Options:
      --jump <JUMP>  Prove it by this jump (from the payer) instead of walking from the head
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion stream top-up`

```text
Add escrow and/or time, as the payer

Usage: sion stream top-up [OPTIONS] <PAYER> <STREAM>

Arguments:
  <PAYER>   The payer's wallet
  <STREAM>  The stream

Options:
      --deposit <DEPOSIT>  Escrow to add (USD) [default: 0]
      --node <NODE>        The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --pulses <PULSES>    Pulses to add to the stream's last [default: 0]
  -h, --help               Print help
```

## `sion stream close`

```text
Close a stream: the payee at any time, the payer after its last Pulse. What is left in escrow goes back to the payer

Usage: sion stream close [OPTIONS] <WHO> <STREAM>

Arguments:
  <WHO>     Who closes
  <STREAM>  The stream

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion stream show`

```text
Show a stream

Usage: sion stream show [OPTIONS] <STREAM>

Arguments:
  <STREAM>  The stream

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
Run a scripted demo: invoice, agents, swarm, tasks, robots, storage, market, receipts, x402, streams, censorship, light, staking or all

Usage: sion demo [OPTIONS] [NAME]

Arguments:
  [NAME]  Which demo [default: all]

Options:
      --local        Start an in-process devnet instead of using --node
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

See [Run the demos](../getting-started/demos.md).

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
      --listen <LISTEN>              Listen address [default: 127.0.0.1:8402]
      --node <NODE>                  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --path <PATH>                  The resource's path [default: /v1/resource]
      --price <PRICE>                Price per request (USD) [default: 0.25]
      --body <BODY>                  What every paid request returns [default: {"paid":true}]
      --chain <CHAIN>                The chain identity receipts must verify against. Without it the node vouches for itself [env: SION_CHAIN=]
      --streams                      Also accept payments from streams (scheme `stream`), settled by PAY_TO's wallet, which must be a wallet name
      --settle-every <SETTLE_EVERY>  With --streams: settle once a stream has this many unsettled steps [default: 1000]
      --settle-after <SETTLE_AFTER>  With --streams: settle a stream's payments at the latest this many seconds after the first unsettled one [default: 30]
  -h, --help                         Print help
```

See [x402](../guides/x402.md).

## `sion x402 fetch`

```text
Fetch a URL, paying through x402 if the server asks

Usage: sion x402 fetch [OPTIONS] <URL>

Arguments:
  <URL>  The URL

Options:
      --node <NODE>            The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --wallet <WALLET>        The paying wallet (the agent's, when paying under a charter; not needed with --stream)
      --charter <CHARTER>      Pay under this Agent Charter (ID) instead of by transfer
      --allotment <ALLOTMENT>  The agent's allotment (ID), with --charter
      --stream <STREAM>        Pay from this payment stream (ID) instead: one chain element per request, no transaction
      --count <COUNT>          Fetch this many times (with --stream, every request after the first is paid up front) [default: 1]
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
      --only <ONLY>  Run only these groups: signatures, state, data, clearing, assurance, execution, streams, devnet (comma-separated)
      --json <JSON>  Write the measurements as JSON to this file
  -h, --help         Print help
```

See [Benchmarks](../guides/benchmarks.md).
