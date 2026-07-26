//! Packing a live board into the bytes an `ARCPOS1` record stores, and
//! unpacking it back into FEN / ASCII without needing a chess library.
//!
//! The reader side is deliberately dependency-free: anything that can read the
//! file can also render the board, which is what makes a record self-contained.

use crate::format::{PieceNibbles, Record, MAX_PIECES};
use shakmaty::{Bitboard, Board, Color, Position, Role, Square};

/// The board state, packed exactly as it lands in a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Packed {
    pub occupied: u64,
    pub pieces: PieceNibbles,
    pub stm: u8,
    pub castling: u8,
    pub ep_square: u8,
}

/// Piece codes stored in the nibble array. Index 0 is unused so that an
/// all-zero nibble tail is unambiguously "no more pieces".
pub const PIECE_CHARS: [char; 13] = [
    '.', 'P', 'N', 'B', 'R', 'Q', 'K', 'p', 'n', 'b', 'r', 'q', 'k',
];

fn piece_code(color: Color, role: Role) -> u8 {
    let base = match role {
        Role::Pawn => 1,
        Role::Knight => 2,
        Role::Bishop => 3,
        Role::Rook => 4,
        Role::Queen => 5,
        Role::King => 6,
    };
    if color == Color::White {
        base
    } else {
        base + 6
    }
}

/// Packs a board. Returns `None` if the board holds more than
/// [`MAX_PIECES`] pieces, since that is all the nibble array can describe.
pub fn pack_board(board: &Board, stm: Color, castling: Bitboard, ep: Option<Square>) -> Option<Packed> {
    let occupied = board.occupied();
    if occupied.count() as u32 > MAX_PIECES {
        return None;
    }
    let mut pieces = [0u8; 8];
    for (slot, square) in occupied.into_iter().enumerate() {
        let piece = board.piece_at(square)?;
        let code = piece_code(piece.color, piece.role);
        if slot % 2 == 0 {
            pieces[slot / 2] |= code;
        } else {
            pieces[slot / 2] |= code << 4;
        }
    }

    let mut castle_mask = 0u8;
    if castling.contains(Square::H1) {
        castle_mask |= 1;
    }
    if castling.contains(Square::A1) {
        castle_mask |= 2;
    }
    if castling.contains(Square::H8) {
        castle_mask |= 4;
    }
    if castling.contains(Square::A8) {
        castle_mask |= 8;
    }

    Some(Packed {
        occupied: occupied.into(),
        pieces,
        stm: if stm == Color::White { 0 } else { 1 },
        castling: castle_mask,
        ep_square: ep.map_or(255, |s| u8::from(s)),
    })
}

/// Convenience wrapper for a `shakmaty` position.
pub fn pack_position<P: Position>(pos: &P) -> Option<Packed> {
    pack_board(
        pos.board(),
        pos.turn(),
        pos.castles().castling_rights(),
        pos.maybe_ep_square(),
    )
}

