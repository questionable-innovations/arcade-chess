//! The `ARCPOS1` container: a flat file of fixed-size position records.
//!
//! The whole point of the format is that a record is *self-contained*. Given
//! 124 bytes you can rebuild the board, name the two humans who played it, say
//! when they played it and who eventually won, and link back to the original
//! lichess game — with no side tables and no second file to keep in sync.
//! Records are fixed-size so the file is directly indexable: record `n` lives
//! at `HEADER_SIZE + n * RECORD_SIZE`, which makes `mmap` + binary search on
//! `id` viable once the file has been sorted.
//!
//! All integers are little-endian.
//!
//! ```text
//! header (32 bytes)
//!   magic        [u8; 8]  = b"ARCPOS1\0"
//!   version      u32      = 2
//!   record_size  u32      = 124
//!   count        u64      number of records that follow
//!   flags        u64      bit 0: records are sorted by `id` and deduplicated
//!
//! record (124 bytes)
//!   0    id            u64      FNV-1a of the packed position (see `position_id`)
//!   8    occupied      u64      bitboard; bit i set = square i is occupied (a1=0 … h8=63)
//!   16   pieces        [u8; 8]  16 nibbles, one per set bit of `occupied`, ascending
//!   24   stm           u8       0 = white to move, 1 = black
//!   25   castling      u8       bitmask: 1 K, 2 Q, 4 k, 8 q
//!   26   ep_square     u8       en-passant target square, 255 = none
//!   27   halfmove      u8       halfmove clock, saturating at 255
//!   28   fullmove      u16      fullmove number
//!   30   ply           u16      0-based ply index of this position within the game
//!   32   game_plies    u16      total plies the game lasted
//!   34   eval_cp       i16      lichess [%eval] in centipawns, white POV
//!   36   verified_cp   i16      stage-2 engine eval, white POV; EVAL_UNSET if not verified
//!   38   wdl_win       u16      stage-2 win  per-mille, side-to-move POV; 0 if unverified
//!   40   wdl_draw      u16      stage-2 draw per-mille
//!   42   wdl_loss      u16      stage-2 loss per-mille
//!   44   white_elo     u16
//!   46   black_elo     u16
//!   48   tc_initial    u16      time control base, seconds
//!   50   tc_increment  u16      time control increment, seconds
//!   52   _pad          [u8; 4]  aligns utc_time to 8
//!   56   utc_time      i64      game start, unix seconds
//!   64   game_id       [u8; 8]  lichess game id (base62), NUL padded
//!   72   winner        u8       0 = white, 1 = black (draws are never stored)
//!   73   termination   u8       see `Termination`
//!   74   flags         u8       bit 0: mate score in `eval_cp`
//!   75   _reserved     u8
//!   76   white_name    [u8; 20] UTF-8 username, NUL padded (lichess caps at 20)
//!   96   black_name    [u8; 20]
//!   116  second_cp     i16      stage-2 second-best root move, white POV
//!   118  drop_cp       i16      stage-2 best minus second-best, side-to-move POV
//!   120  legal_moves   u8       root moves the engine reported (capped at MultiPV)
//!   121  losing_moves  u8       of those, how many lose by >= 300cp
//!   122  holding_moves u8       of those, how many stay within 50cp of the best
//!   123  _reserved2    u8
//! ```
//!
//! The nibble array holds up to [`MAX_PIECES`] pieces, which is what bounds
//! how large a position this format can describe.
//!
//! `holding_moves` is the sharpness measure the whole pipeline exists to find.
//! A position that is level (`verified_cp` ≈ 0) but has only one or two
//! holding moves out of a long list is one where a single slip decides the
//! game — level on the scoreboard, nothing like a draw in practice. See
//! `verify.rs` for why the score alone cannot express this.
//!
//! Mate scores from lichess (`[%eval #3]`) are stored as `±(MATE_BASE - plies)`
//! centipawns with record flag bit 0 set, so an ordinary centipawn comparison
//! still sorts them to the extremes.

use anyhow::{bail, Context, Result};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

pub const MAGIC: [u8; 8] = *b"ARCPOS1\0";
pub const VERSION: u32 = 2;
pub const HEADER_SIZE: usize = 32;
pub const RECORD_SIZE: usize = 124;

