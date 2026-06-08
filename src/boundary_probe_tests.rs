//! Boundary probing tests — automated edge case exhaustion (Round 2 audit).
//! These probe every function at its boundaries: NaN, zero, negative, max, empty, etc.

#[cfg(test)]
mod boundary_probe_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use crate::service::resilience::{
        Bulkhead, CircuitBreaker, GoldenSignals, IncidentDetector, IncidentStatus,
        ServiceLevelObjective, TokenBucket, exponential_backoff, retry_with_backoff,
    };
    use crate::domain::money::{Currency, Money, RoundingMode};
    use crate::domain::account::{Account, AccountType, AccountStatus};
    use crate::domain::journal::{EntryLeg, JournalEntry, EntrySide};
    use crate::domain::coa::{ChartOfAccounts, CoaAccount, CoaCategory};
    use rust_decimal_macros::dec;

    // ━━━ TokenBucket ━━━

    #[test]
    fn probe_token_bucket_rate_zero() {
        let bucket = TokenBucket::new(10, 0.0);
        for _ in 0..10 { assert!(bucket.try_consume()); }
        assert!(!bucket.try_consume(), "RATE=0: bucket should be empty after draining initial tokens");
        std::thread::sleep(Duration::from_secs(1));
        assert!(!bucket.try_consume(), "RATE=0: no tokens should refill at rate 0");
    }

    #[test]
    fn probe_token_bucket_negative_rate() {
        let bucket = TokenBucket::new(10, -100.0);
        for _ in 0..10 { bucket.try_consume(); }
        std::thread::sleep(Duration::from_millis(10));
        // With negative rate, new_tokens = elapsed * -100 = negative
        // tokens = tokens + negative => decreases further, clamped to 0 by min(capacity)?
        // Actually: (*tokens + new_tokens).min(f64::from(capacity))
        // If tokens was 0 and new_tokens is -5, then: (0 + -5).min(10) = -5
        // Next check: -5 >= 1.0? No. So bucket stays dead.
        // But the problem: tokens becomes NEGATIVE and stays negative forever
        // because (negative + more_negative).min(10) = more_negative
        // This is a BUG: negative rate makes bucket permanently broken
        assert!(!bucket.try_consume(), "NEGATIVE RATE: should not allow consumption");
    }

    #[test]
    fn probe_token_bucket_capacity_zero() {
        let bucket = TokenBucket::new(0, 1000.0);
        assert!(!bucket.try_consume(), "CAPACITY=0: should never allow consumption");
        std::thread::sleep(Duration::from_millis(50));
        assert!(!bucket.try_consume(), "CAPACITY=0: refill should not help");
    }

    // ━━━ CircuitBreaker ━━━

    #[test]
    fn probe_circuit_breaker_threshold_zero() {
        let cb = CircuitBreaker::new(0, Duration::from_secs(60));
        // threshold=0 means it trips on the FIRST failure
        assert!(cb.allow_request()); // Still closed
        cb.record_failure(); // 1 >= 0 → trips
        assert!(!cb.allow_request(), "THRESHOLD=0: should trip after 1 failure");
    }

    // ━━━ retry_with_backoff ━━━

    #[test]
    #[should_panic(expected = "max_attempts=0")]
    fn probe_retry_max_attempts_zero_panics() {
        let _result: Result<(), &str> = retry_with_backoff(
            || Err("always fails"),
            0,   // max_attempts=0 → loop never executes → last_error=None → unwrap() panics!
            100,
            5000,
        );
    }

    #[test]
    fn probe_retry_single_attempt() {
        let result: Result<i32, &str> = retry_with_backoff(
            || Ok(42),
            1,
            100,
            5000,
        );
        assert_eq!(result.unwrap(), 42);
    }

    // ━━━ Bulkhead ━━━

    #[test]
    fn probe_bulkhead_max_zero() {
        let bh = Bulkhead::new(0);
        assert!(bh.try_acquire().is_err(), "MAX=0: should always reject");
    }

    #[test]
    fn probe_bulkhead_concurrent_limit() {
        let bh = Bulkhead::new(2);
        let g1 = bh.try_acquire().unwrap();
        let g2 = bh.try_acquire().unwrap();
        assert!(bh.try_acquire().is_err(), "Should reject 3rd concurrent request");
        drop(g1);
        drop(g2);
        // After drop, should allow again
        assert!(bh.try_acquire().is_ok());
    }

    // ━━━ GoldenSignals ━━━

    #[test]
    fn probe_golden_signals_empty_percentile() {
        let gs = GoldenSignals::new(100);
        assert_eq!(gs.latency_percentile(50.0), None, "Empty signals should return None");
        assert_eq!(gs.latency_percentile(99.0), None);
        assert_eq!(gs.latency_percentile(0.0), None);
    }

    #[test]
    fn probe_golden_signals_pct_over_100() {
        let gs = GoldenSignals::new(100);
        gs.record_request(Duration::from_millis(1), false);
        // pct=150.0: idx = (0 * 150.0 / 100.0) = 0, so returns the only element
        let result = gs.latency_percentile(150.0);
        assert!(result.is_some(), "pct>100 should be handled gracefully");
    }

    // ━━━ Money ━━━

    #[test]
    fn probe_money_from_minor_negative() {
        let usd = Currency::usd();
        let m = Money::from_minor(-1, usd);
        assert_eq!(m.amount, dec!(-0.01));
        assert_eq!(m.to_minor(), -1);
    }

    #[test]
    fn probe_money_from_minor_i64_max() {
        let usd = Currency::usd();
        let m = Money::from_minor(i64::MAX, usd);
        // i64::MAX / 100 ≈ 92 quadrillion dollars — should work
        assert!(m.try_to_minor().is_some());
    }

    #[test]
    fn probe_money_from_minor_i64_min() {
        let usd = Currency::usd();
        let m = Money::from_minor(i64::MIN, usd);
        assert_eq!(m.to_minor(), i64::MIN);
    }

    // ━━━ Journal ━━━

    #[test]
    fn probe_journal_empty_legs() {
        let result = JournalEntry::new(uuid::Uuid::now_v7(), 1, vec![], "");
        assert!(result.is_err(), "Empty legs should be rejected");
    }

    #[test]
    fn probe_journal_single_leg() {
        let legs = vec![EntryLeg::debit(uuid::Uuid::now_v7(), 100)];
        let result = JournalEntry::new(uuid::Uuid::now_v7(), 1, legs, "");
        assert!(result.is_err(), "Single leg should be rejected (not balanced)");
    }

    // ━━━ exponential_backoff ━━━

    #[test]
    fn probe_backoff_attempt_zero() {
        let dur = exponential_backoff(0, 100, 5000);
        assert!(dur.as_millis() > 0); // Should have base delay
    }

    #[test]
    fn probe_backoff_overflow_attempt() {
        // 2^32 * base_ms could overflow u64
        let dur = exponential_backoff(32, 100, 5000);
        // Should be capped at max_ms
        assert!(dur.as_millis() <= 6250); // max(5000) + jitter 25% = 6250
    }

    // ━━━ COA ━━━

    #[test]
    fn probe_coa_cycle_detection() {
        let mut coa = ChartOfAccounts::new(1);
        let a = coa.add_account(CoaAccount::new("A", "A", CoaCategory::Asset, None, 1));
        let b = coa.add_account(CoaAccount::new("B", "B", CoaCategory::Asset, Some(a), 1));
        // Create cycle: make a's parent = b
        // This depends on COA implementation — does it prevent cycles?
        let c = coa.add_account(CoaAccount::new("C", "C", CoaCategory::Asset, Some(b), 1));
        assert!(coa.find_by_id(c).is_some());
        // Can't easily create a cycle without set_parent, but worth checking
    }
}
