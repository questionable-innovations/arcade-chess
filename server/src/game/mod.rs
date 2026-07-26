//! Arcade puzzle mode: the state machine, and the single tokio task that owns
//! it.
//!
//! # The rule this is all built around
//!
//! > **The website is the game; the board is a peripheral.**
//!
//! Every physical input has a screen equivalent, every physical output is
//! decorative, and the game can be played start to finish with the ESP
//! unplugged. Every capability degrades along a fixed path and *names* what it
//! lost: a `degraded` list rides on every broadcast and renders as amber chips.
//! "The system knows what's broken" beats "the system pretends".
//!
//! # Why one task and not a mutex
//!
//! State is server-authoritative because a browser reload must not lose the
//! game mid-demo, Stockfish has to be server-side (the frontend is static assets
//! on Cloudflare), and the projector, the operator's laptop and any phone must
//! all see one truth. A `Mutex<GameState>` is the obvious alternative and it is
//! a trap: the engine call is `async`, and holding a std mutex across that await
//! is a deadlock waiting to happen. So the task owns the state outright and
//! three producers feed it through one channel — device events, client actions,
//! and engine results.
//!
//! Broadcasts are coalesced on a 100 ms tick behind a dirty flag. Setup-phase
//! occupancy changes with every sensor event, and a full snapshot per event is
//! what trips the existing `shed_lagged` path and starts dropping viewers.

pub mod engine;
pub mod infer;
pub mod observe;
pub mod paint;
pub mod positions;

use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use shakmaty::uci::UciMove;
use shakmaty::{Chess, Color, Move, Position};
use tokio::sync::mpsc;

use crate::state::{command_envelope, AppState, DeviceLookup};
use crate::util::now_ms;

use engine::{EvalResult, EvalSource};
use infer::{Confidence, Inference, Offset};
use observe::{Observer, Pol, NODES, SQUARES};
use paint::{Frame, Paint, Painter};
use positions::PositionRecord;

/// How often the task wakes to settle, count down, repaint and broadcast.
const TICK: Duration = Duration::from_millis(100);
/// A bound device whose events stop arriving is no longer proof of anything.
const SENSOR_STALE_MS: u64 = 20_000;
/// Correlation ids retained so a `node_error` can be attributed to the command
/// that provoked it. Only ever a handful are in flight.
const COMMAND_MEMORY: usize = 24;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Idle,
    Setup,
    Countdown,
    Playing,
    AwaitingChoice,
    Scoring,
    Finished,
}

impl Phase {
    fn as_str(self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Setup => "setup",
            Phase::Countdown => "countdown",
            Phase::Playing => "playing",
            Phase::AwaitingChoice => "awaiting_choice",
            Phase::Scoring => "scoring",
            Phase::Finished => "finished",
        }
    }

    /// Phases in which a move may be committed at all.
    fn in_play(self) -> bool {
        matches!(self, Phase::Playing | Phase::AwaitingChoice)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DetectMode {
    /// Tier 1/2 commit silently, Tier 3 commits with a confidence badge.
    Auto,
    /// Nothing auto-commits; the board proposes and one tap confirms. Switch
    /// here the moment auto misfires on stage: still feels magic, cannot be
    /// wrong.
    Suggest,
    /// Pure click-to-move.
    Off,
}

impl DetectMode {
    fn as_str(self) -> &'static str {
        match self {
            DetectMode::Auto => "auto",
            DetectMode::Suggest => "suggest",
            DetectMode::Off => "off",
        }
    }

    fn parse(s: &str) -> Option<DetectMode> {
        match s {
            "auto" => Some(DetectMode::Auto),
            "suggest" => Some(DetectMode::Suggest),
            "off" => Some(DetectMode::Off),
            _ => None,
        }
    }
}

/// Everything worth changing from a phone at the venue. Being able to nudge
/// these without a redeploy is the highest-value risk mitigation in the build.
#[derive(Clone, Copy, Debug)]
pub struct Tunables {
    pub settle_ms: u64,
    pub autostart_stable_ms: u64,
    pub unknown_tolerance: usize,
    pub tier3_max_distance: f64,
    pub tier3_margin: f64,
    pub draw_band_cp: i32,
    pub countdown_ms: u64,
}

impl Default for Tunables {
    fn default() -> Self {
        Tunables {
            settle_ms: env_u64("SETTLE_MS", 700),
            autostart_stable_ms: env_u64("AUTOSTART_STABLE_MS", 1500),
            unknown_tolerance: env_u64("UNKNOWN_TOLERANCE", 0) as usize,
            tier3_max_distance: env_f64("TIER3_MAX_DISTANCE", 1.0),
            tier3_margin: env_f64("TIER3_MARGIN", 1.0),
            draw_band_cp: env_u64("DRAW_BAND_CP", 40) as i32,
            countdown_ms: env_u64("COUNTDOWN_MS", 3000),
        }
    }
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

#[derive(Clone, Debug)]
struct MoveRecord {
    uci: String,
    san: String,
    by: &'static str,
    confidence: Confidence,
}

/// One reversible ply. The whole position and the whole fingerprint table are
/// kept rather than a delta: 64 bytes a ply over ten plies is nothing, and an
/// exact restore has no edge cases to get wrong.
struct Ply {
    pos: Chess,
    pol_tag: [Option<Pol>; SQUARES],
    eval: EvalView,
}

#[derive(Clone, Copy, Debug)]
struct EvalView {
    cp: i32,
    mate: Option<i32>,
    win_prob: f64,
    depth: u32,
    source: EvalSource,
    /// `ok` once a real number has landed; `pending` while a search is out.
    status: &'static str,
}

impl Default for EvalView {
    fn default() -> Self {
        EvalView {
            cp: 0,
            mate: None,
            win_prob: 0.5,
            depth: 0,
            source: EvalSource::Material,
            status: "pending",
        }
    }
}

#[derive(Clone, Debug)]
struct Choice {
    kind: &'static str,
    prompt: String,
    options: Vec<infer::Candidate>,
}

struct Autopilot {
    interval_ms: u64,
    next_ms: u64,
}

/// Everything the game task owns.
pub struct Game {
    state: Arc<AppState>,
    deck: positions::Deck,
    engine: engine::EngineHandle,

    game_seq: u64,
    phase: Phase,
    device_id: Option<String>,
    position: Option<PositionRecord>,
    start_fen: String,
    start_cp: i32,
    pos: Chess,
    moves: Vec<MoveRecord>,
    history: Vec<Ply>,
    max_ply: usize,

