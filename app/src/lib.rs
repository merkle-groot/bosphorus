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
//! `unknown_method`, `not_found`, `timeout`, `network` (the sequencer can't be reached or didn't
//! answer), `rejected` (the sequencer refused a tx: nothing was sent), `insufficient_balance`,
//! `unconfirmed` (a transfer went out but hasn't shown up: don't repeat it before a sync),
//! `no_pool` (no screened pool configured), `not_approved` (the lots exist but the ASP hasn't
//! approved enough of them yet), or `failed`.

pub mod jobs;

use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context as _, Result, anyhow, ensure};
use bosphorus_client::{
    Client, Feed, Holder, LENDING_ELF, ORACLE_ELF, POOL_ELF, Rejected, TOKEN_ELF, hex_id, image_id,
    funds::TransferSent,
    lending::{
        ClaimConfig, Expect, HashType, LeeTransaction, Market, MarketAccounts, MarketId, On, Owner, Position, Prepared, Refusal, Scanner, Sent,
        SharedSecretKey, market_id_hex, market_status, now_ms, parse_market_id, position_status,
    },
    pool::{Label, LotState, hex32, parse_label},
};
use bosphorus_core::{U256, WAD, format_wad, shares};
use lee::{AccountId, native_token::NATIVE_TOKEN_PROGRAM_ID};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::sync::{RwLock, broadcast, mpsc};

use jobs::{Expected, Job, State, Store, Verdict, settle};

/// What `wallet::BEFORE_SEND` calls.
type SendHook = Box<dyn Fn(HashType, &[SharedSecretKey]) + Send + Sync>;

/// How often a job looks for its tx (the localnet makes a block every 15 s, the testnet every 10 s).
const POLL: Duration = Duration::from_secs(3);
/// A transfer not included this many blocks after it was sent was dropped (the sequencer includes
/// a tx in the next block or skips it: a failed settlement, a stale nonce).
const TRANSFER_BLOCKS: u64 = 8;
/// …or, if the chain makes no blocks, after this long: then it's `unconfirmed`, not dropped.
const TRANSFER_WAIT: Duration = Duration::from_secs(10 * 60);
/// How long `info` waits for the sequencer before reporting it unreachable.
const INFO_TIMEOUT: Duration = Duration::from_secs(10);
/// Blocks per wallet lock while syncing, so reads go on between steps.
const SYNC_STEP: u64 = 100;

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
    /// `"assets"`: an exact withdraw/repay `amount` counts loan-token units, converted to shares at
    /// the market's current (projected) ratio, rounded down and capped at the position's shares.
    /// Default `"shares"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
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
            } else if cause.is::<Rejected>() {
                Some("rejected".into())
            } else {
                cause.downcast_ref::<Refusal>().map(|r| match r {
                    Refusal::PayerCannotFund { .. } => "payer_cannot_fund".to_string(),
                })
            }
        });
        let code = code.or_else(|| is_network(&message).then(|| "network".to_string()));
        Self { code: code.unwrap_or_else(|| "failed".into()), message }
    }
}

/// Whether an error is the sequencer being unreachable or silent. The RPC client's errors reach
/// here as text (the wallet formats them), so this reads the text.
fn is_network(message: &str) -> bool {
    ["Request timeout", "RequestTimeout", "client error (SendRequest)", "client error (Connect)", "Connection refused", "Connection reset", "connection closed", "IncompleteMessage", "dns error", "error trying to connect", "Transport("]
        .iter()
        .any(|m| message.contains(m))
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
    /// The wallet's directory; `None` is `LEE_WALLET_HOME_DIR`, like the `wallet` binary.
    pub wallet_home: Option<PathBuf>,
    pub lending: AccountId,
    /// Jobs and the market scan cache.
    pub data_dir: PathBuf,
    /// Where the market scan starts (the lending program's deploy block saves scanning the chain).
    pub scan_from: u64,
    /// The screened pool (E10), if any: `pool_*` methods answer `no_pool` without one.
    pub pool: Option<AccountId>,
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
    /// Held briefly by reads; for writing only while holding `heavy`, so a writer never waits
    /// behind a long read (a proof) and reads never queue behind that writer.
    client: RwLock<Client>,
    /// One slow or wallet-changing thing at a time: a job's prepare-prove-send, a transfer, a sync
    /// step, a new account, recording a landed tx. Proofs run under it (two would fight over the
    /// GPU and could spend the same note).
    heavy: tokio::sync::Mutex<()>,
    /// Subscribed before the worker starts, so the first subscriber sees the resumed jobs' events.
    first_events: Mutex<Option<broadcast::Receiver<Value>>>,
    /// `(synced, target)` while a sync runs.
    syncing: Mutex<Option<(u64, u64)>>,
    /// Set by [`App::stop`]: calls in flight answer `stopping`, no new job starts.
    stopping: tokio::sync::watch::Sender<bool>,
    config: Config,
    store: Store,
    jobs: Mutex<BTreeMap<String, Job>>,
    queue: mpsc::UnboundedSender<String>,
    events: broadcast::Sender<Value>,
    scan: tokio::sync::Mutex<MarketScan>,
    next_id: AtomicU64,
    /// The wallet's and the data dir's, held while the app runs.
    _locks: [fs::File; 2],
}