/// Pieces the nibble array can describe. Two per byte over 8 bytes.
pub const MAX_PIECES: u32 = 16;

/// The packed piece nibbles: one per set bit of `occupied`, ascending square.
pub type PieceNibbles = [u8; 8];

/// Sentinel for "stage 2 has not looked at this record yet".
pub const EVAL_UNSET: i16 = i16::MIN;
/// Mate scores are stored as `MATE_BASE - plies_to_mate`, signed by the winner.
pub const MATE_BASE: i16 = 30000;

/// Header flag: records are sorted by `id` with duplicates collapsed.
pub const FILE_FLAG_SORTED: u64 = 1 << 0;
/// Record flag: `eval_cp` came from a mate announcement rather than a centipawn score.
pub const REC_FLAG_MATE: u8 = 1 << 0;

pub const NAME_LEN: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Termination {
    /// Checkmate, stalemate, or resignation — decided on the board.
    Normal = 0,
    /// One side ran out of clock.
    TimeForfeit = 1,
    Abandoned = 2,
    RulesInfraction = 3,
    #[default]
    Unknown = 4,
}

impl Termination {
    pub fn from_pgn(value: &str) -> Self {
        match value {
            "Normal" => Termination::Normal,
            "Time forfeit" => Termination::TimeForfeit,
            "Abandoned" => Termination::Abandoned,
            "Rules infraction" => Termination::RulesInfraction,
            _ => Termination::Unknown,
        }
    }

    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => Termination::Normal,
            1 => Termination::TimeForfeit,
            2 => Termination::Abandoned,
            3 => Termination::RulesInfraction,
            _ => Termination::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Termination::Normal => "Normal",
            Termination::TimeForfeit => "Time forfeit",
            Termination::Abandoned => "Abandoned",
            Termination::RulesInfraction => "Rules infraction",
            Termination::Unknown => "Unknown",
        }
    }
}

/// One mined position, exactly as it sits on disk.
///
/// `PartialEq` is derived so the round-trip test can assert the *whole* record
/// survives encoding rather than the handful of fields someone remembered to
/// list — every field is a plain integer or byte array, so equality means
/// exactly what it looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    pub id: u64,
    pub occupied: u64,
    pub pieces: PieceNibbles,
    pub stm: u8,
    pub castling: u8,
    pub ep_square: u8,
    pub halfmove: u8,
    pub fullmove: u16,
    pub ply: u16,
    pub game_plies: u16,
    pub eval_cp: i16,
    pub verified_cp: i16,
    pub wdl_win: u16,
    pub wdl_draw: u16,
    pub wdl_loss: u16,
    pub white_elo: u16,
    pub black_elo: u16,
    pub tc_initial: u16,
    pub tc_increment: u16,
    pub utc_time: i64,
    pub game_id: [u8; 8],
    pub winner: u8,
    pub termination: u8,
    pub flags: u8,
    pub white_name: [u8; NAME_LEN],
    pub black_name: [u8; NAME_LEN],
    pub second_cp: i16,
    pub drop_cp: i16,
    pub legal_moves: u8,
    pub losing_moves: u8,
    pub holding_moves: u8,
}

impl Default for Record {
    fn default() -> Self {
        Record {
            id: 0,
            occupied: 0,
            pieces: [0; 8],
            stm: 0,
            castling: 0,
            ep_square: 255,
            halfmove: 0,
            fullmove: 1,
            ply: 0,
            game_plies: 0,
            eval_cp: 0,
            verified_cp: EVAL_UNSET,
            wdl_win: 0,
            wdl_draw: 0,
            wdl_loss: 0,
            white_elo: 0,
            black_elo: 0,
            tc_initial: 0,
            tc_increment: 0,
            utc_time: 0,
            game_id: [0; 8],
            winner: 0,
            termination: Termination::Unknown as u8,
            flags: 0,
            white_name: [0; NAME_LEN],
            black_name: [0; NAME_LEN],
            second_cp: EVAL_UNSET,
            drop_cp: 0,
            legal_moves: 0,
            losing_moves: 0,
            holding_moves: 0,
        }
    }
}

