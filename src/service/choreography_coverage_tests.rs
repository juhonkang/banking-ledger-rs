//! Comprehensive choreography saga coverage tests.
//! Covers: full lifecycle with notification, debit failure paths,
//! credit failure with compensation, idempotency on both paths,
//! and concurrent saga states.

#[cfg(test)]
mod tests {
    use crate::service::choreography::{
        ChoreographyAccountService, ChoreographyNotificationService,
        ChoreographyTransactionService, SagaState, TransferInitiatedEvent,
    };
    use chrono::Utc;
    use rust_decimal_macros::dec;
    use std::sync::Arc;
    use std::thread;
    use uuid::Uuid;

    // ━━━ Helper ━━━

    /// Helper: execute the full choreography happy path and return all intermediates.
    /// Useful as the "control" baseline for other tests.
    fn run_full_transfer(
        tx_svc: &ChoreographyTransactionService,
        acct_svc: &ChoreographyAccountService,
        from: &str,
        to: &str,
        amount: rust_decimal::Decimal,
    ) -> (
        TransferInitiatedEvent,
        crate::service::choreography::AccountDebitedEvent,
        crate::service::choreography::AccountCreditedEvent,
        crate::service::choreography::TransferCompletedEvent,
    ) {
        let initiated = tx_svc.initiate_transfer(from, to, amount);
        ChoreographyNotificationService::on_transfer_initiated(&initiated);

        let debited = acct_svc
            .handle_transfer_initiated(&initiated)
            .expect("debit should succeed");
        tx_svc.handle_account_debited(&debited);

        let credited = acct_svc
            .handle_account_debited(&debited, &initiated.to_account)
            .expect("credit should succeed");

        let completed = tx_svc.handle_account_credited(&credited);
        ChoreographyNotificationService::on_transfer_completed(&completed);

        (initiated, debited, credited, completed)
    }

    // ━━━ Test 1: Full Lifecycle Happy Path with Notification ━━━

    /// Verify every step of the full lifecycle: Initiated → Debited → Credited → Completed.
    /// Assert every intermediate saga state AND the final balance.
    #[test]
    fn test_full_lifecycle_happy_path_with_notification() {
        let tx_svc = ChoreographyTransactionService::new();
        let acct_svc = ChoreographyAccountService::new();

        // Seed accounts
        acct_svc.seed("alice", dec!(5000.00));
        acct_svc.seed("bob", dec!(300.00));

        // Step 1: Initiate
        let initiated = tx_svc.initiate_transfer("alice", "bob", dec!(1200.00));
        assert!(!initiated.transaction_id.is_empty());
        assert_eq!(initiated.from_account, "alice");
        assert_eq!(initiated.to_account, "bob");
        assert_eq!(initiated.amount, dec!(1200.00));
        assert_eq!(
            *tx_svc.states.get(&initiated.transaction_id).unwrap(),
            SagaState::Initiated
        );

        // Notification: transfer initiated
        ChoreographyNotificationService::on_transfer_initiated(&initiated);

        // Step 2: Debit alice
        let debited = acct_svc
            .handle_transfer_initiated(&initiated)
            .expect("debit should succeed");
        assert_eq!(debited.account_id, "alice");
        assert_eq!(debited.amount, dec!(1200.00));
        assert_eq!(debited.correlation_id, initiated.correlation_id);

        tx_svc.handle_account_debited(&debited);
        assert_eq!(
            *tx_svc.states.get(&initiated.transaction_id).unwrap(),
            SagaState::Debited
        );

        // Verify intermediate balance
        assert_eq!(*acct_svc.balances.get("alice").unwrap(), dec!(3800.00));
        // Bob unchanged yet
        assert_eq!(*acct_svc.balances.get("bob").unwrap(), dec!(300.00));

        // Step 3: Credit bob
        let credited = acct_svc
            .handle_account_debited(&debited, &initiated.to_account)
            .expect("credit should succeed");
        assert_eq!(credited.account_id, "bob");
        assert_eq!(credited.amount, dec!(1200.00));

        // Step 4: Complete
        let completed = tx_svc.handle_account_credited(&credited);
        assert_eq!(completed.transaction_id, initiated.transaction_id);
        assert_eq!(completed.correlation_id, initiated.correlation_id);

        // Notification: transfer completed
        ChoreographyNotificationService::on_transfer_completed(&completed);

        // Final state
        assert_eq!(
            *tx_svc.states.get(&initiated.transaction_id).unwrap(),
            SagaState::Completed
        );
        assert_eq!(*acct_svc.balances.get("alice").unwrap(), dec!(3800.00));
        assert_eq!(*acct_svc.balances.get("bob").unwrap(), dec!(1500.00));
    }

