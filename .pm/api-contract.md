# API Contract

> OpenAPI 3.0 specification for the Banking Ledger REST API.

## Base URL

```
http://localhost:3001
```

## Authentication

None in current version. Production should add JWT Bearer token.

## Rate Limiting

Token bucket: 1000 requests/second globally. Returns 429 when exhausted.

## Circuit Breaker

After 10 consecutive failures within 30s, circuit opens → returns 503.
Probes every 30s with 2 successful probes to close.

---

## Endpoints

### GET /health

Health check — always available (bypasses circuit breaker).

**Response 200:**
```json
{
  "status": "healthy",
  "uptime_seconds": 3600,
  "circuit_state": "Closed",
  "error_rate": 0.001
}
```

---

### POST /accounts

Create a new ledger account.

**Request:**
```json
{
  "account_type": "ASSET",
  "currency": "USD",
  "initial_balance_cents": 100000
}
```

| Field | Type | Constraints |
|-------|------|-------------|
| account_type | string | ASSET, LIABILITY, EQUITY, REVENUE, EXPENSE |
| currency | string | ISO 4217 code |
| initial_balance_cents | i64 | 0 ≤ x ≤ 10^12 |

**Response 200:**
```json
{
  "id": "019e9a81-8a64-7c93-9700-262c63e45025",
  "account_type": "Asset",
  "currency": "USD",
  "balance_cents": 100000,
  "available_balance_cents": 100000,
  "status": "Open"
}
```

**Errors:**
- `400` — Invalid account_type or currency
- `503` — Circuit breaker open

---

### GET /accounts/:id

Retrieve account details.

**Response 200:**
```json
{
  "id": "019e9a81-8a64-7c93-9700-262c63e45025",
  "account_type": "Asset",
  "currency": "USD",
  "balance_cents": 100000,
  "available_balance_cents": 95000,
  "status": "Open"
}
```

**Errors:**
- `404` — Account not found

---

### POST /accounts/:id/debit

Atomic debit (withdraw) using CAS loop.

**Request:**
```json
{
  "amount_cents": 5000
}
```

**Response 200:**
```json
{
  "id": "019e9a81...",
  "account_type": "Asset",
  "currency": "USD",
  "balance_cents": 95000,
  "available_balance_cents": 95000,
  "status": "Open"
}
```

**Errors:**
- `400` — Invalid amount (≤ 0)
- `400` — Insufficient available funds
- `400` — Account frozen or closed
- `404` — Account not found

---

### POST /accounts/:id/credit

Atomic credit (deposit).

**Request:**
```json
{
  "amount_cents": 10000
}
```

**Response 200:**
```json
{
  "id": "...",
  "balance_cents": 110000,
  "available_balance_cents": 110000,
  "status": "Open"
}
```

**Errors:**
- `400` — Invalid amount
- `400` — Account frozen or closed
- `404` — Account not found

---

### POST /accounts/:id/status

Change account lifecycle state.

**Request:**
```json
{
  "status": "FROZEN"
}
```

Valid statuses: `OPEN`, `FROZEN`, `CLOSED`

**Response 200:**
```json
{
  "id": "...",
  "status": "Frozen"
}
```

**Errors:**
- `400` — Invalid status
- `404` — Account not found

---

### POST /transfers

Double-entry transfer between two accounts.

**Request:**
```json
{
  "from_account": "019e9a81-8a64-7c93-9700-262c63e45025",
  "to_account": "019e9a81-8a64-7c93-9700-262c63e45026",
  "amount_cents": 10000,
  "description": "Invoice #1234 payment"
}
```

| Field | Type | Required |
|-------|------|----------|
| from_account | UUID | Yes |
| to_account | UUID | Yes |
| amount_cents | i64 | Yes |
| description | string | No (default: "Transfer") |

**Response 200:**
```json
{
  "transaction_id": "019e9a82...",
  "journal_entry_id": "019e9a82...",
  "from_balance": 90000,
  "to_balance": 110000
}
```

**Errors:**
- `400` — Insufficient funds, frozen account
- `404` — Account not found
- `503` — Circuit breaker open

**Atomicity:** If credit fails after debit succeeds, the debit is ROLLED BACK (credit back to source).

---

### GET /admin/metrics

Golden Signals observability.

**Response 200:**
```json
{
  "total_requests": 123456,
  "error_rate": 0.001,
  "latency_p50_ms": 1,
  "latency_p99_ms": 5,
  "circuit_state": "Closed",
  "accounts_count": 42
}
```

---

## Idempotency (Future)

Production should add `Idempotency-Key` header:

```
POST /transfers
Idempotency-Key: 550e8400-e29b-41d4-a716-446655440000
```

Same key → same result returned, no duplicate transfer.
