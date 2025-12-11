//! Queue Position Tracking for LOB Orders
//!
//! This module tracks the FIFO queue position of orders at each price level.
//! Queue position is critical for:
//!
//! - **Execution probability models**: Orders at front of queue fill first
//! - **Alpha signals**: Queue position changes indicate informed trading
//! - **Market making**: Optimal placement requires queue awareness
//!
//! # Research Reference
//!
//! > "Queue position is a first-order determinant of execution probability"
//! > — HLOB Paper
//!
//! > "Queue imbalance at the best level provides significant predictive power
//! >  for the direction of the next mid-price movement"
//! > — Gould & Bonart (2015)
//!
//! # Design
//!
//! This module is **standalone and composable** - it does NOT modify the core
//! `LobReconstructor` or `PriceLevel` structures. This keeps the hot path fast
//! when queue position tracking is not needed.
//!
//! # Example
//!
//! ```ignore
//! use mbo_lob_reconstructor::lob::queue_position::{QueuePositionTracker, QueuePositionConfig};
//!
//! let mut tracker = QueuePositionTracker::new(QueuePositionConfig::default());
//!
//! for msg in messages {
//!     tracker.process_message(&msg);
//!     
//!     // Query queue position for a specific order
//!     if let Some(pos) = tracker.queue_position(msg.order_id) {
//!         println!("Order {} is at position {} with {} volume ahead",
//!                  msg.order_id, pos.position, pos.volume_ahead);
//!     }
//! }
//! ```

use crate::types::{Action, MboMessage, Side};
use ahash::AHashMap;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for queue position tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuePositionConfig {
    /// Maximum number of price levels to track per side.
    ///
    /// Tracking deep levels uses more memory but provides more information.
    /// Default: 10 (matches typical LOB depth)
    pub max_levels_per_side: usize,

    /// Whether to track queue position changes as events.
    ///
    /// Enables `recent_position_changes()` queries.
    /// Default: false (saves memory)
    pub track_position_changes: bool,

    /// Maximum position changes to retain.
    ///
    /// Only used if `track_position_changes` is true.
    /// Default: 1000
    pub max_position_changes: usize,
}

impl Default for QueuePositionConfig {
    fn default() -> Self {
        Self {
            max_levels_per_side: 10,
            track_position_changes: false,
            max_position_changes: 1000,
        }
    }
}

impl QueuePositionConfig {
    /// Create with custom depth.
    pub fn with_depth(mut self, depth: usize) -> Self {
        self.max_levels_per_side = depth;
        self
    }

    /// Enable position change tracking.
    pub fn with_change_tracking(mut self) -> Self {
        self.track_position_changes = true;
        self
    }

    /// Research preset (full tracking).
    pub fn research() -> Self {
        Self {
            max_levels_per_side: 20,
            track_position_changes: true,
            max_position_changes: 10_000,
        }
    }
}

// ============================================================================
// Queue Position Info
// ============================================================================

/// Position information for a single order.
#[derive(Debug, Clone, Copy)]
pub struct QueuePositionInfo {
    /// Order ID.
    pub order_id: u64,

    /// Price level.
    pub price: i64,

    /// Side (Bid or Ask).
    pub side: Side,

    /// Queue position (0 = front of queue, first to fill).
    pub position: usize,

    /// Total volume ahead of this order in the queue.
    pub volume_ahead: u64,

    /// Total volume at this price level (including this order).
    pub total_level_volume: u64,

    /// This order's size.
    pub order_size: u32,

    /// Number of orders in the queue.
    pub queue_length: usize,
}

impl QueuePositionInfo {
    /// Calculate position as percentage of queue (0.0 = front, 1.0 = back).
    pub fn position_percentile(&self) -> f64 {
        if self.queue_length <= 1 {
            return 0.0;
        }
        self.position as f64 / (self.queue_length - 1) as f64
    }

    /// Calculate volume ahead as percentage of total level volume.
    pub fn volume_ahead_percentile(&self) -> f64 {
        if self.total_level_volume == 0 {
            return 0.0;
        }
        self.volume_ahead as f64 / self.total_level_volume as f64
    }

