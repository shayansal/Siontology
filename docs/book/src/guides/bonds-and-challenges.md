# Bonds and challenges

A bond is money that stands behind an assumption. If a claim resting on it
proves false, an adjudicator rules, the bond is slashed, and the people
who relied on the claim are compensated from it. This guide runs that whole
path on a devnet. The concepts are in
[Bonds, challenges and slashing](../concepts/assurance/bonds-challenges-slashing.md).

The cast: **dana** issues a claim backed by her bond, **judge** is the
adjudicator, **erin** relies on the claim, and **frank** challenges it.

## Post bonds

```text
Usage: sion bond [OPTIONS] <FROM> <LABEL> <AMOUNT>

Options:
      --class <CLASS>  economic, adjudicator or institutional [default: economic]
```

A bond stands behind the sender's own atom `bond:<sender>:<label>`. The
first bond of a label registers the atom; later bonds of the same label add
to it.

```text
$ sion bond judge arbitration 200 --class adjudicator
judge signs: Post a 200 USD bond behind your Adjudicator assumption "arbitration". Fee 0.0012 USD; valid until Pulse 35.
Pulse 17: Done: accepted; fee 0.0012 USD.
  digest d799eafb0568f720da9dff50f52d7dc35b3adee650f0e5662eb60f59f57a5993
atom c007f9d7ea8bd2ce4000c8a422629a482b03e9bfbada1c910f1c257d7f4202c5

$ sion bond dana deliveries 50
dana signs: Post a 50 USD bond behind your Economic assumption "deliveries". Fee 0.0011 USD; valid until Pulse 37.
Pulse 19: Done: accepted; fee 0.0011 USD.
  digest 26fd5c67f0f8303653b14705273433f14a11485acb1e212e7c311c831d38b6ff
atom 94d5e302f3e8e746e194c7ef107eea0b7e474aaae51409841add560bf804cc5d
```

The receipt says `accepted` because the bond is registered in the Pulse's
deferred phase, after the transaction; the event `BondPosted` appears
there. The last line is the atom's identifier, which claims and challenges
refer to.

Dana then issues a claim on her bond with the judge as adjudicator, and
erin relies on it for 10 USD, reserving 20 USD (κ·V) of the bond; see
[Issuing and assessing claims](claims.md).

## Challenge

```text
Usage: sion challenge [OPTIONS] <FROM> <CLAIM> <ATOM> <DEPOSIT>

Options:
      --evidence <EVIDENCE>  Evidence (hashed and recorded) [default: ""]
```

A challenge names one bonded atom of the claim and carries a deposit. The
evidence text is hashed and the hash recorded; the text itself is not
published.

```text
$ sion challenge frank 9cfc28039b98694e547d9cc469a9b550c1aa1ae3e92fbdbeec419e5c4ac2ccb1 94d5e302f3e8e746e194c7ef107eea0b7e474aaae51409841add560bf804cc5d 1 --evidence "the dock log shows no delivery"
frank signs: Challenge assumption 94d5e302 of claim 9cfc2803, depositing 1 USD. Fee 0.0012 USD; valid until Pulse 46.
Pulse 28: Done: challenge opened against assumption 94d5e302 of claim 9cfc2803; fee 0.0012 USD.
  digest 503c3da8f4e898b5513f932df0d2251cdbb719dfeb578233f716575ebf9e61b8
```

The receipt names the challenge only by its claim and atom. Its identifier
is in the event:

```text
$ sion events --from 28 --limit 10
Pulse    28  {"ChallengeOpened":{"challenge":"069ca6acfd1c9dc9ea8844e4dfd4262f737121c74cbc091238959521189097fd","claim":"9cfc28039b98694e547d9cc469a9b550c1aa1ae3e92fbdbeec419e5c4ac2ccb1","target":"94d5e302f3e8e746e194c7ef107eea0b7e474aaae51409841add560bf804cc5d"}}
```

Only claims that name an adjudicator can be challenged this way:

```text
Pulse 20: Failed (assurance: challenge: claim names no adjudicator; only cryptographic evidence can break its atoms); nothing happened except the fee of 0.0011 USD.
```

## Rule

The owner of the adjudicator's bond rules. Upholding the challenge says the
challenged atom is broken:

```text
$ sion balance erin
account f9b79999824af932a759cd47258f11b68c1a5be2c6fe18e8cbcd4e871f19ab0c (Pulse 29)
  99.9988 USD
  1 coins, 0 allotments

$ sion rule judge 069ca6acfd1c9dc9ea8844e4dfd4262f737121c74cbc091238959521189097fd --upheld
judge signs: As adjudicator, uphold challenge 069ca6ac. Fee 0.0011 USD; valid until Pulse 49.
Pulse 31: Done: 50 USD slashed from assumption 94d5e302; claim 9cfc2803 falsified; account f9b79999 compensated 10 USD; challenge upheld; fee 0.0011 USD.
  digest 12e0c64fbe1e850bbb03357ce2fbf4df934a403480d0b66c916d00ae0b5154d5

$ sion balance erin
account f9b79999824af932a759cd47258f11b68c1a5be2c6fe18e8cbcd4e871f19ab0c (Pulse 31)
  109.9988 USD
  2 coins, 0 allotments

$ sion balance frank
account a2229501a66be2aa4963d60e7938b6577a0f5b2360611b3f7a4224996cf64674 (Pulse 31)
  119.9988 USD
  3 coins, 0 allotments
```

In one transaction:

1. Dana's 50 USD bond was slashed into a pool.
2. The claim was falsified, and so would be every claim resting on it.
3. Erin, who had reserved backing for a value of 10 USD, was paid 10 USD
   (`account f9b79999` is erin; the receipt names accounts the CLI has no
   wallet name for by their short identifier).
4. Of the 40 USD left in the pool, the successful challenger received half
   (`challenger_share_permille` 500): frank got his 1 USD deposit back plus
   20 USD. The rest went to the treasury.

The claim now shows its status and a cost of corruption of 0:

```text
  "status": "Falsified { at: Pulse(31), causes: [AtomId(94d5e302)] }"
```

Rejecting instead (`sion rule judge <challenge>` without `--upheld`)
leaves the claim active and the bond intact.

Only the adjudicator can rule, and only on a challenge that exists:

```text
Pulse 25: Failed (invalid transaction: unknown challenge); nothing happened except the fee of 0.0011 USD.
```

## Withdrawing a bond

A bond is locked while any claim resting on it can still be challenged:
until the claim's validity ends plus its challenge period, and longer for
claims that reference it. After that, `WithdrawBond { atom }` returns the
free part. No CLI command sends it yet; it is available through the Rust
API (`sion_exec::TxKind::WithdrawBond`).
