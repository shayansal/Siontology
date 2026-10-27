# The Fold state transition — specification v1.2

Status: normative for this reference implementation. Two implementations
exist: the reference executor (`crates/fold-exec`, proof stack A) and an
independent one written from this document (`crates/fold-spec`, proof stack
B). Where they disagree, one of them is wrong, and the differential tests in
`crates/fold-spec/tests` exist to find out which.

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

- **Data layouts.** The Rust type definitions in `fold_core`, `fold_crypto`,
  `fold_assurance`, `fold_clearing` and the *type definitions* in
  `fold_exec` (`object`, `account`, `claim`, `charter`, `escrow`, `market`,
  `assurance`, `tx`, `state`, `pulse::{ChunkStatement, ChunkTrace, Witness,
  PulseInput}`, `execute::{Deferred, Event, ExecStatus, TxEffects}`) are
  normative; their canonical encoding is Borsh. One rule goes beyond the
  layout: a `Formula` nested deeper than 64 levels
  (`fold_assurance::formula::MAX_DECODE_DEPTH`, counted as
  `Formula::depth`) is not a valid encoding, so a transaction carrying one
  fails to decode and never enters a Pulse.
- **Shared primitives.** Both implementations use, and trust as separate
  atoms of the World Proof's formula: hashing and Merkle structures
  (`fold_core`), signature verification (`fold_crypto`), the Assurance
  Algebra (`fold_assurance`: registry operations, cost of corruption,
  reservation, decision rule, slashing), clearing-certificate verification
  (`fold_clearing::verify_clearing`) and committee envelopes
  (`fold_crypto::CommitteeEnvelope::open`).

An implementation of this document MUST NOT call the reference executor's
logic: `fold_exec::execute`, `fold_exec::pulse` (other than the type
definitions and the commitment helpers named in §7), `fold_exec::stateless`,
`fold_exec::wallet`, `fold_exec::sim`, `fold_exec::genesis`, nor the methods
`Allotment::spend`, `Caveats::attenuates`, `OrderTerms::escrow`,
`Market::clearing_batch` and `Market::batch_index`. It MAY call the type
helpers `Transaction::{digest, domain, class, size}`,
`SignedTransaction::digest`, `Object::{digest, is_tombstone, reference,
as_coin}`, `ChainConfig::{min_fee, digest}`, `Payload::size`,
`WorldCommitment::digest`, `ChunkStatement::digest`, `claim::{claim_id,
knowledge_leaf, formulas::*}`, `object::{created_id, well_known::*}`,
`tx::object_domain`, `tx::signing_context`, `market::sealing_aad`, and
`execute::{bond_atom, registered_atom, bond_owner}`.

## 1. Conventions

- `H(domain, bytes)` is `fold_core::hash::hash_domain`; `Hp(domain, parts)`
  is `fold_core::hash::hash_parts`. `enc(x)` is the Borsh encoding.
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
id to `Object::digest()` (`fold_core::smt`). The knowledge root is the RFC
9162 root of the log's leaves.

Well-known objects (all created at genesis): the assurance domain
`well_known::registry()` (`ObjectData::Assurance`), the treasury
`well_known::treasury()`, the schedule `well_known::schedule()`. The account
object of account `a` is `well_known::account(a)`.

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
   `Deferred::Treasury { asset: USD, amount: fee }` to `effects.deferred`.
6. `mint(tx.sender, asset, available − need)` (the change).

### 3.3 charge_fee_from_gas and finalize_inputs

- **charge_fee_from_gas**: if `fee_paid` or `tx.fee = 0`, do nothing.
  Otherwise `load(tx.inputs[0].id)`; it is an error if it is a tombstone or
  not a coin; subtract `tx.fee` from its amount (error if insufficient);
  store it; `fee_paid = true`; `effects.fee = tx.fee`; append
  `Deferred::Treasury { asset: USD, amount: tx.fee }`.
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
   `fold_exec::tx::MAX_INPUTS`) → reject. Duplicate input ids → reject.
3. If the kind is `Faucet`, `tx.fee ≠ 0` → reject. Otherwise `tx.fee <
   ctx.config.min_fee(tx.size())` → reject.
