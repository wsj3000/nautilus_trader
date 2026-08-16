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

//! Provides the raw HTTP client for the Alpaca REST APIs.
//!
//! Alpaca splits its REST surface across two hosts with independent rate limits: the Trading
//! API (account, assets, orders, positions) and the Market Data API (bars, quotes, trades).
//! One client covers both, because they share credentials and retry policy, and applies a
//! separate quota to each through the transport's keyed rate limiter.
//!
//! Authentication is a static header pair, so unlike signed or token-based venues the headers
//! are installed once as transport defaults rather than rebuilt per request.

use std::{collections::HashMap, num::NonZeroU32, sync::LazyLock};

use nautilus_core::consts::NAUTILUS_USER_AGENT;
use nautilus_network::{
    http::{HttpClient, HttpClientError, HttpResponse, Method, USER_AGENT},
    ratelimiter::quota::Quota,
    retry::{RetryConfig, RetryManager},
};
use serde::{Serialize, de::DeserializeOwned};
use tokio_util::sync::CancellationToken;

use crate::{
    common::{
        consts::{
            HEADER_API_KEY_ID, HEADER_API_SECRET_KEY, REST_DATA_STOCKS_PATH, REST_TRADING_PATH,
        },
        credential::AlpacaCredential,
        enums::AlpacaEnvironment,
        urls,
    },
    http::{
        error::{Error, Result},
        models::{
            Account, AlpacaBar, AlpacaOrder, AlpacaPosition, Asset, BarsResponse, CalendarDay,
            Clock,
        },
        query::{
            BarsParams, CalendarParams, ListAssetsParams, ListOrdersParams, ReplaceOrderRequest,
            SubmitOrderRequest,
        },
    },
};

/// Rate limiter key for Trading API requests.
pub const RATE_LIMIT_KEY_TRADING: &str = "alpaca:trading";
/// Rate limiter key for Market Data API requests.
pub const RATE_LIMIT_KEY_DATA: &str = "alpaca:data";

/// Default Trading API rate limit (200 requests per minute per account).
pub static ALPACA_TRADING_QUOTA: LazyLock<Quota> =
    LazyLock::new(|| Quota::per_minute(NonZeroU32::new(200).expect("non-zero")));

/// Default Market Data API rate limit.
///
/// 200 requests per minute matches the entry-level market data plan. Higher plans raise this
/// substantially, so it is overridable at construction rather than assumed.
pub static ALPACA_DATA_QUOTA: LazyLock<Quota> =
    LazyLock::new(|| Quota::per_minute(NonZeroU32::new(200).expect("non-zero")));

/// Returns the default retry configuration for the Alpaca HTTP client.
#[must_use]
pub fn default_retry_config() -> RetryConfig {
    RetryConfig {
        max_retries: 3,
        initial_delay_ms: 100,
        max_delay_ms: 5_000,
        backoff_factor: 2.0,
        jitter_ms: 250,
        operation_timeout_ms: Some(60_000),
        immediate_first: false,
        max_elapsed_ms: Some(180_000),
    }
}

/// Selects which Alpaca host and quota a request uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiTarget {
    /// The Trading API host.
    Trading,
    /// The Market Data API host.
    Data,
}

impl ApiTarget {
    const fn rate_limit_key(self) -> &'static str {
        match self {
            Self::Trading => RATE_LIMIT_KEY_TRADING,
            Self::Data => RATE_LIMIT_KEY_DATA,
        }
    }
}

/// Provides a raw HTTP client for low-level Alpaca REST API operations.
#[derive(Debug)]
pub struct AlpacaRawHttpClient {
    client: HttpClient,
    trading_base_url: String,
    data_base_url: String,
    has_credentials: bool,
    retry_manager: RetryManager<Error>,
    cancellation_token: CancellationToken,
}

