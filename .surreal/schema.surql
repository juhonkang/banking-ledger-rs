-- Banking Ledger — SurrealDB Schema
-- Auto-applied on first docker compose up

DEFINE NAMESPACE banking_ledger;
USE NS banking_ledger;
DEFINE DATABASE ledger;
USE DB ledger;

-- ━━━ Accounts ━━━
DEFINE TABLE account SCHEMAFULL;
DEFINE FIELD account_type    ON account TYPE string ASSERT $value IN ["Asset", "Liability", "Equity", "Revenue", "Expense"];
DEFINE FIELD currency        ON account TYPE string ASSERT string::len($value) == 3;
DEFINE FIELD balance_cents   ON account TYPE int DEFAULT 0;
DEFINE FIELD hold_cents      ON account TYPE int DEFAULT 0;
DEFINE FIELD status          ON account TYPE string DEFAULT "Active" ASSERT $value IN ["Active", "Frozen", "Closed"];
DEFINE FIELD created_at      ON account TYPE datetime DEFAULT time::now();
DEFINE FIELD updated_at      ON account TYPE datetime DEFAULT time::now();
DEFINE INDEX idx_account_status ON account FIELDS status;
DEFINE INDEX idx_account_type   ON account FIELDS account_type;

-- ━━━ Journal Entries ━━━
DEFINE TABLE journal_entry SCHEMAFULL;
DEFINE FIELD transaction_id   ON journal_entry TYPE string;
DEFINE FIELD sequence_number  ON journal_entry TYPE int;
DEFINE FIELD description      ON journal_entry TYPE string;
DEFINE FIELD recorded_at      ON journal_entry TYPE datetime DEFAULT time::now();
DEFINE FIELD legs             ON journal_entry TYPE array;
DEFINE INDEX idx_journal_txn  ON journal_entry FIELDS transaction_id;
DEFINE INDEX idx_journal_time ON journal_entry FIELDS recorded_at;

-- ━━━ Hash Chain Blocks ━━━
DEFINE TABLE hash_block SCHEMAFULL;
DEFINE FIELD index_pos       ON hash_block TYPE int;
DEFINE FIELD hash            ON hash_block TYPE string;
DEFINE FIELD previous_hash   ON hash_block TYPE string;
DEFINE FIELD data             ON hash_block TYPE string;
DEFINE FIELD timestamp       ON hash_block TYPE datetime DEFAULT time::now();
DEFINE FIELD nonce           ON hash_block TYPE int DEFAULT 0;
DEFINE INDEX idx_block_index ON hash_block FIELDS index_pos;
