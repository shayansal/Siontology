//! Commitments a chunk statement carries (spec §7), implemented from the
//! specification text.
//!
//! The reference executor exposes the same helpers
//! (`sion_exec::pulse::{effects_leaf, chain_deferred, chain_effects_leaf,
//! steps_digest, deferred_coin_id}`); this crate computes them itself so
//! that a divergence between those helpers and §7 shows up as a
//! disagreement, and the unit tests below check that the two agree.

use sion_core::encode::encode;
use sion_core::hash::{hash_domain, hash_parts, Hash32};
use sion_core::ids::{ObjectId, Pulse, TxDigest};
use sion_core::merkle;
use sion_exec::execute::{Deferred, ExecStatus, TxEffects};
use sion_exec::tx::SignedTransaction;

/// Domain of the deferred-effects accumulator.
const DEFERRED_ACC_DOMAIN: &str = "sion/exec/deferred-acc";
/// Domain of the receipts (effects) accumulator.
const EFFECTS_ACC_DOMAIN: &str = "sion/exec/effects-acc";
/// Domain of the steps digest.
const STEPS_DOMAIN: &str = "sion/exec/steps";

/// The committed code of an outcome: 0 success, 1 failed, 2 rejected. The
/// failure message itself is never committed.
pub fn status_code(status: &ExecStatus) -> u8 {
    match status {
        ExecStatus::Success => 0,
        ExecStatus::Failed(_) => 1,
        ExecStatus::Rejected(_) => 2,
    }
}

/// The effects leaf: `merkle::leaf_hash(enc((digest, sender, class, code,
/// fee, created, mutated, consumed, events, deferred)))`.
pub fn effects_leaf(effects: &TxEffects) -> Hash32 {
    merkle::leaf_hash(&encode(&(
        effects.digest,
        effects.sender,
        effects.class,
        status_code(&effects.status),
        effects.fee,
        &effects.created,
        &effects.mutated,
        &effects.consumed,
        &effects.events,
        &effects.deferred,
    )))
}

/// Folds `acc = Hp("sion/exec/deferred-acc", [acc, enc(item)])` over `items`.
pub fn chain_deferred(acc: &Hash32, items: &[Deferred]) -> Hash32 {
    items.iter().fold(*acc, |acc, item| {
        hash_parts(DEFERRED_ACC_DOMAIN, &[&acc.0, &encode(item)])
    })
}

/// `Hp("sion/exec/effects-acc", [acc, leaf])`.
pub fn chain_effects_leaf(acc: &Hash32, leaf: &Hash32) -> Hash32 {
    hash_parts(EFFECTS_ACC_DOMAIN, &[&acc.0, &leaf.0])
}

/// `H("sion/exec/steps", enc([tx digests in order]))`.
pub fn steps_digest(transactions: &[SignedTransaction]) -> Hash32 {
    let digests: Vec<TxDigest> = transactions.iter().map(SignedTransaction::digest).collect();
    hash_domain(STEPS_DOMAIN, &encode(&digests))
}

/// The identifier of the `counter`-th coin the deferred phase of `pulse`
/// pays out: `ObjectId::derive_parts([b"deferred", pulse as 8 big-endian
/// bytes, counter as 8 big-endian bytes])` (§6.3).
pub fn deferred_coin_id(pulse: Pulse, counter: u64) -> ObjectId {
    ObjectId::derive_parts(&[b"deferred", &pulse.0.to_be_bytes(), &counter.to_be_bytes()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use sion_core::ids::{AccountId, AssetId};
    use sion_core::Amount;
    use sion_exec::execute::Event;
    use sion_exec::object::{CoordinationClass, ScheduledEvent};

    fn sample_effects(status: ExecStatus) -> TxEffects {
        TxEffects {
            digest: TxDigest::derive(b"tx"),
            sender: AccountId::derive(b"sender"),
            class: CoordinationClass::OwnedLinear,
            status,
            fee: Amount(1_234),
            created: [ObjectId::derive(b"c")].into_iter().collect(),
            mutated: [ObjectId::derive(b"m")].into_iter().collect(),
            consumed: [ObjectId::derive(b"x")].into_iter().collect(),
            events: vec![Event::PolicyRotated {
                account: AccountId::derive(b"sender"),
            }],
            deferred: sample_items(),
        }
    }

    fn sample_items() -> Vec<Deferred> {
        vec![
            Deferred::Treasury {
                asset: AssetId::usd(),
                amount: Amount(7),
            },
            Deferred::Schedule {
                at: Pulse(9),
                event: ScheduledEvent::RegistryTick,
            },
            Deferred::Fee {
                asset: AssetId::usd(),
                amount: Amount(1_234),
            },
        ]
    }

    #[test]
    fn matches_the_shared_commitment_helpers() {
        for status in [
            ExecStatus::Success,
            ExecStatus::Failed("a".into()),
            ExecStatus::Rejected("b".into()),
        ] {
            let effects = sample_effects(status.clone());
            assert_eq!(
                effects_leaf(&effects),
                sion_exec::pulse::effects_leaf(&effects)
            );
            assert_eq!(status_code(&status), sion_exec::pulse::status_code(&status));
        }
        let items = sample_items();
        let start = hash_domain("test", b"acc");
        assert_eq!(
            chain_deferred(&start, &items),
            sion_exec::pulse::chain_deferred(&start, &items)
        );
        let leaf = hash_domain("test", b"leaf");
        assert_eq!(
            chain_effects_leaf(&start, &leaf),
            sion_exec::pulse::chain_effects_leaf(&start, &leaf)
        );
        assert_eq!(steps_digest(&[]), sion_exec::pulse::steps_digest(&[]));
        assert_eq!(
            deferred_coin_id(Pulse(5), 3),
            sion_exec::pulse::deferred_coin_id(Pulse(5), 3)
        );
    }

    #[test]
    fn messages_are_not_committed() {
        let a = sample_effects(ExecStatus::Failed("one reason".into()));
        let b = sample_effects(ExecStatus::Failed("another".into()));
        assert_eq!(effects_leaf(&a), effects_leaf(&b));
        let c = sample_effects(ExecStatus::Rejected("one reason".into()));
        assert_ne!(effects_leaf(&a), effects_leaf(&c));
    }

    #[test]
    fn accumulators_fold_item_by_item() {
        let items = sample_items();
        let once = chain_deferred(&Hash32::ZERO, &items);
        let stepwise = chain_deferred(&chain_deferred(&Hash32::ZERO, &items[..1]), &items[1..]);
        assert_eq!(once, stepwise);
        assert_eq!(chain_deferred(&Hash32::ZERO, &[]), Hash32::ZERO);
    }
}
