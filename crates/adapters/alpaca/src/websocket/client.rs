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

//! WebSocket client for the Alpaca trading event stream.
//!
//! This is the account stream on the trading host. Its connection limit is separate from the
//! market data stream's, so using it does not compete with a market data consumer elsewhere.
//!
//! The handshake is two round trips — authenticate, then subscribe — and both are confirmed before
//! the stream is handed to the caller. Returning early would let order events be missed in the gap
//! between connecting and being subscribed.

use futures_util::StreamExt;
use nautilus_network::{
    Message,
    websocket::{MessageReader, TransportBackend, WebSocketClient, WebSocketConfig},
};

use crate::{
    common::{
        consts::{
            RECONNECT_BACKOFF_FACTOR, RECONNECT_BASE_BACKOFF, RECONNECT_JITTER_MS,
            RECONNECT_MAX_BACKOFF, RECONNECT_TIMEOUT, WS_HEARTBEAT_SECS,
        },
        credential::AlpacaCredential,
        enums::AlpacaEnvironment,
        urls,
    },
    websocket::messages::{
        AuthMessage, AuthorizationData, ListenMessage, STREAM_AUTHORIZATION, STREAM_LISTENING,
        STREAM_TRADE_UPDATES, StreamEnvelope, StreamList, TradeUpdate,
    },
};

/// How many frames the handshake will read before giving up on a confirmation.
///
/// Bounded so a stream that answers with anything other than the expected confirmation fails
/// rather than blocking the caller indefinitely.
const HANDSHAKE_FRAME_LIMIT: usize = 16;

/// A connected and subscribed trading event stream.
pub struct AlpacaTradingStream {
    /// The frame reader, positioned after the handshake.
    pub reader: MessageReader,
    /// The client handle, kept so the connection can be closed.
    pub client: WebSocketClient,
}

/// Written by hand because the frame reader wraps a boxed transport that is not `Debug`.
impl std::fmt::Debug for AlpacaTradingStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(AlpacaTradingStream))
            .field("is_active", &self.client.is_active())
            .finish_non_exhaustive()
    }
}

impl AlpacaTradingStream {
    /// Reads the next trade update, skipping frames from other streams.
    ///
    /// Returns `None` when the connection closes.
    pub async fn next_update(&mut self) -> Option<anyhow::Result<TradeUpdate>> {
        while let Some(frame) = self.reader.next().await {
            let text = match frame {
                Ok(Message::Text(bytes) | Message::Binary(bytes)) => {
                    match String::from_utf8(bytes.to_vec()) {
                        Ok(text) => text,
                        Err(e) => return Some(Err(anyhow::anyhow!("Non-UTF8 frame: {e}"))),
                    }
                }
                // Control frames are handled by the transport; anything else is not a payload.
                Ok(_) => continue,
                Err(e) => return Some(Err(anyhow::anyhow!("Trading stream error: {e}"))),
            };

            match parse_trade_update(&text) {
                Ok(Some(update)) => return Some(Ok(update)),
                // Frames from the other streams are not payloads for this reader.
                Ok(None) => {}
                Err(e) => return Some(Err(e)),
            }
        }
        None
    }
}

/// Parses a frame, returning `None` for frames that are not trade updates.
///
/// # Errors
///
/// Returns an error if a trade update frame cannot be decoded.
pub fn parse_trade_update(text: &str) -> anyhow::Result<Option<TradeUpdate>> {
    let envelope: StreamEnvelope = match serde_json::from_str(text) {
        Ok(envelope) => envelope,
        Err(e) => {
            // An unrecognised frame shape is not fatal: the venue may add streams.
            log::debug!("Ignoring unrecognised trading stream frame: {e}");
            return Ok(None);
        }
    };

    if envelope.stream != STREAM_TRADE_UPDATES {
        return Ok(None);
    }

    serde_json::from_value(envelope.data)
        .map(Some)
        .map_err(|e| anyhow::anyhow!("Failed to decode trade update: {e}"))
}

/// Connects to the trading event stream and completes the handshake.
///
/// # Errors
///
/// Returns an error if the connection fails, authentication is refused, or the subscription is not
/// confirmed.
pub async fn connect_trading_stream(
    environment: AlpacaEnvironment,
    credential: &AlpacaCredential,
    url_override: Option<String>,
) -> anyhow::Result<AlpacaTradingStream> {
    let url = url_override.unwrap_or_else(|| urls::trading_ws_url(environment).to_string());

    let config = WebSocketConfig {
        url: url.clone(),
        headers: vec![],
        // The venue does not define an application-level heartbeat on this stream, so liveness
        // rests on transport control frames.
        heartbeat: Some(WS_HEARTBEAT_SECS),
        heartbeat_msg: None,
        reconnect_timeout_ms: Some(RECONNECT_TIMEOUT.as_millis() as u64),
        reconnect_delay_initial_ms: Some(RECONNECT_BASE_BACKOFF.as_millis() as u64),
        reconnect_delay_max_ms: Some(RECONNECT_MAX_BACKOFF.as_millis() as u64),
        reconnect_backoff_factor: Some(RECONNECT_BACKOFF_FACTOR),
        reconnect_jitter_ms: Some(RECONNECT_JITTER_MS),
        reconnect_max_attempts: None,
        idle_timeout_ms: None,
        backend: TransportBackend::default(),
        proxy_url: None,
    };

    let (mut reader, client) = WebSocketClient::connect_stream(config, vec![], None)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to the Alpaca trading stream: {e}"))?;

    let auth = serde_json::to_string(&AuthMessage::new(
        credential.api_key(),
        credential.api_secret(),
    ))?;
    client
        .send_text(auth, None)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to send the authentication frame: {e}"))?;

    await_authorization(&mut reader).await?;

    let listen = serde_json::to_string(&ListenMessage::trade_updates())?;
    client
        .send_text(listen, None)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to send the subscription frame: {e}"))?;

    await_listening(&mut reader).await?;

    log::info!("Connected to the Alpaca trading event stream at {url}");
    Ok(AlpacaTradingStream { reader, client })
}

