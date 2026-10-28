# Running a network of processes

`sion devnet` runs every validator as a thread of one process. `sion node`
runs one validator per process, and the processes talk to each other over
QUIC, on one machine or on several. Each validator generates its own keys,
a genesis file collects their public entries, and every connection is
authenticated against keys that the genesis and a peer file pin. This guide
runs four validators and a gateway on one machine.

It is a test network. What it does not do yet is listed at the
[end](#what-is-not-built-yet); the most important limit is that **a
validator that stops cannot rejoin** ([Restarts](#restarts)).

## How the nodes connect

- **Transport.** QUIC (`quinn`) with TLS 1.3 only, the hybrid key exchange
  X25519MLKEM768 only, and ML-DSA-44 signatures only (rustls with the
  aws-lc-rs provider). A peer that offers only classical key exchange
  cannot connect.
- **Network keys.** Each node has an ML-DSA-44 network key, separate from
  a validator's dual consensus keys. Nodes present it as a raw public key
  (RFC 7250) instead of an X.509 certificate, and both ends check it
  against the key pinned for the other: a node accepts connections only
  from the validators in the genesis and peer files and the gateways in the
  peer file, and a dialing node accepts only the key it expects at that
  address. Right after the handshake both sides say which protocol
  version, chain identity and node they are; a mismatch closes the
  connection.
- **Records.** A validator's network record binds its network key and
  addresses to its identity on one chain. It is signed with both of the
  validator's consensus keys (ML-DSA-44 and SLH-DSA-SHA2-128f), so it
  proves that the validator holds them, and whoever hosts the genesis file
  cannot forge one.
- **Mesh.** Every pair of validators keeps one connection; the validator
  with the smaller identifier dials. Gateways dial every validator, and
  approvals come back on the gateway's own connections. Each message
  class (control, consensus, ingress, bulk) has its own stream in each
  direction, with its own size limit and priority, so a 2 MB certificate
  never delays a vote. Queues are bounded per peer and class, and per-peer
  quotas drop floods.

## 1. Keys

Each validator generates its keys in a directory of its own. On separate
machines, each operator runs this on its own machine and sends only the
public `validator.json`:

```text
$ sion keys validator --dir net/v0 --chain sion-localnet --address 127.0.0.1:9100 --operator operator-0
validator ae62504408e7c3d402ee2b842cd7785339c8095d313e7b523b6b7531ba772960 of chain sion-localnet: keys in net/v0 (validator.key, network.key, beacon.key: keep them secret)
network key 356d9838, reachable at 127.0.0.1:9100
its entry for the genesis: net/v0/validator.json
$ sion keys validator --dir net/v1 --chain sion-localnet --address 127.0.0.1:9101 --operator operator-1
$ sion keys validator --dir net/v2 --chain sion-localnet --address 127.0.0.1:9102 --operator operator-2
$ sion keys validator --dir net/v3 --chain sion-localnet --address 127.0.0.1:9103 --operator operator-3
$ sion keys gateway --dir net/gw --name gw
gateway gw (node:6583dd8d49a4db72207d9e29aad6957a2479fb3e1f5169a134703a5f5933324f): network key in net/gw (network.key: keep it secret)
its entry for the peer file: net/gw/gateway.json
```

| File | Contents |
|---|---|
| `validator.key` | The seed of the validator's dual consensus keys (secret, mode 0600) |
| `network.key` | The seed of the node's ML-DSA-44 network key (secret) |
| `beacon.key` | The seed of the validator's beacon hash chain (secret) |
| `validator.json` | Public: the operator account, the tip of the beacon hash chain, and the signed network record (chain, validator, network key, addresses, sequence) |
| `gateway.json` | Public: a gateway's name and network key |

`--address` is where other nodes reach the validator (`host:port`, UDP);
repeat it to list several. `--operator` is the wallet name or account that
stakes for the validator and receives its rewards. `sion keys` refuses to
overwrite keys that already exist.

## 2. The genesis file

```text
$ sion genesis --validator net/v0/validator.json --validator net/v1/validator.json \
    --validator net/v2/validator.json --validator net/v3/validator.json \
    --gateway net/gw/gateway.json --accounts alice,bob,carol --out net
genesis of chain sion-localnet: 4 validators, written to net/genesis.json
peer file (1 gateways accepted): net/peers.json
TEST NETWORK: anyone can spend from alice, bob, carol (their keys derive from their names)
chain identity (pin it: sion verify --chain …): 3a5d55d40caaa2522dcb00f20a80aa680a683c963321302b73bcc2a5da0afd8b
```

`genesis.json` holds everything every node must agree on: the chain's
name, the protocol parameters (`--shards`, `--seats`,
`--rounds-per-pulse`, `--epoch-length`; the others as `sion devnet` has
them), the genesis validators with their stake (`--stake`, 100,000 USD by
default) and their entries, and the accounts funded at genesis. Every node
derives the same genesis state and chain identity from it, the way the
devnet does from its configuration. `sion genesis` checks every record's
signatures, that no validator or network key appears twice, and that every
stake reaches the minimum; every node checks the file again when it starts.

`--accounts` funds named accounts and `--faucet` funds the devnet faucet.
Their keys derive from their names, so anyone can spend from them: use
them on test networks only.

`peers.json` lists the gateways that validators accept, by network key,
and can carry validator records newer than the genesis file's (a record
with a higher sequence replaces the older one; a validator that moves signs
a new record). Peers come from these files and nothing else
(`net:static-peers`). Nodes read `peers.json` next to the genesis file
unless `--peers` names another.

## 3. The validators

One process per validator, each with its own directory and API port:

```text
$ sion node --dir net/v0 --genesis net/genesis.json --api 127.0.0.1:8731
Siontology node: validator ae62504408e7c3d402ee2b842cd7785339c8095d313e7b523b6b7531ba772960 of chain sion-localnet, QUIC on 127.0.0.1:9100, API on http://127.0.0.1:8731
chain identity (pin it: sion verify --chain …): 3a5d55d40caaa2522dcb00f20a80aa680a683c963321302b73bcc2a5da0afd8b
Pulse     6  certified     6  round    28  executed [0,0,0]  links 4/4
Pulse    13  certified    13  round    59  executed [0,0,0]  links 4/4
```

and likewise `net/v1` on `--api 127.0.0.1:8732`, `net/v2` on 8733 and
`net/v3` on 8734. `links 4/4` counts the connections that are up: three
validators and the gateway.

| Flag | Notes |
|---|---|
| `--dir` | The keys `sion keys validator` wrote. The node checks that its network key is the one its record pins and that its beacon seed matches its genesis entry |
| `--genesis`, `--peers` | The network's files (above) |
| `--listen` | Where to listen for QUIC; by default the first address of the validator's record |
| `--api` | The HTTP API, the same routes as the devnet's ([HTTP API](../reference/http-api.md)). Keep it on localhost: it is plain HTTP |
| `--gateway` | Also run a gateway in the process, so the API accepts transactions. It shares the node's identity |
| `--prove`, `--chunk` | Run the prover: a World Proof for every Pulse (off by default: every node would otherwise prove) |
| `--no-fsync` | Do not fsync log records |
| `--duration`, `--report` | As for `sion devnet` |

A node's status (`sion status`, `GET /v1/status`) includes what it sent
and received per message class, kind and peer, and under
`network.endpoint` each link's connection state, queues and drops, and
the handshakes it refused.

What a node keeps in `data/` inside its directory:

| File | Contents |
|---|---|
| `genesis` | The genesis digest. A directory holds one chain |
| `signed-session` | The last consensus session this validator started signing in ([below](#restarts)) |
| `validator/wal-*.log`, `validator/snapshot-*.bin` | Its write-ahead log and snapshots, as the devnet's ([Storage](../architecture/storage.md)) |
| `validator/lanes.log` | Its lane approvals, written before they leave the node |
| `validator/epochs/` | With epochs that end: the proofs linking each epoch to the previous one, for light clients |

## 4. A gateway on its own

```text
$ sion gateway --dir net/gw --genesis net/genesis.json --follow http://127.0.0.1:8731 --api 127.0.0.1:8740
Siontology gateway node:6583dd8d49a4db72207d9e29aad6957a2479fb3e1f5169a134703a5f5933324f of chain sion-localnet, following http://127.0.0.1:8731, API on http://127.0.0.1:8740
links 4/4  following: yes
```

The gateway dials every validator, collects lane approvals into
certificates and hands certified transactions to consensus. It learns the
committees, and answers queries about the state, from the node named by
`--follow`, which it trusts: a lying node can make it ask the wrong
validators or show wrong balances, though never get a transaction
certified that a committee did not approve (`net:gateway-trusts-node`).
Its peer-file entry must be in the `peers.json` the validators run with,
or they refuse its connections.

## 5. Use it

Every client command works against a gateway's API, or a node's that runs
`--gateway`:

```text
$ sion --node http://127.0.0.1:8740 send alice bob 5
alice signs: Pay 5 USD to bob. Fee 0.0011 USD; valid until Pulse 45.
Pulse 27: Done: paid 5 USD to bob; fee 0.0011 USD.
  digest 6eb81dad580b489b182bfd282a193a9dfcf7514e7d7fd27afb7661981618ac30
$ sion --node http://127.0.0.1:8733 balance bob
account a27e5e28be5f6fcef56c7adbd682ceb0bc6966f666e87696998c743dc6f88b37 (Pulse 34)
  100,005 USD
  5 coins, 0 allotments
```

A light client pins the chain identity that `sion genesis` printed and
checks a certified Pulse from any node:

```text
$ sion --node http://127.0.0.1:8732 verify --chain 3a5d55d40caaa2522dcb00f20a80aa680a683c963321302b73bcc2a5da0afd8b --no-world
chain: 3a5d55d4 matches the pinned identity; 4 validators, keys derive their identifiers ✓
Pulse 37 header ba2d7cd9 (state root b24cca06, knowledge root 5bffd82f, 0 claims)
certificate: 3 of 4 validators signed, 300000 of 400000 stake (quorum 266667), each signature ML-DSA-44 + SLH-DSA-SHA2-128f ✓
forced inclusion: no overdue intents (0 pending within their deadline) ✓
receipts: 1 of 1 transactions proven against the receipts root, in the certified order ✓
state: proof skipped (node already at Pulse 44)
availability: 16 of 16 random shares of the 16×16 square verified; a withholding would go unnoticed with probability < 1.8e-3 ✓
```

(`--no-world` because the nodes ran without `--prove`.) On the 2-core
machine this guide was written on, the four nodes (a dev build) closed
about 0.7 Pulses a second with dual post-quantum signatures.

## Restarts

On a devnet, every restart starts a new consensus session for every
validator, so none can sign a round twice. A validator of a network
restarts alone, and its session is the epoch: started again within the
epoch, it would begin at round 1 of a session it has already signed in
and could sign a second, different header for a round, which is
equivocation and slashable. Restarting into the current session safely
needs a consensus signing log, which is not built. So a validator records
each session in `data/signed-session` before it signs anything in it, and
refuses to start in a session it has recorded:

```text
$ sion node --dir net/v1 --genesis net/genesis.json --api 127.0.0.1:8732
error: configuration: this validator already signed in consensus session 0 and would start session 0: rejoining a session it signed in needs the consensus signing log, which is not built yet, so it could sign a second header for a round and be slashed. A validator of a network that stops cannot rejoin yet (net:no-rejoin)
```

The network carries on without it while more than two thirds of the stake
remains (three of four validators here). But the stopped validator cannot
come back: it restarts from its own log, in the session it last signed
in, and catching up to a later session from certified Pulses (Pulse sync)
is not built either. Every stopped validator is a fault for good, and
once more than a third of the stake has stopped, the chain halts
(`net:no-rejoin`). Stop validators only to retire them.

The guard lives in `data/`. Deleting that directory, or running the same
keys from two directories, defeats it: the validator would sign round 1
of session 0 again, which is the equivocation it exists to prevent.

## What is not built yet

- Restarting a stopped validator (the consensus signing log), and
  catching up from certified Pulses after missing many (Pulse sync).
- The epoch hand-over's drain window and lane-lock hand-over
  (`impl:reliable-handover`, `impl:lock-handover`).
- Peers from the state: records live in files, and `set_peers` is not
  driven by committee changes yet.
- A TCP fallback for networks that block UDP, HTTPS on the API, and
  stake-weighted bandwidth under congestion.
- A gateway that follows committees as a light client (from epoch proofs)
  instead of trusting a node.

See [Networking between machines](../research/networking.md) for the
design and the plan, and the
[implementation status](https://github.com/ColdAI-org/siontology/blob/main/docs/implementation-status.md)
for the atoms named here (`net:transport-ml-dsa`, `net:aws-lc`,
`net:raw-public-keys`, `net:static-peers`, `net:partial-synchrony`,
`net:gateway-trusts-node`, `net:no-rejoin`).
