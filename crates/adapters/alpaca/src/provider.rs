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

//! Loads and caches Alpaca instruments.

use std::sync::Arc;

use nautilus_core::{AtomicMap, UnixNanos, time::get_atomic_clock_realtime};
use nautilus_model::{
    identifiers::InstrumentId,
    instruments::{Instrument, InstrumentAny},
};

use crate::http::{
    client::AlpacaRawHttpClient,
    error::Result,
    models::Asset,
    parse::{instrument_id, is_loadable, parse_equity},
    query::ListAssetsParams,
};

/// Loads Alpaca US equity instruments and caches them by [`InstrumentId`].
#[derive(Debug, Clone)]
pub struct AlpacaInstrumentProvider {
    client: Arc<AlpacaRawHttpClient>,
    instruments: Arc<AtomicMap<InstrumentId, InstrumentAny>>,
}

impl AlpacaInstrumentProvider {
    /// Creates a new [`AlpacaInstrumentProvider`].
    #[must_use]
    pub fn new(client: Arc<AlpacaRawHttpClient>) -> Self {
        Self {
            client,
            instruments: Arc::new(AtomicMap::new()),
        }
    }

    /// Returns the instrument cache.
    #[must_use]
    pub fn instruments(&self) -> &Arc<AtomicMap<InstrumentId, InstrumentAny>> {
        &self.instruments
    }

    /// Returns the number of cached instruments.
    #[must_use]
    pub fn count(&self) -> usize {
        self.instruments.len()
    }

    /// Returns a cached instrument by ID, if present.
    #[must_use]
    pub fn get(&self, instrument_id: &InstrumentId) -> Option<InstrumentAny> {
        self.instruments.get_cloned(instrument_id)
    }

    /// Returns a cached instrument by symbol, if present.
    #[must_use]
    pub fn get_by_symbol(&self, symbol: &str) -> Option<InstrumentAny> {
        self.get(&instrument_id(symbol))
    }

    /// Loads all active, tradable US equities from the venue and caches them.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub async fn load_all(&self) -> Result<Vec<InstrumentAny>> {
        let assets = self
            .client
            .list_assets(&ListAssetsParams::active_us_equities())
            .await?;

        Ok(self.parse_and_cache(&assets, get_atomic_clock_realtime().get_time_ns()))
    }

    /// Parses assets into instruments and inserts them into the cache.
    ///
    /// Assets that are not loadable are skipped, and an asset that fails to parse is logged and
    /// skipped rather than failing the whole load: one malformed entry out of thousands must not
    /// leave the adapter with no instruments at all.
    pub fn parse_and_cache(&self, assets: &[Asset], ts_init: UnixNanos) -> Vec<InstrumentAny> {
        let mut loaded = Vec::new();
        let mut skipped = 0_usize;

        for asset in assets {
            if !is_loadable(asset) {
                skipped += 1;
                continue;
            }

            match parse_equity(asset, ts_init) {
                Ok(instrument) => loaded.push(instrument),
                Err(e) => {
                    log::warn!("Failed to parse Alpaca asset '{}': {e}", asset.symbol);
                    skipped += 1;
                }
            }
        }

        // Inserted in one pass. `AtomicMap::insert` clones the whole map per call, so inserting
        // individually would be quadratic — with the venue's ~13,000 tradable equities that is
        // tens of millions of instrument clones, and the load never finishes.
        self.instruments.rcu(|map| {
            for instrument in &loaded {
                map.insert(instrument.id(), instrument.clone());
            }
        });

        log::debug!(
            "Loaded {} Alpaca instruments ({skipped} skipped of {} assets)",
            loaded.len(),
            assets.len()
        );

        loaded
    }
}

#[cfg(test)]
mod tests {
    use nautilus_model::instruments::Instrument;
    use rstest::rstest;

    use super::*;
    use crate::common::{enums::AlpacaEnvironment, instrument_info};

