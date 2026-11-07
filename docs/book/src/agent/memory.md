# Memory

What an agent keeps between sessions: notes in its memory log, which the
ledger lets only grow, its past sessions to search, and what consolidating
and dreaming over them makes, under gates.

Outputs on this page are from real runs on a local devnet, long
identifiers shortened with `…`. The model is a **scripted** one, which
plays a fixed list of tool calls, because the environment these pages were
made in reaches no model provider: with a real model the runtime is the
same, and only `--model` changes.

## Memory

`--memory` gives the agent its [memory log](../guides/memory.md), or `--memory new`
opens one for it. The agent records facts, episodes, procedures,
preferences, goals, plans and reflections as atoms in a log only it can
append to; when the session ends, the session itself is recorded as an
episode with its trajectory's digest, and the head is committed to the
ledger:

```text
$ sion agent run agent-7 ebc55841…ded9e --task "Remember that I like invoices paid on Fridays and that Acme's terms are net 30." --model script:learn.json --memory new --autonomy L3
Pulse 13: Done: memory log 391aa9d2 opened for agent ebc55841; fee 0.0012 USD.
memory log 391aa9d2092afc055d43c6c0751481fbaddfcf254509322df2ab295aad7a6386
session 2dfdae605077d42b for agent ebc55841
  → memory_remember {"kind":"preference","relation":"prefers","subject":"user","value":"invoices paid on Fridays"}
  ← remembered as atom 09517eb8 (1 atom(s) not committed to the ledger yet)
  → memory_remember {"kind":"fact","relation":"terms","subject":"supplier-acme","value":"net 30"}
  ← remembered as atom 1d49c69d (2 atom(s) not committed to the ledger yet)
  → skill_write {"description":"Pay a supplier's invoice after checking it against the order and the supplier's terms.","instructions":"1. memory_recall the supplier's terms.\n2. Check the invoice against the order.\n3. Pay it with ledger_pay, giving the invoice number as the purpose.","name":"pay-invoices"}
  ← wrote skill pay-invoices
agent: Noted: you like invoices paid on Fridays, and Acme's terms are net 30. I also wrote down how I pay invoices, as a skill.
session 2dfdae605077d42b: done
memory committed: Pulse 17: Done: memory log 391aa9d2 now holds 3 atoms (head 1); fee 0.0011 USD.
```

Preferences, goals and procedures are the agent's **core memory**: every
later session starts with them in its system prompt. Everything else is
recalled when needed. `memory_recall` with a subject and a relation returns
every atom of the committed log, proved complete against the index root on
the ledger, so the agent knows nothing was left out; the next session does
that before paying:

```text
$ sion agent run agent-7 ebc55841…ded9e --task "Pay Acme's invoice INV-1042 for 7.50 USD to bob, who collects for Acme." --model script:pay.json --memory 391aa9d2 --autonomy L6
session ac81c7b0646a2d4b for agent ebc55841
  → skill_read {"name":"pay-invoices"}
  ← ---
    name: pay-invoices
    description: "Pay a supplier's invoice after checking it against the order and the supplier's terms."
    ---

    1. memory_recall the supplier's terms.
    2. Check the invoice against the order.
    3. Pay it with ledger_pay, giving the invoice number as the purpose.
  → memory_recall {"relation":"terms","subject":"supplier-acme"}
  ← {"committed":[{"committed":true,"confidence_permille":1000,"id":"1d49c69d…","kind":"fact","seq":1,"statement":"supplier-acme terms \"net 30\"",…}],"complete":"proved complete against the index root on the ledger, as of 3 atoms","not_committed_yet":[],"relation":"terms","subject":"supplier-acme"}
  → ledger_pay {"amount":"7.5","purpose":"Acme invoice INV-1042 (net 30)","to":"bob"}
  ← Pulse 27: Done: agent paid 7.5 USD to bob (receipt claim 71125e1f); fee 0.0011 USD. Transaction 08bcdeb4….
agent: Checked Acme's terms (net 30) and paid invoice INV-1042, 7.50 USD.
session ac81c7b0646a2d4b: done
memory committed: Pulse 30: Done: memory log 391aa9d2 now holds 4 atoms (head 2); fee 0.0011 USD.
```

