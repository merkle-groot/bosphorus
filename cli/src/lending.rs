//! `bos lending …`, `bos market …`, `bos status`. Amounts are base units; rates and LLTVs are
//! decimals or percentages ("0.8", "80%"). Each command prints its main result on the last line.

use anyhow::{Context as _, Result, bail};
use bosphorus_client::{
    Client, Holder,
    lending::{
        ClaimConfig, Instruction, IrmParams, MarketId, MarketParams, On, Scanner, market_id_hex, market_status,
        now_ms, parse_market_id, position_status,
    },
};
use bosphorus_core::{U256, format_wad, parse_wad};
use clap::{Args, Subcommand};

fn wad(s: &str) -> Result<u128> {
    parse_wad(s).with_context(|| format!("bad decimal or percentage: {s}"))
}

/// Roles that only work as signing public accounts: admin, liquidator.
fn public(h: Holder) -> Result<lee::AccountId> {
    match h {
        Holder::Public(id) => Ok(id),
        Holder::Private(_) => bail!("this role needs a Public/ account"),
    }
}

#[derive(Args, Clone)]
pub struct Irm {
    /// Borrow APR at zero utilisation.
    #[arg(long, value_parser = wad)]
    base: u128,
    /// APR added from 0 to the kink.
    #[arg(long, value_parser = wad)]
    slope1: u128,
    /// APR added from the kink to 100%.
    #[arg(long, value_parser = wad)]
    slope2: u128,
    /// Optimal utilisation.
    #[arg(long, value_parser = wad)]
    kink: u128,
}

impl Irm {
    fn params(&self) -> IrmParams {
        IrmParams::linear_kink_apr(self.base, self.slope1, self.slope2, self.kink)
    }
}

#[derive(Args, Clone)]
pub struct ClaimArgs {
    /// Validity window of the tx, seconds [default: 600 public, 1200 private].
    #[arg(long)]
    window: Option<u64>,
    /// How far ahead (seconds) ratio claims allow other txs to accrue the market before ours lands
    /// [default: 30 public, 1200 private].
    #[arg(long)]
    ahead: Option<u64>,
    /// Widening of ratio claims (fraction) [default: 0.000000001].
    #[arg(long, value_parser = wad)]
    ratio_width: Option<u128>,
    /// How far below the feed price the health check's price claim goes (fraction)
    /// [default: 0.5% public, 2% private].
    #[arg(long, value_parser = wad)]
    price_width: Option<u128>,
}

impl ClaimArgs {
    /// Defaults sized for a public tx, or for a private one's proving delay.
    fn config(&self, private: bool) -> ClaimConfig {
        let base = if private { ClaimConfig::private() } else { ClaimConfig::default() };
        ClaimConfig {
            window_ms: self.window.map_or(base.window_ms, |w| w * 1000),
            ahead_ms: self.ahead.map_or(base.ahead_ms, |a| a * 1000),
            ratio_width: self.ratio_width.unwrap_or(base.ratio_width),
            price_width: self.price_width.unwrap_or(base.price_width),
            ..base
        }
    }
}

fn private(holders: &[Option<Holder>]) -> bool {
    holders.iter().flatten().any(|h| matches!(h, Holder::Private(_)))
}