    obs: Observer,
    pol_tag: [Option<Pol>; SQUARES],
    painter: Painter,

    detect_mode: DetectMode,
    tun: Tunables,

    choice: Option<Choice>,
    eval: EvalView,
    result: Option<Value>,
    manual_degraded: BTreeSet<String>,

    stable_since: Option<u64>,
    countdown_until: Option<u64>,
    last_inferred_change_ms: u64,
    /// Consecutive settles a square has disagreed with the game position.
    disagree_streak: [u8; SQUARES],
    auto_masked: BTreeSet<u8>,
    nudge: Option<Offset>,
    mismatch: Vec<u8>,

    autopilot: Option<Autopilot>,
    recent_commands: VecDeque<(String, &'static str)>,
    dirty: bool,
    last_persisted_phase: Option<Phase>,
}

pub enum GameInput {
    Device { device_id: String, event: Value },
    Client { action: Value, is_admin: bool, reply: mpsc::Sender<String> },
    Eval(EvalResult),
}

#[derive(Clone)]
pub struct GameHandle {
    tx: mpsc::UnboundedSender<GameInput>,
}

impl GameHandle {
    pub fn send(&self, input: GameInput) {
        let _ = self.tx.send(input);
    }
}

/// Spawns the game task and returns the handle the rest of the server posts to.
pub fn spawn(state: Arc<AppState>) -> GameHandle {
    let (tx, rx) = mpsc::unbounded_channel::<GameInput>();

    // The engine speaks back through the same channel every other input arrives
    // on, so an eval needs no lock and no separate wakeup path.
    let (eval_tx, mut eval_rx) = mpsc::unbounded_channel::<EvalResult>();
    {
        let tx = tx.clone();
        tokio::spawn(async move {
            while let Some(result) = eval_rx.recv().await {
                if tx.send(GameInput::Eval(result)).is_err() {
                    break;
                }
            }
        });
    }
    let engine = engine::spawn(eval_tx);

    let game = Game::new(state, engine);
    tokio::spawn(run(game, rx));
    GameHandle { tx }
}

async fn run(mut game: Game, mut rx: mpsc::UnboundedReceiver<GameInput>) {
    game.restore();
    game.publish();
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            input = rx.recv() => {
                let Some(input) = input else { break };
                game.handle(input);
            }
            _ = ticker.tick() => game.tick(),
        }
    }
}

impl Game {
    fn new(state: Arc<AppState>, engine: engine::EngineHandle) -> Game {
        let deck = positions::Deck::load();
        let start = "8/8/8/8/8/8/8/8 w - - 0 1";
        Game {
            state,
            deck,
            engine,
            game_seq: 0,
            phase: Phase::Idle,
            device_id: None,
            position: None,
            start_fen: start.to_string(),
            start_cp: 0,
            pos: Chess::default(),
            moves: Vec::new(),
            history: Vec::new(),
            max_ply: env_u64("MAX_PLY", 10) as usize,
            obs: Observer::new(),
            pol_tag: [None; SQUARES],
            painter: Painter::new(),
            detect_mode: DetectMode::Auto,
            tun: Tunables::default(),
            choice: None,
            eval: EvalView::default(),
            result: None,
            manual_degraded: BTreeSet::new(),
            stable_since: None,
            countdown_until: None,
            last_inferred_change_ms: 0,
            disagree_streak: [0; SQUARES],
            auto_masked: BTreeSet::new(),
            nudge: None,
            mismatch: Vec::new(),
            autopilot: None,
            recent_commands: VecDeque::new(),
            dirty: true,
            last_persisted_phase: None,
        }
    }

    // ── Input ────────────────────────────────────────────────────────────────

    fn handle(&mut self, input: GameInput) {
        match input {
            GameInput::Device { device_id, event } => self.on_device_event(&device_id, &event),
            GameInput::Client {
                action,
                is_admin,
                reply,
            } => self.on_action(&action, is_admin, &reply),
            GameInput::Eval(result) => self.on_eval(result),
        }
    }

    fn on_device_event(&mut self, device_id: &str, event: &Value) {
        // Auto-bind the first board that speaks: there is only ever one on the
        // table, and making the operator bind it by hand before anything works
        // is a step to forget under stage lights. `bind_device` overrides.
        if self.device_id.is_none() {
            self.device_id = Some(device_id.to_string());
            self.dirty = true;
        }
        if self.device_id.as_deref() != Some(device_id) {
            return;
        }
        let now = now_ms();
        let etype = event.get("type").and_then(Value::as_str).unwrap_or("");
        let data = event.get("data").cloned().unwrap_or(Value::Null);
        match etype {
            "board.snapshot" => self.obs.apply_snapshot(&data, now),
            "sensor.changed" => self.obs.apply_sensor_changed(&data, now),
            "node.status" => self.obs.apply_node_status(&data, now),
            "command.result" => self.on_command_result(event),
            _ => return,
        }
        self.dirty = true;
    }

    /// Capability discovery. An unknown UART message type answers error code 2,
    /// surfaced here as `rejected/node_error`. One reply per quadrant is the
    /// whole mechanism — no version parsing, and it works with mixed firmware
    /// across the four nodes.
    fn on_command_result(&mut self, event: &Value) {
        let id = event.get("id").and_then(Value::as_str).unwrap_or("");
        let issued = self
            .recent_commands
            .iter()
            .find(|(cid, _)| cid == id)
            .map(|(_, name)| *name);

        if event.get("status").and_then(Value::as_str) == Some("applied") {
            return;
        }
        if event.get("reason").and_then(Value::as_str) != Some("node_error") {
            return;
        }
        let data = event.get("data");
        let node = data.and_then(|d| d.get("node")).and_then(Value::as_u64);
        let code = data.and_then(|d| d.get("code")).and_then(Value::as_u64);
        let (Some(node), Some(code)) = (node, code) else {
            return;
        };
        let was_bar = issued == Some("lighting.bar");
        if self.painter.note_node_error(node as usize, code, was_bar) {
            tracing::info!(node, was_bar, "quadrant refused rich lighting; dropping to basic tier");
            self.painter.forget();
        }
    }

