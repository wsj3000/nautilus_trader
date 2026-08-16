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
//! These cover the activities cursor, which cannot be tested from the payload fixtures: paging is
//! about the sequence of requests, not the shape of any one response.

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
    http::{client::AlpacaRawHttpClient, query::ActivitiesParams},
};
use rstest::rstest;
use serde::Deserialize;
use serde_json::{Value, json};

/// The venue's own page ceiling.
const PAGE_SIZE: usize = 100;

#[derive(Debug, Deserialize)]
struct ActivitiesQuery {
    page_size: Option<usize>,
    page_token: Option<String>,
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

fn activity(index: usize) -> Value {
    json!({
        "id": format!("2026062421123{index:04}::1444f6ad-0000-4000-8000-{index:012}"),
        "activity_type": "FILL",
        "type": "fill",
        "transaction_time": "2026-06-25T01:12:30.709802Z",
        "symbol": "AAPL",
        "side": "buy",
        "price": "201.45",
        "qty": "1",
        "order_id": format!("0000aaaa-0000-4000-8000-{index:012}"),
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
                // The cursor is the previous page's last `id`, whose trailing digits are its index.
                let start = match query.page_token {
                    Some(token) => {
                        let suffix = token.rsplit('-').next().unwrap_or_default();
                        suffix.parse::<usize>().unwrap_or_default() + 1
                    }
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

    let tokens = recorder.tokens.lock().unwrap().clone();
    assert_eq!(tokens[0], None, "the first request carries no cursor");
    assert!(tokens[1].is_some() && tokens[2].is_some());

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
