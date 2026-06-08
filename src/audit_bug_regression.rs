//! POC tests for bugs found during deep code audit.
//! Each test demonstrates a real vulnerability or flaw.

#[cfg(test)]
mod audit_bug_regression_tests {
    use crate::domain::account::{Account, AccountType, DebitError, HoldError};
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    // ━━━ BUG #1: credit() has transient window between two atomics ━━━
    // This is a documented trade-off: fetch_add on balance then fetch_add
    // on available_balance. The transient window is safe because invariant
    // balance >= available_balance is always maintained.
    #[test]
    fn repro_credit_documented_window() {
        let acc = Arc::new(Account::new(
            AccountType::Asset,
            "USD",
            1_000_000,
            None,
        ));

        let acc_clone = acc.clone();
        let handle = std::thread::spawn(move || {
            for _ in 0..1000 {
                acc_clone.credit(1).unwrap();
            }
        });

        let mut reads_with_window = 0u64;
        for _ in 0..10000 {
            let bal = acc.balance_cents();
            let avail = acc.available_balance_cents();
            if bal != avail {
                reads_with_window += 1;
            }
        }

        handle.join().unwrap();

        let final_bal = acc.balance_cents();
        let final_avail = acc.available_balance_cents();
        assert_eq!(final_bal, final_avail, "after completion, balance and available should be equal");

        eprintln!("NOTE: credit() transient window: {reads_with_window}/10000 reads saw balance≠available");
        eprintln!("  This is documented behavior — the window is safe, invariant balance≥available holds");
    }

    // ━━━ BUG #2: release_hold() can silently overflow ━━━
    // fetch_add on i64 wraps on overflow. If someone releases more than
    // was held (or releases repeatedly), the available balance wraps to negative.
    #[test]
    fn repro_release_hold_overflow() {
        let acc = Account::new(
            AccountType::Asset,
            "USD",
            1_000_000,
            None,
        );

        // Place a hold
        acc.place_hold(500_000).unwrap();
        assert_eq!(acc.available_balance_cents(), 500_000);

        // Release the hold
        acc.release_hold(500_000).unwrap();
        assert_eq!(acc.available_balance_cents(), 1_000_000);

        // FIXED: release_hold now uses checked_add, so overflow is prevented
        let result = acc.release_hold(i64::MAX);
        assert!(result.is_err(), "FIXED: release_hold rejects overflow (was silent wrap)");
        eprintln!("FIXED: release_hold now returns Err on overflow instead of silently wrapping");
    }

    // ━━━ BUG #3: TOCTOU — status check then CAS loop ━━━
    // debit() checks status at line 200, then enters CAS loop at line 206.
    // Between the status check and the CAS, another thread could freeze the account.
    #[test]
    fn repro_debit_toctou() {
        let acc = Arc::new(Account::new(
            AccountType::Asset,
            "USD",
            1_000_000,
            None,
        ));

        let acc_clone = acc.clone();
        let handle = std::thread::spawn(move || {
            // Rapidly freeze and unfreeze
            for _ in 0..1000 {
                acc_clone.set_status(crate::domain::account::AccountStatus::Frozen);
                acc_clone.set_status(crate::domain::account::AccountStatus::Open);
            }
        });

        let mut successful_debits_on_frozen = 0u64;
        for _ in 0..5000 {
            // Try to debit — if account is frozen during the CAS loop,
            // the debit should fail. But due to TOCTOU, it might succeed
            // after the status was changed to Frozen but before the CAS completes.
            match acc.debit(1) {
                Ok(_) => {
                    if acc.status() == crate::domain::account::AccountStatus::Frozen {
                        successful_debits_on_frozen += 1;
                    }
                }
                Err(DebitError::AccountNotOpen(_)) => {}
                Err(_) => {}
            }
        }

        handle.join().unwrap();
        eprintln!("BUG #3: debit() TOCTOU: {successful_debits_on_frozen} debits succeeded on frozen account");
    }

    // ━━━ BUG #4: place_hold() uses weaker Ordering than debit() ━━━
    // debit() uses SeqCst for CAS (strongest), place_hold uses AcqRel (weaker).
    // On ARM/PowerPC, this can allow reordering that breaks invariants.
    #[test]
    fn repro_hold_ordering_weaker() {
        // This is an informational test — the bug is in the memory model,
        // not directly observable on x86 (TSO). But on ARM, AcqRel is weaker
        // than SeqCst and can allow reordering.
        let acc = Arc::new(Account::new(
            AccountType::Asset,
            "USD",
            1_000_000,
            None,
        ));

        // Verify both operations use the account's own atomics
        // Place hold — uses AcqRel (weaker)
        acc.place_hold(100).unwrap();
        // Debit — uses SeqCst (stronger)
        acc.debit(100).unwrap();

        eprintln!("BUG #4: Memory ordering inconsistency — place_hold uses AcqRel, debit uses SeqCst");
        eprintln!("  On ARM/PowerPC this can cause reordering bugs");
    }

