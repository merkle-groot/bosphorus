//! `faucet`: test tokens and gas for newcomers (onboarding). One instance per deployment; its
//! config lists a fixed drop per asset and a cooldown. A claim pays the drop to an account of the
//! claimer's choice, at most once per cooldown per (claimer, asset):
//! - a token drop is minted: the token's `mint_authority` is this instance's mint PDA, which signs
//!   the chained `Mint` through `pda_seeds`;
//! - a native drop (gas) is paid out of this instance's native custody PDA, funded by plain native
//!   transfers.
//!
//! The claimer's last claim per asset lives in the claimer's own faucet shard, so a private
//! claimer's record stays private. A private claimer makes the claim a private tx, which is
//! fee-exempt: a fresh wallet claims its first gas with its private account, paid to its public
//! one. (Fresh accounts can always claim again; the cooldown only stops casual repeats.)

use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::{AccountId, ProgramShardSelector},
    native_token::{self, NATIVE_TOKEN_PROGRAM_ID},
    program::{ChainedCall, PdaSeed},
};
use risc0_zkvm::sha::{Impl, Sha256 as _};

/// At most this many assets per faucet (keeps the config shard small).
pub const MAX_DROPS: usize = 8;
/// A claim's timestamp window, `[t, t + CLAIM_WINDOW_MS)`: the claim must land within it.
pub const CLAIM_WINDOW_MS: u64 = 10 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Drop {
    /// A token instance, or `NATIVE_TOKEN_PROGRAM_ID` for gas.
    pub asset: AccountId,
    pub amount: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Instruction {
    /// Accounts: `[instance, config]`. `instance` is this program's own header account and must
    /// sign, so only whoever created the header can initialise it.
    Init { drops: Vec<Drop>, cooldown_ms: u64 },
    /// Accounts: a token drop `[claimer, config, mint_authority, token_config, to]`, a native drop
    /// `[claimer, config, custody, to]`. `claimer` signs and selects this program's shard; `to`
    /// receives, selecting the asset's shard. `amount` and `cooldown_ms` are
    /// claims the config checks; `t` is the claimed time, checked by the window.
    Claim { asset: AccountId, amount: u128, cooldown_ms: u64, t: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Config {
    pub drops: Vec<Drop>,
    pub cooldown_ms: u64,
}

impl Config {
    pub fn drop_of(&self, asset: &AccountId) -> Option<u128> {
        self.drops.iter().find(|d| d.asset == *asset).map(|d| d.amount)
    }
}

/// A claimer's last claim time (ms) per asset, in the claimer's faucet shard.
#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Record {
    pub last: Vec<(AccountId, u64)>,
}

impl Record {
    pub fn last_claim(&self, asset: &AccountId) -> Option<u64> {
        self.last.iter().find(|(a, _)| a == asset).map(|(_, t)| *t)
    }

    /// When `asset` can next be claimed, given the cooldown (0: now).
    pub fn next_claim(&self, asset: &AccountId, cooldown_ms: u64) -> u64 {
        self.last_claim(asset).map_or(0, |t| t.saturating_add(cooldown_ms))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Effect {
    Init(Config),
    /// On the config: the claimed drop and cooldown are the configured ones.
    CheckDrop { asset: AccountId, amount: u128, cooldown_ms: u64 },
    /// On the claimer's record: the cooldown has passed at `t`; records `t`.
    Claimed { asset: AccountId, cooldown_ms: u64, t: u64 },
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Impl::hash_bytes(bytes).as_bytes().try_into().expect("sha256 is 32 bytes")
}

fn seed(tag: &[u8]) -> PdaSeed {
    PdaSeed::new(sha256(tag))
}

pub fn config_seed() -> PdaSeed {
    seed(b"/bosphorus/v1/faucet/config")
}

pub fn mint_seed() -> PdaSeed {
    seed(b"/bosphorus/v1/faucet/mint")
}

pub fn custody_seed() -> PdaSeed {
    seed(b"/bosphorus/v1/faucet/custody")
}

pub fn config_id(faucet: &AccountId) -> AccountId {
    AccountId::for_public_pda(faucet, &config_seed())
}

/// The `mint_authority` to give a token this faucet drops.
pub fn mint_authority(faucet: &AccountId) -> AccountId {
    AccountId::for_public_pda(faucet, &mint_seed())
}

/// Holds the native drops; fund it with native transfers.
pub fn custody_id(faucet: &AccountId) -> AccountId {
    AccountId::for_public_pda(faucet, &custody_seed())
}

pub fn is_native(asset: &AccountId) -> bool {
    *asset == NATIVE_TOKEN_PROGRAM_ID
}

/// The chained call that pays a drop to `to`.
pub fn payout(faucet: &AccountId, asset: AccountId, to: AccountId, amount: u128) -> ChainedCall {
    if is_native(&asset) {
        return native_token::custody_transfer(custody_id(faucet), custody_seed(), to, amount);
    }
    ChainedCall::new(
        asset,
        vec![
            ProgramShardSelector::new(mint_authority(faucet), asset),
            ProgramShardSelector::new(bosphorus_token::config_id(&asset), asset),
            ProgramShardSelector::new(to, asset),
        ],
        &bosphorus_token::Instruction::Mint { amount },
    )
    .with_pda_seeds(vec![mint_seed()])
}

/// A claim's `(account, shard owner)` selectors, in instruction order.
pub fn claim_selectors(faucet: &AccountId, asset: &AccountId, claimer: AccountId, to: AccountId) -> Vec<(AccountId, AccountId)> {
    let mut out = vec![(claimer, *faucet), (config_id(faucet), *faucet)];
    if is_native(asset) {
        out.push((custody_id(faucet), NATIVE_TOKEN_PROGRAM_ID));
    } else {
        out.push((mint_authority(faucet), *asset));
        out.push((bosphorus_token::config_id(asset), *asset));
    }
    out.push((to, *asset));
    out
}
