use lee_core::{
    account::ShardData,
    program::{Plan, PlanInput, run_program},
};
use bosphorus_token::{Config, Effect, Instruction, config_id, decode_balance, encode_balance};

fn main() {
    run_program(plan, apply)
}

fn plan(input: &PlanInput, instruction: Instruction) -> Plan {
    let this = input.self_account_id;
    for account in &input.accounts {
        assert_eq!(account.program_account_id, this, "every account must select this token's shard");
    }
    let mut plan = Plan::new(input);
    match instruction {
        Instruction::Transfer { amount } => {
            // Same rules as the native token.
            let [from, to] = input.accounts.as_slice() else { panic!("transfer takes [from, to]") };
            assert_ne!(from.account_id, to.account_id, "transfer to self");
            assert!(from.is_authorized, "sender is not authorised");
            plan.effect(from, &Effect::Debit(amount));
            plan.effect(to, &Effect::Credit(amount));
        }
        Instruction::Init { mint_authority, decimals } => {
            let [instance, config] = input.accounts.as_slice() else { panic!("init takes [instance, config]") };
            assert_eq!(instance.account_id, this, "first account must be this token's header");
            assert!(instance.is_authorized, "the header account must sign init");
            assert_eq!(config.account_id, config_id(&this), "wrong config account");
            plan.effect(config, &Effect::Init(Config { mint_authority, decimals, total_supply: 0 }));
        }
        Instruction::Mint { amount } => {
            let [authority, config, to] = input.accounts.as_slice() else {
                panic!("mint takes [authority, config, to]")
            };
            assert!(authority.is_authorized, "mint authority must sign");
            assert_eq!(config.account_id, config_id(&this), "wrong config account");
            plan.effect(config, &Effect::Mint { authority: authority.account_id, amount });
            plan.effect(to, &Effect::Credit(amount));
        }
    }
    plan
}

fn apply(effect: Effect, pre_data: &[u8]) -> Option<ShardData> {
    match effect {
        Effect::Debit(amount) => {
            let balance = decode_balance(pre_data).expect("non-canonical balance");
            Some(encode_balance(balance.checked_sub(amount).expect("insufficient balance")))
        }
        Effect::Credit(amount) => {
            let balance = decode_balance(pre_data).expect("non-canonical balance");
            Some(encode_balance(balance.checked_add(amount).expect("balance overflow")))
        }
        Effect::Init(config) => {
            assert!(pre_data.is_empty(), "token already initialised");
            Some(shard(borsh::to_vec(&config).unwrap()))
        }
        Effect::Mint { authority, amount } => {
            let mut config: Config = borsh::from_slice(pre_data).expect("token not initialised");
            assert_eq!(authority, config.mint_authority, "not the mint authority");
            config.total_supply = config.total_supply.checked_add(amount).expect("supply overflow");
            Some(shard(borsh::to_vec(&config).unwrap()))
        }
    }
}

fn shard(bytes: Vec<u8>) -> ShardData {
    ShardData::try_from(bytes).expect("config fits a shard")
}
