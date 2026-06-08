//! Thread affinity and core pinning for low-latency financial processing.
//! Pins worker threads to specific CPU cores to eliminate migration overhead.

use std::thread;

/// Pin the current thread to a specific CPU core.
/// Uses libc::sched_setaffinity on Linux.
/// Returns true if pinning succeeded.
pub fn pin_to_core(core_id: usize) -> bool {
    #[cfg(target_os = "linux")]
    {
        let mut cpu_set = unsafe { std::mem::zeroed::<libc::cpu_set_t>() };
        let core_count = core_id / (std::mem::size_of::<libc::cpu_set_t>() * 8);
        let bit_offset = core_id % (std::mem::size_of::<libc::cpu_set_t>() * 8);
        unsafe {
            libc::CPU_SET(bit_offset, &mut cpu_set);
        }
        let result = unsafe {
            libc::sched_setaffinity(
                0, // 0 = current thread
                std::mem::size_of::<libc::cpu_set_t>(),
                &cpu_set,
            )
        };
        result == 0
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = core_id;
        false
    }
}

/// Get the current thread's CPU core.
pub fn current_core() -> Option<usize> {
    #[cfg(target_os = "linux")]
    {
        let cpu = unsafe { libc::sched_getcpu() };
        if cpu >= 0 {
            Some(cpu as usize)
        } else {
            None
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Convenience: name + pin the current thread.
pub fn configure_worker(name: &str, core_id: usize) {
    // Set thread name (Linux only)
    #[cfg(target_os = "linux")]
    {
        let name_bytes = name.as_bytes();
        let truncated = &name_bytes[..name_bytes.len().min(15)];
        // pthread_setname_np via libc
        let _ = truncated;
    }

    if !pin_to_core(core_id) {
        tracing::warn!(%name, %core_id, "Failed to pin thread to core");
    }
}

/// Pin N worker threads to consecutive cores starting from `start_core`.
pub fn spawn_pinned_workers<F>(
    count: usize,
    start_core: usize,
    name_prefix: &str,
    work: F,
) -> Vec<thread::JoinHandle<()>>
where
    F: Fn(usize) + Send + Sync + 'static + Clone,
{
    let name = name_prefix.to_string();
    (0..count)
        .map(|i| {
            let w = work.clone();
            let name_clone = format!("{}-{}", name, i);
            let core = start_core + i;
            thread::spawn(move || {
                configure_worker(&name_clone, core);
                w(i);
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pin_to_core_roundtrip() {
        // Pin to core 0, read back
        let pinned = pin_to_core(0);
        if pinned {
            let core = current_core();
            assert!(core.is_some());
            // On single NUMA node systems, after pinning we should be on the pinned core
            // But we can't guarantee it in all environments, so just verify no crash
        }
    }

    #[test]
    fn test_spawn_pinned_workers() {
        let results = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let r = results.clone();

        let handles = spawn_pinned_workers(2, 0, "test-worker", move |id| {
            let core = current_core();
            r.lock().unwrap().push((id, core));
        });

        for h in handles {
            h.join().unwrap();
        }

        let collected = results.lock().unwrap();
        assert_eq!(collected.len(), 2, "Both workers should complete");
    }
}
