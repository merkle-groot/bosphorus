//! S6: events from public vs private txs, and what a keeper can find without them.
//!
//! `RISC0_DEV_MODE=1 cargo run --release -p spike-harness --bin s6_events`

use std::collections::HashMap;

use anyhow::Result;
use lee::{
    Account, AccountId, PrivacyPreservingTransaction, PrivateKey, PublicKey, PublicTransaction,
    V03State, ValidatedStateDiff,
    native_token::NATIVE_TOKEN_PROGRAM_ID,
    privacy_preserving_transaction::{
        self as ppt,
        circuit::{ProgramWithDependencies, ProvingInput, execute_and_prove},
    },
    program::Program,
    public_transaction::{Message, WitnessSet},
};
use lee_core::{Commitment, Nullifier, account::Nonce};
use spike_guest::s2::{Instruction, POSITION_UPDATED, VAULT_SEED, position_id};
use spike_harness::{Keys, id, program, sel};
use spike_methods::S2_PRIVATE_ELF;

fn borrow(amount: u128) -> Instruction {
    Instruction::Borrow { salt: [0x5A; 32], amount, t: 0, w: 1_000 }
}

fn main() -> Result<()> {
    let lending = id(0xC0);
    let p = program(S2_PRIVATE_ELF);
    let vault = AccountId::for_public_pda(&lending, &VAULT_SEED);
    let public_key = PrivateKey::try_new([0x33; 32])?;
    let public_owner = AccountId::from(&PublicKey::new_from_private_key(&public_key));
    let private_owner = Keys::new(10);
    let private_account =
        Account { nonce: Nonce::private_account_nonce_init(&private_owner.id()), ..Account::funded(5) };
    let state = V03State::new()
        .with_named_programs([(lending, p.clone())])
        .with_public_accounts([(vault, Account::funded(1_000)), (public_owner, Account::funded(1))])
        .with_private_accounts([(
            Commitment::new(&private_owner.id(), &private_account),
            Nullifier::for_account_initialization(&private_owner.id()),
        )]);

    // Public owner: same program, same plan (which emits PositionUpdated).
    let position = position_id(&lending, &public_owner, &[0x5A; 32]);
    let message = Message::try_new(
        lending,
        vec![sel(public_owner, NATIVE_TOKEN_PROGRAM_ID), sel(position, lending), sel(vault, NATIVE_TOKEN_PROGRAM_ID)],
        vec![state.get_account_by_id(public_owner).nonce],
        borrow(10),
    )?;
    let tx = PublicTransaction::new(message.clone(), WitnessSet::for_message(&message, &[&public_key]));
    let diff = ValidatedStateDiff::from_public_transaction(&tx, &state, 1, 0)?;
    let events = state.clone().apply_state_diff(diff);
    println!("public tx:  {} event(s)", events.len());
    for e in &events {
        assert_eq!(e.account_id, lending);
        assert_eq!(e.event.selector, POSITION_UPDATED);
        let (pos, amount): (AccountId, u128) = borsh::from_slice(&e.event.data)?;
        println!("  emitter = lending, selector = {:?}, data = (position {pos}, {amount})", e.event.selector);
    }

    // Private owner: the plan emits the same event inside the proof.
    let position = position_id(&lending, &private_owner.id(), &[0x5A; 32]);
    let (output, proof) = execute_and_prove(
        ProvingInput {
            shard_selectors: vec![
                sel(private_owner.id(), NATIVE_TOKEN_PROGRAM_ID),
                sel(position, lending),
                sel(vault, NATIVE_TOKEN_PROGRAM_ID),
            ],
            private_witnesses: vec![private_owner.update(&private_account, &state, true)],
            instruction_data: Program::serialize_instruction(borrow(10))?,
            ..Default::default()
        },
        &ProgramWithDependencies::new(p, lending, HashMap::new()),
    )?;
    let message = ppt::Message::from_circuit_output(vec![], output);
    let tx = PrivacyPreservingTransaction::new(message.clone(), ppt::WitnessSet::for_message(&message, proof, &[]));
    let diff = ValidatedStateDiff::from_privacy_preserving_transaction(&tx, &state, 1, 0)?;
    let events = state.clone().apply_state_diff(diff);
    println!("private tx: {} event(s) (proved and settled fine; the event is dropped)", events.len());
    assert!(events.is_empty());

    // What a keeper can still see in the private tx body.
    let touched: Vec<_> = tx
        .message
        .public_actions
        .iter()
        .filter(|a| a.effects.iter().any(|e| e.shard_program_account_id == lending))
        .map(|a| a.account_id)
        .collect();
    assert_eq!(touched, vec![position]);
    println!("keeper view of the private tx: lending shards touched on {touched:?} (= the position)");
    println!("S6: private txs emit no events; discover positions from tx bodies instead");
    Ok(())
}
