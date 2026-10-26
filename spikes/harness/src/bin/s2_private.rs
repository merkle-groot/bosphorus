//! S2: in one private tx, a private owner authorises with `ask`, a public position PDA gets a
//! deferred write, and a vault PDA pays out to a private account. Also S3's private half: the
//! plan's timestamp window is enforced at settlement.
//!
//! Dev mode (fake proofs):  `RISC0_DEV_MODE=1 cargo run --release -p spike-harness --bin s2_private`
//! Real proofs (S4):        `cargo run --release -p spike-harness --bin s2_private`
//! Stage timeline (S4):     `S4_TRACE=1 cargo run --release -p spike-harness --features prove --bin s2_private`
//!   (needs the in-process prover; with `r0vm` the stages run in another process and don't log here)
//! Circuit cycle profile:  `RISC0_DEV_MODE=1 S4_ONLY_A=1 RISC0_PPROF_OUT=ppe.pb cargo run ... --features prove`
//!   then `go tool pprof -top ppe.pb`

use std::{collections::HashMap, time::Instant};

use anyhow::{Result, bail, ensure};
#[cfg(feature = "prove")]
use bosphorus_prover::execute_and_prove;
#[cfg(not(feature = "prove"))]
use lee::privacy_preserving_transaction::circuit::execute_and_prove;
use lee::{
    Account, AccountId, PrivacyPreservingTransaction, V03State,
    native_token::NATIVE_TOKEN_PROGRAM_ID,
    privacy_preserving_transaction::{
        Message, WitnessSet,
        circuit::{ProgramWithDependencies, Proof, ProvingInput},
    },
    program::Program,
};
use lee_core::{Commitment, Nullifier, PrivateWitness, account::Nonce};
use spike_guest::s2::{Instruction, VAULT_SEED, position_id};
use spike_harness::{Keys, id, program, sel};
use spike_methods::S2_PRIVATE_ELF;

struct Setup {
    lending: AccountId,
    program: Program,
    vault: AccountId,
    owner: Keys,
    owner_account: Account,
    state: V03State,
}

fn setup() -> Setup {
    let lending = id(0xC0);
    let program = program(S2_PRIVATE_ELF);
    let vault = AccountId::for_public_pda(&lending, &VAULT_SEED);
    let owner = Keys::new(10);
    let owner_account = Account { nonce: Nonce::private_account_nonce_init(&owner.id()), ..Account::funded(5) };
    let state = V03State::new()
        .with_named_programs([(lending, program.clone())])
        .with_public_accounts([(vault, Account::funded(1_000))])
        .with_private_accounts([(
            Commitment::new(&owner.id(), &owner_account),
            Nullifier::for_account_initialization(&owner.id()),
        )]);
    Setup { lending, program, vault, owner, owner_account, state }
}

