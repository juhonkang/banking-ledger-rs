//! REST API server for the Banking Ledger.
// Full CRUD for accounts, transfer endpoint, audit trail.
// Production-ready with circuit breaker, rate limiting, golden signals.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use axum::{
    extract::{Json, Path, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::account::{Account, AccountId, AccountStatus, AccountType};
use crate::domain::coa::CoaCategory;
use crate::domain::identifier::{IdentifierType};
use crate::domain::journal::{EntryLeg, JournalEntry};
use crate::domain::money::{Currency, Money};
use crate::domain::party::{Party, PartyStatus, PartyType};
use crate::extensions::{AccountExt, HashChainExt, JournalExt};
use crate::log::hash_chain::HashChain;
use crate::rbac::{extract_subject, Permission, RbacEngine, RbacExt, SubjectId};
use crate::service::identity_service::IdentityService;
use crate::service::ledger_service::LedgerService;
use crate::service::resilience::{CircuitBreaker, GoldenSignals, TokenBucket};
use crate::service::saga::SagaOrchestrator;
use crate::service::account_service::AccountService;

use crate::store::SurrealStore;

// ━━━ Shared Application State ━━━

pub struct AppState {
    /// Thread-safe account registry (DashMap-backed)
    pub account_service: AccountService,
    pub ledger: RwLock<Option<LedgerService>>,
    pub metrics: GoldenSignals,
    pub circuit_breaker: CircuitBreaker,
    /// Token bucket rate limiter
    pub rate_limiter: TokenBucket,
    /// Immutable hash chain for cryptographic audit trail
    pub hash_chain: Mutex<HashChain>,
    /// Journal entries (append-only, keyed by entry ID)
    pub journal: RwLock<Vec<JournalEntry>>,
    /// Monotonically increasing sequence counter for journal entries
    pub journal_seq: Mutex<u64>,
    /// RBAC engine — role-based access control
    pub rbac: RwLock<RbacEngine>,
    /// SurrealDB persistence (None if in-memory mode)
    pub store: Option<Arc<SurrealStore>>,
    /// Identity management (Party + Identifier lifecycle)
    pub identity_service: RwLock<IdentityService>,
    /// Saga orchestrator for long-lived transactions
    pub saga_service: RwLock<SagaOrchestrator>,
}

impl AppState {
    pub fn new(store: Option<Arc<SurrealStore>>) -> Self {
        let signing_key = b"banking-ledger-hmac-key-v1-32b";
        let mut rbac = RbacEngine::new();

        // Bootstrap: pre-seed default admin subject from env or well-known UUID.
        // In production, this comes from config. For now, UUID namespace for "admin".
        let default_admin = SubjectId(Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap());
        rbac.bind(default_admin, crate::rbac::Role::Admin);

        // Default auditor (read-only audit access)
        let default_auditor = SubjectId(Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap());
        rbac.bind(default_auditor, crate::rbac::Role::Auditor);

        Self {
            account_service: AccountService::new(),
            ledger: RwLock::new(None),
            metrics: GoldenSignals::new(1000),
            circuit_breaker: CircuitBreaker::new(10, std::time::Duration::from_secs(30)),
            rate_limiter: TokenBucket::new(100, 100.0), // 100 req burst, 100 req/s sustain
            hash_chain: Mutex::new(HashChain::new(signing_key)),
            journal: RwLock::new(Vec::new()),
            journal_seq: Mutex::new(0),
            rbac: RwLock::new(rbac),
            store,
            identity_service: RwLock::new(IdentityService::new()),
            saga_service: RwLock::new(SagaOrchestrator::new()),
        }
    }

    /// Restore state from SurrealDB on startup.
    /// Called after store connection is established.
    pub async fn restore_from_store(&self) {
        let Some(ref store) = self.store else { return };

        // Load accounts (restored directly into accounts HashMap)
        match store.load_all_accounts().await {
            Ok(accounts) => {
                let count = accounts.len();
                let svc = &self.account_service;
                for acc in accounts {
                    svc.insert_raw(acc.id, acc);
                }
                if count > 0 {
                    eprintln!("  Restored {count} accounts from SurrealDB");
                }
            }
            Err(e) => eprintln!("  ⚠ Failed to load accounts: {e}"),
        }

        // Load hash chain
        match store.load_hash_chain(b"banking-ledger-hmac-key-v1-32b").await {
            Ok(chain) => {
                let block_count = chain.blocks.len();
                if block_count > 1 {
                    // More than just genesis
                    let mut guard = self.hash_chain.lock().unwrap();
                    *guard = chain;
                    eprintln!("  Restored hash chain: {block_count} blocks");
                }
            }
            Err(e) => eprintln!("  ⚠ Failed to load hash chain: {e}"),
        }
    }
}

// ━━━ Currency Helpers ━━━

/// Resolve a currency code string to a Currency object
fn resolve_currency(code: &str) -> Currency {
    match code.to_uppercase().as_str() {
        "USD" => Currency::usd(),
        "EUR" => Currency::eur(),
        "VND" => Currency::vnd(),
        "JPY" => Currency::jpy(),
        _ => Currency {
            code: code.to_uppercase(),
            name: code.to_string(),
            minor_unit: 2,
            symbol: code.to_string(),
            numeric_code: 0,
        },
    }
}

/// Format an amount in minor units (cents) as a display string
fn format_money(amount_cents: i64, currency: &Currency) -> String {
    let money = Money::from_minor(amount_cents, currency.clone());
    let minor = currency.minor_unit as usize;
    format!("{:.minor$} {}", money.amount, currency.code, minor = minor)
}

/// Format decimal value from minor units
fn decimal_from_minor(amount_cents: i64, currency: &Currency) -> String {
    let money = Money::from_minor(amount_cents, currency.clone());
    let minor = currency.minor_unit as usize;
    format!("{:.minor$}", money.amount, minor = minor)
}

// ━━━ Request/Response Types ━━━

#[derive(Debug, Deserialize)]
pub struct CreateAccountRequest {
    pub account_type: String,
    pub currency: String,
    pub initial_balance_cents: i64,
}

#[derive(Debug, Serialize)]
pub struct AccountResponse {
    pub id: Uuid,
    pub account_type: String,
    pub currency: String,
    /// Balance in minor units (cents) — backward compat
    pub balance_cents: i64,
    pub available_balance_cents: i64,
    pub status: String,
    /// Human-readable balance (e.g., "$1,000.00 USD")
    pub balance_formatted: String,
    /// Decimal amount (e.g., "1000.00")
    pub balance_decimal: String,
    /// Currency symbol (e.g., "$")
    pub currency_symbol: String,
    /// Number of decimal places for this currency
    pub currency_decimals: u8,
}

impl AccountResponse {
    fn from_account(a: &Account, currency: &Currency) -> Self {
        let balance = a.balance_cents();
        Self {
            id: a.id,
            account_type: format!("{:?}", a.account_type),
            currency: currency.code.clone(),
            balance_cents: balance,
            available_balance_cents: a.available_balance_cents(),
            status: format!("{:?}", a.status()),
            balance_formatted: format_money(balance, currency),
            balance_decimal: decimal_from_minor(balance, currency),
            currency_symbol: currency.symbol.clone(),
            currency_decimals: currency.minor_unit,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct TransferRequest {
    pub from_account: Uuid,
    pub to_account: Uuid,
    pub amount_cents: i64,
    pub description: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TransferResponse {
    pub transaction_id: Uuid,
    pub journal_entry_id: Uuid,
    pub from_balance: i64,
    pub to_balance: i64,
    /// Hash chain block index for this journal entry
    pub chain_index: u64,
    /// SHA-256 hash of the block securing this entry
    pub chain_hash: String,
    /// Human-readable transfer amount (e.g., "$200.00 USD")
    pub amount_formatted: String,
    /// Decimal transfer amount (e.g., "200.00")
    pub amount_decimal: String,
}

#[derive(Debug, Deserialize)]
pub struct DebitRequest {
    pub amount_cents: i64,
}

#[derive(Debug, Deserialize)]
pub struct CreditRequest {
    pub amount_cents: i64,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub uptime_seconds: u64,
    pub circuit_state: String,
    pub error_rate: f64,
}

// ━━━ Routes ━━━

/// Rate-limiting middleware. Returns 429 if the token bucket is empty.
async fn rate_limit_middleware(
    State(state): State<Arc<AppState>>,
    request: axum::extract::Request,
    next: Next,
) -> Result<axum::response::Response, StatusCode> {
    if state.rate_limiter.try_consume() {
        let mut response = next.run(request).await;
        response.headers_mut().insert(
            "X-RateLimit-Remaining",
            "100".parse().unwrap(),
        );
        Ok(response)
    } else {
        Err(StatusCode::TOO_MANY_REQUESTS)
    }
}

/// Build the full router
pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        // Health
        .route("/health", get(health_handler))
        // Accounts
        .route("/accounts", post(create_account))
        .route("/accounts/{id}", get(get_account))
        .route("/accounts/{id}/debit", post(debit_account))
        .route("/accounts/{id}/credit", post(credit_account))
        .route("/accounts/{id}/status", post(set_account_status))
        .route("/accounts/{id}/hold", post(place_hold))
        .route("/accounts/{id}/release", post(release_hold))
        // Transfers
        .route("/transfers", post(transfer))
        // Admin
        .route("/admin/metrics", get(metrics_handler))
        // Journal + Audit
        .route("/journal", get(list_journal))
        .route("/journal/verify", get(verify_chain))
        .route("/journal/proof/{index}", get(chain_proof))
        .route("/journal/trial-balance", get(trial_balance))
        .route("/journal/account/{id}", get(entries_for_account))
        .route("/journal/validate", post(validate_entries))
        // Audit trail
        .route("/audit/report", get(audit_report))
        .route("/audit/export", get(export_audit_log))
        .route("/audit/redact/{index}", post(redact_block))
        // Account extensions
        .route("/accounts/{id}/snapshot", get(account_snapshot))
        // Party management
        .route("/parties", post(create_party))
        .route("/parties", get(list_parties))
        .route("/parties/{id}", get(get_party))
        .route("/parties/{id}/identifiers", post(add_identifier))
        .route("/parties/{id}/identifiers", get(list_identifiers))
        .route("/identifiers/{id}/verify", post(verify_identifier))
        // Saga management
        .route("/sagas/{id}", get(get_saga_status))
        // Chart of Accounts
        .route("/coa", get(coa_summary))
        // RBAC management
        .route("/rbac/permissions", get(list_rbac_permissions))
        .route("/rbac/bind", post(bind_role))
        .route("/rbac/subject/{id}", get(get_subject_roles))
        .route("/rbac/audit", get(rbac_audit_export))
        .route("/rbac/matrix", get(rbac_matrix))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit_middleware,
        ))
        .with_state(state)
}

// ━━━ Handlers ━━━

/// Fire-and-forget persistence after mutation.
/// Spawns a background task so the API response is never blocked by DB I/O.
fn persist_after_mutation(state: &Arc<AppState>) {
    if let Some(ref store) = state.store {
        let store = store.clone();
        // Extract all data upfront (drop all guards before tokio::spawn)
        let account_data: Vec<(uuid::Uuid, String, String, i64, i64, String)> = {
            let svc = &state.account_service;
            let mut items = Vec::new();
            svc.for_each(|id, acc| {
                items.push((*id, format!("{:?}", acc.account_type), acc.currency.clone(),
                    acc.balance_cents(), acc.available_balance_cents(), format!("{:?}", acc.status())));
            });
            items
        };
        let journal_clone: Vec<JournalEntry> = state.journal.read().unwrap().clone();
        let chain_blocks = {
            let guard = state.hash_chain.lock().unwrap();
            guard.blocks.clone()
        };
        // All guards dropped here — no references escape to tokio::spawn

        tokio::spawn(async move {
            for (id, atype, currency, balance, hold, status) in &account_data {
                let _ = store.save_account_raw(&id.to_string(), atype, currency, *balance, *hold, status).await;
            }
            for entry in journal_clone.iter().rev().take(10) {
                let _ = store.save_journal_entry(entry).await;
            }
            let mut chain = crate::log::hash_chain::HashChain::new(b"banking-ledger-hmac-key-v1-32b");
            chain.blocks = chain_blocks;
            let _ = store.save_hash_chain(&chain).await;
        });
    }
}

async fn health_handler(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy".into(),
        uptime_seconds: 0,
        circuit_state: format!("{:?}", state.circuit_breaker.state()),
        error_rate: state.metrics.error_rate(),
    })
}

