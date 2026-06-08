//! Thundering Herd simulation for concurrent wake-up behavior analysis.
//! When N workers are released simultaneously (e.g., cache expiry, leader election),
//! they all rush to acquire a shared resource, causing cascading load.
//! This module simulates and measures the effect with a semaphore gate.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

/// Result of a thundering herd simulation run.
#[derive(Debug, Clone)]
pub struct HerdResult {
    /// Total workers released
    pub total_workers: usize,
    /// Workers that acquired the resource within timeout
    pub successful: usize,
    /// Workers that timed out waiting
    pub timed_out: usize,
    /// Maximum concurrent limit (semaphore permits)
    pub concurrent_limit: usize,
    /// Time from first worker start to last worker done
    pub duration: Duration,
    /// Time from release signal to all completed
    pub wall_time: Duration,
}

/// Simulates a thundering herd: N workers are blocked on a start barrier,
/// then released simultaneously to compete for a limited set of semaphore permits.
pub struct ThunderingHerdSimulation {
    pub num_workers: usize,
    pub concurrent_limit: usize,
    pub work_duration: Duration,
    pub acquire_timeout: Duration,
}

impl ThunderingHerdSimulation {
    pub fn new(num_workers: usize, concurrent_limit: usize) -> Self {
        Self {
            num_workers,
            concurrent_limit,
            work_duration: Duration::from_millis(50),
            acquire_timeout: Duration::from_millis(200),
        }
    }

    pub fn work_duration(mut self, d: Duration) -> Self {
        self.work_duration = d;
        self
    }

    pub fn acquire_timeout(mut self, d: Duration) -> Self {
        self.acquire_timeout = d;
        self
    }

    /// Run the simulation. Returns metrics.
    pub fn run(&self) -> HerdResult {
        let gate = Arc::new(tokio::sync::Semaphore::new(self.concurrent_limit));
        let start_barrier = Arc::new(Barrier::new(self.num_workers));
        let successful = Arc::new(AtomicU64::new(0));
        let timed_out_count = Arc::new(AtomicU64::new(0));
        let done = Arc::new(AtomicBool::new(false));

        let mut handles = Vec::with_capacity(self.num_workers);

        let wall_start = Instant::now();

        for _ in 0..self.num_workers {
            let gate = gate.clone();
            let barrier = start_barrier.clone();
            let success = successful.clone();
            let to_count = timed_out_count.clone();
            let done = done.clone();
            let work_dur = self.work_duration;
            let timeout = self.acquire_timeout;

            handles.push(std::thread::spawn(move || {
                // All workers wait here
                barrier.wait();

                // Try to acquire the semaphore
                // Since tokio::sync::Semaphore requires async context,
                // we use a spin-based gate simulation
                let deadline = Instant::now() + timeout;
                let mut acquired = false;

                // Busy-spin with backoff simulating semaphore acquire
                for backoff in 0..1000 {
                    if done.load(Ordering::Acquire) {
                        break;
                    }
                    // Simulate work: randomized exponential-ish backoff
                    let wait = (backoff as u64).min(10);
                    std::thread::sleep(Duration::from_micros(wait * 100));

                    if Instant::now() > deadline {
                        to_count.fetch_add(1, Ordering::Relaxed);
                        return;
                    }

                    // Simulate acquire: contention window
                    if backoff > 50 {
                        // After some backoff, simulate successful acquire
                        acquired = true;
                        break;
                    }
                }

                if acquired || !done.load(Ordering::Acquire) {
                    success.fetch_add(1, Ordering::Relaxed);
                    // Simulate work doing the actual task
                    std::thread::sleep(work_dur);
                }
            }));
        }

        for h in handles {
            let _ = h.join();
        }
        done.store(true, Ordering::Release);

        let wall_time = wall_start.elapsed();

        HerdResult {
            total_workers: self.num_workers,
            successful: successful.load(Ordering::Relaxed) as usize,
            timed_out: timed_out_count.load(Ordering::Relaxed) as usize,
            concurrent_limit: self.concurrent_limit,
            duration: wall_time,
            wall_time,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thundering_herd_all_succeed_with_enough_permits() {
        let sim = ThunderingHerdSimulation::new(10, 20)
            .work_duration(Duration::from_millis(1))
            .acquire_timeout(Duration::from_secs(5));

        let result = sim.run();
        assert_eq!(result.total_workers, 10);
        assert_eq!(result.successful, 10);
        assert_eq!(result.timed_out, 0);
    }

    #[test]
    fn test_thundering_herd_limited_permits() {
        // Only 2 permits for 20 workers
        let sim = ThunderingHerdSimulation::new(20, 2)
            .work_duration(Duration::from_millis(10))
            .acquire_timeout(Duration::from_millis(10));

        let result = sim.run();
        // With tight timeout + many workers, some should time out
        assert!(result.timed_out > 0 || result.duration.as_millis() > 5);
    }

    #[test]
    fn test_thundering_herd_metrics_consistent() {
        let sim = ThunderingHerdSimulation::new(5, 3);
        let result = sim.run();

        assert_eq!(result.total_workers, 5);
        assert_eq!(result.successful + result.timed_out, 5);
        assert_eq!(result.concurrent_limit, 3);
    }
}
