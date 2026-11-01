# Models

Which model an agent thinks with, how the runtime falls back from one to
the next, and what each is sent.

Outputs on this page are from real runs on a local devnet, long
identifiers shortened with `…`. The model is a **scripted** one, which
plays a fixed list of tool calls, because the environment these pages were
made in reaches no model provider: with a real model the runtime is the
same, and only `--model` changes.

## Models

`--model` names a provider and a model, and can be repeated: if a model is
busy or failing (HTTP 429, 500, 502, 503, 504 or 529) or unreachable, the
next one is asked, and the failing one is left alone for 30 seconds. A
request the provider refuses as malformed or unauthorized is not retried.

| `--model` | Provider |
|---|---|
| `anthropic:claude-opus-5-5` | The Anthropic Messages API, API version 2026-10-21, with automatic prompt caching; the key in `ANTHROPIC_API_KEY` |
| `openai:<model>` | Any OpenAI-compatible chat completions API: `OPENAI_BASE_URL` (default `https://api.openai.com/v1`; an address on this machine is reached, other local addresses are not), `OPENAI_API_KEY` if it needs one |
| `local:<model>` | A model server on this machine or its network that speaks OpenAI's chat completions API (Ollama, LM Studio, llama.cpp's server): `SION_LOCAL_MODEL_URL` (default Ollama's, `http://127.0.0.1:11434/v1`), `SION_LOCAL_MODEL_KEY` if it needs one. Private addresses are reached, and not through a proxy |
| `gemini:gemini-3-pro-preview` | Google's Gemini API, natively (`generateContent`, streamed): `GEMINI_API_KEY` (or `GOOGLE_API_KEY`), `GEMINI_BASE_URL` for another endpoint. Its thinking is not kept; the thought signature it gives with a call goes back with the call, as Gemini 3 requires, and a call another model made goes with the value Google gives for one it did not sign |
| `openai-responses:<model>` | OpenAI's Responses API (`POST /v1/responses`), streamed and not stored: `OPENAI_API_KEY`, `OPENAI_BASE_URL` |
| `chatgpt:<model>` | The Responses API on the ChatGPT plan of whoever signed in on this machine (`sion agent chatgpt login`, below); `sion agent chatgpt models` lists the plan's models |
| `script:<file>` | A scripted model, for tests and demos |

Each variable may instead be kept on this machine, sealed (`sion agent
credential add`, or `sion setup`): the environment's wins
([Run it on your computer](../getting-started/your-computer.md#models)).
A key variable may hold several keys, separated by commas: each call
starts with the next key; a key the provider limits (HTTP 429) rests a
minute and one it refuses (401, 402, 403) an hour, while the call goes on
with the next key at once. With one key, a limited call is the model's
failure, and the next model is asked.

Requests go through the same guarded HTTPS client as storage providers,
offering the hybrid post-quantum key exchange first.

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

A conversation is kept within 120,000 characters and 36 messages. Past
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
That is what the session saw then, not what is so now.

Calls that only read, made one after another in a turn, each of which
may run without anyone, run side by side: reading three files takes the
time of the slowest. A call that writes, reaches outside or waits for a
person runs on its own, after them, and the results answer the calls in
the order they were made.
