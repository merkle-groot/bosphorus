//! `lending` guest: plan/apply glue. `plan` checks accounts and authorisation, derives amounts
//! from the claims (`bosphorus_core::claims`), and emits the same deltas to every shard that needs
//! them; each `apply` checks the claims against its own shard and applies them (constraints §6).

use bosphorus_core::{claims, validate_lltv};
use bosphorus_lending::{
    AdminOp, Config, Effect, Event, HealthClaim, Instruction, MAX_ASSETS, MAX_IRMS, MAX_LLTVS, Market, MarketAccounts,
    MarketId, MarketOp, MarketParams, MarketState, Position, PositionOp, Salt, Shard, Window, collateral_vault_seed,
    config_id, loan_vault_seed, position_id, transfer,
};
use bosphorus_oracle::Feed;
use lee_core::{
    account::AccountId,
    program::{
        AccountMeta, PROGRAM_LOADER_ACCOUNT_ID, Plan, PlanInput, ProgramEvent, ProgramHeader, TimestampValidityWindow,
        run_program,
    },
};

fn main() {
    run_program(plan, apply)
}

fn ok<T>(result: bosphorus_core::Result<T>) -> T {
    result.unwrap_or_else(|e| panic!("{e}"))
}

/// Sets `[t, t + w)` and returns its end.
fn window(plan: &mut Plan, window: Window) -> u64 {
    let end = ok(window.end());
    plan.timestamp_window(TimestampValidityWindow::try_from(window.t..end).expect("w > 0"));
    end
}

fn event(plan: &mut Plan, event: Event) {
    plan.event(ProgramEvent { selector: event.selector(), data: borsh::to_vec(&event).unwrap() });
}

/// The market's accounts, after checking `market` is the PDA of these params.
fn market(this: &AccountId, params: &MarketParams, market: &AccountMeta) -> (MarketId, MarketAccounts) {
    let id = params.id();
    let accounts = MarketAccounts::new(this, &id);
    assert_eq!(market.account_id, accounts.market, "market account is not derived from these params");
    (id, accounts)
}

/// Design §4: an owner-only op needs an authorised `owner` whose `(owner, salt)` derives `position`.
fn owned(this: &AccountId, id: &MarketId, owner: &AccountMeta, position: &AccountMeta, salt: &Salt) {
    assert!(owner.is_authorized, "owner must authorise");
    assert_eq!(
        position.account_id,
        position_id(this, id, &owner.account_id, salt),
        "position is not derived from this owner and salt"
    );
}

/// Owned, or held on the owner's own account (E9: a lender keeps its shares in `shard[lending]` of
/// its private account; only `lending` writes that shard, constraints §1). Never for `Borrow`: the
/// holder of a private account's keys can re-version it, so held debt couldn't be liquidated
/// (Option 2 §2). Held accounts carry no debt, so withdrawals from them can't hurt anyone.
fn owned_or_held(this: &AccountId, id: &MarketId, owner: &AccountMeta, position: &AccountMeta, salt: &Salt) {
    if position.account_id == owner.account_id {
        assert!(owner.is_authorized, "owner must authorise");
    } else {
        owned(this, id, owner, position, salt);
    }
}

/// Optional trailing payout account; defaults to the owner.
fn payout(owner: &AccountMeta, rest: &[AccountMeta]) -> AccountId {
    match rest {
        [] => owner.account_id,
        [to] => to.account_id,
        _ => panic!("too many accounts"),
    }
}

fn price_check(plan: &mut Plan, params: &MarketParams, feed: &AccountMeta, health: &HealthClaim, end: u64) {
    assert_eq!(feed.account_id, params.oracle_feed, "wrong oracle feed");
    let check = Effect::Price { claim: health.price, max_age: params.max_price_age, window_end: end };
    plan.inspect(feed, params.oracle_program, &check);
}

