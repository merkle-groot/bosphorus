//! `bos-keeper`: watches every market under a lending program and liquidates unhealthy positions.
//!
//! Each round: scan new blocks for lending accounts (positions created by public or private txs),
//! then for each known position read market, feed and position, and liquidate it in full if it's
//! unhealthy at the claims the tx will make. Stateless apart from the scanner's known-position set.
//! Uses the LEZ wallet at `LEE_WALLET_HOME_DIR`, like `bos`.

use std::{
    collections::{BTreeMap, HashMap},
    time::Duration,
};

use anyhow::{Context as _, Result};
use bosphorus_client::{
    Client, Holder,
    lending::{ClaimConfig, MarketId, On, Scanner, market_id_hex, now_ms, quote_liquidation},
};
use bosphorus_core::parse_wad;
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
    /// Seconds between rounds.
    #[arg(long, default_value_t = 5)]
    interval: u64,
    /// First block to scan.
    #[arg(long, default_value_t = 1)]
    from_block: u64,
    /// Run one round and exit.
    #[arg(long)]
    once: bool,
    /// Validity window of liquidation txs, seconds.
    #[arg(long, default_value_t = 60)]
    window: u64,
    #[arg(long, default_value = "0.000000001", value_parser = |s: &str| parse_wad(s).context("bad fraction"))]
    ratio_width: u128,
}

fn log(msg: impl std::fmt::Display) {
    eprintln!("[keeper {}] {msg}", now_ms().unwrap_or_default() / 1000);
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let client = Client::from_env().context("opening the wallet (is LEE_WALLET_HOME_DIR set?)")?;
    let lending = args.lending.id();
    let liquidator = args.liquidator.id();
    let claims = ClaimConfig { window_ms: args.window * 1000, ratio_width: args.ratio_width, ..ClaimConfig::default() };
    let payer = args.payer.map(Holder::id);
    client.check_program(lending, bosphorus_client::LENDING_ELF).await?;
    let mut scanner = Scanner::new(lending, args.from_block);
    let mut backoff = Backoff::default();
    log(format!("watching lending {lending} from block {}, liquidator {liquidator}", args.from_block));
    loop {
        backoff.round += 1;
        if let Err(e) = round(&client, &mut scanner, &mut backoff, liquidator, claims, payer).await {
            log(format!("round failed: {e:#}"));
        }
        if args.once {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(args.interval)).await;
    }
}

/// A failed public tx is still charged (constraints §5), so a position whose liquidation failed is
/// skipped for 1, 2, 4, … rounds (up to 64).
#[derive(Default)]
struct Backoff {
    round: u64,
    /// Position → (failures, next round to try).
    failed: HashMap<AccountId, (u32, u64)>,
}

impl Backoff {
    fn ready(&self, position: &AccountId) -> bool {
        self.failed.get(position).is_none_or(|(_, next)| self.round >= *next)
    }

    fn fail(&mut self, position: AccountId) {
        let round = self.round;
        let entry = self.failed.entry(position).or_insert((0, 0));
        entry.0 += 1;
        entry.1 = round + (1u64 << (entry.0 - 1).min(6));
    }
}

async fn round(
    client: &Client,
    scanner: &mut Scanner,
    backoff: &mut Backoff,
    liquidator: AccountId,
    claims: ClaimConfig,
    payer: Option<AccountId>,
) -> Result<()> {
    for position in scanner.scan(client).await? {
        log(format!("found position {position}"));
    }
    let mut by_market: BTreeMap<MarketId, Vec<AccountId>> = BTreeMap::new();
    for (position, market) in &scanner.positions {
        by_market.entry(*market).or_default().push(*position);
    }
    for (market_id, positions) in by_market {
        let on = On { lending: scanner.lending, market: market_id, claims, payer };
        let market = client.market(on.lending, &market_id).await?;
        let feed = client.feed(market.params.oracle_feed, market.params.oracle_program).await?;
        for position in positions {
            if !backoff.ready(&position) {
                continue;
            }
            let Some(p) = client.position(on.lending, position).await? else { continue };
            let quote = match quote_liquidation(&market, &p, &feed, &claims, now_ms()?) {
                Ok(Some(q)) => q,
                Ok(None) => continue,
                Err(e) => {
                    log(format!("market {}: can't quote {position}: {e:#}", market_id_hex(&market_id)));
                    continue;
                }
            };
            log(format!("{position} is unhealthy (collateral {}, debt shares {}); liquidating", p.collateral, p.borrow_shares));
            match client.liquidate(&on, liquidator, position, None).await {
                Ok(Some(l)) => {
                    backoff.failed.remove(&position);
                    log(format!(
                        "liquidated {position}: repaid {} ({} shares), seized {}, bad-debt shares {}",
                        l.repaid_assets, l.repaid_shares, l.seized, l.bad_shares
                    ));
                }
                Ok(None) => log(format!("{position} recovered before we got to it")),
                Err(e) => {
                    backoff.fail(position);
                    log(format!("liquidating {position} failed ({:?}): {e:#}", quote.amount));
                }
            }
        }
    }
    Ok(())
}
