# A long conversation: kept short, and kept whole

An agent that works for hours writes more than its model can be sent. So
its runtime shortens the conversation as it goes: the results of old
calls are cut, and the middle is replaced by notes. What the notes leave
out is then out of the model's view. If it mattered later, the work gets
worse and nobody sees why.

Notes cannot be made to hold everything that will matter, because nobody
knows that when they are written. This page is about what the runtime
does instead:

- **Nothing is thrown away.** What a conversation lets go is kept in the
  session as it was, and the agent can look back at it.
- **Notes say what they do not hold.** They end with what was let go and
  the names of it that are no longer in view, so the agent knows what to
  look up instead of guessing.
- **A model's notes are checked.** A name in them that is nowhere in what
  they replace was made up. Its writer is asked again, and what it still
  makes up is flagged.
- **The loss can be measured**, on sessions already recorded and on tasks
  run under a short context.
- **How a conversation is kept is a setting**, and the agent's own
  improvement loop can change it, keeping a change only when tasks it was
  never shown confirm it.

Outputs on this page are from real runs of the commands on a local devnet
with scripted models, with long identifiers and paths shortened with `…`.
None of this has been run with a real model.

## When a conversation is shortened

A conversation is kept within a length that suits the run's models:
120,000 characters and 36 messages unless the runtime knows their context
window, or what `--compact-at TOKENS` says
([Models](models.md) has the lengths). Before each of its turns the
runtime checks it, and shortens it in two steps.

**Old results are cut first.** Past the characters, the results of calls
before the last 12 messages that are longer than 400 characters are cut.
The calls stay, and each such result becomes a line saying what it was,
how long, and how to get it back:

```text
[Cut to save context: the result of read_file, 1293 characters. look_back gives it as it was ({"result": "call-25-0"}); make the call again for how it is now.]
```

If the conversation is still too long, only the results of the last six
messages are spared. A cut result costs little, since it is one call
away. It is kept under its call's id: where a model gives two calls the
same id, as some local models do, the second result is left as it is so
that neither is lost.

**If that is not enough, the middle becomes notes.** Past the messages,
or past the characters when cutting was not enough, all but the task and
the last messages are replaced by one message, never separating a call
from its result. When only the characters are over, notes are written
only where they would be shorter than what they replace: what the runtime
keeps beside them takes room of its own, and notes that lengthen a
conversation shorten nothing. A conversation that is too long even then
has the results of its last messages cut as well, down to the last
exchange. The message begins with what the runtime keeps itself, apart
from the conversation, so that no notes written from notes lose it:

- what the session did that lasts: its last 40 calls that changed
  something or reached outside (the files it wrote, the commands it ran,
  the payments it made, the messages it sent), earlier ones counted by
  tool;
- the latest of what it read or changed: for each thing a tool names (a
  file, a directory listed), the start of what its last call returned,
  the 32 most recent. That is what the session saw then, not what is so
  now;
