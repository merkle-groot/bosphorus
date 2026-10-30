//! `pool` guest: plan/apply glue (E10). `plan` checks accounts and authorisation, verifies each
//! spent label's Merkle path against the claimed root, and emits the same lots to every shard
//! that needs them; each `apply` checks them against its own shard (constraints §6).

use bosphorus_provenance::{MAX_REMOVE, MAX_SPENDS, push_root, verify};
use bosphorus_pool::{
    Config, DepositRecord, Effect, Instruction, Label, Lot, Lots, Shard, approved_id, config_id, decode_lots, deposit_label,
    encode_lots, escrow_id, escrow_seed, record_id, sha256, transfer,
};
use lee_core::{
    account::AccountId,
    program::{AccountMeta, Plan, PlanInput, run_program},
};

fn main() {
    run_program(plan, apply)
}

fn ok<T>(result: bosphorus_provenance::Result<T>) -> T {
    result.unwrap_or_else(|e| panic!("{e}"))
}

fn plan(input: &PlanInput, instruction: Instruction) -> Plan {
    let this = &input.self_account_id;
    let accounts = input.accounts.as_slice();
    let mut plan = Plan::new(input);
    match instruction {
        Instruction::Init { asp } => {
            let [instance, config, approved] = accounts else { panic!("takes [instance, config, approved]") };
            assert_eq!(instance.account_id, *this, "first account must be this program's header");
            assert!(instance.is_authorized, "the header account must sign init");
            assert_eq!(config.account_id, config_id(this), "wrong config account");
            assert_eq!(approved.account_id, approved_id(this), "wrong approved account");
            plan.effect(config, &Effect::Init(Config { asp, roots: vec![] }));
            plan.effect(approved, &Effect::InitApproved);
        }
        Instruction::Approve { labels, root } => {
            let [asp, config, approved] = asp_accounts(this, accounts, &labels);
            plan.effect(config, &Effect::PushRoot { signer: asp.account_id, root, flush: false });
            plan.effect(approved, &Effect::Approve { labels, root });
        }
        Instruction::Remove { removals, root } => {
            let labels: Vec<Label> = removals.iter().map(|r| r.label).collect();
            assert!(removals.len() <= MAX_REMOVE, "remove at most {MAX_REMOVE} labels at once");
            let [asp, config, approved] = asp_accounts(this, accounts, &labels);
            plan.effect(config, &Effect::PushRoot { signer: asp.account_id, root, flush: true });
            plan.effect(approved, &Effect::Remove { removals, root });
        }
        Instruction::Deposit { asset, amount, nonce } => {
            let [depositor, record, holder, escrow] = accounts else {
                panic!("takes [depositor, record, holder, escrow]")
            };
            assert!(depositor.is_authorized, "depositor must sign");
            assert!(amount > 0, "zero amount");
            let label = deposit_label(this, &asset, &depositor.account_id, &nonce);
            assert_eq!(record.account_id, record_id(this, &label), "record is not derived from this deposit");
            assert_eq!(escrow.account_id, escrow_id(this, &asset), "wrong escrow");
            let record_data = DepositRecord { label, asset, depositor: depositor.account_id, amount, exited: false };
            plan.effect(record, &Effect::Record(record_data));
            plan.effect(holder, &Effect::Credit(Lot { asset: *asset.value(), label, amount }));
            plan.call(transfer(asset, depositor.account_id, escrow.account_id, amount, None));
        }
        Instruction::Withdraw { asset, spends, root } => {
            let [holder, config, escrow, to] = accounts else { panic!("takes [holder, config, escrow, to]") };
            assert!(holder.is_authorized, "holder must sign");
            assert_eq!(config.account_id, config_id(this), "wrong config account");
            assert_eq!(escrow.account_id, escrow_id(this, &asset), "wrong escrow");
            assert!(!spends.is_empty() && spends.len() <= MAX_SPENDS, "spend 1 to {MAX_SPENDS} lots");
            let mut total: u128 = 0;
            for spend in &spends {
                assert!(spend.amount > 0, "zero amount");
                ok(verify(sha256, &spend.label, &spend.path, &root));
                total = total.checked_add(spend.amount).expect("amount overflow");
            }
            let spends = spends.into_iter().map(|s| (s.label, s.amount)).collect();
            plan.effect(holder, &Effect::Debit { asset, spends });
            plan.effect(config, &Effect::CheckRoot { root });
            plan.call(transfer(asset, escrow.account_id, to.account_id, total, Some(escrow_seed(&asset))));
        }
        Instruction::Ragequit { asset, label, amount } => {
            let [depositor, record, holder, escrow] = accounts else {
                panic!("takes [depositor, record, holder, escrow]")
            };
            assert!(depositor.is_authorized, "depositor must sign");
            assert!(holder.is_authorized, "holder must sign");
            assert!(amount > 0, "zero amount");
            assert_eq!(record.account_id, record_id(this, &label), "record is not this label's");
            assert_eq!(escrow.account_id, escrow_id(this, &asset), "wrong escrow");
            plan.effect(record, &Effect::Exit { depositor: depositor.account_id, asset });
            plan.effect(holder, &Effect::Take(Lot { asset: *asset.value(), label, amount }));
            plan.call(transfer(asset, escrow.account_id, depositor.account_id, amount, Some(escrow_seed(&asset))));
        }
    }
    plan
}

