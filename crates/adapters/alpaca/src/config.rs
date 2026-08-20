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

//! Configuration structures for the Alpaca adapter.

use nautilus_network::websocket::TransportBackend;
use serde::{Deserialize, Serialize};

use crate::common::{
    enums::{AlpacaDataFeed, AlpacaEnvironment},
    urls,
};

/// Configuration for which Alpaca instruments are loaded on startup.
///
/// The venue lists roughly 13,000 tradable US equities, and loading them all costs about 93 MB
/// resident: once in the provider's cache and once in the engine's, at roughly 3.5 KB each. A node
/// that trades a handful of symbols pays that for instruments it never looks at, so the set is
/// narrowable.
#[derive(Debug, Clone, Serialize, Deserialize, bon::Builder)]
#[serde(default, deny_unknown_fields)]
pub struct AlpacaInstrumentProviderConfig {
    /// Whether to load every tradable US equity on startup.
    ///
    /// Defaults to true, which is what a node needs when it does not know in advance which
    /// instruments it will trade.
    #[builder(default = true)]
    pub load_all: bool,
    /// Instrument IDs to load when `load_all` is false.
    ///
    /// Anything not listed is skipped before it is parsed, so an excluded instrument costs
    /// nothing beyond the bytes the venue already sent.
    pub load_ids: Option<Vec<String>>,
}

impl Default for AlpacaInstrumentProviderConfig {
    fn default() -> Self {
        Self::builder().build()
    }
}

/// Configuration for the Alpaca data client.
#[derive(Debug, Clone, Serialize, Deserialize, bon::Builder)]
#[serde(default, deny_unknown_fields)]
pub struct AlpacaDataClientConfig {
    /// API key ID (falls back to `APCA_API_KEY_ID` env var).
    pub api_key: Option<String>,
    /// API secret key (falls back to `APCA_API_SECRET_KEY` env var).
    pub api_secret: Option<String>,
    /// Override for the Market Data API base URL.
    pub base_url_rest: Option<String>,
    /// Override for the Trading API base URL.
    ///
    /// The data client reaches the Trading API too, for the instrument list and the calendar, so
    /// that host is overridable independently of the market data one. Both are needed to point
    /// the client at a local proxy.
    pub base_url_trading: Option<String>,
    /// Override for the market data WebSocket URL.
    pub base_url_ws: Option<String>,
    /// Optional proxy URL for HTTP and WebSocket transports.
    pub proxy_url: Option<String>,
    /// The Alpaca environment to connect to. Market data is served from a single host for
    /// both environments; this selects the environment used for instrument metadata requests
    /// against the Trading API.
    #[builder(default)]
    pub environment: AlpacaEnvironment,
    /// The market data feed to consume.
    #[builder(default)]
    pub feed: AlpacaDataFeed,
    /// HTTP timeout in seconds.
    #[builder(default = 10)]
    pub http_timeout_secs: u64,
    /// WebSocket timeout in seconds.
    #[builder(default = 30)]
    pub ws_timeout_secs: u64,
    /// Interval for refreshing instruments in minutes.
    #[builder(default = 60)]
    pub update_instruments_interval_mins: u64,
    /// Seconds between market data polls.
    ///
    /// The venue's smallest bar is one minute, so polling faster than that only reduces the delay
    /// before a closed bar is seen; it cannot produce finer data.
    #[builder(default = 15)]
    pub poll_interval_secs: u64,
    /// Minutes of history requested on each poll.
    ///
    /// Each poll asks for a rolling window rather than only the newest bar, so a poll that fails
    /// or arrives late is recovered by the next one instead of leaving a permanent gap. Bars
    /// already emitted are filtered out by timestamp.
    #[builder(default = 5)]
    pub poll_window_mins: u64,
    /// Days of trading calendar loaded on connect.
    ///
    /// The calendar drives session detection and the SIP/overnight feed switch, and it is the only
    /// source of holidays.
    #[builder(default = 14)]
    pub calendar_lookahead_days: u32,
    /// Which instruments to load on startup.
    #[builder(default)]
    pub instrument_provider: AlpacaInstrumentProviderConfig,
    /// WebSocket transport backend (defaults to `Tungstenite`).
    ///
    /// Unused while market data is polled over REST; retained for the WebSocket transport.
    #[builder(default)]
    pub transport_backend: TransportBackend,
}

impl Default for AlpacaDataClientConfig {
    fn default() -> Self {
        Self::builder().build()
    }
}

