//! Advanced Analytics Example for LOB Reconstructor.
//!
//! This example demonstrates all the new analytics features:
//! - Book consistency validation
//! - Enriched LOB analytics (microprice, VWAP, depth imbalance)
//! - DayStats for normalization tracking
//! - DepthStats for per-side analysis
//! - MarketImpact simulation
//! - LiquidityMetrics for comprehensive analysis
//!
//! Run with: cargo run --example advanced_analytics

use mbo_lob_reconstructor::{
    Action, CrossedQuotePolicy, DayStats, DepthStats, LiquidityMetrics, LobConfig,
    LobReconstructor, LobState, MarketImpact, MboMessage, NormalizationParams, Side,
};

fn main() {
    println!("═══════════════════════════════════════════════════════════════");
    println!("  LOB Reconstructor - Advanced Analytics Example");
    println!("═══════════════════════════════════════════════════════════════\n");

    // Create a realistic order book
    let state = create_sample_lob();

    // 1. Basic Enriched Analytics
    demo_enriched_analytics(&state);

    // 2. Book Consistency
    demo_book_consistency();

    // 3. DepthStats
    demo_depth_stats(&state);

    // 4. MarketImpact
    demo_market_impact(&state);

    // 5. LiquidityMetrics
    demo_liquidity_metrics(&state);

    // 6. DayStats and Normalization
    demo_day_stats();

    println!("═══════════════════════════════════════════════════════════════");
    println!("  ✅ All examples completed successfully!");
    println!("═══════════════════════════════════════════════════════════════\n");
}

/// Create a sample LOB with realistic data
fn create_sample_lob() -> LobState {
    let mut lob = LobReconstructor::new(10);

    // Add bid orders (highest to lowest)
    let bids = [
        (1001, 100.00, 500), // Best bid
        (1002, 99.99, 300),
        (1003, 99.98, 800),
        (1004, 99.97, 200),
        (1005, 99.96, 400),
    ];

    for (id, price, size) in bids {
        let msg = MboMessage::new(id, Action::Add, Side::Bid, (price * 1e9) as i64, size);
        lob.process_message(&msg).unwrap();
    }

    // Add ask orders (lowest to highest)
    let asks = [
        (2001, 100.01, 400), // Best ask
        (2002, 100.02, 600),
        (2003, 100.03, 350),
        (2004, 100.04, 500),
        (2005, 100.05, 250),
    ];

    for (id, price, size) in asks {
        let msg = MboMessage::new(id, Action::Add, Side::Ask, (price * 1e9) as i64, size);
        lob.process_message(&msg).unwrap();
    }

    lob.get_lob_state()
}

/// Demonstrate enriched LOB analytics
fn demo_enriched_analytics(state: &LobState) {
    println!("┌─────────────────────────────────────────────────────────────┐");
    println!("│  1. ENRICHED LOB ANALYTICS                                  │");
    println!("└─────────────────────────────────────────────────────────────┘\n");

    // Basic metrics
    println!("  📊 Basic Metrics:");
    if let Some(mid) = state.mid_price() {
        println!("     Mid-price:        ${mid:.4}");
    }
    if let Some(spread) = state.spread() {
        println!("     Spread:           ${spread:.4}");
    }
    if let Some(spread_bps) = state.spread_bps() {
        println!("     Spread (bps):     {spread_bps:.2}");
    }

    // Enriched metrics
    println!("\n  📈 Enriched Metrics:");
    if let Some(microprice) = state.microprice() {
        println!("     Microprice:       ${microprice:.4}");
        if let Some(mid) = state.mid_price() {
            let diff = (microprice - mid) * 10000.0 / mid;
            println!("     vs Mid-price:     {diff:+.2} bps");
        }
    }

    // VWAP for top 3 levels
    if let Some(vwap_bid) = state.vwap_bid(3) {
        println!("     VWAP Bid (3 lvl): ${vwap_bid:.4}");
    }
    if let Some(vwap_ask) = state.vwap_ask(3) {
        println!("     VWAP Ask (3 lvl): ${vwap_ask:.4}");
    }
    if let Some(wmid) = state.weighted_mid(3) {
        println!("     Weighted Mid:     ${wmid:.4}");
    }

    // Volume metrics
    println!("\n  📦 Volume Metrics:");
    println!("     Total Bid Volume: {}", state.total_bid_volume());
    println!("     Total Ask Volume: {}", state.total_ask_volume());
    if let Some(imbalance) = state.depth_imbalance() {
        let direction = if imbalance > 0.0 {
            "BUY pressure"
        } else {
            "SELL pressure"
        };
        println!("     Depth Imbalance:  {imbalance:.4} ({direction})");
    }

    // Active levels
    println!("\n  📊 Active Levels:");
    println!("     Bid levels:       {}", state.active_bid_levels());
    println!("     Ask levels:       {}", state.active_ask_levels());

    println!();
}

