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

//! Python bindings from `pyo3`.

use nautilus_common::factories::{ClientConfig, DataClientFactory, ExecutionClientFactory};
use nautilus_core::python::{to_pyruntime_err, to_pyvalue_err};
use nautilus_system::get_global_pyo3_registry;
use pyo3::prelude::*;

use crate::{
    common::{
        consts::{ALPACA, ALPACA_CLIENT_ID, ALPACA_VENUE},
        enums::{AlpacaDataFeed, AlpacaEnvironment},
    },
    config::{AlpacaDataClientConfig, AlpacaExecClientConfig},
    factories::{AlpacaDataClientFactory, AlpacaExecutionClientFactory},
};

#[expect(clippy::needless_pass_by_value)]
fn extract_alpaca_data_factory(
    py: Python<'_>,
    factory: Py<PyAny>,
) -> PyResult<Box<dyn DataClientFactory>> {
    match factory.extract::<AlpacaDataClientFactory>(py) {
        Ok(f) => Ok(Box::new(f)),
        Err(e) => Err(to_pyvalue_err(format!(
            "Failed to extract AlpacaDataClientFactory: {e}"
        ))),
    }
}

#[expect(clippy::needless_pass_by_value)]
fn extract_alpaca_exec_factory(
    py: Python<'_>,
    factory: Py<PyAny>,
) -> PyResult<Box<dyn ExecutionClientFactory>> {
    match factory.extract::<AlpacaExecutionClientFactory>(py) {
        Ok(f) => Ok(Box::new(f)),
        Err(e) => Err(to_pyvalue_err(format!(
            "Failed to extract AlpacaExecutionClientFactory: {e}"
        ))),
    }
}

#[expect(clippy::needless_pass_by_value)]
fn extract_alpaca_data_config(
    py: Python<'_>,
    config: Py<PyAny>,
) -> PyResult<Box<dyn ClientConfig>> {
    match config.extract::<AlpacaDataClientConfig>(py) {
        Ok(c) => Ok(Box::new(c)),
        Err(e) => Err(to_pyvalue_err(format!(
            "Failed to extract AlpacaDataClientConfig: {e}"
        ))),
    }
}

#[expect(clippy::needless_pass_by_value)]
fn extract_alpaca_exec_config(
    py: Python<'_>,
    config: Py<PyAny>,
) -> PyResult<Box<dyn ClientConfig>> {
    match config.extract::<AlpacaExecClientConfig>(py) {
        Ok(c) => Ok(Box::new(c)),
        Err(e) => Err(to_pyvalue_err(format!(
            "Failed to extract AlpacaExecClientConfig: {e}"
        ))),
    }
}

/// Exposed through `nautilus_trader.adapters.alpaca`.
///
/// # Errors
///
/// Returns an error if any bindings fail to register with the Python module.
#[pymodule]
pub fn alpaca(_: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add(stringify!(ALPACA), ALPACA)?;
    m.add(stringify!(ALPACA_CLIENT_ID), *ALPACA_CLIENT_ID)?;
    m.add(stringify!(ALPACA_VENUE), *ALPACA_VENUE)?;
    m.add_class::<AlpacaEnvironment>()?;
    m.add_class::<AlpacaDataFeed>()?;
    m.add_class::<AlpacaDataClientConfig>()?;
    m.add_class::<AlpacaExecClientConfig>()?;
    m.add_class::<AlpacaDataClientFactory>()?;
    m.add_class::<AlpacaExecutionClientFactory>()?;

    let registry = get_global_pyo3_registry();

    if let Err(e) =
        registry.register_factory_extractor(ALPACA.to_string(), extract_alpaca_data_factory)
    {
        return Err(to_pyruntime_err(format!(
            "Failed to register Alpaca data factory extractor: {e}"
        )));
    }

    if let Err(e) =
        registry.register_exec_factory_extractor(ALPACA.to_string(), extract_alpaca_exec_factory)
    {
        return Err(to_pyruntime_err(format!(
            "Failed to register Alpaca exec factory extractor: {e}"
        )));
    }

    if let Err(e) = registry.register_config_extractor(
        "AlpacaDataClientConfig".to_string(),
        extract_alpaca_data_config,
    ) {
        return Err(to_pyruntime_err(format!(
            "Failed to register Alpaca data config extractor: {e}"
        )));
    }

    if let Err(e) = registry.register_config_extractor(
        "AlpacaExecClientConfig".to_string(),
        extract_alpaca_exec_config,
    ) {
        return Err(to_pyruntime_err(format!(
            "Failed to register Alpaca exec config extractor: {e}"
        )));
    }

    Ok(())
}