/// Takes `dir/bosphorus-app.lock` (an OS file lock, released when the app is dropped or its
/// process dies). Two apps on one wallet would overwrite each other's saves (each holds the wallet
/// in memory), and on one data dir both would resume the same jobs, so the second is refused,
/// whether in this process or another. `bos` and LEZ's `wallet` CLI don't take it.
fn lock(dir: &Path) -> Result<fs::File> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join("bosphorus-app.lock");
    let file = fs::OpenOptions::new().write(true).create(true).truncate(false).mode(0o600).open(&path).with_context(|| format!("opening {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(fs::TryLockError::WouldBlock) => Err(coded("wallet_in_use", format!("{} is in use by another Bosphorus app: close that one first", dir.display()))),
        Err(fs::TryLockError::Error(e)) => Err(anyhow::Error::new(e).context(format!("locking {}", path.display()))),
    }
}

impl App {
    /// Opens the wallet (`config.wallet_home`, else `LEE_WALLET_HOME_DIR`), locks it and the data
    /// dir ([`lock`]) and resumes unfinished jobs. Needs a tokio runtime.
    pub async fn open(config: Config) -> Result<Self> {
        let (client, home) = match &config.wallet_home {
            Some(home) => (Client::open(home).with_context(|| format!("opening the wallet in {}", home.display()))?, home.clone()),
            None => (Client::from_env().context("opening the wallet (is LEE_WALLET_HOME_DIR set?)")?, wallet::helperfunctions::get_home()?),
        };
        // Before any job is resumed.
        let locks = [lock(&home)?, lock(&config.data_dir)?];
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
        let (events, first_events) = broadcast::channel(256);
        let inner = Arc::new(Inner {
            client: RwLock::new(client),
            heavy: tokio::sync::Mutex::new(()),
            first_events: Mutex::new(Some(first_events)),
            syncing: Mutex::new(None),
            stopping: tokio::sync::watch::Sender::new(false),
            config,
            store,
            jobs: Mutex::new(jobs),
            queue,
            events,
            scan: tokio::sync::Mutex::new(scan),
            next_id: AtomicU64::new(0),
            _locks: locks,
        });
        tokio::spawn(worker(inner.clone(), rx, resume));
        bosphorus_client::proving::warm_up();
        Ok(Self { inner })
    }

    /// Ends every call in flight with `stopping` (a transfer being proven sends nothing) and
    /// starts no more jobs, so the app can be closed without waiting for a proof. A job stopped
    /// mid-proof is `interrupted` at the next open. Doesn't block.
    pub fn stop(&self) {
        self.inner.stopping.send_replace(true);
    }

