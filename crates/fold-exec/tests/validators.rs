//! Validators, epochs, rewards, slashing and forced-inclusion obligations,
//! end to end on the simulated chain (every chunk replayed from its witness,
//! conservation checked after every Pulse).

use std::collections::BTreeSet;

use fold_consensus::{
    Authenticator, Committee, Header, LaneRequest, LaneValidator, OwnerAuthorization, SignedHeader,
};
use fold_core::encode::encode;
use fold_core::hash::{hash_domain, Hash32};
use fold_core::ids::{AssetId, Epoch, Pulse, Round, ShardId, ValidatorId};
use fold_core::Amount;
use fold_crypto::{validator_id, DualKeypair, Scheme, VALIDATOR_DOMAIN};
use fold_exec::sim::Chain;
use fold_exec::validators::{
    possession_digest, Equivocation, EvidenceApproval, ForcedParams, ValidatorParams,
    ValidatorStatus,
};
use fold_exec::{Event, ExecStatus, PulseOutput, SignedTransaction, TxKind};

fn usd(n: u64) -> Amount {
    Amount::units(n)
}

fn params() -> ValidatorParams {
    ValidatorParams {
        stake_asset: AssetId::usd(),
        epoch_length: 10,
        min_stake: usd(1_000),
        unbonding: 5,
        seats: 2,
        max_active: 10,
        reward_permille: 500,
        reporter_permille: 500,
    }
}

fn forced(window: u64) -> ForcedParams {
    ForcedParams {
        window,
        max_pending: 16,
    }
}

fn events(out: &PulseOutput) -> Vec<Event> {
    out.effects
        .iter()
        .flat_map(|e| e.events.iter().cloned())
        .chain(out.deferred_events.iter().cloned())
        .collect()
}

fn status(out: &PulseOutput, tx: &SignedTransaction) -> ExecStatus {
    out.effects
        .iter()
        .find(|e| e.digest == tx.digest())
        .map(|e| e.status.clone())
        .expect("executed")
}

/// Signs as a validator holding `keys` (the node's dual scheme).
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

fn register(chain: &mut Chain, operator: &str, keys: &DualKeypair, stake: Amount) -> TxKind {
    let (lattice, hash) = (keys.lattice.public().clone(), keys.hash.public().clone());
    let id = validator_id(&lattice, &hash);
    let message = possession_digest(&chain.state.config, &chain.id(operator), &id);
    let possession = encode(
        &keys
            .sign_deterministic(VALIDATOR_DOMAIN, &message.0)
            .unwrap(),
    );
    TxKind::RegisterValidator {
        lattice,
        hash,
        beacon_tip: hash_domain("test/beacon", &id.0 .0),
        stake,
        possession,
    }
}

