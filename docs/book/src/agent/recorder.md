# A flight recorder for an agent that acts

When a machine an agent drives does harm, what happened is argued from
its logs, and the logs are its operator's. `sion agent recorder` keeps a
device's events in a log the ledger holds append-only, so that an
investigator can hold the operator to what was recorded and when.

It builds on [memory logs](../concepts/memory.md): a recorder is one,
with a format for a device's events and the tools an investigation
needs.

Outputs on this page are from real runs of the commands on a local
devnet, with long identifiers and paths shortened with `…`. No robot or
other device was involved: the events are a file.

## Recording, with no network

A recorder is opened once, online. It names the wallet that writes it,
how often the device is expected to record something, and the channels
whose readings are sealed:

```text
$ sion agent recorder open arm-7 --wallet operator --heartbeat-ms 1000 --seal arm/joint-3
opened recorder arm-7: log c91d3199…, written by operator
  sensor readings of arm/joint-3 are logged as commitments; what opens them is kept in …/recorders/arm-7.openings.json
```

The device reports events as lines of JSON: what a sensor read, a
command sent, a decision taken, a fault, or a heartbeat that says only
that it is alive. Each has the device's own time:

```text
$ cat shift1.jsonl
{"t_ms": 1700000000000, "kind": "heartbeat"}
{"t_ms": 1700000000200, "kind": "command", "channel": "arm", "name": "move_to", "value": "bin-4"}
{"t_ms": 1700000000400, "kind": "sensor", "channel": "arm/joint-3", "name": "torque_mnm", "value": 4200}
…
$ sion agent recorder record arm-7 --file shift1.jsonl
recorded 7 event(s) as records 0 to 6: on this machine until the next commit
```

Recording touches no network. Without `--file` the events are read from
standard input, so a control loop can pipe into it.

A recorder drops nothing. A line that is not an event, or an event it
cannot record as it came, is recorded as a fault that says why, at its
time, and the rest of the batch is recorded with it.

## Commits fix what was recorded

```text
$ sion agent recorder commit arm-7
committed at Pulse 12: records 0 to 6 are fixed
```

A commit puts the log's head on the ledger with a proof that the log
only grew. From then on those records cannot be changed, reordered or
left out without the head changing, and the Pulse is a time by which
they existed that the operator did not choose.

A device out of reach goes on recording, and what it records waits for
the next commit. Until then it is only the device's:

```text
$ sion agent recorder record arm-7 --file shift2.jsonl
recorded 5 event(s) as records 7 to 11: on this machine until the next commit
$ sion agent recorder show arm-7 --last 3
recorder arm-7: 12 record(s) in log c91d3199, 7 fixed by a commit, 5 only on this machine
  from t=1700000000.000 to t=1700000018.000 by the device's clock; a record is expected every 1.000 s
  records 0 to 6 were fixed at Pulse 12
  records 7 to 11 are in no commit this recorder noted
  silence: nothing recorded for 14.000 s, from t=1700000003.000 (record 7) to t=1700000017.000 (record 8)
  9 t=1700000017.100 fault arm/joint-3 overcurrent = "drive tripped"
  10 t=1700000017.200 sensor arm/joint-3 torque_mnm = (sealed)
  11 t=1700000018.000 heartbeat
```

## What is missing shows

The device is expected to record something every heartbeat. A stretch of
more than twice that with no record at all is reported as a silence, as
above, and so is a device clock that steps back. Both are read from the
records' own times, by `show` on the device and by `check` on an
extract.

## After an incident

The operator seals the log, which closes it for good, and makes an
extract of the window that matters:

```text
$ sion agent recorder commit arm-7 --seal
committed at Pulse 15: records 7 to 11 are fixed, and the log is sealed: it takes no more
$ sion agent recorder extract arm-7 --from-ms 1700000000000 --until-ms 1700000060000 \
    --withhold planner --out incident.json
extract of records 0 to 11 (11 shown, 1 withheld), as of Pulse 15: incident.json
```

An extract is the run of records from the first to the last whose device
time is in the window, every record between them, each with the path
that shows it is at its place in the log. Records of a withheld channel
are given as digests: the investigator sees that a record is there, and
where, and not what it says. Heartbeats and sealed readings are always
shown.

The investigator checks the extract against the ledger:

```text
$ sion agent recorder check incident.json --records
holds: the extract is of the log as the ledger holds it: 12 record(s), committed at Pulse 15, and sealed: it takes no more
holds: records 0 to 11, all 12 of them, are in that log in this order (11 shown, 1 withheld)
  the records shown run from t=1700000000.000 to t=1700000018.000 by the device's clock
  every one of them was fixed no later than Pulse 15
  records 0 to 6 were fixed at Pulse 12 (the ledger's events confirm it)
  records 7 to 11 were fixed at Pulse 15 (the ledger's events confirm it)
  silence: nothing recorded for 14.000 s, from t=1700000003.000 (record 7) to t=1700000017.000 (record 8)
  0 t=1700000000.000 heartbeat
  1 t=1700000000.200 command arm move_to = "bin-4"
  2 t=1700000000.400 sensor arm/joint-3 torque_mnm = (sealed)
  3 t=1700000001.000 heartbeat
  4 (withheld)
  …
```

A record changed after the fact is not at its place in the log:

```text
$ sion agent recorder check incident-changed.json
holds: the extract is of the log as the ledger holds it: 12 record(s), committed at Pulse 15, and sealed: it takes no more
differs: 1 of the 12 record(s) are not at their place in that log
…
error: verification failed: the extract does not check out
```

## A bound on readings that are not shown

A sealed channel's readings are in the log as commitments. The operator
can prove something about every one of them in an extract without
showing any:

```text
$ sion agent recorder prove arm-7 incident.json --channel arm/joint-3 --reading torque_mnm --at-most 5000
proved a bound on 3 sealed reading(s) of arm/joint-3 torque_mnm, without showing them: incident.json
$ sion agent recorder check incident.json
…
holds, proven without the readings: every one of the 3 sealed reading(s) of arm/joint-3 torque_mnm in these records is at most 5000
  (1 record(s) are withheld: whether any is a reading of this channel is not shown)
```

There is no proof of a bound that a reading breaks:

```text
$ sion agent recorder prove arm-7 incident.json --channel arm/joint-3 --reading torque_mnm --at-most 4500
error: no proof of that bound: statement 2 does not hold of these numbers. A bound that a reading breaks cannot be proven, and one proof holds at most 4096 readings and bounds together
```

The proof is bound to the log and the run of records it was made for. It
is the kind of proof described in
[Proving rules were kept](proofs.md), with the same limits.

## What a recorder shows, and what it does not

(`agent:flight-recorder`)

- **It shows what was recorded, not what happened.** A sensor that is
  wrong and a device that lies are recorded faithfully. The device and
  its software are the operator's.
- **Before a commit, a record is only the device's.** Whoever holds the
  device can change or drop what is not committed yet. The Pulse each
  run of records was fixed at is how an investigator sees how long that
  was. A device that cannot reach the ledger cannot shorten it.
- **Times are the device's word.** The ledger adds a Pulse no later than
  which a record existed, and nothing about when it was made.
- **The ledger holds the log's head, not its records.** If the device's
  store is lost, so are the records: the head then shows only that they
  existed. `sion memory archive` keeps a copy with a storage provider.
- **An extract is a run of records, whole.** That no record outside the
  run belongs to the window is not shown; the times of the records at
  its edges are, and an investigator who doubts them asks for a wider
  run.
- **An extract is of the log as the ledger holds it.** After a later
  commit it has to be made again. Sealing the log after an incident
  keeps an extract checkable for good.
- **A withheld record is a digest of its contents.** Someone who can
  guess the contents exactly, down to the nanosecond the recorder wrote
  it, can confirm the guess.
- **A bound is proven of the sealed readings that are shown.** `extract`
  never withholds one, and `check` says when records are withheld.
  What opens the readings is kept on the operator's machine: without it
  they can be neither shown nor proven about.
- **Whoever holds the recorder's wallet can append to its log**, and
  cannot rewrite it.
- Which Pulse an earlier run of records was fixed at is the recorder's
  note. `check` confirms it from the node's events where the node still
  holds them (a node keeps its last Pulses only), and says so where it
  cannot. The Pulse of the head is always the ledger's.
- Nothing here is real-time or measured for rate, and it was not run on
  a device.

This is tested on a devnet (`agent_recorder.rs`): events recorded
offline and fixed by two commits, a silence and a clock that stepped
back found, an extract with a withheld channel checked, a bound proven
and a broken one refused, a changed record, a missing one, a swapped
digest, another bound and another run's proof all caught, a stale
extract refused and a sealed log's extract still checked, and events
that could not be recorded kept as faults.
