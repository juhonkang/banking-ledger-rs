# Financial Correctness Guarantees

## Invariants

1. **Balance = Available + Holds** — `available_balance_cents() == balance_cents() - total_held`
2. **Credit is monotonic** — balance never decreases on credit
3. **Debit never negative** — debit amount must be positive, balance after debit >= 0
4. **Hold invariant** — `available_balance_cents() >= 0` always
5. **Double-entry** — every transaction has equal debits and credits
6. **Hash chain** — block[i].hash = SHA256(block[i].prev_hash | block[i].data), genesis has 64 zeros

## Overflow Safety

- `credit()`: CAS loop with `checked_add` — returns `CreditError::Overflow` on overflow
- `debit()`: CAS loop on available balance — returns `DebitError::InsufficientFunds`  
- `Money::from_minor()`: `Decimal::from_i128_with_scale` — exact for all ISO 4217 minor units
- `net_position()`: uses `saturating_add` for journal sums

## Memory Ordering

All financial operations use `Ordering::SeqCst` — the strongest guarantee.
- x86-64 already has TSO (total store order), so SeqCst is free on reads
- ARM/POWER need the full barrier — SeqCst prevents store/load reordering
- ~5% perf cost vs Acquire/Release, negligible for correctness

## Precision

- Internal: `i64` cents (5 decimal places reserved for future)
- Display: `Decimal` with currency-specific minor_unit
- Rounding: `RoundingMode::HalfUp` (banker's rounding naive — use `HalfEven` for true banking)
- VND/JPY: minor_unit=0, no fractional display

## Known Gaps

1. Journal-before-balances ordering in transfer — crash window exists (documented in ledger_service.rs:92-96)
2. i128→i64 truncation in journal error display (cosmetic)
3. `f64` token counting in TokenBucket — drift over millions of operations
4. `f64` error_rate comparison in SLO — epsilon issues
