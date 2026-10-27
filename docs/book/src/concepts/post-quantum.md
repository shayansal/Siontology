# Post-quantum identity

Every actor (person, company, device or AI agent) is an account whose
identifier never changes while its keys and policies can. Every
authorization needs signatures from **two unrelated mathematical families**,
so a forger must break both. In a trust formula that is
`module-lattice ∨ sha2-256`.

## Accounts

- **Stable identifiers, swappable keys.** An account's identifier is the
  hash of its genesis authorization policy. `RotatePolicy` replaces the keys
  (and the viewing key) without changing the identifier, so a broken scheme
  is replaced in place, not migrated around.
- **Policies, not keys.** A policy lists up to 255 public keys, the minimum
  number of valid signatures and the minimum number of distinct families
  among them. Every policy must require at least two families; a policy
  that does not is invalid.
- **The default policy** has one ML-DSA-44 key and one SLH-DSA-SHA2-128f
  key, and requires both.
- **Account creation.** An account exists once its first transaction
  executes: that transaction carries the genesis policy, which must hash to
  the sender's identifier.
- **Chain binding.** A transaction is signed over its digest in a signing
  context made of a fixed domain tag and the digest of the chain
  configuration, passed as the FIPS context string. A transaction signed for
  one chain never verifies on a chain with another configuration.

## The schemes

| Scheme | Family | Public key | Signature | Use here |
|---|---|---|---|---|
| ML-DSA-44 ([FIPS 204](https://csrc.nist.gov/pubs/fips/204/final)) | Module lattice | 1,312 B | 2,420 B | First leg of every account and validator |
| SLH-DSA-SHA2-128f ([FIPS 205](https://csrc.nist.gov/pubs/fips/205/final)) | Hash-based, stateless | 32 B | 17,088 B | Second leg of wallets and validators: fast signing, large signatures |
| SLH-DSA-SHA2-128s (FIPS 205) | Hash-based, stateless | 32 B | 7,856 B | Available for accounts that sign rarely: small signatures, slow signing |
| ML-KEM-768 ([FIPS 203](https://csrc.nist.gov/pubs/fips/203/final)) | Module lattice | 1,184 B | (a 1,088 B ciphertext) | Encrypting payloads to viewing keys and market committees |

Measured by `sion bench` on the machine used for this book (2 shared
cores, under load):

```text
signatures   ML-DSA-44 sign                                               688.36 µs
signatures   ML-DSA-44 verify                                             202.60 µs       2420 B signature
signatures   SLH-DSA-SHA2-128f sign                                     16366.62 µs
signatures   SLH-DSA-SHA2-128f verify                                    1062.20 µs       17088 B signature
signatures   SLH-DSA-SHA2-128s sign                                    335272.82 µs
signatures   SLH-DSA-SHA2-128s verify                                     292.59 µs       7856 B signature
```

## Where the bytes go

A dual signature (ML-DSA-44 and SLH-DSA-SHA2-128f) is 19,518 bytes as
encoded. A signed transfer captured from the devnet was 19,718 bytes, so
the signatures are about 99% of it. That is the main cost of post-quantum
safety, and the design answers it three ways:

- **Signatures never reach the data layer.** Validators check them and the
  World Proof covers the checks; the published data is transaction bodies
  only.
- **Validators never sign individual transactions.** Each validator signs
  one Merkle root per batch of approvals (batch-root voting, see
  [Linear Lanes](linear-lanes.md)).
- **Each authorization is verified once.** A validator caches an
  authorization when it locks a transaction's inputs, and executes a Pulse
  without re-verifying signatures it already knows to be valid under the
  policy the transaction will execute under. Proof stacks always verify.

## Why not Falcon, why not Ed25519

FN-DSA (Falcon) has smaller signatures (about 666 bytes), but it is also
lattice-based, so it cannot be the unrelated second family, and its
standard, FIPS 206, was not final when the architecture was written.
Ed25519 has 64-byte signatures but is not post-quantum; it is not used for
authorization.

## Validators

Validators sign with the same dual scheme: lane batch roots, consensus
headers and votes, and Pulse headers. A validator's identifier is derived
from its two public keys, and the chain identity lists every validator with
its keys (see [The chain identity](../reference/chain-identity.md)).

## In this implementation

- **Built**: dual-origin account policies with stable identifiers, key
  rotation, chain-bound signatures, ML-KEM-768 envelopes, dual-signed
  validator messages.
- **Simplified**: the implementations are the pure-Rust `fips203`,
  `fips204` and `fips205` crates, which have not been audited for side
  channels (atom `impl:unaudited-pq-crates`). Devnet validator keys are
  derived from the chain name (atom `impl:devnet-keys`).
- **Not built**: stateful XMSS^MT keys for validators and busy agents,
  forward-secure validator keys that defeat long-range rewriting, device
  passkeys (P-256) as an optional third leg, recovery through guardians or
  institutions, signature aggregation inside proofs.
- **A flag that turns it off**: `sion devnet --insecure-fast-signatures`
  replaces validator signatures with a simulation MAC whose verifier can
  forge. The chain identity records it, and every verification report says
  so (atom `impl:insecure-sim-signatures`).
