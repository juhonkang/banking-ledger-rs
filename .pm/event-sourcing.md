# Event Sourcing Design

> How the banking ledger achieves immutability, auditability, and crash recovery through append-only event storage.

## Core Principle

```
State = fold(initial_state, events[0..n])
```

Current account balances are a CACHED PROJECTION of the event log. The log is the source of truth.

---

## WAL (Write-Ahead Log) Format

### Binary Layout

```
┌────────────────────────────────────────────┐
│ Magic: 0x424C414C (4 bytes) — "BLAL"      │
├────────────────────────────────────────────┤
│ Version: u16 (2 bytes)                     │
├────────────────────────────────────────────┤
│ Sequence: u64 (8 bytes)                    │
├────────────────────────────────────────────┤
│ Timestamp: i64 (8 bytes) — Unix µs         │
├────────────────────────────────────────────┤
│ Event Type: u16 (2 bytes)                  │
├────────────────────────────────────────────┤
│ Payload Length: u32 (4 bytes)              │
├────────────────────────────────────────────┤
│ Payload: JSON (variable)                   │
├────────────────────────────────────────────┤
│ CRC-64: u64 (8 bytes)                      │
└────────────────────────────────────────────┘
```

### Event Types

| Code | Type | Description |
|------|------|-------------|
| 0x01 | AccountCreated | New account with initial balance |
| 0x02 | FundsDeposited | Credit to account |
| 0x03 | FundsWithdrawn | Debit from account |
| 0x04 | TransferRecorded | Double-entry transfer |
| 0x05 | HoldPlaced | Funds reserved |
| 0x06 | HoldReleased | Reservation cleared |
| 0x07 | StatusChanged | Account frozen/unfrozen/closed |
| 0x08 | EntryReversed | Correcting journal entry |

---

## CRC-64 Checksum

```
CRC64 = hash(payload) mod 2^64
```

Simple polynomial hash: `h = h * 31 + byte` for each byte.

Verification on replay: recompute CRC, compare with stored value. Mismatch = corruption detected.

---

## Replay Algorithm

```
function replay(wal_path):
    entries = []
    file = open(wal_path, "r")
    for line in file:
        entry = parse_json(line)
        stored_crc = entry.checksum
        entry.checksum = 0  // zero out for recompute
        recomputed = crc64(json(entry))
        if recomputed == stored_crc:
            entries.append(entry)
        else:
            log_error("CRC mismatch at seq {}", entry.sequence)
    return entries
```

---

## Snapshot Strategy

### When to Snapshot

Every N events per aggregate (default N=10,000):

```
if aggregate_version % 10_000 == 0:
    take_snapshot(aggregate_id, current_state)
```

### Snapshot Format

```json
{
  "aggregate_id": "uuid",
  "version": 10000,
  "state_json": "{\"balance_cents\": 500000, ...}",
  "taken_at": "2026-06-06T09:00:00Z"
}
```

### Recovery with Snapshot

```
function rebuild(aggregate_id):
    snapshot = load_latest_snapshot(aggregate_id)
    state = snapshot.state
    events = load_events_after(aggregate_id, snapshot.version)
    for event in events:
        state = apply(state, event)
    return state
```

Speedup: 10,000 events with snapshot = 1 snapshot load + 0 events (instant) vs 10,000 event replays (seconds).

---

## CQRS Projection

### Write Model (Command Side)

```
Command → validate → produce events → append to WAL → update cache
```

### Read Model (Query Side)

```
Query → ReadModel (denormalized) → response
```

### Projections

| Projection | Source Events | Fields | Use Case |
|-----------|---------------|--------|----------|
| AccountProjection | AccountCreated, FundsDeposited, FundsWithdrawn | balance, total_credits, total_debits | GET /accounts/:id |
| DailyVolume | FundsDeposited, FundsWithdrawn | date, total_volume | Risk monitoring |
| TransferReport | TransferRecorded | from, to, amount, timestamp | Compliance audit |

### Rebuild from Scratch

```rust
let model = ReadModel::rebuild(&all_events);
// Replay every event in order → fully consistent read model
```

---

## Idempotent Command Processing

```
function process(command):
    if already_processed(command.id):
        return existing_result
    events = handler(command)
    for event in events:
        append_to_wal(event)
        mark_processed(command.id)
    return events
```

Duplicate command IDs return the same result — no double-spending.

---

## Async Flush Buffer

For high-throughput, batch WAL writes:

```
buffer.add(event1)  → None (not full yet)
buffer.add(event2)  → None
buffer.add(event3)  → Some([event1, event2, event3])  ← batch flush
```

Trade-off: batch_size=100 → 100x fewer fsync calls, but up to 100 events at risk on crash.
