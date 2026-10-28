include!(concat!(env!("OUT_DIR"), "/methods.rs"));

// Token and oracle: the bytes deployed on the testnet (`DEPLOYMENTS.md`, built from `e881180`), not
// a fresh build. The workspace's risc0 `[patch]` entries reach the guests too and change their
// image ids with no source change (spikes/vendor/README.md, "Guest image ids drifted"). Neither
// source has changed since; to change one, put its guest back in `[package.metadata.risc0]` and
// deploy the new build.
pub const TOKEN_ELF: &[u8] = include_bytes!("../deployed/token.bin");
pub const ORACLE_ELF: &[u8] = include_bytes!("../deployed/oracle.bin");