    /// Estimate probability of execution (simple model: inversely proportional to position).
    ///
    /// Note: This is a simplified model. Real execution probability depends on
    /// order flow dynamics, not just queue position.
    pub fn simple_execution_prob(&self) -> f64 {
        if self.queue_length == 0 {
            return 0.0;
        }
        1.0 - self.position_percentile()
    }
}

// ============================================================================
// Position Change Event
// ============================================================================

/// A change in an order's queue position.
#[derive(Debug, Clone)]
pub struct PositionChange {
    /// Order ID affected.
    pub order_id: u64,

    /// Timestamp of the change.
    pub timestamp: i64,

    /// Old position (None if order was just added).
    pub old_position: Option<usize>,

    /// New position (None if order was removed).
    pub new_position: Option<usize>,

    /// Change in volume ahead.
    pub volume_ahead_change: i64,

    /// Reason for the position change.
    pub reason: PositionChangeReason,
}

/// Why the queue position changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionChangeReason {
    /// Order was added to queue.
    Added,
    /// Order was removed (cancelled or filled).
    Removed,
    /// Another order ahead was removed, improving our position.
    ImprovedByRemoval,
    /// Order was modified (price change moves to back of queue).
    ModifiedPrice,
}

// ============================================================================
// Queue Level (FIFO-ordered)
// ============================================================================

/// A single price level with FIFO-ordered queue.
///
/// Uses `IndexMap` to maintain insertion order while allowing O(1) lookups.
#[derive(Debug, Clone)]
struct QueueLevel {
    /// Orders in FIFO order: order_id → size
    orders: IndexMap<u64, u32>,
    /// Cached total volume.
    total_volume: u64,
}

impl QueueLevel {
    fn new() -> Self {
        Self {
            orders: IndexMap::new(),
            total_volume: 0,
        }
    }

    /// Add order to back of queue.
    fn add_order(&mut self, order_id: u64, size: u32) {
        // If order exists, remove it first (will be re-added at back)
        if let Some(old_size) = self.orders.shift_remove(&order_id) {
            self.total_volume = self.total_volume.saturating_sub(old_size as u64);
        }
        self.orders.insert(order_id, size);
        self.total_volume = self.total_volume.saturating_add(size as u64);
    }

    /// Remove order from queue.
    fn remove_order(&mut self, order_id: u64) -> Option<u32> {
        if let Some(size) = self.orders.shift_remove(&order_id) {
            self.total_volume = self.total_volume.saturating_sub(size as u64);
            Some(size)
        } else {
            None
        }
    }

    /// Reduce order size (partial fill/cancel).
    fn reduce_order(&mut self, order_id: u64, delta: u32) -> Option<u32> {
        if let Some(size) = self.orders.get_mut(&order_id) {
            let actual_reduction = delta.min(*size);
            *size = size.saturating_sub(delta);
            self.total_volume = self.total_volume.saturating_sub(actual_reduction as u64);
            Some(*size)
        } else {
            None
        }
    }

    /// Get queue position for an order (0 = front).
    fn get_position(&self, order_id: u64) -> Option<usize> {
        self.orders.get_index_of(&order_id)
    }

    /// Get volume ahead of an order.
    fn volume_ahead(&self, order_id: u64) -> Option<u64> {
        let position = self.orders.get_index_of(&order_id)?;
        let ahead: u64 = self
            .orders
            .values()
            .take(position)
            .map(|&s| s as u64)
            .sum();
        Some(ahead)
    }

    /// Get full position info for an order.
    fn get_position_info(&self, order_id: u64, price: i64, side: Side) -> Option<QueuePositionInfo> {
        let position = self.orders.get_index_of(&order_id)?;
        let order_size = *self.orders.get(&order_id)?;
        let volume_ahead: u64 = self
            .orders
            .values()
            .take(position)
            .map(|&s| s as u64)
            .sum();

        Some(QueuePositionInfo {
            order_id,
            price,
            side,
            position,
            volume_ahead,
            total_level_volume: self.total_volume,
            order_size,
            queue_length: self.orders.len(),
        })
    }

