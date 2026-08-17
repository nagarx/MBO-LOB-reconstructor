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
> | counter | 2025-02-03 shipped → corrected | 2025-07-01 shipped → corrected | still a valid channel? |
> |---|---|---|---|
> | `cancel_order_not_found` | 393,790 → **0** | 261,386 → **0** | ✅ **YES — the primary falsifier** |
> | `trade_order_not_found`  |  33,293 → **0** |  18,061 → **0** | 🔴 **NO — zero increment sites post-L-ROUTE; reads 0 on any data. See the L-ROUTE block below.** |
>
> **The `cancel_order_not_found` row is the free acceptance test for the fix (DECISION-033 /
> Phase 1): post-fix it must read EXACTLY 0 — not "≈0", not "reduced".** Until then, treat any
> non-zero value as an open defect, not as expected feed behaviour. ⚠️ The `trade_order_not_found`
> row was written when that counter still had a writer; do **not** lift it as a passing channel now. The "Common Causes" list below is
> retained as the *pre-correction* hypothesis and is NOT supported by the measurement.>
> ⚠️ **SCOPE ADDED AT L-DECODE — THESE COUNTERS WERE STILL NON-ZERO, AND THAT WAS CORRECT.** The
> L-DECODE commit split the DECODER only (`b'T'` → `Action::TradeAggregate`, `b'F'` →
> `Action::Fill`); it deliberately did NOT change routing, so `LobReconstructor` still sent both
> carriers to `process_trade` and `F` still mutated the book. Measured on 2025-07-01, baseline vs
> the L-DECODE candidate: **all 18 reconstruction-stats fields identical**, `cancel_order_not_found`
> **261,386 in both arms**. The "EXACTLY 0" threshold is the acceptance test for **L-ROUTE**, not
> for L-DECODE. Do not read a non-zero counter *at L-DECODE* as evidence that the decode split
> failed — its acceptance signal is the `action`-column histogram (byte 84 splitting into
> {84, 70}), which passes.

> ⭐ **L-ROUTE HAS LANDED — THE ACCEPTANCE TEST ABOVE FIRED (commit `c9c6f60`, this branch).**
> `TradeAggregate` and `Fill` no longer reach `reduce_or_remove_order`; `process_trade` is
> **deleted**; `OrderReductionOp` collapsed to a single `Cancel` variant, so **`Cancel` is now
> provably the only action in this crate that reduces a resting order**. Measured, four counters ×
> two venues × seven days:
>
> | counter | XNAS 07-01 | XNAS 07-02 | ARCX 07-01 | ARCX 07-02 |
> |---|---|---|---|---|
> | `cancel_order_not_found` | 261,386 → **0** | 207,959 → **0** | 157,493 → **0** | 127,527 → **0** |
> | `modify_order_not_found` | — | — | 369 → **0** | 324 → **0** |
> | `active_orders` | 5,909 → 5,910 | 5,557 → 5,559 | — | — |
>
> Five held-out days (2025-07-03 / 09 / 16 / 23 / 30) are clean on every channel.
> **`cancel_order_not_found` is the PRIMARY falsifier** because its code path *survives* the
> commit — `Action::Cancel` still calls `reduce_or_remove_order` and its miss branch is still
> live — so reaching 0 is a **measurement**, not a removal. `modify_order_not_found` is a third
> independent counter on a path **invisible on XNAS** (XNAS.ITCH emits zero `M` bytes).
> `active_orders` is a **weak** channel (+1 on 5,909 = 0.017%); do not lean on it alone.
>
> 🔴 **`trade_order_not_found` 18,061 → 0 IS NOT A FALSIFIER — NEVER QUOTE IT AS A PASSING
> CHANNEL.** The three `trade_*` counters now have **ZERO increment sites** in production code, so
> they read 0 on **any** data, any venue, forever. A deliberately-wrong "route-to-not-found"
> impostor build scores 0 there too, so the channel carries **zero** discriminating power. What
> discriminates is the test suite: a correct fix reds exactly **seven** lib tests, an impostor reds
> only **five** (both counter-asserting canaries staying green). Measured on the landed commit:
> **293 passed / 7 failed**, with `test_trade_unknown_order_is_ok` and `test_warning_stats_accumulate`
> **both** red at `left: 0 / right: 1`, reproduced from two separately-constructed trees.


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
underlying `F` mutation had already corrupted the book transiently (see the Validation Results section
below). **Post-L-ROUTE the `F` mutation is gone**, and on the seven measured days the counter is 0.

