//! Core data types for MBO messages and LOB state.
//!
//! These types are designed to be:
//! - Memory efficient (use smallest types possible)
//! - Cache-friendly (aligned, packed where appropriate)
//! - Zero-copy where possible
//! - Compatible with Databento's MBO format
//!
//! # Performance Notes
//!
//! `LobState` uses fixed-size stack-allocated arrays instead of `Vec` to eliminate
//! heap allocations in the hot path. This provides significant throughput improvements
//! (~30-50% faster) for high-frequency LOB reconstruction.

use crate::constants::{BASIS_POINTS_PER_UNIT, NANODOLLARS_PER_DOLLAR_F64, NS_PER_SECOND_F64};
use serde::{Deserialize, Serialize};

/// Maximum supported LOB levels (compile-time constant).
///
/// This determines the size of stack-allocated arrays in `LobState`.
/// - Most research papers use 10 levels (DeepLOB, TLOB, FI-2010)
/// - 20 levels provides headroom for deeper analysis
/// - Total `LobState` size: 20 * (8 + 4 + 8 + 4) + metadata ≈ 520 bytes
///
/// # Rationale
///
/// Research shows that market microstructure signals decay rapidly beyond
/// the first few levels. 20 levels captures essentially all tradeable liquidity
/// while keeping the struct cache-friendly (fits in ~8 cache lines).
pub const MAX_LOB_LEVELS: usize = 20;

/// MBO action type (what happened to the order)
///
/// # ⚠ THE DISCRIMINANTS ARE A LIVE WIRE FORMAT — `#[repr(u8)]` IS LOAD-BEARING
///
/// [`Action::to_byte`] is literally `self as u8`, and that byte is written into the Parquet
/// `action` and `triggering_action` columns (`export/batch.rs`). Downstream consumers key on the
/// **literal ASCII values** (e.g. `MBO-LOB-analyzer`'s `ACTION_TRADE = 84`, `ACTION_FILL = 70`).
///
/// **The silent failure is dropping an explicit `= b'X'` discriminant** — that variant then takes
/// `previous + 1`, so the encoder writes `0..6` into the corpus and **every downstream mask
/// silently matches nothing, with no error raised anywhere.** Measured: removing all seven
/// discriminants makes `TradeAggregate` emit `3` and `Fill` emit `4`, and the export still exits
/// 0 with a structurally valid Parquet file.
///
/// **Dropping `#[repr(u8)]` is NOT the silent mode — it is a compile error.** The `= b'X'` syntax
/// requires a compatible `repr`; without it rustc rejects every variant with
/// `error[E0308]: mismatched types ... expected 'isize', found 'u8'`. Keep the attribute (the
/// discriminants cannot compile without it), but understand that the thing actually worth
/// guarding is the **discriminants**. `test_action_discriminants_are_ascii_wire_format` pins all
/// seven as literal integers, which is what makes the real failure mode loud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)] // ⚠ LOAD-BEARING WIRE FORMAT — DO NOT DROP. See the type docs above.
pub enum Action {
    /// Add new order to book
    Add = b'A',
    /// Modify existing order
    Modify = b'M',
    /// Cancel/remove order
    Cancel = b'C',
    /// The vendor's aggressing-order **TRADE PRINT**. `side` is the **AGGRESSOR's** side.
    /// Carries `order_id == 0` on XNAS.ITCH. **DOES NOT AFFECT THE BOOK.**
    ///
    /// ⚠ Not interchangeable with [`Action::Fill`]: the two carry **opposite** side conventions
    /// and are exact side-mirrors of one another, so merging them annihilates signed order flow.
    ///
    /// ⚠ **NOT** related to `TradeAggregator` or its `Trade` in `lob::trade_aggregator` (both
    /// UN-EXPORTED 2026-09-07 and no longer reachable from the crate root; the intra-doc links
    /// that stood here would now be broken). That type builds one `Trade` by *aggregating
    /// many* `Fill`s; this variant is **one vendor print per physical execution**.
    /// "TradeAggregate" here means *the vendor's aggregate trade print*, not *an aggregate of
    /// trades*.
    TradeAggregate = b'T',
    /// A fill against an **EXISTING RESTING** order. `side` is the **RESTING order's** side —
    /// the OPPOSITE convention from [`Action::TradeAggregate`]. Carries `order_id != 0`.
    /// **DOES NOT AFFECT THE BOOK** — a paired `Cancel` performs the removal.
    ///
    /// ⚠ This is NOT "an alternative trade representation". `TradeAggregate` and `Fill` are two
    /// views of the same physical execution from opposite sides; counting both double-counts.
    Fill = b'F',
    /// Clear/Reset the book
    Clear = b'R',
    /// No-op action (may carry flags or other information)
    None = b'N',
}

impl Action {
    /// Parse action from a byte (Databento format).
    ///
    /// This is the **canonical** vendor-byte → `Action` map for this crate.
    /// `DbnBridge::convert_action` delegates here rather than keeping a second copy: two maps
    /// meant the same byte-semantics question could be answered correctly in one and wrongly in
    /// the other, which is exactly how `b'F'` came to decode as a trade.
    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            b'A' => Some(Action::Add),
            b'M' => Some(Action::Modify),
            b'C' => Some(Action::Cancel),
            b'T' => Some(Action::TradeAggregate),
            b'F' => Some(Action::Fill),
            b'R' => Some(Action::Clear),
            b'N' => Some(Action::None),
            _ => Option::None,
        }
    }

    /// Convert to byte representation.
    ///
    /// ⚠ This is the wire-format encoder — see the type-level warning on [`Action`].
    pub fn to_byte(self) -> u8 {
        self as u8
    }
}

/// Order side (bid or ask)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Side {
    /// Buy order (bid)
    Bid = b'B',
    /// Sell order (ask)
    Ask = b'A',
    /// Non-directional (used for some trade types)
    None = b'N',
}

impl Side {
    /// Parse side from a byte.
    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            b'B' => Some(Side::Bid),
            b'A' => Some(Side::Ask),
            b'N' => Some(Side::None),
            _ => None,
        }
    }

    /// Convert to byte representation.
    pub fn to_byte(self) -> u8 {
        self as u8
    }

    /// Check if this is a bid.
    #[inline(always)]
    pub fn is_bid(self) -> bool {
        matches!(self, Side::Bid)
    }

    /// Check if this is an ask.
    #[inline(always)]
    pub fn is_ask(self) -> bool {
        matches!(self, Side::Ask)
    }
}

/// Market By Order (MBO) message.
///
/// This represents a single order book event. All fields use fixed-size types
/// for predictable memory layout and cache efficiency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MboMessage {
    /// Unique order identifier
    pub order_id: u64,

    /// Order action (add, modify, cancel, trade)
    pub action: Action,

    /// Order side (bid or ask)
    pub side: Side,

    /// Price in fixed-point format (divide by 1e9 for dollars)
    /// Using i64 to match Databento format
    pub price: i64,

    /// Order size in shares/contracts
    pub size: u32,

    /// Timestamp (nanoseconds since epoch)
    /// Optional - not always needed for LOB reconstruction
    pub timestamp: Option<i64>,

    /// The vendor's raw record-flag byte, carried verbatim from `dbn::MboMsg.flags`.
    ///
    /// Databento documents this field as "a bit field indicating event end, message
    /// characteristics, and **data quality**"
    /// ([`dbn::MboMsg::flags`], v0.64.0 `record.rs:91-93`). Until 2026-09-03 the
    /// decoder never read it, so the byte was destroyed at
    /// [`crate::DbnBridge::convert`] and no consumer downstream could recover it.
    ///
    /// # The bits, and what is actually set in this corpus
    ///
    /// Bit constants are `dbn::flags` (`flags.rs:10-23`). Measured 2026-09-03 over
    /// **94,542,598** MBO records — 16 day-files, XNAS.ITCH + ARCX.PILLAR, 5
    /// instruments, 2025-02-03 -> 2026-01-07:
    ///
    /// ```text
    ///   LAST              1<<7   52.291% - 85.168% per file   the venue-event boundary
    ///   PUBLISHER_SPECIFIC 1<<1  see the era note below
    ///   BAD_TS_RECV       1<<3   exactly 1 record per file, 16/16 files (the leading `R`)
    ///   TOB               1<<6   0
    ///   SNAPSHOT          1<<5   0
    ///   MBP               1<<4   0
    ///   MAYBE_BAD_BOOK    1<<2   0
    ///   bit 0                    0
    /// ```
    ///
    /// ⚠ **Do not write "vendor data-quality flags are being lost".** The two bits
    /// that carry a genuine quality signal — `MAYBE_BAD_BOOK` and `SNAPSHOT` — are
    /// **absent** from every record measured. What was being destroyed is `LAST`,
    /// `PUBLISHER_SPECIFIC` and `BAD_TS_RECV`.
    ///
    /// ⚠ **THE ENCODING IS NOT STABLE ACROSS THE CORPUS.** `PUBLISHER_SPECIFIC` went
    /// from **52.197% -> 0.000%** between `xnas-itch-20250801` and
    /// `xnas-itch-20250804`, and on ARCX from **84.228% -> 0.000%** on the same two
    /// dates; the distinct raw-value set went `[0, 8, 128, 130] -> [0, 8, 128]`.
    /// Re-measured here on three further instruments, all flipping on the same date
    /// (SNAP 70.585 -> 0.000, CRSP 46.563 -> 0.000, PEP 55.354 -> 0.000). The
    /// 233-day flagship corpus straddles that boundary, and nothing in either repo
    /// could detect it. That is why the carrier is the **raw byte** rather than a
    /// fixed set of per-bit counters: a counter set chosen today forecloses whichever
    /// bit turns out to matter next.
    ///
    /// # Zero is a real value, not "unknown"
    ///
    /// `0` means "the vendor set no bits" — 130 is the modal value on a 2025-07 file
    /// and `0` is the second-most common. A message built by
    /// [`MboMessage::new`] also carries `0`, because a synthesised message has no
    /// vendor flags. The field is deliberately a plain `u8` and not an
    /// `Option<u8>`: the vendor's own type is a transparent `u8` bit field with, in
    /// its own words, **no universal null**, and inventing a third "unknown" state
    /// would create a distinction no consumer can act on.
    pub flags: u8,
}