#[test]
fn validators_join_earn_leave_and_withdraw_by_epoch() {
    let mut chain = Chain::with_params(&["alice", "bob"], 50_000, 4, params(), forced(8));
    let keys = DualKeypair::from_seed(&[7; 32], Scheme::SlhDsaSha2_128f);
    let id = validator_id(keys.lattice.public(), keys.hash.public());

    // A proof of possession must come from the validator's keys and name
    // the operator: bob cannot replay alice's.
    let kind = register(&mut chain, "alice", &keys, usd(2_000));
    let stolen = chain.tx("bob", kind.clone()).unwrap();
    let low_kind = register(&mut chain, "alice", &keys, usd(10));
    let low = chain.tx("alice", low_kind).unwrap();
    let out = chain.pulse(vec![stolen.clone(), low.clone()]).unwrap();
    assert!(matches!(status(&out, &stolen), ExecStatus::Failed(r) if r.contains("possession")));
    assert!(matches!(status(&out, &low), ExecStatus::Failed(r) if r.contains("minimum")));

    let tx = chain.tx("alice", kind).unwrap();
    let before = chain.balance("alice", &AssetId::usd());
    let out = chain.pulse(vec![tx.clone()]).unwrap();
    assert_eq!(status(&out, &tx), ExecStatus::Success);
    let fee = out.effects[0].fee;
    assert_eq!(
        chain.balance("alice", &AssetId::usd()),
        before
            .checked_sub(usd(2_000))
            .unwrap()
            .checked_sub(fee)
            .unwrap(),
        "the stake leaves alice's coins"
    );
    let record = chain.state.validators().validators[&id].clone();
    assert_eq!(record.status, ValidatorStatus::Joining);
    assert!(!chain
        .state
        .validators()
        .active
        .iter()
        .any(|(v, _)| *v == id));

    // Pulses run with fees until the first epoch ends (Pulse 10).
    for _ in 0..3 {
        let pay = chain
            .tx(
                "bob",
                TxKind::Transfer {
                    asset: AssetId::usd(),
                    outputs: vec![(chain.id("alice"), usd(1))],
                },
            )
            .unwrap();
        chain.pulse(vec![pay]).unwrap();
    }
    let epoch_end = chain.state.validators().epoch_end();
    assert_eq!(epoch_end, Pulse(10));
    chain.advance_to(Pulse(9)).unwrap();
    let out = chain.pulse(Vec::new()).unwrap();
    let changed = events(&out)
        .into_iter()
        .find_map(|e| match e {
            Event::EpochChanged {
                epoch,
                active,
                rewards,
            } => Some((epoch, active, rewards)),
            _ => None,
        })
        .expect("the epoch changed at its last Pulse");
    assert_eq!(changed.0, 1);
    assert_eq!(changed.1, 5, "four genesis validators and alice's");
    assert!(
        changed.2 > Amount::ZERO,
        "half of the epoch's fees are paid out"
    );
    let set = chain.state.validators();
    assert_eq!(set.epoch, 1);
    assert_eq!(set.epoch_start, Pulse(11));
    assert_eq!(set.validators[&id].status, ValidatorStatus::Active);
    assert!(set.active.iter().any(|(v, w)| *v == id && *w == 2_000));
    assert_eq!(
        set.validators[&id].rewards,
        Amount::ZERO,
        "alice's validator joined after the epoch it would be paid for"
    );

    // During epoch 1 it earns its share.
    let pay = chain
        .tx(
            "bob",
            TxKind::Transfer {
                asset: AssetId::usd(),
                outputs: vec![(chain.id("alice"), usd(1))],
            },
        )
        .unwrap();
    chain.pulse(vec![pay]).unwrap();
    chain.advance_to(Pulse(20)).unwrap();
    let earned = chain.state.validators().validators[&id].rewards;
    assert!(earned > Amount::ZERO);

    // Only the operator can claim or leave.
    let theft = chain
        .tx("bob", TxKind::ClaimRewards { validator: id })
        .unwrap();
    let claim = chain
        .tx("alice", TxKind::ClaimRewards { validator: id })
        .unwrap();
    let out = chain.pulse(vec![theft.clone(), claim.clone()]).unwrap();
    assert!(matches!(status(&out, &theft), ExecStatus::Failed(_)));
    assert_eq!(status(&out, &claim), ExecStatus::Success);
    assert!(events(&out).contains(&Event::RewardsClaimed {
        validator: id,
        amount: earned
    }));

    let leave = chain
        .tx("alice", TxKind::Unstake { validator: id })
        .unwrap();
    let out = chain.pulse(vec![leave.clone()]).unwrap();
    assert_eq!(status(&out, &leave), ExecStatus::Success);
    assert_eq!(
        chain.state.validators().validators[&id].status,
        ValidatorStatus::Leaving
    );
    chain.advance_to(Pulse(30)).unwrap();
    let record = chain.state.validators().validators[&id].clone();
    assert_eq!(
        record.status,
        ValidatorStatus::Exited {
            withdrawable_at: Pulse(35)
        }
    );
    assert!(!chain
        .state
        .validators()
        .active
        .iter()
        .any(|(v, _)| *v == id));

    let early = chain
        .tx("alice", TxKind::WithdrawStake { validator: id })
        .unwrap();
    let out = chain.pulse(vec![early.clone()]).unwrap();
    assert!(matches!(status(&out, &early), ExecStatus::Failed(r) if r.contains("withdrawable")));
    chain.advance_to(Pulse(35)).unwrap();
    let before = chain.balance("alice", &AssetId::usd());
    let withdraw = chain
        .tx("alice", TxKind::WithdrawStake { validator: id })
        .unwrap();
    let out = chain.pulse(vec![withdraw.clone()]).unwrap();
    assert_eq!(status(&out, &withdraw), ExecStatus::Success);
    let fee = out.effects[0].fee;
    let after = chain.balance("alice", &AssetId::usd());
    assert!(
        after.0 >= before.0 + usd(2_000).0 - fee.0,
        "the stake comes back, plus rewards accrued in epoch 2"
    );
    assert_eq!(
        chain.state.validators().validators[&id].status,
        ValidatorStatus::Withdrawn
    );
}

