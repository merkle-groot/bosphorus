//! Calls `token` to move `amount` from `accounts[0]` to `accounts[1]`, claiming `accounts[0]` is
//! `PDA(self, seed)`. The runtime authorises the debit only if that's true.

use lee_core::{
    account::AccountId,
    program::{Plan, PlanInput, PdaSeed, run_program},
};
use bosphorus_token::custody_transfer;

fn main() {
    run_program(plan, |_: (), _: &[u8]| -> Option<Vec<u8>> { None })
}

fn plan(input: &PlanInput, (token, seed, amount): (AccountId, [u8; 32], u128)) -> Plan {
    let [from, to] = input.accounts.as_slice() else { panic!("takes [from, to]") };
    let mut plan = Plan::new(input);
    plan.call(custody_transfer(token, from.account_id, PdaSeed::new(seed), to.account_id, amount));
    plan
}
