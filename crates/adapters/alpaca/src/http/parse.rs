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

use nautilus_core::UnixNanos;
use nautilus_model::{
    identifiers::{InstrumentId, Symbol},
    instruments::{Equity, InstrumentAny},
    types::{Currency, Price},
};

use crate::{
    common::{consts::ALPACA_VENUE, instrument_info, reg_nms},
    http::models::Asset,
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
    use nautilus_model::instruments::Instrument;
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
}
