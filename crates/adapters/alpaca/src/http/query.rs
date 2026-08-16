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

//! Typed query parameters for Alpaca REST requests.
//!
//! Parameters are serialized by the transport rather than concatenated by hand, so reserved
//! characters are percent-encoded and `None` fields are omitted from the query string.

use serde::Serialize;

use crate::common::{
    enums::AlpacaDataFeed,
    order_enums::{AlpacaOrderSide, AlpacaOrderType, AlpacaTimeInForce},
};

/// Query parameters for `GET /v2/assets`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ListAssetsParams {
    /// Filter by asset status, e.g. `active`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Filter by asset class. This adapter uses `us_equity`.
    #[serde(rename = "asset_class", skip_serializing_if = "Option::is_none")]
    pub asset_class: Option<String>,
    /// Filter by listing exchange.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exchange: Option<String>,
    /// Filter by venue attributes, comma separated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attributes: Option<String>,
}

impl ListAssetsParams {
    /// Returns parameters selecting active, tradable US equities.
    #[must_use]
    pub fn active_us_equities() -> Self {
        Self {
            status: Some("active".to_string()),
            asset_class: Some("us_equity".to_string()),
            ..Self::default()
        }
    }
}

/// Request body for `POST /v2/orders`.
///
/// Quantities and prices are carried as strings so an exact decimal reaches the venue; formatting
/// them through a float could shift the last digit of a price the caller specified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SubmitOrderRequest {
    /// Ticker symbol.
    pub symbol: String,
    /// Quantity in whole shares.
    pub qty: String,
    /// Order side.
    pub side: AlpacaOrderSide,
    /// Order type.
    #[serde(rename = "type")]
    pub order_type: AlpacaOrderType,
    /// Time in force.
    pub time_in_force: AlpacaTimeInForce,
    /// Client-assigned order identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_order_id: Option<String>,
    /// Limit price.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_price: Option<String>,
    /// Stop price.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_price: Option<String>,
    /// Whether the order may execute outside the regular session.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub extended_hours: bool,
}

/// Request body for `PATCH /v2/orders/{id}`.
///
/// The venue treats an amendment as a replacement: it answers with a new order carrying a new
/// identifier, and moves the original to `replaced`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReplaceOrderRequest {
    /// New quantity in whole shares.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qty: Option<String>,
    /// New time in force.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_in_force: Option<AlpacaTimeInForce>,
    /// New limit price.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_price: Option<String>,
    /// New stop price.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_price: Option<String>,
    /// Client-assigned identifier for the replacement order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_order_id: Option<String>,
}

impl ReplaceOrderRequest {
    /// Returns true when the request would change nothing.
    ///
    /// Sending an empty amendment would still replace the order and issue a new identifier, so it
    /// is worth detecting rather than dispatching.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.qty.is_none()
            && self.time_in_force.is_none()
            && self.limit_price.is_none()
            && self.stop_price.is_none()
    }
}

/// Query parameters for `GET /v2/orders`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ListOrdersParams {
    /// Status filter: `open`, `closed`, or `all`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Maximum orders returned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Comma-separated symbol filter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbols: Option<String>,
    /// Whether to include the legs of multi-leg orders.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nested: Option<bool>,
}

impl ListOrdersParams {
    /// Returns parameters selecting all working orders.
    #[must_use]
    pub fn open() -> Self {
        Self {
            status: Some("open".to_string()),
            ..Self::default()
        }
    }
}

/// Query parameters for `GET /v2/calendar`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CalendarParams {
    /// Inclusive first date, `YYYY-MM-DD`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    /// Inclusive last date, `YYYY-MM-DD`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
}

/// Query parameters for `GET /v2/stocks/bars`.
///
/// One request covers many symbols, which is what keeps polling affordable: a rolling window over
/// the whole subscribed universe costs one request rather than one per symbol.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct BarsParams {
    /// Comma-separated symbols.
    pub symbols: String,
    /// Venue timeframe string, e.g. `1Min`.
    pub timeframe: String,
    /// Inclusive window start (RFC 3339).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    /// Inclusive window end (RFC 3339).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    /// Market data feed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feed: Option<AlpacaDataFeed>,
    /// Maximum bars per page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Cursor returned by a previous page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_token: Option<String>,
}

impl BarsParams {
    /// Creates parameters for a symbol set and timeframe.
    #[must_use]
    pub fn new(symbols: &[String], timeframe: impl Into<String>) -> Self {
        Self {
            symbols: symbols.join(","),
            timeframe: timeframe.into(),
            ..Self::default()
        }
    }

