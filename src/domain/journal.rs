//! Double-entry journal — the immutable core of financial truth.
//! Every financial event impacts at least two accounts, equal and opposite.
//! Σ(debits) ≡ Σ(credits). Journal entries are immutable — corrections are reversals.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::account::AccountId;

pub type JournalEntryId = Uuid;
pub type TransactionId = Uuid;

/// The direction of a financial impact on an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum EntrySide {
    /// Increases Asset/Expense, decreases Liability/Equity/Revenue
    Debit,
    /// Increases Liability/Equity/Revenue, decreases Asset/Expense
    Credit,
}

/// A single leg of a double-entry — one side of the equation.
/// A `JournalEntry` always has at least 2 legs (one debit, one credit).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryLeg {
    pub account_id: AccountId,
    pub side: EntrySide,
    /// Amount in smallest currency unit (cents). Always positive.
    pub amount_cents: i64,
    /// Optional: what this represents in decimal form
    #[serde(skip)]
    pub amount: Option<Decimal>,
}

impl EntryLeg {
    /// Create a debit leg. Panics in debug if amount ≤ 0.
    /// In release, returns an EntryLeg with amount=0 (caller should validate).
    pub fn debit(account_id: AccountId, amount_cents: i64) -> Self {
        debug_assert!(amount_cents > 0, "EntryLeg::debit requires amount_cents > 0, got {}", amount_cents);
        Self {
            account_id,
            side: EntrySide::Debit,
            // Use saturating to 0 in release — caller is responsible for validation.
            // A 0-amount leg will be caught by JournalEntry::new() MissingSide check.
            amount_cents: if amount_cents > 0 { amount_cents } else { 0 },
            amount: None,
        }
    }

    /// Create a credit leg. Panics in debug if amount ≤ 0.
    /// In release, returns an EntryLeg with amount=0 (caller should validate).
    pub fn credit(account_id: AccountId, amount_cents: i64) -> Self {
        debug_assert!(amount_cents > 0, "EntryLeg::credit requires amount_cents > 0, got {}", amount_cents);
        Self {
            account_id,
            side: EntrySide::Credit,
            // Use saturating to 0 in release — caller is responsible for validation.
            // A 0-amount leg will be caught by JournalEntry::new() MissingSide check.
            amount_cents: if amount_cents > 0 { amount_cents } else { 0 },
            amount: None,
        }
    }
}

/// A journal entry — the immutable record of a financial event.
/// Once created, NEVER modified. The foundation of audit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    pub id: JournalEntryId,
    /// The transaction this entry belongs to
    pub transaction_id: TransactionId,
    /// Sequence number within the journal (monotonically increasing)
    pub sequence_number: u64,
    /// All legs — must balance (sum debits == sum credits)
    pub legs: Vec<EntryLeg>,
    /// Human-readable description (e.g., "Transfer from savings to checking")
    pub description: String,
    /// When this was recorded (immutable)
    pub recorded_at: DateTime<Utc>,
    /// Reference to a reversing/correcting entry if applicable
    pub reverses: Option<JournalEntryId>,
}

impl JournalEntry {
    /// Create a new journal entry. Validates that debits = credits.
    pub fn new(
        transaction_id: TransactionId,
        sequence_number: u64,
        legs: Vec<EntryLeg>,
        description: &str,
    ) -> Result<Self, JournalError> {
        // Must have at least 2 legs
        if legs.len() < 2 {
            return Err(JournalError::NotEnoughLegs);
        }

        // Validate balance: sum(debits) == sum(credits)
        // Use i128 to prevent overflow in large journal entries
        let total_debits: i128 = legs
            .iter()
            .filter(|l| l.side == EntrySide::Debit)
            .map(|l| i128::from(l.amount_cents))
            .sum();

        let total_credits: i128 = legs
            .iter()
            .filter(|l| l.side == EntrySide::Credit)
            .map(|l| i128::from(l.amount_cents))
            .sum();
        // Must have at least one debit and one credit
        if total_debits == 0 || total_credits == 0 {
            return Err(JournalError::MissingSide);
        }

        // Validate balance: sum(debits) == sum(credits)
        if total_debits != total_credits {
            let total_debits_i64: i64 = total_debits.try_into().unwrap_or(i64::MAX);
            let total_credits_i64: i64 = total_credits.try_into().unwrap_or(i64::MAX);
            return Err(JournalError::Unbalanced {
                total_debits: total_debits_i64,
                total_credits: total_credits_i64,
            });
        }

        Ok(Self {
            id: Uuid::now_v7(),
            transaction_id,
            sequence_number,
            legs,
            description: description.to_string(),
            recorded_at: Utc::now(),
            reverses: None,
        })
    }

