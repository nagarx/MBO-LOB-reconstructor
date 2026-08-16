# LOB Reconstructor Warnings & Issues Reference

This document catalogs known warnings, issues, and edge cases that may occur during LOB reconstruction. Use this as a reference when debugging preprocessing issues or investigating anomalies in live environments.

## Warning Categories

### 1. ORDER_NOT_FOUND

**Severity**: ~~Low (1)~~ → **HIGH — this is a decoder bug signature, not a market property (corrected 2026-08-01).**

**Description**: A cancel or trade message references an order that doesn't exist in the book.

> ⚠️ **CORRECTION 2026-08-01.**
> **OLD (this section, and `CLAUDE.md` / `CODEBASE.md`):** "~0.5% `cancel_order_not_found` — normal";
> causes listed as late delivery / missing snapshot / duplicate cancels; "Impact: None".
> **NEW: 100% of the observed mass is an artifact of the `b'T' | b'F' => Action::Trade` merge at
> `src/dbn_bridge.rs:125`, which lets a fill (`F`) mutate the book that the vendor spec says it must
> not touch.** On 2025-02-03, **473,410 of 473,410 (100.000%)** `F` rows are followed by a `C`
> carrying an identical order_id, size, timestamp and side. A full fill (349,499/day) has already
> deleted the order, so its paired `C` finds nothing; a partial fill (90,618/day) double-decrements
> and exhausts the order early. A corrected replay treating `F` as a book no-op yields **exactly
> zero** on two independent days:
>
> | counter | 2025-02-03 shipped → corrected | 2025-07-01 shipped → corrected |
> |---|---|---|
> | `cancel_order_not_found` | 393,790 → **0** | 261,386 → **0** |
> | `trade_order_not_found`  |  33,293 → **0** |  18,061 → **0** |
>
> **These counters are the free acceptance test for the pending decoder fix (DECISION-033 / Phase 1):
> post-fix they must read EXACTLY 0 — not "≈0", not "reduced".** Until then, treat any non-zero
> value as an open defect, not as expected feed behaviour. The "Common Causes" list below is
> retained as the *pre-correction* hypothesis and is NOT supported by the measurement.

> **FINDING-122 scope boundary (validated 2026-08-01).** The decoder merge has
> two different observed consequences. A direct raw-tape consumer sees total
> signed-direction annihilation because `T` carries aggressor side and `F`
> carries resting side. The current NVDA/XNAS feature-extractor path does not
> see that merged population: its separate structural filter drops exactly the
> true-Trade rows (`order_id == 0`) and leaves Fills. Those two defects cancel
> exactly for sign on that bounded path, so existing direction closures are not
> decoder artifacts; the path instead loses coverage. Do not generalize this
> result beyond the named current producer/data path, and do not describe it as
> a property of direct raw-tape consumers. After producer behavior changes, the
> cancellation statement is historical by construction.

**Common Causes** *(superseded — see correction above; retained for history)*:
- Order was already cancelled by a previous message
- Late/out-of-order message delivery
- Order existed before the data feed started (snapshot missing)
- Duplicate cancel messages

**Impact**: ~~None - the operation is skipped safely.~~ The skipped operation is safe *locally*, but the
underlying `F` mutation has already corrupted the book transiently (see the Validation Results section
below).

**Tracked In**: `LobStats.cancel_order_not_found`, `LobStats.trade_order_not_found`

---

### 2. PRICE_LEVEL_NOT_FOUND

**Severity**: Medium (2)

**Description**: An order exists in tracking but its price level doesn't exist in the book.

**Common Causes**:
- Data inconsistency between order tracking and price levels
- Race condition in message processing
- Corrupted or incomplete data

**Impact**: Low - the orphaned order is cleaned up automatically.

**Tracked In**: `LobStats.cancel_price_level_missing`, `LobStats.trade_price_level_missing`

---

### 3. ORDER_AT_LEVEL_MISSING

**Severity**: Medium (2)

**Description**: Price level exists but the specific order is not found at that level.

**Common Causes**:
- Order was moved to a different price (modify without proper tracking)
- Data feed issue
- Internal state corruption

**Impact**: Low - the orphaned order is cleaned up automatically.

**Tracked In**: `LobStats.cancel_order_at_level_missing`, `LobStats.trade_order_at_level_missing`

---

### 4. BOOK_CLEARED

**Severity**: Low (1)

**Description**: An `Action::Clear` message was received, resetting the entire order book.

