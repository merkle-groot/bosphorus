//! Spike (screened markets, 2026-11-05): one private tx spends pool lots and credits a position,
//! with lending's shape (`screened_supply`, a test-only guest, chains the pool's `Withdraw` into
//! its vault). Questions: does the holder's `ask` reach the chained pool call; can one private
//! account have `shard[pool]` and another program's shard rewritten in one tx; does the pool still
//! refuse unapproved lots; what does the tx publish.
//!
//! `RISC0_DEV_MODE=1 cargo test -p bosphorus-methods --release --test screened_spike`.

mod common;

use bosphorus_methods::{POOL_ELF, SCREENED_SUPPLY_ELF, TOKEN_ELF};
use bosphorus_pool::{
    Approved, Instruction, Label, Lot, Lots, Shard, Spend, approved_id, config_id, decode_lots, deposit_label,
    encode_lots, escrow_id, record_id, sha256,
};
use common::{
    Chain, addr, key,
    private::{Keys, PrivateAccount},
};
use lee::{AccountId, PrivacyPreservingTransaction};
use lee_core::program::PdaSeed;

const MINTER: u8 = 1;
const ALICE: u8 = 2;
const ASP: u8 = 21;
/// `screened_supply`'s vault seed.
const VAULT_SEED: PdaSeed = PdaSeed::new([0x5C; 32]);

struct World {
    chain: Chain,
    token: AccountId,
    pool: AccountId,
    market: AccountId,
    holder: PrivateAccount,
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn world() -> World {
    assert!(std::env::var("RISC0_DEV_MODE").is_ok_and(|v| v == "1" || v == "true"), "run with RISC0_DEV_MODE=1");
    let mut chain = Chain::new();
    let token = chain.deploy(TOKEN_ELF, &key(10));
    let config = bosphorus_token::config_id(&token);
    let init = bosphorus_token::Instruction::Init { mint_authority: addr(&key(MINTER)), decimals: 6 };
    chain.call(token, vec![(token, token), (config, token)], &[&key(10)], init).unwrap();
    let accounts = vec![(addr(&key(MINTER)), token), (config, token), (addr(&key(ALICE)), token)];
    chain.call(token, accounts, &[&key(MINTER)], bosphorus_token::Instruction::Mint { amount: 1_000_000 }).unwrap();
    let pool = chain.deploy(POOL_ELF, &key(20));
    let accounts = vec![(pool, pool), (config_id(&pool), pool), (approved_id(&pool), pool)];
    chain.call(pool, accounts, &[&key(20)], Instruction::Init { asp: addr(&key(ASP)) }).unwrap();
    let market = chain.deploy(SCREENED_SUPPLY_ELF, &key(30));
    let holder = chain.private_account(Keys::new(60), &[]);
    World { chain, token, pool, market, holder }
}

impl World {
    fn balance(&self, of: AccountId) -> u128 {
        bosphorus_token::decode_balance(&self.chain.shard(of, self.token)).unwrap()
    }

    fn escrow(&self) -> AccountId {
        escrow_id(&self.pool, &self.token)
    }

    fn vault(&self) -> AccountId {
        AccountId::for_public_pda(&self.market, &VAULT_SEED)
    }

    fn lots(&self) -> Lots {
        decode_lots(self.holder.account.data.shard(self.pool)).unwrap()
    }

    fn approved(&self) -> Approved {
        match borsh::from_slice(&self.chain.shard(approved_id(&self.pool), self.pool)).unwrap() {
            Shard::Approved(approved) => approved,
            other => panic!("{other:?}"),
        }
    }

    /// Alice deposits `amount` publicly into the holder's lots.
    fn deposit(&mut self, amount: u128, nonce: u8) -> Label {
        let (pool, token, depositor) = (self.pool, self.token, addr(&key(ALICE)));
        let label = deposit_label(&pool, &token, &depositor, &[nonce; 32]);
        let selectors =
            vec![(depositor, token), (record_id(&pool, &label), pool), (self.holder.keys.id(), pool), (self.escrow(), token)];
        let ix = Instruction::Deposit { asset: token, amount, nonce: [nonce; 32] };
        let witness = self.holder.witness(&self.chain, true);
        let tx = self.chain.prove_signed(pool, POOL_ELF, &[(token, TOKEN_ELF)], selectors, &[&key(ALICE)], vec![witness], ix).unwrap();
        self.chain.settle(&tx).unwrap();
        let mut lots = self.lots();
        lots.credit(Lot { asset: *token.value(), label, amount }).unwrap();
        self.holder.advance(&tx, |a| a.data.set_shard(pool, encode_lots(lots).try_into().unwrap()));
        label
    }

    fn approve(&mut self, labels: &[Label]) {
        let pool = self.pool;
        let mut set = self.approved();
        set.approve(sha256, labels).unwrap();
        let accounts = vec![(addr(&key(ASP)), pool), (config_id(&pool), pool), (approved_id(&pool), pool)];
        self.chain.call(pool, accounts, &[&key(ASP)], Instruction::Approve { labels: labels.to_vec(), root: set.root }).unwrap();
    }

