use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::Json;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};

const RECENT_MAX: usize = 200;
const BROADCAST_CAP: usize = 1024;
/// Hard cap on distinct device_ids retained, so an unauthenticated `/board`
/// cannot leak entries without bound. Bring-up only ever has a handful.
const MAX_DEVICES: usize = 64;
/// Disconnected entries are swept once idle this long, reclaiming their retained
/// snapshot/recent while still giving a reconnecting device continuity.
const DEVICE_RETENTION_MS: u64 = 600_000;
/// Bounds the set backing unknown-`type` warning dedup.
const MAX_TRACKED_UNKNOWN_TYPES: usize = 64;
/// Minimum spacing between client-connect-triggered snapshot refreshes, so a
/// reconnect-looping browser cannot spam devices with `board.snapshot.get`.
const SNAPSHOT_REQUEST_DEBOUNCE_MS: u64 = 2_000;
/// Bounds the per-device node-transition journal. Kept separate from `recent`,
/// which a live bus trace rotates through in a few seconds.
const NODE_EVENTS_MAX: usize = 64;

/// Per-device state accumulated from the device WebSocket. Envelopes are stored
/// verbatim so the client `init`/`event` fan-out matches what the device sent.
#[derive(Default)]
pub struct DeviceEntry {
    pub connected: bool,
    pub hello: Option<Value>,
    pub snapshot: Option<Value>,
    pub device_status: Option<Value>,
    pub node_status: [Option<Value>; 4],
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
    pub node_status: [Option<Value>; 4],
    pub device_status: Option<Value>,
    pub recent: Vec<Value>,
    pub node_events: Vec<NodeEvent>,
}

pub enum DeviceLookup {
    Unknown,
    Offline,
    Online(mpsc::UnboundedSender<String>),
}

pub struct AppState {
    pub devices: Mutex<HashMap<String, DeviceEntry>>,
    pub broadcast: broadcast::Sender<String>,
    pub server_seq: AtomicU32,
    pub session_seq: AtomicU64,
    pub oversized_dropped: AtomicU64,
    pub admin_password: String,
    pub device_token: Option<String>,
    started: Instant,
    logged_unknown_types: Mutex<HashSet<String>>,
}

impl AppState {
    pub fn new(admin_password: String, device_token: Option<String>) -> Self {
        let (broadcast, _) = broadcast::channel(BROADCAST_CAP);
        Self {
            devices: Mutex::new(HashMap::new()),
            broadcast,
            server_seq: AtomicU32::new(1),
            session_seq: AtomicU64::new(1),
            oversized_dropped: AtomicU64::new(0),
            admin_password,
            device_token,
            started: Instant::now(),
            logged_unknown_types: Mutex::new(HashSet::new()),
        }
    }

    /// One counter feeds both `welcome.server_seq` and command `server_seq`/`id`.
    pub fn next_seq(&self) -> u32 {
        self.server_seq.fetch_add(1, Ordering::Relaxed)
    }

    pub fn broadcast_msg(&self, msg: String) {
        let _ = self.broadcast.send(msg);
    }

    pub fn snapshot_views(&self) -> Vec<DeviceView> {
        let devices = self.devices.lock().expect("devices lock");
        devices.iter().map(|(id, e)| view(id, e)).collect()
    }

    pub fn device_count(&self) -> usize {
        self.devices.lock().expect("devices lock").len()
    }

    /// Server-side counters for `init` and `GET /api/state`; without these the
    /// only record of a dropped oversized frame is a log nobody reads.
    pub fn server_view(&self) -> Value {
        json!({
            "oversized_dropped": self.oversized_dropped.load(Ordering::Relaxed),
            "device_count": self.device_count(),
            "uptime_ms": self.started.elapsed().as_millis() as u64,
        })
    }

    /// Registers a connection and returns its session generation, or `None`
    /// when the device cap is reached and no disconnected entry can be evicted.
    /// The caller passes that generation back to `ingest_event` and
    /// `mark_disconnected` so a stale connection cannot mutate or tear down a
    /// newer one for the same `device_id`.
    pub fn register_device(
        &self,
        device_id: &str,
        hello: Value,
        cmd_tx: mpsc::UnboundedSender<String>,
    ) -> Option<u64> {
        let session = self.session_seq.fetch_add(1, Ordering::Relaxed);
        let mut devices = self.devices.lock().expect("devices lock");
        if !devices.contains_key(device_id) && devices.len() >= MAX_DEVICES {
            evict_oldest_disconnected(&mut devices);
            if devices.len() >= MAX_DEVICES {
                return None;
            }
        }
        let entry = devices.entry(device_id.to_string()).or_default();
        entry.connected = true;
        entry.hello = Some(hello);
        entry.cmd_tx = Some(cmd_tx);
        entry.session = session;
        entry.last_active_ms = now_ms();
        // Reset seq tracking; the device sends a fresh snapshot after welcome.
        entry.boot_id = None;
        entry.last_seq = None;
        Some(session)
    }

