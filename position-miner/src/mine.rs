//! Stage 1: stream a lichess `.pgn.zst` dump and pull out candidate positions.
//!
//! The stream is never materialised — a zstd frame decoder feeds a
//! `pgn-reader` visitor, which replays each game move by move and tests the
//! live board. That means the same code works on a 10 GB monthly dump, on a
//! partially-downloaded prefix of one, or on `curl … | arcpos mine -`.
//!
//! What counts as a candidate is described in `Filters`.

use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use pgn_reader::{BufferedReader, RawComment, RawHeader, SanPlus, Skip, Visitor};
use shakmaty::{Chess, Position};
use std::collections::HashSet;
use std::io::{self, Read};
use std::path::Path;

use crate::format::{
    game_id_field, name_field, Record, Termination, Writer, MATE_BASE, REC_FLAG_MATE,
};
use crate::position::{pack_position, position_id, Packed};

/// Everything that decides whether a position is worth storing.
///
/// The concept these filters encode: *balanced but not drawish*. A near-zero
/// engine eval on its own overwhelmingly selects dead-drawn endings, so the
/// game must also have been **won by somebody**, on the board rather than on
/// the clock. That pairing — the engine says level, yet a human converted it —
/// is the signal we are mining for.
#[derive(Debug, Clone)]
pub struct Filters {
    /// Exact number of pieces on the board, kings included.
    pub pieces: u32,
    /// Keep positions whose lichess eval is within ±this many centipawns.
    pub eval_band_cp: i16,
    /// How many consecutive plies (ending at this one) must sit inside the
    /// band. 1 accepts a momentary crossing; 2+ rejects mid-tactic noise.
    pub stable_plies: u16,
    /// Both players must be at least this strong.
    pub min_elo: u16,
    /// The game must run at least this many more plies after the position, so
    /// the result was actually played out from here.
    pub min_remaining_plies: u16,
    /// Reject games decided by the clock, not the board.
    pub require_normal_termination: bool,
    /// Keep drawn games. Off by default — a draw tells us nothing about
    /// whether the position had winning chances.
    pub allow_draws: bool,
    /// Require the two sides' non-king material to differ in composition
    /// (R vs B+P and friends). Symmetric material is where the dead draws
    /// concentrate.
    pub require_imbalance: bool,
    /// Keep at most this many positions from any one game. A long balanced
    /// ending offers a dozen near-identical consecutive plies; without a cap
    /// a handful of games would swamp the file.
    pub per_game: usize,
    /// When `per_game > 1`, kept positions from the same game must be at
    /// least this many plies apart.
    pub min_gap_plies: u16,
    /// Stop after this many records. 0 = unlimited.
    pub limit: u64,
}

