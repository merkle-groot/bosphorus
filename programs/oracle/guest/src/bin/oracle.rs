use lee_core::{
    account::ShardData,
    program::{Plan, PlanInput, TimestampValidityWindow, run_program},
};
use bosphorus_oracle::{Effect, Feed, Instruction, MAX_NAME_LEN, PUBLISH_WINDOW_MS, feed_id};

fn main() {
    run_program(plan, apply)
}

fn plan(input: &PlanInput, instruction: Instruction) -> Plan {
    let this = input.self_account_id;
    let [publisher, feed] = input.accounts.as_slice() else { panic!("takes [publisher, feed]") };
    assert!(publisher.is_authorized, "publisher must sign");
    assert_eq!(feed.program_account_id, this, "feed must select this oracle's shard");
    let publisher_id = publisher.account_id;
    let mut plan = Plan::new(input);
    match instruction {
        Instruction::CreateFeed { name } => {
            assert!(name.len() <= MAX_NAME_LEN, "feed name too long");
            assert_eq!(feed.account_id, feed_id(&this, &publisher_id, &name), "wrong feed account");
            plan.effect(feed, &Effect::Create { publisher: publisher_id });
        }
        Instruction::Publish { price, timestamp } => {
            assert!(!price.is_zero(), "zero price");
            let end = timestamp.checked_add(PUBLISH_WINDOW_MS).expect("timestamp overflow");
            plan.timestamp_window(TimestampValidityWindow::try_from(timestamp..end).unwrap());
            plan.effect(feed, &Effect::Publish { publisher: publisher_id, price, timestamp });
        }
    }
    plan
}

fn apply(effect: Effect, pre_data: &[u8]) -> Option<ShardData> {
    let feed = match effect {
        Effect::Create { publisher } => {
            assert!(pre_data.is_empty(), "feed already exists");
            Feed { price: Default::default(), updated_at: 0, round: 0, publisher }
        }
        Effect::Publish { publisher, price, timestamp } => {
            let feed: Feed = borsh::from_slice(pre_data).expect("no feed here");
            assert_eq!(publisher, feed.publisher, "not this feed's publisher");
            assert!(timestamp > feed.updated_at, "stale publish");
            Feed { price, updated_at: timestamp, round: feed.round + 1, publisher }
        }
    };
    Some(ShardData::try_from(borsh::to_vec(&feed).unwrap()).expect("a feed fits a shard"))
}