**Tracked In**: `LobStats.cancel_order_not_found` — LIVE, written by `Cancel`, and the primary
acceptance channel above. ~~`LobStats.trade_order_not_found`~~ — **STRUCTURALLY DEAD post-L-ROUTE:
zero increment sites, reads 0 on any data forever.** It is retained deliberately, as the
machine-checkable receipt that no carrier enters the reduction path
(`tests/carrier_routing_discriminator.rs::assert_reduction_path_untaken` asserts exactly that).
Do **not** revive it, and do **not** read its 0 as a health signal. The signal it used to carry
moved to `LobStats.fill_referenced_unknown_order` — see §7 below.

---

### 2. PRICE_LEVEL_NOT_FOUND

**Severity**: Medium (2)

**Description**: An order exists in tracking but its price level doesn't exist in the book.

**Common Causes**:
- Data inconsistency between order tracking and price levels
- Race condition in message processing
- Corrupted or incomplete data

**Impact**: Low - the orphaned order is cleaned up automatically.

**Tracked In**: `LobStats.cancel_price_level_missing` — LIVE. ~~`LobStats.trade_price_level_missing`~~
— **STRUCTURALLY DEAD post-L-ROUTE (zero increment sites); see §1 "Tracked In".**

---

### 3. ORDER_AT_LEVEL_MISSING

**Severity**: Medium (2)

**Description**: Price level exists but the specific order is not found at that level.

**Common Causes**:
- Order was moved to a different price (modify without proper tracking)
- Data feed issue
- Internal state corruption

**Impact**: Low - the orphaned order is cleaned up automatically.

**Tracked In**: `LobStats.cancel_order_at_level_missing` — LIVE.
~~`LobStats.trade_order_at_level_missing`~~ — **STRUCTURALLY DEAD post-L-ROUTE (zero increment
sites); see §1 "Tracked In".**

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

### 7. RESTING-FILL CONFORMANCE — the fill oracle (NEW at L-ROUTE)

**Severity**: mixed — one alarm, one benign, two informational. Read the table, not the total.

**Description**: `Action::Fill` (wire `b'F'`) is a vendor **BOOK NO-OP** — the venue removes the
resting order itself, with a paired `Cancel` that is the literal next record. L-ROUTE stops the
`Fill` from mutating the book. It does **not** delete the lookup: every `F` is the vendor
**ASSERTING** a fact about the book —

> *order `order_id`, on side S, at price P, had at least `size` resting at this instant.*

— and `LobReconstructor::observe_resting_fill` now **CHECKS that assertion without mutating
anything**. That turns a discarded event into a continuous conformance oracle that runs on **every
venue and every day**, including ARCX, where no vendor MBP-10 covers most dates and where the
`Modify` path has never been independently graded.

**Why the removal is safe** — cite the pairing, never a "zero `F`" claim (see the strike under
*Size Estimation Variance*): `F`→`C` pairing measured at **1.00000000 over 6 days / 1,808,570
records**, the paired `C` being the literal next record with `side` and `size` matching, and the
identity **holds inside both auction crosses**, so **no auction carve-out is required**.
Independently, on ARCX 2025-02-03 (full day, 7,547,528 records) **356,515 of 356,515** `F` records
are immediately followed by a `C` with the same `order_id` **and** the same `size` — zero unpaired —
which is **18,220,516 shares** double-decremented on that day alone under the old routing.
Vendor publication rates on ARCX (MBO → vendor MBP-10) corroborate: `A` 75.90%, `C` 75.94%,
`T` 100.00%, but **`F` 0.0199% (37 of 185,706)**. A record that reduced a resting order the way `C`
does would publish at ~76%; `F` publishes ~3,800× less, because the vendor carries the book change
on the paired `C`.

**Counters** (all six are `#[serde(default)]`; see the schema note below):

