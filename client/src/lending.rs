//! Lending: state readers, the claim builder and tx builders (plan 2.3). Public owners only;
//! private owners are Phase 4.
//!
//! Every op reads the market (and feed) first, builds its claims, and after inclusion checks that
//! the position moved by exactly what `plan` derives from those claims. A failed public tx still
//! lands and pays (constraints §5), so that check is how a failure shows up.

use std::collections::BTreeSet;

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use bosphorus_core::{WAD, claims, irm, position};
pub use bosphorus_lending::{
    Config as LendingConfig, HealthClaim, Instruction, Interval, IrmParams, Market, MarketAccounts, MarketId,
    MarketParams, Position, Shard, Window,
};
use bosphorus_lending::{config_id, position_id};
use common::transaction::LeeTransaction;
use lee::{AccountId, ProgramShardSelector};
use sequencer_service_rpc::RpcClient as _;
use wallet::{AccountIdentity, AccountMention};

use crate::{Client, Feed, U256};

pub const PUBLIC_SALT: [u8; 32] = [0; 32];

fn sign(id: AccountId, program: AccountId) -> AccountMention {
    AccountIdentity::Public(id).select_program_shard(program)
}

fn read(id: AccountId, program: AccountId) -> AccountMention {
    AccountIdentity::PublicNoSign(id).select_program_shard(program)
}

/// How a client turns state it has read into claims (E7 tunes these).
#[derive(Debug, Clone, Copy)]
pub struct ClaimConfig {
    /// `w`: how long the tx stays valid after `t`.
    pub window_ms: u64,
    /// `t = now − lag`, so a sequencer clock slightly behind ours still accepts the window.
    pub lag_ms: u64,
    /// Ratio claims reach the ratios accrued to `t + lag + ahead`: room for txs built after ours
    /// (with a later `t`) to land first. Suppliers and repayers pay for it in slippage.
    pub ahead_ms: u64,
    /// Ratio claims are widened by this fraction (WAD) on each side.
    pub ratio_width: u128,
    /// The price claim's `lo` is this fraction (WAD) below the feed price.
    pub price_width: u128,
}

impl Default for ClaimConfig {
    fn default() -> Self {
        Self { window_ms: 120_000, lag_ms: 2_000, ahead_ms: 30_000, ratio_width: WAD / 1_000_000_000, price_width: WAD / 200 }
    }
}

/// Claims for one tx. Each op only uses one end of some of these (`bosphorus_core::claims`), and
/// opens the other end fully so the claim fails only when it would cost the protocol.
#[derive(Debug, Clone, Copy)]
pub struct Claims {
    pub window: Window,
    pub rs: Interval<u128>,
    pub rb: Interval<u128>,
}

impl ClaimConfig {
    pub fn claims(&self, market: &Market, now_ms: u64) -> Result<Claims> {
        let t = now_ms.saturating_sub(self.lag_ms);
        let ahead = self.lag_ms + self.ahead_ms;
        let (rs, rb) = claims::ratio_claims(&market.state, &market.params.irm, t, ahead, self.ratio_width)?;
        Ok(Claims { window: Window { t, w: self.window_ms }, rs, rb })
    }

    /// For a health check only the price's `lo` matters.
    pub fn price(&self, feed: &Feed) -> Result<Interval<U256>> {
        Ok(Interval { lo: claims::price_claim(feed.price, self.price_width)?.lo, hi: U256::MAX })
    }
}

/// The market an op acts on, how it claims, and who pays the fee (default: the signer).
#[derive(Debug, Clone, Copy)]
pub struct On {
    pub lending: AccountId,
    pub market: MarketId,
    pub claims: ClaimConfig,
    pub payer: Option<AccountId>,
}

impl On {
    pub fn accounts(&self) -> MarketAccounts {
        MarketAccounts::new(&self.lending, &self.market)
    }

