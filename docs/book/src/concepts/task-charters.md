# Task Charters

A **task** escrows money for a piece of work, such as an evaluation run, a
batch of reinforcement-learning rollouts, a labelled dataset or a code
change, and fixes, when it is posted, the rule a result must pass to be
paid. The worker is paid once a result passes that rule, and only once.
If the result turns out to rest on something false, the party whose money
paid for it is compensated from the bonds that backed it.

Orchestrators such as Temporal, Ray or a lab's own agent harness already
run work well inside one organization. Task Charters are for the work that
crosses a trust boundary: environments, graders, evaluations, data and
compute bought from others, and agents that spend money on their behalf.
They do not run the work. They hold the money, the rule and the record.

## What is fixed when a task is posted

| Term | What it fixes | Example |
|---|---|---|
| `amount` | The escrow, paid in full for an accepted result | 150 USD |
| `worker` | Who may settle it, or nobody named: a bounty anyone may take | `coder` |
| `requires` | Claims every result must rest on | The environment vendor's bonded claim that `swe-sandbox-7` has no known exploit that passes grader `g-3` |
| `verifiers` | Whose attestations count | `reviewer-1`, `reviewer-2` |
| `must_attest` | Verifiers who must each attest every result (v1.15) | `reviewer-2` |
| `min_attestations` | How many attestations a result needs at least (v1.15) | 1 |
| `policy` | The poster's decision rule: whose assumptions it trusts, and κ | The standard policy, κ = 2 |
| `adjudicator` | Who rules on challenges to results | `arbiter` |
| `detection_permille` | The largest detection factor a result may declare | 900 |
| `reliance` | How long the poster relies on a paid result: its backing stays reserved, and compensation stays possible | 2,000 Pulses |
| `deadline` | The last Pulse a result may be settled | Pulse 2,024 |
| `spec` | A commitment to the task's description: its input, environment, grader, licence | the hash of `task.md` |

Neither side can change the rule after the work is done.

## A result, and what it rests on

A result is a claim by the worker, with schema `task.result/v1`, whose
payload commits to the task and to the output. Its formula must be exactly

```text
dual-signature ∧ (bond:worker ∨ attestation₁ ∨ … ∨ attestationₖ) ∧ required₁ ∧ … ∧ requiredⱼ
```

- **Integrity, where any honest party suffices.** The worker's own task
  bond (`bond:<worker>:task-worker`), or the attestation of any verifier
  the task names, that the same output passes (`task.attestation/v1`).
- **Validity.** Every claim the task requires, typically an environment
  vendor's bonded statement that its environment has no known exploit
  that passes the grader without solving the task.

The ledger builds this formula itself and compares, so a worker cannot
make a result look harder to fake by citing unrelated claims, and cannot
leave out the environment's. Settling then runs the
[decision rule](assurance/decision-rule.md) for the task's amount under the
poster's policy, and reserves κ times the amount of the result's backing
for the party whose money paid ([reservations](assurance/reservations.md)).

With bonds of 100 USD (the worker), 1,000 USD (each of two reviewers) and
5,000 USD (the environment), the cost of corrupting an attested result is
min(∞, 100 + 1,000 + 1,000, 5,000) = 2,100 USD; at a detection factor of
0.9 and κ = 2 it covers payments up to 945 USD. The worker's bond alone,
100 USD, covers 45 USD: the ledger refuses to pay 150 USD on it.

## Requiring attestations

The worker's own bond can be enough for the decision rule. A poster who
wants someone else to check the work says so when posting (specification
v1.15): `must_attest` names verifiers who must each attest every result,
and `min_attestations` how many attestations a result needs. The ledger
refuses a result without them, however large the worker's bond. It also
refuses an attestation by the result's own author, and a task whose named
worker is one of its verifiers: whoever wrote a result cannot be the one
who vouches for it.

The formula does not change. Required attestations sit in the same *or*
as the worker's bond and the other attestations, so their backings still
add up; requiring them makes sure that a verifier other than the author
checked the output, and that its bond is part of the result's backing.
In the `tasks` demo, the bounty requires reviewer-2's attestation, and the
ledger refuses the bounty hunter's result without it.

## A check fixed when the task is posted

