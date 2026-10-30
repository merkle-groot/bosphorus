//! `pool`: screened deposits, Privacy Pools on LEZ v0.3.1 (E10).
//!
//! A public account deposits into a per-asset escrow; the deposit's label goes, with the amount,
//! into a lot on the holder's private account (`shard[pool]`), and a public record names the
//! depositor. The ASP approves labels into a Merkle tree kept on chain. A withdrawal burns lots and
//! proves, inside the private tx, that each label is under an accepted root, so the payout names
//! no deposit. The depositor can always take a lot back publicly (`Ragequit`).
//!
//! Shared by the guest and clients: instructions with their account layouts, effects, shard
//! layouts and PDA seeds. The lot and tree logic is in `bosphorus_provenance`.

use borsh::{BorshDeserialize, BorshSerialize};
pub use bosphorus_provenance::{
    Approved, DEPTH, Hash, Label, Lot, Lots, MAX_APPROVE, MAX_LABELS, MAX_LOTS, MAX_REMOVE, MAX_ROOTS, MAX_SPENDS,
    MerklePath, REMOVED, Removal,
};
use lee_core::{
    account::{AccountId, ProgramShardSelector},
    execution_state::DeferredPublicEffect,
    native_token,
    program::{ChainedCall, PdaSeed},
};
use risc0_zkvm::sha::{Impl, Sha256 as _};

/// `[instance]` etc.: account layouts are in each variant's doc. "Signs" means `is_authorized`: a
/// signer, a private account with its `ask`, or a PDA. Accounts marked `(asset)` select the
/// asset's shard (token instance or `0x00…00`), the rest this program's.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Instruction {
    /// `[instance (this program's header, signs), config, approved]`. Only the header's key can
    /// initialise.
    Init { asp: AccountId },
    /// `[asp (signs), config, approved]`. Adds up to `MAX_APPROVE` labels to the approved set;
    /// `root` is the set's root after, which the approved list's apply computes. Older roots stay
    /// accepted.
    Approve { labels: Vec<Label>, root: Hash },
    /// `[asp (signs), config, approved]`. Removes up to `MAX_REMOVE` labels, each with its path in
    /// the tree as it stands after the ones before; only `root` stays accepted, so a removed label
    /// stops working at once.
    Remove { removals: Vec<Removal>, root: Hash },
    /// `[depositor (signs, asset), record, holder, escrow (asset)]`. Moves `amount` into the
    /// escrow, writes the public record and credits a lot to `holder` (a private account, for any
    /// privacy). The ASP admits only deposits whose depositor shows a public debit (E10).
    Deposit { asset: AccountId, amount: u128, nonce: [u8; 32] },
    /// `[holder (signs), config, escrow (asset), to (asset)]`. Burns `spends` from the holder's
    /// lots and pays their sum to `to`; each label must be under `root`, one of the accepted roots.
    Withdraw { asset: AccountId, spends: Vec<Spend>, root: Hash },
    /// `[depositor (signs, asset), record, holder (signs), escrow (asset)]`. The depositor takes
    /// back what is left of its lot, exactly `amount`, publicly and whether or not approved.
    Ragequit { asset: AccountId, label: Label, amount: u128 },
}

impl Instruction {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Init { .. } => "init",
            Self::Approve { .. } => "approve",
            Self::Remove { .. } => "remove",
            Self::Deposit { .. } => "deposit",
            Self::Withdraw { .. } => "withdraw",
            Self::Ragequit { .. } => "ragequit",
        }
    }
}

/// One lot spent by a withdrawal: never published (a private tx's instruction stays in the proof).
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Spend {
    pub label: Label,
    pub amount: u128,
    pub path: MerklePath,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Effect {
    /// Config: write once.
    Init(Config),
    /// Approved list: write once (empty).
    InitApproved,
    /// Config: the signer must be the ASP.
    PushRoot { signer: AccountId, root: Hash, flush: bool },
    /// Approved list: add, then the root must be `root`.
    Approve { labels: Vec<Label>, root: Hash },
    /// Approved list: remove, then the root must be `root`.
    Remove { removals: Vec<Removal>, root: Hash },
    /// Config (read only): `root` is accepted.
    CheckRoot { root: Hash },
    /// Record: write once.
    Record(DepositRecord),
    /// Record: the signer is its depositor, for this asset, and hasn't exited.
    Exit { depositor: AccountId, asset: AccountId },
    /// Holder: a new lot.
    Credit(Lot),
    /// Holder: `(label, amount)` out of its lots of `asset`.
    Debit { asset: AccountId, spends: Vec<(Label, u128)> },
    /// Holder: the whole lot, exactly as claimed.
    Take(Lot),
}

/// Lives in `PDA(pool, config_seed())`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Config {
    pub asp: AccountId,
    /// Accepted roots of the approved set, oldest first (`bosphorus_provenance::push_root`).
    pub roots: Vec<Hash>,
}

/// Lives in `PDA(pool, record_seed(label))`: public, so the ASP can screen the depositor.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct DepositRecord {
    pub label: Label,
    pub asset: AccountId,
    pub depositor: AccountId,
    pub amount: u128,
    /// Ragequit: the lot went back to the depositor.
    pub exited: bool,
}

/// `shard[pool]` on every pool account. The tag stops one kind passing as another. A holder's
/// empty shard means no lots.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Shard {
    Config(Config),
    Approved(Approved),
    Record(DepositRecord),
    Lots(Lots),
}