#[derive(Subcommand)]
pub enum Admin {
    /// Upload the lending program. Prints its id.
    Deploy {
        #[arg(long)]
        payer: Holder,
    },
    /// Initialise the config (signed by the program's header key, held by the deploying wallet).
    Init {
        #[arg(long)]
        lending: Holder,
        #[arg(long)]
        admin: Holder,
        #[arg(long)]
        fee_recipient: Holder,
        #[arg(long)]
        payer: Holder,
    },
    EnableLltv {
        #[arg(long)]
        lending: Holder,
        #[arg(long)]
        admin: Holder,
        #[arg(long, value_parser = wad)]
        lltv: u128,
        #[arg(long)]
        payer: Option<Holder>,
    },
    EnableIrm {
        #[arg(long)]
        lending: Holder,
        #[arg(long)]
        admin: Holder,
        #[command(flatten)]
        irm: Irm,
        #[arg(long)]
        payer: Option<Holder>,
    },
    /// Let markets use assets running the bytecode of program `of` (e.g. a token instance).
    EnableAsset {
        #[arg(long)]
        lending: Holder,
        #[arg(long)]
        admin: Holder,
        #[arg(long)]
        of: Holder,
        #[arg(long)]
        payer: Option<Holder>,
    },
    SetFeeRecipient {
        #[arg(long)]
        lending: Holder,
        #[arg(long)]
        admin: Holder,
        #[arg(long)]
        fee_recipient: Holder,
        #[arg(long)]
        payer: Option<Holder>,
    },
    /// Set a market's fee (share of interest, ≤ 25%).
    SetFee {
        #[arg(long)]
        lending: Holder,
        #[arg(long)]
        admin: Holder,
        #[arg(long, value_parser = parse_market_id)]
        market: MarketId,
        #[arg(long, value_parser = wad)]
        fee: u128,
        #[command(flatten)]
        claims: ClaimArgs,
        #[arg(long)]
        payer: Option<Holder>,
    },
    /// Print the config.
    Config {
        #[arg(long)]
        lending: Holder,
    },
}

#[derive(Args, Clone)]
pub struct MarketArgs {
    #[arg(long)]
    lending: Holder,
    #[arg(long, value_parser = parse_market_id)]
    market: MarketId,
    /// Fee payer (default: the signing account).
    #[arg(long)]
    payer: Option<Holder>,
}

impl MarketArgs {
    fn on(&self, claims: ClaimConfig) -> On {
        On { lending: self.lending.id(), market: self.market, claims, payer: payer(self.payer) }
    }
}

#[derive(Subcommand)]
pub enum MarketCmd {
    /// Create a market (permissionless for enabled LLTVs, IRMs and asset bytecode). Prints the market id.
    Create {
        #[arg(long)]
        lending: Holder,
        /// Loan token (or the native token, Public/11111111111111111111111111111111).
        #[arg(long)]
        loan: Holder,
        #[arg(long)]
        collateral: Holder,
        #[arg(long)]
        oracle: Holder,
        /// Feed account (`bos oracle create-feed` prints it).
        #[arg(long)]
        feed: Holder,
        #[arg(long, value_parser = wad)]
        lltv: u128,
        #[command(flatten)]
        irm: Irm,
        /// Seconds.
        #[arg(long, default_value_t = 3600)]
        max_price_age: u64,
        #[arg(long)]
        payer: Holder,
    },
    /// Markets created under a lending program (scans blocks).
    List {
        #[arg(long)]
        lending: Holder,
    },
    /// Supply loan assets from `from` to `owner`'s position (default: from's own).
    Supply {
        #[command(flatten)]
        on: MarketArgs,
        #[arg(long)]
        from: Holder,
        #[arg(long)]
        owner: Option<Holder>,
        /// Which of a private owner's positions in this market (salt index).
        #[arg(long, default_value_t = 0)]
        index: u32,
        #[arg(long)]
        assets: u128,
        #[command(flatten)]
        claims: ClaimArgs,
    },
    /// Withdraw supply shares (or all of them). Prints the assets received.
    Withdraw {
        #[command(flatten)]
        on: MarketArgs,
        #[arg(long)]
        owner: Holder,
        #[arg(long, default_value_t = 0)]
        index: u32,
        #[arg(long, required_unless_present = "all")]
        shares: Option<u128>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        to: Option<Holder>,
        #[command(flatten)]
        claims: ClaimArgs,
    },
    SupplyCollateral {
        #[command(flatten)]
        on: MarketArgs,
        #[arg(long)]
        from: Holder,
        #[arg(long)]
        owner: Option<Holder>,
        /// Which of a private owner's positions in this market (salt index).
        #[arg(long, default_value_t = 0)]
        index: u32,
        #[arg(long)]
        assets: u128,
    },
    WithdrawCollateral {
        #[command(flatten)]
        on: MarketArgs,
        #[arg(long)]
        owner: Holder,
        #[arg(long, default_value_t = 0)]
        index: u32,
        #[arg(long, required_unless_present = "all")]
        assets: Option<u128>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        to: Option<Holder>,
        #[command(flatten)]
        claims: ClaimArgs,
    },
    /// Borrow loan assets. Prints the debt shares taken on.
    Borrow {
        #[command(flatten)]
        on: MarketArgs,
        #[arg(long)]
        owner: Holder,
        #[arg(long, default_value_t = 0)]
        index: u32,
        #[arg(long)]
        assets: u128,
        #[arg(long)]
        to: Option<Holder>,
        #[command(flatten)]
        claims: ClaimArgs,
    },
    /// Repay debt shares (or all of them) of `owner`'s position from `from`. Prints the assets paid.
    Repay {
        #[command(flatten)]
        on: MarketArgs,
        #[arg(long)]
        from: Holder,
        #[arg(long)]
        owner: Option<Holder>,
        #[arg(long, default_value_t = 0)]
        index: u32,
        #[arg(long, required_unless_present = "all")]
        shares: Option<u128>,
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        claims: ClaimArgs,
    },
    /// Liquidate a position in full if it's unhealthy. Prints "repaid seized bad_shares".
    Liquidate {
        #[command(flatten)]
        on: MarketArgs,
        #[arg(long)]
        liquidator: Holder,
        /// The position account (`bos market positions` lists them).
        #[arg(long)]
        position: Holder,
        #[arg(long)]
        to: Option<Holder>,
        #[command(flatten)]
        claims: ClaimArgs,
    },
    /// Move pending fee shares to the fee recipient's position (anyone can). Prints the shares.
    ClaimFees {
        #[command(flatten)]
        on: MarketArgs,
    },
    /// Positions under a lending program (scans blocks), with their market. With `--market` and
    /// `--owner`: that owner's positions, recovered by salt index.
    Positions {
        #[arg(long)]
        lending: Holder,
        #[arg(long, value_parser = parse_market_id, requires = "owner")]
        market: Option<MarketId>,
        #[arg(long, requires = "market")]
        owner: Option<Holder>,
    },
}

