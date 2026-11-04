//! `bos`: the Bosphorus command line. Uses the LEZ wallet at `LEE_WALLET_HOME_DIR`.
//!
//! Accounts are written `Public/<id>` or `Private/<id>` (a bare id means public).
//! Private txs need real proofs, or `RISC0_DEV_MODE=1` against a dev-mode sequencer.

mod lending;
mod disclose;
mod pool;
mod report;

use anyhow::{Context as _, Result};
use bosphorus_client::{Client, Holder, LENDING_ELF, ORACLE_ELF, TOKEN_ELF};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "bos", about = "Bosphorus: a lending market on LEZ")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(subcommand)]
    Token(Token),
    #[command(subcommand)]
    Oracle(Oracle),
    /// Test tokens and gas for newcomers.
    #[command(subcommand)]
    Faucet(Faucet),
    /// Deploy and administer the lending program.
    #[command(subcommand)]
    Lending(lending::Admin),
    /// Create markets and act on positions.
    #[command(subcommand)]
    Market(lending::MarketCmd),
    /// The screened pool (E10): deposits, withdrawals, ragequit, and the ASP.
    #[command(subcommand)]
    Pool(pool::PoolCmd),
    /// Solvency proofs (E11): prove a position meets claims without saying which it is.
    #[command(subcommand)]
    Disclose(disclose::DiscloseCmd),
    /// A market's utilisation and rates, and optionally a position's health.
    Status(lending::Status),
    /// What a tx published, and a scan for an owner's identifying bytes.
    TxReport(report::TxReport),
    /// Pre-flight: the sequencer's privacy circuit, and that deployed programs run this build.
    Check {
        #[arg(long)]
        lending: Option<Holder>,
        #[arg(long)]
        oracle: Option<Holder>,
        /// Repeatable.
        #[arg(long)]
        token: Vec<Holder>,
    },
    /// This build's program image ids and ELF sizes; `--write-dir` exports the ELFs (a private tx
    /// must be proven with exactly the deployed bytecode).
    Programs {
        #[arg(long)]
        write_dir: Option<std::path::PathBuf>,
    },
    /// Block timestamps and tx counts: "id timestamp_ms gap_ms public private", then a summary line.
    Blocks {
        #[arg(long)]
        from: Option<u64>,
        #[arg(long)]
        to: Option<u64>,
        /// The last N blocks (default 100) when `--from` is absent.
        #[arg(long, default_value_t = 100)]
        last: u64,
        /// Only the summary.
        #[arg(long)]
        summary: bool,
    },
}

#[derive(Subcommand)]
enum Token {
    /// Create and initialise a token instance. Prints its id.
    Create {
        #[arg(long)]
        decimals: u8,
        #[arg(long)]
        mint_authority: Holder,
        /// Reuse the bytecode of an existing token instance (one header tx) instead of uploading it.
        #[arg(long)]
        segments_of: Option<Holder>,
        #[arg(long)]
        payer: Holder,
    },
    Mint {
        #[arg(long)]
        token: Holder,
        #[arg(long)]
        authority: Holder,
        #[arg(long)]
        to: Holder,
        #[arg(long)]
        amount: u128,
        #[arg(long)]
        payer: Option<Holder>,
    },
    /// Public transfer.
    Transfer {
        #[arg(long)]
        token: Holder,
        #[arg(long)]
        from: Holder,
        #[arg(long)]
        to: Holder,
        #[arg(long)]
        amount: u128,
        #[arg(long)]
        payer: Option<Holder>,
    },
    /// Move tokens from a public account into one of this wallet's private accounts.
    Shield {
        #[arg(long)]
        token: Holder,
        #[arg(long)]
        from: Holder,
        #[arg(long)]
        to: Holder,
        #[arg(long)]
        amount: u128,
    },
    Balance {
        #[arg(long)]
        token: Holder,
        #[arg(long)]
        account: Holder,
    },
    Info {
        #[arg(long)]
        token: Holder,
    },
}

#[derive(Subcommand)]
enum Faucet {
    /// Upload the faucet program. Prints its id.
    Deploy {
        #[arg(long)]
        payer: Holder,
    },
    /// Set the drops (`ASSET=AMOUNT`, `native` for gas) and the cooldown. Once.
    Init {
        #[arg(long)]
        faucet: Holder,
        #[arg(long, required = true, num_args = 1..)]
        drop: Vec<String>,
        #[arg(long, default_value_t = 86_400)]
        cooldown_secs: u64,
        #[arg(long)]
        payer: Holder,
    },
    /// Prints the `mint_authority` to give a token this faucet drops, and the gas custody account.
    Accounts {
        #[arg(long)]
        faucet: Holder,
    },
    /// Claim `asset`'s drop (`native` for gas) for `claimer`, paid to `to` (default: the claimer).
    Claim {
        #[arg(long)]
        faucet: Holder,
        #[arg(long)]
        asset: String,
        #[arg(long)]
        claimer: Holder,
        #[arg(long)]
        to: Option<Holder>,
        /// Fee payer of a public claim (default: the claimer).
        #[arg(long)]
        payer: Option<Holder>,
    },
    Info {
        #[arg(long)]
        faucet: Holder,
    },
}

