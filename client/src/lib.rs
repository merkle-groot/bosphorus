//! Bosphorus client: deploys, tx builders and state readers on top of LEZ's `WalletCore`.
//!
//! Every tx helper waits for inclusion. Inclusion doesn't mean success (a failed public tx still
//! lands and pays, constraints §5), so callers check the resulting state.

use std::{borrow::Cow, collections::HashMap, fmt, str::FromStr};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use borsh::BorshSerialize;
use common::transaction::LeeTransaction;
use lee::{
    Account, AccountId, ProgramShardSelector, privacy_preserving_transaction::circuit::ProgramWithDependencies,
    program::Program,
};
use lee_core::program::{PROGRAM_LOADER_ACCOUNT_ID, ProgramHeader};
use wallet::{
    AccDecodeData, AccountIdentity, AccountMention, WalletCore, program_facades::program_loader::ProgramLoader,
};

pub use bosphorus_methods::{ORACLE_ELF, TOKEN_ELF};
pub use bosphorus_oracle::{Feed, U256};
pub use bosphorus_token::Config as TokenConfig;

/// A wallet account, written `Public/<id>` or `Private/<id>` as in the LEZ wallet CLI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Holder {
    Public(AccountId),
    Private(AccountId),
}

impl Holder {
    pub fn id(self) -> AccountId {
        match self {
            Self::Public(id) | Self::Private(id) => id,
        }
    }

    /// Mention as a signer (owned public / owned private account).
    fn signer(self, program: AccountId) -> AccountMention {
        match self {
            Self::Public(id) => AccountIdentity::Public(id),
            Self::Private(id) => AccountIdentity::PrivateOwned(id),
        }
        .select_program_shard(program)
    }
}

impl FromStr for Holder {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let (kind, id) = s.split_once('/').unwrap_or(("Public", s));
        let id = id.parse().map_err(|e| anyhow!("bad account id {id}: {e:?}"))?;
        match kind {
            "Public" => Ok(Self::Public(id)),
            "Private" => Ok(Self::Private(id)),
            _ => bail!("expected Public/<id> or Private/<id>, got {s}"),
        }
    }
}

impl fmt::Display for Holder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Public(id) => write!(f, "Public/{id}"),
            Self::Private(id) => write!(f, "Private/{id}"),
        }
    }
}

fn no_sign(id: AccountId, program: AccountId) -> AccountMention {
    AccountIdentity::PublicNoSign(id).select_program_shard(program)
}

pub struct Client {
    pub wallet: WalletCore,
}

impl Client {
    /// Uses `LEE_WALLET_HOME_DIR`, like the `wallet` binary.
    pub fn from_env() -> Result<Self> {
        Ok(Self { wallet: WalletCore::from_env()? })
    }

    async fn send_public(
        &self,
        program: AccountId,
        accounts: Vec<AccountMention>,
        instruction: impl BorshSerialize,
        payer: Option<AccountId>,
    ) -> Result<()> {
        let data = Program::serialize_instruction(instruction)?;
        let hash = self
            .wallet
            .send_pub_tx_paid_by(accounts, data, program, payer)
            .await
            .map_err(|e| anyhow!("{e:?}"))?;
        let (_, block) = self.wallet.poll_transaction(hash).await?;
        eprintln!("tx {hash} included in block {block}");
        Ok(())
    }

    // ---- programs -------------------------------------------------------------------------

    pub async fn header(&self, program: AccountId) -> Result<ProgramHeader> {
        let account = self.wallet.get_account_view(ProgramShardSelector::new(program, PROGRAM_LOADER_ACCOUNT_ID)).await?;
        ProgramHeader::from_bytes(account.data.shard(PROGRAM_LOADER_ACCOUNT_ID))
            .with_context(|| format!("{program} holds no program header"))
    }

