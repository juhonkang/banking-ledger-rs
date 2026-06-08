#[cfg(test)]
mod tests {
    use crate::domain::journal::{
        net_position, EntryLeg, EntrySide, JournalEntry, JournalError, Transaction,
        TransactionStatus,
    };

    fn account_id() -> uuid::Uuid {
        uuid::Uuid::now_v7()
    }

    fn transaction_id() -> uuid::Uuid {
        uuid::Uuid::now_v7()
    }

    // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    // Test 1: EntryLeg::debit with zero amount (should error)
    // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

    /// EntryLeg::debit panics in debug mode when amount_cents is zero.
    #[test]
    #[should_panic(expected = "EntryLeg::debit requires amount_cents > 0")]
    fn entry_leg_debit_zero_panics_in_debug() {
        let a = account_id();
        let _ = EntryLeg::debit(a, 0); // debug_assert! panics
    }

    /// EntryLeg::credit panics in debug mode when amount_cents is zero.
    #[test]
    #[should_panic(expected = "EntryLeg::credit requires amount_cents > 0")]
    fn entry_leg_credit_zero_panics_in_debug() {
        let a = account_id();
        let _ = EntryLeg::credit(a, 0); // debug_assert! panics
    }

    /// A zero-amount debit leg (constructed manually, simulating release mode
    /// where the debug_assert is compiled out) results in MissingSide when
    /// paired with a valid credit (because sum of debits == 0).
    #[test]
    fn zero_amount_debit_leg_causes_missing_side() {
        use crate::domain::journal::EntrySide;

        let a = account_id();
        let b = account_id();

        // Manually construct a zero-amount debit leg (bypasses debug_assert)
        let zero_debit = EntryLeg {
            account_id: a,
            side: EntrySide::Debit,
            amount_cents: 0,
            amount: None,
        };
        assert_eq!(zero_debit.amount_cents, 0);

        let legs = vec![zero_debit, EntryLeg::credit(b, 100)];
        let result = JournalEntry::new(
            transaction_id(),
            1,
            legs,
            "zero-debit + credit",
        );
        assert!(
            matches!(result, Err(JournalError::MissingSide)),
            "zero-sum debit side must be rejected: got {:?}",
            result
        );
    }

    /// A zero-amount credit leg (manually constructed) results in MissingSide
    /// when paired with a valid debit.
    #[test]
    fn zero_amount_credit_leg_causes_missing_side() {
        use crate::domain::journal::EntrySide;

        let a = account_id();
        let b = account_id();

        let zero_credit = EntryLeg {
            account_id: a,
            side: EntrySide::Credit,
            amount_cents: 0,
            amount: None,
        };
        assert_eq!(zero_credit.amount_cents, 0);

        let legs = vec![EntryLeg::debit(b, 500), zero_credit];
        let result = JournalEntry::new(
            transaction_id(),
            1,
            legs,
            "debit + zero-credit",
        );
        assert!(
            matches!(result, Err(JournalError::MissingSide)),
            "zero-sum credit side must be rejected: got {result:?}",
        );
    }

    // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    // Test 2: JournalEntry unbalanced detection
    // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

    /// Unbalanced entries (debits ≠ credits) must return JournalError::Unbalanced.
    #[test]
    fn unbalanced_entry_rejected_with_details() {
        let a = account_id();
        let b = account_id();

        let legs = vec![
            EntryLeg::debit(a, 100_000),
            EntryLeg::credit(b, 99_999), // one cent short
        ];

        let result = JournalEntry::new(transaction_id(), 1, legs, "off-by-one");
        match result {
            Err(JournalError::Unbalanced {
                total_debits,
                total_credits,
            }) => {
                assert_eq!(total_debits, 100_000);
                assert_eq!(total_credits, 99_999);
            }
            other => panic!("expected Unbalanced, got {:?}", other),
        }
    }

    /// Debits exceed credits — must be Unbalanced.
    #[test]
    fn debits_exceed_credits_rejected() {
        let a = account_id();
        let b = account_id();

        let legs = vec![
            EntryLeg::debit(a, 750_000),
            EntryLeg::credit(b, 250_000),
        ];

        let result = JournalEntry::new(transaction_id(), 1, legs, "debits > credits");
        assert!(matches!(result, Err(JournalError::Unbalanced { .. })));
    }

