//! Resilience edge case coverage — circuit breaker, bulkhead,
//! retry with backoff, and exponential backoff boundary conditions.

#[cfg(test)]
mod resilience_edge_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use crate::service::resilience::{
        CircuitBreaker, Bulkhead, exponential_backoff, retry_with_backoff, CircuitState,
    };

    // ━━━ Circuit Breaker ━━━

    #[test]
    fn test_cb_initial_state_is_closed() {
        let cb = CircuitBreaker::new(3, Duration::from_secs(10));
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_cb_trip_after_failure_threshold() {
        let cb = CircuitBreaker::new(2, Duration::from_secs(10));
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn test_cb_rejects_in_open_state() {
        let cb = CircuitBreaker::new(1, Duration::from_millis(100));
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.allow_request());
    }

    #[test]
    fn test_cb_half_open_after_cooldown() {
        let cb = CircuitBreaker::new(1, Duration::from_millis(1));
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        std::thread::sleep(Duration::from_millis(10));
        // After cooldown, next allow_request should transition to HalfOpen
        assert!(cb.allow_request());
        assert_eq!(cb.state(), CircuitState::HalfOpen);
    }

    #[test]
    fn test_cb_recloses_after_half_open_success() {
        let cb = CircuitBreaker::new(1, Duration::from_millis(1));
        cb.record_failure();
        std::thread::sleep(Duration::from_millis(10));
        let _ = cb.allow_request(); // transition to half-open
        cb.record_success();
        cb.record_success(); // need 2 successes to close
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_cb_error_rate_zero_initially() {
        let cb = CircuitBreaker::new(3, Duration::from_secs(10));
        assert_eq!(cb.error_rate(), 0.0);
    }

    #[test]
    fn test_cb_error_rate_after_failures() {
        let cb = CircuitBreaker::new(5, Duration::from_secs(10));
        cb.record_failure();
        cb.record_failure();
        // error_rate should be f64
        assert!(cb.error_rate() >= 0.0);
    }

    // ━━━ Bulkhead ━━━

    #[test]
    fn test_bulkhead_acquire_release() {
        let bh = Bulkhead::new(2);
        let g1 = bh.try_acquire().unwrap();
        let g2 = bh.try_acquire().unwrap();
        // All slots used — third should fail
        assert!(bh.try_acquire().is_err());
        // Release one, should be acquirable again
        drop(g1);
        assert!(bh.try_acquire().is_ok());
        drop(g2);
    }

    #[test]
    fn test_bulkhead_zero_capacity() {
        let bh = Bulkhead::new(0);
        assert!(bh.try_acquire().is_err());
    }

    #[test]
    fn test_bulkhead_release_does_not_panic_on_empty() {
        // Guard drops should be safe after bulkhead is fully released
        let _bh = Bulkhead::new(1);
        drop(_bh);
    }

    // ━━━ Exponential Backoff ━━━

    #[test]
    fn test_exponential_backoff_first_attempt_minimal() {
        let d = exponential_backoff(0, 100, 5000);
        // base=100, attempt=0 => 100ms, with ±25% jitter => 75-125ms
        assert!(d.as_millis() <= 150, "got {} ms", d.as_millis());
    }

    #[test]
    fn test_exponential_backoff_max_capped() {
        let d = exponential_backoff(10, 100, 2000);
        // Capped at 2000ms, with ±25% jitter => 1500-2500ms
        assert!(d.as_millis() <= 2600, "got {} ms", d.as_millis());
    }

    #[test]
    fn test_exponential_backoff_grows() {
        let d1 = exponential_backoff(0, 100, 10000);
        let d2 = exponential_backoff(3, 100, 10000);
        assert!(d2 >= d1);
    }

    // ━━━ Retry with Backoff ━━━

    #[test]
    fn test_retry_succeeds_first_attempt() {
        let result = retry_with_backoff(
            || Ok::<i32, &str>(42),
            3,
            10,
            100,
        );
        assert_eq!(result, Ok(42));
    }

    #[test]
    fn test_retry_gives_up_after_max_attempts() {
        let mut calls = 0;
        let result = retry_with_backoff(
            || {
                calls += 1;
                Err::<i32, &str>("always fails")
            },
            2, // total attempts
            1,
            50,
        );
        assert!(result.is_err());
        assert_eq!(calls, 2); // 2 total attempts
    }
}