    fn len(&self) -> usize {
        self.orders.len()
    }

    fn is_empty(&self) -> bool {
        self.orders.is_empty()
    }
}

// ============================================================================
// Queue Position Tracker
// ============================================================================

/// Tracks FIFO queue positions for all orders.
pub struct QueuePositionTracker {
    config: QueuePositionConfig,

    /// Bid side: price → QueueLevel (sorted descending by price)
    bids: AHashMap<i64, QueueLevel>,

    /// Ask side: price → QueueLevel (sorted ascending by price)
    asks: AHashMap<i64, QueueLevel>,

    /// Order lookup: order_id → (side, price)
    order_locations: AHashMap<u64, (Side, i64)>,

    /// Recent position changes (if tracking enabled).
    position_changes: Vec<PositionChange>,

    /// Statistics
    stats: QueueStats,
}

/// Statistics about queue tracking.
#[derive(Debug, Clone, Default)]
pub struct QueueStats {
    /// Total orders tracked.
    pub orders_tracked: u64,
    /// Total orders removed.
    pub orders_removed: u64,
    /// Total position queries.
    pub position_queries: u64,
    /// Position changes recorded.
    pub position_changes_recorded: u64,
    /// Messages skipped (system messages).
    pub messages_skipped: u64,
}

impl QueuePositionTracker {
    /// Create a new queue position tracker.
    pub fn new(config: QueuePositionConfig) -> Self {
        Self {
            config,
            bids: AHashMap::new(),
            asks: AHashMap::new(),
            order_locations: AHashMap::new(),
            position_changes: Vec::new(),
            stats: QueueStats::default(),
        }
    }

    /// Process an MBO message to update queue positions.
    pub fn process_message(&mut self, msg: &MboMessage) {
        // Skip system messages
        if msg.order_id == 0 || msg.size == 0 || msg.price <= 0 {
            self.stats.messages_skipped += 1;
            return;
        }

        if msg.side == Side::None {
            self.stats.messages_skipped += 1;
            return;
        }

        match msg.action {
            Action::Add => self.handle_add(msg),
            Action::Modify => self.handle_modify(msg),
            Action::Cancel => self.handle_cancel(msg),
            Action::Trade | Action::Fill => self.handle_fill(msg),
            Action::Clear | Action::None => {
                self.stats.messages_skipped += 1;
            }
        }
    }

    fn handle_add(&mut self, msg: &MboMessage) {
        let levels = match msg.side {
            Side::Bid => &mut self.bids,
            Side::Ask => &mut self.asks,
            Side::None => return,
        };

        let level = levels.entry(msg.price).or_insert_with(QueueLevel::new);
        level.add_order(msg.order_id, msg.size);

        // Get new position before releasing borrow
        let new_position = level.get_position(msg.order_id);

        self.order_locations
            .insert(msg.order_id, (msg.side, msg.price));
        self.stats.orders_tracked += 1;

        if self.config.track_position_changes {
            self.record_change(PositionChange {
                order_id: msg.order_id,
                timestamp: msg.timestamp.unwrap_or(0),
                old_position: None,
                new_position,
                volume_ahead_change: 0,
                reason: PositionChangeReason::Added,
            });
        }
    }

