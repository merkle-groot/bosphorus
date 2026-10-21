//! The Bosphorus app API: JSON in, JSON out, for front-ends (a Basecamp module, a local web UI).
//!
//! One [`App`] per wallet and lending program. Reads answer directly. Txs run as jobs ([`jobs`]):
//! one at a time, saved at every step, so a restart, even in the middle of a 10-minute proof,
//! settles them by the position's state instead of losing track of them.
//!
//! Conventions: accounts are `Public/<id>` / `Private/<id>`, market ids are hex, amounts are base
//! units and WAD fractions are decimals (`"0.8"`), both as strings (a u128 doesn't fit a JSON
//! number). Errors carry a `code`: `core`'s refusals in snake case (`stale_price`, `unhealthy`,
//! `insufficient_liquidity`, …), `payer_cannot_fund`, `not_included`, `bad_request`,
//! `unknown_method`, `not_found`, `timeout`, or `failed`.

pub mod jobs;

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context as _, Result, anyhow, ensure};
use bosphorus_client::{
    Client, Feed, Holder, LENDING_ELF, ORACLE_ELF, TOKEN_ELF, hex_id, image_id,
    lending::{
        ClaimConfig, Expect, HashType, Market, MarketAccounts, MarketId, On, Owner, Position, Prepared, Refusal, Scanner, Sent,
        SharedSecretKey, market_id_hex, market_status, now_ms, parse_market_id, position_status,
    },
};
use bosphorus_core::{U256, WAD, format_wad};
use lee::AccountId;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::sync::{RwLock, broadcast, mpsc};

use jobs::{Expected, Job, State, Store, Verdict, settle};

/// How often a job looks for its tx (the localnet makes a block every 15 s, the testnet every 10 s).
const POLL: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpKind {
    Supply,
    Withdraw,
    SupplyCollateral,
    WithdrawCollateral,
    Borrow,
    Repay,
}

/// One op, as `quote` and `submit` take it. `amount` is base units, or `"all"` for withdraw,
/// withdraw_collateral and repay; withdraw and repay count shares, the others assets. A private
/// `from`, `owner` or `to` makes it a private tx.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpRequest {
    pub op: OpKind,
    pub market: String,
    /// Where the tokens come from (supply, supply_collateral, repay).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The position's owner (default: `from`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Which of a private owner's positions in this market (salt index).
    #[serde(default)]
    pub index: u32,
    /// E9: the shares sit on the owner's own private account, with no public position.
    #[serde(default)]
    pub held: bool,
    pub amount: String,
    /// Where payouts go (withdraw, withdraw_collateral, borrow; default: the owner).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// Fee payer for public txs (default: the signer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payer: Option<String>,
}

enum Amount {
    All,
    Exact(u128),
}

impl Amount {
    fn exact(&self, op: &str) -> Result<u128> {
        match self {
            Self::Exact(x) => Ok(*x),
            Self::All => Err(coded("bad_request", format!("{op} takes an amount, not \"all\""))),
        }
    }
}

/// The parsed parts of an [`OpRequest`], checked without touching the chain.
struct Parsed {
    market: MarketId,
    from: Option<Holder>,
    owner: Option<Holder>,
    to: Option<Holder>,
    payer: Option<AccountId>,
    amount: Amount,
}

