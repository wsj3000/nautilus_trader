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

//! Checks that one execution carries the same identifier on both paths that report it.
//!
//! Run with:
//!
//! ```bash
//! cargo run -p nautilus-alpaca --example execution_id_check -- --run
//! ```
//!
//! # What it settles
//!
//! Fills reach the engine two ways. The trading event stream reports them live with an
//! `execution_id`, and startup reconciliation recovers them from the activity feed, where the
//! record's `id` is `<sequence>::<uuid>` and the adapter takes the half after the separator as the
//! `TradeId`.
//!
//! The engine de-duplicates fills by `TradeId` — `position_contains_trade_id` in the execution
//! engine — so those two must resolve to the same value. If they do not, an execution seen on both
//! paths is applied to the position twice.
//!
//! The two paths overlap only in a narrow window, because reconciliation runs at startup while the
//! stream connects, so a fill landing between the activity fetch and the stream subscription can
//! arrive on both. Narrow is not never, and the failure is a silently wrong position.
//!
//! # Why this needs an open market
//!
//! It has to produce a real execution. One share is bought with a limit priced through the last
//! trade, marked for extended hours, so it fills in the pre-market and after-hours sessions as well
//! as the regular one. A market order would have been simpler but the venue accepts those only
//! between 09:30 and 16:00 Eastern, and an adapter built for 24/5 coverage should not be verified
//! by something that only works for six and a half hours of it.
//!
//! When nothing is trading it reports that it could not get a fill rather than pretending to have
//! checked. That distinction is the point: run against a closed market it still receives the
//! `accepted` event on the stream, and a check that treated any event as success would pass
//! without having compared anything.
//!
//! Paper only, and it cancels what it created on every exit path.

use std::time::Duration;

use nautilus_alpaca::{
    common::{
        credential::AlpacaCredential,
        enums::{AlpacaDataFeed, AlpacaEnvironment},
        order_enums::{AlpacaOrderSide, AlpacaOrderType, AlpacaTimeInForce},
        reg_nms,
    },
    http::{
        client::AlpacaRawHttpClient,
        parse_exec::extract_execution_id,
        query::{ActivitiesParams, BarsParams, SubmitOrderRequest},
    },
    websocket::client::connect_trading_stream,
};
use rust_decimal::Decimal;

/// One share, so a fill costs about the price of the share and nothing is left over to unwind.
const QTY: &str = "1";
const SYMBOL: &str = "AAPL";

/// How far through the last trade to price the limit, so it crosses and fills.
///
/// Wide enough that the book moving between the quote and the submission does not leave the order
/// resting, and on one share of a large cap the difference is small change in a paper account.
const CROSS_PCT: f64 = 0.01;

/// How long to wait for the fill to appear on the stream.
const STREAM_WINDOW: Duration = Duration::from_secs(20);

/// How long to wait for the same fill to appear on the activity feed, which lags the stream.
const ACTIVITY_ATTEMPTS: usize = 10;
const ACTIVITY_INTERVAL: Duration = Duration::from_secs(3);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Paper only: this submits a marketable order, and the live environment would spend real money.
    let credential = AlpacaCredential::from_env()?;
    let mut client = AlpacaRawHttpClient::from_env(AlpacaEnvironment::Paper)?;
    if let Ok(base) = std::env::var("ALPACA_PROXY_URL") {
        println!("routing REST through proxy at {base}");
        client.set_trading_base_url(base.clone());
        client.set_data_base_url(base);
    }

    let clock = client.get_clock().await?;
    println!("venue clock: is_open={}", clock.is_open);

    if !std::env::args().any(|arg| arg == "--run") {
        println!(
            "Pass --run to submit one marketable {SYMBOL} order and compare the identifiers.\n\
             This spends about one share of {SYMBOL} in the paper account and needs an open \
             session to fill."
        );
        return Ok(());
    }

    // Connect before submitting. The stream does not replay, so a subscription completed after the
    // fill would miss the very event this check exists to read.
    println!("connecting to the trading event stream");
    let stream_url = std::env::var("ALPACA_PROXY_WS").ok();
    let mut stream = connect_trading_stream(AlpacaEnvironment::Paper, &credential, stream_url)
        .await
        .map_err(|e| format!("could not connect the trading stream: {e}"))?;

    let last = last_trade_price(&client).await?;
    let limit = marketable_limit(last)?;
    println!("last {SYMBOL} print {last:.2}, buying {QTY} at limit {limit}");

    let client_order_id = format!("nautilus-execid-{}", std::process::id());
    let request = SubmitOrderRequest {
        symbol: SYMBOL.to_string(),
        qty: QTY.to_string(),
        side: AlpacaOrderSide::Buy,
        // Limit rather than market: the venue takes market orders only during the regular session,
        // and this needs to run whenever anything is trading.
        order_type: AlpacaOrderType::Limit,
        // Extended-hours orders must be limit and day; the venue rejects other combinations.
        time_in_force: AlpacaTimeInForce::Day,
        client_order_id: Some(client_order_id.clone()),
        limit_price: Some(limit.clone()),
        stop_price: None,
        extended_hours: true,
    };

    println!("submitting");
    let submitted = client.submit_order(&request).await?;
    let venue_order_id = submitted.id.clone();
    println!("  venue id: {venue_order_id}");

    let outcome = compare(&client, &mut stream, &venue_order_id).await;

    // Whatever happened, do not leave a working order behind.
    println!("cancelling {venue_order_id} if it is still working");
    match client.cancel_order(&venue_order_id).await {
        Ok(()) => println!("  cancel accepted"),
        Err(e) => println!("  nothing to cancel ({e})"),
    }

    match outcome {
        Ok(true) => {
            println!("\nMATCH: the activity id's UUID half equals the stream's execution_id.");
            println!("A fill recovered at startup and the same fill seen live de-duplicate.");
            Ok(())
        }
        Ok(false) => {
            eprintln!(
                "\nMISMATCH: the two paths report different identifiers for one execution.\n\
                 The engine de-duplicates fills by TradeId, so this execution would be applied to \
                 the position twice. Do not run reconciliation against real positions until the \
                 adapter derives a shared identifier."
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("\nINCONCLUSIVE: {e}");
            eprintln!("Nothing was proven either way. Re-run during an open session.");
            std::process::exit(2);
        }
    }
}

