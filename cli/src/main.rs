//! `bos`: the Bosphorus command line. Uses the LEZ wallet at `LEE_WALLET_HOME_DIR`.
//!
//! Accounts are written `Public/<id>` or `Private/<id>` (a bare id means public).
//! Private txs need real proofs, or `RISC0_DEV_MODE=1` against a dev-mode sequencer.

use anyhow::{Context as _, Result};
use bosphorus_client::{Client, Holder, ORACLE_ELF, TOKEN_ELF};
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

#[tokio::main]
async fn main() -> Result<()> {
    let mut client = Client::from_env().context("opening the wallet (is LEE_WALLET_HOME_DIR set?)")?;
    let id = Holder::id;
    let payer = |p: Option<Holder>| p.map(Holder::id);
    match Cli::parse().command {
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
    }
    Ok(())
}
