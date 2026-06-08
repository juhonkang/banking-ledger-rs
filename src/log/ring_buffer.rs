//! High-throughput, lock-free, cache-line-padded ring buffer.
//! Core mechanic: producer writes, consumer reads, sequence barriers coordinate.
//! Pre-allocated slots with cache-line-padded sequence counters.

use std::cell::UnsafeCell;
use std::fmt::Debug;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

// ━━━ Cache Line Padding ━━━

/// Cache line size for x86-64. Prevents false sharing.
pub const CACHE_LINE_SIZE: usize = 64;

/// Pad a sequence counter to its own cache line to prevent false sharing.
#[repr(align(64))]
#[derive(Debug)]
pub struct PaddedAtomic(AtomicUsize);

impl PaddedAtomic {
    pub fn new(value: usize) -> Self {
        Self(AtomicUsize::new(value))
    }

    pub fn load(&self, ordering: Ordering) -> usize {
        self.0.load(ordering)
    }

    pub fn store(&self, value: usize, ordering: Ordering) {
        self.0.store(value, ordering);
    }

    pub fn compare_exchange(
        &self,
        current: usize,
        new: usize,
        success: Ordering,
        failure: Ordering,
    ) -> Result<usize, usize> {
        self.0.compare_exchange(current, new, success, failure)
    }
}

// ━━━ Wait Strategies ━━━

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitStrategy {
    /// Busy-spin (lowest latency, highest CPU)
    Spin,
    /// `Thread::yield_now()` between retries
    Yield,
    /// `Thread::park()` / `park_timeout`
    Park(Duration),
    /// Blocking — `thread::park()` (lowest CPU, highest latency)
    Blocking,
}

impl WaitStrategy {
    pub fn apply(&self) {
        match self {
            Self::Spin => {
                std::hint::spin_loop();
            }
            Self::Yield => {
                thread::yield_now();
            }
            Self::Park(timeout) => {
                thread::park_timeout(*timeout);
            }
            Self::Blocking => {
                thread::park();
            }
        }
    }
}

// ━━━ Sequence Barrier ━━━

/// Coordinates between producers and consumers.
/// Tracks what sequence numbers have been published.
#[derive(Debug)]
pub struct SequenceBarrier {
    /// Cursor — the latest published sequence
    cursor: PaddedAtomic,
    /// Dependent sequences (e.g., consumer positions)
    dependents: Vec<PaddedAtomic>,
}

impl SequenceBarrier {
    pub fn new(initial: usize) -> Self {
        Self {
            cursor: PaddedAtomic::new(initial),
            dependents: vec![],
        }
    }

    /// Wait for a specific sequence to become available
    pub fn wait_for(&self, sequence: usize, strategy: WaitStrategy) -> usize {
        loop {
            let available = self.cursor.load(Ordering::Acquire);
            if available >= sequence {
                return available;
            }
            strategy.apply();
        }
    }

    /// Get the minimum sequence among all dependents.
    /// Used to prevent overwriting unread data.
    pub fn minimum_dependent_sequence(&self) -> usize {
        self.dependents
            .iter()
            .map(|d| d.load(Ordering::Acquire))
            .min()
            .unwrap_or(usize::MAX)
    }
}

// ━━━ Ring Buffer ━━━

/// A pre-allocated, lock-free ring buffer for financial events.
/// Size MUST be a power of 2 for efficient modulo via bitwise AND.
pub struct RingBuffer<T> {
    /// Pre-allocated slots
    slots: Box<[UnsafeCell<MaybeUninit<T>>]>,
    /// Bitmask for index calculation (size - 1)
    mask: usize,
    /// Producer sequence
    producer_sequence: PaddedAtomic,
    /// Consumer sequence
    consumer_sequence: PaddedAtomic,
    /// Buffer size
    capacity: usize,
}

// UnsafeCell implies !Sync, but we're managing synchronization ourselves
unsafe impl<T: Send> Send for RingBuffer<T> {}
unsafe impl<T: Send> Sync for RingBuffer<T> {}

