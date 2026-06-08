# Security Model — STRIDE Analysis

## Data Classification

| Level | Data | Protection |
|-------|------|-----------|
| **PII** | Party name, email, identifiers | Redact on export, audit-log access |
| **Financial** | Account balances, transaction amounts | Immutable hash chain, HMAC verification |
| **Operational** | Journal entries, audit trail | Tamper detection, chain proofs |
| **Public** | API health, metrics | Rate-limited read |

## STRIDE Threat Matrix

| Threat | Component | Mitigation | Status |
|--------|-----------|-----------|--------|
| **S**poofing | API auth | RBAC SubjectId header, admin bootstrap | ⚠️ Default admin on missing header (debug only) |
| **T**ampering | HashChain | SHA-256 chain, tamper detection scan, chain proofs | ✅ |
| **R**epudiation | Journal | Immutable append-only journal, HMAC-signed entries | ✅ |
| **I**nfo Disclosure | Redaction API | Replace block data with REDACTED marker, preserve hash | ✅ |
| **D**oS | Rate limiting | TokenBucket per-endpoint, Bulkhead isolation, CircuitBreaker | ✅ |
| **E**levation | RBAC | Role-based permissions (Admin/Auditor/Customer), action gating | ✅ |

## Attack Surface

| Vector | Exposure | Hardening |
|--------|----------|-----------|
| HTTP API | Port 3001 | Rate limiting, input validation, RBAC |
| SurrealDB | Port 4321 (internal) | Localhost-only, no external bind |
| Hash chain | In-memory | Immutable after append, redact preserves chain |
| Memory | Account balances | AtomicI64 CAS — no unsafe access |

## Cryptographic Primitives

- **SHA-256**: Hash chain blocks, transaction hashing
- **HMAC-SHA256**: Journal entry signing, internal verification
- **Ed25519**: Digital signatures for transaction non-repudiation (added R77)
- **CRC-64**: WAL entry integrity (lightweight, non-cryptographic)

## Known Gaps

1. Default admin on missing auth header — MUST be compile-time gated for production
2. No TLS on HTTP server — termination should be at reverse proxy
3. No secret rotation mechanism for HMAC signing key
4. SurrealDB credentials in plaintext — use env vars or vault
