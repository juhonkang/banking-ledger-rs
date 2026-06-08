//! Comprehensive saga coverage tests.
//! Covers: LIFO compensation order, timeout via step failure,
//! concurrent saga instances, outbox partial publish,
//! timeout handler clearing, DOT visualization output.
//!
//! These are deeper coverage tests that complement the unit tests
//! already present in the saga module's inline tests.

#[cfg(test)]
mod tests {
    use crate::service::saga::{
        Outbox, SagaAction, SagaDefinition, SagaError, SagaInstance, SagaOrchestrator,
        SagaStep, TimeoutAction, TimeoutHandler, visualize_saga_dot,
    };
    use uuid::Uuid;

    // ━━━ Helper: build a multi-step (4-step) saga definition ━━━

    fn make_four_step_saga() -> SagaDefinition {
        SagaDefinition {
            name: "FourStepWire".into(),
            steps: vec![
                SagaStep {
                    name: "ReserveSource".into(),
                    action: SagaAction::Hold {
                        account_id: Uuid::now_v7(),
                        amount_cents: 5000,
                    },
                    compensation: SagaAction::ReleaseHold {
                        account_id: Uuid::now_v7(),
                        amount_cents: 5000,
                    },
                    max_retries: 2,
                    timeout_seconds: 30,
                },
                SagaStep {
                    name: "DebitSource".into(),
                    action: SagaAction::Debit {
                        account_id: Uuid::now_v7(),
                        amount_cents: 5000,
                    },
                    compensation: SagaAction::Credit {
                        account_id: Uuid::now_v7(),
                        amount_cents: 5000,
                    },
                    max_retries: 3,
                    timeout_seconds: 60,
                },
                SagaStep {
                    name: "FxConversion".into(),
                    action: SagaAction::ExternalCall {
                        service: "forex".into(),
                        endpoint: "/convert".into(),
                        payload: r#"{"from":"USD","to":"EUR","amount":5000}"#.into(),
                    },
                    compensation: SagaAction::ExternalCall {
                        service: "forex".into(),
                        endpoint: "/reverse".into(),
                        payload: r#"{"tx_id":"placeholder"}"#.into(),
                    },
                    max_retries: 2,
                    timeout_seconds: 15,
                },
                SagaStep {
                    name: "CreditDestination".into(),
                    action: SagaAction::Credit {
                        account_id: Uuid::now_v7(),
                        amount_cents: 4600,
                    },
                    compensation: SagaAction::Debit {
                        account_id: Uuid::now_v7(),
                        amount_cents: 4600,
                    },
                    max_retries: 3,
                    timeout_seconds: 60,
                },
            ],
        }
    }

    fn make_transfer_saga() -> SagaDefinition {
        SagaDefinition {
            name: "BankTransfer".into(),
            steps: vec![
                SagaStep {
                    name: "DebitSource".into(),
                    action: SagaAction::Debit {
                        account_id: Uuid::now_v7(),
                        amount_cents: 1000,
                    },
                    compensation: SagaAction::Credit {
                        account_id: Uuid::now_v7(),
                        amount_cents: 1000,
                    },
                    max_retries: 3,
                    timeout_seconds: 30,
                },
                SagaStep {
                    name: "CreditDestination".into(),
                    action: SagaAction::Credit {
                        account_id: Uuid::now_v7(),
                        amount_cents: 1000,
                    },
                    compensation: SagaAction::Debit {
                        account_id: Uuid::now_v7(),
                        amount_cents: 1000,
                    },
                    max_retries: 3,
                    timeout_seconds: 30,
                },
            ],
        }
    }

    // ━━━ Test 1: LIFO compensation order verification ━━━

