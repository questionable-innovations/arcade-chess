//! Stage 2: re-score the mined positions with a local engine, and measure how
//! *sharp* they are.
//!
//! # Why "eval ≈ 0" is not enough
//!
//! The obvious way to look for a 50/50 position is to ask an engine for a
//! score near zero. On an eight-piece board that filter fails badly: run it
//! and essentially everything it returns is a **dead draw**. Stockfish will
//! happily tell you a level king-and-pawns ending is `0.00` with a
//! win/draw/loss spread of `1 / 998 / 1`. The human who won that game did so
//! because their opponent blundered twenty plies later, not because the
//! position held any tension.
//!
//! WDL cannot rescue the filter either. Stockfish's WDL is derived from its
//! own eval through a material-scaled logistic curve — at `0.00` with eight
//! pieces on the board it reports ~99% draw more or less by construction,
//! whatever the position actually looks like. Asking it to separate "tense"
//! from "dead" is asking a question it does not answer.
//!
//! # What actually separates them
//!
//! A position is interesting when it is *balanced but unforgiving*: the
//! evaluation is level, yet only one or two moves keep it that way and the
//! rest lose. That is a property of the **move list**, not of the score, and
//! it is measured with a MultiPV search:
//!
//! ```text
//!   dead draw          sharp position
//!   1. Kf3   0.00      1. Rd7   0.00
//!   2. Kg3   0.00      2. Kf2  -3.10
//!   3. Kh3  -0.05      3. Rd1  -3.40
//!   4. Kf2  -0.10      4. Ra7  -4.00
//!   → 4 moves hold     → 1 move holds
//! ```
//!
//! The measure is therefore **`holding_moves`**: how many root moves stay
//! within [`HOLDING_TOLERANCE_CP`] of the best one. A level position with one
//! or two holding moves out of a long list is one where a single slip decides
//! the game — 50/50 in the sense that matters, and nothing like a draw.
//!
//! The obvious alternative, `best - second_best` (recorded as `drop_cp`), is
//! *not* the gate, because it only catches the strict only-move case. A
//! position where exactly two moves hold and seventeen lose has a drop of
//! zero and is every bit as sharp. Counting the holders catches both; the drop
//! is kept alongside as a finer-grained ranking signal.

