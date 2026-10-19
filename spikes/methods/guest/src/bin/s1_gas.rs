use lee_core::{
    account::{AccountId, ProgramShardSelector},
    native_token::custody_transfer,
    program::{ChainedCall, Plan, PlanInput, run_program},
};
use spike_guest::s1::{Effect, Instruction, VAULT_SEED};

fn main() {
    run_program(plan, apply)
}

fn plan(input: &PlanInput, instruction: Instruction) -> Plan {
    let mut plan = Plan::new(input);
    let accounts = &input.accounts;
    match instruction {
        Instruction::Fill { len } => {
            for account in accounts {
                plan.effect(account, &Effect::Fill(len));
            }
        }
        Instruction::Op {
            touch,
            inspect,
            native_transfer,
            guest_call,
        } => {
            let mut next = 0_usize;
            let mut take = || {
                let account = &accounts[next];
                next += 1;
                account
            };
            for _ in 0..touch {
                plan.effect(take(), &Effect::Touch);
            }
            if inspect {
                let feed = take();
                plan.inspect(feed, feed.program_account_id, &Effect::Inspect);
            }
            if native_transfer > 0 {
                let vault = take();
                let to = take();
                assert_eq!(
                    vault.account_id,
                    AccountId::for_public_pda(&input.self_account_id, &VAULT_SEED)
                );
                plan.call(custody_transfer(
                    vault.account_id,
                    VAULT_SEED,
                    to.account_id,
                    native_transfer,
                ));
            }
            if let Some(callee) = guest_call {
                let x = take();
                let y = take();
                plan.call(ChainedCall::new(
                    callee,
                    vec![
                        ProgramShardSelector::new(x.account_id, callee),
                        ProgramShardSelector::new(y.account_id, callee),
                    ],
                    &Instruction::Op {
                        touch: 2,
                        inspect: false,
                        native_transfer: 0,
                        guest_call: None,
                    },
                ));
            }
        }
    }
    plan
}

fn apply(effect: Effect, pre_data: &[u8]) -> Option<Vec<u8>> {
    match effect {
        Effect::Fill(len) => Some(vec![0xAB; len as usize]),
        Effect::Touch => {
            let mut data = pre_data.to_vec();
            if let Some(first) = data.first_mut() {
                *first ^= 1;
            }
            Some(data)
        }
        Effect::Inspect => {
            assert!(!pre_data.is_empty(), "inspected shard is empty");
            None
        }
    }
}