impl AlpacaRawHttpClient {
    /// Creates a new unauthenticated [`AlpacaRawHttpClient`].
    ///
    /// Alpaca requires credentials on effectively every endpoint, so this is useful mainly for
    /// tests and for pointing at a local stub through [`Self::set_trading_base_url`] and
    /// [`Self::set_data_base_url`].
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying HTTP client cannot be created.
    pub fn new(
        environment: AlpacaEnvironment,
        timeout_secs: u64,
        proxy_url: Option<String>,
        retry_config: Option<RetryConfig>,
    ) -> std::result::Result<Self, HttpClientError> {
        Self::build(None, environment, timeout_secs, proxy_url, retry_config)
    }

    /// Creates a new [`AlpacaRawHttpClient`] with credentials.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying HTTP client cannot be created.
    pub fn with_credentials(
        credential: AlpacaCredential,
        environment: AlpacaEnvironment,
        timeout_secs: u64,
        proxy_url: Option<String>,
        retry_config: Option<RetryConfig>,
    ) -> std::result::Result<Self, HttpClientError> {
        Self::build(
            Some(credential),
            environment,
            timeout_secs,
            proxy_url,
            retry_config,
        )
    }

    /// Creates an authenticated client from environment variables.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Auth`] if the credential environment variables are unset or empty, or
    /// if the underlying HTTP client cannot be created.
    pub fn from_env(environment: AlpacaEnvironment) -> Result<Self> {
        let credential = AlpacaCredential::from_env()
            .map_err(|e| Error::auth(format!("Missing credentials in environment: {e}")))?;
        Self::with_credentials(credential, environment, 10, None, None)
            .map_err(|e| Error::auth(format!("Failed to create HTTP client: {e}")))
    }

    fn build(
        credential: Option<AlpacaCredential>,
        environment: AlpacaEnvironment,
        timeout_secs: u64,
        proxy_url: Option<String>,
        retry_config: Option<RetryConfig>,
    ) -> std::result::Result<Self, HttpClientError> {
        let has_credentials = credential.is_some();
        let mut headers =
            HashMap::from([(USER_AGENT.to_string(), NAUTILUS_USER_AGENT.to_string())]);

        // Alpaca authenticates with a static header pair, so these are installed once as
        // transport defaults instead of being rebuilt for every request.
        if let Some(credential) = credential {
            headers.insert(
                HEADER_API_KEY_ID.to_string(),
                credential.api_key().to_string(),
            );
            headers.insert(
                HEADER_API_SECRET_KEY.to_string(),
                credential.api_secret().to_string(),
            );
        }

        let keyed_quotas = vec![
            (RATE_LIMIT_KEY_TRADING.to_string(), *ALPACA_TRADING_QUOTA),
            (RATE_LIMIT_KEY_DATA.to_string(), *ALPACA_DATA_QUOTA),
        ];

        Ok(Self {
            client: HttpClient::new(
                headers,
                vec![],
                keyed_quotas,
                None, // Every request carries an explicit key, so no default quota is needed
                Some(timeout_secs),
                proxy_url,
            )?,
            trading_base_url: urls::trading_rest_url(environment).to_string(),
            data_base_url: urls::data_rest_url().to_string(),
            has_credentials,
            retry_manager: RetryManager::new(retry_config.unwrap_or_else(default_retry_config)),
            cancellation_token: CancellationToken::new(),
        })
    }

    /// Overrides the Trading API base URL.
    pub fn set_trading_base_url(&mut self, base_url: impl Into<String>) {
        self.trading_base_url = base_url.into();
    }

    /// Overrides the Market Data API base URL.
    pub fn set_data_base_url(&mut self, base_url: impl Into<String>) {
        self.data_base_url = base_url.into();
    }

    /// Returns true when the client was constructed with credentials.
    #[must_use]
    pub const fn has_credentials(&self) -> bool {
        self.has_credentials
    }

    /// Returns the cancellation token used to abort in-flight retries.
    #[must_use]
    pub const fn cancellation_token(&self) -> &CancellationToken {
        &self.cancellation_token
    }