impl MboMessage {
    /// Create a new MBO message.
    ///
    /// `timestamp` is `None` and [`Self::flags`] is `0` — this constructor builds a
    /// SYNTHESISED message, which by definition carries no vendor flag byte. The
    /// vendor path is [`crate::DbnBridge::convert`], which populates both.
    pub fn new(order_id: u64, action: Action, side: Side, price: i64, size: u32) -> Self {
        Self {
            order_id,
            action,
            side,
            price,
            size,
            timestamp: None,
            flags: 0,
        }
    }

    /// Create with timestamp.
    pub fn with_timestamp(mut self, timestamp: i64) -> Self {
        self.timestamp = Some(timestamp);
        self
    }

    /// Get price as floating point dollars.
    #[inline]
    pub fn price_as_f64(&self) -> f64 {
        self.price as f64 / NANODOLLARS_PER_DOLLAR_F64
    }

    /// Returns `true` if this message has the FIELD SHAPE of a record that cannot
    /// describe a resting order.
    ///
    /// The shape is any of:
    /// - `order_id == 0` (no associated order)
    /// - `size == 0` (no quantity)
    /// - `price <= 0` (no valid price)
    ///
    /// ⚠ **THIS IS A FIELD-SHAPE TEST, NOT AN ADMISSION DECISION.** It is ACTION-BLIND,
    /// so it also matches two records that are not heartbeats: the vendor's book-reset
    /// `Action::Clear` (`order_id == 0`, `size == 0`) and every XNAS.ITCH trade print
    /// `Action::TradeAggregate` (`order_id == 0` on 100% of them). Do NOT use it to
    /// decide whether to skip a record before reconstruction — that drops every `Clear`
    /// and every XNAS `T`. The reconstructor's skip gate uses [`Self::is_heartbeat`].
    /// Measured share of records matching it: 375,644 of 9,314,830 (4.03%) on XNAS NVDA
    /// 2025-07-01 and 185,531 of 5,234,876 (3.54%) on ARCX NVDA 2025-07-01 — every one of
    /// them a `T` or a `Clear` (an earlier note here said "~10-15%" with no source).
    ///
    /// Its body is deliberately BYTE-IDENTICAL across rung 4 (DESIGN B — see
    /// [`Self::is_heartbeat`]).
    #[inline]
    pub fn is_system_message(&self) -> bool {
        self.order_id == 0 || self.size == 0 || self.price <= 0
    }

    /// Returns `true` if this message is a **heartbeat** in this crate's LOCAL sense: a
    /// record on an order-bearing or no-op action (`Add`, `Modify`, `Cancel`, `Fill`,
    /// `None`) whose fields cannot name an order or a level (`order_id == 0`, `size == 0`
    /// or `price <= 0`). The reconstructor's skip gate (`LobConfig::skip_system_messages`,
    /// default `true`) skips such a record rather than routing it.
    ///
    /// ⚠ **CRATE-LOCAL — NOT DATABENTO'S HEARTBEAT.** `dbn`'s `SystemMsg::is_heartbeat()`
    /// tests a different RECORD TYPE (a gateway `SystemMsg` whose text is the heartbeat
    /// string), which never becomes an `MboMessage`. The two share a name only.
    ///
    /// ⚠ **ON REAL DATA IT MATCHES NOTHING.** The COMMIT A review's vendor census (1,807
    /// day-files, 5,143,699,736 MBO records; relayed, not re-derived here) found no record
    /// this predicate matches: every field-shape match is a `TradeAggregate` or a `Clear`.
    /// So since rung 4A `LobStats::system_messages_skipped` is a STRUCTURAL 0 on the corpus,
    /// where a correct counter and a dead one read identically (`FINDING-155`); its
    /// validation is behavioural —
    /// `tests/l_admit_half_landing_lock.rs::heartbeat_still_skipped`.
    ///
    /// This is the ACTION-AWARE admission predicate added at rung 4 (L-ADMIT). For five
    /// actions it is exactly the field-shape test [`Self::is_system_message`]. For the two
    /// actions whose vendor wire shape legitimately matches that test it is `false`,
    /// whatever the fields:
    ///
    /// * [`Action::Clear`] — **Phase O Cycle 1 / B.2a (NEW-AUDIT-A3 closure), moved here
    ///   from the call site in `LobReconstructor::process_message_into` at rung 4.** A Clear
    ///   is a SEMANTIC market event — the session-boundary / mid-day book wipe (circuit
    ///   breaker, market-wide halt) — that canonically carries a heartbeat's zero-field shape:
    ///   the vendor sends `order_id == 0`, `size == 0` and `price == UNDEF_PRICE`. Before B.2a
    ///   the skip gate swallowed it under the DEFAULT config, so the `Action::Clear =>
    ///   self.reset()` handler was unreachable, the book never reset, and each day inherited
    ///   the previous day's resting orders. The companion Phase O B.2b fix exempted Clear in
    ///   the sibling extractor's outer filter; B.2a is the load-bearing book-state fix.
    ///   Locked by `tests/l_admit_half_landing_lock.rs::clear_is_never_a_heartbeat`.
    /// * [`Action::TradeAggregate`] — **rung 4 (L-ADMIT).** The vendor `T` is the aggressing
    ///   order's EXECUTION PRINT, not an order: it carries `order_id == 0` on 100% of
    ///   XNAS.ITCH records (375,643 / 375,643 on 2025-07-01) and on 100% of ARCX's `T|A` and
    ///   `T|B` cells (104,268 and 81,261 on 2025-07-01). It is a vendor book no-op that must
    ///   be COUNTED (the carrier census, `LobStats::aggregate_trades_*`), not skipped as a
    ///   heartbeat; under the field-shape test every XNAS `T` was skipped and the census read
    ///   a structural 0 there. Admission is for the ROUTER and the COUNTERS only — the
    ///   router's `TradeAggregate` arm mutates no book state.
    ///
    /// [`Action::None`] is deliberately **NOT** exempt: it has the same zero-field shape but
    /// is a no-op with no required handler side effect, so a zero-field `None` stays a
    /// heartbeat — the pre-B.2a behaviour, preserved. (`N` measures ZERO records in the
    /// 94,542,598-record census cited in `DbnBridge::convert`, so this is a judgement, not a
    /// measured need.) [`Action::Fill`], the OTHER carrier, is not exempt either: its
    /// `order_id` is a real reference to a resting order, so an order-less `Fill` is not a
    /// fill.
    ///
    /// # DESIGN B — `is_system_message()` is deliberately left BYTE-IDENTICAL
    ///
    /// Rung 4 could have been landed by editing [`Self::is_system_message`] to exempt
    /// `TradeAggregate`. It must not be. Three consumers — `feature-extractor-MBO-LOB`,
    /// `mbo-statistical-profiler` and `xsec_equity_discovery/extractor` — are linked to this
    /// crate BY PATH during the candidate cycle, and the extractor drops system messages with
    /// its OWN call to `is_system_message()` before its sampler counts the event. Changing
    /// that body would admit `T` in the extractor with NO extractor edit and silently
    /// re-phase its exported rows, with no counter moving and no test going red. So the
    /// action-aware predicate is ADDED here, and each repo migrates its own call sites in its
    /// own commit. Locked by
    /// `tests/l_admit_half_landing_lock.rs::design_b_is_system_message_truth_table_unchanged`.
    ///
    /// # Why an exhaustive match with one arm per variant
    ///
    /// No wildcard, so a future `Action` variant fails to compile here until it is
    /// dispositioned. No or-pattern naming a carrier: `scripts/ci/check_carrier_disjunction.py`
    /// flags an arm that names both carriers, and a predicate that decides a carrier's fate
    /// must never read as a merge of the two.
    #[inline]
    pub fn is_heartbeat(&self) -> bool {
        match self.action {
            Action::Add => self.is_system_message(),
            Action::Modify => self.is_system_message(),
            Action::Cancel => self.is_system_message(),
            Action::TradeAggregate => false,
            Action::Fill => self.is_system_message(),
            Action::Clear => false,
            Action::None => self.is_system_message(),
        }
    }

