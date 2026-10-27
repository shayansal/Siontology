# FAQ

## What is Siontology?

An open-source reference implementation of a neo-blockchain: a network
whose product is verified knowledge. Every claim it records carries a proof
or priced evidence. See [Neo-blockchains](../concepts/neo-blockchains.md).

## Is it a blockchain?

It keeps a replicated ledger with validators and consensus, but it is
organised differently. Knowledge is appended without coordination; only
the consumption of resources is ordered, and only among contenders. Owned
transfers finalize with one committee certificate and no consensus round.
Every Pulse is proven, so a client can check validity without replaying
history.

## Can I use it in production?

No. It is a devnet-scale reference implementation: every validator runs in
one process, the proofs are replay proofs that simulate succinct ones, and
devnet keys can be derived by anyone. It secures nothing of value. See
[What exists today](../introduction.md#what-exists-today).

## Is there a token?

Siontology has no token and nothing is on offer. Fees are paid in
stablecoins.

## Who are the validators, and how do they join?

On a devnet, four genesis validators, plus any standby validators you
start. A validator joins by staking, takes its seat at the next epoch
boundary, earns a share of fees, and loses its stake if it signs two
conflicting messages. See [Validators, staking and epochs](../concepts/committees.md).

## Is it post-quantum?

Every account and validator signs with two unrelated post-quantum families
at once, ML-DSA-44 and SLH-DSA-SHA2-128f, so a forger must break both, and
payloads are encrypted with ML-KEM-768. The implementations are unaudited
pure-Rust crates (`impl:unaudited-pq-crates`). See
[Post-quantum identity](../concepts/post-quantum.md).

## Are the proofs real?

The World Proof's bindings are real: it chains every chunk's statement,
binds receipts, published data and lane certificates, and two independent
implementations check every chunk. The proofs themselves are replay
proofs: verifying one costs as much as executing it. A succinct proof
stack would plug into the same interface; none is built yet. See
[The Fold and World Proofs](../concepts/the-fold.md).

## Why two proof stacks?

So that a bug in one cannot forge the world: a World Proof verifies only if
both accept every chunk. The second stack is written from the
specification alone, and their disagreements have already found real
defects. See [Keeping proof stack B independent](../architecture/independence.md).

## How fast is it?

In one benchmark run on a shared 2-core machine, a devnet of four
validators closed a Pulse every 3.9 seconds and moved about 15 transfers a
second, mostly bound by post-quantum signatures. The architecture's targets (100,000 operations a
second on a testnet) assume many machines and signature aggregation in
proofs, and have not been measured. See [Benchmarks](../guides/benchmarks.md).

## What can a phone check?

With the chain identity pinned: that a header was certified by the
committee of its epoch, that a receipt, an object or a claim is in a
certified Pulse, that the Pulse's data is available (by random sampling),
and, at the cost of replay, the Pulse's World Proof. See
[Verify as a light client](../getting-started/light-client.md).

## How does an AI agent use it?

Through the MCP server (`sion mcp`), the CLI or the HTTP API. An agent can
pay inside an Agent Charter whose limits the ledger enforces, price a claim
before relying on it, and buy HTTP resources through x402. See
[Connect an AI agent over MCP](../getting-started/mcp.md).

## What if my transactions are ignored?

Post them as forced intents (`sion send --forced`). One honest proposer is
enough to order them, and a shard that does not execute them in time has
its committee replaced and the miss recorded in every certified header.
See [Censorship resistance](../concepts/censorship-resistance.md).

## Can I run validators on several machines?

Not yet. Validators exchange real signed messages, but over in-memory
channels inside one process (`impl:in-process-network`).

## Where is the specification?

[`docs/spec/state-transition.md`](https://github.com/ColdAI-org/siontology/blob/main/docs/spec/state-transition.md),
version 1.5. See [The state transition specification](../specification.md).

## How do I report a security problem?

Privately, by email; see [Responsible disclosure](../security/disclosure.md).

## Under which licence?

Apache-2.0 or MIT, at your option.
