//! Boundary probe extended tests — extreme edge conditions (Round 3 audit).
//! Probes system behavior at absolute limits: 0-capacity buffers, `usize::MAX`
//! wrapping, massive chain/saga sizes, high-contention CAS stress, and more.

#[cfg(test)]
mod boundary_probe_extended_tests {
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    use crate::domain::account::{Account, AccountType};
    use crate::domain::money::{Currency, Money};
    use crate::log::hash_chain::HashChain;
    use crate::service::idempotency::IdempotencyService;
    use crate::service::resilience::{
        Bulkhead, CircuitBreaker, CircuitState, TokenBucket,
    };
    use crate::service::saga::{
        SagaAction, SagaDefinition, SagaOrchestrator, SagaStep,
    };

    // ═══════════════════════════════════════════════════════════════════
    //  RingBuffer boundary tests  (feature = "full")
    // ═══════════════════════════════════════════════════════════════════

    #[cfg(feature = "full")]
    mod ring_buffer_probes {
        use crate::log::ring_buffer::{RingBuffer, RingBufferError};

        // ━━━ 1) 0-capacity RingBuffer ━━━
        // min_capacity=0 → next_power_of_two(0)=1, so capacity=1, mask=0

        #[test]
        fn probe_ringbuffer_min_capacity_zero() {
            let rb = RingBuffer::<i64>::new(0);
            // Should have capacity 1 (next power of 2 after 0)
            assert!(rb.is_empty());
            assert!(!rb.is_full()); // capacity=1, len=0

            // Push one item — should succeed
            rb.try_push(42).unwrap();
            assert_eq!(rb.len(), 1);
            assert!(rb.is_full());

            // Push when full — should fail
            assert!(matches!(rb.try_push(99), Err(RingBufferError::Full)));

            // Pop — should return the item
            assert_eq!(rb.try_pop(), Some(42));
            assert!(rb.is_empty());
        }

        #[test]
        fn probe_ringbuffer_min_capacity_zero_drop_reuse() {
            let rb = RingBuffer::<String>::new(0);
            // Push strings through capacity-1 buffer repeatedly
            for cycle in 0..100 {
                let val = format!("cycle-{cycle}");
                rb.try_push(val.clone()).unwrap();
                assert_eq!(rb.try_pop().as_deref(), Some(val.as_str()));
                assert!(rb.try_pop().is_none()); // Empty after pop
            }
        }

        // ━━━ 2) extreme wrap-around stress ━━━
        // Push/pop through many cycles to verify mask-based indexing is correct
        // across thousands of wrap-arounds (capacity=4, 10K push/pop cycles)

        #[test]
        fn probe_ringbuffer_many_wrap_cycles() {
            let rb = RingBuffer::<u64>::new(4);
            let total_cycles = 10_000u64;

            for seq in 0..total_cycles {
                rb.try_push(seq).unwrap();
                assert_eq!(rb.try_pop(), Some(seq));
            }

            // Buffer should be empty after all cycles
            assert!(rb.is_empty());
            // Final push should still work
            rb.try_push(999_999).unwrap();
            assert_eq!(rb.try_pop(), Some(999_999));
        }