**Common Causes**:
- Market session transition (pre-market → regular → after-hours)
- Trading halt/resume
- Exchange system reset
- End of trading day

**Impact**: Normal operation - book is reset and continues processing.

**Tracked In**: `LobStats.book_clears`

**Note** (Phase O Cycle 1 / B.2a — 2026-05-03): wire `R` means clear all
orders for the instrument. It legitimately has the zero-shaped fields that
also satisfy the crate-local `is_system_message()` heuristic. Pre-B.2a,
default-config callers filtered Clear before dispatch, so the book never reset
and `LobStats.book_clears` stayed zero. Current v0.3.0 code exempts
`Action::Clear` from the inner structural filter and message validation; the
sibling extractor carries the companion outer-filter exemption. This is why
`order_id == 0 || size == 0 || price <= 0` must not be documented as a universal
DBN heartbeat/status taxonomy. Preserve the dated closure record in
`CHANGELOG.md [0.2.1]`.

---

### 5. CROSSED_QUOTES

**Severity**: High (3)

**Description**: Best bid price is greater than or equal to best ask price.

**Common Causes**:
- Aggressive order placement
- Market maker repositioning
- Data timing issues
- Locked market (bid == ask)

**Impact**: Depends on policy:
- `CrossedQuotePolicy::Allow` - Accepts crossed state
- `CrossedQuotePolicy::UseLastValid` - Returns last valid state
- `CrossedQuotePolicy::Error` - Returns error

**Tracked In**: `LobStats.crossed_quotes`, `LobStats.locked_quotes`

---

### 6. NOOP_MESSAGES

**Severity**: Low (1)

**Description**: `Action::None` messages received (no-op).

**Common Causes**:
- Heartbeat messages
- Flag-only messages
- Placeholder messages from exchange

**Impact**: None - message is ignored.

**Tracked In**: `LobStats.noop_messages`

---

## Known Edge Cases

### Pre-Market Session Start ($4.97 Error Pattern)

**Observation**: At market session boundaries (especially 04:00 AM ET), large price discrepancies (~$4.97) may appear briefly.

**Root Cause**: The MBO data stream may start mid-session without a complete book snapshot. The reconstructor builds state incrementally, so early snapshots may be incomplete.

**Mitigation**: 
- Wait for book to stabilize before using data
- Use `Action::Clear` messages as session boundary markers
- Consider skipping first N messages after session start

**Status**: Expected behavior, not a bug.

---

### Partial Cancel Handling

**Fixed in**: v0.1.0

**Previous Issue**: Partial cancellations were treated as full cancellations, removing orders entirely instead of reducing their size.

**Current Behavior**: Partial cancels correctly reduce order size. If `msg.size >= order_size`, the order is fully removed.

---

### ⚠️ THE 2026-08-01 CORRECTION IS ITSELF SUPERSEDED (2026-08-12)

**Every `95.56% / 95.73%` figure in this file, in `README.md`, in `CODEBASE.md` and in root
`CLAUDE.md` comes from `data/validation_results_july2025.json`, and THAT ARTIFACT HAS NO EMITTING
CODE.** Measured 2026-08-12, unscoped over the whole tree: **68 files reference it and every single
one is a `.md`** — not one `.rs`, `.py` or `.sh`. It is hand-transcribed, dated `2025-12-01`, and
its own `book_state_events.book_clears` is **21** where today's reconstructor emits **0** — i.e. it
describes a book the current code does not produce.

So the 2026-08-01 correction replaced one unsourced number (`99.17%`) with **another number from an
artifact with no emitter**. Both are provenance-free. Do not quote either as a current measurement.

**WHAT TO CITE INSTEAD — there is now a real, reproducible measurement.** The ten-level MBP-10
oracle (2026-08-11), script and frozen per-day verdicts git-backed at
`hft-wiki/audit/2026-08-11-mbo-backbone-second-opinion/evidence/phase8_census_oracle/`
(`O1_mbp10_10level_scratch/oracle10.py`, report `O1-mbp10-oracle-10-level.md`):

| arm | ten-level conformance vs the vendor MBP-10 |
|---|---|
| candidate — `F` as a book **no-op** (the router fix) | **100.000%**, 465,065,790 level-comparisons, **0 misses**, 14 days, price bit-identical L1 and L10 |
| shipped — `F` merged into `Trade` | **83.632% at L1 → 94.935% at L10** (stratum A) |