    /// Credits exceed debits — must be Unbalanced.
    #[test]
    fn credits_exceed_debits_rejected() {
        let a = account_id();
        let b = account_id();

        let legs = vec![
            EntryLeg::debit(a, 100_000),
            EntryLeg::credit(b, 999_999),
        ];

        let result = JournalEntry::new(transaction_id(), 1, legs, "credits > debits");
        assert!(matches!(result, Err(JournalError::Unbalanced { .. })));
    }

    /// Balanced multi-leg compound entries are accepted.
    #[test]
    fn compound_entry_must_balance() {
        let s_exp = account_id();
        let p_tax = account_id();
        let cash = account_id();

        let legs = vec![
            EntryLeg::debit(s_exp, 850_000),
            EntryLeg::debit(p_tax, 150_000),
            EntryLeg::credit(cash, 1_000_000),
        ];

        let entry = JournalEntry::new(transaction_id(), 1, legs, "compound payroll");
        assert!(entry.is_ok(), "balanced compound entry must succeed");
        assert!(entry.unwrap().verify_balance());
    }

    /// verify_balance after creation should return true for valid entries.
    #[test]
    fn verify_balance_returns_true_for_valid_entry() {
        let from = account_id();
        let to = account_id();
        let legs = vec![
            EntryLeg::debit(from, 50_000),
            EntryLeg::credit(to, 50_000),
        ];
        let entry = JournalEntry::new(transaction_id(), 1, legs, "balance check").unwrap();
        assert!(entry.verify_balance());
    }

    /// verify_balance returns false after tampering.
    #[test]
    fn verify_balance_detects_post_creation_tampering() {
        let from = account_id();
        let to = account_id();
        let legs = vec![
            EntryLeg::debit(from, 200_000),
            EntryLeg::credit(to, 200_000),
        ];
        let mut entry = JournalEntry::new(transaction_id(), 1, legs, "tamper test").unwrap();
        assert!(entry.verify_balance());

        // Mutate a leg directly (bypasses validation)
        entry.legs[0].amount_cents = 500_000;
        assert!(!entry.verify_balance(), "tampered entry must not verify");
    }

    // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    // Test 3: Transaction commit/reject states
    // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

    /// New transactions start in Pending.
    #[test]
    fn transaction_starts_pending() {
        let txn = Transaction::new("TXN-001");
        assert_eq!(txn.status, TransactionStatus::Pending);
        assert!(txn.completed_at.is_none());
    }

    /// commit() transitions Pending → Committed and returns true.
    #[test]
    fn commit_sets_committed_and_returns_true() {
        let mut txn = Transaction::new("TXN-002");
        let result = txn.commit();
        assert!(result, "commit must return true for Pending");
        assert_eq!(txn.status, TransactionStatus::Committed);
        assert!(txn.completed_at.is_some(), "committed_at must be set");
    }

    /// reject() transitions Pending → Rejected and returns true.
    #[test]
    fn reject_sets_rejected_and_returns_true() {
        let mut txn = Transaction::new("TXN-003");
        let result = txn.reject();
        assert!(result, "reject must return true for Pending");
        assert_eq!(txn.status, TransactionStatus::Rejected);
        assert!(txn.completed_at.is_some());
    }

    /// commit() on an already Committed transaction must return false.
    #[test]
    fn double_commit_returns_false() {
        let mut txn = Transaction::new("TXN-004");
        assert!(txn.commit());
        assert!(!txn.commit(), "double commit must return false");
        assert_eq!(txn.status, TransactionStatus::Committed);
    }

    /// reject() on an already Rejected transaction must return false.
    #[test]
    fn double_reject_returns_false() {
        let mut txn = Transaction::new("TXN-005");
        assert!(txn.reject());
        assert!(!txn.reject(), "double reject must return false");
        assert_eq!(txn.status, TransactionStatus::Rejected);
    }

    /// commit() on a Rejected transaction must return false.
    #[test]
    fn commit_on_rejected_returns_false() {
        let mut txn = Transaction::new("TXN-006");
        txn.reject();
        assert!(!txn.commit(), "cannot commit a rejected txn");
        assert_eq!(txn.status, TransactionStatus::Rejected);
    }

