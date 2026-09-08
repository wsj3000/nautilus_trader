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

//! Typed models for Alpaca REST responses.
//!
//! Monetary and quantity fields are kept as `String` exactly as the venue sends them. Parsing
//! them into `Decimal` or the Nautilus value types happens at the conversion boundary so no
//! precision is lost to an intermediate float.

use anyhow::Context;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::common::order_enums::{
    AlpacaOrderSide, AlpacaOrderStatus, AlpacaOrderType, AlpacaTimeInForce,
};

/// Market clock and session state (`GET /v2/clock`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clock {
    /// Current venue timestamp (RFC 3339).
    pub timestamp: String,
    /// Whether the regular session is currently open.
    pub is_open: bool,
    /// Next regular session open (RFC 3339).
    pub next_open: String,
    /// Next regular session close (RFC 3339).
    pub next_close: String,
}

/// An order as reported by the venue (`GET /v2/orders`).
///
/// Quantities and prices stay as strings exactly as sent. Parsing them into `Decimal` or the
/// Nautilus value types happens at the conversion boundary so nothing is lost to an intermediate
/// float.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlpacaOrder {
    /// Venue order identifier.
    pub id: String,
    /// Client-assigned order identifier.
    pub client_order_id: String,
    /// Ticker symbol.
    pub symbol: String,
    /// Asset class, e.g. `us_equity`.
    #[serde(default)]
    pub asset_class: Option<String>,
    /// Order type, from the `order_type` field.
    ///
    /// The venue sends the order type twice, once here and once as `type`. Both are modelled
    /// separately because a serde alias would reject the payload as a duplicate field; read them
    /// through [`AlpacaOrder::resolved_order_type`] rather than directly.
    #[serde(default)]
    pub order_type: Option<AlpacaOrderType>,
    /// Order type, from the `type` field. See [`AlpacaOrder::order_type`].
    #[serde(rename = "type", default)]
    pub type_alias: Option<AlpacaOrderType>,
    /// Order side.
    pub side: AlpacaOrderSide,
    /// Time in force.
    pub time_in_force: AlpacaTimeInForce,
    /// Order status.
    pub status: AlpacaOrderStatus,
    /// Ordered quantity, in shares.
    #[serde(default)]
    pub qty: Option<String>,
    /// Filled quantity, in shares.
    #[serde(default)]
    pub filled_qty: Option<String>,
    /// Average fill price.
    #[serde(default)]
    pub filled_avg_price: Option<String>,
    /// Limit price.
    #[serde(default)]
    pub limit_price: Option<String>,
    /// Stop price.
    #[serde(default)]
    pub stop_price: Option<String>,
    /// Notional order amount.
    ///
    /// This adapter never sets it: orders are placed in whole shares.
    #[serde(default)]
    pub notional: Option<String>,
    /// Whether the order may execute outside the regular session.
    #[serde(default)]
    pub extended_hours: bool,
    /// Order class, e.g. `simple`, `bracket`. Simple orders carry an empty string, not null.
    #[serde(default)]
    pub order_class: Option<String>,
    /// Identifier of the order this one replaced.
    #[serde(default)]
    pub replaces: Option<String>,
    /// Identifier of the order that replaced this one.
    #[serde(default)]
    pub replaced_by: Option<String>,
    /// When the order was created (RFC 3339).
    #[serde(default)]
    pub created_at: Option<String>,
    /// When the order was submitted (RFC 3339).
    #[serde(default)]
    pub submitted_at: Option<String>,
    /// When the order was last updated (RFC 3339).
    #[serde(default)]
    pub updated_at: Option<String>,
    /// When the order was filled (RFC 3339).
    #[serde(default)]
    pub filled_at: Option<String>,
    /// When the order was canceled (RFC 3339).
    #[serde(default)]
    pub canceled_at: Option<String>,
    /// When the order expired (RFC 3339).
    ///
    /// Distinct from `expires_at`, which is the scheduled expiry rather than the event.
    #[serde(default)]
    pub expired_at: Option<String>,
    /// When the order is scheduled to expire (RFC 3339).
    #[serde(default)]
    pub expires_at: Option<String>,
    /// When the order was replaced (RFC 3339).
    #[serde(default)]
    pub replaced_at: Option<String>,
    /// When the order failed (RFC 3339).
    #[serde(default)]
    pub failed_at: Option<String>,
}

