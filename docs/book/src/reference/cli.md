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
  serve`, `x402 fetch`, `proving attest` and `proving show` pins the chain
  identity. Without it, the node vouches for itself, and the output says
  so.
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
| Agents | `charter create`, `charter pay`, `charter revoke`, `agent register`, `agent spawn`, `agent show`, `agent card`, `agent card-template`, `agent suspend`, `agent resume`, `agent revoke`, `agent reclaim`, `agent run`, `agent continue`, `agent sessions`, `agent trace`, `agent kill-switch`, `agent profile add`, `agent profile list`, `agent profile remove`, `agent schedule add`, `agent schedule list`, `agent schedule remove`, `agent scheduler`, `agent serve`, `agent talk`, `agent meeting`, `agent rent serve`, `agent rent use`, `agent sell serve`, `agent sell orders`, `agent sell fulfill`, `agent sell refund`, `agent sell feed`, `agent buy`, `agent commerce psp`, `agent commerce three-ds`, `agent commerce feeds`, `agent commerce webhooks`, `agent commerce keygen`, `agent hooks add`, `agent browser login`, `agent telegram`, `agent mcp add`, `agent mcp sampling`, `agent mcp resources`, `agent mcp prompts`, `mcp`, `mcp mail`, `mcp calendar`, `mcp documents`, `x402 serve`, `x402 fetch` |
| Skills | `skill import`, `skill starter`, `skill pack`, `skill audit`, `skill publish`, `skill show`, `skill install`, `skill eval`, `skill evolve`, `skill vouch`, `skill bounty`, `skill verify`, `skill submit`, `skill lineage` |
| Work | `task post`, `task attest`, `task submit`, `task settle`, `task cancel`, `task show` |
| Proving | `proving request`, `proving fetch`, `proving attest`, `proving deliver`, `proving show` |
| Memory | `memory open`, `memory remember`, `memory commit`, `memory recall`, `memory show` |
| Storage | `storage prepare`, `storage post`, `storage deals`, `storage show`, `storage accept`, `storage prove`, `storage settle`, `storage close`, `storage serve` |
| Stake | `stake pools`, `stake delegate`, `stake show`, `stake claim`, `stake undelegate`, `stake withdraw`, `stake commission` |
| Markets | `market create`, `market order`, `market clear`, `market show` |
| Payment streams | `stream open`, `stream pay`, `stream jump`, `stream check`, `stream settle`, `stream top-up`, `stream close`, `stream show` |
| Verification | `tx`, `receipt`, `verify` |
| Your computer | `setup`, `update`, `deploy`, `agent up`, `agent credential add`, `agent credential list`, `agent credential remove` |
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

Usage: sion [OPTIONS] [COMMAND]

Commands:
  devnet     Run a devnet and serve its API
  genesis    Collect validators' entries into a network's genesis file
  node       Run one validator of a network, in this process
  gateway    Run a gateway of a network, in this process
  status     Chain status
  setup      Set up Siontology for one person on this machine: a chain of your own (a service of yours), your wallet and your agent's, the agent with a budget and its memory, its model, a profile `sion` talks with, and Telegram if you like. Safe to run again
  update     Replace this sion with the latest release, once its signature and SHA-256 are checked
  deploy     Put your agents on a server of yours over SSH, so that they work while this computer is off: sion copied there, set up there (or this machine's home copied, with --copy-home), and everything run by one service of yours (sion agent up)
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
  proving    A market for proofs of Pulses: ask for a Pulse's World Proof with a payment in escrow, deliver one, and attest one as a verifier after checking it under both proof stacks
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
  mcp        Serve the Model Context Protocol on stdio (for AI agents): the ledger's tools, or a person's mail, calendars or documents
  x402       Sell or buy HTTP resources with x402 (HTTP 402 payments settled on Siontology)
  bench      Measure primitives, execution, proving, clearing and a devnet
  help       Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -c, --continue     With no command: continue the last conversation, rather than start one
  -h, --help         Print help
  -V, --version      Print version

With no command, sion talks with your agent in a new conversation (`sion setup` makes one); `sion -c` continues the last.
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
      --idle-round-ms <MS>
          When a validator has nothing to order, wait this long before its next round (0: never): a chain at rest then uses little CPU. A transaction, or another validator's header, starts the round at once, and the chain goes at full pace until the Pulse holding it closes [default: 0]
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


## `sion setup`

```text
Set up Siontology for one person on this machine: a chain of your own (a service of yours), your wallet and your agent's, the agent with a budget and its memory, its model, a profile `sion` talks with, and Telegram if you like. Safe to run again

Usage: sion setup [OPTIONS]