    /// reject() on a Committed transaction must return false.
    #[test]
    fn reject_on_committed_returns_false() {
        let mut txn = Transaction::new("TXN-007");
        txn.commit();
        assert!(!txn.reject(), "cannot reject a committed txn");
        assert_eq!(txn.status, TransactionStatus::Committed);
    }

    // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    // Test 4: EntrySide::counterpart
    // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

    #[test]
    fn debit_counterpart_is_credit() {
        assert_eq!(EntrySide::Debit.counterpart(), EntrySide::Credit);
    }

    #[test]
    fn credit_counterpart_is_debit() {
        assert_eq!(EntrySide::Credit.counterpart(), EntrySide::Debit);
    }

    /// Double counterpart should return to original side.
    #[test]
    fn double_counterpart_is_identity() {
        assert_eq!(
            EntrySide::Debit.counterpart().counterpart(),
            EntrySide::Debit
        );
        assert_eq!(
            EntrySide::Credit.counterpart().counterpart(),
            EntrySide::Credit
        );
    }

    // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    // Test 5: net_position calculation
    // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

    /// Account with only credits → net positive.
    #[test]
    fn net_position_credit_only() {
        let acc = account_id();
        let other = account_id();
        let legs = vec![
            EntryLeg::debit(other, 100_000),
            EntryLeg::credit(acc, 100_000),
        ];
        let (credits, debits, net) = net_position(&legs, acc);
        assert_eq!(credits, 100_000);
        assert_eq!(debits, 0);
        assert_eq!(net, 100_000);
    }

    /// Account with only debits → net negative.
    #[test]
    fn net_position_debit_only() {
        let acc = account_id();
        let other = account_id();
        let legs = vec![
            EntryLeg::debit(acc, 250_000),
            EntryLeg::credit(other, 250_000),
        ];
        let (credits, debits, net) = net_position(&legs, acc);
        assert_eq!(credits, 0);
        assert_eq!(debits, 250_000);
        assert_eq!(net, -250_000);
    }

    /// Account with both debits and credits across multiple legs.
    #[test]
    fn net_position_mixed() {
        let target = account_id();
        let a = account_id();
        let b = account_id();

        let legs = vec![
            EntryLeg::debit(target, 100_000),
            EntryLeg::credit(a, 100_000),
            EntryLeg::credit(target, 75_000),
            EntryLeg::debit(b, 75_000),
        ];

        let (credits, debits, net) = net_position(&legs, target);
        assert_eq!(credits, 75_000);
        assert_eq!(debits, 100_000);
        assert_eq!(net, -25_000, "net = 75k - 100k = -25k");
    }

    /// Account not present → all zeros.
    #[test]
    fn net_position_account_not_found() {
        let acc = account_id();
        let other = account_id();
        let legs = vec![
            EntryLeg::debit(other, 500_000),
            EntryLeg::credit(other, 500_000),
        ];
        let (credits, debits, net) = net_position(&legs, acc);
        assert_eq!(credits, 0);
        assert_eq!(debits, 0);
        assert_eq!(net, 0);
    }

    /// Empty legs → all zeros.
    #[test]
    fn net_position_empty_legs() {
        let acc = account_id();
        let (credits, debits, net) = net_position(&[], acc);
        assert_eq!((credits, debits, net), (0, 0, 0));
    }

    // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    // Bonus: JournalEntry::reverse integration
    // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

    /// Reversing an entry produces a balanced entry with sides flipped.
    #[test]
    fn reverse_produces_balanced_flipped_entry() {
        let from = account_id();
        let to = account_id();
        let legs = vec![
            EntryLeg::debit(from, 50_000),
            EntryLeg::credit(to, 50_000),
        ];
        let original = JournalEntry::new(transaction_id(), 1, legs, "original").unwrap();

        let reversal = original
            .reverse(transaction_id(), 2)
            .expect("reversal must succeed");

        assert!(reversal.verify_balance());
        assert_eq!(reversal.legs[0].side, EntrySide::Credit);
        assert_eq!(reversal.legs[1].side, EntrySide::Debit);
        assert_eq!(reversal.reverses, Some(original.id));
    }
}
