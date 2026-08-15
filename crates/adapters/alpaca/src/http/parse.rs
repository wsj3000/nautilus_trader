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

//! Conversion from Alpaca REST payloads into Nautilus domain types.

use jiff::Timestamp;
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{Bar, BarSpecification, BarType},
    enums::BarAggregation,
    identifiers::{InstrumentId, Symbol},
    instruments::{Equity, InstrumentAny},
    types::{Currency, Price, Quantity},
};

use crate::{
    common::{consts::ALPACA_VENUE, instrument_info, reg_nms},
    http::models::{AlpacaBar, Asset},
};

/// Asset class value identifying US equities in the venue payload.
pub const ASSET_CLASS_US_EQUITY: &str = "us_equity";

/// Returns the [`InstrumentId`] for an Alpaca symbol.
#[must_use]
pub fn instrument_id(symbol: &str) -> InstrumentId {
    InstrumentId::new(Symbol::from(symbol), *ALPACA_VENUE)
}

/// Converts an Alpaca [`Asset`] into a Nautilus [`Equity`] instrument.
///
/// # Price sizing
///
/// Alpaca publishes no price increment for equities, so the instrument is modelled at the finest
/// increment the regulation permits: precision 4 and an increment of `0.0001`. Coarser sizing
/// would make the risk engine reject legitimate sub-dollar limit prices, since it denies any order
/// price carrying more precision than its instrument declares. The complementary rule — orders at
/// or above `$1.00` must be whole cents — is not expressible as a single increment and is enforced
/// at order submission through [`reg_nms::check_order_price`].
///
/// # Errors
///
/// Returns an error if the asset is not a US equity.
pub fn parse_equity(asset: &Asset, ts_init: UnixNanos) -> anyhow::Result<InstrumentAny> {
    if asset.class != ASSET_CLASS_US_EQUITY {
        anyhow::bail!(
            "Cannot parse instrument for '{}': expected asset class '{ASSET_CLASS_US_EQUITY}', was '{}'",
            asset.symbol,
            asset.class
        );
    }

    let equity = Equity::new_checked(
        instrument_id(&asset.symbol),
        Symbol::from(asset.symbol.as_str()),
        None, // isin: not published by Alpaca
        Currency::USD(),
        reg_nms::PRICE_PRECISION,
        Price::new(0.0001, reg_nms::PRICE_PRECISION),
        None, // lot_size: Alpaca has no board lot; orders are in whole shares from one upwards
        None, // max_quantity
        None, // min_quantity
        None, // max_price
        None, // min_price
        None, // margin_init
        None, // margin_maint
        None, // maker_fee
        None, // taker_fee
        None, // tick_scheme: no registered scheme matches Reg NMS, see module docs
        Some(instrument_info::build_info(asset)),
        ts_init,
        ts_init,
    )?;

    Ok(InstrumentAny::Equity(equity))
}

/// Returns the venue timeframe string for a Nautilus bar specification.
///
/// # Errors
///
/// Returns an error for aggregations the venue does not offer. Alpaca's smallest bar is one
/// minute, so second and millisecond aggregations cannot be requested; those must be aggregated
/// by the engine from tick data instead.
pub fn bar_spec_to_timeframe(spec: BarSpecification) -> anyhow::Result<String> {
    let unit = match spec.aggregation {
        BarAggregation::Minute => "Min",
        BarAggregation::Hour => "Hour",
        BarAggregation::Day => "Day",
        BarAggregation::Week => "Week",
        BarAggregation::Month => "Month",
        other => anyhow::bail!(
            "Alpaca does not provide {other:?} bars; the smallest venue timeframe is one minute"
        ),
    };
    Ok(format!("{}{unit}", spec.step))
}

