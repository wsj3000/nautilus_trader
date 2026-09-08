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

//! Execution client for Alpaca.
//!
//! # Amendments create new orders
//!
//! Alpaca implements an amendment as a replacement rather than an in-place edit: `PATCH` answers
//! with a **new** order under a new identifier and moves the original to `replaced`. Nautilus
//! expects an order to keep its venue identifier across a modification, so the two models do not
//! line up.
//!
//! The client therefore keeps a chain from the identifier the engine knows to the one currently
//! live at the venue, and resolves through it before every cancel, query, or amendment. Without
//! that, the first amendment would leave the engine addressing an order the venue no longer acts
//! on, and subsequent cancels would silently target a dead identifier.
//!
//! # Price validation
//!
//! Order prices are checked against Reg NMS Rule 612 before submission and **denied** rather than
//! rounded. Rounding a price the caller specified changes their order; the regulator's own
//! guidance is that such an order is rejected, and the venue rejects it too, so denying locally
//! produces the same outcome without spending a round trip or a rate limit slot.

use std::sync::Arc;

use ahash::AHashMap;
use anyhow::anyhow;
use async_trait::async_trait;
use nautilus_common::{
    clients::ExecutionClient,
    messages::execution::{
        BatchCancelOrders, CancelAllOrders, CancelOrder, GenerateFillReports,
        GenerateOrderStatusReport, GenerateOrderStatusReports, GeneratePositionStatusReports,
        ModifyOrder, QueryAccount, QueryOrder, SubmitOrder,
    },
};
use nautilus_core::{
    UnixNanos,
    time::{AtomicTime, get_atomic_clock_realtime},
};
use nautilus_live::{ExecutionClientCore, ExecutionEventEmitter};
use nautilus_model::{
    accounts::AccountAny,
    enums::{OmsType, OrderType},
    identifiers::{AccountId, ClientId, VenueOrderId},
    reports::{FillReport, OrderStatusReport, PositionStatusReport},
    types::{AccountBalance, Currency, MarginBalance},
};
use nautilus_network::backoff::ExponentialBackoff;
use rust_decimal::Decimal;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::{
    common::{
        consts::{
            ALPACA_VENUE, RECONNECT_BACKOFF_FACTOR, RECONNECT_BASE_BACKOFF, RECONNECT_JITTER_MS,
            RECONNECT_MAX_BACKOFF,
        },
        credential::AlpacaCredential,
        order_enums::{AlpacaOrderSide, AlpacaOrderType, AlpacaTimeInForce},
        reg_nms,
    },
    config::AlpacaExecClientConfig,
    http::{
        client::AlpacaRawHttpClient,
        models::{AlpacaOrder, AlpacaPosition},
        parse_exec::{
            parse_decimal, parse_fill_activity_report, parse_fill_report,
            parse_order_status_report, parse_position_status_report,
        },
        query::{ActivitiesParams, ListOrdersParams, ReplaceOrderRequest, SubmitOrderRequest},
    },
    websocket::{
        client::{AlpacaTradingStream, connect_trading_stream},
        messages::{AlpacaTradeEvent, TradeUpdate},
    },
};

/// Maximum pages walked when following the activities cursor.
///
/// The venue caps a page at 100, so this admits 5,000 fills. Reconciliation asks for a bounded
/// window rather than the whole history, and a busier account than that wants a narrower window
/// rather than a larger limit here.
const MAX_ACTIVITY_PAGES: usize = 50;

/// Maximum pages walked when following the orders time cursor.
///
/// The venue returns at most 500 orders per page, so this admits 10,000. Beyond that the caller
/// wants a narrower window rather than a larger limit here.
const MAX_ORDER_PAGES: usize = 20;

/// Tracks the venue identifier an order currently trades under.
///
/// Nautilus addresses an order by the identifier it was first accepted with, while Alpaca issues a
/// new one on every amendment. The chain maps the former to the latter.
#[derive(Debug, Default)]
struct ReplacementChain {
    current: AHashMap<VenueOrderId, VenueOrderId>,
}

impl ReplacementChain {
    /// Records that `original` now trades as `replacement`.
    ///
    /// Chains collapse rather than nest: amending twice leaves the first identifier pointing
    /// straight at the third, so resolution never walks more than one hop.
    fn record(&mut self, original: VenueOrderId, replacement: VenueOrderId) {
        let root = self
            .current
            .iter()
            .find(|(_, live)| **live == original)
            .map_or(original, |(root, _)| *root);
        self.current.insert(root, replacement);
    }

    /// Returns the identifier currently live at the venue.
    fn resolve(&self, venue_order_id: VenueOrderId) -> VenueOrderId {
        self.current
            .get(&venue_order_id)
            .copied()
            .unwrap_or(venue_order_id)
    }

    fn forget(&mut self, venue_order_id: VenueOrderId) {
        self.current.remove(&venue_order_id);
    }
}

/// Execution client for Alpaca US equities.
#[derive(Debug)]
pub struct AlpacaExecutionClient {
    core: ExecutionClientCore,
    emitter: ExecutionEventEmitter,
    config: AlpacaExecClientConfig,
    http_client: Arc<AlpacaRawHttpClient>,
    chain: Arc<Mutex<ReplacementChain>>,
    account: Arc<Mutex<Option<AccountAny>>>,
    cancellation_token: CancellationToken,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    clock: &'static AtomicTime,
}

