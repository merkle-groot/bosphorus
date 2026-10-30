//! Essential solvency-proof tests (E11), executing the real `disclosure` guest. Authorisation: only
//! the holder of the spending key behind a listed position can prove, only for a position in the
//! cover set and only if it meets the claims; the journal names the request and the prover's id.
//!
//! `cargo test -p bosphorus-methods --release --test disclosure`. Stress (cycles by cover size)
//! and a real proof, timed: add `--features prove -- --ignored --nocapture`.

use std::time::Instant;

use bosphorus_core::{PRICE_SCALE, Position, WAD};
use bosphorus_disclosure::{Claims, Header, Journal, Request, Valuation, Witness, disclosure_id, position_salt};
use bosphorus_lending::position_id;
use bosphorus_methods::{DISCLOSURE_ELF, DISCLOSURE_ID};
use lee::AccountId;
use risc0_zkvm::{ExecutorEnv, default_executor};

const LENDING: [u8; 32] = [2; 32];
const MARKET: [u8; 32] = [7; 32];
const ALICE_KEY: [u8; 32] = [11; 32];
const ALICE: [u8; 32] = [12; 32];
const MALLORY_KEY: [u8; 32] = [21; 32];
const MALLORY: [u8; 32] = [22; 32];

fn position_of(secret: &[u8; 32], owner: &[u8; 32], index: u32) -> [u8; 32] {
    let salt = position_salt(secret, &MARKET, owner, index);
    position_id(&AccountId::new(LENDING), &MARKET, &AccountId::new(*owner), &salt).into_value()
}

/// Price 1, ratios 1, LLTV 80%: `collateral` 100 backs 80 of debt.
fn pos(collateral: u128, borrow_shares: u128) -> Position {
    Position { market_id: MARKET, supply_shares: 0, borrow_shares, collateral }
}

/// Alice's position 0 (HF 1.6) and position 1 (HF 1.0) among `others` filler positions (HF 2).
fn request(others: usize, min_health: u128) -> Request {
    let mut positions = vec![(position_of(&ALICE_KEY, &ALICE, 0), pos(100, 50)), (position_of(&ALICE_KEY, &ALICE, 1), pos(100, 80))];
    for i in 0..others {
        let mut id = [0u8; 32];
        id[..8].copy_from_slice(&(i as u64).to_le_bytes());
        id[31] = 0xee;
        positions.push((id, pos(100, 40)));
    }
    positions.sort_by_key(|(id, _)| *id);
    Request {
        header: Header {
            label: "audit".into(),
            nonce: [5; 32],
            lending: LENDING,
            market_id: MARKET,
            valuation: Valuation { price: PRICE_SCALE, rb: WAD, rs: WAD, lltv: WAD / 10 * 8, at_ms: 1 },
            claims: Claims { min_health: Some(min_health), ..Default::default() },
        },
        positions,
    }
}

fn run(witness: &Witness) -> anyhow::Result<(Journal, u64)> {
    let bytes = borsh::to_vec(witness)?;
    let env = ExecutorEnv::builder().write(&(bytes.len() as u32))?.write_slice(&bytes).build()?;
    let session = default_executor().execute(env, DISCLOSURE_ELF)?;
    Ok((borsh::from_slice(&session.journal.bytes)?, session.cycles()))
}

/// The witness an owner would build: for `claimed` (default: the position the keys derive), with
/// the request's real path. Falls back to the first listed position when `claimed` isn't listed.
fn witness(request: Request, secret: [u8; 32], owner: [u8; 32], index: u32) -> Witness {
    let claimed = position_of(&secret, &owner, index);
    Witness::new(&request, claimed, secret, owner, index)
        .unwrap_or_else(|| Witness::new(&request, request.positions[0].0, secret, owner, index).unwrap())
}

#[test]
fn owner_proves_and_the_journal_names_request_and_id() {
    let r = request(5, WAD / 2 * 3);
    let (journal, _) = run(&witness(r.clone(), ALICE_KEY, ALICE, 0)).unwrap();
    assert_eq!(journal, Journal { request: r.digest(), disclosure_id: disclosure_id(&ALICE_KEY) });
}

#[test]
fn another_key_cant_prove_someone_elses_position() {
    let r = request(5, WAD);
    let alices = position_of(&ALICE_KEY, &ALICE, 0);
    // Mallory claims Alice's position, naming Alice as owner: her key gives another salt.
    assert!(run(&Witness::new(&r, alices, MALLORY_KEY, ALICE, 0).unwrap()).is_err());
    // ... or naming herself.
    assert!(run(&Witness::new(&r, alices, MALLORY_KEY, MALLORY, 0).unwrap()).is_err());
}