impl AlpacaDataClientConfig {
    /// Creates a new configuration with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns true when credentials are populated and non-empty.
    #[must_use]
    pub fn has_credentials(&self) -> bool {
        has_credentials(self.api_key.as_deref(), self.api_secret.as_deref())
    }

    /// Returns the Market Data API base URL, respecting overrides.
    #[must_use]
    pub fn rest_url(&self) -> String {
        self.base_url_rest
            .clone()
            .unwrap_or_else(|| urls::data_rest_url().to_string())
    }

    /// Returns the Trading API base URL, respecting environment and overrides.
    ///
    /// Instrument metadata is served by the Trading API rather than the Market Data API, so
    /// the data client needs both hosts.
    #[must_use]
    pub fn trading_rest_url(&self) -> String {
        self.base_url_trading
            .clone()
            .unwrap_or_else(|| urls::trading_rest_url(self.environment).to_string())
    }

    /// Returns the market data WebSocket URL, respecting feed and overrides.
    #[must_use]
    pub fn ws_url(&self) -> String {
        self.base_url_ws
            .clone()
            .unwrap_or_else(|| urls::data_ws_url(self.feed).to_string())
    }
}

/// Configuration for the Alpaca execution client.
#[derive(Debug, Clone, Serialize, Deserialize, bon::Builder)]
#[serde(default, deny_unknown_fields)]
pub struct AlpacaExecClientConfig {
    /// API key ID (falls back to `APCA_API_KEY_ID` env var).
    pub api_key: Option<String>,
    /// API secret key (falls back to `APCA_API_SECRET_KEY` env var).
    pub api_secret: Option<String>,
    /// Override for the Trading API base URL.
    pub base_url_rest: Option<String>,
    /// Override for the trading event stream WebSocket URL.
    pub base_url_ws: Option<String>,
    /// Optional proxy URL for HTTP and WebSocket transports.
    pub proxy_url: Option<String>,
    /// The Alpaca environment to trade in. Defaults to
    /// [`AlpacaEnvironment::Paper`] so that an unconfigured client cannot transact real capital.
    #[builder(default)]
    pub environment: AlpacaEnvironment,
    /// HTTP timeout in seconds.
    #[builder(default = 10)]
    pub http_timeout_secs: u64,
    /// Maximum number of retry attempts for HTTP requests.
    #[builder(default = 3)]
    pub max_retries: u32,
    /// Initial retry delay in milliseconds.
    #[builder(default = 100)]
    pub retry_delay_initial_ms: u64,
    /// Maximum retry delay in milliseconds.
    #[builder(default = 5000)]
    pub retry_delay_max_ms: u64,
    /// Whether submitted orders may execute outside the regular session.
    ///
    /// Defaults to false: an order that trades pre-market or after hours does so in thinner
    /// liquidity than the caller may expect, so extended-hours execution is opted into.
    #[builder(default)]
    pub default_extended_hours: bool,
    /// WebSocket transport backend (defaults to `Tungstenite`).
    #[builder(default)]
    pub transport_backend: TransportBackend,
}

impl Default for AlpacaExecClientConfig {
    fn default() -> Self {
        Self::builder().build()
    }
}

impl AlpacaExecClientConfig {
    /// Creates a new configuration with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns true when credentials are populated and non-empty.
    #[must_use]
    pub fn has_credentials(&self) -> bool {
        has_credentials(self.api_key.as_deref(), self.api_secret.as_deref())
    }

    /// Returns the Trading API base URL, respecting environment and overrides.
    #[must_use]
    pub fn rest_url(&self) -> String {
        self.base_url_rest
            .clone()
            .unwrap_or_else(|| urls::trading_rest_url(self.environment).to_string())
    }

    /// Returns the trading event stream WebSocket URL, respecting environment and overrides.
    #[must_use]
    pub fn ws_url(&self) -> String {
        self.base_url_ws
            .clone()
            .unwrap_or_else(|| urls::trading_ws_url(self.environment).to_string())
    }
}

