//! Essential pool tests (E10: screened deposits, Privacy Pools on LEZ), in-process with the real
//! guests. Money safety: a lot pays out at most its deposit, only on its holder's order, only
//! under an accepted root, and only its depositor takes it back. Privacy: a withdrawal publishes
//! no label, record or holder. Admission: a deposit from a private account isn't screenable.
//!
//! Dev-mode proofs: `RISC0_DEV_MODE=1 cargo test -p bosphorus-methods --release --test pool`.
//! Real proofs, timed (GPU): `cargo test -p bosphorus-methods --release --features prove --test pool
//! -- --ignored --nocapture real_proofs_pool`.

mod common;

use std::time::Instant;

use bosphorus_methods::{POOL_ELF, TOKEN_ELF};
use bosphorus_pool::{
    Admission, Approved, DepositRecord, Instruction, Label, Lot, Lots, MAX_APPROVE, Removal, Shard, Spend, admissions,
    approved_id, config_id, decode_lots, deposit_label, encode_lots, escrow_id, record_id, sha256,
};
use common::{
    Chain, addr, key,
    private::{Keys, PrivateAccount},
};
use lee::{AccountId, PrivacyPreservingTransaction};
use lee_core::{NullifierWitness, native_token::encode_balance};

const MINTER: u8 = 1;
const ALICE: u8 = 2;
const MALLORY: u8 = 4;
const ASP: u8 = 21;
const RECIPIENT: u8 = 30;

struct World {
    chain: Chain,
    token: AccountId,
    pool: AccountId,
    /// Holds every lot here; starts with 5,000 TKN of its own (a private source).
    holder: PrivateAccount,
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// A token minted to alice and mallory, and a pool whose ASP is `ASP`.
fn world() -> World {
    assert!(
        std::env::var("RISC0_DEV_MODE").is_ok_and(|v| v == "1" || v == "true"),
        "run with RISC0_DEV_MODE=1 (real proofs: `--ignored real_proofs_pool`)"
    );
    setup()
}

fn setup() -> World {
    let mut chain = Chain::new();
    let token = chain.deploy(TOKEN_ELF, &key(10));
    let config = bosphorus_token::config_id(&token);
    let init = bosphorus_token::Instruction::Init { mint_authority: addr(&key(MINTER)), decimals: 6 };
    chain.call(token, vec![(token, token), (config, token)], &[&key(10)], init).unwrap();
    for holder in [ALICE, MALLORY] {
        let accounts = vec![(addr(&key(MINTER)), token), (config, token), (addr(&key(holder)), token)];
        chain.call(token, accounts, &[&key(MINTER)], bosphorus_token::Instruction::Mint { amount: 1_000_000 }).unwrap();
    }
    let pool = chain.deploy(POOL_ELF, &key(20));
    let accounts = vec![(pool, pool), (config_id(&pool), pool), (approved_id(&pool), pool)];
    chain.call(pool, accounts, &[&key(20)], Instruction::Init { asp: addr(&key(ASP)) }).unwrap();
    let holder = chain.private_account(Keys::new(60), &[(token, encode_balance(5_000))]);
    World { chain, token, pool, holder }
}

/// The approved set holding `labels`, added in this order.
fn set_of(labels: &[Label]) -> Approved {
    let mut set = Approved::default();
    set.approve(sha256, labels).unwrap();
    set
}

/// What the ASP reads from a settled deposit tx.
fn admitted(pool: &AccountId, tx: &PrivacyPreservingTransaction) -> Vec<Admission> {
    let actions: Vec<_> = tx.message.public_actions.iter().map(|a| (a.account_id, a.effects.as_slice())).collect();
    let signers: Vec<_> = tx.witness_set.signatures_and_public_keys().iter().map(|(_, pk)| AccountId::from(pk)).collect();
    admissions(pool, &actions, &signers)
}

impl World {
    fn balance(&self, of: AccountId) -> u128 {
        bosphorus_token::decode_balance(&self.chain.shard(of, self.token)).unwrap()
    }

    fn escrow(&self) -> AccountId {
        escrow_id(&self.pool, &self.token)
    }

    fn lots(&self) -> Lots {
        decode_lots(self.holder.account.data.shard(self.pool)).unwrap()
    }

