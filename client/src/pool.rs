//! The screened pool (E10, Privacy Pools on LEZ): deposits from a public account into one of this
//! wallet's private accounts, withdrawals that prove a deposit's label approved without naming it,
//! ragequit, the ASP's approvals, and the ASP's scan of deposits with their admission.
//!
//! Like transfers, each `send_pool_*` returns once the tx is handed over ([`TransferSent`]); the
//! caller waits for it and calls [`Client::finish_transfer`] (or [`Client::land`] does both).

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use bosphorus_pool::{Removal, Shard, Spend, approved_id, config_id, decode_lots, deposit_label, escrow_id, record_id, sha256};
use common::{HashType, transaction::LeeTransaction};
use lee::{AccountId, ProgramShardSelector, native_token::NATIVE_TOKEN_PROGRAM_ID, program::Program};
use sequencer_service_rpc::RpcClient as _;
use sha2::{Digest as _, Sha256};
use wallet::{AccountIdentity, AccountMention};

use crate::{Client, Holder, POOL_ELF, TOKEN_ELF, funds::TransferSent, no_sign, now_ms, send_error};

pub use bosphorus_pool::{Admission, Approved, Config as PoolConfig, DepositRecord, Label, Lot, Lots, MAX_APPROVE, MAX_REMOVE, MAX_SPENDS};

/// Where a holder's lot stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LotState {
    /// Under the ASP's current root: withdrawable.
    Approved,
    /// Not approved (yet, or ever): only its depositor's ragequit takes it out.
    Pending,
}

impl LotState {
    pub fn name(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Pending => "pending",
        }
    }
}

/// Labels and asset ids as hex.
pub fn hex32(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn parse_label(s: &str) -> Result<Label> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    ensure!(s.len() == 64, "a label is 32 bytes of hex");
    let mut out = [0; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[2 * i..2 * i + 2], 16)?;
    }
    Ok(out)
}

/// The private accounts among `mentions`, once each, in order: one shared secret each.
fn private_ids(mentions: &[AccountMention]) -> Vec<AccountId> {
    let mut ids = vec![];
    for m in mentions {
        if let AccountIdentity::PrivateOwned(id) = m.identity
            && !ids.contains(&id)
        {
            ids.push(id);
        }
    }
    ids
}

/// An asset's program: the native token is built in, every other asset is our token.
fn deps(asset: AccountId) -> Vec<(AccountId, &'static [u8])> {
    if asset == NATIVE_TOKEN_PROGRAM_ID { vec![] } else { vec![(asset, TOKEN_ELF)] }
}

impl Client {
    // ---- reads ----------------------------------------------------------------------------

    async fn pool_shard(&self, pool: AccountId, account: AccountId) -> Result<Option<Shard>> {
        let view = self.wallet.get_account_view(ProgramShardSelector::new(account, pool)).await?;
        let data = view.data.shard(pool);
        if data.is_empty() {
            return Ok(None);
        }
        Ok(Some(borsh::from_slice(data).with_context(|| format!("{account} isn't a pool account"))?))
    }

    pub async fn pool_config(&self, pool: AccountId) -> Result<PoolConfig> {
        match self.pool_shard(pool, config_id(&pool)).await? {
            Some(Shard::Config(config)) => Ok(config),
            _ => bail!("pool {pool} isn't initialised"),
        }
    }

    /// The ASP's approved set (its labels, tree and root).
    pub async fn pool_approved(&self, pool: AccountId) -> Result<Approved> {
        match self.pool_shard(pool, approved_id(&pool)).await? {
            Some(Shard::Approved(approved)) => Ok(approved),
            _ => bail!("pool {pool} isn't initialised"),
        }
    }

    pub async fn pool_record(&self, pool: AccountId, label: &Label) -> Result<Option<DepositRecord>> {
        match self.pool_shard(pool, record_id(&pool, label)).await? {
            None => Ok(None),
            Some(Shard::Record(record)) => Ok(Some(record)),
            Some(_) => bail!("not a deposit record"),
        }
    }

    /// A private holder's lots, from the wallet's copy (as of the last sync).
    pub fn pool_lots(&self, pool: AccountId, holder: AccountId) -> Result<Lots> {
        let account = self.wallet.get_account_private(holder).unwrap_or_default();
        decode_lots(account.data.shard(pool)).map_err(|e| anyhow!(e))
    }

