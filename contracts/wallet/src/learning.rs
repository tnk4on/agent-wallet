//! Bounded numeric observation and an isolated, task-constrained co-signer.
use super::*;

pub const MAX_ROWS: usize = 16;
pub const MAX_PENDING: usize = 16;
pub const MAX_TASKS: usize = 16;
const OBSERVE_DAILY_CAP: u128 = 500_000_000_000_000_000_000_000; // 0.5 testnet NEAR
const OBSERVE_SINGLE_CAP: u128 = 50_000_000_000_000_000_000_000; // 0.05 testnet NEAR

#[near(serializers = [borsh, json])]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Observe,
    Proposed,
    Enforce,
}

/// Separate the native payment operation from an external method of the same name.
#[near(serializers = [borsh, json])]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObservedMethod {
    Pay,
    Call(String),
}

/// Requests deliberately carry no arbitrary call arguments or text.
#[near(serializers = [borsh, json])]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaymentMethod {
    Pay,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug)]
pub struct Observation {
    pub peer: AccountId,
    pub method: ObservedMethod,
    pub count: u32,
    pub sum: U128,
    pub max_amount: U128,
    pub first_ts: U64,
    pub last_ts: U64,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug)]
pub struct LearnedLimit {
    pub peer: AccountId,
    pub method: ObservedMethod,
    pub cap: U128,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug)]
pub struct Proposal {
    pub rows: Vec<LearnedLimit>,
    pub daily_cap: U128,
    pub margin_bps: u32,
    pub min_count: u32,
    pub observations: u32,
    pub first_ts: U64,
    pub last_ts: U64,
    pub hash: Base64VecU8,
}

/// Owner registers immutable exception budgets in the atomic init transaction.
#[near(serializers = [borsh, json])]
#[derive(Clone, Debug)]
pub struct TaskCondition {
    pub task_id: U64,
    pub peer: AccountId,
    pub method: PaymentMethod,
    pub max_amount: U128,
    pub budget: U128,
    pub expires_at_ns: U64,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug)]