/// Reads the next text payload from the stream.
async fn next_text(reader: &mut MessageReader) -> anyhow::Result<Option<String>> {
    while let Some(frame) = reader.next().await {
        match frame {
            Ok(Message::Text(bytes) | Message::Binary(bytes)) => {
                return String::from_utf8(bytes.to_vec())
                    .map(Some)
                    .map_err(|e| anyhow::anyhow!("Non-UTF8 frame: {e}"));
            }
            Ok(Message::Close(frame)) => {
                anyhow::bail!("Trading stream closed during the handshake: {frame:?}")
            }
            // Control frames carry no handshake payload.
            Ok(_) => {}
            Err(e) => anyhow::bail!("Trading stream error during the handshake: {e}"),
        }
    }
    Ok(None)
}

/// Waits for the authorization confirmation.
async fn await_authorization(reader: &mut MessageReader) -> anyhow::Result<()> {
    for _ in 0..HANDSHAKE_FRAME_LIMIT {
        let Some(text) = next_text(reader).await? else {
            anyhow::bail!("Trading stream closed before authorizing");
        };

        let Ok(envelope) = serde_json::from_str::<StreamEnvelope>(&text) else {
            continue;
        };
        if envelope.stream != STREAM_AUTHORIZATION {
            continue;
        }

        let data: AuthorizationData = serde_json::from_value(envelope.data)
            .map_err(|e| anyhow::anyhow!("Failed to decode the authorization frame: {e}"))?;

        return if data.is_authorized() {
            Ok(())
        } else {
            // Reported without the credential values, which must not reach logs.
            Err(anyhow::anyhow!(
                "Alpaca refused the trading stream credentials (status '{}')",
                data.status
            ))
        };
    }

    anyhow::bail!("No authorization response within {HANDSHAKE_FRAME_LIMIT} frames")
}

/// Waits for the subscription confirmation.
async fn await_listening(reader: &mut MessageReader) -> anyhow::Result<()> {
    for _ in 0..HANDSHAKE_FRAME_LIMIT {
        let Some(text) = next_text(reader).await? else {
            anyhow::bail!("Trading stream closed before confirming the subscription");
        };

        let Ok(envelope) = serde_json::from_str::<StreamEnvelope>(&text) else {
            continue;
        };
        if envelope.stream != STREAM_LISTENING {
            continue;
        }

        let listed: StreamList = serde_json::from_value(envelope.data)
            .map_err(|e| anyhow::anyhow!("Failed to decode the listening frame: {e}"))?;

        return if listed.streams.iter().any(|s| s == STREAM_TRADE_UPDATES) {
            Ok(())
        } else {
            // Proceeding here would leave a connection that never delivers an order event.
            Err(anyhow::anyhow!(
                "Alpaca did not subscribe the trade update stream; got {:?}",
                listed.streams
            ))
        };
    }

    anyhow::bail!("No subscription response within {HANDSHAKE_FRAME_LIMIT} frames")
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::websocket::messages::AlpacaTradeEvent;

    #[rstest]
    fn test_trade_update_frame_is_parsed() {
        let frame = r#"{"stream":"trade_updates","data":{
            "event":"fill","price":"300.005","qty":"10",
            "execution_id":"e-1",
            "order":{"id":"o-1","client_order_id":"c-1","symbol":"AAPL","type":"limit",
                     "side":"buy","time_in_force":"day","status":"filled","qty":"10"}
        }}"#;
        let update = parse_trade_update(frame).unwrap().unwrap();
        assert_eq!(update.event_kind(), AlpacaTradeEvent::Fill);
        assert!(update.has_fill_detail());
    }

    #[rstest]
    #[case(r#"{"stream":"authorization","data":{"status":"authorized"}}"#)]
    #[case(r#"{"stream":"listening","data":{"streams":["trade_updates"]}}"#)]
    fn test_other_streams_are_skipped(#[case] frame: &str) {
        assert!(parse_trade_update(frame).unwrap().is_none());
    }

    #[rstest]
    fn test_unrecognised_frame_shape_is_skipped_not_fatal() {
        // The venue may add frames this adapter does not model; dropping the connection over one
        // would be worse than ignoring it.
        assert!(
            parse_trade_update(r#"{"unexpected":true}"#)
                .unwrap()
                .is_none()
        );
        assert!(parse_trade_update("not json").unwrap().is_none());
    }

    #[rstest]
    fn test_malformed_trade_update_payload_is_an_error() {
        // A frame that claims to be a trade update but cannot be decoded is a real problem: it
        // means an order event was lost.
        let frame = r#"{"stream":"trade_updates","data":{"event":"fill"}}"#;
        assert!(parse_trade_update(frame).is_err());
    }
}