    /// Validate a message for ADMISSION to the reconstructor's router: [`Self::validate`]
    /// made action-aware (rung 4, L-ADMIT). The reconstructor's validation gate
    /// (`LobConfig::validate_messages`, default `true`) calls this, not `validate()`.
    ///
    /// * [`Action::Clear`] => `Ok(())`. A Clear names no order and no level. This is the
    ///   Phase O B.2a validation exemption, unchanged in meaning, moved here from the call
    ///   site: without it the B.2a skip exemption would only move the silent drop one line
    ///   down (Clear passes the skip gate, then dies here as `InvalidOrderId(0)`).
    /// * [`Action::TradeAggregate`] => every clause of [`Self::validate`] EXCEPT
    ///   `order_id == 0`: the FIELD clauses (`price > 0`, `price != UNDEF_PRICE`,
    ///   `size != 0`, in `validate()`'s own order). A trade print's `order_id` is not an
    ///   order reference (0 on XNAS; a trade identifier on ARCX `T|N`), so the
    ///   ORDER-REFERENCE clause is inapplicable to it.
    /// * every other action => [`Self::validate`], unchanged.
    ///
    /// `validate()` is two private clause groups, `validate_order_reference` then
    /// `validate_fields`; the `TradeAggregate` arm calls `validate_fields` alone. So the
    /// W04 undefined-price test and any future field clause have ONE definition shared with
    /// the order-bearing actions, and a future clause that reads `order_id` belongs in
    /// `validate_order_reference`, where it cannot silently pass a trade print.
    ///
    /// # A malformed trade print FAILS CLOSED — deliberately
    ///
    /// A trade print whose fields fail a clause gets exactly the error [`Self::validate`]
    /// gives an order-bearing record with those fields. End to end under the DEFAULT config
    /// the two are NOT treated alike, and the asymmetry is the decision (hft-rules §8,
    /// fail-open vs fail-closed stated at the site): a malformed `Add`, `Modify`, `Cancel`,
    /// `Fill` or `None` (`size == 0` or `price <= 0`) is a heartbeat and is SKIPPED before
    /// validation runs, but a `TradeAggregate` is never a heartbeat, so its malformation
    /// surfaces here as an `Err` instead of a silent skip. Before rung 4A such a print was
    /// skipped. The measured population is zero — the COMMIT A review's vendor census found
    /// 0 malformed `T` among 202,054,096 (relayed, not re-derived here) — so nothing on disk
    /// moves, and a consumer that propagates with `?` (`mbo-statistical-profiler`) would
    /// abort on the first one rather than lose it.
    ///
    /// # ⚠ THE SKIP GATE AND THIS GATE ARE ONE CHANGE
    ///
    /// Once [`Self::is_heartbeat`] admits a `TradeAggregate`, a validation gate that still
    /// called `validate()` would return `Err(InvalidOrderId(0))` for EVERY XNAS trade print.
    /// Four production sites turn that into a silent skip — three `.is_err()` sites
    /// (`xsec_equity_discovery/extractor`'s panel producer `continue`s;
    /// `fill_bracket_extract` and `auction_book_extract` return from the per-message
    /// handler) plus one counted, WARN-logged `Err` arm in this crate's `export_to_parquet`
    /// — so the carrier would vanish from their counts on a green build with exit code 0.
    /// Locked by
    /// `tests/l_admit_half_landing_lock.rs::l_admit_relaxes_both_gates_or_the_carrier_is_rejected_not_merely_skipped`.
    pub fn validate_admission(&self) -> crate::error::Result<()> {
        match self.action {
            Action::Add => self.validate(),
            Action::Modify => self.validate(),
            Action::Cancel => self.validate(),
            Action::TradeAggregate => self.validate_fields(),
            Action::Fill => self.validate(),
            Action::Clear => Ok(()),
            Action::None => self.validate(),
        }
    }

    /// Validate the message fields.
    ///
    /// Unlike [`Self::is_system_message()`], this method checks whether a message
    /// that *should* represent a valid order actually has valid field values. The
    /// reconstructor skips heartbeats ([`Self::is_heartbeat`]) before it validates, and
    /// validates through [`Self::validate_admission`], not this method directly.
    ///
    /// Since rung 4A the body is two private clause groups, `validate_order_reference`
    /// (`order_id == 0`) then `validate_fields` (`price <= 0`, the undefined-price
    /// sentinel, `size == 0`), in the original clause order: behaviour and error
    /// precedence are unchanged.
    ///
    /// # W04 — the undefined-value sentinels, and why `price <= 0` does not catch them
    ///
    /// `i64::MAX` is **positive**, so the vendor's undefined-price sentinel walks
    /// straight past the `price <= 0` clause and emerges from
    /// [`Self::price_as_f64`] as `9_223_372_036.854_776` — a **finite, plausible**
    /// $9.2-billion quote that satisfies every `is_finite()` guard downstream. That
    /// is hft-rules §2's named failure: "an unguarded divide neither crashes nor
    /// yields `NaN`". Measured on the candidate at HEAD before this change:
    /// `convert() -> Ok`, `price_as_f64 = 9223372036.854776`, `validate() = true`,
    /// `mid = Some(4611686096.947389)`, `is_finite(mid) = true`.
    ///
    /// ⚠ **ONLY THE PRICE SENTINEL IS CHECKED HERE, AND THE ASYMMETRY IS THE POINT**
    /// (hft-rules §2: a sentinel is a PER-FIELD property of a vendor's wire format;
    /// guard on what the schema declares for THAT field).
    ///
    /// * **PRICE — declared, mandated, and checked in BOTH places.** `MboMsg.price`
    ///   carries the `fixed_price` attribute in the vendor's own record definition,
    ///   and `data/DATABENTO_SCHEMA_REFERENCE.md` states the rule twice: the sentinel
    ///   table gives `i64::MAX` for every fixed price or value, and §7 says "Test
    ///   `i64::MAX` before dividing a fixed price or value by 1e9." Nothing in this
    ///   crate treats `i64::MAX` as a legitimate price, and `price <= 0` already
    ///   asserts price sanity for this type, so the sentinel belongs here as
    ///   well as at the boundary.
    /// * **SIZE — a WIRE null only, so it is checked at the WIRE only.** See the
    ///   comment beside the `size == 0` clause in `validate_fields` for the full argument
    ///   and the execution that settled it. `dbn::UNDEF_ORDER_SIZE` is guarded in
    ///   [`crate::DbnBridge::convert`] and deliberately NOT here.
    ///
    /// # FAIL-CLOSED, and the decision is stated here (hft-rules §8)
    ///
    /// The clause REJECTS rather than clamps or zeroes. A rejection is observable —
    /// the caller sees a typed `TlobError`, and on the loader path
    /// `LoaderStats::messages_skipped` advances under `skip_invalid` — whereas a
    /// clamped or zeroed value is a silent wrong number of exactly the kind this
    /// pipeline exists to prevent.
    ///
    /// # Reachability, and the measured exposure
    ///
    /// `LobReconstructor::process_message_into` calls this through
    /// [`Self::validate_admission`] under `config.validate_messages`, which **defaults to
    /// `true`** — so it runs on every admitted non-`Clear` record (on a `TradeAggregate`
    /// only its field clauses, since rung 4). `Action::Clear` is exempted by
    /// the CALLER (it is not supposed to represent a valid order, and it is the one action
    /// that legitimately carries the sentinel), which is why no action test appears here.
    ///
    /// ⚠ **This is a GUARD GAP, not a live wrong number.** Re-measured 2026-09-03
    /// over 94,542,598 MBO records / 16 files / 2 venues / 5 instruments:
    /// `price == i64::MAX` occurs **20 times, 100% of them on `Action::Clear`**;
    /// `size == u32::MAX`, `size == i32::MAX` and `price <= 0` occur **ZERO** times.
    /// Nothing on disk changes because of this clause. Promoting it to a
    /// block-production defect without that hedge would repeat `FINDING-181`.
    pub fn validate(&self) -> crate::error::Result<()> {
        self.validate_order_reference()?;
        self.validate_fields()
    }

    /// The ORDER-REFERENCE clause of [`Self::validate`]: a record that references a
    /// resting order must name one (`order_id != 0`). Split out at rung 4A so the
    /// `TradeAggregate` arm of [`Self::validate_admission`] can apply every OTHER clause —
    /// a trade print's `order_id` is not an order reference. A future clause that reads
    /// `order_id` belongs here, where it cannot silently pass a trade print.
    fn validate_order_reference(&self) -> crate::error::Result<()> {
        if self.order_id == 0 {
            return Err(crate::error::TlobError::InvalidOrderId(0));
        }
        Ok(())
    }

