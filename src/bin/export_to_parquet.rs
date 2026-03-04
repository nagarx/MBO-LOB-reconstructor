//! CLI tool for exporting raw LOB snapshots and MBO events to Parquet files.
//!
//! Processes `.dbn.zst` (or decompressed `.dbn`) files through the LOB
//! reconstructor and writes the raw snapshots to Parquet, providing an
//! unbiased data source for downstream statistical analysis.
//!
//! # Usage
//!
//! ```bash
//! # Export using a TOML config file (recommended for reproducibility)
//! cargo run --release --features export --bin export_to_parquet -- \
//!     --config configs/nvda_full_export.toml
//!
//! # Export a single day (CLI flags)
//! cargo run --release --features export --bin export_to_parquet -- \
//!     --input data/NVDA/xnas-itch-20250203.mbo.dbn.zst \
//!     --output data/exports/raw_lob/ \
//!     --symbol NVDA
//!
//! # CLI flags override config values
//! cargo run --release --features export --bin export_to_parquet -- \
//!     --config configs/nvda_full_export.toml \
//!     --levels 5 --compression snappy
//! ```

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use mbo_lob_reconstructor::export::{
    DownsampleConfig, DownsampleStrategy, ExportConfig, LobSnapshotWriter, MboEventWriter,
};
use mbo_lob_reconstructor::hotstore::HotStoreManager;
use mbo_lob_reconstructor::{DbnLoader, LobReconstructor, LobState, Result, TlobError};

use parquet::basic::Compression;
use serde::Deserialize;

// ─────────────────────────────────────────────────────────────────────────────
// TOML config schema
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
struct TomlConfig {
    export: Option<TomlExport>,
    input: Option<TomlInput>,
    output: Option<TomlOutput>,
}

#[derive(Debug, Deserialize, Default)]
struct TomlExport {
    symbol: Option<String>,
    levels: Option<usize>,
    include_derived: Option<bool>,
    include_mbo: Option<bool>,
    batch_size: Option<usize>,
    compression: Option<String>,
    downsample_every: Option<usize>,
    downsample_interval_ns: Option<u64>,
}

