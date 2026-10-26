//! Verifies a proof from the patched Metal build with stock crates.io risc0-zkvm 3.0.5 (this crate
//! is its own workspace, so none of the repo's [patch] entries apply). Dump one first:
//!   S4_DUMP=spikes/stockverify/data S4_ONLY_A=1 R0_METAL_FULL=1 RISC0_KECCAK_PO2=14 ./target/release/s2_private
//!   cd spikes/stockverify && cargo run --release --offline -- data

use risc0_zkvm::{InnerReceipt, Receipt};

/// lee v0.3.1 PRIVACY_PRESERVING_CIRCUIT_ID (from LEZ's own build).
const CIRCUIT_ID: [u32; 8] = [3943929112, 3682468618, 2822605667, 1764660516, 290278025, 2512717227, 411457898, 556249114];

fn main() -> anyhow::Result<()> {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "data".into());
    let proof = std::fs::read(format!("{dir}/proof.bin"))?;
    let journal = std::fs::read(format!("{dir}/journal.bin"))?;
    let inner: InnerReceipt = borsh::from_slice(&proof)?;
    let receipt = Receipt::new(inner, journal.clone());
    receipt.verify(CIRCUIT_ID)?;
    println!("stock risc0-zkvm 3.0.5: receipt verified against the circuit image id");
    let mut tampered = journal;
    tampered[100] ^= 1;
    let bad = Receipt::new(borsh::from_slice(&proof)?, tampered);
    assert!(bad.verify(CIRCUIT_ID).is_err(), "a tampered journal verified");
    println!("stock risc0-zkvm 3.0.5: tampered journal rejected");
    Ok(())
}