impl Default for Filters {
    fn default() -> Self {
        Filters {
            pieces: 8,
            eval_band_cp: 30,
            stable_plies: 2,
            min_elo: 1800,
            min_remaining_plies: 10,
            require_normal_termination: true,
            allow_draws: false,
            require_imbalance: false,
            per_game: 1,
            min_gap_plies: 8,
            limit: 0,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    pub games_seen: u64,
    pub games_scanned: u64,
    pub games_with_eval: u64,
    pub positions_at_size: u64,
    pub candidates: u64,
    pub written: u64,
    pub duplicates: u64,
    pub rejected_draw: u64,
    pub rejected_termination: u64,
    pub rejected_elo: u64,
    pub rejected_no_eval: u64,
    pub rejected_band: u64,
    pub rejected_unstable: u64,
    pub rejected_tail: u64,
    pub rejected_symmetric: u64,
    pub rejected_same_game: u64,
}

/// Runs stage 1 over `input`, appending records to `out`.
pub fn run(input: &Path, out: &Path, filters: &Filters, progress: bool) -> Result<Stats> {
    let (reader, bar) = open_input(input, progress)?;
    let decoder = zstd::Decoder::new(reader)
        .context("initialising zstd decoder (is the input a .pgn.zst?)")?;

    let mut writer = Writer::create(out)?;
    let mut visitor = MineVisitor::new(filters, &mut writer, bar.clone());

    let mut buffered = BufferedReader::new(decoder);
    let outcome = buffered.read_all(&mut visitor);

    let stats = visitor.stats;
    match outcome {
        Ok(()) => {}
        Err(e) if is_truncation(&e) => {
            // Expected when mining a dump that is still downloading.
            bar.println(format!(
                "input ended mid-stream ({e}); keeping the {} records mined so far",
                stats.written
            ));
        }
        Err(e) => return Err(e).context("reading PGN stream"),
    }

    writer.finish()?;
    bar.finish_and_clear();
    Ok(stats)
}

/// Distinguishes "the input stopped early" from "the input is broken".
///
/// Mining a dump that is still downloading is a supported workflow, so a
/// stream that simply ends mid-frame is not an error. Genuine corruption must
/// still propagate — otherwise a damaged dump would quietly yield a short file
/// that looks like a successful run.
fn is_truncation(e: &io::Error) -> bool {
    if e.kind() == io::ErrorKind::UnexpectedEof {
        return true;
    }
    // The zstd bindings report a short frame through a generic error kind, so
    // the message is the only signal available.
    let text = e.to_string().to_ascii_lowercase();
    text.contains("unexpected end") || text.contains("truncated")
}

fn open_input(path: &Path, progress: bool) -> Result<(Box<dyn Read>, ProgressBar)> {
    if path.as_os_str() == "-" {
        let bar = spinner(progress);
        return Ok((Box::new(io::stdin()), bar));
    }
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let total = file.metadata()?.len();
    let bar = if progress {
        let bar = ProgressBar::new(total);
        bar.set_style(
            ProgressStyle::with_template(
                "{bar:32} {bytes}/{total_bytes} ({eta}) {msg}",
            )
            .unwrap(),
        );
        bar
    } else {
        ProgressBar::hidden()
    };
    Ok((Box::new(CountingReader::new(file, bar.clone())), bar))
}

fn spinner(progress: bool) -> ProgressBar {
    if !progress {
        return ProgressBar::hidden();
    }
    let bar = ProgressBar::new_spinner();
    bar.set_style(ProgressStyle::with_template("{spinner} {msg}").unwrap());
    bar
}

struct CountingReader<R> {
    inner: R,
    bar: ProgressBar,
}

impl<R: Read> CountingReader<R> {
    fn new(inner: R, bar: ProgressBar) -> Self {
        CountingReader { inner, bar }
    }
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bar.inc(n as u64);
        Ok(n)
    }
}

// ---------------------------------------------------------------------------
// Per-game header state
// ---------------------------------------------------------------------------

#[derive(Default)]
struct GameHeaders {
    game_id: [u8; 8],
    white: [u8; 20],
    black: [u8; 20],
    white_elo: u16,
    black_elo: u16,
    /// 0 = white won, 1 = black won, 2 = draw, 3 = unknown.
    result: u8,
    utc_date: Option<(i32, u32, u32)>,
    utc_time: Option<(u32, u32, u32)>,
    tc_initial: u16,
    tc_increment: u16,
    termination: Termination,
    non_standard: bool,
}

impl GameHeaders {
    fn unix_time(&self) -> i64 {
        let (y, m, d) = match self.utc_date {
            Some(v) => v,
            None => return 0,
        };
        let (hh, mm, ss) = self.utc_time.unwrap_or((0, 0, 0));
        days_from_civil(y, m, d) * 86_400 + (hh as i64) * 3600 + (mm as i64) * 60 + ss as i64
    }
}

/// Howard Hinnant's `days_from_civil`: civil date to days since 1970-01-01.
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let d = d as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// A position that passed the board-level tests and is waiting on the
/// game-level ones (which are only known once the game ends).
struct Candidate {
    packed: Packed,
    halfmove: u8,
    fullmove: u16,
    ply: u16,
    eval_cp: i16,
    mate: bool,
}

struct MineVisitor<'a> {
    filters: &'a Filters,
    writer: &'a mut Writer,
    bar: ProgressBar,
    stats: Stats,
    seen_ids: HashSet<u64>,

    pos: Chess,
    ply: u16,
    skip_current: bool,
    headers: GameHeaders,
    game_had_eval: bool,
    /// Position reached by the most recent move, awaiting its `[%eval]`.
    awaiting_eval: Option<PendingPosition>,
    /// How many consecutive plies up to and including the last one had an
    /// in-band eval.
    in_band_run: u16,
    candidates: Vec<Candidate>,
    done: bool,
}

struct PendingPosition {
    packed: Option<Packed>,
    halfmove: u8,
    fullmove: u16,
    ply: u16,
}

