//! `token`: a fungible token for LEZ v0.3.1, shaped like the native token (design §2.1).
//!
//! One program instance per asset; the instance's header address is the token's identity.
//! Balances live in `shard[token] = u128 LE` on the holder's own account, with the native
//! codec (`0` ⇒ empty shard), so private accounts hold them with no extra work.

use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::{AccountId, ProgramShardSelector},
    program::{ChainedCall, PdaSeed},
};
use risc0_zkvm::sha::{Impl, Sha256 as _};

pub use lee_core::native_token::{decode_balance, encode_balance};

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Instruction {
    /// Variant 0, borsh-identical to the native `Transfer`. Accounts: `[from, to]`.
    /// `from` must be authorised: a signer, or a PDA of the caller passed via `pda_seeds`.
    Transfer { amount: u128 },
    /// Accounts: `[instance, config]`. `instance` is this program's own header account and must
    /// sign, so only whoever created the header can initialise it (no front-running).
    Init { mint_authority: AccountId, decimals: u8 },
    /// Accounts: `[authority, config, to]`. `authority` signs; the config's apply checks it.
    Mint { amount: u128 },
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Effect {
    Debit(u128),
    Credit(u128),
    Init(Config),
    Mint { authority: AccountId, amount: u128 },
}

/// Lives in `PDA(instance, config_seed())`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Config {
    pub mint_authority: AccountId,
    pub decimals: u8,
    pub total_supply: u128,
}

pub fn config_seed() -> PdaSeed {
    let digest = Impl::hash_bytes(b"/bosphorus/v1/token/config");
    PdaSeed::new(digest.as_bytes().try_into().expect("sha256 is 32 bytes"))
}

pub fn config_id(token: &AccountId) -> AccountId {
    AccountId::for_public_pda(token, &config_seed())
}

/// A chained transfer out of an account the caller holds under `seed`; the custom-token twin of
/// `lee_core::native_token::custody_transfer`.
pub fn custody_transfer(
    token: AccountId,
    from: AccountId,
    seed: PdaSeed,
    to: AccountId,
    amount: u128,
) -> ChainedCall {
    ChainedCall::new(
        token,
        vec![ProgramShardSelector::new(from, token), ProgramShardSelector::new(to, token)],
        &Instruction::Transfer { amount },
    )
    .with_pda_seeds(vec![seed])
}
