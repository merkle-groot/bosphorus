//! The wallet's own funds, for an app with a wallet of its own (the Basecamp module): create or
//! restore it, add accounts, move native LEZ, sync private state.
//!
//! Private transfers go through `send_privacy_preserving_tx`, so they prove with
//! `bosphorus_prover` like every other private tx (`proving::install`).

use std::path::Path;

use anyhow::{Context as _, Result, anyhow, bail};
use lee::{AccountId, program::Program};
use lee_core::encryption::SharedSecretKey;
use bip39::Mnemonic;
use common::{HashType, transaction::LeeTransaction};
use lee::native_token::NATIVE_TOKEN_PROGRAM_ID;
use sequencer_service_rpc::RpcClient as _;
use wallet::{AccDecodeData, AccountIdentity, WalletCore, program_facades::native_token_transfer::NativeTokenTransfer};

use crate::{Client, Holder, TOKEN_ELF, no_sign, now_ms, send_error};

/// A transfer handed to the sequencer ([`Client::send_transfer`]).
pub struct TransferSent {
    pub hash: HashType,
    /// The notes to decode once it lands: one shared secret per private account.
    decode: Vec<(SharedSecretKey, AccountId)>,
    label: &'static str,
    started_ms: u64,
    pub submitted_ms: u64,
}

/// LEZ v0.3.1 ignores the storage password (keys are plain JSON); the file's 0600 mode is what
/// protects it.
const PASSWORD: &str = "";

/// Creates `wallet_config.json` and `storage.json` in `home` for `sequencer`. A new wallet
/// (`mnemonic: None`) starts syncing at the chain's tip, since nothing before it can be its own,
/// and returns its recovery phrase; a restored one scans from `sync_from` (default the first
/// block; LEZ's wallet saves after every block, so a scan of the whole testnet is slow).
pub async fn create_wallet(home: &Path, sequencer: &str, mnemonic: Option<&str>, sync_from: Option<u64>) -> Result<Option<String>> {
    let (config_path, storage_path) = (home.join("wallet_config.json"), home.join("storage.json"));
    if storage_path.exists() {
        bail!("{} already holds a wallet", home.display());
    }
    std::fs::create_dir_all(home).with_context(|| format!("creating {}", home.display()))?;
    restrict(home, 0o700)?;
    // The testnet settings `scripts/testnet/wallet_config.json` uses: poll every 5 s, up to 30 min.
    let config = serde_json::json!({
        "sequencer": { "sequencer_addr": sequencer },
        "seq_poll_timeout": "5s",
        "seq_tx_poll_max_blocks": 360,
        "seq_poll_max_retries": 1,
        "seq_block_poll_max_amount": 100,
    });
    std::fs::write(&config_path, serde_json::to_vec_pretty(&config)?)?;
    restrict(&config_path, 0o600)?;
    let (mut wallet, created) = WalletCore::new_init_storage(config_path, storage_path.clone(), None, PASSWORD)?;
    let phrase = match mnemonic {
        None => {
            let tip = wallet.client_owned().get_last_block_id().await.context("reading the chain's tip")?;
            wallet.storage_mut().set_last_synced_block(tip);
            Some(created.to_string())
        }
        Some(words) => {
            let words = Mnemonic::parse(words.trim()).map_err(|e| anyhow!("recovery phrase: {e}"))?;
            wallet.restore_storage(&words, PASSWORD)?;
            wallet.storage_mut().set_last_synced_block(sync_from.unwrap_or(1).saturating_sub(1));
            None
        }
    };
    wallet.store_persistent_data()?;
    restrict(&storage_path, 0o600)?;
    Ok(phrase)
}

fn restrict(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).with_context(|| format!("chmod {}", path.display()))
}

impl Client {
    /// A new account from the wallet's key tree, saved at once.
    pub fn new_account(&mut self, private: bool) -> Result<Holder> {
        let holder = if private {
            Holder::Private(self.wallet.create_new_account_private(None).0)
        } else {
            Holder::Public(self.wallet.create_new_account_public(None).0)
        };
        self.wallet.store_persistent_data()?;
        Ok(holder)
    }

    /// Native LEZ: a public balance from the sequencer, a private one from the wallet's notes.
    pub async fn native(&self, holder: Holder) -> Result<u128> {
        match holder {
            Holder::Public(id) => self.native_balance(id).await,
            Holder::Private(id) => {
                let account = self.wallet.get_account_private(id).unwrap_or_default();
                lee_core::native_token::decode_balance(account.data.shard(NATIVE_TOKEN_PROGRAM_ID)).map_err(|e| anyhow!("{e:?}"))
            }
        }
    }

