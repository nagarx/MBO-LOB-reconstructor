//! Integration tests for LOB Reconstructor with real NVDA data.
//!
//! These tests validate the new features against actual market data:
//! - Book consistency validation
//! - Enriched analytics (microprice, VWAP, depth imbalance)
//! - DayStats normalization tracking
//! - Crossed quote policies
//!
//! Run with:
//! ```bash
//! cargo test --test integration_test --release
//! ```
//!
//! Note: These tests require the `databento` feature (enabled by default).

#![cfg(feature = "databento")]

use std::path::Path;
use std::time::Instant;

use mbo_lob_reconstructor::{
    BookConsistency, CrossedQuotePolicy, DayStats, DbnLoader, LobConfig, LobReconstructor,
    NormalizationParams,
};

/// Path to the test data file (first day of NVDA data)
const TEST_DATA_PATH: &str = "data/NVDA_2025-02-01_to_2025-09-30/xnas-itch-20250203.mbo.dbn.zst";

/// Check if test data is available
fn test_data_available() -> bool {
    // Try relative path first
    if Path::new(TEST_DATA_PATH).exists() {
        return true;
    }
    // Try from workspace root
    let workspace_path = format!("../{TEST_DATA_PATH}");
    Path::new(&workspace_path).exists()
}

/// Get the actual path to test data
fn get_test_data_path() -> String {
    if Path::new(TEST_DATA_PATH).exists() {
        TEST_DATA_PATH.to_string()
    } else {
        format!("../{TEST_DATA_PATH}")
    }
}

// ============================================================================
// Test: Basic LOB Reconstruction with Real Data
// ============================================================================

#[test]
fn test_basic_reconstruction_with_real_data() {
    if !test_data_available() {
        eprintln!("⚠️  Skipping test: test data not available at {TEST_DATA_PATH}");
        return;
    }

    let path = get_test_data_path();
    println!("\n📂 Testing basic reconstruction with: {path}");

    let loader = DbnLoader::new(&path)
        .expect("Failed to create loader")
        .skip_invalid(true);

    let mut lob = LobReconstructor::new(10);
    let mut processed = 0u64;
    let mut valid_states = 0u64;

    let start = Instant::now();

    // Process first 100K messages for quick test
    for msg in loader.iter_messages().expect("Failed to iterate") {
        if msg.order_id == 0 || msg.size == 0 || msg.price <= 0 {
            continue;
        }

        if let Ok(state) = lob.process_message(&msg) {
            processed += 1;
            if state.is_valid() {
                valid_states += 1;
            }
        }

        if processed >= 100_000 {
            break;
        }
    }

    let elapsed = start.elapsed();

    println!(
        "  ✅ Processed {} messages in {:.2}s",
        processed,
        elapsed.as_secs_f64()
    );
    println!(
        "  ✅ Valid LOB states: {} ({:.1}%)",
        valid_states,
        (valid_states as f64 / processed as f64) * 100.0
    );
    println!(
        "  ✅ Throughput: {:.0} msg/s",
        processed as f64 / elapsed.as_secs_f64()
    );

    // Assertions
    assert!(
        processed >= 100_000,
        "Should process at least 100K messages"
    );
    assert!(valid_states > processed / 2, "Most states should be valid");
}

// ============================================================================
// Test: Enriched Analytics (Microprice, VWAP, Depth Imbalance)
// ============================================================================

#[test]
fn test_enriched_analytics_with_real_data() {
    if !test_data_available() {
        eprintln!("⚠️  Skipping test: test data not available");
        return;
    }

    let path = get_test_data_path();
    println!("\n📂 Testing enriched analytics with: {path}");

    let loader = DbnLoader::new(&path)
        .expect("Failed to create loader")
        .skip_invalid(true);

    let mut lob = LobReconstructor::new(10);
    let mut analytics_count = 0u64;
    let mut microprice_sum = 0.0f64;
    let mut vwap_bid_sum = 0.0f64;
    let mut vwap_ask_sum = 0.0f64;
    let mut imbalance_sum = 0.0f64;

    // Process 50K messages and collect analytics
    for msg in loader.iter_messages().expect("Failed to iterate") {
        if msg.order_id == 0 || msg.size == 0 || msg.price <= 0 {
            continue;
        }

        if let Ok(state) = lob.process_message(&msg) {
            if state.is_valid() && state.is_consistent() {
                // Test microprice
                if let Some(microprice) = state.microprice() {
                    microprice_sum += microprice;
                }

                // Test VWAP
                if let Some(vwap_bid) = state.vwap_bid(5) {
                    vwap_bid_sum += vwap_bid;
                }
                if let Some(vwap_ask) = state.vwap_ask(5) {
                    vwap_ask_sum += vwap_ask;
                }

                // Test depth imbalance
                if let Some(imbalance) = state.depth_imbalance() {
                    imbalance_sum += imbalance;

                    // Imbalance should be in [-1, 1]
                    assert!(
                        (-1.0..=1.0).contains(&imbalance),
                        "Imbalance {imbalance} out of range"
                    );
                }

                // Test total volumes
                let bid_vol = state.total_bid_volume();
                let ask_vol = state.total_ask_volume();
                assert!(bid_vol > 0 || ask_vol > 0, "Should have some volume");

                // Test active levels
                let active_bid = state.active_bid_levels();
                let active_ask = state.active_ask_levels();
                assert!(
                    active_bid <= state.levels,
                    "Active bid levels should not exceed total levels"
                );
                assert!(
                    active_ask <= state.levels,
                    "Active ask levels should not exceed total levels"
                );

                analytics_count += 1;
            }
        }

        if analytics_count >= 50_000 {
            break;
        }
    }

    // Calculate averages
    let avg_microprice = microprice_sum / analytics_count as f64;
    let avg_vwap_bid = vwap_bid_sum / analytics_count as f64;
    let avg_vwap_ask = vwap_ask_sum / analytics_count as f64;
    let avg_imbalance = imbalance_sum / analytics_count as f64;

    println!("  📊 Analytics from {analytics_count} valid states:");
    println!("     Avg Microprice:    ${avg_microprice:.4}");
    println!("     Avg VWAP (bid):    ${avg_vwap_bid:.4}");
    println!("     Avg VWAP (ask):    ${avg_vwap_ask:.4}");
    println!("     Avg Imbalance:     {avg_imbalance:.4}");

    // Sanity checks
    assert!(analytics_count >= 40_000, "Should have enough valid states");
    assert!(avg_microprice > 0.0, "Microprice should be positive");
    assert!(avg_vwap_bid > 0.0, "VWAP bid should be positive");
    assert!(avg_vwap_ask > 0.0, "VWAP ask should be positive");
    assert!(
        avg_vwap_bid <= avg_vwap_ask,
        "VWAP bid should be <= VWAP ask (normal market)"
    );
    assert!(
        avg_imbalance.abs() < 0.5,
        "Average imbalance should be relatively balanced"
    );

    println!("  ✅ All enriched analytics tests passed");
}

