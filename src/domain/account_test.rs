#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use crate::domain::account::{
        Account, AccountStatus, AccountType, CreditError, DebitError, HoldError,
    };

    // ━━━ Basic Debit/Credit ━━━

    #[test]
    fn test_initial_balance() {
        let acc = Account::new(AccountType::Asset, "USD", 100_000, None);
        assert_eq!(acc.balance_cents(), 100_000);
        assert_eq!(acc.available_balance_cents(), 100_000);
        assert_eq!(acc.status(), AccountStatus::Open);
    }

    #[test]
    fn test_credit_increases_balance() {
        let acc = Account::new(AccountType::Asset, "USD", 100_000, None);
        let result = acc.credit(50_000);
        assert!(result.is_ok());
        assert_eq!(acc.balance_cents(), 150_000);
        assert_eq!(acc.available_balance_cents(), 150_000);
    }

    #[test]
    fn test_debit_decreases_balance() {
        let acc = Account::new(AccountType::Asset, "USD", 100_000, None);
        let result = acc.debit(30_000);
        assert!(result.is_ok());
        assert_eq!(acc.balance_cents(), 70_000);
        assert_eq!(acc.available_balance_cents(), 70_000);
    }

    #[test]
    fn test_debit_insufficient_funds() {
        let acc = Account::new(AccountType::Asset, "USD", 10_000, None);
        let result = acc.debit(20_000);
        assert!(matches!(result, Err(DebitError::InsufficientFunds { .. })));
        assert_eq!(acc.balance_cents(), 10_000); // balance unchanged
    }

    #[test]
    fn test_debit_rejects_zero_or_negative() {
        let acc = Account::new(AccountType::Asset, "USD", 100_000, None);
        assert!(matches!(acc.debit(0), Err(DebitError::InvalidAmount)));
        assert!(matches!(acc.debit(-50), Err(DebitError::InvalidAmount)));
    }

    #[test]
    fn test_credit_rejects_zero_or_negative() {
        let acc = Account::new(AccountType::Asset, "USD", 100_000, None);
        assert!(matches!(acc.credit(0), Err(CreditError::InvalidAmount)));
        assert!(matches!(acc.credit(-50), Err(CreditError::InvalidAmount)));
    }

    // ━━━ Status Machine ━━━

    #[test]
    fn test_frozen_account_rejects_debit() {
        let acc = Account::new(AccountType::Asset, "USD", 100_000, None);
        acc.set_status(AccountStatus::Frozen);
        let result = acc.debit(10_000);
        assert!(matches!(
            result,
            Err(DebitError::AccountNotOpen(AccountStatus::Frozen))
        ));
    }

    #[test]
    fn test_frozen_account_rejects_credit() {
        let acc = Account::new(AccountType::Asset, "USD", 100_000, None);
        acc.set_status(AccountStatus::Frozen);
        let result = acc.credit(10_000);
        assert!(matches!(
            result,
            Err(CreditError::AccountNotOpen(AccountStatus::Frozen))
        ));
    }

    #[test]
    fn test_closed_account_rejects_all() {
        let acc = Account::new(AccountType::Asset, "USD", 100_000, None);
        acc.set_status(AccountStatus::Closed);
        assert!(acc.debit(1000).is_err());
        assert!(acc.credit(1000).is_err());
    }

    #[test]
    fn test_reopen_frozen_account() {
        let acc = Account::new(AccountType::Asset, "USD", 100_000, None);
        acc.set_status(AccountStatus::Frozen);
        assert!(acc.debit(1000).is_err());
        acc.set_status(AccountStatus::Open);
        assert!(acc.debit(1000).is_ok());
    }

    // ━━━ Hold Mechanism ━━━

    #[test]
    fn test_place_hold_reduces_available_only() {
        let acc = Account::new(AccountType::Asset, "USD", 100_000, None);
        acc.place_hold(30_000).unwrap();
        assert_eq!(acc.balance_cents(), 100_000); // current balance unchanged
        assert_eq!(acc.available_balance_cents(), 70_000); // available reduced
    }

    #[test]
    fn test_release_hold_restores_available() {
        let acc = Account::new(AccountType::Asset, "USD", 100_000, None);
        acc.place_hold(30_000).unwrap();
        acc.release_hold(30_000).unwrap();
        assert_eq!(acc.balance_cents(), 100_000);
        assert_eq!(acc.available_balance_cents(), 100_000);
    }

    #[test]
    fn test_hold_exceeding_available_fails() {
        let acc = Account::new(AccountType::Asset, "USD", 10_000, None);
        let result = acc.place_hold(20_000);
        assert!(matches!(result, Err(HoldError::InsufficientFunds { .. })));
    }

    #[test]
    fn test_debit_respects_holds() {
        let acc = Account::new(AccountType::Asset, "USD", 100_000, None);
        acc.place_hold(80_000).unwrap();

        // available is now 20_000, so 30_000 debit should fail
        let result = acc.debit(30_000);
        assert!(matches!(result, Err(DebitError::InsufficientFunds { .. })));

        // 20_000 should succeed
        assert!(acc.debit(20_000).is_ok());
        assert_eq!(acc.balance_cents(), 80_000);
        assert_eq!(acc.available_balance_cents(), 0);
    }

    // ━━━ Concurrency — CAS Loop ━━━

    #[test]
    fn test_concurrent_debits_maintain_consistency() {
        let acc = Arc::new(Account::new(AccountType::Asset, "USD", 1_000_000, None));
        let num_threads = 8;
        let debits_per_thread = 100;
        let debit_amount = 100; // 8 * 100 * 100 = 80_000 total debit

        let mut handles = vec![];
        for _ in 0..num_threads {
            let acc = Arc::clone(&acc);
            handles.push(thread::spawn(move || {
                for _ in 0..debits_per_thread {
                    acc.debit(debit_amount).unwrap();
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        let expected = 1_000_000 - (num_threads * debits_per_thread * debit_amount);
        assert_eq!(acc.balance_cents(), expected);
        assert_eq!(acc.available_balance_cents(), expected);
    }

    #[test]
    fn test_concurrent_credits_and_debits_maintain_consistency() {
        let acc = Arc::new(Account::new(AccountType::Asset, "USD", 1_000_000, None));
        let mut handles = vec![];

        // 4 threads debiting
        for _ in 0..4 {
            let acc = Arc::clone(&acc);
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    acc.debit(50).unwrap();
                }
            }));
        }

        // 4 threads crediting
        for _ in 0..4 {
            let acc = Arc::clone(&acc);
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    acc.credit(100).unwrap();
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        // 4*100*50 = 20_000 debit, 4*100*100 = 40_000 credit
        // net: +20_000
        let expected = 1_000_000 + 20_000;
        assert_eq!(acc.balance_cents(), expected);
        assert_eq!(acc.available_balance_cents(), expected);
    }

    #[test]
    fn test_cas_loop_retries_on_contention() {
        // Under extreme contention, CAS retries but still produces correct result
        let acc = Arc::new(Account::new(AccountType::Asset, "USD", 10_000_000, None));
        let num_threads = 16;
        let ops_per_thread = 500;

        let mut handles = vec![];
        for i in 0..num_threads {
            let acc = Arc::clone(&acc);
            handles.push(thread::spawn(move || {
                for _ in 0..ops_per_thread {
                    if i % 2 == 0 {
                        let _ = acc.debit(10);
                    } else {
                        let _ = acc.credit(20);
                    }
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        // 8 debit threads * 500 * 10 = 40_000 debit
        // 8 credit threads * 500 * 20 = 80_000 credit
        // net: +40_000
        let expected = 10_000_000 + 40_000;
        assert_eq!(acc.balance_cents(), expected);
    }

    // ━━━ Edge Cases ━━━

    #[test]
    fn test_debit_exact_balance() {
        let acc = Account::new(AccountType::Asset, "USD", 1, None);
        assert!(acc.debit(1).is_ok());
        assert_eq!(acc.balance_cents(), 0);
    }

    #[test]
    #[should_panic(expected = "attempt to add with overflow")]
    fn test_max_values_panics_in_debug() {
        let acc = Account::new(AccountType::Asset, "USD", i64::MAX, None);
        // This panics in debug mode — AtomicI64 wraps in release.
        // For production we'd use checked math.
        let _ = acc.credit(1);
    }

    #[test]
    fn test_multiple_holds_then_release() {
        let acc = Account::new(AccountType::Asset, "USD", 100_000, None);
        acc.place_hold(10_000).unwrap();
        acc.place_hold(20_000).unwrap();
        acc.place_hold(5_000).unwrap();

        assert_eq!(acc.available_balance_cents(), 65_000);
        assert_eq!(acc.balance_cents(), 100_000);

        acc.release_hold(10_000).unwrap();
        assert_eq!(acc.available_balance_cents(), 75_000);

        acc.release_hold(25_000).unwrap();
        assert_eq!(acc.available_balance_cents(), 100_000);
    }
}