A poster can also fix how results are checked.
`sion task post --check <dir>` (or an agent's `task_post` with a
`check`) takes a directory of files and a command, `sh check.sh` by
default, and the task's `spec` commits to them with its description:
it is the commitment to the description and to the check's own
commitment, which covers the command and every file's path and bytes.
Neither can change once the task is posted. The check is kept on the
poster's machine for the task's verifiers and is not handed to the
worker, which sees only its commitment.

A verifier runs it with `sion task verify`. The check is used only if it
is the one the spec commits to; it runs in a sandbox with no network, in
a fresh directory holding its files, with the output read-only at
`$OUTPUT`, and the output is attested, on the verifier's bond, only if
the check exits with status 0. A check is run by those who attest, so a
task with one must ask for an attestation (`must_attest` or
`min_attestations`): otherwise a worker's bond alone could settle it
unchecked.

What a worker is paid for is then judged by a check fixed before the
work starts, out of the worker's reach, and applied by someone other
than the worker. The ledger does not see a check run, only an
attestation: a verifier that attests without running it is caught as any
false attestation is, by a challenge its adjudicator upholds
(`agent:task-check`). A worker that cannot do the work as asked releases
the task before the deadline (`sion task cancel`, or an agent's
`task_release`, which says why): the escrow goes back, and the worker's
bond is untouched.

## When a grader turns out to be exploitable

Someone shows that the grader accepts a patch that deletes the failing
test. They challenge the vendor's claim, and its adjudicator upholds the
challenge:

1. The vendor's bond is slashed, and its claim is falsified.
2. Every result resting on it is falsified too: the formula references
   it.
3. Every reservation on those results is compensated from the slashed
   bond: the lab gets back what it paid for results built on the exploit.
4. The challenger gets its deposit back and a share of what remains: a
   bounty for the reward hack, paid by whoever sold the environment.

The `tasks` demo runs exactly this, with real transactions.

## Who can stop a task

| Situation | Who may cancel | Where the escrow goes |
|---|---|---|
| A worker is named and the deadline has not passed | Only the worker (it releases the task) | The poster, or the backer of the agent that posted it |
| No worker is named (a bounty) | The poster, any time until it is settled | The same |
| The deadline has passed | The poster | The same |

A worker named by a task can therefore count on it until the deadline,
whoever posted it: revoking the poster does not void a commitment to an
outside worker. A bounty, which nobody has committed to, can be
withdrawn.

## Budgets down a tree of agents

An agent posts tasks from its own allotment, under its caveats: the escrow
counts against its balance, its payees, its per-payment limit and its
token bucket, and every narrowing above it applies. An agent limited to
listed payees, by its caveats or by a narrowing, cannot post a bounty that
anyone may take. Since subagents' budgets are carved from their parents'
([agent identities](agent-identities.md)), a whole tree of agents posting
work can never spend more than its root was given, and the escrow of a
cancelled task goes back to the root's backer, whose money it was.

## The transactions

| Kind | Class | Spec |
|---|---|---|
| `PostTask { spec, acceptance, worker, amount, deadline, charter }` | Owned; bounded when an agent posts from its allotment | §5.42 |
| `SettleTask { task, result, attestations }` | Contended, in the assurance domain (it reserves backing) | §5.43 |
| `CancelTask { task }` | Contended, in the task's domain | §5.44 |

Results and attestations are ordinary claims (`IssueClaim`), so they enter
the knowledge log and can be proved from a light client. Both proof
stacks implement the three kinds, and the differential tests run every
rule and every way it can fail through both.

## In this implementation

- **Built**: tasks, acceptance fixed at posting, results bound to their
  task, the exact result formula, settlement with reservation, exactly-once
  payment, bounties, cancellation and refunds, posting from an agent's
  budget, compensation when a required claim is falsified (specification
  v1.10); required attestations, and no attestation by a result's author
  (v1.15); `sion task` commands, MCP tools and the `tasks` demo; checks
  fixed in a task's spec, kept from the worker and run by its verifiers
  before they attest (`sion task verify`), and, for agents, `task_post`
  with a check, `task_submit` with attestations and `task_release`.
- **Simplified**: settlement reserves backing in the assurance registry,
  which is one contended object, so settlements are ordered one after
  another.
- **Not built**: batches of outputs checked by beacon-chosen samples;
  assignment markets; automatic slashing for two signed outputs of one
  deterministic task; capability leases that cut a cancelled task's
  credentials at gateways; dataset manifests over result claims. They are
  designed in [Billions of agents](../research/agent-scale.md#5-task-charters-orchestration-with-priced-acceptance).