/// Little-endian scalar writes into a fixed buffer, by explicit offset, so the
/// layout in the doc comment above is the layout on disk.
macro_rules! put {
    ($buf:expr, $off:expr, $val:expr) => {{
        let bytes = $val.to_le_bytes();
        $buf[$off..$off + bytes.len()].copy_from_slice(&bytes);
    }};
}

macro_rules! get {
    ($buf:expr, $off:expr, $ty:ty) => {{
        const N: usize = std::mem::size_of::<$ty>();
        let mut tmp = [0u8; N];
        tmp.copy_from_slice(&$buf[$off..$off + N]);
        <$ty>::from_le_bytes(tmp)
    }};
}

impl Record {
    pub fn encode(&self) -> [u8; RECORD_SIZE] {
        let mut b = [0u8; RECORD_SIZE];
        put!(b, 0, self.id);
        put!(b, 8, self.occupied);
        b[16..24].copy_from_slice(&self.pieces);
        b[24] = self.stm;
        b[25] = self.castling;
        b[26] = self.ep_square;
        b[27] = self.halfmove;
        put!(b, 28, self.fullmove);
        put!(b, 30, self.ply);
        put!(b, 32, self.game_plies);
        put!(b, 34, self.eval_cp);
        put!(b, 36, self.verified_cp);
        put!(b, 38, self.wdl_win);
        put!(b, 40, self.wdl_draw);
        put!(b, 42, self.wdl_loss);
        put!(b, 44, self.white_elo);
        put!(b, 46, self.black_elo);
        put!(b, 48, self.tc_initial);
        put!(b, 50, self.tc_increment);
        put!(b, 56, self.utc_time);
        b[64..72].copy_from_slice(&self.game_id);
        b[72] = self.winner;
        b[73] = self.termination;
        b[74] = self.flags;
        b[76..96].copy_from_slice(&self.white_name);
        b[96..116].copy_from_slice(&self.black_name);
        put!(b, 116, self.second_cp);
        put!(b, 118, self.drop_cp);
        b[120] = self.legal_moves;
        b[121] = self.losing_moves;
        b[122] = self.holding_moves;
        b
    }

    pub fn decode(b: &[u8]) -> Record {
        let mut pieces = [0u8; 8];
        pieces.copy_from_slice(&b[16..24]);
        let mut game_id = [0u8; 8];
        game_id.copy_from_slice(&b[64..72]);
        let mut white_name = [0u8; NAME_LEN];
        white_name.copy_from_slice(&b[76..96]);
        let mut black_name = [0u8; NAME_LEN];
        black_name.copy_from_slice(&b[96..116]);
        Record {
            id: get!(b, 0, u64),
            occupied: get!(b, 8, u64),
            pieces,
            stm: b[24],
            castling: b[25],
            ep_square: b[26],
            halfmove: b[27],
            fullmove: get!(b, 28, u16),
            ply: get!(b, 30, u16),
            game_plies: get!(b, 32, u16),
            eval_cp: get!(b, 34, i16),
            verified_cp: get!(b, 36, i16),
            wdl_win: get!(b, 38, u16),
            wdl_draw: get!(b, 40, u16),
            wdl_loss: get!(b, 42, u16),
            white_elo: get!(b, 44, u16),
            black_elo: get!(b, 46, u16),
            tc_initial: get!(b, 48, u16),
            tc_increment: get!(b, 50, u16),
            utc_time: get!(b, 56, i64),
            game_id,
            winner: b[72],
            termination: b[73],
            flags: b[74],
            white_name,
            black_name,
            second_cp: get!(b, 116, i16),
            drop_cp: get!(b, 118, i16),
            legal_moves: b[120],
            losing_moves: b[121],
            holding_moves: b[122],
        }
    }

    pub fn white(&self) -> &str {
        str_field(&self.white_name)
    }

    pub fn black(&self) -> &str {
        str_field(&self.black_name)
    }

    pub fn game_id_str(&self) -> &str {
        let end = self.game_id.iter().position(|&c| c == 0).unwrap_or(8);
        std::str::from_utf8(&self.game_id[..end]).unwrap_or("")
    }

