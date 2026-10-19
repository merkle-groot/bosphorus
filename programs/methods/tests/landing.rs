//! Experiment E7 / plan 4.4: do a private op's claims survive the proving delay?
//!
//! A private borrow's claims are built at t₀ the way the client builds them; then, for the proving
//! delay D, the price takes a random walk (published every 30 s) and other users' public borrows
//! and repays accrue the market with their own `t`; then the borrow settles at t₀ + D. Settlement
//! re-checks every claim against the state then (constraints §7), so this is what decides landing.
//! Proofs are dev-mode (instant); D is simulated time.
//!
//! `RISC0_DEV_MODE=1 cargo test -p bosphorus-methods --release --test landing -- --ignored --nocapture`
//! Knobs: `LANDING_TRIALS` (40), `LANDING_DELAY_MIN` (10), `LANDING_VOL` (annual, 0.8).

mod common;

use bosphorus_core::{WAD, claims};
use bosphorus_lending::{HealthClaim, Instruction, Interval, Salt, U256, Window, position_id};
use bosphorus_methods::{LENDING_ELF, TOKEN_ELF};
use common::{
    private::Keys,
    world::{ALICE, BOB, World, setup},
};
use lee_core::native_token::encode_balance;

const SALT: Salt = [0x77; 32];

/// xorshift64*: deterministic, no dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn uniform(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Standard normal (Box–Muller).
    fn normal(&mut self) -> f64 {
        let (u, v) = (self.uniform().max(1e-12), self.uniform());
        (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
    }
}

fn env<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Outcome {
    Landed,
    /// The deferred feed check: price at settlement below the claim's `lo`.
    PriceBelowClaim,
    /// The deferred market check: others accrued the ratio past the claim's `hi`.
    RatioAboveClaim,
    Other,
}

/// Real sizes (6 decimals), so a minute of interest is visible in the ratio: bob lends 600,000
/// TKB, alice borrows 400,000 publicly (67% utilisation, ~10% APR), and the private owner has
/// 100,000 TKA of collateral in its salted position.
const OWNER_COLLATERAL: u128 = 100_000_000_000;

fn base() -> (World, common::private::PrivateAccount) {
    let mut w = setup();
    w.supply(BOB, w.position_of(BOB), 600_000_000_000).unwrap();
    w.supply_collateral(ALICE, w.position_of(ALICE), 1_000_000_000_000).unwrap();
    w.borrow(ALICE, 400_000_000_000).unwrap();
    let mut owner = w.chain.private_account(Keys::new(70), &[(w.tka, encode_balance(OWNER_COLLATERAL))]);
    let m = w.accounts();
    let position = position_id(&w.lending, &w.params.id(), &owner.keys.id(), &SALT);
    let ix = Instruction::SupplyCollateral { params: w.params.clone(), assets: OWNER_COLLATERAL };
    let deps = [(w.tka, TOKEN_ELF), (w.tkb, TOKEN_ELF)];
    let witness = owner.witness(&w.chain, true);
    let selectors = vec![(position, w.lending), (owner.keys.id(), w.tka), (m.collateral_vault, w.tka)];
    let tx = w.chain.prove(w.lending, LENDING_ELF, &deps, selectors, vec![witness], ix).unwrap();
    w.chain.settle(&tx).unwrap();
    owner.advance(&tx, |a| a.data.set_shard(w.tka, encode_balance(0)));
    (w, owner)
}