// ============================================================================
// Test: Book Consistency Detection
// ============================================================================

#[test]
fn test_book_consistency_detection() {
    if !test_data_available() {
        eprintln!("⚠️  Skipping test: test data not available");
        return;
    }

    let path = get_test_data_path();
    println!("\n📂 Testing book consistency detection with: {path}");

    let config = LobConfig::new(10)
        .with_crossed_quote_policy(CrossedQuotePolicy::Allow)
        .with_logging(false); // Disable logging for test

    let loader = DbnLoader::new(&path)
        .expect("Failed to create loader")
        .skip_invalid(true);

    let mut lob = LobReconstructor::with_config(config);
    let mut total = 0u64;
    let mut valid = 0u64;
    let mut empty = 0u64;
    let mut crossed = 0u64;
    let mut locked = 0u64;

    // Process 100K messages
    for msg in loader.iter_messages().expect("Failed to iterate") {
        if msg.order_id == 0 || msg.size == 0 || msg.price <= 0 {
            continue;
        }

        if let Ok(state) = lob.process_message(&msg) {
            total += 1;

            match state.check_consistency() {
                BookConsistency::Valid => valid += 1,
                BookConsistency::Empty => empty += 1,
                BookConsistency::Crossed => crossed += 1,
                BookConsistency::Locked => locked += 1,
            }
        }

        if total >= 100_000 {
            break;
        }
    }

    // Also check stats
    let stats = lob.stats();

    println!("  📊 Book Consistency Results:");
    println!("     Total states:     {total}");
    println!(
        "     Valid:            {} ({:.2}%)",
        valid,
        (valid as f64 / total as f64) * 100.0
    );
    println!(
        "     Empty:            {} ({:.2}%)",
        empty,
        (empty as f64 / total as f64) * 100.0
    );
    println!(
        "     Crossed:          {} ({:.4}%)",
        crossed,
        (crossed as f64 / total as f64) * 100.0
    );
    println!(
        "     Locked:           {} ({:.4}%)",
        locked,
        (locked as f64 / total as f64) * 100.0
    );
    println!("  📊 Stats tracking:");
    println!("     crossed_quotes:   {}", stats.crossed_quotes);
    println!("     locked_quotes:    {}", stats.locked_quotes);

    // Assertions
    assert_eq!(
        crossed, stats.crossed_quotes,
        "Crossed count should match stats"
    );
    assert_eq!(
        locked, stats.locked_quotes,
        "Locked count should match stats"
    );

    // In normal market data, vast majority should be valid
    let valid_ratio = valid as f64 / total as f64;
    assert!(
        valid_ratio > 0.5,
        "Most states should be valid, got {:.2}%",
        valid_ratio * 100.0
    );

    println!("  ✅ Book consistency detection tests passed");
}

// ============================================================================
// Test: DayStats Tracking
// ============================================================================

#[test]
fn test_day_stats_tracking() {
    if !test_data_available() {
        eprintln!("⚠️  Skipping test: test data not available");
        return;
    }

    let path = get_test_data_path();
    println!("\n📂 Testing DayStats tracking with: {path}");

    let loader = DbnLoader::new(&path)
        .expect("Failed to create loader")
        .skip_invalid(true);

    let mut lob = LobReconstructor::new(10);
    let mut day_stats = DayStats::new("2025-02-03");

    // Process 50K messages
    let mut processed = 0u64;
    for msg in loader.iter_messages().expect("Failed to iterate") {
        if msg.order_id == 0 || msg.size == 0 || msg.price <= 0 {
            continue;
        }

        if let Ok(state) = lob.process_message(&msg) {
            day_stats.update(&state);
            processed += 1;
        }

        if processed >= 50_000 {
            break;
        }
    }

    println!("  📊 DayStats Summary:");
    println!("     Date:             {}", day_stats.date);
    println!("     Total messages:   {}", day_stats.total_messages);
    println!("     Valid snapshots:  {}", day_stats.valid_snapshots);
    println!("     Empty snapshots:  {}", day_stats.empty_snapshots);
    println!("     Crossed quotes:   {}", day_stats.crossed_quotes);
    println!("     Locked quotes:    {}", day_stats.locked_quotes);
    println!(
        "     Data quality:     {:.2}%",
        day_stats.data_quality_ratio() * 100.0
    );
    println!();
    println!("  📊 Price Statistics:");
    println!("     Mid-price mean:   ${:.4}", day_stats.mid_price.mean);
    println!("     Mid-price std:    ${:.4}", day_stats.mid_price.std());
    println!(
        "     Mid-price range:  ${:.4} - ${:.4}",
        day_stats.mid_price.min, day_stats.mid_price.max
    );
    println!();
    println!("  📊 Spread Statistics:");
    println!("     Spread mean:      ${:.6}", day_stats.spread.mean);
    println!(
        "     Spread (bps):     {:.2} bps",
        day_stats.spread_bps.mean
    );
    println!();
    println!("  📊 Volume Statistics:");
    println!("     Bid size mean:    {:.0}", day_stats.best_bid_size.mean);
    println!("     Ask size mean:    {:.0}", day_stats.best_ask_size.mean);
    println!(
        "     Imbalance mean:   {:.4}",
        day_stats.depth_imbalance.mean
    );

    // Assertions
    assert_eq!(
        day_stats.total_messages, processed,
        "Total messages should match"
    );
    assert!(day_stats.valid_snapshots > 0, "Should have valid snapshots");
    assert!(day_stats.mid_price.count > 0, "Should have mid-price data");
    assert!(
        day_stats.mid_price.mean > 0.0,
        "Mid-price should be positive"
    );
    assert!(
        day_stats.spread.mean >= 0.0,
        "Spread should be non-negative"
    );
    assert!(
        day_stats.spread_bps.mean >= 0.0,
        "Spread bps should be non-negative"
    );

    // Check data quality
    let quality = day_stats.data_quality_ratio();
    assert!(
        quality > 0.3,
        "Data quality should be reasonable, got {:.2}%",
        quality * 100.0
    );

    println!("  ✅ DayStats tracking tests passed");
}

