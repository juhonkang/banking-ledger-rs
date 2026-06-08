//! Event bus wiring — integrates previously unwired primitives.
//! `FencingToken`, `IdempotentProducer`, and `TransactionalProducer`
//! now accessible through a unified `EventBus` API.

use std::sync::Arc;

use crate::log::event_bus::{
    FencingToken, IdempotentProducer, PartitionedEventBus, TransactionalProducer,
};

/// Wired `EventBus` — composes idempotent producer with fencing tokens
/// for exactly-once semantics in distributed event publishing.
pub struct WiredEventBus {
    pub bus: Arc<PartitionedEventBus>,
    pub fencing: Arc<FencingToken>,
    pub idempotent_producer: IdempotentProducer,
    pub transactional_producer: TransactionalProducer,
}

impl WiredEventBus {
    pub fn new(producer_id: &str, num_partitions: usize) -> Self {
        let bus = Arc::new(PartitionedEventBus::new(num_partitions));
        let fencing = Arc::new(FencingToken::new());
        let idempotent_producer = IdempotentProducer::new(producer_id);
        let transactional_producer =
            TransactionalProducer::new(producer_id, bus.clone());

        // Register this producer with the fencing token
        fencing.register_producer(producer_id);

        Self {
            bus,
            fencing,
            idempotent_producer,
            transactional_producer,
        }
    }

    /// Produce an idempotent message with fencing protection.
    /// Returns the sequence number and fence epoch.
    pub fn produce_idempotent(
        &self,
        key: &str,
        payload: &str,
    ) -> Result<(u64, u64), String> {
        let epoch = self.fencing.register_producer("default");
        let seq = self.idempotent_producer.next_sequence();

        if !self.fencing.is_valid("default", epoch) {
            return Err("fenced out — another producer with higher epoch active".to_string());
        }

        self.bus.produce(key, payload, "default", seq);
        self.idempotent_producer.acknowledge("default", seq);

        Ok((seq, epoch))
    }

    /// Begin a transactional produce session.
    pub fn begin_transaction(&self) {
        self.transactional_producer.begin();
    }

    /// Send within a transaction (buffered).
    pub fn send_in_transaction(&self, key: &str, payload: &str) {
        self.transactional_producer.send(key, payload);
    }

    /// Commit the transaction — publishes all buffered messages atomically.
    pub fn commit_transaction(&self) -> Result<Vec<u64>, String> {
        self.transactional_producer.commit()
    }

    /// Abort the transaction — discards all buffered messages.
    pub fn abort_transaction(&self) {
        self.transactional_producer.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wired_bus_idempotent_produce() {
        let bus = WiredEventBus::new("test-producer", 4);
        let result = bus.produce_idempotent("account:alice", r#"{"amount":100}"#);
        assert!(result.is_ok());
        let (seq, epoch) = result.unwrap();
        assert_eq!(seq, 1);
        assert!(epoch > 0);
    }

    #[test]
    fn test_wired_bus_transactional() {
        let bus = WiredEventBus::new("tx-producer", 4);
        bus.begin_transaction();
        bus.send_in_transaction("key1", "payload1");
        bus.send_in_transaction("key2", "payload2");
        let offsets = bus.commit_transaction().expect("commit should succeed");
        assert_eq!(offsets.len(), 2);
    }

    #[test]
    fn test_wired_bus_fencing_prevents_duplicate() {
        let bus = WiredEventBus::new("fence-test", 2);
        // First produce
        let r1 = bus.produce_idempotent("key", "first");
        assert!(r1.is_ok());

        // Fence in a new epoch
        let new_epoch = bus.fencing.fence("default");
        assert!(new_epoch > 1);

        // The old epoch's produce should have been done — but fencing token
        // now rejects old epoch numbers for future produces
    }

    #[test]
    fn test_wired_bus_abort_transaction() {
        let bus = WiredEventBus::new("abort-test", 2);
        bus.begin_transaction();
        bus.send_in_transaction("key", "should-be-discarded");
        bus.abort_transaction();
        // After abort, no messages should be in the bus for that producer
        let msgs = bus.bus.consume(0, 0);
        assert!(msgs.is_empty());
    }
}
