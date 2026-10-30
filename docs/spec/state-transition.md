# The Siontology state transition — specification v1.15

Status: normative for this reference implementation. Two implementations
exist: the reference executor (`crates/sion-exec`, proof stack A) and an
independent one written from this document (`crates/sion-spec`, proof stack
B). Where they disagree, one of them is wrong, and the differential tests in
`crates/sion-spec/tests` exist to find out which.

The key words MUST, MUST NOT and MAY are used as in RFC 2119.

## 0. Scope

This document specifies the function

```text
apply_chunk(statement, witness, transactions | deferred_items) -> accept | reject
```

that a proof stack evaluates for every chunk of every Pulse, and through it
the whole state transition: preparing and executing one transaction, the
three phases of a Pulse, and the commitments a chunk statement carries.

It does **not** re-specify:

- **Data layouts.** The Rust type definitions in `sion_core`, `sion_crypto`,
  `sion_assurance`, `sion_clearing` and the *type definitions* in
  `sion_exec` (`object`, `account`, `claim`, `charter`, `cohort`, `escrow`, `market`,
  `stream`, `memory`, `task`, `delegation` (its types and constants),
  `storage` (its types and constants), `beacons` (its types and
  constants),
  `assurance`, `tx`, `state`, `validators` (its types), `pulse::{ChunkStatement,
  ChunkTrace, Witness, PulseInput}`, `execute::{Deferred, Event, ExecStatus,
  TxEffects}`) are
  normative; their canonical encoding is Borsh. One rule goes beyond the
  layout: a `Formula` nested deeper than 64 levels
  (`sion_assurance::formula::MAX_DECODE_DEPTH`, counted as
  `Formula::depth`) is not a valid encoding, so a transaction carrying one
  fails to decode and never enters a Pulse.
- **Shared primitives.** Both implementations use, and trust as separate
  atoms of the World Proof's formula: hashing and Merkle structures
  (`sion_core`), signature verification (`sion_crypto`), the Assurance
  Algebra (`sion_assurance`: registry operations, cost of corruption,
  reservation, decision rule, slashing), clearing-certificate verification
  (`sion_clearing::verify_clearing`), committee envelopes
  (`sion_crypto::CommitteeEnvelope::open`), validator identities and
  signatures (`sion_crypto::{validator_id, verify_validator}`), committee
  sampling (`sion_consensus::{sample_committee, resample_committee}`) and
  the consensus message formats (`sion_consensus::{Header::digest,
  Header::signing_digest, SignedHeader::verify_signature,
  ApprovalProof::verify_signed}`).

An implementation of this document MUST NOT call the reference executor's
logic: `sion_exec::execute`, `sion_exec::pulse` (other than the type
definitions and the commitment helpers named in §7), `sion_exec::stateless`,
`sion_exec::wallet`, `sion_exec::sim`, `sion_exec::genesis`, the
`sion_exec::stream` helpers (`step`, `walk`, `extends`, `checkpoint_leaf`,
`jump_hashes`, `verify_jump`, `PayerChain`, `Stream::checkpoint_count`), the
`sion_exec::task` helpers (`result_formula`, `worker_bond`,
`TaskResult::payload`), the `sion_exec::delegation` helpers (`mul_div`,
`cap_permille`, `within_cap`, `resolve`, `Pool::{shares, value_of,
shares_for, bonded, earning_value, owed}`), the `sion_exec::storage`
helpers (`challenge_index`, `round_seed`, `current_round`,
`DataTree`, `split_leaves`, `Deal::end`), the `sion_exec::beacons` helpers
(`slot_of`, `next_boundary`), the `sion_exec::tx` helpers `fee_floor`,
`required_fee` and `sign_batch`, nor
the `sion_exec::charter` helpers (`Bucket::{level, refill, carve, take}`,
`Lineage::{child, ancestry}`, `Narrowing::{attenuates, permits}`), the
`sion_exec::cohort` helpers (`member_node`, `empty_node`, `join`,
`height`, `verify_member`, `CohortTree`), the methods `Allotment::spend`,
`Caveats::attenuates`, `OrderTerms::escrow`,
`Market::clearing_batch`, `Market::batch_index`, `Equivocation::offender`
and the `ValidatorSet` methods. It MAY call the type
helpers `Transaction::{digest, domain, class, size}`,
`SignedTransaction::digest`, `TxBatch::message`, `Object::{digest,
is_tombstone, reference, as_coin}`, `ChainConfig::{min_fee, digest}`,
`Payload::size`, `WorldCommitment::digest`, `ChunkStatement::digest`,
`claim::{claim_id, knowledge_leaf, formulas::*}`, `object::{created_id,
well_known::*}`, `tx::object_domain`, `tx::signing_context`,
`tx::batch_signing_context`, `charter::{keyless_account, lineage_id,
overlay_id}`,
`market::sealing_aad`, `execute::{bond_atom, registered_atom,
bond_owner}`, and `validators::{weight, permille_of, share_of,
possession_digest, rotation_seed}`.

## 1. Conventions

- `H(domain, bytes)` is `sion_core::hash::hash_domain`; `Hp(domain, parts)`
  is `sion_core::hash::hash_parts`. `enc(x)` is the Borsh encoding.
- Amounts are `Amount(u128)` micro-units; `units(n) = n × 10⁶`. Any
  arithmetic overflow or underflow in this document is an error of the
  operation in which it occurs (§3.4), unless stated otherwise.
- `USD` is `AssetId::usd()`.
- Maps and sets are ordered (`BTreeMap`, `BTreeSet`); "in order" means
  ascending key order.
- `ctx` is the Pulse context: `pulse`, `beacon`, `config` (a
  `ChainConfig`), and `verify_signatures` (always true for proof stacks).

## 2. State

The state is a map `objects: ObjectId → Object`, a knowledge log (an
RFC 9162 Merkle log of claim leaves), and the Pulse number. Objects are
never deleted; a consumed object is replaced by a tombstone
(`ObjectData::Tombstone { consumed_by, at }`) that keeps its id, owner and
version.

The state root is the root of the sparse Merkle tree mapping each object
id to `Object::digest()` (`sion_core::smt`). The knowledge root is the RFC
9162 root of the log's leaves.

Well-known objects (all created at genesis): the assurance domain
`well_known::registry()` (`ObjectData::Assurance`), the treasury
`well_known::treasury()`, the schedule `well_known::schedule()`, the
validator set `well_known::validators()` (`ObjectData::Validators`) and the
forced-inclusion obligations `well_known::forced()` (`ObjectData::Forced`).
The account object of account `a` is `well_known::account(a)`.

