# The prover

The devnet runs one prover thread (`sion_node::prover`). It turns every
Pulse into a World Proof, verifies it, and keeps it for clients. The
concepts are in [The Fold and World Proofs](../concepts/the-fold.md); the
format is in [The World Proof format](../reference/world-proof.md).

## Where jobs come from

Validator 0 executes every Pulse with tracing
(`sion_exec::pulse::execute_pulse_traced`), which cuts execution into
chunks of 16 transactions (`--chunk`) plus the deferred phase, and records
for each chunk its statement and its witness. It sends the prover a job:

- the Pulse header it signed;
- the execution output and the chunk traces;
- the published data (transaction bodies in execution order and the posted
  forced intents);
- the lane certificate of every executed transaction, in execution order;
- the validator set as the Pulse began, with its state proof.

The job queue holds 16 Pulses. When it is full, the validator does not
wait: that Pulse goes unproven, and `proven_pulse` in `sion status` lags.

## What it does with a job

1. **Prove.** Each chunk is proven by each stack: stack A
   (`sion_prover::ReplayStack`, the reference executor in stateless mode)
   and stack B (`sion_prover::SpecStack`, the independent implementation in
   `sion-spec`). A leaf proof is the chunk's witness and transactions; both
   stacks are labelled `simulated: true`.
2. **Fold.** The statements are chained and put in a Merkle tree
   (`statements_root`); the receipts, the data and its availability root
   are bound in (`sion_prover::prove_pulse`).
3. **Check the certificates** against the lane committees derived from the
   validator set the proof carries (`sion_prover::verify_certificates`).
4. **Verify** the whole proof with both stacks
   (`sion_prover::verify_world_proof`) and check it against the header: the
   same before and after commitments, receipts root, data root and beacon.
5. **Publish** a summary and the proof's bytes. The prover keeps the last
   32 proofs in memory.

A proof that fails any step is published as an error for that Pulse, and
`GET /v1/pulses/{n}/world` returns it.

## What a client gets

`GET /v1/pulses/{n}/world` returns the summary with the proof's bytes as
hex. The summary for one Pulse of the devnet in this book:

```json
{"bytes":108046,"certificates":1,"claim":"f889f014…","data_root":"7c73b1f6…","header":"13919836…","leaves":2,"proving_ms":15,"pulse":522,"simulated":true,"stacks":["A","B"],"statements_root":"e65e1719…","transactions":1}
```

`sion verify` downloads the proof, re-verifies every leaf with both stacks,
and checks the certificates and the binding to the certified header:

```text
world proof: 2 leaves re-verified by stack ["A", "B"]; the published data is what executed and encodes to the certified data root (113 KB; replay proofs simulate succinct ones) ✓
lane certificates: all 1 executed transactions locked their inputs with a quorum of their shard's committee, taken from the proven validator set ✓
```

## Cost

A replay proof is as large as its witnesses and as expensive to check as
execution. In the benchmark on the machine used for this book, a Pulse of
512 transfers replayed in about 2.4 seconds per stack, with 1.7 MB of
witnesses; see [Benchmarks](../guides/benchmarks.md). A succinct stack
would plug into the same `ProofStack` trait and keep the statements,
chaining and bindings unchanged.

## Not built

A proving market, payments to provers, recursion, and more than one
prover. With `--no-prove` the devnet runs no prover at all.
