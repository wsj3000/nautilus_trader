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

//! Market data client for Alpaca.
//!
//! # Transport
//!
//! Bars are polled over REST rather than streamed. Alpaca allows a single concurrent market data
//! stream per account, and that slot is in use elsewhere, so the adapter keeps off it.
//!
//! Polling is affordable because one request covers every subscribed symbol: the cost scales with
//! the number of distinct timeframes, not with the size of the universe.
//!
//! Each poll asks for a rolling window rather than the newest bar alone. A poll that fails or
//! arrives late is then recovered by the next one, where fetching only the latest bar would leave
//! a permanent hole. Bars already emitted are filtered out by timestamp before dispatch.
//!
//! # What this transport cannot do
//!
//! Quotes, trades, and order book data are not available: polling cannot reconstruct a tick
//! stream, and requesting one would silently deliver something other than what was asked for, so
//! those subscriptions are refused outright.
//!
//! Trading halts and LULD events are likewise unavailable, since the venue publishes them only on
//! the market data stream. A strategy that must react to halts needs the WebSocket transport.
//!
//! # Sessions
//!
//! The feed follows the clock: the SIP feed covers 04:00-20:00 US Eastern and the Blue Ocean
//! overnight feed covers 20:00-04:00, giving continuous 24/5 coverage from one connection. The
//! switch is driven by the venue calendar, which is also the only source of holidays.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use ahash::AHashMap;
use arc_swap::ArcSwap;
use async_trait::async_trait;
use nautilus_common::{
    clients::DataClient,
    live::{runner::get_data_event_sender, runtime::get_runtime},
    messages::{
        DataEvent,
        data::{
            RequestBars, RequestInstruments, SubscribeBars, SubscribeBookDeltas,
            SubscribeBookDepth10, SubscribeInstrumentStatus, SubscribeInstruments, SubscribeQuotes,
            SubscribeTrades, UnsubscribeBars, UnsubscribeInstruments,
        },
    },
};
use nautilus_core::{
    UnixNanos,
    time::{AtomicTime, get_atomic_clock_realtime},
};
use nautilus_model::{
    data::{Bar, BarType, Data},
    identifiers::{ClientId, Venue},
};
use tokio::{sync::Mutex, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::{
    common::{
        consts::ALPACA_VENUE, credential::AlpacaCredential, enums::AlpacaDataFeed,
        session::SessionCalendar,
    },
    config::AlpacaDataClientConfig,
    http::{
        client::AlpacaRawHttpClient,
        parse::{bar_spec_to_timeframe, parse_bar},
        query::{BarsParams, CalendarParams},
    },
    provider::AlpacaInstrumentProvider,
};

/// Maximum pages walked when following a bars cursor.
const MAX_BAR_PAGES: usize = 20;

/// Tracks which bars have already been emitted, so a rolling window does not republish them.
#[derive(Debug, Default)]
struct BarSubscriptions {
    /// Subscribed bar types and the close timestamp of the newest bar dispatched for each.
    watermarks: AHashMap<BarType, Option<UnixNanos>>,
}

impl BarSubscriptions {
    fn insert(&mut self, bar_type: BarType) -> bool {
        if self.watermarks.contains_key(&bar_type) {
            return false;
        }
        self.watermarks.insert(bar_type, None);
        true
    }

    fn remove(&mut self, bar_type: &BarType) -> bool {
        self.watermarks.remove(bar_type).is_some()
    }

    fn is_empty(&self) -> bool {
        self.watermarks.is_empty()
    }

    /// Returns the subscribed bar types grouped by their venue timeframe.
    ///
    /// Grouping is what makes polling cheap: every symbol sharing a timeframe is fetched by one
    /// request.
    fn by_timeframe(&self) -> AHashMap<String, Vec<BarType>> {
        let mut groups: AHashMap<String, Vec<BarType>> = AHashMap::new();
        for bar_type in self.watermarks.keys() {
            match bar_spec_to_timeframe(bar_type.spec()) {
                Ok(timeframe) => groups.entry(timeframe).or_default().push(*bar_type),
                Err(e) => log::error!("Cannot poll {bar_type}: {e}"),
            }
        }
        groups
    }

    /// Returns true when the bar is newer than anything already dispatched for its type.
    fn is_new(&self, bar_type: &BarType, ts_event: UnixNanos) -> bool {
        match self.watermarks.get(bar_type) {
            Some(Some(seen)) => ts_event > *seen,
            Some(None) => true,
            None => false, // Unsubscribed while the request was in flight
        }
    }

    fn advance(&mut self, bar_type: &BarType, ts_event: UnixNanos) {
        if let Some(slot) = self.watermarks.get_mut(bar_type) {
            let newer = slot.is_none_or(|seen| ts_event > seen);
            if newer {
                *slot = Some(ts_event);
            }
        }
    }
}

/// The shared state one polling tick needs.
///
/// Bundled rather than threaded through as loose arguments so the polling loop can own a single
/// cheap-to-clone handle.
#[derive(Debug, Clone)]
struct PollContext {
    http_client: Arc<AlpacaRawHttpClient>,
    subscriptions: Arc<Mutex<BarSubscriptions>>,
    data_sender: tokio::sync::mpsc::UnboundedSender<DataEvent>,
    clock: &'static AtomicTime,
    window_mins: u64,
    price_precision: u8,
}

impl PollContext {
    /// Polls one timeframe group and dispatches any bars not yet seen.
    async fn poll_group(&self, timeframe: &str, bar_types: &[BarType], feed: AlpacaDataFeed) {
        let now = self.clock.get_time_ns();
        let (Some(start), Some(end)) = (window_start(now, self.window_mins), format_rfc3339(now))
        else {
            log::error!("Cannot compute the Alpaca poll window");
            return;
        };

        let symbols: Vec<String> = bar_types
            .iter()
            .map(|bt| bt.instrument_id().symbol.to_string())
            .collect();

        let params = BarsParams::new(&symbols, timeframe)
            .with_feed(feed)
            .with_window(start, end);

        let bars_by_symbol = match self
            .http_client
            .get_bars_all_pages(&params, MAX_BAR_PAGES)
            .await
        {
            Ok(bars) => bars,
            Err(e) => {
                // A failed poll is recoverable: the rolling window means the next one refetches
                // whatever this one missed.
                log::warn!("Alpaca bars poll failed for timeframe {timeframe}: {e}");
                return;
            }
        };

        let mut guard = self.subscriptions.lock().await;
        for bar_type in bar_types {
            let symbol = bar_type.instrument_id().symbol.to_string();
            let Some(venue_bars) = bars_by_symbol.get(&symbol) else {
                continue;
            };

            let mut parsed: Vec<Bar> = Vec::new();
            for venue_bar in venue_bars {
                match parse_bar(venue_bar, *bar_type, self.price_precision, now) {
                    Ok(bar) => parsed.push(bar),
                    Err(e) => log::warn!("Skipping malformed Alpaca bar for {symbol}: {e}"),
                }
            }
            // The venue returns ascending timestamps, but sort so the watermark cannot be
            // advanced past a bar that has not been dispatched yet.
            parsed.sort_by_key(|bar| bar.ts_event);

            for bar in parsed {
                if !guard.is_new(bar_type, bar.ts_event) {
                    continue;
                }
                if let Err(e) = self.data_sender.send(DataEvent::Data(Data::Bar(bar))) {
                    log::error!("Failed to dispatch Alpaca bar: {e}");
                    return;
                }
                guard.advance(bar_type, bar.ts_event);
            }
        }
    }
}

/// Market data client for Alpaca US equities.
#[derive(Debug)]
pub struct AlpacaDataClient {
    client_id: ClientId,
    config: AlpacaDataClientConfig,
    http_client: Arc<AlpacaRawHttpClient>,
    provider: AlpacaInstrumentProvider,
    calendar: Arc<ArcSwap<SessionCalendar>>,
    subscriptions: Arc<Mutex<BarSubscriptions>>,
    is_connected: Arc<AtomicBool>,
    cancellation_token: CancellationToken,
    tasks: Vec<JoinHandle<()>>,
    data_sender: tokio::sync::mpsc::UnboundedSender<DataEvent>,
    clock: &'static AtomicTime,
}

impl AlpacaDataClient {
    /// Creates a new [`AlpacaDataClient`].
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP client cannot be created.
    pub fn new(client_id: ClientId, config: AlpacaDataClientConfig) -> anyhow::Result<Self> {
        let credential =
            AlpacaCredential::resolve(config.api_key.as_deref(), config.api_secret.as_deref());

        let mut http_client = match credential {
            Some(credential) => AlpacaRawHttpClient::with_credentials(
                credential,
                config.environment,
                config.http_timeout_secs,
                config.proxy_url.clone(),
                None,
            )?,
            None => AlpacaRawHttpClient::new(
                config.environment,
                config.http_timeout_secs,
                config.proxy_url.clone(),
                None,
            )?,
        };

        if let Some(url) = config.base_url_rest.clone() {
            http_client.set_data_base_url(url);
        }
        if let Some(url) = config.base_url_trading.clone() {
            http_client.set_trading_base_url(url);
        }

        let http_client = Arc::new(http_client);
        let provider =
            AlpacaInstrumentProvider::from_config(http_client.clone(), &config.instrument_provider);

        Ok(Self {
            client_id,
            config,
            http_client,
            provider,
            calendar: Arc::new(ArcSwap::from_pointee(SessionCalendar::new())),
            subscriptions: Arc::new(Mutex::new(BarSubscriptions::default())),
            is_connected: Arc::new(AtomicBool::new(false)),
            cancellation_token: CancellationToken::new(),
            tasks: Vec::new(),
            data_sender: get_data_event_sender(),
            clock: get_atomic_clock_realtime(),
        })
    }

    /// Returns the instrument provider.
    #[must_use]
    pub const fn provider(&self) -> &AlpacaInstrumentProvider {
        &self.provider
    }

    /// Returns the market data feed for the current instant.
    ///
    /// Falls back to the configured feed when the calendar reports no session, so a poll running
    /// just outside a session boundary still targets a sensible endpoint rather than aborting.
    #[must_use]
    pub fn current_feed(&self) -> AlpacaDataFeed {
        self.calendar
            .load()
            .feed_at(self.clock.get_time_ns())
            .unwrap_or(self.config.feed)
    }

    /// Loads the trading calendar covering the configured look-ahead.
    async fn load_calendar(&self) -> anyhow::Result<usize> {
        let today = jiff::Timestamp::from_nanosecond(i128::from(self.clock.get_time_ns().as_u64()))
            .map(|ts| crate::common::session::MARKET_TZ.to_datetime(ts).date())
            .map_err(|e| anyhow::anyhow!("Cannot resolve the current market date: {e}"))?;

        // Reach back a day as well: the overnight session in progress belongs to a date that may
        // already have passed.
        let start = today.yesterday().unwrap_or(today);
        let end = today
            .checked_add(jiff::Span::new().days(i64::from(self.config.calendar_lookahead_days)))
            .unwrap_or(today);

        let days = self
            .http_client
            .get_calendar(&CalendarParams {
                start: Some(start.to_string()),
                end: Some(end.to_string()),
            })
            .await?;

        let calendar = SessionCalendar::from_calendar_days(&days);
        let count = calendar.len();
        self.calendar.store(Arc::new(calendar));
        Ok(count)
    }

    /// Spawns the polling loop.
    fn spawn_poll_task(&mut self) {
        let context = PollContext {
            http_client: self.http_client.clone(),
            subscriptions: self.subscriptions.clone(),
            data_sender: self.data_sender.clone(),
            clock: self.clock,
            window_mins: self.config.poll_window_mins.max(1),
            price_precision: crate::common::reg_nms::PRICE_PRECISION,
        };
        let calendar = self.calendar.clone();
        let token = self.cancellation_token.clone();
        let interval = std::time::Duration::from_secs(self.config.poll_interval_secs.max(1));
        let fallback_feed = self.config.feed;

        let handle = get_runtime().spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    () = token.cancelled() => {
                        log::debug!("Alpaca poll task cancelled");
                        return;
                    }
                    _ = ticker.tick() => {}
                }

                let groups = {
                    let guard = context.subscriptions.lock().await;
                    if guard.is_empty() {
                        continue;
                    }
                    guard.by_timeframe()
                };

                let feed = calendar
                    .load()
                    .feed_at(context.clock.get_time_ns())
                    .unwrap_or(fallback_feed);

                for (timeframe, bar_types) in groups {
                    context.poll_group(&timeframe, &bar_types, feed).await;
                }
            }
        });

        self.tasks.push(handle);
    }

    fn abort_tasks(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }
}