impl AlpacaExecutionClient {
    fn conflicting_working_client_order_ids<'a>(
        current_client_order_id: &str,
        open_orders: &'a [AlpacaOrder],
    ) -> Vec<&'a str> {
        open_orders
            .iter()
            .filter(|working| {
                working.client_order_id != current_client_order_id && !working.status.is_terminal()
            })
            .map(|working| working.client_order_id.as_str())
            .collect()
    }

    /// Creates a new [`AlpacaExecutionClient`].
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP client cannot be created.
    pub fn new(core: ExecutionClientCore, config: AlpacaExecClientConfig) -> anyhow::Result<Self> {
        for (name, limit) in [
            ("max_daily_loss_usd", config.max_daily_loss_usd),
            ("max_daily_loss_pct", config.max_daily_loss_pct),
        ] {
            if limit.is_some_and(|value| value <= Decimal::ZERO) {
                anyhow::bail!("Alpaca {name} must be positive when configured");
            }
        }
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
            http_client.set_trading_base_url(url);
        }

        let clock = get_atomic_clock_realtime();
        let emitter = ExecutionEventEmitter::new(
            clock,
            core.trader_id,
            core.account_id,
            core.account_type,
            core.base_currency,
        );

        Ok(Self {
            core,
            emitter,
            config,
            http_client: Arc::new(http_client),
            chain: Arc::new(Mutex::new(ReplacementChain::default())),
            account: Arc::new(Mutex::new(None)),
            cancellation_token: CancellationToken::new(),
            tasks: Vec::new(),
            clock,
        })
    }

    /// Returns the event emitter.
    #[must_use]
    pub const fn emitter(&self) -> &ExecutionEventEmitter {
        &self.emitter
    }

    /// Returns a mutable reference to the event emitter.
    pub const fn emitter_mut(&mut self) -> &mut ExecutionEventEmitter {
        &mut self.emitter
    }

    /// Builds the venue submission body for an order.
    ///
    /// # Errors
    ///
    /// Returns an error if the order cannot be expressed on the venue, or if a price violates
    /// Rule 612.
    fn build_submit_request(
        cmd: &SubmitOrder,
        extended_hours: bool,
    ) -> anyhow::Result<SubmitOrderRequest> {
        let init = &cmd.order_init;

        let order_type = AlpacaOrderType::from_nautilus(init.order_type)?;
        let side = AlpacaOrderSide::from_nautilus(init.order_side)?;
        let time_in_force = AlpacaTimeInForce::from_nautilus(init.time_in_force)?;

        if init.quantity.precision != 0 {
            anyhow::bail!(
                "Alpaca orders are whole shares; quantity {} carries a fraction",
                init.quantity
            );
        }

        let limit_price = match init.price {
            Some(price) => {
                reg_nms::check_order_price(price.as_decimal())?;
                Some(price.to_string())
            }
            None => None,
        };
        let stop_price = match init.trigger_price {
            Some(price) => {
                reg_nms::check_order_price(price.as_decimal())?;
                Some(price.to_string())
            }
            None => None,
        };

        if matches!(init.order_type, OrderType::Limit | OrderType::StopLimit)
            && limit_price.is_none()
        {
            anyhow::bail!("A {:?} order requires a limit price", init.order_type);
        }

        Ok(SubmitOrderRequest {
            symbol: cmd.instrument_id.symbol.to_string(),
            qty: init.quantity.to_string(),
            side,
            order_type,
            time_in_force,
            client_order_id: Some(cmd.client_order_id.to_string()),
            limit_price,
            stop_price,
            extended_hours,
        })
    }

    /// Converts a venue account into balances for an account state event.
    ///
    /// # Errors
    ///
    /// Returns an error if the currency or any amount cannot be parsed.
    fn account_balances(
        account: &crate::http::models::Account,
    ) -> anyhow::Result<Vec<AccountBalance>> {
        let currency = Currency::try_from_str(&account.currency)
            .ok_or_else(|| anyhow!("Unrecognised account currency '{}'", account.currency))?;

        let total = parse_decimal(&account.equity, "equity")?;
        let free = parse_decimal(&account.cash, "cash")?;
        // Long holdings normally make cash lower than equity, while short-sale proceeds can make
        // it higher. AccountBalance requires total == locked + free and cannot represent a
        // negative locked amount. Build it through the checked fixed-point constructor, which
        // clamps reported cash to equity in that short-account case instead of panicking inside an
        // event-loop callback. Signed positions remain the source of exposure truth.
        let balance = AccountBalance::from_total_and_free(total, free, currency)
            .map_err(|error| anyhow!("Invalid Alpaca account balance: {error}"))?;

        Ok(vec![balance])
    }

    /// Returns true only for a whole order which exactly closes the venue's current signed
    /// position. A partial reduction is deliberately denied after a daily-loss breach because the
    /// remaining exposure would no longer be governed by the strategy decision which tripped.
    fn exactly_closes_position(
        request: &SubmitOrderRequest,
        positions: &[AlpacaPosition],
    ) -> anyhow::Result<bool> {
        let quantity = parse_decimal(&request.qty, "order quantity")?;
        if quantity <= Decimal::ZERO {
            anyhow::bail!("order quantity must be positive");
        }
        let mut matches = positions
            .iter()
            .filter(|position| position.symbol == request.symbol);
        let Some(position) = matches.next() else {
            return Ok(false);
        };
        if matches.next().is_some() {
            anyhow::bail!("Alpaca returned multiple positions for {}", request.symbol);
        }
        let signed_position = parse_decimal(&position.qty, "position quantity")?;
        Ok(match request.side {
            AlpacaOrderSide::Sell => signed_position == quantity,
            AlpacaOrderSide::Buy => signed_position == -quantity,
            AlpacaOrderSide::SellShort | AlpacaOrderSide::Unknown => false,
        })
    }

    /// Fetches the account and emits its state.
    async fn refresh_account(&self) -> anyhow::Result<()> {
        let account = self.http_client.get_account().await?;
        account.validate_for_trading(self.config.expected_account_number.as_deref())?;

        let balances = Self::account_balances(&account)?;
        self.emitter.emit_account_state(
            balances,
            Vec::<MarginBalance>::new(),
            true,
            self.clock.get_time_ns(),
        );
        Ok(())
    }

    /// Consumes the trading event stream, publishing reports as events arrive.
    ///
    /// Fills become fill reports and everything else becomes an order status report, so the
    /// engine learns about venue-side activity it did not initiate. A `replaced` event records the
    /// new identifier in the chain: the venue may replace an order without the engine asking, and
    /// losing that link would leave later cancels addressing a dead identifier.
    async fn open_trading_stream(&self) -> anyhow::Result<AlpacaTradingStream> {
        let Some(credential) = AlpacaCredential::resolve(
            self.config.api_key.as_deref(),
            self.config.api_secret.as_deref(),
        ) else {
            anyhow::bail!("Cannot open the Alpaca trading stream without credentials");
        };

        connect_trading_stream(
            self.config.environment,
            &credential,
            self.config.base_url_ws.clone(),
        )
        .await
    }

    fn spawn_stream_task(&mut self, mut stream: AlpacaTradingStream) -> anyhow::Result<()> {
        let Some(credential) = AlpacaCredential::resolve(
            self.config.api_key.as_deref(),
            self.config.api_secret.as_deref(),
        ) else {
            anyhow::bail!("Cannot reconnect the Alpaca trading stream without credentials");
        };

        let environment = self.config.environment;
        let url_override = self.config.base_url_ws.clone();
        let http_client = self.http_client.clone();
        let emitter = self.emitter.clone();
        let chain = self.chain.clone();
        let account_id = self.core.account_id;
        let token = self.cancellation_token.clone();
        let clock = self.clock;
        let mut backoff = ExponentialBackoff::new(
            RECONNECT_BASE_BACKOFF,
            RECONNECT_MAX_BACKOFF,
            RECONNECT_BACKOFF_FACTOR,
            RECONNECT_JITTER_MS,
            true,
        )?;

        let handle = nautilus_common::live::runtime::get_runtime().spawn(async move {
            let mut recovery_start = clock.get_time_ns();

            loop {
                let update = tokio::select! {
                    () = token.cancelled() => {
                        log::debug!("Alpaca trading stream task cancelled");
                        return;
                    }
                    update = stream.next_update() => update,
                };

                let Some(update) = update else {
                    log::warn!("Alpaca trading stream closed");
                    loop {
                        let delay = backoff.next_duration();
                        if !delay.is_zero() {
                            log::warn!(
                                "Reconnecting the Alpaca trading stream in {} ms",
                                delay.as_millis(),
                            );
                        }
                        tokio::select! {
                            () = token.cancelled() => return,
                            () = tokio::time::sleep(delay) => {}
                        }

                        let reconnect = tokio::select! {
                            () = token.cancelled() => return,
                            result = connect_trading_stream(
                                environment,
                                &credential,
                                url_override.clone(),
                            ) => result,
                        };
                        let candidate = match reconnect {
                            Ok(candidate) => candidate,
                            Err(e) => {
                                log::error!("Failed to reconnect the Alpaca trading stream: {e}");
                                continue;
                            }
                        };
                        let recovered_at = clock.get_time_ns();
                        match Self::recover_stream_gap(
                            &http_client,
                            &emitter,
                            account_id,
                            recovery_start,
                            recovered_at,
                            clock.get_time_ns(),
                        )
                        .await
                        {
                            Ok((orders, fills)) => {
                                log::warn!(
                                    "Recovered Alpaca trading stream gap with {orders} order reports and {fills} fill reports",
                                );
                                stream = candidate;
                                recovery_start = recovered_at;
                                backoff.reset();
                                break;
                            }
                            Err(e) => {
                                log::error!(
                                    "Failed to recover the Alpaca trading stream gap: {e}; reconnecting before retry",
                                );
                            }
                        }
                    }
                    continue;
                };

                let update = match update {
                    Ok(update) => update,
                    Err(e) => {
                        // A frame that claimed to be a trade update but could not be decoded means
                        // an order event was lost. Reconcile immediately because a malformed frame
                        // does not necessarily close an otherwise healthy transport.
                        log::error!("Alpaca trade update could not be decoded: {e}");
                        let recovered_at = clock.get_time_ns();
                        match Self::recover_stream_gap(
                            &http_client,
                            &emitter,
                            account_id,
                            recovery_start,
                            recovered_at,
                            clock.get_time_ns(),
                        )
                        .await
                        {
                            Ok((orders, fills)) => {
                                log::warn!(
                                    "Recovered malformed Alpaca trade update with {orders} order reports and {fills} fill reports",
                                );
                                recovery_start = recovered_at;
                            }
                            Err(recovery_error) => {
                                log::error!(
                                    "Failed to recover malformed Alpaca trade update: {recovery_error}",
                                );
                            }
                        }
                        continue;
                    }
                };

                Self::handle_update(&update, &emitter, &chain, account_id, clock.get_time_ns())
                    .await;
                recovery_start = clock.get_time_ns();
            }
        });

        self.tasks.push(handle);
        Ok(())
    }

    async fn recover_stream_gap(
        http_client: &AlpacaRawHttpClient,
        emitter: &ExecutionEventEmitter,
        account_id: AccountId,
        start: UnixNanos,
        end: UnixNanos,
        ts_init: UnixNanos,
    ) -> anyhow::Result<(usize, usize)> {
        let orders = http_client
            .list_orders_all_pages(
                &ListOrdersParams::all()
                    .with_window(Some(start.to_rfc3339()), Some(end.to_rfc3339())),
                MAX_ORDER_PAGES,
            )
            .await?;
        let order_reports = Self::reports_from_orders_at(&orders, account_id, ts_init);
        let order_count = order_reports.len();
        for report in order_reports {
            emitter.send_order_status_report(report);
        }

        let activities = http_client
            .list_fill_activities_all_pages(
                &ActivitiesParams::fills()
                    .with_window(Some(start.to_rfc3339()), Some(end.to_rfc3339())),
                MAX_ACTIVITY_PAGES,
            )
            .await?;
        let fill_reports = activities
            .iter()
            .filter_map(|activity| {
                parse_fill_activity_report(activity, account_id, reg_nms::PRICE_PRECISION, ts_init)
                    .inspect_err(|e| {
                        log::warn!("Skipping Alpaca fill activity {}: {e}", activity.id);
                    })
                    .ok()
            })
            .collect::<Vec<_>>();
        let fill_count = fill_reports.len();
        for report in fill_reports {
            emitter.send_fill_report(report);
        }
        Ok((order_count, fill_count))
    }

    /// Publishes one trade update.
    async fn handle_update(
        update: &TradeUpdate,
        emitter: &ExecutionEventEmitter,
        chain: &Mutex<ReplacementChain>,
        account_id: AccountId,
        ts_init: UnixNanos,
    ) {
        let kind = update.event_kind();

        if kind == AlpacaTradeEvent::Unknown {
            // Named rather than counted, so an unmodelled event can actually be chased down.
            log::warn!(
                "Unmodelled Alpaca trade event '{}' for order {}",
                update.event,
                update.order.id
            );
        }

        if kind == AlpacaTradeEvent::Replaced
            && let Some(replaced_by) = update.order.replaced_by.as_deref()
        {
            chain.lock().await.record(
                VenueOrderId::new(update.order.id.as_str()),
                VenueOrderId::new(replaced_by),
            );
        }

        if update.has_fill_detail() {
            match parse_fill_report(update, account_id, reg_nms::PRICE_PRECISION, ts_init) {
                Ok(report) => emitter.send_fill_report(report),
                Err(e) => log::error!("Cannot build an Alpaca fill report: {e}"),
            }
            return;
        }

        if kind.is_request_rejection() {
            // The order is untouched by a refused request; a status report would say nothing new.
            log::warn!(
                "Alpaca refused a request on order {} ({})",
                update.order.id,
                update.event
            );
            return;
        }

        match parse_order_status_report(
            &update.order,
            account_id,
            reg_nms::PRICE_PRECISION,
            ts_init,
        ) {
            Ok(report) => emitter.send_order_status_report(report),
            Err(e) => log::debug!("No status report for order {}: {e}", update.order.id),
        }
    }

    /// Resolves the identifier an order currently trades under.
    async fn live_venue_order_id(&self, venue_order_id: VenueOrderId) -> VenueOrderId {
        self.chain.lock().await.resolve(venue_order_id)
    }
}

