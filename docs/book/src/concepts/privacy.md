# Privacy

The architecture makes state private by default: the network stores
commitments, and proofs show that rules were followed without revealing the
data. This implementation builds the encryption half of that and none of
the zero-knowledge half. Amounts and balances are public.

## What exists

| Feature | How |
|---|---|
| Encrypted claim payloads | A claim's payload can be sealed to an ML-KEM-768 viewing key (FIPS 203) with ChaCha20-Poly1305. Data harvested today stays unreadable to a future quantum computer |
| Committed claim payloads | A claim can publish only a hash of its payload; the owner keeps the data |
| Viewing keys | An account can set an ML-KEM viewing key with `RotatePolicy` |
| Sealed market orders | An order can be encrypted to a market's committee: the payload key is split with Shamir t-of-n sharing and each share sealed to one member's ML-KEM key. Any *t* members reveal the batch after it seals. The committee threshold is an atom in any formula that relies on the sealing |
| Signatures off the data layer | Published Pulse data carries transaction bodies, never signatures |

## What does not exist

- **Confidential amounts.** Transfer amounts and balances are visible to
  everyone (atom `impl:public-amounts`). Private notes with nullifiers, as
  the architecture describes, are not built.
- **Compliance-by-proof.** KYC, sanctions screening, reporting thresholds,
  the Travel Rule and AI-content marking, proven without revealing the
  data, all need a succinct zero-knowledge stack. A claim can carry such an
  attestation today only as an issuer-signed claim.
- **Scoped viewing keys** that grant an auditor one account, one period or
  one claim type.
- **Private computation** in MPC, FHE or TEE coprocessors.
- **Personhood.** Issuer-signed credentials with per-application
  nullifiers can be expressed as claims, but there is no nullifier scheme
  and no unlinkability.

## Compliance as a proof, in the architecture

| Obligation | What the proof shows | What stays private |
|---|---|---|
| KYC | Both parties hold a credential from an accredited issuer, issued within 12 months | Names, documents and, if desired, which issuer |
| Sanctions screening | Neither party appears in the sanctions list committed at time *t* | Who the parties are |
| Reporting thresholds | The amount is below the threshold, or a filed-report claim exists | The amount, when below the threshold |
| Travel Rule | Originator and beneficiary data were encrypted to the counterparty's provider | That data, from everyone else |
| AI-content marking | The output carries a receipt from a declared model and provider | Prompt and inputs |

A proof shows that the rule as written was applied to the committed data.
Whether a regulator accepts a proof in place of a record is a legal
question in each jurisdiction.

## What to assume today

Anything you put in a public payload, every transfer, every balance and
every charter's caveats are public on a devnet. Devnet validator keys
(`impl:devnet-keys`), and the keys of named wallets that have no seed file,
can be derived by anyone. Do not put personal or confidential data on a
devnet.