    /// Moves native LEZ, or a token when `token` is given, and returns once it is sent; the
    /// caller waits for inclusion (no lock needed: [`TransferSent::hash`]) and then calls
    /// [`Self::finish_transfer`]. `from` is this wallet's; a private `to` must be too (paying
    /// someone else's private account needs their keys, not just an id). Public → public is a
    /// public tx; anything touching a private account is a private one (proven here).
    pub async fn send_transfer(&self, token: Option<AccountId>, from: Holder, to: Holder, amount: u128) -> Result<TransferSent> {
        let owned_private = |id| self.wallet.resolve_private_account(id).is_some();
        if let Holder::Private(id) = from
            && !owned_private(id)
        {
            bail!("{from} isn't a private account of this wallet");
        }
        if let Holder::Private(id) = to
            && !owned_private(id)
        {
            bail!("{to} isn't a private account of this wallet: sending to someone else's private account needs their keys");
        }
        let started_ms = now_ms()?;
        let (hash, decode) = match token {
            None => {
                let facade = NativeTokenTransfer(&self.wallet);
                match (from, to) {
                    (Holder::Public(f), Holder::Public(t)) => {
                        self.check_payer(f).await?;
                        (facade.send_public_transfer(AccountIdentity::Public(f), AccountIdentity::PublicNoSign(t), amount).await.map_err(send_error)?, vec![])
                    }
                    (Holder::Public(f), Holder::Private(t)) => {
                        let (hash, secret) = facade.send_shielded_transfer(AccountIdentity::Public(f), t, amount).await.map_err(send_error)?;
                        (hash, vec![(secret, t)])
                    }
                    (Holder::Private(f), Holder::Public(t)) => {
                        let (hash, secret) = facade.send_deshielded_transfer(f, t, amount).await.map_err(send_error)?;
                        (hash, vec![(secret, f)])
                    }
                    (Holder::Private(f), Holder::Private(t)) => {
                        let (hash, [s0, s1]) = facade.send_private_transfer_to_owned_account(f, t, amount).await.map_err(send_error)?;
                        (hash, vec![(s0, f), (s1, t)])
                    }
                }
            }
            Some(token) => match (from, to) {
                (Holder::Public(f), Holder::Public(t)) => {
                    self.check_payer(f).await?;
                    let accounts = vec![Holder::Public(f).signer(token), no_sign(t, token)];
                    let data = Program::serialize_instruction(bosphorus_token::Instruction::Transfer { amount })?;
                    (self.wallet.send_pub_tx_paid_by(accounts, data, token, None).await.map_err(send_error)?, vec![])
                }
                (Holder::Public(f), Holder::Private(t)) => {
                    let program = self.program(token, TOKEN_ELF).await?;
                    let accounts = vec![Holder::Public(f).signer(token), Holder::Private(t).signer(token)];
                    let data = Program::serialize_instruction(bosphorus_token::Instruction::Transfer { amount })?;
                    let (hash, secrets) = self.wallet.send_privacy_preserving_tx(accounts, data, &program).await.map_err(send_error)?;
                    let secret = secrets.into_iter().next().context("no shared secret for the recipient")?;
                    (hash, vec![(secret, t)])
                }
                (Holder::Private(f), to) => {
                    let program = self.program(token, TOKEN_ELF).await?;
                    let to_mention = match to {
                        Holder::Public(t) => no_sign(t, token),
                        Holder::Private(_) => to.signer(token),
                    };
                    let accounts = vec![Holder::Private(f).signer(token), to_mention];
                    let data = Program::serialize_instruction(bosphorus_token::Instruction::Transfer { amount })?;
                    let (hash, secrets) = self.wallet.send_privacy_preserving_tx(accounts, data, &program).await.map_err(send_error)?;
                    // One shared secret per private account, in account order.
                    let mut secrets = secrets.into_iter();
                    let mut decode = vec![(secrets.next().context("no shared secret for the sender")?, f)];
                    if let Holder::Private(t) = to {
                        decode.push((secrets.next().context("no shared secret for the recipient")?, t));
                    }
                    (hash, decode)
                }
            },
        };
        let submitted_ms = now_ms()?;
        let label = if token.is_some() { "token.transfer" } else { "native.transfer" };
        Ok(TransferSent { hash, decode, label, started_ms, submitted_ms })
    }

    /// Records a landed transfer: the `[measure]` line, then the private notes it made into the
    /// wallet, saved.
    pub async fn finish_transfer(&mut self, sent: &TransferSent, tx: &LeeTransaction, block: u64) -> Result<()> {
        let private = !sent.decode.is_empty();
        let proving = private.then(|| sent.submitted_ms - sent.started_ms);
        self.measure(sent.label, if private { "private" } else { "public" }, sent.hash, sent.submitted_ms, proving, block, None).await;
        if let LeeTransaction::PrivacyPreserving(tx) = tx {
            let decode: Vec<_> = sent.decode.iter().map(|(secret, id)| AccDecodeData::Decode(*secret, *id)).collect();
            self.wallet.decode_insert_privacy_preserving_transaction_results(tx, &decode)?;
            self.wallet.store_persistent_data()?;
        }
        Ok(())
    }

    /// Scans up to `max_blocks` new blocks for this wallet's private notes and returns the block
    /// synced to, so a long catch-up can be done in steps without holding the wallet throughout.
    pub async fn sync_step(&mut self, max_blocks: u64) -> Result<(u64, u64)> {
        let tip = self.wallet.get_last_block_id().await?;
        let to = tip.min(self.synced_block().saturating_add(max_blocks.max(1)));
        self.wallet.sync_to_block(to).await?;
        Ok((to, tip))
    }

    /// Scans new blocks for this wallet's private notes; returns the block synced to.
    pub async fn sync(&mut self) -> Result<u64> {
        self.wallet.sync_to_latest_block().await
    }

    /// The block private state is synced to.
    pub fn synced_block(&self) -> u64 {
        self.wallet.storage().last_synced_block()
    }
}