/// Converts a venue bar into a Nautilus [`Bar`].
///
/// # Timestamps
///
/// Alpaca stamps a bar with its **open** time, while the engine's convention is to stamp a time
/// bar at its **close** (`time_bars_timestamp_on_close` defaults to true). The interval is
/// therefore added, so a bar labelled `14:30` by the venue is emitted as `14:31` for a one-minute
/// bar. Emitting the open time instead would date every bar one interval in the past and let a
/// strategy act on a bar before the period it covers had finished.
///
/// # Errors
///
/// Returns an error if the timestamp cannot be parsed or the prices are not a valid OHLC set.
pub fn parse_bar(
    bar: &AlpacaBar,
    bar_type: BarType,
    price_precision: u8,
    ts_init: UnixNanos,
) -> anyhow::Result<Bar> {
    let ts_open = parse_rfc3339_nanos(&bar.t)?;
    let interval = bar_type.spec().timedelta();
    let interval_nanos = u64::try_from(interval.as_nanos())
        .map_err(|_| anyhow::anyhow!("Bar interval is not representable: {interval:?}"))?;
    let ts_event = UnixNanos::from(ts_open.as_u64().saturating_add(interval_nanos));

    Bar::new_checked(
        bar_type,
        Price::new(bar.o, price_precision),
        Price::new(bar.h, price_precision),
        Price::new(bar.l, price_precision),
        Price::new(bar.c, price_precision),
        Quantity::new(bar.v, 0),
        ts_event,
        ts_init,
    )
}

/// Parses an RFC 3339 timestamp into nanoseconds since the UNIX epoch.
///
/// # Errors
///
/// Returns an error if the timestamp is malformed or outside the representable range.
pub fn parse_rfc3339_nanos(raw: &str) -> anyhow::Result<UnixNanos> {
    let timestamp: Timestamp = raw
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid RFC 3339 timestamp '{raw}': {e}"))?;
    let nanos = u64::try_from(timestamp.as_nanosecond())
        .map_err(|_| anyhow::anyhow!("Timestamp '{raw}' is outside the representable range"))?;
    Ok(UnixNanos::from(nanos))
}

/// Returns true when an asset should be loaded as a tradable instrument.
///
/// `status` and `tradable` are independent in the venue payload: of 14,234 assets returned as
/// `active`, 863 were not tradable. Loading those would surface instruments the venue rejects
/// orders for.
#[must_use]
pub fn is_loadable(asset: &Asset) -> bool {
    asset.class == ASSET_CLASS_US_EQUITY && asset.is_tradable_now()
}

#[cfg(test)]
mod tests {
    use nautilus_model::{
        data::BarSpecification,
        enums::{AggregationSource, PriceType},
        instruments::Instrument,
    };
    use rstest::rstest;
    use rust_decimal_macros::dec;

    use super::*;
    use crate::common::consts::ALPACA;

    const ASSET_AAPL_JSON: &str = include_str!("../../test_data/http_asset_aapl.json");
    const ASSETS_SAMPLE_JSON: &str = include_str!("../../test_data/http_assets_sample.json");

    fn aapl() -> Asset {
        serde_json::from_str(ASSET_AAPL_JSON).unwrap()
    }

    fn sample(symbol: &str) -> Asset {
        serde_json::from_str::<Vec<Asset>>(ASSETS_SAMPLE_JSON)
            .unwrap()
            .into_iter()
            .find(|a| a.symbol == symbol)
            .unwrap()
    }

    #[rstest]
    fn test_instrument_id_uses_alpaca_venue() {
        let id = instrument_id("AAPL");
        assert_eq!(id.symbol.as_str(), "AAPL");
        assert_eq!(id.venue.as_str(), ALPACA);
        assert_eq!(id.to_string(), "AAPL.ALPACA");
    }

    #[rstest]
    fn test_parse_equity_from_canonical_payload() {
        let instrument = parse_equity(&aapl(), UnixNanos::from(1)).unwrap();
        assert_eq!(instrument.id().to_string(), "AAPL.ALPACA");
        assert_eq!(instrument.quote_currency(), Currency::USD());
    }

    #[rstest]
    fn test_price_sizing_permits_sub_dollar_quotes() {
        // Precision 4 is load-bearing: at precision 2 the risk engine would deny a legitimate
        // 0.9999 limit price for carrying more precision than the instrument declares.
        let instrument = parse_equity(&aapl(), UnixNanos::from(1)).unwrap();
        assert_eq!(instrument.price_precision(), 4);
        assert_eq!(instrument.price_increment(), Price::new(0.0001, 4));
        assert!(reg_nms::is_valid_order_price(dec!(0.9999)));
    }

