# Strict XNAS Replay Contract

Status: first stable candidate for the `mbo-lob-reconstructor` 1.0.0 public
boundary. It is not a publication, research-admission, or vendor-conformance
authority. Earlier probe-v2 receipts are immutable development evidence and
must not be reinterpreted as this contract.

## Owned responsibilities

The strict XNAS path owns:

- exact opened-file and independently supplied logical-source binding;
- lossless DBN MBO projection and typed accepted/rejected custody;
- XNAS historical publisher policy;
- identity-local envelope assembly and causal availability;
- exact order-level book mutation and terminal reconciliation;
- validity, quarantine, recovery, and EOF-tail ledgers;
- committed-observation and terminal-receipt identities;
- a two-pass software equivalence boundary for downstream private staging.

It does not own exchange-session eligibility, sampling policy, features,
normalization, labels, artifact publication, catalog authority, or research
admission. Those policies belong to downstream owners and must remain bound to
the receipt rather than being inferred from it.

## Event and book semantics

`EventDispositionV1` preserves the interpreted event lane in addition to the
raw provider payload:

| Raw action | Public lane | Exact resting-book mutation |
|---|---|---|
| `A`, `M`, `C`, `R` | `Book` | transactional command |
| `T` | `Execution` trade carrier | none |
| `F` | `Execution` resting-fill carrier | none |
| `N` | `Control` | none |

An envelope's `events()` slice is its once-consumed member population. Its
`witness()` is closure evidence and is not another member of that envelope; it
may become a member of the next envelope. `effective_available_ns` is the first
global receive-time watermark that proves closure and is the causal downstream
clock. Neither `ts_event` nor the envelope endpoint alone proves availability.

`XnasBookCommitV1::book_commands_committed` measures gross committed commands.
`exact_endpoint_state_changed` instead measures whether the exact resting-order
endpoint or reset epoch differs after the whole transaction. Therefore an
add-then-cancel envelope can have two commands and `false`; a deeper-than-
exported-depth modification can have an unchanged visible snapshot and `true`;
and a qualified empty-book recovery reset is `true` because its reset epoch
advances.

Validity epochs and book reset epochs are intentionally different counters. A
source that starts invalid can first qualify in validity epoch 1 after the book
advances to reset epoch 2. Consumers must persist both and must not assert that
they are equal.

## Fail-loud lifecycle

`StrictXnasReplayV1` exposes pending updates internally but mints
`XnasReplayReceiptV1` only after physical EOF, post-read source verification,
population reconciliation, identity-ledger closure, and whole-book
reconciliation. A prefix failure cannot produce a receipt. A replay that
reaches EOF but fails a terminal invariant produces the explicitly
non-consumable `XnasTerminalDisqualificationV1`.

Failures have exactly three owners:

- identity-attributable market-data anomalies quarantine that identity, emit no
  contaminated observation, and permit requalification only through a clean,
  witnessed `R` recovery envelope;
- source, custody, resource-bound, arithmetic, and internal reconciliation
  failures are replay-fatal and cannot be converted into selective record
  loss;
- an identity that never qualified is a terminal disqualification; for an
  identity that was previously valid, an EOF tail or unrecovered-invalid
  interval is explicitly quarantined and recorded in the replay receipt while
  earlier closed validity epochs remain attestable. That replay receipt proves
  deterministic custody, not complete-day scientific admissibility. A
  downstream day/generation qualifier must inspect terminal identity status,
  validity epochs, and quarantine populations before publication.

Envelope and book error classification is compiler-exhaustive: a new error
variant cannot compile until its owner is chosen. Snapshot-driven recovery is
not implemented in this version. Snapshot-like input is quarantined and cannot
restore validity; only the documented clean `R` path can do so. This is an
intentional fail-closed boundary, not an implicit promise of snapshot support.

Downstream staging uses this closed lifecycle:

1. `StrictXnasReplayV1::qualify()` completes pass one and returns
   `XnasQualifiedReplayPlanV1`.
2. `open_revalidation_pass()` opens the same expectation-bound path with the
   same policy and configuration.
3. `next_observation()` yields non-serializable
   `XnasPendingEnvelopeObservationV1` values for private staging only.
4. The consumer feeds each observation, in order, to
   `XnasCommittedObservationAccumulatorV1`. Its closure is an in-memory token,
   not a serializable receipt or publication capability.
