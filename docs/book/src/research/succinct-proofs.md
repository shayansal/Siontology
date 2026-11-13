# Succinct validity proofs: research and engineering plan

> **Status.** A plan written on 28 October 2026. Nothing in it is built.
> Proof stacks A and B are replay stacks today (atom `impl:replay-proofs`,
> see [`docs/implementation-status.md`][impl-status]). This document proposes
> how real succinct proofs replace them without giving up the rule that two
> independent stacks must both accept every chunk.
>
> A market for proofs has been built beside consensus since
> ([A market for proofs](../guides/proving.md)). It trades today's replay
> proofs and builds no part of this plan. A request in it names its proof
> system, so a succinct one can be asked for the same way.
>
> Every figure says what kind of number it is: **measured** (and by whom, on
> what hardware), **claimed** (a vendor statement we could not check), or
> **our arithmetic** (computed here from published parameters or from the
> code, not measured). We opened every web source on 28 October 2026; the
> list is at the end. Vendor figures are the vendors' own unless the text
> says otherwise.

## Summary

- **Recommendation.** Stack A: OpenVM 2.x proving the reference executor
  (`sion-exec`, stateless replay) compiled to RISC-V. Stack B: ZisK proving
  the independent implementation (`sion-spec`), configured to use BLAKE3
  inside its proofs rather than its default Poseidon-family hash. The two
  differ in field
  (BabyBear against Goldilocks), proof system (multilinear SWIRL with WHIR
  against univariate DEEP-ALI with FRI), internal hash and code lineage. Both
  are hash-based, transparent and programmed in Rust. The Ethereum
  Foundation's soundcalc rates OpenVM 2.0 at 100 bits and ZisK 0.16.1 at 128
  bits of provable security ([soundcalc]).
- **Rejected for now.** SP1 Hypercube assumes that discrete logarithms are
  hard on an elliptic curve in its memory argument ([sp1-security]), so it
  is not post-quantum. Jolt's original version commits with elliptic
  curves; Lattice Jolt rests on a lattice assumption rather than on hashes.
  RISC Zero rates
  its RISC-V prover at 96 conjectured bits ([r0-security]). Stwo has no
  production-ready way to prove Rust programs. The fastest provers of
  standard hashes (leanVM, Flock, BinarySpartan, Magnetar) are research
  code. They form the research track.
- **Signatures set the cost.** Verifying one SLH-DSA-SHA2-128f signature, the
  hash leg of today's default policy, takes about 6,308 SHA-256 compressions
  on average; the rest of a transfer takes about 234 to 304 Keccak-f
  permutations (our arithmetic, section 4.6). In a general zkVM one
  SLH-DSA-SHA2-128s signature took 3.1 s to prove on an RTX 5090 (measured
  by a third party, [btc-slh]); in a dedicated binary-field prover it took
  109 ms on a desktop CPU (measured by the author, [plonky3-pr]).
- **Order of work.** Chunk proofs without signatures, then the Fold as
  recursion, then signatures, then cost reduction, then scale. Until phase
  3 proves signatures, a new atom `impl:native-authorization` names the gap.
- **Hash choice.** Keep SHA3-256 for every consensus commitment. Choose each
  stack's internal proof hash separately, and make the two differ.
- **Gate 3 hardware.** About 470 to 1,930 Apple M4 Max-class machines for
  both stacks at 100,000 operations a second, before 2× headroom, if we make
  the signature changes of section 4.6 and hot paths prove at the best
  published per-hash rates. With today's signature policy the same model
  gives about 6,600 to 69,500 machines (our arithmetic, section 5.2).
- **1 billion operations a second** is not reachable by scaling this design:
  4.7 to 19.3 million such machines, 32 GB/s of published data and 8 to
  10 TB/s of signature ingress (our arithmetic, section 5.3).

## 1. Starting point

### 1.1 What the replay stacks do

Execution of a Pulse is cut into chunks (`sion_exec::pulse::execute_pulse_traced`).
The devnet uses 16 transactions per chunk (`prove_chunk: Some(16)` in
`crates/sion-node/src/config.rs`); `sion bench` uses 64. Each chunk carries a
`ChunkStatement` (world commitments before and after, the deferred-effects
and receipts accumulators, a digest of the steps) and a witness (every
touched object with a sparse-Merkle proof, and the knowledge-log frontier).

The `ProofStack` trait (`crates/sion-prover/src/stack.rs`) has two
operations: `prove(&ChunkTrace) -> LeafProof` and
`verify(&ChunkStatement, &LeafProof)`. Stack A (`ReplayStack`) re-executes a
chunk with `sion_exec::replay_chunk`; stack B (`SpecStack`) re-executes it
with `sion_spec::verify_chunk`, which was written from
[`docs/spec/state-transition.md`][spec] alone. A leaf proof's body is the
witness, the transactions and their effects.

`prove_pulse` (`crates/sion-prover/src/world.rs`) collects one leaf proof per
stack per statement into a `WorldProof`, together with the statements, the
receipt leaves, the published data, every lane certificate and the validator
set. `verify_world_proof` checks the chaining (`check_structure`), the data
binding including the two-dimensional Reed–Solomon encoding (`check_data`)
and every leaf under every stack. `verify_certificates` checks each
transaction's lane certificate. "Folding" is a Merkle tree over statement
digests, not recursion.

### 1.2 What a verifier pays today

Four costs grow with the Pulse. A succinct World Proof has to absorb all
four; replacing leaf proofs alone would leave the verifier linear in the
Pulse.

| Cost | Today | Source |
|---|---|---|
| Leaves | Re-executing 512 transfers took 2,375.81 ms (stack A) and 2,470.59 ms (stack B), with 1,699 KiB of witnesses | Measured, `sion bench`, 2 shared cores ([bench]) |
| Data | `check_data` re-encodes the data: extending 2,048 KiB took 235.63 ms and committing rows and columns 7,975.27 ms on one core | Measured, same run ([bench]) |
| Certificates | One `ApprovalProof` per approving validator per transaction, each with a signature over a batch header and a Merkle path | Code, `crates/sion-consensus/src/lanes.rs` |
| Signatures | A dual signature is 19,518 bytes and takes about 1.3 ms to check (202.60 µs ML-DSA-44, 1,062.20 µs SLH-DSA-SHA2-128f); checking them brings one core from 15,959 to 612 transfers a second | Measured ([bench], [pq-book]) |

### 1.3 Requirements

Hard requirements for the proof system:

1. **Post-quantum, hash-based.** Security rests on hash functions only: no
   pairings, no elliptic curves, in any part of the proof that a verifier
   relies on. Optional wrappers that use curves (for verification on
   Ethereum) are never used.
2. **Transparent setup.** No trusted setup.
3. **Rust.** The prover and verifier integrate with this Rust workspace,
   and the state transition is written in Rust.
4. **The whole state transition.** Every transaction kind of §5, the
   deferred phase of §6.3 and the chunk check of §8 of the
   [specification][spec], including signature checks, the Assurance Algebra
   and clearing certificates.

From the architecture ([arch], "The Fold" and "The proving stack"):

5. **Two independent stacks.** Different code and different arithmetizations
   prove every leaf and every level. A World Proof verifies only if both do,
   so a false statement needs a soundness bug in both. The trust formula is
   `spec ∧ sha3 ∧ ((stack_A ∧ circuit_A) ∨ (stack_B ∧ circuit_B))`.
6. **128-bit security for both stacks**, the bar the Ethereum Foundation's
   soundcalc ratings track.
7. **Cadence.** A World Proof at least every 30 seconds at Gate 3, with the
   testnet sustaining 100,000 operations a second ([arch], kill gates).

Derived requirements:

8. **Completeness is now liveness.** Both stacks must be able to prove every
   valid chunk within their memory and cycle limits. A chunk one stack cannot
   prove stops proof finality, even if it is valid.
9. **Constant verification.** Verifying a World Proof must not depend on the
   number of transactions in the Pulse.
10. **Pinned programs and keys.** A verifier must never accept a program
    commitment or verification key that comes with the proof or from a node.

## 2. Candidate proof stacks, October 2026

### 2.1 What each candidate is