    /// Every job change, as the job's view.
    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.inner.first_events.lock().expect("events lock").take().unwrap_or_else(|| self.inner.events.subscribe())
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
        let mut stopping = self.inner.stopping.subscribe();
        tokio::select! {
            r = self.dispatch(method, params) => r.map_err(|e| ApiError::from_anyhow(&e)),
            _ = stopping.wait_for(|s| *s) => Err(ApiError { code: "stopping".into(), message: "Bosphorus is closing".into() }),
        }
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
        let amount = |s: &str| -> Result<u128> {
            let amount = s.parse::<u128>().map_err(|e| coded("bad_request", format!("amount: {e}")))?;
            ensure!(amount > 0, coded("bad_request", "amount: nothing to move"));
            Ok(amount)
        };
        // A token program (`Public/<id>`); native LEZ if absent.
        let token = |s: Option<&str>| match s.map(holder).transpose()? {
            None => Ok(NATIVE_TOKEN_PROGRAM_ID),
            Some(Holder::Public(token)) => Ok(token),
            Some(Holder::Private(_)) => Err(coded("bad_request", "token: a token is a Public/<id> program")),
        };
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
            "new_account" => {
                #[derive(Deserialize)]
                struct P {
                    #[serde(default)]
                    private: bool,
                }
                let p: P = params(p)?;
                let _heavy = self.inner.heavy.lock().await;
                let holder = self.inner.client.write().await.new_account(p.private)?;
                Ok(json!({ "account": holder.to_string() }))
            }
            "transfer" => {
                #[derive(Deserialize)]
                struct P {
                    from: String,
                    to: String,
                    amount: String,
                    // A token program (`Public/<id>`); native LEZ if absent.
                    #[serde(default)]
                    token: Option<String>,
                }
                let p: P = params(p)?;
                let (amount, token) = (amount(&p.amount)?, token(p.token.as_deref())?);
                let token = (token != NATIVE_TOKEN_PROGRAM_ID).then_some(token);
                self.move_funds(Move::Transfer { token, from: holder(&p.from)?, to: holder(&p.to)?, amount }).await
            }
            "pool" => self.pool().await,
            "pool_deposit" => {
                #[derive(Deserialize)]
                struct P {
                    from: String,
                    to: String,
                    amount: String,
                    #[serde(default)]
                    token: Option<String>,
                }
                let p: P = params(p)?;
                let (pool, asset, amount) = (self.pool_id()?, token(p.token.as_deref())?, amount(&p.amount)?);
                self.move_funds(Move::PoolDeposit { pool, asset, from: holder(&p.from)?, to: holder(&p.to)?, amount }).await
            }
            "pool_withdraw" => {
                #[derive(Deserialize)]
                struct P {
                    from: String,
                    to: String,
                    amount: String,
                    #[serde(default)]
                    token: Option<String>,
                }
                let p: P = params(p)?;
                let (pool, asset, amount) = (self.pool_id()?, token(p.token.as_deref())?, amount(&p.amount)?);
                let Holder::Private(from) = holder(&p.from)? else { return Err(coded("bad_request", "from: lots sit on a private account")) };
                self.move_funds(Move::PoolWithdraw { pool, asset, holder: from, to: holder(&p.to)?, amount }).await
            }
            "pool_ragequit" => {
                #[derive(Deserialize)]
                struct P {
                    holder: String,
                    label: String,
                }
                let p: P = params(p)?;
                let pool = self.pool_id()?;
                let Holder::Private(id) = holder(&p.holder)? else { return Err(coded("bad_request", "holder: lots sit on a private account")) };
                let label = parse_label(&p.label).map_err(|e| coded("bad_request", format!("label: {e:#}")))?;
                self.move_funds(Move::PoolRagequit { pool, holder: id, label }).await
            }
            "sync" => self.sync().await,
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
    /// Answers even when the sequencer can't be reached: then `last_block` is null and
    /// `network_problem` says why (the program checks are null, not known to be wrong).
    async fn info(&self) -> Result<Value> {
        let client = self.inner.client.read().await;
        let lending = self.inner.config.lending;
        let problem = |r: Result<()>| r.err().map(|e| format!("{e:#}"));
        let (last_block, network_problem) = match tokio::time::timeout(INFO_TIMEOUT, client.last_block()).await {
            Ok(Ok(b)) => (Some(b), None),
            Ok(Err(e)) => (None, Some(format!("{e:#}"))),
            Err(_) => (None, Some(format!("the sequencer didn't answer within {} s", INFO_TIMEOUT.as_secs()))),
        };
        let reachable = last_block.is_some();
        Ok(json!({
            "lending": public(lending),
            "sequencer": client.wallet.client_url().to_string(),
            "last_block": last_block,
            "network_problem": network_problem,
            "synced_block": client.synced_block(),
            "syncing": self.inner.syncing.lock().expect("sync lock").map(|(synced, target)| json!({ "synced_block": synced, "target_block": target })),
            "privacy_circuit_problem": if reachable { problem(client.check_circuit().await) } else { None },
            "lending_problem": if reachable { problem(client.check_program(lending, LENDING_ELF).await) } else { None },
            "pool": self.inner.config.pool.map(public),
            "pool_problem": match self.inner.config.pool {
                Some(pool) if reachable => problem(client.check_program(pool, POOL_ELF).await),
                _ => None,
            },
            "build": {
                "lending": hex_id(&image_id(LENDING_ELF)?),
                "pool": hex_id(&image_id(POOL_ELF)?),
                "oracle": hex_id(&image_id(ORACLE_ELF)?),
                "token": hex_id(&image_id(TOKEN_ELF)?),
            },
            "data_dir": self.inner.config.data_dir.display().to_string(),
        }))
    }

    /// The wallet's accounts, with their native balances.
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
            // Private balances are as of the last sync (`synced_block`).
            let native = match id {
                wallet::account::AccountIdWithPrivacy::Public(a) => client.native(Holder::Public(a)).await.ok(),
                wallet::account::AccountIdWithPrivacy::Private(a) => client.native(Holder::Private(a)).await.ok(),
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

    /// Markets under the lending program: scans blocks since the last call (cached on disk). If the
    /// sequencer can't be reached, the last answer, each market marked `"cached": true` (names
    /// and decimals still right; rates and totals as they were).
    async fn markets(&self) -> Result<Value> {
        let path = self.inner.config.data_dir.join("markets-view.json");
        match self.markets_now().await {
            Ok(v) => {
                let _ = std::fs::write(&path, serde_json::to_vec(&v)?);
                Ok(v)
            }
            Err(e) if ApiError::from_anyhow(&e).code == "network" => {
                let Some(Value::Array(mut list)) = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()) else { return Err(e) };
                for m in &mut list {
                    m["cached"] = json!(true);
                }
                Ok(Value::Array(list))
            }
            Err(e) => Err(e),
        }
    }

