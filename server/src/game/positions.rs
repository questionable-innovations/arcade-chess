//! The puzzle deck.
//!
//! The miner (`position-miner/`) already produces exactly what puzzle mode
//! needs and already emits JSON, so the contract with that workstream is one
//! command:
//!
//! ```sh
//! arcpos dump position-miner/data/curated.arcpos --json -n 100000 \
//!     > server/positions.json
//! ```
//!
//! Note `-n` defaults to 20 (`position-miner/src/main.rs`); without raising it
//! past the record count you silently get a twenty-position file.
//!
//! Both files are committed — the curated `.arcpos` is un-ignored specifically
//! so the JSON can be regenerated from a clean clone — because CapRover builds
//! from git and the deployed container gets whatever is in the tree.
//!
//! Loading never fails fatally. A missing, empty or malformed deck falls back to
//! the eight hand-checked endgames in `positions.fallback.json`, compiled into
//! the binary. Game mode must not go down with the content pipeline.

use std::collections::VecDeque;

use serde_json::Value;
use shakmaty::fen::Fen;
use shakmaty::{CastlingMode, Chess, Position};

use crate::util::random_u64;

/// The hand-checked emergency deck, compiled in so that a missing, empty or
/// malformed deck at runtime is a log line rather than a dead game mode. It is
/// deliberately *not* the file the Dockerfile ships to `POSITIONS_PATH`: keeping
/// the two separate is what lets `Deck::fallback` tell "playing the mined deck"
/// apart from "the mined deck did not load".
const EMBEDDED: &str = include_str!("../../positions.fallback.json");
/// Pieces a dealt position may have. This is the miner's format ceiling
/// (`position-miner/src/format.rs`, `MAX_PIECES`) — the packed nibble array
/// cannot describe a larger board, so nothing the miner emits can exceed it.
/// The miner's own `--max-pieces` sits below this; the check is here to catch a
/// hand-edited or foreign deck, not to re-impose the miner's taste.
const MAX_PIECES: usize = 16;
/// How many recently dealt positions to avoid repeating.
const RECENT_MEMORY: usize = 8;

#[derive(Clone, Debug)]
pub struct PositionRecord {
    pub id: String,
    pub fen: String,
    /// Engine score at mining time, white POV, when the record carries one.
    pub verified_cp: Option<i32>,
    /// How far the second-best root move falls behind the best — the sharpness
    /// signal the miner ranks on.
    pub drop_cp: Option<i32>,
}

pub struct Deck {
    positions: Vec<PositionRecord>,
    recent: VecDeque<usize>,
    pub source: String,
    pub skipped: usize,
    /// True when what got loaded is the hand-checked fallback rather than a
    /// mined deck — including when someone points `POSITIONS_PATH` at a copy of
    /// it, since the point is to report what is actually being dealt.
    pub fallback: bool,
}

impl Deck {
    /// Loads `POSITIONS_PATH`, falling back to the embedded deck. Logs whatever
    /// went wrong rather than dying: a broken content file must not take the
    /// game down with it.
    pub fn load() -> Deck {
        let path = std::env::var("POSITIONS_PATH").ok().filter(|p| !p.is_empty());
        if let Some(path) = path {
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    let (positions, skipped) = parse(&text);
                    if positions.is_empty() {
                        tracing::error!(
                            path = %path,
                            skipped,
                            "positions file yielded nothing usable; using the embedded deck"
                        );
                    } else {
                        tracing::info!(
                            path = %path,
                            loaded = positions.len(),
                            skipped,
                            "loaded puzzle positions"
                        );
                        return Deck {
                            fallback: text.trim() == EMBEDDED.trim(),
                            positions,
                            recent: VecDeque::new(),
                            source: path,
                            skipped,
                        };
                    }
                }
                Err(err) => tracing::error!(
                    path = %path,
                    %err,
                    "could not read POSITIONS_PATH; using the embedded deck"
                ),
            }
        }
        let (positions, skipped) = parse(EMBEDDED);
        // The embedded deck is validated by a unit test, so this really is
        // unreachable — but an empty deck would be a silent no-new-game.
        assert!(!positions.is_empty(), "embedded fallback deck is unusable");
        Deck {
            positions,
            recent: VecDeque::new(),
            source: "embedded".to_string(),
            skipped,
            fallback: true,
        }
    }

    pub fn len(&self) -> usize {
        self.positions.len()
    }

    /// Whether the deck in play is the hand-checked handful rather than a
    /// mined one. Surfaced as a `degraded` chip: eight positions is enough to
    /// demo with and not enough to hide that the mined deck failed to load.
    pub fn is_fallback(&self) -> bool {
        self.fallback
    }

    pub fn find(&self, id: &str) -> Option<&PositionRecord> {
        self.positions.iter().find(|p| p.id == id)
    }

    /// Picks uniformly at random, avoiding the last few dealt. Retrying rather
    /// than filtering keeps this O(1) on a five-thousand-position deck and
    /// degrades gracefully when the deck is smaller than the memory.
    pub fn deal(&mut self) -> PositionRecord {
        let count = self.positions.len();
        let mut index = 0;
        for _ in 0..16 {
            index = (random_u64() % count as u64) as usize;
            if !self.recent.contains(&index) {
                break;
            }
        }
        self.recent.push_back(index);
        while self.recent.len() > RECENT_MEMORY.min(count.saturating_sub(1)) {
            self.recent.pop_front();
        }
        self.positions[index].clone()
    }
}