The shipped book **improving with depth** is the F-merge signature: a fill hits the resting order
**at the touch**, so the damage concentrates at L1. It also establishes there is **no second,
depth-specific book-construction defect**.

⚠️ SCOPE, stated because the numbers above are strong: 14 of 21 available days (input set
deliberately frozen), **NVDA / XNAS / July-2025 only**, and the `T` stratum (6.19%) excluded exactly
as the shipped gate excludes it. There is **no ARCX MBP-10 anywhere on the data volume** — a
full-volume search for `*mbp*10*` returns 21 XNAS + 20 GLBX + **0 ARCX** — so no external-oracle
conformance claim can be made for ARCX at all, and the two venues are structurally different (XNAS
carries 0 filter-escaping `T` records on 12/12 sampled days; ARCX carries 25,901–97,956 per day,
100.0000% `side='N'`).

---

### Size Estimation Variance

**Observation** *(corrected 2026-08-01; ⚠️ its SOURCE is superseded — see the block above)*: OLD — "~91% exact match vs ~99% for prices".
NEW — the source artifact reports size exact-match **83.66% bid / 83.06% ask** against price
exact-match **95.56% bid / 95.73% ask**. The old pair (91%/99%) is not in the artifact.

**Root Cause** *(corrected)*: OLD — "size aggregation timing differences between MBO reconstruction
and MBP-10 snapshots". NEW — that attribution is falsified. Databento's own MBP-10 contains **zero
`F` records**; re-running the comparison with `F` treated as a book no-op reproduces the vendor book
**exactly** on 100.000% of book-affecting records (2025-07-01, 4,214,602 RTH comparisons:
`A` 2,071,194 and `C` 1,893,575 rows both go to 100.000% price and 100.000% size, from
99.700%/86.549% and 99.303%/84.307%). The shortfall is the `F`-merge defect, not aggregation timing.

**Impact** *(corrected)*: NOT minor at sequence resolution. The distortion is transient — it lives
between each `F` and its paired `C` — but feature sampling lands on those positions. Shipped-corpus
distortion on 2025-02-03: best-bid wrong 0.741%, best-ask 1.016%, **mid wrong 1.753%**, L1 sizes
7.2%/10.4%, spread systematically **+1.093% too wide**. End-of-day state is identical either way,
which is why day-level checks looked clean.

---

## Exporting Warnings

### Using LobStats

```rust
use mbo_lob_reconstructor::LobReconstructor;

let mut lob = LobReconstructor::new(10);
// ... process messages ...

// Check for warnings
if lob.stats().has_warnings() {
    println!("Total warnings: {}", lob.stats().total_warnings());
}

// Export to file
lob.stats().export_to_file("preprocessing_warnings.json")?;
```

### Using WarningTracker (Advanced)

```rust
use mbo_lob_reconstructor::warnings::{WarningTracker, WarningCategory};

let mut tracker = WarningTracker::new();

// Record custom warnings
tracker.record_order_warning(
    WarningCategory::OrderNotFound,
    "Order 12345 not found during cancel",
    12345,
    Some(100_000_000_000), // price
    Some(1234567890_000_000_000), // timestamp
);

// Export to JSON
tracker.export_to_file("detailed_warnings.json")?;

// Export to CSV for spreadsheet analysis
tracker.export_to_csv("warnings.csv")?;

// Get summary
let summary = tracker.summary();
println!("Total: {}, Unique orders: {}", summary.total, summary.unique_orders);
```

---

## Validation Results Summary

Based on July 2025 NVIDIA data (MBO vs MBP-10), 21 trading days, 88,062,096 aligned comparisons.
**Source artifact: `data/validation_results_july2025.json` (repo root `data/`).**

> ⚠️ **CORRECTED 2026-08-01 — the previously published table did not match its own source artifact.**
> OLD (published here, and echoed in `README.md`, `CODEBASE.md` and root `CLAUDE.md` as
> "BBO 99.17%"): BBO Price Match **99.17%** · BBO Size Match **91.15%** · Price Within 1¢ **99.71%** ·
> MAE **$0.000095** · Regular Hours Accuracy **99.69%**.
> NEW (read directly out of `validation_results_july2025.json`): **none of those five numbers appears
> in the artifact.** The artifact's actual values are in the table below. There is no
> "regular hours accuracy" field and no by-session breakdown in it at all, so the "By Time of Day"
> table that used to sit here has been withdrawn as unsourced.

