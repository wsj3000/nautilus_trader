// -------------------------------------------------------------------------------------------------
//  Copyright (C) 2015-2026 Nautech Systems Pty Ltd. All rights reserved.
//  https://nautechsystems.io
//
//  Licensed under the GNU Lesser General Public License Version 3.0 (the "License");
//  You may not use this file except in compliance with the License.
//  You may obtain a copy of the License at https://www.gnu.org/licenses/lgpl-3.0.en.html
//
//  Unless required by applicable law or agreed to in writing, software
//  distributed under the License is distributed on an "AS IS" BASIS,
//  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//  See the License for the specific language governing permissions and
//  limitations under the License.
// -------------------------------------------------------------------------------------------------

//! Minimum pricing increment rules for US equities (SEC Reg NMS Rule 612).
//!
//! Alpaca publishes no per-instrument price increment, so order prices are validated against the
//! regulation directly.
//!
//! Rule 612 sets the increment from the **order's own price**, not from where the stock currently
//! trades: a limit at `0.9999` is always permissible and a limit at `1.001` never is, whatever the
//! last trade was. Modelling the increment as a property of the instrument would get this wrong.
//!
//! The rule governs quoting and order acceptance only. The SEC declined to set a minimum increment
//! for executions, so fills may still print in sub-penny amounts and must not be validated here.
//!
//! # Pending amendment
//!
//! The 2024 amendments to Rule 612 add a `$0.005` increment for tick-constrained NMS stocks at or
//! above `$1.00`, making the increment symbol-dependent and reassessed semiannually. Compliance has
//! been deferred by exemptive relief three times, most recently to **2027-11-01**, so every symbol
//! is on the penny increment until then and the flat rule below is correct as written. When the
//! amendment takes effect this becomes a per-symbol lookup; [`min_price_increment`] is the single
//! place that has to change.

use rust_decimal::Decimal;
use rust_decimal_macros::dec;

/// Decimal places used to model Alpaca equity prices.
///
/// Four, not two: sub-dollar quotes are permitted to `$0.0001`, and the risk engine rejects any
/// order price carrying more precision than its instrument declares.
pub const PRICE_PRECISION: u8 = 4;

/// The price at or above which orders must be priced in whole cents.
pub const PENNY_THRESHOLD: Decimal = dec!(1.00);

/// Minimum increment for orders priced at or above [`PENNY_THRESHOLD`].
pub const PENNY_INCREMENT: Decimal = dec!(0.01);

/// Minimum increment for orders priced below [`PENNY_THRESHOLD`].
pub const SUB_DOLLAR_INCREMENT: Decimal = dec!(0.0001);

/// Returns the minimum permissible price increment for an order priced at `price`.
///
/// Keyed on the order price itself, never on the instrument or its last traded price.
#[must_use]
pub fn min_price_increment(price: Decimal) -> Decimal {
    if price.abs() >= PENNY_THRESHOLD {
        PENNY_INCREMENT
    } else {
        SUB_DOLLAR_INCREMENT
    }
}

/// Returns true when `price` is a permissible order price under Rule 612.
///
/// A price is permissible when it is an exact multiple of the increment that applies at its own
/// magnitude.
#[must_use]
pub fn is_valid_order_price(price: Decimal) -> bool {
    let increment = min_price_increment(price);
    // `Decimal` remainder is exact, so this is a true divisibility test rather than an
    // epsilon comparison.
    (price % increment).is_zero()
}

/// Describes why an order price is not permissible.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "price {price} violates SEC Rule 612: orders {bound} {threshold} must be in increments of {increment}"
)]
pub struct RegNmsViolation {
    /// The offending price.
    pub price: Decimal,
    /// The increment that applies at this price.
    pub increment: Decimal,
    /// The threshold the price was compared against.
    pub threshold: Decimal,
    /// Whether the price sits at/above or below the threshold.
    pub bound: &'static str,
}

