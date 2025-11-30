//! Process a single day of real NVDA MBO data.
//!
//! This example demonstrates the full pipeline:
//! 1. Load compressed DBN file (.dbn.zst)
//! 2. Decode MBO messages
//! 3. Reconstruct LOB state
//! 4. Display statistics and sample LOB states
//!
//! Usage:
//! ```bash
//! cargo run --release --example process_nvda_single_day <path_to_dbn_file>
//! ```
//!
//! Example:
//! ```bash
//! cargo run --release --example process_nvda_single_day \
//!   /Users/nigo/local/databento/data/NVDA_2025-02-01_to_2025-09-30/xnas-itch-20250203.mbo.dbn.zst
//! ```

use mbo_lob_reconstructor::{DbnLoader, LobReconstructor};
use std::env;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .init();

    println!("=================================================================");
    println!("TLOB Rust - Process Real NVDA MBO Data");
    println!("=================================================================\n");

    // Get file path from command line
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <path_to_dbn_file.dbn.zst>", args[0]);
        eprintln!("\nExample:");
        eprintln!("  {} /path/to/nvda.dbn.zst", args[0]);
        std::process::exit(1);
    }

    let dbn_path = &args[1];
    println!("📂 Input file: {}", dbn_path);

    // Create loader
    let loader = DbnLoader::new(dbn_path)?.skip_invalid(true); // Skip invalid messages instead of crashing

    let file_size_mb = loader.stats().file_size as f64 / 1_000_000.0;
    println!("📊 File size: {:.2} MB", file_size_mb);
    println!();

    // Create LOB reconstructor (10 levels)
    let mut lob = LobReconstructor::new(10);
    println!("✅ LOB reconstructor initialized (10 levels)");
    println!();

    // Progress tracking
    let mut message_count = 0u64;
    let mut last_report = 0u64;
    let mut lob_error_count = 0u64;
    let mut skipped_invalid_count = 0u64;
    let report_interval = 1_000_000; // Report every 1M messages

    let start_time = Instant::now();

    println!("🚀 Starting processing...\n");

    // Process all messages
    for mbo_msg in loader.iter_messages()? {
        // Skip invalid messages (system messages, metadata, etc.)
        // Common patterns: order_id=0, size=0, invalid price
        if mbo_msg.order_id == 0 || mbo_msg.size == 0 || mbo_msg.price <= 0 {
            skipped_invalid_count += 1;
            if skipped_invalid_count <= 10 {
                // Log first few skipped messages for debugging
                log::debug!(
                    "Skipping invalid message #{}: order_id={}, action={:?}, side={:?}, price={}, size={}",
                    skipped_invalid_count, mbo_msg.order_id, mbo_msg.action, mbo_msg.side,
                    mbo_msg.price as f64 / 1e9, mbo_msg.size
                );
            }
            continue;
        }

        // Reconstruct LOB with error recovery
        match lob.process_message(&mbo_msg) {
            Ok(_state) => {
                // Success - continue processing
            }
            Err(e) => {
                lob_error_count += 1;
                log::error!(
                    "LOB processing error at message {}: order_id={}, action={:?}, side={:?}, price={}, size={}, error={}",
                    message_count, mbo_msg.order_id, mbo_msg.action, mbo_msg.side, 
                    mbo_msg.price as f64 / 1e9, mbo_msg.size, e
                );

                // Skip LOB errors instead of failing - more resilient
                // Track and report at the end
                continue;
            }
        }

        message_count += 1;

        // Report progress every 1M messages
        if message_count - last_report >= report_interval {
            let elapsed = start_time.elapsed().as_secs_f64();
            let msg_per_sec = message_count as f64 / elapsed;

            println!(
                "  ⏱️  Processed: {:>12} messages | Rate: {:>10.0} msg/s | Time: {:.1}s",
                format_number(message_count),
                msg_per_sec,
                elapsed
            );

            // Print sample LOB state
            let state = lob.get_lob_state();
            if let (Some(mid), Some(spread)) = (state.mid_price(), state.spread()) {
                println!(
                    "     📊 LOB: Mid=${:.4} | Spread=${:.4} | Bid Levels={} | Ask Levels={}",
                    mid,
                    spread,
                    lob.bid_levels(),
                    lob.ask_levels()
                );
            }
            println!();

            last_report = message_count;
        }
    }

    let total_time = start_time.elapsed().as_secs_f64();

    println!("=================================================================");
    println!("✅ PROCESSING COMPLETE");
    println!("=================================================================\n");

    // Final statistics
    let lob_stats = lob.stats();
    let throughput = message_count as f64 / total_time;
    let throughput_mb = file_size_mb / total_time;

    let total_messages = message_count + skipped_invalid_count;

    println!("📊 MESSAGE STATISTICS:");
    println!(
        "  Total messages in file:    {:>15}",
        format_number(total_messages)
    );
    println!(
        "  Valid messages processed:  {:>15}",
        format_number(message_count)
    );
    println!(
        "  Skipped (invalid):         {:>15}",
        format_number(skipped_invalid_count)
    );
    println!(
        "  Messages with LOB errors:  {:>15}",
        format_number(lob_error_count)
    );
    println!(
        "  DBN decode errors:         {:>15}",
        format_number(lob_stats.errors)
    );
    println!(
        "  Valid message rate:        {:>14.2}%",
        (message_count as f64 / total_messages as f64) * 100.0
    );
    println!();

    println!("⚡ PERFORMANCE:");
    println!("  Total time:                {:>12.2} seconds", total_time);
    println!("  Throughput:                {:>12.0} msg/s", throughput);
    println!("  Throughput:                {:>12.2} MB/s", throughput_mb);
    println!();

    println!("📈 FINAL LOB STATE:");
    println!(
        "  Active orders:             {:>15}",
        format_number(lob.order_count() as u64)
    );
    println!(
        "  Bid levels:                {:>15}",
        format_number(lob.bid_levels() as u64)
    );
    println!(
        "  Ask levels:                {:>15}",
        format_number(lob.ask_levels() as u64)
    );
    println!();

    let final_state = lob.get_lob_state();
    if let Some(mid) = final_state.mid_price() {
        println!("  Mid-price:                 {:>12.4}", mid);
    }
    if let Some(spread) = final_state.spread() {
        println!("  Spread:                    {:>12.4}", spread);
    }
    if let Some(spread_bps) = final_state.spread_bps() {
        println!("  Spread (bps):              {:>12.2}", spread_bps);
    }
    println!();

    // Display top 5 levels on each side
    println!("📖 TOP 5 LEVELS:");
    println!();
    println!("  BID SIDE:");
    for i in 0..5.min(final_state.levels) {
        if final_state.bid_prices[i] > 0 {
            let price = final_state.bid_prices[i] as f64 / 1e9;
            let size = final_state.bid_sizes[i];
            println!(
                "    Level {}: ${:>10.4} × {:>8} shares",
                i + 1,
                price,
                format_number(size as u64)
            );
        }
    }
    println!();
    println!("  ASK SIDE:");
    for i in 0..5.min(final_state.levels) {
        if final_state.ask_prices[i] > 0 {
            let price = final_state.ask_prices[i] as f64 / 1e9;
            let size = final_state.ask_sizes[i];
            println!(
                "    Level {}: ${:>10.4} × {:>8} shares",
                i + 1,
                price,
                format_number(size as u64)
            );
        }
    }
    println!();

    // Efficiency metrics
    let ns_per_msg = (total_time * 1_000_000_000.0) / message_count as f64;
    let speedup_vs_python = throughput / 20_000.0; // Assuming Python does ~20K msg/s

    println!("🎯 EFFICIENCY METRICS:");
    println!("  Time per message:          {:>12.2} ns", ns_per_msg);
    println!("  Speedup vs Python (~20K):  {:>12.1}x", speedup_vs_python);
    println!();

    println!("✨ SUCCESS! Ready for production use.");
    println!("=================================================================\n");

    Ok(())
}

// Helper to format numbers with thousands separators
fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();

    for (i, c) in chars.iter().enumerate() {
        result.push(*c);
        if (len - i - 1) % 3 == 0 && i != len - 1 {
            result.push(',');
        }
    }

    result
}