fn has_credentials(api_key: Option<&str>, api_secret: Option<&str>) -> bool {
    api_key.is_some_and(|s| !s.trim().is_empty())
        && api_secret.is_some_and(|s| !s.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::common::consts::{
        DATA_REST_URL, REST_URL_LIVE, REST_URL_PAPER, WS_DATA_URL_BOATS, WS_DATA_URL_SIP,
        WS_TRADING_URL_PAPER,
    };

    #[rstest]
    fn test_data_config_defaults() {
        let config = AlpacaDataClientConfig::default();
        assert_eq!(config.environment, AlpacaEnvironment::Paper);
        assert_eq!(config.feed, AlpacaDataFeed::Sip);
        assert_eq!(config.http_timeout_secs, 10);
        assert_eq!(config.ws_timeout_secs, 30);
        assert_eq!(config.update_instruments_interval_mins, 60);
        assert!(!config.has_credentials());
    }

    #[rstest]
    fn test_exec_config_defaults_to_paper() {
        let config = AlpacaExecClientConfig::default();
        assert_eq!(config.environment, AlpacaEnvironment::Paper);
        assert_eq!(config.rest_url(), REST_URL_PAPER);
        assert_eq!(config.ws_url(), WS_TRADING_URL_PAPER);
    }

    #[rstest]
    fn test_exec_config_live_resolves_live_host() {
        let config = AlpacaExecClientConfig {
            environment: AlpacaEnvironment::Live,
            ..AlpacaExecClientConfig::default()
        };
        assert_eq!(config.rest_url(), REST_URL_LIVE);
    }

    #[rstest]
    fn test_data_config_rest_url_is_market_data_host() {
        let config = AlpacaDataClientConfig::default();
        assert_eq!(config.rest_url(), DATA_REST_URL);
        assert_eq!(config.trading_rest_url(), REST_URL_PAPER);
    }

    #[rstest]
    #[case(AlpacaDataFeed::Sip, WS_DATA_URL_SIP)]
    #[case(AlpacaDataFeed::Boats, WS_DATA_URL_BOATS)]
    fn test_data_config_ws_url_follows_feed(#[case] feed: AlpacaDataFeed, #[case] expected: &str) {
        let config = AlpacaDataClientConfig {
            feed,
            ..AlpacaDataClientConfig::default()
        };
        assert_eq!(config.ws_url(), expected);
    }

    #[rstest]
    fn test_config_overrides_take_precedence() {
        let config = AlpacaExecClientConfig {
            base_url_rest: Some("https://example.test".to_string()),
            base_url_ws: Some("wss://example.test/stream".to_string()),
            environment: AlpacaEnvironment::Live,
            ..AlpacaExecClientConfig::default()
        };
        assert_eq!(config.rest_url(), "https://example.test");
        assert_eq!(config.ws_url(), "wss://example.test/stream");
    }

    #[rstest]
    fn test_has_credentials() {
        let config = AlpacaExecClientConfig {
            api_key: Some("key".to_string()),
            api_secret: Some("secret".to_string()),
            ..AlpacaExecClientConfig::default()
        };
        assert!(config.has_credentials());
    }

    #[rstest]
    #[case(Some("  "), Some("secret"))]
    #[case(Some("key"), Some("  "))]
    #[case(None, Some("secret"))]
    #[case(Some("key"), None)]
    fn test_has_credentials_rejects_incomplete(
        #[case] api_key: Option<&str>,
        #[case] api_secret: Option<&str>,
    ) {
        let config = AlpacaExecClientConfig {
            api_key: api_key.map(String::from),
            api_secret: api_secret.map(String::from),
            ..AlpacaExecClientConfig::default()
        };
        assert!(!config.has_credentials());
    }

    #[rstest]
    fn test_data_config_toml_minimal() {
        let config: AlpacaDataClientConfig = toml::from_str(
            r#"
environment = "Live"
feed = "boats"
http_timeout_secs = 5
update_instruments_interval_mins = 30
"#,
        )
        .unwrap();

        assert_eq!(config.environment, AlpacaEnvironment::Live);
        assert_eq!(config.feed, AlpacaDataFeed::Boats);
        assert_eq!(config.http_timeout_secs, 5);
        assert_eq!(config.update_instruments_interval_mins, 30);
    }

    #[rstest]
    fn test_exec_config_toml_empty_uses_defaults() {
        let config: AlpacaExecClientConfig = toml::from_str("").unwrap();
        let expected = AlpacaExecClientConfig::default();

        assert_eq!(config.environment, expected.environment);
        assert_eq!(config.http_timeout_secs, expected.http_timeout_secs);
        assert_eq!(config.max_retries, expected.max_retries);
        assert_eq!(config.transport_backend, expected.transport_backend);
    }
}