use anyhow::{bail, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::format::{Reader, Record, Writer, EVAL_UNSET};
use crate::position::fen;

/// How many root moves to ask for. Eight-piece positions rarely have more
/// legal moves than this, so in practice we see the whole move list and can
/// count how many of them lose.
pub const MULTIPV: u32 = 48;

/// A root move this far below the best one counts as "losing" for the
/// `losing_moves` tally.
pub const LOSING_DROP_CP: i32 = 300;

/// A root move within this much of the best one counts as "holding" — it
/// preserves the balance rather than throwing it away.
pub const HOLDING_TOLERANCE_CP: i32 = 50;

#[derive(Debug, Clone)]
pub struct VerifyOpts {
    pub engine: String,
    pub depth: u32,
    pub threads_per_engine: u32,
    pub hash_mb: u32,
    pub workers: usize,
    /// Keep positions whose best line is within ± this many centipawns.
    pub eval_band_cp: i16,
    /// The sharpness gate: keep positions where at most this many root moves
    /// hold the balance.
    pub max_holding: u8,
    /// Reject positions with fewer root moves than this. In a position with
    /// three legal moves, "only one holds" is arithmetic, not tension.
    pub min_legal: u8,
    /// Drop positions whose draw probability is at least this per-mille.
    /// Defaults to 1000 (off) — WDL is recorded for information, but it is
    /// not a useful discriminator at this material count.
    pub max_draw_permille: u16,
    pub limit: u64,
}

impl Default for VerifyOpts {
    fn default() -> Self {
        VerifyOpts {
            engine: "stockfish".into(),
            depth: 26,
            threads_per_engine: 1,
            hash_mb: 128,
            workers: 4,
            eval_band_cp: 40,
            max_holding: 2,
            min_legal: 6,
            max_draw_permille: 1000,
            limit: 0,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct VerifyStats {
    pub scored: u64,
    pub kept: u64,
    pub dropped_band: u64,
    pub dropped_flat: u64,
    pub dropped_forced: u64,
    pub dropped_drawish: u64,
}

/// The result of one MultiPV search.
#[derive(Debug, Clone, Copy)]
pub struct Analysis {
    /// Best line, white POV.
    pub best_cp: i16,
    /// Second-best line, white POV. Equals `best_cp` when there is only one
    /// legal move.
    pub second_cp: i16,
    /// `best - second`, side-to-move POV, so always >= 0.
    pub drop_cp: i16,
    pub wdl: [u16; 3],
    /// Root moves the engine reported (capped at `MULTIPV`).
    pub legal_moves: u8,
    /// Of those, how many are at least `LOSING_DROP_CP` worse than the best.
    pub losing_moves: u8,
    /// Of those, how many are within `HOLDING_TOLERANCE_CP` of the best.
    /// This is the sharpness measure — see the module docs.
    pub holding_moves: u8,
}

struct Engine {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Engine {
    fn spawn(opts: &VerifyOpts) -> Result<Engine> {
        let mut child = Command::new(&opts.engine)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("launching engine {:?}", opts.engine))?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = BufReader::new(child.stdout.take().expect("piped"));
        let mut engine = Engine {
            child,
            stdin,
            stdout,
        };
        engine.send("uci")?;
        engine.wait_for("uciok")?;
        engine.send(&format!(
            "setoption name Threads value {}",
            opts.threads_per_engine
        ))?;
        engine.send(&format!("setoption name Hash value {}", opts.hash_mb))?;
        engine.send("setoption name UCI_ShowWDL value true")?;
        engine.send(&format!("setoption name MultiPV value {MULTIPV}"))?;
        engine.send("isready")?;
        engine.wait_for("readyok")?;
        Ok(engine)
    }

    fn send(&mut self, line: &str) -> Result<()> {
        writeln!(self.stdin, "{line}")?;
        self.stdin.flush()?;
        Ok(())
    }

    fn wait_for(&mut self, token: &str) -> Result<()> {
        let mut line = String::new();
        loop {
            line.clear();
            if self.stdout.read_line(&mut line)? == 0 {
                bail!("engine closed its output while waiting for {token}");
            }
            if line.trim_end() == token || line.starts_with(token) {
                return Ok(());
            }
        }
    }

    fn analyse(&mut self, position: &str, depth: u32, white_to_move: bool) -> Result<Analysis> {
        self.send("ucinewgame")?;
        self.send("isready")?;
        self.wait_for("readyok")?;
        self.send(&format!("position fen {position}"))?;
        self.send(&format!("go depth {depth}"))?;

        // Keep the last reported line per multipv slot: the engine re-reports
        // each slot at every depth, and we want the deepest set.
        let mut lines: BTreeMap<u32, InfoLine> = BTreeMap::new();
        let mut buf = String::new();
        loop {
            buf.clear();
            if self.stdout.read_line(&mut buf)? == 0 {
                bail!("engine closed its output mid-search");
            }
            if buf.starts_with("bestmove") {
                break;
            }
            if let Some(info) = parse_info(&buf) {
                lines.insert(info.multipv, info);
            }
        }

        summarise(&lines, white_to_move).context("engine reported no usable score")
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.send("quit");
        let _ = self.child.wait();
    }
}

#[derive(Debug, Clone, Copy)]
struct InfoLine {
    multipv: u32,
    /// Side-to-move POV, as UCI reports it.
    cp: i32,
    wdl: Option<[u16; 3]>,
}

/// Pulls `multipv`, `score cp|mate` and `wdl` out of one info line.
fn parse_info(line: &str) -> Option<InfoLine> {
    if !line.starts_with("info ") {
        return None;
    }
    let mut tokens = line.split_ascii_whitespace();
    let mut multipv = 1;
    let mut cp = None;
    let mut wdl = None;
    while let Some(tok) = tokens.next() {
        match tok {
            "multipv" => multipv = tokens.next()?.parse().ok()?,
            "score" => match tokens.next()? {
                "cp" => cp = tokens.next()?.parse::<i32>().ok(),
                "mate" => {
                    let plies: i32 = tokens.next()?.parse().ok()?;
                    let magnitude = crate::format::MATE_BASE as i32 - plies.abs().min(1000);
                    cp = Some(if plies < 0 { -magnitude } else { magnitude });
                }
                _ => {}
            },
            "wdl" => {
                let w = tokens.next()?.parse().ok()?;
                let d = tokens.next()?.parse().ok()?;
                let l = tokens.next()?.parse().ok()?;
                wdl = Some([w, d, l]);
            }
            _ => {}
        }
    }
    Some(InfoLine {
        multipv,
        cp: cp?,
        wdl,
    })
}

/// Folds the per-slot info lines into the sharpness summary.
fn summarise(lines: &BTreeMap<u32, InfoLine>, white_to_move: bool) -> Option<Analysis> {
    let best = lines.get(&1)?;
    let second = lines.get(&2).unwrap_or(best);

    // UCI scores are side-to-move POV; the file stores white POV.
    let flip = |cp: i32| -> i16 {
        let v = if white_to_move { cp } else { -cp };
        v.clamp(-30_000, 30_000) as i16
    };

    let drop_cp = (best.cp - second.cp).clamp(0, 30_000) as i16;
    let losing = lines
        .values()
        .filter(|l| best.cp - l.cp >= LOSING_DROP_CP)
        .count();
    let holding = lines
        .values()
        .filter(|l| best.cp - l.cp <= HOLDING_TOLERANCE_CP)
        .count();

    Some(Analysis {
        best_cp: flip(best.cp),
        second_cp: flip(second.cp),
        drop_cp,
        wdl: best.wdl.unwrap_or([0, 0, 0]),
        legal_moves: lines.len().min(255) as u8,
        losing_moves: losing.min(255) as u8,
        holding_moves: holding.min(255) as u8,
    })
}

/// Re-scores every record in `input` and writes the survivors to `out`,
/// sharpest first.
pub fn run(input: &Path, out: &Path, opts: &VerifyOpts, progress: bool) -> Result<VerifyStats> {
    let reader = Reader::open(input)?;
    let total = if opts.limit == 0 {
        reader.count
    } else {
        reader.count.min(opts.limit)
    };

    let mut records: Vec<Record> = Vec::with_capacity(total as usize);
    for rec in reader {
        records.push(rec?);
        if records.len() as u64 >= total {
            break;
        }
    }

    let bar = if progress {
        let bar = ProgressBar::new(total);
        bar.set_style(
            ProgressStyle::with_template("{bar:32} {pos}/{len} ({eta}) {msg}").unwrap(),
        );
        bar
    } else {
        ProgressBar::hidden()
    };

    let cursor = AtomicUsize::new(0);
    let scored: Mutex<Vec<Record>> = Mutex::new(Vec::with_capacity(records.len()));
    let workers = opts.workers.max(1);

    std::thread::scope(|scope| -> Result<()> {
        let mut handles = Vec::new();
        for _ in 0..workers {
            let cursor = &cursor;
            let records = &records;
            let scored = &scored;
            let bar = bar.clone();
            handles.push(scope.spawn(move || -> Result<()> {
                let mut engine = Engine::spawn(opts)?;
                let mut local = Vec::new();
                loop {
                    let idx = cursor.fetch_add(1, Ordering::Relaxed);
                    if idx >= records.len() {
                        break;
                    }
                    let mut rec = records[idx];
                    let position = fen(&rec);
                    match engine.analyse(&position, opts.depth, rec.stm == 0) {
                        Ok(a) => {
                            rec.verified_cp = a.best_cp;
                            rec.second_cp = a.second_cp;
                            rec.drop_cp = a.drop_cp;
                            rec.legal_moves = a.legal_moves;
                            rec.losing_moves = a.losing_moves;
                            rec.holding_moves = a.holding_moves;
                            rec.wdl_win = a.wdl[0];
                            rec.wdl_draw = a.wdl[1];
                            rec.wdl_loss = a.wdl[2];
                            local.push(rec);
                        }
                        Err(e) => {
                            bar.println(format!("skipping {}: {e}", rec.id_str()));
                        }
                    }
                    bar.inc(1);
                }
                scored.lock().unwrap().extend(local);
                Ok(())
            }));
        }
        for handle in handles {
            handle
                .join()
                .map_err(|_| anyhow::anyhow!("worker panicked"))??;
        }
        Ok(())
    })?;

    let scored = scored.into_inner().unwrap();
    bar.finish_and_clear();
    write_filtered(&scored, out, opts)
}

/// Applies the keep/reject gates to already-scored records and writes the
/// survivors, sharpest first.
///
/// Split out from [`run`] so thresholds can be retuned with [`filter`] without
/// paying for the search again — a full MultiPV pass over a month's candidates
/// costs far more than anyone wants to spend on a changed constant.
pub fn write_filtered(
    scored: &[Record],
    out: &Path,
    opts: &VerifyOpts,
) -> Result<VerifyStats> {
    let mut ranked: Vec<Record> = scored.to_vec();
    // Sharpest first: fewest ways to hold, then the most ways to lose, then
    // the biggest cliff behind the best move, then closest to level.
    ranked.sort_by_key(|r| {
        (
            r.holding_moves,
            std::cmp::Reverse(r.losing_moves),
            std::cmp::Reverse(r.drop_cp),
            r.verified_cp.abs(),
        )
    });

    let mut stats = VerifyStats::default();
    let mut writer = Writer::create(out)?;
    for rec in &ranked {
        stats.scored += 1;
        if rec.verified_cp == EVAL_UNSET || rec.verified_cp.abs() > opts.eval_band_cp {
            stats.dropped_band += 1;
            continue;
        }
        // A position with almost no legal moves is forced, not tense.
        if rec.legal_moves < opts.min_legal {
            stats.dropped_forced += 1;
            continue;
        }
        if rec.holding_moves > opts.max_holding {
            stats.dropped_flat += 1;
            continue;
        }
        let has_wdl = rec.wdl_win + rec.wdl_draw + rec.wdl_loss > 0;
        if has_wdl && rec.wdl_draw > opts.max_draw_permille {
            stats.dropped_drawish += 1;
            continue;
        }
        writer.push(rec)?;
        stats.kept += 1;
    }
    writer.finish()?;
    Ok(stats)
}

/// Re-applies the gates to a file that has already been through [`run`].
pub fn filter(input: &Path, out: &Path, opts: &VerifyOpts) -> Result<VerifyStats> {
    let reader = Reader::open(input)?;
    let mut records = Vec::with_capacity(reader.count as usize);
    for rec in reader {
        records.push(rec?);
    }
    write_filtered(&records, out, opts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(multipv: u32, cp: i32) -> InfoLine {
        InfoLine {
            multipv,
            cp,
            wdl: Some([10, 980, 10]),
        }
    }

    #[test]
    fn parses_multipv_info_lines() {
        let line = "info depth 24 seldepth 30 multipv 3 score cp -13 wdl 121 795 84 nodes 1234 pv e2e4";
        let parsed = parse_info(line).unwrap();
        assert_eq!(parsed.multipv, 3);
        assert_eq!(parsed.cp, -13);
        assert_eq!(parsed.wdl, Some([121, 795, 84]));
    }

    #[test]
    fn parses_mate_info() {
        let parsed = parse_info("info depth 12 multipv 1 score mate -3 nodes 99 pv a1a2").unwrap();
        assert!(parsed.cp < -29_000);
    }

    #[test]
    fn ignores_scoreless_info() {
        assert!(parse_info("info depth 1 currmove e2e4 currmovenumber 1").is_none());
        assert!(parse_info("bestmove e2e4").is_none());
    }

    /// The dead-draw shape: every root move scores about the same, so every
    /// one of them holds and nothing is at stake.
    #[test]
    fn flat_position_has_no_drop() {
        let lines = BTreeMap::from([(1, info(1, 0)), (2, info(2, 0)), (3, info(3, -5))]);
        let a = summarise(&lines, true).unwrap();
        assert_eq!(a.best_cp, 0);
        assert_eq!(a.drop_cp, 0);
        assert_eq!(a.losing_moves, 0);
        assert_eq!(a.legal_moves, 3);
        assert_eq!(a.holding_moves, 3, "everything holds — nothing at stake");
    }

    /// The shape we are mining for: level, but only one move holds it.
    #[test]
    fn only_move_position_has_a_big_drop() {
        let lines = BTreeMap::from([
            (1, info(1, 12)),
            (2, info(2, -310)),
            (3, info(3, -450)),
            (4, info(4, -900)),
        ]);
        let a = summarise(&lines, true).unwrap();
        assert_eq!(a.best_cp, 12);
        assert_eq!(a.second_cp, -310);
        assert_eq!(a.drop_cp, 322);
        assert_eq!(a.losing_moves, 3, "three moves lose by 300cp or more");
        assert_eq!(a.holding_moves, 1);
    }

    /// The case a best-vs-second comparison misses entirely: two moves hold
    /// the balance and everything else loses. `drop_cp` is zero, yet the
    /// position is exactly as unforgiving as an only-move.
    #[test]
    fn two_holders_are_sharp_even_though_the_drop_is_zero() {
        let lines = BTreeMap::from([
            (1, info(1, 0)),
            (2, info(2, 0)),
            (3, info(3, -420)),
            (4, info(4, -530)),
            (5, info(5, -800)),
        ]);
        let a = summarise(&lines, true).unwrap();
        assert_eq!(a.drop_cp, 0, "the drop metric alone would call this flat");
        assert_eq!(a.holding_moves, 2, "but only two moves actually hold");
        assert_eq!(a.losing_moves, 3);
    }

    /// The holding tolerance is a band, not an exact match.
    #[test]
    fn near_best_moves_still_count_as_holding() {
        let lines = BTreeMap::from([
            (1, info(1, 0)),
            (2, info(2, -HOLDING_TOLERANCE_CP)),
            (3, info(3, -HOLDING_TOLERANCE_CP - 1)),
        ]);
        let a = summarise(&lines, true).unwrap();
        assert_eq!(a.holding_moves, 2, "exactly at the tolerance still holds");
    }

    #[test]
    fn scores_flip_for_black_to_move() {
        let lines = BTreeMap::from([(1, info(1, 20)), (2, info(2, -400))]);
        let a = summarise(&lines, false).unwrap();
        assert_eq!(a.best_cp, -20, "white POV is the negation");
        assert_eq!(a.second_cp, 400);
        assert_eq!(a.drop_cp, 420, "the drop stays side-to-move POV");
    }

    #[test]
    fn single_legal_move_is_not_sharp() {
        let lines = BTreeMap::from([(1, info(1, 0))]);
        let a = summarise(&lines, true).unwrap();
        assert_eq!(a.legal_moves, 1);
        assert_eq!(a.drop_cp, 0, "nothing to compare against");
    }

    #[test]
    fn empty_analysis_is_rejected() {
        assert!(summarise(&BTreeMap::new(), true).is_none());
    }
}