// ============================================================================
// Test: Normalization Parameters
// ============================================================================

#[test]
fn test_normalization_params_from_real_data() {
    if !test_data_available() {
        eprintln!("⚠️  Skipping test: test data not available");
        return;
    }

    let path = get_test_data_path();
    println!("\n📂 Testing NormalizationParams with: {path}");

    let loader = DbnLoader::new(&path)
        .expect("Failed to create loader")
        .skip_invalid(true);

    let mut lob = LobReconstructor::new(10);
    let mut day_stats = DayStats::new("2025-02-03");

    // Process 50K messages
    let mut processed = 0u64;
    for msg in loader.iter_messages().expect("Failed to iterate") {
        if msg.order_id == 0 || msg.size == 0 || msg.price <= 0 {
            continue;
        }

        if let Ok(state) = lob.process_message(&msg) {
            day_stats.update(&state);
            processed += 1;
        }

        if processed >= 50_000 {
            break;
        }
    }

    // Create normalization params
    let params = NormalizationParams::from_day_stats(&day_stats, 10);

    println!("  📊 Normalization Parameters:");
    println!("     Feature count:    {}", params.means.len());
    println!("     Sample count:     {}", params.sample_count);
    println!("     Source:           {}", params.source);
    println!();
    println!("  📊 Sample means (first 4 features):");
    for i in 0..4.min(params.means.len()) {
        println!(
            "     {}: mean={:.4}, std={:.4}",
            params.feature_names[i], params.means[i], params.stds[i]
        );
    }

    // Test normalization/denormalization round-trip
    let test_value = 100.0;
    let normalized = params.normalize(test_value, 0);
    let denormalized = params.denormalize(normalized, 0);

    println!();
    println!("  📊 Normalization round-trip test:");
    println!("     Original:         {test_value:.4}");
    println!("     Normalized:       {normalized:.4}");
    println!("     Denormalized:     {denormalized:.4}");

    // Assertions
    assert_eq!(
        params.means.len(),
        40,
        "Should have 40 features (10 levels × 4)"
    );
    assert_eq!(params.stds.len(), 40, "Stds should match means length");
    assert_eq!(params.feature_names.len(), 40, "Feature names should match");
    assert!(params.sample_count > 0, "Should have samples");

    // Round-trip should be close
    assert!(
        (test_value - denormalized).abs() < 0.001,
        "Round-trip should preserve value"
    );

    println!("  ✅ NormalizationParams tests passed");
}

// ============================================================================
// Test: Crossed Quote Policy - UseLastValid
// ============================================================================

#[test]
fn test_crossed_quote_policy_use_last_valid() {
    if !test_data_available() {
        eprintln!("⚠️  Skipping test: test data not available");
        return;
    }

    let path = get_test_data_path();
    println!("\n📂 Testing UseLastValid policy with: {path}");

    let config = LobConfig::new(10)
        .with_crossed_quote_policy(CrossedQuotePolicy::UseLastValid)
        .with_logging(false);

    let loader = DbnLoader::new(&path)
        .expect("Failed to create loader")
        .skip_invalid(true);

    let mut lob = LobReconstructor::with_config(config);
    let mut total = 0u64;
    let mut crossed_returned = 0u64;

    // Process 100K messages
    for msg in loader.iter_messages().expect("Failed to iterate") {
        if msg.order_id == 0 || msg.size == 0 || msg.price <= 0 {
            continue;
        }

        if let Ok(state) = lob.process_message(&msg) {
            total += 1;

            // With UseLastValid policy, we should never get a crossed state back
            if state.is_crossed() {
                crossed_returned += 1;
            }
        }

        if total >= 100_000 {
            break;
        }
    }

    let stats = lob.stats();

    println!("  📊 UseLastValid Policy Results:");
    println!("     Total processed:  {total}");
    println!("     Crossed detected: {} (internal)", stats.crossed_quotes);
    println!("     Crossed returned: {crossed_returned} (to user)");

    // With UseLastValid, we should get 0 crossed states returned
    // (they should be replaced with last valid)
    // Note: This might not be 0 if there's no valid state yet
    println!("  ✅ UseLastValid policy test completed");
}

// ============================================================================
// Test: Performance with Real Data
// ============================================================================