    fn on_eval(&mut self, result: EvalResult) {
        // An eval for a position that has since been undone is dropped rather
        // than displayed.
        if result.game_seq != self.game_seq {
            return;
        }
        let result = if result.available {
            result
        } else {
            engine::material_eval(&self.pos, self.game_seq, result.final_verdict)
        };
        self.eval = EvalView {
            cp: result.cp,
            mate: result.mate,
            win_prob: result.win_prob,
            depth: result.depth,
            source: result.source,
            status: "ok",
        };
        // The verdict judges the swing, so the baseline is whatever the dealt
        // position actually scored — not the 0.00 the mining band implies.
        if self.moves.is_empty() {
            self.start_cp = result.cp;
        }
        if result.final_verdict && self.phase == Phase::Scoring {
            let winner = engine::verdict(self.start_cp, result.cp, self.tun.draw_band_cp);
            self.finish(winner, "eval", Some(result.cp));
        }
        self.dirty = true;
    }

    // ── Client actions ───────────────────────────────────────────────────────

    fn on_action(&mut self, action: &Value, is_admin: bool, reply: &mpsc::Sender<String>) {
        let name = action.get("action").and_then(Value::as_str).unwrap_or("");
        let reject = |reason: &str| {
            let _ = reply.try_send(json!({ "type": "error", "reason": reason }).to_string());
        };
        if !(is_admin || (open_controls() && is_player_action(name))) {
            return reject("unauthorized");
        }
        let ok = match name {
            "new_game" => self.new_game(action),
            "start" => self.force_start(),
            "move" => self.action_move(action),
            "choose" => self.action_choose(action),
            "undo" => self.undo(),
            "resync" => self.resync(),
            "set_detect" => self.set_detect(action),
            "mask_square" => self.mask_square(action),
            "set_tunables" => self.set_tunables(action),
            "set_eval" => self.set_eval(action),
            "set_fen" => self.set_fen(action),
            "rescore" => self.rescore(),
            "end" => self.action_end(action),
            "abort" => self.abort(),
            "bind_device" => self.bind_device(action),
            "set_rotation" => self.set_rotation(action),
            "bars_map" => self.bars_map(action),
            "bars_test" => self.bars_test(action),
            "autopilot" => self.set_autopilot(action),
            _ => false,
        };
        if !ok {
            return reject("invalid_args");
        }
        self.dirty = true;
    }

    fn new_game(&mut self, action: &Value) -> bool {
        let record = if let Some(fen) = action.get("fen").and_then(Value::as_str) {
            if positions::validate(fen).is_err() {
                return false;
            }
            PositionRecord {
                id: "custom".to_string(),
                fen: fen.to_string(),
                verified_cp: None,
                drop_cp: None,
            }
        } else if let Some(id) = action.get("position_id").and_then(Value::as_str) {
            match self.deck.find(id) {
                Some(record) => record.clone(),
                None => return false,
            }
        } else {
            self.deck.deal()
        };
        let Ok(pos) = positions::validate(&record.fen) else {
            return false;
        };

        self.game_seq += 1;
        self.phase = Phase::Setup;
        self.start_fen = record.fen.clone();
        self.pos = pos;
        self.position = Some(record);
        self.moves.clear();
        self.history.clear();
        self.pol_tag = [None; SQUARES];
        self.choice = None;
        self.result = None;
        self.mismatch.clear();
        self.nudge = None;
        self.disagree_streak = [0; SQUARES];
        self.stable_since = None;
        self.countdown_until = None;
        self.eval = EvalView::default();
        self.start_cp = 0;
        self.obs.clear_journal();
        self.painter.forget();
        // One consistent "a piece is here" colour costs an EEPROM commit per
        // quadrant, so it happens once here rather than once a frame.
        if !self.painter.colours_neutralised {
            let online = self.obs.node_online;
            for command in self.painter.neutralise_colours(online) {
                self.send_command(command.name, command.args);
            }
        }
        self.request_eval(false);
        true
    }

    /// Always enabled, always works, and shows what it overrides. A manual
    /// start records polarity only for squares that actually read occupied; the
    /// rest stay unknown and simply supply no evidence later.
    fn force_start(&mut self) -> bool {
        if !matches!(self.phase, Phase::Setup | Phase::Countdown) {
            return false;
        }
        self.begin_play();
        true
    }

    fn begin_play(&mut self) {
        self.phase = Phase::Playing;
        self.countdown_until = None;
        self.stable_since = None;
        // Learn the per-piece fingerprints for free, from whatever is actually
        // on the board right now.
        self.pol_tag = [None; SQUARES];
        for square in 0..SQUARES {
            if self.obs.known(square) {
                self.pol_tag[square] = self.obs.occ[square].polarity();
            }
        }
        self.obs.clear_journal();
        self.last_inferred_change_ms = self.obs.last_change_ms;
        self.request_eval(false);
        self.dirty = true;
    }

    fn action_move(&mut self, action: &Value) -> bool {
        let Some(uci) = action.get("uci").and_then(Value::as_str) else {
            return false;
        };
        if !self.phase.in_play() {
            return false;
        }
        self.commit(uci, "manual", Confidence::Certain, None)
    }

    fn action_choose(&mut self, action: &Value) -> bool {
        let uci = action.get("uci").and_then(Value::as_str).unwrap_or("");
        // "None of these" dismisses the prompt and leaves the position alone.
        if uci.is_empty() {
            self.choice = None;
            self.phase = Phase::Playing;
            return true;
        }
        if !self.phase.in_play() {
            return false;
        }
        self.commit(uci, "chosen", Confidence::Certain, None)
    }

    fn undo(&mut self) -> bool {
        let Some(ply) = self.history.pop() else {
            return false;
        };
        self.pos = ply.pos;
        self.pol_tag = ply.pol_tag;
        self.eval = ply.eval;
        self.moves.pop();
        self.game_seq += 1;
        self.phase = Phase::Playing;
        self.choice = None;
        self.result = None;
        self.nudge = None;
        self.mismatch.clear();
        self.obs.clear_journal();
        self.disagree_streak = [0; SQUARES];
        self.last_inferred_change_ms = self.obs.last_change_ms;
        self.request_eval(false);
        true
    }

    /// The big hammer: "the game state is right, believe the board matches it
    /// now" — for when someone knocks the pieces over and rebuilds.
    fn resync(&mut self) -> bool {
        self.obs.clear_journal();
        self.disagree_streak = [0; SQUARES];
        self.nudge = None;
        self.mismatch.clear();
        self.choice = None;
        if self.phase == Phase::AwaitingChoice {
            self.phase = Phase::Playing;
        }
        self.last_inferred_change_ms = self.obs.last_change_ms;
        true
    }

