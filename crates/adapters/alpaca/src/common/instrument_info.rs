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

//! Alpaca-specific metadata carried on Nautilus instruments.
//!
//! Venue flags that have no home in the Nautilus domain model — most importantly overnight (Blue
//! Ocean ATS) eligibility — travel in the instrument's `info` map so that the data and execution
//! clients can consult them without holding a second copy of the asset list.
//!
//! Keys are written and read only through this module. `info` is an untyped string-keyed map, so a
//! key spelled at a call site is a silent lookup failure rather than a compile error.
//!
//! # Durability
//!
//! `info` survives the in-memory cache, the Redis cache backend, and the PyO3 boundary, but the
//! Postgres cache backend has no column for it and drops it. Treat `info` as a convenience carrier
//! rather than the system of record: it is rebuilt on every instrument load.

use nautilus_core::params::Params;
use nautilus_model::instruments::InstrumentAny;
use serde_json::Value;

use crate::http::models::Asset;

/// Key for the venue-assigned asset identifier.
pub const INFO_ASSET_ID: &str = "alpaca_asset_id";
/// Key for the listing exchange.
pub const INFO_EXCHANGE: &str = "alpaca_exchange";
/// Key for Blue Ocean ATS overnight eligibility.
pub const INFO_OVERNIGHT_TRADABLE: &str = "alpaca_overnight_tradable";
/// Key for the overnight halt flag.
pub const INFO_OVERNIGHT_HALTED: &str = "alpaca_overnight_halted";
/// Key for fractional trading support.
pub const INFO_FRACTIONABLE: &str = "alpaca_fractionable";
/// Key for marginability.
pub const INFO_MARGINABLE: &str = "alpaca_marginable";
/// Key for shortability.
pub const INFO_SHORTABLE: &str = "alpaca_shortable";
/// Key for easy-to-borrow status.
pub const INFO_EASY_TO_BORROW: &str = "alpaca_easy_to_borrow";
/// Key for publicly traded partnership status.
pub const INFO_PTP: &str = "alpaca_publicly_traded_partnership";

/// Builds the `info` map carried on an [`crate::http::models::Asset`]-derived instrument.
///
/// Keys are enumerated explicitly rather than serializing the whole asset, so the contract stays
/// reviewable and a venue field appearing or disappearing cannot silently change it.
#[must_use]
pub fn build_info(asset: &Asset) -> Params {
    let mut params = Params::new();
    params.insert(INFO_ASSET_ID.to_string(), Value::String(asset.id.clone()));
    params.insert(
        INFO_EXCHANGE.to_string(),
        Value::String(asset.exchange.clone()),
    );
    params.insert(
        INFO_OVERNIGHT_TRADABLE.to_string(),
        Value::Bool(asset.is_overnight_tradable()),
    );
    params.insert(
        INFO_OVERNIGHT_HALTED.to_string(),
        Value::Bool(asset.is_overnight_halted()),
    );
    params.insert(
        INFO_FRACTIONABLE.to_string(),
        Value::Bool(asset.fractionable),
    );
    params.insert(INFO_MARGINABLE.to_string(), Value::Bool(asset.marginable));
    params.insert(INFO_SHORTABLE.to_string(), Value::Bool(asset.shortable));
    params.insert(
        INFO_EASY_TO_BORROW.to_string(),
        Value::Bool(asset.easy_to_borrow),
    );
    params.insert(
        INFO_PTP.to_string(),
        Value::Bool(asset.is_publicly_traded_partnership()),
    );
    params
}

/// Returns the instrument's `info` map.
///
/// `info` has no accessor on the `Instrument` trait, so it has to be reached through the concrete
/// variant. This adapter only ever produces equities, and anything else did not come from here and
/// carries no Alpaca metadata.
fn info_of(instrument: &InstrumentAny) -> Option<&Params> {
    match instrument {
        InstrumentAny::Equity(equity) => equity.info.as_ref(),
        _ => None,
    }
}

fn info_bool(instrument: &InstrumentAny, key: &str) -> Option<bool> {
    info_of(instrument).and_then(|p| p.get_bool(key))
}