fn prove(
    s: &Setup,
    position: AccountId,
    to: Option<AccountId>,
    witnesses: Vec<PrivateWitness>,
    instruction: Instruction,
) -> Result<PrivacyPreservingTransaction> {
    let mut selectors = vec![
        sel(s.owner.id(), NATIVE_TOKEN_PROGRAM_ID),
        sel(position, s.lending),
        sel(s.vault, NATIVE_TOKEN_PROGRAM_ID),
    ];
    selectors.extend(to.map(|to| sel(to, NATIVE_TOKEN_PROGRAM_ID)));
    let deps = ProgramWithDependencies::new(s.program.clone(), s.lending, HashMap::new());
    let (output, proof) = execute_and_prove(
        ProvingInput {
            shard_selectors: selectors,
            private_witnesses: witnesses,
            instruction_data: Program::serialize_instruction(instruction)?,
            ..Default::default()
        },
        &deps,
    )?;
    if let Ok(dir) = std::env::var("S4_DUMP") {
        // For a check with an unpatched verifier: the receipt and the journal it commits to.
        std::fs::write(format!("{dir}/proof.bin"), proof.clone().into_inner())?;
        std::fs::write(format!("{dir}/journal.bin"), output.to_bytes())?;
    }
    let message = Message::from_circuit_output(vec![], output);
    let witness_set = WitnessSet::for_message(&message, proof, &[]);
    Ok(PrivacyPreservingTransaction::new(message, witness_set))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn borrow(salt: [u8; 32], amount: u128) -> Instruction {
    Instruction::Borrow { salt, amount, t: 1_000, w: 500 }
}

fn main() -> Result<()> {
    if std::env::var("S4_TRACE").is_ok_and(|v| v == "1") {
        tracing_subscriber::fmt()
            .with_env_filter("risc0_zkvm=debug,risc0_circuit_rv32im=debug,risc0_circuit_recursion=debug")
            .with_timer(tracing_subscriber::fmt::time::uptime())
            .init();
    }
    let dev = std::env::var("RISC0_DEV_MODE").is_ok_and(|v| v == "1" || v == "true");
    println!("proofs: {}", if dev { "DEV MODE (fake)" } else { "REAL" });
    let salt = [0x5A; 32];

    // A: borrow to the owner's own private account.
    let mut s = setup();
    let position = position_id(&s.lending, &s.owner.id(), &salt);
    let witness = s.owner.update(&s.owner_account, &s.state, true);
    let started = Instant::now();
    let tx = prove(&s, position, None, vec![witness], borrow(salt, 40))?;
    let proving = started.elapsed();
    if !dev {
        let mut inner: risc0_zkvm::InnerReceipt = borsh::from_slice(&tx.witness_set.proof().clone().into_inner())?;
        let risc0_zkvm::InnerReceipt::Succinct(receipt) = &mut inner else {
            bail!("real-proof benchmark requires a succinct receipt");
        };
        receipt.seal[0] ^= 1;
        let bad = PrivacyPreservingTransaction::new(tx.message.clone(), WitnessSet::for_message(
            &tx.message, Proof::from_inner(borsh::to_vec(&inner)?), &[],
        ));
        ensure!(s.state.clone().transition_from_privacy_preserving_transaction(&bad, 1, 1_499).is_err(),
            "verifier accepted a corrupted proof");
    }
    let bytes = borsh::to_vec(&tx)?;
    println!("A: proved in {proving:.2?}; tx {} bytes", bytes.len());
    if std::env::var_os("S4_CLAIM").is_some() {
        let inner: risc0_zkvm::InnerReceipt = borsh::from_slice(&tx.witness_set.proof().clone().into_inner())?;
        if let risc0_zkvm::InnerReceipt::Succinct(receipt) = &inner {
            println!("   claim: {:?}", receipt.claim);
            println!("   seal {} words, claim {} bytes", receipt.seal.len(), borsh::to_vec(&receipt.claim)?.len());
        }
    }
    #[cfg(feature = "prove")]
    if std::env::var_os("R0_PROFILE").is_some() {
        let [witness, accum, constraints] = risc0_circuit_rv32im::prove::phase_profile_ns();
        println!("rv32im CPU stages: witness={:.3}s accum={:.3}s constraints={:.3}s",
            witness as f64 / 1e9, accum as f64 / 1e9, constraints as f64 / 1e9);
    }
    if std::env::var("S4_PROFILE_ONLY").is_ok_and(|v| v == "1") {
        // With `RISC0_PPROF_OUT`, the last session profiled is then the privacy circuit.
        return Ok(());
    }

    // S3 (private): window is [1000, 1500) ms.
    for ts in [999, 1_500] {
        match s.state.clone().transition_from_privacy_preserving_transaction(&tx, 1, ts) {
            Err(e) => println!("   settle at t={ts}: rejected ({e})"),
            Ok(()) => bail!("settled outside its window at t={ts}"),
        }
    }
    s.state.transition_from_privacy_preserving_transaction(&tx, 1, 1_499)?;
    println!("   settle at t=1499: ok");

    let debt = s.state.get_account_by_id(position).data.shard(s.lending).clone();
    assert_eq!(u128::from_le_bytes(debt.into_inner().try_into().unwrap()), 40);
    assert_eq!(s.state.get_account_by_id(s.vault).data.native_balance()?, 960);
    let mut expected = s.owner_account.clone();
    expected.data = Account::funded(45).data;
    expected.nonce = expected.nonce.private_account_nonce_increment(&s.owner.nsk());
    let expected = Commitment::new(&s.owner.id(), &expected);
    assert!(tx.message.commitments().contains(&expected), "owner's new commitment holds 45");
    println!("   position debt = 40, vault 1000 → 960, owner's private balance 5 → 45 (commitment matches)");

    // What landed publicly.
    let m = &tx.message;
    println!("   public accounts: {:?}", m.public_actions.iter().map(|a| a.account_id).collect::<Vec<_>>());
    println!(
        "   private actions: {} (nullifier + commitment + ciphertext); program claims: {}",
        m.private_actions.len(),
        m.program_image_claims.len()
    );
    assert!(!contains(&bytes, s.owner.id().as_ref()), "owner id leaked");
    assert!(!contains(&bytes, &salt), "salt leaked");
    println!("   byte scan: neither the owner id nor the salt appears in the tx");

    if std::env::var("S4_ONLY_A").is_ok_and(|v| v == "1") {
        println!("S2 A: proof verified and settled");
        if let Ok(limit) = std::env::var("S4_MAX_SECONDS") {
            let limit: f64 = limit.parse()?;
            ensure!(limit.is_finite() && limit > 0.0, "S4_MAX_SECONDS must be a positive finite number");
            ensure!(!dev, "a proof latency budget must use real proofs");
            ensure!(proving.as_secs_f64() < limit, "proof took {:.3}s; target is <{limit}s", proving.as_secs_f64());
        }
        return Ok(());
    }

    // B: pay out to a different, fresh private account.
    let mut s = setup();
    let recipient = Keys::new(40);
    let witnesses = vec![s.owner.update(&s.owner_account, &s.state, true), recipient.init()];
    let tx = prove(&s, position, Some(recipient.id()), witnesses, borrow(salt, 25))?;
    s.state.transition_from_privacy_preserving_transaction(&tx, 1, 1_000)?;
    assert_eq!(s.state.get_account_by_id(s.vault).data.native_balance()?, 975);
    println!("B: payout to a fresh private account: ok (vault 1000 → 975)");

    // Authorisation (must fail at proving time).
    let s = setup();
    let no_ask = s.owner.update(&s.owner_account, &s.state, false);
    match prove(&s, position, None, vec![no_ask], borrow(salt, 1)) {
        Err(e) => println!("N1: owner without ask: refused ({})", first_line(&e)),
        Ok(_) => bail!("N1: proved without the owner's ask"),
    }
    let other = position_id(&s.lending, &s.owner.id(), &[0; 32]);
    let witness = s.owner.update(&s.owner_account, &s.state, true);
    match prove(&s, other, None, vec![witness], borrow(salt, 1)) {
        Err(e) => println!("N2: position not derived from (owner, salt): refused ({})", first_line(&e)),
        Ok(_) => bail!("N2: proved against a position the salt doesn't derive"),
    }

    println!("S2: GO");
    Ok(())
}

fn first_line(e: &anyhow::Error) -> String {
    e.to_string().lines().next().unwrap_or_default().chars().take(140).collect()
}
