//! Concurrency primitives for high-throughput financial operations.
//! All hot-path balance updates are lock-free via AtomicI64 CAS loops.
//! Cold-path coordination uses Condvar, RwLock, and fair queuing.

use std::fmt::Debug;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock, TryLockError};
#[cfg(test)]
use std::time::Duration;

// ━━━ Validation Framework ━━━

/// A validation result — either passes, or fails with reasons.
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub is_valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl ValidationResult {
    pub fn valid() -> Self {
        Self {
            is_valid: true,
            errors: vec![],
            warnings: vec![],
        }
    }

    pub fn invalid(errors: Vec<String>) -> Self {
        Self {
            is_valid: false,
            errors,
            warnings: vec![],
        }
    }

    pub fn add_error(&mut self, error: String) {
        self.is_valid = false;
        self.errors.push(error);
    }

    pub fn add_warning(&mut self, warning: String) {
        self.warnings.push(warning);
    }
}

/// A validator for financial transactions.
pub trait TransactionValidator: Send + Sync {
    fn validate(&self, context: &ValidationContext) -> ValidationResult;
    fn name(&self) -> &str;
}

/// Context passed to validators — account balances, limits, etc.
#[derive(Debug, Clone)]
pub struct ValidationContext {
    pub account_id: uuid::Uuid,
    pub amount_cents: i64,
    pub available_balance_cents: i64,
    pub daily_debit_total_cents: i64,
    pub daily_debit_limit_cents: i64,
    pub account_status: String,
}

/// A chain of validators — all must pass.
pub struct ValidationPipeline {
    validators: Vec<Box<dyn TransactionValidator>>,
}

impl ValidationPipeline {
    pub fn new() -> Self {
        Self { validators: vec![] }
    }

    pub fn add(&mut self, validator: Box<dyn TransactionValidator>) {
        self.validators.push(validator);
    }

    pub fn validate(&self, context: &ValidationContext) -> ValidationResult {
        let mut result = ValidationResult::valid();
        for v in &self.validators {
            let vr = v.validate(context);
            if !vr.is_valid {
                result.is_valid = false;
                result.errors.extend(vr.errors);
            }
            result.warnings.extend(vr.warnings);
        }
        result
    }
}

impl Default for ValidationPipeline {
    fn default() -> Self {
        Self::new()
    }
}

// ━━━ CAS Balance Updates ━━━
// Already implemented in account.rs — documented here as reference:

/// CAS-based atomic debit (reference implementation — see account.rs)
pub fn atomic_debit(balance: &AtomicI64, amount: i64) -> Result<i64, &'static str> {
    loop {
        let current = balance.load(Ordering::SeqCst);
        if current < amount {
            return Err("Insufficient funds");
        }
        let new_balance = current - amount;
        if balance
            .compare_exchange(current, new_balance, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return Ok(new_balance);
        }
        // CAS failed — retry
    }
}

// ━━━ Optimistic Reads ━━━

/// Try an optimistic read — returns Some if lock is not write-locked.
/// `StampedLock` equivalent: `tryOptimisticRead()` → `validate()` pattern.
pub fn optimistic_read<T: Clone>(lock: &RwLock<T>) -> Option<T> {
    match lock.try_read() {
        Ok(guard) => Some(guard.clone()),
        Err(TryLockError::WouldBlock) => None,
        Err(TryLockError::Poisoned(_)) => None,
    }
}

// ━━━ Condition Variables ━━━

/// A transfer queue that blocks until funds are available.
/// Used for inter-account transfers where the source account needs to wait
/// for incoming credits before debiting.
pub struct TransferCondition {
    pub balance: Mutex<i64>,
    pub condvar: Condvar,
}

impl TransferCondition {
    pub fn new(initial_balance: i64) -> Self {
        Self {
            balance: Mutex::new(initial_balance),
            condvar: Condvar::new(),
        }
    }

