//! Policy-enforced wallet for AI agents (NEAR testnet).
//! On-chain logs carry only amounts, counterparties, key ids, and rule codes.

use near_sdk::json_types::{Base64VecU8, U64, U128};
use near_sdk::store::LookupMap;
use near_sdk::{AccountId, Gas, NearToken, PanicOnDefault, Promise, PublicKey, env, near, require};

mod learning;
pub use learning::*;

const NS_PER_DAY: u64 = 86_400_000_000_000;
const EVENT_STANDARD: &str = "agent-wallet";
const EVENT_VERSION: &str = "1.0.0";

/// Machine-readable rule result codes (no free text).
#[near(serializers = [json])]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleCode {
    Ok,
    Unauthorized,
    Paused,
    Expired,
    MaxCalls,
    DailyLimit,
    NotAllowlisted,
    MethodNotAllowed,
    ZeroAmount,
    KeyInactive,
    InvalidPolicy,
    InvalidGas,
    ObserveLimit,
    StatsFull,
    PolicyProposed,
    NotInLearnedPolicy,
    InvalidProposal,
    Pending,
    PendingFull,
    NotPending,
    Timeout,
    TaskCondition,
    Cosigned,
    Vetoed,
}

#[near(serializers = [borsh])]
#[derive(Clone)]
pub struct AgentKey {
    pub expires_at_ns: u64,
    pub max_calls: u32,
    pub used_calls: u32,
    pub active: bool,
    pub generation: u64,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct Contract {
    owner: AccountId,
    paused: bool,
    daily_limit: u128,
    spent_today: u128,
    day_bucket: u64,
    allowed_recipients: LookupMap<AccountId, bool>,
    allowed_methods: LookupMap<String, bool>,
    blocked_recipients: LookupMap<AccountId, bool>,
    agents: LookupMap<PublicKey, AgentKey>,
    learning: LearningState,
}

#[near]
impl Contract {
    #[init]
    pub fn init(owner: AccountId, daily_limit: U128, learning: Option<LearningConfig>) -> Self {
        require!(
            env::current_account_id().as_str().ends_with(".testnet")
                && owner.as_str().ends_with(".testnet"),
            "TestnetOnly"
        );
        require!(
            (env::predecessor_account_id() == owner && env::signer_account_id() == owner)
                || (env::predecessor_account_id() == env::current_account_id()
                    && env::signer_account_id() == env::current_account_id()),
            "Unauthorized"
        );
        let learning_state = LearningState::new(learning, &owner);
        let mut contract = Self {
            owner,
            paused: false,
            daily_limit: daily_limit.0,
            spent_today: 0,
            day_bucket: utc_day(env::block_timestamp()),
            allowed_recipients: LookupMap::new(b"r"),
            allowed_methods: LookupMap::new(b"m"),
            blocked_recipients: LookupMap::new(b"b"),
            agents: LookupMap::new(b"a"),
            learning: learning_state,
        };
        if let Some(config) = &contract.learning.config {
            contract.agents.insert(
                config.watcher_key.clone(),
                AgentKey {
                    expires_at_ns: config.watcher_expires_at_ns.0,
                    max_calls: config.watcher_max_calls,
                    used_calls: 0,
                    active: true,
                    generation: 0,
                },
            );
        }
        contract
    }

    // --- owner config ---

    pub fn set_daily_limit(&mut self, daily_limit: U128) {
        self.require_owner();
        self.daily_limit = daily_limit.0;
        emit(
            "policy",
            near_sdk::serde_json::json!({
                "rule": RuleCode::Ok,
                "amount": daily_limit.0.to_string(),
            }),
        );
    }

    pub fn allow_recipient(&mut self, account_id: AccountId) {
        self.require_owner();
        self.allowed_recipients.insert(account_id.clone(), true);
        self.blocked_recipients.remove(&account_id);
        emit(
            "recipient_allow",
            near_sdk::serde_json::json!({ "to": account_id, "rule": RuleCode::Ok }),
        );
    }

    pub fn deny_recipient(&mut self, account_id: AccountId) {
        self.require_owner();
        self.allowed_recipients.remove(&account_id);
        self.blocked_recipients.insert(account_id.clone(), true);
        emit(
            "recipient_deny",
            near_sdk::serde_json::json!({ "to": account_id, "rule": RuleCode::Ok }),
        );
    }

    pub fn allow_method(&mut self, method: String) {
        self.require_owner();
        require!(
            !method.is_empty() && method.len() <= 256,
            rule_str(&RuleCode::InvalidPolicy)
        );
        self.allowed_methods.insert(method, true);
        emit(
            "method_allow",
            near_sdk::serde_json::json!({ "rule": RuleCode::Ok }),
        );
    }

    pub fn deny_method(&mut self, method: String) {
        self.require_owner();
        self.allowed_methods.remove(&method);
        emit(
            "method_deny",
            near_sdk::serde_json::json!({ "rule": RuleCode::Ok }),
        );
    }