#[test]
fn equivocation_forfeits_the_stake_and_pays_the_reporter() {
    let mut chain = Chain::with_params(&["alice", "bob"], 50_000, 4, params(), forced(8));
    let keys = std::sync::Arc::new(chain.validator_keys[0].clone());
    let auth = KeyAuth(keys.clone());
    let offender = auth.validator();
    let stake = chain.state.validators().validators[&offender].stake;

    // Two different consensus headers for one round.
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
    let same = Equivocation::Headers {
        first: header(b"a"),
        second: header(b"a"),
    };
    let real = Equivocation::Headers {
        first: header(b"a"),
        second: header(b"b"),
    };
    let bogus = chain
        .tx(
            "bob",
            TxKind::ReportEquivocation {
                evidence: Box::new(same),
            },
        )
        .unwrap();
    let out = chain.pulse(vec![bogus.clone()]).unwrap();
    assert!(matches!(status(&out, &bogus), ExecStatus::Failed(r) if r.contains("same")));

    let before = chain.balance("bob", &AssetId::usd());
    let report = chain
        .tx(
            "bob",
            TxKind::ReportEquivocation {
                evidence: Box::new(real.clone()),
            },
        )
        .unwrap();
    let out = chain.pulse(vec![report.clone()]).unwrap();
    assert_eq!(status(&out, &report), ExecStatus::Success);
    let fee = out.effects[0].fee;
    assert_eq!(
        chain.balance("bob", &AssetId::usd()).0,
        before.0 + stake.0 / 2 - fee.0,
        "the reporter receives half of the stake"
    );
    let record = &chain.state.validators().validators[&offender];
    assert!(matches!(record.status, ValidatorStatus::Slashed { .. }));
    assert_eq!(record.stake, Amount::ZERO);
    let again = chain
        .tx(
            "alice",
            TxKind::ReportEquivocation {
                evidence: Box::new(real),
            },
        )
        .unwrap();
    let out = chain.pulse(vec![again.clone()]).unwrap();
    assert!(matches!(status(&out, &again), ExecStatus::Failed(r) if r.contains("already")));

    // A slashed validator leaves the active set at the epoch's end.
    chain.advance_to(Pulse(10)).unwrap();
    assert!(!chain
        .state
        .validators()
        .active
        .iter()
        .any(|(v, _)| *v == offender));
}

/// Owners for the evidence test: every request is authorized.
struct AnyOwner;

impl OwnerAuthorization for AnyOwner {
    fn authorizes(&self, _: &LaneRequest) -> bool {
        true
    }
}

#[test]
fn double_approvals_of_one_version_are_slashable_while_both_can_execute() {
    let mut chain = Chain::with_params(&["alice", "bob"], 50_000, 4, params(), forced(8));
    let auth = KeyAuth(std::sync::Arc::new(chain.validator_keys[1].clone()));
    let offender = auth.validator();
    // alice signs two transfers spending the same coin.
    let spend = |chain: &mut Chain, to: &str| {
        let tx = chain
            .tx(
                "alice",
                TxKind::Transfer {
                    asset: AssetId::usd(),
                    outputs: vec![(chain.id(to), usd(1))],
                },
            )
            .unwrap();
        chain.wallets.get_mut("alice").unwrap().settle();
        tx
    };
    let one = spend(&mut chain, "bob");
    let two = spend(&mut chain, "alice");
    assert_eq!(one.tx.inputs, two.tx.inputs, "both spend the same coin");
    // A Byzantine validator approves both, from two lock states.
    let committee = Committee::equal_weight(Epoch(0), ShardId(0), [offender]).unwrap();
    let approve = |tx: &SignedTransaction| {
        let mut lane = LaneValidator::new(&committee, auth.clone());
        let request = LaneRequest {
            tx: tx.digest(),
            inputs: tx
                .tx
                .inputs
                .iter()
                .map(|r| fold_consensus::ObjectRef {
                    id: r.id,
                    version: r.version,
                })
                .collect(),
            owner_proof: Vec::new(),
        };
        lane.approve(request, &AnyOwner).unwrap();
        lane.seal_batch();
        EvidenceApproval {
            tx: tx.tx.clone(),
            proof: lane.approval_proof(&tx.digest()).unwrap().clone(),
        }
    };
    let evidence = Equivocation::Approvals {
        first: Box::new(approve(&one)),
        second: Box::new(approve(&two)),
    };
    assert_eq!(
        evidence.offender(chain.state.validators(), chain.state.pulse),
        Ok(offender)
    );
    // Once one of them has expired, re-approving the version is what an
    // honest validator does, so it is no longer evidence.
    let expiry = one.tx.expires_at;
    assert!(evidence
        .offender(chain.state.validators(), Pulse(expiry.0 + 1))
        .is_err());
    let report = chain
        .tx(
            "bob",
            TxKind::ReportEquivocation {
                evidence: Box::new(evidence),
            },
        )
        .unwrap();
    let out = chain.pulse(vec![report.clone()]).unwrap();
    assert_eq!(status(&out, &report), ExecStatus::Success);
    assert!(matches!(
        chain.state.validators().validators[&offender].status,
        ValidatorStatus::Slashed { .. }
    ));
}

