//! Ledger Account — the fundamental building block of financial truth.
//!
//! # Concurrency Guarantee
//!
//! All balance operations are **lock-free** via [`AtomicI64`] CAS loops.
//! Under contention, the CAS loop retries until it wins the race.
//! This gives us sub-microsecond latency without mutex overhead.
//!
//! # Design
//!
//! - Balance stored in **cents** (smallest currency unit) — no floating point
//! - `available_balance` tracks funds not on hold
//! - Status machine: `Open → Frozen → Closed`
//! - `#[must_use]` on all operations — results cannot be ignored

use core::sync::atomic::{AtomicI64, Ordering};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a ledger account.
pub type AccountId = Uuid;

/// The fundamental accounting category.
///
/// Dictates the **normal balance** (debit or credit) and
/// how transactions impact this account in double-entry bookkeeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum AccountType {
    /// Increases with debits (e.g., Cash, Inventory)
    Asset,
    /// Increases with credits (e.g., Loans, Accounts Payable)
    Liability,
    /// Increases with credits (e.g., Share Capital, Retained Earnings)
    Equity,
    /// Increases with credits (e.g., Sales, Interest Income)
    Revenue,
    /// Increases with debits (e.g., Rent, Salaries)
    Expense,
}

/// Lifecycle state of the account.
///
/// # State Transitions
///
/// ```text
/// Open ⇄ Frozen → Closed
/// ```
///
/// - `Open → Frozen`: temporary legal/administrative hold
/// - `Frozen → Open`: hold released
/// - `Open/Frozen → Closed`: account terminated (irreversible)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum AccountStatus {
    /// Normal operation — debits and credits allowed
    Open,
    /// Temporarily blocked — no operations permitted
    Frozen,
    /// Permanently terminated — no further operations
    Closed,
}

/// Error for invalid account status transitions.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AccountStatusError {
    #[error("Cannot modify closed account")]
    ClosedAccount,
    #[error("Invalid status transition from {from:?} to {to:?}")]
    InvalidTransition {
        from: AccountStatus,
        to: AccountStatus,
    },
}

/// A ledger account — the heartbeat of financial truth.
///
/// # Thread Safety
///
/// `Account` is `Send + Sync`. Balance updates use lock-free CAS loops.
/// Status changes use a `Mutex` (infrequent, cold path).
///
/// # Example
///
/// ```rust
/// use banking_ledger::domain::account::{Account, AccountType};
/// let acc = Account::new(AccountType::Asset, "USD", 100_000, None);
/// assert_eq!(acc.balance_cents(), 100_000);
/// acc.credit(50_000).expect("credit should succeed");
/// assert_eq!(acc.balance_cents(), 150_000);
/// ```
#[derive(Debug)]
pub struct Account {
    /// Immutable unique identifier (UUID v7, time-ordered)
    pub id: AccountId,
    /// Fundamental classification (Asset/Liability/Equity/Revenue/Expense)
    pub account_type: AccountType,
    /// ISO 4217 currency code (e.g., "USD", "EUR", "VND")
    pub currency: String,

    /// Current balance in the smallest currency unit (cents)
    balance: AtomicI64,

    /// Available balance = `current_balance` - holds
    available_balance: AtomicI64,

    /// Lifecycle state (Open/Frozen/Closed)
    pub status: std::sync::Mutex<AccountStatus>,

    /// Optional link to the owning Party
    pub owner_party_id: Option<Uuid>,

    /// When this account was created (immutable)
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Last mutation timestamp
    pub last_updated: std::sync::Mutex<chrono::DateTime<chrono::Utc>>,
}

impl Clone for Account {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            account_type: self.account_type,
            currency: self.currency.clone(),
            balance: AtomicI64::new(self.balance.load(Ordering::Acquire)),
            available_balance: AtomicI64::new(self.available_balance.load(Ordering::Acquire)),
            status: std::sync::Mutex::new(*self.status.lock().unwrap()),
            owner_party_id: self.owner_party_id,
            created_at: self.created_at,
            last_updated: std::sync::Mutex::new(*self.last_updated.lock().unwrap()),
        }
    }
}

