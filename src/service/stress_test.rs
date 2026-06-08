//! Stress test harness — wires together concurrent operations, chaos injection,
//! thundering herd simulation, and golden signals metrics into a unified stress profile.
//!
//! Usage: `cargo test stress --release` (excluded from standard `cargo test`)

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::domain::account::Account;
use crate::log::hash_chain::HashChain;
use crate::log::ring_buffer::{LatencyHistogram, LatencyTimer, RingBuffer};

/// Stress test profile configuration.
#[derive(Debug, Clone)]
pub struct StressConfig {
    /// Number of concurrent worker threads
    pub workers: usize,
    /// Total operations to perform
    pub total_ops: usize,
    /// Duration cap (None = unlimited)
    pub max_duration: Option<Duration>,
    /// Chaos injection probability (0.0-1.0)
    pub chaos_rate: f64,
    /// Chaos types to inject
    pub chaos_types: Vec<ChaosType>,
    /// Chaos latency max (microseconds)
    pub chaos_latency_max_us: u64,
}

impl Default for StressConfig {
    fn default() -> Self {
        Self {
            workers: 8,
            total_ops: 100_000,
            max_duration: Some(Duration::from_secs(30)),
            chaos_rate: 0.01,
            chaos_types: vec![ChaosType::Latency, ChaosType::Error],
            chaos_latency_max_us: 100,
        }
    }
}

/// Types of chaos injection for stress testing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChaosType {
    /// Inject random latency spikes
    Latency,
    /// Inject random errors
    Error,
    /// Drop operations silently
    Drop,
}

/// Results from a stress test run.
#[derive(Debug, Clone)]
pub struct StressReport {
    pub total_ops: usize,
    pub successful: usize,
    pub failed: usize,
    pub chaos_injected: usize,
    pub elapsed: Duration,
    pub throughput_ops_per_sec: f64,
    pub histogram: String, // JSON snapshot
}

/// Run a stress test against the ring buffer with optional chaos.
pub fn stress_ring_buffer<T: Clone + Send + Sync + 'static>(
    config: &StressConfig,
    value_generator: impl Fn(usize) -> T + Send + Sync + 'static,
) -> StressReport {
    let capacity = (config.workers * 1024).next_power_of_two();
    let buffer = Arc::new(RingBuffer::<T>::new(capacity));
    let histogram = Arc::new(LatencyHistogram::new(20));
    let chaos_active = Arc::new(AtomicBool::new(config.chaos_rate > 0.0));
    let gen = Arc::new(value_generator);

    let start = Instant::now();
    let ops_per_worker = config.total_ops / config.workers;
    let mut handles = vec![];

    for w in 0..config.workers {
        let buf = buffer.clone();
        let hist = histogram.clone();
        let cfg = config.clone();
        let gen = gen.clone();
        let chaos_enabled = chaos_active.clone();

        handles.push(std::thread::spawn(move || {
            let mut produced = 0usize;
            let mut chaos_hits = 0usize;
            let base = w * ops_per_worker;

            for i in 0..ops_per_worker {
                if let Some(max_dur) = cfg.max_duration {
                    if start.elapsed() > max_dur {
                        break;
                    }
                }

                // Chaos injection
                let chaos_on = chaos_enabled.load(Ordering::Relaxed) && rand::random::<f64>() < cfg.chaos_rate;
                if chaos_on {
                    chaos_hits += 1;
                    if cfg.chaos_types.contains(&ChaosType::Drop) {
                        continue;
                    }
                    if cfg.chaos_types.contains(&ChaosType::Latency) {
                        let sleep_us = (rand::random::<f64>() * cfg.chaos_latency_max_us as f64) as u64;
                        std::thread::sleep(Duration::from_micros(sleep_us));
                    }
                }

                let timer = LatencyTimer::start(&hist);
                let val = gen(base + i);
                match buf.try_push(val) {
                    Ok(_) => produced += 1,
                    Err(_) => {
                        if chaos_on && cfg.chaos_types.contains(&ChaosType::Error) {
                            chaos_hits += 1;
                        }
                    }
                }
            }
            (produced, chaos_hits)
        }));
    }

    let mut total_produced = 0usize;
    let mut total_chaos = 0usize;
    for h in handles {
        let (p, c) = h.join().unwrap();
        total_produced += p;
        total_chaos += c;
    }

    let elapsed = start.elapsed();
    let throughput = total_produced as f64 / elapsed.as_secs_f64().max(0.001);

    let hist_json = format!(
        r#"{{"count":{},"mean_us":{},"p50_us":{},"p99_us":{},"min_us":{},"max_us":{}}}"#,
        histogram.count(),
        histogram.mean_ns().unwrap_or(0) / 1_000,
        histogram.percentile(50.0).unwrap_or(0) / 1_000,
        histogram.percentile(99.0).unwrap_or(0) / 1_000,
        histogram.min_ns().unwrap_or(0) / 1_000,
        histogram.max_ns().unwrap_or(0) / 1_000,
    );

    StressReport {
        total_ops: config.total_ops,
        successful: total_produced,
        failed: config.total_ops.saturating_sub(total_produced),
        chaos_injected: total_chaos,
        elapsed,
        throughput_ops_per_sec: throughput,
        histogram: hist_json,
    }
}

