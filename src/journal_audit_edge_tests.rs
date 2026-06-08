//! Journal entry audit trail — comprehensive edge case coverage for
//! double-entry accounting validation, negative amounts, and multi-leg entries.

#[cfg(test)]
mod journal_audit_edge_tests {
    use crate::domain::journal::{EntryLeg, JournalEntry, JournalError};

    // ━━━ Double-entry validation ━━━

    #[test]
    fn test_journal_entry_single_leg_rejected() {
        let debit = EntryLeg::debit(uuid::Uuid::now_v7(), 100);
        let result = JournalEntry::new(uuid::Uuid::now_v7(), 1, vec![debit], "single-leg");
        assert!(matches!(result, Err(JournalError::NotEnoughLegs)));
    }

    #[test]
    fn test_journal_entry_unbalanced_rejected() {
        let a = uuid::Uuid::now_v7();
        let b = uuid::Uuid::now_v7();
        let debit = EntryLeg::debit(a, 100);
        let credit = EntryLeg::credit(b, 99);
        let result = JournalEntry::new(uuid::Uuid::now_v7(), 1, vec![debit, credit], "imbalanced");
        assert!(matches!(result, Err(JournalError::Unbalanced { .. })));
    }

    #[test]
    fn test_journal_entry_multi_leg_balanced() {
        let a = uuid::Uuid::now_v7();
        let b = uuid::Uuid::now_v7();
        let c = uuid::Uuid::now_v7();
        let d1 = EntryLeg::debit(a, 100);
        let d2 = EntryLeg::debit(b, 50);
        let cr = EntryLeg::credit(c, 150);
        let result = JournalEntry::new(uuid::Uuid::now_v7(), 1, vec![d1, d2, cr], "multi-leg");
        assert!(result.is_ok());
    }

    #[test]
    fn test_journal_entry_zero_amount_debit_panics_in_debug() {
        let result = std::panic::catch_unwind(|| {
            EntryLeg::debit(uuid::Uuid::now_v7(), 0);
        });
        assert!(result.is_err());
    }

    #[test]
    fn test_journal_entry_negative_credit_panics_in_debug() {
        let result = std::panic::catch_unwind(|| {
            EntryLeg::credit(uuid::Uuid::now_v7(), -1);
        });
        assert!(result.is_err());
    }

    #[test]
    fn test_journal_entry_max_safe_amount() {
        let a = uuid::Uuid::now_v7();
        let b = uuid::Uuid::now_v7();
        let debit = EntryLeg::debit(a, i64::MAX);
        let credit = EntryLeg::credit(b, i64::MAX);
        let result = JournalEntry::new(uuid::Uuid::now_v7(), 1, vec![debit, credit], "max-amount");
        assert!(result.is_ok());
    }

    #[test]
    fn test_journal_sequence_monotonic() {
        let a = uuid::Uuid::now_v7();
        let b = uuid::Uuid::now_v7();

        let e1 = JournalEntry::new(
            uuid::Uuid::now_v7(), 1,
            vec![EntryLeg::debit(a, 50), EntryLeg::credit(b, 50)],
            "seq-1",
        );
        let e2 = JournalEntry::new(
            uuid::Uuid::now_v7(), 2,
            vec![EntryLeg::debit(a, 50), EntryLeg::credit(b, 50)],
            "seq-2",
        );

        assert!(e1.is_ok());
        assert!(e2.is_ok());
        assert!(e1.unwrap().sequence_number < e2.unwrap().sequence_number);
    }

    #[test]
    fn test_entry_leg_equality() {
        let account = uuid::Uuid::now_v7();
        let leg1 = EntryLeg::debit(account, 100);
        let leg2 = EntryLeg::debit(account, 100);
        assert_eq!(leg1.account_id, leg2.account_id);
        assert_eq!(leg1.amount, leg2.amount);
    }

    #[test]
    fn test_journal_error_display() {
        assert!(!format!("{}", JournalError::NotEnoughLegs).is_empty());
        let unbalanced = JournalError::Unbalanced { total_debits: 100, total_credits: 99 };
        assert!(!format!("{}", unbalanced).is_empty());
    }
}
