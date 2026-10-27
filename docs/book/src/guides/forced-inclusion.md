# Forced inclusion and the censorship demo

This guide forces a transaction past a censoring gateway, then past
censoring validators, and shows what a light client sees. It is the
`censorship` demo done by hand. The rules are in
[Censorship resistance](../concepts/censorship-resistance.md).

The censorship controls used here (`sion censor`) are an **adversary
simulation**, available only on a devnet.

```text
Usage: sion censor [OPTIONS] [ACCOUNTS]...

Arguments:
  [ACCOUNTS]...  Wallet names or account IDs to censor

Options:
      --gateway                  The gateway drops their transactions silently
      --validators <VALIDATORS>  This many validators ignore them too (more than a third of a committee can stall certification) [default: 0]
```

## A forced send

`sion send --forced` posts the transaction straight to every validator's
proposals instead of the gateway:

```text
$ sion send dana bob 1 --forced
dana signs (forced): Pay 1 USD to bob. Fee 0.0011 USD; valid until Pulse 141.
Pulse 125: Done: paid 1 USD to bob; fee 0.0011 USD.
  digest e9dc2590d429e60f75a2ee019ed872ff6ea844269e54dd857ded8fa943df138b
```

The Pulse that committed it (123) posted it as an obligation of dana's
shard, due by Pulse 131. The shard's validators certified it among
themselves, and it executed at Pulse 125. The obligation stays listed until
its deadline, when the state checks that the intent can no longer execute:

```text
$ sion forced
{
  "pending": [
    {
      "deadline": 131,
      "digest": "e9dc2590d429e60f75a2ee019ed872ff6ea844269e54dd857ded8fa943df138b",
      "missed": 0,
      "posted_at": 123,
      "shard": 1
    }
  ],
  "pulse": 125,
  "rotations": {},
  "summary": {
    "overdue": 0,
    "overdue_root": "0000000000000000000000000000000000000000000000000000000000000000",
    "pending": 1
  },
  "window": 8
}
```

## A censoring gateway

```text
$ sion censor dana --gateway
ADVERSARY SIMULATION: censoring 1 account(s) at the gateway

$ sion send dana bob 2
dana signs: Pay 2 USD to bob. Fee 0.0011 USD; valid until Pulse 145.
error: 48f9605053ef83cc9bc735888b9acde37ac23a2f920758d178b3f70a2fc69873 did not execute in time
```

The gateway acknowledged the transaction and did nothing; the CLI gave up
after 90 seconds. The same payment, forced:

```text
$ sion send dana bob 2 --forced
dana signs (forced): Pay 2 USD to bob. Fee 0.0011 USD; valid until Pulse 181.
Pulse 165: Done: paid 2 USD to bob; fee 0.0011 USD.
  digest 831aca37225e1169c63a67999b484ba72f18bdcd7e6e0e3a0e71d514241631ff
```

A censoring gateway is routed around.

## Censoring validators

Now two of the four validators censor dana as well. A lane certificate
needs three approvals, so dana's shard can no longer certify her
transactions:

```text
$ sion censor dana --gateway --validators 2
ADVERSARY SIMULATION: censoring 1 account(s) at the gateway and at 2 validator(s)
```

A third forced send (run in the background, since it now waits) was posted
at Pulse 168 with a deadline of 176. At Pulse 176 the intent could still
execute, so the state marked it overdue, replaced the shard's committee
and gave the shard another window:

```text
$ sion forced
{
  "pending": [
    {
      "deadline": 184,
      "digest": "d4a76c8d812e3a5abb180a26a163a9f9d24fe4f40e5a14cd145681dc0a2d56c5",
      "missed": 1,
      "posted_at": 168,
      "shard": 1
    }
  ],
  "pulse": 176,
  "rotations": {
    "1": {
      "at": 176,
      "count": 1,
      "members": [
        "04dde303fa8297520d5ab13c037c1aa125e3eb3fef5e36762e0ceb654ca729af",
        "2ed647f178e36cbf1eef5439575a5452e6925df47fe52a05bec7aaa9b105f27b",
        "92e026940f9662e9fe173fb6125c3a0755592003cca37efee242170511219ef3",
        "cdc0cc1da885c87ebdb67a8c05e027449f8cc15e4743959f8c0f3fa469b3ab38"
      ]
    }
  },
  "summary": {
    "overdue": 1,
    "overdue_root": "f47fd578f57d167ab8a80a4f80f5853cab7405e8915ce38efcc61725d70676dc",
    "pending": 0
  },
  "window": 8
}
```

The devnet has four validators for four seats, so the replacement
committee is the same four. With more validators than seats, members are
drawn from outside the failed committee first.

## What a light client sees

Every certified header from Pulse 176 on counts the overdue intent:

```text
$ sion verify --chain 6345655cfd2a714ed1dab69c0660f2f48f429ce2981e25629d0467d7b7ee97b7 --pulse 176 --no-world
chain: 6345655c matches the pinned identity; 4 validators, keys derive their identifiers ✓
Pulse 176 header 8cea20a2 (state root 3c959a1a, knowledge root fce2f174, 3 claims)
certificate: 300000 of 400000 stake signed (quorum 266667), each signature ML-DSA-44 + SLH-DSA-SHA2-128f ✓
forced inclusion: 1 intent(s) OVERDUE, censorship detected and certified by the committee (0 pending)
receipts: 0 of 0 transactions proven against the receipts root, in the certified order ✓
state: proof skipped (node already at Pulse 177)
availability: 16 of 16 random shares of the 8×8 square verified; a withholding would go unnoticed with probability < 7.7e-5 ✓
```

`sion status` reports it too (`"forced_overdue": 1`), and the validators
log it:

```text
validator: Pulse 176: forced intent d4a76c8d overdue in shard 1: censorship detected
validator: Pulse 176: shard 1's committee rotated (rotation 1)
```

## Lifting censorship

```text
$ sion censor
censorship stopped
```

The waiting send then completed, eight Pulses after its first deadline:

```text
$ sion send dana bob 3 --forced
dana signs (forced): Pay 3 USD to bob. Fee 0.0011 USD; valid until Pulse 186.
Pulse 184: Done: paid 3 USD to bob; fee 0.0011 USD.
  digest d4a76c8d812e3a5abb180a26a163a9f9d24fe4f40e5a14cd145681dc0a2d56c5
```

At its second deadline, Pulse 184, the intent could no longer execute, so
the obligation was resolved, and `sion forced` listed nothing pending. The
record keeps the shard's rotation until the epoch ends.

## Posting a forced intent yourself

Any signed transaction can be forced over HTTP:

```bash
curl -s -X POST http://127.0.0.1:8730/v1/forced \
  -H 'Content-Type: application/json' \
  -d '{"tx": "<hex of the Borsh-encoded SignedTransaction>"}'
```

```json
{"digest":"30c4f19181d0a2ddaee54f3ee102f3547f66c263bac9193ca0d2015d159716d3","route":"forced","shard":0}
```

The node hands it to every validator. Intents that cannot be posted (an
expired transaction, a wrong input version, a fee below the minimum, …) are
dismissed in the deferred phase with a `ForcedDismissed` event and a
reason; see [Censorship resistance](../concepts/censorship-resistance.md#the-obligation).

## The demo

`sion demo censorship --local` runs all of this on its own devnet in about
two and a half minutes; its output is in
[Censorship resistance](../concepts/censorship-resistance.md#on-the-devnet).
