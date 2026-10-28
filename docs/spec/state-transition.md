# The Siontology state transition — specification v1.11

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
  `sion_exec` (`object`, `account`, `claim`, `charter`, `escrow`, `market`,
  `stream`, `memory`, `task`, `delegation` (its types and constants),
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
shares_for, bonded, earning_value, owed}`), nor
the methods `Allotment::spend`, `Caveats::attenuates`, `OrderTerms::escrow`,
`Market::clearing_batch`, `Market::batch_index`, `Equivocation::offender`
and the `ValidatorSet` methods. It MAY call the type
helpers `Transaction::{digest, domain, class, size}`,
`SignedTransaction::digest`, `Object::{digest, is_tombstone, reference,
as_coin}`, `ChainConfig::{min_fee, digest}`, `Payload::size`,
`WorldCommitment::digest`, `ChunkStatement::digest`, `claim::{claim_id,
knowledge_leaf, formulas::*}`, `object::{created_id, well_known::*}`,
`tx::object_domain`, `tx::signing_context`, `market::sealing_aad`,
`execute::{bond_atom, registered_atom, bond_owner}`, and
`validators::{weight, permille_of, share_of, possession_digest,
rotation_seed}`.

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
   `config.digest()` in lowercase hex (`sion_exec::tx::signing_context`).
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

1. `caveats.expires_at > ctx.pulse`; `caveats.window > 0`.
2. `draw(USD, funding + agent_bond)`.
3. `charter_id = created_id(digest, next_index)` (the id the next create
   will use); `agent_atom = bond_atom(Economic, sender,
   "charter:" + hex(charter_id))`, where `hex` is the 64-digit lowercase
   form (`ObjectId`'s `Display`).
4. `create(Shared(object_domain("charter", charter_id)), Charter {
   principal: sender, agent, caveats, agent_atom: agent_atom.id, backer:
   sender, revoked_at: None, parent: None, allotments: ∅, created_at:
   ctx.pulse, suspended_by: None, identity: None })`.
5. `allotment = create(Account(agent), Allotment { source: charter_id,
   writer: agent, asset: USD, remaining: funding, caveats: Some(caveats),
   window_start: ctx.pulse, spent_in_window: 0, carved_window: 0 })`.
6. Load the charter, insert `allotment` into `allotments`, store it.
7. Defer `RegisterAtom { atom: agent_atom }`; if `agent_bond > 0`, defer
   `PostBond { atom: agent_atom, owner: sender, amount: agent_bond }`.
8. Event `CharterCreated { charter: charter_id, allotment }`.

### 5.13 Charter checks

- **Chain check** of charter `c`: follow `c, c.parent, …`, loading each
  (MUST be a charter); for each in turn, `revoked_at` set fails
  ("revoked"), then `suspended_by` set fails ("suspended"). At most
  `MAX_CHARTER_CHAIN` (16, `sion_exec::charter::MAX_CHARTER_CHAIN`)
  charters are loaded: if the last one loaded still has a parent, the check
  fails without loading that parent. A chain check **ignoring suspension**
  skips the `suspended_by` test. A passing check also yields `n`, the
  number of charters it loaded.
- **carve(allotment, k, limit)**: `limit > k.max_per_window −
  allotment.carved_window` fails ("window allowance exhausted"); otherwise
  `carved_window += limit`. A child's window limit is carved out of its
  parent's, as its budget is out of the parent's allotment, so the window
  limits of a whole subtree add up to at most its root's.
- **Overseers** of charter `c`, for a sender `s`: `s` is an overseer if
  `s = c.backer` or `s = c.principal`; otherwise load `c.parent`, then its
  parent, and so on (each MUST be a charter), stopping as soon as one has
  `agent = s` (an overseer), or has no parent, or `MAX_CHARTER_CHAIN − 1`
  have been loaded (not an overseer).
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
     `spent_in_window + amount > k.max_per_window − carved_window` fails; if
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
   `attenuates(caveats, k)`; **carve**(allotment, k,
   `caveats.max_per_window`).
5. `remaining -= amount` (error if insufficient); store the allotment.
6. `child_id = created_id(digest, next_index)`;
   `create(Shared(object_domain("charter", child_id)), Charter { principal:
   sender, agent: sub_agent, caveats, agent_atom: parent.agent_atom,
   backer: parent.backer, revoked_at: None, parent: Some(charter),
   allotments: ∅, created_at: ctx.pulse, suspended_by: None, identity:
   None })`.
7. `child_allotment = create(Account(sub_agent), Allotment { source:
   child_id, writer: sub_agent, asset: USD, remaining: amount, caveats:
   Some(caveats), window_start: ctx.pulse, spent_in_window: 0, carved_window: 0 })`.
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
   registry or not `Active`, and otherwise `sion_assurance::decide(registry,
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

Load it (MUST be a charter); the sender MUST be its `agent` (an agent may
retire itself) or one of its **overseers** (§5.13): the agent is compared
first, so the overseer search loads nothing when the agent revokes. If
`revoked_at` is unset set it to `ctx.pulse`; store; event `CharterRevoked
{ charter }`. Every charter below a revoked one fails its chain check, so
one revocation stops a whole subtree.

### 5.17 ReclaimCharter { charter }

1. Load it (MUST be a charter, call it `s`); the sender MUST be
   `s.principal` or `s.backer`.
2. The charter MUST be dead: `s.revoked_at` set, or `ctx.pulse >
   s.caveats.expires_at`, or its **chain check ignoring suspension** fails.
   (Evaluate in this order and stop at the first that holds; the chain
   check is only run if the first two do not hold.) A suspended charter is
   not dead: its funds wait for the suspension to be lifted or for a
   revocation.
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
   caveats: None, window_start: ctx.pulse, spent_in_window: 0, carved_window: 0 })`; record
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
caveats: None, window_start: ctx.pulse, spent_in_window: 0, carved_window: 0 })`; event
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
5. For `(asset, forfeited)` in `[(params.stake_asset, stake + slashed),
   (USD, rewards)]`, in that order: `paid = permille_of(forfeited,
   params.reporter_permille)`; `mint(sender, asset, paid)`; if `forfeited −
   paid > 0`, defer `Treasury { asset, amount: forfeited − paid }`.
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
4. `tx.fee ≥ ctx.config.min_fee(tx.size()) + hashes × STREAM_STEP_FEE` (the
   arithmetic saturating).
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
and budget.