| Counter | Meaning | Expected |
|---|---|---|
| `resting_fills_observed` | every `F` reaching the reconstructor | non-zero on every venue |
| `fill_referenced_unknown_order` | the vendor referenced an order the book does not hold | **0** |
| `fill_side_mismatch` | the RESTING side disagrees with the stored order | **0 — THIS ONE IS AN ALARM** |
| `fill_size_exceeded_resting` | vendor asserts more size than the book holds for that order | **0** |
| `fill_price_differs_from_resting` | execution price ≠ the resting order's DISPLAY price | **non-zero, benign (~3%/day)** |
| `aggregate_trades_observed` | every `TradeAggregate` (`b'T'`) reaching the reconstructor | see the ⚠ below |

⚠️ **SIDE and PRICE are two counters on purpose, and must never be merged.** A side disagreement is
a defect signature that must read 0 (it means either the book has the order on the wrong side, or
the `T`/`F` side conventions have been re-merged). A price difference is *expected*, because a
fill's price is the EXECUTION price and legitimately differs from the resting order's DISPLAY price
(on ITCH, the distinct "Order Executed With Price" message). Merging them would let a benign ~3%
rate hide a real wrong-side bug.

**Measured on XNAS NVDA** — 556,278 vendor assertions across two days at **100.000%** conformance
on existence, side and sufficient resting size:

| | 2025-07-01 | 2025-07-02 |
|---|---|---|
| `resting_fills_observed` | 307,584 | 248,694 |
| `fill_referenced_unknown_order` | 0 | 0 |
| `fill_side_mismatch` | 0 | 0 |
| `fill_size_exceeded_resting` | 0 | 0 |
| `fill_price_differs_from_resting` | 9,119 (2.965%) | 8,636 (3.47%) |
| `aggregate_trades_observed` | 0 | 0 |

`resting_fills_observed` is **bit-identical to the L-DECODE acceptance histogram's byte-70 count** —
an unplanned cross-check between two independently-derived instruments.

⚠️ **`aggregate_trades_observed` reads 0 on XNAS AND THAT IS THE PASSING VALUE — do not "fix" it.**
100% of the `TradeAggregate` population carries `order_id == 0` (375,643/375,643 on 2025-07-01 and
319,230/319,230 on 2025-07-02), so `is_system_message()` drops it upstream of the router. It becomes
reachable only when L-ADMIT lands. A gate that reads a scalar sum over these counters would go
**green** in that window without the fix; use a **per-carrier conjunction**, never a sum.

⚠️ **A PREDICTION THAT WAS WRONG, RECORDED BECAUSE THE FINDING IS INSIDE IT.** The plan expected the
18,061 `trade_order_not_found` events to *transfer* into `fill_referenced_unknown_order`. It reads
**0** — because **the 18,061 was itself an artifact of the double-decrement**: `F` and its paired
`C` both reduced exhausted orders early, so later `F` records missed. Fix the bug and nothing
misses. The claim *"deleting the Fill lookup would discard 18,061 events/day of signal"* was
therefore **wrong** — that was **symptom**, not signal. Keeping the lookup still stands, on the
stronger ground that it yields 556,278 vendor assertions per two days at 100.000% conformance.

**Tracked In**: `LobStats.{resting_fills_observed, fill_referenced_unknown_order, fill_side_mismatch,
fill_size_exceeded_resting, fill_price_differs_from_resting, aggregate_trades_observed}`.
The queue mirror keeps its own `QueueStats.fill_not_found`, whose lookup is likewise retained while
the depletion is removed. ⚠️ Its semantics **narrowed** at L-ROUTE: `TradeAggregate` misses are no
longer counted, so "exactly what it counted before" is true on XNAS only.

⚠️ **SCHEMA BUMP — MANDATORY, NOT COSMETIC.** All six new fields carry `#[serde(default)]`, so a
pre-L-ROUTE stats file (keys absent → 0) would be numerically **indistinguishable** from a
post-L-ROUTE file in which the carrier genuinely observed zero. `LOB_STATS_SCHEMA_VERSION` was
therefore bumped **2.0.0 → 2.1.0**. A consumer that does not check the version cannot tell the two
apart. (The value itself lives in `src/lob/reconstructor.rs`; see `CHANGELOG.md` — do not hand-copy
it into new documents.)

