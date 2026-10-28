"""Figure and code-block definitions for the Siontology site.

Every figure is backed by something you can run or read: a demo command, the
specification or the code, and its status in the reference implementation
(docs/implementation-status.md).
"""
REPO = "https://github.com/ColdAI-org/siontology"
SPEC = REPO + "/blob/main/docs/spec/state-transition.md"
TREE = REPO + "/tree/main/crates/"
BLOB = REPO + "/blob/main/crates/"

FIGURES = {
    "two-sorted": {
        "scene": "two-sorted",
        "title": "Two sorts of state",
        "label": "Animation: on the left, linear objects are consumed once and stay as tombstones; on the right, claims are appended to a grow-only knowledge log.",
        "desc": ("Left: linear objects, such as coins and allotments. A spend consumes its inputs, which stay in state "
                 "as tombstones (dashed), and creates new objects; every spend carries a lane certificate. Right: the "
                 "knowledge log. Claims are appended and never conflict, so they need no ordering; the knowledge root "
                 "and size grow with each claim. Nothing is ever deleted."),
        "spec": ("Specification &sect;2 and &sect;3", SPEC),
        "status": ("built", "Built"),
    },
    "lanes": {
        "scene": "lanes",
        "title": "Linear Lanes",
        "label": "Animation: four validators lock version 7 of a coin to transaction T1; three approvals form a certificate; a conflicting T2 for the same version gets at most two.",
        "desc": ("An owner spends version 7 of a coin with transaction T1. Each validator locks (coin, version 7) to the "
                 "first transaction it sees and signs its approvals in batches, one dual signature per batch. Approvals "
                 "from more than two thirds of the committee's stake, here three of four, form a certificate: T1 is "
                 "final without a consensus round. The owner then signs T2, spending the same version. V1 and V2 are "
                 "locked to T1 and refuse. Even with a faulty V3 approving both, T2 reaches two of four and never a "
                 "certificate, because any two quorums share an honest validator. V3's two approvals are signed "
                 "evidence of equivocation: its whole stake is slashed, with a share to whoever reports it."),
        "spec": ("crates/sion-consensus/src/lanes.rs", BLOB + "sion-consensus/src/lanes.rs"),
        "status": ("built", "Built"),
    },
    "dag": {
        "scene": "dag",
        "title": "Contention Consensus",
        "label": "Animation: four validators build a certified DAG round by round; leaders of even rounds commit together with their causal history.",
        "desc": ("Each of four validators proposes a header every round, referencing at least three certificates from "
                 "the round before. A header with votes from more than two thirds of the stake becomes a certificate "
                 "(solid). Even rounds have a leader, the anchor, drawn with a second outline. Once certificates of the "
                 "next round holding more than a third of the stake reference it, the anchor commits, and its history "
                 "that is not yet ordered joins the committed sequence, by round and then by author. Only contended "
                 "objects, such as a market or a shared registry, are ordered this way."),
        "spec": ("crates/sion-consensus/src/core.rs", BLOB + "sion-consensus/src/core.rs"),
        "status": ("simplified", "Simplified: a certified DAG; the design targets an uncertified one"),
    },
    "da": {
        "scene": "da",
        "title": "Data availability sampling",
        "label": "Animation: an 8 by 8 data square is extended to 16 by 16 with a Reed–Solomon code; a withheld block is caught by light clients sampling random shares.",
        "desc": ("A Pulse's data is laid out as a k &times; k square, here k = 8, and extended with a two-dimensional "
                 "Reed&ndash;Solomon code to 2k &times; 2k, so any half of a row or column rebuilds it. Rows and "
                 "columns are committed with hash-based Merkle roots, which need no trusted setup. To make the data "
                 "unrecoverable, a producer must withhold at least (k + 1)<sup>2</sup> of the 4k<sup>2</sup> shares, "
                 "more than a quarter. A light client that samples s random shares misses the withholding with "
                 "probability at most (3/4)<sup>s</sup>: about 1% at s = 16 and about 0.02% at s = 30."),
        "run": "sion verify --samples 16",
        "spec": ("crates/sion-da", TREE + "sion-da"),
        "status": ("built", "Built"),
    },
    "fold": {
        "scene": "fold",
        "title": "The Fold and two proof stacks",
        "label": "Animation: chunk statements are verified by two independent proof stacks and folded pairwise into two roots; the World Proof stands only if both accept.",
        "desc": ("A Pulse executes as a chain of chunks, each with a statement binding the world before and after it; "
                 "the last chunk is the deferred phase. Stack A, the reference executor, and stack B, an independent "
                 "implementation written only from the specification, each verify every chunk. Their proofs fold "
                 "pairwise into two roots, and the World Proof is the pair: a client accepts it only if both stacks "
                 "accept. In the second Pulse shown, stack A has a hypothetical soundness bug and accepts an invalid "
                 "chunk; stack B rejects it, so no World Proof exists for that Pulse. Today both stacks verify by "
                 "replay: they bind the same statements a succinct proof would, but checking costs as much as "
                 "executing."),
        "run": "sion verify --chain <identity>",
        "spec": ("Specification &sect;8 and &sect;9", SPEC),
        "status": ("simulated", "Simulated: replay proofs with real bindings"),
    },
    "assurance": {
        "scene": "assurance",
        "title": "The Assurance Algebra",
        "label": "Animation: a claim's trust formula evaluates to a cost of corruption of 60,000 USD; relying on 25,000 USD reserves 50,000 USD of backing; a second, equal reliance is refused.",
        "desc": ("The worked example from the architecture paper. Invoice #1042 is payable if three inputs all hold "
                 "(&and;): delivery, confirmed by three independent witnesses bonded at 20,000 USD each (&or;, so "
                 "their bonds add up to 60,000); the invoice arithmetic, proven by two independent stacks (&infin; "
                 "under a policy that trusts the cryptography); and the supplier's identity, backed by a bank's "
                 "1,000,000 USD bond. The cost of corruption is the minimum over the &and;: 60,000 USD, so delivery "
                 "is the weak link. A financier with 25,000 USD at risk and safety factor &kappa; = 2 needs 50,000, "
                 "accepts, and reserves 50,000 down the formula. A second financier asking the same is refused: only "
                 "10,000 USD of the delivery route is still unreserved."),
        "run": "sion demo invoice --local",
        "spec": ("crates/sion-assurance", TREE + "sion-assurance"),
        "status": ("built", "Built"),
    },
    "clearing": {
        "scene": "clearing",
        "title": "Verifiable Clearing",
        "label": "Animation: four sealed GPU-hour orders clear at one price, 1.95 USD, with a certificate that no better allocation exists.",
        "desc": ("From the market demo. Four orders arrive during a batch window: agent-1 bids for 5 GPU-hours at up "
                 "to 2.10 USD, agent-2 for 3 at up to 1.90; lab-a offers 5 at 1.80 or more, lab-b 6 at 2.00 or more. "
                 "The batch seals with the Pulse's beacon, so nobody can reorder it, and the orders are revealed. "
                 "The welfare-maximising allocation trades 5 GPU-hours. Every uniform price from 1.90 to 2.00 "
                 "supports it, and the canonical price is the midpoint, 1.95 USD. The solver submits the allocation "
                 "with its dual prices, and the ledger checks the certificate in linear time: both feasible, both "
                 "non-negative, no duality gap. Everyone trades at one price, so a place in the batch is worth "
                 "nothing."),
        "run": "sion demo market --local",
        "spec": ("Specification &sect;5.22 to &sect;5.25", SPEC),
        "status": ("built", "Built"),
    },
    "charters": {
        "scene": "charters",
        "title": "Agent Charters",
        "label": "Animation: an AI agent pays within its charter; a payment to an unlisted payee and an over-limit payment are refused; revoking the charter stops the agent and its sub-agent.",
        "desc": ("From the agents demo. acme charters its agent: pay only gpu-cloud, at most 100 USD per payment and "
                 "250 USD per window, a 1,000 USD budget, receipts on, 500 USD bonded behind the receipts. The agent "
                 "pays gpu-cloud 80 USD and a receipt claim joins the knowledge log. Prompt-injected, it tries to pay "
                 "mallory 90 USD: the ledger refuses, payee not allowed. It tries 150 USD: refused, above the "
                 "per-payment limit. It delegates 100 USD to a sub-agent under narrower caveats, at most 20 USD per "
                 "payment, and the sub-agent pays 20 USD. Once acme revokes the charter, both the agent and the "
                 "sub-agent are refused."),
        "run": "sion demo agents --local",
        "spec": ("Specification &sect;5.12 to &sect;5.17", SPEC),
        "status": ("built", "Built"),
    },
    "forced": {
        "scene": "forced",
        "title": "Forced inclusion",
        "label": "Animation: a censored payment is posted straight to consensus; when the shard misses its 8-Pulse deadline, certified Pulse headers count the intent as overdue and the shard's committee is rotated.",
        "desc": ("From the censorship demo. A gateway silently drops a user's payment. The user posts it as a forced "
                 "intent straight to consensus, where every validator proposes every round, so one honest proposer "
                 "is enough; the shard's validators certify it among themselves and it executes. Then two of the "
                 "shard's four validators censor as well, so no certificate, which needs three, can form. The shard "
                 "has a window of 8 Pulses to execute the intent. When the deadline passes and the intent could "
                 "still execute, every certified Pulse header counts it as overdue, any light client sees it, and "
                 "the state replaces the shard's committee, drawing from validators outside it first, as drawn here. On the "
                 "default devnet, four validators for four seats, the same members return and the intent stays owed. "
                 "The architecture also freezes the shard until it meets the obligation; the implementation does "
                 "not."),
        "run": "sion demo censorship --local",
        "spec": ("Specification &sect;6.3", SPEC),
        "status": ("simplified", "Simplified: committee rotation, but the shard is not frozen"),
    },
    "x402": {
        "scene": "x402",
        "title": "x402: paying per request",
        "label": "Animation: an agent's request gets 402 Payment Required; it pays on Siontology under its charter; the server verifies the receipt as a light client and answers 200 OK.",
        "desc": ("From the x402 demo. The agent asks carol's API for a forecast and gets 402 Payment Required with a "
                 "quote: 0.25 USD to carol, scheme exact, a fresh nonce. The agent signs a payment under its charter "
                 "whose purpose names the quote, and retries with it. carol's facilitator submits the payment, waits "
                 "for it to execute and verifies the receipt the way a light client would: committee certificate, "
                 "certified order, inclusion proof. Only then does the API answer 200 OK. Replaying the same payment "
                 "is refused, because each quote pays for one request."),
        "run": "sion demo x402 --local",
        "spec": ("crates/sion-cli/src/x402.rs", BLOB + "sion-cli/src/x402.rs"),
        "status": ("built", "Built: x402 version 2, not yet tested against third-party clients"),
    },
    "streams": {
        "scene": "streams",
        "title": "Payment streams: pay per call, settle per million",
        "label": "Animation: an AI agent pays an API per call with the next element of its hash chain; the API checks each payment with one hash; after about a million calls, one settlement proven by a checkpoint puts them all on the ledger.",
        "desc": ("From the streams demo and the streams benchmark. The agent opened a payment stream to the API: an "
                 "escrow, and the last element of a secret hash chain. Each call is paid with the next element, "
                 "which the API checks with one SHA3-256 hash against the last one it accepted, 0.42 &micro;s on one "
                 "core; nothing per call reaches the ledger. The API then settles every payment so far in one "
                 "transaction, ordered by consensus. With checkpoints, that settlement walks only to the checkpoint "
                 "below its position and proves the checkpoint's Merkle path: 524 hashes and one signature check for "
                 "1,048,064 payments, about a nanosecond per payment on the ledger. Payments are one-way, from one "
                 "payer to one payee, out of escrow the payer locked in advance, and the rates were measured on one "
                 "core of a two-core machine, not on a network."),
        "run": "sion demo streams --local",
        "spec": ("Specification &sect;5.31 to &sect;5.34", SPEC),
        "status": ("built", "Built on the devnet; measured on one machine"),
    },
    "staking": {
        "scene": "staking",
        "title": "Staking, epochs and slashing",
        "label": "Animation: a standby validator stakes 50,000 USD and joins at the next epoch boundary; it signs two headers for one round, is reported, forfeits its stake and loses its seat.",
        "desc": ("From the staking demo. A standby validator stakes 50,000 USD with a proof that it holds its keys "
                 "and joins the active set at the next epoch boundary; the committees are re-derived from the state. "
                 "It then signs two consensus headers for one round, and a watchdog reports the signed evidence. Its "
                 "whole stake of 50,000 USD is forfeited, 2,500 USD of it (5 % on the devnet) goes to the watchdog, "
                 "and it loses its seat at the next boundary. "
                 "Every committee change follows rules in the state, so the World Proof covers it."),
        "run": "sion demo staking --local",
        "spec": ("Specification &sect;5.26 to &sect;5.30", SPEC),
        "status": ("built", "Built on the devnet"),
    },
    "pq": {
        "scene": "pq",
        "title": "Two signature families",
        "label": "Animation: every authorization carries an ML-DSA-44 signature and an SLH-DSA signature; breaking one family alone forges nothing.",
        "desc": ("Every account and validator signs with two unrelated families at once: ML-DSA-44 (FIPS 204, module "
                 "lattices) and SLH-DSA (FIPS 205, hash-based). A verifier requires both. An attacker who breaks "
                 "lattices forges only the first signature and is refused; one who breaks hash functions forges only "
                 "the second and is refused. Forgery needs both families broken, so the trust formula of an "
                 "authorization is lattice &or; hash. The bars show the cost in bytes: 2,420 for ML-DSA-44 and "
                 "17,088 for SLH-DSA-SHA2-128f, the repository's default second signature, against 64 for Ed25519, "
                 "which is not post-quantum and is not used for authorization."),
        "run": "sion bench --only signatures",
        "spec": ("crates/sion-crypto", TREE + "sion-crypto"),
        "status": ("built", "Built: pure-Rust FIPS 203, 204 and 205 crates, not yet audited"),
    },
}

