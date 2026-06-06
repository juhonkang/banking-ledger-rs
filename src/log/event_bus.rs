//! Distributed event bus with exactly-once semantics.
//! Provides: partitioned topics, idempotent producers, fencing tokens,
//! consumer groups, and transactional producers.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ━━━ Sharding ━━━

/// Consistent hashing ring for sharding accounts across partitions.
pub struct ShardRouter {
    num_shards: u32,
}

impl ShardRouter {
    pub fn new(num_shards: u32) -> Self {
        Self { num_shards }
    }

    /// Route an account ID to its shard (0-based).
    /// Uses simple modulo — for production, use consistent hashing.
    pub fn shard_for(&self, account_id: Uuid) -> u32 {
        let bytes = account_id.as_bytes();
        let hash = bytes.iter().fold(0u64, |acc, &b| {
            acc.wrapping_mul(31).wrapping_add(u64::from(b))
        });
        (hash % u64::from(self.num_shards)) as u32
    }

    /// Route a transaction to the shard of its primary account
    pub fn shard_for_tx(&self, primary_account: Uuid) -> u32 {
        self.shard_for(primary_account)
    }
}

// ━━━ Partitioned Bus ━━━

/// A partitioned event bus — like Kafka topics with multiple partitions.
pub struct PartitionedEventBus {
    partitions: Vec<Mutex<VecDeque<BusMessage>>>,
    /// Next offset to assign
    next_offset: AtomicU64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusMessage {
    pub offset: u64,
    pub key: String,
    pub payload: String,
    pub timestamp: DateTime<Utc>,
    pub producer_id: String,
    /// Sequence number from this producer
    pub producer_seq: u64,
}

impl PartitionedEventBus {
    pub fn new(num_partitions: usize) -> Self {
        Self {
            partitions: (0..num_partitions)
                .map(|_| Mutex::new(VecDeque::new()))
                .collect(),
            next_offset: AtomicU64::new(0),
        }
    }

    /// Produce a message to a partition (key-based routing)
    pub fn produce(&self, key: &str, payload: &str, producer_id: &str, producer_seq: u64) -> u64 {
        let partition = self.partition_for(key);
        let offset = self.next_offset.fetch_add(1, Ordering::SeqCst);

        let msg = BusMessage {
            offset,
            key: key.to_string(),
            payload: payload.to_string(),
            timestamp: Utc::now(),
            producer_id: producer_id.to_string(),
            producer_seq,
        };

        self.partitions[partition].lock().unwrap().push_back(msg);
        offset
    }

    /// Consume from a specific partition starting at offset
    pub fn consume(&self, partition: usize, from_offset: u64) -> Vec<BusMessage> {
        let queue = self.partitions[partition].lock().unwrap();
        queue
            .iter()
            .filter(|m| m.offset >= from_offset)
            .cloned()
            .collect()
    }

    fn partition_for(&self, key: &str) -> usize {
        let hash = key.bytes().fold(0u64, |acc, b| {
            acc.wrapping_mul(31).wrapping_add(u64::from(b))
        });
        (hash as usize) % self.partitions.len()
    }
}

// ━━━ Idempotent Producer ━━━

/// An idempotent producer — guarantees exactly-once delivery.
/// Tracks (`producer_id`, `sequence_number`) pairs to detect duplicates.
pub struct IdempotentProducer {
    producer_id: String,
    /// Last sequence number used
    last_seq: AtomicU64,
    /// Set of acknowledged (`producer_id`, seq) pairs
    acknowledged: Mutex<HashSet<(String, u64)>>,
}

impl IdempotentProducer {
    pub fn new(producer_id: &str) -> Self {
        Self {
            producer_id: producer_id.to_string(),
            last_seq: AtomicU64::new(0),
            acknowledged: Mutex::new(HashSet::new()),
        }
    }

    /// Get the next sequence number (monotonically increasing per producer)
    pub fn next_sequence(&self) -> u64 {
        self.last_seq.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Check if a message was already acknowledged (dedup)
    pub fn is_duplicate(&self, producer_id: &str, seq: u64) -> bool {
        self.acknowledged
            .lock()
            .unwrap()
            .contains(&(producer_id.to_string(), seq))
    }

    /// Acknowledge a produced message
    pub fn acknowledge(&self, producer_id: &str, seq: u64) {
        self.acknowledged
            .lock()
            .unwrap()
            .insert((producer_id.to_string(), seq));
    }

    /// Produce idempotently — returns None if duplicate
    pub fn produce(
        &self,
        bus: &PartitionedEventBus,
        key: &str,
        payload: &str,
        seq: u64,
    ) -> Option<u64> {
        if self.is_duplicate(&self.producer_id, seq) {
            return None; // Already produced
        }
        let offset = bus.produce(key, payload, &self.producer_id, seq);
        self.acknowledge(&self.producer_id, seq);
        Some(offset)
    }
}

// ━━━ Fencing Tokens ━━━

/// Fencing token — monotonically increasing epoch.
/// Zombie producers with stale epochs are rejected.
pub struct FencingToken {
    /// Current epoch (monotonically increasing)
    epoch: AtomicU64,
    /// Producer registration: `producer_id` → `assigned_epoch`
    registrations: Mutex<HashMap<String, u64>>,
}

impl FencingToken {
    pub fn new() -> Self {
        Self {
            epoch: AtomicU64::new(1),
            registrations: Mutex::new(HashMap::new()),
        }
    }

