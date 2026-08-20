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

use std::{str::FromStr, sync::Arc};

use ahash::AHashSet;
use nautilus_core::{AtomicMap, UnixNanos, time::get_atomic_clock_realtime};
use nautilus_model::{
    identifiers::InstrumentId,
    instruments::{Instrument, InstrumentAny},
};

use crate::{
    config::AlpacaInstrumentProviderConfig,
    http::{
        client::AlpacaRawHttpClient,
        error::Result,
        models::Asset,
        parse::{instrument_id, is_loadable, parse_equity},
        query::{ListAssetsParams, ListOrdersParams},
    },
};

/// Loads Alpaca US equity instruments and caches them by [`InstrumentId`].
#[derive(Debug, Clone)]
pub struct AlpacaInstrumentProvider {
    client: Arc<AlpacaRawHttpClient>,
    instruments: Arc<AtomicMap<InstrumentId, InstrumentAny>>,
    /// `None` loads every instrument; `Some` restricts the load to these IDs.
    load_ids: Option<AHashSet<InstrumentId>>,
}

impl AlpacaInstrumentProvider {
    /// Creates a new [`AlpacaInstrumentProvider`] that loads every instrument.
    #[must_use]
    pub fn new(client: Arc<AlpacaRawHttpClient>) -> Self {
        Self {
            client,
            instruments: Arc::new(AtomicMap::new()),
            load_ids: None,
        }
    }

    /// Creates a new [`AlpacaInstrumentProvider`] restricted by configuration.
    ///
    /// An unparseable instrument ID is logged and dropped rather than failing construction: the
    /// remaining IDs still load, and refusing to start over one malformed entry would be a worse
    /// outcome than trading the rest.
    #[must_use]
    pub fn from_config(
        client: Arc<AlpacaRawHttpClient>,
        config: &AlpacaInstrumentProviderConfig,
    ) -> Self {
        let load_ids = if config.load_all {
            None
        } else {
            let ids: AHashSet<InstrumentId> = config
                .load_ids
                .as_deref()
                .unwrap_or_default()
                .iter()
                .filter_map(|raw| {
                    InstrumentId::from_str(raw)
                        .inspect_err(|e| {
                            log::error!("Ignoring invalid load_ids entry '{raw}': {e}");
                        })
                        .ok()
                })
                .collect();

            if ids.is_empty() {
                log::warn!(
                    "load_all is false and load_ids resolved to nothing, so no Alpaca instruments \
                     will be loaded and no subscription can be validated"
                );
            }
            Some(ids)
        };

        Self {
            client,
            instruments: Arc::new(AtomicMap::new()),
            load_ids,
        }
    }

