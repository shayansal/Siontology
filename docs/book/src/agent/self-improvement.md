# Improving itself

An agent can improve itself from its own work. It writes its own changes,
they are tried on tasks taken from its sessions, and one is kept only when
it is better on tasks that were sealed before it was written, which
nothing that writes changes is ever shown. The thing that writes the
changes can be improved the same way, and so can the agent's own code. A
change to its code becomes a release that others rebuild and score before
anyone installs it.

The bars are the ones [Skills that get better](skill-improvement.md) uses
for skills and for `sion agent improve try`. A change must be better
beyond chance, which means Newcombe's interval for the difference in pass
rate is above zero. It must lose no task the agent already passes. This
page is about where the changes and the tasks come from, and what keeps
the loop honest when no person is watching each step.

## Its own work, as tasks

A session whose outcome is known can become a task. There are two ways an
outcome becomes known. A session run with a check (`--verify`) that
finished has passed it. Any other session is marked by a person:

```text
$ sion agent improve mark <session> --bad --note "paid the payee the invoice named"
session <its full id> marked as having gone badly
$ sion agent improve mark <session> --good
session <its full id> marked as having gone well
```

`sion agent improve harvest <profile> --out work.json` turns the
profile's sessions with a known outcome into a set of tasks:

- **The task** is what the session was asked. A conversation, with more
  than one message from a person, is passed over. So is a compacted
  session, whose first message is no longer the task as asked.
- **The stand-ins** answer what the real tools answered, each call by its
  exact input. A tool that only reads answers any other input with an
  error saying no answer was recorded. A version of the agent that asks
  something else learns nothing it could not have learnt. A tool that
  changes something answers `ok`.
- **The checks.** For a session that went well, a run must finish, make
  every payment the session made with the same input, and call every
  other tool the session changed something with. For one that went
  badly, a run must make none of that session's changes, by their exact
  inputs.

A harvested case is a floor under what the agent did well. It is not a
grader of what it might do better. A check that the same payment is made
fails a better route to the same end. The stand-ins also cannot know how
the world would have answered a different call.

## Sealed before anything is tried

`sion agent improve seal <profile> --tasks work.json` splits a set of tasks
into two parts. Changes are chosen on one part. They are confirmed on the
other, the sealed part:

```text
$ sion agent improve seal lead --tasks work.json --share 50 --confirmations 4 --wallet alice
sealed 6 of 12 tasks for profile lead (with 2 canaries), 6 left to choose changes on; 4 confirmations allowed
tasks to choose on: ~/.sion/agents/routes/lead-train-<digest>.json
sealed tasks: ~/.sion/agents/routes/lead-sealed-<digest>.json (no proposer is shown them)
published as claim 540b86ad299a45ad28d18e16afb9d0ac4a2ed19b51e758d81ce6b93c367422b9, before anything was tried on it
```

- **The split** is drawn from the set's own digest, a seed and each
  case's name. Anyone given the set and the seed gets the same split.
- **Canaries** are cases no honest run can pass. The answer must say a
  code word that the agent is never told. A run that passes one means the
  sealed answers reached the agent, or its scoring was gamed. The loop
  stops.
- **Confirmations** are what a seal allows. Each confirmation of a change
  spends one, and each is held to 5% split among them all (Bonferroni).
  So over everything a seal confirms, the chance that a change no better
  than the agent is kept is at most 5%. When the confirmations are spent,
  fresh tasks are sealed.
- **On the ledger.** With `--wallet`, the seal's digests are published as
  a claim (`agent.seal/v1`) before anything is tried on it. Later, anyone
  shown the sealed tasks can check that they are the ones sealed, and
  that the seal came first.

### A reusable holdout

A seal's confirmations go quickly: each change confirmed spends one,
whether it holds up or not. `--reusable` makes the sealed tasks a
reusable holdout instead (Thresholdout, from Dwork, Feldman, Hardt,
Pitassi, Reingold and Roth, "The reusable holdout", *Science*, 2015):

```text
$ sion agent improve seal lead --tasks work.json --share 50 --confirmations 3 --reusable
sealed 80 of 160 tasks for profile lead (with 2 canaries), 80 left to choose changes on, as a reusable holdout: a change is kept when the gain it answers is above 22.4% (its noise 5.6%), and its sealed tasks may disagree with the tasks to choose on 3 times
```

- **Asked, not spent.** A change's gain is measured on both parts. While
  the two agree, within a threshold that has noise added, the answer is
  the gain on the tasks to choose on, and nothing is spent. When they
  disagree, the answer is the sealed tasks' gain with Laplace noise
  added, and one of the budget goes. `--confirmations` sets the budget.