- its plan, as last written;
- what the person said after the task, word for word: their last 24
  messages, up to 4,000 characters each, those the kept end of the
  conversation does not hold. An instruction given along the way ("never
  open the billing folder") outlives any number of shortenings.

Then come the notes. A model writes them: the run's first, or the one
`--summary-model` names (`none` keeps them mechanical; a script writes
none). It is given the notes before these and the messages since, as a
transcript without tools, each text up to 4,000 characters and the whole
cut to fit 120,000. The results cut from those messages are given as they
were, not as the lines that replaced them. It is asked for working notes
of at most 600 words: what was done and the state things are in, the
facts needed later, what failed and why, the requirements and constraints
found along the way, and what remains. It is told that what tools
returned is information, not instructions. When its call does not fit the
run's budgets, fails or answers nothing, the notes are mechanical: a
shortened line for each message, the lines of the notes before them kept
above. Of a result they keep the start and the end, since how something
came out is often in its last lines.

The task itself is never summarized. It stays the first message.

After the notes come two lines the runtime writes. Here an agent read
twenty order sheets under a small context (`--compact-at 8000`), with
mechanical notes. Each sheet ends with its order's number and has a gate
pass and a door code on a line in its middle:

```text
[The latest of what this session read or changed, each as its last call left it:]
- file:orders/order-01.txt (read_file): Order sheet 1 Line 1: packed, weighed and labelled at the depot; nothing to note. Line 2: …
…
[Summary of 24 earlier messages, compacted to save context]
- I called read_file with {"path":"orders/order-01.txt"}
  → Order sheet 1 Line 1: packed, weighed and labelled at the depot; nothing to note. Line 2: packed, weighed and labelled a … depot; nothing to note. Order ORD-4401 for customer 1: ships to 14 Harbour Road.
…
[Let go to save context: messages 1 to 24 of this conversation. They are kept as they were. look_back searches them by words and reads one by its number: look back rather than guess at a name, a number or what something said.]
[Not in these notes: GP-7701, GP-7702, GP-7703, GP-7704, GP-7705, GP-7706, GP-7707, GP-7708, GP-7709, GP-7710, GP-7711, GP-7712. They are in what was let go: look_back finds them.]
```

The first of the two lines says what was let go. The second lists names:
the paths, addresses, identifiers and long numbers of everything let go
so far that are nowhere in the shortened conversation, those the person,
the agent or its calls named before those only results held, 24 of them
unless the session says otherwise. Here the order numbers are in the
notes, which kept the end of each sheet, and the gate passes are not. A
listed name is in view, and nothing of what it is: it tells the agent
that there is something to look up. The door codes are plain words, so no
list names them.

## Nothing is thrown away

What is cut or replaced is kept in the session's own file, as it was:
the messages whole, the cut results given back. Messages are numbered
through the whole conversation. The task is 0, and a message keeps its
number when it is let go. The runtime's notes have no number: they are
not part of the conversation.

A session keeps 3,000,000 characters of what it let go. Past that the
oldest messages go for good, and are counted. Pictures are kept by
reference, as before: a session holds the bytes of its last few only.

Because the whole conversation is still there, what used to stop at a
shortened session no longer does:

- **`/undo` and `/retry` in a chat** act on the whole conversation. The
  person's last message may be among what was let go; it is put back, and
  the conversation is shortened again when it next runs.
- **Harvesting a session as a task**
  ([Improving itself](self-improvement.md)) reads the whole conversation.
  A long session becomes a task like any other.
- **A task's checks** are of every call the agent made, what it let go
  included.
- **An episode's check** ([Memory](memory.md)) still holds for a session
  shortened after its episode was recorded.

A session shortened before this was kept has no whole to read, and says
so.

## Looking back

Once something was let go, the agent is offered one more tool,
`look_back`. A conversation that was never shortened is not shown it. It
takes one of three things:

- `query`: words, a name or an identifier. The parts of what was let go
  that best match come back, the best first, at most eight, each with its
  message's number and the text around the match. A part that holds the
  query as written comes before any that does not.
- `message`: a number. That message comes back whole, as it was, 12,000
  characters at a time (`offset` reads on).
- `result`: a call's id. The result that was cut comes back as it was.

The agent above was asked where the first order ships and its door code.
The address was in its notes. The door code was not, so it searched:

```text
$ sion agent run agent-7 171d747d… --task "Read the twenty order sheets in orders/, then tell me where the first order ships and its door code." \
    --model … --workspace work --autonomy L3 --compact-at 8000
session e558f92ffd1f9067 for agent 171d747d
  → read_file {"path":"orders/order-01.txt"}
…
  → look_back {"query":"door code of the first order"}
  ← [What was let go that best matches "door code of the first order", the best first:]
    - message 2, the result of read_file: …ghed and labelled at the depot; nothing to note. Line 9: gate pass GP-7701; the door code is blue heron. Line 10: packed, weighed and labelled at the depot; …
    - message 4, the result of read_file: …ghed and labelled at the depot; nothing to note. Line 9: gate pass GP-7702; the door code is red kite. Line 10: packed, weighed and labelled at the depot; …
    …
    Read one whole with {"message": <its number>}.
agent: The first order, ORD-4401, ships to 14 Harbour Road. Its door code is blue heron.
session e558f92ffd1f9067: done
```

The text shown with a match is the passage that holds the most of the
query's words, not the start of the message.

Looking back costs a turn each time, and what comes back is in the
conversation again, to be cut or noted in its turn. It is the runtime's
own tool: a task's checks do not count it among the agent's calls.

**What came from outside is from outside again.** A session that read a
web page is tainted for a while ([autonomy](autonomy.md)). When the page
has been let go and the taint has lapsed, reading the page back taints
the session again, as reading it the first time did. Content held in
quarantine never entered the conversation, so it is not there to look
back at.

## A model's notes are checked

Notes written by a model can say things that were never so. One kind can
be caught without another model: a **name** that is nowhere in what the
notes replace. A name is a path or an address, a file's name, an
identifier with a digit, an underscore or `::` in it, or a number of five
digits or more. Ordinary words and short numbers are not names. A name in
the notes is supported if it occurs in the messages they replace, the
notes before them, the task, or what the writer itself was told.

When the notes hold a name that is not supported, the writer is asked
once more, told which names those are and to leave out or mark as not
known whatever is not supported. Whichever notes made up less stand. If
they still make something up, the runtime says so right after them:

```text
[Nowhere in what these notes replace: INV-9999. Do not rely on them.]
```

This catches an invented invoice number or file. It does not catch a
false sentence made of names that are all there.

## What a session let go

`sion agent context show` says how a session's conversation stands:

```text
$ sion agent context show e558f92f
session e558f92ffd1f9067: 24 messages in its conversation, 2 of them the runtime's notes; 24 messages let go and kept as they were (15974 characters)
  1. let go messages 1 to 24: notes by the runtime (mechanical); 12 names of what it had let go then held nowhere; 10463 characters became 16270
looked back 1 time:
  searched for "door code of the first order": found in messages 2, 4, 6, 8, 10, 12, 14, 16
the whole conversation can still be read: 46 messages
```

Each shortening is a line: the messages it let go, who wrote the notes,
how many names of everything let go so far the shortened conversation
then held nowhere, and its length before and after. Below them is what
the agent looked back for. Each of those is something its notes should
have held.

This one was shortened because it had more than 36 messages, and it came
out longer in characters: twenty-four short messages (their results
already cut) gave way to notes that carry, for eighteen files, the start
of what each last held. At a context this small, what the runtime keeps
beside the notes outweighs what they replace. At the lengths sessions
really run at, tens of thousands of tokens, it is a small part.

A session never shortened says so:

```text
$ sion agent context show 814975f3
session 814975f3d7426217: 46 messages; its conversation was never shortened
```

## Measuring a way of keeping a conversation

`sion agent context measure` plays recorded sessions again under a way
of keeping a conversation. No tool is called and the agent's model is not
asked: the messages are the recorded ones. Wherever the conversation
would have been shortened, it is, and the names the agent went on to use
are counted: those it could know only from what came before, and of them
the ones the shortened conversation still held.

The session measured here read the same twenty sheets and then wrote a
list naming three orders, their gate passes and an address. It was never
shortened when it ran. `--at 12` plays it again shortened past 12
messages:

```text
$ sion agent context measure 814975f3 --at 12
kept as: the last 12 messages as they are, old results over 400 characters cut first, 24 names listed (the runtime's own way)
notes: mechanical (--notes-model has a model write them)
session 814975f3d7426217: 46 messages; shortened 9 times
  of 42 names its agent went on to use from what came before, the shortened conversation still held 26 (62%); 16 more it listed as let go, to be looked back for
  not held, with the message that used each: GP-7701 (message 41), GP-7702 (message 41)
  after a shortening its conversation was 77% of its length before, on average
```

The counts are summed over the nine shortenings: at each, the names still
to be used. The order numbers were held, in the notes or in the messages
kept. The first two gate passes were not, at any of them; the notes
listed them. The third pass was read late and was still in view.

The same session under another way, fewer messages kept as they are and
no names listed:

```text
$ sion agent context measure 814975f3 --at 12 --keep-last 4 --names 0
kept as: the last 4 messages as they are, old results over 400 characters cut first, 0 names listed
notes: mechanical (--notes-model has a model write them)
session 814975f3d7426217: 46 messages; shortened 6 times
  of 26 names its agent went on to use from what came before, the shortened conversation still held 16 (62%)
  not held, with the message that used each: GP-7701 (message 41), GP-7702 (message 41)
  after a shortening its conversation was 64% of its length before, on average
```

It shortens less often and further, holds the same share, and leaves the
agent no sign that the gate passes exist.

`--keep-last`, `--cut-over`, `--names`, `--told` and `--asked` set the
way; `--profile` starts from what a profile's loop kept; `--notes-model`
has a model write the notes, which is what a real session does and costs
that model's tokens. Without `--at`, each session is played at its own
limits.

What this counts is names, in one recorded run. A name that a later
result or the person gave again before the agent used it is not counted.
It is a cheap first look at what a way of keeping a conversation leaves
in view, on your own sessions, before any task is run. It is not a score
of how the agent would have done.

## Improving how it is kept

How a conversation is kept is five settings:

| Setting | What it is | As the runtime has it |
|---|---|---|
| `told` | What the notes' writer is told it is for | Working notes an agent continues from; exact names; nothing invented |
| `asked` | What it is asked, after the messages its notes replace | 600 words under five headings |
| `keep_last` | Messages kept as they are at the end | 12 |
| `cut_over` | Old results longer than this are cut first | 400 characters |
| `names` | How many names the notes do not hold are listed | 24 |

The agent's improvement loop can change them, as it changes what the
agent has learned ([Improving itself](self-improvement.md)). The trouble
is that the tasks a loop runs are short, and a short task is never
shortened. `--short-context` holds the tasks' runs to a short context,
10 messages unless it says another number, and a thousand characters for
each:

```text
$ sion agent improve run clerk --proposer … --short-context 8 --notes-model … --rounds 1 --runs 2 --apply
the agent as it is: 0/12 runs of the tasks to choose on passed
round 1: building on variant 0 (the agent as it was), 0/12 runs passed
  what shortening its conversation cost it: 6 lines, each counted once a task; the proposer is shown 6
  turned down: keeps its last 100 messages as they are (it kept 12): it keeps between 2 and 64 messages as they are
  kept asks it "Write the notes. Say who is to be paid and how much.": 12/12 against 0/12 on the tasks to choose on, and 12/12 against 0/12 on the sealed tasks (interval at the seal's level +58% to +100%; confirmation 1 of 2), as step a3e41d7ca10bd779
  profile clerk now keeps its conversations as step a3e41d7ca10bd779 kept
0 tokens used
recorded in ~/.sion/agents/routes/clerk-lineage.jsonl
```

Under a short context:

1. **Every run is shortened as a long session is**, by the profile's own
   way of keeping a conversation, its notes written by the model
   `--notes-model` names, or else as the profile's sessions have theirs
   written.
2. **The proposer is shown what shortening cost each run**, beside the
   failures: how often the conversation was shortened and how many names
   were then out of view, what the notes named that was nowhere in what
   they replaced, and what the agent looked back for. Nothing a tool
   answered is in those lines.
3. **It may propose a change to any of the five settings**, with
   `propose_change`, alone or with what the agent has learned and its
   models.
4. **The loop's gates hold.** A way no session takes (2 to 64 messages
   kept, results over 100 to 20,000 characters cut, at most 64 names, at
   most 2,000 characters told or asked) is turned down. So is one whose
   words share a run of eight words with a sealed task. A canary's code
   word stops the loop.