    /// The FIELD clauses of [`Self::validate`], in its order: `price <= 0`, the vendor's
    /// undefined-price sentinel, `size == 0`. The one definition shared by `validate()`
    /// and the `TradeAggregate` arm of [`Self::validate_admission`], so a field clause
    /// added here reaches both. It must read no `order_id`.
    fn validate_fields(&self) -> crate::error::Result<()> {
        use crate::error::TlobError;

        if self.price <= 0 {
            return Err(TlobError::InvalidPrice(self.price));
        }

        // `dbn::UNDEF_PRICE`. Spelled as `i64::MAX` because this module must compile
        // without the `databento` feature, so it cannot name the vendor constant.
        // `vendor_sentinel_constants_still_have_the_values_this_crate_hardcodes`
        // in `tests/decode_sentinel_contract.rs` is the check that the two agree —
        // without it this would be a second, silently-divergent copy of the vendor's
        // map, which is exactly how `b'F'` came to decode as a trade.
        if self.price == i64::MAX {
            return Err(TlobError::InvalidPrice(self.price));
        }

        if self.size == 0 {
            return Err(TlobError::InvalidSize(0));
        }

        // ⛔ THERE IS DELIBERATELY NO `size == u32::MAX` CLAUSE HERE, AND THE
        // OMISSION IS THE POINT. `dbn::UNDEF_ORDER_SIZE` is a property of the VENDOR
        // WIRE FORMAT, not of this domain type, so it is guarded at the vendor
        // boundary (`DbnBridge::convert`) and nowhere else — hft-rules §8: validate
        // at system boundaries, trust internal code.
        //
        // Adding it here was tried and REFUTED BY EXECUTION. It fails
        // `tests/integration_test.rs::test_edge_case_large_sizes`, a pre-existing
        // OVERFLOW-BOUNDARY contract (hft-rules §6 "Boundary") that builds a book at
        // `size = u32::MAX` through `MboMessage::new` and asserts the u32 -> u64
        // widening does not overflow: `total_bid_volume == u32::MAX as u64`,
        // `MarketImpact::simulate_buy(..).can_fill()`, `depth_imbalance ~ 0`. It uses
        // the value as the LARGEST REPRESENTABLE SIZE, never as a sentinel, and it
        // never touches the vendor path.
        //
        // Three independent sources agree that `u32::MAX` is a legal size for this
        // TYPE and only a null on the WIRE: (1) the vendor declines to declare it —
        // `data/DATABENTO_SCHEMA_REFERENCE.md` limits §4, "ordinary trade, MBO, MBP,
        // BBO and CBBO size fields do not all document a field-specific null rule";
        // (2) that test; (3) the live corpus, where `size == u32::MAX` occurs 0 times
        // in 94,542,598 records. The price sentinel is NOT symmetric with it — see
        // `validate()`'s docs — which is why exactly one of the two clauses lives here.

        Ok(())
    }
}

/// Order information stored in LOB.
///
/// Minimal representation to save memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Order {
    pub side: Side,
    pub price: i64,
    pub size: u32,
}

/// Book consistency status after validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BookConsistency {
    /// Book is valid: best_bid < best_ask
    Valid,
    /// Book is empty (no quotes on one or both sides)
    Empty,
    /// Book is locked: best_bid == best_ask (unusual but can occur)
    Locked,
    /// Book is crossed: best_bid > best_ask (invalid state)
    Crossed,
}

impl BookConsistency {
    /// Returns true if the book state is valid for trading/analysis.
    #[inline]
    pub fn is_valid(&self) -> bool {
        matches!(self, BookConsistency::Valid)
    }

    /// Returns true if the book is crossed (invalid state).
    #[inline]
    pub fn is_crossed(&self) -> bool {
        matches!(self, BookConsistency::Crossed)
    }

    /// Returns true if the book is locked (bid == ask).
    #[inline]
    pub fn is_locked(&self) -> bool {
        matches!(self, BookConsistency::Locked)
    }

    /// Returns true if the book is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        matches!(self, BookConsistency::Empty)
    }
}

/// LOB state snapshot.
///
/// This represents the current state of the order book at N levels.
/// **Fixed-size arrays are used for stack allocation** to eliminate heap allocations
/// in the hot path, providing significant throughput improvements.
///
/// # Memory Layout
///
/// - `bid_prices`: 20 × 8 bytes = 160 bytes  
/// - `bid_sizes`: 20 × 4 bytes = 80 bytes
/// - `ask_prices`: 20 × 8 bytes = 160 bytes
/// - `ask_sizes`: 20 × 4 bytes = 80 bytes
/// - temporal fields: ~32 bytes
/// - metadata: ~48 bytes
/// - **Total**: ~560 bytes (stack-allocated, fits in ~9 cache lines)
///
/// # Invariants
///
/// - `levels` ≤ `MAX_LOB_LEVELS` (20)
/// - Prices are in **nanodollars** (i64, divide by 1e9 for dollars)
/// - Index 0 = best level (highest bid, lowest ask)
/// - Unused levels contain zeros
///
/// # Temporal Information
///
/// The temporal fields enable time-sensitive feature extraction (FI-2010 u6-u9):
/// - `delta_ns`: Time since last LOB update (for dP/dt, dV/dt)
/// - `triggering_action`: What caused this state change
/// - `triggering_side`: Which side was affected
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobState {
    /// Bid prices in nanodollars (highest to lowest).
    /// Index 0 = best bid (highest price), increasing index = deeper levels.
    /// Unused levels are zero.
    pub bid_prices: [i64; MAX_LOB_LEVELS],

    /// Bid sizes in shares (corresponding to bid_prices).
    pub bid_sizes: [u32; MAX_LOB_LEVELS],

    /// Ask prices in nanodollars (lowest to highest).
    /// Index 0 = best ask (lowest price), increasing index = deeper levels.
    /// Unused levels are zero.
    pub ask_prices: [i64; MAX_LOB_LEVELS],

    /// Ask sizes in shares (corresponding to ask_prices).
    pub ask_sizes: [u32; MAX_LOB_LEVELS],

    /// Best bid price (cached for O(1) access).
    pub best_bid: Option<i64>,

    /// Best ask price (cached for O(1) access).
    pub best_ask: Option<i64>,

    /// Number of active levels (≤ MAX_LOB_LEVELS).
    /// Methods like `mid_price()` only consider levels up to this value.
    pub levels: usize,

    /// Timestamp of this snapshot (nanoseconds since epoch).
    pub timestamp: Option<i64>,

    /// Index of the message that produced this state — this crate's OWN
    /// counter, **not the vendor's `sequence` field**.
    ///
    /// It is `LobStats::messages_processed` at the moment the snapshot was
    /// filled: 1 for the first message the reconstructor accepted, and +1 for
    /// every message thereafter. It says where a row sits in OUR stream. It
    /// says nothing about where the record sat in the VENUE's.
    ///
    /// ⚠ **THE TWO ARE NOT INTERCHANGEABLE, AND THIS FIELD WAS CALLED
    /// `sequence` UNTIL 2026-09-06.** `dbn::MboMsg` carries its own
    /// `sequence: u32` — the venue's number, which is what a gap check, a
    /// message-loss estimate or a cross-feed join needs. This crate has never
    /// read it: it is dropped at [`crate::DbnBridge::convert`], which builds
    /// [`MboMessage`] without a sequence field at all. So a consumer that took
    /// the old name at face value was handed a dense `+1` counter in place of
    /// the sparse venue one, and any gap check over it returns "no gaps"
    /// unconditionally, for any input, forever.
    ///
    /// Measured on ARCX.PILLAR NVDA (9,314,830 records): the vendor's
    /// `sequence` spans 0..=657,600,477 with 7,188,190 distinct values. Ours
    /// would have spanned 1..=9,314,830 with no repeats and no holes.
    ///
    /// ⚠ **THE EXPORTED PARQUET COLUMN IS STILL NAMED `sequence`** — see
    /// `crate::export::schema::lob_snapshot_schema` for why that name did not
    /// move with the field.
    pub message_index: u64,

    // =========================================================================
    // Temporal Information (FI-2010 time-sensitive features u6-u9)
    // =========================================================================
    /// Previous timestamp for Δt calculation (nanoseconds since epoch).
    ///
    /// This is the timestamp of the previous LOB update, enabling:
    /// - Inter-arrival time analysis
    /// - Rate-of-change calculations (dP/dt, dV/dt)
    pub previous_timestamp: Option<i64>,

    /// Time delta since last LOB update (nanoseconds).
    ///
    /// Computed as: `timestamp - previous_timestamp`
    ///
    /// Use cases:
    /// - Intensity features: events_per_second = 1e9 / delta_ns
    /// - Velocity features: dP/dt = Δprice / (delta_ns / 1e9)
    /// - Volatility scaling by time
    pub delta_ns: u64,

    /// The action that triggered this LOB state change.
    ///
    /// Enables action-specific feature extraction:
    /// - Add events: new liquidity arriving
    /// - Cancel events: liquidity withdrawing
    /// - Trade events: aggressive order execution
    /// - Modify events: order repricing/resizing
    pub triggering_action: Option<Action>,

    /// The side affected by the triggering action.
    ///
    /// Combined with `triggering_action`, enables asymmetric analysis:
    /// - Bid-side adds vs ask-side adds
    /// - Which side is experiencing more cancellations
    /// - Trade direction (aggressor side)
    pub triggering_side: Option<Side>,
}

impl LobState {
    /// Create a new empty LOB state with specified number of levels.
    ///
    /// # Arguments
    ///
    /// * `levels` - Number of price levels to track (clamped to MAX_LOB_LEVELS)
    ///
    /// # Performance
    ///
    /// This is now **zero-allocation** - the entire struct lives on the stack.
    /// Previous implementation allocated 4 Vecs on the heap per call.
    #[inline]
    pub fn new(levels: usize) -> Self {
        Self {
            bid_prices: [0; MAX_LOB_LEVELS],
            bid_sizes: [0; MAX_LOB_LEVELS],
            ask_prices: [0; MAX_LOB_LEVELS],
            ask_sizes: [0; MAX_LOB_LEVELS],
            best_bid: None,
            best_ask: None,
            levels: levels.min(MAX_LOB_LEVELS), // Clamp to max
            timestamp: None,
            message_index: 0,
            // Temporal fields
            previous_timestamp: None,
            delta_ns: 0,
            triggering_action: None,
            triggering_side: None,
        }
    }

    // =========================================================================
    // Temporal Analytics
    // =========================================================================

    /// Get time delta in seconds (as f64).
    ///
    /// Useful for rate calculations like dP/dt.
    ///
    /// # Returns
    /// - `Some(seconds)` if delta_ns > 0
    /// - `None` if no previous timestamp
    #[inline]
    pub fn delta_seconds(&self) -> Option<f64> {
        if self.delta_ns > 0 {
            Some(self.delta_ns as f64 / NS_PER_SECOND_F64)
        } else {
            None
        }
    }

