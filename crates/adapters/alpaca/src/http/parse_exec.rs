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
    enums::{LiquiditySide, OrderStatus, PositionSideSpecified},
    identifiers::{AccountId, ClientOrderId, TradeId, VenueOrderId},
    reports::{FillReport, OrderStatusReport, PositionStatusReport},
    types::{Currency, Money, Price, Quantity},
};
use rust_decimal::Decimal;

use crate::{
    http::{
        models::{AlpacaFillActivity, AlpacaOrder, AlpacaPosition},
        parse::{instrument_id, parse_rfc3339_nanos},
    },
    websocket::messages::TradeUpdate,
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

/// Converts a fill-bearing trade update into a Nautilus fill report.
///
/// # Commission
///
/// Alpaca reports no per-fill commission on this stream, and US equity trading there is
/// commission-free, so zero is recorded. Inventing a figure would corrupt realised PnL.
///
/// # Liquidity
///
/// The venue does not say whether the fill took or made liquidity, so it is left unspecified
/// rather than guessed.
///
/// # Errors
///
/// Returns an error if the update carries no execution detail, or if any value cannot be
/// converted.
pub fn parse_fill_report(
    update: &TradeUpdate,
    account_id: AccountId,
    price_precision: u8,
    ts_init: UnixNanos,
) -> anyhow::Result<FillReport> {
    if !update.has_fill_detail() {
        anyhow::bail!(
            "Trade update '{}' for order {} carries no execution detail",
            update.event,
            update.order.id
        );
    }

    let raw_price = update
        .price
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Fill for order {} has no price", update.order.id))?;
    let raw_qty = update
        .qty
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Fill for order {} has no quantity", update.order.id))?;

    // Without an execution identifier a redelivered fill after a reconnect cannot be told from a
    // new one, and would be counted twice.
    let trade_id = update
        .execution_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Fill for order {} has no execution ID", update.order.id))?;

    let ts_event = match update.timestamp.as_deref() {
        Some(raw) => parse_rfc3339_nanos(raw)?,
        None => ts_init,
    };

    let currency = Currency::USD();

    Ok(FillReport::new(
        account_id,
        instrument_id(&update.order.symbol),
        VenueOrderId::new(update.order.id.as_str()),
        TradeId::new(trade_id),
        update.order.side.to_nautilus()?,
        parse_quantity(raw_qty, "qty")?,
        parse_price(raw_price, "price", price_precision)?,
        Money::new(0.0, currency),
        LiquiditySide::NoLiquiditySide,
        Some(ClientOrderId::new(update.order.client_order_id.as_str())),
        None,
        ts_event,
        ts_init,
        None,
    ))
}

/// Extracts the execution identifier from an activity ID.
///
/// The venue formats it as `<sequence>::<uuid>`, 55 characters against the 36 a `TradeId` holds.
/// The half after the separator is taken: across a sample of live activities it was invariably a
/// 36-character UUID, unique per fill, which is both what fits and the same shape the trading
/// event stream reports as `execution_id`.
///
/// The two are the same value, confirmed on 2026-08-20 by observing one execution on both paths:
/// the stream reported `c27c5376-c4eb-414c-ba5a-4fb21d0b4c87` and the activity feed
/// `20260820050934216::c27c5376-c4eb-414c-ba5a-4fb21d0b4c87`. A fill recovered here and the same
/// fill seen live therefore resolve to one trade identifier, which is what lets the engine
/// de-duplicate them. `examples/execution_id_check.rs` re-runs that check.
///
/// Note that only a *fill* event carries the execution identifier this matches. Lifecycle events
/// carry an `execution_id` too — `new` has one, and it is unrelated — so anything comparing these
/// has to gate on `has_fill_detail` first, as `parse_fill_report` does.
///
/// # Errors
///
/// Returns an error when no part of the identifier fits, rather than truncating: a truncated
/// identifier could collide with another fill.
pub fn extract_execution_id(activity_id: &str) -> anyhow::Result<&str> {
    const MAX_TRADE_ID_LEN: usize = 36;

    let candidate = activity_id.rsplit("::").next().unwrap_or(activity_id);

    if candidate.len() <= MAX_TRADE_ID_LEN {
        return Ok(candidate);
    }
    if activity_id.len() <= MAX_TRADE_ID_LEN {
        return Ok(activity_id);
    }

    anyhow::bail!(
        "Activity ID '{activity_id}' has no part short enough for a trade ID (max \
         {MAX_TRADE_ID_LEN} characters)"
    )
}

/// Converts a fill activity into a Nautilus fill report.
///
/// The activity feed is how fills are recovered at startup: the trading event stream reports them
/// as they happen but cannot be replayed.
///
/// # Errors
///
/// Returns an error if any value cannot be converted.
pub fn parse_fill_activity_report(
    activity: &AlpacaFillActivity,
    account_id: AccountId,
    price_precision: u8,
    ts_init: UnixNanos,
) -> anyhow::Result<FillReport> {
    Ok(FillReport::new(
        account_id,
        instrument_id(&activity.symbol),
        VenueOrderId::new(activity.order_id.as_str()),
        TradeId::new(extract_execution_id(&activity.id)?),
        activity.side.to_nautilus()?,
        // `qty` is this execution's own quantity. `cum_qty` is the order's running total, so
        // reading it would count every earlier execution again on each subsequent fill.
        parse_quantity(&activity.qty, "qty")?,
        parse_price(&activity.price, "price", price_precision)?,
        // The venue reports no commission on this feed and US equity trading there is
        // commission-free; inventing a figure would corrupt realised PnL.
        Money::new(0.0, Currency::USD()),
        LiquiditySide::NoLiquiditySide,
        None, // client_order_id: not carried by the activity
        None,
        parse_rfc3339_nanos(&activity.transaction_time)?,
        ts_init,
        None,
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
    use crate::common::order_enums::AlpacaOrderSide;

    const ORDERS_JSON: &str = include_str!("../../test_data/http_orders.json");
    const POSITIONS_JSON: &str = include_str!("../../test_data/http_positions.json");
    const ACTIVITIES_JSON: &str = include_str!("../../test_data/http_activities_fills.json");

    fn orders() -> Vec<AlpacaOrder> {
        serde_json::from_str(ORDERS_JSON).unwrap()
    }

    fn activities() -> Vec<AlpacaFillActivity> {
        serde_json::from_str(ACTIVITIES_JSON).unwrap()
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

    fn fill_update(execution_id: Option<&str>, price: Option<&str>) -> TradeUpdate {
        let exec = execution_id.map_or("null".to_string(), |id| format!("\"{id}\""));
        let px = price.map_or("null".to_string(), |p| format!("\"{p}\""));
        let json = format!(
            r#"{{
                "event": "fill",
                "timestamp": "2026-08-14T14:30:05.123456Z",
                "price": {px},
                "qty": "10",
                "execution_id": {exec},
                "order": {{
                    "id": "o-1", "client_order_id": "c-1", "symbol": "AAPL",
                    "type": "limit", "side": "buy", "time_in_force": "day",
                    "status": "filled", "qty": "10", "filled_qty": "10"
                }}
            }}"#
        );
        serde_json::from_str(&json).unwrap()
    }

    #[rstest]
    fn test_parse_fill_report() {
        let update = fill_update(Some("e-1"), Some("300.005"));
        let report = parse_fill_report(&update, account(), 4, UnixNanos::from(9)).unwrap();

        assert_eq!(report.instrument_id.to_string(), "AAPL.ALPACA");
        assert_eq!(report.venue_order_id, VenueOrderId::new("o-1"));
        assert_eq!(report.trade_id, TradeId::new("e-1"));
        assert_eq!(report.last_qty, Quantity::new(10.0, 0));
        assert_eq!(report.last_px, Price::new(300.005, 4));
        assert_eq!(report.ts_init, UnixNanos::from(9));
    }

    #[rstest]
    fn test_fill_report_records_zero_commission() {
        // US equity trading on this venue is commission-free and the stream reports none;
        // inventing a figure would corrupt realised PnL.
        let report = parse_fill_report(
            &fill_update(Some("e-1"), Some("300.00")),
            account(),
            4,
            UnixNanos::default(),
        )
        .unwrap();
        assert_eq!(report.commission, Money::new(0.0, Currency::USD()));
        assert_eq!(report.liquidity_side, LiquiditySide::NoLiquiditySide);
    }

    #[rstest]
    fn test_fill_without_execution_id_is_refused() {
        // Without it a redelivered fill after a reconnect cannot be distinguished from a new one.
        let update = fill_update(None, Some("300.00"));
        let err = parse_fill_report(&update, account(), 4, UnixNanos::default()).unwrap_err();
        assert!(err.to_string().contains("execution ID"), "{err}");
    }

    #[rstest]
    fn test_fill_without_price_is_refused() {
        let update = fill_update(Some("e-1"), None);
        assert!(parse_fill_report(&update, account(), 4, UnixNanos::default()).is_err());
    }

    #[rstest]
    fn test_non_fill_update_is_refused() {
        let json = r#"{
            "event": "accepted",
            "order": {
                "id": "o-1", "client_order_id": "c-1", "symbol": "AAPL", "type": "limit",
                "side": "buy", "time_in_force": "day", "status": "accepted", "qty": "10"
            }
        }"#;
        let update: TradeUpdate = serde_json::from_str(json).unwrap();
        let err = parse_fill_report(&update, account(), 4, UnixNanos::default()).unwrap_err();
        assert!(err.to_string().contains("no execution detail"), "{err}");
    }

    fn fill_activity() -> AlpacaFillActivity {
        // Shaped as the venue returns it from /v2/account/activities, with the identifiers
        // replaced but their exact lengths and format kept.
        let json = r#"{
            "id": "20260624211230709::1444f6ad-0000-4000-8000-000000000001",
            "activity_type": "FILL",
            "transaction_time": "2026-06-25T01:12:30.709802Z",
            "type": "fill",
            "price": "736.03",
            "qty": "1",
            "side": "buy",
            "symbol": "SPY",
            "leaves_qty": "0",
            "order_id": "0000aaaa-0000-4000-8000-000000000001",
            "cum_qty": "1",
            "order_status": "filled"
        }"#;
        serde_json::from_str(json).unwrap()
    }

    #[rstest]
    fn test_parse_fill_activity_report() {
        let report =
            parse_fill_activity_report(&fill_activity(), account(), 4, UnixNanos::from(5)).unwrap();

        assert_eq!(report.instrument_id.to_string(), "SPY.ALPACA");
        assert_eq!(
            report.venue_order_id,
            VenueOrderId::new("0000aaaa-0000-4000-8000-000000000001")
        );
        assert_eq!(report.last_qty, Quantity::new(1.0, 0));
        assert_eq!(report.last_px, Price::new(736.03, 4));
        assert_eq!(report.ts_init, UnixNanos::from(5));
    }

    #[rstest]
    fn test_trade_id_is_the_uuid_half_of_the_activity_id() {
        // The full activity ID is 55 characters and a TradeId holds 36. The UUID half is also
        // what the trading event stream reports as `execution_id`, so a fill seen live and again
        // here resolves to one trade identifier.
        let report =
            parse_fill_activity_report(&fill_activity(), account(), 4, UnixNanos::default())
                .unwrap();
        assert_eq!(
            report.trade_id,
            TradeId::new("1444f6ad-0000-4000-8000-000000000001")
        );
    }

    #[rstest]
    #[case(
        "20260624211230709::1444f6ad-0000-4000-8000-000000000001",
        "1444f6ad-0000-4000-8000-000000000001"
    )]
    #[case(
        "1444f6ad-0000-4000-8000-000000000001",
        "1444f6ad-0000-4000-8000-000000000001"
    )]
    #[case("short-id", "short-id")]
    fn test_execution_id_extraction(#[case] activity_id: &str, #[case] expected: &str) {
        assert_eq!(extract_execution_id(activity_id).unwrap(), expected);
    }

    #[rstest]
    fn test_overlong_execution_id_is_refused_not_truncated() {
        // Truncating could collide with another fill and double-count it.
        let too_long = "x".repeat(40);
        assert!(extract_execution_id(&too_long).is_err());
    }

    #[rstest]
    fn test_fill_activity_records_zero_commission() {
        let report =
            parse_fill_activity_report(&fill_activity(), account(), 4, UnixNanos::default())
                .unwrap();
        assert_eq!(report.commission, Money::new(0.0, Currency::USD()));
    }

    #[rstest]
    fn test_fill_activity_timestamp_is_the_transaction_time() {
        let report =
            parse_fill_activity_report(&fill_activity(), account(), 4, UnixNanos::default())
                .unwrap();
        let expected = parse_rfc3339_nanos("2026-06-25T01:12:30.709802Z").unwrap();
        assert_eq!(report.ts_event, expected);
    }

    #[rstest]
    fn test_activities_fixture_parses_every_record() {
        // Captured from the live account, so the shapes are the venue's own. Every record has to
        // convert: one that does not is a fill dropped from reconciliation, and a dropped fill is
        // a position the engine and the venue disagree about.
        let activities = activities();
        assert_eq!(activities.len(), 6);
        for activity in &activities {
            parse_fill_activity_report(activity, account(), 4, UnixNanos::from(1))
                .unwrap_or_else(|e| panic!("failed to parse {}: {e}", activity.id));
        }
    }

    #[rstest]
    fn test_short_sale_activity_reports_a_sell() {
        // The venue reports `sell_short` on the fill that opens a short. It is rare — one record
        // in several hundred on the account this fixture came from — and refusing it left the
        // engine flat on an instrument the venue reported short.
        let activity = activities()
            .into_iter()
            .find(|a| a.side == AlpacaOrderSide::SellShort)
            .expect("fixture has no short sale");

        let report = parse_fill_activity_report(&activity, account(), 4, UnixNanos::from(1))
            .expect("a short sale must convert");
        assert_eq!(report.order_side, OrderSide::Sell);
        assert_eq!(report.last_qty, Quantity::new(1.0, 0));
    }

    #[rstest]
    fn test_partial_fills_report_the_execution_quantity_not_the_running_total() {
        // A partially filled order arrives as several records. Reading `cum_qty` instead of `qty`
        // would count the earlier executions again on every subsequent one.
        let partials: Vec<_> = activities()
            .into_iter()
            .filter(|a| a.fill_type.as_deref() == Some("partial_fill"))
            .collect();
        assert!(partials.len() >= 2);

        for activity in &partials {
            let report = parse_fill_activity_report(activity, account(), 4, UnixNanos::from(1))
                .unwrap_or_else(|e| panic!("failed to parse {}: {e}", activity.id));
            let expected = parse_quantity(&activity.qty, "qty").unwrap();
            assert_eq!(report.last_qty, expected);
        }

        // The last record's running total exceeds its own quantity, so the two are distinguishable
        // and this test would fail if the wrong field were read.
        let last = partials.last().unwrap();
        assert_ne!(last.qty, *last.cum_qty.as_ref().unwrap());
    }

    #[rstest]
    fn test_activity_trade_ids_are_unique_across_the_fixture() {
        // Fills de-duplicate on trade identifier, so a collision would silently merge two
        // executions into one.
        let ids: std::collections::HashSet<_> = activities()
            .iter()
            .map(|a| extract_execution_id(&a.id).unwrap().to_string())
            .collect();
        assert_eq!(ids.len(), 6);
    }
}