    /// Clears the connection only if `session` still owns the entry. Returns
    /// whether it took effect, so the caller broadcasts `device.disconnected`
    /// only for the live connection, not a superseded reconnect.
    pub fn mark_disconnected(&self, device_id: &str, session: u64) -> bool {
        let mut devices = self.devices.lock().expect("devices lock");
        if let Some(e) = devices.get_mut(device_id) {
            if e.session == session {
                e.connected = false;
                e.cmd_tx = None;
                e.last_active_ms = now_ms();
                return true;
            }
        }
        false
    }

    /// Logs each distinct unrecognized event `type` once (bounded), satisfying
    /// the contract's "unknown `type` values must be logged and ignored".
    pub fn warn_unknown_type(&self, device_id: &str, etype: &str) {
        let mut seen = self.logged_unknown_types.lock().expect("unknown types lock");
        if seen.contains(etype) || seen.len() >= MAX_TRACKED_UNKNOWN_TYPES {
            return;
        }
        seen.insert(etype.to_string());
        drop(seen);
        tracing::warn!(
            device_id = %device_id,
            unknown_type = %etype,
            "ignoring device event with unrecognized type"
        );
        // Also surface it to browsers: firmware/server version skew is otherwise
        // invisible on the only console anyone actually watches.
        self.broadcast_msg(
            json!({
                "type": "error",
                "reason": "unknown_event_type",
                "etype": etype,
                "device_id": device_id,
            })
            .to_string(),
        );
    }

    /// Removes disconnected entries idle past the retention window.
    pub fn sweep_stale(&self) {
        let cutoff = now_ms().saturating_sub(DEVICE_RETENTION_MS);
        let mut devices = self.devices.lock().expect("devices lock");
        devices.retain(|_, e| e.connected || e.last_active_ms >= cutoff);
    }

    /// Asks every connected device for a fresh snapshot, debounced per device.
    /// Run on client connect: the `init` snapshot may predate events the recent
    /// ring has rotated past, a gap the client cannot recover from on its own.
    pub fn request_snapshots(&self) {
        let now = now_ms();
        let mut devices = self.devices.lock().expect("devices lock");
        for (id, e) in devices.iter_mut() {
            let Some(tx) = &e.cmd_tx else { continue };
            if now.saturating_sub(e.last_snapshot_req_ms) < SNAPSHOT_REQUEST_DEBOUNCE_MS {
                continue;
            }
            let n = self.next_seq();
            let cmd = json!({
                "v": 1,
                "type": "command",
                "server_seq": n,
                "id": format!("cmd-{n}"),
                "name": "board.snapshot.get",
                "args": {},
            });
            if tx.send(cmd.to_string()).is_ok() {
                e.last_snapshot_req_ms = now;
                tracing::debug!(device_id = %id, "requested snapshot for new client");
            }
        }
    }

    pub fn lookup_device(&self, device_id: &str) -> DeviceLookup {
        let devices = self.devices.lock().expect("devices lock");
        match devices.get(device_id) {
            None => DeviceLookup::Unknown,
            Some(e) => match &e.cmd_tx {
                Some(tx) => DeviceLookup::Online(tx.clone()),
                None => DeviceLookup::Offline,
            },
        }
    }

