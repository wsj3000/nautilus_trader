# nautilus-alpaca

[![build](https://github.com/nautechsystems/nautilus_trader/actions/workflows/build.yml/badge.svg?branch=master)](https://github.com/nautechsystems/nautilus_trader/actions/workflows/build.yml)
[![Documentation](https://img.shields.io/docsrs/nautilus-alpaca)](https://docs.rs/nautilus-alpaca/latest/nautilus-alpaca/)
[![crates.io version](https://img.shields.io/crates/v/nautilus-alpaca.svg)](https://crates.io/crates/nautilus-alpaca)
![license](https://img.shields.io/github/license/nautechsystems/nautilus_trader?color=blue)
[![Discord](https://img.shields.io/badge/Discord-%235865F2.svg?logo=discord&logoColor=white)](https://discord.gg/NautilusTrader)

[NautilusTrader](https://nautilustrader.io) adapter for the [Alpaca](https://alpaca.markets) API.

The `nautilus-alpaca` crate provides client bindings (HTTP & WebSocket), data models,
and helper utilities that wrap the **Alpaca Trading API** and **Alpaca Market Data API**
for US equities.

## Scope

- **Products**: US equities only. Options and crypto are not covered.
- **Market data**: the consolidated SIP feed and the Blue Ocean ATS (BOATS) overnight
  session feed. The IEX and delayed-SIP feeds are not exposed.
- **Quantities**: whole shares only. Fractional shares and notional orders are not supported.

The Alpaca environment defaults to paper trading, so an unconfigured client cannot reach the
live trading endpoint.

## NautilusTrader

[NautilusTrader](https://nautilustrader.io) is an open-source, production-grade, Rust-native
engine for multi-asset, multi-venue trading systems.

The system spans research, deterministic simulation, and live execution within a single
event-driven architecture, providing research-to-live semantic parity.

## Feature flags

This crate provides feature flags to control source code inclusion during compilation:

- `python`: Enables Python bindings from [PyO3](https://pyo3.rs).
- `extension-module`: Builds as a Python extension module.

[High-precision mode](https://nautilustrader.io/docs/nightly/getting_started/installation#precision-mode) (128-bit value types) is enabled by default.

## Documentation

See [the docs](https://docs.rs/nautilus-alpaca) for more detailed usage.

## License

The source code for NautilusTrader is available on GitHub under the
[GNU Lesser General Public License v3.0](https://www.gnu.org/licenses/lgpl-3.0.en.html).
Contributions to the project are welcome and require the completion of a standard
[Contributor License Agreement (CLA)](https://github.com/nautechsystems/nautilus_trader/blob/develop/CLA.md).

---

NautilusTrader™ is developed and maintained by Nautech Systems, a technology
company specializing in the development of high-performance trading systems.
For more information, visit <https://nautilustrader.io>.

<img src="https://nautilustrader.io/nautilus-logo-white.png" alt="logo" width="400" height="auto"/>

<span style="font-size: 0.8em; color: #dedede;">© 2015-2026 Nautech Systems Pty Ltd. All rights reserved.</span>