CODE = {
    "quickstart": ("Quick start (Rust 1.80 or later)",
                   "git clone https://github.com/ColdAI-org/siontology\n"
                   "cd siontology\n"
                   "cargo build --release\n"
                   "alias sion=./target/release/sion\n"
                   "\n"
                   "<span class=\"c\"># Run every scripted demo against an in-process devnet</span>\n"
                   "sion demo all --local"),
    "devnet": ("A devnet and a client, in two terminals",
               "<span class=\"c\"># Terminal 1: 4 validators, 2 shards, API on port 8730</span>\n"
               "sion devnet\n"
               "\n"
               "<span class=\"c\"># Terminal 2</span>\n"
               "sion status\n"
               "sion keys new dana    <span class=\"c\"># dual post-quantum keys</span>\n"
               "sion faucet dana 100\n"
               "sion send dana alice 12.5\n"
               "sion balance dana\n"
               "sion verify --chain &lt;identity from sion devnet&gt;"),
    "verify-output": ("sion verify: output from a devnet run (hashes differ on every run)",
                      "chain: 6345655c matches the pinned identity; 4 validators, keys derive their identifiers ✓\n"
                      "Pulse 14 header d42357fe (state root aafa7aec, knowledge root b3950ae7, 0 claims)\n"
                      "certificate: 300000 of 400000 stake signed (quorum 266667), each signature ML-DSA-44 + SLH-DSA-SHA2-128f ✓\n"
                      "forced inclusion: no overdue intents (0 pending within their deadline) ✓\n"
                      "receipts: 1 of 1 transactions proven against the receipts root, in the certified order ✓\n"
                      "state: an account object proven against the state root ✓\n"
                      "availability: 16 of 16 random shares of the 32×32 square verified; a withholding would go unnoticed with probability &lt; 4.7e-3 ✓\n"
                      "world proof: 2 leaves re-verified by stack [\"A\", \"B\"]; the published data is what executed and encodes to the certified data root (113 KB; replay proofs simulate succinct ones) ✓\n"
                      "lane certificates: all 1 executed transactions locked their inputs with a quorum of their shard's committee, taken from the proven validator set ✓"),
    "mcp": ("MCP client configuration",
            "{\n"
            "  \"mcpServers\": {\n"
            "    \"siontology\": {\n"
            "      \"command\": \"sion\",\n"
            "      \"args\": [\"mcp\"],\n"
            "      \"env\": { \"SION_NODE\": \"http://127.0.0.1:8730\" }\n"
            "    }\n"
            "  }\n"
            "}"),
    "x402": ("Sell an API per request, and buy from it",
             "sion x402 serve carol --price 0.25\n"
             "<span class=\"c\"># serves http://127.0.0.1:8402/v1/resource for 0.25 USD a request</span>\n"
             "\n"
             "sion x402 fetch http://127.0.0.1:8402/v1/resource --wallet dana --max 1"),
    "streams": ("Pay per call from a stream, one hash per call",
                "sion x402 serve carol --price 0.001 --streams\n"
                "\n"
                "sion stream open dana carol --unit 0.001 --deposit 5\n"
                "<span class=\"c\"># prints the stream's ID; each call below is paid with one hash</span>\n"
                "sion x402 fetch http://127.0.0.1:8402/v1/resource --stream &lt;ID&gt; --count 1000\n"
                "\n"
                "sion bench --only streams   <span class=\"c\"># what a payment costs the API and the ledger</span>"),
    "http": ("HTTP API",
             "curl http://127.0.0.1:8730/v1/status\n"
             "curl http://127.0.0.1:8730/v1/pulses/latest\n"
             "curl http://127.0.0.1:8730/v1/claims/&lt;claim id&gt;\n"
             "\n"
             "<span class=\"c\"># the decision rule for 25,000 USD at risk (amounts in micro-units)</span>\n"
             "curl -X POST http://127.0.0.1:8730/v1/claims/&lt;claim id&gt;/decide \\\n"
             "     -d '{\"value\": 25000000000}'"),
    "develop": ("Before every commit",
                "cargo fmt --all\n"
                "cargo clippy --workspace --all-targets -- -D warnings\n"
                "cargo test --workspace"),
    "bench": ("Measure on your machine",
              "sion bench                 <span class=\"c\"># signatures, hashing, data availability, clearing,</span>\n"
              "                           <span class=\"c\"># assurance, execution, proving and a live devnet</span>\n"
              "sion bench --quick --only signatures,data"),
}


