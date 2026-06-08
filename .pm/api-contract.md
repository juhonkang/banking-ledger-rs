# API Contract

## Base URL

`http://localhost:3001`

## Endpoints

### Accounts

| Method | Path | Description | Auth |
|--------|------|-------------|------|
| POST | `/accounts` | Create account | Admin |
| GET | `/accounts/:id` | Get account details | Customer (own) / Admin |
| POST | `/accounts/:id/debit` | Debit account | Admin |
| POST | `/accounts/:id/credit` | Credit account | Admin |
| POST | `/accounts/:id/hold` | Place hold on funds | Admin |
| POST | `/accounts/:id/release` | Release hold | Admin |
| PUT | `/accounts/:id/status` | Change account status | Admin |

### Journal

| Method | Path | Description | Auth |
|--------|------|-------------|------|
| GET | `/journal` | List all journal entries | Auditor / Admin |
| GET | `/journal/:id` | Get specific entry | Auditor / Admin |
| GET | `/journal/account/:id` | Entries for account | Customer (own) / Admin |
| GET | `/journal/trial-balance` | Trial balance report | Auditor / Admin |

### Audit

| Method | Path | Description | Auth |
|--------|------|-------------|------|
| GET | `/audit/verify` | Verify hash chain integrity | Auditor / Admin |
| GET | `/audit/proof/:index` | Get chain proof at index | Auditor / Admin |
| GET | `/audit/export` | Export full audit log | Auditor / Admin |
| POST | `/audit/redact/:index` | Redact block (preserves hash) | Admin |

### RBAC

| Method | Path | Description | Auth |
|--------|------|-------------|------|
| GET | `/rbac/matrix` | View permission matrix | Admin |
| POST | `/rbac/bind` | Bind role to subject | Admin |
| GET | `/rbac/subject/:id` | Get subject roles | Admin |
| GET | `/rbac/export` | Export RBAC state | Admin |

### Identity

| Method | Path | Description | Auth |
|--------|------|-------------|------|
| POST | `/parties` | Create party | Admin |
| GET | `/parties` | List all parties | Admin |
| GET | `/parties/:id` | Get party details | Customer (own) / Admin |
| POST | `/parties/:id/identifiers` | Add identifier | Admin |
| GET | `/parties/:id/identifiers` | List identifiers | Customer (own) / Admin |
| POST | `/identifiers/verify` | Verify identifier | Admin |

### System

| Method | Path | Description | Auth |
|--------|------|-------------|------|
| GET | `/health` | Health check | Public |
| GET | `/metrics` | Service metrics | Admin |

## Error Response Format

```json
{
  "error": "ERROR_CODE",
  "message": "Human-readable description",
  "details": {}
}
```

## Rate Limiting

- Default: 100 requests/second per endpoint
- Header: `X-RateLimit-Remaining`
- 429 Too Many Requests on exhaustion

## Authentication

- Header: `X-Subject-Id: <uuid>`
- Falls back to default admin in debug mode
- Production requires real auth provider

## Data Types

- `amount_cents`: i64 — amount in smallest currency unit
- `currency`: ISO 4217 3-letter code (USD, EUR, VND, JPY, BHD...)
- `status`: `Open` | `Frozen` | `Closed`
- `account_type`: `Asset` | `Liability` | `Equity` | `Revenue` | `Expense`
