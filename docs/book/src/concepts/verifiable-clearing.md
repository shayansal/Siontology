# Verifiable Clearing

Contended markets clear in sealed batches, and the winning solver must
submit, with its allocation, a **certificate that no better allocation
exists**. Checking the certificate takes linear time however hard the
problem was to solve. Anyone can run a solver, and nobody can quietly
extract value by controlling the order of transactions.

## How a batch clears

1. **Orders arrive** during a batch window. An order states a side, a
   quantity and a limit price, such as *8 GPU-hours at no more than
   2.10 USD per hour*. Orders may be sealed (encrypted).
2. **The batch seals.** Its contents are fixed before anyone can read
   sealed orders; then the committee reveals them.
3. **Solvers compute** an allocation *x* and a price vector *y*.
4. **The ledger checks the certificate.**
5. **Everyone in the batch trades at the same price.** A position inside
   the batch is worth nothing, so there is nothing to front-run.

## The certificate

When clearing is a linear program, the allocation solves the primal and the
prices solve the dual:

```text
maximize  cᵀx  subject to  Ax ≤ b, x ≥ 0          minimize  bᵀy  subject to  Aᵀy ≥ c, y ≥ 0
```

The ledger accepts the pair (x\*, y\*) when all five checks pass:

```text
Ax* ≤ b,    x* ≥ 0,    Aᵀy* ≥ c,    y* ≥ 0,    cᵀx* ≥ bᵀy* − ε
```

By weak duality no feasible allocation scores above bᵀy\*, so x\* is within
ε of optimal; with ε = 0 it is exactly optimal. The duals y\* are the
clearing prices, the role locational marginal prices play in electricity
markets.

`sion-lp` does this in **exact rational arithmetic**: values are reduced
rationals with one canonical encoding, there is no floating point anywhere
in the verification path, and no two verifiers can disagree. Its simplex
solver is a reference, not part of the trust base: whatever any solver
returns is accepted only if the certificate verifies. Certificate values
longer than 512 bits are refused.

## The double auction

The markets in this implementation are single-good double auctions priced
in a numeraire (for example GPU-hours in USD). The program has one variable
per order:

```text
maximize   Σ_buy c'_k x_k + Σ_sell c'_k x_k
subject to Σ_buy x_k − Σ_sell x_k ≤ 0       nobody buys what was not sold
           x_k ≤ quantity_k
           x ≥ 0
```

with `c'_k` the order's limit price (negative for sellers) plus a tiny
perturbation. The constraint matrix is totally unimodular, so with integer
quantities the certified optimum is integral.

- **Canonical tie-breaking.** Each order's objective coefficient gets a
  perturbation derived from a hash of the beacon revealed when the batch
  sealed and the order's identifier. It is too small to overturn a real
  improvement, but it makes the optimum unique, so a solver cannot choose
  which of two equal bidders wins. The certificate covers the perturbed
  program; welfare and prices are reported at the original limits.
- **Canonical price.** The certified allocation is supported by a band of
  prices. The settlement price is the band's midpoint, rounded down.

## The lifecycle on the ledger

| Step | Transaction or event | Class |
|---|---|---|
| Open a market | `CreateMarket { label, good, numeraire, batch_length, clear_deadline, committee, threshold }` | Owned linear |
| Submit an order | `SubmitOrder { market, submission }`; the escrow is drawn now: limit × quantity of the numeraire for a buy, quantity units of the good for a sell | Owned linear; the order joins its batch at the end of the Pulse |
| Seal | Scheduled `SealBatch` at the end of the batch window; event `BatchSealed` | Deferred phase |
| Reveal sealed orders | `RevealBatch { market, batch, shares }` with the committee's key shares | Contended |
| Clear | `ClearBatch { market, batch, result }`: the first clearing whose certificate verifies settles the batch | Contended |
| Refund | Scheduled `BatchDeadline` `clear_deadline` Pulses after sealing: an uncleared batch refunds every escrow; event `BatchRefunded` | Deferred phase |

Batch *i* collects the orders submitted in Pulses `i · batch_length` to
`(i + 1) · batch_length − 1`. A batch holds at most 1,024 orders; batch
lengths and deadlines are at most one million Pulses.

Settlement: a filled buy receives its units of the good and gets back its
escrow minus price × units; a filled sell receives price × units of the
numeraire and gets back its unsold units; unfilled orders get their whole
escrow back.

## On the devnet

From `sion demo market`:

```text
Pulse  61  agent-1 bids for 5 GPU-hours at ≤ 2.10 USD: Success
Pulse  62  agent-2 bids for 3 GPU-hours at ≤ 1.90 USD: Success
Pulse  62  lab-a offers 5 GPU-hours at ≤ 1.80 USD: Success
Pulse  62  lab-b offers 6 GPU-hours at ≤ 2.00 USD: Success
           batch 5 sealed with the Pulse beacon 18f40c60 — 4 orders, nobody can reorder them now
           solver computes the welfare-maximizing allocation: 5 GPU-hours at one price 1.95 USD (supporting band 1.9–2), welfare 1.5 USD, with an LP optimality certificate
Pulse  73  the ledger checks the certificate in linear time and settles everyone at that price
```

(The demo prints `≤` for every order; for a sell order the limit is a
floor.) The guide [Markets and clearing](../guides/markets.md) runs the same
market from the command line.

## What it covers, in the architecture

| Problem class | Markets it fits | Certificate strength |
|---|---|---|
| Linear programs | Bids priced in one numeraire; DC optimal power flow | Exact |
| Flow, assignment, transportation | Ride, job and compute-slot matching | Exact and integral |
| Convex programs | Asset-to-asset exchange; routing across market makers | ε-exact |
| Integer and combinatorial | Package bids, indivisible bundles | A certified gap against a bound |

## Limits, stated plainly

- **Optimal for what was reported.** The certificate proves optimality for
  the orders as submitted. Bidders can still misreport, and uniform-price
  batch auctions are not fully strategy-proof.
- **Objectives must be public.** Each market publishes its objective and
  constraint templates.
- **Latency costs one batch.** A market trades only when its batch seals
  and clears.

## In this implementation

- **Built**: exact rational LP with linear-time certificate checks, sealed
  double auctions with beacon tie-breaking and a canonical price, sealed
  orders (ML-KEM with Shamir t-of-n sharing to the market committee),
  deadlines and refunds. Assignment markets (jobs to slots, with
  competitive-equilibrium slot prices) exist in `sion-clearing` as a
  library.
- **Not built**: assignment, convex and combinatorial markets on the
  ledger; a solver competition (the first valid clearing wins, which is the
  optimum anyway, because the perturbed optimum is unique); threshold
  decryption that is both post-quantum and efficient for large committees
  (the Shamir construction grows by about 1.1 KB per member); CLI commands
  for sealed orders and reveals.
