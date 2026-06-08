//! Money — precise financial arithmetic with currency context.
//!
//! # Why `rust_decimal` over f64?
//!
//! Binary floating point (f64) cannot represent 0.1 exactly. Over millions
//! of transactions, rounding errors compound into real money loss.
//! `rust_decimal` uses a 96-bit integer mantissa — exact for base-10 values.
//!
//! # Example
//!
//! ```rust
//! use banking_ledger::domain::money::{Money, Currency};
//! use rust_decimal_macros::dec;
//!
//! let usd = Currency::usd();
//! let salary = Money::new(dec!(5000.00), usd.clone());
//! let bonus = Money::new(dec!(1000.00), usd.clone());
//! let total = (salary + bonus).unwrap();
//! assert_eq!(total.amount, dec!(6000.00));
//! ```

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::ops::{Add, Sub};

/// ISO 4217 currency with metadata for proper scaling and display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Currency {
    /// ISO 4217 alphabetic code (e.g., "USD", "EUR", "VND")
    pub code: String,
    /// Human-readable name
    pub name: String,
    /// Number of decimal places (2 for USD cents, 0 for JPY/VND)
    pub minor_unit: u8,
    /// Display symbol
    pub symbol: String,
    /// ISO 4217 numeric code
    pub numeric_code: u16,
}

impl Currency {
    /// US Dollar — 2 decimal places
    #[must_use]
    pub fn usd() -> Self {
        Self {
            code: "USD".into(),
            name: "US Dollar".into(),
            minor_unit: 2,
            symbol: "$".into(),
            numeric_code: 840,
        }
    }

    /// Euro — 2 decimal places
    #[must_use]
    pub fn eur() -> Self {
        Self {
            code: "EUR".into(),
            name: "Euro".into(),
            minor_unit: 2,
            symbol: "€".into(),
            numeric_code: 978,
        }
    }

    /// Vietnamese Dong — 0 decimal places
    #[must_use]
    pub fn vnd() -> Self {
        Self {
            code: "VND".into(),
            name: "Vietnamese Dong".into(),
            minor_unit: 0,
            symbol: "₫".into(),
            numeric_code: 704,
        }
    }

    /// Japanese Yen — 0 decimal places
    #[must_use]
    pub fn jpy() -> Self {
        Self {
            code: "JPY".into(),
            name: "Japanese Yen".into(),
            minor_unit: 0,
            symbol: "¥".into(),
            numeric_code: 392,
        }
    }

    /// Number of subunits per major unit (10^minor_unit)
    #[must_use]
    pub const fn subunits_per_unit(&self) -> u64 {
        10u64.pow(self.minor_unit as u32)
    }

    /// Check if this is a zero-decimal currency (like JPY, VND).
    #[must_use]
    #[cfg(feature = "full")]
    pub const fn is_zero_decimal(&self) -> bool {
        self.minor_unit == 0
    }
}

/// Rounding modes as defined in IEEE 754-2008 / Java `MathContext`.
///
/// **Default is [`RoundingMode::HalfEven`]** (banker's rounding) —
/// the standard for financial systems.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[non_exhaustive]
#[cfg(feature = "full")]
pub enum RoundingMode {
    /// Banker's rounding — round to nearest, ties to even.
    /// Minimizes cumulative rounding bias. **Default for finance.**
    #[default]
    HalfEven,
    /// Round to nearest, ties away from zero
    HalfUp,
    /// Round to nearest, ties toward zero
    HalfDown,
    /// Round away from zero (ceil for positive, floor for negative)
    Up,
    /// Round toward zero (truncate)
    Down,
    /// Round toward positive infinity
    Ceiling,
    /// Round toward negative infinity
    Floor,
}

#[cfg(feature = "full")]
impl RoundingMode {
    /// Apply this rounding mode to a [`Decimal`] value at the given scale.
    #[must_use]
    pub fn apply(self, value: Decimal, scale: u32) -> Decimal {
        match self {
            Self::HalfEven => value
                .round_dp_with_strategy(scale, rust_decimal::RoundingStrategy::MidpointNearestEven),
            Self::HalfUp => value.round_dp_with_strategy(
                scale,
                rust_decimal::RoundingStrategy::MidpointAwayFromZero,
            ),
            Self::HalfDown => value
                .round_dp_with_strategy(scale, rust_decimal::RoundingStrategy::MidpointTowardZero),
            Self::Up => {
                value.round_dp_with_strategy(scale, rust_decimal::RoundingStrategy::AwayFromZero)
            }
            Self::Down => {
                value.round_dp_with_strategy(scale, rust_decimal::RoundingStrategy::ToZero)
            }
            Self::Ceiling => {
                if value >= Decimal::ZERO {
                    value
                        .round_dp_with_strategy(scale, rust_decimal::RoundingStrategy::AwayFromZero)
                } else {
                    value.round_dp_with_strategy(scale, rust_decimal::RoundingStrategy::ToZero)
                }
            }
            Self::Floor => {
                if value >= Decimal::ZERO {
                    value.round_dp_with_strategy(scale, rust_decimal::RoundingStrategy::ToZero)
                } else {
                    value
                        .round_dp_with_strategy(scale, rust_decimal::RoundingStrategy::AwayFromZero)
                }
            }
        }
    }
}

