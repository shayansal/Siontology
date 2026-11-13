# Receipts: which model answered

Whoever pays for an agent's work pays, among other things, for the model
behind it. A model that costs less can be put in its place, and the
answer does not say so. A receipt is the runtime's record of each answer
a model gave it: which model it asked for, at which address, which model
the provider said answered, and the tokens counted.

Receipts come in two strengths, and this page keeps them apart:

- **An operator's receipts** are its word about its own machine. A claim
  fixes them at a Pulse, so they cannot be changed afterwards.
- **An enclave's receipts** are signed with a key its attestation binds.
  They are the word of the code that was measured, on the chip that
  signed the report, and not of the agent's owner.

Outputs on this page are from real runs of the commands on a local
devnet with scripted models and a simulated enclave, with long
identifiers and paths shortened with `…`. Nothing here was run against a
model provider or AMD's hardware.

## What a runtime keeps

Every answer a model gives a session has a receipt: a turn of the
conversation, a check against the operator's rules, a reading of content
from outside, a summary. A receipt holds

- the model asked for, as it is configured, and the scheme and host of
  the address the request went to;
- the model the provider's answer named;
- the digest of the request as the runtime made it (system prompt,
  conversation, tools) and the digest of what came back;
- the tokens the provider counted;
- the digest of the receipt before it.

The last item makes a session's receipts a chain with one head. Once the
head is signed or committed to, no receipt can be changed, dropped or put
in without the head changing. A model that fails and gives no answer has
no receipt; the one that answered in its place has.

```text
$ sion agent receipts show c1b25fa7…
session c1b25fa72c548e10: 2 answer(s) from models, head 2ad2b10b6fb4ef01…
  scripted (asked for script:payer2.json): 2 answer(s), 3945 tokens in and 80 out
the last 2:
  0: scripted (asked for script:payer2.json), 1840 in and 62 out
  1: scripted (asked for script:payer2.json), 2105 in and 18 out
```

The model here is a script, which says that it is one. With a provider's
model a line has the form `big-1-2026 (asked for big-1) at
https://api.example.com`: the name the provider's answer gave, the name
configured, and the address asked.

A session keeps its last 200 receipts whole. The count, the head and the
tallies cover every one.

## An operator's receipts

`sion agent receipts write` writes a session's receipts to a file. With
`--wallet` it commits to them in a claim:

```text
$ sion agent receipts write c1b25fa7… --out receipts.json --wallet alice
committed to 2 receipt(s) in claim 3b29bef5…
2 receipt(s) written to receipts.json
```

Whoever is given the file checks it against the ledger:

```text
$ sion agent receipts check receipts.json
holds: 2 receipt(s) in a chain that ends at the head, each holding the digest of the one before
holds: account 42424444 committed to this head and these tallies in claim 3b29bef5 at Pulse 10
  this is its operator's word about its own machine, fixed as of that Pulse
  scripted (asked for script:payer2.json): 2 answer(s), 3945 tokens in and 80 out
```

A file changed after the commitment no longer adds up to the head that
was committed to. Here the model that answered was renamed:

```text
$ sion agent receipts check receipts-changed.json
differs: the head is not the digest of the last receipt
…
error: verification failed: the receipts do not check out
```

This is worth what the operator's word is worth. The receipts were kept
by a runtime on a machine the operator controls, and a changed runtime
can keep any receipts it likes. What the claim adds is the time: the
operator said this then, and cannot say something else later. A file
with no claim behind it shows only that it agrees with itself, and
`check` says so.

## An enclave's receipts

An agent [rented from an enclave](renting.md) signs its receipts with
the key its attestation binds, the same key that signs its count of
tokens. They are one chain over the whole rental. The renter's client
checks the signature on every reply and shows who answered:

```text
$ sion agent rent use a3cdba19… summarizer 'Summarize the attached terms.' \
    --at http://127.0.0.1:8795 --wallet renter --deposit 5 \
    --simulated-platform 045543c8… --receipts rental.json
its enclave checks out: a simulated enclave (nothing stands behind it)
…
it used 1000 input and 200 output tokens: 0.006 USD
which models answered, signed by its enclave:
  scripted (asked for script:…/helper.json): 1 answer(s), 1000 tokens in and 200 out
its receipts are in rental.json
paid 0.006 USD for it
```

`--receipts` keeps the receipts, the enclave's signature and its
attestation in a file. The renter can hand that file to someone else,
its own customer for instance, who checks it with nothing but the file
and the measurement of the code they trust:

```text
$ sion agent receipts check rental.json --simulated-platform 045543c8… --measurement 3b4756d5…
holds: 1 receipt(s) in a chain that ends at the head, each holding the digest of the one before
holds: the key is bound to a simulated enclave (nothing stands behind it), running code of measurement 3b4756d5…
holds: that key signed this head and these tallies: it is the measured code that says where it asked and which model it was told answered
  scripted (asked for script:…/helper.json): 1 answer(s), 1000 tokens in and 200 out
```

What that shows, on real hardware: code with that measurement, in a
virtual machine its host cannot read, asked that address and was told
that model answered. The agent's owner serves the traffic and cannot
sign a receipt in the enclave's place. If the owner's agent quietly
falls back to a cheaper model, the receipts name it.

A simulated enclave is taken only from a platform named outright, as
everywhere else:

```text
$ sion agent receipts check rental.json
holds: 1 receipt(s) in a chain that ends at the head, each holding the digest of the one before
differs: the enclave does not check out: a simulated enclave, which is taken only from a platform named outright
…
error: verification failed: the receipts do not check out
```

## What a receipt shows, and what it does not

(`agent:model-receipts`)

- **It names an address and the provider's name for a model.** Nothing
  looks inside the model. A provider that served another model under the
  same name would not show, and neither would one that changed a model
  behind its name.
- **The owner chooses the address.** The model's address and key are the
  owner's settings, in an enclave too. A receipt says which address was
  asked, and the reader checks that it is the provider's own. A runtime
  that trusted certificate roots beyond the public ones (`SSL_CERT_FILE`)
  says so in what it signs, and `check` notes that an address's name then
  does not show who answered there.
- **An enclave's receipts rest on what its attestation rests on**: AMD's
  certificate chain and its chips, and the measured code, which the
  reader names by its measurement. A measurement says what was launched,
  not that it is right.
- **An operator's receipts are its word.** See above.
- **A model that is not asked over a network has no address.** That
  covers a scripted model and a model run on the same machine. The
  weights of a local model are not measured.
- **The request's digest is of the request as the runtime made it**, not
  of the bytes the provider received. Whoever holds the request can
  recompute it; a renter, who does not see the owner's system prompt,
  cannot.
- **Each session keeps its own receipts.** A subagent's are in its own
  session.
- **Not built:** proof to a third party that the provider itself sent an
  answer (a proof over the TLS session, as TLS notaries make), and proof
  that given weights computed an answer. The second would be a
  zero-knowledge proof of the model's whole computation, which is not
  practical for models of this size
  ([what is proven with zero knowledge](proofs.md)).

This is tested on a devnet with scripted models and a simulated enclave
(`agent_receipts.rs`, and `receipts.rs` in `sion-agent`): a session's
receipts committed to and checked, a renamed model, a dropped receipt
and a forged chain found; an enclave's receipts over two requests of a
rental, one answered by another model than the first, checked by the
renter and from the file by a third party, and refused for another
platform, another measurement and a changed file.
