//! `arcpos` — mine balanced-but-decisive endgame positions from lichess dumps.
//!
//! Two stages:
//!
//! 1. `arcpos mine` streams a `.pgn.zst` dump, replays every game, and keeps
//!    positions of a given piece count where lichess's own `[%eval]` says the
//!    game is level *and* somebody went on to win it on the board.
//! 2. `arcpos verify` re-scores those candidates with a local MultiPV search
//!    and keeps the ones where only one or two moves hold the balance — the
//!    property that separates a tense position from a dead draw.
//!
//! `stats`, `dump` and `review` read the resulting `ARCPOS1` file.

mod format;
mod mine;
mod position;
mod review;
mod verify;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

use format::Reader;

#[derive(Parser)]
#[command(name = "arcpos", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Stage 1: stream a lichess .pgn.zst dump into an ARCPOS1 file.
    Mine(MineArgs),
    /// Stage 2: re-score candidates with a local UCI engine.
    Verify(VerifyArgs),
    /// Re-apply the keep/reject gates to an already-verified file, without
    /// re-running the engine.
    Filter(FilterArgs),
    /// Summarise an ARCPOS1 file.
    Stats(FileArg),
    /// Print records as text blocks or JSON lines.
    Dump(DumpArgs),
    /// Render a self-contained HTML contact sheet of a sample.
    Review(ReviewArgs),
}

#[derive(Args)]
struct MineArgs {
    /// Input .pgn.zst, or `-` to read the compressed stream from stdin.
    input: PathBuf,
    /// Output ARCPOS1 file.
    #[arg(short, long, default_value = "positions.arcpos")]
    out: PathBuf,
    /// Exact piece count to mine for, kings included.
    #[arg(long, default_value_t = 8)]
    pieces: u32,
    /// Keep positions whose lichess eval is within ± this many centipawns.
    #[arg(long, default_value_t = 30)]
    band: i16,
    /// Consecutive in-band plies required (2+ rejects mid-tactic noise).
    #[arg(long, default_value_t = 2)]
    stable: u16,
    /// Minimum rating for both players.
    #[arg(long, default_value_t = 1800)]
    min_elo: u16,
    /// Plies the game must still run after the position.
    #[arg(long, default_value_t = 10)]
    min_tail: u16,
    /// Keep games decided by the clock rather than the board.
    #[arg(long)]
    allow_time_forfeit: bool,
    /// Keep drawn games (off by default — the whole point is decisive games).
    #[arg(long)]
    allow_draws: bool,
    /// Require the two sides' non-king material to differ.
    #[arg(long)]
    require_imbalance: bool,
    /// Keep at most this many positions from any single game.
    #[arg(long, default_value_t = 1)]
    per_game: usize,
    /// Minimum plies between two kept positions from the same game.
    #[arg(long, default_value_t = 8)]
    min_gap: u16,
    /// Stop after this many records (0 = unlimited).
    #[arg(long, default_value_t = 0)]
    limit: u64,
}

#[derive(Args)]
struct VerifyArgs {
    /// Input ARCPOS1 file from `mine`.
    input: PathBuf,
    /// Output ARCPOS1 file.
    #[arg(short, long, default_value = "verified.arcpos")]
    out: PathBuf,
    /// UCI engine binary.
    #[arg(long, default_value = "stockfish")]
    engine: String,
    /// Search depth per position.
    #[arg(long, default_value_t = 24)]
    depth: u32,
    /// Concurrent engine processes.
    #[arg(long, default_value_t = 4)]
    workers: usize,
    /// Threads per engine process.
    #[arg(long, default_value_t = 1)]
    engine_threads: u32,
    /// Hash table size per engine, MB.
    #[arg(long, default_value_t = 128)]
    hash_mb: u32,
    /// Keep positions within ± this many centipawns after the deep search.
    #[arg(long, default_value_t = 40)]
    band: i16,
    /// Sharpness gate: keep positions where at most this many root moves hold
    /// the balance. This is what separates a tense position from a dead draw.
    #[arg(long, default_value_t = 2)]
    max_holding: u8,
    /// Reject positions with fewer legal root moves than this — being forced
    /// is not the same as being sharp.
    #[arg(long, default_value_t = 6)]
    min_legal: u8,
    /// Drop positions whose draw probability exceeds this per-mille. Off by
    /// default — Stockfish's WDL is a function of eval and material, so it
    /// cannot tell a sharp level position from a dead one.
    #[arg(long, default_value_t = 1000)]
    max_draw: u16,
    /// Verify at most this many records (0 = all).
    #[arg(long, default_value_t = 0)]
    limit: u64,
}

