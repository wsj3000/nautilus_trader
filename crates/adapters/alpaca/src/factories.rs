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

//! Factories for constructing Alpaca clients from configuration.

use std::{any::Any, cell::RefCell, rc::Rc};

use nautilus_common::{
    cache::CacheView,
    clients::{DataClient, ExecutionClient},
    clock::Clock,
    factories::{ClientConfig, DataClientFactory, ExecutionClientFactory},
};
use nautilus_live::ExecutionClientCore;
use nautilus_model::{
    enums::{AccountType, OmsType},
    identifiers::{AccountId, ClientId, TraderId},
};

use crate::{
    common::consts::{ALPACA, ALPACA_VENUE},
    config::{AlpacaDataClientConfig, AlpacaExecClientConfig},
    data::AlpacaDataClient,
    execution::AlpacaExecutionClient,
};

impl ClientConfig for AlpacaDataClientConfig {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl ClientConfig for AlpacaExecClientConfig {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Factory for creating Alpaca data clients.
#[derive(Debug, Clone)]
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(module = "nautilus_trader.adapters.alpaca", from_py_object)
)]
#[cfg_attr(
    feature = "python",
    pyo3_stub_gen::derive::gen_stub_pyclass(module = "nautilus_trader.adapters.alpaca")
)]
pub struct AlpacaDataClientFactory;

impl AlpacaDataClientFactory {
    /// Creates a new [`AlpacaDataClientFactory`] instance.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for AlpacaDataClientFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl DataClientFactory for AlpacaDataClientFactory {
    fn create(
        &self,
        name: &str,
        config: &dyn ClientConfig,
        _cache: CacheView,
        _clock: Rc<RefCell<dyn Clock>>,
    ) -> anyhow::Result<Box<dyn DataClient>> {
        let config = config
            .as_any()
            .downcast_ref::<AlpacaDataClientConfig>()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Invalid config type for AlpacaDataClientFactory. Expected AlpacaDataClientConfig, was {config:?}",
                )
            })?
            .clone();

        let client = AlpacaDataClient::new(ClientId::from(name), config)?;
        Ok(Box::new(client))
    }

    fn name(&self) -> &'static str {
        ALPACA
    }

    fn config_type(&self) -> &'static str {
        "AlpacaDataClientConfig"
    }
}

/// Factory for creating Alpaca execution clients.
///
/// The venue nets positions per symbol and offers no hedge mode, so the OMS type is always
/// `Netting`. Accounts are cash or margin depending on the account itself rather than on
/// configuration, and the venue reports the distinction, so `Cash` is used as the declared type
/// and margin detail comes from the account state.
#[derive(Debug, Clone)]
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(module = "nautilus_trader.adapters.alpaca", from_py_object)
)]
#[cfg_attr(
    feature = "python",
    pyo3_stub_gen::derive::gen_stub_pyclass(module = "nautilus_trader.adapters.alpaca")
)]
pub struct AlpacaExecutionClientFactory {
    trader_id: TraderId,
    account_id: AccountId,
}

impl AlpacaExecutionClientFactory {
    /// Creates a new [`AlpacaExecutionClientFactory`] instance.
    #[must_use]
    pub const fn new(trader_id: TraderId, account_id: AccountId) -> Self {
        Self {
            trader_id,
            account_id,
        }
    }
}

impl ExecutionClientFactory for AlpacaExecutionClientFactory {
    fn create(
        &self,
        name: &str,
        config: &dyn ClientConfig,
        cache: CacheView,
    ) -> anyhow::Result<Box<dyn ExecutionClient>> {
        let config = config
            .as_any()
            .downcast_ref::<AlpacaExecClientConfig>()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Invalid config type for AlpacaExecutionClientFactory. Expected AlpacaExecClientConfig, was {config:?}",
                )
            })?
            .clone();

        let core = ExecutionClientCore::new(
            self.trader_id,
            ClientId::from(name),
            *ALPACA_VENUE,
            OmsType::Netting,
            self.account_id,
            AccountType::Cash,
            None, // base_currency: reported by the venue on the account state
            cache,
        );

        let client = AlpacaExecutionClient::new(core, config)?;
        Ok(Box::new(client))
    }

    fn name(&self) -> &'static str {
        ALPACA
    }

    fn config_type(&self) -> &'static str {
        "AlpacaExecClientConfig"
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn exec_factory() -> AlpacaExecutionClientFactory {
        AlpacaExecutionClientFactory::new(
            TraderId::from("TRADER-001"),
            AccountId::new("ALPACA-001"),
        )
    }

    #[rstest]
    fn test_factories_report_the_venue_name() {
        assert_eq!(AlpacaDataClientFactory::new().name(), ALPACA);
        assert_eq!(exec_factory().name(), ALPACA);
    }

    #[rstest]
    fn test_factories_declare_their_config_types() {
        // These strings are how the Python side resolves a config to its factory; a mismatch
        // would surface only at node startup.
        assert_eq!(
            AlpacaDataClientFactory::new().config_type(),
            "AlpacaDataClientConfig"
        );
        assert_eq!(exec_factory().config_type(), "AlpacaExecClientConfig");
    }

    #[rstest]
    fn test_config_type_names_match_the_actual_types() {
        // Guards against the declared name drifting from the struct it names.
        assert_eq!(
            AlpacaDataClientFactory::new().config_type(),
            std::any::type_name::<AlpacaDataClientConfig>()
                .rsplit("::")
                .next()
                .unwrap()
        );
        assert_eq!(
            exec_factory().config_type(),
            std::any::type_name::<AlpacaExecClientConfig>()
                .rsplit("::")
                .next()
                .unwrap()
        );
    }
}
