//! Price level with cached aggregate size.
//!
//! This module provides a `PriceLevel` struct that maintains both individual
//! order sizes and a cached total size for O(1) aggregate queries.
//!
//! # Invariant
//!
//! The `total_size` field MUST always equal `orders.values().sum()`.
//! This invariant is enforced through encapsulated mutation methods and
//! verified in debug builds via `verify_invariant()`.
//!
//! # Performance
//!
//! | Operation | Complexity |
//! |-----------|------------|
//! | `add_order` | O(1) amortized |
//! | `remove_order` | O(1) amortized |
//! | `reduce_order` | O(1) |
//! | `total_size` | O(1) |
//! | `is_empty` | O(1) |

use ahash::AHashMap;

/// A price level in the order book with cached aggregate size.
#[derive(Debug, Clone)]
pub struct PriceLevel {
    /// Individual orders at this price level: order_id → size
    orders: AHashMap<u64, u32>,
    /// Cached total size (invariant: == orders.values().sum())
    total_size: u32,
}

impl Default for PriceLevel {
    fn default() -> Self {
        Self::new()
    }
}

impl PriceLevel {
    /// Create a new empty price level.
    #[inline]
    pub fn new() -> Self {
        Self {
            orders: AHashMap::new(),
            total_size: 0,
        }
    }

    /// Add or replace an order at this price level.
    #[inline]
    pub fn add_order(&mut self, order_id: u64, size: u32) -> Option<u32> {
        let old = self.orders.insert(order_id, size);
        
        if let Some(old_size) = old {
            if size >= old_size {
                self.total_size = self.total_size.saturating_add(size - old_size);
            } else {
                self.total_size = self.total_size.saturating_sub(old_size - size);
            }
        } else {
            self.total_size = self.total_size.saturating_add(size);
        }
        
        #[cfg(debug_assertions)]
        self.verify_invariant();
        
        old
    }

    /// Remove an order from this price level.
    #[inline]
    pub fn remove_order(&mut self, order_id: u64) -> Option<u32> {
        if let Some(size) = self.orders.remove(&order_id) {
            self.total_size = self.total_size.saturating_sub(size);
            
            #[cfg(debug_assertions)]
            self.verify_invariant();
            
            Some(size)
        } else {
            None
        }
    }

    /// Reduce an order's size (for partial cancels or fills).
    #[inline]
    pub fn reduce_order(&mut self, order_id: u64, delta: u32) -> Option<u32> {
        if let Some(size) = self.orders.get_mut(&order_id) {
            let actual_reduction = delta.min(*size);
            *size = size.saturating_sub(delta);
            self.total_size = self.total_size.saturating_sub(actual_reduction);
            let new_size = *size;
            
            #[cfg(debug_assertions)]
            self.verify_invariant();
            
            Some(new_size)
        } else {
            None
        }
    }

    /// Get the cached total size (O(1)).
    #[inline]
    pub fn total_size(&self) -> u32 {
        self.total_size
    }