fn admin(plan: &mut Plan, this: &AccountId, accounts: &[AccountMeta], op: AdminOp) {
    let [admin, config] = accounts else { panic!("takes [admin, config]") };
    assert!(admin.is_authorized, "admin must sign");
    assert_eq!(config.account_id, config_id(this), "wrong config account");
    plan.effect(config, &Effect::Admin { signer: admin.account_id, op });
}

fn plan(input: &PlanInput, instruction: Instruction) -> Plan {
    let this = &input.self_account_id;
    let accounts = input.accounts.as_slice();
    let mut plan = Plan::new(input);
    match instruction {
        Instruction::Init { admin, fee_recipient } => {
            let [instance, config] = accounts else { panic!("takes [instance, config]") };
            assert_eq!(instance.account_id, *this, "first account must be this program's header");
            assert!(instance.is_authorized, "the header account must sign init");
            assert_eq!(config.account_id, config_id(this), "wrong config account");
            let config_data =
                Config { admin, fee_recipient, enabled_lltvs: vec![], enabled_irms: vec![], enabled_assets: vec![] };
            plan.effect(config, &Effect::Init(config_data));
        }
        Instruction::EnableLltv { lltv } => {
            ok(validate_lltv(lltv));
            admin(&mut plan, this, accounts, AdminOp::EnableLltv(lltv));
        }
        Instruction::EnableIrm { irm } => {
            ok(irm.validate());
            admin(&mut plan, this, accounts, AdminOp::EnableIrm(irm));
        }
        Instruction::SetFeeRecipient { fee_recipient } => {
            admin(&mut plan, this, accounts, AdminOp::SetFeeRecipient(fee_recipient));
        }
        Instruction::EnableAsset { image_id } => {
            admin(&mut plan, this, accounts, AdminOp::EnableAsset(image_id));
        }
        Instruction::SetFee { params, fee, window: w } => {
            let [admin_account, config, market_account] = accounts else { panic!("takes [admin, config, market]") };
            admin(&mut plan, this, &[admin_account.clone(), config.clone()], AdminOp::Check);
            market(this, &params, market_account);
            plan.effect(market_account, &Effect::Market { t: w.t, op: MarketOp::SetFee { fee } });
            window(&mut plan, w);
        }
        Instruction::CreateMarket { params, window: w, images } => {
            let [config, market_account, headers @ ..] = accounts else {
                panic!("takes [config, market, header of each checked asset]")
            };
            ok(validate_lltv(params.lltv));
            ok(params.irm.validate());
            assert!(params.max_price_age > 0, "max_price_age must be positive");
            assert_eq!(config.account_id, config_id(this), "wrong config account");
            let (market_id, _) = market(this, &params, market_account);
            // Each non-native asset runs enabled, immutable bytecode: its header shows the image id
            // the config checks (findings F1).
            let assets = params.checked_assets();
            assert!(headers.len() == assets.len() && images.len() == assets.len(), "one header and image per checked asset");
            for ((asset, header), image_id) in assets.iter().zip(headers).zip(&images) {
                assert_eq!(header.account_id, *asset, "wrong asset header");
                plan.inspect(header, PROGRAM_LOADER_ACCOUNT_ID, &Effect::AssetHeader { image_id: *image_id });
            }
            plan.effect(config, &Effect::CheckEnabled { lltv: params.lltv, irm: params.irm, images });
            plan.effect(market_account, &Effect::CreateMarket { params: params.clone(), t: w.t });
            window(&mut plan, w);
            event(&mut plan, Event::MarketCreated { market_id, params });
        }
        Instruction::Supply { params, assets, rs, window: w } => {
            let [market_account, position, payer, loan_vault] = accounts else {
                panic!("takes [market, position, payer, loan_vault]")
            };
            let (market_id, m) = market(this, &params, market_account);
            assert_eq!(loan_vault.account_id, m.loan_vault, "wrong loan vault");
            let shares = ok(claims::supply_shares(assets, &rs));
            plan.effect(market_account, &Effect::Market { t: w.t, op: MarketOp::Supply { rs, assets, shares } });
            plan.effect(position, &Effect::Position { market_id, op: PositionOp::Supply { shares } });
            plan.call(transfer(params.loan_asset, payer.account_id, m.loan_vault, assets, None));
            window(&mut plan, w);
            event(&mut plan, Event::Supply { market_id, position: position.account_id, assets, shares });
        }
        Instruction::Withdraw { params, shares, salt, rs, window: w } => {
            let [owner, market_account, position, loan_vault, rest @ ..] = accounts else {
                panic!("takes [owner, market, position, loan_vault, (to)]")
            };
            let (market_id, m) = market(this, &params, market_account);
            owned_or_held(this, &market_id, owner, position, &salt);
            assert_eq!(loan_vault.account_id, m.loan_vault, "wrong loan vault");
            let assets = ok(claims::withdraw_assets(shares, &rs));
            plan.effect(market_account, &Effect::Market { t: w.t, op: MarketOp::Withdraw { rs, shares, assets } });
            plan.effect(position, &Effect::Position { market_id, op: PositionOp::Withdraw { shares } });
            let to = payout(owner, rest);
            plan.call(transfer(params.loan_asset, m.loan_vault, to, assets, Some(loan_vault_seed(&market_id))));
            window(&mut plan, w);
            event(&mut plan, Event::Withdraw { market_id, position: position.account_id, assets, shares });
        }
        Instruction::SupplyCollateral { params, assets } => {
            let [position, payer, collateral_vault] = accounts else {
                panic!("takes [position, payer, collateral_vault]")
            };
            let market_id = params.id();
            let m = MarketAccounts::new(this, &market_id);
            assert_eq!(collateral_vault.account_id, m.collateral_vault, "wrong collateral vault");
            assert!(assets > 0, "zero amount");
            plan.effect(position, &Effect::Position { market_id, op: PositionOp::SupplyCollateral { assets } });
            plan.call(transfer(params.collateral_asset, payer.account_id, m.collateral_vault, assets, None));
            event(&mut plan, Event::SupplyCollateral { market_id, position: position.account_id, assets });
        }
        Instruction::WithdrawCollateral { params, assets, salt, health } => {
            assert!(assets > 0, "zero amount");
            let market_id = params.id();
            let m = MarketAccounts::new(this, &market_id);
            let (owner, position, collateral_vault, rest) = match (&health, accounts) {
                (Some(claim), [owner, market_account, position, feed, collateral_vault, rest @ ..]) => {
                    market(this, &params, market_account);
                    let end = window(&mut plan, claim.window);
                    price_check(&mut plan, &params, feed, claim, end);
                    plan.effect(market_account, &Effect::Market { t: claim.window.t, op: MarketOp::Check { rb: claim.rb } });
                    (owner, position, collateral_vault, rest)
                }
                (None, [owner, position, collateral_vault, rest @ ..]) => (owner, position, collateral_vault, rest),
                (Some(_), _) => panic!("takes [owner, market, position, feed, collateral_vault, (to)]"),
                (None, _) => panic!("takes [owner, position, collateral_vault, (to)]"),
            };
            owned_or_held(this, &market_id, owner, position, &salt);
            assert_eq!(collateral_vault.account_id, m.collateral_vault, "wrong collateral vault");
            let health = health.map(|claim| ok(claims::health(&claim.price, &claim.rb, params.lltv)));
            plan.effect(position, &Effect::Position { market_id, op: PositionOp::WithdrawCollateral { assets, health } });
            let to = payout(owner, rest);
            let seed = Some(collateral_vault_seed(&market_id));
            plan.call(transfer(params.collateral_asset, m.collateral_vault, to, assets, seed));
            event(&mut plan, Event::WithdrawCollateral { market_id, position: position.account_id, assets });
        }
        Instruction::Borrow { params, assets, salt, health: claim } => {
            let [owner, market_account, position, feed, loan_vault, rest @ ..] = accounts else {
                panic!("takes [owner, market, position, feed, loan_vault, (to)]")
            };
            let (market_id, m) = market(this, &params, market_account);
            owned(this, &market_id, owner, position, &salt);
            assert_eq!(loan_vault.account_id, m.loan_vault, "wrong loan vault");
            let end = window(&mut plan, claim.window);
            let shares = ok(claims::borrow_shares(assets, &claim.rb));
            let health = ok(claims::health(&claim.price, &claim.rb, params.lltv));
            price_check(&mut plan, &params, feed, &claim, end);
            let op = MarketOp::Borrow { rb: claim.rb, assets, shares };
            plan.effect(market_account, &Effect::Market { t: claim.window.t, op });
            plan.effect(position, &Effect::Position { market_id, op: PositionOp::Borrow { shares, health } });
            let to = payout(owner, rest);
            plan.call(transfer(params.loan_asset, m.loan_vault, to, assets, Some(loan_vault_seed(&market_id))));
            event(&mut plan, Event::Borrow { market_id, position: position.account_id, assets, shares });
        }
        Instruction::Liquidate { params, claimed, amount, price, rb, window: w } => {
            let [liquidator, market_account, position, feed, loan_vault, collateral_vault, rest @ ..] = accounts else {
                panic!("takes [liquidator, market, position, feed, loan_vault, collateral_vault, (to)]")
            };
            let (market_id, m) = market(this, &params, market_account);
            assert_eq!(loan_vault.account_id, m.loan_vault, "wrong loan vault");
            assert_eq!(collateral_vault.account_id, m.collateral_vault, "wrong collateral vault");
            let liquidation = ok(claims::liquidation(amount, &claimed, &price, &rb, params.lltv));
            let health = ok(claims::liquidation_health(&price, &rb, params.lltv));
            let end = window(&mut plan, w);
            assert_eq!(feed.account_id, params.oracle_feed, "wrong oracle feed");
            plan.inspect(feed, params.oracle_program, &Effect::Price { claim: price, max_age: params.max_price_age, window_end: end });
            let op = MarketOp::Liquidate {
                rb,
                repaid_shares: liquidation.repaid_shares,
                repaid_assets: liquidation.repaid_assets,
                bad_shares: liquidation.bad_shares,
            };
            plan.effect(market_account, &Effect::Market { t: w.t, op });
            let op = PositionOp::Liquidate { claimed, health, liquidation };
            plan.effect(position, &Effect::Position { market_id, op });
            let l = liquidation;
            plan.call(transfer(params.loan_asset, liquidator.account_id, m.loan_vault, l.repaid_assets, None));
            if l.seized != 0 {
                let to = payout(liquidator, rest);
                let seed = Some(collateral_vault_seed(&market_id));
                plan.call(transfer(params.collateral_asset, m.collateral_vault, to, l.seized, seed));
            }
            event(&mut plan, Event::Liquidate { market_id, position: position.account_id, liquidation });
        }
        Instruction::ClaimFees { params, recipient, shares } => {
            let [config, market_account, position] = accounts else {
                panic!("takes [config, market, recipient_position]")
            };
            assert!(shares > 0, "zero amount");
            assert_eq!(config.account_id, config_id(this), "wrong config account");
            let (market_id, _) = market(this, &params, market_account);
            let expected = position_id(this, &market_id, &recipient, &[0; 32]);
            assert_eq!(position.account_id, expected, "not the recipient's position");
            plan.effect(config, &Effect::CheckFeeRecipient { recipient });
            // t = 0: no accrual (the shares claimed were read before this tx).
            plan.effect(market_account, &Effect::Market { t: 0, op: MarketOp::ClaimFees { shares } });
            plan.effect(position, &Effect::Position { market_id, op: PositionOp::Supply { shares } });
            event(&mut plan, Event::FeesClaimed { market_id, position: position.account_id, shares });
        }
        Instruction::Repay { params, shares, rb, window: w } => {
            let [market_account, position, payer, loan_vault] = accounts else {
                panic!("takes [market, position, payer, loan_vault]")
            };
            let (market_id, m) = market(this, &params, market_account);
            assert_eq!(loan_vault.account_id, m.loan_vault, "wrong loan vault");
            let assets = ok(claims::repay_assets(shares, &rb));
            plan.effect(market_account, &Effect::Market { t: w.t, op: MarketOp::Repay { rb, shares, assets } });
            plan.effect(position, &Effect::Position { market_id, op: PositionOp::Repay { shares } });
            plan.call(transfer(params.loan_asset, payer.account_id, m.loan_vault, assets, None));
            window(&mut plan, w);
            event(&mut plan, Event::Repay { market_id, position: position.account_id, assets, shares });
        }
    }
    plan
}

