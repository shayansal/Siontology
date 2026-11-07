# Checked before it is done

A model that says it has finished may not have. These are the checks a run
passes before it ends, and the nets under it when something goes wrong.

Outputs on this page are from real runs on a local devnet, long
identifiers shortened with `…`. The model is a **scripted** one, which
plays a fixed list of tool calls, because the environment these pages were
made in reaches no model provider: with a real model the runtime is the
same, and only `--model` changes.

## Checked before it is done

A model that says it has finished may not have. With `--verify <command>`
(and a shell, `--shell`), each time the model says it is done the command
runs with /bin/sh in the agent's sandbox, in its workspace, and an exit
status of 0 passes. A check that fails is shown to the model, the end of
what the command printed, and the run goes on:

```text
$ sion agent run agent-7 266036a8…32fe --task 'Write add.sh, which prints the sum of its two arguments.' --model script:verify.json --workspace work --shell --verify '[ "$(sh add.sh 2 3)" = 5 ] || { echo "sh add.sh 2 3 printed $(sh add.sh 2 3), not 5"; exit 1; }' --autonomy L3
session d076e6774d2e5abd for agent 266036a8
  → write_file {"content":"echo $(($1 - $2))\n","path":"add.sh"}
  ← wrote 18 bytes to add.sh
agent: add.sh is written: it prints the sum of its two arguments.
  ✗ The check of your answer failed, attempt 1 of 3. It reported:
    exit status 1
    --- stdout
    sh add.sh 2 3 printed -1, not 5
    Fix what it reports and finish again, or, if the task cannot be done, say so with cannot_complete.]
  → write_file {"content":"echo $(($1 + $2))\n","path":"add.sh"}
  ← wrote 18 bytes to add.sh
agent: Fixed: add.sh subtracted. It adds its two arguments now.
session d076e6774d2e5abd: done, and its check passed at attempt 2
```

After three failed checks (`verify_attempts`), the run ends unverified,
with the best of the answers the model gave: the one whose check printed
the highest last line `score: <0 to 1>`, for a check that passes by
degrees, or the later of two that scored the same. The model is also
given `cannot_complete`, to say that the task cannot be done and why: the
run then ends there, as not done, rather than with an answer made to look
finished:

```text
$ sion agent run agent-7 266036a8…32fe --task 'Make the integration tests pass.' --model script:cannot.json --workspace work2 --shell --verify 'sh tests/integration.sh' --autonomy L3
session 9e7ffee20a4ae380 for agent 266036a8
  → shell {"command":"psql -h db.internal -c 'select 1'"}
  ← exit status 2
    --- stderr
    psql: error: could not translate host name "db.internal" to address: Temporary failure in name resolution
  → cannot_complete {"why":"the integration tests need the Postgres server db.internal, and commands here have no network"}
  ← Noted: the run ends here, as not done.
session 9e7ffee20a4ae380: the agent cannot complete the task: the integration tests need the Postgres server db.internal, and commands here have no network
```

The check runs where the agent works, so the agent can change what it
reads or runs there (a test, a script). A check's own files are kept out
of its reach with `--verify-files <dir>`: a directory apart from the
workspace and the skills, bound into the sandbox read-only at `/check`
while the check runs, and only then, so the agent's commands neither see
nor change it. Here the cases are the check's, and the agent cannot list
them:

```text
$ sion agent run agent-7 6a3e150a…6549 --task 'Write sum.sh, which prints the sum of the numbers it is given.' --model script:files.json --workspace work --shell --verify 'sh /check/check.sh' --verify-files hidden --autonomy L3
session ce6f69e7ab63b2f5 for agent 6a3e150a
  → shell {"command":"ls /check"}
  ← exit status 2
    --- stderr
    ls: cannot access '/check': No such file or directory
  → write_file {"content":"echo $(($1 + $2))\n","path":"sum.sh"}
  ← wrote 18 bytes to sum.sh
agent: sum.sh is written: it prints the sum of the numbers it is given.
  ✗ The check of your answer failed, attempt 1 of 3. It reported:
    exit status 1
    --- stdout
    sh sum.sh 1 2 3 4 printed '3', not 10
    Fix what it reports and finish again, or, if the task cannot be done, say so with cannot_complete.]
  → write_file {"content":"s=0\nfor n in \"$@\"; do s=$((s + n)); done\necho $s\n","path":"sum.sh"}
  ← wrote 49 bytes to sum.sh
agent: Fixed: sum.sh added only its first two numbers. It adds them all now.
session ce6f69e7ab63b2f5: done, and its check passed at attempt 2
```

