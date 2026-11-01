# Connect an AI agent over MCP

`sion mcp` serves the [Model Context Protocol](https://modelcontextprotocol.io)
on standard input and output, so an AI agent can use the ledger as a set of
tools: check balances, pay, issue claims, price a claim before relying on
it, pay inside an Agent Charter, submit a transaction it signed itself,
verify a Pulse or a receipt, read events, buy HTTP resources that charge
through x402, post and settle work, keep a memory log, and keep data with
storage providers.

This page is for an agent built elsewhere. An agent of your own, which
the runtime that comes with `sion` runs on your computer, has the ledger
among its tools already: see [Your agent](../agent/index.md).

```bash
sion mcp            # talks to the node at $SION_NODE (default http://127.0.0.1:8730)
```

The server speaks JSON-RPC 2.0, one message per line, in both eras of the
protocol: version `2026-07-28`, whose requests carry their version
(`server/discover` says what it speaks), and the `initialize` handshake of
`2025-11-25` and earlier versions. It offers tools only, and needs a
running node (see [Run a devnet](devnet.md)).

## Add it to an MCP client

Most clients start a stdio server from a command and its arguments. In
Claude Code, for example
([MCP documentation](https://code.claude.com/docs/en/mcp.md)):

```bash
claude mcp add --env SION_NODE=http://127.0.0.1:8730 --transport stdio siontology -- sion mcp
```

Clients that read a JSON `mcpServers` configuration take an entry like
this:

```json
{
  "mcpServers": {
    "siontology": {
      "command": "sion",
      "args": ["mcp"],
      "env": { "SION_NODE": "http://127.0.0.1:8730" }
    }
  }
}
```

## Which keys the agent holds

Tools that sign (`sion_pay`, `sion_issue_claim`, `sion_agent_pay`,
`sion_fetch_paid`) take a **wallet name**. The server resolves it the way
the CLI does: the seed file `$SION_HOME/wallets/<name>.seed` if it exists,
otherwise the deterministic devnet wallet of that name, whose keys anyone
can derive. An agent connected to `sion mcp` can therefore sign as any
wallet whose seed is in that directory, and as any deterministic devnet
wallet.

On a devnet that is the point: it lets an agent play every part. Outside
one, run the server with a `SION_HOME` that holds only the agent's own
wallet, and give the agent an [Agent Charter](../concepts/agent-charters.md)
rather than a funded account: the charter's limits hold whatever the agent
is told.

## A session

A real exchange, one JSON message per line. The client initializes:

```json
{"jsonrpc":"2.0","method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"docs","version":"0"}},"id":1}
```

```json
{"id":1,"jsonrpc":"2.0","result":{"capabilities":{"tools":{"listChanged":false}},"instructions":"Tools for the Siontology neo-blockchain devnet. Assess a claim with sion_assess_claim before relying on it.","protocolVersion":"2025-06-18","serverInfo":{"name":"sion","version":"0.1.0"}}}
```

`tools/list` returns 29 tools ([MCP tools](../reference/mcp.md) lists
them all); among them:

```text
sion_status  sion_balance  sion_pay  sion_issue_claim  sion_assess_claim  sion_agent_pay
sion_verify_pulse  sion_events  sion_submit_intent  sion_fetch_paid  sion_fetch_receipt
```

A payment:

```json
{"jsonrpc":"2.0","method":"tools/call","params":{"name":"sion_pay","arguments":{"from":"dana","to":"bob","amount_usd":1.5}},"id":5}
```

```json
{"id":5,"jsonrpc":"2.0","result":{"content":[{"text":"{\n  \"digest\": \"21e98fa52e2e93548f46961448250d1639b9d61a162335b0028e451264338f59\",\n  \"fee\": \"0.0011\",\n  \"pulse\": 205,\n  \"receipt\": \"Done: paid 1.5 USD to bob; fee 0.0011 USD.\",\n  \"status\": \"Success\"\n}","type":"text"}],"isError":false,"structuredContent":{"digest":"21e98fa52e2e93548f46961448250d1639b9d61a162335b0028e451264338f59","fee":"0.0011","pulse":205,"receipt":"Done: paid 1.5 USD to bob; fee 0.0011 USD.","status":"Success"}}}
```

Every tool result carries the same value twice: as pretty-printed JSON text
in `content`, and as `structuredContent`. Failures come back as
`isError: true` with the error message as text. Every transaction and
receipt also comes back as a sentence (`receipt`, `intent`), so the agent
can tell its user what happened.

## Assess before relying

The server's instructions tell the agent to call `sion_assess_claim` before
relying on a claim. It applies the [decision rule](../concepts/assurance/decision-rule.md)
under the standard policy, with the agent's κ and any atoms it distrusts:

```json
{"name":"sion_assess_claim","arguments":{"claim":"e825c122db0c61842f067b444583c2b3593a75e1c5f551f4c0e3162079239412","value_usd":1000}}
```

The claim rests only on its issuer's dual signature, so under the standard
policy it is worth ∞ and the answer is `"accept": true` with the reason
`cost of corruption ∞ covers κ·V = 2,000`. The same claim assessed with
`"kappa": 3` and `"distrust_atoms"` naming both signature families
(`module-lattice` and `sha2-256`) returns `"accept": false`:
`cost of corruption 0 (after detection discount 1000/1000) is below κ·V = 3,000`.

The [MCP tools reference](../reference/mcp.md) lists every tool with its
schema and a real call.
