# Channels

An agent is reached over HTTP, OpenAI's chat API, A2A, an agent gateway
and [the messengers](messengers.md), on its own machine or through others.
What follows holds on every channel.

Outputs on this page are from real runs on a local devnet, long
identifiers and some JSON shortened with `…`. The models are scripted, and
the messengers run against stand-ins for their APIs, because the
environment these pages were made in reaches none of them. The runs of
subagents and hiring predate specification v1.13 (run again on v1.13, they
printed the same lines with other identifiers); a profile's own tokens,
Slack and the agent gateway are from a run on v1.15.


Each conversation on a channel is a session, continued message by
message. On a messenger that keeps threads apart (a Slack or Mattermost
thread, a Telegram topic), each thread is a conversation of its own.
These commands work on every channel:

| Command | What it does |
|---|---|
| `/stop` | stops what the agent is doing, at once: a model that is writing is cut off there, what it wrote kept |
| `/approve`, `/decline <why>` | decide the call the session waits on; given its request's id first (`/approve 3f2a9c1e`), only if it is still the one waiting |
| `/approve always`, `/approve always <rule>` | approve it, and for the rest of the session the same call again, or the calls of its tool the rule matches (`write_file(notes:*)`, `shell(git status:*)`) |
| `/allowed`, `/allowed off` | what the session may do without asking beyond its autonomy level, and withdrawing it |
| `/new` | starts over, in a new session |
| `/status` | where the session stands |
| `/retry` | answers the last message again |
| `/undo` | forgets the last message and its answer (what its calls did stays done) |
| `/compact` | shortens the session, its start kept as a summary |
| `/usage` | the turns, tokens and messages the session used, its subagents' tokens, and a run's budget if it has one |
| `/sessions`, `/resume <id>` | this conversation's sessions, and going back to one of them |
| `/title <name>` | names the session |
| `/model`, `/model <number>` | the profile's models, and which one the session uses |
| `/system`, `/system <text>`, `/system off` | what the chat asks of the agent, after its operator's instructions |
| `/tools`, `/tools <name> …`, `/tools all` | the only tools the agent may use in the chat |
| `/whoami` | on a bridge: who you are there, and what you may do |
| `/help` | these commands |

`/approve`, `/decline`, `/model`, `/system` and `/tools` are for those
who may decide: on a bridge, those `--approvers` names (anyone in a chat
it answers, if it names no one); on the HTTP channel and through a
gateway, a token made for approvals. The session keeps each decision with
who made it, as the channel names them (`slack:U07`, `http:token
3046d92e`, `cli:<user>` on the command line), and the run shows it:
`(approved by slack:U07)`. What `/approve always` allows is never a
payment, which is approved on its own, nor a call whose argument comes
from content held in quarantine; it is not given while the session
has read content from outside, nor holds then; and it never goes past
what the autonomy level refuses, as for [a skill's
pre-approvals](skill-permissions.md).

While the agent works in a conversation, `/stop`, `/status`, `/help`,
`/usage` and `/sessions` are answered at once; any other command waits
for the agent on a bridge, and is refused on the HTTP channel and through
a gateway (which answer many requests at once, so a `/stop` there reaches
a run another request started).

What a chat chooses holds for its sessions, the one it is in and those it
starts later, and is kept with the channel's state: `/model <number>`
the model they start with (one of the profile's), `/system <text>` what
they are asked after the operator's instructions (`/system off` takes it
back), `/tools <name> …` the only tools they may use of those the
profile gives (`/tools all` gives them all again).

A bridge keeps each message it has taken until it has answered it, so a
message taken when the bridge stopped is answered when it starts again.
A chat whose answer the stop cut off is told so first ("I was restarted
while I worked on what you sent, so I am taking it up again."), and is
answered again: it may have seen part of the first answer.

## HTTP

`sion agent serve` answers `POST /v1/profiles/<name>/messages` with
`{"text": …, "conversation": …}`, and `GET /v1/sessions/<id>`. Every
request needs a bearer token: the machine's, in `SION_AGENT_TOKEN` (at
least 16 characters), which reaches every profile and session, or one of
a profile's own (below):

```text
$ SION_AGENT_TOKEN=… sion agent serve --listen 127.0.0.1:8791
agent channel on http://127.0.0.1:8791 (POST /v1/profiles/<name>/messages)
```

