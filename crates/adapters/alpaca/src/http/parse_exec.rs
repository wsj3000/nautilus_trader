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

//! Conversion from Alpaca execution payloads into Nautilus reports.

use nautilus_core::UnixNanos;
use nautilus_model::{
    enums::{OrderStatus, PositionSideSpecified},
    identifiers::{AccountId, ClientOrderId, VenueOrderId},
    reports::{OrderStatusReport, PositionStatusReport},
    types::{Price, Quantity},
};
use rust_decimal::Decimal;

use crate::http::{
    models::{AlpacaOrder, AlpacaPosition},
    parse::{instrument_id, parse_rfc3339_nanos},
};

/// Parses a decimal string exactly, without an intermediate float.
///
/// # Errors
///
/// Returns an error if the value is not a valid decimal.
pub fn parse_decimal(raw: &str, field: &str) -> anyhow::Result<Decimal> {
    raw.trim()
        .parse::<Decimal>()
        .map_err(|e| anyhow::anyhow!("Invalid decimal for {field}: '{raw}' ({e})"))
}

/// Parses a share quantity, which the venue may report signed.
///
/// # Errors
///
/// Returns an error if the value is not a valid decimal.
pub fn parse_quantity(raw: &str, field: &str) -> anyhow::Result<Quantity> {
    let value = parse_decimal(raw, field)?.abs();
    Quantity::new_checked(
        value
            .try_into()
            .map_err(|_| anyhow::anyhow!("Quantity out of range for {field}: '{raw}'"))?,
        0,
    )
    .map_err(|e| anyhow::anyhow!("Invalid quantity for {field}: '{raw}' ({e})"))
}

/// Converts a venue order into a Nautilus order status report.
///
/// # Superseded orders
///
/// An amendment on Alpaca creates a new order: the original moves to `replaced` and a fresh venue
/// identifier takes over. There is no Nautilus status for that, and reporting it as a cancellation
/// would tell the engine an order is dead while it is still working. Such orders are refused here
/// so the caller can follow `replaced_by` instead of publishing a misleading report.
///
/// # Errors
///
/// Returns an error if any enum, quantity, price, or timestamp cannot be converted.
pub fn parse_order_status_report(
    order: &AlpacaOrder,
    account_id: AccountId,
    price_precision: u8,
    ts_init: UnixNanos,
) -> anyhow::Result<OrderStatusReport> {
    if order.status.is_superseded() {
        anyhow::bail!(
            "Order {} was replaced by {}; report the replacement instead",
            order.id,
            order.replaced_by.as_deref().unwrap_or("an unknown order")
        );
    }

    let quantity = match order.qty.as_deref() {
        Some(raw) => parse_quantity(raw, "qty")?,
        None => anyhow::bail!(
            "Order {} has no quantity; notional orders are not supported by this adapter",
            order.id
        ),
    };
    let filled_qty = match order.filled_qty.as_deref() {
        Some(raw) => parse_quantity(raw, "filled_qty")?,
        None => Quantity::new(0.0, 0),
    };

    let ts_accepted = match order.ts_accepted_raw() {
        Some(raw) => parse_rfc3339_nanos(raw)?,
        None => ts_init,
    };
    let ts_last = match order.ts_last_raw() {
        Some(raw) => parse_rfc3339_nanos(raw)?,
        None => ts_accepted,
    };

    let mut report = OrderStatusReport::new(
        account_id,
        instrument_id(&order.symbol),
        Some(ClientOrderId::new(order.client_order_id.as_str())),
        VenueOrderId::new(order.id.as_str()),
        order.side.to_nautilus()?,
        order.resolved_order_type()?.to_nautilus()?,
        order.time_in_force.to_nautilus()?,
        order.status.to_nautilus()?,
        quantity,
        filled_qty,
        ts_accepted,
        ts_last,
        ts_init,
        None,
    );

    if let Some(raw) = order.limit_price.as_deref() {
        report = report.with_price(parse_price(raw, "limit_price", price_precision)?);
    }
    if let Some(raw) = order.stop_price.as_deref() {
        report = report.with_trigger_price(parse_price(raw, "stop_price", price_precision)?);
    }
    if let Some(raw) = order.filled_avg_price.as_deref() {
        report = report.with_avg_px(parse_decimal(raw, "filled_avg_price")?);
    }

    Ok(report)
}

/// Parses a price string at the instrument's precision.
///
/// # Errors
///
/// Returns an error if the value is not a valid decimal or is outside the representable range.
pub fn parse_price(raw: &str, field: &str, precision: u8) -> anyhow::Result<Price> {
    let value: f64 = parse_decimal(raw, field)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("Price out of range for {field}: '{raw}'"))?;
    Price::new_checked(value, precision)
        .map_err(|e| anyhow::anyhow!("Invalid price for {field}: '{raw}' ({e})"))
}