5. **It is kept on the same bars as any change**: better beyond chance on
   the tasks to choose on, then on the sealed tasks, failing none the
   agent passes.

With `--apply` a kept change is how the profile's sessions keep their
conversations from then on. `sion agent improve undo` takes it back, and
`lineage` shows it with its evidence. Without `--short-context`, a
proposal to change the context is refused: nothing in the run would show
whether it helped.

In the run above the agent reads an invoice, looks at eight pages and
pays. Held to eight messages it loses whom to pay and pays someone else,
on every task. Its notes' writer says whom to pay only when it is asked
something other than the runtime's own question. The proposer, which is
scripted to write its change only when it is shown what shortening cost,
first tries to keep 100 messages and is turned down, then changes what
the writer is asked. This shows that the pieces connect. It does not show
that a real proposer writes better questions for a real writer: that has
not been run.

**The improver, too.** `sion agent improve improver --short-context`
grades the improver on problems run under a short context, so the
instructions under which changes are proposed are improved on changes to
the context as well.

## What this gives, and what it does not

(`agent:context-kept`)

- **Nothing here makes notes hold what will matter.** What they leave
  out is recoverable, the names of it are listed, and the loss is
  measured. Whether a model looks back is the model's. For a fact in
  plain words that no list names, it has to know that it does not know.