/// Formats an instant as an RFC 3339 UTC timestamp.
fn format_rfc3339(ts: UnixNanos) -> Option<String> {
    jiff::Timestamp::from_nanosecond(i128::from(ts.as_u64()))
        .ok()
        .map(|t| t.to_string())
}

/// Builds the error returned for subscriptions this transport cannot serve.
///
/// Refusing explicitly rather than accepting silently means a strategy asking for tick data fails
/// at subscription time, instead of waiting indefinitely for data that will never arrive.
fn unsupported(what: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "Alpaca {what} are not available over the REST polling transport; they require the market \
         data WebSocket"
    )
}

/// Returns the RFC 3339 start of a rolling window ending at `now`.
fn window_start(now: UnixNanos, window_mins: u64) -> Option<String> {
    let span_nanos = window_mins.checked_mul(60)?.checked_mul(1_000_000_000)?;
    format_rfc3339(UnixNanos::from(now.as_u64().saturating_sub(span_nanos)))
}

#[async_trait(?Send)]
impl DataClient for AlpacaDataClient {
    fn client_id(&self) -> ClientId {
        self.client_id
    }

    fn venue(&self) -> Option<Venue> {
        Some(*ALPACA_VENUE)
    }

    fn start(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        self.cancellation_token.cancel();
        self.abort_tasks();
        Ok(())
    }

