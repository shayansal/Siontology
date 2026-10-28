# Siontology

Siontology is an open-source **neo-blockchain**: a network whose product is
verified knowledge, not ordered transactions. Every claim it records, about
a payment, a computation, an AI output or a physical event, carries a proof
or priced evidence, and anyone can check it from a phone.

This book documents the reference implementation in Rust: how to run it,
what each mechanism does, how the code is organised, and what is not built
yet. The design it implements is described in the architecture paper
*Neo-Blockchains: The Fold Architecture* (ColdAI, October 2026).

- Website: [siontology.org](https://siontology.org)
- Source: [github.com/ColdAI-org/siontology](https://github.com/ColdAI-org/siontology)
- Licence: Apache-2.0 OR MIT

> **Status.** Siontology is a devnet-scale reference implementation. Its
> devnet runs every validator in one process (separate `sion node`
> processes also talk over QUIC, tested on one machine), its validity
> proofs are *replay proofs* that simulate succinct ones, and its devnet
> keys can be derived by anyone. It secures nothing of value and has no
> production deployment. [What exists today](#what-exists-today) below
> says exactly what is built, simplified, simulated or missing, and every
> chapter follows it.

## Why

AI made generating text, images, code and decisions nearly free. Checking
them did not get cheaper. Every AI output, agent action and data feed raises
the same question: is this real, and who pays if it is not?

Trust is already one of the economy's largest costs. Financial-crime
compliance alone costs about 206 billion USD a year
([LexisNexis, 2023](https://risk.lexisnexis.com/about-us/press-room/press-release/20230926-global-financial-crime-compliance-costs)),
and sending 200 USD abroad costs 6.36% on average
([World Bank, Q3 2025](https://remittanceprices.worldbank.org/sites/default/files/2026-04/RPW_main_report_and_annex_Q325.pdf)).
AI agents are about to multiply the exchanges between strangers that pay
these costs.

Blockchains did not solve this. The architecture paper's diagnosis is that
they sold trust in a narrow unit, with heavy friction:

- they make one thing trustworthy, their own ledger state;
- they cannot check the facts most transactions depend on (that goods
  arrived, that a person is who they claim);
- every node re-executes every transaction, so popularity raises fees;
- they are public by default, so businesses keep their data off them.

A neo-blockchain extends verifiable trust to anything that can carry
evidence, and it puts a price on evidence that is not a proof.

## How it works, in four pieces

Blockchains chain blocks; DAGs link transactions; neo-blockchains **fold
claims**. Four pieces carry the design:

| Piece | One sentence | Chapter |
|---|---|---|
| Two-sorted state | Knowledge never needs coordination; only the consumption of resources is ordered, and only among contenders. | [Two-sorted state](concepts/two-sorted-state.md) |
| The Assurance Algebra | Every claim carries a formula of its assumptions; any agent computes what an attacker must forfeit to falsify it, relies on it only when that exceeds what is at stake, and reserves that backing. | [The Assurance Algebra](concepts/assurance/index.md) |
| Verifiable Clearing | A market clearing is valid only with a certificate that no better allocation exists. | [Verifiable Clearing](concepts/verifiable-clearing.md) |
| The Fold | Every proof folds into one World Proof per Pulse, so a phone verifies the whole history. | [The Fold and World Proofs](concepts/the-fold.md) |

Around them: owned resources finalize with one committee certificate and no
consensus ([Linear Lanes](concepts/linear-lanes.md)); shared totals split
into single-writer allotments ([Escrow Lanes](concepts/escrow-lanes.md));
only contended objects go through consensus
([Contention Consensus](concepts/contention-consensus.md)); AI agents act
inside limits the protocol enforces
([Agent Charters](concepts/agent-charters.md)); every authorization needs
signatures from two unrelated post-quantum families
([Post-quantum identity](concepts/post-quantum.md)); censored users can
force their transactions in
([Censorship resistance](concepts/censorship-resistance.md)); and who
certifies is itself part of the state: validators stake, and committees
change only at epoch boundaries, by rules the World Proof covers
([Validators, staking and epochs](concepts/committees.md)).

## What exists today

The repository's
[implementation status](https://github.com/ColdAI-org/siontology/blob/main/docs/implementation-status.md)
is the authority on what is built. This table copies the main rows of its
summary.

- **Built**: implemented as the architecture describes, at devnet scale,
  with tests.
- **Simplified**: implemented, with a named simplification that changes a
  guarantee or a cost.
- **Simulated**: something stands in for a required component, and is
  labelled as a stand-in in the types and in the output.
- **Not built**: absent.

| Architecture section | Status |
|---|---|
| Foundations: two-sorted state, derived coordination class | Built |
| Identity: post-quantum accounts | Built |
| Identity: personhood | Not built |
| Identity: Agent Charters | Built |
| Identity: agent identities and subagents | Built (devnet scale) |
| Agents: a runtime that lets a model work as an agent identity, within its charter (beyond the paper) | Partly built (runtime with providers and a scripted model, memory, the agent's own skills and skills from others installed through bonds and audits; self-improvement, schedules and channels not built) |
| Data availability: 2D Reed–Solomon, sampling, reconstruction, fraud proofs | Built |
| Data availability: retention and archival | Simplified |
| Ordering: Linear Lanes (owned objects) | Built |
| Ordering: Escrow Lanes (bounded shared totals) | Built |
| Ordering: Contention Consensus | Simplified (certified DAG, not uncertified) |
| Ordering: censorship resistance (forced inclusion) | Simplified (obligations and committee rotation; the shard is not frozen) |
| Ordering: committees, epochs and staking | Built (devnet scale) |
| Ordering: two levels of finality | Built |
| Execution: prove heavy, execute light | Simulated (replay proofs) |
| Execution: N-version proving | Built (two independent replay stacks) |
| Execution: proving market | Not built |
| The Fold: World Proof per Pulse | Simulated (replay proofs, real bindings) |
| Knowledge layer: claims and the Assurance Algebra | Built |
| Verifiable Clearing | Built |
| Privacy: encrypted payloads, viewing keys, sealed orders | Built |
| Privacy: confidential amounts, zero-knowledge compliance | Not built |
| Interfaces: CLI, HTTP API, MCP server for agents | Built |
| Interfaces: x402, AP2, ERC-8004, C2PA adapters | Partly built (x402) |
| Interfaces: the app, a light node on every device (beyond the paper) | Partly built (desktop shell built and run; mobile configured, not built) |
| Payments: streams with checkpoint settlement (beyond the paper) | Built (devnet scale) |
| Memory: logs for agents and robots, kept append-only by the ledger (beyond the paper) | Built (devnet scale) |
| Orchestration: Task Charters, escrowed work with acceptance fixed at posting (beyond the paper) | Built (devnet scale) |
| Economics: fees, treasury, slashing with compensation | Built |
| Economics: staking, rewards, equivocation slashing | Built |
| Economics: storage deals, devices paid to keep data and prove it every round (beyond the paper) | Built (devnet scale) |
| Economics: payments to provers, value-in-flight cap | Not built |
| Networking between machines | Simplified (QUIC between processes, peers from files; a stopped validator cannot rejoin) |

The implementation also adds trust assumptions of its own, such as
`impl:replay-proofs` and `impl:devnet-keys`. They are listed in
[Trust model](security/trust-model.md).

## What the design does not claim

From the architecture paper, stated plainly:

- **It does not make the world honest.** A claim about a physical event is
  only as strong as the bonds and evidence behind it. The design prices
  that risk instead of hiding it.
- **It does not remove stake from safety.** Proofs stop invalid state, not
  equivocation. A Byzantine third of a committee can still certify two
  conflicting spends, and only stake decides which history is canonical.
- **It is not proven at scale.** The paper's capacity figures (100,000
  operations a second on a testnet, 1.6 million at maturity) are design
  targets. This implementation has run as a devnet in one process and as a
  few processes on one machine, and has not been measured against them.

## How to read this book

- To try it, start with [Install from source](getting-started/install.md)
  and [Run the demos](getting-started/demos.md).
- To understand it, read the [Concepts](concepts/neo-blockchains.md) in
  order; each chapter ends with what the implementation does and does not
  do.
- To build on it, use the [Guides](guides/devnet.md) and the
  [Reference](reference/cli.md).
- To change it, read [Architecture of the code](architecture/crates.md),
  the [specification](specification.md) and the repository's
  `CONTRIBUTING.md`.

Command output in this book was captured from real runs of the devnet on one
machine. Identifiers, chain identities, Pulse numbers and timings in your
runs will differ.