/// Returns the close of the most recent minute bar, as a stand-in for the last print.
async fn last_trade_price(client: &AlpacaRawHttpClient) -> Result<f64, String> {
    let mut params = BarsParams::new(&[SYMBOL.to_string()], "1Min").with_feed(AlpacaDataFeed::Sip);
    params.limit = Some(1);

    let response = client
        .get_bars(&params)
        .await
        .map_err(|e| format!("could not read a recent {SYMBOL} bar to price the order: {e}"))?;

    response
        .bars
        .get(SYMBOL)
        .and_then(|bars| bars.last())
        .map(|bar| bar.c)
        .ok_or_else(|| format!("the venue returned no recent {SYMBOL} bars to price the order"))
}

/// Returns a buy limit priced through `last`, on a tick the venue will accept.
///
/// The increment comes from the adapter's own Rule 612 implementation rather than being written
/// out again here: the rule is not symmetric around a dollar and the half-penny amendment moves in
/// 2027, so a second copy would be a second thing to get wrong. Rounding up keeps a buy marketable.
fn marketable_limit(last: f64) -> Result<String, String> {
    let crossed = Decimal::from_f64_retain(last * (1.0 + CROSS_PCT))
        .ok_or_else(|| format!("last price {last} is not representable as a decimal"))?;

    let increment = reg_nms::min_price_increment(crossed);
    let ticks = (crossed / increment).ceil();
    let price = ticks * increment;

    if !reg_nms::is_valid_order_price(price) {
        return Err(format!("computed limit {price} is not a valid order price"));
    }
    Ok(price.normalize().to_string())
}

/// Reads the execution from both paths and reports whether the identifiers agree.
async fn compare(
    client: &AlpacaRawHttpClient,
    stream: &mut nautilus_alpaca::websocket::client::AlpacaTradingStream,
    venue_order_id: &str,
) -> Result<bool, String> {
    let stream_execution_id = wait_for_stream_fill(stream, venue_order_id).await?;
    println!("stream execution_id: {stream_execution_id}");

    let activity_id = wait_for_activity_fill(client, venue_order_id).await?;
    println!("activity id:         {activity_id}");

    let derived = extract_execution_id(&activity_id)
        .map_err(|e| format!("the activity id yields no usable trade id: {e}"))?;
    println!("derived TradeId:     {derived}");

    Ok(derived == stream_execution_id)
}

/// Waits for a fill on the stream for this order and returns its `execution_id`.
async fn wait_for_stream_fill(
    stream: &mut nautilus_alpaca::websocket::client::AlpacaTradingStream,
    venue_order_id: &str,
) -> Result<String, String> {
    let deadline = tokio::time::Instant::now() + STREAM_WINDOW;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(format!(
                "no fill arrived on the stream within {STREAM_WINDOW:?}; the market is likely \
                 closed, so the order rested instead of filling"
            ));
        }

        match tokio::time::timeout(remaining, stream.next_update()).await {
            Ok(Some(Ok(update))) => {
                if update.order.id != venue_order_id {
                    continue;
                }
                println!(
                    "  stream: event={} status={}",
                    update.event, update.order.status
                );

                // Only a fill carries the identifier this compares. The venue puts an
                // `execution_id` on lifecycle events too — `new` has one — so taking the first
                // that happened to be present compared the id of an acknowledgement against the id
                // of an execution, which differ by construction. `has_fill_detail` is the same gate
                // the adapter applies before it builds a fill report.
                if !update.has_fill_detail() {
                    continue;
                }
                if let Some(execution_id) = update.execution_id.clone() {
                    return Ok(execution_id);
                }
            }
            Ok(Some(Err(e))) => println!("  decode error: {e}"),
            Ok(None) => return Err("the stream closed before a fill arrived".to_string()),
            Err(_) => {}
        }
    }
}

/// Polls the activity feed until the fill for this order appears, and returns its `id`.
///
/// The feed lags the stream, so a single fetch straight after the fill usually misses it.
async fn wait_for_activity_fill(
    client: &AlpacaRawHttpClient,
    venue_order_id: &str,
) -> Result<String, String> {
    for attempt in 1..=ACTIVITY_ATTEMPTS {
        let activities = client
            .list_fill_activities(&ActivitiesParams::fills())
            .await
            .map_err(|e| format!("could not read the activity feed: {e}"))?;

        if let Some(activity) = activities.iter().find(|a| a.order_id == venue_order_id) {
            return Ok(activity.id.clone());
        }

        println!("  activity feed: not there yet (attempt {attempt}/{ACTIVITY_ATTEMPTS})");
        tokio::time::sleep(ACTIVITY_INTERVAL).await;
    }

    Err(format!(
        "the fill never appeared on the activity feed within \
         {}s",
        ACTIVITY_ATTEMPTS as u64 * ACTIVITY_INTERVAL.as_secs()
    ))
}
