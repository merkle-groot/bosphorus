//! Helpers for running spike transactions against an in-memory `V03State`.

use std::borrow::Cow;

use anyhow::{Context as _, Result};
use borsh::BorshSerialize;
use lee::{
    Account, AccountId, ProgramShardSelector, PublicTransaction, V03State, ValidatedStateDiff,
    program::Program,
    public_transaction::{Message, WitnessSet},
};
use lee_core::{
    AuthorizationSecretKey, Commitment, DUMMY_COMMITMENT_HASH, Identifier, NullifierPublicKey,
    NullifierSecretKey, NullifierWitness, PrivateWitness, WitnessKind,
    encryption::ViewingPublicKey,
};

pub fn program(elf: &'static [u8]) -> Program {
    Program::new(Cow::Borrowed(elf)).expect("spike guest is a valid ProgramBinary")
}

/// An arbitrary, recognisable account id.
pub fn id(tag: u8) -> AccountId {
    AccountId::new([tag; 32])
}

pub fn sel(account: AccountId, program: AccountId) -> ProgramShardSelector {
    ProgramShardSelector::new(account, program)
}

/// Unsigned, fee-exempt public tx. Fees are charged by the sequencer, not `V03State`.
pub fn tx<I: BorshSerialize>(
    program: AccountId,
    selectors: Vec<ProgramShardSelector>,
    instruction: I,
) -> PublicTransaction {
    let message = Message::try_new(program, selectors, vec![], instruction).unwrap();
    let witness_set = WitnessSet::for_message(&message, &[]);
    PublicTransaction::new(message, witness_set)
}

/// Executes `tx` at (block 1, timestamp 0), applies it, and returns the charged cycles.
pub fn run(state: &mut V03State, tx: &PublicTransaction) -> Result<u64> {
    run_at(state, tx, 1, 0)
}

pub fn run_at(
    state: &mut V03State,
    tx: &PublicTransaction,
    block: u64,
    timestamp: u64,
) -> Result<u64> {
    let (diff, charge) = ValidatedStateDiff::from_public_transaction_with_cycle_budget(
        tx,
        state,
        block,
        timestamp,
        lee::program::DEFAULT_PUBLIC_CYCLE_BUDGET,
    )
    .context("tx failed")?;
    drop(state.apply_state_diff(diff));
    Ok(charge.cycles)
}

/// Mirrors LEZ's test-only `TestPrivateKeys`.
pub struct Keys {
    ask: AuthorizationSecretKey,
    d: [u8; 32],
    z: [u8; 32],
}

impl Keys {
    pub fn new(seed: u8) -> Self {
        Self { ask: AuthorizationSecretKey([seed; 32]), d: [seed + 1; 32], z: [seed + 2; 32] }
    }
    pub fn nsk(&self) -> NullifierSecretKey {
        (&self.ask).into()
    }
    pub fn npk(&self) -> NullifierPublicKey {
        NullifierPublicKey::from(&self.nsk())
    }
    pub fn vpk(&self) -> ViewingPublicKey {
        ViewingPublicKey::from_seed(&self.d, &self.z)
    }
    pub fn id(&self) -> AccountId {
        AccountId::for_regular_private_account(&self.npk(), &self.vpk(), Identifier::ZERO)
    }
    /// Spend an existing private account; `ask: None` means "not authorised".
    pub fn update(&self, account: &Account, state: &V03State, ask: bool) -> PrivateWitness {
        let commitment = Commitment::new(&self.id(), account);
        PrivateWitness {
            vpk: self.vpk(),
            random_seed: [7; 32],
            identifier: Identifier::ZERO,
            kind: WitnessKind::Regular { ask: ask.then(|| self.ask.clone()) },
            nullifier: NullifierWitness::Update {
                account: account.clone(),
                view_tag: 0,
                nsk: self.nsk(),
                membership_proof: state.get_proof_for_commitment(&commitment).expect("in state"),
            },
        }
    }
    /// A fresh private account receiving a credit; no authorisation needed.
    pub fn init(&self) -> PrivateWitness {
        PrivateWitness {
            vpk: self.vpk(),
            random_seed: [8; 32],
            identifier: Identifier::ZERO,
            kind: WitnessKind::Regular { ask: None },
            nullifier: NullifierWitness::Init { npk: self.npk(), commitment_root: DUMMY_COMMITMENT_HASH },
        }
    }
}