    fn record(&self, label: &Label) -> DepositRecord {
        match borsh::from_slice(&self.chain.shard(record_id(&self.pool, label), self.pool)).unwrap() {
            Shard::Record(record) => record,
            other => panic!("{other:?}"),
        }
    }

    fn approved(&self) -> Approved {
        match borsh::from_slice(&self.chain.shard(approved_id(&self.pool), self.pool)).unwrap() {
            Shard::Approved(approved) => approved,
            other => panic!("{other:?}"),
        }
    }

    /// The holder's next version, with these lots (and `token` balance, if given).
    fn advance(&mut self, tx: &PrivacyPreservingTransaction, lots: Lots, token: Option<u128>) {
        let (pool, t) = (self.pool, self.token);
        self.holder.advance(tx, |a| {
            a.data.set_shard(pool, encode_lots(lots).try_into().unwrap());
            if let Some(balance) = token {
                a.data.set_shard(t, encode_balance(balance));
            }
        });
    }

    /// A public account deposits `amount` into the holder.
    fn deposit(&mut self, from: u8, amount: u128, nonce: u8) -> (Label, PrivacyPreservingTransaction) {
        let (pool, token, depositor) = (self.pool, self.token, addr(&key(from)));
        let label = deposit_label(&pool, &token, &depositor, &[nonce; 32]);
        let selectors =
            vec![(depositor, token), (record_id(&pool, &label), pool), (self.holder.keys.id(), pool), (self.escrow(), token)];
        let ix = Instruction::Deposit { asset: token, amount, nonce: [nonce; 32] };
        let witness = self.holder.witness(&self.chain, true);
        let tx = self.chain.prove_signed(pool, POOL_ELF, &[(token, TOKEN_ELF)], selectors, &[&key(from)], vec![witness], ix).unwrap();
        self.chain.settle(&tx).unwrap();
        let mut lots = self.lots();
        lots.credit(Lot { asset: *token.value(), label, amount }).unwrap();
        self.advance(&tx, lots, None);
        (label, tx)
    }

    fn asp_call(&mut self, ix: Instruction, signer: u8) -> anyhow::Result<u64> {
        let pool = self.pool;
        let accounts = vec![(addr(&key(signer)), pool), (config_id(&pool), pool), (approved_id(&pool), pool)];
        self.chain.call(pool, accounts, &[&key(signer)], ix)
    }

    /// Returns the cycles the approval took.
    fn approve(&mut self, labels: &[Label]) -> u64 {
        let mut set = self.approved();
        set.approve(sha256, labels).unwrap();
        self.asp_call(Instruction::Approve { labels: labels.to_vec(), root: set.root }, ASP).unwrap()
    }

    fn remove(&mut self, labels: &[Label]) -> u64 {
        let mut set = self.approved();
        let mut removals = vec![];
        for label in labels {
            let removal = Removal { label: *label, path: set.path(sha256, set.index_of(label).unwrap()).unwrap() };
            set.remove(sha256, &removal).unwrap();
            removals.push(removal);
        }
        self.asp_call(Instruction::Remove { removals, root: set.root }, ASP).unwrap()
    }

    /// Spends lots under the root of `set` (a label outside it gets the path of its first leaf).
    fn withdraw_ix(&self, spends: &[(Label, u128)], set: &Approved) -> Instruction {
        let spends = spends
            .iter()
            .map(|(label, amount)| {
                let index = set.index_of(label).unwrap_or(0);
                Spend { label: *label, amount: *amount, path: set.path(sha256, index).unwrap() }
            })
            .collect();
        Instruction::Withdraw { asset: self.token, spends, root: set.root }
    }

    fn prove_withdraw(&self, ix: Instruction, to: AccountId, ask: bool) -> anyhow::Result<PrivacyPreservingTransaction> {
        let (pool, token) = (self.pool, self.token);
        let selectors = vec![(self.holder.keys.id(), pool), (config_id(&pool), pool), (self.escrow(), token), (to, token)];
        let witness = self.holder.witness(&self.chain, ask);
        self.chain.prove(pool, POOL_ELF, &[(token, TOKEN_ELF)], selectors, vec![witness], ix)
    }