    /// Uploads `elf` and creates an immutable header owned by a fresh wallet key. The header's
    /// address is the program's id.
    pub async fn deploy(&mut self, elf: &[u8], payer: AccountId) -> Result<AccountId> {
        let program = Program::new(Cow::Owned(elf.to_vec()))?;
        let segments = program.user_elf()?.len().div_ceil(program_loader_core::MAX_SEGMENT_DATA_LEN);
        let header = self.wallet.create_new_account_public(None).0;
        let segments: Vec<_> =
            std::iter::repeat_with(|| self.wallet.create_new_account_public(None).0).take(segments).collect();
        self.wallet.store_persistent_data()?;
        ProgramLoader(&self.wallet).deploy(header, &segments, elf.to_vec(), true, Some(payer)).await?;
        Ok(header)
    }

    /// A new instance of `of`'s bytecode: one `CreateHeader` on its segments (S5).
    pub async fn instance(&mut self, of: AccountId, payer: AccountId) -> Result<AccountId> {
        let first = self.header(of).await?.program_first_segment;
        let loader = ProgramLoader(&self.wallet);
        let chain = loader.resolve_chain(first).await?;
        let header = self.wallet.create_new_account_public(None).0;
        self.wallet.store_persistent_data()?;
        let hash = ProgramLoader(&self.wallet)
            .create_header(header, first, &chain, true, Some(payer))
            .await
            .map_err(|e| anyhow!("{e:?}"))?;
        self.wallet.poll_transaction(hash).await?;
        ensure!(self.header(header).await.is_ok(), "header {header} wasn't created");
        Ok(header)
    }

    /// The program at `id`, checked against the bytecode we hold, for private txs.
    async fn program(&self, id: AccountId, elf: &'static [u8]) -> Result<ProgramWithDependencies> {
        let program = Program::new(Cow::Borrowed(elf))?;
        ensure!(
            self.header(id).await?.image_id == program.id(),
            "{id} runs different bytecode than this client's build"
        );
        Ok(ProgramWithDependencies::new(program, id, HashMap::new()))
    }

    // ---- token ----------------------------------------------------------------------------

    /// `Init`, signed by the token's own header key (held by this wallet if it deployed it).
    pub async fn token_init(&self, token: AccountId, mint_authority: AccountId, decimals: u8, payer: AccountId) -> Result<()> {
        let accounts = vec![Holder::Public(token).signer(token), no_sign(bosphorus_token::config_id(&token), token)];
        let ix = bosphorus_token::Instruction::Init { mint_authority, decimals };
        self.send_public(token, accounts, ix, Some(payer)).await?;
        let config = self.token_config(token).await?;
        ensure!(config.mint_authority == mint_authority && config.decimals == decimals, "init didn't apply: {config:?}");
        Ok(())
    }

    pub async fn token_mint(&self, token: AccountId, authority: AccountId, to: AccountId, amount: u128, payer: Option<AccountId>) -> Result<()> {
        let before = self.token_balance(token, Holder::Public(to)).await?;
        let accounts = vec![
            Holder::Public(authority).signer(token),
            no_sign(bosphorus_token::config_id(&token), token),
            no_sign(to, token),
        ];
        self.send_public(token, accounts, bosphorus_token::Instruction::Mint { amount }, payer).await?;
        let after = self.token_balance(token, Holder::Public(to)).await?;
        ensure!(after == before + amount, "mint didn't apply: balance {before} → {after}");
        Ok(())
    }

    pub async fn token_transfer(&self, token: AccountId, from: AccountId, to: AccountId, amount: u128, payer: Option<AccountId>) -> Result<()> {
        let before = self.token_balance(token, Holder::Public(to)).await?;
        let accounts = vec![Holder::Public(from).signer(token), no_sign(to, token)];
        self.send_public(token, accounts, bosphorus_token::Instruction::Transfer { amount }, payer).await?;
        let after = self.token_balance(token, Holder::Public(to)).await?;
        ensure!(after == before + amount, "transfer didn't apply: recipient {before} → {after}");
        Ok(())
    }

