//! High-throughput, lock-free, cache-line-padded ring buffer.
//! Core mechanic: producer writes, consumer reads, sequence barriers coordinate.
//! Pre-allocated slots with cache-line-padded sequence counters.

use std::cell::UnsafeCell;
use std::fmt::Debug;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

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
            return None; // Empty
        }

        let seq = consumer_seq;
        let slot = self.slot(seq).get();

        // SAFETY: producer has published, consumer hasn't read yet
        let data = unsafe { (*slot).assume_init_read() };
        self.consumer_sequence.store(seq + 1, Ordering::Release);

        Some(data)
    }

    /// Pop with a wait strategy for blocking consumers
    pub fn pop_wait(&self, strategy: WaitStrategy) -> T {
        loop {
            if let Some(data) = self.try_pop() {
                return data;
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
        let started = std::time::Instant::now();

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
}
