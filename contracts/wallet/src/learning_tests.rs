use super::*;
use near_sdk::mock::MockAction;
use near_sdk::test_utils::{VMContextBuilder, get_created_receipts, get_logs};
use near_sdk::testing_env;

const TS: u64 = 1_100_000_000_000_000;
fn account(name: &str) -> AccountId {
    format!("{name}.testnet").parse().unwrap()
}
fn key(n: u8) -> PublicKey {
    PublicKey::try_from([vec![0], vec![n; 32]].concat()).unwrap()
}
fn context(who: &str, n: u8, ts: u64) {
    let mut c = VMContextBuilder::new();
    c.current_account_id(account("wallet"))
        .signer_account_id(account(who))
        .predecessor_account_id(account(who))
        .signer_account_pk(key(n))
        .block_timestamp(ts)
        .account_balance(NearToken::from_near(10))
        .prepaid_gas(Gas::from_tgas(100));
    testing_env!(c.build());
}
fn owner() {
    context("owner", 1, TS);
}
fn agent() {
    context("wallet", 7, TS);
}
fn watcher() {
    context("watcher", 8, TS);
}
fn setup() -> Contract {
    owner();
    let config = LearningConfig {
        observe_single_cap: U128(100),
        min_observations: 3,
        min_count: 3,
        watcher: account("watcher"),
        watcher_key: key(8),
        watcher_expires_at_ns: U64(TS + 10 * NS_PER_DAY),
        watcher_max_calls: 100,
        pending_timeout_ns: U64(NS_PER_DAY),
        tasks: vec![TaskCondition {
            task_id: U64(1),
            peer: account("usual"),
            method: PaymentMethod::Pay,
            max_amount: U128(250),
            budget: U128(400),
            expires_at_ns: U64(TS + 10 * NS_PER_DAY),
        }],
    };
    let mut c = Contract::init(account("owner"), U128(10_000), Some(config));
    c.register_key(key(7), U64(TS + 10 * NS_PER_DAY), 1000);
    c.allow_method("ft_transfer".into());
    c
}
fn pay(c: &mut Contract, peer: &str, amount: u128) -> PaymentResult {
    c.request_payment(account(peer), U128(amount), PaymentMethod::Pay, U64(1))
}
fn learned() -> Contract {
    let mut c = setup();
    for _ in 0..3 {
        agent();
        pay(&mut c, "usual", 100);
    }
    owner();
    let p = c.propose_policy(2000, 3);
    owner();
    c.approve_policy(p.hash);
    c
}
fn pending(c: &mut Contract, amount: u128) -> U64 {
    agent();
    pay(c, "usual", amount).id.unwrap()
}
fn rejects(code: &str, f: impl FnOnce()) {
    let err =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).expect_err("expected rejection");
    let message = err
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| err.downcast_ref::<&str>().copied())
        .unwrap_or("");
    assert!(message.contains(code), "{message}");
    assert!(get_created_receipts().is_empty());
}

#[test]
fn observation_records_numeric_rows_and_keeps_methods_separate() {
    let mut c = setup();
    agent();
    pay(&mut c, "usual", 20);
    agent();
    drop(c.transfer(account("usual"), U128(30)));
    agent();
    drop(c.call(
        account("usual"),
        "ft_transfer".into(),
        Base64VecU8(b"{}".to_vec()),
        U128(10),
        U64(10_000_000_000_000),
    ));
    let s = c.get_stats();
    assert_eq!(s.mode, Mode::Observe);
    assert_eq!(s.rows.len(), 2);
    assert_eq!(
        (s.rows[0].count, s.rows[0].sum.0, s.rows[0].max_amount.0),
        (2, 50, 30)
    );
    assert_eq!(s.rows[1].method, ObservedMethod::Call("ft_transfer".into()));
    assert_eq!(
        (s.observations, s.max_daily.0, c.get_spent_today().0),
        (3, 60, 60)
    );
    assert_eq!(s.rows[0].first_ts, U64(TS));
}

