# Banking Ledger — Rust Architecture

> High-throughput financial ledger core. 100M RPS ready. Immutable. Double-entry. Hash-chain verified.

## Table of Contents

1. [System Architecture](#system-architecture)
2. [Data Flow](#data-flow)
3. [API Design](#api-design)
4. [Database Schema](#database-schema)
5. [Concurrency Model](#concurrency-model)
6. [Security Model](#security-model)
7. [Deployment](#deployment)

---

## System Architecture

```
┌─────────────────────────────────────────────────────────┐
│                      CLIENT                             │
│              HTTP REST (JSON over TLS)                   │
└─────────────────────┬───────────────────────────────────┘
                      │
┌─────────────────────▼───────────────────────────────────┐
│                   API LAYER                              │
│  axum :: Router                                         │
│  ┌──────────┬──────────┬──────────┬──────────────────┐  │
│  │ /health  │/accounts │/transfers│ /admin/metrics   │  │
│  │ GET      │ CRUD     │ POST     │ GET              │  │
│  └──────────┴──────────┴──────────┴──────────────────┘  │
│  Middleware: CircuitBreaker, GoldenSignals, RateLimit   │
└─────────────────────┬───────────────────────────────────┘
                      │
┌─────────────────────▼───────────────────────────────────┐
│                 SERVICE LAYER                            │
│  ┌─────────────┐  ┌──────────────┐  ┌────────────────┐  │
│  │ LedgerSvc   │  │ AccountSvc   │  │ IdentitySvc    │  │
│  │ (double     │  │ (CAS balance │  │ (Party +       │  │
│  │  entry)     │  │  DashMap)    │  │  Identifier)   │  │
│  └──────┬──────┘  └──────┬───────┘  └────────────────┘  │
│         │                │                               │
│  ┌──────┴────────────────┴──────────────────────────┐   │
│  │              Concurrency Arsenal                  │   │
│  │  CAS loop · Condvar · FairQueue · Bulkhead       │   │
│  │  CircuitBreaker · TokenBucket · ChaosAgent       │   │
│  └──────────────────────────────────────────────────┘   │
└─────────────────────┬───────────────────────────────────┘
                      │
┌─────────────────────▼───────────────────────────────────┐
│                  DOMAIN LAYER                            │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐   │
│  │ Account  │ │ Journal  │ │ Money    │ │ Party    │   │
│  │AtomicI64 │ │Entry(legs│ │Decimal+  │ │UUID v7   │   │
│  │ CAS bal  │ │ immutable│ │Currency  │ │Identifier│   │
│  └──────────┘ └──────────┘ └──────────┘ └──────────┘   │
│  ┌──────────┐ ┌──────────┐ ┌──────────────────────┐     │
│  │ COA      │ │ Saga     │ │ Resilience           │     │
│  │Tree struct│ │Orch+Chor │ │Chaos+Circuit+Signal │     │
│  └──────────┘ └──────────┘ └──────────────────────┘     │
└─────────────────────┬───────────────────────────────────┘
                      │
┌─────────────────────▼───────────────────────────────────┐
│                   LOG LAYER                              │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐   │
│  │ RingBuffer   │  │ EventLog     │  │ HashChain    │   │
│  │ Cache-padded │  │ WAL + CQRS   │  │ SHA-256      │   │
│  │ Lock-free    │  │ Snapshot     │  │ Immutable    │   │
│  └──────────────┘  └──────────────┘  └──────────────┘   │
│  ┌──────────────────────────────────────────────────┐    │
│  │ EventBus: Partitioned + Idempotent + ExactlyOnce │    │
│  └──────────────────────────────────────────────────┘    │
└─────────────────────┬───────────────────────────────────┘
                      │
┌─────────────────────▼───────────────────────────────────┐
│                 STORE LAYER                              │
│  ┌──────────────────────────────────────────────────┐    │
│  │ SurrealClient (pure Rust TCP, zero deps)         │    │
│  │ Account ⟷ journal_entry tables                   │    │
│  └──────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────┘
```

---

## Data Flow

### Transfer (Double-Entry)

```
Client                    API                    LedgerService              Journal
  │                        │                         │                        │
  │ POST /transfers        │                         │                        │
  │ {from,to,amount}       │                         │                        │
  │───────────────────────▶│                         │                        │
  │                        │ CircuitBreaker.check()  │                        │
  │                        │────────────────────────▶│                        │
  │                        │                         │ Validate accounts      │
  │                        │                         │ Debit from_account     │
  │                        │                         │  └─ CAS loop (Atomic)  │
  │                        │                         │ Credit to_account      │
  │                        │                         │  └─ fetch_add (Atomic) │
  │                        │                         │ Create JournalEntry    │
  │                        │                         │───────────────────────▶│
  │                        │                         │                        │ append(entry)
  │                        │                         │ ◀──────────────────────│
  │                        │                         │ Verify: debits=credits │
  │                        │ ◀────────────────────────│                        │
  │ ◀──────────────────────│                         │                        │
  │ 200 {txn_id, balances} │                         │                        │
```

### Balance Update (CAS Loop)

```
Thread A                    AtomicI64(available)           Thread B
  │                              │                            │
  │ load(available) = 1000       │                            │
  │─────────────────────────────▶│                            │
  │                              │                            │
  │ check: 1000 >= 100 ✓         │  load(available) = 1000    │
  │ new = 1000 - 100 = 900       │◀───────────────────────────│
  │                              │                            │
  │ CAS(1000, 900)               │  check: 1000 >= 50 ✓       │
  │─────────────────────────────▶│  new = 1000 - 50 = 950     │
  │         ✓ SUCCESS            │                            │
  │                              │  CAS(1000, 950)            │
  │                              │◀───────────────────────────│
  │                              │         ✗ FAIL (now 900)   │
  │                              │                            │
  │                              │  RETRY: load = 900         │
  │                              │  check: 900 >= 50 ✓        │
  │                              │  CAS(900, 850) → ✓         │
```

---

## API Design

### REST Endpoints

| Method | Path | Request Body | Response | Errors |
|--------|------|-------------|----------|--------|
| `GET` | `/health` | — | `{"status":"healthy","circuit_state":"Closed","error_rate":0.0}` | — |
| `POST` | `/accounts` | `{"account_type":"ASSET","currency":"USD","initial_balance_cents":100000}` | `{"id":"uuid","balance_cents":100000,"status":"Open"}` | 400 invalid type |
| `GET` | `/accounts/:id` | — | `{"id":"uuid","balance_cents":100000,"available_balance_cents":100000}` | 404 not found |
| `POST` | `/accounts/:id/debit` | `{"amount_cents":5000}` | `{"id":"uuid","balance_cents":95000}` | 400 insufficient/negative, 400 frozen |
| `POST` | `/accounts/:id/credit` | `{"amount_cents":5000}` | `{"id":"uuid","balance_cents":105000}` | 400 invalid, 400 frozen |
| `POST` | `/accounts/:id/status` | `{"status":"FROZEN"}` | `{"id":"uuid","status":"Frozen"}` | 400 invalid status |
| `POST` | `/transfers` | `{"from_account":"uuid","to_account":"uuid","amount_cents":1000,"description":"..."}` | `{"transaction_id":"uuid","journal_entry_id":"uuid","from_balance":99000,"to_balance":101000}` | 404 account, 400 insufficient, 503 circuit open |
| `GET` | `/admin/metrics` | — | `{"total_requests":42,"error_rate":0.0,"latency_p50_ms":1,"latency_p99_ms":5}` | — |

### Status Codes

| Code | Meaning | When |
|------|---------|------|
| `200` | Success | Operation completed |
| `400` | Bad Request | Invalid input, insufficient funds, frozen account |
| `404` | Not Found | Account doesn't exist |
| `405` | Method Not Allowed | Wrong HTTP method |
| `503` | Service Unavailable | Circuit breaker open |

### Account Types

| Type | Normal Balance | Description |
|------|---------------|-------------|
| `ASSET` | Debit | Cash, Inventory, Receivables |
| `LIABILITY` | Credit | Loans, Payables |
| `EQUITY` | Credit | Share Capital, Retained Earnings |
| `REVENUE` | Credit | Sales, Interest Income |
| `EXPENSE` | Debit | Rent, Salaries, COGS |

### State Machine

```
  ┌──────┐  freeze   ┌────────┐
  │ OPEN │──────────▶│ FROZEN │
  │      │◀──────────│        │
  └──┬───┘  unfreeze └───┬────┘
     │                   │
     │ close             │ close
     ▼                   ▼
  ┌────────┐
  │ CLOSED │ (irreversible)
  └────────┘
```

---

## Database Schema

### SurrealDB (banking_ledger/ledger @ :29180)

```sql
-- Account table
DEFINE TABLE account SCHEMAFULL;
DEFINE FIELD id              ON account TYPE string;     -- UUID
DEFINE FIELD account_type    ON account TYPE string;     -- Asset|Liability|Equity|Revenue|Expense
DEFINE FIELD currency        ON account TYPE string;     -- ISO 4217
DEFINE FIELD balance_cents   ON account TYPE int;        -- Current balance
DEFINE FIELD available_balance_cents ON account TYPE int; -- Available (balance - holds)
DEFINE FIELD status          ON account TYPE string;     -- Open|Frozen|Closed
DEFINE FIELD owner_party_id  ON account TYPE option<string>;
DEFINE FIELD created_at      ON account TYPE string;     -- RFC 3339
DEFINE INDEX idx_account_id  ON account COLUMNS id UNIQUE;

-- Journal entry table
DEFINE TABLE journal_entry SCHEMAFULL;
DEFINE FIELD id              ON journal_entry TYPE string;
DEFINE FIELD transaction_id  ON journal_entry TYPE string;
DEFINE FIELD sequence_number ON journal_entry TYPE int;
DEFINE FIELD description     ON journal_entry TYPE string;
DEFINE FIELD recorded_at     ON journal_entry TYPE string;
DEFINE FIELD reverses        ON journal_entry TYPE option<string>;
DEFINE INDEX idx_journal_id  ON journal_entry COLUMNS id UNIQUE;

-- Entry legs (normalized — one per debit/credit)
DEFINE TABLE entry_leg SCHEMAFULL;
DEFINE FIELD journal_entry_id ON entry_leg TYPE string;
DEFINE FIELD account_id       ON entry_leg TYPE string;
DEFINE FIELD side             ON entry_leg TYPE string;  -- Debit|Credit
DEFINE FIELD amount_cents     ON entry_leg TYPE int;
DEFINE INDEX idx_leg_entry    ON entry_leg COLUMNS journal_entry_id;
```

### In-Memory Structures

| Structure | Type | Concurrency | Purpose |
|-----------|------|-------------|---------|
| `accounts` | `DashMap<AccountId, Account>` | Lock-free reads | Account registry |
| `journal` | `RwLock<Vec<Arc<JournalEntry>>>` | RwLock | Append-only event log |
| `hash_chain.blocks` | `Vec<HashLink>` | Single-threaded | Immutable audit trail |
| `ring_buffer.slots` | `Box<[UnsafeCell<MaybeUninit<T>>]>` | Lock-free CAS | High-throughput event buffer |

---

## Concurrency Model

### Lock-Free Hot Path

The balance update path (`debit`/`credit`) is **entirely lock-free**:

```rust
loop {
    let current = self.available_balance.load(Ordering::SeqCst);  // Atomic read
    if current < amount { return Err(...); }                       // Check
    let new = current - amount;                                    // Compute
    if self.available_balance.compare_exchange(
        current, new, Ordering::SeqCst, Ordering::SeqCst
    ).is_ok() {                                                    // Atomic CAS
        return Ok(new);                                            // Success
    }
    // CAS failed → retry (another thread modified the value)
}
```

### Lock Hierarchy (cold paths only)

```
journal.write()           ← acquired only on append (infrequent)
  └─ accounts.read()      ← shared read lock (never blocks writes)
       └─ account.debit() ← lock-free CAS loop
```

### Memory Ordering

**All financial operations use `Ordering::SeqCst`** — the strongest guarantee:
- Required for correctness on ARM/POWER architectures
- Prevents subtle reordering bugs between balance and available_balance
- Performance cost: ~5-10ns on x86 (acceptable for financial correctness)

---

## Security Model

| Layer | Mechanism | Purpose |
|-------|-----------|---------|
| **Input** | UUID validation, amount bounds (0 < x < $10B) | Prevent injection, overflow |
| **Transport** | TLS (via reverse proxy) | Encrypt data in transit |
| **Concurrency** | Atomic CAS + SeqCst ordering | Prevent lost updates, data races |
| **Immutability** | SHA-256 hash chain | Tamper detection |
| **Integrity** | HMAC-SHA256 signatures | Internal message auth |
| **Availability** | Circuit Breaker + Bulkhead | Prevent cascading failures |
| **Rate Limiting** | Token Bucket (configurable rate) | Prevent DoS |
| **Observability** | Golden Signals (latency/traffic/errors/saturation) | Detect anomalies |
| **Audit** | Append-only journal + hash chain | Full traceability |

---

## Deployment

### Quick Start

```bash
cargo run                          # API server on :3001
python3 test_api.py                # 19 integration tests
```

### Docker

```bash
docker build -t banking-ledger .
docker run -p 3001:3001 banking-ledger
```

### Systemd

```bash
sudo cp banking-ledger.service /etc/systemd/system/
sudo systemctl enable --now banking-ledger
```

### Binary Size

```
Release (stripped): 1.6 MB
Debug:              55 MB
```

---

## File Map

```
banking-ledger-rs/
├── src/
│   ├── main.rs                    # Entry point + crate-level docs + lint attrs
│   ├── domain/
│   │   ├── account.rs             # Account (AtomicI64 CAS, holds, status machine)
│   │   ├── journal.rs             # JournalEntry (immutable, self-validating)
│   │   ├── money.rs               # Money (Decimal + Currency + Rounding)
│   │   ├── party.rs               # Party (UUID v7, immutable identity)
│   │   ├── identifier.rs          # Identifier (versioned, never overwritten)
│   │   ├── coa.rs                 # Chart of Accounts (hierarchical tree)
│   │   └── *_test.rs              # Unit tests per domain module
│   ├── service/
│   │   ├── ledger_service.rs      # Double-entry orchestration
│   │   ├── account_service.rs     # DashMap account registry
│   │   ├── identity_service.rs    # Party + Identifier management
│   │   ├── concurrency.rs         # CAS, Condvar, RwLock, FairQueue, Race tests
│   │   ├── saga.rs                # Saga Orchestrator + Outbox + Compensation
│   │   ├── resilience.rs          # CircuitBreaker, Bulkhead, TokenBucket, Chaos
│   │   ├── advanced.rs            # DeadlockDetector, LatencyHistogram, EventBus
│   │   ├── production.rs          # PostMortem, Tuning, StressTest, Security
│   │   └── *_test.rs              # Service-level tests
│   ├── log/
│   │   ├── ring_buffer.rs         # Cache-padded lock-free ring buffer
│   │   ├── event_log.rs           # WAL + Event Sourcing + CQRS
│   │   ├── hash_chain.rs          # SHA-256 chain + HMAC + Tamper detection
│   │   └── event_bus.rs           # Partitioned + Idempotent + Exactly-Once
│   ├── store/
│   │   └── surrealdb.rs           # Pure Rust HTTP SurrealDB client
│   ├── api/
│   │   └── mod.rs                 # Axum router + 7 handlers
│   ├── edge_cases_test.rs         # 17 boundary condition tests
│   ├── exhaustive_edge_tests.rs   # 46 comprehensive edge case tests
│   └── deep_correctness_tests.rs  # 5 correctness proof tests
├── test_api.py                    # 19 API integration tests
├── Cargo.toml                     # 14 dependencies
├── Dockerfile                     # Multi-stage (5MB final image)
├── Makefile                       # test, run, docker, bench, ci
├── banking-ledger.service         # Systemd unit file
├── .github/workflows/ci.yml      # CI/CD pipeline
├── README.md                      # This file
└── ARCHITECTURE.md                # Architecture document
```

---

## Commit Conventions

All commits follow [Conventional Commits](https://www.conventionalcommits.org/):

| Prefix | Meaning | Example |
|--------|---------|---------|
| `feat:` | New feature | `feat: SurrealDB persistence layer` |
| `fix:` | Bug fix | `fix: SeqCst ordering for ARM correctness` |
| `test:` | Tests added | `test: 46 exhaustive edge case tests` |
| `docs:` | Documentation | `docs: add architecture documentation` |
| `refactor:` | Code restructuring | `refactor: idiomatic Rust patterns` |
| `chore:` | Maintenance | `chore: cargo fmt + clippy clean` |
| `ops:` | Operations | `ops: systemd service file` |
| `audit:` | Security/quality audit | `audit: Rust KG cross-reference` |
| `ci:` | CI/CD | `ci: GitHub Actions pipeline` |
| `devops:` | Docker/Makefile | `devops: Dockerfile + Makefile` |