# Real output of the scripted demos (docs/book/src/getting-started/demos.md),
# shown on the use-cases page.
CODE.update({
    "demo-agents": ('sion demo agents --local: real output',
        '== Agent Charters: an AI agent with a budget the ledger enforces ==\n'
        "Pulse  32  acme charters its agent: pay only gpu-cloud, ≤ 100 USD per payment, ≤ 250 USD per window, 1,000 USD budget, receipts on, 500 USD bond behind the agent's receipts\n"
        'Pulse  34  agent pays gpu-cloud 80 USD for inference: Success; receipt claim 2c4e5264 joins the knowledge log\n'
        'Pulse  36  a prompt-injected agent tries to pay mallory 90 USD → refused by the ledger: charter caveat violated: payee not allowed\n'
        'Pulse  38  the agent tries a 150 USD payment → refused by the ledger: charter caveat violated: payment 150 above per-payment limit 100\n'
        'Pulse  41  agent delegates 100 USD to a sub-agent under narrower caveats (≤ 20 USD per payment)\n'
        'Pulse  43  sub-agent pays gpu-cloud 20 USD: Success\n'
        'Pulse  45  acme revokes the charter (ordered by consensus)\n'
        'Pulse  47  acme-agent tries to pay again → charter caveat violated: charter revoked\n'
        'Pulse  49  sub-agent tries to pay again → charter caveat violated: charter revoked\n'
        "           the agent's receipt is a claim backed by acme's bond: cost to forge it 500 USD\n"
        'Result: gpu-cloud received 100 USD; mallory received nothing; the limits lived in the ledger, not the agent.'),
    "demo-x402": ('sion demo x402 --local: real output',
        '== x402: an AI agent pays an API per request, inside limits the ledger enforces ==\n'
        'Pulse  86  acme charters its agent: pay only carol, ≤ 1 USD per payment, ≤ 2 USD per window, 20 USD budget\n'
        '           carol serves a forecast API at http://127.0.0.1:36159: 0.25 USD per call, settled on sion:b016a36ab737405623a15a6a9466c434\n'
        '           agent: GET /v1/forecast → 402 Payment Required: 0.25 USD to carol (quote c752275a2dcfb17487dc0f7a829c6417)\n'
        'Pulse  89  agent pays under its charter (purpose x402:&lt;quote&gt;) and retries → 200 OK {"region":"eu-west","gpu_hours":18250,"confidence":0.82}\n'
        "           carol's server answered only after verifying the receipt as a light client: committee certificate, certified order, inclusion proof\n"
        '           replaying the same PAYMENT-SIGNATURE → 402: unknown quote, or one already used\n'
        "           agent: GET /v1/report (5 USD) → pays → the ledger refused the payment: charter caveat violated: payment 5 above per-payment limit 1; carol's API answers 402 and serves nothing\n"
        'Pulse  93  bob, a person without a charter, pays the same API with a plain transfer → 200 OK\n'
        "Result: no accounts, no API keys; every request is paid and proven before it is served, and the agent's limits live in the ledger, not in its prompt."),
    "demo-receipts": ('sion demo receipts --local: real output',
        '== Inference receipts: what would it cost to forge this answer? ==\n'
        'Pulse  78  inference-co bonds 5,000 USD behind its receipts\n'
        'Pulse  80  receipt issued  [dual-signature ∧ (TEE quote: nvidia-h100 ∨ bond:inference-co)]\n'
        '           standard policy: cost to forge = 6,000 USD  (TEE attack ≈ 1,000 USD + 5,000 USD bond)\n'
        "           the TEE's attack cost deters forgery but cannot repay anyone; only the bond is slashable\n"
        '           standard           relying on 2000 USD → ACCEPT: cost of corruption 6,000 covers κ·V = 4,000\n'
        '           standard           relying on 3000 USD → REJECT: only 5,000 of backing is unreserved; κ·V = 6,000 is needed\n'
        '           assumes TEE broken relying on 2000 USD → ACCEPT: cost of corruption 5,000 covers κ·V = 4,000\n'
        '           assumes TEE broken relying on 3000 USD → REJECT: cost of corruption 5,000 (after detection discount 1000/1000) is below κ·V = 6,000\n'
        '           trusts the TEE     relying on 2000 USD → ACCEPT: cost of corruption ∞ covers κ·V = 4,000\n'
        '           trusts the TEE     relying on 3000 USD → ACCEPT: cost of corruption ∞ covers κ·V = 6,000\n'
        'Result: every consumer prices the same receipt under its own assumptions; trust is a number, not a brand.'),
    "demo-invoice": ('sion demo invoice --local: real output',
        '== Invoice #1042: trade finance on insured knowledge ==\n'
        'Pulse   6  supplier bonds 30,000 USD behind "invoices"\n'
        'Pulse   8  shipper bonds 20,000 USD behind "logistics"\n'
        'Pulse  10  judge bonds 100,000 USD behind "arbitration"\n'
        'Pulse  12  shipper attests "Shipment for invoice #1042 delivered"  [dual-signature ∧ bond:shipper]\n'
        'Pulse  15  supplier issues "Invoice #1042: 10,000 USD payable"  [dual-signature ∧ (bond:supplier ∨ delivery)]\n'
        '           cost of corruption (standard policy): 50,000 USD, exact\n'
        "           = 30,000 (supplier's bond) + 20,000 (shipper's bond): a forger must corrupt both routes\n"
        '           rely on  10000 USD at κ = 2 → ACCEPT: cost of corruption 50,000 covers κ·V = 20,000\n'
        '           rely on  30000 USD at κ = 2 → REJECT: cost of corruption 50,000 (after detection discount 1000/1000) is below κ·V = 60,000\n'
        'Pulse  18  financier funds the invoice and reserves 20,000 USD of its backing (κ·V): nobody else can rely on the same bonds beyond what slashing can repay\n'
        "Pulse  20  buyer challenges the supplier's attestation (deposit 1,000 USD)\n"
        "Pulse  22  judge upholds: supplier's bond slashed (30,000 USD); the invoice still holds through the delivery route\n"
        "Pulse  24  buyer challenges the shipper's delivery attestation, under the shipper's own adjudicator\n"
        'Pulse  26  judge upholds: 2 claims falsified (the delivery, and the invoice resting on it)\n'
        '           financier compensated 10,000 USD automatically from the slashed bonds (balance 199,999.9988 → 209,999.9988)\n'
        'Result: reliance on a false invoice was repaid in full, in Pulses, with no lawsuit and no adjuster.'),
    "demo-streams": ('sion demo streams --local: real output',
        '== Streams: an agent pays an API a tenth of a cent per call, one hash per payment ==\n'
        'Pulse   5  the agent opens a stream to carol: 20 USD in escrow, 0.001 USD a step, 100,000 steps; carol may settle until Pulse 2002\n'
        "           carol's API at http://127.0.0.1:38625/v1/price: 0.001 USD per call, paid by `exact` (a transaction per call) or `stream` (a hash per call)\n"
        '           one call paid with a transfer: a transaction, certified and proven, 0.0011 USD in fees\n'
        "           2,000 calls paid from the stream in 0.6 s: carol's server checked 2,220 hashes and sent no transaction per call\n"
        '           carol settled 2,000 payments in 2 transactions: 2 USD received, 0.002388 USD in fees (1.19 micro-USD per payment, against 1,100 for a transaction per call)\n'
        '           the validators checked 44 hashes for those settlements (2 proven by a jump to a checkpoint), not one per payment; the stream is at position 2,000 with 18 USD left in escrow\n'
        "           an auditor checks call 1,234's receipt: genuine and settled, in 766 hashes from the settled head, or 45 from the checkpoint below it\n"
        "           a forged receipt for the same call → not an element of the stream's chain\n"
        'Pulse  14  carol closes the stream; the agent gets 18 USD back\n'
        'Result: each payment costs the server one hash, not a signature; a settlement costs the ledger the same however many payments it covers; and every payment stays checkable by anyone.'),
    "demo-market": ('sion demo market --local: real output',
        '== Verifiable Clearing: a GPU-hour market that proves it is optimal ==\n'
        'Pulse  56  operator opens a market: batches of 12 Pulses, sealed with the beacon, cleared by anyone with a certificate\n'
        'Pulse  61  agent-1 bids for 5 GPU-hours at ≤ 2.10 USD: Success\n'
        'Pulse  62  agent-2 bids for 3 GPU-hours at ≤ 1.90 USD: Success\n'
        'Pulse  62  lab-a offers 5 GPU-hours at ≤ 1.80 USD: Success\n'
        'Pulse  62  lab-b offers 6 GPU-hours at ≤ 2.00 USD: Success\n'
        '           batch 5 sealed with the Pulse beacon 18f40c60 — 4 orders, nobody can reorder them now\n'
        '           solver computes the welfare-maximizing allocation: 5 GPU-hours at one price 1.95 USD (supporting band 1.9–2), welfare 1.5 USD, with an LP optimality certificate\n'
        'Pulse  73  the ledger checks the certificate in linear time and settles everyone at that price\n'
        '           agent-1  now holds 5 GPU-hours and 990.2488 USD\n'
        '           agent-2  now holds 0 GPU-hours and 999.9988 USD\n'
        '           lab-a    now holds 0 GPU-hours and 109.7488 USD\n'
        '           lab-b    now holds 6 GPU-hours and 99.9988 USD\n'
        'Result: one uniform, certified-optimal price; a place in the batch is worth nothing, so there is nothing to front-run.'),
})

