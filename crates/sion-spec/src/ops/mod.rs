//! The operations of spec §5, one function per transaction kind.
//!
//! Every function runs the kind's **apply** on a prepared transaction. An
//! operation error makes the transaction fail (§4.2); reads outside the
//! witness reject the chunk.

mod assurance;
mod bounded;
mod charter;
mod coins;
pub(crate) mod delegation;
mod index;
mod knowledge;
mod market;
mod memory;
mod storage;
mod stream;
mod task;
mod validators;

use sion_exec::tx::TxKind;

use crate::error::Step;
use crate::tx::TxRun;

pub(crate) use charter::spend;

/// Runs the operation of `run.tx.kind`.
pub(crate) fn apply(run: &mut TxRun<'_>) -> Step {
    let tx = run.tx;
    match &tx.kind {
        TxKind::Transfer { asset, outputs } => coins::transfer(run, *asset, outputs),
        TxKind::Faucet { grants } => coins::faucet(run, grants),
        TxKind::RotatePolicy {
            policy,
            viewing_key,
        } => coins::rotate_policy(run, policy, viewing_key),
        TxKind::IssueClaim { claim } => knowledge::issue_claim(run, claim),
        TxKind::RegisterAtom { class, label } => knowledge::register_atom(run, *class, label),
        TxKind::PostBond {
            class,
            label,
            amount,
        } => knowledge::post_bond(run, *class, label, *amount),
        TxKind::Accept {
            claim,
            value_at_risk,
            policy,
            until,
            nonce,
        } => assurance::accept(run, claim, *value_at_risk, policy, *until, *nonce),
        TxKind::Release { reservation } => assurance::release(run, reservation),
        TxKind::Challenge {
            claim,
            target,
            evidence,
            deposit,
        } => assurance::challenge(run, *claim, *target, *evidence, *deposit),
        TxKind::Rule { challenge, upheld } => assurance::rule(run, challenge, *upheld),
        TxKind::WithdrawBond { atom } => assurance::withdraw_bond(run, atom),
        TxKind::CreateCharter {
            agent,
            caveats,
            funding,
            agent_bond,
        } => charter::create_charter(run, *agent, caveats, *funding, *agent_bond),
        TxKind::Delegate {
            charter,
            sub_agent,
            caveats,
            amount,
        } => charter::delegate(run, *charter, *sub_agent, caveats, *amount),
        TxKind::AgentPay {
            charter,
            payee,
            amount,
            purpose,
            evidence,
        } => charter::agent_pay(run, *charter, *payee, *amount, purpose, *evidence),
        TxKind::RevokeCharter { charter } => charter::revoke_charter(run, *charter),
        TxKind::ReclaimCharter { charter } => charter::reclaim_charter(run, *charter),
        TxKind::RegisterAgent {
            agent,
            card,
            caveats,
            funding,
            agent_bond,
        } => charter::register_agent(run, *agent, *card, caveats, *funding, *agent_bond),
        TxKind::SpawnAgents { charter, children } => charter::spawn_agents(run, *charter, children),
        TxKind::SetAgentCard { charter, card } => charter::set_agent_card(run, *charter, *card),
        TxKind::SuspendAgent { charter } => charter::suspend_agent(run, *charter),
        TxKind::ResumeAgent { charter } => charter::resume_agent(run, *charter),
        TxKind::SpawnCohort {
            charter,
            root,
            count,
            burst,
            rate,
            full,
            caveats,
        } => charter::spawn_cohort(run, *charter, *root, *count, *burst, *rate, *full, caveats),
        TxKind::MaterializeAgents { cohort, members } => {
            charter::materialize_agents(run, *cohort, members)
        }
        TxKind::NarrowAgent { charter, narrowing } => {
            charter::narrow_agent(run, *charter, narrowing)
        }
        TxKind::CreateBounded {
            label,
            asset,
            total,
            allotments,
        } => bounded::create_bounded(run, label, *asset, *total, allotments),
        TxKind::SpendAllotment { to, amount } => bounded::spend_allotment(run, *to, *amount),
        TxKind::ReturnAllotment => bounded::return_allotment(run),
        TxKind::Rebalance {
            bounded,
            holder,
            amount,
        } => bounded::rebalance(run, *bounded, *holder, *amount),
        TxKind::CreateMarket {
            label,
            good,
            numeraire,
            batch_length,
            clear_deadline,
            committee,
            threshold,
        } => market::create_market(
            run,
            market::MarketParams {
                label,
                good: *good,
                numeraire: *numeraire,
                batch_length: *batch_length,
                clear_deadline: *clear_deadline,
                committee,
                threshold: *threshold,
            },
        ),
        TxKind::SubmitOrder { market, submission } => {
            market::submit_order(run, *market, submission)
        }
        TxKind::RevealBatch {
            market,
            batch,
            shares,
        } => market::reveal_batch(run, *market, *batch, shares),
        TxKind::ClearBatch {
            market,
            batch,
            result,
        } => market::clear_batch(run, *market, *batch, result),
        TxKind::RegisterValidator {
            lattice,
            hash,
            beacon_tip,
            stake,
            possession,
        } => validators::register_validator(run, lattice, hash, *beacon_tip, *stake, possession),
        TxKind::Unstake { validator } => validators::unstake(run, validator),
        TxKind::WithdrawStake { validator } => validators::withdraw_stake(run, validator),
        TxKind::ClaimRewards { validator } => validators::claim_rewards(run, validator),
        TxKind::ReportEquivocation { evidence } => validators::report_equivocation(run, evidence),
        TxKind::OpenStream {
            payee,
            asset,
            unit,
            length,
            anchor,
            deposit,
            until,
            checkpoints,
        } => stream::open(
            run,
            stream::OpenTerms {
                payee: *payee,
                asset: *asset,
                unit: *unit,
                length: *length,
                anchor: *anchor,
                deposit: *deposit,
                until: *until,
                checkpoints: *checkpoints,
            },
        ),
        TxKind::SettleStream {
            stream,
            position,
            value,
            jump,
        } => stream::settle(run, *stream, *position, *value, jump.as_ref()),
        TxKind::TopUpStream {
            stream,
            deposit,
            until,
        } => stream::top_up(run, *stream, *deposit, *until),
        TxKind::CloseStream { stream } => stream::close(run, *stream),
        TxKind::OpenMemory { agent, policy } => memory::open(run, *agent, *policy),
        TxKind::CommitMemory {
            size,
            root,
            index,
            manifest,
            proof,
            seal,
        } => memory::commit(
            run,
            memory::NewHead {
                size: *size,
                root: *root,
                index: *index,
                manifest: *manifest,
                proof,
                seal: *seal,
            },
        ),
        TxKind::PostTask {
            spec,
            acceptance,
            worker,
            amount,
            deadline,
            charter,
        } => task::post(
            run,
            task::Posting {
                spec: *spec,
                acceptance,
                worker: *worker,
                amount: *amount,
                deadline: *deadline,
                charter: *charter,
            },
        ),
        TxKind::SettleTask {
            task,
            result,
            attestations,
        } => task::settle(run, *task, result, attestations),
        TxKind::CancelTask { task } => task::cancel(run, *task),
        TxKind::IndexClaim { key, claim } => index::index_claim(run, key, *claim),
        TxKind::DelegateStake { validator, amount } => {
            delegation::delegate_stake(run, validator, *amount)
        }
        TxKind::Undelegate => delegation::undelegate(run),
        TxKind::WithdrawDelegation => delegation::withdraw_delegation(run),
        TxKind::ClaimDelegatorRewards => delegation::claim_delegator_rewards(run),
        TxKind::SetCommission {
            validator,
            permille,
        } => delegation::set_commission(run, validator, *permille),
        TxKind::PostDeal {
            root,
            size,
            challenges,
            price,
            rounds,
            round_length,
            collateral,
            provider,
            accept_by,
            locator,
        } => storage::post(
            run,
            storage::Terms {
                root: *root,
                size: *size,
                challenges: *challenges,
                price: *price,
                rounds: *rounds,
                round_length: *round_length,
                collateral: *collateral,
                provider: *provider,
                accept_by: *accept_by,
                locator,
            },
        ),
        TxKind::AcceptDeal { deal } => storage::accept(run, *deal),
        TxKind::ProveStorage { proofs } => storage::prove(run, proofs),
        TxKind::SettleDeal { deal } => storage::settle(run, *deal),
        TxKind::CloseDeal { deal } => storage::close(run, *deal),
    }
}

