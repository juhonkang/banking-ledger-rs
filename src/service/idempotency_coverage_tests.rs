//! Comprehensive coverage tests for idempotency module.
//! Covers: concurrent dedup, eviction at capacity, negative TTL,
//! custom capacity, and empty-service edge cases.

#[cfg(test)]
mod tests {
    use crate::service::idempotency::{IdempotentHandler, IdempotencyService};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    // ── Test 1: Concurrent check_and_mark under thread contention ──────────

    #[test]
    fn test_concurrent_check_and_mark_unique_keys() {
        // Many threads each inserting a unique key — none should return "already processed".
        let svc = Arc::new(IdempotencyService::new());
        let thread_count = 16;
        let keys_per_thread = 500;
        let mut handles = Vec::new();

        for t in 0..thread_count {
            let svc = Arc::clone(&svc);
            handles.push(thread::spawn(move || {
                let mut dupes = 0usize;
                for i in 0..keys_per_thread {
                    let key = format!("thread-{t}-key-{i}");
                    if svc.check_and_mark(&key) {
                        dupes += 1;
                    }
                }
                dupes
            }));
        }

        let total_dupes: usize = handles.into_iter().map(|h| h.join().unwrap()).sum();
        assert_eq!(
            total_dupes, 0,
            "No duplicates expected — all keys are unique per thread"
        );
        assert_eq!(svc.len(), thread_count * keys_per_thread);
    }

    #[test]
    fn test_concurrent_check_and_mark_same_key() {
        // All threads race on the SAME single key — exactly one should "win".
        let svc = Arc::new(IdempotencyService::new());
        let key = "racing-key";
        let thread_count = 32;

        let handles: Vec<_> = (0..thread_count)
            .map(|_| {
                let svc = Arc::clone(&svc);
                let key = key.to_string();
                thread::spawn(move || svc.check_and_mark(&key))
            })
            .collect();

        let results: Vec<bool> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let first_wins = results.iter().filter(|&&b| !b).count(); // false = first insert
        let dupes = results.iter().filter(|&&b| b).count(); // true = already there

        assert_eq!(
            first_wins, 1,
            "Exactly one thread should get the first insert"
        );
        assert_eq!(dupes, thread_count - 1, "All others should see it as duplicate");
        assert_eq!(svc.len(), 1);
    }

    // ── Test 2: evict_oldest_batch triggered at capacity ────────────────────

    #[test]
    fn test_evict_oldest_batch_triggered_by_capacity() {
        // Use a tiny capacity so eviction kicks in quickly.
        // with_capacity(10) → evict_oldest_batch removes max(1, 10/10) = 1 entry.
        let svc = IdempotencyService::new().with_capacity(10);

        // Fill to exactly capacity (10 entries)
        for i in 0..10 {
            svc.mark_processed(&format!("key-{i}"));
        }
        assert_eq!(svc.len(), 10, "Should be at capacity");

        // Insert one more — triggers eviction, oldest 1 entry removed, new one added.
        svc.mark_processed("overflow-key");
        assert_eq!(
            svc.len(),
            10,
            "Still at capacity after eviction + insert"
        );

        // The oldest entry (key-0) should be gone.
        assert!(
            !svc.is_processed("key-0"),
            "Oldest entry should be evicted"
        );
        // The newest entries should still be present.
        assert!(svc.is_processed("key-9"));
        assert!(svc.is_processed("overflow-key"));
    }

