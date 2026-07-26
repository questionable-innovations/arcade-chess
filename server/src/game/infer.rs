//! The move matcher. Pure, no I/O, and the one component of puzzle mode that
//! cannot be debugged live on stage — hence the tests at the bottom.
//!
//! # The asymmetry that makes this tractable
//!
//! We are not identifying pieces. **We already know the position.** The server
//! holds the truth in `shakmaty`, so the job is matching a sensor delta against
//! a legal move list — a few dozen entries on the simplified boards puzzle
//! mode deals, which run from about eight pieces up to sixteen. The
//! board can never tell a rook from a king; piece identity lives exclusively in
//! the tracked game state.
//!
//! # Settled state, not transitions
//!
//! Nothing here tracks the *order* in which squares changed. Players slide,
//! hover, lift the wrong piece and put it back, and knock things over; sequence
//! matching drowns in that. Matching net occupancy across a settled window makes
//! lift-attacker-first, lift-victim-first, fumbles and put-it-back all invisible.
//!
//! # Three tiers
//!
//! Tier 1 is provably correct and has no tunable constants. Tier 2 resolves the
//! small, enumerable set of collisions Tier 1 can produce. Tier 3 is the only
//! tunable path and only ever runs once the hardware has already lied — so its
//! constants can be wrong without breaking the common case, which matters when
//! they are guesses until game time.

use shakmaty::san::SanPlus;
use shakmaty::uci::UciMove;
use shakmaty::{CastlingSide, Chess, Move, Position, Role, Square};

use super::observe::{neighbours, Occ, Pol, SQUARES};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confidence {
    /// Tier 1 or 2: the occupancy can only mean this.
    Certain,
    /// Tier 3: the best reading of a board that is disagreeing with itself.
    Likely,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Confidence::Certain => "certain",
            Confidence::Likely => "likely",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub uci: String,
    pub san: String,
    pub confidence: Confidence,
}

/// A committed move whose piece physically sits one square off. Never absorbed
/// silently — every later settle would inherit the disagreement and erode the
/// margin ply after ply until the game degraded into prompts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Offset {
    /// Where the game says the piece is.
    pub expected: u8,
    /// Where the sensors say it is.
    pub actual: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inference {
    /// The board still shows the current position: a piece was lifted and put
    /// back, or nothing happened at all.
    NoChange,
    Commit {
        uci: String,
        san: String,
        confidence: Confidence,
        offset: Option<Offset>,
    },
    /// Several legal moves fit equally well; the operator taps one.
    Ambiguous {
        kind: &'static str,
        options: Vec<Candidate>,
    },
    /// Nothing legal fits. `squares` are the disagreements against the current
    /// position, for flashing red and drawing the on-screen diff.
    Mismatch {
        squares: Vec<u8>,
        options: Vec<Candidate>,
    },
}

/// What the board is currently saying, in game coordinates.
pub struct Observation<'a> {
    pub occ: &'a [Occ; SQUARES],
    /// Squares that may be believed. Unknown squares can neither confirm nor
    /// deny, and never contribute negative evidence.
    pub known: &'a [bool; SQUARES],
    /// Squares seen momentarily empty or wobbling since the last commit.
    pub journal: &'a [bool; SQUARES],
    /// The per-piece polarity fingerprint learned at game start and migrated
    /// with every commit.
    pub pol_tag: &'a [Option<Pol>; SQUARES],
}

/// Every constant the matcher scores with, in one place.
///
/// These are calibrated *against each other*, not independently: with the
/// shipped defaults, `unreadable_penalty` (2.0) sits above `max_distance` (1.0)
/// precisely so that a masked or offline destination is a hard veto on
/// committing onto it. Raising `max_distance` past `unreadable_penalty` to "be
/// more permissive" silently re-enables the failure where one dead sensor
/// quietly swallows every move played onto its square, so the two move together
/// or not at all — see `config::limits()`, which advertises that ceiling.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub max_distance: f64,
    pub margin: f64,
    /// The off-centre pair, credited once rather than counted as two
    /// disagreements.
    pub neighbour_credit: f64,
    /// No confirmed arrival at all, because the destination cannot be read.
    pub unreadable_penalty: f64,
    /// Destination reads empty. Half of `unreadable_penalty` on purpose: an
    /// empty square already contributed one as a plain disagreement, whereas a
    /// masked one contributed nothing and owes both halves.
    pub empty_penalty: f64,
    /// Polarity fingerprint matches, so the same physical piece is still there.
    /// Subtracted. A *mismatch* is never added — an off-centre piece can read
    /// genuinely inverted, so it is not evidence against anything.
    pub polarity_credit: f64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            max_distance: 1.0,
            margin: 1.0,
            neighbour_credit: 0.5,
            unreadable_penalty: 2.0,
            empty_penalty: 1.0,
            polarity_credit: 0.5,
        }
    }
}

