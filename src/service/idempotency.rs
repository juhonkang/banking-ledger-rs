//! Idempotent event consumer — deduplicates events by transaction_id.
//! Uses DashMap for lock-free dedup with configurable TTL-based eviction.

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use uuid::Uuid;

/// Dedicated idempotency service for event consumers.
/// Every event carries a transaction_id used as the dedup key.
pub struct IdempotencyService {
    processed: DashMap<String, DateTime<Utc>>,
}

impl IdempotencyService {
    pub fn new() -> Self {
        Self {
            processed: DashMap::new(),
        }
    }

    /// Check if this transaction has already been processed.
    /// Returns true if already seen (idempotent skip).
    pub fn is_processed(&self, transaction_id: &str) -> bool {
        self.processed.contains_key(transaction_id)
    }

    /// Atomically check AND mark a transaction as processed.
    /// Returns true if it was already processed, false if newly marked.
    /// Uses DashMap's entry API for atomicity.
    pub fn check_and_mark(&self, transaction_id: &str) -> bool {
        self.processed
            .insert(transaction_id.to_string(), Utc::now())
            .is_some() // None = newly inserted (not duplicate), Some = already existed
    }

    /// Mark a transaction as processed without checking.
    pub fn mark_processed(&self, transaction_id: &str) {
        self.processed
            .insert(transaction_id.to_string(), Utc::now());
    }

    /// Evict entries older than the given seconds.
    pub fn evict_older_than(&self, ttl_seconds: i64) -> usize {
        let cutoff = Utc::now() - chrono::Duration::seconds(ttl_seconds);
        let before = self.processed.len();
        self.processed.retain(|_, ts| *ts > cutoff);
        before - self.processed.len()
    }

    /// Number of tracked transaction IDs.
    pub fn len(&self) -> usize {
        self.processed.len()
    }

    pub fn is_empty(&self) -> bool {
        self.processed.is_empty()
    }
}

/// Generic idempotent event handler.
/// Wraps a closure with dedup logic.
pub struct IdempotentHandler<F>
where
    F: Fn(&str) -> Result<(), String>,
{
    dedup: IdempotencyService,
    handler: F,
}

impl<F> IdempotentHandler<F>
where
    F: Fn(&str) -> Result<(), String>,
{
    pub fn new(handler: F) -> Self {
        Self {
            dedup: IdempotencyService::new(),
            handler,
        }
    }

    /// Process an event idempotently.
    /// If already processed, returns Ok(()) without calling handler.
    pub fn process(&self, transaction_id: &str) -> Result<(), String> {
        if self.dedup.is_processed(transaction_id) {
            tracing::info!(%transaction_id, "Duplicate event — skipped");
            return Ok(());
        }
        let result = (self.handler)(transaction_id);
        if result.is_ok() {
            self.dedup.mark_processed(transaction_id);
        }
        result
    }

    /// Number of distinct events processed.
    pub fn processed_count(&self) -> usize {
        self.dedup.len()
    }
}

impl Default for IdempotencyService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_idempotency_service_check_and_mark() {
        let svc = IdempotencyService::new();

        // First call: new
        assert!(!svc.check_and_mark("tx-1"));

        // Second call: duplicate
        assert!(svc.check_and_mark("tx-1"));

        // Third: still duplicate
        assert!(svc.is_processed("tx-1"));
    }

    #[test]
    fn test_idempotency_service_eviction() {
        let svc = IdempotencyService::new();
        svc.mark_processed("old-tx");

        // Artificially set an old timestamp
        let old_time = Utc::now() - chrono::Duration::seconds(3600);
        svc.processed.insert("old-tx".to_string(), old_time);

        let evicted = svc.evict_older_than(1800); // 30 min TTL
        assert_eq!(evicted, 1);
        assert!(!svc.is_processed("old-tx"));
    }

    #[test]
    fn test_idempotent_handler_skip_duplicate() {
        let mut call_count = std::sync::atomic::AtomicUsize::new(0);
        let handler = IdempotentHandler::new(|tx_id: &str| {
            call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        });

        handler.process("evt-1").unwrap();
        handler.process("evt-1").unwrap(); // duplicate
        handler.process("evt-1").unwrap(); // duplicate again
        handler.process("evt-2").unwrap();

        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(handler.processed_count(), 2); // evt-1, evt-2
    }

    #[test]
    fn test_idempotent_handler_does_not_mark_on_failure() {
        let handler = IdempotentHandler::new(|tx_id: &str| {
            if tx_id == "fail" {
                return Err("simulated failure".to_string());
            }
            Ok(())
        });

        assert!(handler.process("fail").is_err());
        assert_eq!(handler.processed_count(), 0);

        // Retry should succeed (not idempotently skipped)
        // But the handler will fail again — the point is dedup didn't mark it
        assert!(handler.process("fail").is_err());
        assert_eq!(handler.processed_count(), 0);
    }
}
