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

//! Typed query parameters for Alpaca REST requests.
//!
//! Parameters are serialized by the transport rather than concatenated by hand, so reserved
//! characters are percent-encoded and `None` fields are omitted from the query string.

use serde::Serialize;

/// Query parameters for `GET /v2/assets`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ListAssetsParams {
    /// Filter by asset status, e.g. `active`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Filter by asset class. This adapter uses `us_equity`.
    #[serde(rename = "asset_class", skip_serializing_if = "Option::is_none")]
    pub asset_class: Option<String>,
    /// Filter by listing exchange.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exchange: Option<String>,
    /// Filter by venue attributes, comma separated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attributes: Option<String>,
}

impl ListAssetsParams {
    /// Returns parameters selecting active, tradable US equities.
    #[must_use]
    pub fn active_us_equities() -> Self {
        Self {
            status: Some("active".to_string()),
            asset_class: Some("us_equity".to_string()),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn encode(params: &ListAssetsParams) -> String {
        serde_urlencoded::to_string(params).unwrap()
    }

    #[rstest]
    fn test_default_params_encode_empty() {
        assert_eq!(encode(&ListAssetsParams::default()), "");
    }

    #[rstest]
    fn test_active_us_equities_params() {
        assert_eq!(
            encode(&ListAssetsParams::active_us_equities()),
            "status=active&asset_class=us_equity"
        );
    }

    #[rstest]
    fn test_none_fields_are_omitted() {
        let params = ListAssetsParams {
            exchange: Some("NASDAQ".to_string()),
            ..ListAssetsParams::default()
        };
        assert_eq!(encode(&params), "exchange=NASDAQ");
    }

    #[rstest]
    fn test_reserved_characters_are_encoded() {
        let params = ListAssetsParams {
            attributes: Some("ptp_no_exception,has_options".to_string()),
            ..ListAssetsParams::default()
        };
        // The comma must be percent-encoded so the venue reads one value, not two params.
        assert_eq!(encode(&params), "attributes=ptp_no_exception%2Chas_options");
    }
}
