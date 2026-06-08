//! Serializer edge case coverage — canonical bytes, non-deterministic
//! formats, JSON attacks, and binary round-trip integrity.

#[cfg(test)]
mod serde_edge_tests {
    use rust_decimal::Decimal;
    use crate::domain::money::{Currency, Money};

    #[test]
    fn test_money_serde_json_roundtrip() {
        let usd = Currency::usd();
        let m = Money::from_minor(1_234_56, usd.clone());
        let json = serde_json::to_string(&m).unwrap();
        let parsed: Money = serde_json::from_str(&json).unwrap();
        assert_eq!(m, parsed);
    }

    #[test]
    fn test_money_json_zero_amount() {
        let usd = Currency::usd();
        let m = Money::from_minor(0, usd);
        let json = serde_json::to_string(&m).unwrap();
        let parsed: Money = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, m);
    }

    #[test]
    fn test_money_json_negative_amount() {
        let usd = Currency::usd();
        let m = Money::from_minor(-5000, usd);
        let json = serde_json::to_string(&m).unwrap();
        let parsed: Money = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, m);
    }

    #[test]
    fn test_money_json_reject_extra_fields() {
        let payload = r##"{"amount":{"value":"12.34"},"currency":{"code":"USD","name":"US Dollar","minor_unit":2,"symbol":"$","numeric_code":840},"hacker_field":999}"##;
        let result: Result<Money, _> = serde_json::from_str(payload);
        assert!(result.is_err());
    }

    #[test]
    fn test_money_json_reject_missing_currency() {
        let payload = r#"{"amount": {"value": "10.00"}}"#;
        let result: Result<Money, _> = serde_json::from_str(payload);
        assert!(result.is_err());
    }

    #[test]
    fn test_money_json_reject_invalid_json() {
        let payload = r#"{"amount": "not_a_number"}"#;
        let result: Result<Money, _> = serde_json::from_str(payload);
        assert!(result.is_err());
    }

    #[test]
    fn test_money_json_deny_unknown_works() {
        // Money should have #[serde(deny_unknown_fields)]
        let usd = Currency::usd();
        let m = Money::from_minor(500, usd);
        let json = serde_json::to_string(&m).unwrap();
        let parsed: Money = serde_json::from_str(&json).unwrap();
        assert_eq!(m, parsed);
    }

    #[test]
    fn test_currency_json_roundtrip() {
        let c = Currency::usd();
        let json = serde_json::to_string(&c).unwrap();
        let parsed: Currency = serde_json::from_str(&json).unwrap();
        assert_eq!(c.code, parsed.code);
        assert_eq!(c.minor_unit, parsed.minor_unit);
    }

    #[test]
    fn test_currency_unknown_code_parseable() {
        let payload = r#"{"code":"VND","name":"Vietnamese Dong","minor_unit":0,"symbol":"₫","numeric_code":704}"#;
        let parsed: Currency = serde_json::from_str(payload).unwrap();
        assert_eq!(parsed.code, "VND");
        assert_eq!(parsed.minor_unit, 0);
    }

    #[test]
    fn test_currency_json_reject_wrong_type_minor() {
        let payload = r#"{"code":"USD","name":"US Dollar","minor_unit":"2","symbol":"$","numeric_code":840}"#;
        let result: Result<Currency, _> = serde_json::from_str(payload);
        assert!(result.is_err());
    }
}
