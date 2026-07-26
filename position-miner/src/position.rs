//! Packing a live board into the 15 bytes an `ARCPOS1` record stores, and
//! unpacking it back into FEN / ASCII without needing a chess library.
//!
//! The reader side is deliberately dependency-free: anything that can read the
//! file can also render the board, which is what makes a record self-contained.

use crate::format::Record;
use shakmaty::{Bitboard, Board, Color, Position, Role, Square};

/// The board state, packed exactly as it lands in a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Packed {
    pub occupied: u64,
    pub pieces: [u8; 4],
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

/// Packs a board. Returns `None` if more than 8 pieces are on it, since the
/// nibble array only has room for eight.
pub fn pack_board(board: &Board, stm: Color, castling: Bitboard, ep: Option<Square>) -> Option<Packed> {
    let occupied = board.occupied();
    if occupied.count() > 8 {
        return None;
    }
    let mut pieces = [0u8; 4];
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

/// The record `id`: FNV-1a over the 15 packed position bytes.
///
/// Only the board, side to move, castling rights and en-passant target feed
/// the hash — clocks and move numbers do not — so the same position reached by
/// two different games collapses onto one id. That is what makes the file
/// deduplicable.
pub fn position_id(p: &Packed) -> u64 {
    let mut bytes = [0u8; 15];
    bytes[0..8].copy_from_slice(&p.occupied.to_le_bytes());
    bytes[8..12].copy_from_slice(&p.pieces);
    bytes[12] = p.stm;
    bytes[13] = p.castling;
    bytes[14] = p.ep_square;

    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
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
    use crate::format::Record;
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

    #[test]
    fn refuses_more_than_eight_pieces() {
        let pos = Chess::default();
        assert!(pack_position(&pos).is_none());
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
