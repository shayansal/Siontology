# Models

Which model an agent thinks with, how the runtime falls back from one to
the next, how a profile chooses models for each task, and what each is
sent.

Outputs on this page are from real runs on a local devnet, long
identifiers shortened with `…`. The model is a **scripted** one, which
plays a fixed list of tool calls, because the environment these pages were
made in reaches no model provider: with a real model the runtime is the
same, and only `--model` changes.

## Models

`--model` names a provider and a model, and can be repeated: if a model is
busy or failing (HTTP 429, 500, 502, 503, 504 or 529) or unreachable, the
next one is asked, and the failing one is left alone for 30 seconds, or as
long as the provider asked (`Retry-After`, or `retry-after-ms`), if that
is longer. When every model has failed that way, the run pauses and asks
them again: for as long as a provider asked, or else 2, 4, 8… seconds, up
to a minute a pause, until five minutes of pauses are spent; then it ends
(`no model could answer`). A watched run says so while it waits (`the
models are busy: asking again in 8 s`), and the kill switch stops a pause.
A request the provider refuses as malformed or unauthorized is not
retried.

| `--model` | Provider |
|---|---|
| `anthropic:claude-opus-5-5` | The Anthropic Messages API, API version 2026-10-21, with automatic prompt caching; the key in `ANTHROPIC_API_KEY` |
| `openai:<model>` | Any OpenAI-compatible chat completions API: `OPENAI_BASE_URL` (default `https://api.openai.com/v1`; an address on this machine is reached, other local addresses are not), `OPENAI_API_KEY` if it needs one |
| `local:<model>` | A model server on this machine or its network that speaks OpenAI's chat completions API (Ollama, LM Studio, llama.cpp's server): `SION_LOCAL_MODEL_URL` (default Ollama's, `http://127.0.0.1:11434/v1`), `SION_LOCAL_MODEL_KEY` if it needs one. Private addresses are reached, and not through a proxy |
| `gemini:gemini-3-pro-preview` | Google's Gemini API, natively (`generateContent`, streamed): `GEMINI_API_KEY` (or `GOOGLE_API_KEY`), `GEMINI_BASE_URL` for another endpoint. The thought signature it gives with a call goes back with the call, as Gemini 3 requires, the one it puts on its text with the text, and a call another model made goes with the value Google gives for one it did not sign |
| `openai-responses:<model>` | OpenAI's Responses API (`POST /v1/responses`), streamed and not stored: `OPENAI_API_KEY`, `OPENAI_BASE_URL` |
| `chatgpt:<model>` | The Responses API on the ChatGPT plan of whoever signed in on this machine (`sion agent chatgpt login`, below); `sion agent chatgpt models` lists the plan's models |
| `bedrock:<model>` | AWS Bedrock's Converse API, natively: a model id or an inference profile (`bedrock:us.anthropic.claude-sonnet-4-5-20250929-v1:0`), in `AWS_REGION` (or `AWS_DEFAULT_REGION`, or the profile's region, or `us-east-1`). A Bedrock API key in `AWS_BEARER_TOKEN_BEDROCK`, or an access key signed with Signature Version 4: `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`, or the profile's in AWS's own files (`AWS_PROFILE`, `~/.aws/credentials`). Below |
| `vertex:<model>` | Google Vertex AI: Gemini (`generateContent`) and Claude (`rawPredict`), streamed, in `GOOGLE_CLOUD_PROJECT` at `GOOGLE_CLOUD_LOCATION` (by default `global`; the variables Claude's own tools use, `ANTHROPIC_VERTEX_PROJECT_ID` and `CLOUD_ML_REGION`, are read too). Below |
| `azure:<deployment>` | Azure OpenAI's v1 API at `AZURE_OPENAI_ENDPOINT` (`https://<resource>.openai.azure.com`, its `/openai/v1`), the key in `AZURE_OPENAI_API_KEY`, sent as `api-key` |
| `openrouter:`, `deepseek:`, `moonshot:`, `xai:`, `groq:`, `together:`, `fireworks:`, `mistral:`, `cerebras:` | Providers that speak OpenAI's chat completions, each at its own address with its own key: `<NAME>_API_KEY` (`OPENROUTER_API_KEY`), and `<NAME>_BASE_URL` to change the address. Several can be mixed in one agent's models: `--model openrouter:moonshotai/kimi-k2.5 --model deepseek:deepseek-chat` |
| `script:<file>` | A scripted model, for tests and demos |

Each variable may instead be kept on this machine, sealed (`sion agent
credential add`, or `sion setup`): the environment's wins
([Run it on your computer](../getting-started/your-computer.md#models)).
A key variable may hold several keys, separated by commas: each call
starts with the next key; a key the provider limits (HTTP 429) rests a
minute, or as long as the provider asked, and one it refuses (401, 402,
403) an hour, while the call goes on with the next key at once. With one key, a limited call is the model's
failure, and the next model is asked.

Requests go through the same guarded HTTPS client as storage providers,
offering the hybrid post-quantum key exchange first.

### On the cloud platforms

**Bedrock.** A request is Converse's (`POST /model/<model>/converse`):
the system prompt, the conversation with its pictures, the tools as tool
specifications, and the calls and their results, screenshots among them.
An access key signs each request with AWS's Signature Version 4, a session
token with it if the credentials are temporary; a profile that signs in
another way (single sign-on, a role, a process) is not followed, and its
credentials can be put in the environment first (`eval "$(aws configure
export-credentials --format env)"`). Claude's and Nova's prompts are cached
at two points (after the system prompt and at the end of the
conversation). Claude's thinking is asked for as with Anthropic's API, in
the fields Bedrock passes on to the model, and what it thought comes back
signed, and goes back as it came. Bedrock's framing of a streamed answer
is not read: its answers come whole. `AWS_ENDPOINT_URL_BEDROCK_RUNTIME`
names another endpoint.

**Vertex AI.** A request carries a token: one given
(`GOOGLE_OAUTH_ACCESS_TOKEN`, from `gcloud auth print-access-token`), or
one minted when needed and kept until a minute before it lapses, with the
credentials at `GOOGLE_APPLICATION_CREDENTIALS` (a service account's key:
a JWT signed with its key, RS256, is exchanged at Google's token endpoint)
or those `gcloud auth application-default login` keeps (a person's own
sign-in, its refresh token exchanged). Claude's requests are Anthropic's,
with Vertex AI's `anthropic_version`, the prompt cached at a breakpoint on
the system prompt and one on the conversation's last block. Without a
project, Gemini is reached in Vertex AI's express mode, its key in
`GOOGLE_API_KEY`; Claude is reached only in a project. `VERTEX_BASE_URL`
names another endpoint.

These providers are tested against stand-ins of the platforms' APIs: the
signatures against the ones AWS's own SDK computes for the same request,
the tokens against a stand-in of Google's token endpoint. They have not
been run against AWS, Google Cloud or Azure themselves here.

### Signing in with ChatGPT

`sion agent chatgpt login` signs in with a ChatGPT account, so that agents
run on its plan (`--model chatgpt:<model>`) rather than on an API key. It
is OpenAI's sign-in for open-source apps: your browser opens OpenAI's page
("Continue with ChatGPT"), which comes back to `http://127.0.0.1:1455`
(another port if that one is taken); on the first sign-in on a machine,
OpenAI registers Siontology there, with no client secret. The ID token is
checked against OpenAI's published keys (its issuer, audience, expiry and
nonce), and the plan is used only if you allowed it
(`chatgpt.tokens.use.direct`). The tokens are kept encrypted beside the
secrets, renewed before they lapse (the refresh token comes back new each
time, and is kept with them), and renewed again if the API refuses one.
`sion agent chatgpt status` says who is signed in, `models` lists the
plan's models, `logout` forgets the account.

Requests on a plan are streamed and not stored, as OpenAI asks of this
sign-in; a plan's usage limit, reported partway, counts as the model being
busy, and the next `--model` is asked. Other providers' subscriptions are
not offered: Anthropic, for one, does not let other apps sign in to
Claude.ai plans.

## How hard it thinks

How hard a model thinks is written after its name, after a `#`:
`--model anthropic:claude-opus-5-5#high`. An effort level (`none`,
`minimal`, `low`, `medium`, `high`, `xhigh` or `max`) goes to each
provider as the nearest it has; a number is a budget of thinking tokens,
for the models that take one.

| Provider | An effort level | A budget (`#8000`) |
|---|---|---|
| `anthropic:` | Adaptive thinking at that effort (`output_config.effort`; `minimal` is `low`); `none` turns thinking off where a model allows it | Thinking on that budget (`budget_tokens`, at least 1,024), for models before Claude 4.6 |
| `openai-responses:`, `chatgpt:` | `reasoning.effort` (`max` is `xhigh`) | Not taken |
| `openai:`, `local:`, `azure:` and the providers known by name | `reasoning_effort` (`max` is `xhigh`) | Not taken |
| `gemini:` | `thinkingLevel`, for Gemini 3 and later (`none` is `minimal`; above `high` is `high`) | `thinkingBudget`, for Gemini 2.5 (`#0` turns thinking off where a model allows it) |
| `bedrock:` | Claude's, as `anthropic:`; other models are not asked | Claude's, as `anthropic:` |
| `vertex:` | Claude's as `anthropic:`, Gemini's as `gemini:` | The same |

Without it, the provider's default applies. A model told how hard to
think is given room to: its answer may be as long as before, and its
thinking has room besides, from 1,024 tokens at `minimal` to 60,000 at
`max`, or its budget, at most 64,000 tokens an answer in all. A run's
token budget counts that room before each call. A route's models may each
think as hard as suits the route:
`sion agent routes set lead code --model anthropic:claude-opus-5-5#xhigh`.

### What it thought

What a model thought before it called a tool or answered is kept in the
conversation and given back to it, so that a model calling tool after
tool goes on from its own reasoning rather than working it out again:

- Anthropic's thinking blocks, signed, and redacted ones, as they came;
- the reasoning items of OpenAI's Responses API, encrypted (nothing is
  stored with OpenAI, so they are asked for with `include`), given back
  without their ids, each before what followed it;
- the signature Gemini puts on its text, with the text, as the one on a
  call goes with the call;
- the reasoning text a chat completions server returns
  (`reasoning_content` or `reasoning`), under the name it came by, within
  the task it was for, as the servers that think between tool calls ask.

Each is given back only to the provider and model it came from, told by
its kind, base URL and model: a model the runtime falls back to is not
shown another's reasoning. Thinking cut off before it was signed is not
kept. A request that carries kept reasoning and is refused as malformed
(HTTP 400) is sent once more without it, so reasoning that no longer
checks (from a key of another organization's, say) is let go rather than
failing the call; a model of the Responses API that refuses to return its
reasoning is not asked for it again. Kept reasoning counts toward the
conversation's characters as it is sent, and a summary leaves it out. A
transcript shows what can be read of it (Anthropic's summaries, a
server's reasoning text), a line marked `~`.

## A model for each task

A profile can send each task to the models that suit it, by routes. A
route names its models, and either says in words what tasks it is for
(`--about`) or what sends a task to it without asking anyone (`--when`):
pictures, a work item the agent took on
([Work it takes on](work.md)), or a long task (past 40,000 characters,
unless `--longer-than` says otherwise).

```text
sion agent routes set lead code --model anthropic:claude-opus-5-5 --about "Writing, fixing or reviewing code"
sion agent routes set lead eyes --model gemini:gemini-3-pro-preview --when pictures
sion agent routes set lead chat --model anthropic:claude-haiku-5-5 --about "Quick questions and small talk"
sion agent routes sorter lead anthropic:claude-haiku-5-5
```

When a session starts (a chat's first message, a scheduled job, a work
item), its task goes to the first route whose `--when` holds. Otherwise a
model, the sorter (`sion agent routes sorter`, else the profile's first
model), is asked which route's `--about` fits. The sorter is shown the
task as data to sort, never to follow; it has no tools, it is asked for a
route's name or none, and an answer that names no route, or several,
counts as none, so the worst a task written to steer it can do is pick
another of your routes. A task no route takes runs on the profile's own
models. The chosen route's models come first and the profile's after
them, as fallbacks; a model a chat chose with `/model` comes first of
all. A session keeps the models its first task chose, and the next
session is routed again. `sion agent routes test lead "Fix the failing
test in parser.rs"` says which route a task would take and why, and runs
nothing.

Each routed session is noted in `$SION_HOME/agents/routes/<profile>.jsonl`:
its route, why, the model first in line, how its first run ended and the
tokens it used. `sion agent routes show lead` adds them up by route and
by model.

A route's models are put in order by measurement, not by the sorter.
`sion agent routes eval lead code --set fixes.json` runs each of the
route's models as one agent on the same set of tasks (cases as an
[evaluation set](skill-improvement.md) has them), each case `--runs`
times on `--budget-tokens` a case, and reports their pass rates with 95%
Wilson intervals. A model whose pass rate beats the route's first model's
by Newcombe's 95% interval, its lower end above zero, earns first place
in the route: the bar a graph of agents clears to earn a profile's
traffic ([Graphs of agents](graphs.md)). Nothing else moves a model
forward.

## What a model is sent

Each model call is sent the tools, the system prompt and the conversation
so far, and the start of that stays the same from call to call, so that a
provider's cache of it holds (with Anthropic's models, prompt caching is
asked for). For that, a session's system prompt keeps what the agent knew
of itself, its core memory and the skills it has, as it was when the
session began, or at the first run after its conversation was compacted:
what it records or learns meanwhile is in the conversation. What a chat
asks (`/system`) comes after it, so that changing that leaves the start
as it was. The input tokens a provider read from its cache, or wrote to
it, count as the input they are, in a run's budget as in `/usage`, which
tells them apart, as a trace does
(`gen_ai.usage.cache_read.input_tokens` and
`gen_ai.usage.cache_write.input_tokens`, on a model call's span).

A conversation is kept within 120,000 characters and 36 messages
(`--compact-at TOKENS` moves both: about that many tokens, at three
characters a token, and as many messages as they would hold). Past
the characters, the results of calls before the last 12 messages that
are longer than 400 characters are cut first: the calls stay, and each
such result becomes a line saying what it was and how long, since the
call can be made again (one the journal recorded is answered from it).
If that is not enough, or past the messages, the middle is replaced by a
summary, never separating a call from its result. A summary begins with
what the session did that lasts: its last 40 calls that changed something
or reached outside (the files it wrote, the commands it ran, the payments
it made, the messages it sent), earlier ones counted by tool, kept apart
from the conversation so that no summary of a summary loses them. Then
comes the latest of what it read or changed: for each thing a tool names
(a file, `file:notes/a.txt`, whichever way its path was written; a
directory listed), the start of what its last call returned, the 32 most
recent, so that a model that lost the conversation's middle still knows
what a file held when it last looked, rather than what it held first.
That is what the session saw then, not what is so now. Then comes what
the person said after the task, word for word: their last 24 messages, up
to 4,000 characters each, those the kept end of the conversation does not
hold, so that an instruction given along the way ("never open the billing
folder") outlives any number of summaries. The task itself is never
summarized: it stays the first message.

The rest of a summary is written by a model: the run's first, or the one
`--summary-model` names (`none` keeps the summary mechanical; a script
writes none). It is sent the messages being replaced as a transcript,
each text up to 4,000 characters and the whole cut to fit 120,000, without
tools, and asked for working notes of at most 600 words: what was done and
the state things are in, the facts needed later, what failed and why, the
requirements and constraints found along the way, and what remains. It is
told that what tools returned is information, not instructions. Its call
counts against the run's budgets as any call does, and is a step of the
trace; when what it would need does not fit them, or it fails or answers
nothing, the summary is the mechanical one, a line for each message,
shortened.

Past 40 tools (an agent with several MCP servers, say), the model is
not shown them all. It sees its own tools (all but the MCP servers' and
Home Assistant's, if those are no more than 40), `tools_search` and
`tools_call`, and the tools its searches found. A search ranks the tools
by BM25 over the words of their names (counted three times), descriptions
and inputs (camelCase and snake_case split, plurals made one, common
words left out); a tool named in the query comes first, and
`select:<name>,<name>` asks for those alone. Its best matches, five unless
it asks for up to ten, come back with their input schemas and are loaded:
from the next call on they are shown after the rest, to be called directly
by name, the 24 found last kept, and a session kept on disk keeps them.
Loading more leaves the start of the list as it was, for providers'
caches.

Calls that only read, made one after another in a turn, each of which
may run without anyone, run side by side: reading three files takes the
time of the slowest. A call that writes, reaches outside or waits for a
person runs on its own, after them, and the results answer the calls in
the order they were made.