pub struct RegisteredTask {
    pub condition: TaskCondition,
    pub hash: Base64VecU8,
    pub spent: U128,
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct LearningConfig {
    pub observe_single_cap: U128,
    pub min_observations: u32,
    pub min_count: u32,
    pub watcher: AccountId,
    pub watcher_key: PublicKey,
    pub watcher_expires_at_ns: U64,
    pub watcher_max_calls: u32,
    pub pending_timeout_ns: U64,
    pub tasks: Vec<TaskCondition>,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingStatus {
    Pending,
    Cosigned,
    Vetoed,
    Expired,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug)]
pub struct PendingPayment {
    pub id: U64,
    pub to: AccountId,
    pub amount: U128,
    pub method: PaymentMethod,
    pub task_id: U64,
    pub expires_at: U64,
    pub status: PendingStatus,
    pub agent_key: PublicKey,
    pub agent_generation: U64,
}

#[near(serializers = [json])]
pub struct PaymentResult {
    pub id: Option<U64>,
    pub rule: RuleCode,
}

#[near(serializers = [json])]
pub struct StatsView {
    pub mode: Mode,
    pub rows: Vec<Observation>,
    pub observations: u32,
    pub max_daily: U128,
    pub proposal: Option<Proposal>,
    pub hard_daily_cap: U128,
    pub committed_today: U128,
    pub reserved: U128,
    pub watcher: Option<AccountId>,
    pub tasks: Vec<RegisteredTask>,
}

#[near(serializers = [borsh])]
pub struct LearningState {
    pub(crate) mode: Mode,
    pub(crate) config: Option<LearningConfig>,
    pub(crate) rows: Vec<Observation>,
    pub(crate) observations: u32,
    pub(crate) max_daily: u128,
    pub(crate) proposal: Option<Proposal>,
    pub(crate) pending: Vec<PendingPayment>,
    pub(crate) next_id: u64,
    pub(crate) tasks: Vec<RegisteredTask>,
}

impl LearningState {
    pub(crate) fn new(config: Option<LearningConfig>, owner: &AccountId) -> Self {
        let mut tasks = Vec::new();
        if let Some(c) = &config {
            require!(
                c.observe_single_cap.0 > 0
                    && c.observe_single_cap.0 <= OBSERVE_SINGLE_CAP
                    && c.min_count > 0
                    && c.min_observations >= c.min_count
                    && c.min_observations <= 1_000_000
                    && c.watcher != *owner
                    && c.watcher != env::current_account_id()
                    && c.watcher.as_str().ends_with(".testnet")
                    && c.watcher_key != env::signer_account_pk()
                    && c.watcher_expires_at_ns.0 > env::block_timestamp()
                    && c.watcher_max_calls > 0
                    && c.pending_timeout_ns.0 > 0
                    && c.pending_timeout_ns.0 <= NS_PER_DAY
                    && c.tasks.len() <= MAX_TASKS,
                "InvalidPolicy"
            );
            for condition in &c.tasks {
                require!(
                    condition.peer.as_str().ends_with(".testnet")
                        && condition.peer != env::current_account_id()
                        && condition.max_amount.0 > 0
                        && condition.budget.0 >= condition.max_amount.0
                        && condition.expires_at_ns.0 > env::block_timestamp()
                        && !tasks
                            .iter()
                            .any(|t: &RegisteredTask| t.condition.task_id == condition.task_id),
                    "InvalidPolicy"
                );
                tasks.push(RegisteredTask {
                    condition: condition.clone(),
                    hash: Base64VecU8(env::sha256(near_sdk::borsh::to_vec(condition).unwrap())),
                    spent: U128(0),
                });
            }
        }
        Self {
            mode: if config.is_some() {
                Mode::Observe
            } else {
                Mode::Enforce
            },
            config,
            rows: vec![],
            observations: 0,
            max_daily: 0,
            proposal: None,
            pending: vec![],
            next_id: 0,
            tasks,
        }
    }
}

#[near]
impl Contract {
    pub fn request_payment(
        &mut self,
        to: AccountId,
        amount: U128,
        method: PaymentMethod,
        task_id: U64,
    ) -> PaymentResult {
        let key = env::signer_account_pk();
        self.authorize_agent(&key)
            .unwrap_or_else(|c| env::panic_str(rule_str(&c)));
        require!(amount.0 > 0, "ZeroAmount");
        require!(
            to != env::current_account_id() && to.as_str().ends_with(".testnet"),
            "Unauthorized"
        );
        let check = self.check_direct(&to, &ObservedMethod::Pay, amount.0);
        match check {
            Ok(()) => {
                // Legacy-mode requests retain the explicit owner allowlist.
                require!(
                    self.learning.mode == Mode::Observe || self.recipient_allowed(&to),
                    "NotAllowlisted"
                );
                self.charge(&key, amount.0)
                    .unwrap_or_else(|c| env::panic_str(rule_str(&c)));
                self.record_observation(to.clone(), ObservedMethod::Pay, amount.0);
                emit_payment("payment", None, &to, amount.0, RuleCode::Ok);
                drop(Promise::new(to).transfer(NearToken::from_yoctonear(amount.0)));
                PaymentResult {
                    id: None,
                    rule: RuleCode::Ok,
                }
            }
            Err(RuleCode::NotInLearnedPolicy) => {
                let config = self.learning.config.as_ref().expect("InvalidPolicy");
                let expires_at = env::block_timestamp()
                    .checked_add(config.pending_timeout_ns.0)
                    .expect("Timeout");
                require!(
                    self.learning.pending.len() < MAX_PENDING
                        || self
                            .learning
                            .pending
                            .iter()
                            .any(|p| p.status != PendingStatus::Pending),
                    "PendingFull"
                );
                self.current_spend()
                    .checked_add(amount.0)
                    .filter(|n| *n <= self.daily_limit)
                    .expect("DailyLimit");
                let id = self.learning.next_id.checked_add(1).expect("PendingFull");
                let mut agent = self.agents.get(&key).cloned().expect("Unauthorized");
                agent.used_calls = agent.used_calls.checked_add(1).expect("MaxCalls");
                let generation = agent.generation;
                // Reservations are separate from executed spend, and survive day rollover.
                self.agents.insert(key.clone(), agent);
                if self.learning.pending.len() == MAX_PENDING {
                    let old = self
                        .learning
                        .pending
                        .iter()
                        .position(|p| p.status != PendingStatus::Pending)
                        .unwrap();
                    self.learning.pending.remove(old);
                }
                self.learning.pending.push(PendingPayment {
                    id: U64(id),
                    to: to.clone(),
                    amount,
                    method,
                    task_id,
                    expires_at: U64(expires_at),
                    status: PendingStatus::Pending,
                    agent_key: key,
                    agent_generation: U64(generation),
                });
                self.learning.next_id = id;
                emit_payment("payment", Some(U64(id)), &to, amount.0, RuleCode::Pending);
                PaymentResult {
                    id: Some(U64(id)),
                    rule: RuleCode::Pending,
                }
            }
            Err(c) => env::panic_str(rule_str(&c)),
        }
    }