    /// A public owner's position (zero salt).
    pub fn position_of(&self, owner: AccountId) -> AccountId {
        position_id(&self.lending, &self.market, &owner, &PUBLIC_SALT)
    }
}

pub fn now_ms() -> Result<u64> {
    Ok(u64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis())?)
}

pub fn parse_market_id(s: &str) -> Result<MarketId> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    ensure!(s.len() == 64, "a market id is 32 bytes of hex");
    let mut id = [0; 32];
    for (i, byte) in id.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).context("bad hex in market id")?;
    }
    Ok(id)
}

pub fn market_id_hex(id: &MarketId) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}

/// Utilisation and rates of a market, projected to `now_ms`.
#[derive(Debug, Clone)]
pub struct MarketStatus {
    pub state: bosphorus_core::MarketState,
    pub utilization: u128,
    pub borrow_apr: u128,
    pub supply_apr: u128,
}

pub fn market_status(market: &Market, now_ms: u64) -> Result<MarketStatus> {
    let state = claims::project(&market.state, &market.params.irm, now_ms)?;
    let utilization = irm::utilization(state.total_borrow_assets, state.total_supply_assets)?;
    let borrow_apr = irm::apr(market.params.irm.rate(utilization)?);
    let supply_apr = bosphorus_core::math::w_mul_down(
        bosphorus_core::math::w_mul_down(borrow_apr, utilization)?,
        WAD - state.fee,
    )?;
    Ok(MarketStatus { state, utilization, borrow_apr, supply_apr })
}

/// A position valued at `now_ms` and the feed price.
#[derive(Debug, Clone)]
pub struct PositionStatus {
    pub supplied_assets: u128,
    pub debt: u128,
    pub max_borrow: U256,
    /// WAD; `None` without debt. Liquidatable below 1.
    pub health_factor: Option<U256>,
}

pub fn position_status(market: &Market, p: &Position, feed: &Feed, now_ms: u64) -> Result<PositionStatus> {
    use bosphorus_core::shares::{exact_to_assets_down, exact_to_assets_up};
    let s = claims::project(&market.state, &market.params.irm, now_ms)?;
    let supplied_assets = exact_to_assets_down(p.supply_shares, s.total_supply_assets, s.total_supply_shares)?;
    let debt = exact_to_assets_up(p.borrow_shares, s.total_borrow_assets, s.total_borrow_shares)?;
    let rb = bosphorus_core::shares::ratio_up(s.total_borrow_assets, s.total_borrow_shares)?;
    let health = bosphorus_core::Health { price: feed.price, rb, lltv: market.params.lltv };
    Ok(PositionStatus {
        supplied_assets,
        debt,
        max_borrow: position::max_borrow(p.collateral, feed.price, market.params.lltv)?,
        health_factor: position::health_factor(p.collateral, p.borrow_shares, &health)?,
    })
}

impl Client {
    async fn lending_shard(&self, account: AccountId, lending: AccountId) -> Result<Option<Shard>> {
        let account = self.wallet.get_account_view(ProgramShardSelector::new(account, lending)).await?;
        let data = account.data.shard(lending);
        if data.is_empty() {
            return Ok(None);
        }
        Ok(Some(borsh::from_slice(data).context("not a lending account")?))
    }

    pub async fn lending_config(&self, lending: AccountId) -> Result<LendingConfig> {
        match self.lending_shard(config_id(&lending), lending).await? {
            Some(Shard::Config(config)) => Ok(config),
            _ => bail!("lending {lending} isn't initialised"),
        }
    }

    pub async fn market(&self, lending: AccountId, id: &MarketId) -> Result<Market> {
        match self.lending_shard(MarketAccounts::new(&lending, id).market, lending).await? {
            Some(Shard::Market(market)) => Ok(market),
            _ => bail!("no market {}", market_id_hex(id)),
        }
    }

