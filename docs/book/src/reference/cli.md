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
| Agents | `charter create`, `charter pay`, `charter revoke`, `agent register`, `agent spawn`, `agent show`, `agent card`, `agent card-template`, `agent suspend`, `agent resume`, `agent revoke`, `agent reclaim`, `agent run`, `agent continue`, `agent sessions`, `agent trace`, `agent kill-switch`, `agent profile add`, `agent profile list`, `agent profile remove`, `agent schedule add`, `agent schedule list`, `agent schedule remove`, `agent scheduler`, `agent serve`, `agent telegram`, `mcp`, `x402 serve`, `x402 fetch` |
| Skills | `skill pack`, `skill audit`, `skill publish`, `skill show`, `skill install`, `skill eval`, `skill evolve`, `skill vouch`, `skill bounty`, `skill verify`, `skill submit`, `skill lineage` |
| Work | `task post`, `task attest`, `task submit`, `task settle`, `task cancel`, `task show` |
| Memory | `memory open`, `memory remember`, `memory commit`, `memory recall`, `memory show` |
| Storage | `storage prepare`, `storage post`, `storage deals`, `storage show`, `storage accept`, `storage prove`, `storage settle`, `storage close`, `storage serve` |
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
  doctor     What this machine has for Siontology and its agents, and what is missing: the home, the node, sandboxes, the browser and desktop, models', speech services' and messengers' credentials (by name)
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
  skill      Skills with a supply chain: pack a skill, audit a bundle, publish a release on bonds and audits, show one, install one for an agent under the decision rule; evaluate a skill, evolve it, and have a better version verified by others through a bounty
  storage    Storage deals: have data kept by a provider who proves every round that it still holds it, or provide storage and be paid for it
  market     Markets
  stream     Payment streams: escrow once, pay per request with one hash each, settle thousands of payments in one transaction
  tx         A transaction's status and effects
  receipt    A transaction's receipt, verified as a light client would
  pulse      A Pulse's header, certificate and summary
  events     Recent events
  verify     Verify a Pulse as a light client
  demo       Run a scripted demo: invoice, agents, swarm, batch, tasks, robots, storage, market, receipts, x402, streams, censorship, light, staking or all
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


## `sion doctor`

```text
What this machine has for Siontology and its agents, and what is missing: the home, the node, sandboxes, the browser and desktop, models', speech services' and messengers' credentials (by name)

Usage: sion doctor [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

It reads and changes nothing; variables are named, never shown. See
[What is here](../getting-started/install.md#what-is-here).

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
      --burst <BURST>              The most the agent may spend at once: its bucket's capacity, full when it starts (USD) [default: 500]
      --rate <RATE>                What refills its bucket each Pulse (USD) [default: 0.5]
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
  pay-batch      Pay from several agents in one batch (spec v1.13): one payment from each agent's own budget, all authorized by one signature of `signer`, the agents' own wallet or, for keyless agents, the wallet of their nearest keyed ancestor
  show           Show an agent: account, principal, lineage, card, status, budget
  status         An agent's standing, checked as a light client: its charter and the overlays of its lineage proven against a certified header, its status worked out from them, its budget proven too
  card           Replace an agent's card with a file's commitment
  card-template  Print an agent card template (A2A-style JSON with a `sion` section)
  suspend        Suspend an agent and everything below it (an overseer in good standing: the backer, or the agent of a charter above it that is neither revoked nor suspended). The highest suspender is recorded
  resume         Lift a suspension (the recorded suspender, in good standing, or the backer)
  revoke         Revoke an agent and everything below it, for good (the agent itself or an overseer in good standing)
  reclaim        Return a revoked or expired agent's unspent budget to its backer (its principal or its backer)
  narrow         Add limits to an agent and everything below it (spec v1.14): its payees, its largest payment, its last Pulse, whether it may spawn. What is not given stays as it is. Only the backer may widen or lift a narrowing
  cohort         Commit to up to 1,048,576 keyless subagents in one transaction (spec v1.14): a cohort, the root of a sum tree over their buckets. A member costs nothing on the ledger until `agent materialize` creates it, with its budget from the parent's allotment then. The cohort is kept in $SION_HOME/agents/cohorts, to prove its members later
  materialize    Materialize members of a cohort: their charters and allotments, at their lineage ids, with proofs of their leaves. The cohort parent's wallet signs, and signs for them afterwards
  run            Put an agent to work on a task: a model drives it, with the ledger (balance, payments under its charter, transactions), optionally files in a workspace and the web, as tools. The session is kept, so one waiting for your approval can resume
  continue       Continue a session: approve or decline the call it waits on, or tell it more
  evolve         Evolve a profile's instructions (what follows the fixed system prompt) against an evaluation set: a mutator proposes variants for the cases its agent fails, each is scored under gates (size, new, no regression), and with --apply the best becomes the profile's
  sessions       The agent sessions on this machine; `search` finds an agent's past sessions by words
  trace          A session as one JSON line in chat format with tool calls, with its outcome: for review, evaluation or training
  kill-switch    The kill switch: `on` stops every agent run on this machine before its next call; `off` lets them run again
  profile        Agent profiles: how to run an agent, under a name, for schedules and channels
  secret         The secrets skills' scripts may receive (`sion-secrets` in a skill's metadata): kept encrypted on this machine, passed to a script as environment variables, and cut out of what it prints
  login          Your logins agents' browsers may fill (`--login`): each bound to an origin, kept encrypted on this machine, filled only into fields of that origin, and never shown to the agent
  pairing        People a channel did not know who asked to talk to a profile: the codes waiting, approving one, letting a chat out again
  chatgpt        Sign in with ChatGPT, so that agents may run on your plan (`--model chatgpt:<model>`): OpenAI's sign-in for open-source apps, in your browser; the tokens kept encrypted on this machine
  weixin         Sign a WeChat account in for `--via weixin`, by a QR code scanned with WeChat (Tencent's iLink bot protocol), or sign it out
  service        Long-running commands (a bridge, the server, the scheduler) as services of your own: systemd user units on Linux, launchd agents on macOS
  mcp            MCP servers whose tools agents may use: add one (its tools are reviewed and pinned), list them, review one again, remove one
  schedule       Scheduled jobs: a task a profile works on at a time, every interval or on a cron expression, each run a session whose answer goes to a channel
  scheduler      Run scheduled jobs as they fall due
  serve          Serve the HTTP channel: POST /v1/profiles/<name>/messages with {"text": …, "conversation": …}, GET /v1/sessions/<id>, and OpenAI's POST /v1/chat/completions (a profile as the model) and GET /v1/models, with a bearer token: the machine's, or a profile's own (`sion agent profile token add`)
  talk           Talk with a profile in the terminal: what you type goes to it, its reply comes as the model writes it; with --voice, heard and spoken
  telegram       Answer Telegram chats with a profile, by long polling (TELEGRAM_BOT_TOKEN; TELEGRAM_API_BASE for another server)
  bridge         Answer a messenger's chats with a profile: slack (SLACK_BOT_TOKEN, SLACK_APP_TOKEN for Socket Mode), discord (DISCORD_BOT_TOKEN), matrix (MATRIX_HOMESERVER, MATRIX_ACCESS_TOKEN), signal (SIGNAL_API_BASE, SIGNAL_NUMBER, for signal-cli-rest-api), telegram, mattermost (MATTERMOST_URL, MATTERMOST_TOKEN), irc (IRC_SERVER, IRC_NICK, IRC_PASSWORD), twitch (TWITCH_NICK, TWITCH_TOKEN), email (EMAIL_ADDRESS, EMAIL_IMAP_URL, EMAIL_SMTP_URL, EMAIL_PASSWORD or EMAIL_TOKEN_COMMAND), imessage (through imsg on the Mac that runs Messages, IMSG_COMMAND), dingtalk (DINGTALK_CLIENT_ID, DINGTALK_CLIENT_SECRET), or through webhooks to --listen: whatsapp (WHATSAPP_TOKEN, WHATSAPP_PHONE_NUMBER_ID, WHATSAPP_APP_SECRET, WHATSAPP_VERIFY_TOKEN), line (LINE_CHANNEL_ACCESS_TOKEN, LINE_CHANNEL_SECRET), sms through Twilio (TWILIO_ACCOUNT_SID, TWILIO_AUTH_TOKEN, TWILIO_NUMBER, TWILIO_WEBHOOK_URL), teams (TEAMS_APP_ID, TEAMS_APP_PASSWORD, TEAMS_TENANT_ID for a single-tenant bot), google-chat (GOOGLE_CHAT_AUDIENCE, and GOOGLE_CHAT_CREDENTIALS to answer through the Chat API), feishu or lark (FEISHU_APP_ID, FEISHU_APP_SECRET, FEISHU_VERIFICATION_TOKEN, FEISHU_ENCRYPT_KEY), nextcloud-talk (NEXTCLOUD_URL, NEXTCLOUD_TALK_SECRET), wecom (WECOM_CORP_ID, WECOM_AGENT_ID, WECOM_SECRET, WECOM_TOKEN, WECOM_AES_KEY), qq (QQ_APP_ID, QQ_APP_SECRET), synology-chat (SYNOLOGY_CHAT_INCOMING_URL, SYNOLOGY_CHAT_TOKEN) or phone calls through Twilio Voice (TWILIO_AUTH_TOKEN, TWILIO_VOICE_URL); or weixin (after `sion agent weixin login`), zalo (ZALO_BOT_TOKEN), ntfy (NTFY_URL, NTFY_TOKEN, its topics as --allow), x, its direct messages (X_ACCESS_TOKEN), simplex, through SimpleX's terminal client (SIMPLEX_URL), or nostr, direct messages on relays (NOSTR_PRIVATE_KEY, NOSTR_RELAYS)
  gateway        Run a gateway: people write to profiles served from other machines, through POST /v1/profiles/<name>/messages with a profile's token (`sion agent profile token add`); hosts connect out to it and prove they hold each profile's agent key, and messages reach an agent only while the ledger has it in good standing
  host           Serve profiles through a gateway: connect out to it, prove the agents' keys, and answer the messages it passes on
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
      --burst <BURST>              The most it may spend at once: its bucket's capacity, full when it starts (USD) [default: 500]
      --rate <RATE>                What refills its bucket each Pulse (USD) [default: 0.5]
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
      --burst <BURST>              The bucket capacity of each (USD), taken from what the parent's bucket holds; default: its budget, or an equal share of what the parent's bucket holds if that is less
      --rate <RATE>                What refills each one's bucket each Pulse (USD); default: the share of the parent's refill that its budget is of the parent's remaining budget
      --may-spawn                  The subagents may launch subagents of their own
      --keyless                    Keyless subagents (spec v1.13): no keys are made for them. Each account is derived from the subagent's id, and the wallet that signs for the parent signs for them, in batches. Their names become `.keyless` wallet files that hold no secret
      --gas <GAS>                  Give each subagent this much USD from the parent's account for its transaction fees (all in one transaction)
  -h, --help                       Print help
```