#[test]
fn observation_rejects_row_seventeen_without_losing_existing_rows() {
    let mut c = setup();
    for i in 0..16 {
        agent();
        pay(&mut c, &format!("peer{i}"), 1);
    }
    agent();
    rejects("StatsFull", || {
        pay(&mut c, "extra", 1);
    });
    assert_eq!(c.get_stats().rows.len(), 16);
    assert_eq!(c.get_spent_today().0, 16);
    agent();
    pay(&mut c, "peer0", 1);
    assert_eq!(c.get_stats().rows[0].count, 2);
}

#[test]
fn observation_caps_daily_rollover_pause_and_revoke_remain_effective() {
    let mut c = setup();
    agent();
    rejects("ObserveLimit", || {
        pay(&mut c, "usual", 101);
    });
    owner();
    c.set_daily_limit(U128(150));
    agent();
    pay(&mut c, "usual", 100);
    agent();
    rejects("DailyLimit", || {
        pay(&mut c, "usual", 51);
    });
    context("wallet", 7, (utc_day(TS) + 1) * NS_PER_DAY);
    pay(&mut c, "usual", 50);
    assert_eq!(c.get_stats().max_daily.0, 100);
    owner();
    c.pause();
    agent();
    rejects("Paused", || {
        pay(&mut c, "usual", 1);
    });
    owner();
    c.unpause();
    c.revoke_key(key(7));
    agent();
    rejects("KeyInactive", || {
        pay(&mut c, "usual", 1);
    });
    assert_eq!(c.get_stats().observations, 2);
}

#[test]
fn proposal_requires_owner_and_non_owner_cannot_replace_approval_hash() {
    let mut c = setup();
    for _ in 0..3 {
        agent();
        pay(&mut c, "usual", 10);
    }
    for (who, n) in [("stranger", 9), ("wallet", 7), ("watcher", 8)] {
        context(who, n, TS);
        rejects("Unauthorized", || {
            c.propose_policy(2000, 3);
        });
        assert_eq!(c.get_stats().mode, Mode::Observe);
        assert!(c.get_stats().proposal.is_none());
        assert_eq!(c.get_stats().observations, 3);
        assert!(get_logs().is_empty());
    }
    owner();
    let p = c.propose_policy(2000, 3);
    assert_eq!(c.get_stats().mode, Mode::Proposed);
    for (who, n) in [("stranger", 9), ("wallet", 7), ("watcher", 8)] {
        context(who, n, TS);
        rejects("Unauthorized", || {
            c.propose_policy(1000, 3);
        });
        assert_eq!(c.get_stats().mode, Mode::Proposed);
        assert_eq!(c.get_stats().proposal.unwrap().hash, p.hash);
        assert!(get_logs().is_empty());
    }
    owner();
    c.approve_policy(p.hash);
    assert_eq!(c.get_stats().mode, Mode::Enforce);
}

#[test]
fn proposal_and_approval_events_include_hash_peers_and_caps() {
    let mut c = setup();
    for _ in 0..3 {
        agent();
        pay(&mut c, "usual", 20);
        agent();
        drop(c.call(
            account("other"),
            "ft_transfer".into(),
            Base64VecU8(vec![]),
            U128(5),
            U64(10_000_000_000_000),
        ));
    }
    owner();
    let p = c.propose_policy(2000, 3);
    let proposed_logs = get_logs();
    owner();
    c.approve_policy(p.hash.clone());
    let approved_logs = get_logs();
    for (logs, event) in [
        (proposed_logs, "policy_proposed"),
        (approved_logs, "policy_approved"),
    ] {
        assert_eq!(logs.len(), 1);
        let payload: near_sdk::serde_json::Value =
            near_sdk::serde_json::from_str(logs[0].strip_prefix("EVENT_JSON:").unwrap()).unwrap();
        assert_eq!(payload["standard"], EVENT_STANDARD);
        assert_eq!(payload["version"], EVENT_VERSION);
        assert_eq!(payload["event"], event);
        assert_eq!(
            payload["data"],
            near_sdk::serde_json::json!([{
                "amount": "90",
                "rule": "Ok",
                "hash": p.hash,
                "peers": [
                    {"peer": "usual.testnet", "method": "Pay", "cap": "24"},
                    {"peer": "other.testnet", "method": {"Call": "ft_transfer"}, "cap": "6"},
                ],
                "daily_cap": "90",
            }])
        );
        assert!(payload["data"][0]["hash"].as_str().is_some());
    }
}

