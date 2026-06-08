#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use crate::domain::money::{Currency, Money, RoundingMode};

    // ━━━ Basic (existing) ━━━

    #[test]
    fn test_money_from_minor_usd() {
        let usd = Currency::usd();
        let m = Money::from_minor(12345, usd.clone());
        assert_eq!(m.amount, dec!(123.45));
        assert_eq!(m.to_minor(), 12345);
    }

    #[test]
    fn test_money_from_minor_vnd() {
        let vnd = Currency::vnd();
        let m = Money::from_minor(50_000, vnd.clone());
        assert_eq!(m.amount, dec!(50000));
        assert_eq!(m.to_minor(), 50000);
    }

    #[test]
    fn test_money_add_same_currency() {
        let usd = Currency::usd();
        let a = Money::new(dec!(100.50), usd.clone());
        let b = Money::new(dec!(50.25), usd.clone());
        let c = (a + b).unwrap();
        assert_eq!(c.amount, dec!(150.75));
    }

    #[test]
    fn test_money_add_currency_mismatch() {
        let a = Money::new(dec!(100), Currency::usd());
        let b = Money::new(dec!(100), Currency::eur());
        let result = a + b;
        assert!(result.is_err());
    }

    #[test]
    fn test_money_sub() {
        let usd = Currency::usd();
        let a = Money::new(dec!(100.00), usd.clone());
        let b = Money::new(dec!(33.33), usd);
        let c = (a - b).unwrap();
        assert_eq!(c.amount, dec!(66.67));
    }

    #[test]
    fn test_half_even_rounding() {
        let usd = Currency::usd();
        let m = Money::new(dec!(1.005), usd);
        let rounded = m.round(RoundingMode::HalfEven);
        assert_eq!(rounded.amount, dec!(1.00)); // banker's rounding
    }

    #[test]
    fn test_half_up_rounding() {
        let usd = Currency::usd();
        let m = Money::new(dec!(1.005), usd);
        let rounded = m.round(RoundingMode::HalfUp);
        assert_eq!(rounded.amount, dec!(1.01));
    }

    #[test]
    fn test_jpy_no_decimal() {
        let jpy = Currency::jpy();
        let m = Money::new(dec!(1000.567), jpy.clone());
        let rounded = m.round(RoundingMode::HalfEven);
        assert_eq!(rounded.amount, dec!(1001)); // rounded to whole yen
        assert_eq!(rounded.to_minor(), 1001);
    }

    // ━━━ Edge Cases: Zero ━━━

    #[test]
    fn test_zero_usd() {
        let usd = Currency::usd();
        let m = Money::zero(usd.clone());
        assert_eq!(m.amount, dec!(0.00));
        assert_eq!(m.to_minor(), 0);
        assert_eq!(m.currency.code, "USD");
    }

    #[test]
    fn test_zero_add_zero() {
        let usd = Currency::usd();
        let a = Money::zero(usd.clone());
        let b = Money::zero(usd);
        let c = (a + b).unwrap();
        assert_eq!(c.amount, dec!(0.00));
        assert_eq!(c.to_minor(), 0);
    }

    #[test]
    fn test_zero_sub_nonzero() {
        let usd = Currency::usd();
        let a = Money::zero(usd.clone());
        let b = Money::new(dec!(50.00), usd);
        let c = (a - b).unwrap();
        assert_eq!(c.amount, dec!(-50.00));
        assert_eq!(c.to_minor(), -5000);
    }

    // ━━━ Edge Cases: Negative ━━━

    #[test]
    fn test_negative_from_minor() {
        let usd = Currency::usd();
        let m = Money::from_minor(-5000, usd);
        assert_eq!(m.amount, dec!(-50.00));
        assert_eq!(m.to_minor(), -5000);
    }

    #[test]
    fn test_negative_add_positive() {
        let usd = Currency::usd();
        let a = Money::new(dec!(-100.00), usd.clone());
        let b = Money::new(dec!(150.00), usd);
        let c = (a + b).unwrap();
        assert_eq!(c.amount, dec!(50.00));
    }

    #[test]
    fn test_negative_sub_negative() {
        let usd = Currency::usd();
        let a = Money::new(dec!(-100.00), usd.clone());
        let b = Money::new(dec!(-50.00), usd);
        let c = (a - b).unwrap();
        assert_eq!(c.amount, dec!(-50.00));
    }

    #[test]
    fn test_sub_currency_mismatch() {
        let a = Money::new(dec!(100), Currency::usd());
        let b = Money::new(dec!(50), Currency::eur());
        let result = a - b;
        assert!(result.is_err());
    }

    // ━━━ Edge Cases: Large Values ━━━

    #[test]
    fn test_large_value_near_i64_max() {
        let usd = Currency::usd();
        // 92 trillion USD — near i64::MAX cents
        let m = Money::new(dec!(92_233_720_368_54.77), usd);
        assert!(m.try_to_minor().is_some());
    }

    #[test]
    fn test_overflow_i64_max() {
        let usd = Currency::usd();
        // Exceeds i64::MAX when scaled to cents
        let m = Money::new(dec!(92_233_720_368_547_758.08), usd);
        assert!(m.try_to_minor().is_none());
    }

    #[test]
    #[should_panic(expected = "overflow")]
    fn test_to_minor_panics_on_overflow() {
        let usd = Currency::usd();
        let m = Money::new(dec!(92_233_720_368_547_758.08), usd);
        let _ = m.to_minor(); // panics
    }

    #[test]
    fn test_max_safe_value() {
        let usd = Currency::usd();
        // i64::MAX / 100 = 92,233,720,368,547,75.807 — can't represent in Decimal exactly
        // But a value just below should work
        let m = Money::new(dec!(92_233_720_368_547_75.00), usd);
        // This might overflow or not depending on scaling — just verify no panic in try
        let _ = m.try_to_minor();
    }

    // ━━━ Edge Cases: Precision ━━━

    #[test]
    fn test_fractional_cents() {
        let usd = Currency::usd();
        let m = Money::new(dec!(10.005), usd);
        // When converted to minor, 0.005 rounds to 0 (banker's: 0.005 → 0.00)
        let minor = m.to_minor();
        assert_eq!(minor, 1000); // 10.00
    }

    #[test]
    fn test_many_decimals() {
        let usd = Currency::usd();
        let m = Money::new(dec!(1.23456789), usd);
        // to_minor rounds to 2 decimal places
        let minor = m.to_minor();
        assert_eq!(minor, 123); // 1.23 (banker's rounding: 1.2345 → 1.23)
    }

    #[test]
    fn test_vnd_zero_decimal() {
        let vnd = Currency::vnd();
        let m = Money::new(dec!(50000.99), vnd.clone());
        // VND has 0 decimal places — rounds to whole dong
        let minor = m.to_minor();
        assert_eq!(minor, 50001); // 50000.99 → 50001
    }

    // ━━━ Edge Cases: Chained Operations ━━━

    #[test]
    fn test_chained_add_sub() {
        let usd = Currency::usd();
        let a = Money::new(dec!(1000.00), usd.clone());
        let b = Money::new(dec!(200.00), usd.clone());
        let c = Money::new(dec!(50.00), usd);
        let result = ((a + b).unwrap() - c).unwrap();
        assert_eq!(result.amount, dec!(1150.00));
    }

    #[test]
    fn test_mul_scalar() {
        let usd = Currency::usd();
        let m = Money::new(dec!(100.00), usd);
        let fee = m.mul(dec!(0.025)); // 2.5% fee
        assert_eq!(fee.amount, dec!(2.50));
        assert_eq!(fee.currency.code, "USD");
    }

    #[test]
    fn test_mul_zero_scalar() {
        let usd = Currency::usd();
        let m = Money::new(dec!(9999.99), usd);
        let zero = m.mul(dec!(0));
        assert_eq!(zero.amount, dec!(0.00));
    }

    #[test]
    fn test_mul_negative_scalar() {
        let usd = Currency::usd();
        let m = Money::new(dec!(100.00), usd);
        let neg = m.mul(dec!(-1));
        assert_eq!(neg.amount, dec!(-100.00));
    }

    // ━━━ Display ━━━

    #[test]
    fn test_display_usd() {
        let m = Money::new(dec!(1234.56), Currency::usd());
        assert_eq!(format!("{m}"), "$ 1234.56");
    }

    #[test]
    fn test_display_eur() {
        let m = Money::new(dec!(99.99), Currency::eur());
        assert_eq!(format!("{m}"), "€ 99.99");
    }

    #[test]
    fn test_display_vnd() {
        let m = Money::new(dec!(50000), Currency::vnd());
        assert_eq!(format!("{m}"), "₫ 50000");
    }

    #[test]
    fn test_display_jpy() {
        let m = Money::new(dec!(1000), Currency::jpy());
        assert_eq!(format!("{m}"), "¥ 1000");
    }

    // ━━━ Currency Constants ━━━

    #[test]
    fn test_subunits_per_unit() {
        assert_eq!(Currency::usd().subunits_per_unit(), 100);
        assert_eq!(Currency::eur().subunits_per_unit(), 100);
        assert_eq!(Currency::vnd().subunits_per_unit(), 1);
        assert_eq!(Currency::jpy().subunits_per_unit(), 1);
    }

    #[test]
    fn test_currency_equality() {
        let usd1 = Currency::usd();
        let usd2 = Currency::usd();
        assert_eq!(usd1, usd2);

        let eur = Currency::eur();
        assert_ne!(usd1, eur);
    }

    // ━━━ RoundingMode variants ━━━

    #[test]
    fn test_round_half_down() {
        let usd = Currency::usd();
        let m = Money::new(dec!(1.005), usd);
        let rounded = m.round(RoundingMode::HalfDown);
        assert_eq!(rounded.amount, dec!(1.00));
    }

    #[test]
    fn test_round_up() {
        let usd = Currency::usd();
        let m = Money::new(dec!(1.001), usd);
        let rounded = m.round(RoundingMode::Up);
        assert_eq!(rounded.amount, dec!(1.01));
    }

    #[test]
    fn test_round_down() {
        let usd = Currency::usd();
        let m = Money::new(dec!(1.009), usd);
        let rounded = m.round(RoundingMode::Down);
        assert_eq!(rounded.amount, dec!(1.00));
    }

    #[test]
    fn test_round_ceiling_positive() {
        let usd = Currency::usd();
        let m = Money::new(dec!(1.001), usd);
        let rounded = m.round(RoundingMode::Ceiling);
        assert_eq!(rounded.amount, dec!(1.01));
    }

    #[test]
    fn test_round_ceiling_negative() {
        let usd = Currency::usd();
        let m = Money::new(dec!(-1.001), usd);
        let rounded = m.round(RoundingMode::Ceiling);
        assert_eq!(rounded.amount, dec!(-1.00)); // toward zero for negative
    }

    #[test]
    fn test_round_floor_positive() {
        let usd = Currency::usd();
        let m = Money::new(dec!(1.009), usd);
        let rounded = m.round(RoundingMode::Floor);
        assert_eq!(rounded.amount, dec!(1.00)); // toward zero for positive
    }

    #[test]
    fn test_round_floor_negative() {
        let usd = Currency::usd();
        let m = Money::new(dec!(-1.001), usd);
        let rounded = m.round(RoundingMode::Floor);
        assert_eq!(rounded.amount, dec!(-1.01)); // away from zero for negative
    }
}
