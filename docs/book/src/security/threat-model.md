# Threat model

Who might attack, what they can do, and what stops them, in the
architecture and in this implementation. Where an answer is "not built",
it says so.

## Adversaries and defences

| Adversary | Can try to | What stops it here |
|---|---|---|
| A censoring gateway | Drop a user's transactions | The user posts a forced intent straight to consensus; any honest proposer orders it, and the shard's validators certify it among themselves ([Censorship resistance](../concepts/censorship-resistance.md)) |
| Fewer than a third of a committee's stake | Equivocate, stall, censor | Quorums need more than two thirds, so safety holds; one honest proposer includes forced intents; equivocation is evidence, and reporting it slashes the whole stake |
| More than a third of a lane committee | Censor a shard; certify conflicting spends together with honest-looking votes | Censorship: the intent falls overdue, every certified header counts it, and the shard's committee is replaced (if the validator set is larger than the committee). Conflicting spends require equivocation by the overlap of two quorums, which leaves evidence; the offenders' stake is slashed, but only when someone reports it, and they keep their seats until the epoch ends |
| More than two thirds of a committee | Certify an invalid state transition | The World Proof for that Pulse fails, so a client that verifies proofs rejects it. A client that trusts certificates alone does not. The architecture caps value in flight at slashable stake and slashes the signers; neither is built |
| A node serving a light client | Serve another chain, substituted keys, a forged committee, a fake receipt, withheld data | The pinned chain identity; keys must derive their identifiers; committees are followed from epoch 0 with boundary proofs; receipts are proven against certified headers in the certified order; random availability samples |
| A transaction sender | Spend a coin twice; exhaust resources | Lane locks refuse the second spend; fees are charged by size; inputs, allotments, orders, formula depth and claim-reference depth are bounded. Open: large fee-paying bodies can still slow Pulses (data-availability encoding of the largest Pulse takes seconds on one core) |
| A prompt-injected AI agent | Pay an attacker, overspend | Agent Charter caveats checked by the protocol: payees, per-payment and per-window limits, expiry, evidence above a threshold; revocation stops every sub-agent ([Agent Charters](../concepts/agent-charters.md)) |
| A false claim's issuer | Get a false claim relied on | Consumers price it with the cost of corruption under their own policy and reserve its backing; an upheld challenge slashes the bond and compensates those who relied on it ([The Assurance Algebra](../concepts/assurance/index.md)) |
| The last beacon revealer | Bias the beacon by withholding its reveal | Nothing beyond visibility: a withheld reveal is seen (`impl:hash-chain-beacon`) |
| A quantum adversary | Forge signatures, decrypt payloads | Dual signatures from unrelated post-quantum families; ML-KEM-768 encryption ([Post-quantum identity](../concepts/post-quantum.md)) |
| Whoever runs a devnet | Anything | Nothing: devnet keys are public (`impl:devnet-keys`) |

## Out of scope for this implementation

- **The network.** Validators exchange messages over in-process channels
  (`impl:in-process-network`). Network-level attacks (eclipse, partition,
  flooding) are exercised only by the consensus simulator.
- **Side channels.** The post-quantum crates are unaudited, pure Rust, and
  not hardened against timing or power analysis
  (`impl:unaudited-pq-crates`).
- **Economic attacks on fees.** Fees are fixed parameters; there is no fee
  market.
- **Privacy of amounts.** Amounts and balances are public
  (`impl:public-amounts`).
- **Attacks across epochs on a real network.** The epoch hand-over relies
  on reliable delivery (`impl:reliable-handover`).

## Where the assumptions are recorded

Every trust assumption this implementation adds is in the
[Trust model](trust-model.md) and, authoritatively, in the repository's
`docs/implementation-status.md`. Findings of the reviews are in
[Reviews and fixes](reviews.md).
