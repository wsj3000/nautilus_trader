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

use serde::{Deserialize, Serialize};

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

impl Account {
    /// Returns true when the venue reports any block that prevents order submission.
    #[must_use]
    pub const fn is_blocked(&self) -> bool {
        self.trading_blocked || self.account_blocked
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
