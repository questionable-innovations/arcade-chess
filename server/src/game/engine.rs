//! Stockfish over UCI, with a material count underneath it.
//!
//! Two rules shape this module:
//!
//! - **Nothing blocks.** The game task never awaits the engine. It posts a
//!   request and gets a result back through the same channel every other input
//!   arrives on, so a wedged engine costs latency on the eval bar and nothing
//!   else.
//! - **Never silently lie about where a number came from.** Every result
//!   carries its `source`, and the UI renders it as a badge. A material count
//!   labelled "stockfish" is worse than no eval at all.

use std::process::Stdio;
use std::time::Duration;

use shakmaty::fen::Fen;
use shakmaty::{CastlingMode, Chess, Color, Position, Role};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::mpsc;

/// Per-ply search. Long enough to be right about the simplified boards puzzle
/// mode deals — eight to sixteen pieces — short enough that the bar tracks the
/// game rather than trailing it. This is the one tunable that genuinely gets
/// shallower as the decks grow: if the bar starts disagreeing with the final
/// verdict on the busier positions, raise this before suspecting anything else.
pub const PLY_MOVETIME_MS: u64 = 400;
/// The final verdict is the one number the whole game is judged on.
pub const FINAL_MOVETIME_MS: u64 = 2000;
/// Past this the child is presumed wedged, killed, and respawned.
const SEARCH_TIMEOUT: Duration = Duration::from_secs(3);
const BACKOFF_MIN: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// Mate scores clamp to this for display, so the bar stays a bar.
const MATE_DISPLAY_CP: i32 = 1500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvalSource {
    Stockfish,
    Material,
    Admin,
    /// Nothing has been evaluated yet. Distinct from `Material` so a bar that
    /// has never been given a number does not wear a badge claiming one was
    /// counted.
    Unknown,
}

impl EvalSource {
    pub fn as_str(self) -> &'static str {
        match self {
            EvalSource::Stockfish => "stockfish",
            EvalSource::Material => "material",
            EvalSource::Admin => "admin",
            EvalSource::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug)]
pub struct EvalRequest {
    pub fen: String,
    /// The game generation this was asked at. An eval for a position that has
    /// since been undone is dropped on arrival rather than displayed.
    pub game_seq: u64,
    pub movetime_ms: u64,
    pub final_verdict: bool,
}

#[derive(Clone, Debug)]
pub struct EvalResult {
    pub game_seq: u64,
    /// Centipawns, **white POV**. UCI reports side-to-move POV; normalising is
    /// the classic sign bug in this kind of code, so it is done once, here,
    /// and unit-tested.
    pub cp: i32,
    pub mate: Option<i32>,
    pub win_prob: f64,
    pub depth: u32,
    pub source: EvalSource,
    pub final_verdict: bool,
    /// False when the engine could not answer at all. The game task then scores
    /// the position itself rather than displaying a zero that means nothing.
    pub available: bool,
}

/// Lichess's win-probability curve. Used for the tug-of-war bar so screen and
/// hardware agree on where the middle is.
pub fn win_probability(cp: i32) -> f64 {
    let p = 0.5 + 0.5 * (2.0 / (1.0 + (-0.003_682_08 * cp as f64).exp()) - 1.0);
    p.clamp(0.0, 1.0)
}

/// Material balance in centipawns, white POV. The eval of last resort — always
/// available, always labelled.
pub fn material_cp(pos: &Chess) -> i32 {
    let value = |role: Role| match role {
        Role::Pawn => 100,
        Role::Knight => 320,
        Role::Bishop => 330,
        Role::Rook => 500,
        Role::Queen => 900,
        Role::King => 0,
    };
    let board = pos.board();
    let mut total = 0;
    for square in board.occupied() {
        if let Some(piece) = board.piece_at(square) {
            let v = value(piece.role);
            total += if piece.color == Color::White { v } else { -v };
        }
    }
    total
}

pub fn material_eval(pos: &Chess, game_seq: u64, final_verdict: bool) -> EvalResult {
    let cp = material_cp(pos);
    EvalResult {
        game_seq,
        cp,
        mate: None,
        win_prob: win_probability(cp),
        depth: 0,
        source: EvalSource::Material,
        final_verdict,
        available: true,
    }
}

#[derive(Clone)]
pub struct EngineHandle {
    tx: mpsc::UnboundedSender<EvalRequest>,
}

impl EngineHandle {
    /// Posts a request. Returns false when the engine task is gone, which is
    /// the caller's cue to fall back to material immediately rather than wait
    /// for a result that will never arrive.
    pub fn request(&self, request: EvalRequest) -> bool {
        self.tx.send(request).is_ok()
    }

