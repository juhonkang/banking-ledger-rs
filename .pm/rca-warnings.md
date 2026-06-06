# RCA: 189 Dead-Code Warnings

## Root Cause

The banking-ledger-rs was built **module-by-module** incrementally across multiple domains. Each module was:
- ✅ Designed independently
- ✅ Tested with unit tests (168 pass)
- ❌ Never integrated into the core API server

Result: `src/main.rs` only uses ~10% of the codebase. The API server calls `api::serve()` which only touches `domain::account` and `service::resilience::GoldenSignals`. All other modules exist but are **orphaned** — compiled, tested, but unreachable from the binary.

## Gap Analysis

| Module | Has Tests? | Used by API? | Gap |
|--------|-----------|-------------|-----|
| `domain::account` | ✅ 38 tests | ✅ debit/credit/status | None — fully integrated |
| `domain::journal` | ✅ 8 tests | ❌ | Journal entries not created on transfer |
| `domain::money` | ✅ 8 tests | ❌ | API uses raw i64, not Money type |
| `domain::coa` | ✅ 4 tests | ❌ | No COA validation on account creation |
| `domain::party` | ✅ via identity_svc | ❌ | No Party/Identifier in API |
| `log::hash_chain` | ✅ 9 tests | ❌ | No hash chain for audit trail |
| `log::event_log` | ✅ 5 tests | ❌ | No WAL/event sourcing |
| `log::ring_buffer` | ✅ 6 tests | ❌ | No high-throughput buffer |
| `log::event_bus` | ✅ 6 tests | ❌ | No partitioned event bus |
| `service::saga` | ✅ 4 tests | ❌ | No saga for complex transfers |
| `service::resilience` | ✅ 13 tests | ⚠️ partial | CircuitBreaker wired, rest not |
| `service::advanced` | ✅ 6 tests | ❌ | DeadlockDetector, LatencyHistogram unused |
| `service::production` | ✅ 4 tests | ❌ | StressTest, PostMortem, Security unused |
| `store::surrealdb` | ✅ 4 tests | ❌ | No persistence — all in-memory |

## Solution

**Integrate, don't suppress.** Wire the orphaned modules into the API server so they're actually used:

1. **Journal → Transfer endpoint**: Create JournalEntry on every transfer
2. **Money → All endpoints**: Replace raw i64 with Money type
3. **HashChain → Audit**: Append journal entries to hash chain
4. **GoldenSignals → Metrics**: Already partially done, complete it
5. **TokenBucket → Rate Limit**: Add rate limiting middleware
6. **SagaOrchestrator → Complex Ops**: Use for multi-step transfers
7. **SurrealDB → Persistence**: Persist accounts to SurrealDB on create

## What Gets Removed

Modules that are design patterns / reference implementations (not runtime dependencies):
- `log::ring_buffer` — reference implementation, not needed at runtime
- `log::event_bus` — reference, use in-process channels instead
- `service::advanced` — design patterns, use when needed
- `service::production` — development tools, not runtime

These stay as `#[cfg(test)]` or `#[cfg(feature = "full")]` modules.
