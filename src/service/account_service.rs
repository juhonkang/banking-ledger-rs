//! Thread-safe account registry backed by DashMap.
//! DashMap provides lock-free reads — ideal for the hot read path.

use dashmap::DashMap;

use crate::domain::account::{
    Account, AccountId, AccountStatus, AccountType, CreditError, DebitError, HoldError,
};

pub struct AccountService {
    accounts: DashMap<AccountId, Account>,
}

impl AccountService {
    pub fn new() -> Self {
        Self {
            accounts: DashMap::new(),
        }
    }

    /// Create a new account with initial balance in cents.
    /// Returns the created Account (cloned from inserted).
    pub fn create_account(
        &self,
        account_type: AccountType,
        currency: &str,
        initial_balance_cents: i64,
        owner_party_id: Option<uuid::Uuid>,
    ) -> Account {
        let account = Account::new(
            account_type,
            currency,
            initial_balance_cents,
            owner_party_id,
        );
        self.accounts.insert(account.id, account.clone());
        // Return the clone (balances are snapshots from insert time)
        account
    }

    /// Perform a debit on the account
    pub fn perform_debit(
        &self,
        account_id: AccountId,
        amount_cents: i64,
    ) -> Result<i64, DebitError> {
        let account = self
            .accounts
            .get(&account_id)
            .ok_or(DebitError::AccountNotOpen(AccountStatus::Closed))?;
        // Note: this error mapping is imperfect; real system would have AccountNotFound
        account.debit(amount_cents)
    }

    /// Perform a credit on the account
    pub fn perform_credit(
        &self,
        account_id: AccountId,
        amount_cents: i64,
    ) -> Result<i64, CreditError> {
        let account = self
            .accounts
            .get(&account_id)
            .ok_or(CreditError::AccountNotOpen(AccountStatus::Closed))?;
        account.credit(amount_cents)
    }

    /// Place a hold on the account
    pub fn place_hold(&self, account_id: AccountId, amount_cents: i64) -> Result<(), HoldError> {
        let account = self
            .accounts
            .get(&account_id)
            .ok_or(HoldError::InvalidAmount)?; // imperfect mapping
        account.place_hold(amount_cents)
    }

    /// Release a hold on the account
    pub fn release_hold(&self, account_id: AccountId, amount_cents: i64) -> Result<(), HoldError> {
        let account = self
            .accounts
            .get(&account_id)
            .ok_or(HoldError::InvalidAmount)?;
        account.release_hold(amount_cents)
    }

    /// Get account info (cloned)
    pub fn get_account(&self, account_id: AccountId) -> Option<Account> {
        self.accounts.get(&account_id).map(|a| a.value().clone())
    }

    /// Set account status
    pub fn set_status(&self, account_id: AccountId, new_status: AccountStatus) -> bool {
        if let Some(account) = self.accounts.get(&account_id) {
            account.set_status(new_status);
            true
        } else {
            false
        }
    }

    /// Get balance in cents
    pub fn get_balance_cents(&self, account_id: AccountId) -> Option<i64> {
        self.accounts.get(&account_id).map(|a| a.balance_cents())
    }

    /// Get all accounts (clone — use sparingly)
    pub fn all(&self) -> Vec<Account> {
        self.accounts.iter().map(|e| e.value().clone()).collect()
    }

    /// Number of accounts
    pub fn count(&self) -> usize {
        self.accounts.len()
    }

    /// For persistence: iterate all accounts
    pub fn for_each(&self, mut f: impl FnMut(&AccountId, &Account)) {
        for entry in self.accounts.iter() {
            f(entry.key(), entry.value());
        }
    }

    /// Direct insert for startup restore (bypasses Account::new)
    pub fn insert_raw(&self, id: AccountId, account: Account) {
        self.accounts.insert(id, account);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashmap_mutation_works() {
        let svc = AccountService::new();
        let acc = svc.create_account(AccountType::Asset, "USD", 1_000_000, None);
        let id = acc.id;
        assert_eq!(acc.balance_cents(), 1_000_000);

        // Credit 500000 — must persist to DashMap
        let new_bal = svc.perform_credit(id, 500_000).unwrap();
        assert_eq!(new_bal, 1_500_000);
        assert_eq!(svc.get_account(id).unwrap().balance_cents(), 1_500_000,
            "credit should persist in DashMap");

        // Debit 300000 — must persist
        let new_bal = svc.perform_debit(id, 300_000).unwrap();
        assert_eq!(new_bal, 1_200_000);
        assert_eq!(svc.get_account(id).unwrap().balance_cents(), 1_200_000,
            "debit should persist in DashMap");
    }
}

impl Default for AccountService {
    fn default() -> Self {
        Self::new()
    }
}
