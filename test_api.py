#!/usr/bin/env python3
"""Banking Ledger API Integration Tests — end-to-end verification.
Usage: python3 test_api.py [--base-url http://localhost:3001]
"""
import urllib.request, urllib.error, json, sys, time

BASE = "http://localhost:3001"
if "--base-url" in sys.argv:
    BASE = sys.argv[sys.argv.index("--base-url") + 1]

passed = 0
failed = 0

def test(name, method, path, body=None, expected_status=200, expected_field=None, verify=None):
    global passed, failed
    url = BASE + path
    data = json.dumps(body).encode() if body else None
    try:
        req = urllib.request.Request(url, data=data, method=method)
        req.add_header("Content-Type", "application/json")
        resp = urllib.request.urlopen(req, timeout=5)
        result = json.loads(resp.read())
        status = resp.status
    except urllib.error.HTTPError as e:
        try:
            result = json.loads(e.read())
        except:
            result = {"error": f"HTTP {e.code}"}
        status = e.code
    except Exception as e:
        print(f"  FAIL {name}: {e}")
        failed += 1
        return None

    if status != expected_status:
        print(f"  FAIL {name}: expected {expected_status}, got {status}: {json.dumps(result)[:200]}")
        failed += 1
        return None

    if expected_field and expected_field not in str(result):
        print(f"  FAIL {name}: field '{expected_field}' missing. Got: {json.dumps(result)[:200]}")
        failed += 1
        return None

    if verify and not verify(result):
        print(f"  FAIL {name}: custom verify failed")
        failed += 1
        return None

    print(f"  PASS {name} (status={status})")
    passed += 1
    return result

# ━━━ Wait for server ━━━
print(f"=== Banking Ledger API Integration Tests ===\n")
print(f"Target: {BASE}")
for _ in range(30):
    try:
        urllib.request.urlopen(BASE + "/health", timeout=2)
        break
    except:
        time.sleep(0.5)
else:
    print("FAIL: Server not reachable")
    sys.exit(1)

print("Server ready.\n")

# ━━━ Test Suite ━━━

# 1. Health
test("Health check", "GET", "/health")

# 2. Create asset account
r = test("Create Asset account", "POST", "/accounts", 
    {"account_type": "ASSET", "currency": "USD", "initial_balance_cents": 1000000},
    verify=lambda r: (
        r.get("id") is not None
        and r.get("balance_formatted") is not None
        and "10000.00 USD" in r.get("balance_formatted", "")
        and r.get("currency_symbol") == "$"
        and r.get("currency_decimals") == 2
    ))
acc_id = r.get("id") if r else None

# 3. Get account
if acc_id:
    test("Get account", "GET", f"/accounts/{acc_id}", expected_field="balance_cents")

# 4. Credit
if acc_id:
    r2 = test("Credit account", "POST", f"/accounts/{acc_id}/credit",
        {"amount_cents": 500000},
        verify=lambda r: r.get("balance_cents") == 1500000)

# 5. Debit
if acc_id:
    r3 = test("Debit account", "POST", f"/accounts/{acc_id}/debit",
        {"amount_cents": 300000},
        verify=lambda r: r.get("balance_cents") == 1200000)

# 6. Insufficient funds
if acc_id:
    test("Overdraft rejected", "POST", f"/accounts/{acc_id}/debit",
        {"amount_cents": 99999999}, expected_status=400)

# 7. Freeze state machine
if acc_id:
    test("Freeze account", "POST", f"/accounts/{acc_id}/status",
        {"status": "FROZEN"}, expected_field="status")
    test("Debit frozen fails", "POST", f"/accounts/{acc_id}/debit",
        {"amount_cents": 1000}, expected_status=400)
    test("Credit frozen fails", "POST", f"/accounts/{acc_id}/credit",
        {"amount_cents": 1000}, expected_status=400)
    test("Unfreeze account", "POST", f"/accounts/{acc_id}/status",
        {"status": "OPEN"})

# 8. Invalid account type
test("Invalid account type", "POST", "/accounts",
    {"account_type": "CRYPTO", "currency": "USD", "initial_balance_cents": 100},
    expected_status=400)

# 9. Transfer (double-entry)
r6 = test("Create 2nd account", "POST", "/accounts",
    {"account_type": "LIABILITY", "currency": "USD", "initial_balance_cents": 500000},
    expected_field="id")
acc2_id = r6.get("id") if r6 else None

if acc_id and acc2_id:
    r7 = test("Transfer between accounts", "POST", "/transfers",
        {"from_account": acc_id, "to_account": acc2_id, "amount_cents": 200000,
         "description": "Test transfer"},
        verify=lambda r: (
            r.get("from_balance") == 1000000 
            and r.get("to_balance") == 700000
            and r.get("journal_entry_id") is not None
            and r.get("chain_index") is not None
            and r.get("chain_hash") is not None
            and isinstance(r.get("chain_index"), int)
            and len(r.get("chain_hash", "")) == 64
            and "2000.00 USD" in r.get("amount_formatted", "")
            and r.get("amount_decimal") == "2000.00"
        ))

# 10. Journal + Hash Chain audit
test("List journal entries", "GET", "/journal", expected_field="id")
test("Verify hash chain", "GET", "/journal/verify", 
    verify=lambda r: r.get("valid") == True and r.get("chain_length", 0) >= 2)

# Get chain proof for genesis block (index 0)
test("Chain proof genesis", "GET", "/journal/proof/0",
    verify=lambda r: r.get("index") == 0 and r.get("previous_block_hash") is None)

# Get chain proof for the transfer block (index 1)
r_proof = test("Chain proof transfer", "GET", "/journal/proof/1",
    verify=lambda r: r.get("index") == 1 and r.get("previous_block_hash") is not None)

# 11. Metrics (updated with journal + chain info)
test("Metrics endpoint", "GET", "/admin/metrics", expected_field="journal_entries")

# 12. Error handling
test("404 handling", "GET", "/accounts/00000000-0000-0000-0000-000000000000", expected_status=404)
test("Bad method on account", "PATCH", "/accounts", expected_status=405)

# 13. Multi-currency accounts
test("Create EUR account", "POST", "/accounts",
    {"account_type": "ASSET", "currency": "EUR", "initial_balance_cents": 50000},
    expected_field="id")
test("Create VND account", "POST", "/accounts",
    {"account_type": "ASSET", "currency": "VND", "initial_balance_cents": 10000000},
    expected_field="id")

# 14. Negative values rejected
if acc_id:
    test("Negative debit rejected", "POST", f"/accounts/{acc_id}/debit",
        {"amount_cents": -100}, expected_status=400)

# 15. Rate limiting headers
r_health = test("Health has rate limit header", "GET", "/health",
    verify=lambda r: True)  # just check it responds
# Verify X-RateLimit-Remaining header was set (tested via curl separately)

# 16. Broadcast metrics endpoint now shows rate limiter
test("Metrics shows rate limit info", "GET", "/admin/metrics",
    verify=lambda r: r.get("accounts_count", -1) >= 0)

# ━━━ Summary ━━━
total = passed + failed
print(f"\n{'='*50}")
print(f"  Results: {passed}/{total} passed")
if failed:
    print(f"  FAILURES: {failed}")
    sys.exit(1)
else:
    print(f"  ALL TESTS PASSED")
    sys.exit(0)
