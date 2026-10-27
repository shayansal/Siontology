# Bonds, challenges and slashing

## Bonds

A **bond** is money locked behind a bonded atom. It is what an attacker
forfeits when a claim resting on that atom is proven false.

- **Posting.** `PostBond { class, label, amount }` draws USD from the
  sender's coins and posts it behind the atom `bond:<sender>:<label>` (class
  `Economic`, `Institutional` or `Adjudicator`). Posting again tops the bond
  up. Only the sender can bond its own atoms.
- **Locking.** When a claim is issued, every bond it rests on (directly,
  through the claims it references, or as its adjudicator) is locked until
  the end of the claim's challenge period. A claim's cost of corruption
  therefore cannot quietly fall after someone relies on it.
- **Withdrawing.** After the lock, `WithdrawBond { atom }` returns the part
  that is neither slashed nor reserved. There is no CLI command for it yet.

A bond's state is visible in `sion claim show`:

```json
"bond": {
  "amount": 50000000,
  "atom": "05ab20b32354fee47b00f925cbe55bee5f3c5d80f248e7f9d7f159ac9168b90b",
  "locked_until": 5147,
  "owner": "51af757f7b8c05fb893f2aa8ea2282edc04e13af93b41d672515563df2cf6d8f",
  "pool": 0,
  "reserved": 0,
  "slashed": 0
}
```

## Challenges

Anyone can challenge a claim until its challenge period ends.

`Challenge { claim, target, evidence, deposit }`:

- `target` must be a **bonded atom the claim rests on directly**. To
  dispute an atom that a referenced claim rests on, challenge that claim,
  under its own adjudicator. (The claim's adjudicator was chosen by the
  party that put that atom at stake.)
- The claim must name an **adjudicator**. A claim without one cannot be
  challenged in this implementation.
- `evidence` is a digest of the counter-evidence; the evidence itself lives
  elsewhere.
- `deposit` is drawn from the challenger's coins, against frivolous
  challenges.

The challenge's identifier hashes the claim, the target, the evidence and
the challenger. It appears in the `ChallengeOpened` event; `sion events`
prints it.

## Rulings

`Rule { challenge, upheld }` must be sent by the owner of the adjudicator
atom's bond. The adjudicator must not itself be broken, and a challenge is
decided once.

- **Rejected.** The challenger's deposit goes to the owner of the
  challenged bond.
- **Upheld.** The deposit returns to the challenger, and the target atom is
  **broken**:
  1. **Slash.** The bond's entire unslashed amount moves to a pool held
     for compensation.
  2. **Falsify.** Every active claim is re-evaluated in issue order; a
     claim whose formula is now false is falsified, and so is everything
     resting on it.
  3. **Compensate.** Every live reservation on a falsified claim is paid
     from the pools of its broken atoms: its allocation divided by κ, up to
     its value at risk. Reservations on claims that still hold but lost an
     atom become *degraded*.
  4. **Release.** Pool funds beyond every outstanding liability are
     released: a share to the challenger (`challenger_share_permille`, 500
     on the devnet, that is half) and the rest to the treasury.

**Victims are paid first**: compensation comes out of the pool before the
challenger's share.

## On the devnet

Dana's claim rests on her 50 USD bond `deliveries`; the judge's 200 USD
adjudicator bond settles disputes; Erin relied on the claim for 10 USD
(reserving 20 USD). Frank challenges:

```text
$ sion challenge frank b677314a931e86fee2403fe437822ec7d087f0df37dbd9452735b16177c41634 05ab20b32354fee47b00f925cbe55bee5f3c5d80f248e7f9d7f159ac9168b90b 1 --evidence "the dock log shows no delivery"
frank signs: Challenge assumption 05ab20b3 of claim b677314a, depositing 1 USD. Fee 0.0012 USD; valid until Pulse 71.
Pulse 53: Done: challenge opened against assumption 05ab20b3 of claim b677314a; fee 0.0012 USD.
  digest f30a2fcab05141770b82dd8949581829b3fad5886e7904b143ba13283bb14792

$ sion events --from 53 --limit 10
Pulse    53  {"ChallengeOpened":{"challenge":"7b4e969a2f903a5b35b0f661d8908eff35fb3c6607036316d55752593a1dce07","claim":"b677314a931e86fee2403fe437822ec7d087f0df37dbd9452735b16177c41634","target":"05ab20b32354fee47b00f925cbe55bee5f3c5d80f248e7f9d7f159ac9168b90b"}}

$ sion rule judge 7b4e969a2f903a5b35b0f661d8908eff35fb3c6607036316d55752593a1dce07 --upheld
judge signs: As adjudicator, uphold challenge 7b4e969a. Fee 0.0011 USD; valid until Pulse 73.
Pulse 56: Done: 50 USD slashed from assumption 05ab20b3; claim b677314a falsified; account f9b79999 compensated 10 USD; challenge upheld; fee 0.0011 USD.
  digest 60734219cf31f2903e3c2af0c10d5cce4c869ea90bf56f32b94eefc9466b8451
```

Erin (`f9b79999`) went from 99.9988 to 109.9988 USD. Frank got his 1 USD
deposit back plus half of the 40 USD left in the pool after compensation,
ending at 119.9988 USD. The claim's status became
`Falsified { at: Pulse(56), causes: [AtomId(05ab20b3)] }` and its cost of
corruption 0.

## In this implementation

- **Built**: bonds, locks, withdrawal, challenges with deposits, rulings,
  slashing, falsification propagation, compensation, release to the
  challenger and the treasury.
- **Not built**: automatic slashing when a contradiction is
  cryptographically checkable (every challenge needs an adjudicator).
- **Separate**: validators' stake is slashed for equivocation by its own
  transaction, outside the Assurance Algebra; see
  [Slashing for equivocation](../validators/equivocation-slashing.md).