async fn create_account(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateAccountRequest>,
) -> Result<Json<AccountResponse>, AppError> {
    if !state.circuit_breaker.allow_request() {
        return Err(AppError::ServiceUnavailable);
    }

    let start = std::time::Instant::now();
    let account_type = match req.account_type.to_uppercase().as_str() {
        "ASSET" => AccountType::Asset,
        "LIABILITY" => AccountType::Liability,
        "EQUITY" => AccountType::Equity,
        "REVENUE" => AccountType::Revenue,
        "EXPENSE" => AccountType::Expense,
        _ => return Err(AppError::BadRequest("Invalid account type".into())),
    };

    let currency = resolve_currency(&req.currency);
    let account = state.account_service.create_account(
        account_type,
        &req.currency,
        req.initial_balance_cents,
        None,
    );

    let response = AccountResponse::from_account(&account, &currency);

    state.metrics.record_request(start.elapsed(), false);
    state.circuit_breaker.record_success();

    persist_after_mutation(&state);
    Ok(Json(response))
}

async fn get_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<AccountResponse>, AppError> {
    let start = std::time::Instant::now();
    let account = state.account_service.get_account(id).ok_or(AppError::NotFound)?;
    let currency = resolve_currency(&account.currency);

    state.metrics.record_request(start.elapsed(), false);
    Ok(Json(AccountResponse::from_account(&account, &currency)))
}

