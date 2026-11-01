# Run it on your computer

One person, one machine, an agent of their own: `sion setup` makes what it
needs, and `sion` talks with it. Everything runs here: the chain the agent
answers on, its wallet and budget, its memory, its workspace. Only the
model is elsewhere, unless you run that here too.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/ColdAI-org/siontology/main/scripts/install.sh | sh
```

The installer puts `sion` in `~/.local/bin`. It takes the latest release
for your machine (Linux or macOS, x86_64 or aarch64), checks the archive
against the release's `SHA256SUMS` and those against their signature by
the release key, and installs nothing if either check fails. With no
release yet, it builds `sion` from the repository instead (Rust and a C
compiler; [Install from source](install.md)).

While the repository is private, those addresses answer only someone
signed in. With the GitHub CLI signed in (`gh auth login`), fetch the
installer with it; it then reads the release with `gh` too:

```bash
gh api repos/ColdAI-org/siontology/contents/scripts/install.sh -H 'Accept: application/vnd.github.raw' | sh
```

## Set up

```bash
sion setup
```

At a terminal, setup asks for what it cannot assume: your wallet's name,
the model and its key, the directory the agent may work in, and whether it
should answer on Telegram too. `sion setup --yes` takes the defaults and
the options given (`sion setup --help`). Each step is skipped when it is
done already, so setup can be run again, to change the model, say.

What it makes:

1. **A chain of your own**: one validator, its API at `127.0.0.1:8730`, its
   state in `~/.sion/chain`. Your service manager runs it (a systemd user
   service on Linux, a launchd agent on macOS: service `chain`), so that it
   starts at each login. Where there is none (a container, say), setup
   prints the command to run in a terminal of its own and goes on when the
   chain answers. A chain that already answers there is used as it is.
2. **Two wallets**: yours, and your agent's (`<name>-agent`), with test
   dollars from the chain's faucet: yours funds the agent's budget, the
   agent's pays its transactions' fees.
3. **The agent**, registered by you: a budget of 1,000 USD, at most 100 USD
   a payment and 500 USD at once, a lifetime of a billion Pulses.
4. **Its memory**: a log only it can append to.
5. **A model**: Claude, OpenAI or Gemini with an API key; your ChatGPT plan,
   signed in with your browser; or a model server on this machine or your
   network (below). A key is kept sealed on this machine (below), and setup
   checks that the model answers before going on.
6. **A profile**, `assistant`: the agent, its model and memory, a workspace
   it may read and write (`~/sion-workspace`), the web, and autonomy L5: it
   asks before it spends, and before it reaches outside once it has read
   something from outside.
7. **Scheduled tasks**: a service of yours running the scheduler (service
   `scheduler`), so that what you schedule (`sion agent schedule add`)
   runs; where there is no service manager, setup says to run
   `sion agent scheduler` in a terminal of its own.
8. **Telegram**, if you say so: a service of yours running the bot, which
   gives whoever writes to it first a pairing code for you to approve.

Here is setup on a machine with no service manager, with a scripted model
standing in for a real one (this environment reaches no model provider).
The chain's command was run in another terminal when setup printed it.
Recorded on the 2-core machine used for this book (long identifiers
shortened with `…`):

```text
$ sion setup --yes --model script:assistant.json
Setting up Siontology for you in ~/.sion (SION_HOME).
The chain: one validator of your own. Nothing here runs services for you, so run it in a terminal of its own (setup goes on when it answers):

    sion devnet --api 127.0.0.1:8730 --validators 1 --shards 1 --seats 1 --idle-round-ms 10000 --report 0 --data-dir ~/.sion/chain

  the chain answers at http://127.0.0.1:8730
Your wallet, ana, and your agent's, ana-agent: test dollars from the chain's faucet fund its budget and its fees.
  $ sion keys new ana
    created ana: account b571db5b…e0df
    seed stored in ~/.sion/wallets/ana.seed
  $ sion faucet ana 10000
    funded ana with 10,000
  $ sion keys new ana-agent
    created ana-agent: account a6a17028…abe3
    seed stored in ~/.sion/wallets/ana-agent.seed
  $ sion faucet ana-agent 100
    funded ana-agent with 100
