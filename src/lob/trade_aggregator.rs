//! Trade Aggregation with Aggressor Side Detection
//!
//! This module aggregates individual fill events into trades and detects the
//! aggressor (initiator) side. This is critical for:
//!
//! - **Price Impact Models**: Aggressor side is a key predictor of short-term price movement
//! - **Trade Flow Analysis**: Understanding buying vs selling pressure
//! - **Feature Engineering**: Trade imbalance features for ML models
//!
//! # Research Reference
//!
//! > "Trade direction is a key predictor of short-term price movement" — Cont et al.
//!
//! # How Aggressor Detection Works
//!
//! In MBO data, when a trade occurs:
//! - If the trade matches against a **bid** order → the aggressor is a **seller**
//! - If the trade matches against an **ask** order → the aggressor is a **buyer**
//!
//! # Example
//!
//! ```ignore
//! use mbo_lob_reconstructor::lob::trade_aggregator::{TradeAggregator, TradeAggregatorConfig};
//!
//! let mut aggregator = TradeAggregator::new(TradeAggregatorConfig::default());
//!
//! for msg in messages {
//!     if let Some(trade) = aggregator.process_message(&msg) {
//!         println!("Trade: {} @ {} (aggressor: {:?})",
//!                  trade.size, trade.price, trade.aggressor_side);
//!     }
//! }
//!
//! // Get trade imbalance
//! let imbalance = aggregator.trade_imbalance();
//! println!("Buy pressure: {:.2}%", (imbalance + 1.0) / 2.0 * 100.0);
//! ```

use crate::types::{Action, MboMessage, Side};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for the trade aggregator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeAggregatorConfig {
    /// Maximum number of recent trades to keep in memory.
    ///
    /// Used for computing rolling statistics like trade imbalance.
    /// Default: 1000
    pub max_recent_trades: usize,

    /// Time window for aggregating fills into a single trade (nanoseconds).
    ///
    /// Fills at the same price within this window are merged.
    /// Default: 1_000_000 (1 millisecond)
    pub aggregation_window_ns: i64,

    /// Whether to track individual fills within trades.
    ///
    /// Enabling this uses more memory but provides fill-level detail.
    /// Default: false
    pub track_fills: bool,
}

impl Default for TradeAggregatorConfig {
    fn default() -> Self {
        Self {
            max_recent_trades: 1000,
            aggregation_window_ns: 1_000_000, // 1ms
            track_fills: false,
        }
    }
}

impl TradeAggregatorConfig {
    /// Create with custom recent trades limit.
    pub fn with_max_trades(mut self, max: usize) -> Self {
        self.max_recent_trades = max;
        self
    }

    /// Create with custom aggregation window.
    pub fn with_aggregation_window_ns(mut self, window_ns: i64) -> Self {
        self.aggregation_window_ns = window_ns;
        self
    }

    /// Enable fill tracking.
    pub fn with_fill_tracking(mut self) -> Self {
        self.track_fills = true;
        self
    }
}

// ============================================================================
// Trade Structure
// ============================================================================

/// A completed trade (potentially aggregating multiple fills).
#[derive(Debug, Clone)]
pub struct Trade {
    /// Trade timestamp (from the first fill).
    pub timestamp: i64,

    /// Trade price (fixed-point, divide by 1e9 for dollars).
    pub price: i64,

    /// Total trade size (sum of all fills).
    pub size: u64,

    /// Aggressor side (who initiated the trade).
    ///
    /// - `Side::Bid` = aggressive buyer (took liquidity from asks)
    /// - `Side::Ask` = aggressive seller (took liquidity from bids)
    pub aggressor_side: Side,

    /// Number of fills that make up this trade.
    pub num_fills: u32,

    /// Tick direction relative to previous trade.
    ///
    /// - `1`: Uptick (price > previous)
    /// - `-1`: Downtick (price < previous)
    /// - `0`: Same price
    pub tick_direction: i8,

    /// Individual fills (only populated if `track_fills` is enabled).
    pub fills: Option<Vec<Fill>>,
}

impl Trade {
    /// Get the price as f64 (dollars).
    pub fn price_f64(&self) -> f64 {
        self.price as f64 / 1_000_000_000.0
    }

    /// Check if this was a buyer-initiated trade.
    pub fn is_buy(&self) -> bool {
        self.aggressor_side == Side::Bid
    }

