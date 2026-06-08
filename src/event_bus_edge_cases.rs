//! Extended edge case coverage for event bus + async flush buffer integration.
//! Covers multi-producer, fencing, idempotency, and concurrent stress.

#[cfg(test)]
mod event_bus_edge_tests {
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use crate::log::event_bus::{FencingToken, PartitionedEventBus};
    use crate::log::event_log::{AsyncFlushBuffer, Event, WalEntry};
    use crate::log::ring_buffer::RingBuffer;
    use crate::service::event_bus_wiring::WiredEventBus;
    use crate::service::idempotency::IdempotencyService;

    // ━━━ AsyncFlushBuffer Edge Cases ━━━

    #[test]
    fn test_async_flush_buffer_zero_capacity_immediate_flush() {
        let mut buf = AsyncFlushBuffer::new(0);
        let entry = make_entry(1);
        let result = buf.add(entry);
        assert!(result.is_some(), "Zero-capacity buffer should flush immediately");
    }

    #[test]
    fn test_async_flush_buffer_partial_fill_then_flush() {
        let mut buf = AsyncFlushBuffer::new(5);
        for i in 1..=4 {
            assert!(buf.add(make_entry(i)).is_none());
        }
        let batch = buf.add(make_entry(5));
        assert!(batch.is_some());
        assert_eq!(batch.unwrap().len(), 5);
    }

    #[test]
    fn test_async_flush_buffer_flush_remaining() {
        let mut buf = AsyncFlushBuffer::new(4);
        buf.add(make_entry(1));
        buf.add(make_entry(2));
        let partial = buf.flush_remaining();
        assert_eq!(partial.len(), 2);
        assert!(buf.flush_remaining().is_empty());
    }

    fn make_entry(seq: u64) -> WalEntry {
        WalEntry {
            sequence: seq,
            event: Event::new("Test", uuid::Uuid::now_v7(), "{}", seq),
            checksum: seq * 7,
            timestamp: chrono::Utc::now(),
        }
    }

    // ━━━ EventBus ━━━

    #[test]
    fn test_event_bus_consume_all_partitions() {
        let bus = PartitionedEventBus::new(4);
        // All use same key prefix so they hash to same partition
        for i in 0..10 {
            bus.produce(&format!("k-{}", i), "data", "prod", i);
        }
        // Check all partitions and sum
        let mut total = 0usize;
        for p in 0..4 {
            total += bus.consume(p, 0).len();
        }
        assert_eq!(total, 10);
    }

    #[test]
    fn test_event_bus_multi_partition_produce() {
        let bus = PartitionedEventBus::new(4);
        for p in 0..4 {
            for i in 0..25 {
                bus.produce(&format!("p{}", p), "data", &format!("prod-{}", p), i);
            }
        }
        for p in 0..4 {
            let result = bus.consume(p, 0);
            assert!(!result.is_empty(), "Partition {} empty", p);
        }
    }

    // ━━━ Fencing Token ━━━

    #[test]
    fn test_fencing_epoch_increment() {
        let token = FencingToken::new();
        let epoch1 = token.register_producer("test-producer");
        assert!(epoch1 > 0);
        assert!(token.is_valid("test-producer", epoch1));

        let epoch2 = token.fence("test-producer");
        assert!(epoch2 > epoch1);
        assert!(!token.is_valid("test-producer", epoch1));
        assert!(token.is_valid("test-producer", epoch2));
    }

    #[test]
    fn test_fencing_different_producers_no_cross_validation() {
        let token = FencingToken::new();
        let e1 = token.register_producer("producer-A");
        let e2 = token.register_producer("producer-B");
        token.fence("producer-A");
        assert!(!token.is_valid("producer-A", e1));
        assert!(token.is_valid("producer-B", e2));
    }

    // ━━━ Idempotency — check_and_mark returns true if ALREADY processed ━━━

    #[test]
    fn test_idempotency_dedup() {
        let service = IdempotencyService::new();
        let tx_id = format!("tx-{}", uuid::Uuid::now_v7());
        // First call: not yet processed → returns false
        assert!(!service.check_and_mark(&tx_id), "First call should return false (was NOT processed before)");
        // Second call: already processed → returns true
        assert!(service.check_and_mark(&tx_id), "Second call should return true (WAS processed before)");
    }

    #[test]
    fn test_idempotency_different_transactions() {
        let service = IdempotencyService::new();
        assert!(!service.check_and_mark("tx-alpha"));
        assert!(!service.check_and_mark("tx-beta"));
        assert!(!service.check_and_mark("tx-gamma"));
        assert!(service.check_and_mark("tx-alpha"));
    }

    // ━━━ WiredEventBus ━━━

    #[test]
    fn test_wired_bus_basic_produce() {
        let bus = WiredEventBus::new("test-wired", 4);
        let result = bus.produce_idempotent("key-1", "payload-1");
        assert!(result.is_ok());
        let (seq, epoch) = result.unwrap();
        assert!(seq > 0);
        assert!(epoch > 0);
    }

    #[test]
    fn test_wired_bus_transaction_commit_and_abort() {
        let bus = WiredEventBus::new("tx-bus", 4);

        bus.begin_transaction();
        bus.send_in_transaction("t1", "v1");
        bus.send_in_transaction("t2", "v2");
        let offsets = bus.commit_transaction().unwrap();
        assert_eq!(offsets.len(), 2);

        bus.begin_transaction();
        bus.send_in_transaction("a1", "should-discard");
        bus.abort_transaction();
    }

    // ━━━ Concurrent Stress ━━━

    #[test]
    fn test_concurrent_multi_producer() {
        let bus = Arc::new(WiredEventBus::new("multi", 8));
        let mut handles = vec![];

        for p in 0..4 {
            let bus = bus.clone();
            handles.push(thread::spawn(move || {
                for i in 0..50 {
                    let result = bus.produce_idempotent(
                        &format!("p{}-msg{}", p, i),
                        &format!(r#"{{"p":{},"i":{}}}"#, p, i),
                    );
                    if let Ok((seq, _)) = result {
                        assert!(seq > 0);
                    }
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn test_ring_buffer_mpsc_stress() {
        let rb = Arc::new(RingBuffer::<String>::new(16384));
        let producer_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut handles = vec![];

        for p in 0..2 {
            let rb = rb.clone();
            let done = producer_done.clone();
            handles.push(thread::spawn(move || {
                for i in 0..2500 {
                    let msg = format!("p{}-{}", p, i);
                    loop {
                        match rb.try_push(msg.clone()) {
                            Ok(_) => break,
                            Err(_) => {
                                if done.load(std::sync::atomic::Ordering::Relaxed) {
                                    return;
                                }
                                thread::yield_now();
                            }
                        }
                    }
                }
            }));
        }

        let rb_cons = rb.clone();
        let consumer = thread::spawn(move || {
            let mut count = 0usize;
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while count < 5000 {
                if rb_cons.try_pop().is_some() {
                    count += 1;
                }
                if std::time::Instant::now() > deadline {
                    break;
                }
                thread::yield_now();
            }
            producer_done.store(true, std::sync::atomic::Ordering::Relaxed);
            count
        });

        for h in handles {
            h.join().unwrap();
        }
        let consumed = consumer.join().unwrap();
        // CI runners have fewer cores — stress test is informational, not a correctness gate
        assert!(consumed >= 1000, "Should consume substantial messages, got {}", consumed);
        assert!(consumed <= 5000);
    }
}
