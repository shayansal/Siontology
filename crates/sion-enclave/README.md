# sion-enclave

Attestation for agents that run in enclaves. An agent's runtime inside an
AMD SEV-SNP confidential VM asks the chip for a report, through the Linux
kernel's configfs-tsm files (`tsm`). The report carries the measurement of
what the VM was launched with and 64 bytes that bind the session's own keys
(`Binding`): the ML-KEM key requests are sealed to, and the ML-DSA key its
answers are signed with. `verify` checks an `Attestation`: the report's
ECDSA P-384 signature by the chip's key (VCEK), that key against AMD's
chain for its generation (ASK and ARK, built in, RSA-PSS), the chip and
firmware versions the key names, the binding, a launch measurement the
checker trusts, a guest its host cannot debug, and the binding's expiry.
Every signature is checked by aws-lc-rs; x509-cert only reads the
certificates.

`simulated` is a platform made of a key on some machine, for tests and
devnets: its reports have AMD's form, nothing stands behind them, and
`verify` takes one only from a platform key it is given outright.

Not built: checking Intel TDX quotes (they are collected, then refused);
AMD's revocation lists; Turin's layout of firmware versions in reports
(checked by the chip key's signature alone).

## Tests

`tests/attestation.rs` checks a real Milan report and its chip key, and a
Turin chip key, from the `virtee/sev` crate's tests (Apache-2.0), against
the built-in chain; changed bits, a forged key and another chip's key are
refused. AMD's ARK and ASK certificates in `src/amd/` are AMD's
published ones, as `virtee/sev` ships them.