    #[rstest]
    fn test_whole_share_sizing() {
        let instrument = parse_equity(&aapl(), UnixNanos::from(1)).unwrap();
        assert_eq!(instrument.size_precision(), 0);
        assert_eq!(instrument.size_increment().as_f64(), 1.0);
        // Alpaca has no board lot; IB's fixed 100-share lot does not apply here.
        assert!(instrument.lot_size().is_none());
    }

    #[rstest]
    fn test_no_tick_scheme_is_attached() {
        let instrument = parse_equity(&aapl(), UnixNanos::from(1)).unwrap();
        match instrument {
            InstrumentAny::Equity(equity) => assert!(equity.tick_scheme.is_none()),
            other => panic!("expected an Equity, was {other:?}"),
        }
    }

    #[rstest]
    fn test_overnight_flags_reach_the_instrument() {
        let instrument = parse_equity(&aapl(), UnixNanos::from(1)).unwrap();
        assert_eq!(
            instrument_info::is_overnight_tradable(&instrument),
            Some(true)
        );
        assert_eq!(
            instrument_info::is_overnight_halted(&instrument),
            Some(false)
        );
        assert!(instrument_info::can_trade_overnight(&instrument));
    }

    #[rstest]
    fn test_overnight_halted_instrument_cannot_trade_overnight() {
        let instrument = parse_equity(&sample("MMLP"), UnixNanos::from(1)).unwrap();
        assert_eq!(
            instrument_info::is_overnight_tradable(&instrument),
            Some(false)
        );
        assert_eq!(
            instrument_info::is_overnight_halted(&instrument),
            Some(true)
        );
        assert!(!instrument_info::can_trade_overnight(&instrument));
    }

    #[rstest]
    fn test_parse_rejects_non_equity_asset_class() {
        let mut asset = aapl();
        asset.class = "crypto".to_string();
        let err = parse_equity(&asset, UnixNanos::from(1)).unwrap_err();
        assert!(err.to_string().contains("us_equity"), "{err}");
    }

    #[rstest]
    #[case("AAPL", true)]
    #[case("MMLP", true)]
    #[case("MAMO", false)]
    #[case("IVCWF", false)]
    #[case("HYB", false)]
    fn test_is_loadable_requires_both_active_and_tradable(
        #[case] symbol: &str,
        #[case] expected: bool,
    ) {
        let asset = sample(symbol);
        assert_eq!(asset.status, "active");
        assert_eq!(is_loadable(&asset), expected);
    }

    #[rstest]
    fn test_timestamps_are_carried_through() {
        let ts = UnixNanos::from(1_755_000_000_000_000_000);
        let instrument = parse_equity(&aapl(), ts).unwrap();
        assert_eq!(instrument.ts_event(), ts);
        assert_eq!(instrument.ts_init(), ts);
    }

    const BARS_SIP_JSON: &str = include_str!("../../test_data/http_bars_sip.json");
    const BARS_MULTI_JSON: &str = include_str!("../../test_data/http_bars_multi.json");
    const BARS_BOATS_JSON: &str = include_str!("../../test_data/http_bars_boats.json");

    fn bars_response(json: &str) -> crate::http::models::BarsResponse {
        serde_json::from_str(json).unwrap()
    }

    fn minute_bar_type() -> BarType {
        BarType::new(
            instrument_id("AAPL"),
            BarSpecification::new(1, BarAggregation::Minute, PriceType::Last),
            AggregationSource::External,
        )
    }

    #[rstest]
    #[case(1, BarAggregation::Minute, "1Min")]
    #[case(5, BarAggregation::Minute, "5Min")]
    #[case(1, BarAggregation::Hour, "1Hour")]
    #[case(1, BarAggregation::Day, "1Day")]
    fn test_bar_spec_maps_to_venue_timeframe(
        #[case] step: usize,
        #[case] aggregation: BarAggregation,
        #[case] expected: &str,
    ) {
        let spec = BarSpecification::new(step, aggregation, PriceType::Last);
        assert_eq!(bar_spec_to_timeframe(spec).unwrap(), expected);
    }

    #[rstest]
    #[case(BarAggregation::Second)]
    #[case(BarAggregation::Millisecond)]
    #[case(BarAggregation::Tick)]
    #[case(BarAggregation::Volume)]
    fn test_sub_minute_and_non_time_aggregations_are_rejected(#[case] aggregation: BarAggregation) {
        // Alpaca's smallest bar is one minute; anything finer has to be aggregated from ticks.
        let spec = BarSpecification::new(1, aggregation, PriceType::Last);
        assert!(bar_spec_to_timeframe(spec).is_err());
    }

