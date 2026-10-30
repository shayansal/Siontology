# Memory for agents and robots

An agent's or a robot's memory on Siontology is a **memory log**: a log of
memory atoms that only its writer appends to, kept off the ledger. The
ledger keeps the log's head and moves it only forward, along a proof that
the new log extends the old one. Nothing once committed can be changed or
removed, and the ledger, not the writer, says when each atom was recorded.

That gives memory three properties that memory kept in a vendor's
database does not have:

- **It cannot be rewritten.** A writer who deletes or edits an atom it has
  committed can no longer prove that its log extends its last head.
- **Its time of recording is attested.** The Pulse of the commit that
  added an atom is the atom's transaction time. A robot cannot backdate an
  observation, and a reader can prove when it was known.
- **Answers can be proved complete.** Because the log and its index are
  committed, a reader can be given proof of everything a writer had
  recorded about a place and an interval, as of a Pulse, including proof
  that it had recorded nothing there.

## Atoms

A memory atom is a statement with provenance, after Hotgraph's Reality
Atoms: what was observed or asserted, when it held (its valid time), where
(a box in a frame, in millimetres), how confident the writer was (in
permille), what it rests on (sources, evidence spans, the model and code
that extracted it), and how it relates to other atoms (it supersedes,
contradicts, supports or was derived from them). Its payload may be public,
encrypted to the members of a memory space, or only a commitment to a
blob kept elsewhere, such as a map tile or a camera frame.

Belief is not stored in the atom. A revision is a new atom that names its
targets and a new state (accepted, disputed, superseded, retracted,
refuted); a conflict is an atom that names atoms that disagree, and both
sides stay. What a reader believes at a Pulse is computed from the
revisions visible then, under a policy the reader names: whose revisions
count, and how to prefer among them. Two readers can believe different
things about the same evidence, and each can prove why.