impl<'a> MineVisitor<'a> {
    fn new(filters: &'a Filters, writer: &'a mut Writer, bar: ProgressBar) -> Self {
        MineVisitor {
            filters,
            writer,
            bar,
            stats: Stats::default(),
            seen_ids: HashSet::new(),
            pos: Chess::default(),
            ply: 0,
            skip_current: false,
            headers: GameHeaders::default(),
            game_had_eval: false,
            awaiting_eval: None,
            in_band_run: 0,
            candidates: Vec::new(),
            done: false,
        }
    }

    /// Called once the `[%eval]` for the position in `awaiting_eval` arrives.
    fn record_eval(&mut self, eval_cp: i16, mate: bool) {
        let pending = match self.awaiting_eval.take() {
            Some(p) => p,
            None => return,
        };
        self.game_had_eval = true;

        let in_band = !mate && eval_cp.abs() <= self.filters.eval_band_cp;
        if in_band {
            self.in_band_run = self.in_band_run.saturating_add(1);
        } else {
            self.in_band_run = 0;
        }

        let packed = match pending.packed {
            Some(p) => p,
            // Right eval, wrong piece count — nothing more to do, but the
            // in-band run above still advances so stability spans it.
            None => return,
        };

        self.stats.positions_at_size += 1;

        if !in_band {
            self.stats.rejected_band += 1;
            return;
        }
        if self.in_band_run < self.filters.stable_plies.max(1) {
            self.stats.rejected_unstable += 1;
            return;
        }
        if self.filters.require_imbalance && is_symmetric(&packed) {
            self.stats.rejected_symmetric += 1;
            return;
        }

        self.candidates.push(Candidate {
            packed,
            halfmove: pending.halfmove,
            fullmove: pending.fullmove,
            ply: pending.ply,
            eval_cp,
            mate,
        });
    }

    /// Game-level gates, applied once the final ply count is known.
    fn flush_game(&mut self) {
        let h = &self.headers;

        if !self.filters.allow_draws && h.result == 2 {
            self.stats.rejected_draw += self.candidates.len() as u64;
            return;
        }
        if h.result > 1 {
            return;
        }
        if self.filters.require_normal_termination && h.termination != Termination::Normal {
            self.stats.rejected_termination += self.candidates.len() as u64;
            return;
        }
        if h.white_elo < self.filters.min_elo || h.black_elo < self.filters.min_elo {
            self.stats.rejected_elo += self.candidates.len() as u64;
            return;
        }

        let game_plies = self.ply;
        self.stats.candidates += self.candidates.len() as u64;

        // Drop candidates the game did not play out from.
        let before = self.candidates.len();
        self.candidates
            .retain(|c| game_plies.saturating_sub(c.ply) >= self.filters.min_remaining_plies);
        self.stats.rejected_tail += (before - self.candidates.len()) as u64;

        // A long level ending yields a run of near-identical consecutive
        // plies. Keep the most balanced ones, spaced out, and drop the rest —
        // otherwise one game contributes a dozen duplicates of itself.
        let before = self.candidates.len();
        let chosen = select_spread(
            &mut self.candidates,
            self.filters.per_game.max(1),
            self.filters.min_gap_plies,
        );
        self.stats.rejected_same_game += (before - chosen.len()) as u64;
        self.candidates.clear();

        for cand in chosen {
            let id = position_id(&cand.packed);
            if !self.seen_ids.insert(id) {
                self.stats.duplicates += 1;
                continue;
            }

            let rec = Record {
                id,
                occupied: cand.packed.occupied,
                pieces: cand.packed.pieces,
                stm: cand.packed.stm,
                castling: cand.packed.castling,
                ep_square: cand.packed.ep_square,
                halfmove: cand.halfmove,
                fullmove: cand.fullmove,
                ply: cand.ply,
                game_plies,
                eval_cp: cand.eval_cp,
                white_elo: h.white_elo,
                black_elo: h.black_elo,
                tc_initial: h.tc_initial,
                tc_increment: h.tc_increment,
                utc_time: h.unix_time(),
                game_id: h.game_id,
                winner: h.result,
                termination: h.termination as u8,
                flags: if cand.mate { REC_FLAG_MATE } else { 0 },
                white_name: h.white,
                black_name: h.black,
                ..Record::default()
            };

            if let Err(e) = self.writer.push(&rec) {
                self.bar.println(format!("write failed: {e}"));
                self.done = true;
                return;
            }
            self.stats.written += 1;

            if self.stats.written % 2_000 == 0 {
                let _ = self.writer.flush_header();
            }
            if self.filters.limit != 0 && self.stats.written >= self.filters.limit {
                self.done = true;
                return;
            }
        }
    }
}