/// Demonstrate book consistency validation
fn demo_book_consistency() {
    println!("┌─────────────────────────────────────────────────────────────┐");
    println!("│  2. BOOK CONSISTENCY VALIDATION                             │");
    println!("└─────────────────────────────────────────────────────────────┘\n");

    // Valid book
    let mut state = LobState::new(5);
    state.best_bid = Some(100_000_000_000);
    state.best_ask = Some(100_010_000_000);

    println!("  📗 Valid Book (bid < ask):");
    println!("     Best Bid: $100.00");
    println!("     Best Ask: $100.01");
    println!("     Consistency: {:?}", state.check_consistency());
    println!("     is_consistent(): {}", state.is_consistent());

    // Locked book
    state.best_ask = Some(100_000_000_000);
    println!("\n  📙 Locked Book (bid == ask):");
    println!("     Best Bid: $100.00");
    println!("     Best Ask: $100.00");
    println!("     Consistency: {:?}", state.check_consistency());
    println!("     is_locked(): {}", state.is_locked());

    // Crossed book
    state.best_ask = Some(99_990_000_000);
    println!("\n  📕 Crossed Book (bid > ask):");
    println!("     Best Bid: $100.00");
    println!("     Best Ask: $99.99");
    println!("     Consistency: {:?}", state.check_consistency());
    println!("     is_crossed(): {}", state.is_crossed());

    // Using crossed quote policy
    println!("\n  🔧 Crossed Quote Policies:");
    println!("     Allow:        Return crossed state as-is");
    println!("     UseLastValid: Return last valid state");
    println!("     Error:        Return error on crossed quote");
    println!("     SkipUpdate:   Skip updates that would cross");

    // Example with policy
    let config = LobConfig::new(10)
        .with_crossed_quote_policy(CrossedQuotePolicy::UseLastValid)
        .with_logging(false);
    let _lob = LobReconstructor::with_config(config);
    println!("\n     Example: LobConfig::new(10)");
    println!("                 .with_crossed_quote_policy(CrossedQuotePolicy::UseLastValid)");

    println!();
}

/// Demonstrate DepthStats
fn demo_depth_stats(state: &LobState) {
    println!("┌─────────────────────────────────────────────────────────────┐");
    println!("│  3. DEPTH STATISTICS (DepthStats)                           │");
    println!("└─────────────────────────────────────────────────────────────┘\n");

    let bid_stats = DepthStats::from_lob_state(state, Side::Bid);
    let ask_stats = DepthStats::from_lob_state(state, Side::Ask);

    println!("  📊 Bid Side Statistics:");
    println!("     Total Volume:     {}", bid_stats.total_volume);
    println!("     Active Levels:    {}", bid_stats.levels_count);
    println!("     Avg Level Size:   {:.1}", bid_stats.avg_level_size);
    println!("     Min Level Size:   {}", bid_stats.min_level_size);
    println!("     Max Level Size:   {}", bid_stats.max_level_size);
    println!("     Std Dev:          {:.1}", bid_stats.std_dev_level_size);
    println!(
        "     VWAP:             ${:.4}",
        bid_stats.weighted_avg_price
    );
    if let (Some(best), Some(worst)) = (bid_stats.best_price, bid_stats.worst_price) {
        println!("     Best Price:       ${best:.4}");
        println!("     Worst Price:      ${worst:.4}");
    }
    println!("     Price Range:      ${:.4}", bid_stats.price_range);
    println!(
        "     Concentration:    {:.2}%",
        bid_stats.concentration_ratio * 100.0
    );

    println!("\n  📊 Ask Side Statistics:");
    println!("     Total Volume:     {}", ask_stats.total_volume);
    println!("     Active Levels:    {}", ask_stats.levels_count);
    println!("     Avg Level Size:   {:.1}", ask_stats.avg_level_size);
    println!(
        "     VWAP:             ${:.4}",
        ask_stats.weighted_avg_price
    );
    println!(
        "     Concentration:    {:.2}%",
        ask_stats.concentration_ratio * 100.0
    );

    println!();
}