    /// Cancels any in-flight retry loops.
    pub fn cancel(&self) {
        self.cancellation_token.cancel();
    }

    fn base_url(&self, target: ApiTarget) -> &str {
        match target {
            ApiTarget::Trading => &self.trading_base_url,
            ApiTarget::Data => &self.data_base_url,
        }
    }

    fn build_url(&self, target: ApiTarget, path: &str, query: Option<String>) -> String {
        let base = self.base_url(target).trim_end_matches('/');
        match query {
            Some(query) if !query.is_empty() => format!("{base}{path}?{query}"),
            _ => format!("{base}{path}"),
        }
    }

    fn require_credentials(&self) -> Result<()> {
        if self.has_credentials {
            Ok(())
        } else {
            Err(Error::auth("No credentials configured"))
        }
    }

    fn parse_response<T: DeserializeOwned>(response: &HttpResponse) -> Result<T> {
        if !response.status.is_success() {
            return Err(Error::from_http_status(
                response.status.as_u16(),
                &response.body,
            ));
        }

        serde_json::from_slice(&response.body).map_err(Error::Serde)
    }

    /// Sends a request, retrying transient failures on idempotent methods only.
    ///
    /// A replayed `POST`, `PATCH`, or `DELETE` against the Trading API could submit, replace,
    /// or cancel an order twice, so retries are gated to `GET` regardless of the error class.
    async fn send_request<T: DeserializeOwned>(
        &self,
        method: Method,
        target: ApiTarget,
        path: &str,
        query: Option<String>,
        body: Option<Vec<u8>>,
    ) -> Result<T> {
        let url = self.build_url(target, path, query);
        let operation_name = format!("{method} {path}");
        let keys = vec![target.rate_limit_key().to_string()];
        let is_idempotent = method == Method::GET;

        let operation = || {
            let method = method.clone();
            let url = url.clone();
            let body = body.clone();
            let keys = keys.clone();

            async move {
                let response = self
                    .client
                    .request(method, url, None, None, body, None, Some(keys))
                    .await
                    .map_err(|e| Error::from_http_client(&e))?;

                Self::parse_response(&response)
            }
        };

        let should_retry = move |err: &Error| is_idempotent && err.is_retryable();

        self.retry_manager
            .execute_with_retry_with_cancel(
                &operation_name,
                operation,
                should_retry,
                |e| Error::transport(e.to_string()),
                &self.cancellation_token,
            )
            .await
    }

    /// Sends a request whose response carries no body.
    ///
    /// Retries are gated to `GET` as elsewhere, so this never replays a mutating request.
    async fn send_request_no_content(
        &self,
        method: Method,
        target: ApiTarget,
        path: &str,
        query: Option<String>,
    ) -> Result<()> {
        let url = self.build_url(target, path, query);
        let operation_name = format!("{method} {path}");
        let keys = vec![target.rate_limit_key().to_string()];
        let is_idempotent = method == Method::GET;

        let operation = || {
            let method = method.clone();
            let url = url.clone();
            let keys = keys.clone();

            async move {
                let response = self
                    .client
                    .request(method, url, None, None, None, None, Some(keys))
                    .await
                    .map_err(|e| Error::from_http_client(&e))?;

                if response.status.is_success() {
                    Ok(())
                } else {
                    Err(Error::from_http_status(
                        response.status.as_u16(),
                        &response.body,
                    ))
                }
            }
        };

        let should_retry = move |err: &Error| is_idempotent && err.is_retryable();

        self.retry_manager
            .execute_with_retry_with_cancel(
                &operation_name,
                operation,
                should_retry,
                |e| Error::transport(e.to_string()),
                &self.cancellation_token,
            )
            .await
    }