#[test]
fn proposal_filters_rare_pairs_rounds_up_and_hashes_deterministically() {
    let mut c = setup();
    for n in [1, 7, 11] {
        agent();
        pay(&mut c, "usual", n);
    }
    agent();
    pay(&mut c, "rare", 99);
    agent();
    drop(c.call(
        account("usual"),
        "ft_transfer".into(),
        Base64VecU8(vec![]),
        U128(5),
        U64(10_000_000_000_000),
    ));
    owner();
    let p = c.propose_policy(2000, 3);
    assert_eq!(p.rows.len(), 1);
    assert_eq!(p.rows[0].peer, account("usual"));
    assert_eq!(p.rows[0].cap.0, 14); // ceil(11 * 1.2)
    assert_eq!(p.daily_cap.0, 148); // ceil(123 * 1.2)
    assert_eq!(p.hash.0.len(), 32);
    assert_eq!(c.propose_policy(2000, 3).hash.0, p.hash.0);
    assert_ne!(c.propose_policy(1000, 3).hash.0, p.hash.0);
    agent();
    rejects("PolicyProposed", || {
        pay(&mut c, "usual", 1);
    });
}

#[test]
fn proposal_rejects_insufficient_samples_invalid_margins_and_empty_allowlists() {
    let mut c = setup();
    owner();
    rejects("InvalidProposal", || {
        c.propose_policy(2000, 3);
    });
    for _ in 0..3 {
        agent();
        pay(&mut c, "usual", 10);
    }
    for (margin, count) in [(10_001, 3), (2000, 2), (2000, 4)] {
        owner();
        rejects("InvalidProposal", || {
            c.propose_policy(margin, count);
        });
    }
    assert_eq!(c.get_stats().mode, Mode::Observe);
}

#[test]
fn approval_requires_owner_exact_hash_and_only_occurs_once() {
    let mut c = setup();
    for _ in 0..3 {
        agent();
        pay(&mut c, "usual", 10);
    }
    owner();
    let p = c.propose_policy(2000, 3);
    agent();
    rejects("Unauthorized", || c.approve_policy(p.hash.clone()));
    owner();
    rejects("InvalidProposal", || {
        c.approve_policy(Base64VecU8(vec![0; 32]))
    });
    assert_eq!(c.get_stats().mode, Mode::Proposed);
    c.approve_policy(p.hash.clone());
    owner();
    rejects("InvalidProposal", || c.approve_policy(p.hash));
    owner();
    rejects("InvalidProposal", || {
        c.propose_policy(2000, 3);
    });
    assert_eq!(c.get_stats().mode, Mode::Enforce);
}

#[test]
fn approved_policy_blocks_unknown_peer_and_legacy_transfer_bypass() {
    let mut c = learned();
    owner();
    c.allow_recipient(account("unknown"));
    agent();
    rejects("ERR_NOT_IN_LEARNED_POLICY", || {
        drop(c.transfer(account("unknown"), U128(1)));
    });
    agent();
    rejects("ERR_NOT_IN_LEARNED_POLICY", || {
        drop(c.transfer(account("usual"), U128(121)));
    });
    agent();
    pay(&mut c, "usual", 50);
    assert_eq!(c.get_spent_today().0, 350);
    assert_eq!(c.get_stats().observations, 3);
    owner();
    c.deny_recipient(account("usual"));
    agent();
    rejects("NotAllowlisted", || {
        pay(&mut c, "usual", 200);
    });
}