impl AlpacaOrder {
    /// Returns the order type, reconciling the venue's two copies of it.
    ///
    /// # Errors
    ///
    /// Returns an error when neither field is present, and when the two disagree. A disagreement
    /// is a venue-side inconsistency: picking one arbitrarily could submit or report the wrong
    /// order type, so it is surfaced instead.
    pub fn resolved_order_type(&self) -> anyhow::Result<AlpacaOrderType> {
        match (self.order_type, self.type_alias) {
            (Some(a), Some(b)) if a != b => anyhow::bail!(
                "Order {} reports conflicting types: order_type={a}, type={b}",
                self.id
            ),
            (Some(value), _) | (None, Some(value)) => Ok(value),
            (None, None) => anyhow::bail!("Order {} carries no order type", self.id),
        }
    }

    /// Returns the timestamp best representing the last activity on the order.
    #[must_use]
    pub fn ts_last_raw(&self) -> Option<&str> {
        self.updated_at
            .as_deref()
            .or(self.filled_at.as_deref())
            .or(self.canceled_at.as_deref())
            .or(self.submitted_at.as_deref())
            .or(self.created_at.as_deref())
    }

    /// Returns the timestamp best representing venue acceptance.
    #[must_use]
    pub fn ts_accepted_raw(&self) -> Option<&str> {
        self.submitted_at.as_deref().or(self.created_at.as_deref())
    }
}

/// A fill activity (`GET /v2/account/activities?activity_types=FILL`).
///
/// This is how completed fills are recovered on startup. The trading event stream reports fills as
/// they happen but cannot be replayed, so reconciliation reads them from here instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlpacaFillActivity {
    /// Activity identifier, unique per fill.
    pub id: String,
    /// Always `FILL` for this type.
    #[serde(default)]
    pub activity_type: Option<String>,
    /// `fill` for a complete fill, `partial_fill` otherwise.
    #[serde(rename = "type", default)]
    pub fill_type: Option<String>,
    /// When the fill occurred (RFC 3339).
    pub transaction_time: String,
    /// Ticker symbol.
    pub symbol: String,
    /// Side of the fill.
    pub side: AlpacaOrderSide,
    /// Execution price.
    pub price: String,
    /// Executed quantity.
    pub qty: String,
    /// Venue order identifier the fill belongs to.
    pub order_id: String,
    /// Cumulative filled quantity on the order.
    #[serde(default)]
    pub cum_qty: Option<String>,
    /// Quantity still working on the order.
    #[serde(default)]
    pub leaves_qty: Option<String>,
    /// Order status after the fill.
    #[serde(default)]
    pub order_status: Option<String>,
}

/// An open position (`GET /v2/positions`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlpacaPosition {
    /// Venue-assigned asset identifier.
    pub asset_id: String,
    /// Ticker symbol.
    pub symbol: String,
    /// Listing exchange.
    #[serde(default)]
    pub exchange: Option<String>,
    /// Asset class.
    #[serde(default)]
    pub asset_class: Option<String>,
    /// Signed position size: negative when short.
    pub qty: String,
    /// Quantity not reserved by working orders.
    #[serde(default)]
    pub qty_available: Option<String>,
    /// Average entry price.
    pub avg_entry_price: String,
    /// Position direction, `long` or `short`.
    pub side: String,
    /// Current market value.
    #[serde(default)]
    pub market_value: Option<String>,
    /// Cost basis.
    #[serde(default)]
    pub cost_basis: Option<String>,
    /// Unrealized profit and loss.
    #[serde(default)]
    pub unrealized_pl: Option<String>,
    /// Latest traded price.
    #[serde(default)]
    pub current_price: Option<String>,
}