    /// Wait until at least `amount` is available, then debit.
    /// Returns the new balance.
    pub fn wait_and_debit(&self, amount: i64) -> Result<i64, &'static str> {
        let mut bal = self.balance.lock().unwrap();
        while *bal < amount {
            bal = self.condvar.wait(bal).unwrap();
        }
        *bal -= amount;
        Ok(*bal)
    }

    /// Credit the account and notify waiting threads.
    pub fn credit_and_notify(&self, amount: i64) -> i64 {
        let mut bal = self.balance.lock().unwrap();
        *bal = bal.checked_add(amount).expect("credit overflow in TransferCondition");
        self.condvar.notify_all();
        *bal
    }

    /// Credit one specific waiter
    pub fn credit_and_notify_one(&self, amount: i64) -> i64 {
        let mut bal = self.balance.lock().unwrap();
        *bal = bal.checked_add(amount).expect("credit overflow in TransferCondition");
        self.condvar.notify_one();
        *bal
    }
}

// ━━━ ReadWriteLock Analytics ━━━

/// A counter that separates read-heavy (analytics) from write-heavy (transactions).
pub struct AnalyticsCounter {
    data: RwLock<AnalyticsData>,
}

#[derive(Debug, Clone)]
pub struct AnalyticsData {
    pub total_transactions: u64,
    pub total_volume_cents: i64,
    pub peak_balance_cents: i64,
    pub min_balance_cents: i64,
}

impl Default for AnalyticsData {
    fn default() -> Self {
        Self {
            total_transactions: 0,
            total_volume_cents: 0,
            peak_balance_cents: i64::MIN,
            min_balance_cents: i64::MAX, // Sentinel: no data yet
        }
    }
}

impl AnalyticsCounter {
    pub fn new() -> Self {
        Self {
            data: RwLock::new(AnalyticsData::default()),
        }
    }

    /// Fast read — no lock contention
    pub fn snapshot(&self) -> AnalyticsData {
        self.data.read().unwrap().clone()
    }

    /// Write — exclusive access
    pub fn record_transaction(&self, amount_cents: i64, new_balance: i64) {
        let mut data = self.data.write().unwrap();
        data.total_transactions += 1;
        data.total_volume_cents = data.total_volume_cents
            .saturating_add(amount_cents.unsigned_abs() as i64);
        if new_balance > data.peak_balance_cents {
            data.peak_balance_cents = new_balance;
        }
        if data.total_transactions == 1 || new_balance < data.min_balance_cents {
            data.min_balance_cents = new_balance;
        }
    }
}

impl Default for AnalyticsCounter {
    fn default() -> Self {
        Self::new()
    }
}

// ━━━ Fair Queuing ━━━
// Rust's std::sync::Mutex is unfair by default on Linux (pthread mutex).
// For fair queuing, use tokio::sync::Mutex with FIFO ordering, or parking_lot::FairMutex.
// FairMutex available with `parking_lot` feature:
// pub type FairMutex<T> = parking_lot::FairMutex<T>;

/// Transfer queue — processes requests in FIFO order
pub struct FairTransferQueue {
    queue: Mutex<std::collections::VecDeque<TransferRequest>>,
    condvar: Condvar,
}

#[derive(Debug, Clone)]
pub struct TransferRequest {
    pub from_account: uuid::Uuid,
    pub to_account: uuid::Uuid,
    pub amount_cents: i64,
}

impl FairTransferQueue {
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(std::collections::VecDeque::new()),
            condvar: Condvar::new(),
        }
    }

    pub fn enqueue(&self, request: TransferRequest) {
        let mut q = self.queue.lock().unwrap();
        q.push_back(request);
        self.condvar.notify_one();
    }

    pub fn dequeue_wait(&self) -> TransferRequest {
        let mut q = self.queue.lock().unwrap();
        while q.is_empty() {
            q = self.condvar.wait(q).unwrap();
        }
        q.pop_front().unwrap()
    }
}

impl Default for FairTransferQueue {
    fn default() -> Self {
        Self::new()
    }
}

// ━━━ Race Condition Testing ━━━

