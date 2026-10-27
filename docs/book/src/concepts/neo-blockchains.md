# Neo-blockchains

A **neo-blockchain** is a network whose product is verified knowledge, not
ordered transactions. Blockchains chain blocks; DAGs link transactions;
neo-blockchains **fold claims**.

The word is a category, not a brand. Siontology is one neo-blockchain.

## Eight inversions

The architecture paper describes the design as eight assumptions of
blockchains, each inverted.

| Blockchains assume | Neo-blockchains invert it | Mechanism |
|---|---|---|
| Verify by re-executing: every node repeats the work | Verify by checking a proof, at near-constant cost | Proof-carrying state; the Fold |
| Totally order every operation | Order only operations that conflict | Two-sorted state; Linear Lanes |
| The ledger knows only its own state | The ledger records claims about the world, with their assumptions priced | Claims; the Assurance Algebra |
| Transparent by default | Private by default, provable on demand | Compliance-by-proof |
| A human signs each action | Agents act inside protocol-enforced limits | Agent Charters |
| Market outcomes depend on who orders transactions | Market outcomes carry a certificate that no better allocation exists | Verifiable Clearing |
| Fees rise when demand meets fixed capacity | Cost per verified claim falls as volume grows | Batching, folding, sampling |
| One cryptographic assumption guards each function | Unrelated families guard authorization; independent implementations guard validity | Dual-origin signatures and proofs |

## One backbone, three primitives

The **Fold** aggregates every proof in the network into one constant-size
**World Proof** per Pulse, which a phone can verify. Three primitives sit on
it:

- **Two-sorted state.** Knowledge needs no coordination; only resource
  consumption is ordered, and only among the parties contending for it.
- **The Assurance Algebra.** Every claim carries a formula of the
  assumptions it rests on and a computed cost of corruption, so trust
  becomes a number an agent can act on.
- **Verifiable Clearing.** Markets clear in batches and publish a
  certificate that no better allocation exists, so ordering power cannot
  extract value.

## The lifecycle of a claim

1. **Issue.** A person, device, program or agent creates a claim: a
   statement over committed data, its evidence and its dependencies, signed
   under its account or Charter.
2. **Check evidence.** Proofs are verified; attestations are checked and
   their atoms written into the claim's trust formula.
3. **Publish.** The payload, or its commitment, lands on the
   data-availability layer.
4. **Order only if needed.** A claim that spends or locks a resource (a fee,
   a payment, a bond) takes a lane certificate, or a consensus slot if the
   resource is contended. Pure knowledge skips this step.
5. **Join the knowledge graph.** The claim and its formula enter the
   grow-only knowledge log.
6. **Fold.** The next World Proof covers the claim, its formula and its
   bonds.
7. **Use.** A consumer verifies the World Proof and one Merkle path,
   evaluates the claim's cost of corruption under its own policy, and acts
   or declines.
8. **Challenge.** Until the window closes, anyone can post counter-evidence.
   A proven falsehood slashes the bonds behind it and compensates those who
   relied on it.

In this implementation, issuing, publishing, ordering, the knowledge log,
folding, using and challenging all exist. Four steps are simpler than the
design:

- The ledger does not check a claim's evidence (step 2). The issuer writes
  the formula; the ledger checks only that its atoms are registered and that
  any bonded atom in it is backed by the issuer's own bond.
- Every transaction pays its fee from a gas coin, so every claim takes a
  lane certificate (step 4 is never skipped).
- The World Proof of step 6 is a replay proof (see
  [The Fold and World Proofs](the-fold.md)).
- Challenges (step 8) are settled by the adjudicator the claim names.
  Automatic slashing for cryptographically checkable contradictions is not
  built.

## What is new

No single primitive is new. The architecture paper's prior-art search found
the combination new, and two ledger rules that no system it found enforces:

- a **policy-relative cost of corruption** computed from each claim's
  assumption formula; and
- **certified optimality** as the condition for accepting a market
  clearing.

The paper's closest prior work, per component:

| Component | Closest prior work | Verdict in the paper |
|---|---|---|
| Assurance Algebra | Provenance semirings; EigenLayer's cost of corruption; quantitative attack trees; bonded claims in UMA and Reality.eth | Partly new |
| Verifiable Clearing | Certifying algorithms; VIPR certificates; CoW Protocol and Gnosis batch auctions; SPEEDEX | Partly new |
| Two-sorted state | CALM; invariant confluence; Sui Lutris owned objects; escrow counters | Synthesis |
| The Fold and World Proof | Mina; proof-carrying data; incrementally verifiable computation | Synthesis |
| Agent Charters | Object capabilities; macaroons and Biscuit; ERC-8004; AP2 mandates | Synthesis |
| Dual-origin cryptography | Hybrid signatures; robust combiners | Synthesis |

## Where it should help, and where it should not

From the architecture paper:

- **Plausible large gains**: cross-border settlement, verifying machine
  output, agent commerce: places where trust is the binding constraint.
- **Smaller gains**: marketplaces, where certified clearing pushes take
  rates toward the cost of computation.
- **No gain**: where trust is not the bottleneck, such as domestic card
  purchases with buyer protection, or where no sensor or party can attest
  the facts.

The paper measures success as *trust cost saved* per pilot, against the
incumbent's cost. None of that has been measured yet.
