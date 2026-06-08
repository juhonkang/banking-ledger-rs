# Concurrency Model

## Lock Hierarchy

```
Account.status: Mutex<AccountStatus>        — cold path (status changes)
Account.balance: AtomicI64 (CAS loop)        — hot path (debit/credit)
Account.available_balance: AtomicI64 (CAS)   — hot path (hold/debit)
AccountService.accounts: DashMap             — shard-level locking
IdentityService.parties: RwLock<HashMap>     — read-heavy
LedgerService.accounts: RwLock<HashMap>      — (dead code, needs wiring)
SagaOrchestrator.instances: DashMap           — concurrent saga ops
IdempotencyService.processed: DashMap         — dedup across consumers
```

## Memory Ordering: SeqCst

All financial operations use `Ordering::SeqCst`. Rationale:
- x86-64: SeqCst is free on loads (TSO already provides it), store barrier cost negligible
- ARM/POWER: SeqCst prevents store/load reordering that Acquire/Release allows
- Financial correctness > 5% perf difference
- Exception: RingBuffer internal counters use Relaxed+Acquire/Release (performance-critical, single-producer invariant)

## Deadlock Prevention

1. No lock nesting across modules
2. DashMap avoids lock contention via sharding
3. Saga steps are sequential within instance (no concurrent step execution)
4. Choreography uses event passing — no shared locks between services

## Known Concurrency Risks

1. **Account CAS window** (account.rs:248-251): available_balance CAS succeeds then balance fetch_sub — nanosecond inconsistency window
2. **Credit double-CAS** (account.rs:286-305): balance CAS then available_balance CAS — phantom held amount visible during window
3. **Identity lock/re-lock** (identity_service.rs:47-69): read→drop→write — party could change between locks
4. **Saga definition race** (saga.rs:177): definition lookup without holding lock across calls