#[derive(Debug, Deserialize, Default)]
struct TomlInput {
    path: Option<String>,
    hot_store: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct TomlOutput {
    path: Option<String>,
}

fn load_toml_config(path: &Path) -> std::result::Result<TomlConfig, String> {
    let contents = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read config '{}': {e}", path.display()))?;
    toml::from_str(&contents)
        .map_err(|e| format!("Failed to parse config '{}': {e}", path.display()))
}

// ─────────────────────────────────────────────────────────────────────────────
// Resolved arguments (TOML defaults merged with CLI overrides)
// ─────────────────────────────────────────────────────────────────────────────

struct Args {
    input: PathBuf,
    output: PathBuf,
    symbol: String,
    levels: usize,
    include_derived: bool,
    include_mbo: bool,
    batch_size: usize,
    compression: Compression,
    downsample_every: Option<usize>,
    downsample_interval_ns: Option<u64>,
    hot_store: Option<PathBuf>,
    verbose: bool,
}

fn parse_compression(s: &str) -> std::result::Result<Compression, String> {
    match s {
        "snappy" => Ok(Compression::SNAPPY),
        "zstd" => Ok(Compression::ZSTD(Default::default())),
        "none" | "uncompressed" => Ok(Compression::UNCOMPRESSED),
        other => Err(format!(
            "Unknown compression: {other}. Use 'snappy', 'zstd', or 'none'"
        )),
    }
}

fn parse_args() -> std::result::Result<Args, String> {
    let args: Vec<String> = env::args().collect();

    // First pass: find --config if present
    let mut config_path: Option<PathBuf> = None;
    let mut cli_input: Option<PathBuf> = None;
    let mut cli_output: Option<PathBuf> = None;
    let mut cli_symbol: Option<String> = None;
    let mut cli_levels: Option<usize> = None;
    let mut cli_include_derived: Option<bool> = None;
    let mut cli_include_mbo: Option<bool> = None;
    let mut cli_batch_size: Option<usize> = None;
    let mut cli_compression: Option<Compression> = None;
    let mut cli_downsample_every: Option<usize> = None;
    let mut cli_downsample_interval_ns: Option<u64> = None;
    let mut cli_hot_store: Option<PathBuf> = None;
    let mut verbose = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--config" | "-c" => {
                i += 1;
                config_path = Some(PathBuf::from(
                    args.get(i).ok_or("--config requires a path")?,
                ));
            }
            "--input" | "-i" => {
                i += 1;
                cli_input = Some(PathBuf::from(args.get(i).ok_or("--input requires a path")?));
            }
            "--output" | "-o" => {
                i += 1;
                cli_output = Some(PathBuf::from(
                    args.get(i).ok_or("--output requires a path")?,
                ));
            }
            "--symbol" | "-s" => {
                i += 1;
                cli_symbol = Some(args.get(i).ok_or("--symbol requires a value")?.clone());
            }
            "--levels" | "-l" => {
                i += 1;
                cli_levels = Some(
                    args.get(i)
                        .ok_or("--levels requires a number")?
                        .parse()
                        .map_err(|_| "--levels must be a positive integer")?,
                );
            }
            "--no-derived" => {
                cli_include_derived = Some(false);
            }
            "--no-mbo" => {
                cli_include_mbo = Some(false);
            }
            "--batch-size" => {
                i += 1;
                cli_batch_size = Some(
                    args.get(i)
                        .ok_or("--batch-size requires a number")?
                        .parse()
                        .map_err(|_| "--batch-size must be a positive integer")?,
                );
            }
            "--compression" => {
                i += 1;
                let val = args.get(i).ok_or("--compression requires a value")?;
                cli_compression = Some(parse_compression(val)?);
            }
            "--downsample-every" => {
                i += 1;
                cli_downsample_every = Some(
                    args.get(i)
                        .ok_or("--downsample-every requires a number")?
                        .parse()
                        .map_err(|_| "--downsample-every must be a positive integer")?,
                );
            }
            "--downsample-interval-ns" => {
                i += 1;
                cli_downsample_interval_ns = Some(
                    args.get(i)
                        .ok_or("--downsample-interval-ns requires a number")?
                        .parse()
                        .map_err(|_| "--downsample-interval-ns must be a positive integer")?,
                );
            }
            "--hot-store" => {
                i += 1;
                cli_hot_store = Some(PathBuf::from(
                    args.get(i).ok_or("--hot-store requires a path")?,
                ));
            }
            "--verbose" | "-v" => {
                verbose = true;
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            arg => {
                if cli_input.is_none() {
                    cli_input = Some(PathBuf::from(arg));
                } else if cli_output.is_none() {
                    cli_output = Some(PathBuf::from(arg));
                } else {
                    return Err(format!("Unknown argument: {arg}"));
                }
            }
        }
        i += 1;
    }

    // Load TOML defaults if --config was provided
    let toml = match config_path {
        Some(ref p) => load_toml_config(p)?,
        None => TomlConfig::default(),
    };

    let toml_export = toml.export.unwrap_or_default();
    let toml_input = toml.input.unwrap_or_default();
    let toml_output = toml.output.unwrap_or_default();

    // Resolve compression: CLI > TOML > default (snappy)
    let compression = match cli_compression {
        Some(c) => c,
        None => match toml_export.compression.as_deref() {
            Some(s) => parse_compression(s)?,
            None => Compression::SNAPPY,
        },
    };

    // Resolve all fields: CLI overrides TOML overrides defaults
    let input = cli_input
        .or_else(|| toml_input.path.map(PathBuf::from))
        .ok_or("Input path is required (--input or config [input].path)")?;

    let output = cli_output
        .or_else(|| toml_output.path.map(PathBuf::from))
        .ok_or("Output directory is required (--output or config [output].path)")?;

    Ok(Args {
        input,
        output,
        symbol: cli_symbol
            .or(toml_export.symbol)
            .unwrap_or_else(|| "UNKNOWN".into()),
        levels: cli_levels.or(toml_export.levels).unwrap_or(10),
        include_derived: cli_include_derived
            .or(toml_export.include_derived)
            .unwrap_or(true),
        include_mbo: cli_include_mbo
            .or(toml_export.include_mbo)
            .unwrap_or(true),
        batch_size: cli_batch_size.or(toml_export.batch_size).unwrap_or(65_536),
        compression,
        downsample_every: cli_downsample_every.or(toml_export.downsample_every),
        downsample_interval_ns: cli_downsample_interval_ns
            .or(toml_export.downsample_interval_ns),
        hot_store: cli_hot_store.or_else(|| toml_input.hot_store.map(PathBuf::from)),
        verbose,
    })
}