#[async_trait(?Send)]
impl ExecutionClient for AlpacaExecutionClient {
    fn is_connected(&self) -> bool {
        self.core.is_connected()
    }

    fn client_id(&self) -> ClientId {
        self.core.client_id
    }

    fn account_id(&self) -> AccountId {
        self.core.account_id
    }

    fn venue(&self) -> nautilus_model::identifiers::Venue {
        *ALPACA_VENUE
    }

    fn oms_type(&self) -> OmsType {
        // Alpaca exposes one net position per symbol and offers no hedge mode.
        OmsType::Netting
    }

    fn get_account(&self) -> Option<AccountAny> {
        self.account.blocking_lock().clone()
    }

    fn generate_account_state(
        &self,
        balances: Vec<AccountBalance>,
        margins: Vec<MarginBalance>,
        reported: bool,
        ts_event: UnixNanos,
    ) -> anyhow::Result<()> {
        self.emitter
            .emit_account_state(balances, margins, reported, ts_event);
        Ok(())
    }

    fn start(&mut self) -> anyhow::Result<()> {
        if self.core.is_started() {
            return Ok(());
        }

        // Without this the emitter drops every event it is handed, so account state and order
        // events never reach the engine.
        self.emitter
            .set_sender(nautilus_common::live::runner::get_exec_event_sender());
        self.core.set_started();

        log::info!(
            "Started: client_id={}, account_id={}, environment={:?}",
            self.core.client_id,
            self.core.account_id,
            self.config.environment,
        );
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        self.cancellation_token.cancel();
        self.core.set_stopped();
        Ok(())
    }