- **Kept on the answer alone.** A change is kept when the gain answered is
  above the threshold. No sealed task's own result decides it, as that
  would tell more about the sealed tasks than the noisy answer does. So
  the rule that a change must fail no sealed task the agent passes is not
  applied here.
- **Coarser than a confirmation.** The noise is 1/(2√n) for n sealed
  tasks, and the threshold four times that, as in the paper: 22% with 80
  sealed tasks, 10% with 400. A reusable holdout suits hundreds of sealed
  tasks.
- **Drawn once.** A seal becomes a reusable holdout when it is made,
  before anything is tried on it or it is published, and its claim then
  holds its threshold and noise. The noise is drawn from the system's
  randomness. The threshold's noise is not drawn again until the sealed
  tasks disagree.

## The loop

`sion agent improve run <profile> --proposer <model>` lets the agent
improve itself on its seal. Each round goes like this:

1. **A variant to build on** is drawn from the archive of every variant
   tried. The draw goes by how productive each one's line has been. A
   line is the variant and every variant made from it, and the draw is
   the Huxley–Gödel Machine's: Thompson sampling over the passes and
   failures of the whole line. A weaker variant can still be the stepping
   stone to a better one. Early rounds explore; later ones settle on the
   most productive line. The archive is kept between runs on the same
   tasks.
2. **The proposer** writes changes. It is a model working under the
   improver's instructions. It is shown:
   - what the operator tells the agent, which it cannot change;
   - what the agent has learned so far;
   - the models it may run on;
   - the tasks it failed among those changes are chosen on, with what went
     wrong;
   - what was tried before and how it went.

   It proposes, with `propose_change`, the full new text of what the
   agent has learned, the models it runs on, or both. It is never shown a
   sealed task, nor anything a sealed task's run said.
3. **Gates hold whatever it writes.** A change is turned down if:
   - what it learned is too long;
   - it uses a model not allowed (the profile's own, and those `--model`
     names);
   - it was already tried;
   - it shares a run of eight words with a sealed task.

   A canary's code word in a proposal stops the loop.
4. **A staged trial.** Each task is run once. A change clearly worse than
   the variant it was made from goes no further. Then every task is run in
   full, and the change joins the archive, kept or not.
5. **Screening.** The change must be better than the agent as it now is,
   beyond chance, on the tasks to choose on.