    /// `None` for a position that has never been used.
    pub async fn position(&self, lending: AccountId, position: AccountId) -> Result<Option<Position>> {
        match self.lending_shard(position, lending).await? {
            None => Ok(None),
            Some(Shard::Position(p)) => Ok(Some(p)),
            Some(_) => bail!("{position} is not a position"),
        }
    }

    async fn position_or_default(&self, lending: AccountId, position: AccountId) -> Result<Position> {
        Ok(self.position(lending, position).await?.unwrap_or_default())
    }

    /// Every market created under `lending`, found by scanning blocks for `CreateMarket` (the keeper
    /// discovers positions the same way, S6).
    pub async fn markets(&self, lending: AccountId) -> Result<Vec<(MarketId, Market)>> {
        const CHUNK: u64 = 100;
        let rpc = self.wallet.client_owned();
        let last = rpc.get_last_block_id().await?;
        let mut ids = BTreeSet::new();
        let mut start = 1;
        while start <= last {
            let end = (start + CHUNK - 1).min(last);
            for block in rpc.get_block_range(start, end).await? {
                for tx in block.body.transactions {
                    let LeeTransaction::Public(tx) = tx else { continue };
                    if tx.message.program_account_id != lending {
                        continue;
                    }
                    if let Ok(Instruction::CreateMarket { params, .. }) = borsh::from_slice(&tx.message.instruction_data) {
                        ids.insert(params.id());
                    }
                }
            }
            start = end + 1;
        }
        let mut markets = vec![];
        for id in ids {
            // A failed create still lands; only list markets that exist.
            if let Ok(market) = self.market(lending, &id).await {
                markets.push((id, market));
            }
        }
        Ok(markets)
    }

    async fn send_lending(&self, lending: AccountId, accounts: Vec<AccountMention>, ix: Instruction, payer: Option<AccountId>) -> Result<()> {
        self.send_public(lending, accounts, ix, payer).await
    }

    // ---- admin ----------------------------------------------------------------------------

    /// Signed by the lending program's own header key (held by this wallet if it deployed it).
    pub async fn lending_init(&self, lending: AccountId, admin: AccountId, fee_recipient: AccountId, payer: AccountId) -> Result<()> {
        let accounts = vec![sign(lending, lending), read(config_id(&lending), lending)];
        self.send_lending(lending, accounts, Instruction::Init { admin, fee_recipient }, Some(payer)).await?;
        let config = self.lending_config(lending).await?;
        ensure!(config.admin == admin, "init didn't apply: {config:?}");
        Ok(())
    }

    pub async fn lending_admin(&self, lending: AccountId, admin: AccountId, ix: Instruction, payer: Option<AccountId>) -> Result<LendingConfig> {
        let before = self.lending_config(lending).await?;
        let accounts = vec![sign(admin, lending), read(config_id(&lending), lending)];
        self.send_lending(lending, accounts, ix, payer).await?;
        let after = self.lending_config(lending).await?;
        ensure!(after != before, "admin op didn't apply (is {admin} the admin? already set?)");
        Ok(after)
    }

    pub async fn set_fee(&self, on: &On, admin: AccountId, fee: u128) -> Result<()> {
        let (lending, market) = (on.lending, self.market(on.lending, &on.market).await?);
        let window = on.claims.claims(&market, now_ms()?)?.window;
        let accounts = vec![sign(admin, lending), read(config_id(&lending), lending), read(on.accounts().market, lending)];
        self.send_lending(lending, accounts, Instruction::SetFee { params: market.params, fee, window }, on.payer).await?;
        let after = self.market(lending, &on.market).await?;
        ensure!(after.state.fee == fee, "set fee didn't apply");
        Ok(())
    }

    // ---- markets and positions ---------------------------------------------------------------

    pub async fn create_market(&self, lending: AccountId, params: MarketParams, cfg: &ClaimConfig, payer: AccountId) -> Result<MarketId> {
        let id = params.id();
        let window = Window { t: now_ms()?.saturating_sub(cfg.lag_ms), w: cfg.window_ms };
        let accounts = vec![read(config_id(&lending), lending), read(MarketAccounts::new(&lending, &id).market, lending)];
        self.send_lending(lending, accounts, Instruction::CreateMarket { params, window }, Some(payer)).await?;
        self.market(lending, &id).await.context("create_market didn't apply (LLTV and IRM enabled? already exists?)")?;
        Ok(id)
    }

