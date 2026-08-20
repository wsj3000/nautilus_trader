# Alpaca

Alpaca is a US broker offering commission-free trading in US equities through a REST and
WebSocket API. This adapter covers US equities across the regular session and the Blue Ocean ATS
overnight session, giving continuous 24/5 coverage.

## Overview

The Alpaca adapter is implemented in Rust.

Components:

- `AlpacaRawHttpClient`: REST client spanning the Trading and Market Data hosts, with a separate rate limit quota for each.
- `AlpacaInstrumentProvider`: Instrument loading and caching.
- `AlpacaDataClient`: Market data client; bars are polled over REST.
- `AlpacaDataClientFactory`: Data client factory.
- `AlpacaExecutionClient`: Execution client; REST orders plus the trading event stream.
- `AlpacaExecutionClientFactory`: Execution client factory.
- `SessionCalendar`: Trading session resolution driving the SIP/overnight feed switch.

This adapter is Rust-only and exposes no Python bindings; a node using it is assembled in Rust.

## Scope

- **Products**: US equities only. Options and crypto are not covered.
- **Quantities**: whole shares only. Fractional shares and notional orders are not supported.
- **Market data feeds**: the consolidated SIP feed and the Blue Ocean ATS (BOATS) overnight feed. The IEX and delayed-SIP feeds are not exposed.

## Environments

`AlpacaEnvironment` selects the trading host and defaults to `PAPER`, so an unconfigured client
cannot reach the live endpoint.

| Environment | Trading host |
| ----------- | ------------ |
| `PAPER`     | `https://paper-api.alpaca.markets` |
| `LIVE`      | `https://api.alpaca.markets` |

Market data is served from `https://data.alpaca.markets` for both environments.

## Authentication

Set the credentials in the environment:

```bash
export APCA_API_KEY_ID=...
export APCA_API_SECRET_KEY=...
```

The venue authenticates with a static header pair rather than a signature, so the headers are
installed once as transport defaults.

## Sessions and 24/5 coverage

All boundaries are US Eastern wall-clock times, so their UTC instants shift with daylight saving.

```text
 20:00 (prev)      04:00        09:30        16:00        20:00
 ────────────────┬────────────┬────────────┬────────────┬──────────
    Overnight    │ PreMarket  │  Regular   │ AfterHours │ Overnight
     (BOATS)     │           (SIP)                      │  (BOATS)
```

Sessions are derived from `GET /v2/calendar`, which is the only source of holidays. The overnight
session is attributed to the trading day it **precedes**, so Sunday evening opens the week and
Friday evening closes it without special cases; the result is continuous coverage from Sunday
20:00 to Friday 20:00 Eastern.

A repeated hour at the autumn transition resolves to its first occurrence, and the nonexistent
hour at the spring transition is rejected rather than shifted into an adjacent session.

## Market data

Bars are **polled over REST** rather than streamed. Alpaca permits one concurrent market data
stream per account, and this adapter stays off it.

Each poll requests a rolling window rather than only the newest bar, so a poll that fails or
arrives late is recovered by the next one; bars already emitted are filtered out by timestamp. One
request serves every symbol sharing a timeframe, so cost scales with the number of timeframes
rather than the size of the universe.

Bars are stamped at their **close**. The venue stamps them at their open, and the engine's
convention is close, so the interval is added.

Bar prices are parsed at precision 4. Bars are built from executions, and Rule 612 constrains
quoting rather than trading, so sub-penny prints are routine.

### Not available over this transport

- Quotes, trades, and order book data. Polling cannot reconstruct a tick stream, so these
  subscriptions return an error rather than being accepted and never served.
- Trading halts and LULD events, which the venue publishes only on the market data stream.

## Instruments

`GET /v2/assets` publishes **no price increment** for equities, so tick sizing is derived from
SEC Reg NMS Rule 612 rather than read from the payload.

Instruments are built with `price_precision = 4` and `price_increment = 0.0001`. A coarser
precision would have the risk engine deny legitimate sub-dollar limit prices, which are permitted
to `$0.0001`. The complementary rule — orders at or above `$1.00` must be whole cents — is not
expressible as a single increment and is enforced at order submission instead.

`lot_size` is unset: the venue has no board lot and orders start at one share.

Assets are loaded only when both `status` is `active` **and** `tradable` is true; the two are
independent.

### Narrowing what is loaded

The venue lists roughly 13,400 tradable US equities and the adapter loads them all by default,
because a node cannot generally know in advance what it will trade. That costs about **93 MB
resident**, held twice — once in the provider's cache and once in the engine's — at roughly 3.5 KB
each. Measured on a node subscribing to a single bar type, it is 76% of the process:

| | RSS |
| --- | ---: |
| Before the instrument load | 28 MB |
| After loading 13,392 instruments | 121 MB |
| After reconciling 350 events | 123 MB |

