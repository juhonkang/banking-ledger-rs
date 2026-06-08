//! Saga orchestrator with compensating transactions and transactional outbox.
//! Long-lived transactions decomposed into steps — each step has a compensating action.
//! If any step fails, compensation runs in reverse order.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use uuid::Uuid;

pub type SagaId = Uuid;

// ━━━ Saga State Machine ━━━

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SagaStatus {
    /// Saga has been created but not yet started
    Pending,
    /// Saga is executing steps
    InProgress,
    /// All steps completed successfully
    Completed,
    /// A step failed, compensating transactions are running
    Compensating,
    /// Compensation completed, saga is failed
    Failed,
    /// Saga timed out
    TimedOut,
}

/// A single step in a saga.
#[derive(Debug, Clone)]
pub struct SagaStep {
    pub name: String,
    /// What action to execute
    pub action: SagaAction,
    /// How to undo this step if a later step fails
    pub compensation: SagaAction,
    /// Maximum retries for this step
    pub max_retries: u32,
    /// Timeout for this step
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone)]
pub enum SagaAction {
    /// Debit an account
    Debit { account_id: Uuid, amount_cents: i64 },
    /// Credit an account
    Credit { account_id: Uuid, amount_cents: i64 },
    /// Place a hold on an account
    Hold { account_id: Uuid, amount_cents: i64 },
    /// Release a hold
    ReleaseHold { account_id: Uuid, amount_cents: i64 },
    /// External API call
    ExternalCall {
        service: String,
        endpoint: String,
        payload: String,
    },
    /// No-op (for simple sagas)
    NoOp,
}

/// Saga definition — the blueprint for a specific saga type.
#[derive(Debug, Clone)]
pub struct SagaDefinition {
    pub name: String,
    pub steps: Vec<SagaStep>,
}

/// A running saga instance.
#[derive(Debug, Clone)]
pub struct SagaInstance {
    pub id: SagaId,
    pub definition_name: String,
    pub status: SagaStatus,
    pub current_step: usize,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    /// Steps that have been completed (for rollback)
    pub completed_steps: Vec<usize>,
    pub error: Option<String>,
}

impl SagaInstance {
    pub fn new(definition_name: &str) -> Self {
        Self {
            id: Uuid::now_v7(),
            definition_name: definition_name.to_string(),
            status: SagaStatus::Pending,
            current_step: 0,
            created_at: Utc::now(),
            completed_at: None,
            completed_steps: Vec::new(),
            error: None,
        }
    }

    /// Record that the current step was completed successfully
    pub fn step_completed(&mut self, step_index: usize) {
        self.completed_steps.push(step_index);
        self.current_step = step_index + 1;
    }

    /// Mark saga as completed
    pub fn complete(&mut self) {
        self.status = SagaStatus::Completed;
        self.completed_at = Some(Utc::now());
    }

    /// Start compensating (rolling back)
    pub fn start_compensating(&mut self, error: String) {
        self.status = SagaStatus::Compensating;
        self.error = Some(error);
    }

    /// Mark as failed (compensation finished)
    pub fn fail(&mut self) {
        self.status = SagaStatus::Failed;
        self.completed_at = Some(Utc::now());
    }
}

// ━━━ Orchestrator ━━━

/// Saga orchestrator — manages saga lifecycle.
/// Orchestration: central coordinator tracks progress and invokes steps.
pub struct SagaOrchestrator {
    definitions: HashMap<String, SagaDefinition>,
    active_sagas: HashMap<SagaId, SagaInstance>,
    completed_sagas: VecDeque<SagaInstance>,
}

impl SagaOrchestrator {
    pub fn new() -> Self {
        Self {
            definitions: HashMap::new(),
            active_sagas: HashMap::new(),
            completed_sagas: VecDeque::new(),
        }
    }

    /// Register a saga definition
    pub fn register(&mut self, definition: SagaDefinition) {
        self.definitions.insert(definition.name.clone(), definition);
    }