| Metric (field in `validation_results_july2025.json`) | Value |
|--------|-------|
| `price_accuracy.bid_exact_match_pct` | **95.56%** |
| `price_accuracy.ask_exact_match_pct` | **95.73%** |
| `price_accuracy.bid_within_1c_pct` | 98.51% |
| `price_accuracy.ask_within_1c_pct` | 98.56% |
| `price_accuracy.bid_mae_dollars` | $0.000498 |
| `price_accuracy.ask_mae_dollars` | $0.000479 |
| `size_accuracy.bid_exact_match_pct` | **83.66%** |
| `size_accuracy.ask_exact_match_pct` | **83.06%** |
| `warnings.cancel_order_not_found_rate_pct` | 2.9879% |
| `warnings.trade_order_not_found_rate_pct` | 4.6236% |

### ⚠️ The acceptance gate could not fail

The artifact's own assertions are `bid_size_exact_gt_80pct` / `ask_size_exact_gt_80pct` — an **80%**
threshold set **just under the measured 83.66% / 83.06%**. Since the size shortfall is a signature of
the `F`-merge defect (§1 ORDER_NOT_FOUND above), the gate was positioned below the bug and therefore
**could never have failed on it**. The artifact's `notes` likewise explain both bug signatures away
as benign ("'Order not found' warnings are normal for aggressor side trades"; "Size differences are
due to MBO vs MBP-10 aggregation timing differences") — both explanations are now falsified.
**Treat this artifact as a record of the pre-fix state, not as a passing validation.**
Re-run it after the decoder fix with a gate set from the corrected replay, not from the buggy value.

---

## Debugging Checklist

1. **Check warning counts**: `lob.stats().total_warnings()`
2. **Check for book clears**: `lob.stats().book_clears`
3. **Check crossed quotes**: `lob.stats().crossed_quotes`
4. **Export detailed stats**: `lob.stats().export_to_file("debug.json")`
5. **Enable logging**: Use `LobConfig::new(10).with_logging(true)`
6. **Check timestamps**: Ensure data is in chronological order

---

## Contact

For issues not covered here, please:
1. Check the test suite: `cargo test -p mbo-lob-reconstructor`
2. Run validation: `cargo test --test integration_test --release --features databento`
3. Open an issue with:
   - Data sample (anonymized if needed)
   - Warning counts from `LobStats`
   - Expected vs actual behavior

---

## BBO accuracy and the MBP-10 oracle — what may and may not be cited

> **RELOCATED HERE 2026-08-16 from root `CLAUDE.md` §Pipeline Overview.** It was a module-scoped
> warning (NVDA/XNAS/July-2025, `T` stratum excluded, zero ARCX MBP-10) being paid for by every
> agent on every turn, including agents working on the backtester, the wiki, or crypto. The oracle
> half was already duplicated below; **the D1 material existed ONLY in the root file** — measured
> 2026-08-16: `grep -c D1_two_day WARNINGS.md` -> 0, `grep -c D1_two_day CLAUDE.md` -> 2. Root now
> carries a one-line pointer here.

🔴 **SUPERSEDED 2026-08-12 — THE CORRECTION BELOW IS ITSELF PROVENANCE-FREE.** Its source,
`data/validation_results_july2025.json`, **HAS NO EMITTING CODE.** Measured unscoped over the whole
tree: **68 files reference it and every one is a `.md`** — not one `.rs`, `.py` or `.sh`. It is
hand-transcribed, dated `2025-12-01`, and its own `book_clears` is **21** where today's
reconstructor emits **0**, so it describes a book the current code does not produce. The
2026-08-01 fix therefore replaced one unsourced number with another. **Quote NEITHER `99.17%` NOR
`95.56%/95.73%` as a current measurement.**
**CITE INSTEAD** the ten-level MBP-10 oracle (2026-08-11; script + frozen verdicts git-backed at
`hft-wiki/audit/2026-08-11-mbo-backbone-second-opinion/evidence/phase8_census_oracle/`):
the candidate `F`-as-book-no-op arm reaches **100.000%** ten-level conformance with the vendor
MBP-10 — **465,065,790 level-comparisons, 0 misses, 14 days**, price bit-identical L1 and L10 —
while the **shipped** book measures **83.632% at L1 rising to 94.935% at L10** (stratum A). The
rise with depth IS the F-merge signature (a fill hits the resting order at the touch) and shows
there is **no second, depth-specific book defect**. Scope: 14/21 days, **NVDA/XNAS/July-2025 only**,
`T` stratum (6.19%) excluded; **there is no ARCX MBP-10 on the data volume at all** (`*mbp*10*` →
21 XNAS + 20 GLBX + **0 ARCX**). Full detail: `MBO-LOB-reconstructor/WARNINGS.md`.

🔴 **AND ONE MORE SCOPE LINE, ADDED 2026-08-13 — THE ORACLE IS A PYTHON PORT. IT QUALIFIES THE
SEMANTICS, NOT THE RUST.** Orchestrator-verified: all **3** scripts in that evidence directory
contain **0** occurrences of `cargo`/`rustc`/`target/`; the only subprocess they launch is
`dbn.cli_path()` (the Databento CLI); `oracle10.py:23` self-declares *"an independent 10-level
**Python** book"* and `:169` *"the 10-level independent **Python book port**"*. **Re-running it
after editing `MBO-LOB-reconstructor/src/dbn_bridge.rs` or `reconstructor.rs` produces
byte-identical output whether a fix is applied, unapplied, or applied WRONG.** The programme's own
packet says the same: `contracts/mbo_backbone/d0_evidence_receipt_v1.json` carries
`authorizes: "nothing"`, `status: "observed_pass_nonadmitting"`, and the limitation *"the oracle
evaluates a **Python candidate** … **not actual Rust candidate behavior**."*
✅ **What it DOES establish stands and is valuable**: the candidate `F`-as-book-no-op SEMANTICS
reproduce the vendor MBP-10 exactly, and the shipped book's rise with depth is the F-merge
signature. Both survive. **What it does NOT establish is that any Rust implementation of those
semantics is correct** — that is the packet's next pre-registered gate,
`D1_two_day_development_replay_with_actual_Rust_subject`.
⚠️ **SCOPE ADDED 2026-08-14 — "the next pre-registered gate" is TRUE OF THE LIST AND MISLEADING AS
A PLAN. D1 IS NOT ACTIONABLE.** Measured: `grep -rn --no-ignore-files "D1_two_day"
contracts/mbo_backbone/` returns **1 line** — a bare string in `required_sequence` — and a
recursive walk for any D1-keyed entry across all **11** packet artifacts returns **0**. D0 has a
builder, a receipt, two evidence files and a Makefile target; **D1 has none of the four.** It is
specified in SCOPE, INPUTS and PROHIBITIONS, with **no METHOD and no ACCEPTANCE CRITERION**.
Worse, `phase_gates.transition_states` puts **`semantic_change_authorize` BEFORE `d1_candidate`**,
and Phase-0 closure was **unsatisfiable as coded** until 2026-08-14 (two validators demanded
contradictory values of one key; proven by experiment, settled fact **S35**). **Do not open the
parked candidate worktree and "run D1" — it would produce an unjudgeable artifact.** The authorized
work is specification: see `hft-wiki/audit/2026-08-11-mbo-backbone-second-opinion/06_SETTLED_REGISTER.md`
**S31–S39** and the continuation contract's KNOWN-WRONG **#26**.
⇒ **Never write "the oracle validates the reconstructor" or "the gate is open". Write "a Python
port of the candidate semantics matches the vendor MBP-10".**

⚠️ **BBO-accuracy correction (2026-08-01) — RETAINED AS THE RECORD, SUPERSEDED AS A FIGURE.** OLD: "BBO accuracy **99.17%**" (also quoted in
`MBO-LOB-reconstructor/{README,CLAUDE,CODEBASE}.md` and its `WARNINGS.md`). NEW: **best-price exact
match 95.56% bid / 95.73% ask; best-SIZE exact match 83.66% / 83.06%** — read directly out of the
claim's own source artifact, `data/validation_results_july2025.json` (21 days, 88,062,096 aligned
MBO-vs-MBP-10 comparisons). The numbers 99.17% / 91.15% / 99.71% / 99.69% do not appear anywhere in
that artifact. Two further problems recorded in `MBO-LOB-reconstructor/WARNINGS.md`: the artifact's
acceptance gate was set at **80%**, just under the buggy 83% size figure, so it could never fail;
and the shortfall is now attributed to the `T`/`F` merge in `dbn_bridge.rs:125` (F-as-no-op is
bit-exact to Databento's own MBP-10 on 100.000% of book-affecting records), not to the
"aggregation timing" the artifact's notes claimed.