fn decode(pre_data: &[u8]) -> Shard {
    borsh::from_slice(pre_data).expect("not a lending account")
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

fn apply(effect: Effect, pre_data: &[u8]) -> Option<Vec<u8>> {
    match effect {
        Effect::Init(config) => {
            assert!(pre_data.is_empty(), "already initialised");
            encode(Shard::Config(config))
        }
        Effect::Admin { signer, op } => {
            let mut config = config(pre_data);
            assert_eq!(signer, config.admin, "not the admin");
            match op {
                AdminOp::EnableLltv(lltv) => {
                    assert!(!config.enabled_lltvs.contains(&lltv), "already enabled");
                    assert!(config.enabled_lltvs.len() < MAX_LLTVS, "too many LLTVs");
                    config.enabled_lltvs.push(lltv);
                }
                AdminOp::EnableIrm(irm) => {
                    assert!(!config.enabled_irms.contains(&irm), "already enabled");
                    assert!(config.enabled_irms.len() < MAX_IRMS, "too many IRMs");
                    config.enabled_irms.push(irm);
                }
                AdminOp::EnableAsset(image_id) => {
                    assert!(!config.enabled_assets.contains(&image_id), "already enabled");
                    assert!(config.enabled_assets.len() < MAX_ASSETS, "too many assets");
                    config.enabled_assets.push(image_id);
                }
                AdminOp::SetFeeRecipient(recipient) => config.fee_recipient = recipient,
                AdminOp::Check => return None,
            }
            encode(Shard::Config(config))
        }
        Effect::CheckEnabled { lltv, irm, images } => {
            let config = config(pre_data);
            assert!(config.enabled_lltvs.contains(&lltv), "LLTV not enabled");
            assert!(config.enabled_irms.contains(&irm), "IRM not enabled");
            assert!(images.iter().all(|i| config.enabled_assets.contains(i)), "asset bytecode not enabled");
            None
        }
        Effect::AssetHeader { image_id } => {
            let header = ProgramHeader::from_bytes(pre_data).expect("asset has no program header");
            assert_eq!(header.image_id, image_id, "asset runs other bytecode than claimed");
            assert!(header.immutable, "asset program can be updated");
            None
        }
        Effect::CheckFeeRecipient { recipient } => {
            assert_eq!(config(pre_data).fee_recipient, recipient, "not the fee recipient");
            None
        }
        Effect::CreateMarket { params, t } => {
            assert!(pre_data.is_empty(), "market already exists");
            encode(Shard::Market(Market { params, state: MarketState::new(t) }))
        }
        Effect::Market { t, op } => {
            let Shard::Market(mut market) = decode(pre_data) else { panic!("not a market") };
            ok(market.state.apply(&market.params.irm, t, &op));
            encode(Shard::Market(market))
        }
        Effect::Position { market_id, op } => {
            let mut position = if pre_data.is_empty() {
                Position { market_id, ..Position::default() }
            } else {
                let Shard::Position(position) = decode(pre_data) else { panic!("not a position") };
                position
            };
            assert_eq!(position.market_id, market_id, "position belongs to another market");
            ok(position.apply(&op));
            encode(Shard::Position(position))
        }
        Effect::Price { claim, max_age, window_end } => {
            let feed: Feed = borsh::from_slice(pre_data).expect("not an oracle feed");
            ok(claims::check_price(&claim, feed.price, feed.updated_at, max_age, window_end));
            None
        }
    }
}
