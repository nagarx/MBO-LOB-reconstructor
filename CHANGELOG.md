# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Hot Store Infrastructure** (`src/hotstore.rs`)
  - `HotStoreConfig` - Configuration for hot store directory and preferences
  - `HotStoreManager` - Manages decompressed data cache
    - `resolve()` - Auto-prefer decompressed files when available
    - `decompress()` - Decompress single file to hot store
    - `list_hot_files()` - Enumerate cached files
    - `hot_store_size()` - Calculate total cache size
    - `clear()` - Remove all cached files
  - Enables ~30% faster processing by skipping zstd decompression

- **MarketDataSource Abstraction** (`src/source.rs`)
  - `MarketDataSource` trait - Provider-agnostic data source interface
  - `SourceMetadata` - Metadata about the data source (symbol, date, etc.)
  - `VecSource` - In-memory source for testing
  - `DbnSource` - DBN file source with hot store integration
    - `with_hot_store()` - Enable hot store path resolution

- **Auto-Detect DBN Compression**
  - `DbnLoader` now uses `DynDecoder` to auto-detect file format
  - Supports both compressed (`.dbn.zst`) and uncompressed (`.dbn`) files
  - No configuration needed - just provide the file path

- **CLI: decompress_to_hot_store** (`src/bin/decompress_to_hot_store.rs`)
  - Standalone tool to populate hot store directory
  - Parallel decompression using Rayon
  - Supports single file, directory, or glob patterns
  - `--dry-run` mode to preview operations
  - `--force` to re-decompress existing files

- **PriceLevel with Cached Size** (`src/lob/price_level.rs`)
  - `PriceLevel` struct with O(1) `total_size()` queries (was O(n))
  - Encapsulated mutation methods: `add_order()`, `remove_order()`, `reduce_order()`
  - Debug assertions verify cache consistency
  - 16 comprehensive unit tests

- **Zero-Allocation API**
  - `LobReconstructor::process_message_into()` - Fill pre-allocated `LobState`
  - Eliminates heap allocation per message in hot loop
  - `fill_lob_state()` helper for reusable state

- **Stack-Allocated LobState**
  - `LobState` fields changed from `Vec` to `[T; MAX_LOB_LEVELS]`
  - `MAX_LOB_LEVELS = 20` constant for fixed-size arrays
  - ~520 bytes per snapshot (fits in cache)

### Changed

- `DbnLoader` now accepts both compressed and uncompressed DBN files
- `DbnLoader` I/O buffer increased from 8KB to 1MB (`IO_BUFFER_SIZE`)
- `LobReconstructor` now uses `BTreeMap<i64, PriceLevel>` instead of `BTreeMap<i64, AHashMap<u64, u32>>`
- Size aggregation in `fill_lob_state()` now O(1) per level (was O(n))

### Performance

- **10.2 million messages/sec** throughput (release mode)
- **0.10 µs** per-message latency
- **~30% faster** with pre-decompressed files via hot store
- Validated against 37M+ real NVIDIA messages with 0 mismatches

## [0.1.1] - 2025-12-04

### Added

- **System Message Filtering**
  - `LobConfig::skip_system_messages` - Skip system messages (order_id=0, size=0, price<=0) by default
  - `LobStats::system_messages_skipped` - Track count of skipped system messages
  - `LobConfig::with_skip_system_messages(bool)` - Configure system message handling
  
- **Full Reset Method**
  - `LobReconstructor::full_reset()` - Completely reset reconstructor including statistics
  - Distinction: `reset()` preserves stats (for Action::Clear), `full_reset()` clears everything

### Fixed

- Collapsed nested if statement for system message check (clippy)

### Changed

- `reset()` now explicitly documents that it preserves statistics (for monitoring across Action::Clear)
- System messages are now filtered at the `LobReconstructor` level, not at the loader level

## [0.1.0] - 2025-12-01

### Added

#### Core LOB Reconstruction
- `LobReconstructor` - High-performance single-symbol LOB reconstruction
- `MultiSymbolLob` - Multi-symbol LOB management
- `LobConfig` - Configurable LOB behavior
- `CrossedQuotePolicy` - Four policies for handling crossed quotes (Allow, UseLastValid, Error, SkipUpdate)

#### Core Types
- `MboMessage` - Market-By-Order message representation
- `LobState` - LOB snapshot with enriched analytics
- `Action` - Order actions (Add, Modify, Cancel, Trade, Fill, Clear, None)
- `Side` - Order side (Bid, Ask, None)
- `BookConsistency` - Book state validation (Valid, Empty, Crossed, Locked)

#### Enriched Analytics on LobState
- `mid_price()` - Average of best bid and ask
- `spread()` / `spread_bps()` - Spread in dollars and basis points
- `microprice()` - Volume-weighted mid-price
- `vwap_bid(n)` / `vwap_ask(n)` - VWAP for top N levels
- `weighted_mid(n)` - VWAP-based mid-price
- `depth_imbalance()` - Normalized volume imbalance [-1, 1]
- `total_bid_volume()` / `total_ask_volume()` - Total volume per side
- `active_bid_levels()` / `active_ask_levels()` - Count of non-empty levels
- `check_consistency()` - Book state validation

#### Statistics for ML
- `RunningStats` - Online mean/std computation using Welford's algorithm
- `DayStats` - Per-day statistics tracking for all LOB metrics
- `NormalizationParams` - Z-score normalization parameters with save/load

#### Advanced Analytics
- `DepthStats` - Per-side depth statistics (volume, VWAP, concentration, price range)
- `MarketImpact` - Order execution simulation with slippage analysis
- `LiquidityMetrics` - Combined book analysis (spread, imbalance, pressure)

#### Databento Support (feature-gated)
- `DbnLoader` - Streaming loader for compressed DBN files
- `DbnBridge` - Databento MboMsg to internal MboMessage conversion
- `LoaderStats` - File loading statistics

#### Error Handling
- `TlobError` - Comprehensive error types
- `CrossedQuote` / `LockedQuote` - Specific errors for book consistency issues

### Performance
- ~974,000 messages/second throughput
- ~1 microsecond latency per message
- 100% data quality on real NVIDIA MBO data (17.8M messages)

### Testing
- 89+ unit tests
- 19+ integration tests with real market data
- 8+ edge case tests for robustness

[Unreleased]: https://github.com/nagarx/MBO-LOB-reconstructor/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/nagarx/MBO-LOB-reconstructor/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/nagarx/MBO-LOB-reconstructor/releases/tag/v0.1.0
