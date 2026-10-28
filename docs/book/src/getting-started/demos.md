# Run the demos

Eleven scripted demos drive real transactions through the whole protocol
(lane certificates, consensus, Pulse execution, header certificates, data
availability and proving) and narrate what the ledger established.

```bash
sion demo all --local
```

`--local` starts an in-process devnet with the default configuration (4
validators, 2 shards, dual post-quantum signatures, proving on) and stops it
at the end. The staking demo needs epochs that end, so it gets a devnet of
its own with a standby validator and 24-Pulse epochs: `sion demo all
--local` runs the first ten demos on one default devnet, then starts a
second devnet for the staking demo.

Without `--local`, a demo runs against the node at `--node` (default
`http://127.0.0.1:8730`), for example a devnet you started with
`sion devnet`. On a devnet without a standby validator, or with one epoch
that never ends, the staking demo only prints how to start one it can use.

On the 2-core machine used for this book, the censorship demo took 159
seconds on its own, the staking demo 67 seconds, and the light demo 25
seconds against a running devnet.

| Demo | What it shows |
|---|---|
| `invoice` | Trade finance on insured knowledge: a bonded invoice claim, priced with the Assurance Algebra, relied on within its backing, and repaid from slashed bonds when it proves false |
| `agents` | An AI agent with a budget the ledger enforces: an unlisted payee and an over-limit payment are refused; revoking the charter stops the agent and its sub-agent |
| `swarm` | Agent identities: an orchestrator launches 12 researchers in one transaction and one of them launches 3 of its own, each with its own identity, account and budget; 15 pay at once without consensus; one suspension stops the whole tree and one resumption restarts it |
| `tasks` | Task Charters: an eval orchestrator posts work from its budget; a result is paid only when it rests on the environment's claim and enough attested backing, and only once; a bounty is settled and another withdrawn; an exploit in the grader is proven and the lab is compensated from the vendor's bond |
| `market` | A GPU-hour market that clears at one price with an optimality certificate |
| `receipts` | An AI inference receipt priced under three policies: trusting a TEE, distrusting it, or assuming it broken |
| `x402` | An AI agent pays an HTTP API per request: `402`, payment under its charter, `200`; a replayed payment and an over-limit payment are refused |
| `streams` | An agent pays an API 2,000 times from one payment stream: one hash per call at the server, a few settlements on the ledger, a receipt audited by its checkpoint |
| `censorship` | Forced inclusion: a censored user gets through by posting straight to consensus; when more than a third of a committee censors too, certified headers count the overdue intent |
| `light` | A light client pins the chain identity, verifies the latest Pulse end to end and catches a node that serves substituted validator keys |
| `staking` | A standby validator stakes and joins at the next epoch boundary, a light client follows the committee change, rewards are claimed, and the newcomer is slashed for equivocating and loses its seat |