/// A faucet asset: `native`, or a token's `Public/<id>`.
fn faucet_asset(s: &str) -> Result<lee::AccountId> {
    if s == "native" {
        return Ok(lee_core::native_token::NATIVE_TOKEN_PROGRAM_ID);
    }
    Ok(s.parse::<Holder>()?.id())
}

#[derive(Subcommand)]
enum Oracle {
    /// Upload the oracle program. Prints its id.
    Deploy {
        #[arg(long)]
        payer: Holder,
    },
    /// Create a feed published by `publisher`. Prints the feed account.
    CreateFeed {
        #[arg(long)]
        oracle: Holder,
        #[arg(long)]
        publisher: Holder,
        #[arg(long)]
        name: String,
        #[arg(long)]
        payer: Option<Holder>,
    },
    /// Publish "1 collateral token = PRICE loan tokens", stamped with the current time.
    Publish {
        #[arg(long)]
        oracle: Holder,
        #[arg(long)]
        publisher: Holder,
        #[arg(long)]
        name: String,
        #[arg(long)]
        price: String,
        #[arg(long, default_value_t = 0)]
        loan_decimals: u8,
        #[arg(long, default_value_t = 0)]
        collateral_decimals: u8,
        #[arg(long)]
        payer: Option<Holder>,
    },
    Feed {
        #[arg(long)]
        oracle: Holder,
        #[arg(long)]
        publisher: Holder,
        #[arg(long)]
        name: String,
    },
}

fn main() -> Result<()> {
    // SAFETY: still single-threaded; the runtime starts below.
    unsafe { bosphorus_client::proving::configure() };
    tokio::runtime::Runtime::new()?.block_on(run(Cli::parse().command))
}

