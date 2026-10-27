# Neo-Blockchains: The Fold Architecture

> **About this paper.** This is the architecture paper behind Siontology,
> written in October 2026 before the reference implementation existed. It
> describes the design and its targets, not what runs today. What the
> reference implementation builds, simplifies or simulates is listed in
> [What exists today](../introduction.md#what-exists-today) and in
> [`docs/implementation-status.md`](https://github.com/ColdAI-org/siontology/blob/main/docs/implementation-status.md).
> Figures marked as targets have not been measured on any network.


*ColdAI · 27 October 2026*

## Executive summary

A neo-blockchain is a network whose product is verified knowledge, not ordered transactions. Every claim — about a computation, a payment, an identity, an AI output or a physical event — carries a proof or priced evidence, and anyone can check it on a phone.

- **Why now.** AI made generation nearly free and verification the bottleneck. Trust already costs 206 billion USD a year in financial-crime compliance alone, and AI agents are about to multiply exchanges between strangers. Blockchains did not move like AI because they sold trust in a narrow unit, with heavy friction. The trilemma was not the reason.
- **The invention.** Blockchains chain blocks; DAGs link transactions; neo-blockchains **fold claims**. Four pieces carry it:
  - *Two-sorted state:* knowledge never needs coordination; only the consumption of resources is ordered, and only among contenders.
  - *Assurance Algebra:* every claim carries a formula of its assumptions. Any agent computes what an attacker must forfeit to falsify it, acts only when that exceeds what is at stake, and reserves that backing so no claim is relied on beyond it.
  - *Verifiable Clearing:* a market clearing is valid only with a certificate that no better allocation exists.
  - *The Fold:* every proof folds into one World Proof per Pulse, so a phone verifies the whole history.
- **Post-quantum from day one.** Every authorization needs a lattice signature and a hash-based signature, and every proof is built twice, by two independent stacks.
- **What is new.** Each building block exists in prior work. The combination does not, as far as our search found, and neither do two ledger rules: a policy-relative cost of corruption computed from each claim's assumption formula, and certified optimality as the condition for accepting a market clearing.
- **Targets.** 100,000 operations a second on testnet and 1.6 million at maturity, bounded by data availability and post-quantum signature bandwidth. Owned-object certificates finalize in well under a second; a World Proof lands every 10 to 30 seconds at launch.
- **Build the business before the chain.** Phase 1 sells verifiable AI receipts and Agent Charters on existing chains; the EU's marking deadline for generative systems already on the market is 2 December 2026. The network is built only after 100 million receipts a month, at under 0.0001 USD each, and 10 paying customers.
- **The honest bottom line.** Nobody can promise a hundredfold. Where trust is the binding constraint — cross-border settlement, verifying machine output, agent commerce — two orders of magnitude are plausible and measurable as trust cost saved. Elsewhere the effect will be modest, and every phase is gated so that each claim is tested before the next dollar is spent.

## Diagnosis: why blockchains did not move like AI

AI reached more than a billion weekly users in under four years. Crypto, after seventeen years, has an estimated 40 to 70 million monthly active users, and genuine stablecoin payments are about 0.02% of global payments. The trilemma is not the main reason. Blockchains sold trust in a narrow unit, with heavy friction, to an economy that mostly runs on intermediaries it already trusts.

| Measure | AI | Blockchains |
| --- | --- | --- |
| People using it | ChatGPT alone: more than 1 billion weekly active users, August 2026 ([OpenAI](https://openai.com/index/expanding-access-to-ai-with-chatgpt-ads/)) | 40 to 70 million monthly active users among 716 million owners, October 2025 ([a16z](https://www.a16z.news/p/state-of-crypto-2025)) |
| Organizations | About 9 in 10 use AI regularly and 44% are scaling it enterprise-wide, 2026 ([McKinsey](https://www.mckinsey.com/capabilities/quantumblack/our-insights/the-state-of-ai)) | Genuine stablecoin payments about 390 billion USD a year, about 0.02% of global payments, December 2025 ([McKinsey](https://www.mckinsey.com/industries/financial-services/our-insights/stablecoins-in-payments-what-the-raw-transaction-numbers-miss)) |
| Momentum | ChatGPT weekly users rose from 700 million ([OpenAI](https://openai.com/index/buy-it-in-chatgpt/)) to over 1 billion in about 11 months | Stablecoin supply about flat at 300 billion USD over 12 months, October 2026 ([DefiLlama](https://defillama.com/stablecoins)) |

### Six reasons

1. **A narrow product.** A blockchain makes one thing trustworthy: its own token state. That matters where intermediaries fail or are unwanted, a real but small part of the economy. AI sold a general capability that helps in nearly every job.
2. **Trust stopped at the chain's edge.** A ledger cannot check the facts most transactions depend on: that goods arrived, that a person is who they claim, that a document is real. The guarantee never reached the real economy.
3. **The wrong scaling curve.** Every node re-executes every transaction, so popularity meant congestion and higher fees. AI had the opposite curve: more compute and more use made the product better and cheaper per unit.
4. **Friction where AI had none.** Keys, wallets, gas tokens, bridges and irreversible mistakes, against a text box.
5. **Speculation crowded out use.** In 2025, raw stablecoin volume exceeded 33 trillion USD, adjusted volume was about 11.6 trillion ([Visa and Allium, via Spark](https://www.spark.money/research/stablecoin-transfer-volume-eleven-trillion)), and genuine payments ran at about 0.39 trillion a year. Incentives rewarded token prices more than usefulness.
6. **Public by default.** No business puts payroll, supplier prices or patient records on a public ledger, and privacy arrived late, as an add-on.

### What did work

Stablecoins did: invisible, dollar-denominated settlement that people use without caring which chain carries it. Payment companies now build chains on that lesson. Stripe's [Tempo](https://tempo.xyz/faq) has run since March 2026 with fees paid in stablecoins, and Circle's [Arc](https://www.arc.io/blog/arc-mainnet-goes-live-on-september-16-2026) mainnet went live in September 2026 with Visa and Mastercard among its validators. Rules are firming too: the GENIUS Act takes effect on 18 January 2027 ([OCC](https://www.occ.gov/news-issuances/news-releases/2026/nr-occ-2026-92.html)), though the US market-structure bill failed a Senate cloture vote in September 2026 ([Senate](https://www.senate.gov/legislative/LIS/roll_call_votes/vote1192/vote_119_2_00234.htm)).

### What trust costs today

| Cost | Size | Source |
| --- | --- | --- |
| Transacting: finding partners, verifying, contracting, enforcing | From about a quarter of US GNP in 1870 to about half in 1970 | [Wallis and North, NBER](https://www.nber.org/system/files/chapters/c9679/c9679.pdf) |
| Financial-crime compliance | 206.1 billion USD a year, 2023 | [LexisNexis](https://risk.lexisnexis.com/about-us/press-room/press-release/20230926-global-financial-crime-compliance-costs) |
| Sending 200 USD abroad | 6.36% on average, Q3 2025; 14.99% through banks | [World Bank](https://remittanceprices.worldbank.org/sites/default/files/2026-04/RPW_main_report_and_annex_Q325.pdf) |
| Card acceptance in the US | 198.25 billion USD in merchant fees, 2025 | [Nilson data, via a merchant coalition](https://merchantspaymentscoalition.com/credit-and-debit-card-swipe-fees-reach-record-19825-billion-president-and-congress-call-action) |
| Scams | About 442 billion USD lost across 42 markets in 12 months, 2025 | [GASA and Feedzai](https://www.gasa.org/post/global-scams-on-the-rise-over-half-of-adults-worldwide-report-scam-encounters) |
| Counterfeit goods | Up to 2.3% of world trade, 467 billion USD, 2021 | [OECD and EUIPO](https://www.oecd.org/en/about/news/press-releases/2025/05/global-trade-in-fake-goods-reached-USD-467-billion-posing-risks-to-consumer-safety-and-compromising-intellectual-property.html) |

These costs fall on nearly every exchange between strangers, and AI is about to multiply those exchanges. Gartner expects AI agents to intermediate 90% of B2B buying, more than 15 trillion USD, by 2028 ([Gartner](https://www.gartner.com/en/articles/strategic-predictions-for-2026)). The lesson for what comes next: trust must be general, reach facts off the ledger, get cheaper with scale, stay invisible and be private by default.

## The thesis: collapse the cost of trust

AI collapsed the cost of producing cognition. A neo-blockchain collapses the cost of trusting it. Its product is verified knowledge — claims about computations, data, identities, payments and physical events that anyone can check in milliseconds — not a ledger of ordered token transfers.

The thesis has three parts:

1. **Generation is becoming free; verification is not.** Every AI output, agent action, image and data feed now raises one question: is this real, and who pays if it is not? The cheaper generation gets, the more the bottleneck shifts to verification.
2. **Trust is the economy's largest hidden cost.** Verification, intermediation, compliance, reconciliation and fraud are paid on almost every exchange between strangers. The diagnosis above puts numbers on them.
3. **Blockchains sold trust in the wrong unit.** They made one thing trustworthy — their own token state — by having every node redo every computation. A neo-blockchain extends verifiable trust to anything that can carry evidence, using proofs that cost about the same to check whether they cover one action or a billion.

### Eight inversions

| Blockchains assume | Neo-blockchains invert it | Mechanism |
| --- | --- | --- |
| Verify by re-executing: every node repeats the work | Verify by checking a proof, at near-constant cost | Proof-carrying state; the Fold |
| Totally order every operation | Order only operations that conflict | Two-sorted state; Linear Lanes |
| The ledger knows only its own state | The ledger records claims about the world, with their assumptions priced | Claims; Assurance Algebra |
| Transparent by default | Private by default, provable on demand | Compliance-by-proof |
| A human signs each action | Agents act inside protocol-enforced limits | Agent Charters |
| Market outcomes depend on who orders transactions | Market outcomes carry a certificate that no better allocation exists | Verifiable Clearing |
| Fees rise when demand meets fixed capacity | Cost per verified claim falls as volume grows | Batching, folding, sampling |
| One cryptographic assumption guards each function | Unrelated families guard authorization; independent implementations guard validity | Dual-origin signatures and proofs |

### One backbone, three primitives

Blockchains chain blocks; DAGs link transactions; neo-blockchains **fold claims**. The **Fold** continuously aggregates every proof in the network into one constant-size **World Proof** that a phone can verify in well under a second. Three primitives sit on it:

- **Two-sorted state:** knowledge needs no coordination; only resource consumption is ordered, and only among the parties contending for it.
- **Assurance Algebra:** every claim carries a formula of the assumptions it rests on and a computed cost of corruption, so trust becomes a number an agent can act on.
- **Verifiable Clearing:** markets clear in batches and publish a certificate that no better allocation exists, so ordering power cannot extract value.

*What is genuinely new* (below) separates these from prior art, piece by piece.

## Foundations: two sorts of state, one coordination rule

Every operation either adds knowledge or consumes a resource, and only consumption ever needs coordination. A neo-blockchain makes that split part of its type system and pays for ordering only where the mathematics requires it. We call this the **Coordination Rule**.

**Why it holds.** The CALM theorem states that a distributed program has a consistent, coordination-free implementation exactly when it is monotone: new inputs can add outputs but never retract them ([Hellerstein and Alvaro](https://arxiv.org/abs/1901.01930)). Publishing a verified claim is monotone; it never invalidates another claim. Spending a coin is not; it removes the possibility of a second spend.

**The type behind it.** Linear logic draws the same line ([Girard, 1987](<https://doi.org/10.1016/0304-3975(87)90045-4>)). A linear resource is used exactly once; an exponential fact (!A) can be reused any number of times. Move-style languages already type assets as linear. A neo-blockchain adds a second sort for knowledge, and the compiler derives each operation's coordination class from its type.

**Judgments use snapshots.** Whether a claim is still trusted can change when a challenge lands, so judgments are not monotone. They are evaluated against Pulse snapshots, which the Fold makes globally consistent once per Pulse. Issuing never waits; only reads that need a consistent cut use one.

| Class | Example | What must be prevented | Mechanism | Finality target |
| --- | --- | --- | --- | --- |
| Knowledge (!) | Issue a claim, attest, publish a proof | Nothing: claims never conflict | Availability certificate + proof check | One round trip to the data layer |
| Owned linear | Pay from your balance; move your asset | The owner signing two spends of one version | Quorum certificate (reliable broadcast, no consensus) | One round trip to the committee |
| Shared, bounded | Ticket inventory, budgets, rate limits | Overdrawing a shared total | Single-writer escrow allotments (escrow method, demarcation protocol) | One round trip; rare rebalancing |
| Shared, contended | Liquidity pool, auction, order book | Conflicting updates to one object | Consensus per contention domain + batch clearing | One consensus round + one batch window |

Blockchains run consensus for every row of this table. Transfers between single-owner accounts provably need only reliable broadcast, not consensus ([Guerraoui et al., 2019](https://arxiv.org/abs/1906.05574)). A neo-blockchain runs consensus only for the last row, and only inside the domain in contention. The same result says an object with k writers needs consensus among k, so shared accounts, revocable Charters and recovery policies count as contended.

In code, the sort is part of the type, so a developer cannot get the coordination class wrong:

```
resource Balance { owner: AccountId, amount: u128 }          // linear: moved, never copied
fact     Receipt { model: Hash, input: Commit, output: Hash } // knowledge: freely copied
bounded  Seats   { remaining: u32 }  escrow(per_shard)        // shared total, split into allotments
shared   Pool    { reserves: [u128; 2] } clearing(cfmm)       // contended: batched and cleared
```

## Architecture overview

Eight layers, each with one job, and privacy across all of them. Identities act; the substrate makes data available; lanes and consensus order only what conflicts; provers prove; the Fold compresses; the knowledge layer prices trust; clearing coordinates markets; interfaces hide the machinery. Every layer emits proofs or claims that the layer above consumes.

&#91;embedded content: reference architecture · 6 stacked layers, 2 cross-cutting, interfaces on top\]

Only the highlighted layer ever runs consensus, and only for contended objects; every other layer publishes, proves or prices.

### Lifecycle of a claim

1. **Issue.** A person, device, program or agent creates a claim — a statement over committed data, its evidence and its dependencies — signed under its account or Charter.
2. **Check evidence.** Proofs are verified; attestations are checked and their atoms written into the claim's trust formula.
3. **Publish.** The payload, or its commitment, lands on the substrate and receives an availability certificate.
4. **Order only if needed.** A claim that spends or locks a resource — a fee, a payment, a bond — takes a lane certificate, or a consensus slot if the resource is contended. Pure knowledge skips this step.
5. **Join the knowledge graph.** The claim and its formula enter the grow-only knowledge root.
6. **Fold.** The next World Proof covers the claim, its formula and its bonds.
7. **Use.** A consumer verifies the World Proof and one Merkle path, evaluates the claim's cost of corruption under its own policy, and acts or declines.
8. **Challenge.** Until the window closes, anyone can post counter-evidence. A proven falsehood slashes the bonds behind it and compensates those who relied on it.

## Identity layer: post-quantum accounts, personhood and Agent Charters

Every actor — person, company, device or AI agent — is an account whose identifier never changes while its keys and policies can. Every authorization needs two signatures from unrelated mathematical families. AI agents act only inside charters the protocol itself enforces.

### Accounts

- **Stable identifiers, swappable keys.** An account ID is the hash of its genesis policy. Keys, schemes and recovery rules rotate without changing the ID, so a broken scheme is replaced in place, not migrated around.
- **Policies, not keys.** Authorization is a policy: which keys, how many, which limits, which recovery path. Passkeys, hardware keys, threshold keys and social recovery all fit inside one.
- **Dual-origin by default.** Every account requires an ML-DSA-44 signature (module lattices, [FIPS 204](https://csrc.nist.gov/pubs/fips/204/final)) and a hash-based signature ([FIPS 205](https://csrc.nist.gov/pubs/fips/205/final) SLH-DSA, or stateful XMSS-class keys for frequent signers). Forgery needs both families broken: φ = lattice ∨ hash.
- **Classical keys as an optional third leg.** Device passkeys (P-256) add convenience and device binding, never as the only factor.

| Scheme | Family | Public key | Signature | Role |
| --- | --- | --- | --- | --- |
| ML-DSA-44 | Module lattice | 1,312 B | 2,420 B | First leg, every account |
| SLH-DSA-SHA2-128s | Hash-based, stateless | 32 B | 7,856 B | Second leg, infrequent high-value accounts |
| XMSS^MT, height 40 or 60 | Hash-based, stateful | 68 B | 5,605 to 8,392 B | Second leg, validators and busy agents; keys in non-exportable hardware |
| Ed25519 (for comparison) | Elliptic curve, not post-quantum | 32 B | 64 B | Not used for authorization |

**Where the bytes go.** Validators check signatures and the Fold proves those checks, so the data layer never stores a signature. Bandwidth still pays. A sender's two signatures come to about 8 to 10 KB per transaction, against 64 bytes for Ed25519. Validators therefore never sign individual transactions: at 100 validators, 67 dual-signed votes would add about 540 KB to every certificate. Each validator instead signs one batch root every few tens of milliseconds (Ordering section). Signature bandwidth is the main cost of post-quantum safety, and the reason validators are sharded clusters rather than single machines.

**Why Falcon is not the second leg.** FN-DSA (Falcon) is smaller, with 897-byte keys and signatures of about 666 bytes, but it is also lattice-based, so it cannot be the unrelated family; its standard, FIPS 206, is not yet final ([NIST forum](https://groups.google.com/a/list.nist.gov/g/pqc-forum/c/7z-y5qp-W2Y)). **Aggregation will help.** Ethereum's leanMultisig prototype folds 1,400 XMSS signatures into one 358 KiB proof in 1.18 s, though it is not yet production-ready ([leanMultisig](https://github.com/leanEthereum/leanMultisig)).

### Personhood

- **Prove a property, not an identity.** A person proves *one account per passport* or *adult EU resident* with a zero-knowledge proof over a government credential, such as an ePassport chip or an EU Digital Identity Wallet credential.
- **One pseudonym per application.** Each app gets an unlinkable per-app nullifier, so one-credential-one-vote holds without a global identifier. Uniqueness is per issuer: someone with two passports can hold two pseudonyms.
- **Issuers are atoms.** A personhood claim's trust formula names its issuing authority; a claim combining several issuers takes their ∨.

### Agent Charters

A Charter is the protocol-level contract between a principal and an AI agent: a claim plus a bond.

| Field | What it fixes | Example |
| --- | --- | --- |
| Principal | Who answers for the agent | A company's treasury account |
| Capabilities | What the agent may do, as attenuable caveats | Pay listed suppliers only; at most 2,000 USD a day |
| Delegation | What sub-agents may inherit | Strictly narrower caveats |
| Model terms | Which models and providers may act | Provider attestation required above 500 USD |
| Receipts | Which actions must emit a receipt claim | Every payment and every signed message |
| Bond | Stake behind the agent's receipts | 10,000 USD, slashed if a receipt is proven false |
| Kill switch | Who can revoke, and how fast | The principal; revocation is ordered by consensus and takes effect when it commits |

- **Limits live in the protocol.** Validators and the Fold check every agent operation against its Charter. A hijacked or prompt-injected agent cannot exceed its caveats, because they are not in its code. The worst case is bounded and known in advance. The agent spends from an allotment only it can write, so its payments stay consensus-free; until a revocation commits, the most it can spend is what remains in that allotment.
- **Delegation only narrows.** Attenuation follows the object-capability model of macaroons and Biscuit tokens: each hop adds caveats and none can remove one.
- **Receipts become reputation.** An agent's unchallenged receipts accumulate into reputation claims, which carry trust formulas of their own.

## Data availability substrate

A validity proof shows that a transition was correct; it cannot show that the transition's data was published. The substrate makes withholding detectable by everyone: data is erasure-coded, and every light client samples a few random pieces of it each Pulse.

### How it works

- **Erasure coding.** Each Pulse's data is laid out as a square and extended with a two-dimensional Reed–Solomon code, so any half of a row or column rebuilds it.
- **Hash-based commitments.** Rows and columns are committed with hash-based, FRI-style commitments rather than KZG. KZG needs a trusted setup and pairings, and a quantum computer breaks pairings. Sampling proves availability only if the square is a correct encoding, which hash roots alone do not show, so the World Proof also proves each Pulse's Reed–Solomon encoding, in the style of ZODA.
- **Sampling.** Each light client samples s random shares per Pulse and checks them against the commitment. Making the data unrecoverable requires withholding about a quarter of the extended square, so one client misses the withholding with probability at most:

```latex
\Pr[\text{withholding goes undetected by one client}] \;\le\; \left(\tfrac{3}{4}\right)^{s}
```

At s = 16 that is about 1%; at s = 30, about 0.02%. Thousands of independent samplers make undetected withholding vanishingly unlikely, provided requests are unlinkable so that an adversary cannot answer only the samplers it chooses.

- **Capacity grows with the crowd.** Each client stores and serves the shares it sampled. More light clients means more total capacity, the opposite of a chain where every node stores everything.

### Retention

- **Hot window.** Pulse data stays available for at least the longest claim window plus its challenge period, 104 days in this document's examples, so anyone can rebuild state and file challenges.
- **Archive.** After the window, data moves to paid archivers who post storage proofs. Validity never depends on archives, because the World Proof already covers the past.
- **Owner-held data.** A private claim may keep its payload with its owner and publish only a commitment. Availability of that payload is then the owner's risk, and the claim's schema records that as an atom.

## Ordering: Linear Lanes and Contention Consensus

Most operations never touch consensus. Owned resources finalize with one quorum certificate, shared totals spend down local allotments, and only genuinely contended objects go through consensus. Even there, batches clear at uniform prices, so the order inside a batch carries no value.

### Linear Lanes: owned resources

- **A lane per object.** Every linear object has an owner policy and a version number. Its history is a lane: a chain of versions only its owner can extend.
- **One signature per version.** To spend version v, the owner sends the signed transaction to the committee. Each validator approves (object, v, transaction digest) at most once, and signs its approvals in batches: one signature over the Merkle root of everything it approved in the last few tens of milliseconds.
- **The certificate is finality.** Any 2f + 1 of 3f + 1 signed approvals form a certificate. Two certificates for one version cannot both exist, because any two quorums share an honest validator.
- **No global order.** Execution runs in parallel per object. Nothing waits for unrelated traffic. Objects with several writers, such as shared accounts, Charters and recovery policies, are shared objects and go through Contention Consensus.
- **Check, don't compute.** This is the consensusless path of FastPay and of owned objects in Sui Lutris, extended: heavy operations arrive with proofs, so validators verify instead of re-executing.

Private resources are notes with nullifiers. They use the same mechanism keyed by nullifier: validators certify each nullifier once.

### Escrow Lanes: shared but bounded

- **Split the total.** A shared total Q — seats, a budget, a rate limit, a supply cap — is split into allotments summing to Q, each with a single writer such as a sales channel, a merchant terminal or an agent.
- **Spend locally.** Each writer draws down its allotment with ordinary lane certificates and never coordinates with the others.
- **Rebalance rarely.** When an allotment runs low, a rebalance goes through consensus. The invariant, never exceeding Q, holds throughout.

This is the escrow method and demarcation protocol of distributed databases, applied to a ledger.

### Contention Consensus: shared and contended

- **Contention domains.** Each contended object belongs to a domain: one pool, one auction, one registry.
- **One DAG, many commits.** Validators run a DAG-based BFT protocol of the Mysticeti class, in which every validator proposes in every round. A domain's batch executes as soon as its own part of the DAG commits.
- **Atomic cross-domain operations.** An operation touching several domains is included in one round and executes when every domain it touches commits.
- **Fair order.** Market domains clear by Verifiable Clearing. Other domains order each batch by a hash salted with a beacon value revealed only after the batch seals, so nobody can buy or predict a position.

### Censorship resistance

- **One honest proposer is enough.** Every validator proposes every round, so any honest one can include an intent.
- **Forced inclusion.** An intent posted directly to the data layer with its fee must be included, or proven invalid, within k Pulses. A shard that fails is frozen in the next World Proof and its committee rotated, so censorship stops one shard visibly instead of succeeding silently, while other shards run on.
- **Accountable safety.** Equivocation and approvals of invalid transitions leave signed evidence and are slashed. Stalling, censorship and withholding cannot be attributed, so rotation, forced inclusion and sampling counter them instead.

### Committees and sharding

Validity proofs shrink what a corrupted shard committee can do, but only once they land. Before then, two thirds of a committee could certify an invalid light operation, which the Fold would fail to prove and the signers would be slashed for. More than a third can certify two conflicting spends; any share can stall or censor. So the network caps value in flight, certified but not yet proven, at the committee's slashable stake. Committees of 301 seats sampled by stake make a Byzantine third very unlikely if an adversary holds a fifth of all stake (about 2.5 × 10⁻⁸ per committee) but not if it holds a quarter (about 5.5 × 10⁻⁴). Stake concentration is therefore monitored, and committees grow when it rises. Throughput grows with the number of shards until data availability or signature bandwidth binds.

### Two levels of finality

| Level | What it guarantees | Who relies on it | Target |
| --- | --- | --- | --- |
| Certificate finality | 2f + 1 validators approved it and will not approve a conflict, if fewer than a third are Byzantine | Users, merchants, agents | Under one second |
| Proof finality | The World Proof covers it; anyone can check its validity without trusting the committee, though ruling out a conflicting proof still assumes fewer than a third are Byzantine | Light clients, bridges, auditors | 10 to 30 s at launch; a few seconds at maturity |

## Execution and the proving market

Heavy computation is proven once, by whoever proves it cheapest, and checked by everyone else. Light operations run natively on validators for speed and are proven afterwards for the Fold. Proving has become cheap enough to be the default rather than the exception: the cost to prove an Ethereum block fell from about 1.69 USD in January 2025 ([Wavect](https://wavect.io/blog/zero-knowledge-proofs-production-2026/)) to about 0.003 USD in October 2026 ([Ethproofs](https://ethproofs.org/)), roughly 560 times in 21 months, though the sources measure cost differently.

### Prove heavy, execute light

- **Light operations** — transfers, counter updates, claim registration — run natively on validators so that certificates come fast. Provers prove the same batches afterwards for the Fold.
- **Heavy operations** — checking an AI receipt's proof, verifying a large clearing certificate, running a complex contract — must arrive with a proof, made by the submitter or bought on the market. Nothing heavy is ever replicated.

### The proving stack

- **General programs** run on a RISC-V zkVM; hot paths such as hashing, ML-DSA and hash-based signature checks, Merkle updates and LP certificate checks get dedicated circuits.
- **Hash-based proofs only**, so the stack stays post-quantum. WHIR, for example, reports about 360 µs verification and 63 KiB proofs at 100-bit security ([ePrint 2024/1586](https://eprint.iacr.org/2024/1586)).
- **Recursive STARKs at launch, lattice folding later.** Nova-style folding rests on discrete logarithms and is not post-quantum; lattice schemes such as [LatticeFold+](https://eprint.iacr.org/2025/247) and [Neo/SuperNeo](https://eprint.iacr.org/2026/242) are, but current code is a proof of concept ([LatticeFold](https://github.com/NethermindEth/latticefold)).
- **128-bit security for both stacks.** Both stacks should meet the 128-bit bar that the Ethereum Foundation's [soundcalc](https://github.com/ethereum/soundcalc/blob/main/reports/summary.md) ratings track; some leading zkVMs are rated at 100 bits today.

### What the frontier shows

| Date | Result | Source |
| --- | --- | --- |
| Oct 2026 | Live tracker: 69% of 401,928 Ethereum slots proven within 10 s; about 0.003 USD per proof on 4 GPUs | [Ethproofs](https://ethproofs.org/) |
| Oct 2026 | Airbender Prime: 99% of 7,200 blocks within 10 s on one GPU; security level not stated (press report) | [KuCoin News](https://www.kucoin.com/news/flash/zksync-launches-airbender-prime-achieves-ethereum-real-time-proofs-on-single-gpu) |
| Jul 2026 | OpenVM 2.1 preview: 99th percentile 9.7 s on 4 RTX 5090 GPUs | [Axiom](https://www.axiom.xyz/blog/2-1) |
| Nov 2025 | SP1 Hypercube: 99.7% of blocks within 12 s on 16 RTX 5090 GPUs | [Succinct](https://blog.succinct.xyz/real-time-proving-16-gpus/) |
| Jul 2025 | Ethereum Foundation target: 99% of blocks within 10 s on at most 100,000 USD of hardware and 10 kW | [Ethereum Foundation](https://blog.ethereum.org/2025/07/10/realtime-proving) |
| May 2025 | SP1 Hypercube: 93% of blocks within 12 s on about 160 to 200 RTX 4090 GPUs | [Succinct](https://blog.succinct.xyz/sp1-hypercube/) |

Most figures are the vendors' own; Ethproofs is the main independent measurement.

### The proving market

- **Jobs, not blocks.** Each job carries a deadline, a maximum fee and a latency class: real-time for Fold leaves and roots, batch for receipts and audits.
- **Collateral and delivery.** Provers bid and post collateral, are paid on delivery of a valid proof, and forfeit collateral when they miss a deadline; the job is then re-auctioned.
- **Useful work.** Any hardware competes, GPUs today and accelerators later. Proving is the network's equivalent of mining, except that every output is a proof someone needs.
- **Markets already exist.** Succinct reported more than 6 million proofs in 2025 across 25 staked provers ([Succinct](https://blog.succinct.xyz/succinct-2025-recap/)).

### Verifiable AI inference, in three tiers

| Tier | Evidence | Where it stands | Trust formula |
| --- | --- | --- | --- |
| Proof | A zero-knowledge proof of the inference | Small and mid-size models: GPT-2 in about 38 s on a laptop ([Jolt Atlas](https://arxiv.org/pdf/2602.17452)); Gemma 3 at 86 tokens a minute ([DeepProve](https://eprint.iacr.org/2026/1112)). Frontier scale: sampled proofs only | hash ∧ circuit |
| Attestation | A GPU confidential-computing quote | Practical at frontier scale; H100 confidential mode cost 6.85% of throughput on an 8B model and about 0% on a 70B model ([study](https://arxiv.org/html/2409.03992v4)). Physical attacks weaken it (Security section) | TEE resists its host ∨ bonded operator |
| Bond | Operator stake behind the receipt, with spot checks | Any model, today | bonded operator |

Tiers stack. A receipt can carry a TEE quote and an operator bond today and gain a sampled proof later; each consumer's policy decides what it needs.

## The Fold: one proof for the whole world

Every Pulse, the network folds all proofs produced since the last Pulse into one World Proof that certifies the entire history from genesis. Checking it costs the same in year ten as on day one, so a phone is as certain of the state's validity as a datacenter.

&#91;embedded content: the Fold · three levels shown of about 20 for a million leaves\]

Competing provers build both trees in parallel, one per stack, and every consumer checks the pair of roots.

### What the World Proof states

The World Proof for Pulse t proves that a history exists from genesis to state root Rₜ in which:

1. every linear transition followed its object's rules and carried a valid committee certificate;
2. every contended batch cleared under a valid optimality certificate;
3. the knowledge root only grew, and every claim's trust formula and bond references are correct;
4. every forced-inclusion obligation older than k Pulses was met;
5. availability certificates exist for all data the history depends on;
6. every committee change followed the staking rules, so the chain of signers is valid from genesis.

### How it is built

- **Leaves in parallel.** Each lane batch, contention-domain batch and claim batch gets its own proof from whichever prover wins the job.
- **A tree of aggregation.** Provers fold or recursively aggregate proofs pairwise. A million leaves need about 20 levels.
- **Pipelining.** Pulse t+1 starts aggregating while Pulse t's root finishes, so the slowest level sets throughput, not the tree's depth. Proof finality lags certificate finality by the pipeline's length.
- **Two proof trees, end to end.** Two independent proving stacks, with different code and different arithmetizations, each prove every leaf and every level. A World Proof is the pair of roots, and both must verify. This roughly doubles proving cost; in return, a soundness bug in one stack cannot forge the world. A bug in the shared specification still could, so the specification is itself an atom.

### What it buys

- **Every light client is a full verifier.** A phone holds the World Proof and Merkle paths for the state it cares about, and trusts no validator for validity.
- **Instant bootstrap.** A new node verifies one proof instead of replaying history.
- **History becomes optional.** Old data serves archives and auditors; it is not needed to verify the present.
- **Bridges become verification.** Any chain that can verify the World Proof can follow this one without a trusted committee. Inbound, other chains' proofs enter as claims, with their assumptions as atoms.

### Cadence

The launch target is a World Proof every 10 to 30 seconds. The maturity target is one per second, with proof finality a few seconds behind certificate finality. Recent real-time proving results (Proving section) make the maturity target plausible, but it is not yet demonstrated for this workload.

## Knowledge layer: claims and the Assurance Algebra

Every claim carries a formula of the assumptions it rests on, and from that formula any consumer computes the minimum stake an attacker must forfeit to make the claim false. An agent trusts a claim exactly when that **cost of corruption** exceeds what it has at stake. The oracle problem — a ledger cannot see the world — stops being an unsolvable flaw and becomes a priced, bounded risk.

### The claim

A claim is the network's atomic unit of knowledge.

| Field | Holds | Example |
| --- | --- | --- |
| Statement | A typed predicate over committed data | Output hash H(y) came from model M on committed input C(x) |
| Evidence | Proof, attestation or bond reference | STARK proof; TEE quote; bond reference |
| Issuer | An account or an Agent Charter | A supplier's procurement agent |
| Trust formula φ | The assumptions the statement rests on | (TEE resists its host) ∨ (bonded operator) |
| Backing | Bonds locked for the claim's window | 50,000 USD |
| Dependencies | Claims used as inputs | Two earlier claim IDs |
| Window | Validity period, then a challenge period | 90 days + 14 days |

### Trust formulas

Atoms are irreducible assumptions: a hash function's collision resistance, lattice hardness, a TEE's resistance to its own host, a circuit's correctness, an attester's honesty. Formulas combine atoms by the two rules of provenance semirings from database theory ([Green, Karvounarakis and Tannen, 2007](https://doi.org/10.1145/1265530.1265535)):

- **Joint use (∧).** A claim derived from inputs c1 and c2 by a proven computation p gets φ(c1) ∧ φ(c2) ∧ φ(p). It fails if any part fails.
- **Alternatives (∨).** A claim backed by independent routes — two proof systems, two signature schemes, several witnesses — gets the ∨ of the routes. It fails only if every route fails.

Three patterns recur throughout this design:

- **Dual-origin signature.** ML-DSA-44 and a hash-based signature are both required, so forging needs both schemes broken: φ = lattice ∨ hash.
- **Dual proof.** Two independent proving stacks must both verify: φ = (stack A ∧ circuit A) ∨ (stack B ∧ circuit B).
- **Witness quorum.** Three bonded witnesses sign *shipment received*. An honest witness signs only true statements, so φ = W1 ∨ W2 ∨ W3.

### Cost of corruption

Cost of corruption is relative to the consumer's policy 𝒫, the set of assumptions it is willing to trust. Each atom gets a cost:

| Atom class | Examples | cost(a) |
| --- | --- | --- |
| Cryptographic | Hash collision resistance; lattice (MLWE/MSIS) hardness | ∞ if in 𝒫, else 0 |
| Code | A circuit or program meets its specification | ∞ if in 𝒫 (audited, verified), else 0 |
| Hardware | A TEE resisting its own host; a sensor's secure element | The cheapest known attack's cost, never ∞ for a TEE (Security section); usually paired with an operator bond |
| Institutional | A bank's KYC record; a public registry | ∞ if in 𝒫, else its bond if posted, else 0 |
| Economic | A bonded attester's honesty | Its slashable bond, in USD |

The cost of corruption is the cheapest set of atom failures that falsifies φ. It is computed bottom-up:

```latex
\mathrm{CoC}(a)=\mathrm{cost}(a),\qquad \mathrm{CoC}(\varphi_1\wedge\varphi_2)=\min\{\mathrm{CoC}(\varphi_1),\,\mathrm{CoC}(\varphi_2)\},\qquad \mathrm{CoC}(\varphi_1\vee\varphi_2)=\mathrm{CoC}(\varphi_1)+\mathrm{CoC}(\varphi_2)
```

The sum rule is exact when alternatives share no atoms. With shared atoms the exact value is a minimum-weight hitting-set problem, NP-hard in general. The network then reports a sound interval: max in place of sum gives the lower bound, sum gives the upper. The lower bound holds by induction, because breaking φ1 ∨ φ2 means breaking both alternatives, which costs at least the dearer one. Falsifying φ is an attack tree with ∧ and ∨ swapped, and that literature has exact algorithms for small shared cases (\[Lopuhää-Zwakenberg, Budde and Stoelinga\](https://arxiv.org/pdf/2212.05358)). Schemas can require atom-disjoint alternatives, which restores exact additive security.

Two properties follow:

- **Independent evidence adds up.** Three disjoint witnesses bonded at 20,000 USD each give a cost of corruption of 60,000 USD. Truth gets harder to fake as participation grows.
- **Sybils gain nothing.** One party running all three witnesses still forfeits all three bonds once the lie is proven.

### The decision rule

An agent with value V at stake accepts claim c when

```latex
\min\!\Big(\delta_c\cdot\mathrm{CoC}^{\mathrm{low}}_{\mathcal{P}}(c),\;\mathrm{CoC}(\text{adjudicator}_c)\Big)\;\ge\;\kappa\cdot V
```

where κ ≥ 1 is a safety factor, δ ≤ 1 is the claim's detection discount, and the adjudicator term caps trust at the cost of corrupting the dispute process. The agent then reserves κ · V of the claim's backing, as described below. The network proves that φ and the bonds behind it are correct. Each consumer evaluates its own policy locally, in time linear in the formula's size.

**Worked example.** An agent must decide whether to pay invoice #1042 for 25,000 USD. The claim *invoice payable* joins three inputs: delivery confirmed by three independent witnesses bonded at 20,000 USD each; the invoice arithmetic proven by two proving stacks; the supplier's identity credential from a bank bonding 1,000,000 USD. The cost of corruption is min(60,000; ∞; 1,000,000) = 60,000 USD, and delivery is the weak link. With κ = 2 the agent pays invoices up to 30,000 USD automatically; above that it asks for a fourth witness or a signed sensor reading. If its policy distrusts stack A, stack B still carries the proof branch to ∞; with one stack, the payment would stop. If three financiers each wanted to lend 25,000 USD against the same invoice, the first would reserve 50,000 USD of the 60,000 USD backing, and the second would be refused until more evidence was posted.

&#91;embedded content: worked example from this section · 3 inputs, 6 atoms\]

The AND at the top takes its weakest input, so the three witnesses — not the proofs or the bank — decide how much value this claim can safely carry.

### Reliance is reserved, not assumed

Cost of corruption is per claim, but reliance adds up across consumers. Three agents each risking 25,000 USD on a claim backed by 60,000 USD would all pass the rule at κ = 2, and a fraud would net 75,000 USD for 60,000 forfeited. So every acceptance reserves κ · V of the claim's backing, pushed down its formula:

- **Each ∧ input carries the full reservation,** because breaking any one input breaks the claim.
- **The ∨ alternatives share it,** because all of them must be broken.
- **A shared atom carries the sum** of every reservation that reaches it, and no bond can be reserved beyond its size.

A bond's reservable capacity is a shared, bounded total, so reservations run on Escrow Lanes and are released when the reliance window ends. The value relying on any falsified claim is then at most the slashed bonds divided by κ, so victims can be paid in full. Reliance without a reservation is allowed, but uninsured.

### Bonds, challenges and insurance

- **Bonds are linear resources.** They stay locked for the claim's window plus its challenge period, and until every claim derived from it expires, so a claim's cost of corruption cannot silently fall after someone relies on it.
- **Anyone can challenge.** A contradiction that is cryptographically checkable slashes automatically. Anything else goes to the adjudicator named in the claim's schema, which is itself an atom with its own bond.
- **Victims are paid first.** Slashed bonds compensate the consumers who relied on the false claim before any reward to the challenger.
- **Insurance is priced from the same formula.** For independent atoms, P(φ1 ∧ φ2 fails) = 1 − (1 − p1)(1 − p2) and P(φ1 ∨ φ2 fails) = p1 · p2. Correlated atoms need explicit correlation models.
- **Undetectable lies stay cheap.** Cost of corruption counts stake that is slashable once a falsehood is proven. Schemas for claims that are rarely disprovable carry a detection discount, and agents should treat them as weak.

### Why this matters for AI

An agent cannot judge a document's authenticity the way a person does, but it can evaluate a formula and compare two numbers. The decision rule gives every agent the same machine-checkable trust test. Forged or prompt-injected inputs fail it unless someone puts real money behind them.

## Verifiable Clearing: markets that prove they are optimal

Contended markets clear in sealed batches, and the winning solver must submit, with its allocation, a certificate that no better allocation exists. Checking the certificate takes linear time however hard the problem was to solve. Anyone can run a solver, and nobody can quietly extract value by controlling the order of transactions.

### How a batch clears

1. **Intents arrive encrypted** during a batch window, for example 250 ms. An intent states constraints and a limit, such as *8 GPU-hours before 18:00 at no more than 2.10 USD per hour*.
2. **The batch seals.** Its contents are fixed before anyone can read them; then the committee decrypts it.
3. **Solvers race** for a short solving window. Each submits an allocation x and a price vector y.
4. **The network checks every certificate** and keeps the best objective. Ties break by a public randomness beacon.
5. **Everyone in the batch trades at the same prices y.** A position inside the batch is worth nothing, so there is nothing to front-run.

### The certificate

When the clearing problem is a linear program, the solver's allocation solves the primal and its prices solve the dual:

```latex
\max_{x}\; c^{\top}x \;\;\text{s.t.}\;\; Ax \le b,\; x \ge 0 \qquad\qquad \min_{y}\; b^{\top}y \;\;\text{s.t.}\;\; A^{\top}y \ge c,\; y \ge 0
```

The network accepts the pair (x\*, y\*) when all five checks pass:

```latex
Ax^{*} \le b,\qquad x^{*} \ge 0,\qquad A^{\top}y^{*} \ge c,\qquad y^{*} \ge 0,\qquad c^{\top}x^{*} \;\ge\; b^{\top}y^{*} - \varepsilon
```

By weak duality, no feasible allocation scores above bᵀy\*, so x\* is within ε of optimal. The duals y\* are the clearing prices, the role locational marginal prices already play in electricity markets. When intents are private, the same five checks run inside a zero-knowledge proof over committed values.

### What it covers

| Problem class | Markets it fits | Certificate strength |
| --- | --- | --- |
| Linear programs | Bids priced in one numeraire, such as GPU-hours or energy in USD; DC optimal power flow | Exact: zero duality gap in exact arithmetic |
| Single-commodity flow, assignment, transportation | Ride, job and compute-slot matching; single-commodity routing | Exact and integral for integer data; the checker verifies integrality |
| Convex programs | Token-to-token exchange as a Fisher or Arrow–Debreu market; routing across constant-function market makers | ε-exact: duality gap and KKT residuals, plus per-intent limit checks |
| Integer and combinatorial | Package bids; indivisible bundles; multi-commodity freight | A certified gap against an LP or Lagrangian bound |

### Limits, stated plainly

- **Optimal for what was reported.** The certificate proves optimality for the intents as submitted. Bidders can still misreport, and uniform-price batch auctions are not fully strategy-proof.
- **Exact means exact.** Rows marked exact require ε = 0 in rational arithmetic, plus a check that every filled intent pays no more than its limit; with ε > 0, a filled bid could pay above it.
- **Ties are broken canonically.** When several allocations are optimal, the solver must not get to pick the winners. The objective gets a tiny perturbation drawn from the beacon after the batch seals, which makes the optimum unique with high probability, and the certificate covers the perturbed program.
- **Objectives must be public.** Each contention domain publishes its objective and constraint templates; preferences that are neither linear nor convex fall to the combinatorial row.
- **Latency costs one batch.** The solving window adds a few hundred milliseconds, the price of a market that ordering power cannot exploit.

### Why it matters

Today a marketplace's matching engine is a black box, and platforms often charge double-digit take rates for running it. Verifiable Clearing makes the engine open, competitive and auditable. Solvers compete on certified quality, so the platform's rent falls toward the cost of computation.

## Privacy and compliance-by-proof

State is private by default: the network stores commitments, and proofs show that rules were followed without revealing the data. Regulators and counterparties get stronger assurance than a report gives them today, through proofs and scoped disclosure rather than public surveillance.

### Private by default

- **Commitments, not values.** Balances, claim payloads and intents are stored as hiding commitments; every transition over them is proven in zero knowledge.
- **Notes and nullifiers.** Private linear resources are notes; each nullifier is certified exactly once, as in the Ordering section.
- **Scoped viewing keys.** An owner can grant an auditor read access to one account, one period or one claim type, and nothing more.
- **Post-quantum confidentiality.** Encrypted payloads use ML-KEM ([FIPS 203](https://csrc.nist.gov/pubs/fips/203/final)), so data harvested today stays unreadable to a future quantum computer.

### Compliance as a proof

| Obligation | What the proof shows | What stays private |
| --- | --- | --- |
| KYC | Both parties hold a credential from an accredited issuer, issued within 12 months | Names, documents and, if desired, which issuer |
| Sanctions screening | Neither party appears in the sanctions list committed at time t | Who the parties are |
| Reporting thresholds | The amount is below the threshold, or a filed-report claim exists | The amount, when below the threshold |
| Travel Rule | Originator and beneficiary data were encrypted to the counterparty's provider | That data, from everyone else |
| AI-content marking | The output carries a receipt from a declared model and provider | Prompt and inputs |

A proof shows that the rule as written was applied to the committed data. Whether a regulator accepts a proof in place of a record is a legal question in each jurisdiction, and the first deployments should run proofs alongside existing reporting.

### Private computation

Joint computations on secrets — sealed-bid auctions, private order matching, pooled risk models — run in MPC or FHE coprocessors with a proof of correct decryption, or inside TEEs. Each method enters the result's trust formula as atoms: an honest-minority assumption for MPC, lattice hardness for FHE, TEE resists its host ∨ a bonded operator for a TEE. Consumers always see what they are relying on.

## Interfaces: invisible to people, native to agents

People should never see a key, a gas fee or a chain; agents should get the whole network as a handful of tools. The interfaces plug into the standards that agents and payment networks are already adopting instead of competing with them.

### For people

- **Sign in with a passkey.** Post-quantum keys live in the device's secure hardware; recovery follows the account policy, through guardians or an institution.
- **Pay fees in stablecoins.** Apps can sponsor fees; nobody needs a native gas token.
- **Read intents and receipts as sentences.** *Pay supplier X 2,400 USD by Friday*, not a hexadecimal payload.

### For AI agents

The network ships as an MCP server with a few tools: issue a claim, verify a claim under a policy, pay under a Charter, submit an intent, fetch a receipt. [MCP now sits with the Agentic AI Foundation](https://blog.modelcontextprotocol.io/posts/2025-12-09-mcp-joins-agentic-ai-foundation/) under the Linux Foundation; at the December 2025 donation it counted more than 97 million monthly SDK downloads and about 10,000 active servers.

| Standard | What it gives agents today | What the neo-blockchain adds |
| --- | --- | --- |
| [MCP](https://blog.modelcontextprotocol.io/posts/2025-12-09-mcp-joins-agentic-ai-foundation/) | Tool access for agents | Claims, receipts and payments as tools |
| [x402](https://www.linuxfoundation.org/press/linux-foundation-is-launching-the-x402-foundation-and-welcoming-the-contribution-of-the-x402-protocol) | Pay per HTTP request in stablecoins; a Linux Foundation project whose members include Visa, Mastercard, Stripe, Google and AWS ([CoinDesk](https://www.coindesk.com/tech/2026/07/15/visa-mastercard-and-ripple-join-the-standard-letting-ai-agents-pay-in-stablecoins)) | Charter limits, bonded accountability, priced receipts |
| [AP2 with Verifiable Intent](https://blog.google/products-and-platforms/platforms/google-pay/agent-payments-protocol-fido-alliance/) | Signed user mandates for agent purchases, now at the FIDO Alliance | Mandates as claims whose cost of corruption a merchant can check |
| [ERC-8004](https://eips.ethereum.org/EIPS/eip-8004) | Agent identity and reputation registries, still a draft standard | Bonds that make reputation expensive to fake |
| [C2PA 2.5](https://spec.c2pa.org/specifications/specifications/2.5/specs/C2PA_Specification.html) | Signed provenance manifests on media | Manifests anchored as claims, so they survive metadata stripping and revocations show up |

### Why adapters, not a rival standard

The rails exist, but real use is thin and noisy. An Artemis and Visa analysis cut x402's raw count of 178.3 million payments worth 135.7 million USD to 109.6 million worth about 15 million USD once wash activity was removed, as of April 2026 ([summary](https://www.danielmcglynn.com/the-x402-counter-has-shown-the-same-four-numbers-since-march/)). One tracker counted 519,333 ERC-8004 agents by September 2026, of which about 2.2% passed basic health checks, with Sybil-like reviews ([Datawallet](https://www.datawallet.com/crypto/erc-8004-explained)). What is missing is not another payment pipe but accountability: who stands behind an agent, within what limits, and what it costs to lie. Charters and the Assurance Algebra add exactly that.

### For enterprises and developers

- **APIs and SDKs** over REST and gRPC, with typed client libraries.
- **A contract language with sorts.** Resource, fact, bounded and shared types, as in Foundations, so the coordination class is derived, not guessed.
- **Private subnets:** permissioned lanes whose proofs fold into the public World Proof.
- **A provenance bridge.** Media manifests are registered as claims and found again by content hash after a platform strips metadata. In one Washington Post test, none of eight platforms kept an AI video's credentials ([report](https://fredritchin.substack.com/p/major-platforms-are-not-labeling)).
- **Anchoring to existing chains** such as Ethereum, Hedera and Solana during Phases 0 and 1.

## Trilemma resolution and capacity model

Security comes from proofs, scalability from parallel lanes and proving, and decentralization from verification cheap enough for a phone. Data availability is the binding constraint on throughput — and throughput was never what held blockchains back.

### How each corner is met

| Corner | Blockchains trade it away through | Neo-blockchain mechanism | What it still rests on |
| --- | --- | --- | --- |
| Security | An honest majority re-executing everything | Validity proofs from two stacks; dual-origin signatures | Proof soundness; under a third Byzantine stake for ordering |
| Scalability | Bigger nodes, or shards that can steal | Owned lanes in parallel; consensus only per contention domain; shards whose committees cannot mint once proofs land | Data-availability bandwidth |
| Decentralization | Small nodes, because everyone re-executes | Verification on a phone; sampling capacity that grows with the crowd; proving open to anyone | Ordering committees of hundreds, not the number of verifiers |

### Capacity model

```latex
T \;\le\; \min\Big(\sum_{\text{shards}} T_{\text{lane}} + \sum_{\text{domains}} T_{\text{domain}},\;\; \frac{B_{\text{DA}}}{\bar{s}},\;\; \frac{B_{\text{val}}}{\bar{b}},\;\; \frac{P}{\bar{p}}\Big)
```

T\_lane is owned-lane capacity per shard and T\_domain consensus capacity per contention domain. B\_DA is data-availability bandwidth and s̄ the mean bytes published per operation. B\_val is a validator cluster's ingress bandwidth and b̄ the signature bytes per operation reaching it, about 8 to 10 KB. P is proving throughput and p̄ the proving work per operation, doubled by the two proof trees. Proofs keep signatures off the data layer, so s̄ is the state change alone: about 32 bytes for a public transfer and at least 96 bytes for a private one, a nullifier and two commitments. Both figures are still to be measured.

| Quantity | Gate 3 testnet | Maturity | Basis |
| --- | --- | --- | --- |
| Owned-lane capacity per shard | 50,000 ops/s | 50,000 ops/s | A target that depends on batch voting and post-quantum signature aggregation. FastPay's lab result of over 80,000 TPS with 20 authorities used 64-byte classical signatures and many shards per authority ([FastPay](https://arxiv.org/abs/2003.11506)) |
| Shards | 2 | 32 | Design choice |
| Simple operations per second | 100,000 | 1.6 million | Shards × lane capacity |
| Data availability, public transfers | 3.2 MB/s | 51 MB/s | Operations × 32 bytes |
| Data availability, private transfers | 9.6 MB/s | 154 MB/s | Operations × 96 bytes |
| Sender-signature ingress per shard | 400 to 500 MB/s | 400 to 500 MB/s | 50,000 ops/s × 8 to 10 KB |
| Batch statements, such as receipts | Not bounded by the ledger | Not bounded by the ledger | One claim commits to millions of statements |

&#91;embedded content: Ethereum: EIP-4844 blob size × BPO2 parameters, 12 s slots (calc); Celestia: Celestia blog, Jan and Oct 2026; needs: the table above\]

Sampled data availability must grow 10 to 30 times for the maturity target, depending on how much traffic is private. Until it does, some lanes can use committee-attested data, and that choice is recorded as an atom in every affected claim's trust formula.

### Latency targets

| Path | Target | Evidence it is reachable |
| --- | --- | --- |
| Owned-lane certificate | 0.1 to 0.5 s, plus one batch-vote interval | FastPay: under 100 ms in the lab; Sui documents owned-object certification in under 0.5 s ([Sui](https://docs.sui.io/concepts/transactions/transaction-lifecycle)) |
| Contended commit | About 0.5 s plus one batch window | Mysticeti: about 400 ms on Sui mainnet ([Decentralized Thoughts](https://decentralizedthoughts.github.io/2026-03-06-mysticeti-revolutionizing-consensus-on-sui/)) and 0.5 s at over 200,000 TPS in its paper ([arXiv](https://arxiv.org/abs/2310.14821)) |
| Sealed-batch overhead | Tens of milliseconds | Aptos's opt-in encrypted mempool, live since October 2026, added about 20 ms per block in benchmarks, though it uses pairings, which are not post-quantum ([Aptos](https://aptosnetwork.com/currents/encrypted-mempool-mainnet)) |
| Proof finality | 10 to 30 s at launch; a few seconds at maturity | Real-time Ethereum proving at a 99th percentile under 10 s on 4 GPUs (Proving section) |
| Light-client check | Well under 1 s on a phone, still to be measured | WHIR verifies in about 360 µs on a desktop (Proving section) |

### Throughput was never the bottleneck

Production chains already have far more capacity than demand. Monad, designed for 10,000 TPS ([Monad docs](https://docs.monad.xyz/introduction/why-monad)), processed 860.1 million transactions from its November 2025 launch to 30 September 2026 ([Monad](https://monad.xyz/blog/monad-ecosystem-highlights-september-2026)): about 32 per second on average. That is why this plan sells trust first and scales throughput second.

## Security model and threat analysis

Validity is cryptographic; ordering safety, liveness and censorship resistance rest on committee stake; facts about the world rest on priced assumptions. Every assumption is named. Equivocation leaves evidence and is slashed; other failures are bounded by rotation, forced inclusion and a cap on value in flight.

### What each property rests on

| Property | Rests on | If the assumption fails |
| --- | --- | --- |
| Validity of every transition | Hash-based proof soundness, correct circuits and a correct specification, checked by two independent proof trees | Both stacks must fail together, or the specification must be wrong |
| Ordering safety: no double spend | Fewer than a third Byzantine stake in each committee; partial synchrony | Conflicting certificates appear; signed evidence slashes the equivocators |
| Liveness and inclusion | Liveness: fewer than a third Byzantine stake and partial synchrony. Inclusion: one honest proposer per round, plus forced inclusion | Progress stalls visibly; a censoring shard is frozen and its committee rotated |
| Data availability | Enough honest samplers whose requests cannot be linked, and a proven-correct encoding | Withholding is detected and the Pulse is rejected |
| Authorization | Lattice ∨ hash signature families | Both families must break |
| Facts about the world | Each claim's trust formula and bonds | Losses up to the cost of corruption, compensated from slashed bonds |

### Threats and mitigations

| Threat | Mitigation |
| --- | --- |
| Soundness or circuit bug. In June 2026 a bug in stwo-cairo allowed forged proofs before it was fixed ([Axiom](https://www.axiom.xyz/blog/stwo-bug)) | Two end-to-end proof trees; formal verification; value caps until audited; bug bounties |
| Equivocation by a committee | Quorum intersection; slashing on signed evidence; committees sized against a Byzantine third; value in flight capped at slashable stake |
| Censorship | Every validator proposes every round; forced inclusion within k Pulses, or the shard is frozen |
| Data withholding | Two-dimensional erasure coding; sampling with unlinkable requests |
| Front-running and MEV | Sealed batches decrypted by a committee threshold, itself an atom; uniform clearing prices; optimality certificates |
| Long-range rewriting with old keys | Validators sign with forward-secure hash-based keys: seeds evolve one way and used one-time keys are erased, so old keys cannot sign an alternative history; erasure is itself an atom. New nodes check validator-set history inside the World Proof |
| TEE compromise | TEEs are atoms, never anchors. Memory-bus devices costing under 1,000 USD broke attestation in [Battering RAM](https://batteringram.eu/), [WireTap](https://wiretap.fail/), [TEE.fail](https://tee.fail/) and [DDRop](https://thehackernews.com/2026/09/new-ddrop-attack-breaks-intel-tdx-and.html) (2025 to 2026), and TEE.fail showed GPU attestations can be relayed. The atom is a TEE resisting its own host, priced at the cheapest known attack, and receipts pair TEE quotes with operator bonds |
| Sybil attesters | Cost of corruption counts bonds, so every fake identity must post its own |
| Hijacked or prompt-injected agents | Charter caveats enforced by the protocol; kill switch; bonded receipts |
| Quantum computers | Hash-based proofs and commitments; lattice ∨ hash signatures; ML-KEM encryption; keys swappable without changing account IDs |
| Governance capture | Time-locked upgrades; a supermajority of stake; time for users to exit before a change takes effect |

### What the design does not claim

- **It does not make the world honest.** A claim about a physical event is only as strong as the bonds and evidence behind it; the design prices that risk instead of hiding it.
- **It does not remove stake from safety.** Proofs stop invalid state, not equivocation; a Byzantine third of a committee can still double-certify, and is slashed for it. Two conflicting World Proofs could then exist, and only stake decides which is canonical.
- **It is not yet proven at scale.** The capacity figures in the trilemma section are design targets, to be met or missed on a public testnet.

## Economics and the positive scaling law

The cost of one verified claim falls as volume grows, because each Pulse's fixed costs are shared by every claim in it and checking the World Proof costs the same at any volume. Fees are paid in stable units for work actually done. A staking asset, if there is one, secures the committee; it does not meter usage.

### The cost of one claim

With n claims in a Pulse and s bytes published per claim:

```latex
c(n) \;=\; \underbrace{c_{\text{data}}\cdot s}_{\text{bytes published}} \;+\; \underbrace{c_{\text{prove}}}_{\text{marginal proving}} \;+\; \underbrace{\frac{C_{\text{pulse}}}{n}}_{\text{root proof, committee, anchoring}}
```

- **The fixed term shrinks with volume.** The root proof, the committee's work and external anchoring cost about the same per Pulse whether it holds a thousand claims or a million.
- **The marginal terms shrink with technology.** Proving costs fall with algorithms and hardware (Proving section), though the two proof trees double them. Data costs fall as more light clients add sampling capacity.
- **Batch claims break the link to ledger throughput.** One claim can commit to a Merkle root over m statements, such as a second's worth of an AI provider's receipts. Each statement then costs c(n)/m to anchor, and its verifier pays about log2(m) hashes to check inclusion.
- **Congestion is priced locally.** Finite resources still need prices. A hot market raises fees in its own contention domain and on data bytes, not for unrelated payments.

This is the reverse of the blockchain experience, where popularity raised everyone's fees. It is the kind of property that let the internet compound: growth made the system cheaper for the next user.

### Who pays whom

| Payer | Pays for | Unit | Earned by |
| --- | --- | --- | --- |
| Issuers | Publishing claims and data | Per byte, per claim | Data operators and archivers |
| Spenders | Lane certificates | Per operation | Validators |
| Market participants | A consensus slot and clearing | Per intent cleared | Validators and winning solvers |
| Anyone with heavy computation | A proof | Per job, at market price | Provers |
| Consumers who want more assurance | Bonds and insurance | Premium per claim | Attesters and insurers |

### The staking question

Committee safety still rests on stake: fewer than a third of each committee's stake may be Byzantine. There are two ways to provide it.

| Option | For | Against |
| --- | --- | --- |
| Native staking asset | Familiar model; funds the ecosystem; validators share the upside | Securities and MiCA exposure; security depends on a volatile price; invites speculation over usage |
| Stake in stable or external assets | Security and cost of corruption both in USD; no reflexive token | Validator yield must come from fees from day one; harder to bootstrap |

Recommendation: run Phases 0 and 1 without a token, with fees in stablecoins. Decide the staking design before the Phase 2 testnet, with legal advice and real fee data in hand.

## What is genuinely new: the prior-art map

No single primitive here is new from first principles. What is new is the combination, and two ledger rules that no system we found enforces: a policy-relative cost of corruption computed from each claim's assumption formula, and certified optimality as the condition for accepting a market clearing. The type system that derives coordination classes is a synthesis of known results. Most platform shifts look like this; the web combined hypertext, internet protocols and markup that already existed.

| Component | Closest prior work | What it already does | What this design adds | Verdict |
| --- | --- | --- | --- | --- |
| Assurance Algebra | [Provenance semirings](https://www.cis.upenn.edu/~val/15MayPODS.pdf) and [ORCHESTRA](https://www.cs.ucdavis.edu/~green/papers/vldb07.pdf) (Green et al., 2007); [EigenLayer](https://docs.eigencloud.xyz/assets/files/EigenLayer_WhitePaper-88c47923ca0319870c611decd6e562ad.pdf) cost of corruption; [Robust](https://arxiv.org/html/2407.21785v1) and [Elastic](https://arxiv.org/abs/2503.00170v3) restaking; [quantitative attack trees](https://arxiv.org/pdf/2212.05358); [ERC-8004](https://eips.ethereum.org/EIPS/eip-8004); bonded claims in [UMA's Optimistic Oracle](https://docs.uma.xyz/protocol-overview/how-does-umas-oracle-work) and [Reality.eth](https://realitio.github.io/docs/html/) | Boolean provenance; trust decided over provenance; min and sum composition of stake; NP-hardness when steps are shared; trust scaled to value at risk; bonds behind individual claims, with challenge windows | Assumption formulas on every ledger claim; a cost of corruption that mixes stake with trusted and untrusted assumptions under each consumer's policy; a sound interval; reservations that bound total reliance; annotations proven in the World Proof | Partly new: a synthesis no system found implements |
| Verifiable Clearing | [Certifying algorithms](https://people.mpi-inf.mpg.de/~mehlhorn/ftp/CertifyingAlgorithms.pdf) (McConnell, Mehlhorn, Näher, Schweitzer); [VIPR](https://arxiv.org/abs/1611.08832) integer-program certificates; [Gnosis Protocol v1](https://github.com/gnosis/dex-contracts/blob/master/contracts/BatchExchange.sol); [CoW Protocol](https://docs.cow.fi/cow-protocol/reference/core/auctions/competition-rules); [convex CFMM routing](https://arxiv.org/pdf/2204.05238); [SPEEDEX](https://arxiv.org/pdf/2111.02719v6) | Optimality certificates for LPs and integer programs; open solver competitions with on-chain feasibility checks; dual variables read as prices | A validity rule: a clearing is accepted only with a certificate of optimality or a certified gap, across LP, flow, convex and combinatorial markets | Partly new: certificates exist, but not as a ledger's acceptance rule |
| Two-sorted state | [CALM](https://arxiv.org/abs/1901.01930); [invariant confluence](https://arxiv.org/abs/1402.2237); [Byzantine eventual consistency](https://arxiv.org/pdf/2012.00472) (Kleppmann and Howard); [Sui Lutris](https://arxiv.org/pdf/2310.18042); [Gallifrey](https://drops.dagstuhl.de/storage/00lipics/lipics-vol136-snapl2019/LIPIcs.SNAPL.2019.11/LIPIcs.SNAPL.2019.11.pdf); [Bounded Counter](https://arxiv.org/pdf/1503.09052) | Consensus only for operations that are not confluent, in Byzantine settings; owned and shared objects in production; linear and replicated types; escrow counters | One ledger type system that derives the coordination class, combined with validity proofs | Synthesis: the least new of the three |
| The Fold and World Proof | Mina's succinct chain; proof-carrying data; incrementally verifiable computation; Nova folding; rollup forced inclusion | Constant-size proofs of a chain; recursion and folding; escape hatches against censorship | One proof pair covering heterogeneous claim batches, assurance metadata, forced inclusion and committee history, built by two independent proof trees | Synthesis |
| Agent Charters | Object capabilities, macaroons and Biscuit; ERC-8004; AP2 mandates; account abstraction | Attenuable delegation; agent registries; signed mandates | Protocol-enforced limits, bonds and mandatory receipts in one object | Synthesis |
| Dual-origin cryptography | Hybrid signatures; robust combiners; multi-proof proposals | Combining schemes so that either one suffices | Applied to every account and every proof, and expressed as ∨ in every trust formula | Synthesis |

**Check before publishing.** The novelty search could not reach four areas, and each could narrow these claims: zero-knowledge proofs of LP or convex optimality; verifiable outsourcing of linear programs through duality (Wang, Ren and Wang, 2011); electricity-market clearing on blockchains; and logical attestation in the Nexus operating system (2011), which attached logical formulas to attestations.

## Applications and North-Star metrics

Six applications lead, each where trust is the binding constraint today and each with a baseline to beat. The network is judged by five North-Star metrics, computed from the World Proof and published live.

### Six applications

| Application | Trust problem today | What changes | Pilot metric |
| --- | --- | --- | --- |
| AI receipts and content marking | EU AI Act Article 50 has required machine-readable marking since 2 Aug 2026; generative systems already on the market must comply by 2 Dec 2026 ([timeline](https://artificialintelligenceact.eu/implementation-timeline/)). The Code of Practice asks for two marking layers ([Jones Day](https://www.jonesday.com/en/insights/2026/06/european-commission-publishes-final-code-of-practice-on-marking-and-labelling-aigenerated-content)) | A receipt per output, anchored in batches: the registry layer beside a watermark | Share of outputs with a verifiable receipt; cost per receipt under 0.0001 USD |
| Agent commerce with bounded risk | Agents may orchestrate up to 1 trillion USD of US retail by 2030 ([McKinsey](https://www.mckinsey.com/industries/retail/our-insights/the-agentic-commerce-opportunity-how-ai-agents-are-ushering-in-a-new-era-for-consumers-and-merchants)), yet agent-payment rails carry little real volume | Charters, receipts and bonds on any payment rail | Agent spend under Charters; losses above Charter limits, target zero |
| Deepfake-proof approvals | US victims reported a record 20.9 billion USD in losses in 2025, 3.05 billion from business email compromise ([FBI IC3](https://www.ic3.gov/AnnualReport/Reports/2025_IC3Report.pdf)); one deepfake video call cost Arup about 25.6 million USD ([CNN](https://edition.cnn.com/2024/05/16/tech/arup-deepfake-scam-loss-hong-kong-intl-hnk)) | A high-value payment needs a mandate claim whose cost of corruption exceeds the amount, not a face on a call | Share of high-value payments gated by claims; impersonation losses year on year |
| Compliance-by-proof settlement | Sending 200 USD abroad costs 6.36% on average; financial-crime compliance costs 206 billion USD a year (Diagnosis) | Private stablecoin settlement with KYC and sanctions proofs | All-in cost in basis points; seconds to settle; compliance cost per transfer |
| Certified-optimal marketplaces | Black-box matching engines with double-digit take rates | Verifiable Clearing, starting with GPU compute | Take rate; published optimality gap |
| Provenance that survives | Platforms strip media credentials; none of eight kept them in one test (Interfaces) | Manifests anchored as claims and found again by content hash; revocations visible | Share of media whose credentials survive a platform round trip |

### North-Star metrics

| Metric | Definition | Direction |
| --- | --- | --- |
| Verified statements per day | Claims and batch statements verifiable against a World Proof | Up: the network's output, as tokens served are AI's |
| Cost per verified statement | Fees divided by verified statements | Down with volume: the positive scaling law |
| Value secured | Value at risk reserved against claims' backing under the decision rule | Up |
| Time to trust | Median time from issue to certificate finality, and to proof finality | Down |
| Trust cost saved | The incumbent cost (fees, fraud losses, compliance spend) minus the network's cost, per pilot | Up: the headline outcome |

### Where 100x is plausible, and where it is not

- **Plausible: cross-border settlement.** From 6.36% on average to a few cents per transfer is two orders of magnitude, provided cash-in and cash-out stop costing more than the transfer itself.
- **Plausible: verifying machine output.** A receipt check takes milliseconds and a fraction of a cent, against human review that takes minutes.
- **Likely smaller: marketplaces.** Certified clearing pushes take rates toward the cost of computation; expect an order of magnitude, not two.
- **Not plausible: where trust is not the bottleneck,** such as domestic card purchases with buyer protection, or where no sensor or party can attest the facts.
- **How we will know.** Trust cost saved, measured per pilot against the incumbent baseline and published every quarter.

## Commercialization: wedge, business model, roadmap

Sell the trust layer to the AI economy before building the network. Start with verifiable receipts and Agent Charters on existing chains, let revenue and the claim graph grow, and build the neo-blockchain itself only once volume justifies it.

### Positioning

- **Category:** neo-blockchain, *the trust layer for the AI economy*.
- **One line:** every AI output, agent payment and data feed gets a receipt with a price on its truth.
- **Buyers:** AI platform teams, enterprises deploying agents, fintechs and payment providers, marketplaces.

### Wedge sequence

| Order | Product | First buyer | Why in this order |
| --- | --- | --- | --- |
| 1 | Receipts: verifiable AI-output and content receipts with trust formulas | AI providers; enterprises deploying AI | Regulatory pull and fraud losses; needs no new chain |
| 2 | Charters: identity, limits and bonded accountability for agents | Companies letting agents spend money | A bounded worst case is a board-level ask; plugs into agent payment protocols |
| 3 | Compliance-by-proof settlement | Fintechs and payment providers | Private stablecoin settlement with provable KYC and sanctions checks |
| 4 | Verifiable Clearing markets | GPU-compute and energy marketplaces | Certified-optimal allocation undercuts black-box take rates |
| 5 | The full neo-blockchain | Everyone above | Moves the proven primitives onto a network built for them |

### Business model

- **Usage fees** per thousand receipts and per verified claim, through a hosted API for teams that will not run nodes.
- **Protocol fees** once the network runs: data, certificates and clearing.
- **Enterprise subnets:** private lanes whose proofs fold into the public World Proof, under an annual license.
- **Assurance marketplace:** a small take on bonds and insurance premiums.
- **Proving cloud:** managed prover capacity, sold to the network and to customers.

### Roadmap

&#91;embedded content: proposed roadmap · 5 phases, 4 gates (criteria from the kill-gate table)\]

The network build starts only after Gate 2, when receipts and Charters already earn revenue on existing chains.

### Team and budget

- **Phases 0 and 1:** about 10 people — two cryptographers (zero knowledge and post-quantum), three distributed-systems engineers, a mechanism designer, a formal-methods engineer, two product and SDK engineers, and a partnerships lead — plus outside counsel.
- **Phases 2 and 3:** 30 to 50 people, adding protocol, security and developer-relations teams.
- **Rough budget:** about 3 to 5 million USD for the first 18 months, and 15 to 25 million USD for the network build including audits. These are planning ranges, not quotes.

### Where incumbents stand

No incumbent combines priced claims, certified clearing and agent accountability, so the plan is to interoperate with each and compete on those three.

| Player | What it does well | What it lacks against this design |
| --- | --- | --- |
| Ethereum | The largest settlement layer; optional execution proofs proposed for a coming fork ([EIP-8025](https://zkevm.ethereum.foundation/blog/eip-8025-optional-execution-proofs-hegota)); a post-quantum roadmap that could finish by 2029 ([Ethereum](https://pq.ethereum.org/)) | Totally orders everything; no claims layer, assurance algebra or certified clearing; post-quantum migration is years away |
| Sui, Aptos, Solana | Fast consensus in production: about 400 ms on Sui's Mysticeti; Solana's Alpenglow, live since October 2026, targets about 150 ms finality | Replicated execution; no claims layer; classical signatures |
| Mina | A constant-size proof of the whole chain | Low throughput; elliptic-curve proofs, not post-quantum; no claims or clearing |
| Stripe's Tempo, Circle's Arc | Payment-grade stablecoin settlement with built-in distribution | A narrow product: trust stops at the payment |
| CoW Protocol, Penumbra, Anoma | Batch auctions and solver competition for intents | No optimality certificate as a condition of validity |
| EigenLayer | Shared security with cost-of-corruption accounting | Stake only; no per-claim assumption formulas |
| Chainlink, EAS, ERC-8004 | Oracles, attestations and agent registries | No computed cost of corruption, so reputation stays cheap to fake |
| C2PA, x402, AP2, card-network agent protocols | Provenance signatures and agent payment rails | No shared record with bonds, so their signatures carry no price on being wrong |

### Naming

In search results today, *neo-blockchain* already means the NEO chain, which is still live: node releases continued through July 2026 and its Neo X sidechain runs on mainnet ([Neo](https://neo.org/news)), though its market value is small, about 147 million USD ([CoinGecko](https://www.coingecko.com/en/coins/neo)). Keep *neo-blockchain* as a descriptive category, pair it with a distinct brand for the network and its products, and clear that brand with USPTO, EUIPO and WIPO searches before launch.

## Risks, open problems and kill criteria

The largest risks are commercial and engineering, not cryptographic: whether customers pay for verified claims before the network exists, and whether proving keeps getting cheaper. Each risk has a signal to watch; each phase has a gate that must clear, or the plan changes.

### Risks

| Risk | Signal to watch | Mitigation |
| --- | --- | --- |
| Nobody pays for receipts | Paying design partners; renewal after pilots | Start where regulation and fraud losses create pull; price per thousand receipts, not per token |
| Proving stays too slow or costly | Cost per leaf proof; root latency on testnet | Specialized circuits; longer Pulses at launch; hardware acceleration |
| A soundness or circuit bug | Audit findings; bounty reports | Two end-to-end proof trees; formal verification; value caps until audited; time-locked upgrades |
| Post-quantum bandwidth: 8 to 10 KB of sender signatures per transaction, plus validator votes | Validator ingress cost per operation | Batch-root voting; signature aggregation inside proofs; sharded validator ingress |
| Incumbents absorb the demand | Adoption of card-network agent protocols, payment chains, L1 zkEVMs | Interoperate with their protocols; compete on assurance and clearing, which they lack |
| Regulation | Token classification; erasure rights over personal data | Tokenless early phases; personal data only as commitments, payloads in deletable storage |
| Too many novel parts at once | Schedule slip per phase | Ship each primitive on existing chains before building the network |
| Lies that are hard to disprove | Share of claims with detection discounts | Lead with claims backed by cryptography, sensors or several independent parties |

### Kill and pivot gates

| Gate | Proposed criteria | If missed |
| --- | --- | --- |
| Gate 1: end of Phase 0 (month 6) | 3 signed design partners; Assurance Algebra specified and reviewed by two outside cryptographers | License the algebra and SDK to existing chains instead of building a network |
| Gate 2: end of Phase 1 (month 18) | 100 million receipts a month; 10 paying customers; under 0.0001 USD per receipt | Stay an application-layer protocol on existing chains |
| Gate 3: end of Phase 2 (month 36) | Testnet sustains 100,000 operations per second with a World Proof at least every 30 seconds; two independent audits with no open critical findings | Delay mainnet; launch with periodic proofs and lower caps |
| Gate 4: end of Phase 3 (month 48) | First cleared market reaches steady volume with a published optimality gap under 0.1% | Keep Clearing to exact-certificate markets only |

### Open research problems

- Aggregating post-quantum signatures, lattice and hash-based, efficiently inside proofs.
- Zero-knowledge proofs of inference for frontier-scale models; until then the practical tier is TEE attestation plus bonds.
- Exact cost of corruption when atoms are shared (NP-hard in general): good approximations, and schemas that enforce disjointness.
- Optimality certificates for combinatorial markets beyond gap bounds, and the incentive properties of batch clearing.
- Pricing correlated atom failures for insurance.
- Fair ordering for contended operations that cannot be batched, and post-quantum threshold encryption for sealed batches.
- Personhood without dependence on a single issuer.
- End-to-end formal verification of the World Proof circuit.

## Appendix: claim schema, reference code, glossary

### A. Claim schema (receipt for an AI inference)

Signatures are verified by validators and proven inside the Fold, so the data layer stores the fields below and never the signatures.

```json
{
  "claim_id": "hash of the canonical encoding",
  "schema": "receipt.ai.inference/v1",
  "statement": {
    "model": "sha3-256 of the model weights or provider model ID",
    "input_commitment": "hiding commitment to prompt and context",
    "output_hash": "sha3-256 of the output"
  },
  "evidence": [
    { "kind": "tee_quote", "vendor": "GPU vendor", "quote": "..." },
    { "kind": "operator_bond", "bond_ref": "lane object ID", "amount_usd": 50000 }
  ],
  "trust_formula": "OR(atom:tee.resists_host, atom:bond.operator)",
  "issuer": "account ID",
  "charter": "charter ID, if issued by an agent",
  "dependencies": [],
  "adjudicator": "claim ID of the arbitration service and its bond",
  "window": { "valid_until": "2027-01-31", "challenge_days": 14 },
  "detection_factor": 0.5
}
```

### B. Cost of corruption under a policy

Formulas are DAGs keyed by claim ID, so evaluation is memoized and linear in the formula's size.

```python
INF = float("inf")

def coc(node, policy, bonds, memo):
    """Return (low, high) cost of corruption in USD, memoized by node ID."""
    if node.id in memo:
        return memo[node.id]
    if node.is_atom:
        a = node.atom
        if a in policy.trusted:
            v = INF
        elif a in bonds:              # economic atoms; bonded institutions
            v = bonds[a]              # locked for window + challenge period
        else:
            v = a.attack_cost         # cheapest known attack; 0 if unknown
        result = (v, v)
    else:
        parts = [coc(ch, policy, bonds, memo) for ch in node.children]
        if node.op == "AND":          # breaking any input breaks the claim
            result = (min(p[0] for p in parts), min(p[1] for p in parts))
        else:                         # OR: every alternative must be broken
            low = sum(p[0] for p in parts) if node.disjoint else max(p[0] for p in parts)
            result = (low, sum(p[1] for p in parts))
    memo[node.id] = result
    return result

def accept(claim, policy, value_at_risk, kappa=2.0):
    low, _ = coc(claim.formula, policy, claim.bonds, memo={})
    effective = min(low * claim.detection_factor, claim.adjudicator_coc)
    if effective < kappa * value_at_risk:
        return False
    return claim.reserve(kappa * value_at_risk)  # refused if the backing is already reserved
```

### C. Verifying a clearing certificate

The cost is linear in the number of nonzero constraint coefficients. All arithmetic is exact (fixed-point or rational) so that no two verifiers can disagree.

```python
def verify_lp_certificate(A, b, c, x, y, eps):
    if any(v < 0 for v in x) or any(v < 0 for v in y):
        return False
    if any(A.row(i).dot(x) > b[i] for i in range(A.m)):   # primal feasible
        return False
    if any(A.col(j).dot(y) < c[j] for j in range(A.n)):   # dual feasible
        return False
    return c.dot(x) >= b.dot(y) - eps                     # duality gap within eps
```

### D. Glossary

| Term | Meaning |
| --- | --- |
| Atom | One irreducible trust assumption, such as a hash's collision resistance or an attester's honesty |
| Charter | The protocol-enforced contract between a principal and an AI agent |
| Claim | A statement with evidence, issuer, trust formula, bonds and a window |
| Contention domain | A set of shared objects that must be ordered together |
| Cost of corruption | The minimum value an attacker must forfeit to falsify a claim, under a given policy |
| Dual-origin | Two independent cryptographic families or implementations, both required |
| Fold | The continuous aggregation of all proofs into the World Proof |
| Lane | An owned object's version history, extended one quorum certificate at a time |
| Pulse | The cadence at which the network emits a World Proof |
| Trust formula (φ) | A monotone Boolean formula over atoms |
| World Proof | A constant-size pair of proofs, one per stack, of the whole history up to the current state root |

## Sources

Pages opened during research on 27 October 2026. Performance figures are the vendors' own unless the text says otherwise.

- **AI and crypto adoption:** [OpenAI, 1 billion weekly users](https://openai.com/index/expanding-access-to-ai-with-chatgpt-ads/) · [OpenAI, 700 million weekly users](https://openai.com/index/buy-it-in-chatgpt/) · [McKinsey, State of AI 2026](https://www.mckinsey.com/capabilities/quantumblack/our-insights/the-state-of-ai) · [a16z, State of Crypto 2025](https://www.a16z.news/p/state-of-crypto-2025) · [Monad docs](https://docs.monad.xyz/introduction/why-monad) · [Monad, September 2026](https://monad.xyz/blog/monad-ecosystem-highlights-september-2026)
- **Stablecoins and payment chains:** [DefiLlama](https://defillama.com/stablecoins) · [McKinsey on stablecoin payments](https://www.mckinsey.com/industries/financial-services/our-insights/stablecoins-in-payments-what-the-raw-transaction-numbers-miss) · [Visa and Allium via Spark](https://www.spark.money/research/stablecoin-transfer-volume-eleven-trillion) · [Tempo](https://tempo.xyz/faq) · [Arc](https://www.arc.io/blog/arc-mainnet-goes-live-on-september-16-2026) · [OCC on the GENIUS Act](https://www.occ.gov/news-issuances/news-releases/2026/nr-occ-2026-92.html) · [US Senate roll call](https://www.senate.gov/legislative/LIS/roll_call_votes/vote1192/vote_119_2_00234.htm)
- **The cost of trust:** [Wallis and North, NBER](https://www.nber.org/system/files/chapters/c9679/c9679.pdf) · [LexisNexis](https://risk.lexisnexis.com/about-us/press-room/press-release/20230926-global-financial-crime-compliance-costs) · [World Bank Remittance Prices Worldwide](https://remittanceprices.worldbank.org/sites/default/files/2026-04/RPW_main_report_and_annex_Q325.pdf) · [Merchants Payments Coalition](https://merchantspaymentscoalition.com/credit-and-debit-card-swipe-fees-reach-record-19825-billion-president-and-congress-call-action) · [GASA](https://www.gasa.org/post/global-scams-on-the-rise-over-half-of-adults-worldwide-report-scam-encounters) · [OECD and EUIPO](https://www.oecd.org/en/about/news/press-releases/2025/05/global-trade-in-fake-goods-reached-USD-467-billion-posing-risks-to-consumer-safety-and-compromising-intellectual-property.html) · [FBI IC3 2025](https://www.ic3.gov/AnnualReport/Reports/2025_IC3Report.pdf) · [CNN on Arup](https://edition.cnn.com/2024/05/16/tech/arup-deepfake-scam-loss-hong-kong-intl-hnk)
- **AI regulation and agent commerce:** [EU AI Act timeline](https://artificialintelligenceact.eu/implementation-timeline/) · [Regulation (EU) 2026/1744](https://eur-lex.europa.eu/legal-content/EN/TXT/HTML/?uri=OJ%3AL_202601744) · [Jones Day on the marking Code of Practice](https://www.jonesday.com/en/insights/2026/06/european-commission-publishes-final-code-of-practice-on-marking-and-labelling-aigenerated-content) · [McKinsey on agentic commerce](https://www.mckinsey.com/industries/retail/our-insights/the-agentic-commerce-opportunity-how-ai-agents-are-ushering-in-a-new-era-for-consumers-and-merchants) · [Gartner](https://www.gartner.com/en/articles/strategic-predictions-for-2026)
- **Agent standards and provenance:** [MCP and the Agentic AI Foundation](https://blog.modelcontextprotocol.io/posts/2025-12-09-mcp-joins-agentic-ai-foundation/) · [x402 Foundation](https://www.linuxfoundation.org/press/linux-foundation-is-launching-the-x402-foundation-and-welcoming-the-contribution-of-the-x402-protocol) · [CoinDesk on x402 members](https://www.coindesk.com/tech/2026/07/15/visa-mastercard-and-ripple-join-the-standard-letting-ai-agents-pay-in-stablecoins) · [x402 usage analysis](https://www.danielmcglynn.com/the-x402-counter-has-shown-the-same-four-numbers-since-march/) · [AP2 at FIDO](https://blog.google/products-and-platforms/platforms/google-pay/agent-payments-protocol-fido-alliance/) · [ERC-8004](https://eips.ethereum.org/EIPS/eip-8004) · [Datawallet on ERC-8004](https://www.datawallet.com/crypto/erc-8004-explained) · [C2PA 2.5](https://spec.c2pa.org/specifications/specifications/2.5/specs/C2PA_Specification.html) · [Platform credential test](https://fredritchin.substack.com/p/major-platforms-are-not-labeling)
- **Proofs and post-quantum cryptography:** [NIST PQC standards](https://csrc.nist.gov/Projects/post-quantum-cryptography/pqc-standards) · [FIPS 203](https://csrc.nist.gov/pubs/fips/203/final) · [FIPS 204](https://csrc.nist.gov/pubs/fips/204/final) · [FIPS 205](https://csrc.nist.gov/pubs/fips/205/final) · [NIST forum on FIPS 206](https://groups.google.com/a/list.nist.gov/g/pqc-forum/c/7z-y5qp-W2Y) · [RFC 8391, XMSS and XMSS^MT](https://www.rfc-editor.org/rfc/rfc8391) · [leanMultisig](https://github.com/leanEthereum/leanMultisig) · [Ethereum Foundation, real-time proving](https://blog.ethereum.org/2025/07/10/realtime-proving) · [Ethproofs](https://ethproofs.org/) · [SP1 Hypercube](https://blog.succinct.xyz/sp1-hypercube/) · [Succinct, 16 GPUs](https://blog.succinct.xyz/real-time-proving-16-gpus/) · [Succinct 2025 recap](https://blog.succinct.xyz/succinct-2025-recap/) · [OpenVM 2.1](https://www.axiom.xyz/blog/2-1) · [Airbender Prime](https://www.kucoin.com/news/flash/zksync-launches-airbender-prime-achieves-ethereum-real-time-proofs-on-single-gpu) · [Wavect](https://wavect.io/blog/zero-knowledge-proofs-production-2026/) · [soundcalc](https://github.com/ethereum/soundcalc/blob/main/reports/summary.md) · [WHIR](https://eprint.iacr.org/2024/1586) · [LatticeFold+](https://eprint.iacr.org/2025/247) · [Neo/SuperNeo](https://eprint.iacr.org/2026/242) · [LatticeFold code](https://github.com/NethermindEth/latticefold) · [stwo-cairo bug](https://www.axiom.xyz/blog/stwo-bug) · [EIP-8025](https://zkevm.ethereum.foundation/blog/eip-8025-optional-execution-proofs-hegota) · [Ethereum post-quantum roadmap](https://pq.ethereum.org/)
- **Verifiable AI and TEEs:** [Jolt Atlas](https://arxiv.org/pdf/2602.17452) · [DeepProve](https://eprint.iacr.org/2026/1112) · [H100 confidential computing study](https://arxiv.org/html/2409.03992v4) · [Battering RAM](https://batteringram.eu/) · [WireTap](https://wiretap.fail/) · [TEE.fail](https://tee.fail/) · [DDRop](https://thehackernews.com/2026/09/new-ddrop-attack-breaks-intel-tdx-and.html)
- **Consensus, ordering and data availability:** [FastPay](https://arxiv.org/abs/2003.11506) · [Guerraoui et al.](https://arxiv.org/abs/1906.05574) · [Mysticeti paper](https://arxiv.org/abs/2310.14821) · [Mysticeti on Sui](https://decentralizedthoughts.github.io/2026-03-06-mysticeti-revolutionizing-consensus-on-sui/) · [Sui transaction lifecycle](https://docs.sui.io/concepts/transactions/transaction-lifecycle) · [Solana Alpenglow](https://solana.com/upgrades/alpenglow) · [Aptos encrypted mempool](https://aptosnetwork.com/currents/encrypted-mempool-mainnet) · [EIP-4844](https://eips.ethereum.org/EIPS/eip-4844) · [Fusaka](https://blog.ethereum.org/2025/11/06/fusaka-mainnet-announcement) · [Celestia blog](https://blog.celestia.org/rss/) · [Celestia on Fibre capacity](https://blog.celestia.org/derive-scaling-the-largest-onchain-options-exchange-with-celestia/) · [Fibre activation](https://thedefiant.io/news/infrastructure/celestia-activates-fibre-opening-direct-validator-uploads-for-rollups) · [Al-Bassam et al.](https://arxiv.org/abs/1809.09044) · [ZODA](https://eprint.iacr.org/2025/034)
- **Theory and prior art:** [CALM](https://arxiv.org/abs/1901.01930) · [Invariant confluence](https://arxiv.org/abs/1402.2237) · [Byzantine eventual consistency](https://arxiv.org/pdf/2012.00472) · [Linear logic](<https://doi.org/10.1016/0304-3975(87)90045-4>) · [Provenance semirings](https://doi.org/10.1145/1265530.1265535) · [Provenance semirings talk](https://www.cis.upenn.edu/~val/15MayPODS.pdf) · [ORCHESTRA](https://www.cs.ucdavis.edu/~green/papers/vldb07.pdf) · [EigenLayer](https://docs.eigencloud.xyz/assets/files/EigenLayer_WhitePaper-88c47923ca0319870c611decd6e562ad.pdf) · [Robust restaking](https://arxiv.org/html/2407.21785v1) · [Elastic restaking](https://arxiv.org/abs/2503.00170v3) · [Quantitative attack trees](https://arxiv.org/pdf/2212.05358) · [UMA Optimistic Oracle](https://docs.uma.xyz/protocol-overview/how-does-umas-oracle-work) · [Reality.eth](https://realitio.github.io/docs/html/) · [Certifying algorithms](https://people.mpi-inf.mpg.de/~mehlhorn/ftp/CertifyingAlgorithms.pdf) · [VIPR](https://arxiv.org/abs/1611.08832) · [Gnosis Protocol v1](https://github.com/gnosis/dex-contracts/blob/master/contracts/BatchExchange.sol) · [CoW Protocol rules](https://docs.cow.fi/cow-protocol/reference/core/auctions/competition-rules) · [CFMM routing](https://arxiv.org/pdf/2204.05238) · [SPEEDEX](https://arxiv.org/pdf/2111.02719v6) · [Sui Lutris](https://arxiv.org/pdf/2310.18042) · [Gallifrey](https://drops.dagstuhl.de/storage/00lipics/lipics-vol136-snapl2019/LIPIcs.SNAPL.2019.11/LIPIcs.SNAPL.2019.11.pdf) · [Bounded Counter](https://arxiv.org/pdf/1503.09052)
- **Naming:** [Neo news](https://neo.org/news) · [CoinGecko on NEO](https://www.coingecko.com/en/coins/neo)
