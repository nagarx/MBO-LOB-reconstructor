//! Multi-symbol processing example.
//!
//! Demonstrates processing multiple stocks simultaneously.
//!
//! Run with: cargo run --example multi_symbol

use mbo_lob_reconstructor::{Action, MboMessage, MultiSymbolLob, Side};

fn main() {
    println!("=================================================================");
    println!("TLOB Rust Core - Multi-Symbol Example");
    println!("=================================================================\n");

    // Create multi-symbol manager
    let mut multi_lob = MultiSymbolLob::new(10);
    println!("✓ Created multi-symbol LOB manager (10 levels per symbol)\n");

    // Add symbols
    println!("Adding symbols...");
    multi_lob.add_symbol("NVDA").unwrap();
    multi_lob.add_symbol("TSLA").unwrap();
    multi_lob.add_symbol("AAPL").unwrap();
    println!("✓ Added: NVDA, TSLA, AAPL\n");

    // Process NVDA orders
    println!("Processing NVDA orders...");
    let nvda_bid = MboMessage::new(1001, Action::Add, Side::Bid, 140_000_000_000, 100);
    let nvda_ask = MboMessage::new(1002, Action::Add, Side::Ask, 140_010_000_000, 150);

    multi_lob.process_message("NVDA", &nvda_bid).unwrap();
    multi_lob.process_message("NVDA", &nvda_ask).unwrap();
    println!("  ✓ Added bid at $140.00 (100 shares)");
    println!("  ✓ Added ask at $140.01 (150 shares)\n");

    // Process TSLA orders
    println!("Processing TSLA orders...");
    let tsla_bid = MboMessage::new(2001, Action::Add, Side::Bid, 250_000_000_000, 50);
    let tsla_ask = MboMessage::new(2002, Action::Add, Side::Ask, 250_020_000_000, 75);

    multi_lob.process_message("TSLA", &tsla_bid).unwrap();
    multi_lob.process_message("TSLA", &tsla_ask).unwrap();
    println!("  ✓ Added bid at $250.00 (50 shares)");
    println!("  ✓ Added ask at $250.02 (75 shares)\n");

    // Process AAPL orders
    println!("Processing AAPL orders...");
    let aapl_bid = MboMessage::new(3001, Action::Add, Side::Bid, 180_000_000_000, 200);
    let aapl_ask = MboMessage::new(3002, Action::Add, Side::Ask, 180_010_000_000, 250);

    multi_lob.process_message("AAPL", &aapl_bid).unwrap();
    multi_lob.process_message("AAPL", &aapl_ask).unwrap();
    println!("  ✓ Added bid at $180.00 (200 shares)");
    println!("  ✓ Added ask at $180.01 (250 shares)\n");

    // Display states for all symbols
    println!("=================================================================");
    println!("Current States");
    println!("=================================================================\n");

    for symbol in multi_lob.symbols() {
        let state = multi_lob.get_state(symbol).unwrap();
        let symbol_stats = multi_lob.symbol_stats(symbol).unwrap();

        println!("{} LOB:", symbol);

        if let Some(mid) = state.mid_price() {
            println!("  Mid-price: ${:.4}", mid);
        }

        if let Some(spread) = state.spread() {
            println!("  Spread: ${:.4}", spread);
        }

        if let Some(spread_bps) = state.spread_bps() {
            println!("  Spread (bps): {:.2}", spread_bps);
        }

        println!("  Best bid: ${:.2}", state.best_bid.unwrap() as f64 / 1e9);
        println!("  Best ask: ${:.2}", state.best_ask.unwrap() as f64 / 1e9);
        println!("  Messages processed: {}", symbol_stats.messages_processed);
        println!();
    }

    // Display global statistics
    println!("=================================================================");
    println!("Global Statistics");
    println!("=================================================================\n");

    let stats = multi_lob.stats();
    println!("Total symbols: {}", stats.symbol_count);
    println!("Total messages: {}", stats.total_messages);
    println!("\nMessages per symbol:");

    let mut symbols: Vec<_> = stats.messages_per_symbol.iter().collect();
    symbols.sort_by_key(|(sym, _)| *sym);

    for (symbol, count) in symbols {
        println!("  {}: {}", symbol, count);
    }

    println!("\n✓ Multi-symbol example complete!");
}