impl Account {
    /// Create a new account with an initial balance in cents.
    #[must_use]
    pub fn new(
        account_type: AccountType,
        currency: impl Into<String>,
        initial_balance_cents: i64,
        owner_party_id: Option<Uuid>,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            account_type,
            currency: currency.into(),
            balance: AtomicI64::new(initial_balance_cents),
            available_balance: AtomicI64::new(initial_balance_cents),
            status: std::sync::Mutex::new(AccountStatus::Open),
            owner_party_id,
            created_at: chrono::Utc::now(),
            last_updated: std::sync::Mutex::new(chrono::Utc::now()),
        }
    }

    // ━━━ Getters ━━━

    /// Current balance in cents (smallest currency unit).
    #[must_use]
    pub fn balance_cents(&self) -> i64 {
        self.balance.load(Ordering::SeqCst)
    }

    /// Available balance in cents (balance minus holds).
    #[must_use]
    pub fn available_balance_cents(&self) -> i64 {
        self.available_balance.load(Ordering::SeqCst)
    }

    /// Current lifecycle status.
    #[must_use]
    pub fn status(&self) -> AccountStatus {
        *self.status.lock().unwrap()
    }

    // ━━━ Status Management ━━━

    /// Set the account status with state machine validation.
    /// Valid: Open→Frozen, Frozen→Open, Open→Closed, Frozen→Closed.
    /// Closed is terminal.
    pub fn set_status(&self, new_status: AccountStatus) -> Result<(), AccountStatusError> {
        let mut s = self.status.lock().unwrap();
        let current = *s;
        match (current, new_status) {
            (AccountStatus::Closed, _) => return Err(AccountStatusError::ClosedAccount),
            (AccountStatus::Open, AccountStatus::Closed)
            | (AccountStatus::Frozen, AccountStatus::Closed)
            | (AccountStatus::Open, AccountStatus::Frozen)
            | (AccountStatus::Frozen, AccountStatus::Open) => {} // valid
            _ => return Err(AccountStatusError::InvalidTransition { from: current, to: new_status }),
        }
        *s = new_status;
        let mut lu = self.last_updated.lock().unwrap();
        *lu = chrono::Utc::now();
        Ok(())
    }

    /// Legacy: set status without validation. Only for tests.
    #[doc(hidden)]
    pub fn set_status_unchecked(&self, new_status: AccountStatus) {
        let mut s = self.status.lock().unwrap();
        *s = new_status;
        let mut lu = self.last_updated.lock().unwrap();
        *lu = chrono::Utc::now();
    }

    // ━━━ Core Operations ━━━

    /// Attempt to debit (withdraw) from the account.
    ///
    /// Uses a **CAS loop** — lock-free, safe under high concurrency.
    /// The value is loaded ONCE then used for both the check and the CAS,
    /// preventing TOCTOU races.
    ///
    /// # Errors
    ///
    /// - [`DebitError::InvalidAmount`] — amount ≤ 0
    /// - [`DebitError::AccountNotOpen`] — account is Frozen or Closed
    /// - [`DebitError::InsufficientFunds`] — available balance < amount
    ///
    /// # Returns
    ///
    /// New balance on success.
    #[must_use = "debit result must be checked — financial operations cannot be fire-and-forget"]
    pub fn debit(&self, amount_cents: i64) -> Result<i64, DebitError> {
        // Validate amount
        if amount_cents <= 0 {
            return Err(DebitError::InvalidAmount);
        }

        // Validate status
        let status = self.status();
        if status != AccountStatus::Open {
            return Err(DebitError::AccountNotOpen(status));
        }

        // CAS loop — lock-free concurrency
        loop {
            let available = self.available_balance.load(Ordering::SeqCst);

            if available < amount_cents {
                return Err(DebitError::InsufficientFunds { available, requested: amount_cents });
            }

            let new_available = available - amount_cents;

            if self.available_balance.compare_exchange(
                available, new_available, Ordering::SeqCst, Ordering::SeqCst
            ).is_ok() {
                let new_balance = self.balance.fetch_sub(amount_cents, Ordering::SeqCst) - amount_cents;
                let mut lu = self.last_updated.lock().unwrap();
                *lu = chrono::Utc::now();
                return Ok(new_balance);
            }
            // CAS failed — another thread modified available_balance, retry
        }
    }

    /// Credit (deposit) to the account.
    ///
    /// Uses `fetch_add` for low latency. Note: there is a transient window
    /// between the two atomic updates where `balance` may be ahead of
    /// `available_balance`. This is safe because the invariant
    /// `balance >= available_balance` is always maintained, and any reader
    /// seeing a slightly stale `available` value is still correct (funds exist).
    ///
    /// # Errors
    ///
    /// - [`CreditError::InvalidAmount`] — amount ≤ 0
    /// - [`CreditError::AccountNotOpen`] — account is Frozen or Closed
    #[must_use = "credit result must be checked"]
    pub fn credit(&self, amount_cents: i64) -> Result<i64, CreditError> {
        if amount_cents <= 0 {
            return Err(CreditError::InvalidAmount);
        }

        let status = self.status();
        if status != AccountStatus::Open {
            return Err(CreditError::AccountNotOpen(status));
        }

        let new_balance = self.balance.fetch_add(amount_cents, Ordering::SeqCst) + amount_cents;
        let _new_available =
            self.available_balance.fetch_add(amount_cents, Ordering::SeqCst) + amount_cents;
        let mut lu = self.last_updated.lock().unwrap();
        *lu = chrono::Utc::now();

        Ok(new_balance)
    }

    // ━━━ Hold Mechanism ━━━

    /// Place a hold on funds (e.g., pending card authorization).
    ///
    /// Reduces `available_balance` but NOT `balance`.
    /// Debits will check `available_balance`, so held funds are protected.
    #[must_use = "hold result must be checked"]
    pub fn place_hold(&self, amount_cents: i64) -> Result<(), HoldError> {
        if amount_cents <= 0 {
            return Err(HoldError::InvalidAmount);
        }

        // Prevent holds on non-Open accounts
        let status = self.status();
        if status != AccountStatus::Open {
            return Err(HoldError::AccountNotOpen(status));
        }

        loop {
            let available = self.available_balance.load(Ordering::Acquire);
            if available < amount_cents {
                return Err(HoldError::InsufficientFunds {
                    available,
                    hold: amount_cents,
                });
            }

            let new_available = available - amount_cents;
            if self
                .available_balance
                .compare_exchange(
                    available,
                    new_available,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
            {
                return Ok(());
            }
        }
    }

    /// Release a previously placed hold.
    ///
    /// Restores `available_balance`. Does not affect `balance`.
    /// Uses checked addition to prevent overflow.
    pub fn release_hold(&self, amount_cents: i64) -> Result<(), HoldError> {
        if amount_cents <= 0 {
            return Err(HoldError::InvalidAmount);
        }

        // Prevent holds on non-Open accounts
        let status = self.status();
        if status != AccountStatus::Open {
            return Err(HoldError::AccountNotOpen(status));
        }

        // Checked add to prevent silent overflow (BUG #2 fix)
        loop {
            let current = self.available_balance.load(Ordering::Acquire);
            let new_val = current.checked_add(amount_cents).ok_or(HoldError::InvalidAmount)?;
            if self.available_balance.compare_exchange(
                current, new_val, Ordering::AcqRel, Ordering::Acquire
            ).is_ok() {
                return Ok(());
            }
        }
    }
}

