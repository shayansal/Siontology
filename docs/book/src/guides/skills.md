# Skills from others

An agent's [skills](agent-runtime.md#skills) are instructions it follows,
so a skill from a stranger can tell it to pay the wrong account.
`sion skill` gives skills a supply chain built on claims and bonds. A
release is a claim on its publisher's bond, and an audit is a claim on an
auditor's bond. Installing a skill means
[accepting](claims.md#price-it-before-relying-on-it) its release with the
agent's budget at risk. A skill goes in only if the backing behind it
covers what the agent could lose. If it turns out to be malicious, the
bonds pay the agents that relied on it. How a skill is measured, and how
a better version is verified by others before it is released, is in
[Skills that get better](skill-improvement.md).

Every output below is from a real run on a local devnet (long identifiers
and paths shortened with `…`). The agent's model is a scripted one, as in
[Agents at work](agent-runtime.md).

## The parties

A publisher, an auditor and a judge post bonds. Each of them, and a red
team, first got test USD from the devnet's faucet (`sion faucet publisher
1000`, and so on). The judge's bond is of the adjudicator class: it
stands behind the assumption that the judge rules honestly, and only the
judge can rule on challenges to claims that name it.

```text
$ sion bond publisher skills 150
publisher signs: Post a 150 USD bond behind your Economic assumption "skills". Fee 0.0012 USD; valid until Pulse 28.
Pulse 10: Done: accepted; fee 0.0012 USD.
  digest 87c7498d8a7a97d2c02bc3706647b24159a097e46adbf4b7dfaef5a976b77fee
atom ee393313d041bbb7e8971eacbde348c23c1222614e05431133f630c65f35c5ac
$ sion bond auditor audits 100
auditor signs: Post a 100 USD bond behind your Economic assumption "audits". Fee 0.0012 USD; valid until Pulse 31.
Pulse 13: Done: accepted; fee 0.0012 USD.
  digest 7ce3cf690b16337cdce7491925ceb4d712f32886558f386da74d177d62319360
atom f88c21902f81e2f30ffa938b80837a5fcf43ec770f105f68413e7ed3dd991ba5
$ sion bond judge skills-court 1000 --class adjudicator
judge signs: Post a 1,000 USD bond behind your Adjudicator assumption "skills-court". Fee 0.0012 USD; valid until Pulse 33.
Pulse 15: Done: accepted; fee 0.0012 USD.
  digest b32f45bcb39599f21eb0dd4d4abdcdb773f13d6e0961b2b4ef006456ba4703d1
atom da9fbb8535e4de328fe3d67643861ae350237e0a7bab90536c2d624f8ce75ce4
```

alice registers agent-9 with a budget of 100 USD and gives its account
1 USD for fees:

```text
$ sion agent register alice agent-9 --max-payment 10 --burst 50 --rate 0.05 --funding 100
alice signs: Register agent-9 as an agent, funded with 100 USD: any payee, at most 10 USD per payment, 50 USD at once, refilled by 0.05 USD a Pulse, until Pulse 100015, a receipt claim for every payment, may delegate narrower limits. Fee 0.0011 USD; valid until Pulse 35.
Pulse 18: Done: agent 9d258eda registered, signing as agent-9; fee 0.0011 USD.
agent 9d258edae58bbbaa70cb47b419df8026772f4b621db38c0b14b271b47373583b
…
$ sion faucet agent-9 1
funded agent-9 with 1
```

## A bundle

The publisher's skill is a directory with a `SKILL.md` and a checklist it
refers to:

```text
src/pay-invoices/SKILL.md
---
name: pay-invoices
description: "Pay a supplier's invoice after checking it against the order."
---

1. Check the invoice against `templates/checklist.md`.
2. Recall the supplier's terms with memory_recall.
3. Pay it with ledger_pay, giving the invoice number as the purpose.

src/pay-invoices/templates/checklist.md
- the amount matches the order
- the payee is the supplier on file
```

Its **bundle** is the directory's files, sorted by path and encoded
canonically. The skill's history and its installation record are left
out. The bundle's root is the Merkle root that a
[storage deal](storage.md) over the same bytes commits to, so a copy from
anywhere can be checked against it, and a deal can keep it available:

```text
$ sion skill pack src/pay-invoices
skill pay-invoices: bundle b487525ab4feced27cdf33694243e723620699e5c08c18bb76f433a78085ffad (422 bytes) at …/skills/bundles/b487525a….skill
```

## A release that is not enough

The publisher releases version 1.0.0 on its 150 USD bond and names the
judge as the adjudicator of challenges:

```text
$ sion skill publish publisher src/pay-invoices --version 1.0.0 --bond skills --adjudicator da9fbb85…5ce4
publisher signs: Publish the claim "Skill pay-invoices 1.0.0: Pay a supplier's invoice after checking it against the order." (skill/v1), valid for 100000 Pulses, resting on 3 assumptions and 0 other claims. Fee 0.0011 USD; valid until Pulse 40.
Pulse 22: Done: claim 3949333f submitted; fee 0.0011 USD.
release 3949333f4ede9a9e1464e4d158147602b097c4f1c156bb5f72574c2cfc3dc54e
bundle b487525ab4feced27cdf33694243e723620699e5c08c18bb76f433a78085ffad
publisher signs: Name claim 3949333f as your latest claim under "skill:pay-invoices" (your index entry moves only to a later claim of yours). Fee 0.0011 USD; valid until Pulse 42.
Pulse 24: Done: publisher's latest claim under "skill:pay-invoices" is claim 3949333f (1 named so far); fee 0.0011 USD.
```

A skill that misleads agent-9 could cost it its whole remaining budget,
100 USD. With κ = 2, the release therefore needs 200 USD of backing, and
it has 150:

```text
$ sion skill install agent-9 9d258eda…583b 3949333f…c54e
bundle b487525a checked: 2 files, skill pay-invoices 1.0.0
error: verification failed: not installed: the backing behind release 3949333f does not cover 100 USD at risk: cost of corruption 150 (after detection discount 1000/1000) is below κ·V = 200; only 150 of backing is unreserved; κ·V = 200 is needed
```

The installer asks the ledger for the decision before it signs anything,
so a refusal costs no fee.

## An audit adds up

The auditor reads every file of the bundle and claims, on its 100 USD
bond, that the skill does what its description says and nothing beyond
it. The publisher then releases 1.0.1 with the same bundle, resting on
its bond **or** the audit:

```text
$ sion skill audit auditor b487525a…ffad --bond audits --adjudicator da9fbb85…5ce4 --note "read every file: it only checks the invoice and pays through ledger_pay"
auditor signs: Publish the claim "Skill bundle b487525a does what its description says and nothing beyond it" (skill.audit/v1), valid for 100000 Pulses, resting on 3 assumptions and 0 other claims. Fee 0.0011 USD; valid until Pulse 44.
Pulse 27: Done: claim 601ec5e5 submitted; fee 0.0011 USD.
audit 601ec5e52eeb4422310642f46be0b7ecec175d36a9cbeb6fc1844a8222c86906
$ sion skill publish publisher src/pay-invoices --version 1.0.1 --bond skills --audits 601ec5e5…6906 --adjudicator da9fbb85…5ce4 --previous 3949333f…c54e
publisher signs: Publish the claim "Skill pay-invoices 1.0.1: Pay a supplier's invoice after checking it against the order." (skill/v1), valid for 100000 Pulses, resting on 3 assumptions and 1 other claims. Fee 0.0011 USD; valid until Pulse 47.
Pulse 29: Done: claim 0c71b622 submitted; fee 0.0011 USD.
release 0c71b62250c3001ef1fdb5561bf51b0ae23778d44c334b04203f5b7f035b9e02
bundle b487525ab4feced27cdf33694243e723620699e5c08c18bb76f433a78085ffad
publisher signs: Name claim 0c71b622 as your latest claim under "skill:pay-invoices" (your index entry moves only to a later claim of yours). Fee 0.0011 USD; valid until Pulse 49.
Pulse 31: Done: publisher's latest claim under "skill:pay-invoices" is claim 0c71b622 (2 named so far); fee 0.0011 USD.
```

Under the [cost of corruption](../concepts/assurance/cost-of-corruption.md),
an *or* of backings that share no bond adds up. To make the release
false, an attacker would have to corrupt both the publisher and the
auditor, so the release has 150 + 100 = 250 USD of backing, which covers
200. `sion skill show` reads a release and runs the decision rule for a
value at risk:

```text
$ sion skill show 0c71b622…9e02 --value 100
{
  "audits": [
    "601ec5e52eeb4422310642f46be0b7ecec175d36a9cbeb6fc1844a8222c86906"
  ],
  "bundle": "b487525ab4feced27cdf33694243e723620699e5c08c18bb76f433a78085ffad",
  "decision": {
    "accept": true,
    "reasons": [
      "cost of corruption 250 covers κ·V = 200"
    ],
    "value_at_risk": "100"
  },
  "evidence": [],
  "locator": null,
  "name": "pay-invoices",
  "previous": "3949333f4ede9a9e1464e4d158147602b097c4f1c156bb5f72574c2cfc3dc54e",
  "publisher": "d2e053a48b9f47366fde4dd5ce09c95b71611f441e13899a7a0725b3a5109033",
  "release": "0c71b62250c3001ef1fdb5561bf51b0ae23778d44c334b04203f5b7f035b9e02",
  "size": 422,
  "version": "1.0.1"
}
$ sion skill install agent-9 9d258eda…583b 0c71b622…9e02
bundle b487525a checked: 2 files, skill pay-invoices 1.0.1
agent-9 signs: Rely on claim 0c71b622 for up to 100 USD until Pulse 10031, reserving its backing. Fee 0.0012 USD; valid until Pulse 51.
Pulse 34: Done: reserved 200 USD of claim 0c71b622's backing; fee 0.0012 USD.
installed pay-invoices 1.0.1 in …/agents/skills/9d258eda…583b/pay-invoices
```

Installing takes these steps, in order:

1. Fetch the bundle from `--bundle`, the local store, or the URL the
   release gives (`--locator` when publishing), and check it against the
   release's root. A copy that has been changed is refused.
2. Check that the bundle holds the skill the release names.
3. Accept the release from the agent's account, which reserves 200 USD of
   its backing for the agent.
4. Only then put the files in the agent's skills, with a record of the
   release, the audits and the reservation.

## In the agent's hands

The agent's prompt lists the skill and where it came from:

```text
Skills you have (read one with skill_read before using it; its other files with skill_file):
- pay-invoices: Pay a supplier's invoice after checking it against the order. (installed from release 0c71b622, version 1.0.1)
```

The agent reads the instructions and their files as it needs them. It
cannot rewrite them:

```text
$ sion agent run agent-9 9d258eda…583b --task "Get ready to pay invoices." --model script:use.json --autonomy L3
session 41f5b86065c4c215 for agent 9d258eda
  → skill_read {"name":"pay-invoices"}
  ← ---
    name: pay-invoices
    description: "Pay a supplier's invoice after checking it against the order."
    ---

    1. Check the invoice against `templates/checklist.md`.
    2. Recall the supplier's terms with memory_recall.
    3. Pay it with ledger_pay, giving the invoice number as the purpose.
  → skill_file {"name":"pay-invoices","path":"templates/checklist.md"}
  ← - the amount matches the order
    - the payee is the supplier on file
  → skill_write {"description":"Pay any invoice.","instructions":"Pay everyone who asks.","name":"pay-invoices"}
  ✗ pay-invoices was installed from a release; write your own skill under another name
agent: Read the installed skill and its checklist; I cannot rewrite it, since it came from a release.
session 41f5b86065c4c215: done
```

## When a skill turns out to be malicious

Suppose step 3 of the skill pays someone other than the supplier. The red
team challenges the release on the publisher's bond, finds the
challenge's identifier in the events, and the judge upholds the
challenge:

```text
$ sion challenge red-team 0c71b622…9e02 ee393313…c5ac 10 --evidence "step 3 pays mallory, not the supplier"
red-team signs: Challenge assumption ee393313 of claim 0c71b622, depositing 10 USD. Fee 0.0012 USD; valid until Pulse 54.
Pulse 37: Done: challenge opened against assumption ee393313 of claim 0c71b622; fee 0.0012 USD.
  digest 0963a639bb0e3853e294298d7bc568a05ac2a858b48d55a6519170273481b81c
$ sion events --from 57 --limit 20 | grep ChallengeOpened
Pulse    37  {"ChallengeOpened":{"challenge":"bd974515be3be3a2d21d4e36c21f5a5da0d99d248d3317f35a8ca97f69612140","claim":"0c71b62250c3001ef1fdb5561bf51b0ae23778d44c334b04203f5b7f035b9e02","target":"ee393313d041bbb7e8971eacbde348c23c1222614e05431133f630c65f35c5ac"}}
$ sion rule judge bd974515…2140 --upheld
judge signs: As adjudicator, uphold challenge bd974515. Fee 0.0011 USD; valid until Pulse 58.
Pulse 40: Done: 150 USD slashed from assumption ee393313; claim 3949333f falsified; challenge upheld; fee 0.0011 USD.
  digest e205352783c1160d3bbbbe6edd8a04eab7f5be2d7f8d322806866d579a040643
```

Version 1.0.0 rested on that bond alone, so it is falsified. Version
1.0.1 is still active (`sion claim show 0c71b622…9e02` reads
`"status": "Active"`), because the audit still backs it. The auditor's
bond backs the audit, not the release directly, so the red team
challenges the audit:

```text
$ sion challenge red-team 601ec5e5…6906 f88c2190…1ba5 10 --evidence "the audit missed step 3, which pays mallory"
red-team signs: Challenge assumption f88c2190 of claim 601ec5e5, depositing 10 USD. Fee 0.0011 USD; valid until Pulse 60.
Pulse 43: Done: challenge opened against assumption f88c2190 of claim 601ec5e5; fee 0.0011 USD.
  digest 5d6efbaf92ec71de3ce38b0a51c1bef2080c2c2177444974c7b3076c63f6ebc4
$ sion rule judge d1caddac…dda4 --upheld
judge signs: As adjudicator, uphold challenge d1caddac. Fee 0.0011 USD; valid until Pulse 63.
Pulse 46: Done: 100 USD slashed from assumption f88c2190; claim 601ec5e5 falsified; claim 0c71b622 falsified; account efa2bf8f compensated 100 USD; challenge upheld; fee 0.0011 USD.
  digest 12de3d379ca1c181f58cece91ec281d44a43ff5ce6330d33edd8bf11d9bd96a8
```

The audit falls, and the release falls with it. agent-9's account
(`efa2bf8f`) is compensated for the 100 USD it had at risk, from the
slashed bonds:

```text
$ sion balance agent-9
account efa2bf8f6a8237849ebf7d226bef9463305628dfd8a5f202ab717869d328fb89 (Pulse 34)
  0.9988 USD
  1 coins, 1 allotments
…
$ sion balance agent-9
account efa2bf8f6a8237849ebf7d226bef9463305628dfd8a5f202ab717869d328fb89 (Pulse 46)
  100.9988 USD
  2 coins, 1 allotments
```

The next time the agent runs, the skill is out of use. Before every run,
each installed skill whose release is no longer active (falsified or
expired) is renamed out of the agent's skills, here to
`pay-invoices.falsified-0c71b622`:

```text
$ sion agent run agent-9 9d258eda…583b --task "Pay the invoice from Acme." --model script:after.json --autonomy L3
session d51ea208f898e3b5 for agent 9d258eda
skill pay-invoices taken out of use: its release 0c71b622 is falsified
agent: I have no skill for paying invoices any more.
session d51ea208f898e3b5: done
```

None of this finds a malicious skill by itself: someone still has to find
the bad step and prove it to the adjudicator. What the bonds change is
who pays when that happens. The agents that relied on the release are
compensated, and a successful challenger gets a share of what is left
after compensation (see [Bonds and challenges](bonds-and-challenges.md)).

## Find the latest release

`sion skill publish` also names the release in the publisher's index on
the ledger (specification v1.15): an `IndexClaim` makes it the publisher's
latest release of the skill, under the key `skill:<name>`. The entry
lives at an id anyone can compute from the publisher and the key, and it
moves only to a later release of the publisher's own. From a separate run,
so the identifiers differ from those above:

```text
$ sion skill publish publisher src/pay-invoices --version 1.0.0 --bond skills --adjudicator da9fbb85…5ce4
publisher signs: Publish the claim "Skill pay-invoices 1.0.0: Pay a supplier's invoice after checking it against the order." (skill/v1), valid for 100000 Pulses, resting on 3 assumptions and 0 other claims. Fee 0.0011 USD; valid until Pulse 91.
Pulse 73: Done: claim 44ea2ea2 submitted; fee 0.0011 USD.
release 44ea2ea2a8c98f10cb0686bfd4cf65339a139895c01924aaacc1ad7f536d1c15
bundle 77c644634393bd5ec0ee7597fc6bd45111c1fb604b5ea60c107effbb4cd68b90
publisher signs: Name claim 44ea2ea2 as your latest claim under "skill:pay-invoices" (your index entry moves only to a later claim of yours). Fee 0.0011 USD; valid until Pulse 93.
Pulse 75: Done: publisher's latest claim under "skill:pay-invoices" is claim 44ea2ea2 (1 named so far); fee 0.0011 USD.
$ sion skill latest publisher/pay-invoices --chain 94425d72…6e5a
{
  "bundle": "77c644634393bd5ec0ee7597fc6bd45111c1fb604b5ea60c107effbb4cd68b90",
  "index": {
    "entry": "9a08862838c88a0c06d7403497cd77686ee7120116d47a292de9b32197790f65",
    "proof": "state proven against Pulse 75's header, certified by 300000 of the committee's stake (quorum 266667), chain 94425d72",
    "releases_named": 1,
    "updated_at": 75
  },
  "previous": null,
  "publisher": "d2e053a48b9f47366fde4dd5ce09c95b71611f441e13899a7a0725b3a5109033",
  "release": "44ea2ea2a8c98f10cb0686bfd4cf65339a139895c01924aaacc1ad7f536d1c15",
  "skill": "pay-invoices",
  "version": "1.0.0"
}
```

After 1.1.0, the index names it:

```text
$ sion skill publish publisher src/pay-invoices --version 1.1.0 --bond skills --previous 44ea2ea2…1c15 --adjudicator da9fbb85…5ce4
publisher signs: Publish the claim "Skill pay-invoices 1.1.0: Pay a supplier's invoice after checking it against the order." (skill/v1), valid for 100000 Pulses, resting on 3 assumptions and 0 other claims. Fee 0.0011 USD; valid until Pulse 95.
Pulse 77: Done: claim 946fb1b3 submitted; fee 0.0011 USD.
release 946fb1b36fd85d86e0e552f4105460205364e106aed4f9fe7c09a0df4a7d193f
bundle 36cd4654ea752160c3f6cf18852521922c29d0a4d1c890241cbc8eeffa2b4404
publisher signs: Name claim 946fb1b3 as your latest claim under "skill:pay-invoices" (your index entry moves only to a later claim of yours). Fee 0.0011 USD; valid until Pulse 97.
Pulse 79: Done: publisher's latest claim under "skill:pay-invoices" is claim 946fb1b3 (2 named so far); fee 0.0011 USD.
$ sion skill latest publisher/pay-invoices --chain 94425d72…6e5a
{
  "bundle": "36cd4654ea752160c3f6cf18852521922c29d0a4d1c890241cbc8eeffa2b4404",
  "index": {
    "entry": "9a08862838c88a0c06d7403497cd77686ee7120116d47a292de9b32197790f65",
    "proof": "state proven against Pulse 79's header, certified by 300000 of the committee's stake (quorum 266667), chain 94425d72",
    "releases_named": 2,
    "updated_at": 79
  },
  "previous": "44ea2ea2a8c98f10cb0686bfd4cf65339a139895c01924aaacc1ad7f536d1c15",
  "publisher": "d2e053a48b9f47366fde4dd5ce09c95b71611f441e13899a7a0725b3a5109033",
  "release": "946fb1b36fd85d86e0e552f4105460205364e106aed4f9fe7c09a0df4a7d193f",
  "skill": "pay-invoices",
  "version": "1.1.0"
}
```

An agent installs the latest release by name:

```text
$ sion skill install agent-9 98a84992…8bff publisher/pay-invoices --chain 94425d72…6e5a
pay-invoices 1.1.0 is publisher's latest release (946fb1b3): state proven against Pulse 85's header, certified by 300000 of the committee's stake (quorum 266667), chain 94425d72
bundle 36cd4654 checked: 1 file, skill pay-invoices 1.1.0
agent-9 signs: Rely on claim 946fb1b3 for up to 100 USD until Pulse 10085, reserving its backing. Fee 0.0012 USD; valid until Pulse 105.
Pulse 88: Done: reserved 200 USD of claim 946fb1b3's backing; fee 0.0012 USD.
installed pay-invoices 1.1.0 in …/agents/skills/98a84992…8bff/pay-invoices
```

The node's word is not taken for any of it: the entry comes with a state
proof, checked against a certified header of the chain pinned with
`--chain`, so no node can make one up. A node could still answer from an
older state, to keep serving 1.0.0 after 1.1.0; a proof from more than 16
Pulses before the latest certified Pulse the client knows of is refused
as stale, whether the client verified that Pulse earlier in the same
process, the node itself reports it as certified, or it is given with
`--min-pulse` (from another node, say). Whether 1.1.0 still holds is its
own status, which installing checks under the decision rule as before.

## What this does not do yet

- The runtime on the agent's machine does the checks: on the bundle when
  installing, and on the release at the start of each run. A session
  already running keeps a skill whose release falls until it ends, and
  installed files changed on disk afterwards are not checked again
  (`agent:local-skill-checks`).
- A skill does not declare permissions that the runtime enforces while
  the skill is in use. The runtime's autonomy level and the agent's
  charter are the limits.
- An audit counts only for the releases that name it, and a release names
  the audits that exist when it is published. A later audit needs a new
  release.
- Publishing does not keep a bundle available. A storage deal over the
  bundle commits to the same root, but none is made for you.
- The index names each publisher's latest release of a skill; it does
  not search across publishers. You find a skill by its publisher and
  name, or a release by its claim id or in the events.