impl AlpacaPosition {
    /// Returns true when the position is short.
    ///
    /// The venue reports direction twice, in `side` and in the sign of `qty`; both are consulted
    /// so a disagreement cannot silently flip a position.
    #[must_use]
    pub fn is_short(&self) -> bool {
        self.side.eq_ignore_ascii_case("short") || self.qty.trim_start().starts_with('-')
    }
}

/// A trading day from the venue calendar (`GET /v2/calendar`).
///
/// The venue sends two different time formats in the same object: `open` and `close` are
/// `"HH:MM"` while `session_open` and `session_close` are `"HHMM"`. Parsing one with the other's
/// format fails silently, so both are kept as raw strings here and normalised in
/// [`crate::common::session`].
///
/// Only days the market is open appear; weekends and holidays are absent rather than flagged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarDay {
    /// Trading date as `YYYY-MM-DD`.
    pub date: String,
    /// Regular session open in US Eastern time, formatted `HH:MM`.
    pub open: String,
    /// Regular session close in US Eastern time, formatted `HH:MM`.
    pub close: String,
    /// Extended session open in US Eastern time, formatted `HHMM`.
    #[serde(default)]
    pub session_open: Option<String>,
    /// Extended session close in US Eastern time, formatted `HHMM`.
    #[serde(default)]
    pub session_close: Option<String>,
    /// Settlement date as `YYYY-MM-DD`.
    #[serde(default)]
    pub settlement_date: Option<String>,
}

/// A single OHLCV bar (`GET /v2/stocks/bars`).
///
/// Field names are the venue's one-letter keys. Prices carry up to four decimal places: bars are
/// built from executions, and Rule 612 constrains quoting rather than trading, so sub-penny prints
/// appear routinely.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlpacaBar {
    /// Bar open timestamp (RFC 3339, UTC).
    pub t: String,
    /// Open price.
    pub o: f64,
    /// High price.
    pub h: f64,
    /// Low price.
    pub l: f64,
    /// Close price.
    pub c: f64,
    /// Volume.
    pub v: f64,
    /// Number of trades in the bar.
    #[serde(default)]
    pub n: Option<u64>,
    /// Volume-weighted average price.
    ///
    /// Carries more precision than the OHLC fields (six decimals observed) and has no counterpart
    /// on the Nautilus `Bar`, so it is not converted.
    #[serde(default)]
    pub vw: Option<f64>,
}

/// Response envelope for `GET /v2/stocks/bars`.
///
/// Bars are keyed by symbol even for a single-symbol request, and `next_page_token` is present
/// whenever more data is available.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BarsResponse {
    /// Bars keyed by symbol.
    #[serde(default)]
    pub bars: std::collections::HashMap<String, Vec<AlpacaBar>>,
    /// Cursor for the next page, when more data is available.
    #[serde(default)]
    pub next_page_token: Option<String>,
}

/// Asset attribute marking eligibility for the Blue Ocean ATS overnight session.
pub const ATTR_OVERNIGHT_TRADABLE: &str = "overnight_tradable";
/// Asset attribute marking the asset as halted for the overnight session.
pub const ATTR_OVERNIGHT_HALTED: &str = "overnight_halted";
/// Asset attribute marking fractional trading during extended hours.
pub const ATTR_FRACTIONAL_EH_ENABLED: &str = "fractional_eh_enabled";
/// Asset attribute marking the presence of a listed options chain.
pub const ATTR_HAS_OPTIONS: &str = "has_options";
/// Asset attribute marking a publicly traded partnership without withholding exception.
pub const ATTR_PTP_NO_EXCEPTION: &str = "ptp_no_exception";
/// Asset attribute marking a publicly traded partnership with a withholding exception.
pub const ATTR_PTP_WITH_EXCEPTION: &str = "ptp_with_exception";

