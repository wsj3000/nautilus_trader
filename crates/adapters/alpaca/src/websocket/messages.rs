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

//! Protocol messages for the Alpaca trading event stream.
//!
//! This is the account stream on the trading host, not the market data stream. It carries order
//! lifecycle events and fill detail, and its connection limit is separate from the market data
//! one.
//!
//! Every frame is an envelope: `{"stream": "...", "data": {...}}`.

use std::str::FromStr;

use serde::{Deserialize, Serialize};
use strum::{AsRefStr, Display, EnumString};

use crate::http::models::AlpacaOrder;

/// Stream name carrying order lifecycle events.
pub const STREAM_TRADE_UPDATES: &str = "trade_updates";
/// Stream name of the authentication response.
pub const STREAM_AUTHORIZATION: &str = "authorization";
/// Stream name of the subscription response.
pub const STREAM_LISTENING: &str = "listening";

/// Outbound authentication frame.
#[derive(Debug, Clone, Serialize)]
pub struct AuthMessage {
    /// Always `auth`.
    pub action: &'static str,
    /// API key ID.
    pub key: String,
    /// API secret key.
    pub secret: String,
}

impl AuthMessage {
    /// Creates an authentication frame.
    #[must_use]
    pub fn new(key: impl Into<String>, secret: impl Into<String>) -> Self {
        Self {
            action: "auth",
            key: key.into(),
            secret: secret.into(),
        }
    }
}

/// Stream list carried by a subscription frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamList {
    /// Stream names.
    pub streams: Vec<String>,
}

/// Outbound subscription frame.
#[derive(Debug, Clone, Serialize)]
pub struct ListenMessage {
    /// Always `listen`.
    pub action: &'static str,
    /// Streams to subscribe to.
    pub data: StreamList,
}

impl ListenMessage {
    /// Creates a subscription frame for the trade update stream.
    #[must_use]
    pub fn trade_updates() -> Self {
        Self {
            action: "listen",
            data: StreamList {
                streams: vec![STREAM_TRADE_UPDATES.to_string()],
            },
        }
    }
}

/// Inbound frame envelope.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct StreamEnvelope {
    /// Stream name.
    pub stream: String,
    /// Stream-specific payload.
    pub data: serde_json::Value,
}

/// Payload of an `authorization` frame.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AuthorizationData {
    /// `authorized` on success.
    pub status: String,
    /// The action being answered.
    #[serde(default)]
    pub action: Option<String>,
}

impl AuthorizationData {
    /// Returns true when the connection was authorized.
    #[must_use]
    pub fn is_authorized(&self) -> bool {
        self.status.eq_ignore_ascii_case("authorized")
    }
}

/// Order lifecycle event carried on the trade update stream.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString, AsRefStr,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum AlpacaTradeEvent {
    /// Order accepted and working.
    New,
    /// Order received by the venue but not yet routed.
    ///
    /// Observed on a submission made while the market was closed.
    Accepted,
    /// Order accepted for bidding.
    AcceptedForBidding,
    /// Order held pending the market session.
    Held,
    /// Order under venue review.
    PendingReview,
    /// Order fully filled.
    Fill,
    /// Order partially filled.
    PartialFill,
    Canceled,
    Expired,
    DoneForDay,
    /// Order replaced by an amendment.
    Replaced,
    Rejected,
    PendingNew,
    PendingCancel,
    PendingReplace,
    Stopped,
    Calculated,
    Suspended,
    /// An amendment was refused; the original order is unchanged.
    OrderReplaceRejected,
    /// A cancellation was refused; the order is still working.
    OrderCancelRejected,
    #[serde(other)]
    Unknown,
}

impl AlpacaTradeEvent {
    /// Returns true when the event carries execution detail.
    #[must_use]
    pub const fn is_fill(self) -> bool {
        matches!(self, Self::Fill | Self::PartialFill)
    }

    /// Returns true when the event means the order is working or on its way there.
    ///
    /// The pending states are included: the venue has not finished with the order, so treating
    /// them as terminal would drop an order that is still live.
    #[must_use]
    pub const fn is_working(self) -> bool {
        matches!(
            self,
            Self::New
                | Self::Accepted
                | Self::AcceptedForBidding
                | Self::Held
                | Self::PendingNew
                | Self::PendingReview
        )
    }

