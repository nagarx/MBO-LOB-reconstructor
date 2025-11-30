//! Advanced analytics for LOB data.
//!
//! This module provides sophisticated analytics for market microstructure analysis,
//! inspired by OrderBook-rs but adapted for ML preprocessing use cases.
//!
//! # Key Features
//!
//! - **DepthStats**: Per-side statistics (volume, VWAP, size distribution)
//! - **MarketImpact**: Simulate order execution impact
//! - **LiquidityMetrics**: Comprehensive liquidity analysis
//!
//! # Usage
//!
//! ```ignore
//! use mbo_lob_reconstructor::analytics::{DepthStats, MarketImpact};
//!
//! // Get depth statistics for bid side
//! let bid_stats = DepthStats::from_lob_state(&state, Side::Bid);
//! println!("Bid VWAP: ${:.4}", bid_stats.weighted_avg_price);
//!
//! // Simulate market impact of a large buy order
//! let impact = MarketImpact::simulate_buy(&state, 10_000);
//! println!("Slippage: {:.2} bps", impact.slippage_bps);
//! ```

use serde::{Deserialize, Serialize};
use crate::types::{LobState, Side};

// ============================================================================
// Depth Statistics
// ============================================================================

/// Comprehensive statistics for one side of the order book.
///
/// Provides detailed metrics about liquidity distribution and depth
/// characteristics, useful for:
/// - Market making decisions
/// - Order sizing
/// - Liquidity analysis
/// - ML feature engineering
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DepthStats {
    /// Which side of the book (Bid or Ask)
    pub side: Side,
    
    /// Total volume across all analyzed levels
    pub total_volume: u64,
    
    /// Number of active price levels (with non-zero size)
    pub levels_count: usize,
    
    /// Average size per level
    pub avg_level_size: f64,
    
    /// Volume-weighted average price (VWAP)
    pub weighted_avg_price: f64,
    
    /// Smallest level size found
    pub min_level_size: u64,
    
    /// Largest level size found
    pub max_level_size: u64,
    
    /// Standard deviation of level sizes
    pub std_dev_level_size: f64,
    
    /// Best price on this side
    pub best_price: Option<f64>,
    
    /// Worst price on this side (furthest from mid)
    pub worst_price: Option<f64>,
    
    /// Price range (worst - best) in dollars
    pub price_range: f64,
    
    /// Concentration ratio: volume at best level / total volume
    pub concentration_ratio: f64,
}

impl DepthStats {
    /// Create empty depth statistics.
    pub fn empty(side: Side) -> Self {
        Self {
            side,
            total_volume: 0,
            levels_count: 0,
            avg_level_size: 0.0,
            weighted_avg_price: 0.0,
            min_level_size: 0,
            max_level_size: 0,
            std_dev_level_size: 0.0,
            best_price: None,
            worst_price: None,
            price_range: 0.0,
            concentration_ratio: 0.0,
        }
    }