    /// The holder's lots, each with whether it is approved now.
    pub async fn pool_lot_states(&self, pool: AccountId, holder: AccountId) -> Result<Vec<(Lot, LotState)>> {
        let lots = self.pool_lots(pool, holder)?;
        if lots.0.is_empty() {
            return Ok(vec![]);
        }
        let approved = self.pool_approved(pool).await?;
        let state = |lot: &Lot| if approved.contains(&lot.label) { LotState::Approved } else { LotState::Pending };
        Ok(lots.0.iter().map(|lot| (*lot, state(lot))).collect())
    }

    // ---- setup and the ASP ----------------------------------------------------------------

    /// `Init`, signed by the pool's own header key (held by this wallet if it deployed it).
    pub async fn pool_init(&self, pool: AccountId, asp: AccountId, payer: AccountId) -> Result<()> {
        let accounts = vec![Holder::Public(pool).signer(pool), no_sign(config_id(&pool), pool), no_sign(approved_id(&pool), pool)];
        self.send_public("pool.init", pool, accounts, bosphorus_pool::Instruction::Init { asp }, Some(payer)).await?;
        ensure!(self.pool_config(pool).await?.asp == asp, "init didn't apply");
        Ok(())
    }

    /// Adds `labels` to the approved set (signed by the ASP), [`MAX_APPROVE`] per tx; each new root
    /// is accepted at once.
    pub async fn pool_approve(&self, pool: AccountId, asp: AccountId, labels: &[Label], payer: Option<AccountId>) -> Result<()> {
        let mut set = self.pool_approved(pool).await?;
        for chunk in labels.chunks(MAX_APPROVE) {
            set.approve(sha256, chunk).map_err(|e| anyhow!("{e}"))?;
            self.pool_set(pool, asp, bosphorus_pool::Instruction::Approve { labels: chunk.to_vec(), root: set.root }, payer).await?;
        }
        Ok(())
    }

    /// Removes `labels`, [`MAX_REMOVE`] per tx; every older root stops being accepted.
    pub async fn pool_remove(&self, pool: AccountId, asp: AccountId, labels: &[Label], payer: Option<AccountId>) -> Result<()> {
        let mut set = self.pool_approved(pool).await?;
        for chunk in labels.chunks(MAX_REMOVE) {
            let mut removals = vec![];
            for label in chunk {
                let index = set.index_of(label).with_context(|| format!("{} isn't approved", hex32(label)))?;
                let removal = Removal { label: *label, path: set.path(sha256, index).expect("in the set") };
                set.remove(sha256, &removal).map_err(|e| anyhow!("{e}"))?;
                removals.push(removal);
            }
            self.pool_set(pool, asp, bosphorus_pool::Instruction::Remove { removals, root: set.root }, payer).await?;
        }
        Ok(())
    }

    async fn pool_set(&self, pool: AccountId, asp: AccountId, ix: bosphorus_pool::Instruction, payer: Option<AccountId>) -> Result<HashType> {
        let (bosphorus_pool::Instruction::Approve { root, .. } | bosphorus_pool::Instruction::Remove { root, .. }) = ix else {
            bail!("not an ASP instruction")
        };
        let accounts = vec![Holder::Public(asp).signer(pool), no_sign(config_id(&pool), pool), no_sign(approved_id(&pool), pool)];
        let hash = self.send_public(&format!("pool.{}", ix.name()), pool, accounts, ix, payer).await?;
        ensure!(self.pool_config(pool).await?.roots.last() == Some(&root), "the ASP's update didn't apply (another update raced it?)");
        Ok(hash)
    }

    // ---- holders --------------------------------------------------------------------------

