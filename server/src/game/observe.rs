//! Occupancy tracking from the relayed device event stream.
//!
//! This layer answers one question — "which squares hold a piece right now, and
//! which squares am I allowed to believe?" — and deliberately answers nothing
//! else. Piece identity lives in the tracked `shakmaty` position; the sensors
//! can only ever report presence and a magnet's polarity.
//!
//! Two firmware truths shape everything here (see `docs/websocket-api.md` and
//! `firmware-esp/src/network_manager.cpp:publishSnapshot`):
//!
//! 1. A `board.snapshot` maps `uncertain` to the same `0` as `empty`, and sets
//!    `valid = online && state != uncertain`. So `valid == false` means *either*
//!    the quadrant is offline *or* a piece is being lifted off that square right
//!    now. Reading it as "offline, assume it matches" fires auto-start while a
//!    player is holding a piece and loses exactly the evidence capture detection
//!    needs. `online_node_mask` is what discriminates the two.
//! 2. `sensor.changed` carries the real state string, including `uncertain`, and
//!    needs no inference.

use serde_json::Value;

pub const SQUARES: usize = 64;
pub const NODES: usize = 4;

/// Settled occupancy of one square. `uncertain` is *not* a value here: a
/// wobbling square keeps its last occupancy and is flagged unstable instead, so
/// a piece hovering over the board can never erase the game's own record of it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Occ {
    #[default]
    Empty,
    Pos,
    Neg,
}

impl Occ {
    pub fn occupied(self) -> bool {
        !matches!(self, Occ::Empty)
    }

    pub fn polarity(self) -> Option<Pol> {
        match self {
            Occ::Pos => Some(Pol::Pos),
            Occ::Neg => Some(Pol::Neg),
            Occ::Empty => None,
        }
    }
}

/// Which way round the magnet was glued into a piece. Random per piece, stable
/// while that piece stays upright, and carrying no colour or type information —
/// but a free one-bit fingerprint that survives being picked up and put down.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pol {
    Pos,
    Neg,
}

/// Maps a square index through `quarter_turns` clockwise rotations of the whole
/// board. Used to reconcile a board mounted at an angle to the game without
/// touching either the device protocol or the chess layer.
pub fn rotate(sq: u8, quarter_turns: u8) -> u8 {
    let (rank, file) = (sq / 8, sq % 8);
    let (r, f) = match quarter_turns & 3 {
        0 => (rank, file),
        1 => (file, 7 - rank),
        2 => (7 - rank, 7 - file),
        _ => (7 - file, rank),
    };
    r * 8 + f
}

/// The quadrant that physically owns a device square (`docs/client-api.md`
/// §Quadrant mapping).
pub fn node_of_device_square(sq: u8) -> usize {
    let (rank, file) = (sq / 8, sq % 8);
    ((rank / 4) * 2 + (file / 4)) as usize
}

/// The four orthogonal neighbours of a square. A magnet sits at the base of a
/// piece and the sensor at the centre of a square, so a piece dropped off-centre
/// reads next door — always orthogonally, since that is where the overlap is.
pub fn neighbours(sq: u8) -> Vec<u8> {
    let (rank, file) = (sq / 8, sq % 8);
    let mut out = Vec::with_capacity(4);
    if rank > 0 {
        out.push(sq - 8);
    }
    if rank < 7 {
        out.push(sq + 8);
    }
    if file > 0 {
        out.push(sq - 1);
    }
    if file < 7 {
        out.push(sq + 1);
    }
    out
}

/// Everything the game knows about the physical board, in game coordinates.
pub struct Observer {
    /// Last settled occupancy per square. Squares that are not `known` still
    /// carry a value; nothing is allowed to read it.
    pub occ: [Occ; SQUARES],
    /// Square is mid-transition (firmware `uncertain`). Sticky until the next
    /// definite reading.
    pub wobble: [bool; SQUARES],
    /// Operator- or auto-masked squares, excluded from every comparison.
    pub masked: [bool; SQUARES],
    pub node_online: [bool; NODES],
    /// Squares seen momentarily empty or wobbling since the last commit. This is
    /// the evidence that names which piece was lifted in an ambiguous capture.
    pub journal: [bool; SQUARES],
    /// Clockwise quarter turns between the physical board and the game.
    pub rotation: u8,
    /// Wall-clock of the last change to occupancy or wobble on a known square.
    pub last_change_ms: u64,
    /// Wall-clock of the last device event of any kind, for staleness.
    pub last_event_ms: u64,
    pub have_snapshot: bool,
}