    fn handle_modify(&mut self, msg: &MboMessage) {
        // Check if order exists
        if let Some(&(old_side, old_price)) = self.order_locations.get(&msg.order_id) {
            // Price change: remove from old location, add to new (back of queue)
            if old_price != msg.price || old_side != msg.side {
                // Remove from old location
                let old_levels = match old_side {
                    Side::Bid => &mut self.bids,
                    Side::Ask => &mut self.asks,
                    Side::None => return,
                };

                if let Some(level) = old_levels.get_mut(&old_price) {
                    level.remove_order(msg.order_id);
                    if level.is_empty() {
                        old_levels.remove(&old_price);
                    }
                }

                // Add to new location
                let new_levels = match msg.side {
                    Side::Bid => &mut self.bids,
                    Side::Ask => &mut self.asks,
                    Side::None => return,
                };

                let level = new_levels.entry(msg.price).or_insert_with(QueueLevel::new);
                level.add_order(msg.order_id, msg.size);

                // Get new position before releasing borrow
                let new_position = level.get_position(msg.order_id);

                self.order_locations
                    .insert(msg.order_id, (msg.side, msg.price));

                if self.config.track_position_changes {
                    self.record_change(PositionChange {
                        order_id: msg.order_id,
                        timestamp: msg.timestamp.unwrap_or(0),
                        old_position: Some(0), // Was somewhere
                        new_position,
                        volume_ahead_change: 0,
                        reason: PositionChangeReason::ModifiedPrice,
                    });
                }
            } else {
                // Same price: just update size (position unchanged)
                let levels = match msg.side {
                    Side::Bid => &mut self.bids,
                    Side::Ask => &mut self.asks,
                    Side::None => return,
                };

                if let Some(level) = levels.get_mut(&msg.price) {
                    // Remove and re-add to update size but keep position
                    if let Some(_old_size) = level.orders.get_mut(&msg.order_id) {
                        // Update size in place (preserves position)
                        let old_size = *level.orders.get(&msg.order_id).unwrap();
                        if msg.size > old_size {
                            level.total_volume += (msg.size - old_size) as u64;
                        } else {
                            level.total_volume =
                                level.total_volume.saturating_sub((old_size - msg.size) as u64);
                        }
                        *level.orders.get_mut(&msg.order_id).unwrap() = msg.size;
                    }
                }
            }
        } else {
            // Order not found: treat as add (pre-existing order)
            self.handle_add(msg);
        }
    }

    fn handle_cancel(&mut self, msg: &MboMessage) {
        if let Some(&(side, price)) = self.order_locations.get(&msg.order_id) {
            let levels = match side {
                Side::Bid => &mut self.bids,
                Side::Ask => &mut self.asks,
                Side::None => return,
            };

            if let Some(level) = levels.get_mut(&price) {
                let old_position = level.get_position(msg.order_id);
                level.remove_order(msg.order_id);

                if level.is_empty() {
                    levels.remove(&price);
                }

                self.order_locations.remove(&msg.order_id);
                self.stats.orders_removed += 1;

                if self.config.track_position_changes {
                    self.record_change(PositionChange {
                        order_id: msg.order_id,
                        timestamp: msg.timestamp.unwrap_or(0),
                        old_position,
                        new_position: None,
                        volume_ahead_change: 0,
                        reason: PositionChangeReason::Removed,
                    });
                }
            }
        }
    }

    fn handle_fill(&mut self, msg: &MboMessage) {
        if let Some(&(side, price)) = self.order_locations.get(&msg.order_id) {
            let levels = match side {
                Side::Bid => &mut self.bids,
                Side::Ask => &mut self.asks,
                Side::None => return,
            };

            if let Some(level) = levels.get_mut(&price) {
                let old_position = level.get_position(msg.order_id);

                // Get current size
                if let Some(&current_size) = level.orders.get(&msg.order_id) {
                    if msg.size >= current_size {
                        // Full fill: remove order
                        level.remove_order(msg.order_id);
                        self.order_locations.remove(&msg.order_id);
                        self.stats.orders_removed += 1;

                        if level.is_empty() {
                            levels.remove(&price);
                        }

                        if self.config.track_position_changes {
                            self.record_change(PositionChange {
                                order_id: msg.order_id,
                                timestamp: msg.timestamp.unwrap_or(0),
                                old_position,
                                new_position: None,
                                volume_ahead_change: 0,
                                reason: PositionChangeReason::Removed,
                            });
                        }
                    } else {
                        // Partial fill: reduce size (position unchanged)
                        level.reduce_order(msg.order_id, msg.size);
                    }
                }
            }
        }
    }

    fn record_change(&mut self, change: PositionChange) {
        self.position_changes.push(change);
        self.stats.position_changes_recorded += 1;

        // Trim if over limit
        if self.position_changes.len() > self.config.max_position_changes {
            self.position_changes.remove(0);
        }
    }

    // ========================================================================
    // Query Methods
    // ========================================================================