fn info_str(instrument: &InstrumentAny, key: &str) -> Option<String> {
    info_of(instrument).and_then(|p| p.get_str(key).map(ToString::to_string))
}

/// Returns whether the instrument is eligible for the overnight session.
///
/// `None` when the instrument carries no Alpaca metadata, which callers must distinguish from
/// `Some(false)`: an unknown eligibility is not the same as a known ineligibility.
#[must_use]
pub fn is_overnight_tradable(instrument: &InstrumentAny) -> Option<bool> {
    info_bool(instrument, INFO_OVERNIGHT_TRADABLE)
}

/// Returns whether the instrument is halted for the overnight session.
#[must_use]
pub fn is_overnight_halted(instrument: &InstrumentAny) -> Option<bool> {
    info_bool(instrument, INFO_OVERNIGHT_HALTED)
}

/// Returns whether the instrument may be routed in the overnight session right now.
///
/// Requires known eligibility and a known absence of a halt; anything unknown resolves to `false`
/// so a missing flag cannot be read as permission to trade.
#[must_use]
pub fn can_trade_overnight(instrument: &InstrumentAny) -> bool {
    is_overnight_tradable(instrument).unwrap_or(false)
        && !is_overnight_halted(instrument).unwrap_or(true)
}

/// Returns the listing exchange recorded for the instrument.
#[must_use]
pub fn exchange(instrument: &InstrumentAny) -> Option<String> {
    info_str(instrument, INFO_EXCHANGE)
}

/// Returns the venue-assigned asset identifier recorded for the instrument.
#[must_use]
pub fn asset_id(instrument: &InstrumentAny) -> Option<String> {
    info_str(instrument, INFO_ASSET_ID)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    const ASSETS_SAMPLE_JSON: &str = include_str!("../../test_data/http_assets_sample.json");

    fn asset(symbol: &str) -> Asset {
        serde_json::from_str::<Vec<Asset>>(ASSETS_SAMPLE_JSON)
            .unwrap()
            .into_iter()
            .find(|a| a.symbol == symbol)
            .unwrap()
    }

    #[rstest]
    fn test_build_info_records_overnight_flags() {
        let info = build_info(&asset("AAPL"));
        assert_eq!(info.get_bool(INFO_OVERNIGHT_TRADABLE), Some(true));
        assert_eq!(info.get_bool(INFO_OVERNIGHT_HALTED), Some(false));
        assert_eq!(info.get_str(INFO_EXCHANGE), Some("NASDAQ"));
    }

    #[rstest]
    fn test_build_info_records_halt_without_eligibility() {
        // MAMO carries `overnight_halted` but not `overnight_tradable`; the two must not collapse.
        let info = build_info(&asset("MAMO"));
        assert_eq!(info.get_bool(INFO_OVERNIGHT_TRADABLE), Some(false));
        assert_eq!(info.get_bool(INFO_OVERNIGHT_HALTED), Some(true));
    }

    #[rstest]
    fn test_build_info_records_ptp() {
        assert_eq!(build_info(&asset("MMLP")).get_bool(INFO_PTP), Some(true));
        assert_eq!(build_info(&asset("AAPL")).get_bool(INFO_PTP), Some(false));
    }

    #[rstest]
    fn test_build_info_is_explicit_not_a_dump() {
        // The map must contain exactly the enumerated keys, so a new venue field cannot silently
        // widen the contract.
        let info = build_info(&asset("AAPL"));
        for key in [
            INFO_ASSET_ID,
            INFO_EXCHANGE,
            INFO_OVERNIGHT_TRADABLE,
            INFO_OVERNIGHT_HALTED,
            INFO_FRACTIONABLE,
            INFO_MARGINABLE,
            INFO_SHORTABLE,
            INFO_EASY_TO_BORROW,
            INFO_PTP,
        ] {
            assert!(info.get(key).is_some(), "missing {key}");
        }
        assert!(info.get("status").is_none());
        assert!(info.get("name").is_none());
    }
}