// ---- seeds and accounts ----------------------------------------------------------------------

pub fn sha256(bytes: &[u8]) -> Hash {
    Impl::hash_bytes(bytes).as_bytes().try_into().expect("sha256 is 32 bytes")
}

fn seed(domain: &[u8], parts: &[&[u8]]) -> PdaSeed {
    PdaSeed::new(sha256(&[domain, &parts.concat()].concat()))
}

pub fn config_seed() -> PdaSeed {
    seed(b"/bosphorus/v1/pool/config", &[])
}

pub fn approved_seed() -> PdaSeed {
    seed(b"/bosphorus/v1/pool/approved", &[])
}

pub fn escrow_seed(asset: &AccountId) -> PdaSeed {
    seed(b"/bosphorus/v1/pool/escrow/", &[asset.as_ref()])
}

pub fn record_seed(label: &Label) -> PdaSeed {
    seed(b"/bosphorus/v1/pool/record/", &[label])
}

/// `SHA256("/bosphorus/v1/pool/label/" ‖ pool ‖ asset ‖ depositor ‖ nonce)`.
pub fn deposit_label(pool: &AccountId, asset: &AccountId, depositor: &AccountId, nonce: &[u8; 32]) -> Label {
    sha256(&[b"/bosphorus/v1/pool/label/".as_slice(), pool.as_ref(), asset.as_ref(), depositor.as_ref(), nonce].concat())
}

pub fn config_id(pool: &AccountId) -> AccountId {
    AccountId::for_public_pda(pool, &config_seed())
}

pub fn approved_id(pool: &AccountId) -> AccountId {
    AccountId::for_public_pda(pool, &approved_seed())
}

pub fn escrow_id(pool: &AccountId, asset: &AccountId) -> AccountId {
    AccountId::for_public_pda(pool, &escrow_seed(asset))
}

pub fn record_id(pool: &AccountId, label: &Label) -> AccountId {
    AccountId::for_public_pda(pool, &record_seed(label))
}

/// A chained `Transfer` of `asset` (native or a token instance: same instruction bytes). `seed`
/// authorises a debit from one of the caller's PDAs.
pub fn transfer(asset: AccountId, from: AccountId, to: AccountId, amount: u128, seed: Option<PdaSeed>) -> ChainedCall {
    let call = ChainedCall::new(
        asset,
        vec![ProgramShardSelector::new(from, asset), ProgramShardSelector::new(to, asset)],
        &native_token::Instruction::Transfer { amount },
    );
    match seed {
        Some(seed) => call.with_pda_seeds(vec![seed]),
        None => call,
    }
}

/// A deposit found in a settled private tx, and whether the ASP may screen it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admission {
    pub record: AccountId,
    pub deposit: DepositRecord,
    /// `Err` says why the deposit can't be screened; the ASP never approves it (Ragequit stays open).
    pub admitted: Result<(), &'static str>,
}

/// E10's admission rule, over a private tx's public actions `(account, effects)` and its public
/// signers. The program can't tell a public account from a private one (`AccountMeta` has no
/// flag), so only the published tx shows where a deposit came from: its depositor must have
/// signed and show a public debit of exactly the amount, and the escrow the matching credit. A
/// deposit from a private account debits privately, so it isn't admitted.
pub fn admissions(pool: &AccountId, actions: &[(AccountId, &[DeferredPublicEffect])], signers: &[AccountId]) -> Vec<Admission> {
    let shows = |account: AccountId, asset: AccountId, expected: native_token::Effect| {
        actions.iter().filter(|(id, _)| *id == account).flat_map(|(_, effects)| effects.iter()).any(|e| {
            e.program_account_id == asset
                && e.shard_program_account_id == asset
                && borsh::from_slice::<native_token::Effect>(&e.data).is_ok_and(|effect| effect == expected)
        })
    };
    let mut found = vec![];
    for (account, effects) in actions {
        for effect in effects.iter().filter(|e| e.program_account_id == *pool && e.shard_program_account_id == *pool) {
            let Ok(Effect::Record(deposit)) = borsh::from_slice(&effect.data) else { continue };
            let admitted = if !signers.contains(&deposit.depositor) {
                Err("the depositor didn't sign: not a public account")
            } else if !shows(deposit.depositor, deposit.asset, native_token::Effect::Debit(deposit.amount)) {
                Err("no public debit from the depositor: a private source")
            } else if !shows(escrow_id(pool, &deposit.asset), deposit.asset, native_token::Effect::Credit(deposit.amount)) {
                Err("the escrow wasn't credited")
            } else {
                Ok(())
            };
            found.push(Admission { record: *account, deposit, admitted });
        }
    }
    found
}

/// A holder's lots (an empty shard holds none).
pub fn decode_lots(data: &[u8]) -> Result<Lots, String> {
    if data.is_empty() {
        return Ok(Lots::default());
    }
    match borsh::from_slice(data) {
        Ok(Shard::Lots(lots)) => Ok(lots),
        Ok(_) => Err("not a holder's lots".into()),
        Err(e) => Err(format!("not a pool account: {e}")),
    }
}

/// Canonical: no lots is an empty shard.
pub fn encode_lots(lots: Lots) -> Vec<u8> {
    if lots.0.is_empty() { vec![] } else { borsh::to_vec(&Shard::Lots(lots)).expect("borsh serialization is infallible") }
}