    fn encode_query<P: Serialize>(params: &P) -> Result<Option<String>> {
        let encoded = serde_urlencoded::to_string(params)
            .map_err(|e| Error::bad_request(format!("Failed to encode query parameters: {e}")))?;
        Ok((!encoded.is_empty()).then_some(encoded))
    }

    /// Requests the market clock and current session state.
    ///
    /// # Errors
    ///
    /// Returns an error if credentials are missing or the request fails.
    pub async fn get_clock(&self) -> Result<Clock> {
        self.require_credentials()?;
        let path = format!("{REST_TRADING_PATH}/clock");
        self.send_request(Method::GET, ApiTarget::Trading, &path, None, None)
            .await
    }

    /// Requests the trading account.
    ///
    /// # Errors
    ///
    /// Returns an error if credentials are missing or the request fails.
    pub async fn get_account(&self) -> Result<Account> {
        self.require_credentials()?;
        let path = format!("{REST_TRADING_PATH}/account");
        self.send_request(Method::GET, ApiTarget::Trading, &path, None, None)
            .await
    }

    /// Requests the tradable asset list.
    ///
    /// # Errors
    ///
    /// Returns an error if credentials are missing, the parameters cannot be encoded, or the
    /// request fails.
    pub async fn list_assets(&self, params: &ListAssetsParams) -> Result<Vec<Asset>> {
        self.require_credentials()?;
        let path = format!("{REST_TRADING_PATH}/assets");
        let query = Self::encode_query(params)?;
        self.send_request(Method::GET, ApiTarget::Trading, &path, query, None)
            .await
    }

    /// Lists orders.
    ///
    /// # Errors
    ///
    /// Returns an error if credentials are missing or the request fails.
    pub async fn list_orders(&self, params: &ListOrdersParams) -> Result<Vec<AlpacaOrder>> {
        self.require_credentials()?;
        let path = format!("{REST_TRADING_PATH}/orders");
        let query = Self::encode_query(params)?;
        self.send_request(Method::GET, ApiTarget::Trading, &path, query, None)
            .await
    }

    /// Requests a single order by venue identifier.
    ///
    /// # Errors
    ///
    /// Returns an error if credentials are missing or the request fails.
    pub async fn get_order(&self, venue_order_id: &str) -> Result<AlpacaOrder> {
        self.require_credentials()?;
        let path = format!("{REST_TRADING_PATH}/orders/{venue_order_id}");
        self.send_request(Method::GET, ApiTarget::Trading, &path, None, None)
            .await
    }

    /// Lists open positions.
    ///
    /// # Errors
    ///
    /// Returns an error if credentials are missing or the request fails.
    pub async fn list_positions(&self) -> Result<Vec<AlpacaPosition>> {
        self.require_credentials()?;
        let path = format!("{REST_TRADING_PATH}/positions");
        self.send_request(Method::GET, ApiTarget::Trading, &path, None, None)
            .await
    }

    /// Submits an order.
    ///
    /// Not retried: a replayed submission would place a second order.
    ///
    /// # Errors
    ///
    /// Returns an error if credentials are missing, the body cannot be encoded, or the venue
    /// rejects the order.
    pub async fn submit_order(&self, request: &SubmitOrderRequest) -> Result<AlpacaOrder> {
        self.require_credentials()?;
        let path = format!("{REST_TRADING_PATH}/orders");
        let body = serde_json::to_vec(request)?;
        self.send_request(Method::POST, ApiTarget::Trading, &path, None, Some(body))
            .await
    }

    /// Replaces an order.
    ///
    /// The venue answers with a **new** order carrying a new identifier and moves the original to
    /// `replaced`. Callers must follow that identifier or they will track an order that no longer
    /// receives updates.
    ///
    /// # Errors
    ///
    /// Returns an error if credentials are missing, the body cannot be encoded, or the venue
    /// rejects the amendment.
    pub async fn replace_order(
        &self,
        venue_order_id: &str,
        request: &ReplaceOrderRequest,
    ) -> Result<AlpacaOrder> {
        self.require_credentials()?;
        let path = format!("{REST_TRADING_PATH}/orders/{venue_order_id}");
        let body = serde_json::to_vec(request)?;
        self.send_request(Method::PATCH, ApiTarget::Trading, &path, None, Some(body))
            .await
    }