```text
$ curl -s -X POST http://127.0.0.1:8791/v1/profiles/lead/messages -d '{"text": "How much budget is left?"}'
{"error":"a bearer token is needed"} (HTTP 401)
$ curl -s -X POST http://127.0.0.1:8791/v1/profiles/lead/messages -H "Authorization: Bearer $SION_AGENT_TOKEN" -d '{"text": "How much budget is left?", "conversation": "ops"}'
{"conversation":"ops","lines":["  → ledger_balance {}","  ← {\"budget_remaining_usd\":\"85\",…}","agent: 85 USD left in the budget.","session 37702955dda47c36: done"],"said":"85 USD left in the budget.","session":"37702955dda47c36"}
$ curl -s -X POST http://127.0.0.1:8791/v1/profiles/lead/messages -H "Authorization: Bearer $SION_AGENT_TOKEN" -d '{"text": "/status", "conversation": "ops"}'
{"conversation":"ops","lines":[],"said":"85 USD left in the budget.","session":"37702955dda47c36"}
```

With `"stream": true`, the answer comes as Server-Sent Events while the
agent works: `session` with the session's id as its run begins, `delta`
with the text it wrote since the last (every answer of the run a
paragraph), `tool` with a tool it calls, runs or waits on (`"state"`:
`calling`, `running`, `waiting`), and `done` with what the whole answer
would have been (or `error`).

Requests are handled one at a time.

## A profile's own tokens

`sion agent profile token add <profile>` makes a token that reaches only
that profile, and names the profile's agent. It is shown once, and the
machine keeps only its hash. Its holder may talk to the profile; with
`--approve`, they may also approve or decline what the profile's
sessions wait on. `sion agent serve` takes these tokens with or without
the machine's token:

```text
$ sion agent profile token add lead
token 5957d62c for profile lead, agent 94629049 (messages only); it is shown once, and kept here only as its hash:
sion_ch_c7307346…
$ sion agent profile token add payer
token 2b871571 for profile payer, agent 23e91a81 (messages only); it is shown once, and kept here only as its hash:
sion_ch_3a16ec5f…
$ sion agent profile token add payer --approve
token 3046d92e for profile payer, agent 23e91a81 (messages, approvals and declines); it is shown once, and kept here only as its hash:
sion_ch_ea2edb78…
$ sion agent profile token list
5957d62c  lead  agent 94629049  messages
2b871571  payer  agent 23e91a81  messages
3046d92e  payer  agent 23e91a81  messages, approvals
$ sion agent serve --listen 127.0.0.1:8794
agent channel on http://127.0.0.1:8794 (POST /v1/profiles/<name>/messages; OpenAI's /v1/chat/completions and /v1/models)
```

Below, `$LEAD_TOKEN` is lead's token, `$TALK` payer's first and `$DECIDE`
its second. payer runs at L5, where a payment waits for a person:

```text
$ curl -s -X POST http://127.0.0.1:8794/v1/profiles/payer/messages -H "Authorization: Bearer $LEAD_TOKEN" -d '{"text": "Pay bob 1 USD for invoice 7.", "conversation": "ops"}'
{"error":"this token does not reach this profile"} (HTTP 401)
$ curl -s -X POST http://127.0.0.1:8794/v1/profiles/payer/messages -H "Authorization: Bearer $TALK" -d '{"text": "Pay bob 1 USD for invoice 7.", "conversation": "ops"}'
{"conversation":"ops","files":[],"lines":["  → ledger_pay {\"amount\":\"1\",\"purpose\":\"invoice 7\",\"to\":\"bob\"}","  ← waiting for a person's approval","session 3ffd3b1fd4d6aa36: waiting for you: ledger_pay {\"amount\":\"1\",\"purpose\":\"invoice 7\",\"to\":\"bob\"} (spending at autonomy L5; request e415977a). Answer with `sion agent continue 3ffd3b1fd4d6aa36 --approve` or `--decline <why>`."],"said":"waiting for your approval: ledger_pay {\"amount\":\"1\",\"purpose\":\"invoice 7\",\"to\":\"bob\"} (spending at autonomy L5; request e415977a)","session":"3ffd3b1fd4d6aa36"} (HTTP 200)
$ curl -s -X POST http://127.0.0.1:8794/v1/profiles/payer/messages -H "Authorization: Bearer $TALK" -d '{"text": "/approve e415977a", "conversation": "ops"}'
{"error":"this token may talk to the profile, not approve or decline"} (HTTP 403)
$ curl -s -X POST http://127.0.0.1:8794/v1/profiles/payer/messages -H "Authorization: Bearer $DECIDE" -d '{"text": "/approve e415977a", "conversation": "ops"}'
{"conversation":"ops","files":[],"lines":["  → ledger_pay {\"amount\":\"1\",\"purpose\":\"invoice 7\",\"to\":\"bob\"} (approved by http:token 3046d92e)","  ← Pulse 14: Done: agent paid 1 USD to bob (receipt claim cee25b76); fee 0.0012 USD. Transaction fc22f4f4…3633.","agent: Paid bob 1 USD for invoice 7.","session 3ffd3b1fd4d6aa36: done"],"said":"Paid bob 1 USD for invoice 7.","session":"3ffd3b1fd4d6aa36"} (HTTP 200)
```

