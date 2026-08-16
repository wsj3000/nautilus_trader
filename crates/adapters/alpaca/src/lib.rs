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

//! [NautilusTrader](https://nautilustrader.io) adapter for [Alpaca](https://alpaca.markets).
//!
//! The `nautilus-alpaca` crate provides integration with the Alpaca Trading API and Market Data
//! API for US equities.
//!
//! Market data is sourced from the consolidated SIP feed and from the Blue Ocean ATS (BOATS)
//! overnight session feed. Fractional shares and notional orders are not supported; all
//! quantities are whole shares.
//!
//! # NautilusTrader
//!
//! [NautilusTrader](https://nautilustrader.io) is an open-source, production-grade, Rust-native
//! engine for multi-asset, multi-venue trading systems.
//!
//! The system spans research, deterministic simulation, and live execution within a single
//! event-driven architecture, providing research-to-live semantic parity.
//!
//! # Feature Flags
//!
//! This crate is Rust-only and exposes no Python bindings; the adapter is driven from Rust.
//!
//! [High-precision mode](https://nautilustrader.io/docs/nightly/getting_started/installation#precision-mode) (128-bit value types) is enabled by default.

#![warn(rustc::all)]
#![deny(unsafe_code)]
#![deny(nonstandard_style)]
#![deny(missing_debug_implementations)]
#![deny(clippy::missing_panics_doc)]
#![deny(rustdoc::broken_intra_doc_links)]

pub mod common;
pub mod config;
pub mod data;
pub mod execution;
pub mod factories;
pub mod http;
pub mod provider;

pub mod websocket;

pub use crate::{
    config::{AlpacaDataClientConfig, AlpacaExecClientConfig},
    data::AlpacaDataClient,
    execution::AlpacaExecutionClient,
    factories::{AlpacaDataClientFactory, AlpacaExecutionClientFactory},
    http::client::AlpacaRawHttpClient,
    provider::AlpacaInstrumentProvider,
};