#[derive(Args)]
pub struct Status {
    #[arg(long)]
    lending: Holder,
    #[arg(long, value_parser = parse_market_id)]
    market: MarketId,
    #[arg(long)]
    owner: Option<Holder>,
    #[arg(long, default_value_t = 0)]
    index: u32,
}

fn payer(p: Option<Holder>) -> Option<lee::AccountId> {
    p.map(Holder::id)
}

async fn ix_admin(
    client: &Client,
    lending: Holder,
    admin: Holder,
    ix: Instruction,
    p: Option<Holder>,
) -> Result<bosphorus_client::lending::LendingConfig> {
    client.lending_admin(lending.id(), public(admin)?, ix, payer(p)).await
}

pub async fn admin(client: &Client, cmd: Admin) -> Result<()> {
    match cmd {
        Admin::Deploy { .. } => unreachable!("handled in main"),
        Admin::Init { lending, admin, fee_recipient, payer } => {
            client.lending_init(lending.id(), public(admin)?, fee_recipient.id(), payer.id()).await?;
            println!("{:?}", client.lending_config(lending.id()).await?);
        }
        Admin::EnableLltv { lending, admin, lltv, payer } => {
            let config = ix_admin(client, lending, admin, Instruction::EnableLltv { lltv }, payer).await?;
            println!("lltvs {:?}", config.enabled_lltvs.iter().map(|l| format_wad(*l, 4)).collect::<Vec<_>>());
        }
        Admin::EnableIrm { lending, admin, irm, payer } => {
            let config = ix_admin(client, lending, admin, Instruction::EnableIrm { irm: irm.params() }, payer).await?;
            println!("{} IRMs enabled", config.enabled_irms.len());
        }
        Admin::EnableAsset { lending, admin, of, payer } => {
            let image_id = client.header(of.id()).await?.image_id;
            let config = ix_admin(client, lending, admin, Instruction::EnableAsset { image_id }, payer).await?;
            println!("{} asset bytecodes enabled", config.enabled_assets.len());
        }
        Admin::SetFeeRecipient { lending, admin, fee_recipient, payer } => {
            let ix = Instruction::SetFeeRecipient { fee_recipient: fee_recipient.id() };
            let config = ix_admin(client, lending, admin, ix, payer).await?;
            println!("fee recipient {}", config.fee_recipient);
        }
        Admin::SetFee { lending, admin, market, fee, claims, payer: p } => {
            let on = On { lending: lending.id(), market, claims: claims.config(false), payer: payer(p) };
            client.set_fee(&on, public(admin)?, fee).await?;
            println!("fee {}", format_wad(fee, 4));
        }
        Admin::Config { lending } => {
            let config = client.lending_config(lending.id()).await?;
            println!("admin {}\nfee recipient {}", config.admin, config.fee_recipient);
            for lltv in &config.enabled_lltvs {
                println!("lltv {}", format_wad(*lltv, 4));
            }
            for irm in &config.enabled_irms {
                println!("irm {}", describe_irm(irm));
            }
            for image_id in &config.enabled_assets {
                println!("asset bytecode {}", bosphorus_client::hex_id(image_id));
            }
        }
    }
    Ok(())
}

