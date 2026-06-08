//! Coverage gap tests for Money & Currency domain models.
//!
//! Features exercised: zero-decimal display, minor-unit roundtrips,
//! mul/div arithmetic, rounding-mode comparisons, and is_zero_decimal.

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use crate::domain::money::{Currency, Money, RoundingMode};

    // ━━━ 1. VND zero-decimal display ━━━

    #[test]
    fn test_vnd_zero_decimal_display_no_trailing_digits() {
        // VND has minor_unit=0 — Display must show no decimal places.
        let vnd = Currency::vnd();
        let m = Money::new(dec!(100000), vnd);
        let rendered = format!("{m}");
        // Must NOT contain a decimal point
        assert!(
            !rendered.contains('.'),
            "VND display must not contain decimal point: got '{rendered}'"
        );
        assert_eq!(rendered, "₫ 100000");
    }

    #[test]
    fn test_vnd_zero_decimal_display_fractional_amount_rounded() {
        // Even with a fractional Decimal value, Display shows 0 decimal places.
        let vnd = Currency::vnd();
        let m = Money::new(dec!(25000.99), vnd);
        let rendered = format!("{m}");
        assert!(!rendered.contains('.'));
        // Decimal formatting with 0 precision truncates: 25000.99 → "25000"
        assert_eq!(rendered, "₫ 25000");
    }

    // ━━━ 2. from_minor / to_minor roundtrip for JPY ━━━

    #[test]
    fn test_jpy_from_minor_to_minor_roundtrip() {
        let jpy = Currency::jpy();
        let cases: &[(i64, &str)] = &[
            (0, "0"),
            (1, "1"),
            (1_000_000, "1000000"),
            (i64::MAX, "9223372036854775807"),
        ];
        for &(cents, expected_str) in cases {
            let m = Money::from_minor(cents, jpy.clone());
            assert_eq!(
                m.amount.to_string(),
                expected_str,
                "from_minor({cents}) amount mismatch"
            );
            let roundtrip = m.to_minor();
            assert_eq!(
                roundtrip, cents,
                "JPY roundtrip failed: from_minor({cents}) → to_minor gave {roundtrip}"
            );
        }
    }

    #[test]
    fn test_jpy_roundtrip_negative() {
        let jpy = Currency::jpy();
        let m = Money::from_minor(-50000, jpy.clone());
        assert_eq!(m.amount, dec!(-50000));
        assert_eq!(m.to_minor(), -50000);
    }

    #[test]
    fn test_vnd_from_minor_to_minor_roundtrip() {
        let vnd = Currency::vnd();
        // VND also has minor_unit=0, so 1 minor unit = 1 dong
        let m = Money::from_minor(123456, vnd.clone());
        assert_eq!(m.amount, dec!(123456));
        assert_eq!(m.to_minor(), 123456);
    }

    // ━━━ 3. Money mul / div scenarios ━━━

    #[test]
    fn test_money_mul_fee_calculation() {
        let usd = Currency::usd();
        let principal = Money::new(dec!(1500.00), usd);
        // 0.25% management fee
        let fee = principal.mul(dec!(0.0025));
        assert_eq!(fee.amount, dec!(3.75));
        assert_eq!(fee.currency.code, "USD");
    }

    #[test]
    fn test_money_mul_fractional_scalar() {
        let usd = Currency::usd();
        let m = Money::new(dec!(10.00), usd);
        // Multiplying by 1/3 — within Decimal precision
        let third = m.mul(dec!(1) / dec!(3));
        // Due to Decimal finite precision, 10.00 * (1/3) ≈ 10/3
        // Compare at 27 dp to allow for the last-digit difference
        let expected = (dec!(10) / dec!(3)).round_dp(27);
        assert_eq!(third.amount.round_dp(27), expected);
    }

    #[test]
    fn test_money_unit_price_calculation() {
        // "div" scenario: split total cost into per-unit price
        let usd = Currency::usd();
        let total = Money::new(dec!(99.99), usd);
        // Per-unit cost when buying 33 items
        let per_unit = total.mul(dec!(1) / dec!(33));
        // Due to Decimal finite precision, compare at 10 dp
        let expected = (dec!(99.99) / dec!(33)).round_dp(10);
        assert_eq!(per_unit.amount.round_dp(10), expected);
        assert_eq!(per_unit.currency.code, "USD");
    }

    #[test]
    fn test_money_mul_preserves_currency() {
        let eur = Currency::eur();
        let m = Money::new(dec!(250.00), eur);
        let doubled = m.mul(dec!(2));
        assert_eq!(doubled.currency.code, "EUR");
        assert_eq!(doubled.currency.symbol, "€");
        assert_eq!(doubled.currency.minor_unit, 2);
    }

    #[test]
    fn test_money_operator_mul_div_equivalent() {
        // Verify operator* and .mul() produce identical results
        let usd = Currency::usd();
        let a = Money::new(dec!(100.00), usd.clone());
        let b = Money::new(dec!(100.00), usd);

        let via_method = a.mul(dec!(0.15));
        let via_operator = b * dec!(0.15);

        assert_eq!(via_method.amount, via_operator.amount);
        assert_eq!(via_method.amount, dec!(15.00));
    }

    // ━━━ 4. RoundingMode::HalfUp vs HalfEven ━━━

    #[test]
    fn test_half_up_vs_half_even_on_exact_midpoint() {
        let usd = Currency::usd();
        let m = Money::new(dec!(1.005), usd);

        let half_even = m.round(RoundingMode::HalfEven);
        let half_up = m.round(RoundingMode::HalfUp);

        // 1.005: HalfEven → 1.00 (ties to even: 0 is even)
        assert_eq!(half_even.amount, dec!(1.00));
        // 1.005: HalfUp   → 1.01 (ties away from zero)
        assert_eq!(half_up.amount, dec!(1.01));
    }

    #[test]
    fn test_half_up_vs_half_even_on_second_midpoint() {
        let usd = Currency::usd();
        // 2.015 — midpoint where even digit is 2
        let m = Money::new(dec!(2.015), usd);

        let half_even = m.round(RoundingMode::HalfEven);
        let half_up = m.round(RoundingMode::HalfUp);

        // 2.015: HalfEven → 2.02 (ties to even: 2 is even)
        assert_eq!(half_even.amount, dec!(2.02));
        // 2.015: HalfUp   → 2.02 (ties away from zero — same result here)
        assert_eq!(half_up.amount, dec!(2.02));
    }

    #[test]
    fn test_half_up_vs_half_even_vnd_midpoint() {
        let vnd = Currency::vnd();
        // VND has 0 dp — test midpoint at 0 decimal places
        let m = Money::new(dec!(1000.5), vnd);

        let half_even = m.round(RoundingMode::HalfEven);
        let half_up = m.round(RoundingMode::HalfUp);

        // 1000.5 at 0 dp: HalfEven → 1000 (ties to even: 0 is even)
        assert_eq!(half_even.amount, dec!(1000));
        // 1000.5 at 0 dp: HalfUp   → 1001 (ties away from zero)
        assert_eq!(half_up.amount, dec!(1001));
    }

    #[test]
    fn test_rounding_modes_negative_midpoint() {
        let usd = Currency::usd();
        let m = Money::new(dec!(-1.005), usd);

        let half_even = m.round(RoundingMode::HalfEven);
        let half_up = m.round(RoundingMode::HalfUp);

        // -1.005: HalfEven → -1.00 (ties to even)
        assert_eq!(half_even.amount, dec!(-1.00));
        // -1.005: HalfUp   → -1.01 (ties away from zero)
        assert_eq!(half_up.amount, dec!(-1.01));
    }

    // ━━━ 5. Currency::is_zero_decimal ━━━

    /// Helper: construct Bahraini Dinar (BHD) — ISO 4217, 3 decimal places (1000 fils).
    fn bhd() -> Currency {
        Currency {
            code: "BHD".into(),
            name: "Bahraini Dinar".into(),
            minor_unit: 3,
            symbol: "BD".into(),
            numeric_code: 48,
        }
    }

    #[test]
    fn test_is_zero_decimal_jpy_true() {
        let jpy = Currency::jpy();
        assert!(jpy.is_zero_decimal(), "JPY should be zero-decimal (minor_unit=0)");
    }

    #[test]
    fn test_is_zero_decimal_vnd_true() {
        let vnd = Currency::vnd();
        assert!(vnd.is_zero_decimal(), "VND should be zero-decimal (minor_unit=0)");
    }

    #[test]
    fn test_is_zero_decimal_bhd_false() {
        let bhd = bhd();
        assert!(!bhd.is_zero_decimal(), "BHD should NOT be zero-decimal (minor_unit=3)");
    }

    #[test]
    fn test_is_zero_decimal_usd_false() {
        let usd = Currency::usd();
        assert!(!usd.is_zero_decimal(), "USD should NOT be zero-decimal (minor_unit=2)");
    }

    #[test]
    fn test_is_zero_decimal_eur_false() {
        let eur = Currency::eur();
        assert!(!eur.is_zero_decimal(), "EUR should NOT be zero-decimal (minor_unit=2)");
    }

    // ━━━ 6. Bonus: BHD rounding at 3 decimal places ━━━

    #[test]
    fn test_bhd_three_decimal_rounding() {
        let bhd = bhd();
        // BHD has 3 decimal places — rounding at 3 dp
        let m = Money::new(dec!(1.2345), bhd);
        let rounded = m.round(RoundingMode::HalfEven);
        // 1.2345 at 3 dp: 4 is even? The 4 is the 3rd decimal digit — 
        // midpoint is 1.2345, 3rd dp digit is 4 (even) → rounds to 1.234
        assert_eq!(rounded.amount, dec!(1.234));
    }

    #[test]
    fn test_bhd_from_minor_to_minor_roundtrip() {
        let bhd = bhd();
        // 1 BHD = 1000 fils
        let m = Money::from_minor(1_500, bhd.clone());
        assert_eq!(m.amount, dec!(1.500));
        assert_eq!(m.to_minor(), 1_500);
    }
}