lead's token does not reach payer. `$TALK` starts the payment, which
waits, but cannot approve it; `$DECIDE` can, naming the request it was
shown, and the session keeps the approval as that token's.
`sion agent profile token remove <profile> <id>` takes a token back.

## OpenAI's chat API

The same server answers OpenAI's chat API, so apps made for it (Open
WebUI, LibreChat, editors, scripts) can talk to a profile: point the app
at `http://<listen>/v1` with a profile's token as its key, and choose the
profile as the model. `GET /v1/models` lists the profiles the token
reaches. Such apps send the whole conversation with every message; the
server finds the session by what was said before the last message, so a
conversation goes on in its session, approvals included (`/approve` and
`/decline <why>`, with a token made to decide). A conversation it has not
seen (edited in the app, or begun elsewhere) starts a new session, told
what was said before. With `"stream": true`, the answer comes as
Server-Sent Events while the agent works: each chunk carries what it wrote
since the last, every answer of the run a paragraph, and the chunks' `id`
names the session, as a whole answer's does. A scripted model, as here,
writes its answer at once, so it comes in one chunk.

```text
$ sion agent profile token add assistant
token 843839a6 for profile assistant, agent 908f6ddc (messages only); it is shown once, and kept here only as its hash:
sion_ch_0c6b4e7a…
$ sion agent serve --listen 127.0.0.1:8794
agent channel on http://127.0.0.1:8794 (POST /v1/profiles/<name>/messages; OpenAI's /v1/chat/completions and /v1/models)
$ curl -s http://127.0.0.1:8794/v1/models -H "Authorization: Bearer $TOKEN"
{"data":[{"created":0,"id":"assistant","object":"model","owned_by":"sion"}],"object":"list"} (HTTP 200)
$ curl -s http://127.0.0.1:8794/v1/chat/completions -H "Authorization: Bearer $TOKEN" -d '{"model": "assistant", "messages": [{"role": "user", "content": "How much budget is left?"}]}'
{"choices":[{"finish_reason":"stop","index":0,"message":{"content":"100 USD left in the budget.","role":"assistant"}}],"created":1793394351,"id":"chatcmpl-b3362ad0e5d01a89","model":"assistant","object":"chat.completion"} (HTTP 200)
$ curl -s http://127.0.0.1:8794/v1/chat/completions -H "Authorization: Bearer $TOKEN" -d '{"model": "assistant", "messages": [{"role": "user", "content": "How much budget is left?"}, {"role": "assistant", "content": "100 USD left in the budget."}, {"role": "user", "content": "Is anything waiting for me?"}]}'
{"choices":[{"finish_reason":"stop","index":0,"message":{"content":"Nothing is waiting for you.","role":"assistant"}}],"created":1793394351,"id":"chatcmpl-b3362ad0e5d01a89","model":"assistant","object":"chat.completion"} (HTTP 200)
$ curl -s http://127.0.0.1:8794/v1/chat/completions -H "Authorization: Bearer $TOKEN" -d '{"model": "assistant", "stream": true, "messages": [{"role": "user", "content": "How much budget is left?"}]}'
data: {"choices":[{"delta":{"content":"","role":"assistant"},"finish_reason":null,"index":0}],"created":1793394352,"id":"chatcmpl-31547ec245a7868a","model":"assistant","object":"chat.completion.chunk"}

data: {"choices":[{"delta":{"content":"100 USD left in the budget."},"finish_reason":null,"index":0}],"created":1793394352,"id":"chatcmpl-31547ec245a7868a","model":"assistant","object":"chat.completion.chunk"}

data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}],"created":1793394352,"id":"chatcmpl-31547ec245a7868a","model":"assistant","object":"chat.completion.chunk"}

data: [DONE]

 (HTTP 200)
```

The second answer is the same session's (`chatcmpl-b3362ad0e5d01a89`); the
streamed one, a conversation the server had not seen, began another. The
answer's `id` names the session (`sion agent trace`).

## Agents that talk A2A

