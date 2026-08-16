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

//! Captures frames from the Alpaca trading event stream on the **paper** environment.
//!
//! Run with:
//!
//! ```bash
//! cargo run -p nautilus-alpaca --example stream_capture
//! ```
//!
//! Connects, completes the handshake, then submits and cancels one resting limit order so the
//! stream has something to deliver. Every trade update is printed as received, and the order is
//! cancelled even if a step fails, so nothing is left working.

use std::time::Duration;

use nautilus_alpaca::{
    common::{
        credential::AlpacaCredential,
        enums::AlpacaEnvironment,
        order_enums::{AlpacaOrderSide, AlpacaOrderType, AlpacaTimeInForce},
    },
    http::{
        client::AlpacaRawHttpClient,
        query::{ListOrdersParams, SubmitOrderRequest},
    },
    websocket::client::connect_trading_stream,
};

const SYMBOL: &str = "AAPL";
/// Far below the market, so the order rests rather than filling.
const RESTING_LIMIT: &str = "1.00";
/// How long to wait for the stream to deliver events after the order activity.
const LISTEN_WINDOW: Duration = Duration::from_secs(12);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let credential = AlpacaCredential::from_env()?;
    let client = AlpacaRawHttpClient::from_env(AlpacaEnvironment::Paper)?;

    println!("connecting to the trading event stream");
    let mut stream = connect_trading_stream(AlpacaEnvironment::Paper, &credential, None).await?;
    println!("handshake complete: {stream:?}");

    let client_order_id = format!("nautilus-capture-{}", std::process::id());
    let request = SubmitOrderRequest {
        symbol: SYMBOL.to_string(),
        qty: "1".to_string(),
        side: AlpacaOrderSide::Buy,
        order_type: AlpacaOrderType::Limit,
        time_in_force: AlpacaTimeInForce::Gtc,
        client_order_id: Some(client_order_id.clone()),
        limit_price: Some(RESTING_LIMIT.to_string()),
        stop_price: None,
        extended_hours: false,
    };

    println!("submitting {SYMBOL} buy 1 @ {RESTING_LIMIT}");
    let submitted = client.submit_order(&request).await?;
    println!("  venue id: {}", submitted.id);

    // Collect whatever the stream delivers for the submission, then cancel and collect again.
    drain(&mut stream, Duration::from_secs(5), "after submit").await;

    println!("cancelling {}", submitted.id);
    if let Err(e) = client.cancel_order(&submitted.id).await {
        eprintln!("  cancel failed: {e}");
    }

    drain(&mut stream, LISTEN_WINDOW, "after cancel").await;

    let open = client.list_orders(&ListOrdersParams::open()).await?;
    println!("open orders remaining: {}", open.len());

    Ok(())
}

/// Prints every trade update arriving within `window`.
async fn drain(
    stream: &mut nautilus_alpaca::websocket::client::AlpacaTradingStream,
    window: Duration,
    label: &str,
) {
    println!("--- listening {label} for {window:?} ---");
    let deadline = tokio::time::Instant::now() + window;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }

        match tokio::time::timeout(remaining, stream.next_update()).await {
            Ok(Some(Ok(update))) => {
                println!(
                    "  event={:?} (raw={}) order_status={} exec_id={:?} price={:?} qty={:?}",
                    update.event_kind(),
                    update.event,
                    update.order.status,
                    update.execution_id,
                    update.price,
                    update.qty
                );
            }
            Ok(Some(Err(e))) => println!("  decode error: {e}"),
            Ok(None) => {
                println!("  stream closed");
                break;
            }
            Err(_) => break, // Window elapsed
        }
    }
}