# Real output of the demos added with tasks, memory and storage.
CODE.update({
    'demo-tasks': ('sion demo tasks --local: real output',
        '== Task Charters: paying for work only when the result passes a rule fixed in advance ==\n'
        '           bonds: sandbox-vendor 5,000 USD behind its environment, arbiter 3,000, reviewers 1,000 each, coder 100\n'
        'Pulse  17  sandbox-vendor claims "swe-sandbox-7 has no known exploit that passes grader g-3"  [backed by its 5,000 USD bond]\n'
        'Pulse  19  lab registers its eval orchestrator with a 1,000 USD budget, at most 500 USD per task\n'
        'Pulse  22  the orchestrator posts task 7243db20: 150 USD escrowed for coder, from its budget. Fixed now: a result must rest on\n'
        "           sandbox-vendor's claim, may be attested by reviewer-1 or reviewer-2, and must pass the lab's policy at κ = 2\n"
        'Pulse  24  …and task 0e88a347: a 40 USD bounty open to anyone, on the same terms\n'
        '           coder submits patch-4711; reviewer-1 and reviewer-2 each attest that it passes, on their own bonds\n'
        'Pulse  41  coder claims the 150 USD on its own 100 USD bond → refused by the ledger: invalid transaction: result not accepted: cost of corruption 90 (after detection discount 900/1000) is below κ·V = 300; only 100 of backing is unreserved; κ·V = 300 is needed\n'
        'Pulse  43  coder settles with both attestations: cost of corruption 2,100 USD (100 + 1,000 + 1,000) covers κ·V = 300 → paid 150 USD; 300 USD of that backing reserved for the lab\n'
        'Pulse  46  coder settles again → refused by the ledger: invalid transaction: the task is not open: a task is paid once\n'
        "Pulse  49  bounty-hunter settles the open bounty with reviewer-2's attestation → paid 40 USD\n"
        'Pulse  54  the orchestrator withdraws an unclaimed bounty: 30 USD back to the lab\n'
        "Pulse  56  red-team shows that grader g-3 accepts any patch that deletes the failing test, and challenges the vendor's claim\n"
        "Pulse  58  arbiter upholds it: the vendor's bond is slashed, and 4 claims fall: the environment's, and the 3 results resting on it, paid or not\n"
        "           the lab is compensated 190 USD from the vendor's bond for the two results it paid for (lab +190 USD)\n"
        '           red-team gets its deposit back and a share of the slashed bond: +2,505 USD, a bounty for the reward hack paid by the vendor'),
    'demo-robots': ('sion demo robots --local: real output',
        '== Memory for robots: a warehouse two robots share, kept append-only by the ledger ==\n'
        '           fleet gives robot-alpha and robot-beta agent identities of their own\n'
        '           each robot opens a memory log only it can append to, tied to its identity; the curator opens one too\n'
        "Pulse  15  robot-alpha commits its log: pallet-17 seen in aisle 4. This Pulse is the observation's transaction time; the robot cannot backdate it\n"
        'Pulse  17  robot-beta commits its log: aisle 4 scanned 10 s later, empty\n'
        '           a reader asks robot-alpha for everything it recorded in aisle 4 during that minute: 1 atom, with a 263-byte proof checked against the head on the ledger\n'
        '           and for aisle 9: 0 atoms, with a proof that it recorded nothing there\n'
        'Pulse  19  robot-alpha tries to replace its observation with one in aisle 5: refused by the ledger: invalid transaction: the new head does not extend the log: invalid proof: equal sizes require equal roots\n'
        "Pulse  23  the curator records that the two robots disagree, then refutes robot-alpha's observation\n"
        '           a reader trusting the curator believes the observation refuted; one trusting only robot-alpha believes it accepted. Same evidence, two beliefs, each checked against the heads on the ledger\n'
        'Pulse  25  robot-beta commits 2 more scans, with an index that leaves out its scan of aisle 4 (the ledger stores index roots without checking them)\n'
        '           anyone holding that scan proves the omission from the head on the ledger alone, with a 517-byte fraud proof\n'
        "Pulse  27  robot-alpha stores a camera frame encrypted under its memory space's key (readable: true), then forgets it by destroying the key (readable: false); the log still proves the frame existed\n"
        'Pulse  29  fleet suspends robot-alpha; at Pulse 31 its next commit is refused by the ledger: charter caveat violated: charter suspended'),
    'demo-storage': ('sion demo storage --local: real output',
        "== Storage for agents: a robot's memory kept by devices it pays, proven every round ==\n"
        "Pulse   7  robot-gamma commits 400 observations of a patrol; the head's manifest is the root of their archive (88 KiB, 23 leaves of at most 4 KiB)\n"
        'Pulse   9  robot-gamma posts an open deal for the archive: 6 rounds of 16 Pulses at 0.5 USD a proven round (3 USD in escrow), 4 leaves challenged a round, 3 USD of collateral\n'
        'Pulse  11  and a deal for its camera footage (16 leaves) with the phone, on the same terms\n'
        "           a laptop and a phone run providers: each fetches a deal's data from its locator in the background, checks it hashes to the deal's root, posts the collateral, then proves every round\n"
        '           laptop: fetching deal 54fef09d from file://…/storage/robot-gamma/patrol-archive.bin\n'
        '           phone:  fetching deal e6c488cb from file://…/storage/robot-gamma/camera.bin\n'
        '           laptop: Pulse 16: Done: laptop accepted storage deal 54fef09d (custody record 1a9dc946); rounds start at Pulse 24; fee 0.0012 USD.\n'
        '           phone:  Pulse 18: Done: phone accepted storage deal e6c488cb (custody record a86a1d8b); rounds start at Pulse 24; fee 0.0012 USD.\n'
        '           the phone then quietly keeps only leaves 0 to 7 of the footage, to save space: the deal still pays it, as long as nobody asks for the rest\n'
        '           round 0: the phone is asked for leaves [7, 0, 8, 11]; it lost 2 of them and cannot answer\n'
        '           laptop: Pulse 28: Done: round 0 of storage deal 54fef09d proven; fee 0.0027 USD.\n'
        '           round 1: the phone is asked for leaves [9, 0, 8, 5]; it lost 2 of them and cannot answer\n'
        '           laptop: Pulse 43: Done: round 1 of storage deal 54fef09d proven; fee 0.0027 USD.\n'
        '           round 2: the phone is asked for leaves [2, 2, 10, 6]; it lost 1 of them and cannot answer\n'
        '           laptop: Pulse 58: Done: round 2 of storage deal 54fef09d proven; fee 0.0027 USD.\n'
        'Pulse  74  robot-gamma closes the footage deal (abandoned): 3 round(s) missed; 3 USD of escrow back, 0 USD paid for what the phone proved, its collateral of 3 USD forfeited to the treasury\n'
        '           laptop: Pulse 77: Done: round 3 of storage deal 54fef09d proven; fee 0.0027 USD.\n'
        '           laptop: Pulse 79: Done: 2 USD paid for 4 proven round(s) of storage deal 54fef09d; fee 0.0011 USD.\n'
        '           laptop: Pulse 90: Done: round 4 of storage deal 54fef09d proven; fee 0.0027 USD.\n'
        '           laptop: Pulse 107: Done: round 5 of storage deal 54fef09d proven; fee 0.0027 USD.\n'
        '           laptop: Pulse 122: Done: storage deal 54fef09d closed: 0 USD refunded, 1 USD paid, 3 USD of collateral returned, 0 USD forfeited, 0 round(s) missed; fee 0.0011 USD.\n'
        '           the laptop proved 6 of 6 rounds (0 missed): paid 3 USD and its collateral returned; it kept the archive the whole time\n'
        "           a reader checks two objects on the ledger: robot-gamma's memory head names manifest 86cfe894, and the deal the laptop proved commits to root 86cfe894: the memory the robot committed is the data that was kept\n"
        '           what a proof shows: the provider could produce the challenged leaves during the round; not that it stores its own copy (no proof of replication)'),
})