    /// Get event intensity (events per second).
    ///
    /// Calculated as: 1 / delta_seconds
    ///
    /// # Returns
    /// - `Some(intensity)` if delta_ns > 0
    /// - `None` if no previous timestamp or delta is 0
    #[inline]
    pub fn event_intensity(&self) -> Option<f64> {
        if self.delta_ns > 0 {
            Some(NS_PER_SECOND_F64 / self.delta_ns as f64)
        } else {
            None
        }
    }

    /// Check if this state was triggered by a specific action.
    ///
    /// ⚠ [`Action::TradeAggregate`] and [`Action::Fill`] are **DISJOINT** populations over
    /// `triggering_action` — they are the aggressor-side and resting-side views of the same
    /// physical execution. A caller wanting "any execution" must therefore test **both**, and in
    /// doing so will **DOUBLE-COUNT every physical execution**. Prefer
    /// [`Self::is_aggregate_trade_event`] (one row per execution, aggressor side) unless the
    /// resting side is specifically wanted.
    #[inline]
    pub fn was_triggered_by(&self, action: Action) -> bool {
        self.triggering_action == Some(action)
    }

    /// Check if this state was triggered on the bid side.
    #[inline]
    pub fn was_triggered_on_bid(&self) -> bool {
        self.triggering_side == Some(Side::Bid)
    }

    /// Check if this state was triggered on the ask side.
    #[inline]
    pub fn was_triggered_on_ask(&self) -> bool {
        self.triggering_side == Some(Side::Ask)
    }

    /// Check if this is an **aggregate trade print** event (`T`) — the aggressor-side view.
    ///
    /// One row per physical execution, `triggering_side` = the **AGGRESSOR's** side.
    /// This is the predicate to use for trade counts, trade-conditional statistics and signed
    /// order flow.
    ///
    /// ⚠ Replaces the former `is_trade_event()`, which returned `true` for **both**
    /// [`Action::TradeAggregate`] and [`Action::Fill`] and therefore counted every physical
    /// execution twice, under two opposite side conventions. There is deliberately **no union
    /// predicate**: a caller that wants both must say so explicitly and own the double-count.
    #[inline]
    pub fn is_aggregate_trade_event(&self) -> bool {
        self.triggering_action == Some(Action::TradeAggregate)
    }

    /// Check if this is a **resting-order fill** event (`F`) — the resting-side view.
    ///
    /// `triggering_side` = the **RESTING order's** side, i.e. the OPPOSITE convention from
    /// [`Self::is_aggregate_trade_event`]. Use this for order-lifecycle / queue-depletion work,
    /// where the resting order is the subject.
    #[inline]
    pub fn is_resting_fill_event(&self) -> bool {
        self.triggering_action == Some(Action::Fill)
    }

    /// Check if this is an add event (new liquidity).
    #[inline]
    pub fn is_add_event(&self) -> bool {
        self.triggering_action == Some(Action::Add)
    }

    /// Check if this is a cancel event (liquidity withdrawal).
    #[inline]
    pub fn is_cancel_event(&self) -> bool {
        self.triggering_action == Some(Action::Cancel)
    }

    // =========================================================================
    // Book Consistency Validation
    // =========================================================================

    /// Check book consistency (whether bid < ask).
    ///
    /// This is a critical validation for market structure integrity.
    /// A crossed book (bid >= ask) indicates either:
    /// - Data quality issues
    /// - Market halt/unusual conditions
    /// - Reconstruction errors
    ///
    /// # Returns
    /// - `BookConsistency::Valid` if best_bid < best_ask
    /// - `BookConsistency::Empty` if either side has no quotes
    /// - `BookConsistency::Locked` if best_bid == best_ask
    /// - `BookConsistency::Crossed` if best_bid > best_ask
    #[inline]
    pub fn check_consistency(&self) -> BookConsistency {
        match (self.best_bid, self.best_ask) {
            (Some(bid), Some(ask)) => {
                if bid < ask {
                    BookConsistency::Valid
                } else if bid == ask {
                    BookConsistency::Locked
                } else {
                    BookConsistency::Crossed
                }
            }
            _ => BookConsistency::Empty,
        }
    }

    /// Returns true if the book is in a valid state (bid < ask).
    #[inline]
    pub fn is_consistent(&self) -> bool {
        self.check_consistency().is_valid()
    }

    /// Returns true if the book is crossed (bid > ask) - invalid state.
    #[inline]
    pub fn is_crossed(&self) -> bool {
        self.check_consistency().is_crossed()
    }

    /// Returns true if the book is locked (bid == ask).
    #[inline]
    pub fn is_locked(&self) -> bool {
        self.check_consistency().is_locked()
    }

    /// Validate book consistency and return an error if invalid.
    ///
    /// # Returns
    /// - `Ok(())` if book is valid
    /// - `Err(TlobError::CrossedQuote)` if book is crossed
    /// - `Err(TlobError::LockedQuote)` if book is locked
    pub fn validate_consistency(&self) -> crate::error::Result<()> {
        use crate::error::TlobError;

        match (self.best_bid, self.best_ask) {
            (Some(bid), Some(ask)) => {
                if bid > ask {
                    Err(TlobError::CrossedQuote(bid, ask))
                } else if bid == ask {
                    Err(TlobError::LockedQuote(bid, ask))
                } else {
                    Ok(())
                }
            }
            _ => Ok(()), // Empty book is considered valid
        }
    }

    // =========================================================================
    // Basic Analytics (already exist)
    // =========================================================================

    /// Calculate mid-price (average of best bid and ask).
    #[inline]
    pub fn mid_price(&self) -> Option<f64> {
        match (self.best_bid, self.best_ask) {
            (Some(bid), Some(ask)) => {
                let bid_f = bid as f64 / NANODOLLARS_PER_DOLLAR_F64;
                let ask_f = ask as f64 / NANODOLLARS_PER_DOLLAR_F64;
                Some((bid_f + ask_f) / 2.0)
            }
            _ => None,
        }
    }

    /// Calculate spread (difference between best ask and best bid).
    #[inline]
    pub fn spread(&self) -> Option<f64> {
        match (self.best_bid, self.best_ask) {
            (Some(bid), Some(ask)) => {
                let bid_f = bid as f64 / NANODOLLARS_PER_DOLLAR_F64;
                let ask_f = ask as f64 / NANODOLLARS_PER_DOLLAR_F64;
                Some(ask_f - bid_f)
            }
            _ => None,
        }
    }

    /// Check if LOB has valid state (at least one bid and one ask).
    #[inline]
    pub fn is_valid(&self) -> bool {
        self.best_bid.is_some() && self.best_ask.is_some()
    }

    /// Get spread in basis points (bps).
    #[inline]
    pub fn spread_bps(&self) -> Option<f64> {
        match (self.mid_price(), self.spread()) {
            (Some(mid), Some(spread)) if mid > 0.0 => Some((spread / mid) * BASIS_POINTS_PER_UNIT),
            _ => None,
        }
    }

    // =========================================================================
    // Enriched Analytics (NEW)
    // =========================================================================