    // ━━━ Test 2: Debit Failure — Account Not Found ━━━

    /// When the source account doesn't exist, debit fails with ACCOUNT_NOT_FOUND.
    /// The saga transitions to Failed state and notification fires.
    #[test]
    fn test_debit_failure_account_not_found() {
        let tx_svc = ChoreographyTransactionService::new();
        let acct_svc = ChoreographyAccountService::new();

        // No accounts seeded — alice is missing

        let initiated = tx_svc.initiate_transfer("alice", "bob", dec!(500.00));
        let result = acct_svc.handle_transfer_initiated(&initiated);

        assert!(result.is_err(), "debit should fail when account is not found");
        let failed = result.unwrap_err();
        assert_eq!(failed.reason, "ACCOUNT_NOT_FOUND");
        assert_eq!(failed.account_id, "alice");
        assert_eq!(failed.amount, dec!(500.00));

        // Saga transitions to Failed
        let tf = tx_svc.handle_debit_failed(&failed);
        ChoreographyNotificationService::on_transfer_failed(&tf);

        assert_eq!(
            *tx_svc.states.get(&initiated.transaction_id).unwrap(),
            SagaState::Failed
        );
        assert_eq!(tf.reason, "ACCOUNT_NOT_FOUND");

        // Balances: nothing was touched (alice never existed)
        assert!(acct_svc.balances.get("alice").is_none());
        assert!(acct_svc.balances.get("bob").is_none());
    }

    // ━━━ Test 3: Debit Failure — Insufficient Funds ━━━

    /// When the source account has insufficient balance, debit fails with
    /// INSUFFICIENT_FUNDS. The original balance is preserved.
    #[test]
    fn test_debit_failure_insufficient_funds_preserves_balance() {
        let tx_svc = ChoreographyTransactionService::new();
        let acct_svc = ChoreographyAccountService::new();
        acct_svc.seed("alice", dec!(50.00));

        let initiated = tx_svc.initiate_transfer("alice", "bob", dec!(500.00));
        let result = acct_svc.handle_transfer_initiated(&initiated);

        assert!(result.is_err(), "debit should fail for insufficient funds");
        let failed = result.unwrap_err();
        assert_eq!(failed.reason, "INSUFFICIENT_FUNDS");

        let tf = tx_svc.handle_debit_failed(&failed);
        ChoreographyNotificationService::on_transfer_failed(&tf);

        assert_eq!(
            *tx_svc.states.get(&initiated.transaction_id).unwrap(),
            SagaState::Failed
        );

        // Balance preserved — no money moved
        assert_eq!(*acct_svc.balances.get("alice").unwrap(), dec!(50.00));
    }

    // ━━━ Test 4: Credit Failure with Compensation Refund ━━━

