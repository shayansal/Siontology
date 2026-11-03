# Pairing and voice

Who may talk to an agent on a messenger it answers, and voice: notes heard
and answered aloud, and talking with it in the terminal.

Outputs on this page are from real runs on a local devnet, long
identifiers and some JSON shortened with `…`. The models are scripted, and
the messengers run against stand-ins for their APIs, because the
environment these pages were made in reaches none of them. The runs of
subagents and hiring predate specification v1.13 (run again on v1.13, they
printed the same lines with other identifiers); a profile's own tokens,
Slack and the agent gateway are from a run on v1.15.

## Pairing and voice notes

With `--pairing`, a bridge gives someone who writes to it directly, and
whom it does not know, a code instead of an answer. They give you the
code, and `sion agent pairing approve` lets their chat in, on that
channel and for that profile only. The bridge never prints the code; it
goes only to them, so what you approve is the chat of whoever gave it to
you. A code lasts an hour, a chat is given one every ten minutes at most,
a channel has three waiting at most, and five wrong approvals in a row
stop approvals for an hour. Codes are consonants and digits, so none
spells a word. Pairing is for direct chats: a private chat on Telegram,
a direct message on Discord, or on Slack with Socket Mode, a room of the
account and one other on Matrix (where the bridge joins the rooms it is
invited to), and Signal's messages. A group chat the bridge does not
answer stays ignored. Slack without Socket Mode is polled channel by
channel, so no direct message from someone it does not know reaches the
bridge, and it refuses `--pairing` there.

With `--hear`, voice notes are transcribed and reach the profile as
`(voice note) …`: Telegram's voice notes and audio files, Discord's and
Signal's audio attachments, Slack's audio clips (with the `files:read`
scope) and Matrix's audio messages (fetched as authenticated media).
With `--speak`, a voice note is answered aloud: on Telegram as a voice
note with the reply as its caption, on Matrix as a voice message, and
on Discord, Slack (with `files:write`) and Signal as the reply with the
audio attached:

| Flag | What hears or speaks | Credentials |
|---|---|---|
| `--hear openai:<model>` | OpenAI's transcriptions, or a compatible server's at `OPENAI_BASE_URL` | `OPENAI_API_KEY` |
| `--hear groq:<model>` | Groq's transcriptions | `GROQ_API_KEY` |
| `--hear deepgram:<model>` | Deepgram's (`nova-3`, say) | `DEEPGRAM_API_KEY` |
| `--hear elevenlabs:<model>` | ElevenLabs' Scribe (`scribe_v1`, say) | `ELEVENLABS_API_KEY` |
| `--hear command:<program>` | A program on this machine, given the audio's file, which prints the text | |
| `--speak openai:<model>:<voice>` | OpenAI's speech (or a compatible server's), as Ogg Opus | `OPENAI_API_KEY` |
| `--speak elevenlabs:<voice id>[:<model>]` | ElevenLabs, as MP3 | `ELEVENLABS_API_KEY` |
| `--speak deepgram:<voice model>` | Deepgram's Aura (`aura-2-thalia-en`, say), as MP3 | `DEEPGRAM_API_KEY` |
| `--speak command:<program>` | A program given the text on its standard input, which writes Ogg Opus or MP3 | |

Here `TELEGRAM_API_BASE` and `OPENAI_BASE_URL` point the bridge at
stand-ins for Telegram's and OpenAI's APIs. Telegram's returned a voice
note from chat 42 and a message from chat 77, which the bridge does not
answer:

```text
$ TELEGRAM_BOT_TOKEN=… TELEGRAM_API_BASE=http://127.0.0.1:8792 OPENAI_API_KEY=… OPENAI_BASE_URL=http://127.0.0.1:8792/v1 sion agent telegram lead --allow 42 --pairing --hear openai:gpt-4o-mini-transcribe --speak openai:gpt-4o-mini-tts:alloy --once
chat 42: (voice note) How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session a72dec3af7de6702: done
to chat 42 (voice note): 100 USD left in the budget.
chat 77: not known yet; sent a pairing code (sion agent pairing list)
```

The stand-ins received:

```text
POST /v1/audio/transcriptions model=gpt-4o-mini-transcribe file=voice.ogg (6752 bytes, audio/ogg)
POST /v1/audio/speech {"input":"100 USD left in the budget.","model":"gpt-4o-mini-tts","response_format":"opus","voice":"alloy"}
sendVoice chat_id=42 caption="100 USD left in the budget." voice=voice.ogg (7229 bytes, audio/ogg)
sendMessage {"chat_id":77,"text":"I do not know you yet. To talk with me, ask the person who runs me to let you in with: sion agent pairing approve QBRS-9XRF"}
```

