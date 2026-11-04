//! `faucet` guest: plan/apply glue. `plan` checks accounts and authorisation and chains the payout;
//! the config's apply checks the claimed drop and cooldown, the claimer's record checks and moves
//! the cooldown (constraints §6).

use bosphorus_faucet::{
    CLAIM_WINDOW_MS, Config, Effect, Instruction, MAX_DROPS, Record, config_id, custody_id, is_native, mint_authority,
    payout,
};
use lee_core::{
    account::ShardData,
    program::{Plan, PlanInput, TimestampValidityWindow, run_program},
};

fn main() {
    run_program(plan, apply)
}

fn plan(input: &PlanInput, instruction: Instruction) -> Plan {
    let this = &input.self_account_id;
    let accounts = input.accounts.as_slice();
    let mut plan = Plan::new(input);
    match instruction {
        Instruction::Init { drops, cooldown_ms } => {
            let [instance, config] = accounts else { panic!("init takes [instance, config]") };
            assert_eq!(instance.account_id, *this, "first account must be this program's header");
            assert!(instance.is_authorized, "the header account must sign init");
            assert_eq!(config.account_id, config_id(this), "wrong config account");
            assert!(!drops.is_empty() && drops.len() <= MAX_DROPS, "1 to {MAX_DROPS} drops");
            for (i, drop) in drops.iter().enumerate() {
                assert!(drop.amount > 0, "zero drop");
                assert!(drops[..i].iter().all(|d| d.asset != drop.asset), "an asset listed twice");
            }
            plan.effect(config, &Effect::Init(Config { drops, cooldown_ms }));
        }
        Instruction::Claim { asset, amount, cooldown_ms, t } => {
            let (claimer, config, to) = match accounts {
                [claimer, config, custody, to] if is_native(&asset) => {
                    assert_eq!(custody.account_id, custody_id(this), "wrong custody account");
                    (claimer, config, to)
                }
                [claimer, config, authority, _token_config, to] if !is_native(&asset) => {
                    // The token checks its own config account and that we are its mint authority.
                    assert_eq!(authority.account_id, mint_authority(this), "wrong mint authority");
                    (claimer, config, to)
                }
                _ => panic!("claim takes [claimer, config, mint_authority, token_config, to] or [claimer, config, custody, to]"),
            };
            assert!(claimer.is_authorized, "claimer must sign");
            assert_eq!(claimer.program_account_id, *this, "claimer must select this program's shard");
            assert_eq!(config.account_id, config_id(this), "wrong config account");
            assert!(amount > 0, "zero amount");
            let end = t.checked_add(CLAIM_WINDOW_MS).expect("t overflow");
            plan.timestamp_window(TimestampValidityWindow::try_from(t..end).expect("window > 0"));
            plan.effect(config, &Effect::CheckDrop { asset, amount, cooldown_ms });
            plan.effect(claimer, &Effect::Claimed { asset, cooldown_ms, t });
            plan.call(payout(this, asset, to.account_id, amount));
        }
    }
    plan
}

fn apply(effect: Effect, pre_data: &[u8]) -> Option<ShardData> {
    match effect {
        Effect::Init(config) => {
            assert!(pre_data.is_empty(), "faucet already initialised");
            Some(shard(borsh::to_vec(&config).unwrap()))
        }
        Effect::CheckDrop { asset, amount, cooldown_ms } => {
            let config: Config = borsh::from_slice(pre_data).expect("faucet not initialised");
            assert_eq!(config.drop_of(&asset), Some(amount), "not this asset's drop");
            assert_eq!(config.cooldown_ms, cooldown_ms, "not the cooldown");
            None
        }
        Effect::Claimed { asset, cooldown_ms, t } => {
            let mut record: Record =
                if pre_data.is_empty() { Record::default() } else { borsh::from_slice(pre_data).expect("not a faucet record") };
            assert!(t >= record.next_claim(&asset, cooldown_ms), "claimed too soon");
            match record.last.iter_mut().find(|(a, _)| *a == asset) {
                Some(entry) => entry.1 = t,
                None => record.last.push((asset, t)),
            }
            Some(shard(borsh::to_vec(&record).unwrap()))
        }
    }
}

fn shard(bytes: Vec<u8>) -> ShardData {
    ShardData::try_from(bytes).expect("fits a shard")
}
