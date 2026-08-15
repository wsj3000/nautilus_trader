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

//! Credential handling for the Alpaca adapter.
//!
//! Alpaca authenticates REST requests with a static key/secret header pair and authenticates
//! WebSocket streams with the same values sent in an auth frame. There is no request signing.

use std::fmt::{Debug, Display};

use nautilus_core::env::resolve_env_var_pair;
use zeroize::{Zeroize, ZeroizeOnDrop};

use super::consts::{ENV_API_KEY_ID, ENV_API_SECRET_KEY};

/// Returns the `(api_key, api_secret)` environment variable names.
#[must_use]
pub const fn credential_env_vars() -> (&'static str, &'static str) {
    (ENV_API_KEY_ID, ENV_API_SECRET_KEY)
}

/// Alpaca API key pair with zeroization on drop.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct AlpacaCredential {
    api_key: String,
    api_secret: String,
}

impl AlpacaCredential {
    /// Creates a new [`AlpacaCredential`] instance.
    #[must_use]
    pub const fn new(api_key: String, api_secret: String) -> Self {
        Self {
            api_key,
            api_secret,
        }
    }

    /// Resolves credentials from provided values or [`credential_env_vars`],
    /// returning `None` when neither yields a complete pair.
    #[must_use]
    pub fn resolve(api_key: Option<&str>, api_secret: Option<&str>) -> Option<Self> {
        let (key_var, secret_var) = credential_env_vars();
        let (key, secret) = resolve_env_var_pair(
            api_key.filter(|s| !s.trim().is_empty()).map(String::from),
            api_secret
                .filter(|s| !s.trim().is_empty())
                .map(String::from),
            key_var,
            secret_var,
        )?;
        Some(Self::new(key, secret))
    }

    /// Loads credentials from environment variables.
    ///
    /// # Errors
    ///
    /// Returns an error if the environment variables are unset or empty.
    pub fn from_env() -> anyhow::Result<Self> {
        let (key_var, secret_var) = credential_env_vars();
        Self::resolve(None, None).ok_or_else(|| {
            anyhow::anyhow!("Missing Alpaca credentials: set {key_var} and {secret_var}")
        })
    }

    /// Returns the API key ID.
    #[must_use]
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Returns the API secret key.
    ///
    /// Callers must not log or otherwise persist the returned value.
    #[must_use]
    pub fn api_secret(&self) -> &str {
        &self.api_secret
    }
}

/// Redacts both credential values so they cannot reach logs through `{:?}`.
impl Debug for AlpacaCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(AlpacaCredential))
            .field("api_key", &"<redacted>")
            .field("api_secret", &"<redacted>")
            .finish()
    }
}

/// Redacts both credential values so they cannot reach logs through `{}`.
impl Display for AlpacaCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}(<redacted>)", stringify!(AlpacaCredential))
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn test_credential_env_var_names() {
        assert_eq!(
            credential_env_vars(),
            ("APCA_API_KEY_ID", "APCA_API_SECRET_KEY")
        );
    }

    #[rstest]
    fn test_resolve_with_explicit_values() {
        let credential = AlpacaCredential::resolve(Some("key"), Some("secret")).unwrap();
        assert_eq!(credential.api_key(), "key");
        assert_eq!(credential.api_secret(), "secret");
    }

    #[rstest]
    #[case(Some("  "), Some("secret"))]
    #[case(Some("key"), Some("  "))]
    fn test_resolve_rejects_blank_values(#[case] key: Option<&str>, #[case] secret: Option<&str>) {
        // A blank explicit value must not be treated as a credential, but `resolve` falls back
        // to the environment, so only assert when the ambient environment is also empty.
        if std::env::var(ENV_API_KEY_ID).is_ok() || std::env::var(ENV_API_SECRET_KEY).is_ok() {
            return; // Skip when the ambient environment supplies credentials
        }
        assert!(AlpacaCredential::resolve(key, secret).is_none());
    }

    // Sentinels chosen so they cannot collide with the `api_key` / `api_secret` field names
    // that `Debug` prints.
    const KEY_SENTINEL: &str = "PKZZZZZZZZZZZZZZZZZZ";
    const SECRET_SENTINEL: &str = "abcdefZZZZZZZZZZZZZZZZZZ";

    #[rstest]
    fn test_debug_redacts_credentials() {
        let credential =
            AlpacaCredential::new(KEY_SENTINEL.to_string(), SECRET_SENTINEL.to_string());
        let rendered = format!("{credential:?}");
        assert!(!rendered.contains(KEY_SENTINEL));
        assert!(!rendered.contains(SECRET_SENTINEL));
        assert!(rendered.contains("<redacted>"));
    }

    #[rstest]
    fn test_display_redacts_credentials() {
        let credential =
            AlpacaCredential::new(KEY_SENTINEL.to_string(), SECRET_SENTINEL.to_string());
        let rendered = format!("{credential}");
        assert!(!rendered.contains(KEY_SENTINEL));
        assert!(!rendered.contains(SECRET_SENTINEL));
        assert!(rendered.contains("<redacted>"));
    }
}