    /// Supplies `assets` from `from` to `position`. Returns the shares minted.
    pub async fn supply(&self, on: &On, from: AccountId, position: AccountId, assets: u128) -> Result<u128> {
        let (lending, market) = (on.lending, self.market(on.lending, &on.market).await?);
        let c = on.claims.claims(&market, now_ms()?)?;
        let rs = Interval { lo: 0, hi: c.rs.hi };
        let shares = claims::supply_shares(assets, &rs)?;
        let (m, loan) = (on.accounts(), market.params.loan_asset);
        let accounts = vec![read(m.market, lending), read(position, lending), sign(from, loan), read(m.loan_vault, loan)];
        let before = self.position_or_default(lending, position).await?;
        let ix = Instruction::Supply { params: market.params, assets, rs, window: c.window };
        self.send_lending(lending, accounts, ix, on.payer).await?;
        let after = self.position_or_default(lending, position).await?;
        ensure!(after.supply_shares == before.supply_shares + shares, "supply didn't apply");
        Ok(shares)
    }

    /// Withdraws `shares` of `owner`'s supply to `to` (default: the owner). Returns the assets.
    pub async fn withdraw(&self, on: &On, owner: AccountId, shares: u128, to: Option<AccountId>) -> Result<u128> {
        let (lending, market) = (on.lending, self.market(on.lending, &on.market).await?);
        let c = on.claims.claims(&market, now_ms()?)?;
        let rs = Interval { lo: c.rs.lo, hi: u128::MAX };
        let assets = claims::withdraw_assets(shares, &rs)?;
        let (m, loan, position) = (on.accounts(), market.params.loan_asset, on.position_of(owner));
        let mut accounts = vec![sign(owner, loan), read(m.market, lending), read(position, lending), read(m.loan_vault, loan)];
        accounts.extend(to.filter(|to| *to != owner).map(|to| read(to, loan)));
        let before = self.position_or_default(lending, position).await?;
        let ix = Instruction::Withdraw { params: market.params, shares, salt: PUBLIC_SALT, rs, window: c.window };
        self.send_lending(lending, accounts, ix, on.payer).await?;
        let after = self.position_or_default(lending, position).await?;
        ensure!(after.supply_shares + shares == before.supply_shares, "withdraw didn't apply (liquidity? shares?)");
        Ok(assets)
    }

    pub async fn supply_collateral(&self, on: &On, from: AccountId, position: AccountId, assets: u128) -> Result<()> {
        // The market isn't touched by this op, so check it exists here.
        let (lending, market) = (on.lending, self.market(on.lending, &on.market).await?);
        let (m, coll) = (on.accounts(), market.params.collateral_asset);
        let accounts = vec![read(position, lending), sign(from, coll), read(m.collateral_vault, coll)];
        let before = self.position_or_default(lending, position).await?;
        self.send_lending(lending, accounts, Instruction::SupplyCollateral { params: market.params, assets }, on.payer).await?;
        let after = self.position_or_default(lending, position).await?;
        ensure!(after.collateral == before.collateral + assets, "supply_collateral didn't apply");
        Ok(())
    }

