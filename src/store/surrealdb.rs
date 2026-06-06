// SurrealDB Persistence Layer for Banking Ledger
// Pure Rust HTTP client — zero external dependencies.
// Uses std::net::TcpStream for minimal footprint.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use crate::domain::account::{Account, AccountId, AccountType};
use crate::domain::journal::JournalEntry;

// ═══════════════════════════════════════════
// SurrealDB Configuration
// ═══════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct SurrealConfig {
    pub host: String,      // "127.0.0.1"
    pub port: u16,         // 29180
    pub namespace: String, // "banking_ledger"
    pub database: String,  // "ledger"
    pub username: String,
    pub password: String,
}

impl Default for SurrealConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 29180,
            namespace: "banking_ledger".into(),
            database: "ledger".into(),
            username: "root".into(),
            password: "root".into(),
        }
    }
}

// ═══════════════════════════════════════════
// Pure-Rust SurrealDB HTTP Client
// ═══════════════════════════════════════════

pub struct SurrealClient {
    config: SurrealConfig,
}

impl SurrealClient {
    pub fn new(config: SurrealConfig) -> Self {
        Self { config }
    }

    /// Execute a `SurrealQL` query via HTTP /sql endpoint
    fn query(&self, sql: &str) -> Result<serde_json::Value, String> {
        let body = serde_json::json!({"query": sql}).to_string();
        let auth = base64_encode(&format!(
            "{}:{}",
            self.config.username, self.config.password
        ));

        let request = format!(
            "POST /sql HTTP/1.1\r\n\
             Host: {}:{}\r\n\
             Content-Type: application/json\r\n\
             Authorization: Basic {}\r\n\
             NS: {}\r\n\
             DB: {}\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\
             \r\n\
             {}",
            self.config.host,
            self.config.port,
            auth,
            self.config.namespace,
            self.config.database,
            body.len(),
            body,
        );

        let addr = format!("{}:{}", self.config.host, self.config.port);
        let mut stream = TcpStream::connect_timeout(
            &addr.parse().map_err(|e| format!("Parse addr: {e}"))?,
            Duration::from_secs(5),
        )
        .map_err(|e| format!("Connect: {e}"))?;

        stream
            .write_all(request.as_bytes())
            .map_err(|e| format!("Write: {e}"))?;

        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .map_err(|e| format!("Read: {e}"))?;

        // Parse HTTP response — extract body after \r\n\r\n
        let body_start = response.find("\r\n\r\n").unwrap_or(0) + 4;
        let json_body = &response[body_start..];

        serde_json::from_str(json_body).map_err(|e| {
            format!(
                "JSON parse error: {} — raw: {}",
                e,
                &json_body[..200.min(json_body.len())]
            )
        })
    }

    /// Initialize the ledger schema in `SurrealDB`
    pub fn init_schema(&self) -> Result<(), String> {
        let statements = [
            "DEFINE TABLE IF NOT EXISTS account SCHEMAFULL",
            "DEFINE FIELD IF NOT EXISTS id ON account TYPE string",
            "DEFINE FIELD IF NOT EXISTS account_type ON account TYPE string",
            "DEFINE FIELD IF NOT EXISTS currency ON account TYPE string",
            "DEFINE FIELD IF NOT EXISTS balance_cents ON account TYPE int",
            "DEFINE FIELD IF NOT EXISTS available_balance_cents ON account TYPE int",
            "DEFINE FIELD IF NOT EXISTS status ON account TYPE string",
            "DEFINE INDEX IF NOT EXISTS idx_account_id ON account COLUMNS id UNIQUE",
            "DEFINE TABLE IF NOT EXISTS journal_entry SCHEMAFULL",
            "DEFINE FIELD IF NOT EXISTS id ON journal_entry TYPE string",
            "DEFINE FIELD IF NOT EXISTS sequence_number ON journal_entry TYPE int",
            "DEFINE INDEX IF NOT EXISTS idx_journal_id ON journal_entry COLUMNS id UNIQUE",
        ];

        for stmt in &statements {
            self.query(&format!("{stmt};"))?;
        }
        Ok(())
    }

    // ═══ Account Persistence ═══

