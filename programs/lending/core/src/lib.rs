//! `lending`: isolated Morpho-style markets on LEZ v0.3.1 (design §2.3, §5).
//!
//! Shared by the guest and clients: instructions with their account layouts, effects, events,
//! shard layouts and PDA seeds. The maths is in `bosphorus_core`.
//!
//! `plan` can't read state, so every market op carries the full [`MarketParams`]: the market's PDA
//! is derived from their hash, which authenticates them. Values in other shards are claimed as
//! intervals and checked by each shard's `apply` (constraints §6).

use borsh::{BorshDeserialize, BorshSerialize};
pub use bosphorus_core::{
    Claimed, Health, Interval, IrmParams, Liquidation, LiquidationAmount, MarketOp, MarketState, Position, PositionOp,
    U256, Window,
};
use lee_core::{
    account::{AccountId, ProgramShardSelector},
    native_token::{self, NATIVE_TOKEN_PROGRAM_ID},
    program::{ChainedCall, PdaSeed},
};
use risc0_zkvm::sha::{Impl, Sha256 as _};

pub type MarketId = [u8; 32];
pub type Salt = [u8; 32];
/// A program's bytecode id, as in its loader header (`lee_core::program::ProgramId`).
pub type ImageId = [u32; 8];

pub const MAX_LLTVS: usize = 32;
pub const MAX_IRMS: usize = 8;
pub const MAX_ASSETS: usize = 16;

/// Immutable per market; `market_id = SHA256(borsh(params))`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct MarketParams {
    /// Token instance, or `0x00…00` for the native token.
    pub loan_asset: AccountId,
    pub collateral_asset: AccountId,
    /// Whose shard on `oracle_feed` holds the `bosphorus_oracle::Feed`.
    pub oracle_program: AccountId,
    pub oracle_feed: AccountId,
    pub irm: IrmParams,
    /// WAD.
    pub lltv: u128,
    /// ms. Checked against the end of each tx's validity window.
    pub max_price_age: u64,
}

impl MarketParams {
    pub fn id(&self) -> MarketId {
        sha256(&borsh::to_vec(self).expect("borsh serialization is infallible"))
    }