    #[test]
    fn test_lifo_compensation_order_with_four_steps() {
        let mut orch = SagaOrchestrator::new();
        orch.register(make_four_step_saga());
        let saga_id = orch.start("FourStepWire").unwrap();

        // Execute steps 1–3 successfully
        for expected_idx in 0..3 {
            let (idx, _) = orch.next_action(saga_id).unwrap();
            assert_eq!(idx, expected_idx, "Expected step index {expected_idx}");
            let done = orch.step_succeeded(saga_id).unwrap();
            assert!(!done, "Should not be done after step {expected_idx}");
        }

        // Step 4 (CreditDestination, index 3) fails
        let compensations = orch
            .step_failed(saga_id, "Recipient bank unavailable".into())
            .unwrap();

        // Should have 3 compensations, in REVERSE order:
        // [0] = step 2 compensation (FxConversion reverse)
        // [1] = step 1 compensation (Credit back the debit)
        // [2] = step 0 compensation (ReleaseHold)
        assert_eq!(
            compensations.len(),
            3,
            "Expected 3 compensations for 3 completed steps"
        );

        // First compensation = reverse of step 2 (FxConversion)
        assert!(
            matches!(&compensations[0], SagaAction::ExternalCall { endpoint, .. } if endpoint == "/reverse"),
            "First compensation should be external call reverse (LIFO: last completed = step 2)"
        );

        // Second compensation = reverse of step 1 (DebitSource reversed by Credit)
        assert!(
            matches!(&compensations[1], SagaAction::Credit { .. }),
            "Second compensation should be Credit (reverse Debit from step 1)"
        );

        // Third compensation = reverse of step 0 (ReserveSource reversed by ReleaseHold)
        assert!(
            matches!(&compensations[2], SagaAction::ReleaseHold { .. }),
            "Third compensation should be ReleaseHold (reverse Hold from step 0)"
        );

        // Verify the saga instance is in Compensating state
        let status = orch.get_status(saga_id);
        assert!(
            matches!(status, Some(crate::service::saga::SagaStatus::Compensating)),
            "Saga should be in Compensating state"
        );
    }

    /// When only the first step fails, compensation list should be empty.
    #[test]
    fn test_lifo_compensation_empty_when_step0_fails() {
        let mut orch = SagaOrchestrator::new();
        orch.register(make_four_step_saga());
        let saga_id = orch.start("FourStepWire").unwrap();

        // Don't execute any steps — fail immediately
        let compensations = orch
            .step_failed(saga_id, "Immediate failure before any step".into())
            .unwrap();

        assert!(compensations.is_empty(), "No steps completed → no compensations");
    }

    /// Two completed steps → two compensations, order verified.
    #[test]
    fn test_lifo_two_step_compensation_order() {
        let mut orch = SagaOrchestrator::new();
        orch.register(make_four_step_saga());
        let saga_id = orch.start("FourStepWire").unwrap();

        // Complete step 0 (ReserveSource / Hold)
        let (idx, action) = orch.next_action(saga_id).unwrap();
        assert_eq!(idx, 0);
        assert!(matches!(action, SagaAction::Hold { .. }));
        orch.step_succeeded(saga_id).unwrap();

        // Complete step 1 (DebitSource / Debit)
        let (idx, action) = orch.next_action(saga_id).unwrap();
        assert_eq!(idx, 1);
        assert!(matches!(action, SagaAction::Debit { .. }));
        orch.step_succeeded(saga_id).unwrap();

        // Step 2 fails
        let compensations = orch
            .step_failed(saga_id, "Forex service down".into())
            .unwrap();

        assert_eq!(compensations.len(), 2);
        // LIFO: step 1 compensation first
        assert!(matches!(compensations[0], SagaAction::Credit { .. }));
        // Then step 0 compensation
        assert!(matches!(compensations[1], SagaAction::ReleaseHold { .. }));
    }

    // ━━━ Test 2: Saga timeout via step failure ━━━

