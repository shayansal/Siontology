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