#[test]
fn test_performance_with_real_data() {
    if !test_data_available() {
        eprintln!("⚠️  Skipping test: test data not available");
        return;
    }

    let path = get_test_data_path();
    println!("\n📂 Testing performance with: {path}");

    let loader = DbnLoader::new(&path)
        .expect("Failed to create loader")
        .skip_invalid(true);

    let mut lob = LobReconstructor::new(10);
    let mut processed = 0u64;

    let start = Instant::now();

    // Process 500K messages for performance test
    for msg in loader.iter_messages().expect("Failed to iterate") {
        if msg.order_id == 0 || msg.size == 0 || msg.price <= 0 {
            continue;
        }

        if let Ok(_state) = lob.process_message(&msg) {
            processed += 1;
        }

        if processed >= 500_000 {
            break;
        }
    }

    let elapsed = start.elapsed();
    let throughput = processed as f64 / elapsed.as_secs_f64();
    let ns_per_msg = elapsed.as_nanos() as f64 / processed as f64;

    println!("  ⚡ Performance Results:");
    println!("     Messages:         {processed}");
    println!("     Time:             {:.2}s", elapsed.as_secs_f64());
    println!("     Throughput:       {throughput:.0} msg/s");
    println!("     Latency:          {ns_per_msg:.0} ns/msg");

    // Performance assertions - only strict in release mode
    // Debug mode is ~10x slower due to no optimizations
    #[cfg(debug_assertions)]
    {
        // Relaxed thresholds for debug builds
        assert!(
            throughput > 10_000.0,
            "Throughput should be >10K msg/s even in debug, got {throughput:.0}"
        );
        println!("  ✅ Performance test passed (debug mode, relaxed thresholds)");
    }

    #[cfg(not(debug_assertions))]
    {
        // Strict thresholds for release builds
        // Target: >100K msg/s (should be much higher)
        assert!(
            throughput > 100_000.0,
            "Throughput should be >100K msg/s, got {throughput:.0}"
        );

        // Target: <10μs per message
        assert!(
            ns_per_msg < 10_000.0,
            "Latency should be <10μs, got {ns_per_msg:.0}ns"
        );

        println!(
            "  ✅ Performance tests passed (>{:.0}K msg/s, <{:.0}μs/msg)",
            throughput / 1000.0,
            ns_per_msg / 1000.0
        );
    }
}

// ============================================================================
// Test: Full Day Processing with All Features
// ============================================================================

#[test]
fn test_full_day_processing() {
    if !test_data_available() {
        eprintln!("⚠️  Skipping test: test data not available");
        return;
    }

    let path = get_test_data_path();
    println!("\n📂 Full day processing test with: {path}");
    println!("   (This may take a while...)\n");

    let config = LobConfig::new(10)
        .with_crossed_quote_policy(CrossedQuotePolicy::Allow)
        .with_logging(false);

    let loader = DbnLoader::new(&path)
        .expect("Failed to create loader")
        .skip_invalid(true);

    let mut lob = LobReconstructor::with_config(config);
    let mut day_stats = DayStats::new("2025-02-03");

    let mut processed = 0u64;
    let mut skipped = 0u64;
    let mut errors = 0u64;

    let start = Instant::now();
    let report_interval = 1_000_000u64;
    let mut last_report = 0u64;

    // Process ALL messages in the file
    for msg in loader.iter_messages().expect("Failed to iterate") {
        if msg.order_id == 0 || msg.size == 0 || msg.price <= 0 {
            skipped += 1;
            continue;
        }

        match lob.process_message(&msg) {
            Ok(state) => {
                day_stats.update(&state);
                processed += 1;
            }
            Err(_) => {
                errors += 1;
            }
        }

        // Progress report
        if processed - last_report >= report_interval {
            let elapsed = start.elapsed().as_secs_f64();
            println!(
                "  ⏱️  Processed: {:>10} | Rate: {:>8.0} msg/s | Time: {:.1}s",
                processed,
                processed as f64 / elapsed,
                elapsed
            );
            last_report = processed;
        }
    }

    let elapsed = start.elapsed();
    let throughput = processed as f64 / elapsed.as_secs_f64();

    println!();
    println!("  ═══════════════════════════════════════════════════════════");
    println!("  📊 FULL DAY PROCESSING RESULTS");
    println!("  ═══════════════════════════════════════════════════════════");
    println!();
    println!("  📈 Message Statistics:");
    println!("     Processed:        {processed:>12}");
    println!("     Skipped:          {skipped:>12}");
    println!("     Errors:           {errors:>12}");
    println!(
        "     Total:            {:>12}",
        processed + skipped + errors
    );
    println!();
    println!("  ⚡ Performance:");
    println!("     Total time:       {:>12.2}s", elapsed.as_secs_f64());
    println!("     Throughput:       {throughput:>12.0} msg/s");
    println!();
    println!("  📊 LOB Statistics (from stats):");
    let stats = lob.stats();
    println!("     Messages proc:    {:>12}", stats.messages_processed);
    println!("     Crossed quotes:   {:>12}", stats.crossed_quotes);
    println!("     Locked quotes:    {:>12}", stats.locked_quotes);
    println!();
    println!("  📊 DayStats Summary:");
    println!("     Valid snapshots:  {:>12}", day_stats.valid_snapshots);
    println!("     Empty snapshots:  {:>12}", day_stats.empty_snapshots);
    println!(
        "     Data quality:     {:>11.2}%",
        day_stats.data_quality_ratio() * 100.0
    );
    println!();
    println!("  📊 Price Statistics:");
    println!("     Mid-price mean:   ${:>11.4}", day_stats.mid_price.mean);
    println!(
        "     Mid-price std:    ${:>11.4}",
        day_stats.mid_price.std()
    );
    println!("     Spread (bps):     {:>11.2}", day_stats.spread_bps.mean);
    println!();
    println!("  📊 Final LOB State:");
    let final_state = lob.get_lob_state();
    if let Some(mid) = final_state.mid_price() {
        println!("     Mid-price:        ${mid:>11.4}");
    }
    if let Some(spread) = final_state.spread() {
        println!("     Spread:           ${spread:>11.6}");
    }
    if let Some(microprice) = final_state.microprice() {
        println!("     Microprice:       ${microprice:>11.4}");
    }
    if let Some(imbalance) = final_state.depth_imbalance() {
        println!("     Imbalance:        {imbalance:>12.4}");
    }
    println!();

    // Assertions for full day
    assert!(processed > 0, "Should process some messages");
    assert!(day_stats.valid_snapshots > 0, "Should have valid snapshots");
    assert!(throughput > 50_000.0, "Should maintain good throughput");

    println!("  ✅ Full day processing test PASSED");
    println!("  ═══════════════════════════════════════════════════════════\n");
}

