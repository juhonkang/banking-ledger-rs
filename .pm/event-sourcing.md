# Event Sourcing Model

## Architecture

```
  Command → CommandDispatcher → EventStore.append()
                                   ↓
                              EventBus.publish()
                                   ↓
                         ┌─────────┼─────────┐
                    ReadModel    Saga     AuditLog
                   (projection) (react)  (HashChain)
```

## Core Primitives

### Event
- `event_id`: Uuid (v7, time-ordered)
- `aggregate_id`: Uuid (which entity)
- `event_type`: String (FundsDeposited, FundsWithdrawn, etc.)
- `payload`: JSON serialized data
- `version`: u64 (monotonic per aggregate)
- `timestamp`: DateTime<Utc>

### Command
- `command_id`: Uuid (idempotency key)
- `aggregate_id`: Uuid
- `command_type`: String
- `payload`: JSON

### WAL (Write-Ahead Log)
- CRC-64 integrity check per entry
- Sequential append-only
- Replayable for state reconstruction

### Snapshot
- Periodic snapshot of aggregate state
- `SnapshotStore::should_snapshot(version)` — configurable frequency
- Reduces replay cost on restart

## CQRS Projections

- **ReadModel**: Materialized view from events
- **AccountProjection**: Current balance, status, holds
- **TransactionStateProjector**: Transaction lifecycle tracking

## Event Sourcing Flow

1. Client sends Command
2. CommandDispatcher checks idempotency (command_id)
3. EventStore appends new Event(s)
4. EventBus publishes to subscribers
5. ReadModel updates projections
6. Saga orchestrator reacts to domain events
7. HashChain records cryptographic proof
8. SnapshotStore periodically snapshots

## Idempotency

- Commands use `command_id` dedup
- Events use `correlation_id` dedup (choreography)
- Consumers use `transaction_id` dedup (saga)
- IdempotencyService provides DashMap-based O(1) lookup with auto-eviction

## Replay

```
fn rebuild_state(events: &[Event]) -> ReadModel {
    let mut model = ReadModel::new();
    for event in events {
        model.apply(event);
    }
    model
}
```

## Known Limitations

1. No event schema migration (breaking changes require full replay)
2. No snapshot compression (in-memory)
3. EventStore grows unboundedly (no archiving/compaction yet)
4. AsyncFlushBuffer defined but not wired into WAL write path