#[test]
fn learned_methods_are_per_peer_and_external_calls_cannot_bypass_caps() {
    let mut c = setup();
    for _ in 0..3 {
        agent();
        drop(c.call(
            account("usual"),
            "ft_transfer".into(),
            Base64VecU8(vec![]),
            U128(10),
            U64(10_000_000_000_000),
        ));
    }
    owner();
    let p = c.propose_policy(2000, 3);
    owner();
    c.approve_policy(p.hash);
    agent();
    rejects("ERR_NOT_IN_LEARNED_POLICY", || {
        drop(c.transfer(account("usual"), U128(1)));
    });
    agent();
    rejects("ERR_NOT_IN_LEARNED_POLICY", || {
        drop(c.call(
            account("usual"),
            "ft_transfer".into(),
            Base64VecU8(vec![]),
            U128(13),
            U64(10_000_000_000_000),
        ));
    });
    owner();
    c.allow_recipient(account("other"));
    agent();
    rejects("ERR_NOT_IN_LEARNED_POLICY", || {
        drop(c.call(
            account("other"),
            "ft_transfer".into(),
            Base64VecU8(vec![]),
            U128(1),
            U64(10_000_000_000_000),
        ));
    });
}

#[test]
fn pending_cosign_schedules_exact_transfer_once_and_preserves_reservation_total() {
    let mut c = learned();
    let id = pending(&mut c, 200);
    assert!(get_created_receipts().is_empty());
    assert_eq!(
        (c.get_stats().reserved.0, c.get_spent_today().0),
        (200, 500)
    );
    owner();
    c.deny_recipient(account("usual"));
    watcher();
    rejects("NotAllowlisted", || {
        drop(c.cosign(id));
    });
    owner();
    c.allow_recipient(account("usual"));
    watcher();
    drop(c.cosign(id));
    let receipts = get_created_receipts();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].receiver_id, account("usual"));
    assert!(
        matches!(receipts[0].actions.as_slice(), [MockAction::Transfer {deposit, ..}] if deposit.as_yoctonear() == 200)
    );
    assert_eq!(c.get_pending(Some(id))[0].status, PendingStatus::Cosigned);
    assert_eq!((c.get_stats().reserved.0, c.get_spent_today().0), (0, 500));
    watcher();
    rejects("NotPending", || {
        drop(c.cosign(id));
    });
    assert_eq!(c.get_stats().tasks[0].spent.0, 200);
    assert_eq!(c.get_agent_key(key(7)).unwrap().2, 4);
}

#[test]
fn veto_releases_budget_without_refunding_agent_quota_and_cannot_replay() {
    let mut c = learned();
    let id = pending(&mut c, 4000);
    watcher();
    c.veto(id);
    assert_eq!(c.get_pending(Some(id))[0].status, PendingStatus::Vetoed);
    assert_eq!((c.get_spent_today().0, c.get_stats().reserved.0), (300, 0));
    assert_eq!(c.get_agent_key(key(7)).unwrap().2, 4);
    watcher();
    rejects("NotPending", || c.veto(id));
    watcher();
    rejects("NotPending", || {
        drop(c.cosign(id));
    });
}

#[test]
fn expiry_is_permissionless_at_exact_deadline_and_late_cosign_fails() {
    let mut c = learned();
    let id = pending(&mut c, 200);
    let deadline = c.get_pending(Some(id))[0].expires_at.0;
    context("stranger", 9, deadline - 1);
    rejects("Timeout", || c.expire(id));
    context("watcher", 8, deadline);
    rejects("Timeout", || {
        drop(c.cosign(id));
    });
    context("stranger", 9, deadline);
    c.expire(id);
    assert_eq!(c.get_pending(Some(id))[0].status, PendingStatus::Expired);
    assert_eq!(c.get_spent_today().0, 0);
    context("stranger", 9, deadline);
    rejects("NotPending", || c.expire(id));
}