The design, with its index, proof sizes, costs at scale and how Hotgraph
fits, is in [Billions of agents](../research/agent-scale.md#4-memory-for-agents-and-robots).

## On the ledger (specification v1.9)

| Field of a `MemoryLog` | What it is |
|---|---|
| `writer` | The account that appends to the log and owns the object |
| `agent` | The agent identity the log belongs to, if any |
| `policy` | A commitment to the log's policy, kept off the ledger: who may read it, its retention, its encryption epochs |
| `size`, `root` | How many atoms the log holds, and the Merkle tree hash over their leaves (RFC 9162 shape) |
| `index`, `manifest` | The root of the log's index and of its storage manifest, stored but not checked |
| `commits`, `committed_at` | The head's sequence number and the Pulse of the last commit |
| `sealed` | Whether the writer has closed the log |

- **`OpenMemory { agent, policy }`** creates a log owned by the sender. For
  an agent identity, the sender must be the agent, and the agent and every
  agent above it must be live.
- **`CommitMemory { size, root, index, manifest, proof, seal }`** moves the
  log (the transaction's second input) to a new head. The ledger checks an
  RFC 9162 consistency proof from the current head to the new one, so the
  new log must start with exactly the atoms of the old. A commit at the
  same size must keep the root; it may move the index and manifest roots.
  A commit can seal the log.

A log is owned by its writer, so a commit needs a lane certificate, not
consensus: memory writes scale like payments. Two commits of the same
version of a log can never both execute, so a log cannot fork. While an
agent, or any agent above it, is suspended or revoked, its log cannot
grow: containment reaches an agent's memory as well as its money.

**Transaction time.** The atoms at positions `old size … new size − 1`
have the Pulse of the commit as their transaction time. Anyone can prove
it with two state proofs against certified headers: before that Pulse the
log's size did not cover the atom, and at it, it did.

**What the ledger does not check.** The index and manifest roots are
stored as the writer gives them. A query proof is complete relative to the
committed index. Anyone holding an atom the index leaves out can prove the
omission from the head alone (an omission proof, built in the library), but
the ledger does not yet act on such proofs, and other malformations of an
index are caught only by rebuilding it, so that the index matches the log
remains a trust atom (`impl:memory-index`).

**Where the atoms are kept.** The ledger keeps heads, not atoms. A writer
pays providers to keep its atoms with [storage deals](storage.md): it can
commit, as the head's manifest, the root of an archive of its atoms and
post a deal with that same root, so a reader can check from the ledger
alone that the memory committed is the data a provider proves it keeps,
round after round. `sion memory archive` does this for any log, with the
archive encrypted for its writer, and `sion memory restore` brings it back
from the provider, checked against the head
([Memory from the command line](../guides/memory.md)).

## Off the ledger: the `sion-memory` library

- **Atoms** are integers only (millimetres, nanoseconds, permille), encoded
  canonically and addressed by their hash, with seventeen kinds (the ten
  agent memory kinds of Hotgraph plus observation, coverage, map tile,
  pose, trajectory, transform and skill) and validation with documented
  limits.
- **The index** is a deterministic function of the log, specified in the
  library's documentation so that another implementation can rebuild it
  and compare roots: per frame, a forest of hourly Merkle R-trees for
  short-lived atoms and one for long-lived ones, plus a map of claims by
  subject and a map of links by target. A second implementation, written
  from that specification alone, agrees with it on random logs.
- **Proofs.** A range proof answers "everything this writer had recorded
  in this box during this interval, as of this head" completely, an empty
  answer included; subject and link proofs list every claim about a
  subject or every revision of an atom; an omission proof convicts a
  writer whose committed index leaves an atom out.
- **Belief** is a fold over the revisions and conflicts written by the
  revisers a reader trusts, as of a Pulse, with a bundle of proofs that
  lets the reader check the fold from heads alone. A simulation's output is
  never treated as fact.
- **Sealing and forgetting.** Payloads can be encrypted under per-atom keys
  derived from a memory space's epoch keys (ChaCha20-Poly1305), shared with
  members by wrapping the epoch key to their ML-KEM-768 keys, and forgotten
  by destroying the key; the commitment stays in the log. Members who
  received a key must destroy their copy too: forgetting relies on them.
- **The Reality Memory Protocol** (remember, recall, verify, revise,
  forget, share, audit) over all of it.

**Measured** (`cargo run --release -p sion-memory --example proof_sizes --
1000000`, one core of an Intel Xeon at 2.10 GHz, a simulated 1 km × 1 km ×
30 m warehouse over 30 days, mean of 25 queries per row, every answer
verified and checked against a brute-force scan):

| Query, over 10⁶ atoms | Results | Proof | Verify |
|---|---|---|---|
| 10 × 10 × 3 m, 1 hour | 0.2 | 15.9 KiB | 0.10 ms |
| 50 × 50 × 10 m, 1 hour | 9 | 36.0 KiB | 0.21 ms |
| 100 × 100 × 30 m, 6 hours | 169 | 154.7 KiB | 1.0 ms |
| 200 × 200 × 30 m, 1 day | 1,711 | 548.6 KiB | 4.2 ms |

Building the index over 10⁶ atoms took 0.78 s, with 325 MiB of memory at
peak.

## In this implementation

- **Built**: memory logs on the ledger, in both proof stacks (specification
  v1.9), with differential tests that run every operation and its failure
  cases, and random commits, honest and forged, through both; the
  `sion-memory` library above, with 84 tests; `sion memory open`,
  `remember`, `commit`, `recall` and `show`
  ([Memory from the command line](../guides/memory.md)); the MCP tools
  `sion_memory_open`, `sion_memory_remember`, `sion_memory_commit` and
  `sion_memory_recall`; `sion memory archive` and `restore`; for agents,
  episodes of every finished session, search over past sessions checked
  against them, and consolidation into derived notes
  ([Agents at work](../guides/agent-runtime.md#memory)); and the `robots`
  demo, which runs agent identities, memory logs and the library together
  on a devnet.
- **Simplified**: the index is rebuilt from scratch for every head (under
  a second at 10⁶ atoms, too slow for a robot checkpointing every few
  seconds at that size); a log is held in memory, about 1 KB per atom; the
  command line records claims only (revisions, conflicts, sealing and
  forgetting are in the library and the demo).
- **Not built yet**: compact fraud proofs for an index whose node summaries
  are inconsistent (a reader detects it, but only a full rebuild proves
  it); proving one field of an atom without revealing the rest;
  aggregators that commit many logs at once; curated shared world memory
  beyond the first building block (cell digests and their omission
  proofs).