    pub fn game_url(&self) -> String {
        format!("https://lichess.org/{}#{}", self.game_id_str(), self.ply)
    }

    pub fn winner_str(&self) -> &'static str {
        if self.winner == 0 {
            "white"
        } else {
            "black"
        }
    }

    pub fn is_mate_eval(&self) -> bool {
        self.flags & REC_FLAG_MATE != 0
    }

    /// Human-facing identifier: the 64-bit `id` in Crockford base32.
    pub fn id_str(&self) -> String {
        crockford32(self.id)
    }
}

fn str_field(bytes: &[u8]) -> &str {
    let end = bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len());
    std::str::from_utf8(&bytes[..end]).unwrap_or("")
}

pub fn name_field(name: &str) -> [u8; NAME_LEN] {
    let mut out = [0u8; NAME_LEN];
    let bytes = name.as_bytes();
    // Truncate on a char boundary so the stored field stays valid UTF-8.
    let mut end = bytes.len().min(NAME_LEN);
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
    out[..end].copy_from_slice(&bytes[..end]);
    out
}

pub fn game_id_field(id: &str) -> [u8; 8] {
    let mut out = [0u8; 8];
    let bytes = id.as_bytes();
    let end = bytes.len().min(8);
    out[..end].copy_from_slice(&bytes[..end]);
    out
}

