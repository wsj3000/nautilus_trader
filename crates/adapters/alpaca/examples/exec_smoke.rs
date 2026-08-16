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

//! Round-trip check of the Alpaca order lifecycle against the **paper** environment.
//!
//! Run with:
//!
//! ```bash
//! cargo run -p nautilus-alpaca --example exec_smoke -- --run
//! ```
//!
//! Submits one limit order priced far below the market so it cannot fill, amends it, then cancels
//! it. The environment is hard-coded to paper, and the run cancels what it created even when a
//! step fails, so it leaves no working order behind.
//!
//! Without `--run` it only reports the account and any existing open orders.

use nautilus_alpaca::{
    common::{
        enums::AlpacaEnvironment,
        order_enums::{AlpacaOrderSide, AlpacaOrderType, AlpacaTimeInForce},
    },
    http::{
        client::AlpacaRawHttpClient,
        query::{ListOrdersParams, ReplaceOrderRequest, SubmitOrderRequest},
    },
};

/// A price no US large cap trades near, so the order rests instead of filling.
const RESTING_LIMIT: &str = "1.00";
const AMENDED_LIMIT: &str = "1.01";
const SYMBOL: &str = "AAPL";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Paper only: this example submits live orders, and pointing it at the live environment would
    // transact real capital.
    let mut client = AlpacaRawHttpClient::from_env(AlpacaEnvironment::Paper)?;
    if let Ok(base) = std::env::var("ALPACA_PROXY_URL") {
        println!("routing through proxy at {base}");
        client.set_trading_base_url(base.clone());
        client.set_data_base_url(base);
    }

    let account = client.get_account().await?;
    println!(
        "account: status={} blocked={}",
        account.status,
        account.is_blocked()
    );

    let open = client.list_orders(&ListOrdersParams::open()).await?;
    println!("open orders before: {}", open.len());

    if !std::env::args().any(|arg| arg == "--run") {
        println!("Pass --run to exercise the submit / amend / cancel round trip.");
        return Ok(());
    }

    // Unique per run: the venue rejects a repeated client order ID, so a fixed one only works
    // once per account.
    let client_order_id = format!(
        "nautilus-smoke-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs()
    );
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

    println!("submitting {SYMBOL} buy 1 @ {RESTING_LIMIT} (client_order_id={client_order_id})");
    let submitted = client.submit_order(&request).await?;
    println!(
        "  accepted: id={} status={} type={:?} tif={:?} limit={:?}",
        submitted.id,
        submitted.status,
        submitted.resolved_order_type()?,
        submitted.time_in_force,
        submitted.limit_price
    );

    // Whatever happens next, the order created above must not be left working.
    let outcome = exercise(&client, &submitted.id).await;

    let final_id = match &outcome {
        Ok(replacement_id) => replacement_id.clone(),
        Err(e) => {
            eprintln!("round trip failed before cancellation: {e}");
            submitted.id.clone()
        }
    };

    println!("cancelling {final_id}");
    match client.cancel_order(&final_id).await {
        Ok(()) => println!("  cancelled"),
        Err(e) => eprintln!("  cancel failed, order may still be working: {e}"),
    }

    let after = client.get_order(&final_id).await?;
    println!("  final status: {}", after.status);

    let open_after = client.list_orders(&ListOrdersParams::open()).await?;
    println!("open orders after: {}", open_after.len());

    outcome?;
    Ok(())
}

/// Amends the order and returns the identifier it now trades under.
async fn exercise(
    client: &AlpacaRawHttpClient,
    venue_order_id: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let fetched = client.get_order(venue_order_id).await?;
    println!(
        "  fetched back: status={} qty={:?}",
        fetched.status, fetched.qty
    );

    println!("amending limit {RESTING_LIMIT} -> {AMENDED_LIMIT}");
    let replacement = client
        .replace_order(
            venue_order_id,
            &ReplaceOrderRequest {
                limit_price: Some(AMENDED_LIMIT.to_string()),
                ..ReplaceOrderRequest::default()
            },
        )
        .await?;

    println!(
        "  replacement: id={} status={} limit={:?} replaces={:?}",
        replacement.id, replacement.status, replacement.limit_price, replacement.replaces
    );

    // The identifier must change: the adapter's replacement chain exists because of this.
    if replacement.id == venue_order_id {
        return Err("venue reused the order identifier across an amendment".into());
    }

    let original = client.get_order(venue_order_id).await?;
    println!(
        "  original now: status={} replaced_by={:?}",
        original.status, original.replaced_by
    );

    Ok(replacement.id)
}