What the check still reads from the workspace (the work itself, a test
runner's settings, a module it imports) the agent can change, so a check
that passed shows that its command exited with status 0 on what the
agent left there, not that the work is right (`agent:verifier`). A judge that reads
a session's steps rather than its result is in
[Judged step by step](graphs.md#judged-step-by-step). Work
another agent is paid for is checked by someone else, with a check its
task fixed when it was posted
([Task Charters from the command line](../guides/tasks.md#9-a-check-fixed-when-the-task-is-posted)).

## Held to its plan

An agent keeps a plan for a task of several steps with `plan_update`: the
whole plan each time, its steps in order, one active at a time. A step is
done only with how it was checked (`checked`: the test that passed, the
output that showed it) and dropped only with why (`why`), and a step open
in the last plan is not left out of the next without a word. What the
agent is shown back is the plan, marked:

```text
The plan, 1 of 3 steps done:
1. [x] Reproduce the failure (checked: the test failed as reported)
2. [>] Fix the parser
3. [ ] Run the whole suite
```

Whoever watches sees where it stands (`plan: 1 of 3 steps done; now: Fix
the parser`, as a chat's draft shows it), a run's last lines show the
plan, and every summary of a compacted conversation carries it as it was
last written. When the agent says it is done with steps still open, it is
reminded of them, once a run: to do them, to drop them and say why, or,
if it is waiting for the person, to say what it needs from them. The
reminder takes a turn of the run's; a run with none left ends without it.
`--no-plan` (on `sion agent run` and on profiles) gives the agent no plan
to keep.

An agent that can run something (a shell, terminals, `run_code`,
`run_tests`, `diagnostics`), changed code and ran nothing since is asked
once, when it says it is done, to check the change: its tests, a build,
running it, or to say why it cannot. Code is what a file's name says is
code or builds it (`.rs`, `.py`, `.ts`, `Cargo.toml`, a `Makefile` and the
like), not prose or data (`.md`, `.txt`, `.csv`). With `--verify` the
check does that, and there is no reminder; `--no-change-check` lets the
agent finish without one. Neither reminder makes the work right: it asks
the model once, and what the model does then is the model's.

## Safety nets

- **No accidental repeats.** A call that changed something is recorded
  under a key made of the session, the tool and the input. The same call
  again returns the recorded outcome, saying so, instead of paying twice.
  Calls meant to run again are not recorded: shell commands, skills'
  scripts and file writes.
- **Nothing lost in a crash.** A call that changes something is written
  down before it runs, and the session is kept on disk then, after
  whatever the call notes on the way, and after it ends. A session resumed
  after a crash finds the call unknown, not forgotten: the turn it was in
  is answered from the journal, and the call is not made again until its
  outcome is known.
- **What "exactly once" means here.** A completed call is never repeated.
  A call whose outcome is unknown is repeated only by a tool that keeps
  its effect from happening twice, as each tool declares: one whose
  service drops a repeated key, or one that writes the same thing again,
  is made again; a payment notes its transaction's digest and the last
  Pulse it may execute in before it is sent, so the ledger says later
  what it did (executed: its receipt, and it is not paid again; past its
  last Pulse: it never will be, and it may be paid again; still pending:
  it waits); any other call waits for a person. `sion agent settle
  <session>` lists the calls whose outcome is unknown, and settles one:
  `--done <what it did>`, or `--not-done` so that it may be made again.
- **Bounded runs.** 12 model turns, 4 more after each turn in which a tool
  call succeeded, at most 32. A call that failed twice with the same input
  is blocked. Long conversations are cut down
  ([What a model is sent](models.md#what-a-model-is-sent)).
- **Going round in circles.** When a call gives the same result three
  times in a row, or two calls take turns with the same results for three
  rounds, the model is told so with the last result; two more, and the run
  stops. Here a model waits for a build by reading a file that does not
  change:

  ```text
  $ sion agent run agent-7 bdbdb742…60df --task 'Tell me when build 7 has finished.' --model script:poll.json --workspace work4
  session 857bcdcab22dae24 for agent bdbdb742
    → read_file {"path":"build-7.txt"}
    ← build 7: running
    → read_file {"path":"build-7.txt"}
    ← build 7: running
    → read_file {"path":"build-7.txt"}
    ← build 7: running

      [read_file gave the same result 3 times in a row: calling again will not change that. Change your approach, or say what stops you.]
    → read_file {"path":"build-7.txt"}
    ← build 7: running

      [read_file gave the same result 4 times in a row: calling again will not change that. Change your approach, or say what stops you.]
    → read_file {"path":"build-7.txt"}
    ← build 7: running
  session 857bcdcab22dae24: stopped going round in circles (read_file gave the same result 5 times in a row); `sion agent continue 857bcdcab22dae24 --say <more>` to go on another way
  ```

  A run's limits set the count (`stuck_after`; 0 turns the check off).
- **A budget of tokens.** `--budget-tokens <n>` (on `sion agent run` and
  on profiles) caps what each run may use, its subagents' tokens
  included. Before each model call, what it sends (estimated at a token
  for every three bytes) and the most it may answer must fit in what is
  left, besides a reserve of 2,000 tokens for a last answer. When they do
  not, the model is asked, within what is left, to say what it did, what
  it found and what remains, and a call it makes then is not made; the
  run ends with `its run's budget is spent`. The subagents of a
  turn share what is left, the reserve kept back, and what they used
  counts against it. The estimate is not a provider's count: some text,
  and pictures, cost more tokens than it says (`agent:token-estimate`).
- **A budget in dollars.** `--budget-usd <dollars>` (on `sion agent run`
  and on profiles) does the same in dollars, at the models' prices: before
  each model call, what it sends, priced as if none of it came from the
  provider's cache, and the most it may answer, at the output price, must
  fit in what is left, besides the reserve's tokens at the output price.
  Every model the run may use needs a price, or the run does not start.
  Anthropic's list prices for Claude models are built in, as its pricing
  page gave them on 8 November 2026; others come from
  `$SION_HOME/prices.json`, which takes the place of a built-in price for
  the same model, and `sion agent prices` lists them. Subagents are held
  to a token budget while they run, not to dollars: what they cost counts
  against the run's dollars when they report.
- **What a run cost.** A run ends with a line of what its session has used
  so far, its subagents included, and what that cost:

  ```text
  used 41230 tokens in 9 turns, 30188 of them read from the cache; cost $0.12
  ```

  The cost is counted from the tokens each provider reports, at the
  prices above: cache reads and (five-minute) cache writes at their own
  prices, and a prompt over a model's long-context threshold at its long
  price. A call to a model without a price is counted apart (`cost
  unknown (… tokens of models without a price)`), never guessed. `/usage`
  on a channel says the same. List prices are not what a provider bills
  under a contract or a discount (`agent:list-prices`).
- **Longer runs.** A run takes at most 32 turns, starting with 12 and
  gaining 4 with each turn whose tool calls succeed; each answer has at
  most 4,096 tokens, besides room to think. `--max-turns` and
  `--max-tokens` (on `sion agent run` and on profiles) raise or lower
  both, for tasks of hundreds of steps. A model call may take 300 seconds;
  `SION_MODEL_TIMEOUT` (seconds) changes that.
- **The kill switch.** `sion agent kill-switch on` stops every agent run
  on this machine before its next model or tool call:

  ```text
  $ sion agent kill-switch on
  kill switch on: every agent run stops before its next call (…/agents/STOP)
  $ sion agent run agent-7 bdbdb742…60df --task 'Pay bob 1 USD.' --model script:pay.json --autonomy L6
  session 8c993ac4876029d0 for agent bdbdb742
  session 8c993ac4876029d0: stopped by the kill switch (…/agents/STOP)
  $ sion agent kill-switch off
  kill switch off
  ```

  Reads that run side by side start together, so a switch thrown while
  they run stops the run before the call after them. The kill switch is
  local. To stop an agent's spending everywhere, suspend its identity on the ledger
  (`sion agent suspend`): every payment it then tries is refused, and so
  are its subagents'.
