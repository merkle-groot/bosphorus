//! Instruction and effect types for the spike guests, shared with the host harness.

use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{account::AccountId, program::PdaSeed};

/// S1: cost probe. Account layout for `Op`, in order:
/// `touch` accounts (this program's shard), then the feed if `inspect`, then `vault, to`
/// (native shards) if `native_transfer > 0`, then two accounts (callee's shard) if `guest_call`.
pub mod s1 {
    use super::{AccountId, BorshDeserialize, BorshSerialize, PdaSeed};

    pub const VAULT_SEED: PdaSeed = PdaSeed::new([0x56; 32]);

    #[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
    pub enum Instruction {
        /// Setup: overwrite this program's shard on every listed account with `len` bytes.
        Fill { len: u32 },
        Op {
            touch: u8,
            inspect: bool,
            native_transfer: u128,
            guest_call: Option<AccountId>,
        },
    }

    #[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
    pub enum Effect {
        Fill(u32),
        /// Read the shard and write it back with one byte flipped.
        Touch,
        /// Read a foreign shard; must not write.
        Inspect,
    }
}

/// S2: borrow-to-private shape. Accounts: `[owner, position, vault, (to)]`; no `to` pays the owner.
pub mod s2 {
    use risc0_zkvm::sha::{Impl, Sha256 as _};

    use super::{AccountId, BorshDeserialize, BorshSerialize, PdaSeed};

    pub const VAULT_SEED: PdaSeed = PdaSeed::new([0x57; 32]);
    /// Event selector (a literal; the convention is `sha256("<program>::<Name>")[..8]`).
    pub const POSITION_UPDATED: [u8; 8] = *b"PosUpdtd";

    #[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
    pub enum Instruction {
        Borrow {
            salt: [u8; 32],
            amount: u128,
            /// Claimed time; the plan sets the window `[t, t + w)`.
            t: u64,
            w: u64,
        },
    }

    #[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
    pub enum Effect {
        AddDebt(u128),
    }

    /// Design §4: `SHA256(domain ‖ owner ‖ salt)` (market id omitted in the spike).
    pub fn position_seed(owner: &AccountId, salt: &[u8; 32]) -> PdaSeed {
        let mut bytes = b"/bosphorus/spike/Position/".to_vec();
        bytes.extend_from_slice(owner.as_ref());
        bytes.extend_from_slice(salt);
        PdaSeed::new(Impl::hash_bytes(&bytes).as_bytes().try_into().unwrap())
    }

    pub fn position_id(program: &AccountId, owner: &AccountId, salt: &[u8; 32]) -> AccountId {
        AccountId::for_public_pda(program, &position_seed(owner, salt))
    }
}
