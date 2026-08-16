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

//! Compares the venue's current payloads against the captured fixtures.
//!
//! The unit tests parse fixtures stored under `test_data/`, which proves the models match what the
//! venue returned when those were captured — not what it returns now. A field renamed today would
//! leave every one of those tests passing. This closes that gap by fetching the same endpoints
//! live and diffing their **shape**: field names, JSON types, and enumeration values.
//!
//! Run with:
//!
//! ```bash
//! cargo run -p nautilus-alpaca --example conformance
//! ```
//!
//! Read-only: it fetches, and never submits or cancels anything.
//!
//! Exit status is 1 when a breaking difference is found, so it can be scheduled and alerted on.
//! Additive differences are reported but do not fail: the adapter ignores unknown fields, so a new
//! one is news rather than damage.

use std::{collections::BTreeSet, process::ExitCode};

use serde_json::Value;

/// Fetches a payload exactly as the venue sends it.
///
/// This deliberately bypasses the adapter's typed client. Comparing a payload that has been parsed
/// into the adapter's models and serialized back would only ever reveal differences between those
/// models and the fixtures — never between the fixtures and the venue, which is the whole point.
struct RawFetcher {
    http: reqwest::Client,
    trading_base: String,
    data_base: String,
    key: String,
    secret: String,
}

impl RawFetcher {
    fn from_env() -> anyhow::Result<Self> {
        let key = std::env::var("APCA_API_KEY_ID")
            .map_err(|_| anyhow::anyhow!("APCA_API_KEY_ID is not set"))?;
        let secret = std::env::var("APCA_API_SECRET_KEY")
            .map_err(|_| anyhow::anyhow!("APCA_API_SECRET_KEY is not set"))?;

        let (trading_base, data_base) = match std::env::var("ALPACA_PROXY_URL") {
            Ok(base) => {
                println!("routing through proxy at {base}");
                (base.clone(), base)
            }
            Err(_) => (
                "https://paper-api.alpaca.markets".to_string(),
                "https://data.alpaca.markets".to_string(),
            ),
        };

        Ok(Self {
            http: reqwest::Client::new(),
            trading_base,
            data_base,
            key,
            secret,
        })
    }

    async fn get(&self, base: &str, path_and_query: &str) -> anyhow::Result<Value> {
        let url = format!("{}{path_and_query}", base.trim_end_matches('/'));
        let response = self
            .http
            .get(&url)
            .header("APCA-API-KEY-ID", &self.key)
            .header("APCA-API-SECRET-KEY", &self.secret)
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            anyhow::bail!("{url} returned HTTP {status}: {body}");
        }
        Ok(serde_json::from_str(&body)?)
    }

    async fn trading(&self, path_and_query: &str) -> anyhow::Result<Value> {
        self.get(&self.trading_base.clone(), path_and_query).await
    }

    async fn data(&self, path_and_query: &str) -> anyhow::Result<Value> {
        self.get(&self.data_base.clone(), path_and_query).await
    }
}

/// How a difference is treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Severity {
    /// The adapter would lose data or fail to parse.
    Breaking,
    /// Worth knowing, but the adapter copes.
    Additive,
}

#[derive(Debug)]
struct Finding {
    severity: Severity,
    endpoint: &'static str,
    detail: String,
}

/// Returns the JSON type name, so a value changing from string to number is visible.
fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Collects the field names of an object, or of the first element when given an array.
fn fields(value: &Value) -> BTreeSet<String> {
    let object = match value {
        Value::Array(items) => items.first(),
        other => Some(other),
    };
    match object {
        Some(Value::Object(map)) => map.keys().cloned().collect(),
        _ => BTreeSet::new(),
    }
}

/// Returns the representative object from a payload: the object itself, or the first element.
fn representative(value: &Value) -> Option<&serde_json::Map<String, Value>> {
    match value {
        Value::Object(map) => Some(map),
        Value::Array(items) => items.first().and_then(|v| v.as_object()),
        _ => None,
    }
}