Given the code by the person in chat 77, you let them in:

```text
$ sion agent pairing list
waiting: telegram:lead chat 77 (9), 0 minutes ago: "Hello?"
$ sion agent pairing approve QBRS-9XRF
let in telegram:lead chat 77 (9): it is answered from its next message
$ sion agent pairing list
no codes waiting
let in: telegram:lead chat 77
```

and the bridge answers them from their next message:

```text
$ TELEGRAM_BOT_TOKEN=… TELEGRAM_API_BASE=http://127.0.0.1:8792 OPENAI_API_KEY=… OPENAI_BASE_URL=http://127.0.0.1:8792/v1 sion agent telegram lead --allow 42 --pairing --hear openai:gpt-4o-mini-transcribe --speak openai:gpt-4o-mini-tts:alloy --once
chat 77 (9): Thanks! How much budget is left?
    → ledger_balance {}
    ← {"budget_remaining_usd":"100",…}
  agent: 100 USD left in the budget.
  session ee56d1a1f268678a: done
to chat 77: 100 USD left in the budget.
```

`sion agent pairing revoke telegram:lead 77` lets the chat out again.
Pairings are kept in `agents/pairing.json` under `SION_HOME`, readable
only by your user.

## In the terminal, and by voice

`sion agent talk <profile>` is a conversation in the terminal: what you
type goes to the agent, its reply comes as the model writes it, and each
tool it runs shows on a line of its own. The conversation goes on in its
session, the channel's commands included; `--conversation` names another.
With `--voice`, you are heard: press Enter and speak, recorded until you
stop (`--record`, sox's `rec` by default, stopping after a second and a
half of silence), transcribed with `--hear`; the reply is spoken with
`--speak` and played (`--play`: mpv, ffplay, afplay or paplay, the first
there). Each program is given the audio file's path.

## A live conversation

`sion agent talk <profile> --voice --live` listens all the time, as a call
does. The microphone is read without pause (`--listen`: sox's `rec`,
`arecord`, `parec` or `ffmpeg`, the first there, or any program that
writes raw 16-bit mono samples at 16 kHz to its output), and what you say
is found in it by its loudness: speech begins after 60 ms louder than
three times the noise floor, which it follows as the room changes, and
ends after 800 ms quieter; a fifth of a second before and after is kept,
an utterance runs to 30 seconds at most, and under 160 ms is a noise, not
speech. Nothing quieter than `--speech-level` is speech (400, as the root
mean square of 16-bit samples): lower it if you are not heard, raise it in
a loud room. Each utterance goes to `--hear` as a WAV file.

The reply is spoken a sentence at a time, as the model writes it: the
first sentence is being said while the rest is written. Code is shown,
not read out ("The code is on the screen."), a link is said as its words,
an address as "the link on the screen", and markdown's marks are dropped.

Speak over it, and it stops: speech twice as loud as would begin an
utterance, for a quarter of a second (so that a cough or the room does
not), silences it at once; its run stops before its next step, and what
you said goes to it next. What it was still writing is not shown or
said. Typing `/stop` does the same. There is no echo cancellation, so
this needs headphones: from loudspeakers, its own voice would interrupt
it. With `--no-interrupt`, the microphone is not listened to while it
works or speaks.

What you type is taken too: a message, a command (one typed while the
agent works runs alongside it, as `/status` does), or the answer to a
question an MCP server asks, which is also said aloud and may be answered
aloud. The question waits for an answer, spoken or typed, or for the
input to end.

### A wake word

`--wake "hey sion"` takes only what begins with the phrase (several,
split by `|`). Up to two words before it are let pass ("okay, hey sion"),
and a word of three letters or more may be one letter off, as
transcribers spell a name they hear ("Zion", "Scion"). After it speaks,
it stays awake for a follow-up for `--awake-secs` (15 by default), and
so it does after asking you something; while it works or speaks, it is
interrupted by its name ("hey sion, stop").

To find the phrase, everything said near the microphone is heard. So it
is heard by a transcriber on this machine (`--wake-hear
command:<program>`, such as whisper.cpp's, or `--hear` if that is one),
and only what was said to the agent goes to `--hear`. `--wake-cloud` lets
a transcriber elsewhere hear it all instead.

Two things here are simplified. Speech is told from silence by its
loudness alone, not by a model of speech, so a loud room is taken for
speech, and quiet speech in it is missed. And there is no echo
cancellation, as above. The tests run the whole conversation with a
stand-in microphone (silence and a tone, paced as spoken), a stand-in
transcriber, a scripted model and a player that takes its time, against a
devnet.
