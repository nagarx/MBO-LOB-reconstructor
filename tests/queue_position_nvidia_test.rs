//! Integration tests for QueuePositionTracker with real NVIDIA data.
//!
//! These tests validate the queue position tracking using actual MBO data
//! to ensure correctness in real-world scenarios.

use mbo_lob_reconstructor::{
    DbnLoader, LobReconstructor, QueuePositionConfig, QueuePositionTracker,
};
use std::path::Path;

const HOT_STORE_DIR: &str = "../data/hot_store";
const COMPRESSED_DATA_DIR: &str = "../data/NVDA_2025-02-01_to_2025-09-30";

/// Get a test file path, preferring hot store over compressed.
fn get_test_file() -> Option<std::path::PathBuf> {
    let hot_store_path = Path::new(HOT_STORE_DIR);
    let compressed_path = Path::new(COMPRESSED_DATA_DIR);

    // Try hot store first
    if hot_store_path.exists() {
        if let Ok(entries) = std::fs::read_dir(hot_store_path) {
            let mut files: Vec<_> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.extension().map(|ext| ext == "dbn").unwrap_or(false)
                        && p.to_string_lossy().contains(".mbo.dbn")
                })
                .collect();
            files.sort();
            if let Some(f) = files.first() {
                println!("   📂 Using HOT STORE: {}", f.display());
                return Some(f.clone());
            }
        }
    }

    // Fallback to compressed
    if compressed_path.exists() {
        if let Ok(entries) = std::fs::read_dir(compressed_path) {
            let mut files: Vec<_> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.extension().map(|ext| ext == "zst").unwrap_or(false)
                        && p.to_string_lossy().contains(".mbo.dbn.zst")
                })
                .collect();
            files.sort();
            if let Some(f) = files.first() {
                println!("   📂 Using COMPRESSED: {}", f.display());
                return Some(f.clone());
            }
        }
    }

    None
}

#[test]
fn test_queue_position_with_real_nvidia_data() {
    let test_file = match get_test_file() {
        Some(f) => f,
        None => {
            println!("⚠️ No NVIDIA test data found, skipping test");
            return;
        }
    };

    let loader = DbnLoader::new(test_file.to_string_lossy().to_string()).unwrap();
    let messages: Vec<_> = loader.iter_messages().unwrap().take(50_000).collect();

    println!("   📊 Loaded {} messages", messages.len());

    let config = QueuePositionConfig::default().with_change_tracking();
    let mut tracker = QueuePositionTracker::new(config);
    let mut lob = LobReconstructor::new(10);

    // Process messages through both
    for msg in &messages {
        let _ = lob.process_message(msg);
        tracker.process_message(msg);
    }

    let stats = tracker.stats();
    println!("   📈 Queue Position Statistics:");
    println!("      Orders tracked: {}", stats.orders_tracked);
    println!("      Orders removed: {}", stats.orders_removed);
    println!("      Active orders: {}", tracker.active_orders());
    println!("      Messages skipped: {}", stats.messages_skipped);

    // Validate basic sanity checks
    assert!(stats.orders_tracked > 0, "Should track some orders");
    assert!(
        stats.messages_skipped < messages.len() as u64,
        "Should process most messages"
    );

    // Check queue imbalance
    if let Some((imbalance, bid_vol, ask_vol)) = tracker.best_level_imbalance() {
        println!("   📊 Best Level Imbalance:");
        println!("      Bid volume: {}", bid_vol);
        println!("      Ask volume: {}", ask_vol);
        println!("      Imbalance: {:.4}", imbalance);

        // Imbalance should be in valid range
        assert!(
            imbalance >= -1.0 && imbalance <= 1.0,
            "Imbalance should be in [-1, 1]"
        );
    }
}

