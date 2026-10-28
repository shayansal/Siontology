//! The reporter's share of an equivocation's slashing (specification
//! §5.30, as settled in v1.11): a share of the offender's own stake and
//! rewards, never of its pool's slashed tenth. Paying a share of the pool
//! made reporting yourself profitable once your pool was about 190 times
//! your stake; with it gone, reporting yourself always costs most of your
//! own stake.

use sion_consensus::{Authenticator, Header, SignedHeader};
use sion_core::encode::encode;
use sion_core::hash::Hash32;
use sion_core::ids::{AssetId, Epoch, Round, ShardId, ValidatorId};
use sion_core::Amount;
use sion_crypto::{validator_id, DualKeypair, VALIDATOR_DOMAIN};
use sion_exec::sim::{Chain, SIM_STAKE};
use sion_exec::validators::{permille_of, Equivocation, ForcedParams, ValidatorParams};
use sion_exec::{Event, ExecStatus, TxKind};

#[derive(Clone)]
struct KeyAuth(std::sync::Arc<DualKeypair>);

impl Authenticator for KeyAuth {
    fn validator(&self) -> ValidatorId {
        validator_id(self.0.lattice.public(), self.0.hash.public())
    }
    fn sign(&self, message: &[u8]) -> Vec<u8> {
        encode(
            &self
                .0
                .sign_deterministic(VALIDATOR_DOMAIN, message)
                .unwrap(),
        )
    }
}

fn params() -> ValidatorParams {
    ValidatorParams {
        stake_asset: AssetId::usd(),
        epoch_length: 1_000,
        min_stake: Amount::units(1_000),
        unbonding: 5,
        seats: 2,
        max_active: 10,
        reward_permille: 500,
        reporter_permille: 50,
    }
}

#[test]
fn reporting_yourself_never_pays_and_the_pools_tenth_goes_to_the_treasury() {
    let mut chain = Chain::with_params(
        &["delegator", "accomplice"],
        1_000_000,
        4,
        params(),
        ForcedParams {
            window: 8,
            max_pending: 16,
        },
    );
    let auth = KeyAuth(std::sync::Arc::new(chain.validator_keys[0].clone()));
    let offender = auth.validator();

    // A delegation as large as the cap allows.
    let delegated = Amount::units(SIM_STAKE);
    let tx = chain
        .tx(
            "delegator",
            TxKind::DelegateStake {
                validator: offender,
                amount: delegated,
            },
        )
        .unwrap();
    let out = chain.pulse(vec![tx]).unwrap();
    assert!(
        out.effects.iter().all(|e| e.succeeded()),
        "{:?}",
        out.effects
    );

    // The operator signs two headers for one round and has its accomplice
    // report them.
    let header = |payload: &[u8]| {
        let header = Header {
            author: offender,
            chain: Hash32([1; 32]),
            round: Round(7),
            epoch: Epoch(0),
            shard: ShardId(0),
            parents: Vec::new(),
            payload: vec![payload.to_vec()],
        };
        SignedHeader {
            signature: auth.sign(&Header::signing_digest(&header.digest()).0),
            header,
        }
    };
    let treasury_before = chain
        .state
        .treasury()
        .balances
        .get(&AssetId::usd())
        .copied()
        .unwrap_or(Amount::ZERO);
    let report = chain
        .tx(
            "accomplice",
            TxKind::ReportEquivocation {
                evidence: Box::new(Equivocation::Headers {
                    first: header(b"a"),
                    second: header(b"b"),
                }),
            },
        )
        .unwrap();
    let out = chain.pulse(vec![report]).unwrap();
    assert_eq!(out.effects[0].status, ExecStatus::Success);
    let events: Vec<Event> = out.effects[0].events.clone();
    let (stake, reward) = events
        .iter()
        .find_map(|e| match e {
            Event::ValidatorSlashed { amount, reward, .. } => Some((*amount, *reward)),
            _ => None,
        })
        .unwrap();
    let pool = events
        .iter()
        .find_map(|e| match e {
            Event::PoolSlashed { amount, .. } => Some(*amount),
            _ => None,
        })
        .unwrap();
    assert_eq!(stake, Amount::units(SIM_STAKE));
    assert_eq!(pool, permille_of(delegated, 100));
    // The reporter's share is of the operator's own stake only.
    assert_eq!(reward, permille_of(stake, 50));
    // Operator and accomplice together lose most of the operator's stake.
    assert!(reward < stake);
    // Everything else, the pool's tenth included, went to the treasury.
    let treasury_after = chain.state.treasury().balances[&AssetId::usd()];
    let gained = treasury_after.checked_sub(treasury_before).unwrap();
    assert!(
        gained
            >= stake
                .checked_add(pool)
                .unwrap()
                .checked_sub(reward)
                .unwrap(),
        "treasury gained {gained}"
    );
}
