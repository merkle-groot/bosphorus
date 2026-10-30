//! Lending: state readers, the claim builder and tx builders (plan 2.3, 4.1–4.2).
//!
//! Every op reads the market (and feed) first, builds its claims, and after inclusion checks that
//! the position moved by exactly what `plan` derives from those claims. A failed public tx still
//! lands and pays (constraints §5), so that check is how a failure shows up.
//!
//! **Private mode:** an op whose owner or payer is a `Private/` account goes out as a
//! privacy-preserving tx, proven here (constraints §7). Its position is salted
//! ([`Client::owner`]), its claims are sized for the proving delay ([`ClaimConfig::private`]), and
//! payouts go to the owner's own private account unless told otherwise.
//!
//! **Held shares (E9):** an [`Owner::held`] position is the owner's own account, so a private
//! lender's supply and withdraw publish no position at all; such a position can never borrow.

use std::collections::{BTreeMap, HashSet};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use bosphorus_core::{MarketOp, PositionOp, WAD, claims, irm, position};
pub use bosphorus_lending::{
    Claimed, Config as LendingConfig, HealthClaim, Instruction, Interval, IrmParams, Liquidation, LiquidationAmount,
    Market, MarketAccounts, MarketId, MarketParams, Position, Salt, Shard, Window,
};
use bosphorus_lending::{config_id, position_id};
pub use common::{HashType, transaction::LeeTransaction};
use hmac::{Hmac, Mac as _};
pub use lee_core::encryption::SharedSecretKey;
use lee::{
    AccountId, ProgramShardSelector, privacy_preserving_transaction::circuit::ProgramWithDependencies, program::Program,
};
use lee_core::program::PROGRAM_LOADER_ACCOUNT_ID;
use sequencer_service_rpc::RpcClient as _;
use wallet::{AccDecodeData, AccountIdentity, AccountMention};

pub use crate::now_ms;
use crate::{Client, Feed, Holder, LENDING_ELF, ORACLE_ELF, TOKEN_ELF, U256};

pub const PUBLIC_SALT: Salt = [0; 32];

fn sign(id: AccountId, program: AccountId) -> AccountMention {
    AccountIdentity::Public(id).select_program_shard(program)
}

fn read(id: AccountId, program: AccountId) -> AccountMention {
    AccountIdentity::PublicNoSign(id).select_program_shard(program)
}

/// A tx's accounts in layout order, remembering which are private (their notes get decoded).
#[derive(Default)]
struct Accounts {
    mentions: Vec<AccountMention>,
    private: Vec<AccountId>,
}

impl Accounts {
    /// A copy to send (`AccountMention` isn't `Clone`).
    fn mentions(&self) -> Vec<AccountMention> {
        self.mentions.iter().map(|m| m.identity.clone().select_program_shard(m.program_account_id)).collect()
    }

    /// An authorising account: a signing public account or an owned private one (with `ask`).
    /// The same account may be mentioned again for another shard (E9: held shares).
    fn signer(&mut self, holder: Holder, program: AccountId) -> &mut Self {
        if let Holder::Private(id) = holder
            && !self.private.contains(&id)
        {
            self.private.push(id);
        }
        self.mentions.push(holder.signer(program));
        self
    }

    fn read(&mut self, id: AccountId, program: AccountId) -> &mut Self {
        self.mentions.push(read(id, program));
        self
    }

    /// A position: `holder`'s own account if it holds the shares there (E9), otherwise read only.
    fn position(&mut self, position: AccountId, holder: Holder, program: AccountId) -> &mut Self {
        if position == holder.id() { self.signer(holder, program) } else { self.read(position, program) }
    }

    /// A payout account other than `owner` (public, or one of this wallet's private accounts).
    fn payout(&mut self, to: Option<Holder>, owner: Holder, program: AccountId) -> &mut Self {
        match to.filter(|to| to.id() != owner.id()) {
            Some(Holder::Public(id)) => self.read(id, program),
            Some(to @ Holder::Private(_)) => self.signer(to, program),
            None => self,
        }
    }
}

/// A position's owner: a wallet account plus the salt that, with it, derives the position
/// (design §4). Public owners use the zero salt. A `held` position is the owner's own account
/// (E9): a lender's shares in `shard[lending]` of its private account, with no public trace. It
/// can supply, withdraw and hold collateral, but never borrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Owner {
    pub holder: Holder,
    pub salt: Salt,
    pub held: bool,
}

impl Owner {
    pub fn public(id: AccountId) -> Self {
        Self { holder: Holder::Public(id), salt: PUBLIC_SALT, held: false }
    }

    pub fn held(holder: Holder) -> Self {
        Self { holder, salt: PUBLIC_SALT, held: true }
    }
}