| Candidate | Proof system and field | Hash inside the proofs | How programs are written | Licence |
|---|---|---|---|---|
| SP1 Hypercube (Succinct), v6.1.0 | Multilinear, Jagged PCS with FRI, LogUp-GKR; KoalaBear⁴ ([soundcalc-sp1], [l2beat-sp1]) | Poseidon2 over KoalaBear ([sp1-security]) | Rust compiled to 64-bit RISC-V ([l2beat-sp1]) | MIT or Apache-2.0 ([sp1-repo]) |
| RISC Zero zkVM | STARK provers for RISC-V and recursion ([r0-security]); field and hash not stated on the pages opened (not verified) | Not verified | Rust compiled to RV32IM ([r0-target]) | Apache-2.0 or MIT ([r0-repo]) |
| Jolt (a16z), curve version | Sum-check with lookups; Dory commitments over elliptic curves ([magnetar], [lattice-jolt]) | Not applicable | Rust compiled to RV64IMAC | MIT or Apache-2.0 ([jolt-repo]) |
| Lattice Jolt (a16z), 9 Sep 2026 | Same front end; Akita commitments based on Module-SIS ([lattice-jolt]) | Not applicable | Rust compiled to RV64IMAC | MIT or Apache-2.0 ([jolt-repo]) |
| OpenVM 2.0 (Axiom) | SWIRL (multilinear) with WHIR; BabyBear⁴ ([soundcalc-openvm]) | Poseidon2 over BabyBear for the transcript and the memory Merkle tree ([openvm-audit]) | Rust compiled to RV32IM, plus extensions for Keccak-256, SHA2-256, 256-bit integers and modular arithmetic ([openvm-repo]) | MIT or Apache-2.0, GPU prover included ([openvm-prod]) |
| ZisK (Polygon spin-off), 1.3.x-alpha | DEEP-ALI with FRI; Goldilocks³ ([soundcalc-zisk]) | A Poseidon variant by default: v1.3.0-alpha (21 September) generates verifiers for BLAKE3, Poseidon1 and Poseidon2 and calls BLAKE3-based recursion "not the default" and "available for testing" ([zisk-130]); which Poseidon is the default was not verified | Rust compiled to 64-bit RISC-V ([zisk-ethcc]), with Keccak-f, SHA-256, Poseidon2, BLAKE2s and BLAKE3 precompiles ([soundcalc-zisk], [zisk-130]) | Apache-2.0 or MIT ([zisk-repo]) |
| Plonky3 (toolkit) | Univariate and multilinear STARKs with FRI, STIR or WHIR; Mersenne31, BabyBear, KoalaBear, Goldilocks and binary towers to GF(2¹²⁸) ([plonky3-repo]) | Configurable: Poseidon2, Keccak-256, SHA-256, BLAKE3, Monolith | AIRs written by hand in Rust | Apache-2.0 or MIT |
| Stwo / S-two (StarkWare) | Circle STARK with Circle FRI over Mersenne31 ([stwo-repo]) | Blake2s by default; BLAKE3, Poseidon252 and Keccak channels | Cairo programs, or AIRs in Rust; RISC-V through the Nexus zkVM, which is experimental ([nexus-repo]) | Apache-2.0 |
| Binius64 (Irreducible) | Binary-field SNARK over 64-bit words ([binius64]) | Not verified | Circuits in Rust | Apache-2.0 or MIT |
| Airbender (Matter Labs) | AIR with DEEP-FRI over Mersenne31 ([l2beat-airbender]) | Not verified | Rust compiled to RV32IM ([airbender-docs]) | Not verified |
| leanVM (Ethereum Foundation), formerly leanMultisig | 192-bit binary tower field, WHIR, Keccak proven with GKR ([leanvm]) | Keccak-256, "a placeholder" | leanISA; a move to RISC-V (rv64im) is planned | Apache-2.0 or MIT |
| Flock (Succinct) | R1CS over GF(2), zerocheck and lincheck, Ligerito over GF(2¹²⁸) ([flock-repo]) | Standard hashes | Batches of hash computations and Merkle paths | Apache-2.0 or MIT |
| Magnetar (Microsoft Research), ePrint 2026/2423 | BinarySpartan over binary fields ([magnetar]) | SHA-256 or SHA-3 | RV64I | Not verified (code not located) |

### 2.2 Against the requirements

| Candidate | Hash-based only | Security (bits) | Final proof size | Verification time |
|---|---|---|---|---|
| SP1 Hypercube | **No.** Its memory argument assumes that discrete logarithms are hard on an elliptic curve over the degree-7 extension of KoalaBear ([sp1-security]); Shor's algorithm computes such logarithms. Its on-chain wrappers use BN254 | 100, unique-decoding regime ([soundcalc]) | 529 KiB expected, 887 KiB worst case ([soundcalc]) | Not verified |
| RISC Zero | Yes for the STARK provers, which the vendor lists as quantum safe; no for its Groth16 wrapper ([r0-security]) | 96 (RISC-V prover) and 99 (recursion), conjectured, relying on a "Toy Problem Conjecture" ([r0-security]); the README says 98 ([r0-repo]) | 218 to 454 KiB for 1 to 512 SLH-DSA signatures ([btc-slh]) | 12 to 15 ms ([btc-slh], measured by a third party) |
| Jolt, curve version | **No** (elliptic curves) | – | – | – |
| Lattice Jolt | **No**: post-quantum, but on Module-SIS, not on hashes | 128 targeted (claimed, [lattice-jolt]) | Under 100 KB (claimed) | Not stated |
| OpenVM 2.0 | Yes; its optional Halo2/KZG static verifier is not, and is not used ([openvm-audit]) | 100, mixed regimes ([soundcalc]); 128 is a stated goal ([openvm-2]) | 270 KiB ([soundcalc]); "under 300 kB" (claimed, [openvm-2]) | Not published |
| ZisK | Yes ([zisk-docs] claims "quantum resistant") | 128, Johnson-bound regime, v0.16.1 ([soundcalc]) | 269 KiB expected, 313 KiB worst case ([soundcalc]) | Not published |
| Plonky3 | Yes, with a hash-based commitment | Depends on configuration | Depends | Depends |
| Stwo | Yes | 96 plus 30 bits of proof of work as configured for Starknet ([l2beat-stwo]) | About 0.9 to 1.2 MB in the S-two book's benchmarks ([stwo-book]) | 10 to 191 ms on CPU ([stwo-book]) |
| Binius64 | Yes (binary fields) | Not verified | Not verified | Not verified |
| Airbender | Yes for the STARK; its Fflonk wrapper uses KZG over BN254 ([l2beat-airbender]) | 81 targeted for the initial release ([l2beat-airbender]) | Not verified | Not verified |
| leanVM | Yes | 128 in the random-oracle model, 64 in the quantum random-oracle model, without proximity-gap conjectures (claimed, [leanvm]) | 218 to 359 KiB ([leanvm]) | 4.1 to 12.1 ms ([leanvm]) |
| Flock | Yes | Not stated in the sources opened | 169 to 336 KiB ([plonky3-pr]) | 45 to 53 ms for XMSS batches, 3.0 to 3.5 s for SLH-DSA batches ([plonky3-pr]) |
| Magnetar | Yes (standard hashes) | 120, provable (claimed, [magnetar]) | About 600 KiB (claimed) | 11 to 13 ms (claimed) |

The Ethereum Foundation asks zkVMs for 100 bits of provable security and
proofs of at most 600 KiB by May 2026, and for 128 bits and at most 300 KiB
by early December 2026 ([ef-security], [ef-update]). soundcalc lists only
zkVMs with at least 100 bits and an expected proof below 600 KiB: OpenVM 2.0,
SP1 6.1.0, Venus 0.1.6, ZisK 0.16.1 and zkDTVM 0.8.0 ([soundcalc]). Venus
(Cysic) lists the same 44 circuits and parameters as ZisK in its report
([soundcalc-venus]), so it is not independent of ZisK (our observation). We
could not verify where zkDTVM comes from.

### 2.3 Measured and claimed throughput

| Candidate | Workload | Result | Hardware | Kind of figure |
|---|---|---|---|---|
| SP1 Hypercube | Ethereum blocks | 99.7% of 954 blocks under 12 s, 95.4% under 10 s ([sp1-16gpu]) | 16 RTX 5090 | Vendor, 18 Nov 2025 |
| OpenVM 2.0 | Ethereum blocks | p99 11.8 s, 1,000 blocks from block 24,000,000 ([openvm-2]) | 16 RTX 5090 | Vendor, 29 Jan 2026 |
| OpenVM 2.0, production release | Ethereum blocks | p99 6.3 s on 16 GPUs, 9.8 s on 8 ([openvm-prod]) | GPU model not stated | Vendor, 10 Jul 2026 |
| OpenVM 2.1 preview | Ethereum blocks | p99 9.7 s, mean 5.0 s, 7,200 blocks from block 24,000,000 ([openvm-21]) | 4 RTX 5090 | Vendor, 29 Jul 2026; "should not be used for production" |
| OpenVM 2.0 | CoreMark | 139 MHz ([openvm-2]), so 8.69 MHz per GPU (our arithmetic) | 16 RTX 5090 | Vendor, 29 Jan 2026 |
| ZisK 1.1.0-alpha | Ethereum blocks | p99 9.62 s; 99.7% under 10 s; block set and sample not disclosed ([zisk-4gpu]) | 4 RTX 5090 | Vendor claim, 18 Aug 2026 |
| RISC Zero | Ethereum blocks | 9.25 s, a projection ([r0-realtime]) | A cluster of about 120,000 USD | Vendor projection |
| RISC Zero | SLH-DSA-SHA2-128s, 512 signatures | 26 min 28 s, so about 3.1 s per signature ([btc-slh]) | 1 RTX 5090 | Third party, 13 Apr 2026 |
| Stwo | SHA-256 of 512 KiB (8,192 compressions) | 43.2 s; SP1 513.7 s, OpenVM 1,790.3 s, RISC Zero 4,673.3 s in the same run ([stwo-book]) | 24-core AMD EPYC, CPU only | Vendor, 3 Jul 2025 (older versions of each system) |
| Lattice Jolt | RISC-V cycles | Over 2 million a second on CPU, over 10 million with Apple's GPU ([lattice-jolt]) | MacBook | Vendor claim, 9 Sep 2026 |
| leanVM | Keccak-f permutations | 539,619 a second ([leanvm]) | M4 Max, 12 performance cores, no GPU | Vendor |
| leanVM | leanSPHINCS / leanXMSS aggregation | 876 / 1,191 signatures a second ([leanvm]) | Same | Vendor; neither parameter set is final or standardized |
| Flock | SHA-256 compressions | 497,900 a second at a batch of 262,144 ([flock-repo]) | M4 Max, 10 performance cores | Authors, 25 Oct 2026 |
| Flock, in a Plonky3 pull request | SLH-DSA-SHA2-128s, 128 signatures in one proof | 14.0 s, so 109 ms per signature ([plonky3-pr]) | AMD Ryzen 9 9950X3D | Author, 22 Oct 2026; not merged |
| Flock, same | XMSS with keyed BLAKE3, 1,024 signatures | 617 ms, so 0.60 ms per signature ([plonky3-pr]) | Same | Same; not a standard scheme |
| BinarySpartan | Keccak-f / SHA-256 | 287,000 / 401,000 a second, including witness generation ([binaryspartan]) | M4 Max, 12 performance cores | Author, Aug–Sep 2026 |
| Magnetar | RV64I cycles | 7.2 to 8.7 million a second ([magnetar]) | M4 Max, 12 performance cores | Author's claim, Oct 2026 |

