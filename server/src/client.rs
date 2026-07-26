use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};

use crate::game::GameInput;
use crate::state::{command_envelope, AppState, DeviceLookup};
use crate::util::now_ms;

/// Bounded per-client outbound queue. A client whose socket stalls fills this and
/// is then shed, so one slow viewer cannot grow server memory without bound.
const CLIENT_QUEUE_CAP: usize = 256;
/// Client messages are tiny (auth + command); the transport rejects anything
/// bigger so an anonymous viewer cannot make the server buffer large payloads.
const MAX_CLIENT_MSG_BYTES: usize = 4096;
/// Keepalive on the browser link. Without it a half-open socket never fails a
/// write, so the reader parks forever and the client task leaks.
const CLIENT_PING_MS: u64 = 15_000;
/// Failed `auth` attempts tolerated before the socket is closed.
const MAX_AUTH_FAILURES: u32 = 5;
/// Delay applied to every failed `auth` reply, capping the guess rate.
const AUTH_FAILURE_DELAY: Duration = Duration::from_millis(500);
/// How long a closing client's writer may keep flushing queued frames.
const WRITER_DRAIN: Duration = Duration::from_secs(2);
/// How long to wait for a queue slot to tell a shed client why it was shed.
const SHED_NOTICE_WAIT: Duration = Duration::from_millis(250);

pub async fn client_ws(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.max_message_size(MAX_CLIENT_MSG_BYTES)
        .max_frame_size(MAX_CLIENT_MSG_BYTES)
        .on_upgrade(move |socket| handle_client(socket, state))
}