1. `1 ≤ |children| ≤ MAX_SPAWN_BATCH` (256,
   `sion_exec::charter::MAX_SPAWN_BATCH`).
2. `allotment_id = tx.inputs[1].id` (error if absent); load it (MUST be an
   allotment).
3. Load `charter` (MUST be a charter, call it `p`); **chain check** it,
   yielding `n`; `n < MAX_CHARTER_CHAIN` ("too deep").
4. The allotment's `source` MUST be `charter`; its `writer` and `p.agent`
   MUST be the sender.
5. The allotment MUST have caveats `k`; `k.may_delegate`.
6. For each child, in order: `attenuates(child.caveats, k)`; then
   **carve**(allotment, k, `child.caveats.max_per_window`); then
   `remaining −= child.budget` (error if insufficient).
7. Store the allotment.
8. For each child, in order:
   1. `child_id = created_id(digest, next_index)`;
      `create(Shared(object_domain("charter", child_id)), Charter {
      principal: sender, agent: child.agent, caveats: child.caveats,
      agent_atom: p.agent_atom, backer: p.backer, revoked_at: None, parent:
      Some(charter), allotments: ∅, created_at: ctx.pulse, suspended_by:
      None, identity: Some(AgentIdentity { card: child.card, depth: n })
      })`.
   2. `a = create(Account(child.agent), Allotment { source: child_id,
      writer: child.agent, asset: USD, remaining: child.budget, caveats:
      Some(child.caveats), window_start: ctx.pulse, spent_in_window: 0, carved_window: 0 })`.
   3. Load the child charter, insert `a` into `allotments`, store it.
   4. Event `AgentSpawned { agent: child_id, parent: charter, account:
      child.agent, allotment: a }`.

Siblings share nothing they write: each spends from its own allotment, so
their payments need no consensus.