/// `[asp (signs), config, approved]`.
fn asp_accounts<'a>(this: &AccountId, accounts: &'a [AccountMeta], labels: &[Label]) -> [&'a AccountMeta; 3] {
    let [asp, config, approved] = accounts else { panic!("takes [asp, config, approved]") };
    assert!(asp.is_authorized, "ASP must sign");
    assert!(!labels.is_empty(), "no labels");
    assert_eq!(config.account_id, config_id(this), "wrong config account");
    assert_eq!(approved.account_id, approved_id(this), "wrong approved account");
    [asp, config, approved]
}

fn decode(pre_data: &[u8]) -> Shard {
    borsh::from_slice(pre_data).expect("not a pool account")
}

fn encode(shard: Shard) -> Option<Vec<u8>> {
    Some(borsh::to_vec(&shard).unwrap())
}

fn config(pre_data: &[u8]) -> Config {
    match decode(pre_data) {
        Shard::Config(config) => config,
        _ => panic!("not the config"),
    }
}

fn lots(pre_data: &[u8]) -> Lots {
    decode_lots(pre_data).unwrap_or_else(|e| panic!("{e}"))
}

fn apply(effect: Effect, pre_data: &[u8]) -> Option<Vec<u8>> {
    match effect {
        Effect::Init(config) => {
            assert!(pre_data.is_empty(), "already initialised");
            encode(Shard::Config(config))
        }
        Effect::InitApproved => {
            assert!(pre_data.is_empty(), "already initialised");
            encode(Shard::Approved(Default::default()))
        }
        Effect::PushRoot { signer, root, flush } => {
            let mut config = config(pre_data);
            assert_eq!(signer, config.asp, "not the ASP");
            push_root(&mut config.roots, root, flush);
            encode(Shard::Config(config))
        }
        Effect::Approve { labels, root } => {
            let Shard::Approved(mut approved) = decode(pre_data) else { panic!("not the approved list") };
            ok(approved.approve(sha256, &labels));
            assert_eq!(approved.root, root, "root doesn't match the approved list");
            encode(Shard::Approved(approved))
        }
        Effect::Remove { removals, root } => {
            let Shard::Approved(mut approved) = decode(pre_data) else { panic!("not the approved list") };
            for removal in &removals {
                ok(approved.remove(sha256, removal));
            }
            assert_eq!(approved.root, root, "root doesn't match the approved list");
            encode(Shard::Approved(approved))
        }
        Effect::CheckRoot { root } => {
            assert!(config(pre_data).roots.contains(&root), "root not accepted (stale, or a label was removed)");
            None
        }
        Effect::Record(record) => {
            assert!(pre_data.is_empty(), "deposit already recorded (reused nonce)");
            encode(Shard::Record(record))
        }
        Effect::Exit { depositor, asset } => {
            let Shard::Record(mut record) = decode(pre_data) else { panic!("not a deposit record") };
            assert_eq!(record.depositor, depositor, "not the depositor");
            assert_eq!(record.asset, asset, "deposit was of another asset");
            assert!(!record.exited, "already taken back");
            record.exited = true;
            encode(Shard::Record(record))
        }
        Effect::Credit(lot) => {
            let mut lots = lots(pre_data);
            ok(lots.credit(lot));
            Some(encode_lots(lots))
        }
        Effect::Debit { asset, spends } => {
            let mut lots = lots(pre_data);
            for (label, amount) in spends {
                ok(lots.debit(asset.value(), &label, amount));
            }
            Some(encode_lots(lots))
        }
        Effect::Take(lot) => {
            let mut lots = lots(pre_data);
            ok(lots.take(&lot.asset, &lot.label, lot.amount));
            Some(encode_lots(lots))
        }
    }
}