    async fn connect(&mut self) -> anyhow::Result<()> {
        if self.core.is_connected() {
            return Ok(());
        }
        self.refresh_account().await?;
        let stream = self.open_trading_stream().await?;
        self.spawn_stream_task(stream)?;
        self.core.set_connected();
        Ok(())
    }

    async fn disconnect(&mut self) -> anyhow::Result<()> {
        self.cancellation_token.cancel();
        for task in self.tasks.drain(..) {
            task.abort();
        }
        self.core.set_disconnected();
        Ok(())
    }

    fn submit_order(&self, cmd: SubmitOrder) -> anyhow::Result<()> {
        let order = self.core.get_order(&cmd.client_order_id)?;

        // Anything the venue cannot express, and any price the regulation forbids, is denied here
        // rather than sent: the venue would reject it anyway, and denying locally keeps the
        // rejection reason precise.
        let request = match Self::build_submit_request(&cmd, self.config.default_extended_hours) {
            Ok(request) => request,
            Err(e) => {
                self.emitter.emit_order_denied(&order, &e.to_string());
                return Ok(());
            }
        };

        let http_client = self.http_client.clone();
        let emitter = self.emitter.clone();
        let clock = self.clock;
        let client_order_id = cmd.client_order_id;
        let expected_account_number = self.config.expected_account_number.clone();
        let max_daily_loss_usd = self.config.max_daily_loss_usd;
        let max_daily_loss_pct = self.config.max_daily_loss_pct;
        let reject_conflicting_open_orders = self.config.reject_conflicting_open_orders;
        nautilus_common::live::runtime::get_runtime().spawn(async move {
            let account = match http_client.get_account().await {
                Ok(account) => account,
                Err(e) => {
                    emitter.emit_order_denied(
                        &order,
                        &format!("Alpaca account pre-submit query failed: {e}"),
                    );
                    return;
                }
            };
            if let Err(e) = account.validate_for_trading(expected_account_number.as_deref()) {
                emitter.emit_order_denied(
                    &order,
                    &format!("Alpaca account pre-submit check failed: {e}"),
                );
                return;
            }
            let breach = match account.daily_loss_breach(
                max_daily_loss_usd,
                max_daily_loss_pct,
            ) {
                Ok(breach) => breach,
                Err(e) => {
                    emitter.emit_order_denied(
                        &order,
                        &format!("Alpaca daily-loss pre-submit check failed: {e}"),
                    );
                    return;
                }
            };
            if let Some(breach) = breach {
                let reason = breach.to_string();
                let positions = match http_client.list_positions().await {
                    Ok(positions) => positions,
                    Err(e) => {
                        emitter.emit_order_denied(
                            &order,
                            &format!(
                                "Alpaca daily-loss close check could not read positions: {e}"
                            ),
                        );
                        return;
                    }
                };
                match Self::exactly_closes_position(&request, &positions) {
                    Ok(true) => log::warn!(
                        "{reason}; allowing exact close {} {} {}",
                        request.side,
                        request.qty,
                        request.symbol,
                    ),
                    Ok(false) => {
                        emitter.emit_order_denied(&order, &reason);
                        return;
                    }
                    Err(e) => {
                        emitter.emit_order_denied(
                            &order,
                            &format!("Alpaca daily-loss close check failed: {e}"),
                        );
                        return;
                    }
                }
            }
            if reject_conflicting_open_orders {
                let mut params = ListOrdersParams::open();
                params.symbols = Some(request.symbol.clone());
                let open_orders = match http_client
                    .list_orders_all_pages(&params, MAX_ORDER_PAGES)
                    .await
                {
                    Ok(orders) => orders,
                    Err(e) => {
                        emitter.emit_order_denied(
                            &order,
                            &format!("Alpaca conflicting-order pre-submit query failed: {e}"),
                        );
                        return;
                    }
                };
                let conflicts = Self::conflicting_working_client_order_ids(
                    client_order_id.as_str(),
                    &open_orders,
                );
                if !conflicts.is_empty() {
                    emitter.emit_order_denied(
                        &order,
                        &format!(
                            "Alpaca symbol {} has {} conflicting working order(s): {}",
                            request.symbol,
                            conflicts.len(),
                            conflicts.join(",")
                        ),
                    );
                    return;
                }
            }

            emitter.emit_order_submitted(&order);
            match http_client.submit_order(&request).await {
                Ok(venue_order) => {
                    log::debug!(
                        "Alpaca accepted order {} as {}",
                        venue_order.client_order_id,
                        venue_order.id
                    );
                    emitter.emit_order_accepted(
                        &order,
                        VenueOrderId::new(venue_order.id.as_str()),
                        clock.get_time_ns(),
                    );
                }
                Err(submit_error) => {
                    // A timed out or broken POST may still have created the order, and even a
                    // duplicate-ID response can refer to one created by an earlier attempt.
                    // Querying by the caller-supplied stable ID is safe; replaying the POST is not.
                    match http_client
                        .get_order_by_client_order_id(client_order_id.as_str())
                        .await
                    {
                        Ok(venue_order)
                            if venue_order.client_order_id == client_order_id.as_str() =>
                        {
                            log::warn!(
                                "Recovered ambiguously submitted Alpaca order {} as {} after: {submit_error}",
                                venue_order.client_order_id, venue_order.id,
                            );
                            emitter.emit_order_accepted(
                                &order,
                                VenueOrderId::new(venue_order.id.as_str()),
                                clock.get_time_ns(),
                            );
                        }
                        Ok(venue_order) => {
                            log::error!(
                                "Alpaca recovery query for {client_order_id} returned mismatched order {} with client ID {}; keeping submission unresolved after: {submit_error}",
                                venue_order.id, venue_order.client_order_id,
                            );
                        }
                        Err(query_error) => {
                            if submit_error.is_definitive_submission_rejection() {
                                emitter.emit_order_rejected(
                                    &order,
                                    &submit_error.to_string(),
                                    clock.get_time_ns(),
                                    false,
                                );
                            } else {
                                // Leave the order submitted. Startup/periodic reconciliation can
                                // still discover it; rejecting here could let the strategy place a
                                // duplicate while the first order is live.
                                log::error!(
                                    "Alpaca order {client_order_id} has an ambiguous submission result: {submit_error}; recovery query failed: {query_error}",
                                );
                            }
                        }
                    }
                }
            }
        });

        Ok(())
    }