/// Tradable asset (`GET /v2/assets`).
///
/// Field presence was verified against the full active `us_equity` list (14,234 assets): every
/// field modelled here without `Option` was present on all of them.
///
/// Note that the venue publishes **no price increment** for equities. Tick size must be derived
/// from the Reg NMS rule rather than read from this payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    /// Venue-assigned asset identifier.
    pub id: String,
    /// Asset class, e.g. `us_equity`.
    pub class: String,
    /// Listing exchange: `NASDAQ`, `NYSE`, `ARCA`, `BATS`, `AMEX`, or `OTC`.
    pub exchange: String,
    /// Ticker symbol.
    pub symbol: String,
    /// Human-readable name.
    ///
    /// Present on every observed asset, but kept optional so one malformed entry cannot fail
    /// the whole instrument load.
    #[serde(default)]
    pub name: Option<String>,
    /// Asset status, e.g. `active`, `inactive`.
    pub status: String,
    /// Whether the asset is tradable through Alpaca.
    ///
    /// Independent of `status`: 863 of the 14,234 `active` assets were not tradable, so both
    /// must be checked. See [`Asset::is_tradable_now`].
    pub tradable: bool,
    /// Whether the asset is marginable.
    #[serde(default)]
    pub marginable: bool,
    /// Whether the asset is shortable.
    #[serde(default)]
    pub shortable: bool,
    /// Whether the asset is currently easy to borrow.
    #[serde(default)]
    pub easy_to_borrow: bool,
    /// Borrow classification, e.g. `easy_to_borrow`, `hard_to_borrow`.
    #[serde(default)]
    pub borrow_status: Option<String>,
    /// Whether the asset supports fractional quantities.
    ///
    /// This adapter trades whole shares only, so the flag is carried for reporting rather
    /// than acted upon.
    #[serde(default)]
    pub fractionable: bool,
    /// Maintenance margin requirement as a whole percentage.
    ///
    /// Sent as a JSON **number**, unlike the `margin_requirement_*` fields which are strings.
    #[serde(default)]
    pub maintenance_margin_requirement: Option<u32>,
    /// Long margin requirement as a percentage string.
    #[serde(default)]
    pub margin_requirement_long: Option<String>,
    /// Short margin requirement as a percentage string.
    #[serde(default)]
    pub margin_requirement_short: Option<String>,
    /// Venue attributes such as `overnight_tradable` or `has_options`.
    ///
    /// Left untyped so a newly introduced attribute cannot fail deserialization. Use the
    /// predicates on this type rather than matching strings at call sites.
    #[serde(default)]
    pub attributes: Vec<String>,
}

impl Asset {
    /// Returns true when the asset carries the given venue attribute.
    #[must_use]
    pub fn has_attribute(&self, attribute: &str) -> bool {
        self.attributes.iter().any(|a| a == attribute)
    }

    /// Returns true when the asset can currently be traded.
    ///
    /// `status` and `tradable` are independent, so both are required.
    #[must_use]
    pub fn is_tradable_now(&self) -> bool {
        self.status == "active" && self.tradable
    }

    /// Returns true when the asset is eligible for the Blue Ocean ATS overnight session.
    #[must_use]
    pub fn is_overnight_tradable(&self) -> bool {
        self.has_attribute(ATTR_OVERNIGHT_TRADABLE)
    }

    /// Returns true when the asset is halted for the overnight session.
    ///
    /// Independent of [`Self::is_overnight_tradable`]; check both before routing an overnight
    /// order.
    #[must_use]
    pub fn is_overnight_halted(&self) -> bool {
        self.has_attribute(ATTR_OVERNIGHT_HALTED)
    }

    /// Returns true when the asset has a listed options chain.
    #[must_use]
    pub fn has_options(&self) -> bool {
        self.has_attribute(ATTR_HAS_OPTIONS)
    }

    /// Returns true when the asset is a publicly traded partnership.
    ///
    /// PTP positions can attract US withholding on gross proceeds, so these warrant explicit
    /// handling rather than being traded as ordinary equities.
    #[must_use]
    pub fn is_publicly_traded_partnership(&self) -> bool {
        self.has_attribute(ATTR_PTP_NO_EXCEPTION) || self.has_attribute(ATTR_PTP_WITH_EXCEPTION)
    }
}

