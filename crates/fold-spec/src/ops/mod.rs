//! The operations of spec §5, one function per transaction kind.
//!
//! Every function runs the kind's **apply** on a prepared transaction. An
//! operation error makes the transaction fail (§4.2); reads outside the
//! witness reject the chunk.

mod assurance;
mod bounded;
mod charter;
mod coins;
mod knowledge;
mod market;

use fold_exec::tx::TxKind;

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
    }
}

#[cfg(test)]
mod tests {
    use fold_core::Amount;

    /// The bounds this crate takes from fold-exec's data layout are the
    /// values the specification states (§4.1, §5.2, §5.13, §5.18, §5.22,
    /// §6.3).
    #[test]
    fn shared_bounds_are_the_specified_values() {
        assert_eq!(fold_exec::tx::MAX_GRANT, Amount::units(1_000_000_000));
        assert_eq!(fold_exec::market::MAX_PERIOD, 1_000_000);
        assert_eq!(fold_exec::charter::MAX_CHARTER_CHAIN, 16);
        assert_eq!(fold_exec::tx::MAX_INPUTS, 256);
        assert_eq!(fold_exec::escrow::MAX_ALLOTMENTS, 256);
        assert_eq!(fold_exec::market::MAX_BATCH_ORDERS, 1_024);
    }
}