    /// Compute depth statistics from a LOB state for the specified side.
    ///
    /// # Arguments
    /// * `state` - The LOB state snapshot
    /// * `side` - Which side to analyze (Bid or Ask)
    ///
    /// # Returns
    /// Depth statistics for the specified side
    pub fn from_lob_state(state: &LobState, side: Side) -> Self {
        let (prices, sizes) = match side {
            Side::Bid => (&state.bid_prices, &state.bid_sizes),
            Side::Ask => (&state.ask_prices, &state.ask_sizes),
            Side::None => return Self::empty(side),
        };

        // Collect active levels (non-zero size)
        let active_levels: Vec<(f64, u64)> = prices
            .iter()
            .zip(sizes.iter())
            .filter(|(&price, &size)| price > 0 && size > 0)
            .map(|(&price, &size)| (price as f64 / 1e9, size as u64))
            .collect();

        if active_levels.is_empty() {
            return Self::empty(side);
        }

        let levels_count = active_levels.len();
        let sizes_only: Vec<u64> = active_levels.iter().map(|(_, s)| *s).collect();
        
        // Total volume
        let total_volume: u64 = sizes_only.iter().sum();
        
        // Average level size
        let avg_level_size = total_volume as f64 / levels_count as f64;
        
        // Min/max level size
        let min_level_size = *sizes_only.iter().min().unwrap_or(&0);
        let max_level_size = *sizes_only.iter().max().unwrap_or(&0);
        
        // Standard deviation of level sizes
        let variance: f64 = sizes_only
            .iter()
            .map(|&s| {
                let diff = s as f64 - avg_level_size;
                diff * diff
            })
            .sum::<f64>() / levels_count as f64;
        let std_dev_level_size = variance.sqrt();
        
        // VWAP (volume-weighted average price)
        let total_value: f64 = active_levels
            .iter()
            .map(|(price, size)| price * (*size as f64))
            .sum();
        let weighted_avg_price = if total_volume > 0 {
            total_value / total_volume as f64
        } else {
            0.0
        };
        
        // Best and worst prices
        let best_price = active_levels.first().map(|(p, _)| *p);
        let worst_price = active_levels.last().map(|(p, _)| *p);
        
        // Price range
        let price_range = match (best_price, worst_price) {
            (Some(best), Some(worst)) => (worst - best).abs(),
            _ => 0.0,
        };
        
        // Concentration ratio (volume at best level / total volume)
        let best_level_volume = active_levels.first().map(|(_, s)| *s).unwrap_or(0);
        let concentration_ratio = if total_volume > 0 {
            best_level_volume as f64 / total_volume as f64
        } else {
            0.0
        };

        Self {
            side,
            total_volume,
            levels_count,
            avg_level_size,
            weighted_avg_price,
            min_level_size,
            max_level_size,
            std_dev_level_size,
            best_price,
            worst_price,
            price_range,
            concentration_ratio,
        }
    }

    /// Check if statistics represent an empty order book side.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.levels_count == 0 || self.total_volume == 0
    }
    
    /// Get the coefficient of variation (std_dev / mean) for level sizes.
    /// 
    /// Higher values indicate more uneven liquidity distribution.
    #[inline]
    pub fn size_coefficient_of_variation(&self) -> f64 {
        if self.avg_level_size > 0.0 {
            self.std_dev_level_size / self.avg_level_size
        } else {
            0.0
        }
    }
}

// ============================================================================
// Market Impact Analysis
// ============================================================================

/// Market impact analysis for a hypothetical order.
///
/// Simulates what would happen if you tried to execute a large order,
/// providing metrics about:
/// - Expected average execution price
/// - Slippage from best price
/// - Number of levels consumed
/// - Available liquidity
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarketImpact {
    /// Side of the order (Buy = takes from Ask, Sell = takes from Bid)
    pub side: Side,
    
    /// Requested order quantity
    pub requested_quantity: u64,
    
    /// Average execution price across all fills (in dollars)
    pub avg_price: f64,
    
    /// Best available price before execution (in dollars)
    pub best_price: f64,
    
    /// Worst (furthest) execution price (in dollars)
    pub worst_price: f64,
    
    /// Absolute slippage from best price (in dollars)
    pub slippage: f64,
    
    /// Slippage in basis points
    pub slippage_bps: f64,
    
    /// Number of price levels that would be consumed
    pub levels_consumed: usize,
    
    /// Total quantity available to fill the order
    pub total_quantity_available: u64,
    
    /// Quantity that would be filled
    pub filled_quantity: u64,
    
    /// Quantity that could not be filled (insufficient liquidity)
    pub unfilled_quantity: u64,
    
    /// Individual fills as (price_dollars, quantity) pairs
    pub fills: Vec<(f64, u64)>,
}

impl MarketImpact {
    /// Create an empty market impact result.
    pub fn empty(side: Side) -> Self {
        Self {
            side,
            requested_quantity: 0,
            avg_price: 0.0,
            best_price: 0.0,
            worst_price: 0.0,
            slippage: 0.0,
            slippage_bps: 0.0,
            levels_consumed: 0,
            total_quantity_available: 0,
            filled_quantity: 0,
            unfilled_quantity: 0,
            fills: Vec::new(),
        }
    }