    /// Owner-only, deterministic snapshot. Proposed mode freezes observation.
    pub fn propose_policy(&mut self, margin_bps: u32, min_count: u32) -> Proposal {
        self.require_owner();
        require!(self.learning.mode != Mode::Enforce, "InvalidProposal");
        let config = self.learning.config.as_ref().expect("InvalidPolicy");
        require!(
            margin_bps <= 10_000
                && min_count >= config.min_count
                && self.learning.observations >= config.min_observations,
            "InvalidProposal"
        );
        let rows: Vec<_> = self
            .learning
            .rows
            .iter()
            .filter(|r| r.count >= min_count)
            .map(|r| LearnedLimit {
                peer: r.peer.clone(),
                method: r.method.clone(),
                cap: U128(with_margin(r.max_amount.0, margin_bps)),
            })
            .collect();
        require!(!rows.is_empty(), "InvalidProposal");
        let mut proposal = Proposal {
            rows,
            daily_cap: U128(with_margin(self.learning.max_daily, margin_bps)),
            margin_bps,
            min_count,
            observations: self.learning.observations,
            first_ts: U64(self
                .learning
                .rows
                .iter()
                .map(|r| r.first_ts.0)
                .min()
                .unwrap()),
            last_ts: U64(self
                .learning
                .rows
                .iter()
                .map(|r| r.last_ts.0)
                .max()
                .unwrap()),
            hash: Base64VecU8(vec![]),
        };
        // The hash covers all fields with its own slot empty, plus the wallet id.
        proposal.hash = Base64VecU8(env::sha256(
            near_sdk::borsh::to_vec(&(env::current_account_id(), &proposal)).unwrap(),
        ));
        self.learning.mode = Mode::Proposed;
        self.learning.proposal = Some(proposal.clone());
        emit(
            "policy_proposed",
            near_sdk::serde_json::json!({
                "amount": proposal.daily_cap,
                "rule": RuleCode::Ok,
                "hash": proposal.hash,
                "peers": proposal.rows,
                "daily_cap": proposal.daily_cap,
            }),
        );
        proposal
    }

    pub fn approve_policy(&mut self, hash: Base64VecU8) {
        self.require_owner();
        require!(self.learning.mode == Mode::Proposed, "InvalidProposal");
        let proposal = self.learning.proposal.as_ref().expect("InvalidProposal");
        require!(hash.0 == proposal.hash.0, "InvalidProposal");
        // New prefixes discard every legacy allow entry, including unobserved ones.
        self.allowed_recipients = LookupMap::new(b"lr".as_slice());
        self.allowed_methods = LookupMap::new(b"lm".as_slice());
        for row in &proposal.rows {
            self.allowed_recipients.insert(row.peer.clone(), true);
            if let ObservedMethod::Call(name) = &row.method {
                self.allowed_methods.insert(name.clone(), true);
            }
        }
        self.learning.mode = Mode::Enforce;
        emit(
            "policy_approved",
            near_sdk::serde_json::json!({
                "amount": proposal.daily_cap,
                "rule": RuleCode::Ok,
                "hash": proposal.hash,
                "peers": proposal.rows,
                "daily_cap": proposal.daily_cap,
            }),
        );
    }