    /// Start a new saga instance
    pub fn start(&mut self, definition_name: &str) -> Result<SagaId, SagaError> {
        let def = self
            .definitions
            .get(definition_name)
            .ok_or_else(|| SagaError::UnknownDefinition(definition_name.to_string()))?;

        if def.steps.is_empty() {
            return Err(SagaError::NoSteps);
        }

        let instance = SagaInstance::new(definition_name);
        let id = instance.id;
        self.active_sagas.insert(id, instance);
        Ok(id)
    }

    /// Execute the next step of a saga.
    /// Returns the action to perform — caller executes it.
    pub fn next_action(&self, saga_id: SagaId) -> Result<(usize, SagaAction), SagaError> {
        let instance = self
            .active_sagas
            .get(&saga_id)
            .ok_or(SagaError::SagaNotFound(saga_id))?;

        if instance.status != SagaStatus::Pending && instance.status != SagaStatus::InProgress {
            return Err(SagaError::SagaNotActive(instance.status));
        }

        let def = self.definitions.get(&instance.definition_name).expect("saga definition removed between calls — concurrent modification");

        if instance.current_step >= def.steps.len() {
            return Err(SagaError::AllStepsCompleted);
        }

        let step = &def.steps[instance.current_step];
        Ok((instance.current_step, step.action.clone()))
    }

    /// Mark the current step as completed successfully
    pub fn step_succeeded(&mut self, saga_id: SagaId) -> Result<bool, SagaError> {
        let instance = self
            .active_sagas
            .get_mut(&saga_id)
            .ok_or(SagaError::SagaNotFound(saga_id))?;

        let def = self.definitions.get(&instance.definition_name).expect("saga definition removed between calls — concurrent modification");
        let step_index = instance.current_step;

        instance.step_completed(step_index);
        instance.status = SagaStatus::InProgress;

        if instance.current_step >= def.steps.len() {
            instance.complete();
            let completed = instance.clone();
            self.active_sagas.remove(&saga_id);
            self.completed_sagas.push_back(completed);
            return Ok(true); // Saga is done
        }

        Ok(false) // More steps to go
    }

    /// A step failed — get compensation actions in reverse order
    pub fn step_failed(
        &mut self,
        saga_id: SagaId,
        error: String,
    ) -> Result<Vec<SagaAction>, SagaError> {
        let instance = self
            .active_sagas
            .get_mut(&saga_id)
            .ok_or(SagaError::SagaNotFound(saga_id))?;

        instance.start_compensating(error);

        let def = self.definitions.get(&instance.definition_name).expect("saga definition removed between calls — concurrent modification");

        // Collect compensations for completed steps in REVERSE order
        let compensations: Vec<SagaAction> = instance
            .completed_steps
            .iter()
            .rev()
            .map(|&idx| def.steps[idx].compensation.clone())
            .collect();

        Ok(compensations)
    }

    /// Mark compensation as complete
    pub fn compensation_complete(&mut self, saga_id: SagaId) {
        if let Some(instance) = self.active_sagas.get_mut(&saga_id) {
            instance.fail();
        }
    }

    /// Get saga status
    pub fn get_status(&self, saga_id: SagaId) -> Option<SagaStatus> {
        self.active_sagas.get(&saga_id).map(|s| s.status)
    }
}

impl Default for SagaOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

// ━━━ Outbox Pattern ━━━

/// An outbox message — written in same DB transaction as the business data.
/// Guarantees at-least-once delivery to external systems.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboxMessage {
    pub id: Uuid,
    pub aggregate_id: Uuid,
    pub event_type: String,
    pub payload: String,
    pub created_at: DateTime<Utc>,
    pub status: OutboxStatus,
    pub retry_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutboxStatus {
    Pending,
    Published,
    Failed,
}

/// Outbox — buffers messages for reliable delivery.
pub struct Outbox {
    messages: Vec<OutboxMessage>,
}