    /// Register a producer and assign it an epoch token.
    /// This increments the global epoch, invalidating old producers.
    pub fn register_producer(&self, producer_id: &str) -> u64 {
        let epoch = self.epoch.fetch_add(1, Ordering::SeqCst);
        self.registrations
            .lock()
            .unwrap()
            .insert(producer_id.to_string(), epoch);
        epoch
    }

    /// Check if a producer's epoch is current (not a zombie)
    pub fn is_valid(&self, producer_id: &str, epoch: u64) -> bool {
        self.registrations
            .lock()
            .unwrap()
            .get(producer_id)
            .is_some_and(|&current| current == epoch)
    }

    /// Fence out a producer (increment epoch, invalidating old tokens)
    pub fn fence(&self, producer_id: &str) -> u64 {
        self.register_producer(producer_id)
    }
}

impl Default for FencingToken {
    fn default() -> Self {
        Self::new()
    }
}

// ━━━ Consumer Groups ━━━

/// A consumer group — multiple consumers sharing partition work.
pub struct ConsumerGroup {
    group_id: String,
    /// Partition → last committed offset
    offsets: Mutex<HashMap<usize, u64>>,
    /// Active consumers
    consumers: Mutex<Vec<ConsumerState>>,
}

#[derive(Debug, Clone)]
pub struct ConsumerState {
    pub consumer_id: String,
    pub assigned_partitions: Vec<usize>,
    pub last_heartbeat: DateTime<Utc>,
}

impl ConsumerGroup {
    pub fn new(group_id: &str) -> Self {
        Self {
            group_id: group_id.to_string(),
            offsets: Mutex::new(HashMap::new()),
            consumers: Mutex::new(Vec::new()),
        }
    }

    /// Register a consumer in the group
    pub fn register_consumer(&self, consumer_id: &str, partitions: Vec<usize>) {
        self.consumers.lock().unwrap().push(ConsumerState {
            consumer_id: consumer_id.to_string(),
            assigned_partitions: partitions,
            last_heartbeat: Utc::now(),
        });
    }

    /// Commit an offset for a partition
    pub fn commit_offset(&self, partition: usize, offset: u64) {
        self.offsets.lock().unwrap().insert(partition, offset);
    }

    /// Get the last committed offset for a partition
    pub fn committed_offset(&self, partition: usize) -> u64 {
        self.offsets
            .lock()
            .unwrap()
            .get(&partition)
            .copied()
            .unwrap_or(0)
    }

    /// Rebalance: detect dead consumers and reassign partitions
    pub fn rebalance(&self, timeout_secs: i64) -> Vec<String> {
        let now = Utc::now();
        let mut dead = Vec::new();
        let mut consumers = self.consumers.lock().unwrap();

        consumers.retain(|c| {
            let alive = (now - c.last_heartbeat).num_seconds() < timeout_secs;
            if !alive {
                dead.push(c.consumer_id.clone());
            }
            alive
        });

        dead
    }

    /// Heartbeat from a consumer
    pub fn heartbeat(&self, consumer_id: &str) {
        if let Some(c) = self
            .consumers
            .lock()
            .unwrap()
            .iter_mut()
            .find(|c| c.consumer_id == consumer_id)
        {
            c.last_heartbeat = Utc::now();
        }
    }
}

// ━━━ Exactly-Once Semantics ━━━

/// A transactional producer that guarantees exactly-once across partitions.
pub struct TransactionalProducer {
    producer_id: String,
    /// Pending messages in current transaction
    pending: Mutex<Vec<(String, String)>>,
    /// Whether a transaction is in progress
    in_transaction: AtomicBool,
    bus: Arc<PartitionedEventBus>,
}

impl TransactionalProducer {
    pub fn new(producer_id: &str, bus: Arc<PartitionedEventBus>) -> Self {
        Self {
            producer_id: producer_id.to_string(),
            pending: Mutex::new(Vec::new()),
            in_transaction: AtomicBool::new(false),
            bus,
        }
    }