async fn debit_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(req): Json<DebitRequest>,
) -> Result<Json<AccountResponse>, AppError> {
    let start = std::time::Instant::now();

    state.account_service
        .perform_debit(id, req.amount_cents)
        .map_err(|e| AppError::BadRequest(format!("{e:?}")))?;

    let account = state.account_service.get_account(id).ok_or(AppError::NotFound)?;
    let currency = resolve_currency(&account.currency);
    let response = AccountResponse::from_account(&account, &currency);
    state.metrics.record_request(start.elapsed(), false);

    persist_after_mutation(&state);
    Ok(Json(response))
}

async fn credit_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(req): Json<CreditRequest>,
) -> Result<Json<AccountResponse>, AppError> {
    let start = std::time::Instant::now();

    state.account_service
        .perform_credit(id, req.amount_cents)
        .map_err(|e| AppError::BadRequest(format!("{e:?}")))?;

    let account = state.account_service.get_account(id).ok_or(AppError::NotFound)?;
    let currency = resolve_currency(&account.currency);
    let response = AccountResponse::from_account(&account, &currency);
    state.metrics.record_request(start.elapsed(), false);
    state.circuit_breaker.record_success();

    persist_after_mutation(&state);
    Ok(Json(response))
}

async fn set_account_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<AccountResponse>, AppError> {
    let status_str = req["status"].as_str().unwrap_or("OPEN");
    let new_status = match status_str.to_uppercase().as_str() {
        "OPEN" => AccountStatus::Open,
        "FROZEN" => AccountStatus::Frozen,
        "CLOSED" => AccountStatus::Closed,
        _ => return Err(AppError::BadRequest("Invalid status".into())),
    };
    if !state.account_service.set_status(id, new_status) {
        return Err(AppError::NotFound);
    }
    let account = state.account_service.get_account(id).ok_or(AppError::NotFound)?;
    let currency = resolve_currency(&account.currency);

    persist_after_mutation(&state);
    Ok(Json(AccountResponse::from_account(&account, &currency)))
}