struct Hypothesis {
    m: Move,
    uci: String,
    san: String,
    occ: [bool; SQUARES],
}

impl Hypothesis {
    fn candidate(&self, confidence: Confidence) -> Candidate {
        Candidate {
            uci: self.uci.clone(),
            san: self.san.clone(),
            confidence,
        }
    }
}

pub fn occupancy(pos: &Chess) -> [bool; SQUARES] {
    let mut out = [false; SQUARES];
    for sq in pos.board().occupied() {
        out[usize::from(u8::from(sq))] = true;
    }
    out
}

fn hypotheses(pos: &Chess) -> Vec<Hypothesis> {
    pos.legal_moves()
        .into_iter()
        .map(|m| {
            let mut next = pos.clone();
            next.play_unchecked(&m);
            Hypothesis {
                uci: UciMove::from_standard(&m).to_string(),
                san: SanPlus::from_move(pos.clone(), &m).to_string(),
                occ: occupancy(&next),
                m,
            }
        })
        .collect()
}

/// Squares where the board disagrees with an expectation, ignoring everything
/// the board cannot speak for.
fn disagreements(expected: &[bool; SQUARES], obs: &Observation) -> Vec<u8> {
    (0..SQUARES)
        .filter(|&sq| obs.known[sq] && obs.occ[sq].occupied() != expected[sq])
        .map(|sq| sq as u8)
        .collect()
}

fn agrees(expected: &[bool; SQUARES], obs: &Observation) -> bool {
    (0..SQUARES).all(|sq| !obs.known[sq] || obs.occ[sq].occupied() == expected[sq])
}

/// Whether the board can actually confirm a piece arrived where a move sends
/// it. A masked or offline destination cannot: "the piece moved there" and "the
/// piece is in the player's hand" look identical, and committing on that is how
/// one dead sensor quietly eats every move played onto it.
fn arrival_is_visible(m: &Move, obs: &Observation) -> bool {
    obs.known[usize::from(u8::from(m.to()))]
}

pub fn infer(pos: &Chess, obs: &Observation, params: &Params) -> Inference {
    let hyps = hypotheses(pos);
    if hyps.is_empty() {
        return Inference::NoChange;
    }
    let current = occupancy(pos);

    // The steady state, and the lift-and-replace no-op. Checked first: a move
    // whose from- and to-squares are both unknown is indistinguishable from
    // nothing happening, and "nothing happened" is the safe reading.
    if agrees(&current, obs) {
        return Inference::NoChange;
    }

    // ── Tier 1: exact match ────────────────────────────────────────────────
    //
    // A quiet move empties `from` and fills `to`, so distinct (from, to) pairs
    // give distinct occupancy sets and a quiet move is unique. Castling changes
    // four squares and en passant three — both unique. Captures reduce the
    // occupied count by one while quiet moves preserve it, so the two classes
    // can never collide. The only collisions are several legal captures by the
    // same piece, and promotion piece choice.
    let exact: Vec<usize> = hyps
        .iter()
        .enumerate()
        .filter(|(_, h)| arrival_is_visible(&h.m, obs) && agrees(&h.occ, obs))
        .map(|(i, _)| i)
        .collect();

    match exact.len() {
        1 => {
            let h = &hyps[exact[0]];
            Inference::Commit {
                uci: h.uci.clone(),
                san: h.san.clone(),
                confidence: Confidence::Certain,
                offset: None,
            }
        }
        0 => tier3(&hyps, &current, obs, params),
        _ => tier2(&hyps, exact, obs),
    }
}