    /// Withdraws under the root of `set`, then checks nothing published links it to the deposits.
    fn withdraw_under(&mut self, spends: &[(Label, u128)], set: &Approved, to: AccountId) -> PrivacyPreservingTransaction {
        let tx = self.prove_withdraw(self.withdraw_ix(spends, set), to, true).unwrap();
        self.chain.settle(&tx).unwrap();
        let mut lots = self.lots();
        for (label, amount) in spends {
            lots.debit(self.token.value(), label, *amount).unwrap();
        }
        self.advance(&tx, lots, None);
        self.assert_unlinked(&tx, &spends.iter().map(|(l, _)| *l).collect::<Vec<_>>());
        tx
    }

    fn withdraw(&mut self, spends: &[(Label, u128)], to: AccountId) -> PrivacyPreservingTransaction {
        self.withdraw_under(spends, &self.approved(), to)
    }

    fn prove_ragequit(&self, from: u8, label: Label, amount: u128) -> anyhow::Result<PrivacyPreservingTransaction> {
        let (pool, token, depositor) = (self.pool, self.token, addr(&key(from)));
        let selectors =
            vec![(depositor, token), (record_id(&pool, &label), pool), (self.holder.keys.id(), pool), (self.escrow(), token)];
        let witness = self.holder.witness(&self.chain, true);
        let ix = Instruction::Ragequit { asset: token, label, amount };
        self.chain.prove_signed(pool, POOL_ELF, &[(token, TOKEN_ELF)], selectors, &[&key(from)], vec![witness], ix)
    }

    /// The byte scan: the serialised tx and every public account it touched hold nothing that
    /// names the holder, nor any of `labels` or their records.
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

/// Alice deposits publicly into a private holder; once approved, the holder withdraws in two
/// parts to a fresh public account. The deposit is admitted; neither withdrawal names it.
#[test]
fn an_approved_deposit_withdraws_unlinked() {
    let mut w = world();
    let (label, tx) = w.deposit(ALICE, 1_000, 1);
    w.assert_unlinked(&tx, &[]);
    let expected = DepositRecord { label, asset: w.token, depositor: addr(&key(ALICE)), amount: 1_000, exited: false };
    assert_eq!(w.record(&label), expected);
    assert_eq!(w.balance(w.escrow()), 1_000);
    let found = admitted(&w.pool, &tx);
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].record, &found[0].deposit, found[0].admitted), (record_id(&w.pool, &label), &expected, Ok(())));

    w.approve(&[label]);
    let to = addr(&key(RECIPIENT));
    w.withdraw(&[(label, 600)], to);
    assert_eq!((w.balance(to), w.balance(w.escrow())), (600, 400));
    w.withdraw(&[(label, 400)], to);
    assert_eq!((w.balance(to), w.balance(w.escrow())), (1_000, 0));
    assert_eq!(w.lots(), Lots::default());
}

/// A withdrawal needs every label under a root the config accepts: never before an approval, never
/// with an unapproved lot mixed in, from an older root only until a removal, and never a removed label.
#[test]
fn only_approved_labels_under_accepted_roots_withdraw() {
    let mut w = world();
    let (a, _) = w.deposit(ALICE, 1_000, 1);
    let (b, _) = w.deposit(MALLORY, 500, 2);
    let (c, _) = w.deposit(ALICE, 300, 3);
    let to = addr(&key(RECIPIENT));

    let tx = w.prove_withdraw(w.withdraw_ix(&[(a, 100)], &set_of(&[a])), to, true).unwrap();
    assert!(w.chain.settle(&tx).is_err(), "withdrew under a root the ASP never published");
    assert!(w.asp_call(Instruction::Approve { labels: vec![a], root: set_of(&[a]).root }, MALLORY).is_err(), "not the ASP");
    assert!(w.asp_call(Instruction::Approve { labels: vec![a], root: set_of(&[b]).root }, ASP).is_err(), "root not the list's");

    w.approve(&[a]);
    for spends in [vec![(b, 100)], vec![(a, 100), (b, 100)]] {
        assert!(w.prove_withdraw(w.withdraw_ix(&spends, &set_of(&[a])), to, true).is_err(), "b has no path under the root");
    }
    // Roots now [a], [a, b]: the older one still works (approvals only add labels).
    w.approve(&[b]);
    w.withdraw_under(&[(a, 100)], &set_of(&[a]), to);
    // Roots [a], [a, b], [a, b, c]; removing c leaves only [a, b, (c removed)].
    w.approve(&[c]);
    w.remove(&[c]);
    let tx = w.prove_withdraw(w.withdraw_ix(&[(a, 100)], &set_of(&[a])), to, true).unwrap();
    assert!(w.chain.settle(&tx).is_err(), "an older root survived a removal");
    let tx = w.prove_withdraw(w.withdraw_ix(&[(c, 100)], &set_of(&[a, b, c])), to, true).unwrap();
    assert!(w.chain.settle(&tx).is_err(), "a removed label withdrew");
    w.withdraw(&[(a, 900), (b, 500)], to);
    assert_eq!(w.balance(to), 1_500);
}