impl<T> RingBuffer<T> {
    /// Create a ring buffer with a capacity that is rounded up to next power of 2.
    pub fn new(min_capacity: usize) -> Self {
        let capacity = min_capacity.next_power_of_two();
        let mut slots = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            slots.push(UnsafeCell::new(MaybeUninit::uninit()));
        }

        Self {
            slots: slots.into_boxed_slice(),
            mask: capacity - 1,
            producer_sequence: PaddedAtomic::new(0),
            consumer_sequence: PaddedAtomic::new(0),
            capacity,
        }
    }

    /// Get the slot for a given sequence number
    fn slot(&self, sequence: usize) -> &UnsafeCell<MaybeUninit<T>> {
        &self.slots[sequence & self.mask]
    }

    // ━━━ Single Producer ━━━

    /// Claim the next slot for writing. Returns the sequence number.
    pub fn claim(&self) -> Result<usize, RingBufferError> {
        let seq = self.producer_sequence.load(Ordering::Relaxed);
        let next_seq = seq + 1;

        // Check if we'd overwrite unread data
        let consumer_seq = self.consumer_sequence.load(Ordering::Acquire);
        if next_seq - consumer_seq > self.capacity {
            return Err(RingBufferError::Full);
        }

        // Claim the slot (single producer — no CAS needed)
        self.producer_sequence.store(next_seq, Ordering::Release);
        Ok(seq)
    }

    /// Write data to a claimed slot and publish.
    /// # Safety
    /// `sequence` must have been claimed via `claim()` first.
    pub unsafe fn publish(&self, sequence: usize, data: T) {
        let slot = self.slot(sequence).get();
        (*slot).write(data);
    }

    /// Claim + publish in one call (single producer)
    pub fn try_push(&self, data: T) -> Result<usize, RingBufferError> {
        let seq = self.claim()?;
        // SAFETY: seq was just claimed
        unsafe {
            self.publish(seq, data);
        }
        Ok(seq)
    }

    // ━━━ Consumer ━━━

    /// Read the next available entry. Returns None if buffer is empty.
    pub fn try_pop(&self) -> Option<T> {
        let consumer_seq = self.consumer_sequence.load(Ordering::Relaxed);
        let producer_seq = self.producer_sequence.load(Ordering::Acquire);

        if consumer_seq >= producer_seq {
            return None;
        }

        let seq = consumer_seq;
        let slot = self.slot(seq).get();
        let data = unsafe { (*slot).assume_init_read() };
        self.consumer_sequence.store(seq + 1, Ordering::Release);
        Some(data)
    }

    /// Pop with wait strategy. Returns None if interrupted.
    pub fn pop_wait(&self, strategy: WaitStrategy, timeout: Duration) -> Option<T> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(data) = self.try_pop() {
                return Some(data);
            }
            if Instant::now() > deadline {
                return None;
            }
            strategy.apply();
        }
    }

    /// Number of unread entries
    pub fn len(&self) -> usize {
        let producer = self.producer_sequence.load(Ordering::Acquire);
        let consumer = self.consumer_sequence.load(Ordering::Acquire);
        producer.saturating_sub(consumer)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_full(&self) -> bool {
        self.len() >= self.capacity
    }
}

impl<T> Drop for RingBuffer<T> {
    fn drop(&mut self) {
        // Drop any remaining populated slots
        let consumer = self.consumer_sequence.load(Ordering::Relaxed);
        let producer = self.producer_sequence.load(Ordering::Relaxed);
        for seq in consumer..producer {
            let slot = self.slot(seq).get();
            unsafe {
                (*slot).assume_init_drop();
            }
        }
    }
}

#[derive(Debug)]
pub enum RingBufferError {
    Full,
    Empty,
}

// ━━━ Dependency Graph ━━━

/// Tracks which sequences depend on which.
/// Used in complex event processing where an event handler
/// must wait for multiple upstream handlers before processing.
pub struct DependencyGraph {
    dependencies: Vec<Vec<usize>>,
    completed: Vec<AtomicBool>,
}