/// Compares a live payload against its fixture.
///
/// A field the fixture has and the payload lacks is breaking: the models were built against the
/// fixture, and a required field going missing fails deserialization. A field only the payload has
/// is additive, because unknown fields are ignored. A field whose type changed is breaking even
/// though the name is unchanged — that is the failure an unchanged field list would hide.
fn compare(endpoint: &'static str, fixture: &Value, live: &Value) -> Vec<Finding> {
    let mut findings = Vec::new();

    let fixture_fields = fields(fixture);
    let live_fields = fields(live);

    if fixture_fields.is_empty() {
        findings.push(Finding {
            severity: Severity::Breaking,
            endpoint,
            detail: "fixture carries no object to compare against".to_string(),
        });
        return findings;
    }

    if live_fields.is_empty() {
        findings.push(Finding {
            severity: Severity::Breaking,
            endpoint,
            detail: "the venue returned no object; the response shape has changed".to_string(),
        });
        return findings;
    }

    for missing in fixture_fields.difference(&live_fields) {
        findings.push(Finding {
            severity: Severity::Breaking,
            endpoint,
            detail: format!("field '{missing}' is no longer returned"),
        });
    }

    for added in live_fields.difference(&fixture_fields) {
        findings.push(Finding {
            severity: Severity::Additive,
            endpoint,
            detail: format!("field '{added}' is new"),
        });
    }

    if let (Some(fixture_map), Some(live_map)) = (representative(fixture), representative(live)) {
        for name in fixture_fields.intersection(&live_fields) {
            let (Some(before), Some(after)) = (fixture_map.get(name), live_map.get(name)) else {
                continue;
            };
            // A null on either side says nothing about the type, since the venue nulls optional
            // fields; comparing those would report a difference on every run.
            if before.is_null() || after.is_null() {
                continue;
            }
            if type_name(before) != type_name(after) {
                findings.push(Finding {
                    severity: Severity::Breaking,
                    endpoint,
                    detail: format!(
                        "field '{name}' changed type: {} -> {}",
                        type_name(before),
                        type_name(after)
                    ),
                });
            }
        }
    }

    findings
}

/// Reports enumeration values the adapter does not model.
///
/// These do not break parsing — unknown values fall to an `Unknown` variant — but they are the
/// earliest signal that the venue has added behaviour.
fn check_enum_values(
    endpoint: &'static str,
    live: &Value,
    field: &str,
    known: &[&str],
) -> Vec<Finding> {
    let Value::Array(items) = live else {
        return Vec::new();
    };

    let seen: BTreeSet<&str> = items
        .iter()
        .filter_map(|item| item.get(field).and_then(Value::as_str))
        .collect();

    seen.into_iter()
        .filter(|value| !known.contains(value))
        .map(|value| Finding {
            severity: Severity::Additive,
            endpoint,
            detail: format!("field '{field}' carries unmodelled value '{value}'"),
        })
        .collect()
}

