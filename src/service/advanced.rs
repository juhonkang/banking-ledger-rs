//! Advanced concurrency — deadlock detection, latency histograms,
//! and choreography-based event bus for decentralized sagas.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

// ━━━ Deadlock Detection ━━━

/// A deadlock detector using resource allocation graph.
/// Detects cycles in wait-for graph before they cause deadlocks.
#[derive(Debug, Clone)]
pub struct DeadlockDetector {
    /// Resource allocation: `thread_id` → held resources
    held: HashMap<u64, Vec<String>>,
    /// Wait-for: `thread_id` → resources it's waiting for
    waiting: HashMap<u64, Vec<String>>,
    /// Resource ownership: resource → thread_id holding it
    pub(crate) owners: HashMap<String, u64>,
}

impl DeadlockDetector {
    pub fn new() -> Self {
        Self {
            held: HashMap::new(),
            waiting: HashMap::new(),
            owners: HashMap::new(),
        }
    }

    /// Register that a thread holds a resource
    pub fn acquire(&mut self, thread_id: u64, resource: &str) {
        self.held
            .entry(thread_id)
            .or_default()
            .push(resource.to_string());
        self.owners.insert(resource.to_string(), thread_id);
    }

    /// Register that a thread is waiting for a resource
    pub fn wait_for(&mut self, thread_id: u64, resource: &str) {
        self.waiting
            .entry(thread_id)
            .or_default()
            .push(resource.to_string());
    }

    /// Release a resource held by a thread
    pub fn release(&mut self, thread_id: u64, resource: &str) {
        if let Some(resources) = self.held.get_mut(&thread_id) {
            resources.retain(|r| r != resource);
        }
        self.owners.remove(resource);
        if let Some(waiting) = self.waiting.get_mut(&thread_id) {
            waiting.retain(|r| r != resource);
        }
    }

    /// Check if there's a potential deadlock cycle.
    /// Returns the cycle of thread IDs if deadlock is detected.
    pub fn detect_cycle(&self) -> Option<Vec<u64>> {
        // Build adjacency: thread A → thread B if A waits for resource held by B
        let mut graph: HashMap<u64, HashSet<u64>> = HashMap::new();

        for (&waiter, resources) in &self.waiting {
            for res in resources {
                if let Some(&holder) = self.owners.get(res) {
                    graph.entry(waiter).or_default().insert(holder);
                }
            }
        }

        // DFS cycle detection
        let mut visited = HashSet::new();
        let mut in_stack = HashSet::new();
        let mut path = Vec::new();

        for &node in graph.keys() {
            if !visited.contains(&node) {
                if let Some(cycle) =
                    self.dfs_cycle(node, &graph, &mut visited, &mut in_stack, &mut path)
                {
                    return Some(cycle);
                }
            }
        }
        None
    }

    fn dfs_cycle(
        &self,
        node: u64,
        graph: &HashMap<u64, HashSet<u64>>,
        visited: &mut HashSet<u64>,
        in_stack: &mut HashSet<u64>,
        path: &mut Vec<u64>,
    ) -> Option<Vec<u64>> {
        visited.insert(node);
        in_stack.insert(node);
        path.push(node);

        if let Some(neighbors) = graph.get(&node) {
            for &neighbor in neighbors {
                if !visited.contains(&neighbor) {
                    if let Some(cycle) = self.dfs_cycle(neighbor, graph, visited, in_stack, path) {
                        return Some(cycle);
                    }
                } else if in_stack.contains(&neighbor) {
                    // Found cycle — extract it from path
                    let pos = path.iter().position(|&x| x == neighbor).unwrap();
                    let mut cycle: Vec<u64> = path[pos..].to_vec();
                    cycle.push(neighbor); // Close the cycle
                    return Some(cycle);
                }
            }
        }

        path.pop();
        in_stack.remove(&node);
        None
    }
}

impl Default for DeadlockDetector {
    fn default() -> Self {
        Self::new()
    }
}

// ━━━ Latency Jitter ━━━

/// High-precision latency measurement with histogram bucketing.
/// Tracks min, max, mean, p50, p95, p99, p999, and jitter.
#[derive(Debug, Clone)]
pub struct LatencyHistogram {
    samples: Vec<Duration>,
    max_samples: usize,
    /// Pre-computed buckets for fast percentile queries
    sorted: bool,
}

impl LatencyHistogram {
    pub fn new(max_samples: usize) -> Self {
        Self {
            samples: Vec::with_capacity(max_samples),
            max_samples,
            sorted: false,
        }
    }

    /// Record a latency sample
    pub fn record(&mut self, latency: Duration) {
        if self.samples.len() >= self.max_samples {
            self.samples.remove(0);
        }
        self.samples.push(latency);
        self.sorted = false;
    }

    /// Ensure samples are sorted for percentile queries
    fn ensure_sorted(&mut self) {
        if !self.sorted {
            self.samples.sort();
            self.sorted = true;
        }
    }