    async fn markets_now(&self) -> Result<Value> {
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
            let m = client.market(lending, &id).await?;
            let mut v = market_json(&id, &m, now)?;
            add_decimals(&client, &m, &mut v).await;
            out.push(v);
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
        add_decimals(&client, &m, &mut v).await;
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
            "position": if e.held() { Holder::Private(e.position).to_string() } else { public(e.position) },
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

    // ---- wallet ---------------------------------------------------------------------------

    /// Moves funds and waits for them to land (`{"tx", "block", "decoded"}`, and `"label"` for a
    /// deposit): a transfer, or a pool deposit, withdrawal or ragequit. The tx hash is known before
    /// it goes out, so a lost reply doesn't lose track of it: the error is then `unconfirmed` (with
    /// the hash), never one that invites sending again while it may still land.
    async fn move_funds(&self, m: Move) -> Result<Value> {
        let _heavy = self.inner.heavy.lock().await;
        let marked = Arc::new(Mutex::new(None::<HashType>));
        let sent = {
            let client = self.inner.client.read().await;
            m.check(&client).await?;
            let hook = {
                let marked = marked.clone();
                Box::new(move |hash: HashType, _: &[SharedSecretKey]| *marked.lock().expect("hash lock") = Some(hash)) as SendHook
            };
            wallet::BEFORE_SEND.scope(hook, m.send(&client)).await
        };
        let hash = *marked.lock().expect("hash lock");
        let (sent, label, hash) = match (sent, hash) {
            (Ok((sent, label)), _) => {
                let hash = sent.hash;
                (Some(sent), label, hash)
            }
            // Failed before it went out, or refused: nothing can land.
            (Err(e), None) => return Err(e),
            (Err(e), Some(_)) if e.is::<Rejected>() => return Err(e),
            (Err(e), Some(hash)) => {
                eprintln!("[app] {} {hash}: sent, but {e:#}; watching for it", m.name());
                (None, None, hash)
            }
        };
        let since = self.inner.client.read().await.last_block().await.ok();
        match self.inner.await_tx(hash, since).await? {
            Landing::Landed(tx, block) => {
                let synced = match &sent {
                    Some(sent) => {
                        self.inner.client.write().await.finish_transfer(sent, &tx, block).await?;
                        true
                    }
                    None => false,
                };
                // Without its secrets (the reply was lost) a private tx's notes show up at the next sync.
                let mut v = json!({ "tx": hash.to_string(), "block": block, "decoded": synced || matches!(*tx, LeeTransaction::Public(_)) });
                if let Some(label) = label {
                    v["label"] = json!(hex32(&label));
                }
                Ok(v)
            }
            Landing::Dropped => Err(coded("not_included", format!("tx {hash} wasn't included in the {TRANSFER_BLOCKS} blocks after it went out: the network most likely dropped it, so nothing moved; sync to be sure before trying again"))),
            Landing::Stalled => Err(coded("unconfirmed", format!("tx {hash} went out, but the network has made almost no blocks since: it may still land; sync later before trying again"))),
        }
    }

    // ---- the screened pool (E10) ----------------------------------------------------------

    fn pool_id(&self) -> Result<AccountId> {
        self.inner.config.pool.ok_or_else(|| coded("no_pool", "no screened pool is configured"))
    }

    /// The pool and this wallet's lots: `{"pool", "asp", "approved", "lots": [{"account", "label",
    /// "token", "amount", "state", "depositor", "can_take_back"}]}`. `token` is null for native
    /// LEZ; `state` is `approved` (withdrawable) or `pending` (only a ragequit takes it out);
    /// `can_take_back`: the depositor is this wallet's. Lots are as of the last sync.
    async fn pool(&self) -> Result<Value> {
        let pool = self.pool_id()?;
        let client = self.inner.client.read().await;
        let config = client.pool_config(pool).await?;
        let approved = client.pool_approved(pool).await?;
        let privates: Vec<AccountId> = client
            .wallet
            .storage()
            .key_chain()
            .account_ids()
            .filter_map(|(id, _)| match id {
                wallet::account::AccountIdWithPrivacy::Private(a) => Some(a),
                wallet::account::AccountIdWithPrivacy::Public(_) => None,
            })
            .collect();
        let mut lots = vec![];
        for holder in privates {
            for (lot, state) in client.pool_lot_states(pool, holder).await? {
                let depositor = client.pool_record(pool, &lot.label).await?.map(|r| r.depositor);
                let ours = depositor.is_some_and(|d| client.wallet.get_account_public_signing_key(d).is_some() || client.wallet.resolve_private_account(d).is_some());
                let asset = AccountId::new(lot.asset);
                lots.push(json!({
                    "account": Holder::Private(holder).to_string(),
                    "label": hex32(&lot.label),
                    "token": (asset != NATIVE_TOKEN_PROGRAM_ID).then(|| public(asset)),
                    "amount": lot.amount.to_string(),
                    "state": state.name(),
                    "depositor": depositor.map(public),
                    "can_take_back": ours,
                }));
            }
        }
        Ok(json!({ "pool": public(pool), "asp": public(config.asp), "approved": approved.labels().count(), "lots": lots }))
    }

    /// Catches the wallet's private state up with the chain, a step at a time (reads go on
    /// between steps; `info.syncing` shows the progress). Then settles jobs left `unknown` on a
    /// held position, which only a synced wallet can read.
    async fn sync(&self) -> Result<Value> {
        let synced = self.inner.sync_wallet().await?;
        self.inner.resettle_unknown().await;
        Ok(json!({ "synced_block": synced }))
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
            send_marked: false,
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

/// What [`App::move_funds`] sends. None touches a position, so each is sent and watched like a
/// transfer, not as a job.
enum Move {
    Transfer { token: Option<AccountId>, from: Holder, to: Holder, amount: u128 },
    PoolDeposit { pool: AccountId, asset: AccountId, from: Holder, to: Holder, amount: u128 },
    PoolWithdraw { pool: AccountId, asset: AccountId, holder: AccountId, to: Holder, amount: u128 },
    PoolRagequit { pool: AccountId, holder: AccountId, label: Label },
}

impl Move {
    fn name(&self) -> &'static str {
        match self {
            Self::Transfer { .. } => "transfer",
            Self::PoolDeposit { .. } => "pool deposit",
            Self::PoolWithdraw { .. } => "pool withdrawal",
            Self::PoolRagequit { .. } => "ragequit",
        }
    }

    /// Refusals with a code, before anything is proven.
    async fn check(&self, client: &Client) -> Result<()> {
        let asof = |h: Holder| if matches!(h, Holder::Private(_)) { " (as of the last sync)" } else { "" };
        match *self {
            Self::Transfer { token, from, amount, .. } => {
                let have = match token {
                    None => client.native(from).await?,
                    Some(token) => client.token_balance(token, from).await?,
                };
                ensure!(have >= amount, coded("insufficient_balance", format!("{from} holds {have}{}, less than {amount}", asof(from))));
            }
            Self::PoolDeposit { asset, from, amount, .. } => {
                ensure!(matches!(from, Holder::Public(_)), coded("bad_request", "from: a screened deposit comes from a public account (a private source can't be screened)"));
                let have = if asset == NATIVE_TOKEN_PROGRAM_ID { client.native(from).await? } else { client.token_balance(asset, from).await? };
                ensure!(have >= amount, coded("insufficient_balance", format!("{from} holds {have}, less than {amount}")));
            }
            Self::PoolWithdraw { pool, asset, holder, amount, .. } => {
                let (mut approved, mut pending) = (0u128, 0u128);
                for (lot, state) in client.pool_lot_states(pool, holder).await? {
                    if lot.asset != *asset.value() {
                        continue;
                    }
                    match state {
                        LotState::Approved => approved = approved.saturating_add(lot.amount),
                        LotState::Pending => pending = pending.saturating_add(lot.amount),
                    }
                }
                if approved < amount {
                    let code = if approved.saturating_add(pending) >= amount { "not_approved" } else { "insufficient_balance" };
                    return Err(coded(code, format!("Private/{holder} holds {approved} approved and {pending} pending (as of the last sync), less than {amount} approved")));
                }
            }
            Self::PoolRagequit { .. } => {}
        }
        Ok(())
    }

    async fn send(&self, client: &Client) -> Result<(TransferSent, Option<Label>)> {
        match *self {
            Self::Transfer { token, from, to, amount } => Ok((client.send_transfer(token, from, to, amount).await?, None)),
            Self::PoolDeposit { pool, asset, from, to, amount } => {
                let (sent, label) = client.send_pool_deposit(pool, asset, from, to, amount).await?;
                Ok((sent, Some(label)))
            }
            Self::PoolWithdraw { pool, asset, holder, to, amount } => Ok((client.send_pool_withdraw(pool, asset, holder, amount, to).await?, None)),
            Self::PoolRagequit { pool, holder, label } => Ok((client.send_pool_ragequit(pool, holder, label).await?, None)),
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

/// `loan_decimals` / `collateral_decimals` from the token programs (null if unreadable).
async fn add_decimals(client: &Client, m: &Market, v: &mut Value) {
    for (key, token) in [("loan_decimals", m.params.loan_asset), ("collateral_decimals", m.params.collateral_asset)] {
        v[key] = json!(client.token_config(token).await.ok().map(|c| c.decimals));
    }
}

/// `index: None` is a held position (E9).
fn position_json(index: Option<u32>, id: AccountId, p: &Position, m: &Market, feed: Option<&Feed>, now: u64) -> Result<Value> {
    let mut v = json!({
        "index": index,
        "held": index.is_none(),
        // A held position is the owner's private account itself.
        "position": if index.is_none() { Holder::Private(id).to_string() } else { public(id) },
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
    let in_assets = match r.unit.as_deref() {
        None | Some("shares") => false,
        Some("assets") => true,
        Some(u) => return Err(coded("bad_request", format!("unit: \"assets\" or \"shares\", got {u:?}"))),
    };
    // Loan-token units → shares at the projected ratio: (total assets, total shares) of supply or borrow.
    let to_shares = |assets: u128, totals: (u128, u128), owned: u128| -> Result<u128> {
        Ok(shares::exact_to_shares_down(assets, totals.0, totals.1).map_err(|e| anyhow!("{e}"))?.min(owned))
    };
    match r.op {
        OpKind::Supply => {
            let from = need(from, "from")?;
            ensure!(!r.held || owner.is_none_or(|o| o == from), coded("bad_request", "a held supply goes into `from`'s own account"));
            let owner = owner_of(owner.unwrap_or(from))?;
            let assets = amount.exact("supply")?;
            has_funds(client, from, client.market(lending, &market).await?.params.loan_asset, assets).await?;
            client.prepare_supply(&on, from, on.position(&owner), assets).await
        }
        OpKind::Withdraw => {
            let owner = owner_of(need(owner, "owner")?)?;
            let owned = client.position_shares(lending, on.position(&owner)).await?.supply_shares;
            let shares = match amount {
                Amount::Exact(assets) if in_assets => {
                    let s = market_status(&client.market(lending, &market).await?, now_ms()?)?.state;
                    to_shares(assets, (s.total_supply_assets, s.total_supply_shares), owned)?
                }
                Amount::Exact(shares) => shares,
                Amount::All => owned,
            };
            client.prepare_withdraw(&on, &owner, shares, to).await
        }
        OpKind::SupplyCollateral => {
            let from = need(from, "from")?;
            let owner = owner_of(owner.unwrap_or(from))?;
            let assets = amount.exact("supply_collateral")?;
            has_funds(client, from, client.market(lending, &market).await?.params.collateral_asset, assets).await?;
            client.prepare_supply_collateral(&on, from, on.position(&owner), assets).await
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
            let owned = client.position_shares(lending, position).await?.borrow_shares;
            let shares = match amount {
                Amount::Exact(assets) if in_assets => {
                    let s = market_status(&client.market(lending, &market).await?, now_ms()?)?.state;
                    to_shares(assets, (s.total_borrow_assets, s.total_borrow_shares), owned)?
                }
                Amount::Exact(shares) => shares,
                Amount::All => owned,
            };
            let p = client.prepare_repay(&on, from, position, shares).await?;
            has_funds(client, from, p.params.loan_asset, p.expect.amount).await?;
            Ok(p)
        }
    }
}

/// An op's tokens come from `from`: refused here, rather than sent and dropped by the network (a
/// private tx) or landed as a failure that still pays its fee (a public one).
async fn has_funds(client: &Client, from: Holder, token: AccountId, need: u128) -> Result<()> {
    let have = client.token_balance(token, from).await?;
    if have < need {
        let asof = if matches!(from, Holder::Private(_)) { " as of the last sync" } else { "" };
        return Err(coded("insufficient_balance", format!("{from} holds {have} of {}{asof}; this needs {need}", public(token))));
    }
    Ok(())
}

/// Runs jobs one at a time: unfinished ones from before a restart first, then new ones. One at a
/// time because two private txs on the same private account would spend the same note.
async fn worker(inner: Arc<Inner>, mut queue: mpsc::UnboundedReceiver<String>, resume: Vec<String>) {
    let mut stopping = inner.stopping.subscribe();
    let work = async {
        for id in resume {
            inner.run(&id).await;
        }
        while let Some(id) = queue.recv().await {
            inner.run(&id).await;
        }
    };
    // A job dropped mid-proof sent nothing (its hash is saved before sending): the next open
    // finds it `proving` and marks it interrupted.
    tokio::select! {
        () = work => {}
        _ = stopping.wait_for(|s| *s) => {}
    }
}

/// How a sent transfer ended up ([`Inner::await_tx`]).
enum Landing {
    Landed(Box<LeeTransaction>, u64),
    /// Not in the [`TRANSFER_BLOCKS`] blocks after it was sent.
    Dropped,
    /// The chain made too few blocks in [`TRANSFER_WAIT`] to tell.
    Stalled,
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

    async fn run(self: &Arc<Self>, id: &str) {
        if let Err(e) = self.step(id).await {
            self.end(id, State::Failed, Some(ApiError::from_anyhow(&e)), None);
        }
    }

    async fn step(self: &Arc<Self>, id: &str) -> Result<()> {
        let job = self.job(id)?;
        match job.state {
            State::Queued | State::Preparing => self.start(&job).await,
            // Stopped before its tx was handed over (the hash is saved first): nothing went out.
            State::Proving | State::Sending if job.send_marked && job.tx.is_none() => {
                let note = if job.state == State::Proving { "Bosphorus stopped during the proof, before anything was sent" } else { "Bosphorus stopped before this was sent" };
                self.end(id, State::Interrupted, None, Some(note.into()));
                Ok(())
            }
            // From a build that didn't save the hash first: it may have gone out.
            State::Proving | State::Sending | State::Checking => self.check(&job).await,
            State::Submitted => self.resume_submitted(&job).await,
            _ => Ok(()),
        }
    }

    /// Prepares, proves (private) and sends. The tx's hash and secrets are saved just before it is
    /// handed to the sequencer (`wallet::BEFORE_SEND`), so a failure before that sent nothing, and
    /// one after it is settled from the chain, never assumed lost.
    async fn start(self: &Arc<Self>, job: &Job) -> Result<()> {
        let heavy = self.heavy.lock().await;
        self.update(&job.id, |j| j.state = State::Preparing);
        let client = self.client.read().await;
        let prepared = prepare(&client, self.config.lending, &job.request).await?;
        let expect = prepared.expect.clone();
        let state = if expect.is_private() { State::Proving } else { State::Sending };
        self.update(&job.id, |j| {
            j.expect = Some(Expected::from(&expect));
            j.state = state;
            j.send_marked = true;
        });
        // Dev only: stretches this step so a test can stop the app in it (dev-mode proofs take
        // ~0.3 s, real ones ~1 min on a GPU).
        if let Some(ms) = std::env::var("BOS_APP_DELAY_SEND_MS").ok().and_then(|v| v.parse().ok()) {
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }
        let started = now_ms()?;
        let hook: SendHook = {
            let (inner, id) = (self.clone(), job.id.clone());
            Box::new(move |hash, secrets| {
                let now = now_ms().unwrap_or(started);
                inner.update(&id, |j| {
                    j.state = State::Submitted;
                    j.tx = Some(hash.to_string());
                    j.secrets = secrets.iter().map(|s| hex(&s.0)).collect();
                    j.sent_ms = Some(now);
                    j.proving_ms = (state == State::Proving).then(|| now.saturating_sub(started));
                });
            })
        };
        let result = wallet::BEFORE_SEND.scope(hook, client.send(&prepared)).await;
        drop(client);
        drop(heavy);
        match result {
            Ok(sent) => self.watch(&job.id, &expect, &sent).await,
            Err(e) => {
                let job = self.job(&job.id)?;
                // Not handed over, or refused: nothing can land.
                if job.tx.is_none() || e.is::<Rejected>() {
                    return Err(e);
                }
                // Handed over without a clean answer (a timeout, a dropped connection): it may land.
                let why = ApiError::from_anyhow(&e).message;
                eprintln!("[app] job {}: sent, but {why}; watching for it", job.id);
                self.update(&job.id, |j| j.note = Some(format!("sent, but the network's answer was lost ({why}): watching for it")));
                self.resume_submitted(&self.job(&job.id)?).await
            }
        }
    }

    async fn resume_submitted(&self, job: &Job) -> Result<()> {
        let expect = job.expect.as_ref().context("a submitted job without its expectation")?.to_expect()?;
        let hash: HashType = job.tx.as_deref().context("a submitted job without its tx")?.parse().map_err(|e| anyhow!("bad tx hash: {e:?}"))?;
        let secrets = job.secrets.iter().map(|s| unhex32(s).map(SharedSecretKey)).collect::<Result<_>>()?;
        let sent = Sent { hash, secrets, submitted_ms: job.sent_ms.unwrap_or(job.since_ms), proving_ms: job.proving_ms, fee_from: None };
        self.watch(&job.id, &expect, &sent).await
    }

    /// Waits for a sent tx: by its hash, and by the position in case the hash got lost (F6). Only
    /// the op's window closing makes it safe to call it not landed; a tx not seen for
    /// [`TRANSFER_BLOCKS`] blocks was most likely dropped, which the job's note says meanwhile.
    async fn watch(&self, id: &str, expect: &Expect, sent: &Sent) -> Result<()> {
        let mut since = None;
        let mut noted = false;
        loop {
            let (found, tip) = {
                let client = self.client.read().await;
                (client.wallet.get_transaction(sent.hash).await, client.last_block().await.ok())
            };
            if !noted
                && let Some(tip) = tip
                && tip >= *since.get_or_insert(tip) + TRANSFER_BLOCKS
            {
                noted = true;
                let until = match expect.valid_until {
                    Some(_) => "its offer ends".to_string(),
                    None => format!("{} minutes after it was sent", jobs::NO_WINDOW_WAIT_MS / 60_000),
                };
                self.update(id, |j| j.note = Some(format!("not on the chain {TRANSFER_BLOCKS} blocks after sending, so most likely dropped; it can't land once {until}, and Bosphorus watches until then")));
            }
            if let Ok(Some((tx, block))) = found {
                let result = {
                    let _heavy = self.heavy.lock().await;
                    self.client.write().await.finish(expect, sent, &tx, block).await
                };
                self.update(id, |j| j.block = Some(block));
                match result {
                    Ok(()) => self.end(id, State::Done, None, None),
                    Err(e) => self.end(id, State::Failed, Some(ApiError::from_anyhow(&e)), None),
                }
                return Ok(());
            }
            let value = if expect.held() { None } else { self.client.read().await.field_now(expect).await.ok() };
            let mut verdict = settle(expect, value, now_ms()?, sent.submitted_ms);
            // A held position is read from the wallet's own copy: sync it, then look again.
            if expect.held() && matches!(verdict, Verdict::Unknown(_)) && self.sync_wallet().await.is_ok() {
                let value = self.client.read().await.field_now(expect).await.ok();
                verdict = settle(expect, value, now_ms()?, sent.submitted_ms);
            }
            match verdict {
                Verdict::Wait => tokio::time::sleep(POLL).await,
                Verdict::Applied => {
                    let mut note = "applied, but the sequencer can't find its tx by hash (findings F6)".to_string();
                    if expect.is_private() && !expect.held() {
                        note.push_str("; its notes weren't decoded: sync the wallet");
                    }
                    self.end(id, State::Done, None, Some(note));
                    return Ok(());
                }
                Verdict::NotApplied => {
                    let message = "it didn't land before its window closed (a claim failed at settlement, or the network dropped it): nothing moved";
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

    /// A job the app stopped in while proving or sending, from a build that didn't save the hash
    /// before sending: its tx may have gone out. Watches the position until the tx can't land.
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
            let value = if expect.held() { None } else { self.client.read().await.field_now(&expect).await.ok() };
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

    /// Syncs the wallet a step at a time under `heavy` (see [`App::sync`]).
    async fn sync_wallet(&self) -> Result<u64> {
        let _heavy = self.heavy.lock().await;
        let result = async {
            loop {
                let (synced, tip) = self.client.write().await.sync_step(SYNC_STEP).await?;
                *self.syncing.lock().expect("sync lock") = Some((synced, tip));
                if synced >= tip {
                    return anyhow::Ok(synced);
                }
            }
        }
        .await;
        *self.syncing.lock().expect("sync lock") = None;
        result
    }

    /// Jobs left `unknown` on a held position, settled again from the just-synced wallet.
    async fn resettle_unknown(&self) {
        let unknown: Vec<Job> = self.jobs.lock().expect("jobs lock").values().filter(|j| j.state == State::Unknown).cloned().collect();
        for job in unknown {
            let Some(expect) = job.expect.as_ref().and_then(|e| e.to_expect().ok()) else { continue };
            if !expect.held() {
                continue;
            }
            let Ok(value) = self.client.read().await.field_now(&expect).await else { continue };
            let Ok(now) = now_ms() else { return };
            match settle(&expect, Some(value), now, job.sent_ms.unwrap_or(job.since_ms)) {
                Verdict::Applied => self.end(&job.id, State::Done, None, Some("found in the wallet after a sync".into())),
                Verdict::NotApplied => {
                    let message = "the synced wallet shows it didn't land: nothing moved";
                    self.end(&job.id, State::Failed, Some(ApiError { code: "not_included".into(), message: message.into() }), None);
                }
                Verdict::Wait | Verdict::Unknown(_) => {}
            }
        }
    }

    /// Waits for a sent transfer by its hash, counting blocks from `since` (the tip when it was
    /// sent), not time, so a stalled chain isn't mistaken for a dropped tx.
    async fn await_tx(&self, hash: HashType, mut since: Option<u64>) -> Result<Landing> {
        let deadline = tokio::time::Instant::now() + TRANSFER_WAIT;
        loop {
            let (found, tip) = {
                let client = self.client.read().await;
                (client.wallet.get_transaction(hash).await, client.last_block().await)
            };
            if let Ok(Some((tx, block))) = found {
                return Ok(Landing::Landed(Box::new(tx), block));
            }
            if let Ok(tip) = tip
                && tip >= *since.get_or_insert(tip) + TRANSFER_BLOCKS
            {
                return Ok(Landing::Dropped);
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(Landing::Stalled);
            }
            tokio::time::sleep(POLL).await;
        }
    }
}