fn describe_irm(irm: &IrmParams) -> String {
    let IrmParams::LinearKink { base, slope1, slope2, u_opt } = *irm;
    let apr = |r: u128| format_wad(bosphorus_core::irm::apr(r), 4);
    format!("linear-kink base {} slope1 {} slope2 {} kink {} (APR)", apr(base), apr(slope1), apr(slope2), format_wad(u_opt, 4))
}

pub async fn market(client: &mut Client, cmd: MarketCmd) -> Result<()> {
    match cmd {
        MarketCmd::Create { lending, loan, collateral, oracle, feed, lltv, irm, max_price_age, payer } => {
            let params = MarketParams {
                loan_asset: loan.id(),
                collateral_asset: collateral.id(),
                oracle_program: oracle.id(),
                oracle_feed: feed.id(),
                irm: irm.params(),
                lltv,
                max_price_age: max_price_age * 1000,
            };
            let id = client.create_market(lending.id(), params, &ClaimConfig::default(), payer.id()).await?;
            println!("{}", market_id_hex(&id));
        }
        MarketCmd::List { lending } => {
            for (id, m) in client.markets(lending.id()).await? {
                let p = &m.params;
                println!(
                    "{}  loan {}  collateral {}  lltv {}  supplied {}  borrowed {}",
                    market_id_hex(&id),
                    p.loan_asset,
                    p.collateral_asset,
                    format_wad(p.lltv, 4),
                    m.state.total_supply_assets,
                    m.state.total_borrow_assets
                );
            }
        }
        MarketCmd::Supply { on, from, owner, index, assets, claims } => {
            let on = on.on(claims.config(private(&[Some(from), owner])));
            let owner = client.owner(owner.unwrap_or(from), &on.market, index)?;
            println!("{}", client.supply(&on, from, on.position(&owner), assets).await?);
        }
        MarketCmd::Withdraw { on, owner, index, shares, all, to, claims } => {
            let on = on.on(claims.config(private(&[Some(owner), to])));
            let owner = client.owner(owner, &on.market, index)?;
            let shares = match shares {
                Some(shares) if !all => shares,
                _ => client.position_shares(on.lending, on.position(&owner)).await?.supply_shares,
            };
            println!("{}", client.withdraw(&on, &owner, shares, to).await?);
        }
        MarketCmd::SupplyCollateral { on, from, owner, index, assets } => {
            let on = on.on(ClaimConfig::default());
            let owner = client.owner(owner.unwrap_or(from), &on.market, index)?;
            let position = on.position(&owner);
            client.supply_collateral(&on, from, position, assets).await?;
            println!("{}", client.position_shares(on.lending, position).await?.collateral);
        }
        MarketCmd::WithdrawCollateral { on, owner, index, assets, all, to, claims } => {
            let on = on.on(claims.config(private(&[Some(owner), to])));
            let owner = client.owner(owner, &on.market, index)?;
            let position = on.position(&owner);
            let assets = match assets {
                Some(assets) if !all => assets,
                _ => client.position_shares(on.lending, position).await?.collateral,
            };
            client.withdraw_collateral(&on, &owner, assets, to).await?;
            println!("{}", client.position_shares(on.lending, position).await?.collateral);
        }
        MarketCmd::Borrow { on, owner, index, assets, to, claims } => {
            let on = on.on(claims.config(private(&[Some(owner), to])));
            let owner = client.owner(owner, &on.market, index)?;
            println!("{}", client.borrow(&on, &owner, assets, to).await?);
        }
        MarketCmd::Repay { on, from, owner, index, shares, all, claims } => {
            let on = on.on(claims.config(private(&[Some(from), owner])));
            let owner = client.owner(owner.unwrap_or(from), &on.market, index)?;
            let position = on.position(&owner);
            let shares = match shares {
                Some(shares) if !all => shares,
                _ => client.position_shares(on.lending, position).await?.borrow_shares,
            };
            println!("{}", client.repay(&on, from, position, shares).await?);
        }
        MarketCmd::Liquidate { on, liquidator, position, to, claims } => {
            let on = on.on(claims.config(false));
            let position = position.id();
            match client.liquidate(&on, public(liquidator)?, position, to.map(public).transpose()?).await? {
                Some(l) => println!("{} {} {}", l.repaid_assets, l.seized, l.bad_shares),
                None => bail!("{position} isn't liquidatable"),
            }
        }
        MarketCmd::ClaimFees { on } => println!("{}", client.claim_fees(&on.on(ClaimConfig::default())).await?),
        MarketCmd::Positions { lending, market: Some(market), owner: Some(owner) } => {
            for (index, position, p) in client.owned_positions(lending.id(), &market, owner).await? {
                println!("{index} {position} supply {} collateral {} borrow {}", p.supply_shares, p.collateral, p.borrow_shares);
            }
        }
        MarketCmd::Positions { lending, .. } => {
            let mut scanner = Scanner::new(lending.id(), 1);
            scanner.scan(client).await?;
            for (position, market) in &scanner.positions {
                println!("{position} {}", market_id_hex(market));
            }
        }
    }
    Ok(())
}