    pub fn register_key(&mut self, public_key: PublicKey, expires_at_ns: U64, max_calls: u32) {
        self.require_owner();
        require!(
            max_calls > 0 && expires_at_ns.0 > env::block_timestamp(),
            rule_str(&RuleCode::InvalidPolicy)
        );
        // A controller must not lose owner access by entering the agent registry.
        require!(
            public_key != env::signer_account_pk(),
            rule_str(&RuleCode::Unauthorized)
        );
        let generation = self.agents.get(&public_key).map_or(0, |k| k.generation + 1);
        self.agents.insert(
            public_key.clone(),
            AgentKey {
                expires_at_ns: expires_at_ns.0,
                max_calls,
                used_calls: 0,
                active: true,
                generation,
            },
        );
        emit(
            "key_register",
            near_sdk::serde_json::json!({
                "key": public_key.to_string(),
                "rule": RuleCode::Ok,
            }),
        );
    }

    pub fn revoke_key(&mut self, public_key: PublicKey) {
        self.require_owner();
        require!(
            public_key != env::signer_account_pk(),
            rule_str(&RuleCode::Unauthorized)
        );
        if let Some(mut meta) = self.agents.get(&public_key).cloned() {
            meta.active = false;
            self.agents.insert(public_key.clone(), meta);
        } else {
            self.agents.insert(
                public_key.clone(),
                AgentKey {
                    expires_at_ns: 0,
                    max_calls: 0,
                    used_calls: 0,
                    active: false,
                    generation: 0,
                },
            );
        }
        emit(
            "key_revoke",
            near_sdk::serde_json::json!({
                "key": public_key.to_string(),
                "rule": RuleCode::Ok,
            }),
        );
    }

    pub fn pause(&mut self) {
        self.require_owner();
        self.paused = true;
        emit(
            "pause",
            near_sdk::serde_json::json!({ "rule": RuleCode::Ok }),
        );
    }

    pub fn unpause(&mut self) {
        self.require_owner();
        self.paused = false;
        emit(
            "unpause",
            near_sdk::serde_json::json!({ "rule": RuleCode::Ok }),
        );
    }

    // --- agent actions ---

    pub fn transfer(&mut self, receiver: AccountId, amount: U128) -> Promise {
        let amount_u128 = amount.0;
        let pk = env::signer_account_pk();
        if let Err(code) = self.authorize_agent(&pk) {
            emit_transfer(&pk, &receiver, amount_u128, code.clone());
            env::panic_str(rule_str(&code));
        }
        if amount_u128 == 0 {
            emit_transfer(&pk, &receiver, amount_u128, RuleCode::ZeroAmount);
            env::panic_str(rule_str(&RuleCode::ZeroAmount));
        }
        if self.learning.mode != Mode::Observe && !self.recipient_allowed(&receiver) {
            let code = if self.learning.proposal.is_some() {
                RuleCode::NotInLearnedPolicy
            } else {
                RuleCode::NotAllowlisted
            };
            emit_transfer(&pk, &receiver, amount_u128, code.clone());
            env::panic_str(rule_str(&code));
        }
        if let Err(code) = self.check_direct(&receiver, &ObservedMethod::Pay, amount_u128) {
            emit_transfer(&pk, &receiver, amount_u128, code.clone());
            env::panic_str(rule_str(&code));
        }
        if let Err(code) = self.charge(&pk, amount_u128) {
            emit_transfer(&pk, &receiver, amount_u128, code.clone());
            env::panic_str(rule_str(&code));
        }
        self.record_observation(receiver.clone(), ObservedMethod::Pay, amount_u128);
        emit_transfer(&pk, &receiver, amount_u128, RuleCode::Ok);
        Promise::new(receiver).transfer(NearToken::from_yoctonear(amount_u128))
    }

    pub fn call(
        &mut self,
        receiver: AccountId,
        method: String,
        args: Base64VecU8,
        deposit: U128,
        gas: U64,
    ) -> Promise {
        let pk = env::signer_account_pk();
        let deposit_u128 = deposit.0;
        if let Err(code) = self.authorize_agent(&pk) {
            emit_call(&pk, &receiver, deposit_u128, code.clone());
            env::panic_str(rule_str(&code));
        }
        if self.learning.mode != Mode::Observe && !self.recipient_allowed(&receiver) {
            emit_call(&pk, &receiver, deposit_u128, RuleCode::NotAllowlisted);
            env::panic_str(rule_str(&RuleCode::NotAllowlisted));
        }
        if !self.method_allowed(&method) {
            emit_call(&pk, &receiver, deposit_u128, RuleCode::MethodNotAllowed);
            env::panic_str(rule_str(&RuleCode::MethodNotAllowed));
        }
        // A delegated call must not turn into a recursive wallet/admin call.
        if receiver == env::current_account_id() {
            emit_call(&pk, &receiver, deposit_u128, RuleCode::Unauthorized);
            env::panic_str(rule_str(&RuleCode::Unauthorized));
        }
        let available_gas = env::prepaid_gas()
            .as_gas()
            .saturating_sub(env::used_gas().as_gas())
            .saturating_sub(Gas::from_tgas(5).as_gas());
        if gas.0 == 0 || gas.0 > available_gas {
            emit_call(&pk, &receiver, deposit_u128, RuleCode::InvalidGas);
            env::panic_str(rule_str(&RuleCode::InvalidGas));
        }
        let observed_method = ObservedMethod::Call(method.clone());
        if let Err(code) = self.check_direct(&receiver, &observed_method, deposit_u128) {
            emit_call(&pk, &receiver, deposit_u128, code.clone());
            env::panic_str(rule_str(&code));
        }
        if let Err(code) = self.charge(&pk, deposit_u128) {
            emit_call(&pk, &receiver, deposit_u128, code.clone());
            env::panic_str(rule_str(&code));
        }
        self.record_observation(receiver.clone(), observed_method, deposit_u128);
        emit_call(&pk, &receiver, deposit_u128, RuleCode::Ok);
        Promise::new(receiver).function_call(
            method,
            args.0,
            NearToken::from_yoctonear(deposit_u128),
            Gas::from_gas(gas.0),
        )
    }