    /// Simulate a BUY order (takes from ASK side).
    ///
    /// # Arguments
    /// * `state` - Current LOB state
    /// * `quantity` - Number of shares to buy
    ///
    /// # Returns
    /// Market impact analysis for the buy order
    pub fn simulate_buy(state: &LobState, quantity: u64) -> Self {
        Self::simulate(state, Side::Ask, quantity)
    }

    /// Simulate a SELL order (takes from BID side).
    ///
    /// # Arguments
    /// * `state` - Current LOB state
    /// * `quantity` - Number of shares to sell
    ///
    /// # Returns
    /// Market impact analysis for the sell order
    pub fn simulate_sell(state: &LobState, quantity: u64) -> Self {
        Self::simulate(state, Side::Bid, quantity)
    }

    /// Simulate order execution against a specific side.
    fn simulate(state: &LobState, take_from_side: Side, quantity: u64) -> Self {
        let (prices, sizes) = match take_from_side {
            Side::Ask => (&state.ask_prices, &state.ask_sizes),
            Side::Bid => (&state.bid_prices, &state.bid_sizes),
            Side::None => return Self::empty(take_from_side),
        };

        let mut remaining = quantity;
        let mut fills: Vec<(f64, u64)> = Vec::new();
        let mut total_cost = 0.0f64;
        let mut total_filled = 0u64;
        let mut levels_consumed = 0usize;
        let mut worst_price = 0.0f64;
        let mut best_price = 0.0f64;
        let mut total_available = 0u64;

        // Iterate through price levels
        for (i, (&price_raw, &size)) in prices.iter().zip(sizes.iter()).enumerate() {
            if price_raw <= 0 || size == 0 {
                continue;
            }

            let price = price_raw as f64 / 1e9;
            total_available += size as u64;

            // Track best price (first valid level)
            if i == 0 || best_price == 0.0 {
                best_price = price;
            }

            if remaining == 0 {
                continue; // Still count total available
            }

            // Calculate fill at this level
            let fill_qty = remaining.min(size as u64);
            fills.push((price, fill_qty));
            total_cost += price * fill_qty as f64;
            total_filled += fill_qty;
            remaining -= fill_qty;
            levels_consumed += 1;
            worst_price = price;
        }

        // Calculate metrics
        let avg_price = if total_filled > 0 {
            total_cost / total_filled as f64
        } else {
            0.0
        };

        let slippage = if best_price > 0.0 {
            (worst_price - best_price).abs()
        } else {
            0.0
        };

        let slippage_bps = if best_price > 0.0 {
            (slippage / best_price) * 10_000.0
        } else {
            0.0
        };

        Self {
            side: take_from_side,
            requested_quantity: quantity,
            avg_price,
            best_price,
            worst_price,
            slippage,
            slippage_bps,
            levels_consumed,
            total_quantity_available: total_available,
            filled_quantity: total_filled,
            unfilled_quantity: remaining,
            fills,
        }
    }

    /// Check if the order can be fully filled.
    #[inline]
    pub fn can_fill(&self) -> bool {
        self.unfilled_quantity == 0
    }

    /// Get the fill ratio (0.0 to 1.0).
    #[inline]
    pub fn fill_ratio(&self) -> f64 {
        if self.requested_quantity == 0 {
            return 0.0;
        }
        self.filled_quantity as f64 / self.requested_quantity as f64
    }

    /// Get the total cost of the simulated order.
    #[inline]
    pub fn total_cost(&self) -> f64 {
        self.fills
            .iter()
            .map(|(price, qty)| price * (*qty as f64))
            .sum()
    }

    /// Get the price improvement (positive) or worsening (negative) vs mid-price.
    pub fn price_improvement_vs_mid(&self, mid_price: f64) -> f64 {
        if self.avg_price == 0.0 || mid_price == 0.0 {
            return 0.0;
        }
        
        match self.side {
            Side::Ask => mid_price - self.avg_price, // Buy: want lower price
            Side::Bid => self.avg_price - mid_price, // Sell: want higher price
            Side::None => 0.0,
        }
    }
}

