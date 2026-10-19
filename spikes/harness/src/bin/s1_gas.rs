//! S1: charged cycles per guest plan/apply/inspect, and how they scale with shard size.
//!
//! `cargo run --release -p spike-harness --bin s1_gas`

use lee::{Account, AccountId, V03State, native_token::NATIVE_TOKEN_PROGRAM_ID};
use spike_guest::s1::{Instruction, VAULT_SEED};
use spike_harness::{id, program, run, sel, tx};
use spike_methods::S1_GAS_ELF;

const SIZES: &[u32] = &[16, 100, 500, 1_000, 2_000, 5_000, 10_000];

fn op(touch: u8, inspect: bool, native_transfer: u128, guest_call: Option<AccountId>) -> Instruction {
    Instruction::Op {
        touch,
        inspect,
        native_transfer,
        guest_call,
    }
}

fn main() -> anyhow::Result<()> {
    let lending = id(0xA0);
    let oracle = id(0xA1);
    let token = id(0xA2);
    let (a, b, feed, to, x, y) = (id(1), id(2), id(3), id(4), id(5), id(6));
    let vault = AccountId::for_public_pda(&lending, &VAULT_SEED);

    let p = program(S1_GAS_ELF);
    let mut state = V03State::new()
        .with_named_programs([(lending, p.clone()), (oracle, p.clone()), (token, p)])
        .with_public_accounts([(vault, Account::funded(1_000_000))]);

    // Token balances stay at 16 bytes (u128) whatever the protocol shard size is.
    run(&mut state, &tx(token, vec![sel(x, token), sel(y, token)], Instruction::Fill { len: 16 }))?;

    let token_transfer = run(
        &mut state,
        &tx(token, vec![sel(x, token), sel(y, token)], op(2, false, 0, None)),
    )?;
    let plan_only = run(&mut state, &tx(lending, vec![sel(a, lending)], op(0, false, 0, None)))?;
    println!("plan only (no effects):            {plan_only:>10}");
    println!("guest token transfer (16 B shards): {token_transfer:>10}");
    println!();
    println!(
        "{:>7} {:>10} {:>10} {:>12} {:>12} {:>14} | {:>9} {:>9} {:>9} {:>9}",
        "size", "p+1apply", "p+2apply", "+inspect", "+native", "+guest token", "apply", "inspect", "native", "g.call"
    );

    for &len in SIZES {
        run(&mut state, &tx(lending, vec![sel(a, lending), sel(b, lending)], Instruction::Fill { len }))?;
        run(&mut state, &tx(oracle, vec![sel(feed, oracle)], Instruction::Fill { len }))?;

        let one = run(&mut state, &tx(lending, vec![sel(a, lending)], op(1, false, 0, None)))?;
        let two = run(
            &mut state,
            &tx(lending, vec![sel(a, lending), sel(b, lending)], op(2, false, 0, None)),
        )?;
        let base = vec![sel(a, lending), sel(b, lending), sel(feed, oracle)];
        let inspect = run(&mut state, &tx(lending, base.clone(), op(2, true, 0, None)))?;

        let mut with_native = base.clone();
        with_native.extend([sel(vault, NATIVE_TOKEN_PROGRAM_ID), sel(to, NATIVE_TOKEN_PROGRAM_ID)]);
        let native = run(&mut state, &tx(lending, with_native.clone(), op(2, true, 1, None)))?;

        let mut with_guest = with_native;
        with_guest.extend([sel(x, token), sel(y, token)]);
        let full = tx(lending, with_guest, op(2, true, 1, Some(token)));
        let guest = run(&mut state, &full)?;
        if len == SIZES[0] {
            // Storage gas = serialized tx bytes. Unsigned here; a signer adds its signature + key.
            println!("(full-shape tx: {} bytes unsigned)", borsh::to_vec(&full)?.len());
        }

        println!(
            "{len:>7} {one:>10} {two:>10} {inspect:>12} {native:>12} {guest:>14} | {:>9} {:>9} {:>9} {:>9}",
            two - one,
            inspect - two,
            native as i64 - inspect as i64,
            guest - native,
        );
    }
    Ok(())
}