/// One trial: claims at t₀, `delay_ms` of market activity, settle.
fn trial(
    base: &(World, common::private::PrivateAccount),
    rng: &mut Rng,
    (price_width, ratio_width, ahead_ms): (u128, u128, u64),
    delay_ms: u64,
    vol: f64,
) -> Outcome {
    let (mut w, owner) = (base.0.clone(), base.1.clone());
    let m = w.accounts();
    let position = position_id(&w.lending, &w.params.id(), &owner.keys.id(), &SALT);

    // t₀: the client reads market and feed and builds claims (ClaimConfig::claims, ::price).
    let t = w.chain.now;
    let (_, rb) = claims::ratio_claims(&w.market().state, &w.params.irm, t, ahead_ms, ratio_width).unwrap();
    let price = Interval { lo: claims::price_claim(w.price(), price_width).unwrap().lo, hi: U256::MAX };
    let health = HealthClaim { price, rb, window: Window { t, w: delay_ms + 5 * 60_000 } };
    // Borrow half the position's limit at t₀'s price: a borrower keeping a buffer.
    let ix = Instruction::Borrow { params: w.params.clone(), assets: 100_000_000_000, salt: SALT, health };
    let selectors = vec![(owner.keys.id(), w.tkb), (m.market, w.lending), (position, w.lending), (w.feed, w.oracle), (m.loan_vault, w.tkb)];
    let deps = [(w.tka, TOKEN_ELF), (w.tkb, TOKEN_ELF)];
    let tx = w.chain.prove(w.lending, LENDING_ELF, &deps, selectors, vec![owner.witness(&w.chain, true)], ix).unwrap();

    // While it proves: a price every 30 s, and a public borrow or repay every 60 s.
    let mut p = 2.5f64;
    let step_ms = 30_000;
    let sigma = vol * (step_ms as f64 / (365.0 * 24.0 * 3_600_000.0)).sqrt();
    for k in 1..=delay_ms / step_ms {
        w.chain.now = t + k * step_ms;
        p *= (sigma * rng.normal() - sigma * sigma / 2.0).exp();
        w.publish(&format!("{p:.6}"));
        w.chain.now = t + k * step_ms + 1;
        if k % 2 == 0 {
            let public = if rng.uniform() < 0.5 {
                w.borrow(ALICE, 1_000_000_000)
            } else {
                w.repay(ALICE, w.position_of(ALICE), 1_000_000_000_000_000)
            };
            public.expect("the public activity must land, or it doesn't accrue the market");
        }
    }
    w.chain.now = t + delay_ms;

    match w.chain.settle(&tx) {
        Ok(()) => Outcome::Landed,
        Err(e) => {
            let e = format!("{e:#}");
            if e.contains("oracle price outside") {
                Outcome::PriceBelowClaim
            } else if e.contains("share ratio outside") {
                Outcome::RatioAboveClaim
            } else {
                eprintln!("other failure: {}", e.lines().next().unwrap_or_default());
                Outcome::Other
            }
        }
    }
}

#[test]
#[ignore = "experiment: prints a landing table (a few minutes)"]
fn landing_rate_vs_claim_width() {
    assert!(std::env::var("RISC0_DEV_MODE").is_ok(), "run with RISC0_DEV_MODE=1");
    let trials: usize = env("LANDING_TRIALS", 40);
    let delay_ms = env("LANDING_DELAY_MIN", 10u64) * 60_000;
    let vol: f64 = env("LANDING_VOL", 0.8);
    let base = base();
    // (price width, ratio width, ahead)
    let configs: Vec<(u128, u128, u64)> = vec![
        (0, WAD / 10_000_000, delay_ms),
        (WAD / 400, WAD / 10_000_000, delay_ms),
        (WAD / 200, WAD / 10_000_000, delay_ms),
        (WAD / 100, WAD / 10_000_000, delay_ms),
        (WAD / 50, WAD / 10_000_000, delay_ms),
        (WAD / 50, WAD / 1_000_000_000, delay_ms),
        (WAD / 50, WAD / 100_000_000, delay_ms),
        (WAD / 50, WAD / 1_000_000, delay_ms),
        (WAD / 50, WAD / 10_000_000, 30_000),
        (WAD / 50, WAD / 10_000_000, 0),
    ];
    let sigma_d = vol * (delay_ms as f64 / (365.0 * 24.0 * 3_600_000.0)).sqrt();
    println!("\nD = {} min, vol {:.0}%/yr (σ over D = {:.2}%), {trials} trials each\n", delay_ms / 60_000, vol * 100.0, sigma_d * 100.0);
    println!("| price width | ratio width | ahead | landed | price below claim | ratio above claim | other |\n|---|---|---|---:|---:|---:|---:|");
    for (i, config) in configs.into_iter().enumerate() {
        let (width, ratio_width, ahead) = config;
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ (i as u64 + 1));
        let mut counts = std::collections::BTreeMap::new();
        for _ in 0..trials {
            *counts.entry(trial(&base, &mut rng, config, delay_ms, vol)).or_insert(0usize) += 1;
        }
        let n = |o| counts.get(&o).copied().unwrap_or(0);
        println!(
            "| {}% | {:e} | {} s | {}/{trials} | {} | {} | {} |",
            width as f64 / WAD as f64 * 100.0,
            ratio_width as f64 / WAD as f64,
            ahead / 1000,
            n(Outcome::Landed),
            n(Outcome::PriceBelowClaim),
            n(Outcome::RatioAboveClaim),
            n(Outcome::Other)
        );
    }
}