    /// Create a reversing entry (e.g., to correct an error).
    /// Flips all debits to credits and vice versa.
    /// Returns Err if the original entry is invalid (unbalanced).
    pub fn reverse(&self, new_transaction_id: TransactionId, sequence_number: u64) -> Result<Self, JournalError> {
        let reversed_legs: Vec<EntryLeg> = self
            .legs
            .iter()
            .map(|leg| EntryLeg {
                account_id: leg.account_id,
                side: match leg.side {
                    EntrySide::Debit => EntrySide::Credit,
                    EntrySide::Credit => EntrySide::Debit,
                },
                amount_cents: leg.amount_cents,
                amount: leg.amount,
            })
            .collect();

        let mut reversal = Self::new(
            new_transaction_id,
            sequence_number,
            reversed_legs,
            &format!("REVERSAL of {}", self.id),
        )?;
        reversal.reverses = Some(self.id);
        Ok(reversal)
    }

    /// Verify this entry is still balanced (tamper detection).
    /// Uses i128 summation to prevent silent overflow.
    pub fn verify_balance(&self) -> bool {
        let debits: i128 = self
            .legs
            .iter()
            .filter(|l| l.side == EntrySide::Debit)
            .map(|l| i128::from(l.amount_cents))
            .sum();
        let credits: i128 = self
            .legs
            .iter()
            .filter(|l| l.side == EntrySide::Credit)
            .map(|l| i128::from(l.amount_cents))
            .sum();
        debits == credits && debits > 0
    }
}

/// A business transaction — may produce multiple journal entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub id: TransactionId,
    /// Human-readable reference (e.g., "PAY-2026-001")
    pub reference: String,
    pub status: TransactionStatus,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionStatus {
    Pending,
    Committed,
    Rejected,
    Reversed,
}

impl Transaction {
    pub fn new(reference: &str) -> Self {
        Self {
            id: Uuid::now_v7(),
            reference: reference.to_string(),
            status: TransactionStatus::Pending,
            created_at: Utc::now(),
            completed_at: None,
        }
    }

    /// Commit this transaction. Only valid for Pending transactions.
    /// Returns false if already in a terminal state.
    pub fn commit(&mut self) -> bool {
        if self.status != TransactionStatus::Pending {
            return false;
        }
        self.status = TransactionStatus::Committed;
        self.completed_at = Some(Utc::now());
        true
    }

    /// Reject this transaction. Only valid for Pending transactions.
    /// Returns false if already in a terminal state.
    pub fn reject(&mut self) -> bool {
        if self.status != TransactionStatus::Pending {
            return false;
        }
        self.status = TransactionStatus::Rejected;
        self.completed_at = Some(Utc::now());
        true
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum JournalError {
    NotEnoughLegs,
    MissingSide,
    Unbalanced {
        total_debits: i64,
        total_credits: i64,
    },
}

impl std::fmt::Display for JournalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotEnoughLegs => write!(f, "Journal entry must have at least 2 legs"),
            Self::MissingSide => write!(
                f,
                "Journal entry must have at least one debit and one credit"
            ),
            Self::Unbalanced {
                total_debits,
                total_credits,
            } => {
                write!(
                    f,
                    "Journal entry unbalanced: debits={total_debits}, credits={total_credits}"
                )
            }
        }
    }
}

impl std::error::Error for JournalError {}
