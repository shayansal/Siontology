# sion-crypto

Post-quantum identity and encryption for Siontology. It wraps the NIST
signature and key-encapsulation standards and builds on them the pieces the
ledger needs: accounts whose identifier survives key rotation, validator
identities, and payloads sealed to one recipient or to a committee. Two ideas
matter to a user. An action is authorized by two signatures from unrelated
families, a lattice scheme (ML-DSA-44) and a hash-based one (SLH-DSA), so a
forger must break both (trust formula `lattice ∨ hash`). And an account's ID
is the hash of its genesis policy, so keys can change while the ID stays.

Part of [Siontology](../../README.md) · [Architecture](../../ARCHITECTURE.md)
· Status: **Built** (post-quantum accounts are implemented with tests, but the
FIPS 203/204/205 implementations are unaudited pure-Rust crates, atom
`impl:unaudited-pq-crates`; see
[implementation status](../../docs/implementation-status.md)).

## What is in it

| Module | What it holds |
|---|---|
| `scheme` | The `Scheme` enum (ML-DSA-44, SLH-DSA-SHA2-128s, SLH-DSA-SHA2-128f) and the `Family` each belongs to |
| `keys` | `Keypair`, `PublicKey`, `Signature` and `verify`; the signing domain tag is passed as the FIPS context string |
| `dual` | `DualKeypair`: one lattice key and one hash-based key, and `verify_dual` |
| `policy` | `AuthPolicy`, `Authorization` and `KeySignature`: who may authorize an account, and what a valid authorization is |
| `kem` | ML-KEM-768 key pairs and single-recipient `SealedEnvelope`s (ML-KEM with ChaCha20-Poly1305) |
| `validator` | Validator identifiers (the hash of two public keys) and dual signatures under `VALIDATOR_DOMAIN` |
| `shamir` | Shamir secret sharing over GF(2^8), byte by byte |
| `committee` | `CommitteeEnvelope`: a payload sealed to a committee, opened with `t` of `n` key shares |
| `error` | `CryptoError` |

## Main types and functions

- [`Scheme`, `Family`](src/scheme.rs): the three signature schemes and the two hardness families.
- [`Keypair`, `verify`](src/keys.rs): sign and verify under a domain tag, so a signature made for one purpose never verifies for another.
- [`DualKeypair`](src/dual.rs): holds one key of each family; `authorize` signs a message with both and returns an `Authorization`.
- [`AuthPolicy`](src/policy.rs): an account's authorization policy; `account_id` hashes it, and `verify` checks an `Authorization` against it. The default policy needs one valid signature from each of two families.
- [`Authorization`](src/policy.rs): the signatures presented for one action.
- [`KemKeypair`, `SealedEnvelope`, `seal`](src/kem.rs): encrypt to a viewing key; used for viewing keys and Travel Rule payloads.
- [`validator_id`, `verify_validator`](src/validator.rs): validator identity and the dual signature validators put on headers, votes and batch roots.
- [`CommitteeEnvelope`, `KeyShare`](src/committee.rs): seal a payload so that no one can read it until `t` committee members publish their shares; used for sealed intent batches.
- [`shamir::split`, `shamir::combine`](src/shamir.rs): the threshold sharing under the committee envelope.

## How it fits

Depends on: `sion-core`, and on `fips203`, `fips204` and `fips205` for the
NIST algorithms, `chacha20poly1305` for the payload cipher, and `zeroize` for
key material. Used by: `sion-exec`, `sion-spec`, `sion-node`, `sion-light`,
`sion-net`, `sion-memory`, `sion-cli` and `sion-app-core`.

Committee sealing is a simple construction for small committees: the payload
key is Shamir-shared and each share is sealed to one member with ML-KEM. The
module's own documentation says that threshold encryption which is both
post-quantum and efficient for large committees is open, so the committee
threshold is an atom in any trust formula that relies on the sealing. This
crate does not decide who the committee is or when a batch seals; the ledger
does.

## Tests

Unit tests inside `src/`: 24 `#[test]` functions at the time of writing
(counted with `grep`). They cover signing and verifying with every scheme,
seeds, key sizes against the standards, dual authorization (both legs are
needed), policies (single-family policies are invalid, a two-of-three policy
with a backup key, a stable account ID), KEM envelopes (a wrong key, domain or
tampered ciphertext fails), committee sealing (a threshold of members opens
it; shares from another envelope do not), validator signatures, and Shamir
sharing. One Shamir test is a `proptest` property test. There is no `tests/`
directory and no `#[ignore]`d test. Key rotation is a ledger operation and is
tested in `sion-exec`, not here.

```text
cargo test -p sion-crypto
```

The crate is 1,801 lines of Rust under `src/` at the time of writing.

## Read more

- [Post-quantum identity](../../docs/book/src/concepts/post-quantum.md): the dual scheme, signature sizes and why two families.
- [Privacy](../../docs/book/src/concepts/privacy.md): viewing keys, sealed payloads and sealed orders.
- [Trust model](../../docs/book/src/security/trust-model.md)
- [Crate map](../../docs/book/src/architecture/crates.md)