    fn set_detect(&mut self, action: &Value) -> bool {
        let Some(mode) = action
            .get("mode")
            .and_then(Value::as_str)
            .and_then(DetectMode::parse)
        else {
            return false;
        };
        self.detect_mode = mode;
        true
    }

    fn mask_square(&mut self, action: &Value) -> bool {
        let Some(square) = action.get("square").and_then(Value::as_u64) else {
            return false;
        };
        if square as usize >= SQUARES {
            return false;
        }
        let masked = action.get("masked").and_then(Value::as_bool).unwrap_or(true);
        self.obs.masked[square as usize] = masked;
        if masked {
            self.auto_masked.remove(&(square as u8));
            self.manual_degraded
                .insert(format!("sensor_{square}_masked"));
        } else {
            self.manual_degraded
                .remove(&format!("sensor_{square}_masked"));
            self.auto_masked.remove(&(square as u8));
        }
        self.disagree_streak[square as usize] = 0;
        true
    }

    fn set_tunables(&mut self, action: &Value) -> bool {
        let mut touched = false;
        let mut take_u64 = |key: &str, slot: &mut u64| {
            if let Some(v) = action.get(key).and_then(Value::as_u64) {
                *slot = v;
                touched = true;
            }
        };
        take_u64("settle_ms", &mut self.tun.settle_ms);
        take_u64("autostart_stable_ms", &mut self.tun.autostart_stable_ms);
        take_u64("countdown_ms", &mut self.tun.countdown_ms);
        if let Some(v) = action.get("unknown_tolerance").and_then(Value::as_u64) {
            self.tun.unknown_tolerance = v as usize;
            touched = true;
        }
        if let Some(v) = action.get("tier3_max_distance").and_then(Value::as_f64) {
            self.tun.tier3_max_distance = v;
            touched = true;
        }
        if let Some(v) = action.get("tier3_margin").and_then(Value::as_f64) {
            self.tun.tier3_margin = v;
            touched = true;
        }
        if let Some(v) = action.get("draw_band_cp").and_then(Value::as_i64) {
            self.tun.draw_band_cp = v as i32;
            touched = true;
        }
        touched
    }

    fn set_eval(&mut self, action: &Value) -> bool {
        let Some(cp) = action.get("cp").and_then(Value::as_i64) else {
            return false;
        };
        let cp = cp as i32;
        self.eval = EvalView {
            cp,
            mate: None,
            win_prob: engine::win_probability(cp),
            depth: 0,
            source: EvalSource::Admin,
            status: "ok",
        };
        true
    }

    /// Overwrite the position outright — the desync escape for when undo cannot
    /// reconstruct reality because reality moved on without us.
    fn set_fen(&mut self, action: &Value) -> bool {
        let Some(fen) = action.get("fen").and_then(Value::as_str) else {
            return false;
        };
        let Some(pos) = engine::position_of(fen) else {
            return false;
        };
        self.pos = pos;
        self.game_seq += 1;
        self.history.clear();
        self.choice = None;
        self.result = None;
        self.nudge = None;
        self.mismatch.clear();
        self.pol_tag = [None; SQUARES];
        if self.phase == Phase::Idle || self.phase == Phase::Finished {
            self.phase = Phase::Playing;
        }
        if self.phase == Phase::AwaitingChoice {
            self.phase = Phase::Playing;
        }
        self.resync();
        self.request_eval(false);
        true
    }

    fn rescore(&mut self) -> bool {
        self.request_eval(self.phase == Phase::Scoring);
        true
    }

    fn action_end(&mut self, action: &Value) -> bool {
        let winner = action.get("winner").and_then(Value::as_str).unwrap_or("draw");
        if !matches!(winner, "white" | "black" | "draw") {
            return false;
        }
        // Leak the string into a 'static so the result shape stays uniform;
        // there are three possible values and they are matched above.
        let winner = match winner {
            "white" => "white",
            "black" => "black",
            _ => "draw",
        };
        self.finish(winner, "admin", None);
        true
    }

    fn abort(&mut self) -> bool {
        self.phase = Phase::Idle;
        self.game_seq += 1;
        self.choice = None;
        self.result = None;
        self.nudge = None;
        self.mismatch.clear();
        self.autopilot = None;
        self.countdown_until = None;
        self.stable_since = None;
        let online = self.obs.node_online;
        for command in self.painter.restore_colours(online) {
            self.send_command(command.name, command.args);
        }
        true
    }

    fn bind_device(&mut self, action: &Value) -> bool {
        let Some(device_id) = action.get("device_id").and_then(Value::as_str) else {
            return false;
        };
        self.device_id = Some(device_id.to_string());
        self.obs.reset_board();
        self.request_snapshot();
        true
    }

    fn set_rotation(&mut self, action: &Value) -> bool {
        let Some(degrees) = action.get("degrees").and_then(Value::as_u64) else {
            return false;
        };
        if degrees % 90 != 0 || degrees >= 360 {
            return false;
        }
        self.obs.set_rotation((degrees / 90) as u8);
        self.painter.forget();
        true
    }

    fn bars_map(&mut self, action: &Value) -> bool {
        let side = action.get("side").and_then(Value::as_u64);
        let half = action.get("half").and_then(Value::as_u64);
        let (Some(side), Some(half)) = (side, half) else {
            return false;
        };
        if side as usize >= paint::SIDES || half > 1 {
            return false;
        }
        let slot = &mut self.painter.bar_map[side as usize][half as usize];
        if let Some(node) = action.get("node").and_then(Value::as_u64) {
            if node as usize >= NODES {
                return false;
            }
            slot.node = node as u8;
        }
        if let Some(strip) = action.get("strip").and_then(Value::as_str) {
            match strip {
                "a" => slot.strip = 'a',
                "b" => slot.strip = 'b',
                _ => return false,
            }
        }
        if let Some(reversed) = action.get("reversed").and_then(Value::as_bool) {
            slot.reversed = reversed;
        }
        self.painter.forget();
        true
    }