/// Resolves a Tier-1 collision with two independent discriminators, both free.
fn tier2(hyps: &[Hypothesis], exact: Vec<usize>, obs: &Observation) -> Inference {
    // Occupancy cannot see which piece a pawn promoted to, so collapse each
    // (from, to) pair onto its queen. Undo plus a manual move covers the
    // crowd-pleasing underpromotion.
    let mut reduced: Vec<usize> = Vec::new();
    for &i in &exact {
        let (from, to) = (hyps[i].m.from(), hyps[i].m.to());
        match reduced
            .iter()
            .position(|&j| hyps[j].m.from() == from && hyps[j].m.to() == to)
        {
            None => reduced.push(i),
            Some(slot) => {
                if hyps[i].m.promotion() == Some(Role::Queen) {
                    reduced[slot] = i;
                }
            }
        }
    }
    if reduced.len() == 1 {
        let h = &hyps[reduced[0]];
        return Inference::Commit {
            uci: h.uci.clone(),
            san: h.san.clone(),
            confidence: Confidence::Certain,
            offset: None,
        };
    }

    // The transient journal. The victim was physically lifted at some point, so
    // its square shows a momentary empty or wobble; the other candidate
    // destinations were never touched.
    // `known` is required here for the same reason every other term requires it:
    // an unbelievable square must not supply *positive* evidence either. Without
    // it a square that went unknown mid-ply — a quadrant dropping between the
    // lift and the placement — could still carry a stale journal entry and name
    // the wrong capture with full confidence. Losing the discriminator is the
    // correct outcome: it falls through to the two-button prompt, which is what
    // "journal lost to a seq gap" is supposed to do.
    let journalled: Vec<usize> = reduced
        .iter()
        .copied()
        .filter(|&i| {
            let to = usize::from(u8::from(hyps[i].m.to()));
            obs.known[to] && obs.journal[to]
        })
        .collect();
    if journalled.len() == 1 {
        let h = &hyps[journalled[0]];
        return Inference::Commit {
            uci: h.uci.clone(),
            san: h.san.clone(),
            confidence: Confidence::Certain,
            offset: None,
        };
    }

    // Polarity. If the attacker's fingerprint now reads on a destination, that
    // capture is confirmed and named. Used only additively: a *mismatching*
    // polarity is not evidence against anything, because per the firmware notes
    // a piece sitting off-centre can genuinely read inverted.
    let confirmed: Vec<usize> = reduced
        .iter()
        .copied()
        .filter(|&i| {
            let Some(from) = hyps[i].m.from() else {
                return false;
            };
            let to = usize::from(u8::from(hyps[i].m.to()));
            match (obs.pol_tag[usize::from(u8::from(from))], obs.occ[to].polarity()) {
                (Some(tag), Some(now)) => tag == now && obs.known[to],
                _ => false,
            }
        })
        .collect();
    if confirmed.len() == 1 {
        let h = &hyps[confirmed[0]];
        return Inference::Commit {
            uci: h.uci.clone(),
            san: h.san.clone(),
            confidence: Confidence::Certain,
            offset: None,
        };
    }

    Inference::Ambiguous {
        kind: "capture",
        options: reduced
            .iter()
            .map(|&i| hyps[i].candidate(Confidence::Likely))
            .collect(),
    }
}

/// Where each piece could legally have gone, as a bitboard per origin square.
/// Used to deny off-centre credit when "the player simply played the other
/// move" is already an available explanation — see [`distance`].
fn destinations_by_origin(hyps: &[Hypothesis]) -> [u64; SQUARES] {
    let mut out = [0u64; SQUARES];
    for h in hyps {
        if let Some(from) = h.m.from() {
            out[usize::from(u8::from(from))] |= 1u64 << u8::from(h.m.to());
        }
    }
    out
}