impl DependencyGraph {
    pub fn new(num_nodes: usize) -> Self {
        Self {
            dependencies: vec![vec![]; num_nodes],
            completed: (0..num_nodes).map(|_| AtomicBool::new(false)).collect(),
        }
    }

    /// Register that node `dependent` depends on node `dependency`
    pub fn add_dependency(&mut self, dependent: usize, dependency: usize) {
        self.dependencies[dependent].push(dependency);
    }

    /// Mark a node as completed
    pub fn complete(&self, node: usize) {
        self.completed[node].store(true, Ordering::Release);
    }

    /// Check if all dependencies of a node are completed
    pub fn all_dependencies_done(&self, node: usize) -> bool {
        self.dependencies[node]
            .iter()
            .all(|&dep| self.completed[dep].load(Ordering::Acquire))
    }
}

// ━━━ Batching Consumer ━━━

/// Consume multiple entries at once for amortized synchronization cost.
pub fn batch_consume<T: Debug + Clone>(buffer: &RingBuffer<T>, batch_size: usize) -> Vec<T> {
    let mut batch = Vec::with_capacity(batch_size);
    for _ in 0..batch_size {
        match buffer.try_pop() {
            Some(item) => batch.push(item),
            None => break,
        }
    }
    batch
}

// ━━━ Latency Histogram ━━━

/// HDR-style latency histogram for ring buffer operations.
/// Tracks operation latencies with exponential buckets for percentile calculation.
#[derive(Debug)]
pub struct LatencyHistogram {
    /// Counters for each latency bucket (powers of 2): 1µs, 2µs, 4µs, ..., 2^20 µs (~1s)
    buckets: Box<[AtomicUsize]>,
    /// Total count of all samples
    total_count: AtomicUsize,
    /// Sum of all latencies (in nanoseconds) for mean calculation
    total_ns: AtomicUsize,
    /// Minimum observed latency (nanoseconds)
    min_ns: AtomicUsize,
    /// Maximum observed latency (nanoseconds)
    max_ns: AtomicUsize,
    /// Total number of buckets
    num_buckets: usize,
}

impl LatencyHistogram {
    /// Create a histogram with `num_buckets` exponential buckets (powers of 2).
    /// Default: 20 buckets covering 1µs to ~1 second.
    pub fn new(num_buckets: usize) -> Self {
        let mut buckets = Vec::with_capacity(num_buckets);
        for _ in 0..num_buckets {
            buckets.push(AtomicUsize::new(0));
        }
        Self {
            buckets: buckets.into_boxed_slice(),
            total_count: AtomicUsize::new(0),
            total_ns: AtomicUsize::new(0),
            min_ns: AtomicUsize::new(usize::MAX),
            max_ns: AtomicUsize::new(0),
            num_buckets,
        }
    }

    /// Record an operation with its latency in nanoseconds.
    pub fn record(&self, latency_ns: u64) {
        let ns = latency_ns as usize;
        let bucket = self.bucket_for(ns);
        self.buckets[bucket].fetch_add(1, Ordering::Relaxed);
        self.total_count.fetch_add(1, Ordering::Relaxed);
        self.total_ns.fetch_add(ns, Ordering::Relaxed);

        // Update min (CAS loop)
        let mut current_min = self.min_ns.load(Ordering::Relaxed);
        while ns < current_min {
            match self.min_ns.compare_exchange_weak(
                current_min, ns,
                Ordering::Release, Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current_min = actual,
            }
        }

        // Update max (CAS loop)
        let mut current_max = self.max_ns.load(Ordering::Relaxed);
        while ns > current_max {
            match self.max_ns.compare_exchange_weak(
                current_max, ns,
                Ordering::Release, Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current_max = actual,
            }
        }
    }

    /// Map nanoseconds to bucket index (powers of 2: 1µs, 2µs, 4µs, ...)
    fn bucket_for(&self, latency_ns: usize) -> usize {
        // Min bucket: 1000ns (1µs)
        let micros = latency_ns / 1_000;
        if micros == 0 {
            return 0;
        }
        let bucket = (usize::BITS - micros.leading_zeros()) as usize;
        bucket.min(self.num_buckets - 1)
    }