impl OpRequest {
    fn parse(&self) -> Result<Parsed> {
        let holder = |s: &Option<String>, what: &str| -> Result<Option<Holder>> {
            s.as_deref().map(|s| s.parse().map_err(|e| coded("bad_request", format!("{what}: {e:#}")))).transpose()
        };
        let amount = match self.amount.as_str() {
            "all" => Amount::All,
            s => Amount::Exact(s.parse().map_err(|_| coded("bad_request", format!("amount: expected base units or \"all\", got {s:?}")))?),
        };
        Ok(Parsed {
            market: parse_market_id(&self.market).map_err(|e| coded("bad_request", format!("market: {e:#}")))?,
            from: holder(&self.from, "from")?,
            owner: holder(&self.owner, "owner")?,
            to: holder(&self.to, "to")?,
            payer: holder(&self.payer, "payer")?.map(Holder::id),
            amount,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

impl ApiError {
    pub fn from_anyhow(e: &anyhow::Error) -> Self {
        let message = format!("{e:#}");
        let code = e.chain().find_map(|cause| {
            if let Some(c) = cause.downcast_ref::<Coded>() {
                Some(c.code.to_string())
            } else if let Some(core) = cause.downcast_ref::<bosphorus_core::Error>() {
                Some(snake_case(&format!("{core:?}")))
            } else {
                cause.downcast_ref::<Refusal>().map(|r| match r {
                    Refusal::PayerCannotFund { .. } => "payer_cannot_fund".to_string(),
                })
            }
        });
        Self { code: code.unwrap_or_else(|| "failed".into()), message }
    }
}

/// An error with an API code.
#[derive(Debug)]
struct Coded {
    code: &'static str,
    message: String,
}

impl std::fmt::Display for Coded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Coded {}

fn coded(code: &'static str, message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(Coded { code, message: message.into() })
}

fn snake_case(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

fn params<T: DeserializeOwned>(v: Value) -> Result<T> {
    let v = if v.is_null() { json!({}) } else { v };
    serde_json::from_value(v).map_err(|e| coded("bad_request", e.to_string()))
}

/// WAD as an exact decimal.
fn wad(x: u128) -> String {
    format_wad(x, 18)
}

fn wad256(x: U256) -> String {
    let w = U256::from(WAD);
    format!("{}.{:018}", x / w, u128::try_from(x % w).expect("below WAD"))
}

fn public(id: AccountId) -> String {
    Holder::Public(id).to_string()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex32(s: &str) -> Result<[u8; 32]> {
    ensure!(s.len() == 64, "expected 32 bytes of hex");
    let mut out = [0; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[2 * i..2 * i + 2], 16)?;
    }
    Ok(out)
}

pub struct Config {
    pub lending: AccountId,
    /// Jobs and the market scan cache.
    pub data_dir: PathBuf,
    /// Where the market scan starts (the lending program's deploy block saves scanning the chain).
    pub scan_from: u64,
}

/// Markets found so far, saved so a restart doesn't rescan the chain.
#[derive(Debug, Default, Serialize, Deserialize)]
struct MarketScan {
    next_block: u64,
    markets: Vec<String>,
}

#[derive(Clone)]
pub struct App {
    inner: Arc<Inner>,
}

struct Inner {
    client: RwLock<Client>,
    config: Config,
    store: Store,
    jobs: Mutex<BTreeMap<String, Job>>,
    queue: mpsc::UnboundedSender<String>,
    events: broadcast::Sender<Value>,
    scan: tokio::sync::Mutex<MarketScan>,
    next_id: AtomicU64,
}

impl App {
    /// Opens the wallet at `LEE_WALLET_HOME_DIR` and resumes unfinished jobs. Needs a tokio runtime.
    pub async fn open(config: Config) -> Result<Self> {
        let client = Client::from_env().context("opening the wallet (is LEE_WALLET_HOME_DIR set?)")?;
        let (store, mut jobs) = Store::open(config.data_dir.join("jobs"))?;
        // A job left preparing sent nothing: run it again.
        for job in jobs.values_mut().filter(|j| j.state == State::Preparing) {
            job.state = State::Queued;
            store.save(job)?;
        }
        // Ids sort by creation time.
        let resume: Vec<_> = jobs.values().filter(|j| !j.state.terminal()).map(|j| j.id.clone()).collect();
        let scan = std::fs::read(scan_path(&config))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or(MarketScan { next_block: config.scan_from, markets: vec![] });
        let (queue, rx) = mpsc::unbounded_channel();
        let (events, _) = broadcast::channel(256);
        let inner = Arc::new(Inner {
            client: RwLock::new(client),
            config,
            store,
            jobs: Mutex::new(jobs),
            queue,
            events,
            scan: tokio::sync::Mutex::new(scan),
            next_id: AtomicU64::new(0),
        });
        tokio::spawn(worker(inner.clone(), rx, resume));
        Ok(Self { inner })
    }

    /// Every job change, as the job's view.
    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.inner.events.subscribe()
    }

    /// `{"id", "method", "params"}` → `{"id", "result"}` or `{"id", "error": {"code", "message"}}`.
    pub async fn handle(&self, request: &str) -> Value {
        #[derive(Deserialize)]
        struct Request {
            #[serde(default)]
            id: Value,
            method: String,
            #[serde(default)]
            params: Value,
        }
        match serde_json::from_str::<Request>(request) {
            Err(e) => json!({ "id": null, "error": ApiError { code: "bad_request".into(), message: e.to_string() } }),
            Ok(r) => match self.call(&r.method, r.params).await {
                Ok(result) => json!({ "id": r.id, "result": result }),
                Err(error) => json!({ "id": r.id, "error": error }),
            },
        }
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value, ApiError> {
        self.dispatch(method, params).await.map_err(|e| ApiError::from_anyhow(&e))
    }

    async fn dispatch(&self, method: &str, p: Value) -> Result<Value> {
        #[derive(Deserialize)]
        struct MarketP {
            market: String,
        }
        #[derive(Deserialize)]
        struct AccountMarketP {
            account: String,
            market: String,
        }
        #[derive(Deserialize)]
        struct OwnerP {
            market: String,
            owner: String,
        }
        #[derive(Deserialize)]
        struct JobP {
            id: String,
            #[serde(default)]
            timeout_s: Option<u64>,
        }
        let market = |s: &str| parse_market_id(s).map_err(|e| coded("bad_request", format!("market: {e:#}")));
        let holder = |s: &str| s.parse::<Holder>().map_err(|e| coded("bad_request", format!("{e:#}")));
        match method {
            "info" => self.info().await,
            "accounts" => self.accounts().await,
            "balances" => {
                let p: AccountMarketP = params(p)?;
                self.balances(holder(&p.account)?, market(&p.market)?).await
            }
            "markets" => self.markets().await,
            "market" => self.market(market(&params::<MarketP>(p)?.market)?).await,
            "positions" => {
                let p: OwnerP = params(p)?;
                self.positions(market(&p.market)?, holder(&p.owner)?).await
            }
            "quote" => self.quote(params(p)?).await,
            "submit" => self.submit(params(p)?),
            "job" => Ok(self.inner.job(&params::<JobP>(p)?.id)?.view()),
            "jobs" => Ok(Value::Array(self.inner.jobs.lock().expect("jobs lock").values().map(Job::view).collect())),
            "wait" => {
                let p: JobP = params(p)?;
                self.wait(&p.id, Duration::from_secs(p.timeout_s.unwrap_or(3600))).await
            }
            _ => Err(coded("unknown_method", format!("no method {method}"))),
        }
    }

    // ---- reads ----------------------------------------------------------------------------

    /// The network, and whether the sequencer's circuit and the lending program match this build.
    async fn info(&self) -> Result<Value> {
        let client = self.inner.client.read().await;
        let lending = self.inner.config.lending;
        let problem = |r: Result<()>| r.err().map(|e| format!("{e:#}"));
        Ok(json!({
            "lending": public(lending),
            "sequencer": client.wallet.client_url().to_string(),
            "last_block": client.last_block().await?,
            "privacy_circuit_problem": problem(client.check_circuit().await),
            "lending_problem": problem(client.check_program(lending, LENDING_ELF).await),
            "build": {
                "lending": hex_id(&image_id(LENDING_ELF)?),
                "oracle": hex_id(&image_id(ORACLE_ELF)?),
                "token": hex_id(&image_id(TOKEN_ELF)?),
            },
            "data_dir": self.inner.config.data_dir.display().to_string(),
        }))
    }

    /// The wallet's accounts, with the native balance of public ones.
    async fn accounts(&self) -> Result<Value> {
        let client = self.inner.client.read().await;
        let storage = client.wallet.storage();
        let ids: Vec<_> = storage
            .key_chain()
            .account_ids()
            .map(|(id, _)| (id, storage.labels_for_account(id).map(ToString::to_string).collect::<Vec<_>>()))
            .collect();
        let mut out = vec![];
        for (id, labels) in ids {
            let native = match id {
                wallet::account::AccountIdWithPrivacy::Public(a) => client.native_balance(a).await.ok(),
                // Private native balances aren't decoded here (lending doesn't use them).
                wallet::account::AccountIdWithPrivacy::Private(_) => None,
            };
            out.push(json!({ "account": id.to_string(), "labels": labels, "native": native.map(|n| n.to_string()) }));
        }
        Ok(Value::Array(out))
    }

    async fn balances(&self, account: Holder, market: MarketId) -> Result<Value> {
        let client = self.inner.client.read().await;
        let m = client.market(self.inner.config.lending, &market).await?;
        let (loan, coll) = (m.params.loan_asset, m.params.collateral_asset);
        Ok(json!({
            "account": account.to_string(),
            "loan_asset": public(loan),
            "loan": client.token_balance(loan, account).await?.to_string(),
            "collateral_asset": public(coll),
            "collateral": client.token_balance(coll, account).await?.to_string(),
        }))
    }

    /// Markets under the lending program: scans blocks since the last call (cached on disk).
    async fn markets(&self) -> Result<Value> {
        let lending = self.inner.config.lending;
        let mut scan = self.inner.scan.lock().await;
        let client = self.inner.client.read().await;
        let mut scanner = Scanner::new(lending, scan.next_block);
        scanner.scan(&client).await?;
        for id in &scanner.markets {
            let id = market_id_hex(id);
            if !scan.markets.contains(&id) {
                scan.markets.push(id);
            }
        }
        scan.next_block = scanner.next_block;
        std::fs::write(scan_path(&self.inner.config), serde_json::to_vec(&*scan)?)?;
        let now = now_ms()?;
        let mut out = vec![];
        for id in &scan.markets {
            let id = parse_market_id(id)?;
            out.push(market_json(&id, &client.market(lending, &id).await?, now)?);
        }
        Ok(Value::Array(out))
    }

    /// A market in full: params, rates, its accounts, the feed, and whether its programs run this
    /// build (findings F1: only then are its assets and oracle what they claim).
    async fn market(&self, id: MarketId) -> Result<Value> {
        let client = self.inner.client.read().await;
        let lending = self.inner.config.lending;
        let m = client.market(lending, &id).await?;
        let now = now_ms()?;
        let mut v = market_json(&id, &m, now)?;
        let accounts = MarketAccounts::new(&lending, &id);
        v["accounts"] = json!({
            "market": public(accounts.market),
            "loan_vault": public(accounts.loan_vault),
            "collateral_vault": public(accounts.collateral_vault),
        });
        v["feed"] = match client.feed(m.params.oracle_feed, m.params.oracle_program).await {
            Ok(f) => json!({
                "price": f.price.to_string(),
                "updated_at": f.updated_at,
                "age_ms": now.saturating_sub(f.updated_at),
                "round": f.round,
            }),
            Err(e) => json!({ "error": format!("{e:#}") }),
        };
        v["code_problem"] = json!(client.check_market_code(lending, &m.params).await.err().map(|e| format!("{e:#}")));
        Ok(v)
    }

    /// `owner`'s positions in a market: salted ones by index (recovery, design §4), then the held
    /// one (E9) if a private owner has one.
    async fn positions(&self, market: MarketId, owner: Holder) -> Result<Value> {
        let client = self.inner.client.read().await;
        let lending = self.inner.config.lending;
        let m = client.market(lending, &market).await?;
        let feed = client.feed(m.params.oracle_feed, m.params.oracle_program).await.ok();
        let now = now_ms()?;
        let mut out = vec![];
        for (index, id, p) in client.owned_positions(lending, &market, owner).await? {
            out.push(position_json(Some(index), id, &p, &m, feed.as_ref(), now)?);
        }
        if let Holder::Private(id) = owner
            && let Some(p) = client.position(lending, id).await?
            && p.market_id == market
        {
            out.push(position_json(None, id, &p, &m, feed.as_ref(), now)?);
        }
        Ok(Value::Array(out))
    }

    /// What `submit` would do now, checked as the program would check it.
    async fn quote(&self, req: OpRequest) -> Result<Value> {
        let client = self.inner.client.read().await;
        let lending = self.inner.config.lending;
        let p = prepare(&client, lending, &req).await?;
        let e = &p.expect;
        let m = client.market(lending, &req.parse()?.market).await?;
        let feed = client.feed(m.params.oracle_feed, m.params.oracle_program).await.ok();
        let mut after = client.position(lending, e.position).await?.unwrap_or_default();
        e.field.set(&mut after, e.to);
        let status = feed.as_ref().and_then(|f| position_status(&m, &after, f, now_ms().ok()?).ok());
        Ok(json!({
            "op": e.op,
            "private": e.is_private(),
            "position": public(e.position),
            "held": e.held(),
            "field": e.field.name(),
            "from": e.from.to_string(),
            "to": e.to.to_string(),
            "amount": e.amount.to_string(),
            "valid_until": e.valid_until,
            "payer": p.payer.map(public),
            "debt_after": status.as_ref().map(|s| s.debt.to_string()),
            "health_factor_after": status.and_then(|s| s.health_factor).map(wad256),
        }))
    }

    // ---- jobs -----------------------------------------------------------------------------

    fn submit(&self, request: OpRequest) -> Result<Value> {
        request.parse()?;
        let now = now_ms()?;
        let id = format!("{now:013}-{:04}", self.inner.next_id.fetch_add(1, Ordering::Relaxed) % 10_000);
        let job = Job {
            id: id.clone(),
            request,
            state: State::Queued,
            created_ms: now,
            since_ms: now,
            expect: None,
            tx: None,
            block: None,
            sent_ms: None,
            proving_ms: None,
            secrets: vec![],
            error: None,
            note: None,
        };
        self.inner.store.save(&job)?;
        let view = job.view();
        self.inner.jobs.lock().expect("jobs lock").insert(id.clone(), job);
        let _ = self.inner.events.send(view.clone());
        self.inner.queue.send(id).map_err(|_| anyhow!("the job worker stopped"))?;
        Ok(view)
    }

    /// Returns the job once it is done, failed, interrupted or unknown.
    async fn wait(&self, id: &str, timeout: Duration) -> Result<Value> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut events = self.inner.events.subscribe();
        loop {
            let job = self.inner.job(id)?;
            if job.state.terminal() {
                return Ok(job.view());
            }
            if tokio::time::timeout_at(deadline, events.recv()).await.is_err() {
                return Err(coded("timeout", format!("job {id} is still {:?}", job.state)));
            }
        }
    }
}

fn scan_path(config: &Config) -> PathBuf {
    config.data_dir.join(format!("markets-{}.json", config.lending))
}

fn market_json(id: &MarketId, m: &Market, now: u64) -> Result<Value> {
    let s = market_status(m, now)?;
    let p = &m.params;
    Ok(json!({
        "market": market_id_hex(id),
        "loan_asset": public(p.loan_asset),
        "collateral_asset": public(p.collateral_asset),
        "oracle_program": public(p.oracle_program),
        "oracle_feed": public(p.oracle_feed),
        "lltv": wad(p.lltv),
        "max_price_age_ms": p.max_price_age,
        "supplied": s.state.total_supply_assets.to_string(),
        "borrowed": s.state.total_borrow_assets.to_string(),
        "utilization": wad(s.utilization),
        "borrow_apr": wad(s.borrow_apr),
        "supply_apr": wad(s.supply_apr),
        "fee": wad(m.state.fee),
    }))
}

/// `index: None` is a held position (E9).
fn position_json(index: Option<u32>, id: AccountId, p: &Position, m: &Market, feed: Option<&Feed>, now: u64) -> Result<Value> {
    let mut v = json!({
        "index": index,
        "held": index.is_none(),
        "position": public(id),
        "supply_shares": p.supply_shares.to_string(),
        "collateral": p.collateral.to_string(),
        "borrow_shares": p.borrow_shares.to_string(),
    });
    if let Some(feed) = feed {
        let s = position_status(m, p, feed, now)?;
        v["supplied_assets"] = json!(s.supplied_assets.to_string());
        v["debt"] = json!(s.debt.to_string());
        v["max_borrow"] = json!(s.max_borrow.to_string());
        v["health_factor"] = json!(s.health_factor.map(wad256));
    }
    Ok(v)
}

/// Builds the op as `bos` does: private claims if any account is private.
async fn prepare(client: &Client, lending: AccountId, r: &OpRequest) -> Result<Prepared> {
    let Parsed { market, from, owner, to, payer, amount } = r.parse()?;
    let private = [from, owner, to].into_iter().flatten().any(|h| matches!(h, Holder::Private(_)));
    let claims = if private { ClaimConfig::private() } else { ClaimConfig::default() };
    let on = On { lending, market, claims, payer };
    let owner_of = |holder: Holder| if r.held { Ok(Owner::held(holder)) } else { client.owner(holder, &market, r.index) };
    let need = |h: Option<Holder>, what: &str| h.ok_or_else(|| coded("bad_request", format!("{:?} needs `{what}`", r.op)));
    match r.op {
        OpKind::Supply => {
            let from = need(from, "from")?;
            ensure!(!r.held || owner.is_none_or(|o| o == from), coded("bad_request", "a held supply goes into `from`'s own account"));
            let owner = owner_of(owner.unwrap_or(from))?;
            client.prepare_supply(&on, from, on.position(&owner), amount.exact("supply")?).await
        }
        OpKind::Withdraw => {
            let owner = owner_of(need(owner, "owner")?)?;
            let shares = match amount {
                Amount::Exact(shares) => shares,
                Amount::All => client.position_shares(lending, on.position(&owner)).await?.supply_shares,
            };
            client.prepare_withdraw(&on, &owner, shares, to).await
        }
        OpKind::SupplyCollateral => {
            let from = need(from, "from")?;
            let owner = owner_of(owner.unwrap_or(from))?;
            client.prepare_supply_collateral(&on, from, on.position(&owner), amount.exact("supply_collateral")?).await
        }
        OpKind::WithdrawCollateral => {
            let owner = owner_of(need(owner, "owner")?)?;
            let assets = match amount {
                Amount::Exact(assets) => assets,
                Amount::All => client.position_shares(lending, on.position(&owner)).await?.collateral,
            };
            client.prepare_withdraw_collateral(&on, &owner, assets, to).await
        }
        OpKind::Borrow => {
            let owner = owner_of(need(owner, "owner")?)?;
            client.prepare_borrow(&on, &owner, amount.exact("borrow")?, to).await
        }
        OpKind::Repay => {
            let from = need(from, "from")?;
            let owner = owner_of(owner.unwrap_or(from))?;
            let position = on.position(&owner);
            let shares = match amount {
                Amount::Exact(shares) => shares,
                Amount::All => client.position_shares(lending, position).await?.borrow_shares,
            };
            client.prepare_repay(&on, from, position, shares).await
        }
    }
}

/// Runs jobs one at a time: unfinished ones from before a restart first, then new ones. One at a
/// time because two private txs on the same private account would spend the same note.
async fn worker(inner: Arc<Inner>, mut queue: mpsc::UnboundedReceiver<String>, resume: Vec<String>) {
    for id in resume {
        inner.run(&id).await;
    }
    while let Some(id) = queue.recv().await {
        inner.run(&id).await;
    }
}

impl Inner {
    fn job(&self, id: &str) -> Result<Job> {
        self.jobs.lock().expect("jobs lock").get(id).cloned().ok_or_else(|| coded("not_found", format!("no job {id}")))
    }

    /// Changes a job, saves it and announces it.
    fn update(&self, id: &str, f: impl FnOnce(&mut Job)) {
        let view = {
            let mut jobs = self.jobs.lock().expect("jobs lock");
            let Some(job) = jobs.get_mut(id) else { return };
            let state = job.state;
            f(job);
            if job.state != state {
                job.since_ms = now_ms().unwrap_or(job.since_ms);
            }
            if let Err(e) = self.store.save(job) {
                eprintln!("[app] saving job {id}: {e:#}");
            }
            job.view()
        };
        let _ = self.events.send(view);
    }

    fn end(&self, id: &str, state: State, error: Option<ApiError>, note: Option<String>) {
        self.update(id, |j| {
            j.state = state;
            j.error = error;
            j.note = note;
        });
    }

    async fn run(&self, id: &str) {
        if let Err(e) = self.step(id).await {
            self.end(id, State::Failed, Some(ApiError::from_anyhow(&e)), None);
        }
    }

    async fn step(&self, id: &str) -> Result<()> {
        let job = self.job(id)?;
        match job.state {
            State::Queued | State::Preparing => self.start(&job).await,
            State::Proving | State::Sending | State::Checking => self.check(&job).await,
            State::Submitted => self.resume_submitted(&job).await,
            _ => Ok(()),
        }
    }

    /// Prepares and sends. A failure here sent nothing (the wallet sends only after proving).
    async fn start(&self, job: &Job) -> Result<()> {
        self.update(&job.id, |j| j.state = State::Preparing);
        let client = self.client.read().await;
        let prepared = prepare(&client, self.config.lending, &job.request).await?;
        let expect = prepared.expect.clone();
        let state = if expect.is_private() { State::Proving } else { State::Sending };
        // Saved before sending, so a restart from here on settles by the position.
        self.update(&job.id, |j| {
            j.expect = Some(Expected::from(&expect));
            j.state = state;
        });
        // Dev only: stretches this step so a test can stop the app in it (dev-mode proofs take
        // ~0.3 s, real ones ~10 min).
        if let Some(ms) = std::env::var("BOS_APP_DELAY_SEND_MS").ok().and_then(|v| v.parse().ok()) {
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }
        let sent = client.send(&prepared).await?;
        drop(client);
        self.update(&job.id, |j| {
            j.state = State::Submitted;
            j.tx = Some(sent.hash.to_string());
            j.secrets = sent.secrets.iter().map(|s| hex(&s.0)).collect();
            j.sent_ms = Some(sent.submitted_ms);
            j.proving_ms = sent.proving_ms;
        });
        self.watch(&job.id, &expect, &sent).await
    }

    async fn resume_submitted(&self, job: &Job) -> Result<()> {
        let expect = job.expect.as_ref().context("a submitted job without its expectation")?.to_expect()?;
        let hash: HashType = job.tx.as_deref().context("a submitted job without its tx")?.parse().map_err(|e| anyhow!("bad tx hash: {e:?}"))?;
        let secrets = job.secrets.iter().map(|s| unhex32(s).map(SharedSecretKey)).collect::<Result<_>>()?;
        let sent = Sent { hash, secrets, submitted_ms: job.sent_ms.unwrap_or(job.since_ms), proving_ms: job.proving_ms, fee_from: None };
        self.watch(&job.id, &expect, &sent).await
    }

    /// Waits for a sent tx: by its hash, and by the position in case the hash got lost (F6).
    async fn watch(&self, id: &str, expect: &Expect, sent: &Sent) -> Result<()> {
        loop {
            let client = self.client.read().await;
            if let Ok(Some((tx, block))) = client.wallet.get_transaction(sent.hash).await {
                drop(client);
                let result = self.client.write().await.finish(expect, sent, &tx, block).await;
                self.update(id, |j| j.block = Some(block));
                match result {
                    Ok(()) => self.end(id, State::Done, None, None),
                    Err(e) => self.end(id, State::Failed, Some(ApiError::from_anyhow(&e)), None),
                }
                return Ok(());
            }
            let value = if expect.held() { None } else { client.field_now(expect).await.ok() };
            drop(client);
            match settle(expect, value, now_ms()?, sent.submitted_ms) {
                Verdict::Wait => tokio::time::sleep(POLL).await,
                Verdict::Applied => {
                    let mut note = "applied, but the sequencer can't find its tx by hash (findings F6)".to_string();
                    if expect.is_private() {
                        note.push_str("; its notes weren't decoded: sync the wallet");
                    }
                    self.end(id, State::Done, None, Some(note));
                    return Ok(());
                }
                Verdict::NotApplied => {
                    let message = "the tx didn't land before its window closed (a claim failed at settlement, or it was dropped)";
                    self.end(id, State::Failed, Some(ApiError { code: "not_included".into(), message: message.into() }), None);
                    return Ok(());
                }
                Verdict::Unknown(note) => {
                    self.end(id, State::Unknown, None, Some(note));
                    return Ok(());
                }
            }
        }
    }

    /// A job the app stopped in while proving or sending: its tx may have gone out without its
    /// hash being saved. Watches the position until the tx can no longer land.
    async fn check(&self, job: &Job) -> Result<()> {
        let expect = job.expect.as_ref().context("an in-flight job without its expectation")?.to_expect()?;
        // From when it may have been sent: the start of proving or sending.
        let since = job.sent_ms.unwrap_or(job.since_ms);
        self.update(&job.id, |j| {
            j.state = State::Checking;
            j.sent_ms = Some(since);
            j.note = Some("the app stopped while this job was in flight: watching its position until the tx can no longer land".into());
        });
        loop {
            let value = {
                let client = self.client.read().await;
                if expect.held() { None } else { client.field_now(&expect).await.ok() }
            };
            match settle(&expect, value, now_ms()?, since) {
                Verdict::Wait => tokio::time::sleep(POLL).await,
                Verdict::Applied => {
                    let mut note = "landed; its hash was lost when the app stopped".to_string();
                    if expect.is_private() {
                        note.push_str("; its notes weren't decoded: sync the wallet");
                    }
                    self.end(&job.id, State::Done, None, Some(note));
                    return Ok(());
                }
                Verdict::NotApplied => {
                    self.end(&job.id, State::Interrupted, None, Some("nothing landed and nothing can now: safe to submit again".into()));
                    return Ok(());
                }
                Verdict::Unknown(note) => {
                    self.end(&job.id, State::Unknown, None, Some(note));
                    return Ok(());
                }
            }
        }
    }
}
