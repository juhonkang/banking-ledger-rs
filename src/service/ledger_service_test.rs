#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, RwLock};

    use crate::domain::account::{Account, AccountId, AccountType};
    
    use crate::service::ledger_service::LedgerService;

    fn setup_accounts() -> (
        Arc<RwLock<HashMap<AccountId, Account>>>,
        AccountId, // savings
        AccountId, // checking
    ) {
        let map = Arc::new(RwLock::new(HashMap::new()));

        let savings = Account::new(AccountType::Asset, "USD", 1_000_000, None);
        let checking = Account::new(AccountType::Asset, "USD", 500_000, None);

        let s_id = savings.id;
        let c_id = checking.id;

        map.write().unwrap().insert(s_id, savings);
        map.write().unwrap().insert(c_id, checking);

        (map, s_id, c_id)
    }

    #[test]
    fn test_simple_transfer() {
        let (accounts, savings, checking) = setup_accounts();
        let ledger = LedgerService::new(accounts);

        let txn_id = ledger.begin_transaction("TRF-001");
        let result = ledger.record_transfer(txn_id, savings, checking, 300_000, "Transfer $3,000");

        assert!(result.is_ok());

        // Verify balances
        let accs = ledger.accounts.read().unwrap();
        let s = accs.get(&savings).unwrap();
        let c = accs.get(&checking).unwrap();

        assert_eq!(s.balance_cents(), 700_000); // 1,000,000 - 300,000
        assert_eq!(c.balance_cents(), 800_000); // 500,000 + 300,000
    }

    #[test]
    fn test_transfer_insufficient_funds() {
        let (accounts, savings, checking) = setup_accounts();
        let ledger = LedgerService::new(accounts);

        let txn_id = ledger.begin_transaction("TRF-002");
        let result = ledger.record_transfer(txn_id, savings, checking, 2_000_000, "Overdraft");

        assert!(result.is_err());

        // Balances unchanged
        let accs = ledger.accounts.read().unwrap();
        assert_eq!(accs.get(&savings).unwrap().balance_cents(), 1_000_000);
        assert_eq!(accs.get(&checking).unwrap().balance_cents(), 500_000);
    }

    #[test]
    fn test_journal_audit_trail() {
        let (accounts, savings, checking) = setup_accounts();
        let ledger = LedgerService::new(accounts);

        let txn1 = ledger.begin_transaction("TRF-003");
        ledger
            .record_transfer(txn1, savings, checking, 100_000, "First transfer")
            .unwrap();

        let txn2 = ledger.begin_transaction("TRF-004");
        ledger
            .record_transfer(txn2, savings, checking, 200_000, "Second transfer")
            .unwrap();

        let entries = ledger.get_all_entries();
        assert_eq!(entries.len(), 2);

        // Sequence numbers should be 1, 2
        assert_eq!(entries[0].sequence_number, 1);
        assert_eq!(entries[1].sequence_number, 2);

        // All entries balanced
        for e in &entries {
            assert!(e.verify_balance());
        }
    }

    #[test]
    fn test_reversal_entry() {
        let (accounts, savings, checking) = setup_accounts();
        let ledger = LedgerService::new(accounts);

        let txn_id = ledger.begin_transaction("TRF-005");
        let original = ledger
            .record_transfer(txn_id, savings, checking, 500_000, "Mistaken transfer")
            .unwrap();

        let original_id = original.id;
        drop(original);

        // Reverse it
        let reversal = ledger.reverse_entry(original_id, "Wrong amount");
        assert!(reversal.is_ok());

        // Balances should be back to original
        let accs = ledger.accounts.read().unwrap();
        assert_eq!(accs.get(&savings).unwrap().balance_cents(), 1_000_000);
        assert_eq!(accs.get(&checking).unwrap().balance_cents(), 500_000);
    }

    #[test]
    fn test_transaction_status() {
        let (accounts, savings, checking) = setup_accounts();
        let ledger = LedgerService::new(accounts);

        let txn_id = ledger.begin_transaction("TRF-006");
        ledger
            .record_transfer(txn_id, savings, checking, 50_000, "Small transfer")
            .unwrap();

        let txn = ledger.get_transaction(txn_id).unwrap();
        assert!(matches!(
            txn.status,
            crate::domain::journal::TransactionStatus::Committed
        ));
    }
}