    #[test]
    fn test_evict_oldest_batch_multiple_rounds() {
        // Capacity 10 → batch size = max(1, 10/10) = 1.
        // Each overflow insert evicts exactly 1 entry, so len stays at capacity.
        let svc = IdempotencyService::new().with_capacity(10);

        // Fill to capacity
        for i in 0..10 {
            svc.mark_processed(&format!("k-{:02}", i));
        }
        assert_eq!(svc.len(), 10);

        // Insert 20 overflow entries — each triggers 1 eviction.
        // 20 evicted, 20 inserted → len stays at 10.
        for i in 10..30 {
            svc.mark_processed(&format!("k-{:02}", i));
        }

        // len should still be 10 — eviction keeps it at capacity
        assert_eq!(svc.len(), 10, "Eviction should keep size at capacity");

        // The most recent 10 entries (k-20..k-29) should be present.
        // After 20 rounds of 1-eviction + 1-insert, older entries are gone.
        for i in 20..30 {
            assert!(
                svc.is_processed(&format!("k-{:02}", i)),
                "k-{:02} must be present",
                i
            );
        }

        // Original entries (k-00..k-09) are long gone
        let originals_left: usize = (0..10)
            .filter(|i| svc.is_processed(&format!("k-{:02}", i)))
            .count();
        assert_eq!(
            originals_left, 0,
            "All original entries should be evicted after 20 rounds"
        );
    }

    // ── Test 3: Negative TTL edge case ──────────────────────────────────────

    #[test]
    fn test_negative_ttl_clamped_to_one_second() {
        // evict_older_than uses ttl.max(1), so negative → 1 second.
        let svc = IdempotencyService::new();

        // Mark an entry now (stale), wait 2 seconds, mark another (fresh).
        svc.mark_processed("stale");
        thread::sleep(Duration::from_secs(2));
        svc.mark_processed("fresh");

        // Negative TTL (-5) should clamp to 1 second.
        // Entries older than 1 second before now get evicted → "stale" is 2s old.
        let evicted = svc.evict_older_than(-5);
        assert_eq!(
            evicted, 1,
            "Only the 2-second-old 'stale' entry should be evicted"
        );
        assert!(!svc.is_processed("stale"));
        assert!(svc.is_processed("fresh"), "Fresh entry should survive");
    }

    #[test]
    fn test_zero_ttl_clamped_to_one_second() {
        // Zero also clamps to 1 second via .max(1)
        let svc = IdempotencyService::new();

        svc.mark_processed("old");
        thread::sleep(Duration::from_secs(2));
        svc.mark_processed("just-now");

        let evicted = svc.evict_older_than(0);
        assert_eq!(
            evicted, 1,
            "Zero TTL clamps to 1s, evicts the 2s-old entry"
        );
        assert!(!svc.is_processed("old"));
        assert!(svc.is_processed("just-now"));
    }

    #[test]
    fn test_very_large_negative_ttl_still_clamped() {
        // i64::MIN should still clamp to 1 second without panicking.
        let svc = IdempotencyService::new();

        svc.mark_processed("ancient");
        thread::sleep(Duration::from_secs(2));

        // i64::MIN should not panic or overflow — clamped to 1 by .max(1)
        let evicted = svc.evict_older_than(i64::MIN);
        assert_eq!(evicted, 1);
    }

    // ── Test 4: with_capacity custom sizing ─────────────────────────────────

    #[test]
    fn test_with_capacity_tiny() {
        let svc = IdempotencyService::new().with_capacity(3);
        svc.mark_processed("a");
        svc.mark_processed("b");
        svc.mark_processed("c");
        assert_eq!(svc.len(), 3);

        // Trigger eviction: capacity 3 → batch = max(1, 3/10) = 1
        svc.mark_processed("d");
        assert_eq!(svc.len(), 3);
        assert!(!svc.is_processed("a"), "Oldest evicted");
        assert!(svc.is_processed("b"));
        assert!(svc.is_processed("c"));
        assert!(svc.is_processed("d"));
    }

    #[test]
    fn test_with_capacity_large_custom() {
        let svc = IdempotencyService::new().with_capacity(1_000);
        for i in 0..1_000 {
            svc.mark_processed(&format!("k-{}", i));
        }
        assert_eq!(svc.len(), 1_000);

        // One more triggers batch eviction of max(1, 1000/10) = 100
        svc.mark_processed("overflow");
        assert_eq!(svc.len(), 901); // 1000 - 100 + 1
    }

    #[test]
    fn test_with_capacity_one() {
        // Minimum meaningful capacity: 1
        let svc = IdempotencyService::new().with_capacity(1);
        svc.mark_processed("only");
        assert_eq!(svc.len(), 1);

        // Inserting another triggers eviction of max(1, 1/10) = 1 entry
        svc.mark_processed("replacement");
        assert_eq!(svc.len(), 1);
        assert!(!svc.is_processed("only"));
        assert!(svc.is_processed("replacement"));
    }