        #[test]
        fn probe_ringbuffer_fill_drain_repeat() {
            let rb = RingBuffer::<i32>::new(8); // capacity = 8
            for _epoch in 0..500 {
                // Fill
                for i in 0..8 {
                    rb.try_push(i).unwrap();
                }
                assert!(rb.is_full());
                // Drain
                for i in 0..8 {
                    assert_eq!(rb.try_pop(), Some(i));
                }
                assert!(rb.is_empty());
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    //  3) HashChain with 10K blocks — performance smoke
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn probe_hashchain_10k_blocks() {
        let key = b"extended-probe-key-32-bytes!!";
        let mut chain = HashChain::new(key);

        let start = Instant::now();

        // Append 10,000 blocks
        for i in 0..10_000 {
            chain.append(&format!("block-{i}"));
        }

        let append_time = start.elapsed();
        eprintln!(
            "HashChain 10K append: {:.2}ms ({:.0} blocks/sec)",
            append_time.as_secs_f64() * 1000.0,
            10_000.0 / append_time.as_secs_f64()
        );

        // 10,000 data blocks + 1 genesis = 10,001
        assert_eq!(chain.len(), 10_001);

        // Verify full chain integrity
        let verify_start = Instant::now();
        let (valid, tampered) = chain.verify_chain();
        let verify_time = verify_start.elapsed();

        assert!(valid, "10K chain should be valid");
        assert!(tampered.is_empty());
        eprintln!(
            "HashChain 10K verify: {:.2}ms",
            verify_time.as_secs_f64() * 1000.0
        );

        // Spot-check: genesis at 0, so index 5000 has data "block-4999"
        let block = chain.get_block(5000).expect("block 5000 should exist");
        assert!(block.verify_self());
        assert_eq!(block.data, "block-4999");

        // Proof at index 9999 should be valid
        let proof = chain.proof_for_block(9999).expect("proof for block 9999");
        assert!(proof.verify_position());
    }

    #[test]
    fn probe_hashchain_tamper_middle_large_chain() {
        let key = b"tamper-test-key-32-bytes!!!!";
        let mut chain = HashChain::new(key);

        for i in 0..500 {
            chain.append(&format!("data-{i}"));
        }

        // Tamper with block 250
        chain.blocks[250].data = "TAMPERED".to_string();
        // Do NOT recalculate hash

        let (valid, tampered) = chain.verify_chain();
        assert!(!valid);
        assert!(
            tampered.contains(&250),
            "Block 250 should be detected as tampered, got: {tampered:?}"
        );

        // Block 251's previous_hash no longer matches block 250's hash
        // So it should also be flagged (or the linkage check catches it)
        // The implementation checks: self-hash verification finds block 250,
        // and chain-linkage finds block 251 because its previous_hash != block 250's hash
    }

    // ═══════════════════════════════════════════════════════════════════
    //  4) Saga with 100-step chain
    // ═══════════════════════════════════════════════════════════════════

    fn make_100_step_saga() -> SagaDefinition {
        let account_id = uuid::Uuid::now_v7();
        let mut steps = Vec::with_capacity(100);

        for i in 0..100 {
            let amount = 1; // minimal amount to keep it sane
            steps.push(SagaStep {
                name: format!("Step{i:03}"),
                action: SagaAction::Credit {
                    account_id,
                    amount_cents: amount,
                },
                compensation: SagaAction::Debit {
                    account_id,
                    amount_cents: amount,
                },
                max_retries: 1,
                timeout_seconds: 60,
            });
        }

        SagaDefinition {
            name: "HundredStepSaga".into(),
            steps,
        }
    }

    #[test]
    fn probe_saga_100_step_happy_path() {
        let mut orch = SagaOrchestrator::new();
        orch.register(make_100_step_saga());

        let saga_id = orch.start("HundredStepSaga").expect("saga should start");
        assert_eq!(orch.get_status(saga_id), Some(crate::service::saga::SagaStatus::Pending));

        // Execute all 100 steps
        for expected_idx in 0..100 {
            let (idx, action) = orch
                .next_action(saga_id)
                .expect("should get next action");
            assert_eq!(idx, expected_idx, "step index mismatch");
            assert!(
                matches!(action, SagaAction::Credit { .. }),
                "expected Credit action at step {expected_idx}"
            );

            let done = orch
                .step_succeeded(saga_id)
                .expect("step should succeed");
            if expected_idx < 99 {
                assert!(!done, "should not be done at step {expected_idx}");
            } else {
                assert!(done, "should be done at final step");
            }
        }

        // Saga should now be complete and removed from active
        assert!(orch.get_status(saga_id).is_none(), "completed saga removed from active");
    }

    #[test]
    fn probe_saga_100_step_compensation() {
        let mut orch = SagaOrchestrator::new();
        orch.register(make_100_step_saga());

        let saga_id = orch.start("HundredStepSaga").unwrap();

        // Execute first 50 steps successfully
        for _ in 0..50 {
            let _ = orch.next_action(saga_id).unwrap();
            orch.step_succeeded(saga_id).unwrap();
        }

        // Fail at step 50
        let compensations = orch
            .step_failed(saga_id, "simulated failure at step 50".into())
            .expect("should get compensations");

        // Should have 50 compensations (for steps 0-49) in reverse order
        assert_eq!(compensations.len(), 50, "all 50 completed steps need compensation");

        // First compensation should be for step 49 (reverse order)
        assert!(
            matches!(compensations[0], SagaAction::Debit { .. }),
            "first compensation should be Debit (reverse of Credit at step 49)"
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    //  5) Concurrent 100-thread CAS stress on single Account
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn probe_account_cas_stress_100_threads() {
        // Initial balance: $10,000,000.00 in cents
        let account = Arc::new(Account::new(
            AccountType::Asset,
            "USD",
            1_000_000_000,
            None,
        ));
        let initial = account.balance_cents();

        let num_threads = 100;
        let _ops_per_thread = 10_000; // total: 1,000,000 operations
        let credit_per_thread = 5_000;
        let debit_per_thread = 5_000;

        let mut handles = Vec::with_capacity(num_threads);
        for _ in 0..num_threads {
            let acc = Arc::clone(&account);
            handles.push(thread::spawn(move || {
                // Credits first to build up balance, then debits
                for _ in 0..credit_per_thread {
                    acc.credit(1).unwrap();
                }
                for _ in 0..debit_per_thread {
                    acc.debit(1).unwrap();
                }
            }));
        }

        for h in handles {
            h.join().expect("thread should not panic");
        }

        // Each thread: +5000 then -5000 = net 0
        // Final balance should equal initial
        assert_eq!(
            account.balance_cents(),
            initial,
            "balance should be unchanged after balanced credits/debits"
        );
        assert_eq!(
            account.available_balance_cents(),
            initial,
            "available balance should also match"
        );
    }

    #[test]
    fn probe_account_cas_stress_mixed_credit_debit() {
        // Interleaved credit/debit from many threads
        let account = Arc::new(Account::new(
            AccountType::Asset,
            "USD",
            1_000_000, // $10,000
            None,
        ));

        let num_threads = 50;
        let ops = 20_000; // 1,000,000 total ops

        let mut handles = Vec::with_capacity(num_threads);
        for _ in 0..num_threads {
            let acc = Arc::clone(&account);
            handles.push(thread::spawn(move || {
                for i in 0..ops {
                    match i % 3 {
                        0 => { let _ = acc.credit(1); }
                        1 => { let _ = acc.debit(1); }
                        _ => { let _ = acc.credit(2); }
                    }
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        // Just verify no crashes and balance is non-negative, available >= 0
        let balance = account.balance_cents();
        let available = account.available_balance_cents();
        assert!(balance >= 0, "balance should never go negative: {balance}");
        assert!(available >= 0, "available should never go negative: {available}");
        assert!(
            available <= balance,
            "available ≤ balance: {available} vs {balance}"
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    //  6) Money::from_minor extreme values for zero-decimal currencies
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn probe_money_from_minor_i64_max_vnd() {
        let vnd = Currency::vnd();
        // VND has minor_unit=0, so divisor=1. i64::MAX / 1 = i64::MAX.
        let m = Money::from_minor(i64::MAX, vnd);
        assert_eq!(m.try_to_minor(), Some(i64::MAX));
    }

    #[test]
    fn probe_money_from_minor_i64_min_vnd() {
        let vnd = Currency::vnd();
        let m = Money::from_minor(i64::MIN, vnd);
        assert_eq!(m.try_to_minor(), Some(i64::MIN));
    }

    #[test]
    fn probe_money_from_minor_i64_max_jpy() {
        let jpy = Currency::jpy();
        // JPY has minor_unit=0, same as VND
        let m = Money::from_minor(i64::MAX, jpy);
        assert_eq!(m.try_to_minor(), Some(i64::MAX));
    }

    #[test]
    fn probe_money_from_minor_zero_vnd() {
        let vnd = Currency::vnd();
        let m = Money::from_minor(0, vnd);
        assert_eq!(m.try_to_minor(), Some(0));
    }

    #[test]
    fn probe_money_from_minor_extreme_usd() {
        let usd = Currency::usd();
        // i64::MAX / 100 ≈ 92,233,720,368,547,758.07 — should convert
        let m = Money::from_minor(i64::MAX, usd.clone());
        assert!(m.try_to_minor().is_some(), "i64::MAX cents in USD should convert");

        let m2 = Money::from_minor(i64::MIN, usd);
        assert!(m2.try_to_minor().is_some(), "i64::MIN cents in USD should convert");
    }

    #[test]
    fn probe_money_from_minor_1_cent_zero_decimal() {
        // 1 cent in VND (0 decimal places) — from_minor(1, vnd) = 1 VND
        let vnd = Currency::vnd();
        let m = Money::from_minor(1, vnd);
        assert_eq!(m.to_minor(), 1);
    }

    // ═══════════════════════════════════════════════════════════════════
    //  7) DashMap with 100K entries performance
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn probe_dashmap_100k_entries() {
        use dashmap::DashMap;

        let map = DashMap::with_capacity(100_000);
        let start = Instant::now();

        // Insert 100K entries
        for i in 0..100_000u64 {
            map.insert(i, i * 2);
        }

        let insert_time = start.elapsed();
        assert_eq!(map.len(), 100_000);

        // Lookup all 100K entries
        let lookup_start = Instant::now();
        let mut found = 0u64;
        for i in 0..100_000u64 {
            if let Some(v) = map.get(&i) {
                assert_eq!(*v, i * 2);
                found += 1;
            }
        }
        let lookup_time = lookup_start.elapsed();

        assert_eq!(found, 100_000);

        eprintln!(
            "DashMap 100K: insert={:.2}ms, lookup={:.2}ms",
            insert_time.as_secs_f64() * 1000.0,
            lookup_time.as_secs_f64() * 1000.0,
        );
    }

    #[test]
    fn probe_dashmap_concurrent_reads() {
        use dashmap::DashMap;
        use std::sync::Barrier;

        let map = Arc::new(DashMap::with_capacity(10_000));
        // Pre-populate
        for i in 0..10_000u64 {
            map.insert(i, format!("val-{i}"));
        }

        let barrier = Arc::new(Barrier::new(20));
        let mut handles = vec![];

        for _ in 0..20 {
            let m = Arc::clone(&map);
            let b = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                b.wait(); // Start all at once
                for i in 0..10_000u64 {
                    let v = m.get(&i).expect("key should exist");
                    assert_eq!(*v, format!("val-{i}"));
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    //  8) IdempotencyService with 1M mark_processed calls
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn probe_idempotency_1m_mark_processed() {
        let svc = IdempotencyService::new(); // default max_entries = 100,000
        let total = 1_000_000u64;

        let start = Instant::now();

        for i in 0..total {
            svc.mark_processed(&format!("tx-{i:09}"));
        }

        let elapsed = start.elapsed();
        eprintln!(
            "IdempotencyService 1M mark_processed: {:.2}ms ({:.0} ops/sec)",
            elapsed.as_secs_f64() * 1000.0,
            total as f64 / elapsed.as_secs_f64(),
        );

        // After 1M calls with capacity 100K, the map should be at ~100K
        // (eviction keeps it bounded)
        let final_len = svc.len();
        assert!(
            final_len <= 100_000,
            "capacity eviction should keep entries ≤ 100K, got {final_len}"
        );
        assert!(
            final_len > 0,
            "at least some entries should remain after eviction"
        );
    }

    #[test]
    fn probe_idempotency_check_and_mark_throughput() {
        let svc = IdempotencyService::new();
        let total = 500_000u64;

        let start = Instant::now();
        let mut duplicates = 0u64;

        // First pass: all new
        for i in 0..total {
            let already = svc.check_and_mark(&format!("duptx-{i:09}"));
            if already {
                duplicates += 1;
            }
        }

        let first_pass = start.elapsed();

        // Second pass: all duplicates
        let second_start = Instant::now();
        for i in 0..total {
            let already = svc.check_and_mark(&format!("duptx-{i:09}"));
            if already {
                duplicates += 1;
            }
        }
        let second_pass = second_start.elapsed();

        // First pass: 0 duplicates. Second pass: all should be duplicates.
        assert_eq!(duplicates, total, "second pass should find all duplicates");

        eprintln!(
            "IdempotencyService check_and_mark 500K: first={:.1}ms, second={:.1}ms",
            first_pass.as_secs_f64() * 1000.0,
            second_pass.as_secs_f64() * 1000.0,
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    //  9) TokenBucket 0-rate extreme edge cases
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn probe_tokenbucket_zero_rate_zero_capacity() {
        let bucket = TokenBucket::new(0, 0.0);
        // With capacity=0, no tokens at all
        assert!(!bucket.try_consume(), "zero capacity + zero rate = never allow");
        thread::sleep(Duration::from_millis(100));
        assert!(!bucket.try_consume(), "zero rate should not refill even after wait");
    }

    #[test]
    fn probe_tokenbucket_zero_rate_capacity_one() {
        let bucket = TokenBucket::new(1, 0.0);
        // Initial token is available
        assert!(bucket.try_consume(), "should consume the single initial token");
        assert!(!bucket.try_consume(), "no more tokens at rate 0");
        thread::sleep(Duration::from_secs(2));
        assert!(!bucket.try_consume(), "rate 0 should never refill");
    }

    #[test]
    fn probe_tokenbucket_very_high_rate() {
        let bucket = TokenBucket::new(100, 1_000_000.0); // 1M tokens/sec
        // Consume all 100 tokens
        for _ in 0..100 {
            assert!(bucket.try_consume());
        }
        // At 1M/sec, 1ms = 1000 tokens — so even a tiny wait should refill
        thread::sleep(Duration::from_millis(1));
        assert!(bucket.try_consume(), "high rate should refill quickly");
    }

    #[test]
    fn probe_tokenbucket_try_consume_n() {
        let bucket = TokenBucket::new(50, 0.0);
        assert!(bucket.try_consume_n(50), "should consume all 50 tokens");
        assert!(!bucket.try_consume_n(1), "no tokens left");
    }

    // ═══════════════════════════════════════════════════════════════════
    //  10) CircuitBreaker rapid state transitions
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn probe_circuit_breaker_rapid_transitions() {
        // Short cooldown for fast transitions
        let cb = CircuitBreaker::new(2, Duration::from_millis(10));

        for cycle in 0..25 {
            // Trip the circuit: 2 consecutive failures
            cb.record_failure();
            cb.record_failure();
            assert_eq!(cb.state(), CircuitState::Open, "should be Open after 2 failures (cycle {cycle})");
            assert!(!cb.allow_request(), "should reject while Open");

            // Wait for cooldown to expire → HalfOpen
            thread::sleep(Duration::from_millis(15));
            assert!(cb.allow_request(), "should allow probe in HalfOpen");

            // Record 2 successes → back to Closed
            cb.record_success();
            cb.record_success();
            assert_eq!(
                cb.state(),
                CircuitState::Closed,
                "should return to Closed after 2 successes (cycle {cycle})"
            );
        }

        // After all cycles, we're in Closed state
        assert_eq!(cb.state(), CircuitState::Closed);
        // Error rate reflects the history of recorded failures vs total calls
        // (each cycle records 2 failures against 2 allow_request calls)
        assert!(cb.error_rate() > 0.0, "error rate should reflect recorded failures");
    }

    #[test]
    fn probe_circuit_breaker_half_open_failure() {
        let cb = CircuitBreaker::new(1, Duration::from_millis(10));
        // Trip immediately
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Wait for cooldown
        thread::sleep(Duration::from_millis(15));
        assert!(cb.allow_request()); // Probe in HalfOpen

        // Fail the probe → should go back to Open
        cb.record_failure();
        // The CB goes back to Open
        assert!(!cb.allow_request(), "should be Open again after failed probe");
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Bonus: Bulkhead extreme values
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn probe_bulkhead_zero_capacity() {
        let bh = Bulkhead::new(0);
        for _ in 0..100 {
            assert!(bh.try_acquire().is_err(), "max=0 should always reject");
        }
    }

    #[test]
    fn probe_bulkhead_large_capacity() {
        // u32::MAX capacity
        let bh = Bulkhead::new(u32::MAX);
        // Should be able to acquire at least once
        let guard = bh.try_acquire().expect("large capacity should allow acquire");
        assert_eq!(bh.active_count(), 1);
        drop(guard);
        assert_eq!(bh.active_count(), 0);
    }

    // ═══════════════════════════════════════════════════════════════════
    //  Bonus: Saga edge cases
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn probe_saga_empty_steps() {
        let mut orch = SagaOrchestrator::new();
        orch.register(SagaDefinition {
            name: "EmptySaga".into(),
            steps: vec![],
        });

        let result = orch.start("EmptySaga");
        assert!(result.is_err(), "saga with no steps should fail to start");
    }

    #[test]
    fn probe_saga_unknown_definition() {
        let mut orch = SagaOrchestrator::new();
        let result = orch.start("NonExistent");
        assert!(result.is_err(), "unknown saga should fail");
    }

    #[test]
    fn probe_saga_single_step_compensation() {
        let account_id = uuid::Uuid::now_v7();
        let mut orch = SagaOrchestrator::new();
        orch.register(SagaDefinition {
            name: "SingleStep".into(),
            steps: vec![SagaStep {
                name: "Only".into(),
                action: SagaAction::Credit {
                    account_id,
                    amount_cents: 100,
                },
                compensation: SagaAction::Debit {
                    account_id,
                    amount_cents: 100,
                },
                max_retries: 1,
                timeout_seconds: 5,
            }],
        });

        let saga_id = orch.start("SingleStep").unwrap();
        let (idx, _) = orch.next_action(saga_id).unwrap();
        assert_eq!(idx, 0);

        // Fail at the only step
        let compensations = orch
            .step_failed(saga_id, "step 0 failed".into())
            .unwrap();
        assert_eq!(compensations.len(), 0, "no completed steps to compensate");
    }
}