/// Additive score for one hypothesis. Every term is in the same unit and the
/// whole thing is a sum, so there is exactly one way to implement it — which
/// matters for constants that cannot be validated before game time.
fn distance(
    h: &Hypothesis,
    obs: &Observation,
    dests: &[u64; SQUARES],
    params: &Params,
) -> (f64, Option<Offset>) {
    let mut disagree = disagreements(&h.occ, obs);
    let mut score = 0.0f64;
    let mut offset = None;

    // The off-centre pair, counted once rather than twice: the magnet is at the
    // piece's base and the sensor at the square's centre, so a piece dropped
    // half a square over reads next door. Polarity is ignored on the neighbour —
    // the axial field of a dipole reverses sign past the magic angle, so a
    // straddling piece registers on its neighbour with the sign flipped.
    //
    // The credit is denied when the stray square is itself a legal destination
    // for the same piece. Along a rook's file every square is adjacent to the
    // next, so without that guard "Rb5, dropped on b6" scores nearly as well as
    // "Rb6, placed properly" and the two blur into a prompt. If the piece could
    // legally have gone where it is sitting, that is the simpler reading and
    // there is nothing off-centre to explain.
    if !h.m.is_castle() {
        let to = u8::from(h.m.to());
        let to_idx = usize::from(to);
        let reachable = h
            .m
            .from()
            .map(|from| dests[usize::from(u8::from(from))])
            .unwrap_or(0);
        if h.occ[to_idx] && obs.known[to_idx] && !obs.occ[to_idx].occupied() {
            let stray: Vec<u8> = neighbours(to)
                .into_iter()
                .filter(|&n| {
                    let bit = usize::from(n);
                    obs.known[bit]
                        && obs.occ[bit].occupied()
                        && !h.occ[bit]
                        && reachable & (1u64 << n) == 0
                })
                .collect();
            if stray.len() == 1 {
                disagree.retain(|&sq| sq != to && sq != stray[0]);
                score += params.neighbour_credit;
                offset = Some(Offset {
                    expected: to,
                    actual: stray[0],
                });
            }
        }
    }

    score += disagree.len() as f64;

    // No confirmed arrival costs two, however the destination fails to confirm
    // it. Every other square could be a sensor telling lies, but the
    // destination is the move's own evidence.
    //
    // Symmetric on purpose: a square that reads empty already contributed one
    // above, so it takes one more, while a masked or offline square contributed
    // nothing and takes both. Without the first half, lifting an attacker and
    // its victim together — the ordinary middle of a capture — commits the
    // capture before the piece lands. Without the second, one masked square
    // swallows every move played onto it, because "moved there" and "in the
    // player's hand" become indistinguishable.
    let to = usize::from(u8::from(h.m.to()));
    if offset.is_none() {
        if !obs.known[to] {
            score += params.unreadable_penalty;
        } else if disagree.contains(&(to as u8)) && !obs.occ[to].occupied() {
            score += params.empty_penalty;
        }
    }

    // One polarity term, skipped for promotions (the promoted queen is a
    // different piece with its own magnet) and when either tag is unknown.
    if h.m.promotion().is_none() {
        if let Some(from) = h.m.from() {
            let to = usize::from(u8::from(h.m.to()));
            if let (Some(tag), Some(now)) = (
                obs.pol_tag[usize::from(u8::from(from))],
                obs.occ[to].polarity(),
            ) {
                if obs.known[to] && tag == now {
                    score -= params.polarity_credit;
                }
            }
        }
    }

    (score.max(0.0), offset)
}

/// No exact match, so the hardware is lying somewhere. A strict matcher fails
/// *totally* and *silently* here: one square stuck confidently-occupied by
/// baseline drift reads `valid == true` throughout and poisons every hypothesis
/// for the rest of the game. Score them instead.
fn tier3(
    hyps: &[Hypothesis],
    current: &[bool; SQUARES],
    obs: &Observation,
    params: &Params,
) -> Inference {
    let dests = destinations_by_origin(hyps);
    let mut scored: Vec<(f64, usize, Option<Offset>)> = hyps
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let (d, offset) = distance(h, obs, &dests, params);
            (d, i, offset)
        })
        .collect();
    scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let (best, index, offset) = scored[0];
    let runner_up = scored.get(1).map(|s| s.0).unwrap_or(f64::INFINITY);
    if best <= params.max_distance && runner_up - best >= params.margin {
        let h = &hyps[index];
        return Inference::Commit {
            uci: h.uci.clone(),
            san: h.san.clone(),
            confidence: Confidence::Likely,
            offset,
        };
    }

    Inference::Mismatch {
        squares: disagreements(current, obs),
        options: scored
            .iter()
            .take(3)
            .map(|&(_, i, _)| hyps[i].candidate(Confidence::Likely))
            .collect(),
    }
}