/// Demonstrate MarketImpact simulation
fn demo_market_impact(state: &LobState) {
    println!("┌─────────────────────────────────────────────────────────────┐");
    println!("│  4. MARKET IMPACT SIMULATION                                │");
    println!("└─────────────────────────────────────────────────────────────┘\n");

    // Small order - should fill at best price
    let small_buy = MarketImpact::simulate_buy(state, 100);
    println!("  📈 Small Buy Order (100 shares):");
    println!("     Best Price:       ${:.4}", small_buy.best_price);
    println!("     Avg Price:        ${:.4}", small_buy.avg_price);
    println!(
        "     Slippage:         ${:.6} ({:.2} bps)",
        small_buy.slippage, small_buy.slippage_bps
    );
    println!("     Levels Consumed:  {}", small_buy.levels_consumed);
    println!(
        "     Filled:           {} / {}",
        small_buy.filled_quantity, small_buy.requested_quantity
    );
    println!("     Can Fill:         {}", small_buy.can_fill());

    // Medium order - crosses multiple levels
    let medium_buy = MarketImpact::simulate_buy(state, 800);
    println!("\n  📈 Medium Buy Order (800 shares):");
    println!("     Best Price:       ${:.4}", medium_buy.best_price);
    println!("     Avg Price:        ${:.4}", medium_buy.avg_price);
    println!("     Worst Price:      ${:.4}", medium_buy.worst_price);
    println!(
        "     Slippage:         ${:.6} ({:.2} bps)",
        medium_buy.slippage, medium_buy.slippage_bps
    );
    println!("     Levels Consumed:  {}", medium_buy.levels_consumed);
    println!("     Total Cost:       ${:.2}", medium_buy.total_cost());
    println!("     Fills:");
    for (price, qty) in &medium_buy.fills {
        println!("       {qty} @ ${price:.4}");
    }

    // Large order - exceeds available liquidity
    let large_buy = MarketImpact::simulate_buy(state, 3000);
    println!("\n  📈 Large Buy Order (3000 shares):");
    println!(
        "     Available:        {}",
        large_buy.total_quantity_available
    );
    println!("     Filled:           {}", large_buy.filled_quantity);
    println!("     Unfilled:         {}", large_buy.unfilled_quantity);
    println!(
        "     Fill Ratio:       {:.1}%",
        large_buy.fill_ratio() * 100.0
    );
    println!("     Can Fill:         {} ⚠️", large_buy.can_fill());

    // Sell order
    let sell = MarketImpact::simulate_sell(state, 500);
    println!("\n  📉 Sell Order (500 shares):");
    println!("     Best Price:       ${:.4}", sell.best_price);
    println!("     Avg Price:        ${:.4}", sell.avg_price);
    println!(
        "     Slippage:         ${:.6} ({:.2} bps)",
        sell.slippage, sell.slippage_bps
    );

    println!();
}

