//! Event-driven saga choreography engine.
//! Unlike the Orchestrator (central coordinator), choreography uses
//! decentralized event propagation — each service reacts to events
//! and publishes its own, forming a reactive chain.
//!
//! Every consumer is idempotent via `correlation_id` deduplication.

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use rust_decimal::Decimal;
use uuid::Uuid;

// ━━━ Saga State ━━━

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SagaState {
    Initiated,
    DebitPending,
    Debited,
    CreditPending,
    Completed,
    Failed,
}

// ━━━ Events ━━━

#[derive(Debug, Clone)]
pub struct TransferInitiatedEvent {
    pub correlation_id: Uuid,
    pub transaction_id: String,
    pub from_account: String,
    pub to_account: String,
    pub amount: Decimal,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct AccountDebitedEvent {
    pub correlation_id: Uuid,
    pub transaction_id: String,
    pub account_id: String,
    pub amount: Decimal,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct AccountCreditedEvent {
    pub correlation_id: Uuid,
    pub transaction_id: String,
    pub account_id: String,
    pub amount: Decimal,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct AccountDebitFailedEvent {
    pub correlation_id: Uuid,
    pub transaction_id: String,
    pub account_id: String,
    pub amount: Decimal,
    pub reason: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct AccountCreditFailedEvent {
    pub correlation_id: Uuid,
    pub transaction_id: String,
    pub account_id: String,
    pub amount: Decimal,
    pub reason: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct TransferCompletedEvent {
    pub correlation_id: Uuid,
    pub transaction_id: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct TransferFailedEvent {
    pub correlation_id: Uuid,
    pub transaction_id: String,
    pub reason: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct RefundInitiatedEvent {
    pub correlation_id: Uuid,
    pub transaction_id: String,
    pub account_id: String,
    pub amount: Decimal,
    pub timestamp: DateTime<Utc>,
}

// ━━━ Transaction Service (Choreography coordinator) ━━━

/// Tracks saga state per transaction and manages the event chain.
pub struct ChoreographyTransactionService {
    pub states: DashMap<String, SagaState>,
}

impl Default for ChoreographyTransactionService {
    fn default() -> Self {
        Self::new()
    }
}

impl ChoreographyTransactionService {
    pub fn new() -> Self {
        Self {
            states: DashMap::new(),
        }
    }

    /// Initiate a new transfer. Returns the event to publish.
    pub fn initiate_transfer(
        &self,
        from_account: &str,
        to_account: &str,
        amount: Decimal,
    ) -> TransferInitiatedEvent {
        let correlation_id = Uuid::now_v7();
        let transaction_id = Uuid::now_v7().to_string();
        self.states
            .insert(transaction_id.clone(), SagaState::Initiated);
        TransferInitiatedEvent {
            correlation_id,
            transaction_id,
            from_account: from_account.to_string(),
            to_account: to_account.to_string(),
            amount,
            timestamp: Utc::now(),
        }
    }

    /// Handle account debited confirmation.
    pub fn handle_account_debited(&self, event: &AccountDebitedEvent) {
        self.states
            .insert(event.transaction_id.clone(), SagaState::Debited);
    }

    /// Handle account credited confirmation — transfer complete.
    pub fn handle_account_credited(&self, event: &AccountCreditedEvent) -> TransferCompletedEvent {
        self.states
            .insert(event.transaction_id.clone(), SagaState::Completed);
        TransferCompletedEvent {
            correlation_id: event.correlation_id,
            transaction_id: event.transaction_id.clone(),
            timestamp: Utc::now(),
        }
    }

    /// Handle debit failure.
    pub fn handle_debit_failed(&self, event: &AccountDebitFailedEvent) -> TransferFailedEvent {
        self.states
            .insert(event.transaction_id.clone(), SagaState::Failed);
        TransferFailedEvent {
            correlation_id: event.correlation_id,
            transaction_id: event.transaction_id.clone(),
            reason: event.reason.clone(),
            timestamp: Utc::now(),
        }
    }

    /// Handle credit failure — trigger compensation.
    pub fn handle_credit_failed(&self, event: &AccountCreditFailedEvent) -> (TransferFailedEvent, RefundInitiatedEvent) {
        self.states
            .insert(event.transaction_id.clone(), SagaState::Failed);
        let failed = TransferFailedEvent {
            correlation_id: event.correlation_id,
            transaction_id: event.transaction_id.clone(),
            reason: event.reason.clone(),
            timestamp: Utc::now(),
        };
        let refund = RefundInitiatedEvent {
            correlation_id: event.correlation_id,
            transaction_id: event.transaction_id.clone(),
            account_id: event.account_id.clone(),
            amount: event.amount,
            timestamp: Utc::now(),
        };
        (failed, refund)
    }
}

// ━━━ Account Service (event handler) ━━━

/// Reacts to transfer initiation events by debiting the source account.
pub struct ChoreographyAccountService {
    pub balances: DashMap<String, Decimal>,
    /// Track already-processed `correlation_ids` for idempotency.
    pub processed_debits: DashMap<Uuid, ()>,
    pub processed_credits: DashMap<Uuid, ()>,
}

impl Default for ChoreographyAccountService {
    fn default() -> Self {
        Self::new()
    }
}

impl ChoreographyAccountService {
    pub fn new() -> Self {
        Self {
            balances: DashMap::new(),
            processed_debits: DashMap::new(),
            processed_credits: DashMap::new(),
        }
    }

    /// Seed an account with an initial balance.
    pub fn seed(&self, account_id: &str, balance: Decimal) {
        self.balances.insert(account_id.to_string(), balance);
    }

    /// Handle `TransferInitiatedEvent`: debit source, return result event.
    pub fn handle_transfer_initiated(
        &self,
        event: &TransferInitiatedEvent,
    ) -> Result<AccountDebitedEvent, AccountDebitFailedEvent> {
        // Idempotency check
        if self.processed_debits.contains_key(&event.correlation_id) {
            // Already processed — return a synthetic debited event
            return Ok(AccountDebitedEvent {
                correlation_id: event.correlation_id,
                transaction_id: event.transaction_id.clone(),
                account_id: event.from_account.clone(),
                amount: event.amount,
                timestamp: Utc::now(),
            });
        }

        let current_balance = self.balances.get(&event.from_account).map(|b| *b);
        match current_balance {
            Some(b) if b >= event.amount => {
                let new_balance = b - event.amount;
                self.balances.insert(event.from_account.clone(), new_balance);
                self.processed_debits.insert(event.correlation_id, ());
                Ok(AccountDebitedEvent {
                    correlation_id: event.correlation_id,
                    transaction_id: event.transaction_id.clone(),
                    account_id: event.from_account.clone(),
                    amount: event.amount,
                    timestamp: Utc::now(),
                })
            }
            Some(_) => Err(AccountDebitFailedEvent {
                correlation_id: event.correlation_id,
                transaction_id: event.transaction_id.clone(),
                account_id: event.from_account.clone(),
                amount: event.amount,
                reason: "INSUFFICIENT_FUNDS".to_string(),
                timestamp: Utc::now(),
            }),
            None => Err(AccountDebitFailedEvent {
                correlation_id: event.correlation_id,
                transaction_id: event.transaction_id.clone(),
                account_id: event.from_account.clone(),
                amount: event.amount,
                reason: "ACCOUNT_NOT_FOUND".to_string(),
                timestamp: Utc::now(),
            }),
        }
    }

    /// Handle `AccountDebitedEvent`: credit destination.
    pub fn handle_account_debited(
        &self,
        event: &AccountDebitedEvent,
        to_account: &str,
    ) -> Result<AccountCreditedEvent, AccountCreditFailedEvent> {
        // Idempotency check
        if self.processed_credits.contains_key(&event.correlation_id) {
            return Ok(AccountCreditedEvent {
                correlation_id: event.correlation_id,
                transaction_id: event.transaction_id.clone(),
                account_id: to_account.to_string(),
                amount: event.amount,
                timestamp: Utc::now(),
            });
        }

        let current_balance = self.balances.get(to_account).map(|b| *b);
        if let Some(b) = current_balance {
            let new_balance = b + event.amount;
            self.balances.insert(to_account.to_string(), new_balance);
            self.processed_credits.insert(event.correlation_id, ());
            Ok(AccountCreditedEvent {
                correlation_id: event.correlation_id,
                transaction_id: event.transaction_id.clone(),
                account_id: to_account.to_string(),
                amount: event.amount,
                timestamp: Utc::now(),
            })
        } else {
            // Compensation: refund the debit
            if let Some(mut b) = self.balances.get_mut(&event.account_id) {
                *b += event.amount;
            }
            Err(AccountCreditFailedEvent {
                correlation_id: event.correlation_id,
                transaction_id: event.transaction_id.clone(),
                account_id: to_account.to_string(),
                amount: event.amount,
                reason: "DESTINATION_NOT_FOUND".to_string(),
                timestamp: Utc::now(),
            })
        }
    }
}

// ━━━ Notification Service ━━━

/// Logs events for observability.
pub struct ChoreographyNotificationService;

impl ChoreographyNotificationService {
    pub fn on_transfer_initiated(event: &TransferInitiatedEvent) {
        tracing::info!(
            cid = %event.correlation_id,
            tid = %event.transaction_id,
            "Transfer initiated: {} -> {}, {}",
            event.from_account,
            event.to_account,
            event.amount
        );
    }

    pub fn on_transfer_completed(event: &TransferCompletedEvent) {
        tracing::info!(
            cid = %event.correlation_id,
            tid = %event.transaction_id,
            "Transfer completed"
        );
    }

    pub fn on_transfer_failed(event: &TransferFailedEvent) {
        tracing::warn!(
            cid = %event.correlation_id,
            tid = %event.transaction_id,
            reason = %event.reason,
            "Transfer failed"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_choreography_happy_path() {
        let tx_svc = ChoreographyTransactionService::new();
        let acct_svc = ChoreographyAccountService::new();
        acct_svc.seed("alice", dec!(1000.00));
        acct_svc.seed("bob", dec!(500.00));

        // Step 1: Initiate transfer
        let initiated = tx_svc.initiate_transfer("alice", "bob", dec!(200.00));
        ChoreographyNotificationService::on_transfer_initiated(&initiated);

        // Step 2: Handle transfer — debit alice
        let debited = acct_svc
            .handle_transfer_initiated(&initiated)
            .expect("debit should succeed");
        tx_svc.handle_account_debited(&debited);

        // Step 3: Handle debited — credit bob
        let credited = acct_svc
            .handle_account_debited(&debited, &initiated.to_account)
            .expect("credit should succeed");

        // Step 4: Complete
        let completed = tx_svc.handle_account_credited(&credited);
        ChoreographyNotificationService::on_transfer_completed(&completed);

        assert_eq!(*acct_svc.balances.get("alice").unwrap(), dec!(800.00));
        assert_eq!(*acct_svc.balances.get("bob").unwrap(), dec!(700.00));
        assert_eq!(*tx_svc.states.get(&initiated.transaction_id).unwrap(), SagaState::Completed);
    }

    #[test]
    fn test_choreography_insufficient_funds() {
        let tx_svc = ChoreographyTransactionService::new();
        let acct_svc = ChoreographyAccountService::new();
        acct_svc.seed("alice", dec!(10.00));

        let initiated = tx_svc.initiate_transfer("alice", "bob", dec!(200.00));
        let result = acct_svc.handle_transfer_initiated(&initiated);

        assert!(result.is_err());
        let failed = result.unwrap_err();
        assert_eq!(failed.reason, "INSUFFICIENT_FUNDS");

        tx_svc.handle_debit_failed(&failed);
        assert_eq!(*tx_svc.states.get(&initiated.transaction_id).unwrap(), SagaState::Failed);
    }

    #[test]
    fn test_choreography_credit_failure_compensation() {
        let tx_svc = ChoreographyTransactionService::new();
        let acct_svc = ChoreographyAccountService::new();
        acct_svc.seed("alice", dec!(1000.00));
        // bob NOT seeded — credit will fail

        let initiated = tx_svc.initiate_transfer("alice", "bob", dec!(200.00));
        let debited = acct_svc
            .handle_transfer_initiated(&initiated)
            .expect("debit should succeed");
        tx_svc.handle_account_debited(&debited);

        // Credit to unseeded bob — fails with compensation
        let credit_result = acct_svc.handle_account_debited(&debited, "bob");
        assert!(credit_result.is_err());

        let failed = credit_result.unwrap_err();
        assert_eq!(failed.reason, "DESTINATION_NOT_FOUND");

        let (_transfer_failed, refund) = tx_svc.handle_credit_failed(&failed);
        assert_eq!(refund.account_id, "bob");
        assert_eq!(refund.amount, dec!(200.00));

        // Verify compensation: alice got refunded
        assert_eq!(*acct_svc.balances.get("alice").unwrap(), dec!(1000.00));
    }

    #[test]
    fn test_choreography_idempotent_debit() {
        let acct_svc = ChoreographyAccountService::new();
        acct_svc.seed("alice", dec!(500.00));

        let event = TransferInitiatedEvent {
            correlation_id: Uuid::now_v7(),
            transaction_id: "tx-123".to_string(),
            from_account: "alice".to_string(),
            to_account: "bob".to_string(),
            amount: dec!(100.00),
            timestamp: Utc::now(),
        };

        // First debit
        let r1 = acct_svc.handle_transfer_initiated(&event);
        assert!(r1.is_ok());

        // Duplicate debit — idempotent
        let r2 = acct_svc.handle_transfer_initiated(&event);
        assert!(r2.is_ok());

        // Balance only debited once
        assert_eq!(*acct_svc.balances.get("alice").unwrap(), dec!(400.00));
    }
}
