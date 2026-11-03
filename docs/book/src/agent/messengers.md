# Messengers

The messaging services an agent answers on, each with what it takes to
connect and how the bridge knows who writes. The commands, approvals,
files and replies of [Channels](channels.md) hold on all of them; what
they do not do yet is listed there.

Outputs on this page are from real runs on a local devnet, long
identifiers and some JSON shortened with `…`. The models are scripted, and
the messengers run against stand-ins for their APIs, because the
environment these pages were made in reaches none of them. The runs of
subagents and hiring predate specification v1.13 (run again on v1.13, they
printed the same lines with other identifiers); a profile's own tokens,
Slack and the agent gateway are from a run on v1.15.

## Telegram

`sion agent telegram` answers a profile's chats by long polling, with the
bot token in `TELEGRAM_BOT_TOKEN`. It answers only the chats in `--allow`,
and with `--approvers`, only the Telegram users named may approve or
decline. Here `TELEGRAM_API_BASE` points it at the stand-in, which
returned two messages, from chat 42 and from chat 99:

```text
$ TELEGRAM_BOT_TOKEN=… TELEGRAM_API_BASE=http://127.0.0.1:8792 sion agent telegram lead --allow 42 --once
chat 42: How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"85",…}
  agent: 85 USD left in the budget.
  session 9fdce72752d0f32a: done
to chat 42: 85 USD left in the budget.
chat 99: not allowed, ignored
```

The stand-in received one message, for chat 42:

```text
sendMessage {"chat_id": 42, "text": "85 USD left in the budget."}
```

## Slack, Discord, Matrix and Signal

`sion agent bridge <profile> --via <messenger>` answers a messenger's
chats with a profile, through the messenger's own API, with credentials
from the environment:

| `--via` | Receives by | Credentials | `--allow` names |
|---|---|---|---|
| `slack` | Socket Mode, given an app-level token; otherwise polling `conversations.history` | `SLACK_BOT_TOKEN`, and `SLACK_APP_TOKEN` for Socket Mode | channel ids |
| `discord` | its gateway | `DISCORD_BOT_TOKEN` | channel ids |
| `matrix` | long polling `/sync` | `MATRIX_HOMESERVER`, `MATRIX_ACCESS_TOKEN` | room ids |
| `signal` | polling the `receive` endpoint of [signal-cli-rest-api](https://github.com/bbernhard/signal-cli-rest-api) | `SIGNAL_API_BASE`, `SIGNAL_NUMBER` | phone numbers |
| `telegram` | long polling `getUpdates` | `TELEGRAM_BOT_TOKEN` | chat ids |
| `mattermost` | its WebSocket | `MATTERMOST_URL`, `MATTERMOST_TOKEN` | channel ids |
| `irc` | a connection of its own (`ircs://`, or `irc://`), with a SASL login if `IRC_PASSWORD` is set | `IRC_SERVER`, `IRC_NICK`, `IRC_PASSWORD` | `#channels` (joined), and nicks |
| `twitch` | Twitch's chat, over IRC | `TWITCH_NICK`, `TWITCH_TOKEN` | `#channels` (joined) |
| `whatsapp`, `line`, `sms` | signed webhooks to `--listen` ([below](#whatsapp-line-and-sms)) | | numbers, user and group ids |

`sion agent telegram` is the same bridge. It does not answer bots:
Telegram does not show a bot other bots' messages, Slack and Discord mark
them, and on Matrix the bridge skips its own. On Signal it answers direct
messages only. It keeps its place, so a restarted bridge goes on where
it stopped, and the first time it polls Slack or Matrix, it starts from
the latest message rather than answering the history; Discord's gateway
and Slack's Socket Mode pass on only what is said while the bridge
listens. With `--approvers`, only the people named, as the messenger
names them, may approve or decline. With `--require-mention`, the group
chats it answers are answered only where a message mentions the bot or
answers one of its messages; direct messages are always answered. A
chat it does not answer is said to be ignored once. On IRC and Twitch, a
reply goes out as lines of about 400 bytes, eight at most, no faster
than the networks allow (Twitch's, one every 1.6 seconds); someone who
writes `lead: …` or names the bot mentions it.

Here `SLACK_API_BASE` points the bridge at a stand-in for Slack's Web API.
Its first poll found one old message, which the bridge took as where to
start; the second found two new messages and a bot's:

```text
$ SLACK_BOT_TOKEN=… SLACK_API_BASE=http://127.0.0.1:8796 sion agent bridge lead --via slack --allow C042 --approvers U07 --once
$ SLACK_BOT_TOKEN=… SLACK_API_BASE=http://127.0.0.1:8796 sion agent bridge lead --via slack --allow C042 --approvers U07 --once
channel C042 (U07): How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session 3c0aa9d0beb2afb0: done
to channel C042: 100 USD left in the budget.
channel C042 (U19): /approve
to channel C042: Only U07 may approve or decline here.
```

The stand-in received:

```text
chat.postMessage {"channel": "C042", "text": "100 USD left in the budget."}
chat.postMessage {"channel": "C042", "text": "Only U07 may approve or decline here."}
```

## Discord's gateway and Slack's Socket Mode

Discord passes a bot what is said over its gateway, a WebSocket the
bridge keeps open: it sends heartbeats, resumes after a drop, and
connects again after a pause that doubles up to a minute. The bot needs
Discord's message-content intent, turned on in its developer portal; if
Discord refuses the token or the intent, the bridge stops and says why.
Slack does the same through Socket Mode once the app has an app-level
token (`SLACK_APP_TOKEN`, `xapp-…`, with `connections:write`): the bridge
acknowledges each event as it comes, and a message Slack sends twice, as
a message and as a mention, is answered once. Both pass on direct
messages as well as channels', so both pair. When the bridge stops, it
closes the connection, and Discord shows the bot offline.

Here `DISCORD_API_BASE` and `SLACK_API_BASE` point the bridge at
stand-ins, whose gateway and Socket Mode are WebSockets on this machine.
Discord's sent four messages: one in channel 900 that mentions the bot,
another there that does not, one in channel 901, and a direct message
(channel 1300) from someone the bridge does not know. Slack's sent one
message in C042, as a message and as a mention, and a direct message
(D088):

```text
$ DISCORD_BOT_TOKEN=… DISCORD_API_BASE=http://127.0.0.1:8796/discord sion agent bridge lead --via discord --allow 900 --pairing --require-mention --once
channel 900 (2201): How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session 6a0b4d998c8b3a40: done
to channel 900: 100 USD left in the budget.
channel 901: not allowed, ignored
channel 1300: not known yet; sent a pairing code (sion agent pairing list)
$ SLACK_BOT_TOKEN=… SLACK_APP_TOKEN=… SLACK_API_BASE=http://127.0.0.1:8796/slack sion agent bridge lead --via slack --allow C042 --pairing --once
channel C042 (U07): How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session 573b1fd79d14ee81: done
to channel C042: 100 USD left in the budget.
channel D088: not known yet; sent a pairing code (sion agent pairing list)
```

The stand-ins received:

```text
gateway: identify op=2 intents=37377 token=…
POST /channels/900/messages {"content":"100 USD left in the budget."}
POST /channels/1300/messages {"content":"I do not know you yet. To talk with me, ask the person who runs me to let you in with: sion agent pairing approve VDWH-KNS9"}
gateway: closed by the bridge
socket mode: {"envelope_id": "e1"}
socket mode: {"envelope_id": "e2"}
socket mode: {"envelope_id": "e3"}
chat.postMessage {"channel":"C042","text":"100 USD left in the budget."}
chat.postMessage {"channel":"D088","text":"I do not know you yet. To talk with me, ask the person who runs me to let you in with: sion agent pairing approve QNB5-KNPN"}
socket mode: closed by the bridge
```

The intents, 37377, are servers, their messages, direct messages and
message content.

## IRC and Twitch

`--via irc` keeps a connection of its own to `IRC_SERVER` (`ircs://` for
TLS), logs in with SASL when `IRC_PASSWORD` is set, joins the `#channels`
in `--allow`, and answers the server's pings; a nick in `--allow` is a
private conversation. `--via twitch` is the same on Twitch's chat, with
the bot's OAuth token. Here `IRC_SERVER` points the bridge at a stand-in,
where, once the bot had joined, ann wrote to it and bob to the channel:

```text
$ IRC_SERVER=irc://127.0.0.1:8793 IRC_NICK=lead sion agent bridge lead --via irc --allow "#ops" --require-mention --once
channel #ops (ann): How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session ccc37f29472e2dd6: done
to channel #ops: 100 USD left in the budget.
```

The server received:

```text
NICK lead
USER lead 0 * :sion
JOIN #ops
PONG :irc.example.org
PRIVMSG #ops :100 USD left in the budget.
QUIT :stopped
```

`lead: How much budget is left?` mentions the bot; `lunch at noon?` does
not, and with `--require-mention` it is not answered.

## WhatsApp, LINE and SMS

WhatsApp (Meta's Cloud API), LINE (its Messaging API) and SMS (through
Twilio) send what is said to the bridge as webhooks. The bridge listens
on an address of its own (`--listen`, `127.0.0.1:8797` by default), which
you expose to the service behind TLS, through a reverse proxy or a
tunnel. Each webhook is checked against the service's signature with the
secret you give, and one that is unsigned or signed with another secret
is refused before anything reaches the profile. A service sends again
what it thinks was not received; each message is answered once.

| `--via` | Credentials | The signature checked | Answers through |
|---|---|---|---|
| `whatsapp` | `WHATSAPP_TOKEN`, `WHATSAPP_PHONE_NUMBER_ID`, `WHATSAPP_APP_SECRET`, `WHATSAPP_VERIFY_TOKEN` | `X-Hub-Signature-256`, with the app secret; Meta's check of the address is answered given the verify token | the Cloud API's `/messages` |
| `line` | `LINE_CHANNEL_ACCESS_TOKEN`, `LINE_CHANNEL_SECRET` | `X-Line-Signature`, with the channel secret | a reply, while the message's reply token lasts (50 seconds here), and a push after |
| `sms` | `TWILIO_ACCOUNT_SID`, `TWILIO_AUTH_TOKEN`, `TWILIO_NUMBER`, `TWILIO_WEBHOOK_URL` | `X-Twilio-Signature`, with the auth token, over the public address Twilio is given (`TWILIO_WEBHOOK_URL`, exactly as Twilio has it) | Twilio's `Messages` API |

Voice notes are heard on all three (WhatsApp's audio messages, LINE's,
and audio sent by MMS) and spoken as an audio message on WhatsApp; LINE
and MMS play audio only from a public address of your own, so there the
reply is text.

Here each bridge ran with `--once` while a stand-in sent it what the
service would, and another stood in for the service's API
(`WHATSAPP_API_BASE`, `LINE_API_BASE` and `TWILIO_API_BASE` pointed at
it):

```text
$ WHATSAPP_TOKEN=… WHATSAPP_PHONE_NUMBER_ID=106 WHATSAPP_APP_SECRET=… WHATSAPP_VERIFY_TOKEN=… WHATSAPP_API_BASE=http://127.0.0.1:8798/meta sion agent bridge lead --via whatsapp --allow 15550001111 --listen 127.0.0.1:8797 --once
whatsapp: webhooks on http://127.0.0.1:8797/
number 15550001111: How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session 010e73028a82599f: done
to number 15550001111: 100 USD left in the budget.
```

What the stand-in for WhatsApp sent the bridge, and the answers:

```text
GET ?hub.mode=subscribe&hub.verify_token=…&hub.challenge=4242: HTTP 200 4242
POST (unsigned): HTTP 401 not signed with the app secret
POST (X-Hub-Signature-256: sha256=…): HTTP 200 ok
```

LINE and SMS, the same way:

```text
$ LINE_CHANNEL_ACCESS_TOKEN=… LINE_CHANNEL_SECRET=… LINE_API_BASE=http://127.0.0.1:8798/line sion agent bridge lead --via line --allow U4af4980629 --listen 127.0.0.1:8796 --once
line: webhooks on http://127.0.0.1:8796/
chat U4af4980629: How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session 810e6fb01af77fad: done
to chat U4af4980629: 100 USD left in the budget.
$ TWILIO_ACCOUNT_SID=AC0001 TWILIO_AUTH_TOKEN=… TWILIO_NUMBER=+15559990000 TWILIO_WEBHOOK_URL=https://bot.example.org/sms TWILIO_API_BASE=http://127.0.0.1:8798/twilio sion agent bridge lead --via sms --allow +15550001111 --listen 127.0.0.1:8795 --once
sms: webhooks on http://127.0.0.1:8795/
number +15550001111: How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session 01959b3c06d8963d: done
to number +15550001111: 100 USD left in the budget.
```

```text
POST (X-Line-Signature: another secret's): HTTP 401 not signed with the channel secret
POST (X-Line-Signature: …): HTTP 200 ok
POST (unsigned): HTTP 401 not signed with the auth token
POST (X-Twilio-Signature: …): HTTP 200 <?xml version="1.0" encoding="UTF-8"?><Response></Response>
```

The stand-in for the three APIs received:

```text
POST /meta/106/messages {"messaging_product":"whatsapp","text":{"body":"100 USD left in the budget."},"to":"15550001111","type":"text"}
POST /line/v2/bot/message/reply {"messages":[{"text":"100 USD left in the budget.","type":"text"}],"replyToken":"r-0042"}
POST /twilio/2010-04-01/Accounts/AC0001/Messages.json To=%2B15550001111&From=%2B15559990000&Body=100%20USD%20left%20in%20the%20budget.
```

## Microsoft Teams and Google Chat

Teams and Google Chat send what is said to the bridge as webhooks too,
to its `--listen` address, which you expose to them behind TLS. Neither
signs a webhook with a secret you share: each sends a token (a JWT,
signed with RS256), which the bridge checks against the keys the service
publishes, fetched over TLS and kept a day, before anything reaches the
profile.

| `--via` | Credentials | The token taken | Answers through |
|---|---|---|---|
| `teams` | `TEAMS_APP_ID`, `TEAMS_APP_PASSWORD`, and `TEAMS_TENANT_ID` for a single-tenant bot | the Bot Connector's (`https://api.botframework.com`), for the app ID, by a key endorsed for the channel, naming the activity's service URL | the Connector API at that service URL, with a token Microsoft Entra ID gives the app, as a reply to the message |
| `google-chat` | `GOOGLE_CHAT_AUDIENCE` (the project number, or the endpoint's URL), and `GOOGLE_CHAT_CREDENTIALS` (a service account's key file) to answer at any time | Chat's own (`chat@system.gserviceaccount.com`) for the project number, or Google's ID token of `GOOGLE_CHAT_SENDER` for the URL | the webhook's own answer, while Chat waits (thirty seconds); with a service account's key, the Chat API, in the message's thread |

In a Teams channel the bot is written to by mentioning it, and the
mention is cut from what the agent reads; a personal chat is a direct
one, for pairing. Google Chat's events come in either of the shapes Chat
sends: an interaction event, or a Workspace add-on's. Without a service
account, an answer the agent takes longer over than Chat waits is not
delivered, and the bridge says so; with one, it goes whenever it comes,
and voice notes are fetched to be heard.

Here each bridge ran with `--once` while a stand-in sent it what the
service would, its tokens signed with a key the stand-in for the service
published (`TEAMS_OPENID_URL`, `TEAMS_LOGIN_URL` and
`GOOGLE_CHAT_KEYS_URL` pointed the bridge at it):

```text
$ TEAMS_APP_ID=app-1 TEAMS_APP_PASSWORD=… TEAMS_TENANT_ID=tenant-1 TEAMS_OPENID_URL=http://127.0.0.1:8790/openid TEAMS_LOGIN_URL=http://127.0.0.1:8790 sion agent bridge lead --via teams --allow a:1chat --listen 127.0.0.1:8789 --once
teams: webhooks on http://127.0.0.1:8789/
conversation a:1chat (aad-alice): How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session c92b696768ee4fb1: done
to conversation a:1chat: 100 USD left in the budget.
$ GOOGLE_CHAT_AUDIENCE=1234567890 GOOGLE_CHAT_KEYS_URL=http://127.0.0.1:8790/jwk/chat sion agent bridge lead --via google-chat --allow spaces/DM1 --listen 127.0.0.1:8788 --once
google-chat: webhooks on http://127.0.0.1:8788/
space spaces/DM1 (users/111): How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session 4d0f4925547a856c: done
to space spaces/DM1: 100 USD left in the budget.
```

What the stand-ins for Teams and Chat sent the bridge, and the answers:

```text
POST (no token): HTTP 401 no token
POST (a token for another bot): HTTP 401 a token for another audience ("app-2")
POST (the Bot Connector's token): HTTP 200 ok
POST (a token for another project): HTTP 401 a token for another audience ("999")
POST (Chat's token): HTTP 200 {"text":"100 USD left in the budget."}
```

What the stand-ins for Microsoft and Google were asked, the secrets
named and not shown:

```text
GET /openid
GET /keys
POST /tenant-1/oauth2/v2.0/token (client_id, client_secret, grant_type, scope)
POST /amer/v3/conversations/a%3A1chat/activities/act-1 (Authorization: Bearer …) {"replyToId":"act-1","text":"100 USD left in the budget.","textFormat":"markdown","type":"message"}
GET /jwk/chat
```

## DingTalk, Feishu and Nextcloud Talk

| `--via` | Credentials | Receives by | Answers through |
|---|---|---|---|
| `dingtalk` | `DINGTALK_CLIENT_ID`, `DINGTALK_CLIENT_SECRET` (an internal app's robot) | Stream mode: a WebSocket the bridge opens to DingTalk, every frame acknowledged, so there is no address to expose | the message's session webhook, or after it expires the robot's API with the app's token |
| `feishu`, `lark` | `FEISHU_APP_ID`, `FEISHU_APP_SECRET`, `FEISHU_VERIFICATION_TOKEN`; `FEISHU_ENCRYPT_KEY` if the subscription has an Encrypt Key | webhooks to `--listen` carrying the subscription's verification token, and with an Encrypt Key encrypted and signed | a reply to the message through the Open Platform's API, with the tenant token |
| `nextcloud-talk` | `NEXTCLOUD_URL`, `NEXTCLOUD_TALK_SECRET` (a bot installed with `occ talk:bot:install`) | webhooks to `--listen`, signed with the bot's secret by the server named | Talk's bot API, signed the same way, as a reply to the message |

DingTalk transcribes voice messages itself, and the agent is given what
it heard, marked as such. Feishu's voice messages are fetched to be heard
(`--hear`), and its pictures and files to be shown or kept. With an
Encrypt Key, Feishu encrypts each event (AES-256) and signs it: an event
not signed with the key is refused. Talk's webhooks do not say whether a
conversation is one person's,
so there is no pairing on Talk; name the conversations (`--allow`, by
their tokens).

Here each bridge ran with `--once` against stand-ins for the three
(`DINGTALK_API_BASE`, `FEISHU_API_BASE` and `NEXTCLOUD_URL` pointed at
them):

```text
$ DINGTALK_CLIENT_ID=ding-app DINGTALK_CLIENT_SECRET=… DINGTALK_API_BASE=http://127.0.0.1:8786 sion agent bridge lead --via dingtalk --allow cid-dm --once
conversation cid-dm (staff7): How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session 5f964ff5927c0f96: done
to conversation cid-dm: 100 USD left in the budget.
$ FEISHU_APP_ID=cli_a1 FEISHU_APP_SECRET=… FEISHU_VERIFICATION_TOKEN=… FEISHU_API_BASE=http://127.0.0.1:8786 sion agent bridge lead --via feishu --allow oc_dm --listen 127.0.0.1:8785 --once
feishu: webhooks on http://127.0.0.1:8785/
chat oc_dm (ou_alice): How much budget is left?
  (the reply is not shown as it is written: Feishu made no card)
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session 6eccad7c90a2c784: done
to chat oc_dm: 100 USD left in the budget.
$ NEXTCLOUD_URL=http://127.0.0.1:8786 NEXTCLOUD_TALK_SECRET=… sion agent bridge lead --via nextcloud-talk --allow n3xtc10ud --listen 127.0.0.1:8784 --once
nextcloud-talk: webhooks on http://127.0.0.1:8784/
conversation n3xtc10ud (users/alice): How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session 6e03b59d2b58e874: done
to conversation n3xtc10ud: 100 USD left in the budget.
```

What the stand-in for DingTalk was told, a ping and a message
acknowledged over its WebSocket:

```text
POST /v1.0/gateway/connections/open (clientId, clientSecret, localIp, subscriptions, ua)
WebSocket /connect?ticket=ticket-1
acknowledged: {"code":200,"data":"{\"opaque\":\"x\"}","headers":{"contentType":"application/json","messageId":"p1"},"message":"OK"}
acknowledged: {"code":200,"data":"{\"response\":\"OK\"}","headers":{"contentType":"application/json","messageId":"c1"},"message":"OK"}
POST /robot/sendBySession?session=s1 {"msgtype":"text","text":{"content":"100 USD left in the budget."}}
WebSocket closed by the bridge
```

What the stand-ins for Feishu and Talk sent the bridge, and the answers:

```text
POST (the address's check): HTTP 200 {"challenge":"c-42"}
POST (another app's token): HTTP 401 not the app's verification token
POST (encrypted): HTTP 400 an encrypted event: set FEISHU_ENCRYPT_KEY to the subscription's Encrypt Key
POST (the app's token): HTTP 200 {}
POST (unsigned): HTTP 401 not signed with the bot's secret
POST (signed, from another server): HTTP 401 from another server
POST (signed with the bot's secret): HTTP 200 ok
```

And what their APIs were asked, the secrets named and not shown:

```text
POST /open-apis/auth/v3/tenant_access_token/internal (app_id, app_secret)
POST /open-apis/im/v1/messages/om_1/reactions (Authorization: Bearer …) {"reaction_type":{"emoji_type":"Typing"}}
POST /open-apis/cardkit/v1/cards (Authorization: Bearer …) {"data":"{\"body\":{\"elements\":[{\"content\":\"… ledger_balance\",\"element_id\":\"reply\",\"tag\":\"markdown\"}]},\"config\":{\"streaming_config\":{\"print_frequency_ms\":{\"default\":30},\"print_step\":{\"default\":2},\"print_strategy\":\"fast\"},\"streaming_mode\":true},\"schema\":\"2.0\"}","type":"card_json"}
POST /open-apis/im/v1/messages/om_1/reply (Authorization: Bearer …) {"content":"{\"text\":\"100 USD left in the budget.\"}","msg_type":"text"}
POST /ocs/v2.php/apps/spreed/api/v1/bot/n3xtc10ud/reaction/1567 (signed: random, signature) {"reaction":"⏳"}
POST /ocs/v2.php/apps/spreed/api/v1/bot/n3xtc10ud/message (signed: random, signature) {"message":"100 USD left in the budget.","referenceId":"f91ac0ee1d62106e2839bf19c83d9238c61a02b5211837fd629ab915d4875f66","replyTo":1567}
```

## Email

`--via email` reads the agent's own mailbox over IMAP and answers over
SMTP. Each person who writes is a chat, named by their address
(`--allow alice@example.org`), and the reply goes to them alone, in the
thread of what they wrote. What the agent reads is what they wrote
themselves: the message they quote and their signature are left out, and
the subject comes first where a conversation starts.

| Variable | What it is |
|---|---|
| `EMAIL_ADDRESS` | the agent's address |
| `EMAIL_IMAP_URL` | `imaps://imap.example.org` (TLS, port 993), or `imap://…` (STARTTLS, port 143) |
| `EMAIL_SMTP_URL` | `smtps://smtp.example.org` (TLS, port 465), or `smtp://…` (STARTTLS, port 587) |
| `EMAIL_PASSWORD` | the password: an app password, where the provider has them |
| `EMAIL_TOKEN_COMMAND` | instead of a password, a command that prints an OAuth access token, for providers that want one (XOAUTH2) |
| `EMAIL_USERNAME`, `EMAIL_MAILBOX`, `EMAIL_NAME` | the login if it is not the address, the mailbox read (`INBOX`), and the name replies are sent under |
| `EMAIL_AUTHSERV_ID` | the name your provider signs its `Authentication-Results` with (`mx.google.com`; a domain stands for its servers) |
| `EMAIL_TRUST_FROM` | `1` to take `From` as it is, on a server that checks nothing and where no stranger can write |

Anyone can put any address in `From`. The bridge believes it only as far
as your provider vouches for it: it reads the `Authentication-Results`
the provider adds to each message it receives (RFC 8601), and answers
only where DMARC passed for the sender's domain, or DKIM or SPF passed
for a domain aligned with it. It reads the topmost results, which the
provider writes above everything a sender wrote, or with
`EMAIL_AUTHSERV_ID`, the topmost the provider signed with that name. A
message the provider did not check is not answered; `EMAIL_TRUST_FROM=1`
takes `From` as it is. Mail written by machines is not answered either:
out-of-office replies, bounces and delivery reports, mailing lists and
bulk mail. The replies say `Auto-Submitted: auto-replied` (RFC 3834), so
that two machines do not answer each other.

Nothing is sent in the clear: an `imap://` or `smtp://` server must offer
STARTTLS, unless it is on a loopback address (a bridge on the machine
itself, as Proton Mail's is), and anything a server sends between
agreeing to TLS and the handshake is refused. The bridge reads with
`BODY.PEEK`, so no message is marked read or changed. It keeps its place
by UID, starts after the newest message the first time, and waits for
mail with IDLE where the server has it. Attachments are files for the
agent, ten of a message at most ([Pictures and files](channels.md#pictures-and-files));
audio ones are voice notes for `--hear`, and an answer spoken with
`--speak` goes back as an audio attachment.

Here a stand-in for a provider offered STARTTLS on both servers, with a
certificate from a CA made for the recording (`SSL_CERT_FILE` names it).
The first run answered nothing, starting after the message already
there. Then three came: Alice's question; one saying it was from Alice
that her domain's DMARC failed, the sender having written a passing
result below the provider's; and her out-of-office reply.

```text
$ EMAIL_ADDRESS=lead@example.net EMAIL_PASSWORD=… EMAIL_IMAP_URL=imap://localhost:8791 EMAIL_SMTP_URL=smtp://localhost:8792 EMAIL_AUTHSERV_ID=mx.example.net SSL_CERT_FILE=ca.pem sion agent bridge lead --via email --allow alice@example.org --once
$ sion agent bridge lead --via email --allow alice@example.org --once
address alice@example.org: DMARC fail for example.org, ignored
address alice@example.org: an automatic message (Auto-Submitted: auto-replied), ignored
address alice@example.org: Subject: Budget

How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session 98398c65086bd867: done
to address alice@example.org: 100 USD left in the budget.
```

What the servers were told on the second run, the logins shown as `…`:

```text
imap: s1 STARTTLS
imap: (TLS from here)
imap: s2 CAPABILITY
imap: s3 AUTHENTICATE PLAIN …
imap: s4 CAPABILITY
imap: s5 SELECT "INBOX"
imap: s6 UID FETCH 2:* (UID RFC822.SIZE)
imap: s7 UID FETCH 2 (BODY.PEEK[])
imap: s8 UID FETCH 3 (BODY.PEEK[])
imap: s9 UID FETCH 4 (BODY.PEEK[])
smtp: EHLO example.net
smtp: STARTTLS
smtp: (TLS from here)
smtp: EHLO example.net
smtp: AUTH PLAIN …
smtp: MAIL FROM:<lead@example.net>
smtp: RCPT TO:<alice@example.org>
smtp: DATA
smtp: (the message)
  From: <lead@example.net>
  To: <alice@example.org>
  Subject: Re: Budget
  Date: Fri, 30 Oct 2026 22:10:13 +0000
  Message-ID: <936c7a877f9df40f0d4c6f0d442381a3@example.net>
  In-Reply-To: <c1f3@mail.example.org>
  References: <c1f3@mail.example.org>
  Auto-Submitted: auto-replied
  MIME-Version: 1.0
  Content-Type: text/plain; charset=utf-8
  Content-Transfer-Encoding: quoted-printable

  100 USD left in the budget.
smtp: QUIT
imap: s10 LOGOUT
```

## iMessage

`--via imessage` answers Messages on a Mac through
[`imsg`](https://github.com/steipete/imsg), the command line OpenClaw
drives too: the bridge runs `imsg rpc` and speaks its JSON-RPC, on the Mac
itself or, with `IMSG_COMMAND="ssh mac imsg rpc"`, from another machine.
imsg reads Messages' database (it needs Full Disk Access) and sends
through Messages (Automation): there is no server to expose and no
password to keep.

The bridge subscribes to the messages after its place, a message's row
id, so what came while it was stopped is answered; the first time, it
starts after the latest message. A chat is named by its identifier: a
handle for a direct chat (`--allow +15551234567`, or an Apple ID's
address), `chat…` for a group. What is sent from the Mac itself is the
agent's own, and not answered. Voice notes (Core Audio files) are heard
as imsg converts them, and other attachments are files for the agent
(as imsg converts them too: HEIC pictures to JPEG), where the bridge runs
on the Mac; spoken answers go as audio messages, with their text.
`IMSG_SERVICE` sends replies as `imessage`, `sms` or `auto`.

Here imsg was a stand-in speaking its JSON-RPC (imsg runs only on
macOS). After the first run, which answered nothing, came a question, the
Mac's own reply to an earlier one, and a group's message:

```text
$ IMSG_COMMAND="python3 -I fake-imsg.py messages.json imsg-asked.jsonl" sion agent bridge lead --via imessage --allow +15550001111 --once
$ sion agent bridge lead --via imessage --allow +15550001111 --once
chat +15550001111: How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session e7382195f8970032: done
to chat +15550001111: 100 USD left in the budget.
chat chat777: not allowed, ignored
```

What imsg was asked over the two runs:

```text
{"method": "chats.list", "params": {"limit": 1}}
{"method": "messages.history", "params": {"chat_id": 1, "limit": 1}}
{"method": "watch.subscribe", "params": {"attachments": true, "since_rowid": 10}}
{"method": "watch.subscribe", "params": {"attachments": true, "since_rowid": 10}}
{"method": "send", "params": {"chat_identifier": "+15550001111", "text": "100 USD left in the budget."}}
```

## WeCom, QQ and Zalo

`--via wecom` answers a WeCom (企业微信) self-built app. WeCom posts each
message to `--listen` encrypted and signed, as its `WXBizMsgCrypt` has it:
AES-256-CBC under the callback's `EncodingAESKey`, and a SHA-1 signature
over the callback's token, the time, a nonce and the ciphertext. A
callback whose signature is not the token's, or whose message does not
decrypt to one for the company, is refused before anything reaches the
profile; the address's check (`echostr`) is answered decrypted, as WeCom
asks. Replies go through the app's API (`message/send`) with an access
token from the corp id and the app's secret, renewed before it lapses;
pictures, voice messages and files come through `media/get`, and what the
agent shares goes up through `media/upload`.

| Variable | What |
|---|---|
| `WECOM_CORP_ID` | the company's id, which every callback names |
| `WECOM_AGENT_ID`, `WECOM_SECRET` | the app's id and secret |
| `WECOM_TOKEN`, `WECOM_AES_KEY` | the callback's token and `EncodingAESKey` |

`--via qq` answers a bot of QQ's bot platform (QQ 机器人) in webhook mode:
QQ signs each event with Ed25519 under a key both sides derive from the
bot's secret, and the bridge answers the address's check (op 13) by
signing its token back, signing nothing that could pass for an event. A
chat is named by its kind: `user:<openid>` for a person's direct
messages, `group:<openid>` for a group that mentions the bot,
`channel:<id>` for a channel of a QQ guild. Replies are QQ's passive
ones, each part numbered (`msg_seq`), while QQ takes them for the message
(a few minutes); later a message is sent unprompted, which QQ allows only
a few times a month (`QQ_APP_ID`, `QQ_APP_SECRET`).

`--via zalo` answers a bot of the Zalo Bot Platform, whose API has
Telegram's shape: `getUpdates` long polled, `sendMessage`,
`sendChatAction` for typing (`ZALO_BOT_TOKEN`). A picture or a voice
message comes as an address on Zalo's servers, fetched to be shown or
heard; the bot sends no files, as Zalo takes a bot's pictures and voice
only from a public address.

## WeChat

`--via weixin` answers a WeChat (微信) account through Tencent's iLink bot
protocol, the one its own ClawBot plugin speaks. `sion agent weixin
login` draws a QR code in the terminal: scan it with WeChat on the phone
and confirm, and the bot's token is kept encrypted beside the secrets
(`sion agent weixin logout` forgets it; `WEIXIN_BOT_TOKEN` gives one
instead). The bridge long polls `getupdates`, and each reply carries the
context token of the message it answers, as iLink asks. Text is taken,
and a voice message as WeChat transcribed it; pictures and files, which
iLink keeps encrypted on its CDN, are named to the agent, not fetched.
The protocol is published only as Tencent's plugin's code and others'
descriptions of it, which is what the bridge speaks.

## X, Synology Chat and SimpleX

`--via x` answers an X account's direct messages, polled through the X
API v2 (`/2/dm_events`) once a minute, as X's smaller tiers allow
(`X_POLL_SECS` says otherwise), and answered in the conversation with
whoever wrote. `X_ACCESS_TOKEN` is a user's OAuth 2.0 token with
`dm.read`, `dm.write`, `users.read` and `tweet.read`; with
`X_REFRESH_TOKEN` and `X_CLIENT_ID` (and `X_CLIENT_SECRET` for a
confidential app) it is renewed when X refuses it. The first poll takes
the bridge's place: what was written before it started is not answered.

### Mentions on X

`sion agent x-mentions <profile>` answers the posts that mention the
account, in reply to each, once a minute (`--poll`). X lets an
account's automated replies go only to posts that mention or quote it, so
a mention is answered once, in its own thread, and no one else is. The
token needs `tweet.read`, `tweet.write` and `users.read` (`dm.read` and
`dm.write` too if the same token serves `--via x`); the tokens a renewal
gives are kept sealed on this machine, since X ends the refresh token
each renewal uses.

An answer rests on what was read for it, not on what the model
remembers. Before the session starts, the runner gathers the sources:
the post the mention replies to or quotes, and, with a search service set
up (`web_search`'s Brave, Tavily or SearXNG), its results for the
mention's own words, the first two pages of them read (`--read`). The
session is given them numbered, as data, and asked to answer from them
in at most 600 characters, citing each claim, like `[1]`. Its answer is
posted only if every number it cites is a source it was given, it names
no address it was not given, and it cites at least one source
(`--sources optional` lets an answer with none through). The reply ends
with the sources it cites and their addresses:

```text
It closes at noon today [1], for roadworks [2].

Sources: [1] https://x.com/cy/status/90 [2] https://city.example/notices
```

What a stranger writes is public input, so the session it starts is
tainted from the start, and it answers with the profile's models,
instructions and skills only: none of its memory, workspace, shell, MCP
servers or web tools, nothing a mention could draw out of it into a
public reply. A profile marked confidential is refused.

A post is at most 280 weighted characters (an address counts 23), and X
bills a post with an address far more than one without, so a longer
answer is split at its sentences into up to `--max-posts` posts (3),
chained, with all the addresses in the last. `--per-day` (50) and
`--per-person` (5) bound how many mentions are answered each UTC day.
Someone who writes `stop` is not answered again until they write `start`;
the account's own posts, retweets and protected accounts' posts are
passed over. `--from @ana,@bo` answers only those people, and
`--dry-run` prints the replies without posting them. The first run
answers nothing: it starts from the newest mention. What it keeps (its
place, who said stop, the day's counts, the replies posted) is in
`agents/x-mentions/<profile>.json`.

A profile that may read the web also gets `x_search`, recent posts
(the last seven days) searched through the same token, X's operators
(`from:`, `lang:`, `-is:retweet`) included, when `X_ACCESS_TOKEN` is set.
X bills each post read; what posts say comes back as from outside.

`--via synology-chat` answers a Synology Chat chatbot: Synology Chat
posts what people write to the bot to `--listen` (the bot's outgoing
URL), each request carrying the bot's token (`SYNOLOGY_CHAT_TOKEN`), and
the reply goes through the bot's incoming URL
(`SYNOLOGY_CHAT_INCOMING_URL`, as Synology Chat shows it), to the user
who wrote, named by the id Synology Chat gives.

`--via simplex` answers a SimpleX Chat profile through the WebSocket API
of SimpleX's terminal client, run on the same machine with the bot's
profile (`simplex-chat -p 5225`; `SIMPLEX_URL` names another address).
The client's API asks no password, so it listens on localhost only, and
the bridge must be its only program. A chat is `@<contact id>` or
`#<group id>`, as the client numbers them; in a group, the bot answers
what mentions it. The profile's address, and accepting whoever asks to
connect, are set up in the client (`/address`, `/auto_accept on`); the
bridge answers only the chats it is let into, or pairs them. A reply
shows as it is written as a SimpleX live message, updated every two
seconds until it is done.

## Nostr

`--via nostr` answers an account's direct messages on Nostr's relays
(`NOSTR_RELAYS`, comma-separated; `wss://relay.damus.io` and
`wss://nos.lol` by default), with its key in `NOSTR_PRIVATE_KEY` (an
`nsec1…` or hex). Both kinds are read: NIP-17's private messages,
encrypted with NIP-44 and sealed and gift-wrapped as NIP-59 has it,
which hide from relays who writes to whom and when; and NIP-04's older
encrypted messages (kind 4), which do not. A reply goes the way its
message came; a chat that has not written yet is written to as
`NOSTR_DM` says (`nip17`, or `nip04`). A chat is the writer's key, as an
`npub1…` (`--allow npub1…`).

Every event is checked against its signature, and a private message
against who sealed it, so a relay can withhold messages but not forge
them. A private reply goes to the relays its reader names for theirs
(kind 10050) as well as to the bridge's own; an older one to the relays
they read (kind 10002). If the account names no relays for its private
messages, the bridge names its own, so that others' apps know where to
write. A relay that asks who reads (NIP-42) is answered with the
account's key. The bridge's place is the time of the last message it
took: on starting again, what was written meanwhile is answered.

## ntfy

`--via ntfy` treats each topic in `--allow` as a chat: polled for what was
published to it since the bridge's place, and answered by publishing to
it, tagged `robot` and `sion` so that the bridge does not take its own
answers for messages (`NTFY_URL`, `https://ntfy.sh` by default;
`NTFY_TOKEN`, or `NTFY_USER` and `NTFY_PASSWORD`, for a server that asks).
ntfy names no writer: whoever may publish to a topic speaks for you there
(`agent:ntfy-topics`), so use a topic of your own on a server that asks
who publishes, or a name nobody can guess.

## Phone calls

`--via phone` answers calls to a Twilio number. Twilio hears what the
caller says (`<Gather input="speech">`) and speaks the agent's reply
(`<Say>`), each step of the call a webhook to `--listen`, signed with the
account's auth token for the public address Twilio was given
(`TWILIO_VOICE_URL`, the number's voice webhook). While the agent works,
the call waits on two-second pauses, the bridge asked again after each;
then the reply is said and the caller heard again. Only the numbers in
`--allow` are answered: other callers are told so, and hung up on.
`PHONE_LANGUAGE` (`en-US` by default) is the language heard and spoken,
`PHONE_VOICE` one of Twilio's voices, `PHONE_GREETING` what a call is
greeted with. A caller is named by the number Twilio gives, which can be
forged (`agent:phone-callers`).