    /// Total sample count
    pub fn count(&self) -> usize {
        self.total_count.load(Ordering::Relaxed)
    }

    /// Mean latency in nanoseconds. Returns None if no samples.
    pub fn mean_ns(&self) -> Option<u64> {
        let count = self.total_count.load(Ordering::Relaxed);
        if count == 0 {
            return None;
        }
        let total = self.total_ns.load(Ordering::Relaxed);
        Some((total / count) as u64)
    }

    /// Minimum latency in nanoseconds. Returns None if no samples.
    pub fn min_ns(&self) -> Option<u64> {
        let v = self.min_ns.load(Ordering::Relaxed);
        if v == usize::MAX { None } else { Some(v as u64) }
    }

    /// Maximum latency in nanoseconds. Returns None if no samples.
    pub fn max_ns(&self) -> Option<u64> {
        let v = self.max_ns.load(Ordering::Relaxed);
        if v == 0 { None } else { Some(v as u64) }
    }

    /// Approximate percentile. Linear interpolation between bucket boundaries.
    /// `pct` should be 0.0-100.0.
    pub fn percentile(&self, pct: f64) -> Option<u64> {
        if !(0.0..=100.0).contains(&pct) {
            return None;
        }
        let total = self.total_count.load(Ordering::Relaxed);
        if total == 0 {
            return None;
        }
        let target = ((pct / 100.0) * total as f64).ceil() as usize;
        let mut cumulative = 0usize;
        for (i, bucket) in self.buckets.iter().enumerate() {
            cumulative += bucket.load(Ordering::Relaxed);
            if cumulative >= target {
                // Return upper bound of this bucket (in ns)
                if i == 0 {
                    return Some(1_000); // 1µs
                }
                return Some((1u64 << i) * 1_000);
            }
        }
        // Fallback: max bucket
        Some((1u64 << (self.num_buckets - 1)) * 1_000)
    }

    /// Reset all counters.
    pub fn reset(&self) {
        for bucket in &self.buckets {
            bucket.store(0, Ordering::Relaxed);
        }
        self.total_count.store(0, Ordering::Relaxed);
        self.total_ns.store(0, Ordering::Relaxed);
        self.min_ns.store(usize::MAX, Ordering::Relaxed);
        self.max_ns.store(0, Ordering::Relaxed);
    }
}

/// Timer helper for recording latency of an operation.
pub struct LatencyTimer<'a> {
    histogram: &'a LatencyHistogram,
    start: Instant,
}

impl<'a> LatencyTimer<'a> {
    pub fn start(histogram: &'a LatencyHistogram) -> Self {
        Self {
            histogram,
            start: Instant::now(),
        }
    }