/// Validates an order price against Rule 612.
///
/// # Errors
///
/// Returns [`RegNmsViolation`] when `price` is not a multiple of the applicable increment.
pub fn check_order_price(price: Decimal) -> Result<(), RegNmsViolation> {
    if is_valid_order_price(price) {
        return Ok(());
    }

    let at_or_above = price.abs() >= PENNY_THRESHOLD;
    Err(RegNmsViolation {
        price,
        increment: min_price_increment(price),
        threshold: PENNY_THRESHOLD,
        bound: if at_or_above { "at or above" } else { "below" },
    })
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case(dec!(0.0001), SUB_DOLLAR_INCREMENT)]
    #[case(dec!(0.9999), SUB_DOLLAR_INCREMENT)]
    #[case(dec!(1.00), PENNY_INCREMENT)]
    #[case(dec!(1.01), PENNY_INCREMENT)]
    #[case(dec!(304.69), PENNY_INCREMENT)]
    fn test_increment_is_keyed_on_the_order_price(
        #[case] price: Decimal,
        #[case] expected: Decimal,
    ) {
        assert_eq!(min_price_increment(price), expected);
    }

    #[rstest]
    fn test_threshold_is_inclusive_at_one_dollar() {
        // Exactly $1.00 falls in the penny band, so $1.0001 is not permissible.
        assert_eq!(min_price_increment(dec!(1.00)), PENNY_INCREMENT);
        assert!(is_valid_order_price(dec!(1.00)));
        assert!(!is_valid_order_price(dec!(1.0001)));
    }

    #[rstest]
    #[case(dec!(0.9999))]
    #[case(dec!(0.0001))]
    #[case(dec!(0.5))]
    #[case(dec!(0.1234))]
    fn test_sub_dollar_prices_may_use_four_decimals(#[case] price: Decimal) {
        assert!(is_valid_order_price(price));
    }

    #[rstest]
    #[case(dec!(1.001))]
    #[case(dec!(1.005))]
    #[case(dec!(10.999))]
    #[case(dec!(304.695))]
    fn test_sub_penny_at_or_above_one_dollar_is_rejected(#[case] price: Decimal) {
        assert!(!is_valid_order_price(price));
        let err = check_order_price(price).unwrap_err();
        assert_eq!(err.increment, PENNY_INCREMENT);
        assert_eq!(err.bound, "at or above");
    }

    #[rstest]
    fn test_finer_than_four_decimals_is_rejected_below_a_dollar() {
        assert!(!is_valid_order_price(dec!(0.00005)));
        let err = check_order_price(dec!(0.00005)).unwrap_err();
        assert_eq!(err.increment, SUB_DOLLAR_INCREMENT);
        assert_eq!(err.bound, "below");
    }

    #[rstest]
    #[case(dec!(1.00))]
    #[case(dec!(0.01))]
    #[case(dec!(304.69))]
    #[case(dec!(1234.56))]
    fn test_valid_prices_pass_the_check(#[case] price: Decimal) {
        assert!(check_order_price(price).is_ok());
    }

    #[rstest]
    fn test_half_penny_is_not_yet_permissible() {
        // Guards the pending 2024 Rule 612 amendment: until the 2027-11-01 compliance date no
        // symbol may quote in half pennies. This test should fail when that changes.
        assert!(!is_valid_order_price(dec!(1.005)));
        assert!(!is_valid_order_price(dec!(25.125)));
    }

    #[rstest]
    fn test_trailing_zeros_do_not_affect_validity() {
        // `1.0100` and `1.01` are the same number with different scales; divisibility must not
        // depend on how the value was written.
        assert!(is_valid_order_price(dec!(1.0100)));
        assert!(is_valid_order_price(dec!(1.01)));
    }

    #[rstest]
    fn test_violation_message_names_the_applicable_increment() {
        let err = check_order_price(dec!(1.001)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("1.001"), "{msg}");
        assert!(msg.contains("0.01"), "{msg}");
        assert!(msg.contains("at or above"), "{msg}");
    }
}