/// Picks at most `keep` candidates from one game: most balanced first, and
/// never two within `min_gap` plies of each other. Returned in ply order.
fn select_spread(candidates: &mut Vec<Candidate>, keep: usize, min_gap: u16) -> Vec<Candidate> {
    // Most balanced first; among equals prefer the later position, which has
    // fewer pieces still to come off and so is closer to the real endgame.
    candidates.sort_by_key(|c| (c.eval_cp.abs(), std::cmp::Reverse(c.ply)));

    let mut chosen: Vec<Candidate> = Vec::with_capacity(keep);
    for cand in candidates.drain(..) {
        if chosen.len() >= keep {
            break;
        }
        if chosen
            .iter()
            .any(|c| c.ply.abs_diff(cand.ply) < min_gap)
        {
            continue;
        }
        chosen.push(cand);
    }
    chosen.sort_by_key(|c| c.ply);
    chosen
}

/// True when both sides hold exactly the same multiset of non-king pieces.
fn is_symmetric(packed: &Packed) -> bool {
    let mut white = [0u8; 7];
    let mut black = [0u8; 7];
    for slot in 0..packed.occupied.count_ones() as usize {
        let byte = packed.pieces[slot / 2];
        let code = if slot % 2 == 0 { byte & 0x0f } else { byte >> 4 };
        if code == 0 || code == 6 || code == 12 {
            continue; // empty or king
        }
        if code < 6 {
            white[code as usize] += 1;
        } else {
            black[(code - 6) as usize] += 1;
        }
    }
    white == black
}

impl Visitor for MineVisitor<'_> {
    type Result = ();

    fn begin_game(&mut self) {
        self.pos = Chess::default();
        self.ply = 0;
        self.skip_current = false;
        self.headers = GameHeaders::default();
        self.game_had_eval = false;
        self.awaiting_eval = None;
        self.in_band_run = 0;
        self.candidates.clear();
        self.stats.games_seen += 1;
    }

    fn header(&mut self, key: &[u8], value: RawHeader<'_>) {
        let raw = value.as_bytes();
        match key {
            // Games from a custom start position or a variant break the
            // move replay, and we only want standard chess anyway.
            b"FEN" => self.headers.non_standard = true,
            b"Variant" if raw != b"Standard" => self.headers.non_standard = true,
            b"Site" => {
                // "https://lichess.org/<id>"
                let text = String::from_utf8_lossy(raw);
                let id = text.rsplit('/').next().unwrap_or("");
                self.headers.game_id = game_id_field(id);
            }
            b"White" => self.headers.white = name_field(&String::from_utf8_lossy(raw)),
            b"Black" => self.headers.black = name_field(&String::from_utf8_lossy(raw)),
            b"WhiteElo" => self.headers.white_elo = parse_u16(raw),
            b"BlackElo" => self.headers.black_elo = parse_u16(raw),
            b"Result" => {
                self.headers.result = match raw {
                    b"1-0" => 0,
                    b"0-1" => 1,
                    b"1/2-1/2" => 2,
                    _ => 3,
                }
            }
            b"UTCDate" => self.headers.utc_date = parse_date(raw),
            b"UTCTime" => self.headers.utc_time = parse_clock(raw),
            b"TimeControl" => {
                let (base, inc) = parse_time_control(raw);
                self.headers.tc_initial = base;
                self.headers.tc_increment = inc;
            }
            b"Termination" => {
                self.headers.termination =
                    Termination::from_pgn(&String::from_utf8_lossy(raw))
            }
            _ => {}
        }
    }

    fn end_headers(&mut self) -> Skip {
        // Cheap pre-filters that need no move replay: bail out of the game
        // before parsing a single SAN token.
        let h = &self.headers;
        let hopeless = h.non_standard
            || (!self.filters.allow_draws && h.result == 2)
            || h.result > 2
            || (self.filters.require_normal_termination && h.termination != Termination::Normal)
            || h.white_elo < self.filters.min_elo
            || h.black_elo < self.filters.min_elo;
        if hopeless {
            self.skip_current = true;
        } else {
            self.stats.games_scanned += 1;
        }
        Skip(self.skip_current || self.done)
    }

    fn san(&mut self, san_plus: SanPlus) {
        if self.skip_current || self.done {
            return;
        }
        // An unconsumed pending position means the previous move carried no
        // eval; that breaks the stability run.
        if self.awaiting_eval.take().is_some() {
            self.in_band_run = 0;
        }

        let mv = match san_plus.san.to_move(&self.pos) {
            Ok(mv) => mv,
            Err(_) => {
                self.skip_current = true;
                return;
            }
        };
        self.pos.play_unchecked(&mv);
        self.ply += 1;

        let packed = pack_position(&self.pos)
            .filter(|p| p.occupied.count_ones() == self.filters.pieces);

        self.awaiting_eval = Some(PendingPosition {
            packed,
            halfmove: u8::try_from(self.pos.halfmoves()).unwrap_or(u8::MAX),
            fullmove: u16::try_from(u32::from(self.pos.fullmoves())).unwrap_or(u16::MAX),
            ply: self.ply,
        });
    }

    fn comment(&mut self, comment: RawComment<'_>) {
        if self.skip_current || self.done || self.awaiting_eval.is_none() {
            return;
        }
        if let Some((cp, mate)) = parse_eval(comment.as_bytes()) {
            self.record_eval(cp, mate);
        }
    }

    // Analysis sidelines are not moves anybody played.
    fn begin_variation(&mut self) -> Skip {
        Skip(true)
    }

    fn end_game(&mut self) -> Self::Result {
        if !self.skip_current && !self.done {
            if self.game_had_eval {
                self.stats.games_with_eval += 1;
            } else {
                self.stats.rejected_no_eval += 1;
            }
            if !self.candidates.is_empty() {
                self.flush_game();
            }
        }
        self.candidates.clear();
        if self.stats.games_seen % 20_000 == 0 {
            self.bar.set_message(format!(
                "{} games · {} written",
                self.stats.games_seen, self.stats.written
            ));
        }
    }
}