    fn modify_order(&self, cmd: ModifyOrder) -> anyhow::Result<()> {
        let Some(venue_order_id) = cmd.venue_order_id else {
            anyhow::bail!("Cannot modify {}: no venue order ID", cmd.client_order_id);
        };

        let mut request = ReplaceOrderRequest::default();
        if let Some(quantity) = cmd.quantity {
            if quantity.precision != 0 {
                anyhow::bail!("Alpaca orders are whole shares; quantity {quantity} has a fraction");
            }
            request.qty = Some(quantity.to_string());
        }
        if let Some(price) = cmd.price {
            reg_nms::check_order_price(price.as_decimal())?;
            request.limit_price = Some(price.to_string());
        }
        if let Some(price) = cmd.trigger_price {
            reg_nms::check_order_price(price.as_decimal())?;
            request.stop_price = Some(price.to_string());
        }

        if request.is_empty() {
            // Sending this would still replace the order and issue a new identifier for no gain.
            log::debug!("Ignoring empty amendment for {}", cmd.client_order_id);
            return Ok(());
        }

        // Held so the rejection path can report against the order the engine knows.
        let order = self.core.get_order(&cmd.client_order_id)?;

        let http_client = self.http_client.clone();
        let chain = self.chain.clone();
        let emitter = self.emitter.clone();
        let clock = self.clock;
        let client_order_id = cmd.client_order_id;
        nautilus_common::live::runtime::get_runtime().spawn(async move {
            let live_id = chain.lock().await.resolve(venue_order_id);
            match http_client.replace_order(live_id.as_str(), &request).await {
                Ok(replacement) => {
                    let new_id = VenueOrderId::new(replacement.id.as_str());
                    chain.lock().await.record(venue_order_id, new_id);
                    log::debug!("Alpaca replaced {live_id} with {new_id} for {client_order_id}");
                }
                Err(e) => {
                    // The venue refuses amendments in several states — an order that has been
                    // received but not yet routed cannot be replaced, for one. The order is still
                    // working, so the engine has to hear that its request failed rather than be
                    // left waiting for an update that will never come.
                    emitter.emit_order_modify_rejected(
                        &order,
                        Some(live_id),
                        &e.to_string(),
                        clock.get_time_ns(),
                    );
                }
            }
        });

        Ok(())
    }