    /// Check if this was a seller-initiated trade.
    pub fn is_sell(&self) -> bool {
        self.aggressor_side == Side::Ask
    }
}

/// An individual fill within a trade.
#[derive(Debug, Clone)]
pub struct Fill {
    /// Fill timestamp.
    pub timestamp: i64,

    /// Fill price.
    pub price: i64,

    /// Fill size.
    pub size: u32,

    /// Order ID that was filled.
    pub order_id: u64,
}

// ============================================================================
// Trade Aggregator
// ============================================================================

/// Aggregates MBO fill events into trades with aggressor detection.
pub struct TradeAggregator {
    config: TradeAggregatorConfig,

    /// Recent completed trades.
    recent_trades: VecDeque<Trade>,

    /// Current trade being built (pending aggregation).
    pending_trade: Option<PendingTrade>,

    /// Last trade price (for tick direction).
    last_trade_price: Option<i64>,

    /// Statistics
    total_buy_volume: u64,
    total_sell_volume: u64,
    total_trades: u64,
}

/// Internal: trade being aggregated.
struct PendingTrade {
    timestamp: i64,
    price: i64,
    size: u64,
    aggressor_side: Side,
    num_fills: u32,
    fills: Vec<Fill>,
}

impl TradeAggregator {
    /// Create a new trade aggregator.
    pub fn new(config: TradeAggregatorConfig) -> Self {
        Self {
            config,
            recent_trades: VecDeque::new(),
            pending_trade: None,
            last_trade_price: None,
            total_buy_volume: 0,
            total_sell_volume: 0,
            total_trades: 0,
        }
    }

    /// Process an MBO message, potentially returning a completed trade.
    ///
    /// Call this for every MBO message. Returns `Some(Trade)` when a trade
    /// is complete (either a single fill or after aggregation window expires).
    pub fn process_message(&mut self, msg: &MboMessage) -> Option<Trade> {
        // Only process trades/fills
        if msg.action != Action::Trade && msg.action != Action::Fill {
            // Check if pending trade should be finalized due to time gap
            return self.check_finalize_pending(msg.timestamp);
        }

        let timestamp = msg.timestamp.unwrap_or(0);
        let price = msg.price;
        let size = msg.size;

        // Determine aggressor side
        // In MBO data, the side field indicates which side of the book was HIT
        // If a bid order was filled, an aggressive seller took liquidity
        // If an ask order was filled, an aggressive buyer took liquidity
        let aggressor_side = match msg.side {
            Side::Bid => Side::Ask,  // Bid was hit → seller aggressed
            Side::Ask => Side::Bid,  // Ask was hit → buyer aggressed
            Side::None => return self.check_finalize_pending(msg.timestamp), // Skip unknown side
        };

        // Check if this fill can be aggregated with pending trade
        if let Some(ref mut pending) = self.pending_trade {
            let time_diff = timestamp - pending.timestamp;
            let same_price = pending.price == price;
            let same_aggressor = pending.aggressor_side == aggressor_side;

            if same_price && same_aggressor && time_diff <= self.config.aggregation_window_ns {
                // Aggregate into pending trade
                pending.size += size as u64;
                pending.num_fills += 1;

                if self.config.track_fills {
                    pending.fills.push(Fill {
                        timestamp,
                        price,
                        size,
                        order_id: msg.order_id,
                    });
                }

                return None;
            }

            // Finalize pending trade and start new one
            let finalized = self.finalize_pending();

            // Start new pending trade
            self.pending_trade = Some(PendingTrade {
                timestamp,
                price,
                size: size as u64,
                aggressor_side,
                num_fills: 1,
                fills: if self.config.track_fills {
                    vec![Fill {
                        timestamp,
                        price,
                        size,
                        order_id: msg.order_id,
                    }]
                } else {
                    Vec::new()
                },
            });

            return finalized;
        }

        // Start new pending trade
        self.pending_trade = Some(PendingTrade {
            timestamp,
            price,
            size: size as u64,
            aggressor_side,
            num_fills: 1,
            fills: if self.config.track_fills {
                vec![Fill {
                    timestamp,
                    price,
                    size,
                    order_id: msg.order_id,
                }]
            } else {
                Vec::new()
            },
        });

        None
    }

    /// Force finalization of any pending trade.
    ///
    /// Call this at the end of processing to get the last trade.
    pub fn flush(&mut self) -> Option<Trade> {
        self.finalize_pending()
    }

