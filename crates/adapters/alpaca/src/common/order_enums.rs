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

//! Order enumerations for the Alpaca adapter, and their mapping onto the Nautilus domain.
//!
//! Every venue enum carries an `Unknown` variant so an unrecognised value deserializes instead of
//! failing the whole payload. Mapping an `Unknown` to a Nautilus type is a different matter and is
//! always an error: guessing a side, a type, or a status would misreport live orders.

use nautilus_model::enums::{OrderSide, OrderStatus, OrderType, TimeInForce};
use serde::{Deserialize, Serialize};
use strum::{AsRefStr, Display, EnumString};

/// Order side as sent by the venue.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString, AsRefStr,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum AlpacaOrderSide {
    Buy,
    Sell,
    /// Reported on a fill that opened a short position.
    ///
    /// This appears on the activity feed rather than on submissions: an equity order is sent as
    /// `sell`, and the venue decides from the position held whether it closes or goes short.
    SellShort,
    #[serde(other)]
    Unknown,
}

impl AlpacaOrderSide {
    /// Converts to the Nautilus order side.
    ///
    /// # Errors
    ///
    /// Returns an error for an unrecognised side.
    pub fn to_nautilus(self) -> anyhow::Result<OrderSide> {
        match self {
            Self::Buy => Ok(OrderSide::Buy),
            // Nautilus carries direction in the position rather than the side, so a short sale is
            // a sell. Refusing it instead would drop the fill that opened the short, leaving the
            // engine's position disagreeing with the venue's.
            Self::Sell | Self::SellShort => Ok(OrderSide::Sell),
            Self::Unknown => anyhow::bail!("Unrecognised Alpaca order side"),
        }
    }

    /// Converts from the Nautilus order side.
    ///
    /// # Errors
    ///
    /// Returns an error if the side is not buy or sell.
    pub fn from_nautilus(side: OrderSide) -> anyhow::Result<Self> {
        match side {
            OrderSide::Buy => Ok(Self::Buy),
            OrderSide::Sell => Ok(Self::Sell),
            OrderSide::NoOrderSide => anyhow::bail!("Cannot submit an order with no side"),
        }
    }
}

/// Order type as sent by the venue.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString, AsRefStr,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum AlpacaOrderType {
    Market,
    Limit,
    Stop,
    StopLimit,
    TrailingStop,
    #[serde(other)]
    Unknown,
}

impl AlpacaOrderType {
    /// Converts to the Nautilus order type.
    ///
    /// # Errors
    ///
    /// Returns an error for an unrecognised type.
    pub fn to_nautilus(self) -> anyhow::Result<OrderType> {
        match self {
            Self::Market => Ok(OrderType::Market),
            Self::Limit => Ok(OrderType::Limit),
            Self::Stop => Ok(OrderType::StopMarket),
            Self::StopLimit => Ok(OrderType::StopLimit),
            Self::TrailingStop => Ok(OrderType::TrailingStopMarket),
            Self::Unknown => anyhow::bail!("Unrecognised Alpaca order type"),
        }
    }

    /// Converts from the Nautilus order type.
    ///
    /// # Errors
    ///
    /// Returns an error for an order type the venue does not offer.
    pub fn from_nautilus(order_type: OrderType) -> anyhow::Result<Self> {
        match order_type {
            OrderType::Market => Ok(Self::Market),
            OrderType::Limit => Ok(Self::Limit),
            OrderType::StopMarket => Ok(Self::Stop),
            OrderType::StopLimit => Ok(Self::StopLimit),
            OrderType::TrailingStopMarket => Ok(Self::TrailingStop),
            other => anyhow::bail!("Alpaca does not support {other:?} orders"),
        }
    }
}

/// Time in force as sent by the venue.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString, AsRefStr,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum AlpacaTimeInForce {
    Day,
    Gtc,
    /// Market on open / limit on open.
    Opg,
    /// Market on close / limit on close.
    Cls,
    Ioc,
    Fok,
    #[serde(other)]
    Unknown,
}