    fn reset(&mut self) -> anyhow::Result<()> {
        self.abort_tasks();
        self.cancellation_token = CancellationToken::new();
        self.subscriptions = Arc::new(Mutex::new(BarSubscriptions::default()));
        self.calendar.store(Arc::new(SessionCalendar::new()));
        self.is_connected.store(false, Ordering::Relaxed);
        Ok(())
    }

    fn dispose(&mut self) -> anyhow::Result<()> {
        self.cancellation_token.cancel();
        self.abort_tasks();
        self.is_connected.store(false, Ordering::Relaxed);
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.is_connected.load(Ordering::Relaxed)
    }

    fn is_disconnected(&self) -> bool {
        !self.is_connected()
    }

    async fn connect(&mut self) -> anyhow::Result<()> {
        if self.is_connected() {
            return Ok(());
        }

        let instruments = self.provider.load_all().await?;
        log::info!("Loaded {} Alpaca instruments", instruments.len());

        for instrument in instruments {
            if let Err(e) = self.data_sender.send(DataEvent::Instrument(instrument)) {
                log::error!("Failed to dispatch Alpaca instrument: {e}");
            }
        }

        // The calendar drives session detection and the feed switch. Without it every instant
        // resolves as closed, so a failure here is fatal to 24/5 coverage rather than cosmetic.
        let days = self.load_calendar().await?;
        log::info!("Loaded {days} Alpaca trading days");

        self.spawn_poll_task();
        self.is_connected.store(true, Ordering::Relaxed);
        Ok(())
    }

