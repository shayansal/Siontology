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
committed index; that the index matches the log is a trust atom
(`impl:memory-index`) until fraud proofs for it are built.

## In this implementation

- **Built**: memory logs on the ledger, in both proof stacks (specification
  v1.9), with differential tests that run every operation and its failure
  cases, and random commits, honest and forged, through both.
- **Not built yet**: the library of atoms, the 4D index and its proofs,
  belief folding, encryption of memory spaces, the command-line and MCP
  interfaces, aggregators that commit many logs at once, curators of shared
  world memory, and storage with proofs.