/// Crockford base32 of a u64 — 13 characters, no vowels, so ids never
/// accidentally spell anything and are safe to read aloud during bring-up.
pub fn crockford32(mut v: u64) -> String {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut buf = [b'0'; 13];
    for slot in buf.iter_mut().rev() {
        *slot = ALPHABET[(v & 31) as usize];
        v >>= 5;
    }
    String::from_utf8(buf.to_vec()).expect("alphabet is ascii")
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

pub struct Writer {
    file: BufWriter<File>,
    count: u64,
    flags: u64,
}

impl Writer {
    pub fn create(path: &Path) -> Result<Writer> {
        let file = File::create(path)
            .with_context(|| format!("creating {}", path.display()))?;
        let mut file = BufWriter::new(file);
        file.write_all(&header_bytes(0, 0))?;
        Ok(Writer {
            file,
            count: 0,
            flags: 0,
        })
    }

    pub fn push(&mut self, rec: &Record) -> Result<()> {
        self.file.write_all(&rec.encode())?;
        self.count += 1;
        Ok(())
    }

    /// Rewrites the header with the final count. Cheap enough to call
    /// periodically during a long mine so the partial file stays readable.
    pub fn flush_header(&mut self) -> Result<()> {
        self.file.flush()?;
        let inner = self.file.get_mut();
        let pos = inner.stream_position()?;
        inner.seek(SeekFrom::Start(0))?;
        inner.write_all(&header_bytes(self.count, self.flags))?;
        inner.seek(SeekFrom::Start(pos))?;
        inner.flush()?;
        Ok(())
    }

    pub fn finish(mut self) -> Result<u64> {
        self.flush_header()?;
        Ok(self.count)
    }
}

fn header_bytes(count: u64, flags: u64) -> [u8; HEADER_SIZE] {
    let mut h = [0u8; HEADER_SIZE];
    h[0..8].copy_from_slice(&MAGIC);
    put!(h, 8, VERSION);
    put!(h, 12, RECORD_SIZE as u32);
    put!(h, 16, count);
    put!(h, 24, flags);
    h
}

// ---------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------

pub struct Reader {
    inner: BufReader<File>,
    pub count: u64,
    pub flags: u64,
    read: u64,
}

impl Reader {
    pub fn open(path: &Path) -> Result<Reader> {
        let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let file_len = file.metadata()?.len();
        let mut inner = BufReader::new(file);
        let mut header = [0u8; HEADER_SIZE];
        inner.read_exact(&mut header).context("reading header")?;
        if header[0..8] != MAGIC {
            bail!("{} is not an ARCPOS1 file", path.display());
        }
        let version = get!(header, 8, u32);
        if version != VERSION {
            bail!("unsupported ARCPOS1 version {version}");
        }
        let record_size = get!(header, 12, u32) as usize;
        if record_size != RECORD_SIZE {
            bail!("unexpected record size {record_size}, expected {RECORD_SIZE}");
        }
        // The header count is only rewritten periodically during a long mine,
        // so an in-progress or killed run leaves it behind what is actually on
        // disk. Counting whole records in the file recovers the rest, and the
        // floor division discards a torn final record. For a finished file the
        // two agree exactly, so taking the larger is always safe.
        let on_disk = file_len.saturating_sub(HEADER_SIZE as u64) / RECORD_SIZE as u64;
        let count = get!(header, 16, u64).max(on_disk);
        Ok(Reader {
            inner,
            count,
            flags: get!(header, 24, u64),
            read: 0,
        })
    }
}

impl Iterator for Reader {
    type Item = Result<Record>;

    fn next(&mut self) -> Option<Result<Record>> {
        if self.read >= self.count {
            return None;
        }
        let mut buf = [0u8; RECORD_SIZE];
        match self.inner.read_exact(&mut buf) {
            Ok(()) => {
                self.read += 1;
                Some(Ok(Record::decode(&buf)))
            }
            Err(e) => Some(Err(e.into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every offset in the record moved when the format went to v2, so this
    /// asserts the whole struct survives rather than a sample of fields — a
    /// field left off the list is precisely how a shifted offset gets shipped.
    #[test]
    fn record_round_trips() {
        let rec = Record {
            id: 0x0123_4567_89ab_cdef,
            occupied: 0xdead_beef_cafe_babe,
            pieces: [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0],
            stm: 1,
            castling: 0b1010,
            ep_square: 42,
            halfmove: 17,
            fullmove: 61,
            ply: 121,
            game_plies: 140,
            eval_cp: -12,
            verified_cp: 8,
            wdl_win: 300,
            wdl_draw: 400,
            wdl_loss: 300,
            white_elo: 2210,
            black_elo: 2185,
            tc_initial: 600,
            tc_increment: 5,
            utc_time: 1_567_300_000,
            game_id: game_id_field("aBcD1234"),
            winner: 1,
            termination: Termination::Normal as u8,
            flags: REC_FLAG_MATE,
            white_name: name_field("someone"),
            black_name: name_field("someone-else"),
            second_cp: -340,
            drop_cp: 348,
            legal_moves: 22,
            losing_moves: 19,
            holding_moves: 2,
        };
        let encoded = rec.encode();
        assert_eq!(encoded.len(), RECORD_SIZE);
        let back = Record::decode(&encoded);
        assert_eq!(back, rec, "every field must survive the round trip");
        // The accessors sit on top of the raw fields, so they are worth naming
        // separately: an offset can be right while the interpretation is wrong.
        assert_eq!(back.game_id_str(), "aBcD1234");
        assert_eq!(back.white(), "someone");
        assert_eq!(back.black(), "someone-else");
        assert_eq!(back.winner_str(), "black");
        assert!(back.is_mate_eval());
    }

    #[test]
    fn long_names_truncate_on_char_boundary() {
        let field = name_field("ααααααααααααααααααααααα");
        let text = str_field(&field);
        assert!(text.len() <= NAME_LEN);
        assert!(text.chars().all(|c| c == 'α'));
    }

    /// A mine flushes its header every few thousand records, so a file read
    /// while the mine is still running has a stale count. The reader must see
    /// every complete record anyway.
    #[test]
    fn reader_recovers_records_a_stale_header_omits() {
        let dir = std::env::temp_dir().join(format!("arcpos-stale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("partial.arcpos");

        let mut writer = Writer::create(&path).unwrap();
        for i in 0..5u64 {
            writer.push(&Record {
                id: i,
                ..Record::default()
            })
            .unwrap();
        }
        // Deliberately do NOT call finish(): the header still says zero.
        drop(writer);

        let reader = Reader::open(&path).unwrap();
        assert_eq!(reader.count, 5, "all five records must be visible");
        let ids: Vec<u64> = reader.map(|r| r.unwrap().id).collect();
        assert_eq!(ids, vec![0, 1, 2, 3, 4]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn crockford_is_thirteen_chars() {
        assert_eq!(crockford32(0).len(), 13);
        assert_eq!(crockford32(u64::MAX).len(), 13);
        assert_ne!(crockford32(1), crockford32(2));
    }
}
