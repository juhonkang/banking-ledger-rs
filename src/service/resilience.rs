//! Resilience patterns for high-availability financial systems.
//! Circuit breakers, exponential backoff, bulkhead isolation,
//! rate limiting, SLOs, golden signals, chaos engineering.
use dashmap::DashMap;
use std::collections::VecDeque;
use std::fmt::Debug;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ━━━ SLOs ━━━

/// Service Level Objective — defines acceptable performance bounds.
#[derive(Debug, Clone)]
pub struct ServiceLevelObjective {
    pub name: String,
    /// Target latency in milliseconds (e.g., 99th percentile < 5ms)
    pub latency_p99_ms: u64,
    /// Target availability (e.g., 0.99999 = five nines)
    pub availability: f64,
    /// Maximum error rate
    pub error_rate_max: f64,
    /// Measurement window
    pub window: Duration,
}

impl ServiceLevelObjective {
    pub fn financial_default() -> Self {
        Self {
            name: "financial-core".into(),
            latency_p99_ms: 5,                // 5ms p99
            availability: 0.99999,            // Five nines
            error_rate_max: 0.000001,         // 0.0001% errors
            window: Duration::from_secs(300), // 5-min window
        }
    }
}

// ━━━ Circuit Breaker ━━━

/// Circuit breaker states — prevents cascading failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation — requests pass through
    Closed,
    /// Too many failures — requests are rejected immediately
    Open,
    /// Testing if downstream has recovered
    HalfOpen,
}

/// A circuit breaker that trips after `failure_threshold` consecutive failures.
pub struct CircuitBreaker {
    state: Mutex<CircuitState>,
    failure_count: AtomicU32,
    success_count: AtomicU32,
    failure_threshold: u32,
    /// How long to stay OPEN before transitioning to `HALF_OPEN`
    cooldown: Duration,
    last_failure_time: Mutex<Option<Instant>>,
    /// Total calls (for metrics)
    total_calls: AtomicU64,
    total_failures: AtomicU64,
    total_successes: AtomicU64,
}

impl CircuitBreaker {
    pub fn new(failure_threshold: u32, cooldown: Duration) -> Self {
        Self {
            state: Mutex::new(CircuitState::Closed),
            failure_count: AtomicU32::new(0),
            success_count: AtomicU32::new(0),
            failure_threshold,
            cooldown,
            last_failure_time: Mutex::new(None),
            total_calls: AtomicU64::new(0),
            total_failures: AtomicU64::new(0),
            total_successes: AtomicU64::new(0),
        }
    }

    /// Check if a request should be allowed through.
    pub fn allow_request(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        self.total_calls.fetch_add(1, Ordering::Relaxed);

        match *state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                // Check if cooldown has expired
                let should_half_open = self
                    .last_failure_time
                    .lock()
                    .unwrap()
                    .map(|t| t.elapsed() >= self.cooldown)
                    .unwrap_or(false);

                if should_half_open {
                    *state = CircuitState::HalfOpen;
                    self.success_count.store(0, Ordering::SeqCst);
                    true // Allow a probe request
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => {
                // Only allow a limited number of probe requests
                self.success_count.load(Ordering::SeqCst) < 3
            }
        }
    }

    /// Record a successful call.
    pub fn record_success(&self) {
        self.total_successes.fetch_add(1, Ordering::Relaxed);
        self.failure_count.store(0, Ordering::SeqCst);

        let mut state = self.state.lock().unwrap();
        if *state == CircuitState::HalfOpen {
            let count = self.success_count.fetch_add(1, Ordering::SeqCst) + 1;
            if count >= 2 {
                // Reset to closed after enough successful probes
                *state = CircuitState::Closed;
                self.success_count.store(0, Ordering::SeqCst);
            }
        }
    }

    /// Record a failed call.
    pub fn record_failure(&self) {
        self.total_failures.fetch_add(1, Ordering::Relaxed);
        let count = self.failure_count.fetch_add(1, Ordering::SeqCst) + 1;

        *self.last_failure_time.lock().unwrap() = Some(Instant::now());

        if count >= self.failure_threshold {
            let mut state = self.state.lock().unwrap();
            *state = CircuitState::Open;
        }
    }

