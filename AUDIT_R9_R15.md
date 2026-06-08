# ⚔️ BANKING LEDGER — DEEP AUDIT ROUNDS 9-15

**Date:** 2026-06-08  
**Files analyzed:** 39 source files + 4 config/infra files  
**New findings:** 23 issues (5 HIGH, 10 MED, 8 LOW)

---

## 🟠 HIGH (5 issues)

### H5. CI: Clippy failures silently allowed
**File:** `.github/workflows/ci.yml:44`  
**Bug:** `cargo clippy -- -D warnings || true` — the `|| true` masks ALL clippy errors.  
**Impact:** Broken lint pipeline. Code quality gates are decorative only.  
**Fix:** Remove `|| true`, fix all 169 warnings.

### H6. Docker HEALTHCHECK broken — curl not installed
**File:** `Dockerfile:15`  
**Bug:** HEALTHCHECK runs `curl` but the runtime image `debian:bookworm-slim` has no curl.  
**Impact:** Container always shows unhealthy in Docker. In production, orchestrator restarts it endlessly.  
**Fix:** Install curl in runtime image or use a different health check (e.g., `/dev/tcp` probe).

### H7. API: No authorization checks — RBAC is disconnected
**File:** `src/api/mod.rs` — all handlers  
**Bug:** RbacEngine exists with full permission model but ZERO handlers check permissions.  
**Impact:** Anyone with network access can create accounts, transfer funds, view audit logs.  
**Fix:** Add middleware that checks `Request::headers()` for `x-subject-id` and validates permissions.

### H8. Docker: Secrets in environment variables
**File:** `docker-compose.yml:18-20, 39-40`  
**Bug:** SURREAL_USER/SURREAL_PASS exposed as plain env vars. `docker inspect` leaks them.  
**Impact:** Password exposure to anyone with Docker access.  
**Fix:** Use Docker secrets or .env file with restricted permissions.

### H9. API: Unbounded journal — OOM on high throughput
**File:** `src/api/mod.rs:600`  
**Bug:** `state.journal` is a `Vec<JournalEntry>` that grows without bound. No pagination, no eviction.  
**Impact:** After ~10M transactions, server consumes GBs of RAM and eventually OOM-killed.  
**Fix:** Implement cap or paginated storage. Use SurrealDB as backing store for journal entries.

---

## 🟡 MEDIUM (10 issues)

| # | File | Description |
|---|------|-------------|
| M6 | api/mod.rs | transfer() rollback is non-atomic — if credit rollback fails, money lost |
| M7 | api/mod.rs | Error messages leak internals: `{e:?}` exposes enum variants to clients |
| M8 | api/mod.rs | No input validation on `amount_cents` — accepts i64::MIN (negative) |
| M9 | api/mod.rs | `persist_after_mutation()` fires unbounded tokio::spawn per request |
| M10 | saga.rs | completed_sagas VecDeque grows forever, never pruned |
| M11 | identity_service.rs | parties/identifiers HashMaps grow forever without eviction |
| M12 | Dockerfile | Rust 1.89 pinned (vs 1.96 current), runs as root |
| M13 | .git/hooks/pre-push | Missing `--container-architecture linux/amd64` (breaks on ARM Mac) |
| M14 | ci.yml | Integration test uses `sleep 3` — race condition, brittle |
| M15 | Dockerfile | `touch src/main.rs` build hack — fragile caching |

---

## ⚪ LOW (8 issues)

| # | File | Description |
|---|------|-------------|
| L7 | api/mod.rs | 40 unwrap() calls — poisoned mutex crashes whole server |
| L8 | ci.yml | No test coverage reporting (tarpaulin/codecov) |
| L9 | ci.yml | No fuzz testing |
| L10 | ci.yml | pkill could kill wrong process |
| L11 | Dockerfile | No non-root user (security best practice) |
| L12 | docker-compose.yml | No resource limits (mem_limit, cpus) |
| L13 | docker-compose.yml | SurrealDB :latest tag (supply chain) |
| L14 | main.rs | tokio::main without worker_threads config |

---

## 📊 CUMULATIVE AUDIT SUMMARY

| Severity | Rounds 1-8 | Rounds 9-15 | Total |
|----------|-----------|-------------|-------|
| CRITICAL | 2 (all fixed) | 0 | 2 |
| HIGH | 4 (all fixed) | 5 | 9 |
| MEDIUM | 5 (2 fixed) | 10 | 15 |
| LOW | 6 | 8 | 14 |
| **TOTAL** | **17** | **23** | **40** |

---

## 🔧 RECOMMENDED FIX PRIORITY

1. **H5: Fix clippy `|| true`** — immediate, 1-line fix
2. **H6: Fix Docker HEALTHCHECK** — install curl, 1-line fix
3. **H7: Wire RBAC to API** — largest effort, but crucial
4. **H9: Cap journal** — critical for production stability