5. The pass must be drained to physical EOF. A pass-two failure is fused and
   cannot be resumed or finished.
6. Replay `finish()` requires field-for-field equality of the two complete terminal
   receipts and returns `XnasReplayEquivalenceReceiptV1`.
7. Accumulator `finish()` accepts only that post-EOF
   `XnasReplayEquivalenceReceiptV1`; the pass-one qualification receipt is not a
   valid closure capability. It checks the full qualification receipt,
   consumed-observation denominator, and terminal rolling chain before
   returning its non-serializable closure token.

Each strict pass pre-hashes, decodes, and post-hashes the opened object. The
two-pass path therefore traverses the source six times. This cost is deliberate
and must be measured by the final executable owner.

An equivalence receipt cannot prove that arbitrary downstream rows consumed
every pending observation. The official extractor must own the actual
revalidation pass inside an unqualified pending generation, process every
observation exactly once, reconcile count and rolling observation chain, and
promote only by consuming that same pending generation and finishing its owned
pass. A free-standing receipt must never qualify separately staged output.

## Versioned identities

The stable candidate uses these explicit identities:

| Surface | Identity |
|---|---|
| success receipt | `xnas_replay_receipt_v1` |
| terminal disqualification | `xnas_terminal_disqualification_v1` |
| two-pass attestation | `xnas_replay_equivalence_receipt_v1` |
| probe envelope | `xnas_strict_replay_probe_v3` |
| replay algorithm | `hft.xnas.strict_replay.v2` |
| ready-envelope digest | `hft.xnas_ready_envelope.v2` |
| book transition chain | `hft.xnas_book_transition_chain.{seed.,}v2` |
| canonical exact book state | `hft.xnas_canonical_book_state.v1` |
| committed observation | `hft.xnas_committed_observation.v2` |
| committed-observation chain | `hft.xnas_committed_observation_chain.{seed.,}v2` |

The observation digest binds source, identity, validity epoch, symbol, member
and witness raw payloads and interpreted semantic tags, sequence and closure
semantics, causal clocks, recovery state, exact book commit/reset/transition,
and the exported snapshot. The rolling chain binds the ordered global
observation population. The terminal receipt binds that chain to source,
configuration, build, counters, identity ledgers, schema, and authority.

Golden vectors and an independently implemented chain calculation protect the
encodings. Any encoding change requires a new domain version and serialized
boundary; never silently update a golden value without explaining the intended
contract change.

## Build identity and authority limits

The receipt records package version, verified package-repository commit and
dirty state, package-repository `Cargo.lock` digest, exact compiler command and
`rustc -vV` identity, target, profile, enabled features, and replay algorithm.
Git discovery fails closed if Git resolves an ancestor consumer repository
instead of this package root.

This package-local identity deliberately does not prove the final executable,
consumer workspace dependency closure, effective invocation, catalog content,
or artifact bytes. The executable/publication owner must additionally bind its
workspace lock, binary digest, effective configuration and command, catalog
receipt, artifact-data digest, and atomic generation transaction. Dirty or
incomplete identities cannot publish.

## Probe and verification

The development probe accepts an independently minted expectation file:

```text
xnas_replay_probe <expectation-v1.json> [selected-raw-ordinals-csv]
```

It writes no stdout before a terminal outcome. Exit 0 is a qualified EOF
receipt, exit 2 is a source-complete but explicitly non-consumable terminal
disqualification, and exit 1 is a source/resource/software failure with no
success-typed stdout.

Primary regression gates:

```text
cargo test --locked --features databento --lib
cargo test --locked --features databento --test xnas_replay
cargo test --locked --workspace --all-features --all-targets
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --locked --workspace --all-features --no-deps
```

The repository-wide test, Clippy, and rustdoc targets are warning-free. The
unqualified pathname iterator and its legacy test targets no longer exist;
file-backed replay begins at the source-bound strict loader.

The tests include fixed hash vectors, independent chain recomputation,
quarantine/recovery two-pass equivalence, pass-two fused failure, source
mutation, and a late same-length mutation restored before post-hash that can
only be rejected by terminal receipt mismatch. These are software gates, not a
substitute for an independent vendor book oracle or immutable source custody.