impl Default for Observer {
    fn default() -> Self {
        Self::new()
    }
}

impl Observer {
    pub fn new() -> Self {
        Observer {
            occ: [Occ::Empty; SQUARES],
            wobble: [false; SQUARES],
            masked: [false; SQUARES],
            node_online: [false; NODES],
            journal: [false; SQUARES],
            rotation: 0,
            last_change_ms: 0,
            last_event_ms: 0,
            have_snapshot: false,
        }
    }

    /// Forgets the board but keeps operator configuration (masks, rotation):
    /// those are calibration of the venue, not state of the game.
    pub fn reset_board(&mut self) {
        self.occ = [Occ::Empty; SQUARES];
        self.wobble = [false; SQUARES];
        self.journal = [false; SQUARES];
        self.have_snapshot = false;
    }

    /// Rotates the stored board with the mounting, so changing rotation mid-game
    /// does not require waiting for the next snapshot to stop lying.
    pub fn set_rotation(&mut self, quarter_turns: u8) {
        let delta = quarter_turns.wrapping_sub(self.rotation) & 3;
        self.rotation = quarter_turns & 3;
        if delta == 0 {
            return;
        }
        let (mut occ, mut wobble, mut masked) =
            ([Occ::Empty; SQUARES], [false; SQUARES], [false; SQUARES]);
        for sq in 0..SQUARES {
            let dst = rotate(sq as u8, delta) as usize;
            occ[dst] = self.occ[sq];
            wobble[dst] = self.wobble[sq];
            masked[dst] = self.masked[sq];
        }
        self.occ = occ;
        self.wobble = wobble;
        self.masked = masked;
    }

    /// A square whose reading may be believed. Unknown squares never contribute
    /// evidence in either direction: a dead quadrant degrades detection without
    /// corrupting it.
    pub fn known(&self, sq: usize) -> bool {
        !self.masked[sq] && self.node_online[node_of_device_square(self.to_device(sq as u8))]
    }

    pub fn to_device(&self, game_sq: u8) -> u8 {
        rotate(game_sq, 4 - (self.rotation & 3))
    }

    pub fn to_game(&self, device_sq: u8) -> u8 {
        rotate(device_sq, self.rotation)
    }

    pub fn known_mask(&self) -> [bool; SQUARES] {
        let mut out = [false; SQUARES];
        for (sq, slot) in out.iter_mut().enumerate() {
            *slot = self.known(sq);
        }
        out
    }

    /// Nothing has moved and nothing is mid-transition for `settle_ms`. A hand
    /// held over the board keeps this false, which is exactly right — it holds
    /// the state machine open rather than committing to a half-made move.
    pub fn settled(&self, now_ms: u64, settle_ms: u64) -> bool {
        if !self.have_snapshot {
            return false;
        }
        if now_ms.saturating_sub(self.last_change_ms) < settle_ms {
            return false;
        }
        !(0..SQUARES).any(|sq| self.wobble[sq] && self.known(sq))
    }

    pub fn clear_journal(&mut self) {
        self.journal = [false; SQUARES];
    }

    /// 64 characters instead of a 600-byte array, which matters at 10 Hz to
    /// every connected client.
    pub fn observed_string(&self) -> String {
        (0..SQUARES)
            .map(|sq| {
                if !self.known(sq) {
                    'x'
                } else if self.wobble[sq] {
                    '?'
                } else {
                    match self.occ[sq] {
                        Occ::Empty => '.',
                        Occ::Pos => '+',
                        Occ::Neg => '-',
                    }
                }
            })
            .collect()
    }