    /// Owner may substitute for the watcher; a watcher must satisfy registered task constraints.
    pub fn cosign(&mut self, id: U64) -> Promise {
        require!(!self.paused, "Paused");
        let watcher_key = self.authorize_approver();
        let index = self.pending_index(id);
        let payment = self.learning.pending[index].clone();
        require!(env::block_timestamp() < payment.expires_at.0, "Timeout");
        let source = self.agents.get(&payment.agent_key).expect("Unauthorized");
        require!(
            source.active
                && env::block_timestamp() < source.expires_at_ns
                && source.generation == payment.agent_generation.0,
            "KeyInactive"
        );
        require!(self.current_spend() <= self.daily_limit, "DailyLimit");
        require!(
            !self
                .blocked_recipients
                .get(&payment.to)
                .copied()
                .unwrap_or(false),
            "NotAllowlisted"
        );
        let task_index = if watcher_key.is_some() {
            let i = self
                .learning
                .tasks
                .iter()
                .position(|t| t.condition.task_id == payment.task_id)
                .expect("TaskCondition");
            let task = &self.learning.tasks[i];
            let c = &task.condition;
            require!(
                c.peer == payment.to
                    && c.method == payment.method
                    && payment.amount.0 <= c.max_amount.0
                    && env::block_timestamp() < c.expires_at_ns.0
                    && task
                        .spent
                        .0
                        .checked_add(payment.amount.0)
                        .is_some_and(|s| s <= c.budget.0),
                "TaskCondition"
            );
            Some(i)
        } else {
            None
        };
        let spent = self
            .actual_spend()
            .checked_add(payment.amount.0)
            .expect("DailyLimit");
        self.consume_approver(watcher_key);
        if let Some(i) = task_index {
            self.learning.tasks[i].spent.0 += payment.amount.0;
        }
        self.learning.pending[index].status = PendingStatus::Cosigned;
        self.spent_today = spent;
        self.day_bucket = utc_day(env::block_timestamp());
        emit_payment(
            "payment",
            Some(id),
            &payment.to,
            payment.amount.0,
            RuleCode::Cosigned,
        );
        Promise::new(payment.to).transfer(NearToken::from_yoctonear(payment.amount.0))
    }

    pub fn veto(&mut self, id: U64) {
        let watcher_key = self.authorize_approver();
        let index = self.pending_index(id);
        require!(
            env::block_timestamp() < self.learning.pending[index].expires_at.0,
            "Timeout"
        );
        self.consume_approver(watcher_key);
        self.learning.pending[index].status = PendingStatus::Vetoed;
        let payment = &self.learning.pending[index];
        emit_payment(
            "payment",
            Some(id),
            &payment.to,
            payment.amount.0,
            RuleCode::Vetoed,
        );
    }

    pub fn expire(&mut self, id: U64) {
        let index = self.pending_index(id);
        require!(
            env::block_timestamp() >= self.learning.pending[index].expires_at.0,
            "Timeout"
        );
        self.learning.pending[index].status = PendingStatus::Expired;
        let payment = &self.learning.pending[index];
        emit_payment(
            "payment",
            Some(id),
            &payment.to,
            payment.amount.0,
            RuleCode::Expired,
        );
    }

    pub fn get_stats(&self) -> StatsView {
        StatsView {
            mode: self.learning.mode,
            rows: self.learning.rows.clone(),
            observations: self.learning.observations,
            max_daily: U128(self.learning.max_daily),
            proposal: self.learning.proposal.clone(),
            hard_daily_cap: U128(self.daily_limit),
            committed_today: U128(self.current_spend()),
            reserved: U128(self.reserved_total()),
            watcher: self.learning.config.as_ref().map(|c| c.watcher.clone()),
            tasks: self.learning.tasks.clone(),
        }
    }

    pub fn get_pending(&self, id: Option<U64>) -> Vec<PendingPayment> {
        self.learning
            .pending
            .iter()
            .filter(|p| id.is_none_or(|id| p.id == id))
            .cloned()
            .collect()
    }
}

impl Contract {
    pub(crate) fn reserved_total(&self) -> u128 {
        self.learning
            .pending
            .iter()
            .filter(|p| p.status == PendingStatus::Pending)
            .fold(0u128, |total, p| {
                total.checked_add(p.amount.0).expect("DailyLimit")
            })
    }