4. Account: `try_load(well_known::account(tx.sender))`.
   - If it exists, it MUST be `ObjectData::Account`; the policy is its
     `policy`.
   - Otherwise `tx.genesis_policy` MUST be present, MUST pass
     `AuthPolicy::validate`, and `policy.account_id()` MUST equal
     `tx.sender`; then **store** a new account object
     `Object { id: well_known::account(sender), version: 0, owner:
     Account(sender), data: Account(AccountState { id: sender, policy,
     viewing_key: None, created_at: ctx.pulse, rotations: 0 }) }`.
5. `policy.verify(signing_context(ctx.config), digest bytes, &signed.auth)`
   MUST succeed, where `signing_context(config)` is `TX_DOMAIN`, a `/`, and
   `config.digest()` in lowercase hex (`fold_exec::tx::signing_context`).
   A transaction signed for one chain therefore never verifies on a chain
   with another configuration.
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
   NOT exceed `MAX_GRANT` (`fold_exec::tx::MAX_GRANT`, one billion units);
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
3. `(decision, reservation) = fold_assurance::accept_and_reserve(&mut
   domain.registry, policy, sender, claim, value_at_risk, until, nonce)`;
   an error is an error; `reservation = None` is an error ("not accepted").
4. Store the assurance object with the updated domain.
5. Defer `Schedule { at: until, event: RegistryTick }`.
6. Event `Reserved { reservation, claim, amount: decision.required }`.

### 5.8 Release { reservation }

