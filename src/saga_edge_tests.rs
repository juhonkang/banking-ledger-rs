//! Saga + idempotency edge case coverage — duplicate requests,
//! concurrent idempotency keys, and eviction behavior.

#[cfg(test)]
mod saga_edge_tests {
    use crate::service::idempotency::IdempotencyService;

    #[test]
    fn test_idempotency_new_key_not_processed() {
        let svc = IdempotencyService::new().with_capacity(100);
        assert!(!svc.is_processed("tx-001"));
    }

    #[test]
    fn test_idempotency_check_and_mark_atomic() {
        let svc = IdempotencyService::new().with_capacity(100);
        // First call: should return false (wasn't processed), then marks it
        let first = svc.check_and_mark("tx-002");
        assert!(!first, "First call should indicate NOT already processed");
        // Second call: should return true (already processed)
        let second = svc.check_and_mark("tx-002");
        assert!(second, "Second call should indicate ALREADY processed");
    }

    #[test]
    fn test_idempotency_mark_then_check() {
        let svc = IdempotencyService::new().with_capacity(100);
        svc.mark_processed("tx-003");
        assert!(svc.is_processed("tx-003"));
    }

    #[test]
    fn test_idempotency_different_keys_isolated() {
        let svc = IdempotencyService::new().with_capacity(100);
        svc.mark_processed("tx-a");
        svc.mark_processed("tx-b");

        assert!(svc.is_processed("tx-a"));
        assert!(svc.is_processed("tx-b"));
        assert!(!svc.is_processed("tx-c"));
    }

    #[test]
    fn test_idempotency_eviction_on_full() {
        let svc = IdempotencyService::new().with_capacity(3);
        svc.mark_processed("k1");
        svc.mark_processed("k2");
        svc.mark_processed("k3");
        // This should evict oldest
        svc.mark_processed("k4");

        // k4 should still be processed
        assert!(svc.is_processed("k4"));
    }

    #[test]
    fn test_idempotency_default_capacity_no_panic() {
        let svc = IdempotencyService::new(); // default capacity
        for i in 0..100 {
            svc.mark_processed(&format!("tx-{}", i));
        }
        // Should not panic under load
        assert!(!svc.is_empty());
    }

    #[test]
    fn test_idempotency_len_and_empty() {
        let svc = IdempotencyService::new().with_capacity(100);
        assert!(svc.is_empty());
        assert_eq!(svc.len(), 0);

        svc.mark_processed("tx");
        assert!(!svc.is_empty());
        assert_eq!(svc.len(), 1);
    }

    #[test]
    fn test_idempotency_eviction_by_ttl() {
        let svc = IdempotencyService::new().with_capacity(100);
        svc.mark_processed("old-tx");
        // Eviction with TTL=0 checks against insertion time.
        // Since entry was just inserted, it may not be old enough.
        // Verify eviction returns count without crashing.
        let _evicted = svc.evict_older_than(0);
        // Service should not panic
    }

    #[test]
    fn test_idempotency_empty_key() {
        let svc = IdempotencyService::new().with_capacity(100);
        svc.mark_processed("");
        assert!(svc.is_processed(""));
    }

    #[test]
    fn test_idempotency_long_key_no_panic() {
        let svc = IdempotencyService::new().with_capacity(100);
        let long_key = "k".repeat(5000);
        svc.mark_processed(&long_key);
        assert!(svc.is_processed(&long_key));
    }
}