/// `HMAC-SHA256(secret, "/bosphorus/v1/position-salt/" ‖ market_id ‖ owner ‖ index)`: unlinkable
/// salts that only the owner's keys can recompute, so positions are recoverable (design §4).
pub fn position_salt(secret: &[u8; 32], market: &MarketId, owner: &AccountId, index: u32) -> Salt {
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(secret).expect("HMAC takes any key length");
    mac.update(b"/bosphorus/v1/position-salt/");
    mac.update(market);
    mac.update(owner.as_ref());
    mac.update(&index.to_le_bytes());
    mac.finalize().into_bytes().into()
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
    /// `t` is rounded down to a multiple of this (1 = exact). Private txs publish their window, so
    /// an exact `t` would timestamp when proving began (P4-privacy-observed, leak 2).
    pub t_grid_ms: u64,
}

impl ClaimConfig {
    /// For private txs: valid for 20 min and room for 20 min of accrual by others (S4: proving
    /// takes ~10 min on an M1), a ratio width that covers the rate moving while proving, and a
    /// price claim 2% below the feed (the health check uses it). E7: under a 10-minute delay,
    /// `ahead` short of it never lands; ratio width 1e-9 lands 60%, 1e-8 97%, 1e-7 100%.
    pub fn private() -> Self {
        Self {
            window_ms: 20 * 60_000,
            ahead_ms: 20 * 60_000,
            ratio_width: WAD / 10_000_000,
            price_width: WAD / 50,
            t_grid_ms: 60_000,
            ..Self::default()
        }
    }
}

/// For public txs: valid for 10 min, which outlasts all but the longest testnet stalls (13.7 h
/// measured 2026-10-20: p99 gap 90 s, three of 5–10 min, one of 3.4 h; a public tx included after
/// its window still pays, findings F3); room for 30 s of accrual by others.
impl Default for ClaimConfig {
    fn default() -> Self {
        Self { window_ms: 600_000, lag_ms: 2_000, ahead_ms: 30_000, ratio_width: WAD / 1_000_000_000, price_width: WAD / 200, t_grid_ms: 1 }
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
        let t = t - t % self.t_grid_ms.max(1);
        // Ratios may have accrued up to `now` (rounding moved `t` back) and `ahead` beyond.
        let ahead = (now_ms - t) + self.ahead_ms;
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

    pub fn position(&self, owner: &Owner) -> AccountId {
        if owner.held {
            return owner.holder.id();
        }
        position_id(&self.lending, &self.market, &owner.holder.id(), &owner.salt)
    }
}

/// A position field an op moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    SupplyShares,
    Collateral,
    BorrowShares,
}

impl Field {
    pub fn of(self, p: &Position) -> u128 {
        match self {
            Self::SupplyShares => p.supply_shares,
            Self::Collateral => p.collateral,
            Self::BorrowShares => p.borrow_shares,
        }
    }

    pub fn set(self, p: &mut Position, value: u128) {
        match self {
            Self::SupplyShares => p.supply_shares = value,
            Self::Collateral => p.collateral = value,
            Self::BorrowShares => p.borrow_shares = value,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::SupplyShares => "supply_shares",
            Self::Collateral => "collateral",
            Self::BorrowShares => "borrow_shares",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        [Self::SupplyShares, Self::Collateral, Self::BorrowShares].into_iter().find(|f| f.name() == s).with_context(|| format!("no position field {s}"))
    }
}

/// What a sent op needs to be settled, by this process or (from what the app saved) a later one.
/// A failed tx changes nothing, so the position's `field` reaching `to` is how success shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expect {
    /// The instruction's name (`supply`, `borrow`, …).
    pub op: String,
    pub lending: AccountId,
    pub position: AccountId,
    pub field: Field,
    pub from: u128,
    pub to: u128,
    /// The op's result: shares minted (supply), assets paid out (withdraw), collateral moved,
    /// debt shares taken on (borrow), assets paid (repay).
    pub amount: u128,
    /// The window's end (ms): the tx can't apply at or after it (a public tx may still be included,
    /// and fail). `None` for ops without a window
    /// (supply_collateral, debt-free withdraw_collateral).
    pub valid_until: Option<u64>,
    /// This wallet's private accounts in the tx, in layout order (their notes get decoded).
    pub private: Vec<AccountId>,
    /// Why it may not have applied, for the error.
    pub hint: String,
}

impl Expect {
    fn new(ix: &Instruction, on: &On, position: AccountId, change: (Field, &Position, u128), amount: u128, window: Option<Window>, hint: &str) -> Result<Self> {
        let (field, before, to) = change;
        Ok(Self {
            op: ix.name().into(),
            lending: on.lending,
            position,
            field,
            from: field.of(before),
            to,
            amount,
            valid_until: window.map(|w| w.end()).transpose()?,
            private: vec![],
            hint: hint.into(),
        })
    }

    pub fn is_private(&self) -> bool {
        !self.private.is_empty()
    }

    /// The position is one of this wallet's private accounts (E9: held shares), so its state is
    /// only known once the tx's note is decoded.
    pub fn held(&self) -> bool {
        self.private.contains(&self.position)
    }
}

/// An op built from state just read and checked with `core` (the program's own maths), ready to
/// send. [`Client::send`] sends it; `expect` says what it will do.
pub struct Prepared {
    ix: Instruction,
    pub params: MarketParams,
    accounts: Accounts,
    pub payer: Option<AccountId>,
    pub expect: Expect,
}

