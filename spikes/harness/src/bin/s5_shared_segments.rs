//! S5: can two program headers share one uploaded segment chain, and are the two instances
//! independent (own shards, own PDAs)?
//!
//! `cargo run --release -p spike-harness --bin s5_shared_segments`

use lee::{
    Account, AccountId, PrivateKey, PublicKey, PublicTransaction, V03State,
    native_token::NATIVE_TOKEN_PROGRAM_ID,
    public_transaction::{Message, WitnessSet},
};
use lee_core::program::{PROGRAM_LOADER_ACCOUNT_ID, ProgramHeader, ProgramSegment};
use program_loader_core::Instruction as LoaderInstruction;
use spike_guest::s1::{Instruction, VAULT_SEED};
use spike_harness::{id, program, run, sel, tx};
use spike_methods::S1_GAS_ELF;

fn header(state: &V03State, at: AccountId) -> Option<ProgramHeader> {
    ProgramHeader::from_bytes(state.get_account_by_id(at).data.shard(PROGRAM_LOADER_ACCOUNT_ID))
}

fn chain(state: &V03State, first: AccountId) -> Vec<AccountId> {
    let mut out = vec![first];
    loop {
        let last = *out.last().unwrap();
        let shard = state.get_account_by_id(last).data.shard(PROGRAM_LOADER_ACCOUNT_ID).clone();
        match ProgramSegment::from_bytes(&shard).unwrap().next_segment {
            Some(next) => out.push(next),
            None => return out,
        }
    }
}

fn create_header_tx(state: &V03State, key: &PrivateKey, segments: &[AccountId]) -> (AccountId, PublicTransaction) {
    let target = AccountId::from(&PublicKey::new_from_private_key(key));
    let mut ids = vec![target];
    ids.extend_from_slice(segments);
    let message = Message::try_new(
        PROGRAM_LOADER_ACCOUNT_ID,
        ids.into_iter().map(|id| sel(id, PROGRAM_LOADER_ACCOUNT_ID)).collect(),
        vec![state.get_account_by_id(target).nonce],
        LoaderInstruction::CreateHeader {
            first_segment: segments[0],
            immutable: true,
        },
    )
    .unwrap();
    let witness_set = WitnessSet::for_message(&message, &[key]);
    (target, PublicTransaction::new(message, witness_set))
}

fn main() -> anyhow::Result<()> {
    let first = id(0xB1);
    let p = program(S1_GAS_ELF);
    let mut state = V03State::new().with_named_programs([(first, p.clone())]);

    let h1 = header(&state, first).unwrap();
    let segments = chain(&state, h1.program_first_segment);
    println!("instance 1 at {first}: {} segment(s), image {:?}", segments.len(), h1.image_id);

    // A second header pointing at the same chain. No bytecode in the tx.
    let key = PrivateKey::try_new([0x42; 32]).unwrap();
    let (second, create) = create_header_tx(&state, &key, &segments);
    let cycles = run(&mut state, &create)?;
    let h2 = header(&state, second).unwrap();
    println!(
        "instance 2 at {second}: CreateHeader tx {} bytes, {cycles} guest cycles",
        borsh::to_vec(&create)?.len()
    );
    assert_eq!(h2.program_first_segment, h1.program_first_segment);
    assert_eq!(h2.image_id, h1.image_id, "same bytecode → same image id");

    // Both dispatch, and keep separate shards on the same account.
    let a = id(1);
    run(&mut state, &tx(first, vec![sel(a, first)], Instruction::Fill { len: 10 }))?;
    run(&mut state, &tx(second, vec![sel(a, second)], Instruction::Fill { len: 20 }))?;
    let acc = state.get_account_by_id(a);
    assert_eq!(acc.data.shard(first).len(), 10);
    assert_eq!(acc.data.shard(second).len(), 20);
    println!("both instances run; shards on one account are separate (10 B vs 20 B)");

    // Separate PDAs: each instance moves only its own vault.
    let (v1, v2) = (
        AccountId::for_public_pda(&first, &VAULT_SEED),
        AccountId::for_public_pda(&second, &VAULT_SEED),
    );
    assert_ne!(v1, v2);
    for v in [v1, v2] {
        state = state.with_public_accounts([(v, Account::funded(100))]);
    }
    let to = id(4);
    let pay = |program, vault| {
        tx(
            program,
            vec![sel(vault, NATIVE_TOKEN_PROGRAM_ID), sel(to, NATIVE_TOKEN_PROGRAM_ID)],
            Instruction::Op { touch: 0, inspect: false, native_transfer: 7, guest_call: None },
        )
    };
    run(&mut state, &pay(second, v2))?;
    assert_eq!(state.get_account_by_id(v2).data.native_balance()?, 93);
    assert_eq!(state.get_account_by_id(v1).data.native_balance()?, 100);
    // The guest asserts vault == PDA(self, seed), so instance 2 naming instance 1's vault fails.
    assert!(run(&mut state, &pay(second, v1)).is_err());
    println!("PDAs differ; instance 2 moved its own vault only");

    // Nobody can re-point instance 1 (immutable) or rewrite a shared segment.
    let rewrite = Message::try_new(
        PROGRAM_LOADER_ACCOUNT_ID,
        vec![sel(segments[0], PROGRAM_LOADER_ACCOUNT_ID)],
        vec![],
        LoaderInstruction::WriteSegment { bytecode: vec![0; 8], next_segment: None },
    )?;
    let rewrite = PublicTransaction::new(rewrite.clone(), WitnessSet::for_message(&rewrite, &[]));
    let err = run(&mut state, &rewrite).unwrap_err();
    println!("rewriting a shared segment fails: {:#}", err);

    println!("S5: GO");
    Ok(())
}