    fn cancel_order(&self, cmd: CancelOrder) -> anyhow::Result<()> {
        let Some(venue_order_id) = cmd.venue_order_id else {
            anyhow::bail!("Cannot cancel {}: no venue order ID", cmd.client_order_id);
        };

        let order = self.core.get_order(&cmd.client_order_id)?;

        let http_client = self.http_client.clone();
        let chain = self.chain.clone();
        let emitter = self.emitter.clone();
        let clock = self.clock;
        let client_order_id = cmd.client_order_id;
        nautilus_common::live::runtime::get_runtime().spawn(async move {
            let live_id = chain.lock().await.resolve(venue_order_id);
            match http_client.cancel_order(live_id.as_str()).await {
                Ok(()) => {
                    chain.lock().await.forget(venue_order_id);
                    log::debug!("Alpaca canceled {live_id} for {client_order_id}");
                }
                Err(e) => {
                    // A refused cancellation leaves the order working; the engine must not be left
                    // believing it is on its way out.
                    emitter.emit_order_cancel_rejected(
                        &order,
                        Some(live_id),
                        &e.to_string(),
                        clock.get_time_ns(),
                    );
                }
            }
        });

        Ok(())
    }

    fn cancel_all_orders(&self, cmd: CancelAllOrders) -> anyhow::Result<()> {
        // The venue cancels every open order account-wide; it has no per-symbol form, so an
        // instrument-scoped request would cancel more than was asked.
        log::warn!(
            "Alpaca cancels all open orders account-wide; the {} scope on this request is ignored",
            cmd.instrument_id
        );

        let http_client = self.http_client.clone();
        let chain = self.chain.clone();
        nautilus_common::live::runtime::get_runtime().spawn(async move {
            match http_client.cancel_all_orders().await {
                Ok(()) => {
                    *chain.lock().await = ReplacementChain::default();
                    log::debug!("Alpaca canceled all open orders");
                }
                Err(e) => log::error!("Failed to cancel all orders: {e}"),
            }
        });

        Ok(())
    }

    fn batch_cancel_orders(&self, cmd: BatchCancelOrders) -> anyhow::Result<()> {
        // No batch endpoint exists, so the cancels are issued individually.
        for cancel in cmd.cancels {
            self.cancel_order(cancel)?;
        }
        Ok(())
    }

    fn query_account(&self, _cmd: QueryAccount) -> anyhow::Result<()> {
        let http_client = self.http_client.clone();
        let emitter = self.emitter.clone();
        let clock = self.clock;
        let expected_account_number = self.config.expected_account_number.clone();
        nautilus_common::live::runtime::get_runtime().spawn(async move {
            match http_client.get_account().await {
                Ok(account) => {
                    if let Err(e) = account.validate_for_trading(expected_account_number.as_deref())
                    {
                        log::error!("Alpaca account query failed eligibility validation: {e}");
                        return;
                    }
                    match Self::account_balances(&account) {
                        Ok(balances) => {
                            emitter.emit_account_state(
                                balances,
                                Vec::<MarginBalance>::new(),
                                true,
                                clock.get_time_ns(),
                            );
                        }
                        Err(e) => log::error!("Failed to convert Alpaca account: {e}"),
                    }
                }
                Err(e) => log::error!("Failed to query Alpaca account: {e}"),
            }
        });
        Ok(())
    }

    fn query_order(&self, cmd: QueryOrder) -> anyhow::Result<()> {
        let http_client = self.http_client.clone();
        let chain = self.chain.clone();
        let emitter = self.emitter.clone();
        let account_id = self.core.account_id;
        let clock = self.clock;
        nautilus_common::live::runtime::get_runtime().spawn(async move {
            let client_order_id = cmd.client_order_id;
            let venue_order_id = cmd.venue_order_id;
            let result = if let Some(venue_order_id) = venue_order_id {
                let live_id = chain.lock().await.resolve(venue_order_id);
                http_client.get_order(live_id.as_str()).await
            } else {
                http_client
                    .get_order_by_client_order_id(client_order_id.as_str())
                    .await
            };
            match result {
                Ok(venue_order) => {
                    match parse_order_status_report(
                        &venue_order,
                        account_id,
                        reg_nms::PRICE_PRECISION,
                        clock.get_time_ns(),
                    ) {
                        Ok(report) => emitter.send_order_status_report(report),
                        Err(e) => log::warn!("Cannot report order {client_order_id}: {e}"),
                    }
                }
                Err(e) => log::error!("Failed to query order {client_order_id}: {e}"),
            }
        });

        Ok(())
    }

    async fn generate_order_status_report(
        &self,
        cmd: &GenerateOrderStatusReport,
    ) -> anyhow::Result<Option<OrderStatusReport>> {
        let venue_order = if let Some(venue_order_id) = cmd.venue_order_id {
            let live_id = self.live_venue_order_id(venue_order_id).await;
            self.http_client.get_order(live_id.as_str()).await?
        } else if let Some(client_order_id) = cmd.client_order_id {
            self.http_client
                .get_order_by_client_order_id(client_order_id.as_str())
                .await?
        } else {
            return Ok(None);
        };

        parse_order_status_report(
            &venue_order,
            self.core.account_id,
            reg_nms::PRICE_PRECISION,
            self.clock.get_time_ns(),
        )
        .map(Some)
    }

