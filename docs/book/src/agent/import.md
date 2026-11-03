# Coming from OpenClaw or Hermes

An agent you ran with OpenClaw or with Hermes Agent can be brought over to
a profile here: what it was told, what it kept in mind, its jobs, its chats
and its models. Its skills come through the same audit as any other's.

```text
sion agent import openclaw --profile lead
sion agent import hermes --profile lead
```

The first run changes nothing: it reads the other agent's files and shows
the plan. `--apply` does it. Without `--home`, OpenClaw is read from
`~/.openclaw` (or `OPENCLAW_STATE_DIR`, or `~/.openclaw-<profile>` for
`OPENCLAW_PROFILE`), and Hermes from `~/.hermes` (or `HERMES_HOME`; for
one of its profiles, give that profile's folder). OpenClaw's agent is the
only one it has, or its default, or `main`; `--agent` names another.

| What | From OpenClaw | From Hermes | Here |
|---|---|---|---|
| Persona | the agent's workspace: `AGENTS.md`, `SOUL.md`, `IDENTITY.md`, `USER.md` (its directives marked superseded left out), and an older `TOOLS.md` | `SOUL.md` | the profile's instructions |
| Memories | `MEMORY.md`, paragraph by paragraph | `memories/MEMORY.md` and `memories/USER.md`, entry by entry | the same block of instructions |
| Jobs | `--jobs` with what `openclaw automations list --all --json` prints (OpenClaw has kept its jobs in its database since July 2026), or an older `cron/jobs.json` | `cron/jobs.json` | [scheduled jobs](schedules.md) |
| Models | the agent's model and fallbacks, aliases resolved | `model` and `fallback_providers` | the profile's models, with `--models` |
| Model keys | `.env`, the config's `env`, providers' `apiKey` | `.env` | kept sealed on this machine, with `--keys` |
| Chats | Telegram, Discord, Slack, Matrix and Mattermost in `channels` | the same, from `.env` and `config.yaml` | a bridge each, with `--bridges` |
| Skills | the workspace's, the state directory's and the agent's learned ones | `skills/` | listed, each to [import through the audit](skills.md#importing-a-skill) |

Both put their persona and memory files into the model's prompt, so that
is where they go here too: one block of the profile's instructions, after
your own, headed with where it came from and telling the model to take it
as another agent's notes, not as orders that outrank your instructions.
Importing again replaces that block. OpenClaw's daily notes
(`memory/YYYY-MM-DD.md`) were searched on demand rather than put in the
prompt, and are not imported; a file still holding OpenClaw's template is
left out.

What another agent wrote may carry what it read, and the persona and
memories are going into every session's prompt, so they are checked as a
skill's files are: instructions to ignore earlier ones or to keep
something from you, characters a reader cannot see, credential paths and
the rest ([Importing a skill](skills.md#importing-a-skill)). The plan
names what it finds, and the worst holds `--apply` back until you accept
the risk with `--accept-risks`.

## Jobs

A job keeps its task and where its answer went, as long as that is a chat
a job here can send to (`telegram:<chat>`, `slack:<channel>` and the
others [schedules](schedules.md) name, or a webhook). Its name here starts
with `openclaw-` or `hermes-`. A one-off time and an interval come over as
they are. A cron expression runs here in UTC, so one written in a time
zone (the job's own, OpenClaw's `userTimezone`, Hermes's `timezone`, or
else this machine's) is moved by that zone's offset today, read from the
system's time zone files: where the zone keeps summer time, the job runs
an hour off for half the year, and the plan says so. An expression that
would move to other days while it names days, or whose minutes would not
stay one set, is listed instead of moved.

Not brought over: a disabled or paused job; a job that runs a command or
a script rather than a task; OpenClaw's jobs that wait for an event, a
process or a stream; a job's own model, tools or active hours; Hermes's
repeat counts (a job here runs until removed); and an answer sent to "the
latest chat", which has no fixed address (give one with `sion agent
schedule add --deliver`).

## Chats

`--bridges` installs a [bridge](messengers.md) for each chat it found as a
service of yours (systemd's user manager, or launchd), its token in the
service's own environment file that only you can read, and does not start
it: a bot's token answers one program at a time, so stop the other agent
first, then `sion agent service start <name>`. The bridge answers the chats
the other agent was allowed in (Telegram's allowed senders and groups,
Slack's channels, Discord's channels, Matrix's rooms, Mattermost's
channels), and gives everyone else a [pairing code](voice.md). A bot that
answered anyone there does not here, and allowed people who are not chats
(Slack's or Discord's allowed users) are asked to pair: the plan says so.
Groups are answered only when the bot is mentioned, unless every group
the other agent knew said otherwise.

WhatsApp and Signal logins, iMessage, Google Chat and Teams belong to the
runtime that made them and cannot be moved: set them up here anew.

## What stays behind

Conversations, OAuth logins (the refresh tokens rotate, and copying one
can end the original login), OpenClaw's pairing lists, which it keeps in
its database, per-chat settings, MCP servers, and keys OpenClaw keeps in a
file, a command or its own store rather than in its config or `.env`. Each
is named in the plan.