// ============================================================================
// Liquidity Metrics (Combined Analysis)
// ============================================================================

/// Comprehensive liquidity metrics combining both sides of the book.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiquidityMetrics {
    /// Bid side depth statistics
    pub bid_depth: DepthStats,
    
    /// Ask side depth statistics
    pub ask_depth: DepthStats,
    
    /// Total volume on both sides
    pub total_volume: u64,
    
    /// Volume imbalance: (bid - ask) / (bid + ask)
    pub volume_imbalance: f64,
    
    /// Spread in dollars
    pub spread: f64,
    
    /// Spread in basis points
    pub spread_bps: f64,
    
    /// Mid-price in dollars
    pub mid_price: f64,
    
    /// Microprice (volume-weighted mid)
    pub microprice: f64,
    
    /// Average depth per level (both sides)
    pub avg_depth_per_level: f64,
    
    /// Total active levels (both sides)
    pub total_levels: usize,
}

impl LiquidityMetrics {
    /// Compute comprehensive liquidity metrics from a LOB state.
    pub fn from_lob_state(state: &LobState) -> Self {
        let bid_depth = DepthStats::from_lob_state(state, Side::Bid);
        let ask_depth = DepthStats::from_lob_state(state, Side::Ask);
        
        let total_volume = bid_depth.total_volume + ask_depth.total_volume;
        let total_levels = bid_depth.levels_count + ask_depth.levels_count;
        
        let volume_imbalance = if total_volume > 0 {
            (bid_depth.total_volume as f64 - ask_depth.total_volume as f64) / total_volume as f64
        } else {
            0.0
        };
        
        let avg_depth_per_level = if total_levels > 0 {
            total_volume as f64 / total_levels as f64
        } else {
            0.0
        };
        
        let spread = state.spread().unwrap_or(0.0);
        let mid_price = state.mid_price().unwrap_or(0.0);
        let spread_bps = state.spread_bps().unwrap_or(0.0);
        let microprice = state.microprice().unwrap_or(mid_price);
        
        Self {
            bid_depth,
            ask_depth,
            total_volume,
            volume_imbalance,
            spread,
            spread_bps,
            mid_price,
            microprice,
            avg_depth_per_level,
            total_levels,
        }
    }

    /// Check if the market is liquid (has quotes on both sides).
    #[inline]
    pub fn is_liquid(&self) -> bool {
        !self.bid_depth.is_empty() && !self.ask_depth.is_empty()
    }

    /// Get the book pressure indicator.
    #[inline]
    pub fn book_pressure(&self) -> f64 {
        self.volume_imbalance
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_state() -> LobState {
        let mut state = LobState::new(5);
        state.bid_prices = vec![100_000_000_000, 99_990_000_000, 99_980_000_000, 0, 0];
        state.bid_sizes = vec![100, 200, 150, 0, 0];
        state.best_bid = Some(100_000_000_000);
        state.ask_prices = vec![100_010_000_000, 100_020_000_000, 100_030_000_000, 0, 0];
        state.ask_sizes = vec![150, 100, 200, 0, 0];
        state.best_ask = Some(100_010_000_000);
        state
    }

    #[test]
    fn test_depth_stats_from_lob_state() {
        let state = create_test_state();
        let stats = DepthStats::from_lob_state(&state, Side::Bid);
        assert_eq!(stats.total_volume, 450);
        assert_eq!(stats.levels_count, 3);
    }

    #[test]
    fn test_market_impact_simulate_buy() {
        let state = create_test_state();
        let impact = MarketImpact::simulate_buy(&state, 200);
        assert!(impact.can_fill());
        assert_eq!(impact.levels_consumed, 2);
    }

    #[test]
    fn test_liquidity_metrics() {
        let state = create_test_state();
        let metrics = LiquidityMetrics::from_lob_state(&state);
        assert!(metrics.is_liquid());
        assert_eq!(metrics.total_volume, 900);
    }
}

