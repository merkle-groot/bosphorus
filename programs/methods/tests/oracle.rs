//! Essential `oracle` test (plan Phase 1): only the feed's publisher can publish.

mod common;

use common::{Chain, addr, key};
use bosphorus_methods::ORACLE_ELF;
use bosphorus_oracle::{Feed, Instruction, PUBLISH_WINDOW_MS, U256, feed_id};

#[test]
fn publishing_needs_the_publishers_signature() {
    let mut chain = Chain::new();
    let oracle = chain.deploy(ORACLE_ELF, &key(9));
    let (publisher, other) = (key(1), key(2));
    let feed = feed_id(&oracle, &addr(&publisher), "TKA/TKB");
    let accounts = |k: &lee::PrivateKey| vec![(addr(k), oracle), (feed, oracle)];
    chain
        .call(oracle, accounts(&publisher), &[&publisher], Instruction::CreateFeed { name: "TKA/TKB".into() })
        .unwrap();

    let at = chain.now;
    let publish = |price: u64| Instruction::Publish { price: U256::from(price), timestamp: at };
    assert!(chain.call(oracle, accounts(&publisher), &[], publish(5)).is_err(), "unsigned");
    assert!(chain.call(oracle, accounts(&other), &[&other], publish(5)).is_err(), "someone else signs");
    chain.call(oracle, accounts(&publisher), &[&publisher], publish(7)).unwrap();
    let read: Feed = borsh::from_slice(&chain.shard(feed, oracle)).unwrap();
    assert_eq!((read.price, read.updated_at, read.round), (U256::from(7u8), at, 1));

    // Replaying the same timestamp, post-dating, and publishing too late all fail.
    assert!(chain.call(oracle, accounts(&publisher), &[&publisher], publish(8)).is_err(), "not newer");
    let future = Instruction::Publish { price: U256::from(8u8), timestamp: at + 1 };
    assert!(chain.call(oracle, accounts(&publisher), &[&publisher], future).is_err(), "post-dated");
    chain.now += PUBLISH_WINDOW_MS + 1;
    let late = Instruction::Publish { price: U256::from(8u8), timestamp: at + 1 };
    assert!(chain.call(oracle, accounts(&publisher), &[&publisher], late).is_err(), "expired");
}