    /// When destination account is missing, the credit fails, triggering
    /// a compensating refund back to the source account. Verify the exact
    /// refund amounts and final balances.
    #[test]
    fn test_credit_failure_compensation_refund_verification() {
        let tx_svc = ChoreographyTransactionService::new();
        let acct_svc = ChoreographyAccountService::new();

        // Only alice exists — bob is NOT seeded
        acct_svc.seed("alice", dec!(10000.00));

        let initiated = tx_svc.initiate_transfer("alice", "bob", dec!(2500.00));

        // Debit succeeds
        let debited = acct_svc
            .handle_transfer_initiated(&initiated)
            .expect("debit should succeed");
        tx_svc.handle_account_debited(&debited);

        // Balance after debit
        assert_eq!(*acct_svc.balances.get("alice").unwrap(), dec!(7500.00));

        // Credit to unseeded bob fails with compensation
        let credit_result = acct_svc.handle_account_debited(&debited, "bob");
        assert!(credit_result.is_err(), "credit should fail with missing destination");
        let credit_failed = credit_result.unwrap_err();
        assert_eq!(credit_failed.reason, "DESTINATION_NOT_FOUND");
        assert_eq!(credit_failed.account_id, "bob");
        assert_eq!(credit_failed.amount, dec!(2500.00));

        // Saga orchestrator triggers compensation
        let (transfer_failed, refund) = tx_svc.handle_credit_failed(&credit_failed);
        ChoreographyNotificationService::on_transfer_failed(&transfer_failed);

        assert_eq!(transfer_failed.reason, "DESTINATION_NOT_FOUND");
        assert_eq!(refund.account_id, "bob"); // refund source is the failed destination
        assert_eq!(refund.amount, dec!(2500.00));
        assert_eq!(refund.transaction_id, initiated.transaction_id);

        // KEY: alice is fully refunded — balance restored to original
        assert_eq!(
            *acct_svc.balances.get("alice").unwrap(),
            dec!(10000.00),
            "source account must be fully refunded after compensation"
        );

        // bob was never credited (never existed)
        assert!(acct_svc.balances.get("bob").is_none());

        // Saga in Failed state
        assert_eq!(
            *tx_svc.states.get(&initiated.transaction_id).unwrap(),
            SagaState::Failed
        );
    }

    // ━━━ Test 5: Idempotency — Duplicate Events on Both Debit and Credit Paths ━━━

    /// Both debit and credit handlers are idempotent. Replaying the same
    /// correlation_id should return Ok (synthetic event) without modifying
    /// balances a second time.
    #[test]
    fn test_idempotency_duplicate_events_both_paths() {
        let acct_svc = ChoreographyAccountService::new();
        acct_svc.seed("alice", dec!(1000.00));
        acct_svc.seed("bob", dec!(0.00));

        // Create a fixed correlation_id so we can replay
        let cid = Uuid::now_v7();
        let tid = "tx-idempotency-test".to_string();

        let initiated = TransferInitiatedEvent {
            correlation_id: cid,
            transaction_id: tid.clone(),
            from_account: "alice".to_string(),
            to_account: "bob".to_string(),
            amount: dec!(200.00),
            timestamp: Utc::now(),
        };

        // ━━━ Debit path idempotency ━━━

        // First debit
        let r1 = acct_svc.handle_transfer_initiated(&initiated);
        assert!(r1.is_ok(), "first debit should succeed");
        assert_eq!(*acct_svc.balances.get("alice").unwrap(), dec!(800.00));

        // Second debit with same correlation_id — idempotent
        let r2 = acct_svc.handle_transfer_initiated(&initiated);
        assert!(r2.is_ok(), "duplicate debit should be idempotent (return Ok)");
        // Balance only debited once
        assert_eq!(
            *acct_svc.balances.get("alice").unwrap(),
            dec!(800.00),
            "duplicate debit must not double-debit"
        );

        // Create the debited event for credit idempotency test
        let debited = r1.unwrap();

        // ━━━ Credit path idempotency ━━━

        // First credit
        let c1 = acct_svc.handle_account_debited(&debited, "bob");
        assert!(c1.is_ok(), "first credit should succeed");
        assert_eq!(*acct_svc.balances.get("bob").unwrap(), dec!(200.00));

        // Second credit with same correlation_id — idempotent
        let c2 = acct_svc.handle_account_debited(&debited, "bob");
        assert!(c2.is_ok(), "duplicate credit should be idempotent (return Ok)");
        // Balance only credited once
        assert_eq!(
            *acct_svc.balances.get("bob").unwrap(),
            dec!(200.00),
            "duplicate credit must not double-credit"
        );
    }