/// A lot pays out at most what was deposited, only with its holder's authorisation, and a forged
/// plaintext doesn't settle.
#[test]
fn lots_pay_out_at_most_their_deposit() {
    let mut w = world();
    let (a, _) = w.deposit(ALICE, 1_000, 1);
    let (b, _) = w.deposit(MALLORY, 1_000, 2);
    w.approve(&[a, b]);
    let (set, to) = (w.approved(), addr(&key(RECIPIENT)));
    for spends in [vec![(a, 1_001)], vec![(a, 600), (a, 401)]] {
        assert!(w.prove_withdraw(w.withdraw_ix(&spends, &set), to, true).is_err(), "more than the lot: {spends:?}");
    }
    assert!(w.prove_withdraw(w.withdraw_ix(&[(a, 1)], &set), to, false).is_err(), "holder didn't authorise");

    // Claiming 2,000 in lot a (the escrow holds 2,000): the proof binds a commitment that isn't on
    // chain ("Unrecognized commitment set digest").
    let (pool, token) = (w.pool, w.token);
    let mut forged = w.holder.witness(&w.chain, true);
    let NullifierWitness::Update { account, .. } = &mut forged.nullifier else { unreachable!() };
    let lots = Lots(vec![Lot { asset: *token.value(), label: a, amount: 2_000 }, Lot { asset: *token.value(), label: b, amount: 1_000 }]);
    account.data.set_shard(pool, encode_lots(lots).try_into().unwrap());
    let selectors = vec![(w.holder.keys.id(), pool), (config_id(&pool), pool), (w.escrow(), token), (to, token)];
    let ix = w.withdraw_ix(&[(a, 2_000)], &set);
    let settled = w.chain.prove(pool, POOL_ELF, &[(token, TOKEN_ELF)], selectors, vec![forged], ix).and_then(|tx| w.chain.settle(&tx));
    assert!(settled.is_err(), "an inflated lot was withdrawn");

    w.withdraw(&[(a, 1_000), (b, 1_000)], to);
    assert_eq!((w.balance(to), w.balance(w.escrow())), (2_000, 0));
}

/// Ragequit: only the depositor takes a lot back, exactly what is left, once; after that the lot
/// can't be withdrawn, approved or not.
#[test]
fn only_the_depositor_takes_a_lot_back() {
    let mut w = world();
    let (a, _) = w.deposit(ALICE, 1_000, 1);
    w.approve(&[a]);
    let to = addr(&key(RECIPIENT));
    w.withdraw(&[(a, 300)], to);

    let tx = w.prove_ragequit(MALLORY, a, 700).unwrap();
    assert!(w.chain.settle(&tx).is_err(), "mallory isn't the depositor (checked on the public record)");
    assert!(w.prove_ragequit(ALICE, a, 1_000).is_err(), "more than is left");
    let before = w.balance(addr(&key(ALICE)));
    let tx = w.prove_ragequit(ALICE, a, 700).unwrap();
    w.chain.settle(&tx).unwrap();
    w.advance(&tx, Lots::default(), None);
    assert_eq!(w.balance(addr(&key(ALICE))), before + 700);
    assert!(w.record(&a).exited);
    assert_eq!(w.balance(w.escrow()), 0);

    assert!(w.prove_ragequit(ALICE, a, 700).is_err(), "the lot is gone");
    assert!(w.prove_withdraw(w.withdraw_ix(&[(a, 1)], &w.approved()), to, true).is_err(), "withdrew a ragequit lot");
}