    pub(crate) fn check_direct(
        &self,
        peer: &AccountId,
        method: &ObservedMethod,
        amount: u128,
    ) -> Result<(), RuleCode> {
        if self.blocked_recipients.get(peer).copied().unwrap_or(false) {
            return Err(RuleCode::NotAllowlisted);
        }
        match self.learning.mode {
            Mode::Observe => {
                let config = self.learning.config.as_ref().expect("InvalidPolicy");
                if amount > config.observe_single_cap.0
                    || self
                        .current_spend()
                        .checked_add(amount)
                        .is_none_or(|n| n > OBSERVE_DAILY_CAP)
                {
                    return Err(RuleCode::ObserveLimit);
                }
                let row = self
                    .learning
                    .rows
                    .iter()
                    .find(|r| &r.peer == peer && &r.method == method);
                if row.is_none() && self.learning.rows.len() >= MAX_ROWS {
                    return Err(RuleCode::StatsFull);
                }
                if self.learning.observations == u32::MAX
                    || row.is_some_and(|r| {
                        r.count == u32::MAX || r.sum.0.checked_add(amount).is_none()
                    })
                {
                    return Err(RuleCode::ObserveLimit);
                }
                Ok(())
            }
            Mode::Proposed => Err(RuleCode::PolicyProposed),
            Mode::Enforce => {
                if let Some(p) = &self.learning.proposal {
                    let row = p
                        .rows
                        .iter()
                        .find(|r| &r.peer == peer && &r.method == method);
                    if row.is_none_or(|r| amount > r.cap.0)
                        || self
                            .current_spend()
                            .checked_add(amount)
                            .is_none_or(|n| n > p.daily_cap.0)
                    {
                        return Err(RuleCode::NotInLearnedPolicy);
                    }
                }
                Ok(())
            }
        }
    }

    pub(crate) fn record_observation(
        &mut self,
        peer: AccountId,
        method: ObservedMethod,
        amount: u128,
    ) {
        if self.learning.mode != Mode::Observe {
            return;
        }
        let now = U64(env::block_timestamp());
        if let Some(row) = self
            .learning
            .rows
            .iter_mut()
            .find(|r| r.peer == peer && r.method == method)
        {
            row.count += 1;
            row.sum.0 += amount;
            row.max_amount.0 = row.max_amount.0.max(amount);
            row.last_ts = now;
        } else {
            self.learning.rows.push(Observation {
                peer,
                method,
                count: 1,
                sum: U128(amount),
                max_amount: U128(amount),
                first_ts: now,
                last_ts: now,
            });
        }
        self.learning.observations += 1;
        self.learning.max_daily = self.learning.max_daily.max(self.actual_spend());
    }

    fn pending_index(&self, id: U64) -> usize {
        self.learning
            .pending
            .iter()
            .position(|p| p.id == id && p.status == PendingStatus::Pending)
            .expect("NotPending")
    }

    fn authorize_approver(&self) -> Option<PublicKey> {
        let config = self.learning.config.as_ref().expect("Unauthorized");
        let key = env::signer_account_pk();
        if env::predecessor_account_id() == self.owner
            && env::signer_account_id() == self.owner
            && self.agents.get(&key).is_none()
        {
            return None;
        }
        require!(
            env::predecessor_account_id() == config.watcher
                && env::signer_account_id() == config.watcher
                && key == config.watcher_key,
            "Unauthorized"
        );
        let meta = self.agents.get(&key).expect("Unauthorized");
        require!(meta.active, "KeyInactive");
        require!(env::block_timestamp() < meta.expires_at_ns, "Expired");
        require!(meta.used_calls < meta.max_calls, "MaxCalls");
        Some(key)
    }

    fn consume_approver(&mut self, key: Option<PublicKey>) {
        if let Some(key) = key {
            self.agents.get_mut(&key).unwrap().used_calls += 1;
        }
    }
}

/// Round up to avoid losing a fractional yocto, without overflowing the product.
fn with_margin(amount: u128, margin_bps: u32) -> u128 {
    let bps = u128::from(margin_bps);
    amount
        .checked_add((amount / 10_000).checked_mul(bps).expect("InvalidProposal"))
        .and_then(|n| n.checked_add((amount % 10_000 * bps).div_ceil(10_000)))
        .expect("InvalidProposal")
}

fn emit_payment(event: &str, id: Option<U64>, peer: &AccountId, amount: u128, code: RuleCode) {
    emit(
        event,
        near_sdk::serde_json::json!({"id": id, "to": peer, "amount": U128(amount), "rule": code}),
    );
}