    /// Cancels an order.
    ///
    /// The venue answers `204 No Content`, so there is no body to decode.
    ///
    /// # Errors
    ///
    /// Returns an error if credentials are missing or the request fails.
    pub async fn cancel_order(&self, venue_order_id: &str) -> Result<()> {
        self.require_credentials()?;
        let path = format!("{REST_TRADING_PATH}/orders/{venue_order_id}");
        self.send_request_no_content(Method::DELETE, ApiTarget::Trading, &path, None)
            .await
    }

    /// Cancels every open order.
    ///
    /// # Errors
    ///
    /// Returns an error if credentials are missing or the request fails.
    pub async fn cancel_all_orders(&self) -> Result<()> {
        self.require_credentials()?;
        let path = format!("{REST_TRADING_PATH}/orders");
        self.send_request_no_content(Method::DELETE, ApiTarget::Trading, &path, None)
            .await
    }

    /// Requests the venue trading calendar.
    ///
    /// Only days the market opens are returned; weekends and holidays are absent.
    ///
    /// # Errors
    ///
    /// Returns an error if credentials are missing or the request fails.
    pub async fn get_calendar(&self, params: &CalendarParams) -> Result<Vec<CalendarDay>> {
        self.require_credentials()?;
        let path = format!("{REST_TRADING_PATH}/calendar");
        let query = Self::encode_query(params)?;
        self.send_request(Method::GET, ApiTarget::Trading, &path, query, None)
            .await
    }

    /// Requests one page of bars.
    ///
    /// Bars are keyed by symbol, and `next_page_token` is set when more data is available. Use
    /// [`Self::get_bars_all_pages`] to follow the cursor.
    ///
    /// # Errors
    ///
    /// Returns an error if credentials are missing, the parameters cannot be encoded, or the
    /// request fails.
    pub async fn get_bars(&self, params: &BarsParams) -> Result<BarsResponse> {
        self.require_credentials()?;
        let path = format!("{REST_DATA_STOCKS_PATH}/bars");
        let query = Self::encode_query(params)?;
        self.send_request(Method::GET, ApiTarget::Data, &path, query, None)
            .await
    }