    /// Get queue position for a specific order.
    pub fn queue_position(&mut self, order_id: u64) -> Option<QueuePositionInfo> {
        self.stats.position_queries += 1;

        let &(side, price) = self.order_locations.get(&order_id)?;

        let levels = match side {
            Side::Bid => &self.bids,
            Side::Ask => &self.asks,
            Side::None => return None,
        };

        let level = levels.get(&price)?;
        level.get_position_info(order_id, price, side)
    }

    /// Get volume ahead for a specific order.
    pub fn volume_ahead(&self, order_id: u64) -> Option<u64> {
        let &(side, price) = self.order_locations.get(&order_id)?;

        let levels = match side {
            Side::Bid => &self.bids,
            Side::Ask => &self.asks,
            Side::None => return None,
        };

        let level = levels.get(&price)?;
        level.volume_ahead(order_id)
    }

    /// Get queue length at a specific price level.
    pub fn queue_length(&self, side: Side, price: i64) -> usize {
        let levels = match side {
            Side::Bid => &self.bids,
            Side::Ask => &self.asks,
            Side::None => return 0,
        };

        levels.get(&price).map(|l| l.len()).unwrap_or(0)
    }

    /// Get total volume at a specific price level.
    pub fn level_volume(&self, side: Side, price: i64) -> u64 {
        let levels = match side {
            Side::Bid => &self.bids,
            Side::Ask => &self.asks,
            Side::None => return 0,
        };

        levels.get(&price).map(|l| l.total_volume).unwrap_or(0)
    }

    /// Get recent position changes.
    pub fn recent_position_changes(&self) -> &[PositionChange] {
        &self.position_changes
    }

    /// Get statistics.
    pub fn stats(&self) -> &QueueStats {
        &self.stats
    }

    /// Get number of active orders being tracked.
    pub fn active_orders(&self) -> usize {
        self.order_locations.len()
    }

    /// Reset the tracker.
    pub fn reset(&mut self) {
        self.bids.clear();
        self.asks.clear();
        self.order_locations.clear();
        self.position_changes.clear();
        self.stats = QueueStats::default();
    }

    // ========================================================================
    // Aggregate Features
    // ========================================================================

    /// Calculate queue imbalance at best bid/ask.
    ///
    /// Returns (imbalance, best_bid_vol, best_ask_vol)
    /// Imbalance = (bid_vol - ask_vol) / (bid_vol + ask_vol)
    pub fn best_level_imbalance(&self) -> Option<(f64, u64, u64)> {
        // Find best bid (highest price)
        let best_bid_vol = self
            .bids
            .iter()
            .max_by_key(|(&price, _)| price)
            .map(|(_, level)| level.total_volume)
            .unwrap_or(0);

        // Find best ask (lowest price)
        let best_ask_vol = self
            .asks
            .iter()
            .min_by_key(|(&price, _)| price)
            .map(|(_, level)| level.total_volume)
            .unwrap_or(0);

        let total = best_bid_vol + best_ask_vol;
        if total == 0 {
            return None;
        }

        let imbalance = (best_bid_vol as f64 - best_ask_vol as f64) / total as f64;
        Some((imbalance, best_bid_vol, best_ask_vol))
    }

    /// Calculate multi-level queue imbalance.
    ///
    /// Aggregates volume across top N levels on each side.
    pub fn multi_level_imbalance(&self, levels: usize) -> Option<(f64, u64, u64)> {
        // Get top N bid levels (highest prices)
        let mut bid_prices: Vec<_> = self.bids.keys().copied().collect();
        bid_prices.sort_by(|a, b| b.cmp(a)); // Descending

        let bid_vol: u64 = bid_prices
            .iter()
            .take(levels)
            .filter_map(|p| self.bids.get(p))
            .map(|l| l.total_volume)
            .sum();

        // Get top N ask levels (lowest prices)
        let mut ask_prices: Vec<_> = self.asks.keys().copied().collect();
        ask_prices.sort(); // Ascending

        let ask_vol: u64 = ask_prices
            .iter()
            .take(levels)
            .filter_map(|p| self.asks.get(p))
            .map(|l| l.total_volume)
            .sum();

        let total = bid_vol + ask_vol;
        if total == 0 {
            return None;
        }

        let imbalance = (bid_vol as f64 - ask_vol as f64) / total as f64;
        Some((imbalance, bid_vol, ask_vol))
    }

