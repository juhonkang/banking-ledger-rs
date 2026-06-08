// Edge case tests — the 0.0001 matters in banking.
// Every boundary condition must be verified.

#[cfg(test)]
mod edge_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use crate::domain::account::{Account, AccountStatus, AccountType, DebitError};
    use crate::domain::coa::{ChartOfAccounts, CoaAccount, CoaCategory};
    use crate::domain::journal::{EntryLeg, JournalEntry};
    use crate::domain::money::{Currency, Money, RoundingMode};
    use crate::log::hash_chain::HashChain;
#[cfg(feature = "full")]
    use crate::service::concurrency::TransferCondition;
    use crate::service::resilience::{CircuitBreaker, GoldenSignals, TokenBucket};

    #[test]
    fn test_money_third_third_third_equals_one() {
        let usd = Currency::usd();
        let third = Money::new(
            rust_decimal_macros::dec!(0.3333333333333333333333333333),
            usd.clone(),
        );
        let sum = (third.clone() + third.clone()).unwrap();
        let sum = (sum + third).unwrap();
        let rounded = sum.round(RoundingMode::HalfUp);
        assert_eq!(rounded.amount, rust_decimal_macros::dec!(1.00));
    }

    #[test]
    fn test_money_sub_negative() {
        let usd = Currency::usd();
        let a = Money::new(rust_decimal_macros::dec!(50.00), usd.clone());
        let b = Money::new(rust_decimal_macros::dec!(100.00), usd);
        assert_eq!((a - b).unwrap().amount, rust_decimal_macros::dec!(-50.00));
    }

    #[test]
    fn test_debit_exactly_to_zero() {
        let acc = Account::new(AccountType::Asset, "USD", 1000, None);
        assert!(acc.debit(1000).is_ok());
        assert_eq!(acc.balance_cents(), 0);
    }

    #[test]
    fn test_freeze_unfreeze_freeze() {
        let acc = Account::new(AccountType::Asset, "USD", 5000, None);
        acc.set_status_unchecked(AccountStatus::Frozen);
        assert!(acc.debit(100).is_err());
        acc.set_status_unchecked(AccountStatus::Open);
        assert!(acc.debit(100).is_ok());
        acc.set_status_unchecked(AccountStatus::Frozen);
        assert!(acc.credit(100).is_err());
        acc.set_status_unchecked(AccountStatus::Open);
        assert!(acc.credit(100).is_ok());
    }

    #[test]
    fn test_hold_then_release_then_debit() {
        let acc = Account::new(AccountType::Asset, "USD", 10000, None);
        acc.place_hold(9000).unwrap();
        assert!(matches!(
            acc.debit(2000),
            Err(DebitError::InsufficientFunds { .. })
        ));
        acc.release_hold(9000).unwrap();
        assert!(acc.debit(2000).is_ok());
    }

    #[test]
    fn test_journal_payroll_6_legs() {
        let cash = uuid::Uuid::now_v7();
        let mut legs = Vec::new();
        for _ in 0..5 {
            legs.push(EntryLeg::debit(uuid::Uuid::now_v7(), 10000));
        }
        legs.push(EntryLeg::credit(cash, 50000));
        assert!(JournalEntry::new(uuid::Uuid::now_v7(), 1, legs, "Payroll").is_ok());
    }

    #[test]
    fn test_journal_all_zero_rejected() {
        let legs = vec![
            EntryLeg::debit(uuid::Uuid::now_v7(), 0),
            EntryLeg::credit(uuid::Uuid::now_v7(), 0),
        ];
        assert!(JournalEntry::new(uuid::Uuid::now_v7(), 1, legs, "Zero").is_err());
    }

    #[test]
    fn test_hash_chain_100_blocks() {
        let key = b"32-byte-test-key-for-hash-chain!";
        let mut chain = HashChain::new(key);
        for i in 0..100 {
            chain.append(&format!("block-{}", i));
        }
        assert!(chain.verify_chain().0);
        assert_eq!(chain.len(), 101);
    }

    #[test]
    fn test_hash_chain_empty_query() {
        let key = b"32-byte-test-key-for-hash-chain!";
        let chain = HashChain::new(key);
        let far = chrono::Utc::now() + chrono::Duration::days(365);
        assert!(chain.query_by_time(far, far).is_empty());
    }

    #[test]
    fn test_circuit_breaker_full_lifecycle() {
        let cb = CircuitBreaker::new(2, Duration::from_millis(5));
        cb.record_failure();
        cb.record_failure();
        assert!(!cb.allow_request());
        std::thread::sleep(Duration::from_millis(10));
        assert!(cb.allow_request());
        cb.record_success();
        cb.record_success();
        assert!(cb.allow_request());
    }

    #[test]
    fn test_token_bucket_capped() {
        let bucket = TokenBucket::new(2, 10000.0);
        std::thread::sleep(Duration::from_millis(100));
        assert!(bucket.try_consume());
        assert!(bucket.try_consume());
        assert!(!bucket.try_consume());
    }

    #[test]
    fn test_token_bucket_refills() {
        let bucket = TokenBucket::new(5, 1000.0);
        for _ in 0..5 {
            assert!(bucket.try_consume());
        }
        assert!(!bucket.try_consume());
        std::thread::sleep(Duration::from_millis(10));
        assert!(bucket.try_consume());
    }

    #[test]
    fn test_condvar_multi_waiter() {
        let tc = Arc::new(TransferCondition::new(0));
        let mut handles = vec![];
        for _ in 0..5 {
            let tc = Arc::clone(&tc);
            handles.push(std::thread::spawn(move || tc.wait_and_debit(100).unwrap()));
        }
        std::thread::sleep(Duration::from_millis(30));
        tc.credit_and_notify(1000);
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(*tc.balance.lock().unwrap(), 500);
    }

    #[test]
    fn test_golden_signals_reset() {
        let signals = GoldenSignals::new(100);
        signals.record_request(Duration::from_millis(1), false);
        signals.record_request(Duration::from_millis(2), true);
        assert_eq!(signals.total_requests(), 2);
        signals.reset();
        assert_eq!(signals.total_requests(), 0);
        assert_eq!(signals.latency_percentile(50.0), None);
    }

    #[test]
    fn test_coa_deep_hierarchy() {
        let mut coa = ChartOfAccounts::new(1);
        let parent = coa.add_account(CoaAccount::new(
            "1000",
            "Assets",
            CoaCategory::Asset,
            None,
            1,
        ));
        let child = coa.add_account(CoaAccount::new(
            "1001",
            "Cash",
            CoaCategory::Asset,
            Some(parent),
            1,
        ));
        let grandchild = coa.add_account(CoaAccount::new(
            "1001.01",
            "Checking",
            CoaCategory::Asset,
            Some(child),
            1,
        ));
        assert!(coa.find_by_id(grandchild).is_some());
        let gc = coa.find_by_id(grandchild).unwrap();
        let c = coa.find_by_id(gc.parent_id.unwrap()).unwrap();
        assert_eq!(c.code, "1001");
    }

    #[test]
    fn test_jpy_rounds_to_whole() {
        let jpy = Currency::jpy();
        let m = Money::new(rust_decimal_macros::dec!(100.99), jpy.clone());
        let r = m.round(RoundingMode::HalfEven);
        assert_eq!(r.amount, rust_decimal_macros::dec!(101));
    }

    #[test]
    fn test_vnd_large_amount() {
        let vnd = Currency::vnd();
        let m = Money::from_minor(50_000_000, vnd);
        assert_eq!(m.amount, rust_decimal_macros::dec!(50000000));
    }
}