    /// The screened supply: `spends` out of the holder's lots, credited to `position`.
    fn prove_supply(&self, spends: &[(Label, u128)], position: AccountId, ask: bool) -> anyhow::Result<PrivacyPreservingTransaction> {
        let (pool, token, market) = (self.pool, self.token, self.market);
        let set = self.approved();
        let spends: Vec<Spend> = spends
            .iter()
            .map(|(label, amount)| Spend { label: *label, amount: *amount, path: Some(set.path(sha256, set.index_of(label).unwrap_or(0)).unwrap()) })
            .collect();
        let selectors = vec![
            (self.holder.keys.id(), pool),
            (position, market),
            (config_id(&pool), pool),
            (self.escrow(), token),
            (self.vault(), token),
        ];
        let witness = self.holder.witness(&self.chain, ask);
        let ix = (pool, token, spends, set.root);
        self.chain.prove(market, SCREENED_SUPPLY_ELF, &[(pool, POOL_ELF), (token, TOKEN_ELF)], selectors, vec![witness], ix)
    }

    /// Nothing published names the holder or the spent labels.
    fn assert_unlinked(&self, tx: &PrivacyPreservingTransaction, labels: &[Label]) {
        let mut published = vec![("tx".to_string(), borsh::to_vec(tx).unwrap())];
        for action in &tx.message.public_actions {
            let account = self.chain.state.get_account_by_id(action.account_id);
            published.push((format!("public account {}", action.account_id), borsh::to_vec(&account).unwrap()));
        }
        let mut secrets = self.holder.keys.identifying_bytes();
        for label in labels {
            secrets.push(("label", label.to_vec()));
            secrets.push(("record", record_id(&self.pool, label).as_ref().to_vec()));
        }
        for (what, data) in &published {
            for (secret, needle) in &secrets {
                assert!(!contains(data, needle), "{secret} found in {what}");
            }
        }
    }
}

fn held(w: &World, of: AccountId, account: Option<&PrivateAccount>) -> u128 {
    let data = match account {
        Some(private) => private.account.data.shard(w.market).clone().into_inner(),
        None => w.chain.shard(of, w.market),
    };
    if data.is_empty() { 0 } else { u128::from_le_bytes(data.try_into().unwrap()) }
}

/// Lend: two approved lots pay a supply held on the holder's own account. `shard[pool]` loses the
/// spent amounts and `shard[market]` gains their sum, in one private tx; the vault is paid
/// straight from the escrow.
#[test]
fn approved_lots_pay_a_held_supply() {
    let mut w = world();
    let (a, b) = (w.deposit(1_000, 1), w.deposit(500, 2));
    w.approve(&[a, b]);
    let holder = w.holder.keys.id();
    let tx = w.prove_supply(&[(a, 600), (b, 500)], holder, true).unwrap();
    w.chain.settle(&tx).unwrap();
    let (pool, market) = (w.pool, w.market);
    let mut lots = w.lots();
    lots.debit(w.token.value(), &a, 600).unwrap();
    lots.debit(w.token.value(), &b, 500).unwrap();
    w.holder.advance(&tx, |acc| {
        acc.data.set_shard(pool, encode_lots(lots).try_into().unwrap());
        acc.data.set_shard(market, 1_100u128.to_le_bytes().to_vec().try_into().unwrap());
    });
    assert_eq!((w.balance(w.vault()), w.balance(w.escrow())), (1_100, 400));
    assert_eq!(held(&w, holder, Some(&w.holder)), 1_100);
    w.assert_unlinked(&tx, &[a, b]);
    let public: Vec<_> = tx.message.public_actions.iter().map(|a| a.account_id).collect();
    let mut expected = vec![config_id(&w.pool), w.escrow(), w.vault()];
    expected.sort();
    let mut got = public.clone();
    got.sort();
    got.dedup();
    assert_eq!(got, expected, "only the pool config, the escrow and the vault are public");
    let size = borsh::to_vec(&tx).unwrap().len();
    println!("screened held supply: {} public actions, {} commitments, tx {size} B", public.len(), tx.message.commitments().len());
}

/// Collateral: the lots pay into a public position (a PDA of the market), as collateral would.
#[test]
fn approved_lots_pay_a_public_position() {
    let mut w = world();
    let a = w.deposit(1_000, 1);
    w.approve(&[a]);
    let position = AccountId::for_public_pda(&w.market, &PdaSeed::new([0x77; 32]));
    let tx = w.prove_supply(&[(a, 1_000)], position, true).unwrap();
    w.chain.settle(&tx).unwrap();
    assert_eq!(held(&w, position, None), 1_000);
    assert_eq!((w.balance(w.vault()), w.balance(w.escrow())), (1_000, 0));
    w.assert_unlinked(&tx, &[a]);
}

/// The pool's checks hold through the chained call: no unapproved lot, no more than the lot, not
/// without the holder's `ask`.
#[test]
fn the_pool_still_refuses() {
    let mut w = world();
    let (a, b) = (w.deposit(1_000, 1), w.deposit(500, 2));
    w.approve(&[a]);
    let holder = w.holder.keys.id();
    assert!(w.prove_supply(&[(b, 500)], holder, true).is_err(), "an unapproved lot paid");
    assert!(w.prove_supply(&[(a, 600), (b, 1)], holder, true).is_err(), "an unapproved lot was mixed in");
    assert!(w.prove_supply(&[(a, 1_001)], holder, true).is_err(), "more than the lot paid");
    assert!(w.prove_supply(&[(a, 600)], holder, false).is_err(), "paid without the holder's ask");
    let tx = w.prove_supply(&[(a, 600)], holder, true).unwrap();
    w.chain.settle(&tx).unwrap();
    assert_eq!(w.balance(w.vault()), 600);
}