impl AlpacaTimeInForce {
    /// Converts to the Nautilus time in force.
    ///
    /// # Errors
    ///
    /// Returns an error for an unrecognised value.
    pub fn to_nautilus(self) -> anyhow::Result<TimeInForce> {
        match self {
            Self::Day => Ok(TimeInForce::Day),
            Self::Gtc => Ok(TimeInForce::Gtc),
            Self::Opg => Ok(TimeInForce::AtTheOpen),
            Self::Cls => Ok(TimeInForce::AtTheClose),
            Self::Ioc => Ok(TimeInForce::Ioc),
            Self::Fok => Ok(TimeInForce::Fok),
            Self::Unknown => anyhow::bail!("Unrecognised Alpaca time in force"),
        }
    }

    /// Converts from the Nautilus time in force.
    ///
    /// # Errors
    ///
    /// Returns an error for a value the venue does not offer. `GTD` has no venue equivalent:
    /// Alpaca derives an expiry from the time in force rather than accepting one, so an explicit
    /// expiry cannot be honoured and is rejected instead of being silently widened to `GTC`.
    pub fn from_nautilus(time_in_force: TimeInForce) -> anyhow::Result<Self> {
        match time_in_force {
            TimeInForce::Day => Ok(Self::Day),
            TimeInForce::Gtc => Ok(Self::Gtc),
            TimeInForce::AtTheOpen => Ok(Self::Opg),
            TimeInForce::AtTheClose => Ok(Self::Cls),
            TimeInForce::Ioc => Ok(Self::Ioc),
            TimeInForce::Fok => Ok(Self::Fok),
            TimeInForce::Gtd => {
                anyhow::bail!("Alpaca has no GTD time in force; use GTC or DAY instead")
            }
        }
    }
}

/// Order status as sent by the venue.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString, AsRefStr,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum AlpacaOrderStatus {
    /// Accepted by the venue and working.
    New,
    /// Received but not yet working.
    PendingNew,
    Accepted,
    AcceptedForBidding,
    PartiallyFilled,
    Filled,
    DoneForDay,
    Canceled,
    Expired,
    Rejected,
    Suspended,
    PendingCancel,
    PendingReplace,
    PendingReview,
    /// Superseded by a replacement order.
    Replaced,
    Stopped,
    Calculated,
    /// Held pending the market session.
    Held,
    #[serde(other)]
    Unknown,
}

impl AlpacaOrderStatus {
    /// Converts to the Nautilus order status.
    ///
    /// # Errors
    ///
    /// Returns an error for an unrecognised status, and for [`Self::Replaced`], which has no
    /// Nautilus equivalent — see [`Self::is_superseded`].
    pub fn to_nautilus(self) -> anyhow::Result<OrderStatus> {
        match self {
            Self::PendingNew => Ok(OrderStatus::Submitted),
            Self::New
            | Self::Accepted
            | Self::AcceptedForBidding
            | Self::Held
            | Self::Calculated
            | Self::Stopped => Ok(OrderStatus::Accepted),
            Self::PartiallyFilled => Ok(OrderStatus::PartiallyFilled),
            Self::Filled => Ok(OrderStatus::Filled),
            // The venue closes out working orders at the end of the session; the order is no
            // longer live, which is what `Expired` conveys.
            Self::DoneForDay | Self::Expired => Ok(OrderStatus::Expired),
            Self::Canceled => Ok(OrderStatus::Canceled),
            Self::Rejected | Self::Suspended => Ok(OrderStatus::Rejected),
            Self::PendingCancel => Ok(OrderStatus::PendingCancel),
            Self::PendingReplace | Self::PendingReview => Ok(OrderStatus::PendingUpdate),
            Self::Replaced => anyhow::bail!(
                "Alpaca 'replaced' has no Nautilus equivalent; the order continues under its \
                 replacement identifier"
            ),
            Self::Unknown => anyhow::bail!("Unrecognised Alpaca order status"),
        }
    }

    /// Returns true when the order was superseded by a replacement rather than ending.
    ///
    /// Alpaca implements an amendment as a new order: the original moves to `replaced` and a fresh
    /// venue identifier takes over. Reporting that as a cancellation would tell the engine an
    /// order is dead while it is still working under another identifier.
    #[must_use]
    pub const fn is_superseded(self) -> bool {
        matches!(self, Self::Replaced)
    }