fn print_help() {
    eprintln!(
        r#"
Export LOB Snapshots & MBO Events to Parquet

Processes .dbn.zst files through the LOB reconstructor and writes raw
LOB snapshots (and optionally MBO events) to Apache Parquet files.

USAGE:
    export_to_parquet [OPTIONS] --input <PATH> --output <DIR>
    export_to_parquet --config <PATH.toml> [OPTIONS]

CONFIG FILE:
    -c, --config <PATH>            TOML config file (CLI flags override)

OPTIONS:
    -i, --input <PATH>             Input .dbn.zst file or directory
    -o, --output <DIR>             Output directory for Parquet files
    -s, --symbol <SYM>             Ticker symbol (default: UNKNOWN)
    -l, --levels <N>               LOB levels to export (default: 10, max: 20)
        --no-derived               Omit derived columns (mid_price, spread, etc.)
        --no-mbo                   Skip MBO event export
        --batch-size <N>           Rows per row group (default: 65536)
        --compression <TYPE>       snappy (default) | zstd | none
        --downsample-every <N>     Export every N-th LOB snapshot
        --downsample-interval-ns <NS>  Min nanoseconds between exports
        --hot-store <DIR>          Use pre-decompressed hot store
    -v, --verbose                  Show detailed progress
    -h, --help                     Print this help message

OUTPUT FILES (per day):
    {{date}}_lob_snapshots.parquet        -- LOB state at each message
    {{date}}_mbo_events.parquet           -- Raw MBO messages (if --no-mbo not set)
    {{date}}_reconstruction_stats.json    -- Reconstruction provenance stats
"#
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Day-level result for progress tracking
// ─────────────────────────────────────────────────────────────────────────────

struct DayResult {
    date: String,
    messages: u64,
    lob_rows: u64,
    mbo_rows: u64,
    elapsed_secs: f64,
}

// ─────────────────────────────────────────────────────────────────────────────
// File discovery
// ─────────────────────────────────────────────────────────────────────────────

/// Find all .dbn.zst and .dbn files in a path.
fn find_dbn_files(path: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if path.is_file() {
        files.push(path.to_path_buf());
    } else if path.is_dir() {
        for entry in fs::read_dir(path).map_err(|e| {
            TlobError::generic(format!("Failed to read directory {}: {e}", path.display()))
        })? {
            let entry = entry.map_err(|e| TlobError::generic(format!("Read entry error: {e}")))?;
            let p = entry.path();
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                if name.ends_with(".dbn.zst") || name.ends_with(".mbo.dbn") {
                    files.push(p);
                }
            }
        }
    } else {
        return Err(TlobError::generic(format!(
            "Path does not exist: {}",
            path.display()
        )));
    }
    files.sort();
    Ok(files)
}

/// Extract date from filename like `xnas-itch-20250203.mbo.dbn.zst`.
fn extract_date(path: &Path) -> String {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    for part in name.split(&['-', '.', '_']) {
        if part.len() == 8 && part.chars().all(|c| c.is_ascii_digit()) {
            return format!("{}-{}-{}", &part[0..4], &part[4..6], &part[6..8]);
        }
    }
    name.to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
// Day processing
// ─────────────────────────────────────────────────────────────────────────────

fn process_day(
    dbn_path: &Path,
    output_dir: &Path,
    config: &ExportConfig,
    symbol: &str,
    levels: usize,
) -> Result<DayResult> {
    let date = extract_date(dbn_path);
    let day_start = Instant::now();

    let loader = DbnLoader::new(dbn_path)?.skip_invalid(true);
    let mut lob = LobReconstructor::new(levels);
    let mut lob_state = LobState::new(levels);

    let mut extra_meta = HashMap::new();
    extra_meta.insert("date".into(), date.clone());
    extra_meta.insert("symbol".into(), symbol.into());

    let lob_path = output_dir.join(format!("{date}_lob_snapshots.parquet"));
    let mut lob_writer = LobSnapshotWriter::new(&lob_path, config, extra_meta.clone())?;

    let mut mbo_writer = if config.include_mbo_events {
        let mbo_path = output_dir.join(format!("{date}_mbo_events.parquet"));
        Some(MboEventWriter::new(&mbo_path, config, extra_meta)?)
    } else {
        None
    };

    let mut msg_count: u64 = 0;

    for msg in loader.iter_messages()? {
        msg_count += 1;

        if let Some(ref mut mw) = mbo_writer {
            mw.write_event(&msg)?;
        }

        if lob.process_message_into(&msg, &mut lob_state).is_ok() && lob_state.is_valid() {
            lob_writer.write_snapshot(&lob_state)?;
        }
    }

    let lob_export_stats = lob_writer.finish()?;
    let mbo_rows = if let Some(mw) = mbo_writer {
        let stats = mw.finish()?;
        stats.rows_written
    } else {
        0
    };

    // Persist reconstruction stats for provenance auditing
    let stats_path = output_dir.join(format!("{date}_reconstruction_stats.json"));
    if let Err(e) = lob.stats().export_to_file(&stats_path) {
        eprintln!(
            "Warning: failed to write stats for {date}: {e}"
        );
    }

    let elapsed_secs = day_start.elapsed().as_secs_f64();

    Ok(DayResult {
        date,
        messages: msg_count,
        lob_rows: lob_export_stats.rows_written,
        mbo_rows,
        elapsed_secs,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Export summary
// ─────────────────────────────────────────────────────────────────────────────

fn write_export_summary(
    output_dir: &Path,
    symbol: &str,
    days_ok: usize,
    days_err: usize,
    total_lob: u64,
    total_mbo: u64,
    total_messages: u64,
    elapsed_secs: f64,
) {
    let summary = serde_json::json!({
        "symbol": symbol,
        "days_processed": days_ok,
        "days_failed": days_err,
        "total_lob_snapshots": total_lob,
        "total_mbo_events": total_mbo,
        "total_messages": total_messages,
        "elapsed_seconds": elapsed_secs,
        "avg_messages_per_sec": if elapsed_secs > 0.0 { total_messages as f64 / elapsed_secs } else { 0.0 },
        "source": "mbo-lob-reconstructor",
        "version": env!("CARGO_PKG_VERSION"),
    });

    let path = output_dir.join("_export_summary.json");
    match fs::File::create(&path) {
        Ok(f) => {
            let _ = serde_json::to_writer_pretty(std::io::BufWriter::new(f), &summary);
        }
        Err(e) => eprintln!("Warning: failed to write export summary: {e}"),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Main
// ─────────────────────────────────────────────────────────────────────────────

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Error: {e}");
            eprintln!("Use --help for usage information");
            std::process::exit(1);
        }
    };

    let downsample = match (args.downsample_every, args.downsample_interval_ns) {
        (Some(n), _) => Some(DownsampleConfig {
            strategy: DownsampleStrategy::EveryN(n),
        }),
        (_, Some(ns)) => Some(DownsampleConfig {
            strategy: DownsampleStrategy::MinIntervalNs(ns),
        }),
        _ => None,
    };

    let config = ExportConfig {
        levels: args.levels,
        include_derived: args.include_derived,
        include_mbo_events: args.include_mbo,
        batch_size: args.batch_size,
        compression: args.compression,
        downsample,
    };

    let input_path = if let Some(ref hs) = args.hot_store {
        let mgr = HotStoreManager::new(
            mbo_lob_reconstructor::hotstore::HotStoreConfig::dbn_defaults(hs),
        );
        let decompressed = mgr.decompressed_path_for(&args.input);
        if decompressed.exists() {
            decompressed
        } else {
            args.input.clone()
        }
    } else {
        args.input.clone()
    };

    let files = match find_dbn_files(&input_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error finding DBN files: {e}");
            std::process::exit(1);
        }
    };

    if files.is_empty() {
        eprintln!(
            "No .dbn.zst or .dbn files found in {}",
            input_path.display()
        );
        std::process::exit(0);
    }

    if let Err(e) = fs::create_dir_all(&args.output) {
        eprintln!("Error creating output directory: {e}");
        std::process::exit(1);
    }

    let compression_name = match args.compression {
        Compression::SNAPPY => "snappy",
        Compression::ZSTD(_) => "zstd",
        Compression::UNCOMPRESSED => "none",
        _ => "other",
    };

    println!(
        "Exporting {} day(s) to Parquet (symbol={}, levels={}, derived={}, mbo={}, compression={})",
        files.len(),
        args.symbol,
        config.levels,
        config.include_derived,
        config.include_mbo_events,
        compression_name,
    );
    println!("  Input:  {}", input_path.display());
    println!("  Output: {}", args.output.display());
    println!();

    let start = Instant::now();
    let mut total_lob: u64 = 0;
    let mut total_mbo: u64 = 0;
    let mut total_messages: u64 = 0;
    let mut errors: usize = 0;
    let mut day_times: Vec<f64> = Vec::new();

    for (i, file) in files.iter().enumerate() {
        let day_num = i + 1;
        let total_days = files.len();
        let name = file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("<unknown>");

        if !args.verbose {
            eprint!("\r[{day_num}/{total_days}] {name}...");
        }

        match process_day(
            file,
            &args.output,
            &config,
            &args.symbol,
            config.effective_levels(),
        ) {
            Ok(result) => {
                total_lob += result.lob_rows;
                total_mbo += result.mbo_rows;
                total_messages += result.messages;
                day_times.push(result.elapsed_secs);

                let throughput =
                    result.messages as f64 / result.elapsed_secs.max(0.001);

                if args.verbose {
                    let avg_day_secs: f64 =
                        day_times.iter().sum::<f64>() / day_times.len() as f64;
                    let remaining = (total_days - day_num) as f64 * avg_day_secs;

                    eprintln!(
                        "  [{day_num}/{total_days}] {date}: {msgs} msgs in {elapsed:.1}s \
                         ({throughput:.0} msg/s), LOB: {lob}, MBO: {mbo} | ETA: {eta}",
                        date = result.date,
                        msgs = result.messages,
                        elapsed = result.elapsed_secs,
                        lob = result.lob_rows,
                        mbo = result.mbo_rows,
                        eta = format_eta(remaining),
                    );
                } else {
                    let avg_day_secs: f64 =
                        day_times.iter().sum::<f64>() / day_times.len() as f64;
                    let remaining = (total_days - day_num) as f64 * avg_day_secs;

                    eprint!(
                        "\r[{day_num}/{total_days}] {name} done ({throughput:.0} msg/s) | ETA: {eta}   ",
                        eta = format_eta(remaining),
                    );
                }
            }
            Err(e) => {
                eprintln!("\nError processing {name}: {e}");
                errors += 1;
            }
        }
    }

    let elapsed = start.elapsed().as_secs_f64();
    let overall_throughput = total_messages as f64 / elapsed.max(0.001);

    println!("\n\n{}", "=".repeat(60));
    println!("Export Complete");
    println!("{}", "=".repeat(60));
    println!("  Symbol:         {}", args.symbol);
    println!("  Days processed: {}", files.len() - errors);
    println!("  Days failed:    {errors}");
    println!("  LOB snapshots:  {total_lob}");
    println!("  MBO events:     {total_mbo}");
    println!("  Total messages: {total_messages}");
    println!("  Time:           {}", format_eta(elapsed));
    println!("  Throughput:     {overall_throughput:.0} msg/s");
    println!("  Output:         {}", args.output.display());

    write_export_summary(
        &args.output,
        &args.symbol,
        files.len() - errors,
        errors,
        total_lob,
        total_mbo,
        total_messages,
        elapsed,
    );

    if errors > 0 {
        std::process::exit(1);
    }
}

fn format_eta(seconds: f64) -> String {
    let total = seconds as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}h{m:02}m{s:02}s")
    } else if m > 0 {
        format!("{m}m{s:02}s")
    } else {
        format!("{s}s")
    }
}