/// Money = a [`Decimal`] amount + [`Currency`] context.
///
/// # Invariants
///
/// - Operations on different currencies return [`MoneyError::CurrencyMismatch`]
/// - Display format respects currency's `minor_unit`
/// - Arithmetic returns new `Money` — original is never mutated
///
/// # Operator Overloading
///
/// ```rust
/// # use banking_ledger::domain::money::{Money, Currency};
/// # use rust_decimal_macros::dec;
/// let a = Money::new(dec!(100), Currency::usd());
/// let b = Money::new(dec!(50), Currency::usd());
/// let c = (a.clone() + b).unwrap();  // Money + Money = Result<Money>
/// let d = a * dec!(3);               // Money * Decimal = Money
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Money {
    /// The monetary amount (precise, no floating point)
    pub amount: Decimal,
    /// The currency context
    pub currency: Currency,
}

impl Money {
    /// Create a new Money value.
    #[must_use]
    pub fn new(amount: Decimal, currency: Currency) -> Self {
        Self { amount, currency }
    }

    /// Zero amount in the given currency.
    #[must_use]
    pub fn zero(currency: Currency) -> Self {
        Self {
            amount: Decimal::ZERO,
            currency,
        }
    }

    /// Create from minor units (cents/satangs/...).
    ///
    /// # Example
    ///
    /// ```rust
    /// # use banking_ledger::domain::money::{Money, Currency};
    /// # use rust_decimal_macros::dec;
    /// let m = Money::from_minor(12345, Currency::usd());
    /// assert_eq!(m.amount, dec!(123.45));
    /// ```
    #[must_use]
    pub fn from_minor(amount_cents: i64, currency: Currency) -> Self {
        let divisor = Decimal::from(currency.subunits_per_unit());
        let amount = Decimal::from_i128_with_scale(i128::from(amount_cents), 0) / divisor;
        Self { amount, currency }
    }

    /// Convert to minor units (cents). Panics on overflow.
    /// Use `try_to_minor` for fallible conversion.
    #[must_use]
    pub fn to_minor(&self) -> i64 {
        self.try_to_minor()
            .expect("Money::to_minor overflow — value too large for i64 cents")
    }

    /// Fallible conversion to minor units. Returns None on overflow.
    #[must_use]
    pub fn try_to_minor(&self) -> Option<i64> {
        let scaled = self.amount * Decimal::from(self.currency.subunits_per_unit());
        let rounded =
            scaled.round_dp_with_strategy(0, rust_decimal::RoundingStrategy::MidpointNearestEven);
        i64::try_from(rounded).ok()
    }

    /// Multiply by a scalar (e.g., for fee calculation).
    #[must_use]
    pub fn mul(&self, scalar: Decimal) -> Money {
        Money {
            amount: self.amount * scalar,
            currency: self.currency.clone(),
        }
    }

    /// Apply rounding with the given mode.
    #[must_use]
    #[cfg(feature = "full")]
    pub fn round(&self, mode: RoundingMode) -> Money {
        let scale = u32::from(self.currency.minor_unit);
        Money {
            amount: mode.apply(self.amount, scale),
            currency: self.currency.clone(),
        }
    }

    /// Check that two Money values have the same currency.
    fn check_currency(&self, other: &Money) -> Result<(), MoneyError> {
        if self.currency.code != other.currency.code {
            return Err(MoneyError::CurrencyMismatch {
                expected: self.currency.code.clone(),
                got: other.currency.code.clone(),
            });
        }
        Ok(())
    }
}

// ━━━ Operator Overloading ━━━

impl Add for Money {
    type Output = Result<Money, MoneyError>;

    fn add(self, rhs: Money) -> Self::Output {
        self.check_currency(&rhs)?;
        Ok(Money {
            amount: self.amount + rhs.amount,
            currency: self.currency.clone(),
        })
    }
}

impl Sub for Money {
    type Output = Result<Money, MoneyError>;

    fn sub(self, rhs: Money) -> Self::Output {
        self.check_currency(&rhs)?;
        Ok(Money {
            amount: self.amount - rhs.amount,
            currency: self.currency.clone(),
        })
    }
}

impl std::ops::Mul<Decimal> for Money {
    type Output = Money;

    fn mul(self, scalar: Decimal) -> Money {
        Money {
            amount: self.amount * scalar,
            currency: self.currency,
        }
    }
}

impl std::fmt::Display for Money {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let decimals = self.currency.minor_unit as usize;
        write!(f, "{} {:.*}", self.currency.symbol, decimals, self.amount)
    }
}

/// Error type for invalid Money operations.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum MoneyError {
    /// Attempted arithmetic between different currencies
    #[error("currency mismatch: expected {expected}, got {got}")]
    CurrencyMismatch {
        /// The currency of the left operand
        expected: String,
        /// The currency of the right operand
        got: String,
    },
}
