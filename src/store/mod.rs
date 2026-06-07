//! SurrealDB persistence layer for Banking Ledger.
//!
//! Connected via WebSocket to a dedicated SurrealDB container.
//! docker compose up → SurrealDB on :4321 → this module connects.

use std::sync::Arc;
use surrealdb::engine::remote::ws::{Client, Ws};
use surrealdb::opt::auth::Root;
use surrealdb::Surreal;

use crate::domain::account::{Account, AccountId, AccountType, AccountStatus};
use crate::domain::journal::JournalEntry;
use crate::log::hash_chain::HashChain;

pub struct SurrealStore {
    db: Surreal<Client>,
}

impl SurrealStore {
    /// Connect to SurrealDB.
    pub async fn connect(
        url: &str,
        ns: &str,
        db_name: &str,
        user: &str,
        pass: &str,
    ) -> Result<Self, String> {
        let db = Surreal::new::<Ws>(url)
            .await
            .map_err(|e| format!("SurrealDB connect {url}: {e}"))?;

        db.signin(Root {
            username: user.to_string(),
            password: pass.to_string(),
        })
        .await
        .map_err(|e| format!("SurrealDB signin: {e}"))?;

        db.use_ns(ns)
            .await
            .map_err(|e| format!("SurrealDB use_ns({ns}): {e}"))?;
        db.use_db(db_name)
            .await
            .map_err(|e| format!("SurrealDB use_db({db_name}): {e}"))?;

        eprintln!("  SurrealDB ready: {ns}/{db_name}");

        Ok(Self { db })
    }

    /// Save an account (upsert).
    pub async fn save_account(&self, account: &Account) -> Result<(), String> {
        self.save_account_raw(
            &account.id.to_string(),
            &format!("{:?}", account.account_type),
            &account.currency,
            account.balance_cents(),
            account.available_balance_cents(),
            &format!("{:?}", account.status()),
        )
        .await
    }

    /// Save an account from raw field values (no Account struct needed).
    pub async fn save_account_raw(
        &self,
        id: &str,
        atype: &str,
        currency: &str,
        balance_cents: i64,
        hold_cents: i64,
        status: &str,
    ) -> Result<(), String> {
        let sql = format!(
            "UPSERT account:{id} CONTENT {{ \
                account_type: '{atype}', \
                currency: '{currency}', \
                balance_cents: {balance_cents}, \
                hold_cents: {hold_cents}, \
                status: '{status}', \
                updated_at: time::now() \
            }}"
        );

        self.db
            .query(&sql)
            .await
            .map_err(|e| format!("save_account: {e}"))?;

        Ok(())
    }

    /// Load all accounts from SurrealDB.
    pub async fn load_all_accounts(&self) -> Result<Vec<Account>, String> {
        let mut result = self
            .db
            .query("SELECT * FROM account")
            .await
            .map_err(|e| format!("load_accounts: {e}"))?;

        let raw: Vec<serde_json::Value> = result
            .take::<Vec<serde_json::Value>>(0)
            .map_err(|e| format!("load_accounts parse: {e}"))?;

        let mut accounts = Vec::new();
        for row in raw {
            let id_str = row["id"].as_str().unwrap_or_default();
            // Parse "account:uuid" → uuid
            let uuid_part = id_str.split(':').nth(1).unwrap_or(id_str);
            let id = uuid::Uuid::parse_str(uuid_part).map_err(|e| format!("uuid parse: {e}"))?;

            let atype = row["account_type"]
                .as_str()
                .map(|s| match s {
                    "Asset" => AccountType::Asset,
                    "Liability" => AccountType::Liability,
                    "Equity" => AccountType::Equity,
                    "Revenue" => AccountType::Revenue,
                    "Expense" => AccountType::Expense,
                    _ => AccountType::Asset,
                })
                .unwrap_or(AccountType::Asset);

            let currency = row["currency"].as_str().unwrap_or("USD").to_string();
            let balance_cents = row["balance_cents"].as_i64().unwrap_or(0);

            let acc = Account::new(atype, &currency, balance_cents, Some(id));
            accounts.push(acc);
        }

        Ok(accounts)
    }

    /// Save a journal entry.
    pub async fn save_journal_entry(&self, entry: &JournalEntry) -> Result<(), String> {
        let legs_json = serde_json::to_string(&entry.legs)
            .map_err(|e| format!("serialize legs: {e}"))?;
        let sql = format!(
            "CREATE journal_entry CONTENT {{ \
                transaction_id: '{}', \
                sequence_number: {}, \
                description: '{}', \
                legs: {}, \
                recorded_at: '{}' \
            }}",
            entry.transaction_id,
            entry.sequence_number,
            entry.description.replace('\'', "''"),
            legs_json,
            entry.recorded_at.to_rfc3339(),
        );

        self.db
            .query(&sql)
            .await
            .map_err(|e| format!("save_journal_entry: {e}"))?;

        Ok(())
    }

    /// Save the entire hash chain.
    pub async fn save_hash_chain(&self, chain: &HashChain) -> Result<(), String> {
        // Clear existing blocks
        self.db
            .query("DELETE hash_block")
            .await
            .map_err(|e| format!("delete hash_blocks: {e}"))?;

        for block in &chain.blocks {
            let sql = format!(
                "CREATE hash_block CONTENT {{ \
                    index_pos: {}, \
                    hash: '{}', \
                    previous_hash: '{}', \
                    data: '{}', \
                    timestamp: '{}', \
                    nonce: {} \
                }}",
                block.index,
                block.hash,
                block.previous_hash,
                block.data.replace('\'', "''"),
                block.timestamp.to_rfc3339(),
                block.nonce,
            );

            self.db
                .query(&sql)
                .await
                .map_err(|e| format!("save_hash_block {}: {e}", block.index))?;
        }

        Ok(())
    }

    /// Health check.
    pub async fn health_check(&self) -> bool {
        self.db.query("SELECT 1").await.is_ok()
    }
}

/// Persist state after every mutation (fire-and-forget for now).
pub async fn persist_if_configured(
    store: &Option<Arc<SurrealStore>>,
    accounts: &[Account],
    journal: &[JournalEntry],
    hash_chain: &HashChain,
) {
    if let Some(ref s) = store {
        for acc in accounts {
            let _ = s.save_account(acc).await;
        }
        for entry in journal.iter().rev().take(5) {
            // Last 5 entries
            let _ = s.save_journal_entry(entry).await;
        }
        let _ = s.save_hash_chain(hash_chain).await;
    }
}