    // ━━━ Test 6: Concurrent Saga States — Independent Transfers ━━━

    /// Multiple concurrent transfers must not interfere with each other.
    /// Each transaction maintains its own saga state in the DashMap.
    #[test]
    fn test_concurrent_sagas_independent_states() {
        let tx_svc = Arc::new(ChoreographyTransactionService::new());
        let acct_svc = Arc::new(ChoreographyAccountService::new());

        // Seed multiple accounts
        acct_svc.seed("alice", dec!(10000.00));
        acct_svc.seed("bob", dec!(1000.00));
        acct_svc.seed("charlie", dec!(5000.00));
        acct_svc.seed("dave", dec!(100.00));

        // Start 3 transfers, each should be fully independent
        let txs = vec![
            ("alice", "bob", dec!(500.00)),
            ("alice", "charlie", dec!(300.00)),
            ("charlie", "dave", dec!(150.00)),
        ];

        let mut handles = vec![];

        for (from, to, amount) in txs {
            let tx_svc = Arc::clone(&tx_svc);
            let acct_svc = Arc::clone(&acct_svc);
            handles.push(thread::spawn(move || {
                run_full_transfer(&tx_svc, &acct_svc, from, to, amount)
            }));
        }

        // Collect results — all must complete successfully
        let mut transaction_ids = vec![];
        let mut all_completed = vec![];

        for handle in handles {
            let (initiated, _, _, completed) = handle.join().expect("thread should not panic");
            transaction_ids.push(initiated.transaction_id.clone());
            all_completed.push(completed);
        }

        // All 3 transfers reached Completed state
        for tid in &transaction_ids {
            assert_eq!(
                *tx_svc.states.get(tid).unwrap(),
                SagaState::Completed,
                "every concurrent saga must complete independently"
            );
        }

        // Final balances must be correct
        // alice: 10000 - 500 - 300 = 9200
        // bob: 1000 + 500 = 1500
        // charlie: 5000 - 150 + 300 = 5150
        // dave: 100 + 150 = 250
        assert_eq!(*acct_svc.balances.get("alice").unwrap(), dec!(9200.00));
        assert_eq!(*acct_svc.balances.get("bob").unwrap(), dec!(1500.00));
        assert_eq!(*acct_svc.balances.get("charlie").unwrap(), dec!(5150.00));
        assert_eq!(*acct_svc.balances.get("dave").unwrap(), dec!(250.00));
    }

    // ━━━ Test 7: Concurrent Saga with One Failure — No Cross-Contamination ━━━