/// A deposit from a private account lands but isn't admitted (no public debit shows where it came
/// from); the depositor takes it back.
#[test]
fn a_private_source_isnt_admitted() {
    let mut w = world();
    let (pool, token, holder) = (w.pool, w.token, w.holder.keys.id());
    let label = deposit_label(&pool, &token, &holder, &[9; 32]);
    let selectors = vec![(holder, token), (record_id(&pool, &label), pool), (holder, pool), (w.escrow(), token)];
    let ix = Instruction::Deposit { asset: token, amount: 2_000, nonce: [9; 32] };
    let tx = w.chain.prove(pool, POOL_ELF, &[(token, TOKEN_ELF)], selectors.clone(), vec![w.holder.witness(&w.chain, true)], ix).unwrap();
    w.chain.settle(&tx).unwrap();
    let found = admitted(&pool, &tx);
    assert_eq!(found.len(), 1);
    assert!(found[0].admitted.is_err(), "a private source was admitted");
    w.advance(&tx, Lots(vec![Lot { asset: *token.value(), label, amount: 2_000 }]), Some(3_000));

    let ix = Instruction::Ragequit { asset: token, label, amount: 2_000 };
    let tx = w.chain.prove(pool, POOL_ELF, &[(token, TOKEN_ELF)], selectors, vec![w.holder.witness(&w.chain, true)], ix).unwrap();
    w.chain.settle(&tx).unwrap();
    w.advance(&tx, Lots::default(), Some(5_000));
    assert_eq!(w.balance(w.escrow()), 0);
}

/// E10's cost: proving time and tx size of a deposit, and of a withdrawal spending one lot and two
/// (Merkle paths of depth 11, a full approved set's), with real proofs.
#[test]
#[ignore = "real proofs: about a minute per op on a GPU"]
fn real_proofs_pool() {
    assert!(std::env::var("RISC0_DEV_MODE").is_err(), "unset RISC0_DEV_MODE for real proofs");
    let mut w = setup();
    let mut timings = vec![];
    let mut timed = |name: &str, f: &mut dyn FnMut() -> PrivacyPreservingTransaction| {
        let started = Instant::now();
        let tx = f();
        timings.push((name.to_string(), started.elapsed(), borsh::to_vec(&tx).unwrap().len()));
    };
    let mut a = [0; 32];
    timed("deposit", &mut || {
        let (label, tx) = w.deposit(ALICE, 1_000, 1);
        a = label;
        tx
    });
    let b = w.deposit(MALLORY, 1_000, 2).0;
    // 2,046 more labels, MAX_APPROVE per tx: the set ends at MAX_LABELS (paths have the full depth
    // either way: the tree's depth is fixed).
    let filler: Vec<Label> = (0u16..2046).map(|i| { let mut l = [0xF0; 32]; l[..2].copy_from_slice(&i.to_le_bytes()); l }).collect();
    let mut costs = vec![];
    for chunk in filler.chunks(MAX_APPROVE) {
        let cycles = w.approve(chunk);
        if [MAX_APPROVE, 1024, 2046].contains(&w.approved().leaves.len()) {
            costs.push((format!("approve {} (set after: {})", chunk.len(), w.approved().leaves.len()), cycles));
        }
    }
    costs.push(("approve 2 (set after: 2048)".into(), w.approve(&[a, b])));
    costs.push(("remove 1 (set of 2048)".into(), w.remove(&filler[100..101])));
    costs.push(("remove 8 (set of 2048)".into(), w.remove(&filler[200..208])));
    let to = addr(&key(RECIPIENT));
    timed("withdraw (1 lot)", &mut || w.withdraw(&[(a, 600)], to));
    timed("withdraw (2 lots)", &mut || w.withdraw(&[(a, 400), (b, 1_000)], to));
    assert_eq!(w.balance(to), 2_000);
    println!("\n| pool op | proving + settling | tx bytes |\n|---|---:|---:|");
    for (op, time, bytes) in &timings {
        println!("| {op} | {:.1} s | {bytes} |", time.as_secs_f64());
    }
    println!("\n| ASP op | cycles (gas) |\n|---|---:|");
    for (op, cycles) in &costs {
        println!("| {op} | {cycles} |");
    }
}