    /// Get average queue position for all tracked orders on a side.
    pub fn average_queue_position(&self, side: Side) -> Option<f64> {
        let levels = match side {
            Side::Bid => &self.bids,
            Side::Ask => &self.asks,
            Side::None => return None,
        };

        let mut total_position = 0usize;
        let mut count = 0usize;

        for level in levels.values() {
            for (idx, _) in level.orders.iter().enumerate() {
                total_position += idx;
                count += 1;
            }
        }

        if count == 0 {
            return None;
        }

        Some(total_position as f64 / count as f64)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_msg(
        order_id: u64,
        action: Action,
        side: Side,
        price: i64,
        size: u32,
        ts: i64,
    ) -> MboMessage {
        MboMessage {
            order_id,
            action,
            side,
            price,
            size,
            timestamp: Some(ts),
        }
    }

    // ------------------------------------------------------------------------
    // Basic Queue Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_add_single_order() {
        let mut tracker = QueuePositionTracker::new(QueuePositionConfig::default());

        let msg = make_msg(1, Action::Add, Side::Bid, 100_000_000_000, 100, 1000);
        tracker.process_message(&msg);

        let pos = tracker.queue_position(1).unwrap();
        assert_eq!(pos.position, 0); // First in queue
        assert_eq!(pos.volume_ahead, 0);
        assert_eq!(pos.order_size, 100);
        assert_eq!(pos.queue_length, 1);
    }

    #[test]
    fn test_fifo_order() {
        let mut tracker = QueuePositionTracker::new(QueuePositionConfig::default());

        // Add 3 orders at same price - FIFO order
        tracker.process_message(&make_msg(1, Action::Add, Side::Bid, 100_000_000_000, 100, 1000));
        tracker.process_message(&make_msg(2, Action::Add, Side::Bid, 100_000_000_000, 200, 2000));
        tracker.process_message(&make_msg(3, Action::Add, Side::Bid, 100_000_000_000, 150, 3000));

        // Order 1: position 0 (front)
        let pos1 = tracker.queue_position(1).unwrap();
        assert_eq!(pos1.position, 0);
        assert_eq!(pos1.volume_ahead, 0);

        // Order 2: position 1
        let pos2 = tracker.queue_position(2).unwrap();
        assert_eq!(pos2.position, 1);
        assert_eq!(pos2.volume_ahead, 100);

        // Order 3: position 2 (back)
        let pos3 = tracker.queue_position(3).unwrap();
        assert_eq!(pos3.position, 2);
        assert_eq!(pos3.volume_ahead, 300);
    }

    #[test]
    fn test_remove_front_improves_positions() {
        let mut tracker = QueuePositionTracker::new(QueuePositionConfig::default());

        // Add 3 orders
        tracker.process_message(&make_msg(1, Action::Add, Side::Bid, 100_000_000_000, 100, 1000));
        tracker.process_message(&make_msg(2, Action::Add, Side::Bid, 100_000_000_000, 200, 2000));
        tracker.process_message(&make_msg(3, Action::Add, Side::Bid, 100_000_000_000, 150, 3000));

        // Remove order 1 (front)
        tracker.process_message(&make_msg(1, Action::Cancel, Side::Bid, 100_000_000_000, 100, 4000));

        // Order 2 should now be at front
        let pos2 = tracker.queue_position(2).unwrap();
        assert_eq!(pos2.position, 0);
        assert_eq!(pos2.volume_ahead, 0);

        // Order 3 should now be position 1
        let pos3 = tracker.queue_position(3).unwrap();
        assert_eq!(pos3.position, 1);
        assert_eq!(pos3.volume_ahead, 200);
    }

