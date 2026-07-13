use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

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
    pub boot_id: Option<String>,
    pub last_seq: Option<u32>,
    pub cmd_tx: Option<mpsc::UnboundedSender<String>>,
    /// Generation of the connection that currently owns this entry. Guards
    /// against a stale reconnect mutating or tearing down the live connection.
    pub session: u64,
    pub last_active_ms: u64,
    pub last_snapshot_req_ms: u64,
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

fn now_ms() -> u64 {
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
    Json(json!({ "devices": state.snapshot_views() }))
}