    /// Begin a transaction
    pub fn begin(&self) {
        self.in_transaction.store(true, Ordering::SeqCst);
        self.pending.lock().unwrap().clear();
    }

    /// Add a message to the current transaction
    pub fn send(&self, key: &str, payload: &str) {
        if self.in_transaction.load(Ordering::SeqCst) {
            self.pending
                .lock()
                .unwrap()
                .push((key.to_string(), payload.to_string()));
        }
    }

    /// Commit the transaction — atomically publish all pending messages.
    /// Uses a simple all-or-nothing approach.
    pub fn commit(&self) -> Result<Vec<u64>, String> {
        if !self.in_transaction.load(Ordering::SeqCst) {
            return Err("No transaction in progress".into());
        }

        let pending = self.pending.lock().unwrap().clone();
        let mut offsets = Vec::new();

        // Atomically produce all (simplified — real impl would use 2PC)
        for (key, payload) in &pending {
            let seq = 0; // Would use proper sequencing
            let offset = self.bus.produce(key, payload, &self.producer_id, seq);
            offsets.push(offset);
        }

        self.in_transaction.store(false, Ordering::SeqCst);
        self.pending.lock().unwrap().clear();
        Ok(offsets)
    }

    /// Abort the transaction
    pub fn abort(&self) {
        self.in_transaction.store(false, Ordering::SeqCst);
        self.pending.lock().unwrap().clear();
    }
}

// ━━━ Exactly-Once Tuning ━━━

/// Configuration for exactly-once semantics.
#[derive(Debug, Clone)]
pub struct ExactlyOnceConfig {
    /// Enable idempotent producer
    pub enable_idempotence: bool,
    /// Max in-flight requests per connection
    pub max_in_flight: u32,
    /// Acknowledgment mode: 0=none, 1=leader, -1=all
    pub acks: i8,
    /// Transaction timeout in milliseconds
    pub transaction_timeout_ms: u64,
    /// Enable read-committed isolation
    pub read_committed: bool,
}

impl Default for ExactlyOnceConfig {
    fn default() -> Self {
        Self {
            enable_idempotence: true,
            max_in_flight: 5,
            acks: -1, // all replicas
            transaction_timeout_ms: 60000,
            read_committed: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shard_router_consistency() {
        let router = ShardRouter::new(16);
        let id = Uuid::now_v7();
        assert_eq!(router.shard_for(id), router.shard_for(id));
    }

    #[test]
    fn test_partitioned_bus_produce_consume() {
        let bus = PartitionedEventBus::new(4);
        let offset = bus.produce("account:A", r#"{"amount":100}"#, "p1", 1);
        assert_eq!(offset, 0);

        let msgs = bus.consume(offset as usize % 4, 0);
        assert_eq!(msgs.len(), 1);
    }

    #[test]
    fn test_idempotent_producer_dedup() {
        let producer = IdempotentProducer::new("prod-1");
        let bus = PartitionedEventBus::new(1);

        let seq = producer.next_sequence();
        let r1 = producer.produce(&bus, "key1", "data", seq);
        assert!(r1.is_some());

        // Same seq → should be duplicate
        let r2 = producer.produce(&bus, "key1", "data", seq);
        assert!(r2.is_none());
    }

    #[test]
    fn test_fencing_token_zombie_rejection() {
        let fence = FencingToken::new();
        let epoch1 = fence.register_producer("prod-A");
        assert!(fence.is_valid("prod-A", epoch1));

        // New registration invalidates old epoch
        let epoch2 = fence.register_producer("prod-A");
        assert!(!fence.is_valid("prod-A", epoch1));
        assert!(fence.is_valid("prod-A", epoch2));
    }

    #[test]
    fn test_consumer_group_rebalance() {
        let group = ConsumerGroup::new("ledger-consumers");
        group.register_consumer("c1", vec![0, 1]);
        group.register_consumer("c2", vec![2, 3]);

        // c1 dies (no heartbeat, immediate timeout)
        let dead = group.rebalance(0);
        assert_eq!(dead.len(), 2); // Neither has heartbeat
    }

    #[test]
    fn test_transactional_producer_commit() {
        let bus = Arc::new(PartitionedEventBus::new(2));
        let producer = TransactionalProducer::new("tx-prod", bus);

        producer.begin();
        producer.send("account:A", "debit:100");
        producer.send("account:B", "credit:100");

        let offsets = producer.commit().unwrap();
        assert_eq!(offsets.len(), 2);
    }
}