    /// Applies a `board.snapshot` payload. Authoritative by contract: it
    /// supersedes every earlier sensor event in the same boot session.
    pub fn apply_snapshot(&mut self, data: &Value, now_ms: u64) {
        self.last_event_ms = now_ms;
        let squares = data.get("squares").and_then(Value::as_array);
        let valid = data.get("valid").and_then(Value::as_array);

        // Prefer the per-node array; fall back to the bitmask, and if neither is
        // present assume every quadrant is up rather than blanking the board.
        let mut online = [false; NODES];
        if let Some(nodes) = data.get("nodes").and_then(Value::as_array) {
            for entry in nodes {
                let Some(idx) = entry.get("node").and_then(Value::as_u64) else {
                    continue;
                };
                if (idx as usize) < NODES {
                    online[idx as usize] = entry
                        .get("online")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                }
            }
        } else if let Some(mask) = data.get("online_node_mask").and_then(Value::as_u64) {
            for (node, slot) in online.iter_mut().enumerate() {
                *slot = mask & (1 << node) != 0;
            }
        } else {
            online = [true; NODES];
        }
        self.node_online = online;

        let Some(squares) = squares else { return };
        let mut changed = !self.have_snapshot;
        for device_sq in 0..SQUARES {
            let game_sq = self.to_game(device_sq as u8) as usize;
            let node = node_of_device_square(device_sq as u8);
            if !self.node_online[node] {
                continue;
            }
            let raw = squares.get(device_sq).and_then(Value::as_i64).unwrap_or(0);
            let is_valid = valid
                .and_then(|v| v.get(device_sq))
                .and_then(Value::as_bool)
                .unwrap_or(true);

            if !is_valid {
                // Node is online, so this is the piece-being-lifted signal, not
                // an offline quadrant. Keep the occupancy, flag the instability.
                if !self.wobble[game_sq] {
                    self.wobble[game_sq] = true;
                    changed = true;
                }
                self.journal[game_sq] = true;
                continue;
            }
            let next = match raw {
                1 => Occ::Pos,
                -1 => Occ::Neg,
                _ => Occ::Empty,
            };
            if !next.occupied() {
                self.journal[game_sq] = true;
            }
            if self.occ[game_sq] != next || self.wobble[game_sq] {
                changed = true;
            }
            self.occ[game_sq] = next;
            self.wobble[game_sq] = false;
        }
        self.have_snapshot = true;
        if changed {
            self.last_change_ms = now_ms;
        }
    }

    /// Applies one `sensor.changed` payload. The caller is responsible for
    /// `(boot_id, seq)` continuity; the server already re-requests a snapshot on
    /// a gap, and that snapshot heals whatever was missed.
    pub fn apply_sensor_changed(&mut self, data: &Value, now_ms: u64) {
        self.last_event_ms = now_ms;
        let Some(device_sq) = data.get("square").and_then(Value::as_u64) else {
            return;
        };
        if device_sq as usize >= SQUARES {
            return;
        }
        let game_sq = self.to_game(device_sq as u8) as usize;
        let state = data.get("state").and_then(Value::as_str).unwrap_or("");
        let before = (self.occ[game_sq], self.wobble[game_sq]);
        match state {
            "positive" => {
                self.occ[game_sq] = Occ::Pos;
                self.wobble[game_sq] = false;
            }
            "negative" => {
                self.occ[game_sq] = Occ::Neg;
                self.wobble[game_sq] = false;
            }
            "empty" => {
                self.occ[game_sq] = Occ::Empty;
                self.wobble[game_sq] = false;
                self.journal[game_sq] = true;
            }
            // Only ever reachable from an occupied state (`sensors.cpp:146`), so
            // this is a piece on its way off the square: journal it and hold the
            // last occupancy until the square resolves.
            "uncertain" => {
                self.wobble[game_sq] = true;
                self.journal[game_sq] = true;
            }
            _ => return,
        }
        if before != (self.occ[game_sq], self.wobble[game_sq]) {
            self.last_change_ms = now_ms;
        }
    }