impl Outbox {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }

    /// Add a message to the outbox (called within a transaction)
    pub fn enqueue(&mut self, aggregate_id: Uuid, event_type: &str, payload: &str) {
        self.messages.push(OutboxMessage {
            id: Uuid::now_v7(),
            aggregate_id,
            event_type: event_type.to_string(),
            payload: payload.to_string(),
            created_at: Utc::now(),
            status: OutboxStatus::Pending,
            retry_count: 0,
        });
    }

    /// Get all pending messages (for the outbox poller to publish)
    pub fn pending_messages(&self) -> Vec<&OutboxMessage> {
        self.messages
            .iter()
            .filter(|m| m.status == OutboxStatus::Pending)
            .collect()
    }

    /// Mark a message as published
    pub fn mark_published(&mut self, message_id: Uuid) {
        if let Some(msg) = self.messages.iter_mut().find(|m| m.id == message_id) {
            msg.status = OutboxStatus::Published;
        }
    }

    /// Mark as failed and increment retry count
    pub fn mark_failed(&mut self, message_id: Uuid) {
        if let Some(msg) = self.messages.iter_mut().find(|m| m.id == message_id) {
            msg.status = OutboxStatus::Failed;
            msg.retry_count += 1;
        }
    }

    /// Messages that can be retried
    pub fn retryable(&self, max_retries: u32) -> Vec<&OutboxMessage> {
        self.messages
            .iter()
            .filter(|m| m.status == OutboxStatus::Failed && m.retry_count < max_retries)
            .collect()
    }
}

impl Default for Outbox {
    fn default() -> Self {
        Self::new()
    }
}

// ━━━ Timeout Handlers ━━━

/// Timer for sagas and transactions that need timeout handling.
pub struct TimeoutHandler {
    timeouts: Vec<TimeoutEntry>,
}

#[derive(Debug, Clone)]
struct TimeoutEntry {
    saga_id: SagaId,
    deadline: DateTime<Utc>,
    callback: TimeoutAction,
}

#[derive(Debug, Clone)]
pub enum TimeoutAction {
    Expire,
    Compensate,
    Retry,
    Alert,
}

impl TimeoutHandler {
    pub fn new() -> Self {
        Self { timeouts: vec![] }
    }

    /// Register a timeout for a saga
    pub fn register(&mut self, saga_id: SagaId, timeout_seconds: u64, action: TimeoutAction) {
        // Clamp to i64::MAX to prevent silent truncation
        let secs = timeout_seconds.min(i64::MAX as u64) as i64;
        self.timeouts.push(TimeoutEntry {
            saga_id,
            deadline: Utc::now() + chrono::Duration::seconds(secs),
            callback: action,
        });
    }

    /// Check for expired timeouts
    pub fn check_expired(&self) -> Vec<(SagaId, TimeoutAction)> {
        let now = Utc::now();
        self.timeouts
            .iter()
            .filter(|t| t.deadline <= now)
            .map(|t| (t.saga_id, t.callback.clone()))
            .collect()
    }

    /// Remove timeout entries for a saga (on completion)
    pub fn clear(&mut self, saga_id: SagaId) {
        self.timeouts.retain(|t| t.saga_id != saga_id);
    }
}

impl Default for TimeoutHandler {
    fn default() -> Self {
        Self::new()
    }
}

// ━━━ Visualization ━━━

/// Simple DOT graph generator for saga state visualization.
pub fn visualize_saga_dot(instance: &SagaInstance, steps: &[SagaStep]) -> String {
    let mut dot = String::from("digraph Saga {\n");
    dot.push_str("  rankdir=LR;\n");
    dot.push_str(&format!("  label=\"{}\";\n", instance.definition_name));

    for (i, step) in steps.iter().enumerate() {
        let color = if i < instance.current_step {
            "green"
        } else if i == instance.current_step {
            "orange"
        } else {
            "gray"
        };
        dot.push_str(&format!(
            "  step{} [label=\"{}\", style=filled, fillcolor={}];\n",
            i, step.name, color
        ));
        if i > 0 {
            dot.push_str(&format!("  step{} -> step{};\n", i - 1, i));
        }
    }
    dot.push_str("}\n");
    dot
}

#[derive(Debug)]
pub enum SagaError {
    UnknownDefinition(String),
    NoSteps,
    SagaNotFound(SagaId),
    SagaNotActive(SagaStatus),
    AllStepsCompleted,
}