/// The record `id`: FNV-1a over the 19 packed position bytes.
///
/// Only the board, side to move, castling rights and en-passant target feed
/// the hash — clocks and move numbers do not — so the same position reached by
/// two different games collapses onto one id. That is what makes the file
/// deduplicable.
pub fn position_id(p: &Packed) -> u64 {
    let mut bytes = [0u8; 19];
    bytes[0..8].copy_from_slice(&p.occupied.to_le_bytes());
    bytes[8..16].copy_from_slice(&p.pieces);
    bytes[16] = p.stm;
    bytes[17] = p.castling;
    bytes[18] = p.ep_square;

    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

// ---------------------------------------------------------------------------
// Material composition
// ---------------------------------------------------------------------------

/// Role bits for [`MaterialFilter::require_any`], indexed to match the piece
/// codes: pawn is role 1, king is role 6.
pub const ROLE_PAWN: u8 = 1 << 0;
pub const ROLE_KNIGHT: u8 = 1 << 1;
pub const ROLE_BISHOP: u8 = 1 << 2;
pub const ROLE_ROOK: u8 = 1 << 3;
pub const ROLE_QUEEN: u8 = 1 << 4;
pub const ROLE_KING: u8 = 1 << 5;

/// Parses a role set from letters, e.g. `"nbr"` → knight|bishop|rook.
/// Case-insensitive; unknown characters are an error.
pub fn parse_roles(spec: &str) -> Result<u8, char> {
    let mut mask = 0u8;
    for c in spec.chars().filter(|c| !c.is_whitespace() && *c != ',') {
        mask |= match c.to_ascii_lowercase() {
            'p' => ROLE_PAWN,
            'n' => ROLE_KNIGHT,
            'b' => ROLE_BISHOP,
            'r' => ROLE_ROOK,
            'q' => ROLE_QUEEN,
            'k' => ROLE_KING,
            _ => return Err(c),
        };
    }
    Ok(mask)
}

pub fn roles_to_string(mask: u8) -> String {
    let mut out = String::new();
    for (bit, ch) in [
        (ROLE_PAWN, 'P'),
        (ROLE_KNIGHT, 'N'),
        (ROLE_BISHOP, 'B'),
        (ROLE_ROOK, 'R'),
        (ROLE_QUEEN, 'Q'),
        (ROLE_KING, 'K'),
    ] {
        if mask & bit != 0 {
            out.push(ch);
        }
    }
    out
}

/// Requirements on *what kinds of pieces* are on the board.
///
/// Balanced simplified positions are dominated by king-and-pawn and
/// king-pawn-rook endings — trading down to a level position is exactly how
/// real games arrive there, and it strips the board of everything else. Those
/// are fine positions but they all look alike: in one 8-piece run, 371 of 740
/// results were either pure king-and-pawns or kings-pawns-rooks, and 27% held
/// no knight, bishop or rook at all. These gates exist to force variety.
#[derive(Debug, Clone, Default)]
pub struct MaterialFilter {
    /// Minimum number of distinct roles on the board, kings included. A pure
    /// king-and-pawn ending has 2; adding a rook makes 3.
    pub min_roles: u8,
    /// At least one piece must have a role in this set. Empty = no constraint.
    /// `nbr` is the useful one: it demands a real piece rather than only
    /// pawns and queens.
    pub require_any: u8,
    /// Cap on total pawns across both sides. `None` = unlimited.
    pub max_pawns: Option<u8>,
}

impl MaterialFilter {
    pub fn is_noop(&self) -> bool {
        self.min_roles == 0 && self.require_any == 0 && self.max_pawns.is_none()
    }

    pub fn accepts(&self, occupied: u64, pieces: &PieceNibbles) -> bool {
        let counts = role_counts(occupied, pieces);
        let present: u8 = (1..=6)
            .filter(|r| counts[*r as usize] > 0)
            .fold(0, |mask, r| mask | (1 << (r - 1)));

        if present.count_ones() < self.min_roles as u32 {
            return false;
        }
        if self.require_any != 0 && present & self.require_any == 0 {
            return false;
        }
        if let Some(max) = self.max_pawns {
            if counts[1] > max {
                return false;
            }
        }
        true
    }
}

/// Pieces per role, both colours combined. Index 1..=6 is pawn..king; index 0
/// is unused so the piece codes can be used directly.
pub fn role_counts(occupied: u64, pieces: &PieceNibbles) -> [u8; 7] {
    let mut counts = [0u8; 7];
    for slot in 0..occupied.count_ones() as usize {
        let byte = pieces[slot / 2];
        let code = if slot % 2 == 0 { byte & 0x0f } else { byte >> 4 };
        if code == 0 {
            continue;
        }
        // Codes 7..12 are the black mirror of 1..6.
        let role = if code > 6 { code - 6 } else { code };
        if (1..=6).contains(&role) {
            counts[role as usize] += 1;
        }
    }
    counts
}

/// The material signature, e.g. `"KKPPPPRR"` — both sides combined, sorted.
pub fn material_signature(occupied: u64, pieces: &PieceNibbles) -> String {
    let counts = role_counts(occupied, pieces);
    let mut out = String::new();
    for (role, ch) in [(3u8, 'B'), (6, 'K'), (2, 'N'), (1, 'P'), (5, 'Q'), (4, 'R')] {
        for _ in 0..counts[role as usize] {
            out.push(ch);
        }
    }
    out
}

/// The 64 squares as piece codes, a1 first.
pub fn squares(rec: &Record) -> [u8; 64] {
    let mut board = [0u8; 64];
    let mut slot = 0usize;
    for square in 0..64u32 {
        if rec.occupied & (1u64 << square) == 0 {
            continue;
        }
        let byte = rec.pieces[slot / 2];
        let code = if slot % 2 == 0 { byte & 0x0f } else { byte >> 4 };
        board[square as usize] = code;
        slot += 1;
    }
    board
}

pub fn piece_count(rec: &Record) -> u32 {
    rec.occupied.count_ones()
}

/// Rebuilds the FEN. Pure byte-shuffling — no chess library involved.
pub fn fen(rec: &Record) -> String {
    let board = squares(rec);
    let mut out = String::with_capacity(90);

    for rank in (0..8).rev() {
        let mut empty = 0;
        for file in 0..8 {
            let code = board[rank * 8 + file];
            if code == 0 {
                empty += 1;
                continue;
            }
            if empty > 0 {
                out.push((b'0' + empty) as char);
                empty = 0;
            }
            out.push(PIECE_CHARS[code as usize]);
        }
        if empty > 0 {
            out.push((b'0' + empty) as char);
        }
        if rank > 0 {
            out.push('/');
        }
    }

    out.push(' ');
    out.push(if rec.stm == 0 { 'w' } else { 'b' });

    out.push(' ');
    if rec.castling == 0 {
        out.push('-');
    } else {
        for (bit, ch) in [(1u8, 'K'), (2, 'Q'), (4, 'k'), (8, 'q')] {
            if rec.castling & bit != 0 {
                out.push(ch);
            }
        }
    }

    out.push(' ');
    if rec.ep_square == 255 {
        out.push('-');
    } else {
        out.push((b'a' + (rec.ep_square % 8)) as char);
        out.push((b'1' + (rec.ep_square / 8)) as char);
    }

    out.push_str(&format!(" {} {}", rec.halfmove, rec.fullmove.max(1)));
    out
}

/// An 8×8 ASCII diagram, white at the bottom, for terminal review.
pub fn ascii(rec: &Record) -> String {
    let board = squares(rec);
    let mut out = String::new();
    for rank in (0..8).rev() {
        out.push((b'1' + rank as u8) as char);
        out.push(' ');
        for file in 0..8 {
            out.push(PIECE_CHARS[board[rank * 8 + file] as usize]);
            out.push(' ');
        }
        out.push('\n');
    }
    out.push_str("  a b c d e f g h\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{Record, MAX_PIECES};
    use shakmaty::{Chess, Position};

    fn record_from(packed: Packed, halfmove: u8, fullmove: u16) -> Record {
        Record {
            id: position_id(&packed),
            occupied: packed.occupied,
            pieces: packed.pieces,
            stm: packed.stm,
            castling: packed.castling,
            ep_square: packed.ep_square,
            halfmove,
            fullmove,
            ..Record::default()
        }
    }

    /// Play a short line and check the packed round trip reproduces the FEN
    /// shakmaty itself would print.
    #[test]
    fn packs_a_real_endgame() {
        // 8 pieces: kings, a rook each, two pawns each.
        let setup: shakmaty::fen::Fen = "4k2r/5pp1/8/8/8/8/5PP1/4K2R w Kk - 4 30"
            .parse()
            .unwrap();
        let pos: Chess = setup
            .into_position(shakmaty::CastlingMode::Standard)
            .unwrap();
        let packed = pack_position(&pos).expect("8 pieces should pack");
        assert_eq!(packed.occupied.count_ones(), 8);
        let rec = record_from(packed, 4, 30);
        assert_eq!(fen(&rec), "4k2r/5pp1/8/8/8/8/5PP1/4K2R w Kk - 4 30");
    }

    /// The two shapes that dominate a balanced eight-piece set, and what the
    /// material gates do with them.
    #[test]
    fn material_gates_reject_the_lookalike_endings() {
        let parse = |fen: &str| -> Record {
            let setup: shakmaty::fen::Fen = fen.parse().unwrap();
            let pos: Chess = setup
                .into_position(shakmaty::CastlingMode::Standard)
                .unwrap();
            record_from(pack_position(&pos).unwrap(), 0, 1)
        };

        // Pure king-and-pawns: 2 distinct roles, no piece at all.
        let pawns = parse("4k3/5ppp/8/8/8/8/5PPP/4K3 w - - 0 40");
        assert_eq!(material_signature(pawns.occupied, &pawns.pieces), "KKPPPPPP");
        // Kings, pawns and rooks: 3 roles, and a rook counts as a piece.
        let rooks = parse("4k2r/5pp1/8/8/8/8/5PP1/4K2R w - - 0 40");
        assert_eq!(material_signature(rooks.occupied, &rooks.pieces), "KKPPPPRR");
        // Kings, pawns, a knight and a bishop.
        let mixed = parse("4k3/5pp1/6n1/8/8/6B1/5PP1/4K3 w - - 0 40");
        assert_eq!(material_signature(mixed.occupied, &mixed.pieces), "BKKNPPPP");

        let min3 = MaterialFilter {
            min_roles: 3,
            ..Default::default()
        };
        assert!(!min3.accepts(pawns.occupied, &pawns.pieces));
        assert!(min3.accepts(rooks.occupied, &rooks.pieces));
        assert!(min3.accepts(mixed.occupied, &mixed.pieces));

        // "must contain a knight, bishop or rook" — a queen does not qualify.
        let nbr = MaterialFilter {
            require_any: parse_roles("nbr").unwrap(),
            ..Default::default()
        };
        assert!(!nbr.accepts(pawns.occupied, &pawns.pieces));
        assert!(nbr.accepts(rooks.occupied, &rooks.pieces));
        assert!(nbr.accepts(mixed.occupied, &mixed.pieces));

        let queens = parse("4k3/5pp1/8/8/8/8/5PP1/3QK3 w - - 0 40");
        assert!(
            !nbr.accepts(queens.occupied, &queens.pieces),
            "a queen is not a knight, bishop or rook"
        );

        // Pawn cap trims the pawn-heavy endings.
        let cap = MaterialFilter {
            max_pawns: Some(4),
            ..Default::default()
        };
        assert!(!cap.accepts(pawns.occupied, &pawns.pieces), "six pawns");
        assert!(cap.accepts(rooks.occupied, &rooks.pieces), "four pawns");
    }

    #[test]
    fn role_spec_parsing() {
        assert_eq!(parse_roles("nbr").unwrap(), ROLE_KNIGHT | ROLE_BISHOP | ROLE_ROOK);
        assert_eq!(parse_roles("NBR").unwrap(), parse_roles("nbr").unwrap());
        assert_eq!(parse_roles("n,b r").unwrap(), parse_roles("nbr").unwrap());
        assert_eq!(parse_roles("").unwrap(), 0);
        assert_eq!(parse_roles("nxr"), Err('x'));
        assert_eq!(roles_to_string(parse_roles("rn").unwrap()), "NR");
    }

    #[test]
    fn empty_material_filter_accepts_everything() {
        let f = MaterialFilter::default();
        assert!(f.is_noop());
        assert!(f.accepts(0xff, &[0x11, 0x11, 0x11, 0x11, 0, 0, 0, 0]));
    }

    /// A full board has 32 pieces, well past what the nibble array can hold.
    #[test]
    fn refuses_boards_larger_than_max_pieces() {
        let pos = Chess::default();
        assert_eq!(pos.board().occupied().count(), 32);
        assert!(pack_position(&pos).is_none());
    }

    /// The format must round-trip the largest board it claims to support.
    #[test]
    fn packs_a_sixteen_piece_position() {
        let expected = "r3k2r/pp3ppp/8/8/8/8/PP3PPP/R3K2R w KQkq - 4 20";
        let setup: shakmaty::fen::Fen = expected.parse().unwrap();
        let pos: Chess = setup
            .into_position(shakmaty::CastlingMode::Standard)
            .unwrap();
        let packed = pack_position(&pos).expect("16 pieces must pack");
        assert_eq!(packed.occupied.count_ones(), MAX_PIECES);
        let rec = record_from(packed, 4, 20);
        assert_eq!(fen(&rec), expected);
        assert_eq!(
            material_signature(rec.occupied, &rec.pieces),
            "KKPPPPPPPPPPRRRR"
        );
    }

    /// A mid-range board, the size this pipeline actually targets.
    #[test]
    fn packs_a_twelve_piece_position() {
        let expected = "4rk2/5pp1/p7/8/8/P1N4P/5PP1/4RK2 w - - 2 30";
        let setup: shakmaty::fen::Fen = expected.parse().unwrap();
        let pos: Chess = setup
            .into_position(shakmaty::CastlingMode::Standard)
            .unwrap();
        let packed = pack_position(&pos).expect("12 pieces must pack");
        assert_eq!(packed.occupied.count_ones(), 12);
        let rec = record_from(packed, 2, 30);
        assert_eq!(fen(&rec), expected);
    }



    #[test]
    fn id_ignores_clocks_but_not_side_to_move() {
        let setup: shakmaty::fen::Fen = "4k2r/5pp1/8/8/8/8/5PP1/4K2R w Kk - 4 30".parse().unwrap();
        let pos: Chess = setup.into_position(shakmaty::CastlingMode::Standard).unwrap();
        let packed = pack_position(&pos).unwrap();

        let a = record_from(packed, 4, 30);
        let b = record_from(packed, 90, 200);
        assert_eq!(a.id, b.id, "clocks must not affect the id");

        let mut flipped = packed;
        flipped.stm = 1;
        assert_ne!(position_id(&flipped), a.id, "side to move must affect the id");
    }
}