    /// Applies a `node.status` envelope so a quadrant dropping out is noticed
    /// between snapshots rather than at the next one.
    pub fn apply_node_status(&mut self, data: &Value, now_ms: u64) {
        self.last_event_ms = now_ms;
        let Some(node) = data.get("node").and_then(Value::as_u64) else {
            return;
        };
        if node as usize >= NODES {
            return;
        }
        let online = data
            .get("online")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if self.node_online[node as usize] != online {
            self.node_online[node as usize] = online;
            self.last_change_ms = now_ms;
        }
    }

    pub fn offline_nodes(&self) -> Vec<usize> {
        (0..NODES).filter(|&n| !self.node_online[n]).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot(squares: Vec<i64>, valid: Vec<bool>, online: u64) -> Value {
        json!({ "squares": squares, "valid": valid, "online_node_mask": online })
    }

    #[test]
    fn rotation_round_trips() {
        for q in 0..4u8 {
            for sq in 0..64u8 {
                assert_eq!(rotate(rotate(sq, q), 4 - q), sq);
            }
        }
        // One clockwise quarter turn carries a1 onto h1; a half turn onto h8.
        assert_eq!(rotate(0, 1), 7);
        assert_eq!(rotate(0, 2), 63);
    }

    #[test]
    fn quadrant_mapping_matches_the_client_contract() {
        assert_eq!(node_of_device_square(0), 0); // a1
        assert_eq!(node_of_device_square(7), 1); // h1
        assert_eq!(node_of_device_square(56), 2); // a8
        assert_eq!(node_of_device_square(63), 3); // h8
    }

    /// The `valid` trap: a lifted piece and an offline quadrant both clear
    /// `valid`, and only `online_node_mask` tells them apart.
    #[test]
    fn invalid_on_an_online_node_is_a_lift_not_an_absence() {
        let mut obs = Observer::new();
        let mut squares = vec![0i64; 64];
        squares[12] = 1;
        obs.apply_snapshot(&snapshot(squares.clone(), vec![true; 64], 0b1111), 1_000);
        assert!(obs.occ[12].occupied());

        let mut valid = vec![true; 64];
        valid[12] = false;
        obs.apply_snapshot(&snapshot(squares, valid, 0b1111), 2_000);
        assert!(obs.occ[12].occupied(), "occupancy is sticky through a lift");
        assert!(obs.wobble[12], "and the square is flagged unstable");
        assert!(obs.journal[12], "the lift is evidence, so it is journalled");
        assert!(!obs.settled(2_100, 700), "a wobbling square holds the window open");
    }

    #[test]
    fn offline_quadrant_squares_are_never_known() {
        let mut obs = Observer::new();
        obs.apply_snapshot(&snapshot(vec![0; 64], vec![true; 64], 0b0111), 1_000);
        assert!(obs.known(0), "node 0 is up");
        assert!(!obs.known(63), "node 3 is down, so h8 says nothing");
        assert_eq!(obs.observed_string().chars().nth(63), Some('x'));
    }

    #[test]
    fn settling_waits_out_the_window_then_holds() {
        let mut obs = Observer::new();
        obs.apply_snapshot(&snapshot(vec![0; 64], vec![true; 64], 0b1111), 1_000);
        assert!(!obs.settled(1_500, 700));
        assert!(obs.settled(1_800, 700));
        obs.apply_sensor_changed(&json!({ "square": 27, "state": "positive" }), 2_000);
        assert!(!obs.settled(2_300, 700));
        assert!(obs.settled(2_800, 700));
    }

    #[test]
    fn rotation_carries_the_board_and_the_masks_with_it() {
        let mut obs = Observer::new();
        obs.apply_snapshot(&snapshot(vec![0; 64], vec![true; 64], 0b1111), 1_000);
        obs.apply_sensor_changed(&json!({ "square": 0, "state": "positive" }), 1_100);
        obs.masked[0] = true;
        obs.set_rotation(2);
        assert!(obs.occ[63].occupied());
        assert!(obs.masked[63]);
        assert!(!obs.masked[0]);
        // Ingest is rotated too, so the device's a1 now lands on the game's h8.
        obs.apply_sensor_changed(&json!({ "square": 0, "state": "empty" }), 1_200);
        assert!(!obs.occ[63].occupied());
    }
}