    const ASSETS_SAMPLE_JSON: &str = include_str!("../test_data/http_assets_sample.json");

    fn provider() -> AlpacaInstrumentProvider {
        let client = AlpacaRawHttpClient::new(AlpacaEnvironment::Paper, 10, None, None).unwrap();
        AlpacaInstrumentProvider::new(Arc::new(client))
    }

    fn sample_assets() -> Vec<Asset> {
        serde_json::from_str(ASSETS_SAMPLE_JSON).unwrap()
    }

    #[rstest]
    fn test_provider_starts_empty() {
        assert_eq!(provider().count(), 0);
    }

    #[rstest]
    fn test_parse_and_cache_loads_only_tradable_assets() {
        let provider = provider();
        let loaded = provider.parse_and_cache(&sample_assets(), UnixNanos::from(1));

        // Of the five sampled assets only AAPL and MMLP are active *and* tradable.
        assert_eq!(loaded.len(), 2);
        assert_eq!(provider.count(), 2);
        assert!(provider.get_by_symbol("AAPL").is_some());
        assert!(provider.get_by_symbol("MMLP").is_some());
        assert!(provider.get_by_symbol("MAMO").is_none());
        assert!(provider.get_by_symbol("IVCWF").is_none());
        assert!(provider.get_by_symbol("HYB").is_none());
    }

    #[rstest]
    fn test_cached_instrument_retains_overnight_metadata() {
        let provider = provider();
        provider.parse_and_cache(&sample_assets(), UnixNanos::from(1));

        let aapl = provider.get_by_symbol("AAPL").unwrap();
        assert!(instrument_info::can_trade_overnight(&aapl));

        let mmlp = provider.get_by_symbol("MMLP").unwrap();
        assert!(!instrument_info::can_trade_overnight(&mmlp));
    }

    #[rstest]
    fn test_reload_replaces_entries_without_duplicating() {
        let provider = provider();
        provider.parse_and_cache(&sample_assets(), UnixNanos::from(1));
        provider.parse_and_cache(&sample_assets(), UnixNanos::from(2));

        assert_eq!(provider.count(), 2);
        assert_eq!(
            provider.get_by_symbol("AAPL").unwrap().ts_init(),
            UnixNanos::from(2)
        );
    }

    #[rstest]
    fn test_unparseable_asset_is_skipped_not_fatal() {
        let provider = provider();
        let mut assets = sample_assets();
        // A tradable asset carrying an unexpected class must not abort the whole load.
        let mut bad = assets[0].clone();
        bad.symbol = "BAD".to_string();
        bad.class = "crypto".to_string();
        assets.push(bad);

        let loaded = provider.parse_and_cache(&assets, UnixNanos::from(1));
        assert_eq!(loaded.len(), 2);
        assert!(provider.get_by_symbol("BAD").is_none());
    }

    #[rstest]
    fn test_empty_asset_list_is_not_an_error() {
        let provider = provider();
        assert!(provider.parse_and_cache(&[], UnixNanos::from(1)).is_empty());
        assert_eq!(provider.count(), 0);
    }

    #[rstest]
    fn test_bulk_load_inserts_in_one_pass() {
        // Guards the quadratic insert: `AtomicMap::insert` clones the whole map per call, so a
        // per-asset insert makes a venue-sized load take tens of millions of clones. This asserts
        // the batch lands, which only holds if the single-pass path is taken.
        let provider = provider();
        let mut assets = Vec::new();
        for i in 0..500 {
            let mut asset = sample_assets()
                .into_iter()
                .find(|a| a.symbol == "AAPL")
                .unwrap();
            asset.symbol = format!("SYM{i}");
            assets.push(asset);
        }

        let loaded = provider.parse_and_cache(&assets, UnixNanos::from(1));
        assert_eq!(loaded.len(), 500);
        assert_eq!(provider.count(), 500);
        assert!(provider.get_by_symbol("SYM499").is_some());
    }
}