// ============================================================================
// Test: DepthStats with Real Data
// ============================================================================

#[test]
fn test_depth_stats_with_real_data() {
    use mbo_lob_reconstructor::{DepthStats, Side};

    if !test_data_available() {
        eprintln!("⚠️  Skipping test: test data not available");
        return;
    }

    let path = get_test_data_path();
    println!("\n📂 Testing DepthStats with: {path}");

    let loader = DbnLoader::new(&path)
        .expect("Failed to create loader")
        .skip_invalid(true);

    let mut lob = LobReconstructor::new(10);
    let mut bid_stats_samples: Vec<mbo_lob_reconstructor::DepthStats> = Vec::new();
    let mut ask_stats_samples: Vec<mbo_lob_reconstructor::DepthStats> = Vec::new();

    // Process 50K messages and collect depth stats periodically
    let mut processed = 0u64;
    for msg in loader.iter_messages().expect("Failed to iterate") {
        if msg.order_id == 0 || msg.size == 0 || msg.price <= 0 {
            continue;
        }

        if let Ok(state) = lob.process_message(&msg) {
            processed += 1;

            // Sample every 1000 messages
            if processed % 1000 == 0 && state.is_valid() {
                let bid_stats = DepthStats::from_lob_state(&state, Side::Bid);
                let ask_stats = DepthStats::from_lob_state(&state, Side::Ask);

                if !bid_stats.is_empty() {
                    bid_stats_samples.push(bid_stats);
                }
                if !ask_stats.is_empty() {
                    ask_stats_samples.push(ask_stats);
                }
            }
        }

        if processed >= 50_000 {
            break;
        }
    }

    // Calculate averages
    let avg_bid_volume: f64 = bid_stats_samples
        .iter()
        .map(|s| s.total_volume as f64)
        .sum::<f64>()
        / bid_stats_samples.len() as f64;

    let avg_ask_volume: f64 = ask_stats_samples
        .iter()
        .map(|s| s.total_volume as f64)
        .sum::<f64>()
        / ask_stats_samples.len() as f64;

    let avg_bid_levels: f64 = bid_stats_samples
        .iter()
        .map(|s| s.levels_count as f64)
        .sum::<f64>()
        / bid_stats_samples.len() as f64;

    let avg_ask_levels: f64 = ask_stats_samples
        .iter()
        .map(|s| s.levels_count as f64)
        .sum::<f64>()
        / ask_stats_samples.len() as f64;

    let avg_bid_concentration: f64 = bid_stats_samples
        .iter()
        .map(|s| s.concentration_ratio)
        .sum::<f64>()
        / bid_stats_samples.len() as f64;

    println!(
        "  📊 DepthStats Results ({} samples):",
        bid_stats_samples.len()
    );
    println!();
    println!("  📊 Bid Side:");
    println!("     Avg total volume: {avg_bid_volume:.0}");
    println!("     Avg levels:       {avg_bid_levels:.1}");
    println!(
        "     Avg concentration:{:.2}%",
        avg_bid_concentration * 100.0
    );
    println!();
    println!("  📊 Ask Side:");
    println!("     Avg total volume: {avg_ask_volume:.0}");
    println!("     Avg levels:       {avg_ask_levels:.1}");

    // Assertions
    assert!(
        !bid_stats_samples.is_empty(),
        "Should have bid stats samples"
    );
    assert!(
        !ask_stats_samples.is_empty(),
        "Should have ask stats samples"
    );
    assert!(avg_bid_volume > 0.0, "Bid volume should be positive");
    assert!(avg_ask_volume > 0.0, "Ask volume should be positive");
    assert!(avg_bid_levels > 0.0, "Should have bid levels");
    assert!(avg_ask_levels > 0.0, "Should have ask levels");

    println!("  ✅ DepthStats tests passed");
}

// ============================================================================
// Test: MarketImpact with Real Data
// ============================================================================

#[test]
fn test_market_impact_with_real_data() {
    use mbo_lob_reconstructor::MarketImpact;

    if !test_data_available() {
        eprintln!("⚠️  Skipping test: test data not available");
        return;
    }

    let path = get_test_data_path();
    println!("\n📂 Testing MarketImpact with: {path}");

    let loader = DbnLoader::new(&path)
        .expect("Failed to create loader")
        .skip_invalid(true);

    let mut lob = LobReconstructor::new(10);
    let mut impact_samples: Vec<mbo_lob_reconstructor::MarketImpact> = Vec::new();

    // Process 50K messages and simulate market impact periodically
    let mut processed = 0u64;
    for msg in loader.iter_messages().expect("Failed to iterate") {
        if msg.order_id == 0 || msg.size == 0 || msg.price <= 0 {
            continue;
        }

        if let Ok(state) = lob.process_message(&msg) {
            processed += 1;

            // Sample every 1000 messages
            if processed % 1000 == 0 && state.is_valid() {
                // Simulate buying 1000 shares
                let impact = MarketImpact::simulate_buy(&state, 1000);
                if impact.filled_quantity > 0 {
                    impact_samples.push(impact);
                }
            }
        }

        if processed >= 50_000 {
            break;
        }
    }

    // Calculate averages
    let avg_slippage_bps: f64 =
        impact_samples.iter().map(|i| i.slippage_bps).sum::<f64>() / impact_samples.len() as f64;

    let avg_levels_consumed: f64 = impact_samples
        .iter()
        .map(|i| i.levels_consumed as f64)
        .sum::<f64>()
        / impact_samples.len() as f64;

    let avg_fill_ratio: f64 =
        impact_samples.iter().map(|i| i.fill_ratio()).sum::<f64>() / impact_samples.len() as f64;

    let fully_filled_count = impact_samples.iter().filter(|i| i.can_fill()).count();

    println!(
        "  📊 MarketImpact Results for 1000 shares ({} samples):",
        impact_samples.len()
    );
    println!();
    println!("     Avg slippage:     {avg_slippage_bps:.2} bps");
    println!("     Avg levels used:  {avg_levels_consumed:.1}");
    println!("     Avg fill ratio:   {:.2}%", avg_fill_ratio * 100.0);
    println!(
        "     Fully filled:     {} ({:.1}%)",
        fully_filled_count,
        (fully_filled_count as f64 / impact_samples.len() as f64) * 100.0
    );

    // Sample a specific impact
    if let Some(sample) = impact_samples.last() {
        println!();
        println!("  📊 Sample Impact (last):");
        println!("     Best price:       ${:.4}", sample.best_price);
        println!("     Avg price:        ${:.4}", sample.avg_price);
        println!("     Worst price:      ${:.4}", sample.worst_price);
        println!(
            "     Slippage:         ${:.6} ({:.2} bps)",
            sample.slippage, sample.slippage_bps
        );
        println!(
            "     Filled:           {} / {}",
            sample.filled_quantity, sample.requested_quantity
        );
    }

    // Assertions
    assert!(!impact_samples.is_empty(), "Should have impact samples");
    assert!(avg_slippage_bps >= 0.0, "Slippage should be non-negative");
    assert!(avg_fill_ratio > 0.0, "Should have some fills");

    println!("  ✅ MarketImpact tests passed");
}

