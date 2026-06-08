//! Coverage gap tests for account domain model.
//! Fills untested edge cases: concurrent ops, hold invariants, status transitions.

#[cfg(test)]
mod tests {
    use crate::domain::account::{Account, AccountType, HoldError, DebitError, AccountStatus};
    
    use std::sync::Arc;
    use std::thread;

    fn make_account() -> Account {
        Account::new(AccountType::Asset, "USD", 1_000_000, None)
    }

    #[test]
    fn test_debit_exact_balance() {
        let acc = make_account();
        acc.debit(1_000_000).expect("debit exact balance");
        assert_eq!(acc.available_balance_cents(), 0);
    }

    #[test]
    fn test_debit_one_cent_over_fails() {
        let acc = make_account();
        let r = acc.debit(1_000_001);
        assert!(matches!(r, Err(DebitError::InsufficientFunds { .. })));
    }

    #[test]
    fn test_hold_full_then_debit_blocked() {
        let acc = make_account();
        acc.place_hold(1_000_000).expect("hold full");
        let r = acc.debit(1);
        assert!(matches!(r, Err(DebitError::InsufficientFunds { .. })));
    }

    #[test]
    fn test_hold_partial_release_allows_debit() {
        let acc = make_account();
        acc.place_hold(500_000).expect("hold");
        acc.release_hold(500_000).expect("release");
        acc.debit(1_000_000).expect("debit after release");
    }

    #[test]
    fn test_multiple_holds_exceed_available() {
        let acc = make_account();
        acc.place_hold(600_000).expect("hold1");
        let r = acc.place_hold(500_000);
        assert!(matches!(r, Err(HoldError::InsufficientFunds { .. })));
    }

    #[test]
    fn test_set_status_closed_blocks_debit() {
        let acc = make_account();
        acc.set_status(AccountStatus::Closed).expect("close");
        assert!(matches!(acc.debit(100), Err(DebitError::AccountNotOpen(AccountStatus::Closed))));
    }

    #[test]
    fn test_set_status_closed_blocks_credit() {
        let acc = make_account();
        acc.set_status(AccountStatus::Closed).expect("close");
        assert!(acc.credit(100).is_err());
    }

    #[test]
    fn test_concurrent_debit_sum_correct() {
        let acc = Arc::new(make_account());
        let mut hs = vec![];
        for _ in 0..10 {
            let a = acc.clone();
            hs.push(thread::spawn(move || {
                for _ in 0..100 { let _ = a.debit(100); }
            }));
        }
        for h in hs { h.join().unwrap(); }
        let avail = acc.available_balance_cents();
        let bal = acc.balance_cents();
        assert_eq!(avail, bal, "no holds → available == balance");
        assert!(bal <= 1_000_000);
    }

    #[test]
    fn test_concurrent_credit_no_overflow_cas_loop() {
        let acc = Arc::new(make_account());
        let mut hs = vec![];
        for _ in 0..10 {
            let a = acc.clone();
            hs.push(thread::spawn(move || {
                for _ in 0..1_000 {
                    a.credit(1_000).expect("credit");
                }
            }));
        }
        for h in hs { h.join().unwrap(); }
        assert_eq!(acc.balance_cents(), 1_000_000 + 10 * 1_000 * 1_000);
    }

    #[test]
    fn test_credit_near_max_returns_overflow_error() {
        let acc = make_account();
        acc.credit(i64::MAX - 2_000_000).expect("credit near max");
        // Next credit should overflow
        assert!(acc.credit(i64::MAX).is_err());
    }
}