Almost all of these are the vendors' or authors' own figures; only the
SLH-DSA benchmark on the Bitcoin development list ([btc-slh]) comes from a
third party. soundcalc, the independent source for security ratings, does not
measure speed. No source gives per-hash throughput for OpenVM's or ZisK's
hash circuits on GPUs; that is the largest unknown in section 5.2.

### 2.4 Maturity and audits

| Candidate | Status | Audits and formal verification |
|---|---|---|
| SP1 Hypercube | Recommended for production ([sp1-repo]) | Audits by Veridise, Cantina, Zellic and KALOS ([sp1-repo]); all 62 RISC-V opcodes verified against the Sail specification with Nethermind and the Ethereum Foundation (vendor, [sp1-mainnet]) |
| RISC Zero | In production use | RISC-V and recursion provers audited 31 Oct 2023, STARK-to-SNARK 20 May 2024 ([r0-security]); determinism of the Keccak circuit and "large parts" of the RISC-V circuit proven with Picus ([r0-fv]) |
| OpenVM 2.0 | "Recommended for production use" since July 2026 ([openvm-repo]) | zkSecurity audit of 2.0 in four phases from 27 Mar and 18 May 2026 ([openvm-audit]); Cantina audits of v1.7; RV32IM formally verified in Lean by Nethermind Research (Feb 2026); Keccak and SHA-2 extensions formally verified ([openvm-prod]) |
| ZisK | Alpha releases; "currently undergoing security and correctness audits" ([zisk-repo]) | "Auditing and formal verification in progress" ([zisk-docs]) |
| Stwo | "Production", runs StarkWare's shared prover for Starknet ([stwo-repo]) | Audit reports "will be linked here as they are published" ([stwo-repo]); a regression in stwo-cairo's global LogUp check let forged proofs verify until it was fixed (reported 5 Jun 2026, [stwo-bug]) |
| Plonky3 | Library | Least Authority report of 7 Nov 2024 on an earlier version: 7 issues and 6 suggestions, without severities ([plonky3-audit]); the README warns that "the verifier may panic on certain malformed proofs" ([plonky3-repo]) |
| Jolt | "In alpha and is not suitable for production use"; "has not been audited" ([jolt-repo]) | None |
| leanVM | "Not (yet) production ready"; formal verification in progress ([leanvm]) | None completed |
| Flock | The authors "do not recommend it for production use yet" ([flock-blog]) | None |
| Binius64, BinarySpartan, Magnetar | Research code or papers | None found |

### 2.5 Verdicts

- **SP1 Hypercube** fails the first hard requirement: an elliptic-curve
  discrete-logarithm assumption sits in the core proof, not only in a
  wrapper. It is otherwise among the fastest and best-audited stacks. It
  becomes a candidate again if its memory argument becomes hash-based.
- **RISC Zero** meets the hard requirements without its wrapper, but the
  vendor rates its RISC-V prover at 96 conjectured bits, below the
  architecture's 128-bit bar and not provable. It is the fallback for either
  stack.
- **Jolt.** The curve version fails. Lattice Jolt is post-quantum but rests
  on Module-SIS, not on hashes, is alpha and unaudited, and does not yet
  describe recursion. It is outside the hard requirements, and interesting
  later as a third stack whose assumption differs from both hash-based
  stacks.
- **OpenVM 2.0** meets every hard requirement, is production-recommended,
  audited and partly formally verified. Recommended for stack A.
- **ZisK** meets every hard requirement and is one of three zkVMs soundcalc
  rates at 128 bits. Of the other two, Venus shares its circuits, and we
  could not verify where zkDTVM comes from. ZisK is alpha and its audits
  are not finished. Recommended for stack B, with conditions.
- **Plonky3** is a toolkit, not a stack. It suits dedicated circuits. OpenVM
  builds its STARK backend on it ([openvm-repo]), so stack B's dedicated
  circuits should not use it.
- **Stwo** is a production prover with a standard hash, but proving the Rust
  specification would mean rewriting it in Cairo or as hand-written AIRs, or
  relying on the experimental Nexus zkVM. Not recommended now.
- **Airbender** targets 81 bits. Not suitable.
- **leanVM, Flock, BinarySpartan, Magnetar and Binius64** work over binary
  fields and prove standard hashes far faster than general zkVMs did. In
  the S-two book's CPU benchmarks of July 2025, general zkVMs proved between
  about 1.8 (RISC Zero) and 190 (S-two) SHA-256 compressions a second
  ([stwo-book], our arithmetic from 8,192 compressions); Flock proved 497,900
  a second on an Apple M4 Max in October 2026 ([flock-repo]). The hardware and
  dates differ, so only the order of magnitude means anything. These systems
  are research code in October 2026. They are the research track: dedicated
  hash provers first, and later possibly a whole stack.

## 3. Recommendation

### 3.1 Stack A: OpenVM 2.x proving the reference executor

The guest program runs `sion_exec::replay_chunk`, the code stack A runs
today, compiled to RV32IM.

1. **Hash-based throughout the STARK.** Poseidon2 for the transcript and the
   memory Merkle tree, WHIR for commitments, no curve assumption outside the
   optional static verifier ([openvm-audit]).
2. **Provable security now.** 100 bits under soundcalc, in proven regimes
   ([soundcalc]). 128 bits is OpenVM's stated goal and the Ethereum
   Foundation's December 2026 milestone ([openvm-2], [ef-update]); it is not
   yet met.
3. **Maturity.** Production-recommended since July 2026, audited by
   zkSecurity, RV32IM formally verified in Lean, and its Keccak and SHA-2
   circuits formally verified ([openvm-repo], [openvm-prod]). Those cover
   the two hash functions Siontology proves most: SHA-256 inside
   SLH-DSA-SHA2, and the Keccak-f permutation behind SHA3-256 in every
   commitment. OpenVM names its extension Keccak-256, which differs from
   SHA3-256 in padding; whether the extension serves SHA3-256 as it is, or
   needs a small change, is a phase 0 check.
4. **Extensions without forks.** New chips plug in without changing the core
   ([openvm-repo]). An ML-DSA chip (arithmetic modulo 8,380,417) and a
   chip that checks a whole signed batch of approvals fit this model.
5. **Open GPU prover** under MIT or Apache-2.0, and a final proof of
   270 KiB ([openvm-prod], [soundcalc]).

Caveats: the guest is 32-bit, so `usize` is 32 bits inside it. The
consensus crates contain 7 `as usize` casts; none of the ones read for this
plan can truncate (for example `max_pending` is a `u32`), but phase 0 checks
the guest build. OpenVM has not published its per-hash throughput on GPUs.

### 3.2 Stack B: ZisK proving the independent implementation

The guest program runs `sion_spec::verify_chunk`, compiled to 64-bit
RISC-V.

1. **A different field.** Goldilocks, `p = 2⁶⁴ − 2³² + 1`. Products of two
   ML-DSA coefficients are below 2⁴⁶, so they fit in one field element
   without splitting. `sion-da` already uses Goldilocks for its Reed–Solomon
   code (`crates/sion-da/src/field.rs`), so proving the encoding is native
   arithmetic.
2. **A different proof system.** Univariate DEEP-ALI with FRI
   ([soundcalc-zisk]), with constraints written in PIL2 ([zisk-ethcc]),
   against OpenVM's multilinear SWIRL with WHIR.