#[test]
fn position_must_be_in_the_cover_set() {
    // A path through another set (one filler changed) leads to another root: another request.
    let r = request(5, WAD);
    let mut other = r.clone();
    other.positions.last_mut().unwrap().1.collateral += 1;
    let w = witness(other, ALICE_KEY, ALICE, 0);
    let (journal, _) = run(&w).unwrap();
    assert_ne!(journal.request, r.digest(), "proves a different set, so not this request");
    // Contents that differ from the listed ones change the leaf: not this request either.
    let mut w = witness(r.clone(), ALICE_KEY, ALICE, 0);
    w.position.collateral = 1_000;
    assert_ne!(run(&w).unwrap().0.request, r.digest());
    // Outside the set.
    let mut w = witness(r, ALICE_KEY, ALICE, 0);
    w.at = w.n;
    assert!(run(&w).is_err());
}

#[test]
fn claims_must_hold() {
    // Position 1 is at HF 1.0: fails 1.5, passes 1.0.
    assert!(run(&witness(request(5, WAD / 2 * 3), ALICE_KEY, ALICE, 1)).is_err());
    assert!(run(&witness(request(5, WAD), ALICE_KEY, ALICE, 1)).is_ok());
    let mut r = request(5, WAD);
    r.header.claims.min_collateral = Some(101);
    assert!(run(&witness(r, ALICE_KEY, ALICE, 0)).is_err());
}

#[test]
fn malformed_requests_fail() {
    let mut r = request(5, WAD);
    r.header.claims = Claims::default();
    assert!(run(&witness(r, ALICE_KEY, ALICE, 0)).is_err(), "no claims");
    let mut r = request(5, WAD);
    let mine = position_of(&ALICE_KEY, &ALICE, 0);
    r.positions.iter_mut().find(|(id, _)| *id == mine).unwrap().1.market_id = [8; 32];
    assert!(run(&witness(r, ALICE_KEY, ALICE, 0)).is_err(), "position from another market");
}

/// Cycles by cover-set size (stress): the guest hashes the whole request.
#[test]
#[ignore]
fn stress_cycles_by_cover_size() {
    for n in [10, 1_000, 100_000, 1_000_000] {
        let r = request(n, WAD);
        let size = borsh::to_vec(&r).unwrap().len();
        let started = Instant::now();
        let w = witness(r, ALICE_KEY, ALICE, 0);
        let built = started.elapsed().as_secs_f64();
        let (_, cycles) = run(&w).unwrap();
        println!("cover {n:>6}: request {size:>8} B, witness built in {built:.2} s, {cycles:>8} cycles");
    }
}

/// A real succinct proof (GPU with `--features prove`), verified, and tampering caught.
#[test]
#[ignore]
fn real_proof_verifies_and_tampering_fails() {
    use risc0_zkvm::{ProverOpts, default_prover};
    for n in [100, 10_000, 100_000] {
        let r = request(n, WAD / 2 * 3);
        let bytes = borsh::to_vec(&witness(r.clone(), ALICE_KEY, ALICE, 0)).unwrap();
        let cycles = run(&witness(r.clone(), ALICE_KEY, ALICE, 0)).unwrap().1;
        let env = ExecutorEnv::builder().write(&(bytes.len() as u32)).unwrap().write_slice(&bytes).build().unwrap();
        let started = Instant::now();
        let receipt = default_prover().prove_with_opts(env, DISCLOSURE_ELF, &ProverOpts::succinct()).unwrap().receipt;
        let size = bincode::serialize(&receipt).unwrap().len();
        println!("cover {n:>6}: {cycles} cycles, proved in {:.1} s, receipt {size} B", started.elapsed().as_secs_f64());
        receipt.verify(DISCLOSURE_ID).unwrap();
        let journal: Journal = borsh::from_slice(&receipt.journal.bytes).unwrap();
        assert_eq!(journal.request, r.digest());

        let mut forged = receipt.clone();
        forged.journal.bytes[0] ^= 1;
        assert!(forged.verify(DISCLOSURE_ID).is_err(), "a changed journal must not verify");
        let mut other_image = DISCLOSURE_ID;
        other_image[0] ^= 1;
        assert!(receipt.verify(other_image).is_err(), "another image id must not verify");
    }
}
