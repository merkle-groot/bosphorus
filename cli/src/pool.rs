//! `bos pool`: the screened pool (E10, Privacy Pools on LEZ). `--token` picks the asset; without
//! it, native LEZ.

use std::collections::HashSet;

use anyhow::{Context as _, Result, ensure};
use bosphorus_client::{
    Client, Holder, POOL_ELF,
    pool::{AspScanner, hex32, parse_label},
};
use clap::Subcommand;
use lee::{AccountId, native_token::NATIVE_TOKEN_PROGRAM_ID};

#[derive(Subcommand)]
pub enum PoolCmd {
    /// Upload the pool program and initialise it with its ASP. Prints its id.
    Deploy {
        #[arg(long)]
        asp: Holder,
        #[arg(long)]
        payer: Holder,
    },
    /// A screened shield: from a public account into a lot on one of this wallet's private
    /// accounts. Prints the deposit's label.
    Deposit {
        #[arg(long)]
        pool: Holder,
        #[arg(long)]
        token: Option<Holder>,
        #[arg(long)]
        from: Holder,
        #[arg(long)]
        to: Holder,
        #[arg(long)]
        amount: u128,
    },
    /// Pay `amount` out of a private account's approved lots to any account. Prints the
    /// recipient's balance.
    Withdraw {
        #[arg(long)]
        pool: Holder,
        #[arg(long)]
        token: Option<Holder>,
        #[arg(long)]
        from: Holder,
        #[arg(long)]
        to: Holder,
        #[arg(long)]
        amount: u128,
    },
    /// The depositor takes a lot back, publicly. Prints the amount returned.
    Ragequit {
        #[arg(long)]
        pool: Holder,
        /// The private account holding the lot.
        #[arg(long)]
        holder: Holder,
        #[arg(long)]
        label: String,
    },
    /// A private account's lots: "label asset amount approved|pending".
    Lots {
        #[arg(long)]
        pool: Holder,
        #[arg(long)]
        holder: Holder,
    },
    /// The ASP's scan: deposits since `--from-block`, "label depositor asset amount block
    /// admitted|<why not> approved|pending|exited".
    Scan {
        #[arg(long)]
        pool: Holder,
        #[arg(long, default_value_t = 1)]
        from_block: u64,
    },
    /// Approve labels (signed by the ASP). `--admitted` approves every admitted deposit the scan
    /// finds that is at least `--min-age-s` old, isn't approved or taken back, and whose depositor
    /// isn't in `--deny` (a file, one account id per line). Prints the labels approved.
    Approve {
        #[arg(long)]
        pool: Holder,
        #[arg(long)]
        asp: Holder,
        labels: Vec<String>,
        #[arg(long)]
        admitted: bool,
        #[arg(long, default_value_t = 1)]
        from_block: u64,
        #[arg(long, default_value_t = 0)]
        min_age_s: u64,
        #[arg(long)]
        deny: Option<std::path::PathBuf>,
        #[arg(long)]
        payer: Option<Holder>,
    },
    /// Remove labels (signed by the ASP): older roots stop working at once.
    Remove {
        #[arg(long)]
        pool: Holder,
        #[arg(long)]
        asp: Holder,
        #[arg(required = true)]
        labels: Vec<String>,
        #[arg(long)]
        payer: Option<Holder>,
    },
    /// The ASP, the accepted roots and the approved count.
    Info {
        #[arg(long)]
        pool: Holder,
    },
}

fn asset(token: Option<Holder>) -> AccountId {
    token.map_or(NATIVE_TOKEN_PROGRAM_ID, Holder::id)
}

async fn balance(client: &Client, asset: AccountId, holder: Holder) -> Result<u128> {
    if asset == NATIVE_TOKEN_PROGRAM_ID { client.native(holder).await } else { client.token_balance(asset, holder).await }
}