    /// Returns whether an instrument is within the configured load set.
    fn is_selected(&self, instrument_id: &InstrumentId) -> bool {
        self.load_ids
            .as_ref()
            .is_none_or(|ids| ids.contains(instrument_id))
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

    /// Loads active, tradable US equities from the venue and caches them.
    ///
    /// When the load is narrowed by configuration, the instruments the account already holds or
    /// has working orders against are added to the set. Reconciliation reads positions and orders
    /// for the whole account, not only for what this node trades, and it skips anything whose
    /// instrument is absent — which showed up as `Position discrepancy detected` and a position the
    /// engine believed was flat while the venue reported four shares. Narrowing the load must not
    /// be able to corrupt the account's picture of itself.
    ///
    /// # Errors
    ///
    /// Returns an error if the asset request fails. A failure to read positions or orders is
    /// logged and the load continues: fewer instruments is recoverable, no instruments is not.
    pub async fn load_all(&self) -> Result<Vec<InstrumentAny>> {
        let held = if self.load_ids.is_some() {
            self.held_instrument_ids().await
        } else {
            AHashSet::new()
        };

        let assets = self
            .client
            .list_assets(&ListAssetsParams::active_us_equities())
            .await?;

        Ok(self.parse_and_cache_with(&assets, get_atomic_clock_realtime().get_time_ns(), &held))
    }

    /// Returns the instruments the account holds or has working orders against.
    async fn held_instrument_ids(&self) -> AHashSet<InstrumentId> {
        let mut ids = AHashSet::new();

        match self.client.list_positions().await {
            Ok(positions) => ids.extend(positions.iter().map(|p| instrument_id(&p.symbol))),
            Err(e) => log::error!(
                "Could not read positions to extend the instrument load: {e}. Reconciliation will \
                 skip any held instrument that is not in load_ids"
            ),
        }

        match self.client.list_orders(&ListOrdersParams::open()).await {
            Ok(orders) => ids.extend(orders.iter().map(|o| instrument_id(&o.symbol))),
            Err(e) => log::error!(
                "Could not read open orders to extend the instrument load: {e}. Reconciliation \
                 will skip any working order whose instrument is not in load_ids"
            ),
        }

        if !ids.is_empty() {
            log::info!(
                "Extending the Alpaca instrument load with {} held or working instrument(s)",
                ids.len()
            );
        }
        ids
    }

    /// Parses assets into instruments and inserts them into the cache.
    ///
    /// Assets that are not loadable are skipped, and an asset that fails to parse is logged and
    /// skipped rather than failing the whole load: one malformed entry out of thousands must not
    /// leave the adapter with no instruments at all.
    pub fn parse_and_cache(&self, assets: &[Asset], ts_init: UnixNanos) -> Vec<InstrumentAny> {
        self.parse_and_cache_with(assets, ts_init, &AHashSet::new())
    }

    /// Parses and caches assets, keeping `also` in addition to the configured selection.
    pub fn parse_and_cache_with(
        &self,
        assets: &[Asset],
        ts_init: UnixNanos,
        also: &AHashSet<InstrumentId>,
    ) -> Vec<InstrumentAny> {
        let mut loaded = Vec::new();
        let mut skipped = 0_usize;

        for asset in assets {
            if !is_loadable(asset) {
                skipped += 1;
                continue;
            }

            // Checked before parsing, so an excluded instrument costs nothing beyond the bytes the
            // venue already sent. Parsing every asset and discarding it afterwards would keep the
            // allocation cost this setting exists to avoid.
            let id = instrument_id(&asset.symbol);
            if !self.is_selected(&id) && !also.contains(&id) {
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
    use std::time::{Duration, Instant};

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

    /// Builds `count` distinct assets from the fixture, so a bulk load can be measured.
    fn many_assets(count: usize) -> Vec<Asset> {
        let template = sample_assets()
            .into_iter()
            .find(|a| a.symbol == "AAPL")
            .expect("fixture has no AAPL");

        (0..count)
            .map(|i| {
                let mut asset = template.clone();
                asset.symbol = format!("SYM{i}");
                asset
            })
            .collect()
    }

    #[rstest]
    fn test_bulk_load_caches_every_asset() {
        let provider = provider();
        let assets = many_assets(500);

        let loaded = provider.parse_and_cache(&assets, UnixNanos::from(1));
        assert_eq!(loaded.len(), 500);
        assert_eq!(provider.count(), 500);
        assert!(provider.get_by_symbol("SYM499").is_some());
    }

    #[rstest]
    fn test_bulk_load_is_linear_in_the_number_of_assets() {
        // `AtomicMap::insert` clones the whole map on every call, so inserting per asset makes the
        // load quadratic. Both paths end with the same map, so nothing but elapsed time tells them
        // apart, and an assertion on the contents cannot detect the regression.
        //
        // Measured on this fixture: 2,000 assets take about 70ms batched and about 17 seconds one
        // at a time, and 4,000 take 137ms against 69 seconds. The threshold sits roughly 40x above
        // the batched cost and 5x below the per-asset cost, which is wide enough that load on the
        // machine cannot push either side across it.
        const ASSETS: usize = 2_000;
        const LIMIT: Duration = Duration::from_secs(3);

        let provider = provider();
        let assets = many_assets(ASSETS);

        let start = Instant::now();
        let loaded = provider.parse_and_cache(&assets, UnixNanos::from(1));
        let elapsed = start.elapsed();

        assert_eq!(loaded.len(), ASSETS);
        assert_eq!(provider.count(), ASSETS);
        assert!(
            elapsed < LIMIT,
            "loading {ASSETS} assets took {elapsed:?}, over the {LIMIT:?} limit: the load is no \
             longer linear, which means instruments are being inserted one at a time again"
        );
    }

    fn provider_loading(ids: &[&str]) -> AlpacaInstrumentProvider {
        let client = AlpacaRawHttpClient::new(AlpacaEnvironment::Paper, 10, None, None).unwrap();
        let config = AlpacaInstrumentProviderConfig {
            load_all: false,
            load_ids: Some(ids.iter().map(|s| (*s).to_string()).collect()),
        };
        AlpacaInstrumentProvider::from_config(Arc::new(client), &config)
    }

    #[rstest]
    fn test_load_ids_restricts_what_is_cached() {
        // The venue lists every US equity whatever the node trades, and holding them all costs
        // about 93 MB. A node that names its instruments should pay for those only.
        let provider = provider_loading(&["AAPL.ALPACA"]);
        let loaded = provider.parse_and_cache(&sample_assets(), UnixNanos::from(1));

        assert_eq!(loaded.len(), 1);
        assert_eq!(provider.count(), 1);
        assert!(provider.get_by_symbol("AAPL").is_some());
        // MMLP is loadable and would be cached under `load_all`; it is excluded here.
        assert!(provider.get_by_symbol("MMLP").is_none());
    }

    #[rstest]
    fn test_load_all_is_the_default_and_keeps_every_loadable_asset() {
        let config = AlpacaInstrumentProviderConfig::default();
        assert!(config.load_all);

        let client = AlpacaRawHttpClient::new(AlpacaEnvironment::Paper, 10, None, None).unwrap();
        let provider = AlpacaInstrumentProvider::from_config(Arc::new(client), &config);
        provider.parse_and_cache(&sample_assets(), UnixNanos::from(1));

        assert_eq!(provider.count(), 2);
    }

    #[rstest]
    fn test_bulk_load_with_load_ids_skips_the_parse_entirely() {
        // Selection happens before `parse_equity`, so 2,000 excluded assets cost no more than the
        // walk over them. Parsing and discarding would keep the allocation this setting avoids.
        let provider = provider_loading(&["SYM7.ALPACA"]);
        let assets = many_assets(2_000);

        let start = Instant::now();
        let loaded = provider.parse_and_cache(&assets, UnixNanos::from(1));
        let elapsed = start.elapsed();

        assert_eq!(loaded.len(), 1);
        assert_eq!(provider.count(), 1);
        assert!(elapsed < Duration::from_millis(500), "took {elapsed:?}");
    }

    #[rstest]
    fn test_invalid_load_id_is_dropped_without_losing_the_rest() {
        // Refusing to start over one malformed entry would be worse than trading the others.
        let provider = provider_loading(&["not-an-instrument-id", "AAPL.ALPACA"]);
        provider.parse_and_cache(&sample_assets(), UnixNanos::from(1));

        assert_eq!(provider.count(), 1);
        assert!(provider.get_by_symbol("AAPL").is_some());
    }

    #[rstest]
    fn test_empty_load_ids_caches_nothing() {
        // `load_all = false` with nothing listed is a misconfiguration, but it must not silently
        // behave as load-all: that would restore the cost the setting exists to avoid.
        let provider = provider_loading(&[]);
        let loaded = provider.parse_and_cache(&sample_assets(), UnixNanos::from(1));

        assert!(loaded.is_empty());
        assert_eq!(provider.count(), 0);
    }

    #[rstest]
    fn test_held_instruments_are_loaded_even_when_not_listed() {
        // Reconciliation reads positions for the whole account, and skips any whose instrument is
        // missing. Narrowing the load must not be able to make the engine believe a held position
        // is flat, which is what happened before this: the engine reported NVDA at 0 against the
        // venue's 4.
        let provider = provider_loading(&["AAPL.ALPACA"]);
        let held: AHashSet<InstrumentId> = ["MMLP.ALPACA"]
            .iter()
            .map(|s| InstrumentId::from_str(s).unwrap())
            .collect();

        let loaded = provider.parse_and_cache_with(&sample_assets(), UnixNanos::from(1), &held);

        assert_eq!(loaded.len(), 2);
        assert!(
            provider.get_by_symbol("AAPL").is_some(),
            "the listed instrument"
        );
        assert!(
            provider.get_by_symbol("MMLP").is_some(),
            "the held instrument"
        );
    }

    #[rstest]
    fn test_held_instruments_do_not_widen_the_load_beyond_themselves() {
        // The extension is exactly what the account holds, not a fallback to loading everything.
        let provider = provider_loading(&[]);
        let held: AHashSet<InstrumentId> = ["MMLP.ALPACA"]
            .iter()
            .map(|s| InstrumentId::from_str(s).unwrap())
            .collect();

        provider.parse_and_cache_with(&sample_assets(), UnixNanos::from(1), &held);

        assert_eq!(provider.count(), 1);
        assert!(provider.get_by_symbol("MMLP").is_some());
        assert!(provider.get_by_symbol("AAPL").is_none());
    }
}