#[test]
fn reservations_block_double_spend_across_agent_keys_before_cosign() {
    let mut c = learned();
    owner();
    c.set_daily_limit(U128(600));
    c.register_key(key(9), U64(TS + NS_PER_DAY), 10);
    let id = pending(&mut c, 200);
    context("wallet", 9, TS);
    rejects("DailyLimit", || {
        pay(&mut c, "usual", 101);
    });
    assert_eq!(c.get_spent_today().0, 500);
    watcher();
    c.veto(id);
    context("wallet", 9, TS);
    pay(&mut c, "usual", 101);
    assert_eq!(c.get_stats().reserved.0, 101);
}

#[test]
fn rollover_keeps_pending_capacity_and_cosign_charges_execution_day() {
    let mut c = learned();
    owner();
    c.set_daily_limit(U128(600));
    let id = pending(&mut c, 200);
    let next_day = (utc_day(TS) + 1) * NS_PER_DAY;
    context("wallet", 7, next_day);
    assert_eq!(c.get_spent_today().0, 200);
    rejects("DailyLimit", || {
        pay(&mut c, "usual", 401);
    });
    context("watcher", 8, next_day);
    drop(c.cosign(id));
    assert_eq!((c.get_spent_today().0, c.get_stats().reserved.0), (200, 0));
    assert_eq!(c.get_day_bucket(), U64(utc_day(next_day)));
}

#[test]
fn watcher_identity_is_scoped_and_cannot_request_spend_or_change_owner_policy() {
    let mut c = learned();
    let id = pending(&mut c, 200);
    for (who, n) in [("stranger", 8), ("watcher", 9), ("wallet", 7)] {
        context(who, n, TS);
        rejects("Unauthorized", || {
            drop(c.cosign(id));
        });
        context(who, n, TS);
        rejects("Unauthorized", || c.veto(id));
    }
    watcher();
    rejects("Unauthorized", || {
        pay(&mut c, "usual", 1);
    });
    watcher();
    rejects("Unauthorized", || {
        drop(c.transfer(account("usual"), U128(1)));
    });
    watcher();
    rejects("Unauthorized", || c.set_daily_limit(U128(u128::MAX)));
    // Forwarded receipt retaining the watcher's key must also fail.
    let mut b = VMContextBuilder::new();
    b.current_account_id(account("wallet"))
        .signer_account_id(account("watcher"))
        .predecessor_account_id(account("relay"))
        .signer_account_pk(key(8))
        .block_timestamp(TS);
    testing_env!(b.build());
    rejects("Unauthorized", || {
        drop(c.cosign(id));
    });
}

#[test]
fn owner_can_approve_unknown_peer_without_watcher_task_permission() {
    let mut c = learned();
    agent();
    let id = pay(&mut c, "unknown", 200).id.unwrap();
    watcher();
    rejects("TaskCondition", || {
        drop(c.cosign(id));
    });
    owner();
    drop(c.cosign(id));
    assert_eq!(get_created_receipts()[0].receiver_id, account("unknown"));
    assert_eq!(c.get_stats().tasks[0].spent.0, 0);
}

#[test]
fn watcher_task_amount_budget_and_deadline_fail_closed() {
    let mut c = learned();
    let id = pending(&mut c, 251);
    watcher();
    rejects("TaskCondition", || {
        drop(c.cosign(id));
    });
    c.veto(id);
    let a = pending(&mut c, 250);
    watcher();
    drop(c.cosign(a));
    let b = pending(&mut c, 200);
    watcher();
    rejects("TaskCondition", || {
        drop(c.cosign(b));
    });
    assert_eq!(c.get_stats().tasks[0].spent.0, 250);
    // Expired task cannot authorize a still-live payment.
    c.learning.tasks[0].condition.expires_at_ns = U64(TS);
    watcher();
    rejects("TaskCondition", || {
        drop(c.cosign(b));
    });
    assert_eq!(c.get_stats().reserved.0, 200);
}

