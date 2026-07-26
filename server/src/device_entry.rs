use std::collections::VecDeque;

use serde::Serialize;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::util::now_ms;

const RECENT_MAX: usize = 200;
/// Bounds the per-device node-transition journal. Kept separate from `recent`,
/// which a live bus trace rotates through in a few seconds.
pub const NODE_EVENTS_MAX: usize = 64;
/// Sensor nodes on a board; fixes the width of the per-node status arrays.
const NODE_COUNT: usize = 4;

/// Per-device state accumulated from the device WebSocket. Envelopes are stored
/// verbatim so the client `init`/`event` fan-out matches what the device sent.
#[derive(Default)]
pub struct DeviceEntry {
    pub connected: bool,
    pub hello: Option<Value>,
    pub snapshot: Option<Value>,
    pub device_status: Option<Value>,
    pub node_status: [Option<Value>; NODE_COUNT],
    pub recent: VecDeque<Value>,
    pub node_events: VecDeque<NodeEvent>,
    pub boot_id: Option<String>,
    pub last_seq: Option<u32>,
    pub cmd_tx: Option<mpsc::UnboundedSender<String>>,
    /// Generation of the connection that currently owns this entry. Guards
    /// against a stale reconnect mutating or tearing down the live connection.
    pub session: u64,
    pub last_active_ms: u64,
    pub last_snapshot_req_ms: u64,
}

/// One node transition worth a post-mortem, stamped with server wall clock:
/// every other record here is latest-value-only and carries device millis.
#[derive(Clone, Copy, Serialize)]
pub struct NodeEvent {
    pub unix_ms: u64,
    pub node: u8,
    pub online: bool,
    pub reset_cause: u8,
    pub timeouts: u32,
    pub event_overflow: u32,
}

/// Wire shape for `init.devices[]` and `GET /api/state`.
#[derive(Serialize)]
pub struct DeviceView {
    pub device_id: String,
    pub connected: bool,
    pub hello: Option<Value>,
    pub snapshot: Option<Value>,
    pub node_status: [Option<Value>; NODE_COUNT],
    pub device_status: Option<Value>,
    pub recent: Vec<Value>,
    pub node_events: Vec<NodeEvent>,
}

impl DeviceEntry {
    pub fn push_recent(&mut self, event: Value) {
        self.recent.push_back(event);
        while self.recent.len() > RECENT_MAX {
            self.recent.pop_front();
        }
    }

    /// Journals the transition a `node.status` envelope carries and stores it as
    /// the node's latest state. Nodes outside the board's range are ignored.
    pub fn record_node_status(&mut self, event: &Value) {
        let Some(node) = event
            .get("data")
            .and_then(|d| d.get("node"))
            .and_then(Value::as_u64)
        else {
            return;
        };
        let idx = node as usize;
        if idx >= NODE_COUNT {
            return;
        }

        let cur = node_marks(event);
        let prev = self.node_status[idx].as_ref().map(node_marks);
        // A steady heartbeat repeats the same marks; only journal
        // the edges, so 64 slots cover hours rather than minutes.
        let changed = match prev {
            Some(p) => (p.0, p.1, p.3) != (cur.0, cur.1, cur.3),
            None => true,
        };
        if changed {
            self.node_events.push_back(NodeEvent {
                unix_ms: now_ms(),
                node: node as u8,
                online: cur.0,
                reset_cause: cur.1,
                timeouts: cur.2,
                event_overflow: cur.3,
            });
            while self.node_events.len() > NODE_EVENTS_MAX {
                self.node_events.pop_front();
            }
        }
        self.node_status[idx] = Some(event.clone());
    }

    pub fn view(&self, device_id: &str) -> DeviceView {
        DeviceView {
            device_id: device_id.to_string(),
            connected: self.connected,
            hello: self.hello.clone(),
            snapshot: self.snapshot.clone(),
            node_status: self.node_status.clone(),
            device_status: self.device_status.clone(),
            recent: self.recent.iter().cloned().collect(),
            node_events: self.node_events.iter().copied().collect(),
        }
    }
}

/// `(online, reset_cause, timeouts, node_event_overflow)` from a `node.status`
/// envelope; missing fields read as zero so older firmware still journals.
fn node_marks(event: &Value) -> (bool, u8, u32, u32) {
    let data = event.get("data");
    let num = |key: &str| {
        data.and_then(|d| d.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    let online = data
        .and_then(|d| d.get("online"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    (
        online,
        num("reset_cause") as u8,
        num("timeouts") as u32,
        num("node_event_overflow") as u32,
    )
}