// ━━━ Hold/Release ━━━

#[derive(Deserialize)]
struct HoldRequest {
    amount_cents: i64,
}

async fn place_hold(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(req): Json<HoldRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    state.account_service
        .place_hold(id, req.amount_cents)
        .map_err(|e| AppError::BadRequest(format!("{e:?}")))?;

    let account = state.account_service.get_account(id).ok_or(AppError::NotFound)?;
    Ok(Json(serde_json::json!({
        "id": id,
        "balance_cents": account.balance_cents(),
        "available_balance_cents": account.available_balance_cents(),
        "hold_amount": req.amount_cents,
    })))
}

async fn release_hold(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(req): Json<HoldRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    state.account_service
        .release_hold(id, req.amount_cents)
        .map_err(|e| AppError::BadRequest(format!("{e:?}")))?;

    let account = state.account_service.get_account(id).ok_or(AppError::NotFound)?;
    Ok(Json(serde_json::json!({
        "id": id,
        "balance_cents": account.balance_cents(),
        "available_balance_cents": account.available_balance_cents(),
        "released_amount": req.amount_cents,
    })))
}

async fn transfer(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TransferRequest>,
) -> Result<Json<TransferResponse>, AppError> {
    if !state.circuit_breaker.allow_request() {
        return Err(AppError::ServiceUnavailable);
    }

    let start = std::time::Instant::now();
    let description = req.description.unwrap_or_else(|| "Transfer".into());
    let transaction_id = Uuid::now_v7();

    // Read currency from source account
    let from_snapshot = state.account_service.get_account(req.from_account).ok_or(AppError::NotFound)?;
    let currency = resolve_currency(&from_snapshot.currency);

    // Execute transfer: debit from account via service
    state.account_service
        .perform_debit(req.from_account, req.amount_cents)
        .map_err(|e| {
            state.circuit_breaker.record_failure();
            AppError::BadRequest(format!("Debit failed: {e:?}"))
        })?;

    // Credit to account via service
    state.account_service
        .perform_credit(req.to_account, req.amount_cents)
        .map_err(|e| {
            // Rollback: credit back the from_account
            let _ = state.account_service.perform_credit(req.from_account, req.amount_cents);
            state.circuit_breaker.record_failure();
            AppError::BadRequest(format!("Credit failed: {e:?}"))
        })?;

    // Capture post-transfer balances
    let from_account = state.account_service.get_account(req.from_account).ok_or(AppError::NotFound)?;
    let to_account = state.account_service.get_account(req.to_account).ok_or(AppError::NotFound)?;
    let from_balance = from_account.balance_cents();
    let to_balance = to_account.balance_cents();
    drop(from_account);
    drop(to_account);

    // ━━━ Create Journal Entry ━━━
    let mut seq = state.journal_seq.lock().unwrap();
    *seq += 1;
    let sequence_number = *seq;
    drop(seq);

    let legs = vec![
        EntryLeg::debit(req.to_account, req.amount_cents),
        EntryLeg::credit(req.from_account, req.amount_cents),
    ];

    let journal_entry = JournalEntry::new(transaction_id, sequence_number, legs, &description)
        .map_err(|e| AppError::BadRequest(format!("Journal error: {e}")))?;

    let journal_entry_id = journal_entry.id;

    // ━━━ Append to Hash Chain ━━━
    let chain_data = serde_json::to_string(&journal_entry)
        .map_err(|e| AppError::BadRequest(format!("Serialize error: {e}")))?;

    let chain_block = {
        let mut chain = state.hash_chain.lock().unwrap();
        chain.append(&chain_data).clone()
    };

    // ━━━ Store Journal Entry ━━━
    state.journal.write().unwrap().push(journal_entry);

    let amount_formatted = format_money(req.amount_cents, &currency);
    let amount_decimal = decimal_from_minor(req.amount_cents, &currency);

    let response = TransferResponse {
        transaction_id,
        journal_entry_id,
        from_balance,
        to_balance,
        chain_index: chain_block.index,
        chain_hash: chain_block.hash.clone(),
        amount_formatted,
        amount_decimal,
    };

    state.metrics.record_request(start.elapsed(), false);
    state.circuit_breaker.record_success();

    persist_after_mutation(&state);
    Ok(Json(response))
}