With `--keyless`, no keys are made: each subagent's account is derived from its id, the wallet that signs for the parent signs for it, and its name becomes a `.keyless` file under `$SION_HOME/wallets` that holds no secret. Every command that takes a wallet takes that name. See [Agent identities from the command line](../guides/agents.md#5-keyless-subagents).

## `sion agent pay-batch`

```text
Pay from several agents in one batch (spec v1.13): one payment from each agent's own budget, all authorized by one signature of `signer`, the agents' own wallet or, for keyless agents, the wallet of their nearest keyed ancestor

Usage: sion agent pay-batch [OPTIONS] --agents <AGENTS> <SIGNER> <PAYEE> <AMOUNT>

Arguments:
  <SIGNER>  The wallet that signs the batch
  <PAYEE>   Payee (wallet or account)
  <AMOUNT>  USD each agent pays

Options:
      --agents <AGENTS>    The agents (their ids, comma-separated), at most 256
      --node <NODE>        The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --purpose <PURPOSE>  What the payments are for [default: ""]
      --print              Print the batch as `POST /v1/batches` takes it (Borsh, in hex) instead of sending it
  -h, --help               Print help
```

Each agent pays from the first of its allotments that holds the amount. Each transaction's sender is the keyless agent's own account, or the signer for a keyed agent, and pays its fee in USD: the floor for a member of a batch of that size (see [Batched authorization](../concepts/batched-authorization.md#the-fee)). With `--print`, nothing is sent: the output is the hex that the body of [`POST /v1/batches`](http-api.md#post-v1batches) carries.

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

Reads the agent's charter and the overlays of its lineage; the node's `GET /v1/agents/{id}` returns the same with state proofs ([HTTP API](http-api.md#get-v1agentsid)).

## `sion agent status`

```text
An agent's standing, checked as a light client: its charter and the overlays of its lineage proven against a certified header, its status worked out from them, its budget proven too

Usage: sion agent status [OPTIONS] <AGENT>

Arguments:
  <AGENT>  Agent id

Options:
      --chain <CHAIN>          The chain identity the proofs must verify against. Without it the node vouches for itself [env: SION_CHAIN=]
      --node <NODE>            The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --min-pulse <MIN_PULSE>  A certified Pulse you know of from elsewhere: a proof from more than 16 Pulses before it is refused as stale
  -h, --help                   Print help
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
Suspend an agent and everything below it (an overseer in good standing: the backer, or the agent of a charter above it that is neither revoked nor suspended). The highest suspender is recorded

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
Lift a suspension (the recorded suspender, in good standing, or the backer)

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
Revoke an agent and everything below it, for good (the agent itself or an overseer in good standing)

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

## `sion agent narrow`

```text
Add limits to an agent and everything below it (spec v1.14): its payees, its largest payment, its last Pulse, whether it may spawn. What is not given stays as it is. Only the backer may widen or lift a narrowing

Usage: sion agent narrow [OPTIONS] <WALLET> <AGENT>

Arguments:
  <WALLET>  The wallet (the agent, an overseer or the backer)
  <AGENT>   Agent id

Options:
      --node <NODE>                The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --payees <PAYEES>            Allowed payees (comma-separated)
      --max-payment <MAX_PAYMENT>  Largest payment (USD)
      --expires-in <EXPIRES_IN>    The subtree may pay for this many more Pulses
      --no-spawn                   The subtree may no longer launch subagents
      --lift                       Lift the narrowing (the backer only)
  -h, --help                       Print help
```

The agent itself or any overseer in good standing may narrow; only the backer may widen or lift. A narrowing applies to the agent and everything below it, and a payment checks every narrowing above it. See [Agent identities from the command line](../guides/agents.md#6-narrow-a-subtree).

## `sion agent cohort`

```text
Commit to up to 1,048,576 keyless subagents in one transaction (spec v1.14): a cohort, the root of a sum tree over their buckets. A member costs nothing on the ledger until `agent materialize` creates it, with its budget from the parent's allotment then. The cohort is kept in $SION_HOME/agents/cohorts, to prove its members later

Usage: sion agent cohort [OPTIONS] --count <COUNT> <PARENT> <AGENT> <ALLOTMENT>

Arguments:
  <PARENT>     The parent agent's wallet
  <AGENT>      The parent's agent id
  <ALLOTMENT>  The parent's allotment

Options:
      --count <COUNT>              How many members
      --node <NODE>                The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --budget <BUDGET>            Budget of each, paid when it is materialized (USD) [default: 1]
      --burst <BURST>              Bucket capacity of each (USD) [default: 1]
      --rate <RATE>                What refills each one's bucket each Pulse (USD) [default: 0]
      --full                       The members' buckets start full, their tokens taken from the parent's now; otherwise they start empty and fill at their rate from the cohort's creation
      --card <CARD>                The members' card file
      --max-payment <MAX_PAYMENT>  Largest payment of each (USD); default: the parent's
      --prefix <PREFIX>            Wallet names of materialized members are `<prefix>-<index>` [default: member]
  -h, --help                       Print help
```

Members are keyless. The plan (their budget, bucket, card, prefix and signer) is kept in `$SION_HOME/agents/cohorts/<cohort>.json`, from which `sion agent materialize` rebuilds the sum tree to prove each member. See [Agent identities from the command line](../guides/agents.md#7-a-hundred-thousand-subagents-in-one-transaction).

## `sion agent materialize`

```text
Materialize members of a cohort: their charters and allotments, at their lineage ids, with proofs of their leaves. The cohort parent's wallet signs, and signs for them afterwards

Usage: sion agent materialize [OPTIONS] <PARENT> <COHORT>

Arguments:
  <PARENT>  The cohort parent's wallet
  <COHORT>  The cohort (its id, or a unique prefix of it)

Options:
      --index <INDEX>  The members' indices (comma-separated); default: the next `--next` not materialized yet
      --node <NODE>    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --next <NEXT>    How many of the next members not materialized yet [default: 1]
  -h, --help           Print help
```

Up to 128 members a transaction. A member is materialized once: a second attempt finds its id taken. Its name becomes a `.keyless` wallet file, signed for by the cohort's parent.

## `sion agent run`

```text
Put an agent to work on a task: a model drives it, with the ledger (balance, payments under its charter, transactions), optionally files in a workspace and the web, as tools. The session is kept, so one waiting for your approval can resume

Usage: sion agent run [OPTIONS] --task <TASK> --model <MODELS> <WALLET> <AGENT>

Arguments:
  <WALLET>  The agent's wallet
  <AGENT>   The agent (its id)

Options:
      --node <NODE>
          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --task <TASK>
          The task, in words
      --model <MODELS>
          The model: anthropic:<model> (ANTHROPIC_API_KEY), openai:<model> (OPENAI_BASE_URL, OPENAI_API_KEY), openai-responses:<model>, chatgpt:<model> (your ChatGPT plan, once signed in: `sion agent chatgpt login`), gemini:<model> (GEMINI_API_KEY) or script:<file>; several keys, comma-separated, are used in turn. Repeat for fallbacks, in order
      --autonomy <AUTONOMY>
          How much it may do without asking: L0 (nothing) to L6 (spend within its charter). L3 reads and writes its workspace; L5 also fetches from the web and asks before paying [default: L5]
      --workspace <WORKSPACE>
          A directory the agent may read and write
      --web
          Let it fetch public web pages (they taint the session: external calls and payments then wait for approval)
      --system <SYSTEM>
          A file with the system prompt (default: one that tells the agent who it is and how its charter limits it)
      --memory <MEMORY>
          The agent's memory log (its id, or a unique prefix of one with a local store), or `new` to open one for it. Its preferences, goals and procedures go into the prompt; what it records is committed when the session ends, with the session as an episode
      --skills <SKILLS>
          The directory of the agent's skills (default: $SION_HOME/agents/skills/<agent>)
      --no-reflect
          Do not ask the agent, after a long task it finished without writing a skill, whether to write down how it did it
      --delegate
          Let it hand work to other agents: spawn subagents with budgets carved from its own (agent_spawn), hire through Task Charters (task_post, task_status) and deliver tasks it was hired for (task_submit)
      --subagent-model <SUBAGENT_MODELS>
          The model its subagents run with (default: its own). Repeat for fallbacks
      --adjudicator <ADJUDICATOR>
          The adjudicator atom of the tasks it posts (without one, their results cannot be challenged)
      --shell [<SANDBOX>]
          Let it run commands with /bin/sh in a sandbox that can write only its workspace (needs --workspace), and its skills' scripts: `bwrap` (bubblewrap, the default) or `docker:<image>@sha256:<digest>`, an image already on this machine
      --sandbox-network <NETWORK>
          The network its commands reach: `none` (the default); `declared` (bubblewrap: commands get none, and skills' scripts reach what their skills declare, through a proxy that refuses private addresses); `host` (bubblewrap: this machine's network, unfiltered); or a Docker network you set up to restrict where they connect [default: none]
      --sandbox-read <DIR>
          A directory its commands may read, at its own path (a toolchain, say); with bubblewrap, its bin directory goes on the PATH. Repeat for more
      --sandbox-timeout <SECONDS>
          Seconds a command may run before it is stopped [default: 120]
      --mcp <SERVER>
          Give it the reviewed tools of this MCP server (see `sion agent mcp add`). Repeat for more
      --storage
          Give it the storage-deal tools: keep a file of its workspace with storage providers (the escrow comes from its account's balance, not its budget), follow its deals and close them
      --browser
          Give it a browser (Chromium or Chrome, found on this machine or at SION_BROWSER): it opens pages, reads them as their accessibility tree, clicks, types and takes screenshots. It runs in bubblewrap where that works, and reaches public addresses only
      --browser-host <HOST>
          A host its browser, and its desktop's programs, may reach (default: any public host); `*` names any public host. Where you name them, it acts on their pages on its own, as its autonomy allows, even after reading them (`agent:browser-hosts`). Repeat for more
      --login <NAME>
          One of your logins its browser may fill (`sion agent login add`): only into a field of the login's origin, and never shown to it. Repeat for more
      --computer [<SANDBOX>]
          Give it a desktop of its own, seen in screenshots and used with a pointer and a keyboard (Xvfb, xdotool and ImageMagick): in bubblewrap (`bwrap`, the default) or `docker:<image>@sha256:<digest>`, an image with those programs. Needs --workspace
      --computer-program <COMMAND>
          A program it may start on its desktop, as a command line (`firefox --new-instance`). Repeat for more
      --computer-size <SIZE>
          Its desktop's size, WIDTHxHEIGHT [default: 1280x800]
      --computer-network <NETWORK>
          With a Docker desktop, the Docker network its programs reach: one you set up (default: none) [default: none]
      --no-skill-approvals
          Ask before every call its autonomy does not cover, even those a skill it read pre-approves with `allowed-tools`
  -h, --help
          Print help
```

See [Agents at work](../guides/agent-runtime.md), [Commands, scripts and MCP servers](../guides/agent-tools.md) for `--shell`, `--mcp` and `--storage`, and [A browser and a desktop](../guides/agent-browser.md) for `--browser`, `--login` and `--computer`.

## `sion agent continue`

```text
Continue a session: approve or decline the call it waits on, or tell it more

Usage: sion agent continue [OPTIONS] <SESSION>

Arguments:
  <SESSION>  The session (its id, or a unique prefix of it)

Options:
      --approve            Approve the call it waits on
      --node <NODE>        The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --decline <DECLINE>  Decline the call it waits on, saying why
      --say <SAY>          Tell it something more and let it go on
  -h, --help               Print help
```

See [Agents at work](../guides/agent-runtime.md#approvals).

## `sion agent evolve`

```text
Evolve a profile's instructions (what follows the fixed system prompt) against an evaluation set: a mutator proposes variants for the cases its agent fails, each is scored under gates (size, new, no regression), and with --apply the best becomes the profile's

Usage: sion agent evolve [OPTIONS] --evals <EVALS> --mutator <MUTATOR> <PROFILE>

Arguments:
  <PROFILE>  The agent profile

Options:
      --evals <EVALS>              The evaluation set (a JSON file)
      --node <NODE>                The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --mutator <MUTATOR>          The model that proposes variants
      --generations <GENERATIONS>  Generations, at most [default: 3]
      --apply                      Make the best version the profile's instructions
  -h, --help                       Print help
```

## `sion agent sessions`

```text
The agent sessions on this machine; `search` finds an agent's past sessions by words

Usage: sion agent sessions [OPTIONS] [COMMAND]

Commands:
  search  Search an agent's past sessions on this machine by words (BM25): each hit is checked against the trajectory digest its episode recorded in the agent's memory log, whose head is on the ledger
  help    Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent sessions search`

```text
Search an agent's past sessions on this machine by words (BM25): each hit is checked against the trajectory digest its episode recorded in the agent's memory log, whose head is on the ledger

Usage: sion agent sessions search [OPTIONS] <AGENT> <QUERY>...

Arguments:
  <AGENT>     The agent: its wallet's name, or its id (or a prefix of it)
  <QUERY>...  The words to look for

Options:
      --limit <LIMIT>  How many sessions at most [default: 5]
      --node <NODE>    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help           Print help
```

## `sion agent trace`

```text
A session as one JSON line in chat format with tool calls, with its outcome: for review, evaluation or training

Usage: sion agent trace [OPTIONS] <SESSION>

Arguments:
  <SESSION>  The session (its id, or a unique prefix of it)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent kill-switch`

```text
The kill switch: `on` stops every agent run on this machine before its next call; `off` lets them run again

Usage: sion agent kill-switch [OPTIONS] <STATE>

Arguments:
  <STATE>  on or off

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent profile`

```text
Agent profiles: how to run an agent, under a name, for schedules and channels

Usage: sion agent profile [OPTIONS] <COMMAND>

Commands:
  add     Add a profile, or replace the one of that name
  list    The profiles on this machine
  remove  Remove a profile
  token   A profile's channel tokens: each reaches only that profile, through the HTTP channel or a gateway, and is kept only as its hash
  help    Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

Profiles are kept in `$SION_HOME/agents/profiles.json`. See [Teams, schedules and channels](../guides/agent-teams.md#profiles).

## `sion agent profile add`

```text
Add a profile, or replace the one of that name

Usage: sion agent profile add [OPTIONS] --model <MODELS> <NAME> <WALLET> <AGENT>

Arguments:
  <NAME>    Its name (lowercase letters, digits and hyphens)
  <WALLET>  The agent's wallet
  <AGENT>   The agent (its id)

Options:
      --model <MODELS>
          The model, as for `sion agent run`. Repeat for fallbacks
      --node <NODE>
          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --autonomy <AUTONOMY>
          How much it may do without asking, L0 to L6 [default: L5]
      --memory <MEMORY>
          Its memory log (its id, or a unique prefix of one with a local store)
      --skills <SKILLS>
          Its skills' directory
      --workspace <WORKSPACE>
          A directory it may read and write
      --web
          Let it fetch public web pages
      --delegate
          Let it hand work to other agents
      --subagent-model <SUBAGENT_MODELS>
          The model its subagents run with. Repeat for fallbacks
      --adjudicator <ADJUDICATOR>
          The adjudicator atom of the tasks it posts
      --no-reflect
          Do not ask it to write down long tasks as skills
      --shell [<SANDBOX>]
          Let it run commands with /bin/sh in a sandbox that can write only its workspace (needs --workspace), and its skills' scripts: `bwrap` (bubblewrap, the default) or `docker:<image>@sha256:<digest>`, an image already on this machine
      --sandbox-network <NETWORK>
          The network its commands reach: `none` (the default); `declared` (bubblewrap: commands get none, and skills' scripts reach what their skills declare, through a proxy that refuses private addresses); `host` (bubblewrap: this machine's network, unfiltered); or a Docker network you set up to restrict where they connect [default: none]
      --sandbox-read <DIR>
          A directory its commands may read, at its own path (a toolchain, say); with bubblewrap, its bin directory goes on the PATH. Repeat for more
      --sandbox-timeout <SECONDS>
          Seconds a command may run before it is stopped [default: 120]
      --mcp <SERVER>
          Give it the reviewed tools of this MCP server. Repeat for more
      --storage
          Give it the storage-deal tools
      --browser
          Give it a browser (Chromium or Chrome, found on this machine or at SION_BROWSER): it opens pages, reads them as their accessibility tree, clicks, types and takes screenshots. It runs in bubblewrap where that works, and reaches public addresses only
      --browser-host <HOST>
          A host its browser, and its desktop's programs, may reach (default: any public host); `*` names any public host. Where you name them, it acts on their pages on its own, as its autonomy allows, even after reading them (`agent:browser-hosts`). Repeat for more
      --login <NAME>
          One of your logins its browser may fill (`sion agent login add`): only into a field of the login's origin, and never shown to it. Repeat for more
      --computer [<SANDBOX>]
          Give it a desktop of its own, seen in screenshots and used with a pointer and a keyboard (Xvfb, xdotool and ImageMagick): in bubblewrap (`bwrap`, the default) or `docker:<image>@sha256:<digest>`, an image with those programs. Needs --workspace
      --computer-program <COMMAND>
          A program it may start on its desktop, as a command line (`firefox --new-instance`). Repeat for more
      --computer-size <SIZE>
          Its desktop's size, WIDTHxHEIGHT [default: 1280x800]
      --computer-network <NETWORK>
          With a Docker desktop, the Docker network its programs reach: one you set up (default: none) [default: none]
      --instructions <INSTRUCTIONS>
          What to tell it beyond the fixed system prompt
      --no-skill-approvals
          Ask before every call its autonomy does not cover, even those a skill it read pre-approves with `allowed-tools`
      --image-model <PROVIDER:MODEL>
          Let it make pictures in its workspace (`image_generate`): openai:<model>, such as openai:gpt-image-1 (OPENAI_API_KEY)
      --home-assistant
          Give it Home Assistant's devices as tools (HOME_ASSISTANT_URL, HOME_ASSISTANT_TOKEN): their states read, a service called as an external call
  -h, --help
          Print help
```

## `sion agent profile list`

```text
The profiles on this machine

Usage: sion agent profile list [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent profile remove`

```text
Remove a profile

Usage: sion agent profile remove [OPTIONS] <NAME>

Arguments:
  <NAME>  Its name

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent profile token`

```text
A profile's channel tokens: each reaches only that profile, through the HTTP channel or a gateway, and is kept only as its hash

Usage: sion agent profile token [OPTIONS] <COMMAND>

Commands:
  add     Make a token for a profile (shown once)
  list    The tokens on this machine, by id (never the tokens themselves)
  remove  Remove a token
  help    Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent profile token add`

```text
Make a token for a profile (shown once)

Usage: sion agent profile token add [OPTIONS] <PROFILE>

Arguments:
  <PROFILE>  The profile (on a gateway, a profile served from elsewhere)

Options:
      --agent <AGENT>  The profile's agent, for a profile served from another machine: a gateway routes the profile only to a host that proves this agent's key. A profile of this machine names its own
      --node <NODE>    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --approve        Let it approve and decline what the profile's sessions wait on
  -h, --help           Print help
```

## `sion agent profile token list`

```text
The tokens on this machine, by id (never the tokens themselves)

Usage: sion agent profile token list [OPTIONS] [PROFILE]

Arguments:
  [PROFILE]  Only this profile's

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent profile token remove`

```text
Remove a token

Usage: sion agent profile token remove [OPTIONS] <PROFILE> <ID>

Arguments:
  <PROFILE>  The profile
  <ID>       The token's id, as `list` shows it

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent secret`

```text
The secrets skills' scripts may receive (`sion-secrets` in a skill's metadata): kept encrypted on this machine, passed to a script as environment variables, and cut out of what it prints

Usage: sion agent secret [OPTIONS] <COMMAND>

Commands:
  add     Add a secret, or replace one: its value is read from standard input (not shown when typed), or taken from an environment variable
  list    The secrets' names (never their values) and when each was added
  remove  Remove a secret
  help    Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent secret add`

```text
Add a secret, or replace one: its value is read from standard input (not shown when typed), or taken from an environment variable

Usage: sion agent secret add [OPTIONS] <NAME>

Arguments:
  <NAME>  Its name, as the environment variable a script receives it in (capital letters, digits and underscores)

Options:
      --from-env <VARIABLE>  Take the value from this environment variable instead
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help                 Print help
```

## `sion agent secret list`

```text
The secrets' names (never their values) and when each was added

Usage: sion agent secret list [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent secret remove`

```text
Remove a secret

Usage: sion agent secret remove [OPTIONS] <NAME>

Arguments:
  <NAME>  Its name

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent login`

```text
Your logins agents' browsers may fill (`--login`): each bound to an origin, kept encrypted on this machine, filled only into fields of that origin, and never shown to the agent

Usage: sion agent login [OPTIONS] <COMMAND>

Commands:
  add     Add a login, or replace one: its password is read from standard input (not shown when typed), or taken from an environment variable
  list    The logins' names, origins and user names (never their passwords)
  remove  Remove a login
  help    Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

See [A browser and a desktop](../guides/agent-browser.md#logins).

## `sion agent login add`

```text
Add a login, or replace one: its password is read from standard input (not shown when typed), or taken from an environment variable

Usage: sion agent login add [OPTIONS] --origin <ORIGIN> --username <USERNAME> <NAME>

Arguments:
  <NAME>  Its name (lowercase letters, digits and hyphens), by which an agent asks for it

Options:
      --node <NODE>              The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --origin <ORIGIN>          The origin it is for: https://accounts.example.com. It is filled only into pages of that origin
      --username <USERNAME>      The user name
      --password-env <VARIABLE>  Take the password from this environment variable instead
  -h, --help                     Print help
```

The login is kept in `$SION_HOME/agents/logins.bin`, encrypted with ChaCha20-Poly1305 under the secrets' key (`agent:secret-store`), bound to the origin of the URL given.

## `sion agent login list`

```text
The logins' names, origins and user names (never their passwords)

Usage: sion agent login list [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent login remove`

```text
Remove a login

Usage: sion agent login remove [OPTIONS] <NAME>

Arguments:
  <NAME>  Its name

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent pairing`

```text
People a channel did not know who asked to talk to a profile: the codes waiting, approving one, letting a chat out again

Usage: sion agent pairing [OPTIONS] <COMMAND>

Commands:
  list     The codes waiting for approval, and the chats let in
  approve  Let in the chat a code was given to, on its channel
  revoke   Let a chat out of a channel again
  help     Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

A bridge with `--pairing` gives a chat it does not answer a code; the
person writing gives it to you, and approving it lets their chat in on
that channel, for that profile only (`agent:pairing`). See
[Pairing and voice notes](../guides/agent-teams.md#pairing-and-voice-notes).

## `sion agent pairing list`

```text
The codes waiting for approval, and the chats let in

Usage: sion agent pairing list [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

The codes themselves are not shown: approve the one you were given.

## `sion agent pairing approve`

```text
Let in the chat a code was given to, on its channel

Usage: sion agent pairing approve [OPTIONS] <CODE>

Arguments:
  <CODE>  The code (`ABCD-EFGH` or `ABCDEFGH`)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

A wrong code counts: five in a row stop approvals for an hour.

## `sion agent pairing revoke`

```text
Let a chat out of a channel again

Usage: sion agent pairing revoke [OPTIONS] <CHANNEL> <CHAT>

Arguments:
  <CHANNEL>  The channel and profile, as `list` shows it (`telegram:support`)
  <CHAT>     The chat

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```


## `sion agent chatgpt`

```text
Sign in with ChatGPT, so that agents may run on your plan (`--model chatgpt:<model>`): OpenAI's sign-in for open-source apps, in your browser; the tokens kept encrypted on this machine

Usage: sion agent chatgpt [OPTIONS] <COMMAND>

Commands:
  login   Sign in with your ChatGPT account, in your browser, so that agents may run on your plan (`--model chatgpt:<model>`)
  status  Who is signed in, and whether the plan is shared
  models  The models your plan offers
  logout  Forget who signed in
  help    Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

The account is the one you sign in with in your browser, on OpenAI's page; the bridge, the server and schedules use it like any model (`--model chatgpt:<model>`). See [Signing in with ChatGPT](../guides/agent-runtime.md#signing-in-with-chatgpt).

## `sion agent chatgpt login`

```text
Sign in with your ChatGPT account, in your browser, so that agents may run on your plan (`--model chatgpt:<model>`)

Usage: sion agent chatgpt login [OPTIONS]

Options:
      --no-browser   Show the address only: do not open a browser
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

Opens OpenAI's sign-in page, which comes back to `http://127.0.0.1:1455` (another port if that one is taken). The ID token is checked against OpenAI's published keys; the tokens are kept encrypted beside the secrets.

## `sion agent chatgpt status`

```text
Who is signed in, and whether the plan is shared

Usage: sion agent chatgpt status [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent chatgpt models`

```text
The models your plan offers

Usage: sion agent chatgpt models [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent chatgpt logout`

```text
Forget who signed in

Usage: sion agent chatgpt logout [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent weixin`

```text
Sign a WeChat account in for `--via weixin`, by a QR code scanned with WeChat (Tencent's iLink bot protocol), or sign it out

Usage: sion agent weixin [OPTIONS] <COMMAND>

Commands:
  login   Sign a WeChat account in: scan the QR code shown with WeChat, and confirm
  logout  Forget the sign-in
  help    Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

WeChat's iLink bot protocol is published only as Tencent's plugin's code and others' descriptions of it; the bridge has been run only against a stand-in built from them. See [WeChat](../guides/agent-teams.md#wechat).

## `sion agent weixin login`

```text
Sign a WeChat account in: scan the QR code shown with WeChat, and confirm

Usage: sion agent weixin login [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

Draws the QR code in the terminal (black on white where the output is a terminal) and prints its address; the sign-in is kept sealed beside the secrets (`weixin.bin`).

## `sion agent weixin logout`

```text
Forget the sign-in

Usage: sion agent weixin logout [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent service`

```text
Long-running commands (a bridge, the server, the scheduler) as services of your own: systemd user units on Linux, launchd agents on macOS

Usage: sion agent service [OPTIONS] <COMMAND>

Commands:
  install  Install a service that runs `sion` with the arguments after `--` (e.g. `sion agent service install support -- agent telegram support --pairing`)
  list     The services installed
  remove   Stop a service and take its files away
  help     Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

See [Services](../getting-started/install.md#services).

## `sion agent service install`

```text
Install a service that runs `sion` with the arguments after `--` (e.g. `sion agent service install support -- agent telegram support --pairing`)

Usage: sion agent service install [OPTIONS] <NAME> -- <ARGS>...

Arguments:
  <NAME>     Its name: letters, digits, `-` and `_`
  <ARGS>...  What it runs: `sion` and these arguments

Options:
      --env <NAME>   A variable it is given, by name, from this environment (a token): kept in a file only your user reads. May be repeated
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --start        Start it now, and at each login
  -h, --help         Print help
```

The tokens `--env` names are kept in plain text in a file only your user
reads (`agent:service-tokens`).

## `sion agent service list`

```text
The services installed

Usage: sion agent service list [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent service remove`

```text
Stop a service and take its files away

Usage: sion agent service remove [OPTIONS] <NAME>

Arguments:
  <NAME>  Its name

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent mcp`

```text
MCP servers whose tools agents may use: add one (its tools are reviewed and pinned), list them, review one again, remove one

Usage: sion agent mcp [OPTIONS] <COMMAND>

Commands:
  add     Add an MCP server for agents' sessions, and review its tools. The server is started and its tools listed, each with what its calls are taken to do; those offered are pinned by a digest of their definitions, so a tool that changes is not offered until reviewed again
  list    The MCP servers on this machine, with their reviewed tools
  review  Review a server's tools again: what changed, what is new and what is gone since its last review. With --accept, its tools are pinned as they are now (a tool keeps the larger of its old effect and what its annotations now say, unless --effect sets one)
  login   Log in to a server over HTTP that asks for authorization (OAuth): a browser opens where you authorize, and the tokens are kept, encrypted, for its sessions, which renew them while they can
  logout  Forget a server's authorization (its tokens and registration), revoking its tokens first where its authorization server takes revocations
  remove  Remove a server, and its authorization as `logout` does
  help    Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

Servers are kept in `$SION_HOME/agents/mcp.json`, with a digest of each reviewed tool's definition. See [Commands, scripts and MCP servers](../guides/agent-tools.md#tools-from-mcp-servers).

## `sion agent mcp add`

```text
Add an MCP server for agents' sessions, and review its tools. The server is started and its tools listed, each with what its calls are taken to do; those offered are pinned by a digest of their definitions, so a tool that changes is not offered until reviewed again

Usage: sion agent mcp add [OPTIONS] <NAME> [-- <COMMAND>...]

Arguments:
  <NAME>        Its name (lowercase letters, digits and hyphens)
  [COMMAND]...  The program and its arguments, after `--`

Options:
      --node <NODE>
          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --url <URL>
          Reach it over HTTP at this endpoint (Streamable HTTP, or the deprecated HTTP+SSE), instead of starting a program
      --header <HEADER>
          A header sent to a server over HTTP, `Name: value`; `${VAR}` in the value is the environment variable's value where the agent runs, so the secret itself is not kept. Repeat for more
      --client-id <CLIENT_ID>
          A client id registered with its authorization server (default: registering one, or the client ID metadata document)
      --client-secret-env <CLIENT_SECRET_ENV>
          The environment variable holding that client's secret
      --client-metadata-url <CLIENT_METADATA_URL>
          The HTTPS URL of a client ID metadata document to use as the client id, where the authorization server takes one
      --callback-port <CALLBACK_PORT>
          The loopback port your browser comes back to (default: a free one)
      --issuer <ISSUER>
          The authorization server to use, when the server names several
      --scope <SCOPE>
          Scopes to ask for besides those the server asks for
      --client-credentials
          Authorize the client itself (the client credentials grant), with no person: for agents that run unattended. Needs --client-id and --client-secret-env
      --no-browser
          Do not open a browser: print the URL to open
      --env <NAME>
          An environment variable it is given, by name; the value is the one where the agent runs. Repeat for more
      --dir <DIR>
          The directory it works in (in a sandbox, at /workspace)
      --allow <TOOL>
          Offer only this tool of its tools (default: all). Repeat for more
      --effect <TOOL=EFFECT>
          What a tool's calls are taken to do, whatever its annotations say: TOOL=read, write, external or spend. Repeat for more
      --timeout <TIMEOUT>
          Seconds a call may take [default: 120]
      --sandbox <SANDBOX>
          The sandbox it runs in: bwrap or docker:<image>@sha256:<digest>. Without one, it runs as you
      --sandbox-network <NETWORK>
          The network it reaches in its sandbox: none (the default), host (bubblewrap) or a Docker network you set up [default: none]
      --sandbox-read <DIR>
          A directory it may read in its sandbox. Repeat for more
  -h, --help
          Print help
```

The server is started (or, with `--url`, reached over HTTP), introduced to (protocol 2026-07-28, or the handshake of 2025-11-25 and before) and asked for its tools, then stopped. A tool that says nothing of itself is taken to reach the open world: its calls are external and what it returns taints the session. Without `--sandbox`, the server runs as you (`agent:mcp-unsandboxed`). A server over HTTP that answers `401` is logged in to first, as `sion agent mcp login` does; see [Servers over HTTP](../guides/agent-tools.md#servers-over-http).

## `sion agent mcp list`

```text
The MCP servers on this machine, with their reviewed tools

Usage: sion agent mcp list [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent mcp review`

```text
Review a server's tools again: what changed, what is new and what is gone since its last review. With --accept, its tools are pinned as they are now (a tool keeps the larger of its old effect and what its annotations now say, unless --effect sets one)

Usage: sion agent mcp review [OPTIONS] <NAME>

Arguments:
  <NAME>  The server

Options:
      --accept                Pin the tools as listed
      --node <NODE>           The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --effect <TOOL=EFFECT>  What a tool's calls are taken to do: TOOL=read, write, external or spend. Repeat for more
  -h, --help                  Print help
```

Without `--accept`, nothing changes. With it, a tool keeps the larger of its old effect and what its annotations now say, unless `--effect` sets one.

## `sion agent mcp login`

```text
Log in to a server over HTTP that asks for authorization (OAuth): a browser opens where you authorize, and the tokens are kept, encrypted, for its sessions, which renew them while they can

Usage: sion agent mcp login [OPTIONS] <NAME>

Arguments:
  <NAME>  The server

Options:
      --node <NODE>    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --scope <SCOPE>  Scopes to ask for besides those the server asks for
      --no-browser     Do not open a browser: print the URL to open
  -h, --help           Print help
```

The server's protected resource metadata names where to authorize; the client registered beforehand, named by a client ID metadata document, or registered then, is kept for later logins; tokens are kept encrypted beside the secrets. A browser on another machine cannot come back to this one: paste the address it was sent to at the terminal. See [Logging in to a server](../guides/agent-tools.md#logging-in-to-a-server).

## `sion agent mcp logout`

```text
Forget a server's authorization (its tokens and registration), revoking its tokens first where its authorization server takes revocations

Usage: sion agent mcp logout [OPTIONS] <NAME>

Arguments:
  <NAME>  The server

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

A session is not offered the server's tools again until the next login. Where the authorization server names no revocation endpoint, its tokens stay valid there until they expire (`agent:mcp-oauth`).

## `sion agent mcp remove`

```text
Remove a server, and its authorization as `logout` does

Usage: sion agent mcp remove [OPTIONS] <NAME>

Arguments:
  <NAME>  The server

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent schedule`

```text
Scheduled jobs: a task a profile works on at a time, every interval or on a cron expression, each run a session whose answer goes to a channel

Usage: sion agent schedule [OPTIONS] <COMMAND>

Commands:
  add     Schedule a task for a profile: once (--at), every interval (--every) or on a cron expression (--cron, UTC)
  list    The scheduled jobs, with their next runs and latest results
  remove  Remove a job
  help    Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

Jobs are kept in `$SION_HOME/agents/schedules.json`, with their latest runs. See [Teams, schedules and channels](../guides/agent-teams.md#schedules).

## `sion agent schedule add`

```text
Schedule a task for a profile: once (--at), every interval (--every) or on a cron expression (--cron, UTC)

Usage: sion agent schedule add [OPTIONS] --task <TASK> <NAME> <PROFILE>

Arguments:
  <NAME>     The job's name (lowercase letters, digits and hyphens)
  <PROFILE>  The agent profile that runs it

Options:
      --node <NODE>        The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --task <TASK>        The task, in words
      --every <EVERY>      Every interval: 90s, 15m, 2h, 1d
      --at <AT>            Once, at a time like 2026-11-02T09:00:00Z
      --cron <CRON>        On a cron expression: minute hour day month weekday, in UTC
      --deliver <DELIVER>  Where what it said goes: file:<path>, webhook:<url>, telegram:<chat id> (TELEGRAM_BOT_TOKEN), or <messenger>:<chat> with the bridge's credentials: slack, discord, matrix, signal, mattermost, whatsapp, line, sms, email, imessage, feishu, lark, nextcloud-talk or google-chat (with a service account)
  -h, --help               Print help
```

## `sion agent schedule list`

```text
The scheduled jobs, with their next runs and latest results

Usage: sion agent schedule list [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent schedule remove`

```text
Remove a job

Usage: sion agent schedule remove [OPTIONS] <NAME>

Arguments:
  <NAME>  Its name

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent scheduler`

```text
Run scheduled jobs as they fall due

Usage: sion agent scheduler [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --once         Run the jobs due now, then stop
      --poll <POLL>  Check for new jobs at least this often, in seconds [default: 60]
  -h, --help         Print help
```

A run that was missed while the scheduler was not running happens once, late (`agent:local-scheduler`).

## `sion agent serve`

```text
Serve the HTTP channel: POST /v1/profiles/<name>/messages with {"text": …, "conversation": …}, GET /v1/sessions/<id>, and OpenAI's POST /v1/chat/completions (a profile as the model) and GET /v1/models, with a bearer token: the machine's, or a profile's own (`sion agent profile token add`)

Usage: sion agent serve [OPTIONS]

Options:
      --listen <LISTEN>        Where to listen [default: 127.0.0.1:8790]
      --node <NODE>            The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --token-env <TOKEN_ENV>  The environment variable holding the machine's token, which reaches every profile and session (at least 16 characters); without it, only profiles' own tokens are taken [default: SION_AGENT_TOKEN]
      --a2a <PROFILE>          Give other agents A2A cards for these profiles (comma-separated): the first at /.well-known/agent-card.json, each at /v1/profiles/<name>/agent-card.json, answered at /v1/profiles/<name>/a2a
      --public-url <URL>       The address other agents reach this channel at, for the cards (https://agents.example.org); else the one a request came to
  -h, --help                   Print help
```

Whoever holds the token can start and continue every profile's sessions, approvals included (`agent:channel-token`). Requests are handled one at a time. With `--a2a`, the profiles named are A2A agents for other agents, with a card each ([Agents that talk A2A](../guides/agent-teams.md#agents-that-talk-a2a)). See [Teams, schedules and channels](../guides/agent-teams.md#http).

## `sion agent talk`

```text
Talk with a profile in the terminal: what you type goes to it, its reply comes as the model writes it; with --voice, heard and spoken

Usage: sion agent talk [OPTIONS] <PROFILE>

Arguments:
  <PROFILE>  The agent profile

Options:
      --conversation <CONVERSATION>  The conversation: each is a session of its own [default: default]
      --node <NODE>                  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --voice                        Speak instead of typing: press Enter and speak; heard with --hear, answered aloud with --speak if given
      --hear <PROVIDER>              What hears you: openai:<model>, groq:<model>, deepgram:<model>, elevenlabs:<model>, or command:<program>
      --speak <PROVIDER>             What speaks the replies: openai:<model>:<voice>, elevenlabs:<voice id>[:<model>], deepgram:<voice model>, or command:<program>
      --record <COMMAND>             The program that records you, given the file to write (sox's rec by default, stopping after a second and a half of silence)
      --play <COMMAND>               The program that plays a spoken reply, given its file (mpv, ffplay, afplay or paplay, the first there, by default)
  -h, --help                         Print help
```

The conversation is a session as on any channel, its commands included (`/new`, `/status`, `/approve`…). There is no `/stop` while the agent works: Ctrl-C ends the conversation, the session kept. See [In the terminal, and by voice](../guides/agent-teams.md#in-the-terminal-and-by-voice).

## `sion agent telegram`

```text
Answer Telegram chats with a profile, by long polling (TELEGRAM_BOT_TOKEN; TELEGRAM_API_BASE for another server)

Usage: sion agent telegram [OPTIONS] <PROFILE>

Arguments:
  <PROFILE>
          The agent profile

Options:
      --allow <ALLOW>
          The chats it answers (comma-separated ids); others are ignored, or given a pairing code with --pairing

      --node <NODE>
          The node's API

          [env: SION_NODE=]
          [default: http://127.0.0.1:8730]

      --approvers <APPROVERS>
          Who may approve or decline (comma-separated Telegram user ids); without it, anyone in an allowed chat

      --once
          Answer one batch of messages, then stop

      --pairing
          Give someone who writes to it directly, and whom it does not know, a pairing code, and answer them once you approve it (`sion agent pairing approve`). Matrix joins the rooms it is invited to; Slack needs Socket Mode (SLACK_APP_TOKEN)

      --require-mention
          In the group chats it answers, answer only what mentions it or answers one of its messages (direct messages are always answered)

      --hear <PROVIDER>
          Hear voice notes: openai:<model>, groq:<model>, or command:<program> (given the audio's file, it prints the text)

      --speak <PROVIDER>
          Answer voice notes aloud: openai:<model>:<voice>, elevenlabs:<voice id>[:<model>], or command:<program> (given the text, it writes Ogg Opus or MP3)

      --stream <STREAM>
          How a reply shows while the agent writes it, where the messenger can show it: `progress`, the text as the model writes it and the tool the agent runs; `partial`, the text only; `off`, the reply once it is done

          Possible values:
          - progress: The text as the model writes it, and the tool the agent runs
          - partial:  The text as the model writes it
          - off:      The reply once it is done

          [default: progress]

      --no-typing
          Show no typing indicator while the agent works

      --busy <BUSY>
          What a message does that comes while the agent works on its chat: `steer`, it is told to the agent as it works; `queue`, answered after, one at a time; `collect`, answered after, together; `interrupt`, it stops the agent, which takes it up. `/stop` always stops it

          Possible values:
          - steer:     Told to the agent as it works: it takes it in before its next step
          - queue:     Answered once the agent is done, one at a time
          - collect:   Answered once the agent is done, all of them together
          - interrupt: Stops the agent, which then takes it up

          [default: steer]

      --debounce <MS>
          Milliseconds a chat's messages are gathered before the agent takes them up, so that a burst of them is one turn

          [default: 500]

      --parallel <N>
          How many chats are answered at once

          [default: 4]

      --no-reactions
          Mark no message received, answered or failed (with a reaction, or a read receipt)

      --route <CHAT=PROFILE>
          A chat that another profile answers: `<chat>=<profile>`, as often as needed (comma-separated too). The chat is answered as --allow's

  -h, --help
          Print help (see a summary with '-h')
```

In a group chat, every member speaks for you, approvals included (`agent:telegram-chats`). See [Teams, schedules and channels](../guides/agent-teams.md#telegram).

## `sion agent bridge`

```text
Answer a messenger's chats with a profile: slack (SLACK_BOT_TOKEN, SLACK_APP_TOKEN for Socket Mode), discord (DISCORD_BOT_TOKEN), matrix (MATRIX_HOMESERVER, MATRIX_ACCESS_TOKEN), signal (SIGNAL_API_BASE, SIGNAL_NUMBER, for signal-cli-rest-api), telegram, mattermost (MATTERMOST_URL, MATTERMOST_TOKEN), irc (IRC_SERVER, IRC_NICK, IRC_PASSWORD), twitch (TWITCH_NICK, TWITCH_TOKEN), email (EMAIL_ADDRESS, EMAIL_IMAP_URL, EMAIL_SMTP_URL, EMAIL_PASSWORD or EMAIL_TOKEN_COMMAND), imessage (through imsg on the Mac that runs Messages, IMSG_COMMAND), dingtalk (DINGTALK_CLIENT_ID, DINGTALK_CLIENT_SECRET), or through webhooks to --listen: whatsapp (WHATSAPP_TOKEN, WHATSAPP_PHONE_NUMBER_ID, WHATSAPP_APP_SECRET, WHATSAPP_VERIFY_TOKEN), line (LINE_CHANNEL_ACCESS_TOKEN, LINE_CHANNEL_SECRET), sms through Twilio (TWILIO_ACCOUNT_SID, TWILIO_AUTH_TOKEN, TWILIO_NUMBER, TWILIO_WEBHOOK_URL), teams (TEAMS_APP_ID, TEAMS_APP_PASSWORD, TEAMS_TENANT_ID for a single-tenant bot), google-chat (GOOGLE_CHAT_AUDIENCE, and GOOGLE_CHAT_CREDENTIALS to answer through the Chat API), feishu or lark (FEISHU_APP_ID, FEISHU_APP_SECRET, FEISHU_VERIFICATION_TOKEN, FEISHU_ENCRYPT_KEY), nextcloud-talk (NEXTCLOUD_URL, NEXTCLOUD_TALK_SECRET), wecom (WECOM_CORP_ID, WECOM_AGENT_ID, WECOM_SECRET, WECOM_TOKEN, WECOM_AES_KEY), qq (QQ_APP_ID, QQ_APP_SECRET), synology-chat (SYNOLOGY_CHAT_INCOMING_URL, SYNOLOGY_CHAT_TOKEN) or phone calls through Twilio Voice (TWILIO_AUTH_TOKEN, TWILIO_VOICE_URL); or weixin (after `sion agent weixin login`), zalo (ZALO_BOT_TOKEN), ntfy (NTFY_URL, NTFY_TOKEN, its topics as --allow), x, its direct messages (X_ACCESS_TOKEN), simplex, through SimpleX's terminal client (SIMPLEX_URL), or nostr, direct messages on relays (NOSTR_PRIVATE_KEY, NOSTR_RELAYS)

Usage: sion agent bridge [OPTIONS] --via <VIA> <PROFILE>

Arguments:
  <PROFILE>
          The agent profile

Options:
      --node <NODE>
          The node's API

          [env: SION_NODE=]
          [default: http://127.0.0.1:8730]

      --via <VIA>
          The messenger

      --allow <ALLOW>
          The chats it answers (comma-separated: channel ids, room ids or numbers); others are ignored, or given a pairing code with --pairing

      --approvers <APPROVERS>
          Who may approve or decline (comma-separated, as the messenger names people); without it, anyone in an allowed chat

      --once
          Answer one batch of messages, then stop

      --listen <LISTEN>
          Where whatsapp, line, sms, teams, google-chat, feishu, nextcloud-talk, wecom, qq, synology-chat and phone send their webhooks (expose it to them behind TLS)

          [default: 127.0.0.1:8797]

      --pairing
          Give someone who writes to it directly, and whom it does not know, a pairing code, and answer them once you approve it (`sion agent pairing approve`). Matrix joins the rooms it is invited to; Slack needs Socket Mode (SLACK_APP_TOKEN)

      --require-mention
          In the group chats it answers, answer only what mentions it or answers one of its messages (direct messages are always answered)

      --hear <PROVIDER>
          Hear voice notes: openai:<model>, groq:<model>, or command:<program> (given the audio's file, it prints the text)

      --speak <PROVIDER>
          Answer voice notes aloud: openai:<model>:<voice>, elevenlabs:<voice id>[:<model>], or command:<program> (given the text, it writes Ogg Opus or MP3)

      --stream <STREAM>
          How a reply shows while the agent writes it, where the messenger can show it: `progress`, the text as the model writes it and the tool the agent runs; `partial`, the text only; `off`, the reply once it is done

          Possible values:
          - progress: The text as the model writes it, and the tool the agent runs
          - partial:  The text as the model writes it
          - off:      The reply once it is done

          [default: progress]

      --no-typing
          Show no typing indicator while the agent works

      --busy <BUSY>
          What a message does that comes while the agent works on its chat: `steer`, it is told to the agent as it works; `queue`, answered after, one at a time; `collect`, answered after, together; `interrupt`, it stops the agent, which takes it up. `/stop` always stops it

          Possible values:
          - steer:     Told to the agent as it works: it takes it in before its next step
          - queue:     Answered once the agent is done, one at a time
          - collect:   Answered once the agent is done, all of them together
          - interrupt: Stops the agent, which then takes it up

          [default: steer]

      --debounce <MS>
          Milliseconds a chat's messages are gathered before the agent takes them up, so that a burst of them is one turn

          [default: 500]

      --parallel <N>
          How many chats are answered at once

          [default: 4]

      --no-reactions
          Mark no message received, answered or failed (with a reaction, or a read receipt)

      --route <CHAT=PROFILE>
          A chat that another profile answers: `<chat>=<profile>`, as often as needed (comma-separated too). The chat is answered as --allow's

  -h, --help
          Print help (see a summary with '-h')
```

## `sion agent gateway`

```text
Run a gateway: people write to profiles served from other machines, through POST /v1/profiles/<name>/messages with a profile's token (`sion agent profile token add`); hosts connect out to it and prove they hold each profile's agent key, and messages reach an agent only while the ledger has it in good standing

Usage: sion agent gateway [OPTIONS] --name <NAME>

Options:
      --listen <LISTEN>        Where to listen [default: 127.0.0.1:8793]
      --node <NODE>            The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --name <NAME>            The gateway's name, which hosts sign: a login cannot be replayed at another gateway
      --token-env <TOKEN_ENV>  The environment variable holding a token that reaches every profile, if one is wanted [default: SION_GATEWAY_TOKEN]
  -h, --help                   Print help
```

## `sion agent host`

```text
Serve profiles through a gateway: connect out to it, prove the agents' keys, and answer the messages it passes on

Usage: sion agent host [OPTIONS] --profile <PROFILES> <GATEWAY>

Arguments:
  <GATEWAY>  The gateway's address, for example https://gateway.example.org

Options:
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --profile <PROFILES>   A profile to serve. Repeat for more
      --messages <MESSAGES>  Answer this many messages, then stop
  -h, --help                 Print help
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
      --must-attest <MUST_ATTEST>  Verifiers who must each attest every result, among --verifiers (comma-separated; spec v1.15)
      --min-attestations <N>       How many attestations a result needs at least (spec v1.15) [default: 0]
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
  open         Open a memory log on the ledger, owned by the wallet (for an agent identity with --agent, whose agent must be the wallet), and a local store for its atoms
  remember     Record an atom in a log's local store. It reaches the ledger's commitment with the next `sion memory commit`
  commit       Commit the local log's head to the ledger, with a proof that it extends the head the ledger holds
  recall       Everything the log had recorded in a box during an interval, as of its committed head, with a proof checked against the index root the ledger holds
  show         A log's head on the ledger and its local store
  consolidate  Consolidate an agent's memory: a model reads the episodes recorded since the last consolidation and the notes the log keeps, and proposes facts, procedures and preferences worth keeping, each citing its episodes. What the gates keep is recorded as derived atoms (the notes they replace are superseded, not deleted), and the log is committed
  archive      Archive a log for safekeeping: every atom the ledger holds, encrypted for the writer, named on the ledger as the log's manifest and kept by a storage provider under a deal
  restore      Restore a log's local store from its latest archive, checked against the head on the ledger before anything is replaced
  help         Print this message or the help of the given subcommand(s)

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

## `sion memory consolidate`

```text
Consolidate an agent's memory: a model reads the episodes recorded since the last consolidation and the notes the log keeps, and proposes facts, procedures and preferences worth keeping, each citing its episodes. What the gates keep is recorded as derived atoms (the notes they replace are superseded, not deleted), and the log is committed

Usage: sion memory consolidate [OPTIONS] --model <MODEL> <WALLET> <LOG>

Arguments:
  <WALLET>  The writer's wallet (the agent's)
  <LOG>     The log (its id, or a unique prefix of it)

Options:
      --model <MODEL>  The model that proposes: anthropic:<model>, openai:<model>, gemini:<model> or script:<file>
      --node <NODE>    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help           Print help
```

## `sion memory archive`

```text
Archive a log for safekeeping: every atom the ledger holds, encrypted for the writer, named on the ledger as the log's manifest and kept by a storage provider under a deal

Usage: sion memory archive [OPTIONS] --price <PRICE> --rounds <ROUNDS> <WALLET> <LOG>

Arguments:
  <WALLET>  The writer's wallet
  <LOG>     The log (its id, or a unique prefix of it)

Options:
      --node <NODE>                    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --out <OUT>                      Where to write the archive (default: $SION_HOME/memory/archives/<log>-<size>.bin)
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

## `sion memory restore`

```text
Restore a log's local store from its latest archive, checked against the head on the ledger before anything is replaced

Usage: sion memory restore [OPTIONS] --from <FROM> <WALLET> <LOG>

Arguments:
  <WALLET>  The writer's wallet
  <LOG>     The log (its full id, if the local store is gone)

Options:
      --from <FROM>  The archive: a file, or the retrieval address of the provider that keeps it (http:// or https://)
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --deal <DEAL>  With a provider: the deal that keeps the archive (default: your latest accepted deal for the log's manifest)
      --replace      Restore over the local store there is
  -h, --help         Print help
```

## `sion skill`

```text
Skills with a supply chain: pack a skill, audit a bundle, publish a release on bonds and audits, show one, install one for an agent under the decision rule; evaluate a skill, evolve it, and have a better version verified by others through a bounty

Usage: sion skill [OPTIONS] <COMMAND>

Commands:
  pack     Print a skill directory's bundle root, the commitment releases and audits name, and write its bundle file
  audit    Audit a bundle: claim, on your bond, that it does what its description says and nothing else
  publish  Publish a release of a skill: a claim naming it, its version and its bundle root, on your bond or the audits of that root, or both
  latest   A publisher's latest release of a skill, from its index on the ledger (spec v1.15), proven against a certified header
  show     A release: what it names, the backing behind it, and the decision rule for a value at risk
  install  Install a release for an agent: fetch and check its bundle, accept the release with the agent's remaining budget at risk, and only then put the skill in the agent's skills
  eval     Run an evaluation set on a skill: each case with stand-in tools, then its checks. Needs no node
  evolve   Evolve a skill against an evaluation set: a model proposes variants (new instructions, and other files but never scripts) for the cases it fails, each is scored under gates, and the best version found is kept as a draft. Needs no node
  vouch    Vouch for an evaluation set on your bond: claim that it has no known way to be passed without doing what the skill describes
  bounty   Post a bounty for a better version of a release: a Task Charter whose spec names the release, the evaluation set and the cases to pass, and whose verifiers are fixed now, before any version exists
  verify   Verify a version for a bounty, as one of its verifiers: check that it keeps the skill's purpose, run the evaluation set on it yourself, and attest on your bond only if enough cases pass
  submit   Submit a version for a bounty, as its worker, with its attestations, and settle the task
  lineage  A release's lineage: each version back to the first, with what it rests on and the evidence for each step
  help     Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

See [Skills from others](../guides/skills.md).

## `sion skill pack`

```text
Print a skill directory's bundle root, the commitment releases and audits name, and write its bundle file

Usage: sion skill pack [OPTIONS] <DIR>

Arguments:
  <DIR>  The skill directory (with a SKILL.md)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --out <OUT>    Where to write the bundle (default: the local bundle store)
  -h, --help         Print help
```

## `sion skill audit`

```text
Audit a bundle: claim, on your bond, that it does what its description says and nothing else

Usage: sion skill audit [OPTIONS] --bond <BOND> --adjudicator <ADJUDICATOR> <WALLET> <BUNDLE>

Arguments:
  <WALLET>  The auditor's wallet
  <BUNDLE>  The bundle root (64 hex digits)

Options:
      --bond <BOND>                The label of your bond the audit rests on
      --node <NODE>                The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --adjudicator <ADJUDICATOR>  The adjudicator atom that rules on challenges
      --note <NOTE>                What you checked [default: ""]
      --valid-for <VALID_FOR>      Validity in Pulses [default: 100000]
  -h, --help                       Print help
```

## `sion skill publish`

```text
Publish a release of a skill: a claim naming it, its version and its bundle root, on your bond or the audits of that root, or both

Usage: sion skill publish [OPTIONS] --version <VERSION> --adjudicator <ADJUDICATOR> <WALLET> <DIR>

Arguments:
  <WALLET>  The publisher's wallet
  <DIR>     The skill directory (with a SKILL.md)

Options:
      --node <NODE>                The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --version <VERSION>          The version, for example 1.0.0
      --bond <BOND>                The label of your bond the release rests on
      --audits <AUDITS>            Audits of the bundle the release rests on (claim ids)
      --adjudicator <ADJUDICATOR>  The adjudicator atom that rules on challenges
      --evidence <EVIDENCE>        Results of settled bounties for this version (claim ids): the release rests on them too, and records them
      --previous <PREVIOUS>        The release this one follows (its claim id)
      --locator <LOCATOR>          Where installers can fetch the bundle (an https URL)
      --valid-for <VALID_FOR>      Validity in Pulses [default: 100000]
      --no-index                   Do not name the release as your latest in your index (spec v1.15)
  -h, --help                       Print help
```

## `sion skill latest`

```text
A publisher's latest release of a skill, from its index on the ledger (spec v1.15), proven against a certified header

Usage: sion skill latest [OPTIONS] <SKILL>

Arguments:
  <SKILL>  The publisher (wallet name or account) and the skill, as `publisher/name`

Options:
      --chain <CHAIN>          The chain identity the proof must verify against. Without it the node vouches for itself [env: SION_CHAIN=]
      --node <NODE>            The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --min-pulse <MIN_PULSE>  A certified Pulse you know of from elsewhere: a proof from more than 16 Pulses before it is refused as stale
  -h, --help                   Print help
```

## `sion skill show`

```text
A release: what it names, the backing behind it, and the decision rule for a value at risk

Usage: sion skill show [OPTIONS] <RELEASE>

Arguments:
  <RELEASE>  The release (its claim id)

Options:
      --node <NODE>    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --value <VALUE>  The value at risk to decide for (USD) [default: 100]
  -h, --help           Print help
```

## `sion skill install`

```text
Install a release for an agent: fetch and check its bundle, accept the release with the agent's remaining budget at risk, and only then put the skill in the agent's skills

Usage: sion skill install [OPTIONS] <WALLET> <AGENT> <RELEASE>

Arguments:
  <WALLET>   The agent's wallet
  <AGENT>    The agent (its id)
  <RELEASE>  The release: its claim id, or `publisher/name` for the latest release in the publisher's index, proven against a certified header

Options:
      --chain <CHAIN>          With `publisher/name`: the chain identity the index's proof must verify against. Without it the node vouches for itself [env: SION_CHAIN=]
      --node <NODE>            The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --min-pulse <MIN_PULSE>  With `publisher/name`: a certified Pulse you know of from elsewhere; an index proof from more than 16 Pulses before it is refused as stale
      --bundle <BUNDLE>        The bundle file (default: the local bundle store, then the release's locator)
      --value <VALUE>          The value at risk (USD; default: the agent's remaining budget)
      --kappa <KAPPA>          κ, the margin the backing must have over the value at risk [default: 2]
      --pulses <PULSES>        How long the agent relies on it, in Pulses [default: 10000]
      --skills <SKILLS>        The directory of the agent's skills (default: $SION_HOME/agents/skills/<agent>)
      --accept-permissions     Grant the permissions the skill declares (calls it pre-approves, where its scripts may connect, the secrets they receive), shown before anything is signed. Without it, a skill that declares any is not installed
  -h, --help                   Print help
```

Asks for the decision before signing: a release whose backing does not cover the value at risk is refused without a fee.

## `sion skill eval`

```text
Run an evaluation set on a skill: each case with stand-in tools, then its checks. Needs no node

Usage: sion skill eval [OPTIONS] --evals <EVALS> --model <MODELS> <DIR>

Arguments:
  <DIR>  The skill directory (with a SKILL.md)

Options:
      --evals <EVALS>                The evaluation set (a JSON file)
      --node <NODE>                  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --model <MODELS>               The agent's model: anthropic:<model>, openai:<model>, gemini:<model> or script:<file>. Repeat for fallbacks, in order
      --trajectories <TRAJECTORIES>  Write each run as a trajectory (JSON Lines in chat format), labelled with its result
  -h, --help                         Print help
```

Needs no node. See [Skills that get better](../guides/skill-improvement.md#measuring-a-skill).

## `sion skill evolve`

```text
Evolve a skill against an evaluation set: a model proposes variants (new instructions, and other files but never scripts) for the cases it fails, each is scored under gates, and the best version found is kept as a draft. Needs no node

Usage: sion skill evolve [OPTIONS] --evals <EVALS> --model <MODELS> --mutator <MUTATOR> <DIR>

Arguments:
  <DIR>  The skill directory (with a SKILL.md)

Options:
      --evals <EVALS>              The evaluation set (a JSON file)
      --node <NODE>                The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --model <MODELS>             The agent's model, as for `eval`. Repeat for fallbacks
      --mutator <MUTATOR>          The model that proposes variants
      --generations <GENERATIONS>  Generations, at most [default: 3]
      --max-bytes <MAX_BYTES>      The largest SKILL.md a version may have, in bytes [default: 15000]
  -h, --help                       Print help
```

Needs no node. Gates: the name and description unchanged, the size limit, a version not tried before, and no case lost that the parent passed. See [Skills that get better](../guides/skill-improvement.md#evolving-a-better-version).

## `sion skill vouch`

```text
Vouch for an evaluation set on your bond: claim that it has no known way to be passed without doing what the skill describes

Usage: sion skill vouch [OPTIONS] --evals <EVALS> --bond <BOND> --adjudicator <ADJUDICATOR> <WALLET>

Arguments:
  <WALLET>  Your wallet

Options:
      --evals <EVALS>              The evaluation set (a JSON file)
      --node <NODE>                The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --bond <BOND>                The label of your bond the claim rests on
      --adjudicator <ADJUDICATOR>  The adjudicator atom that rules on challenges
      --valid-for <VALID_FOR>      Validity in Pulses [default: 100000]
  -h, --help                       Print help
```

## `sion skill bounty`

```text
Post a bounty for a better version of a release: a Task Charter whose spec names the release, the evaluation set and the cases to pass, and whose verifiers are fixed now, before any version exists

Usage: sion skill bounty [OPTIONS] --evals <EVALS> --amount <AMOUNT> --verifiers <VERIFIERS> --adjudicator <ADJUDICATOR> <WALLET> <RELEASE>

Arguments:
  <WALLET>   The poster's wallet
  <RELEASE>  The release to improve (its claim id)

Options:
      --evals <EVALS>              The evaluation set (a JSON file)
      --node <NODE>                The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --amount <AMOUNT>            Escrow (USD), paid for an accepted version
      --verifiers <VERIFIERS>      Verifiers whose attestations count (comma-separated wallets or accounts)
      --must-attest <MUST_ATTEST>  Verifiers who must each attest a version, among --verifiers (comma-separated)
      --min-attestations <N>       How many attestations a version needs at least [default: 1]
      --adjudicator <ADJUDICATOR>  The adjudicator atom results must name
      --requires <REQUIRES>        Claims every result must rest on, such as the set's vouching (comma-separated ids)
      --worker <WORKER>            The worker (default: anyone may take it)
      --pass <PASS>                The cases a version must pass (default: all)
      --deadline <DEADLINE>        Pulses until the deadline [default: 1000]
      --kappa <KAPPA>              κ of your decision rule for a result [default: 2]
      --reliance <RELIANCE>        Pulses you rely on a paid version (its backing stays reserved) [default: 1000]
  -h, --help                       Print help
```

The spec is kept in `$SION_HOME/skills/specs`, under the commitment the task records. See [Skills that get better](../guides/skill-improvement.md#a-bounty-fixed-before-any-better-version-exists).

## `sion skill verify`

```text
Verify a version for a bounty, as one of its verifiers: check that it keeps the skill's purpose, run the evaluation set on it yourself, and attest on your bond only if enough cases pass

Usage: sion skill verify [OPTIONS] --model <MODELS> <WALLET> <TASK> <DIR>

Arguments:
  <WALLET>  The verifier's wallet
  <TASK>    The bounty (its task id)
  <DIR>     The version's directory

Options:
      --model <MODELS>  The agent's model, as for `eval`. Repeat for fallbacks
      --node <NODE>     The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --evals <EVALS>   The evaluation set (default: the local store, by the digest the spec names)
      --spec <SPEC>     The task's spec (default: the local store)
      --bond <BOND>     The label of your bond the attestation rests on [default: verifier]
  -h, --help            Print help
```

Refuses a wallet the task does not name as a verifier, the task's worker, a closed task, the release the task improves, and a version that changes the skill's name or description. The results are printed before anything is signed.

## `sion skill submit`

```text
Submit a version for a bounty, as its worker, with its attestations, and settle the task

Usage: sion skill submit [OPTIONS] <WALLET> <TASK> <DIR>

Arguments:
  <WALLET>  The worker's wallet
  <TASK>    The bounty (its task id)
  <DIR>     The version's directory

Options:
      --attestations <ATTESTATIONS>  Attestation claim ids (comma-separated)
      --node <NODE>                  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help                         Print help
```

Refuses an attestation by the submitter itself. The worker needs a task bond (`sion bond <worker> task-worker <amount>`).

## `sion skill lineage`

```text
A release's lineage: each version back to the first, with what it rests on and the evidence for each step

Usage: sion skill lineage [OPTIONS] <RELEASE>

Arguments:
  <RELEASE>  The release (its claim id)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion storage`

```text
Storage deals: have data kept by a provider who proves every round that it still holds it, or provide storage and be paid for it

Usage: sion storage [OPTIONS] <COMMAND>

Commands:
  prepare   Split a file into leaves and print the root a deal would commit to
  post      Post a deal for a file: escrow PRICE × ROUNDS to have it kept and proven every round
  deals     List storage deals
  show      One deal: its terms, its state and its provider's progress
  accept    Accept a deal as its provider: fetch its data (from --file or its locator), check it hashes to the deal's root, keep a copy and post the collateral
  prove     Prove the round under way for every deal you hold
  settle    Collect for the rounds you proved
  retrieve  Retrieve a deal's data as its client from the provider that keeps it (the address its `sion storage serve --listen` gives), checked against the deal's root
  close     Close a deal: refund an open one (client), or end an accepted one at its end (client or provider) or once its provider abandoned it (client)
  serve     Run a provider: accept deals whose data you can fetch, prove every round, collect and close, every second until stopped
  help      Print this message or the help of the given subcommand(s)

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

## `sion storage retrieve`

```text
Retrieve a deal's data as its client from the provider that keeps it (the address its `sion storage serve --listen` gives), checked against the deal's root

Usage: sion storage retrieve [OPTIONS] --provider-url <PROVIDER_URL> --out <OUT> <FROM> <DEAL>

Arguments:
  <FROM>  Your wallet (the client)
  <DEAL>  The deal (its id, or a unique prefix of it)

Options:
      --node <NODE>                  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --provider-url <PROVIDER_URL>  The provider's retrieval address, for example http://127.0.0.1:8740
      --out <OUT>                    Where to write the data
  -h, --help                         Print help
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
      --listen <LISTEN>
          Serve the data of the deals you keep to their clients on this address: POST /v1/deals/<deal>/retrieve, signed by the deal's client
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
Run a scripted demo: invoice, agents, swarm, batch, tasks, robots, storage, market, receipts, x402, streams, censorship, light, staking or all

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
      --only <ONLY>  Run only these groups: signatures, authorization, state, data, clearing, assurance, execution, streams, lineage, devnet (comma-separated)
      --json <JSON>  Write the measurements as JSON to this file
  -h, --help         Print help
```

See [Benchmarks](../guides/benchmarks.md).