    /// Sets the market data feed.
    #[must_use]
    pub fn with_feed(mut self, feed: AlpacaDataFeed) -> Self {
        self.feed = Some(feed);
        self
    }

    /// Sets the request window.
    #[must_use]
    pub fn with_window(mut self, start: impl Into<String>, end: impl Into<String>) -> Self {
        self.start = Some(start.into());
        self.end = Some(end.into());
        self
    }

    /// Sets the page cursor.
    #[must_use]
    pub fn with_page_token(mut self, token: impl Into<String>) -> Self {
        self.page_token = Some(token.into());
        self
    }

    /// Sets the page size limit.
    #[must_use]
    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn encode(params: &ListAssetsParams) -> String {
        serde_urlencoded::to_string(params).unwrap()
    }

    #[rstest]
    fn test_default_params_encode_empty() {
        assert_eq!(encode(&ListAssetsParams::default()), "");
    }

    #[rstest]
    fn test_active_us_equities_params() {
        assert_eq!(
            encode(&ListAssetsParams::active_us_equities()),
            "status=active&asset_class=us_equity"
        );
    }

    #[rstest]
    fn test_none_fields_are_omitted() {
        let params = ListAssetsParams {
            exchange: Some("NASDAQ".to_string()),
            ..ListAssetsParams::default()
        };
        assert_eq!(encode(&params), "exchange=NASDAQ");
    }

    #[rstest]
    fn test_reserved_characters_are_encoded() {
        let params = ListAssetsParams {
            attributes: Some("ptp_no_exception,has_options".to_string()),
            ..ListAssetsParams::default()
        };
        // The comma must be percent-encoded so the venue reads one value, not two params.
        assert_eq!(encode(&params), "attributes=ptp_no_exception%2Chas_options");
    }

    #[rstest]
    fn test_bars_params_joins_symbols() {
        let params = BarsParams::new(&["AAPL".to_string(), "MSFT".to_string()], "1Min");
        assert_eq!(params.symbols, "AAPL,MSFT");
        assert_eq!(params.timeframe, "1Min");
    }

    #[rstest]
    fn test_bars_params_encode_omits_unset_fields() {
        let params = BarsParams::new(&["AAPL".to_string()], "1Min");
        assert_eq!(
            serde_urlencoded::to_string(&params).unwrap(),
            "symbols=AAPL&timeframe=1Min"
        );
    }

    #[rstest]
    #[case(AlpacaDataFeed::Sip, "sip")]
    #[case(AlpacaDataFeed::Boats, "boats")]
    fn test_bars_params_feed_uses_wire_value(#[case] feed: AlpacaDataFeed, #[case] expected: &str) {
        let params = BarsParams::new(&["AAPL".to_string()], "1Min").with_feed(feed);
        let encoded = serde_urlencoded::to_string(&params).unwrap();
        assert!(encoded.contains(&format!("feed={expected}")), "{encoded}");
    }

    #[rstest]
    fn test_bars_params_window_is_percent_encoded() {
        let params = BarsParams::new(&["AAPL".to_string()], "1Min")
            .with_window("2026-08-14T14:30:00Z", "2026-08-14T14:33:00Z");
        let encoded = serde_urlencoded::to_string(&params).unwrap();
        // Colons in RFC 3339 timestamps must be escaped or the venue reads a different window.
        assert!(
            encoded.contains("start=2026-08-14T14%3A30%3A00Z"),
            "{encoded}"
        );
    }

    #[rstest]
    fn test_bars_params_page_token_is_encoded() {
        // Cursors are base64 and can contain `=` and `+`.
        let params = BarsParams::new(&["AAPL".to_string()], "1Min")
            .with_page_token("QUFQTHxNfDE3ODY3MTc5ODAwMDAwMDAwMDA=");
        let encoded = serde_urlencoded::to_string(&params).unwrap();
        assert!(
            encoded.contains("page_token=QUFQTHxNfDE3ODY3MTc5ODAwMDAwMDAwMDA%3D"),
            "{encoded}"
        );
    }

    #[rstest]
    fn test_calendar_params_encode() {
        let params = CalendarParams {
            start: Some("2026-08-14".to_string()),
            end: Some("2026-08-19".to_string()),
        };
        assert_eq!(
            serde_urlencoded::to_string(&params).unwrap(),
            "start=2026-08-14&end=2026-08-19"
        );
    }
}