// ━━━ Journal + Audit Endpoints ━━━

/// List all journal entries (paginated)
async fn list_journal(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, AppError> {
    let journal = state.journal.read().unwrap();
    let entries: Vec<serde_json::Value> = journal
        .iter()
        .map(|e| {
            serde_json::json!({
                "id": e.id,
                "transaction_id": e.transaction_id,
                "sequence_number": e.sequence_number,
                "legs": e.legs.iter().map(|l| serde_json::json!({
                    "account_id": l.account_id,
                    "side": format!("{:?}", l.side),
                    "amount_cents": l.amount_cents,
                })).collect::<Vec<_>>(),
                "description": e.description,
                "recorded_at": e.recorded_at.to_rfc3339(),
                "reverses": e.reverses,
            })
        })
        .collect();

    Ok(Json(entries))
}

/// Verify the hash chain integrity
async fn verify_chain(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let chain = state.hash_chain.lock().unwrap();
    let (valid, tampered_indices) = chain.verify_chain();

    Json(serde_json::json!({
        "valid": valid,
        "chain_length": chain.len(),
        "tampered_indices": tampered_indices,
    }))
}

/// Get a chain proof for a specific block index
async fn chain_proof(
    State(state): State<Arc<AppState>>,
    Path(index): Path<u64>,
) -> Result<Json<serde_json::Value>, AppError> {
    let chain = state.hash_chain.lock().unwrap();
    let proof = chain.proof_for_block(index).ok_or(AppError::NotFound)?;

    Ok(Json(serde_json::json!({
        "index": proof.block.index,
        "hash": proof.block.hash,
        "previous_hash": proof.block.previous_hash,
        "data": proof.block.data,
        "timestamp": proof.block.timestamp.to_rfc3339(),
        "previous_block_hash": proof.previous_block_hash,
        "next_block_hash": proof.next_block_hash,
    })))
}

async fn metrics_handler(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let p50 = state.metrics.latency_percentile(50.0);
    let p99 = state.metrics.latency_percentile(99.0);

    Json(serde_json::json!({
        "total_requests": state.metrics.total_requests(),
        "error_rate": state.metrics.error_rate(),
        "latency_p50_ms": p50.map(|d| d.as_millis() as u64),
        "latency_p99_ms": p99.map(|d| d.as_millis() as u64),
        "circuit_state": format!("{:?}", state.circuit_breaker.state()),
        "accounts_count": state.account_service.count(),
        "journal_entries": state.journal.read().unwrap().len(),
        "chain_length": state.hash_chain.lock().unwrap().len(),
    }))
}

// ━━━ Error Handling ━━━

#[derive(Debug)]
pub enum AppError {
    NotFound,
    BadRequest(String),
    ServiceUnavailable,
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            Self::NotFound => (StatusCode::NOT_FOUND, "Not found".to_string()),
            Self::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            Self::ServiceUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "Circuit breaker open".to_string(),
            ),
        };
        (status, Json(serde_json::json!({"error": message}))).into_response()
    }
}

// ━━━ Extension Handlers — Auditing, Redaction, Trial Balance ━━━

/// Trial balance: returns (debits, credits) per account
async fn trial_balance(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let journal = state.journal.read().unwrap();
    let balances = JournalEntry::trial_balance(&journal);
    
    let result: serde_json::Map<String, serde_json::Value> = balances
        .iter()
        .map(|(id, (debits, credits))| {
            (id.to_string(), serde_json::json!({
                "debits": debits,
                "credits": credits,
                "net": debits - credits,
            }))
        })
        .collect();

    Json(serde_json::Value::Object(result))
}

