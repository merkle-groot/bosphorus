//! Essential private-mode tests (plan Phase 4), in-process with the real guests:
//! a private owner opens a salted position, borrows to its private account, repays from it and
//! withdraws the collateral back to it; no published byte identifies the owner or the salt.
//!
//! Dev-mode proofs: `RISC0_DEV_MODE=1 cargo test -p bosphorus-methods --release --test private`.
//! Real proofs, timed (plan 4.4, ~40 min on an M1):
//! `cargo test -p bosphorus-methods --release --test private -- --ignored --nocapture real_proofs`.

mod common;

use std::time::Instant;

use bosphorus_core::claims;
use bosphorus_lending::{Instruction, Interval, Salt, U256, position_id};
use bosphorus_methods::{LENDING_ELF, TOKEN_ELF};
use common::{
    private::{Keys, PrivateAccount},
    world::{BOB, World, setup},
};
use lee::{AccountId, PrivacyPreservingTransaction};
use lee_core::native_token::encode_balance;

const SALT: Salt = [0x5A; 32];
const COLLATERAL: u128 = 1_000_000;
const BORROWED: u128 = 1_000_000;

struct Private {
    w: World,
    owner: PrivateAccount,
    position: AccountId,
    /// (op, proving time, tx bytes) for the real-proof run.
    timings: Vec<(&'static str, std::time::Duration, usize)>,
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn require_dev_mode() {
    assert!(
        std::env::var("RISC0_DEV_MODE").is_ok_and(|v| v == "1" || v == "true"),
        "run with RISC0_DEV_MODE=1 (real proofs take ~10 min each: use `--ignored real_proofs`)"
    );
}

/// Bob lends 2,000,000 TKB publicly; the private owner holds 1,000,000 TKA and 100,000 TKB.
fn private_world() -> Private {
    let mut w = setup();
    w.supply(BOB, w.position_of(BOB), 2_000_000).unwrap();
    let shards = [(w.tka, encode_balance(COLLATERAL)), (w.tkb, encode_balance(100_000))];
    let owner = w.chain.private_account(Keys::new(60), &shards);
    let position = position_id(&w.lending, &w.params.id(), &owner.keys.id(), &SALT);
    Private { w, owner, position, timings: vec![] }
}

impl Private {
    /// Proves and settles a lending op with the owner as its only private account, then checks
    /// that nothing published identifies the owner.
    fn op(&mut self, name: &'static str, selectors: Vec<(AccountId, AccountId)>, ix: Instruction) -> anyhow::Result<PrivacyPreservingTransaction> {
        let deps = [(self.w.tka, TOKEN_ELF), (self.w.tkb, TOKEN_ELF)];
        let witness = self.owner.witness(&self.w.chain, true);
        let started = Instant::now();
        let tx = self.w.chain.prove(self.w.lending, LENDING_ELF, &deps, selectors, vec![witness], ix)?;
        let proving = started.elapsed();
        let bytes = borsh::to_vec(&tx)?;
        self.timings.push((name, proving, bytes.len()));
        self.w.chain.settle(&tx)?;
        self.assert_no_leaks(&tx, &bytes);
        Ok(tx)
    }

    /// The byte scan: the serialised tx, and every public account it touched after settlement.
    fn assert_no_leaks(&self, tx: &PrivacyPreservingTransaction, bytes: &[u8]) {
        let mut published = vec![("tx".to_string(), bytes.to_vec())];
        for action in &tx.message.public_actions {
            let account = self.w.chain.state.get_account_by_id(action.account_id);
            published.push((format!("public account {}", action.account_id), borsh::to_vec(&account).unwrap()));
        }
        let mut secrets = self.owner.keys.identifying_bytes();
        secrets.push(("salt", SALT.to_vec()));
        for (what, data) in &published {
            for (secret, needle) in &secrets {
                assert!(!contains(data, needle), "{secret} found in {what}");
            }
        }
        let public: Vec<_> = tx.message.public_actions.iter().map(|a| a.account_id).collect();
        assert!(!public.contains(&self.owner.keys.id()), "owner listed as a public account");
        assert!(public.contains(&self.position), "the position is the one public trace of the owner");
    }

    fn balance(&self, token: AccountId) -> u128 {
        bosphorus_token::decode_balance(self.owner.account.data.shard(token)).unwrap()
    }
}

/// open → borrow to private → (a day) → repay from private → withdraw collateral to private.
fn lifecycle(p: &mut Private) {
    let (l, m, owner) = (p.w.lending, p.w.accounts(), p.owner.keys.id());
    let (tka, tkb, params) = (p.w.tka, p.w.tkb, p.w.params.clone());

    // Open: supply collateral from the private account.
    let ix = Instruction::SupplyCollateral { params: params.clone(), assets: COLLATERAL };
    let tx = p.op("supply_collateral", vec![(p.position, l), (owner, tka), (m.collateral_vault, tka)], ix).unwrap();
    p.owner.advance(&tx, |a| a.data.set_shard(tka, encode_balance(0)));
    let position = p.w.chain.shard(p.position, l);
    assert!(!position.is_empty());
    assert_eq!(p.w.balance(tka, m.collateral_vault), COLLATERAL);

    // Borrow to the owner's own private account (no `to`).
    let health = p.w.health();
    let shares = claims::borrow_shares(BORROWED, &health.rb).unwrap();
    let ix = Instruction::Borrow { params: params.clone(), assets: BORROWED, salt: SALT, health };
    let selectors = vec![(owner, tkb), (m.market, l), (p.position, l), (p.w.feed, p.w.oracle), (m.loan_vault, tkb)];
    let tx = p.op("borrow", selectors, ix).unwrap();
    let tkb_before = p.balance(tkb);
    p.owner.advance(&tx, |a| a.data.set_shard(tkb, encode_balance(tkb_before + BORROWED)));

    // A day of interest; repay every share from the private account.
    p.w.chain.now += 24 * 3_600_000;
    p.w.publish("2.5");
    let rb = p.w.ratios().1;
    let rb = Interval { lo: 0, hi: rb.hi };
    let repaid = claims::repay_assets(shares, &rb).unwrap();
    assert!(repaid > BORROWED, "interest accrued");
    let ix = Instruction::Repay { params: params.clone(), shares, rb, window: p.w.window() };
    let tx = p.op("repay", vec![(m.market, l), (p.position, l), (owner, tkb), (m.loan_vault, tkb)], ix).unwrap();
    let tkb_before = p.balance(tkb);
    p.owner.advance(&tx, |a| a.data.set_shard(tkb, encode_balance(tkb_before - repaid)));

    // Withdraw all the collateral back to the private account (debt-free: no price).
    let ix = Instruction::WithdrawCollateral { params, assets: COLLATERAL, salt: SALT, health: None };
    let tx = p.op("withdraw_collateral", vec![(owner, tka), (p.position, l), (m.collateral_vault, tka)], ix).unwrap();
    p.owner.advance(&tx, |a| a.data.set_shard(tka, encode_balance(COLLATERAL)));
    assert_eq!(p.balance(tka), COLLATERAL);
    assert_eq!(p.w.balance(tka, m.collateral_vault), 0);
}

#[test]
fn a_private_owner_runs_the_lifecycle_without_leaking() {
    require_dev_mode();
    let mut p = private_world();
    lifecycle(&mut p);
}

#[test]
fn a_private_owner_must_authorise_and_own_the_salt() {
    require_dev_mode();
    let mut p = private_world();
    let (l, m, owner) = (p.w.lending, p.w.accounts(), p.owner.keys.id());
    let (tka, tkb, params) = (p.w.tka, p.w.tkb, p.w.params.clone());
    let ix = Instruction::SupplyCollateral { params: params.clone(), assets: COLLATERAL };
    let tx = p.op("supply_collateral", vec![(p.position, l), (owner, tka), (m.collateral_vault, tka)], ix).unwrap();
    p.owner.advance(&tx, |a| a.data.set_shard(tka, encode_balance(0)));

    let borrow = |salt: Salt, health| Instruction::Borrow { params: params.clone(), assets: 1000, salt, health };
    let health = p.w.health();
    let deps = [(tka, TOKEN_ELF), (tkb, TOKEN_ELF)];
    let selectors = |position| vec![(owner, tkb), (m.market, l), (position, l), (p.w.feed, p.w.oracle), (m.loan_vault, tkb)];
    // Without `ask`, the owner isn't authorised: refused while proving.
    let no_ask = p.owner.witness(&p.w.chain, false);
    assert!(p.w.chain.prove(l, LENDING_ELF, &deps, selectors(p.position), vec![no_ask], borrow(SALT, health)).is_err());
    // Another salt derives another position.
    let witness = p.owner.witness(&p.w.chain, true);
    assert!(p.w.chain.prove(l, LENDING_ELF, &deps, selectors(p.position), vec![witness], borrow([0; 32], health)).is_err());
    // A stale price claim is refused at settlement (the deferred feed check).
    let witness = p.owner.witness(&p.w.chain, true);
    let mut stale = health;
    stale.price = Interval { lo: health.price.lo + U256::from(1u8), hi: U256::MAX };
    let tx = p.w.chain.prove(l, LENDING_ELF, &deps, selectors(p.position), vec![witness], borrow(SALT, stale)).unwrap();
    assert!(p.w.chain.settle(&tx).is_err(), "price claim above the feed");
}

/// Plan 4.4: proving time and tx size per private op, with real proofs.
#[test]
#[ignore = "real proofs: ~10 min per op on an M1"]
fn real_proofs() {
    assert!(std::env::var("RISC0_DEV_MODE").is_err(), "unset RISC0_DEV_MODE for real proofs");
    let mut p = private_world();
    lifecycle(&mut p);
    println!("\n| private op | proving | tx bytes |\n|---|---:|---:|");
    for (op, time, bytes) in &p.timings {
        println!("| {op} | {:.1} s | {bytes} |", time.as_secs_f64());
    }
}