The agent: registered by ana, signing as ana-agent.
  $ sion agent register ana ana-agent --lifetime 1000000000
    ana signs: Register ana-agent as an agent, funded with 1,000 USD: any payee, at most 100 USD per payment, 500 USD at once, refilled by 0.5 USD a Pulse, until Pulse 1000000007, a receipt claim for every payment, may delegate narrower limits. Fee 0.0012 USD; valid until Pulse 27.
    Pulse 9: Done: agent a0894889 registered, signing as ana-agent; fee 0.0012 USD.
    agent a0894889…2b5f
    allotment 3f4ce748…15bd
    did did:sion:062600d1…36a0:a0894889…2b5f
Its memory: a log of its own.
  $ sion memory open ana-agent --agent a0894889…2b5f
    ana-agent signs: Open a memory log for agent a0894889, which only it can append to. Fee 0.0012 USD; valid until Pulse 31.
    Pulse 13: Done: memory log a0ca885a opened for agent a0894889; fee 0.0012 USD.
    log a0ca885a…6789
  checking that script:~/assistant.json answers… it said: Hello. I am ready.
The profile assistant: how it runs.
  $ sion agent profile add assistant ana-agent a0894889…2b5f --model script:~/assistant.json --autonomy L5 --memory a0ca885a…6789 --workspace ~/sion-workspace --web
    profile assistant: agent a0894889 as ana-agent, script:~/assistant.json at L5, with memory, web
Scheduled tasks: for what you schedule (sion agent schedule add) to run, run `sion agent scheduler` in a terminal of its own too.

Ready.
  Talk with your agent:           sion
  Continue the last conversation: sion -c
  Its workspace:                  ~/sion-workspace
  What this machine has:          sion doctor
  A newer release:                sion update
```

## Talk with it

`sion` starts a new conversation with the profile setup made; `sion -c`
goes on with the last one. The agent's memory log lasts across
conversations; a conversation is what it was told and did in one.

In this recording what was typed went in on standard input, so it does not
show after `you>`: "What is in your workspace?", then "Write my notes
down."

```text
$ sion
Talking with assistant (conversation 2026-11-01-153915); end with Ctrl-D, continue later with `sion -c`.
you>   · list_files
agent> Your workspace is empty so far. Put files in it, or ask me to write some.
you> 
$ sion -c
Continuing with assistant (conversation 2026-11-01-153915); end with Ctrl-D, continue later with `sion -c`.
you>   · write_file
agent> I wrote notes.md in your workspace.
you> 
$ cat ~/sion-workspace/notes.md
# Notes

- Try sion -c to continue a conversation.
```

`sion agent talk <profile> --conversation <name>` talks with another
profile, or in a conversation of your naming; `--voice` speaks instead of
typing ([Pairing and voice](../agent/voice.md)).

Setup run again changes nothing it need not:

```text
$ sion setup --yes
Setting up Siontology for you in ~/.sion (SION_HOME).
The chain: http://127.0.0.1:8730 answers; setup uses it.
Your wallet, ana, and your agent's, ana-agent: test dollars from the chain's faucet fund its budget and its fees.
The agent: a0894889…2b5f, registered already.
Its memory: log a0ca885a…6789, opened already.
The profile assistant: how it runs.
  $ sion agent profile add assistant ana-agent a0894889…2b5f --model script:~/assistant.json --autonomy L5 --memory a0ca885a…6789 --workspace ~/sion-workspace --web
    profile assistant: agent a0894889 as ana-agent, script:~/assistant.json at L5, with memory, web
Scheduled tasks: for what you schedule (sion agent schedule add) to run, run `sion agent scheduler` in a terminal of its own too.

Ready.
  Talk with your agent:           sion
  Continue the last conversation: sion -c
  Its workspace:                  ~/sion-workspace
  What this machine has:          sion doctor
  A newer release:                sion update
```

## Models

| Choice | `--model` | What it needs |
|---|---|---|
| Claude | `anthropic:claude-sonnet-5-5` (or another Claude model) | `ANTHROPIC_API_KEY` |
| OpenAI | `openai-responses:<model>` | `OPENAI_API_KEY` |
| Gemini | `gemini:<model>` | `GEMINI_API_KEY` |
| Your ChatGPT plan | `chatgpt:<model>` | `sion agent chatgpt login` (setup runs it) |
| A model server here | `local:<model>` | Its address: `--local-url` (default Ollama's, `http://127.0.0.1:11434/v1`; LM Studio `http://127.0.0.1:1234/v1`, llama.cpp `http://127.0.0.1:8080/v1`) |