    /// Requests bars, following the pagination cursor until the window is exhausted.
    ///
    /// `max_pages` bounds the walk so a cursor that fails to advance cannot loop forever. When the
    /// bound is reached the bars gathered so far are returned and the shortfall is logged, because
    /// silently returning a truncated series would read as a complete one.
    ///
    /// # Errors
    ///
    /// Returns an error if any page request fails.
    pub async fn get_bars_all_pages(
        &self,
        params: &BarsParams,
        max_pages: usize,
    ) -> Result<HashMap<String, Vec<AlpacaBar>>> {
        let mut merged: HashMap<String, Vec<AlpacaBar>> = HashMap::new();
        let mut page_params = params.clone();

        for page in 0..max_pages {
            let response = self.get_bars(&page_params).await?;
            for (symbol, bars) in response.bars {
                merged.entry(symbol).or_default().extend(bars);
            }

            match response.next_page_token {
                Some(token) => page_params = page_params.clone().with_page_token(token),
                None => return Ok(merged),
            }

            if page + 1 == max_pages {
                log::warn!(
                    "Alpaca bars pagination stopped at the {max_pages}-page limit for '{}'; the result is truncated",
                    params.symbols
                );
            }
        }

        Ok(merged)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn client() -> AlpacaRawHttpClient {
        AlpacaRawHttpClient::new(AlpacaEnvironment::Paper, 10, None, None).unwrap()
    }

    #[rstest]
    fn test_unauthenticated_client_reports_no_credentials() {
        assert!(!client().has_credentials());
    }

    #[rstest]
    fn test_credentialed_client_reports_credentials() {
        let credential = AlpacaCredential::new("key".to_string(), "secret".to_string());
        let client = AlpacaRawHttpClient::with_credentials(
            credential,
            AlpacaEnvironment::Paper,
            10,
            None,
            None,
        )
        .unwrap();
        assert!(client.has_credentials());
    }

    #[rstest]
    fn test_paper_client_targets_paper_trading_host() {
        let client = client();
        assert_eq!(client.base_url(ApiTarget::Trading), REST_URL_PAPER_EXPECTED);
    }

    const REST_URL_PAPER_EXPECTED: &str = "https://paper-api.alpaca.markets";

    #[rstest]
    fn test_live_client_targets_live_trading_host() {
        let client = AlpacaRawHttpClient::new(AlpacaEnvironment::Live, 10, None, None).unwrap();
        assert_eq!(
            client.base_url(ApiTarget::Trading),
            "https://api.alpaca.markets"
        );
    }

    #[rstest]
    fn test_data_host_is_shared_across_environments() {
        let paper = client();
        let live = AlpacaRawHttpClient::new(AlpacaEnvironment::Live, 10, None, None).unwrap();
        assert_eq!(
            paper.base_url(ApiTarget::Data),
            live.base_url(ApiTarget::Data)
        );
    }

    #[rstest]
    fn test_build_url_without_query() {
        let client = client();
        assert_eq!(
            client.build_url(ApiTarget::Trading, "/v2/account", None),
            "https://paper-api.alpaca.markets/v2/account"
        );
    }

    #[rstest]
    fn test_build_url_with_query() {
        let client = client();
        assert_eq!(
            client.build_url(
                ApiTarget::Trading,
                "/v2/assets",
                Some("status=active".to_string())
            ),
            "https://paper-api.alpaca.markets/v2/assets?status=active"
        );
    }

    #[rstest]
    fn test_build_url_ignores_empty_query() {
        let client = client();
        assert_eq!(
            client.build_url(ApiTarget::Data, "/v2/stocks/AAPL/bars", Some(String::new())),
            "https://data.alpaca.markets/v2/stocks/AAPL/bars"
        );
    }

    #[rstest]
    fn test_build_url_does_not_double_slash_on_overridden_base() {
        let mut client = client();
        client.set_trading_base_url("http://localhost:8080/");
        assert_eq!(
            client.build_url(ApiTarget::Trading, "/v2/account", None),
            "http://localhost:8080/v2/account"
        );
    }

    #[rstest]
    #[case(ApiTarget::Trading, RATE_LIMIT_KEY_TRADING)]
    #[case(ApiTarget::Data, RATE_LIMIT_KEY_DATA)]
    fn test_rate_limit_keys_are_distinct_per_target(
        #[case] target: ApiTarget,
        #[case] expected: &str,
    ) {
        assert_eq!(target.rate_limit_key(), expected);
    }

    #[rstest]
    fn test_encode_query_omits_empty() {
        let encoded = AlpacaRawHttpClient::encode_query(&ListAssetsParams::default()).unwrap();
        assert!(encoded.is_none());
    }

    #[rstest]
    fn test_encode_query_returns_pairs() {
        let encoded =
            AlpacaRawHttpClient::encode_query(&ListAssetsParams::active_us_equities()).unwrap();
        assert_eq!(
            encoded.as_deref(),
            Some("status=active&asset_class=us_equity")
        );
    }

    #[tokio::test]
    async fn test_endpoints_reject_missing_credentials_before_dispatch() {
        let client = client();
        assert!(matches!(client.get_account().await, Err(Error::Auth(_))));
        assert!(matches!(client.get_clock().await, Err(Error::Auth(_))));
        assert!(matches!(
            client.list_assets(&ListAssetsParams::default()).await,
            Err(Error::Auth(_))
        ));
    }
}
