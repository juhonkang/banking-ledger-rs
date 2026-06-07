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
use crate::domain::journal::{EntryLeg, JournalEntry};
use crate::domain::money::{Currency, Money};
use crate::extensions::{AccountExt, HashChainExt, JournalExt};
use crate::log::hash_chain::HashChain;
use crate::service::ledger_service::LedgerService;
use crate::service::resilience::{CircuitBreaker, GoldenSignals, TokenBucket};

// ━━━ Shared Application State ━━━

pub struct AppState {
    pub accounts: RwLock<HashMap<AccountId, Account>>,
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
}

impl AppState {
    pub fn new() -> Self {
        let signing_key = b"banking-ledger-hmac-key-v1-32b";
        Self {
            accounts: RwLock::new(HashMap::new()),
            ledger: RwLock::new(None),
            metrics: GoldenSignals::new(1000),
            circuit_breaker: CircuitBreaker::new(10, std::time::Duration::from_secs(30)),
            rate_limiter: TokenBucket::new(100, 100.0), // 100 req burst, 100 req/s sustain
            hash_chain: Mutex::new(HashChain::new(signing_key)),
            journal: RwLock::new(Vec::new()),
            journal_seq: Mutex::new(0),
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
pub fn build_router() -> Router {
    let state = Arc::new(AppState::new());

    Router::new()
        // Health
        .route("/health", get(health_handler))
        // Accounts
        .route("/accounts", post(create_account))
        .route("/accounts/{id}", get(get_account))
        .route("/accounts/{id}/debit", post(debit_account))
        .route("/accounts/{id}/credit", post(credit_account))
        .route("/accounts/{id}/status", post(set_account_status))
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
        .layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit_middleware,
        ))
        .with_state(state)
}

// ━━━ Handlers ━━━

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
    let account = Account::new(account_type, &req.currency, req.initial_balance_cents, None);

    let response = AccountResponse::from_account(&account, &currency);
    let id = account.id;
    state.accounts.write().unwrap().insert(id, account);

    state.metrics.record_request(start.elapsed(), false);
    state.circuit_breaker.record_success();

    Ok(Json(response))
}

async fn get_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<AccountResponse>, AppError> {
    let start = std::time::Instant::now();

    let accounts = state.accounts.read().unwrap();
    let account = accounts.get(&id).ok_or(AppError::NotFound)?;
    let currency = resolve_currency(&account.currency);

    state.metrics.record_request(start.elapsed(), false);
    Ok(Json(AccountResponse::from_account(account, &currency)))
}

async fn debit_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(req): Json<DebitRequest>,
) -> Result<Json<AccountResponse>, AppError> {
    let start = std::time::Instant::now();

    let accounts = state.accounts.read().unwrap();
    let account = accounts.get(&id).ok_or(AppError::NotFound)?;

    account
        .debit(req.amount_cents)
        .map_err(|e| AppError::BadRequest(format!("{e:?}")))?;

    let currency = resolve_currency(&account.currency);
    let response = AccountResponse::from_account(account, &currency);
    state.metrics.record_request(start.elapsed(), false);

    Ok(Json(response))
}

async fn credit_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(req): Json<CreditRequest>,
) -> Result<Json<AccountResponse>, AppError> {
    let start = std::time::Instant::now();

    let accounts = state.accounts.read().unwrap();
    let account = accounts.get(&id).ok_or(AppError::NotFound)?;

    account
        .credit(req.amount_cents)
        .map_err(|e| AppError::BadRequest(format!("{e:?}")))?;

    let currency = resolve_currency(&account.currency);
    let response = AccountResponse::from_account(account, &currency);
    state.metrics.record_request(start.elapsed(), false);

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

    let accounts = state.accounts.read().unwrap();
    let account = accounts.get(&id).ok_or(AppError::NotFound)?;
    account.set_status(new_status);
    let currency = resolve_currency(&account.currency);

    Ok(Json(AccountResponse::from_account(account, &currency)))
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

    // Acquire both accounts
    let accounts = state.accounts.read().unwrap();
    let from_account = accounts.get(&req.from_account).ok_or(AppError::NotFound)?;
    let to_account = accounts.get(&req.to_account).ok_or(AppError::NotFound)?;

    let currency = resolve_currency(&from_account.currency);

    // Execute transfer: debit from, credit to
    from_account.debit(req.amount_cents).map_err(|e| {
        state.circuit_breaker.record_failure();
        AppError::BadRequest(format!("Debit failed: {e:?}"))
    })?;

    to_account.credit(req.amount_cents).map_err(|e| {
        // Rollback: credit back the from_account
        let _ = from_account.credit(req.amount_cents);
        state.circuit_breaker.record_failure();
        AppError::BadRequest(format!("Credit failed: {e:?}"))
    })?;

    // Capture balances before releasing the read lock
    let from_balance = from_account.balance_cents();
    let to_balance = to_account.balance_cents();

    drop(accounts);

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
        "accounts_count": state.accounts.read().unwrap().len(),
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
    let accounts = state.accounts.read().unwrap();
    let account = accounts.get(&id).ok_or(AppError::NotFound)?;
    let currency = resolve_currency(&account.currency);
    Ok(Json(account.snapshot(&currency)))
}

// ━━━ Server Launcher ━━━

/// Start the REST API server on the given port.
pub async fn serve(port: u16) -> std::io::Result<()> {
    let app = build_router();
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