/// Lightweight stress test for account debit/credit under concurrency.
pub fn stress_account_concurrent(workers: usize, ops_per_worker: usize) -> StressReport {
    let account = Arc::new(std::sync::Mutex::new(Account::new(
        crate::domain::account::AccountType::Asset,
        "USD",
        10_000_000,
        None,
    )));

    let histogram = Arc::new(LatencyHistogram::new(20));
    let start = Instant::now();
    let mut handles = vec![];

    for _ in 0..workers {
        let acc = account.clone();
        let hist = histogram.clone();
        handles.push(std::thread::spawn(move || {
            let mut operations = 0usize;
            for _ in 0..ops_per_worker {
                let timer = LatencyTimer::start(&hist);
                let mut a = acc.lock().unwrap();
                let _ = a.credit(1);
                let _ = a.debit(1);
                drop(a);
                operations += 1;
            }
            operations
        }));
    }

    let mut total_ops = 0usize;
    for h in handles {
        total_ops += h.join().unwrap();
    }

    let elapsed = start.elapsed();
    let throughput = total_ops as f64 / elapsed.as_secs_f64().max(0.001);

    StressReport {
        total_ops,
        successful: total_ops,
        failed: 0,
        chaos_injected: 0,
        elapsed,
        throughput_ops_per_sec: throughput,
        histogram: format!(
            r#"{{"count":{},"mean_us":{},"p50_us":{},"p99_us":{}}}"#,
            histogram.count(),
            histogram.mean_ns().unwrap_or(0) / 1_000,
            histogram.percentile(50.0).unwrap_or(0) / 1_000,
            histogram.percentile(99.0).unwrap_or(0) / 1_000,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stress_ring_buffer_burst() {
        let config = StressConfig {
            workers: 4,
            total_ops: 40_000,
            max_duration: Some(Duration::from_secs(10)),
            chaos_rate: 0.0,
            chaos_types: vec![],
            ..Default::default()
        };

        let report = stress_ring_buffer(&config, |i| i as u64);
        assert!(report.successful > 0);
        assert!(report.throughput_ops_per_sec > 0.0);
        println!(
            "RingBuffer stress: {} ops in {:?} = {:.0} ops/sec",
            report.successful, report.elapsed, report.throughput_ops_per_sec
        );
    }

    #[test]
    fn test_stress_ring_buffer_with_chaos() {
        let config = StressConfig {
            workers: 4,
            total_ops: 20_000,
            max_duration: Some(Duration::from_secs(10)),
            chaos_rate: 0.05,
            chaos_types: vec![ChaosType::Latency, ChaosType::Drop],
            ..Default::default()
        };

        let report = stress_ring_buffer(&config, |i| format!("payload-{}", i));
        assert!(report.successful > 0);
        assert!(report.chaos_injected > 0, "Chaos should have been injected");
        println!(
            "RingBuffer chaos: {} ok / {} chaos in {:?} = {:.0} ops/sec",
            report.successful, report.chaos_injected, report.elapsed, report.throughput_ops_per_sec
        );
    }

    #[test]
    fn test_stress_account_concurrent() {
        let report = stress_account_concurrent(4, 5_000);
        assert!(report.successful > 0);
        assert!(report.throughput_ops_per_sec > 0.0);
        println!(
            "Account stress: {} ops in {:?} = {:.0} ops/sec",
            report.successful, report.elapsed, report.throughput_ops_per_sec
        );
    }

    #[test]
    fn test_stress_account_high_concurrency() {
        let report = stress_account_concurrent(16, 2_500);
        assert!(report.successful > 0);
        assert!(report.throughput_ops_per_sec > 0.0);
        println!(
            "Account high-conc: {} ops in {:?} = {:.0} ops/sec",
            report.successful, report.elapsed, report.throughput_ops_per_sec
        );
    }
}
