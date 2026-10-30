//! `bos tx-report`: what a lending tx actually published (plan 4.3), and a byte scan for anything
//! that identifies an owner. The observed-privacy report (`docs/spikes/P4-privacy-observed.md`)
//! is built from its output.

use anyhow::{Context as _, Result, bail};
use bosphorus_client::{Client, Holder, lending::MarketId};
use clap::Args;
use common::{HashType, transaction::LeeTransaction};
use lee::AccountId;
use lee_core::native_token::NATIVE_TOKEN_PROGRAM_ID;

#[derive(Args)]
pub struct TxReport {
    /// Tx hash (hex), as printed by `bos` ("tx … included in block …").
    #[arg(long)]
    tx: String,
    /// Scan for this owner's id and keys, and with `--market` its position salt.
    #[arg(long)]
    owner: Option<Holder>,
    #[arg(long, value_parser = bosphorus_client::lending::parse_market_id)]
    market: Option<MarketId>,
    /// A pool (E10): decode its effects; with `--label`, scan for the labels and their records.
    #[arg(long)]
    pool: Option<Holder>,
    /// Repeatable; needs `--pool`.
    #[arg(long, requires = "pool")]
    label: Vec<String>,
    #[arg(long, default_value_t = 0)]
    index: u32,
    /// Exit non-zero if anything identifying is found.
    #[arg(long)]
    check: bool,
}

/// Pool effects (by the pool), then lending, token and native effects in that order.
fn effect(data: &[u8], program: AccountId, shard_program: AccountId, pool: Option<AccountId>) -> String {
    if Some(program) == pool
        && let Ok(e) = borsh::from_slice::<bosphorus_pool::Effect>(data)
    {
        return format!("pool {e:?}");
    }
    if shard_program == NATIVE_TOKEN_PROGRAM_ID
        && let Ok(e) = borsh::from_slice::<lee_core::native_token::Effect>(data)
    {
        return format!("native {e:?}");
    }
    if let Ok(e) = borsh::from_slice::<bosphorus_lending::Effect>(data) {
        return format!("lending {e:?}");
    }
    if let Ok(e) = borsh::from_slice::<bosphorus_token::Effect>(data) {
        return format!("token {e:?}");
    }
    format!("{} undecoded bytes", data.len())
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

pub async fn tx_report(client: &Client, cmd: TxReport) -> Result<()> {
    let hash: HashType = cmd.tx.parse().map_err(|e| anyhow::anyhow!("bad tx hash: {e}"))?;
    let (tx, block) = client.wallet.get_transaction(hash).await?.context("tx not found")?;
    let bytes = borsh::to_vec(&tx)?;
    match &tx {
        LeeTransaction::Public(tx) => {
            let m = &tx.message;
            println!("tx {hash}: PUBLIC, block {block}, {} bytes", bytes.len());
            println!("  program {}  signers {}  fee {:?}", m.program_account_id, m.nonces.len(), m.fee.is_some());
            for s in &m.shard_selectors {
                println!("  account {} (shard of {})", s.account_id, s.program_account_id);
            }
            match borsh::from_slice::<bosphorus_lending::Instruction>(&m.instruction_data) {
                Ok(ix) => println!("  instruction {ix:?}"),
                Err(_) => println!("  instruction: {} bytes", m.instruction_data.len()),
            }
        }
        LeeTransaction::PrivacyPreserving(tx) => {
            let m = &tx.message;
            let message_len = borsh::to_vec(m)?.len();
            println!("tx {hash}: PRIVATE, block {block}, {} bytes ({message_len} message, {} proof and signatures)", bytes.len(), bytes.len() - message_len);
            println!("  timestamp window {:?} (its start is the claimed t, ≈ when proving began)", m.timestamp_validity_window);
            println!("  block window {:?}", m.block_validity_window);
            println!("  public accounts: {}", m.public_actions.len());
            for a in &m.public_actions {
                println!("    {}", a.account_id);
                for e in &a.effects {
                    println!("      by {} on shard of {}: {}", e.program_account_id, e.shard_program_account_id, effect(&e.data, e.program_account_id, e.shard_program_account_id, cmd.pool.map(Holder::id)));
                }
            }
            println!("  private actions: {}", m.private_actions.len());
            for p in &m.private_actions {
                let e = &p.encrypted_post_state;
                println!("    nullifier + commitment + {}-byte ciphertext, view tag {:?}", borsh::to_vec(&e.ciphertext)?.len(), e.view_tag);
            }
            println!("  signer nonces: {}", m.nonces.len());
            for c in &m.program_image_claims {
                println!("  program claim {c:?}");
            }
        }
    }

    let mut needles = vec![];
    for label in &cmd.label {
        let label = bosphorus_client::pool::parse_label(label)?;
        let pool = cmd.pool.expect("clap requires --pool").id();
        needles.push(("deposit label", label.to_vec()));
        needles.push(("deposit record", bosphorus_pool::record_id(&pool, &label).as_ref().to_vec()));
    }
    if let Some(holder) = cmd.owner {
        needles.push(("owner account id", holder.id().as_ref().to_vec()));
    }
    if let (Some(holder @ Holder::Private(_)), Some(market)) = (cmd.owner, cmd.market) {
        needles.push(("position salt", client.owner(holder, &market, cmd.index)?.salt.to_vec()));
    }
    if let Some(Holder::Private(id)) = cmd.owner {
        let keys = client.wallet.storage().key_chain().private_account(id).context("not in this wallet")?.key_chain;
        needles.push(("nullifier public key", keys.nullifier_public_key.0.to_vec()));
        needles.push(("viewing public key (first 32 bytes)", keys.viewing_public_key.to_bytes()[..32].to_vec()));
    }
    if needles.is_empty() {
        return Ok(());
    }
    let found: Vec<_> = needles.iter().filter(|(_, n)| contains(&bytes, n)).map(|(what, _)| *what).collect();
    for (what, needle) in &needles {
        println!("  scan: {what}: {}", if contains(&bytes, needle) { "FOUND" } else { "absent" });
    }
    if found.is_empty() {
        println!("LEAKS none");
    } else {
        println!("LEAKS {}", found.join(", "));
        if cmd.check {
            bail!("identifying bytes in {hash}: {}", found.join(", "));
        }
    }
    Ok(())
}