    async fn generate_order_status_reports(
        &self,
        cmd: &GenerateOrderStatusReports,
    ) -> anyhow::Result<Vec<OrderStatusReport>> {
        // `open_only` is the engine telling us whether finished orders matter. When it is false it
        // expects recently closed orders in the answer, and returning only working ones leaves it
        // querying each of them individually to find out what happened.
        let mut params = if cmd.open_only {
            ListOrdersParams::open()
        } else {
            ListOrdersParams::all()
        }
        .with_window(
            cmd.start.map(|nanos| nanos.to_rfc3339()),
            cmd.end.map(|nanos| nanos.to_rfc3339()),
        );

        if let Some(instrument_id) = cmd.instrument_id {
            params = params.with_symbol(instrument_id.symbol.as_str());
        }

        let orders = self
            .http_client
            .list_orders_all_pages(&params, MAX_ORDER_PAGES)
            .await?;

        Ok(self.reports_from_orders(&orders))
    }

    async fn generate_fill_reports(
        &self,
        cmd: GenerateFillReports,
    ) -> anyhow::Result<Vec<FillReport>> {
        // Recovered from the activity feed rather than the trading event stream, which reports
        // fills as they happen but cannot be replayed. Reconciliation runs at startup, before the
        // stream has delivered anything, so it needs this path.
        //
        // The endpoint returns newest first and accepts a time window, so the request is bounded
        // where it can be. Symbol and order filters are applied here because it takes neither.
        let params = ActivitiesParams::fills().with_window(
            cmd.start.map(|nanos| nanos.to_rfc3339()),
            cmd.end.map(|nanos| nanos.to_rfc3339()),
        );
        let activities = self
            .http_client
            .list_fill_activities_all_pages(&params, MAX_ACTIVITY_PAGES)
            .await?;

        let symbol = cmd.instrument_id.map(|id| id.symbol);
        let ts_init = self.clock.get_time_ns();
        Ok(activities
            .iter()
            .filter(|activity| {
                symbol.is_none_or(|symbol| symbol.as_str() == activity.symbol)
                    && cmd
                        .venue_order_id
                        .is_none_or(|id| id.as_str() == activity.order_id)
            })
            .filter_map(|activity| {
                parse_fill_activity_report(
                    activity,
                    self.core.account_id,
                    reg_nms::PRICE_PRECISION,
                    ts_init,
                )
                .inspect_err(|e| log::warn!("Skipping Alpaca fill activity {}: {e}", activity.id))
                .ok()
            })
            .collect())
    }

    async fn generate_position_status_reports(
        &self,
        _cmd: &GeneratePositionStatusReports,
    ) -> anyhow::Result<Vec<PositionStatusReport>> {
        let positions = self.http_client.list_positions().await?;
        let ts_init = self.clock.get_time_ns();

        Ok(positions
            .iter()
            .filter_map(|position| {
                parse_position_status_report(position, self.core.account_id, ts_init)
                    .inspect_err(|e| {
                        log::warn!("Skipping Alpaca position for {}: {e}", position.symbol);
                    })
                    .ok()
            })
            .collect())
    }
}

impl AlpacaExecutionClient {
    /// Converts venue orders into reports, skipping any that cannot be represented.
    ///
    /// Orders that were superseded by a replacement are dropped rather than reported: they are not
    /// cancelled, and the replacement carries the live state.
    fn reports_from_orders(&self, orders: &[AlpacaOrder]) -> Vec<OrderStatusReport> {
        Self::reports_from_orders_at(orders, self.core.account_id, self.clock.get_time_ns())
    }