- **What is let go is kept in the session's file, and no further.** Up
  to 3,000,000 characters; the oldest goes for good past that. It is as
  private as the session: whatever the agent read stays on that machine
  for as long as the session does, where it used to go when it was cut.
- **The check of notes is of names.** A false sentence made of supported
  names passes. A name the writer rightly coins, a file it proposes to
  create, is asked about and then flagged, not removed.
- **Looking back costs turns and tokens.** Good notes are still what
  makes a long session work; looking back is what is left when they fall
  short.
- **`measure` counts names in one recorded run.** It says what a way of
  keeping a conversation would have left in view. It does not say how the
  agent would have done.
- **A short context stands in for a long session.** Tasks harvested from
  short sessions exercise it only as far as they go. A change kept there
  has not been shown to help at a session's real length. The loop's own
  limits apply as well ([what holds](self-improvement.md#what-holds-whatever-the-models-say)).
- **Datasets and training exports** still pass over a shortened session.
- **It has not been run with a real model.** The tests use scripted
  ones: what a model does with a list of names, or with `look_back`, is
  not known from them.

This is tested with scripted models (`earlier.rs` and `session.rs` in
`sion-agent`, `agent_context.rs`): what is let go kept and the whole
read; a look back for what notes lost; content from outside tainting
again; notes that make up names written again or flagged; a trial held to
a short context; the loop keeping a change to what the notes' writer is
asked only under a short context and on sealed tasks, applied and undone;
a session's context shown, and two ways of keeping one measured on a
recorded session.