    #[rstest]
    fn test_bars_response_deserializes_canonical_payload() {
        let response = bars_response(BARS_SIP_JSON);
        let bars = response.bars.get("AAPL").unwrap();
        assert_eq!(bars.len(), 3);
        assert_eq!(bars[0].t, "2026-08-14T14:30:00Z");
        assert!(response.next_page_token.is_some());
    }

    #[rstest]
    fn test_bars_response_keys_by_symbol_for_multi_symbol_requests() {
        let response = bars_response(BARS_MULTI_JSON);
        assert!(response.bars.contains_key("AAPL"));
        assert!(response.bars.contains_key("MSFT"));
    }

    #[rstest]
    fn test_overnight_bars_deserialize() {
        let response = bars_response(BARS_BOATS_JSON);
        let bars = response.bars.get("AAPL").unwrap();
        assert!(!bars.is_empty());
    }

    #[rstest]
    fn test_parse_bar_stamps_the_close_not_the_open() {
        // The venue labels this bar 14:30; a one-minute bar closes at 14:31.
        let response = bars_response(BARS_SIP_JSON);
        let venue_bar = &response.bars["AAPL"][0];
        let bar = parse_bar(venue_bar, minute_bar_type(), 4, UnixNanos::from(99)).unwrap();

        let ts_open = parse_rfc3339_nanos("2026-08-14T14:30:00Z").unwrap();
        let ts_close = parse_rfc3339_nanos("2026-08-14T14:31:00Z").unwrap();
        assert_ne!(bar.ts_event, ts_open);
        assert_eq!(bar.ts_event, ts_close);
        assert_eq!(bar.ts_init, UnixNanos::from(99));
    }

    #[rstest]
    fn test_parse_bar_preserves_sub_penny_prices() {
        // Bars are built from executions, which may print finer than the quoting increment.
        let response = bars_response(BARS_SIP_JSON);
        let venue_bar = &response.bars["AAPL"][1];
        assert_eq!(venue_bar.c, 304.885);

        let bar = parse_bar(venue_bar, minute_bar_type(), 4, UnixNanos::default()).unwrap();
        assert_eq!(bar.close, Price::new(304.885, 4));
        assert_eq!(bar.close.precision, 4);
    }

    #[rstest]
    fn test_parse_bar_ohlc_values_round_trip() {
        let response = bars_response(BARS_SIP_JSON);
        let venue_bar = &response.bars["AAPL"][0];
        let bar = parse_bar(venue_bar, minute_bar_type(), 4, UnixNanos::default()).unwrap();

        assert_eq!(bar.open, Price::new(venue_bar.o, 4));
        assert_eq!(bar.high, Price::new(venue_bar.h, 4));
        assert_eq!(bar.low, Price::new(venue_bar.l, 4));
        assert_eq!(bar.close, Price::new(venue_bar.c, 4));
        assert_eq!(bar.volume, Quantity::new(venue_bar.v, 0));
    }

    #[rstest]
    fn test_parse_bar_rejects_inconsistent_ohlc() {
        let mut venue_bar = bars_response(BARS_SIP_JSON).bars["AAPL"][0].clone();
        venue_bar.h = venue_bar.l - 1.0;
        assert!(parse_bar(&venue_bar, minute_bar_type(), 4, UnixNanos::default()).is_err());
    }

    #[rstest]
    fn test_parse_bar_rejects_malformed_timestamp() {
        let mut venue_bar = bars_response(BARS_SIP_JSON).bars["AAPL"][0].clone();
        venue_bar.t = "not a timestamp".to_string();
        assert!(parse_bar(&venue_bar, minute_bar_type(), 4, UnixNanos::default()).is_err());
    }

    #[rstest]
    fn test_rfc3339_parsing_keeps_nanosecond_precision() {
        let ts = parse_rfc3339_nanos("2026-08-15T11:06:45.899614065Z").unwrap();
        assert_eq!(ts.as_u64() % 1_000_000_000, 899_614_065);
    }
}