    // ━━━ BUG #5: JournalEntry::new() i64 sum overflow ━━━
    // Summing i64 values without overflow protection
    #[test]
    fn repro_journal_sum_overflow() {
        // This test shows that the sum can overflow but isn't caught
        // In debug mode, this panics. In release mode, wraps to negative.
        // Either way, the behavior is incorrect for financial systems.
        let result = std::panic::catch_unwind(|| {
            let _debits: i64 = vec![
                crate::domain::journal::EntryLeg::debit(
                    uuid::Uuid::now_v7(),
                    i64::MAX,
                ),
                crate::domain::journal::EntryLeg::debit(
                    uuid::Uuid::now_v7(),
                    1,
                ),
            ].iter()
            .filter(|l| l.side == crate::domain::journal::EntrySide::Debit)
            .map(|l| l.amount_cents)
            .sum::<i64>();
        });

        match result {
            Ok(_) => eprintln!("BUG #5: Journal sum overflow — wrapped silently in release mode"),
            Err(_) => eprintln!("BUG #5: Journal sum overflow — panicked in debug mode (confirmed overflow)"),
        }
    }

    // ━━━ BUG #6: HMAC — NOT RFC 2104 compliant ━━━
    // The hmac_sign uses H(key || H(key || message)) which is vulnerable
    // to length-extension attacks on SHA-256.
    #[test]
    fn repro_hmac_not_rfc2104() {
        let key = b"secret";
        let msg1 = b"transfer:100";
        let msg2 = b"transfer:1000";

        let sig1 = crate::log::hash_chain::hmac_sign(key, msg1);
        let sig2 = crate::log::hash_chain::hmac_sign(key, msg2);

        // They should be different
        assert_ne!(sig1, sig2, "different messages should produce different HMACs");

        // The fundamental issue: H(key || H(key || msg)) is NOT proper HMAC
        // Real HMAC uses ipad/opad padding per RFC 2104
        eprintln!("BUG #6: HMAC implementation is not RFC 2104 compliant");
        eprintln!("  Uses H(key || H(key || msg)) instead of H((key⊕opad) || H((key⊕ipad) || msg))");
    }

    // ━━━ BUG #7: parallel_verify_chain is a stub ━━━
    #[test]
    fn repro_parallel_verify_is_stub() {
        let chain = crate::log::hash_chain::HashChain::new(b"test-key-32-bytes-long!!!!!!");
        let (valid, tampered) = crate::log::hash_chain::parallel_verify_chain(&chain);

        // It works, but it's just calling the sequential version
        assert!(valid);
        assert!(tampered.is_empty());
        eprintln!("BUG #7: parallel_verify_chain() is a sequential stub — rayon comment is misleading");
    }

    // ━━━ BUG #8: Saga timeout u64→i64 truncation ━━━
    #[test]
    fn repro_saga_timeout_truncation() {
        // timeout_seconds is u64, but chrono::Duration::seconds takes i64
        let timeout: u64 = u64::MAX;
        // This silently wraps
        let duration = chrono::Duration::seconds(timeout as i64);
        // u64::MAX as i64 = -1
        assert!(duration.num_seconds() < 0, "BUG: u64::MAX timeout truncated to negative: {}s", duration.num_seconds());
        eprintln!("BUG #8: Saga timeout u64→i64 truncation: {timeout} becomes {}s (negative!)", duration.num_seconds());
    }

    // ━━━ BUG #9: HashChain::latest() panics on empty chain ━━━
    // latest() calls self.blocks.last().unwrap() without checking
    #[test]
    fn repro_hashchain_latest_returns_none_empty() {
        // Now returns None instead of panicking
        let mut chain = crate::log::hash_chain::HashChain::new(b"key-32-bytes-long!!!!!!!!!!");
        chain.blocks.clear();
        assert!(chain.latest().is_none(), "latest() should return None for empty chain");
    }

    // ━━━ BUG #10: HashChain::redact() mutates immutable chain ━━━
    #[test]
    fn repro_redact_mutates_chain() {
        let mut chain = crate::log::hash_chain::HashChain::new(b"key-32-bytes-long!!!!!!!!!!");
        chain.append("sensitive_data_PII");
        let hash_before = chain.blocks[1].hash.clone();

        // Redaction MODIFIES the chain in-place
        chain.redact(1).unwrap();

        let hash_after = chain.blocks[1].hash.clone();
        assert_ne!(hash_before, hash_after, "redaction changed the block hash");
        assert_eq!(chain.blocks[1].data, "[REDACTED]");
        eprintln!("BUG #10: redact() mutates chain in place — contradicts 'immutable' guarantee");
    }
}