    /// Returns true when the event means an amendment or cancellation was refused.
    ///
    /// These leave the order working, so treating them as terminal would lose track of it.
    #[must_use]
    pub const fn is_request_rejection(self) -> bool {
        matches!(self, Self::OrderReplaceRejected | Self::OrderCancelRejected)
    }
}

/// Payload of a `trade_updates` frame.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TradeUpdate {
    /// What happened, as the venue named it.
    ///
    /// Kept as the raw string so an event this adapter does not model can still be logged by
    /// name. A bare `Unknown` variant would record that something happened without saying what,
    /// which is not enough to diagnose a missed order event. Read it through
    /// [`TradeUpdate::event_kind`].
    pub event: String,
    /// When it happened (RFC 3339).
    #[serde(default)]
    pub timestamp: Option<String>,
    /// The order in its state after the event.
    pub order: AlpacaOrder,
    /// Execution price, present on fills.
    #[serde(default)]
    pub price: Option<String>,
    /// Executed quantity, present on fills.
    #[serde(default)]
    pub qty: Option<String>,
    /// Resulting position size, present on fills.
    #[serde(default)]
    pub position_qty: Option<String>,
    /// Venue execution identifier, present on fills.
    ///
    /// This is what makes a fill de-duplicable across a reconnect: the same execution redelivered
    /// carries the same identifier.
    #[serde(default)]
    pub execution_id: Option<String>,
}

impl TradeUpdate {
    /// Returns the typed event, or [`AlpacaTradeEvent::Unknown`] for one this adapter does not
    /// model.
    #[must_use]
    pub fn event_kind(&self) -> AlpacaTradeEvent {
        AlpacaTradeEvent::from_str(&self.event).unwrap_or(AlpacaTradeEvent::Unknown)
    }

    /// Returns true when the update carries enough detail to build a fill report.
    #[must_use]
    pub fn has_fill_detail(&self) -> bool {
        self.event_kind().is_fill() && self.price.is_some() && self.qty.is_some()
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn test_auth_message_serializes_to_the_venue_shape() {
        let json = serde_json::to_value(AuthMessage::new("key", "secret")).unwrap();
        assert_eq!(json["action"], "auth");
        assert_eq!(json["key"], "key");
        assert_eq!(json["secret"], "secret");
    }

    #[rstest]
    fn test_listen_message_requests_trade_updates() {
        let json = serde_json::to_value(ListenMessage::trade_updates()).unwrap();
        assert_eq!(json["action"], "listen");
        assert_eq!(json["data"]["streams"][0], STREAM_TRADE_UPDATES);
    }

    #[rstest]
    fn test_authorization_frame_parses() {
        let json =
            r#"{"stream":"authorization","data":{"status":"authorized","action":"authenticate"}}"#;
        let envelope: StreamEnvelope = serde_json::from_str(json).unwrap();
        assert_eq!(envelope.stream, STREAM_AUTHORIZATION);

        let data: AuthorizationData = serde_json::from_value(envelope.data).unwrap();
        assert!(data.is_authorized());
    }

    #[rstest]
    fn test_unauthorized_is_detected() {
        let data: AuthorizationData = serde_json::from_str(r#"{"status":"unauthorized"}"#).unwrap();
        assert!(!data.is_authorized());
    }

    #[rstest]
    #[case("fill", AlpacaTradeEvent::Fill, true)]
    #[case("partial_fill", AlpacaTradeEvent::PartialFill, true)]
    #[case("new", AlpacaTradeEvent::New, false)]
    #[case("canceled", AlpacaTradeEvent::Canceled, false)]
    #[case("replaced", AlpacaTradeEvent::Replaced, false)]
    fn test_trade_event_parsing(
        #[case] raw: &str,
        #[case] expected: AlpacaTradeEvent,
        #[case] is_fill: bool,
    ) {
        let json = format!("\"{raw}\"");
        let event: AlpacaTradeEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, expected);
        assert_eq!(event.is_fill(), is_fill);
    }

    #[rstest]
    fn test_unknown_event_does_not_fail_parsing() {
        let event: AlpacaTradeEvent = serde_json::from_str("\"some_future_event\"").unwrap();
        assert_eq!(event, AlpacaTradeEvent::Unknown);
    }

    #[rstest]
    #[case(AlpacaTradeEvent::OrderReplaceRejected)]
    #[case(AlpacaTradeEvent::OrderCancelRejected)]
    fn test_request_rejections_are_distinguished(#[case] event: AlpacaTradeEvent) {
        // These refuse a request without ending the order, so they must not be read as terminal.
        assert!(event.is_request_rejection());
        assert!(!event.is_fill());
    }

    #[rstest]
    fn test_trade_update_with_fill_detail() {
        let json = r#"{
            "event": "fill",
            "timestamp": "2026-08-14T14:30:05.123456Z",
            "price": "300.005",
            "qty": "10",
            "position_qty": "10",
            "execution_id": "00000000-0000-4000-8000-00000000000e",
            "order": {
                "id": "00000000-0000-4000-8000-000000000001",
                "client_order_id": "c-1",
                "symbol": "AAPL",
                "type": "limit",
                "side": "buy",
                "time_in_force": "day",
                "status": "filled",
                "qty": "10",
                "filled_qty": "10"
            }
        }"#;
        let update: TradeUpdate = serde_json::from_str(json).unwrap();
        assert_eq!(update.event_kind(), AlpacaTradeEvent::Fill);
        assert!(update.has_fill_detail());
        assert_eq!(update.price.as_deref(), Some("300.005"));
        assert_eq!(update.order.symbol, "AAPL");
        assert_eq!(
            update.execution_id.as_deref(),
            Some("00000000-0000-4000-8000-00000000000e")
        );
    }