#[test]
fn pause_origin_revocation_renewal_and_lowered_ceiling_block_cosign() {
    let mut c = learned();
    let id = pending(&mut c, 200);
    owner();
    c.pause();
    watcher();
    rejects("Paused", || {
        drop(c.cosign(id));
    });
    owner();
    c.unpause();
    c.set_daily_limit(U128(400));
    watcher();
    rejects("DailyLimit", || {
        drop(c.cosign(id));
    });
    owner();
    c.set_daily_limit(U128(10_000));
    c.revoke_key(key(7));
    watcher();
    rejects("KeyInactive", || {
        drop(c.cosign(id));
    });
    owner();
    c.register_key(key(7), U64(TS + NS_PER_DAY), 10);
    watcher();
    rejects("KeyInactive", || {
        drop(c.cosign(id));
    });
    owner();
    c.revoke_key(key(8));
    watcher();
    rejects("KeyInactive", || c.veto(id));
    owner();
    c.veto(id);
    assert_eq!(c.get_stats().reserved.0, 0);
}

#[test]
fn pending_storage_is_bounded_and_terminal_records_are_reused_without_id_replay() {
    let mut c = learned();
    let mut ids = vec![];
    for _ in 0..16 {
        ids.push(pending(&mut c, 121));
    }
    agent();
    rejects("PendingFull", || {
        pay(&mut c, "usual", 121);
    });
    watcher();
    c.veto(ids[0]);
    let next = pending(&mut c, 121);
    assert_eq!(next.0, 17);
    assert_eq!(c.get_pending(None).len(), 16);
    watcher();
    rejects("NotPending", || {
        drop(c.cosign(ids[0]));
    });
    assert_eq!(c.get_stats().reserved.0, 16 * 121);
}

#[test]
fn watcher_expiry_quota_and_event_payloads_contain_only_codes_and_numeric_fields() {
    let mut c = learned();
    let a = pending(&mut c, 200);
    owner();
    c.register_key(key(8), U64(TS + 1), 1);
    watcher();
    c.veto(a);
    for log in get_logs() {
        let e: near_sdk::serde_json::Value =
            near_sdk::serde_json::from_str(log.strip_prefix("EVENT_JSON:").unwrap()).unwrap();
        for (k, v) in e["data"][0].as_object().unwrap() {
            assert!(["id", "to", "amount", "rule"].contains(&k.as_str()));
            if k == "rule" {
                assert_eq!(v, "Vetoed");
            }
        }
    }
    let b = pending(&mut c, 200);
    watcher();
    rejects("MaxCalls", || c.veto(b));
    context("watcher", 8, TS + 1);
    rejects("Expired", || c.veto(b));
    owner();
    c.veto(b);
}

#[test]
fn unsafe_learning_configuration_cannot_deploy() {
    let mut c = setup();
    let base = c.learning.config.take().unwrap();
    let cases: [fn(&mut LearningConfig); 7] = [
        |x| x.observe_single_cap = U128(50_000_000_000_000_000_000_001),
        |x| x.pending_timeout_ns = U64(0),
        |x| x.watcher = account("owner"),
        |x| x.min_count = 0,
        |x| x.tasks.push(x.tasks[0].clone()),
        |x| x.tasks[0].budget = U128(0),
        |x| x.watcher_key = key(1),
    ];
    for change in cases {
        owner();
        let mut config = base.clone();
        change(&mut config);
        rejects("InvalidPolicy", || {
            LearningState::new(Some(config), &account("owner"));
        });
    }
}
