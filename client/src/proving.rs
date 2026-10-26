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