/// Moves the per-piece polarity fingerprints along with the pieces. Without
/// this the table is stale after move one: the tag would still describe
/// whatever used to stand on a square rather than what stands there now.
pub fn migrate_pol_tags(
    tags: &mut [Option<Pol>; SQUARES],
    turn: shakmaty::Color,
    m: &Move,
    observed: &[Occ; SQUARES],
) {
    let idx = |sq: Square| usize::from(u8::from(sq));
    match *m {
        Move::Castle { king, rook } => {
            let side = CastlingSide::from_king_side(king < rook);
            let king_to = idx(side.king_to(turn));
            let rook_to = idx(side.rook_to(turn));
            let (king_tag, rook_tag) = (tags[idx(king)], tags[idx(rook)]);
            tags[idx(king)] = None;
            tags[idx(rook)] = None;
            tags[king_to] = king_tag;
            tags[rook_to] = rook_tag;
        }
        Move::EnPassant { from, to } => {
            // The captured pawn stands beside the destination, not on it.
            let victim = idx(Square::from_coords(to.file(), from.rank()));
            tags[victim] = None;
            tags[idx(to)] = tags[idx(from)];
            tags[idx(from)] = None;
        }
        Move::Normal {
            from,
            to,
            promotion,
            ..
        } => {
            // A promoted queen is a different physical piece with its own
            // magnet, so relearn the tag from whatever actually landed.
            tags[idx(to)] = if promotion.is_some() {
                observed[idx(to)].polarity()
            } else {
                tags[idx(from)]
            };
            tags[idx(from)] = None;
        }
        Move::Put { to, .. } => {
            tags[idx(to)] = observed[idx(to)].polarity();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shakmaty::fen::Fen;
    use shakmaty::CastlingMode;

    struct Fixture {
        occ: [Occ; SQUARES],
        known: [bool; SQUARES],
        journal: [bool; SQUARES],
        pol_tag: [Option<Pol>; SQUARES],
    }

    impl Fixture {
        /// Starts from "the board shows exactly this position", with every
        /// square known, nothing journalled and no fingerprints learned.
        fn of(pos: &Chess) -> Fixture {
            let mut occ = [Occ::Empty; SQUARES];
            for sq in pos.board().occupied() {
                occ[usize::from(u8::from(sq))] = Occ::Pos;
            }
            Fixture {
                occ,
                known: [true; SQUARES],
                journal: [false; SQUARES],
                pol_tag: [None; SQUARES],
            }
        }

        fn view(&self) -> Observation<'_> {
            Observation {
                occ: &self.occ,
                known: &self.known,
                journal: &self.journal,
                pol_tag: &self.pol_tag,
            }
        }

        fn set(&mut self, name: &str, value: Occ) -> &mut Self {
            self.occ[sq(name)] = value;
            self
        }

        fn lift(&mut self, name: &str) -> &mut Self {
            self.occ[sq(name)] = Occ::Empty;
            self.journal[sq(name)] = true;
            self
        }

        fn tag(&mut self, name: &str, pol: Pol) -> &mut Self {
            self.pol_tag[sq(name)] = Some(pol);
            self
        }
    }

    fn sq(name: &str) -> usize {
        usize::from(u8::from(Square::from_ascii(name.as_bytes()).expect("square")))
    }

    fn position(fen: &str) -> Chess {
        fen.parse::<Fen>()
            .expect("fen")
            .into_position(CastlingMode::Standard)
            .expect("legal position")
    }

    /// Plays a physical quiet move on the fixture: the piece leaves `from` and
    /// arrives at `to`, carrying its polarity with it.
    fn slide(f: &mut Fixture, from: &str, to: &str) {
        let pol = f.occ[sq(from)];
        f.lift(from);
        f.occ[sq(to)] = pol;
    }

    fn committed(inf: &Inference) -> &str {
        match inf {
            Inference::Commit { uci, .. } => uci,
            other => panic!("expected a commit, got {other:?}"),
        }
    }

    #[test]
    fn quiet_move_is_tier_one() {
        let pos = position("8/6k1/8/8/8/8/1R4K1/1r6 w - - 0 1");
        let mut f = Fixture::of(&pos);
        slide(&mut f, "b2", "b6");
        let inf = infer(&pos, &f.view(), &Params::default());
        assert_eq!(committed(&inf), "b2b6");
        assert!(matches!(
            inf,
            Inference::Commit {
                confidence: Confidence::Certain,
                offset: None,
                ..
            }
        ));
    }

    #[test]
    fn an_untouched_board_commits_nothing() {
        let pos = position("8/6k1/8/8/8/8/1R4K1/1r6 w - - 0 1");
        let f = Fixture::of(&pos);
        assert_eq!(infer(&pos, &f.view(), &Params::default()), Inference::NoChange);
    }

    /// Lift a piece, put it back — including on the wrong way round, which flips
    /// its polarity. Tier 1 is occupancy-only, so both are invisible.
    #[test]
    fn lift_and_replace_is_a_no_op_even_when_polarity_flips() {
        let pos = position("8/6k1/8/8/8/8/1R4K1/1r6 w - - 0 1");
        let mut f = Fixture::of(&pos);
        f.tag("b2", Pol::Pos);
        f.journal[sq("b2")] = true;
        f.set("b2", Occ::Neg);
        assert_eq!(infer(&pos, &f.view(), &Params::default()), Inference::NoChange);
    }

    /// The capture lift order is invisible to settled-state matching: whichever
    /// piece left the board first, the net occupancy is the same.
    #[test]
    fn capture_commits_whichever_order_the_pieces_were_lifted() {
        let pos = position("8/6k1/8/3r4/8/8/3R2K1/8 w - - 0 1");
        for victim_first in [true, false] {
            let mut f = Fixture::of(&pos);
            if victim_first {
                f.lift("d5");
                slide(&mut f, "d2", "d5");
            } else {
                slide(&mut f, "d2", "d5");
            }
            assert_eq!(committed(&infer(&pos, &f.view(), &Params::default())), "d2d5");
        }
    }

    #[test]
    fn castling_is_tier_one() {
        let pos = position("4k3/8/8/8/8/8/8/R3K2R w KQ - 0 1");
        let mut f = Fixture::of(&pos);
        slide(&mut f, "e1", "g1");
        slide(&mut f, "h1", "f1");
        assert_eq!(committed(&infer(&pos, &f.view(), &Params::default())), "e1g1");
    }

    #[test]
    fn en_passant_is_tier_one() {
        let pos = position("8/6k1/8/3pP3/8/8/8/6K1 w - d6 0 1");
        let mut f = Fixture::of(&pos);
        slide(&mut f, "e5", "d6");
        f.lift("d5");
        assert_eq!(committed(&infer(&pos, &f.view(), &Params::default())), "e5d6");
    }

    /// A pawn on e4 taking d5 or f5 leaves identical occupancy. The victim was
    /// lifted, so the journal names it.
    #[test]
    fn ambiguous_capture_resolves_by_journal() {
        let pos = position("8/6k1/8/3p1p2/4P3/8/6K1/8 w - - 0 1");
        let mut f = Fixture::of(&pos);
        f.lift("d5");
        slide(&mut f, "e4", "d5");
        assert_eq!(committed(&infer(&pos, &f.view(), &Params::default())), "e4d5");
    }

    /// Same collision, journal lost to a seq gap. The attacker's fingerprint
    /// now reads on d5, which is decisive.
    #[test]
    fn ambiguous_capture_resolves_by_polarity() {
        let pos = position("8/6k1/8/3p1p2/4P3/8/6K1/8 w - - 0 1");
        let mut f = Fixture::of(&pos);
        f.tag("e4", Pol::Neg).tag("d5", Pol::Pos).tag("f5", Pol::Pos);
        f.occ[sq("e4")] = Occ::Neg;
        slide(&mut f, "e4", "d5");
        f.journal = [false; SQUARES];
        assert_eq!(committed(&infer(&pos, &f.view(), &Params::default())), "e4d5");
    }

    /// Neither discriminator fires: two big buttons on screen, ten seconds of
    /// operator attention, not a broken game.
    #[test]
    fn ambiguous_capture_with_no_evidence_asks() {
        let pos = position("8/6k1/8/3p1p2/4P3/8/6K1/8 w - - 0 1");
        let mut f = Fixture::of(&pos);
        slide(&mut f, "e4", "d5");
        f.journal = [false; SQUARES];
        match infer(&pos, &f.view(), &Params::default()) {
            Inference::Ambiguous { kind, options } => {
                assert_eq!(kind, "capture");
                let ucis: Vec<&str> = options.iter().map(|o| o.uci.as_str()).collect();
                assert_eq!(ucis.len(), 2);
                assert!(ucis.contains(&"e4d5") && ucis.contains(&"e4f5"));
            }
            other => panic!("expected a prompt, got {other:?}"),
        }
    }

    /// Occupancy cannot see the promotion choice, so auto-queen rather than
    /// prompting on every promotion.
    #[test]
    fn promotion_auto_queens() {
        let pos = position("6k1/4P3/8/8/8/8/8/6K1 w - - 0 1");
        let mut f = Fixture::of(&pos);
        slide(&mut f, "e7", "e8");
        assert_eq!(committed(&infer(&pos, &f.view(), &Params::default())), "e7e8q");
    }

    /// Both endpoints on a dead quadrant: no evidence, so the move list is the
    /// input method and detection stays quiet rather than guessing.
    #[test]
    fn a_move_entirely_inside_an_offline_quadrant_is_invisible() {
        let pos = position("8/6k1/8/8/8/8/1R4K1/1r6 w - - 0 1");
        let mut f = Fixture::of(&pos);
        slide(&mut f, "b2", "b3");
        // Node 0 covers a1-d4, which holds both endpoints.
        for sq in 0..SQUARES {
            let (rank, file) = (sq / 8, sq % 8);
            if rank < 4 && file < 4 {
                f.known[sq] = false;
            }
        }
        assert_eq!(infer(&pos, &f.view(), &Params::default()), Inference::NoChange);
    }

    /// One square stuck confidently occupied poisons every hypothesis. The true
    /// move still wins by a clear margin, and commits with a badge.
    #[test]
    fn a_stuck_square_falls_through_to_tier_three() {
        let pos = position("8/6k1/8/8/8/8/1R4K1/1r6 w - - 0 1");
        let mut f = Fixture::of(&pos);
        f.set("h5", Occ::Pos); // baseline drift: nothing is really there
        slide(&mut f, "b2", "b6");
        match infer(&pos, &f.view(), &Params::default()) {
            Inference::Commit {
                uci, confidence, ..
            } => {
                assert_eq!(uci, "b2b6");
                assert_eq!(confidence, Confidence::Likely);
            }
            other => panic!("expected a tier-3 commit, got {other:?}"),
        }
    }

    /// A piece dropped half a square over reads next door. Credit the pair once
    /// and report the offset, so the game can ask for the nudge.
    #[test]
    fn a_piece_dropped_one_square_off_earns_neighbour_credit() {
        let pos = position("8/6k1/8/8/8/8/1R4K1/8 w - - 0 1");
        let mut f = Fixture::of(&pos);
        f.lift("b2");
        f.set("c6", Occ::Pos); // meant for b6
        match infer(&pos, &f.view(), &Params::default()) {
            Inference::Commit { uci, offset, .. } => {
                assert_eq!(uci, "b2b6");
                assert_eq!(
                    offset,
                    Some(Offset {
                        expected: u8::from(Square::B6),
                        actual: u8::from(Square::C6),
                    })
                );
            }
            other => panic!("expected a neighbour-credit commit, got {other:?}"),
        }
    }

    /// The credit is denied when the stray square is somewhere the piece could
    /// legally have gone. Along a rook's own file every square is adjacent to
    /// the next, so without that guard "Rb5, dropped on b6" would score almost
    /// as well as "Rb6, placed properly" and the true move would lose its
    /// margin to its own neighbours.
    #[test]
    fn off_centre_credit_is_denied_when_the_piece_could_have_gone_there() {
        let pos = position("8/6k1/8/8/8/8/1R4K1/8 w - - 0 1");
        let mut f = Fixture::of(&pos);
        f.set("h5", Occ::Pos); // a stuck sensor, to force tier 3
        slide(&mut f, "b2", "b6");
        assert_eq!(committed(&infer(&pos, &f.view(), &Params::default())), "b2b6");
    }

    /// Off-centre credit is only worth half a square of doubt, and a capture
    /// that explains the same board is worth a whole one. When those land
    /// inside the margin the honest answer is a prompt, not a guess.
    #[test]
    fn an_off_centre_drop_that_a_capture_also_explains_asks() {
        let pos = position("8/6k1/8/8/8/8/1R4K1/1r6 w - - 0 1");
        let mut f = Fixture::of(&pos);
        f.lift("b2");
        f.set("c6", Occ::Pos);
        match infer(&pos, &f.view(), &Params::default()) {
            Inference::Mismatch { options, .. } => {
                assert_eq!(options[0].uci, "b2b6", "the likeliest reading leads");
            }
            other => panic!("expected a prompt, got {other:?}"),
        }
    }

    /// The ordinary middle of a capture: both pieces are off the board and the
    /// attacker has not landed yet. It looks exactly like the capture minus one
    /// square, so without weighting the destination it would commit early — and
    /// then be wrong if the player puts the attacker back instead.
    #[test]
    fn a_capture_is_not_committed_until_the_attacker_lands() {
        let pos = position("8/6k1/8/3r4/8/8/3R2K1/8 w - - 0 1");
        let mut f = Fixture::of(&pos);
        f.lift("d2");
        f.lift("d5");
        match infer(&pos, &f.view(), &Params::default()) {
            Inference::Mismatch { squares, options } => {
                assert!(squares.contains(&u8::from(Square::D2)));
                assert!(squares.contains(&u8::from(Square::D5)));
                assert_eq!(options[0].uci, "d2d5", "but it is still the likeliest reading");
            }
            other => panic!("expected a prompt, got {other:?}"),
        }
        // Land the attacker and it commits, with no prompt at all.
        f.set("d5", Occ::Pos);
        assert_eq!(committed(&infer(&pos, &f.view(), &Params::default())), "d2d5");
    }

    /// A masked or offline destination cannot confirm anything, so a piece
    /// lifted next to one must not be read as having moved onto it. Otherwise a
    /// single dead sensor quietly eats every move played its way — and the
    /// square is masked precisely because it is the one that lies.
    #[test]
    fn a_move_onto_an_unreadable_square_is_never_committed() {
        let pos = position("8/6k1/8/8/8/8/6K1/R7 w - - 0 1");
        let mut f = Fixture::of(&pos);
        f.known[sq("b1")] = false;
        f.lift("a1");
        match infer(&pos, &f.view(), &Params::default()) {
            Inference::Mismatch { squares, .. } => {
                assert_eq!(squares, vec![u8::from(Square::A1)]);
            }
            other => panic!("expected a prompt, got {other:?}"),
        }
        // With the sensor believed again, the same board commits immediately.
        f.known[sq("b1")] = true;
        f.set("b1", Occ::Pos);
        assert_eq!(committed(&infer(&pos, &f.view(), &Params::default())), "a1b1");
    }

    /// Two-handed fiddling, or a physically illegal move: nothing fits, the
    /// position is left untouched and the diff goes on screen.
    #[test]
    fn an_illegal_settle_is_a_mismatch() {
        let pos = position("8/6k1/8/8/8/8/1R4K1/1r6 w - - 0 1");
        let mut f = Fixture::of(&pos);
        slide(&mut f, "b2", "b6");
        slide(&mut f, "b1", "h1"); // black moved too, on white's turn
        match infer(&pos, &f.view(), &Params::default()) {
            Inference::Mismatch { squares, options } => {
                assert!(squares.contains(&u8::from(Square::H1)));
                assert!(!options.is_empty(), "the prompt still offers a way out");
            }
            other => panic!("expected a mismatch, got {other:?}"),
        }
    }

    #[test]
    fn pol_tags_follow_the_piece_across_a_capture() {
        let pos = position("8/6k1/8/3r4/8/8/3R2K1/8 w - - 0 1");
        let mut tags = [None; SQUARES];
        tags[sq("d2")] = Some(Pol::Neg);
        tags[sq("d5")] = Some(Pol::Pos);
        let m = pos
            .legal_moves()
            .into_iter()
            .find(|m| UciMove::from_standard(m).to_string() == "d2d5")
            .expect("capture is legal");
        migrate_pol_tags(&mut tags, pos.turn(), &m, &[Occ::Empty; SQUARES]);
        assert_eq!(tags[sq("d2")], None);
        assert_eq!(tags[sq("d5")], Some(Pol::Neg), "the attacker's tag wins");
    }

    #[test]
    fn pol_tags_relearn_across_a_promotion() {
        let pos = position("6k1/4P3/8/8/8/8/8/6K1 w - - 0 1");
        let mut tags = [None; SQUARES];
        tags[sq("e7")] = Some(Pol::Pos);
        let mut observed = [Occ::Empty; SQUARES];
        observed[sq("e8")] = Occ::Neg;
        let m = pos
            .legal_moves()
            .into_iter()
            .find(|m| UciMove::from_standard(m).to_string() == "e7e8q")
            .expect("promotion is legal");
        migrate_pol_tags(&mut tags, pos.turn(), &m, &observed);
        assert_eq!(
            tags[sq("e8")],
            Some(Pol::Neg),
            "the queen is a different magnet"
        );
    }

    #[test]
    fn pol_tags_follow_both_pieces_through_castling() {
        let pos = position("4k3/8/8/8/8/8/8/R3K2R w KQ - 0 1");
        let mut tags = [None; SQUARES];
        tags[sq("e1")] = Some(Pol::Pos);
        tags[sq("h1")] = Some(Pol::Neg);
        let m = pos
            .legal_moves()
            .into_iter()
            .find(|m| UciMove::from_standard(m).to_string() == "e1g1")
            .expect("castling is legal");
        migrate_pol_tags(&mut tags, pos.turn(), &m, &[Occ::Empty; SQUARES]);
        assert_eq!(tags[sq("g1")], Some(Pol::Pos));
        assert_eq!(tags[sq("f1")], Some(Pol::Neg));
        assert_eq!(tags[sq("e1")], None);
        assert_eq!(tags[sq("h1")], None);
    }

    #[test]
    fn en_passant_clears_the_victim_tag_beside_the_destination() {
        let pos = position("8/6k1/8/3pP3/8/8/8/6K1 w - d6 0 1");
        let mut tags = [None; SQUARES];
        tags[sq("e5")] = Some(Pol::Pos);
        tags[sq("d5")] = Some(Pol::Neg);
        let m = pos
            .legal_moves()
            .into_iter()
            .find(|m| UciMove::from_standard(m).to_string() == "e5d6")
            .expect("en passant is legal");
        migrate_pol_tags(&mut tags, pos.turn(), &m, &[Occ::Empty; SQUARES]);
        assert_eq!(tags[sq("d5")], None);
        assert_eq!(tags[sq("d6")], Some(Pol::Pos));
    }
}