    fn bars_test(&mut self, action: &Value) -> bool {
        let Some(node) = action.get("node").and_then(Value::as_u64) else {
            return false;
        };
        if node as usize >= NODES {
            return false;
        }
        let strip = match action.get("strip").and_then(Value::as_str) {
            Some("a") | None => 'a',
            Some("b") => 'b',
            _ => return false,
        };
        let pixel = action
            .get("pixel")
            .and_then(Value::as_u64)
            .map(|p| p as usize)
            .filter(|&p| p < 8);
        let command = Painter::bars_test(node as u8, strip, pixel);
        self.send_command(command.name, command.args);
        true
    }

    fn set_autopilot(&mut self, action: &Value) -> bool {
        let on = action.get("on").and_then(Value::as_bool).unwrap_or(false);
        if !on {
            self.autopilot = None;
            return true;
        }
        let interval_ms = action
            .get("interval_ms")
            .and_then(Value::as_u64)
            .unwrap_or(4000)
            .clamp(500, 60_000);
        self.autopilot = Some(Autopilot {
            interval_ms,
            next_ms: now_ms() + interval_ms,
        });
        true
    }

    // ── The clock ────────────────────────────────────────────────────────────

    fn tick(&mut self) {
        let now = now_ms();
        match self.phase {
            Phase::Setup => self.tick_setup(now),
            Phase::Countdown => self.tick_countdown(now),
            Phase::Playing | Phase::AwaitingChoice => self.tick_playing(now),
            _ => {}
        }
        self.tick_autopilot(now);
        self.repaint(now);
        if self.dirty {
            self.publish();
        }
    }

    fn tick_setup(&mut self, now: u64) {
        let diff = self.setup_diff();
        let ready = diff.missing.is_empty()
            && diff.extra.is_empty()
            && diff.unknown.len() <= self.tun.unknown_tolerance
            && self.obs.settled(now, self.tun.settle_ms);
        if !ready {
            if self.stable_since.is_some() {
                self.dirty = true;
            }
            self.stable_since = None;
            return;
        }
        let since = *self.stable_since.get_or_insert(now);
        if now.saturating_sub(since) >= self.tun.autostart_stable_ms {
            self.phase = Phase::Countdown;
            self.countdown_until = Some(now + self.tun.countdown_ms);
            self.dirty = true;
        }
    }

    fn tick_countdown(&mut self, now: u64) {
        // Any board change aborts the countdown: someone is still building.
        let diff = self.setup_diff();
        if !diff.missing.is_empty() || !diff.extra.is_empty() {
            self.phase = Phase::Setup;
            self.countdown_until = None;
            self.stable_since = None;
            self.dirty = true;
            return;
        }
        if self.countdown_until.map(|at| now >= at).unwrap_or(false) {
            self.begin_play();
        } else {
            // The on-screen counter ticks every frame; the board pulses at 1 Hz.
            self.dirty = true;
        }
    }

    fn tick_playing(&mut self, now: u64) {
        if self.detect_mode == DetectMode::Off || !self.sensors_live(now) {
            return;
        }
        if !self.obs.settled(now, self.tun.settle_ms) {
            return;
        }
        if self.last_inferred_change_ms == self.obs.last_change_ms {
            return;
        }
        self.last_inferred_change_ms = self.obs.last_change_ms;
        self.on_settle();
    }

    fn tick_autopilot(&mut self, now: u64) {
        let Some(pilot) = self.autopilot.as_mut() else {
            return;
        };
        if now < pilot.next_ms {
            return;
        }
        pilot.next_ms = now + pilot.interval_ms;
        if !self.phase.in_play() {
            return;
        }
        let moves = self.pos.legal_moves();
        if moves.is_empty() {
            return;
        }
        let pick = (crate::util::random_u64() % moves.len() as u64) as usize;
        let uci = UciMove::from_standard(&moves[pick]).to_string();
        self.commit(&uci, "autopilot", Confidence::Certain, None);
        self.dirty = true;
    }

    // ── Detection ────────────────────────────────────────────────────────────

    fn on_settle(&mut self) {
        let known = self.obs.known_mask();
        let observation = infer::Observation {
            occ: &self.obs.occ,
            known: &known,
            journal: &self.obs.journal,
            pol_tag: &self.pol_tag,
        };
        let params = infer::Params {
            max_distance: self.tun.tier3_max_distance,
            margin: self.tun.tier3_margin,
        };
        let inference = infer::infer(&self.pos, &observation, &params);

        match inference {
            Inference::NoChange => {
                self.nudge = None;
                self.mismatch.clear();
                if self.phase == Phase::AwaitingChoice {
                    self.phase = Phase::Playing;
                    self.choice = None;
                }
            }
            Inference::Commit {
                uci,
                san,
                confidence,
                offset,
            } => {
                if self.detect_mode == DetectMode::Suggest {
                    self.ask(
                        "suggest",
                        format!("Play {san}?"),
                        vec![infer::Candidate {
                            uci,
                            san,
                            confidence,
                        }],
                    );
                } else {
                    self.commit(&uci, "sensor", confidence, offset);
                }
            }
            Inference::Ambiguous { kind, options } => {
                let prompt = match kind {
                    "capture" => "Which capture was that?".to_string(),
                    _ => "Which move was that?".to_string(),
                };
                self.ask(kind, prompt, options);
            }
            Inference::Mismatch { squares, options } => {
                // A standing offset is not a mismatch, it is a request. Both the
                // banner and the amber square persist until the piece is nudged
                // back; detection deliberately does not re-baseline around it.
                if let Some(nudge) = self.nudge {
                    if squares
                        .iter()
                        .all(|&sq| sq == nudge.expected || sq == nudge.actual)
                    {
                        return;
                    }
                }
                self.mismatch = squares.clone();
                self.ask(
                    "no_match",
                    "The board doesn't match any legal move.".to_string(),
                    options,
                );
            }
        }
        self.update_auto_mask();
        self.dirty = true;
    }

    fn ask(&mut self, kind: &'static str, prompt: String, options: Vec<infer::Candidate>) {
        self.choice = Some(Choice {
            kind,
            prompt,
            options,
        });
        self.phase = Phase::AwaitingChoice;
    }