/// Demonstrate LiquidityMetrics
fn demo_liquidity_metrics(state: &LobState) {
    println!("┌─────────────────────────────────────────────────────────────┐");
    println!("│  5. LIQUIDITY METRICS                                       │");
    println!("└─────────────────────────────────────────────────────────────┘\n");

    let metrics = LiquidityMetrics::from_lob_state(state);

    println!("  📊 Market Overview:");
    println!("     Is Liquid:        {}", metrics.is_liquid());
    println!("     Mid-price:        ${:.4}", metrics.mid_price);
    println!("     Microprice:       ${:.4}", metrics.microprice);
    println!(
        "     Spread:           ${:.4} ({:.2} bps)",
        metrics.spread, metrics.spread_bps
    );

    println!("\n  📦 Volume Analysis:");
    println!("     Total Volume:     {}", metrics.total_volume);
    println!("     Bid Volume:       {}", metrics.bid_depth.total_volume);
    println!("     Ask Volume:       {}", metrics.ask_depth.total_volume);
    println!("     Volume Imbalance: {:.4}", metrics.volume_imbalance);

    let pressure = metrics.book_pressure();
    let pressure_str = if pressure > 0.1 {
        "Strong BUY pressure 📈"
    } else if pressure < -0.1 {
        "Strong SELL pressure 📉"
    } else {
        "Balanced ⚖️"
    };
    println!("     Book Pressure:    {pressure} ({pressure_str})");

    println!("\n  📊 Depth Analysis:");
    println!("     Total Levels:     {}", metrics.total_levels);
    println!("     Avg Depth/Level:  {:.1}", metrics.avg_depth_per_level);

    println!();
}

/// Demonstrate DayStats and NormalizationParams
fn demo_day_stats() {
    println!("┌─────────────────────────────────────────────────────────────┐");
    println!("│  6. DAY STATISTICS & NORMALIZATION                          │");
    println!("└─────────────────────────────────────────────────────────────┘\n");

    let mut day_stats = DayStats::new("2025-02-03");
    let mut lob = LobReconstructor::new(10);

    // Simulate some market activity
    let events = [
        // Morning: tight spreads
        (1001, Action::Add, Side::Bid, 100.00, 500),
        (2001, Action::Add, Side::Ask, 100.01, 400),
        // Trade
        (1001, Action::Trade, Side::Bid, 100.00, 100),
        // Add more depth
        (1002, Action::Add, Side::Bid, 99.99, 300),
        (2002, Action::Add, Side::Ask, 100.02, 600),
        // Price move
        (1003, Action::Add, Side::Bid, 100.01, 200),
        (2003, Action::Add, Side::Ask, 100.03, 350),
    ];

    for (id, action, side, price, size) in events {
        let msg = MboMessage::new(id, action, side, (price * 1e9) as i64, size);
        if let Ok(state) = lob.process_message(&msg) {
            day_stats.update(&state);
        }
    }

    println!("  📊 DayStats Summary:");
    println!("     Date:             {}", day_stats.date);
    println!("     Total Messages:   {}", day_stats.total_messages);
    println!("     Valid Snapshots:  {}", day_stats.valid_snapshots);
    println!("     Empty Snapshots:  {}", day_stats.empty_snapshots);
    println!("     Crossed Quotes:   {}", day_stats.crossed_quotes);
    println!(
        "     Data Quality:     {:.2}%",
        day_stats.data_quality_ratio() * 100.0
    );

    println!("\n  📈 Price Statistics:");
    println!("     Mid-price Mean:   ${:.4}", day_stats.mid_price.mean);
    println!("     Mid-price Std:    ${:.4}", day_stats.mid_price.std());
    if day_stats.mid_price.min != f64::INFINITY {
        println!(
            "     Mid-price Range:  ${:.4} - ${:.4}",
            day_stats.mid_price.min, day_stats.mid_price.max
        );
    }

    println!("\n  📊 Spread Statistics:");
    println!("     Spread Mean:      ${:.6}", day_stats.spread.mean);
    println!("     Spread (bps):     {:.2}", day_stats.spread_bps.mean);

    // Create normalization parameters
    let norm_params = NormalizationParams::from_day_stats(&day_stats, 10);

    println!("\n  🔧 Normalization Parameters:");
    println!("     Feature Count:    {}", norm_params.means.len());
    println!("     Sample Count:     {}", norm_params.sample_count);
    println!("     Source:           {}", norm_params.source);

    // Show sample normalization
    let test_price = 100.0;
    let normalized = norm_params.normalize(test_price, 0);
    let denormalized = norm_params.denormalize(normalized, 0);

    println!("\n  📐 Normalization Example:");
    println!("     Original:         ${test_price:.4}");
    println!("     Normalized:       {normalized:.4} (z-score)");
    println!("     Denormalized:     ${denormalized:.4}");

    println!();
}
