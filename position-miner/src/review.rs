//! Turns an `ARCPOS1` file into things a human can actually look at: JSON
//! lines for tooling, and a self-contained HTML contact sheet for eyeballing
//! a sample of boards.

use anyhow::Result;
use serde::Serialize;
use std::fmt::Write as _;

use crate::format::{Record, Termination, EVAL_UNSET};
use crate::position::{ascii, fen, material_signature, piece_count, squares};

#[derive(Serialize)]
pub struct Json<'a> {
    pub id: String,
    pub fen: String,
    pub pieces: u32,
    /// Both sides' material combined and sorted, e.g. `KKPPPPRR`. The variety
    /// the material gates exist to force is only visible in aggregate, so it
    /// wants to be a field you can group a whole deck by rather than something
    /// re-derived from the FEN each time.
    pub material: String,
    pub side_to_move: &'a str,
    pub ply: u16,
    pub game_plies: u16,
    pub eval_cp: i16,
    pub eval_is_mate: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified_cp: Option<i16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub second_cp: Option<i16>,
    /// Best minus second-best root move, in centipawns. The sharpness measure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drop_cp: Option<i16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legal_moves: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub losing_moves: Option<u8>,
    /// Root moves that stay within 50cp of the best. The sharpness measure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub holding_moves: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wdl: Option<[u16; 3]>,
    pub winner: &'a str,
    pub white: String,
    pub black: String,
    pub white_elo: u16,
    pub black_elo: u16,
    pub time_control: String,
    pub utc_time: i64,
    pub utc_date: String,
    pub termination: &'static str,
    pub game_id: String,
    pub game_url: String,
}

pub fn to_json(rec: &Record) -> Json<'static> {
    Json {
        id: rec.id_str(),
        fen: fen(rec),
        pieces: piece_count(rec),
        material: material_signature(rec.occupied, &rec.pieces),
        side_to_move: if rec.stm == 0 { "white" } else { "black" },
        ply: rec.ply,
        game_plies: rec.game_plies,
        eval_cp: rec.eval_cp,
        eval_is_mate: rec.is_mate_eval(),
        verified_cp: (rec.verified_cp != EVAL_UNSET).then_some(rec.verified_cp),
        second_cp: (rec.second_cp != EVAL_UNSET).then_some(rec.second_cp),
        drop_cp: (rec.verified_cp != EVAL_UNSET).then_some(rec.drop_cp),
        legal_moves: (rec.legal_moves > 0).then_some(rec.legal_moves),
        losing_moves: (rec.legal_moves > 0).then_some(rec.losing_moves),
        holding_moves: (rec.legal_moves > 0).then_some(rec.holding_moves),
        wdl: (rec.wdl_win + rec.wdl_draw + rec.wdl_loss > 0)
            .then_some([rec.wdl_win, rec.wdl_draw, rec.wdl_loss]),
        winner: if rec.winner == 0 { "white" } else { "black" },
        white: rec.white().to_string(),
        black: rec.black().to_string(),
        white_elo: rec.white_elo,
        black_elo: rec.black_elo,
        time_control: format!("{}+{}", rec.tc_initial, rec.tc_increment),
        utc_time: rec.utc_time,
        utc_date: format_date(rec.utc_time),
        termination: Termination::from_u8(rec.termination).as_str(),
        game_id: rec.game_id_str().to_string(),
        game_url: rec.game_url(),
    }
}

/// Unix seconds → `YYYY-MM-DD HH:MM` (UTC). Inverse of `days_from_civil`.
pub fn format_date(unix: i64) -> String {
    if unix == 0 {
        return "unknown".into();
    }
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60
    )
}

/// A terminal-friendly block for one record.
pub fn text_block(rec: &Record) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{}  {} to move  ·  {} pieces ({})  ·  eval {}",
        rec.id_str(),
        if rec.stm == 0 { "white" } else { "black" },
        piece_count(rec),
        material_signature(rec.occupied, &rec.pieces),
        eval_label(rec)
    );
    out.push_str(&ascii(rec));
    if rec.verified_cp != EVAL_UNSET {
        let _ = writeln!(
            out,
            "  engine {:+.2}  ·  {} of {} moves hold  ·  {} lose outright  ·  2nd best {:+.2}",
            rec.verified_cp as f64 / 100.0,
            rec.holding_moves,
            rec.legal_moves,
            rec.losing_moves,
            rec.second_cp as f64 / 100.0
        );
    }
    let _ = writeln!(out, "  {}", fen(rec));
    let _ = writeln!(
        out,
        "  {} ({}) vs {} ({})  ·  {} won  ·  ply {}/{}  ·  {}  ·  {}",
        rec.white(),
        rec.white_elo,
        rec.black(),
        rec.black_elo,
        rec.winner_str(),
        rec.ply,
        rec.game_plies,
        format_date(rec.utc_time),
        rec.game_url()
    );
    out
}