    /// Get the Nth percentile latency
    pub fn percentile(&mut self, pct: f64) -> Option<Duration> {
        if self.samples.is_empty() {
            return None;
        }
        self.ensure_sorted();
        let idx = ((self.samples.len() - 1) as f64 * pct / 100.0) as usize;
        Some(self.samples[idx.min(self.samples.len() - 1)])
    }

    /// Minimum latency
    pub fn min(&self) -> Option<Duration> {
        self.samples.iter().min().copied()
    }

    /// Maximum latency
    pub fn max(&self) -> Option<Duration> {
        self.samples.iter().max().copied()
    }

    /// Mean latency
    pub fn mean(&self) -> Option<Duration> {
        if self.samples.is_empty() {
            return None;
        }
        let total_ns: u128 = self.samples.iter().map(std::time::Duration::as_nanos).sum();
        Some(Duration::from_nanos(
            (total_ns / self.samples.len() as u128) as u64,
        ))
    }

    /// Jitter = standard deviation of latencies
    pub fn jitter(&self) -> Option<Duration> {
        if self.samples.len() < 2 {
            return None;
        }
        let mean = self.mean()?.as_nanos() as f64;
        let variance: f64 = self
            .samples
            .iter()
            .map(|d| {
                let diff = d.as_nanos() as f64 - mean;
                diff * diff
            })
            .sum::<f64>()
            / (self.samples.len() - 1) as f64;
        Some(Duration::from_nanos(variance.sqrt() as u64))
    }

    /// Sample count
    pub fn len(&self) -> usize {
        self.samples.len()
    }
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Reset histogram
    pub fn clear(&mut self) {
        self.samples.clear();
        self.sorted = false;
    }

    /// Full latency report
    pub fn report(&mut self) -> String {
        let p50 = self
            .percentile(50.0)
            .map_or("N/A".into(), |d| format!("{}µs", d.as_micros()));
        let p99 = self
            .percentile(99.0)
            .map_or("N/A".into(), |d| format!("{}µs", d.as_micros()));
        let p999 = self
            .percentile(99.9)
            .map_or("N/A".into(), |d| format!("{}µs", d.as_micros()));
        let min = self
            .min()
            .map_or("N/A".into(), |d| format!("{}µs", d.as_micros()));
        let max = self
            .max()
            .map_or("N/A".into(), |d| format!("{}µs", d.as_micros()));
        let mean = self
            .mean()
            .map_or("N/A".into(), |d| format!("{}µs", d.as_micros()));
        let jitter = self
            .jitter()
            .map_or("N/A".into(), |d| format!("{}µs", d.as_micros()));
        format!(
            "Latency[{} samples]: min={} max={} mean={} p50={} p99={} p999={} jitter={}",
            self.len(),
            min,
            max,
            mean,
            p50,
            p99,
            p999,
            jitter
        )
    }
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        Self::new(100_000)
    }
}

// ━━━ Choreography ━━━

/// In choreography, each service listens for events and decides what to do next.
/// No central orchestrator — decentralized coordination via event bus.

#[derive(Debug, Clone)]
pub struct ChoreographyStep {
    pub service_name: String,
    /// Event that triggers this step
    pub listens_for: String,
    /// Action to execute
    pub action: String,
    /// Event emitted on success
    pub emits_on_success: String,
    /// Event emitted on failure
    pub emits_on_failure: String,
    /// Compensating action (for rollback)
    pub compensation: Option<String>,
}

/// Event bus for choreography-based sagas.
/// Services subscribe to events, handlers produce new events.
pub struct EventBus {
    /// subscribers: `event_type` → list of (service, handler)
    subscribers: HashMap<
        String,
        Vec<(
            String,
            Box<dyn Fn(&str) -> Result<String, String> + Send + Sync>,
        )>,
    >,
    /// Event history (for debugging/replay)
    pub history: Vec<BusEvent>,
}

