# ⚔️ BANKING LEDGER — DEEP CODE AUDIT REPORT

**Date:** 2026-06-08  
**Scope:** 39 source files, 275 tests  
**Severity scale:** 🔴 CRIT · 🟠 HIGH · 🟡 MED · ⚪ LOW

---

## 🔴 CRITICAL (2 issues — potential financial impact)

### C1. `credit()` non-atomic race condition
**File:** `src/domain/account.rs:247-249`  
**Root cause:** `credit()` performs TWO separate atomic ops — `fetch_add` on `balance` then `fetch_add` on `available_balance`. Between these two operations, reader threads see balance ≠ available_balance.  
**Impact:** In production, a concurrent read could show more money than available. Audits reading mid-credit would see an inconsistency.  
**POC:** `audit_bug_regression::repro_credit_nonatomic_race` — confirmed violations  
**Fix:** Use a single CAS loop that updates both atomically, or use a Mutex for the write path.

### C2. Custom HMAC is NOT RFC 2104 compliant
**File:** `src/log/hash_chain.rs:96-108`  
**Root cause:** Uses `H(key || H(key || message))` instead of proper `H((key⊕opad) || H((key⊕ipad) || message))`.  
**Impact:** SHA-256 is vulnerable to length-extension attacks with this construction. An attacker who knows `H(key || message1)` can compute `H(key || message1 || padding || evil_data)` without knowing the key.  
**Fix:** Use `hmac` crate or implement RFC 2104 properly.

---

## 🟠 HIGH (4 issues)

### H1. `release_hold()` silently wraps on overflow
**File:** `src/domain/account.rs:296-304`  
**Root cause:** `fetch_add` on i64 with no overflow check. Calling `release_hold(i64::MAX)` wraps to negative.  
**Impact:** Buggy client or double-release corrupts account balance silently.  
**POC:** `audit_bug_regression::repro_release_hold_overflow` — wraps to -$9,223,372,036,854,775,807  
**Fix:** Use `checked_add` or `saturating_add`, return error on overflow.

### H2. Memory ordering inconsistency: `debit` uses SeqCst, `place_hold` uses AcqRel
**File:** `src/domain/account.rs:215` vs `:280`  
**Root cause:** `debit()` uses `SeqCst` (strongest), `place_hold()` uses `AcqRel` (weaker). On ARM/PowerPC, AcqRel allows reordering that SeqCst prevents.  
**Impact:** On non-x86 hardware, holds could be reordered relative to debits, causing incorrect balance calculations.  
**Fix:** Standardize on `SeqCst` for all financial operations (performance hit is negligible vs correctness risk).

### H3. `JournalEntry::new()` i64 sum overflow
**File:** `src/domain/journal.rs:91-95`  
**Root cause:** Summing i64 values without overflow protection. Debug mode panics; release mode wraps to negative.  
**Impact:** Very large journal entries (rare but possible) cause silent corruption.  
**Fix:** Use `i128` for intermediate sums, or `checked_add`.

### H4. Saga timeout u64→i64 truncation
**File:** `src/service/saga.rs:372`  
**Root cause:** `timeout_seconds: u64` cast to `as i64` silently truncates to negative for values > i64::MAX.  
**Impact:** Very long timeout requests become negative, causing immediate expiration or panic.  
**POC:** `audit_bug_regression::repro_saga_timeout_truncation` — confirmed  
**Fix:** Clamp to `i64::MAX` or use a different representation.

---

## 🟡 MEDIUM (5 issues)

### M1. TOCTOU: Status check before CAS loop in `debit()`
**File:** `src/domain/account.rs:200-224`  
**Root cause:** Status is checked at line 200, then CAS loop starts at line 206. Another thread can freeze the account between these two points.  
**Impact:** A debit could succeed on a frozen account (theoretically).  
**Fix:** Re-check status inside the CAS loop.

### M2. `HashChain::latest()` panics on empty chain
**File:** `src/log/hash_chain.rs:221`  
**Root cause:** `self.blocks.last().unwrap()` — no error handling.  
**Impact:** If blocks vector is somehow emptied (bug, corruption), calling `latest()` crashes the server.  
**Fix:** Return `Option<&HashLink>` or at minimum use `.expect("genesis must exist")`.

### M3. `HashChain::redact()` mutates "immutable" chain in place
**File:** `src/log/hash_chain.rs:266-301`  
**Root cause:** Takes `&mut self`, modifies blocks in place. Doc says "only do this on a copy" but API doesn't enforce it.  
**Impact:** Accidental mutation of the audit trail destroys immutability guarantee.  
**Fix:** Take `self` (move), return a new `HashChain`, or use copy-on-write.

### M4. `parallel_verify_chain()` is a misleading stub
**File:** `src/log/hash_chain.rs:339-350`  
**Root cause:** Creates `AtomicBool` and `Mutex<Vec>` that are never used. Just calls sequential `verify_chain()`.  
**Impact:** Callers expecting parallel verification get sequential performance.  
**Fix:** Either implement with `rayon` or remove and rename to clarify it's sequential.

### M5. `SurrealStore::save_account_raw()` SQL injection via string format
**File:** `src/store/mod.rs:74-83`  
**Root cause:** Uses `format!()` to build SQL with string interpolation.  
**Impact:** While inputs come from internal types (not user input directly), this is fragile and violates security best practices.  
**Fix:** Use SurrealDB parameterized queries with `$param`.

---

## ⚪ LOW (6 issues — code quality)

### L1. `EntryLeg::amount_cents` is public — invariant violation risk
**File:** `src/domain/journal.rs:32`  
**Fix:** Make private, provide accessor that validates.

### L2. `Currency` equality uses code-only comparison
**File:** `src/domain/money.rs:263`  
**Impact:** Two currencies with same code but different minor_units would add together.  
**Fix:** Compare entire struct or use a CurrencyId.

### L3. `HashChain::signing_key` stored as plaintext `Vec<u8>`
**File:** `src/log/hash_chain.rs:153`  
**Fix:** Use `zeroize` or secure enclave.

### L4. `RwLock<HashMap>` in IdentityService — single-writer bottleneck
**File:** `src/service/identity_service.rs:12-15`  
**Fix:** Use `DashMap` for lock-free reads (like AccountService already does).

### L5. `SagaOrchestrator::completed_sagas` unbounded growth
**File:** `src/service/saga.rs:131`  
**Fix:** Add eviction policy or cap.

### L6. `AppState::ledger` field never read
**File:** `src/api/mod.rs:41`  
**Fix:** Wire or remove.

---

## 📊 TEST COVERAGE GAPS

| Module | Tests | Status |
|--------|-------|--------|
| `account.rs` | 17 | ✅ Good |
| `money.rs` | 8 | ✅ Good |
| `journal.rs` | 8 | ✅ Good |
| `party` + `identifier` | 15 | ✅ Good |
| `account_service` | 29 | ✅ Exhaustive |
| `rbac` | 7 | ✅ Good |
| `resilience` | 23 | ✅ Good |
| `hash_chain` | 22 | ✅ Good |
| `extensions` | 4 | ⚠ Low coverage |
| `api/mod.rs` | 0 | ❌ No direct tests |
| `store/mod.rs` | 0 | ❌ No tests |
| `saga.rs` | 4 | ⚠ Low for complexity |

---

**Total bugs found:** 17 (2 CRIT, 4 HIGH, 5 MED, 6 LOW)  
**POC tests added:** 10 (all passing, demonstrating real bugs)  
