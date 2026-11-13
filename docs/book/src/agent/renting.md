# Renting your agent out

An agent can be rented to other people by the token, in an enclave they
can check. The owner lists it with a price; a renter checks the enclave,
escrows a deposit, and pays for the tokens the enclave counted. The rented
sessions get none of the owner's memory, files, skills or reach.

Every output below is from a real run on a local devnet. The enclave is
simulated, as this machine has no confidential-computing hardware, and the
agent is a stand-in: nothing here was run against AMD's hardware or a model
provider.

## What the renter is trusting

An owner who rents an agent out sees everything that passes through it,
unless the machine stops them. So renting rests on an *enclave*: a virtual
machine whose memory the host cannot read, whose chip signs a report of
what was launched in it ([Attestation](#attestation)). The session's keys
are made inside the enclave and never leave it, and the renter seals each
request to the enclave's key. The owner serves the traffic and cannot read
it (`agent:signed-in-browser` is a different matter; this is
`agent:rented-enclave`).

What that rests on, in turn: AMD's certificate chain and its chips, the
measured code itself (which the renter names by its measurement, and whose
behaviour no measurement proves), and that the enclave was not launched
debuggable. A simulated enclave rests on nothing at all, and is taken only
when the renter names its platform key outright.

## Listing an agent

`sion agent rent serve <profile>` attests the enclave, publishes the
listing and serves renters:

```text
$ sion agent rent serve helper --name summarizer --per-1k-input 0.003 --per-1k-output 0.015 \
    --description 'summarizes documents and contracts' --listen-at 127.0.0.1:8795
(not in an enclave this attests: /sys/kernel/config/tsm/report is not here: this is not a confidential VM, or its kernel has no configfs-tsm)
(using a SIMULATED platform: nothing stands behind its reports, and a renter takes one only with --simulated-platform)
listed helper as summarizer: 0.003 USD per 1k input, 0.015 USD per 1k output, steps of 0.001 USD
its listing is claim 7b31c0a4…
its enclave is SIMULATED; renters need --simulated-platform 04a1f2…
renters find it with: sion agent rent show 9f2c71ab… summarizer
serving sealed requests at http://127.0.0.1:8795/v1/rent (Ctrl-C ends it)
```

The listing is a claim, named the owner's latest under `rental:<name>` in
its index, so a renter who checks the index entry against a certified
header knows it is the owner's current offer. It carries the price, the
agent, the stream step, the least to escrow, a line of what the agent is
for, and the attestation. The attestation's binding commits to a digest of
the terms, so a listing whose price or payee was changed afterwards no
longer matches its own attestation and is refused.

The model runs on the owner's account, priced into what they charge; a
listing can instead say the renter brings their own key, which is not
served yet.

## Renting one

`sion agent rent show` reads a listing and checks it:

```text
$ sion agent rent show 9f2c71ab… summarizer --simulated-platform 04a1f2…
summarizer by 9f2c71ab…
  summarizes documents and contracts
  0.003 USD per 1k input, 0.015 USD per 1k output, steps of 0.001 USD, at least 1 USD escrowed
  its agent is 4d8e0b17…
  its listing is claim 7b31c0a4…
  its enclave checks out: a simulated enclave (nothing stands behind it)
  running code of measurement 9c2e…
```

`sion agent rent use` checks the enclave *before* anything is sent to it or
paid, escrows a deposit in a payment stream, asks, and pays for what the
enclave signed for:

```text
$ sion agent rent use 9f2c71ab… summarizer 'Summarize the attached terms.' \
    --at http://127.0.0.1:8795 --wallet renter --deposit 5 --simulated-platform 04a1f2…
its enclave checks out: a simulated enclave (nothing stands behind it)
(you did not say which code you trust: it says it runs 9c2e…)
escrowed 5 USD in stream 2f90ab3c… at 0.001 USD a step

The terms run for twelve months and renew unless either side gives notice.

it used 1000 input and 200 output tokens: 0.006 USD
which models answered, signed by its enclave:
  scripted (asked for script:…/helper.json): 1 answer(s), 1000 tokens in and 200 out
paid 0.006 USD for it
(the stream stays open for more; `sion stream close 2f90ab3c…` takes back the rest)
```

The enclave also signs which models answered, with the same key: the
receipts its runtime kept, as one chain over the rental. `--receipts
<file>` keeps them with the attestation for someone else to check
([Receipts](receipts.md)).

Give `--measurement <hex>` to say which code you trust; without it
everything else is checked and what the enclave runs is only its own word.

## Paying by the token

The price is the tokens the session used: input and output each at their
price per thousand, rounded up to a whole number of the listing's stream
steps. A [payment stream](../guides/streams.md) carries it: the renter
escrows once and reveals one hash per step, so thousands of requests settle
in a few transactions.

The enclave counts the tokens and signs the running total with the key its
attestation binds, so the count is the enclave's word and not the owner's:
a renter pays only for what was signed. Payment runs one request behind —
each request carries the payment for the total signed through the request
before it — so the owner is ever owed at most one request's tokens, and a
renter that stops paying is served no further. A request with an empty
message runs nothing and only settles, which closes the tab.

On the ledger, the stream's position is the total in steps and its escrow
falls by exactly that; the tests check both.

## What the rented session may do

A renter's session runs the owner's profile stripped of its reach: no
memory log, no workspace or files, no skills, no web, no shell, no MCP
servers, no browser or desktop, no subagents. What the renter says is
content from outside, so the session is tainted from the start and its
external calls and spending wait for a person — which, for a rented
session, means they do not happen.

## Attestation

`sion-enclave` reads an AMD SEV-SNP report and checks it: the report's
ECDSA P-384 signature by the chip's own key, that key against AMD's chain
for its generation (its root and signing keys are built in), the chip and
firmware versions the key names, that the guest was not launched
debuggable, the 64 bytes binding the session's keys and the listing's
terms, and that the binding has not expired. Reports are asked for through
the kernel's configfs-tsm files; the chip's key comes from the host's table
of certificates or AMD's key distribution service.

The tests check a real report and chip key of an EPYC Milan chip, and a
Turin chip key, against the built-in chain, and refuse reports with
changed bits, a forged key and another chip's key.

## What this does not do yet

- Intel TDX quotes are collected and then refused: they are not checked.
- AMD's revocation lists are not consulted, so a key AMD has revoked still
  checks.
- Turin's layout of firmware versions in a report is not read, so a Turin
  report rests on its chip key's signature alone.
- A listing that says the renter brings their own model key is not served.
- Nothing runs the enclave for you: building and launching a confidential
  VM with a measured image of `sion`, and publishing that measurement for
  renters to trust, is the operator's own work.
- A renter learns what an agent costs after the fact, one request at a
  time; there is no quote beforehand and no spending cap inside a session.