Load the assurance object; `fold_assurance::release(&mut registry,
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

1. `caveats.expires_at > ctx.pulse`; `caveats.window > 0`.
2. `draw(USD, funding + agent_bond)`.
3. `charter_id = created_id(digest, next_index)` (the id the next create
   will use); `agent_atom = bond_atom(Economic, sender,
   "charter:" + hex(charter_id))`, where `hex` is the 64-digit lowercase
   form (`ObjectId`'s `Display`).
4. `create(Shared(object_domain("charter", charter_id)), Charter {
   principal: sender, agent, caveats, agent_atom: agent_atom.id, backer:
   sender, revoked_at: None, parent: None, allotments: ∅, created_at:
   ctx.pulse })`.
5. `allotment = create(Account(agent), Allotment { source: charter_id,
   writer: agent, asset: USD, remaining: funding, caveats: Some(caveats),
   window_start: ctx.pulse, spent_in_window: 0 })`.
6. Load the charter, insert `allotment` into `allotments`, store it.
7. Defer `RegisterAtom { atom: agent_atom }`; if `agent_bond > 0`, defer
   `PostBond { atom: agent_atom, owner: sender, amount: agent_bond }`.
8. Event `CharterCreated { charter: charter_id, allotment }`.

### 5.13 Charter checks

- **Chain check** of charter `c`: follow `c, c.parent, …`, loading each
  (MUST be a charter); any with `revoked_at` set fails ("revoked"). At most
  `MAX_CHARTER_CHAIN` (16, `fold_exec::charter::MAX_CHARTER_CHAIN`)
  charters are loaded: if the last one loaded still has a parent, the check
  fails without loading that parent.
- **attenuates(child, parent)** holds when: `parent.payees = None`, or both
  are sets and `child.payees ⊆ parent.payees`; `parent.evidence_above =
  None`, or both are set and `child ≤ parent`; `child.max_per_payment ≤
  parent.max_per_payment`; `child.max_per_window ≤ parent.max_per_window`;
  `child.window ≥ parent.window`; `child.expires_at ≤ parent.expires_at`;
  `parent.receipts ⇒ child.receipts`; `child.may_delegate ⇒
  parent.may_delegate`.
- **spend(allotment, payee, amount, evidence_ok)**, in this order:
  1. `amount > remaining` fails.
  2. If the allotment has caveats `k`: `ctx.pulse > k.expires_at` fails;
     `k.payees` set and `payee ∉ k.payees` fails; `amount >
     k.max_per_payment` fails; if `ctx.pulse ≥ window_start + k.window`
     (saturating, so a window that would end after `u64::MAX` never
     resets), set `window_start = ctx.pulse` and `spent_in_window = 0`;
     `spent_in_window + amount > k.max_per_window` fails; if
     `k.evidence_above = Some(t)` and `amount > t` and `evidence_ok ≠
     Some(true)` fails; then `spent_in_window += amount`.
  3. `remaining -= amount`.

### 5.14 Delegate { charter, sub_agent, caveats, amount }

1. `allotment_id = tx.inputs[1].id` (error if absent); load it (MUST be an
   allotment).
2. Load `charter` (MUST be a charter); **chain check** it.
3. The allotment's `source` MUST be `charter`, its `writer` and the
   charter's `agent` MUST be the sender.
4. The allotment MUST have caveats `k`; `k.may_delegate`;
   `attenuates(caveats, k)`.
5. `remaining -= amount` (error if insufficient); store the allotment.
6. `child_id = created_id(digest, next_index)`;
   `create(Shared(object_domain("charter", child_id)), Charter { principal:
   sender, agent: sub_agent, caveats, agent_atom: parent.agent_atom,
   backer: parent.backer, revoked_at: None, parent: Some(charter),
   allotments: ∅, created_at: ctx.pulse })`.
7. `child_allotment = create(Account(sub_agent), Allotment { source:
   child_id, writer: sub_agent, asset: USD, remaining: amount, caveats:
   Some(caveats), window_start: ctx.pulse, spent_in_window: 0 })`.
8. Load the child charter, insert `child_allotment`, store it.
9. Event `CharterCreated { charter: child_id, allotment: child_allotment }`.

### 5.15 AgentPay { charter, payee, amount, purpose, evidence }

1. Load `allotment_id = tx.inputs[1].id` (MUST be an allotment); load
   `charter` (MUST be a charter); **chain check** it; `source = charter`,
   `writer = sender`, `charter.agent = sender`; the allotment MUST have
   caveats `k`.
2. `evidence_ok = None`, unless `evidence = Some(c)` and `k.evidence_above
   = Some(t)` and `amount > t`: then load the assurance object and
   `evidence_ok = Some(b)` where `b` is false if `c` is unknown to the
   registry or not `Active`, and otherwise `fold_assurance::decide(registry,
   k.evidence_policy, c, amount, insured = false).accept`.
3. **spend**; store the allotment; `mint(payee, USD, amount)`.
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

Load it (MUST be a charter); `principal` MUST be the sender; if
`revoked_at` is unset set it to `ctx.pulse`; store; event
`CharterRevoked { charter }`.

### 5.17 ReclaimCharter { charter }

1. Load it (MUST be a charter, call it `s`); the sender MUST be
   `s.principal` or `s.backer`.
2. The charter MUST be dead: `s.revoked_at` set, or `ctx.pulse >
   s.caveats.expires_at`, or its **chain check** fails. (Evaluate in this
   order and stop at the first that holds; the chain check is only run if
   the first two do not hold.)
3. `total = 0`; for each id in `s.allotments`, in order: load it; if it is
   an allotment, `total += remaining` and **consume** it.
4. `mint(s.backer, USD, total)`; event `CharterReclaimed { charter, amount:
   total }`.

### 5.18 CreateBounded { label, asset, total, allotments }

1. At most `MAX_ALLOTMENTS` (256, `fold_exec::escrow::MAX_ALLOTMENTS`)
   allotments; `allotted = Σ amounts`; `allotted ≤ total`.
2. `draw(asset, total)`.
3. `bounded_id = created_id(digest, next_index)`;
   `create(Shared(object_domain("bounded", bounded_id)), BoundedTotal {
   label, asset, manager: sender, total, unallocated: total − allotted,
   allotments: ∅, returned: 0 })`.
4. For each `(writer, amount)` in order: `id = create(Account(writer),
   Allotment { source: bounded_id, writer, asset, remaining: amount,
   caveats: None, window_start: ctx.pulse, spent_in_window: 0 })`; record
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
caveats: None, window_start: ctx.pulse, spent_in_window: 0 })`; event
`AllotmentUsed { allotment, amount }`.

### 5.22 CreateMarket { label, good, numeraire, batch_length, clear_deadline, committee, threshold }