    /// Check if pending trade should be finalized due to time gap.
    fn check_finalize_pending(&mut self, current_ts: Option<i64>) -> Option<Trade> {
        if let (Some(ref pending), Some(ts)) = (&self.pending_trade, current_ts) {
            let time_diff = ts - pending.timestamp;
            if time_diff > self.config.aggregation_window_ns {
                return self.finalize_pending();
            }
        }
        None
    }

    /// Finalize the pending trade.
    fn finalize_pending(&mut self) -> Option<Trade> {
        let pending = self.pending_trade.take()?;

        // Compute tick direction
        let tick_direction = match self.last_trade_price {
            Some(last) if pending.price > last => 1,
            Some(last) if pending.price < last => -1,
            _ => 0,
        };
        self.last_trade_price = Some(pending.price);

        // Update statistics
        self.total_trades += 1;
        if pending.aggressor_side == Side::Bid {
            self.total_buy_volume += pending.size;
        } else {
            self.total_sell_volume += pending.size;
        }

        let trade = Trade {
            timestamp: pending.timestamp,
            price: pending.price,
            size: pending.size,
            aggressor_side: pending.aggressor_side,
            num_fills: pending.num_fills,
            tick_direction,
            fills: if self.config.track_fills {
                Some(pending.fills)
            } else {
                None
            },
        };

        // Store in recent trades
        self.recent_trades.push_back(trade.clone());
        while self.recent_trades.len() > self.config.max_recent_trades {
            self.recent_trades.pop_front();
        }

        Some(trade)
    }

    // ========================================================================
    // Statistics
    // ========================================================================

    /// Get recent trades.
    pub fn recent_trades(&self) -> &VecDeque<Trade> {
        &self.recent_trades
    }

    /// Get the N most recent trades.
    pub fn last_n_trades(&self, n: usize) -> Vec<&Trade> {
        self.recent_trades.iter().rev().take(n).collect()
    }

    /// Compute trade imbalance: (buy_volume - sell_volume) / total_volume.
    ///
    /// Returns a value in [-1, 1]:
    /// - `1.0`: All buying pressure
    /// - `-1.0`: All selling pressure
    /// - `0.0`: Balanced
    pub fn trade_imbalance(&self) -> f64 {
        let total = self.total_buy_volume + self.total_sell_volume;
        if total == 0 {
            return 0.0;
        }
        (self.total_buy_volume as f64 - self.total_sell_volume as f64) / total as f64
    }

    /// Compute recent trade imbalance (last N trades only).
    pub fn recent_trade_imbalance(&self, n: usize) -> f64 {
        let mut buy_vol = 0u64;
        let mut sell_vol = 0u64;

        for trade in self.recent_trades.iter().rev().take(n) {
            if trade.is_buy() {
                buy_vol += trade.size;
            } else {
                sell_vol += trade.size;
            }
        }

        let total = buy_vol + sell_vol;
        if total == 0 {
            return 0.0;
        }
        (buy_vol as f64 - sell_vol as f64) / total as f64
    }

    /// Get total buy volume.
    pub fn total_buy_volume(&self) -> u64 {
        self.total_buy_volume
    }

    /// Get total sell volume.
    pub fn total_sell_volume(&self) -> u64 {
        self.total_sell_volume
    }

    /// Get total number of trades.
    pub fn total_trades(&self) -> u64 {
        self.total_trades
    }

    /// Get last trade price (fixed-point).
    pub fn last_trade_price(&self) -> Option<i64> {
        self.last_trade_price
    }