🔴 **KNOWN REGRESSION, RECORDED NOT HIDDEN — `has_warnings()` and `total_warnings()` ARE BLIND TO
ALL SIX.** Both aggregate only the pre-L-ROUTE set (three `cancel_*`, three now-dead `trade_*`,
`modify_order_not_found`, `add_order_id_collision`). Consequences, both real:
> * A non-zero `fill_side_mismatch` — the one alarm in this section — leaves `has_warnings()`
>   **false**. The health summary is silent on the newest defect signature in the crate.
> * Three of the eight terms they *do* sum are structurally dead, so `total_warnings()` is now a
>   sum over a partly-fossil set.
>
> This is a **net regression in health signalling** and is deliberately left for the counter commit
> (2b) rather than patched here. **Until it lands, read the six fields directly from the stats
> envelope; do not gate on `has_warnings()`.**

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
as the shipped gate excludes it. ~~There is **no ARCX MBP-10 anywhere on the data volume** — a
full-volume search for `*mbp*10*` returns 21 XNAS + 20 GLBX + **0 ARCX**~~ — **true when written,
FALSE since 2026-08-16; see the ARCX note below.** The two venues remain structurally different
(XNAS carries 0 filter-escaping `T` records on 12/12 sampled days; ARCX carries 25,901–97,956 per
day, 100.0000% `side='N'`).

⭐ **THE ORACLE HAS NOW BEEN RUN AGAINST AN ACTUAL RUST SUBJECT — the qualification above was
correct and is now DISCHARGED for the development days.** The 2026-08-11 arm was an independent
**Python** book port, so it graded the *semantics* and could not have detected a defect in any Rust
implementation of them. As of L-ROUTE (`c9c6f60`) the same oracle was run over three arms on
identical rows (`ts_row_proof_pct = 100.0`, `n_scored = 4,214,602`, all three):

| subject | day | exit | A-L1 | C-L1 | A-L10 | C-L10 |
|---|---|---|---|---|---|---|
| P — the shipped artifact | 2025-07-01 | 1 | 86.5490% | 84.3066% | 96.4154% | 96.0378% |
| R — a **HEAD Rust build** | 2025-07-01 | 1 | 86.5490% | 84.3066% | 96.4154% | 96.0378% |
| R — the **candidate Rust build** | 2025-07-01 | **0** | **100.0000%** | **100.0000%** | **100.0000%** | **100.0000%** |
| R — the candidate Rust build | 2025-07-02 | **0** | 100.0000% | 100.0000% | 100.0000% | 100.0000% |
| R — the candidate Rust build | 5 held-out days | **0** | all 20 A/C × L1–L10 cells conforming | | |

**The HEAD-build arm is the load-bearing one** and was not requested: it scores *bit-identically* to
a shipped artifact produced five months earlier by pre-L-DECODE code. That proves the decode split
(L-DECODE) is **book-neutral** and that **L-ROUTE is the sole cause** of 86.549% → 100.000%. Because
all three arms are paired on identical rows, this is an **attribution**, not a before/after.
⇒ Do not repeat *"the oracle only qualifies the Python semantics"* for these days without the date;
it was accurate through 2026-08-15 and is now bounded to the days the Rust subject has not covered.
Equally, do **not** generalise the Rust result beyond the two development days + five held-out days
measured.

⭐ **ARCX MBP-10 NOW EXISTS ON THE VOLUME — for exactly two days.** Acquired 2026-08-16 under
operator ruling R7 for the two development days only (`data/ARCX_MBP10_2025-07/`:
`arcx-pillar-20250701.mbp-10.dbn.zst` 3,797,338 records, `arcx-pillar-20250702.mbp-10.dbn.zst`
2,799,305 records; sha256-verified after an SSD disconnect/reconnect). Against it the candidate
build reaches **100.0000%** at A-L1 and C-L1 on both days, where HEAD measures 91.8865% / 91.0339%
(A-L1) and 91.0107% / 90.0705% (C-L1) with all 20 cells nonconforming.
⛔ **THE ARCX DATA IS INERT TO THE CHECKED-IN ORACLES UNTIL A `--venue` OPTION LANDS.** Both oracle
scripts hardcode XNAS on **eleven** sites, on the **MBO side as well as the MBP-10 side**. Fixing
only the MBP-10 side does **not** produce "file not found" — it produces a run that **succeeds and
returns a confident, wrong number**: an ARCX MBP-10 graded against an **XNAS** MBO book, emitted at
full precision with no error. The resolution is a **triple** — (MBO dir, MBP-10 dir, filename
prefix) — behind one option defaulting to XNAS so every existing citation keeps working. Tracked as
repo task #33; the scripts live at the monorepo root, not in this repo.

