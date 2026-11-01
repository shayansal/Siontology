# Task Charters from the command line

This guide posts a task, has two reviewers attest a worker's output, pays
the worker for a result that passes the rule fixed at posting, withdraws
a bounty nobody took, and fixes a check that a reviewer runs before it
attests. It assumes a devnet is running
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

## 7. Require an attestation

A worker's own bond can be enough for the decision rule. A poster who
wants someone else to check the work says so when posting (specification
v1.15): `--must-attest` names verifiers who must each attest every result,
and `--min-attestations` how many attestations a result needs. In a
separate run, with coder's task bond at 1,000 USD, enough on its own to
cover κ times 150 USD, alice posts a task that reviewer-1 must attest:

```text
$ sion task post alice 150 --worker coder --spec task.md --requires 90284b8c…de9b --verifiers reviewer-1,reviewer-2 --must-attest reviewer-1 --adjudicator e1d8032b…a12f --deadline 500
alice signs: Escrow 150 USD for a task for coder, paid to them for a result resting on 1 required claim, attested by reviewer-1 and accepted under your policy, settled by Pulse 527. Fee 0.0011 USD; valid until Pulse 47.
Pulse 29: Done: task d58c15cb posted with 150 USD in escrow, for coder, until Pulse 527; fee 0.0011 USD.
task d58c15cb87f89830361a5685872c07435603a5120fb840dd3f3d13e362b798a7
```

On its bond alone, coder's result is refused, however large the bond:

```text
$ sion task submit coder d58c15cb…98a7 patch.diff
coder signs: Publish the claim "result for task d58c15cb" (task.result/v1), valid for 1498 Pulses, resting on 3 assumptions and 1 other claims. Fee 0.0011 USD; valid until Pulse 49.
Pulse 31: Done: claim 5bb64d8b submitted; fee 0.0011 USD.
claim 5bb64d8bc995588bade3476384f7902edb64f84a0199ee5e13f2d987480d94e8
$ sion task settle coder d58c15cb…98a7 5bb64d8b…94e8
coder signs: Settle task d58c15cb with your result. Fee 0.0011 USD; valid until Pulse 53.
Pulse 35: Failed (invalid transaction: every verifier the task says must attest has to attest the result); nothing happened except the fee of 0.0011 USD.
```

With reviewer-1's attestation it is paid:

```text
$ sion task attest reviewer-1 d58c15cb…98a7 patch.diff
reviewer-1 signs: Publish the claim "the output for task d58c15cb passes" (task.attestation/v1), valid for 1492 Pulses, resting on 3 assumptions and 0 other claims. Fee 0.0011 USD; valid until Pulse 55.
Pulse 37: Done: claim 0d7c8d0f submitted; fee 0.0011 USD.
claim 0d7c8d0f00288b2edca265b8ed61ed52eea93a3808615c0641a4c91c5f23dd45
$ sion task submit coder d58c15cb…98a7 patch.diff --attestations 0d7c8d0f…dd45
coder signs: Publish the claim "result for task d58c15cb" (task.result/v1), valid for 1487 Pulses, resting on 3 assumptions and 2 other claims. Fee 0.0011 USD; valid until Pulse 60.
Pulse 43: Done: claim 636e495d submitted; fee 0.0011 USD.
claim 636e495d47925b2587ee3490572d60295e1d2cd6a5b49fbbf86c55b88624d3e6
$ sion task settle coder d58c15cb…98a7 636e495d…d3e6 --attestations 0d7c8d0f…dd45
coder signs: Settle task d58c15cb with your result and 1 attestation. Fee 0.0011 USD; valid until Pulse 65.
Pulse 47: Done: task d58c15cb settled: 150 USD paid to coder for result 636e495d, its backing reserved; fee 0.0011 USD.
```

## 8. Nobody attests their own result

In an open bounty a verifier may take the work like anyone else, but its
own attestation never counts: the ledger refuses a result that its author
attested.

```text
$ sion task post alice 30 --requires 90284b8c…de9b --verifiers reviewer-1,reviewer-2 --min-attestations 1 --adjudicator e1d8032b…a12f --deadline 500
alice signs: Escrow 30 USD for a task open to anyone, paid to the first worker for a result resting on 1 required claim, with at least 1 attestation and accepted under your policy, settled by Pulse 547. Fee 0.0011 USD; valid until Pulse 67.
Pulse 49: Done: task 2109898a posted with 30 USD in escrow, open to anyone, until Pulse 547; fee 0.0011 USD.
task 2109898acfa7542c1f6f9ea8516c91421014ab221fef6c64698e75dad48e7605
$ sion task attest reviewer-2 2109898a…7605 patch.diff
reviewer-2 signs: Publish the claim "the output for task 2109898a passes" (task.attestation/v1), valid for 1498 Pulses, resting on 3 assumptions and 0 other claims. Fee 0.0011 USD; valid until Pulse 69.
Pulse 52: Done: claim 611da3a1 submitted; fee 0.0011 USD.
claim 611da3a1a3a262cf65f668b51dee90e0545ce07ff6a77261036efe78109deb23
$ sion task submit reviewer-2 2109898a…7605 patch.diff --attestations 611da3a1…eb23
reviewer-2 signs: Publish the claim "result for task 2109898a" (task.result/v1), valid for 1493 Pulses, resting on 3 assumptions and 2 other claims. Fee 0.0011 USD; valid until Pulse 74.
Pulse 56: Done: claim 62d7864c submitted; fee 0.0011 USD.
claim 62d7864cf42b848927eb5f90b772f525aaaa9d56f05181258d5c818f32158e47
$ sion task settle reviewer-2 2109898a…7605 62d7864c…8e47 --attestations 611da3a1…eb23
reviewer-2 signs: Settle task 2109898a with your result and 1 attestation. Fee 0.0011 USD; valid until Pulse 78.
Pulse 60: Failed (invalid transaction: a result is never attested by its author); nothing happened except the fee of 0.0011 USD.
```