    /// The assets whose bytecode `create_market` checks: loan then collateral, without the native
    /// token (built in) and without repeats. Each needs its header account in the tx.
    pub fn checked_assets(&self) -> Vec<AccountId> {
        let mut assets = vec![];
        for asset in [self.loan_asset, self.collateral_asset] {
            if asset != NATIVE_TOKEN_PROGRAM_ID && !assets.contains(&asset) {
                assets.push(asset);
            }
        }
        assets
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Config {
    pub admin: AccountId,
    /// Receives `pending_fee_shares` (`claim_fees`, Phase 3).
    pub fee_recipient: AccountId,
    pub enabled_lltvs: Vec<u128>,
    pub enabled_irms: Vec<IrmParams>,
    /// Token bytecode a market's assets may run (the native token is always allowed). On LEZ a
    /// tx's signer is authorised in every program it reaches, so a malicious "token" could move
    /// any balance of anyone who signs a tx in its market (findings F1).
    pub enabled_assets: Vec<ImageId>,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Market {
    pub params: MarketParams,
    pub state: MarketState,
}

/// `shard[lending]` on every lending account. The tag stops one kind being passed off as another
/// (a market as a position, say).
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[allow(clippy::large_enum_variant, reason = "decoded once per apply; boxing changes nothing on chain")]
pub enum Shard {
    Config(Config),
    Market(Market),
    Position(Position),
}

/// The client's claims for a health check: the oracle price and the borrow ratio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct HealthClaim {
    pub price: Interval<U256>,
    pub rb: Interval<u128>,
    pub window: Window,
}

/// Account layouts are in each variant's doc; `(to)` is optional and defaults to the owner.
/// "Signs" means `is_authorized`: a signer, a private account with its `ask`, or a PDA.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Instruction {
    /// `[instance (this program's header, signs), config]`. Only the header's key can initialise.
    Init { admin: AccountId, fee_recipient: AccountId },
    /// `[admin (signs), config]`.
    EnableLltv { lltv: u128 },
    /// `[admin (signs), config]`.
    EnableIrm { irm: IrmParams },
    /// `[admin (signs), config]`.
    SetFeeRecipient { fee_recipient: AccountId },
    /// `[admin (signs), config, market]`. Accrues at the old fee first.
    SetFee { params: MarketParams, fee: u128, window: Window },
    /// `[config, market, header of each checked asset]` (`MarketParams::checked_assets`: loan, then
    /// collateral, each a program's header account selecting the loader's shard). Permissionless
    /// for enabled LLTVs and IRMs; `images` claims each asset's image id, which must be enabled
    /// and its header immutable.
    CreateMarket { params: MarketParams, window: Window, images: Vec<ImageId> },
    /// `[market, position, payer (signs), loan_vault]`. On behalf of any position.
    Supply { params: MarketParams, assets: u128, rs: Interval<u128>, window: Window },
    /// `[owner (signs), market, position, loan_vault, (to)]`.
    Withdraw { params: MarketParams, shares: u128, salt: Salt, rs: Interval<u128>, window: Window },
    /// `[position, payer (signs), collateral_vault]`. On behalf of any position; the market isn't
    /// touched, so the client checks it exists.
    SupplyCollateral { params: MarketParams, assets: u128 },
    /// With `health`: `[owner (signs), market, position, feed, collateral_vault, (to)]`.
    /// Without: `[owner (signs), position, collateral_vault, (to)]`, and the position must hold no
    /// debt (so a dead oracle can't lock debt-free collateral).
    WithdrawCollateral { params: MarketParams, assets: u128, salt: Salt, health: Option<HealthClaim> },
    /// `[owner (signs), market, position, feed, loan_vault, (to)]`.
    Borrow { params: MarketParams, assets: u128, salt: Salt, health: HealthClaim },
    /// `[market, position, payer (signs), loan_vault]`. On behalf of any position. Pays
    /// `⌈shares · rb.hi⌉`; the overpayment (the claim's width and projected interest) lowers the
    /// market's borrow assets, and once those reach zero it sits unowned in the loan vault.
    Repay { params: MarketParams, shares: u128, rb: Interval<u128>, window: Window },
    /// `[liquidator (signs), market, position, feed, loan_vault, collateral_vault, (to)]`.
    /// Permissionless. `claimed` must be the position's exact contents; it must be unhealthy at
    /// `(price.hi, rb.lo)`. The liquidator pays the repaid assets and receives the seized collateral.
    Liquidate {
        params: MarketParams,
        claimed: Claimed,
        amount: LiquidationAmount,
        price: Interval<U256>,
        rb: Interval<u128>,
        window: Window,
    },
    /// `[config, market, recipient_position]`. Permissionless: moves `shares` of the market's
    /// pending fee shares into the fee recipient's (zero-salt) position; the config checks
    /// `recipient` is the fee recipient.
    ClaimFees { params: MarketParams, recipient: AccountId, shares: u128 },
    /// `[admin (signs), config]`. Lets markets use assets running this bytecode.
    EnableAsset { image_id: ImageId },
}

impl Instruction {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Init { .. } => "init",
            Self::EnableLltv { .. } => "enable_lltv",
            Self::EnableIrm { .. } => "enable_irm",
            Self::SetFeeRecipient { .. } => "set_fee_recipient",
            Self::SetFee { .. } => "set_fee",
            Self::CreateMarket { .. } => "create_market",
            Self::Supply { .. } => "supply",
            Self::Withdraw { .. } => "withdraw",
            Self::SupplyCollateral { .. } => "supply_collateral",
            Self::WithdrawCollateral { health: None, .. } => "withdraw_collateral_no_price",
            Self::WithdrawCollateral { .. } => "withdraw_collateral",
            Self::Borrow { .. } => "borrow",
            Self::Repay { .. } => "repay",
            Self::Liquidate { .. } => "liquidate",
            Self::ClaimFees { .. } => "claim_fees",
            Self::EnableAsset { .. } => "enable_asset",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum AdminOp {
    EnableLltv(u128),
    EnableIrm(IrmParams),
    EnableAsset(ImageId),
    SetFeeRecipient(AccountId),
    /// Only check the signer (for `SetFee`, which writes the market).
    Check,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Effect {
    /// Config: write once.
    Init(Config),
    /// Config: the signer must be the admin.
    Admin { signer: AccountId, op: AdminOp },
    /// Config (read only): the market's LLTV, IRM and asset bytecode are enabled.
    CheckEnabled { lltv: u128, irm: IrmParams, images: Vec<ImageId> },
    /// An asset's header (inspect, the loader's shard): runs `image_id` and can't be updated.
    AssetHeader { image_id: ImageId },
    /// Config (read only): `recipient` is the fee recipient.
    CheckFeeRecipient { recipient: AccountId },
    /// Market: write once.
    CreateMarket { params: MarketParams, t: u64 },
    /// Market: accrue to `t`, check the op's claim, apply it.
    Market { t: u64, op: MarketOp },
    /// Position: belongs to `market_id` (or is empty), then apply.
    Position { market_id: MarketId, op: PositionOp },
    /// Oracle feed (inspect, the oracle program's shard): price inside the claim, fresh at `window_end`.
    Price { claim: Interval<U256>, max_age: u64, window_end: u64 },
}

/// `ProgramEvent { selector: SHA256("lending::<Name>")[..8], data: borsh(event) }`. Never contains
/// an owner or a payout account. Dropped from private txs (S6).
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Event {
    MarketCreated { market_id: MarketId, params: MarketParams },
    Supply { market_id: MarketId, position: AccountId, assets: u128, shares: u128 },
    Withdraw { market_id: MarketId, position: AccountId, assets: u128, shares: u128 },
    SupplyCollateral { market_id: MarketId, position: AccountId, assets: u128 },
    WithdrawCollateral { market_id: MarketId, position: AccountId, assets: u128 },
    Borrow { market_id: MarketId, position: AccountId, assets: u128, shares: u128 },
    Repay { market_id: MarketId, position: AccountId, assets: u128, shares: u128 },
    /// The written-off assets are only known to the market's apply; `bad_shares` is the debt burnt.
    Liquidate { market_id: MarketId, position: AccountId, liquidation: Liquidation },
    FeesClaimed { market_id: MarketId, position: AccountId, shares: u128 },
}

impl Event {
    pub fn name(&self) -> &'static str {
        match self {
            Self::MarketCreated { .. } => "MarketCreated",
            Self::Supply { .. } => "Supply",
            Self::Withdraw { .. } => "Withdraw",
            Self::SupplyCollateral { .. } => "SupplyCollateral",
            Self::WithdrawCollateral { .. } => "WithdrawCollateral",
            Self::Borrow { .. } => "Borrow",
            Self::Repay { .. } => "Repay",
            Self::Liquidate { .. } => "Liquidate",
            Self::FeesClaimed { .. } => "FeesClaimed",
        }
    }

    pub fn selector(&self) -> [u8; 8] {
        let digest = sha256(format!("lending::{}", self.name()).as_bytes());
        digest[..8].try_into().expect("8 bytes")
    }
}

// ---- seeds and accounts ----------------------------------------------------------------------

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Impl::hash_bytes(bytes).as_bytes().try_into().expect("sha256 is 32 bytes")
}

fn seed(domain: &[u8], parts: &[&[u8]]) -> PdaSeed {
    let mut bytes = domain.to_vec();
    for part in parts {
        bytes.extend_from_slice(part);
    }
    PdaSeed::new(sha256(&bytes))
}

pub fn config_seed() -> PdaSeed {
    seed(b"/bosphorus/v1/lending/config", &[])
}

pub fn market_seed(market_id: &MarketId) -> PdaSeed {
    seed(b"/bosphorus/v1/lending/market/", &[market_id])
}

pub fn loan_vault_seed(market_id: &MarketId) -> PdaSeed {
    seed(b"/bosphorus/v1/lending/loan-vault/", &[market_id])
}

pub fn collateral_vault_seed(market_id: &MarketId) -> PdaSeed {
    seed(b"/bosphorus/v1/lending/coll-vault/", &[market_id])
}

/// Design §4: `SHA256("/bosphorus/v1/Position/" ‖ market_id ‖ owner ‖ salt)`. Public owners use a
/// zero salt; private owners must not (their root account id is computable by anyone who has
/// paid them).
pub fn position_seed(market_id: &MarketId, owner: &AccountId, salt: &Salt) -> PdaSeed {
    seed(b"/bosphorus/v1/Position/", &[market_id, owner.as_ref(), salt])
}

/// The accounts of one market under a `lending` instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarketAccounts {
    pub market: AccountId,
    pub loan_vault: AccountId,
    pub collateral_vault: AccountId,
}

