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

//! Integration tests for the Alpaca HTTP client against a mock Axum server.
//!
//! These cover the two paging cursors, which cannot be tested from the payload fixtures: paging is
//! about the sequence of requests, not the shape of any one response. Activities page on an
//! opaque token; orders page on an exclusive time bound.

use std::{
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use axum::{Router, extract::Query, response::Json, routing::get};
use nautilus_alpaca::{
    common::{credential::AlpacaCredential, enums::AlpacaEnvironment},
    http::{
        client::AlpacaRawHttpClient,
        query::{ActivitiesParams, ListOrdersParams},
    },
};
use rstest::rstest;
use serde::Deserialize;
use serde_json::{Value, json};

/// The venue's own page ceiling for activities.
const PAGE_SIZE: usize = 100;

/// The venue's own page ceiling for orders.
const ORDERS_PAGE_SIZE: usize = 500;

#[derive(Debug, Deserialize)]
struct ActivitiesQuery {
    page_size: Option<usize>,
    page_token: Option<String>,
    after: Option<String>,
    until: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OrdersQuery {
    status: Option<String>,
    limit: Option<usize>,
    symbols: Option<String>,
    after: Option<String>,
    until: Option<String>,
}

/// The `after` and `until` bounds one request carried.
type Window = (Option<String>, Option<String>);

/// Records what each request asked for, so the test can assert on the sequence.
#[derive(Debug, Clone, Default)]
struct Recorder {
    tokens: Arc<Mutex<Vec<Option<String>>>>,
    windows: Arc<Mutex<Vec<Window>>>,
    requests: Arc<AtomicUsize>,
}

/// The activity id for a record, which is also the page token that follows it.
fn activity_id(index: usize) -> String {
    format!("2026062421123{index:04}::1444f6ad-0000-4000-8000-{index:012}")
}

fn activity(index: usize) -> Value {
    json!({
        "id": activity_id(index),
        "activity_type": "FILL",
        "type": "fill",
        "transaction_time": "2026-06-25T01:12:30.709802Z",
        "symbol": "AAPL",
        "side": "buy",
        "price": "201.45",
        "qty": "1",
        // Deliberately unrelated to the index. When this shared the id's trailing digits, a cursor
        // built from `order_id` decoded to the same page as one built from `id`, so the mock and
        // the client agreed while both were reading the wrong field.
        "order_id": format!("0000aaaa-0000-4000-8000-{:012}", 900_000 - index),
        "cum_qty": "1",
        "leaves_qty": "0",
        "order_status": "filled",
    })
}

/// Serves `total` activities across as many pages as that takes.
///
/// The venue returns a bare array, so the only signal that a page is the last one is that it is
/// shorter than requested. This reproduces that, including the case where the total is an exact
/// multiple of the page size and the final request comes back empty.
async fn start_server(total: usize, recorder: Recorder) -> SocketAddr {
    let router = Router::new().route(
        "/v2/account/activities",
        get(move |Query(query): Query<ActivitiesQuery>| {
            let recorder = recorder.clone();
            async move {
                recorder.requests.fetch_add(1, Ordering::SeqCst);
                recorder
                    .tokens
                    .lock()
                    .unwrap()
                    .push(query.page_token.clone());
                recorder
                    .windows
                    .lock()
                    .unwrap()
                    .push((query.after.clone(), query.until.clone()));

                let size = query.page_size.unwrap_or(PAGE_SIZE);
                // The cursor must be the previous page's `id` exactly. Resolving it by lookup
                // rather than by decoding digits means a token taken from any other field finds
                // no record and pages from the start, which the tests then catch as a duplicate.
                let start = match query.page_token {
                    Some(token) => (0..total)
                        .find(|i| activity_id(*i) == token)
                        .map_or(0, |i| i + 1),
                    None => 0,
                };
                let end = total.min(start + size);
                let page: Vec<Value> = (start..end).map(activity).collect();
                Json(page)
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router.into_make_service())
            .await
            .unwrap();
    });
    addr
}

fn client(addr: SocketAddr) -> AlpacaRawHttpClient {
    let credential = AlpacaCredential::new("test-key".to_string(), "test-secret".to_string());
    let mut client =
        AlpacaRawHttpClient::with_credentials(credential, AlpacaEnvironment::Paper, 10, None, None)
            .unwrap();
    client.set_trading_base_url(format!("http://{addr}"));
    client
}

#[rstest]
#[tokio::test]
async fn test_activities_paging_follows_the_cursor_across_pages() {
    let recorder = Recorder::default();
    let addr = start_server(250, recorder.clone()).await;

    let activities = client(addr)
        .list_fill_activities_all_pages(&ActivitiesParams::fills(), 10)
        .await
        .unwrap();

    assert_eq!(activities.len(), 250);
    // Three requests: two full pages and a short one that ends the walk.
    assert_eq!(recorder.requests.load(Ordering::SeqCst), 3);

    // Assert the cursor's exact value, not merely that one was sent. Checking only for presence
    // let a cursor built from the neighbouring `order_id` pass, because both fields once ended in
    // the same digits.
    let tokens = recorder.tokens.lock().unwrap().clone();
    assert_eq!(tokens[0], None, "the first request carries no cursor");
    assert_eq!(tokens[1], Some(activity_id(99)));
    assert_eq!(tokens[2], Some(activity_id(199)));

    // Every fill is distinct: a mis-threaded cursor would repeat a page.
    let ids: std::collections::HashSet<_> = activities.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids.len(), 250);
}

#[rstest]
#[tokio::test]
async fn test_activities_paging_stops_on_a_short_first_page() {
    let recorder = Recorder::default();
    let addr = start_server(7, recorder.clone()).await;

    let activities = client(addr)
        .list_fill_activities_all_pages(&ActivitiesParams::fills(), 10)
        .await
        .unwrap();

    assert_eq!(activities.len(), 7);
    // A second request here would be a wasted round trip on every reconciliation.
    assert_eq!(recorder.requests.load(Ordering::SeqCst), 1);
}

#[rstest]
#[tokio::test]
async fn test_activities_paging_handles_an_exact_multiple_of_the_page_size() {
    let recorder = Recorder::default();
    let addr = start_server(200, recorder.clone()).await;

    let activities = client(addr)
        .list_fill_activities_all_pages(&ActivitiesParams::fills(), 10)
        .await
        .unwrap();

    // The venue cannot say "that was the last page", so an empty third page is what ends it.
    assert_eq!(activities.len(), 200);
    assert_eq!(recorder.requests.load(Ordering::SeqCst), 3);
}

#[rstest]
#[tokio::test]
async fn test_activities_paging_stops_at_the_page_limit() {
    let recorder = Recorder::default();
    let addr = start_server(1_000, recorder.clone()).await;

    let activities = client(addr)
        .list_fill_activities_all_pages(&ActivitiesParams::fills(), 2)
        .await
        .unwrap();

    // Truncated rather than looping to the end. The client logs a warning, because silently
    // returning a partial history would read as "these are all the fills".
    assert_eq!(activities.len(), 200);
    assert_eq!(recorder.requests.load(Ordering::SeqCst), 2);
}

#[rstest]
#[tokio::test]
async fn test_activities_window_is_sent_on_every_page() {
    let recorder = Recorder::default();
    let addr = start_server(250, recorder.clone()).await;

    let params = ActivitiesParams::fills().with_window(
        Some("2026-06-01T00:00:00Z".to_string()),
        Some("2026-07-01T00:00:00Z".to_string()),
    );
    client(addr)
        .list_fill_activities_all_pages(&params, 10)
        .await
        .unwrap();

    // Dropping the bounds after the first page would widen the walk to the whole account history.
    let windows = recorder.windows.lock().unwrap().clone();
    assert_eq!(windows.len(), 3);
    for window in windows {
        assert_eq!(
            window,
            (
                Some("2026-06-01T00:00:00Z".to_string()),
                Some("2026-07-01T00:00:00Z".to_string())
            )
        );
    }
}

// -------------------------------------------------------------------------------------------------
//  Orders
// -------------------------------------------------------------------------------------------------

/// Records the status, symbol, and window each orders request carried.
#[derive(Debug, Clone, Default)]
struct OrdersRecorder {
    statuses: Arc<Mutex<Vec<Option<String>>>>,
    symbols: Arc<Mutex<Vec<Option<String>>>>,
    windows: Arc<Mutex<Vec<Window>>>,
    requests: Arc<AtomicUsize>,
}

/// Orders are timestamped one second apart, newest first, so `index` and time run in opposite
/// directions exactly as they do at the venue.
fn order(index: usize) -> Value {
    let seconds = 100_000 - index;
    json!({
        "id": format!("0000aaaa-0000-4000-8000-{index:012}"),
        "client_order_id": format!("C-{index}"),
        "symbol": "AAPL",
        "side": "buy",
        "type": "limit",
        "time_in_force": "day",
        "status": "filled",
        "qty": "1",
        "filled_qty": "1",
        "limit_price": "200.00",
        "submitted_at": format!("2026-06-25T{:02}:{:02}:{:02}Z", seconds / 3600 % 24, seconds / 60 % 60, seconds % 60),
        "created_at": "2026-06-25T00:00:00Z",
    })
}

/// Serves `total` orders newest first, paging on the exclusive `until` cursor as the venue does.
async fn start_orders_server(total: usize, recorder: OrdersRecorder) -> SocketAddr {
    let router = Router::new().route(
        "/v2/orders",
        get(move |Query(query): Query<OrdersQuery>| {
            let recorder = recorder.clone();
            async move {
                recorder.requests.fetch_add(1, Ordering::SeqCst);
                recorder.statuses.lock().unwrap().push(query.status.clone());
                recorder.symbols.lock().unwrap().push(query.symbols.clone());
                recorder
                    .windows
                    .lock()
                    .unwrap()
                    .push((query.after.clone(), query.until.clone()));

                let limit = query.limit.unwrap_or(ORDERS_PAGE_SIZE);
                // `until` is exclusive at the venue, so the boundary order is not served again.
                let start = match query.until {
                    Some(until) => (0..total)
                        .find(|i| order(*i)["submitted_at"].as_str().unwrap() < until.as_str())
                        .unwrap_or(total),
                    None => 0,
                };
                let end = total.min(start + limit);
                let page: Vec<Value> = (start..end).map(order).collect();
                Json(page)
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router.into_make_service())
            .await
            .unwrap();
    });
    addr
}

#[rstest]
#[tokio::test]
async fn test_orders_paging_follows_the_time_cursor() {
    let recorder = OrdersRecorder::default();
    let addr = start_orders_server(1_200, recorder.clone()).await;

    let orders = client(addr)
        .list_orders_all_pages(&ListOrdersParams::all(), 10)
        .await
        .unwrap();

    assert_eq!(orders.len(), 1_200);
    // Two full pages of 500 and a short third that ends the walk.
    assert_eq!(recorder.requests.load(Ordering::SeqCst), 3);

    // No order is served twice: the cursor bound is exclusive, so a page boundary that repeated
    // its edge would double-count an order in reconciliation.
    let ids: std::collections::HashSet<_> = orders.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(ids.len(), 1_200);

    let windows = recorder.windows.lock().unwrap().clone();
    assert_eq!(windows[0].1, None, "the first request carries no cursor");
    assert!(windows[1].1.is_some() && windows[2].1.is_some());
}

#[rstest]
#[tokio::test]
async fn test_orders_paging_stops_on_a_short_page() {
    let recorder = OrdersRecorder::default();
    let addr = start_orders_server(12, recorder.clone()).await;

    let orders = client(addr)
        .list_orders_all_pages(&ListOrdersParams::open(), 10)
        .await
        .unwrap();

    assert_eq!(orders.len(), 12);
    assert_eq!(recorder.requests.load(Ordering::SeqCst), 1);
    assert_eq!(
        recorder.statuses.lock().unwrap()[0],
        Some("open".to_string())
    );
}

#[rstest]
#[tokio::test]
async fn test_orders_paging_stops_at_the_page_limit() {
    let recorder = OrdersRecorder::default();
    let addr = start_orders_server(5_000, recorder.clone()).await;

    let orders = client(addr)
        .list_orders_all_pages(&ListOrdersParams::all(), 2)
        .await
        .unwrap();

    // Truncated rather than walked to the end, and the client logs that it stopped short —
    // silently returning part of the history would read as the whole of it.
    assert_eq!(orders.len(), 1_000);
    assert_eq!(recorder.requests.load(Ordering::SeqCst), 2);
}

#[rstest]
#[tokio::test]
async fn test_orders_status_and_symbol_reach_the_venue() {
    let recorder = OrdersRecorder::default();
    let addr = start_orders_server(3, recorder.clone()).await;

    let params = ListOrdersParams::all().with_symbol("AAPL").with_window(
        Some("2026-06-01T00:00:00Z".to_string()),
        Some("2026-07-01T00:00:00Z".to_string()),
    );
    client(addr)
        .list_orders_all_pages(&params, 5)
        .await
        .unwrap();

    // `status=all` is what makes finished orders visible; dropping it would send the engine back
    // to querying each closed order one at a time.
    assert_eq!(
        recorder.statuses.lock().unwrap()[0],
        Some("all".to_string())
    );
    assert_eq!(
        recorder.symbols.lock().unwrap()[0],
        Some("AAPL".to_string())
    );
    assert_eq!(
        recorder.windows.lock().unwrap()[0].0,
        Some("2026-06-01T00:00:00Z".to_string())
    );
}