    /// Public → private transfer of a custom token, as a privacy-preserving tx (fee-exempt today).
    /// `to` must be a private account in this wallet.
    pub async fn token_shield(&mut self, token: AccountId, from: AccountId, to: AccountId, amount: u128) -> Result<()> {
        let program = self.program(token, TOKEN_ELF).await?;
        let accounts = vec![Holder::Public(from).signer(token), Holder::Private(to).signer(token)];
        let data = Program::serialize_instruction(bosphorus_token::Instruction::Transfer { amount })?;
        let (hash, secrets) = self
            .wallet
            .send_privacy_preserving_tx(accounts, data, &program)
            .await
            .map_err(|e| anyhow!("{e:?}"))?;
        let (tx, block) = self.wallet.poll_transaction(hash).await?;
        eprintln!("tx {hash} included in block {block}");
        let LeeTransaction::PrivacyPreserving(tx) = tx else { bail!("expected a private tx") };
        let secret = secrets.into_iter().next().context("no shared secret for the recipient")?;
        self.wallet.decode_insert_privacy_preserving_transaction_results(&tx, &[AccDecodeData::Decode(secret, to)])?;
        self.wallet.store_persistent_data()?;
        Ok(())
    }

    /// Public balances come from the sequencer; private ones from the wallet's decoded notes.
    pub async fn token_balance(&self, token: AccountId, holder: Holder) -> Result<u128> {
        let account = match holder {
            Holder::Public(id) => self.wallet.get_account_view(ProgramShardSelector::new(id, token)).await?,
            Holder::Private(id) => self.wallet.get_account_private(id).unwrap_or_else(Account::default),
        };
        bosphorus_token::decode_balance(account.data.shard(token)).map_err(|e| anyhow!("{e}"))
    }

    pub async fn token_config(&self, token: AccountId) -> Result<TokenConfig> {
        let config = bosphorus_token::config_id(&token);
        let account = self.wallet.get_account_view(ProgramShardSelector::new(config, token)).await?;
        borsh::from_slice(account.data.shard(token)).with_context(|| format!("token {token} isn't initialised"))
    }

    // ---- oracle ---------------------------------------------------------------------------

    pub async fn feed_create(&self, oracle: AccountId, publisher: AccountId, name: &str, payer: Option<AccountId>) -> Result<AccountId> {
        let feed = bosphorus_oracle::feed_id(&oracle, &publisher, name);
        let accounts = vec![Holder::Public(publisher).signer(oracle), no_sign(feed, oracle)];
        self.send_public(oracle, accounts, bosphorus_oracle::Instruction::CreateFeed { name: name.into() }, payer).await?;
        self.feed(feed, oracle).await?;
        Ok(feed)
    }

    /// Publishes `price` (already scaled, see `bosphorus_core::scale_price`) stamped with now.
    pub async fn feed_publish(&self, oracle: AccountId, publisher: AccountId, name: &str, price: U256, payer: Option<AccountId>) -> Result<Feed> {
        let feed = bosphorus_oracle::feed_id(&oracle, &publisher, name);
        let timestamp = u64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis())?;
        let accounts = vec![Holder::Public(publisher).signer(oracle), no_sign(feed, oracle)];
        self.send_public(oracle, accounts, bosphorus_oracle::Instruction::Publish { price, timestamp }, payer).await?;
        let read = self.feed(feed, oracle).await?;
        ensure!(read.updated_at == timestamp && read.price == price, "publish didn't apply: {read:?}");
        Ok(read)
    }

    pub async fn feed(&self, feed: AccountId, oracle: AccountId) -> Result<Feed> {
        let account = self.wallet.get_account_view(ProgramShardSelector::new(feed, oracle)).await?;
        borsh::from_slice(account.data.shard(oracle)).with_context(|| format!("no feed at {feed}"))
    }
}

pub fn feed_id(oracle: AccountId, publisher: AccountId, name: &str) -> AccountId {
    bosphorus_oracle::feed_id(&oracle, &publisher, name)
}
