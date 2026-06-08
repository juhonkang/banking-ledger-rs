//! Performance benchmarks for critical financial paths.
//! Run with: cargo bench (requires nightly) or cargo test --release

#[cfg(test)]
mod benches {
    use crate::domain::account::{Account, AccountType};
    use crate::domain::journal::{EntryLeg, JournalEntry};
    use crate::log::hash_chain::HashChain;
    use crate::log::ring_buffer::RingBuffer;
    use std::sync::Arc;
    use std::time::Instant;

    const WARMUP_ITERS: usize = 100;
    const BENCH_ITERS: usize = 10_000;

    #[test]
    fn bench_account_debit_credit_throughput() {
        let acc = Account::new(AccountType::Asset, "USD", 1_000_000_000, None);
        // Warmup
        for _ in 0..WARMUP_ITERS {
            acc.debit(100).unwrap();
            acc.credit(100).unwrap();
        }
        let start = Instant::now();
        for _ in 0..BENCH_ITERS {
            acc.debit(100).unwrap();
            acc.credit(100).unwrap();
        }
        let elapsed = start.elapsed();
        let ops_per_sec = (BENCH_ITERS * 2) as f64 / elapsed.as_secs_f64();
        println!("Account debit+credit: {:.0} ops/sec ({}µs/op)", ops_per_sec, elapsed.as_micros() as f64 / (BENCH_ITERS * 2) as f64);
    }

    #[test]
    fn bench_hash_chain_append_throughput() {
        let mut chain = HashChain::new(b"bench-key");
        let start = Instant::now();
        for i in 0..BENCH_ITERS {
            chain.append(&format!("block-{}", i));
        }
        let elapsed = start.elapsed();
        println!("HashChain append: {} blocks in {:?} ({:.0} blocks/sec)",
            BENCH_ITERS, elapsed, BENCH_ITERS as f64 / elapsed.as_secs_f64());
    }

    #[test]
    fn bench_hash_chain_verify() {
        let mut chain = HashChain::new(b"bench-key");
        for i in 0..1_000 {
            chain.append(&format!("data-{}", i));
        }
        let start = Instant::now();
        for _ in 0..100 {
            let (valid, _) = chain.verify_chain();
            assert!(valid);
        }
        let elapsed = start.elapsed();
        println!("HashChain verify (1000 blocks × 100): {:?} ({:.0} verifies/sec)",
            elapsed, 100.0 / elapsed.as_secs_f64());
    }

    #[test]
    fn bench_ring_buffer_throughput() {
        let rb = RingBuffer::<u64>::new(1024);
        let start = Instant::now();
        for i in 0..BENCH_ITERS {
            rb.try_push(i as u64).unwrap();
            rb.try_pop().unwrap();
        }
        let elapsed = start.elapsed();
        let ops = BENCH_ITERS * 2;
        println!("RingBuffer push+pop: {:.0} ops/sec", ops as f64 / elapsed.as_secs_f64());
    }

    #[test]
    fn bench_journal_entry_creation() {
        let a = uuid::Uuid::now_v7();
        let b = uuid::Uuid::now_v7();
        let start = Instant::now();
        for _ in 0..BENCH_ITERS {
            let debit = EntryLeg::debit(a, 100);
            let credit = EntryLeg::credit(b, 100);
            let _entry = JournalEntry::new(uuid::Uuid::now_v7(), 1, vec![debit, credit], "bench").unwrap();
        }
        let elapsed = start.elapsed();
        println!("JournalEntry create: {:.0} entries/sec", BENCH_ITERS as f64 / elapsed.as_secs_f64());
    }

    #[test]
    fn bench_concurrent_account_stress() {
        let acc = Arc::new(Account::new(AccountType::Asset, "USD", 1_000_000, None));
        let threads = 8;
        let iters_per_thread = 10_000;
        let mut handles = vec![];
        let start = Instant::now();
        for _ in 0..threads {
            let acc = acc.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..iters_per_thread {
                    if i % 2 == 0 {
                        let _ = acc.debit(1);
                    } else {
                        let _ = acc.credit(1);
                    }
                }
            }));
        }
        for h in handles { h.join().unwrap(); }
        let elapsed = start.elapsed();
        let total_ops = threads * iters_per_thread;
        println!("Concurrent stress ({} threads × {} ops): {:.0} ops/sec",
            threads, iters_per_thread, total_ops as f64 / elapsed.as_secs_f64());
        println!("Final balance: {} cents", acc.balance_cents());
    }
}