    async fn disconnect(&mut self) -> anyhow::Result<()> {
        self.cancellation_token.cancel();
        self.abort_tasks();
        self.is_connected.store(false, Ordering::Relaxed);
        Ok(())
    }

    fn subscribe_instruments(&mut self, _cmd: SubscribeInstruments) -> anyhow::Result<()> {
        // Instruments are dispatched on connect and refreshed by the provider; there is no
        // incremental instrument stream to subscribe to.
        Ok(())
    }

    fn unsubscribe_instruments(&mut self, _cmd: &UnsubscribeInstruments) -> anyhow::Result<()> {
        Ok(())
    }

    fn subscribe_bars(&mut self, cmd: SubscribeBars) -> anyhow::Result<()> {
        let bar_type = cmd.bar_type;

        // Reject an unsupported timeframe here rather than letting the poll loop log it every
        // tick: the caller can act on an error, but not on a log line.
        bar_spec_to_timeframe(bar_type.spec())?;

        if self.provider.get(&bar_type.instrument_id()).is_none() {
            anyhow::bail!(
                "Cannot subscribe to {bar_type}: instrument {} is not loaded",
                bar_type.instrument_id()
            );
        }

        let subscriptions = self.subscriptions.clone();
        get_runtime().spawn(async move {
            if subscriptions.lock().await.insert(bar_type) {
                log::debug!("Subscribed to Alpaca bars for {bar_type}");
            }
        });

        Ok(())
    }