`sion agent serve --a2a <profile>` gives other agents an Agent2Agent
card for the profile, at `/.well-known/agent-card.json` and
`/v1/profiles/<profile>/agent-card.json`, and answers A2A's JSON-RPC at
`POST /v1/profiles/<profile>/a2a`, with one of the channel's tokens
(`--public-url` is the address the card gives, if not the one the
request came to). Both wire versions are spoken, as the request names
them: 0.3 (`message/send`, `message/stream`, `tasks/get`, `tasks/cancel`)
and 1.0 (`SendMessage`, `SendStreamingMessage`, `GetTask`, `CancelTask`).
A message's `contextId` is a conversation, each a session as on every
channel; each message is a task, `completed` when the agent is done, or
`input-required` while the session waits for an approval, which a token
made to decide gives in the same context (`/approve`). Another agent's
message is content from outside: the session is tainted, so its external
calls and payments wait for a person, as after reading a web page. A streamed task
sends its reply as an artifact that grows, and ends with its state,
marked final. The card names no instructions of the operator's: only
what the agent is.

A client may have a task's updates pushed to it rather than wait or
stream: it registers a webhook for the task (0.3's
`tasks/pushNotificationConfig/set`, `get`, `list` and `delete`; 1.0's
`CreateTaskPushNotificationConfig` and the rest), or gives one with its
message (`configuration.pushNotificationConfig`, or 1.0's
`taskPushNotificationConfig`), and each time the task's state changes the
server POSTs the task, as it stands, to it: with the client's token in
`X-A2A-Notification-Token`, and its Bearer credentials if it gave any (no
other scheme is offered). A webhook is an HTTPS address that resolves to
a public one; `--a2a-push-private` lets them be on this machine's network,
for testing. A task has at most eight, and they are forgotten with it.
`tasks/resubscribe` (1.0's `SubscribeToTask`) streams a task's state as it
changes, the task first, until it is final. Whoever holds a token that
reaches the profile may register a webhook for its tasks, anywhere public
(`agent:a2a-webhooks`).

## Pictures and files

What a person sends besides text reaches the agent. A picture (a PNG,
JPEG, GIF or WebP, known by its bytes whatever its name says, of 5 MB at
most) is shown to the model with what they wrote. With a workspace, every
file they send, pictures included, is put in the workspace's `inbox/`
under a name not taken yet, and the agent is told where, on a line of
its own after their words:

```text
(A picture was sent, put in your workspace as inbox/<name>: <media type>, <size> bytes.)
(A file was sent, put in your workspace as inbox/<name>: <media type>, <size> bytes.)
```

Without a workspace a picture is still shown, and another file is named
to the agent but not kept. A file of more than 25 MB is not taken, and a
file the messenger no longer gives is named as one that could not be
fetched. The model is shown the pictures of the person's last message
that has any, ten at most, besides the last three of the conversation
(such as a screenshot the agent took): an album sent at once is seen
whole, however many screenshots follow it. A session keeps the bytes of
those pictures and of the last six; the others stay named by their
digest.

The agent sends files back with `share_file`, given a file of its
workspace of 25 MB at most: it goes after the reply, as each messenger
takes files. With `--image-model openai:<model>` on its profile, and a
workspace, the agent also makes pictures (`image_generate`, through
OpenAI's Images API with `OPENAI_API_KEY`, or a compatible server at
`OPENAI_BASE_URL`): it is shown each, which is put in the workspace's
`images/`, named as it asks, for it to share. Making a picture is a
write, as the model calls are not gated: its description goes only to
the provider the operator chose, and spends the operator's credit there.

| `--via` | What a person sends | What the agent shares |
|---|---|---|
| `telegram` | photos (the largest size) and documents, with their captions | a picture as a photo, anything else (a GIF too) as a document, in the message's topic |
| `slack` | the files a message shares (`files:read`) | uploaded and shared in the message's thread (`files:write`) |
| `discord` | attachments | an attachment of a message |
| `matrix` | `m.image`, `m.video` and `m.file` messages, with their captions (not in encrypted rooms) | uploaded, as an `m.image` or an `m.file` |
| `signal` | attachments | attached |
| `mattermost` | the files of a post | uploaded, and posted in the thread |
| `whatsapp` | pictures, videos, documents and stickers, with their captions | a JPEG or PNG as a picture, anything else as a document |
| `email` | attachments, ten of a message at most | a message of its own in the thread, the file attached |
| `imessage` | attachments, where the bridge runs on the Mac | sent through imsg |
| `line` | pictures, videos and files | nothing: LINE takes a bot's files only from a public address |
| `sms` | an MMS's pictures, through Twilio | nothing: Twilio takes media only from a public address |
| the others | nothing: text and voice notes only | nothing |

Where the agent shares a file the messenger does not send, the person is
told it was shared and could not be sent.

The HTTP channel takes files with a message, `"files": [{"name": …,
"media_type": …, "base64": …}]` (a name and a media type if you like;
32 MB at most, the files with it), and answers with what the agent
shared, `"files": [{"name": …, "media_type": …, "size": …, "base64":
…}]`, in a stream's `done` event too. OpenAI's chat API takes pictures
sent inline, `image_url` parts whose URL is a `data:` one, in the last
message; a picture's own address is not fetched.

## Typing, and replies as they are written

While the agent works on a message, a bridge shows the chat that it is at
work, from the moment it takes the message, and shows the reply as the
model writes it: the models stream (Anthropic's and OpenAI's APIs, over
Server-Sent Events), so the text reaches the chat as it is written, not
when the run ends. Each messenger does it the way its API offers:

| `--via` | Typing | The reply as it is written |
|---|---|---|
| `telegram` | `sendChatAction` every 4 seconds (`record_voice` for a reply it will hear) | in a private chat, an animated draft (`sendMessageDraft`), shown again before Telegram's 30 seconds lapse, which the reply replaces; in a group, a message edited every 3 seconds (Telegram takes twenty a minute there) |
| `slack` | in a thread, Slack's status line (`assistant.threads.setStatus`, for an app that may set it); otherwise an hourglass on the message, taken off at the end | in a thread, Slack's own stream (`chat.startStream`), each tool the agent runs a task on its timeline; otherwise a message edited (`chat.update`) |
| `discord` | `POST /channels/…/typing` every 9 seconds | a message edited |
| `matrix` | typing for 30 seconds at a time, turned off at the end | a message replaced (`m.replace`), its updates marked live (MSC4357), one every 4 seconds and twelve at most: each is an event, and homeservers limit them |
| `mattermost` | `POST /users/me/typing`, in the message's thread | a post patched, in the thread |
| `teams` | `typing` activities every 3 seconds | in a chat with one person, Teams' own streaming: the tool the agent runs as an informative update, then the text |
| `google-chat` | none: Chat has none for apps | with a service account's key, a message patched, one write every second and a half (Chat takes one a second in a space) |
| `feishu`, `lark` | the `Typing` reaction on the message, taken off at the end | a streaming card (CardKit), which Feishu types out |
| `dingtalk` | none | on an AI card, given its template (`DINGTALK_CARD_TEMPLATE_ID`) |
| `whatsapp` | WhatsApp's typing indicator, which marks the message read | none: WhatsApp edits nothing a business sends |
| `line` | LINE's loading animation, in a chat with one person | none |
| `signal` | signal-cli's typing indicator, turned off at the end | none: Signal lets a message be edited ten times |
| `sms` | for a WhatsApp sender on Twilio, Twilio's typing indicator | none |
| `nextcloud-talk` | an hourglass on the message, taken off at the end: a Talk bot has no typing | none: a Talk bot edits nothing |
| `email`, `imessage`, `irc`, `twitch` | none | none |

Updates come no oftener than each platform takes them, the latest text
each time. A message that is edited as it grows is first sent with at
least 40 characters, so that the notification it makes has something to
read; a draft and a platform's own stream show at once, and with
`--stream progress` show the tool the agent runs before it writes. A reply
longer than one message holds is split where it reads best, at a
paragraph, a line, a sentence or a word, a code block closed at the end of
one part and opened again at the start of the next, each part finished as
it fills. A reply sent whole is split the same way: a long reply is no
longer cut.

The reply is everything the agent said in the run, each answer a
paragraph: what it said before it called a tool, and what it said after.
When the run ends, the typing indicator stops and the draft becomes that
reply, the same text, which the chat keeps.

`--stream progress` (the default) shows the text and, while a tool runs,
the tool; `--stream partial` shows the text only; `--stream off` sends
the reply once it is done. `--no-typing` shows no typing indicator.

## While the agent works

A bridge answers each chat on a thread of its own, `--parallel` chats at
once (four by default) and one turn at a time in each, the chat that has
waited longest first. Messages a chat sends within `--debounce`
milliseconds of each other (500 by default) are one turn: someone who
writes in three short messages gets one answer, and in a group each line
says who wrote it. What a chat says while the agent works on it does what
`--busy` says:

| `--busy` | A message that comes while the agent works |
|---|---|
| `steer` (the default) | is told to the agent before its next step, as "(The person said, while you worked:) …", and answered with the rest; if the agent is done before that step, it is the next turn |
| `queue` | is answered once the agent is done, each message a turn of its own |
| `collect` | is answered once the agent is done, all of them one turn |
| `interrupt` | stops the agent, whose reply is what it wrote so far, and is the next turn |

A command or a voice note waits for its turn whatever `--busy` says, but
`/stop` does not wait: the agent stops before its next model or tool
call, a model that is writing is cut off between two words (its
connection closed, so that the provider stops too), what it wrote is kept
in the session, and the reply ends with "Stopped.". What waited in the
chat is dropped.

Each message the bridge takes is marked where the messenger can: 👀 while
the agent will answer it or is at it, taken off once it has answered, and
⚠️ if answering failed. `--no-reactions` marks nothing.

| `--via` | Marks |
|---|---|
| `telegram` | the bot's reaction, 👀, gone once answered, or 😢 (Telegram offers bots no warning sign) |
| `slack` | `:eyes:`, then `:warning:` if it failed |
| `discord` | 👀, then ⚠️ if it failed |
| `mattermost` | `eyes`, then `warning` if it failed |
| `matrix`, `signal`, `whatsapp` | a read receipt, as a person's app sends one, then ⚠️ if it failed |
| the others | none |

One messenger account can serve many profiles: `--route <chat>=<profile>`
(as often as needed) has that chat answered by another profile than the
bridge's, each in sessions of its own.

## When a reply does not go through

A reply that fails in a way that may pass (the service down, overloaded
or slow: a server error, too many requests, a timeout) is tried again,
half a second and then a second later. One the service refuses (a client
error: a chat that is gone, a bot that is not let in) is not tried again,
and its message is marked failed. After five failures in a row the
bridge sends the messenger nothing for a minute, then tries once: if that
fails, nothing for two minutes, then four, at most ten; a reply that goes
through starts the count again. A reply not sent after its three tries,
or while the bridge waits, is kept in an outbox, in the channel state
beside the bridge's cursor: tried again every five seconds and when the
bridge starts again, each chat's replies in the order they were written,
the last 200 kept. A messenger that fails to receive is asked again after
2, 4, 8 seconds and so on, a minute at most; only a failure the first
time, such as a wrong token's, stops the bridge.

## Through an agent gateway

The channels above answer on the machine that runs the profile. An agent
gateway (`sion agent gateway`, not the node's `sion gateway`) lets people
reach profiles served from other machines, without opening those
machines to the internet: it takes the messages, and each machine that
serves profiles connects out to it with `sion agent host`. The gateway
checks the profiles' tokens, so they are made on its machine (here with
a `SION_HOME` of its own). There, `--agent` names each profile's agent:
the gateway routes a profile only to a host that proves that agent's
key, so no other agent can take the profile's name:

```text
$ sion agent profile token add lead --agent 0d7240c5…05aa
token 878455fa for profile lead, agent 0d7240c5 (messages only); it is shown once, and kept here only as its hash:
sion_ch_6822a022…
$ sion agent gateway --name gateway-1 --listen 127.0.0.1:8795
gateway gateway-1 on http://127.0.0.1:8795: hosts connect to /v1/hosts, people write to /v1/profiles/<name>/messages
```

On the agent's machine, the host proves for each profile that it holds the
agent's key. It asks the gateway for a challenge and signs it, with the
gateway's name, the agent and the profile, under the account's policy on
the ledger, so the login cannot be replayed at another gateway. Then it
asks the gateway for messages, each request waiting up to 50 seconds, and
never accepts a connection itself. With `--messages 1` it stops after one:

```text
$ sion agent host http://127.0.0.1:8795 --profile lead --messages 1
serving lead at gateway gateway-1
lead (gateway:lead:ops): How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session 03f902cd42c4bc7c: done
```

The message it answered was written to the gateway with the token made
there (`$TOKEN`):

```text
$ curl -s -X POST http://127.0.0.1:8795/v1/profiles/lead/messages -d '{"text": "How much budget is left?"}'
{"error":"a bearer token is needed"} (HTTP 401)
$ curl -s -X POST http://127.0.0.1:8795/v1/profiles/lead/messages -H "Authorization: Bearer $TOKEN" -d '{"text": "How much budget is left?", "conversation": "ops"}'
{"conversation":"ops","lines":["  → ledger_balance {}","  ← {\"budget_remaining_usd\":\"100\",…}","agent: 100 USD left in the budget.","session 03f902cd42c4bc7c: done"],"said":"100 USD left in the budget.","session":"03f902cd42c4bc7c"} (HTTP 200)
```

The gateway passes a message only while the ledger has the agent in good
standing, as of at most five seconds before. Once alice suspended lead,
its messages stopped at the gateway, which answered them itself:

```text
$ sion agent suspend alice 0d7240c5…05aa
alice signs: Suspend agent 0d7240c5 and every agent below it. Fee 0.0011 USD; valid until Pulse 38.
Pulse 20: Done: agent 0d7240c5 suspended by alice, with everything below it; fee 0.0011 USD.
$ curl -s -X POST http://127.0.0.1:8795/v1/profiles/lead/messages -H "Authorization: Bearer $TOKEN" -d '{"text": "How much budget is left?", "conversation": "ops"}'
{"conversation":"ops","lines":[],"said":"Agent 0d7240c5 is suspended on the ledger: the gateway passes it no messages.","session":null} (HTTP 200)
$ sion agent resume alice 0d7240c5…05aa
alice signs: Lift the suspension of agent 0d7240c5. Fee 0.0011 USD; valid until Pulse 41.
Pulse 24: Done: agent 0d7240c5 resumed; fee 0.0011 USD.
```

A profile goes to the host that proved the agent's key most recently. A
gateway keeps its routes in memory; when it restarts, its hosts log in
again.

With `"stream": true`, a message through the gateway is answered as the
HTTP channel answers it: `session`, `tool` and `delta` events while the
agent works, then `done` with the answer. The host tells the gateway what
the agent did every quarter of a second (`POST /v1/hosts/progress`), and
a host at work is waited for as long as it keeps telling.

## What this does not do yet

- `/stop` stops an agent before its next model or tool call and cuts off
  a model that is writing, but not a tool that is running: that tool's
  call runs to its end. Steering reaches the agent before its next step,
  never in the middle of one. Marks, retries and the outbox have been run
  against a messenger kept in memory and stand-ins, not the platforms
  themselves. Per-chat instructions and tools are the chat's approvers'
  to set: anyone who may approve there may change what the agent is asked
  in that chat, within what its profile gives it.
- The OpenAI-compatible endpoint takes text and the pictures of the last
  message sent inline: a picture sent as an address, and other files, are
  left out, the files the agent shares are not given back, the app's own
  system prompt reaches a new session only as what was said before, and
  no token counts are given. Through an agent gateway, messages carry
  text only.
- Files have been run only against stand-ins for each messenger's API.
  Only pictures are shown to the model; a video, a PDF or a spreadsheet
  reaches the workspace for the agent's tools to open, not the model.
  Telegram gives bots files of 20 MB at most.
  A shared file is sent once, after the reply, and not kept in the outbox
  if it does not go through. An email's files are held in memory until
  they are answered (the last 64, 64 MB at most), so a restart loses
  those not yet answered. Teams, Google Chat, Feishu, DingTalk, Nextcloud
  Talk, IRC and Twitch carry no files here; LINE and SMS take files in
  and send none.
- What a chat is shown while the agent writes is the model's text before
  the run ends. An answer that fails partway (the provider overloaded,
  the next model asked) may show until the next answer replaces it; where
  the reply can only grow (Slack's stream, the HTTP channel's events,
  OpenAI's chunks), the reply then follows it, a paragraph apart, and
  Slack's message is set to the reply once the stream stops. Teams'
  streaming takes only text that grows, so a reply that changed shows at
  its end. Through an agent gateway, the host tells what the agent did a
  quarter of a second at a time, so a stream from it comes in steps.
  Typing and drafts have been run only against stand-ins for each
  platform's API.
- On IRC there is no typing: IRCv3's `+typing` needs the `message-tags`
  capability, which the bridge does not ask for. Telegram's drafts are
  not offered to stop (`can_stop`): a person cannot cut a run short from
  the draft.
- Whoever holds the machine's channel token can start and continue the
  sessions of every profile on the machine, approvals included; whoever
  holds a profile's token, that profile's, approvals only with a token
  made for them; always within each profile's autonomy and the agent's
  charter. The HTTP channel and the agent gateway speak plain HTTP: keep
  them on a local address, or put them behind TLS
  (`agent:channel-token`).
- A bridge trusts its messenger for who writes in a chat it answers. In a
  group chat, everyone who can write there speaks for you, approvals
  included unless `--approvers` names who may decide
  (`agent:telegram-chats`, `agent:messenger-identity`). An approved
  pairing code lets in whoever gave it to you (`agent:pairing`).
- A spoken reply reaches Discord, Slack and Signal as audio attached to
  the text, not as those apps' own voice messages; on LINE and SMS the
  reply is text.
- On IRC, who wrote is the nick the server gives: on a network that does
  not hold nicks to accounts, anyone can take a nick that is free,
  approvals included where `--approvers` names nicks (`agent:irc-nicks`).
  IRC and Twitch carry no voice notes. Mattermost, IRC and Twitch have
  been run only against stand-ins.
- A webhook is taken as its service's when it carries the service's
  signature with the secret you gave (`agent:webhooks`). The signatures
  name no time: a webhook copied on its way could be sent again later,
  and is answered again once its id has left the last thousand
  remembered. The bridge listens on plain HTTP; TLS is your proxy's.
  WhatsApp, LINE and Twilio have been run only against stand-ins. The services
  `--hear` and `--speak` name hear the voice notes and the replies
  (`agent:voice`). Pairing needs a messenger that shows direct
  messages from people it does not know: not Slack without Socket Mode.
  Discord's gateway and Slack's Socket Mode do not answer later what was
  said while the bridge was not listening, and one Discord bot runs one
  gateway connection (no sharding, which Discord asks of bots in 2,500
  servers or more). Slack, Discord, Matrix and Signal have been run only
  against stand-ins for their APIs (the WebSocket client also against
  Discord's real gateway, which it reached and was greeted by), and voice
  notes only against stand-ins for the speech services'.
- Teams and Google Chat are taken as their services' by their tokens
  (`agent:teams-tokens`, `agent:google-chat-tokens`): a webhook copied
  on its way could be sent again while its token lasts (about an hour),
  and is answered again once its message's id has left the last thousand
  remembered.
  Teams carries no voice notes here (its audio comes as files), and
  Google Chat's spoken answers go as text; a Google Chat answer the agent
  takes more than 25 seconds over goes only with a service account's key.
  Both have been run only against stand-ins for Microsoft's and Google's
  side.
- DingTalk's, Feishu's and Talk's messages are taken as their services'
  (`agent:dingtalk-stream`, `agent:feishu-token`,
  `agent:talk-signatures`): Feishu's verification token is a secret in
  every event, not a signature, unless the subscription has an Encrypt
  Key, which encrypts and signs each event; Talk's signature names no
  time. Spoken answers go as text on all three. They have been run only
  against stand-ins.
- WeCom, QQ, WeChat, Zalo, X, Synology Chat, SimpleX, Nostr, ntfy and
  phone calls have been run only against stand-ins for their services
  (`agent:wecom-callbacks`, `agent:qq-signatures`,
  `agent:synology-chat-token`, `agent:simplex-client`,
  `agent:nostr-relays`, `agent:ntfy-topics`, `agent:phone-callers`).
  Nostr's cryptography is checked against NIP-44's and BIP-340's test
  vectors and NIP-19's examples.
  WeChat's iLink protocol is published only as Tencent's plugin's code
  and others' descriptions of it: the bridge speaks what those
  describe, takes text and WeChat's own transcriptions of voice
  messages, and names the pictures and files it does not fetch (iLink
  keeps them encrypted on its CDN). A WeChat bot only replies, so a job
  cannot write to a chat first. QQ takes a bot's unprompted messages
  only a few times a month: a reply later than a few minutes after its
  message may be refused. Zalo, LINE, SMS, X, Synology Chat and SimpleX
  take no files from the bridge, and SimpleX's pictures, voice messages
  and files are named to the agent, not fetched, as are the files of
  Nostr's private messages. X is answered in direct messages only, not
  in posts or mentions. Nostr's group messages (NIP-17 with more than
  two people) are not answered. On a phone call, what was said is what Twilio heard, and the
  reply is Twilio's voice; a reply longer than a few paragraphs is said
  whole. A call that hangs up loses the reply it was waiting for.
- An A2A task is answered when the agent is done (streamed as it writes,
  or pushed to a webhook as its state changes); resubscribing streams its
  state, not the reply's pieces; a push that fails is not tried again; and
  tasks, with their webhooks, are remembered only while the server runs.
  Files in A2A messages are not taken: text only.
- `sion agent talk` has no `/stop` while the agent works (the terminal
  is waiting on it): Ctrl-C ends the conversation, the session kept.
  Voice mode needs programs that record and play audio on this machine.
- Discord's voice channels are not joined: since March 2026 Discord
  admits only clients that speak DAVE, its end-to-end encryption (MLS
  and Opus frames encrypted per sender), which the bridge does not.
- On iMessage, who wrote is the handle Messages shows; an SMS's number
  can be forged (`agent:imessage-handles`). Groups carry no mentions
  here, so `--require-mention` answers nothing in them; a voice note is
  heard only where the bridge runs on the Mac (its file is the Mac's).
  imsg has been run only as a stand-in speaking its JSON-RPC.
- An email is answered as from whom the provider's
  `Authentication-Results` say (`agent:email-from`): a provider that adds
  none, or does not check what it says it checked, leaves the bridge
  nothing to go on. Domains are taken as aligned where one is the other
  or under it, not by the public suffix list. Mailboxes are named in
  ASCII; text is read in UTF-8, Latin-1, Windows-1252 and Latin-9 (other
  character sets as UTF-8, where they agree), and HTML for its text. A
  reply goes in the thread of the last message its writer sent. The
  bridge has been run only against stand-ins for IMAP and SMTP servers.
- An agent gateway sees, and could change, the messages and answers it
  passes. It cannot act as the agent, holding no key, and checks the
  agent's standing as of up to five seconds before (`agent:gateway`).
  Messages waiting for an answer are lost if it restarts.