pub fn eval_label(rec: &Record) -> String {
    if rec.is_mate_eval() {
        return "mate".into();
    }
    format!("{:+.2}", rec.eval_cp as f64 / 100.0)
}

// ---------------------------------------------------------------------------
// HTML contact sheet
// ---------------------------------------------------------------------------

const GLYPHS: [&str; 13] = [
    "", "♙", "♘", "♗", "♖", "♕", "♔", "♟", "♞", "♝", "♜", "♛", "♚",
];

pub struct PageMeta {
    pub title: String,
    pub subtitle: String,
    pub notes: Vec<String>,
}

pub fn html_page(records: &[Record], meta: &PageMeta) -> String {
    let mut cards = String::new();
    for rec in records {
        cards.push_str(&card(rec));
    }

    let notes = meta
        .notes
        .iter()
        .map(|n| format!("<li>{}</li>", escape(n)))
        .collect::<Vec<_>>()
        .join("");

    format!(
        r#"<title>{title}</title>
<style>{css}</style>
<header>
  <h1>{title}</h1>
  <p class="sub">{subtitle}</p>
  <ul class="notes">{notes}</ul>
</header>
<main class="grid">{cards}</main>
"#,
        title = escape(&meta.title),
        subtitle = escape(&meta.subtitle),
        css = CSS,
        notes = notes,
        cards = cards
    )
}