fn load_fixture(name: &str) -> anyhow::Result<Value> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("test_data")
        .join(name);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("Cannot read fixture {}: {e}", path.display()))?;
    Ok(serde_json::from_str(&text)?)
}

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    let fetcher = RawFetcher::from_env()?;
    let mut findings = Vec::new();

    println!("comparing live payloads against captured fixtures\n");

    let live = fetcher.trading("/v2/clock").await?;
    findings.extend(compare(
        "/v2/clock",
        &load_fixture("http_clock.json")?,
        &live,
    ));

    // The extended-session fields here are what the session model depends on.
    let live = fetcher
        .trading("/v2/calendar?start=2026-08-14&end=2026-08-19")
        .await?;
    findings.extend(compare(
        "/v2/calendar",
        &load_fixture("http_calendar.json")?,
        &live,
    ));

    let live = fetcher
        .trading("/v2/assets?status=active&asset_class=us_equity")
        .await?;
    findings.extend(compare(
        "/v2/assets",
        &load_fixture("http_assets_sample.json")?,
        &live,
    ));
    findings.extend(check_enum_values(
        "/v2/assets",
        &live,
        "exchange",
        &["NASDAQ", "NYSE", "ARCA", "BATS", "AMEX", "OTC"],
    ));
    findings.extend(check_enum_values(
        "/v2/assets",
        &live,
        "status",
        &["active", "inactive"],
    ));

    let live = fetcher.trading("/v2/positions").await?;
    if live.as_array().is_some_and(|a| a.is_empty()) {
        println!("note: no open positions, so /v2/positions was not compared\n");
    } else {
        findings.extend(compare(
            "/v2/positions",
            &load_fixture("http_positions.json")?,
            &live,
        ));
    }

    // Startup reconciliation reads fills from here. `alpaca-py`'s `TradingClient` does not cover
    // this endpoint, so watching SDK releases would not surface a change to it either.
    let live = fetcher
        .trading("/v2/account/activities?activity_types=FILL&page_size=10")
        .await?;
    if live.as_array().is_some_and(|a| a.is_empty()) {
        println!("note: no fill activities, so /v2/account/activities was not compared\n");
    } else {
        findings.extend(compare(
            "/v2/account/activities",
            &load_fixture("http_activities_fills.json")?,
            &live,
        ));
        // An unmodelled side drops the fill from reconciliation, and the engine's position parts
        // company with the venue's without saying so.
        findings.extend(check_enum_values(
            "/v2/account/activities",
            &live,
            "side",
            &["buy", "sell", "sell_short"],
        ));
        findings.extend(check_enum_values(
            "/v2/account/activities",
            &live,
            "type",
            &["fill", "partial_fill"],
        ));
    }

    for (feed, start, end, fixture) in [
        (
            "sip",
            "2026-08-14T14:30:00Z",
            "2026-08-14T14:33:00Z",
            "http_bars_sip.json",
        ),
        (
            "boats",
            "2026-08-14T00:30:00Z",
            "2026-08-14T01:00:00Z",
            "http_bars_boats.json",
        ),
    ] {
        let live = fetcher
            .data(&format!(
                "/v2/stocks/bars?symbols=AAPL&timeframe=1Min&start={start}&end={end}&feed={feed}&limit=3"
            ))
            .await?;

        // The envelope itself is part of the contract, so it is compared before the bars.
        findings.extend(compare(
            "/v2/stocks/bars (envelope)",
            &load_fixture(fixture)?,
            &live,
        ));

        let pick = |v: &Value| v.get("bars").and_then(|b| b.get("AAPL")).cloned();
        match (pick(&load_fixture(fixture)?), pick(&live)) {
            (Some(before), Some(after)) => {
                findings.extend(compare("/v2/stocks/bars", &before, &after));
            }
            _ => findings.push(Finding {
                severity: Severity::Breaking,
                endpoint: "/v2/stocks/bars",
                detail: format!("feed '{feed}' returned no bars for the sampled window"),
            }),
        }
    }

    let live = fetcher.trading("/v2/orders?status=all&limit=1").await?;
    if live.as_array().is_some_and(|a| a.is_empty()) {
        println!("note: no order history, so /v2/orders was not compared\n");
    } else {
        findings.extend(compare(
            "/v2/orders",
            &load_fixture("http_orders.json")?,
            &live,
        ));
    }

    let breaking: Vec<_> = findings
        .iter()
        .filter(|f| f.severity == Severity::Breaking)
        .collect();
    let additive: Vec<_> = findings
        .iter()
        .filter(|f| f.severity == Severity::Additive)
        .collect();

    if !breaking.is_empty() {
        println!("BREAKING ({}):", breaking.len());
        for finding in &breaking {
            println!("  {} {}", finding.endpoint, finding.detail);
        }
        println!();
    }

    if !additive.is_empty() {
        println!("ADDITIVE ({}):", additive.len());
        for finding in &additive {
            println!("  {} {}", finding.endpoint, finding.detail);
        }
        println!();
    }

    if findings.is_empty() {
        println!("no differences: the fixtures still describe the venue");
    }

    Ok(if breaking.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