    fn reports_from_orders_at(
        orders: &[AlpacaOrder],
        account_id: AccountId,
        ts_init: UnixNanos,
    ) -> Vec<OrderStatusReport> {
        orders
            .iter()
            .filter(|order| !order.status.is_superseded())
            .filter_map(|order| {
                parse_order_status_report(order, account_id, reg_nms::PRICE_PRECISION, ts_init)
                    .inspect_err(|e| log::warn!("Skipping Alpaca order {}: {e}", order.id))
                    .ok()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use rust_decimal_macros::dec;

    use super::*;

    fn vid(raw: &str) -> VenueOrderId {
        VenueOrderId::new(raw)
    }

    fn active_account() -> crate::http::models::Account {
        serde_json::from_str(
            r#"{
                "id": "paper-id",
                "account_number": "PAPER123",
                "status": "ACTIVE",
                "currency": "USD",
                "cash": "10000",
                "equity": "10000",
                "last_equity": "10000",
                "buying_power": "10000"
            }"#,
        )
        .unwrap()
    }

    fn request(side: AlpacaOrderSide, quantity: &str) -> SubmitOrderRequest {
        SubmitOrderRequest {
            symbol: "AAPL".to_string(),
            qty: quantity.to_string(),
            side,
            order_type: AlpacaOrderType::Limit,
            time_in_force: AlpacaTimeInForce::Day,
            client_order_id: Some("daily-loss-test".to_string()),
            limit_price: Some("100.00".to_string()),
            stop_price: None,
            extended_hours: true,
        }
    }

    fn position(quantity: &str, side: &str) -> AlpacaPosition {
        serde_json::from_value(serde_json::json!({
            "asset_id": "aapl-id",
            "symbol": "AAPL",
            "qty": quantity,
            "avg_entry_price": "100.00",
            "side": side
        }))
        .unwrap()
    }

    fn order(client_order_id: &str, status: &str) -> AlpacaOrder {
        serde_json::from_value(serde_json::json!({
            "id": format!("venue-{client_order_id}"),
            "client_order_id": client_order_id,
            "symbol": "AAPL",
            "side": "buy",
            "time_in_force": "day",
            "status": status
        }))
        .unwrap()
    }

    #[rstest]
    fn test_active_unblocked_expected_account_passes_preflight() {
        assert!(
            active_account()
                .validate_for_trading(Some("PAPER123"))
                .is_ok()
        );
    }

    #[rstest]
    #[case("CLOSED", "USD", false)]
    #[case("ACTIVE", "EUR", false)]
    #[case("ACTIVE", "USD", true)]
    fn test_ineligible_account_fails_preflight(
        #[case] status: &str,
        #[case] currency: &str,
        #[case] blocked: bool,
    ) {
        let mut account = active_account();
        account.status = status.to_string();
        account.currency = currency.to_string();
        account.trading_blocked = blocked;

        assert!(account.validate_for_trading(Some("PAPER123")).is_err());
    }

    #[rstest]
    fn test_unexpected_account_number_fails_preflight() {
        assert!(
            active_account()
                .validate_for_trading(Some("LIVE456"))
                .is_err()
        );
    }

    #[rstest]
    #[case("100000", "99800", "100000.00", "200.00", "99800.00")]
    #[case("100000", "100100", "100000.00", "0.00", "100000.00")]
    fn test_account_balances_preserve_the_invariant_for_long_and_short_cash(
        #[case] equity: &str,
        #[case] cash: &str,
        #[case] total: &str,
        #[case] locked: &str,
        #[case] free: &str,
    ) {
        let mut account = active_account();
        account.equity = equity.to_string();
        account.cash = cash.to_string();

        let balance = AlpacaExecutionClient::account_balances(&account)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(balance.total.to_string(), format!("{total} USD"));
        assert_eq!(balance.locked.to_string(), format!("{locked} USD"));
        assert_eq!(balance.free.to_string(), format!("{free} USD"));
        assert_eq!(
            balance.locked.checked_add(balance.free),
            Some(balance.total)
        );
    }

    #[rstest]
    fn test_daily_loss_uses_previous_close_and_either_limit() {
        let mut account = active_account();
        account.equity = "9400".to_string();

        let cash = account
            .daily_loss_breach(Some(dec!(500)), Some(dec!(10)))
            .unwrap();
        let percent = account
            .daily_loss_breach(Some(dec!(1000)), Some(dec!(5)))
            .unwrap();
        let below = account
            .daily_loss_breach(Some(dec!(1000)), Some(dec!(10)))
            .unwrap();

        assert!(cash.is_some());
        assert!(percent.is_some());
        assert!(below.is_none());
    }

    #[rstest]
    fn test_daily_loss_with_invalid_previous_close_fails_closed() {
        let mut account = active_account();
        account.last_equity = "0".to_string();

        assert!(account.daily_loss_breach(Some(dec!(500)), None).is_err());
    }

    #[rstest]
    #[case(AlpacaOrderSide::Sell, "2", "2", "long")]
    #[case(AlpacaOrderSide::Buy, "2", "-2", "short")]
    fn test_daily_loss_gate_allows_exact_long_or_short_close(
        #[case] side: AlpacaOrderSide,
        #[case] order_quantity: &str,
        #[case] position_quantity: &str,
        #[case] position_side: &str,
    ) {
        assert!(
            AlpacaExecutionClient::exactly_closes_position(
                &request(side, order_quantity),
                &[position(position_quantity, position_side)],
            )
            .unwrap()
        );
    }

    #[rstest]
    #[case(AlpacaOrderSide::Buy, "2", "2", "long")]
    #[case(AlpacaOrderSide::Sell, "2", "-2", "short")]
    #[case(AlpacaOrderSide::Sell, "1", "2", "long")]
    #[case(AlpacaOrderSide::Sell, "3", "2", "long")]
    fn test_daily_loss_gate_denies_new_risk_partial_reductions_and_oversells(
        #[case] side: AlpacaOrderSide,
        #[case] order_quantity: &str,
        #[case] position_quantity: &str,
        #[case] position_side: &str,
    ) {
        assert!(
            !AlpacaExecutionClient::exactly_closes_position(
                &request(side, order_quantity),
                &[position(position_quantity, position_side)],
            )
            .unwrap()
        );
    }

    #[rstest]
    fn test_conflicting_order_gate_ignores_same_identity_and_terminal_history() {
        let orders = [
            order("current", "accepted"),
            order("manual-working", "new"),
            order("finished", "filled"),
        ];

        assert_eq!(
            AlpacaExecutionClient::conflicting_working_client_order_ids("current", &orders),
            vec!["manual-working"]
        );
    }

    #[rstest]
    fn test_unknown_id_resolves_to_itself() {
        let chain = ReplacementChain::default();
        assert_eq!(chain.resolve(vid("A")), vid("A"));
    }

    #[rstest]
    fn test_replacement_is_resolved() {
        let mut chain = ReplacementChain::default();
        chain.record(vid("A"), vid("B"));
        assert_eq!(chain.resolve(vid("A")), vid("B"));
    }

    #[rstest]
    fn test_repeated_amendment_collapses_rather_than_nesting() {
        // The engine keeps addressing the original identifier, so it must reach the newest one in
        // a single hop however many amendments have happened.
        let mut chain = ReplacementChain::default();
        chain.record(vid("A"), vid("B"));
        chain.record(vid("B"), vid("C"));
        assert_eq!(chain.resolve(vid("A")), vid("C"));
    }

    #[rstest]
    fn test_third_amendment_still_resolves_from_the_original() {
        let mut chain = ReplacementChain::default();
        chain.record(vid("A"), vid("B"));
        chain.record(vid("B"), vid("C"));
        chain.record(vid("C"), vid("D"));
        assert_eq!(chain.resolve(vid("A")), vid("D"));
    }

    #[rstest]
    fn test_chains_for_different_orders_stay_separate() {
        let mut chain = ReplacementChain::default();
        chain.record(vid("A"), vid("B"));
        chain.record(vid("X"), vid("Y"));
        assert_eq!(chain.resolve(vid("A")), vid("B"));
        assert_eq!(chain.resolve(vid("X")), vid("Y"));
    }

    #[rstest]
    fn test_forget_drops_the_mapping() {
        let mut chain = ReplacementChain::default();
        chain.record(vid("A"), vid("B"));
        chain.forget(vid("A"));
        assert_eq!(chain.resolve(vid("A")), vid("A"));
    }
}