3. **128 bits** in the Johnson-bound regime under soundcalc for v0.16.1
   ([soundcalc]), which meets the architecture's bar today.
4. **A different code lineage.** ZisK's prover is the Polygon Hermez PIL2
   and proofman stack (ZisK's workspace `Cargo.toml` depends on `proofman`
   crates), rather than OpenVM's Plonky3-based backend. ZisK credits Plonky3
   among its influences ([zisk-repo]), so the phase 0 dependency audit
   checks for shared crates. Its RISC-V core is 64-bit ([zisk-ethcc]), so
   the specification's `u64` and `u128` arithmetic is cheaper than on RV32.
5. **A non-Poseidon hash inside its proofs.** BLAKE3-based recursion is
   available for testing since v1.3.0-alpha ([zisk-130]). With it, stack B
   does not share Poseidon2 with stack A.
6. **Apache-2.0 or MIT**, with Keccak-f, SHA-256, BLAKE2s, BLAKE3 and
   256-bit arithmetic precompiles.

Conditions, checked at each phase's exit: completed audits before Gate 3;
the BLAKE3 configuration supported for production and rated at 128 bits or
more by soundcalc; and the 4-GPU real-time claim reproduced on our own
workload. If ZisK misses them, RISC Zero is the interim stack B, at 96 to 99
conjectured bits and named as an atom.

### 3.3 How independent the two stacks are

| | Stack A | Stack B |
|---|---|---|
| Implementation of the specification | `sion-exec`, the reference executor | `sion-spec`, written from the specification alone |
| Field | BabyBear (31 bits), degree-4 extension | Goldilocks (64 bits), degree-3 extension |
| Proof system | Multilinear SWIRL: zerocheck and GKR-based LogUp | Univariate DEEP-ALI |
| Commitment | WHIR | FRI |
| Hash inside proofs | Poseidon2 over BabyBear | BLAKE3 (to be confirmed for production) |
| Prover code lineage | Axiom, on Plonky3 | Polygon Hermez PIL2 and proofman |
| ISA | RV32IM | 64-bit RISC-V |
| Signature verification in the guest | `sion-crypto` (`fips204`, `fips205` crates) | An independent implementation, to be chosen in phase 3 |

What remains common to both, and what to do about it:

- **The specification** (atom `spec`). Two implementations of one document
  share its mistakes. The differential tests keep running.
- **SHA3-256 and Borsh** (atom `sha3`). Every commitment uses them.
- **The Rust compiler and core library.** rustc and LLVM build both guests
  for RISC-V. Every corpus chunk runs natively and in both guests, and we
  build stack B with a different compiler release.
- **Shared primitives** (atom `impl:shared-primitives`). Both stacks call
  `sion-core` for hashing and Merkle trees and `sion-crypto` for signatures.
  Stack B should move to independent implementations: its own SHA3 sponge
  over ZisK's Keccak-f precompile, its own sparse-Merkle and Merkle-log
  verification written from a prose specification of `sion_core::smt` and
  `sion_core::merkle`, and ML-DSA and SLH-DSA verifiers from another code
  base than `fips204` and `fips205`.

### 3.4 Research track

- **A binary-field, standard-hash zkVM** as a replacement for one stack or as
  a third stack: Magnetar, leanVM once it runs RISC-V, or Succinct's work on
  folding Flock into SP1 ([flock-blog]). It would share neither the field
  characteristic nor the hash family with stack A, and it proves SHA3 and
  SHA-256 cheaply. Evaluated at the phase 4 decision gate.
- **Dedicated binary-field hash provers** for the hottest paths (SMT updates,
  SLH-DSA, lane certificates), composed with each stack's zkVM proofs. Same
  gate.
- **Lattice-based proofs** (Lattice Jolt, Aerie) for assumption diversity.
  They fall outside the hash-based requirement and would change the trust
  formula, so they stay research.

## 4. Integration

### 4.1 Leaf proofs behind the `ProofStack` trait

The trait does not need to change for leaves.

- **Backends.** Two crates, for example `sion-prover-openvm` and
  `sion-prover-zisk`, each implementing `ProofStack` with
  `simulated() = false`, and two guest crates: the stack A guest calls
  `sion_exec::replay_chunk`, the stack B guest calls
  `sion_spec::verify_chunk`.
- **Guest input and output.** The input is the Borsh encoding of the
  `ChunkTrace`: statement, transactions with their signatures, deferred
  items and witness. The guest evaluates §8 of the specification and
  commits one public value, `ChunkStatement::digest()`. A chunk that fails
  produces no proof.
- **`LeafProof`.** Unchanged: `stack`, `statement`, `body`. The body becomes
  the Borsh encoding of (backend, backend version, program commitment, proof
  bytes).
- **`verify`.** Checks the backend and version, checks that the program
  commitment equals the one pinned for the chain, verifies the proof, and
  checks that its public value equals `statement.digest()`.
- **Pinned programs.** The two program commitments join the chain
  configuration (and therefore `ChainConfig::digest()` and the chain
  identity), so changing a guest is a governed upgrade. ZisK v1.3.0-alpha
  fixed exactly this failure mode: it "bound verification to trusted setup
  and program verification keys rather than keys supplied by the proof"
  ([zisk-130]).
- **Atoms.** `stack_A` expands into OpenVM's soundness, `poseidon2-babybear`,
  OpenVM's circuits, the guest program and the compiler; `stack_B` likewise.
- **Oracles.** `ReplayStack` and `SpecStack` stay as test oracles.
- **Resource limits.** The node runs validators with 64 MiB thread stacks
  because recursion depth in the Assurance Algebra matters
  ([impl-status], reviews). Each zkVM bounds its guest's stack and memory,
  so phase 0 shows that the specification's depth limits (formulas 32 and
  64 levels, claim chains 16) fit in each guest.
- **Hostile proofs.** Verification runs isolated (in a separate process, or
  behind `catch_unwind`) so a malformed proof cannot crash a node: Plonky3
  warns its verifier may panic on some malformed proofs ([plonky3-repo]).

### 4.2 Authorization as a separate commitment

Signature checks happen inside the state transition in four places: prepare
step 5 (§4.1), forced-intent admissibility (§6.3, step 3), validator
registration (§5.26) and equivocation evidence (§5.30). Lane certificates
and Pulse headers add validator signatures outside §8.

Until phase 3 proves signatures, each leaf guest records every check
as an entry (signer key digest, message digest, signature digest, outcome)
and commits the Merkle root of the entries, `auth_root`, next to the
statement digest. A separate authorization proof shows that every entry's
outcome is what the verification algorithm returns; in phases 1 and 2 a
native check over a side file of signatures stands in for that proof. Each
stack's aggregation checks that both commit the same `auth_root` and passes
on only the statement digest, so the statement a light client checks is
still exactly §8.

The authorization proof must prove both outcomes, not only acceptance. A
prover that could call a valid signature invalid would make a valid
transaction look rejected, which is why `sion_exec::stateless` rejects a
whole chunk on a witness miss instead of failing one transaction.

New atom until phase 3: `impl:native-authorization`, "verifiers re-check the
signatures behind a World Proof natively, from a side file; no proof covers
them".

### 4.3 The Fold as recursion

- **Node proofs.** Each stack builds its own binary tree. A node verifies two
  child proofs and checks what `check_structure` checks between neighbours
  today: `after` and `before`, both accumulators, the beacon, the indices.
- **Root proof.** It also checks that the first index is 0, that only the
  last statement is the deferred phase, that the accumulators start at zero
  and that the commitments are for consecutive Pulses. Its public value is
  `WorldProof::claim_digest()`: Pulse, before, after, receipts root,
  statements root and data root.
- **Data leaf.** It derives the transaction digests from the published data,
  checks every chunk's `steps` and the posted forced intents (§9, item 5),
  and recomputes the Reed–Solomon encoding and its row and column roots, so
  the data root is bound inside the proof. Phase 5 replaces the recomputation
  with a ZODA-style check, in which sampled rows and columns serve as their
  own proofs of correct encoding ([zoda]).