    /// Stores the event and returns `Some(true)` when a `(boot_id, seq)` gap or
    /// boot change means the caller should request a fresh `board.snapshot`.
    /// Returns `None` (dropping the event) when `session` no longer owns the
    /// entry, so a superseded connection cannot mutate the live session's state.
    pub fn ingest_event(
        &self,
        device_id: &str,
        session: u64,
        etype: &str,
        boot_id: Option<&str>,
        seq: Option<u32>,
        event: &Value,
    ) -> Option<bool> {
        let mut devices = self.devices.lock().expect("devices lock");
        let Some(entry) = devices.get_mut(device_id) else {
            return None;
        };
        if entry.session != session {
            return None;
        }
        entry.last_active_ms = now_ms();

        match etype {
            "board.snapshot" => entry.snapshot = Some(event.clone()),
            "device.status" => entry.device_status = Some(event.clone()),
            "node.status" => {
                let node = event
                    .get("data")
                    .and_then(|d| d.get("node"))
                    .and_then(Value::as_u64);
                if let Some(n) = node {
                    if (n as usize) < 4 {
                        let cur = node_marks(event);
                        let prev = entry.node_status[n as usize].as_ref().map(node_marks);
                        // A steady heartbeat repeats the same marks; only journal
                        // the edges, so 64 slots cover hours rather than minutes.
                        let changed = match prev {
                            Some(p) => (p.0, p.1, p.3) != (cur.0, cur.1, cur.3),
                            None => true,
                        };
                        if changed {
                            entry.node_events.push_back(NodeEvent {
                                unix_ms: now_ms(),
                                node: n as u8,
                                online: cur.0,
                                reset_cause: cur.1,
                                timeouts: cur.2,
                                event_overflow: cur.3,
                            });
                            while entry.node_events.len() > NODE_EVENTS_MAX {
                                entry.node_events.pop_front();
                            }
                        }
                        entry.node_status[n as usize] = Some(event.clone());
                    }
                }
            }
            _ => {}
        }

        entry.recent.push_back(event.clone());
        while entry.recent.len() > RECENT_MAX {
            entry.recent.pop_front();
        }

        let mut need_snapshot = false;
        if let (Some(b), Some(s)) = (boot_id, seq) {
            // A snapshot heals a gap rather than opening one: it already
            // supersedes everything missed, so don't request another.
            if etype != "board.snapshot" {
                match (entry.boot_id.as_deref(), entry.last_seq) {
                    (Some(prev_b), Some(prev_s)) if prev_b == b => {
                        if s != prev_s.wrapping_add(1) {
                            need_snapshot = true;
                        }
                    }
                    (Some(prev_b), _) if prev_b != b => need_snapshot = true,
                    _ => {}
                }
            }
            entry.boot_id = Some(b.to_string());
            entry.last_seq = Some(s);
        }
        Some(need_snapshot)
    }
}

/// Drops the least-recently-active disconnected entry to make room under the
/// device cap. Live connections are never evicted.
fn evict_oldest_disconnected(devices: &mut HashMap<String, DeviceEntry>) {
    let victim = devices
        .iter()
        .filter(|(_, e)| !e.connected)
        .min_by_key(|(_, e)| e.last_active_ms)
        .map(|(id, _)| id.clone());
    if let Some(id) = victim {
        devices.remove(&id);
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

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn view(id: &str, e: &DeviceEntry) -> DeviceView {
    DeviceView {
        device_id: id.to_string(),
        connected: e.connected,
        hello: e.hello.clone(),
        snapshot: e.snapshot.clone(),
        node_status: e.node_status.clone(),
        device_status: e.device_status.clone(),
        recent: e.recent.iter().cloned().collect(),
        node_events: e.node_events.iter().copied().collect(),
    }
}

/// Dependency-free pseudo-random hex id for `session_id`.
pub fn random_hex() -> String {
    static CTR: AtomicU64 = AtomicU64::new(0);
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let c = CTR.fetch_add(1, Ordering::Relaxed);
    let mix = t ^ c.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    format!("{mix:016x}")
}

pub async fn api_state(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({ "devices": state.snapshot_views(), "server": state.server_view() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node_status(online: bool, reset_cause: u64, overflow: u64) -> Value {
        json!({
            "type": "node.status",
            "data": {
                "node": 1,
                "online": online,
                "reset_cause": reset_cause,
                "timeouts": 7,
                "node_event_overflow": overflow,
            }
        })
    }

    #[test]
    fn node_journal_records_only_transitions() {
        let state = AppState::new("pw".to_string(), None);
        let (tx, _rx) = mpsc::unbounded_channel();
        let session = state.register_device("dev", json!({}), tx).expect("register");
        for event in [
            node_status(true, 0, 0),
            node_status(true, 0, 0),
            node_status(false, 0, 0),
            node_status(true, 3, 1),
        ] {
            state
                .ingest_event("dev", session, "node.status", None, None, &event)
                .expect("ingest");
        }

        let views = state.snapshot_views();
        let journal = &views[0].node_events;
        assert_eq!(journal.len(), 3);
        assert!(!journal[1].online);
        assert_eq!(journal[2].reset_cause, 3);
        assert_eq!(journal[2].event_overflow, 1);
        assert_eq!(journal[2].timeouts, 7);
    }

    #[test]
    fn node_journal_is_bounded() {
        let state = AppState::new("pw".to_string(), None);
        let (tx, _rx) = mpsc::unbounded_channel();
        let session = state.register_device("dev", json!({}), tx).expect("register");
        for i in 0..(NODE_EVENTS_MAX as u64 * 2) {
            let event = node_status(i % 2 == 0, 0, 0);
            state
                .ingest_event("dev", session, "node.status", None, None, &event)
                .expect("ingest");
        }

        assert_eq!(state.snapshot_views()[0].node_events.len(), NODE_EVENTS_MAX);
    }
}