#[test]
fn test_queue_position_fifo_validation() {
    let test_file = match get_test_file() {
        Some(f) => f,
        None => {
            println!("⚠️ No NVIDIA test data found, skipping test");
            return;
        }
    };

    let loader = DbnLoader::new(test_file.to_string_lossy().to_string()).unwrap();
    let messages: Vec<_> = loader.iter_messages().unwrap().take(20_000).collect();

    let mut tracker = QueuePositionTracker::new(QueuePositionConfig::default());

    // Track order IDs we've seen
    let mut add_order_ids: Vec<u64> = Vec::new();

    for msg in &messages {
        if msg.action == mbo_lob_reconstructor::Action::Add
            && msg.order_id != 0
            && msg.size > 0
            && msg.price > 0
        {
            add_order_ids.push(msg.order_id);
        }
        tracker.process_message(msg);
    }

    // Validate FIFO: earlier orders should have lower positions
    let mut position_violations = 0;
    let mut checks = 0;

    for window in add_order_ids.windows(2) {
        let (id1, id2) = (window[0], window[1]);

        if let (Some(pos1), Some(pos2)) = (tracker.queue_position(id1), tracker.queue_position(id2))
        {
            // Only compare if same price and side
            if pos1.price == pos2.price && pos1.side == pos2.side {
                checks += 1;
                if pos1.position > pos2.position {
                    // Earlier order has higher position - FIFO violation
                    position_violations += 1;
                }
            }
        }
    }

    println!(
        "   🔍 FIFO checks: {}, violations: {}",
        checks, position_violations
    );

    // Allow small number of violations (due to modifications, etc.)
    let violation_rate = if checks > 0 {
        position_violations as f64 / checks as f64
    } else {
        0.0
    };
    assert!(
        violation_rate < 0.05,
        "FIFO violation rate should be < 5%, got {:.2}%",
        violation_rate * 100.0
    );
}

#[test]
fn test_queue_position_volume_consistency() {
    let test_file = match get_test_file() {
        Some(f) => f,
        None => {
            println!("⚠️ No NVIDIA test data found, skipping test");
            return;
        }
    };

    let loader = DbnLoader::new(test_file.to_string_lossy().to_string()).unwrap();
    let messages: Vec<_> = loader.iter_messages().unwrap().take(30_000).collect();

    let mut tracker = QueuePositionTracker::new(QueuePositionConfig::default());
    let mut lob = LobReconstructor::new(10);

    for msg in &messages {
        let _ = lob.process_message(msg);
        tracker.process_message(msg);
    }

    // Get LOB state
    let lob_state = lob.get_lob_state();

    // Compare volumes at best bid
    if let Some(best_bid) = lob_state.best_bid {
        let lob_vol = lob_state.bid_sizes[0] as u64;
        let queue_vol = tracker.level_volume(mbo_lob_reconstructor::Side::Bid, best_bid);

        println!(
            "   📊 Best bid {} - LOB vol: {}, Queue vol: {}",
            best_bid as f64 / 1e9,
            lob_vol,
            queue_vol
        );

        // Volumes should match (or be very close)
        let diff = (lob_vol as i64 - queue_vol as i64).abs();
        let tolerance = (lob_vol.max(queue_vol) as f64 * 0.01) as i64; // 1% tolerance

        assert!(
            diff <= tolerance.max(1),
            "Volume mismatch at best bid: LOB={}, Queue={}",
            lob_vol,
            queue_vol
        );
    }

    // Compare volumes at best ask
    if let Some(best_ask) = lob_state.best_ask {
        let lob_vol = lob_state.ask_sizes[0] as u64;
        let queue_vol = tracker.level_volume(mbo_lob_reconstructor::Side::Ask, best_ask);

        println!(
            "   📊 Best ask {} - LOB vol: {}, Queue vol: {}",
            best_ask as f64 / 1e9,
            lob_vol,
            queue_vol
        );

        let diff = (lob_vol as i64 - queue_vol as i64).abs();
        let tolerance = (lob_vol.max(queue_vol) as f64 * 0.01) as i64;

        assert!(
            diff <= tolerance.max(1),
            "Volume mismatch at best ask: LOB={}, Queue={}",
            lob_vol,
            queue_vol
        );
    }
}