Nor can a task name its worker among its verifiers:

```text
$ sion task post alice 30 --worker reviewer-1 --requires 90284b8c…de9b --verifiers reviewer-1,reviewer-2 --adjudicator e1d8032b…a12f --deadline 500
alice signs: Escrow 30 USD for a task for reviewer-1, paid to them for a result resting on 1 required claim and accepted under your policy, settled by Pulse 560. Fee 0.0011 USD; valid until Pulse 80.
Pulse 62: Failed (invalid transaction: the task's worker cannot be one of its verifiers: nobody attests its own result); nothing happened except the fee of 0.0011 USD.
```

## 9. A check fixed when the task is posted

A poster can also fix how results are checked: `--check <dir>` takes a
directory of files and a command (`--check-command`, `sh check.sh` by
default), and the task's spec commits to them with the description
`--spec` gives. Neither can change once the task is posted. The check is
kept on this machine for the task's verifiers and is not handed to the
worker. It is run by those who attest, so a task with one must ask for
an attestation:

```text
$ sion task post alice 50 --worker coder --spec task.md --check check --deadline 500
error: a check is run by the verifiers who attest results, so a task with one must ask for an attestation: name a verifier who must attest every result, or how many attestations a result needs
[exit 1]
```

In a separate run, alice posts a task for `sum.sh`, whose check runs the
worker's script on cases of its own, and which reviewer-1 must attest:

```text
$ sion task post alice 50 --worker coder --spec task.md --check check --verifiers reviewer-1 --must-attest reviewer-1 --deadline 500
alice signs: Escrow 50 USD for a task for coder, paid to them for a result resting on 0 required claims, attested by reviewer-1 and accepted under your policy, settled by Pulse 512. Fee 0.0011 USD; valid until Pulse 32.
Pulse 14: Done: task 4b8d2f24 posted with 50 USD in escrow, for coder, until Pulse 512; fee 0.0011 USD.
task 4b8d2f24afcdc080734ef1a38dd70e3431f865faaa7e63ea634e9bfbfb7570d9
check 296edda8af89d60e81fe791798c81b22d4baec6361a64306ef78e86cfad63f31 (fixed in the task's spec, kept on this machine for its verifiers)
```

reviewer-1 runs the check with `sion task verify`, in a sandbox with no
network, in a fresh directory holding the check's files, with the output
read-only at `$OUTPUT`. A first version of the script fails it, and is not
attested:

```text
$ sion task verify reviewer-1 4b8d2f24…70d9 sum-v1.sh
error: verification failed: the check did not pass, so nothing was attested. It reported:
exit status 1
--- stdout
sum.sh 1 2 3 4 printed '3', not 10
[exit 1]
```

A check other than the one the task fixed is refused, before it runs:

```text
$ sion task verify reviewer-1 4b8d2f24…70d9 sum-v1.sh --check edited
error: verification failed: the check given is not the one task 4b8d2f24 fixed when it was posted: its spec commits to check 296edda8, and the one given is 547c5c71
[exit 1]
```

The script that passes is attested, on reviewer-1's verifier bond:

```text
$ sion task verify reviewer-1 4b8d2f24…70d9 sum.sh
the check passed:
exit status 0
--- stdout
all 4 cases pass
reviewer-1 signs: Publish the claim "the output for task 4b8d2f24 passes" (task.attestation/v1), valid for 1498 Pulses, resting on 3 assumptions and 0 other claims. Fee 0.0011 USD; valid until Pulse 35.
Pulse 17: Done: claim 423f172e submitted; fee 0.0011 USD.
claim 423f172e08399065b91888fe6218f56009b04ac6d11af28886cf5409bcc6d81a
```

With that attestation, coder's result is paid as in section 7:

```text
$ sion task submit coder 4b8d2f24…70d9 sum.sh --attestations 423f172e…d81a
coder signs: Publish the claim "result for task 4b8d2f24" (task.result/v1), valid for 1494 Pulses, resting on 3 assumptions and 1 other claims. Fee 0.0011 USD; valid until Pulse 38.
Pulse 21: Done: claim 2e352923 submitted; fee 0.0011 USD.
claim 2e3529234dfc9f77abeba6edb30694d1b955473f22d92bb1609a7e5bcf22f880
$ sion task settle coder 4b8d2f24…70d9 2e352923…f880 --attestations 423f172e…d81a
coder signs: Settle task 4b8d2f24 with your result and 1 attestation. Fee 0.0011 USD; valid until Pulse 42.
Pulse 25: Done: task 4b8d2f24 settled: 50 USD paid to coder for result 2e352923, its backing reserved; fee 0.0011 USD.
```

The check is kept under `$SION_HOME/agents/tasks` on the poster's
machine. A verifier elsewhere is given the description and the check's
files and runs `sion task verify … --spec task.md --check <dir>`
(`--check-command` if the task was posted with another command): the
command refuses them unless they are the ones the task's spec commits
to. The ledger sees the attestation, not the run, so a verifier that
attests without running the check is caught only as any false attestation
is, by a challenge its adjudicator upholds (`agent:task-check`).

## If the environment turns out to be exploitable

Anyone who finds an exploit challenges the vendor's claim
(`sion challenge`), and the arbiter rules (`sion rule --upheld`). The
vendor's bond is slashed, its claim and every result resting on it are
falsified, and alice is compensated from the slashed bond for what she
paid. `sion demo tasks --local` runs this end to end.
