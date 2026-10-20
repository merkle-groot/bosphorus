//! `bos-keeper`: watches every market under a lending program and liquidates unhealthy positions.
//!
//! Each round: scan new blocks for lending accounts (positions created by public or private txs),
//! then for each known position read market, feed and position, and liquidate it in full if it's
//! unhealthy at the claims the tx will make. Stateless apart from the scanner's known-position set.
//! Uses the LEZ wallet at `LEE_WALLET_HOME_DIR`, like `bos`.
//!
//! Markets are permissionless, so the keeper only acts on markets it can trust (findings F1): the
//! lending program, both assets and the oracle run this build's bytecode, and the feed's publisher
//! is one of `--publisher`. Otherwise a market creator could feed it a price that makes it buy junk
//! collateral, or a "token" that uses its signature to move its other balances. A market it can't
//! read or doesn't trust is skipped, never fatal to the round (findings F2).

use std::{
    collections::{BTreeMap, HashMap},
    time::Duration,
};

use anyhow::{Context as _, Result, ensure};
use bosphorus_client::{
    Client, Feed, Holder,
    lending::{ClaimConfig, Liquidation, Market, MarketId, On, Scanner, market_id_hex, now_ms, quote_liquidation},
};
use bosphorus_core::{PRICE_SCALE, U256, parse_wad};
use clap::Parser;
use lee::AccountId;

#[derive(Parser)]
#[command(name = "bos-keeper", about = "Bosphorus liquidation keeper")]
struct Args {
    #[arg(long)]
    lending: Holder,
    /// Public account that pays the debt and receives the collateral (needs the loan tokens).
    #[arg(long)]
    liquidator: Holder,
    /// Fee payer (default: the liquidator).
    #[arg(long)]
    payer: Option<Holder>,
    /// Feed publishers whose prices the keeper acts on (repeatable, at least one).
    #[arg(long, required = true)]
    publisher: Vec<Holder>,
    /// Seconds between rounds.
    #[arg(long, default_value_t = 5)]
    interval: u64,
    /// First block to scan.
    #[arg(long, default_value_t = 1)]
    from_block: u64,
    /// Run one round and exit.
    #[arg(long)]
    once: bool,
    /// Validity window of liquidation txs, seconds. A public tx included after it still pays, and
    /// the testnet stalls for minutes (findings F3).
    #[arg(long, default_value_t = 600)]
    window: u64,
    #[arg(long, default_value = "0.000000001", value_parser = |s: &str| parse_wad(s).context("bad fraction"))]
    ratio_width: u128,
}

fn log(msg: impl std::fmt::Display) {
    eprintln!("[keeper {}] {msg}", now_ms().unwrap_or_default() / 1000);
}

struct Keeper {
    client: Client,
    scanner: Scanner,
    liquidator: AccountId,
    payer: AccountId,
    publishers: Vec<AccountId>,
    claims: ClaimConfig,
    round: u64,
    /// Position → (failures, next round to try).
    failed: HashMap<AccountId, (u32, u64)>,
    /// Market → (trusted?, round to check again if not).
    trust: HashMap<MarketId, (bool, u64)>,
}

/// Untrusted or unreadable markets are looked at again after this many rounds (~5 min at 5 s).
const RECHECK_ROUNDS: u64 = 60;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let client = Client::from_env().context("opening the wallet (is LEE_WALLET_HOME_DIR set?)")?;
    let lending = args.lending.id();
    let liquidator = args.liquidator.id();
    client.check_program(lending, bosphorus_client::LENDING_ELF).await?;
    let mut keeper = Keeper {
        client,
        scanner: Scanner::new(lending, args.from_block),
        liquidator,
        // The keeper pays its own fees: two senders on one payer race on its nonce.
        payer: args.payer.map_or(liquidator, Holder::id),
        publishers: args.publisher.iter().map(|p| p.id()).collect(),
        claims: ClaimConfig { window_ms: args.window * 1000, ratio_width: args.ratio_width, ..ClaimConfig::default() },
        round: 0,
        failed: HashMap::new(),
        trust: HashMap::new(),
    };
    log(format!("watching lending {lending} from block {}, liquidator {liquidator}", args.from_block));
    loop {
        keeper.round += 1;
        if let Err(e) = keeper.round().await {
            log(format!("round failed: {e:#}"));
        }
        if args.once {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(args.interval)).await;
    }
}