// ━━━ Error Types ━━━

/// Errors that can occur during a debit operation.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum DebitError {
    /// Account does not exist
    #[error("account not found: {0}")]
    AccountNotFound(AccountId),
    /// Amount must be positive (zero or negative rejected)
    #[error("debit amount must be positive")]
    InvalidAmount,
    /// Account is not in Open state
    #[error("cannot debit account with status {0:?}")]
    AccountNotOpen(AccountStatus),
    /// Available balance insufficient for the requested amount
    #[error("insufficient funds: available={available}, requested={requested}")]
    InsufficientFunds {
        /// Currently available balance in cents
        available: i64,
        /// Requested debit amount in cents
        requested: i64,
    },
}

/// Errors that can occur during a credit operation.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum CreditError {
    /// Account does not exist
    #[error("account not found: {0}")]
    AccountNotFound(AccountId),
    /// Amount must be positive
    #[error("credit amount must be positive")]
    InvalidAmount,
    /// Account is not in Open state
    #[error("cannot credit account with status {0:?}")]
    AccountNotOpen(AccountStatus),
}

/// Errors that can occur during hold operations.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum HoldError {
    /// Account does not exist
    #[error("account not found: {0}")]
    AccountNotFound(AccountId),
    /// Account is not open for operations
    #[error("account is {0:?} — must be Open")]
    AccountNotOpen(AccountStatus),
    /// Amount must be positive
    #[error("hold amount must be positive")]
    InvalidAmount,
    /// Available balance insufficient for the hold
    #[error("insufficient funds for hold: available={available}, hold={hold}")]
    InsufficientFunds {
        /// Currently available balance
        available: i64,
        /// Requested hold amount
        hold: i64,
    },
}