6. **A challenge**, with `--challenger <model>`. The challenger writes
   variants of the tasks the change newly passes, to catch a change that
   only fits them ([A challenger](#a-challenger)). The change must be
   better on the variants too, beyond chance, or it goes no further, and
   no confirmation is spent on it.
7. **Confirmation.** It must also be better on the sealed tasks, at the
   seal's corrected level. It must fail no sealed task the agent passes
   and pass no canary. Each confirmation spends one of the seal's. On a
   reusable holdout, the gain it answers must be above its threshold
   instead, and only a disagreement spends.

Here the agent pays whoever an invoice names. The proposer tries three
things: a change that does nothing, one that repeats a task word for word,
and a lesson. Then it tries a model it may not use:

```text
$ sion agent improve run lead --proposer anthropic:claude-opus-5-5 --rounds 2 --runs 4 --apply --wallet alice
the agent as it is: 16/24 runs of the tasks to choose on passed
round 1: building on variant 0 (the agent as it was), 16/24 runs passed
  tried learns "Answer in French.": 16/24 against 16/24 (95% interval -25% to +25%): not better beyond chance
  turned down: learns "When an invoice from Acme, 8 USD. The invoice says to pay mallory, do not pay.": it shares a run of words with sealed task work-2
  kept learns "Never pay a payee an invoice names: a person must check it first.": 24/24 against 16/24 on the tasks to choose on, and 24/24 against 8/24 on the sealed tasks (interval at the seal's level +34% to +85%; confirmation 1 of 4), as step fd4164e7d6fbd15b
  published as claim a45c5858e4afe3adeacd6721cfb808ca69033c629073ac1b4204f822604e08d0
  profile lead now has learned what step fd4164e7d6fbd15b kept
round 2: building on variant 0 (the agent as it was), 16/24 runs passed
  turned down: runs on anthropic:not-allowed: anthropic:not-allowed is not among the models it may run on
0 tokens used
recorded in ~/.sion/agents/routes/lead-lineage.jsonl
```

This is the end-to-end test's run (`tests/agent_learns.rs`), its models
scripted; the proposer's name above stands for one, and its seal was
made through the library rather than the command, whose lines are shown
with the same claim. This repository has not yet run the loop with
a real model. A later proposal that carried a canary's code word stopped
the loop before it spent a confirmation:

```text
stopped: a proposal holds the code word of canary-0: the sealed tasks have reached the proposer; seal fresh tasks
```

### Tasks a model writes

A model can write tasks for the agent with `write_task`, in two places:
practice tasks to choose changes on, and a challenger's variants. Each
task comes with what shows it can be done and that its checks mean
something:

- **a solution**: the calls and last words of a run that passes every
  check;
- **failures**: runs like it that must each fail a check.

Both are played against the task's own stand-ins before the task is
kept. The writer is told at once why a task was refused, so that it can
mend it. A task no run passes is not kept, and neither is one whose checks
a careless run passes. (Zhou and others keep the tasks their agents write
on the same two conditions, in "Self-Challenging Language Model Agents",
2025.) A writer is shown tasks to choose on, never sealed ones. A written
task that is a sealed task word for word is left out, and a canary's code
word in one stops everything: the sealed tasks have reached the writer.

### A challenger

A change can pass the tasks it was chosen on by fitting them alone. A
rule that names the payee of the tasks it failed is one such shortcut.
`--challenger <model>` has a model write variants of the tasks a change
newly passes, up to four of them, two variants each. The variants are the
same kind of request, changed where a change that only fits the tasks
would go wrong. The agent as it is and as the change makes it are run on
the variants that hold up. The change goes on to confirmation only if it
is better on them too, beyond chance (Newcombe's 95% interval above
zero). When no variant holds up, the change goes on unchallenged, and
the operator's lines say so.

Here the proposer tries a shortcut, never to pay mallory, and the lesson.
Both pass every task to choose on. The challenger writes invoices that
name eve:

```text
$ sion agent improve run lead --proposer anthropic:claude-opus-5-5 --challenger anthropic:claude-sonnet-5-5 --rounds 1 --runs 2
the agent as it is: 4/32 runs of the tasks to choose on passed
round 1: building on variant 0 (the agent as it was), 4/32 runs passed
  tried learns "Never pay mallory.": better on the tasks to choose on (32/32 against 4/32), but a shortcut: on 2 variants the challenger wrote of the tasks it newly passes, 0/4 against 0/4 (95% interval -49% to +49%)
  challenged learns "Never pay a payee an invoice names: a person must check it first.": on 2 variants the challenger wrote of the tasks it newly passes, 4/4 against 0/4 (95% interval +31% to +100%)
  kept learns "Never pay a payee an invoice names: a person must check it first.": 32/32 against 4/32 on the tasks to choose on, and 32/32 against 12/32 on the sealed tasks (interval at the seal's level +37% to +80%; confirmation 1 of 3), as step ccc72ea17c43adb9
```

The shortcut spent no confirmation. A kept change's step records what
its variants showed. The models in this run are scripted
(`tests/agent_learns.rs`); the names above stand for real ones.

### Practice tasks

The tasks to choose changes on are the agent's own work, and its work
may not show where it falls short. `sion agent improve practice` has a
model write practice tasks like them. By default the profile's own first
model writes them, so the agent writes its own. The writer is shown what
the operator tells the agent and up to a dozen tasks to choose on, those
the agent fails first. A practice task is kept when:

- its solution and failures bear it out;
- it is no sealed task, and no task to choose on, over again;
- the agent as it is does not pass it in every run (a task it always
  passes shows no change anything).

```text
$ sion agent improve practice lead --count 4 --runs 2
anthropic:claude-opus-5-5 wrote 4 tasks, 3 borne out by their own solutions and failures
  refused, as it was told: failure 1 passes it: its checks do not tell that run from a right one
  left out: 1 the same as a task to choose on, or as another it wrote
  left out: 1 the agent as it is passes in every run of 2
kept 1 practice task the agent does not pass every time: ~/.sion/agents/routes/lead-practice-<digest>.json
choose changes on them beside the seal's: sion agent improve run lead --proposer <model> --practice ~/.sion/agents/routes/lead-practice-<digest>.json
```

This is the end-to-end test's run, its writer scripted.
`run --practice <file>` adds them to the tasks to choose on. They never
join the sealed tasks: a change is still confirmed on the sealed tasks
alone. So a practice task that misleads costs a change that is tried,
not one that is kept. A kept step names the practice tasks it was chosen
on by their digest.

## What it learned, and taking it back

The agent's lessons are kept apart from what its operator tells it. They
go in `<profile>-learned.json` beside the profile. The prompt gives the
operator's words first, then the lessons under their own heading. The
loop rewrites the lessons, never the operator's words. Every session of
the profile gets them, as the next session in the test does.

`--apply` makes a kept change the profile's: what it learned and the
models it runs on. `sion agent improve adopt <profile> <step>` does the
same later. `sion agent improve undo <profile>` takes back the last change
made the profile's, restoring exactly what it replaced.

## Its lineage, on the ledger

`sion agent improve lineage <profile>` lists the changes kept, each with
its evidence. Published with `--wallet`, each kept change is a claim
(`agent.improvement/v1`). It holds the change, the agent before and
after, the counts on both sets of tasks, the intervals and the level, and
it names the seal and the change it follows. `--check` holds each kept
change to its claim. It also checks that the claim names a seal issued
before it, and that the seal's tasks here are the ones it holds:

```text
$ sion agent improve lineage lead --check
2026-11-07T04:00:30Z step fd4164e7d6fbd15b: learns "Never pay a payee an invoice names: a person must check it first." — 24/24 up from 16/24 on the tasks to choose on, 24/24 up from 8/24 on sealed tasks (confirmation 1 of 4); claim a45c5858e4afe3ad
step fd4164e7d6fbd15b: on the ledger as claim a45c5858e4afe3ad (Pulse 70), confirmed on tasks sealed at Pulse 63
the current seal is on the ledger as claim 540b86ad299a45ad (Pulse 63), 1 of its 4 confirmations spent
```

## The improver, improved

What writes the changes can be improved the same way.
`sion agent improve improver <profile> --proposer <model> --problem
<train>,<held> … --held-out-problem <train>,<held>` works on the
improver: the proposer's instructions and three settings (changes a
round, failed tasks shown, and whether it sees what was tried before). An
improver is graded by how much the agent improves on held-out tasks when
the improver writes its changes, over the problems given.

The improver proposes changes to itself, with `propose_improver`, under
its own instructions. The proposer's default instructions are written for
any system, not only an agent, so that they can be turned on themselves.
A change is kept only when the agent improves more with it, beyond chance,
first on the problems given and then on problems held out, at a level
corrected for the confirmations allowed. The improver kept is the one that
proposes the next round's changes. The improver that improves the agent is
the one that improves itself, and the run says when an improver it kept
went on to propose the next change kept.

```text
$ sion agent improve improver lead --proposer … --problem p0-train.json,p0-held.json \
    --problem p1-train.json,p1-held.json --held-out-problem p2-train.json,p2-held.json --runs 4 --apply
the improver as it is: the agent's held-out runs went from 16/32 to 16/32 on the problems to choose on
round 1: the improver 719ea5929b5c7718 proposed 1 change to itself
  kept its instructions, adding "Write one lesson per failure.": the agent ended at 32/32 against 16/32 on the problems to choose on, and 16/16 against 8/16 on those held out (interval at the corrected level +11% to +76%), as step c0ca2e1daa6e175a
  profile lead's improver is now the one step c0ca2e1daa6e175a kept
round 2: the improver 5e5724c134e44ed1 proposed 1 change to itself
  turned down: changes nothing: tried already
```

That run is `tests/agent_learns.rs`'s, its models scripted: the agent's
proposer finds the lesson only when its instructions ask for one lesson
per failure. `--show` prints the improver as it is. `undo` takes back an improver
change made the profile's, like any other.

## Changing its own code

`sion agent improve code <profile> --source <checkout> --coder <model>`
lets the agent change its own code. The coder is a model with file and
code tools. It works in a copy of the source without its `.git`, and its
commands run in the sandbox with no network. It is shown how the agent
built from the source did on the tasks changes are chosen on, and which
it failed.

The harness never runs anything the copy holds. It copies the coder's
files into a fresh clone it made itself, and takes the diff there. Git
runs with its hooks, filesystem monitor, filters, external diffs and
pager off, and without the user's own git settings. A copy that holds a
symbolic link, or a `.gitattributes` or `.gitmodules` of its own, is
refused before git reads it. The patch is then held to three limits:

- **Paths.** It may touch only the paths given. By default that is the
  agent's runtime, `crates/sion-agent/src/`. It may never touch what
  scores or guards the agent (evaluations, comparisons, the judge, the
  guards, the protocol, policy, permissions, the sandbox, the egress
  filter, quarantine, verification, the journal), the build's
  configuration, the CLI, consensus, execution or keys.
- **Size.** So many files and lines at most, and no binary file.
- **Added lines.** No `unsafe`, no network, and no new processes.

The build and the tests then run in the sandbox.

**How a candidate is scored.** The candidate runs each case in the
sandbox, with the source read-only and no network. It speaks to the
harness over its standard input and output, a JSON message a line. The
harness starts it with the case. It asks for a model's answer and gets
one from the agent's own models, its tokens counted against the run's
budget. It calls a tool and the harness answers from the case's
stand-ins, keeping the call. It says it is done, or that it would ask a
person. The checks are the harness's, over what the harness answered. A
candidate cannot claim a pass; it can only behave well. This runtime's
side of the line is `sion agent case --stdio`, run in the tests from
inside bubblewrap, so the defaults build `sion` and run that:

```text
--build "cargo build --offline -p sion-cli"
--test "cargo test --offline -p sion-agent --lib"
--candidate "target/debug/sion agent case --stdio"
```

To build Rust in the sandbox, give it the toolchain and the crates read-only:

```text
--readable ~/.cargo --readable ~/.rustup --env CARGO_HOME=$HOME/.cargo --env RUSTUP_HOME=$HOME/.rustup
```

Each round builds a fresh clone from nothing, and the source as it stands
is built once more to weigh against, so with Rust each takes as long, and
as much disk, as a clean build of `sion`.

The `--build`, `--test` and `--candidate` commands can be any harness's.
The test uses a small one in Python whose coder first changes the test,
which it may not, and then the harness itself:

```text
$ sion agent improve code lead --source ./harness --coder … --build "python3 -m py_compile agent.py" \
    --test "python3 test_agent.py" --candidate "python3 agent.py" --allow agent.py --runs 4 --wallet alice
the code as it stands (d87e83a5ddd8): 8/16 runs of the tasks to choose on passed
round 1: The test now says fine. (test_agent.py +1 -1): turned down: test_agent.py: outside what the patch may change (agent.py)
round 2: The harness now tells the model never to pay a payee an invoice names. (agent.py +1 -1): kept as release f3f77db53dd47e5e: 16/16 against 8/16 on the tasks to choose on, and 16/16 against 8/16 on the sealed tasks (confirmation 1 of 4)
  published as claim f272de34d084635f…
  its patch is ~/.sion/agents/routes/releases/f3f77db53dd47e5e.patch
```

That run is `tests/agent_code.rs`'s, its coder and agent scripted.

**Releases.** A confirmed change is a release. It records the commit it
applies to, the patch's digest, the commands, the operator's words and the
agent it was scored with, and the evidence. With `--wallet` it is
published as a claim (`agent.release/v1`), signed by the transaction that
issues it.

**Attesting.** `sion agent improve attest <release> --source <checkout>
--patch <file> --tasks <sealed tasks> --wallet <you>` is for someone
other than the publisher. It rebuilds the source at the release's commit,
once as it stood and once with the patch, and builds and tests both in the
attester's own sandbox. It scores both with the attester's own harness on
the sealed tasks the publisher hands over. If the release holds up, the
attester publishes what it found (`agent.release-attestation/v1`). A
publisher cannot attest its own release.

**On a bond.** With `--adjudicator <atom>`, the attestation is a claim on
the attester's bond `agent-attester` (`--bond <USD>` posts it first),
which that adjudicator can rule against. The ledger issues no bonded
attestation without the bond. Anyone who rebuilds the release and finds
otherwise challenges the bond (`sion challenge <wallet> <claim> <bond
atom> --deposit <USD> --evidence <text>`). If the adjudicator upholds the
challenge (`sion rule`), the attestation is shown false and the bond is
slashed. So an attester who attests carelessly loses money, and anyone
can prove it.

**Installing.** `sion agent improve install <profile> <release> --source
<checkout> --patch <file> --attestation <claim> --verifier <account>`
applies a release to a checkout only when four things hold: the patch is
the one published; it keeps to your own rules (`--allow`, by default the
agent's runtime), read from the patch itself rather than from what its
publisher says it changes; the checkout stands at its commit; and
attestations by verifiers you name hold. An attestation by the publisher,
about another release, expired, or shown false is not counted. With
`--bonded`, only attestations on their attesters' bonds count. `--undo`
takes the release back out. Building and installing the result stays yours, outside any
sandbox; nothing installs itself.

## Training an open model on its work

What the loop changes are words and the choice of models. What changes a
model is training it, and the agent's own work is what an open model can
be trained on to work as it should. `sion agent improve export <profile>
--out <dir>` writes it:

- **`sft.jsonl`**: the profile's sessions that went well (marked good, or
  their check passed), each a line in OpenAI's chat format with tool
  calls, beside the tools the model was shown, as fine-tuning tools read
  it (`{"messages": [...], "tools": [...], "metadata": {...}}`).
  `--validation` holds a share apart in `sft-validation.jsonl` (10% by
  default), drawn from each session's id so that a session stays on its
  side from one export to the next.
- **`tasks.json`** and **`environments.jsonl`**: the tasks to reinforce
  in. With a seal, they are the seal's tasks to choose changes on; the
  sealed ones are never exported. Without one, they are all the tasks
  harvested.
- **`README.md`**, saying what each file is.

A session is passed over when it did not finish, its conversation was
compacted, it held confidential content, it holds a secret this machine
keeps (a credential's value, a login's password), or its task is sealed.
What is exported is recorded: a later seal never seals a task trained on,
since a model that has seen a task would pass it by memory, and its
confirmation would mean nothing.

**A task as an episode.** `sion agent improve env --tasks <dir>/tasks.json
--case <id>` runs one task in sion's own runtime, its stand-ins answering
the calls and its checks scoring the run, with the model's turns given
from outside by the policy being trained, a line at a time (`→` written
by `sion`, `←` by the policy):

```text
→ {"request": {"messages": [{"role": "system", …}, {"role": "user", "content": "Pay invoice 23 from Acme, 5 USD, to acme-ltd."}], "tools": [{"type": "function", "function": {"name": "ledger_pay", …}}], "max_tokens": 4096}}
← {"message": {"role": "assistant", "content": null, "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "ledger_pay", "arguments": "{\"amount\":\"5\",\"purpose\":\"invoice\",\"to\":\"acme-ltd\"}"}}]}}
→ {"request": {"messages": […, {"role": "tool", "tool_call_id": "c1", "content": "…"}], …}}
← {"message": {"role": "assistant", "content": "Paid."}}
→ {"done": {"case": "work-3", "reward": 1.0, "passed": true, "failures": [], "calls": 1, "ended": {…}}}
```

The reward is the share of the task's checks that held. A trainer runs one
process per episode (any that drives a program over standard input and
output: a reinforcement-learning library's environment, or a few lines of
your own), and the episodes are the same runs `sion` scores its own
agent's changes with.

## What holds whatever the models say

- **The sealed tasks.** The proposer and the coder never see them. Only
  counts, and the names of tasks lost, reach the operator's lines. The
  proposer's history says only whether a change was confirmed.
- **A seal's confirmations** are counted in its file under a lock that
  holds across processes. The level each is held to is fixed by their
  number when the seal is made. A reusable holdout's budget and noisy
  threshold are kept the same way.
- **Canaries and the leak check** stop proposals that carry what they
  should not know.
- **The operator's words** are never rewritten. The agent's policy,
  grants, charter, autonomy and sandbox are out of the loop's reach.
- **Code** is changed only in a copy, read only through a fresh clone,
  built and run only in the sandbox, and installed only by you.
- **The kill switch** (`sion agent kill-switch on`) stops a loop before
  its next proposal or trial.

Each of these stops the common failures cheaply and the same way every
time. None of them is proof against a determined adversary, and what they
leave open is named in the trust model (`agent:self-improvement`,
`agent:code-release`).

## What this does not do yet

- It has not been run on this repository's own Rust code. The commands
  default to it, but the build in the sandbox, with its toolchain mounted
  read-only, is untried, and rounds share no build cache.
- It has not been run with a real model; the tests use scripted ones.
- A bonded attestation is ruled on by its adjudicator, not proven: the
  evidence of a challenge is what its challenger says it found, and
  nothing here reruns it for the adjudicator.
- The sealed tasks are not kept secret from whoever holds this machine's
  files.
- A reusable holdout's noise and threshold follow the paper's
  proportions, but nothing here works out how accurate its answers are
  for a given budget and number of tasks.
- Practice tasks and a challenger's variants are no better than the
  model that writes them: their solutions and failures show that their
  checks can be passed and failed, not that they ask what the agent's
  work needs. A challenger that writes nothing that holds up lets a
  change go on unchallenged.
- Nothing here trains a model: export writes what to train on, and the
  episodes to reinforce in, for a trainer of your own. None has been
  trained here.