    /// A handle wired to a caller-owned channel, for driving the game task in
    /// tests without spawning Stockfish.
    #[cfg(test)]
    pub fn for_test(tx: mpsc::UnboundedSender<EvalRequest>) -> EngineHandle {
        EngineHandle { tx }
    }
}

/// Spawns the engine task. `results` is the game task's own input channel, so
/// an eval arrives the same way a sensor event does and needs no lock.
pub fn spawn(results: mpsc::UnboundedSender<EvalResult>) -> EngineHandle {
    let (tx, rx) = mpsc::unbounded_channel::<EvalRequest>();
    tokio::spawn(run(rx, results));
    EngineHandle { tx }
}

fn engine_path() -> String {
    std::env::var("STOCKFISH_PATH")
        .ok()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| {
            // Debian's package installs here; a PATH lookup covers everything
            // else, including a developer's Homebrew build.
            if std::path::Path::new("/usr/games/stockfish").exists() {
                "/usr/games/stockfish".to_string()
            } else {
                "stockfish".to_string()
            }
        })
}

struct Engine {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Engine {
    async fn spawn() -> std::io::Result<Engine> {
        let path = engine_path();
        let mut child = Command::new(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        let mut engine = Engine {
            child,
            stdin,
            stdout,
        };
        engine.send("uci").await?;
        engine.wait_for("uciok").await?;
        engine.send("setoption name UCI_ShowWDL value true").await?;
        engine.send("setoption name Threads value 1").await?;
        engine.send("setoption name Hash value 64").await?;
        engine.send("isready").await?;
        engine.wait_for("readyok").await?;
        tracing::info!(engine = %path, "stockfish ready");
        Ok(engine)
    }

    async fn send(&mut self, line: &str) -> std::io::Result<()> {
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await
    }

    async fn wait_for(&mut self, token: &str) -> std::io::Result<()> {
        let mut line = String::new();
        loop {
            line.clear();
            if self.stdout.read_line(&mut line).await? == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!("engine closed while waiting for {token}"),
                ));
            }
            if line.trim_end() == token {
                return Ok(());
            }
        }
    }

    async fn analyse(&mut self, fen: &str, movetime_ms: u64) -> std::io::Result<Info> {
        self.send("ucinewgame").await?;
        self.send("isready").await?;
        self.wait_for("readyok").await?;
        self.send(&format!("position fen {fen}")).await?;
        self.send(&format!("go movetime {movetime_ms}")).await?;

        let mut best = Info::default();
        let mut line = String::new();
        loop {
            line.clear();
            if self.stdout.read_line(&mut line).await? == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "engine closed mid-search",
                ));
            }
            if line.starts_with("bestmove") {
                break;
            }
            if let Some(info) = parse_info(&line) {
                best = info;
            }
        }
        if best.cp.is_none() && best.mate.is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "engine reported no score",
            ));
        }
        Ok(best)
    }

    async fn kill(mut self) {
        let _ = self.send("quit").await;
        let _ = self.child.kill().await;
    }
}

async fn run(
    mut requests: mpsc::UnboundedReceiver<EvalRequest>,
    results: mpsc::UnboundedSender<EvalResult>,
) {
    let mut engine: Option<Engine> = None;
    let mut backoff = BACKOFF_MIN;
    // Wall-clock guard so a hard-down engine is not respawned per request.
    let mut retry_at = tokio::time::Instant::now();

    while let Some(request) = requests.recv().await {
        // Only the newest request matters: the bar shows one number, and the
        // positions behind a backlog have already been superseded.
        let mut request = request;
        while let Ok(next) = requests.try_recv() {
            request = next;
        }

        if engine.is_none() {
            if tokio::time::Instant::now() < retry_at {
                let _ = results.send(unavailable(&request));
                continue;
            }
            // Bounded exactly like the search below it. `spawn` waits for
            // `uciok` and `readyok` in an unbounded read loop that exits only on
            // the token or on EOF — so a binary that execs, stays alive and
            // never speaks UCI wedges this task forever. The channel stays open,
            // so `request` keeps returning true, the material fallback never
            // fires, `eval.status` stays `pending` for the rest of the night,
            // and the state machine deadlocks the first time it reaches Scoring.
            // `STOCKFISH_PATH` exists precisely to be repointed at the venue,
            // which is exactly when a wrong binary gets pointed at.
            match tokio::time::timeout(SEARCH_TIMEOUT, Engine::spawn())
                .await
                .unwrap_or_else(|_| {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "engine did not answer the UCI handshake",
                    ))
                }) {
                Ok(started) => {
                    engine = Some(started);
                    backoff = BACKOFF_MIN;
                }
                Err(err) => {
                    tracing::warn!(%err, "could not start stockfish; eval falls back to material");
                    retry_at = tokio::time::Instant::now() + backoff;
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                    let _ = results.send(unavailable(&request));
                    continue;
                }
            }
        }

        let running = engine.as_mut().expect("engine present");
        let analysis =
            tokio::time::timeout(SEARCH_TIMEOUT, running.analyse(&request.fen, request.movetime_ms))
                .await;

        match analysis {
            Ok(Ok(info)) => {
                let white_to_move = side_to_move_is_white(&request.fen);
                let _ = results.send(to_result(&request, &info, white_to_move));
            }
            Ok(Err(err)) => {
                tracing::warn!(%err, "stockfish failed; respawning");
                if let Some(dead) = engine.take() {
                    dead.kill().await;
                }
                retry_at = tokio::time::Instant::now() + backoff;
                backoff = (backoff * 2).min(BACKOFF_MAX);
                let _ = results.send(unavailable(&request));
            }
            Err(_) => {
                tracing::warn!(
                    movetime_ms = request.movetime_ms,
                    "stockfish search timed out; respawning"
                );
                if let Some(dead) = engine.take() {
                    dead.kill().await;
                }
                retry_at = tokio::time::Instant::now() + backoff;
                backoff = (backoff * 2).min(BACKOFF_MAX);
                let _ = results.send(unavailable(&request));
            }
        }
    }

    if let Some(dead) = engine.take() {
        dead.kill().await;
    }
}

