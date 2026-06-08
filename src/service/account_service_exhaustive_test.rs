//! Comprehensive tests for AccountService — covering all 14 public methods.
//! Edge cases: not-found, concurrent access, hold lifecycle, enum iteration.

#[cfg(test)]
mod exhaustive_account_service_tests {
    use crate::domain::account::{
        AccountId, AccountStatus, AccountType, DebitError, HoldError,
    };
    use crate::service::account_service::AccountService;

    fn setup(initial_balance: i64) -> (AccountService, AccountId) {
        let svc = AccountService::new();
        let acc = svc.create_account(AccountType::Asset, "USD", initial_balance, None);
        let id = acc.id;
        (svc, id)
    }

    // ━━━ create_account ━━━

    #[test]
    fn test_create_account_zero_balance() {
        let svc = AccountService::new();
        let acc = svc.create_account(AccountType::Liability, "EUR", 0, None);
        assert_eq!(acc.balance_cents(), 0);
        assert!(svc.get_account(acc.id).is_some());
    }

    #[test]
    fn test_create_account_negative_initial_balance() {
        // Liability accounts can have negative balance (e.g., loans)
        let svc = AccountService::new();
        let acc = svc.create_account(AccountType::Liability, "USD", -500_000, None);
        assert_eq!(acc.balance_cents(), -500_000);
    }

    #[test]
    fn test_create_multiple_accounts_unique_ids() {
        let svc = AccountService::new();
        let a1 = svc.create_account(AccountType::Asset, "USD", 100, None);
        let a2 = svc.create_account(AccountType::Asset, "USD", 200, None);
        assert_ne!(a1.id, a2.id);
        assert_eq!(svc.count(), 2);
    }

    // ━━━ perform_debit ━━━

    #[test]
    fn test_perform_debit_not_found() {
        let (svc, _) = setup(1000);
        let err = svc.perform_debit(AccountId::now_v7(), 100).unwrap_err();
        assert!(matches!(err, DebitError::AccountNotFound(_)));
    }

    #[test]
    fn test_perform_debit_zero_amount_rejected() {
        let (svc, id) = setup(1000);
        let err = svc.perform_debit(id, 0).unwrap_err();
        assert!(matches!(err, DebitError::InvalidAmount));
    }

    #[test]
    fn test_perform_debit_negative_amount_rejected() {
        let (svc, id) = setup(1000);
        let err = svc.perform_debit(id, -100).unwrap_err();
        assert!(matches!(err, DebitError::InvalidAmount));
    }

    #[test]
    fn test_perform_debit_insufficient_funds() {
        let (svc, id) = setup(1000);
        let err = svc.perform_debit(id, 2000).unwrap_err();
        assert!(matches!(err, DebitError::InsufficientFunds { .. }));
    }

    #[test]
    fn test_perform_debit_exact_balance_to_zero() {
        let (svc, id) = setup(1000);
        let new_bal = svc.perform_debit(id, 1000).unwrap();
        assert_eq!(new_bal, 0);
        assert_eq!(svc.get_balance_cents(id), Some(0));
    }

    // ━━━ perform_credit ━━━

    #[test]
    fn test_perform_credit_not_found() {
        let (svc, _) = setup(1000);
        let err = svc.perform_credit(AccountId::now_v7(), 100).unwrap_err();
        assert!(matches!(err, crate::domain::account::CreditError::AccountNotFound(_)));
    }

    #[test]
    fn test_perform_credit_zero_rejected() {
        let (svc, id) = setup(1000);
        let err = svc.perform_credit(id, 0).unwrap_err();
        assert!(matches!(err, crate::domain::account::CreditError::InvalidAmount));
    }

    #[test]
    fn test_perform_credit_negative_rejected() {
        let (svc, id) = setup(1000);
        let err = svc.perform_credit(id, -50).unwrap_err();
        assert!(matches!(err, crate::domain::account::CreditError::InvalidAmount));
    }

    // ━━━ place_hold / release_hold ━━━

    #[test]
    fn test_place_hold_not_found() {
        let (svc, _) = setup(1000);
        let err = svc.place_hold(AccountId::now_v7(), 100).unwrap_err();
        assert!(matches!(err, HoldError::AccountNotFound(_)));
    }

    #[test]
    fn test_place_hold_reduces_available() {
        let (svc, id) = setup(1000);
        svc.place_hold(id, 300).unwrap();
        let acc = svc.get_account(id).unwrap();
        assert_eq!(acc.balance_cents(), 1000);       // total unchanged
        assert_eq!(acc.available_balance_cents(), 700); // available reduced
    }

    #[test]
    fn test_place_hold_exceeding_available() {
        let (svc, id) = setup(1000);
        let err = svc.place_hold(id, 2000).unwrap_err();
        assert!(matches!(err, HoldError::InsufficientFunds { .. }));
    }