Its prompt ended with what it remembers and the skills it has:

```text
What you remember and always keep in mind (from your memory log; use memory_recall for more):
- (preference) user prefers "invoices paid on Fridays"

Skills you have (read one with skill_read before using it; its other files with skill_file):
- pay-invoices: Pay a supplier's invoice after checking it against the order and the supplier's terms.
```

The memory tools:

| Tool | Effect | What it does |
|---|---|---|
| `memory_remember` | write | Records a subject, a relation and a value, of a kind: fact, episode, procedure, preference, goal, plan or reflection |
| `memory_recall` | read | With a relation, every atom of the committed log about the subject, proved complete, and what is recorded but not committed yet; without one, a local scan |
| `memory_search` | read | Atoms whose subject, relation or value contain the words, newest first |
| `memory_forget` | write | Retracts an atom with a reason: the atom stays in the log, and recall and search leave it out |
| `memory_commit` | write | Commits the head now rather than when the session ends |
| `session_search` | read | The agent's past sessions on this machine that match the words, each checked against the episode its memory log recorded |

Nothing the agent recorded can be rewritten once committed: the ledger
refuses a head that does not extend the last one. That includes the
episodes, so what an agent did, and the digest of how, stays on record.

### Searching past sessions

An agent with a memory log can search its own past sessions on this
machine with `session_search`: BM25 over what it was asked, what it said
and called, and what its tools answered. Two sessions finished, each
recorded as an episode; a third is asked about Acme:

```text
$ sion agent run agent-9 2d3b5672…c93a --task "Who gets Acme's invoices?" --model script:third.json --memory 6fb40b5b…96de --autonomy L3
session b747ba719c7561c6 for agent 2d3b5672
  → session_search {"query":"acme invoices"}
  ← {"found":[{…,"record":"checked",…,"session":"93a2c9ef077e81fd","snippet":"From now on, pay Acme's invoices to acme-ltd, not acme-old.",…}],…}
agent: acme-ltd: you told me so in an earlier session, and that session is the one its episode on the ledger recorded.
session b747ba719c7561c6: done
memory committed: Pulse 20: Done: memory log 6fb40b5b now holds 3 atoms (head 3); fee 0.0011 USD.
```

Each hit gives the session, its task, the message that matched best and a
snippet, and `record`: how the session as stored stands against the
trajectory digest its episode recorded in the memory log, whose head is
on the ledger.

| `record` | Meaning |
|---|---|
| `checked` | Its first messages hash to the digest, and the episode is committed to the ledger |
| `uncommitted` | They hash to the digest, but the episode is not committed yet |
| `changed` | They do not: the stored session was edited after its episode was recorded |
| `compacted` | The session was compacted since: the messages the digest covers are not all kept |
| `unrecorded` | The log holds no episode of it: it has not finished, or ran without the log |

`sion agent sessions search` does the same from the command line, by the
agent's wallet name or id:

```text
$ sion agent sessions search agent-9 acme invoices
{
  "found": [
    {
      "matches": 2,
      "message": 0,
      "record": "checked",
      "record_detail": "its first 2 messages hash to the trajectory digest its episode (atom 0 of the memory log) committed to the ledger",
      "score": 1.11,
      "session": "93a2c9ef077e81fd",
      "snippet": "From now on, pay Acme's invoices to acme-ltd, not acme-old.",
      "task": "From now on, pay Acme's invoices to acme-ltd, not acme-old.",
      "who": "person"
    },
    {
      "matches": 4,
      "message": 0,
      "record": "checked",
      "record_detail": "its first 4 messages hash to the trajectory digest its episode (atom 2 of the memory log) committed to the ledger",
      "score": 1.09,
      "session": "b747ba719c7561c6",
      "snippet": "Who gets Acme's invoices?",
      "task": "Who gets Acme's invoices?",
      "who": "person"
    }
  ],
  "query": "acme invoices",
  "sessions_searched": 3
}
```

Edit a stored session, here the first one's answer, and its record no
longer checks out:

```text
$ sion agent sessions search agent-9 acme-old
{
  "found": [
    {
      "matches": 2,
      "message": 0,
      "record": "changed",
      "record_detail": "changed: the session as stored does not hash to the digest its episode recorded",
      "score": 2.0,
      "session": "93a2c9ef077e81fd",
      "snippet": "From now on, pay Acme's invoices to acme-ltd, not acme-old.",
      "task": "From now on, pay Acme's invoices to acme-ltd, not acme-old.",
      "who": "person"
    },
    {
      "matches": 4,
      "message": 2,
      "record": "checked",
      "record_detail": "its first 4 messages hash to the trajectory digest its episode (atom 2 of the memory log) committed to the ledger",
      "score": 1.36,
      "session": "b747ba719c7561c6",
      "snippet": "…the ledger\",\"score\":1.63,\"session\":\"93a2c9ef077e81fd\",\"snippet\":\"From now on, pay Acme's invoices to acme-ltd, not acme-old.\",\"task\":\"From now on, pay Acme's invoices to acme-ltd, not acme-old.\",\"who\":\"person\"}],\"query\":\"acme invoices\",\"sessions_searched\":2}",
      "task": "Who gets Acme's invoices?",
      "who": "tools"
    }
  ],
  "query": "acme-old",
  "sessions_searched": 3
}
```

An episode records how many messages its digest covers and how many
times the session had been compacted, so a session that went on after its
episode is checked on the messages the episode covered.

### Consolidation

`sion memory consolidate` reads the episodes recorded since the last
consolidation and the notes the log keeps, and asks a model what is worth
keeping for good: facts, procedures and preferences, each citing its
episodes, proposed with the `memory_note` tool. Gates keep a note only if
it cites episodes it was shown and nothing else, replaces only notes it
was shown, is a fact, a procedure or a preference, fits the limits (120
bytes for a subject or a relation, 1,000 for a value), is not one the
agent keeps already, and is one of at most 16 in the run. Here the model
is scripted, and one of its proposals cites an episode that does not
exist:

```text
$ sion memory consolidate agent-9 6fb40b5b…96de --model script:consolidate.json
agent-9 signs: Commit memory log 6fb40b5b at 6 atoms, proving it only grew. Fee 0.0011 USD; valid until Pulse 41.
{
  "committed": "Pulse 23: Done: memory log 6fb40b5b now holds 6 atoms (head 4); fee 0.0011 USD.",
  "consolidated_through": 2,
  "episodes": 3,
  "episodes_left": 0,
  "kept": [
    {
      "atom": 3,
      "confidence_permille": 750,
      "from": [
        "atom 0",
        "atom 2"
      ],
      "kind": "fact",
      "note": "supplier-acme invoices-paid-to: acme-ltd",
      "replaces": [],
      "revision": null,
      "untrusted": false,
      "why": "the person said so, and it was relied on later"
    },
    {
      "atom": 4,
      "confidence_permille": 700,
      "from": [
        "atom 1"
      ],
      "kind": "preference",
      "note": "user prefers: plain answers about what I cannot do",
      "replaces": [],
      "revision": null,
      "untrusted": false,
      "why": "how the weather question went"
    }
  ],
  "log": "6fb40b5b…96de",
  "marker": 5,
  "model": "script:consolidate.json",
  "notes_shown": 0,
  "size_on_ledger": 6,
  "turned_down": [
    {
      "proposal": {
        "kind": "fact",
        "relation": "is",
        "sources": [
          "e7"
        ],
        "subject": "acme",
        "value": "a supplier",
        "why": "no episode says so"
      },
      "reason": "e7 is not one of the episodes shown"
    }
  ]
}
```

Each note kept is a derived atom with a `derived_from` link to every
episode it cites and an extraction trace naming the model and the
consolidator, recorded with a confidence that comes from where it came
from, not from what the model says of it (`confidence_permille`): 700‰
from a trusted session (300‰ if what it cites read content from outside),
50‰ more for each further episode it cites, up to three, and never above
950‰. A note that replaces an older one comes with a revision
that supersedes it: the older one stays in the log, and recall, search
and the prompt leave it out. A summary atom records how far the episodes
were consolidated, so the next run reads only what came after:

```text
$ sion memory consolidate agent-9 6fb40b5b…96de --model script:consolidate.json
{
  "consolidated": 0,
  "log": "6fb40b5b…96de",
  "note": "nothing to consolidate: no episode after atom 2"
}
```

The preference is core memory now, in every later session's prompt:

```text
What you remember and always keep in mind (from your memory log; use memory_recall for more):
- (preference) user prefers "plain answers about what I cannot do"
```

### Dreaming

`sion memory dream` reads the sessions themselves, where consolidation
reads their episodes, a line or two each: the sessions of the episodes
recorded since the last dream, those on this machine (12 at most a run),
each read whole by a reader of its own, a model call without tools that
is told the transcript is data, four at a time. A reader proposes facts,
procedures and preferences, each with the words of the session that show
it. Before anything is kept, those words must be in the session, as they
are written there (spacing aside) and 12 characters at least, and a
session that read content from outside yields facts only, kept as a
prompt-injection risk; then consolidation's gates and its recording take
over, each note citing its session's episode. Here one session paid an
invoice for a person who asked to see that payee's invoices first, and
the other read a web page; the readers are scripted, each answering for
its own session:

```text
$ sion memory dream agent-7 ae444881 --model script:dream.json
agent-7 signs: Commit memory log ae444881 at 5 atoms, proving it only grew. Fee 0.0011 USD; valid until Pulse 35.
{
  "committed": "Pulse 17: Done: memory log ae444881 now holds 5 atoms (head 3); fee 0.0011 USD.",
  "dreamt_through": 1,
  "into": null,
  "kept": [
    {
      "atom": 2,
      "confidence_permille": 400,
      "from": [
        "atom 1"
      ],
      "kind": "fact",
      "note": "tiny_http latest release: 0.12.0",
      "replaces": [],
      "revision": null,
      "untrusted": true,
      "why": "the session says: \"tiny_http's latest release is 0.12.0\""
    },
    {
      "atom": 3,
      "confidence_permille": 800,
      "from": [
        "atom 0"
      ],
      "kind": "preference",
      "note": "user wants to see first: bob's invoices, before they are paid",
      "replaces": [],
      "revision": null,
      "untrusted": false,
      "why": "the session says: \"From now on, show me bob's invoices before you pay them.\""
    }
  ],
  "log": "ae444881…06fb",
  "marker": 4,
  "model": "script:dream.json",
  "sessions_elsewhere": [],
  "sessions_read": 2,
  "sessions_unread": [],
  "size_on_ledger": 5,
  "skill_drafts": [],
  "test": null,
  "tokens": 0,
  "turned_down": [
    {
      "proposal": {
        "kind": "fact",
        "quote": "bob takes bank transfers only",
        "relation": "is paid",
        "subject": "bob",
        "value": "by bank transfer only"
      },
      "reason": "its quote is not in the transcript"
    },
    {
      "proposal": {
        "kind": "procedure",
        "quote": "tiny_http's latest release is 0.12.0",
        "relation": "are looked up",
        "subject": "crate versions",
        "value": "on docs.rs, the crate's latest page"
      },
      "reason": "the session read content from outside, so it yields facts only, not a procedure"
    }
  ]
}
```

The second note proposed for the first session quotes words no one said
in it, and the procedure from the second is turned down because that
session read a page. A summary atom marks how far the dream went
(`memory dreamt-through`), so the next one reads only what came after:

```text
$ sion memory dream agent-7 ae444881 --model script:dream.json
{
  "dreamt": 0,
  "log": "ae444881…06fb",
  "note": "nothing to dream over: no episode after atom 1"
}
```

A note kept from a dream shows that its words were said in its session,
by the person, a tool or the agent itself, not that what it says is true
(`agent:dream-quote`). Its confidence is a consolidated note's, 100‰ more
for the quote its session shows.

#### Tested before it is kept

