# X.509 test chains for AP2's `x5c` roots

Test certificates and keys, made by `make.py` (Python's `cryptography`)
for `tests/ap2_vectors.rs` and `tests/interop.rs` only. The private keys
(`.p8`, PKCS#8 DER) sign nothing anywhere else: they are here so that a
root mandate can be signed by a certificate's key in the tests.

| File | What it is |
| --- | --- |
| `root.der` | A P-384 root CA, path length 1 |
| `inter.der` | A P-256 CA under the root, path length 0 |
| `leaf.der`, `leaf.p8` | A P-256 signer under the intermediate (digital signature only) |
| `rogue-root.der` | A root no test trusts |
| `noca-inter.der` | A certificate under the root that is not a CA |
| `noca-leaf.der`, `noca-leaf.p8` | A signer under the certificate that is not a CA |
| `expired-leaf.der`, `expired-leaf.p8` | A signer under the intermediate, valid only in 2020 |
| `rsa-root.der` | An RSA 2048 root CA, path length 0 |
| `rsa-leaf.der`, `rsa-leaf.p8` | A P-256 signer under the RSA root |

All but the expired one are valid from 2025 to 2125.
