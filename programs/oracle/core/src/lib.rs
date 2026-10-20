//! `oracle`: push price feeds with a fixed shard layout (design §2.2).
//!
//! A consumer can't call an oracle and get a value back on v0.3.1, so `lending` inspects the feed
//! account and decodes [`Feed`] itself. Any program writing this layout can serve as an oracle.

use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{account::AccountId, program::PdaSeed};
use risc0_zkvm::sha::{Impl, Sha256 as _};

pub use bosphorus_core::U256;
/// `Publish` sets the window `[timestamp, timestamp + PUBLISH_WINDOW_MS)`: a publisher can't
/// post-date a price, and a delayed publish expires. 10 min outlasts all but the longest testnet
/// stalls (findings F3); a late publish still carries its own timestamp, so lending sees its real age.
pub const PUBLISH_WINDOW_MS: u64 = 10 * 60_000;
pub const MAX_NAME_LEN: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Instruction {
    /// Accounts: `[publisher, feed]`. The publisher signs and is fixed for the feed's lifetime.
    CreateFeed { name: String },
    /// Accounts: `[publisher, feed]`. `timestamp` is in ms and must be newer than the last one.
    Publish { price: U256, timestamp: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Effect {
    Create { publisher: AccountId },
    Publish { publisher: AccountId, price: U256, timestamp: u64 },
}

/// `shard[oracle]` of the feed account. Fixed size (80 bytes borsh).
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Feed {
    /// Loan-asset base units per collateral-asset base unit × 1e36 (`bosphorus_core::scale_price`).
    pub price: U256,
    /// ms; 0 until the first publish.
    pub updated_at: u64,
    pub round: u64,
    pub publisher: AccountId,
}

/// `SHA256("/bosphorus/v1/oracle/feed/" ‖ publisher ‖ name)`. Keying by publisher stops anyone
/// squatting a feed name with their own publisher.
pub fn feed_seed(publisher: &AccountId, name: &str) -> PdaSeed {
    let mut bytes = b"/bosphorus/v1/oracle/feed/".to_vec();
    bytes.extend_from_slice(publisher.as_ref());
    bytes.extend_from_slice(name.as_bytes());
    PdaSeed::new(Impl::hash_bytes(&bytes).as_bytes().try_into().expect("sha256 is 32 bytes"))
}

pub fn feed_id(oracle: &AccountId, publisher: &AccountId, name: &str) -> AccountId {
    AccountId::for_public_pda(oracle, &feed_seed(publisher, name))
}