    /// Proves and sends a pool tx whose private accounts are all this wallet's.
    async fn send_pool(&self, pool: AccountId, asset: AccountId, mentions: Vec<AccountMention>, ix: bosphorus_pool::Instruction) -> Result<TransferSent> {
        let program = self.program_with(pool, POOL_ELF, &deps(asset)).await?;
        let private = private_ids(&mentions);
        let label = match ix {
            bosphorus_pool::Instruction::Deposit { .. } => "pool.deposit",
            bosphorus_pool::Instruction::Withdraw { .. } => "pool.withdraw",
            _ => "pool.ragequit",
        };
        let data = Program::serialize_instruction(ix)?;
        let started_ms = now_ms()?;
        let (hash, secrets) = self.wallet.send_privacy_preserving_tx(mentions, data, &program).await.map_err(send_error)?;
        ensure!(secrets.len() == private.len(), "{} shared secrets for {} private accounts", secrets.len(), private.len());
        Ok(TransferSent { hash, decode: secrets.into_iter().zip(private).collect(), label, started_ms, submitted_ms: now_ms()? })
    }

    fn owned_private(&self, id: AccountId) -> Result<()> {
        ensure!(self.wallet.resolve_private_account(id).is_some(), "Private/{id} isn't a private account of this wallet");
        Ok(())
    }

    /// A screened shield: `amount` of `asset` (a token, or the native token) from a public account
    /// into a lot on one of this wallet's private accounts. Returns the deposit's label.
    pub async fn send_pool_deposit(&self, pool: AccountId, asset: AccountId, from: Holder, to: Holder, amount: u128) -> Result<(TransferSent, Label)> {
        let Holder::Public(depositor) = from else {
            bail!("a deposit from a private account can't be screened: the ASP admits only a public debit (E10); deposit from a public account")
        };
        let Holder::Private(holder) = to else { bail!("the lot goes to a private account (a public one would show its label when withdrawn)") };
        self.owned_private(holder)?;
        ensure!(amount > 0, "zero amount");
        let have = if asset == NATIVE_TOKEN_PROGRAM_ID { self.native(from).await? } else { self.token_balance(asset, from).await? };
        ensure!(have >= amount, "{from} holds {have}, less than {amount}");
        // Labels are public; the nonce only has to be unique for this depositor.
        let nonce: [u8; 32] = Sha256::digest([&now_ms()?.to_le_bytes()[..], holder.as_ref(), &amount.to_le_bytes()].concat()).into();
        let label = deposit_label(&pool, &asset, &depositor, &nonce);
        let mentions = vec![
            Holder::Public(depositor).signer(asset),
            no_sign(record_id(&pool, &label), pool),
            Holder::Private(holder).signer(pool),
            no_sign(escrow_id(&pool, &asset), asset),
        ];
        let ix = bosphorus_pool::Instruction::Deposit { asset, amount, nonce };
        Ok((self.send_pool(pool, asset, mentions, ix).await?, label))
    }

    /// Withdraws `amount` of `asset` from the holder's approved lots, largest first, to `to` (a
    /// public account, or one of this wallet's private ones). Nothing published names the lots.
    pub async fn send_pool_withdraw(&self, pool: AccountId, asset: AccountId, holder: AccountId, amount: u128, to: Holder) -> Result<TransferSent> {
        self.owned_private(holder)?;
        if let Holder::Private(id) = to {
            self.owned_private(id)?;
        }
        ensure!(amount > 0, "zero amount");
        let set = self.pool_approved(pool).await?;
        let current = set.root;
        ensure!(self.pool_config(pool).await?.roots.contains(&current), "the approved list changed while it was read: try again");
        let mut lots: Vec<Lot> = self.pool_lots(pool, holder)?.0.into_iter().filter(|l| l.asset == *asset.value()).collect();
        let pending: u128 = lots.iter().filter(|l| !set.contains(&l.label)).map(|l| l.amount).sum();
        lots.retain(|l| set.contains(&l.label));
        lots.sort_by_key(|l| std::cmp::Reverse(l.amount));
        let approved: u128 = lots.iter().map(|l| l.amount).sum();
        ensure!(approved >= amount, "Private/{holder} holds {approved} approved (and {pending} pending) of this asset, less than {amount}");
        let mut spends = vec![];
        let mut left = amount;
        for lot in lots {
            if left == 0 {
                break;
            }
            let take = left.min(lot.amount);
            let index = set.index_of(&lot.label).expect("approved");
            spends.push(Spend { label: lot.label, amount: take, path: set.path(sha256, index).expect("in the set") });
            left -= take;
        }
        ensure!(spends.len() <= MAX_SPENDS, "{amount} spans {} lots, more than {MAX_SPENDS} at once: withdraw it in parts", spends.len());
        let to_mention = match to {
            Holder::Public(t) => no_sign(t, asset),
            Holder::Private(_) => to.signer(asset),
        };
        let mentions = vec![Holder::Private(holder).signer(pool), no_sign(config_id(&pool), pool), no_sign(escrow_id(&pool, &asset), asset), to_mention];
        self.send_pool(pool, asset, mentions, bosphorus_pool::Instruction::Withdraw { asset, spends, root: current }).await
    }

