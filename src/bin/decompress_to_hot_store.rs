//! CLI tool for decompressing DBN files to a hot store.
//!
//! This tool pre-decompresses compressed DBN files (.dbn.zst) to eliminate
//! the zstd decompression bottleneck during data processing.
//!
//! # Usage
//!
//! ```bash
//! # Decompress a single file
//! cargo run --release --bin decompress_to_hot_store -- \
//!     --input data/NVDA.mbo.dbn.zst \
//!     --output data/hot/
//!
//! # Decompress all files in a directory
//! cargo run --release --bin decompress_to_hot_store -- \
//!     --input data/NVDA_2025-02-01_to_2025-09-30/ \
//!     --output data/hot/
//!
//! # Check space requirements first (dry run)
//! cargo run --release --bin decompress_to_hot_store -- \
//!     --input data/NVDA_2025-02-01_to_2025-09-30/ \
//!     --output data/hot/ \
//!     --dry-run
//! ```
//!
//! # Performance
//!
//! Decompression is I/O bound, typically achieving:
//! - ~150-200 MB/s read (compressed)
//! - ~500-700 MB/s write (decompressed)
//!
//! A 1GB compressed file takes ~5-10 seconds to decompress.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use mbo_lob_reconstructor::hotstore::{HotStoreConfig, HotStoreManager};
use mbo_lob_reconstructor::Result;

/// Command-line arguments
struct Args {
    /// Input file or directory containing .dbn.zst files
    input: PathBuf,
    /// Output directory for decompressed files
    output: PathBuf,
    /// Dry run - show what would be done without doing it
    dry_run: bool,
    /// Force re-decompression of existing files
    force: bool,
    /// Verbose output
    verbose: bool,
}

fn parse_args() -> std::result::Result<Args, String> {
    let args: Vec<String> = env::args().collect();
    
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut dry_run = false;
    let mut force = false;
    let mut verbose = false;
    
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--input" | "-i" => {
                i += 1;
                if i >= args.len() {
                    return Err("--input requires a path".to_string());
                }
                input = Some(PathBuf::from(&args[i]));
            }
            "--output" | "-o" => {
                i += 1;
                if i >= args.len() {
                    return Err("--output requires a path".to_string());
                }
                output = Some(PathBuf::from(&args[i]));
            }
            "--dry-run" | "-n" => {
                dry_run = true;
            }
            "--force" | "-f" => {
                force = true;
            }
            "--verbose" | "-v" => {
                verbose = true;
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            arg => {
                // Positional arguments
                if input.is_none() {
                    input = Some(PathBuf::from(arg));
                } else if output.is_none() {
                    output = Some(PathBuf::from(arg));
                } else {
                    return Err(format!("Unknown argument: {}", arg));
                }
            }
        }
        i += 1;
    }
    
    let input = input.ok_or("Input path is required")?;
    let output = output.ok_or("Output directory is required")?;
    
    Ok(Args {
        input,
        output,
        dry_run,
        force,
        verbose,
    })
}

fn print_help() {
    eprintln!(r#"
Decompress DBN Files to Hot Store

Pre-decompresses .dbn.zst files to eliminate the zstd decompression
bottleneck during data processing.

USAGE:
    decompress_to_hot_store [OPTIONS] --input <PATH> --output <DIR>
    decompress_to_hot_store <INPUT> <OUTPUT>

OPTIONS:
    -i, --input <PATH>    Input file or directory containing .dbn.zst files
    -o, --output <DIR>    Output directory for decompressed files
    -n, --dry-run         Show what would be done without doing it
    -f, --force           Re-decompress files that already exist
    -v, --verbose         Show detailed progress
    -h, --help            Print this help message

EXAMPLES:
    # Decompress a single file
    decompress_to_hot_store -i data/NVDA.mbo.dbn.zst -o data/hot/

    # Decompress all files in a directory
    decompress_to_hot_store -i data/raw/ -o data/hot/

    # Check space requirements (dry run)
    decompress_to_hot_store -i data/raw/ -o data/hot/ --dry-run

NOTES:
    - Decompressed files are typically 3-4x larger than compressed
    - Existing files are skipped unless --force is specified
    - Progress is shown for multi-file operations
"#);
}

/// Find all .dbn.zst files in a path (file or directory)
fn find_compressed_files(path: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    
    if path.is_file() {
        if path.extension().map_or(false, |e| e == "zst") {
            files.push(path.to_path_buf());
        }
    } else if path.is_dir() {
        for entry in fs::read_dir(path).map_err(|e| {
            mbo_lob_reconstructor::TlobError::generic(format!(
                "Failed to read directory {}: {}",
                path.display(),
                e
            ))
        })? {
            let entry = entry.map_err(|e| {
                mbo_lob_reconstructor::TlobError::generic(format!(
                    "Failed to read entry: {}",
                    e
                ))
            })?;
            let path = entry.path();
            
            // Check for .dbn.zst extension
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.ends_with(".dbn.zst") {
                    files.push(path);
                }
            }
        }
    } else {
        return Err(mbo_lob_reconstructor::TlobError::generic(format!(
            "Path does not exist: {}",
            path.display()
        )));
    }
    
    files.sort();
    Ok(files)
}

