//! Basic usage example for TLOB Rust core.
//!
//! Run with: cargo run --example basic_usage

use mbo_lob_reconstructor::{Action, LobReconstructor, MboMessage, Side};

fn main() {
    println!("=================================================================");
    println!("TLOB Rust Core - Basic Usage Example");
    println!("=================================================================\n");

    // Create LOB reconstructor with 10 levels
    let mut lob = LobReconstructor::new(10);
    println!("✓ Created LOB reconstructor (10 levels)\n");

    // Simulate order book events
    println!("Processing order book events...\n");

    // Event 1: Add bid order at $100.00
    let bid1 = MboMessage::new(
        1001,            // order_id
        Action::Add,     // action
        Side::Bid,       // side
        100_000_000_000, // price ($100.00 in fixed-point)
        100,             // size
    );

    let state = lob.process_message(&bid1).unwrap();
    println!("Event 1: Add BID order");
    println!("  Order ID: 1001");
    println!("  Price: $100.00");
    println!("  Size: 100 shares");
    println!("  Best bid: ${:.2}", state.best_bid.unwrap() as f64 / 1e9);
    println!();

    // Event 2: Add ask order at $100.01
    let ask1 = MboMessage::new(
        2001,
        Action::Add,
        Side::Ask,
        100_010_000_000, // $100.01
        200,
    );

    let state = lob.process_message(&ask1).unwrap();
    println!("Event 2: Add ASK order");
    println!("  Order ID: 2001");
    println!("  Price: $100.01");
    println!("  Size: 200 shares");
    println!("  Best ask: ${:.2}", state.best_ask.unwrap() as f64 / 1e9);
    println!();

    // Event 3: Add another bid at $99.99
    let bid2 = MboMessage::new(
        1002,
        Action::Add,
        Side::Bid,
        99_990_000_000, // $99.99
        150,
    );

    let state = lob.process_message(&bid2).unwrap();
    println!("Event 3: Add BID order at lower price");
    println!("  Order ID: 1002");
    println!("  Price: $99.99");
    println!("  Size: 150 shares");
    println!(
        "  Best bid still: ${:.2}",
        state.best_bid.unwrap() as f64 / 1e9
    );
    println!();

    // Event 4: Trade (partial fill) on bid1
    let trade = MboMessage::new(
        1001,
        Action::Trade,
        Side::Bid,
        100_000_000_000,
        50, // Partial fill: 50 of 100
    );

    let state = lob.process_message(&trade).unwrap();
    println!("Event 4: TRADE (partial fill)");
    println!("  Order ID: 1001");
    println!("  Filled: 50 shares");
    println!("  Remaining at $100.00: {} shares", state.bid_sizes[0]);
    println!();

    // Event 5: Cancel bid2
    let cancel = MboMessage::new(1002, Action::Cancel, Side::Bid, 99_990_000_000, 150);

    let state = lob.process_message(&cancel).unwrap();
    println!("Event 5: CANCEL order");
    println!("  Order ID: 1002");
    println!("  Cancelled: 150 shares at $99.99");
    println!();

    // Display final LOB state
    println!("=================================================================");
    println!("Final LOB State");
    println!("=================================================================\n");

    if let Some(mid) = state.mid_price() {
        println!("Mid-price: ${:.4}", mid);
    }

    if let Some(spread) = state.spread() {
        println!("Spread: ${:.4}", spread);
    }

    if let Some(spread_bps) = state.spread_bps() {
        println!("Spread (bps): {:.2}", spread_bps);
    }

    println!("\nBid Side:");
    for i in 0..state.levels {
        if state.bid_prices[i] > 0 {
            let price = state.bid_prices[i] as f64 / 1e9;
            let size = state.bid_sizes[i];
            println!("  Level {}: ${:.2} x {} shares", i + 1, price, size);
        }
    }

    println!("\nAsk Side:");
    for i in 0..state.levels {
        if state.ask_prices[i] > 0 {
            let price = state.ask_prices[i] as f64 / 1e9;
            let size = state.ask_sizes[i];
            println!("  Level {}: ${:.2} x {} shares", i + 1, price, size);
        }
    }

    // Display statistics
    println!("\nStatistics:");
    let stats = lob.stats();
    println!("  Messages processed: {}", stats.messages_processed);
    println!("  Active orders: {}", stats.active_orders);
    println!("  Bid levels: {}", stats.bid_levels);
    println!("  Ask levels: {}", stats.ask_levels);

    println!("\n✓ Example complete!");
}