Run one with `sion demo <name> --local`, except `light`, which needs a
devnet that has already proven a Pulse ([light](#light) below).

What follows is real output: the first five demos from one run of
`sion demo all --local`, and the swarm, tasks, streams, censorship, light and
staking demos from runs of their own on later versions, so chain identities
differ between them. Account names such as `supplier` and `judge` are deterministic devnet
wallets: anyone who knows the name can derive the keys.

## invoice

```text
== Invoice #1042: trade finance on insured knowledge ==
Pulse   6  supplier bonds 30,000 USD behind "invoices"
Pulse   8  shipper bonds 20,000 USD behind "logistics"
Pulse  10  judge bonds 100,000 USD behind "arbitration"
Pulse  12  shipper attests "Shipment for invoice #1042 delivered"  [dual-signature ∧ bond:shipper]
Pulse  15  supplier issues "Invoice #1042: 10,000 USD payable"  [dual-signature ∧ (bond:supplier ∨ delivery)]
           cost of corruption (standard policy): 50,000 USD, exact
           = 30,000 (supplier's bond) + 20,000 (shipper's bond): a forger must corrupt both routes
           rely on  10000 USD at κ = 2 → ACCEPT: cost of corruption 50,000 covers κ·V = 20,000
           rely on  30000 USD at κ = 2 → REJECT: cost of corruption 50,000 (after detection discount 1000/1000) is below κ·V = 60,000
Pulse  18  financier funds the invoice and reserves 20,000 USD of its backing (κ·V): nobody else can rely on the same bonds beyond what slashing can repay
Pulse  20  buyer challenges the supplier's attestation (deposit 1,000 USD)
Pulse  22  judge upholds: supplier's bond slashed (30,000 USD); the invoice still holds through the delivery route
Pulse  24  buyer challenges the shipper's delivery attestation, under the shipper's own adjudicator
Pulse  26  judge upholds: 2 claims falsified (the delivery, and the invoice resting on it)
           financier compensated 10,000 USD automatically from the slashed bonds (balance 199,999.9988 → 209,999.9988)
Result: reliance on a false invoice was repaid in full, in Pulses, with no lawsuit and no adjuster.
```

The invoice rests on two independent routes, so its cost of corruption is
the sum of the two bonds. Slashing the first route leaves the invoice
standing; slashing the second falsifies the delivery claim and, through it,
the invoice. See [The Assurance Algebra](../concepts/assurance/index.md).

## agents

```text
== Agent Charters: an AI agent with a budget the ledger enforces ==
Pulse  32  acme charters its agent: pay only gpu-cloud, ≤ 100 USD per payment, ≤ 250 USD per window, 1,000 USD budget, receipts on, 500 USD bond behind the agent's receipts
Pulse  34  agent pays gpu-cloud 80 USD for inference: Success; receipt claim 2c4e5264 joins the knowledge log
Pulse  36  a prompt-injected agent tries to pay mallory 90 USD → refused by the ledger: charter caveat violated: payee not allowed
Pulse  38  the agent tries a 150 USD payment → refused by the ledger: charter caveat violated: payment 150 above per-payment limit 100
Pulse  41  agent delegates 100 USD to a sub-agent under narrower caveats (≤ 20 USD per payment)
Pulse  43  sub-agent pays gpu-cloud 20 USD: Success
Pulse  45  acme revokes the charter (ordered by consensus)
Pulse  47  acme-agent tries to pay again → charter caveat violated: charter revoked
Pulse  49  sub-agent tries to pay again → charter caveat violated: charter revoked
           the agent's receipt is a claim backed by acme's bond: cost to forge it 500 USD
Result: gpu-cloud received 100 USD; mallory received nothing; the limits lived in the ledger, not the agent.
```

See [Agent Charters](../concepts/agent-charters.md).

## swarm

```text
== Agent identities: a swarm of subagents, each with its own identity ==
Pulse   6  acme registers its orchestrator: agent 90d88d8b, card 94efec04, 1,000 USD budget, may launch subagents
Pulse   8  the orchestrator launches 12 researchers in one transaction: 12 identities, 12 accounts, 40 USD each, ≤ 10 USD per payment
Pulse  10  researcher-1 launches 3 sub-researchers of its own (depth 2), 10 USD each, ≤ 5 USD per payment
Pulse  17  15 subagents each pay gpu-cloud for a tool call, all at once: no consensus, each spends only its own allotment
Pulse  19  sub-researcher-3 tries to pay 6 USD, above its own limit → refused by the ledger: charter caveat violated: payment 6 above per-payment limit 5
Pulse  21  acme suspends the orchestrator: one transaction
Pulse  23  sub-researcher-3, two levels down, tries to pay 3 USD → refused by the ledger: charter caveat violated: charter suspended
Pulse  25  acme lifts the suspension
Pulse  28  sub-researcher-3 pays 3 USD: accepted
Pulse  30  the orchestrator revokes researcher-2 for good
Pulse  32  acme reclaims researcher-2's unspent budget: 32 USD
            sub-researcher-3's lineage: f62d720f → 2c022b22 → 90d88d8b → acme (depth 2, active)
            16 identities created in 3 transactions; every limit was checked by the ledger, not by the agents
```

Each Pulse line is one transaction, except the payments line, which is
fifteen transactions certified together. Every refusal comes from the
ledger: the subagents' code is not involved. See [Agent identities and
subagents](../concepts/agent-identities.md).

## tasks

```text
== Task Charters: paying for work only when the result passes a rule fixed in advance ==
           bonds: sandbox-vendor 5,000 USD behind its environment, arbiter 3,000, reviewers 1,000 each, coder 100
Pulse  17  sandbox-vendor claims "swe-sandbox-7 has no known exploit that passes grader g-3"  [backed by its 5,000 USD bond]
Pulse  19  lab registers its eval orchestrator with a 1,000 USD budget, at most 500 USD per task
Pulse  22  the orchestrator posts task 7243db20: 150 USD escrowed for coder, from its budget. Fixed now: a result must rest on
           sandbox-vendor's claim, may be attested by reviewer-1 or reviewer-2, and must pass the lab's policy at κ = 2
Pulse  24  …and task 0e88a347: a 40 USD bounty open to anyone, on the same terms
           coder submits patch-4711; reviewer-1 and reviewer-2 each attest that it passes, on their own bonds
Pulse  41  coder claims the 150 USD on its own 100 USD bond → refused by the ledger: invalid transaction: result not accepted: cost of corruption 90 (after detection discount 900/1000) is below κ·V = 300; only 100 of backing is unreserved; κ·V = 300 is needed
Pulse  43  coder settles with both attestations: cost of corruption 2,100 USD (100 + 1,000 + 1,000) covers κ·V = 300 → paid 150 USD; 300 USD of that backing reserved for the lab
Pulse  46  coder settles again → refused by the ledger: invalid transaction: the task is not open: a task is paid once
Pulse  49  bounty-hunter settles the open bounty with reviewer-2's attestation → paid 40 USD
Pulse  54  the orchestrator withdraws an unclaimed bounty: 30 USD back to the lab
Pulse  56  red-team shows that grader g-3 accepts any patch that deletes the failing test, and challenges the vendor's claim
Pulse  58  arbiter upholds it: the vendor's bond is slashed, and 4 claims fall: the environment's, and the 3 results resting on it, paid or not
           the lab is compensated 190 USD from the vendor's bond for the two results it paid for (lab +190 USD)
           red-team gets its deposit back and a share of the slashed bond: +2,505 USD, a bounty for the reward hack paid by the vendor
```

The lab never sees the coder's work: it sees a result claim committed to
the task and the output, two reviewers' attestations on their own bonds,
and the vendor's claim about its environment. The ledger pays only when
those add up to the rule the lab fixed at posting, pays once, and keeps
the lab's reliance backed until the reliance period ends; the red team is
paid by the vendor for finding the exploit. See [Task
Charters](../concepts/task-charters.md).

## market

```text
== Verifiable Clearing: a GPU-hour market that proves it is optimal ==
Pulse  56  operator opens a market: batches of 12 Pulses, sealed with the beacon, cleared by anyone with a certificate
Pulse  61  agent-1 bids for 5 GPU-hours at ≤ 2.10 USD: Success
Pulse  62  agent-2 bids for 3 GPU-hours at ≤ 1.90 USD: Success
Pulse  62  lab-a offers 5 GPU-hours at ≤ 1.80 USD: Success
Pulse  62  lab-b offers 6 GPU-hours at ≤ 2.00 USD: Success
           batch 5 sealed with the Pulse beacon 18f40c60 — 4 orders, nobody can reorder them now
           solver computes the welfare-maximizing allocation: 5 GPU-hours at one price 1.95 USD (supporting band 1.9–2), welfare 1.5 USD, with an LP optimality certificate
Pulse  73  the ledger checks the certificate in linear time and settles everyone at that price
           agent-1  now holds 5 GPU-hours and 990.2488 USD
           agent-2  now holds 0 GPU-hours and 999.9988 USD
           lab-a    now holds 0 GPU-hours and 109.7488 USD
           lab-b    now holds 6 GPU-hours and 99.9988 USD
Result: one uniform, certified-optimal price; a place in the batch is worth nothing, so there is nothing to front-run.
```

The demo prints `≤` for every limit; for an offer the limit is a floor. The
only trade that adds welfare is agent-1 buying lab-a's 5 GPU-hours, and any
price between 1.90 and 2.00 USD supports it; the canonical price is the
midpoint. See [Verifiable Clearing](../concepts/verifiable-clearing.md).

## receipts

```text
== Inference receipts: what would it cost to forge this answer? ==
Pulse  78  inference-co bonds 5,000 USD behind its receipts
Pulse  80  receipt issued  [dual-signature ∧ (TEE quote: nvidia-h100 ∨ bond:inference-co)]
           standard policy: cost to forge = 6,000 USD  (TEE attack ≈ 1,000 USD + 5,000 USD bond)
           the TEE's attack cost deters forgery but cannot repay anyone; only the bond is slashable
           standard           relying on 2000 USD → ACCEPT: cost of corruption 6,000 covers κ·V = 4,000
           standard           relying on 3000 USD → REJECT: only 5,000 of backing is unreserved; κ·V = 6,000 is needed
           assumes TEE broken relying on 2000 USD → ACCEPT: cost of corruption 5,000 covers κ·V = 4,000
           assumes TEE broken relying on 3000 USD → REJECT: cost of corruption 5,000 (after detection discount 1000/1000) is below κ·V = 6,000
           trusts the TEE     relying on 2000 USD → ACCEPT: cost of corruption ∞ covers κ·V = 4,000
           trusts the TEE     relying on 3000 USD → ACCEPT: cost of corruption ∞ covers κ·V = 6,000
Result: every consumer prices the same receipt under its own assumptions; trust is a number, not a brand.
```

See [Cost of corruption](../concepts/assurance/cost-of-corruption.md).

## x402

```text
== x402: an AI agent pays an API per request, inside limits the ledger enforces ==
Pulse  86  acme charters its agent: pay only carol, ≤ 1 USD per payment, ≤ 2 USD per window, 20 USD budget
           carol serves a forecast API at http://127.0.0.1:36159: 0.25 USD per call, settled on sion:b016a36ab737405623a15a6a9466c434
           agent: GET /v1/forecast → 402 Payment Required: 0.25 USD to carol (quote c752275a2dcfb17487dc0f7a829c6417)
Pulse  89  agent pays under its charter (purpose x402:<quote>) and retries → 200 OK {"region":"eu-west","gpu_hours":18250,"confidence":0.82}
           carol's server answered only after verifying the receipt as a light client: committee certificate, certified order, inclusion proof
           replaying the same PAYMENT-SIGNATURE → 402: unknown quote, or one already used
           agent: GET /v1/report (5 USD) → pays → the ledger refused the payment: charter caveat violated: payment 5 above per-payment limit 1; carol's API answers 402 and serves nothing
Pulse  93  bob, a person without a charter, pays the same API with a plain transfer → 200 OK
Result: no accounts, no API keys; every request is paid and proven before it is served, and the agent's limits live in the ledger, not in its prompt.
```

See [x402: selling and buying an API](../guides/x402.md).

## streams

From `sion demo streams --local` on a release build (15 seconds in all):

```text
== Streams: an agent pays an API a tenth of a cent per call, one hash per payment ==
Pulse   5  the agent opens a stream to carol: 20 USD in escrow, 0.001 USD a step, 100,000 steps; carol may settle until Pulse 2002
           carol's API at http://127.0.0.1:38625/v1/price: 0.001 USD per call, paid by `exact` (a transaction per call) or `stream` (a hash per call)
           one call paid with a transfer: a transaction, certified and proven, 0.0011 USD in fees
           2,000 calls paid from the stream in 0.6 s: carol's server checked 2,220 hashes and sent no transaction per call
           carol settled 2,000 payments in 2 transactions: 2 USD received, 0.002388 USD in fees (1.19 micro-USD per payment, against 1,100 for a transaction per call)
           the validators checked 44 hashes for those settlements (2 proven by a jump to a checkpoint), not one per payment; the stream is at position 2,000 with 18 USD left in escrow
           an auditor checks call 1,234's receipt: genuine and settled, in 766 hashes from the settled head, or 45 from the checkpoint below it
           a forged receipt for the same call → not an element of the stream's chain
Pulse  14  carol closes the stream; the agent gets 18 USD back
Result: each payment costs the server one hash, not a signature; a settlement costs the ledger the same however many payments it covers; and every payment stays checkable by anyone.
```

The server's 2,220 hashes are one per call plus the checkpoint paths it
checked (one every 100 calls). See [Payment streams](../concepts/payment-streams.md).

## censorship

From `sion demo censorship --local` (159 seconds):

```text
== Censorship resistance: forced inclusion makes censorship visible ==
           ADVERSARY SIMULATION: the gateway now drops every transaction from dissident, silently
           dissident signs: Pay 5 USD to alice. Fee 0.0012 USD; valid until Pulse 202.
           3 Pulses later: not executed; the gateway acknowledged it and did nothing
Pulse   9  posted as a forced intent: one honest proposer ordered it, the shard's validators certified it among themselves
           Done: paid 5 USD to alice; fee 0.0012 USD.
           ADVERSARY SIMULATION: 2 of the 4 validators now censor dissident as well (a quorum needs 3)
           forced intent posted around Pulse 9; by Pulse 19 it is OVERDUE (window 8 Pulses): the shard owes it and cannot certify it
Pulse  19  the state replaces shard 0's committee (rotation 1), drawing from validators outside it first; this devnet has 4 validators for 4 seats, so the same members return and the intent stays owed
           light client: forced inclusion: 1 intent(s) OVERDUE, censorship detected and certified by the committee (0 pending)
Pulse  23  censorship lifted: the intent executes late, and the record shows it: Done: paid 7 USD to alice; fee 0.0011 USD.
Result: a censoring gateway is routed around; a censoring third of a committee cannot hide, because every certified header counts what it owes.
```

Validator 0 also writes to standard error:

```text
validator: Pulse 19: forced intent 5ea754dc overdue in shard 0: censorship detected
validator: Pulse 19: shard 0's committee rotated (rotation 1)
```

See [Censorship resistance](../concepts/censorship-resistance.md).

## light

From `sion demo light`, run against a devnet that had just executed one
faucet transaction:

```text
== Light client: verify without running a node ==
           pinned chain identity 6345655c (genesis, validators, keys and shard layout)
           chain: 6345655c matches the pinned identity; 4 validators, keys derive their identifiers ✓
           Pulse 2 header da8f990c (state root aa697334, knowledge root b3950ae7, 0 claims)
           certificate: 300000 of 400000 stake signed (quorum 266667), each signature ML-DSA-44 + SLH-DSA-SHA2-128f ✓
           forced inclusion: no overdue intents (0 pending within their deadline) ✓
           receipts: 1 of 1 transactions proven against the receipts root, in the certified order ✓
           state: proof skipped (node already at Pulse 3)
           availability: 16 of 16 random shares of the 16×16 square verified; a withholding would go unnoticed with probability < 1.8e-3 ✓
           world proof: 2 leaves re-verified by stack ["A", "B"]; the published data is what executed and encodes to the certified data root (110 KB; replay proofs simulate succinct ones) ✓
           lane certificates: all 1 executed transactions locked their inputs with a quorum of their shard's committee, taken from the proven validator set ✓
           a node serving substituted validator keys is caught: header chain: the node serves chain 6fc4795f, not the pinned chain 6345655c ✗
```

The demo takes the chain identity from the devnet itself; a real client
pins it from a source it trusts. See
[Verify as a light client](light-client.md).

Run on its own with `--local`, the light demo fails after about 3 seconds
with `error: verification failed: no certified Pulse yet`: its fresh
devnet has not proven a Pulse yet. Inside `sion demo all --local` it runs
after six other demos, so there is a proven Pulse to check.

## staking

From `sion demo staking --local` (67 seconds). The line after the title,
which names the standby validator and the genesis validators' stake, is
left out:

```text
== Staking: committees come from the state, epoch by epoch ==
…
           epochs last 24 Pulses; this one ends after Pulse 24
Pulse   6  operator-4 stakes 50,000 USD for validator a6f32863, with a proof that it holds the validator's keys
           Done: validator a6f32863 registered by operator-4 with 50,000 USD at stake; fee 0.0032 USD.
Pulse  24  epoch 1 begins: 5 validators hold seats, the newcomer among them; 0.00224 USD of fees paid as rewards, split by stake
           every validator derived the same committees from the state: consensus (weighted by stake) and 2 lane committees sampled with the beacon
           light client: followed the committee from genesis through 1 boundary proof(s); Pulse 25's certificate has 4 of 5 validators, 350000 of 450000 stake (quorum 300001) ✓
Pulse  29  operator-2 collects: Done: 0.00056 USD of rewards claimed for validator 04dde303; fee 0.0012 USD.
           ADVERSARY SIMULATION: validator a6f32863 signs two different consensus headers for the same round
Pulse  31  watchdog reports both signed headers: Done: validator a6f32863 slashed: 50,000 USD forfeited, 2,500 USD of it to watchdog for the report; fee 0.005 USD.
           epoch 2 begins with 4 validators: the slashed validator lost its seat
           light client: still following, 2 boundary proof(s) from the pinned genesis ✓
Result: who certifies is part of the state transition, proven like any balance; stake buys seats, misbehaviour is evidence anyone can submit, and a client needs nothing but the genesis it pinned.
```

See [Staking and epochs](../guides/staking.md).

## What the demos do not show

The demos run on one machine, in one process, with devnet keys anyone can
derive. They show the protocol's logic end to end, not its performance or
its security against a real adversary. The adversaries in the
`censorship` demo are simulated by the node itself.