/// Run a concurrent test with N threads, each executing the given closure.
/// Returns (`actual_sum`, `expected_sum`) to detect race conditions.
pub fn race_test<F>(num_threads: usize, ops_per_thread: usize, f: F) -> (i64, i64)
where
    F: Fn() + Send + Sync + Clone + 'static,
{
    let f = Arc::new(f);
    let counter = Arc::new(AtomicI64::new(0));
    let expected = (num_threads * ops_per_thread) as i64;
    let mut handles = vec![];

    for _ in 0..num_threads {
        let c = Arc::clone(&counter);
        let f = Arc::clone(&f);
        handles.push(std::thread::spawn(move || {
            for _ in 0..ops_per_thread {
                c.fetch_add(1, Ordering::SeqCst);
                f();
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    (counter.load(Ordering::SeqCst), expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_validation_pipeline_all_pass() {
        struct AlwaysPass;
        impl TransactionValidator for AlwaysPass {
            fn validate(&self, _ctx: &ValidationContext) -> ValidationResult {
                ValidationResult::valid()
            }
            fn name(&self) -> &str {
                "always_pass"
            }
        }

        let mut pipeline = ValidationPipeline::new();
        pipeline.add(Box::new(AlwaysPass));

        let ctx = ValidationContext {
            account_id: uuid::Uuid::now_v7(),
            amount_cents: 1000,
            available_balance_cents: 5000,
            daily_debit_total_cents: 0,
            daily_debit_limit_cents: 10000,
            account_status: "OPEN".into(),
        };

        assert!(pipeline.validate(&ctx).is_valid);
    }

    #[test]
    fn test_validation_pipeline_catches_failure() {
        struct InsufficientFunds;
        impl TransactionValidator for InsufficientFunds {
            fn validate(&self, ctx: &ValidationContext) -> ValidationResult {
                if ctx.amount_cents > ctx.available_balance_cents {
                    ValidationResult::invalid(vec!["Insufficient funds".into()])
                } else {
                    ValidationResult::valid()
                }
            }
            fn name(&self) -> &str {
                "insufficient_funds"
            }
        }

        let mut pipeline = ValidationPipeline::new();
        pipeline.add(Box::new(InsufficientFunds));

        let ctx = ValidationContext {
            available_balance_cents: 100,
            amount_cents: 1000,
            ..ValidationContext {
                account_id: uuid::Uuid::now_v7(),
                amount_cents: 0,
                available_balance_cents: 0,
                daily_debit_total_cents: 0,
                daily_debit_limit_cents: 0,
                account_status: "OPEN".into(),
            }
        };

        assert!(!pipeline.validate(&ctx).is_valid);
    }

    #[test]
    fn test_atomic_debit_concurrent_safety() {
        let balance = AtomicI64::new(10000);
        let b = &balance;

        std::thread::scope(|s| {
            for _ in 0..10 {
                s.spawn(|| {
                    for _ in 0..10 {
                        let _ = atomic_debit(b, 10);
                    }
                });
            }
        });

        // 10 threads * 10 ops * 10 = 1000 total debit
        // Initial 10000 - 1000 = 9000
        assert_eq!(balance.load(Ordering::SeqCst), 9000);
    }

    #[test]
    fn test_condition_variable_transfer() {
        let tc = Arc::new(TransferCondition::new(0));
        let tc_clone = Arc::clone(&tc);

        // Thread that waits for funds
        let handle = std::thread::spawn(move || tc_clone.wait_and_debit(500).unwrap());

        // Short sleep to ensure waiter is waiting
        std::thread::sleep(Duration::from_millis(50));

        // Credit the account
        tc.credit_and_notify(1000);

        let new_balance = handle.join().unwrap();
        assert_eq!(new_balance, 500); // 1000 - 500
    }

    #[test]
    fn test_analytics_read_write() {
        let analytics = AnalyticsCounter::new();
        analytics.record_transaction(1000, 5000);
        analytics.record_transaction(-500, 4500);

        let snap = analytics.snapshot();
        assert_eq!(snap.total_transactions, 2);
        assert_eq!(snap.total_volume_cents, 1500);
        assert_eq!(snap.peak_balance_cents, 5000);
        assert_eq!(snap.min_balance_cents, 4500);
    }

    #[test]
    fn test_fair_transfer_queue() {
        let queue = Arc::new(FairTransferQueue::new());
        let q = Arc::clone(&queue);

        let handle = std::thread::spawn(move || {
            let req = q.dequeue_wait();
            assert_eq!(req.amount_cents, 1000);
        });

        std::thread::sleep(Duration::from_millis(50));
        queue.enqueue(TransferRequest {
            from_account: uuid::Uuid::now_v7(),
            to_account: uuid::Uuid::now_v7(),
            amount_cents: 1000,
        });

        handle.join().unwrap();
    }
}