fn pct(x: u128) -> String {
    format!("{}%", format_wad(x * 100, 2))
}

pub async fn status(client: &Client, cmd: Status) -> Result<()> {
    let lending = cmd.lending.id();
    let market = client.market(lending, &cmd.market).await?;
    let now = now_ms()?;
    let s = market_status(&market, now)?;
    let p = &market.params;
    println!("market {}", market_id_hex(&cmd.market));
    println!("  loan {}  collateral {}  lltv {}", p.loan_asset, p.collateral_asset, format_wad(p.lltv, 4));
    println!("  irm {}  max price age {} s", describe_irm(&p.irm), p.max_price_age / 1000);
    let accounts = bosphorus_client::lending::MarketAccounts::new(&lending, &cmd.market);
    println!("  market {}  loan vault {}  collateral vault {}", accounts.market, accounts.loan_vault, accounts.collateral_vault);
    println!("  supplied {}  borrowed {}  (projected to now)", s.state.total_supply_assets, s.state.total_borrow_assets);
    println!(
        "  utilisation {}  borrow APR {}  supply APR {}  fee {}",
        pct(s.utilization),
        pct(s.borrow_apr),
        pct(s.supply_apr),
        pct(market.state.fee)
    );
    let feed = client.feed(p.oracle_feed, p.oracle_program).await?;
    let age = now.saturating_sub(feed.updated_at) / 1000;
    println!("  price {} (1e36 scale), {age} s old, round {}", feed.price, feed.round);
    if let Some(owner) = cmd.owner {
        let on = On { lending, market: cmd.market, claims: ClaimConfig::default(), payer: None };
        let position = on.position(&client.owner(owner, &cmd.market, cmd.index)?);
        let Some(pos) = client.position(lending, position).await? else {
            println!("position {position}: empty");
            return Ok(());
        };
        let st = position_status(&market, &pos, &feed, now)?;
        println!("position {position}");
        println!("  supply shares {} (≈ {} assets)", pos.supply_shares, st.supplied_assets);
        println!("  collateral {}  borrow shares {}  debt {}  max borrow {}", pos.collateral, pos.borrow_shares, st.debt, st.max_borrow);
        let hf = match st.health_factor {
            Some(hf) => format_wad(u128::try_from(hf.min(U256::from(u128::MAX))).unwrap_or(u128::MAX), 4),
            None => "∞".into(),
        };
        println!("  health factor {hf}");
        // Last line: the debt, for scripts.
        println!("{}", st.debt);
    }
    Ok(())
}