    // --- views ---

    pub fn get_owner(&self) -> AccountId {
        self.owner.clone()
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn get_daily_limit(&self) -> U128 {
        U128(self.daily_limit)
    }

    pub fn get_spent_today(&self) -> U128 {
        U128(self.current_spend())
    }

    pub fn get_day_bucket(&self) -> U64 {
        U64(self.day_bucket)
    }

    pub fn is_recipient_allowed(&self, account_id: AccountId) -> bool {
        self.recipient_allowed(&account_id)
    }

    pub fn is_method_allowed(&self, method: String) -> bool {
        self.method_allowed(&method)
    }

    pub fn get_agent_key(&self, public_key: PublicKey) -> Option<(U64, u32, u32, bool)> {
        self.agents
            .get(&public_key)
            .map(|k| (U64(k.expires_at_ns), k.max_calls, k.used_calls, k.active))
    }
}

impl Contract {
    fn require_owner(&self) {
        require!(
            env::predecessor_account_id() == self.owner
                && env::signer_account_id() == self.owner
                && self.agents.get(&env::signer_account_pk()).is_none(),
            rule_str(&RuleCode::Unauthorized)
        );
    }

    fn recipient_allowed(&self, account_id: &AccountId) -> bool {
        self.allowed_recipients
            .get(account_id)
            .copied()
            .unwrap_or(false)
    }

    fn method_allowed(&self, method: &str) -> bool {
        self.allowed_methods.get(method).copied().unwrap_or(false)
    }

    fn authorize_agent(&self, pk: &PublicKey) -> Result<(), RuleCode> {
        // Signer keys are scoped to accounts. Reject forwarded calls even if
        // they retain a registered signer key from an earlier receipt.
        if env::signer_account_id() != env::current_account_id()
            || env::predecessor_account_id() != env::current_account_id()
        {
            return Err(RuleCode::Unauthorized);
        }
        if self.paused {
            return Err(RuleCode::Paused);
        }
        let Some(meta) = self.agents.get(pk) else {
            return Err(RuleCode::Unauthorized);
        };
        if !meta.active {
            return Err(RuleCode::KeyInactive);
        }
        if env::block_timestamp() >= meta.expires_at_ns {
            return Err(RuleCode::Expired);
        }
        if meta.used_calls >= meta.max_calls {
            return Err(RuleCode::MaxCalls);
        }
        Ok(())
    }

    fn charge(&mut self, pk: &PublicKey, amount: u128) -> Result<(), RuleCode> {
        self.current_spend()
            .checked_add(amount)
            .filter(|total| *total <= self.daily_limit)
            .ok_or(RuleCode::DailyLimit)?;
        let mut meta = self.agents.get(pk).cloned().expect("Unauthorized");
        meta.used_calls = meta.used_calls.checked_add(1).expect("MaxCalls");
        self.agents.insert(pk.clone(), meta);
        self.spent_today = self.actual_spend().checked_add(amount).expect("DailyLimit");
        self.day_bucket = utc_day(env::block_timestamp());
        Ok(())
    }

    fn current_spend(&self) -> u128 {
        self.actual_spend().saturating_add(self.reserved_total())
    }