# Pictograms for the use cases, drawn on a 32-unit grid in the diagram
# vocabulary (brand/guidelines.md, Iconography): a diamond is an AI agent, a
# solid cell something real and final, an outline cell a claim, tally strokes
# approvals, a hatched cell a batch ordered by consensus, a rounded rectangle a
# phone; a check means accepted and a cross, the only colour, refused.
_S = 'fill="none" stroke="currentColor" stroke-width="1.6"'
ICONS = {
    "agent": ('<path d="M11 4 20 16 11 28 2 16Z" ' + _S + '/>'
              '<rect x="23" y="20" width="7" height="7" fill="currentColor"/>'
              '<path class="pict-signal" d="M23 5l7 7M30 5l-7 7" fill="none" stroke-width="1.6"/>'),
    "api": ('<rect x="2" y="10" width="12" height="12" fill="currentColor"/>'
            '<path d="M16 16h5M18.5 13.5 21 16l-2.5 2.5" ' + _S + '/>'
            '<path d="M23 16.5l2.5 2.5L30 12" ' + _S + '/>'),
    "receipt": ('<rect x="4" y="3" width="17" height="23" ' + _S + '/>'
                '<path d="M8 9h9M8 13h9M8 17h6" ' + _S + '/>'
                '<path d="M19 25l3 3 7-9" ' + _S + '/>'),
    "approval": ('<path d="M4 7v18M9 7v18M14 7v18M19 7v18M2 22 21 10" ' + _S + '/>'
                 '<path class="pict-signal" d="M24 19l6 6M30 19l-6 6" fill="none" stroke-width="1.6"/>'),
    "invoice": ('<rect x="5" y="3" width="22" height="15" ' + _S + '/>'
                '<path d="M9 8h14M9 12h9" ' + _S + '/>'
                '<rect x="5" y="21" width="22" height="8" fill="currentColor"/>'),
    "market": ('<rect x="3" y="7" width="18" height="18" ' + _S + '/>'
               '<path d="M3 13l6-6M3 19 15 7M3 25 21 7M9 25l12-12M15 25l6-6" fill="none" stroke="currentColor" stroke-width="1.2"/>'
               '<path d="M23 17l2.5 2.5L30 13" ' + _S + '/>'),
    "settlement": ('<rect x="2" y="12" width="8" height="8" fill="currentColor"/>'
                   '<path d="M11 16h10M18.5 13.5 21 16l-2.5 2.5" ' + _S + '/>'
                   '<rect x="22" y="12" width="8" height="8" fill="currentColor"/>'
                   '<rect x="7" y="4" width="18" height="5" ' + _S + '/>'),
    "provenance": ('<rect x="3" y="5" width="14" height="19" ' + _S + '/>'
                   '<path d="M7 11h6M7 15h6" ' + _S + '/>'
                   '<rect x="19" y="8" width="10" height="19" rx="2.5" ' + _S + '/>'
                   '<path d="M21.5 18l2 2 3.5-4.5" ' + _S + '/>'),
    'tasks': '<rect x="2" y="12" width="9" height="9" fill="currentColor"/><path d="M13 16.5h6M16.5 14l2.5 2.5-2.5 2.5" fill="none" stroke="currentColor" stroke-width="1.6"/><rect x="21" y="6" width="9" height="21" fill="none" stroke="currentColor" stroke-width="1.6"/><path d="M23 17l2 2 3.5-4.5" fill="none" stroke="currentColor" stroke-width="1.6"/>',
    'memory': '<rect x="3" y="4" width="20" height="5" fill="currentColor"/><rect x="3" y="11" width="20" height="5" fill="currentColor"/><rect x="3" y="18" width="20" height="5" fill="none" stroke="currentColor" stroke-width="1.6"/><path d="M3 27h20" fill="none" stroke="currentColor" stroke-width="1.6"/><path class="pict-signal" d="M25 4l5 5M30 4l-5 5" fill="none" stroke-width="1.6"/>',
    'storage': '<rect x="3" y="3" width="7" height="7" fill="none" stroke="currentColor" stroke-width="1.6"/><rect x="12.5" y="3" width="7" height="7" fill="currentColor"/><rect x="22" y="3" width="7" height="7" fill="none" stroke="currentColor" stroke-width="1.6"/><rect x="3" y="12.5" width="7" height="7" fill="none" stroke="currentColor" stroke-width="1.6"/><rect x="12.5" y="12.5" width="7" height="7" fill="none" stroke="currentColor" stroke-width="1.6"/><rect x="22" y="12.5" width="7" height="7" fill="currentColor"/><rect x="3" y="22" width="7" height="7" fill="currentColor"/><rect x="12.5" y="22" width="7" height="7" fill="none" stroke="currentColor" stroke-width="1.6"/><rect x="22" y="22" width="7" height="7" fill="none" stroke="currentColor" stroke-width="1.6"/>',
}