#[test]
fn forced_intents_are_owed_and_censorship_rotates_the_committee() {
    let mut chain = Chain::with_params(&["alice", "bob"], 50_000, 4, params(), forced(3));
    let shard = |chain: &Chain, name: &str| {
        ShardId::for_account(&chain.id(name), chain.state.config.shard_count)
    };
    let alice_shard = shard(&chain, "alice");

    // An intent nobody executes: its shard misses the deadline.
    let censored = chain
        .tx(
            "alice",
            TxKind::Transfer {
                asset: AssetId::usd(),
                outputs: vec![(chain.id("bob"), usd(5))],
            },
        )
        .unwrap();
    chain.wallets.get_mut("alice").unwrap().settle();
    let previous: BTreeSet<ValidatorId> = chain
        .state
        .validators()
        .lane_members(alice_shard)
        .unwrap()
        .into_iter()
        .collect();
    let out = chain
        .pulse_forced(Vec::new(), Vec::new(), vec![censored.clone()])
        .unwrap();
    let posted_at = out.pulse;
    let deadline = events(&out)
        .into_iter()
        .find_map(|e| match e {
            Event::ForcedPosted {
                deadline, shard, ..
            } => {
                assert_eq!(shard, alice_shard.0);
                Some(deadline)
            }
            _ => None,
        })
        .expect("posted");
    assert_eq!(deadline, Pulse(posted_at.0 + 3));
    assert_eq!(chain.state.forced().pending.len(), 1);
    chain.advance_to(Pulse(deadline.0 - 1)).unwrap();
    let out = chain.pulse(Vec::new()).unwrap();
    let evs = events(&out);
    assert!(evs.contains(&Event::ForcedOverdue {
        intent: censored.digest(),
        shard: alice_shard.0
    }));
    assert!(evs.contains(&Event::ShardRotated {
        shard: alice_shard.0,
        rotation: 1
    }));
    let now: BTreeSet<ValidatorId> = chain
        .state
        .validators()
        .lane_members(alice_shard)
        .unwrap()
        .into_iter()
        .collect();
    assert_eq!(now.len(), 2);
    assert!(
        now.is_disjoint(&previous),
        "with four validators and two seats, the replacement is entirely fresh"
    );
    let obligation = &chain.state.forced().pending[&censored.digest()];
    assert_eq!(obligation.missed, 1);
    assert_eq!(obligation.deadline, Pulse(deadline.0 + 3));

    // The new committee includes it: at the next deadline it is resolved.
    let out = chain.pulse(vec![censored.clone()]).unwrap();
    assert_eq!(status(&out, &censored), ExecStatus::Success);
    chain.advance_to(Pulse(deadline.0 + 3)).unwrap();
    assert!(chain.state.forced().pending.is_empty());
    let resolved = chain.history.iter().any(|record| {
        record
            .output
            .deferred_events
            .contains(&Event::ForcedResolved {
                intent: censored.digest(),
            })
    });
    assert!(resolved);

    // Intents that could never execute are dismissed when posted.
    let mut faucet_wallet = chain.wallets.get("faucet").unwrap().clone();
    let faucet = faucet_wallet
        .faucet(
            &chain.state,
            vec![(chain.id("bob"), AssetId::usd(), usd(1))],
        )
        .unwrap();
    let mut forged = chain
        .tx(
            "bob",
            TxKind::Transfer {
                asset: AssetId::usd(),
                outputs: vec![(chain.id("alice"), usd(1))],
            },
        )
        .unwrap();
    forged.tx.nonce += 1;
    let out = chain
        .pulse_forced(Vec::new(), Vec::new(), vec![faucet, forged.clone(), forged])
        .unwrap();
    let dismissed: Vec<String> = events(&out)
        .into_iter()
        .filter_map(|e| match e {
            Event::ForcedDismissed { reason, .. } => Some(reason),
            _ => None,
        })
        .collect();
    assert_eq!(
        dismissed.len(),
        2,
        "the duplicate is posted once: {dismissed:?}"
    );
    assert!(dismissed.iter().any(|r| r.contains("faucet")));
    assert!(dismissed.iter().any(|r| r.contains("authorization")));
    assert!(chain.state.forced().pending.is_empty());
}