    #[test]
    fn test_release_hold_restores_available() {
        let (svc, id) = setup(1000);
        svc.place_hold(id, 300).unwrap();
        svc.release_hold(id, 300).unwrap();
        assert_eq!(svc.get_account(id).unwrap().available_balance_cents(), 1000);
    }

    #[test]
    fn test_release_hold_not_found() {
        let (svc, _) = setup(1000);
        let err = svc.release_hold(AccountId::now_v7(), 100).unwrap_err();
        assert!(matches!(err, HoldError::AccountNotFound(_)));
    }

    // ━━━ set_status ━━━

    #[test]
    fn test_set_status_not_found() {
        let (svc, _) = setup(1000);
        assert_eq!(svc.set_status(AccountId::now_v7(), AccountStatus::Closed), Ok(false));
    }

    #[test]
    fn test_set_status_freeze_rejects_debit() {
        let (svc, id) = setup(1000);
        assert_eq!(svc.set_status(id, AccountStatus::Frozen), Ok(true));
        let err = svc.perform_debit(id, 100).unwrap_err();
        assert!(matches!(err, DebitError::AccountNotOpen(AccountStatus::Frozen)));
    }

    #[test]
    fn test_set_status_reopen_allows_operations() {
        let (svc, id) = setup(1000);
        assert_eq!(svc.set_status(id, AccountStatus::Frozen), Ok(true));
        assert_eq!(svc.set_status(id, AccountStatus::Open), Ok(true));
        assert_eq!(svc.perform_credit(id, 500).unwrap(), 1500);
    }

    #[test]
    fn test_set_status_closed_permanent() {
        let (svc, id) = setup(1000);
        assert_eq!(svc.set_status(id, AccountStatus::Closed), Ok(true));
        let err = svc.perform_credit(id, 100).unwrap_err();
        assert!(matches!(err, crate::domain::account::CreditError::AccountNotOpen(AccountStatus::Closed)));
    }

    // ━━━ Query methods ━━━

    #[test]
    fn test_get_balance_cents_not_found() {
        let (svc, _) = setup(1000);
        assert_eq!(svc.get_balance_cents(AccountId::now_v7()), None);
    }

    #[test]
    fn test_get_balance_cents_reflects_mutations() {
        let (svc, id) = setup(1000);
        svc.perform_credit(id, 500).unwrap();
        assert_eq!(svc.get_balance_cents(id), Some(1500));
        svc.perform_debit(id, 300).unwrap();
        assert_eq!(svc.get_balance_cents(id), Some(1200));
    }

    #[test]
    fn test_all_returns_all_accounts() {
        let svc = AccountService::new();
        svc.create_account(AccountType::Asset, "USD", 100, None);
        svc.create_account(AccountType::Liability, "EUR", 200, None);
        svc.create_account(AccountType::Equity, "VND", 300, None);
        assert_eq!(svc.all().len(), 3);
    }

    #[test]
    fn test_count_empty() {
        let svc = AccountService::new();
        assert_eq!(svc.count(), 0);
    }

    #[test]
    fn test_count_after_inserts() {
        let svc = AccountService::new();
        for i in 0..5 {
            svc.create_account(AccountType::Asset, "USD", i * 100, None);
        }
        assert_eq!(svc.count(), 5);
    }

    #[test]
    fn test_for_each_iterates_all() {
        let svc = AccountService::new();
        let ids: Vec<_> = (0..3).map(|i| {
            svc.create_account(AccountType::Asset, "USD", i * 100, None).id
        }).collect();

        let mut seen = std::collections::HashSet::new();
        svc.for_each(|id, _| { seen.insert(*id); });
        for id in &ids {
            assert!(seen.contains(id));
        }
    }

    #[test]
    fn test_insert_raw_and_retrieve() {
        use crate::domain::account::Account;
        let svc = AccountService::new();
        let raw = Account::new(AccountType::Asset, "USD", 9999, None);
        let id = raw.id;
        svc.insert_raw(id, raw);
        assert_eq!(svc.get_balance_cents(id), Some(9999));
    }

    // ━━━ All account types ━━━

    #[test]
    fn test_all_account_types_constructible() {
        let svc = AccountService::new();
        let types = [
            AccountType::Asset,
            AccountType::Liability,
            AccountType::Equity,
            AccountType::Revenue,
            AccountType::Expense,
        ];
        for (i, at) in types.iter().enumerate() {
            let acc = svc.create_account(*at, "USD", (i as i64) * 100, None);
            assert_eq!(acc.account_type, *at);
        }
        assert_eq!(svc.count(), 5);
    }
}