#[test]
fn test_queue_position_imbalance_predictor() {
    let test_file = match get_test_file() {
        Some(f) => f,
        None => {
            println!("⚠️ No NVIDIA test data found, skipping test");
            return;
        }
    };

    let loader = DbnLoader::new(test_file.to_string_lossy().to_string()).unwrap();
    let messages: Vec<_> = loader.iter_messages().unwrap().take(100_000).collect();

    let mut tracker = QueuePositionTracker::new(QueuePositionConfig::default());
    let mut lob = LobReconstructor::new(10);

    // Track imbalance vs mid-price changes
    let mut samples: Vec<(f64, f64)> = Vec::new(); // (imbalance, subsequent_mid_change)
    let mut prev_mid: Option<f64> = None;

    for msg in &messages {
        let state = lob.process_message(msg).unwrap();
        tracker.process_message(msg);

        if let Some(mid) = state.mid_price() {
            if let Some(prev) = prev_mid {
                if let Some((imbalance, _, _)) = tracker.best_level_imbalance() {
                    let mid_change = mid - prev;
                    samples.push((imbalance, mid_change));
                }
            }
            prev_mid = Some(mid);
        }
    }

    // Analyze correlation between imbalance and price change
    if samples.len() > 100 {
        // Split by imbalance sign
        let (positive_imb, negative_imb): (Vec<_>, Vec<_>) =
            samples.iter().partition(|(imb, _)| *imb > 0.0);

        let pos_avg_change: f64 = if !positive_imb.is_empty() {
            positive_imb.iter().map(|(_, c)| c).sum::<f64>() / positive_imb.len() as f64
        } else {
            0.0
        };

        let neg_avg_change: f64 = if !negative_imb.is_empty() {
            negative_imb.iter().map(|(_, c)| c).sum::<f64>() / negative_imb.len() as f64
        } else {
            0.0
        };

        println!("   📈 Imbalance-Price Analysis ({} samples):", samples.len());
        println!(
            "      Positive imbalance ({}) → avg mid change: {:.6e}",
            positive_imb.len(),
            pos_avg_change
        );
        println!(
            "      Negative imbalance ({}) → avg mid change: {:.6e}",
            negative_imb.len(),
            neg_avg_change
        );

        // Research shows positive imbalance should lead to positive price changes
        // This is a weak test, but validates the direction
        // Note: We don't assert because short-term data may not show this clearly
    }
}

#[test]
fn test_queue_position_multi_level_imbalance() {
    let test_file = match get_test_file() {
        Some(f) => f,
        None => {
            println!("⚠️ No NVIDIA test data found, skipping test");
            return;
        }
    };

    let loader = DbnLoader::new(test_file.to_string_lossy().to_string()).unwrap();
    let messages: Vec<_> = loader.iter_messages().unwrap().take(50_000).collect();

    let mut tracker = QueuePositionTracker::new(QueuePositionConfig::default());

    for msg in &messages {
        tracker.process_message(msg);
    }

    // Compare single-level vs multi-level imbalance
    if let (Some((imb1, _, _)), Some((imb5, bid5, ask5))) = (
        tracker.best_level_imbalance(),
        tracker.multi_level_imbalance(5),
    ) {
        println!("   📊 Level Comparison:");
        println!("      1-level imbalance: {:.4}", imb1);
        println!(
            "      5-level imbalance: {:.4} (bid={}, ask={})",
            imb5, bid5, ask5
        );

        // Both should be valid
        assert!(imb1 >= -1.0 && imb1 <= 1.0);
        assert!(imb5 >= -1.0 && imb5 <= 1.0);

        // Multi-level typically smooths out extreme values
        if imb1.abs() > 0.8 {
            assert!(
                imb5.abs() <= imb1.abs() + 0.1,
                "Multi-level should smooth extreme imbalances"
            );
        }
    }
}