impl MarketAccounts {
    pub fn new(lending: &AccountId, market_id: &MarketId) -> Self {
        Self {
            market: AccountId::for_public_pda(lending, &market_seed(market_id)),
            loan_vault: AccountId::for_public_pda(lending, &loan_vault_seed(market_id)),
            collateral_vault: AccountId::for_public_pda(lending, &collateral_vault_seed(market_id)),
        }
    }
}

pub fn config_id(lending: &AccountId) -> AccountId {
    AccountId::for_public_pda(lending, &config_seed())
}

pub fn position_id(lending: &AccountId, market_id: &MarketId, owner: &AccountId, salt: &Salt) -> AccountId {
    AccountId::for_public_pda(lending, &position_seed(market_id, owner, salt))
}

/// A chained `Transfer` of `asset` (native or a token instance: same instruction bytes, design
/// §2.1). `seed` authorises a debit from one of the caller's PDAs.
pub fn transfer(asset: AccountId, from: AccountId, to: AccountId, amount: u128, seed: Option<PdaSeed>) -> ChainedCall {
    let call = ChainedCall::new(
        asset,
        vec![ProgramShardSelector::new(from, asset), ProgramShardSelector::new(to, asset)],
        &native_token::Instruction::Transfer { amount },
    );
    match seed {
        Some(seed) => call.with_pda_seeds(vec![seed]),
        None => call,
    }
}
