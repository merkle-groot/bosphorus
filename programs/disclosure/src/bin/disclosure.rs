//! Proves that one position of the request's cover set belongs to whoever holds `secret`, and
//! that it meets the request's claims. Commits only the request's digest and the disclosure id.
//! The cover set comes in as a Merkle root, so the cost doesn't grow with the market.

use bosphorus_disclosure::{Journal, Witness, check, digest, disclosure_id, leaf, position_salt, root_from_path};
use bosphorus_lending::position_id;
use lee_core::account::AccountId;
use risc0_zkvm::guest::env;

fn main() {
    let len: u32 = env::read();
    let mut bytes = vec![0u8; len as usize];
    env::read_slice(&mut bytes);
    let w: Witness = borsh::from_slice(&bytes).expect("bad witness");
    let h = &w.header;
    h.validate().expect("bad request");
    assert!(w.at < w.n, "position outside the cover set");
    assert_eq!(w.position.market_id, h.market_id, "a position from another market");

    // The address only the holder of `secret` can derive (design §4): another key gives another salt.
    let salt = position_salt(&w.secret, &h.market_id, &w.owner, w.index);
    let id = position_id(&AccountId::new(h.lending), &h.market_id, &AccountId::new(w.owner), &salt);
    assert_eq!(*id.value(), w.position_id, "not this key's position");
    check(&h.valuation, &h.claims, &w.position).expect("claims not met");

    // In the cover set: the path leads to the root the request's digest names.
    let root = root_from_path(leaf(&w.position_id, &w.position), w.at, &w.path);
    let journal = Journal { request: digest(h, &root, w.n), disclosure_id: disclosure_id(&w.secret) };
    env::commit_slice(&borsh::to_vec(&journal).unwrap());
}