The validator set holds the parameters (`stake_asset`, `epoch_length`,
`min_stake`, `unbonding`, `seats`, `max_active`, `reward_permille`,
`reporter_permille`), the current `epoch`, `epoch_start`, the sampling
`seed`, the `active` set as (validator, weight) pairs sorted by validator,
every validator's record, this epoch's shard `rotations`, and the
`fees_mark`, the treasury's USD fees when the last epoch ended. `epoch_end =
epoch_start + max(epoch_length, 1) − 1`
(saturating). At genesis, `epoch = 0`, `epoch_start = 1`, `seed` is the
genesis digest, `fees_mark = 0`, and an `EpochEnd` event is scheduled at
`epoch_end`.

The treasury holds, by asset, its `balances`, what was ever `minted`, and
the `fees` ever credited to it (a running total that only grows; at
genesis all three are empty).

### 2.1 Reads and the witness

Every object an implementation reads while executing a chunk MUST be in the
chunk's witness; a read of an uncovered id rejects the whole chunk (§8).
This document therefore names what each step reads. An implementation MUST
NOT read objects this document does not require it to read. (Reading fewer
is impossible: every read named here decides an outcome.) A read of an
object the chunk has already written is served by that write and needs no
witness entry.

## 3. One transaction: bookkeeping

Executing a transaction works on a scratch view of the objects: reads see
the state left by earlier transactions of the Pulse plus this
transaction's own writes; writes are kept only if the transaction ends in
success or failure as specified in §4.

The transaction keeps:

- `digest = SignedTransaction::digest()`;
- `originals: ObjectId → Version`, the version of every pre-existing object
  when it was first read in this transaction;
- `created: set of ObjectId`, objects created by this transaction;
- `next_index: u32 = 0`;
- `fee_paid: bool = false`;
- the effects under construction: `fee = 0`, `created`, `mutated`,
  `consumed` (sets), `events` and `deferred` (lists).

### 3.1 load, store, create, consume

- **load(id)** reads the object; it is an error ("missing object") if it
  does not exist. If `id ∉ created`, record `originals[id] = version` unless
  already recorded. **try_load(id)** is the same but returns nothing (no
  error) for a missing object.
- **store(object)** writes `object` with its version set as follows:
  - if `object.id ∈ created`: version 0;
  - else if `object.id ∈ originals`: version `originals[id] + 1`, and if
    the object is not a tombstone, add `id` to `effects.mutated`;
  - else: version 0, add `id` to `created` and to `effects.created`.
- **create(owner, data)** computes `id = created_id(digest, next_index)`,
  increments `next_index`, stores `Object { id, version: 0, owner, data }`
  and returns `id`.
- **create_at(id, owner, data)** stores `Object { id, version: 0, owner,
  data }` under an `id` the operation derived (a lineage id, §5.13);
  `next_index` is unchanged. The operation makes sure that `id` does not
  exist: by construction, or by a `try_load` where this document says so.
- **mint(owner, asset, amount)**: if `amount = 0` do nothing; otherwise
  `create(Owner::Account(owner), Coin { asset, amount })`.
- **consume(id)**: `load(id)`, then store
  `Object { id, version: <as loaded>, owner: <as loaded>, data:
  Tombstone { consumed_by: digest, at: ctx.pulse } }` (store assigns the
  version), remove `id` from `effects.mutated`, add it to
  `effects.consumed`. Returns the object as loaded.

### 3.2 draw(asset, amount)

1. `available = 0`, `used = []`. For each input `r` of `tx.inputs`, in
   order: `load(r.id)`; if it is a coin of `asset`, add its amount to
   `available` and append `r.id` to `used`.
2. `fee = tx.fee` if `asset = USD` and not `fee_paid`, else 0.
3. `need = amount + fee`. If `available < need`: error "insufficient funds".
4. `consume` every id in `used`, in order.
5. If `fee > 0`: `fee_paid = true`, `effects.fee = fee`, append
   `Deferred::Fee { asset: USD, amount: fee }` to `effects.deferred`.
6. `mint(tx.sender, asset, available − need)` (the change).

### 3.3 charge_fee_from_gas and finalize_inputs

- **charge_fee_from_gas**: if `fee_paid` or `tx.fee = 0`, do nothing.
  Otherwise `load(tx.inputs[0].id)`; it is an error if it is a tombstone or
  not a coin; subtract `tx.fee` from its amount (error if insufficient);
  store it; `fee_paid = true`; `effects.fee = tx.fee`; append
  `Deferred::Fee { asset: USD, amount: tx.fee }`.
- **finalize_inputs**: for each input `r`, in order: `load(r.id)`; if its
  current version still equals `r.version` (nothing in this transaction
  stored it), store it unchanged (bumping its version).

### 3.4 Errors

An "error" in an operation aborts it. Where it aborts decides the outcome
(§4). Error messages are not committed and MAY differ between
implementations; only the outcome class (success, failed, rejected) is
committed.

## 4. One transaction: outcome

### 4.1 prepare

In this order; any failure is a **rejection**:

1. `ctx.pulse > tx.expires_at` → reject (expired).
2. `tx.inputs` empty, or longer than `MAX_INPUTS` (256,
   `sion_exec::tx::MAX_INPUTS`) → reject. Duplicate input ids → reject.
3. If the kind is `Faucet`, `tx.fee ≠ 0` → reject. Otherwise `tx.fee <
   required_fee(signed)` (§4.3) → reject.
4. The **authorizing policy** MUST be found (§4.3). Finding it may store
   the sender's new account object.
5. The authorization MUST pass the **check** under that policy (§4.3).
6. For each input `r` at position `i`, in order: `load(r.id)`; its version
   MUST equal `r.version`; its owner MUST be `Owner::Account(tx.sender)`; it
   MUST NOT be a tombstone. (An input may be the account object stored in
   step 4.) For `i = 0`:
   - for `Faucet`: `tx.sender` MUST equal `ctx.config.faucet` and `r.id`
     MUST be the sender's account object;
   - otherwise the object MUST be a USD coin with amount ≥ `tx.fee`.

### 4.2 execute

1. Run **prepare**. On failure the transaction is **rejected**: nothing is
   written, and the effects are `{ digest, sender, class: tx.class(),
   status: Rejected, fee: 0, created: ∅, mutated: ∅, consumed: ∅, events:
   [], deferred: [] }`.
2. Run the kind's **apply** (§5), then **charge_fee_from_gas**, then
   **finalize_inputs**. If all succeed the transaction **succeeds**: its
   writes are kept and its effects are as built, with status `Success`.
3. If any of step 2 fails, discard every write of steps 1–2 and start again
   with fresh bookkeeping: **prepare**, **charge_fee_from_gas**,
   **finalize_inputs**. If these succeed, keep their writes; the effects
   are those of this second run with `events` emptied and status `Failed`.
   If they fail (impossible for a transaction that prepared once: the gas
   coin covers the fee and a `Faucet` pays none; specified for
   completeness), the transaction is rejected as in step 1.

`TxEffects.class` is `tx.class()` in every case.

### 4.3 Authorization

A transaction is authorized in one of two ways, told apart by
`signed.batch`:

- **Signed by itself** (`batch = None`): `signed.auth` signs the
  transaction's digest.
- **A batch member** (`batch = Some(m)`, v1.13): `signed.auth` signs a
  batch `m.batch = TxBatch { signer, size, root }` of `size` transactions,
  `root` being the RFC 9162 Merkle root (`sion_core::merkle::root`) over
  their leaves `merkle::leaf_hash(digest bytes)`, in the batch's order.
  `m.index` is the member's position, `m.path` its inclusion path, and
  `m.charter` names, for a keyless sender, the sender's charter. Each
  member carries the batch's one authorization, so a node that checks it
  once has checked it for every member.

**The fee floor.** For a transaction signed by itself, `required_fee(signed)
= ctx.config.min_fee(tx.size())`. For a batch member,

```text
share        = ⌊ctx.config.base_fee × SIGNATURE_SHARE_PERMILLE / 1000⌋
required_fee = ctx.config.min_fee(tx.size()) − share + ⌈share / max(m.batch.size, 1)⌉
```

with `SIGNATURE_SHARE_PERMILLE = 800`
(`sion_exec::tx::SIGNATURE_SHARE_PERMILLE`, a protocol constant, so the
configuration and its digest are unchanged); the multiplication
overflowing is an error. `share` is the part of the base fee that pays for
checking an authorization, and a batch's members split it.

**The authorizing policy**, found in one of two ways:

1. **The sender's own**, if `batch = None` or `m.charter = None`. For a
   batch member, `m.batch.signer` MUST equal `tx.sender`. Then
   `try_load(well_known::account(tx.sender))`:
   - if it exists, it MUST be `ObjectData::Account`, and the policy is its
     `policy`;
   - otherwise `tx.genesis_policy` MUST be present, MUST pass
     `AuthPolicy::validate`, and `policy.account_id()` MUST equal
     `tx.sender`; then **store** a new account object `Object { id:
     well_known::account(sender), version: 0, owner: Account(sender), data:
     Account(AccountState { id: sender, policy, viewing_key: None,
     created_at: ctx.pulse, rotations: 0 }) }`.
2. **The nearest keyed ancestor's**, if `m.charter = Some(c)`: the sender
   is a keyless subagent (§5.36). `tx.sender` MUST equal
   `keyless_account(c)`. Load `c` (MUST be a charter); its `agent` MUST
   be `tx.sender`. Then load `c.parent`, its parent, and so on (each MUST
   be a charter), until one whose agent is not keyless: a charter `a` with
   id `id_a` is keyless if `a.agent = keyless_account(id_a)`. The search
   fails at a charter without a parent, and after `MAX_LINEAGE_DEPTH`
   ancestors (8, §5.13): if the last one loaded
   is keyless, its parent is not loaded. The ancestor found MUST have `agent = m.batch.signer`. Then
   `try_load(well_known::account(m.batch.signer))` MUST exist and be
   `ObjectData::Account`; its `policy` is the policy. Nothing is stored,
   and `tx.genesis_policy` is ignored.

`keyless_account(c) = AccountId(H("sion/agent/keyless", c bytes))`
(`sion_exec::charter::keyless_account`). Account ids are derived from
policies in another domain, so no policy hashes to a keyless account and
no key ever controls one: it acts only through batches its nearest keyed
ancestor signs. That ancestor's own budget contains the subagent's, and it
can already suspend or revoke it.

**The check**, under the policy found:

- Signed by itself: `policy.verify(signing_context(ctx.config), digest
  bytes, &signed.auth)` MUST succeed, where `signing_context(config)` is
  `TX_DOMAIN`, a `/`, and `config.digest()` in lowercase hex
  (`sion_exec::tx::signing_context`). A transaction signed for one chain
  therefore never verifies on a chain with another configuration.
- A batch member, in this order: `1 ≤ m.batch.size ≤ MAX_BATCH` (256,
  `sion_exec::tx::MAX_BATCH`);
  `merkle::verify_inclusion(m.batch.root, m.batch.size, m.index,
  merkle::leaf_hash(digest bytes), m.path)` MUST succeed (so `m.index <
  m.batch.size`); and `policy.verify(batch_signing_context(ctx.config),
  m.batch.message() bytes, &signed.auth)` MUST succeed, where
  `batch_signing_context(config)` is `TX_DOMAIN`, `/batch/` and
  `config.digest()` in lowercase hex (`sion_exec::tx::batch_signing_context`)
  and `TxBatch::message() = H("sion/tx/batch", enc(batch))`. The two
  contexts differ, so a transaction's signature never verifies as a
  batch's, nor a batch's as a transaction's.

The check reads nothing: finding the policy does all the reading, so what
a transaction reads does not depend on whether its signatures verify.

A batch's members are separate transactions. Each has its own inputs, fee,
expiry and outcome, and each may execute in a different Pulse, or never;
nothing orders them. A signer that signs two batches holding transactions
that spend the same input version gets at most one of them certified, as
with any two transactions.

**What the fee floor trusts.** It splits the share by the size a batch
declares. A signer that declares 256 members and sends one pays about a
fifth of the base fee for a check that validators run in full. The
reference node checks a batch's authorization once for all its members,
after the cheap checks of each, and accepts batches whole
(`sion_node::owners`, `POST /v1/batches`); a node may refuse a batch it
does not receive in full.

## 5. Operations

Notation: "event E" appends E to `effects.events`; "defer D" appends D to
`effects.deferred`. Unless stated, every read is a `load`.

### 5.1 Transfer { asset, outputs }

1. `outputs` MUST have 1 to 256 entries, none with amount 0.
2. `total = Σ amounts`; `draw(asset, total)`.
3. For each `(to, amount)` in order: `mint(to, asset, amount)`.
4. Event `Transferred { asset, outputs }`.

### 5.2 Faucet { grants }

1. `grants` MUST have 1 to 256 entries.
2. For each `(to, asset, amount)` in order: amount MUST NOT be 0 and MUST
   NOT exceed `MAX_GRANT` (`sion_exec::tx::MAX_GRANT`, one billion units);
   `mint(to, asset, amount)`; defer `Minted { asset, amount }`; event
   `Minted { to, asset, amount }`.

### 5.3 RotatePolicy { policy, viewing_key }

1. `policy` MUST pass `validate`.
2. The sender's account object MUST be among `tx.inputs`.
3. Load it (MUST be `Account`); set `policy`, `viewing_key`, increment
   `rotations`; store it. Event `PolicyRotated { account: sender }`.

### 5.4 IssueClaim { claim }

1. Validate the body against `ctx.config`: `schema` 1..=128 bytes;
   `subject` ≤ 512 bytes; `payload.size() ≤ max_payload_bytes`;
   `formula.size() ≤ max_formula_size`; `detection_permille ≤ 1000`;
   `1 ≤ valid_for ≤ max_claim_window`; `formula ≠ False`.
2. Defer `IssueClaim { issuer: sender, backer: sender, body: claim }`.
3. Event `ClaimSubmitted { claim: claim_id(sender, claim) }`.

### 5.5 RegisterAtom { class, label }

`class` MUST be `Code` or `Hardware`; `label` 1..=128 bytes. Defer
`RegisterAtom { atom: registered_atom(class, sender, label) }`. No event.

### 5.6 PostBond { class, label, amount }

`class` MUST be `Economic`, `Institutional` or `Adjudicator`; `amount > 0`.
`draw(USD, amount)`; defer `PostBond { atom: bond_atom(class, sender,
label), owner: sender, amount }`. No event.

### 5.7 Accept { claim, value_at_risk, policy, until, nonce }

1. `until > ctx.pulse`.
2. Load the assurance object (MUST be `Assurance`); work on a copy of its
   domain.
3. `(decision, reservation) = sion_assurance::accept_and_reserve(&mut
   domain.registry, policy, sender, claim, value_at_risk, until, nonce)`;
   an error is an error; `reservation = None` is an error ("not accepted").
4. Store the assurance object with the updated domain.
5. Defer `Schedule { at: until, event: RegistryTick }`.
6. Event `Reserved { reservation, claim, amount: decision.required }`.

### 5.8 Release { reservation }

Load the assurance object; `sion_assurance::release(&mut registry,
reservation, sender)`; store it; defer `Schedule { at: ctx.pulse, event:
RegistryTick }`; event `Released { reservation }`.

### 5.9 Challenge { claim, target, evidence, deposit }

1. `draw(USD, deposit)`.
2. Load the assurance object; `id = registry.open_challenge(sender, claim,
   target, evidence, deposit, ctx.pulse)`.
3. `domain.vault += deposit`; `domain.deposits[id] = (sender, deposit)`.
4. Store; event `ChallengeOpened { challenge: id, claim, target }`.

### 5.10 Rule { challenge, upheld }

1. Load the assurance object. The challenge MUST exist; its claim MUST
   exist and name an adjudicator `adj`; `registry.bond(adj).owner` MUST be
   the sender.
2. `outcome = registry.rule(challenge, adj, upheld, ctx.pulse)`.
3. `(challenger, deposit) = deposits.remove(challenge)`, or
   `(challenge.challenger, 0)` if absent. `vault -= deposit`. If `upheld`,
   `mint(challenger, USD, deposit)`; otherwise
   `mint(owner of registry.bond(challenge.target) or challenger, USD,
   deposit)`.
4. If `outcome` is a slash, **settle** it (§5.10.1) with challenger
   `challenger`.
5. Store the assurance object; event `ChallengeRuled { challenge, upheld }`.

#### 5.10.1 Settling a slash

Given `outcome` and `challenger`:

1. Event `Slashed { atom: outcome.atom, amount: outcome.slashed }`.
2. For each falsified entry in order: event `Falsified { claim, causes }`.
3. For each compensation in order: `vault -= amount`;
   `mint(consumer, USD, amount)`; event `Compensated { reservation,
   consumer, amount }`.
4. `released = Σ outcome.released`; `vault -= released`;
   `to_challenger = released × challenger_share_permille / 1000` (rounded
   down, `Amount::mul_ratio`); `mint(challenger, USD, to_challenger)`;
   `to_treasury = released − to_challenger`; if positive, defer
   `Treasury { asset: USD, amount: to_treasury }`.

### 5.11 WithdrawBond { atom }

Load the assurance object; `amount = registry.withdraw_bond(atom, sender,
ctx.pulse)`; `vault -= amount`; `mint(sender, USD, amount)`; store; event
`BondWithdrawn { atom, amount }`.

### 5.12 CreateCharter { agent, caveats, funding, agent_bond }

1. `caveats.expires_at > ctx.pulse`.
2. `draw(USD, funding + agent_bond)`.
3. `charter_id = created_id(digest, next_index)` (the id the next create
   will use); `agent_atom = bond_atom(Economic, sender,
   "charter:" + hex(charter_id))`, where `hex` is the 64-digit lowercase
   form (`ObjectId`'s `Display`).
4. `create(Shared(object_domain("charter", charter_id)), Charter {
   principal: sender, agent, caveats, agent_atom: agent_atom.id, backer:
   sender, parent: None, lineage: Lineage { root: charter_id, path: [] },
   allotments: ∅, created_at: ctx.pulse, identity: None })`.
5. `allotment = create(Account(agent), Allotment { source: charter_id,
   writer: agent, asset: USD, remaining: funding, caveats: Some(caveats),
   bucket: full(caveats), spawned: 0 })`, where `full(k) = Bucket { tokens:
   k.burst, refilled_at: ctx.pulse, burst: k.burst, rate: k.rate }`: an
   agent starts with a full bucket.
6. Load the charter, insert `allotment` into `allotments`, store it.
7. Defer `RegisterAtom { atom: agent_atom }`; if `agent_bond > 0`, defer
   `PostBond { atom: agent_atom, owner: sender, amount: agent_bond }`.
8. Event `CharterCreated { charter: charter_id, allotment }`.

### 5.13 Charter checks

A charter's limits are its caveats, copied into its allotment when it is
funded. What happens to it afterwards (a revocation, a suspension, a
narrowing) is kept in its **overlay**, and its place in its tree is its
**lineage** (v1.14).

- **Lineage.** A root charter (§5.12, §5.35) has `lineage = { root: its
  id, path: [] }`. A charter spawned under a parent `p` with index `i`
  (§5.14, §5.36, §5.56) has the id `lineage_id(p, i) =
  ObjectId(H("sion/id/agent", enc((p, i))))` (`i` a `u64`,
  `sion_exec::charter::lineage_id`) and `lineage = { root:
  p.lineage.root, path: p.lineage.path ++ [i] }`. Its **depth** is `|path|`,
  at most `MAX_LINEAGE_DEPTH` (8, `sion_exec::charter::MAX_LINEAGE_DEPTH`).
  Indices come from the `spawned` counter of the parent's allotment, which
  only the parent's agent writes, so lineage ids never repeat, and spawning
  reads nothing at the new ids.
- **ancestry(c)** of a charter whose lineage is `{ root, path }` is the
  list `a₀ = root`, `aⱼ₊₁ = lineage_id(aⱼ, path[j])`, computed, not read;
  `a_|path|` is the charter itself.
- **Overlays.** The overlay of charter `c` is the object at `overlay_id(c)
  = ObjectId(H("sion/overlay", c bytes))` (`sion_exec::charter::overlay_id`),
  an `AgentOverlay { agent, revoked_at, suspended_by, narrowing }` owned by
  `Shared(object_domain("charter", c))`, written only by §5.16, §5.38,
  §5.39 and §5.57. **overlay(c)** is `try_load(overlay_id(c))` (MUST be an
  overlay if it exists), or else `AgentOverlay { agent: c, revoked_at:
  None, suspended_by: None, narrowing: None }`; **storing** it stores
  `Object { id: overlay_id(c), version: 0, owner: Shared(object_domain(
  "charter", c)), data: o }` (store assigns the version, §3.1).
- **Standing** of charter `c` (already loaded), with its id: ancestry(c)
  MUST end at that id. For each `a` of ancestry(c), from the charter itself
  up to the root (`a_d`, …, `a₀`): `try_load(overlay_id(a))`; if it exists
  it MUST be an overlay `o`, and then `o.revoked_at` set fails ("revoked"),
  then `o.suspended_by` set fails ("suspended"), then `o.narrowing =
  Some(n)` appends `n` to `N`. The check stops at the first failure. A
  standing check **ignoring suspension** skips the `suspended_by` test. A
  passing check yields `d = |c.lineage.path|` and the narrowings `N`, in
  the order found. It reads at most `MAX_LINEAGE_DEPTH + 1` overlays, most
  of them usually absent, and no charter but `c`.
- **refill(b)** of a bucket `b`: if `ctx.pulse > b.refilled_at`, then
  `b.tokens = min(b.burst, b.tokens + b.rate × (ctx.pulse −
  b.refilled_at))` (saturating) and `b.refilled_at = ctx.pulse`.
- **carve(b, burst, rate, take)**: `burst > b.burst` fails ("burst
  exhausted"); `rate > b.rate` fails ("rate exhausted"); `take > b.tokens`
  fails ("bucket too low"); then `b.burst −= burst`, `b.rate −= rate` and
  `b.tokens = min(b.tokens − take, b.burst)`. A child's burst and rate are
  carved out of its parent's bucket, as its budget is out of the parent's
  allotment, and the tokens it starts with out of the parent's tokens.
- **may_spawn(k, N)**: `k.may_delegate` ("delegation not allowed"), and
  `n.may_delegate` for every `n ∈ N` ("narrowed: delegation not allowed");
  then `ctx.pulse ≤ k.expires_at` ("charter expired") and `ctx.pulse ≤
  n.expires_at` for every `n ∈ N` ("narrowed: expired"). An expired charter
  spawns nothing, so it cannot move its budget into children while its
  backer waits to reclaim it.
- **born(k)**: a charter about to be spawned with caveats `k` MUST have
  `k.expires_at > ctx.pulse` ("charter would already be expired"), as a
  root charter must (§5.12 step 1).
- **position(c, s)** of an account `s` under charter `c` (v1.14): 0 if `s =
  c.backer`; otherwise, with ancestry(c) = `a₀ … a_d`, load `a₀`, `a₁`, …,
  `a_{d−1}` in that order (each MUST be a charter), stopping at the first
  whose `agent = s`: for `a_j` the position is `j + 1`. If none matches,
  `s` has no position. A lower position is higher in the tree. At most
  `MAX_LINEAGE_DEPTH` charters are loaded.
- **Oversight.** `s` is an **overseer in good standing** of `c`, at
  position `r`, if `r = position(c, s)` exists and, when `r > 0`, for each
  of `a₀ … a_{r−1}`, in that order, `try_load(overlay_id(a))` finds nothing
  or an overlay (it MUST be one) with neither `revoked_at` nor
  `suspended_by` set: an overseer whose own charter, or one above it, has
  been stopped has no power over those below. The backer always is one. A
  spawned charter's principal is its parent's agent and a root charter's
  is its backer, so the principal is an overseer whenever it is in good
  standing.
- **attenuates(child, parent)** holds when: `parent.payees = None`, or both
  are sets and `child.payees ⊆ parent.payees`; `parent.evidence_above =
  None`, or both are set, `child ≤ parent` and `child.evidence_policy =
  parent.evidence_policy` (v1.14: a laxer policy would pass evidence the
  parent's rejects); `child.max_per_payment ≤
  parent.max_per_payment`; `child.burst ≤ parent.burst`; `child.rate ≤
  parent.rate`; `child.expires_at ≤ parent.expires_at`;
  `parent.receipts ⇒ child.receipts`; `child.may_delegate ⇒
  parent.may_delegate`.
- **narrows(n, prev)** holds for narrowings when: `prev.payees = None`, or
  both are sets and `n.payees ⊆ prev.payees`; `n.max_per_payment ≤
  prev.max_per_payment`; `n.expires_at ≤ prev.expires_at`; `n.may_delegate
  ⇒ prev.may_delegate`.
- **spend(allotment, payee, amount, evidence_ok, N)**, in this order:
  1. `amount > remaining` fails.
  2. If the allotment has caveats `k`: `ctx.pulse > k.expires_at` fails;
     `k.payees` set and `payee ∉ k.payees` fails; `amount >
     k.max_per_payment` fails; then for each `n ∈ N`, in order:
     `ctx.pulse > n.expires_at` fails, `n.payees` set and `payee ∉
     n.payees` fails, `amount > n.max_per_payment` fails; then
     **refill**(bucket); `amount > bucket.tokens` fails ("bucket too
     low"); if `k.evidence_above = Some(t)` and `amount > t` and
     `evidence_ok ≠ Some(true)` fails; then `bucket.tokens −= amount`.
  3. `remaining -= amount`.

**What the buckets bound.** A subtree whose root has caveats with `burst =
σ` and `rate = ρ` pays at most `σ + ρ·τ` in any `τ + 1` consecutive
Pulses, however it is shaped and whatever it spawns: spawning only moves
burst, rate and tokens down the tree, so at every Pulse the subtree's
buckets hold at most `σ` together and refill by at most `ρ` together.
Until v1.13, window limits reset at fixed Pulses, so a subtree could pay
twice its limit across a window's boundary. The bound is property-tested
(`sion-exec/tests/buckets.rs`).

### 5.14 Delegate { charter, sub_agent, caveats, amount }

1. `allotment_id = tx.inputs[1].id` (error if absent); load it (MUST be an
   allotment).
2. Load `charter` (MUST be a charter, call it `p`); **standing** of it,
   yielding `d` and `N`; `d < MAX_LINEAGE_DEPTH` ("too deep").
3. The allotment's `source` MUST be `charter`, its `writer` and the
   charter's `agent` MUST be the sender.
4. The allotment MUST have caveats `k`; **may_spawn**(k, N);
   `attenuates(caveats, k)`; **born**(caveats); **refill**(its bucket);
   **carve**(its bucket, `caveats.burst`, `caveats.rate`, `caveats.burst`).
5. `remaining -= amount` (error if insufficient); `index = spawned`;
   `spawned += 1`; store the allotment.
6. `child_id = lineage_id(charter, index)`; `create_at(child_id,
   Shared(object_domain("charter", child_id)), Charter { principal: sender,
   agent: sub_agent, caveats, agent_atom: p.agent_atom, backer: p.backer,
   parent: Some(charter), lineage: p.lineage.child(index), allotments: ∅,
   created_at: ctx.pulse, identity: None })`, where `l.child(i) = { root:
   l.root, path: l.path ++ [i] }`.
7. `child_allotment = create(Account(sub_agent), Allotment { source:
   child_id, writer: sub_agent, asset: USD, remaining: amount, caveats:
   Some(caveats), bucket: full(caveats), spawned: 0 })`.
8. Load the child charter, insert `child_allotment`, store it.
9. Event `CharterCreated { charter: child_id, allotment: child_allotment }`.

### 5.15 AgentPay { charter, payee, amount, purpose, evidence }

1. Load `allotment_id = tx.inputs[1].id` (MUST be an allotment); load
   `charter` (MUST be a charter); **standing** of it, yielding `N`;
   `source = charter`, `writer = sender`, `charter.agent = sender`; the
   allotment MUST have caveats `k`.
2. `evidence_ok = None`, unless `evidence = Some(c)` and `k.evidence_above
   = Some(t)` and `amount > t`: then load the assurance object and
   `evidence_ok = Some(b)` where `b` is false if `c` is unknown to the
   registry or not `Active`, and otherwise `sion_assurance::decide(registry,
   k.evidence_policy, c, amount, insured = false).accept`.
3. **spend**(allotment, payee, amount, evidence_ok, N); store the
   allotment; `mint(payee, USD, amount)`.
4. If `k.receipts`: build `body = ClaimBody { schema:
   "receipt.agent.payment/v1", subject: "agent paid {amount} USD for:
   {purpose}" (amount in `Amount`'s display form), payload:
   Public(enc((charter, payee, amount, purpose, evidence, digest))),
   formula: formulas::bonded_statement(charter.agent_atom), adjudicator:
   None, detection_permille: 1000, valid_for: min(604800,
   max_claim_window), challenge_period: 86400 }`; defer `IssueClaim {
   issuer: sender, backer: charter.backer, body }`; `receipt =
   Some(claim_id(sender, body))`. Otherwise `receipt = None`.
5. Event `AgentPaid { charter, payee, amount, receipt }`.

### 5.16 RevokeCharter { charter }

Load it (MUST be a charter); the sender MUST be its `agent` (an agent may
retire itself) or an **overseer in good standing** (§5.13): the agent is
compared first, so the overseer search loads nothing when the agent
revokes. `o =
overlay(charter)`; if `o.revoked_at` is unset set it to `ctx.pulse`; store
`o`; event `CharterRevoked { charter }`. Every charter below a revoked one
fails its standing check, so one revocation, one write, stops a whole
subtree.

### 5.17 ReclaimCharter { charter }

1. Load it (MUST be a charter, call it `s`); the sender MUST be
   `s.principal` or `s.backer`.
2. The charter MUST be dead: `ctx.pulse > s.caveats.expires_at`, or its
   **standing check ignoring suspension** fails (run only if it has not
   expired); its own revocation is in its overlay, which that check reads
   first. A suspended charter is not dead: its funds wait for the
   suspension to be lifted or for a revocation.
3. `total = 0`; for each id in `s.allotments`, in order: load it; if it is
   an allotment, `total += remaining` and **consume** it.
4. `mint(s.backer, USD, total)`; event `CharterReclaimed { charter, amount:
   total }`.

### 5.18 CreateBounded { label, asset, total, allotments }

1. At most `MAX_ALLOTMENTS` (256, `sion_exec::escrow::MAX_ALLOTMENTS`)
   allotments; `allotted = Σ amounts`; `allotted ≤ total`.
2. `draw(asset, total)`.
3. `bounded_id = created_id(digest, next_index)`;
   `create(Shared(object_domain("bounded", bounded_id)), BoundedTotal {
   label, asset, manager: sender, total, unallocated: total − allotted,
   allotments: ∅, returned: 0 })`.
4. For each `(writer, amount)` in order: `id = create(Account(writer),
   Allotment { source: bounded_id, writer, asset, remaining: amount,
   caveats: None, bucket: none, spawned: 0 })`, where `none = Bucket {
   tokens: 0, refilled_at: ctx.pulse, burst: 0, rate: 0 }`; record
   `id → (writer, amount)`.
5. Load the bounded total, set `allotments` to the record, store it.
6. Event `BoundedCreated { bounded: bounded_id, allotments: [ids in
   order] }`.

### 5.19 SpendAllotment { to, amount }

Load `tx.inputs[1]` (MUST be an allotment without caveats; its writer MUST
be the sender); **spend** (no caveats, so only the balance check); store;
`mint(to, allotment.asset, amount)`; event `AllotmentUsed { allotment,
amount }`.

### 5.20 ReturnAllotment

Load `tx.inputs[1]` (MUST be an allotment without caveats); **consume** it;
defer `ReturnToBounded { bounded: source, amount: remaining }`; event
`AllotmentUsed { allotment, amount: remaining }`.

### 5.21 Rebalance { bounded, holder, amount }

Load `bounded` (MUST be a bounded total); `manager` MUST be the sender;
`unallocated -= amount` (error if insufficient); `allotment =
created_id(digest, next_index)`; insert `allotment → (holder, amount)`
into its `allotments`; store it; `create(Account(holder), Allotment {
source: bounded, writer: holder, asset: <bounded asset>, remaining: amount,
caveats: None, bucket: none, spawned: 0 })` (`none` as in §5.18); event
`AllotmentUsed { allotment, amount }`.

### 5.22 CreateMarket { label, good, numeraire, batch_length, clear_deadline, committee, threshold }

`1 ≤ batch_length ≤ MAX_PERIOD` and `1 ≤ clear_deadline ≤ MAX_PERIOD`
(`sion_exec::market::MAX_PERIOD`, one million Pulses); if `committee` is
non-empty,
`1 ≤ threshold ≤ |committee|`; `good ≠ numeraire`. `market_id =
created_id(digest, next_index)`; `create(Shared(object_domain("market",
market_id)), Market { label, domain: object_domain("market", market_id),
good, numeraire, batch_length, clear_deadline, committee, threshold,
batches: ∅ })`; event `MarketCreated { market: market_id }`.

### 5.23 SubmitOrder { market, submission }

1. Load `market` (MUST be a market).
2. Plain `terms`: `quantity > 0`; escrow is `(numeraire, limit_price ×
   quantity)` (saturating multiplication) for buys, `(good,
   units(quantity))` for sells; `terms` known.
   Sealed `{ envelope, side, escrow }`: the market MUST have a committee;
   `envelope.members` MUST equal the committee keys' fingerprints in order
   and `envelope.threshold` MUST equal the market's threshold; `escrow >
   0`; the escrow asset is the numeraire for buys, the good for sells;
   terms unknown.
3. `draw(escrow_asset, escrow)`.
4. `order_id = Hp("sion/market/order", [digest bytes])`; defer `AddOrder {
   market, order: PendingOrder { id: order_id, owner: sender, escrow_asset,
   escrow, terms, sealed } }`; event `OrderSubmitted { market, order:
   order_id }`.

### 5.24 RevealBatch { market, batch, shares }

1. Load `market` (MUST be a market); `batches[batch]` MUST exist and be
   `Sealed`.
2. `provided`: the map order id → shares built from `shares` in order
   (later duplicates replace earlier ones).
3. For each order of the batch in order, only if it is sealed and its terms
   are unknown and `provided` has shares for it: open the envelope with
   those shares, domain `SEALED_ORDER_DOMAIN`, associated data
   `sealing_aad(market.domain, batch)`; skip on failure; decode
   `OrderTerms` (skip on failure); compute `need` as in §5.23 (escrow asset
   and amount from the revealed terms); if the asset equals the order's
   escrow asset, `need ≤ escrow` and `quantity > 0`, set the terms and count
   it as revealed.
4. Store the market; event `BatchRevealed { market, batch, revealed }`.

### 5.25 ClearBatch { market, batch, result }

1. Load `market`; `batches[batch]` MUST exist and be `Sealed { beacon, at }`.
2. If any order of the batch has unknown terms, `ctx.pulse ≥ at +
   max(clear_deadline, 1) − 1` MUST hold (that is, `at +
   clear_deadline.saturating_sub(1)`, the addition saturating).
3. Build the clearing batch `sion_clearing::Batch { domain: market.domain,
   pulse: at, beacon, orders }` where `orders` are, in batch order, the
   orders with known terms mapped to `Order { id, owner, side, quantity,
   limit_price }`.
4. `verified = sion_clearing::verify_clearing(&batch, result)` (errors are
   errors; among them, a certificate value whose numerator or denominator
   is longer than `sion_lp::MAX_VALUE_BITS`, 512 bits); `price = verified.price`; `fills` = `result.fills` as a map
   order id → quantity.
5. For each order of the batch in order, with `filled = fills[id]` or 0:
   - if its terms are known, `price` is set and `filled > 0`: `value =
     price × filled` (an overflow is an error); a buy receives `(good, units(filled))` and refunds
     `escrow − value`; a sell receives `(numeraire, value)` and refunds
     `escrow − units(filled)`;
   - otherwise it receives nothing and refunds its whole escrow;
   - `mint(owner, receive_asset, receive)`, then `mint(owner,
     escrow_asset, refund)`.
6. Set the batch to `Cleared { price, volume: verified.volume, welfare:
   verified.welfare, solver: sender }`; store the market; event
   `BatchCleared { market, batch, price, volume, welfare }`.

### 5.26 RegisterValidator { lattice, hash, beacon_tip, stake, possession }

1. `id = validator_id(lattice, hash)`. Load the validator set.
2. `id` MUST NOT be a registered validator; `stake ≥ params.min_stake`.
3. `verify_validator(lattice, hash, possession_digest(ctx.config, sender,
   id) bytes, possession)` MUST hold (a proof that the sender's validator
   holds both keys, bound to this chain and this operator).
4. `draw(params.stake_asset, stake)`.
5. Insert the record `{ id, lattice, hash, operator: sender, stake, status:
   Joining, beacon_tip, rewards: 0, registered_at: ctx.pulse }`; store the
   set. Event `ValidatorRegistered { validator: id, operator: sender,
   stake }`.

A validator is **operated** by `sender` if its record exists and its
`operator` is `sender`; the operations below require it (an error
otherwise).

### 5.27 Unstake { validator }

1. Load the validator set. `staying` = the number of records other than
   `validator` whose status is `Active` or `Joining`. `validator` MUST be
   operated by the sender.
2. If its status is `Joining`: set it `Exited { withdrawable_at:
   ctx.pulse }`. If `Active`: `staying` MUST be positive (the last
   validator cannot leave); set it `Leaving`. Any other status is an error.
3. Store the set. Event `ValidatorLeaving { validator }`.

### 5.28 WithdrawStake { validator }

1. Load the validator set; `validator` MUST be operated by the sender, with
   status `Exited { withdrawable_at }` and `withdrawable_at ≤ ctx.pulse`.
2. Take `stake` and `rewards` from the record, set both to 0 and the
   status to `Withdrawn`; store the set.
3. `mint(sender, params.stake_asset, stake)`, then `mint(sender, USD,
   rewards)`. Event `StakeWithdrawn { validator, amount: stake }`.

### 5.29 ClaimRewards { validator }

Load the validator set; `validator` MUST be operated by the sender and its
`rewards` MUST be positive. Set `rewards` to 0; store the set; `mint(sender,
USD, rewards)`. Event `RewardsClaimed { validator, amount: rewards }`.

### 5.30 ReportEquivocation { evidence }

1. Load the validator set. The evidence names an **offender**, under these
   rules (any failure is an error); signatures are checked with the keys
   the set records for the signer (`verify_validator(record.lattice,
   record.hash, message, signature)`; an unknown signer fails):
   - `Headers { first, second }`: the two headers have the same `author`,
     `chain`, `epoch`, `shard` and `round` and different digests, and both
     `SignedHeader::verify_signature` checks pass. The offender is the
     author.
   - `Approvals { first, second }`: the two approvals' batch headers have
     the same `validator`, `chain`, `epoch` and `shard`; the two
     transactions have different digests; `ctx.pulse` is at most both
     transactions' `expires_at`; some input `(id, version)` of the first is
     an input of the second; and each proof's `verify_signed(tx digest,
     tx.inputs, keys)` passes. The offender is the batch's validator.
     (An honest validator approves a version again only once the first
     transaction can no longer execute: it succeeded or failed, which
     moves every input it names to a new version (§3.3), or it expired. A
     rejected transaction wrote nothing and may still execute later, so
     its approval holds its inputs until it expires. Two live approvals
     are therefore proof.)
2. The offender's record MUST exist and its status MUST NOT be `Slashed` or
   `Withdrawn`.
3. Take `stake` and `rewards` from the record, set both to 0 and the status
   to `Slashed { at: ctx.pulse }`; store the set.
4. **The pool** (v1.11, §5.45): `try_load(well_known::delegations())`; if
   it exists (it MUST be a delegation book) and has a pool for the
   offender: `slashed = permille_of(value, DELEGATION_SLASH_PERMILLE)`;
   `value −= slashed`; store the book. Otherwise `slashed = 0`. Every
   share of the pool, pending and unbonding ones included, is worth less.
5. For `(asset, own, pooled)` in `[(params.stake_asset, stake, slashed),
   (USD, rewards, 0)]`, in that order: `paid = permille_of(own,
   params.reporter_permille)`; `mint(sender, asset, paid)`; if `own +
   pooled − paid > 0`, defer `Treasury { asset, amount: own + pooled −
   paid }`. The reporter's share is of the offender's own stake and
   rewards, never of its pool: a share of the pool would let an operator
   whose pool is large enough against its own stake profit from reporting
   itself through another account, since header evidence never expires.
6. Event `ValidatorSlashed { validator: offender, amount: stake, reporter:
   sender, reward: <paid of the stake> }`; then, if the offender had a
   pool, event `PoolSlashed { validator: offender, amount: slashed }`.

Evidence is checked against itself, not against this chain or a time: the
two messages must agree on `chain`, but nothing requires it to be this
chain's identity, and header evidence does not expire (approvals evidence
lapses with the transactions' `expires_at`). An offence is an offence
wherever it was signed, so an operator MUST NOT use a validator's keys on
more than one network. A validator that has withdrawn has nothing left to
slash (step 2).

### 5.31 OpenStream { payee, asset, unit, length, anchor, deposit, until, checkpoints }

A **stream** lets the sender, its payer, pay `payee` in steps of `unit`
without a transaction per payment. The payer keeps a secret hash chain whose
last element is `anchor`: with `step(x) = H("sion/stream/step", x)` for a
32-byte `x`, and `stepᵏ` for `k` applications of `step`, the chain is
`w_length, …, w_1, w_0 = anchor` with `w_{i−1} = step(w_i)`. The payer
reveals `w_1, w_2, …` in order, off the ledger; a payee holding `w_i` holds
`i` steps. Checking a revealed element against the last one costs one hash.
The payee settles from time to time (§5.32). Constants of
`sion_exec::stream`: `MAX_STREAM_LENGTH` (2³²), `MAX_SETTLE_STEPS` (65,536),
`STREAM_STEP_FEE` (`Amount(2)`, 2 micro-units of USD per hash a settlement
costs), `MAX_CHECKPOINTS` (2²⁰), `MAX_CHECKPOINT_DEPTH` (20) and
`CHECKPOINT_TAG` (the bytes `sion/stream/checkpoint`).

**Checkpoints** (optional). `checkpoints = Some(Checkpoints { spacing, root
})` commits to every `spacing`-th element of the chain: for `j = 0, …,
⌊length / spacing⌋`, leaf `j` is `L_j = merkle::leaf_hash(CHECKPOINT_TAG ‖
be64(j) ‖ w_{j·spacing})` (`be64` the 8-byte big-endian encoding, `w` the 32
bytes of the element), and `root = merkle::root(L_0, …, L_m)`, the shared
Merkle tree hash (`sion_core::merkle`, RFC 9162 shape). They let the payee
settle any number of steps with a bounded number of hashes (§5.32). The
ledger cannot check that `root` commits to the payer's chain; a wrong root
only makes jumps fail, and the payee can always settle by walking.

1. `unit > 0`; `1 ≤ length ≤ MAX_STREAM_LENGTH`; `deposit > 0`;
   `ctx.pulse < until ≤ ctx.pulse + MAX_PERIOD` (the addition saturating;
   `MAX_PERIOD` as in §5.22). If `checkpoints` is `Some { spacing, root }`:
   `1 ≤ spacing ≤ length` and `⌊length / spacing⌋ < MAX_CHECKPOINTS`.
2. `draw(asset, deposit)`.
3. `stream_id = created_id(digest, next_index)`;
   `create(Shared(object_domain("stream", stream_id)), Stream { payer:
   sender, payee, asset, unit, length, position: 0, head: anchor, escrow:
   deposit, until, open: true, spacing, checkpoints: root })`, with
   `spacing = 0` and `checkpoints = Hash32::ZERO` when `checkpoints` is
   `None`.
4. Event `StreamOpened { stream: stream_id, payer: sender, payee, deposit }`.

### 5.32 SettleStream { stream, position, value, jump }

1. Load `stream` (MUST be a stream); it MUST be `open`; the sender MUST be
   its `payee`; `ctx.pulse ≤ until`.
2. `stream.position < position ≤ length`; `steps = position −
   stream.position`.
3. The hashes the check costs:
   - `jump = None` (a **walk** from the head): `steps ≤ MAX_SETTLE_STEPS`;
     `hashes = steps`.
   - `jump = Some(Jump { index, path })` (a **jump** to a checkpoint):
     `stream.spacing > 0`; `|path| ≤ MAX_CHECKPOINT_DEPTH`; `base = index ×
     spacing` (checked: an overflow fails); `base ≤ position < base +
     spacing`; `rest = position − base`; `hashes = rest + 1 + |path|`.
4. `tx.fee ≥ required_fee(signed) + hashes × STREAM_STEP_FEE` (§4.3; the
   addition and multiplication saturating).
5. The element MUST be genuine:
   - walk: `stepˢᵗᵉᵖˢ(value) = head`;
   - jump: with `x = stepʳᵉˢᵗ(value)`, `merkle::verify_inclusion(checkpoints,
     ⌊length / spacing⌋ + 1, index, merkle::leaf_hash(CHECKPOINT_TAG ‖
     be64(index) ‖ x), path)` MUST succeed.
6. `amount = unit × steps` (checked: an overflow fails); `amount ≤ escrow`.
7. `escrow −= amount`; `position = position`; `head = value`; store the
   stream; `mint(sender, asset, amount)`.
8. Event `StreamSettled { stream, position, head: value, amount }`.

A jump proves `value` is the chain element at `position` without walking to
the head, so its cost does not depend on `steps`: at most `spacing + 21`
hashes, however many payments the settlement covers. A jump's checkpoint may
lie below the settled position.

Anyone holding an element `v` that a payer revealed at position `p` can
check that it was paid once a settlement reaches a position `c ≥ p`:
`stepᶜ⁻ᵖ(head after that settlement) = v`, or, with the jump for checkpoint
`⌊p / spacing⌋`, by step 5's jump check.

### 5.33 TopUpStream { stream, deposit, until }

1. Load `stream` (MUST be a stream); it MUST be `open`; the sender MUST be
   its `payer`; `ctx.pulse ≤ stream.until`.
2. `stream.until ≤ until ≤ ctx.pulse + MAX_PERIOD` (saturating); `deposit
   > 0` or `until > stream.until`.
3. If `deposit > 0`: `draw(asset, deposit)` and `escrow += deposit`.
4. `stream.until = until`; store the stream; event `StreamToppedUp {
   stream, deposit, until }`.

### 5.34 CloseStream { stream }

1. Load `stream` (MUST be a stream); it MUST be `open`. The sender MUST be
   its `payee`, or its `payer` once `ctx.pulse > until`.
2. `refund = escrow`; `escrow = 0`; `open = false`; store the stream;
   `mint(payer, asset, refund)`.
3. Event `StreamClosed { stream, refund }`.

A stream is never deleted: once closed it keeps its last `position` and
`head`, so items it paid stay checkable.

### 5.35 RegisterAgent { agent, card, caveats, funding, agent_bond }

Registers an agent identity: a charter whose `identity` is set. Exactly
§5.12 CreateCharter, with two differences: the charter is created with
`identity: Some(AgentIdentity { card, depth: 0 })`, and the event is
`AgentRegistered { agent: charter_id, account: agent, allotment }` instead
of `CharterCreated`. The charter's id is the agent's id (`did:sion:` and
the id's 64 hex digits, in the off-ledger card).

### 5.36 SpawnAgents { charter, children }

An agent launches subagents, each with its own identity, account, caveats
and budget. A child's `agent` is the account that acts for it, or `None`
for a **keyless** subagent (v1.13), whose account is derived from its id
and which acts only through batches its nearest keyed ancestor signs
(§4.3).

1. `1 ≤ |children| ≤ MAX_SPAWN_BATCH` (256,
   `sion_exec::charter::MAX_SPAWN_BATCH`).
2. `allotment_id = tx.inputs[1].id` (error if absent); load it (MUST be an
   allotment).
3. Load `charter` (MUST be a charter, call it `p`); **standing** of it,
   yielding `d` and `N`; `d < MAX_LINEAGE_DEPTH` ("too deep").
4. The allotment's `source` MUST be `charter`; its `writer` and `p.agent`
   MUST be the sender.
5. The allotment MUST have caveats `k`; **may_spawn**(k, N).
6. **refill**(its bucket). For each child, in order:
   `attenuates(child.caveats, k)`; **born**(child.caveats); then
   **carve**(its bucket, `child.caveats.burst`, `child.caveats.rate`,
   `child.caveats.burst`); then `remaining −= child.budget` (error if
   insufficient).
7. `first = spawned`; `spawned += |children|`; store the allotment.
8. For each child, the `j`-th (from 0), in order:
   1. `child_id = lineage_id(charter, first + j)`; `account =
      child.agent`, or `keyless_account(child_id)` if `child.agent = None`;
      `create_at(child_id, Shared(object_domain("charter", child_id)),
      Charter { principal: sender, agent: account, caveats: child.caveats,
      agent_atom: p.agent_atom, backer: p.backer, parent: Some(charter),
      lineage: p.lineage.child(first + j), allotments: ∅, created_at:
      ctx.pulse, identity: Some(AgentIdentity { card: child.card, depth:
      d + 1 }) })`.
   2. `a = create(Account(account), Allotment { source: child_id,
      writer: account, asset: USD, remaining: child.budget, caveats:
      Some(child.caveats), bucket: full(child.caveats), spawned: 0 })`.
   3. Load the child charter, insert `a` into `allotments`, store it.
   4. Event `AgentSpawned { agent: child_id, parent: charter, account,
      allotment: a }`.

Siblings share nothing they write: each spends from its own allotment, so
their payments need no consensus.

A suspension or revocation takes effect when it executes. An agent's
payment certified before then, but executed after, fails: for agent
operations a lane certificate guarantees that the inputs are locked, not
that the operation will succeed.

### 5.37 SetAgentCard { charter, card }

Contended, in the charter's domain. Load `charter` (MUST be a charter with
`identity = Some(i)`); the sender MUST be its `agent`; **standing** of it;
`i.card = card`; store; event `AgentCardSet { agent: charter, card }`.

### 5.38 SuspendAgent { charter }

Contended, in the charter's domain. Load it (MUST be a charter); the
sender MUST be an **overseer in good standing** (§5.13), at position `r`.
`o = overlay(charter)`; if `o.suspended_by` is unset, set it to the
sender; if it is `Some(p)`, `p` has a position `q = position(charter, p)`
and `r < q`, set it to the sender (v1.14): the highest suspender is kept,
so an overseer below it cannot lift its suspension (§5.39). Store `o`;
event `AgentSuspended { agent: charter, by: o.suspended_by }`. Until the
suspension is lifted, the standing check of the charter and of every
charter below it fails, so nothing in the subtree can pay or spawn.

### 5.39 ResumeAgent { charter }

Contended, in the charter's domain. Load it (MUST be a charter);
`try_load(overlay_id(charter))` MUST exist (an overlay `o`) with
`o.suspended_by = Some(s)` ("not suspended" otherwise); the sender MUST be
`s` or the charter's `backer`, and an **overseer in good standing**
(§5.13); set `o.suspended_by = None`; store `o`; event `AgentResumed {
agent: charter }`.

### 5.40 OpenMemory { agent, policy }

A **memory log** commits its writer to a log of memory atoms that only the
writer appends to, kept off the ledger. The ledger keeps the log's head:
its `size` in atoms, its `root`, the Merkle tree hash of `sion_core::merkle`
(RFC 9162 shape) over leaf hashes the writer computes, and two roots it
stores without checking, `index` and `manifest`. Constant of
`sion_exec::memory`: `MAX_CONSISTENCY_PROOF` (128).

1. If `agent = Some(a)`: load `a` (MUST be a charter with `identity` set);
   its `agent` MUST be the sender; **standing** of it.
2. `log = created_id(digest, next_index)`; `create(Account(sender),
   MemoryLog { writer: sender, agent, policy, size: 0, root:
   merkle::empty_root(), index: Hash32::ZERO, manifest: Hash32::ZERO,
   commits: 0, opened_at: ctx.pulse, committed_at: ctx.pulse, sealed: false
   })`.
3. Event `MemoryOpened { log, account: sender, agent }`.

The log is owned by its writer, so a commit (§5.41) needs a lane
certificate, not consensus, and two commits of the same version of a log
can never both execute: a log cannot fork.

### 5.41 CommitMemory { size, root, index, manifest, proof, seal }

1. `log_id = tx.inputs[1].id` (error if absent); load it (MUST be a memory
   log, call it `l`); `l.writer` MUST be the sender; `l.sealed` MUST be
   false.
2. If `l.agent = Some(a)`: load `a` (MUST be a charter); **standing** of
   it.
3. `|proof| ≤ MAX_CONSISTENCY_PROOF`; `size ≥ l.size`; if `size = 0`, `root
   = merkle::empty_root()`; `merkle::verify_consistency(l.root, l.size,
   root, size, proof)` MUST succeed.
4. `l.size = size`; `l.root = root`; `l.index = index`; `l.manifest =
   manifest`; `l.commits = l.commits + 1` (saturating); `l.committed_at =
   ctx.pulse`; if `seal`, `l.sealed = true`; store the log.
5. Event `MemoryCommitted { log: log_id, size, root, commits: l.commits,
   sealed: l.sealed }`.

A commit at the same size MUST keep the root (step 3), and may move
`index` and `manifest`. The atoms at positions `l.size … size − 1` before
the commit have `ctx.pulse` as their **transaction time**: the ledger, not
the writer, says when they were recorded. Anyone can prove an atom's
transaction time from two state proofs of the log against certified
headers: at Pulse `p − 1` the log did not exist or its size did not cover
the atom, and at `p` it did.

The ledger does not check `index` or `manifest`. Completeness proofs for
queries are relative to the committed `index`; that the index matches the
log is a trust atom (`impl:memory-index`). The index a writer commits is
meant to be the canonical index of the log's first `size` atoms, version 1,
specified in the documentation of `sion_memory::index` (a client
specification, not part of this state transition): a reader holding the
atoms can rebuild it and compare roots, and anyone holding an atom it
leaves out can prove the omission from the head alone
(`sion_memory::verify_omission`). Nothing on the ledger acts on such a
proof yet.

### 5.42 PostTask { spec, acceptance, worker, amount, deadline, charter }

A **task** escrows `amount` for work whose result must pass an acceptance
rule fixed now, when the task is posted. Owned-linear, or bounded with
`charter`; the task is a shared object in its own domain. Constants of
`sion_exec::task`:
`RESULT_SCHEMA` (`task.result/v1`), `ATTESTATION_SCHEMA`
(`task.attestation/v1`), `WORKER_BOND_LABEL` (`task-worker`),
`MAX_REQUIRED` (16) and `MAX_VERIFIERS` (16).

1. `amount > 0`; `ctx.pulse < deadline ≤ ctx.pulse + MAX_PERIOD`
   (saturating); `|acceptance.requires| ≤ MAX_REQUIRED`;
   `|acceptance.verifiers| ≤ MAX_VERIFIERS`;
   `acceptance.detection_permille ≤ 1000`; `1 ≤ acceptance.reliance ≤
   MAX_PERIOD`; `acceptance.must_attest ⊆ acceptance.verifiers`;
   `acceptance.min_attestations ≤ |acceptance.verifiers|`; if `worker =
   Some(w)`, `w ∉ acceptance.verifiers` (v1.15: nobody attests its own
   result).
2. Funding:
   - `charter = None`: `draw(USD, amount)`; `funder = None`; `refund_to =
     sender`.
   - `charter = Some(c)`: `allotment_id = tx.inputs[1].id` (error if
     absent); load it (MUST be an allotment), then load `c` (MUST be a
     charter); **standing** of `c`, yielding `N`; the allotment's `source`
     MUST be `c`, and its `writer` and `c.agent` MUST be the sender; it MUST
     have caveats `k`. If `worker = None`, `k.payees` MUST be `None`, and so
     MUST every `n.payees` for `n ∈ N`: an agent limited to listed payees
     cannot post work anyone may take.
     `evidence_ok = (acceptance.policy = k.evidence_policy)`;
     **spend**(allotment, `worker` or, for an open task, the sender,
     `amount`, `Some(evidence_ok)`, N) (§5.13), so the escrow counts against
     the agent's balance, payees, per-payment limits and bucket, and a task
     above the evidence threshold must be accepted under the charter's own
     policy; store the allotment; `funder = Some(c)`; `refund_to =
     c.backer`.
3. `task = created_id(digest, next_index)`; `create(Shared(object_domain(
   "task", task)), Task { poster: sender, funder, refund_to, spec,
   acceptance, worker, amount, deadline, posted_at: ctx.pulse, state: Open
   })`.
4. Event `TaskPosted { task, poster: sender, worker, amount, deadline }`.

### 5.43 SettleTask { task, result, attestations }

Contended, in the assurance domain. `result` is the body of the sender's
result claim; `attestations` lists `(verifier, body)` pairs.

1. Load `task` (MUST be a task, call it `t`); `t.state = Open`;
   `ctx.pulse ≤ t.deadline`; if `t.worker = Some(w)`, the sender MUST be
   `w` (an open task may be settled by anyone).
2. `r = claim_id(sender, result)`. Load the assurance object; the registry
   MUST hold `r` with status Active.
3. `result.schema = RESULT_SCHEMA`; `result.payload = Public(bytes)` with
   `decode::<TaskResult>(bytes) = TaskResult { task, output }` for this
   `task` and some `output`.
4. `|attestations| ≤ MAX_VERIFIERS`; their verifiers are distinct, and
   none is the sender (v1.15: a result is never attested by its author).
   For each `(v, body)`, in order: `v ∈ t.acceptance.verifiers`; `a =
   claim_id(v, body)` is Active in the registry; `body.schema =
   ATTESTATION_SCHEMA`; `body.payload = result.payload`. Then (v1.15):
   every verifier in `t.acceptance.must_attest` is among them, and
   `|attestations| ≥ t.acceptance.min_attestations`.
5. The result's formula MUST be exactly
   `Formula::and([dual_signature(), Formula::or(alternatives),
   Formula::claim(q₁), …, Formula::claim(qⱼ)])`, where `alternatives` is
   `Formula::atom(bond_atom(Economic, sender, WORKER_BOND_LABEL).id)`
   followed by `Formula::claim(a)` for each attestation in the order
   listed, and `q₁ < … < qⱼ` are
   `t.acceptance.requires`. The registry's record of `r` MUST name
   `t.acceptance.adjudicator` as its adjudicator, and
   `result.detection_permille ≤ t.acceptance.detection_permille`.
6. `(decision, reservation) = accept_and_reserve(&mut registry,
   t.acceptance.policy, t.refund_to, r, t.amount, ctx.pulse +
   t.acceptance.reliance (saturating), nonce)`, where `nonce` is the
   first 8 bytes of `task`'s identifier read as a big-endian integer;
   `reservation = None` is an error ("not accepted").
7. Store the assurance object; defer `Schedule { at: ctx.pulse +
   reliance, event: RegistryTick }`.
8. `mint(sender, USD, t.amount)`; `t.state = Settled { worker: sender,
   result: r, reservation, at: ctx.pulse }`; store the task.
9. Event `TaskSettled { task, worker: sender, result: r, amount, reservation }`.

Step 5 fixes what a result rests on: the worker's own task bond, or any
named verifier's attestation of the same output, and every required
claim. The attestations step 4 requires sit in the same *or* as the
worker's bond, so their backings add up as before; requiring them makes
the presence of an independent check a condition of payment, which no
bond, however large, can stand in for. A worker therefore needs a task bond, however small, to issue a
result at all (a claim's atoms must be registered, §5.4). A worker cannot make a result look harder to fake by citing
unrelated claims, and cannot leave out the environment's. Because the
reservation is made for `t.refund_to`, if a required claim is later
proven false (an exploit in the environment, say), the result falls with
it, and the party whose money paid for it is compensated from the slashed
bonds the reservation holds (§5.10).

A task is paid at most once: settling it closes it.

### 5.44 CancelTask { task }

Contended, in the task's domain.

1. Load `task` (MUST be a task, `t`); `t.state = Open`.
2. The sender MUST be `t.worker` (a worker releases its task), or `t.poster`
   or `t.refund_to` when `t.worker = None` (an open task) or `ctx.pulse >
   t.deadline` (an expired one). A worker named by a task can therefore
   count on it until the deadline, whoever posted it.
3. `mint(t.refund_to, USD, t.amount)`; `t.state = Cancelled { by: sender,
   at: ctx.pulse }`; store.
4. Event `TaskCancelled { task, refund: t.amount, to: t.refund_to }`.

### 5.45 DelegateStake { validator, amount }

**Pooled stake.** Anyone may add stake to a validator's weight without
handing it over. A **delegation** is an object its delegator owns, holding
**shares** of the validator's **pool**; the pool holds the stake. Pools
live in the **delegation book**, a shared object at
`well_known::delegations()` owned by `Shared(validators_domain())`; the
first `DelegateStake` or `SetCommission` creates it. All five delegation
kinds are contended, in the validators' domain. Constants of
`sion_exec::delegation`: `REWARD_SCALE` (10¹²), `MIN_DELEGATION`
(`Amount(1_000_000)`), `COMMISSION_CEILING_PERMILLE` (250),
`COMMISSION_STEP_PERMILLE` (20), `DELEGATION_SLASH_PERMILLE` (100) and
`CAP_FLOOR_PERMILLE` (100).

**Arithmetic**, in `u128`, every step an error on overflow or division
by zero: `mul_div(a, b, c) = (a / c) × b + (a mod c) × b / c` with integer
division (equal to `⌊a·b/c⌋`). For a pool `p`: `shares(p) = earning +
pending + unbonding` (saturating); `value_of(p, s) = 0` if `shares(p) =
0`, else `mul_div(s, value, shares(p))`; the shares `amount` buys are
`amount` if `shares(p) = 0`, else `mul_div(amount, shares(p), value)`. A
validator's **bonded stake** is its record's `stake` plus, if it has a
pool, `value_of(p, earning + pending)` (`earning + pending` saturating).

1. `amount ≥ MIN_DELEGATION`. Load the validator set; `validator` MUST be
   a record with status `Joining` or `Active`.
2. `draw(params.stake_asset, amount)`.
3. `try_load(well_known::delegations())`: if it exists it MUST be a
   delegation book; otherwise start an empty one.
4. **Cap.** With `n` the number of records whose status is `Joining` or
   `Active`, `T` the sum of their bonded stakes and `B` the bonded stake
   of `validator`: `(B + amount) × 1000 ≤ cap × (T + amount)`, where `cap =
   max(⌊2000 / n⌋, CAP_FLOOR_PERMILLE)`. No validator may gain, by
   delegation, more than twice an equal share of the bonded stake, and the
   cap is never below a tenth.
5. Let `p` be `validator`'s pool (a default pool, all zero, if it has
   none) and `s` the shares `amount` buys; `s > 0`. `p.pending += s`;
   `p.pending_delegations += 1`; `p.value += amount`.
6. `d = create(Account(sender), Delegation { owner: sender, validator,
   shares: s, state: Pending { epoch: set.epoch }, since: ctx.pulse })`;
   store the book (with `p` in it).
7. Event `Delegated { delegation: d, validator, owner: sender, amount,
   shares: s }`.

A delegation made during an epoch earns, and counts towards its
validator's weight, from that epoch's end (§6.3): delegating just before
an epoch ends earns nothing for that epoch.

**Resolving** a delegation `d` of pool `p`, with `E` the validator set's
current `epoch`:

- `Pending { epoch: e }` with `e < E`: it joined at the end of epoch `e`.
  `j = p.joins[e]` (an error if absent); `j.waiting −= 1` (an error if it
  was 0), and the entry is removed if `waiting` is now 0; `d.state =
  Earning { entry: j.reward_per_share }`. It resolves as **earning** from
  that entry.
- `Pending { epoch: E }`: it resolves as **pending**.
- `Earning { entry }`: it resolves as **earning** from `entry`.
- `Unbonding`: an error.

An earning delegation of `s` shares from `entry` is **owed**
`mul_div(s, p.reward_per_share − entry, REWARD_SCALE)` (an error if
`entry > p.reward_per_share`).

### 5.46 Undelegate

`tx.inputs[1]` is the delegation.

1. Load `tx.inputs[1].id` (an error if absent; it MUST be a delegation
   `d`); `d.owner` MUST be the sender. Load the validator set. Load
   `well_known::delegations()` (it MUST exist and be a delegation book)
   and take `d.validator`'s pool `p` (it MUST exist).
2. Resolve `d`. Pending: `p.pending −= d.shares`; `p.pending_delegations
   −= 1`; `owed = 0`. Earning from `entry`: `owed` as above; `p.earning −=
   d.shares`; `p.rewards_held −= owed`. Then `p.unbonding += d.shares`;
   `d.state = Unbonding { at: ctx.pulse + params.unbonding }`
   (saturating).
3. Store `d`, then the book; `mint(sender, USD, owed)`.
4. Event `Undelegated { delegation: d, rewards: owed, withdrawable_at:
   <at> }`.

An unbonding delegation earns nothing and no longer counts towards its
validator's weight from the next epoch's end, but its stake stays in the
pool, where it is slashed with the pool (§5.30), until it is withdrawn.

### 5.47 WithdrawDelegation

1. Load `tx.inputs[1].id` (an error if absent; it MUST be a delegation
   `d`); `d.owner` MUST be the sender; `d.state` MUST be `Unbonding { at
   }` with `at ≤ ctx.pulse`.
2. Load the validator set; load the delegation book (it MUST exist) and
   take `d.validator`'s pool `p` (it MUST exist).
3. `amount = value_of(p, d.shares)`; `p.unbonding −= d.shares`; `p.value
   −= amount`; store the book.
4. `consume(d)`; `mint(sender, params.stake_asset, amount)`.
5. Event `DelegationWithdrawn { delegation: d, amount }`.

### 5.48 ClaimDelegatorRewards

1. Load `tx.inputs[1].id` (an error if absent; it MUST be a delegation
   `d`); `d.owner` MUST be the sender. Load the validator set; load the
   delegation book (it MUST exist) and take `d.validator`'s pool `p` (it
   MUST exist).
2. Resolve `d`; it MUST resolve as earning, from `entry`, and be owed
   `owed > 0`.
3. `p.rewards_held −= owed`; `d.state = Earning { entry:
   p.reward_per_share }`; store `d`, then the book; `mint(sender, USD,
   owed)`.
4. Event `DelegatorRewardsClaimed { delegation: d, amount: owed }`.

### 5.49 SetCommission { validator, permille }

1. Load the validator set; `validator` MUST be operated by the sender
   (§5.26); `permille ≤ COMMISSION_CEILING_PERMILLE`.
2. `try_load(well_known::delegations())` (if it exists it MUST be a
   delegation book; otherwise start an empty one); take `validator`'s
   pool `p` (a default pool if it has none). If `shares(p) > 0`, `permille
   ≤ p.commission_permille + COMMISSION_STEP_PERMILLE`.
3. `p.next_commission = Some(permille)`; store the book.
4. Event `CommissionSet { validator, permille }`.

The commission takes effect at the epoch's end (§6.3), after that epoch's
rewards, so delegators see every change at least an epoch before it costs
them anything, and a pool with delegators sees its commission rise by at
most 2 percentage points per epoch. A pool nobody has delegated to yet may
set any commission up to the ceiling.

### 5.50 PostDeal { root, size, challenges, price, rounds, round_length, collateral, provider, accept_by, locator }

A **storage deal** pays a provider, round by round, for keeping a body of
data whose Merkle root is fixed when the deal is posted, and for proving in
each round that it still holds pieces of it that nobody could predict.
The data is `size` **leaves**, byte strings of at most `MAX_LEAF_BYTES`
bytes each, and `root = merkle::root([leaf_hash(d₀), …, leaf_hash(d_{size−1})])`
(`sion_core::merkle`, RFC 9162 shape). The ledger does not store the
data: `locator` only tells providers where to fetch it. Proofs do publish
the leaves they answer with (§5.52), so data that must stay private
should be encrypted before its root is computed.

Money stays in the deal, a shared object in its own domain
(`object_domain("deal", deal)`): the client's escrow, and the provider's
collateral once it accepts. Proofs go to a **custody record** the provider
owns (§5.51), so proving is owned-linear: it needs a lane certificate, not
consensus, and one transaction can prove many deals (§5.52). Money moves
only when a deal is settled or closed (§5.53, §5.54).

Owned-linear. Constants of `sion_exec::storage`: `MAX_DEAL_LEAVES` (2⁴⁰),
`MAX_LEAF_BYTES` (4,096), `MAX_PATH` (40), `MAX_CHALLENGES` (16),
`MAX_ROUNDS` (1,000,000), `MIN_ROUND_LENGTH` (8), `MAX_ROUND_LENGTH`
(100,000), `MAX_LOCATOR` (256), `MAX_PROOF_LEAVES` (64),
`ABANDON_ROUNDS` (3), `SEED_DOMAIN` (`sion/storage/seed`) and
`CHALLENGE_DOMAIN` (`sion/storage/challenge`); and of `sion_exec::beacons`
(§6.3): `BEACON_INTERVAL` (8) and `BEACON_SLOTS` (12,500).

1. `1 ≤ size ≤ MAX_DEAL_LEAVES`; `1 ≤ challenges ≤ MAX_CHALLENGES`; `price
   > 0`; `1 ≤ rounds ≤ MAX_ROUNDS`; `MIN_ROUND_LENGTH ≤ round_length ≤
   MAX_ROUND_LENGTH` and `round_length mod BEACON_INTERVAL = 0`; if
   `provider = None`, `collateral > 0`; `ctx.pulse < accept_by ≤ ctx.pulse
   + MAX_PERIOD` (saturating; `sion_exec::market::MAX_PERIOD`); `locator`
   is at most `MAX_LOCATOR` bytes long (UTF-8).
2. `escrow = price × rounds` (an error if it overflows); `draw(USD,
   escrow)`.
3. `deal = created_id(digest, next_index)`; `create(Shared(object_domain(
   "deal", deal)), Deal { client: sender, provider, root, size, challenges,
   price, rounds, round_length, collateral, accept_by, locator, escrow,
   posted_at: ctx.pulse, state: Open })`.
4. Event `DealPosted { deal, client: sender, provider, size, price,
   rounds, collateral }`.

`provider = None` posts an **open** deal: any account may accept it, so
it must ask for collateral; without any, a stranger could accept it, never
prove a round, and hold the client's escrow until the deal is abandoned. A
provider should accept only once it holds data whose leaves hash to
`root`: it can check that before it accepts, and from then on its
collateral is at stake.

### 5.51 AcceptDeal { deal }

Contended, in the deal's domain.

1. Load `deal` (MUST be a deal, `d`); `d.state = Open`; `ctx.pulse ≤
   d.accept_by`; if `d.provider = Some(p)`, the sender MUST be `p`.
2. `draw(USD, d.collateral)`.
3. `start = BEACON_INTERVAL × (⌊ctx.pulse / BEACON_INTERVAL⌋ + 1)`
   (saturating): the first multiple of `BEACON_INTERVAL` after this Pulse.
4. `custody = create(Account(sender), Custody { deal, root: d.root, size:
   d.size, challenges: d.challenges, start, round_length: d.round_length,
   rounds: d.rounds, next: 0, proven: 0 })`.
5. `d.state = Active { provider: sender, custody, start, bond:
   d.collateral, paid: 0 }`; store `d`.
6. Event `DealAccepted { deal, provider: sender, custody, start }`.

Round `i` (from 0) of the deal is the Pulses `start + i × round_length` to
`start + (i + 1) × round_length − 1`; the deal **ends** at `end = start +
rounds × round_length` (saturating), the first Pulse after its last round.
Since `start` and `round_length` are multiples of `BEACON_INTERVAL`, so is
the first Pulse of every round, and the chain records its beacon (§6.3).

### 5.52 ProveStorage { proofs }

Owned-linear. `proofs[j]` is a `CustodyProof { round, leaves }` for the
custody record `tx.inputs[j + 1]`, and `leaves[k]` is a `LeafProof { data,
path }`.

1. `|proofs| ≥ 1`; `|tx.inputs| > |proofs|`; `Σ |proofs[j].leaves| ≤
   MAX_PROOF_LEAVES`.
2. For each `j`, in order, with `id = tx.inputs[j + 1].id` and `p =
   proofs[j]`: load `id` (MUST be a custody record, `c`; prepare has
   checked that the sender owns it). With `i = p.round`: `ctx.pulse ≥
   c.start`; `i = (ctx.pulse − c.start) / c.round_length` (the round under
   way: a round is proven during the round); `i < c.rounds`; `i ≥ c.next`;
   `|p.leaves| = c.challenges`. With `b = c.start + i × c.round_length`,
   the round's first Pulse, `seed = hash_parts(SEED_DOMAIN, [beacon_at(b),
   c.deal])`, each part its 32 bytes. For each `k` from 0 to
   `c.challenges − 1`:
   - `index = challenge(seed, i, k) mod c.size`, where `challenge(seed,
     i, k)` is the first 8 bytes, read big-endian, of
     `hash_parts(CHALLENGE_DOMAIN, [seed (32 bytes), i (4 bytes,
     big-endian), k (1 byte)])`;
   - with `(data, path) = p.leaves[k]`: `|data| ≤ MAX_LEAF_BYTES`; `|path|
     ≤ MAX_PATH`; `merkle::verify_inclusion(c.root, c.size, index,
     leaf_hash(data), path)` MUST succeed.

   Then `c.proven += 1`; `c.next = i + 1`; store `c`; event
   `StorageProven { deal: c.deal, custody: id, round: i }`.

**beacon_at(b)**, for a Pulse `b ≤ ctx.pulse` with `b mod
BEACON_INTERVAL = 0`: if `b = ctx.pulse`, `ctx.beacon`; otherwise `load(
well_known::beacon((b / BEACON_INTERVAL) mod BEACON_SLOTS))`, which MUST
be a beacon record with `pulse = b`, and its `beacon`. Every round's first
Pulse is a multiple of `BEACON_INTERVAL`, and a proof executes less than
`MAX_ROUND_LENGTH = BEACON_INTERVAL × BEACON_SLOTS` Pulses after it, so the
record is still there.

A round's challenge is fixed by the beacon of its first Pulse, a Pulse
fixed when the deal was accepted. A provider learns it only when that Pulse
executes, so it must hold every leaf until then, and it cannot change the
challenge by choosing when its proof executes. (Before this was settled,
each proof fixed the next round's challenge through the beacon of the Pulse
it executed in; a provider that could time its proofs, with a validator's
help, could steer its challenges towards the leaves it kept.) What remains
is the beacon's own bias: a validator that withholds its reveal for the
round's first Pulse chooses between two beacons, and `m` colluding
validators among `2^m`. A round passes unproven if no proof of it executes
during it. A custody record does not know whether its deal is still
active: proofs after a deal is closed change nothing but the record.

### 5.53 SettleDeal { deal }

Contended, in the deal's domain.

1. Load `deal` (MUST be a deal, `d`); `d.state = Active { provider,
   custody, … , paid }`; the sender MUST be `provider`.
2. Load `custody` (MUST be a custody record, `c`, with `c.deal = deal`).
3. `due = c.proven − paid` (MUST be positive); `amount = due × d.price`
   (an error if it overflows); `d.escrow −= amount` (an error if it would
   be negative); `paid = c.proven`; store `d`; `mint(provider, USD,
   amount)`.
4. Event `DealSettled { deal, rounds: due, amount }`.

### 5.54 CloseDeal { deal }

Contended, in the deal's domain.

1. Load `deal` (MUST be a deal, `d`).
2. If `d.state = Open`: the sender MUST be `d.client`; `proven = 0`,
   `missed = 0`, `paid_now = 0`, `returned = 0`, `forfeited = 0`.
3. If `d.state = Active { provider, custody, start, bond, paid }`: load
   `custody` (MUST be a custody record, `c`, with `c.deal = deal`);
   `proven = c.proven`; `end = start + d.rounds × d.round_length`
   (saturating).
   - **At the end**, `ctx.pulse ≥ end`: the sender MUST be `d.client` or
     `provider`; `missed = d.rounds − proven`; `forfeited = ⌊bond × missed
     / d.rounds⌋`, computed exactly in `u128` as `mul_div(bond, missed,
     d.rounds)` (§5.45; at most `bond`, since `missed ≤ d.rounds`);
     `returned = bond − forfeited`.
   - **Abandoned**, `ctx.pulse < end`: the sender MUST be `d.client`;
     `ctx.pulse ≥ start`; with `r = (ctx.pulse − start) / d.round_length`,
     `r ≥ c.next + ABANDON_ROUNDS` (the provider proved none of the last
     `ABANDON_ROUNDS` rounds or more); `missed = r − proven`; `forfeited =
     bond`; `returned = 0`.

   Then `due = proven − paid`; `paid_now = due × d.price` (an error if it
   overflows); `d.escrow −= paid_now` (an error if it would be negative);
   `mint(provider, USD, paid_now + returned)`.
4. Otherwise (`Closed`): an error.
5. `refund = d.escrow`; `mint(d.client, USD, refund)`; if `forfeited >
   0`, defer `Treasury { asset: USD, amount: forfeited }`.
6. `d.escrow = 0`; `d.state = Closed { at: ctx.pulse, proven, missed,
   refund, forfeited }`; store `d`.
7. Event `DealClosed { deal, refund, paid: paid_now, returned, forfeited,
   missed }`.

The provider is paid for every round it proved, whoever closes. A
provider that proves every round gets its whole collateral back; the
rounds it missed cost it their share of it, `bond × missed / rounds`
rounded down once (rounding each round's share down instead would let a
collateral smaller than the number of rounds escape entirely); one that
stops proving for
`ABANDON_ROUNDS` rounds may lose all of it as soon as the client closes.
Forfeited collateral goes to the treasury, not to the client, so that
nobody profits from making a provider fail (by flooding its network, say).
The client gets back the escrow of every round not proven.

### 5.55 SpawnCohort { charter, root, count, burst, rate, full, caveats }

A **cohort** commits to up to `MAX_COHORT` (2²⁰,
`sion_exec::cohort::MAX_COHORT`) subagents in one transaction, as the root
of a Merkle sum tree over its members. A member costs nothing on the
ledger until it is **materialized** (§5.56), when first needed. Bounded,
like SpawnAgents: it writes only the sender's allotment and creates the
cohort.

1. `1 ≤ count ≤ MAX_COHORT`.
2. `allotment_id = tx.inputs[1].id` (error if absent); load it (MUST be an
   allotment).
3. Load `charter` (MUST be a charter, call it `p`); **standing** of it,
   yielding `d` and `N`; `d < MAX_LINEAGE_DEPTH` ("too deep").
4. The allotment's `source` MUST be `charter`; its `writer` and `p.agent`
   MUST be the sender.
5. The allotment MUST have caveats `k`; **may_spawn**(k, N);
   `attenuates(caveats, k)`; **born**(caveats).
6. **refill**(its bucket); **carve**(its bucket, `burst`, `rate`, `burst`
   if `full`, else 0).
7. `first = spawned`; `spawned += count`; store the allotment.
8. `cohort = created_id(digest, next_index)`;
   `create(Shared(object_domain("cohort", cohort)), Cohort { parent:
   charter, first, count, root, burst, rate, full, caveats, created_at:
   ctx.pulse })`.
9. Event `CohortSpawned { cohort, parent: charter, first, count }`.

The cohort is never written again: materializing a member only reads it.
Member `i` will be `lineage_id(charter, first + i)`.

**The sum tree.** Members are `CohortMember { agent, card, budget, burst,
rate }`. The tree has height `h = ⌈log₂ count⌉` (0 for one member) and
`2^h` leaves: member `i` at leaf `i`, and `empty = (H("sion/cohort/empty",
""), 0, 0)` at the rest. A node is a `SumNode { hash, burst, rate }`: a
member's is `(H("sion/cohort/member", enc(member)), member.burst,
member.rate)`; the node over `l` and `r` is `(H("sion/cohort/node",
enc((l.hash, l.burst, l.rate, r.hash, r.burst, r.rate))), l.burst +
r.burst, l.rate + r.rate)`. The cohort's `root`, `burst` and `rate` are
the root node's. **verify(q, index, member, siblings)** for a cohort `q`:
`index < q.count` and `|siblings| = h`; then, starting from the member's
node, for each `j` from 0 to `h − 1`, join the node with `siblings[j]`,
the node on the left if bit `j` of `index` is 0 and on the right otherwise
(a sum overflowing fails); the last node MUST equal `(q.root, q.burst,
q.rate)`.

Every node on a path that verifies carries the sums that the node above
it commits to, so the members that can ever be materialized together carry
at most the `burst` and `rate` carved at step 6, whatever the tree's
creator put in the rest of it.

### 5.56 MaterializeAgents { cohort, members }

1. `1 ≤ |members| ≤ MAX_MATERIALIZE` (128,
   `sion_exec::cohort::MAX_MATERIALIZE`), with strictly increasing
   `index`es.
2. `allotment_id = tx.inputs[1].id` (error if absent); load it (MUST be an
   allotment).
3. Load `cohort` (MUST be a cohort, call it `q`); load `q.parent` (MUST be
   a charter, call it `p`); **standing** of it, yielding `d` and `N`.
4. The allotment's `source` MUST be `q.parent`; its `writer` and `p.agent`
   MUST be the sender.
5. The allotment MUST have caveats `k`; **may_spawn**(k, N);
   **born**(q.caveats).
6. For each member `m`, in order: **verify**(q, m.index, m.member,
   m.siblings) ("not in the cohort"); `m.member.burst ≤ q.caveats.burst`
   and `m.member.rate ≤ q.caveats.rate`; `id = lineage_id(q.parent,
   q.first + m.index)`; `try_load(id)` MUST find nothing ("already
   materialized"); `remaining −= m.member.budget` (error if insufficient).
7. Store the allotment.
8. For each member `m`, in order, with `id` as in step 6: `caveats =
   q.caveats` with `burst = m.member.burst` and `rate = m.member.rate`;
   `account = m.member.agent`, or `keyless_account(id)` if it is `None`;
   `create_at(id, Shared(object_domain("charter", id)), Charter {
   principal: sender, agent: account, caveats, agent_atom: p.agent_atom,
   backer: p.backer, parent: Some(q.parent), lineage: p.lineage.child(
   q.first + m.index), allotments: ∅, created_at: ctx.pulse, identity:
   Some(AgentIdentity { card: m.member.card, depth: d + 1 }) })`; `a =
   create(Account(account), Allotment { source: id, writer: account,
   asset: USD, remaining: m.member.budget, caveats: Some(caveats), bucket:
   Bucket { tokens: min(m.member.burst, start + m.member.rate × (ctx.pulse
   − q.created_at)) (saturating), refilled_at: ctx.pulse, burst:
   m.member.burst, rate: m.member.rate }, spawned: 0 })`, where `start =
   m.member.burst` if `q.full`, else 0; load the charter, insert `a`, store
   it; event `AgentMaterialized { agent: id, cohort, index: m.index,
   account, allotment: a }`.

A member's bucket is what it would hold had it existed since the cohort:
full if the parent gave up the tokens (`full`), otherwise filling at its
rate from the cohort's creation. Its budget comes from the parent's
allotment when it is materialized; a cohort escrows nothing, so a parent
whose allotment runs low cannot materialize more members, and a revoked,
suspended or expired parent none. Materializing reads the cohort and the new ids, and writes
only the sender's allotment and new objects: materializations of
different parents never contend.

### 5.57 NarrowAgent { charter, narrowing }

Contended, in the charter's domain. An overseer adds limits to an agent
and everything below it: every payment in the subtree is also checked
against them (§5.13 **spend**), and nothing in it spawns unless
`narrowing.may_delegate`.

1. Load `charter` (MUST be a charter, call it `c`); the sender MUST be its
   `agent` or an **overseer in good standing** (§5.13), the agent compared
   first.
2. `o = overlay(charter)`.
3. Unless the sender is `c.backer`: `narrowing` MUST be `Some(n)`, and if
   `o.narrowing = Some(prev)`, **narrows**(n, prev) ("only the backer may
   widen or lift a narrowing").
4. `o.narrowing = narrowing`; store `o`; event `AgentNarrowed { agent:
   charter, by: sender, narrowing }` (`None` when it was lifted).

A narrowing binds the subtree as soon as it executes; payments certified
before then but executed after are checked against it.

### 5.58 IndexClaim { key, claim }

An **index entry** names the latest claim its owner made under a key, such
as a publisher's latest release of a skill (v1.15). It moves only to a
later claim of the owner's own, so a reader who checks the entry with a
state proof against a certified header knows the owner's latest claim
under the key as of that header. A server can still answer from an older
header: how recent a header must be is for the reader to require.
Contended, in the assurance domain. Constant of `sion_exec::index`:
`MAX_INDEX_KEY` (64). `index_id(owner, key) = ObjectId(H("sion/id/index",
borsh((owner, key))))`.

1. `1 ≤ |key| ≤ MAX_INDEX_KEY` bytes, each from `0x21` to `0x7e`.
2. Load the assurance object; its registry MUST hold `claim` with status
   Active and issuer the sender (its record `c`).
3. `id = index_id(sender, key)`. If `try_load(id)` gives an object (MUST be
   an index entry `e`): the registry's record of `e.claim` MUST have a
   `sequence` below `c.sequence` ("an index entry moves only to a later
   claim"); `e.previous = Some(e.claim)`; `e.claim = claim`; `e.count =
   e.count + 1` (saturating); `e.updated_at = ctx.pulse`; store `e`.
   Otherwise `create_at(id, Shared(object_domain("index", id)), IndexEntry
   { owner: sender, key, claim, previous: None, count: 1, updated_at:
   ctx.pulse })`.
4. Event `ClaimIndexed { entry: id, owner: sender, key, claim, count }`
   (the entry's count after this transaction).

An entry is never removed. The claim it names may later expire or be
proven false: a reader checks the claim's status as it would any claim's.
A claim registers at the end of the Pulse it is issued in (§5.4), so it can
be indexed from the next Pulse on.

## 6. The Pulse

### 6.1 Order

A Pulse executes `PulseInput { pulse, beacon, owned, contended, forced }`
with `pulse = state.pulse + 1`. `forced` are the forced intents committed
in the Pulse: they are not executed, they are **posted** in the deferred
phase (§6.3), each digest once, in commit order
(`sion_exec::pulse::posted_intents`). The nodes compute a canonical order (owned
transactions by digest, in dependency passes, then contended transactions
in consensus order; see `sion_exec::pulse::canonical_order`). Proof stacks
do not recompute it: a chunk statement binds the transactions it executes
(`steps`), and the order is bound into the certified Pulse header. The
order decides only which dependent transactions find their inputs, never
what a transaction may do.

### 6.2 Phases and chunks

Transactions execute one after another (§4) in the given order, each seeing
the writes of those before it. The sequence is cut into chunks; the last
chunk of a Pulse is the **deferred phase** (§6.3), which executes no
transactions and consumes the concatenation of every transaction's
`effects.deferred`, in execution order.

### 6.3 The deferred phase

Bookkeeping is as in §3.1 (fresh `originals` and `created`), with a
counter `k = 0` for payouts: **pay(owner, asset, amount)**: if `amount =
0` do nothing; otherwise store a new coin with id `ObjectId::derive_parts(
[b"deferred", pulse as 8 big-endian bytes, k as 8 big-endian bytes])`
(`sion_exec::pulse::deferred_coin_id`) and increment `k`.

**Protocol objects.** The schedule object is always loaded first. The
assurance object, the treasury, the validator set and the forced-inclusion
obligations are loaded the first time something needs them (and only
then); a market is loaded the first time an item or event needs it; the
delegation book (v1.11) is read, with `try_load`, only by EpochEnd, and
may not exist. Each of these is written back at the end **only if**
something used it mutably (assurance, treasury, validator set, delegation
book, obligations, markets: anything that obtained it; schedule: any
change to its entries, including removing due entries). Write-back order:
markets in id order, then assurance, treasury, validator set, delegation
book, obligations, schedule, then the beacon record. Other objects (bounded
totals, coins) are stored when changed. Reads of other objects in this
phase are `try_load`s.

**The beacon record** (v1.12). If `pulse mod BEACON_INTERVAL = 0`, the
phase ends by recording the Pulse's beacon: with `slot = (pulse /
BEACON_INTERVAL) mod BEACON_SLOTS` and `id = well_known::beacon(slot)`,
`try_load(id)`; if it exists it MUST be a beacon record, and it is stored
with its data replaced by `Beacon(BeaconRecord { pulse, beacon })`;
otherwise a new object `Object { id, version: 0, owner:
Shared(well_known::beacons_domain()), data: Beacon(BeaconRecord { pulse,
beacon }) }` is stored. (Each slot is reused every `BEACON_INTERVAL ×
BEACON_SLOTS` Pulses; the beacons themselves stay in the certified Pulse
headers.) No other Pulse writes a beacon record.

**Sorting.** Items are sorted by the key `(rank, natural, enc(item))`,
where `natural` and `enc(item)` compare as byte strings (so Schedule items,
whose `enc(at)` is little-endian, do not sort numerically; scheduling
commutes, so their order does not matter):

| item | rank | natural key |
|---|---|---|
| RegisterAtom | 0 | `enc(atom.id)` |
| PostBond | 1 | `enc((atom.id, owner))` |
| IssueClaim | 2 | `enc(claim_id(issuer, body))` |
| AddOrder | 3 | `enc(order.id)` |
| ReturnToBounded | 4 | `enc(bounded)` |
| Treasury | 5 | `enc(asset)` |
| Minted | 6 | `enc(asset)` |
| Schedule | 7 | `enc((at, event))` |
| Fee | 8 | `enc(asset)` |

**Items**, in sorted order:

- **RegisterAtom { atom }**: obtain the assurance domain; if the registry
  has no atom with that id and `register_atom(atom)` succeeds, record event
  `AtomRegistered { atom: atom.id }`.
- **PostBond { atom, owner, amount }**: obtain the assurance domain. If
  `vault + amount` would overflow, `pay(owner, USD, amount)` and continue
  (nothing is registered or posted). Otherwise the atom is acceptable if
  the registry has an identical atom, or has none and `register_atom`
  succeeds; then `post_bond(atom.id, owner, amount, ctx.pulse)`. On success
  `vault += amount` and event `BondPosted`. On any failure, `pay(owner,
  USD, amount)`.
- **IssueClaim { issuer, backer, body }**: obtain the assurance domain;
  `id = claim_id(issuer, body)`. The claim is rejected (event
  `ClaimRejected { claim: id, reason }`, reason free text) if: a direct atom
  of the formula is not registered; a direct atom is bonded (its `bonded`
  flag is set, whether or not a bond has been posted) and
  `bond_owner(atom) ≠ Some(backer)`; a referenced claim is unknown, or was
  issued at a Pulse ≥ `ctx.pulse`, or is not `Active`; or
  `registry.issue_claim(record)` fails for `record = ClaimRecord { id,
  issuer, formula, adjudicator, detection: Ratio::permille(detection_permille),
  issued_at: ctx.pulse, valid_until: ctx.pulse + valid_for (saturating),
  challenge_until: valid_until + challenge_period (saturating), status:
  Active, sequence: 0 }`. (Among its failures: a formula nesting deeper
  than `MAX_FORMULA_DEPTH`, 32, or a chain of claim references longer than
  `MAX_CLAIM_DEPTH`, 16, counting the new claim; both are
  `sion_assurance::registry` constants.) Otherwise: append the leaf `knowledge_leaf(
  StoredClaim { id, issuer, body, issued_at: ctx.pulse, log_index: <log
  size before appending> })` to the knowledge log; schedule `RegistryTick`
  at `valid_until`; event `ClaimIssued { claim, log_index }`.
- **AddOrder { market, order }**: obtain the market (if the object is
  missing or not a market, `pay(order.owner, escrow_asset, escrow)` and
  continue). `index = ctx.pulse / max(batch_length, 1)`; `seal_at = (index +
  1) × max(batch_length, 1) − 1` (the addition and multiplication
  saturating); get or create batch `index` (status
  `Open`, no orders). If its status is not `Open`, or it already holds
  `MAX_BATCH_ORDERS` (1,024, `sion_exec::market::MAX_BATCH_ORDERS`) orders,
  pay the escrow back.
  Otherwise insert the order keeping the batch's orders sorted by id; if
  the batch had no orders before, schedule `SealBatch { market, batch:
  index }` at `seal_at`.
- **ReturnToBounded { bounded, amount }**: `try_load(bounded)`; if it is a
  bounded total, `returned += amount` (saturating), `unallocated += amount`,
  store it.
- **Treasury { asset, amount }**: obtain the treasury; `balances[asset] +=
  amount` (saturating).
- **Minted { asset, amount }**: obtain the treasury; `minted[asset] +=
  amount` (saturating).
- **Fee { asset, amount }**: obtain the treasury; `balances[asset] +=
  amount` and `fees[asset] += amount` (both saturating).
- **Schedule { at, event }**: schedule `event` at `max(at, ctx.pulse)`.

Scheduling inserts into `schedule.due[at]` (a set); the schedule counts as
changed only if the event was not already there.

**Posting forced intents.** After the items, for each posted intent `t`
with digest `d`, in order: obtain the obligations. If `d` is already
pending, or `max_pending` obligations are pending, `t` is **dismissed**.
Otherwise `t` is dismissed unless it is **admissible**, checked in this
order (stop at the first failure):

1. its kind is not `Faucet`; `ctx.pulse < tx.expires_at`; it has 1 to
   `MAX_INPUTS` inputs with distinct ids; `tx.fee ≥ required_fee(t)`
   (§4.3);
2. its authorizing policy is found as in §4.3, except that every read is
   a `try_load` (a missing object fails) and nothing is stored. This gives
   the intent's **signer**, the account whose policy it is (`tx.sender`,
   or `m.batch.signer` for a keyless sender), and `rotations`, that
   account object's `rotations` (0 for a sender without an account object,
   whose policy is its genesis policy);
3. the authorization passes the check of §4.3 under that policy (always
   checked);
4. for each input, in order: `try_load` it; it MUST exist, be at
   `r.version`, not be a tombstone and be owned by `Account(sender)`; the
   first MUST be a USD coin with amount ≥ `tx.fee`.

Intents are posted before due events are processed, so an obligation
resolved in this Pulse still counts toward `max_pending` while this
Pulse's intents are posted; its slot frees one Pulse later.

A dismissed intent records event `ForcedDismissed { intent: d, reason }`
(reason free text). An admissible one becomes an obligation: `shard =
ShardId::for_account(sender, max(ctx.config.shard_count, 1))`, `deadline =
min(ctx.pulse + window, tx.expires_at)` (the addition saturating, `window =
max(params.window, 1)`); insert `pending[d] = { shard, sender, inputs:
tx.inputs, expires_at: tx.expires_at, rotations, posted_at: ctx.pulse,
deadline, missed: 0, signer }`; schedule `ForcedDeadline { intent: d }` at
`deadline`; event `ForcedPosted { intent: d, shard, deadline }`.

**Due events.** Then remove from `schedule.due` every entry whose key is
`≤ ctx.pulse` (the schedule changes if there was any) and process their
events, in ascending key order and, within a key, in set order. Events
scheduled while processing them are not processed in this Pulse, even if
scheduled at a key `≤ ctx.pulse` (the only one, `BatchDeadline`, is
scheduled at least one Pulse ahead unless the Pulse number saturates):

- **SealBatch { market, batch }**: obtain the market (skip if absent); skip
  if the batch is absent or not `Open`; set it `Sealed { beacon:
  ctx.beacon, at: ctx.pulse }`; event `BatchSealed { market, batch, orders:
  <count> }`; schedule `BatchDeadline { market, batch }` at `ctx.pulse +
  clear_deadline` (saturating).
- **BatchDeadline { market, batch }**: obtain the market (skip if absent);
  skip unless the batch is `Sealed`; set it `Refunded`; for each order in
  order, `pay(owner, escrow_asset, escrow)`; event `BatchRefunded`.
- **RegistryTick**: obtain the assurance domain; `expiries =
  registry.advance_to(ctx.pulse + 1)` (saturating); `released = Σ
  expiries.released_pools`; if positive, `vault -= released`, obtain the
  treasury and `balances[USD] += released` (saturating).
- **ForcedDeadline { intent }**: obtain the obligations; skip if `intent`
  is not pending. The intent is **still executable** if every input of the
  obligation, in order, still exists at its version, is not a tombstone and
  is owned by `Account(sender)` (stop reading at the first that fails), and
  then `try_load(well_known::account(signer))` gives the same `rotations`
  (absent counts as 0; present but not an `Account` counts as not
  executable). A rotation of the signer's policy may void the
  authorization, so an intent whose signer rotated is not held against its
  shard. If it is not, remove it from pending; event
  `ForcedResolved { intent }`. If it is, the shard censored it: event
  `ForcedOverdue { intent, shard }`; obtain the validator set and
  **rotate** the shard; then, if `ctx.pulse ≥ expires_at`, remove the
  obligation; otherwise set its `deadline` to `min(ctx.pulse + window,
  expires_at)` (saturating), increment `missed` (saturating), and schedule
  `ForcedDeadline { intent }` at the new deadline.
- **Rotate** a shard `s` (with `window` as above): skip if the active set
  is empty (the chain has halted: EpochEnd step 4), or if `rotations[s]`
  exists and `ctx.pulse < rotations[s].at + window` (saturating). Otherwise `previous` = the shard's **lane members**
  (`rotations[s].members` if present, else the members of
  `sample_committee(active, seats, seed, Epoch(epoch), s)`); `count =
  rotations[s].count + 1` (or 1; saturating); `rotations[s] = { count,
  members: <members of resample_committee(active, seats,
  rotation_seed(ctx.beacon, s, count), Epoch(epoch), s, previous)>, at:
  ctx.pulse }`; event `ShardRotated { shard: s, rotation: count }`.
- **EpochEnd**: obtain the validator set; skip unless `ctx.pulse =
  epoch_end`. Obtain the treasury; `balance = balances[USD]` (or 0) and
  `collected = fees[USD]` (or 0). Validators are paid from fees only: what
  else reaches the treasury (slashing remainders, released pools) stays
  there.
  Then obtain the delegation book: `try_load(well_known::delegations())`
  (if it exists it MUST be a delegation book, and it is written back).
  1. `gain = collected − fees_mark` (0 if negative); `pool =
     permille_of(gain, reward_permille)`; `total` = the sum of the active
     set's weights. For each `(v, w)` of the active set, in order, whose
     record is `Active` or `Leaving`: `share = share_of(pool, w, total)`;
     `paid += share`. If the book has a pool `p` for `v` with `p.earning >
     0` and `e = value_of(p, p.earning) > 0` (§5.45): `part = mul_div(share,
     e, stake + e)`, with `stake` the record's; `commission =
     permille_of(part, p.commission_permille)`; `delegated = part −
     commission`; `p.reward_per_share += mul_div(delegated, REWARD_SCALE,
     p.earning)`; `p.rewards_held += delegated`; the record's `rewards +=
     share − delegated`. Otherwise the record's `rewards += share`. (The
     rounding left over by the accumulator stays in `rewards_held`.)
  2. Every pool of the book, in validator order: if `pending_delegations
     > 0`, `joins[epoch] = Join { reward_per_share, waiting:
     pending_delegations }`, with `epoch` the ending epoch; `earning +=
     pending`; `pending = 0`; `pending_delegations = 0`; if
     `next_commission = Some(c)`, `commission_permille = c` and
     `next_commission = None`.
  3. Every record, in id order: `Joining` becomes `Active`; `Leaving`
     becomes `Exited { withdrawable_at: ctx.pulse + unbonding }`
     (saturating).
  4. The next active set: the `Active` records, ranked by `stake +
     value_of(p, p.earning)` for their pool `p` (just `stake` without one),
     descending, then id ascending; the first `max(max_active, 1)`, as
     `(id, weight(<that sum>))` sorted by id. It may be empty: if every
     validator that could serve was slashed, the next epoch has no active
     set, no committee can certify its Pulses, and the chain **halts**
     rather than run on proven equivocators with nothing at stake. (§5.27
     keeps the last validator from leaving, so only slashing empties the
     set.)
  5. `epoch += 1` (saturating); `epoch_start = ctx.pulse + 1` (an error at
     Pulse `u64::MAX`, which no chain reaches); `seed = ctx.beacon`;
     `rotations` emptied; `fees_mark = collected`.
  6. Set `balances[USD]` to `balance − paid` (an error if `paid > balance`,
     which no reachable state allows: rewards are a share of fees the
     treasury already holds, and nothing else leaves it). The entry is
     written even when nothing is paid and the treasury had no USD entry
     (it then holds 0). `fees` is not written: a treasury that has
     collected no USD fee keeps no `fees[USD]` entry. Schedule `EpochEnd`
     at the new `epoch_end`; event `EpochChanged { epoch, active: <size of
     the active set>, rewards: paid }`.

Lane committees follow from the validator set: shard `s` is served by its
lane members (above), one seat each, with the committee epoch
`lane_epoch(epoch, rotations[s].count or 0)` (`ValidatorSet::lane_epoch`);
the consensus committee of an epoch is its active set, weighted.

Note that "obtain the market" for a scheduled event of a market not
otherwise touched marks it used, so it is written back even if the event
was skipped.

**Errors.** With the bounds of §5.2 and §5.22 and the saturating
operations above, an item or event can err only if a protocol object is
missing or malformed, `unallocated += amount` overflows (impossible: what
is unallocated plus what live allotments hold never exceeds the bounded
total), the assurance registry's own accounting fails (for example
`vault −= released` underflowing), or sampling a committee fails while
rotating a shard (no seats, which genesis refuses; a shard is never
rotated from an empty active set). No state reachable from a valid genesis
allows any of these. If one happens anyway, the deferred phase is invalid:
the reference executor refuses the Pulse, and a proof stack rejects the
chunk, so no World Proof can exist for it.

## 7. Commitments

- **World commitment** `{ pulse, state_root, knowledge_root,
  knowledge_size, config }` with `config = ChainConfig::digest()`.
- **Effects leaf** (`sion_exec::pulse::effects_leaf`):
  `merkle::leaf_hash(enc((digest, sender, class, code, fee, created,
  mutated, consumed, events, deferred)))` with `code` 0 for success, 1 for
  failed, 2 for rejected.
- **Accumulators**: `chain_deferred(acc, items)` folds
  `acc = Hp("sion/exec/deferred-acc", [acc, enc(item)])` over items;
  `chain_effects_leaf(acc, leaf) = Hp("sion/exec/effects-acc", [acc,
  leaf])`. Both start at `Hash32::ZERO` at the start of a Pulse.
- **Steps**: `H("sion/exec/steps", enc([tx digests in order]))`.

## 8. Verifying a chunk

Given a `ChunkTrace` (statement `S`, witness `W`, transactions `T` or
deferred items `D`):

1. `W.config.digest() = S.before.config = S.after.config`.
2. `S.after.pulse = S.pulse`, and `S.before.pulse ∈ {S.pulse, S.pulse −
   1}`.
3. For each `(id, object, proof)` of `W.objects`: if `object` is present its
   id MUST be `id`; the proof MUST verify against `S.before.state_root` for
   `id` with value `object.digest()` (or absence); no id may appear twice.
4. `W.frontier` MUST be well formed, with root `S.before.knowledge_root`
   and size `S.before.knowledge_size`.
5. Execute with `ctx = { pulse: S.pulse, beacon: S.beacon, config:
   W.config, verify_signatures: true }`, reading only witnessed objects; a
   read of an id outside the witness rejects the chunk.
   - **Transaction chunk** (`S.deferred_phase = false`): `D` MUST be empty;
     `steps(T) = S.steps`; execute each transaction (§4) in order,
     extending `deferred = chain_deferred(deferred, effects.deferred)` from
     `S.deferred_in` and `effects = chain_effects_leaf(effects,
     effects_leaf(·))` from `S.effects_in`; the results MUST equal
     `S.deferred_out` and `S.effects_out`. The knowledge log is untouched.
   - **Deferred phase** (`S.deferred_phase = true`): `T` are the Pulse's
     posted forced intents, with `S.steps = steps(T)`, posted in that
     order; `chain_deferred(ZERO, D)` MUST equal both
     `S.deferred_in` and
     `S.deferred_out`; `S.effects_in = S.effects_out`; run §6.3 on `D`,
     appending claims to a copy of `W.frontier`.
6. The frontier's root and size MUST equal `S.after.knowledge_root` and
   `S.after.knowledge_size`.
7. Apply every write (object id → `Object::digest()`) to a partial sparse
   Merkle tree built from the witness proofs; its root MUST equal
   `S.after.state_root`.

A chunk is valid if and only if every check passes.

Some statement fields cannot be checked from one chunk: its `index`, its
`beacon`, which of `S.pulse − 1` and `S.pulse` its `before.pulse` is, and
whether the deferred phase's `deferred_in` really is everything the
Pulse's transactions deferred. The World Proof binds them (§9).

## 9. Folding a Pulse

A World Proof for Pulse `p` (`sion_prover::WorldProof`) is valid if and
only if:

1. It has at least one statement; statement `i` has `index = i` and
   `pulse = p`; exactly the last one is the deferred phase.
2. The first statement starts at the proof's `before` and the last ends at
   its `after`; `before.pulse = p − 1` and `after.pulse = p`.
3. Consecutive statements chain: each one's `before`, `deferred_in`,
   `effects_in` and `beacon` equal the previous one's `after`,
   `deferred_out`, `effects_out` and `beacon`. The first one's
   `deferred_in` and `effects_in` are `ZERO`. (So the deferred phase
   consumes exactly what the transaction chunks deferred.)
4. Folding `chain_effects_leaf` over the published receipt leaves from
   `ZERO` gives the last statement's `effects_out`, and their Merkle root
   is the proof's `receipts_root`.
5. The published data is for Pulse `p` with the statements' beacon; the
   transaction chunks' `steps` are the digests of consecutive runs of the
   published transactions, covering all of them; the deferred phase's
   `steps` are the digests of the published forced intents, which are the
   Pulse's **posted** intents (each digest once, in posting order, §6.1),
   not the raw committed list; and the data encodes (`sion_da`) to the
   proof's `data_root` and `data_k`. This binding is what pins the
   deferred phase's `T` (§8): within one chunk, a reordered `T` or an
   extra intent dismissed before any read would verify against refitted
   `steps`.
6. Every leaf proof verifies (§8) under every required proof stack.

Whoever relies on the proof also checks it against the certified Pulse
header: the same `pulse`, `before`, `after`, `receipts_root`, `data_root`,
`data_k` and beacon. Lane certificates
(`sion_prover::verify_certificates`) are checked against the lane
committees of the validator set in force when the Pulse began: the proof
carries the validator set with a state proof against `before.state_root`.

## Changes

- **v1.15.** Attested tasks and an index of claims. A task's acceptance
  rule can require attestations: `Acceptance` gains `must_attest` (the
  verifiers who must each attest every result) and `min_attestations` (how
  many attestations a result needs), and a result is never attested by
  its author (§5.43 step 4), nor is a named worker one of its task's
  verifiers (§5.42 step 1). Until v1.14 a worker could settle on its own
  bond alone if the bond was large enough, and only the commands kept a
  worker from attesting its own result: in an open bounty, a named
  verifier could submit a result of its own with its own attestation. New
  operation `IndexClaim` (§5.58): an owner's index entry at
  `index_id(owner, key)` names its latest claim under the key and moves
  only to later claims of its own. New module `sion_exec::index` (type
  `IndexEntry`, constant `MAX_INDEX_KEY`, `index_id`), object
  `ObjectData::IndexEntry`, transaction kind `IndexClaim` and event
  `ClaimIndexed`; the encodings of `Acceptance`, and so of `PostTask` and
  of tasks, change.

- **v1.14.** Lineage ids, token buckets, overlays and cohorts (§5.13).
  A spawned charter's id is `lineage_id(parent, index)`, with the index
  from a counter in the parent's allotment (`Allotment::spawned`), and every
  charter keeps its `Lineage` (its tree's root and the path down to it), so
  its ancestors' ids are computed, not read; depths run from 0 to
  `MAX_LINEAGE_DEPTH` (8), down from chains of up to 16 charters. Window
  limits become token buckets: caveats carry `burst` and `rate` instead of
  `max_per_window` and `window`; allotments carry a `Bucket` instead of
  `window_start`, `spent_in_window` and `carved_window`, start full, and a
  spawn carves its children's burst, rate and starting tokens out of its
  parent's bucket, so a subtree pays at most `σ + ρ·τ` in any `τ + 1`
  Pulses, where until v1.13 it could pay twice its window's limit across a
  boundary. What happens to a charter after it is created moves into its
  overlay, an `AgentOverlay` at `overlay_id(charter)`: `RevokeCharter`,
  `SuspendAgent` and `ResumeAgent` write it, charters lose `revoked_at`
  and `suspended_by`, and the chain check, which loaded up to 16 charters,
  becomes the standing check, which reads up to 9 overlays, most of them
  absent. New operations: `SpawnCohort` (§5.55) commits to up to 2²⁰
  subagents in one transaction as the root of a Merkle sum tree over their
  bursts and rates, carved from the parent's bucket; `MaterializeAgents`
  (§5.56) creates up to 128 of them at their lineage ids, each with a proof
  of its leaf, its budget from the parent's allotment; `NarrowAgent`
  (§5.57) adds limits (payees, per-payment, expiry, spawning) to a whole
  subtree with one write, which only the backer may widen or lift. New
  primitive `create_at` (§3.1). New types `sion_exec::charter::{Bucket,
  Lineage, Narrowing, AgentOverlay}` and `sion_exec::cohort::{Cohort,
  CohortMember, SumNode, Materialize}`, objects `ObjectData::{AgentOverlay,
  Cohort}`, constants `MAX_LINEAGE_DEPTH`, `MAX_COHORT` and
  `MAX_MATERIALIZE` (`MAX_CHARTER_CHAIN` is gone), transaction kinds and
  events `CohortSpawned`, `AgentMaterialized` and `AgentNarrowed`. The
  keyless ancestor search (§4.3) and the overseer search stop after
  `MAX_LINEAGE_DEPTH` ancestors. From a review of this version: an
  overseer acts only in good standing, its own charter and those above it
  neither revoked nor suspended (**oversight**, replacing the overseer
  search); a suspension by an overseer above the recorded suspender
  replaces it, where the first suspender used to be kept and could lift
  the suspension of one above it (§5.38); an expired charter, or one under
  an expired narrowing, spawns nothing, and nothing is spawned already
  expired (**may_spawn**, **born**); and a child that requires evidence
  judges it by its parent's policy (**attenuates**).

- **v1.13.** Batched authorization and keyless subagents (§4.3). One
  authorization can sign a batch of up to 256 transactions: the Merkle
  root over their digests, with its signer and size. Each member carries
  the authorization, its position and its inclusion path instead of
  signatures of its own, and its fee floor is lowered by the share of the
  base fee that pays for checking an authorization (80 %), less its part
  of the batch's. A subagent may be spawned keyless (§5.36): its account is
  derived from its id, so no key controls it, and its transactions are
  authorized only as members of batches signed by its nearest keyed
  ancestor. Forced intents may be batch members; an obligation records the
  account whose policy authorized its intent (`signer`), and a rotation of
  that account's policy resolves it (§6.3). New types
  `sion_exec::tx::{TxBatch, BatchMember}`, field `SignedTransaction::batch`
  (so the encoding of every signed transaction changes), `Spawn::agent`
  becomes optional, `Obligation` gains `signer`, constants
  `sion_exec::tx::{MAX_BATCH, SIGNATURE_SHARE_PERMILLE}`, helpers
  `sion_exec::tx::{batch_signing_context, fee_floor, required_fee,
  sign_batch}` and `sion_exec::charter::keyless_account`.

- **v1.12.** Storage deals (§5.50 to §5.54). A client escrows payment for
  keeping data whose Merkle root it fixes at posting (`PostDeal`), for a
  named provider or for any; the provider accepts with collateral
  (`AcceptDeal`) and gets a custody record it owns. In each round it
  proves, from its record, that it holds leaves picked by a challenge
  derived from the beacon of the round's first Pulse (`ProveStorage`,
  owned-linear, many deals per transaction). Rounds start on multiples of
  `BEACON_INTERVAL` (8), and the deferred phase of every such Pulse records
  its beacon in one of `BEACON_SLOTS` (12,500) beacon records, so a proof
  can read the beacon of its round's first Pulse for up to
  `MAX_ROUND_LENGTH` (100,000) Pulses. The provider collects for proven
  rounds (`SettleDeal`); closing (`CloseDeal`) pays what is due, refunds the
  escrow of unproven rounds to the client, returns the collateral less
  `⌊bond × missed / rounds⌋`, and forfeits all of it to the treasury if the
  client closes a deal its provider has abandoned for `ABANDON_ROUNDS`
  rounds. An open deal asks for collateral. New types
  `sion_exec::storage::{Deal, DealState, Custody, CustodyProof,
  LeafProof}` and `sion_exec::beacons::BeaconRecord`, objects
  `ObjectData::{Deal, Custody, Beacon}` and `well_known::beacon(slot)`,
  transaction kinds and events; the supply counts deal escrow and
  collateral. Before release, an independent review changed three things:
  as first written, each proof fixed the next round's challenge through the
  beacon of the Pulse it executed in, so a provider that could time its
  proofs could steer its challenges (one keeping half its data passed 20
  rounds of 20 in a simulation); a missed round cost `⌊bond / rounds⌋`,
  nothing when the collateral was smaller than the number of rounds; and
  open deals could ask for no collateral.

- **v1.11.** Pooled stake (§5.45 to §5.49). Anyone may delegate stake to a
  validator without handing it over: a delegation is an object its
  delegator owns, holding shares of the validator's pool, which counts
  towards the validator's weight and shares its rewards from the end of the
  epoch the delegation was made in. At each epoch's end a validator's
  reward is split between its own stake and its pool's earning stake in
  proportion; the operator keeps its commission (at most 25 %, raised by at
  most 2 points per epoch once anyone has delegated, every change taking
  effect at the epoch's end) and the rest is credited per share, exactly,
  for delegators to claim. Validators rank for the active set by their own
  plus their pools' earning stake. No validator may gain more than twice
  an equal share of the bonded stake by delegation (never less than a
  tenth). An equivocation now also slashes a tenth of the offender's pool,
  unbonding delegations included, all of it to the treasury: the reporter's
  share is of the offender's own stake and rewards only. (As first written,
  v1.11 paid the reporter a share of the pool's tenth too, which made
  reporting yourself profitable once your pool was about 190 times your
  stake; a review of the economics found it, and it was changed before
  release.) New
  types `sion_exec::delegation::{DelegationBook, Pool, Join, Delegation,
  DelegationState}`, objects `ObjectData::{Delegations, Delegation}` and
  `well_known::delegations()`, transaction kinds and events; the supply
  counts pooled stake and delegators' unclaimed rewards; the epoch end
  reads and writes the delegation book (§6.3).

- **v1.10.** Task Charters (§5.42 to §5.44). `PostTask` escrows money for
  a task whose acceptance is fixed when it is posted: the policy the
  decision rule runs under, the claims every result must rest on (an
  environment's bonded "no known exploit" claim, say), the verifiers whose
  attestations count, the adjudicator, the largest detection factor and how
  long the poster relies on a paid result. An agent posts from its
  allotment, under its caveats. `SettleTask` pays the worker once a result
  claim whose payload commits to the task, and whose formula is exactly
  the worker's bond or a named verifier's attestation of the same output
  and every required claim, passes the decision rule; κ times the payment
  of its backing is reserved for the party whose money paid, so a result
  later proven false compensates it. `CancelTask` refunds an open task, an
  expired one, or one its worker releases. New types
  `sion_exec::task::{Task, TaskState, Acceptance, TaskResult}`, object
  `ObjectData::Task`, transaction kinds and events; the supply counts open
  task escrow.

- **v1.9.** Memory logs (§5.40, §5.41). `OpenMemory` creates a memory log
  owned by its writer, optionally for an agent identity whose agent is the
  writer; `CommitMemory` moves the log's head to a new size and root with
  an RFC 9162 consistency proof from the current head, which the ledger
  checks, so a log only grows and cannot fork. The Pulse of a commit is
  the transaction time of the atoms it adds. A suspended or revoked
  agent's log cannot grow. New type `sion_exec::memory::MemoryLog`, object
  `ObjectData::MemoryLog`, transaction kinds and events. The atoms, their
  index and query proofs are off the ledger (the `sion-memory` crate).

- **v1.8.** Agent identities (§5.35 to §5.39). An agent identity is a
  charter with an `identity` record: a commitment to the agent's card and
  its depth in the delegation tree; the charter's id is the agent's id.
  `RegisterAgent` registers one; `SpawnAgents` launches up to 256
  subagents in one transaction, each with its own account, card,
  attenuated caveats and budget carved from the parent's allotment;
  `SetAgentCard` replaces a card; `SuspendAgent` and `ResumeAgent` pause and resume a charter and
  everything below it. Charters gain `suspended_by` and `identity`; the
  chain check fails on a suspended charter (§5.13), except when
  `ReclaimCharter` asks whether a charter is dead. `RevokeCharter` may now
  be sent by the agent itself or by any overseer: the backer, the
  principal, or the agent of any charter above (§5.16), so one transaction
  from any ancestor stops a whole subtree. Window limits are carved like
  budgets (§5.13 **carve**, used by `Delegate` and `SpawnAgents`):
  allotments gain `carved_window`, and an agent's own window limit is
  `max_per_window − carved_window`. Until v1.7, each child could spend its
  parent's whole window limit, so *k* children could spend *k* times the
  parent's rate. New types `sion_exec::charter::{AgentIdentity, Spawn}`,
  transaction kinds and events.

- **v1.7.** Stream checkpoints (§5.31, §5.32): a payer may commit, when it
  opens a stream, to a Merkle tree over every `spacing`-th element of its
  chain, and a payee may then settle by a jump to the checkpoint below its
  position instead of a walk from the head. A jump costs at most `spacing +
  1 + 20` hashes however many steps it covers, and the per-hash fee applies
  to the hashes actually checked; a walk is still limited to 65,536 steps.
  New types `sion_exec::stream::{Checkpoints, Jump}`; `OpenStream` gains
  `checkpoints`, `SettleStream` gains `jump`, and `Stream` gains `spacing`
  and `checkpoints`.

- **v1.6.** Payment streams (§5.31 to §5.34): a payer escrows funds for one
  payee and pays it off the ledger by revealing the elements of a hash
  chain, one hash to check per payment; the payee settles many payments in
  one transaction, which checks one hash per step and pays a fee per step.
  `SettleStream`, `TopUpStream` and `CloseStream` are contended in the
  stream's own domain. New types `sion_exec::stream::Stream`, object
  `ObjectData::Stream`, transaction kinds and events.

- **v1.5.** From an independent review of the staking machinery: at an
  epoch's end, slashed validators lose their seats even if no other
  validator is eligible (§6.3, EpochEnd step 4). Until v1.4 the old active
  set was kept when the next one would be empty, so a committee whose every
  member had been slashed carried on into the next epoch with nothing at
  stake; now the next active set is empty and the chain halts, and a
  halted chain rotates no shard (an overdue intent is still recorded). The
  reasoning behind the `Approvals` evidence is spelled out (§5.30): an
  approval holds its inputs until its transaction succeeds, fails or
  expires, and a rejection is none of these (the reference node used to
  release a rejected transaction's locks, which let an honest validator be
  slashed; it now keeps them until the transaction expires).

- **v1.4.** Validators are paid from fees only. A transaction's fee is
  deferred as `Fee { asset, amount }` (§3.2, §3.3), which credits the
  treasury's balance and its running total of `fees` (§6.3). At an epoch's
  end the reward pool is `reward_permille` of the fees collected since the
  last epoch end (`fees_mark`), not of everything the treasury gained, so
  slashing remainders and released bond pools never become validator
  income. `ValidatorSet::treasury_mark` becomes `fees_mark`.

- **v1.3.** Validators, stake, epochs and the censorship response join the
  state transition. A validator stakes the stake asset with a proof that
  it holds its keys (§5.26), leaves and withdraws after unbonding (§5.27,
  §5.28), and collects rewards: at each epoch's end, a share of what the
  treasury gained is paid to the epoch's validators by weight (§5.29,
  §6.3). Equivocation, two consensus headers for one round or two live
  approvals of one object version, is slashed on signed evidence, with a
  share to the reporter (§5.30). Forced intents committed in a Pulse are
  posted as obligations of their sender's shard (§6.1, §6.3); at the
  deadline an intent that could still execute is overdue and the shard's
  committee is replaced by validators outside it (rotation). Committees are
  derived from the state, so every committee change follows these rules
  and the World Proof covers it (§9).

  Clarified after proof stack B implemented v1.3 (no behaviour change; the
  differential tests agree): the epoch end always writes the treasury's
  USD entry (it found the one divergence, now resolved in favour of the
  reference executor); the deferred phase's `steps` cover the posted,
  deduplicated intents; committee sampling joins the deferred phase's
  possible errors; evidence is not bound to this chain or to a time, so
  validator keys must not be shared between networks; an obligation
  resolved in a Pulse frees its slot one Pulse later.

- **v1.2.** From independent reviews of the node and of the
  consensus-critical libraries: transaction signatures are bound to the
  chain configuration (§4.1), so they cannot be replayed on another chain;
  a transaction names at most 256 inputs (§4.1); a bounded total has at
  most 256 allotments (§5.18); a batch holds at most 1,024 orders (§6.3);
  and two limits enforced by the shared libraries are written down: claim
  references and formulas have bounded depth (the unbounded chain could
  overflow a validator's stack), and certificate values have bounded size
  (§5.25, §6.3). Reservation splits are now computed exactly (they
  overflowed for large capacities). Formulas nested deeper than 64 levels
  no longer decode (§0): decoding, sizing and evaluating a formula recurse
  once per level, so a hostile transaction could otherwise overflow the
  stack of whichever thread decoded it.

- **v1.1.** From the differential testing of stack B against stack A:
  faucet transactions pay no fee and grants are capped (§4.1, §5.2);
  market periods are bounded (§5.22); charter windows, the clearing
  check, sealing and deadlines saturate (§5.13, §5.25, §6.3); the chain
  check reads at most 16 charters (§5.13); a bond the vault cannot hold is
  refunded (§6.3); treasury totals and returned amounts saturate (§6.3);
  due events scheduled during due processing wait (§6.3); deferred-phase
  errors are specified (§6.3); the deferred phase's `steps` is checked
  (§8); and what the World Proof binds is specified (§9). Before v1.1, a
  market with a clearing deadline near `u64::MAX` could halt a debug build
  and split debug from release builds; a charter window near `u64::MAX`
  made the reference executor diverge from this document.
