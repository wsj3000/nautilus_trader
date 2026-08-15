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

//! Error types for the Alpaca HTTP client.

use nautilus_network::http::HttpClientError;
use thiserror::Error;

/// Result alias for Alpaca HTTP operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Error type for Alpaca operations.
#[derive(Debug, Error)]
pub enum Error {
    /// Transport layer errors (network, connection issues).
    #[error("transport error: {0}")]
    Transport(String),

    /// JSON serialization/deserialization errors.
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),

    /// Authentication errors (missing or rejected credentials).
    #[error("auth error: {0}")]
    Auth(String),

    /// Rate limiting errors.
    #[error("rate limited (retry_after_ms={retry_after_ms:?})")]
    RateLimit { retry_after_ms: Option<u64> },

    /// Bad request errors (client-side invalid payload or parameters).
    #[error("bad request: {0}")]
    BadRequest(String),

    /// Venue-side errors reported by Alpaca.
    #[error("venue error: {0}")]
    Venue(String),

    /// Request timeout.
    #[error("timeout")]
    Timeout,

    /// Message decoding/parsing errors.
    #[error("decode error: {0}")]
    Decode(String),

    /// HTTP errors with a status code that has no more specific mapping.
    #[error("HTTP error {status}: {message}")]
    Http { status: u16, message: String },
}

impl Error {
    /// Creates a transport error.
    pub fn transport(msg: impl Into<String>) -> Self {
        Self::Transport(msg.into())
    }

    /// Creates an auth error.
    pub fn auth(msg: impl Into<String>) -> Self {
        Self::Auth(msg.into())
    }

    /// Creates a rate limit error.
    #[must_use]
    pub const fn rate_limit(retry_after_ms: Option<u64>) -> Self {
        Self::RateLimit { retry_after_ms }
    }

    /// Creates a bad request error.
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self::BadRequest(msg.into())
    }

    /// Creates a venue error.
    pub fn venue(msg: impl Into<String>) -> Self {
        Self::Venue(msg.into())
    }

    /// Creates a decode error.
    pub fn decode(msg: impl Into<String>) -> Self {
        Self::Decode(msg.into())
    }

    /// Creates an HTTP error.
    pub fn http(status: u16, message: impl Into<String>) -> Self {
        Self::Http {
            status,
            message: message.into(),
        }
    }

    /// Creates an error from an HTTP status code and response body.
    ///
    /// Alpaca reports request validation failures as `422 Unprocessable Entity` rather than
    /// `400`, so both map to [`Error::BadRequest`] and are therefore never retried.
    #[must_use]
    pub fn from_http_status(status: u16, body: &[u8]) -> Self {
        let message = String::from_utf8_lossy(body).to_string();
        match status {
            400 | 422 => Self::bad_request(format!("HTTP {status}: {message}")),
            401 | 403 => Self::auth(format!("HTTP {status}: {message}")),
            429 => Self::rate_limit(None),
            500..=599 => Self::venue(format!("HTTP {status}: {message}")),
            _ => Self::http(status, message),
        }
    }

    /// Maps a transport-level client error into this error type.
    #[must_use]
    pub fn from_http_client(error: &HttpClientError) -> Self {
        match error {
            HttpClientError::TimeoutError(_) => Self::Timeout,
            other => Self::transport(other.to_string()),
        }
    }

    /// Returns true when the failure is transient and the request may be retried.
    ///
    /// Retrying is only safe for idempotent requests; callers must gate on the HTTP method
    /// as well as on this predicate.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Transport(_) | Self::Timeout | Self::RateLimit { .. } | Self::Venue(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case(400, "bad request")]
    #[case(422, "bad request")]
    fn test_validation_statuses_map_to_bad_request(#[case] status: u16, #[case] expected: &str) {
        let error = Error::from_http_status(status, b"invalid symbol");
        assert!(matches!(error, Error::BadRequest(_)));
        assert!(error.to_string().starts_with(expected));
    }

    #[rstest]
    #[case(401)]
    #[case(403)]
    fn test_auth_statuses(#[case] status: u16) {
        assert!(matches!(
            Error::from_http_status(status, b"forbidden"),
            Error::Auth(_)
        ));
    }

    #[rstest]
    fn test_rate_limit_status() {
        assert!(matches!(
            Error::from_http_status(429, b""),
            Error::RateLimit { .. }
        ));
    }

    #[rstest]
    #[case(500)]
    #[case(502)]
    #[case(503)]
    fn test_server_statuses_map_to_venue(#[case] status: u16) {
        assert!(matches!(
            Error::from_http_status(status, b"upstream"),
            Error::Venue(_)
        ));
    }

    #[rstest]
    fn test_unmapped_status_keeps_code() {
        let error = Error::from_http_status(418, b"teapot");
        assert!(matches!(error, Error::Http { status: 418, .. }));
    }

    #[rstest]
    fn test_retryable_classification() {
        assert!(Error::transport("reset").is_retryable());
        assert!(Error::Timeout.is_retryable());
        assert!(Error::rate_limit(None).is_retryable());
        assert!(Error::venue("500").is_retryable());
    }

    #[rstest]
    fn test_non_retryable_classification() {
        assert!(!Error::auth("401").is_retryable());
        assert!(!Error::bad_request("422").is_retryable());
        assert!(!Error::decode("bad json").is_retryable());
        assert!(!Error::http(418, "teapot").is_retryable());
    }

    #[rstest]
    fn test_from_http_client_timeout() {
        let error = Error::from_http_client(&HttpClientError::TimeoutError("slow".to_string()));
        assert!(matches!(error, Error::Timeout));
    }

    #[rstest]
    fn test_from_http_client_transport() {
        let error = Error::from_http_client(&HttpClientError::Error("dns".to_string()));
        assert!(matches!(error, Error::Transport(_)));
    }
}