impl Prepared {
    fn new(ix: Instruction, params: MarketParams, accounts: Accounts, payer: Option<AccountId>, mut expect: Expect) -> Result<Self> {
        ensure!(!zero_amount(&ix), "zero amount");
        expect.private = accounts.private.clone();
        Ok(Self { ix, params, accounts, payer, expect })
    }

    /// The first signing public account: who pays when no payer is given.
    fn public_signer(&self) -> Option<AccountId> {
        self.accounts.mentions.iter().find_map(|m| match m.identity {
            AccountIdentity::Public(id) => Some(id),
            _ => None,
        })
    }
}

/// A tx handed to the sequencer.
#[derive(Clone)]
pub struct Sent {
    pub hash: HashType,
    /// One per [`Expect::private`] account, to decode its note once the tx lands.
    pub secrets: Vec<SharedSecretKey>,
    pub submitted_ms: u64,
    pub proving_ms: Option<u64>,
    /// The payer and its native balance before (public txs), for the `[measure]` fee.
    pub fee_from: Option<(AccountId, u128)>,
}

/// Why the client refused to send, beyond what `core` reports ([`bosphorus_core::Error`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Findings F7.
    PayerCannotFund { payer: AccountId, balance: u128, reserve: u128 },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PayerCannotFund { payer, balance, reserve } => {
                write!(f, "payer {payer} holds {balance} native, below the sequencer's fee reserve {reserve}")
            }
        }
    }
}

impl std::error::Error for Refusal {}

/// Runs an op through `core` as the program would if it landed now, so a tx that would fail isn't
/// sent (a failed public tx still pays; a private one costs ~10 min of proving). Returns the
/// position after.
fn simulate(market: &Market, before: &Position, market_op: Option<(u64, MarketOp)>, op: &PositionOp) -> Result<Position> {
    if let Some((t, market_op)) = market_op {
        market.state.clone().apply(&market.params.irm, t, &market_op).context("refused before sending")?;
    }
    let mut after = before.clone();
    after.apply(op).context("refused before sending")?;
    Ok(after)
}

/// Findings F5: the feed must still be fresh at the window's end, or the tx can't land.
fn check_feed(price: &Interval<U256>, feed: &Feed, params: &MarketParams, window: &Window) -> Result<()> {
    let end = window.end()?;
    claims::check_price(price, feed.price, feed.updated_at, params.max_price_age, end).with_context(|| {
        format!(
            "refused before sending: the feed is {} s old, the market accepts {} s, and this tx stays valid {} s more (publish a fresh price first)",
            window.t.saturating_sub(feed.updated_at) / 1000,
            params.max_price_age / 1000,
            end.saturating_sub(window.t) / 1000,
        )
    })
}

