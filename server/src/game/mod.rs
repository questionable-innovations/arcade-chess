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

pub mod config;
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

pub use config::{Palette, Rules, Tunables};

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
            // Deliberately `Unknown`, not `Material`: nothing has been counted
            // yet. Claiming a source before a number exists puts a "MATERIAL"
            // badge next to a 0.00 that was never computed, which is exactly the
            // dishonesty the labelling exists to prevent.
            source: EvalSource::Unknown,
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
    /// Which engine produced `start_cp`. The verdict is a subtraction, so a
    /// material baseline against a Stockfish final is two different scales
    /// masquerading as one number — and it decides who the demo says won.
    start_source: EvalSource,
    pos: Chess,
    moves: Vec<MoveRecord>,
    history: Vec<Ply>,

    obs: Observer,
    pol_tag: [Option<Pol>; SQUARES],
    painter: Painter,

    detect_mode: DetectMode,
    tun: Tunables,
    rules: Rules,
    palette: Palette,
    /// Which of the four board edges show the eval bar, and which the turn
    /// indicator. Both values are already on screen, but the edge the audience
    /// happens to face is the one that has to carry the eval bar.
    eval_sides: Vec<u8>,
    turn_sides: Vec<u8>,

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
    /// The game snapshot is keyed on `(phase, game_seq)` rather than phase
    /// alone: a clean auto-detected run stays in `Playing` for every ply, so a
    /// phase-only key wrote once and then went stale for the whole game.
    last_persisted: Option<(Phase, u64)>,
    /// Set by any action that changes calibration, so the venue profile is
    /// written without doing a file write per sensor event.
    config_dirty: bool,

    /// Where the game snapshot and the venue profile are written. Resolved
    /// once at construction rather than read from the environment per call, so
    /// they are injectable in tests and cannot change under a running game.
    snapshot_path: String,
    config_path: String,

    /// Monotonic milliseconds since the task started. Wall clock is wrong for
    /// every one of these deadlines — an NTP step backwards silently freezes the
    /// settle window, and a step forwards fires everything at once.
    epoch: tokio::time::Instant,
    /// The delta stream tore; nothing may be committed off the torn view until a
    /// fresh snapshot has healed it.
    gap_until_ms: Option<u64>,
}

