//! `bos disclose`: solvency proofs (E11). The auditor makes a request from chain state and keeps
//! it; the owner proves against it; the auditor verifies the attestation against the kept request.
//! Files are borsh. Amounts are base units; `--min-health` is a decimal (`1.5`).

use std::path::PathBuf;

use anyhow::{Context as _, Result, anyhow};
use bosphorus_client::{
    Client, Holder,
    disclosure::{Attestation, Claims, Request, disclosure_verify, fmt_wad, parse_wad},
    lending::{market_id_hex, parse_market_id},
    pool::hex32,
};
use clap::Subcommand;

#[derive(Subcommand)]
pub enum DiscloseCmd {
    /// A private account's disclosure id: give it to an auditor once; every proof names it.
    Id {
        #[arg(long)]
        holder: Holder,
    },
    /// The auditor's side: a request for claims on one market's positions, written to `--out`.
    Request {
        #[arg(long)]
        lending: Holder,
        #[arg(long)]
        market: String,
        /// Who is asking and why, shown to the owner.
        #[arg(long, default_value = "")]
        label: String,
        /// Health factor at least this (decimal). A position without debt passes.
        #[arg(long)]
        min_health: Option<String>,
        /// Collateral at least this (collateral-asset base units).
        #[arg(long)]
        min_collateral: Option<u128>,
        /// Debt at most this (loan-asset base units).
        #[arg(long)]
        max_debt: Option<u128>,
        /// Supplied assets at least this (loan-asset base units).
        #[arg(long)]
        min_supplied: Option<u128>,
        /// Scan for positions from this block (default 1; the lending instance's deploy block is enough).
        #[arg(long, default_value_t = 1)]
        from_block: u64,
        #[arg(long)]
        out: PathBuf,
    },
    /// What a request asks, and whether it matches the chain (the owner's check before proving).
    Show {
        #[arg(long)]
        request: PathBuf,
        /// Scan for positions from this block (default 1; the lending instance's deploy block is enough).
        #[arg(long, default_value_t = 1)]
        from_block: u64,
    },
    /// The owner's side: prove the request for one of `--holder`'s positions, written to `--out`.
    Prove {
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        holder: Holder,
        /// The position's index (default: the first that meets the claims).
        #[arg(long)]
        index: Option<u32>,
        /// Prove even if fewer than 3 positions meet the claims (they'd narrow you down).
        #[arg(long)]
        force: bool,
        /// Scan for positions from this block (default 1; the lending instance's deploy block is enough).
        #[arg(long, default_value_t = 1)]
        from_block: u64,
        #[arg(long)]
        out: PathBuf,
    },
    /// The auditor's check: the attestation proves the kept request. No wallet or chain needed.
    Verify {
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        attestation: PathBuf,
        /// Fail unless the prover is this disclosure id (hex).
        #[arg(long)]
        expect_id: Option<String>,
    },
}

fn read<T: borsh::BorshDeserialize>(path: &PathBuf) -> Result<T> {
    borsh::from_slice(&std::fs::read(path).with_context(|| format!("reading {}", path.display()))?)
        .with_context(|| format!("{} isn't the right kind of file", path.display()))
}

fn describe(r: &Request) {
    let c = &r.header.claims;
    println!("label: {}", r.header.label);
    println!("market: {}", market_id_hex(&r.header.market_id));
    println!("valued at: {} ms, price {}", r.header.valuation.at_ms, r.header.valuation.price);
    if let Some(h) = c.min_health {
        println!("claim: health >= {}", fmt_wad(h));
    }
    if let Some(x) = c.min_collateral {
        println!("claim: collateral >= {x}");
    }
    if let Some(x) = c.max_debt {
        println!("claim: debt <= {x}");
    }
    if let Some(x) = c.min_supplied {
        println!("claim: supplied >= {x}");
    }
    println!("cover: {} of {} positions meet the claims", r.qualifying(), r.positions.len());
}

/// `verify` runs before a wallet is opened.
pub fn verify(cmd: &DiscloseCmd) -> Option<Result<()>> {
    let DiscloseCmd::Verify { request, attestation, expect_id } = cmd else { return None };
    Some((|| {
        let asked: Request = read(request)?;
        let att: Attestation = read(attestation)?;
        let v = disclosure_verify(&asked, &att)?;
        if let Some(want) = expect_id {
            anyhow::ensure!(hex32(&v.disclosure_id) == want.trim_start_matches("0x"), "proved by {}, not {want}", hex32(&v.disclosure_id));
        }
        describe(&v.request);
        println!("prover: {}", hex32(&v.disclosure_id));
        println!("{}", if v.fake { "VERIFIED (dev-mode receipt: proves nothing)" } else { "VERIFIED" });
        Ok(())
    })())
}

pub async fn run(client: &Client, cmd: DiscloseCmd) -> Result<()> {
    match cmd {
        DiscloseCmd::Id { holder } => println!("{}", hex32(&client.disclosure_id(holder)?)),
        DiscloseCmd::Request { lending, market, label, min_health, min_collateral, max_debt, min_supplied, from_block, out } => {
            let min_health = min_health
                .map(|h| parse_wad(&h).ok_or_else(|| anyhow!("bad --min-health {h}")))
                .transpose()?;
            let claims = Claims { min_health, min_collateral, max_debt, min_supplied };
            let request = client.disclosure_request(lending.id(), &parse_market_id(&market)?, claims, label, from_block).await?;
            std::fs::write(&out, borsh::to_vec(&request)?)?;
            describe(&request);
            println!("nonce: {}", hex32(&request.header.nonce));
        }
        DiscloseCmd::Show { request, from_block } => {
            let request: Request = read(&request)?;
            describe(&request);
            let checked = client.check_disclosure_request(&request, from_block).await?;
            println!("matches the chain: {} of {} positions meet the claims", checked.qualifying, checked.cover);
        }
        DiscloseCmd::Prove { request, holder, index, force, from_block, out } => {
            let request: Request = read(&request)?;
            let started = std::time::Instant::now();
            let home = std::env::var_os("LEE_WALLET_HOME_DIR").map(PathBuf::from).context("LEE_WALLET_HOME_DIR isn't set")?;
            let att = client.disclosure_prove(&request, holder, index, force, &home.join("bosphorus-disclosures"), from_block).await?;
            std::fs::write(&out, borsh::to_vec(&att)?)?;
            println!("proved in {:.1} s, {} bytes", started.elapsed().as_secs_f64(), att.receipt.len());
        }
        DiscloseCmd::Verify { .. } => unreachable!("handled before the wallet opens"),
    }
    Ok(())
}