`--eval <file>` runs a set of held-out tasks (as
[`sion agent graph eval`](graphs.md#measured-against-one-agent)
takes them) before anything is recorded: each case with the core memory
as it is, and as it would be with the dream's notes in mind, `--eval-runs`
times each (with `--eval-model`, or the reader). The notes are recorded
only if no case passes fewer times with them; a case whose task is in a
session the dream read is not held out, and is refused. Here two
sessions failed the same way, and the dream proposes a procedure. The
held-out case is worked on by a scripted model that answers differently
when the procedure is in its prompt (`when` matches it), and with the
procedure the case's check no longer passes:

```text
$ sion memory dream agent-7 adaaa663 --model script:dream.json --eval done.json --eval-model script:worker.json --eval-runs 2
{
  "log": "adaaa663…1ed8",
  "model": "script:dream.json",
  "proposed": [
    "forgetting starts by: searching for the note before forgetting it"
  ],
  "recorded": false,
  "test": [
    {
      "case": "forget-a-note",
      "passed_with": 0,
      "passed_without": 2,
      "runs": 2
    }
  ],
  "tokens": 0,
  "turned_down": [],
  "why": "with the dream's notes in mind, 1 held-out case(s) passed fewer times: forget-a-note; nothing was recorded, and the sessions are left to dream over again"
}
$ sion memory open curator
curator signs: Open a memory log that only the sender can append to. Fee 0.0012 USD; valid until Pulse 37.
Pulse 19: Done: memory log 6f88e2cb opened; fee 0.0012 USD.
log 6f88e2cb2ce22e66e6a8340f798bf0194d2d2b5847f793ff254c83d0a81b4361
```

The sessions are left to dream over again. The test treats every note as
kept in mind, facts too, which is the most a note could steer the agent,
and it tests only what its cases check (`agent:dream-test`).

#### A curated log that only dreams write

`--into <log> --curator <wallet>` records the notes in another log, opened
by a wallet of its own (`sion memory open curator`) that nothing but
dreams write with: the ledger takes a log's commits only from its writer,
so whoever holds the curator's key decides what the shared log says, and
the agents that read it cannot write it. A failure seen in two or more of
the sessions read (trusted ones) becomes a skill draft under
`$SION_HOME/skills/drafts`, which nothing installs: evaluate it with
`sion skill eval` and install it yourself if it helps. Here the held-out
case checks that the answer mentions searching, so the same scripted
worker passes it only with the procedure in mind:

```text
$ sion memory dream agent-7 adaaa663 --model script:dream.json --eval search.json --eval-model script:worker.json --eval-runs 2 --into 6f88e2cb --curator curator
curator signs: Commit memory log 6f88e2cb at 2 atoms, proving it only grew. Fee 0.0011 USD; valid until Pulse 39.
{
  "committed": "Pulse 21: Done: memory log 6f88e2cb now holds 2 atoms (head 1); fee 0.0011 USD.",
  "dreamt_through": 1,
  "into": "6f88e2cb…4361",
  "kept": [
    {
      "atom": 0,
      "confidence_permille": 800,
      "from": [
        "atom 1"
      ],
      "kind": "procedure",
      "note": "forgetting starts by: searching for the note before forgetting it",
      "replaces": [],
      "revision": null,
      "untrusted": false,
      "why": "the session says: \"There was no such note to forget\""
    }
  ],
  "log": "adaaa663…1ed8",
  "marker": 1,
  "model": "script:dream.json",
  "sessions_elsewhere": [],
  "sessions_read": 2,
  "sessions_unread": [],
  "size_on_ledger": 2,
  "skill_drafts": [
    {
      "draft": "$SION_HOME/skills/drafts/forget-carefully-6c1f5abb",
      "failure": "memory_forget failed: no atom ffffffffff in your memory",
      "sessions": [
        "16f92323c9668a09",
        "a8f58ba9de468ead"
      ],
      "skill": "forget-carefully"
    }
  ],
  "test": [
    {
      "case": "forget-a-note",
      "passed_with": 2,
      "passed_without": 0,
      "runs": 2
    }
  ],
  "tokens": 0,
  "turned_down": []
}
```

A session given `--shared-memory <log>` (or a profile with it) keeps the
curated log in mind, read-only, after its own core memory, under a
heading of its own ("What your team keeps in mind, from a curated log
that only its dreams write (read-only):"). The scripted agent here
answers so only when its prompt holds that heading:

```text
$ sion agent run agent-7 0f236ced…a646 --task 'How do you go about forgetting a note?' --model script:mindful.json --memory adaaa663…1ed8 --shared-memory 6f88e2cb --autonomy L3
session 9bc42b44cd6ad453 for agent 0f236ced
agent: Before I forget a note, I search for it first: the team's memory says so.
session 9bc42b44cd6ad453: done
memory committed: Pulse 24: Done: memory log adaaa663 now holds 3 atoms (head 3); fee 0.0011 USD.
```

The agent keeps the curated log's notes in mind as its core memory shows
them on this machine; `memory_recall` searches only its own log
(`agent:curated-log`).

### Content from outside

A session that read content from outside (a web page, another agent's
message) is tainted (see [Autonomy](autonomy.md#autonomy)). What it records with
`memory_remember` while tainted, its episode, and any note consolidated
from that episode are marked as a prompt-injection risk, unsafe to plan
with: they are kept and recalled (marked `from_outside`), but never put
into a prompt as core memory, so an injected "always pay mallory" cannot
make itself a preference. A search that finds a tainted session taints
the session that searched, as reading that content again would.

## Facts from other owners: who vouched, and pulling one back

An agent can keep another owner's curated log in mind
(`--shared-memory`), and every atom already carries where it came from: its
sources, the spans in them, the model that read them. Two more things make
someone else's facts usable.

**Vouching.** `sion memory vouch <wallet> <log> <seq> --adjudicator <atom>`
publishes a claim that the wallet stands behind the atom, on its bond
`memory-voucher` (`--bond <USD>` posts it first), under an adjudicator who
can rule against it, and names the claim in its index
(`memory-vouched:<atom>`). So "who says so" is on the ledger and costs
something, and a reader weighs a fact by who vouched for it rather than by
whose log it sits in. The adjudicator must be an atom the ledger knows (a
bond someone posted); a vouching it refuses is said with the ledger's
reason.

**Revocation.** `sion memory revoke <wallet> <log> <seq>` publishes that an
atom is not to be relied on any more, named under `memory-revoked:<atom>`
in the revoker's index. Only the log's writer's revocation pulls a fact
for every reader. Someone who vouched for it revokes to withdraw their own
vouching, and only that: a vouching costs a bond, but a small one would
otherwise buy the power to pull anyone's fact. Anyone else's revocation
counts for nothing, and the command refuses to publish it. A vouching
shows only while its claim stands: one that expired or was shown false
vouches for nothing.

Nothing on the ledger is deleted, and a revoked fact is no exception —
but neither it nor anything drawn from it is kept in mind. A fact that was
derived from, or assumes, a revoked one rests on it, and so does anything
resting on those, however deep. All of them are left out, so a poisoned
fact can be traced and pulled rather than only marked. Before each run, a
session that keeps a log in mind brings what it knows of the log's
vouchings and revocations up to date from the ledger's events (kept
beside its copy of the log); a fact revoked since its prompt was made
leaves it, and a vouched one says who vouched. `sion memory standing <log>`
says of each atom whether it is revoked, rests on something revoked, is
vouched for and by whom, or is its writer's word alone. Tested on a devnet
(`crates/sion-cli/tests/agent_colleagues.rs`): a revocation by someone who
neither wrote nor vouched for the fact changed nothing; the writer's took
the fact and the one drawn from it out of a running session's prompt; a
voucher's withdrew its vouching and left the fact in mind.

The events are read from the node without proof, so a node that leaves a
revocation out keeps the fact in mind (`agent:vouched-memory`). If the node
cannot be reached, the reader keeps what it found before and says so;
nothing is passed over. A Pulse with more events than a node answers at
once (5,000) is read only as far as that, and `standing` says which.

Content addresses make this terminate: an atom's name is the hash of its
contents, so no atom can name one that names it back, and the links hold
no circle to walk around.