    /// Reset all statistics.
    pub fn reset(&mut self) {
        self.recent_trades.clear();
        self.pending_trade = None;
        self.last_trade_price = None;
        self.total_buy_volume = 0;
        self.total_sell_volume = 0;
        self.total_trades = 0;
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_trade_msg(order_id: u64, side: Side, price: i64, size: u32, ts: i64) -> MboMessage {
        MboMessage {
            order_id,
            action: Action::Trade,
            side,
            price,
            size,
            timestamp: Some(ts),
        }
    }

    #[test]
    fn test_config_default() {
        let config = TradeAggregatorConfig::default();
        assert_eq!(config.max_recent_trades, 1000);
        assert_eq!(config.aggregation_window_ns, 1_000_000);
        assert!(!config.track_fills);
    }

    #[test]
    fn test_single_trade() {
        let mut agg = TradeAggregator::new(TradeAggregatorConfig::default());

        // Single trade - bid side hit means seller aggressed
        let msg = make_trade_msg(1, Side::Bid, 100_000_000_000, 100, 1000);
        let result = agg.process_message(&msg);

        // Trade should be pending (waiting for potential aggregation)
        assert!(result.is_none());

        // Flush to get the trade
        let trade = agg.flush().expect("Should have trade");
        assert_eq!(trade.price, 100_000_000_000);
        assert_eq!(trade.size, 100);
        assert_eq!(trade.aggressor_side, Side::Ask); // Seller aggressed
        assert_eq!(trade.num_fills, 1);
    }

    #[test]
    fn test_aggressor_detection_buyer() {
        let mut agg = TradeAggregator::new(TradeAggregatorConfig::default());

        // Ask side hit means buyer aggressed
        let msg = make_trade_msg(1, Side::Ask, 100_000_000_000, 100, 1000);
        agg.process_message(&msg);

        let trade = agg.flush().unwrap();
        assert_eq!(trade.aggressor_side, Side::Bid); // Buyer aggressed
        assert!(trade.is_buy());
        assert!(!trade.is_sell());
    }

    #[test]
    fn test_trade_aggregation() {
        let config = TradeAggregatorConfig::default()
            .with_aggregation_window_ns(10_000_000); // 10ms window
        let mut agg = TradeAggregator::new(config);

        // Three fills at same price within window
        agg.process_message(&make_trade_msg(1, Side::Ask, 100_000_000_000, 50, 1000));
        agg.process_message(&make_trade_msg(2, Side::Ask, 100_000_000_000, 30, 2000));
        agg.process_message(&make_trade_msg(3, Side::Ask, 100_000_000_000, 20, 3000));

        let trade = agg.flush().unwrap();
        assert_eq!(trade.size, 100); // 50 + 30 + 20
        assert_eq!(trade.num_fills, 3);
    }

    #[test]
    fn test_no_aggregation_different_price() {
        let config = TradeAggregatorConfig::default()
            .with_aggregation_window_ns(10_000_000);
        let mut agg = TradeAggregator::new(config);

        // First fill
        let result1 = agg.process_message(&make_trade_msg(1, Side::Ask, 100_000_000_000, 50, 1000));
        assert!(result1.is_none());

        // Second fill at different price - should finalize first trade
        let result2 = agg.process_message(&make_trade_msg(2, Side::Ask, 101_000_000_000, 30, 2000));
        assert!(result2.is_some());

        let trade1 = result2.unwrap();
        assert_eq!(trade1.price, 100_000_000_000);
        assert_eq!(trade1.size, 50);

        let trade2 = agg.flush().unwrap();
        assert_eq!(trade2.price, 101_000_000_000);
        assert_eq!(trade2.size, 30);
    }

    #[test]
    fn test_trade_imbalance_all_buys() {
        let mut agg = TradeAggregator::new(TradeAggregatorConfig::default());

        // All buyer-initiated trades
        agg.process_message(&make_trade_msg(1, Side::Ask, 100_000_000_000, 100, 1000));
        agg.flush();

        assert_eq!(agg.trade_imbalance(), 1.0);
    }

    #[test]
    fn test_trade_imbalance_all_sells() {
        let mut agg = TradeAggregator::new(TradeAggregatorConfig::default());

        // All seller-initiated trades
        agg.process_message(&make_trade_msg(1, Side::Bid, 100_000_000_000, 100, 1000));
        agg.flush();

        assert_eq!(agg.trade_imbalance(), -1.0);
    }

    #[test]
    fn test_trade_imbalance_balanced() {
        let config = TradeAggregatorConfig::default()
            .with_aggregation_window_ns(0); // No aggregation
        let mut agg = TradeAggregator::new(config);

        // Equal buy and sell volume
        agg.process_message(&make_trade_msg(1, Side::Ask, 100_000_000_000, 100, 1000));
        agg.process_message(&make_trade_msg(2, Side::Bid, 100_000_000_000, 100, 2000));
        agg.flush();

        assert_eq!(agg.trade_imbalance(), 0.0);
    }

    #[test]
    fn test_tick_direction() {
        let config = TradeAggregatorConfig::default()
            .with_aggregation_window_ns(0);
        let mut agg = TradeAggregator::new(config);

        // First trade
        agg.process_message(&make_trade_msg(1, Side::Ask, 100_000_000_000, 100, 1000));
        let trade1 = agg.flush().unwrap();
        assert_eq!(trade1.tick_direction, 0); // No previous

        // Uptick
        agg.process_message(&make_trade_msg(2, Side::Ask, 101_000_000_000, 100, 2000));
        let trade2 = agg.flush().unwrap();
        assert_eq!(trade2.tick_direction, 1);

        // Downtick
        agg.process_message(&make_trade_msg(3, Side::Ask, 99_000_000_000, 100, 3000));
        let trade3 = agg.flush().unwrap();
        assert_eq!(trade3.tick_direction, -1);

        // Same price
        agg.process_message(&make_trade_msg(4, Side::Ask, 99_000_000_000, 100, 4000));
        let trade4 = agg.flush().unwrap();
        assert_eq!(trade4.tick_direction, 0);
    }

    #[test]
    fn test_recent_trades_limit() {
        let config = TradeAggregatorConfig::default()
            .with_max_trades(3)
            .with_aggregation_window_ns(0);
        let mut agg = TradeAggregator::new(config);

        // Add 5 trades
        for i in 0..5 {
            agg.process_message(&make_trade_msg(i as u64, Side::Ask, 100_000_000_000, 100, i * 1000));
        }
        agg.flush();

        // Should only have 3 most recent
        assert_eq!(agg.recent_trades().len(), 3);
    }

    #[test]
    fn test_fill_tracking() {
        let config = TradeAggregatorConfig::default()
            .with_fill_tracking()
            .with_aggregation_window_ns(10_000_000);
        let mut agg = TradeAggregator::new(config);

        agg.process_message(&make_trade_msg(1, Side::Ask, 100_000_000_000, 50, 1000));
        agg.process_message(&make_trade_msg(2, Side::Ask, 100_000_000_000, 30, 2000));

        let trade = agg.flush().unwrap();
        assert!(trade.fills.is_some());

        let fills = trade.fills.unwrap();
        assert_eq!(fills.len(), 2);
        assert_eq!(fills[0].order_id, 1);
        assert_eq!(fills[0].size, 50);
        assert_eq!(fills[1].order_id, 2);
        assert_eq!(fills[1].size, 30);
    }

    #[test]
    fn test_reset() {
        let config = TradeAggregatorConfig::default().with_aggregation_window_ns(0);
        let mut agg = TradeAggregator::new(config);

        agg.process_message(&make_trade_msg(1, Side::Ask, 100_000_000_000, 100, 1000));
        agg.flush();

        assert_eq!(agg.total_trades(), 1);
        assert!(agg.last_trade_price().is_some());

        agg.reset();

        assert_eq!(agg.total_trades(), 0);
        assert!(agg.last_trade_price().is_none());
        assert!(agg.recent_trades().is_empty());
    }

    #[test]
    fn test_non_trade_messages_ignored() {
        let mut agg = TradeAggregator::new(TradeAggregatorConfig::default());

        // Add message (not a trade)
        let msg = MboMessage::new(1, Action::Add, Side::Bid, 100_000_000_000, 100);
        let result = agg.process_message(&msg);

        assert!(result.is_none());
        assert!(agg.flush().is_none());
    }

    #[test]
    fn test_recent_trade_imbalance() {
        let config = TradeAggregatorConfig::default()
            .with_aggregation_window_ns(0);
        let mut agg = TradeAggregator::new(config);

        // 3 buys, then 2 sells
        for i in 0..3u64 {
            agg.process_message(&make_trade_msg(i, Side::Ask, 100_000_000_000, 100, (i * 1000) as i64));
        }
        for i in 3..5u64 {
            agg.process_message(&make_trade_msg(i, Side::Bid, 100_000_000_000, 100, (i * 1000) as i64));
        }
        agg.flush();

        // Overall: 300 buy, 200 sell = 0.2 imbalance
        let overall = agg.trade_imbalance();
        assert!((overall - 0.2).abs() < 0.001);

        // Last 2 trades: both sells = -1.0
        let recent_2 = agg.recent_trade_imbalance(2);
        assert_eq!(recent_2, -1.0);

        // Last 3 trades: 1 buy, 2 sell = -0.333...
        let recent_3 = agg.recent_trade_imbalance(3);
        assert!((recent_3 - (-1.0 / 3.0)).abs() < 0.001);
    }
}