pub enum GameInput {
    Device {
        device_id: String,
        event: Value,
        /// The device event stream had a sequence gap or the board rebooted, so
        /// the observer's deltas no longer describe a real board.
        gap: bool,
    },
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
    // Calibration first, so a restored game lands into a *calibrated* observer
    // rather than one with rotation 0 and no masks.
    game.restore_config();
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
            start_source: EvalSource::Material,
            pos: Chess::default(),
            moves: Vec::new(),
            history: Vec::new(),
            obs: Observer::new(),
            pol_tag: [None; SQUARES],
            painter: Painter::new(),
            detect_mode: DetectMode::Auto,
            tun: Tunables::default(),
            rules: Rules::default(),
            palette: Palette::default(),
            eval_sides: vec![0, 2],
            turn_sides: vec![1, 3],
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
            last_persisted: None,
            config_dirty: false,
            snapshot_path: std::env::var("GAME_SNAPSHOT_PATH")
                .unwrap_or_else(|_| "/tmp/arcade-game.json".to_string()),
            config_path: config::config_path(),
            epoch: tokio::time::Instant::now(),
            gap_until_ms: None,
        }
    }

    /// Monotonic milliseconds since the task started.
    fn now(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    // ── Input ────────────────────────────────────────────────────────────────

    fn handle(&mut self, input: GameInput) {
        match input {
            GameInput::Device {
                device_id,
                event,
                gap,
            } => self.on_device_event(&device_id, &event, gap),
            GameInput::Client {
                action,
                is_admin,
                reply,
            } => self.on_action(&action, is_admin, &reply),
            GameInput::Eval(result) => self.on_eval(result),
        }
    }

    fn on_device_event(&mut self, device_id: &str, event: &Value, gap: bool) {
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
        let now = self.now();
        let etype = event.get("type").and_then(Value::as_str).unwrap_or("");
        let data = event.get("data").cloned().unwrap_or(Value::Null);
        // A gap means the deltas since the last known-good point describe a
        // board that never existed. The server already re-requests a snapshot;
        // until that lands, nothing may be committed off the torn view.
        if gap {
            self.gap_until_ms = Some(now);
            tracing::debug!("device event stream tore; holding detection until the next snapshot");
        }
        match etype {
            "board.snapshot" => {
                self.obs.apply_snapshot(&data, now);
                // Authoritative by contract: this *is* the heal.
                self.gap_until_ms = None;
                self.obs.clear_journal();
            }
            "sensor.changed" => self.obs.apply_sensor_changed(&data, now),
            "node.status" => self.obs.apply_node_status(&data, now),
            "command.result" => self.on_command_result(event),
            _ => return,
        }
        self.dirty = true;
    }

    /// True while the observer's view is known to be torn.
    ///
    /// Gap detection only fires on the *next* sequenced event, and the only
    /// periodic one is `device.status` every 15 s — so a dropped trailing event
    /// can leave the view torn for far longer than the 700 ms settle window it
    /// has to beat.
    fn stream_torn(&self) -> bool {
        self.gap_until_ms.is_some()
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
        // An admin decree is the last word by design, so a search that was
        // already in flight when it landed must not quietly overwrite it — and
        // during scoring that search would otherwise decide the winner.
        if self.eval.source == EvalSource::Admin {
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
            self.start_source = result.source;
        }
        if result.final_verdict && self.phase == Phase::Scoring {
            let winner = self.verdict_for(result.cp, result.source);
            self.finish(winner, "eval", Some(result.cp));
        }
        self.dirty = true;
    }

    /// The swing only means anything if both ends were measured with the same
    /// ruler. A 2 s final search against a 3 s timeout is the longest and
    /// tightest search of the game, so a fallback to material at exactly that
    /// moment is realistic — and it is the number the whole demo is judged on.
    ///
    /// Measured against the shipped deck the two scales disagree by a median of
    /// ~40 cp, which is the entire draw band, so a mismatch is not a rounding
    /// error: it is a coin flip wearing a confident badge. Refuse to name a
    /// winner instead, and say why.
    fn verdict_for(&mut self, final_cp: i32, final_source: EvalSource) -> &'static str {
        let comparable = self.start_source == final_source
            || (self.start_source != EvalSource::Material
                && final_source != EvalSource::Material);
        if !comparable {
            tracing::warn!(
                start = ?self.start_source,
                final_source = ?final_source,
                "eval baseline and final came from different engines; declaring a draw"
            );
            self.manual_degraded.insert("verdict_incomparable".to_string());
            return "draw";
        }
        self.manual_degraded.remove("verdict_incomparable");
        engine::verdict(self.start_cp, final_cp, self.rules.draw_band_cp)
    }

    // ── Client actions ───────────────────────────────────────────────────────

    fn on_action(&mut self, action: &Value, is_admin: bool, reply: &mpsc::Sender<String>) {
        let name = action.get("action").and_then(Value::as_str).unwrap_or("");
        let reject = |reason: &str| {
            let _ = reply.try_send(json!({ "type": "error", "reason": reason }).to_string());
        };
        if !(is_admin || (self.rules.open_controls && is_player_action(name))) {
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
            // `set_tunables` is the documented name and still works; `set_config`
            // is the schema-driven form the admin rail uses.
            "set_tunables" | "set_config" => self.set_config(action),
            "set_eval" => self.set_eval(action),
            "set_fen" => self.set_fen(action),
            "rescore" => self.rescore(),
            "end" => self.action_end(action),
            "abort" => self.abort(),
            "bind_device" => self.bind_device(action),
            "set_rotation" => self.set_rotation(action),
            "bars_map" => self.bars_map(action),
            "bars_test" => self.bars_test(action),
            "bars_sides" => self.bars_sides(action),
            "set_palette" => self.set_palette(action),
            "node_config" => self.node_config(action),
            "autopilot" => self.set_autopilot(action),
            _ => false,
        };
        if !ok {
            return reject("invalid_args");
        }
        if self.config_dirty {
            self.persist_config();
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
        self.start_source = EvalSource::Material;
        self.gap_until_ms = None;
        // Auto-masks are per-game evidence about a position that is now gone.
        // Operator masks are calibration and deliberately survive.
        self.clear_auto_masks();
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
        // The guard comes first. Settles keep being processed throughout
        // `AwaitingChoice`, so the prompt can be resolved out from under an
        // operator whose tap is already in flight — and without this, that tap
        // dragged Scoring or Finished back into Playing, discarding the pending
        // verdict on the way.
        if !self.phase.in_play() {
            return false;
        }
        let uci = action.get("uci").and_then(Value::as_str).unwrap_or("");
        // "None of these" dismisses the prompt and leaves the position alone.
        if uci.is_empty() {
            self.choice = None;
            self.mismatch.clear();
            self.phase = Phase::Playing;
            return true;
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
        // "Believe the board matches the game now" has to include the squares
        // the game gave up on. Auto-masks were evidence about a position that no
        // longer exists, and without clearing them they accumulate silently
        // across every game of the evening.
        self.clear_auto_masks();
        self.nudge = None;
        self.mismatch.clear();
        self.choice = None;
        self.gap_until_ms = None;
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
        self.config_dirty = true;
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
        self.config_dirty = true;
        true
    }

    /// Every tunable, by key, clamped to the range the server itself advertises.
    ///
    /// Accepts both `{"key": "settle_ms", "value": 900}` and the older flat
    /// `{"settle_ms": 900}` shape. An out-of-range number is clamped rather than
    /// refused — under stage lights, "it went to the nearest sane value" beats
    /// "nothing happened" — but an unknown key or an unparseable value is a hard
    /// `invalid_args`, because a control that silently ignores you is worse than
    /// one that says no.
    fn set_config(&mut self, action: &Value) -> bool {
        let mut touched = false;
        if let Some(key) = action.get("key").and_then(Value::as_str) {
            let value = action.get("value").unwrap_or(&Value::Null);
            if !config::apply(&mut self.tun, &mut self.rules, key, value) {
                return false;
            }
            touched = true;
        }
        if let Some(map) = action.as_object() {
            for (key, value) in map {
                if matches!(key.as_str(), "action" | "key" | "value") {
                    continue;
                }
                if config::setting(key).is_none() {
                    continue;
                }
                if !config::apply(&mut self.tun, &mut self.rules, key, value) {
                    return false;
                }
                touched = true;
            }
        }
        if touched {
            self.config_dirty = true;
        }
        touched
    }

    /// Which board edges carry the eval bar and which carry the turn indicator.
    /// Unknowable until the room is set up and the audience is standing
    /// somewhere, so it is a live control rather than a constant.
    fn bars_sides(&mut self, action: &Value) -> bool {
        let read = |key: &str| -> Option<Vec<u8>> {
            let arr = action.get(key)?.as_array()?;
            let mut out = Vec::new();
            for v in arr {
                let side = v.as_u64()?;
                if side as usize >= paint::SIDES {
                    return None;
                }
                out.push(side as u8);
            }
            Some(out)
        };
        let eval = read("eval_sides");
        let turn = read("turn_sides");
        if eval.is_none() && turn.is_none() {
            return false;
        }
        if let Some(eval) = eval {
            self.eval_sides = eval;
        }
        if let Some(turn) = turn {
            self.turn_sides = turn;
        }
        self.painter.forget();
        self.config_dirty = true;
        true
    }

    /// Live palette. Whether `ff8c00` reads as amber through a wooden board at
    /// LED brightness 48 under venue lighting is not knowable from a desk, and
    /// "amber = still needed" is the entire setup experience.
    fn set_palette(&mut self, action: &Value) -> bool {
        let mut touched = false;
        let mut take = |key: &str, slot: &mut u32| {
            if let Some(v) = action.get(key).and_then(Value::as_str) {
                if let Ok(rgb) = u32::from_str_radix(v.trim_start_matches('#'), 16) {
                    *slot = rgb & 0xff_ff_ff;
                    touched = true;
                }
            }
        };
        let mut p = self.palette;
        take("alert", &mut p.alert);
        take("needed", &mut p.needed);
        take("focus", &mut p.focus);
        take("sweep", &mut p.sweep);
        take("bar_white", &mut p.bar_white);
        take("bar_black", &mut p.bar_black);
        if !touched {
            return false;
        }
        self.palette = p;
        // The board is showing the old colours right now, so drop the diff and
        // let the next frame repaint from scratch.
        self.painter.forget();
        self.config_dirty = true;
        true
    }

    /// Pass-through to the AVR's own EEPROM config keys (thresholds, debounce,
    /// LED brightness, polarity colours, orientation). The transport already
    /// existed; only a way to reach it from the web did not — which left the
    /// most venue-dependent values in the system behind a USB cable.
    ///
    /// Each write commits EEPROM (~240 ms), so this is deliberately one key at a
    /// time and driven by a human, never by a frame.
    fn node_config(&mut self, action: &Value) -> bool {
        let node = action.get("node").and_then(Value::as_u64);
        let key = action.get("key").and_then(Value::as_u64);
        let value = action.get("value").and_then(Value::as_u64);
        let (Some(node), Some(key), Some(value)) = (node, key, value) else {
            return false;
        };
        if node as usize >= NODES || key == 0 || key > 10 || value > u16::MAX as u64 {
            return false;
        }
        self.send_command(
            "node.config.set",
            json!({ "node": node, "key": key, "value": value }),
        );
        true
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
        // Setup lighting, the placed/needed counter and the on-screen target all
        // derive from `start_fen`. Leaving it pointing at the superseded
        // position meant the board kept asking players to build the old one,
        // auto-started on it, and began play in total mismatch.
        if !self.phase.in_play() {
            self.start_fen = fen.to_string();
            self.position = None;
            self.start_cp = 0;
        }
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
        self.config_dirty = true;
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
        self.config_dirty = true;
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
        self.config_dirty = true;
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
        // Written straight down the wire, bypassing the diff — so the painter's
        // record of that half-bar is now wrong and the next frame would skip it,
        // leaving the walking-pixel test stuck on the strip for the rest of the
        // game.
        self.painter.forget();
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
            .unwrap_or(self.rules.autopilot_interval_ms)
            .clamp(500, 60_000);
        self.autopilot = Some(Autopilot {
            interval_ms,
            next_ms: self.now() + interval_ms,
        });
        true
    }

    // ── The clock ────────────────────────────────────────────────────────────

    fn tick(&mut self) {
        let now = self.now();
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
        // `Observer` has no notion of link liveness: a websocket disconnect
        // touches neither `node_online` nor `last_change_ms`, so a frozen
        // snapshot looks exactly like a board holding perfectly still. Without
        // this the countdown runs off a dead link and `begin_play` fingerprints
        // `pol_tag` from a board nobody can see — while the very same broadcast
        // says `sensors_live: false`.
        if !self.sensors_live(now) || self.stream_torn() {
            if self.stable_since.is_some() {
                self.dirty = true;
            }
            self.stable_since = None;
            return;
        }
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
            self.countdown_until = Some(now + self.rules.countdown_ms);
            self.dirty = true;
        }
    }

    fn tick_countdown(&mut self, now: u64) {
        // Losing the board mid-countdown aborts it too — starting a game off a
        // snapshot that stopped updating three seconds ago is worse than asking
        // the operator to press Start.
        if !self.sensors_live(now) || self.stream_torn() {
            self.phase = Phase::Setup;
            self.countdown_until = None;
            self.stable_since = None;
            self.dirty = true;
            return;
        }
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
        // Never infer off a torn delta stream; the pending snapshot is the heal.
        if self.stream_torn() {
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
        let params = self.tun.params();
        let inference = infer::infer(&self.pos, &observation, &params);
        self.on_inference(inference);
    }

    /// Split out from `on_settle` so the outcome handling can be tested without
    /// having to stage a physical board that produces it.
    fn on_inference(&mut self, inference: Inference) {
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
                // After `ask`, not before: `ask` clears any mismatch left over
                // from a previous settle, and this one is the reason it is
                // asking.
                self.ask(
                    "no_match",
                    "The board doesn't match any legal move.".to_string(),
                    options,
                );
                self.mismatch = squares.clone();
            }
        }
        self.update_auto_mask();
        self.dirty = true;
    }

    fn ask(&mut self, kind: &'static str, prompt: String, options: Vec<infer::Candidate>) {
        // A stale mismatch left over from a previous settle makes `compose_frame`
        // paint the board Alert, and the Basic tier can only show one class at a
        // time — so the candidates the prompt is asking about never light up,
        // and two superseded red squares do instead.
        self.mismatch.clear();
        self.choice = Some(Choice {
            kind,
            prompt,
            options,
        });
        self.phase = Phase::AwaitingChoice;
    }

    /// Retires the masks the game applied itself, leaving the operator's alone.
    fn clear_auto_masks(&mut self) {
        for square in std::mem::take(&mut self.auto_masked) {
            self.obs.masked[square as usize] = false;
            self.manual_degraded
                .remove(&format!("sensor_{square}_suspect"));
        }
        self.disagree_streak = [0; SQUARES];
    }

    /// A square that disagrees with expectation across two consecutive settles
    /// is stuck, not surprising. Masking it lets Tier 1 resume cleanly on the
    /// remaining known squares, which beats both prompting on every ply forever
    /// and leaning on Tier 3's weights.
    ///
    /// Only ever during play: during setup a piece on the wrong square looks
    /// identical, and masking it would let a bad build pass.
    fn update_auto_mask(&mut self) {
        // Never while a prompt is open. The whole point of `AwaitingChoice` is
        // that the game position has deliberately not advanced yet, so the
        // pending move's own from/to squares disagree *by construction* — two
        // settles of the operator reading the screen and they would be masked,
        // with a `sensor_N_suspect` chip blaming hardware that is fine. In
        // `suggest` mode, the recommended on-stage fallback, every single ply
        // goes through here.
        if self.phase != Phase::Playing {
            return;
        }
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
            if self.disagree_streak[square] >= self.tun.auto_mask_streak {
                self.obs.masked[square] = true;
                self.auto_masked.insert(square as u8);
                tracing::info!(
                    square,
                    streak = self.disagree_streak[square],
                    "square disagreed repeatedly; masking it"
                );
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
        // A standing nudge means a piece is physically on the wrong square. Only
        // the move that actually touches it can have fixed that, so a commit
        // arriving from anywhere else — a tapped prompt, a manual move,
        // autopilot — must not silently retire the banner and leave the board
        // one piece out of place with nothing on screen saying so.
        self.nudge = match offset {
            Some(offset) => Some(offset),
            None => self.nudge.filter(|standing| {
                let from = m.from().map(u8::from);
                let to = u8::from(m.to());
                from != Some(standing.actual)
                    && from != Some(standing.expected)
                    && to != standing.expected
                    && to != standing.actual
            }),
        };
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
        } else if self.moves.len() >= self.rules.max_ply {
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
        // A quadrant that came back has forgotten everything it was holding, so
        // whatever the painter believes about it is fiction. Tear that up before
        // composing, and the next frame rebuilds it.
        for node in self.obs.take_rejoined() {
            self.painter.node_rejoined(node);
        }
        self.painter.set_palette(self.palette);
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

        // Which edges carry which is a venue decision, not a constant: the eval
        // bar is the narrative device the whole demo hangs on, and it has to be
        // on the edge the audience is actually facing.
        frame.bars_wanted = !matches!(self.phase, Phase::Idle | Phase::Setup);
        let white_to_move = self.pos.turn() == Color::White;
        for &side in &self.eval_sides {
            if (side as usize) < paint::SIDES {
                frame.eval_bar(side as usize, self.eval.win_prob, &self.palette);
            }
        }
        for &side in &self.turn_sides {
            if (side as usize) < paint::SIDES {
                frame.turn_bar(side as usize, white_to_move, &self.palette);
            }
        }
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
        // The delta stream tore and the healing snapshot has not landed. Worth
        // naming: detection is deliberately held off during this window, so
        // "the board stopped responding" needs an honest explanation rather
        // than looking like a hang.
        if self.stream_torn() {
            out.insert("stream_gap".to_string());
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
        let now = self.now();
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
            "max_ply": self.rules.max_ply,
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
            // Current values keyed exactly as `settings` names them, so the
            // admin rail can pair the two without knowing any field names.
            "tunables": config::values(&self.tun, &self.rules),
            // The schema the rail renders itself from. Shipping the ranges means
            // the UI cannot offer a value the server would refuse, and adding a
            // knob is one line in `config::SETTINGS` rather than a change in five
            // layers.
            "settings": config::schema(),
            "palette": {
                "alert": format!("{:06x}", self.palette.alert),
                "needed": format!("{:06x}", self.palette.needed),
                "focus": format!("{:06x}", self.palette.focus),
                "sweep": format!("{:06x}", self.palette.sweep),
                "bar_white": format!("{:06x}", self.palette.bar_white),
                "bar_black": format!("{:06x}", self.palette.bar_black),
            },
            "lighting": {
                "squares": "override",
                "bars_supported": self.painter.bars_supported,
                "bar_map": self.painter.bar_map,
                "eval_sides": self.eval_sides,
                "turn_sides": self.turn_sides,
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
        // Keyed on `(phase, game_seq)`, not phase alone. A clean auto-detected
        // run never leaves `Playing`, so a phase-only key wrote once at the
        // start of the game and then went stale for the rest of it — losing
        // exactly the demo case that matters.
        let key = (self.phase, self.game_seq);
        if self.last_persisted != Some(key) {
            self.last_persisted = Some(key);
            self.persist();
        }
    }

    // ── Restart insurance ────────────────────────────────────────────────────

    /// Written on every phase change, so a server restart or redeploy mid-demo
    /// is not fatal. Without it the rule is simply: do not redeploy during the
    /// demo, and restart the round — it is a five-minute game.
    fn persist(&self) {
        if self.phase == Phase::Idle {
            let _ = std::fs::remove_file(&self.snapshot_path);
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
        if let Err(err) = std::fs::write(&self.snapshot_path, payload.to_string()) {
            tracing::warn!(%err, "could not write the game snapshot");
        }
    }

    fn restore(&mut self) {
        let Ok(text) = std::fs::read_to_string(&self.snapshot_path) else {
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
        // Mapped explicitly. Collapsing setup and countdown into `Playing` meant
        // a crash during setup came back as a game in progress that would never
        // run setup detection or auto-start again.
        self.phase = match saved.get("phase").and_then(Value::as_str) {
            Some("finished") => Phase::Finished,
            Some("idle") | None => Phase::Idle,
            Some("setup") | Some("countdown") => Phase::Setup,
            _ => Phase::Playing,
        };
        self.game_seq += 1;
        self.manual_degraded
            .insert("restored_after_restart".to_string());
        tracing::info!(phase = ?self.phase, plies = self.moves.len(), "restored game after restart");
        if self.phase != Phase::Idle {
            self.request_eval(false);
        }
    }

    // ── Venue profile ────────────────────────────────────────────────────────
    //
    // The runbook budgets ten minutes at the venue to bind the device, square
    // the board up, mask whatever lies and assign the bar map. That is the
    // expensive artifact of the evening — the five-minute puzzle above can
    // simply be replayed. So it is written on every change and read back before
    // the game snapshot, and it lives outside `/tmp`, which does not survive the
    // container being replaced.

    /// Assembles the profile from live state. Nothing here is a second copy: the
    /// observer owns rotation and masks, the painter owns the bar map, and this
    /// is only their serialised view.
    fn profile(&self) -> config::VenueProfile {
        config::VenueProfile {
            device_id: self.device_id.clone(),
            rotation: self.obs.rotation,
            masked: (0..SQUARES)
                .filter(|&sq| self.obs.masked[sq] && !self.auto_masked.contains(&(sq as u8)))
                .map(|sq| sq as u8)
                .collect(),
            bar_map: Some(self.painter.bar_map_slots()),
            detect_mode: Some(self.detect_mode.as_str().to_string()),
            eval_sides: self.eval_sides.clone(),
            turn_sides: self.turn_sides.clone(),
        }
    }

    fn persist_config(&mut self) {
        self.config_dirty = false;
        let file = config::ConfigFile {
            version: config::CONFIG_VERSION,
            tunables: Some(self.tun),
            rules: Some(self.rules),
            palette: Some(self.palette),
            profile: Some(self.profile()),
        };
        let path = self.config_path.clone();
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(&file) {
            Ok(text) => {
                if let Err(err) = std::fs::write(&path, text) {
                    // Not fatal: everything still works for this process, the
                    // calibration just will not outlive it. Say so rather than
                    // letting a restart quietly discard the evening.
                    tracing::warn!(%err, path, "could not write the venue profile");
                    self.manual_degraded
                        .insert("config_not_persisted".to_string());
                    return;
                }
                self.manual_degraded.remove("config_not_persisted");
            }
            Err(err) => tracing::warn!(%err, "could not serialise the venue profile"),
        }
    }

    fn restore_config(&mut self) {
        let path = self.config_path.clone();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        let file: config::ConfigFile = match serde_json::from_str(&text) {
            Ok(file) => file,
            Err(err) => {
                tracing::warn!(%err, path, "venue profile is unreadable; using defaults");
                return;
            }
        };
        if file.version != config::CONFIG_VERSION {
            tracing::warn!(
                found = file.version,
                expected = config::CONFIG_VERSION,
                "venue profile is from another build; using defaults"
            );
            return;
        }
        if let Some(tun) = file.tunables {
            self.tun = tun;
        }
        if let Some(rules) = file.rules {
            self.rules = rules;
        }
        if let Some(palette) = file.palette {
            self.palette = palette;
        }
        // A file edited by hand, or written by a build with different limits,
        // must not be able to smuggle in a value the wire would reject.
        config::clamp_all(&mut self.tun, &mut self.rules);

        if let Some(profile) = file.profile {
            self.device_id = profile.device_id.clone();
            self.obs.set_rotation(profile.rotation);
            self.obs.masked = profile.masked_array();
            if let Some(mode) = profile.detect_mode.as_deref().and_then(DetectMode::parse) {
                self.detect_mode = mode;
            }
            if let Some(map) = profile.bar_map {
                self.painter.set_bar_map_slots(&map);
            }
            if !profile.eval_sides.is_empty() || !profile.turn_sides.is_empty() {
                self.eval_sides = profile.eval_sides;
                self.turn_sides = profile.turn_sides;
            }
            for sq in (0..SQUARES).filter(|&sq| self.obs.masked[sq]) {
                self.manual_degraded.insert(format!("sensor_{sq}_masked"));
            }
        }
        tracing::info!(path, "restored the venue profile");
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

/// The actions a second, unauthenticated tablet at the board may drive when
/// `open_controls` is on. Everything destructive stays admin-only.
fn is_player_action(name: &str) -> bool {
    matches!(
        name,
        "new_game" | "start" | "move" | "choose" | "undo" | "resync"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Every seam this needs already existed: `AppState::new` is trivially
    /// constructible, `EngineHandle` is a bare channel wrapper, and every
    /// deadline takes injected time. The orchestrator was untested by omission,
    /// not by design.
    fn game() -> Game {
        let state = Arc::new(AppState::new("test".to_string(), None));
        let (tx, rx) = mpsc::unbounded_channel::<engine::EvalResult>();
        // Keep the receiver alive so `request` keeps reporting the engine up.
        std::mem::forget(rx);
        let engine = engine::EngineHandle::for_test(mpsc::unbounded_channel().0);
        drop(tx);
        let mut game = Game::new(state, engine);
        game.device_id = Some("test-board".to_string());
        game.obs.node_online = [true; NODES];
        game.obs.have_snapshot = true;
        game
    }

    fn deal(game: &mut Game, fen: &str) {
        assert!(game.new_game(&json!({ "fen": fen })), "position must be legal");
    }

    /// Two consecutive disagreements retire a square. Mutating this threshold to
    /// `>= 1` used to leave the whole suite green.
    #[test]
    fn auto_mask_needs_the_full_streak_and_only_during_play() {
        let mut game = game();
        deal(&mut game, "8/5k2/8/8/8/8/3K4/7R w - - 0 1");
        game.phase = Phase::Playing;

        // e4 reads occupied but the game says it is empty.
        game.obs.occ[28] = observe::Occ::Pos;

        game.update_auto_mask();
        assert_eq!(game.disagree_streak[28], 1);
        assert!(!game.obs.masked[28], "one disagreement is a surprise");

        game.update_auto_mask();
        assert_eq!(game.disagree_streak[28], 2);
        assert!(game.obs.masked[28], "two running is a stuck sensor");
        assert!(game.auto_masked.contains(&28));
        assert!(game.degraded(0).iter().any(|c| c == "sensor_28_suspect"));
    }

    /// While a prompt is open the game position has deliberately not advanced,
    /// so the pending move's own squares disagree by construction. Masking them
    /// blames hardware that is fine — and `suggest` mode routes every ply here.
    #[test]
    fn a_prompt_does_not_auto_mask_the_squares_it_is_asking_about() {
        let mut game = game();
        deal(&mut game, "8/5k2/8/8/8/8/3K4/7R w - - 0 1");
        game.phase = Phase::Playing;
        game.obs.occ[28] = observe::Occ::Pos;

        game.ask("capture", "which one?".into(), Vec::new());
        assert_eq!(game.phase, Phase::AwaitingChoice);
        for _ in 0..6 {
            game.update_auto_mask();
        }
        assert!(
            !game.obs.masked[28],
            "an open prompt must not mask the board out from under itself"
        );
        assert_eq!(game.disagree_streak[28], 0);
    }

    /// A material baseline against a Stockfish final is two different scales
    /// subtracted from each other. Half the shipped deck would flip out of
    /// "draw" on that alone.
    #[test]
    fn a_verdict_across_two_engines_refuses_to_name_a_winner() {
        let mut game = game();
        game.start_cp = 0;

        game.start_source = EvalSource::Material;
        assert_eq!(
            game.verdict_for(400, EvalSource::Stockfish),
            "draw",
            "mixed scales must not crown anybody"
        );
        assert!(game
            .degraded(0)
            .iter()
            .any(|c| c == "verdict_incomparable"));

        // Same ruler at both ends, and the swing is real.
        game.start_source = EvalSource::Stockfish;
        assert_eq!(game.verdict_for(400, EvalSource::Stockfish), "white");
        assert_eq!(game.verdict_for(-400, EvalSource::Stockfish), "black");
        assert_eq!(game.verdict_for(10, EvalSource::Stockfish), "draw");
        assert!(!game
            .degraded(0)
            .iter()
            .any(|c| c == "verdict_incomparable"));

        // An admin decree is comparable with anything: it is the last word.
        game.start_source = EvalSource::Stockfish;
        assert_eq!(game.verdict_for(400, EvalSource::Admin), "white");
    }

    /// The decree is documented as the last word, so a search already in flight
    /// must not land on top of it.
    #[test]
    fn an_in_flight_eval_cannot_overwrite_an_admin_decree() {
        let mut game = game();
        deal(&mut game, "8/5k2/8/8/8/8/3K4/7R w - - 0 1");
        assert!(game.set_eval(&json!({ "cp": 250 })));
        assert_eq!(game.eval.source, EvalSource::Admin);

        game.on_eval(engine::EvalResult {
            game_seq: game.game_seq,
            cp: -30,
            mate: None,
            win_prob: 0.4,
            depth: 12,
            source: EvalSource::Stockfish,
            final_verdict: false,
            available: true,
        });
        assert_eq!(game.eval.cp, 250, "the decree stands");
        assert_eq!(game.eval.source, EvalSource::Admin);
    }

    /// The empty-uci dismissal used to return before the phase guard, so it
    /// could drag Scoring or Finished back into Playing and discard the pending
    /// verdict on the way.
    #[test]
    fn dismissing_a_prompt_cannot_resurrect_a_finished_game() {
        let mut game = game();
        deal(&mut game, "8/5k2/8/8/8/8/3K4/7R w - - 0 1");

        for phase in [
            Phase::Idle,
            Phase::Setup,
            Phase::Countdown,
            Phase::Scoring,
            Phase::Finished,
        ] {
            game.phase = phase;
            assert!(
                !game.action_choose(&json!({ "uci": "" })),
                "{phase:?} must reject a choice"
            );
            assert_eq!(game.phase, phase, "{phase:?} must not be moved");
        }

        game.phase = Phase::AwaitingChoice;
        assert!(game.action_choose(&json!({ "uci": "" })));
        assert_eq!(game.phase, Phase::Playing);
    }

    /// Setup lighting, the placed counter and the on-screen target all derive
    /// from `start_fen`, so overwriting the position has to move it too.
    #[test]
    fn set_fen_during_setup_moves_the_target_as_well() {
        let mut game = game();
        deal(&mut game, "8/5k2/8/8/8/8/3K4/7R w - - 0 1");
        assert_eq!(game.phase, Phase::Setup);

        let corrected = "8/8/4k3/8/8/8/8/4K2R w K - 0 1";
        assert!(game.set_fen(&json!({ "fen": corrected })));
        assert_eq!(
            game.start_fen, corrected,
            "the board must be asked to build the position the game is actually on"
        );
        assert_eq!(engine::fen_of(&game.pos), corrected);
    }

    /// A standing nudge means a piece is physically on the wrong square. Only a
    /// move that touches it can have fixed that.
    #[test]
    fn an_unrelated_commit_keeps_a_standing_nudge() {
        let mut game = game();
        deal(&mut game, "8/5k2/8/8/8/8/3K4/7R w - - 0 1");
        game.phase = Phase::Playing;
        // e4 expected, piece actually sitting on d4.
        game.nudge = Some(Offset {
            expected: 28,
            actual: 27,
        });

        // A king move nowhere near the offset pair.
        assert!(game.commit("d2d3", "manual", Confidence::Certain, None));
        assert!(
            game.nudge.is_some(),
            "the piece is still on the wrong square and the board must keep saying so"
        );

        // Black's reply is unrelated too.
        assert!(game.commit("f7f6", "manual", Confidence::Certain, None));
        assert!(game.nudge.is_some());

        // A move that actually lands on the nudged square resolves it: h5 is 39.
        game.nudge = Some(Offset {
            expected: 39,
            actual: 38,
        });
        assert!(game.commit("h1h5", "manual", Confidence::Certain, None));
        assert!(game.nudge.is_none(), "the board no longer has anything to ask for");
    }

    /// A stale mismatch makes the Basic tier paint the board Alert, which
    /// suppresses the very candidates the prompt is asking about.
    #[test]
    fn opening_a_prompt_clears_the_previous_mismatch() {
        let mut game = game();
        deal(&mut game, "8/5k2/8/8/8/8/3K4/7R w - - 0 1");
        game.phase = Phase::Playing;
        game.mismatch = vec![12, 13];
        game.ask("capture", "which one?".into(), Vec::new());
        assert!(game.mismatch.is_empty());
    }

    /// ...but a `no_match` prompt must still carry the squares it is complaining
    /// about, so the clear above has to happen *before* they are assigned.
    #[test]
    fn a_no_match_prompt_keeps_the_squares_it_is_about() {
        let mut game = game();
        deal(&mut game, "8/5k2/8/8/8/8/3K4/7R w - - 0 1");
        game.phase = Phase::Playing;

        game.on_inference(infer::Inference::Mismatch {
            squares: vec![19, 42],
            options: Vec::new(),
        });
        assert_eq!(game.phase, Phase::AwaitingChoice);
        assert_eq!(
            game.mismatch,
            vec![19, 42],
            "the board has to light the squares the prompt is asking about"
        );
    }

    /// The expensive artifact of the evening is the calibration, not the
    /// five-minute puzzle. This round trip is what F5 was about.
    #[test]
    fn the_venue_profile_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("arcade-cfg-{}", crate::util::random_hex()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("venue.json");

        {
            let mut game = game();
            game.config_path = path.display().to_string();
            game.obs.set_rotation(2);
            assert!(game.mask_square(&json!({ "square": 27, "masked": true })));
            assert!(game.set_detect(&json!({ "mode": "suggest" })));
            assert!(game.bars_sides(&json!({ "eval_sides": [1, 3], "turn_sides": [0, 2] })));
            assert!(game.set_config(&json!({ "key": "settle_ms", "value": 1250 })));
            assert!(game.bind_device(&json!({ "device_id": "arcade-chess-007" })));
            game.persist_config();
        }

        let mut fresh = Game::new(
            Arc::new(AppState::new("test".to_string(), None)),
            engine::EngineHandle::for_test(mpsc::unbounded_channel().0),
        );
        fresh.config_path = path.display().to_string();
        fresh.restore_config();

        assert_eq!(fresh.obs.rotation, 2, "board mounting survives");
        assert!(fresh.obs.masked[27], "the lying sensor stays retired");
        assert_eq!(fresh.detect_mode, DetectMode::Suggest);
        assert_eq!(fresh.eval_sides, vec![1, 3]);
        assert_eq!(fresh.turn_sides, vec![0, 2]);
        assert_eq!(fresh.tun.settle_ms, 1250);
        assert_eq!(fresh.device_id.as_deref(), Some("arcade-chess-007"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file written by hand or by an older build must not smuggle in a value
    /// the wire itself would refuse.
    #[test]
    fn a_hand_edited_profile_is_clamped_on_the_way_in() {
        let dir = std::env::temp_dir().join(format!("arcade-cfg-{}", crate::util::random_hex()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("venue.json");
        std::fs::write(
            &path,
            json!({
                "version": config::CONFIG_VERSION,
                "tunables": {
                    "settle_ms": 0,
                    "autostart_stable_ms": 1500,
                    "unknown_tolerance": 0,
                    "tier3_max_distance": 9999.0,
                    "tier3_margin": 1.0,
                    "tier3_neighbour_credit": 0.5,
                    "tier3_unreadable_penalty": 2.0,
                    "tier3_empty_penalty": 1.0,
                    "tier3_polarity_credit": 0.5,
                    "auto_mask_streak": 2
                }
            })
            .to_string(),
        )
        .unwrap();

        let mut game = game();
        game.config_path = path.display().to_string();
        game.restore_config();
        assert_eq!(game.tun.settle_ms, 100, "clamped to the advertised floor");
        assert_eq!(game.tun.tier3_max_distance, 1.75, "clamped to the ceiling");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A clean auto-detected run never leaves `Playing`, so keying the snapshot
    /// on phase alone wrote once and then went stale for the whole game.
    #[test]
    fn the_game_snapshot_is_rewritten_every_ply() {
        let dir = std::env::temp_dir().join(format!("arcade-snap-{}", crate::util::random_hex()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("game.json");

        let mut game = game();
        game.snapshot_path = path.display().to_string();
        deal(&mut game, "8/5k2/8/8/8/8/3K4/7R w - - 0 1");
        game.phase = Phase::Playing;
        game.publish();

        assert!(game.commit("h1h5", "manual", Confidence::Certain, None));
        game.publish();
        let after_one: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(after_one["moves"].as_array().unwrap().len(), 1);

        assert!(game.commit("f7f6", "manual", Confidence::Certain, None));
        game.publish();
        let after_two: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            after_two["moves"].as_array().unwrap().len(),
            2,
            "the second ply must reach disk too, without a phase change to trigger it"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A crash during setup used to come back as a game in progress that would
    /// never run setup detection again.
    #[test]
    fn a_restart_during_setup_comes_back_in_setup() {
        let dir = std::env::temp_dir().join(format!("arcade-snap-{}", crate::util::random_hex()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("game.json");

        let mut first = game();
        first.snapshot_path = path.display().to_string();
        deal(&mut first, "8/5k2/8/8/8/8/3K4/7R w - - 0 1");
        assert_eq!(first.phase, Phase::Setup);
        first.publish();

        let mut fresh = game();
        fresh.snapshot_path = path.display().to_string();
        fresh.restore();
        assert_eq!(fresh.phase, Phase::Setup, "still waiting for the board");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Auto-masks are evidence about a position that no longer exists.
    #[test]
    fn a_new_game_retires_auto_masks_but_keeps_operator_masks() {
        let mut game = game();
        deal(&mut game, "8/5k2/8/8/8/8/3K4/7R w - - 0 1");
        assert!(game.mask_square(&json!({ "square": 10, "masked": true })));
        game.obs.masked[42] = true;
        game.auto_masked.insert(42);

        deal(&mut game, "8/5k2/8/8/8/8/3K4/7R w - - 0 1");
        assert!(game.obs.masked[10], "the operator's call stands");
        assert!(!game.obs.masked[42], "the game's own guess does not");
        assert!(game.auto_masked.is_empty());
    }

    /// Auto-start off a frozen snapshot fingerprints polarity from a board
    /// nobody can see.
    #[test]
    fn a_dead_link_cannot_auto_start_a_game() {
        let mut game = game();
        deal(&mut game, "8/5k2/8/8/8/8/3K4/7R w - - 0 1");
        // Build the position perfectly.
        for sq in infer::occupancy(&game.pos)
            .iter()
            .enumerate()
            .filter(|(_, &o)| o)
            .map(|(sq, _)| sq)
        {
            game.obs.occ[sq] = observe::Occ::Pos;
        }
        game.obs.last_change_ms = 0;
        game.obs.last_event_ms = 0;

        // Far past both the settle window and the stability hold, but the device
        // stopped talking a long time ago.
        let now = SENSOR_STALE_MS + 10_000;
        game.tick_setup(now);
        game.tick_setup(now + 1);
        assert_eq!(
            game.phase,
            Phase::Setup,
            "a frozen snapshot is not a board holding still"
        );
        assert!(game.degraded(now).iter().any(|c| c == "sensors_stale"));
    }

    /// Nothing may be inferred off a torn delta stream.
    #[test]
    fn a_sequence_gap_holds_detection_until_the_snapshot_heals_it() {
        let mut game = game();
        deal(&mut game, "8/5k2/8/8/8/8/3K4/7R w - - 0 1");
        game.phase = Phase::Playing;

        game.on_device_event(
            "test-board",
            &json!({ "type": "sensor.changed", "data": { "square": 27, "state": "positive" } }),
            true,
        );
        assert!(game.stream_torn());
        assert!(game.degraded(0).iter().any(|c| c == "stream_gap"));

        game.on_device_event(
            "test-board",
            &json!({
                "type": "board.snapshot",
                "data": { "squares": vec![0; 64], "valid": vec![true; 64], "online_node_mask": 0b1111 }
            }),
            false,
        );
        assert!(!game.stream_torn(), "the snapshot is the heal");
    }

    /// The unauthenticated tablet may play, but must not be able to rewrite the
    /// calibration or decree a winner.
    #[test]
    fn open_controls_do_not_open_the_destructive_actions() {
        for name in ["new_game", "start", "move", "choose", "undo", "resync"] {
            assert!(is_player_action(name), "{name} should be player-facing");
        }
        for name in [
            "set_config",
            "set_tunables",
            "set_eval",
            "set_fen",
            "end",
            "abort",
            "bind_device",
            "mask_square",
            "set_rotation",
            "bars_map",
            "node_config",
            "set_palette",
        ] {
            assert!(!is_player_action(name), "{name} must stay admin-only");
        }
    }

    /// The rail pairs `settings` with `tunables` by key, so the two have to
    /// agree in every broadcast.
    #[test]
    fn the_broadcast_carries_a_schema_that_matches_its_values() {
        let game = game();
        let view = game.view();
        let settings = view["settings"].as_array().expect("settings array");
        let values = view["tunables"].as_object().expect("tunables object");
        assert!(!settings.is_empty());
        for spec in settings {
            let key = spec["key"].as_str().unwrap();
            assert!(values.contains_key(key), "{key} advertised but not valued");
            for field in ["min", "max", "step", "label", "group", "kind"] {
                assert!(!spec[field].is_null(), "{key} is missing {field}");
            }
        }
    }
}