    #[rstest]
    fn test_non_fill_update_has_no_fill_detail() {
        let json = r#"{
            "event": "new",
            "order": {
                "id": "1", "client_order_id": "c-1", "symbol": "AAPL",
                "type": "limit", "side": "buy", "time_in_force": "day",
                "status": "new", "qty": "10"
            }
        }"#;
        let update: TradeUpdate = serde_json::from_str(json).unwrap();
        assert!(!update.has_fill_detail());
        assert!(update.execution_id.is_none());
    }

    #[rstest]
    fn test_fill_without_price_is_not_reportable() {
        // A fill missing its execution detail cannot become a fill report; treating it as one
        // would invent a price.
        let json = r#"{
            "event": "fill",
            "qty": "10",
            "order": {
                "id": "1", "client_order_id": "c-1", "symbol": "AAPL",
                "type": "limit", "side": "buy", "time_in_force": "day",
                "status": "filled", "qty": "10"
            }
        }"#;
        let update: TradeUpdate = serde_json::from_str(json).unwrap();
        assert_eq!(update.event_kind(), AlpacaTradeEvent::Fill);
        assert!(!update.has_fill_detail());
    }

    #[rstest]
    #[case("accepted", AlpacaTradeEvent::Accepted)]
    #[case("held", AlpacaTradeEvent::Held)]
    #[case("pending_review", AlpacaTradeEvent::PendingReview)]
    fn test_working_events_are_modelled(#[case] raw: &str, #[case] expected: AlpacaTradeEvent) {
        // `accepted` was observed on a live submission while the market was closed and had been
        // missing from this enum.
        let json = format!("\"{raw}\"");
        let event: AlpacaTradeEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, expected);
        assert!(event.is_working());
    }

    #[rstest]
    fn test_unknown_event_keeps_its_name_for_diagnosis() {
        // Knowing that something unmodelled happened is not enough; the name has to survive so a
        // missed order event can be identified.
        let json = r#"{
            "event": "some_future_event",
            "order": {
                "id": "1", "client_order_id": "c-1", "symbol": "AAPL", "type": "limit",
                "side": "buy", "time_in_force": "day", "status": "new", "qty": "1"
            }
        }"#;
        let update: TradeUpdate = serde_json::from_str(json).unwrap();
        assert_eq!(update.event, "some_future_event");
        assert_eq!(update.event_kind(), AlpacaTradeEvent::Unknown);
    }
}