    /// Current circuit state
    pub fn state(&self) -> CircuitState {
        *self.state.lock().unwrap()
    }

    /// Error rate
    pub fn error_rate(&self) -> f64 {
        let total = self.total_calls.load(Ordering::Relaxed) as f64;
        if total == 0.0 {
            return 0.0;
        }
        self.total_failures.load(Ordering::Relaxed) as f64 / total
    }
}

// ━━━ Retries ━━━

/// Exponential backoff with jitter.
pub fn exponential_backoff(attempt: u32, base_ms: u64, max_ms: u64) -> Duration {
    let exponential = base_ms * 2u64.pow(attempt);
    let capped = exponential.min(max_ms);

    // Add jitter: ±25% of the capped delay
    // Use i64 for signed jitter calculation to avoid UB from f64→u64 on negatives
    let jitter_i64 = (capped as f64 * 0.25 * (rand::random::<f64>() * 2.0 - 1.0)) as i64;
    let jitter_abs = jitter_i64.unsigned_abs();
    if jitter_i64 < 0 {
        Duration::from_millis(capped.saturating_sub(jitter_abs))
    } else {
        Duration::from_millis(capped.saturating_add(jitter_abs))
    }
}

/// Retry a fallible operation with exponential backoff.
pub fn retry_with_backoff<F, T, E>(
    mut operation: F,
    max_attempts: u32,
    base_ms: u64,
    max_ms: u64,
) -> Result<T, E>
where
    F: FnMut() -> Result<T, E>,
{
    let mut last_error = None;

    for attempt in 0..max_attempts {
        match operation() {
            Ok(value) => return Ok(value),
            Err(e) => {
                last_error = Some(e);
                if attempt < max_attempts - 1 {
                    let delay = exponential_backoff(attempt, base_ms, max_ms);
                    std::thread::sleep(delay);
                }
            }
        }
    }

    Err(last_error.expect("bug: retry_with_backoff called with max_attempts=0"))
}

// ━━━ Bulkhead ━━━

/// Bulkhead — limits concurrent operations to prevent resource exhaustion.
pub struct Bulkhead {
    /// Maximum concurrent operations
    max_concurrent: u32,
    /// Current active operations
    active: AtomicU32,
    /// Total rejected (for metrics)
    rejected: AtomicU64,
}

impl Bulkhead {
    pub fn new(max_concurrent: u32) -> Self {
        Self {
            max_concurrent,
            active: AtomicU32::new(0),
            rejected: AtomicU64::new(0),
        }
    }

    /// Try to acquire a bulkhead slot. Returns Ok(guard) or Err.
    pub fn try_acquire(&self) -> Result<BulkheadGuard<'_>, BulkheadError> {
        loop {
            let current = self.active.load(Ordering::Acquire);
            if current >= self.max_concurrent {
                self.rejected.fetch_add(1, Ordering::Relaxed);
                return Err(BulkheadError::Full);
            }
            if self
                .active
                .compare_exchange(current, current + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Ok(BulkheadGuard { bulkhead: self });
            }
        }
    }

    fn release(&self) {
        self.active.fetch_sub(1, Ordering::Release);
    }

    pub fn active_count(&self) -> u32 {
        self.active.load(Ordering::Acquire)
    }
}

pub struct BulkheadGuard<'a> {
    bulkhead: &'a Bulkhead,
}

impl Drop for BulkheadGuard<'_> {
    fn drop(&mut self) {
        self.bulkhead.release();
    }
}

#[derive(Debug)]
pub enum BulkheadError {
    Full,
}

// ━━━ Rate Limiting ━━━

/// Token bucket rate limiter.
pub struct TokenBucket {
    /// Maximum tokens in the bucket
    capacity: u32,
    /// Tokens added per second
    rate: f64,
    /// Current tokens
    tokens: Mutex<f64>,
    /// Last refill time
    last_refill: Mutex<Instant>,
}

impl TokenBucket {
    pub fn new(capacity: u32, rate_per_second: f64) -> Self {
        Self {
            capacity,
            rate: rate_per_second,
            tokens: Mutex::new(f64::from(capacity)),
            last_refill: Mutex::new(Instant::now()),
        }
    }