`1 ≤ batch_length ≤ MAX_PERIOD` and `1 ≤ clear_deadline ≤ MAX_PERIOD`
(`fold_exec::market::MAX_PERIOD`, one million Pulses); if `committee` is
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
4. `order_id = Hp("fold/market/order", [digest bytes])`; defer `AddOrder {
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
3. Build the clearing batch `fold_clearing::Batch { domain: market.domain,
   pulse: at, beacon, orders }` where `orders` are, in batch order, the
   orders with known terms mapped to `Order { id, owner, side, quantity,
   limit_price }`.
4. `verified = fold_clearing::verify_clearing(&batch, result)` (errors are
   errors; among them, a certificate value whose numerator or denominator
   is longer than `fold_lp::MAX_VALUE_BITS`, 512 bits); `price = verified.price`; `fills` = `result.fills` as a map
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

## 6. The Pulse

### 6.1 Order

A Pulse executes `PulseInput { pulse, beacon, owned, contended }` with
`pulse = state.pulse + 1`. The nodes compute a canonical order (owned
transactions by digest, in dependency passes, then contended transactions
in consensus order; see `fold_exec::pulse::canonical_order`). Proof stacks
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
(`fold_exec::pulse::deferred_coin_id`) and increment `k`.

**Protocol objects.** The schedule object is always loaded first. The
assurance object and the treasury are loaded the first time an item needs
them (and only then); a market is loaded the first time an item or event
needs it. Each of these is written back at the end **only if** something
used it mutably (assurance, treasury: any item or event that obtained it;
markets: any item or event that obtained it; schedule: any change to its
entries, including removing due entries). Write-back order: markets in id
order, then assurance, then treasury, then schedule. Other objects (bounded
totals, coins) are stored when changed.

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
  `fold_assurance::registry` constants.) Otherwise: append the leaf `knowledge_leaf(
  StoredClaim { id, issuer, body, issued_at: ctx.pulse, log_index: <log
  size before appending> })` to the knowledge log; schedule `RegistryTick`
  at `valid_until`; event `ClaimIssued { claim, log_index }`.
- **AddOrder { market, order }**: obtain the market (if the object is
  missing or not a market, `pay(order.owner, escrow_asset, escrow)` and
  continue). `index = ctx.pulse / max(batch_length, 1)`; `seal_at = (index +
  1) × max(batch_length, 1) − 1` (the addition and multiplication
  saturating); get or create batch `index` (status
  `Open`, no orders). If its status is not `Open`, or it already holds
  `MAX_BATCH_ORDERS` (1,024, `fold_exec::market::MAX_BATCH_ORDERS`) orders,
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
- **Schedule { at, event }**: schedule `event` at `max(at, ctx.pulse)`.

Scheduling inserts into `schedule.due[at]` (a set); the schedule counts as
changed only if the event was not already there.

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

Note that "obtain the market" for a scheduled event of a market not
otherwise touched marks it used, so it is written back even if the event
was skipped.

**Errors.** With the bounds of §5.2 and §5.22 and the saturating
operations above, an item or event can err only if a protocol object is
missing or malformed, `unallocated += amount` overflows (impossible: what
is unallocated plus what live allotments hold never exceeds the bounded
total), or the assurance registry's own accounting fails (for example
`vault −= released` underflowing). No state reachable from a valid genesis
allows any of these. If one happens anyway, the deferred phase is invalid:
the reference executor refuses the Pulse, and a proof stack rejects the
chunk, so no World Proof can exist for it.

## 7. Commitments

- **World commitment** `{ pulse, state_root, knowledge_root,
  knowledge_size, config }` with `config = ChainConfig::digest()`.
- **Effects leaf** (`fold_exec::pulse::effects_leaf`):
  `merkle::leaf_hash(enc((digest, sender, class, code, fee, created,
  mutated, consumed, events, deferred)))` with `code` 0 for success, 1 for
  failed, 2 for rejected.
- **Accumulators**: `chain_deferred(acc, items)` folds
  `acc = Hp("fold/exec/deferred-acc", [acc, enc(item)])` over items;
  `chain_effects_leaf(acc, leaf) = Hp("fold/exec/effects-acc", [acc,
  leaf])`. Both start at `Hash32::ZERO` at the start of a Pulse.
- **Steps**: `H("fold/exec/steps", enc([tx digests in order]))`.

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
   - **Deferred phase** (`S.deferred_phase = true`): `T` MUST be empty;
     `S.steps = steps([])`; `chain_deferred(ZERO, D)` MUST equal both
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

A World Proof for Pulse `p` (`fold_prover::WorldProof`) is valid if and
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
   published transactions, covering all of them; and the data encodes
   (`fold_da`) to the proof's `data_root` and `data_k`.
6. Every leaf proof verifies (§8) under every required proof stack.

Whoever relies on the proof also checks it against the certified Pulse
header: the same `pulse`, `before`, `after`, `receipts_root`, `data_root`,
`data_k` and beacon. Lane certificates
(`fold_prover::verify_certificates`) are checked against the shard
committees that the chain identity fixes for the header's epoch.

## Changes

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
