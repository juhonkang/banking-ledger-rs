#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use crate::domain::money::{Currency, Money, RoundingMode};

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
}