Setup asks for a key without showing it, or takes it from the environment
(`--key-from-env <VARIABLE>`), and keeps it sealed in
`~/.sion/agents/credentials.bin`, encrypted under the key beside it, so
that `sion` and the services it runs find it without the environment. A
variable set in the environment wins over a kept one. `sion agent
credential list` names what is kept (never the values); `add` and `remove`
change it. Skills' scripts never receive these: theirs are `sion agent
secret`'s. Whoever can read `~/.sion/agents` can read them
(`agent:kept-credentials`), as whoever can read a process's environment
can read the variables it would otherwise be given.

For a model server, setup lists the models it serves and asks which. A
`local:` model may be on this machine or your network; a cloud provider's
address set to something else (`ANTHROPIC_BASE_URL`, `OPENAI_BASE_URL`) may
be on this machine only.

## The chain at rest

A validator with nothing to order waits before its next round
(`--idle-round-ms`, 10 seconds for the chain setup makes), unless a
transaction comes or another validator proposes; after ordering anything
but a beacon reveal it goes at full pace until the Pulse holding it
closes. Without the wait, one validator signs rounds as fast as it can.
Measured on the 2-core machine used for this book (one validator, the
prover on; CPU averaged over 30 seconds at rest, then `sion send alice bob
1` timed from the command to its receipt):

| `--idle-round-ms` | CPU at rest (of one core) | Pulses at rest | The transfer |
|---|---|---|---|
| 0 (the default) | 98.8% | 65 in 56 s | the API did not answer within 30 s |
| 2,000 | 17.7% | 6 in 31 s | 2.4 s |
| 5,000 | 9.4% | 3 in 30 s | 2.3 s |
| 10,000 | 5.7% | 1 in 30 s | 2.3 s |

Before a validator kept its pace after ordering a transaction, the same
transfer at 10-second rounds waited 22.1 s for its Pulse to close. At
rest, Pulses come slowly, and so does what is counted in them: an agent's
spending refills by the Pulse, and a transaction is valid for about 18
Pulses after it is signed.

## Telegram

`sion setup --telegram` (or yes at the question) takes the bot's token
from `@BotFather`, asked for without showing it or taken from
`TELEGRAM_BOT_TOKEN`, and installs service `telegram`, running
`sion agent telegram assistant --pairing` (where there is no service
manager, it prints that command instead): write to your bot, and approve
the code it gives you with `sion agent pairing approve <code>`. The token
is kept in the service's environment file, which only you can read
(`agent:service-tokens`). [Messengers](../agent/messengers.md) names the
other messengers a profile can answer on.

## Update

```bash
sion update --check     # is there a newer release?
sion update             # install it
```

`sion update` takes the latest release from GitHub, checks the signature
on its `SHA256SUMS` against the release key `sion` was built with
(`scripts/release/allowed_signers`, with `ssh-keygen`), then the archive
against its signed SHA-256, then that what it holds runs and is the
release's version, and only then puts it in place of the program it runs
as. A build whose file names no key refuses to update itself; so does a
`sion` that Homebrew or Nix manages, or one built in a checkout. For a
private repository it sends a token, from `GH_TOKEN` or `GITHUB_TOKEN` or
the GitHub CLI, to GitHub's API only. Services go on with the old program
until they restart; `sion update` says how. Trusting an update means
trusting whoever holds the release key, a secret the release workflow
signs with (`release:signing-key`).

## Undo it

```bash
sion agent service remove chain      # and telegram, if you made it
rm -rf ~/.sion ~/sion-workspace ~/.local/bin/sion
```

## What this is not

- The chain is a devnet: one validator whose keys anyone can derive from
  public names (`impl:devnet-keys`), and a faucet that mints test dollars
  for whoever reaches its API. That is safe only because nothing but this
  machine reaches it (its API listens on `127.0.0.1`). The money is test
  money.
- One machine: the chain, the agent and its memory are on it. Back up
  `~/.sion` to keep them; a chain whose state is lost is a new chain, on
  which setup registers the agent again.
- Not everything here was run for this book. The container it was
  written in has no service manager, so setup's service path (a systemd
  unit or a launchd agent, started) was not run, only the files it writes
  are tested; the release workflow had not run, so no release, macOS
  build or release key existed; and macOS binaries are not notarized, so
  one downloaded by a browser rather than the installer may be stopped by
  Gatekeeper.