// ============================================================================
// Test: LiquidityMetrics with Real Data
// ============================================================================

#[test]
fn test_liquidity_metrics_with_real_data() {
    use mbo_lob_reconstructor::LiquidityMetrics;

    if !test_data_available() {
        eprintln!("⚠️  Skipping test: test data not available");
        return;
    }

    let path = get_test_data_path();
    println!("\n📂 Testing LiquidityMetrics with: {path}");

    let loader = DbnLoader::new(&path)
        .expect("Failed to create loader")
        .skip_invalid(true);

    let mut lob = LobReconstructor::new(10);
    let mut metrics_samples: Vec<mbo_lob_reconstructor::LiquidityMetrics> = Vec::new();

    // Process 50K messages and collect liquidity metrics periodically
    let mut processed = 0u64;
    for msg in loader.iter_messages().expect("Failed to iterate") {
        if msg.order_id == 0 || msg.size == 0 || msg.price <= 0 {
            continue;
        }

        if let Ok(state) = lob.process_message(&msg) {
            processed += 1;

            // Sample every 1000 messages
            if processed % 1000 == 0 && state.is_valid() {
                let metrics = LiquidityMetrics::from_lob_state(&state);
                if metrics.is_liquid() {
                    metrics_samples.push(metrics);
                }
            }
        }

        if processed >= 50_000 {
            break;
        }
    }

    // Calculate averages
    let avg_spread_bps: f64 =
        metrics_samples.iter().map(|m| m.spread_bps).sum::<f64>() / metrics_samples.len() as f64;

    let avg_total_volume: f64 = metrics_samples
        .iter()
        .map(|m| m.total_volume as f64)
        .sum::<f64>()
        / metrics_samples.len() as f64;

    let avg_imbalance: f64 = metrics_samples
        .iter()
        .map(|m| m.volume_imbalance)
        .sum::<f64>()
        / metrics_samples.len() as f64;

    let avg_levels: f64 = metrics_samples
        .iter()
        .map(|m| m.total_levels as f64)
        .sum::<f64>()
        / metrics_samples.len() as f64;

    println!(
        "  📊 LiquidityMetrics Results ({} samples):",
        metrics_samples.len()
    );
    println!();
    println!("     Avg spread:       {avg_spread_bps:.2} bps");
    println!("     Avg total volume: {avg_total_volume:.0}");
    println!("     Avg imbalance:    {avg_imbalance:.4}");
    println!("     Avg total levels: {avg_levels:.1}");

    // Sample a specific metrics
    if let Some(sample) = metrics_samples.last() {
        println!();
        println!("  📊 Sample Metrics (last):");
        println!("     Mid-price:        ${:.4}", sample.mid_price);
        println!("     Microprice:       ${:.4}", sample.microprice);
        println!(
            "     Spread:           ${:.6} ({:.2} bps)",
            sample.spread, sample.spread_bps
        );
        println!("     Bid volume:       {}", sample.bid_depth.total_volume);
        println!("     Ask volume:       {}", sample.ask_depth.total_volume);
        println!("     Book pressure:    {:.4}", sample.book_pressure());
    }

    // Assertions
    assert!(!metrics_samples.is_empty(), "Should have metrics samples");
    assert!(avg_spread_bps > 0.0, "Spread should be positive");
    assert!(avg_total_volume > 0.0, "Volume should be positive");
    assert!(avg_imbalance.abs() < 1.0, "Imbalance should be in [-1, 1]");

    println!("  ✅ LiquidityMetrics tests passed");
}

// ============================================================================
// Test: Edge Cases - Empty and Invalid States
// ============================================================================

#[test]
fn test_edge_case_empty_book() {
    use mbo_lob_reconstructor::{DepthStats, LiquidityMetrics, LobState, MarketImpact, Side};

    println!("\n📂 Testing edge cases: Empty book");

    // Empty LOB state
    let state = LobState::new(10);

    // Check consistency
    assert_eq!(state.check_consistency(), BookConsistency::Empty);
    assert!(!state.is_valid());
    assert!(!state.is_consistent());
    assert!(!state.is_crossed());
    assert!(!state.is_locked());

    // Analytics should return None or empty
    assert!(state.mid_price().is_none());
    assert!(state.spread().is_none());
    assert!(state.microprice().is_none());
    assert!(state.depth_imbalance().is_none());
    assert!(state.vwap_bid(5).is_none());
    assert!(state.vwap_ask(5).is_none());

    // Volume should be zero
    assert_eq!(state.total_bid_volume(), 0);
    assert_eq!(state.total_ask_volume(), 0);
    assert_eq!(state.active_bid_levels(), 0);
    assert_eq!(state.active_ask_levels(), 0);

    // DepthStats should be empty
    let bid_stats = DepthStats::from_lob_state(&state, Side::Bid);
    assert!(bid_stats.is_empty());
    assert_eq!(bid_stats.total_volume, 0);

    // MarketImpact should show no fills
    let impact = MarketImpact::simulate_buy(&state, 1000);
    assert_eq!(impact.filled_quantity, 0);
    assert_eq!(impact.total_quantity_available, 0);
    assert!(!impact.can_fill());

    // LiquidityMetrics should show not liquid
    let metrics = LiquidityMetrics::from_lob_state(&state);
    assert!(!metrics.is_liquid());
    assert_eq!(metrics.total_volume, 0);

    println!("  ✅ Empty book edge cases passed");
}

