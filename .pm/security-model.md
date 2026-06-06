# Security Threat Model

> STRIDE analysis for the banking ledger. What can go wrong, and how we prevent it.

## STRIDE Matrix

| Threat | Attack Vector | Impact | Mitigation | Status |
|--------|--------------|--------|------------|--------|
| **S**poofing | Fake account creation | Unauthorized transactions | UUID v7 validation, rate limiting | ✅ |
| **T**ampering | Modify journal entry | Financial fraud | SHA-256 hash chain, verify_chain() | ✅ |
| **T**ampering | Modify balance in memory | Silent theft | SeqCst ordering, CAS prevents lost updates | ✅ |
| **R**epudiation | Deny making a transfer | Compliance violation | Append-only journal, HMAC signatures | ✅ |
| **I**nfo Disclosure | Read other's balance | Privacy breach | Input validation, no SQL injection vector | ✅ |
| **I**nfo Disclosure | Memory dump reveals data | Key exposure | No secrets in memory (keys in env vars) | ✅ |
| **D**enial of Service | 100M bogus requests | System overload | Token bucket rate limiter, circuit breaker | ✅ |
| **D**enial of Service | Large payload crash | Memory exhaustion | Axum default body limit (2MB) | ✅ |
| **E**levation | Bypass status check | Debit frozen account | Status checked BEFORE CAS in hot path | ✅ |
| **E**levation | Direct SurrealDB access | Bypass all logic | SurrealDB auth (root:root in dev), network isolation | ⚠️ |

---

## Attack Surface

### 1. REST API (port 3001)

```
POST /accounts          → Input: account_type, currency, balance
POST /accounts/:id/debit → Input: amount_cents
POST /accounts/:id/credit→ Input: amount_cents
POST /transfers          → Input: from, to, amount
```

**Validations:**
- `account_type`: must be one of {ASSET, LIABILITY, EQUITY, REVENUE, EXPENSE}
- `amount_cents`: must be 0 < x < 1,000,000,000,000 ($10B max)
- `currency`: must be valid ISO 4217 (trusted, not validated — fixme)
- `id`: must parse as UUID

### 2. SurrealDB HTTP API (port 29180)

```
POST /sql → Arbitrary SurrealQL execution
```

**Risk:** Anyone with network access + credentials can execute arbitrary SurrealQL.
**Mitigation:** Network isolation (localhost-only binding), auth required.

### 3. In-Memory State

```
DashMap<AccountId, Account> → Readable by any thread with reference
AtomicI64 balance           → Writable by any thread with reference
```

**Risk:** In-process attacks (dependency with malicious code).
**Mitigation:** Cargo audit, minimal dependencies, no unsafe in hot path.

---

## Data Classification

| Data | Classification | Storage | Encryption |
|------|---------------|---------|------------|
| Account balance | Confidential | In-memory + SurrealDB | At-rest: no (SurrealDB file) |
| Journal entries | Critical (immutable) | In-memory + WAL file | At-rest: no |
| Hash chain | Integrity proof | In-memory | SHA-256 (self-verifying) |
| Party PII | Sensitive | SurrealDB | At-rest: no (add in production) |
| HMAC keys | Secret | Environment variable | In-memory only |

---

## Audit Requirements

### What Must Be Logged

Every state-changing operation:

```
[AUDIT 2026-06-06T09:15:30.123Z] TRANSFER | actor=api | from=ACC-001 to=ACC-002 amount=10000
[AUDIT 2026-06-06T09:15:30.456Z] DEBIT    | actor=api | account=ACC-001 amount=10000 result=OK
[AUDIT 2026-06-06T09:15:30.789Z] CREDIT   | actor=api | account=ACC-002 amount=10000 result=OK
```

### What Must Be Retained

- Journal entries: forever (immutable hash chain)
- Audit logs: 7 years (regulatory minimum)
- Hash chain proofs: forever (integrity verification)

---

## Secure Development

- `cargo audit` on every CI run
- `cargo clippy -- -D warnings` gate
- No `unsafe` in hot path (only in ring buffer for MaybeUninit — verified)
- Dependencies pinned in `Cargo.lock`
- `.github/workflows/ci.yml` runs `rustsec/audit-check@v2`

---

## Production Hardening Checklist

- [ ] TLS termination (nginx/Caddy reverse proxy)
- [ ] Authentication (JWT/OAuth2 middleware)
- [ ] Authorization (RBAC: admin vs teller vs customer)
- [ ] Network isolation (SurrealDB on localhost only)
- [ ] Secret rotation (HMAC keys rotated quarterly)
- [ ] Rate limiting per client (not just global token bucket)
- [ ] Audit log shipping to SIEM
- [ ] Memory encryption (SGX/SEV for balance cache)
- [ ] Fuzzing (cargo-fuzz on input parsers)
- [ ] Penetration testing before production