/// A zero amount fails in `plan`, and a zero delta would pass the post-tx checks.
fn zero_amount(ix: &Instruction) -> bool {
    match ix {
        Instruction::Supply { assets, .. }
        | Instruction::SupplyCollateral { assets, .. }
        | Instruction::WithdrawCollateral { assets, .. }
        | Instruction::Borrow { assets, .. } => *assets == 0,
        Instruction::Withdraw { shares, .. } | Instruction::Repay { shares, .. } => *shares == 0,
        _ => false,
    }
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
    /// A private account of this wallet (E9: held shares) is read from the wallet's own copy.
    async fn lending_shard(&self, account: AccountId, lending: AccountId) -> Result<Option<Shard>> {
        let account = match self.wallet.get_account_private(account) {
            Some(account) => account,
            None => self.wallet.get_account_view(ProgramShardSelector::new(account, lending)).await?,
        };
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

    /// Every market under `lending`, found by scanning all blocks (see [`Scanner`]).
    pub async fn markets(&self, lending: AccountId) -> Result<Vec<(MarketId, Market)>> {
        let mut scanner = Scanner::new(lending, 1);
        scanner.scan(self).await?;
        let mut markets = vec![];
        for id in scanner.markets {
            markets.push((id, self.market(lending, &id).await?));
        }
        Ok(markets)
    }

    async fn send_lending(&self, lending: AccountId, accounts: Vec<AccountMention>, ix: Instruction, payer: Option<AccountId>) -> Result<()> {
        ensure!(!zero_amount(&ix), "zero amount");
        self.send_public(&format!("lending.{}", ix.name()), lending, accounts, ix, payer).await?;
        Ok(())
    }

    /// `lending` and the market's assets, each checked to run this client's bytecode: a tx that
    /// moves tokens runs the asset programs with the signer's authority over all its balances
    /// (findings F1), and a private tx must be proven with exactly the deployed code.
    async fn market_programs(&self, lending: AccountId, params: &MarketParams) -> Result<ProgramWithDependencies> {
        let deps: Vec<_> = params.checked_assets().into_iter().map(|asset| (asset, TOKEN_ELF)).collect();
        self.program_with(lending, LENDING_ELF, &deps).await
    }

    /// Errors unless the market's lending program, assets and oracle all run this client's
    /// bytecode. A keeper checks this before acting on a market it discovered (findings F1).
    pub async fn check_market_code(&self, lending: AccountId, params: &MarketParams) -> Result<()> {
        self.market_programs(lending, params).await?;
        self.check_program(params.oracle_program, ORACLE_ELF).await
    }

    /// Sends a prepared op: a public tx, or a private one if any account is private (proven here,
    /// ~10 min on the testnet; fee-exempt). Doesn't wait for inclusion.
    pub async fn send(&self, p: &Prepared) -> Result<Sent> {
        let program = self.market_programs(p.expect.lending, &p.params).await?;
        let data = Program::serialize_instruction(&p.ix)?;
        if !p.expect.is_private() {
            let payer = p.payer.or_else(|| p.public_signer());
            let fee_from = match payer {
                Some(payer) => Some((payer, self.check_payer(payer).await?)),
                None => None,
            };
            let submitted_ms = now_ms()?;
            let hash = self
                .wallet
                .send_pub_tx_paid_by(p.accounts.mentions(), data, p.expect.lending, p.payer)
                .await
                .map_err(crate::send_error)?;
            return Ok(Sent { hash, secrets: vec![], submitted_ms, proving_ms: None, fee_from });
        }
        let started = now_ms()?;
        let (hash, secrets) = self
            .wallet
            .send_privacy_preserving_tx(p.accounts.mentions(), data, &program)
            .await
            .map_err(crate::send_error)?;
        let submitted_ms = now_ms()?;
        eprintln!("private tx {hash} proved and sent in {:.1} s", (submitted_ms - started) as f64 / 1000.0);
        ensure!(secrets.len() == p.expect.private.len(), "{} shared secrets for {} private accounts", secrets.len(), p.expect.private.len());
        Ok(Sent { hash, secrets, submitted_ms, proving_ms: Some(submitted_ms - started), fee_from: None })
    }

    /// Findings F7: the sequencer refuses a tx whose payer can't cover its fee reserve, with an
    /// opaque error. Checks the execution part (`gas_limit × base_fee_exec`) and returns the
    /// payer's balance.
    pub(crate) async fn check_payer(&self, payer: AccountId) -> Result<u128> {
        let balance = self.native_balance(payer).await?;
        let fees = self.wallet.client_owned().get_fee_state().await?;
        let reserve = u128::from(self.wallet.config().gas_limit) * u128::from(fees.base_fee_exec);
        if balance < reserve {
            return Err(anyhow::Error::new(Refusal::PayerCannotFund { payer, balance, reserve }));
        }
        Ok(balance)
    }

    /// Records a landed tx: the `[measure]` line, its private notes into the wallet, then checks
    /// the position moved as expected.
    pub async fn finish(&mut self, expect: &Expect, sent: &Sent, tx: &LeeTransaction, block: u64) -> Result<()> {
        eprintln!("tx {} included in block {block}", sent.hash);
        let fee = match sent.fee_from {
            Some((payer, before)) => Some(before.saturating_sub(self.native_balance(payer).await?)),
            None => None,
        };
        let kind = if expect.is_private() { "private" } else { "public" };
        self.measure(&format!("lending.{}", expect.op), kind, sent.hash, sent.submitted_ms, sent.proving_ms, block, fee).await;
        if expect.is_private() {
            let LeeTransaction::PrivacyPreserving(tx) = tx else { bail!("expected a private tx") };
            let decode: Vec<_> = sent.secrets.iter().zip(&expect.private).map(|(secret, id)| AccDecodeData::Decode(*secret, *id)).collect();
            self.wallet.decode_insert_privacy_preserving_transaction_results(tx, &decode)?;
            self.wallet.store_persistent_data()?;
        }
        let now = self.field_now(expect).await?;
        ensure!(now == expect.to, "{} (position {} {} is {now}, expected {})", expect.hint, expect.field.name(), expect.from, expect.to);
        Ok(())
    }

    /// The position's `expect.field` as it is now. A held position (E9) is read from the wallet's
    /// copy, which only moves once its note is decoded.
    pub async fn field_now(&self, expect: &Expect) -> Result<u128> {
        Ok(expect.field.of(&self.position_or_default(expect.lending, expect.position).await?))
    }

    /// Sends, waits for inclusion and checks. Returns `expect.amount`.
    pub async fn execute(&mut self, p: Prepared) -> Result<u128> {
        let sent = self.send(&p).await?;
        let not_included = if p.expect.is_private() { "private tx not included (a claim failed at settlement?)" } else { "tx not included" };
        let (tx, block) = self.wallet.poll_transaction(sent.hash).await.context(not_included)?;
        self.finish(&p.expect, &sent, &tx, block).await?;
        Ok(p.expect.amount)
    }

    /// `holder`'s position number `index` in a market. Private owners get a derived salt; public
    /// owners have only index 0 (zero salt).
    pub fn owner(&self, holder: Holder, market: &MarketId, index: u32) -> Result<Owner> {
        match holder {
            Holder::Public(id) => {
                ensure!(index == 0, "public owners have one position per market (index 0)");
                Ok(Owner::public(id))
            }
            Holder::Private(id) => {
                let found = self
                    .wallet
                    .storage()
                    .key_chain()
                    .private_account(id)
                    .with_context(|| format!("{id} isn't a private account in this wallet"))?;
                let secret = found.key_chain.secret_spending_key.0;
                Ok(Owner { holder, salt: position_salt(&secret, market, &id, index), held: false })
            }
        }
    }

    /// Recovery (design §4): `holder`'s positions in a market, walking indices until one is unused.
    pub async fn owned_positions(&self, lending: AccountId, market: &MarketId, holder: Holder) -> Result<Vec<(u32, AccountId, Position)>> {
        let mut found = vec![];
        for index in 0.. {
            let owner = self.owner(holder, market, index)?;
            let id = position_id(&lending, market, &holder.id(), &owner.salt);
            match self.position(lending, id).await? {
                Some(p) => found.push((index, id, p)),
                None => break,
            }
            if matches!(holder, Holder::Public(_)) {
                break;
            }
        }
        Ok(found)
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

    /// The assets' image ids are read from their headers; the config must have enabled them.
    pub async fn create_market(&self, lending: AccountId, params: MarketParams, cfg: &ClaimConfig, payer: AccountId) -> Result<MarketId> {
        let id = params.id();
        let window = Window { t: now_ms()?.saturating_sub(cfg.lag_ms), w: cfg.window_ms };
        let mut accounts = vec![read(config_id(&lending), lending), read(MarketAccounts::new(&lending, &id).market, lending)];
        let mut images = vec![];
        for asset in params.checked_assets() {
            images.push(self.header(asset).await?.image_id);
            accounts.push(read(asset, PROGRAM_LOADER_ACCOUNT_ID));
        }
        self.send_lending(lending, accounts, Instruction::CreateMarket { params, window, images }, Some(payer)).await?;
        self.market(lending, &id)
            .await
            .context("create_market didn't apply (LLTV, IRM and asset bytecode enabled? already exists?)")?;
        Ok(id)
    }

    /// Supplies `assets` from `from` to `position`. Returns the shares minted.
    pub async fn supply(&mut self, on: &On, from: Holder, position: AccountId, assets: u128) -> Result<u128> {
        let p = self.prepare_supply(on, from, position, assets).await?;
        self.execute(p).await
    }

    pub async fn prepare_supply(&self, on: &On, from: Holder, position: AccountId, assets: u128) -> Result<Prepared> {
        let (lending, market) = (on.lending, self.market(on.lending, &on.market).await?);
        let c = on.claims.claims(&market, now_ms()?)?;
        let rs = Interval { lo: 0, hi: c.rs.hi };
        let shares = claims::supply_shares(assets, &rs)?;
        let (m, loan) = (on.accounts(), market.params.loan_asset);
        let mut accounts = Accounts::default();
        accounts.read(m.market, lending).position(position, from, lending).signer(from, loan).read(m.loan_vault, loan);
        let before = self.position_or_default(lending, position).await?;
        simulate(&market, &before, Some((c.window.t, MarketOp::Supply { rs, assets, shares })), &PositionOp::Supply { shares })?;
        let ix = Instruction::Supply { params: market.params.clone(), assets, rs, window: c.window };
        let expect = Expect::new(&ix, on, position, (Field::SupplyShares, &before, before.supply_shares + shares), shares, Some(c.window), "supply didn't apply")?;
        Prepared::new(ix, market.params, accounts, on.payer, expect)
    }

    /// Withdraws `shares` of `owner`'s supply to `to` (default: the owner). Returns the assets.
    pub async fn withdraw(&mut self, on: &On, owner: &Owner, shares: u128, to: Option<Holder>) -> Result<u128> {
        let p = self.prepare_withdraw(on, owner, shares, to).await?;
        self.execute(p).await
    }

    pub async fn prepare_withdraw(&self, on: &On, owner: &Owner, shares: u128, to: Option<Holder>) -> Result<Prepared> {
        let (lending, market) = (on.lending, self.market(on.lending, &on.market).await?);
        let c = on.claims.claims(&market, now_ms()?)?;
        let rs = Interval { lo: c.rs.lo, hi: u128::MAX };
        let assets = claims::withdraw_assets(shares, &rs)?;
        let (m, loan, position) = (on.accounts(), market.params.loan_asset, on.position(owner));
        let mut accounts = Accounts::default();
        accounts.signer(owner.holder, loan).read(m.market, lending).position(position, owner.holder, lending).read(m.loan_vault, loan);
        accounts.payout(to, owner.holder, loan);
        let before = self.position_or_default(lending, position).await?;
        simulate(&market, &before, Some((c.window.t, MarketOp::Withdraw { rs, shares, assets })), &PositionOp::Withdraw { shares })?;
        let ix = Instruction::Withdraw { params: market.params.clone(), shares, salt: owner.salt, rs, window: c.window };
        let to = before.supply_shares - shares;
        let expect = Expect::new(&ix, on, position, (Field::SupplyShares, &before, to), assets, Some(c.window), "withdraw didn't apply (liquidity? shares?)")?;
        Prepared::new(ix, market.params, accounts, on.payer, expect)
    }

    pub async fn supply_collateral(&mut self, on: &On, from: Holder, position: AccountId, assets: u128) -> Result<()> {
        let p = self.prepare_supply_collateral(on, from, position, assets).await?;
        self.execute(p).await.map(drop)
    }

    pub async fn prepare_supply_collateral(&self, on: &On, from: Holder, position: AccountId, assets: u128) -> Result<Prepared> {
        // The market isn't touched by this op, so check it exists here.
        let (lending, market) = (on.lending, self.market(on.lending, &on.market).await?);
        let (m, coll) = (on.accounts(), market.params.collateral_asset);
        let mut accounts = Accounts::default();
        accounts.position(position, from, lending).signer(from, coll).read(m.collateral_vault, coll);
        let before = self.position_or_default(lending, position).await?;
        simulate(&market, &before, None, &PositionOp::SupplyCollateral { assets })?;
        let ix = Instruction::SupplyCollateral { params: market.params.clone(), assets };
        let expect = Expect::new(&ix, on, position, (Field::Collateral, &before, before.collateral + assets), assets, None, "supply_collateral didn't apply")?;
        Prepared::new(ix, market.params, accounts, on.payer, expect)
    }

    /// Without debt this skips the oracle and the market (so it works with a stale feed).
    pub async fn withdraw_collateral(&mut self, on: &On, owner: &Owner, assets: u128, to: Option<Holder>) -> Result<()> {
        let p = self.prepare_withdraw_collateral(on, owner, assets, to).await?;
        self.execute(p).await.map(drop)
    }

    pub async fn prepare_withdraw_collateral(&self, on: &On, owner: &Owner, assets: u128, to: Option<Holder>) -> Result<Prepared> {
        let (lending, market) = (on.lending, self.market(on.lending, &on.market).await?);
        let (m, coll, position) = (on.accounts(), market.params.collateral_asset, on.position(owner));
        let before = self.position_or_default(lending, position).await?;
        let mut accounts = Accounts::default();
        accounts.signer(owner.holder, coll);
        let health = if before.borrow_shares == 0 {
            accounts.position(position, owner.holder, lending);
            simulate(&market, &before, None, &PositionOp::WithdrawCollateral { assets, health: None })?;
            None
        } else {
            let c = on.claims.claims(&market, now_ms()?)?;
            let feed = self.feed(market.params.oracle_feed, market.params.oracle_program).await?;
            let oracle = (market.params.oracle_feed, market.params.oracle_program);
            accounts.read(m.market, lending).position(position, owner.holder, lending).read(oracle.0, oracle.1);
            let claim = HealthClaim { price: on.claims.price(&feed)?, rb: Interval { lo: 0, hi: c.rb.hi }, window: c.window };
            check_feed(&claim.price, &feed, &market.params, &c.window)?;
            let health = Some(claims::health(&claim.price, &claim.rb, market.params.lltv)?);
            simulate(&market, &before, Some((c.window.t, MarketOp::Check { rb: claim.rb })), &PositionOp::WithdrawCollateral { assets, health })?;
            Some(claim)
        };
        accounts.read(m.collateral_vault, coll).payout(to, owner.holder, coll);
        let window = health.map(|h| h.window);
        let ix = Instruction::WithdrawCollateral { params: market.params.clone(), assets, salt: owner.salt, health };
        let to = before.collateral - assets;
        let expect = Expect::new(&ix, on, position, (Field::Collateral, &before, to), assets, window, "withdraw_collateral didn't apply (health? price?)")?;
        Prepared::new(ix, market.params, accounts, on.payer, expect)
    }

    /// Borrows `assets` against `owner`'s position, paid to `to` (default: the owner). Returns the
    /// debt shares.
    pub async fn borrow(&mut self, on: &On, owner: &Owner, assets: u128, to: Option<Holder>) -> Result<u128> {
        let p = self.prepare_borrow(on, owner, assets, to).await?;
        self.execute(p).await
    }

    pub async fn prepare_borrow(&self, on: &On, owner: &Owner, assets: u128, to: Option<Holder>) -> Result<Prepared> {
        ensure!(!owner.held, "held positions can't borrow (E9): use a salted position");
        let (lending, market) = (on.lending, self.market(on.lending, &on.market).await?);
        let feed = self.feed(market.params.oracle_feed, market.params.oracle_program).await?;
        let c = on.claims.claims(&market, now_ms()?)?;
        let claim = HealthClaim { price: on.claims.price(&feed)?, rb: c.rb, window: c.window };
        check_feed(&claim.price, &feed, &market.params, &c.window)?;
        let shares = claims::borrow_shares(assets, &c.rb)?;
        let (m, loan, position) = (on.accounts(), market.params.loan_asset, on.position(owner));
        let mut accounts = Accounts::default();
        accounts
            .signer(owner.holder, loan)
            .read(m.market, lending)
            .read(position, lending)
            .read(market.params.oracle_feed, market.params.oracle_program)
            .read(m.loan_vault, loan)
            .payout(to, owner.holder, loan);
        let before = self.position_or_default(lending, position).await?;
        let health = claims::health(&claim.price, &claim.rb, market.params.lltv)?;
        simulate(&market, &before, Some((c.window.t, MarketOp::Borrow { rb: c.rb, assets, shares })), &PositionOp::Borrow { shares, health })?;
        let ix = Instruction::Borrow { params: market.params.clone(), assets, salt: owner.salt, health: claim };
        let to = before.borrow_shares + shares;
        let expect = Expect::new(&ix, on, position, (Field::BorrowShares, &before, to), shares, Some(c.window), "borrow didn't apply (health? liquidity? price fresh?)")?;
        Prepared::new(ix, market.params, accounts, on.payer, expect)
    }

    /// Repays `shares` of `position`'s debt from `from`. Returns the assets paid.
    pub async fn repay(&mut self, on: &On, from: Holder, position: AccountId, shares: u128) -> Result<u128> {
        let p = self.prepare_repay(on, from, position, shares).await?;
        self.execute(p).await
    }

    pub async fn prepare_repay(&self, on: &On, from: Holder, position: AccountId, shares: u128) -> Result<Prepared> {
        let (lending, market) = (on.lending, self.market(on.lending, &on.market).await?);
        let c = on.claims.claims(&market, now_ms()?)?;
        let rb = Interval { lo: 0, hi: c.rb.hi };
        let assets = claims::repay_assets(shares, &rb)?;
        let (m, loan) = (on.accounts(), market.params.loan_asset);
        let mut accounts = Accounts::default();
        accounts.read(m.market, lending).position(position, from, lending).signer(from, loan).read(m.loan_vault, loan);
        let before = self.position_or_default(lending, position).await?;
        simulate(&market, &before, Some((c.window.t, MarketOp::Repay { rb, shares, assets })), &PositionOp::Repay { shares })?;
        let ix = Instruction::Repay { params: market.params.clone(), shares, rb, window: c.window };
        let to = before.borrow_shares - shares;
        let expect = Expect::new(&ix, on, position, (Field::BorrowShares, &before, to), assets, Some(c.window), "repay didn't apply")?;
        Prepared::new(ix, market.params, accounts, on.payer, expect)
    }

    /// Liquidates `position` in full if it is liquidatable now (see [`quote_liquidation`]); the
    /// seized collateral goes to `to` (default: the liquidator). `Ok(None)` if it isn't.
    pub async fn liquidate(&self, on: &On, liquidator: AccountId, position: AccountId, to: Option<AccountId>) -> Result<Option<Liquidation>> {
        let (lending, market) = (on.lending, self.market(on.lending, &on.market).await?);
        let feed = self.feed(market.params.oracle_feed, market.params.oracle_program).await?;
        let Some(before) = self.position(lending, position).await? else { return Ok(None) };
        let Some(q) = quote_liquidation(&market, &before, &feed, &on.claims, now_ms()?)? else { return Ok(None) };
        let (m, params) = (on.accounts(), &market.params);
        self.market_programs(lending, params).await?;
        let balance = self.token_balance(params.loan_asset, crate::Holder::Public(liquidator)).await?;
        ensure!(balance >= q.liquidation.repaid_assets, "liquidator holds {balance}, needs {}", q.liquidation.repaid_assets);
        let mut accounts = vec![
            sign(liquidator, params.loan_asset),
            read(m.market, lending),
            read(position, lending),
            read(params.oracle_feed, params.oracle_program),
            read(m.loan_vault, params.loan_asset),
            read(m.collateral_vault, params.collateral_asset),
        ];
        accounts.extend(to.filter(|to| *to != liquidator).map(|to| read(to, params.collateral_asset)));
        let ix = Instruction::Liquidate {
            params: market.params.clone(),
            claimed: q.claimed,
            amount: q.amount,
            price: q.price,
            rb: q.rb,
            window: q.window,
        };
        self.send_lending(lending, accounts, ix, on.payer).await?;
        // Not the position: its owner (or another liquidator) may have moved it since. Only the
        // liquidator can debit its own balance, so an exact drop means our tx applied.
        let after = self.token_balance(params.loan_asset, crate::Holder::Public(liquidator)).await?;
        let l = q.liquidation;
        ensure!(after + l.repaid_assets == balance, "liquidation didn't apply (price or position moved?)");
        Ok(Some(l))
    }

    /// Moves the market's pending fee shares to the fee recipient's position. Returns the shares.
    pub async fn claim_fees(&self, on: &On) -> Result<u128> {
        let (lending, market) = (on.lending, self.market(on.lending, &on.market).await?);
        let recipient = self.lending_config(lending).await?.fee_recipient;
        let shares = market.state.pending_fee_shares;
        ensure!(shares > 0, "no pending fees (they accrue with interest when the market is touched)");
        let accounts = vec![read(config_id(&lending), lending), read(on.accounts().market, lending), read(on.position_of(recipient), lending)];
        self.send_lending(lending, accounts, Instruction::ClaimFees { params: market.params, recipient, shares }, on.payer).await?;
        let after = self.market(lending, &on.market).await?;
        ensure!(after.state.pending_fee_shares + shares == market.state.pending_fee_shares, "claim_fees didn't apply");
        Ok(shares)
    }

    /// All of a position's supply shares or debt shares, for "withdraw all" / "repay all".
    pub async fn position_shares(&self, lending: AccountId, position: AccountId) -> Result<Position> {
        self.position(lending, position).await?.ok_or_else(|| anyhow!("{position} has never been used"))
    }
}

/// A full liquidation as it would be submitted now.
#[derive(Debug, Clone, Copy)]
pub struct LiquidationQuote {
    pub claimed: Claimed,
    pub amount: LiquidationAmount,
    pub price: Interval<U256>,
    pub rb: Interval<u128>,
    pub window: Window,
    pub liquidation: Liquidation,
}

/// `Some` if `position` is liquidatable at the claims a keeper makes now: the exact position, the
/// price `[0, feed]` (a higher price may land meanwhile; then it fails, as it should), and the
/// ratio claim from `cfg`. Errors if the feed is too old for the window.
pub fn quote_liquidation(market: &Market, position: &Position, feed: &Feed, cfg: &ClaimConfig, now_ms: u64) -> Result<Option<LiquidationQuote>> {
    if position.borrow_shares == 0 {
        return Ok(None);
    }
    let lltv = market.params.lltv;
    let c = cfg.claims(market, now_ms)?;
    let price = Interval { lo: U256::ZERO, hi: feed.price };
    claims::check_price(&price, feed.price, feed.updated_at, market.params.max_price_age, c.window.end()?)
        .map_err(|e| anyhow!("{e}: feed updated at {} ms", feed.updated_at))?;
    if position::is_healthy(position.collateral, position.borrow_shares, &claims::liquidation_health(&price, &c.rb, lltv)?)? {
        return Ok(None);
    }
    let claimed = Claimed { collateral: position.collateral, borrow_shares: position.borrow_shares };
    let amount = claims::full_liquidation(&claimed, &price, &c.rb, lltv)?;
    let liquidation = claims::liquidation(amount, &claimed, &price, &c.rb, lltv)?;
    Ok(Some(LiquidationQuote { claimed, amount, price, rb: c.rb, window: c.window, liquidation }))
}

/// Finds lending markets and positions by scanning sequencer blocks (S6: private-tx events are
/// dropped and there's no public indexer). Public txs name every lending shard they touch in
/// their shard selectors; private txs in `public_actions` whose effects target lending's shard.
/// Each newly seen account is read once and classified.
pub struct Scanner {
    pub lending: AccountId,
    pub next_block: u64,
    pub markets: std::collections::BTreeSet<MarketId>,
    /// Position → its market.
    pub positions: BTreeMap<AccountId, MarketId>,
    /// Block ranges the sequencer didn't return (seen on localnet after a full disk: the state has
    /// the blocks, the block store doesn't). Whatever they touched is missing from the scan.
    pub missing: Vec<(u64, u64)>,
    seen: HashSet<AccountId>,
}

impl Scanner {
    pub fn new(lending: AccountId, from_block: u64) -> Self {
        Self { lending, next_block: from_block.max(1), markets: Default::default(), positions: BTreeMap::new(), missing: vec![], seen: HashSet::new() }
    }

    /// Scans up to the chain head. Returns the positions found in this call.
    pub async fn scan(&mut self, client: &Client) -> Result<Vec<AccountId>> {
        const CHUNK: u64 = 100;
        let rpc = client.wallet.client_owned();
        let last = rpc.get_last_block_id().await?;
        let mut found = vec![];
        while self.next_block <= last {
            let end = (self.next_block + CHUNK - 1).min(last);
            let mut candidates = vec![];
            let mut expected = self.next_block;
            for block in rpc.get_block_range(self.next_block, end).await? {
                let id = block.header.block_id;
                if id > expected {
                    self.missing.push((expected, id - 1));
                }
                expected = expected.max(id + 1);
                for tx in &block.body.transactions {
                    candidates.extend(lending_accounts(tx, self.lending));
                }
            }
            for account in candidates {
                if self.seen.contains(&account) {
                    continue;
                }
                // An empty shard means that tx failed; look again if the account shows up later.
                match client.lending_shard(account, self.lending).await? {
                    None => continue,
                    Some(Shard::Market(m)) => {
                        self.markets.insert(m.params.id());
                    }
                    Some(Shard::Position(p)) => {
                        self.positions.insert(account, p.market_id);
                        found.push(account);
                    }
                    Some(Shard::Config(_)) => {}
                }
                self.seen.insert(account);
            }
            if expected <= end {
                self.missing.push((expected, end));
            }
            self.next_block = end + 1;
        }
        Ok(found)
    }
}

/// The accounts whose lending shard `tx` touches.
fn lending_accounts(tx: &LeeTransaction, lending: AccountId) -> Vec<AccountId> {
    match tx {
        LeeTransaction::Public(tx) => tx
            .message
            .shard_selectors
            .iter()
            .filter(|s| s.program_account_id == lending)
            .map(|s| s.account_id)
            .collect(),
        LeeTransaction::PrivacyPreserving(tx) => tx
            .message
            .public_actions
            .iter()
            .filter(|a| a.effects.iter().any(|e| e.shard_program_account_id == lending))
            .map(|a| a.account_id)
            .collect(),
    }
}
