# Content Credentials: C2PA

A picture, a recording or a document the agent makes can carry Content
Credentials: a [C2PA](https://c2pa.org/specifications/) manifest. The
manifest binds the file's exact bytes and says what made it, and a
certificate signs it. `sion agent c2pa sign` writes one beside the file
(`<file>.c2pa`, a sidecar manifest in C2PA 2.x):

```text
$ sion agent c2pa sign pic.png --profile lead --cert chain.pem --key key.pem
signed pic.png as made by agent lead (did:sion:devnet:0707): pic.png.c2pa
```

The manifest holds three assertions:

- `c2pa.actions.v2`: the file was created by *trained algorithmic media*,
  the IPTC digital source type for what a model makes, with Siontology as
  the software agent. With `--edited`, it was edited instead.
- `org.siontology.agent`: the agent that made it, by its DID and name,
  and the models its profile runs on. The DID leads to its charter, its
  record and its bonds on Siontology.
- `c2pa.hash.data`: the SHA-256 of the whole file. Any change to the file
  breaks it.

The claim (`c2pa.claim.v2`) lists each assertion by the hash of its box,
and a COSE_Sign1 signature (ES256) covers the claim. The signature's
protected header carries the certificate chain. The key is a P-256 key in
PKCS#8 PEM, and the chain is PEM, the signer's certificate first. Both are
read from the files given and kept nowhere.

`verify` checks a file against its manifest: the claim's signature, every
assertion's hash, and the data hash. With `--trust <root.pem>` it also
checks that the signer's chain leads to that root, as RFC 5280's path
validation has it:

```text
$ sion agent c2pa verify pic.png --trust root.pem
pic.png: Content Credentials hold (3 assertions checked, the data hash among them)
  title: pic.png; made with Siontology
  c2pa.created, by http://cv.iptc.org/newscodes/digitalsourcetype/trainedAlgorithmicMedia
  by agent lead (did:sion:devnet:0707)
  signed by O=Siontology Test,CN=Siontology Test Agent: its chain leads to a root you trust
```

A file changed after it was signed is refused:

```text
$ sion agent c2pa verify changed.png --manifest pic.png.c2pa
error: the asset is not the one signed: its data hash differs
```

These are the end-to-end test's manifest (`crates/sion-cli/tests/c2pa.rs`),
signed with a test CA's certificate, and checked by the binary.

## Whose certificate

A manifest is as trusted as the certificate that signs it. A validator
that keeps C2PA's trust list shows a manifest as trusted only when its
signer's certificate chains to a CA on that list, and getting such a
certificate is up to you. A certificate from a CA of your own makes
manifests that validate, signed by a credential that nobody else has
reason to trust. That is what C2PA's own SDK reports for one (state
*Valid*, with `signingCredential.untrusted`).

## What was checked

C2PA's own SDK (c2pa-rs 0.91.3, through c2pa-python 0.38.1) validates a
manifest made here: the claim's signature, the three assertions' hashed
URIs and the data hash all hold. It reports the same untrusted credential
for a test CA's certificate as it does for its own manifest signed with
that certificate, and it refuses the file once a byte is added. In the
other direction, `verify` checks a manifest that c2pa-rs made, with a
thumbnail among its assertions, and refuses it when the file changes
(`crates/sion-cli/tests/c2pa.rs`).

A data hash that leaves any of the file out is refused: a sidecar is not
in the file, so it has nothing to leave out, and an exclusion could leave
out all of it. A claim that lists an assertion twice is refused too.

The agent a manifest names is its signer's word: nothing here ties the
agent's DID to the certificate that signed it.

Not built: a manifest embedded in the file (in a PNG's `caBX` chunk or a
JPEG's APP11 segments), signatures other than ES256, time-stamps from a
time-stamp authority, ingredients (the files an edit was made from), and
the agent's tools signing what they make on their own.
