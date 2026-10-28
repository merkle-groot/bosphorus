//! Process-wide host prover defaults. These do not change guest images or verification.

/// Apply before creating the async runtime. An explicit environment setting wins.
///
/// # Safety
/// Must run while the process is single-threaded: changing environment variables can race
/// with readers in Rust or native libraries. Library callers can instead set the variable
/// in the environment before launching their process.
pub unsafe fn configure() {
    #[cfg(feature = "prove")]
    if std::env::var_os("RISC0_PROVER").is_none() {
        unsafe { std::env::set_var("RISC0_PROVER", "local") };
    }
    if std::env::var_os("RISC0_KECCAK_PO2").is_none() {
        // 65 permutations per private account; po2 14 holds 81, whereas the default 17
        // pays to prove 131,072 rows. Every supported size verifies with the same circuit.
        unsafe { std::env::set_var("RISC0_KECCAK_PO2", "14") };
    }
}

/// Private txs prove with `bosphorus_prover` (its own schedule on the GPU, if the host has one,
/// and the keccak po2 without env vars) instead of LEZ's `execute_and_prove`. Without the `prove`
/// feature they go to `r0vm` as before. Idempotent; `Client`'s constructors call it.
pub fn install() {
    #[cfg(feature = "prove")]
    wallet::set_private_prover(bosphorus_prover::execute_and_prove);
}

/// What private txs prove with, for front-ends: `(keccak po2, on the GPU)`. Without the `prove`
/// feature (proving through `r0vm`) the po2 is the environment's, else risc0's default 17.
pub fn info() -> (u32, bool) {
    let env = std::env::var("RISC0_KECCAK_PO2").ok().and_then(|v| v.parse().ok());
    #[cfg(feature = "prove")]
    return (env.unwrap_or(bosphorus_prover::KECCAK_PO2), bosphorus_prover::gpu());
    #[cfg(not(feature = "prove"))]
    return (env.unwrap_or(17), false);
}

/// Starts building the GPU pipelines in the background (`bosphorus_prover::warm_up`), for hosts
/// that live long enough to use them (`App::open`).
pub fn warm_up() {
    #[cfg(feature = "prove")]
    bosphorus_prover::warm_up();
}
