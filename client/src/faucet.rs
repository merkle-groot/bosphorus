//! The faucet (onboarding): test tokens and gas. A token drop is a public claim paid by the
//! claimer; the first gas comes from a claim by the wallet's private account (a private tx,
//! fee-exempt), paid to its public account.

use anyhow::{Context as _, Result, bail, ensure};
use bosphorus_faucet::{Config as FaucetConfig, Drop, Instruction, Record, config_id, custody_id, is_native, mint_authority};
use common::HashType;
use lee::{AccountId, ProgramShardSelector, program::Program};
use lee_core::native_token::NATIVE_TOKEN_PROGRAM_ID;
use wallet::AccountMention;

pub use bosphorus_faucet::Config;
pub use bosphorus_methods::FAUCET_ELF;

use crate::{Client, Holder, PUBLISH_LAG_MS, TOKEN_ELF, funds::TransferSent, no_sign, now_ms, send_error};

/// How far ahead a private claim's `t` goes. The faucet takes a claim only within
/// `[t, t + CLAIM_WINDOW_MS)` (10 min), which a CPU proof outlasts (11 and 15 min on the M1,
/// 2026-11-04): on a CPU the window opens 7 min after the proof starts and closes at 17. On a GPU
/// or in dev mode a claim lands within seconds to a minute of `t`. Unchanged program: `t` is the
/// client's. Sturdier, later: hold a proof that's early until `t` (the wallet's send hook).
pub const CPU_CLAIM_LEAD_MS: u64 = 7 * 60 * 1000;

fn private_claim_lead_ms() -> u64 {
    let dev = std::env::var("RISC0_DEV_MODE").is_ok_and(|v| !v.is_empty() && v != "0" && v.to_lowercase() != "false");
    if dev || crate::proving::info().1 { 0 } else { CPU_CLAIM_LEAD_MS }
}

impl Client {
    pub async fn faucet_config(&self, faucet: AccountId) -> Result<FaucetConfig> {
        let account = self.wallet.get_account_view(ProgramShardSelector::new(config_id(&faucet), faucet)).await?;
        borsh::from_slice(account.data.shard(faucet)).with_context(|| format!("faucet {faucet} isn't initialised"))
    }

    /// The claimer's record: on chain for a public account, from the wallet's notes for a private one.
    pub async fn faucet_record(&self, faucet: AccountId, claimer: Holder) -> Result<Record> {
        let account = match claimer {
            Holder::Public(id) => self.wallet.get_account_view(ProgramShardSelector::new(id, faucet)).await?,
            Holder::Private(id) => self.wallet.get_account_private(id).unwrap_or_default(),
        };
        let shard = account.data.shard(faucet);
        if shard.is_empty() {
            return Ok(Record::default());
        }
        borsh::from_slice(shard).context("not a faucet record")
    }

    /// `Init`, signed by the faucet's own header key (held by this wallet if it deployed it).
    pub async fn faucet_init(&self, faucet: AccountId, drops: Vec<Drop>, cooldown_ms: u64, payer: AccountId) -> Result<()> {
        let accounts = vec![Holder::Public(faucet).signer(faucet), no_sign(config_id(&faucet), faucet)];
        self.send_public("faucet.init", faucet, accounts, Instruction::Init { drops: drops.clone(), cooldown_ms }, Some(payer)).await?;
        let config = self.faucet_config(faucet).await?;
        ensure!(config.drops == drops && config.cooldown_ms == cooldown_ms, "init didn't apply: {config:?}");
        Ok(())
    }

    /// The mentions of a claim, in instruction order (`bosphorus_faucet::claim_selectors`).
    /// `to` signs too when it's the claimer or the fee payer: one account takes one identity in a tx.
    fn claim_mentions(faucet: AccountId, asset: AccountId, claimer: Holder, to: Holder, payer: Option<AccountId>) -> Vec<AccountMention> {
        let mut out = vec![claimer.signer(faucet), no_sign(config_id(&faucet), faucet)];
        if is_native(&asset) {
            out.push(no_sign(custody_id(&faucet), NATIVE_TOKEN_PROGRAM_ID));
        } else {
            out.push(no_sign(mint_authority(&faucet), asset));
            out.push(no_sign(bosphorus_token::config_id(&asset), asset));
        }
        out.push(match to {
            Holder::Public(id) if to != claimer && payer != Some(id) => no_sign(id, asset),
            _ => to.signer(asset),
        });
        out
    }

    /// Sends a claim of `asset`'s drop for `claimer`, paid to `to`. A public claimer sends a
    /// public tx, its fee paid by `payer` (default: the claimer); a private one proves a private tx
    /// (fee-exempt), paid to a public account or to the claimer itself. Errors before sending if the cooldown hasn't passed.
    pub async fn send_faucet_claim(&self, faucet: AccountId, asset: AccountId, claimer: Holder, to: Holder, payer: Option<AccountId>) -> Result<TransferSent> {
        let config = self.faucet_config(faucet).await?;
        let amount = config.drop_of(&asset).with_context(|| format!("the faucet doesn't drop {asset}"))?;
        let t = now_ms()?.saturating_sub(PUBLISH_LAG_MS) + if matches!(claimer, Holder::Private(_)) { private_claim_lead_ms() } else { 0 };
        let next = self.faucet_record(faucet, claimer).await?.next_claim(&asset, config.cooldown_ms);
        if t < next {
            bail!(crate::lending::Refusal::FaucetCooldown { asset, next_ms: next });
        }
        let ix = Instruction::Claim { asset, amount, cooldown_ms: config.cooldown_ms, t };
        let accounts = Self::claim_mentions(faucet, asset, claimer, to, payer);
        let data = Program::serialize_instruction(ix)?;
        let started_ms = now_ms()?;
        let (hash, decode) = match claimer {
            Holder::Public(id) => {
                self.check_payer(payer.unwrap_or(id)).await?;
                (self.wallet.send_pub_tx_paid_by(accounts, data, faucet, payer).await.map_err(send_error)?, vec![])
            }
            Holder::Private(id) => {
                // Paid to a public account, or to the claimer itself (one private account, two shards).
                ensure!(matches!(to, Holder::Public(_)) || to == claimer, "a private claim pays a public account or the claimer");
                let deps: Vec<(AccountId, &'static [u8])> = if is_native(&asset) { vec![] } else { vec![(asset, TOKEN_ELF)] };
                let program = self.program_with(faucet, FAUCET_ELF, &deps).await?;
                let (hash, secrets) = self.wallet.send_privacy_preserving_tx(accounts, data, &program).await.map_err(send_error)?;
                let secret = secrets.into_iter().next().context("no shared secret for the claimer")?;
                (hash, vec![(secret, id)])
            }
        };
        Ok(TransferSent { hash, decode, label: "faucet.claim", started_ms, submitted_ms: now_ms()? })
    }

    /// [`Self::send_faucet_claim`], then waits for it and records it.
    pub async fn faucet_claim(&mut self, faucet: AccountId, asset: AccountId, claimer: Holder, to: Holder, payer: Option<AccountId>) -> Result<HashType> {
        let sent = self.send_faucet_claim(faucet, asset, claimer, to, payer).await?;
        let (tx, block) = self.wallet.poll_transaction(sent.hash).await?;
        eprintln!("tx {} included in block {block}", sent.hash);
        self.finish_transfer(&sent, &tx, block).await?;
        Ok(sent.hash)
    }
}