/// Trading account (`GET /v2/account`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// Venue-assigned account identifier.
    pub id: String,
    /// Human-readable account number.
    pub account_number: String,
    /// Account status, e.g. `ACTIVE`.
    pub status: String,
    /// Settlement currency, e.g. `USD`.
    pub currency: String,
    /// Available cash balance.
    pub cash: String,
    /// Total portfolio value.
    #[serde(default)]
    pub portfolio_value: Option<String>,
    /// Total equity.
    pub equity: String,
    /// Equity at the previous close.
    pub last_equity: String,
    /// Available buying power.
    pub buying_power: String,
    /// Initial margin requirement.
    #[serde(default)]
    pub initial_margin: Option<String>,
    /// Maintenance margin requirement.
    #[serde(default)]
    pub maintenance_margin: Option<String>,
    /// Whether the account is flagged as a pattern day trader.
    #[serde(default)]
    pub pattern_day_trader: bool,
    /// Whether trading is blocked on the account.
    #[serde(default)]
    pub trading_blocked: bool,
    /// Whether the account is blocked from transfers.
    #[serde(default)]
    pub transfers_blocked: bool,
    /// Whether the account is blocked outright.
    #[serde(default)]
    pub account_blocked: bool,
    /// Number of day trades in the trailing five sessions.
    #[serde(default)]
    pub daytrade_count: Option<u32>,
}

/// A previous-close equity loss which crossed a configured account limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DailyLossBreach {
    /// Absolute loss from the previous close, in USD.
    pub loss_usd: Decimal,
    /// Loss as a percentage of previous-close equity.
    pub loss_pct: Decimal,
}

impl std::fmt::Display for DailyLossBreach {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Alpaca daily loss {} USD ({}%) crossed configured previous-close limit",
            self.loss_usd, self.loss_pct,
        )
    }
}

impl Account {
    /// Returns true when the venue reports any block that prevents order submission.
    #[must_use]
    pub const fn is_blocked(&self) -> bool {
        self.trading_blocked || self.account_blocked
    }

    /// Validates the venue facts required before trading this account.
    ///
    /// # Errors
    ///
    /// Returns an error when the account is inactive, blocked, non-USD, or does not match the
    /// configured account number.
    pub fn validate_for_trading(
        &self,
        expected_account_number: Option<&str>,
    ) -> anyhow::Result<()> {
        if self.status != "ACTIVE" {
            anyhow::bail!(
                "Alpaca account {} has status {}; expected ACTIVE",
                self.account_number,
                self.status,
            );
        }
        if self.is_blocked() {
            anyhow::bail!(
                "Alpaca account {} is blocked from trading",
                self.account_number,
            );
        }
        if self.currency != "USD" {
            anyhow::bail!(
                "Alpaca account {} uses {}; expected USD",
                self.account_number,
                self.currency,
            );
        }
        if let Some(expected) = expected_account_number
            && self.account_number != expected
        {
            anyhow::bail!(
                "Authenticated Alpaca account number {} does not match expected {}",
                self.account_number,
                expected,
            );
        }
        Ok(())
    }

