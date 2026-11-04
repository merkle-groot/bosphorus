//! Essential faucet tests (onboarding), in-process with the real guests. Money safety: a claim pays
//! exactly the configured drop, once per cooldown per (claimer, asset), only on the claimer's
//! signature; only this faucet's mint PDA mints. Gas: a fresh wallet with no native claims its
//! first gas with its private account (a fee-exempt private tx), paid to its public account.
//!
//! `RISC0_DEV_MODE=1 cargo test -p bosphorus-methods --release --test faucet`.

mod common;

use bosphorus_faucet::{
    CLAIM_WINDOW_MS, Drop, Instruction, Record, claim_selectors, config_id, custody_id, mint_authority,
};
use bosphorus_methods::{FAUCET_ELF, TOKEN_ELF};
use common::{
    Chain, addr, key,
    private::Keys,
};
use lee::AccountId;
use lee_core::native_token::{NATIVE_TOKEN_PROGRAM_ID as NATIVE, decode_balance};

const DEPLOYER: u8 = 1;
const ALICE: u8 = 2;
const BOB: u8 = 3;
const DAY: u64 = 24 * 60 * 60 * 1000;
const USDC_DROP: u128 = 10_000_000_000;
const GAS_DROP: u128 = 5_000_000;

struct World {
    chain: Chain,
    faucet: AccountId,
    usdc: AccountId,
}

fn token(chain: &mut Chain, owner: u8, mint_authority: AccountId, decimals: u8) -> AccountId {
    let token = chain.deploy(TOKEN_ELF, &key(owner));
    let accounts = vec![(token, token), (bosphorus_token::config_id(&token), token)];
    chain.call(token, accounts, &[&key(owner)], bosphorus_token::Instruction::Init { mint_authority, decimals }).unwrap();
    token
}

fn setup() -> World {
    let mut chain = Chain::new();
    let faucet = chain.deploy(FAUCET_ELF, &key(10));
    let usdc = token(&mut chain, 11, mint_authority(&faucet), 6);
    let drops = vec![Drop { asset: usdc, amount: USDC_DROP }, Drop { asset: NATIVE, amount: GAS_DROP }];
    let accounts = vec![(faucet, faucet), (config_id(&faucet), faucet)];
    chain.call(faucet, accounts, &[&key(10)], Instruction::Init { drops, cooldown_ms: DAY }).unwrap();
    // The custody PDA's native, as a plain transfer would put it there.
    chain.state = chain.state.clone().with_public_account_balances([(custody_id(&faucet), 3 * GAS_DROP)]);
    let _ = DEPLOYER;
    World { chain, faucet, usdc }
}

impl World {
    fn claim_as(&mut self, signer: Option<u8>, claimer: u8, asset: AccountId, amount: u128, cooldown_ms: u64) -> anyhow::Result<u64> {
        let selectors = claim_selectors(&self.faucet, &asset, addr(&key(claimer)), addr(&key(claimer)));
        let signers: Vec<_> = signer.iter().map(|s| key(*s)).collect();
        let signers: Vec<_> = signers.iter().collect();
        let ix = Instruction::Claim { asset, amount, cooldown_ms, t: self.chain.now };
        self.chain.call(self.faucet, selectors, &signers, ix)
    }

    fn claim(&mut self, claimer: u8, asset: AccountId, amount: u128) -> anyhow::Result<u64> {
        self.claim_as(Some(claimer), claimer, asset, amount, DAY)
    }

    fn balance(&self, asset: AccountId, of: AccountId) -> u128 {
        decode_balance(&self.chain.shard(of, asset)).unwrap_or(0)
    }

    fn native(&self, of: AccountId) -> u128 {
        self.chain.state.get_account_by_id(of).data.native_balance().unwrap_or(0)
    }
}

