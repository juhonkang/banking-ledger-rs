//! Extension traits — idiomatic Rust pattern for extending types with
//! domain-specific functionality without modifying the original impl blocks.
//!
//! # Pattern
//!
//! ```rust
//! trait HashChainExt {
//!     fn sign_and_append(&mut self, data: &str) -> &HashLink;
//! }
//! impl HashChainExt for HashChain { ... }
//! ```
//!
//! This is THE canonical Rust idiom for API extension. Used extensively
//! in std (Iterator, Read, Write, Future). Each trait adds a focused
//! capability layer.

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::domain::account::{Account, AccountId, AccountStatus};
use crate::domain::journal::{EntryLeg, EntrySide, JournalEntry, JournalEntryId};
use crate::domain::money::{Currency, Money};
use crate::log::hash_chain::{ChainProof, HashChain, HashLink, SignedTransaction};

// ━━━ HashChain Extension — Cryptographic Operations ━━━

/// Extends [`HashChain`] with signing, redaction, and audit reporting.
pub trait HashChainExt {
    /// Append data and return a signed transaction with HMAC verification.
    fn sign_and_append(&mut self, data: &str) -> SignedTransaction;

    /// Generate a human-readable audit report for a time range.
    fn audit_report(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> String;

    /// Redact a block at the given index (GDPR/privacy compliance).
    /// Returns the new chain head hash after re-hashing forward.
    fn redact_block(&mut self, index: u64) -> Result<String, String>;

    /// Parallel chain verification using rayon (if available).
    fn parallel_verify(&self) -> (bool, Vec<u64>);

    /// Export the chain as a JSON-serializable audit log.
    fn export_audit_log(&self) -> Vec<serde_json::Value>;
}

impl HashChainExt for HashChain {
    fn sign_and_append(&mut self, data: &str) -> SignedTransaction {
        let tx_id = Uuid::now_v7();
        // Extract key before mutable borrow
        let key = {
            let k = self.signing_key();
            k.to_vec()
        };
        let block = self.append(data);
        SignedTransaction::sign(tx_id, &block.hash, &key)
    }

    fn audit_report(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> String {
        let blocks = self.query_by_time(from, to);
        let mut report = format!(
            "=== AUDIT REPORT ===\nPeriod: {} → {}\nBlocks: {}\n\n",
            from.to_rfc3339(),
            to.to_rfc3339(),
            blocks.len()
        );

        for block in &blocks {
            report.push_str(&format!(
                "[{}] {} | data: {} | prev: {}\n",
                block.index,
                &block.hash[..12],
                if block.data.len() > 60 {
                    format!("{}...", &block.data[..57])
                } else {
                    block.data.clone()
                },
                &block.previous_hash[..12],
            ));
        }

        let (valid, tampered) = self.verify_chain();
        report.push_str(&format!(
            "\nIntegrity: {} | Tampered: {:?}\n",
            if valid { "✅ VERIFIED" } else { "❌ TAMPERED" },
            tampered
        ));

        report
    }

    fn redact_block(&mut self, index: u64) -> Result<String, String> {
        self.redact(index)
            .map_err(|e| format!("Redaction failed: {:?}", e))?;
        Ok(self.latest().expect("chain always has genesis block").hash.clone())
    }

    fn parallel_verify(&self) -> (bool, Vec<u64>) {
        crate::log::hash_chain::parallel_verify_chain(self)
    }

    fn export_audit_log(&self) -> Vec<serde_json::Value> {
        self.blocks
            .iter()
            .map(|b| {
                serde_json::json!({
                    "index": b.index,
                    "hash": b.hash,
                    "previous_hash": b.previous_hash,
                    "data": b.data,
                    "timestamp": b.timestamp.to_rfc3339(),
                    "nonce": b.nonce,
                })
            })
            .collect()
    }
}

// ━━━ Journal Extension — Double-Entry Operations ━━━

/// Extends journal-related types with query and analysis capabilities.
pub trait JournalExt {
    /// Compute the net position (debits - credits) for an account.
    fn net_position(entries: &[JournalEntry], account_id: AccountId) -> i64;

    /// Find all entries involving a specific account.
    fn for_account(entries: &[JournalEntry], account_id: AccountId) -> Vec<&JournalEntry>;

    /// Validate that all entries in a slice are balanced (debits == credits).
    fn validate_all(entries: &[JournalEntry]) -> (bool, Vec<JournalEntryId>);

    /// Generate a trial balance from journal entries.
    fn trial_balance(
        entries: &[JournalEntry],
    ) -> std::collections::HashMap<AccountId, (i64, i64)>;
}

impl JournalExt for JournalEntry {
    fn net_position(entries: &[JournalEntry], account_id: AccountId) -> i64 {
        entries
            .iter()
            .flat_map(|e| &e.legs)
            .filter(|l| l.account_id == account_id)
            .map(|l| match l.side {
                EntrySide::Debit => l.amount_cents,
                EntrySide::Credit => -l.amount_cents,
            })
            .sum()
    }

