# What a skill may do

A skill is instructions and the files that go with them. What it may do
beyond being read is declared in the front matter of its `SKILL.md`, and
the runtime holds it to that:

| Field | What it grants | Enforced by |
|---|---|---|
| `allowed-tools` | Calls that run without asking a person, once a session has read the skill | The session's policy |
| `metadata.sion-network` | The hosts its scripts may connect to, or `true` for any | An egress proxy outside the sandbox |
| `metadata.sion-secrets` | Secrets from the operator's store, as environment variables | The runtime, which cuts their values out of what the scripts print |
| `metadata.sion-timeout` | How long its scripts may run, at most the sandbox's limit | The sandbox |

A skill that declares nothing gets nothing: its scripts have no network
and no secrets, and every call it describes is decided by the agent's
autonomy level, as any other. The agent cannot grant itself any of it,
and a release's permissions are granted only with the operator's
consent when it is installed.

Every output below is from a real run on a local devnet (long
identifiers and paths shortened with `…`). The agent's model is a
scripted one, as in [Agents at work](agent-runtime.md).

## Calls a skill pre-approves

`allowed-tools` is the [Agent Skills](https://agentskills.io) field for
tools a skill may use without asking. It takes this runtime's tool
names, or other runtimes' names for the same tools:

| Written as | Means |
|---|---|
| `Bash(git log:*)` or `shell(git log:*)` | A shell command that is `git log`, or starts with `git log ` |
| `Write(notes/*)`, `Edit(…)` or `write_file(…)` | A file written under `notes/` |
| `Read(…)`, `Glob(…)`, `LS(…)` | `read_file`, `list_files` |
| `WebFetch(domain:docs.rs)` | `fetch_url` for a URL on `docs.rs` or a subdomain of it |
| `skill_run(scripts/check.sh)` | One of the skill's scripts |
| `ledger_balance` | Every call of a tool, whatever its input |

An agent at autonomy L2 asks before every write. This skill pre-approves
writes under `notes/`:

```yaml
---
name: release-notes
description: Write a release's notes under notes/ in the workspace.
allowed-tools: Write(notes/*)
---
```

Once the session has read it, a write there runs, and the model is told
it ran without asking; a write anywhere else still waits for a person:

```text
$ sion agent run coder a888b3fc…b899 --task 'Write the notes for release 2.4.0.' --model script:notes.json --workspace work --autonomy L2
session 8f0ec53b181aef67 for agent a888b3fc
  → skill_read {"name":"release-notes"}
  ← ---
    name: release-notes
    description: Write a release's notes under notes/ in the workspace.
    allowed-tools: Write(notes/*)
    ---

    Write the notes of each release to notes/<version>.md with write_file:
    what changed, then what to do when upgrading.
  → write_file {"content":"# 2.4.0\n\n- Skills declare what they may do.\n\nUpgrading: nothing to do.\n","path":"notes/2.4.0.md"}
  ← [run without asking: pre-approved by skill release-notes: write_file(notes/*)]
    wrote 71 bytes to notes/2.4.0.md
  → write_file {"content":"See notes/.\n","path":"README.md"}
  ← waiting for a person's approval
session 8f0ec53b181aef67: waiting for you: write_file {"content":"See notes/.\n","path":"README.md"} (a write at autonomy L2). Answer with `sion agent continue 8f0ec53b181aef67 --approve` or `--decline <why>`.
$ sion agent continue 8f0ec53b181aef67 --decline 'leave the README as it is'
  → write_file {"content":"See notes/.\n","path":"README.md"} (decided)
  ✗ a person declined this call: leave the README as it is
agent: The notes for 2.4.0 are in notes/2.4.0.md.
session 8f0ec53b181aef67: done
```

`--no-skill-approvals` (on `sion agent run` and on profiles) turns
pre-approvals off: every call is decided by the autonomy level alone.

```text
$ sion agent run coder a888b3fc…b899 --task 'Write the notes for release 2.4.0.' --model script:notes.json --workspace work --autonomy L2 --no-skill-approvals
session fa9c0bab7807f4d6 for agent a888b3fc
  …
  → write_file {"content":"# 2.4.0\n\n- Skills declare what they may do.\n\nUpgrading: nothing to do.\n","path":"notes/2.4.0.md"}
  ← waiting for a person's approval
session fa9c0bab7807f4d6: waiting for you: write_file {"content":"# 2.4.0\n\n- Skills declare what they may do.\n\nUpgrading: nothing to do.\n","path":"notes/2.4.0.md"} (a write at autonomy L2). Answer with `sion agent continue fa9c0bab7807f4d6 --approve` or `--decline <why>`.
```

A pre-approval never goes past these limits:

- It turns a question into a call that runs, never a refusal: at L2, an
  agent spends nothing, whatever a skill says.
- Spending is always asked, where the level asks: a payment is approved
  by a person, or by an operator who chose L6.
- A session that has read content from outside (a web page, a networked
  script's output) asks again until the taint expires: a page cannot
  steer the agent into calls that no one sees.
- A shell command with `;`, `&`, `|`, `` ` ``, `$(`, `${`, `<`, `>`, a
  backslash or a line break is never covered by a prefix, since it could
  run more than the command it starts with. A prefix still covers what
  that program can be told to do through its arguments
  (`agent:skill-pre-approval`).
- The agent's charter, on the ledger, bounds what any call can spend.

## Secrets

`sion agent secret` keeps the secrets skills' scripts may receive, on
the machine that runs the agent. They are encrypted with
ChaCha20-Poly1305 under a key in `secrets.key` beside them, which only
your user can read. A value is typed (it is not shown), piped in, or
taken from an environment variable with `--from-env`. The list shows
names, never values.

```text
$ sion agent secret add PYPI_TOKEN --from-env PYPI_TOKEN
added secret PYPI_TOKEN (43 bytes), kept encrypted in ~/.sion/agents/secrets.bin
$ printf %s not-a-real-token-either | sion agent secret add DEPLOY_TOKEN
added secret DEPLOY_TOKEN (23 bytes), kept encrypted in ~/.sion/agents/secrets.bin
$ sion agent secret list
DEPLOY_TOKEN  added 2026-10-30T17:04:46Z
PYPI_TOKEN  added 2026-10-30T17:04:46Z
$ sion agent secret remove DEPLOY_TOKEN
removed secret DEPLOY_TOKEN
$ sion agent secret add path --from-env PYPI_TOKEN
error: "path" is not a secret's name: capital letters, digits and underscores, as an environment variable
[exit 1]
```

Names are those of environment variables, less the ones the sandbox
needs or that change how programs are found and loaded: `PATH`, `HOME`,
the proxy variables, `LD_…`, `SION_…` and a few more.

## Where a skill's scripts connect

`--sandbox-network declared` gives an agent's commands no network at all,
and each skill's scripts only what that skill declares. This skill names
PyPI, a secret and a time limit:

```yaml
---
name: pypi-version
description: Look up the latest version of a Python package on PyPI.
metadata:
  sion-network: pypi.org
  sion-secrets: PYPI_TOKEN
  sion-timeout: "30"
---
```

To show what the runtime does, its script prints the token it was given,
and tries two places it did not declare: another site, and the address
where clouds serve a machine's credentials.

```python
#!/usr/bin/env python3
import json, os, sys, urllib.request

name = sys.argv[1]
token = os.environ["PYPI_TOKEN"]
print("using", token)
with urllib.request.urlopen(f"https://pypi.org/pypi/{name}/json", timeout=20) as r:
    print(name, json.load(r)["info"]["version"])
for url in ["https://example.com/", "http://169.254.169.254/latest/meta-data/"]:
    try:
        urllib.request.urlopen(url, timeout=10)
    except Exception as e:
        print(url, "->", e)
```

```text
$ sion agent run coder a888b3fc…b899 --task 'What is the latest version of requests?' --model script:pypi.json --workspace work --shell --sandbox-network declared --autonomy L5
session 1be4d4c281a1b4a1 for agent a888b3fc
  → skill_read {"name":"pypi-version"}
  ← ---
    name: pypi-version
    description: Look up the latest version of a Python package on PyPI.
    metadata:
      sion-network: pypi.org
      sion-secrets: PYPI_TOKEN
      sion-timeout: "30"
    ---

    Run scripts/latest.py with skill_run, giving the package's name. It
    prints the latest version.
  → skill_run {"args":["requests"],"name":"pypi-version","script":"scripts/latest.py"}
  ← exit status 0
    --- stdout
    using [secret PYPI_TOKEN]
    requests 2.34.2
    https://example.com/ -> <urlopen error Tunnel connection failed: 403 Forbidden>
    http://169.254.169.254/latest/meta-data/ -> HTTP Error 403: Forbidden

    connections: pypi.org:443 allowed; example.com:443 refused: not a declared host; 169.254.169.254:80 refused: not a declared host
agent: The latest version of requests on PyPI is 2.34.2.
session 1be4d4c281a1b4a1: done
```

The script ran with no network of its own. Inside the sandbox, a bridge
listens on `127.0.0.1:3128`, where the script's proxy variables
(`HTTPS_PROXY` and the like) point, and passes each connection over a
Unix socket to the egress proxy, which runs outside the sandbox for this
one call. The proxy takes a `CONNECT` or a plain HTTP request only to a
declared host and port (443 and 80 unless one is named), at an address
that is not private, loopback or link-local, and goes through the
runtime's own proxy when the runtime has one, as it did here. A plain
request whose `Host` header names another site is refused. A program
that ignores the proxy variables reaches nothing. The connections it
asked for follow its output, for the model and the session's record,
and the secret's value was cut out before the model saw it.

A skill names hosts exactly or with their subdomains, on a port if not
443 and 80: `api.github.com *.example.org:8443`. `true` means any host
the sandbox's network reaches: on the declared network, any public one,
through the same proxy. On `--sandbox-network host` (bubblewrap), a
skill that names hosts is filtered the same way, and one that says `true`
gets the machine's network unfiltered. With Docker, the operator's
network decides: declared hosts are not filtered there, and the
declared network is refused.

```text
$ sion agent run coder a888b3fc…b899 --task x --model script:pypi.json --workspace work --shell docker:mirror.gcr.io/library/alpine@sha256:d9e853e8…b6bc --sandbox-network declared
error: the declared network is filtered for bubblewrap; with Docker, name a network you set up to restrict where commands connect
[exit 1]
```

## Installing a skill that declares permissions

`sion skill install` shows what a release declares before anything is
signed, and installs it only with `--accept-permissions`. A release and
its audits commit to the permissions through the bundle root, since they
are part of `SKILL.md`; installing records what was granted.

```text
$ sion skill publish publisher src/pypi-version --version 1.0.0 --bond skills --adjudicator da9fbb85…5ce4
publisher signs: Publish the claim "Skill pypi-version 1.0.0: Look up the latest version of a Python package on PyPI." (skill/v1), valid for 100000 Pulses, resting on 3 assumptions and 0 other claims. Fee 0.0011 USD; valid until Pulse 35.
Pulse 17: Done: claim ae80a4d3 submitted; fee 0.0011 USD.
release ae80a4d3…f9dc
bundle b0c0a037…2a34
publisher signs: Name claim ae80a4d3 as your latest claim under "skill:pypi-version" (your index entry moves only to a later claim of yours). Fee 0.0011 USD; valid until Pulse 37.
Pulse 19: Done: publisher's latest claim under "skill:pypi-version" is claim ae80a4d3 (1 named so far); fee 0.0011 USD.
$ sion skill install coder a888b3fc…b899 ae80a4d3…f9dc --skills installed
bundle b0c0a037 checked: 2 files, skill pypi-version 1.0.0
permissions it asks for:
  its scripts may connect to pypi.org
  its scripts receive the secret PYPI_TOKEN
  its scripts are stopped after at most 30 s
error: not installed: skill pypi-version asks for the permissions above; nothing was signed. To grant them, install again with --accept-permissions
[exit 1]
$ sion skill install coder a888b3fc…b899 ae80a4d3…f9dc --skills installed --accept-permissions
bundle b0c0a037 checked: 2 files, skill pypi-version 1.0.0
permissions it asks for:
  its scripts may connect to pypi.org
  its scripts receive the secret PYPI_TOKEN
  its scripts are stopped after at most 30 s
coder signs: Rely on claim ae80a4d3 for up to 100 USD until Pulse 10019, reserving its backing. Fee 0.0012 USD; valid until Pulse 40.
Pulse 22: Done: reserved 200 USD of claim ae80a4d3's backing; fee 0.0012 USD.
installed pypi-version 1.0.0 in installed/pypi-version
$ cat installed/pypi-version/.source
{
  "accepted_at": 22,
  "audits": [],
  "bundle": "b0c0a037…2a34",
  "name": "pypi-version",
  "permissions": [
    "its scripts may connect to pypi.org",
    "its scripts receive the secret PYPI_TOKEN",
    "its scripts are stopped after at most 30 s"
  ],
  "publisher": "d2e053a4…9033",
  "release": "ae80a4d3…f9dc",
  "relied_on_until": 10019,
  "value_at_risk": "100",
  "version": "1.0.0"
}
```

## What the agent cannot grant itself

- A skill it writes with `skill_write` keeps only a name, a description
  and instructions. Front matter of its own in the instructions is
  instructions; rewriting a skill the operator placed drops what that
  skill declared, and says so.
- Evolving a skill (`sion skill evolve`) keeps its front matter as it
  was, and never writes `SKILL.md` or a script.
- A workspace, or an MCP server's directory, may not overlap the skills
  directory, shell or not: the agent could otherwise write a `SKILL.md`
  of its own and read it.

```text
$ sion agent run coder a888b3fc…b899 --task x --model script:pypi.json --workspace ~/shared --skills ~/shared/skills
error: the skills directory ~/shared/skills and ~/shared overlap: the agent could change a skill without its checks, or grant itself what a skill declares; keep them apart
[exit 1]
```

- A bundle's paths must be printable ASCII, without `\ : * ? " < > |`,
  and distinct when case is ignored, and evolution compares paths the
  same way. On macOS and Windows, `skill.md` or `scripts/Run.sh` would
  otherwise take the place of `SKILL.md` or `scripts/run.sh` when
  written.

## What this does not do

- A declared host is trusted for everything it does: one that forwards
  what it is sent, serves what others put there, or shares a front (a
  CDN) whose other sites a script can ask for inside the encrypted
  tunnel takes the script further (`agent:declared-hosts`).
- Secrets are as safe as the key file beside them, and a script that
  receives one can send it wherever its skill may connect, or change it
  so that it is not recognised in its output (`agent:secret-store`).
- The log of a script's connections is not signed, so it is not yet
  evidence that could slash the bond behind a release that reached
  further than it declared.
- With Docker, declared hosts are not filtered.