/// Converts a venue position into a Nautilus position status report.
///
/// # Errors
///
/// Returns an error if the quantity or entry price cannot be converted.
pub fn parse_position_status_report(
    position: &AlpacaPosition,
    account_id: AccountId,
    ts_init: UnixNanos,
) -> anyhow::Result<PositionStatusReport> {
    let quantity = parse_quantity(&position.qty, "qty")?;
    let side = if quantity.is_zero() {
        PositionSideSpecified::Flat
    } else if position.is_short() {
        PositionSideSpecified::Short
    } else {
        PositionSideSpecified::Long
    };

    let avg_px_open = parse_decimal(&position.avg_entry_price, "avg_entry_price").ok();

    Ok(PositionStatusReport::new(
        account_id,
        instrument_id(&position.symbol),
        side,
        quantity,
        ts_init,
        ts_init,
        None,
        None,
        avg_px_open,
    ))
}

/// Returns true when the report describes an order the engine should still track.
#[must_use]
pub fn is_open_status(status: OrderStatus) -> bool {
    matches!(
        status,
        OrderStatus::Submitted
            | OrderStatus::Accepted
            | OrderStatus::PartiallyFilled
            | OrderStatus::PendingUpdate
            | OrderStatus::PendingCancel
    )
}

#[cfg(test)]
mod tests {
    use nautilus_model::enums::{OrderSide, OrderType, TimeInForce};
    use rstest::rstest;
    use rust_decimal_macros::dec;

    use super::*;

    const ORDERS_JSON: &str = include_str!("../../test_data/http_orders.json");
    const POSITIONS_JSON: &str = include_str!("../../test_data/http_positions.json");

    fn orders() -> Vec<AlpacaOrder> {
        serde_json::from_str(ORDERS_JSON).unwrap()
    }

    fn positions() -> Vec<AlpacaPosition> {
        serde_json::from_str(POSITIONS_JSON).unwrap()
    }

    fn order_with_status(status: &str) -> AlpacaOrder {
        orders()
            .into_iter()
            .find(|o| o.status.as_ref() == status)
            .unwrap_or_else(|| panic!("no fixture order with status {status}"))
    }

    fn account() -> AccountId {
        AccountId::new("ALPACA-001")
    }

    #[rstest]
    fn test_orders_fixture_deserializes() {
        assert_eq!(orders().len(), 8);
    }