/// Format bytes as human-readable string
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    
    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}

/// Format duration as human-readable string
fn format_duration(secs: f64) -> String {
    if secs >= 60.0 {
        let mins = secs / 60.0;
        format!("{:.1} min", mins)
    } else {
        format!("{:.1}s", secs)
    }
}

fn main() {
    // Initialize logging
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();
    
    // Parse arguments
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("Error: {}", e);
            eprintln!("Use --help for usage information");
            std::process::exit(1);
        }
    };
    
    // Find files to process
    let files = match find_compressed_files(&args.input) {
        Ok(files) => files,
        Err(e) => {
            eprintln!("Error finding files: {}", e);
            std::process::exit(1);
        }
    };
    
    if files.is_empty() {
        eprintln!("No .dbn.zst files found in {}", args.input.display());
        std::process::exit(0);
    }
    
    println!("Found {} compressed file(s)", files.len());
    
    // Create hot store manager
    let config = HotStoreConfig::dbn_defaults(&args.output);
    let manager = HotStoreManager::new(config);
    
    // Calculate sizes
    let mut total_compressed_size: u64 = 0;
    let mut files_to_process: Vec<(PathBuf, u64)> = Vec::new();
    let mut skipped = 0;
    
    for file in &files {
        let size = fs::metadata(file)
            .map(|m| m.len())
            .unwrap_or(0);
        
        // Check if already decompressed
        if !args.force && manager.has_decompressed(file) {
            if args.verbose {
                println!("  Skip (exists): {}", file.file_name().unwrap().to_str().unwrap());
            }
            skipped += 1;
            continue;
        }
        
        total_compressed_size += size;
        files_to_process.push((file.clone(), size));
    }
    
    // Estimate decompressed size (typically 3-4x)
    let estimated_decompressed = total_compressed_size * 4;
    
    println!("\nSummary:");
    println!("  Files to decompress: {}", files_to_process.len());
    println!("  Files skipped (exist): {}", skipped);
    println!("  Compressed size: {}", format_bytes(total_compressed_size));
    println!("  Estimated decompressed: ~{}", format_bytes(estimated_decompressed));
    println!("  Output directory: {}", args.output.display());
    
    if args.dry_run {
        println!("\n[DRY RUN] No files were decompressed.");
        println!("Remove --dry-run to perform decompression.");
        return;
    }
    
    if files_to_process.is_empty() {
        println!("\nAll files already decompressed. Use --force to re-decompress.");
        return;
    }
    
    // Create output directory
    if let Err(e) = fs::create_dir_all(&args.output) {
        eprintln!("Error creating output directory: {}", e);
        std::process::exit(1);
    }
    
    // Process files
    println!("\nDecompressing...\n");
    
    let start_time = Instant::now();
    let mut processed = 0;
    let mut total_bytes_written: u64 = 0;
    let mut errors = 0;
    
    for (i, (file, compressed_size)) in files_to_process.iter().enumerate() {
        let filename = file.file_name().unwrap().to_str().unwrap();
        
        print!(
            "[{}/{}] {} ({})...",
            i + 1,
            files_to_process.len(),
            filename,
            format_bytes(*compressed_size)
        );
        
        let file_start = Instant::now();
        
        match manager.decompress(file) {
            Ok(decompressed_path) => {
                let decompressed_size = fs::metadata(&decompressed_path)
                    .map(|m| m.len())
                    .unwrap_or(0);
                
                let elapsed = file_start.elapsed().as_secs_f64();
                let throughput = *compressed_size as f64 / elapsed / (1024.0 * 1024.0);
                
                println!(
                    " done ({} → {}, {:.1} MB/s)",
                    format_bytes(*compressed_size),
                    format_bytes(decompressed_size),
                    throughput
                );
                
                processed += 1;
                total_bytes_written += decompressed_size;
            }
            Err(e) => {
                println!(" ERROR: {}", e);
                errors += 1;
            }
        }
    }
    
    let total_elapsed = start_time.elapsed().as_secs_f64();
    let avg_throughput = total_compressed_size as f64 / total_elapsed / (1024.0 * 1024.0);
    
    println!("\n{}", "=".repeat(60));
    println!("Decompression Complete!");
    println!("  Files processed: {}", processed);
    println!("  Errors: {}", errors);
    println!("  Total written: {}", format_bytes(total_bytes_written));
    println!("  Total time: {}", format_duration(total_elapsed));
    println!("  Average throughput: {:.1} MB/s (compressed)", avg_throughput);
    
    if errors > 0 {
        std::process::exit(1);
    }
}