/// The engine could not answer. The game task turns this into a material eval
/// against its own position — the engine task holds no board of its own.
fn unavailable(request: &EvalRequest) -> EvalResult {
    EvalResult {
        game_seq: request.game_seq,
        cp: 0,
        mate: None,
        win_prob: 0.5,
        depth: 0,
        source: EvalSource::Material,
        final_verdict: request.final_verdict,
        available: false,
    }
}

fn side_to_move_is_white(fen: &str) -> bool {
    fen.split_whitespace().nth(1) != Some("b")
}

#[derive(Default, Debug, Clone, Copy)]
struct Info {
    cp: Option<i32>,
    mate: Option<i32>,
    depth: u32,
    wdl: Option<[u32; 3]>,
}

/// Pulls `score cp|mate`, `depth` and `wdl` out of one info line. Cribbed from
/// `position-miner/src/verify.rs`, minus the MultiPV bookkeeping puzzle mode
/// has no use for.
fn parse_info(line: &str) -> Option<Info> {
    if !line.starts_with("info ") {
        return None;
    }
    let mut tokens = line.split_ascii_whitespace();
    let mut info = Info::default();
    while let Some(token) = tokens.next() {
        match token {
            "depth" => info.depth = tokens.next()?.parse().unwrap_or(0),
            "score" => match tokens.next()? {
                "cp" => info.cp = tokens.next()?.parse().ok(),
                "mate" => info.mate = tokens.next()?.parse().ok(),
                _ => {}
            },
            "wdl" => {
                let w = tokens.next()?.parse().ok()?;
                let d = tokens.next()?.parse().ok()?;
                let l = tokens.next()?.parse().ok()?;
                info.wdl = Some([w, d, l]);
            }
            _ => {}
        }
    }
    if info.cp.is_none() && info.mate.is_none() {
        return None;
    }
    Some(info)
}

/// Normalises one search to white POV and derives the bar's win probability.
/// Both UCI scores and UCI WDL are side-to-move POV, so both flip together.
fn to_result(request: &EvalRequest, info: &Info, white_to_move: bool) -> EvalResult {
    let flip = |v: i32| if white_to_move { v } else { -v };
    let (cp, mate) = match (info.cp, info.mate) {
        (_, Some(plies)) => {
            let magnitude = if plies < 0 { -MATE_DISPLAY_CP } else { MATE_DISPLAY_CP };
            (flip(magnitude), Some(flip(plies)))
        }
        (Some(cp), None) => (flip(cp), None),
        (None, None) => (0, None),
    };
    // Prefer the engine's own WDL when it reports one: it already accounts for
    // material scaling, and the two curves agree closely enough that the bar
    // does not jump when a source changes mid-game.
    let win_prob = match info.wdl {
        Some([w, d, _]) if mate.is_none() => {
            let stm = (w as f64 + d as f64 / 2.0) / 1000.0;
            if white_to_move {
                stm
            } else {
                1.0 - stm
            }
        }
        _ => win_probability(cp),
    };
    EvalResult {
        game_seq: request.game_seq,
        cp,
        mate,
        win_prob: win_prob.clamp(0.0, 1.0),
        depth: info.depth,
        source: EvalSource::Stockfish,
        final_verdict: request.final_verdict,
        available: true,
    }
}