    /// Record the elapsed time and return it.
    pub fn stop(self) -> u64 {
        let elapsed = self.start.elapsed().as_nanos() as u64;
        self.histogram.record(elapsed);
        elapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ring_buffer_push_pop() {
        let rb = RingBuffer::new(8);
        assert!(rb.is_empty());

        rb.try_push(42).unwrap();
        assert_eq!(rb.len(), 1);

        assert_eq!(rb.try_pop(), Some(42));
        assert!(rb.is_empty());
    }

    #[test]
    fn test_ring_buffer_full() {
        let rb = RingBuffer::new(4); // capacity = 4
        for i in 0..4 {
            rb.try_push(i).unwrap();
        }
        assert!(rb.is_full());
        assert!(rb.try_push(99).is_err());
    }

    #[test]
    fn test_ring_buffer_wrap_around() {
        let rb = RingBuffer::new(4);
        // Push 4, pop 4, push 4 again — verify wrap
        for i in 0..4 {
            rb.try_push(i * 10).unwrap();
        }
        for i in 0..4 {
            assert_eq!(rb.try_pop(), Some(i * 10));
        }
        for i in 0..4 {
            rb.try_push(i * 100).unwrap();
        }
        for i in 0..4 {
            assert_eq!(rb.try_pop(), Some(i * 100));
        }
    }

    #[test]
    fn test_batch_consume() {
        let rb = RingBuffer::new(8);
        for i in 0..6 {
            rb.try_push(i).unwrap();
        }

        let batch = batch_consume(&rb, 3);
        assert_eq!(batch, vec![0, 1, 2]);
        assert_eq!(rb.len(), 3);

        let batch2 = batch_consume(&rb, 10);
        assert_eq!(batch2, vec![3, 4, 5]);
        assert!(rb.is_empty());
    }

    #[test]
    fn test_dependency_graph() {
        let mut dg = DependencyGraph::new(4);
        // Node 3 depends on nodes 0, 1, 2
        dg.add_dependency(3, 0);
        dg.add_dependency(3, 1);
        dg.add_dependency(3, 2);

        assert!(!dg.all_dependencies_done(3));

        dg.complete(0);
        assert!(!dg.all_dependencies_done(3));

        dg.complete(1);
        dg.complete(2);
        assert!(dg.all_dependencies_done(3));
    }

    #[test]
    fn test_wait_strategy_spin() {
        // Verifies spin doesn't block forever on empty
        let rb = RingBuffer::<i32>::new(4);
        let _started = std::time::Instant::now();

        // Spin for at most 100ms then give up
        let result = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_millis(100);
            loop {
                if let Some(_) = rb.try_pop() {
                    return true;
                }
                if std::time::Instant::now() > deadline {
                    return false;
                }
                std::hint::spin_loop();
            }
        })
        .join()
        .unwrap();

        assert!(!result); // Should time out, not find data
    }

    // ━━━ Latency Histogram Tests ━━━

    #[test]
    fn test_histogram_empty() {
        let h = LatencyHistogram::new(20);
        assert_eq!(h.count(), 0);
        assert_eq!(h.mean_ns(), None);
        assert_eq!(h.min_ns(), None);
        assert_eq!(h.max_ns(), None);
        assert_eq!(h.percentile(50.0), None);
    }

    #[test]
    fn test_histogram_single_record() {
        let h = LatencyHistogram::new(20);
        h.record(5_000); // 5µs
        assert_eq!(h.count(), 1);
        assert_eq!(h.mean_ns(), Some(5_000));
        assert_eq!(h.min_ns(), Some(5_000));
        assert_eq!(h.max_ns(), Some(5_000));
    }

    #[test]
    fn test_histogram_percentiles() {
        let h = LatencyHistogram::new(20);
        // Record: 10 × 1µs, 20 × 10µs, 30 × 100µs, 40 × 1ms
        for _ in 0..10 { h.record(1_000); }
        for _ in 0..20 { h.record(10_000); }
        for _ in 0..30 { h.record(100_000); }
        for _ in 0..40 { h.record(1_000_000); }
        // Total: 100 records
        assert_eq!(h.count(), 100);
        // p50 should be around 100µs (50th record falls in 100µs bucket)
        let p50 = h.percentile(50.0).unwrap();
        assert!(p50 >= 1_000 && p50 <= 1_000_000);
        // p99 should be 1ms bucket
        let p99 = h.percentile(99.0).unwrap();
        assert!(p99 >= 1_000_000);
    }

    #[test]
    fn test_histogram_reset() {
        let h = LatencyHistogram::new(20);
        h.record(42_000);
        assert_eq!(h.count(), 1);
        h.reset();
        assert_eq!(h.count(), 0);
        assert_eq!(h.mean_ns(), None);
    }

    #[test]
    fn test_latency_timer() {
        let h = LatencyHistogram::new(20);
        {
            let timer = LatencyTimer::start(&h);
            std::thread::sleep(Duration::from_micros(500));
            let elapsed = timer.stop();
            assert!(elapsed >= 400_000); // at least 400µs
        }
        assert_eq!(h.count(), 1);
        assert!(h.mean_ns().unwrap() >= 400_000);
    }

    #[test]
    fn test_histogram_negative_percentile_returns_none() {
        let h = LatencyHistogram::new(20);
        h.record(1_000);
        assert_eq!(h.percentile(-1.0), None);
        assert_eq!(h.percentile(101.0), None);
    }
}
