# Task Charters from the command line

This guide posts a task, has two reviewers attest a worker's output, pays
the worker for a result that passes the rule fixed at posting, and
withdraws a bounty nobody took. It assumes a devnet is running
(`sion devnet`) and uses its deterministic wallet `alice` as the poster.
For background, see [Task Charters](../concepts/task-charters.md).

## 1. Wallets and bonds

```bash
for w in vendor arbiter coder reviewer-1 reviewer-2; do sion keys new $w; done
sion faucet vendor 6000; sion faucet arbiter 3100; sion faucet coder 200
sion faucet reviewer-1 1100; sion faucet reviewer-2 1100

sion bond vendor environment 5000                     # behind the environment's claim
sion bond arbiter arbitration 3000 --class adjudicator
sion bond coder task-worker 100                       # every worker needs a task bond
sion bond reviewer-1 verifier 1000
sion bond reviewer-2 verifier 1000
```

Each `sion bond` prints the atom it backs. Keep the arbiter's: it is the
adjudicator results must name.

## 2. The environment's claim

```bash
sion claim issue vendor "swe-sandbox-7 has no known exploit that passes grader g-3" \
  --schema environment.no-known-exploit/v1 --bond environment --adjudicator <arbiter atom>
```

The vendor stands behind its environment with 5,000 USD. The claim joins
the knowledge log at the end of the Pulse it is issued in.

## 3. Post the task

```bash
echo "Fix issue #4711: the parser rejects trailing commas." > task.md
sion task post alice 150 --worker coder --spec task.md \
  --requires <environment claim> --verifiers reviewer-1,reviewer-2 \
  --adjudicator <arbiter atom> --deadline 500
```

```text
alice signs: Escrow 150 USD for a task for coder, paid to them for a result resting on 1 required claim and accepted under your policy, settled by Pulse 527. Fee 0.0011 USD; valid until Pulse 47.
Pulse 30: Done: task 19148c3d posted with 150 USD in escrow, for coder, until Pulse 527; fee 0.0011 USD.
task 19148c3d13640e4fc44902aaa3683357e99502eb023f5a606a79ff7e3f026227
```

The rule is fixed now: a result must rest on the vendor's claim and on the
coder's bond or a reviewer's attestation, name the arbiter, and pass
alice's policy at κ = 2 (the default; `--kappa` changes it). `--reliance`
sets how long alice relies on a paid result (1,000 Pulses by default).
Without `--worker`, the task is a bounty anyone may settle. With `--agent`
and `--allotment`, an agent posts from its own budget, under its caveats.

## 4. Attest and submit

```bash
printf 'diff --git a/parser.rs b/parser.rs\n' > patch.diff
sion task attest reviewer-1 <task> patch.diff
sion task attest reviewer-2 <task> patch.diff
sion task submit coder <task> patch.diff --attestations <attestation 1>,<attestation 2>
```

Each command issues a claim and prints its id. Attestations rest on each
reviewer's `verifier` bond; the result carries exactly the formula the
task requires, over the attestations in the order given. All three commit
to the task and to the exact bytes of `patch.diff`. Wait one Pulse after
the attestations before submitting: a claim can reference only claims
already in the log.

## 5. Settle

```bash
sion task settle coder <task> <result> --attestations <attestation 1>,<attestation 2>
```

```text
Pulse 45: Done: task 19148c3d settled: 150 USD paid to coder for result 2a3404ed, its backing reserved; fee 0.0012 USD.
```

The ledger checked that the result commits to this task, that its formula
is the one the task requires, that both attestations are by named reviewers
and attest the same output, and that the cost of corrupting the result
covers κ times 150 USD under alice's policy. It reserved 300 USD of the
result's backing for alice for the reliance period. Settling again fails:

```text
Pulse 48: Failed (invalid transaction: the task is not open); nothing happened except the fee of 0.0012 USD.
```

`sion task show <task>` prints the task's terms and its state, here
`Settled`, with the result and the reservation.

## 6. Withdraw a bounty

```bash
sion task post alice 30 --requires <environment claim> --adjudicator <arbiter atom> --deadline 500
sion task cancel alice <bounty>
```

```text
Pulse 54: Done: task e14ba3b6 cancelled: 30 USD refunded to alice; fee 0.0011 USD.
```

A task with a named worker cannot be withdrawn before its deadline: only
the worker may release it, so a worker can count on the work it took.

## If the environment turns out to be exploitable

Anyone who finds an exploit challenges the vendor's claim
(`sion challenge`), and the arbiter rules (`sion rule --upheld`). The
vendor's bond is slashed, its claim and every result resting on it are
falsified, and alice is compensated from the slashed bond for what she
paid. `sion demo tasks --local` runs this end to end.
