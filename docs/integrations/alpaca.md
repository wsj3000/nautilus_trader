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

### Known limitations

- `cancel_all_orders` is account-wide at the venue; the instrument scope on the command cannot be honoured.
- `generate_fill_reports` returns an error. Fills are published from the event stream as they occur and cannot be requested retrospectively.
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