    fn for_account(entries: &[JournalEntry], account_id: AccountId) -> Vec<&JournalEntry> {
        entries
            .iter()
            .filter(|e| e.legs.iter().any(|l| l.account_id == account_id))
            .collect()
    }

    fn validate_all(entries: &[JournalEntry]) -> (bool, Vec<JournalEntryId>) {
        let mut invalid = Vec::new();
        for entry in entries {
            if !entry.verify_balance() {
                invalid.push(entry.id);
            }
        }
        (invalid.is_empty(), invalid)
    }

    /// Compute trial balance (sum of debits/credits per account).
    /// Uses saturating arithmetic to prevent panics on overflow.
    /// For production, use i128 intermediates via verify_balance pattern.
    fn trial_balance(
        entries: &[JournalEntry],
    ) -> std::collections::HashMap<AccountId, (i64, i64)> {
        let mut balances: std::collections::HashMap<AccountId, (i64, i64)> =
            std::collections::HashMap::new();

        for entry in entries {
            for leg in &entry.legs {
                let (debits, credits) = balances.entry(leg.account_id).or_insert((0, 0));
                match leg.side {
                    EntrySide::Debit => *debits = debits.saturating_add(leg.amount_cents),
                    EntrySide::Credit => *credits = credits.saturating_add(leg.amount_cents),
                }
            }
        }

        balances
    }
}

// ━━━ Account Extension — Balance Operations ━━━

/// Extends [`Account`] with formatted display and history tracking.
pub trait AccountExt {
    /// Get balance formatted as decimal string with currency symbol.
    fn balance_display(&self, currency: &Currency) -> String;

    /// Check if an account can be closed (zero balance required).
    fn can_close(&self) -> bool;

    /// Get a summary snapshot of account state.
    fn snapshot(&self, currency: &Currency) -> serde_json::Value;
}

impl AccountExt for Account {
    fn balance_display(&self, currency: &Currency) -> String {
        let money = Money::from_minor(self.balance_cents(), currency.clone());
        let decimals = currency.minor_unit as usize;
        format!(
            "{} {:.*}",
            currency.symbol, decimals, money.amount
        )
    }

    fn can_close(&self) -> bool {
        self.balance_cents() == 0 && self.available_balance_cents() == 0
    }

    fn snapshot(&self, currency: &Currency) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "account_type": format!("{:?}", self.account_type),
            "currency": currency.code,
            "balance_cents": self.balance_cents(),
            "available_balance_cents": self.available_balance_cents(),
            "balance_display": self.balance_display(currency),
            "status": format!("{:?}", self.status()),
            "can_close": self.can_close(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::account::AccountType;
    use crate::domain::money::Currency;

    #[test]
    fn test_hashchain_ext_sign_and_append() {
        let mut chain = HashChain::new(b"test-key-32-bytes-long!!!!!!");
        let signed = chain.sign_and_append(r#"{"tx":"deposit","amount":1000}"#);
        assert!(signed.verify(b"test-key-32-bytes-long!!!!!!"));
        assert_eq!(chain.len(), 2); // genesis + 1
    }

    #[test]
    fn test_hashchain_ext_audit_report() {
        let mut chain = HashChain::new(b"test-key-32-bytes-long!!!!!!");
        chain.append("event1");
        chain.append("event2");
        let report = chain.audit_report(
            chrono::Utc::now() - chrono::Duration::hours(1),
            chrono::Utc::now() + chrono::Duration::hours(1),
        );
        assert!(report.contains("AUDIT REPORT"));
        assert!(report.contains("VERIFIED"));
    }

    #[test]
    fn test_hashchain_ext_export() {
        let mut chain = HashChain::new(b"test-key-32-bytes-long!!!!!!");
        chain.append("test-data");
        let log = chain.export_audit_log();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0]["index"], 0);
        assert_eq!(log[1]["index"], 1);
    }

    #[test]
    fn test_journal_ext_net_position() {
        use crate::domain::journal::{EntryLeg, EntrySide, JournalEntry, TransactionId};
        
        let txn_id = Uuid::now_v7();
        let acc_a = Uuid::now_v7();
        let acc_b = Uuid::now_v7();
        
        let legs = vec![
            EntryLeg::debit(acc_a, 1000),
            EntryLeg::credit(acc_b, 1000),
        ];
        let entry = JournalEntry::new(txn_id, 1, legs, "test").unwrap();
        
        let pos_a = JournalEntry::net_position(&[entry.clone()], acc_a);
        let pos_b = JournalEntry::net_position(&[entry], acc_b);
        assert_eq!(pos_a, 1000);
        assert_eq!(pos_b, -1000);
    }

    #[test]
    fn test_account_ext_can_close() {
        let acc = Account::new(AccountType::Asset, "USD", 0, None);
        assert!(acc.can_close());
        
        let acc2 = Account::new(AccountType::Asset, "USD", 100, None);
        assert!(!acc2.can_close());
    }
}