    #[rstest]
    fn test_order_type_accepts_either_field_name() {
        // The venue sends `order_type` and `type` with the same value; only one is required.
        let json = r#"{
            "id": "1", "client_order_id": "c1", "symbol": "AAPL",
            "type": "limit", "side": "buy", "time_in_force": "day", "status": "new",
            "qty": "1"
        }"#;
        let order: AlpacaOrder = serde_json::from_str(json).unwrap();
        assert_eq!(
            order.resolved_order_type().unwrap().to_nautilus().unwrap(),
            OrderType::Limit
        );
    }

    #[rstest]
    fn test_parse_new_order_report() {
        let order = order_with_status("new");
        let report = parse_order_status_report(&order, account(), 4, UnixNanos::from(7)).unwrap();

        assert_eq!(report.instrument_id.to_string(), "AAPL.ALPACA");
        assert_eq!(report.order_side, OrderSide::Buy);
        assert_eq!(report.order_type, OrderType::Limit);
        assert_eq!(report.time_in_force, TimeInForce::Day);
        assert_eq!(report.order_status, OrderStatus::Accepted);
        assert_eq!(report.quantity, Quantity::new(10.0, 0));
        assert_eq!(report.filled_qty, Quantity::new(0.0, 0));
        assert_eq!(report.ts_init, UnixNanos::from(7));
    }

    #[rstest]
    fn test_parse_filled_order_carries_average_price() {
        let order = order_with_status("filled");
        let report = parse_order_status_report(&order, account(), 4, UnixNanos::default()).unwrap();
        assert_eq!(report.order_status, OrderStatus::Filled);
        assert_eq!(report.filled_qty, Quantity::new(10.0, 0));
        assert_eq!(report.avg_px, Some(dec!(300.0050)));
    }

    #[rstest]
    fn test_partially_filled_keeps_both_quantities() {
        let order = order_with_status("partially_filled");
        let report = parse_order_status_report(&order, account(), 4, UnixNanos::default()).unwrap();
        assert_eq!(report.order_status, OrderStatus::PartiallyFilled);
        assert_eq!(report.quantity, Quantity::new(100.0, 0));
        assert_eq!(report.filled_qty, Quantity::new(40.0, 0));
    }

    #[rstest]
    fn test_replaced_order_is_refused_rather_than_reported_as_canceled() {
        let order = order_with_status("replaced");
        let err =
            parse_order_status_report(&order, account(), 4, UnixNanos::default()).unwrap_err();
        assert!(err.to_string().contains("replaced by"), "{err}");
    }

    #[rstest]
    fn test_stop_limit_order_carries_trigger_price() {
        let order = order_with_status("held");
        let report = parse_order_status_report(&order, account(), 4, UnixNanos::default()).unwrap();
        assert_eq!(report.order_type, OrderType::StopLimit);
        assert_eq!(report.trigger_price, Some(Price::new(295.00, 4)));
    }

    #[rstest]
    fn test_market_order_has_no_limit_price() {
        let order = order_with_status("accepted");
        let report = parse_order_status_report(&order, account(), 4, UnixNanos::default()).unwrap();
        assert_eq!(report.order_type, OrderType::Market);
        assert!(report.price.is_none());
    }

    #[rstest]
    fn test_notional_order_is_refused() {
        // This adapter trades whole shares; an order without a share quantity cannot be modelled.
        let mut order = order_with_status("new");
        order.qty = None;
        order.notional = Some("500.00".to_string());
        let err =
            parse_order_status_report(&order, account(), 4, UnixNanos::default()).unwrap_err();
        assert!(err.to_string().contains("notional"), "{err}");
    }

    #[rstest]
    fn test_prices_are_parsed_exactly() {
        // A decimal string must not lose digits on the way to `Price`.
        assert_eq!(
            parse_price("300.0050", "limit_price", 4).unwrap(),
            Price::new(300.005, 4)
        );
        assert_eq!(
            parse_decimal("10000.12345678", "cash").unwrap(),
            dec!(10000.12345678)
        );
    }

    #[rstest]
    #[case("abc")]
    #[case("")]
    fn test_malformed_decimals_are_rejected(#[case] raw: &str) {
        assert!(parse_decimal(raw, "field").is_err());
    }

    #[rstest]
    fn test_positions_fixture_deserializes() {
        assert_eq!(positions().len(), 2);
    }

    #[rstest]
    fn test_long_position_report() {
        let position = positions()
            .into_iter()
            .find(|p| p.symbol == "AAPL")
            .unwrap();
        let report =
            parse_position_status_report(&position, account(), UnixNanos::default()).unwrap();
        assert_eq!(report.position_side, PositionSideSpecified::Long);
        assert_eq!(report.quantity, Quantity::new(4.0, 0));
        assert_eq!(report.signed_decimal_qty, dec!(4));
        assert_eq!(report.avg_px_open, Some(dec!(200.125)));
    }

    #[rstest]
    fn test_short_position_reports_negative_signed_quantity() {
        let position = positions()
            .into_iter()
            .find(|p| p.symbol == "MSFT")
            .unwrap();
        let report =
            parse_position_status_report(&position, account(), UnixNanos::default()).unwrap();
        assert_eq!(report.position_side, PositionSideSpecified::Short);
        // The venue sends a negative `qty`; the report carries magnitude plus a signed quantity.
        assert_eq!(report.quantity, Quantity::new(2.0, 0));
        assert_eq!(report.signed_decimal_qty, dec!(-2));
    }

    #[rstest]
    fn test_short_detected_from_quantity_sign_alone() {
        let mut position = positions()
            .into_iter()
            .find(|p| p.symbol == "MSFT")
            .unwrap();
        position.side = "long".to_string(); // Disagrees with the negative quantity
        assert!(position.is_short());
    }

    #[rstest]
    fn test_zero_quantity_position_is_flat() {
        let mut position = positions().into_iter().next().unwrap();
        position.qty = "0".to_string();
        position.side = "long".to_string();
        let report =
            parse_position_status_report(&position, account(), UnixNanos::default()).unwrap();
        assert_eq!(report.position_side, PositionSideSpecified::Flat);
    }

    #[rstest]
    #[case(OrderStatus::Accepted, true)]
    #[case(OrderStatus::PartiallyFilled, true)]
    #[case(OrderStatus::PendingCancel, true)]
    #[case(OrderStatus::Filled, false)]
    #[case(OrderStatus::Canceled, false)]
    #[case(OrderStatus::Rejected, false)]
    fn test_open_status_classification(#[case] status: OrderStatus, #[case] expected: bool) {
        assert_eq!(is_open_status(status), expected);
    }
}