A node that knows its instruments can name them:

```rust
AlpacaDataClientConfig {
    instrument_provider: AlpacaInstrumentProviderConfig {
        load_all: false,
        load_ids: Some(vec!["AAPL.ALPACA".to_string()]),
    },
    ..Default::default()
}
```

Selection happens before the asset is parsed, so an excluded instrument costs nothing beyond the
bytes the venue already sent.

**Instruments the account holds, or has working orders against, are loaded whether or not they are
listed.** Reconciliation reads positions and orders for the whole account rather than only for what
this node trades, and it skips any whose instrument is absent from the cache. Without this, a
narrowed load left the engine reporting a held position as flat — `Position discrepancy detected
for NVDA.ALPACA: cached_signed_qty=0, venue_signed_qty=4` — while 341 orders were skipped. Reading
positions or orders is best-effort: a failure is logged and the load continues, because fewer
instruments is recoverable and none is not.

Two things still need care. **Subscribing to an instrument that was not loaded is refused**, since
the data client checks the provider's cache first, so the list has to cover everything the node
subscribes to including anything a strategy adds later. And `load_all = false` with an empty or
entirely invalid `load_ids` loads nothing beyond what the account holds, and logs a warning, rather
than falling back to loading everything — a silent fallback would restore the cost the setting
exists to avoid. An individually malformed ID is logged and skipped; the rest still load.

### Overnight eligibility

Asset attributes carry overnight eligibility, and they are independent of one another:
`overnight_halted` appears on assets that do not carry `overnight_tradable`. Both are recorded on
the instrument's `info` map and both must be consulted before routing an overnight order.

## Orders capability

| Order type | Supported | Notes |
| ---------- | --------- | ----- |
| `MARKET` | ✓ | |
| `LIMIT` | ✓ | |
| `STOP_MARKET` | ✓ | |
| `STOP_LIMIT` | ✓ | |
| `TRAILING_STOP_MARKET` | ✓ | |
| `MARKET_IF_TOUCHED` | ✗ | Not offered by the venue |
| `LIMIT_IF_TOUCHED` | ✗ | Not offered by the venue |
| `MARKET_TO_LIMIT` | ✗ | Not offered by the venue |

| Time in force | Supported | Notes |
| ------------- | --------- | ----- |
| `DAY`, `GTC`, `IOC`, `FOK` | ✓ | |
| `AT_THE_OPEN`, `AT_THE_CLOSE` | ✓ | Venue `opg` and `cls` |
| `GTD` | ✗ | Rejected rather than widened to `GTC`, which would leave an order working past its intended expiry |

### Price validation

Order prices are validated against Rule 612 before submission and **denied** rather than rounded.
Rounding would change an order the caller specified; the venue rejects such prices anyway.

The increment depends on the **order's own price**, not on where the stock trades: `0.9999` is
always permissible and `1.001` never is.

:::note
The 2024 amendments to Rule 612 introduce a `$0.005` increment for tick-constrained stocks, making
the increment symbol-dependent. Compliance has been deferred to **2027-11-01**, so every symbol is
on the penny increment until then.
:::

## Execution client behaviour

### Amendments create new orders

Alpaca implements an amendment as a replacement: `PATCH` answers with a **new** order under a new
identifier and moves the original to `replaced`. Nautilus expects an order to keep its venue
identifier across a modification.

The client holds a chain from the identifier the engine knows to the one currently live, and
resolves through it before every cancel, query, and amendment. Repeated amendments collapse the
chain rather than nesting it.

The venue also refuses amendments in some states — an order received but not yet routed, which is
what happens outside market hours, cannot be replaced. That refusal is reported as a modify
rejection, since the order is still working.

### Trading event stream

Order events and fills arrive on the account WebSocket at `/stream`. Its connection limit is
separate from the market data stream's.

Fill reports record **zero commission** and an **unspecified liquidity side**, because the venue
reports neither. A fill without an execution identifier is refused: after a reconnect the same
execution is redelivered, and without that identifier a repeat cannot be told from a new fill.

### Fill recovery

The event stream reports fills as they happen but cannot be replayed, and reconciliation runs
before it has delivered anything. `generate_fill_reports` therefore reads
`GET /v2/account/activities?activity_types=FILL`, which returns one record per execution, newest
first.

Each record's `qty` is that execution's own quantity while `cum_qty` runs to the order total, so a
partially filled order appears as several records. A fill that opens a short position is reported
with side `sell_short`, which maps to a Nautilus sell — direction lives in the position there, not
in the side. Submissions never carry it: an equity order is sent as `sell` and the venue decides
from the position held. The venue caps a page at 100 and sends no
`next_page_token`: the cursor is the last record's `id`, and a page shorter than requested ends
the walk. The command's time window is passed through as `after` and `until`; its instrument and
order filters are applied locally, because the endpoint accepts neither.