    /// Evaluates absolute and percentage loss from the venue's previous-close equity.
    ///
    /// # Errors
    ///
    /// Returns an error when a configured limit is non-positive, an equity amount is not a
    /// decimal, or previous-close equity is not positive.
    pub fn daily_loss_breach(
        &self,
        max_loss_usd: Option<Decimal>,
        max_loss_pct: Option<Decimal>,
    ) -> anyhow::Result<Option<DailyLossBreach>> {
        if max_loss_usd.is_none() && max_loss_pct.is_none() {
            return Ok(None);
        }
        for (name, limit) in [
            ("maximum daily loss USD", max_loss_usd),
            ("maximum daily loss percent", max_loss_pct),
        ] {
            if limit.is_some_and(|value| value <= Decimal::ZERO) {
                anyhow::bail!("{name} must be positive");
            }
        }
        let equity = self
            .equity
            .parse::<Decimal>()
            .with_context(|| format!("Invalid equity: {}", self.equity))?;
        let last_equity = self
            .last_equity
            .parse::<Decimal>()
            .with_context(|| format!("Invalid last_equity: {}", self.last_equity))?;
        if last_equity <= Decimal::ZERO {
            anyhow::bail!("Alpaca last_equity must be positive when daily-loss limits are enabled");
        }
        let loss_usd = last_equity - equity;
        if loss_usd <= Decimal::ZERO {
            return Ok(None);
        }
        let loss_pct = loss_usd * Decimal::from(100) / last_equity;
        let cash_hit = max_loss_usd.is_some_and(|limit| loss_usd >= limit);
        let pct_hit = max_loss_pct.is_some_and(|limit| loss_pct >= limit);
        Ok((cash_hit || pct_hit).then_some(DailyLossBreach { loss_usd, loss_pct }))
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    // Canonical payloads captured from the Alpaca paper Trading API.
    const CLOCK_JSON: &str = include_str!("../../test_data/http_clock.json");
    const ASSET_AAPL_JSON: &str = include_str!("../../test_data/http_asset_aapl.json");
    const ASSETS_SAMPLE_JSON: &str = include_str!("../../test_data/http_assets_sample.json");

    fn sample_assets() -> Vec<Asset> {
        serde_json::from_str(ASSETS_SAMPLE_JSON).unwrap()
    }

    fn sample(symbol: &str) -> Asset {
        sample_assets()
            .into_iter()
            .find(|a| a.symbol == symbol)
            .unwrap_or_else(|| panic!("{symbol} missing from sample payload"))
    }

    #[rstest]
    fn test_clock_deserializes_canonical() {
        let clock: Clock = serde_json::from_str(CLOCK_JSON).unwrap();
        assert!(!clock.is_open);
        assert_eq!(clock.next_open, "2026-08-17T09:30:00-04:00");
        assert_eq!(clock.next_close, "2026-08-17T16:00:00-04:00");
        // Nanosecond precision must survive as text; parsing through a float would truncate it.
        assert_eq!(clock.timestamp, "2026-08-15T11:06:45.899614065-04:00");
    }

    #[rstest]
    fn test_asset_deserializes_canonical() {
        let asset: Asset = serde_json::from_str(ASSET_AAPL_JSON).unwrap();
        assert_eq!(asset.symbol, "AAPL");
        assert_eq!(asset.class, "us_equity");
        assert_eq!(asset.exchange, "NASDAQ");
        assert_eq!(asset.name.as_deref(), Some("Apple Inc. Common Stock"));
        assert_eq!(asset.borrow_status.as_deref(), Some("easy_to_borrow"));
        assert_eq!(asset.margin_requirement_long.as_deref(), Some("30"));
        assert_eq!(asset.margin_requirement_short.as_deref(), Some("30"));
        assert!(asset.is_tradable_now());
    }

    #[rstest]
    fn test_maintenance_margin_requirement_is_a_number_not_a_string() {
        // The venue sends this field as a JSON number while the sibling `margin_requirement_*`
        // fields are strings; modelling it as a String silently fails the whole payload.
        let asset: Asset = serde_json::from_str(ASSET_AAPL_JSON).unwrap();
        assert_eq!(asset.maintenance_margin_requirement, Some(30));
    }

    #[rstest]
    fn test_asset_publishes_no_price_increment() {
        // Regression guard for the Reg NMS tick-size work: the venue does not publish a price
        // increment for equities, so it must be derived rather than read.
        let raw: serde_json::Value = serde_json::from_str(ASSET_AAPL_JSON).unwrap();
        for absent in ["price_increment", "min_order_size", "min_trade_increment"] {
            assert!(
                raw.get(absent).is_none(),
                "venue unexpectedly published `{absent}`; tick-size derivation may need revisiting"
            );
        }
    }

    #[rstest]
    fn test_assets_sample_deserializes() {
        assert_eq!(sample_assets().len(), 5);
    }

    #[rstest]
    #[case("AAPL", true, false)]
    #[case("MAMO", false, true)]
    #[case("MMLP", false, true)]
    #[case("IVCWF", false, false)]
    fn test_overnight_attributes_are_independent(
        #[case] symbol: &str,
        #[case] tradable_overnight: bool,
        #[case] halted_overnight: bool,
    ) {
        // `overnight_halted` appears without `overnight_tradable`, so neither predicate can be
        // derived from the other; routing an overnight order must consult both.
        let asset = sample(symbol);
        assert_eq!(asset.is_overnight_tradable(), tradable_overnight);
        assert_eq!(asset.is_overnight_halted(), halted_overnight);
    }

    #[rstest]
    #[case("AAPL", true)]
    #[case("MMLP", true)]
    #[case("MAMO", false)]
    #[case("IVCWF", false)]
    #[case("HYB", false)]
    fn test_status_active_does_not_imply_tradable(#[case] symbol: &str, #[case] expected: bool) {
        let asset = sample(symbol);
        assert_eq!(asset.status, "active");
        assert_eq!(asset.is_tradable_now(), expected);
    }

    #[rstest]
    fn test_publicly_traded_partnership_detected() {
        assert!(sample("MMLP").is_publicly_traded_partnership());
        assert!(!sample("AAPL").is_publicly_traded_partnership());
    }

    #[rstest]
    fn test_has_options() {
        assert!(sample("AAPL").has_options());
        assert!(!sample("IVCWF").has_options());
    }

    #[rstest]
    fn test_asset_without_attributes_reports_all_predicates_false() {
        let asset = sample("IVCWF");
        assert!(asset.attributes.is_empty());
        assert!(!asset.is_overnight_tradable());
        assert!(!asset.is_overnight_halted());
        assert!(!asset.has_options());
        assert!(!asset.is_publicly_traded_partnership());
    }

    #[rstest]
    fn test_unknown_attribute_does_not_fail_deserialization() {
        // The venue adds attributes over time; an unseen value must not break instrument loading.
        let json = r#"{
            "id": "1", "class": "us_equity", "exchange": "NASDAQ", "symbol": "NEW",
            "status": "active", "tradable": true,
            "attributes": ["some_future_attribute"]
        }"#;
        let asset: Asset = serde_json::from_str(json).unwrap();
        assert!(asset.has_attribute("some_future_attribute"));
        assert!(!asset.is_overnight_tradable());
    }

