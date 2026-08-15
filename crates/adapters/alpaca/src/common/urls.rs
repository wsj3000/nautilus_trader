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

//! URL resolution for the Alpaca adapter.
//!
//! Alpaca separates the trading host from the market data host. The trading host is selected by
//! [`AlpacaEnvironment`], while market data is served from a single host for both environments
//! and is selected by [`AlpacaDataFeed`].

use super::{
    consts::{
        DATA_REST_URL, REST_URL_LIVE, REST_URL_PAPER, WS_DATA_URL_BOATS, WS_DATA_URL_SIP,
        WS_TRADING_URL_LIVE, WS_TRADING_URL_PAPER,
    },
    enums::{AlpacaDataFeed, AlpacaEnvironment},
};

/// Returns the Trading API base URL for the given environment.
#[must_use]
pub const fn trading_rest_url(environment: AlpacaEnvironment) -> &'static str {
    match environment {
        AlpacaEnvironment::Live => REST_URL_LIVE,
        AlpacaEnvironment::Paper => REST_URL_PAPER,
    }
}

/// Returns the Market Data API base URL.
///
/// Alpaca serves market data from the same host for both the paper and live environments, so
/// this does not vary by [`AlpacaEnvironment`].
#[must_use]
pub const fn data_rest_url() -> &'static str {
    DATA_REST_URL
}

/// Returns the trading (account) event stream URL for the given environment.
#[must_use]
pub const fn trading_ws_url(environment: AlpacaEnvironment) -> &'static str {
    match environment {
        AlpacaEnvironment::Live => WS_TRADING_URL_LIVE,
        AlpacaEnvironment::Paper => WS_TRADING_URL_PAPER,
    }
}

/// Returns the market data stream URL for the given feed.
#[must_use]
pub const fn data_ws_url(feed: AlpacaDataFeed) -> &'static str {
    match feed {
        AlpacaDataFeed::Sip => WS_DATA_URL_SIP,
        AlpacaDataFeed::Boats => WS_DATA_URL_BOATS,
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case(AlpacaEnvironment::Live, REST_URL_LIVE)]
    #[case(AlpacaEnvironment::Paper, REST_URL_PAPER)]
    fn test_trading_rest_url(#[case] environment: AlpacaEnvironment, #[case] expected: &str) {
        assert_eq!(trading_rest_url(environment), expected);
    }

    #[rstest]
    fn test_data_rest_url_is_environment_independent() {
        assert_eq!(data_rest_url(), DATA_REST_URL);
    }

    #[rstest]
    #[case(AlpacaEnvironment::Live, WS_TRADING_URL_LIVE)]
    #[case(AlpacaEnvironment::Paper, WS_TRADING_URL_PAPER)]
    fn test_trading_ws_url(#[case] environment: AlpacaEnvironment, #[case] expected: &str) {
        assert_eq!(trading_ws_url(environment), expected);
    }

    #[rstest]
    #[case(AlpacaDataFeed::Sip, WS_DATA_URL_SIP)]
    #[case(AlpacaDataFeed::Boats, WS_DATA_URL_BOATS)]
    fn test_data_ws_url(#[case] feed: AlpacaDataFeed, #[case] expected: &str) {
        assert_eq!(data_ws_url(feed), expected);
    }

    #[rstest]
    fn test_paper_urls_never_resolve_to_live_host() {
        assert_ne!(
            trading_rest_url(AlpacaEnvironment::Paper),
            trading_rest_url(AlpacaEnvironment::Live)
        );
        assert_ne!(
            trading_ws_url(AlpacaEnvironment::Paper),
            trading_ws_url(AlpacaEnvironment::Live)
        );
    }
}