/// Get all journal entries for a specific account
async fn entries_for_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<serde_json::Value>>, AppError> {
    let journal = state.journal.read().unwrap();
    let entries = JournalEntry::for_account(&journal, id);
    
    let result: Vec<serde_json::Value> = entries
        .iter()
        .map(|e| serde_json::json!({
            "id": e.id,
            "transaction_id": e.transaction_id,
            "sequence_number": e.sequence_number,
            "description": e.description,
            "recorded_at": e.recorded_at.to_rfc3339(),
            "legs": e.legs.iter().map(|l| serde_json::json!({
                "account_id": l.account_id,
                "side": format!("{:?}", l.side),
                "amount_cents": l.amount_cents,
            })).collect::<Vec<_>>(),
        }))
        .collect();

    Ok(Json(result))
}

/// Validate all journal entries are balanced
async fn validate_entries(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let journal = state.journal.read().unwrap();
    let (valid, invalid_ids) = JournalEntry::validate_all(&journal);

    Json(serde_json::json!({
        "valid": valid,
        "total_entries": journal.len(),
        "invalid_entry_ids": invalid_ids,
    }))
}

/// Generate audit report for a time range
async fn audit_report(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<String, AppError> {
    let from = params.get("from")
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(|| chrono::Utc::now() - chrono::Duration::hours(24));
    
    let to = params.get("to")
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now);

    let chain = state.hash_chain.lock().unwrap();
    let report = chain.audit_report(from, to);
    Ok(report)
}

/// Export full audit log as JSON
async fn export_audit_log(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let chain = state.hash_chain.lock().unwrap();
    let log = chain.export_audit_log();
    Json(serde_json::json!({
        "chain_length": chain.len(),
        "blocks": log,
    }))
}

/// Redact a block at the given index (GDPR/privacy compliance)
async fn redact_block(
    State(state): State<Arc<AppState>>,
    Path(index): Path<u64>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut chain = state.hash_chain.lock().unwrap();
    let new_head = chain.redact_block(index)
        .map_err(|e| AppError::BadRequest(e))?;

    persist_after_mutation(&state);
    Ok(Json(serde_json::json!({
        "redacted_index": index,
        "new_chain_head": new_head,
        "chain_length": chain.len(),
    })))
}

/// Account snapshot with extended info
async fn account_snapshot(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    let account = state.account_service.get_account(id).ok_or(AppError::NotFound)?;
    let currency = resolve_currency(&account.currency);
    Ok(Json(account.snapshot(&currency)))
}

// ━━━ RBAC Handlers ━━━

/// Middleware: require a specific permission to access an endpoint.
async fn require_permission(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    permission: Permission,
) -> Result<SubjectId, (StatusCode, Json<serde_json::Value>)> {
    let subject = extract_subject(&headers).ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "Missing or invalid X-Subject-Id header"})),
        )
    })?;

    let rbac = state.rbac.read().unwrap();
    if rbac.can(&subject, permission) {
        Ok(subject)
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "Permission denied",
                "required": format!("{:?}", permission),
                "subject": subject.0.to_string(),
                "roles": rbac.roles_for(&subject).iter().map(|r| format!("{:?}", r)).collect::<Vec<_>>(),
            })),
        ))
    }
}

/// List all available permissions with their descriptions.
async fn list_rbac_permissions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let subject = require_permission(State(state.clone()), headers, Permission::ManageRoles).await?;

    let perms = vec![
        ("read_any_account", "View any account details and balance"),
        ("read_own_account", "View own account details only"),
        ("create_account", "Create new accounts"),
        ("update_account_status", "Freeze/unfreeze/close accounts"),
        ("close_account", "Permanently close an account"),
        ("initiate_transfer", "Create transfers between any accounts"),
        ("initiate_own_transfer", "Transfer from own accounts only"),
        ("view_any_transaction", "View all transactions"),
        ("view_own_transaction", "View own transactions only"),
        ("view_audit_log", "Access audit trail and hash chain"),
        ("verify_chain_integrity", "Run hash chain verification"),
        ("export_audit_report", "Download audit report"),
        ("view_trial_balance", "View trial balance"),
        ("manage_users", "Create/modify/delete subjects"),
        ("manage_roles", "Bind/unbind roles to subjects"),
        ("configure_limits", "Change rate limits, thresholds"),
        ("view_system_metrics", "Access /admin/metrics"),
        ("redact_pii", "Redact personally identifiable information"),
        ("export_user_data", "GDPR data export"),
        ("generate_sar_report", "Suspicious Activity Report"),
    ];

    Ok(Json(serde_json::json!({
        "requested_by": subject.0.to_string(),
        "permissions": perms.iter().map(|(code, desc)| {
            serde_json::json!({"code": code, "description": desc})
        }).collect::<Vec<_>>(),
    })))
}