    /// When one saga fails while another succeeds concurrently, the failure
    /// must not affect the successful saga's state or balance.
    #[test]
    fn test_concurrent_sagas_one_failure_no_cross_contamination() {
        let tx_svc = Arc::new(ChoreographyTransactionService::new());
        let acct_svc = Arc::new(ChoreographyAccountService::new());

        acct_svc.seed("alice", dec!(1000.00));
        acct_svc.seed("bob", dec!(0.00));
        // charlie NOT seeded — will cause failure

        let tx_svc_a = Arc::clone(&tx_svc);
        let acct_svc_a = Arc::clone(&acct_svc);

        // Transfer A: alice → bob (should succeed)
        let handle_ok = thread::spawn(move || {
            run_full_transfer(&tx_svc_a, &acct_svc_a, "alice", "bob", dec!(300.00))
        });

        // Transfer B: alice → charlie (debit succeeds, credit fails → compensation refunds)
        let tx_svc_b = Arc::clone(&tx_svc);
        let acct_svc_b = Arc::clone(&acct_svc);
        let handle_fail = thread::spawn(move || {
            let initiated = tx_svc_b.initiate_transfer("alice", "charlie", dec!(100.00));

            let debited = acct_svc_b
                .handle_transfer_initiated(&initiated)
                .expect("debit should succeed");
            tx_svc_b.handle_account_debited(&debited);

            // Credit to charlie fails (not seeded)
            let credit_result = acct_svc_b.handle_account_debited(&debited, "charlie");
            assert!(credit_result.is_err(), "credit should fail");

            let failed = credit_result.unwrap_err();
            let (tf, _refund) = tx_svc_b.handle_credit_failed(&failed);
            ChoreographyNotificationService::on_transfer_failed(&tf);

            (initiated, tf)
        });

        // Wait for both
        let (ok_initiated, _, _, _ok_completed) =
            handle_ok.join().expect("success saga should not panic");
        let (fail_initiated, fail_tf) =
            handle_fail.join().expect("failure saga should not panic");

        // Successful saga: Completed
        assert_eq!(
            *tx_svc
                .states
                .get(&ok_initiated.transaction_id)
                .unwrap(),
            SagaState::Completed
        );

        // Failed saga: Failed
        assert_eq!(
            *tx_svc
                .states
                .get(&fail_initiated.transaction_id)
                .unwrap(),
            SagaState::Failed
        );
        assert_eq!(fail_tf.reason, "DESTINATION_NOT_FOUND");

        // Final balances:
        // alice: 1000 - 300 (successful transfer to bob) - 100 (failed transfer debit)
        //        + 100 (compensation refund) = 700
        // bob: 0 + 300 = 300
        // charlie: not seeded, no balance
        assert_eq!(*acct_svc.balances.get("alice").unwrap(), dec!(700.00));
        assert_eq!(*acct_svc.balances.get("bob").unwrap(), dec!(300.00));
        assert!(acct_svc.balances.get("charlie").is_none());
    }

    // ━━━ Test 8: Idempotency — Duplicate Transfer Initiation ━━━

    /// The same TransferInitiatedEvent processed twice on the debit side
    /// is idempotent even when the correlation_id was already recorded.
    /// This simulates an at-least-once delivery scenario.
    #[test]
    fn test_idempotency_duplicate_transfer_initiation_at_least_once() {
        let tx_svc = ChoreographyTransactionService::new();
        let acct_svc = ChoreographyAccountService::new();
        acct_svc.seed("alice", dec!(500.00));
        acct_svc.seed("bob", dec!(100.00));

        let initiated = tx_svc.initiate_transfer("alice", "bob", dec!(100.00));

        // Process the transfer fully using the SAME initiated event
        // (do NOT call run_full_transfer — it creates a new transfer internally)
        ChoreographyNotificationService::on_transfer_initiated(&initiated);

        let debited = acct_svc
            .handle_transfer_initiated(&initiated)
            .expect("debit should succeed");
        tx_svc.handle_account_debited(&debited);

        let credited = acct_svc
            .handle_account_debited(&debited, &initiated.to_account)
            .expect("credit should succeed");

        let completed = tx_svc.handle_account_credited(&credited);
        ChoreographyNotificationService::on_transfer_completed(&completed);

        // Balances after first execution
        assert_eq!(*acct_svc.balances.get("alice").unwrap(), dec!(400.00));
        assert_eq!(*acct_svc.balances.get("bob").unwrap(), dec!(200.00));

        // Now replay the SAME TransferInitiatedEvent (simulating at-least-once)
        // The debit handler should detect the duplicate correlation_id and return Ok
        let replayed = acct_svc.handle_transfer_initiated(&initiated);
        assert!(
            replayed.is_ok(),
            "replaying the same initiation event must be idempotent"
        );

        // Balances unchanged by replay — no double-debit
        assert_eq!(*acct_svc.balances.get("alice").unwrap(), dec!(400.00));
        assert_eq!(*acct_svc.balances.get("bob").unwrap(), dec!(200.00));
    }
}
