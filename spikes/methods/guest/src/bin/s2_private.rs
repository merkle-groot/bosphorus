use lee_core::{
    native_token::custody_transfer,
    program::{Plan, PlanInput, ProgramEvent, TimestampValidityWindow, run_program},
};
use spike_guest::s2::{Effect, Instruction, POSITION_UPDATED, VAULT_SEED, position_id};

fn main() {
    run_program(plan, apply)
}

fn plan(input: &PlanInput, instruction: Instruction) -> Plan {
    let Instruction::Borrow { salt, amount, t, w } = instruction;
    // Three accounts: pay out to the owner itself (it can't be listed twice).
    let (owner, position, vault, to) = match input.accounts.as_slice() {
        [owner, position, vault] => (owner, position, vault, owner),
        [owner, position, vault, to] => (owner, position, vault, to),
        _ => panic!("expected [owner, position, vault, (to)]"),
    };
    // Design §4: ownership by derivation. Nothing about the owner is stored or emitted.
    assert!(owner.is_authorized, "owner must authorise");
    assert_eq!(
        position.account_id,
        position_id(&input.self_account_id, &owner.account_id, &salt),
        "position is not derived from this owner and salt"
    );

    let mut plan = Plan::new(input);
    plan.effect(position, &Effect::AddDebt(amount));
    // S6: does an event survive the private path?
    plan.event(ProgramEvent {
        selector: POSITION_UPDATED,
        data: borsh::to_vec(&(position.account_id, amount)).unwrap(),
    });
    plan.call(custody_transfer(vault.account_id, VAULT_SEED, to.account_id, amount));
    plan.timestamp_window(TimestampValidityWindow::try_from(t..t.checked_add(w).unwrap()).unwrap());
    plan
}

fn apply(effect: Effect, pre_data: &[u8]) -> Option<Vec<u8>> {
    let Effect::AddDebt(amount) = effect;
    let debt = if pre_data.is_empty() {
        0
    } else {
        u128::from_le_bytes(pre_data.try_into().expect("debt is a u128"))
    };
    Some((debt + amount).to_le_bytes().to_vec())
}
