use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{header::AUTHORIZATION, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::stream::SplitStream;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::state::{random_hex, AppState};

const MAX_MSG_BYTES: usize = 2048;
const HEARTBEAT_MS: u64 = 15_000;

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

    let hello = match read_hello(&state, &mut receiver).await {
        Some(hello) => hello,
        None => return,
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

    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<String>();
    let session = match state.register_device(&device_id, hello, cmd_tx.clone()) {
        Some(session) => session,
        None => {
            tracing::warn!(device_id = %device_id, "device cap reached; refusing registration");
            return;
        }
    };
    state.broadcast_msg(json!({ "type": "device.connected", "device_id": device_id }).to_string());

    // Writer task owns the socket sink and drains queued commands to the device.
    let writer = tokio::spawn(async move {
        while let Some(text) = cmd_rx.recv().await {
            if sender.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    let mut warned_stale = false;
    while let Some(msg) = receiver.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(_) => break,
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
    state.broadcast_msg(
        json!({ "type": "event", "device_id": device_id, "event": event }).to_string(),
    );

    if need_snapshot {
        let n = state.next_seq();
        let cmd = json!({
            "v": 1,
            "type": "command",
            "server_seq": n,
            "id": format!("cmd-{n}"),
            "name": "board.snapshot.get",
            "args": {},
        });
        let _ = cmd_tx.send(cmd.to_string());
        tracing::info!(
            device_id = %device_id,
            boot_id = ?boot_id,
            seq = ?seq,
            "seq gap or boot change; requested board.snapshot.get"
        );
    }
}