    /// Takes a lot back to its depositor (whose key this wallet must hold): all that is left,
    /// publicly, approved or not.
    pub async fn send_pool_ragequit(&self, pool: AccountId, holder: AccountId, label: Label) -> Result<TransferSent> {
        self.owned_private(holder)?;
        let lot = self.pool_lots(pool, holder)?.0.into_iter().find(|l| l.label == label).with_context(|| format!("Private/{holder} holds no lot {}", hex32(&label)))?;
        let record = self.pool_record(pool, &label).await?.context("no deposit record for this label")?;
        ensure!(!record.exited, "already taken back");
        let asset = record.asset;
        let depositor = if self.wallet.resolve_private_account(record.depositor).is_some() {
            Holder::Private(record.depositor)
        } else {
            ensure!(self.wallet.get_account_public_signing_key(record.depositor).is_some(), "the depositor {} isn't an account of this wallet", record.depositor);
            Holder::Public(record.depositor)
        };
        let mentions = vec![
            depositor.signer(asset),
            no_sign(record_id(&pool, &label), pool),
            Holder::Private(holder).signer(pool),
            no_sign(escrow_id(&pool, &asset), asset),
        ];
        self.send_pool(pool, asset, mentions, bosphorus_pool::Instruction::Ragequit { asset, label, amount: lot.amount }).await
    }

    /// Waits for a sent tx and records it (the CLI's path; the app waits its own way).
    pub async fn land(&mut self, sent: TransferSent) -> Result<HashType> {
        let (tx, block) = self.wallet.poll_transaction(sent.hash).await.context("tx not included (a check failed at settlement?)")?;
        eprintln!("tx {} included in block {block}", sent.hash);
        self.finish_transfer(&sent, &tx, block).await?;
        Ok(sent.hash)
    }
}

/// A deposit the ASP found, with its admission.
#[derive(Debug, Clone)]
pub struct Found {
    pub admission: Admission,
    pub tx: HashType,
    pub block: u64,
    pub timestamp_ms: u64,
}

/// The ASP's scan: deposit records written by settled private txs, each with E10's admission
/// rule applied to what the tx published. A deposit made by a public tx (a public holder) or by
/// another program isn't found; it stays unapproved, and ragequit stays open.
pub struct AspScanner {
    pub pool: AccountId,
    pub next_block: u64,
}

impl AspScanner {
    pub fn new(pool: AccountId, from_block: u64) -> Self {
        Self { pool, next_block: from_block.max(1) }
    }

    /// Scans up to the chain head; returns the deposits found in this call.
    pub async fn scan(&mut self, client: &Client) -> Result<Vec<Found>> {
        const CHUNK: u64 = 100;
        let rpc = client.wallet.client_owned();
        let last = rpc.get_last_block_id().await?;
        let mut found = vec![];
        while self.next_block <= last {
            let end = (self.next_block + CHUNK - 1).min(last);
            for block in rpc.get_block_range(self.next_block, end).await? {
                for tx in &block.body.transactions {
                    let LeeTransaction::PrivacyPreserving(private) = tx else { continue };
                    let actions: Vec<_> = private.message.public_actions.iter().map(|a| (a.account_id, a.effects.as_slice())).collect();
                    let signers: Vec<_> = private.witness_set.signatures_and_public_keys().iter().map(|(_, pk)| AccountId::from(pk)).collect();
                    for admission in bosphorus_pool::admissions(&self.pool, &actions, &signers) {
                        found.push(Found { admission, tx: tx.hash(), block: block.header.block_id, timestamp_ms: block.header.timestamp });
                    }
                }
            }
            self.next_block = end + 1;
        }
        Ok(found)
    }
}