#[derive(Debug, Clone)]
pub struct BusEvent {
    pub event_type: String,
    pub payload: String,
    pub produced_by: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl EventBus {
    pub fn new() -> Self {
        Self {
            subscribers: HashMap::new(),
            history: Vec::new(),
        }
    }

    /// Subscribe a service to an event type
    pub fn subscribe<F>(&mut self, service: &str, event_type: &str, handler: F)
    where
        F: Fn(&str) -> Result<String, String> + Send + Sync + Clone + 'static,
    {
        self.subscribers
            .entry(event_type.to_string())
            .or_default()
            .push((service.to_string(), Box::new(handler)));
    }

    /// Publish an event — all subscribers are notified in order.
    /// Returns the events produced by handlers.
    pub fn publish(&mut self, event_type: &str, payload: &str, source: &str) -> Vec<BusEvent> {
        self.history.push(BusEvent {
            event_type: event_type.to_string(),
            payload: payload.to_string(),
            produced_by: source.to_string(),
            timestamp: chrono::Utc::now(),
        });

        // Collect handler indices to avoid borrow issues
        let handler_count = self
            .subscribers
            .get(event_type)
            .map_or(0, std::vec::Vec::len);
        let mut output_events = Vec::new();

        // Process each handler by index
        for _ in 0..handler_count {
            // Take handlers temporarily, process one, put back
            let handler_info = if let Some(handlers) = self.subscribers.get_mut(event_type) {
                if handlers.is_empty() {
                    break;
                }
                Some(handlers.remove(0))
            } else {
                None
            };

            if let Some((service, handler)) = handler_info {
                let result = handler(payload);
                match result {
                    Ok(new_payload) => {
                        let event_name = format!("{event_type}_SUCCESS");
                        let sub_events = self.publish(&event_name, &new_payload, &service);
                        output_events.extend(sub_events);
                    }
                    Err(err) => {
                        let fail_event = format!("{event_type}_FAILED");
                        let fail_payload =
                            format!("{{\"error\":\"{err}\",\"original\":{payload}}}");
                        self.publish(&fail_event, &fail_payload, &service);
                    }
                }
            }
        }

        output_events
    }

    /// Get all events for debugging
    pub fn event_history(&self) -> &[BusEvent] {
        &self.history
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

/// Build a choreography-based transfer saga (vs orchestration).
pub fn build_transfer_choreography() -> (EventBus, ChoreographyDefinition) {
    let definition = ChoreographyDefinition {
        name: "TransferChoreography".into(),
        steps: vec![
            ChoreographyStep {
                service_name: "AccountService".into(),
                listens_for: "TransferRequested".into(),
                action: "DebitSource".into(),
                emits_on_success: "SourceDebited".into(),
                emits_on_failure: "TransferFailed".into(),
                compensation: Some("CreditSource".into()),
            },
            ChoreographyStep {
                service_name: "AccountService".into(),
                listens_for: "SourceDebited".into(),
                action: "CreditDestination".into(),
                emits_on_success: "TransferCompleted".into(),
                emits_on_failure: "DestinationCreditFailed".into(),
                compensation: Some("DebitDestination".into()),
            },
            ChoreographyStep {
                service_name: "NotificationService".into(),
                listens_for: "TransferCompleted".into(),
                action: "NotifyParties".into(),
                emits_on_success: "Notified".into(),
                emits_on_failure: "NotifyFailed".into(),
                compensation: None,
            },
        ],
    };

    let mut bus = EventBus::new();

    // Simplified handlers that just pass through
    bus.subscribe("TransferRequested", "AccountService", |payload| {
        Ok(payload.to_string())
    });
    bus.subscribe("SourceDebited", "AccountService", |payload| {
        Ok(payload.to_string())
    });
    bus.subscribe("TransferCompleted", "NotificationService", |payload| {
        Ok(payload.to_string())
    });

    (bus, definition)
}

#[derive(Debug, Clone)]
pub struct ChoreographyDefinition {
    pub name: String,
    pub steps: Vec<ChoreographyStep>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deadlock_cycle_detection() {
        let mut dd = DeadlockDetector::new();
        // Thread 1 holds A, waits for B
        dd.acquire(1, "A");
        dd.wait_for(1, "B");
        dd.owners.insert("A".into(), 1);

        // Thread 2 holds B, waits for A
        dd.acquire(2, "B");
        dd.wait_for(2, "A");
        dd.owners.insert("B".into(), 2);

        let cycle = dd.detect_cycle();
        assert!(cycle.is_some());
        let c = cycle.unwrap();
        assert!(c.contains(&1) && c.contains(&2));
    }

    #[test]
    fn test_no_deadlock() {
        let mut dd = DeadlockDetector::new();
        dd.acquire(1, "A");
        dd.acquire(2, "B");
        dd.owners.insert("A".into(), 1);
        dd.owners.insert("B".into(), 2);
        // No waiting — no deadlock
        assert!(dd.detect_cycle().is_none());
    }

    #[test]
    fn test_latency_histogram() {
        let mut hist = LatencyHistogram::new(100);
        for i in 1..=100 {
            hist.record(Duration::from_micros(i));
        }

        assert_eq!(hist.len(), 100);
        assert!(hist.min().unwrap().as_micros() <= 5);
        assert!(hist.max().unwrap().as_micros() >= 95);
        let p50 = hist.percentile(50.0).unwrap();
        assert!(p50.as_micros() >= 45 && p50.as_micros() <= 55);

        let jitter = hist.jitter().unwrap();
        assert!(jitter.as_micros() > 0);
    }

    #[test]
    fn test_choreography_event_bus() {
        let mut bus = EventBus::new();

        bus.subscribe("OrderPlaced", "PaymentService", |payload| {
            Ok(format!("{{\"paid\":true,\"order\":{}}}", payload))
        });

        bus.publish("OrderPlaced", r#"{"id":"ORD-001"}"#, "OrderService");

        assert_eq!(bus.event_history().len(), 1);
        assert_eq!(bus.event_history()[0].event_type, "OrderPlaced");
    }
}