impl std::fmt::Display for SagaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownDefinition(name) => write!(f, "Unknown saga definition: {name}"),
            Self::NoSteps => write!(f, "Saga definition has no steps"),
            Self::SagaNotFound(id) => write!(f, "Saga not found: {id}"),
            Self::SagaNotActive(status) => write!(f, "Saga is not active: {status:?}"),
            Self::AllStepsCompleted => write!(f, "All saga steps already completed"),
        }
    }
}

impl std::error::Error for SagaError {}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_saga_happy_path() {
        let mut orch = SagaOrchestrator::new();
        orch.register(make_transfer_saga());

        let saga_id = orch.start("BankTransfer").unwrap();

        // Step 1
        let (idx, action) = orch.next_action(saga_id).unwrap();
        assert_eq!(idx, 0);
        assert!(matches!(action, SagaAction::Debit { .. }));

        let done = orch.step_succeeded(saga_id).unwrap();
        assert!(!done); // Not done yet

        // Step 2
        let (idx, _action) = orch.next_action(saga_id).unwrap();
        assert_eq!(idx, 1);

        let done = orch.step_succeeded(saga_id).unwrap();
        assert!(done); // Done!

        assert_eq!(orch.get_status(saga_id), None); // Moved to completed
    }

    #[test]
    fn test_saga_compensation() {
        let mut orch = SagaOrchestrator::new();
        orch.register(make_transfer_saga());

        let saga_id = orch.start("BankTransfer").unwrap();

        // Step 1 succeeds
        let (_idx, _) = orch.next_action(saga_id).unwrap();
        orch.step_succeeded(saga_id).unwrap();

        // Step 2 fails
        let compensations = orch
            .step_failed(saga_id, "Destination account frozen".into())
            .unwrap();

        // Should have 1 compensation (reverse of step 1)
        assert_eq!(compensations.len(), 1);
        assert!(matches!(compensations[0], SagaAction::Credit { .. }));
    }

    #[test]
    fn test_outbox_enqueue_and_publish() {
        let mut outbox = Outbox::new();
        let agg_id = Uuid::now_v7();

        outbox.enqueue(agg_id, "FundsTransferred", r#"{"amount":1000}"#);

        let pending = outbox.pending_messages();
        assert_eq!(pending.len(), 1);

        let msg_id = pending[0].id;
        outbox.mark_published(msg_id);

        assert_eq!(outbox.pending_messages().len(), 0);
    }

    #[test]
    fn test_timeout_handler() {
        let mut handler = TimeoutHandler::new();
        let saga_id = Uuid::now_v7();

        // Immediate timeout
        handler.register(saga_id, 0, TimeoutAction::Compensate);

        let expired = handler.check_expired();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].0, saga_id);

        handler.clear(saga_id);
        assert!(handler.check_expired().is_empty());
    }

    // ━━━ Saga Edge Cases ━━━

    fn make_three_step_saga() -> SagaDefinition {
        SagaDefinition {
            name: "ThreeStep".into(),
            steps: vec![
                SagaStep {
                    name: "Step1".into(),
                    action: SagaAction::Debit { account_id: Uuid::now_v7(), amount_cents: 100 },
                    compensation: SagaAction::Credit { account_id: Uuid::now_v7(), amount_cents: 100 },
                    max_retries: 2,
                    timeout_seconds: 30,
                },
                SagaStep {
                    name: "Step2".into(),
                    action: SagaAction::Credit { account_id: Uuid::now_v7(), amount_cents: 50 },
                    compensation: SagaAction::Debit { account_id: Uuid::now_v7(), amount_cents: 50 },
                    max_retries: 2,
                    timeout_seconds: 30,
                },
                SagaStep {
                    name: "Step3".into(),
                    action: SagaAction::Credit { account_id: Uuid::now_v7(), amount_cents: 25 },
                    compensation: SagaAction::Debit { account_id: Uuid::now_v7(), amount_cents: 25 },
                    max_retries: 2,
                    timeout_seconds: 30,
                },
            ],
        }
    }

    #[test]
    fn test_saga_not_found() {
        let orch = SagaOrchestrator::new();
        let result = orch.next_action(Uuid::now_v7());
        assert!(result.is_err());
    }

    #[test]
    fn test_start_unknown_definition() {
        let mut orch = SagaOrchestrator::new();
        let result = orch.start("NonExistentSaga");
        assert!(result.is_err());
    }

    #[test]
    fn test_multiple_saga_instances_independent() {
        let mut orch = SagaOrchestrator::new();
        orch.register(make_transfer_saga());
        let id1 = orch.start("BankTransfer").unwrap();
        let id2 = orch.start("BankTransfer").unwrap();
        // Two different instances
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_compensation_on_first_step_failure() {
        let mut orch = SagaOrchestrator::new();
        orch.register(make_transfer_saga());
        let saga_id = orch.start("BankTransfer").unwrap();

        // Fail immediately on step 1
        let compensations = orch.step_failed(saga_id, "Immediate failure".into()).unwrap();
        // No prior steps to compensate
        assert!(compensations.is_empty());
    }

    #[test]
    fn test_three_step_compensation_lifo() {
        let mut orch = SagaOrchestrator::new();
        orch.register(make_three_step_saga());
        let saga_id = orch.start("ThreeStep").unwrap();

        // Step 1 succeeds
        let (idx, _) = orch.next_action(saga_id).unwrap();
        assert_eq!(idx, 0);
        orch.step_succeeded(saga_id).unwrap();

        // Step 2 succeeds
        let (idx, _) = orch.next_action(saga_id).unwrap();
        assert_eq!(idx, 1);
        orch.step_succeeded(saga_id).unwrap();

        // Step 3 fails — should compensate 2, then 1 (LIFO)
        let compensations = orch.step_failed(saga_id, "Step 3 failed".into()).unwrap();
        assert_eq!(compensations.len(), 2); // Reverse of step 2 + step 1
        // First compensation should be for step 2
        assert!(matches!(compensations[0], SagaAction::Debit { .. })); // Reverse of Credit
    }

    #[test]
    fn test_saga_timeout_via_step_failure() {
        let mut orch = SagaOrchestrator::new();
        orch.register(make_transfer_saga());
        let saga_id = orch.start("BankTransfer").unwrap();

        // Start step 1
        let (_idx, _) = orch.next_action(saga_id).unwrap();

        // Fail with timeout reason — should trigger compensation flow
        let compensations = orch.step_failed(saga_id, "Timeout expired".into()).unwrap();
        // First step failed with no prior completions → empty compensations
        assert_eq!(compensations.len(), 0);
    }

    #[test]
    fn test_outbox_ordering_fifo() {
        let mut outbox = Outbox::new();
        let agg_id = Uuid::now_v7();

        outbox.enqueue(agg_id, "First", "{}");
        outbox.enqueue(agg_id, "Second", "{}");
        outbox.enqueue(agg_id, "Third", "{}");

        let pending = outbox.pending_messages();
        assert_eq!(pending.len(), 3);
        // FIFO order
        assert_eq!(pending[0].event_type, "First");
        assert_eq!(pending[1].event_type, "Second");
        assert_eq!(pending[2].event_type, "Third");
    }

    #[test]
    fn test_outbox_partial_publish() {
        let mut outbox = Outbox::new();
        let agg_id = Uuid::now_v7();

        outbox.enqueue(agg_id, "A", "{}");
        outbox.enqueue(agg_id, "B", "{}");
        outbox.enqueue(agg_id, "C", "{}");

        // Collect IDs before mutable borrow
        let ids: Vec<Uuid> = outbox.pending_messages().iter().map(|m| m.id).collect();
        outbox.mark_published(ids[0]);
        outbox.mark_published(ids[1]);

        let remaining = outbox.pending_messages();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].event_type, "C");
    }

    #[test]
    fn test_timeout_multiple_sagas() {
        let mut handler = TimeoutHandler::new();
        let s1 = Uuid::now_v7();
        let s2 = Uuid::now_v7();
        let s3 = Uuid::now_v7();

        handler.register(s1, 0, TimeoutAction::Compensate);
        handler.register(s2, 3600, TimeoutAction::Retry); // not expired
        handler.register(s3, 0, TimeoutAction::Alert);

        let expired = handler.check_expired();
        assert_eq!(expired.len(), 2); // s1 and s3 expired, s2 not
    }
}
