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

//! Sanity-check example that exercises the Alpaca Trading REST API against the paper environment.
//!
//! Run with:
//!
//! ```bash
//! cargo run -p nautilus-alpaca --example rest_smoke
//! ```
//!
//! Requires `APCA_API_KEY_ID` and `APCA_API_SECRET_KEY` in the environment. Three requests are
//! issued against the Trading API; the Market Data API is not touched.
//!
//! Account balances and identifiers are deliberately not printed.

use nautilus_alpaca::{
    common::enums::AlpacaEnvironment,
    http::{client::AlpacaRawHttpClient, query::ListAssetsParams},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = AlpacaRawHttpClient::from_env(AlpacaEnvironment::Paper)?;
    println!(
        "client constructed, credentials present: {}",
        client.has_credentials()
    );

    let clock = client.get_clock().await?;
    println!(
        "clock: is_open={} next_open={} next_close={}",
        clock.is_open, clock.next_open, clock.next_close
    );

    let account = client.get_account().await?;
    println!(
        "account: status={} currency={} blocked={} pdt={}",
        account.status,
        account.currency,
        account.is_blocked(),
        account.pattern_day_trader
    );

    let assets = client
        .list_assets(&ListAssetsParams::active_us_equities())
        .await?;
    println!("assets: {} active us_equity", assets.len());

    let tradable = assets.iter().filter(|a| a.is_tradable_now()).count();
    let overnight = assets.iter().filter(|a| a.is_overnight_tradable()).count();
    let halted = assets.iter().filter(|a| a.is_overnight_halted()).count();
    println!("  tradable now:       {tradable}");
    println!("  overnight tradable: {overnight}");
    println!("  overnight halted:   {halted}");

    if let Some(aapl) = assets.iter().find(|a| a.symbol == "AAPL") {
        println!(
            "  AAPL: exchange={} fractionable={} overnight={} mmr={:?}",
            aapl.exchange,
            aapl.fractionable,
            aapl.is_overnight_tradable(),
            aapl.maintenance_margin_requirement
        );
    }

    Ok(())
}
