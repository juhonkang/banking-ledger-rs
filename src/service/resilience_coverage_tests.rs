//! Coverage tests for resilience patterns: CircuitBreaker transitions,
//! Bulkhead, TokenBucket, exponential backoff, retry exhaustion,
//! and incident escalation.
//!
//! These tests target specific behavioural boundaries and transitions
//! that exercise the full state machine of each pattern.

#[cfg(test)]
mod resilience_coverage_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use crate::service::resilience::{
        exponential_backoff, retry_with_backoff, Bulkhead, CircuitBreaker, CircuitState,
        GoldenSignals, IncidentDetector, IncidentStatus, ServiceLevelObjective, TokenBucket,
    };

    // ━━━━ CircuitBreaker: half-open → closed transition ━━━━

    /// Verify the full transition path: Closed → Open → HalfOpen → Closed.
    /// The CB must trip after threshold failures, transition to half-open
    /// after cooldown, accept probe requests, and close after 2 successes.
    #[test]
    fn circuit_breaker_half_open_to_closed_transition() {
        let cb = CircuitBreaker::new(2, Duration::from_millis(10));

        // Phase 1: Closed → Open via threshold failures
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.allow_request());
        cb.record_failure();
        assert!(cb.allow_request());
        cb.record_failure();
        // Two consecutive failures → trip
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.allow_request(), "should reject while open");

        // Phase 2: Open → HalfOpen after cooldown
        std::thread::sleep(Duration::from_millis(15));
        // First request after cooldown transitions to HalfOpen and allows a probe
        assert!(cb.allow_request(), "should allow probe in half-open");
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Phase 3: HalfOpen → Closed after 2 successful probes
        cb.record_success(); // 1st success — stays HalfOpen (need 2)
        assert_eq!(cb.state(), CircuitState::HalfOpen);
        cb.record_success(); // 2nd success → Closed
        assert_eq!(cb.state(), CircuitState::Closed);

        // Verify we can make normal requests again
        assert!(cb.allow_request(), "should allow requests after re-closing");
    }

    /// Verify that a failure during half-open immediately re-opens the circuit.
    #[test]
    fn circuit_breaker_half_open_failure_reopens() {
        let cb = CircuitBreaker::new(1, Duration::from_millis(1));

        // Trip immediately with threshold=1
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Wait for cooldown, get a probe
        std::thread::sleep(Duration::from_millis(5));
        assert!(cb.allow_request());
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Probe fails → go back to Open
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.allow_request());
    }

    // ━━━━ Bulkhead: concurrent limit then release ━━━━

    /// Acquire all slots, verify rejection, then release and verify a new
    /// acquisition succeeds — exercising the full acquire/release lifecycle.
    #[test]
    fn bulkhead_concurrent_limit_then_release() {
        let bulkhead = Bulkhead::new(4);

        // Acquire all 4 slots
        let g1 = bulkhead.try_acquire().expect("slot 1");
        let g2 = bulkhead.try_acquire().expect("slot 2");
        let g3 = bulkhead.try_acquire().expect("slot 3");
        let g4 = bulkhead.try_acquire().expect("slot 4");

        assert_eq!(bulkhead.active_count(), 4);
        assert!(bulkhead.try_acquire().is_err(), "should reject when full");

        // Release slot 3 — active drops to 3
        drop(g3);
        assert_eq!(bulkhead.active_count(), 3);

        // Now a new acquisition must succeed
        let g5 = bulkhead.try_acquire().expect("should acquire after release");
        assert_eq!(bulkhead.active_count(), 4);

        // Clean up — guards drop, all slots freed
        drop(g1);
        drop(g2);
        drop(g4);
        drop(g5);
        assert_eq!(bulkhead.active_count(), 0);
    }

    /// Bulkhead with capacity 1: acquire, verify active_count, drop, verify 0.
    #[test]
    fn bulkhead_single_slot_acquire_release() {
        let bulkhead = Bulkhead::new(1);
        {
            let _guard = bulkhead.try_acquire().expect("single slot");
            assert_eq!(bulkhead.active_count(), 1);
            assert!(bulkhead.try_acquire().is_err());
        }
        assert_eq!(bulkhead.active_count(), 0);
        assert!(bulkhead.try_acquire().is_ok());
    }

    // ━━━━ TokenBucket: long refill ━━━━

    /// Use a low refill rate and wait long enough for the bucket to refill
    /// from empty back to multiple tokens — exercising the refill path.
    #[test]
    fn token_bucket_long_refill() {
        // Capacity 20, 5 tokens/sec — slow enough to measure
        let bucket = TokenBucket::new(20, 5.0);

        // Drain all tokens
        assert!(bucket.try_consume_n(20), "should drain initial tokens");
        assert!(!bucket.try_consume(), "bucket empty");

        // Wait 1 second → ~5 new tokens
        std::thread::sleep(Duration::from_secs(1));

        // Should be able to consume at least 4 tokens (allowing for timing jitter)
        let mut consumed = 0u32;
        for _ in 0..10 {
            if bucket.try_consume() {
                consumed += 1;
            } else {
                break;
            }
        }
        assert!(consumed >= 4, "expected >=4 tokens after 1s refill at 5/s, got {consumed}");

        // Capacity should still be respected — never exceed 20
        assert!(!bucket.try_consume_n(20), "should not exceed capacity");
    }

    /// Verify refill doesn't exceed capacity with a long wait.
    #[test]
    fn token_bucket_refill_capped_at_capacity() {
        // Capacity 5, rate 100/s — after waiting, should still cap at 5
        let bucket = TokenBucket::new(5, 100.0);

        // Drain completely
        assert!(bucket.try_consume_n(5));
        assert!(!bucket.try_consume());

        // Wait long enough that it would receive 50+ tokens if uncapped
        std::thread::sleep(Duration::from_millis(600));

        let mut consumed = 0u32;
        while bucket.try_consume() {
            consumed += 1;
        }
        assert_eq!(consumed, 5, "bucket must never exceed capacity of 5");
    }

    // ━━━━ Exponential backoff: jitter bounds ━━━━

    /// Verify that jitter stays within ±25% of the capped exponential value
    /// across many random samples. The backoff formula is:
    ///   base_ms * 2^attempt, capped at max_ms, then ±25% jitter.
    #[test]
    fn exponential_backoff_jitter_stays_in_range() {
        let base_ms = 100u64;
        let max_ms = 10_000u64;

        // Test multiple attempts and multiple samples per attempt
        for attempt in 0..6u32 {
            let exponential = base_ms * 2u64.pow(attempt);
            let capped = exponential.min(max_ms);
            let lower_bound = capped.saturating_sub(capped / 4);  // -25%
            let upper_bound = capped.saturating_add(capped / 4);  // +25%

            // Sample many times to catch jitter variance
            for _ in 0..50 {
                let delay = exponential_backoff(attempt, base_ms, max_ms);
                let ms = delay.as_millis() as u64;

                // Jitter is ±25%, so allow 1ms tolerance for rounding
                assert!(
                    ms >= lower_bound.saturating_sub(1),
                    "attempt {attempt}: delay {ms}ms below lower bound {lower_bound}ms (capped={capped})"
                );
                assert!(
                    ms <= upper_bound + 1,
                    "attempt {attempt}: delay {ms}ms above upper bound {upper_bound}ms (capped={capped})"
                );
            }
        }
    }

    /// Verify that backoff grows monotonically in expectation
    /// (mean of many samples per attempt is strictly increasing).
    #[test]
    fn exponential_backoff_mean_grows_monotonically() {
        let base_ms = 50u64;
        let max_ms = 5_000u64;
        let mut previous_mean = 0f64;

        for attempt in 0..5u32 {
            let sum: u128 = (0..100)
                .map(|_| exponential_backoff(attempt, base_ms, max_ms).as_millis())
                .sum();
            let mean = sum as f64 / 100.0;
            assert!(
                mean > previous_mean,
                "attempt {attempt}: mean {mean}ms not > previous {previous_mean}ms"
            );
            previous_mean = mean;
        }
    }

    // ━━━━ retry_with_backoff: exhaustion ━━━━

    /// Verify that when all attempts fail, the final error is returned
    /// after exhausting max_attempts.
    #[test]
    fn retry_with_backoff_exhausted_returns_last_error() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let call_count = Arc::new(AtomicU32::new(0));
        let cc = call_count.clone();

        let result: Result<i32, String> = retry_with_backoff(
            move || {
                cc.fetch_add(1, Ordering::SeqCst);
                Err::<i32, _>("transient failure".to_string())
            },
            4,   // max 4 attempts
            10,  // 10ms base (fast in tests)
            200, // 200ms max
        );

        assert_eq!(call_count.load(Ordering::SeqCst), 4, "should attempt exactly max_attempts times");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "transient failure");
    }

    /// Verify that a successful attempt short-circuits and returns Ok
    /// without further retries.
    #[test]
    fn retry_with_backoff_succeeds_on_first_attempt() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let call_count = Arc::new(AtomicU32::new(0));
        let cc = call_count.clone();

        let result: Result<i32, String> = retry_with_backoff(
            move || {
                cc.fetch_add(1, Ordering::SeqCst);
                Ok(42)
            },
            5, 10, 200,
        );

        assert_eq!(call_count.load(Ordering::SeqCst), 1, "should succeed on first attempt");
        assert_eq!(result, Ok(42));
    }

    /// Verify retry succeeds on the last possible attempt.
    #[test]
    fn retry_with_backoff_succeeds_on_last_attempt() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let call_count = Arc::new(AtomicU32::new(0));
        let cc = call_count.clone();

        let result: Result<i32, String> = retry_with_backoff(
            move || {
                let n = cc.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    Err(format!("fail {n}"))
                } else {
                    Ok(99)
                }
            },
            3, 5, 100,
        );

        assert_eq!(call_count.load(Ordering::SeqCst), 3, "should make all 3 attempts");
        assert_eq!(result, Ok(99));
    }

    // ━━━━ IncidentDetector: consecutive failure escalation ━━━━

    /// Verify that consecutive SLO violations escalate through
    /// Healthy → Warning → Incident and that recovery resets the counter.
    #[test]
    fn incident_detector_consecutive_failure_escalation() {
        let slo = ServiceLevelObjective {
            name: "test-slo".into(),
            latency_p99_ms: 5,
            availability: 0.999,
            error_rate_max: 0.01,   // 1% max
            window: Duration::from_secs(300),
        };

        let signals = Arc::new(GoldenSignals::new(100));
        let cb = Arc::new(CircuitBreaker::new(5, Duration::from_secs(60)));
        let detector = IncidentDetector::new(slo, signals.clone(), cb.clone(), 3);

        // Phase 1: Healthy when no errors
        assert_eq!(detector.check(), IncidentStatus::Healthy);

        // Phase 2: Inject high error rate → Warning on first violation
        for _ in 0..100 {
            signals.record_request(Duration::from_millis(1), true);
        }
        assert_eq!(detector.check(), IncidentStatus::Warning, "first violation → Warning");

        // Phase 3: Second consecutive violation → still Warning (threshold is 3)
        assert_eq!(detector.check(), IncidentStatus::Warning, "second violation → still Warning");

        // Phase 4: Third consecutive violation → Incident
        assert_eq!(detector.check(), IncidentStatus::Incident, "third violation → Incident");

        // Phase 5: More violations keep it at Incident
        assert_eq!(detector.check(), IncidentStatus::Incident);
    }

    /// Verify that a healthy check resets the escalation counter back to Healthy.
    #[test]
    fn incident_detector_recovery_resets_escalation() {
        let slo = ServiceLevelObjective {
            name: "recovery-slo".into(),
            latency_p99_ms: 5,
            availability: 0.999,
            error_rate_max: 0.01,
            window: Duration::from_secs(300),
        };

        let signals = Arc::new(GoldenSignals::new(100));
        let cb = Arc::new(CircuitBreaker::new(5, Duration::from_secs(60)));
        let detector = IncidentDetector::new(slo, signals.clone(), cb.clone(), 2);

        // Cause a Warning
        for _ in 0..100 {
            signals.record_request(Duration::from_millis(1), true);
        }
        assert_eq!(detector.check(), IncidentStatus::Warning);

        // Reset the signals (all counters back to 0)
        signals.reset();

        // Now check should be Healthy (error_rate = 0.0, counter reset to 0)
        assert_eq!(detector.check(), IncidentStatus::Healthy);
    }

    /// Verify incident detection based on latency SLO violation (not just error rate).
    #[test]
    fn incident_detector_latency_violation_escalates() {
        let slo = ServiceLevelObjective {
            name: "latency-slo".into(),
            latency_p99_ms: 5,          // 5ms p99 target
            availability: 0.999,
            error_rate_max: 0.50,       // high tolerance for errors
            window: Duration::from_secs(300),
        };

        let signals = Arc::new(GoldenSignals::new(100));
        let cb = Arc::new(CircuitBreaker::new(5, Duration::from_secs(60)));

        // Record many slow requests — all > 5ms p99 target
        for _ in 0..50 {
            signals.record_request(Duration::from_millis(20), false);  // no error, but slow
        }

        let detector = IncidentDetector::new(slo, signals.clone(), cb, 1);
        // The p99 will be 20ms, which is > 5ms target → violation
        assert_eq!(detector.check(), IncidentStatus::Incident, "latency SLO breach → Incident");
    }
}