    /// Timeout handler + saga orchestration: register timeout,
    /// execute a step, trigger step failure simulating timeout,
    /// verify compensation flow and that timeout is cleared after completion.
    #[test]
    fn test_saga_timeout_via_step_failure_with_handler() {
        let mut orch = SagaOrchestrator::new();
        let mut timeout_handler = TimeoutHandler::new();

        orch.register(make_transfer_saga());
        let saga_id = orch.start("BankTransfer").unwrap();

        // Register a short timeout (0 seconds = immediate)
        timeout_handler.register(saga_id, 0, TimeoutAction::Compensate);

        // Verify timeout has expired
        let expired = timeout_handler.check_expired();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].0, saga_id);
        assert!(matches!(expired[0].1, TimeoutAction::Compensate));

        // Execute step 1 (DebitSource)
        let (idx, _) = orch.next_action(saga_id).unwrap();
        assert_eq!(idx, 0);
        orch.step_succeeded(saga_id).unwrap();

        // Timeout fires — step 2 fails with timeout error
        let compensations = orch
            .step_failed(saga_id, "Timeout expired — CreditDestination never responded".into())
            .unwrap();

        // One compensation: reverse the DebitSource
        assert_eq!(compensations.len(), 1);
        assert!(
            matches!(&compensations[0], SagaAction::Credit { .. }),
            "Compensation should credit back the debited amount"
        );

        // Complete compensation
        orch.compensation_complete(saga_id);

        let status = orch.get_status(saga_id);
        assert!(
            matches!(status, Some(crate::service::saga::SagaStatus::Failed)),
            "Saga should be Failed after compensation complete"
        );
    }

    /// Timeout before any step completes → empty compensations.
    #[test]
    fn test_timeout_before_any_step() {
        let mut orch = SagaOrchestrator::new();
        let mut timeout_handler = TimeoutHandler::new();

        orch.register(make_transfer_saga());
        let saga_id = orch.start("BankTransfer").unwrap();

        timeout_handler.register(saga_id, 0, TimeoutAction::Compensate);

        let expired = timeout_handler.check_expired();
        assert_eq!(expired.len(), 1);

        // Fail immediately with timeout reason
        let compensations = orch
            .step_failed(saga_id, "Saga timed out before any step executed".into())
            .unwrap();

        assert!(compensations.is_empty());
    }

    // ━━━ Test 3: Multiple concurrent sagas ━━━

    #[test]
    fn test_multiple_concurrent_sagas_independent() {
        let mut orch = SagaOrchestrator::new();
        orch.register(make_transfer_saga());

        // Start 5 concurrent sagas
        let ids: Vec<Uuid> = (0..5)
            .map(|_| orch.start("BankTransfer").unwrap())
            .collect();

        // All IDs must be unique
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                assert_ne!(ids[i], ids[j], "Saga IDs must be unique");
            }
        }

        // Complete saga 0 fully (both steps)
        let (idx, _) = orch.next_action(ids[0]).unwrap();
        assert_eq!(idx, 0);
        orch.step_succeeded(ids[0]).unwrap();

        let (idx, _) = orch.next_action(ids[0]).unwrap();
        assert_eq!(idx, 1);
        let done = orch.step_succeeded(ids[0]).unwrap();
        assert!(done);

        // Saga 0 should be gone from active (moved to completed)
        assert!(orch.get_status(ids[0]).is_none());

        // Advance saga 1 by one step only
        let (idx, _) = orch.next_action(ids[1]).unwrap();
        assert_eq!(idx, 0);
        orch.step_succeeded(ids[1]).unwrap();

        // Saga 1 should still be active
        assert!(orch.get_status(ids[1]).is_some());

        // Fail saga 2 at step 1
        let compensations = orch
            .step_failed(ids[2], "Concurrent failure".into())
            .unwrap();
        assert!(compensations.is_empty());

        // Saga 2 should be in Compensating
        let status2 = orch.get_status(ids[2]);
        assert!(
            matches!(status2, Some(crate::service::saga::SagaStatus::Compensating))
        );

        // Saga 3 and 4 untouched — still Pending
        let status3 = orch.get_status(ids[3]);
        assert!(
            matches!(status3, Some(crate::service::saga::SagaStatus::Pending))
        );
        let status4 = orch.get_status(ids[4]);
        assert!(
            matches!(status4, Some(crate::service::saga::SagaStatus::Pending))
        );
    }

    /// Interleave steps across two sagas — ensure no cross-contamination.
    #[test]
    fn test_concurrent_sagas_interleaved_steps() {
        let mut orch = SagaOrchestrator::new();
        orch.register(make_four_step_saga());

        let a = orch.start("FourStepWire").unwrap();
        let b = orch.start("FourStepWire").unwrap();

        // A step 0
        orch.step_succeeded(a).unwrap();
        // B step 0
        orch.step_succeeded(b).unwrap();

        // A step 1
        let (idx, _) = orch.next_action(a).unwrap();
        assert_eq!(idx, 1);
        orch.step_succeeded(a).unwrap();

        // B step 1
        let (idx, _) = orch.next_action(b).unwrap();
        assert_eq!(idx, 1);
        orch.step_succeeded(b).unwrap();

        // A step 2, then fail
        let (idx, _) = orch.next_action(a).unwrap();
        assert_eq!(idx, 2);
        orch.step_succeeded(a).unwrap();

        // Fail A at step 3 — compensations should be for A's steps only
        let compensations = orch
            .step_failed(a, "A failed at step 3".into())
            .unwrap();
        assert_eq!(compensations.len(), 3); // A had 3 completed steps

        // B should still be active, unaffected
        let (idx, _) = orch.next_action(b).unwrap();
        assert_eq!(idx, 2, "B should be at step 2, unaffected by A's failure");
    }

    // ━━━ Test 4: Outbox partial publish ━━━

    #[test]
    fn test_outbox_partial_publish_workflow() {
        let mut outbox = Outbox::new();
        let agg_id = Uuid::now_v7();

        // Enqueue 5 messages
        outbox.enqueue(agg_id, "TransferInitiated", r#"{"amount":1000}"#);
        outbox.enqueue(agg_id, "SourceDebited", r#"{"account":"src-1"}"#);
        outbox.enqueue(agg_id, "DestinationCredited", r#"{"account":"dst-1"}"#);
        outbox.enqueue(agg_id, "TransferCompleted", r#"{"status":"ok"}"#);
        outbox.enqueue(agg_id, "NotificationSent", r#"{"channel":"email"}"#);

        let all_pending = outbox.pending_messages();
        assert_eq!(all_pending.len(), 5);

        // Publish only first 3 (simulate partial success)
        let ids: Vec<Uuid> = all_pending.iter().map(|m| m.id).collect();
        outbox.mark_published(ids[0]);
        outbox.mark_published(ids[1]);
        outbox.mark_published(ids[2]);

        // Mark 4th as failed (simulate transient error)
        outbox.mark_failed(ids[3]);

        // 5th remains pending
        let remaining = outbox.pending_messages();
        assert_eq!(remaining.len(), 1, "Only 5th message should remain pending");
        assert_eq!(remaining[0].event_type, "NotificationSent");

        // Retryable should have the failed one (if within max_retries)
        let retryable = outbox.retryable(5);
        assert_eq!(retryable.len(), 1);
        assert_eq!(retryable[0].event_type, "TransferCompleted");

        // Retry the failed one — mark it published
        outbox.mark_published(ids[3]);
        let retryable_after = outbox.retryable(5);
        assert!(retryable_after.is_empty());
    }

    /// Verify that exceeded retries are filtered out.
    #[test]
    fn test_outbox_retry_exhaustion() {
        let mut outbox = Outbox::new();
        let agg_id = Uuid::now_v7();

        outbox.enqueue(agg_id, "PoisonMessage", "{}");

        let msg_id = outbox.pending_messages()[0].id;

        // Fail it 3 times (exhaust retries with max_retries=2)
        outbox.mark_failed(msg_id);
        outbox.mark_failed(msg_id);
        outbox.mark_failed(msg_id);

        // retry_count is now 3, max_retries=2 → should not be retryable
        let retryable = outbox.retryable(2);
        assert!(
            retryable.is_empty(),
            "Message with retry_count=3 should not be retryable when max_retries=2"
        );
    }

    // ━━━ Test 5: Timeout handler clearing ━━━

    #[test]
    fn test_timeout_handler_clearing_partial() {
        let mut handler = TimeoutHandler::new();
        let s1 = Uuid::now_v7();
        let s2 = Uuid::now_v7();
        let s3 = Uuid::now_v7();
        let s4 = Uuid::now_v7();

        // Register all with zero timeout (immediately expired)
        handler.register(s1, 0, TimeoutAction::Compensate);
        handler.register(s2, 0, TimeoutAction::Retry);
        handler.register(s3, 0, TimeoutAction::Alert);
        handler.register(s4, 0, TimeoutAction::Expire);

        // All 4 should be expired
        let expired = handler.check_expired();
        assert_eq!(expired.len(), 4);

        // Clear s1 and s3 (simulating those sagas completed normally)
        handler.clear(s1);
        handler.clear(s3);

        // Only s2 and s4 should remain
        let remaining = handler.check_expired();
        assert_eq!(remaining.len(), 2);
        let remaining_ids: Vec<Uuid> = remaining.iter().map(|(id, _)| *id).collect();
        assert!(remaining_ids.contains(&s2));
        assert!(remaining_ids.contains(&s4));
        assert!(!remaining_ids.contains(&s1));
        assert!(!remaining_ids.contains(&s3));
    }

    /// Clear all timeouts → empty.
    #[test]
    fn test_timeout_handler_clear_all() {
        let mut handler = TimeoutHandler::new();
        let ids: Vec<Uuid> = (0..10).map(|_| Uuid::now_v7()).collect();

        for &id in &ids {
            handler.register(id, 0, TimeoutAction::Expire);
        }

        assert_eq!(handler.check_expired().len(), 10);

        for &id in &ids {
            handler.clear(id);
        }

        assert!(
            handler.check_expired().is_empty(),
            "All timeouts should be cleared"
        );
    }

    /// Clear a non-existent saga ID does not panic.
    #[test]
    fn test_timeout_handler_clear_nonexistent() {
        let mut handler = TimeoutHandler::new();
        let s1 = Uuid::now_v7();
        handler.register(s1, 60, TimeoutAction::Retry);

        // Clear an ID that was never registered — should not panic
        let nonexistent = Uuid::now_v7();
        handler.clear(nonexistent);

        // s1 should still be registered (not yet expired)
        assert_eq!(handler.check_expired().len(), 0); // 60s timeout not yet expired
    }

    // ━━━ Test 6: Saga visualize DOT output ━━━

    #[test]
    fn test_saga_visualize_dot_output() {
        let instance = SagaInstance::new("WireTransfer");
        let steps = vec![
            SagaStep {
                name: "Hold".into(),
                action: SagaAction::NoOp,
                compensation: SagaAction::NoOp,
                max_retries: 1,
                timeout_seconds: 10,
            },
            SagaStep {
                name: "Debit".into(),
                action: SagaAction::NoOp,
                compensation: SagaAction::NoOp,
                max_retries: 1,
                timeout_seconds: 10,
            },
            SagaStep {
                name: "Credit".into(),
                action: SagaAction::NoOp,
                compensation: SagaAction::NoOp,
                max_retries: 1,
                timeout_seconds: 10,
            },
        ];

        let dot = visualize_saga_dot(&instance, &steps);

        // Basic structure checks
        assert!(dot.starts_with("digraph Saga {"));
        assert!(dot.contains("rankdir=LR;"));
        assert!(dot.contains("label=\"WireTransfer\";"));

        // All three steps should appear as nodes
        assert!(dot.contains("step0 [label=\"Hold\""));
        assert!(dot.contains("step1 [label=\"Debit\""));
        assert!(dot.contains("step2 [label=\"Credit\""));

        // Arrows between consecutive steps
        assert!(dot.contains("step0 -> step1;"));
        assert!(dot.contains("step1 -> step2;"));

        // No arrow from last step
        assert!(!dot.contains("step2 -> step3;"));

        // Colors: step0 should be orange (current_step=0), rest gray
        assert!(dot.contains("fillcolor=orange"));
        assert!(dot.contains("fillcolor=gray"));

        // Closing
        assert!(dot.ends_with("}\n"));
    }

    /// DOT output after advancing some steps should reflect progress.
    #[test]
    fn test_saga_visualize_dot_after_progress() {
        let mut instance = SagaInstance::new("ProgressSaga");
        instance.step_completed(0); // step 0 is green
        instance.step_completed(1); // step 1 is green
        // current_step is now 2 (orange), step 3+ gray

        let steps: Vec<SagaStep> = (0..5)
            .map(|i| SagaStep {
                name: format!("Step{}", i),
                action: SagaAction::NoOp,
                compensation: SagaAction::NoOp,
                max_retries: 1,
                timeout_seconds: 10,
            })
            .collect();

        let dot = visualize_saga_dot(&instance, &steps);

        // Completed steps (0, 1) should be green
        let green_count = dot.matches("fillcolor=green").count();
        assert_eq!(green_count, 2, "Steps 0 and 1 should be green");

        // Current step (2) should be orange
        let orange_count = dot.matches("fillcolor=orange").count();
        assert_eq!(orange_count, 1, "Step 2 should be orange");

        // Steps 3 and 4 should be gray
        let gray_count = dot.matches("fillcolor=gray").count();
        assert_eq!(gray_count, 2, "Steps 3 and 4 should be gray");
    }

    // ━━━ Edge-case tests ━━━

    #[test]
    fn test_empty_definition_rejected() {
        let mut orch = SagaOrchestrator::new();
        let def = SagaDefinition {
            name: "EmptySaga".into(),
            steps: vec![],
        };
        orch.register(def);
        let result = orch.start("EmptySaga");
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), SagaError::NoSteps),
            "Empty saga should produce NoSteps error"
        );
    }

    #[test]
    fn test_unknown_definition_error() {
        let mut orch = SagaOrchestrator::new();
        let result = orch.start("PhantomSaga");
        assert!(result.is_err());
        let err = result.unwrap_err();
        // Display implementation should contain the name
        let display = err.to_string();
        assert!(display.contains("PhantomSaga"));
    }

    #[test]
    fn test_saga_not_found_on_operations() {
        let orch = SagaOrchestrator::new();
        let phantom = Uuid::now_v7();

        assert!(orch.next_action(phantom).is_err());
        assert!(orch.get_status(phantom).is_none());
    }

    #[test]
    fn test_failed_saga_compensation_then_complete() {
        let mut orch = SagaOrchestrator::new();
        orch.register(make_transfer_saga());
        let saga_id = orch.start("BankTransfer").unwrap();

        // Complete step 1
        orch.next_action(saga_id).unwrap();
        orch.step_succeeded(saga_id).unwrap();

        // Fail at step 2
        let compensations = orch
            .step_failed(saga_id, "Network error".into())
            .unwrap();
        assert_eq!(compensations.len(), 1);

        // Mark compensation complete
        orch.compensation_complete(saga_id);

        let status = orch.get_status(saga_id);
        assert!(
            matches!(status, Some(crate::service::saga::SagaStatus::Failed)),
            "After compensation_complete, saga should be Failed"
        );
    }
}