A suspension or revocation takes effect when it executes. An agent's
payment certified before then, but executed after, fails: for agent
operations a lane certificate guarantees that the inputs are locked, not
that the operation will succeed.

### 5.37 SetAgentCard { charter, card }

Contended, in the charter's domain. Load `charter` (MUST be a charter with
`identity = Some(i)`); the sender MUST be its `agent`; **chain check** it;
`i.card = card`; store; event `AgentCardSet { agent: charter, card }`.

### 5.38 SuspendAgent { charter }

Contended, in the charter's domain. Load it (MUST be a charter); the
sender MUST be one of its **overseers** (§5.13). If `suspended_by` is
unset, set it to the sender (the first suspension is kept); store; event
`AgentSuspended { agent: charter, by: suspended_by }`. Until the
suspension is lifted, the chain check of the charter and of every charter
below it fails, so nothing in the subtree can pay or spawn.

### 5.39 ResumeAgent { charter }

Contended, in the charter's domain. Load it (MUST be a charter);
`suspended_by = Some(s)` ("not suspended" otherwise); the sender MUST be
`s` or the charter's `backer`; set `suspended_by = None`; store; event
`AgentResumed { agent: charter }`.

### 5.40 OpenMemory { agent, policy }

A **memory log** commits its writer to a log of memory atoms that only the
writer appends to, kept off the ledger. The ledger keeps the log's head:
its `size` in atoms, its `root`, the Merkle tree hash of `sion_core::merkle`
(RFC 9162 shape) over leaf hashes the writer computes, and two roots it
stores without checking, `index` and `manifest`. Constant of
`sion_exec::memory`: `MAX_CONSISTENCY_PROOF` (128).

1. If `agent = Some(a)`: load `a` (MUST be a charter with `identity` set);
   its `agent` MUST be the sender; **chain check** it.
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
2. If `l.agent = Some(a)`: **chain check** `a`.
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
   MAX_PERIOD`.
2. Funding:
   - `charter = None`: `draw(USD, amount)`; `funder = None`; `refund_to =
     sender`.
   - `charter = Some(c)`: `allotment_id = tx.inputs[1].id` (error if
     absent); load it (MUST be an allotment), then load `c` (MUST be a
     charter); **chain check** `c`; the allotment's `source` MUST be `c`,
     and its `writer` and `c.agent` MUST be the sender; it MUST have
     caveats `k`. If `worker = None`, `k.payees` MUST be `None`: an agent
     limited to listed payees cannot post work anyone may take.
     `evidence_ok = (acceptance.policy = k.evidence_policy)`;
     **spend**(allotment, `worker` or, for an open task, the sender,
     `amount`, `Some(evidence_ok)`) (§5.13), so the escrow counts against
     the agent's balance, payees, per-payment and window limits, and a task
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
4. `|attestations| ≤ MAX_VERIFIERS`; their verifiers are distinct. For
   each `(v, body)`: `v ∈ t.acceptance.verifiers`; `a = claim_id(v,
   body)` is Active in the registry; `body.schema = ATTESTATION_SCHEMA`;
   `body.payload = result.payload`.
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
claim. A worker therefore needs a task bond, however small, to issue a
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
book, obligations, schedule. Other objects (bounded totals, coins)
are stored when changed. Reads of other objects in this phase are
`try_load`s.

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
   `MAX_INPUTS` inputs with distinct ids; `tx.fee ≥
   ctx.config.min_fee(tx.size())`;
2. `try_load(well_known::account(sender))`: if present it MUST be an
   `Account`, giving `policy` and `rotations`; if absent,
   `tx.genesis_policy` MUST be present, pass `validate`, and hash to the
   sender, giving that policy and `rotations = 0`;
3. `policy.verify(signing_context(ctx.config), d bytes, t.auth)` passes
   (always checked);
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
deadline, missed: 0 }`; schedule `ForcedDeadline { intent: d }` at
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
  then `try_load(well_known::account(sender))` gives the same `rotations`
  (absent counts as 0; present but not an `Account` counts as not
  executable). If it is not, remove it from pending; event
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
  unbonding delegations included; the reporter's share is of both. New
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
