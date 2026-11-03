# Wallets and keys

A wallet is an account's keys plus the logic that turns an operation into a
signed transaction: coin selection, the fee, the expiry and the signatures.

## Named wallets

```text
$ sion keys new dana
created dana: account dab40d27bb62fa8dc40c1bf8194f6d04bff293ac65dd08ace075a8d6d5cec5be
seed stored in $HOME/.sion/wallets/dana.seed

$ sion keys show dana
account dab40d27bb62fa8dc40c1bf8194f6d04bff293ac65dd08ace075a8d6d5cec5be
ML-DSA-44 key   621fafde6a1a72503c71b2a1e9c5da1212d5e3eb4560f4e13c21485196269d79
SLH-DSA key     412112d70e98c1a97c2dffa3b396fc99caae90e6c1fccef6ecedbd5b14eb0333
```

- The seed is 32 random bytes from the operating system, stored as 64 hex
  digits in `$SION_HOME/wallets/<name>.seed` (default `~/.sion/wallets`),
  readable by its owner only (mode 0600). `sion keys new` refuses to
  overwrite an existing file. A seed made by an earlier version was
  readable by others on the machine: `chmod 600 ~/.sion/wallets/*.seed`.
- Both keys derive from the seed: an ML-DSA-44 key and an SLH-DSA-SHA2-128f
  key. The account's policy requires both signatures.
- `sion keys show` prints the account identifier and fingerprints of the two
  public keys.
- To back up a wallet, copy its seed file. Anyone who reads it controls the
  account.

## Deterministic devnet wallets

**Any name without a seed file is a deterministic devnet wallet**: its seed
is a hash of the name. The genesis accounts `alice`, `bob`, `carol` and
`faucet`, and every account the demos use, are wallets of this kind. Anyone
who knows the name can derive the keys, so they are for devnets and tests
only.

The CLI does not warn when it falls back to a deterministic wallet:
`sion balance mallory` or `sion send mallory …` work for any name. Create
real wallets with `sion keys new`.

## Names and identifiers

Wherever a command takes an account (`sion balance`, the recipient of
`sion send`, `--payees`, …) you can pass a wallet name or a 64-digit hex
account identifier. Commands that sign (`sion send <from>`, `sion bond
<from>`, …) need a wallet name, because they need the keys.

Receipts print names for the devnet's usual accounts and for wallets the
command used, and short identifiers for everyone else:

```text
Pulse 31: Done: 50 USD slashed from assumption 94d5e302; claim 9cfc2803 falsified; account f9b79999 compensated 10 USD; challenge upheld; fee 0.0011 USD.
```

## How a wallet builds a transaction

1. It looks up the sender's account object. If the account does not exist
   on the ledger yet, the transaction carries the wallet's genesis policy,
   which creates the account when it executes.
2. It selects USD coins, largest first, to cover the fee plus any USD the
   operation draws (a transfer's outputs, a bond, a challenge deposit, a
   charter's funding, a buy order's escrow). The first input is the gas
   coin that pays the fee. Other assets are drawn from coins of that asset.
3. It computes the minimum fee for the transaction's size:
   `base_fee + fee_per_kib × ⌈size / 1024⌉` (0.001 and 0.0001 USD on the
   devnet).
4. It sets the expiry to the current Pulse plus 20.
5. It signs the transaction's digest with both keys, for this chain's
   configuration.

Within one process, the wallet holds back coins it has put into pending
transactions, so two transactions built in the same Pulse never spend the
same coin version. Separate CLI invocations do not share that memory, so
wait for one command to finish before the next (the CLI does: every
command waits for its transaction to execute).

A genesis account starts with four coins, so it can have four transactions
in flight at once. An account funded once by the faucet has one coin, and
one transaction at a time.

## Rotating keys

`RotatePolicy { policy, viewing_key }` replaces an account's authorization
policy, and optionally its ML-KEM viewing key, without changing its
identifier. The new policy must require at least two signature families.
There is no CLI command for it yet; it is available through the Rust API
(`sion_exec::TxKind::RotatePolicy`).

## Hardware and recovery

Not built: passkeys and hardware keys, threshold keys, social recovery, and
SLH-DSA-SHA2-128s keys from the CLI (the library supports them; the CLI
always creates SLH-DSA-SHA2-128f keys).