    #[test]
    fn test_partial_fill_reduces_volume() {
        let mut tracker = QueuePositionTracker::new(QueuePositionConfig::default());

        tracker.process_message(&make_msg(1, Action::Add, Side::Ask, 100_000_000_000, 100, 1000));
        tracker.process_message(&make_msg(2, Action::Add, Side::Ask, 100_000_000_000, 200, 2000));

        // Partial fill of order 1
        tracker.process_message(&make_msg(1, Action::Fill, Side::Ask, 100_000_000_000, 40, 3000));

        let pos1 = tracker.queue_position(1).unwrap();
        assert_eq!(pos1.order_size, 60); // 100 - 40
        assert_eq!(pos1.position, 0); // Still at front

        let pos2 = tracker.queue_position(2).unwrap();
        assert_eq!(pos2.volume_ahead, 60); // Reduced from 100
    }

    #[test]
    fn test_full_fill_removes_order() {
        let mut tracker = QueuePositionTracker::new(QueuePositionConfig::default());

        tracker.process_message(&make_msg(1, Action::Add, Side::Ask, 100_000_000_000, 100, 1000));
        tracker.process_message(&make_msg(2, Action::Add, Side::Ask, 100_000_000_000, 200, 2000));

        // Full fill of order 1
        tracker.process_message(&make_msg(1, Action::Fill, Side::Ask, 100_000_000_000, 100, 3000));

        assert!(tracker.queue_position(1).is_none());

        let pos2 = tracker.queue_position(2).unwrap();
        assert_eq!(pos2.position, 0);
        assert_eq!(pos2.volume_ahead, 0);
    }

    #[test]
    fn test_price_change_moves_to_back() {
        let mut tracker = QueuePositionTracker::new(QueuePositionConfig::default());

        // Add 2 orders at same price
        tracker.process_message(&make_msg(1, Action::Add, Side::Bid, 100_000_000_000, 100, 1000));
        tracker.process_message(&make_msg(2, Action::Add, Side::Bid, 100_000_000_000, 200, 2000));

        // Modify order 1 to different price then back (simulates price improvement)
        // This should move it to back of queue at new price
        tracker.process_message(&make_msg(1, Action::Modify, Side::Bid, 101_000_000_000, 100, 3000));

        // Order 1 should be at new price, alone
        let pos1 = tracker.queue_position(1).unwrap();
        assert_eq!(pos1.price, 101_000_000_000);
        assert_eq!(pos1.position, 0);

        // Order 2 should now be alone at old price
        let pos2 = tracker.queue_position(2).unwrap();
        assert_eq!(pos2.price, 100_000_000_000);
        assert_eq!(pos2.position, 0);
    }

    // ------------------------------------------------------------------------
    // Queue Imbalance Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_best_level_imbalance() {
        let mut tracker = QueuePositionTracker::new(QueuePositionConfig::default());

        // Bid: 100 @ 100
        tracker.process_message(&make_msg(1, Action::Add, Side::Bid, 100_000_000_000, 100, 1000));
        // Ask: 200 @ 101
        tracker.process_message(&make_msg(2, Action::Add, Side::Ask, 101_000_000_000, 200, 2000));