    /// Returns true when no further activity is expected on the order.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Filled
                | Self::Canceled
                | Self::Expired
                | Self::Rejected
                | Self::DoneForDay
                | Self::Replaced
        )
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case("buy", AlpacaOrderSide::Buy)]
    #[case("sell", AlpacaOrderSide::Sell)]
    #[case("sell_short", AlpacaOrderSide::SellShort)]
    fn test_side_deserializes(#[case] raw: &str, #[case] expected: AlpacaOrderSide) {
        let json = format!("\"{raw}\"");
        assert_eq!(
            serde_json::from_str::<AlpacaOrderSide>(&json).unwrap(),
            expected
        );
    }

    #[rstest]
    fn test_short_sale_converts_to_a_sell() {
        // The activity feed reports the fill that opens a short as `sell_short`. Nautilus has no
        // such side, and refusing it would drop that fill from reconciliation, leaving the engine
        // flat on an instrument the venue reports as short.
        assert_eq!(
            AlpacaOrderSide::SellShort.to_nautilus().unwrap(),
            OrderSide::Sell
        );
    }

    #[rstest]
    fn test_a_nautilus_sell_is_submitted_as_a_plain_sell() {
        // Submissions never carry `sell_short`: the venue derives it from the position held, and
        // sending it would be rejected.
        assert_eq!(
            AlpacaOrderSide::from_nautilus(OrderSide::Sell).unwrap(),
            AlpacaOrderSide::Sell
        );
    }

    #[rstest]
    #[case::side("\"sideways\"")]
    fn test_unknown_side_deserializes_rather_than_failing(#[case] json: &str) {
        assert_eq!(
            serde_json::from_str::<AlpacaOrderSide>(json).unwrap(),
            AlpacaOrderSide::Unknown
        );
    }

    #[rstest]
    fn test_unknown_side_will_not_convert() {
        assert!(AlpacaOrderSide::Unknown.to_nautilus().is_err());
    }

    #[rstest]
    #[case(AlpacaOrderSide::Buy, OrderSide::Buy)]
    #[case(AlpacaOrderSide::Sell, OrderSide::Sell)]
    fn test_side_round_trips(#[case] venue: AlpacaOrderSide, #[case] nautilus: OrderSide) {
        assert_eq!(venue.to_nautilus().unwrap(), nautilus);
        assert_eq!(AlpacaOrderSide::from_nautilus(nautilus).unwrap(), venue);
    }

    #[rstest]
    fn test_no_order_side_is_rejected() {
        assert!(AlpacaOrderSide::from_nautilus(OrderSide::NoOrderSide).is_err());
    }

    #[rstest]
    #[case(AlpacaOrderType::Market, OrderType::Market)]
    #[case(AlpacaOrderType::Limit, OrderType::Limit)]
    #[case(AlpacaOrderType::Stop, OrderType::StopMarket)]
    #[case(AlpacaOrderType::StopLimit, OrderType::StopLimit)]
    #[case(AlpacaOrderType::TrailingStop, OrderType::TrailingStopMarket)]
    fn test_order_type_round_trips(#[case] venue: AlpacaOrderType, #[case] nautilus: OrderType) {
        assert_eq!(venue.to_nautilus().unwrap(), nautilus);
        assert_eq!(AlpacaOrderType::from_nautilus(nautilus).unwrap(), venue);
    }

    #[rstest]
    #[case(OrderType::MarketIfTouched)]
    #[case(OrderType::LimitIfTouched)]
    #[case(OrderType::MarketToLimit)]
    #[case(OrderType::TrailingStopLimit)]
    fn test_unsupported_order_types_are_rejected(#[case] order_type: OrderType) {
        assert!(AlpacaOrderType::from_nautilus(order_type).is_err());
    }

    #[rstest]
    #[case("stop_limit", AlpacaOrderType::StopLimit)]
    #[case("trailing_stop", AlpacaOrderType::TrailingStop)]
    fn test_snake_case_order_types_deserialize(
        #[case] raw: &str,
        #[case] expected: AlpacaOrderType,
    ) {
        let json = format!("\"{raw}\"");
        assert_eq!(
            serde_json::from_str::<AlpacaOrderType>(&json).unwrap(),
            expected
        );
    }

    #[rstest]
    #[case(AlpacaTimeInForce::Day, TimeInForce::Day)]
    #[case(AlpacaTimeInForce::Gtc, TimeInForce::Gtc)]
    #[case(AlpacaTimeInForce::Opg, TimeInForce::AtTheOpen)]
    #[case(AlpacaTimeInForce::Cls, TimeInForce::AtTheClose)]
    #[case(AlpacaTimeInForce::Ioc, TimeInForce::Ioc)]
    #[case(AlpacaTimeInForce::Fok, TimeInForce::Fok)]
    fn test_time_in_force_round_trips(
        #[case] venue: AlpacaTimeInForce,
        #[case] nautilus: TimeInForce,
    ) {
        assert_eq!(venue.to_nautilus().unwrap(), nautilus);
        assert_eq!(AlpacaTimeInForce::from_nautilus(nautilus).unwrap(), venue);
    }

    #[rstest]
    fn test_gtd_is_rejected_rather_than_widened_to_gtc() {
        // Silently substituting GTC would leave an order working long past its intended expiry.
        let err = AlpacaTimeInForce::from_nautilus(TimeInForce::Gtd).unwrap_err();
        assert!(err.to_string().contains("GTD"), "{err}");
    }

    #[rstest]
    #[case(AlpacaOrderStatus::New, OrderStatus::Accepted)]
    #[case(AlpacaOrderStatus::PendingNew, OrderStatus::Submitted)]
    #[case(AlpacaOrderStatus::Accepted, OrderStatus::Accepted)]
    #[case(AlpacaOrderStatus::Held, OrderStatus::Accepted)]
    #[case(AlpacaOrderStatus::PartiallyFilled, OrderStatus::PartiallyFilled)]
    #[case(AlpacaOrderStatus::Filled, OrderStatus::Filled)]
    #[case(AlpacaOrderStatus::Canceled, OrderStatus::Canceled)]
    #[case(AlpacaOrderStatus::Expired, OrderStatus::Expired)]
    #[case(AlpacaOrderStatus::DoneForDay, OrderStatus::Expired)]
    #[case(AlpacaOrderStatus::Rejected, OrderStatus::Rejected)]
    #[case(AlpacaOrderStatus::PendingCancel, OrderStatus::PendingCancel)]
    #[case(AlpacaOrderStatus::PendingReplace, OrderStatus::PendingUpdate)]
    fn test_status_mapping(#[case] venue: AlpacaOrderStatus, #[case] expected: OrderStatus) {
        assert_eq!(venue.to_nautilus().unwrap(), expected);
    }

    #[rstest]
    fn test_replaced_is_superseded_not_canceled() {
        // Mapping this to `Canceled` would tell the engine the order is dead while it is still
        // working under the replacement identifier.
        assert!(AlpacaOrderStatus::Replaced.is_superseded());
        assert!(AlpacaOrderStatus::Replaced.to_nautilus().is_err());
        assert!(!AlpacaOrderStatus::Canceled.is_superseded());
    }

    #[rstest]
    #[case(AlpacaOrderStatus::Filled, true)]
    #[case(AlpacaOrderStatus::Canceled, true)]
    #[case(AlpacaOrderStatus::Expired, true)]
    #[case(AlpacaOrderStatus::Rejected, true)]
    #[case(AlpacaOrderStatus::Replaced, true)]
    #[case(AlpacaOrderStatus::New, false)]
    #[case(AlpacaOrderStatus::PartiallyFilled, false)]
    #[case(AlpacaOrderStatus::Held, false)]
    fn test_terminal_classification(#[case] status: AlpacaOrderStatus, #[case] expected: bool) {
        assert_eq!(status.is_terminal(), expected);
    }

    #[rstest]
    fn test_unknown_status_deserializes_but_will_not_convert() {
        let status: AlpacaOrderStatus = serde_json::from_str("\"some_new_status\"").unwrap();
        assert_eq!(status, AlpacaOrderStatus::Unknown);
        assert!(status.to_nautilus().is_err());
    }
}
