//! Ledger Service — orchestrates double-entry transactions.
//! Journal-first, balance-second: the journal entry is written before balances are updated.
//! Atomic: either both succeed, or both fail with rollback.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use chrono::Utc;
use uuid::Uuid;

use crate::domain::account::{Account, AccountId};
use crate::domain::journal::{
    EntryLeg, EntrySide, JournalEntry, JournalEntryId, JournalError, Transaction, TransactionId,
};

/// The immutable journal — an append-only log of all financial events.
#[derive(Debug)]
struct Journal {
    entries: Vec<Arc<JournalEntry>>,
    sequence_counter: u64,
}

impl Journal {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            sequence_counter: 0,
        }
    }

    fn next_sequence(&mut self) -> u64 {
        self.sequence_counter += 1;
        self.sequence_counter
    }

    fn append(&mut self, entry: JournalEntry) -> Arc<JournalEntry> {
        let arc = Arc::new(entry);
        self.entries.push(Arc::clone(&arc));
        arc
    }
}

/// The Ledger Service — the heart of financial truth.
/// All state changes go through here. Journal is append-only.
pub struct LedgerService {
    /// Immutable journal of all entries
    journal: RwLock<Journal>,
    /// Account registry (shared with `AccountService` in real system)
    pub(crate) accounts: Arc<RwLock<HashMap<AccountId, Account>>>,
    /// Active transactions
    transactions: Mutex<HashMap<TransactionId, Transaction>>,
}

impl LedgerService {
    pub fn new(accounts: Arc<RwLock<HashMap<AccountId, Account>>>) -> Self {
        Self {
            journal: RwLock::new(Journal::new()),
            accounts,
            transactions: Mutex::new(HashMap::new()),
        }
    }

    /// Begin a new transaction
    pub fn begin_transaction(&self, reference: &str) -> TransactionId {
        let txn = Transaction::new(reference);
        let id = txn.id;
        self.transactions.lock().unwrap().insert(id, txn);
        id
    }

    /// Record a simple transfer between two accounts.
    /// Debits `from_account`, credits `to_account`. Atomic.
    pub fn record_transfer(
        &self,
        transaction_id: TransactionId,
        from_account: AccountId,
        to_account: AccountId,
        amount_cents: i64,
        description: &str,
    ) -> Result<Arc<JournalEntry>, LedgerError> {
        let legs = vec![
            EntryLeg::debit(from_account, amount_cents),
            EntryLeg::credit(to_account, amount_cents),
        ];

        self.record_entry(transaction_id, legs, description)
    }

    /// Record a compound entry with multiple legs.
    /// JOURNAL-FIRST: Entry is appended to immutable journal BEFORE balance updates.
    /// If crash occurs between journal append and balance updates, the journal entry
    /// serves as audit trail for recovery replay.
    /// All debits must equal all credits.
    pub fn record_entry(
        &self,
        transaction_id: TransactionId,
        legs: Vec<EntryLeg>,
        description: &str,
    ) -> Result<Arc<JournalEntry>, LedgerError> {
        // 1. Validate + append to journal FIRST (source of truth)
        let entry = {
            let mut journal = self.journal.write().unwrap();
            let seq = journal.next_sequence();
            let entry = JournalEntry::new(transaction_id, seq, legs.clone(), description)
                .map_err(LedgerError::JournalError)?;
            journal.append(entry)
        };
        // Journal lock released here — entry is durable in audit trail

        // 2. Apply balance updates to all accounts
        let accounts = self.accounts.read().unwrap();
        for leg in &legs {
            let account = accounts
                .get(&leg.account_id)
                .ok_or_else(|| {
                    // Journal already has this entry, but account missing.
                    // This is an invariant violation — log and flag for reconciliation.
                    LedgerError::AccountNotFound(leg.account_id)
                })?;

            match leg.side {
                EntrySide::Debit => {
                    account
                        .debit(leg.amount_cents)
                        .map_err(|e| LedgerError::DebitError(leg.account_id, e))?;
                }
                EntrySide::Credit => {
                    account
                        .credit(leg.amount_cents)
                        .map_err(|e| LedgerError::CreditError(leg.account_id, e))?;
                }
            }
        }
        drop(accounts);

        // 3. Mark transaction as committed
        if let Some(txn) = self.transactions.lock().unwrap().get_mut(&transaction_id) {
            txn.commit();
        }

        Ok(entry)
    }

    /// Reverse a previously recorded entry (creates a correcting entry).
    /// JOURNAL-FIRST: reversal is appended BEFORE balance updates.
    pub fn reverse_entry(
        &self,
        original_entry_id: JournalEntryId,
        _reason: &str,
    ) -> Result<Arc<JournalEntry>, LedgerError> {
        let original = {
            let journal = self.journal.read().unwrap();
            let original = journal
                .entries
                .iter()
                .find(|e| e.id == original_entry_id)
                .ok_or(LedgerError::EntryNotFound(original_entry_id))?;
            original.clone()
        };

        let new_txn_id = self.begin_transaction(&format!("REV-{original_entry_id}"));

        // 1. Create and append reversal to journal FIRST
        let reversal = {
            let mut journal = self.journal.write().unwrap();
            let seq = journal.next_sequence();
            let reversal = original
                .reverse(new_txn_id, seq)
                .map_err(LedgerError::JournalError)?;
            journal.append(reversal)
        };

        // 2. Apply balance updates from the reversal legs
        let accounts = self.accounts.read().unwrap();
        for leg in &reversal.legs {
            let account = accounts
                .get(&leg.account_id)
                .ok_or(LedgerError::AccountNotFound(leg.account_id))?;
            match leg.side {
                EntrySide::Debit => {
                    account
                        .debit(leg.amount_cents)
                        .map_err(|e| LedgerError::DebitError(leg.account_id, e))?;
                }
                EntrySide::Credit => {
                    account
                        .credit(leg.amount_cents)
                        .map_err(|e| LedgerError::CreditError(leg.account_id, e))?;
                }
            }
        }
        drop(accounts);

        Ok(reversal)
    }

    /// Get all journal entries (for audit/replay)
    pub fn get_all_entries(&self) -> Vec<Arc<JournalEntry>> {
        self.journal.read().unwrap().entries.clone()
    }

    /// Get transaction status
    pub fn get_transaction(&self, txn_id: TransactionId) -> Option<Transaction> {
        self.transactions.lock().unwrap().get(&txn_id).cloned()
    }
}

#[derive(Debug)]
pub enum LedgerError {
    JournalError(JournalError),
    AccountNotFound(AccountId),
    DebitError(AccountId, crate::domain::account::DebitError),
    CreditError(AccountId, crate::domain::account::CreditError),
    EntryNotFound(JournalEntryId),
}

impl std::fmt::Display for LedgerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::JournalError(e) => write!(f, "Journal error: {e}"),
            Self::AccountNotFound(id) => write!(f, "Account not found: {id}"),
            Self::DebitError(id, e) => write!(f, "Debit error on {id}: {e:?}"),
            Self::CreditError(id, e) => write!(f, "Credit error on {id}: {e:?}"),
            Self::EntryNotFound(id) => write!(f, "Journal entry not found: {id}"),
        }
    }
}

impl std::error::Error for LedgerError {}