    #[test]
    fn test_with_capacity_zero_rounds_up_eviction_batch() {
        // with_capacity(0) → eviction batch = max(1, 0/10) = 1
        let svc = IdempotencyService::new().with_capacity(0);

        // At capacity 0, every mark_processed triggers eviction.
        // The first insert: len=0, 0 >= 0 → evict (no-op on empty), insert → len=1.
        // The second insert: len=1, 1 >= 0 → evict oldest 1 → len=0, insert → len=1.
        svc.mark_processed("first");
        assert_eq!(svc.len(), 1);
        svc.mark_processed("second");
        assert_eq!(svc.len(), 1);
        assert!(!svc.is_processed("first"), "First evicted at capacity 0");
        assert!(svc.is_processed("second"));
    }

    // ── Test 5: Empty service edge cases ────────────────────────────────────

    #[test]
    fn test_empty_service_operations() {
        let svc = IdempotencyService::new();

        assert!(svc.is_empty());
        assert_eq!(svc.len(), 0);

        // evict_older_than on empty service
        let evicted = svc.evict_older_than(3600);
        assert_eq!(evicted, 0);
        assert!(svc.is_empty());

        // is_processed on empty
        assert!(!svc.is_processed("never-seen"));
    }

    #[test]
    fn test_empty_handler() {
        let call_count = std::sync::atomic::AtomicUsize::new(0);
        let handler = IdempotentHandler::new(|_: &str| {
            call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        });

        assert_eq!(handler.processed_count(), 0);
        handler.process("evt-A").unwrap();
        assert_eq!(handler.processed_count(), 1);
        assert_eq!(
            call_count.load(std::sync::atomic::Ordering::SeqCst),
            1
        );

        // Duplicate — not called
        handler.process("evt-A").unwrap();
        assert_eq!(handler.processed_count(), 1);
        assert_eq!(
            call_count.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }

    // ── Bonus: Default implementation ───────────────────────────────────────

    #[test]
    fn test_default_service_has_default_capacity() {
        let svc: IdempotencyService = Default::default();
        // Fill 1_000 entries into a default-capacity service (100_000) — no eviction.
        for i in 0..1_000 {
            svc.mark_processed(&format!("def-{}", i));
        }
        assert_eq!(svc.len(), 1_000);
    }

    // ── Bonus: IdempotentHandler failure path ───────────────────────────────

    #[test]
    fn test_handler_failure_then_retry_succeeds() {
        let attempts = std::sync::atomic::AtomicUsize::new(0);
        let handler = IdempotentHandler::new(|tx_id: &str| {
            let n = attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n < 2 {
                Err("transient failure".to_string())
            } else {
                assert_eq!(tx_id, "retry-me");
                Ok(())
            }
        });

        // First two calls fail, handler not marked as processed
        assert!(handler.process("retry-me").is_err());
        assert!(handler.process("retry-me").is_err());
        assert_eq!(handler.processed_count(), 0);

        // Third call succeeds — now it's marked
        assert!(handler.process("retry-me").is_ok());
        assert_eq!(handler.processed_count(), 1);

        // Subsequent calls skip the handler
        assert!(handler.process("retry-me").is_ok());
        assert_eq!(handler.processed_count(), 1); // no change
    }

    // ── Bonus: check_and_mark vs is_processed consistency ──────────────────

    #[test]
    fn test_check_and_mark_consistency() {
        let svc = IdempotencyService::new();

        // First time: check_and_mark returns false (new), marks it
        assert!(!svc.check_and_mark("idem-1"));
        assert!(svc.is_processed("idem-1"));

        // Second time: check_and_mark returns true (already there)
        assert!(svc.check_and_mark("idem-1"));
        assert!(svc.is_processed("idem-1"));

        // Different key: not processed
        assert!(!svc.is_processed("idem-2"));
        assert!(!svc.check_and_mark("idem-2"));
        assert!(svc.is_processed("idem-2"));
    }
}