    /// A square that disagrees with expectation across two consecutive settles
    /// is stuck, not surprising. Masking it lets Tier 1 resume cleanly on the
    /// remaining known squares, which beats both prompting on every ply forever
    /// and leaning on Tier 3's weights.
    ///
    /// Only ever during play: during setup a piece on the wrong square looks
    /// identical, and masking it would let a bad build pass.
    fn update_auto_mask(&mut self) {
        let expected = infer::occupancy(&self.pos);
        for (square, &expected_here) in expected.iter().enumerate() {
            if !self.obs.known(square) {
                self.disagree_streak[square] = 0;
                continue;
            }
            // The nudge already explains these two, and is being asked about.
            if let Some(nudge) = self.nudge {
                if square as u8 == nudge.expected || square as u8 == nudge.actual {
                    self.disagree_streak[square] = 0;
                    continue;
                }
            }
            if self.obs.occ[square].occupied() == expected_here {
                self.disagree_streak[square] = 0;
                continue;
            }
            self.disagree_streak[square] = self.disagree_streak[square].saturating_add(1);
            if self.disagree_streak[square] >= 2 {
                self.obs.masked[square] = true;
                self.auto_masked.insert(square as u8);
                tracing::info!(square, "square disagreed twice running; masking it");
            }
        }
    }

    // ── Moves ────────────────────────────────────────────────────────────────

    fn parse_move(&self, uci: &str) -> Option<Move> {
        let parsed: UciMove = uci.parse().ok()?;
        parsed.to_move(&self.pos).ok()
    }

    fn commit(
        &mut self,
        uci: &str,
        by: &'static str,
        confidence: Confidence,
        offset: Option<Offset>,
    ) -> bool {
        let Some(m) = self.parse_move(uci) else {
            return false;
        };
        let san = shakmaty::san::SanPlus::from_move(self.pos.clone(), &m).to_string();
        let record = MoveRecord {
            uci: UciMove::from_standard(&m).to_string(),
            san,
            by,
            confidence,
        };
        self.history.push(Ply {
            pos: self.pos.clone(),
            pol_tag: self.pol_tag,
            eval: self.eval,
        });
        infer::migrate_pol_tags(&mut self.pol_tag, self.pos.turn(), &m, &self.obs.occ);
        self.pos.play_unchecked(&m);
        self.moves.push(record);
        self.game_seq += 1;
        self.choice = None;
        self.mismatch.clear();
        self.nudge = offset;
        self.obs.clear_journal();
        self.last_inferred_change_ms = self.obs.last_change_ms;
        self.phase = Phase::Playing;

        if self.pos.is_checkmate() {
            let winner = if self.pos.turn() == Color::White {
                "black"
            } else {
                "white"
            };
            self.finish(winner, "mate", None);
        } else if self.pos.is_stalemate() || self.pos.is_insufficient_material() {
            self.finish("draw", "stalemate", None);
        } else if self.moves.len() >= self.max_ply {
            self.phase = Phase::Scoring;
            self.request_eval(true);
        } else {
            self.request_eval(false);
        }
        self.dirty = true;
        true
    }

    fn finish(&mut self, winner: &'static str, reason: &'static str, final_cp: Option<i32>) {
        self.phase = Phase::Finished;
        self.autopilot = None;
        self.result = Some(json!({
            "winner": winner,
            "final_cp": final_cp.unwrap_or(self.eval.cp),
            "start_cp": self.start_cp,
            "swing": final_cp.unwrap_or(self.eval.cp) - self.start_cp,
            "reason": reason,
        }));
        self.dirty = true;
    }

    fn request_eval(&mut self, final_verdict: bool) {
        let fen = engine::fen_of(&self.pos);
        let movetime_ms = if final_verdict {
            engine::FINAL_MOVETIME_MS
        } else {
            engine::PLY_MOVETIME_MS
        };
        self.eval.status = "pending";
        let posted = self.engine.request(engine::EvalRequest {
            fen,
            game_seq: self.game_seq,
            movetime_ms,
            final_verdict,
        });
        if !posted {
            // The engine task is gone; score it here rather than wait for a
            // result that will never arrive.
            let result = engine::material_eval(&self.pos, self.game_seq, final_verdict);
            self.on_eval(result);
        }
    }

    // ── Setup ────────────────────────────────────────────────────────────────

    fn setup_diff(&self) -> SetupDiff {
        let target = infer::occupancy(&self.start_position());
        let mut diff = SetupDiff::default();
        for (square, &wanted) in target.iter().enumerate() {
            let known = self.obs.known(square);
            if wanted && !known {
                diff.unknown.push(square as u8);
                continue;
            }
            if !known {
                continue;
            }
            let occupied = self.obs.occ[square].occupied();
            if wanted && !occupied {
                diff.missing.push(square as u8);
            } else if !wanted && occupied {
                diff.extra.push(square as u8);
            }
        }
        diff.needed = target.iter().filter(|&&t| t).count();
        diff.placed = diff.needed - diff.missing.len() - diff.unknown.len();
        diff
    }

    fn start_position(&self) -> Chess {
        engine::position_of(&self.start_fen).unwrap_or_else(|| self.pos.clone())
    }

    // ── Device output ────────────────────────────────────────────────────────

    fn send_command(&mut self, name: &'static str, args: Value) {
        let Some(device_id) = self.device_id.clone() else {
            return;
        };
        let DeviceLookup::Online(tx) = self.state.lookup_device(&device_id) else {
            return;
        };
        let (id, cmd) = command_envelope(self.state.next_seq(), name, args);
        if tx.send(cmd.to_string()).is_ok() {
            self.recent_commands.push_back((id, name));
            while self.recent_commands.len() > COMMAND_MEMORY {
                self.recent_commands.pop_front();
            }
        }
    }

    fn request_snapshot(&mut self) {
        self.send_command("board.snapshot.get", json!({}));
    }

    fn repaint(&mut self, now: u64) {
        // Idle sends nothing at all, so between games the board keeps the debug
        // behaviour it has when no lighting command has ever been sent.
        if self.phase == Phase::Idle || self.device_id.is_none() {
            return;
        }
        let frame = self.compose_frame(now);
        let rotation = self.obs.rotation;
        let online = self.obs.node_online;
        let commands = self
            .painter
            .diff(&frame, online, now, |sq| observe::rotate(sq, 4 - (rotation & 3)));
        for command in commands {
            self.send_command(command.name, command.args);
        }
    }