fn card(rec: &Record) -> String {
    let board = squares(rec);
    let mut cells = String::new();
    // Rank 8 down to rank 1 so white sits at the bottom.
    for rank in (0..8).rev() {
        for file in 0..8 {
            let code = board[rank * 8 + file] as usize;
            let dark = (rank + file) % 2 == 0;
            let shade = if dark { "d" } else { "l" };
            let colour = if code == 0 {
                ""
            } else if code <= 6 {
                " w"
            } else {
                " b"
            };
            let _ = write!(
                cells,
                r#"<div class="sq {shade}{colour}">{}</div>"#,
                GLYPHS[code]
            );
        }
    }

    let stm = if rec.stm == 0 { "White" } else { "Black" };
    let winner = if rec.winner == 0 { "White" } else { "Black" };
    let win_class = if rec.winner == 0 { "wwin" } else { "bwin" };
    let verified = if rec.verified_cp != EVAL_UNSET {
        format!(
            r#"<span class="tag">engine {:+.2}</span><span class="tag sharp">{} of {} moves hold</span>"#,
            rec.verified_cp as f64 / 100.0,
            rec.holding_moves,
            rec.legal_moves
        )
    } else {
        String::new()
    };
    let wdl = if rec.verified_cp != EVAL_UNSET {
        format!(
            r#"<div class="row dim"><b class="sharpv">{}</b> of {} root moves lose outright · 2nd best {:+.2}</div>"#,
            rec.losing_moves,
            rec.legal_moves,
            rec.second_cp as f64 / 100.0,
        )
    } else {
        String::new()
    };

    format!(
        r#"<article class="card">
  <div class="board">{cells}</div>
  <div class="meta">
    <div class="row"><code class="id">{id}</code><span class="tag">{stm} to move</span><span class="tag mat">{material}</span><span class="tag">eval {eval}</span>{verified}</div>
    {wdl}
    <div class="players">
      <span class="p"><b>{white}</b> {welo}</span>
      <span class="vs">vs</span>
      <span class="p"><b>{black}</b> {belo}</span>
    </div>
    <div class="row dim"><span class="{win_class}">{winner} won</span> · ply {ply}/{plies} · {tc} · {date}</div>
    <code class="fen">{fen}</code>
    <a href="{url}" target="_blank" rel="noopener">lichess/{gid} &rarr;</a>
  </div>
</article>
"#,
        cells = cells,
        id = escape(&rec.id_str()),
        stm = stm,
        material = material_signature(rec.occupied, &rec.pieces),
        eval = eval_label(rec),
        verified = verified,
        wdl = wdl,
        white = escape(rec.white()),
        welo = rec.white_elo,
        black = escape(rec.black()),
        belo = rec.black_elo,
        win_class = win_class,
        winner = winner,
        ply = rec.ply,
        plies = rec.game_plies,
        tc = format!("{}+{}", rec.tc_initial, rec.tc_increment),
        date = format_date(rec.utc_time),
        fen = escape(&fen(rec)),
        url = escape(&rec.game_url()),
        gid = escape(rec.game_id_str()),
    )
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const CSS: &str = r#"
:root {
  --bg: #14161a; --panel: #1c1f26; --line: #2c313b;
  --ink: #e7e9ee; --dim: #969cab; --accent: #7fb3ff;
  --sq-l: #b9c2cf; --sq-d: #6f7d90;
  --pw: #ffffff; --pb: #14161a;
}
@media (prefers-color-scheme: light) {
  :root {
    --bg: #f6f7f9; --panel: #ffffff; --line: #e2e5ea;
    --ink: #191c22; --dim: #616a7a; --accent: #1f5fbf;
    --sq-l: #e8ecf2; --sq-d: #93a1b5;
    --pw: #ffffff; --pb: #14161a;
  }
}
:root[data-theme="dark"] {
  --bg: #14161a; --panel: #1c1f26; --line: #2c313b;
  --ink: #e7e9ee; --dim: #969cab; --accent: #7fb3ff;
  --sq-l: #b9c2cf; --sq-d: #6f7d90;
}
:root[data-theme="light"] {
  --bg: #f6f7f9; --panel: #ffffff; --line: #e2e5ea;
  --ink: #191c22; --dim: #616a7a; --accent: #1f5fbf;
  --sq-l: #e8ecf2; --sq-d: #93a1b5;
}
body {
  margin: 0; padding: 2rem 1.25rem 4rem;
  background: var(--bg); color: var(--ink);
  font: 15px/1.5 ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif;
}
header { max-width: 1200px; margin: 0 auto 2rem; }
h1 { font-size: 1.6rem; margin: 0 0 .35rem; letter-spacing: -0.01em; }
.sub { margin: 0 0 .75rem; color: var(--dim); }
.notes { margin: 0; padding-left: 1.1rem; color: var(--dim); font-size: .875rem; }
.notes li { margin: .15rem 0; }
.grid {
  max-width: 1200px; margin: 0 auto;
  display: grid; gap: 1rem;
  grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
}
.card {
  background: var(--panel); border: 1px solid var(--line); border-radius: 12px;
  padding: .85rem; display: flex; gap: .85rem; align-items: flex-start;
  min-width: 0;
}
.board {
  display: grid; grid-template-columns: repeat(8, 1fr);
  width: 152px; min-width: 152px; aspect-ratio: 1;
  border-radius: 6px; overflow: hidden; border: 1px solid var(--line);
}
.sq {
  display: grid; place-items: center;
  font-size: 15px; line-height: 1;
}
.sq.l { background: var(--sq-l); }
.sq.d { background: var(--sq-d); }
.sq.w { color: var(--pw); text-shadow: 0 0 1px #000, 0 1px 1px rgba(0,0,0,.5); }
.sq.b { color: var(--pb); text-shadow: 0 0 1px rgba(255,255,255,.4); }
.meta { min-width: 0; flex: 1; display: flex; flex-direction: column; gap: .3rem; }
.row { display: flex; flex-wrap: wrap; gap: .35rem; align-items: center; font-size: .8rem; }
.dim { color: var(--dim); }
.id { font-size: .7rem; letter-spacing: .04em; color: var(--accent); }
.tag {
  font-size: .7rem; padding: .1rem .4rem; border-radius: 999px;
  border: 1px solid var(--line); color: var(--dim); white-space: nowrap;
}
.players { font-size: .82rem; display: flex; flex-wrap: wrap; gap: .3rem; }
.p b { font-weight: 600; }
.vs { color: var(--dim); }
.wwin { color: #6fd08c; font-weight: 600; }
.bwin { color: #ff9f8a; font-weight: 600; }
.tag.sharp { border-color: #d98b4a; color: #d98b4a; }
/* Monospaced so signatures line up down a column and an odd one stands out. */
.tag.mat { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; letter-spacing: .02em; }
.sharpv { color: #d98b4a; }
.fen {
  font-size: .68rem; color: var(--dim); word-break: break-all;
  background: rgba(127,127,127,.09); padding: .25rem .35rem; border-radius: 5px;
}
a { color: var(--accent); font-size: .78rem; text-decoration: none; }
a:hover { text-decoration: underline; }
"#;

pub fn write_html(path: &std::path::Path, records: &[Record], meta: &PageMeta) -> Result<()> {
    std::fs::write(path, html_page(records, meta))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_dates_back_from_unix() {
        assert_eq!(format_date(1_567_296_000), "2019-09-01 00:00");
        assert_eq!(format_date(0), "unknown");
        assert_eq!(format_date(1_577_836_800), "2020-01-01 00:00");
    }
}