---

### Size Estimation Variance

**Observation** *(corrected 2026-08-01; ⚠️ its SOURCE is superseded — see the block above)*: OLD — "~91% exact match vs ~99% for prices".
NEW — the source artifact reports size exact-match **83.66% bid / 83.06% ask** against price
exact-match **95.56% bid / 95.73% ask**. The old pair (91%/99%) is not in the artifact.

**Root Cause** *(corrected)*: OLD — "size aggregation timing differences between MBO reconstruction
and MBP-10 snapshots". NEW — that attribution is falsified. Re-running the comparison with `F`
treated as a book no-op reproduces the vendor book **exactly** on 100.000% of book-affecting records
(2025-07-01, 4,214,602 RTH comparisons: `A` 2,071,194 and `C` 1,893,575 rows both go to 100.000%
price and 100.000% size, from 99.700%/86.549% and 99.303%/84.307%). The shortfall is the `F`-merge
defect, not aggregation timing.

> 🔴 **STRUCK 2026-08-16 — THE "ZERO `F`" PREMISE IS REFUTED. DO NOT CITE IT, HERE OR ANYWHERE.**
> This paragraph used to open with ~~"Databento's own MBP-10 contains **zero `F` records**"~~ —
> **FALSE.** Measured across the 21 vendor MBP-10 day files: **38 `F` records on 11 of the 21 days**,
> every one at the opening or closing cross. The claim was generalised from one of the 10 genuinely
> zero days. A gate or a reviewer asserting "the vendor MBP-10 contains no `F`" will read a
> **correct** vendor file as anomalous on 11 days in 21.
> ⭐ **The conclusion survives on a strictly stronger argument — cite the PAIRING, never the zero.**
> `F` is a book no-op because **the venue removes the resting order itself, with a paired `C`**, not
> because `F` is absent from the vendor's book view: `F`→`C` pairing **1.00000000 over 6 days /
> 1,808,570 records**, paired `C` the literal next record, `side` and `size` matching, holding
> **inside both auction crosses** (so **no auction carve-out is required**). The 100.000%
> reproduction quoted above is unaffected by the struck premise and stands. Struck in production
> source at `src/dbn_bridge.rs` by the same correction — see §7 for the full pairing evidence.

**Impact** *(corrected; ⚠️ now HISTORICAL — the defect is fixed on this branch)*: NOT minor at
sequence resolution. The distortion was transient — it lived between each `F` and its paired `C` —
but feature sampling lands on those positions. Shipped-corpus distortion on 2025-02-03: best-bid
wrong 0.741%, best-ask 1.016%, **mid wrong 1.753%**, L1 sizes 7.2%/10.4%, spread systematically
**+1.093% too wide**. End-of-day state is identical either way, which is why day-level checks looked
clean. **L-ROUTE removes the mutation**; the candidate build reproduces the vendor book at
100.0000% on every A/C × L1–L10 cell measured (see the oracle table above). ⚠️ **Every export
currently on disk predates that fix and still carries this distortion** — the fix is on a candidate
branch and **neither repo's `main` carries it**.

---

## Exporting Warnings

### Using LobStats

> 🔴 **`has_warnings()` / `total_warnings()` ARE NOT A COMPLETE HEALTH CHECK post-L-ROUTE.** They
> aggregate only the pre-L-ROUTE set — three `cancel_*`, three now-dead `trade_*`,
> `modify_order_not_found`, `add_order_id_collision` — and are **blind to all six fill-oracle
> counters** (§7). A non-zero `fill_side_mismatch`, the crate's newest defect signature, leaves
> `has_warnings()` **false**. Read the six fields directly until the counter commit lands.