#[test]
fn a_claim_pays_the_drop_once_per_cooldown() {
    let mut w = setup();
    let (usdc, alice) = (w.usdc, addr(&key(ALICE)));
    w.claim(ALICE, usdc, USDC_DROP).unwrap();
    assert_eq!(w.balance(usdc, alice), USDC_DROP);

    w.chain.now += DAY - 1;
    assert!(w.claim(ALICE, usdc, USDC_DROP).is_err(), "within the cooldown");
    // Another claimer, and another asset for the same claimer, have their own cooldowns.
    w.claim(BOB, usdc, USDC_DROP).unwrap();
    w.chain.now += 1;
    w.claim(ALICE, usdc, USDC_DROP).unwrap();
    assert_eq!(w.balance(usdc, alice), 2 * USDC_DROP);
    let record: Record = borsh::from_slice(&w.chain.shard(alice, w.faucet)).unwrap();
    assert_eq!(record.last_claim(&usdc), Some(w.chain.now));
    let supply = borsh::from_slice::<bosphorus_token::Config>(&w.chain.shard(bosphorus_token::config_id(&usdc), usdc));
    assert_eq!(supply.unwrap().total_supply, 3 * USDC_DROP);
}

#[test]
fn claims_must_match_the_config_and_be_signed() {
    let mut w = setup();
    let usdc = w.usdc;
    assert!(w.claim(ALICE, usdc, USDC_DROP + 1).is_err(), "more than the drop");
    assert!(w.claim(ALICE, usdc, USDC_DROP - 1).is_err(), "not the drop");
    assert!(w.claim_as(Some(ALICE), ALICE, usdc, USDC_DROP, 0).is_err(), "cooldown claimed as 0");
    assert!(w.claim_as(None, ALICE, usdc, USDC_DROP, DAY).is_err(), "unsigned");
    assert!(w.claim_as(Some(BOB), ALICE, usdc, USDC_DROP, DAY).is_err(), "signed by someone else");

    // A claimed time in the future is outside the window; one too far back is too.
    let late = Instruction::Claim { asset: usdc, amount: USDC_DROP, cooldown_ms: DAY, t: w.chain.now + 1 };
    let selectors = claim_selectors(&w.faucet, &usdc, addr(&key(ALICE)), addr(&key(ALICE)));
    assert!(w.chain.call(w.faucet, selectors.clone(), &[&key(ALICE)], late).is_err(), "t in the future");
    let early = Instruction::Claim { asset: usdc, amount: USDC_DROP, cooldown_ms: DAY, t: w.chain.now - CLAIM_WINDOW_MS };
    assert!(w.chain.call(w.faucet, selectors, &[&key(ALICE)], early).is_err(), "t past its window");
    assert_eq!(w.balance(usdc, addr(&key(ALICE))), 0);
}

#[test]
fn only_this_faucets_pda_mints() {
    let mut w = setup();
    // A second faucet on the same bytecode lists USDC, but isn't its mint authority.
    let first = w.chain.header(w.faucet).program_first_segment;
    let other = w.chain.instance(first, &key(20));
    let accounts = vec![(other, other), (config_id(&other), other)];
    let drops = vec![Drop { asset: w.usdc, amount: USDC_DROP }];
    w.chain.call(other, accounts, &[&key(20)], Instruction::Init { drops, cooldown_ms: DAY }).unwrap();
    let selectors = claim_selectors(&other, &w.usdc, addr(&key(ALICE)), addr(&key(ALICE)));
    let ix = Instruction::Claim { asset: w.usdc, amount: USDC_DROP, cooldown_ms: DAY, t: w.chain.now };
    assert!(w.chain.call(other, selectors, &[&key(ALICE)], ix).is_err(), "not the mint authority");
    // And nobody can re-init the real one.
    let accounts = vec![(w.faucet, w.faucet), (config_id(&w.faucet), w.faucet)];
    let drops = vec![Drop { asset: w.usdc, amount: u128::MAX }];
    assert!(w.chain.call(w.faucet, accounts, &[&key(10)], Instruction::Init { drops, cooldown_ms: 0 }).is_err());
}