    /// Try to consume one token. Returns true if allowed.
    pub fn try_consume(&self) -> bool {
        let mut tokens = self.tokens.lock().unwrap();
        let mut last = self.last_refill.lock().unwrap();

        // Refill — rate clamped to 0 to prevent negative token accumulation
        let elapsed = last.elapsed().as_secs_f64();
        let new_tokens = elapsed * self.rate.max(0.0);
        *tokens = (*tokens + new_tokens).max(0.0).min(f64::from(self.capacity));
        *last = Instant::now();

        if *tokens >= 1.0 {
            *tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Try to consume N tokens
    pub fn try_consume_n(&self, n: u32) -> bool {
        for _ in 0..n {
            if !self.try_consume() {
                return false;
            }
        }
        true
    }
}

// ━━━ Golden Signals ━━━

/// Metrics collector for the Four Golden Signals:
/// Latency, Traffic, Errors, Saturation.
#[derive(Debug, Default)]
pub struct GoldenSignals {
    /// Latency samples (last N requests)
    latency_samples: Mutex<VecDeque<Duration>>,
    /// Total requests in current window
    request_count: AtomicU64,
    /// Error count in current window
    error_count: AtomicU64,
    /// Error count by category (`INSUFFICIENT_FUNDS`, TIMEOUT, `SYSTEM_ERROR`, etc.)
    error_categories: DashMap<String, AtomicU64>,
    /// Latency bucket counts: "<10ms", "10-50ms", "50-100ms", ">100ms"
    latency_buckets: DashMap<String, AtomicU64>,
    /// Current saturation (active connections / max)
    saturation: AtomicU64,
    max_samples: usize,
}

impl GoldenSignals {
    pub fn new(max_samples: usize) -> Self {
        let latency_buckets = DashMap::new();
        latency_buckets.insert("<10ms".to_string(), AtomicU64::new(0));
        latency_buckets.insert("10-50ms".to_string(), AtomicU64::new(0));
        latency_buckets.insert("50-100ms".to_string(), AtomicU64::new(0));
        latency_buckets.insert(">100ms".to_string(), AtomicU64::new(0));
        Self {
            latency_samples: Mutex::new(VecDeque::with_capacity(max_samples)),
            request_count: AtomicU64::new(0),
            error_count: AtomicU64::new(0),
            error_categories: DashMap::new(),
            latency_buckets,
            saturation: AtomicU64::new(0),
            max_samples,
        }
    }

    /// Record a request with error category and latency bucketing.
    pub fn record_request(&self, latency: Duration, is_error: bool) {
        self.request_count.fetch_add(1, Ordering::Relaxed);
        if is_error {
            self.error_count.fetch_add(1, Ordering::Relaxed);
        }

        // Latency bucketing
        let bucket = if latency < Duration::from_millis(10) {
            "<10ms"
        } else if latency < Duration::from_millis(50) {
            "10-50ms"
        } else if latency < Duration::from_millis(100) {
            "50-100ms"
        } else {
            ">100ms"
        };
        if let Some(b) = self.latency_buckets.get(bucket) {
            b.fetch_add(1, Ordering::Relaxed);
        }

        let mut samples = self.latency_samples.lock().unwrap();
        if samples.len() >= self.max_samples {
            samples.pop_front();
        }
        samples.push_back(latency);
    }

    /// Record an error with a category for observability.
    pub fn record_error(&self, error_type: &str) {
        self.error_count.fetch_add(1, Ordering::Relaxed);
        let counter = self
            .error_categories
            .entry(error_type.to_string())
            .or_insert_with(|| AtomicU64::new(0));
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// Get error counts by category.
    pub fn errors_by_category(&self) -> Vec<(String, u64)> {
        self.error_categories
            .iter()
            .map(|e| (e.key().clone(), e.value().load(Ordering::Relaxed)))
            .collect()
    }

    /// Get latency bucket distribution.
    pub fn latency_bucket_counts(&self) -> Vec<(String, u64)> {
        self.latency_buckets
            .iter()
            .map(|e| (e.key().clone(), e.value().load(Ordering::Relaxed)))
            .collect()
    }

    /// Reset all counters including categorised errors and latency buckets.
    pub fn reset(&self) {
        self.request_count.store(0, Ordering::SeqCst);
        self.error_count.store(0, Ordering::SeqCst);
        self.latency_samples.lock().unwrap().clear();
        for e in &self.error_categories {
            e.value().store(0, Ordering::SeqCst);
        }
        for b in &self.latency_buckets {
            b.value().store(0, Ordering::SeqCst);
        }
    }

    /// Set current saturation level
    pub fn set_saturation(&self, active: u64, max: u64) {
        if max > 0 {
            let pct = (active * 100) / max;
            self.saturation.store(pct, Ordering::Relaxed);
        }
    }

    /// Get latency percentiles
    pub fn latency_percentile(&self, pct: f64) -> Option<Duration> {
        let samples = self.latency_samples.lock().unwrap();
        if samples.is_empty() {
            return None;
        }

        let mut sorted: Vec<Duration> = samples.iter().copied().collect();
        sorted.sort();

        let idx = ((sorted.len() - 1) as f64 * pct / 100.0) as usize;
        Some(sorted[idx.min(sorted.len() - 1)])
    }

    /// Get error rate
    pub fn error_rate(&self) -> f64 {
        let total = self.request_count.load(Ordering::Relaxed) as f64;
        if total == 0.0 {
            return 0.0;
        }
        self.error_count.load(Ordering::Relaxed) as f64 / total
    }

    /// Current RPS (simple — caller tracks time window externally)
    pub fn total_requests(&self) -> u64 {
        self.request_count.load(Ordering::Relaxed)
    }
}

// ━━━ Incident Response ━━━

/// Simple incident detection based on SLO violations.
pub struct IncidentDetector {
    slo: ServiceLevelObjective,
    signals: Arc<GoldenSignals>,
    circuit_breaker: Arc<CircuitBreaker>,
    /// Consecutive check failures before declaring incident
    consecutive_failures: Mutex<u32>,
    threshold: u32,
}

impl IncidentDetector {
    pub fn new(
        slo: ServiceLevelObjective,
        signals: Arc<GoldenSignals>,
        circuit_breaker: Arc<CircuitBreaker>,
        threshold: u32,
    ) -> Self {
        Self {
            slo,
            signals,
            circuit_breaker,
            consecutive_failures: Mutex::new(0),
            threshold,
        }
    }

    /// Check if we're in an incident state.
    pub fn check(&self) -> IncidentStatus {
        let error_rate = self.signals.error_rate();
        let p99 = self.signals.latency_percentile(99.0);

        let mut healthy = true;

        // Check error rate
        if error_rate > self.slo.error_rate_max {
            healthy = false;
        }

        // Check latency
        if let Some(p99_dur) = p99 {
            if p99_dur.as_millis() as u64 > self.slo.latency_p99_ms {
                healthy = false;
            }
        }

        let mut failures = self.consecutive_failures.lock().unwrap();
        if healthy {
            *failures = 0;
        } else {
            *failures += 1;
        }

        if *failures >= self.threshold {
            IncidentStatus::Incident
        } else if *failures > 0 {
            IncidentStatus::Warning
        } else {
            IncidentStatus::Healthy
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum IncidentStatus {
    Healthy,
    Warning,
    Incident,
}

// ━━━ Chaos Engineering ━━━

/// Fault injection — simulates failures for resilience testing.
pub struct ChaosAgent {
    /// Probability of injecting latency (0.0 - 1.0)
    pub latency_probability: f64,
    /// How much latency to inject
    pub latency_duration: Duration,
    /// Probability of returning an error (0.0 - 1.0)
    pub error_probability: f64,
    /// Probability of a crash (0.0 - 1.0)
    pub crash_probability: f64,
    /// Is chaos active?
    pub active: AtomicBool,
}

impl ChaosAgent {
    pub fn new() -> Self {
        Self {
            latency_probability: 0.0,
            latency_duration: Duration::from_millis(100),
            error_probability: 0.0,
            crash_probability: 0.0,
            active: AtomicBool::new(false),
        }
    }

    /// Enable chaos with given parameters
    pub fn enable(&mut self, latency_pct: f64, latency_ms: u64, error_pct: f64, crash_pct: f64) {
        self.latency_probability = latency_pct;
        self.latency_duration = Duration::from_millis(latency_ms);
        self.error_probability = error_pct;
        self.crash_probability = crash_pct;
        self.active.store(true, Ordering::SeqCst);
    }

    /// Disable all chaos
    pub fn disable(&self) {
        self.active.store(false, Ordering::SeqCst);
    }

    /// Intercept a request — may inject latency, error, or crash.
    pub fn intercept<T>(&self, result: Result<T, String>) -> Result<T, String> {
        if !self.active.load(Ordering::SeqCst) {
            return result;
        }

        // Inject latency
        if rand::random::<f64>() < self.latency_probability {
            std::thread::sleep(self.latency_duration);
        }

        // Inject error
        if rand::random::<f64>() < self.error_probability {
            return Err("CHAOS: injected fault".to_string());
        }

        // Inject crash
        assert!(
            rand::random::<f64>() >= self.crash_probability,
            "CHAOS: simulated crash"
        );

        result
    }
}

impl Default for ChaosAgent {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_breaker_trips() {
        let cb = CircuitBreaker::new(3, Duration::from_secs(60));
        assert!(cb.allow_request());
        cb.record_failure();
        cb.record_failure();
        assert!(cb.allow_request()); // Still closed

        cb.record_failure(); // 3rd failure → trips
        assert!(!cb.allow_request()); // Open
    }

    #[test]
    fn test_circuit_breaker_half_open() {
        let cb = CircuitBreaker::new(2, Duration::from_millis(10));
        cb.record_failure();
        cb.record_failure();
        assert!(!cb.allow_request()); // Open

        std::thread::sleep(Duration::from_millis(20));
        assert!(cb.allow_request()); // Half-open probe
        cb.record_success();
        cb.record_success(); // 2 successes → closed
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_exponential_backoff() {
        // Base 10ms, max 1s, ±25% jitter
        let d0 = exponential_backoff(0, 10, 1000);
        assert!(d0.as_millis() >= 7 && d0.as_millis() <= 12); // 10 ± 25%

        let d3 = exponential_backoff(3, 10, 1000);
        assert!(d3.as_millis() >= 60 && d3.as_millis() <= 100); // 80 ± 25%

        let d10 = exponential_backoff(10, 10, 1000);
        assert!(d10.as_millis() <= 1250); // capped at max(1000) + 25%
    }

    #[test]
    fn test_bulkhead_limits_concurrency() {
        let bh = Bulkhead::new(2);
        let g1 = bh.try_acquire().unwrap();
        let _g2 = bh.try_acquire().unwrap();
        assert!(bh.try_acquire().is_err()); // Full

        drop(g1);
        assert!(bh.try_acquire().is_ok()); // Slot freed
    }

    #[test]
    fn test_token_bucket() {
        let bucket = TokenBucket::new(10, 100.0); // 100 tokens/sec
        assert!(bucket.try_consume()); // Has tokens
        assert!(bucket.try_consume_n(9)); // Consume remaining

        // Bucket should be empty now
        assert!(!bucket.try_consume());
    }

    #[test]
    fn test_golden_signals() {
        let signals = GoldenSignals::new(100);
        signals.record_request(Duration::from_millis(1), false);
        signals.record_request(Duration::from_millis(5), false);
        signals.record_request(Duration::from_millis(10), true);

        assert_eq!(signals.total_requests(), 3);
        let err_rate = signals.error_rate();
        assert!((err_rate - 0.333).abs() < 0.01);

        let p50 = signals.latency_percentile(50.0).unwrap();
        assert_eq!(p50.as_millis(), 5); // median of [1,5,10]
    }

    #[test]
    fn test_chaos_agent_intercept() {
        let mut agent = ChaosAgent::new();
        agent.enable(0.0, 0, 1.0, 0.0); // 100% error
        let result = agent.intercept::<i32>(Ok(42));
        assert!(result.is_err());
        agent.disable();
        let result = agent.intercept(Ok(42));
        assert_eq!(result, Ok(42));
    }
}
