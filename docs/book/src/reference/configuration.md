# Configuration

What can be configured, where, and the devnet's values. Command-line flags
are in the [command-line reference](cli.md).

## Environment variables

| Variable | Used by | Default | Meaning |
|---|---|---|---|
| `SION_NODE` | every command (`--node`) | `http://127.0.0.1:8730` | The node's HTTP API |
| `SION_HOME` | wallet commands | `~/.sion` (`$HOME/.sion`) | Named wallets' seeds live in `$SION_HOME/wallets/<name>.seed` |
| `SION_CHAIN` | `sion verify`, `sion receipt`, `sion x402 serve`, `sion x402 fetch` (`--chain`) | none | The chain identity you trust. Without it, the node vouches for itself |

Set `SION_CHAIN` once per shell to pin the devnet you use:

```bash
export SION_CHAIN=<identity printed by sion devnet>
```

## Devnet (`DevnetConfig`)

`sion devnet` builds a `sion_node::DevnetConfig` from its flags. Fields
without a flag keep their default; Rust code that starts a devnet
(`sion_node::Devnet::start`) can set them all.

| Field | Default | Flag | Meaning |
|---|---|---|---|
| `chain_id` | `sion-devnet-1` | none | The chain's name; devnet keys derive from it |
| `validators` | 4 | `--validators` | Validators in the genesis active set |
| `standby` | 0 | `--standby` | Validators that run from the start without a seat and join by staking |
| `validator_stake` | 100,000 USD | none | Each genesis validator's stake |
| `min_stake` | 10,000 USD | none | The least stake a validator may register with |
| `epoch_length` | 0 | `--epoch-length` | Pulses per epoch; 0 means one epoch that never ends |
| `unbonding` | 20 | `--unbonding` | Pulses between leaving the active set and withdrawing stake |
| `shards` | 2 | `--shards` | Lane shards |
| `shard_seats` | 4 | `--seats` | Seats per lane committee, capped at the active set's size |
| `rounds_per_pulse` | 4 | `--rounds-per-pulse` | Consensus rounds per Pulse |
| `batch_interval_ms` | 25 | none | How often validators sign a batch of lane approvals |
| `signatures` | dual post-quantum | `--insecure-fast-signatures` | Validator signatures; the flag selects an insecure simulation MAC |
| `data_dir` | none (memory only) | `--data-dir` | Where validators keep logs, snapshots and epoch proofs |
| `snapshot_every` | 50 | none | Pulses between snapshots |
| `fsync` | false | none | Whether to fsync every log record |
| `demo_accounts` | `alice`, `bob`, `carol` | `--accounts` | Named devnet wallets funded at genesis |
| `demo_usd` | 100,000 | none | USD each named account starts with, as four equal coins |
| `accounts` | none | none | Extra genesis accounts |
| `core` | see below | none | Consensus parameters |
| `prove_chunk` | 16 | `--chunk`, `--no-prove` | Transactions per proof chunk; `--no-prove` disables proving |
| `history` | 256 | none | Pulses of outputs, data and traces kept in memory |
| `forced_window` | 8 | `--forced-window` | Pulses a shard has to execute a forced intent |

The genesis also funds a `faucet` account with 1,000,000,000 USD.

## Chain (`ChainConfig`)

Part of the genesis; its digest is in every world commitment, and
transaction signatures cover it.

| Field | Devnet value | Meaning |
|---|---|---|
| `chain_id` | `sion-devnet-1` | The chain's name |
| `shard_count` | 2 | Lane shards |
| `base_fee` | 0.001 USD | Fee per transaction |
| `fee_per_kib` | 0.0001 USD | Fee per started KiB of the body (signatures excluded) |
| `faucet` | the `faucet` account | Who may send `Faucet` transactions (none on a production chain) |
| `max_claim_window` | 1,000,000 Pulses | The longest validity a claim may ask for |
| `max_formula_size` | 256 | The most nodes in a claim's formula, before claim references are unfolded |
| `max_payload_bytes` | 64 KiB | The largest claim payload |
| `challenger_share_permille` | 500 | The successful challenger's share of a released slashing pool, in thousandths; the rest goes to the treasury |

## Validator set (`ValidatorParams`)

Fixed at genesis, held in the validator set object. See
[Validators, staking and epochs](../concepts/committees.md).

| Field | Devnet value | Meaning |
|---|---|---|
| `stake_asset` | USD | The asset validators stake |
| `epoch_length` | `--epoch-length`, or 2⁶³ − 1 for 0 | Pulses per epoch |
| `min_stake` | 10,000 USD | The least stake to register |
| `unbonding` | `--unbonding` (20) | Pulses from exit to withdrawal |
| `seats` | `--seats` (4) | Seats per lane committee |
| `max_active` | 100 | The most validators in the active set |
| `reward_permille` | 700 | Share of each epoch's fees paid to that epoch's validators |
| `reporter_permille` | 50 | Share of a slashed stake paid to the reporter |

## Forced inclusion (`ForcedParams`)

| Field | Devnet value | Meaning |
|---|---|---|
| `window` | `--forced-window` (8) | Pulses a shard has to execute a posted intent |
| `max_pending` | 4,096 | The most obligations pending at once; later intents are dismissed |

## Consensus (`CoreConfig`)

| Field | Devnet value | Meaning |
|---|---|---|
| `leader_timeout_ms` | 400 | How long to wait for an anchor or its votes before advancing |
| `resend_interval_ms` | 600 | How often to re-broadcast and re-request missing parents |
| `gc_depth` | 50 | Rounds of history kept below the last committed anchor |
| `max_items_per_header` | 2,000 | Payload items per header |
| `max_payload_bytes` | 512 KiB | Payload bytes per header |
| `max_rounds_ahead` | 64 | Headers for rounds further ahead are ignored |
| `max_pending_headers` | 4,096 | Headers waiting for parents |
| `repropose_after_rounds` | 8 | Re-propose an item not delivered after this many rounds |
| `leader_schedule` | `RoundRobin` | Anchors in committee order |

## Fixed limits

| Limit | Value | Where |
|---|---|---|
| Inputs per transaction | 1 to 256 | `sion_exec::tx::MAX_INPUTS` |
| Allotments per bounded total | 256 | `sion_exec::escrow::MAX_ALLOTMENTS` |
| Orders per market batch | 1,024 | `sion_exec::market::MAX_BATCH_ORDERS` |
| Formula nesting at issuance | 32 | `sion_assurance::registry::MAX_FORMULA_DEPTH` |
| Formula nesting when decoding | 64 | `sion_assurance::formula::MAX_DECODE_DEPTH` |
| Chain of claim references | 16 | `sion_assurance::registry::MAX_CLAIM_DEPTH` |
| Charters read by a chain check | 16 | specification §5.13 |
| Transaction validity set by the CLI | 20 Pulses | wallet default |
| Lane requests parked for a lagging validator | up to 4 Pulses | lane validator |
| Beacon hash chain per devnet validator | 2¹⁷ Pulses | `sion_node::devnet::BEACON_CHAIN_LENGTH` |
| Devnet faucet grant through the API | 1 micro-unit to 1,000,000 units | `POST /v1/faucet` |
| Faucet grant in a transaction | at most 1,000,000,000 units | `sion_exec::tx::MAX_GRANT` |