    fn compose_frame(&self, now: u64) -> Frame {
        let mut frame = Frame::default();
        match self.phase {
            Phase::Setup => {
                let diff = self.setup_diff();
                for square in diff.missing {
                    frame.set(square, Paint::Needed);
                }
                for square in diff.extra {
                    frame.set(square, Paint::Alert);
                }
                frame.basic = Some(Paint::Needed);
            }
            Phase::Countdown => {
                // One frame a second, not an animation: the bus is 38400 baud
                // and a stutter there reads as "the board froze".
                if (now / 500).is_multiple_of(2) {
                    let target = infer::occupancy(&self.start_position());
                    for (square, &wanted) in target.iter().enumerate() {
                        if wanted {
                            frame.set(square as u8, Paint::Needed);
                        }
                    }
                }
                frame.basic = Some(Paint::Needed);
            }
            Phase::Playing | Phase::Scoring => {
                // The move is already played, so its squares come off the UCI
                // text rather than from replaying it against the position.
                if let Some(last) = self.moves.last() {
                    if let Ok(m) = last.uci.parse::<UciMove>() {
                        for square in uci_squares(&m) {
                            frame.set(square, Paint::Focus);
                        }
                    }
                }
                if let Some(nudge) = self.nudge {
                    frame.set(nudge.expected, Paint::Needed);
                    frame.set(nudge.actual, Paint::Alert);
                }
                frame.basic = if self.nudge.is_some() {
                    Some(Paint::Needed)
                } else {
                    Some(Paint::Focus)
                };
            }
            Phase::AwaitingChoice => {
                if let Some(choice) = &self.choice {
                    for option in &choice.options {
                        if let Ok(m) = option.uci.parse::<UciMove>() {
                            for square in uci_squares(&m) {
                                frame.set(square, Paint::Focus);
                            }
                        }
                    }
                }
                for &square in &self.mismatch {
                    frame.set(square, Paint::Alert);
                }
                frame.basic = if self.mismatch.is_empty() {
                    Some(Paint::Focus)
                } else {
                    Some(Paint::Alert)
                };
            }
            Phase::Finished => {
                let winner = self
                    .result
                    .as_ref()
                    .and_then(|r| r.get("winner"))
                    .and_then(Value::as_str)
                    .unwrap_or("draw");
                let rank = match winner {
                    "white" => 0..8,
                    "black" => 56..64,
                    _ => 24..32,
                };
                for square in rank {
                    frame.set(square as u8, Paint::Sweep);
                }
                frame.basic = Some(Paint::Sweep);
            }
            Phase::Idle => {}
        }

        // Two sides carry the eval bar and two the turn indicator, so both
        // players see both.
        frame.bars_wanted = !matches!(self.phase, Phase::Idle | Phase::Setup);
        frame.eval_bar(0, self.eval.win_prob);
        frame.eval_bar(2, self.eval.win_prob);
        let white_to_move = self.pos.turn() == Color::White;
        frame.turn_bar(1, white_to_move);
        frame.turn_bar(3, white_to_move);
        frame
    }

    // ── Publishing ───────────────────────────────────────────────────────────

    /// Whether the board's readings may be believed at all. A disconnect is
    /// known immediately — the server owns that socket — so waiting out the
    /// staleness window before admitting it would leave the UI claiming live
    /// sensors for twenty seconds after the ESP dropped off the stage.
    fn sensors_live(&self, now: u64) -> bool {
        let Some(device_id) = self.device_id.as_deref() else {
            return false;
        };
        if !matches!(self.state.lookup_device(device_id), DeviceLookup::Online(_)) {
            return false;
        }
        self.obs.have_snapshot && now.saturating_sub(self.obs.last_event_ms) < SENSOR_STALE_MS
    }

    fn degraded(&self, now: u64) -> Vec<String> {
        let mut out: BTreeSet<String> = self.manual_degraded.clone();
        if self.device_id.is_none() {
            out.insert("no_device".to_string());
        } else if !self.sensors_live(now) {
            out.insert("sensors_stale".to_string());
        } else {
            for node in self.obs.offline_nodes() {
                out.insert(format!("node{node}_offline"));
            }
        }
        if self.eval.status == "ok" && self.eval.source == EvalSource::Material {
            out.insert("engine_unavailable".to_string());
        }
        if !self.painter.bars_supported {
            out.insert("bars_unsupported".to_string());
        }
        for square in &self.auto_masked {
            out.insert(format!("sensor_{square}_suspect"));
        }
        if self.deck.is_fallback() {
            out.insert("positions_fallback".to_string());
        }
        if self.detect_mode != DetectMode::Auto {
            out.insert(format!("detect_{}", self.detect_mode.as_str()));
        }
        out.into_iter().collect()
    }

    fn view(&self) -> Value {
        let now = now_ms();
        let board_synced = {
            let expected = infer::occupancy(&self.pos);
            (0..SQUARES)
                .all(|sq| !self.obs.known(sq) || self.obs.occ[sq].occupied() == expected[sq])
        };
        let legal_moves: Vec<String> = self
            .pos
            .legal_moves()
            .iter()
            .map(|m| UciMove::from_standard(m).to_string())
            .collect();

        let mut view = json!({
            "type": "game.state",
            "game_seq": self.game_seq,
            "phase": self.phase.as_str(),
            "device_id": self.device_id,
            "position": self.position.as_ref().map(|p| json!({
                "id": p.id,
                "start_fen": p.fen,
                "verified_cp": p.verified_cp,
                "drop_cp": p.drop_cp,
            })),
            "start_fen": self.start_fen,
            "fen": engine::fen_of(&self.pos),
            "turn": if self.pos.turn() == Color::White { "white" } else { "black" },
            "ply": self.moves.len(),
            "max_ply": self.max_ply,
            "moves": self.moves.iter().map(|m| json!({
                "uci": m.uci,
                "san": m.san,
                "by": m.by,
                "confidence": m.confidence.as_str(),
            })).collect::<Vec<_>>(),
            "legal_moves": legal_moves,
            "detect": {
                "mode": self.detect_mode.as_str(),
                "sensors_live": self.sensors_live(now),
                "board_synced": board_synced,
                "mismatch": self.mismatch,
                "observed": self.obs.observed_string(),
                "masked": (0..SQUARES).filter(|&s| self.obs.masked[s]).collect::<Vec<_>>(),
                "rotation": (self.obs.rotation as u32) * 90,
                "nudge": self.nudge.map(|n| json!({ "expected": n.expected, "actual": n.actual })),
            },
            "eval": {
                "cp": self.eval.cp,
                "mate": self.eval.mate,
                "win_prob": self.eval.win_prob,
                "status": self.eval.status,
                "source": self.eval.source.as_str(),
                "depth": self.eval.depth,
                "start_cp": self.start_cp,
            },
            "tunables": {
                "settle_ms": self.tun.settle_ms,
                "autostart_stable_ms": self.tun.autostart_stable_ms,
                "unknown_tolerance": self.tun.unknown_tolerance,
                "tier3_max_distance": self.tun.tier3_max_distance,
                "tier3_margin": self.tun.tier3_margin,
                "draw_band_cp": self.tun.draw_band_cp,
                "countdown_ms": self.tun.countdown_ms,
            },
            "lighting": {
                "squares": "override",
                "bars_supported": self.painter.bars_supported,
                "bar_map": self.painter.bar_map,
                "colours_neutralised": self.painter.colours_neutralised,
            },
            "autopilot": self.autopilot.as_ref().map(|a| json!({ "interval_ms": a.interval_ms })),
            "deck": { "source": self.deck.source, "count": self.deck.len(), "skipped": self.deck.skipped },
            "degraded": self.degraded(now),
        });

        if matches!(self.phase, Phase::Setup | Phase::Countdown) {
            let diff = self.setup_diff();
            view["setup"] = json!({
                "placed": diff.placed,
                "needed": diff.needed,
                "missing": diff.missing,
                "extra": diff.extra,
                "unknown": diff.unknown,
                "auto_start_in_ms": self.countdown_until.map(|at| at.saturating_sub(now)),
            });
        }
        if let Some(choice) = &self.choice {
            view["choice"] = json!({
                "kind": choice.kind,
                "prompt": choice.prompt,
                "options": choice.options.iter().map(|o| json!({
                    "uci": o.uci,
                    "san": o.san,
                    "confidence": o.confidence.as_str(),
                })).collect::<Vec<_>>(),
            });
        }
        if let Some(result) = &self.result {
            view["result"] = result.clone();
        }
        view
    }

