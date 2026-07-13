use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};

use crate::state::{AppState, DeviceLookup};

/// Bounded per-client outbound queue. A client whose socket stalls fills this and
/// is then shed, so one slow viewer cannot grow server memory without bound.
const CLIENT_QUEUE_CAP: usize = 256;

pub async fn client_ws(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |socket| handle_client(socket, state))
}

async fn handle_client(socket: WebSocket, state: Arc<AppState>) {
    let (mut sink, mut stream) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<String>(CLIENT_QUEUE_CAP);

    // Writer task owns the socket sink and drains the bounded outbound queue.
    let writer = tokio::spawn(async move {
        while let Some(text) = out_rx.recv().await {
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    let mut events = state.broadcast.subscribe();
    // Queue init ahead of any relayed event so the client sees full state first.
    let init = json!({ "type": "init", "devices": state.snapshot_views() });
    let _ = out_tx.try_send(init.to_string());

    let mut is_admin = false;
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(msg) => match out_tx.try_send(msg) {
                    Ok(()) => {}
                    // A full queue means the writer is stalled behind a slow
                    // socket; shed the client instead of buffering unbounded.
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        tracing::warn!("client outbound queue full; dropping slow client");
                        break;
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => break,
                },
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(dropped = n, "client fan-out lagged");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            inbound = stream.next() => {
                let Some(Ok(msg)) = inbound else { break };
                match msg {
                    Message::Text(text) => {
                        let Ok(val) = serde_json::from_str::<Value>(text.as_str()) else {
                            continue;
                        };
                        match val.get("type").and_then(Value::as_str) {
                            Some("auth") => {
                                let ok = val.get("password").and_then(Value::as_str)
                                    == Some(state.admin_password.as_str());
                                is_admin |= ok;
                                let _ = out_tx
                                    .try_send(json!({ "type": "auth.result", "ok": ok }).to_string());
                            }
                            Some("command") => handle_command(&state, &val, is_admin, &out_tx),
                            _ => {}
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        }
    }

    writer.abort();
}

fn handle_command(
    state: &Arc<AppState>,
    val: &Value,
    is_admin: bool,
    out_tx: &mpsc::Sender<String>,
) {
    let reject = |reason: &str| {
        let _ = out_tx.try_send(json!({ "type": "error", "reason": reason }).to_string());
    };

    if !is_admin {
        return reject("unauthorized");
    }
    let Some(device_id) = val.get("device_id").and_then(Value::as_str) else {
        return reject("unknown_device");
    };
    let tx = match state.lookup_device(device_id) {
        DeviceLookup::Unknown => return reject("unknown_device"),
        DeviceLookup::Offline => return reject("device_offline"),
        DeviceLookup::Online(tx) => tx,
    };
    let name = match val.get("name").and_then(Value::as_str) {
        Some(n) if !n.is_empty() => n,
        _ => return reject("invalid_args"),
    };
    let args = match val.get("args") {
        Some(a) if a.is_object() => a.clone(),
        _ => return reject("invalid_args"),
    };

    let n = state.next_seq();
    let id = format!("cmd-{n}");
    let cmd = json!({
        "v": 1,
        "type": "command",
        "server_seq": n,
        "id": id,
        "name": name,
        "args": args,
    });
    let _ = tx.send(cmd.to_string());
    let _ = out_tx.try_send(
        json!({ "type": "command.queued", "id": id, "device_id": device_id }).to_string(),
    );
}