```rust
use mbo_lob_reconstructor::LobReconstructor;

let mut lob = LobReconstructor::new(10);
// ... process messages ...

// Check for warnings — NECESSARY, NOT SUFFICIENT (see the warning above)
if lob.stats().has_warnings() {
    println!("Total warnings: {}", lob.stats().total_warnings());
}

// The fill oracle is NOT covered by has_warnings(); check it explicitly.
let s = lob.stats();
assert_eq!(s.fill_side_mismatch, 0, "resting-side disagreement — a defect signature");
assert_eq!(s.fill_referenced_unknown_order, 0);
assert_eq!(s.fill_size_exceeded_resting, 0);
// fill_price_differs_from_resting is EXPECTED non-zero (~3%/day) — do not assert 0.

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

> ⭐ **THE RE-RUN NOW EXISTS, AND IT IS A DIFFERENT INSTRUMENT.** The corrected replay is the
> ten-level MBP-10 oracle, and its gate is set from the corrected value, not the buggy one: the
> candidate Rust build exits **0** at **100.0000%** on every A/C × L1–L10 cell across two
> development days plus five held-out days, while a HEAD Rust build exits **1** and reproduces the
> shipped artifact bit-identically. See the oracle table above. That measurement supersedes this
> artifact for every purpose; this section is retained as the record of how a gate can be
> positioned below its own bug.

---

## Debugging Checklist

1. **Check warning counts**: `lob.stats().total_warnings()` — ⚠️ **incomplete**; blind to the six
   fill-oracle counters (§7)
2. **Check the fill oracle explicitly**: `fill_side_mismatch` / `fill_referenced_unknown_order` /
   `fill_size_exceeded_resting` must be **0**; `fill_price_differs_from_resting` is expected
   non-zero (~3%/day)
3. **Check the acceptance channel**: `cancel_order_not_found` must be **0** post-L-ROUTE — a
   non-zero value is an open defect, not feed behaviour. ⚠️ Do **not** check `trade_order_not_found`:
   it has zero increment sites and reads 0 unconditionally (§1)
4. **Check for book clears**: `lob.stats().book_clears`
5. **Check crossed quotes**: `lob.stats().crossed_quotes`
6. **Export detailed stats**: `lob.stats().export_to_file("debug.json")` — confirm the envelope's
   `schema_version` against `LOB_STATS_SCHEMA_VERSION` in `src/lob/reconstructor.rs` — a file
   written before the L-ROUTE bump has the six new keys *absent*, which `#[serde(default)]` renders
   as 0 and is **indistinguishable** from a genuine zero observation
7. **Enable logging**: Use `LobConfig::new(10).with_logging(true)`
8. **Check timestamps**: Ensure data is in chronological order

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

## ⚠️ BRANCH NOTE — this copy is one merge behind `main` on the BBO/oracle/D1 block

`main` carries an additional trailing section, **"BBO accuracy and the MBP-10 oracle — what may and
may not be cited"**, added there (and only there) when that block was relocated out of the root
`CLAUDE.md` always-on layer. It is **not** on this candidate branch, which branched earlier. Nothing
above is written to replace it: it arrives intact on the next merge, and this file deliberately adds
no copy of it, so the merge stays clean.

Two things in that block are **dated statements this branch's measurements now bound** — read them
together, not in isolation:

* *"there is no ARCX MBP-10 on the data volume at all"* — true when written, **false since
  2026-08-16** for exactly two days. See the ARCX note in the oracle section above.
* *"a Python port of the candidate semantics matches the vendor MBP-10"* / *"never write 'the oracle
  validates the reconstructor'"* — the **correct** rule through 2026-08-15, and the reason it was
  written still holds for every day the Rust subject has not covered. As of L-ROUTE the same oracle
  **has** been run against actual Rust subjects (a HEAD build and the candidate build, paired on
  identical rows) for the two development days plus five held-out days. State the scope; do not
  drop either half.

Its other qualifications — the provenance-free status of `data/validation_results_july2025.json`,
the `T`-stratum exclusion, the NVDA/XNAS/July-2025 bound, and the fact that `D1` has no method or
acceptance criterion in the packet — are **unaffected by anything on this branch and stand as
written**.