pub async fn run(client: &mut Client, cmd: PoolCmd) -> Result<()> {
    match cmd {
        PoolCmd::Deploy { asp, payer } => {
            let pool = client.deploy(POOL_ELF, payer.id()).await?;
            client.pool_init(pool, asp.id(), payer.id()).await?;
            println!("{}", Holder::Public(pool));
        }
        PoolCmd::Deposit { pool, token, from, to, amount } => {
            let (sent, label) = client.send_pool_deposit(pool.id(), asset(token), from, to, amount).await?;
            client.land(sent).await?;
            let lots = client.pool_lots(pool.id(), to.id())?;
            ensure!(lots.0.iter().any(|l| l.label == label && l.amount == amount), "the lot isn't in {to} (sync the wallet?)");
            println!("{}", hex32(&label));
        }
        PoolCmd::Withdraw { pool, token, from, to, amount } => {
            let asset = asset(token);
            let before = balance(client, asset, to).await?;
            let sent = client.send_pool_withdraw(pool.id(), asset, from.id(), amount, to).await?;
            client.land(sent).await?;
            let after = balance(client, asset, to).await?;
            ensure!(after == before + amount, "{to} went {before} → {after}, expected +{amount}");
            println!("{after}");
        }
        PoolCmd::Ragequit { pool, holder, label } => {
            let label = parse_label(&label)?;
            let record = client.pool_record(pool.id(), &label).await?.context("no deposit record for this label")?;
            let lot = client.pool_lots(pool.id(), holder.id())?.0.into_iter().find(|l| l.label == label).context("no such lot")?;
            let sent = client.send_pool_ragequit(pool.id(), holder.id(), label).await?;
            client.land(sent).await?;
            ensure!(client.pool_record(pool.id(), &label).await?.is_some_and(|r| r.exited), "the record isn't marked taken back");
            eprintln!("returned to {}", record.depositor);
            println!("{}", lot.amount);
        }
        PoolCmd::Lots { pool, holder } => {
            for (lot, state) in client.pool_lot_states(pool.id(), holder.id()).await? {
                println!("{} {} {} {}", hex32(&lot.label), AccountId::new(lot.asset), lot.amount, state.name());
            }
        }
        PoolCmd::Scan { pool, from_block } => {
            let approved = client.pool_approved(pool.id()).await?;
            let mut scanner = AspScanner::new(pool.id(), from_block);
            for f in scanner.scan(client).await? {
                let d = &f.admission.deposit;
                let exited = client.pool_record(pool.id(), &d.label).await?.is_some_and(|r| r.exited);
                let state = if exited { "exited" } else if approved.contains(&d.label) { "approved" } else { "pending" };
                let admitted = f.admission.admitted.err().unwrap_or("admitted");
                println!("{} {} {} {} {} {admitted} {state}", hex32(&d.label), d.depositor, d.asset, d.amount, f.block);
            }
        }
        PoolCmd::Approve { pool, asp, labels, admitted, from_block, min_age_s, deny, payer } => {
            let mut chosen = labels.iter().map(|l| parse_label(l)).collect::<Result<Vec<_>>>()?;
            if admitted {
                let denied: HashSet<AccountId> = match deny {
                    Some(path) => std::fs::read_to_string(&path)
                        .with_context(|| format!("reading {}", path.display()))?
                        .lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty() && !l.starts_with('#'))
                        .map(|l| l.parse::<Holder>().map(Holder::id))
                        .collect::<Result<_>>()?,
                    None => HashSet::new(),
                };
                let approved = client.pool_approved(pool.id()).await?;
                let now = bosphorus_client::now_ms()?;
                let mut scanner = AspScanner::new(pool.id(), from_block);
                for f in scanner.scan(client).await? {
                    let d = &f.admission.deposit;
                    let old_enough = now.saturating_sub(f.timestamp_ms) >= min_age_s * 1000;
                    let exited = client.pool_record(pool.id(), &d.label).await?.is_none_or(|r| r.exited);
                    if f.admission.admitted.is_ok() && old_enough && !exited && !denied.contains(&d.depositor) && !approved.contains(&d.label) && !chosen.contains(&d.label) {
                        chosen.push(d.label);
                    }
                }
            }
            if !chosen.is_empty() {
                client.pool_approve(pool.id(), asp.id(), &chosen, payer.map(Holder::id)).await?;
            }
            for label in &chosen {
                println!("{}", hex32(label));
            }
        }
        PoolCmd::Remove { pool, asp, labels, payer } => {
            let labels = labels.iter().map(|l| parse_label(l)).collect::<Result<Vec<_>>>()?;
            client.pool_remove(pool.id(), asp.id(), &labels, payer.map(Holder::id)).await?;
        }
        PoolCmd::Info { pool } => {
            let config = client.pool_config(pool.id()).await?;
            println!("asp {}", Holder::Public(config.asp));
            println!("roots {}", config.roots.len());
            println!("approved {}", client.pool_approved(pool.id()).await?.labels().count());
        }
    }
    Ok(())
}
