# Roadmap

> **These are plans, not commitments.** The phases and gates below come
> from the architecture paper, *Neo-Blockchains: The Fold Architecture*,
> and describe what the project intends to test before building further.
> Each gate can change the plan. What is built today is in
> [What exists today](../introduction.md#what-exists-today).

## Build the use before the network

The paper's plan is to sell the trust layer before building the network:
start with verifiable receipts and Agent Charters on existing chains, let
usage grow, and build the neo-blockchain itself only once volume justifies
it. The order in which it intends to offer the pieces:

| Order | Product | First users | Why in this order |
|---|---|---|---|
| 1 | Receipts: verifiable AI-output and content receipts with trust formulas | AI providers; enterprises deploying AI | Regulatory pull and fraud losses; needs no new chain |
| 2 | Charters: identity, limits and bonded accountability for agents | Companies letting agents spend money | A bounded worst case; plugs into agent payment protocols |
| 3 | Compliance-by-proof settlement | Fintechs and payment providers | Private stablecoin settlement with provable checks |
| 4 | Verifiable Clearing markets | GPU-compute and energy marketplaces | Certified-optimal allocation |
| 5 | The full neo-blockchain | Everyone above | Moves the proven pieces onto a network built for them |

## Phases and gates

The plan has five phases separated by four gates. Each gate has criteria
that must be met, or the plan changes:

| Gate | Proposed criteria | If missed |
|---|---|---|
| Gate 1: end of Phase 0 (month 6) | 3 signed design partners; the Assurance Algebra specified and reviewed by two outside cryptographers | License the algebra and SDK to existing chains instead of building a network |
| Gate 2: end of Phase 1 (month 18) | 100 million receipts a month; 10 paying customers; under 0.0001 USD per receipt | Stay an application-layer protocol on existing chains |
| Gate 3: end of Phase 2 (month 36) | A testnet sustains 100,000 operations per second with a World Proof at least every 30 seconds; two independent audits with no open critical findings | Delay the main network; launch with periodic proofs and lower caps |
| Gate 4: end of Phase 3 (month 48) | The first cleared market reaches steady volume with a published optimality gap under 0.1% | Keep Clearing to exact-certificate markets only |

The network build starts only after Gate 2.

## What this repository has not built yet

The reference implementation is a devnet. From the
[implementation status](https://github.com/ColdAI-org/siontology/blob/main/docs/implementation-status.md),
the main pieces of the architecture that are not built, or only
simulated:

- **Succinct proofs.** World Proofs are replay proofs. A succinct,
  hash-based proof stack would plug into the existing `ProofStack`
  interface.
- **A proving market**, and payments to provers.
- **Networking between machines**, beyond what is simplified today:
  `sion node` processes talk QUIC, tested on one machine, with peers from
  files; a validator that stops cannot rejoin.
- **An uncertified DAG** (Mysticeti class) for Contention Consensus; the
  current protocol is a certified DAG.
- **Sampled consensus committees** and a **value-in-flight cap** at
  slashable stake.
- **Freezing a censoring shard** until it meets its obligation; today its
  committee is replaced.
- **Personhood**, **confidential amounts** and **compliance by
  zero-knowledge proof**.
- **Adapters** for AP2, ERC-8004 and C2PA (x402 is built).
- **Archival** of published data, and incentives to keep it.
- **A release** of `sion`, signed with the release key, and the agent run
  against the real model providers and messaging services: it has been
  tested with a scripted model and stand-ins for their APIs.
- **The mobile app.** The desktop app is built; its Android and iOS builds
  are configured, not attempted.

## Open research problems

The paper lists problems that are open for everyone, not only for this
project, among them aggregating post-quantum signatures (lattice and
hash-based) efficiently inside proofs. Progress on them moves the targets
above; none of them is solved here.
