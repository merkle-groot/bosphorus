//! In-process chain for the essential tests: a `V03State`, real guests, signed public txs.

#![allow(dead_code, unused_imports, reason = "each test binary uses a different subset")]

pub mod private;
pub mod world;

use std::borrow::Cow;

use anyhow::{Context as _, Result};
use borsh::BorshSerialize;
use lee::{
    AccountId, PrivateKey, PublicKey, PublicTransaction, V03State, ValidatedStateDiff,
    program::Program,
    public_transaction::{Message, WitnessSet},
};
use lee_core::{
    account::ProgramShardSelector,
    program::{PROGRAM_LOADER_ACCOUNT_ID, ProgramHeader, ProgramSegment},
};
use program_loader_core::Instruction as LoaderInstruction;

#[derive(Clone)]
pub struct Chain {
    pub state: V03State,
    /// Block timestamp (ms) for the next tx.
    pub now: u64,
    block: u64,
}

pub fn key(tag: u8) -> PrivateKey {
    PrivateKey::try_new([tag; 32]).unwrap()
}

pub fn addr(key: &PrivateKey) -> AccountId {
    AccountId::from(&PublicKey::new_from_private_key(key))
}

impl Chain {
    pub fn new() -> Self {
        Self { state: V03State::new(), now: 1_000_000, block: 1 }
    }

    /// Deploys `elf` the way a real deploy does: segments once, then a header signed by `owner`,
    /// whose address becomes the program instance's id.
    pub fn deploy(&mut self, elf: &'static [u8], owner: &PrivateKey) -> AccountId {
        // The segments get uploaded under a throwaway header; only `owner`'s header is used.
        let mut staging: [u8; 32] = addr(owner).as_ref().try_into().unwrap();
        staging[0] ^= 0xFF;
        let staging = AccountId::new(staging);
        let program = Program::new(Cow::Borrowed(elf)).unwrap();
        self.state = self.state.clone().with_named_programs([(staging, program)]);
        let first = self.header(staging).program_first_segment;
        self.instance(first, owner)
    }

    /// Another instance on segments that are already uploaded (S5).
    pub fn instance(&mut self, first_segment: AccountId, owner: &PrivateKey) -> AccountId {
        let mut selectors = vec![addr(owner)];
        selectors.extend(self.chain(first_segment));
        self.call(
            PROGRAM_LOADER_ACCOUNT_ID,
            selectors.into_iter().map(|id| (id, PROGRAM_LOADER_ACCOUNT_ID)).collect(),
            &[owner],
            LoaderInstruction::CreateHeader { first_segment, immutable: true },
        )
        .expect("create header");
        addr(owner)
    }

    pub fn header(&self, at: AccountId) -> ProgramHeader {
        ProgramHeader::from_bytes(self.state.get_account_by_id(at).data.shard(PROGRAM_LOADER_ACCOUNT_ID))
            .expect("a header")
    }

    fn chain(&self, first: AccountId) -> Vec<AccountId> {
        let mut out = vec![first];
        loop {
            let shard = self.state.get_account_by_id(*out.last().unwrap()).data.shard(PROGRAM_LOADER_ACCOUNT_ID).clone();
            match ProgramSegment::from_bytes(&shard).unwrap().next_segment {
                Some(next) => out.push(next),
                None => return out,
            }
        }
    }

    /// A public tx calling `program`; `selectors` are `(account, shard owner)`. Fee-exempt
    /// (fees are charged by the sequencer, not `V03State`). Commits only on success, and returns
    /// the charged cycles (the sequencer's gas, S1).
    pub fn call<I: BorshSerialize>(
        &mut self,
        program: AccountId,
        selectors: Vec<(AccountId, AccountId)>,
        signers: &[&PrivateKey],
        instruction: I,
    ) -> Result<u64> {
        let nonces = signers.iter().map(|k| self.state.get_account_by_id(addr(k)).nonce).collect();
        let selectors = selectors.into_iter().map(|(a, p)| ProgramShardSelector::new(a, p)).collect();
        let message = Message::try_new(program, selectors, nonces, instruction)?;
        let witness_set = WitnessSet::for_message(&message, signers);
        let tx = PublicTransaction::new(message, witness_set);
        let (diff, charge) = ValidatedStateDiff::from_public_transaction_with_cycle_budget(
            &tx,
            &self.state,
            self.block,
            self.now,
            lee::program::DEFAULT_PUBLIC_CYCLE_BUDGET,
        )
        .context("tx failed")?;
        drop(self.state.apply_state_diff(diff));
        self.block += 1;
        Ok(charge.cycles)
    }

    /// The block id for the next tx.
    pub fn next_block(&mut self) -> u64 {
        self.block += 1;
        self.block - 1
    }

    pub fn shard(&self, account: AccountId, program: AccountId) -> Vec<u8> {
        self.state.get_account_by_id(account).data.shard(program).clone().into_inner()
    }
}
