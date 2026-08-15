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

//! Constants for the Alpaca adapter.

use std::{sync::LazyLock, time::Duration};

use nautilus_model::identifiers::{ClientId, Venue};
use ustr::Ustr;

/// Venue identifier string.
pub const ALPACA: &str = "ALPACA";

/// Static venue instance.
pub static ALPACA_VENUE: LazyLock<Venue> = LazyLock::new(|| Venue::new(Ustr::from(ALPACA)));

/// Static client ID instance.
pub static ALPACA_CLIENT_ID: LazyLock<ClientId> =
    LazyLock::new(|| ClientId::new(Ustr::from(ALPACA)));

/// Trading API base URL for the live environment.
pub const REST_URL_LIVE: &str = "https://api.alpaca.markets";
/// Trading API base URL for the paper environment.
pub const REST_URL_PAPER: &str = "https://paper-api.alpaca.markets";

/// Market Data API base URL. Alpaca serves market data from a single host for both the
/// paper and live trading environments.
pub const DATA_REST_URL: &str = "https://data.alpaca.markets";

/// Trading API version path prefix.
pub const REST_TRADING_PATH: &str = "/v2";
/// Market Data API path prefix for US equities.
pub const REST_DATA_STOCKS_PATH: &str = "/v2/stocks";

/// Trading (account) event stream URL for the live environment.
pub const WS_TRADING_URL_LIVE: &str = "wss://api.alpaca.markets/stream";
/// Trading (account) event stream URL for the paper environment.
pub const WS_TRADING_URL_PAPER: &str = "wss://paper-api.alpaca.markets/stream";

/// Market data stream URL for the consolidated SIP feed.
pub const WS_DATA_URL_SIP: &str = "wss://stream.data.alpaca.markets/v2/sip";
/// Market data stream URL for the Blue Ocean ATS (BOATS) overnight session feed.
pub const WS_DATA_URL_BOATS: &str = "wss://stream.data.alpaca.markets/v2/boats";

/// Request header carrying the API key ID.
pub const HEADER_API_KEY_ID: &str = "APCA-API-KEY-ID";
/// Request header carrying the API secret key.
pub const HEADER_API_SECRET_KEY: &str = "APCA-API-SECRET-KEY";

/// Environment variable holding the API key ID.
pub const ENV_API_KEY_ID: &str = "APCA_API_KEY_ID";
/// Environment variable holding the API secret key.
pub const ENV_API_SECRET_KEY: &str = "APCA_API_SECRET_KEY";

/// Default HTTP request timeout.
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// WebSocket control-frame ping interval, in seconds.
pub const WS_HEARTBEAT_SECS: u64 = 30;

pub const RECONNECT_BASE_BACKOFF: Duration = Duration::from_millis(250);
pub const RECONNECT_MAX_BACKOFF: Duration = Duration::from_secs(30);
pub const RECONNECT_JITTER_MS: u64 = 200;
pub const RECONNECT_BACKOFF_FACTOR: f64 = 2.0;
pub const RECONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Maximum time the client waits for the feed handler task to drain on
/// disconnect before forcibly aborting.
pub const WS_DISCONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn test_venue_constant() {
        assert_eq!(ALPACA_VENUE.as_str(), ALPACA);
    }

    #[rstest]
    fn test_client_id_constant() {
        assert_eq!(ALPACA_CLIENT_ID.as_str(), ALPACA);
    }

    #[rstest]
    fn test_rest_url_constants() {
        assert!(REST_URL_LIVE.starts_with("https://"));
        assert!(REST_URL_PAPER.starts_with("https://"));
        assert!(DATA_REST_URL.starts_with("https://"));
    }

    #[rstest]
    fn test_paper_and_live_rest_urls_differ() {
        assert_ne!(REST_URL_LIVE, REST_URL_PAPER);
    }

    #[rstest]
    fn test_ws_url_constants() {
        assert!(WS_TRADING_URL_LIVE.starts_with("wss://"));
        assert!(WS_TRADING_URL_PAPER.starts_with("wss://"));
        assert!(WS_DATA_URL_SIP.starts_with("wss://"));
        assert!(WS_DATA_URL_BOATS.starts_with("wss://"));
    }

    #[rstest]
    fn test_paper_and_live_ws_urls_differ() {
        assert_ne!(WS_TRADING_URL_LIVE, WS_TRADING_URL_PAPER);
    }

    #[rstest]
    fn test_sip_and_boats_ws_urls_differ() {
        assert_ne!(WS_DATA_URL_SIP, WS_DATA_URL_BOATS);
    }

    #[rstest]
    fn test_timeout_constants() {
        assert_eq!(HTTP_TIMEOUT, Duration::from_secs(10));
        assert_eq!(WS_HEARTBEAT_SECS, 30);
        assert_eq!(WS_DISCONNECT_TIMEOUT, Duration::from_secs(5));
    }
}