/// Bind a role to a subject.
#[derive(Deserialize)]
struct BindRoleRequest {
    subject_id: Uuid,
    role: String,
}

async fn bind_role(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<BindRoleRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let _subject = require_permission(State(state.clone()), headers, Permission::ManageRoles).await?;

    let role = match req.role.to_lowercase().as_str() {
        "admin" => crate::rbac::Role::Admin,
        "auditor" => crate::rbac::Role::Auditor,
        "teller" => crate::rbac::Role::Teller,
        "customer" => crate::rbac::Role::Customer,
        "system" => crate::rbac::Role::System,
        "compliance" => crate::rbac::Role::Compliance,
        _ => return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": format!("Unknown role: {}", req.role)})),
        )),
    };

    let mut rbac = state.rbac.write().unwrap();
    let subject = SubjectId(req.subject_id);
    rbac.bind(subject.clone(), role);

    Ok(Json(serde_json::json!({
        "bound": true,
        "subject": subject.0.to_string(),
        "role": req.role,
        "effective_permissions": rbac.permissions_for(&subject)
            .iter()
            .map(|p| format!("{:?}", p))
            .collect::<Vec<_>>(),
    })))
}

/// Get roles for a specific subject.
async fn get_subject_roles(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let _subject = require_permission(State(state.clone()), headers, Permission::ManageRoles).await?;

    let rbac = state.rbac.read().unwrap();
    let subject = SubjectId(id);
    let roles = rbac.roles_for(&subject);
    let perms = rbac.permissions_for(&subject);

    Ok(Json(serde_json::json!({
        "subject": id.to_string(),
        "roles": roles.iter().map(|r| format!("{:?}", r)).collect::<Vec<_>>(),
        "effective_permissions": perms.iter().map(|p| format!("{:?}", p)).collect::<Vec<_>>(),
        "permission_count": perms.len(),
    })))
}

/// Export full RBAC state for audit.
async fn rbac_audit_export(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let _subject = require_permission(State(state.clone()), headers, Permission::ViewAuditLog).await?;

    let rbac = state.rbac.read().unwrap();
    Ok(Json(rbac.export_audit()))
}

/// Display the permission matrix.
async fn rbac_matrix(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<String, (StatusCode, Json<serde_json::Value>)> {
    let _subject = require_permission(State(state.clone()), headers, Permission::ViewAuditLog).await?;

    let rbac = state.rbac.read().unwrap();
    Ok(rbac.permission_matrix())
}

// ━━━ Party Handlers ━━━

#[derive(Deserialize)]
struct CreatePartyRequest {
    party_type: String,
    legal_name: String,
}

async fn create_party(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreatePartyRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let ptype = match req.party_type.to_lowercase().as_str() {
        "individual" => PartyType::Individual,
        "corporation" => PartyType::Corporation,
        "trust" => PartyType::Trust,
        "government" => PartyType::GovernmentAgency,
        "financial" => PartyType::FinancialInstitution,
        other => PartyType::Other(other.to_string()),
    };
    let mut svc = state.identity_service.write().unwrap();
    let party = svc.create_party(ptype, &req.legal_name);

    Ok(Json(serde_json::json!({
        "id": party.id,
        "status": "Active",
    })))
}

async fn list_parties(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let svc = state.identity_service.read().unwrap();
    let parties: Vec<_> = svc.all_parties().iter().map(|p| serde_json::json!({
        "id": p.id,
        "legal_name": p.legal_name,
        "party_type": format!("{:?}", p.party_type),
        "status": format!("{:?}", p.status),
        "created_at": p.created_at.to_rfc3339(),
    })).collect();
    Json(serde_json::json!({ "parties": parties, "count": parties.len() }))
}

async fn get_party(
    State(state): State<Arc<AppState>>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    let svc = state.identity_service.read().unwrap();
    let party = svc.get_party(id).ok_or(AppError::NotFound)?;
    Ok(Json(serde_json::json!({
        "id": party.id,
        "legal_name": party.legal_name,
        "party_type": format!("{:?}", party.party_type),
        "status": format!("{:?}", party.status),
        "created_at": party.created_at.to_rfc3339(),
    })))
}

// ━━━ Identifier Handlers ━━━

#[derive(Deserialize)]
struct AddIdentifierRequest {
    identifier_type: String,
    value: String,
    issuing_country: Option<String>,
}

async fn add_identifier(
    State(state): State<Arc<AppState>>,
    Path(party_id): Path<uuid::Uuid>,
    Json(req): Json<AddIdentifierRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let id_type = match req.identifier_type.to_lowercase().as_str() {
        "national_id" => IdentifierType::NationalId,
        "passport" => IdentifierType::PassportNumber,
        "drivers_license" => IdentifierType::DriversLicense,
        "tax_id" => IdentifierType::TaxIdentificationNumber,
        "business_reg" => IdentifierType::BusinessRegistrationNumber,
        "email" => IdentifierType::Email,
        "phone" => IdentifierType::Phone,
        other => IdentifierType::Other(other.to_string()),
    };

    let svc = state.identity_service.write().unwrap();
    let identifier = svc
        .add_identifier(party_id, id_type, &req.value, req.issuing_country.as_deref())
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    Ok(Json(serde_json::json!({
        "id": identifier.id,
        "party_id": identifier.party_id,
        "identifier_type": format!("{:?}", identifier.identifier_type),
        "status": format!("{:?}", identifier.status),
    })))
}

async fn list_identifiers(
    State(state): State<Arc<AppState>>,
    Path(party_id): Path<uuid::Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    let svc = state.identity_service.read().unwrap();
    let identifiers = svc.get_active_identifiers(party_id);
    let result: Vec<_> = identifiers.iter().map(|i| serde_json::json!({
        "id": i.id,
        "party_id": i.party_id,
        "identifier_type": format!("{:?}", i.identifier_type),
        "value": i.value,
        "status": format!("{:?}", i.status),
        "issuing_country": i.issuing_country,
        "effective_from": i.effective_from.to_rfc3339(),
        "effective_to": i.effective_to.map(|t| t.to_rfc3339()),
    })).collect();
    Ok(Json(serde_json::json!({ "identifiers": result, "count": result.len() })))
}

// ━━━ Identifier Verification ━━━

async fn verify_identifier(
    State(state): State<Arc<AppState>>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    let svc = state.identity_service.write().unwrap();

    // Find the identifier across all parties
    let mut found = None;
    svc.for_each_party(|party| {
        if found.is_none() {
            for ident in svc.get_active_identifiers(party.id) {
                if ident.id == id {
                    found = Some(ident.clone());
                    break;
                }
            }
        }
    });

    match found {
        Some(mut ident) => {
            ident.verify();
            Ok(Json(serde_json::json!({
                "id": ident.id,
                "status": "Active",
                "verified_at": ident.verified_at.map(|t| t.to_rfc3339()),
            })))
        }
        None => Err(AppError::NotFound),
    }
}

// ━━━ Saga Handlers ━━━

async fn get_saga_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    let svc = state.saga_service.read().unwrap();
    let status = svc.get_status(id);
    match status {
        Some(s) => Ok(Json(serde_json::json!({
            "saga_id": id.to_string(),
            "status": format!("{:?}", s),
        }))),
        None => Err(AppError::NotFound),
    }
}

