//! Spike (screened markets, 2026-11-05): a lending-shaped program whose inflow is paid from pool
//! lots in the same tx. `plan` credits `position` with the lots' sum (this program's shard) and
//! chains the pool's `Withdraw` with `to` = this program's vault, as a screened market's supply
//! would. `position` may be the holder's own account (held shares, E9): two shards of one private
//! account, `shard[pool]` debited and `shard[self]` credited.
//!
//! `[holder (signs), position, pool_config, escrow (asset), vault (asset)]`.

use bosphorus_pool::{Instruction as PoolInstruction, Spend, config_id, escrow_id};
use lee_core::{
    account::{AccountId, ProgramShardSelector},
    program::{ChainedCall, PdaSeed, Plan, PlanInput, run_program},
};

pub const VAULT_SEED: PdaSeed = PdaSeed::new([0x5C; 32]);

fn main() {
    run_program(plan, apply)
}

fn plan(input: &PlanInput, (pool, asset, spends, root): (AccountId, AccountId, Vec<Spend>, [u8; 32])) -> Plan {
    let this = &input.self_account_id;
    let [holder, position, pool_config, escrow, vault] = input.accounts.as_slice() else {
        panic!("takes [holder, position, pool_config, escrow, vault]")
    };
    // No `holder.is_authorized` check here: the spike shows the pool's own check holds through the call.
    assert_eq!(pool_config.account_id, config_id(&pool), "wrong pool config");
    assert_eq!(escrow.account_id, escrow_id(&pool, &asset), "wrong escrow");
    assert_eq!(vault.account_id, AccountId::for_public_pda(this, &VAULT_SEED), "wrong vault");
    let total = spends.iter().try_fold(0u128, |sum, s| sum.checked_add(s.amount)).expect("amount overflow");
    let mut plan = Plan::new(input);
    plan.effect(position, &total);
    let selectors = vec![
        ProgramShardSelector::new(holder.account_id, pool),
        ProgramShardSelector::new(pool_config.account_id, pool),
        ProgramShardSelector::new(escrow.account_id, asset),
        ProgramShardSelector::new(vault.account_id, asset),
    ];
    plan.call(ChainedCall::new(pool, selectors, &PoolInstruction::Withdraw { asset, spends, root }));
    plan
}

/// `shard[self]` = u128 LE: the position's shares.
fn apply(amount: u128, pre_data: &[u8]) -> Option<Vec<u8>> {
    let held = if pre_data.is_empty() { 0 } else { u128::from_le_bytes(pre_data.try_into().expect("16 bytes")) };
    Some(held.checked_add(amount).expect("overflow").to_le_bytes().to_vec())
}