/// Accepts a JSON array *or* JSON lines, because `arcpos dump --json` emits the
/// latter and a hand-written deck is naturally the former. Each record needs
/// only `fen`; unknown fields are ignored and malformed entries are counted
/// rather than fatal.
fn parse(text: &str) -> (Vec<PositionRecord>, usize) {
    let mut out = Vec::new();
    let mut skipped = 0usize;

    let values: Vec<Value> = match serde_json::from_str::<Value>(text) {
        Ok(Value::Array(items)) => items,
        _ => {
            let mut items = Vec::new();
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                match serde_json::from_str::<Value>(line) {
                    Ok(value) => items.push(value),
                    Err(_) => skipped += 1,
                }
            }
            items
        }
    };

    for (n, value) in values.into_iter().enumerate() {
        let Some(fen) = value.get("fen").and_then(Value::as_str) else {
            skipped += 1;
            continue;
        };
        if validate(fen).is_err() {
            skipped += 1;
            continue;
        }
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("p{n}"));
        out.push(PositionRecord {
            id,
            fen: fen.to_string(),
            verified_cp: value.get("verified_cp").and_then(Value::as_i64).map(|v| v as i32),
            drop_cp: value.get("drop_cp").and_then(Value::as_i64).map(|v| v as i32),
        });
    }
    (out, skipped)
}

