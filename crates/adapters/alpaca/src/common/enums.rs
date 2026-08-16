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

//! Enumerations for the Alpaca adapter.

use serde::{Deserialize, Serialize};
use strum::{AsRefStr, Display, EnumIter, EnumString};

/// Alpaca trading environment.
///
/// Defaults to [`AlpacaEnvironment::Paper`] so that an unconfigured client cannot reach the
/// live trading endpoint and transact real capital.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AlpacaEnvironment {
    /// Paper trading environment.
    #[default]
    Paper,
    /// Live trading environment, transacting real capital.
    Live,
}

impl AlpacaEnvironment {
    /// Returns true if this is the paper environment.
    #[must_use]
    pub const fn is_paper(self) -> bool {
        matches!(self, Self::Paper)
    }

    /// Returns true if this is the live environment.
    #[must_use]
    pub const fn is_live(self) -> bool {
        matches!(self, Self::Live)
    }
}

/// Alpaca market data feed for US equities.
///
/// Only the feeds this adapter supports are represented. The IEX and delayed-SIP feeds are
/// intentionally omitted.
#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    Display,
    EnumString,
    EnumIter,
    AsRefStr,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum AlpacaDataFeed {
    /// Consolidated Securities Information Processor feed, covering the regular session
    /// across all US exchanges.
    #[default]
    Sip,
    /// Blue Ocean ATS overnight session feed.
    ///
    /// This feed covers the overnight session only, which spans a calendar-day boundary in
    /// US Eastern time. A trading day for this feed is therefore not the same as its
    /// calendar date.
    Boats,
}

impl AlpacaDataFeed {
    /// Returns true if this feed covers the overnight session.
    #[must_use]
    pub const fn is_overnight(self) -> bool {
        matches!(self, Self::Boats)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use rstest::rstest;

    use super::*;

    #[rstest]
    fn test_environment_defaults_to_paper() {
        assert_eq!(AlpacaEnvironment::default(), AlpacaEnvironment::Paper);
        assert!(AlpacaEnvironment::default().is_paper());
        assert!(!AlpacaEnvironment::default().is_live());
    }

    #[rstest]
    #[case(AlpacaEnvironment::Paper, true, false)]
    #[case(AlpacaEnvironment::Live, false, true)]
    fn test_environment_predicates(
        #[case] environment: AlpacaEnvironment,
        #[case] expected_paper: bool,
        #[case] expected_live: bool,
    ) {
        assert_eq!(environment.is_paper(), expected_paper);
        assert_eq!(environment.is_live(), expected_live);
    }

    #[rstest]
    fn test_feed_defaults_to_sip() {
        assert_eq!(AlpacaDataFeed::default(), AlpacaDataFeed::Sip);
    }

    #[rstest]
    #[case(AlpacaDataFeed::Sip, "sip")]
    #[case(AlpacaDataFeed::Boats, "boats")]
    fn test_feed_wire_value(#[case] feed: AlpacaDataFeed, #[case] expected: &str) {
        assert_eq!(feed.to_string(), expected);
        assert_eq!(feed.as_ref(), expected);
        assert_eq!(AlpacaDataFeed::from_str(expected).unwrap(), feed);
    }

    #[rstest]
    #[case(AlpacaDataFeed::Sip, false)]
    #[case(AlpacaDataFeed::Boats, true)]
    fn test_feed_is_overnight(#[case] feed: AlpacaDataFeed, #[case] expected: bool) {
        assert_eq!(feed.is_overnight(), expected);
    }

    #[rstest]
    #[case(AlpacaDataFeed::Sip, "\"sip\"")]
    #[case(AlpacaDataFeed::Boats, "\"boats\"")]
    fn test_feed_serde_roundtrip(#[case] feed: AlpacaDataFeed, #[case] expected_json: &str) {
        let json = serde_json::to_string(&feed).unwrap();
        assert_eq!(json, expected_json);
        assert_eq!(
            serde_json::from_str::<AlpacaDataFeed>(expected_json).unwrap(),
            feed
        );
    }
}
