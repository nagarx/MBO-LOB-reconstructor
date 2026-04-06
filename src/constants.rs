//! Domain constants for the MBO-LOB reconstruction pipeline.
//!
//! Centralizes numeric constants that appear throughout the codebase.
//! All constants include source citations per project Rule 4.

// =============================================================================
// Price Unit Conversion
// =============================================================================

/// Nanodollars per dollar (integer form, for integer arithmetic).
///
/// Databento MBO Schema v0.20: the `price` field is a signed 64-bit integer
/// with a fixed scale factor of 1/1,000,000,000 (1e-9).
/// To convert: `dollars = price_i64 / NANODOLLARS_PER_DOLLAR`.
pub const NANODOLLARS_PER_DOLLAR: i64 = 1_000_000_000;

/// Nanodollars per dollar (f64 form, for hot-path floating-point division).
///
/// Use this for `price as f64 / NANODOLLARS_PER_DOLLAR_F64` conversions.
/// Exact in IEEE 754 (1e9 < 2^53).
pub const NANODOLLARS_PER_DOLLAR_F64: f64 = 1_000_000_000.0;

// =============================================================================
// Financial Constants
// =============================================================================

/// Multiplier to convert a ratio to basis points.
///
/// 1 basis point = 0.01% = 1/10,000. Standard financial convention.
/// Usage: `bps = (spread / mid_price) * BASIS_POINTS_PER_UNIT`
pub const BASIS_POINTS_PER_UNIT: f64 = 10_000.0;

// =============================================================================
// Numerical Precision
// =============================================================================

/// Division guard epsilon for ratio calculations.
///
/// Chosen as ~sqrt(f64::EPSILON) ~ 1.49e-8, rounded to 1e-8.
/// Large enough to guard near-zero denominators, small enough not to
/// distort results at typical HFT price/size magnitudes.
///
/// Matches `feature-extractor-MBO-LOB/src/contract.rs::DIVISION_GUARD_EPS`.
pub const DIVISION_GUARD_EPS: f64 = 1e-8;

// =============================================================================
// Time Unit Conversion
// =============================================================================

/// Nanoseconds per millisecond.
pub const NS_PER_MILLISECOND: i64 = 1_000_000;

/// Nanoseconds per second.
///
/// Databento timestamps are nanoseconds since Unix epoch (i64).
pub const NS_PER_SECOND: i64 = 1_000_000_000;

/// Nanoseconds per second (f64 form, for rate calculations in hot paths).
///
/// Exact in IEEE 754 (1e9 < 2^53).
pub const NS_PER_SECOND_F64: f64 = 1_000_000_000.0;

/// Nanoseconds per minute.
pub const NS_PER_MINUTE: i64 = 60 * NS_PER_SECOND;

/// Nanoseconds per hour.
pub const NS_PER_HOUR: i64 = 3_600 * NS_PER_SECOND;

/// Nanoseconds per day.
pub const NS_PER_DAY: i64 = 86_400 * NS_PER_SECOND;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_i64_f64_consistency() {
        // Verify the i64 and f64 forms are exactly equal (both < 2^53).
        assert_eq!(
            NANODOLLARS_PER_DOLLAR as f64, NANODOLLARS_PER_DOLLAR_F64,
            "NANODOLLARS_PER_DOLLAR i64->f64 must equal NANODOLLARS_PER_DOLLAR_F64"
        );
        assert_eq!(
            NS_PER_SECOND as f64, NS_PER_SECOND_F64,
            "NS_PER_SECOND i64->f64 must equal NS_PER_SECOND_F64"
        );
    }

    #[test]
    fn test_time_constant_relationships() {
        assert_eq!(NS_PER_MINUTE, 60 * NS_PER_SECOND);
        assert_eq!(NS_PER_HOUR, 3_600 * NS_PER_SECOND);
        assert_eq!(NS_PER_DAY, 86_400 * NS_PER_SECOND);
        assert_eq!(NS_PER_MILLISECOND, NS_PER_SECOND / 1_000);
    }

    #[test]
    fn test_basis_points_conversion() {
        // 1% = 100 bps
        assert_eq!(0.01 * BASIS_POINTS_PER_UNIT, 100.0);
        // 1 bp = 0.01%
        assert_eq!(1.0 / BASIS_POINTS_PER_UNIT, 0.0001);
    }
}