/// Which way the game swung, white POV. **The swing, not the absolute** — the
/// miner keeps anything inside ±40 cp (`position-miner/src/verify.rs`), so a
/// dealt position can legitimately sit at +35 before anyone touches it. Judging
/// the absolute means two players can shuffle for five moves, change nothing,
/// and be told White won, with no way to explain it.
pub fn verdict(start_cp: i32, final_cp: i32, draw_band_cp: i32) -> &'static str {
    let swing = final_cp - start_cp;
    if swing >= draw_band_cp {
        "white"
    } else if swing <= -draw_band_cp {
        "black"
    } else {
        "draw"
    }
}

pub fn fen_of(pos: &Chess) -> String {
    Fen::from_position(pos.clone(), shakmaty::EnPassantMode::Legal).to_string()
}

pub fn position_of(fen: &str) -> Option<Chess> {
    fen.parse::<Fen>()
        .ok()?
        .into_position(CastlingMode::Standard)
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(final_verdict: bool) -> EvalRequest {
        EvalRequest {
            fen: String::new(),
            game_seq: 7,
            movetime_ms: 400,
            final_verdict,
        }
    }

    #[test]
    fn parses_score_depth_and_wdl() {
        let info =
            parse_info("info depth 18 seldepth 24 score cp -34 wdl 121 795 84 nodes 9 pv e2e4")
                .expect("parsed");
        assert_eq!(info.cp, Some(-34));
        assert_eq!(info.depth, 18);
        assert_eq!(info.wdl, Some([121, 795, 84]));
        assert!(parse_info("info depth 1 currmove e2e4").is_none());
        assert!(parse_info("bestmove e2e4").is_none());
    }

    /// UCI reports side-to-move POV. Getting this backwards is the classic sign
    /// bug: the bar would swing the wrong way on every black move.
    #[test]
    fn scores_normalise_to_white_pov() {
        let info = Info {
            cp: Some(120),
            ..Default::default()
        };
        assert_eq!(to_result(&request(false), &info, true).cp, 120);
        assert_eq!(
            to_result(&request(false), &info, false).cp,
            -120,
            "black being 120 up is white being 120 down"
        );
    }

    #[test]
    fn wdl_flips_with_the_side_to_move() {
        let info = Info {
            cp: Some(0),
            wdl: Some([800, 150, 50]),
            ..Default::default()
        };
        let white = to_result(&request(false), &info, true).win_prob;
        let black = to_result(&request(false), &info, false).win_prob;
        assert!((white - 0.875).abs() < 1e-9);
        assert!((black - 0.125).abs() < 1e-9);
    }

    #[test]
    fn mate_clamps_for_display_and_keeps_its_sign() {
        let info = Info {
            mate: Some(3),
            ..Default::default()
        };
        let white = to_result(&request(true), &info, true);
        assert_eq!(white.cp, MATE_DISPLAY_CP);
        assert_eq!(white.mate, Some(3));
        let black = to_result(&request(true), &info, false);
        assert_eq!(black.cp, -MATE_DISPLAY_CP);
        assert_eq!(black.mate, Some(-3));
    }

    #[test]
    fn win_curve_hits_the_endpoints_and_the_middle() {
        assert!((win_probability(0) - 0.5).abs() < 1e-9);
        assert!(win_probability(2000) > 0.99);
        assert!(win_probability(-2000) < 0.01);
        assert!(win_probability(100) > 0.5 && win_probability(100) < 0.6);
    }

    #[test]
    fn material_counts_from_whites_point_of_view() {
        let even = position_of("8/6k1/8/8/8/8/1R4K1/1r6 w - - 0 1").expect("legal");
        assert_eq!(material_cp(&even), 0);
        let a_rook_up = position_of("8/6k1/8/8/8/8/1R4K1/8 w - - 0 1").expect("legal");
        assert_eq!(material_cp(&a_rook_up), 500);
    }

    /// The mining band is ±40 cp, so a dealt position can sit at +35 with
    /// nobody having done anything. Judging the swing is what makes the result
    /// explainable on stage.
    #[test]
    fn the_verdict_judges_the_swing_not_the_absolute() {
        assert_eq!(verdict(35, 40, 40), "draw", "nobody changed anything");
        assert_eq!(verdict(35, 200, 40), "white");
        assert_eq!(verdict(35, -20, 40), "black");
        assert_eq!(verdict(0, 40, 40), "white", "the band edge counts");
        assert_eq!(verdict(0, 39, 40), "draw");
    }

    #[test]
    fn side_to_move_is_read_off_the_fen() {
        assert!(side_to_move_is_white("8/8/8/8/8/8/8/8 w - - 0 1"));
        assert!(!side_to_move_is_white("8/8/8/8/8/8/8/8 b - - 0 1"));
    }
}