    #[rstest]
    fn test_account_deserializes_and_preserves_string_amounts() {
        let json = r#"{
            "id": "904837e3-3b76-47ec-b432-046db621571b",
            "account_number": "010203ABCD",
            "status": "ACTIVE",
            "currency": "USD",
            "cash": "10000.12345678",
            "portfolio_value": "12345.67",
            "equity": "12345.67",
            "last_equity": "12300.00",
            "buying_power": "40000.00",
            "pattern_day_trader": false,
            "daytrade_count": 2
        }"#;
        let account: Account = serde_json::from_str(json).unwrap();
        // The raw decimal string must survive untouched; a float round-trip would lose digits.
        assert_eq!(account.cash, "10000.12345678");
        assert_eq!(account.daytrade_count, Some(2));
        assert!(!account.is_blocked());
    }

    #[rstest]
    #[case(true, false, true)]
    #[case(false, true, true)]
    #[case(false, false, false)]
    fn test_account_is_blocked(
        #[case] trading_blocked: bool,
        #[case] account_blocked: bool,
        #[case] expected: bool,
    ) {
        let json = r#"{
            "id": "1",
            "account_number": "1",
            "status": "ACTIVE",
            "currency": "USD",
            "cash": "0",
            "equity": "0",
            "last_equity": "0",
            "buying_power": "0"
        }"#;
        let mut account: Account = serde_json::from_str(json).unwrap();
        account.trading_blocked = trading_blocked;
        account.account_blocked = account_blocked;
        assert_eq!(account.is_blocked(), expected);
    }
}