Options:
      --name <NAME>              Your wallet's name (default: your login name)
      --node <NODE>              The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --profile <PROFILE>        The profile to make, which bare `sion` talks with [default: assistant]
      --model <MODEL>            Its model: anthropic:<model>, openai-responses:<model>, gemini:<model>, chatgpt:<model> or local:<model>
      --key-from-env <VARIABLE>  Take the model's key from this environment variable
      --local-url <URL>          For local:<model>, where its server answers (OpenAI's API): Ollama http://127.0.0.1:11434/v1, LM Studio http://127.0.0.1:1234/v1, llama.cpp http://127.0.0.1:8080/v1
      --workspace <WORKSPACE>    The directory it may read and write (default: ~/sion-workspace)
      --autonomy <AUTONOMY>      How much it may do without asking, L0 to L6 [default: L5]
      --idle-round-ms <MS>       How long the chain waits between rounds while it has nothing to order, in milliseconds [default: 10000]
      --no-service               Do not install the chain as a service: print the command that runs it, and wait for it
      --telegram                 Also answer on Telegram (the bot's token is asked for, or taken from TELEGRAM_BOT_TOKEN)
      --no-scheduler             Do not run the scheduler as a service
      --no-check                 Do not check that the model answers
  -y, --yes                      Ask nothing: take the defaults and these options
  -h, --help                     Print help
```

What it makes, step by step, and what it does when a step is done already:
[Run it on your computer](../getting-started/your-computer.md).

## `sion update`

```text
Replace this sion with the latest release, once its signature and SHA-256 are checked

Usage: sion update [OPTIONS]

Options:
      --check          Only say whether there is a newer release
      --node <NODE>    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --version <TAG>  This release rather than the latest (its tag, `v0.2.0`)
      --force          Install it even if it is not newer
  -h, --help           Print help
```

It installs a release only once its `SHA256SUMS` are signed by the release
key it was built with and its archive matches them: [Update](../getting-started/your-computer.md#update).

## `sion deploy`

```text
Put your agents on a server of yours over SSH, so that they work while this computer is off: sion copied there, set up there (or this machine's home copied, with --copy-home), and everything run by one service of yours (sion agent up)

Usage: sion deploy [OPTIONS] <DESTINATION>

Arguments:
  <DESTINATION>  The server: `user@host`, as SSH reaches it

Options:
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --port <PORT>          SSH's port on the server
      --identity <IDENTITY>  The key SSH signs in with
      --copy-home            Copy this machine's home (wallets and keys included) instead of running setup on the server
      --profile <PROFILE>    The profile whose work runner runs there (default: setup's)
      --webhooks             Take GitHub's webhooks in the work runner there
      --no-start             Install the service, but do not start it
      --no-setup             Do not run setup on the server (with no --copy-home: set it up there yourself)
  -h, --help                 Print help
```

It needs SSH to the server (`ssh`, or `SION_SSH`), and copies this `sion` only to the same kind of machine (Linux on the same processor); elsewhere, install a release there first. With `--copy-home`, the agents' keys go too: whoever controls the server controls their wallets (`deploy:copied-keys`). See [Run it on a server](../agent/server.md).

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
  confidential   What a workspace keeps confidential besides the paths you name (--confidential-file): where a confidential file went when a command or a patch moved it. Such a path stays hidden from commands in later sessions until it is forgotten here
  decisions      The decisions a session records: what was approved or declined, by whom, when, and whether a wallet signed it and its signature holds
  expected       What a session's agent said it expected of its calls (`run --expect`), against what they did: how many went as it said, and each that did not
  evolve         Evolve a profile's instructions (what follows the fixed system prompt) against an evaluation set: a mutator proposes variants for the cases its agent fails, each is scored under gates (size, new, no regression), and with --apply the best becomes the profile's
  sessions       The agent sessions on this machine; `search` finds an agent's past sessions by words
  trace          A session as one JSON line in chat format with tool calls, with its outcome: for review, evaluation or training
  judge          Judge a session's steps with a model: whether each moved the task on, stalled or did harm, and why. The judge is shown labelled steps first; with --calibrate, its agreement with labelled steps it was not shown is measured before it judges
  settle         The calls of a session whose outcome is unknown (a run stopped while they ran, or a service did not answer), and settling one once you know what it did: done, or not done (so that it may be made again)
  kill-switch    The kill switch: `on` stops every agent run on this machine before its next call; `off` lets them run again
  profile        Agent profiles: how to run an agent, under a name, for schedules and channels
  secret         The secrets skills' scripts may receive (`sion-secrets` in a skill's metadata): kept encrypted on this machine, passed to a script as environment variables, and cut out of what it prints
  web-identity   This machine's web identity: a key your agents' web requests are signed with, so that a site can tell whose agent asks (Web Bot Auth: HTTP Message Signatures, RFC 9421), and the key directory to publish for it
  credential     The credentials sion itself uses (models' keys and addresses, speech services' keys), kept sealed on this machine so that sion and its services find them without the environment; never given to skills' scripts
  login          Your logins agents' browsers may fill (`--login`): each bound to an origin, kept encrypted on this machine, filled only into fields of that origin, and never shown to the agent
  pairing        People a channel did not know who asked to talk to a profile: the codes waiting, approving one, letting a chat out again
  chatgpt        Sign in with ChatGPT, so that agents may run on your plan (`--model chatgpt:<model>`): OpenAI's sign-in for open-source apps, in your browser; the tokens kept encrypted on this machine
  weixin         Sign a WeChat account in for `--via weixin`, by a QR code scanned with WeChat (Tencent's iLink bot protocol), or sign it out
  service        Long-running commands (a bridge, the server, the scheduler) as services of your own: systemd user units on Linux, launchd agents on macOS
  mcp            MCP servers whose tools agents may use: add one (its tools are reviewed and pinned), list them, review one again, remove one
  graph          Graphs of agents: a profile's agent run as the nodes of a small graph (fan-out, work-check, draft-review, self-consistency, orchestrator, or one of your own), each node a session of its own
  schedule       Scheduled jobs: a task a profile works on at a time, every interval or on a cron expression, each run a session whose answer goes to a channel
  import         Bring an OpenClaw or Hermes Agent over to a profile: its persona and memories into the profile's instructions, its jobs as schedules, and with --models, --keys and --bridges its models, its model keys and its chats; its skills listed, to import through the audit. Shows the plan; --apply does it
  routes         Routes that choose a profile's models for each task: pictures, work items and long tasks by rule, other tasks sorted by a model among routes that say what they are for; what each route was given, and its models compared on a set of tasks
  improve        Improve the agent itself, keeping only what measurably helps: words added to its instructions, the models it reaches for, a vote, each tried on a set of tasks against the agent as it is and kept only when better beyond chance and failing no held-out task it passes
  immune         Attacks reported between operators: report the text an agent was attacked with, on a bond; vouch for a report; say whose reports your agents trust, and they hold back content from outside that holds the same text
  record         An agent's record, for whoever is asked to stand behind it or rely on it: what the ledger shows of what it may do and has done, which anyone can check, what its operator says of its sessions, and a warranty on a bond
  comply         Proof that payments kept your rules without showing the payments: commitments to each amount and payee, and zero-knowledge proofs that none was above a limit, that they came to no more than a total, and that each went to one of a list
  receipts       Receipts for which model answered: the model a session's runtime asked for each answer, at which address, and the model the provider said answered, in a chain; written for someone else to check, committed to in a claim, or signed by an enclave
  market         A market for agents' work, priced per accepted result: sealed bids in rounds, each cleared at one price anyone can recompute, and a Task Charter from each buyer to the worker it was paired with
  recorder       A flight recorder for an agent that acts in the world: a device's events in a log the ledger keeps append-only, recorded without a network and fixed by each commit, with extracts an investigator checks and bounds proven of readings that are not shown
  dataset        Datasets of checked work: sessions contributed by their owners on a dataset's terms, each with what checked the work in it, built into versions a trainer checks against the ledger
  exam           Sealed exams: tasks encrypted and committed to on the ledger, agents entered before a threshold of trustees opens them, and results that anyone can run again once they are open
  find           Find the best agent for a task among those you name: how well each one's offer fits the task (BM25), weighed against its record on the ledger, held to what you need (an enclave that checks out, a price)
  project        Projects a profile's agent owns, running for weeks: a goal, a budget its payments for the project are held to, milestones, and check-ins that report where it stands
  x-mentions     Answer the posts on X that mention your account, in their threads: each answer from the posts it replies to or quotes and the web search's results for its words, citing them, with their addresses in its last post; an answer that rests on nothing it was given is not posted. Needs X_ACCESS_TOKEN with tweet.read, tweet.write and users.read
  browser        Browser profiles you sign in with, for agents' browsers to use (--browser-profile): your cookies and sessions, kept here
  hooks          The operator's hooks: commands of your own run before and after agents' tool calls (a hook before may block one, with a reason the model is given) and before and after their runs
  rent           Rent a profile's agent out to other people by the token, in an enclave they can check, or rent someone else's
  sell           Sell an agent's work to other agents through the Agentic Commerce Protocol's checkout, paid in USD on this chain, and take their users' AP2 mandates
  buy            Buy from a merchant that serves ACP's checkout: make a checkout session, check its total, pay it on Siontology or by a card delegated to the merchant's PSP (with 3-D Secure if asked)
  commerce       The services around an ACP checkout: a PSP's delegate payment vault, a 3-D Secure provider, an agent's Feed API, a webhook receiver
  meeting        Take a meeting's notes: dial in by phone through Twilio (its number and keys) or hear it here (the microphone, and what this machine plays), the transcript written as it goes, and the profile's agent writing up the decisions, the action items and what is still open
  erc8004        The agent in ERC-8004's registries on EVM chains: its registration file, naming its Siontology DID; the calls that register it, bind its wallet, give it feedback and answer a validation, written out for a wallet to send
  c2pa           Content Credentials (C2PA) for what the agent makes: a manifest beside a file that binds its bytes and names the agent that made it, signed with your certificate; and checking one
  bench          Benchmark suites built in (payments over x402, attacks on an agent that pays, confirming before paying), runs of them with your models and stand-in tools, and the records of those runs
  prices         The models' prices that runs are counted at: Anthropic's list prices for Claude, built in, and yours from `$SION_HOME/prices.json` (an object of model names, each with `input`, `output` and optionally `cache_read` and `cache_write`, in dollars a million tokens), in their place where both price a model
  scheduler      Run scheduled jobs as they fall due
  work           Work a profile's agent takes on by itself: the GitHub repositories and chats it watches, who may ask it for work there, the kinds of work it may take on unasked (recorded only, until you let it act), and the runner that does the work
  waiting        Everything that waits for a person: work items' questions, approvals and failures, messages that could not be sent, sessions waiting for approval, and chats waiting to be let in
  up             Run everything on the list (the chain, bridges, the work runner, the scheduler) and keep it running, each started again when it stops; or change the list (init, add, remove, list)
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
          The model: anthropic:<model> (ANTHROPIC_API_KEY), openai:<model> (OPENAI_API_KEY; OpenAI's Responses API, or the chat completions of another server at OPENAI_BASE_URL), openai-chat:<model>, openai-responses:<model>, chatgpt:<model> (your ChatGPT plan, once signed in: `sion agent chatgpt login`), gemini:<model> (GEMINI_API_KEY), local:<model> (a model server on this machine or its network, SION_LOCAL_MODEL_URL), bedrock:<model> (AWS_BEARER_TOKEN_BEDROCK or AWS's access keys), vertex:<model> (GOOGLE_CLOUD_PROJECT and Google's sign-in), azure:<deployment> (AZURE_OPENAI_ENDPOINT, AZURE_OPENAI_API_KEY), openrouter, deepseek, moonshot, xai, groq, together, fireworks, mistral or cerebras:<model> (<NAME>_API_KEY) or script:<file>; how hard it thinks may follow a # (anthropic:claude-opus-5-5#high: none, minimal, low, medium, high, xhigh, max, or a budget of tokens); several keys, comma-separated, are used in turn, and keys may be kept on this machine (sion agent credential add). Repeat for fallbacks, in order
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
          Let it hand work to other agents: spawn subagents with budgets carved from its own (agent_spawn), hire through Task Charters (task_post, task_status), deliver tasks it was hired for (task_submit) and give up those it cannot complete (task_release)
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
      --terminal-timeout <SECONDS>
          The most seconds the agent may give a command in one of its terminals (terminal_run's timeout), for long builds and runs [default: 3600]
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
      --browser-profile <NAME>
          A browser profile you signed in with (`sion agent browser login`) for its browser to use: your cookies and sessions, so that it acts on those sites as you (`agent:signed-in-browser`)
      --computer [<SANDBOX>]
          Give it a desktop of its own, seen in screenshots and used with a pointer and a keyboard (Xvfb, xdotool and ImageMagick): in bubblewrap (`bwrap`, the default) or `docker:<image>@sha256:<digest>`, an image with those programs. Needs --workspace
      --computer-program <COMMAND>
          A program it may start on its desktop, as a command line (`firefox --new-instance`). Repeat for more
      --computer-size <SIZE>
          Its desktop's size, WIDTHxHEIGHT [default: 1280x800]
      --computer-no-code
          Use its desktop only with the pointer and the keyboard: no programs of its own run there (`run_code`, Python or bash, on the desktop's display, in its sandbox)
      --canvas
          Give it a canvas: a page on this machine where it shows you what it works on (A2UI's components: text, pictures, lists, cards, tabs, buttons, fields) as it goes, and hears what you press there. The page's address is in its first canvas_show
      --computer-network <NETWORK>
          With a Docker desktop, the Docker network its programs reach: one you set up (default: none) [default: none]
      --no-skill-approvals
          Ask before every call its autonomy does not cover, even those a skill it read pre-approves with `allowed-tools`
      --budget-tokens <TOKENS>
          Tokens each run may use, its subagents' included: before each model call, what it sends (estimated) and the most it may answer must fit in what is left, and when they do not, the run ends with a last answer, without tools, of what it did and what remains
      --budget-usd <DOLLARS>
          Dollars each run may spend, its subagents' included, at the models' prices (`sion agent prices`): before each model call, what it sends at the uncached price and the most it may answer must fit in what is left, as with --budget-tokens. Every model it may use needs a price
      --max-turns <TURNS>
          The most model turns a run may take (by default 32): it starts with 12 and gains 4 with each turn whose tool calls succeed
      --max-tokens <TOKENS>
          The most tokens each of the model's answers may have (by default 4,096), not counting room to think
      --compact-at <TOKENS>
          Compact the conversation once it reaches about this many tokens (by default about 40,000, or 36 messages): the middle is replaced by a summary that keeps, word for word, what the person said after the task
      --summary-model <MODEL>
          The model that writes that summary, or none for a mechanical one (by default the first --model, unless it is a script)
      --no-plan
          Give it no plan to keep (`plan_update`). With one, when it says it is done while steps are open, it is asked once to do them or drop them, saying why
      --no-change-check
          Let it finish after changing code without running anything since: otherwise, with a shell and no --verify, it is asked once to check the change first
      --expect
          Have it say, with a call, what it expects of it (every tool takes an `expect` input the runtime keeps to itself): where the call goes otherwise, it is told how, and the session keeps count
      --hear <SPEC>
          What hears the workspace's audio and videos, for `transcribe` (a video's sound is taken out in the sandbox, with --shell): openai:<model>, groq:<model>, deepgram:<model>, elevenlabs:<model>, whisper:<model file>, local:<model> or command:<program>
      --rules <FILE>
          A file of written rules (a refund policy, who may be sent what): each call that changes something is checked against them by a model first, and not made if it breaks one; a check that cannot be made holds the call too
      --rules-model <MODEL>
          The model that checks calls against the rules (by default the first --model that is not a script)
      --embed <SPEC>
          Search by meaning as well as by words, in its memory and in the documents of its workspace (`document_search`): openai:<model>, voyage:<model>, local:<model> (a server on this machine that speaks OpenAI's embeddings API, at SION_LOCAL_EMBED_URL, Ollama's by default) or command:<program>. Vectors are kept, so a text is embedded once
      --outside <PATH>
          A path of the workspace whose files come from outside your trust (mail saved there, a repository of others): what is read from it taints the session as a web page does. A file, a directory with a closing `/`, or a prefix ending in `*`; `downloads/` always is, and files sent in a chat are marked so. Repeat for more
      --always-ask <RULE>
          A call that always waits for you, whatever the autonomy level says, as `allowed-tools` rules are written: ledger_pay, shell(git push:*). Commands that cannot be undone (a forced push, `rm -rf` of the workspace) always wait. Repeat for more
      --no-ask
          Give it no `ask_person`: it decides what it cannot ask, and says what it assumed
      --quarantine
          Hold what tools return from outside out of the conversation: the agent reads it through a reader without tools, its answers held to a schema, and the session is not tainted by values
      --verify <COMMAND>
          Check the task each time the agent says it is done: run this command with /bin/sh in its sandbox, in its workspace (it needs --shell). An exit status of 0 passes. A failed check is shown to the agent, which goes on; after three, the run ends unverified, with its best answer (the one whose check printed the highest last line `score: <0 to 1>`, if any). The agent may also say that it cannot complete the task
      --verify-files <DIR>
          The check's own files (its tests, say): a directory apart from the workspace, shown to the check alone, read-only at /check, while it runs. The agent's commands neither see nor change it
      --shared-memory <LOG>
          A curated log to keep in mind, read-only, besides the agent's own memory (its id, or a unique prefix of it): one that only dreams write (`sion memory dream --into`). Repeat for more
      --confidential
          The task is confidential: the session holds confidential content from the start, so its external calls and spending wait for a person, and nothing allowed before runs them unasked
      --confidential-file <PATH>
          A path of the workspace whose content is confidential: a file, a directory with a closing `/`, or a prefix ending in `*`. What read_file reads from it holds the session as --confidential does. Repeat for more
      --confidential-tool <TOOL>
          A tool whose results are confidential, whatever they hold: its name, or the start of names ending in `*` (an MCP server's tools: `crm_*`). What it returns holds the session as --confidential does. Repeat for more
  -h, --help
          Print help
```

See [Your agent](../agent/index.md), [Commands, scripts and MCP servers](../agent/tools.md) for `--shell`, `--mcp` and `--storage`, and [A browser and a desktop](../agent/browser.md) for `--browser`, `--login` and `--computer`.

## `sion agent confidential`

```text
What a workspace keeps confidential besides the paths you name (--confidential-file): where a confidential file went when a command or a patch moved it. Such a path stays hidden from commands in later sessions until it is forgotten here

Usage: sion agent confidential [OPTIONS] <WORKSPACE>

Arguments:
  <WORKSPACE>  The workspace

Options:
      --forget <PATH>  Forget a path kept, as it is listed. Repeat for more
      --node <NODE>    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --forget-all     Forget every path kept
  -h, --help           Print help
```

## `sion agent decisions`

```text
The decisions a session records: what was approved or declined, by whom, when, and whether a wallet signed it and its signature holds

Usage: sion agent decisions [OPTIONS] <SESSION>

Arguments:
  <SESSION>  The session (its id, or a unique prefix of it)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent expected`

```text
What a session's agent said it expected of its calls (`run --expect`), against what they did: how many went as it said, and each that did not

Usage: sion agent expected [OPTIONS] <SESSION>

Arguments:
  <SESSION>  The session (its id, or a unique prefix of it)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent continue`

```text
Continue a session: approve or decline the call it waits on, or tell it more

Usage: sion agent continue [OPTIONS] <SESSION>

Arguments:
  <SESSION>  The session (its id, or a unique prefix of it)

Options:
      --approve            Approve the call it waits on
      --node <NODE>        The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --always [<RULE>]    With --approve: allow the same call again for the rest of the session, or, given a rule such as `shell(git status:*)`, the calls of its tool that the rule matches. Never a payment, never while the session is tainted, and never for a call whose argument comes from content held in quarantine
      --request <ID>       The request decided (as the session shows it): refused if it is not the call waiting
      --decline <DECLINE>  Decline the call it waits on, saying why
      --say <SAY>          Tell it something more and let it go on
      --sign <WALLET>      With --approve or --decline: sign the decision with this wallet's keys (one made on this machine, `sion keys new`), so that it can be checked later who decided (`sion agent decisions`)
  -h, --help               Print help
```

See [What it does alone, and what it asks](../agent/autonomy.md#approvals).

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
      --otel         Write its steps as OpenTelemetry spans (OTLP's JSON) instead: the session, each model asked and each call, with their times, tokens and outcomes
  -h, --help         Print help
```

## `sion agent settle`

```text
The calls of a session whose outcome is unknown (a run stopped while they ran, or a service did not answer), and settling one once you know what it did: done, or not done (so that it may be made again)

Usage: sion agent settle [OPTIONS] <SESSION> [KEY]

Arguments:
  <SESSION>  The session (its id, or a unique prefix of it)
  [KEY]      The call (its key, or a unique prefix of it); without it, the calls whose outcome is unknown are listed

Options:
      --done <WHAT>  It took effect: what it did, as the agent will be told
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --not-done     It did not take effect: it may be made again
  -h, --help         Print help
```

## `sion agent judge`

```text
Judge a session's steps with a model: whether each moved the task on, stalled or did harm, and why. The judge is shown labelled steps first; with --calibrate, its agreement with labelled steps it was not shown is measured before it judges

Usage: sion agent judge [OPTIONS] --model <MODEL> <SESSION>

Arguments:
  <SESSION>  The session (its id, or a unique prefix of it)

Options:
      --model <MODEL>  The model that judges
      --node <NODE>    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --calibrate      Measure the judge on labelled steps it was not shown, first
  -h, --help           Print help
```

See [Safety nets](../agent/checks.md#safety-nets).

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

Profiles are kept in `$SION_HOME/agents/profiles.json`. See [Profiles and schedules](../agent/schedules.md#profiles).

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
      --terminal-timeout <SECONDS>
          The most seconds the agent may give a command in one of its terminals (terminal_run's timeout), for long builds and runs [default: 3600]
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
      --browser-profile <NAME>
          A browser profile you signed in with (`sion agent browser login`) for its browser to use: your cookies and sessions, so that it acts on those sites as you (`agent:signed-in-browser`)
      --computer [<SANDBOX>]
          Give it a desktop of its own, seen in screenshots and used with a pointer and a keyboard (Xvfb, xdotool and ImageMagick): in bubblewrap (`bwrap`, the default) or `docker:<image>@sha256:<digest>`, an image with those programs. Needs --workspace
      --computer-program <COMMAND>
          A program it may start on its desktop, as a command line (`firefox --new-instance`). Repeat for more
      --computer-size <SIZE>
          Its desktop's size, WIDTHxHEIGHT [default: 1280x800]
      --computer-no-code
          Use its desktop only with the pointer and the keyboard: no programs of its own run there (`run_code`, Python or bash, on the desktop's display, in its sandbox)
      --canvas
          Give it a canvas: a page on this machine where it shows you what it works on (A2UI's components: text, pictures, lists, cards, tabs, buttons, fields) as it goes, and hears what you press there. The page's address is in its first canvas_show
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
      --budget-tokens <TOKENS>
          Tokens each run may use, its subagents' included: before each model call, what it sends (estimated) and the most it may answer must fit in what is left, and when they do not, the run ends with a last answer, without tools, of what it did and what remains
      --budget-usd <DOLLARS>
          Dollars each run may spend, its subagents' included, at the models' prices (`sion agent prices`). Every model it may use needs a price
      --max-turns <TURNS>
          The most model turns a run may take (by default 32)
      --max-tokens <TOKENS>
          The most tokens each of the model's answers may have (by default 4,096), not counting room to think
      --compact-at <TOKENS>
          Compact a conversation once it reaches about this many tokens (by default about 40,000, or 36 messages): the middle is replaced by a summary
      --summary-model <MODEL>
          The model that writes the summary when a conversation is compacted, or none for a mechanical one (by default the first model)
      --no-plan
          Give it no plan to keep (`plan_update`), nor hold it to one
      --no-change-check
          Let it finish after changing code without running anything since
      --expect
          Have it say, with a call, what it expects of it: where the call goes otherwise it is told how, and `sion agent improve` is shown what it did not expect
      --hear <SPEC>
          What hears its workspace's audio and videos (`transcribe`), as voice notes are heard: openai:<model>, groq:<model>, deepgram:<model>, elevenlabs:<model>, whisper:<model file>, local:<model> or command:<program>
      --rules <FILE>
          A file of written rules (a refund policy, who may be sent what): each call that changes something is checked against them by a model first, and not made if it breaks one. Read now: add the profile again after changing them
      --rules-model <MODEL>
          The model that checks calls against the rules (by default the first that is not a script)
      --embed <SPEC>
          Search by meaning as well as by words, in its memory and its workspace's documents: openai:<model>, voyage:<model>, local:<model> or command:<program>
      --outside <PATH>
          A path of its workspace whose files come from outside: what is read from it taints the session as a web page does. Repeat for more
      --always-ask <RULE>
          A call that always waits for a person, whatever the autonomy level says, as `allowed-tools` rules are written: ledger_pay, shell(git push:*). Repeat for more
      --quarantine
          Hold what tools return from outside out of the conversation: the agent reads it through a reader without tools, its answers held to a schema, and the session is not tainted by values
      --shared-memory <LOG>
          A curated log its sessions keep in mind, read-only (its id, or a unique prefix of it). Repeat for more
      --confidential
          Its sessions hold confidential content from the start: their external calls and spending wait for a person
      --confidential-file <PATH>
          A path of its workspace whose content is confidential (a file, a directory with a closing `/`, or a prefix ending in `*`). Repeat for more
      --confidential-tool <TOOL>
          A tool whose results are confidential, whatever they hold: its name, or the start of names ending in `*`. Repeat for more
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

## `sion agent web-identity`

```text
This machine's web identity: a key your agents' web requests are signed with, so that a site can tell whose agent asks (Web Bot Auth: HTTP Message Signatures, RFC 9421), and the key directory to publish for it

Usage: sion agent web-identity [OPTIONS] <COMMAND>

Commands:
  new     Make this machine's web identity: an Ed25519 key, kept sealed here, that your agents' web requests (fetch_url, fetch_paid) are signed with from then on, as Web Bot Auth has it (HTTP Message Signatures, RFC 9421). Prints the key directory to publish
  show    The identity: its key's id, where it is published, and the key directory to publish there
  remove  Remove the identity: requests are no longer signed, and its key is gone for good
  help    Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent web-identity new`

```text
Make this machine's web identity: an Ed25519 key, kept sealed here, that your agents' web requests (fetch_url, fetch_paid) are signed with from then on, as Web Bot Auth has it (HTTP Message Signatures, RFC 9421). Prints the key directory to publish

Usage: sion agent web-identity new [OPTIONS] --published-at <URL>

Options:
      --node <NODE>         The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --published-at <URL>  The HTTPS address you will publish the key directory at: sites fetch it to check a signature, and it is what names you to them. The well-known place is https://<your host>/.well-known/http-message-signatures-directory
      --replace             Replace the identity there is: its key is gone for good, and sites that knew it no longer know your agents
  -h, --help                Print help
```

## `sion agent web-identity show`

```text
The identity: its key's id, where it is published, and the key directory to publish there

Usage: sion agent web-identity show [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent web-identity remove`

```text
Remove the identity: requests are no longer signed, and its key is gone for good

Usage: sion agent web-identity remove [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent credential`

```text
The credentials sion itself uses (models' keys and addresses, speech services' keys), kept sealed on this machine so that sion and its services find them without the environment; never given to skills' scripts

Usage: sion agent credential [OPTIONS] <COMMAND>

Commands:
  add     Keep a model's or a speech service's credential on this machine: its value is read from standard input (not shown when typed), or taken from an environment variable
  list    The names kept (never their values)
  remove  Forget one
  help    Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

What `sion` itself uses (models' keys and addresses, speech services'
keys), kept sealed in `$SION_HOME/agents/credentials.bin`; the environment
wins over what is kept (`agent:kept-credentials`).

## `sion agent credential add`

```text
Keep a model's or a speech service's credential on this machine: its value is read from standard input (not shown when typed), or taken from an environment variable

Usage: sion agent credential add [OPTIONS] <NAME>

Arguments:
  <NAME>  Its name: ANTHROPIC_API_KEY, OPENAI_API_KEY, OPENAI_BASE_URL, GEMINI_API_KEY and the like

Options:
      --from-env <VARIABLE>  Take the value from this environment variable instead
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help                 Print help
```

## `sion agent credential list`

```text
The names kept (never their values)

Usage: sion agent credential list [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent credential remove`

```text
Forget one

Usage: sion agent credential remove [OPTIONS] <NAME>

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

See [A browser and a desktop](../agent/browser.md#logins).

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

## `sion agent browser`

```text
Browser profiles you sign in with, for agents' browsers to use (--browser-profile): your cookies and sessions, kept here

Usage: sion agent browser [OPTIONS] <COMMAND>

Commands:
  login   Sign in with a browser profile for agents to use: Chromium opens in a window of its own; sign in to what the agent should reach, then close the window
  list    List the browser profiles
  remove  Remove a browser profile, and everything it was signed in to here
  help    Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

See [Signed in as you](../agent/browser.md#signed-in-as-you).

## `sion agent browser login`

```text
Sign in with a browser profile for agents to use: Chromium opens in a window of its own; sign in to what the agent should reach, then close the window

Usage: sion agent browser login [OPTIONS] <NAME>

Arguments:
  <NAME>  The profile's name (made if new)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --url <URL>    The page to open first [default: about:blank]
  -h, --help         Print help
```

## `sion agent browser list`

```text
List the browser profiles

Usage: sion agent browser list [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent browser remove`

```text
Remove a browser profile, and everything it was signed in to here

Usage: sion agent browser remove [OPTIONS] <NAME>

Arguments:
  <NAME>  The profile's name

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
[Pairing and voice notes](../agent/voice.md#pairing-and-voice-notes).

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

The account is the one you sign in with in your browser, on OpenAI's page; the bridge, the server and schedules use it like any model (`--model chatgpt:<model>`). See [Signing in with ChatGPT](../agent/models.md#signing-in-with-chatgpt).

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

WeChat's iLink bot protocol is published only as Tencent's plugin's code and others' descriptions of it; the bridge has been run only against a stand-in built from them. See [WeChat](../agent/messengers.md#wechat).

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

## `sion agent up`

```text
Run everything on the list (the chain, bridges, the work runner, the scheduler) and keep it running, each started again when it stops; or change the list (init, add, remove, list)

Usage: sion agent up [OPTIONS] [COMMAND]

Commands:
  init    Fill the list from what `sion setup` made: the chain, the scheduler, Telegram if it was set up, and the work runner of the profile
  add     Add something to run: a name, and the arguments of `sion` after `--` (e.g. `sion agent up add slack -- agent bridge me --via slack --pairing`)
  remove  Take something off the list
  list    What the list holds, and how each one runs
  rehome  Rewrite the paths a home copied from another machine holds (profiles' workspaces and skills, setup's record), from that machine's home to this one's
  help    Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>         The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --for-secs <SECONDS>  Run for this many seconds, then stop everything (for trying it)
  -h, --help                Print help
```

What it runs is the list in `$SION_HOME/up.json`; what each said goes to `$SION_HOME/logs/<name>.log`, and `$SION_HOME/up.status.json` says what runs. Each is started again when it stops, after 1 s, then 2, 4… up to a minute, and the pause starts over once it has run steadily for a minute. When `sion agent up` stops, however it stops, what it ran stops too. See [Run it on a server](../agent/server.md).

## `sion agent up init`

```text
Fill the list from what `sion setup` made: the chain, the scheduler, Telegram if it was set up, and the work runner of the profile

Usage: sion agent up init [OPTIONS]

Options:
      --node <NODE>        The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --profile <PROFILE>  The profile whose work runner runs (default: setup's)
      --webhooks           Take GitHub's webhooks in the work runner (GITHUB_WEBHOOK_SECRET)
  -h, --help               Print help
```

The chain (if setup made one here), the scheduler, the work runner for the profile (with `--webhooks`, taking GitHub's webhooks), and a bridge for each messenger setup connected.

## `sion agent up add`

```text
Add something to run: a name, and the arguments of `sion` after `--` (e.g. `sion agent up add slack -- agent bridge me --via slack --pairing`)

Usage: sion agent up add [OPTIONS] <NAME> -- <ARGS>...

Arguments:
  <NAME>     Its name: letters, digits, `-` and `_`
  <ARGS>...  The arguments of `sion`

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent up remove`

```text
Take something off the list

Usage: sion agent up remove [OPTIONS] <NAME>

Arguments:
  <NAME>  Its name

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent up list`

```text
What the list holds, and how each one runs

Usage: sion agent up list [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent up rehome`

```text
Rewrite the paths a home copied from another machine holds (profiles' workspaces and skills, setup's record), from that machine's home to this one's

Usage: sion agent up rehome [OPTIONS] --from <FROM>

Options:
      --from <FROM>  The home directory on the machine the files came from
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

For a home copied from another machine (`sion deploy --copy-home` runs it): paths that name the other machine's home are rewritten to this one's.

## `sion agent mcp`

```text
MCP servers whose tools agents may use: add one (its tools are reviewed and pinned), list them, review one again, remove one

Usage: sion agent mcp [OPTIONS] <COMMAND>

Commands:
  add        Add an MCP server for agents' sessions, and review its tools. The server is started and its tools listed, each with what its calls are taken to do; those offered are pinned by a digest of their definitions, so a tool that changes is not offered until reviewed again
  list       The MCP servers on this machine, with their reviewed tools
  review     Review a server's tools again: what changed, what is new and what is gone since its last review. With --accept, its tools are pinned as they are now (a tool keeps the larger of its old effect and what its annotations now say, unless --effect sets one)
  login      Log in to a server over HTTP that asks for authorization (OAuth): a browser opens where you authorize, and the tokens are kept, encrypted, for its sessions, which renew them while they can
  logout     Forget a server's authorization (its tokens and registration), revoking its tokens first where its authorization server takes revocations
  remove     Remove a server, and its authorization as `logout` does
  sampling   Let a server have the session's model write for it (sampling), within limits: at most --max-tokens a completion and --per-run completions a run, written by --model (default: the session's first). The completions are counted in the run's token budget. With --off, it may not. Sampling is deprecated by protocol 2026-07-28
  resources  The resources a server offers, and its templates; with --read, what one holds
  prompts    The prompts a server offers, which people run in chats with `/prompt <server> <prompt> name=value …`
  help       Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

Servers are kept in `$SION_HOME/agents/mcp.json`, with a digest of each reviewed tool's definition. See [Commands, scripts and MCP servers](../agent/tools.md#tools-from-mcp-servers).

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
      --first-party <SERVER>
          One of sion's own servers instead of a program: mail (MAIL_…), calendar (CALDAV_…) or documents (the folder --dir names). The program and the variables it needs are filled in
  -h, --help
          Print help
```

The server is started (or, with `--url`, reached over HTTP), introduced to (protocol 2026-07-28, or the handshake of 2025-11-25 and before) and asked for its tools, then stopped. A tool that says nothing of itself is taken to reach the open world: its calls are external and what it returns taints the session. Without `--sandbox`, the server runs as you (`agent:mcp-unsandboxed`). A server over HTTP that answers `401` is logged in to first, as `sion agent mcp login` does; see [Servers over HTTP](../agent/tools.md#servers-over-http).

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

The server's protected resource metadata names where to authorize; the client registered beforehand, named by a client ID metadata document, or registered then, is kept for later logins; tokens are kept encrypted beside the secrets. A browser on another machine cannot come back to this one: paste the address it was sent to at the terminal. See [Logging in to a server](../agent/tools.md#logging-in-to-a-server).

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

## `sion agent mcp sampling`

```text
Let a server have the session's model write for it (sampling), within limits: at most --max-tokens a completion and --per-run completions a run, written by --model (default: the session's first). The completions are counted in the run's token budget. With --off, it may not. Sampling is deprecated by protocol 2026-07-28

Usage: sion agent mcp sampling [OPTIONS] <NAME>

Arguments:
  <NAME>  The server

Options:
      --max-tokens <MAX_TOKENS>  The most tokens one completion may write [default: 1000]
      --node <NODE>              The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --per-run <PER_RUN>        The most completions in one run of a session [default: 5]
      --model <MODEL>            The model that writes them
      --off                      Take sampling away
  -h, --help                     Print help
```

Off by default. See [What MCP servers ask for](../agent/tools.md#what-mcp-servers-ask-for).

## `sion agent mcp resources`

```text
The resources a server offers, and its templates; with --read, what one holds

Usage: sion agent mcp resources [OPTIONS] <NAME>

Arguments:
  <NAME>  The server

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --read <URI>   The address of a resource to read
  -h, --help         Print help
```

A session reaches a server's resources through two tools, `<server>_resources` and `<server>_read_resource`; what they return is from outside. See [What MCP servers ask for](../agent/tools.md#what-mcp-servers-ask-for).

## `sion agent mcp prompts`

```text
The prompts a server offers, which people run in chats with `/prompt <server> <prompt> name=value …`

Usage: sion agent mcp prompts [OPTIONS] <NAME>

Arguments:
  <NAME>  The server

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

See [What MCP servers ask for](../agent/tools.md#what-mcp-servers-ask-for).

## `sion agent graph`

```text
Graphs of agents: a profile's agent run as the nodes of a small graph (fan-out, work-check, draft-review, self-consistency, orchestrator, or one of your own), each node a session of its own

Usage: sion agent graph [OPTIONS] <COMMAND>

Commands:
  templates  The graphs the runtime knows, a line each; with a name, that graph as JSON, to start a graph of your own from
  run        Run a graph on a task, each of its nodes a session of a profile's agent: a template, a graph of your own, or what the profile's model says the task needs
  eval       Compare ways of working through a set of tasks, with a profile's models: one agent, one agent several times with its answers put to a vote, and each graph given, on the same token budget for a case, each case run several times, with the cases' stand-in tools and checks. A graph that beats both (by Newcombe's 95% interval for the difference of pass rates) earns the profile's traffic: `run --route` offers only templates that have
  show       Show a graph's run: what each node answered, and how it stands
  continue   Go on with a graph's run that stopped at nodes whose sessions waited for a person, once those sessions were approved and run on (`sion agent continue <session> --approve`)
  replay     Replay a graph's run from what it recorded: each node answered as it was, the graph (as it is kept with the run) must ask the same and route the answers the same way, to the same end
  help       Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

See [Graphs of agents](../agent/graphs.md).

## `sion agent graph templates`

```text
The graphs the runtime knows, a line each; with a name, that graph as JSON, to start a graph of your own from

Usage: sion agent graph templates [OPTIONS] [NAME]

Arguments:
  [NAME]  A template's name

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

See [Graphs of agents](../agent/graphs.md).

## `sion agent graph run`

```text
Run a graph on a task, each of its nodes a session of a profile's agent: a template, a graph of your own, or what the profile's model says the task needs

Usage: sion agent graph run [OPTIONS] --task <TASK> <PROFILE>

Arguments:
  <PROFILE>  The profile whose agent runs the nodes

Options:
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --task <TASK>          The task
      --template <TEMPLATE>  A template: fan-out, work-check, draft-review, self-consistency, orchestrator, attempts or research
      --graph <GRAPH>        A graph of your own: a JSON file
      --route                Ask the profile's model first how to work on the task: one agent (a single session), or one of the templates that beat one agent in the profile's comparisons (`sion agent graph eval`)
      --untested             With --route, let the router choose any template, measured or not
      --check <COMMAND>      A command that checks each run of a node that works in a workspace of its own (the attempts), there, as --verify checks a session: what it says goes to the judge. The profile needs a shell
  -h, --help                 Print help
```

See [Graphs of agents](../agent/graphs.md).

## `sion agent graph show`

```text
Show a graph's run: what each node answered, and how it stands

Usage: sion agent graph show [OPTIONS] <RUN>

Arguments:
  <RUN>  The run, or the start of its id

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent graph continue`

```text
Go on with a graph's run that stopped at nodes whose sessions waited for a person, once those sessions were approved and run on (`sion agent continue <session> --approve`)

Usage: sion agent graph continue [OPTIONS] <RUN>

Arguments:
  <RUN>  The run, or the start of its id

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent graph replay`

```text
Replay a graph's run from what it recorded: each node answered as it was, the graph (as it is kept with the run) must ask the same and route the answers the same way, to the same end

Usage: sion agent graph replay [OPTIONS] <RUN>

Arguments:
  <RUN>  The run, or the start of its id

Options:
      --graph <FILE>  Replay it through this graph instead (a JSON file): to see whether a changed graph would have routed the run the same way
      --node <NODE>   The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help          Print help
```

## `sion agent graph eval`

```text
Compare ways of working through a set of tasks, with a profile's models: one agent, one agent several times with its answers put to a vote, and each graph given, on the same token budget for a case, each case run several times, with the cases' stand-in tools and checks. A graph that beats both (by Newcombe's 95% interval for the difference of pass rates) earns the profile's traffic: `run --route` offers only templates that have

Usage: sion agent graph eval [OPTIONS] --set <FILE> <PROFILE>

Arguments:
  <PROFILE>  The profile whose models do the work

Options:
      --node <NODE>                    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --set <FILE>                     The set of tasks: a JSON file of cases, as an evaluation set's
      --template <NAME>                A template to compare (repeat for more)
      --graph <FILE>                   A graph of your own to compare: a JSON file (repeat for more)
      --samples <SAMPLES>              How many times one agent works on a case for the vote [default: 3]
      --runs <RUNS>                    How many times each case is run by each way of working [default: 5]
      --budget-tokens <BUDGET_TOKENS>  The tokens each way of working may use on a case, its sessions together [default: 30000]
      --judge <MODEL>                  A model that judges each run step by step, its agreement with labelled steps it was not shown measured first
  -h, --help                           Print help
```

See [Graphs of agents](../agent/graphs.md).

## `sion agent bench`

```text
Benchmark suites built in (payments over x402, attacks on an agent that pays, confirming before paying), runs of them with your models and stand-in tools, and the records of those runs

Usage: sion agent bench [OPTIONS] <COMMAND>

Commands:
  list     The suites built in, a line each
  show     A suite as JSON: one built in by name, to read or start one of your own from
  run      Run a suite with your models: each case several times, each run with fresh models and the case's stand-in tools (nothing is paid or sent but the model calls), and report the pass rate, pass^k, the tokens and the cost. The run is kept, with its log
  runs     The runs kept, newest first
  publish  Publish a kept run on the ledger, as a claim signed by your wallet: the suite and its digest, the models, the pass rate, pass^k, the tokens and the cost, and the digest of the run's log, which you give to whoever is to check it
  check    Check a published run: the claim on the ledger, who signed it and when, and, given its log, that the log is the one it names, that every run in it judged again by the suite's checks comes out as it says, and that the figures are the claim's
  help     Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent bench list`

```text
The suites built in, a line each

Usage: sion agent bench list [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent bench show`

```text
A suite as JSON: one built in by name, to read or start one of your own from

Usage: sion agent bench show [OPTIONS] <SUITE>

Arguments:
  <SUITE>  The suite's name

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent bench run`

```text
Run a suite with your models: each case several times, each run with fresh models and the case's stand-in tools (nothing is paid or sent but the model calls), and report the pass rate, pass^k, the tokens and the cost. The run is kept, with its log

Usage: sion agent bench run [OPTIONS] <SUITE>

Arguments:
  <SUITE>  A suite built in, or a JSON file of one (a set of tasks, as `sion agent graph eval` takes)

Options:
      --model <MODEL>          A model to run with, in order of preference (repeat for more)
      --node <NODE>            The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --profile <PROFILE>      Run with a profile's models instead
      --repeats <REPEATS>      How many times each case is run (pass^k is reported for k this) [default: 3]
      --max-turns <MAX_TURNS>  The most turns a run may take
  -h, --help                   Print help
```

## `sion agent bench runs`

```text
The runs kept, newest first

Usage: sion agent bench runs [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent bench publish`

```text
Publish a kept run on the ledger, as a claim signed by your wallet: the suite and its digest, the models, the pass rate, pass^k, the tokens and the cost, and the digest of the run's log, which you give to whoever is to check it

Usage: sion agent bench publish [OPTIONS] --wallet <WALLET> <RUN>

Arguments:
  <RUN>  The run: its directory, or the start of its name (`sion agent bench runs`)

Options:
      --node <NODE>      The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --wallet <WALLET>  The wallet that signs the claim
  -h, --help             Print help
```

## `sion agent bench check`

```text
Check a published run: the claim on the ledger, who signed it and when, and, given its log, that the log is the one it names, that every run in it judged again by the suite's checks comes out as it says, and that the figures are the claim's

Usage: sion agent bench check [OPTIONS] <CLAIM>

Arguments:
  <CLAIM>  The claim's id

Options:
      --log <FILE>    The run's log (by default, a run kept here with the log the claim names)
      --node <NODE>   The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --suite <FILE>  The suite, if it is not one built in
  -h, --help          Print help
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

Jobs are kept in `$SION_HOME/agents/schedules.json`, with their latest runs. See [Profiles and schedules](../agent/schedules.md#schedules).

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

## `sion agent import`

```text
Bring an OpenClaw or Hermes Agent over to a profile: its persona and memories into the profile's instructions, its jobs as schedules, and with --models, --keys and --bridges its models, its model keys and its chats; its skills listed, to import through the audit. Shows the plan; --apply does it

Usage: sion agent import [OPTIONS] --profile <PROFILE> <FROM>

Arguments:
  <FROM>  What to import from: openclaw or hermes

Options:
      --node <NODE>        The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --profile <PROFILE>  The profile that receives it
      --home <DIR>         Its home (default: ~/.openclaw, or OPENCLAW_STATE_DIR; ~/.hermes, or HERMES_HOME; a Hermes profile's own folder for one of its profiles)
      --agent <AGENT>      OpenClaw's agent to import (default: the only one, or main)
      --jobs <FILE>        OpenClaw's jobs, as `openclaw automations list --all --json` prints them
      --apply              Do it; without this, the plan is shown and nothing changes
      --models             With --apply: give the profile the models the import names
      --keys               With --apply: keep the model keys it found on this machine, sealed
      --bridges            With --apply: install a service for each chat bridge it found, not started, its credentials in the service's own environment file
      --accept-risks       With --apply: bring it over despite the findings the plan says hold it back, once you have read them
  -h, --help               Print help
```

See [Coming from OpenClaw or Hermes](../agent/import.md).

## `sion agent hooks`

```text
The operator's hooks: commands of your own run before and after agents' tool calls (a hook before may block one, with a reason the model is given) and before and after their runs

Usage: sion agent hooks [OPTIONS] <COMMAND>

Commands:
  add     Add a hook: the command after `--` runs at the event, reading it as JSON on its standard input
  list    List the hooks, numbered
  remove  Remove a hook, by its number in the list
  help    Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

See [Hooks around calls](../agent/tools.md#hooks-around-calls).

## `sion agent hooks add`

```text
Add a hook: the command after `--` runs at the event, reading it as JSON on its standard input

Usage: sion agent hooks add [OPTIONS] --event <EVENT> -- <COMMAND>...

Arguments:
  <COMMAND>...
          The command and its arguments

Options:
      --event <EVENT>
          When it runs

          Possible values:
          - before-tool: Before a tool call runs: it may block it
          - after-tool:  After a tool call ran: it may add a note to its result
          - run-start:   Before a run of a session: it may add a note to the person's message
          - run-end:     After a run of a session

      --node <NODE>
          The node's API
          
          [env: SION_NODE=]
          [default: http://127.0.0.1:8730]

      --tool <PATTERN>
          For tool events, a tool it is for, by a pattern of names (`*` for any characters). Repeat for more; none, every tool

      --timeout <TIMEOUT>
          Seconds before it is stopped
          
          [default: 30]

  -h, --help
          Print help (see a summary with '-h')
```

## `sion agent hooks list`

```text
List the hooks, numbered

Usage: sion agent hooks list [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent hooks remove`

```text
Remove a hook, by its number in the list

Usage: sion agent hooks remove [OPTIONS] <NUMBER>

Arguments:
  <NUMBER>  Its number

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent routes`

```text
Routes that choose a profile's models for each task: pictures, work items and long tasks by rule, other tasks sorted by a model among routes that say what they are for; what each route was given, and its models compared on a set of tasks

Usage: sion agent routes [OPTIONS] <COMMAND>

Commands:
  set     Give a profile a route, or change one: tasks of one kind go to its models first
  remove  Remove a route from a profile
  sorter  The model that sorts a profile's tasks among its routes (default: its first model); `default` to go back to that
  show    A profile's routes, and what each was given: sessions, how their first runs ended, tokens, by the model first in line
  test    Which route a task would take, and why (the sorter is asked; nothing runs)
  eval    Compare a route's models on a set of tasks, each as one agent on the same cases and budget: a model whose pass rate beats the route's first by Newcombe's 95% interval earns first place in the route
  help    Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

See [A model for each task](../agent/models.md#a-model-for-each-task).

## `sion agent routes set`

```text
Give a profile a route, or change one: tasks of one kind go to its models first

Usage: sion agent routes set [OPTIONS] --model <MODEL> <PROFILE> <NAME>

Arguments:
  <PROFILE>
          The profile

  <NAME>
          The route's name (lowercase letters, digits and hyphens)

Options:
      --model <MODEL>
          Its models, in order of preference (repeat for more)

      --node <NODE>
          The node's API
          
          [env: SION_NODE=]
          [default: http://127.0.0.1:8730]

      --about <ABOUT>
          The tasks it is for, in words: the sorter reads this
          
          [default: ""]

      --when <WHEN>
          What sends a task to it without asking the sorter (repeat for more)

          Possible values:
          - pictures: The task carries pictures
          - work:     The task is a work item the agent took on ([`crate::work`])
          - long:     The task is longer than the route's `longer_than` characters ([`LONG_CHARS`] by default)

      --longer-than <LONGER_THAN>
          With --when long: the characters past which a task is long (default 40000)

  -h, --help
          Print help (see a summary with '-h')
```

## `sion agent routes remove`

```text
Remove a route from a profile

Usage: sion agent routes remove [OPTIONS] <PROFILE> <NAME>

Arguments:
  <PROFILE>  The profile
  <NAME>     The route

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent routes sorter`

```text
The model that sorts a profile's tasks among its routes (default: its first model); `default` to go back to that

Usage: sion agent routes sorter [OPTIONS] <PROFILE> <MODEL>

Arguments:
  <PROFILE>  The profile
  <MODEL>    The model, or `default`

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent routes show`

```text
A profile's routes, and what each was given: sessions, how their first runs ended, tokens, by the model first in line

Usage: sion agent routes show [OPTIONS] <PROFILE>

Arguments:
  <PROFILE>  The profile

Options:
      --last <LAST>  How many of the latest decisions to list [default: 10]
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent routes test`

```text
Which route a task would take, and why (the sorter is asked; nothing runs)

Usage: sion agent routes test [OPTIONS] <PROFILE> <TASK>

Arguments:
  <PROFILE>  The profile
  <TASK>     The task, in words

Options:
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --pictures <PICTURES>  How many pictures the task would carry [default: 0]
      --work                 The task as a work item
  -h, --help                 Print help
```

## `sion agent routes eval`

```text
Compare a route's models on a set of tasks, each as one agent on the same cases and budget: a model whose pass rate beats the route's first by Newcombe's 95% interval earns first place in the route

Usage: sion agent routes eval [OPTIONS] --set <FILE> <PROFILE> <ROUTE>

Arguments:
  <PROFILE>  The profile
  <ROUTE>    The route

Options:
      --node <NODE>                    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --set <FILE>                     The set of tasks: a JSON file of cases, as an evaluation set's
      --runs <RUNS>                    How many times each case is run by each model [default: 5]
      --budget-tokens <BUDGET_TOKENS>  The tokens each model may use on a case [default: 30000]
  -h, --help                           Print help
```

## `sion agent improve`

```text
Improve the agent itself, keeping only what measurably helps: words added to its instructions, the models it reaches for, a vote, each tried on a set of tasks against the agent as it is and kept only when better beyond chance and failing no held-out task it passes

Usage: sion agent improve [OPTIONS] <COMMAND>

Commands:
  try       Try changes to a profile's agent on a set of tasks: words added to its instructions, the models a route reaches for, a vote. Each is run against the agent as it is and kept only if it is better beyond chance (Newcombe's 95% interval above zero) and fails no held-out task the agent passes; what is kept is recorded with its evidence
  kept      The changes a profile's agent has kept, each with the evidence for keeping it
  mark      Say how one of the agent's sessions went, so that it can be learnt from (`--good` or `--bad`); a session whose check passed needs none
  harvest   Turn the profile's sessions whose outcome is known (marked, or their check passed) into a set of tasks: each task asked again, of stand-ins that answer what its tools answered, checked against what the session did
  export    Export what the profile's work gives for training an open model on it: its sessions that went well as examples to fine-tune on (sft.jsonl, OpenAI's chat format with tool calls and the tools shown), and its tasks as environments to reinforce in (environments.jsonl, tasks.json: sion agent improve env runs one). Nothing sealed is exported, and nothing exported is sealed later
  env       Run one task as an episode for reinforcement learning, the policy outside: each request on standard output, in OpenAI's chat format ({"request": {"messages", "tools", "max_tokens"}}), the policy's turn on standard input ({"message": {...}}), and the reward last ({"done": {"reward", "passed", "failures"}}), the share of the task's checks that held
  seal      Seal a set of tasks for the profile: a share held apart to confirm changes on, which no proposer is shown, with canary cases among them and a number of confirmations (with --reusable, a reusable holdout instead). With --wallet, the seal's digests are published on the ledger before anything is tried on it
  run       Let the agent improve itself: a proposer model writes changes from the tasks it failed among the seal's tasks to choose on; each is tried there, and kept only when confirmed better on the sealed tasks. With --apply a kept change becomes the profile's; with --wallet each is published on the ledger
  practice  Have a model write practice tasks for the profile, like the seal's tasks to choose on (it is never shown the sealed ones): each kept only when its own solution passes it and its failures fail it, when it is no sealed task or task to choose on over again, and when the agent as it is does not pass it every time. `run --practice FILE` chooses changes on them beside the seal's
  lineage   The changes the agent kept from its own work, with their evidence; with --check, each held to its claim on the ledger
  adopt     Make a kept change the profile's
  undo      Take back the last change made the profile's
  share     Share a lesson the profile's agent kept, as your claim: its words and the evidence it was kept on, on your bond `lesson-author`, which the adjudicator can rule against. Other owners' agents find it (`lessons`) and keep it only when their own sealed tasks confirm it (`learn`)
  lessons   The lessons shared on this ledger, each with the evidence its author kept it on and whose agents have kept it since
  learn     Have the profile's agent try lessons other owners shared. Each is added to what it has learned and kept only when that is better beyond chance on the profile's tasks to choose on and then on its sealed tasks, like any change; no model proposes anything. With --apply a kept lesson becomes the profile's; with --wallet it is published, with a claim saying whose lesson it was
  code      Let the agent change its own code: a coding agent edits a copy of the source, its commands in the sandbox; a change that keeps to the paths it may touch, builds, passes its tests, is better on the seal's tasks to choose on and is confirmed on the sealed tasks, the candidate scored by this harness, becomes a release (published with --wallet). Nothing is installed
  attest    Rebuild and score a release someone published: the source at its commit with its patch, and as it stood, each built and tested in the sandbox and scored by this harness on the sealed tasks it was confirmed on; publish what you found if it holds up
  install   Apply a release to a checkout of the source, only if its patch is the one published and verifiers you name attest it (or, with --undo, take it back out). Build and install the result as you build sion
  improver  Improve the improver: the proposer's instructions and settings, graded by how much the agent improves on tasks it was not shown when the improver writes its changes, over problems (pairs of task sets). The improver proposes its own changes, under its own instructions; one is kept only when the agent improves more with it, beyond chance, on the problems given and then on problems held out, which no proposer is shown. With --show, the improver as it is
  help      Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

Each change is tried on its own against the agent as it is, on the same cases and budget; what is kept is recorded in `$SION_HOME/agents/routes/<profile>-improvements.jsonl`. See [Improving the agent, not only a skill](../agent/skill-improvement.md#improving-the-agent-not-only-a-skill).

## `sion agent improve try`

```text
Try changes to a profile's agent on a set of tasks: words added to its instructions, the models a route reaches for, a vote. Each is run against the agent as it is and kept only if it is better beyond chance (Newcombe's 95% interval above zero) and fails no held-out task the agent passes; what is kept is recorded with its evidence

Usage: sion agent improve try [OPTIONS] --tasks <FILE> --held-out <FILE> <PROFILE>

Arguments:
  <PROFILE>  The profile

Options:
      --node <NODE>                    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --tasks <FILE>                   The tasks the changes are tried on: a set of tasks, as `sion agent routes eval --set` takes (a name and cases with checks)
      --held-out <FILE>                Tasks held out: none of the changes is chosen on them, and a change that fails one the agent passes is turned down
      --instructions <TEXT>            Words to add after what the operator tells it (repeat to try several, each on its own)
      --route <ROUTE=MODELS>           Models to try, best first: `<route>=<model>,<model>`, the route `default` for the profile's own models (repeat to try several)
      --vote <SAMPLES>                 Answer by a vote of this many runs, the answer given most often winning (2 to 9; repeat to try several)
      --from <FILE>                    Changes to try from a file: a JSON list of `{"instructions": {"words": …}}`, `{"route": {"task": …, "models": […]}}` and `{"working": {"arm": "vote-3"}}`
      --runs <RUNS>                    How many times each task is run by each way of working [default: 4]
      --budget-tokens <BUDGET_TOKENS>  The tokens each run of a task may use [default: 20000]
      --apply                          Make what was kept the profile's: the best words kept added to its instructions, the best models kept for a route that route's
  -h, --help                           Print help
```

## `sion agent improve kept`

```text
The changes a profile's agent has kept, each with the evidence for keeping it

Usage: sion agent improve kept [OPTIONS] <PROFILE>

Arguments:
  <PROFILE>  The profile

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent improve mark`

```text
Say how one of the agent's sessions went, so that it can be learnt from (`--good` or `--bad`); a session whose check passed needs none

Usage: sion agent improve mark [OPTIONS] <SESSION>

Arguments:
  <SESSION>  The session (its id, or a unique prefix of it)

Options:
      --good         It went well
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --bad          It went badly
      --note <NOTE>  What you say of it [default: ""]
  -h, --help         Print help
```

## `sion agent improve harvest`

```text
Turn the profile's sessions whose outcome is known (marked, or their check passed) into a set of tasks: each task asked again, of stand-ins that answer what its tools answered, checked against what the session did

Usage: sion agent improve harvest [OPTIONS] <PROFILE>

Arguments:
  <PROFILE>  The profile

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --out <FILE>   Where to write the set (default: printed)
  -h, --help         Print help
```

## `sion agent improve export`

```text
Export what the profile's work gives for training an open model on it: its sessions that went well as examples to fine-tune on (sft.jsonl, OpenAI's chat format with tool calls and the tools shown), and its tasks as environments to reinforce in (environments.jsonl, tasks.json: sion agent improve env runs one). Nothing sealed is exported, and nothing exported is sealed later

Usage: sion agent improve export [OPTIONS] --out <DIR> <PROFILE>

Arguments:
  <PROFILE>  The profile

Options:
      --node <NODE>              The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --out <DIR>                The directory to write into
      --validation <VALIDATION>  The share of examples held apart for validation, in percent [default: 10]
  -h, --help                     Print help
```

## `sion agent improve env`

```text
Run one task as an episode for reinforcement learning, the policy outside: each request on standard output, in OpenAI's chat format ({"request": {"messages", "tools", "max_tokens"}}), the policy's turn on standard input ({"message": {...}}), and the reward last ({"done": {"reward", "passed", "failures"}}), the share of the task's checks that held

Usage: sion agent improve env [OPTIONS] --tasks <FILE> --case <CASE>

Options:
      --node <NODE>   The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --tasks <FILE>  The tasks (a set, as export or harvest writes it)
      --case <CASE>   The task's id
  -h, --help          Print help
```

## `sion agent improve seal`

```text
Seal a set of tasks for the profile: a share held apart to confirm changes on, which no proposer is shown, with canary cases among them and a number of confirmations (with --reusable, a reusable holdout instead). With --wallet, the seal's digests are published on the ledger before anything is tried on it

Usage: sion agent improve seal [OPTIONS] --tasks <FILE> <PROFILE>

Arguments:
  <PROFILE>  The profile

Options:
      --node <NODE>                    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --tasks <FILE>                   The tasks (a set, as `sion agent improve harvest` writes)
      --share <SHARE>                  The share sealed, in percent [default: 30]
      --confirmations <CONFIRMATIONS>  The confirmations the seal allows; each is held to 5% split among them all. For a reusable holdout, also how many times its sealed tasks may disagree with the tasks to choose on [default: 8]
      --canaries <CANARIES>            Canary cases added to the sealed tasks [default: 2]
      --seed <SEED>                    A seed mixed into the split (default: drawn)
      --reusable                       Make the sealed tasks a reusable holdout too (Thresholdout): asked first about a change's gain, they turn down, without spending a confirmation, a change whose gain they answer, with noise, as not above a threshold; any other change is confirmed as on any seal. Its own budget is spent only when they disagree with the tasks to choose on
      --wallet <WALLET>                Publish the seal as this wallet's claim
  -h, --help                           Print help
```

With `--wallet`, the seal's digests are published as a claim (`agent.seal/v1`) before anything is tried on it. With `--reusable`, its sealed tasks are a reusable holdout (Thresholdout), asked about each change's gain instead of spending a fixed confirmation each. See [Improving itself](../agent/self-improvement.md#sealed-before-anything-is-tried).

## `sion agent improve run`

```text
Let the agent improve itself: a proposer model writes changes from the tasks it failed among the seal's tasks to choose on; each is tried there, and kept only when confirmed better on the sealed tasks. With --apply a kept change becomes the profile's; with --wallet each is published on the ledger

Usage: sion agent improve run [OPTIONS] --proposer <MODEL> <PROFILE>

Arguments:
  <PROFILE>  The profile

Options:
      --node <NODE>                    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --proposer <MODEL>               The model that proposes changes
      --rounds <ROUNDS>                Rounds, at most [default: 4]
      --runs <RUNS>                    How many times each task is run by each variant [default: 4]
      --budget-tokens <BUDGET_TOKENS>  The tokens each run of a task may use [default: 20000]
      --total-tokens <TOTAL_TOKENS>    The tokens the whole run may use, its proposer's included (0: no limit but the rounds) [default: 0]
      --model <MODEL>                  A model the proposer may choose, besides the profile's own (repeat for more)
      --seed <SEED>                    The seed of the archive's draws (default: drawn)
      --practice <FILE>                Practice tasks (as `sion agent improve practice` writes) to choose changes on beside the seal's; a change is still confirmed on the sealed tasks alone
      --challenger <MODEL>             A model that writes variants of the tasks each change newly passes, to catch a change that only fits them: it must be better on them too, beyond chance, before it is confirmed
      --apply                          Make each kept change the profile's
      --wallet <WALLET>                Publish each kept change as this wallet's claim
  -h, --help                           Print help
```

See [Improving itself](../agent/self-improvement.md#the-loop).

## `sion agent improve practice`

```text
Have a model write practice tasks for the profile, like the seal's tasks to choose on (it is never shown the sealed ones): each kept only when its own solution passes it and its failures fail it, when it is no sealed task or task to choose on over again, and when the agent as it is does not pass it every time. `run --practice FILE` chooses changes on them beside the seal's

Usage: sion agent improve practice [OPTIONS] <PROFILE>

Arguments:
  <PROFILE>  The profile

Options:
      --node <NODE>                    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --writer <MODEL>                 The model that writes them (default: the profile's first, so that the agent writes its own)
      --count <COUNT>                  How many to ask for [default: 8]
      --runs <RUNS>                    How many times the agent runs each [default: 2]
      --budget-tokens <BUDGET_TOKENS>  The tokens each run may use [default: 20000]
      --out <FILE>                     Where to write them (default: beside the seal)
  -h, --help                           Print help
```

The writer is shown what the operator tells the agent and the seal's tasks to choose on, never its sealed ones. A task it writes comes with a solution and failures, played against the task's own stand-ins before it is kept. See [Improving itself](../agent/self-improvement.md#practice-tasks).

## `sion agent improve lineage`

```text
The changes the agent kept from its own work, with their evidence; with --check, each held to its claim on the ledger

Usage: sion agent improve lineage [OPTIONS] <PROFILE>

Arguments:
  <PROFILE>  The profile

Options:
      --check        Check the claims on the ledger
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent improve adopt`

```text
Make a kept change the profile's

Usage: sion agent improve adopt [OPTIONS] <PROFILE> <STEP>

Arguments:
  <PROFILE>  The profile
  <STEP>     The kept change (its id, or a unique prefix of it)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent improve undo`

```text
Take back the last change made the profile's

Usage: sion agent improve undo [OPTIONS] <PROFILE>

Arguments:
  <PROFILE>  The profile

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent improve share`

```text
Share a lesson the profile's agent kept, as your claim: its words and the evidence it was kept on, on your bond `lesson-author`, which the adjudicator can rule against. Other owners' agents find it (`lessons`) and keep it only when their own sealed tasks confirm it (`learn`)

Usage: sion agent improve share [OPTIONS] --wallet <WALLET> --adjudicator <ADJUDICATOR> <PROFILE>

Arguments:
  <PROFILE>  The profile

Options:
      --node <NODE>                The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --step <STEP>                The kept change whose lesson to share (its id, or a unique prefix of it; default: the latest that changed what the agent has learned)
      --wallet <WALLET>            Your wallet
      --adjudicator <ADJUDICATOR>  The adjudicator who can rule against the lesson (its atom's id, 64 hex digits)
      --bond <USD>                 Post your bond `lesson-author` with this many USD first
      --price <USD>                What you ask of an owner whose agent keeps it, in USD. Whether they pay is theirs to choose: a lesson's words are public [default: 0]
      --valid-for <VALID_FOR>      How many Pulses the lesson stands for [default: 1000000]
  -h, --help                       Print help
```

## `sion agent improve lessons`

```text
The lessons shared on this ledger, each with the evidence its author kept it on and whose agents have kept it since

Usage: sion agent improve lessons [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent improve learn`

```text
Have the profile's agent try lessons other owners shared. Each is added to what it has learned and kept only when that is better beyond chance on the profile's tasks to choose on and then on its sealed tasks, like any change; no model proposes anything. With --apply a kept lesson becomes the profile's; with --wallet it is published, with a claim saying whose lesson it was

Usage: sion agent improve learn [OPTIONS] <PROFILE>

Arguments:
  <PROFILE>  The profile

Options:
      --lesson <CLAIM>                 Only these lessons (a claim's id, or a prefix of it; repeat for more)
      --node <NODE>                    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --by <ACCOUNT>                   Only lessons by these authors (a wallet's name or an account's id; repeat for more)
      --adoptions <ADOPTIONS>          Only lessons that at least this many other owners' agents kept [default: 0]
      --most <MOST>                    Lessons to try, at most. A lesson that passes the tasks to choose on spends one of the seal's confirmations [default: 4]
      --runs <RUNS>                    How many times each task is run by each variant [default: 4]
      --budget-tokens <BUDGET_TOKENS>  The tokens each run of a task may use [default: 20000]
      --seed <SEED>                    The seed of the archive's draws (default: drawn)
      --apply                          Make each kept lesson the profile's
      --wallet <WALLET>                Publish each kept lesson as this wallet's claim, and a claim saying whose lesson it was (an adoption)
      --pay-up-to <USD>                Pay the author of a kept lesson what it asks, if that is at most this many USD (with --wallet)
  -h, --help                           Print help
```

## `sion agent immune`

```text
Attacks reported between operators: report the text an agent was attacked with, on a bond; vouch for a report; say whose reports your agents trust, and they hold back content from outside that holds the same text

Usage: sion agent immune [OPTIONS] <COMMAND>

Commands:
  report   Report an attack's signature, as you: a piece of the text that attacked an agent, as a claim on your bond `attack-reporter`, which the adjudicator can rule against. Agents whose operators trust you, or someone who vouches for it, hold back content from outside that holds this text
  vouch    Vouch for a reported signature, as you: a claim on your bond `attack-verifier` that you checked it marks an attack and not ordinary content
  trust    Name the accounts whose report or vouching makes a signature one your agents look for. With none named, nothing is looked for
  list     The signatures reported on this ledger: who reported and vouches for each, whether your agents look for it, and what it held here
  suspend  Stop looking for a signature on this machine, whatever stands behind it: for one that holds what it should not
  resume   Look for a suspended signature again
  thank    Say, as you, that the signatures that held content here did, and pay each one's reporter a tip if you name one
  help     Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent immune report`

```text
Report an attack's signature, as you: a piece of the text that attacked an agent, as a claim on your bond `attack-reporter`, which the adjudicator can rule against. Agents whose operators trust you, or someone who vouches for it, hold back content from outside that holds this text

Usage: sion agent immune report [OPTIONS] --pattern <PATTERN> --adjudicator <ADJUDICATOR> <WALLET>

Arguments:
  <WALLET>  Your wallet

Options:
      --node <NODE>                The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --pattern <PATTERN>          The text that marks the attack: at least 16 characters and 3 words, at most 300. Case, spacing and invisible characters do not matter
      --kind <KIND>                What kind of attack it is, in a word or two [default: "prompt injection"]
      --note <NOTE>                What it does and how it was met, in a line [default: ""]
      --sample <FILE>              The content it was found in: checked to hold the text, and its digest published (never the content)
      --seen-at <HOST>             The host it was met at
      --adjudicator <ADJUDICATOR>  The adjudicator who can rule against the report (its atom's id, 64 hex digits)
      --bond <USD>                 Post your bond `attack-reporter` with this many USD first
      --valid-for <VALID_FOR>      How many Pulses the report stands for [default: 1000000]
  -h, --help                       Print help
```

## `sion agent immune vouch`

```text
Vouch for a reported signature, as you: a claim on your bond `attack-verifier` that you checked it marks an attack and not ordinary content

Usage: sion agent immune vouch [OPTIONS] --adjudicator <ADJUDICATOR> <WALLET> <SIGNATURE>

Arguments:
  <WALLET>     Your wallet
  <SIGNATURE>  The signature (its claim's id, or a unique prefix of it)

Options:
      --adjudicator <ADJUDICATOR>  The adjudicator who can rule against your vouching (its atom's id, 64 hex digits)
      --node <NODE>                The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --bond <USD>                 Post your bond `attack-verifier` with this many USD first
      --note <NOTE>                What you say of it, in a line [default: ""]
      --valid-for <VALID_FOR>      How many Pulses the vouching stands for [default: 1000000]
  -h, --help                       Print help
```

## `sion agent immune trust`

```text
Name the accounts whose report or vouching makes a signature one your agents look for. With none named, nothing is looked for

Usage: sion agent immune trust [OPTIONS] [ACCOUNTS]...

Arguments:
  [ACCOUNTS]...  The accounts (a wallet's name or an account's id)

Options:
      --need <NEED>  How many of the accounts you trust a signature needs behind it
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --remove       Stop trusting these accounts instead
  -h, --help         Print help
```

## `sion agent immune list`

```text
The signatures reported on this ledger: who reported and vouches for each, whether your agents look for it, and what it held here

Usage: sion agent immune list [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent immune suspend`

```text
Stop looking for a signature on this machine, whatever stands behind it: for one that holds what it should not

Usage: sion agent immune suspend [OPTIONS] <SIGNATURE>

Arguments:
  <SIGNATURE>  The signature (its claim's id, or a unique prefix of it)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent immune resume`

```text
Look for a suspended signature again

Usage: sion agent immune resume [OPTIONS] <SIGNATURE>

Arguments:
  <SIGNATURE>  The signature (its claim's id, or a unique prefix of it)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent immune thank`

```text
Say, as you, that the signatures that held content here did, and pay each one's reporter a tip if you name one

Usage: sion agent immune thank [OPTIONS] <WALLET>

Arguments:
  <WALLET>  Your wallet

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --tip <USD>    What to pay each such signature's reporter, in USD [default: 0]
  -h, --help         Print help
```

## `sion agent record`

```text
An agent's record, for whoever is asked to stand behind it or rely on it: what the ledger shows of what it may do and has done, which anyone can check, what its operator says of its sessions, and a warranty on a bond

Usage: sion agent record [OPTIONS] <COMMAND>

Commands:
  write    Write an agent's record: what the ledger shows of it over a span of Pulses, which anyone can recompute, and, on the machine that holds its sessions, what its operator says of them. With --wallet, a claim commits to the record
  check    Check a record someone gave you against the ledger: every figure it says the ledger shows is added up again, and whether its owner committed to it is looked up. What its operator says of its own sessions is shown as that, and each rule it proved of sealed figures is verified
  prove    Prove that a sealed record's figures keep a rule, without showing them, and add the proof to the record. A rule names figures (calls, calls_failed, payments_refused, expected, expected_held, attacks_held, sessions, done, checked, asked, approved, declined, signed), whole numbers and products of the two, joined by +, and compares two sides with <=, >= or ==: "calls_failed*50 <= calls"
  warrant  Stand behind an agent, as you: a claim on your bond `agent-warranty` saying what you warrant of it, which the adjudicator can rule against. A counterparty relies on it with `sion accept`, which reserves part of the bond, and is compensated from the bond if a challenge to it is upheld
  help     Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent record write`

```text
Write an agent's record: what the ledger shows of it over a span of Pulses, which anyone can recompute, and, on the machine that holds its sessions, what its operator says of them. With --wallet, a claim commits to the record

Usage: sion agent record write [OPTIONS] <AGENT>

Arguments:
  <AGENT>  The agent (its id)

Options:
      --from <FROM>      The first Pulse counted [default: 0]
      --node <NODE>      The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --to <TO>          The last Pulse counted (default: the latest)
      --ledger-only      Leave out what this machine's sessions show
      --seal             Seal what this machine's sessions show: each figure is committed to and not shown, and what opens them is written beside the record (--out) for you to keep. `prove` then shows whoever asks that the figures keep a rule, and nothing else about them
      --out <FILE>       Write the record to this file, as JSON
      --wallet <WALLET>  Commit to the record in a claim of this wallet's
  -h, --help             Print help
```

## `sion agent record check`

```text
Check a record someone gave you against the ledger: every figure it says the ledger shows is added up again, and whether its owner committed to it is looked up. What its operator says of its own sessions is shown as that, and each rule it proved of sealed figures is verified

Usage: sion agent record check [OPTIONS] <FILE>

Arguments:
  <FILE>  The record (a file `write --out` made)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent record prove`

```text
Prove that a sealed record's figures keep a rule, without showing them, and add the proof to the record. A rule names figures (calls, calls_failed, payments_refused, expected, expected_held, attacks_held, sessions, done, checked, asked, approved, declined, signed), whole numbers and products of the two, joined by +, and compares two sides with <=, >= or ==: "calls_failed*50 <= calls"

Usage: sion agent record prove [OPTIONS] --rule <RULE> <FILE>

Arguments:
  <FILE>  The sealed record (a file `write --seal --out` made)

Options:
      --node <NODE>      The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --rule <RULE>      A rule to prove (repeat for more)
      --openings <FILE>  What opens the record's commitments (default: the file `write --seal` wrote beside the record)
  -h, --help             Print help
```

## `sion agent record warrant`

```text
Stand behind an agent, as you: a claim on your bond `agent-warranty` saying what you warrant of it, which the adjudicator can rule against. A counterparty relies on it with `sion accept`, which reserves part of the bond, and is compensated from the bond if a challenge to it is upheld

Usage: sion agent record warrant [OPTIONS] --covers <COVERS> --adjudicator <ADJUDICATOR> <WALLET> <AGENT>

Arguments:
  <WALLET>  Your wallet
  <AGENT>   The agent (its id)

Options:
      --covers <COVERS>            What you warrant of it
      --node <NODE>                The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --adjudicator <ADJUDICATOR>  The adjudicator who rules on challenges to it (its atom's id, 64 hex digits)
      --bond <USD>                 Post your bond `agent-warranty` with this many USD first
      --valid-for <VALID_FOR>      How many Pulses the warranty stands for [default: 1000000]
  -h, --help                       Print help
```

## `sion agent comply`

```text
Proof that payments kept your rules without showing the payments: commitments to each amount and payee, and zero-knowledge proofs that none was above a limit, that they came to no more than a total, and that each went to one of a list

Usage: sion agent comply [OPTIONS] <COMMAND>

Commands:
  prove  Prove that payments you will not show kept your rules: none above a limit, no more than a total in all, each to one of a list of payees if you give one. Writes a certificate anyone can check, and the openings, which you keep to yourself
  check  Check a certificate someone gave you. Needs nothing but the file
  help   Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent comply prove`

```text
Prove that payments you will not show kept your rules: none above a limit, no more than a total in all, each to one of a list of payees if you give one. Writes a certificate anyone can check, and the openings, which you keep to yourself

Usage: sion agent comply prove [OPTIONS] --payments <FILE> --max-payment <USD> --total <USD> --for <TEXT> --out <FILE>

Options:
      --node <NODE>        The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --payments <FILE>    The payments: a JSON file, a list of {"amount": "<USD>", "to": "<payee>"}
      --max-payment <USD>  What no payment may be above, in USD
      --total <USD>        What they may come to in all, in USD
      --payee <NAME>       A payee payments may go to (repeat for more). With none, payees are neither committed to nor proven
      --for <TEXT>         What the certificate is for: it holds for this and nothing else
      --out <FILE>         Where to write the certificate
      --wallet <WALLET>    Commit to the certificate in a claim of this wallet's, so that when the list was fixed is on the ledger
  -h, --help               Print help
```

## `sion agent comply check`

```text
Check a certificate someone gave you. Needs nothing but the file

Usage: sion agent comply check [OPTIONS] <FILE>

Arguments:
  <FILE>  The certificate

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent exam`

```text
Sealed exams: tasks encrypted and committed to on the ledger, agents entered before a threshold of trustees opens them, and results that anyone can run again once they are open

Usage: sion agent exam [OPTIONS] <COMMAND>

Commands:
  seal     Seal a set of tasks as an exam, as you: the tasks are encrypted, their digest published, and the key split among trustees. Writes the exam's file, which anyone may hold, and a file for each trustee with its share, which you hand to that trustee and no one else
  enter    Enter a profile's agent in an exam, as you: a claim that fixes its models, what you tell it and what it has learned, in full. An entry counts only if it is made before the exam opens
  release  Release your share of an exam's key, as one of its trustees. When enough trustees have, the tasks can be read by anyone and no further entry counts
  run      Run an entry's agent on an opened exam's tasks, with your own access to its models; with --wallet, publish what you found
  results  Where an exam stands: whether it is open, the entries and whether each counts, and what runs of each found, by whom. With several exams, by different authors, also each agent's results side by side: no author has read another's tasks
  tasks    Write an opened exam's tasks to a file, for anyone to read and run
  help     Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent exam seal`

```text
Seal a set of tasks as an exam, as you: the tasks are encrypted, their digest published, and the key split among trustees. Writes the exam's file, which anyone may hold, and a file for each trustee with its share, which you hand to that trustee and no one else

Usage: sion agent exam seal [OPTIONS] --tasks <FILE> --trustee <ACCOUNT> --threshold <THRESHOLD> --wallet <WALLET> <NAME>

Arguments:
  <NAME>  The exam's name (lowercase letters, digits and hyphens)

Options:
      --node <NODE>                    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --tasks <FILE>                   The tasks (a set, as `sion agent improve harvest` writes)
      --about <ABOUT>                  What it examines, in a line [default: ""]
      --trustee <ACCOUNT>              A trustee (a wallet's name or an account's id; repeat for more)
      --threshold <THRESHOLD>          How many trustees' shares open it
      --runs <RUNS>                    How many times each task is run [default: 3]
      --budget-tokens <BUDGET_TOKENS>  The tokens each run of a task may use [default: 20000]
      --wallet <WALLET>                Your wallet
      --out <DIR>                      The directory to write the files in [default: .]
  -h, --help                           Print help
```

## `sion agent exam enter`

```text
Enter a profile's agent in an exam, as you: a claim that fixes its models, what you tell it and what it has learned, in full. An entry counts only if it is made before the exam opens

Usage: sion agent exam enter [OPTIONS] --wallet <WALLET> <EXAM> <PROFILE>

Arguments:
  <EXAM>     The exam's file
  <PROFILE>  The profile

Options:
      --node <NODE>      The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --wallet <WALLET>  Your wallet
  -h, --help             Print help
```

## `sion agent exam release`

```text
Release your share of an exam's key, as one of its trustees. When enough trustees have, the tasks can be read by anyone and no further entry counts

Usage: sion agent exam release [OPTIONS] --wallet <WALLET> <EXAM> <SHARE>

Arguments:
  <EXAM>   The exam's file
  <SHARE>  Your share's file

Options:
      --node <NODE>      The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --wallet <WALLET>  Your wallet
  -h, --help             Print help
```

## `sion agent exam run`

```text
Run an entry's agent on an opened exam's tasks, with your own access to its models; with --wallet, publish what you found

Usage: sion agent exam run [OPTIONS] <EXAM> <ENTRY>

Arguments:
  <EXAM>   The exam's file
  <ENTRY>  The entry (its claim's id, or a unique prefix of it)

Options:
      --node <NODE>      The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --wallet <WALLET>  Publish what the run found as this wallet's claim
  -h, --help             Print help
```

## `sion agent exam results`

```text
Where an exam stands: whether it is open, the entries and whether each counts, and what runs of each found, by whom. With several exams, by different authors, also each agent's results side by side: no author has read another's tasks

Usage: sion agent exam results [OPTIONS] <EXAMS>...

Arguments:
  <EXAMS>...  The exam's file (repeat for several exams)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent exam tasks`

```text
Write an opened exam's tasks to a file, for anyone to read and run

Usage: sion agent exam tasks [OPTIONS] --out <FILE> <EXAM>

Arguments:
  <EXAM>  The exam's file

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --out <FILE>   Where to write them
  -h, --help         Print help
```

## `sion agent receipts`

```text
Receipts for which model answered: the model a session's runtime asked for each answer, at which address, and the model the provider said answered, in a chain; written for someone else to check, committed to in a claim, or signed by an enclave

Usage: sion agent receipts [OPTIONS] <COMMAND>

Commands:
  show   A session's receipts: which model was asked for each answer, at which address, which model the provider said answered, and the tokens
  write  Write a session's receipts to a file for someone else to check; with --wallet, commit to them in a claim, which fixes them as of now
  check  Check a file of receipts: that they are a chain, and who stands behind them. An operator's are checked against its claim; an enclave's against its attestation and the key it binds
  help   Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent receipts show`

```text
A session's receipts: which model was asked for each answer, at which address, which model the provider said answered, and the tokens

Usage: sion agent receipts show [OPTIONS] <SESSION>

Arguments:
  <SESSION>  The session

Options:
      --last <LAST>  How many of the last receipts to list [default: 10]
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent receipts write`

```text
Write a session's receipts to a file for someone else to check; with --wallet, commit to them in a claim, which fixes them as of now

Usage: sion agent receipts write [OPTIONS] --out <FILE> <SESSION>

Arguments:
  <SESSION>  The session

Options:
      --node <NODE>      The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --out <FILE>       The file to write
      --wallet <WALLET>  Commit to them in a claim of this wallet's
  -h, --help             Print help
```

## `sion agent receipts check`

```text
Check a file of receipts: that they are a chain, and who stands behind them. An operator's are checked against its claim; an enclave's against its attestation and the key it binds

Usage: sion agent receipts check [OPTIONS] <FILE>

Arguments:
  <FILE>  The file

Options:
      --measurement <MEASUREMENT>  The measurement of the enclave's code you trust (hex)
      --node <NODE>                The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --simulated-platform <KEY>   SIMULATED: a simulated platform's key to take (hex), for tests and devnets
  -h, --help                       Print help
```

## `sion agent market`

```text
A market for agents' work, priced per accepted result: sealed bids in rounds, each cleared at one price anyone can recompute, and a Task Charter from each buyer to the worker it was paired with

Usage: sion agent market [OPTIONS] <COMMAND>

Commands:
  open    Open a market for one kind of work: what a unit is, the rule a result must pass to be paid, a price band, and how long each round takes bids and reveals
  bid     Make a sealed bid in the round now taking bids: to buy units of work at no more than a price, or to offer them at no less. The price and quantity stay on this machine until you reveal them; what is published commits to them and proves they are inside the market's band
  reveal  Reveal your bids of the round now taking reveals. A bid that is not revealed in time is left out of its round
  clear   Clear a closed round from the bids the ledger holds: the certified allocation, its one price, and who works for whom. Anyone computes the same; with --wallet, record it in a claim
  post    Post the tasks you owe from a cleared round: one for each unit you bought, to the worker you were paired with, at the round's price, under the market's acceptance. The price is escrowed now and paid for an accepted result
  show    Where a market stands: its terms, each round's bids and price, who works for whom, and who posted the tasks they owed and was paid
  help    Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent market open`

```text
Open a market for one kind of work: what a unit is, the rule a result must pass to be paid, a price band, and how long each round takes bids and reveals

Usage: sion agent market open [OPTIONS] --unit <UNIT> --min-price <USD> --max-price <USD> --wallet <WALLET> <NAME>

Arguments:
  <NAME>  The market's name (lowercase letters, digits and hyphens)

Options:
      --node <NODE>
          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --unit <UNIT>
          What one unit of work is, in words a worker can price
      --min-price <USD>
          The lowest price of a unit a bid may name, in USD
      --max-price <USD>
          The highest
      --max-quantity <MAX_QUANTITY>
          The most units one bid may be for [default: 100]
      --bidding <BIDDING>
          How many Pulses each round takes bids for [default: 20]
      --revealing <REVEALING>
          How many Pulses, after those, it takes reveals for [default: 20]
      --first <FIRST>
          The first Pulse of the first round (the next Pulse, if not given)
      --verifier <ACCOUNT>
          Verifiers whose attestations of a result count (wallets or accounts; repeat for more)
      --min-attestations <MIN_ATTESTATIONS>
          How many attestations a result needs at least [default: 0]
      --adjudicator <ADJUDICATOR>
          The adjudicator atom results must name (hex)
      --reliance <RELIANCE>
          Pulses of reliance on a paid result [default: 1000]
      --post-within <POST_WITHIN>
          How many Pulses after a round closes its buyers have to post their tasks [default: 100]
      --deadline <DEADLINE>
          How many Pulses a posted task's worker has to settle it [default: 2000]
      --wallet <WALLET>
          Your wallet
  -h, --help
          Print help
```

## `sion agent market bid`

```text
Make a sealed bid in the round now taking bids: to buy units of work at no more than a price, or to offer them at no less. The price and quantity stay on this machine until you reveal them; what is published commits to them and proves they are inside the market's band

Usage: sion agent market bid [OPTIONS] --side <SIDE> --quantity <QUANTITY> --price <USD> --wallet <WALLET> <MARKET>

Arguments:
  <MARKET>
          The market (its claim)

Options:
      --node <NODE>
          The node's API
          
          [env: SION_NODE=]
          [default: http://127.0.0.1:8730]

      --side <SIDE>
          Which side

          Possible values:
          - buy:  Buy work: pay at most the price for each unit
          - sell: Offer work: take at least the price for each unit

      --quantity <QUANTITY>
          How many units

      --price <USD>
          The price of a unit, in USD: the most you pay, or the least you take

      --wallet <WALLET>
          Your wallet

  -h, --help
          Print help (see a summary with '-h')
```

## `sion agent market reveal`

```text
Reveal your bids of the round now taking reveals. A bid that is not revealed in time is left out of its round

Usage: sion agent market reveal [OPTIONS] --wallet <WALLET> <MARKET>

Arguments:
  <MARKET>  The market (its claim)

Options:
      --node <NODE>      The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --wallet <WALLET>  Your wallet
  -h, --help             Print help
```

## `sion agent market clear`

```text
Clear a closed round from the bids the ledger holds: the certified allocation, its one price, and who works for whom. Anyone computes the same; with --wallet, record it in a claim

Usage: sion agent market clear [OPTIONS] <MARKET> <ROUND>

Arguments:
  <MARKET>  The market (its claim)
  <ROUND>   The round

Options:
      --node <NODE>      The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --wallet <WALLET>  Record the clearing as this wallet's claim
  -h, --help             Print help
```

## `sion agent market post`

```text
Post the tasks you owe from a cleared round: one for each unit you bought, to the worker you were paired with, at the round's price, under the market's acceptance. The price is escrowed now and paid for an accepted result

Usage: sion agent market post [OPTIONS] --wallet <WALLET> <MARKET> <ROUND>

Arguments:
  <MARKET>  The market (its claim)
  <ROUND>   The round

Options:
      --item <FILE>      A file describing your work items, committed to in each task's spec
      --node <NODE>      The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --wallet <WALLET>  Your wallet
  -h, --help             Print help
```

## `sion agent market show`

```text
Where a market stands: its terms, each round's bids and price, who works for whom, and who posted the tasks they owed and was paid

Usage: sion agent market show [OPTIONS] <MARKET>

Arguments:
  <MARKET>  The market (its claim)

Options:
      --node <NODE>    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --round <ROUND>  A round to show (every round so far, if not given)
  -h, --help           Print help
```

## `sion agent recorder`

```text
A flight recorder for an agent that acts in the world: a device's events in a log the ledger keeps append-only, recorded without a network and fixed by each commit, with extracts an investigator checks and bounds proven of readings that are not shown

Usage: sion agent recorder [OPTIONS] <COMMAND>

Commands:
  open     Open a recorder: a log on the ledger that this wallet writes, and its store on this machine
  record   Record events on this machine, with no network: lines of JSON, each {"t_ms": …, "kind": "sensor|command|decision|fault|heartbeat", "channel": …, "name": …, "value": a whole number or text}
  commit   Commit the log to the ledger: every record made so far is fixed as of this Pulse and cannot be changed or left out afterwards
  show     Where a recorder stands on this machine: how much is recorded and fixed, stretches with no record, and a clock that stepped back
  extract  Make an extract for an investigator: every committed record from the first to the last whose device time is in a window, each with the path that shows it is in the log the ledger holds
  prove    Prove a bound on every sealed reading of one kind in an extract, without showing the readings, and add the proof to the extract
  check    Check an extract against the ledger, as an investigator: that its records are in the log the ledger holds, all of them and in order, by when they were fixed, what their own times show to be missing, and the bounds proven of its sealed readings
  help     Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent recorder open`

```text
Open a recorder: a log on the ledger that this wallet writes, and its store on this machine

Usage: sion agent recorder open [OPTIONS] --wallet <WALLET> <NAME>

Arguments:
  <NAME>  The recorder's name (lowercase letters, digits and hyphens)

Options:
      --node <NODE>                  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --wallet <WALLET>              The wallet that writes it
      --heartbeat-ms <HEARTBEAT_MS>  How often the device records something, a heartbeat if nothing else, in milliseconds: a stretch of more than twice this with no record is reported [default: 1000]
      --seal <CHANNEL>               A channel whose sensor readings are logged as commitments, not shown (it and the channels under it; repeat for more)
  -h, --help                         Print help
```

## `sion agent recorder record`

```text
Record events on this machine, with no network: lines of JSON, each {"t_ms": …, "kind": "sensor|command|decision|fault|heartbeat", "channel": …, "name": …, "value": a whole number or text}

Usage: sion agent recorder record [OPTIONS] <NAME>

Arguments:
  <NAME>  The recorder

Options:
      --file <FILE>  The file of events (standard input, if not given)
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent recorder commit`

```text
Commit the log to the ledger: every record made so far is fixed as of this Pulse and cannot be changed or left out afterwards

Usage: sion agent recorder commit [OPTIONS] <NAME>

Arguments:
  <NAME>  The recorder

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --seal         Close the log for good: after an incident, so that an extract of it stays checkable
  -h, --help         Print help
```

## `sion agent recorder show`

```text
Where a recorder stands on this machine: how much is recorded and fixed, stretches with no record, and a clock that stepped back

Usage: sion agent recorder show [OPTIONS] <NAME>

Arguments:
  <NAME>  The recorder

Options:
      --last <LAST>  List the last records too [default: 0]
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent recorder extract`

```text
Make an extract for an investigator: every committed record from the first to the last whose device time is in a window, each with the path that shows it is in the log the ledger holds

Usage: sion agent recorder extract [OPTIONS] --from-ms <FROM_MS> --until-ms <UNTIL_MS> --out <FILE> <NAME>

Arguments:
  <NAME>  The recorder

Options:
      --from-ms <FROM_MS>    The window's start, in milliseconds since the Unix epoch
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --until-ms <UNTIL_MS>  The window's end (exclusive)
      --withhold <CHANNEL>   A channel whose records are given as digests only (it and the channels under it; repeat for more)
      --out <FILE>           The file to write
  -h, --help                 Print help
```

## `sion agent recorder prove`

```text
Prove a bound on every sealed reading of one kind in an extract, without showing the readings, and add the proof to the extract

Usage: sion agent recorder prove [OPTIONS] --channel <CHANNEL> --reading <READING> <NAME> <EXTRACT>

Arguments:
  <NAME>     The recorder
  <EXTRACT>  The extract

Options:
      --channel <CHANNEL>    The channel
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --reading <READING>    The reading's name
      --at-most <AT_MOST>    What every reading is at most
      --at-least <AT_LEAST>  What every reading is at least
  -h, --help                 Print help
```

## `sion agent recorder check`

```text
Check an extract against the ledger, as an investigator: that its records are in the log the ledger holds, all of them and in order, by when they were fixed, what their own times show to be missing, and the bounds proven of its sealed readings

Usage: sion agent recorder check [OPTIONS] <EXTRACT>

Arguments:
  <EXTRACT>  The extract

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --records      List the records shown
  -h, --help         Print help
```

## `sion agent dataset`

```text
Datasets of checked work: sessions contributed by their owners on a dataset's terms, each with what checked the work in it, built into versions a trainer checks against the ledger

Usage: sion agent dataset [OPTIONS] <COMMAND>

Commands:
  open        Open a dataset, as its curator: what it is for, the terms a contributor agrees to, and what counts as checked
  contribute  Contribute a session to a dataset, on its terms: an example is written to a file for the curator, and a claim of yours commits to it, says what checked the work and agrees to the terms. Nothing of the example goes on the ledger
  withdraw    Withdraw an example you contributed: versions built from now on leave it out. A version already built cannot be called back
  build       Build a version, as the dataset's curator, from the example files in a directory: each is checked against its contribution and what it says checked it, and those taken are written as a file to train on, with a manifest, and committed to in a claim
  check       Check a file of examples against its manifest and the ledger, as someone about to train on it: that the examples are the version's, that each was contributed on the dataset's terms, and what checked each
  show        A dataset's card and its latest version
  help        Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent dataset open`

```text
Open a dataset, as its curator: what it is for, the terms a contributor agrees to, and what counts as checked

Usage: sion agent dataset open [OPTIONS] --terms <FILE> --take <TAKES> --wallet <WALLET> <NAME>

Arguments:
  <NAME>
          The dataset's name (lowercase letters, digits and hyphens)

Options:
      --about <ABOUT>
          What it is for, in a line
          
          [default: ""]

      --node <NODE>
          The node's API
          
          [env: SION_NODE=]
          [default: http://127.0.0.1:8730]

      --terms <FILE>
          A file with the terms a contributor agrees to

      --take <TAKES>
          What counts as checked (repeat for more): task (a settled Task Charter), check (the session's own check passed), marked (its operator marked it good)

          Possible values:
          - task:   A Task Charter settled on the ledger: the result was accepted under the rule its poster fixed
          - check:  The session's own check passed (`--verify`): its operator's word
          - marked: Its operator marked the session good: its operator's word

      --outside
          Take sessions that read content from outside too

      --wallet <WALLET>
          Your wallet

  -h, --help
          Print help (see a summary with '-h')
```

## `sion agent dataset contribute`

```text
Contribute a session to a dataset, on its terms: an example is written to a file for the curator, and a claim of yours commits to it, says what checked the work and agrees to the terms. Nothing of the example goes on the ledger

Usage: sion agent dataset contribute [OPTIONS] --wallet <WALLET> <DATASET> <SESSION>

Arguments:
  <DATASET>  The dataset (its card's claim)
  <SESSION>  The session

Options:
      --agree            Say that you agree to the dataset's terms: without it the terms are shown and nothing is contributed
      --node <NODE>      The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --task <TASK>      The Task Charter whose settlement checked this work
      --output <FILE>    The file the task was settled with (the session's answer, if not given)
      --redact <TEXT>    Text to take out of the example wherever it stands (repeat for more)
      --withhold-system  Leave your system prompt out of the example
      --out <DIR>        The directory to write the example in [default: .]
      --wallet <WALLET>  Your wallet
  -h, --help             Print help
```

## `sion agent dataset withdraw`

```text
Withdraw an example you contributed: versions built from now on leave it out. A version already built cannot be called back

Usage: sion agent dataset withdraw [OPTIONS] --wallet <WALLET> <EXAMPLE>

Arguments:
  <EXAMPLE>  The example's file

Options:
      --node <NODE>      The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --wallet <WALLET>  Your wallet
  -h, --help             Print help
```

## `sion agent dataset build`

```text
Build a version, as the dataset's curator, from the example files in a directory: each is checked against its contribution and what it says checked it, and those taken are written as a file to train on, with a manifest, and committed to in a claim

Usage: sion agent dataset build [OPTIONS] --from <DIR> --wallet <WALLET> <DATASET>

Arguments:
  <DATASET>  The dataset (its card's claim)

Options:
      --from <DIR>       The directory of example files
      --node <NODE>      The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --out <DIR>        The directory to write the version in [default: .]
      --wallet <WALLET>  Your wallet
  -h, --help             Print help
```

## `sion agent dataset check`

```text
Check a file of examples against its manifest and the ledger, as someone about to train on it: that the examples are the version's, that each was contributed on the dataset's terms, and what checked each

Usage: sion agent dataset check [OPTIONS] <MANIFEST> <EXAMPLES>

Arguments:
  <MANIFEST>  The manifest
  <EXAMPLES>  The examples

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent dataset show`

```text
A dataset's card and its latest version

Usage: sion agent dataset show [OPTIONS] <DATASET>

Arguments:
  <DATASET>  The dataset (its card's claim)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent improve improver`

```text
Improve the improver: the proposer's instructions and settings, graded by how much the agent improves on tasks it was not shown when the improver writes its changes, over problems (pairs of task sets). The improver proposes its own changes, under its own instructions; one is kept only when the agent improves more with it, beyond chance, on the problems given and then on problems held out, which no proposer is shown. With --show, the improver as it is

Usage: sion agent improve improver [OPTIONS] <PROFILE>

Arguments:
  <PROFILE>  The profile

Options:
      --node <NODE>                    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --show                           Show the profile's improver, and do nothing else
      --proposer <MODEL>               The model that proposes the agent's changes
      --meta-proposer <MODEL>          The model the improver proposes its own changes with (default: the proposer)
      --problem <TRAIN,HELD>           A problem to grade on: `<tasks to choose on>,<held-out tasks>` (repeat)
      --held-out-problem <TRAIN,HELD>  A problem held out, to confirm changes on (repeat)
      --rounds <ROUNDS>                Rounds of changes to the improver, at most [default: 2]
      --agent-rounds <AGENT_ROUNDS>    Rounds of the agent's loop on each problem [default: 2]
      --runs <RUNS>                    How many times each task is run [default: 2]
      --budget-tokens <BUDGET_TOKENS>  The tokens each run of a task may use [default: 20000]
      --confirmations <CONFIRMATIONS>  The confirmations of changes to the improver this allows [default: 4]
      --seed <SEED>                    The seed of the archives' draws (default: drawn)
      --apply                          Make a kept change the profile's improver
  -h, --help                           Print help
```

See [Improving itself](../agent/self-improvement.md#the-improver-improved).

## `sion agent improve code`

```text
Let the agent change its own code: a coding agent edits a copy of the source, its commands in the sandbox; a change that keeps to the paths it may touch, builds, passes its tests, is better on the seal's tasks to choose on and is confirmed on the sealed tasks, the candidate scored by this harness, becomes a release (published with --wallet). Nothing is installed

Usage: sion agent improve code [OPTIONS] --source <DIR> --coder <MODEL> <PROFILE>

Arguments:
  <PROFILE>  The profile

Options:
      --node <NODE>                    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --source <DIR>                   The source: a git checkout, whose last commit is the code as it stands
      --coder <MODEL>                  The model that changes the code
      --build <BUILD>                  The build, run with /bin/sh at the source's root in the sandbox [default: "cargo build --offline -p sion-cli"]
      --test <TEST>                    The tests [default: "cargo test --offline -p sion-agent --lib"]
      --candidate <CANDIDATE>          The program that runs a case, and its arguments, at the root [default: "target/debug/sion agent case --stdio"]
      --allow <PATH>                   A path a change may touch (repeat; default: the agent's runtime, crates/sion-agent/src/, apart from what scores or guards it)
      --max-files <MAX_FILES>          The most files a change may touch [default: 8]
      --max-lines <MAX_LINES>          The most lines it may add and remove [default: 400]
      --readable <DIR>                 A directory the sandbox shows read-only, a toolchain's (repeat)
      --env <NAME=VALUE>               A variable the build and the tests are given: NAME=VALUE (repeat)
      --build-secs <BUILD_SECS>        Seconds the build and the tests may each take [default: 1800]
      --rounds <ROUNDS>                Rounds, at most [default: 2]
      --runs <RUNS>                    How many times each task is run [default: 2]
      --budget-tokens <BUDGET_TOKENS>  The tokens each run of a task may use [default: 20000]
      --wallet <WALLET>                Publish each release as this wallet's claim
  -h, --help                           Print help
```

See [Improving itself](../agent/self-improvement.md#changing-its-own-code).

## `sion agent improve attest`

```text
Rebuild and score a release someone published: the source at its commit with its patch, and as it stood, each built and tested in the sandbox and scored by this harness on the sealed tasks it was confirmed on; publish what you found if it holds up

Usage: sion agent improve attest [OPTIONS] --source <DIR> --patch <FILE> --tasks <FILE> --wallet <WALLET> <RELEASE>

Arguments:
  <RELEASE>  The release's claim

Options:
      --node <NODE>                    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --source <DIR>                   A checkout of the source holding the release's commit
      --patch <FILE>                   The release's patch
      --tasks <FILE>                   The sealed tasks it was confirmed on, as its publisher gives them
      --wallet <WALLET>                The wallet that attests
      --readable <DIR>                 A directory the sandbox shows read-only (repeat)
      --env <NAME=VALUE>               A variable the build and the tests are given (repeat)
      --allow <PATH>                   A path a change may touch, as the publisher allowed (repeat)
      --build-secs <BUILD_SECS>        Seconds the build and the tests may each take [default: 1800]
      --runs <RUNS>                    How many times each task is run [default: 2]
      --budget-tokens <BUDGET_TOKENS>  The tokens each run of a task may use [default: 20000]
      --adjudicator <ADJUDICATOR>      The adjudicator who can rule against the attestation (its atom's id, 64 hex digits): the attestation is then a claim on your bond `agent-attester`, which anyone who rebuilds the release and finds otherwise can challenge
      --bond <USD>                     Post your bond `agent-attester` with this many USD first
  -h, --help                           Print help
```

## `sion agent improve install`

```text
Apply a release to a checkout of the source, only if its patch is the one published and verifiers you name attest it (or, with --undo, take it back out). Build and install the result as you build sion

Usage: sion agent improve install [OPTIONS] --source <DIR> --patch <FILE> <PROFILE> <RELEASE>

Arguments:
  <PROFILE>  The profile whose lineage records it
  <RELEASE>  The release's claim

Options:
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --source <DIR>         The checkout
      --patch <FILE>         The release's patch
      --attestation <CLAIM>  An attestation's claim (repeat)
      --verifier <ACCOUNT>   An account, or a wallet's name, whose attestation counts (repeat)
      --need <NEED>          Attestations needed, by different verifiers [default: 1]
      --bonded               Count only attestations on their attester's bond, unbroken and holding at least --least-bond, under an adjudicator you name
      --adjudicator <ATOM>   An adjudicator whose rulings you accept, for --bonded (its atom's id, 64 hex digits; repeat)
      --least-bond <USD>     The least an attester's bond must hold, in USD, for --bonded [default: 10]
      --allow <PATH>         A path the patch may touch (repeat; default: the agent's runtime, crates/sion-agent/src/, apart from what scores or guards it). The patch is held to these whatever its publisher and attesters allowed
      --undo                 Take the release back out instead
  -h, --help                 Print help
```

## `sion agent find`

```text
Find the best agent for a task among those you name: how well each one's offer fits the task (BM25), weighed against its record on the ledger, held to what you need (an enclave that checks out, a price)

Usage: sion agent find [OPTIONS] <TASK>...

Arguments:
  <TASK>...  The task, in words

Options:
      --listing <OWNER/NAME>      A rental listing to consider: `<owner>/<name>`, the owner an account (64 hex digits) or a devnet wallet's name (repeat for more). Its offer is its description, its price what it charges per thousand output tokens
      --node <NODE>               The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --agent <AGENT=OFFER>       An agent to consider, with a line of what it offers: `<agent>=<offer>`, the agent's id in 64 hex digits (repeat for more)
      --enclave                   Only agents in an enclave whose attestation checks out
      --max-per-1k-output <USD>   At most this USD per thousand output tokens: an agent with no price is left out
      --measurement <HEX>         The measurement of the enclave code you trust (48 bytes in hex); without it, each listing's own is taken and only the rest checked
      --simulated-platform <HEX>  Take a simulated enclave with this platform key (hex): for devnets, where there is no SEV-SNP hardware
      --from <FROM>               Read the records from this Pulse on [default: 0]
      --limit <LIMIT>             How many to show, best first [default: 5]
  -h, --help                      Print help
```

Records are read from the node's events, unproven (`agent:agent-record`); a listing's attestation is checked here. See [An agent's record, and finding one for a task](../agent/teams.md#an-agents-record-and-finding-one-for-a-task).

## `sion agent x-mentions`

```text
Answer the posts on X that mention your account, in their threads: each answer from the posts it replies to or quotes and the web search's results for its words, citing them, with their addresses in its last post; an answer that rests on nothing it was given is not posted. Needs X_ACCESS_TOKEN with tweet.read, tweet.write and users.read

Usage: sion agent x-mentions [OPTIONS] <PROFILE>

Arguments:
  <PROFILE>  The profile whose agent answers

Options:
      --node <NODE>              The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --once                     Poll once, then stop
      --poll <POLL>              Seconds between polls [default: 60]
      --per-day <PER_DAY>        Mentions answered in a UTC day, at most [default: 50]
      --per-person <PER_PERSON>  One person's mentions answered in a UTC day, at most [default: 5]
      --max-posts <MAX_POSTS>    Posts in a reply, at most [default: 3]
      --sources <SOURCES>        required: an answer is posted only if it cites a source it was given; optional: one with none is posted too [default: required]
      --read <READ>              How many of the search's first results are read as pages [default: 2]
      --from <FROM>              Answer only these people (ids or usernames, comma-separated)
      --dry-run                  Show the replies it would post, and post none
  -h, --help                     Print help
```

See [Mentions on X](../agent/messengers.md#mentions-on-x).

## `sion agent rent`

```text
Rent a profile's agent out to other people by the token, in an enclave they can check, or rent someone else's

Usage: sion agent rent [OPTIONS] <COMMAND>

Commands:
  serve  Rent a profile's agent out by the token: attest the enclave it runs in, publish the listing, and serve sealed requests. Renters' sessions get none of this profile's memory, files or reach, and the model runs on your own account, priced into what you charge
  use    Rent an owner's agent: check its listing's attestation, escrow a deposit in a payment stream, ask it, and pay for the tokens it used
  show   Show an owner's listing and check its attestation: what it costs, what it runs, and whether its enclave checks out
  help   Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

See [Renting your agent out](../agent/renting.md).

## `sion agent rent serve`

```text
Rent a profile's agent out by the token: attest the enclave it runs in, publish the listing, and serve sealed requests. Renters' sessions get none of this profile's memory, files or reach, and the model runs on your own account, priced into what you charge

Usage: sion agent rent serve [OPTIONS] --name <NAME> --per-1k-input <USD> --per-1k-output <USD> <PROFILE>

Arguments:
  <PROFILE>  The profile whose agent is rented out

Options:
      --name <NAME>                The listing's name, yours to choose (`rental:<name>` in your index)
      --node <NODE>                The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --per-1k-input <USD>         USD per thousand input tokens (as a decimal, 0.003)
      --per-1k-output <USD>        USD per thousand output tokens
      --unit <USD>                 One step of a renter's payment stream, in USD: prices are rounded up to a whole number of these [default: 0.001]
      --min-deposit <USD>          The least a renter must escrow to start, in USD [default: 1]
      --description <DESCRIPTION>  What the agent is for, in a line, which renters search [default: ""]
      --listen-at <ADDRESS>        Where to listen (127.0.0.1:0 picks a free port) [default: 127.0.0.1:8795]
      --requests <N>               Serve this many requests, then stop (for trying it out)
  -h, --help                       Print help
```

## `sion agent rent use`

```text
Rent an owner's agent: check its listing's attestation, escrow a deposit in a payment stream, ask it, and pay for the tokens it used

Usage: sion agent rent use [OPTIONS] --at <URL> <OWNER> <NAME> <MESSAGE>

Arguments:
  <OWNER>    The owner's account (64 hex digits, or a devnet wallet's name)
  <NAME>     The listing's name
  <MESSAGE>  What to ask it

Options:
      --at <URL>                  Where its enclave serves (http://host:port)
      --node <NODE>               The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --wallet <WALLET>           Your wallet, which pays [default: alice]
      --deposit <USD>             What to escrow, in USD [default: 1]
      --pulses <PULSES>           How long the stream stays open, in Pulses [default: 10000]
      --measurement <HEX>         The measurement of the enclave you trust (48 bytes in hex)
      --simulated-platform <HEX>  Take a simulated enclave with this platform key (hex)
      --receipts <FILE>           Write the receipts its enclave signed of which models answered to this file, with its attestation (`sion agent receipts check`)
  -h, --help                      Print help
```

## `sion agent rent show`

```text
Show an owner's listing and check its attestation: what it costs, what it runs, and whether its enclave checks out

Usage: sion agent rent show [OPTIONS] <OWNER> <NAME>

Arguments:
  <OWNER>  The owner's account (64 hex digits, or a devnet wallet's name)
  <NAME>   The listing's name

Options:
      --measurement <HEX>         The measurement of the enclave you trust (48 bytes in hex); without it the attestation's own measurement is shown, unchecked
      --node <NODE>               The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --simulated-platform <HEX>  Take a simulated enclave with this platform key (hex): for devnets, where there is no SEV-SNP hardware
  -h, --help                      Print help
```

## `sion agent sell`

```text
Sell an agent's work to other agents through the Agentic Commerce Protocol's checkout, paid in USD on this chain, and take their users' AP2 mandates

Usage: sion agent sell [OPTIONS] <COMMAND>

Commands:
  serve    Serve ACP's checkout and cart API for a catalog (agent work at fixed prices), paid to a wallet in USD on this chain or by card: other agents make a checkout session, pay it, and get an order. Sessions, carts and orders are kept across restarts
  orders   List the orders a merchant made (its admin API)
  fulfill  Mark an order delivered: the buyer's platform is told by webhook
  refund   Refund an order, all of it or some cents of it: back to the payer on the ledger, or at the PSP for a card
  feed     Publish a catalog as an ACP product feed: to an agent's Feed API, or as the offline snapshot (`metadata.json`, `products.jsonl`)
  help     Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

See [Selling and buying: ACP and AP2](../agent/commerce.md).

## `sion agent sell serve`

```text
Serve ACP's checkout and cart API for a catalog (agent work at fixed prices), paid to a wallet in USD on this chain or by card: other agents make a checkout session, pay it, and get an order. Sessions, carts and orders are kept across restarts

Usage: sion agent sell serve [OPTIONS] --pay-to <PAY_TO> --name <NAME> <CATALOG>

Arguments:
  <CATALOG>  The catalog: `{"offers": [{"id", "name", "description", "price_usd"}]}`, prices in whole cents

Options:
      --node <NODE>                The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --pay-to <PAY_TO>            Who is paid on the ledger: a wallet's name, or an account (64 hex digits)
      --name <NAME>                The merchant's name, as buyers and their mandates see it
      --listen-at <ADDRESS>        Where to listen [default: 127.0.0.1:8796]
      --public-url <URL>           The URL buyers reach it at (its API base and order links); http://<listen-at> if not given
      --tokens-env <NAME>          The environment variable holding the buyers' bearer tokens, comma-separated, each `name=token` or a bare token, at least 16 characters [default: SION_ACP_TOKENS]
      --client-keys <FILE>         The buyers' request-signing keys: `{"<buyer's name>": <JWK>}` (EC P-256 for ES256, OKP Ed25519, or oct for HMAC-SHA256)
      --require-signatures         Refuse a request from a buyer with a key that is not signed
      --chain <HEX>                The chain identity payments are verified against (64 hex digits), which the node must serve; the node's word if not given
      --state <FILE>               Where the merchant's state is kept (sessions, carts, orders, keys, the webhook outbox); `acp-merchant-state.json` in the configuration directory if not given
      --ap2-trust <FILE>           Take AP2 mandates whose roots these keys signed (a JWK Set file, `{"keys": [{"kid", "kty": "EC", "crv": "P-256", …}]}`)
      --ap2-roots <FILE>           Take AP2 mandates whose roots carry a certificate chain (`x5c`) to one of these certificates (PEM)
      --require-mandate            Complete a session only with an AP2 checkout mandate
      --webhook-url <URL>          Send order webhooks here (the agent platform's `/agentic_checkout/webhooks/order_events`)
      --webhook-secret-env <NAME>  The environment variable holding the webhook signing secret [default: SION_ACP_WEBHOOK_SECRET]
      --admin-token-env <NAME>     The environment variable holding the admin token (fulfill, refund, list orders); no admin API without it [default: SION_ACP_ADMIN_TOKEN]
      --refunds-from <WALLET>      Refund ledger payments from this wallet (the payee's)
      --card-psp <URL>             Take cards delegated to this ACP PSP (its base URL)
      --card-psp-token-env <NAME>  The environment variable holding this merchant's token at the PSP [default: SION_ACP_PSP_TOKEN]
      --stripe-key-env <NAME>      Take Stripe shared payment tokens, charged with the secret key in this environment variable
      --card-merchant-id <ID>      The merchant's id at its PSP (delegated cards are scoped to it)
      --card-brands <CARD_BRANDS>  The card brands taken [default: visa,mastercard,amex,discover]
      --three-ds <FILE>            Ask for 3-D Secure on card payments: ACP's `AuthenticationMetadata` (acquirer details, directory server, flow preference) as JSON
      --requests <N>               Serve this many requests, then stop (for trying it out)
  -h, --help                       Print help
```

## `sion agent sell orders`

```text
List the orders a merchant made (its admin API)

Usage: sion agent sell orders [OPTIONS] <URL>

Arguments:
  <URL>  The merchant's API base URL

Options:
      --admin-token-env <NAME>  The environment variable holding the merchant's admin token [default: SION_ACP_ADMIN_TOKEN]
      --node <NODE>             The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help                    Print help
```

## `sion agent sell fulfill`

```text
Mark an order delivered: the buyer's platform is told by webhook

Usage: sion agent sell fulfill [OPTIONS] <URL> <ORDER>

Arguments:
  <URL>    The merchant's API base URL
  <ORDER>  The order's id

Options:
      --access-url <URL>        Where the buyer gets what was bought
      --node <NODE>             The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --license-key <KEY>       A license key, if what was bought has one
      --description <TEXT>      What was delivered, in a few words
      --admin-token-env <NAME>  The environment variable holding the merchant's admin token [default: SION_ACP_ADMIN_TOKEN]
  -h, --help                    Print help
```

## `sion agent sell refund`

```text
Refund an order, all of it or some cents of it: back to the payer on the ledger, or at the PSP for a card

Usage: sion agent sell refund [OPTIONS] <URL> <ORDER>

Arguments:
  <URL>    The merchant's API base URL
  <ORDER>  The order's id

Options:
      --cents <CENTS>           How much, in cents (what is left of the order if not given)
      --node <NODE>             The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --reason <TEXT>           Why
      --admin-token-env <NAME>  The environment variable holding the merchant's admin token [default: SION_ACP_ADMIN_TOKEN]
  -h, --help                    Print help
```

## `sion agent sell feed`

```text
Publish a catalog as an ACP product feed: to an agent's Feed API, or as the offline snapshot (`metadata.json`, `products.jsonl`)

Usage: sion agent sell feed [OPTIONS] --name <NAME> --public-url <URL> <CATALOG>

Arguments:
  <CATALOG>  The catalog, as `sell serve` takes it

Options:
      --name <NAME>       The merchant's name, as the feed shows the seller
      --node <NODE>       The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --public-url <URL>  The merchant's storefront (its checkout API's base URL)
      --to <URL>          The agent's Feed API to publish to
      --token-env <NAME>  The environment variable holding the token the agent gave [default: SION_ACP_FEED_TOKEN]
      --feed <ID>         The feed to fill (made if not given)
      --country <CODE>    The country the feed is for (ISO 3166-1 alpha-2)
      --export <DIR>      Write the offline snapshot to this directory instead
  -h, --help              Print help
```

## `sion agent buy`

```text
Buy from a merchant that serves ACP's checkout: make a checkout session, check its total, pay it on Siontology or by a card delegated to the merchant's PSP (with 3-D Secure if asked)

Usage: sion agent buy [OPTIONS] --max-usd <USD> <URL> <ITEMS>...

Arguments:
  <URL>       The merchant's API base URL
  <ITEMS>...  The offers bought, by id (an id twice buys two)

Options:
      --node <NODE>                The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --wallet <WALLET>            The wallet that pays (the agent's, with --charter) [default: alice]
      --charter <ID>               Pay under this Agent Charter, from --allotment
      --allotment <ID>             The agent's allotment under --charter
      --max-usd <USD>              The most to pay, in USD
      --chain <HEX>                The chain identity payments are checked against (64 hex digits), which the node must serve; the node's word if not given
      --token-env <NAME>           The environment variable holding the merchant's token [default: SION_ACP_TOKEN]
      --sign-key <FILE>            Sign requests with this key (PKCS#8, PEM or DER: P-256 for ES256, or Ed25519)
      --sign-secret-env <NAME>     Sign requests with HMAC-SHA256, the secret in this environment variable
      --card <FILE>                Pay by card instead: ACP's `PaymentMethodCard` as JSON (it is sent to the PSP, never kept)
      --psp <URL>                  The PSP the card is delegated to (its base URL)
      --psp-token-env <NAME>       The environment variable holding the PSP's token [default: SION_PSP_TOKEN]
      --three-ds <URL>             The 3-D Secure provider, when the merchant or the issuer asks
      --three-ds-token-env <NAME>  The environment variable holding the 3-D Secure provider's token [default: SION_3DS_TOKEN]
      --notification-url <URL>     Where the issuer posts back [default: https://localhost/3ds/notify]
      --simulated-otp <CODE>       Answer a simulated issuer's challenge with this code (devnets only: a real issuer's challenge is the buyer's to answer)
  -h, --help                       Print help
```

See [Selling and buying: ACP and AP2](../agent/commerce.md).

## `sion agent erc8004`

```text
The agent in ERC-8004's registries on EVM chains: its registration file, naming its Siontology DID; the calls that register it, bind its wallet, give it feedback and answer a validation, written out for a wallet to send

Usage: sion agent erc8004 [OPTIONS] <COMMAND>

Commands:
  card      Write the agent's registration file: its name, what it does, its services (an A2A card, an MCP server, a web page) and its Siontology DID, and its entry in a registry once it has one. With --data-uri, the file as a data: URI too, to register wholly on chain
  register  The call that registers an agent in the Identity Registry, its registration file at a URI (https://, ipfs:// or data:), for its owner to send
  wallet    Sign, as the wallet whose key is in EVM_PRIVATE_KEY, that it is an agent's wallet (EIP-712, as setAgentWallet checks), and write the call the agent's owner sends
  feedback  Feedback for an agent, as a client gives it: its file, to put at a URI, and the call (giveFeedback) the client sends, naming the file by that URI and its Keccak digest
  validate  A validator's answer to a validation request: the call (validationResponse) the validator sends, naming what it found by its URI and Keccak digest
  check     Check a registration file: its type, its name and its entries
  help      Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent erc8004 card`

```text
Write the agent's registration file: its name, what it does, its services (an A2A card, an MCP server, a web page) and its Siontology DID, and its entry in a registry once it has one. With --data-uri, the file as a data: URI too, to register wholly on chain

Usage: sion agent erc8004 card [OPTIONS] <PROFILE>

Arguments:
  <PROFILE>  The profile

Options:
      --description <DESCRIPTION>  What the agent does, in a sentence or two (public)
      --node <NODE>                The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --a2a <URL>                  Its A2A card's address (…/.well-known/agent-card.json)
      --mcp <URL>                  Its MCP server's address
      --web <URL>                  Its web page
      --image <URL>                A picture of it [default: ""]
      --x402                       Say that it takes x402 payments
      --agent-id <AGENT_ID>        Its id in the Identity Registry, once registered
      --chain <CHAIN>              The chain (EIP-155 id) [default: 1]
      --registry <REGISTRY>        The Identity Registry (default: the one deployed on that chain)
      --out <FILE>                 Where to write it (default: standard output)
      --data-uri                   Also print it as a data: URI
  -h, --help                       Print help
```

## `sion agent erc8004 register`

```text
The call that registers an agent in the Identity Registry, its registration file at a URI (https://, ipfs:// or data:), for its owner to send

Usage: sion agent erc8004 register [OPTIONS] --uri <URI>

Options:
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --uri <URI>            The registration file's URI
      --chain <CHAIN>        The chain (EIP-155 id) [default: 1]
      --registry <REGISTRY>  The Identity Registry (default: the one deployed on that chain)
  -h, --help                 Print help
```

## `sion agent erc8004 wallet`

```text
Sign, as the wallet whose key is in EVM_PRIVATE_KEY, that it is an agent's wallet (EIP-712, as setAgentWallet checks), and write the call the agent's owner sends

Usage: sion agent erc8004 wallet [OPTIONS] --owner <OWNER> <AGENT_ID>

Arguments:
  <AGENT_ID>  The agent's id in the Identity Registry

Options:
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --owner <OWNER>        The agent's owner, who sends the call
      --within <WITHIN>      Seconds the signature is good for (at most 300, by the chain's clock) [default: 240]
      --chain <CHAIN>        The chain (EIP-155 id) [default: 1]
      --registry <REGISTRY>  The Identity Registry (default: the one deployed on that chain)
  -h, --help                 Print help
```

## `sion agent erc8004 feedback`

```text
Feedback for an agent, as a client gives it: its file, to put at a URI, and the call (giveFeedback) the client sends, naming the file by that URI and its Keccak digest

Usage: sion agent erc8004 feedback [OPTIONS] --value <VALUE> --client <CLIENT> <AGENT_ID>

Arguments:
  <AGENT_ID>  The agent's id in the Identity Registry

Options:
      --node <NODE>              The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --value <VALUE>            The value, a decimal (99.77, -3.2, 1)
      --tag1 <TAG1>              What it is about (`starred`, `uptime`, …) [default: ""]
      --tag2 <TAG2>              More of what it is about [default: ""]
      --endpoint <ENDPOINT>      The endpoint it is about [default: ""]
      --client <CLIENT>          The client's address
      --uri <URI>                Where the file will be (default: none, and no digest)
      --chain <CHAIN>            The chain (EIP-155 id) [default: 1]
      --registry <REGISTRY>      The Identity Registry the agent is in (default: the one deployed on that chain)
      --reputation <REPUTATION>  The Reputation Registry (default: the one deployed there)
      --out <FILE>               Where to write the file
  -h, --help                     Print help
```

## `sion agent erc8004 validate`

```text
A validator's answer to a validation request: the call (validationResponse) the validator sends, naming what it found by its URI and Keccak digest

Usage: sion agent erc8004 validate [OPTIONS] --request <REQUEST> --response <RESPONSE> --registry <REGISTRY>

Options:
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --request <REQUEST>    The request's digest (0x and 64 hex digits)
      --response <RESPONSE>  The answer, 0 (failed) to 100 (passed)
      --uri <URI>            Where what was found is [default: ""]
      --found <FILE>         The file of what was found, whose digest the call names
      --tag <TAG>            A tag for the answer [default: ""]
      --chain <CHAIN>        The chain (EIP-155 id) [default: 1]
      --registry <REGISTRY>  The Validation Registry (no address is listed as deployed yet)
  -h, --help                 Print help
```

## `sion agent erc8004 check`

```text
Check a registration file: its type, its name and its entries

Usage: sion agent erc8004 check [OPTIONS] <FILE>

Arguments:
  <FILE>  The file

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

See [On Ethereum's agent registries: ERC-8004](../agent/erc8004.md).

## `sion agent c2pa`

```text
Content Credentials (C2PA) for what the agent makes: a manifest beside a file that binds its bytes and names the agent that made it, signed with your certificate; and checking one

Usage: sion agent c2pa [OPTIONS] <COMMAND>

Commands:
  sign    Sign a file as made by the profile's agent: a C2PA manifest (Content Credentials) kept beside it as <file>.c2pa, binding its bytes, saying that trained algorithmic media made it (or edited it, with --edited), by Siontology, and naming the agent by its DID
  verify  Check a file's Content Credentials against the file: the claim's signature, every assertion's hash, and the data hash; with --trust, that the signer's chain leads to a root you trust
  help    Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent c2pa sign`

```text
Sign a file as made by the profile's agent: a C2PA manifest (Content Credentials) kept beside it as <file>.c2pa, binding its bytes, saying that trained algorithmic media made it (or edited it, with --edited), by Siontology, and naming the agent by its DID

Usage: sion agent c2pa sign [OPTIONS] --profile <PROFILE> --cert <PEM> --key <PEM> <FILE>

Arguments:
  <FILE>  The file

Options:
      --node <NODE>        The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --profile <PROFILE>  The profile whose agent made it
      --cert <PEM>         The signer's certificate chain, PEM, the signer's first
      --key <PEM>          The signer's P-256 key, PKCS#8 PEM
      --edited             It was edited, not made from nothing
      --out <FILE>         Where to write the manifest (default: <file>.c2pa)
  -h, --help               Print help
```

## `sion agent c2pa verify`

```text
Check a file's Content Credentials against the file: the claim's signature, every assertion's hash, and the data hash; with --trust, that the signer's chain leads to a root you trust

Usage: sion agent c2pa verify [OPTIONS] <FILE>

Arguments:
  <FILE>  The file

Options:
      --manifest <FILE>  Its manifest (default: <file>.c2pa)
      --node <NODE>      The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --trust <PEM>      A root certificate you trust, PEM (repeat)
  -h, --help             Print help
```

See [Content Credentials: C2PA](../agent/c2pa.md).

## `sion agent prices`

```text
The models' prices that runs are counted at: Anthropic's list prices for Claude, built in, and yours from `$SION_HOME/prices.json` (an object of model names, each with `input`, `output` and optionally `cache_read` and `cache_write`, in dollars a million tokens), in their place where both price a model

Usage: sion agent prices [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent commerce`

```text
The services around an ACP checkout: a PSP's delegate payment vault, a 3-D Secure provider, an agent's Feed API, a webhook receiver

Usage: sion agent commerce [OPTIONS] <COMMAND>

Commands:
  psp       Serve a PSP's delegate payment API: agents vault a card for one checkout, merchants redeem the token once. Its acquirer is simulated: no card network is reached and no money moves
  three-ds  Serve ACP's Delegate Authentication API (3-D Secure). Its issuer is simulated: for devnets and tests, its results say so
  feeds     Serve an agent's Feed API: merchants publish their products to it
  webhooks  Receive a merchant's order webhooks: each verified event is printed, one JSON line each
  keygen    Make a request-signing key: written to a file (PKCS#8, readable only by its owner), its public JWK printed for the services to be given
  help      Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

See [Selling and buying: ACP and AP2](../agent/commerce.md).

## `sion agent commerce psp`

```text
Serve a PSP's delegate payment API: agents vault a card for one checkout, merchants redeem the token once. Its acquirer is simulated: no card network is reached and no money moves

Usage: sion agent commerce psp [OPTIONS]

Options:
      --listen-at <ADDRESS>   Where to listen [default: 127.0.0.1:8797]
      --node <NODE>           The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --public-url <URL>      The URL it is reached at
      --agents-env <NAME>     The environment variable holding the agents' tokens [default: SION_PSP_AGENT_TOKENS]
      --merchants-env <NAME>  The environment variable holding the merchants' tokens, each `merchant_id=token` [default: SION_PSP_MERCHANT_TOKENS]
      --client-keys <FILE>    The agents' request-signing keys (`{"<name>": <JWK>}`)
      --require-signatures    Refuse an agent's request with a key that is not signed
      --requests <N>          Serve this many requests, then stop
  -h, --help                  Print help
```

## `sion agent commerce three-ds`

```text
Serve ACP's Delegate Authentication API (3-D Secure). Its issuer is simulated: for devnets and tests, its results say so

Usage: sion agent commerce three-ds [OPTIONS]

Options:
      --listen-at <ADDRESS>  Where to listen [default: 127.0.0.1:8798]
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --public-url <URL>     The URL it is reached at (its issuer pages are under it)
      --tokens-env <NAME>    The environment variable holding the agents' tokens [default: SION_3DS_TOKENS]
      --client-keys <FILE>   The agents' request-signing keys (`{"<name>": <JWK>}`)
      --require-signatures   Refuse an agent's request with a key that is not signed
      --requests <N>         Serve this many requests, then stop
  -h, --help                 Print help
```

## `sion agent commerce feeds`

```text
Serve an agent's Feed API: merchants publish their products to it

Usage: sion agent commerce feeds [OPTIONS]

Options:
      --listen-at <ADDRESS>  Where to listen [default: 127.0.0.1:8799]
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --tokens-env <NAME>    The environment variable holding the merchants' tokens [default: SION_ACP_FEED_TOKENS]
      --state <FILE>         Where the feeds are kept; `acp-feeds.json` in the configuration directory if not given
      --requests <N>         Serve this many requests, then stop
  -h, --help                 Print help
```

## `sion agent commerce webhooks`

```text
Receive a merchant's order webhooks: each verified event is printed, one JSON line each

Usage: sion agent commerce webhooks [OPTIONS]

Options:
      --listen-at <ADDRESS>  Where to listen [default: 127.0.0.1:8800]
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --secret-env <NAME>    The environment variable holding the signing secret [default: SION_ACP_WEBHOOK_SECRET]
      --requests <N>         Serve this many requests, then stop
  -h, --help                 Print help
```

## `sion agent commerce keygen`

```text
Make a request-signing key: written to a file (PKCS#8, readable only by its owner), its public JWK printed for the services to be given

Usage: sion agent commerce keygen [OPTIONS] <OUT>

Arguments:
  <OUT>  Where to write it

Options:
      --ed25519      Ed25519 instead of ECDSA P-256 (ES256)
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent meeting`

```text
Take a meeting's notes: dial in by phone through Twilio (its number and keys) or hear it here (the microphone, and what this machine plays), the transcript written as it goes, and the profile's agent writing up the decisions, the action items and what is still open

Usage: sion agent meeting [OPTIONS] <PROFILE>

Arguments:
  <PROFILE>  The profile whose agent writes the notes

Options:
      --dial <NUMBER>        Dial in to the meeting by phone, through Twilio: its dial-in number (TWILIO_ACCOUNT_SID, TWILIO_AUTH_TOKEN, and TWILIO_NUMBER to call from)
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --digits <KEYS>        The keys pressed once the call is answered: the meeting id and passcode, w waiting half a second (wwww123456#ww987#)
      --listen-at <ADDRESS>  Where Twilio's transcriptions are taken (behind --public-url) [default: 0.0.0.0:8790]
      --public-url <URL>     The public address of --listen-at, which Twilio posts to (https://…)
      --language <LANGUAGE>  The language spoken (en-US by default) [default: en-US]
      --here                 Hear the meeting on this machine: its microphone, and with --monitor what it plays
      --join <LINK>          Join the meeting by its link, as a guest that listens: Chromium opens it with a silent microphone and a black camera, gives the guest's name and asks to join, and what the meeting plays is heard through a PulseAudio or PipeWire sink of its own (pactl, parec)
      --name <NAME>          The guest's name, as the meeting shows it [default: "Notes (sion)"]
      --hear <PROVIDER>      What hears what is said here, or in a meeting joined by its link: openai:<model>, groq:<model>, deepgram:<model>, elevenlabs:<model>, whisper:<model file>, local:<model> or command:<program>
      --listen <COMMAND>     The program that gives the microphone's sound (raw 16-bit mono at 16 kHz), as for a live conversation
      --monitor [<COMMAND>]  Hear what this machine plays too: the meeting app's sound, from PulseAudio's or PipeWire's monitor (parec), or the program given
      --minutes <MINUTES>    The longest the meeting is heard, in minutes [default: 120]
      --title <TITLE>        What the meeting is: in the notes' file name and title [default: meeting]
      --deliver <CHANNEL>    Where the notes are sent too, as a schedule's are (telegram:<chat>, file:<path>, …)
      --diarize <PROVIDER>   Record what is heard here (or by the guest), and at the end have it heard again by a transcriber that tells speakers apart: openai:<model> (gpt-4o-transcribe-diarize), deepgram:<model> or elevenlabs:<model>. The notes then say who said what
      --keep-recording       Keep the recordings beside the notes, rather than removing them once they are heard
      --speak <SPEECH>       Give the guest that joins by the link a voice, and what speaks for it: openai:<model>:<voice>, elevenlabs:<voice id>[:<model>], deepgram:<voice model>, command:<program>, or on this machine piper:<model file> or local:<model>:<voice>. It answers a line that names it and asks it something, by the profile's agent from the transcript alone, into the meeting (a PulseAudio microphone of its own) and into the transcript. Such a guest has no camera
      --answer-to <PHRASES>  The words that name the guest, `|` between alternatives (default: its --name, and its --name without what is in brackets)
  -h, --help                 Print help
```

See [Meeting notes](../agent/voice.md#meeting-notes).

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

## `sion agent work`

```text
Work a profile's agent takes on by itself: the GitHub repositories and chats it watches, who may ask it for work there, the kinds of work it may take on unasked (recorded only, until you let it act), and the runner that does the work

Usage: sion agent work [OPTIONS] <COMMAND>

Commands:
  rules      Show a profile's rules for work: the mode, the kinds of work its agent may take on by itself, and what it watches; or change them
  watch      Watch a GitHub repository or a chat for work, and say who may start it there; or stop watching it
  run        Take work on as it comes: read the watches, run the work in its slots, and send what it says
  list       The work taken on, and what each item waits for
  show       One item: its request, its mandate, what reached it, what it reported, and its messages
  decisions  What became of each trigger, newest last: taken on, ignored, gone to open work, or not weighed; and in observe mode, what would have been taken on
  answer     Answer an item's question, or tell it what to do, as its operator
  approve    Approve the call an item's session waits on, or decline it
  stop       Stop an item: its mandate is withdrawn, on the ledger too
  retry      Try again: the messages of an item that could not be sent, or an item that failed or is blocked
  help       Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

See [Work it takes on by itself](../agent/work.md). A profile's work is kept in `$SION_HOME/agents/work/<profile>`.

## `sion agent work rules`

```text
Show a profile's rules for work: the mode, the kinds of work its agent may take on by itself, and what it watches; or change them

Usage: sion agent work rules [OPTIONS] <PROFILE>

Arguments:
  <PROFILE>  The agent profile

Options:
      --mode <MODE>            off (only what it is asked for), observe (weigh the rest, take none of it on, record what it would have: the default), or auto (take on what a rule covers)
      --node <NODE>            The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --rule <ID=INSTRUCTION>  A kind of work it may take on without being asked: `id=instruction`, the instruction saying what it covers and how the work is done
      --drop <ID>              Drop a rule
  -h, --help                   Print help
```

The mode, the rules and the watches have a version, the first 16 hex digits of their digest, which every decision and mandate records.

## `sion agent work watch`

```text
Watch a GitHub repository or a chat for work, and say who may start it there; or stop watching it

Usage: sion agent work watch [OPTIONS] <PROFILE> <WATCH>

Arguments:
  <PROFILE>  The agent profile
  <WATCH>    github:<owner>/<repo>, or <messenger>:<chat> (telegram:-100123, slack:C042)

Options:
      --allow <NAME>                 Let them ask the agent for work here, as the watch names them (GitHub logins, a messenger's user ids)
      --node <NODE>                  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --disallow <NAME>              Take them off the list
      --bot <NAME>                   A bot whose messages may become work nobody asked for (never a request)
      --listed-unaddressed <CELL>    What a listed person's message not said to the agent becomes: inferred or ignore (when the watch is added: inferred on GitHub, ignore in a chat)
      --unlisted-addressed <CELL>    What a message said to the agent by someone not on the list becomes: explicit, inferred or ignore (when added: ignore)
      --unlisted-unaddressed <CELL>  What a message by someone not on the list, not said to the agent, becomes: inferred or ignore (when added: ignore)
      --remove                       Stop watching it: the work open there is revoked when it would go on, and its messages not yet sent are dropped
  -h, --help                         Print help
```

A new GitHub watch weighs a listed person's messages not said to the agent; a new chat watch ignores them, unless `--listed-unaddressed inferred`.

## `sion agent work run`

```text
Take work on as it comes: read the watches, run the work in its slots, and send what it says

Usage: sion agent work run [OPTIONS] <PROFILE>

Arguments:
  <PROFILE>  The agent profile

Options:
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --once                 Read the watches once, run what that starts until it waits, send what it said, and stop
      --poll <POLL>          How often to read the watches, in seconds [default: 30]
      --slots <SLOTS>        How many items may run at once; one more slot is kept for a person's message [default: 2]
      --assess-with <MODEL>  The model that weighs work nobody asked for (default: the profile's first)
      --listen [<ADDRESS>]   Take GitHub's webhooks here as well (signed with GITHUB_WEBHOOK_SECRET): what they bring is taken at once, and the repositories are still read every --poll seconds for what they miss (127.0.0.1:8794; put it behind TLS for GitHub)
  -h, --help                 Print help
```

One runner works for a profile at a time (`runner.pid` in its directory). Its token for GitHub is `GH_TOKEN` or `GITHUB_TOKEN`, else the GitHub CLI's; `SION_GITHUB_API` names another API, `SION_GITHUB_LOGIN` the agent's login. Chats use the bridge's credentials (`agent:work-runner`).

## `sion agent work list`

```text
The work taken on, and what each item waits for

Usage: sion agent work list [OPTIONS] <PROFILE>

Arguments:
  <PROFILE>  The agent profile

Options:
      --all          Ended items too
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent work show`

```text
One item: its request, its mandate, what reached it, what it reported, and its messages

Usage: sion agent work show [OPTIONS] <PROFILE> <ITEM>

Arguments:
  <PROFILE>  The agent profile
  <ITEM>     The item (w1, w2, …)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent work decisions`

```text
What became of each trigger, newest last: taken on, ignored, gone to open work, or not weighed; and in observe mode, what would have been taken on

Usage: sion agent work decisions [OPTIONS] <PROFILE>

Arguments:
  <PROFILE>  The agent profile

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --would        Only what observe mode would have taken on
      --last <LAST>  How many to show [default: 20]
  -h, --help         Print help
```

## `sion agent work answer`

```text
Answer an item's question, or tell it what to do, as its operator

Usage: sion agent work answer [OPTIONS] <PROFILE> <ITEM> <TEXT>

Arguments:
  <PROFILE>  The agent profile
  <ITEM>     The item
  <TEXT>     What to tell it

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent work approve`

```text
Approve the call an item's session waits on, or decline it

Usage: sion agent work approve [OPTIONS] <PROFILE> <ITEM>

Arguments:
  <PROFILE>  The agent profile
  <ITEM>     The item

Options:
      --decline <WHY>  Decline it instead, saying why
      --node <NODE>    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help           Print help
```

## `sion agent work stop`

```text
Stop an item: its mandate is withdrawn, on the ledger too

Usage: sion agent work stop [OPTIONS] <PROFILE> <ITEM>

Arguments:
  <PROFILE>  The agent profile
  <ITEM>     The item

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

An item that is running stops before its session's next step; its mandate's withdrawal is issued then.

## `sion agent work retry`

```text
Try again: the messages of an item that could not be sent, or an item that failed or is blocked

Usage: sion agent work retry [OPTIONS] <PROFILE> <ITEM>

Arguments:
  <PROFILE>  The agent profile
  <ITEM>     The item

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent project`

```text
Projects a profile's agent owns, running for weeks: a goal, a budget its payments for the project are held to, milestones, and check-ins that report where it stands

Usage: sion agent project [OPTIONS] <COMMAND>

Commands:
  start    Start a project for a profile's agent: its goal, the USD its payments for it may come to, how long it runs, and how often it reports where it stands
  list     A profile's projects, a line each
  show     Where a project stands, as its next report would say it (without marking it reported)
  work     Have the agent work on a project now: one session on its goal, its milestones and what was done lately, with tools to record what it does; its payments are held to what the project has left
  note     Note something done for a project, as its operator, with what it cost if it spent from the project's budget
  reached  Mark a milestone reached (added if the project did not list it)
  report   Make the report now and send it where the project's reports go
  pause    Set a project aside: it takes no work until it is resumed
  resume   Take a paused project up again
  done     Mark a project's goal reached: it takes no more work
  stop     Give a project up: it takes no more work
  help     Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

A project is kept in `$SION_HOME/agents/routes/<profile>-projects.json`; its budget is counted there, not escrowed on the ledger (`agent:project-budget`). `sion agent scheduler` makes its check-ins. See [Projects that run for weeks](../agent/work.md#projects-that-run-for-weeks).

## `sion agent project start`

```text
Start a project for a profile's agent: its goal, the USD its payments for it may come to, how long it runs, and how often it reports where it stands

Usage: sion agent project start [OPTIONS] --goal <GOAL> --budget <USD> --for <DURATION> <PROFILE> <NAME>

Arguments:
  <PROFILE>  The profile whose agent owns it
  <NAME>     Its name (lowercase letters, digits and hyphens)

Options:
      --goal <GOAL>          What it is for, in words
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --budget <USD>         The USD it may spend in all
      --for <DURATION>       How long it runs: 30d, 12h
      --check-in <INTERVAL>  How often it reports where it stands: 1d, 12h [default: 1d]
      --milestone <TEXT>     A milestone to work towards (repeat for more)
      --deliver <DELIVER>    Where its reports go, as a schedule's do: file:<path>, webhook:<url>, telegram:<chat id>, or <messenger>:<chat>
      --work-at-check-in     At each check-in, have the agent work on it once before the report is made (with `sion agent scheduler` running)
  -h, --help                 Print help
```

## `sion agent project list`

```text
A profile's projects, a line each

Usage: sion agent project list [OPTIONS] <PROFILE>

Arguments:
  <PROFILE>  The profile

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent project show`

```text
Where a project stands, as its next report would say it (without marking it reported)

Usage: sion agent project show [OPTIONS] <PROFILE> <NAME>

Arguments:
  <PROFILE>  The profile
  <NAME>     The project

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent project work`

```text
Have the agent work on a project now: one session on its goal, its milestones and what was done lately, with tools to record what it does; its payments are held to what the project has left

Usage: sion agent project work [OPTIONS] <PROFILE> <NAME>

Arguments:
  <PROFILE>  The profile
  <NAME>     The project

Options:
      --node <NODE>    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --steer <STEER>  What to work on now, in words
  -h, --help           Print help
```

## `sion agent project note`

```text
Note something done for a project, as its operator, with what it cost if it spent from the project's budget

Usage: sion agent project note [OPTIONS] <PROFILE> <NAME> <WHAT>

Arguments:
  <PROFILE>  The profile
  <NAME>     The project
  <WHAT>     What was done, in a line

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --spent <USD>  The USD it cost
  -h, --help         Print help
```

## `sion agent project reached`

```text
Mark a milestone reached (added if the project did not list it)

Usage: sion agent project reached [OPTIONS] <PROFILE> <NAME> <MILESTONE>

Arguments:
  <PROFILE>    The profile
  <NAME>       The project
  <MILESTONE>  The milestone, as listed

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent project report`

```text
Make the report now and send it where the project's reports go

Usage: sion agent project report [OPTIONS] <PROFILE> <NAME>

Arguments:
  <PROFILE>  The profile
  <NAME>     The project

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent project pause`

```text
Set a project aside: it takes no work until it is resumed

Usage: sion agent project pause [OPTIONS] <PROFILE> <NAME>

Arguments:
  <PROFILE>  The profile
  <NAME>     The project

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent project resume`

```text
Take a paused project up again

Usage: sion agent project resume [OPTIONS] <PROFILE> <NAME>

Arguments:
  <PROFILE>  The profile
  <NAME>     The project

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent project done`

```text
Mark a project's goal reached: it takes no more work

Usage: sion agent project done [OPTIONS] <PROFILE> <NAME>

Arguments:
  <PROFILE>  The profile
  <NAME>     The project

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent project stop`

```text
Give a project up: it takes no more work

Usage: sion agent project stop [OPTIONS] <PROFILE> <NAME>

Arguments:
  <PROFILE>  The profile
  <NAME>     The project

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

## `sion agent waiting`

```text
Everything that waits for a person: work items' questions, approvals and failures, messages that could not be sent, sessions waiting for approval, and chats waiting to be let in

Usage: sion agent waiting [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

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
      --a2a-push-private       Let other agents' A2A webhooks be on this machine's network: loopback and private addresses, and plain HTTP (for testing; by default a webhook is an HTTPS address that resolves to a public one)
  -h, --help                   Print help
```

Whoever holds the token can start and continue every profile's sessions, approvals included (`agent:channel-token`). Requests are handled one at a time. With `--a2a`, the profiles named are A2A agents for other agents, with a card each ([Agents that talk A2A](../agent/channels.md#agents-that-talk-a2a)). See [Channels](../agent/channels.md#http).

## `sion agent talk`

```text
Talk with a profile in the terminal: what you type goes to it, its reply comes as the model writes it; with --voice, heard and spoken

Usage: sion agent talk [OPTIONS] <PROFILE>

Arguments:
  <PROFILE>  The agent profile

Options:
      --conversation <CONVERSATION>  The conversation: each is a session of its own [default: default]
      --node <NODE>                  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --voice                        Speak instead of typing: press Enter and speak; heard with --hear, answered aloud with --speak if given (or, live, through --realtime)
      --hear <PROVIDER>              What hears you: openai:<model>, groq:<model>, deepgram:<model>, elevenlabs:<model>, command:<program>, or on this machine whisper:<model file> (whisper.cpp) or local:<model> (a server that speaks OpenAI's API, SION_LOCAL_STT_URL)
      --speak <PROVIDER>             What speaks the replies: openai:<model>:<voice>, elevenlabs:<voice id>[:<model>], deepgram:<voice model>, command:<program>, or on this machine piper:<model file> (Piper) or local:<model>:<voice> (a server that speaks OpenAI's API, SION_LOCAL_TTS_URL)
      --record <COMMAND>             The program that records you, given the file to write (sox's rec by default, stopping after a second and a half of silence)
      --play <COMMAND>               The program that plays a spoken reply, given its file (mpv, ffplay, afplay or paplay, the first there, by default)
      --live                         Listen all the time instead of at Enter: speech is told from other sound by a model of it, the reply is spoken a sentence at a time, its echo from the loudspeakers taken out of what the microphone hears, and speaking over it stops it
      --listen <COMMAND>             The program that gives the microphone's sound: raw 16-bit mono at 16 kHz on its output (sox's rec, arecord, parec or ffmpeg, the first there, by default)
      --wake <PHRASE>                Take only what begins with this phrase ("hey sion"; several split by |)
      --wake-hear <PROVIDER>         What hears what is said before the wake word: a transcriber on this machine (whisper:<model file>, local:<model> at an address of this machine's, or command:<program>); by default --hear's, if it is one
      --wake-cloud                   Let a transcriber elsewhere hear what is said before the wake word: everything said near the microphone
      --awake-secs <AWAKE_SECS>      Seconds it stays awake for a follow-up after it speaks [default: 15]
      --no-interrupt                 Speaking over the agent does not stop it: the microphone is not listened to while it works or speaks (for loudspeakers)
      --speech-level <LEVEL>         The quietest speech taken, as the root mean square of its samples (of 32767): lower it if you are not heard, raise it in a loud room [default: 400]
      --detector <HOW>               How speech is told from other sound: model (RNNoise's network, with its loudness) or loudness (alone) [default: model]
      --turns <HOW>                  How the end of what you say is told: semantic (after a short pause, unless what was said sounds unfinished, which waits a little for the rest) or silence (after a longer pause, always) [default: semantic]
      --no-echo-cancel               Do not take the reply's echo out of what the microphone hears: replies are played as files (use headphones, or --no-interrupt)
      --play-raw <COMMAND>           With the echo cancelled, the program that plays raw 16-bit mono samples at 16 kHz read on its standard input (aplay, paplay, sox's play or ffplay, the first there, by default)
      --realtime <MODEL>             Hear and speak through a realtime speech model instead (openai:<model>[:<voice>], such as openai:gpt-realtime): faster; it does nothing itself, and asks the agent for all but small talk
  -h, --help                         Print help
```

The conversation is a session as on any channel, its commands included (`/new`, `/status`, `/approve`…). Typed, or pressing Enter to speak, there is no `/stop` while the agent works: Ctrl-C ends the conversation, the session kept. With `--live`, it listens all the time: speaking over the agent stops it (as does typing `/stop`), and a command typed while it works runs alongside. See [In the terminal, and by voice](../agent/voice.md#in-the-terminal-and-by-voice) and [A live conversation](../agent/voice.md#a-live-conversation).

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
          Hear voice notes: openai:<model>, groq:<model>, deepgram:<model>, elevenlabs:<model>, command:<program> (given the audio's file, it prints the text), or on this machine whisper:<model file> or local:<model>

      --speak <PROVIDER>
          Answer voice notes aloud: openai:<model>:<voice>, elevenlabs:<voice id>[:<model>], deepgram:<voice model>, command:<program> (given the text, it writes Ogg Opus or MP3), or on this machine piper:<model file> or local:<model>:<voice>

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

In a group chat, every member speaks for you, approvals included (`agent:telegram-chats`). See [Messengers](../agent/messengers.md#telegram).

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
          Hear voice notes: openai:<model>, groq:<model>, deepgram:<model>, elevenlabs:<model>, command:<program> (given the audio's file, it prints the text), or on this machine whisper:<model file> or local:<model>

      --speak <PROVIDER>
          Answer voice notes aloud: openai:<model>:<voice>, elevenlabs:<voice id>[:<model>], deepgram:<voice model>, command:<program> (given the text, it writes Ogg Opus or MP3), or on this machine piper:<model file> or local:<model>:<voice>

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
  verify  Run the check a task fixed when it was posted on an output, as one of its verifiers, and attest the output (on your bond `bond:<you>:verifier`) only if the check passes. The check runs in a sandbox with no network, in a fresh directory holding its files, with the output read-only at $OUTPUT; an exit status of 0 passes
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
      --check <DIR>                A check results must pass, fixed now: the files of this directory and --check-command, committed in the task's spec with the description --spec gives, and kept on this machine for the task's verifiers, who run it with `sion task verify`. A task with a check needs an attestation (--must-attest or --min-attestations)
      --check-command <COMMAND>    The check's command, run with /bin/sh in a directory holding its files, the output to check at $OUTPUT [default: "sh check.sh"]
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

## `sion task verify`

```text
Run the check a task fixed when it was posted on an output, as one of its verifiers, and attest the output (on your bond `bond:<you>:verifier`) only if the check passes. The check runs in a sandbox with no network, in a fresh directory holding its files, with the output read-only at $OUTPUT; an exit status of 0 passes

Usage: sion task verify [OPTIONS] <VERIFIER> <TASK> <OUTPUT>

Arguments:
  <VERIFIER>  The verifier's wallet
  <TASK>      The task
  <OUTPUT>    The output: a file (its exact bytes), or the commitment of an output kept on this machine (as an agent's task_submit keeps it)

Options:
      --check <DIR>              The check's files, if the check is not kept on this machine: it must be the one the task fixed
      --node <NODE>              The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --check-command <COMMAND>  The check's command, with --check: the one the task was posted with [default: "sh check.sh"]
      --spec <FILE>              The task's description, with --check, if it is not kept on this machine
      --bond <BOND>              The bond label the attestation rests on [default: verifier]
      --sandbox <SANDBOX>        What runs the check: `bwrap` (bubblewrap, the default) or `docker:<image>@sha256:<digest>`, an image already on this machine [default: bwrap]
      --sandbox-read <DIR>       A directory the check may read, at its own path (a toolchain, say). Repeat for more
      --timeout <SECONDS>        Seconds the check may run before it is stopped (and fails) [default: 600]
  -h, --help                     Print help
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

## `sion proving`

```text
A market for proofs of Pulses: ask for a Pulse's World Proof with a payment in escrow, deliver one, and attest one as a verifier after checking it under both proof stacks

Usage: sion proving [OPTIONS] <COMMAND>

Commands:
  request  Ask for the World Proof of a Pulse: escrow a payment for whoever delivers it, and name the verifiers who must each check and attest it before it is paid
  fetch    Fetch the World Proof a request wants from your node and check it, as a prover: the file is what you deliver
  attest   Check a delivered proof against a request, and attest it, as one of the request's verifiers: that it is of the certified header of that Pulse and verifies under both proof stacks. The attestation rests on your `verifier` bond
  deliver  Deliver a proof for a request with its verifiers' attestations, as its prover, and be paid
  show     Where a request stands; with a proof, also whether it is what the request wants
  help     Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

See [A market for proofs](../guides/proving.md).

## `sion proving request`

```text
Ask for the World Proof of a Pulse: escrow a payment for whoever delivers it, and name the verifiers who must each check and attest it before it is paid

Usage: sion proving request [OPTIONS] --verifier <ACCOUNT> --wallet <WALLET> <PULSE> <AMOUNT>

Arguments:
  <PULSE>   The Pulse (one to come, or a recent one: a node keeps the proofs of its last Pulses only)
  <AMOUNT>  What to pay for it, in USD

Options:
      --node <NODE>                The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --verifier <ACCOUNT>         A verifier (a wallet or an account; repeat for more). Each must attest the proof
      --prover <ACCOUNT>           The one prover who may deliver it (a wallet or an account). Without it anyone may, and whoever settles first with the attested proof is paid
      --adjudicator <ADJUDICATOR>  The adjudicator atom the result and its attestations name (hex)
      --deadline <DEADLINE>        How many Pulses the request stays open [default: 500]
      --wallet <WALLET>            Your wallet
  -h, --help                       Print help
```

## `sion proving fetch`

```text
Fetch the World Proof a request wants from your node and check it, as a prover: the file is what you deliver

Usage: sion proving fetch [OPTIONS] --out <FILE> <TASK>

Arguments:
  <TASK>  The request (its task)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --out <FILE>   The file to write the proof to
  -h, --help         Print help
```

## `sion proving attest`

```text
Check a delivered proof against a request, and attest it, as one of the request's verifiers: that it is of the certified header of that Pulse and verifies under both proof stacks. The attestation rests on your `verifier` bond

Usage: sion proving attest [OPTIONS] --wallet <WALLET> <TASK> <PROOF>

Arguments:
  <TASK>   The request (its task)
  <PROOF>  The proof

Options:
      --chain <CHAIN>    The chain identity you trust. Without it the request's chain is taken from the node, which vouches for itself [env: SION_CHAIN=]
      --node <NODE>      The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --wallet <WALLET>  Your wallet
  -h, --help             Print help
```

## `sion proving deliver`

```text
Deliver a proof for a request with its verifiers' attestations, as its prover, and be paid

Usage: sion proving deliver [OPTIONS] --wallet <WALLET> <TASK> <PROOF>

Arguments:
  <TASK>   The request (its task)
  <PROOF>  The proof

Options:
      --attestations <ATTESTATIONS>  The verifiers' attestations (comma-separated claims)
      --node <NODE>                  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --wallet <WALLET>              Your wallet
  -h, --help                         Print help
```

## `sion proving show`

```text
Where a request stands; with a proof, also whether it is what the request wants

Usage: sion proving show [OPTIONS] <TASK>

Arguments:
  <TASK>  The request (its task)

Options:
      --node <NODE>    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --proof <FILE>   A proof to check against it, as a verifier checks one
      --chain <CHAIN>  The chain identity you trust, for checking a proof [env: SION_CHAIN=]
  -h, --help           Print help
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
  dream        Dream over an agent's memory: the sessions of the episodes recorded since the last dream, those on this machine, each read whole by a reader of its own, which proposes facts, procedures and preferences with the words of the session that show them. A note is kept only if its session says those words (a session that read content from outside, facts only), then as consolidation keeps it, and the log is committed
  archive      Archive a log for safekeeping: every atom the ledger holds, encrypted for the writer, named on the ledger as the log's manifest and kept by a storage provider under a deal
  restore      Restore a log's local store from its latest archive, checked against the head on the ledger before anything is replaced
  vouch        Vouch for an atom of a log, as you: a claim on your bond `memory-voucher`, which the adjudicator can rule against, named in your index so that readers find it. Readers who keep the log in mind see who vouched for each fact
  revoke       Revoke an atom of a log, as its writer: readers leave it out, and whatever was derived from it or assumes it, however deep. Someone who vouched for it withdraws their vouching the same way. Nothing is deleted from the log
  standing     Where each atom of a log stands for a reader here, once what is known of its vouchings and revocations is brought up to date from the ledger: revoked, resting on a revoked atom, vouched for and by whom, or its writer's word alone
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
      --model <MODEL>  The model that proposes: anthropic:<model>, openai:<model>, gemini:<model>, local:<model> or script:<file>
      --node <NODE>    The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help           Print help
```

## `sion memory dream`

```text
Dream over an agent's memory: the sessions of the episodes recorded since the last dream, those on this machine, each read whole by a reader of its own, which proposes facts, procedures and preferences with the words of the session that show them. A note is kept only if its session says those words (a session that read content from outside, facts only), then as consolidation keeps it, and the log is committed

Usage: sion memory dream [OPTIONS] --model <MODEL> <WALLET> <LOG>

Arguments:
  <WALLET>  The writer's wallet (the agent's)
  <LOG>     The log (its id, or a unique prefix of it)

Options:
      --model <MODEL>       The model that reads: anthropic:<model>, openai:<model>, gemini:<model>, local:<model> or script:<file>
      --node <NODE>         The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --eval <FILE>         Held-out tasks (a set of tasks, as `sion agent graph eval` takes): each case is run with the core memory as it is and as it would be with the dream's notes in mind, and the notes are recorded only if no case passes fewer times with them
      --eval-model <MODEL>  The model those runs work with (default: the reader)
      --eval-runs <N>       How many times each case runs each way [default: 3]
      --into <LOG>          A curated log to record the notes in instead of the agent's own (its id, or a unique prefix of it): dreams are its only writer
      --curator <WALLET>    The wallet that writes the curated log
  -h, --help                Print help
```

See [Dreaming](../agent/memory.md#dreaming).

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

## `sion memory vouch`

```text
Vouch for an atom of a log, as you: a claim on your bond `memory-voucher`, which the adjudicator can rule against, named in your index so that readers find it. Readers who keep the log in mind see who vouched for each fact

Usage: sion memory vouch [OPTIONS] --adjudicator <ADJUDICATOR> <WALLET> <LOG> <SEQ>

Arguments:
  <WALLET>  Your wallet
  <LOG>     The log (its id, or a unique prefix of it)
  <SEQ>     The atom's place in the log

Options:
      --adjudicator <ADJUDICATOR>  The adjudicator who can rule against your vouching (its atom's id, 64 hex digits)
      --node <NODE>                The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --bond <USD>                 Post your bond `memory-voucher` with this many USD first
      --note <NOTE>                What you say of it, in a line [default: ""]
      --valid-for <VALID_FOR>      How many Pulses the vouching stands for [default: 1000000]
  -h, --help                       Print help
```

See [Facts from other owners](../agent/memory.md#facts-from-other-owners-who-vouched-and-pulling-one-back).

## `sion memory revoke`

```text
Revoke an atom of a log, as its writer: readers leave it out, and whatever was derived from it or assumes it, however deep. Someone who vouched for it withdraws their vouching the same way. Nothing is deleted from the log

Usage: sion memory revoke [OPTIONS] <WALLET> <LOG> <SEQ>

Arguments:
  <WALLET>  Your wallet
  <LOG>     The log (its id, or a unique prefix of it)
  <SEQ>     The atom's place in the log

Options:
      --node <NODE>            The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --note <NOTE>            Why, in a line [default: ""]
      --valid-for <VALID_FOR>  How many Pulses the revocation stands for [default: 1000000]
  -h, --help                   Print help
```

## `sion memory standing`

```text
Where each atom of a log stands for a reader here, once what is known of its vouchings and revocations is brought up to date from the ledger: revoked, resting on a revoked atom, vouched for and by whom, or its writer's word alone

Usage: sion memory standing [OPTIONS] <LOG>

Arguments:
  <LOG>  The log (its id, or a unique prefix of it)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
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
  import   Import an Agent Skill for a profile's agent, from a folder or a git repository: it is read and reported on (its files, its scripts, the hosts and patterns in them, the permissions it asks for), and installed only once --accept names its bundle root after you read it, or --audit names an active audit of that root on a bond
  search   Search a registry for skills to import: ClawHub (OpenClaw's) or skills.sh. What a registry says of a skill is shown, not relied on
  teach    Teach a task by doing it once in a browser window: what you do is recorded (never a password, a secret field or a login's user name; at most 10 minutes) and written down as a draft skill for the agent's browser, to read and then install with `sion skill import`
  starter  Install sion's starter skills for a profile's agent: web-research, code-change, pr-review, ci-fix, inbox-triage, scheduling and document-brief (all of them, or those named)
  help     Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

See [Skills from others](../agent/skills.md).

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

## `sion skill import`

```text
Import an Agent Skill for a profile's agent, from a folder or a git repository: it is read and reported on (its files, its scripts, the hosts and patterns in them, the permissions it asks for), and installed only once --accept names its bundle root after you read it, or --audit names an active audit of that root on a bond

Usage: sion skill import [OPTIONS] --profile <PROFILE> <SOURCE>

Arguments:
  <SOURCE>  A folder, github:owner/repo, a git address (https://…), clawhub:owner/slug[@version], skills.sh:owner/repo/skill, or a site's skill (well-known:example.org#name)

Options:
      --node <NODE>           The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --profile <PROFILE>     The profile whose agent gets it
      --path <PATH>           The skill's folder within the repository
      --ref <REF>             The branch or tag to read (default: the repository's own)
      --accept <ROOT>         Install it: the bundle root the report gave, after reading it
      --audit <CLAIM>         Install it on an audit: the claim of an active audit of its bundle on a bond (needs the node)
      --accept-permissions    Grant the permissions it asks for (a network, secrets)
      --accept-risks          Install it despite the findings the report says hold it back, once you have read them
      --grant-network <HOST>  A host its scripts may reach, declared for it in its metadata (repeat for more; needs --accept-permissions)
      --grant-secret <NAME>   A secret its scripts receive as an environment variable, declared for it (repeat for more; keep it with sion agent secret add; needs --accept-permissions)
  -h, --help                  Print help
```

Nothing is installed by the first run: it reports, and prints the bundle root to accept. A skill installed on an audit is taken out of use if the audit is shown false. See [Importing a skill](../agent/skills.md#importing-a-skill).

## `sion skill search`

```text
Search a registry for skills to import: ClawHub (OpenClaw's) or skills.sh. What a registry says of a skill is shown, not relied on

Usage: sion skill search [OPTIONS] <QUERY>

Arguments:
  <QUERY>  What to look for

Options:
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --registry <REGISTRY>  The registry: clawhub or skills.sh [default: clawhub]
      --limit <LIMIT>        How many to list [default: 10]
  -h, --help                 Print help
```

See [Importing a skill](../agent/skills.md#importing-a-skill).

## `sion skill teach`

```text
Teach a task by doing it once in a browser window: what you do is recorded (never a password, a secret field or a login's user name; at most 10 minutes) and written down as a draft skill for the agent's browser, to read and then install with `sion skill import`

Usage: sion skill teach [OPTIONS] <NAME>

Arguments:
  <NAME>  The skill's name: lowercase letters, digits and hyphens

Options:
      --node <NODE>          The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --start <START>        The page the browser opens first
      --describe <DESCRIBE>  What the task is, in a sentence: how an agent knows when to use the skill
      --minutes <MINUTES>    How long the recording may last, in minutes (at most 10) [default: 10]
      --out <OUT>            Where the skill's folder goes (default: $SION_HOME/agents/taught/<name>)
      --force                Replace a draft of that name
      --write-with <MODEL>   A model that rewrites the draft from the recording (anthropic:<model>, openai:<model>, …)
      --profile <PROFILE>    The profile the skill is for: named in how to install it
  -h, --help                 Print help
```

See [Teaching a task](../agent/skills.md#teaching-a-task).

## `sion skill starter`

```text
Install sion's starter skills for a profile's agent: web-research, code-change, pr-review, ci-fix, inbox-triage, scheduling and document-brief (all of them, or those named)

Usage: sion skill starter [OPTIONS] --profile <PROFILE> [NAMES]...

Arguments:
  [NAMES]...  The skills (default: all)

Options:
      --node <NODE>        The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --profile <PROFILE>  The profile whose agent gets them
  -h, --help               Print help
```

See [Starter skills](../agent/skills.md#starter-skills).

## `sion skill eval`

```text
Run an evaluation set on a skill: each case with stand-in tools, then its checks. Needs no node

Usage: sion skill eval [OPTIONS] --evals <EVALS> --model <MODELS> <DIR>

Arguments:
  <DIR>  The skill directory (with a SKILL.md)

Options:
      --evals <EVALS>                The evaluation set (a JSON file)
      --node <NODE>                  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
      --model <MODELS>               The agent's model: anthropic:<model>, openai:<model>, gemini:<model>, local:<model> or script:<file>. Repeat for fallbacks, in order
      --trajectories <TRAJECTORIES>  Write each run as a trajectory (JSON Lines in chat format), labelled with its result
  -h, --help                         Print help
```

Needs no node. See [Skills that get better](../agent/skill-improvement.md#measuring-a-skill).

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
      --held-out <FILE>            An evaluation set of the same skill whose cases the evolution never sees: the best version is kept as a draft only if it passes every one of them that the version given passes
  -h, --help                       Print help
```

Needs no node. Gates: the name and description unchanged, the size limit, a version not tried before, and no case lost that the parent passed. See [Skills that get better](../agent/skill-improvement.md#evolving-a-better-version).

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

The spec is kept in `$SION_HOME/skills/specs`, under the commitment the task records. See [Skills that get better](../agent/skill-improvement.md#a-bounty-fixed-before-any-better-version-exists).

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
Serve the Model Context Protocol on stdio (for AI agents): the ledger's tools, or a person's mail, calendars or documents

Usage: sion mcp [OPTIONS] [COMMAND]

Commands:
  mail       A person's mailbox over IMAP and SMTP (MAIL_ADDRESS, MAIL_IMAP_URL, MAIL_SMTP_URL, MAIL_PASSWORD or MAIL_TOKEN_COMMAND): search, read, draft, send
  calendar   A person's calendars over CalDAV (CALDAV_URL, CALDAV_USERNAME and CALDAV_PASSWORD, or CALDAV_TOKEN_COMMAND): list, read, add, remove events
  documents  A folder of documents: list, read (text, web pages, Word, OpenDocument, PowerPoint, PDF), search, write text and Markdown
  help       Print this message or the help of the given subcommand(s)

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

See [MCP tools](mcp.md).

## `sion mcp mail`

```text
A person's mailbox over IMAP and SMTP (MAIL_ADDRESS, MAIL_IMAP_URL, MAIL_SMTP_URL, MAIL_PASSWORD or MAIL_TOKEN_COMMAND): search, read, draft, send

Usage: sion mcp mail [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

Over IMAP and SMTP, from `MAIL_ADDRESS`, `MAIL_IMAP_URL`, `MAIL_SMTP_URL` and `MAIL_PASSWORD` or `MAIL_TOKEN_COMMAND` (`MAIL_USERNAME` if it is not the address), from the environment or kept with `sion agent credential add`. Drafts are saved to the mailbox's drafts folder; what is sent carries no `Auto-Submitted` header, as the person's own mail. See [sion's own servers](../agent/tools.md#sions-own-servers-mail-calendars-documents).

## `sion mcp calendar`

```text
A person's calendars over CalDAV (CALDAV_URL, CALDAV_USERNAME and CALDAV_PASSWORD, or CALDAV_TOKEN_COMMAND): list, read, add, remove events

Usage: sion mcp calendar [OPTIONS]

Options:
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

Over CalDAV, from `CALDAV_URL`, `CALDAV_USERNAME`, and `CALDAV_PASSWORD` or `CALDAV_TOKEN_COMMAND`, from the environment or kept with `sion agent credential add`. An event is added only where none has its identifier (`If-None-Match: *`), and removed with `If-Match` when its tag is given, so never one changed since it was read. See [sion's own servers](../agent/tools.md#sions-own-servers-mail-calendars-documents).

## `sion mcp documents`

```text
A folder of documents: list, read (text, web pages, Word, OpenDocument, PowerPoint, PDF), search, write text and Markdown

Usage: sion mcp documents [OPTIONS] --dir <DIR>

Options:
      --dir <DIR>    The folder [env: DOCUMENTS_DIR=]
      --node <NODE>  The node's API [env: SION_NODE=] [default: http://127.0.0.1:8730]
  -h, --help         Print help
```

Reads Markdown, text, HTML, Word (.docx), OpenDocument (.odt), PowerPoint (.pptx) and PDF (with `pdftotext`); writes only Markdown and text, inside the folder. See [sion's own servers](../agent/tools.md#sions-own-servers-mail-calendars-documents).

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