impl Keeper {
    async fn round(&mut self) -> Result<()> {
        for position in self.scanner.scan(&self.client).await? {
            log(format!("found position {position}"));
        }
        let mut by_market: BTreeMap<MarketId, Vec<AccountId>> = BTreeMap::new();
        for (position, market) in &self.scanner.positions {
            by_market.entry(*market).or_default().push(*position);
        }
        for (market_id, positions) in by_market {
            if let Err(e) = self.market(market_id, positions).await {
                log(format!("market {}: {e:#}", market_id_hex(&market_id)));
            }
        }
        Ok(())
    }

    async fn market(&mut self, market_id: MarketId, positions: Vec<AccountId>) -> Result<()> {
        if let Some(&(trusted, recheck)) = self.trust.get(&market_id)
            && !trusted
            && self.round < recheck
        {
            return Ok(());
        }
        let lending = self.scanner.lending;
        let checked = async {
            let market = self.client.market(lending, &market_id).await?;
            if !self.trust.get(&market_id).is_some_and(|(trusted, _)| *trusted) {
                self.client.check_market_code(lending, &market.params).await?;
            }
            let feed = self.client.feed(market.params.oracle_feed, market.params.oracle_program).await?;
            ensure!(self.publishers.contains(&feed.publisher), "feed publisher {} isn't trusted", feed.publisher);
            anyhow::Ok((market, feed))
        };
        let (market, feed) = match checked.await {
            Ok(x) => x,
            Err(e) => {
                self.trust.insert(market_id, (false, self.round + RECHECK_ROUNDS));
                return Err(e.context(format!("skipped for {RECHECK_ROUNDS} rounds")));
            }
        };
        self.trust.insert(market_id, (true, 0));
        let on = On { lending, market: market_id, claims: self.claims, payer: Some(self.payer) };
        for position in positions {
            if self.failed.get(&position).is_some_and(|(_, next)| self.round < *next) {
                continue;
            }
            if let Err(e) = self.position(&on, &market, &feed, position).await {
                self.fail(position);
                log(format!("liquidating {position} failed: {e:#}"));
            }
        }
        Ok(())
    }

    async fn position(&mut self, on: &On, market: &Market, feed: &Feed, position: AccountId) -> Result<()> {
        let Some(p) = self.client.position(on.lending, position).await? else { return Ok(()) };
        let Some(quote) = quote_liquidation(market, &p, feed, &self.claims, now_ms()?)? else { return Ok(()) };
        ensure!(profitable(&quote.liquidation, feed), "seized collateral worth less than the repaid debt: {:?}", quote.liquidation);
        log(format!("{position} is unhealthy (collateral {}, debt shares {}); liquidating", p.collateral, p.borrow_shares));
        match self.client.liquidate(on, self.liquidator, position, None).await? {
            Some(l) => {
                self.failed.remove(&position);
                log(format!(
                    "liquidated {position}: repaid {} ({} shares), seized {}, bad-debt shares {}",
                    l.repaid_assets, l.repaid_shares, l.seized, l.bad_shares
                ));
            }
            None => log(format!("{position} recovered before we got to it")),
        }
        Ok(())
    }

    /// A failed public tx is still charged (constraints §5), so a position whose liquidation failed
    /// is skipped for 1, 2, 4, … rounds (up to 64).
    fn fail(&mut self, position: AccountId) {
        let round = self.round;
        let entry = self.failed.entry(position).or_insert((0, 0));
        entry.0 += 1;
        entry.1 = round + (1u64 << (entry.0 - 1).min(6));
    }
}

/// The seized collateral, valued at the (trusted) feed price, covers the repaid debt. With an honest
/// feed the incentive makes this hold except for dust; it guards against rounding-only liquidations.
fn profitable(l: &Liquidation, feed: &Feed) -> bool {
    U256::from(l.seized) * feed.price / PRICE_SCALE >= U256::from(l.repaid_assets)
}