    /// Without debt this skips the oracle and the market (so it works with a stale feed).
    pub async fn withdraw_collateral(&self, on: &On, owner: AccountId, assets: u128, to: Option<AccountId>) -> Result<()> {
        let (lending, market) = (on.lending, self.market(on.lending, &on.market).await?);
        let (m, coll, position) = (on.accounts(), market.params.collateral_asset, on.position_of(owner));
        let before = self.position_or_default(lending, position).await?;
        let mut accounts = vec![sign(owner, coll)];
        let health = if before.borrow_shares == 0 {
            accounts.push(read(position, lending));
            None
        } else {
            let c = on.claims.claims(&market, now_ms()?)?;
            let feed = self.feed(market.params.oracle_feed, market.params.oracle_program).await?;
            accounts.extend([
                read(m.market, lending),
                read(position, lending),
                read(market.params.oracle_feed, market.params.oracle_program),
            ]);
            Some(HealthClaim { price: on.claims.price(&feed)?, rb: Interval { lo: 0, hi: c.rb.hi }, window: c.window })
        };
        accounts.push(read(m.collateral_vault, coll));
        accounts.extend(to.filter(|to| *to != owner).map(|to| read(to, coll)));
        let ix = Instruction::WithdrawCollateral { params: market.params, assets, salt: PUBLIC_SALT, health };
        self.send_lending(lending, accounts, ix, on.payer).await?;
        let after = self.position_or_default(lending, position).await?;
        ensure!(after.collateral + assets == before.collateral, "withdraw_collateral didn't apply (health? price?)");
        Ok(())
    }

    /// Borrows `assets` against `owner`'s position, paid to `to` (default: the owner). Returns the
    /// debt shares.
    pub async fn borrow(&self, on: &On, owner: AccountId, assets: u128, to: Option<AccountId>) -> Result<u128> {
        let (lending, market) = (on.lending, self.market(on.lending, &on.market).await?);
        let feed = self.feed(market.params.oracle_feed, market.params.oracle_program).await?;
        let c = on.claims.claims(&market, now_ms()?)?;
        let health = HealthClaim { price: on.claims.price(&feed)?, rb: c.rb, window: c.window };
        let shares = claims::borrow_shares(assets, &c.rb)?;
        let (m, loan, position) = (on.accounts(), market.params.loan_asset, on.position_of(owner));
        let mut accounts = vec![
            sign(owner, loan),
            read(m.market, lending),
            read(position, lending),
            read(market.params.oracle_feed, market.params.oracle_program),
            read(m.loan_vault, loan),
        ];
        accounts.extend(to.filter(|to| *to != owner).map(|to| read(to, loan)));
        let before = self.position_or_default(lending, position).await?;
        let ix = Instruction::Borrow { params: market.params, assets, salt: PUBLIC_SALT, health };
        self.send_lending(lending, accounts, ix, on.payer).await?;
        let after = self.position_or_default(lending, position).await?;
        ensure!(after.borrow_shares == before.borrow_shares + shares, "borrow didn't apply (health? liquidity? price fresh?)");
        Ok(shares)
    }

    /// Repays `shares` of `position`'s debt from `from`. Returns the assets paid.
    pub async fn repay(&self, on: &On, from: AccountId, position: AccountId, shares: u128) -> Result<u128> {
        let (lending, market) = (on.lending, self.market(on.lending, &on.market).await?);
        let c = on.claims.claims(&market, now_ms()?)?;
        let rb = Interval { lo: 0, hi: c.rb.hi };
        let assets = claims::repay_assets(shares, &rb)?;
        let (m, loan) = (on.accounts(), market.params.loan_asset);
        let accounts = vec![read(m.market, lending), read(position, lending), sign(from, loan), read(m.loan_vault, loan)];
        let before = self.position_or_default(lending, position).await?;
        let ix = Instruction::Repay { params: market.params, shares, rb, window: c.window };
        self.send_lending(lending, accounts, ix, on.payer).await?;
        let after = self.position_or_default(lending, position).await?;
        ensure!(after.borrow_shares + shares == before.borrow_shares, "repay didn't apply");
        Ok(assets)
    }

    /// All of a position's supply shares or debt shares, for "withdraw all" / "repay all".
    pub async fn position_shares(&self, lending: AccountId, position: AccountId) -> Result<Position> {
        self.position(lending, position).await?.ok_or_else(|| anyhow!("{position} has never been used"))
    }
}