    fn unsubscribe_bars(&mut self, cmd: &UnsubscribeBars) -> anyhow::Result<()> {
        let bar_type = cmd.bar_type;
        let subscriptions = self.subscriptions.clone();
        get_runtime().spawn(async move {
            if subscriptions.lock().await.remove(&bar_type) {
                log::debug!("Unsubscribed from Alpaca bars for {bar_type}");
            }
        });
        Ok(())
    }

    fn subscribe_quotes(&mut self, _cmd: SubscribeQuotes) -> anyhow::Result<()> {
        Err(unsupported("quotes"))
    }

    fn subscribe_trades(&mut self, _cmd: SubscribeTrades) -> anyhow::Result<()> {
        Err(unsupported("trades"))
    }

    fn subscribe_book_deltas(&mut self, _cmd: SubscribeBookDeltas) -> anyhow::Result<()> {
        Err(unsupported("order book deltas"))
    }

    fn subscribe_book_depth10(&mut self, _cmd: SubscribeBookDepth10) -> anyhow::Result<()> {
        Err(unsupported("order book depth"))
    }

    fn subscribe_instrument_status(
        &mut self,
        _cmd: SubscribeInstrumentStatus,
    ) -> anyhow::Result<()> {
        // Halts and LULD events are published only on the market data stream.
        Err(unsupported("instrument status updates"))
    }

    fn request_instruments(&self, request: RequestInstruments) -> anyhow::Result<()> {
        let instruments: Vec<_> = self
            .provider
            .instruments()
            .load()
            .values()
            .cloned()
            .collect();

        log::debug!(
            "Responding to instruments request {} with {} instruments",
            request.request_id,
            instruments.len()
        );

        for instrument in instruments {
            if let Err(e) = self.data_sender.send(DataEvent::Instrument(instrument)) {
                log::error!("Failed to dispatch Alpaca instrument: {e}");
            }
        }
        Ok(())
    }

