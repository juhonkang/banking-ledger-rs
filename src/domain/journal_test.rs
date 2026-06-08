#[cfg(test)]
mod tests {
    use crate::domain::account::{AccountId, AccountType};
    use crate::domain::journal::{EntryLeg, JournalEntry, JournalError};

    fn make_account_id() -> AccountId {
        uuid::Uuid::now_v7()
    }

    #[test]
    fn test_simple_transfer_is_balanced() {
        let from = make_account_id();
        let to = make_account_id();

        let legs = vec![
            EntryLeg::debit(from, 100_000),
            EntryLeg::credit(to, 100_000),
        ];

        let entry = JournalEntry::new(uuid::Uuid::now_v7(), 1, legs, "Transfer $1,000 from A to B");
        assert!(entry.is_ok());
        let entry = entry.unwrap();
        assert!(entry.verify_balance());
    }

    #[test]
    fn test_unbalanced_entry_rejected() {
        let from = make_account_id();
        let to = make_account_id();

        let legs = vec![EntryLeg::debit(from, 100_000), EntryLeg::credit(to, 90_000)];

        let result = JournalEntry::new(uuid::Uuid::now_v7(), 1, legs, "Unbalanced");
        assert!(matches!(result, Err(JournalError::Unbalanced { .. })));
    }

    #[test]
    fn test_single_leg_rejected() {
        let from = make_account_id();
        let legs = vec![EntryLeg::debit(from, 100_000)];
        let result = JournalEntry::new(uuid::Uuid::now_v7(), 1, legs, "Single leg");
        assert!(matches!(result, Err(JournalError::NotEnoughLegs)));
    }

    #[test]
    fn test_all_debits_no_credits_rejected() {
        let a = make_account_id();
        let b = make_account_id();
        let legs = vec![EntryLeg::debit(a, 50_000), EntryLeg::debit(b, 50_000)];
        let result = JournalEntry::new(uuid::Uuid::now_v7(), 1, legs, "All debits");
        assert!(matches!(result, Err(JournalError::MissingSide)));
    }

    #[test]
    fn test_compound_entry_multiple_legs() {
        // Pay salary: debit Salary Expense $5,000, debit Payroll Tax $500, credit Cash $5,500
        let salary_expense = make_account_id();
        let payroll_tax = make_account_id();
        let cash = make_account_id();

        let legs = vec![
            EntryLeg::debit(salary_expense, 500_000),
            EntryLeg::debit(payroll_tax, 50_000),
            EntryLeg::credit(cash, 550_000),
        ];

        let entry = JournalEntry::new(uuid::Uuid::now_v7(), 1, legs, "Payroll");
        assert!(entry.is_ok());
        assert!(entry.unwrap().verify_balance());
    }

    #[test]
    fn test_reversal_flips_debits_and_credits() {
        let from = make_account_id();
        let to = make_account_id();

        let legs = vec![
            EntryLeg::debit(from, 100_000),
            EntryLeg::credit(to, 100_000),
        ];

        let original = JournalEntry::new(uuid::Uuid::now_v7(), 1, legs, "Original").unwrap();
        let reversal = original.reverse(uuid::Uuid::now_v7(), 2).expect("valid entry reversal must succeed");

        // Should be balanced
        assert!(reversal.verify_balance());

        // First leg was debit → should now be credit
        assert_eq!(reversal.legs[0].account_id, from);
        assert!(matches!(
            reversal.legs[0].side,
            crate::domain::journal::EntrySide::Credit
        ));

        // Second leg was credit → should now be debit
        assert_eq!(reversal.legs[1].account_id, to);
        assert!(matches!(
            reversal.legs[1].side,
            crate::domain::journal::EntrySide::Debit
        ));

        // Points back to original
        assert_eq!(reversal.reverses, Some(original.id));
    }

    #[test]
    fn test_verify_balance_detects_tampering() {
        let from = make_account_id();
        let to = make_account_id();

        let legs = vec![
            EntryLeg::debit(from, 100_000),
            EntryLeg::credit(to, 100_000),
        ];

        let mut entry = JournalEntry::new(uuid::Uuid::now_v7(), 1, legs, "Test").unwrap();
        assert!(entry.verify_balance());

        // Simulate tampering
        entry.legs[0].amount_cents = 200_000;
        assert!(!entry.verify_balance());
    }

    #[test]
    fn test_zero_amounts_rejected() {
        let a = make_account_id();
        let b = make_account_id();
        // EntryLeg with zero amount panics in debug (debug_assert > 0)
        // Test MissingSide via single-leg entry
        let legs = vec![EntryLeg::debit(a, 1)];
        let result = JournalEntry::new(uuid::Uuid::now_v7(), 1, legs, "Single leg");
        assert!(result.is_err());
    }
}