fn parse_u16(raw: &[u8]) -> u16 {
    std::str::from_utf8(raw)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

fn parse_date(raw: &[u8]) -> Option<(i32, u32, u32)> {
    let text = std::str::from_utf8(raw).ok()?;
    let mut parts = text.split(['.', '-', '/']);
    let y = parts.next()?.parse().ok()?;
    let m = parts.next()?.parse().ok()?;
    let d = parts.next()?.parse().ok()?;
    Some((y, m, d))
}

fn parse_clock(raw: &[u8]) -> Option<(u32, u32, u32)> {
    let text = std::str::from_utf8(raw).ok()?;
    let mut parts = text.split(':');
    let h = parts.next()?.parse().ok()?;
    let m = parts.next()?.parse().ok()?;
    let s = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    Some((h, m, s))
}

/// `"600+5"` → `(600, 5)`. `"-"` and anything unparseable → `(0, 0)`.
fn parse_time_control(raw: &[u8]) -> (u16, u16) {
    let text = match std::str::from_utf8(raw) {
        Ok(t) => t,
        Err(_) => return (0, 0),
    };
    let mut parts = text.split('+');
    let base = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let inc = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    (base, inc)
}

/// Pulls the score out of a lichess annotation comment.
///
/// Accepts `[%eval 0.17]`, `[%eval -1.25]` and `[%eval #-4]`, ignoring any
/// `[%clk …]` that shares the comment. Returns centipawns (white POV) and
/// whether it was a mate announcement.
pub fn parse_eval(comment: &[u8]) -> Option<(i16, bool)> {
    let text = std::str::from_utf8(comment).ok()?;
    let start = text.find("%eval")? + "%eval".len();
    let rest = &text[start..];
    let body: &str = rest
        .trim_start()
        .split(|c: char| c == ']' || c.is_whitespace())
        .next()?;
    if body.is_empty() {
        return None;
    }

    if let Some(mate) = body.strip_prefix('#') {
        let plies: i32 = mate.parse().ok()?;
        let magnitude = (MATE_BASE as i32 - plies.abs().min(1000)) as i16;
        return Some((if plies < 0 { -magnitude } else { magnitude }, true));
    }

    let pawns: f64 = body.parse().ok()?;
    let cp = (pawns * 100.0).round().clamp(-29_000.0, 29_000.0) as i16;
    Some((cp, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_eval_comments() {
        assert_eq!(parse_eval(b" [%eval 0.17] ").unwrap(), (17, false));
        assert_eq!(parse_eval(b"[%eval -1.25]").unwrap(), (-125, false));
        assert_eq!(parse_eval(b"[%eval 0.0]").unwrap(), (0, false));
        assert_eq!(
            parse_eval(b"[%eval 0.24] [%clk 0:03:00]").unwrap(),
            (24, false)
        );
        assert_eq!(
            parse_eval(b"[%clk 0:03:00] [%eval -0.05]").unwrap(),
            (-5, false)
        );
        assert!(parse_eval(b"[%clk 0:03:00]").is_none());
    }

    #[test]
    fn parses_mate_evals() {
        let (cp, mate) = parse_eval(b"[%eval #4]").unwrap();
        assert!(mate);
        assert!(cp > 29_000);
        let (cp, mate) = parse_eval(b"[%eval #-3]").unwrap();
        assert!(mate);
        assert!(cp < -29_000);
    }

    #[test]
    fn truncation_is_told_apart_from_corruption() {
        assert!(is_truncation(&io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "eof"
        )));
        assert!(is_truncation(&io::Error::other("Unexpected end of input")));
        assert!(!is_truncation(&io::Error::other(
            "Corrupted block detected"
        )));
        assert!(!is_truncation(&io::Error::new(
            io::ErrorKind::PermissionDenied,
            "nope"
        )));
    }

    #[test]
    fn parses_time_controls() {
        assert_eq!(parse_time_control(b"600+5"), (600, 5));
        assert_eq!(parse_time_control(b"180+0"), (180, 0));
        assert_eq!(parse_time_control(b"-"), (0, 0));
    }

    #[test]
    fn civil_dates_match_known_epochs() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11017);
        // 2019-09-01T00:00:00Z
        let h = GameHeaders {
            utc_date: Some((2019, 9, 1)),
            utc_time: Some((0, 0, 0)),
            ..Default::default()
        };
        assert_eq!(h.unix_time(), 1_567_296_000);
    }

    fn candidate(ply: u16, eval_cp: i16) -> Candidate {
        Candidate {
            packed: Packed {
                occupied: 0,
                pieces: [0; 4],
                stm: 0,
                castling: 0,
                ep_square: 255,
            },
            halfmove: 0,
            fullmove: 1,
            ply,
            eval_cp,
            mate: false,
        }
    }

    #[test]
    fn per_game_cap_takes_the_most_balanced() {
        let mut cands = vec![candidate(80, 25), candidate(81, 3), candidate(82, -18)];
        let chosen = select_spread(&mut cands, 1, 8);
        assert_eq!(chosen.len(), 1);
        assert_eq!(chosen[0].ply, 81, "should keep the closest to level");
    }

    #[test]
    fn spread_enforces_a_ply_gap() {
        // Best is ply 81; the next-best inside the gap must be skipped, so the
        // second pick is the far-away ply 120 despite its worse eval.
        let mut cands = vec![
            candidate(80, 25),
            candidate(81, 3),
            candidate(84, 5),
            candidate(120, 28),
        ];
        let chosen = select_spread(&mut cands, 2, 8);
        assert_eq!(
            chosen.iter().map(|c| c.ply).collect::<Vec<_>>(),
            vec![81, 120]
        );
    }

    #[test]
    fn spread_returns_at_most_what_exists() {
        let mut cands = vec![candidate(90, 1)];
        assert_eq!(select_spread(&mut cands, 5, 8).len(), 1);
        assert_eq!(select_spread(&mut Vec::new(), 3, 8).len(), 0);
    }

    #[test]
    fn symmetry_detection() {
        // Kings + one rook each: symmetric.
        let sym = Packed {
            occupied: 0b1111,
            pieces: [0x46, 0xac, 0, 0], // WR, WK, BR, BK
            stm: 0,
            castling: 0,
            ep_square: 255,
        };
        assert!(is_symmetric(&sym));

        // Kings + white rook vs black bishop: not symmetric.
        let asym = Packed {
            occupied: 0b1111,
            pieces: [0x46, 0xc9, 0, 0], // WR, WK, BB, BK
            stm: 0,
            castling: 0,
            ep_square: 255,
        };
        assert!(!is_symmetric(&asym));
    }
}