    /// Check if the price level has no orders.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.orders.is_empty()
    }

    /// Get the number of orders at this price level.
    #[inline]
    pub fn order_count(&self) -> usize {
        self.orders.len()
    }

    /// Get an order's current size.
    #[inline]
    pub fn get(&self, order_id: &u64) -> Option<&u32> {
        self.orders.get(order_id)
    }

    /// Get a mutable reference to an order's size.
    /// WARNING: Direct mutation will NOT update total_size.
    #[inline]
    pub fn get_mut(&mut self, order_id: &u64) -> Option<&mut u32> {
        self.orders.get_mut(order_id)
    }

    /// Check if an order exists at this price level.
    #[inline]
    pub fn contains(&self, order_id: &u64) -> bool {
        self.orders.contains_key(order_id)
    }

    /// Clear all orders from this price level.
    #[inline]
    pub fn clear(&mut self) {
        self.orders.clear();
        self.total_size = 0;
    }

    /// Compute the actual total by summing all orders (O(n)).
    /// Uses saturating arithmetic to match cached total behavior.
    #[inline]
    pub fn compute_actual_total(&self) -> u32 {
        self.orders.values().fold(0u32, |acc, &v| acc.saturating_add(v))
    }

    /// Verify the size invariant holds.
    /// Note: Uses saturating arithmetic, so extreme values may still match
    /// even if individual orders would overflow when summed normally.
    #[cfg(debug_assertions)]
    #[inline]
    pub fn verify_invariant(&self) {
        let actual: u32 = self.compute_actual_total();
        debug_assert_eq!(
            actual, self.total_size,
            "PriceLevel invariant violated: actual={}, cached={}",
            actual, self.total_size
        );
    }

    #[cfg(not(debug_assertions))]
    #[inline]
    pub fn verify_invariant(&self) {}

    /// Iterate over all orders (order_id, size).
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = (&u64, &u32)> {
        self.orders.iter()
    }

    /// Get immutable access to the underlying orders map.
    #[inline]
    pub fn orders(&self) -> &AHashMap<u64, u32> {
        &self.orders
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_price_level_is_empty() {
        let level = PriceLevel::new();
        assert!(level.is_empty());
        assert_eq!(level.total_size(), 0);
        assert_eq!(level.order_count(), 0);
    }

    #[test]
    fn test_add_single_order() {
        let mut level = PriceLevel::new();
        let result = level.add_order(1001, 100);
        assert_eq!(result, None);
        assert_eq!(level.total_size(), 100);
        assert_eq!(level.order_count(), 1);
    }

    #[test]
    fn test_add_multiple_orders() {
        let mut level = PriceLevel::new();
        level.add_order(1, 100);
        level.add_order(2, 200);
        level.add_order(3, 150);
        assert_eq!(level.total_size(), 450);
        assert_eq!(level.order_count(), 3);
    }

    #[test]
    fn test_add_order_replace_larger() {
        let mut level = PriceLevel::new();
        level.add_order(1, 100);
        let old = level.add_order(1, 150);
        assert_eq!(old, Some(100));
        assert_eq!(level.total_size(), 150);
        assert_eq!(level.order_count(), 1);
    }

    #[test]
    fn test_add_order_replace_smaller() {
        let mut level = PriceLevel::new();
        level.add_order(1, 200);
        let old = level.add_order(1, 50);
        assert_eq!(old, Some(200));
        assert_eq!(level.total_size(), 50);
    }

    #[test]
    fn test_remove_existing_order() {
        let mut level = PriceLevel::new();
        level.add_order(1, 100);
        level.add_order(2, 200);
        let removed = level.remove_order(1);
        assert_eq!(removed, Some(100));
        assert_eq!(level.total_size(), 200);
    }

    #[test]
    fn test_remove_nonexistent_order() {
        let mut level = PriceLevel::new();
        level.add_order(1, 100);
        let removed = level.remove_order(999);
        assert_eq!(removed, None);
        assert_eq!(level.total_size(), 100);
    }

    #[test]
    fn test_reduce_order_partial() {
        let mut level = PriceLevel::new();
        level.add_order(1, 100);
        let new_size = level.reduce_order(1, 30);
        assert_eq!(new_size, Some(70));
        assert_eq!(level.total_size(), 70);
    }

    #[test]
    fn test_reduce_order_beyond_size() {
        let mut level = PriceLevel::new();
        level.add_order(1, 100);
        let new_size = level.reduce_order(1, 150);
        assert_eq!(new_size, Some(0));
        assert_eq!(level.total_size(), 0);
    }

    #[test]
    fn test_reduce_nonexistent_order() {
        let mut level = PriceLevel::new();
        level.add_order(1, 100);
        let result = level.reduce_order(999, 50);
        assert_eq!(result, None);
        assert_eq!(level.total_size(), 100);
    }

    #[test]
    fn test_clear() {
        let mut level = PriceLevel::new();
        level.add_order(1, 100);
        level.add_order(2, 200);
        level.clear();
        assert!(level.is_empty());
        assert_eq!(level.total_size(), 0);
    }

    #[test]
    fn test_compute_actual_matches_cached() {
        let mut level = PriceLevel::new();
        level.add_order(1, 100);
        level.add_order(2, 200);
        level.add_order(3, 50);
        assert_eq!(level.compute_actual_total(), level.total_size());
    }

    #[test]
    fn test_invariant_after_complex_operations() {
        let mut level = PriceLevel::new();
        level.add_order(1, 100);
        level.add_order(2, 200);
        level.add_order(3, 150);
        level.reduce_order(1, 30);
        level.remove_order(2);
        level.add_order(4, 75);
        level.add_order(1, 50); // Replace order 1
        assert_eq!(level.compute_actual_total(), level.total_size());
        level.verify_invariant();
    }

    #[test]
    fn test_overflow_protection() {
        let mut level = PriceLevel::new();
        level.add_order(1, u32::MAX);
        level.add_order(2, 1);
        assert_eq!(level.total_size(), u32::MAX); // Saturated
    }

    #[test]
    fn test_realistic_order_lifecycle() {
        let mut level = PriceLevel::new();
        level.add_order(1001, 500);
        assert_eq!(level.total_size(), 500);
        level.add_order(1002, 300);
        assert_eq!(level.total_size(), 800);
        level.reduce_order(1001, 100);
        assert_eq!(level.total_size(), 700);
        level.remove_order(1001);
        assert_eq!(level.total_size(), 300);
        level.remove_order(1002);
        assert_eq!(level.total_size(), 0);
        assert!(level.is_empty());
        level.verify_invariant();
    }

    #[test]
    fn test_stress_operations() {
        let mut level = PriceLevel::new();
        for i in 0..100 {
            level.add_order(i, (i as u32 + 1) * 10);
        }
        assert_eq!(level.total_size(), 50500);
        for i in (0..100).step_by(2) {
            level.remove_order(i);
        }
        assert_eq!(level.total_size(), 25500);
        level.verify_invariant();
    }
}