async fn handle_client(socket: WebSocket, state: Arc<AppState>) {
    let (mut sink, mut stream) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<String>(CLIENT_QUEUE_CAP);

    // Writer task owns the socket sink, drains the bounded outbound queue and
    // pings, so a dead path fails a write instead of stalling silently.
    let writer = tokio::spawn(async move {
        // interval_at, not interval: the first tick of an `interval` fires
        // immediately and would ping before init is even flushed.
        let period = Duration::from_millis(CLIENT_PING_MS);
        let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        loop {
            tokio::select! {
                text = out_rx.recv() => {
                    let Some(text) = text else { break };
                    if sink.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                _ = ping.tick() => {
                    if sink.send(Message::Ping(Default::default())).await.is_err() {
                        break;
                    }
                    // Browsers answer pings in the transport and never surface
                    // them to onmessage, so a ping alone cannot prove liveness
                    // to the page. With no device attached the server has
                    // nothing else to say, and a silent link is exactly what
                    // this keepalive exists to distinguish from a dead one.
                    let beat = json!({ "type": "keepalive", "unix_ms": now_ms() });
                    if sink.send(Message::Text(beat.to_string().into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut events = state.broadcast.subscribe();
    // Queue init ahead of any relayed event so the client sees full state first.
    // The game snapshot rides along in `init`, so a browser refresh or a
    // projector hiccup mid-demo costs nothing and needs no round trip through
    // the game task.
    let init = json!({
        "type": "init",
        "devices": state.snapshot_views(),
        "server": state.server_view(),
        "game": state.game_view(),
    });
    // Without init the client renders an empty board forever; closing at least
    // makes it reconnect.
    if out_tx.try_send(init.to_string()).is_err() {
        tracing::error!("could not queue client init; closing socket");
        writer.abort();
        return;
    }
    state.request_snapshots();

    let mut is_admin = false;
    let mut auth_failures: u32 = 0;
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(msg) => match out_tx.try_send(msg) {
                    Ok(()) => {}
                    // A full queue means the writer is stalled behind a slow
                    // socket; shed the client instead of buffering unbounded.
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        tracing::warn!(
                            devices = state.device_count(),
                            "client outbound queue full; dropping slow client"
                        );
                        // The notice needs the queue that just refused an event,
                        // so wait briefly for a slot: merely-slow sockets drain
                        // one, dead ones time out and are shed just as fast.
                        let _ = tokio::time::timeout(
                            SHED_NOTICE_WAIT,
                            out_tx.send(
                                json!({ "type": "error", "reason": "shed_slow_client" }).to_string(),
                            ),
                        )
                        .await;
                        break;
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => break,
                },
                // A lagged subscription has silently missed events the client
                // cannot recover from; shed it so it reconnects for a fresh init.
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(dropped = n, "client fan-out lagged; dropping client");
                    // The queue is not full on this path, so the client can be
                    // told why it is about to see a close.
                    let _ = out_tx.try_send(
                        json!({ "type": "error", "reason": "shed_lagged", "dropped": n })
                            .to_string(),
                    );
                    break;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            inbound = stream.next() => {
                let Some(Ok(msg)) = inbound else { break };
                if !handle_inbound(&state, msg, &mut is_admin, &mut auth_failures, &out_tx).await {
                    break;
                }
            }
        }
    }

    // Give the writer a bounded moment to flush what is already queued (a shed
    // notice is worthless if the task dies first); dropping out_tx ends it.
    let abort = writer.abort_handle();
    drop(out_tx);
    if tokio::time::timeout(WRITER_DRAIN, writer).await.is_err() {
        abort.abort();
    }
}

/// Length-independent byte fold, so a wrong password cannot be narrowed down by
/// timing the reply. No new dependency for four lines of arithmetic.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = (a.len() ^ b.len()) as u64;
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= u64::from(x ^ y);
    }
    diff == 0
}

/// Dispatches one inbound client frame; `false` closes the socket.
async fn handle_inbound(
    state: &Arc<AppState>,
    msg: Message,
    is_admin: &mut bool,
    auth_failures: &mut u32,
    out_tx: &mpsc::Sender<String>,
) -> bool {
    let text = match msg {
        Message::Text(text) => text,
        Message::Close(_) => return false,
        _ => return true,
    };
    let Ok(val) = serde_json::from_str::<Value>(text.as_str()) else {
        return true;
    };
    match val.get("type").and_then(Value::as_str) {
        Some("auth") => return handle_auth(state, &val, is_admin, auth_failures, out_tx).await,
        Some("command") => handle_command(state, &val, *is_admin, out_tx),
        // Gated exactly like `command`: the game task applies the finer-grained
        // rule, since a couple of actions are player-facing under
        // GAME_OPEN_CONTROLS while the rest never are.
        Some("game") => state.send_game(GameInput::Client {
            action: val,
            is_admin: *is_admin,
            reply: out_tx.clone(),
        }),
        _ => {}
    }
    true
}

/// Applies one `auth` frame and replies with the result. Returns `false` once
/// the client has burned its attempt budget and the socket must close.
async fn handle_auth(
    state: &Arc<AppState>,
    val: &Value,
    is_admin: &mut bool,
    auth_failures: &mut u32,
    out_tx: &mpsc::Sender<String>,
) -> bool {
    let ok = constant_time_eq(
        val.get("password")
            .and_then(Value::as_str)
            .unwrap_or("")
            .as_bytes(),
        state.admin_password.as_bytes(),
    );
    *is_admin |= ok;
    if !ok {
        *auth_failures += 1;
        tracing::warn!(attempt = *auth_failures, "client admin auth failed");
        // Rate-limit the reply, then close: /ws is otherwise a free password
        // oracle.
        tokio::time::sleep(AUTH_FAILURE_DELAY).await;
    }
    let _ = out_tx.try_send(json!({ "type": "auth.result", "ok": ok }).to_string());
    if *auth_failures >= MAX_AUTH_FAILURES {
        tracing::warn!("client exceeded admin auth attempts; closing");
        return false;
    }
    true
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

    let (id, cmd) = command_envelope(state.next_seq(), name, args);
    let _ = tx.send(cmd.to_string());
    let _ = out_tx.try_send(
        json!({ "type": "command.queued", "id": id, "device_id": device_id }).to_string(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_matches_equality() {
        assert!(constant_time_eq(b"hunter2", b"hunter2"));
        assert!(constant_time_eq(b"", b""));
        assert!(!constant_time_eq(b"hunter2", b"hunter3"));
        assert!(!constant_time_eq(b"hunter", b"hunter2"));
        assert!(!constant_time_eq(b"", b"hunter2"));
    }
}