- **Certificate leaves.** They verify each signed lane batch once (the
  header signature and the batch's whole Merkle tree) and match approvals to
  executed transactions with lookups. That costs about 2 hashes per approving
  validator per transaction, against 1 + log₂(batch size) when the proof
  checks each approval's path separately (our arithmetic).
- **World Proof v2.** For light clients, a constant-size object: the fields a
  light client checks against the certified header, and one root proof per
  stack. Data, witnesses, certificates and statements become prover inputs.
  The replay `WorldProof` stays for tests.
- **Two trees, never mixed.** No proof of one stack verifies a proof of the
  other.
- **History.** In phase 6 the root for Pulse t also verifies the root for
  Pulse t − 1, so a light client needs only the chain identity and the latest
  pair.
- **Pipelining.** Leaves of Pulse t + 1 start while the tree of Pulse t
  finishes.

### 4.4 Chunk sizes

Cut chunks by a proving-cost budget, not by a count of transactions. Start
at 256 simple transfers per chunk and tune between 256 and 1,024 by
measurement. Heavy transactions get chunks of their own: verifying the
clearing certificate of a 2,000-order batch took 187.59 ms natively
([bench]).

The arithmetic behind the range (ours):

- **Leaves per Pulse.** At 100,000 operations a second and a Pulse of 10 s
  (the launch target is 10 to 30 s), a Pulse holds 1,000,000 operations:
  3,907 chunks of 256 or 977 of 1,024 per stack, so 12 or 10 levels of
  folding.
- **Folding work.** That is about 390 or 98 node proofs a second per stack.
  At 0.303 s per 2-to-1 recursion (leanVM, measured by the vendor on an M4
  Max, [leanvm]) they need about 118 or 30 machines per stack.
- **Shared tree paths.** A chunk touching m keys shares the top log₂(m)
  levels of the state tree. With tombstones kept, a year at 100,000
  operations a second adds about 6.3 × 10¹² objects (2 per transfer), a tree
  depth of about 42.5. For 1,024 transfers (about 4,096 keys) the per-key
  path drops to about 30.5 levels, about 28% less sparse-Merkle hashing.
- **Latency.** The work inside a chunk (state transition and user
  authorization) comes to 1.04 to 6.74 ms per operation per stack in section
  5.2's model, so a leaf of 1,024 operations takes about 1.1 to 6.9 s on one
  machine, and one of 256 takes about 0.3 to 1.7 s.

### 4.5 What moves into proofs, in order

1. **The chunk state transition**, with signature checks as committed hints
   (section 4.2). This replaces the most code at once.
2. **Chaining and receipts**, as recursion (section 4.3). After this step,
   the World Proof carries one root proof per stack instead of one leaf
   proof per chunk.
3. **The data binding** (digests, steps, posted intents) and the
   Reed–Solomon encoding.
4. **Lane certificates**, with validator signatures as hints first.
5. **The ML-DSA-44 leg**: about 100 Keccak-f permutations and 13
   number-theoretic transforms per verification, the cheaper leg (our
   arithmetic from FIPS 204, section 4.6).
6. **The SLH-DSA leg**, which needs 21 to 63 times as many hash calls as
   ML-DSA, depending on the parameter set (2,122 to 6,308 against 100).
7. **Validator signatures** on lane batches and Pulse headers.
8. **History**, so the proof covers every committee change from genesis.

### 4.6 Signature verification cost

Work per verification, our arithmetic from FIPS 204 Table 1, FIPS 205 Table
2 and SP 800-230 ipd Table 1 ([fips204], [fips205], [sp800-230]). It counts
SHA-256 compressions or Keccak-f permutations, with the chain's signing
context of 75 bytes:

| Scheme | Signature (bytes) | Hash calls per verification, average (worst) | Other work |
|---|---|---|---|
| ML-DSA-44 | 2,420 | 100 Keccak-f: 10 for `tr`, 80 for expanding the matrix, 10 more; 10 for a repeat signer if the prover caches `tr` and the matrix | 13 transforms of 256 points, about 19,500 multiplications modulo 8,380,417 |
| SLH-DSA-SHA2-128f (today's default) | 17,088 | 6,308 SHA-256 (12,083) | – |
| SLH-DSA-SHAKE-128f | 17,088 | 6,189 Keccak-f (11,964) | – |
| SLH-DSA-SHA2-128s | 7,856 | 2,162 SHA-256 (3,999) | – |
| SLH-DSA-SHAKE-128s | 7,856 | 2,122 Keccak-f (3,960) | – |
| SLH-DSA-SHA2-128-24 / SHAKE-128-24 (draft) | 3,856 | 299 SHA-256 (401) / 287 Keccak-f (389) | At most 2²⁴ signatures per key |
| The rest of a transfer | – | About 234 Keccak-f at tree depth 30, 304 at depth 40 | – |

The transfer estimate counts, from the code: seven state-tree paths (four
witness proofs and three updated paths), the account object's digest (its
policy holds the 1,312-byte ML-DSA key, so 11 Keccak-f), four coin digests,
the transaction digest, two derived identifiers, the effects leaf and two
accumulators. Phase 0 measures it.

Measured end-to-end costs of proving signature verification:

| Scheme | Prover | Result | Hardware | Source |
|---|---|---|---|---|
| SLH-DSA-SHA2-128s | RISC Zero, SHA-256 precompile | 3.1 s per signature; 454 KiB for 512 | RTX 5090 | [btc-slh] |
| SLH-DSA-SHA2-128s | Flock | 109 ms per signature at 128 per proof; 336 KiB; 3.2 s to verify | Ryzen 9 9950X3D | [plonky3-pr] |
| leanSPHINCS (Keccak; not standard) | leanVM | 876 signatures a second; 330.8 KiB; 7.46 ms to verify | M4 Max CPU | [leanvm] |
| leanXMSS (stateful; not standard) | leanVM | 1,191 signatures a second; 358.5 KiB; 7.43 ms to verify | M4 Max CPU | [leanvm] |
| XMSS with keyed BLAKE3 (not standard) | Flock | 0.60 ms per signature at 1,024 per proof; 304 KiB; 53 ms to verify | Ryzen 9 9950X3D | [plonky3-pr] |
| ML-DSA-44 | Hekate (binary towers) | 318 ms per proof, batch size not stated; 1,341 KiB; 100-bit soundness; AGPL-3.0 | M3 Max | [hekate] |
| Falcon-512 | Aerie, a lattice-based SNARK | 32,768 signatures in 0.65 s; 54 ms to verify | M4 Max | [aerie] |

What the numbers say:

- **The hash leg dominates.** 6,308 SHA-256 compressions for one
  SLH-DSA-SHA2-128f verification, against 234 to 304 Keccak-f for the rest
  of a transfer.
- **Generic zkVM code is the wrong place for it.** 3.1 s per signature on a
  GPU means 310,000 RTX 5090s per stack for 100,000 signatures a second (our
  arithmetic).
- **Parameter design matters more than the prover.** The measured dedicated
  provers differ by a factor of about 100 (109 ms against about 1 ms per
  signature). leanVM's step counts imply about 268 Keccak calls per
  leanSPHINCS verification and 186 per leanXMSS verification (our
  arithmetic from [leanvm]), against 2,122 to 6,308 for the FIPS 205 sets.
  The same counts show leanVM running at about 2.3 times its pure Keccak
  cost per signature, which section 5.2 uses as an engineering target.
- **ML-DSA has no usable measurement in a hash-based prover.** Hekate does
  not state its batch size, and noir-dilithium ([noir-dilithium]) uses an
  elliptic-curve backend. Its one useful number is a ratio: caching the
  prepared key cut ML-DSA-65 verification from 3,869,436 to 577,843 gates.
  ML-DSA proving cost is the first measurement of phase 3.

Decisions:

1. **Dedicated circuits in each stack**: an OpenVM extension for stack A,
   ZisK precompiles and PIL2 components for stack B. Never generic guest
   code.
2. **SLH-DSA-128s by default for new accounts**, with 128f kept for accounts
   that sign often. 128s needs 2.9 times fewer hash calls and 54% fewer
   bytes, and signing takes 335 ms instead of 16 ms (measured, [bench]),
   which a person paying by hand does not notice.
3. **Batch authorization** (a specification change). A sender signs one
   Merkle root over up to N of its transaction digests; each transaction
   carries its leaf index and path. Per transaction the proof costs log₂ N
   hashes plus 1/N of a dual verification, and the wire carries
   19,518/N + 32·log₂ N bytes of authorization. It suits agents and services
   that pay often (x402, Agent Charters).
4. **Stateful hash-based keys for validators.** The architecture already
   plans XMSS^MT keys for validators and busy agents ([arch]); NIST SP
   800-208 standardizes XMSS^MT and LMS ([sp800-208]). Validators already
   sign one batch root per batch.
   Aggregating these signatures is what an information brief prepared for
   the Ethereum Foundation called the real blocker for post-quantum
   Ethereum in February 2026 ([magicians]).
5. **Cache ML-DSA's `tr` and expanded matrix per account within a chunk**:
   100 Keccak-f become 10 for a repeat signer.
6. **Not SP 800-230's 128-24 sets for wallets.** They verify with about 290
   hash calls, but signing must compute six FORS trees of 2²⁴ leaves each,
   about 10⁸ leaves per signature (our arithmetic), and NIST does not approve
   them for general-purpose use ([sp800-230]). They may suit keys that sign
   rarely once the standard is final.
7. **Lattice-native aggregation for the ML-DSA leg** is research. Aerie
   aggregates Falcon, not ML-DSA, with a lattice-based SNARK whose soundness
   is proven in the classical random-oracle model ([aerie]).
8. **Phase 3 measurements in both stacks decide between SHA2 and SHAKE for
   the hash leg.** Flock proves SHA-256 compressions faster than Keccak
   permutations (42,000 against 30,000 per core, [flock-paper]); SHA2 also
   keeps the authorization formula `module-lattice ∨ sha2-256`
   ([pq-book]) apart from the SHAKE inside ML-DSA. Keep SHA2 unless the
   measurement says otherwise.

### 4.7 Hash choice inside proofs and in consensus

Two hashes are in play. The **consensus hash** builds every commitment,
identifier and signed message: SHA3-256 with a domain tag
(`crates/sion-core/src/hash.rs`). Each proof system's **internal hash**
builds its Merkle commitments and its Fiat–Shamir challenges.

Keep SHA3-256 for consensus:

1. **It is neutral between fields.** A field-native hash such as Poseidon2
   over BabyBear, KoalaBear or Goldilocks would tie consensus to one stack's
   field, and the other stack would have to emulate a foreign field. Stack
   independence needs different fields.
2. **The proving penalty has shrunk.** Binary-field provers prove 287,000 to
   539,619 Keccak-f permutations a second on a laptop CPU
   ([binaryspartan], [leanvm]; authors' measurements).
3. **Poseidon2 is under active analysis.** An attack paper of February 2026
   improved algebraic collision attacks on a recommended 128-bit Poseidon2
   parameter set by a factor of 2¹⁰⁶, while stating that the claimed
   security level still holds ([poseidon2-attack]). In August 2026 the
   Ethereum Foundation dropped Poseidon for future layer-1 designs in favour
   of SHA or BLAKE, citing gains in proof systems rather than a break
   ([ef-poseidon]).
4. **Changing it would start a new chain.** Every account identifier (the
   hash of a genesis policy), object identifier, transaction digest and claim
   identifier, every root and every signed message would change, and bytes
   would need a canonical packing into field elements.

An option, not a recommendation: TurboSHAKE128 uses 12 rounds of
`Keccak-p[1600]` instead of the 24 of SHA-3 ([rfc9861]), which should roughly
halve the proving cost of each state-tree node (our inference from the round
count). It is an informational RFC, not a NIST standard, and adopting it is a
consensus change.

Inside the proofs, each stack uses its own hash, and the two differ: stack A
Poseidon2 over BabyBear (OpenVM's default), stack B BLAKE3 (ZisK's
alternative configuration). Each is an atom of its stack. Two cautions apply
to both:

- **Fiat–Shamir.** Practical attacks exist on Fiat–Shamir-compiled GKR
  protocols when the circuit can compute the hash ([fs-attack]). SWIRL uses
  GKR-based LogUp; SP1 guards against the attack by keeping the proven
  circuit's depth below the hash's ([sp1-security]). Each stack's recursion
  soundness argument, which the Ethereum Foundation's December milestone
  requires ([ef-security]), must cover it.
- **Quantum adversaries.** soundcalc counts classical bits. Succinct
  arguments built from interactive oracle proofs are secure in the quantum
  random-oracle model ([qrom]), but with lower concrete bounds: leanVM
  states 64 quantum bits for its 128 classical bits ([leanvm]).

### 4.8 Changes to the specification and the code

Specification (v1.6 and later):

- Pin the program commitments and proof-system versions in the chain
  configuration, with an upgrade rule.
- Add batch authorization, make SLH-DSA-128s the default for new accounts,
  and add SHAKE schemes to `Scheme` if phase 3 chooses them.
- Account for proving cost. Fees today are a base fee plus a fee per KiB
  ([impl-status]); they say nothing about proving work. Each transaction gets
  a proving-cost bound and a fee term, so fees pay for proofs and chunks
  stay bounded.
- Write the hashing, Merkle-log and sparse-Merkle rules of `sion-core` in
  prose, so that stack B can implement them independently.
- Split large objects. Every touch of the assurance registry, the schedule
  or the validator set hashes the whole object, and the registry's witness
  grows with the registry ([impl-status]). Every deferred phase loads the
  schedule (§6.3).

Code:

- `sion-prover`: leave `ProofStack` as it is for leaves; add a folding trait
  (fold two proofs, verify a node), World Proof v2 and the backend crates.
- `scripts/check-independence.sh`: extend to the stack B guest and its
  dependency tree.
- Differential tests: run both guests in each zkVM's execute-only mode on the
  whole corpus in CI, and prove a sample nightly on prover hardware.
- `sion bench`: count Keccak-f permutations, SHA-256 compressions, signature
  checks and witness bytes per chunk and per transaction kind.
- `docs/implementation-status.md`: add `impl:native-authorization`; retire
  `impl:replay-proofs` only when both stacks are succinct and World Proof
  verification is constant-size.

## 5. Plan

### 5.1 Phases, milestones and exit criteria

Months count from the start of the work. Phases overlap. Phase 5 must end by
Gate 3.

| Phase | Months | Deliverables | Exit criteria (measured) |
|---|---|---|---|
| 0. Measure and prepare | 0–2 | Proving-cost counters in both executors; both guests built for their targets; dependency audit of both stacks; design note for the folding trait and World Proof v2 | A cost table for every transaction kind (median and worst case: Keccak-f, SHA-256 compressions, signature checks, witness bytes) in the repository. Both guests run the whole differential corpus in each zkVM's executor with no divergence from native execution. A list of crates the two stacks share |
| 1. Leaves without signatures | 2–8 | Backend crates; `auth_root` and the side file; pinned program commitments | Both stacks prove every chunk of the differential corpus and refuse every tampered trace in it. Native A, native B, guest A and guest B agree on every chunk. Proving time and proof size for a 512-transfer Pulse, on one RTX 5090 per stack, published in `benchmarks.md` |
| 2. The Fold | 6–12 | Aggregation trees; data leaf; certificate leaves; World Proof v2 | World Proof size and verification time do not grow with the Pulse, measured at 10³, 10⁵ and 10⁶ transfers; targets of 2 × 300 KiB and 100 ms on one desktop core |
| 3. Signatures in proofs | 9–18 | ML-DSA-44 and SLH-DSA circuits in both stacks, stack B's from independent code; validator signatures proven | Every NIST ACVP signature-verification vector for ML-DSA-44 and the SLH-DSA sets in use gives the same answer in each circuit as natively. No signature bytes in the World Proof. `impl:native-authorization` retired. Proving cost per signature measured on each device |
| 4. Cost reduction and decision gate | 12–24 | SLH-DSA-128s default; batch authorization; XMSS^MT or LMS validator keys; proving-cost fees; stack B on BLAKE3; evaluation of binary-field hash provers | At most 10 ms of one reference machine per stack for a dual-signed transfer with batches of 8, measured, the top of section 5.2's range. soundcalc rates both configurations at 128 bits or more. A recorded decision on binary-field hash provers: adopt them where they are at least 5 times cheaper at equal security |
| 5. Scale to Gate 3 | 20–36 | Distributed proving; prover fleet; proving-market jobs; the encoding check inside proofs | 24 hours at 100,000 operations a second with a World Proof at least every 30 s and no disagreement between stacks. Fleet size and power within section 5.2. Two independent audits per stack with no open critical finding |
| 6. History and phones | 30–42 | Recursion across Pulses; a phone verifier | A phone verifies the latest World Proof pair from the chain identity alone in under 1 s, the architecture's target |

### 5.2 Proving hardware for 100,000 operations a second

**Unit.** One "machine" is an Apple M4 Max-class CPU with 10 to 12
performance cores, because the best published per-hash proving rates come
from one: 287,000 Keccak-f a second (BinarySpartan, [binaryspartan]),
539,619 Keccak-f a second (leanVM, [leanvm]) and 497,900 SHA-256
compressions a second (Flock, [flock-repo]). A Mac Studio with this chip
starts at 1,999 USD ([mac-price]) and draws at most 145 W ([mac-power]).
Neither recommended stack has published per-hash rates for its own circuits;
phase 1 measures them, and phase 4 decides whether to bring in binary-field
hash provers. This is the main uncertainty in the estimate.

**Assumptions.** Two shards; committees of 100 seats, so 67 approvals per
transaction; one lane batch per validator every 50 ms; state-tree depth 30
(low) to 40 (high); state-transition work 2 (low) to 4 (high) times its pure
hash cost; ML-DSA-44 at 0.5 to 10 ms per verification (unmeasured: 100
Keccak-f alone take 0.19 to 0.35 ms at the rates above); hash leg at 0.84 ms
(leanXMSS-class keys, low) to 10 ms (SLH-DSA-SHA2-128s at 2.3 times its pure
hash cost, high); batch authorization with 8 transactions per signature;
chunks of 1,024; 3.2 MB/s of published data, the architecture's Gate 3
figure for public transfers ([arch]).

Machines per stack, our arithmetic:

| Component | Low | High | Arithmetic (low; high) |
|---|---|---|---|
| State transition | 87 | 424 | 100,000 × 234 × 2 ÷ 539,619; 100,000 × 304 × 4 ÷ 287,000 |
| Lane certificates | 50 | 93 | 100,000 × 2 × 67 × 2 ÷ 539,619; same ÷ 287,000 |
| User authorization | 17 | 250 | 100,000 × (0.5 + 0.84) ms ÷ 8; 100,000 × (10 + 10) ms ÷ 8 |
| Validator signatures | 5 | 80 | 2 × 100 × 20 = 4,000 a second × (0.5 + 0.84) ms; × (10 + 10) ms |
| Encoding check | 47 | 89 | 3.2 MB/s ÷ 2 MiB = 1.53 squares a second × 16,777,216 hashes = 25.6 million a second, ÷ 539,619; ÷ 287,000 |
| Recursion | 30 | 30 | 98 nodes a second × 0.303 s |
| **Per stack** | **236** | **966** | |
| **Both stacks** | **471** | **1,932** | |

With 2× headroom for peaks and for redundant provers that is 943 to 3,863
machines, 1.9 to 7.7 million USD at 1,999 USD each, and 0.14 to 0.56 MW at
145 W each. The architecture's planning range for the whole network build,
audits included, is 15 to 25 million USD ([arch]).

**With today's signature policy** (SLH-DSA-SHA2-128f on every transaction
and every validator vote, no batching), user authorization costs 29.1 ms (the
pure hash cost of 6,308 compressions at 497,900 a second, times 2.3) to
318 ms (Flock's measured 109 ms for 128s, scaled by 6,308 ÷ 2,162) per
transaction, plus 0.5 to 10 ms for ML-DSA. The same model then gives 3,296 to
34,751 machines per stack, 6,592 to 69,501 for both: 26 to 278 million USD
and 1.9 to 20 MW with 2× headroom. This is why section 4.6's changes come
before scale.

**A general zkVM for the state transition alone**, without dedicated
circuits: at 8.69 MHz per RTX 5090 (OpenVM 2.0 on CoreMark, [openvm-2]) and
an assumed 250,000 to 600,000 cycles per transfer, hash precompiles included
(to be measured in phase 0), that is 2,878 to 6,906 GPUs per stack, or 5,756
to 13,812 for both: 11.5 to 27.6 million USD at 1,999 USD each
([rtx5090-price]) and 3.3 to 7.9 MW of board power at 575 W each
([rtx5090]).

### 5.3 What 1 billion operations a second would need

Our arithmetic, from the same per-operation costs:

- **Proving.** 2.36 to 9.66 ms of machine time per operation per stack gives
  10⁹ × 2 × (2.36 to 9.66) ms = 4.7 to 19.3 million machines, 0.68 to
  2.80 GW at 145 W and 9.4 to 38.6 billion USD at 1,999 USD. Fitting 10⁹
  operations a second into 20,000 machines needs about 10 µs per operation
  per stack, 236 to 966 times below the Gate 3 estimate.
- **Data availability.** 10⁹ × 32 bytes per public transfer ([arch]) is
  32 GB/s, 627 times the architecture's maturity figure of 51 MB/s.
- **Signature ingress.** 10⁹ × 8 to 10 KB of sender signatures ([arch]) is 8
  to 10 TB/s.
- **State growth.** With tombstones kept (rule 4 of `AGENTS.md`), two new
  objects per transfer add 6.3 × 10¹⁶ objects a year: a tree depth of about
  56 and about 9.5 EB a year at an assumed 150 bytes per object.

So 10⁹ ledger operations a second is not a matter of buying more provers. It
needs four things at once: batch claims, where one ledger operation commits
to many statements (the architecture's batch statements, which are not
bounded by the ledger); authorization amortized over a thousand or more
statements per signature; hardware for binary-field hashing that gains two
to three orders of magnitude per hash; and a state commitment whose proving
cost does not grow with every tombstone ever written, which conflicts with
rule 4 as written and is an open design question.

## 6. Risks and open problems

**Risks**

1. **Signature aggregation is not solved at the rate we need.** The fastest
   measured schemes are not standard (leanXMSS, leanSPHINCS, XMSS with keyed
   BLAKE3). For FIPS 205 the best measurement is 109 ms per 128s signature,
   from research code. ML-DSA has no measurement in a hash-based prover.
2. **The recommended stacks' hash rates are unknown.** If OpenVM's and ZisK's
   Keccak and SHA-256 circuits prove much more slowly than binary-field
   provers, the fleet grows, or each stack gains a second proof system for
   hashing, which doubles the audit surface.
3. **ZisK is alpha.** Its audits are not finished, and its 4-GPU real-time
   result is a claim with an undisclosed block set.
4. **Common-mode failures.** The specification, SHA3-256, Borsh, rustc and
   LLVM, and any crate both stacks' dependency trees share. A Poseidon hash
   joins the list if stack B keeps ZisK's default configuration.
5. **Soundness bugs in proof systems.** In June 2026 a value left out of
   stwo-cairo's Fiat–Shamir transcript after a refactor allowed forged
   proofs until fixed ([stwo-bug]). Our guest programs can have soundness
   bugs of their own: a guest that accepts an invalid chunk is a circuit bug.
6. **Liveness through completeness.** A valid chunk one stack cannot prove
   (memory, cycles, a guest panic, a 32-bit difference) stops proof
   finality.
7. **Witnesses are not public.** Signatures and witnesses are never
   published, so provers depend on validators to hand them over. A committee
   that withholds them stops proofs; the cap on value in flight bounds the
   damage ([arch]).
8. **Proving work is not priced.** Without a proving-cost bound, a
   transaction that is cheap in bytes can be expensive to prove, which is a
   denial-of-service path.
9. **Large objects and state growth** make per-operation proving cost grow
   with the state: about 0.95 PB of new objects a year at 100,000 operations
   a second and 150 bytes per object (our arithmetic).
10. **Thin security margins.** OpenVM is at 100 provable bits today, and
    quantum bounds are lower than classical ones. Both stacks must stay in
    the regimes soundcalc counts as provable (unique decoding or the Johnson
    bound) and never rely on proximity-gap conjectures beyond them.
11. **Certificate checks grow with committee size.** At 301 seats, as the
    architecture plans for maturity, a transaction carries 201 approvals,
    three times the Gate 3 assumption above.
12. **Lattice proofs are faster in places** (Aerie, Lattice Jolt) but fall
    outside the hash-based requirement. Adopting them changes the trust
    formula and needs its own decision.

**Open research problems**

- Aggregating FIPS 204 and FIPS 205 signatures efficiently inside hash-based
  proofs, the problem the architecture already lists ([arch]).
- 128 provable classical bits and an adequate quantum bound for recursive
  STARKs over small fields, at proof sizes under 300 KiB.
- Proving correct two-dimensional Reed–Solomon encoding cheaply inside the
  World Proof (ZODA-style designs claim near-zero overhead, [zoda], without
  published measurements).
- State commitments that keep "nothing is deleted" auditable while keeping
  proving cost bounded.
- Recursion between binary-field and prime-field proofs, if the project
  adopts dedicated hash provers.
- End-to-end formal verification of the World Proof circuit, as the
  architecture lists.

## Sources

Opened on 28 October 2026. Figures are the publishers' own unless the text
says otherwise.

**This repository:** [implementation status][impl-status] ·
[architecture paper][arch] · [benchmarks][bench] · [specification][spec] ·
[post-quantum identity][pq-book] · `crates/sion-prover/src/{stack,world,replay,spec}.rs` ·
`crates/sion-exec/src/{pulse,stateless,tx}.rs` · `crates/sion-consensus/src/lanes.rs` ·
`crates/sion-crypto/src/{keys,policy,dual}.rs` · `crates/sion-core/src/{hash,smt}.rs` ·
`crates/sion-node/src/{config,prover}.rs` · `crates/sion-da/src/field.rs`

**Security ratings and requirements:** [soundcalc summary][soundcalc] ·
[soundcalc SP1][soundcalc-sp1] · [soundcalc OpenVM2][soundcalc-openvm] ·
[soundcalc ZisK][soundcalc-zisk] · [soundcalc Venus][soundcalc-venus] ·
[Ethereum Foundation, zkEVM security foundations, 18 Dec 2025][ef-security] ·
[Ethereum Foundation, cryptography research update, 11 Feb 2026][ef-update] ·
[Ethereum Magicians, the real blocker is aggregation, 27 Feb 2026][magicians]

**zkVMs:** [SP1 security model][sp1-security] ·
[SP1 Hypercube, 20 May 2025][sp1-hypercube] · [SP1, 16 GPUs, 18 Nov 2025][sp1-16gpu] ·
[SP1 Hypercube on mainnet, 19 Feb 2026][sp1-mainnet] · [SP1 repository][sp1-repo] ·
[L2BEAT, SP1 Hypercube][l2beat-sp1] · [RISC Zero security model][r0-security] ·
[RISC Zero repository][r0-repo] · [Rust target riscv32im-risc0-zkvm-elf][r0-target] ·
[RISC Zero, real-time proving][r0-realtime] ·
[RISC Zero, formal verification][r0-fv] · [Jolt repository][jolt-repo] ·
[Lattice Jolt, 9 Sep 2026][lattice-jolt] · [OpenVM repository][openvm-repo] ·
[OpenVM 2.0, 29 Jan 2026][openvm-2] · [OpenVM 2.0 in production, 10 Jul 2026][openvm-prod] ·
[OpenVM 2.1 preview, 29 Jul 2026][openvm-21] · [zkSecurity audit of OpenVM 2.0][openvm-audit] ·
[ZisK repository][zisk-repo] · [ZisK v1.3.0-alpha][zisk-130] · [ZisK documentation][zisk-docs] ·
[ZisK at EthCC, talk digest][zisk-ethcc] ·
[CryptoSlate on ZisK, 19 Aug 2026][zisk-4gpu] · [Stwo repository][stwo-repo] ·
[L2BEAT, Stwo][l2beat-stwo] · [S-two book benchmarks, 3 Jul 2025][stwo-book] ·
[Axiom on the stwo-cairo bug, 27 Jul 2026][stwo-bug] · [Nexus zkVM][nexus-repo] ·
[L2BEAT, Airbender][l2beat-airbender] · [Airbender deep dive][airbender-docs]

**Proof-system toolkits and research:** [Plonky3][plonky3-repo] ·
[Least Authority audit of Plonky3, 7 Nov 2024][plonky3-audit] ·
[Plonky3 pull request 2533, 22 Oct 2026][plonky3-pr] · [Binius64][binius64] ·
[leanVM][leanvm] · [Flock repository][flock-repo] · [Flock paper][flock-paper] ·
[Introducing Flock, 25 Jun 2026][flock-blog] · [BinarySpartan][binaryspartan] ·
[Magnetar][magnetar] · [ZODA][zoda] · [Fiat–Shamir attacks][fs-attack] ·
[Succinct arguments in the quantum random-oracle model][qrom]

**Signatures:** [FIPS 204][fips204] · [FIPS 205][fips205] ·
[SP 800-208][sp800-208] · [SP 800-230 initial public draft, 13 Apr 2026][sp800-230] ·
[SLH-DSA STARK aggregation benchmark, 13 Apr 2026][btc-slh] ·
[Hekate ML-DSA chiplet][hekate] · [noir-dilithium][noir-dilithium] ·
[Aerie][aerie] · [Survey of Ethereum's post-quantum consensus, Aug 2026][goat-survey]

**Hashes:** [Poseidon2 algebraic attacks, ePrint 2026/306][poseidon2-attack] ·
[Ethereum's roadmap drops Poseidon][ef-poseidon] · [RFC 9861][rfc9861]

**Hardware:** [NVIDIA RTX 5090 specifications][rtx5090] ·
[NVIDIA RTX 50 series prices, 6 Jan 2025][rtx5090-price] ·
[Apple Mac Studio power consumption][mac-power] ·
[Apple Mac Studio announcement, 5 Mar 2025][mac-price]

The survey of Ethereum's post-quantum consensus ([goat-survey]) adds context
not used in the tables: it cites a goal of aggregating 10,000 signatures in
one second, and reports Falcon-512 aggregation with LaBRADOR at 10,000
signatures in 74 KB with 2.65 s of verification.

[impl-status]: https://github.com/ColdAI-org/siontology/blob/main/docs/implementation-status.md
[arch]: ../papers/architecture.md
[bench]: ../guides/benchmarks.md
[spec]: ../specification.md
[pq-book]: ../concepts/post-quantum.md
[magicians]: https://ethereum-magicians.org/t/the-real-blocker-for-post-quantum-ethereum-isn-t-the-math-it-s-aggregation-and-we-don-t-have-it-yet/27839
[soundcalc]: https://github.com/ethereum/soundcalc/blob/main/reports/summary.md
[soundcalc-sp1]: https://github.com/ethereum/soundcalc/blob/main/reports/sp1.md
[soundcalc-openvm]: https://github.com/ethereum/soundcalc/blob/main/reports/openvm2.md
[soundcalc-zisk]: https://github.com/ethereum/soundcalc/blob/main/reports/zisk.md
[soundcalc-venus]: https://github.com/ethereum/soundcalc/blob/main/reports/venus.md
[ef-security]: https://blog.ethereum.org/2025/12/18/zkevm-security-foundations
[ef-update]: https://zkevm.ethereum.foundation/blog/cryptography-research-update
[sp1-security]: https://docs.succinct.xyz/docs/sp1/security/security-model
[sp1-hypercube]: https://blog.succinct.xyz/sp1-hypercube/
[sp1-16gpu]: https://blog.succinct.xyz/real-time-proving-16-gpus/
[sp1-mainnet]: https://blog.succinct.xyz/sp1-hypercube-is-now-live-on-mainnet/
[sp1-repo]: https://github.com/succinctlabs/sp1
[l2beat-sp1]: https://l2beat.com/zk-catalog/sp1hypercube
[r0-security]: https://dev.risczero.com/api/security-model
[r0-repo]: https://github.com/risc0/risc0
[r0-target]: https://doc.rust-lang.org/rustc/platform-support/riscv32im-risc0-zkvm-elf.html
[r0-realtime]: https://risczero.com/blog/making-real-time-proving-accessible
[r0-fv]: https://risczero.com/blog/RISCZero-formally-verified-zkvm
[btc-slh]: https://groups.google.com/g/bitcoindev/c/0IdqdnlC4Og/m/6t5fx110AQAJ
[jolt-repo]: https://github.com/a16z/jolt
[lattice-jolt]: https://a16zfrontier.com/posts/article/lattice-snarks-jolt-post-quantum-faster/
[aerie]: https://eprint.iacr.org/2026/2501
[openvm-repo]: https://github.com/openvm-org/openvm
[openvm-2]: https://axiom.xyz/blog/openvm-2
[openvm-prod]: https://axiom.xyz/blog/openvm-2-production
[openvm-21]: https://www.axiom.xyz/blog/2-1
[openvm-audit]: https://reports.zksecurity.xyz/reports/openvm/
[zisk-repo]: https://github.com/0xPolygonHermez/zisk
[zisk-130]: https://github.com/0xPolygonHermez/zisk/releases/tag/v1.3.0-alpha
[zisk-docs]: https://0xpolygonhermez.github.io/zisk-docs/
[zisk-ethcc]: https://alearesearch.io/summit-digest/ethcc/jordi-baylina-polygon-cofounder-zisk-proving-in-real-time
[zisk-4gpu]: https://cryptoslate.com/ethereums-12-gpu-proving-problem-just-got-a-4-gpu-answer/
[plonky3-repo]: https://github.com/Plonky3/Plonky3
[plonky3-audit]: https://leastauthority.com/polygon-plonky3-final-audit-report
[plonky3-pr]: https://github.com/Plonky3/Plonky3/pull/2533
[stwo-repo]: https://github.com/starkware-libs/stwo
[l2beat-stwo]: https://l2beat.com/zk-catalog/stwo
[stwo-book]: https://docs.starknet.io/learn/S-two-book/benchmarks
[stwo-bug]: https://www.axiom.xyz/blog/stwo-bug
[nexus-repo]: https://github.com/nexus-xyz/nexus-zkvm
[binius64]: https://github.com/binius-zk/binius64
[l2beat-airbender]: https://l2beat.com/zk-catalog/airbender
[airbender-docs]: https://docs.zksync.io/zk-stack/components/zksync-airbender/deepdive
[leanvm]: https://github.com/leanEthereum/leanVM
[flock-repo]: https://github.com/succinctlabs/flock
[flock-paper]: https://eprint.iacr.org/2026/1329
[flock-blog]: https://blog.succinct.xyz/introducing-flock/
[binaryspartan]: https://eprint.iacr.org/2026/1656
[magnetar]: https://eprint.iacr.org/2026/2423
[hekate]: https://oumuamua.dev/pqc/ml-dsa
[noir-dilithium]: https://github.com/wstran/noir-dilithium
[goat-survey]: https://hackmd.io/@goatresearch/H1G2tOCwGx
[poseidon2-attack]: https://eprint.iacr.org/2026/306
[ef-poseidon]: https://postquantum.com/security-pqc/ethereum-roadmap-drops-poseidon/
[fs-attack]: https://eprint.iacr.org/2025/118
[qrom]: https://eprint.iacr.org/2019/834
[zoda]: https://eprint.iacr.org/2025/034
[fips204]: https://csrc.nist.gov/pubs/fips/204/final
[fips205]: https://csrc.nist.gov/pubs/fips/205/final
[sp800-208]: https://csrc.nist.gov/pubs/sp/800/208/final
[sp800-230]: https://csrc.nist.gov/pubs/sp/800/230/ipd
[rfc9861]: https://www.rfc-editor.org/rfc/rfc9861.html
[rtx5090]: https://www.nvidia.com/en-us/geforce/graphics-cards/50-series/rtx-5090/
[rtx5090-price]: https://nvidianews.nvidia.com/news/nvidia-blackwell-geforce-rtx-50-series-opens-new-world-of-ai-computer-graphics
[mac-power]: https://support.apple.com/en-us/102027
[mac-price]: https://www.apple.com/newsroom/2025/03/apple-unveils-new-mac-studio-the-most-powerful-mac-ever/