#[derive(Args)]
struct FilterArgs {
    /// Input ARCPOS1 file that has already been through `verify`.
    input: PathBuf,
    #[arg(short, long, default_value = "curated.arcpos")]
    out: PathBuf,
    /// Keep positions within ± this many centipawns.
    #[arg(long, default_value_t = 40)]
    band: i16,
    /// At most this many root moves may hold the balance.
    #[arg(long, default_value_t = 2)]
    max_holding: u8,
    /// Reject positions with fewer legal root moves than this.
    #[arg(long, default_value_t = 6)]
    min_legal: u8,
    /// Drop positions whose draw probability exceeds this per-mille.
    #[arg(long, default_value_t = 1000)]
    max_draw: u16,
}

#[derive(Args)]
struct FileArg {
    file: PathBuf,
}

#[derive(Args)]
struct DumpArgs {
    file: PathBuf,
    /// Emit JSON lines instead of text blocks.
    #[arg(long)]
    json: bool,
    /// Print at most this many records.
    #[arg(short = 'n', long, default_value_t = 20)]
    limit: usize,
    /// Take every Nth record, to spread the sample across the file.
    #[arg(long, default_value_t = 1)]
    stride: usize,
}

#[derive(Args)]
struct ReviewArgs {
    file: PathBuf,
    #[arg(short, long, default_value = "review.html")]
    out: PathBuf,
    /// Number of positions on the page.
    #[arg(short = 'n', long, default_value_t = 48)]
    limit: usize,
    /// Take every Nth record. 0 spreads the sample evenly across the file.
    #[arg(long, default_value_t = 0)]
    stride: usize,
    /// Page title.
    #[arg(long, default_value = "Mined positions")]
    title: String,
    /// Subtitle / provenance line.
    #[arg(long, default_value = "")]
    subtitle: String,
    /// Extra bullet under the subtitle; repeatable.
    #[arg(long = "note")]
    notes: Vec<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Mine(args) => cmd_mine(args),
        Command::Verify(args) => cmd_verify(args),
        Command::Filter(args) => cmd_filter(args),
        Command::Stats(args) => cmd_stats(args),
        Command::Dump(args) => cmd_dump(args),
        Command::Review(args) => cmd_review(args),
    }
}

fn cmd_mine(args: MineArgs) -> Result<()> {
    let filters = mine::Filters {
        pieces: args.pieces,
        eval_band_cp: args.band,
        stable_plies: args.stable,
        min_elo: args.min_elo,
        min_remaining_plies: args.min_tail,
        require_normal_termination: !args.allow_time_forfeit,
        allow_draws: args.allow_draws,
        require_imbalance: args.require_imbalance,
        per_game: args.per_game,
        min_gap_plies: args.min_gap,
        limit: args.limit,
    };

    let stats = mine::run(&args.input, &args.out, &filters, true)?;

    println!("games seen                {}", stats.games_seen);
    println!("games passing headers     {}", stats.games_scanned);
    println!("  of those, with evals    {}", stats.games_with_eval);
    println!("  without any eval        {}", stats.rejected_no_eval);
    println!("{}-piece positions scored  {}", args.pieces, stats.positions_at_size);
    println!("  outside eval band       {}", stats.rejected_band);
    println!("  in band but unstable    {}", stats.rejected_unstable);
    println!("  symmetric material      {}", stats.rejected_symmetric);
    println!("candidates after board    {}", stats.candidates);
    println!("  game ended too soon     {}", stats.rejected_tail);
    println!("  same game, already kept {}", stats.rejected_same_game);
    println!("  duplicate position      {}", stats.duplicates);
    println!("records written           {}", stats.written);
    println!("\n-> {}", args.out.display());
    Ok(())
}

fn cmd_verify(args: VerifyArgs) -> Result<()> {
    let opts = verify::VerifyOpts {
        engine: args.engine,
        depth: args.depth,
        threads_per_engine: args.engine_threads,
        hash_mb: args.hash_mb,
        workers: args.workers,
        eval_band_cp: args.band,
        max_holding: args.max_holding,
        min_legal: args.min_legal,
        max_draw_permille: args.max_draw,
        limit: args.limit,
    };
    let stats = verify::run(&args.input, &args.out, &opts, true)?;
    print_verify_stats(&stats);
    println!("\n-> {}", args.out.display());
    Ok(())
}