    fn actual_spend(&self) -> u128 {
        if utc_day(env::block_timestamp()) == self.day_bucket {
            self.spent_today
        } else {
            0
        }
    }
}

fn utc_day(ts_ns: u64) -> u64 {
    ts_ns / NS_PER_DAY
}

fn rule_str(code: &RuleCode) -> &'static str {
    match code {
        RuleCode::Ok => "Ok",
        RuleCode::Unauthorized => "Unauthorized",
        RuleCode::Paused => "Paused",
        RuleCode::Expired => "Expired",
        RuleCode::MaxCalls => "MaxCalls",
        RuleCode::DailyLimit => "DailyLimit",
        RuleCode::NotAllowlisted => "NotAllowlisted",
        RuleCode::MethodNotAllowed => "MethodNotAllowed",
        RuleCode::ZeroAmount => "ZeroAmount",
        RuleCode::KeyInactive => "KeyInactive",
        RuleCode::InvalidPolicy => "InvalidPolicy",
        RuleCode::InvalidGas => "InvalidGas",
        RuleCode::ObserveLimit => "ObserveLimit",
        RuleCode::StatsFull => "StatsFull",
        RuleCode::PolicyProposed => "PolicyProposed",
        RuleCode::NotInLearnedPolicy => "ERR_NOT_IN_LEARNED_POLICY",
        RuleCode::InvalidProposal => "InvalidProposal",
        RuleCode::Pending => "Pending",
        RuleCode::PendingFull => "PendingFull",
        RuleCode::NotPending => "NotPending",
        RuleCode::Timeout => "Timeout",
        RuleCode::TaskCondition => "TaskCondition",
        RuleCode::Cosigned => "Cosigned",
        RuleCode::Vetoed => "Vetoed",
    }
}

fn emit(event: &str, data: near_sdk::serde_json::Value) {
    let payload = near_sdk::serde_json::json!({
        "standard": EVENT_STANDARD,
        "version": EVENT_VERSION,
        "event": event,
        "data": [data],
    });
    env::log_str(&format!(
        "EVENT_JSON:{}",
        near_sdk::serde_json::to_string(&payload).unwrap()
    ));
}

fn emit_transfer(pk: &PublicKey, receiver: &AccountId, amount: u128, rule: RuleCode) {
    emit(
        "transfer",
        near_sdk::serde_json::json!({
            "key": pk.to_string(),
            "to": receiver,
            "amount": amount.to_string(),
            "rule": rule,
        }),
    );
}

fn emit_call(pk: &PublicKey, receiver: &AccountId, deposit: u128, rule: RuleCode) {
    emit(
        "call",
        near_sdk::serde_json::json!({
            "key": pk.to_string(),
            "to": receiver,
            "amount": deposit.to_string(),
            "rule": rule,
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::mock::MockAction;
    use near_sdk::test_utils::{VMContextBuilder, get_created_receipts, get_logs};
    use near_sdk::{PublicKey, testing_env};

    const ACTION_TS: u64 = 1_100_000_000_000_000;

    fn accounts(index: usize) -> AccountId {
        [
            "wallet.testnet",
            "owner.testnet",
            "payee.testnet",
            "blocked.testnet",
            "relay.testnet",
        ][index]
            .parse()
            .unwrap()
    }

    fn pk(n: u8) -> PublicKey {
        // 33-byte ed25519 key material: 0x00 prefix + 32 bytes
        let mut bytes = vec![0u8; 33];
        bytes[0] = 0;
        bytes[1] = n;
        bytes[32] = n;
        PublicKey::try_from(bytes).unwrap()
    }

    fn context_owner(owner: AccountId) -> VMContextBuilder {
        let mut b = VMContextBuilder::new();
        b.current_account_id(accounts(0))
            .predecessor_account_id(owner.clone())
            .signer_account_id(owner)
            .signer_account_pk(pk(1))
            .block_timestamp(1_000_000_000_000_000); // day bucket stable
        b
    }

    fn context_agent(_owner: AccountId, agent_pk: PublicKey, ts: u64) -> VMContextBuilder {
        let mut b = VMContextBuilder::new();
        b.current_account_id(accounts(0))
            .predecessor_account_id(accounts(0))
            .signer_account_id(accounts(0))
            .signer_account_pk(agent_pk)
            .account_balance(NearToken::from_near(100))
            .block_timestamp(ts);
        b
    }

    fn setup() -> (Contract, AccountId, PublicKey) {
        let owner = accounts(1);
        testing_env!(context_owner(owner.clone()).build());
        let mut c = Contract::init(
            owner.clone(),
            U128(NearToken::from_near(10).as_yoctonear()),
            None,
        );
        let agent = pk(7);
        let expires = U64(2_000_000_000_000_000);
        c.register_key(agent.clone(), expires, 5);
        c.allow_recipient(accounts(2));
        c.allow_method("ft_transfer".to_string());
        (c, owner, agent)
    }

    #[test]
    fn transfer_ok() {
        let (mut c, owner, agent) = setup();
        testing_env!(context_agent(owner, agent.clone(), 1_100_000_000_000_000).build());
        let amount = U128(NearToken::from_near(1).as_yoctonear());
        let _ = c.transfer(accounts(2), amount);
        assert_eq!(c.get_spent_today().0, amount.0);
        let meta = c.get_agent_key(agent).unwrap();
        assert_eq!(meta.2, 1); // used_calls
    }

    #[test]
    #[should_panic(expected = "Expired")]
    fn expiry_by_time() {
        let (mut c, owner, agent) = setup();
        // expires_at = 2e15; use ts beyond that
        testing_env!(context_agent(owner, agent, 2_000_000_000_000_001).build());
        let _ = c.transfer(accounts(2), U128(1));
    }

    #[test]
    #[should_panic(expected = "MaxCalls")]
    fn expiry_by_count() {
        let owner = accounts(1);
        testing_env!(context_owner(owner.clone()).build());
        let mut c = Contract::init(
            owner.clone(),
            U128(NearToken::from_near(100).as_yoctonear()),
            None,
        );
        let agent = pk(3);
        c.register_key(agent.clone(), U64(9_000_000_000_000_000), 2);
        c.allow_recipient(accounts(2));
        testing_env!(context_agent(owner.clone(), agent.clone(), 1_100_000_000_000_000).build());
        let _ = c.transfer(accounts(2), U128(1));
        testing_env!(context_agent(owner.clone(), agent.clone(), 1_100_000_000_000_001).build());
        let _ = c.transfer(accounts(2), U128(1));
        testing_env!(context_agent(owner, agent, 1_100_000_000_000_002).build());
        let _ = c.transfer(accounts(2), U128(1));
    }

    #[test]
    #[should_panic(expected = "DailyLimit")]
    fn daily_limit() {
        let (mut c, owner, agent) = setup();
        testing_env!(context_agent(owner, agent, 1_100_000_000_000_000).build());
        // daily_limit is 10 N; try 11 N
        let _ = c.transfer(accounts(2), U128(NearToken::from_near(11).as_yoctonear()));
    }

    #[test]
    #[should_panic(expected = "NotAllowlisted")]
    fn allowlist_reject() {
        let (mut c, owner, agent) = setup();
        testing_env!(context_agent(owner, agent, 1_100_000_000_000_000).build());
        let _ = c.transfer(accounts(3), U128(1));
    }

    #[test]
    #[should_panic(expected = "MethodNotAllowed")]
    fn method_reject() {
        let (mut c, owner, agent) = setup();
        testing_env!(context_agent(owner, agent, 1_100_000_000_000_000).build());
        let _ = c.call(
            accounts(2),
            "evil".to_string(),
            Base64VecU8(vec![]),
            U128(0),
            U64(30_000_000_000_000),
        );
    }

    #[test]
    #[should_panic(expected = "Paused")]
    fn pause_blocks_transfer() {
        let (mut c, owner, agent) = setup();
        testing_env!(context_owner(owner.clone()).build());
        c.pause();
        testing_env!(context_agent(owner, agent, 1_100_000_000_000_000).build());
        let _ = c.transfer(accounts(2), U128(1));
    }

    #[test]
    fn call_ok_zero_deposit() {
        let (mut c, owner, agent) = setup();
        testing_env!(context_agent(owner, agent.clone(), 1_100_000_000_000_000).build());
        let _ = c.call(
            accounts(2),
            "ft_transfer".to_string(),
            Base64VecU8(b"{}".to_vec()),
            U128(0),
            U64(10_000_000_000_000),
        );
        assert_eq!(c.get_agent_key(agent).unwrap().2, 1);
    }

    fn assert_panics(code: &str, action: impl FnOnce()) {
        let error = std::panic::catch_unwind(std::panic::AssertUnwindSafe(action))
            .expect_err("expected policy rejection");
        let message = error
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| error.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(message.contains(code), "unexpected rejection: {message}");
    }

    fn assert_log_schema() {
        let logs = get_logs();
        assert!(!logs.is_empty());
        for log in logs {
            let event: near_sdk::serde_json::Value =
                near_sdk::serde_json::from_str(log.strip_prefix("EVENT_JSON:").unwrap()).unwrap();
            assert_eq!(event["standard"], EVENT_STANDARD);
            assert_eq!(event["version"], EVENT_VERSION);
            assert_eq!(event.as_object().unwrap().len(), 4);
            for data in event["data"].as_array().unwrap() {
                for key in data.as_object().unwrap().keys() {
                    assert!(
                        ["amount", "to", "key", "rule"].contains(&key.as_str()),
                        "unexpected log field: {key}"
                    );
                }
                assert!(data["rule"].is_string());
            }
        }
    }

    fn assert_agent_rejected(c: &mut Contract, code: RuleCode, action: impl FnOnce(&mut Contract)) {
        let key = env::signer_account_pk();
        let before = (c.spent_today, c.day_bucket, c.get_agent_key(key.clone()));
        assert_panics(rule_str(&code), || action(c));
        assert_eq!((c.spent_today, c.day_bucket, c.get_agent_key(key)), before);
        assert!(get_created_receipts().is_empty());
        assert_log_schema();
        let event: near_sdk::serde_json::Value = near_sdk::serde_json::from_str(
            get_logs()
                .last()
                .unwrap()
                .strip_prefix("EVENT_JSON:")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(event["data"][0]["rule"], rule_str(&code));
    }

    fn allowed_call(c: &mut Contract, deposit: u128) {
        drop(c.call(
            accounts(2),
            "ft_transfer".to_string(),
            Base64VecU8(b"{}".to_vec()),
            U128(deposit),
            U64(Gas::from_tgas(10).as_gas()),
        ));
    }

    #[test]
    fn transfer_creates_exact_receipt() {
        let (mut c, owner, agent) = setup();
        testing_env!(context_agent(owner, agent, ACTION_TS).build());
        drop(c.transfer(accounts(2), U128(123)));
        let receipts = get_created_receipts();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].receiver_id, accounts(2));
        assert!(
            matches!(receipts[0].actions.as_slice(), [MockAction::Transfer { deposit, .. }]
            if *deposit == NearToken::from_yoctonear(123))
        );
        assert_log_schema();
    }

    #[test]
    fn call_creates_exact_receipt_and_charges_deposit() {
        let (mut c, owner, agent) = setup();
        testing_env!(context_agent(owner, agent.clone(), ACTION_TS).build());
        allowed_call(&mut c, 123);
        let receipts = get_created_receipts();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].receiver_id, accounts(2));
        assert!(matches!(receipts[0].actions.as_slice(),
            [MockAction::FunctionCallWeight { method_name, args, attached_deposit, prepaid_gas, .. }]
            if method_name == b"ft_transfer" && args == b"{}"
                && *attached_deposit == NearToken::from_yoctonear(123)
                && *prepaid_gas == Gas::from_tgas(10)));
        assert_eq!(c.get_spent_today(), U128(123));
        assert_eq!(c.get_agent_key(agent).unwrap().2, 1);
        assert_log_schema();
    }

    #[test]
    fn expiry_at_exact_timestamp_rejects_without_consuming_budget() {
        let (mut c, owner, agent) = setup();
        testing_env!(context_agent(owner, agent, 2_000_000_000_000_000).build());
        assert_agent_rejected(&mut c, RuleCode::Expired, |c| {
            drop(c.transfer(accounts(2), U128(1)));
        });
    }

    #[test]
    fn exact_daily_limit_succeeds_then_cumulative_spend_rejects() {
        let (mut c, owner, agent) = setup();
        let limit = c.get_daily_limit().0;
        testing_env!(context_agent(owner.clone(), agent.clone(), ACTION_TS).build());
        drop(c.transfer(accounts(2), U128(limit - 1)));
        testing_env!(context_agent(owner.clone(), agent.clone(), ACTION_TS).build());
        allowed_call(&mut c, 1);
        assert_eq!(c.get_spent_today(), U128(limit));
        assert_eq!(c.get_agent_key(agent.clone()).unwrap().2, 2);
        testing_env!(context_agent(owner, agent, ACTION_TS).build());
        assert_agent_rejected(&mut c, RuleCode::DailyLimit, |c| {
            drop(c.transfer(accounts(2), U128(1)));
        });
    }

    #[test]
    fn daily_budget_is_shared_between_keys() {
        let (mut c, owner, agent) = setup();
        let other = pk(8);
        c.register_key(other.clone(), U64(2_000_000_000_000_000), 5);
        let limit = c.get_daily_limit().0;
        testing_env!(context_agent(owner.clone(), agent, ACTION_TS).build());
        drop(c.transfer(accounts(2), U128(limit)));
        testing_env!(context_agent(owner, other, ACTION_TS).build());
        assert_agent_rejected(&mut c, RuleCode::DailyLimit, |c| {
            allowed_call(c, 1);
        });
    }

    #[test]
    fn next_utc_day_resets_budget_but_keeps_call_count() {
        let (mut c, owner, agent) = setup();
        let next_day = (utc_day(ACTION_TS) + 1) * NS_PER_DAY;
        let limit = c.get_daily_limit().0;
        testing_env!(context_agent(owner.clone(), agent.clone(), next_day - 1).build());
        drop(c.transfer(accounts(2), U128(limit)));
        assert_eq!(c.get_spent_today(), U128(limit));
        testing_env!(context_agent(owner.clone(), agent.clone(), next_day).build());
        assert_eq!(c.get_spent_today(), U128(0));
        allowed_call(&mut c, 0);
        assert_eq!(c.get_day_bucket(), U64(utc_day(next_day)));
        assert_eq!(c.get_spent_today(), U128(0));
        testing_env!(context_agent(owner, agent.clone(), next_day + 1).build());
        drop(c.transfer(accounts(2), U128(limit)));
        assert_eq!(c.get_agent_key(agent).unwrap().2, 3);
    }

    #[test]
    fn lowered_daily_limit_preserves_existing_spend() {
        let (mut c, owner, agent) = setup();
        testing_env!(context_agent(owner.clone(), agent.clone(), ACTION_TS).build());
        drop(c.transfer(accounts(2), U128(10)));
        let mut context = context_owner(owner.clone());
        context.block_timestamp(ACTION_TS);
        testing_env!(context.build());
        c.set_daily_limit(U128(9));
        assert_eq!(c.get_spent_today(), U128(10));
        testing_env!(context_agent(owner, agent, ACTION_TS).build());
        assert_agent_rejected(&mut c, RuleCode::DailyLimit, |c| {
            drop(c.transfer(accounts(2), U128(1)));
        });
    }

    #[test]
    fn daily_sum_overflow_rejects() {
        let (mut c, owner, agent) = setup();
        c.set_daily_limit(U128(u128::MAX));
        c.day_bucket = utc_day(ACTION_TS);
        c.spent_today = u128::MAX;
        testing_env!(context_agent(owner, agent, ACTION_TS).build());
        assert_agent_rejected(&mut c, RuleCode::DailyLimit, |c| {
            drop(c.transfer(accounts(2), U128(1)));
        });
    }

    #[test]
    fn unregistered_key_rejects() {
        let (mut c, owner, _) = setup();
        testing_env!(context_agent(owner, pk(9), ACTION_TS).build());
        assert_agent_rejected(&mut c, RuleCode::Unauthorized, |c| {
            drop(c.transfer(accounts(2), U128(1)));
        });
    }

    #[test]
    fn same_key_from_another_signer_account_rejects() {
        let (mut c, owner, agent) = setup();
        let mut context = context_agent(owner, agent, ACTION_TS);
        context
            .signer_account_id(accounts(3))
            .predecessor_account_id(accounts(3));
        testing_env!(context.build());
        assert_agent_rejected(&mut c, RuleCode::Unauthorized, |c| {
            drop(c.transfer(accounts(2), U128(1)));
        });
    }

    #[test]
    fn forwarded_call_with_registered_signer_key_rejects() {
        let (mut c, owner, agent) = setup();
        let mut context = context_agent(owner, agent, ACTION_TS);
        context.predecessor_account_id(accounts(4));
        testing_env!(context.build());
        assert_agent_rejected(&mut c, RuleCode::Unauthorized, |c| {
            allowed_call(c, 0);
        });
    }

    #[test]
    fn revoke_blocks_transfer_and_preserves_usage() {
        let (mut c, owner, agent) = setup();
        testing_env!(context_agent(owner.clone(), agent.clone(), ACTION_TS).build());
        drop(c.transfer(accounts(2), U128(1)));
        testing_env!(context_owner(owner.clone()).build());
        c.revoke_key(agent.clone());
        assert_eq!(c.get_agent_key(agent.clone()).unwrap().2, 1);
        assert!(!c.get_agent_key(agent.clone()).unwrap().3);
        testing_env!(context_agent(owner, agent, ACTION_TS).build());
        assert_agent_rejected(&mut c, RuleCode::KeyInactive, |c| {
            drop(c.transfer(accounts(2), U128(1)));
        });
    }

    #[test]
    fn owner_reregistration_resets_revoked_key() {
        let (mut c, owner, agent) = setup();
        c.revoke_key(agent.clone());
        c.register_key(agent.clone(), U64(ACTION_TS + NS_PER_DAY), 1);
        testing_env!(context_agent(owner, agent.clone(), ACTION_TS).build());
        allowed_call(&mut c, 0);
        assert_eq!(
            c.get_agent_key(agent).unwrap(),
            (U64(ACTION_TS + NS_PER_DAY), 1, 1, true)
        );
    }

    #[test]
    fn revoked_key_cannot_call() {
        let (mut c, owner, agent) = setup();
        c.revoke_key(agent.clone());
        testing_env!(context_agent(owner, agent, ACTION_TS).build());
        assert_agent_rejected(&mut c, RuleCode::KeyInactive, |c| {
            allowed_call(c, 0);
        });
    }

    #[test]
    fn zero_deposit_call_still_consumes_max_calls() {
        let (mut c, owner, agent) = setup();
        c.register_key(agent.clone(), U64(2_000_000_000_000_000), 1);
        testing_env!(context_agent(owner.clone(), agent.clone(), ACTION_TS).build());
        allowed_call(&mut c, 0);
        testing_env!(context_agent(owner, agent, ACTION_TS).build());
        assert_agent_rejected(&mut c, RuleCode::MaxCalls, |c| {
            allowed_call(c, 0);
        });
    }

    #[test]
    fn expired_key_cannot_call() {
        let (mut c, owner, agent) = setup();
        testing_env!(context_agent(owner, agent, 2_000_000_000_000_000).build());
        assert_agent_rejected(&mut c, RuleCode::Expired, |c| {
            allowed_call(c, 0);
        });
    }

    #[test]
    fn call_recipient_must_be_allowlisted() {
        let (mut c, owner, agent) = setup();
        c.deny_recipient(accounts(2));
        testing_env!(context_agent(owner, agent, ACTION_TS).build());
        assert_agent_rejected(&mut c, RuleCode::NotAllowlisted, |c| {
            allowed_call(c, 0);
        });
    }

    #[test]
    fn denied_method_cannot_call() {
        let (mut c, owner, agent) = setup();
        c.deny_method("ft_transfer".to_string());
        testing_env!(context_agent(owner, agent, ACTION_TS).build());
        assert_agent_rejected(&mut c, RuleCode::MethodNotAllowed, |c| {
            allowed_call(c, 0);
        });
    }

    #[test]
    fn zero_transfer_rejects() {
        let (mut c, owner, agent) = setup();
        testing_env!(context_agent(owner, agent, ACTION_TS).build());
        assert_agent_rejected(&mut c, RuleCode::ZeroAmount, |c| {
            drop(c.transfer(accounts(2), U128(0)));
        });
    }

    #[test]
    fn pause_blocks_calls_and_owner_unpause_restores_transfer() {
        let (mut c, owner, agent) = setup();
        c.pause();
        testing_env!(context_agent(owner.clone(), agent.clone(), ACTION_TS).build());
        assert_agent_rejected(&mut c, RuleCode::Paused, |c| {
            allowed_call(c, 0);
        });
        testing_env!(context_owner(owner.clone()).build());
        c.unpause();
        testing_env!(context_agent(owner, agent, ACTION_TS).build());
        drop(c.transfer(accounts(2), U128(1)));
        assert!(!c.is_paused());
    }

    #[test]
    fn agent_cannot_change_owner_controls() {
        let (mut c, owner, agent) = setup();
        let actions: [fn(&mut Contract); 9] = [
            |c| c.set_daily_limit(U128(u128::MAX)),
            |c| c.allow_recipient(accounts(3)),
            |c| c.deny_recipient(accounts(2)),
            |c| c.allow_method("untrusted_method".to_string()),
            |c| c.deny_method("ft_transfer".to_string()),
            |c| c.register_key(pk(9), U64(2_000_000_000_000_000), 99),
            |c| c.revoke_key(pk(7)),
            |c| c.pause(),
            |c| c.unpause(),
        ];
        for action in actions {
            testing_env!(context_agent(owner.clone(), agent.clone(), ACTION_TS).build());
            assert_panics("Unauthorized", || action(&mut c));
        }
        assert_eq!(
            c.get_daily_limit(),
            U128(NearToken::from_near(10).as_yoctonear())
        );
        assert!(c.is_recipient_allowed(accounts(2)));
        assert!(!c.is_recipient_allowed(accounts(3)));
        assert!(c.is_method_allowed("ft_transfer".to_string()));
        assert!(!c.is_paused());
        assert_eq!(c.get_agent_key(agent).unwrap().2, 0);
        assert!(c.get_agent_key(pk(9)).is_none());
    }

    #[test]
    fn self_owned_wallet_agent_cannot_use_owner_controls_even_after_revoke() {
        let owner = accounts(0);
        testing_env!(context_owner(owner.clone()).build());
        let mut c = Contract::init(owner.clone(), U128(100), None);
        let agent = pk(7);
        c.register_key(agent.clone(), U64(2_000_000_000_000_000), 5);
        testing_env!(context_agent(owner.clone(), agent.clone(), ACTION_TS).build());
        assert_panics("Unauthorized", || c.set_daily_limit(U128(u128::MAX)));
        testing_env!(context_owner(owner.clone()).build());
        c.revoke_key(agent.clone());
        testing_env!(context_agent(owner, agent, ACTION_TS).build());
        assert_panics("Unauthorized", || c.unpause());
        assert_eq!(c.get_daily_limit(), U128(100));
    }

    #[test]
    fn self_calls_cannot_invoke_allowed_owner_methods() {
        let (mut c, owner, agent) = setup();
        c.allow_recipient(accounts(0));
        c.allow_method("unpause".to_string());
        testing_env!(context_agent(owner, agent, ACTION_TS).build());
        assert_agent_rejected(&mut c, RuleCode::Unauthorized, |c| {
            drop(c.call(
                accounts(0),
                "unpause".to_string(),
                Base64VecU8(b"{}".to_vec()),
                U128(0),
                U64(Gas::from_tgas(10).as_gas()),
            ));
        });
    }

    #[test]
    fn invalid_gas_rejects_without_consuming_budget() {
        let (mut c, owner, agent) = setup();
        for gas in [0, u64::MAX] {
            testing_env!(context_agent(owner.clone(), agent.clone(), ACTION_TS).build());
            assert_agent_rejected(&mut c, RuleCode::InvalidGas, |c| {
                drop(c.call(
                    accounts(2),
                    "ft_transfer".to_string(),
                    Base64VecU8(vec![]),
                    U128(1),
                    U64(gas),
                ));
            });
        }
    }

    #[test]
    fn registration_rejects_zero_calls_or_expired_timestamp() {
        let (mut c, owner, _) = setup();
        for (expires, max_calls) in [(2_000_000_000_000_000, 0), (1_000_000_000_000_000, 1)] {
            testing_env!(context_owner(owner.clone()).build());
            assert_panics("InvalidPolicy", || {
                c.register_key(pk(9), U64(expires), max_calls)
            });
            assert!(c.get_agent_key(pk(9)).is_none());
        }
    }

    #[test]
    fn controlling_key_cannot_be_registered_or_revoked() {
        for owner in [accounts(0), accounts(1)] {
            testing_env!(context_owner(owner.clone()).build());
            let mut c = Contract::init(owner, U128(100), None);
            assert_panics("Unauthorized", || {
                c.register_key(pk(1), U64(2_000_000_000_000_000), 1)
            });
            assert_panics("Unauthorized", || c.revoke_key(pk(1)));
            assert!(c.get_agent_key(pk(1)).is_none());
            c.set_daily_limit(U128(101));
            assert_eq!(c.get_daily_limit(), U128(101));
        }
    }

    #[test]
    fn event_payloads_exclude_method_names_and_arguments() {
        let (mut c, owner, agent) = setup();
        assert_log_schema();
        c.set_daily_limit(U128(100));
        c.deny_recipient(accounts(3));
        c.deny_method("untrusted_method".to_string());
        c.revoke_key(pk(9));
        c.pause();
        c.unpause();
        assert_log_schema();
        testing_env!(context_agent(owner, agent, ACTION_TS).build());
        drop(c.call(
            accounts(2),
            "ft_transfer".to_string(),
            Base64VecU8(b"{\"payload\":\"private_arg\"}".to_vec()),
            U128(0),
            U64(Gas::from_tgas(10).as_gas()),
        ));
        assert_log_schema();
        for log in get_logs() {
            assert!(!log.contains("ft_transfer"));
            assert!(!log.contains("private_arg"));
        }
    }
}

#[cfg(test)]
mod learning_tests;