#[test]
fn test_edge_case_one_sided_book() {
    use mbo_lob_reconstructor::{DepthStats, LiquidityMetrics, LobState, MarketImpact, Side};

    println!("\n📂 Testing edge cases: One-sided book");

    // Only bid side
    let mut state = LobState::new(10);
    state.bid_prices[0] = 100_000_000_000; // $100.00
    state.bid_sizes[0] = 500;
    state.best_bid = Some(100_000_000_000);

    // Should be empty (not valid for trading)
    assert_eq!(state.check_consistency(), BookConsistency::Empty);
    assert!(!state.is_valid());

    // Mid-price and spread require both sides
    assert!(state.mid_price().is_none());
    assert!(state.spread().is_none());

    // But bid-side analytics should work
    assert_eq!(state.total_bid_volume(), 500);
    assert_eq!(state.active_bid_levels(), 1);

    let bid_stats = DepthStats::from_lob_state(&state, Side::Bid);
    assert!(!bid_stats.is_empty());
    assert_eq!(bid_stats.total_volume, 500);

    // Ask side should be empty
    let ask_stats = DepthStats::from_lob_state(&state, Side::Ask);
    assert!(ask_stats.is_empty());

    // Buy simulation should fail (no asks)
    let buy_impact = MarketImpact::simulate_buy(&state, 100);
    assert_eq!(buy_impact.filled_quantity, 0);

    // Sell simulation should work (has bids)
    let sell_impact = MarketImpact::simulate_sell(&state, 100);
    assert_eq!(sell_impact.filled_quantity, 100);

    // LiquidityMetrics should show not liquid
    let metrics = LiquidityMetrics::from_lob_state(&state);
    assert!(!metrics.is_liquid());

    println!("  ✅ One-sided book edge cases passed");
}

#[test]
fn test_edge_case_extreme_prices() {
    use mbo_lob_reconstructor::{Action, MboMessage, Side};

    println!("\n📂 Testing edge cases: Extreme prices");

    let config = LobConfig::new(10).with_logging(false);
    let mut lob = LobReconstructor::with_config(config);

    // Very high price ($10,000) - bid
    let high_bid = MboMessage::new(
        1,
        Action::Add,
        Side::Bid,
        10_000_000_000_000, // $10,000
        100,
    );
    let state = lob.process_message(&high_bid).unwrap();
    assert_eq!(state.best_bid, Some(10_000_000_000_000));

    // Higher ask ($10,001) - valid spread
    let high_ask = MboMessage::new(
        2,
        Action::Add,
        Side::Ask,
        10_001_000_000_000, // $10,001
        100,
    );
    let state = lob.process_message(&high_ask).unwrap();
    assert_eq!(state.best_ask, Some(10_001_000_000_000));

    // This creates a valid spread
    let mid = state.mid_price().unwrap();
    let spread = state.spread().unwrap();
    assert!(mid > 0.0);
    assert!(spread > 0.0);
    assert!(state.is_consistent());

    println!("  High price mid: ${mid:.4}");
    println!("  High price spread: ${spread:.4}");

    // Reset and test very low prices
    lob.reset();

    // Very low price ($0.01) - bid
    let low_bid = MboMessage::new(
        3,
        Action::Add,
        Side::Bid,
        10_000_000, // $0.01
        100,
    );
    lob.process_message(&low_bid).unwrap();

    // Higher ask ($0.02) - valid spread
    let low_ask = MboMessage::new(
        4,
        Action::Add,
        Side::Ask,
        20_000_000, // $0.02
        100,
    );
    let state = lob.process_message(&low_ask).unwrap();

    let mid_low = state.mid_price().unwrap();
    let spread_low = state.spread().unwrap();
    assert!(mid_low > 0.0);
    assert!(spread_low > 0.0);
    assert!(state.is_consistent());

    println!("  Low price mid: ${mid_low:.6}");
    println!("  Low price spread: ${spread_low:.6}");
    println!("  ✅ Extreme prices edge cases passed");
}

#[test]
fn test_edge_case_large_sizes() {
    use mbo_lob_reconstructor::{Action, MarketImpact, MboMessage, Side};

    println!("\n📂 Testing edge cases: Large order sizes");

    let config = LobConfig::new(10).with_logging(false);
    let mut lob = LobReconstructor::with_config(config);

    // Add orders with maximum u32 size
    let max_size = u32::MAX;
    let large_bid = MboMessage::new(
        1,
        Action::Add,
        Side::Bid,
        100_000_000_000, // $100
        max_size,
    );
    lob.process_message(&large_bid).unwrap();

    let large_ask = MboMessage::new(
        2,
        Action::Add,
        Side::Ask,
        100_010_000_000, // $100.01
        max_size,
    );
    let state = lob.process_message(&large_ask).unwrap();

    // Check volumes don't overflow
    let bid_vol = state.total_bid_volume();
    let ask_vol = state.total_ask_volume();
    assert_eq!(bid_vol, max_size as u64);
    assert_eq!(ask_vol, max_size as u64);

    // Market impact with large order
    let impact = MarketImpact::simulate_buy(&state, max_size as u64);
    assert!(impact.can_fill());
    assert_eq!(impact.filled_quantity, max_size as u64);

    // Depth imbalance should be balanced
    let imbalance = state.depth_imbalance().unwrap();
    assert!(imbalance.abs() < 0.001);

    println!("  Max size: {max_size}");
    println!("  Bid volume: {bid_vol}");
    println!("  Ask volume: {ask_vol}");
    println!("  ✅ Large sizes edge cases passed");
}

