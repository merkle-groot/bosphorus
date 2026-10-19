//! Essential `token` tests (plan Phase 1): authorisation of debits and mints, PDA seeds,
//! canonical balances.

mod common;

use common::{Chain, addr, key};
use lee::AccountId;
use lee_core::native_token;
use bosphorus_methods::{TEST_CALLER_ELF, TOKEN_ELF};
use bosphorus_token::{Config, Instruction, config_id, encode_balance};

struct Token {
    chain: Chain,
    token: AccountId,
}

/// A token instance initialised with `key(1)` as mint authority, and 100 minted to alice (`key(2)`).
fn setup() -> Token {
    let mut chain = Chain::new();
    let deployer = key(9);
    let token = chain.deploy(TOKEN_ELF, &deployer);
    let init = Instruction::Init { mint_authority: addr(&key(1)), decimals: 6 };
    chain.call(token, vec![(token, token), (config_id(&token), token)], &[&deployer], init).unwrap();
    let mut t = Token { chain, token };
    t.mint(&key(1), addr(&key(2)), 100).unwrap();
    t
}

impl Token {
    fn mint(&mut self, authority: &lee::PrivateKey, to: AccountId, amount: u128) -> anyhow::Result<()> {
        let t = self.token;
        let accounts = vec![(addr(authority), t), (config_id(&t), t), (to, t)];
        self.chain.call(t, accounts, &[authority], Instruction::Mint { amount })
    }

    fn transfer(&mut self, from: AccountId, to: AccountId, signers: &[&lee::PrivateKey], amount: u128) -> anyhow::Result<()> {
        let t = self.token;
        self.chain.call(t, vec![(from, t), (to, t)], signers, Instruction::Transfer { amount })
    }

    fn balance(&self, of: AccountId) -> u128 {
        bosphorus_token::decode_balance(&self.chain.shard(of, self.token)).unwrap()
    }

    fn config(&self) -> Config {
        borsh::from_slice(&self.chain.shard(config_id(&self.token), self.token)).unwrap()
    }
}

#[test]
fn a_debit_needs_the_senders_authorisation() {
    let mut t = setup();
    let (alice, bob) = (addr(&key(2)), addr(&key(3)));
    assert!(t.transfer(alice, bob, &[], 10).is_err(), "unsigned");
    assert!(t.transfer(alice, bob, &[&key(3)], 10).is_err(), "signed by the recipient");
    t.transfer(alice, bob, &[&key(2)], 10).unwrap();
    assert_eq!((t.balance(alice), t.balance(bob)), (90, 10));
    assert!(t.transfer(alice, bob, &[&key(2)], 91).is_err(), "overdraft");
    assert_eq!((t.balance(alice), t.balance(bob)), (90, 10));
}

#[test]
fn a_pda_debit_needs_the_right_seed() {
    let mut t = setup();
    let caller = t.chain.deploy(TEST_CALLER_ELF, &key(20));
    let seed = [0x51; 32];
    let vault = AccountId::for_public_pda(&caller, &lee_core::program::PdaSeed::new(seed));
    t.mint(&key(1), vault, 50).unwrap();
    let bob = addr(&key(3));
    let token = t.token;
    let pay = |t: &mut Token, from: AccountId, seed: [u8; 32], amount: u128| {
        t.chain.call(caller, vec![(from, token), (bob, token)], &[], (token, seed, amount))
    };

    assert!(pay(&mut t, vault, [0x52; 32], 5).is_err(), "wrong seed");
    assert!(pay(&mut t, addr(&key(2)), seed, 5).is_err(), "seed of another account");
    pay(&mut t, vault, seed, 5).unwrap();
    assert_eq!((t.balance(vault), t.balance(bob)), (45, 5));

    // Another program can't move it: the PDA belongs to `caller`.
    let other = t.chain.instance(t.chain.header(caller).program_first_segment, &key(21));
    let token = t.token;
    assert!(t.chain.call(other, vec![(vault, token), (bob, token)], &[], (token, seed, 5u128)).is_err());
    assert_eq!(t.balance(vault), 45);
}

#[test]
fn minting_needs_the_authority() {
    let mut t = setup();
    let alice = addr(&key(2));
    assert!(t.mint(&key(2), alice, 1).is_err(), "signed, but not the authority");
    let unsigned = vec![(addr(&key(1)), t.token), (config_id(&t.token), t.token), (alice, t.token)];
    assert!(t.chain.call(t.token, unsigned, &[], Instruction::Mint { amount: 1 }).is_err(), "authority didn't sign");
    t.mint(&key(1), alice, 1).unwrap();
    assert_eq!(t.balance(alice), 101);
    assert_eq!(t.config().total_supply, 101);
}

#[test]
fn only_the_header_key_can_init_once() {
    let mut chain = Chain::new();
    let deployer = key(9);
    let token = chain.deploy(TOKEN_ELF, &deployer);
    let accounts = vec![(token, token), (config_id(&token), token)];
    let init = |authority: u8| Instruction::Init { mint_authority: addr(&key(authority)), decimals: 6 };

    assert!(chain.call(token, accounts.clone(), &[], init(5)).is_err(), "unsigned");
    let squatter = vec![(addr(&key(5)), token), (config_id(&token), token)];
    assert!(chain.call(token, squatter, &[&key(5)], init(5)).is_err(), "signed by someone else");
    chain.call(token, accounts.clone(), &[&deployer], init(1)).unwrap();
    assert!(chain.call(token, accounts, &[&deployer], init(5)).is_err(), "second init");
}

#[test]
fn balances_use_the_native_encoding() {
    let mut t = setup();
    let (alice, bob) = (addr(&key(2)), addr(&key(3)));
    assert_eq!(t.chain.shard(alice, t.token), encode_balance(100).into_inner());
    t.transfer(alice, bob, &[&key(2)], 100).unwrap();
    assert!(t.chain.shard(alice, t.token).is_empty(), "zero balance is an empty shard");
    assert_eq!(t.chain.shard(bob, t.token), encode_balance(100).into_inner());
    // `lending` moves native and custom tokens with the same instruction bytes.
    assert_eq!(
        borsh::to_vec(&Instruction::Transfer { amount: 7 }).unwrap(),
        borsh::to_vec(&native_token::Instruction::Transfer { amount: 7 }).unwrap(),
    );
}