    fn request_bars(&self, request: RequestBars) -> anyhow::Result<()> {
        let bar_type = request.bar_type;
        let timeframe = bar_spec_to_timeframe(bar_type.spec())?;
        let symbol = bar_type.instrument_id().symbol.to_string();
        let http_client = self.http_client.clone();
        let data_sender = self.data_sender.clone();
        let clock = self.clock;
        let feed = self.current_feed();
        let price_precision = crate::common::reg_nms::PRICE_PRECISION;

        let mut params = BarsParams::new(std::slice::from_ref(&symbol), timeframe).with_feed(feed);
        if let Some(start) = request.start {
            params.start = Some(start.to_string());
        }
        if let Some(end) = request.end {
            params.end = Some(end.to_string());
        }
        if let Some(limit) = request.limit {
            params.limit = u32::try_from(limit.get()).ok();
        }

        get_runtime().spawn(async move {
            match http_client.get_bars_all_pages(&params, MAX_BAR_PAGES).await {
                Ok(bars_by_symbol) => {
                    let ts_init = clock.get_time_ns();
                    let Some(venue_bars) = bars_by_symbol.get(&symbol) else {
                        log::debug!("No Alpaca bars returned for {symbol}");
                        return;
                    };
                    for venue_bar in venue_bars {
                        match parse_bar(venue_bar, bar_type, price_precision, ts_init) {
                            Ok(bar) => {
                                if let Err(e) = data_sender.send(DataEvent::Data(Data::Bar(bar))) {
                                    log::error!("Failed to dispatch Alpaca bar: {e}");
                                    return;
                                }
                            }
                            Err(e) => log::warn!("Skipping malformed Alpaca bar: {e}"),
                        }
                    }
                }
                Err(e) => log::error!("Alpaca historical bars request failed: {e}"),
            }
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use nautilus_model::{
        data::BarSpecification,
        enums::{AggregationSource, BarAggregation, PriceType},
    };
    use rstest::rstest;

    use super::*;
    use crate::http::parse::instrument_id;

    fn bar_type(symbol: &str, step: usize, aggregation: BarAggregation) -> BarType {
        BarType::new(
            instrument_id(symbol),
            BarSpecification::new(step, aggregation, PriceType::Last),
            AggregationSource::External,
        )
    }

    fn minute(symbol: &str) -> BarType {
        bar_type(symbol, 1, BarAggregation::Minute)
    }

    #[rstest]
    fn test_insert_is_idempotent() {
        let mut subs = BarSubscriptions::default();
        assert!(subs.insert(minute("AAPL")));
        assert!(!subs.insert(minute("AAPL")));
    }

    #[rstest]
    fn test_remove_reports_whether_it_was_present() {
        let mut subs = BarSubscriptions::default();
        subs.insert(minute("AAPL"));
        assert!(subs.remove(&minute("AAPL")));
        assert!(!subs.remove(&minute("AAPL")));
        assert!(subs.is_empty());
    }

    #[rstest]
    fn test_symbols_sharing_a_timeframe_are_grouped_into_one_request() {
        let mut subs = BarSubscriptions::default();
        subs.insert(minute("AAPL"));
        subs.insert(minute("MSFT"));
        subs.insert(bar_type("AAPL", 5, BarAggregation::Minute));

        let groups = subs.by_timeframe();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups["1Min"].len(), 2);
        assert_eq!(groups["5Min"].len(), 1);
    }

    #[rstest]
    fn test_unpollable_timeframes_are_dropped_from_grouping() {
        let mut subs = BarSubscriptions::default();
        subs.insert(minute("AAPL"));
        subs.insert(bar_type("AAPL", 1, BarAggregation::Second));

        // The venue has no second bars, so that subscription cannot produce a request.
        let groups = subs.by_timeframe();
        assert_eq!(groups.len(), 1);
        assert!(groups.contains_key("1Min"));
    }

    #[rstest]
    fn test_first_bar_for_a_subscription_is_new() {
        let mut subs = BarSubscriptions::default();
        subs.insert(minute("AAPL"));
        assert!(subs.is_new(&minute("AAPL"), UnixNanos::from(100)));
    }

    #[rstest]
    fn test_rolling_window_does_not_republish_seen_bars() {
        let mut subs = BarSubscriptions::default();
        let bt = minute("AAPL");
        subs.insert(bt);
        subs.advance(&bt, UnixNanos::from(200));

        // Overlapping polls resend the same bars; only strictly newer ones may be dispatched.
        assert!(!subs.is_new(&bt, UnixNanos::from(100)));
        assert!(!subs.is_new(&bt, UnixNanos::from(200)));
        assert!(subs.is_new(&bt, UnixNanos::from(300)));
    }

    #[rstest]
    fn test_watermark_never_moves_backwards() {
        let mut subs = BarSubscriptions::default();
        let bt = minute("AAPL");
        subs.insert(bt);
        subs.advance(&bt, UnixNanos::from(300));
        subs.advance(&bt, UnixNanos::from(100));
        assert!(!subs.is_new(&bt, UnixNanos::from(200)));
    }

    #[rstest]
    fn test_bars_for_an_unsubscribed_type_are_never_new() {
        // A response can arrive after an unsubscribe; those bars must not be dispatched.
        let subs = BarSubscriptions::default();
        assert!(!subs.is_new(&minute("AAPL"), UnixNanos::from(100)));
    }

    #[rstest]
    fn test_advance_on_an_unsubscribed_type_is_a_no_op() {
        let mut subs = BarSubscriptions::default();
        subs.advance(&minute("AAPL"), UnixNanos::from(100));
        assert!(subs.is_empty());
    }

    #[rstest]
    fn test_watermarks_are_independent_per_bar_type() {
        let mut subs = BarSubscriptions::default();
        let aapl = minute("AAPL");
        let msft = minute("MSFT");
        subs.insert(aapl);
        subs.insert(msft);
        subs.advance(&aapl, UnixNanos::from(500));

        assert!(!subs.is_new(&aapl, UnixNanos::from(400)));
        assert!(subs.is_new(&msft, UnixNanos::from(400)));
    }

    #[rstest]
    fn test_window_start_precedes_now_by_the_configured_span() {
        let now = UnixNanos::from(1_000 * 60 * 1_000_000_000);
        let start = window_start(now, 5).unwrap();
        let now_str = format_rfc3339(now).unwrap();
        assert!(start < now_str, "{start} !< {now_str}");
    }

    #[rstest]
    fn test_window_start_saturates_at_the_epoch() {
        assert!(window_start(UnixNanos::from(0), 5).is_some());
    }

    #[rstest]
    fn test_rfc3339_formatting_round_trips() {
        let ts = UnixNanos::from(1_786_717_800_000_000_000);
        let formatted = format_rfc3339(ts).unwrap();
        let parsed = crate::http::parse::parse_rfc3339_nanos(&formatted).unwrap();
        assert_eq!(parsed, ts);
    }
}
