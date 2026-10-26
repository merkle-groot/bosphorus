//! Private accounts for in-process tests: keys (LEZ's test-only `TestPrivateKeys`, as in the S2
//! spike), funding, and proving/settling a privacy-preserving tx against `V03State`.
//! Proofs are real unless `RISC0_DEV_MODE=1`.

use std::{borrow::Cow, collections::HashMap};

use anyhow::Result;
use borsh::BorshSerialize;
#[cfg(feature = "prove")]
use bosphorus_prover::execute_and_prove;
#[cfg(not(feature = "prove"))]
use lee::privacy_preserving_transaction::circuit::execute_and_prove;
use lee::{
    Account, AccountId, PrivacyPreservingTransaction,
    privacy_preserving_transaction::{
        Message, WitnessSet,
        circuit::{ProgramWithDependencies, ProvingInput},
    },
    program::Program,
};
use lee_core::{
    AuthorizationSecretKey, Commitment, Identifier, NullifierPublicKey, NullifierSecretKey, NullifierWitness,
    PrivateWitness, WitnessKind,
    account::{Nonce, ProgramShardSelector},
    encryption::ViewingPublicKey,
};

use super::Chain;

#[derive(Clone)]
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

    /// Byte strings that would identify this owner if they showed up on chain.
    pub fn identifying_bytes(&self) -> Vec<(&'static str, Vec<u8>)> {
        vec![
            ("owner account id", self.id().as_ref().to_vec()),
            ("nullifier public key", self.npk().0.to_vec()),
            ("nullifier secret key", self.nsk().to_vec()),
            ("authorisation key", self.ask.0.to_vec()),
            ("viewing key seed", self.d.to_vec()),
            ("viewing public key (first 32 bytes)", self.vpk().to_bytes()[..32].to_vec()),
        ]
    }
}

/// A private account as its owner tracks it: the keys and the current plaintext.
#[derive(Clone)]
pub struct PrivateAccount {
    pub keys: Keys,
    pub account: Account,
}

impl PrivateAccount {
    /// Spend witness with `ask` (authorised) or without.
    pub fn witness(&self, chain: &Chain, ask: bool) -> PrivateWitness {
        let commitment = Commitment::new(&self.keys.id(), &self.account);
        PrivateWitness {
            vpk: self.keys.vpk(),
            random_seed: [7; 32],
            identifier: Identifier::ZERO,
            kind: WitnessKind::Regular { ask: ask.then_some(self.keys.ask) },
            nullifier: NullifierWitness::Update {
                account: self.account.clone(),
                view_tag: 0,
                nsk: self.keys.nsk(),
                membership_proof: chain.state.get_proof_for_commitment(&commitment).expect("commitment in state"),
            },
        }
    }

    /// After a tx the owner knows its new plaintext: `update` the shards, bump the nonce. The tx
    /// must carry the matching commitment, so this also checks what the circuit paid out.
    pub fn advance(&mut self, tx: &PrivacyPreservingTransaction, update: impl FnOnce(&mut Account)) {
        let mut next = self.account.clone();
        update(&mut next);
        next.nonce = next.nonce.private_account_nonce_increment(&self.keys.nsk());
        let commitment = Commitment::new(&self.keys.id(), &next);
        assert!(tx.message.commitments().contains(&commitment), "the owner's new private state isn't what we expected");
        self.account = next;
    }
}

impl Chain {
    /// A private account that exists from genesis with these shards (e.g. token balances).
    pub fn private_account(&mut self, keys: Keys, shards: &[(AccountId, lee_core::account::ShardData)]) -> PrivateAccount {
        let id = keys.id();
        let mut account = Account { nonce: Nonce::private_account_nonce_init(&id), ..Account::default() };
        for (program, data) in shards {
            account = account.with_shard(*program, data.clone());
        }
        self.state = self.state.clone().with_private_accounts([(
            Commitment::new(&id, &account),
            lee_core::Nullifier::for_account_initialization(&id),
        )]);
        PrivateAccount { keys, account }
    }

    /// Proves a private tx calling `program`, with every program it may chain-call in `deps`.
    pub fn prove<I: BorshSerialize>(
        &self,
        program: AccountId,
        elf: &'static [u8],
        deps: &[(AccountId, &'static [u8])],
        selectors: Vec<(AccountId, AccountId)>,
        witnesses: Vec<PrivateWitness>,
        instruction: I,
    ) -> Result<PrivacyPreservingTransaction> {
        let load = |elf: &'static [u8]| Program::new(Cow::Borrowed(elf)).expect("a valid guest");
        let deps: HashMap<_, _> = deps.iter().map(|(id, elf)| (*id, load(elf))).collect();
        let with_deps = ProgramWithDependencies::new(load(elf), program, deps);
        let (output, proof) = execute_and_prove(
            ProvingInput {
                shard_selectors: selectors.into_iter().map(|(a, p)| ProgramShardSelector::new(a, p)).collect(),
                private_witnesses: witnesses,
                instruction_data: Program::serialize_instruction(instruction)?,
                ..Default::default()
            },
            &with_deps,
        )?;
        let message = Message::from_circuit_output(vec![], output);
        let witness_set = WitnessSet::for_message(&message, proof, &[]);
        Ok(PrivacyPreservingTransaction::new(message, witness_set))
    }

    /// Settles as the sequencer would: deferred public effects re-run against current state.
    pub fn settle(&mut self, tx: &PrivacyPreservingTransaction) -> Result<()> {
        let block = self.next_block();
        self.state.transition_from_privacy_preserving_transaction(tx, block, self.now)?;
        Ok(())
    }
}