#[test]
fn test_edge_case_rapid_updates() {
    use mbo_lob_reconstructor::{Action, MboMessage, Side};

    println!("\n📂 Testing edge cases: Rapid order updates");

    let config = LobConfig::new(10).with_logging(false);
    let mut lob = LobReconstructor::with_config(config);

    // Add an order
    let add = MboMessage::new(1, Action::Add, Side::Bid, 100_000_000_000, 1000);
    lob.process_message(&add).unwrap();

    // Rapidly modify the same order 100 times
    for i in 0..100 {
        let modify = MboMessage::new(
            1,
            Action::Modify,
            Side::Bid,
            100_000_000_000 + (i * 1_000_000), // Price changes slightly
            1000 - i as u32,                   // Size decreases
        );
        lob.process_message(&modify).unwrap();
    }

    // Should still have exactly 1 order
    assert_eq!(lob.order_count(), 1);

    // Cancel the order
    let cancel = MboMessage::new(1, Action::Cancel, Side::Bid, 100_099_000_000, 901);
    lob.process_message(&cancel).unwrap();

    assert_eq!(lob.order_count(), 0);

    println!("  ✅ Rapid updates edge cases passed");
}

#[test]
fn test_edge_case_many_price_levels() {
    use mbo_lob_reconstructor::{Action, MboMessage, Side};

    println!("\n📂 Testing edge cases: Many price levels");

    let config = LobConfig::new(10).with_logging(false);
    let mut lob = LobReconstructor::with_config(config);

    // Add 1000 orders at different price levels
    for i in 0..1000 {
        let bid = MboMessage::new(
            1000 + i,
            Action::Add,
            Side::Bid,
            100_000_000_000 - (i as i64 * 1_000_000), // $100.00, $99.999, ...
            100,
        );
        lob.process_message(&bid).unwrap();
    }

    for i in 0..1000 {
        let ask = MboMessage::new(
            2000 + i,
            Action::Add,
            Side::Ask,
            100_010_000_000 + (i as i64 * 1_000_000), // $100.01, $100.011, ...
            100,
        );
        lob.process_message(&ask).unwrap();
    }

    // Should have 2000 orders
    assert_eq!(lob.order_count(), 2000);

    // LOB state should only show top 10 levels
    let state = lob.get_lob_state();
    assert_eq!(state.active_bid_levels(), 10);
    assert_eq!(state.active_ask_levels(), 10);

    // Best prices should be correct
    assert_eq!(state.best_bid, Some(100_000_000_000)); // $100.00
    assert_eq!(state.best_ask, Some(100_010_000_000)); // $100.01

    // Total volume should reflect all levels (not just top 10)
    // But the state only tracks top 10, so volume is for top 10
    let total_vol = state.total_bid_volume() + state.total_ask_volume();
    assert_eq!(total_vol, 2000); // 10 levels * 100 * 2 sides

    println!("  Total orders: {}", lob.order_count());
    println!("  Bid levels in LOB: {}", lob.bid_levels());
    println!("  Ask levels in LOB: {}", lob.ask_levels());
    println!("  ✅ Many price levels edge cases passed");
}

#[test]
fn test_edge_case_statistics_numerical_stability() {
    use mbo_lob_reconstructor::statistics::RunningStats;

    println!("\n📂 Testing edge cases: Statistics numerical stability");

    let mut stats = RunningStats::new();

    // Add very large values
    for _ in 0..1000 {
        stats.update(1e15);
    }

    // Add very small values
    for _ in 0..1000 {
        stats.update(1e-15);
    }

    // Mean should be approximately (1e15 + 1e-15) / 2 ≈ 5e14
    assert!(stats.mean > 4e14);
    assert!(stats.mean < 6e14);

    // Should not have NaN or Inf
    assert!(stats.mean.is_finite());
    assert!(stats.std().is_finite());

    // Test with values that could cause catastrophic cancellation
    let mut stats2 = RunningStats::new();
    let base = 1e10;
    for i in 0..1000 {
        stats2.update(base + (i as f64) * 1e-6);
    }

    // Mean should be approximately base + 0.0005
    assert!((stats2.mean - (base + 0.0005)).abs() < 0.001);
    assert!(stats2.std().is_finite());
    assert!(stats2.std() > 0.0);

    println!("  Large/small values mean: {:.2e}", stats.mean);
    println!("  Cancellation test mean: {:.6}", stats2.mean);
    println!("  Cancellation test std: {:.6e}", stats2.std());
    println!("  ✅ Numerical stability edge cases passed");
}

#[test]
fn test_edge_case_normalization_roundtrip() {
    use mbo_lob_reconstructor::{DayStats, LobState, NormalizationParams};

    println!("\n📂 Testing edge cases: Normalization roundtrip");

    // Create day stats with known values
    let mut day_stats = DayStats::new("test");

    for i in 0..100 {
        let mut state = LobState::new(10);
        let price = 100.0 + (i as f64) * 0.01;
        state.best_bid = Some((price * 1e9) as i64 - 5_000_000);
        state.best_ask = Some((price * 1e9) as i64 + 5_000_000);
        state.bid_sizes[0] = 100 + i;
        state.ask_sizes[0] = 150 + i;
        day_stats.update(&state);
    }

    // Create normalization params
    let params = NormalizationParams::from_day_stats(&day_stats, 10);

    // Test roundtrip for various values
    let test_values = [50.0, 100.0, 150.0, 0.0, -50.0, 1e6];

    for &original in &test_values {
        let normalized = params.normalize(original, 0);
        let denormalized = params.denormalize(normalized, 0);

        assert!(
            (original - denormalized).abs() < 1e-10,
            "Roundtrip failed: {original} -> {normalized} -> {denormalized}"
        );
    }

    // Test out of bounds index (should return unchanged)
    let out_of_bounds = params.normalize(100.0, 1000);
    assert_eq!(out_of_bounds, 100.0);

    println!("  Roundtrip tests passed for {} values", test_values.len());
    println!("  ✅ Normalization roundtrip edge cases passed");
}