// ━━━ COA Handler ━━━

async fn coa_summary(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    use crate::domain::coa::{CoaAccount, ChartOfAccounts, CoaCategory};
    use crate::domain::account::AccountType;

    let mut coa = ChartOfAccounts::new(1);
    state.account_service.for_each(|id, acc| {
        let (cat, code) = match acc.account_type {
            AccountType::Asset => (CoaCategory::Asset, "1000"),
            AccountType::Liability => (CoaCategory::Liability, "2000"),
            AccountType::Equity => (CoaCategory::Equity, "3000"),
            AccountType::Revenue => (CoaCategory::Revenue, "4000"),
            AccountType::Expense => (CoaCategory::Expense, "5000"),
        };
        let coa_acct = CoaAccount::new(
            &format!("{}_{}", code, id),
            &format!("{:?}", acc.account_type),
            cat,
            None,
            1,
        );
        coa.add_account(coa_acct);
    });

    let accounts: Vec<_> = coa.active_accounts().iter().map(|a| serde_json::json!({
        "code": a.code,
        "name": a.name,
        "category": format!("{:?}", a.category),
        "normal_balance": format!("{:?}", a.normal_balance),
    })).collect();

    Json(serde_json::json!({
        "chart_of_accounts": accounts,
        "total_count": accounts.len(),
    }))
}

// ━━━ Server Launcher ━━━

/// Start the REST API server on the given port.
pub async fn serve(port: u16, store: Option<Arc<SurrealStore>>) -> std::io::Result<()> {
    let state = Arc::new(AppState::new(store));
    state.restore_from_store().await;
    let app = build_router(state.clone());
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    println!("\u{1f3e6} Banking Ledger API listening on http://{addr}");
    println!("   POST /accounts           — Create account");
    println!("   GET  /accounts/:id       — Get account");
    println!("   POST /accounts/:id/debit  — Debit");
    println!("   POST /accounts/:id/credit — Credit");
    println!("   POST /transfers          — Transfer (double-entry + hash chain)");
    println!("   GET  /health             — Health check");
    println!("   GET  /admin/metrics      — Golden signals");
    println!("   GET  /journal            — List journal entries");
    println!("   GET  /journal/verify     — Verify hash chain integrity");
    println!("   GET  /journal/proof/:idx — Chain proof for block");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await
}
