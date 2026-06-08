//! Affinity module edge tests — CPU pinning, core detection, pinned workers.

#[cfg(test)]
mod affinity_edge_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use crate::service::affinity::{pin_to_core, current_core, spawn_pinned_workers};

    #[test]
    fn test_current_core_returns_some() {
        let core = current_core();
        assert!(core.is_some(), "Should detect current CPU core");
    }

    #[test]
    fn test_pin_to_core_zero() {
        let _ = pin_to_core(0);
    }

    #[test]
    fn test_pin_to_core_invalid() {
        assert!(!pin_to_core(99999), "Invalid core should return false");
    }

    #[test]
    fn test_spawn_pinned_workers_runs_all() {
        let counter = std::sync::Arc::new(AtomicUsize::new(0));
        let c = counter.clone();
        let handles = spawn_pinned_workers(2, 0, "test-wkr", move |_id| {
            c.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(handles.len(), 2);
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn test_spawn_pinned_workers_single() {
        let counter = std::sync::Arc::new(AtomicUsize::new(0));
        let c = counter.clone();
        let mut handles = spawn_pinned_workers(1, 0, "single", move |_id| {
            c.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(handles.len(), 1);
        handles.pop().unwrap().join().unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_spawn_pinned_workers_zero() {
        let handles = spawn_pinned_workers(0, 0, "zero", |_id| {});
        assert!(handles.is_empty());
    }

    #[test]
    fn test_pin_to_current_core() {
        if let Some(core) = current_core() {
            let _ = pin_to_core(core);
        }
    }

    #[test]
    fn test_multiple_pins_same_thread() {
        let _ = pin_to_core(0);
        let _ = pin_to_core(0);
    }
}
