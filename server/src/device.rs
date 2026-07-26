use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{close_code, CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{header::AUTHORIZATION, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::state::{command_envelope, AppState};
use crate::util::{now_ms, random_hex};

// A full sensor.raw_scan carries four 64-entry arrays and can approach the old
// 2 KiB ceiling once envelope metadata and longer device IDs are included.
const MAX_MSG_BYTES: usize = 4096;
const HEARTBEAT_MS: u64 = 15_000;
/// Read deadline for an established device link. The device publishes
/// `device.status` every 15 s and pings on the same period, and its longest
/// main-loop stall is one ~400 ms flash page write, so three missed periods
/// means the socket is half-open (power cut, NAT eviction, AP gone) and must be
/// torn down — otherwise the entry stays `connected` and commands vanish.
const DEVICE_READ_TIMEOUT: Duration = Duration::from_secs(45);
/// A device that connects and never says `hello` holds a task and a socket.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// Device event `type` values defined by websocket-api.md. Anything else is
/// relayed but logged as unrecognized per the transport contract.
const KNOWN_EVENT_TYPES: &[&str] = &[
    "board.snapshot",
    "sensor.changed",
    "sensor.raw_scan",
    "node.status",
    "device.status",
    "diagnostic.log",
    "diagnostic.bus",
    "calibration.progress",
    "calibration.result",
    "command.result",
];

pub async fn board_ws(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Some(expected) = &state.device_token {
        let authorized = headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|token| token == expected);
        if !authorized {
            return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
        }
    }
    ws.max_message_size(MAX_MSG_BYTES)
        .max_frame_size(MAX_MSG_BYTES)
        .on_upgrade(move |socket| handle_board(socket, state))
}

async fn handle_board(socket: WebSocket, state: Arc<AppState>) {
    let (mut sender, mut receiver) = socket.split();

    let hello = match tokio::time::timeout(HELLO_TIMEOUT, read_hello(&state, &mut receiver)).await {
        Ok(Some(hello)) => hello,
        Ok(None) => return,
        Err(_) => {
            tracing::warn!("device sent no hello before deadline; closing");
            return;
        }
    };
    let device_id = match hello.get("device_id").and_then(Value::as_str) {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => {
            tracing::warn!("hello missing device_id; closing device connection");
            return;
        }
    };
    let boot_id = hello
        .get("boot_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let welcome = json!({
        "v": 1,
        "type": "welcome",
        "server_seq": state.next_seq(),
        "session_id": random_hex(),
        "heartbeat_ms": HEARTBEAT_MS,
        "snapshot_required": true,
    });
    if sender
        .send(Message::Text(welcome.to_string().into()))
        .await
        .is_err()
    {
        return;
    }
    tracing::info!(device_id = %device_id, boot_id = %boot_id, "device connected");

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<String>();
    let session = match state.register_device(&device_id, hello, cmd_tx.clone()) {
        Some(session) => session,
        None => {
            tracing::warn!(device_id = %device_id, "device cap reached; refusing registration");
            let _ = sender
                .send(Message::Close(Some(CloseFrame {
                    code: close_code::AGAIN,
                    reason: "device_cap".into(),
                })))
                .await;
            return;
        }
    };
    state.broadcast_msg(json!({ "type": "device.connected", "device_id": device_id }).to_string());

    let writer = spawn_writer(sender, cmd_rx);

    let mut warned_stale = false;
    loop {
        let msg = match tokio::time::timeout(DEVICE_READ_TIMEOUT, receiver.next()).await {
            Ok(Some(Ok(m))) => m,
            Ok(_) => break,
            Err(_) => {
                tracing::warn!(device_id = %device_id, "device read timed out; closing link");
                break;
            }
        };
        match msg {
            Message::Text(text) => {
                if text.as_str().len() > MAX_MSG_BYTES {
                    state.oversized_dropped.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(device_id = %device_id, "dropped oversized device message");
                    continue;
                }
                match serde_json::from_str::<Value>(text.as_str()) {
                    Ok(event) => {
                        handle_event(&state, &device_id, session, event, &cmd_tx, &mut warned_stale)
                    }
                    Err(_) => {
                        tracing::warn!(device_id = %device_id, "dropped non-JSON device message");
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    writer.abort();
    if state.mark_disconnected(&device_id, session) {
        state.broadcast_msg(
            json!({ "type": "device.disconnected", "device_id": device_id }).to_string(),
        );
        tracing::info!(device_id = %device_id, "device disconnected");
    } else {
        // A newer connection already owns this device_id; leave its state intact.
        tracing::info!(device_id = %device_id, "stale device connection closed; live session retained");
    }
}

/// Writer task owns the socket sink: it drains queued commands and sends the
/// heartbeat the welcome advertises, which also keeps NAT bindings alive.
fn spawn_writer(
    mut sender: SplitSink<WebSocket, Message>,
    mut cmd_rx: mpsc::UnboundedReceiver<String>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        // interval_at: `interval`'s first tick is immediate, which would ping the
        // device before it has finished processing the welcome.
        let period = Duration::from_millis(HEARTBEAT_MS);
        let mut beat = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        loop {
            tokio::select! {
                text = cmd_rx.recv() => {
                    let Some(text) = text else { break };
                    if sender.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                _ = beat.tick() => {
                    if sender.send(Message::Ping(Default::default())).await.is_err() {
                        break;
                    }
                }
            }
        }
    })
}

/// Blocks until the mandatory first `hello` frame arrives; `None` closes.
async fn read_hello(state: &Arc<AppState>, receiver: &mut SplitStream<WebSocket>) -> Option<Value> {
    loop {
        match receiver.next().await? {
            Ok(Message::Text(text)) => {
                if text.as_str().len() > MAX_MSG_BYTES {
                    state.oversized_dropped.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                match serde_json::from_str::<Value>(text.as_str()) {
                    Ok(v) if v.get("type").and_then(Value::as_str) == Some("hello") => {
                        if v.get("v").and_then(Value::as_i64) != Some(1) {
                            tracing::warn!("device hello has incompatible protocol version; closing");
                            return None;
                        }
                        return Some(v);
                    }
                    _ => {
                        tracing::warn!("device first message was not hello; closing");
                        return None;
                    }
                }
            }
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => continue,
            _ => return None,
        }
    }
}

fn handle_event(
    state: &Arc<AppState>,
    device_id: &str,
    session: u64,
    event: Value,
    cmd_tx: &mpsc::UnboundedSender<String>,
    warned_stale: &mut bool,
) {
    let etype = event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if !KNOWN_EVENT_TYPES.contains(&etype.as_str()) {
        state.warn_unknown_type(device_id, &etype);
    }
    let boot_id = event.get("boot_id").and_then(Value::as_str).map(str::to_string);
    let seq = event.get("seq").and_then(Value::as_u64).map(|s| s as u32);

    let Some(need_snapshot) =
        state.ingest_event(device_id, session, &etype, boot_id.as_deref(), seq, &event)
    else {
        if !*warned_stale {
            *warned_stale = true;
            tracing::warn!(device_id = %device_id, "dropping events from superseded device session");
        }
        return;
    };
    // Occupancy-relevant events also go to the game task. Only these four
    // types: a live bus trace would otherwise flood a channel that has no use
    // for it.
    if matches!(
        etype.as_str(),
        "board.snapshot" | "sensor.changed" | "node.status" | "command.result"
    ) {
        state.send_game(crate::game::GameInput::Device {
            device_id: device_id.to_string(),
            event: event.clone(),
        });
    }

    // Stamp arrival outside the envelope: the device only knows its own millis,
    // so nothing else can line an event up against a server log or a wall clock.
    state.broadcast_msg(
        json!({
            "type": "event",
            "device_id": device_id,
            "recv_unix_ms": now_ms(),
            "event": event,
        })
        .to_string(),
    );

    if need_snapshot {
        let (_, cmd) = command_envelope(state.next_seq(), "board.snapshot.get", json!({}));
        let _ = cmd_tx.send(cmd.to_string());
        tracing::info!(
            device_id = %device_id,
            boot_id = ?boot_id,
            seq = ?seq,
            "seq gap or boot change; requested board.snapshot.get"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_raw_scan_fits_device_message_limit() {
        let event = json!({
            "v": 1,
            "type": "sensor.raw_scan",
            "device_id": "arcade-chess-production-001",
            "boot_id": "ffffffff",
            "seq": u32::MAX,
            "at_ms": u32::MAX,
            "data": {
                "scan_id": u32::MAX,
                "complete": true,
                "captured_ms": u32::MAX,
                "target_node_mask": 15,
                "response_node_mask": 15,
                "online_node_mask": 15,
                "raw_adc": vec![1023; 64],
                "baseline_adc": vec![1023; 64],
                "noise_adc": vec![255; 64],
                "state": vec!["uncertain"; 64],
            }
        });

        assert!(event.to_string().len() <= MAX_MSG_BYTES);
    }
}