#[test]
fn gas_comes_out_of_custody() {
    let mut w = setup();
    let custody = custody_id(&w.faucet);
    for claimer in [ALICE, BOB, 4] {
        w.claim(claimer, NATIVE, GAS_DROP).unwrap();
        assert_eq!(w.native(addr(&key(claimer))), GAS_DROP);
    }
    assert_eq!(w.native(custody), 0);
    assert!(w.claim(5, NATIVE, GAS_DROP).is_err(), "custody is empty");
    assert!(w.claim(ALICE, NATIVE, GAS_DROP).is_err(), "within the cooldown");
}

#[test]
fn a_fresh_wallet_claims_gas_privately() {
    let mut w = setup();
    // Carol's wallet: a public account with no native, and a private account.
    let everyday = addr(&key(6));
    let private = w.chain.private_account(Keys::new(7), &[]);
    let claimer = private.keys.id();
    let selectors = claim_selectors(&w.faucet, &NATIVE, claimer, everyday);
    let ix = Instruction::Claim { asset: NATIVE, amount: GAS_DROP, cooldown_ms: DAY, t: w.chain.now };
    let claim = |w: &World, ix| w.chain.prove(w.faucet, FAUCET_ELF, &[], selectors.clone(), vec![private.witness(&w.chain, true)], ix);
    let tx = claim(&w, ix.clone()).unwrap();
    w.chain.settle(&tx).unwrap();
    assert_eq!(w.native(everyday), GAS_DROP);
    // Nothing public says who claimed: the record is in the private account.
    assert!(w.chain.shard(claimer, w.faucet).is_empty());

    // An unauthorised private account can't claim (no nullifier key, no signature).
    let stranger = w.chain.private_account(Keys::new(8), &[]);
    let selectors = claim_selectors(&w.faucet, &NATIVE, stranger.keys.id(), everyday);
    let unsigned = w.chain.prove(w.faucet, FAUCET_ELF, &[], selectors, vec![stranger.witness(&w.chain, false)], ix);
    assert!(unsigned.is_err() || w.chain.settle(&unsigned.unwrap()).is_err());
    assert_eq!(w.native(everyday), GAS_DROP);
}

#[test]
fn a_private_account_claims_tokens_into_itself() {
    let mut w = setup();
    // A fresh wallet's private account, no native anywhere: the claim is a private tx (no fee).
    let mut private = w.chain.private_account(Keys::new(9), &[]);
    let id = private.keys.id();
    let selectors = claim_selectors(&w.faucet, &w.usdc, id, id);
    let ix = Instruction::Claim { asset: w.usdc, amount: USDC_DROP, cooldown_ms: DAY, t: w.chain.now };
    let tx = w.chain.prove(w.faucet, FAUCET_ELF, &[(w.usdc, TOKEN_ELF)], selectors.clone(), vec![private.witness(&w.chain, true)], ix.clone()).unwrap();
    w.chain.settle(&tx).unwrap();
    let (usdc, faucet) = (w.usdc, w.faucet);
    private.advance(&tx, |a| {
        *a = a.clone().with_shard(usdc, lee_core::native_token::encode_balance(USDC_DROP));
        *a = a.clone().with_shard(faucet, lee_core::account::ShardData::try_from(borsh::to_vec(&Record { last: vec![(usdc, w.chain.now)] }).unwrap()).unwrap());
    });
    // Nothing public holds the drop, and the second claim within a day is refused.
    assert_eq!(w.balance(usdc, id), 0);
    let again = w.chain.prove(w.faucet, FAUCET_ELF, &[(w.usdc, TOKEN_ELF)], selectors, vec![private.witness(&w.chain, true)], ix);
    assert!(again.is_err() || w.chain.settle(&again.unwrap()).is_err(), "within the cooldown");
}
