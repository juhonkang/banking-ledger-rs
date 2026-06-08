//! Advanced module edge tests — DeadlockDetector cycle detection,
//! LatencyHistogram percentiles, edge cases.

#[cfg(test)]
mod advanced_edge_tests {
    use std::time::Duration;
    use crate::service::advanced::{DeadlockDetector, LatencyHistogram};

    #[test]
    fn test_deadlock_detector_no_cycle_initially() {
        let dd = DeadlockDetector::new();
        assert!(dd.detect_cycle().is_none());
    }

    #[test]
    fn test_deadlock_detector_simple_cycle() {
        let mut dd = DeadlockDetector::new();
        dd.acquire(1, "A");
        dd.acquire(2, "B");
        dd.wait_for(1, "B");
        dd.wait_for(2, "A");
        assert!(dd.detect_cycle().is_some(), "Should detect deadlock cycle");
    }

    #[test]
    fn test_deadlock_detector_release_prevents_cycle() {
        let mut dd = DeadlockDetector::new();
        dd.acquire(1, "A");
        dd.acquire(2, "B");
        dd.release(1, "A");
        dd.wait_for(1, "B");
        dd.wait_for(2, "A");
        assert!(dd.detect_cycle().is_none());
    }

    #[test]
    fn test_deadlock_detector_many_resources() {
        let mut dd = DeadlockDetector::new();
        for i in 0..20 {
            dd.acquire(i, &format!("R{}", i));
        }
        assert!(dd.detect_cycle().is_none());
    }

    #[test]
    fn test_latency_histogram_empty() {
        let mut h = LatencyHistogram::new(100);
        assert!(h.is_empty());
        assert_eq!(h.len(), 0);
        assert!(h.min().is_none());
        assert!(h.max().is_none());
        assert!(h.mean().is_none());
    }

    #[test]
    fn test_latency_histogram_record_and_stats() {
        let mut h = LatencyHistogram::new(100);
        h.record(Duration::from_micros(100));
        h.record(Duration::from_micros(200));
        h.record(Duration::from_micros(300));
        assert_eq!(h.len(), 3);
        assert!(h.max().unwrap() >= Duration::from_micros(300));
    }

    #[test]
    fn test_latency_histogram_percentile() {
        let mut h = LatencyHistogram::new(1000);
        for i in 1..=100 {
            h.record(Duration::from_micros(i * 10));
        }
        let p50 = h.percentile(50.0);
        let p99 = h.percentile(99.0);
        assert!(p50.is_some());
        assert!(p99.is_some());
        assert!(p50.unwrap() <= p99.unwrap());
    }

    #[test]
    fn test_latency_histogram_clear() {
        let mut h = LatencyHistogram::new(100);
        h.record(Duration::from_secs(1));
        h.clear();
        assert!(h.is_empty());
    }

    #[test]
    fn test_latency_histogram_report_nonempty() {
        let mut h = LatencyHistogram::new(100);
        h.record(Duration::from_micros(500));
        let report = h.report();
        assert!(!report.is_empty());
    }

    #[test]
    fn test_latency_histogram_jitter() {
        let mut h = LatencyHistogram::new(100);
        h.record(Duration::from_micros(100));
        h.record(Duration::from_micros(150));
        let _ = h.jitter(); // Should not panic
    }
}
