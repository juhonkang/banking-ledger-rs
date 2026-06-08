//! `SurrealDB` persistence layer for Banking Ledger.
//!
//! Connected via WebSocket to a dedicated `SurrealDB` container.
//! docker compose up → `SurrealDB` on :4321 → this module connects.

use std::sync::Arc;
use surrealdb::engine::remote::ws::{Client, Ws};
use surrealdb::opt::auth::Root;
use surrealdb::Surreal;

use crate::domain::account::{Account, AccountType};
use crate::domain::journal::JournalEntry;
use crate::log::hash_chain::HashChain;

pub struct SurrealStore {
    db: Surreal<Client>,
}

impl SurrealStore {
    /// Connect to `SurrealDB`.
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
    /// Uses parameterized queries — no SQL injection via string interpolation.
    pub async fn save_account_raw(
        &self,
        id: &str,
        atype: &str,
        currency: &str,
        balance_cents: i64,
        hold_cents: i64,
        status: &str,
    ) -> Result<(), String> {
        let sql = "UPSERT type::thing('account', $id) CONTENT {
            account_type: $atype,
            currency: $currency,
            balance_cents: $balance_cents,
            hold_cents: $hold_cents,
            status: $status,
            updated_at: time::now()
        }";

        self.db
            .query(sql)
            .bind(("id", id.to_string()))
            .bind(("atype", atype.to_string()))
            .bind(("currency", currency.to_string()))
            .bind(("balance_cents", balance_cents))
            .bind(("hold_cents", hold_cents))
            .bind(("status", status.to_string()))
            .await
            .map_err(|e| format!("save_account: {e}"))?;

        Ok(())
    }

    /// Load all accounts from `SurrealDB`.
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
                .map_or(AccountType::Asset, |s| match s {
                    "Asset" => AccountType::Asset,
                    "Liability" => AccountType::Liability,
                    "Equity" => AccountType::Equity,
                    "Revenue" => AccountType::Revenue,
                    "Expense" => AccountType::Expense,
                    _ => AccountType::Asset,
                });

            let currency = row["currency"].as_str().unwrap_or("USD").to_string();
            let balance_cents = row["balance_cents"].as_i64().unwrap_or(0);

            // Build account with correct ID from DB (not a random new one)
            let mut acc = Account::new(atype, &currency, balance_cents, None);
            acc.id = id; // restore original DB identity
            accounts.push(acc);
        }

        Ok(accounts)
    }

    /// Save a journal entry using parameterized queries.
    pub async fn save_journal_entry(&self, entry: &JournalEntry) -> Result<(), String> {
        let legs_json = serde_json::to_string(&entry.legs)
            .map_err(|e| format!("serialize legs: {e}"))?;
        let sql = "CREATE journal_entry CONTENT {
            transaction_id: $txn_id,
            sequence_number: $seq,
            description: $desc,
            legs: $legs,
            recorded_at: $recorded_at
        }";

        self.db
            .query(sql)
            .bind(("txn_id", entry.transaction_id.to_string()))
            .bind(("seq", entry.sequence_number))
            .bind(("desc", entry.description.clone()))
            .bind(("legs", legs_json))
            .bind(("recorded_at", entry.recorded_at.to_rfc3339()))
            .await
            .map_err(|e| format!("save_journal_entry: {e}"))?;

        Ok(())
    }

    /// Save the entire hash chain using parameterized queries.
    /// Uses UPSERT instead of DELETE+CREATE to avoid destructive patterns.
    pub async fn save_hash_chain(&self, chain: &HashChain) -> Result<(), String> {
        for block in &chain.blocks {
            let sql = "UPSERT type::thing('hash_block', $idx) CONTENT {
                index_pos: $idx,
                hash: $hash,
                previous_hash: $prev,
                data: $data,
                timestamp: $ts,
                nonce: $nonce
            }";

            self.db
                .query(sql)
                .bind(("idx", block.index))
                .bind(("hash", block.hash.clone()))
                .bind(("prev", block.previous_hash.clone()))
                .bind(("data", block.data.clone()))
                .bind(("ts", block.timestamp.to_rfc3339()))
                .bind(("nonce", block.nonce as i64))
                .await
                .map_err(|e| format!("save_hash_block {}: {e}", block.index))?;
        }

        Ok(())
    }

    /// Health check.
    pub async fn health_check(&self) -> bool {
        self.db.query("SELECT 1").await.is_ok()
    }

    /// Load the hash chain from `SurrealDB`.
    /// Returns a `HashChain` rebuilt from stored blocks, or a fresh chain if empty.
    pub async fn load_hash_chain(&self, signing_key: &[u8]) -> Result<HashChain, String> {
        let mut result = self
            .db
            .query("SELECT * FROM hash_block ORDER BY index_pos ASC")
            .await
            .map_err(|e| format!("load_hash_chain: {e}"))?;

        let raw: Vec<serde_json::Value> = result
            .take::<Vec<serde_json::Value>>(0)
            .map_err(|e| format!("load_hash_chain parse: {e}"))?;

        let mut chain = HashChain::new(signing_key);
        // Skip genesis (index 0) — HashChain::new already creates it
        let mut blocks: Vec<crate::log::hash_chain::HashLink> = Vec::new();

        for row in &raw {
            let idx = row["index_pos"].as_u64().unwrap_or(0);
            if idx == 0 {
                continue; // use the genesis from HashChain::new()
            }
            let hash = row["hash"].as_str().unwrap_or("").to_string();
            let prev = row["previous_hash"].as_str().unwrap_or("").to_string();
            let data = row["data"].as_str().unwrap_or("").to_string();
            let ts = row["timestamp"].as_str().unwrap_or("");
            let nonce = row["nonce"].as_u64().unwrap_or(0);
            let timestamp = chrono::DateTime::parse_from_rfc3339(ts).map_or_else(|_| chrono::Utc::now(), |dt| dt.with_timezone(&chrono::Utc));

            blocks.push(crate::log::hash_chain::HashLink {
                index: idx,
                hash,
                previous_hash: prev,
                data,
                timestamp,
                nonce,
            });
        }

        // Replace chain blocks with loaded ones (keep genesis from new())
        if !blocks.is_empty() {
            // Genesis is already at blocks[0], append the rest
            chain.blocks.extend(blocks);
        }

        Ok(chain)
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