async fn run(command: Command) -> Result<()> {
    if let Command::Disclose(cmd) = &command
        && let Some(done) = disclose::verify(cmd)
    {
        return done;
    }
    if let Command::Programs { write_dir } = &command {
        // No wallet needed.
        for (name, elf) in [("token", TOKEN_ELF), ("oracle", ORACLE_ELF), ("lending", LENDING_ELF), ("pool", bosphorus_client::POOL_ELF), ("faucet", bosphorus_client::FAUCET_ELF), ("disclosure", bosphorus_client::disclosure::DISCLOSURE_ELF)] {
            println!("{name} {} {} bytes", bosphorus_client::hex_id(&bosphorus_client::image_id(elf)?), elf.len());
            if let Some(dir) = write_dir {
                std::fs::create_dir_all(dir)?;
                std::fs::write(dir.join(format!("{name}.bin")), elf)?;
            }
        }
        return Ok(());
    }
    let mut client = Client::from_env().context("opening the wallet (is LEE_WALLET_HOME_DIR set?)")?;
    let id = Holder::id;
    let payer = |p: Option<Holder>| p.map(Holder::id);
    match command {
        Command::Token(cmd) => match cmd {
            Token::Create { decimals, mint_authority, segments_of, payer } => {
                let token = match segments_of {
                    Some(of) => client.instance(of.id(), payer.id()).await?,
                    None => client.deploy(TOKEN_ELF, payer.id()).await?,
                };
                client.token_init(token, mint_authority.id(), decimals, payer.id()).await?;
                println!("{}", Holder::Public(token));
            }
            Token::Mint { token, authority, to, amount, payer: p } => {
                client.token_mint(id(token), id(authority), id(to), amount, payer(p)).await?;
                println!("{}", client.token_balance(id(token), to).await?);
            }
            Token::Transfer { token, from, to, amount, payer: p } => {
                client.token_transfer(id(token), id(from), id(to), amount, payer(p)).await?;
                println!("{}", client.token_balance(id(token), from).await?);
            }
            Token::Shield { token, from, to, amount } => {
                client.token_shield(id(token), id(from), id(to), amount).await?;
                println!("{}", client.token_balance(id(token), to).await?);
            }
            Token::Balance { token, account } => println!("{}", client.token_balance(id(token), account).await?),
            Token::Info { token } => println!("{:?}", client.token_config(id(token)).await?),
        },
        Command::Oracle(cmd) => match cmd {
            Oracle::Deploy { payer } => println!("{}", Holder::Public(client.deploy(ORACLE_ELF, payer.id()).await?)),
            Oracle::CreateFeed { oracle, publisher, name, payer: p } => {
                let feed = client.feed_create(id(oracle), id(publisher), &name, payer(p)).await?;
                println!("{}", Holder::Public(feed));
            }
            Oracle::Publish { oracle, publisher, name, price, loan_decimals, collateral_decimals, payer: p } => {
                let scaled = bosphorus_core::scale_price(&price, loan_decimals, collateral_decimals)
                    .with_context(|| format!("bad price {price}"))?;
                let feed = client.feed_publish(id(oracle), id(publisher), &name, scaled, payer(p)).await?;
                println!("round {} price {}", feed.round, feed.price);
            }
            Oracle::Feed { oracle, publisher, name } => {
                let feed = bosphorus_client::feed_id(id(oracle), id(publisher), &name);
                println!("{:?}", client.feed(feed, id(oracle)).await?);
            }
        },
        Command::Faucet(cmd) => match cmd {
            Faucet::Deploy { payer } => println!("{}", Holder::Public(client.deploy(bosphorus_client::FAUCET_ELF, payer.id()).await?)),
            Faucet::Init { faucet, drop, cooldown_secs, payer } => {
                let drops = drop
                    .iter()
                    .map(|d| {
                        let (asset, amount) = d.split_once('=').with_context(|| format!("expected ASSET=AMOUNT, got {d}"))?;
                        Ok(bosphorus_faucet::Drop { asset: faucet_asset(asset)?, amount: amount.parse()? })
                    })
                    .collect::<Result<Vec<_>>>()?;
                client.faucet_init(faucet.id(), drops, cooldown_secs * 1000, payer.id()).await?;
                println!("{:?}", client.faucet_config(faucet.id()).await?);
            }
            Faucet::Accounts { faucet } => {
                println!("mint_authority {}", Holder::Public(bosphorus_faucet::mint_authority(&faucet.id())));
                println!("custody {}", Holder::Public(bosphorus_faucet::custody_id(&faucet.id())));
            }
            Faucet::Claim { faucet, asset, claimer, to, payer: p } => {
                let asset = faucet_asset(&asset)?;
                let to = to.unwrap_or(claimer);
                client.faucet_claim(faucet.id(), asset, claimer, to, payer(p)).await?;
                let balance = if bosphorus_faucet::is_native(&asset) {
                    client.native_balance(to.id()).await?
                } else {
                    client.token_balance(asset, to).await?
                };
                println!("{balance}");
            }
            Faucet::Info { faucet } => println!("{:?}", client.faucet_config(faucet.id()).await?),
        },
        Command::Lending(lending::Admin::Deploy { payer }) => {
            println!("{}", Holder::Public(client.deploy(LENDING_ELF, payer.id()).await?));
        }
        Command::Lending(cmd) => lending::admin(&client, cmd).await?,
        Command::Market(cmd) => lending::market(&mut client, cmd).await?,
        Command::Pool(cmd) => pool::run(&mut client, cmd).await?,
        Command::Disclose(cmd) => disclose::run(&client, cmd).await?,
        Command::Status(cmd) => lending::status(&client, cmd).await?,
        Command::TxReport(cmd) => report::tx_report(&client, cmd).await?,
        Command::Check { lending, oracle, token } => {
            client.check_circuit().await?;
            println!("privacy circuit: matches");
            let programs = lending.map(|l| ("lending", l, LENDING_ELF)).into_iter()
                .chain(oracle.map(|o| ("oracle", o, ORACLE_ELF)))
                .chain(token.into_iter().map(|t| ("token", t, TOKEN_ELF)));
            for (name, holder, elf) in programs {
                let header = client.header(holder.id()).await?;
                let ours = bosphorus_client::image_id(elf)?;
                anyhow::ensure!(header.image_id == ours, "{name} {holder} runs {}, this build is {}", bosphorus_client::hex_id(&header.image_id), bosphorus_client::hex_id(&ours));
                anyhow::ensure!(header.immutable, "{name} {holder} has a mutable header");
                println!("{name} {holder}: this build, immutable");
            }
            println!("ok");
        }
        Command::Programs { .. } => unreachable!("handled above"),
        Command::Blocks { from, to, last, summary } => {
            let head = client.last_block().await?;
            let to = to.unwrap_or(head).min(head);
            let from = from.unwrap_or(to.saturating_sub(last.saturating_sub(1)).max(1));
            let blocks = client.blocks(from, to).await?;
            let mut gaps = vec![];
            let (mut public, mut private) = (0, 0);
            for (i, b) in blocks.iter().enumerate() {
                let gap = i.checked_sub(1).map(|j| b.timestamp_ms.saturating_sub(blocks[j].timestamp_ms));
                gaps.extend(gap);
                public += b.public;
                private += b.private;
                if !summary {
                    println!("{} {} {} {} {}", b.id, b.timestamp_ms, gap.map_or("-".into(), |g| g.to_string()), b.public, b.private);
                }
            }
            gaps.sort_unstable();
            let pct = |p: usize| gaps.get((gaps.len().saturating_sub(1)) * p / 100).copied().unwrap_or(0);
            let span = blocks.last().zip(blocks.first()).map_or(0, |(l, f)| l.timestamp_ms - f.timestamp_ms);
            println!(
                "blocks {from}..={to}: {} blocks over {} s, gap ms p50 {} p90 {} p99 {} max {}, stalls (gap > 2 × p50): {}, txs public {public} (incl. clock) private {private}",
                blocks.len(),
                span / 1000,
                pct(50),
                pct(90),
                pct(99),
                gaps.last().copied().unwrap_or(0),
                gaps.iter().filter(|g| **g > 2 * pct(50)).count(),
            );
        }
    }
    Ok(())
}
