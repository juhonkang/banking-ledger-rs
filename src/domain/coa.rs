//! Chart of Accounts — hierarchical account registry.
//! Semantic contract for the entire ledger: defines what every account means,
//! its normal balance, and its place in the financial statement hierarchy.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type CoaAccountId = Uuid;

/// The fundamental accounting classification.
/// Drives debit/credit semantics and financial statement placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoaCategory {
    Asset,
    Liability,
    Equity,
    Revenue,
    Expense,
}

/// Whether an increase to this account is normally a debit or credit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NormalBalance {
    /// Assets and Expenses increase with debits
    Debit,
    /// Liabilities, Equity, Revenue increase with credits
    Credit,
}

impl CoaCategory {
    pub fn normal_balance(&self) -> NormalBalance {
        match self {
            Self::Asset | Self::Expense => NormalBalance::Debit,
            Self::Liability | Self::Equity | Self::Revenue => NormalBalance::Credit,
        }
    }
}

/// A single account in the Chart of Accounts hierarchy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoaAccount {
    pub id: CoaAccountId,
    /// Short code (e.g., "1000", "1100-001")
    pub code: String,
    pub name: String,
    pub category: CoaCategory,
    /// Parent in the hierarchy (None for root-level accounts)
    pub parent_id: Option<CoaAccountId>,
    pub normal_balance: NormalBalance,
    pub description: String,
    pub status: CoaAccountStatus,
    /// Version of the COA when this account was created
    pub coa_version: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoaAccountStatus {
    Active,
    Inactive,
    /// Proposed but not yet approved
    Proposed,
}

impl CoaAccount {
    pub fn new(
        code: &str,
        name: &str,
        category: CoaCategory,
        parent_id: Option<CoaAccountId>,
        coa_version: u32,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            code: code.to_string(),
            name: name.to_string(),
            category,
            parent_id,
            normal_balance: category.normal_balance(),
            description: String::new(),
            status: CoaAccountStatus::Active,
            coa_version,
        }
    }
}

/// The Chart of Accounts — hierarchical registry of all accounts.
/// Acts as semantic contract across all financial services.
pub struct ChartOfAccounts {
    accounts: Vec<CoaAccount>,
    version: u32,
}

impl ChartOfAccounts {
    pub fn new(version: u32) -> Self {
        Self {
            accounts: Vec::new(),
            version,
        }
    }

    /// Add an account to the COA
    pub fn add_account(&mut self, account: CoaAccount) -> CoaAccountId {
        let id = account.id;
        self.accounts.push(account);
        id
    }

    /// Find by code
    pub fn find_by_code(&self, code: &str) -> Option<&CoaAccount> {
        self.accounts.iter().find(|a| a.code == code)
    }

    /// Find by ID
    pub fn find_by_id(&self, id: CoaAccountId) -> Option<&CoaAccount> {
        self.accounts.iter().find(|a| a.id == id)
    }

    /// List all accounts of a given category
    pub fn by_category(&self, category: CoaCategory) -> Vec<&CoaAccount> {
        self.accounts
            .iter()
            .filter(|a| a.category == category && a.status == CoaAccountStatus::Active)
            .collect()
    }

    /// Get all active accounts
    pub fn active_accounts(&self) -> Vec<&CoaAccount> {
        self.accounts
            .iter()
            .filter(|a| a.status == CoaAccountStatus::Active)
            .collect()
    }

    /// Deactivate an account (never delete)
    pub fn deactivate(&mut self, id: CoaAccountId) -> Result<(), CoaError> {
        let account = self
            .accounts
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or(CoaError::NotFound(id))?;
        account.status = CoaAccountStatus::Inactive;
        Ok(())
    }

    /// Check for circular parent references before adding
    pub fn validate_no_circular_ref(&self, account: &CoaAccount) -> bool {
        let mut current = account.parent_id;
        let mut visited = std::collections::HashSet::new();
        visited.insert(account.id);

        while let Some(pid) = current {
            if visited.contains(&pid) {
                return false; // circular!
            }
            visited.insert(pid);
            current = self.find_by_id(pid).and_then(|a| a.parent_id);
        }
        true
    }
}

#[derive(Debug)]
pub enum CoaError {
    NotFound(CoaAccountId),
    CircularReference(CoaAccountId),
}

impl std::fmt::Display for CoaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "COA account not found: {id}"),
            Self::CircularReference(id) => write!(f, "Circular reference detected for: {id}"),
        }
    }
}

impl std::error::Error for CoaError {}
