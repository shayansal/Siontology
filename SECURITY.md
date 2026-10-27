# Security policy

## Supported versions

None in production. Siontology is a devnet-scale reference implementation:
it runs every validator in one process, its proofs are replay proofs, and
its devnet keys can be derived by anyone. It secures nothing of value.
Fixes go to the `main` branch; there are no maintained release branches.

## Reporting a vulnerability

Please report security problems privately, by email to
**security@siontology.org**.

> This address is a placeholder until the project announces its security
> contact, and it may not be monitored yet. If you receive no answer within
> a few days, open a GitHub issue that says only that you have a security
> report and asks for a private channel. Do not put details in the issue.

Include:

- what you found and why it matters;
- how to reproduce it: the commit, the commands and the output;
- which components it affects;
- whether and how you would like to be credited.

Do not open a public issue, pull request or discussion with the details
until a fix is available and a disclosure date is agreed. We will confirm
that we received your report, work with you on a fix, and credit you in the
fix and the changelog if you wish.

## Scope

In scope:

- **The protocol** as specified in `docs/spec/state-transition.md`:
  anything that lets an invalid transition be proven, lets a proof stack
  accept what the specification rejects, or makes the two proof stacks
  disagree.
- **Consensus, Linear Lanes and the light client**: anything that breaks
  safety with less than a third of a committee's stake Byzantine, makes a
  light client accept a false header, receipt, state proof or epoch proof,
  or lets equivocation evidence be forged or misattributed.
- **The node**: crashes, unbounded memory or disk use, or a split between
  validators, caused by a transaction or message that anyone can send.
- **The cryptographic wrappers** in `crates/sion-crypto`.

Out of scope:

- The trust assumptions the implementation documents as `impl:` atoms in
  `docs/implementation-status.md`, for example: replay proofs instead of
  succinct ones, devnet keys anyone can derive, validators in one process,
  the epoch hand-over relying on reliable delivery, a slashed validator
  keeping its seat until the epoch ends, equivocation evidence not bound to
  a chain, and the beacon's last-revealer bias. They are known; a report
  that one of them can be exploited in a way the documentation does not
  describe is in scope.
- Everything marked "not built" in `docs/implementation-status.md`.
- The adversary-simulation controls (`sion censor`,
  `POST /v1/devnet/censor`), the devnet faucet, and devnets started with
  `--insecure-fast-signatures`, which are insecure by design.
- Attacks that need control of a devnet's process, machine or keys.
- Vulnerabilities in third-party crates, unless Siontology uses them in a
  way that makes them exploitable; report those upstream.
- Social engineering, denial of service by volume against someone else's
  machine, and findings from automated scanners without a demonstrated
  impact.

## Safe harbour

We will not pursue legal action against anyone who, in good faith and
within this policy, researches and reports a vulnerability: who tests only
against devnets they run themselves, does not access or modify other
people's data, does not degrade services others rely on, and gives us
reasonable time to fix the problem before disclosing it.