    fn publish(&mut self) {
        self.dirty = false;
        let view = self.view();
        let text = view.to_string();
        self.state.set_game_view(view);
        self.state.broadcast_msg(text);
        if self.last_persisted_phase != Some(self.phase) {
            self.last_persisted_phase = Some(self.phase);
            self.persist();
        }
    }

    // ── Restart insurance ────────────────────────────────────────────────────

    fn snapshot_path() -> String {
        std::env::var("GAME_SNAPSHOT_PATH").unwrap_or_else(|_| "/tmp/arcade-game.json".to_string())
    }

    /// Written on every phase change, so a server restart or redeploy mid-demo
    /// is not fatal. Without it the rule is simply: do not redeploy during the
    /// demo, and restart the round — it is a five-minute game.
    fn persist(&self) {
        if self.phase == Phase::Idle {
            let _ = std::fs::remove_file(Self::snapshot_path());
            return;
        }
        let payload = json!({
            "phase": self.phase.as_str(),
            "start_fen": self.start_fen,
            "fen": engine::fen_of(&self.pos),
            "start_cp": self.start_cp,
            "position": self.position.as_ref().map(|p| json!({ "id": p.id, "fen": p.fen })),
            "moves": self.moves.iter().map(|m| json!({
                "uci": m.uci, "san": m.san, "by": m.by,
            })).collect::<Vec<_>>(),
            "eval_cp": self.eval.cp,
        });
        if let Err(err) = std::fs::write(Self::snapshot_path(), payload.to_string()) {
            tracing::warn!(%err, "could not write the game snapshot");
        }
    }

    fn restore(&mut self) {
        let Ok(text) = std::fs::read_to_string(Self::snapshot_path()) else {
            return;
        };
        let Ok(saved) = serde_json::from_str::<Value>(&text) else {
            return;
        };
        let Some(fen) = saved.get("fen").and_then(Value::as_str) else {
            return;
        };
        let Some(pos) = engine::position_of(fen) else {
            return;
        };
        self.pos = pos;
        self.start_fen = saved
            .get("start_fen")
            .and_then(Value::as_str)
            .unwrap_or(fen)
            .to_string();
        self.start_cp = saved.get("start_cp").and_then(Value::as_i64).unwrap_or(0) as i32;
        if let Some(position) = saved.get("position") {
            if let (Some(id), Some(pfen)) = (
                position.get("id").and_then(Value::as_str),
                position.get("fen").and_then(Value::as_str),
            ) {
                self.position = Some(PositionRecord {
                    id: id.to_string(),
                    fen: pfen.to_string(),
                    verified_cp: None,
                    drop_cp: None,
                });
            }
        }
        for entry in saved
            .get("moves")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            self.moves.push(MoveRecord {
                uci: entry.get("uci").and_then(Value::as_str).unwrap_or("").to_string(),
                san: entry.get("san").and_then(Value::as_str).unwrap_or("").to_string(),
                by: "manual",
                confidence: Confidence::Certain,
            });
        }
        // History is not restored: undo across a process restart would need the
        // whole position chain, and the honest thing is to say so rather than
        // half-support it.
        self.phase = match saved.get("phase").and_then(Value::as_str) {
            Some("finished") => Phase::Finished,
            Some("idle") | None => Phase::Idle,
            _ => Phase::Playing,
        };
        self.game_seq += 1;
        self.manual_degraded.insert("restored_after_restart".to_string());
        tracing::info!(phase = ?self.phase, plies = self.moves.len(), "restored game after restart");
        if self.phase != Phase::Idle {
            self.request_eval(false);
        }
    }
}

#[derive(Default)]
struct SetupDiff {
    placed: usize,
    needed: usize,
    missing: Vec<u8>,
    extra: Vec<u8>,
    unknown: Vec<u8>,
}

/// The squares a UCI move touches, for lighting. Reading them off the text
/// rather than the `Move` keeps this usable after the move has been played.
fn uci_squares(m: &UciMove) -> Vec<u8> {
    match m {
        UciMove::Normal { from, to, .. } => vec![u8::from(*from), u8::from(*to)],
        UciMove::Put { to, .. } => vec![u8::from(*to)],
        UciMove::Null => Vec::new(),
    }
}

fn open_controls() -> bool {
    std::env::var("GAME_OPEN_CONTROLS").map(|v| v == "1").unwrap_or(false)
}

/// The actions a second, unauthenticated tablet at the board may drive when
/// `GAME_OPEN_CONTROLS=1`. Everything destructive stays admin-only.
fn is_player_action(name: &str) -> bool {
    matches!(
        name,
        "new_game" | "start" | "move" | "choose" | "undo" | "resync"
    )
}