#[cfg(test)]
mod tests {
    use sion_core::Amount;

    /// The bounds this crate takes from sion-exec's data layout are the
    /// values the specification states (§4.1, §5.2, §5.13, §5.18, §5.22,
    /// §5.31, §5.40, §5.42, §5.45, §5.50, §6.3).
    #[test]
    fn shared_bounds_are_the_specified_values() {
        assert_eq!(sion_exec::tx::MAX_GRANT, Amount::units(1_000_000_000));
        assert_eq!(sion_exec::market::MAX_PERIOD, 1_000_000);
        assert_eq!(sion_exec::charter::MAX_LINEAGE_DEPTH, 8);
        assert_eq!(sion_exec::cohort::MAX_COHORT, 1 << 20);
        assert_eq!(sion_exec::cohort::MAX_MATERIALIZE, 128);
        assert_eq!(sion_exec::tx::MAX_INPUTS, 256);
        assert_eq!(sion_exec::escrow::MAX_ALLOTMENTS, 256);
        assert_eq!(sion_exec::market::MAX_BATCH_ORDERS, 1_024);
        // §5.31.
        assert_eq!(sion_exec::stream::MAX_STREAM_LENGTH, 1 << 32);
        assert_eq!(sion_exec::stream::MAX_SETTLE_STEPS, 65_536);
        assert_eq!(sion_exec::stream::STREAM_STEP_FEE, Amount(2));
        assert_eq!(sion_exec::stream::STEP_DOMAIN, "sion/stream/step");
        // §5.31, v1.7.
        assert_eq!(sion_exec::stream::MAX_CHECKPOINTS, 1 << 20);
        assert_eq!(sion_exec::stream::MAX_CHECKPOINT_DEPTH, 20);
        assert_eq!(sion_exec::stream::CHECKPOINT_TAG, b"sion/stream/checkpoint");
        // §5.40, v1.9.
        assert_eq!(sion_exec::memory::MAX_CONSISTENCY_PROOF, 128);
        // §5.42, v1.10.
        assert_eq!(sion_exec::task::RESULT_SCHEMA, "task.result/v1");
        assert_eq!(sion_exec::task::ATTESTATION_SCHEMA, "task.attestation/v1");
        assert_eq!(sion_exec::task::WORKER_BOND_LABEL, "task-worker");
        assert_eq!(sion_exec::task::MAX_REQUIRED, 16);
        assert_eq!(sion_exec::task::MAX_VERIFIERS, 16);
        // §5.45, v1.11.
        assert_eq!(sion_exec::delegation::REWARD_SCALE, 1_000_000_000_000);
        assert_eq!(sion_exec::delegation::MIN_DELEGATION, Amount::units(1));
        assert_eq!(sion_exec::delegation::COMMISSION_CEILING_PERMILLE, 250);
        assert_eq!(sion_exec::delegation::COMMISSION_STEP_PERMILLE, 20);
        assert_eq!(sion_exec::delegation::DELEGATION_SLASH_PERMILLE, 100);
        assert_eq!(sion_exec::delegation::CAP_FLOOR_PERMILLE, 100);
        // §5.50, v1.12.
        assert_eq!(sion_exec::storage::MAX_DEAL_LEAVES, 1 << 40);
        assert_eq!(sion_exec::storage::MAX_LEAF_BYTES, 4_096);
        assert_eq!(sion_exec::storage::MAX_PATH, 40);
        assert_eq!(sion_exec::storage::MAX_CHALLENGES, 16);
        assert_eq!(sion_exec::storage::MAX_ROUNDS, 1_000_000);
        assert_eq!(sion_exec::storage::MIN_ROUND_LENGTH, 8);
        assert_eq!(sion_exec::storage::MAX_ROUND_LENGTH, 100_000);
        assert_eq!(sion_exec::storage::MAX_LOCATOR, 256);
        assert_eq!(sion_exec::storage::MAX_PROOF_LEAVES, 64);
        assert_eq!(sion_exec::storage::ABANDON_ROUNDS, 3);
        assert_eq!(sion_exec::storage::SEED_DOMAIN, "sion/storage/seed");
        assert_eq!(
            sion_exec::storage::CHALLENGE_DOMAIN,
            "sion/storage/challenge"
        );
        // §6.3, v1.12.
        assert_eq!(sion_exec::beacons::BEACON_INTERVAL, 8);
        assert_eq!(sion_exec::beacons::BEACON_SLOTS, 12_500);
    }
}