/// Rejects everything that would make a five-move game pointless or impossible.
/// "Illegal position" is a real `shakmaty` concern and worth naming precisely:
/// it is `into_position` that rejects the side *not* to move standing in check.
pub fn validate(fen: &str) -> Result<Chess, &'static str> {
    let parsed: Fen = fen.parse().map_err(|_| "unparseable FEN")?;
    let pos: Chess = parsed
        .into_position(CastlingMode::Standard)
        .map_err(|_| "illegal position")?;
    if pos.board().occupied().count() > MAX_PIECES {
        return Err("too many pieces");
    }
    if pos.is_game_over() {
        return Err("already over");
    }
    if pos.legal_moves().len() < 2 {
        return Err("forced");
    }
    Ok(pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fallback is the deck that ships when nothing else does, so every
    /// entry has to survive the same gates a mined position does.
    #[test]
    fn the_embedded_deck_is_playable() {
        let (positions, skipped) = parse(EMBEDDED);
        assert_eq!(skipped, 0, "every embedded position must validate");
        assert!(positions.len() >= 8, "want a handful to choose between");
        for record in &positions {
            let pos = validate(&record.fen)
                .unwrap_or_else(|err| panic!("{}: {err}", record.id));
            assert!(
                pos.board().occupied().count() <= MAX_PIECES,
                "{} has too many pieces",
                record.id
            );
        }
    }

    /// The committed mined deck is what production actually deals, and it only
    /// reaches production through git — so a bad regeneration should fail here
    /// rather than quietly turn into a `positions_fallback` chip on the night.
    /// Compiled into the test binary only; the release build reads it from
    /// `POSITIONS_PATH`.
    #[test]
    fn the_shipped_deck_is_playable() {
        let (positions, skipped) = parse(include_str!("../../positions.json"));
        assert_eq!(skipped, 0, "every shipped position must validate");
        assert!(
            positions.len() > 8,
            "positions.json holds {} positions — did a dump run without -n?",
            positions.len()
        );
        for record in &positions {
            validate(&record.fen).unwrap_or_else(|err| panic!("{}: {err}", record.id));
        }
    }

    #[test]
    fn json_lines_and_arrays_both_load() {
        let lines = "{\"id\":\"a\",\"fen\":\"8/6k1/8/8/8/8/1R4K1/1r6 w - - 0 1\"}\n\
                     {\"id\":\"b\",\"fen\":\"8/5pk1/8/8/8/8/5PK1/8 w - - 0 1\"}\n";
        let (from_lines, skipped) = parse(lines);
        assert_eq!(skipped, 0);
        assert_eq!(from_lines.len(), 2);
        assert_eq!(from_lines[0].id, "a");

        let array = "[{\"fen\":\"8/6k1/8/8/8/8/1R4K1/1r6 w - - 0 1\"}]";
        let (from_array, _) = parse(array);
        assert_eq!(from_array.len(), 1);
        assert_eq!(from_array[0].id, "p0", "an id is synthesised when absent");
    }

    #[test]
    fn malformed_records_are_counted_not_fatal() {
        let text = "{\"fen\":\"8/6k1/8/8/8/8/1R4K1/1r6 w - - 0 1\"}\n\
                    not json at all\n\
                    {\"no_fen\":true}\n\
                    {\"fen\":\"garbage\"}\n";
        let (positions, skipped) = parse(text);
        assert_eq!(positions.len(), 1);
        assert_eq!(skipped, 3);
    }

    #[test]
    fn validation_names_what_it_rejects() {
        // Checkmate: nothing to play.
        assert_eq!(validate("7k/5KQ1/8/8/8/8/8/8 b - - 0 1"), Err("already over"));
        // A full opening position has thirty-two pieces.
        assert_eq!(
            validate("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1"),
            Err("too many pieces")
        );
        assert!(validate("not a fen").is_err());
    }

    /// The miner mines a *range* of board sizes, not a fixed eight, and its
    /// format tops out at sixteen pieces. Every size it can emit has to survive
    /// `validate` — a ceiling left behind at eight rejects the whole deck, and
    /// because that failure is non-fatal by design it surfaces only as a
    /// `positions_fallback` chip rather than as anything that stops a deploy.
    #[test]
    fn the_full_mined_piece_range_validates() {
        for fen in [
            "8/6k1/8/8/8/8/1R4K1/1r6 w - - 0 1",                    // 5
            "7R/2B5/p7/P2K4/8/8/r5k1/7q w - - 0 58",                // 8
            "4rk2/5pp1/p7/8/8/P1N4P/5PP1/4RK2 w - - 2 30",          // 12
            "r3k2r/pp3ppp/8/8/8/8/PP3PPP/R3K2R w KQkq - 4 20",      // 16
        ] {
            let pos = validate(fen).unwrap_or_else(|err| panic!("{fen}: {err}"));
            assert!(pos.board().occupied().count() <= MAX_PIECES);
        }
        // Seventeen is past the format ceiling, so it cannot have come from the
        // miner and is not trusted here either.
        assert_eq!(
            validate("r3k2r/pp3ppp/8/8/7N/8/PP3PPP/R3K2R w KQkq - 4 20"),
            Err("too many pieces")
        );
    }

    #[test]
    fn dealing_avoids_immediate_repeats() {
        let (positions, _) = parse(EMBEDDED);
        let mut deck = Deck {
            positions,
            recent: VecDeque::new(),
            source: "embedded".to_string(),
            skipped: 0,
            fallback: true,
        };
        let first = deck.deal().id;
        for _ in 0..4 {
            assert_ne!(deck.deal().id, first, "the recent memory holds it off");
        }
    }

    /// A one-position deck must still deal rather than loop looking for a
    /// position it has not just used.
    #[test]
    fn a_single_position_deck_still_deals() {
        let mut deck = Deck {
            positions: parse(EMBEDDED).0.into_iter().take(1).collect(),
            recent: VecDeque::new(),
            source: "embedded".to_string(),
            skipped: 0,
            fallback: true,
        };
        assert_eq!(deck.deal().id, deck.deal().id);
    }
}