fn cmd_filter(args: FilterArgs) -> Result<()> {
    let opts = verify::VerifyOpts {
        eval_band_cp: args.band,
        max_holding: args.max_holding,
        min_legal: args.min_legal,
        max_draw_permille: args.max_draw,
        ..verify::VerifyOpts::default()
    };
    let stats = verify::filter(&args.input, &args.out, &opts)?;
    print_verify_stats(&stats);
    println!("\n-> {}", args.out.display());
    Ok(())
}

fn print_verify_stats(stats: &verify::VerifyStats) {
    println!("scored              {}", stats.scored);
    println!("  outside band      {}", stats.dropped_band);
    println!("  too few moves     {}", stats.dropped_forced);
    println!("  flat (not sharp)  {}", stats.dropped_flat);
    println!("  too drawish       {}", stats.dropped_drawish);
    println!("kept                {}", stats.kept);
}

fn cmd_stats(args: FileArg) -> Result<()> {
    let reader = Reader::open(&args.file)?;
    let count = reader.count;
    let sorted = reader.flags & format::FILE_FLAG_SORTED != 0;

    let mut eval_sum = 0i64;
    let mut white_wins = 0u64;
    let mut verified = 0u64;
    let mut draw_permille_sum = 0u64;
    let mut drop_sum = 0u64;
    let mut sharp = 0u64;
    let mut earliest = i64::MAX;
    let mut latest = 0i64;
    let mut ply_sum = 0u64;

    for rec in reader {
        let rec = rec?;
        eval_sum += rec.eval_cp as i64;
        ply_sum += rec.ply as u64;
        if rec.winner == 0 {
            white_wins += 1;
        }
        if rec.verified_cp != format::EVAL_UNSET {
            verified += 1;
            draw_permille_sum += rec.wdl_draw as u64;
            drop_sum += rec.drop_cp.max(0) as u64;
            if rec.holding_moves > 0 && rec.holding_moves <= 2 {
                sharp += 1;
            }
        }
        if rec.utc_time > 0 {
            earliest = earliest.min(rec.utc_time);
            latest = latest.max(rec.utc_time);
        }
    }

    println!("file        {}", args.file.display());
    println!("records     {count}");
    println!("sorted      {sorted}");
    if count > 0 {
        println!("mean eval   {:+.3} pawns", eval_sum as f64 / count as f64 / 100.0);
        println!("mean ply    {:.1}", ply_sum as f64 / count as f64);
        println!(
            "white wins  {white_wins} ({:.1}%)",
            white_wins as f64 * 100.0 / count as f64
        );
        println!(
            "date range  {} .. {}",
            review::format_date(if earliest == i64::MAX { 0 } else { earliest }),
            review::format_date(latest)
        );
        println!("verified    {verified}");
        if verified > 0 {
            println!(
                "mean draw%  {:.1}%",
                draw_permille_sum as f64 / verified as f64 / 10.0
            );
            println!(
                "mean drop   {:.2} pawns",
                drop_sum as f64 / verified as f64 / 100.0
            );
            println!(
                "sharp (<=2) {sharp} ({:.1}% of verified)",
                sharp as f64 * 100.0 / verified as f64
            );
        }
    }
    Ok(())
}

/// Reads `limit` records spread across the file with the given stride.
fn sample(path: &std::path::Path, limit: usize, stride: usize) -> Result<Vec<format::Record>> {
    let reader = Reader::open(path)?;
    let count = reader.count as usize;
    let stride = if stride == 0 {
        (count / limit.max(1)).max(1)
    } else {
        stride
    };
    let mut out = Vec::with_capacity(limit);
    for (i, rec) in reader.enumerate() {
        if i % stride != 0 {
            continue;
        }
        out.push(rec?);
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

fn cmd_dump(args: DumpArgs) -> Result<()> {
    let records = sample(&args.file, args.limit, args.stride)?;
    for rec in &records {
        if args.json {
            println!("{}", serde_json::to_string(&review::to_json(rec))?);
        } else {
            println!("{}", review::text_block(rec));
        }
    }
    Ok(())
}

fn cmd_review(args: ReviewArgs) -> Result<()> {
    let records = sample(&args.file, args.limit, args.stride)?;
    let meta = review::PageMeta {
        title: args.title,
        subtitle: if args.subtitle.is_empty() {
            format!("{} positions from {}", records.len(), args.file.display())
        } else {
            args.subtitle
        },
        notes: args.notes,
    };
    review::write_html(&args.out, &records, &meta)
        .with_context(|| format!("writing {}", args.out.display()))?;
    println!("{} positions -> {}", records.len(), args.out.display());
    Ok(())
}