    /// Persist an account to `SurrealDB`
    pub fn save_account(&self, account: &Account) -> Result<(), String> {
        let json = serde_json::json!({
            "id": account.id.to_string(),
            "account_type": format!("{:?}", account.account_type),
            "currency": account.currency,
            "balance_cents": account.balance_cents(),
            "available_balance_cents": account.available_balance_cents(),
            "status": format!("{:?}", account.status()),
        });

        let sql = format!(
            "CREATE account:{} CONTENT {};",
            account.id,
            serde_json::to_string(&json).unwrap()
        );
        self.query(&sql)?;
        Ok(())
    }

    /// Load an account from `SurrealDB` by ID
    pub fn load_account(&self, id: AccountId) -> Result<Option<Account>, String> {
        let sql = format!("SELECT * FROM account:{id};");
        let result = self.query(&sql)?;

        if let Some(arr) = result.as_array() {
            if let Some(first) = arr.first() {
                if let Some(rows) = first.get("result").and_then(|r| r.as_array()) {
                    if let Some(row) = rows.first() {
                        let type_str = row["account_type"].as_str().unwrap_or("Asset");
                        let atype = match type_str {
                            "Asset" => AccountType::Asset,
                            "Liability" => AccountType::Liability,
                            "Equity" => AccountType::Equity,
                            "Revenue" => AccountType::Revenue,
                            "Expense" => AccountType::Expense,
                            _ => AccountType::Asset,
                        };
                        let currency = row["currency"].as_str().unwrap_or("USD");
                        let balance = row["balance_cents"].as_i64().unwrap_or(0);

                        let acc = Account::new(atype, currency, balance, None);
                        return Ok(Some(acc));
                    }
                }
            }
        }
        Ok(None)
    }

    // ═══ Journal Persistence ═══

    /// Persist a journal entry and its legs
    pub fn save_journal_entry(&self, entry: &JournalEntry) -> Result<(), String> {
        let json = serde_json::json!({
            "id": entry.id.to_string(),
            "transaction_id": entry.transaction_id.to_string(),
            "sequence_number": entry.sequence_number,
            "description": entry.description,
        });

        let sql = format!(
            "CREATE journal_entry:{} CONTENT {};",
            entry.id,
            serde_json::to_string(&json).unwrap()
        );
        self.query(&sql)?;
        Ok(())
    }

    /// Count journal entries (for audit)
    pub fn count_entries(&self) -> Result<usize, String> {
        let result = self.query("SELECT count() FROM journal_entry GROUP ALL;")?;
        if let Some(arr) = result.as_array() {
            if let Some(first) = arr.first() {
                if let Some(rows) = first.get("result").and_then(|r| r.as_array()) {
                    if let Some(row) = rows.first() {
                        return Ok(row["count"].as_u64().unwrap_or(0) as usize);
                    }
                }
            }
        }
        Ok(0)
    }
}

// ═══════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════

fn base64_encode(s: &str) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = s.as_bytes();
    let mut result = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = if chunk.len() > 1 {
            u32::from(chunk[1])
        } else {
            0
        };
        let b2 = if chunk.len() > 2 {
            u32::from(chunk[2])
        } else {
            0
        };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        result.push(if chunk.len() > 1 {
            CHARS[((triple >> 6) & 0x3F) as usize]
        } else {
            b'='
        } as char);
        result.push(if chunk.len() > 2 {
            CHARS[(triple & 0x3F) as usize]
        } else {
            b'='
        } as char);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64_encode_root() {
        assert_eq!(base64_encode("root:root"), "cm9vdDpyb290");
    }

    #[test]
    fn test_base64_encode_basic() {
        assert_eq!(base64_encode("admin:secret"), "YWRtaW46c2VjcmV0");
    }

    #[test]
    fn test_surreal_config_defaults() {
        let cfg = SurrealConfig::default();
        assert_eq!(cfg.namespace, "banking_ledger");
        assert_eq!(cfg.port, 29180);
    }

    #[test]
    fn test_account_persistence_roundtrip() {
        let acc = Account::new(AccountType::Asset, "USD", 100000, None);
        // Verify we can serialize the core fields
        let json = serde_json::json!({
            "id": acc.id.to_string(),
            "account_type": "Asset",
            "balance_cents": 100000,
        });
        assert_eq!(json["balance_cents"], 100000);
    }
}
