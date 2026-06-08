//! Distributed state visualization — materialized view of transaction state.
//! Implements CQRS projection pattern: events are consumed, projected into
//! a read-optimized state table, and queried without impacting write path.

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use rust_decimal::Decimal;
use uuid::Uuid;

// ━━━ Transaction Status ━━━

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionStatus {
    Initiated,
    FundsReserved,
    Completed,
    Failed,
}

impl TransactionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Initiated => "INITIATED",
            Self::FundsReserved => "FUNDS_RESERVED",
            Self::Completed => "COMPLETED",
            Self::Failed => "FAILED",
        }
    }
}

// ━━━ Domain Events ━━━

#[derive(Debug, Clone)]
pub struct TransactionEvent {
    pub transaction_id: Uuid,
    pub event_type: TransactionStatus,
    pub from_account: String,
    pub to_account: String,
    pub amount: Decimal,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct TransactionState {
    pub transaction_id: Uuid,
    pub current_status: TransactionStatus,
    pub from_account: String,
    pub to_account: String,
    pub amount: Decimal,
    pub last_updated: DateTime<Utc>,
    pub event_count: usize,
}

// ━━━ State Projector ━━━

/// Consumes events and projects them into a materialized view.
/// Idempotent: duplicate events are detected by timestamp comparison.
pub struct TransactionStateProjector {
    states: DashMap<Uuid, TransactionState>,
}

impl TransactionStateProjector {
    pub fn new() -> Self {
        Self {
            states: DashMap::new(),
        }
    }

    /// Apply a transaction event to the materialized view.
    /// Idempotent: ignores events older than the current state.
    pub fn apply(&self, event: TransactionEvent) {
        let mut entry = self.states.entry(event.transaction_id);

        match entry {
            dashmap::mapref::entry::Entry::Occupied(mut occ) => {
                let state = occ.get_mut();
                // Idempotency: only apply if event is newer
                if event.timestamp > state.last_updated {
                    state.current_status = event.event_type;
                    state.last_updated = event.timestamp;
                    state.event_count += 1;
                    state.amount = event.amount;
                }
            }
            dashmap::mapref::entry::Entry::Vacant(vac) => {
                vac.insert(TransactionState {
                    transaction_id: event.transaction_id,
                    current_status: event.event_type,
                    from_account: event.from_account,
                    to_account: event.to_account,
                    amount: event.amount,
                    last_updated: event.timestamp,
                    event_count: 1,
                });
            }
        }
    }

    /// Get the current state of a transaction.
    pub fn get(&self, transaction_id: Uuid) -> Option<TransactionState> {
        self.states.get(&transaction_id).map(|r| r.clone())
    }

    /// Get all transactions in a given status.
    pub fn by_status(&self, status: TransactionStatus) -> Vec<TransactionState> {
        self.states
            .iter()
            .filter(|e| e.value().current_status == status)
            .map(|e| e.value().clone())
            .collect()
    }

    /// Number of tracked transactions.
    pub fn len(&self) -> usize {
        self.states.len()
    }

    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }
}

impl Default for TransactionStateProjector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event(tx_id: Uuid, status: TransactionStatus, offset_secs: i64) -> TransactionEvent {
        TransactionEvent {
            transaction_id: tx_id,
            event_type: status,
            from_account: "alice".into(),
            to_account: "bob".into(),
            amount: Decimal::new(10000, 2),
            timestamp: Utc::now() + chrono::Duration::seconds(offset_secs),
        }
    }

    #[test]
    fn test_projector_lifecycle() {
        let projector = TransactionStateProjector::new();
        let tx_id = Uuid::now_v7();

        projector.apply(make_event(tx_id, TransactionStatus::Initiated, 0));
        projector.apply(make_event(tx_id, TransactionStatus::FundsReserved, 1));
        projector.apply(make_event(tx_id, TransactionStatus::Completed, 2));

        let state = projector.get(tx_id).unwrap();
        assert_eq!(state.current_status, TransactionStatus::Completed);
        assert_eq!(state.event_count, 3);
    }

    #[test]
    fn test_projector_idempotent() {
        let projector = TransactionStateProjector::new();
        let tx_id = Uuid::now_v7();

        projector.apply(make_event(tx_id, TransactionStatus::Initiated, 0));
        projector.apply(make_event(tx_id, TransactionStatus::Completed, 2));
        // Replay old event — should be ignored
        projector.apply(make_event(tx_id, TransactionStatus::Initiated, 0));

        let state = projector.get(tx_id).unwrap();
        assert_eq!(state.current_status, TransactionStatus::Completed);
        assert_eq!(state.event_count, 2);
    }

    #[test]
    fn test_projector_by_status() {
        let projector = TransactionStateProjector::new();

        let tx1 = Uuid::now_v7();
        let tx2 = Uuid::now_v7();

        projector.apply(make_event(tx1, TransactionStatus::Initiated, 0));
        projector.apply(make_event(tx2, TransactionStatus::Initiated, 0));
        projector.apply(make_event(tx1, TransactionStatus::Completed, 1));

        let initiated = projector.by_status(TransactionStatus::Initiated);
        let completed = projector.by_status(TransactionStatus::Completed);

        assert_eq!(initiated.len(), 1); // only tx2 still initiated
        assert_eq!(completed.len(), 1); // tx1 completed
    }
}
