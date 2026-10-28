# Responsible disclosure

If you find a security problem, report it privately so it can be fixed
before it is public. The repository's
[`SECURITY.md`](https://github.com/ColdAI-org/siontology/blob/main/SECURITY.md)
is the policy; this page summarises it.

## How to report

Email **security@siontology.org**. (This address is a placeholder until the
project announces its security contact; until then, it may not be
monitored. If you get no answer within a few days, open a GitHub issue that
says only that you have a security report and asks for a private channel,
without details.)

Include:

- what you found and why it matters;
- how to reproduce it: the commit, the commands, and the output;
- which components it affects (for example `sion-consensus` or the light
  client);
- whether you want to be credited, and how.

Please do not open a public issue, pull request or discussion with the
details.

## What is in scope

- The protocol as specified in `docs/spec/state-transition.md`: anything
  that lets an invalid transition be proven, a proof stack accept what the
  specification rejects, or the two stacks disagree.
- The consensus, lane and light-client code: anything that breaks safety
  with fewer than a third Byzantine stake, makes a light client accept a
  false header, receipt or proof, or lets evidence be forged.
- The node: crashes, unbounded memory, or a split between replicas caused
  by a transaction or message anyone can send.
- The cryptographic wrappers in `sion-crypto`.

## What is out of scope

- The assumptions the implementation documents as `impl:` atoms
  ([Trust model](trust-model.md)), such as replay proofs, public devnet
  keys, the in-process network, a slashed validator keeping its seat until
  the epoch ends, or the beacon's last-revealer bias. They are known.
- The adversary-simulation controls (`sion censor`,
  `POST /v1/devnet/censor`) and `--insecure-fast-signatures`, which are
  insecure by design.
- Anything that needs control of a devnet's process or keys.
- Bugs in third-party crates, unless Siontology uses them in a way that
  makes them exploitable. Report those upstream.

## Supported versions

None is supported in production. Siontology is a devnet-scale reference
implementation; fixes go to the default branch.

## Safe harbour

The project will not take legal action against research done in good
faith within this policy. Test only against devnets you run yourself, and
do not access other people's data.
