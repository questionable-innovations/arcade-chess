# position-miner (`arcpos`)

Mines **balanced but decisive** positions out of the public
[lichess database](https://database.lichess.org/) dumps, so the board has a
library of positions that are genuinely 50/50 and genuinely playable out.

Targets simplified-but-not-bare boards — 10 to 15 pieces by default — with
material variety enforced, because the naive filters collapse onto a monoculture
of king-and-pawn endings (see below).

```
lichess .pgn.zst ─► arcpos mine ─► .arcpos ──► arcpos verify ─► .arcpos ─► arcpos filter ─► .arcpos
  (raw games)      (stage 1,      (candidates)  (stage 2,       (scored)   (retune gates,   (curated)
                    streaming)                   Stockfish)                 no engine)
                                                                                 │
                                                             arcpos review ──────┴──► HTML contact sheet
                                                             arcpos dump   ───────────► text / JSON lines
```

## The thing that is not obvious

The naive filter — "ask an engine for a position it scores near `0.00`" — does
not work at eight pieces. It returns dead draws, essentially every time. A
level king-and-pawn ending is `0.00` with a win/draw/loss spread of `1/998/1`;
the human who eventually won it did so because their opponent blundered later,
not because the position had anything in it.

Stockfish's WDL output does not save the filter, either. It is derived from the
eval through a material-scaled curve, so at `0.00` with eight pieces it reports
~99% draw more or less by construction. It cannot tell a tense position from a
dead one because it is not really looking at the position.

What *does* separate them is the **shape of the move list**. A position is
worth playing when it is level yet unforgiving — one move holds the balance and
the rest lose:

```
  dead draw          sharp position
  1. Kf3   0.00      1. Rd7   0.00
  2. Kg3   0.00      2. Kf2  -3.10
  3. Kh3  -0.05      3. Rd1  -3.40
  4. Kf2  -0.10      4. Ra7  -4.00
  → 4 moves hold     → 1 move holds
```

So the metric is **`holding_moves`**: of the root moves a MultiPV search
reports, how many stay within 50cp of the best. Level score plus one or two
holders out of a long list is the definition of "50/50, but somebody is going
to lose this".

The tempting simpler metric, `best − second_best`, is *not* the gate. It only
catches the strict only-move case, and in practice a lot of the good positions
have two saving moves and seventeen losing ones — a difference of zero between
best and second, and every bit as sharp. Counting holders catches both shapes.
The drop is still recorded, as a finer-grained tiebreak when ranking.

## Stage 1 — `arcpos mine`

Streams a `.pgn.zst` dump through a zstd decoder into a `pgn-reader` visitor,
replaying every game move by move. Nothing is ever materialised, so the same
command works on a 10 GB monthly dump or on `curl … | arcpos mine -`.

It keeps a position when all of the following hold:

| gate | default | why |
| --- | --- | --- |
| piece count in range | `--min-pieces 10 --max-pieces 15` | the target board size |
| distinct piece roles | `--min-roles` (off) | 2 means king-and-pawns only; 3+ forces something else |
| a real piece present | `--require-any` (off) | pass `nbr` to demand a knight, bishop or rook |
| pawn cap | `--max-pawns` (off) | trims the pawn-heavy lookalikes |
| lichess `[%eval]` within ±band | `--band 30` | cheap "is it level" pre-filter |
| in-band for consecutive plies | `--stable 2` | rejects momentary crossings mid-tactic |
| game was **won**, not drawn | `--allow-draws` off | a draw says nothing about winning chances |
| decided on the board | `--allow-time-forfeit` off | a flag-fall win is not a position quality |
| both players rated ≥ | `--min-elo 1800` | annotated games skew strong anyway |
| game ran ≥ N more plies | `--min-tail 10` | the result was played out *from here* |
| at most N per game | `--per-game 1` | one level ending yields a dozen near-identical plies |
| duplicate position ids | always | transpositions collapse onto one record |

Only ~5% of lichess games carry `[%eval]` annotations (server analysis is
opt-in and biased toward stronger players), which is why stage 1 is a *filter*
and not the answer: it exists to get the candidate count down to something an
engine can afford to search properly.

```sh
arcpos mine data/lichess_db_standard_rated_2019-09.pgn.zst \
    -o data/candidates.arcpos
```

## Stage 2 — `arcpos verify`

Runs a real MultiPV search on every candidate with a local UCI engine, records
the sharpness profile, and keeps the positions that are both level and sharp.
Output is sorted sharpest-first.

```sh
arcpos verify data/candidates.arcpos -o data/curated.arcpos \
    --engine data/engine/stockfish --depth 26 --workers 8
```

| flag | default | meaning |
| --- | --- | --- |
| `--band` | 40 | keep \|best\| ≤ this many centipawns |
| `--max-holding` | 2 | at most this many root moves may hold the balance — the sharpness gate |
| `--min-legal` | 6 | reject near-forced positions; being forced is not being sharp |
| `--max-draw` | 1000 (off) | WDL cutoff; recorded for information, not useful as a filter |
| `--depth` | 26 | search depth per position |
| `--workers` | 4 | concurrent engine processes |

Stockfish is not vendored. Either `apt install stockfish` or drop a binary at
`data/engine/stockfish` and point `--engine` at it.

### Retuning without re-searching

A MultiPV pass over a month's candidates is the expensive part of the pipeline,
and it would be silly to repeat it because a threshold moved. So score once
with the gates wide open, then narrow with `filter`, which re-applies exactly
the same tests to an already-scored file:

```sh
# score everything, reject nothing
arcpos verify data/candidates.arcpos -o data/scored.arcpos \
    --engine data/engine/stockfish --depth 26 --workers 20 \
    --band 30000 --max-holding 255 --min-legal 0

# then narrow, as often as you like — no engine involved
arcpos filter data/scored.arcpos -o data/curated.arcpos --max-holding 2
```

## Looking at the results

```sh
arcpos stats  data/curated.arcpos
arcpos dump   data/curated.arcpos -n 5              # ASCII boards
arcpos dump   data/curated.arcpos -n 100 --json     # JSON lines
arcpos review data/curated.arcpos -o review.html -n 48
```

`review` writes a single self-contained HTML page — rendered boards, both
players, the eval and sharpness profile, and a deep link back to the ply in
the original lichess game.

## Material variety

Left alone, "level and sharp" produces a monoculture. In an 8-piece run over
2019-09, of 740 curated positions **185 were pure king-and-pawns and 186 were
kings-pawns-rooks** — half the set in two shapes — and 27% contained no knight,
bishop or rook at all.

That is not a bug in the sharpness metric; it is what the source data looks
like. Games reach a level simplified position *by trading everything off*, and
what survives the trades is pawns, rooks and kings. So variety has to be
demanded explicitly:

```sh
# at least one knight/bishop/rook, and at least 3 distinct roles on the board
arcpos mine … --require-any nbr --min-roles 3
arcpos filter data/scored.arcpos -o data/curated.arcpos --require-any nbr
```

`--require-any` takes role letters `p n b r q k`. Note a queen does *not*
satisfy `nbr` — the point is to demand a piece that is neither a pawn nor a
queen, since queens are already common in level positions.

Both gates exist on `mine` (cheap, skips the engine entirely) and on
`filter`/`verify` (retune an already-scored file).

## The `.arcpos` file

A 32-byte header followed by fixed-size 124-byte records. Every record is
self-contained: the packed board, side to move, castling and en-passant state,
both usernames and ratings, the time control, the UTC timestamp, who won, the
lichess game id and ply, and both engine passes. Fixed-size records mean record
`n` is at `32 + n*124`, so the file is `mmap`-able and binary-searchable on
`id` once sorted.

The board is packed as an occupancy bitboard plus one nibble per occupied
square, which caps a position at **16 pieces** — that ceiling is what bounds
`--max-pieces`.

The record `id` is FNV-1a over the 19 packed position bytes — board, side to
move, castling, en-passant — and nothing else. Clocks and move numbers are
excluded, so the same position reached by two different games collapses onto
one id. Rendered for humans as 13 characters of Crockford base32.

The authoritative layout is the doc comment at the top of
[`src/format.rs`](src/format.rs).

## Tests

```sh
cargo test
```

Covers the record round trip, position packing and FEN reconstruction, id
stability, the `[%eval]` and UCI info parsers, date conversion both ways, the
per-game spread selection, and the sharpness summary for both the dead-draw
and only-move shapes.