    /// Calculate microprice (volume-weighted mid-price).
    ///
    /// Microprice = (bid_price * ask_size + ask_price * bid_size) / (bid_size + ask_size)
    ///
    /// This provides a better estimate of "fair value" than simple mid-price
    /// by incorporating the relative sizes at the best levels.
    ///
    /// # Returns
    /// - `Some(microprice)` in dollars if both sides have valid quotes
    /// - `None` if either side is empty or total size is zero
    #[inline]
    pub fn microprice(&self) -> Option<f64> {
        match (self.best_bid, self.best_ask) {
            (Some(bid), Some(ask)) => {
                let bid_size = self.bid_sizes.first().copied().unwrap_or(0) as f64;
                let ask_size = self.ask_sizes.first().copied().unwrap_or(0) as f64;
                let total_size = bid_size + ask_size;

                if total_size > 0.0 {
                    let bid_f = bid as f64 / NANODOLLARS_PER_DOLLAR_F64;
                    let ask_f = ask as f64 / NANODOLLARS_PER_DOLLAR_F64;
                    Some((bid_f * ask_size + ask_f * bid_size) / total_size)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Calculate total bid volume across active levels.
    #[inline]
    pub fn total_bid_volume(&self) -> u64 {
        self.bid_sizes[..self.levels]
            .iter()
            .map(|&s| s as u64)
            .sum()
    }

    /// Calculate total ask volume across active levels.
    #[inline]
    pub fn total_ask_volume(&self) -> u64 {
        self.ask_sizes[..self.levels]
            .iter()
            .map(|&s| s as u64)
            .sum()
    }

    /// Calculate depth imbalance (normalized difference between bid and ask volume).
    ///
    /// Imbalance = (bid_volume - ask_volume) / (bid_volume + ask_volume)
    ///
    /// Range: [-1.0, 1.0]
    /// - Positive: more volume on bid side (buying pressure)
    /// - Negative: more volume on ask side (selling pressure)
    /// - Zero: balanced book
    ///
    /// # Returns
    /// - `Some(imbalance)` if total volume > 0
    /// - `None` if book is empty
    #[inline]
    pub fn depth_imbalance(&self) -> Option<f64> {
        let bid_vol = self.total_bid_volume() as f64;
        let ask_vol = self.total_ask_volume() as f64;
        let total = bid_vol + ask_vol;

        if total > 0.0 {
            Some((bid_vol - ask_vol) / total)
        } else {
            None
        }
    }

    /// Calculate VWAP for the bid side (top N levels).
    ///
    /// VWAP = Σ(price * size) / Σ(size)
    ///
    /// # Arguments
    /// - `n_levels`: Number of levels to include (clamped to available levels)
    ///
    /// # Returns
    /// - `Some(vwap)` in dollars if bid side has volume
    /// - `None` if no bid volume
    #[inline]
    pub fn vwap_bid(&self, n_levels: usize) -> Option<f64> {
        let n = n_levels.min(self.levels);
        let mut total_value: f64 = 0.0;
        let mut total_size: u64 = 0;

        for i in 0..n {
            let price = self.bid_prices[i];
            let size = self.bid_sizes[i] as u64;
            if price > 0 && size > 0 {
                total_value += (price as f64 / NANODOLLARS_PER_DOLLAR_F64) * (size as f64);
                total_size += size;
            }
        }

        if total_size > 0 {
            Some(total_value / total_size as f64)
        } else {
            None
        }
    }

    /// Calculate VWAP for the ask side (top N levels).
    ///
    /// VWAP = Σ(price * size) / Σ(size)
    ///
    /// # Arguments
    /// - `n_levels`: Number of levels to include (clamped to available levels)
    ///
    /// # Returns
    /// - `Some(vwap)` in dollars if ask side has volume
    /// - `None` if no ask volume
    #[inline]
    pub fn vwap_ask(&self, n_levels: usize) -> Option<f64> {
        let n = n_levels.min(self.levels);
        let mut total_value: f64 = 0.0;
        let mut total_size: u64 = 0;

        for i in 0..n {
            let price = self.ask_prices[i];
            let size = self.ask_sizes[i] as u64;
            if price > 0 && size > 0 {
                total_value += (price as f64 / NANODOLLARS_PER_DOLLAR_F64) * (size as f64);
                total_size += size;
            }
        }

        if total_size > 0 {
            Some(total_value / total_size as f64)
        } else {
            None
        }
    }

    /// Calculate weighted mid-price using VWAP from both sides.
    ///
    /// This is the average of bid VWAP and ask VWAP for top N levels.
    ///
    /// # Arguments
    /// - `n_levels`: Number of levels to include on each side
    ///
    /// # Returns
    /// - `Some(weighted_mid)` in dollars if both sides have volume
    /// - `None` if either side is empty
    #[inline]
    pub fn weighted_mid(&self, n_levels: usize) -> Option<f64> {
        match (self.vwap_bid(n_levels), self.vwap_ask(n_levels)) {
            (Some(bid_vwap), Some(ask_vwap)) => Some((bid_vwap + ask_vwap) / 2.0),
            _ => None,
        }
    }

    /// Get number of active bid levels (with non-zero size).
    #[inline]
    pub fn active_bid_levels(&self) -> usize {
        self.bid_sizes[..self.levels]
            .iter()
            .filter(|&&s| s > 0)
            .count()
    }

    /// Get number of active ask levels (with non-zero size).
    #[inline]
    pub fn active_ask_levels(&self) -> usize {
        self.ask_sizes[..self.levels]
            .iter()
            .filter(|&&s| s > 0)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Action and Side tests
    // =========================================================================

    #[test]
    fn test_action_from_byte() {
        assert_eq!(Action::from_byte(b'A'), Some(Action::Add));
        assert_eq!(Action::from_byte(b'M'), Some(Action::Modify));
        assert_eq!(Action::from_byte(b'C'), Some(Action::Cancel));
        assert_eq!(Action::from_byte(b'T'), Some(Action::TradeAggregate));
        assert_eq!(Action::from_byte(b'F'), Some(Action::Fill));
        assert_eq!(Action::from_byte(b'R'), Some(Action::Clear));
        assert_eq!(Action::from_byte(b'N'), Some(Action::None));
        assert_eq!(Action::from_byte(b'X'), Option::None);
    }

    #[test]
    fn test_action_to_byte() {
        assert_eq!(Action::Add.to_byte(), b'A');
        assert_eq!(Action::Modify.to_byte(), b'M');
        assert_eq!(Action::Cancel.to_byte(), b'C');
        assert_eq!(Action::TradeAggregate.to_byte(), b'T');
        assert_eq!(Action::Fill.to_byte(), b'F');
        assert_eq!(Action::Clear.to_byte(), b'R');
        assert_eq!(Action::None.to_byte(), b'N');
    }

    /// ⚠ THE WIRE-FORMAT LOCK. Do not weaken, do not delete.
    ///
    /// `Action::to_byte()` is `self as u8`, and that byte is written verbatim into the Parquet
    /// `action` / `triggering_action` columns of the shipped corpus. Downstream consumers key on
    /// the **decimal** values (`MBO-LOB-analyzer`: `ACTION_TRADE = 84`, `ACTION_FILL = 70`).
    ///
    /// If `#[repr(u8)]` is dropped, or an explicit `= b'X'` discriminant is lost during a rename,
    /// the enum silently emits `0..6` instead. Nothing errors — the analyzer's masks just match
    /// nothing, and a `if count > 0` guard makes the absent key indistinguishable from a zero
    /// count. This test asserts the **literal integers**, not `b'X'` spellings, precisely so a
    /// discriminant loss cannot hide behind a self-consistent rename.
    ///
    /// (`tests/export_test.rs::test_mbo_all_action_variants` CANNOT serve this purpose: it
    /// asserts `written == action.to_byte()`, i.e. the value against the function that wrote it.)
    #[test]
    fn test_action_discriminants_are_ascii_wire_format() {
        // The two carriers this crate's T/F split exists to keep apart.
        assert_eq!(
            Action::TradeAggregate.to_byte(),
            84u8,
            "Action::TradeAggregate MUST encode as decimal 84 (ASCII 'T') — the analyzer's \
             ACTION_TRADE. A different value silently voids every trade mask on 94 GB of Parquet."
        );
        assert_eq!(
            Action::Fill.to_byte(),
            70u8,
            "Action::Fill MUST encode as decimal 70 (ASCII 'F') — the analyzer's ACTION_FILL. \
             This byte has never appeared on disk; the T/F split is what activates it."
        );

        // The full alphabet, so a partial rename cannot pass by touching only T/F.
        assert_eq!(Action::Add.to_byte(), 65u8); // 'A'
        assert_eq!(Action::Modify.to_byte(), 77u8); // 'M'
        assert_eq!(Action::Cancel.to_byte(), 67u8); // 'C'
        assert_eq!(Action::Clear.to_byte(), 82u8); // 'R'
        assert_eq!(Action::None.to_byte(), 78u8); // 'N'

        // Round-trip through the canonical decoder: byte -> Action -> byte is the identity.
        for byte in [65u8, 77, 67, 84, 70, 82, 78] {
            let action = Action::from_byte(byte)
                .unwrap_or_else(|| panic!("canonical decoder rejected wire byte {byte}"));
            assert_eq!(
                action.to_byte(),
                byte,
                "decode/encode round-trip broken for wire byte {byte}"
            );
        }

        // The discriminants must be pairwise distinct (a collision would alias two populations).
        let all = [
            Action::Add,
            Action::Modify,
            Action::Cancel,
            Action::TradeAggregate,
            Action::Fill,
            Action::Clear,
            Action::None,
        ];
        let mut bytes: Vec<u8> = all.iter().map(|a| a.to_byte()).collect();
        bytes.sort_unstable();
        bytes.dedup();
        assert_eq!(
            bytes.len(),
            all.len(),
            "Action discriminants are not distinct"
        );
    }

    #[test]
    fn test_side_checks() {
        assert!(Side::Bid.is_bid());
        assert!(!Side::Ask.is_bid());
        assert!(Side::Ask.is_ask());
        assert!(!Side::Bid.is_ask());
        assert!(!Side::None.is_bid());
        assert!(!Side::None.is_ask());
    }

    #[test]
    fn test_side_from_byte() {
        assert_eq!(Side::from_byte(b'B'), Some(Side::Bid));
        assert_eq!(Side::from_byte(b'A'), Some(Side::Ask));
        assert_eq!(Side::from_byte(b'N'), Some(Side::None));
        assert_eq!(Side::from_byte(b'X'), None);
    }

    // =========================================================================
    // MboMessage tests
    // =========================================================================

    #[test]
    fn test_mbo_message_price_conversion() {
        let msg = MboMessage::new(
            123,
            Action::Add,
            Side::Bid,
            100_000_000_000, // $100.00
            100,
        );

        assert_eq!(msg.price_as_f64(), 100.0);
    }

    #[test]
    fn test_mbo_message_with_timestamp() {
        let msg = MboMessage::new(123, Action::Add, Side::Bid, 100_000_000_000, 100)
            .with_timestamp(1234567890_000_000_000);

        assert_eq!(msg.timestamp, Some(1234567890_000_000_000));
    }

    #[test]
    fn test_mbo_message_validation() {
        let msg = MboMessage::new(123, Action::Add, Side::Bid, 100_000_000_000, 100);

        assert!(msg.validate().is_ok());

        // Invalid: zero order_id
        let invalid = MboMessage::new(0, Action::Add, Side::Bid, 100_000_000_000, 100);
        assert!(invalid.validate().is_err());

        // Invalid: zero price
        let invalid = MboMessage::new(123, Action::Add, Side::Bid, 0, 100);
        assert!(invalid.validate().is_err());

        // Invalid: negative price
        let invalid = MboMessage::new(123, Action::Add, Side::Bid, -100, 100);
        assert!(invalid.validate().is_err());

        // Invalid: zero size
        let invalid = MboMessage::new(123, Action::Add, Side::Bid, 100_000_000_000, 0);
        assert!(invalid.validate().is_err());
    }

    // =========================================================================
    // BookConsistency tests
    // =========================================================================

    #[test]
    fn test_book_consistency_valid() {
        let consistency = BookConsistency::Valid;
        assert!(consistency.is_valid());
        assert!(!consistency.is_crossed());
        assert!(!consistency.is_locked());
        assert!(!consistency.is_empty());
    }

    #[test]
    fn test_book_consistency_crossed() {
        let consistency = BookConsistency::Crossed;
        assert!(!consistency.is_valid());
        assert!(consistency.is_crossed());
        assert!(!consistency.is_locked());
        assert!(!consistency.is_empty());
    }

    #[test]
    fn test_book_consistency_locked() {
        let consistency = BookConsistency::Locked;
        assert!(!consistency.is_valid());
        assert!(!consistency.is_crossed());
        assert!(consistency.is_locked());
        assert!(!consistency.is_empty());
    }

    #[test]
    fn test_book_consistency_empty() {
        let consistency = BookConsistency::Empty;
        assert!(!consistency.is_valid());
        assert!(!consistency.is_crossed());
        assert!(!consistency.is_locked());
        assert!(consistency.is_empty());
    }

    // =========================================================================
    // LobState Consistency Validation tests
    // =========================================================================

    #[test]
    fn test_lob_state_consistency_valid() {
        let mut state = LobState::new(10);
        state.best_bid = Some(100_000_000_000); // $100.00
        state.best_ask = Some(100_010_000_000); // $100.01

        assert_eq!(state.check_consistency(), BookConsistency::Valid);
        assert!(state.is_consistent());
        assert!(!state.is_crossed());
        assert!(!state.is_locked());
        assert!(state.validate_consistency().is_ok());
    }

    #[test]
    fn test_lob_state_consistency_crossed() {
        let mut state = LobState::new(10);
        state.best_bid = Some(100_010_000_000); // $100.01 (bid > ask!)
        state.best_ask = Some(100_000_000_000); // $100.00

        assert_eq!(state.check_consistency(), BookConsistency::Crossed);
        assert!(!state.is_consistent());
        assert!(state.is_crossed());
        assert!(!state.is_locked());

        let err = state.validate_consistency();
        assert!(err.is_err());
        assert!(matches!(
            err.unwrap_err(),
            crate::error::TlobError::CrossedQuote(_, _)
        ));
    }

    #[test]
    fn test_lob_state_consistency_locked() {
        let mut state = LobState::new(10);
        state.best_bid = Some(100_000_000_000); // $100.00
        state.best_ask = Some(100_000_000_000); // $100.00 (same as bid!)

        assert_eq!(state.check_consistency(), BookConsistency::Locked);
        assert!(!state.is_consistent());
        assert!(!state.is_crossed());
        assert!(state.is_locked());

        let err = state.validate_consistency();
        assert!(err.is_err());
        assert!(matches!(
            err.unwrap_err(),
            crate::error::TlobError::LockedQuote(_, _)
        ));
    }

    #[test]
    fn test_lob_state_consistency_empty() {
        let state = LobState::new(10);

        assert_eq!(state.check_consistency(), BookConsistency::Empty);
        assert!(!state.is_consistent());
        assert!(!state.is_crossed());
        assert!(!state.is_locked());
        // Empty book should not produce an error
        assert!(state.validate_consistency().is_ok());
    }

    #[test]
    fn test_lob_state_consistency_partial_empty() {
        // Only bid side filled
        let mut state = LobState::new(10);
        state.best_bid = Some(100_000_000_000);
        assert_eq!(state.check_consistency(), BookConsistency::Empty);

        // Only ask side filled
        let mut state = LobState::new(10);
        state.best_ask = Some(100_010_000_000);
        assert_eq!(state.check_consistency(), BookConsistency::Empty);
    }

    // =========================================================================
    // LobState Basic Analytics tests
    // =========================================================================

    #[test]
    fn test_lob_state_mid_price() {
        let mut state = LobState::new(10);
        state.best_bid = Some(100_000_000_000); // $100.00
        state.best_ask = Some(100_010_000_000); // $100.01

        let mid = state.mid_price().unwrap();
        assert!((mid - 100.005).abs() < 1e-6);
    }

    #[test]
    fn test_lob_state_mid_price_empty() {
        let state = LobState::new(10);
        assert!(state.mid_price().is_none());
    }

    #[test]
    fn test_lob_state_spread() {
        let mut state = LobState::new(10);
        state.best_bid = Some(100_000_000_000); // $100.00
        state.best_ask = Some(100_010_000_000); // $100.01

        let spread = state.spread().unwrap();
        assert!((spread - 0.01).abs() < 1e-6);
    }

    #[test]
    fn test_lob_state_spread_bps() {
        let mut state = LobState::new(10);
        state.best_bid = Some(100_000_000_000); // $100.00
        state.best_ask = Some(100_010_000_000); // $100.01

        let spread_bps = state.spread_bps().unwrap();
        assert!((spread_bps - 1.0).abs() < 0.01); // ~1 bps
    }

    // =========================================================================
    // LobState Enriched Analytics tests
    // =========================================================================

    #[test]
    fn test_lob_state_microprice() {
        let mut state = LobState::new(10);
        state.best_bid = Some(100_000_000_000); // $100.00
        state.best_ask = Some(100_020_000_000); // $100.02
        state.bid_sizes[0] = 100;
        state.ask_sizes[0] = 100;

        // Equal sizes: microprice should equal mid-price
        let microprice = state.microprice().unwrap();
        let mid = state.mid_price().unwrap();
        assert!((microprice - mid).abs() < 1e-6);
    }

    #[test]
    fn test_lob_state_microprice_weighted() {
        let mut state = LobState::new(10);
        state.best_bid = Some(100_000_000_000); // $100.00
        state.best_ask = Some(100_020_000_000); // $100.02
        state.bid_sizes[0] = 100; // Small bid
        state.ask_sizes[0] = 300; // Large ask

        // More volume on ask side: microprice should be closer to bid
        // microprice = (100.00 * 300 + 100.02 * 100) / 400 = 100.005
        let microprice = state.microprice().unwrap();
        assert!((microprice - 100.005).abs() < 1e-6);
    }

    #[test]
    fn test_lob_state_microprice_empty() {
        let state = LobState::new(10);
        assert!(state.microprice().is_none());

        // Has prices but no sizes
        let mut state = LobState::new(10);
        state.best_bid = Some(100_000_000_000);
        state.best_ask = Some(100_010_000_000);
        assert!(state.microprice().is_none());
    }

    #[test]
    fn test_lob_state_total_volume() {
        let mut state = LobState::new(3);
        state.bid_sizes[0] = 100;
        state.bid_sizes[1] = 200;
        state.bid_sizes[2] = 50;
        state.ask_sizes[0] = 150;
        state.ask_sizes[1] = 100;
        state.ask_sizes[2] = 75;

        assert_eq!(state.total_bid_volume(), 350);
        assert_eq!(state.total_ask_volume(), 325);
    }

    #[test]
    fn test_lob_state_depth_imbalance() {
        let mut state = LobState::new(3);

        // Balanced book
        state.bid_sizes[0] = 100;
        state.bid_sizes[1] = 100;
        state.bid_sizes[2] = 100;
        state.ask_sizes[0] = 100;
        state.ask_sizes[1] = 100;
        state.ask_sizes[2] = 100;
        let imbalance = state.depth_imbalance().unwrap();
        assert!((imbalance - 0.0).abs() < 1e-6);

        // More bids (positive imbalance)
        state.bid_sizes[0] = 200;
        state.bid_sizes[1] = 100;
        state.bid_sizes[2] = 100;
        state.ask_sizes[0] = 100;
        state.ask_sizes[1] = 100;
        state.ask_sizes[2] = 100;
        let imbalance = state.depth_imbalance().unwrap();
        assert!(imbalance > 0.0);
        assert!((imbalance - 0.142857).abs() < 0.001); // (400-300)/700

        // More asks (negative imbalance)
        state.bid_sizes[0] = 100;
        state.bid_sizes[1] = 100;
        state.bid_sizes[2] = 100;
        state.ask_sizes[0] = 200;
        state.ask_sizes[1] = 200;
        state.ask_sizes[2] = 200;
        let imbalance = state.depth_imbalance().unwrap();
        assert!(imbalance < 0.0);
        // (300-600)/900 = -300/900 = -1/3 ≈ -0.333
        assert!((imbalance - (-0.333333)).abs() < 0.001);
    }

    #[test]
    fn test_lob_state_depth_imbalance_empty() {
        let state = LobState::new(3);
        assert!(state.depth_imbalance().is_none());
    }

    #[test]
    fn test_lob_state_vwap() {
        let mut state = LobState::new(3);

        // Bid side: 100 @ $100, 200 @ $99, 50 @ $98
        state.bid_prices[0] = 100_000_000_000;
        state.bid_prices[1] = 99_000_000_000;
        state.bid_prices[2] = 98_000_000_000;
        state.bid_sizes[0] = 100;
        state.bid_sizes[1] = 200;
        state.bid_sizes[2] = 50;

        // VWAP = (100*100 + 99*200 + 98*50) / (100+200+50) = (10000+19800+4900) / 350 = 99.14...
        let vwap_all = state.vwap_bid(3).unwrap();
        assert!((vwap_all - 99.142857).abs() < 0.001);

        // VWAP for top 2 levels = (100*100 + 99*200) / 300 = 99.333...
        let vwap_2 = state.vwap_bid(2).unwrap();
        assert!((vwap_2 - 99.333333).abs() < 0.001);

        // VWAP for top 1 level = 100
        let vwap_1 = state.vwap_bid(1).unwrap();
        assert!((vwap_1 - 100.0).abs() < 1e-6);
    }

    #[test]
    fn test_lob_state_vwap_empty() {
        let state = LobState::new(3);
        assert!(state.vwap_bid(3).is_none());
        assert!(state.vwap_ask(3).is_none());
    }

    #[test]
    fn test_lob_state_weighted_mid() {
        let mut state = LobState::new(2);

        // Bid: 100 @ $100, 100 @ $99 → VWAP = 99.5
        state.bid_prices[0] = 100_000_000_000;
        state.bid_prices[1] = 99_000_000_000;
        state.bid_sizes[0] = 100;
        state.bid_sizes[1] = 100;

        // Ask: 100 @ $101, 100 @ $102 → VWAP = 101.5
        state.ask_prices[0] = 101_000_000_000;
        state.ask_prices[1] = 102_000_000_000;
        state.ask_sizes[0] = 100;
        state.ask_sizes[1] = 100;

        // Weighted mid = (99.5 + 101.5) / 2 = 100.5
        let weighted_mid = state.weighted_mid(2).unwrap();
        assert!((weighted_mid - 100.5).abs() < 1e-6);
    }

    #[test]
    fn test_lob_state_active_levels() {
        let mut state = LobState::new(5);
        state.bid_sizes[0] = 100;
        state.bid_sizes[1] = 0;
        state.bid_sizes[2] = 50;
        state.bid_sizes[3] = 0;
        state.bid_sizes[4] = 0;
        state.ask_sizes[0] = 100;
        state.ask_sizes[1] = 200;
        state.ask_sizes[2] = 0;
        state.ask_sizes[3] = 0;
        state.ask_sizes[4] = 50;

        assert_eq!(state.active_bid_levels(), 2);
        assert_eq!(state.active_ask_levels(), 3);
    }

    #[test]
    fn test_lob_state_new_fields() {
        let mut state = LobState::new(10);

        // Test timestamp and message_index fields
        assert!(state.timestamp.is_none());
        assert_eq!(state.message_index, 0);

        state.timestamp = Some(1234567890_000_000_000);
        state.message_index = 42;

        assert_eq!(state.timestamp, Some(1234567890_000_000_000));
        assert_eq!(state.message_index, 42);
    }

    // =========================================================================
    // LobState Temporal Fields tests
    // =========================================================================

    #[test]
    fn test_lob_state_temporal_fields_initialized() {
        let state = LobState::new(10);

        // All temporal fields should be initialized to None/0
        assert!(state.previous_timestamp.is_none());
        assert_eq!(state.delta_ns, 0);
        assert!(state.triggering_action.is_none());
        assert!(state.triggering_side.is_none());
    }

    #[test]
    fn test_lob_state_delta_seconds() {
        let mut state = LobState::new(10);

        // No delta: should return None
        assert!(state.delta_seconds().is_none());

        // Set delta to 1 second (1e9 nanoseconds)
        state.delta_ns = 1_000_000_000;
        let delta = state.delta_seconds().unwrap();
        assert!((delta - 1.0).abs() < 1e-6);

        // Set delta to 100 milliseconds
        state.delta_ns = 100_000_000;
        let delta = state.delta_seconds().unwrap();
        assert!((delta - 0.1).abs() < 1e-6);

        // Set delta to 1 microsecond
        state.delta_ns = 1_000;
        let delta = state.delta_seconds().unwrap();
        assert!((delta - 1e-6).abs() < 1e-9);
    }

    #[test]
    fn test_lob_state_event_intensity() {
        let mut state = LobState::new(10);

        // No delta: should return None
        assert!(state.event_intensity().is_none());

        // 1 second delta = 1 event/second
        state.delta_ns = 1_000_000_000;
        let intensity = state.event_intensity().unwrap();
        assert!((intensity - 1.0).abs() < 1e-6);

        // 100ms delta = 10 events/second
        state.delta_ns = 100_000_000;
        let intensity = state.event_intensity().unwrap();
        assert!((intensity - 10.0).abs() < 1e-6);

        // 1ms delta = 1000 events/second
        state.delta_ns = 1_000_000;
        let intensity = state.event_intensity().unwrap();
        assert!((intensity - 1000.0).abs() < 1e-6);
    }

    #[test]
    fn test_lob_state_was_triggered_by() {
        let mut state = LobState::new(10);

        // No action set
        assert!(!state.was_triggered_by(Action::Add));
        assert!(!state.was_triggered_by(Action::TradeAggregate));

        // Set to Add
        state.triggering_action = Some(Action::Add);
        assert!(state.was_triggered_by(Action::Add));
        assert!(!state.was_triggered_by(Action::TradeAggregate));
        assert!(!state.was_triggered_by(Action::Cancel));

        // Set to Trade
        state.triggering_action = Some(Action::TradeAggregate);
        assert!(!state.was_triggered_by(Action::Add));
        assert!(state.was_triggered_by(Action::TradeAggregate));
    }

    #[test]
    fn test_lob_state_was_triggered_on_side() {
        let mut state = LobState::new(10);

        // No side set
        assert!(!state.was_triggered_on_bid());
        assert!(!state.was_triggered_on_ask());

        // Set to Bid
        state.triggering_side = Some(Side::Bid);
        assert!(state.was_triggered_on_bid());
        assert!(!state.was_triggered_on_ask());

        // Set to Ask
        state.triggering_side = Some(Side::Ask);
        assert!(!state.was_triggered_on_bid());
        assert!(state.was_triggered_on_ask());
    }

    #[test]
    fn test_lob_state_event_type_checks() {
        let mut state = LobState::new(10);

        // No action
        assert!(!state.is_aggregate_trade_event());
        assert!(!state.is_resting_fill_event());
        assert!(!state.is_add_event());
        assert!(!state.is_cancel_event());

        // Aggregate trade print (`T`) — aggressor-side view.
        state.triggering_action = Some(Action::TradeAggregate);
        assert!(state.is_aggregate_trade_event());
        assert!(
            !state.is_resting_fill_event(),
            "TradeAggregate must NOT satisfy the resting-fill predicate: the two are DISJOINT \
             views of one physical execution. A union predicate here re-merges the carriers."
        );
        assert!(!state.is_add_event());
        assert!(!state.is_cancel_event());

        // Resting-order fill (`F`) — resting-side view, the OPPOSITE side convention.
        state.triggering_action = Some(Action::Fill);
        assert!(state.is_resting_fill_event());
        assert!(
            !state.is_aggregate_trade_event(),
            "Fill must NOT satisfy the aggregate-trade predicate. This assertion is the lock on \
             the T/F split: it fails the moment either predicate is widened back to a union."
        );
        assert!(!state.is_add_event());
        assert!(!state.is_cancel_event());

        // Add event
        state.triggering_action = Some(Action::Add);
        assert!(!state.is_aggregate_trade_event());
        assert!(!state.is_resting_fill_event());
        assert!(state.is_add_event());
        assert!(!state.is_cancel_event());

        // Cancel event
        state.triggering_action = Some(Action::Cancel);
        assert!(!state.is_aggregate_trade_event());
        assert!(!state.is_resting_fill_event());
        assert!(!state.is_add_event());
        assert!(state.is_cancel_event());
    }

    #[test]
    fn test_lob_state_temporal_combined() {
        let mut state = LobState::new(10);

        // Simulate a sequence of updates
        state.timestamp = Some(1_000_000_000);
        state.previous_timestamp = None;
        state.delta_ns = 0;
        state.triggering_action = Some(Action::Add);
        state.triggering_side = Some(Side::Bid);

        // First update: no previous, so delta is 0
        assert_eq!(state.delta_ns, 0);
        assert!(state.delta_seconds().is_none()); // delta_ns is 0
        assert!(state.is_add_event());
        assert!(state.was_triggered_on_bid());

        // Simulate second update
        state.previous_timestamp = Some(1_000_000_000);
        state.timestamp = Some(1_001_000_000); // 1ms later
        state.delta_ns = 1_000_000; // 1ms
        state.triggering_action = Some(Action::TradeAggregate);
        state.triggering_side = Some(Side::Ask);

        assert!((state.delta_seconds().unwrap() - 0.001).abs() < 1e-9);
        assert!((state.event_intensity().unwrap() - 1000.0).abs() < 1e-6);
        assert!(state.is_aggregate_trade_event());
        assert!(state.was_triggered_on_ask());
    }

    #[test]
    fn test_lob_state_size_unchanged() {
        // Verify that LobState size is reasonable after adding temporal fields
        let state = LobState::new(10);
        let size = std::mem::size_of_val(&state);

        // Should be around 560-600 bytes (increased from ~520)
        assert!(size > 500, "LobState too small: {} bytes", size);
        assert!(size < 700, "LobState too large: {} bytes", size);
    }
}