Activity identifiers are 55 characters against the 36 a `TradeId` holds, and take the form
`<sequence>::<uuid>`. The half after the separator is used: in a sample of live activities it was
always a 36-character UUID, unique per fill, and the same shape the event stream reports as
`execution_id`. An identifier with no part short enough is refused rather than truncated, since
truncation could collide with another execution.

That UUID **is** the value the stream reports, confirmed on 2026-08-20 by observing one execution
on both paths: the stream gave `c27c5376-c4eb-414c-ba5a-4fb21d0b4c87` and the activity feed
`20260820050934216::c27c5376-c4eb-414c-ba5a-4fb21d0b4c87`. The engine de-duplicates fills by trade
identifier, so a fill recovered at startup and the same fill seen live resolve to a single trade.

Only a **fill** event carries that identifier. Lifecycle events have an `execution_id` too — `new`
has one, unrelated to any execution — so `parse_fill_report` gates on `has_fill_detail` before
reading it. The first version of the check below did not, compared an acknowledgement's identifier
against an execution's, and reported a mismatch that was its own.

To re-run it:

```bash
cargo run -p nautilus-alpaca --example execution_id_check -- --run
```

It buys one share at market in the paper account, reads the fill from the stream and again from
the activity feed, and compares. It exits 0 on a match, 1 on a mismatch, and **2 when it could not
get a fill** — a closed market reports inconclusive rather than passing, since a check that goes
green without testing anything is worse than no check.

### Order reports

`generate_order_status_reports` follows the command's `open_only` flag: set, it asks for
`status=open`; clear, it asks for `status=all`, because the engine then expects recently finished
orders in the answer and will otherwise query each of them one at a time to find out what
happened. The command's instrument and time window are passed through as `symbols`, `after`, and
`until`.

The two paginated endpoints work differently, and neither reports that more data exists:

| | Page size | Cursor | Oversized request |
| --- | --- | --- | --- |
| `/v2/account/activities` | 100 | the last record's `id` | rejected, code 40010001 |
| `/v2/orders` | 500 | `until`, set to the last order's `submitted_at`, exclusive | silently reduced to 500 |

The orders cursor is a timestamp, so two orders submitted in the same microsecond would straddle a
page boundary and the older of the pair would be missed. The venue timestamps to microseconds and
none of the 500 orders this was checked against shared one, so it is unlikely rather than
impossible; bound the request with a window rather than paging if that matters. Both walks stop at
a page limit and log when they do, since a truncated history that said nothing would read as a
complete one.

### Known limitations

- `cancel_all_orders` is account-wide at the venue; the instrument scope on the command cannot be honoured.
- Bracket and OCO order classes are not implemented.
- Extended-hours execution is opt-in through `default_extended_hours`.

## Configuration

```rust
use nautilus_alpaca::{
    common::enums::{AlpacaDataFeed, AlpacaEnvironment},
    config::{AlpacaDataClientConfig, AlpacaExecClientConfig},
};

let data_config = AlpacaDataClientConfig {
    environment: AlpacaEnvironment::Paper,
    feed: AlpacaDataFeed::Sip,
    poll_interval_secs: 15,
    poll_window_mins: 5,
    ..AlpacaDataClientConfig::default()
};

let exec_config = AlpacaExecClientConfig {
    environment: AlpacaEnvironment::Paper,
    default_extended_hours: false,
    ..AlpacaExecClientConfig::default()
};
```

## Routing through a proxy

The three base URL overrides point the clients somewhere other than the venue, which is how the
official SDK can be placed in front without changing adapter code:

```rust
let data_config = AlpacaDataClientConfig {
    base_url_rest: Some("http://127.0.0.1:8765".to_string()),
    base_url_trading: Some("http://127.0.0.1:8765".to_string()),
    base_url_ws: Some("ws://127.0.0.1:8765/stream".to_string()),
    ..AlpacaDataClientConfig::default()
};
```

Clearing them sends the clients straight back to the venue.

`poll_window_mins` sets how much history each poll requests. Widening it costs nothing extra per
request and increases tolerance to missed polls.

## Rate limiting

The Trading and Market Data hosts carry independent quotas, defaulting to 200 requests per minute
each. Higher market data plans raise the data limit substantially.

Retries are gated to `GET`. The venue's `DELETE` endpoints on positions submit closing orders, so
replaying them could trade twice.

## Diagnostic examples

```bash
cargo run -p nautilus-alpaca --example rest_smoke
cargo run -p nautilus-alpaca --example exec_smoke -- --run
cargo run -p nautilus-alpaca --example stream_capture
```

`exec_smoke` and `stream_capture` submit and cancel a resting order on the **paper** environment
and cancel what they create even when a step fails.