        let (imbalance, bid_vol, ask_vol) = tracker.best_level_imbalance().unwrap();
        assert_eq!(bid_vol, 100);
        assert_eq!(ask_vol, 200);
        // (100 - 200) / 300 = -0.333...
        assert!((imbalance - (-1.0 / 3.0)).abs() < 0.001);
    }

    #[test]
    fn test_multi_level_imbalance() {
        let mut tracker = QueuePositionTracker::new(QueuePositionConfig::default());

        // 2 bid levels
        tracker.process_message(&make_msg(1, Action::Add, Side::Bid, 100_000_000_000, 100, 1000));
        tracker.process_message(&make_msg(2, Action::Add, Side::Bid, 99_000_000_000, 150, 2000));

        // 2 ask levels
        tracker.process_message(&make_msg(3, Action::Add, Side::Ask, 101_000_000_000, 50, 3000));
        tracker.process_message(&make_msg(4, Action::Add, Side::Ask, 102_000_000_000, 100, 4000));

        let (imbalance, bid_vol, ask_vol) = tracker.multi_level_imbalance(2).unwrap();
        assert_eq!(bid_vol, 250); // 100 + 150
        assert_eq!(ask_vol, 150); // 50 + 100
        // (250 - 150) / 400 = 0.25
        assert!((imbalance - 0.25).abs() < 0.001);
    }

    // ------------------------------------------------------------------------
    // Position Info Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_position_percentile() {
        let mut tracker = QueuePositionTracker::new(QueuePositionConfig::default());

        // Add 5 orders
        for i in 1..=5 {
            tracker.process_message(&make_msg(
                i,
                Action::Add,
                Side::Bid,
                100_000_000_000,
                100,
                i as i64 * 1000,
            ));
        }

        let pos1 = tracker.queue_position(1).unwrap();
        assert!((pos1.position_percentile() - 0.0).abs() < 0.001); // Front

        let pos3 = tracker.queue_position(3).unwrap();
        assert!((pos3.position_percentile() - 0.5).abs() < 0.001); // Middle

        let pos5 = tracker.queue_position(5).unwrap();
        assert!((pos5.position_percentile() - 1.0).abs() < 0.001); // Back
    }

    #[test]
    fn test_volume_ahead_percentile() {
        let mut tracker = QueuePositionTracker::new(QueuePositionConfig::default());

        // Add orders with different sizes
        tracker.process_message(&make_msg(1, Action::Add, Side::Bid, 100_000_000_000, 100, 1000));
        tracker.process_message(&make_msg(2, Action::Add, Side::Bid, 100_000_000_000, 300, 2000));
        tracker.process_message(&make_msg(3, Action::Add, Side::Bid, 100_000_000_000, 100, 3000));

        let pos3 = tracker.queue_position(3).unwrap();
        // Volume ahead: 400, total: 500
        assert!((pos3.volume_ahead_percentile() - 0.8).abs() < 0.001);
    }

    // ------------------------------------------------------------------------
    // System Message Handling
    // ------------------------------------------------------------------------

    #[test]
    fn test_system_messages_skipped() {
        let mut tracker = QueuePositionTracker::new(QueuePositionConfig::default());

        tracker.process_message(&make_msg(0, Action::Add, Side::Bid, 100_000_000_000, 100, 1000));
        tracker.process_message(&make_msg(1, Action::Add, Side::Bid, 100_000_000_000, 0, 2000));
        tracker.process_message(&make_msg(1, Action::Add, Side::Bid, 0, 100, 3000));
        tracker.process_message(&make_msg(1, Action::Add, Side::None, 100_000_000_000, 100, 4000));

        assert_eq!(tracker.stats().messages_skipped, 4);
        assert_eq!(tracker.active_orders(), 0);
    }

    // ------------------------------------------------------------------------
    // Reset Test
    // ------------------------------------------------------------------------

    #[test]
    fn test_reset() {
        let mut tracker = QueuePositionTracker::new(QueuePositionConfig::default());

        tracker.process_message(&make_msg(1, Action::Add, Side::Bid, 100_000_000_000, 100, 1000));
        assert_eq!(tracker.active_orders(), 1);

        tracker.reset();

        assert_eq!(tracker.active_orders(), 0);
        assert!(tracker.queue_position(1).is_none());
    }

    // ------------------------------------------------------------------------
    // Position Change Tracking Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_position_change_tracking() {
        let config = QueuePositionConfig::default().with_change_tracking();
        let mut tracker = QueuePositionTracker::new(config);

        tracker.process_message(&make_msg(1, Action::Add, Side::Bid, 100_000_000_000, 100, 1000));
        tracker.process_message(&make_msg(1, Action::Cancel, Side::Bid, 100_000_000_000, 100, 2000));

        let changes = tracker.recent_position_changes();
        assert_eq!(changes.len(), 2);

        assert_eq!(changes[0].reason, PositionChangeReason::Added);
        assert_eq!(changes[0].old_position, None);
        assert_eq!(changes[0].new_position, Some(0));

        assert_eq!(changes[1].reason, PositionChangeReason::Removed);
        assert!(changes[1].old_position.is_some());
        assert_eq!(changes[1].new_position, None);
    }
}

