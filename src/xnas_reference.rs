//! Independently authored reference semantics for DECISION-031 conformance.
//!
//! This lane deliberately shares only the lossless raw record DTOs, fixed DBN
//! constants, and identity/ordinal value types with the primary implementation.
//! It does not call `XnasMboStreamV1`, `XnasMbp10StreamV1`,
//! `LobReconstructor`, or any primary book/envelope helper.  Its book is an
//! order map and its open MBO candidate is represented as explicit sequence
//! blocks, providing a separately implemented path to the canonical
//! conformance projection.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::xnas_semantics::{
    Mbp10LevelV1, RawMboRecordV1, RawMbp10RecordV1, SourceOrdinal, XnasIdentityV1,
    DBN_FLAG_BAD_TS_RECV, DBN_FLAG_LAST, DBN_FLAG_MAYBE_BAD_BOOK, DBN_FLAG_SNAPSHOT, DBN_RTYPE_MBO,
    DBN_RTYPE_MBP_10, DBN_UNDEF_PRICE, DBN_UNDEF_TIMESTAMP, XNAS_ITCH_PUBLISHER_ID,
};

/// Independent error vocabulary. Stable codes, rather than shared primary
/// enum variants, are compared by the conformance harness.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
pub(crate) enum ReferenceSemanticErrorV1 {
    #[error("source ordinal mismatch: expected {expected}, observed {observed}")]
    SourceOrdinalMismatch { expected: u64, observed: u64 },
    #[error("unexpected identity")]
    UnexpectedIdentity,
    #[error("wrong record type")]
    WrongRecordType,
    #[error("initial clear signature mismatch")]
    InitialClearSignatureMismatch,
    #[error("later initial clear")]
    LaterInitialClear,
    #[error("undefined event timestamp")]
    UndefinedTsEvent,
    #[error("undefined receive timestamp")]
    UndefinedTsRecv,
    #[error("BAD_TS_RECV")]
    BadTsRecv,
    #[error("MAYBE_BAD_BOOK")]
    MaybeBadBook,
    #[error("identity-local receive timestamp regression")]
    NonMonotoneTsRecv,
    #[error("exact duplicate")]
    ExactDuplicate,
    #[error("block timestamp mismatch")]
    BlockTimestampMismatch,
    #[error("LAST to non-LAST transition")]
    LastToNonLast,
    #[error("receive time changed before terminality")]
    ReceiveTimeChangedBeforeTerminal,
    #[error("channel changed")]
    ChannelChange,
    #[error("reset boundary")]
    ResetBoundary,
    #[error("snapshot boundary")]
    SnapshotBoundary,
    #[error("source gap")]
    SourceGap,
    #[error("decode gap")]
    DecodeGap,
    #[error("session boundary")]
    SessionBoundary,
    #[error("invalid state")]
    InvalidState,
    #[error("initialization incomplete at EOF")]
    InitializationIncompleteAtEof,
    #[error("missing expected identity")]
    MissingExpectedIdentity,
    #[error("terminal MBP block has no book-bearing record")]
    NoBookBearingTerminalRecord,
    #[error("sequence regression or reuse")]
    SequenceRegressionOrReuse,
    #[error("unsupported action {0}")]
    UnsupportedAction(u8),
    #[error("unsupported side {0}")]
    UnsupportedSide(u8),
    #[error("undefined execution price")]
    UndefinedExecutionPrice,
    #[error("terminal candidate at EOF")]
    TerminalAtEof,
    #[error("open candidate at EOF")]
    OpenAtEof,
    #[error("book mutation failed: {0}")]
    BookMutation(String),
    #[error("book mutation anomaly")]
    BookMutationAnomaly,
    #[error("locked or crossed endpoint")]
    InvalidEndpointBook,
    #[error("timestamp exceeds signed private-book range")]
    TimestampOutOfRange,
}

impl ReferenceSemanticErrorV1 {
    pub(crate) const fn code(&self) -> &'static str {
        match self {
            Self::SourceOrdinalMismatch { .. } => "SOURCE_ORDINAL_MISMATCH",
            Self::UnexpectedIdentity => "UNEXPECTED_IDENTITY",
            Self::WrongRecordType => "WRONG_RECORD_TYPE",
            Self::InitialClearSignatureMismatch => "INITIAL_CLEAR_SIGNATURE_MISMATCH",
            Self::LaterInitialClear => "LATER_INITIAL_CLEAR",
            Self::UndefinedTsEvent => "UNDEFINED_TS_EVENT",
            Self::UndefinedTsRecv => "UNDEFINED_TS_RECV",
            Self::BadTsRecv => "BAD_TS_RECV",
            Self::MaybeBadBook => "MAYBE_BAD_BOOK",
            Self::NonMonotoneTsRecv => "NONMONOTONE_TS_RECV",
            Self::ExactDuplicate => "EXACT_DUPLICATE",
            Self::BlockTimestampMismatch => "BLOCK_TIMESTAMP_MISMATCH",
            Self::LastToNonLast => "LAST_TO_NON_LAST",
            Self::ReceiveTimeChangedBeforeTerminal => "RECEIVE_TIME_CHANGED_BEFORE_TERMINAL",
            Self::ChannelChange => "CHANNEL_CHANGE",
            Self::ResetBoundary => "RESET_BOUNDARY",
            Self::SnapshotBoundary => "SNAPSHOT_BOUNDARY",
            Self::SourceGap => "SOURCE_GAP",
            Self::DecodeGap => "DECODE_GAP",
            Self::SessionBoundary => "SESSION_BOUNDARY",
            Self::InvalidState => "INVALID_STATE",
            Self::InitializationIncompleteAtEof => "INITIALIZATION_INCOMPLETE_AT_EOF",
            Self::MissingExpectedIdentity => "MISSING_EXPECTED_IDENTITY",
            Self::NoBookBearingTerminalRecord => "NO_BOOK_BEARING_TERMINAL_RECORD",
            Self::SequenceRegressionOrReuse => "SEQUENCE_REGRESSION_OR_REUSE",
            Self::UnsupportedAction(_) => "UNSUPPORTED_ACTION",
            Self::UnsupportedSide(_) => "UNSUPPORTED_SIDE",
            Self::UndefinedExecutionPrice => "UNDEFINED_EXECUTION_PRICE",
            Self::TerminalAtEof => "TERMINAL_AT_EOF",
            Self::OpenAtEof => "OPEN_AT_EOF",
            Self::BookMutation(_) => "BOOK_MUTATION",
            Self::BookMutationAnomaly => "BOOK_MUTATION_ANOMALY",
            Self::InvalidEndpointBook => "INVALID_ENDPOINT_BOOK",
            Self::TimestampOutOfRange => "TIMESTAMP_OUT_OF_RANGE",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReferenceQuarantinePopulationV1 {
    pub(crate) incident_count: u64,
    pub(crate) open_candidate_count: u64,
    pub(crate) record_count: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReferenceMboCountsV1 {
    pub(crate) raw_record_count: u64,
    pub(crate) initial_xnas_clear_control_count: u64,
    pub(crate) completed_member_record_count: u64,
    pub(crate) pending_record_count: u64,
    pub(crate) quarantined_record_count: u64,
    pub(crate) private_book_reset_count: u64,
    pub(crate) completed_update_envelope_count: u64,
    pub(crate) venue_sequence_block_count: u64,
    pub(crate) execution_sequence_block_count: u64,
    pub(crate) execution_envelope_count: u64,
    pub(crate) execution_carrier_count: u64,
    pub(crate) published_book_state_count: u64,
    pub(crate) first_valid_publication_ordinal: Option<SourceOrdinal>,
    pub(crate) first_valid_publication_time_ns: Option<u64>,
    pub(crate) quarantined_by_reason: BTreeMap<String, ReferenceQuarantinePopulationV1>,
}

impl ReferenceMboCountsV1 {
    pub(crate) fn population_reconciles(&self) -> bool {
        self.raw_record_count
            == self.initial_xnas_clear_control_count
                + self.completed_member_record_count
                + self.pending_record_count
                + self.quarantined_record_count
            && self.quarantined_record_count
                == self
                    .quarantined_by_reason
                    .values()
                    .map(|value| value.record_count)
                    .sum::<u64>()
    }

    fn admit(&mut self) {
        self.raw_record_count += 1;
        self.pending_record_count += 1;
    }

    fn consume_initial_control(&mut self) {
        self.pending_record_count -= 1;
        self.initial_xnas_clear_control_count += 1;
    }

    fn complete(&mut self, record_count: u64) {
        self.pending_record_count -= record_count;
        self.completed_member_record_count += record_count;
    }

    fn quarantine(
        &mut self,
        error: &ReferenceSemanticErrorV1,
        open_candidate_count: u64,
        record_count: u64,
    ) {
        self.pending_record_count -= record_count;
        self.quarantined_record_count += record_count;
        let population = self
            .quarantined_by_reason
            .entry(error.code().to_owned())
            .or_default();
        population.incident_count += 1;
        population.open_candidate_count += open_candidate_count;
        population.record_count += record_count;
    }
}

/// Canonical independent MBO envelope used for exact primary/reference parity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReferenceMboEnvelopeV1 {
    pub(crate) identity: XnasIdentityV1,
    pub(crate) channel_id: u8,
    pub(crate) ordered_distinct_sequence_vector: Vec<u32>,
    pub(crate) terminal_sequence: u32,
    pub(crate) records: Vec<RawMboRecordV1>,
    pub(crate) terminal_source_ordinal: SourceOrdinal,
    pub(crate) witness_source_ordinal: SourceOrdinal,
    pub(crate) endpoint_ns: u64,
    pub(crate) witness_ts_recv: u64,
    pub(crate) effective_available_ns: u64,
    pub(crate) closure_confirmation_delay_ns: u64,
    pub(crate) venue_sequence_block_count: u64,
    pub(crate) execution_sequence_block_count: u64,
    pub(crate) execution_carrier_count: u64,
    pub(crate) execution_envelope: bool,
    pub(crate) last_execution_price: Option<i64>,
    pub(crate) execution_price_change_proxy_v1: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReferenceMboPublicationV1 {
    pub(crate) envelope: ReferenceMboEnvelopeV1,
    pub(crate) levels: [Mbp10LevelV1; 10],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReferenceMboFinishReportV1 {
    pub(crate) counts: ReferenceMboCountsV1,
    pub(crate) terminal_error: Option<ReferenceSemanticErrorV1>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReferenceOrderV1 {
    side: u8,
    price: i64,
    size: u32,
}

/// Independent order-map book. Aggregate levels are materialized only at a
/// completed endpoint, unlike the primary price-level reconstructor.
#[derive(Debug, Clone, Default)]
struct ReferenceOrderBookV1 {
    orders: BTreeMap<u64, ReferenceOrderV1>,
}

impl ReferenceOrderBookV1 {
    fn apply_transaction(
        &self,
        records: &[RawMboRecordV1],
    ) -> Result<(Self, [Mbp10LevelV1; 10]), ReferenceSemanticErrorV1> {
        let mut next = self.clone();
        for record in records {
            if record.ts_event > i64::MAX as u64 {
                return Err(ReferenceSemanticErrorV1::TimestampOutOfRange);
            }
            match record.action {
                b'T' | b'F' => {}
                b'R' => next.orders.clear(),
                b'A' => {
                    validate_book_order_fields(record)?;
                    if next.orders.contains_key(&record.order_id) {
                        return Err(ReferenceSemanticErrorV1::BookMutationAnomaly);
                    }
                    next.orders.insert(
                        record.order_id,
                        ReferenceOrderV1 {
                            side: record.side,
                            price: record.price,
                            size: record.size,
                        },
                    );
                }
                b'M' => {
                    validate_book_order_fields(record)?;
                    if !next.orders.contains_key(&record.order_id) {
                        return Err(ReferenceSemanticErrorV1::BookMutationAnomaly);
                    }
                    next.orders.insert(
                        record.order_id,
                        ReferenceOrderV1 {
                            side: record.side,
                            price: record.price,
                            size: record.size,
                        },
                    );
                }
                b'C' => {
                    validate_book_order_fields(record)?;
                    let Some(order) = next.orders.get_mut(&record.order_id) else {
                        return Err(ReferenceSemanticErrorV1::BookMutationAnomaly);
                    };
                    if record.size >= order.size {
                        next.orders.remove(&record.order_id);
                    } else {
                        order.size -= record.size;
                    }
                }
                action => return Err(ReferenceSemanticErrorV1::UnsupportedAction(action)),
            }
        }
        let levels = next.materialize_levels()?;
        Ok((next, levels))
    }

    fn materialize_levels(&self) -> Result<[Mbp10LevelV1; 10], ReferenceSemanticErrorV1> {
        let mut bids = BTreeMap::<i64, (u64, u64)>::new();
        let mut asks = BTreeMap::<i64, (u64, u64)>::new();
        for order in self.orders.values() {
            let side = match order.side {
                b'B' => &mut bids,
                b'A' => &mut asks,
                side => return Err(ReferenceSemanticErrorV1::UnsupportedSide(side)),
            };
            let aggregate = side.entry(order.price).or_default();
            aggregate.0 = aggregate
                .0
                .checked_add(u64::from(order.size))
                .ok_or_else(|| {
                    ReferenceSemanticErrorV1::BookMutation("level size overflow".to_owned())
                })?;
            aggregate.1 += 1;
        }

        if let (Some(best_bid), Some(best_ask)) = (bids.last_key_value(), asks.first_key_value()) {
            if best_bid.0 >= best_ask.0 {
                return Err(ReferenceSemanticErrorV1::InvalidEndpointBook);
            }
        }

        let mut levels = [Mbp10LevelV1::default(); 10];
        for (index, (&price, &(size, count))) in bids.iter().rev().take(10).enumerate() {
            levels[index].bid_px = price;
            levels[index].bid_sz = u32::try_from(size).map_err(|_| {
                ReferenceSemanticErrorV1::BookMutation("bid size exceeds u32".to_owned())
            })?;
            levels[index].bid_ct = u32::try_from(count).map_err(|_| {
                ReferenceSemanticErrorV1::BookMutation("bid count exceeds u32".to_owned())
            })?;
        }
        for (index, (&price, &(size, count))) in asks.iter().take(10).enumerate() {
            levels[index].ask_px = price;
            levels[index].ask_sz = u32::try_from(size).map_err(|_| {
                ReferenceSemanticErrorV1::BookMutation("ask size exceeds u32".to_owned())
            })?;
            levels[index].ask_ct = u32::try_from(count).map_err(|_| {
                ReferenceSemanticErrorV1::BookMutation("ask count exceeds u32".to_owned())
            })?;
        }
        Ok(levels)
    }
}

fn validate_book_order_fields(record: &RawMboRecordV1) -> Result<(), ReferenceSemanticErrorV1> {
    if record.order_id == 0 || record.price <= 0 || record.size == 0 {
        return Err(ReferenceSemanticErrorV1::BookMutation(
            "invalid order id, price, or size".to_owned(),
        ));
    }
    Ok(())
}

#[derive(Debug)]
struct ReferenceMboBlockV1 {
    sequence: u32,
    ts_event: u64,
    saw_last: bool,
    records: Vec<RawMboRecordV1>,
}

impl ReferenceMboBlockV1 {
    fn new(record: RawMboRecordV1) -> Self {
        Self {
            sequence: record.sequence,
            ts_event: record.ts_event,
            saw_last: record.flags & DBN_FLAG_LAST != 0,
            records: vec![record],
        }
    }

    fn has_execution(&self) -> bool {
        self.records
            .iter()
            .any(|record| matches!(record.action, b'T' | b'F'))
    }
}

/// Reference candidate is represented as explicit blocks rather than the
/// primary lane's flattened open envelope and current-block fields.
#[derive(Debug)]
struct ReferenceMboCandidateV1 {
    identity: XnasIdentityV1,
    channel_id: u8,
    common_ts_recv: u64,
    blocks: Vec<ReferenceMboBlockV1>,
}

impl ReferenceMboCandidateV1 {
    fn new(record: RawMboRecordV1) -> Self {
        Self {
            identity: record.identity(),
            channel_id: record.channel_id,
            common_ts_recv: record.ts_recv,
            blocks: vec![ReferenceMboBlockV1::new(record)],
        }
    }

    fn record_count(&self) -> u64 {
        self.blocks
            .iter()
            .map(|block| block.records.len() as u64)
            .sum()
    }

    fn current(&self) -> &ReferenceMboBlockV1 {
        self.blocks.last().expect("candidate has one block")
    }

    fn contains_payload(&self, record: &RawMboRecordV1) -> bool {
        self.blocks
            .iter()
            .flat_map(|block| block.records.iter())
            .any(|prior| same_mbo_payload(prior, record))
    }

    fn append_same_sequence(
        &mut self,
        record: RawMboRecordV1,
    ) -> Result<(), ReferenceSemanticErrorV1> {
        if record.channel_id != self.channel_id {
            return Err(ReferenceSemanticErrorV1::ChannelChange);
        }
        if record.ts_recv != self.common_ts_recv || record.ts_event != self.current().ts_event {
            return Err(ReferenceSemanticErrorV1::BlockTimestampMismatch);
        }
        if self.contains_payload(&record) {
            return Err(ReferenceSemanticErrorV1::ExactDuplicate);
        }
        if self.current().saw_last && record.flags & DBN_FLAG_LAST == 0 {
            return Err(ReferenceSemanticErrorV1::LastToNonLast);
        }
        let current = self.blocks.last_mut().expect("candidate has one block");
        current.saw_last |= record.flags & DBN_FLAG_LAST != 0;
        current.records.push(record);
        Ok(())
    }

    fn append_nonterminal_sequence(
        &mut self,
        record: RawMboRecordV1,
    ) -> Result<(), ReferenceSemanticErrorV1> {
        if record.channel_id != self.channel_id {
            return Err(ReferenceSemanticErrorV1::ChannelChange);
        }
        if record.sequence <= self.current().sequence {
            return Err(ReferenceSemanticErrorV1::SequenceRegressionOrReuse);
        }
        if record.ts_recv != self.common_ts_recv {
            return Err(ReferenceSemanticErrorV1::ReceiveTimeChangedBeforeTerminal);
        }
        self.blocks.push(ReferenceMboBlockV1::new(record));
        Ok(())
    }

    fn close(
        self,
        witness: &RawMboRecordV1,
        effective_available_ns: u64,
        previous_execution_price: Option<i64>,
    ) -> Result<ReferenceMboEnvelopeV1, ReferenceSemanticErrorV1> {
        if witness.channel_id != self.channel_id {
            return Err(ReferenceSemanticErrorV1::ChannelChange);
        }
        if witness.sequence <= self.current().sequence {
            return Err(ReferenceSemanticErrorV1::SequenceRegressionOrReuse);
        }
        let terminal_sequence = self.current().sequence;
        let ordered_distinct_sequence_vector = self
            .blocks
            .iter()
            .map(|block| block.sequence)
            .collect::<Vec<_>>();
        let execution_sequence_block_count = self
            .blocks
            .iter()
            .filter(|block| block.has_execution())
            .count() as u64;
        let records = self
            .blocks
            .into_iter()
            .flat_map(|block| block.records)
            .collect::<Vec<_>>();
        let terminal_source_ordinal = records
            .last()
            .expect("candidate contains one record")
            .source_ordinal;
        let execution_carrier_count = records
            .iter()
            .filter(|record| matches!(record.action, b'T' | b'F'))
            .count() as u64;
        let last_execution_price = records
            .iter()
            .rev()
            .find(|record| matches!(record.action, b'T' | b'F'))
            .map(|record| record.price);
        Ok(ReferenceMboEnvelopeV1 {
            identity: self.identity,
            channel_id: self.channel_id,
            venue_sequence_block_count: ordered_distinct_sequence_vector.len() as u64,
            ordered_distinct_sequence_vector,
            terminal_sequence,
            records,
            terminal_source_ordinal,
            witness_source_ordinal: witness.source_ordinal,
            endpoint_ns: self.common_ts_recv,
            witness_ts_recv: witness.ts_recv,
            effective_available_ns,
            closure_confirmation_delay_ns: effective_available_ns
                .checked_sub(self.common_ts_recv)
                .ok_or_else(|| {
                    ReferenceSemanticErrorV1::BookMutation(
                        "availability precedes endpoint".to_owned(),
                    )
                })?,
            execution_sequence_block_count,
            execution_carrier_count,
            execution_envelope: execution_carrier_count > 0,
            last_execution_price,
            execution_price_change_proxy_v1: last_execution_price.map(|price| {
                u8::from(previous_execution_price.is_some_and(|prior| prior != price))
            }),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReferenceMboInitializationV1 {
    Uninitialized,
    Cleared,
    Recovering,
    Valid,
    Invalid,
}

#[derive(Debug)]
struct ReferenceMboIdentityStateV1 {
    initialization: ReferenceMboInitializationV1,
    last_valid_ts_recv: Option<u64>,
    candidate: Option<ReferenceMboCandidateV1>,
    book: Option<ReferenceOrderBookV1>,
    previous_execution_price: Option<i64>,
}

impl Default for ReferenceMboIdentityStateV1 {
    fn default() -> Self {
        Self {
            initialization: ReferenceMboInitializationV1::Uninitialized,
            last_valid_ts_recv: None,
            candidate: None,
            book: None,
            previous_execution_price: None,
        }
    }
}

/// Streaming, source-order independent MBO reference reducer.
#[derive(Debug)]
pub(crate) struct ReferenceMboReducerV1 {
    expected_identities: BTreeSet<XnasIdentityV1>,
    observed_identities: BTreeSet<XnasIdentityV1>,
    states: BTreeMap<XnasIdentityV1, ReferenceMboIdentityStateV1>,
    next_ordinal: u64,
    global_watermark: Option<u64>,
    counts: ReferenceMboCountsV1,
    terminal_error: Option<ReferenceSemanticErrorV1>,
}

impl ReferenceMboReducerV1 {
    pub(crate) fn new(expected_identities: BTreeSet<XnasIdentityV1>) -> Self {
        Self {
            expected_identities,
            observed_identities: BTreeSet::new(),
            states: BTreeMap::new(),
            next_ordinal: 1,
            global_watermark: None,
            counts: ReferenceMboCountsV1::default(),
            terminal_error: None,
        }
    }

    pub(crate) fn counts(&self) -> &ReferenceMboCountsV1 {
        &self.counts
    }

    pub(crate) fn global_watermark(&self) -> Option<u64> {
        self.global_watermark
    }

    pub(crate) fn terminal_error(&self) -> Option<&ReferenceSemanticErrorV1> {
        self.terminal_error.as_ref()
    }

    pub(crate) fn push(
        &mut self,
        record: RawMboRecordV1,
    ) -> Result<Option<ReferenceMboPublicationV1>, ReferenceSemanticErrorV1> {
        if let Some(error) = &self.terminal_error {
            return Err(error.clone());
        }
        if record.source_ordinal.get() != self.next_ordinal {
            self.counts.admit();
            let error = ReferenceSemanticErrorV1::SourceOrdinalMismatch {
                expected: self.next_ordinal,
                observed: record.source_ordinal.get(),
            };
            return self.fail_terminal_admitted(error, 1);
        }
        self.next_ordinal += 1;
        self.counts.admit();

        let identity = record.identity();
        if !self.expected_identities.contains(&identity)
            || identity.publisher_id != XNAS_ITCH_PUBLISHER_ID
        {
            return self.fail_terminal_admitted(ReferenceSemanticErrorV1::UnexpectedIdentity, 1);
        }
        self.observed_identities.insert(identity);
        let initialization = self.states.entry(identity).or_default().initialization;

        if is_reference_initial_clear(&record) {
            if initialization == ReferenceMboInitializationV1::Invalid {
                return self
                    .fail_identity_with_current(identity, ReferenceSemanticErrorV1::InvalidState);
            }
            if initialization != ReferenceMboInitializationV1::Uninitialized {
                return self.fail_identity_with_current(
                    identity,
                    ReferenceSemanticErrorV1::LaterInitialClear,
                );
            }
            let state = self.states.get_mut(&identity).expect("state exists");
            state.book = Some(ReferenceOrderBookV1::default());
            state.candidate = None;
            state.previous_execution_price = None;
            state.initialization = ReferenceMboInitializationV1::Cleared;
            self.counts.consume_initial_control();
            self.counts.private_book_reset_count += 1;
            return Ok(None);
        }

        if initialization == ReferenceMboInitializationV1::Uninitialized {
            return self.fail_terminal_admitted(
                ReferenceSemanticErrorV1::InitialClearSignatureMismatch,
                1,
            );
        }
        if record.rtype != DBN_RTYPE_MBO {
            return self.fail_terminal_admitted(ReferenceSemanticErrorV1::WrongRecordType, 1);
        }
        if let Err(error) = validate_reference_receive_clock(record.ts_recv, record.flags) {
            return self.fail_identity_with_current(identity, error);
        }
        self.global_watermark = Some(
            self.global_watermark
                .map_or(record.ts_recv, |prior| prior.max(record.ts_recv)),
        );
        if self
            .states
            .get(&identity)
            .and_then(|state| state.last_valid_ts_recv)
            .is_some_and(|prior| record.ts_recv < prior)
        {
            return self
                .fail_identity_with_current(identity, ReferenceSemanticErrorV1::NonMonotoneTsRecv);
        }
        self.states
            .get_mut(&identity)
            .expect("state exists")
            .last_valid_ts_recv = Some(record.ts_recv);
        if let Err(error) = validate_reference_record(&record) {
            return self.fail_identity_with_current(identity, error);
        }

        if record.action == b'R' {
            let prior = self
                .states
                .get_mut(&identity)
                .expect("state exists")
                .candidate
                .take();
            if let Some(candidate) = prior {
                self.counts.quarantine(
                    &ReferenceSemanticErrorV1::ResetBoundary,
                    1,
                    candidate.record_count(),
                );
            }
            let state = self.states.get_mut(&identity).expect("state exists");
            state.book = Some(ReferenceOrderBookV1::default());
            state.previous_execution_price = None;
            state.initialization = ReferenceMboInitializationV1::Recovering;
            state.candidate = Some(ReferenceMboCandidateV1::new(record));
            self.counts.private_book_reset_count += 1;
            return Ok(None);
        }

        if self
            .states
            .get(&identity)
            .is_some_and(|state| state.initialization == ReferenceMboInitializationV1::Invalid)
        {
            return self
                .fail_identity_with_current(identity, ReferenceSemanticErrorV1::InvalidState);
        }

        let candidate = self
            .states
            .get_mut(&identity)
            .expect("state exists")
            .candidate
            .take();
        let Some(mut candidate) = candidate else {
            self.states
                .get_mut(&identity)
                .expect("state exists")
                .candidate = Some(ReferenceMboCandidateV1::new(record));
            return Ok(None);
        };
        let candidate_record_count = candidate.record_count();
        let current_sequence = candidate.current().sequence;
        if record.sequence == current_sequence {
            if let Err(error) = candidate.append_same_sequence(record) {
                return self.fail_identity_detached(identity, error, candidate_record_count + 1);
            }
            self.states
                .get_mut(&identity)
                .expect("state exists")
                .candidate = Some(candidate);
            return Ok(None);
        }
        if record.sequence < current_sequence {
            return self.fail_identity_detached(
                identity,
                ReferenceSemanticErrorV1::SequenceRegressionOrReuse,
                candidate_record_count + 1,
            );
        }
        if !candidate.current().saw_last {
            if let Err(error) = candidate.append_nonterminal_sequence(record) {
                return self.fail_identity_detached(identity, error, candidate_record_count + 1);
            }
            self.states
                .get_mut(&identity)
                .expect("state exists")
                .candidate = Some(candidate);
            return Ok(None);
        }

        let previous_execution_price = self
            .states
            .get(&identity)
            .expect("state exists")
            .previous_execution_price;
        let effective_available_ns = self
            .global_watermark
            .expect("validated witness establishes the watermark");
        let envelope =
            match candidate.close(&record, effective_available_ns, previous_execution_price) {
                Ok(value) => value,
                Err(error) => {
                    return self.fail_identity_detached(identity, error, candidate_record_count + 1)
                }
            };
        let book = self
            .states
            .get(&identity)
            .and_then(|state| state.book.as_ref())
            .cloned()
            .ok_or(ReferenceSemanticErrorV1::InvalidState);
        let (next_book, levels) =
            match book.and_then(|book| book.apply_transaction(&envelope.records)) {
                Ok(value) => value,
                Err(error) => {
                    return self.fail_identity_detached(
                        identity,
                        error,
                        envelope.records.len() as u64 + 1,
                    )
                }
            };

        let state = self.states.get_mut(&identity).expect("state exists");
        state.book = Some(next_book);
        state.initialization = ReferenceMboInitializationV1::Valid;
        state.candidate = Some(ReferenceMboCandidateV1::new(record));
        if let Some(price) = envelope.last_execution_price {
            state.previous_execution_price = Some(price);
        }
        self.counts.complete(envelope.records.len() as u64);
        self.counts.completed_update_envelope_count += 1;
        self.counts.venue_sequence_block_count += envelope.venue_sequence_block_count;
        self.counts.execution_sequence_block_count += envelope.execution_sequence_block_count;
        self.counts.execution_envelope_count += u64::from(envelope.execution_envelope);
        self.counts.execution_carrier_count += envelope.execution_carrier_count;
        self.counts.published_book_state_count += 1;
        if self.counts.first_valid_publication_ordinal.is_none() {
            self.counts.first_valid_publication_ordinal = Some(envelope.witness_source_ordinal);
            self.counts.first_valid_publication_time_ns = Some(envelope.effective_available_ns);
        }
        Ok(Some(ReferenceMboPublicationV1 { envelope, levels }))
    }

    pub(crate) fn invalidate_boundary(
        &mut self,
        error: ReferenceSemanticErrorV1,
    ) -> ReferenceSemanticErrorV1 {
        debug_assert!(matches!(
            error,
            ReferenceSemanticErrorV1::SourceGap
                | ReferenceSemanticErrorV1::DecodeGap
                | ReferenceSemanticErrorV1::SessionBoundary
        ));
        let mut candidates = 0_u64;
        let mut records = 0_u64;
        for identity in &self.expected_identities {
            let state = self.states.entry(*identity).or_default();
            if let Some(candidate) = state.candidate.take() {
                candidates += 1;
                records += candidate.record_count();
            }
            state.initialization = ReferenceMboInitializationV1::Invalid;
            state.book = None;
            state.previous_execution_price = None;
        }
        self.counts.quarantine(&error, candidates, records);
        error
    }

    pub(crate) fn finish(mut self) -> ReferenceMboFinishReportV1 {
        let mut terminal_error = self.terminal_error.take();
        if terminal_error.is_none() {
            if self
                .expected_identities
                .iter()
                .any(|identity| !self.observed_identities.contains(identity))
            {
                let error = ReferenceSemanticErrorV1::MissingExpectedIdentity;
                self.counts.quarantine(&error, 0, 0);
                terminal_error = Some(error);
            }
            for state in self.states.values_mut() {
                let initialization = state.initialization;
                if let Some(candidate) = state.candidate.take() {
                    let error = if matches!(
                        initialization,
                        ReferenceMboInitializationV1::Cleared
                            | ReferenceMboInitializationV1::Recovering
                    ) {
                        ReferenceSemanticErrorV1::InitializationIncompleteAtEof
                    } else if candidate.current().saw_last {
                        ReferenceSemanticErrorV1::TerminalAtEof
                    } else {
                        ReferenceSemanticErrorV1::OpenAtEof
                    };
                    self.counts.quarantine(&error, 1, candidate.record_count());
                    if matches!(
                        initialization,
                        ReferenceMboInitializationV1::Cleared
                            | ReferenceMboInitializationV1::Recovering
                    ) {
                        terminal_error.get_or_insert(error);
                    }
                } else if matches!(
                    initialization,
                    ReferenceMboInitializationV1::Cleared
                        | ReferenceMboInitializationV1::Recovering
                ) {
                    let error = ReferenceSemanticErrorV1::InitializationIncompleteAtEof;
                    self.counts.quarantine(&error, 0, 0);
                    terminal_error.get_or_insert(error);
                } else if initialization == ReferenceMboInitializationV1::Invalid {
                    terminal_error.get_or_insert(ReferenceSemanticErrorV1::InvalidState);
                }
            }
        }
        debug_assert!(self.counts.population_reconciles());
        debug_assert_eq!(self.counts.pending_record_count, 0);
        ReferenceMboFinishReportV1 {
            counts: self.counts,
            terminal_error,
        }
    }

    fn fail_identity_with_current<T>(
        &mut self,
        identity: XnasIdentityV1,
        error: ReferenceSemanticErrorV1,
    ) -> Result<T, ReferenceSemanticErrorV1> {
        let mut records = 1_u64;
        let mut candidates = 0_u64;
        if let Some(state) = self.states.get_mut(&identity) {
            if let Some(candidate) = state.candidate.take() {
                candidates = 1;
                records += candidate.record_count();
            }
            state.initialization = ReferenceMboInitializationV1::Invalid;
            state.book = None;
            state.previous_execution_price = None;
        }
        self.counts.quarantine(&error, candidates, records);
        Err(error)
    }

    fn fail_identity_detached<T>(
        &mut self,
        identity: XnasIdentityV1,
        error: ReferenceSemanticErrorV1,
        record_count: u64,
    ) -> Result<T, ReferenceSemanticErrorV1> {
        if let Some(state) = self.states.get_mut(&identity) {
            state.candidate = None;
            state.book = None;
            state.previous_execution_price = None;
            state.initialization = ReferenceMboInitializationV1::Invalid;
        }
        self.counts.quarantine(&error, 1, record_count);
        Err(error)
    }

    fn fail_terminal_admitted<T>(
        &mut self,
        error: ReferenceSemanticErrorV1,
        current_records: u64,
    ) -> Result<T, ReferenceSemanticErrorV1> {
        let (candidates, records) = self.drain_all_candidates();
        self.counts
            .quarantine(&error, candidates, records + current_records);
        self.terminal_error = Some(error.clone());
        Err(error)
    }

    fn drain_all_candidates(&mut self) -> (u64, u64) {
        let mut candidates = 0_u64;
        let mut records = 0_u64;
        for state in self.states.values_mut() {
            if let Some(candidate) = state.candidate.take() {
                candidates += 1;
                records += candidate.record_count();
            }
            state.initialization = ReferenceMboInitializationV1::Invalid;
            state.book = None;
            state.previous_execution_price = None;
        }
        (candidates, records)
    }
}

fn is_reference_initial_clear(record: &RawMboRecordV1) -> bool {
    record.rtype == DBN_RTYPE_MBO
        && record.publisher_id == XNAS_ITCH_PUBLISHER_ID
        && record.channel_id == 0
        && record.sequence == 0
        && record.action == b'R'
        && record.side == b'N'
        && record.order_id == 0
        && record.price == DBN_UNDEF_PRICE
        && record.size == 0
        && record.ts_in_delta == 0
        && record.flags == DBN_FLAG_BAD_TS_RECV
}

fn validate_reference_receive_clock(
    ts_recv: u64,
    flags: u8,
) -> Result<(), ReferenceSemanticErrorV1> {
    if ts_recv == DBN_UNDEF_TIMESTAMP {
        return Err(ReferenceSemanticErrorV1::UndefinedTsRecv);
    }
    if flags & DBN_FLAG_BAD_TS_RECV != 0 {
        return Err(ReferenceSemanticErrorV1::BadTsRecv);
    }
    Ok(())
}

fn validate_reference_record(record: &RawMboRecordV1) -> Result<(), ReferenceSemanticErrorV1> {
    if record.flags & DBN_FLAG_SNAPSHOT != 0 {
        return Err(ReferenceSemanticErrorV1::SnapshotBoundary);
    }
    if record.ts_event == DBN_UNDEF_TIMESTAMP {
        return Err(ReferenceSemanticErrorV1::UndefinedTsEvent);
    }
    if record.flags & DBN_FLAG_MAYBE_BAD_BOOK != 0 {
        return Err(ReferenceSemanticErrorV1::MaybeBadBook);
    }
    match record.action {
        b'A' | b'C' | b'M' if matches!(record.side, b'A' | b'B') => {}
        b'R' if record.side == b'N' => {}
        b'T' | b'F' if matches!(record.side, b'A' | b'B' | b'N') => {}
        b'A' | b'C' | b'M' | b'R' | b'T' | b'F' => {
            return Err(ReferenceSemanticErrorV1::UnsupportedSide(record.side))
        }
        action => return Err(ReferenceSemanticErrorV1::UnsupportedAction(action)),
    }
    if matches!(record.action, b'T' | b'F') && record.price == DBN_UNDEF_PRICE {
        return Err(ReferenceSemanticErrorV1::UndefinedExecutionPrice);
    }
    Ok(())
}

fn same_mbo_payload(left: &RawMboRecordV1, right: &RawMboRecordV1) -> bool {
    left.rtype == right.rtype
        && left.publisher_id == right.publisher_id
        && left.instrument_id == right.instrument_id
        && left.ts_event == right.ts_event
        && left.order_id == right.order_id
        && left.price == right.price
        && left.size == right.size
        && left.flags == right.flags
        && left.channel_id == right.channel_id
        && left.action == right.action
        && left.side == right.side
        && left.ts_recv == right.ts_recv
        && left.ts_in_delta == right.ts_in_delta
        && left.sequence == right.sequence
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReferenceMbp10CountsV1 {
    pub(crate) raw_record_count: u64,
    pub(crate) completed_member_record_count: u64,
    pub(crate) pending_record_count: u64,
    pub(crate) quarantined_record_count: u64,
    pub(crate) completed_endpoint_count: u64,
    pub(crate) sequence_block_count: u64,
    pub(crate) quarantined_by_reason: BTreeMap<String, ReferenceQuarantinePopulationV1>,
}

impl ReferenceMbp10CountsV1 {
    pub(crate) fn population_reconciles(&self) -> bool {
        self.raw_record_count
            == self.completed_member_record_count
                + self.pending_record_count
                + self.quarantined_record_count
            && self.quarantined_record_count
                == self
                    .quarantined_by_reason
                    .values()
                    .map(|population| population.record_count)
                    .sum::<u64>()
    }

    fn admit(&mut self) {
        self.raw_record_count += 1;
        self.pending_record_count += 1;
    }

    fn complete(&mut self, record_count: u64) {
        self.pending_record_count = self
            .pending_record_count
            .checked_sub(record_count)
            .expect("completed MBP-10 records are pending");
        self.completed_member_record_count += record_count;
    }

    fn quarantine(
        &mut self,
        error: &ReferenceSemanticErrorV1,
        open_candidate_count: u64,
        record_count: u64,
    ) {
        self.pending_record_count = self
            .pending_record_count
            .checked_sub(record_count)
            .expect("quarantined MBP-10 records are pending");
        self.quarantined_record_count += record_count;
        let population = self
            .quarantined_by_reason
            .entry(error.code().to_owned())
            .or_default();
        population.incident_count += 1;
        population.open_candidate_count += open_candidate_count;
        population.record_count += record_count;
    }
}

/// Independently reduced, channel-free MBP-10 corroborating endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReferenceMbp10EndpointV1 {
    pub(crate) identity: XnasIdentityV1,
    pub(crate) ordered_distinct_sequence_vector: Vec<u32>,
    pub(crate) terminal_sequence: u32,
    pub(crate) terminal_source_ordinal: SourceOrdinal,
    pub(crate) witness_source_ordinal: SourceOrdinal,
    pub(crate) endpoint_ns: u64,
    pub(crate) witness_ts_recv: u64,
    pub(crate) effective_available_ns: u64,
    pub(crate) closure_confirmation_delay_ns: u64,
    pub(crate) levels: [Mbp10LevelV1; 10],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReferenceMbp10FinishReportV1 {
    pub(crate) counts: ReferenceMbp10CountsV1,
    pub(crate) terminal_error: Option<ReferenceSemanticErrorV1>,
}

#[derive(Debug)]
struct ReferenceMbp10BlockV1 {
    sequence: u32,
    ts_event: u64,
    saw_last: bool,
    records: Vec<RawMbp10RecordV1>,
}

impl ReferenceMbp10BlockV1 {
    fn new(record: RawMbp10RecordV1) -> Self {
        Self {
            sequence: record.sequence,
            ts_event: record.ts_event,
            saw_last: record.flags & DBN_FLAG_LAST != 0,
            records: vec![record],
        }
    }
}

/// An MBP candidate is an explicit identity-local list of maximal sequence
/// blocks. No channel field exists in this representation.
#[derive(Debug)]
struct ReferenceMbp10CandidateV1 {
    identity: XnasIdentityV1,
    common_ts_recv: u64,
    blocks: Vec<ReferenceMbp10BlockV1>,
}

impl ReferenceMbp10CandidateV1 {
    fn new(record: RawMbp10RecordV1) -> Self {
        Self {
            identity: record.identity(),
            common_ts_recv: record.ts_recv,
            blocks: vec![ReferenceMbp10BlockV1::new(record)],
        }
    }

    fn record_count(&self) -> u64 {
        self.blocks
            .iter()
            .map(|block| block.records.len() as u64)
            .sum()
    }

    fn current(&self) -> &ReferenceMbp10BlockV1 {
        self.blocks.last().expect("candidate has one MBP block")
    }

    fn contains_payload(&self, record: &RawMbp10RecordV1) -> bool {
        self.blocks
            .iter()
            .flat_map(|block| block.records.iter())
            .any(|prior| same_mbp10_payload(prior, record))
    }

    fn append_same_sequence(
        &mut self,
        record: RawMbp10RecordV1,
    ) -> Result<(), ReferenceSemanticErrorV1> {
        if record.ts_recv != self.common_ts_recv || record.ts_event != self.current().ts_event {
            return Err(ReferenceSemanticErrorV1::BlockTimestampMismatch);
        }
        if self.contains_payload(&record) {
            return Err(ReferenceSemanticErrorV1::ExactDuplicate);
        }
        if self.current().saw_last && record.flags & DBN_FLAG_LAST == 0 {
            return Err(ReferenceSemanticErrorV1::LastToNonLast);
        }
        let current = self.blocks.last_mut().expect("candidate has one MBP block");
        current.saw_last |= record.flags & DBN_FLAG_LAST != 0;
        current.records.push(record);
        Ok(())
    }

    fn append_nonterminal_sequence(
        &mut self,
        record: RawMbp10RecordV1,
    ) -> Result<(), ReferenceSemanticErrorV1> {
        if record.sequence <= self.current().sequence {
            return Err(ReferenceSemanticErrorV1::SequenceRegressionOrReuse);
        }
        if record.ts_recv != self.common_ts_recv {
            return Err(ReferenceSemanticErrorV1::ReceiveTimeChangedBeforeTerminal);
        }
        self.blocks.push(ReferenceMbp10BlockV1::new(record));
        Ok(())
    }

    fn close(
        self,
        witness: &RawMbp10RecordV1,
        effective_available_ns: u64,
    ) -> Result<ReferenceMbp10EndpointV1, ReferenceSemanticErrorV1> {
        if witness.sequence <= self.current().sequence {
            return Err(ReferenceSemanticErrorV1::SequenceRegressionOrReuse);
        }
        let terminal_sequence = self.current().sequence;
        let terminal_record = self
            .current()
            .records
            .iter()
            .rev()
            .find(|record| matches!(record.action, b'A' | b'C' | b'M' | b'R'))
            .ok_or(ReferenceSemanticErrorV1::NoBookBearingTerminalRecord)?;
        let terminal_source_ordinal = terminal_record.source_ordinal;
        let levels = terminal_record.levels;
        let ordered_distinct_sequence_vector = self
            .blocks
            .iter()
            .map(|block| block.sequence)
            .collect::<Vec<_>>();
        let closure_confirmation_delay_ns = effective_available_ns
            .checked_sub(self.common_ts_recv)
            .ok_or_else(|| {
                ReferenceSemanticErrorV1::BookMutation(
                    "MBP availability precedes endpoint".to_owned(),
                )
            })?;

        Ok(ReferenceMbp10EndpointV1 {
            identity: self.identity,
            ordered_distinct_sequence_vector,
            terminal_sequence,
            terminal_source_ordinal,
            witness_source_ordinal: witness.source_ordinal,
            endpoint_ns: self.common_ts_recv,
            witness_ts_recv: witness.ts_recv,
            effective_available_ns,
            closure_confirmation_delay_ns,
            levels,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReferenceMbp10ValidityV1 {
    Valid,
    Recovering,
    Invalid,
}

#[derive(Debug)]
struct ReferenceMbp10IdentityStateV1 {
    last_valid_ts_recv: Option<u64>,
    candidate: Option<ReferenceMbp10CandidateV1>,
    validity: ReferenceMbp10ValidityV1,
}

impl Default for ReferenceMbp10IdentityStateV1 {
    fn default() -> Self {
        Self {
            last_valid_ts_recv: None,
            candidate: None,
            validity: ReferenceMbp10ValidityV1::Valid,
        }
    }
}

/// Streaming independent MBP-10 reference reducer for DECISION-031.
///
/// It consumes one globally ordered source image, but assembles blocks and
/// endpoints separately in each `(publisher_id, instrument_id)` projection.
/// The global receive watermark still includes every admitted finite,
/// non-BAD receive clock in the clean decoded prefix.
#[derive(Debug)]
pub(crate) struct ReferenceMbp10ReducerV1 {
    expected_identities: BTreeSet<XnasIdentityV1>,
    observed_identities: BTreeSet<XnasIdentityV1>,
    states: BTreeMap<XnasIdentityV1, ReferenceMbp10IdentityStateV1>,
    next_ordinal: u64,
    global_watermark: Option<u64>,
    counts: ReferenceMbp10CountsV1,
    terminal_error: Option<ReferenceSemanticErrorV1>,
}

impl ReferenceMbp10ReducerV1 {
    pub(crate) fn new(expected_identities: BTreeSet<XnasIdentityV1>) -> Self {
        Self {
            expected_identities,
            observed_identities: BTreeSet::new(),
            states: BTreeMap::new(),
            next_ordinal: 1,
            global_watermark: None,
            counts: ReferenceMbp10CountsV1::default(),
            terminal_error: None,
        }
    }

    pub(crate) fn counts(&self) -> &ReferenceMbp10CountsV1 {
        &self.counts
    }

    pub(crate) fn global_watermark(&self) -> Option<u64> {
        self.global_watermark
    }

    pub(crate) fn terminal_error(&self) -> Option<&ReferenceSemanticErrorV1> {
        self.terminal_error.as_ref()
    }

    pub(crate) fn population_reconciles(&self) -> bool {
        self.counts.population_reconciles()
            && self.counts.pending_record_count
                == self
                    .states
                    .values()
                    .filter_map(|state| state.candidate.as_ref())
                    .map(ReferenceMbp10CandidateV1::record_count)
                    .sum::<u64>()
    }

    pub(crate) fn push(
        &mut self,
        record: RawMbp10RecordV1,
    ) -> Result<Option<ReferenceMbp10EndpointV1>, ReferenceSemanticErrorV1> {
        if let Some(error) = &self.terminal_error {
            return Err(error.clone());
        }
        if record.source_ordinal.get() != self.next_ordinal {
            self.counts.admit();
            let error = ReferenceSemanticErrorV1::SourceOrdinalMismatch {
                expected: self.next_ordinal,
                observed: record.source_ordinal.get(),
            };
            return self.fail_terminal_admitted(error, 1);
        }
        self.next_ordinal += 1;
        self.counts.admit();

        let identity = record.identity();
        if !self.expected_identities.contains(&identity)
            || identity.publisher_id != XNAS_ITCH_PUBLISHER_ID
        {
            return self.fail_terminal_admitted(ReferenceSemanticErrorV1::UnexpectedIdentity, 1);
        }
        self.observed_identities.insert(identity);
        self.states.entry(identity).or_default();

        if record.rtype != DBN_RTYPE_MBP_10 {
            return self.fail_terminal_admitted(ReferenceSemanticErrorV1::WrongRecordType, 1);
        }
        if let Err(error) = validate_reference_mbp_receive_clock(&record) {
            return self.fail_identity_with_current(identity, error);
        }

        self.global_watermark = Some(
            self.global_watermark
                .map_or(record.ts_recv, |prior| prior.max(record.ts_recv)),
        );
        if self
            .states
            .get(&identity)
            .and_then(|state| state.last_valid_ts_recv)
            .is_some_and(|prior| record.ts_recv < prior)
        {
            return self
                .fail_identity_with_current(identity, ReferenceSemanticErrorV1::NonMonotoneTsRecv);
        }
        self.states
            .get_mut(&identity)
            .expect("MBP identity exists")
            .last_valid_ts_recv = Some(record.ts_recv);

        if let Err(error) = validate_reference_mbp_record(&record) {
            return self.fail_identity_with_current(identity, error);
        }

        if record.action == b'R' {
            let prior = self
                .states
                .get_mut(&identity)
                .expect("MBP identity exists")
                .candidate
                .take();
            if let Some(candidate) = prior {
                self.counts.quarantine(
                    &ReferenceSemanticErrorV1::ResetBoundary,
                    1,
                    candidate.record_count(),
                );
            }
            let state = self.states.get_mut(&identity).expect("MBP identity exists");
            state.validity = ReferenceMbp10ValidityV1::Recovering;
            state.candidate = Some(ReferenceMbp10CandidateV1::new(record));
            return Ok(None);
        }

        if self
            .states
            .get(&identity)
            .is_some_and(|state| state.validity == ReferenceMbp10ValidityV1::Invalid)
        {
            return self
                .fail_identity_with_current(identity, ReferenceSemanticErrorV1::InvalidState);
        }

        let candidate = self
            .states
            .get_mut(&identity)
            .expect("MBP identity exists")
            .candidate
            .take();
        let Some(mut candidate) = candidate else {
            self.states
                .get_mut(&identity)
                .expect("MBP identity exists")
                .candidate = Some(ReferenceMbp10CandidateV1::new(record));
            return Ok(None);
        };

        let candidate_record_count = candidate.record_count();
        let current_sequence = candidate.current().sequence;
        if record.sequence == current_sequence {
            if let Err(error) = candidate.append_same_sequence(record) {
                return self.fail_identity_detached(identity, error, candidate_record_count + 1);
            }
            self.states
                .get_mut(&identity)
                .expect("MBP identity exists")
                .candidate = Some(candidate);
            return Ok(None);
        }
        if record.sequence < current_sequence {
            return self.fail_identity_detached(
                identity,
                ReferenceSemanticErrorV1::SequenceRegressionOrReuse,
                candidate_record_count + 1,
            );
        }
        if !candidate.current().saw_last {
            if let Err(error) = candidate.append_nonterminal_sequence(record) {
                return self.fail_identity_detached(identity, error, candidate_record_count + 1);
            }
            self.states
                .get_mut(&identity)
                .expect("MBP identity exists")
                .candidate = Some(candidate);
            return Ok(None);
        }

        let effective_available_ns = self
            .global_watermark
            .expect("validated MBP witness establishes the watermark");
        let endpoint = match candidate.close(&record, effective_available_ns) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                return self.fail_identity_detached(identity, error, candidate_record_count + 1)
            }
        };
        let state = self.states.get_mut(&identity).expect("MBP identity exists");
        state.validity = ReferenceMbp10ValidityV1::Valid;
        state.candidate = Some(ReferenceMbp10CandidateV1::new(record));
        self.counts.complete(candidate_record_count);
        self.counts.completed_endpoint_count += 1;
        self.counts.sequence_block_count += endpoint.ordered_distinct_sequence_vector.len() as u64;
        Ok(Some(endpoint))
    }

    pub(crate) fn invalidate_boundary(
        &mut self,
        error: ReferenceSemanticErrorV1,
    ) -> ReferenceSemanticErrorV1 {
        debug_assert!(matches!(
            error,
            ReferenceSemanticErrorV1::SourceGap
                | ReferenceSemanticErrorV1::DecodeGap
                | ReferenceSemanticErrorV1::SessionBoundary
        ));
        let mut open_candidate_count = 0_u64;
        let mut record_count = 0_u64;
        for identity in &self.expected_identities {
            let state = self.states.entry(*identity).or_default();
            if let Some(candidate) = state.candidate.take() {
                open_candidate_count += 1;
                record_count += candidate.record_count();
            }
            state.validity = ReferenceMbp10ValidityV1::Invalid;
        }
        self.counts
            .quarantine(&error, open_candidate_count, record_count);
        error
    }

    pub(crate) fn finish(mut self) -> ReferenceMbp10FinishReportV1 {
        let mut terminal_error = self.terminal_error.take();
        if terminal_error.is_none() {
            if self
                .expected_identities
                .iter()
                .any(|identity| !self.observed_identities.contains(identity))
            {
                let error = ReferenceSemanticErrorV1::MissingExpectedIdentity;
                self.counts.quarantine(&error, 0, 0);
                terminal_error = Some(error);
            }
            for state in self.states.values_mut() {
                let validity = state.validity;
                if let Some(candidate) = state.candidate.take() {
                    let error = if validity == ReferenceMbp10ValidityV1::Recovering {
                        ReferenceSemanticErrorV1::InitializationIncompleteAtEof
                    } else if candidate.current().saw_last {
                        ReferenceSemanticErrorV1::TerminalAtEof
                    } else {
                        ReferenceSemanticErrorV1::OpenAtEof
                    };
                    self.counts.quarantine(&error, 1, candidate.record_count());
                    if validity == ReferenceMbp10ValidityV1::Recovering {
                        terminal_error.get_or_insert(error);
                    }
                } else if validity == ReferenceMbp10ValidityV1::Recovering {
                    let error = ReferenceSemanticErrorV1::InitializationIncompleteAtEof;
                    self.counts.quarantine(&error, 0, 0);
                    terminal_error.get_or_insert(error);
                } else if validity == ReferenceMbp10ValidityV1::Invalid {
                    terminal_error.get_or_insert(ReferenceSemanticErrorV1::InvalidState);
                }
            }
        }
        debug_assert!(self.counts.population_reconciles());
        debug_assert_eq!(self.counts.pending_record_count, 0);
        ReferenceMbp10FinishReportV1 {
            counts: self.counts,
            terminal_error,
        }
    }

    fn fail_identity_with_current<T>(
        &mut self,
        identity: XnasIdentityV1,
        error: ReferenceSemanticErrorV1,
    ) -> Result<T, ReferenceSemanticErrorV1> {
        let mut record_count = 1_u64;
        let mut open_candidate_count = 0_u64;
        if let Some(state) = self.states.get_mut(&identity) {
            if let Some(candidate) = state.candidate.take() {
                open_candidate_count = 1;
                record_count += candidate.record_count();
            }
            state.validity = ReferenceMbp10ValidityV1::Invalid;
        }
        self.counts
            .quarantine(&error, open_candidate_count, record_count);
        Err(error)
    }

    fn fail_identity_detached<T>(
        &mut self,
        identity: XnasIdentityV1,
        error: ReferenceSemanticErrorV1,
        record_count: u64,
    ) -> Result<T, ReferenceSemanticErrorV1> {
        if let Some(state) = self.states.get_mut(&identity) {
            state.candidate = None;
            state.validity = ReferenceMbp10ValidityV1::Invalid;
        }
        self.counts.quarantine(&error, 1, record_count);
        Err(error)
    }

    fn fail_terminal_admitted<T>(
        &mut self,
        error: ReferenceSemanticErrorV1,
        current_record_count: u64,
    ) -> Result<T, ReferenceSemanticErrorV1> {
        let (open_candidate_count, open_record_count) = self.drain_all_candidates();
        self.counts.quarantine(
            &error,
            open_candidate_count,
            open_record_count + current_record_count,
        );
        self.terminal_error = Some(error.clone());
        Err(error)
    }

    fn drain_all_candidates(&mut self) -> (u64, u64) {
        let mut open_candidate_count = 0_u64;
        let mut record_count = 0_u64;
        for state in self.states.values_mut() {
            if let Some(candidate) = state.candidate.take() {
                open_candidate_count += 1;
                record_count += candidate.record_count();
            }
            state.validity = ReferenceMbp10ValidityV1::Invalid;
        }
        (open_candidate_count, record_count)
    }
}

fn validate_reference_mbp_receive_clock(
    record: &RawMbp10RecordV1,
) -> Result<(), ReferenceSemanticErrorV1> {
    if record.ts_recv == DBN_UNDEF_TIMESTAMP {
        return Err(ReferenceSemanticErrorV1::UndefinedTsRecv);
    }
    if record.flags & DBN_FLAG_BAD_TS_RECV != 0 {
        return Err(ReferenceSemanticErrorV1::BadTsRecv);
    }
    Ok(())
}

fn validate_reference_mbp_record(
    record: &RawMbp10RecordV1,
) -> Result<(), ReferenceSemanticErrorV1> {
    if record.flags & DBN_FLAG_SNAPSHOT != 0 {
        return Err(ReferenceSemanticErrorV1::SnapshotBoundary);
    }
    if record.ts_event == DBN_UNDEF_TIMESTAMP {
        return Err(ReferenceSemanticErrorV1::UndefinedTsEvent);
    }
    if record.flags & DBN_FLAG_MAYBE_BAD_BOOK != 0 {
        return Err(ReferenceSemanticErrorV1::MaybeBadBook);
    }
    match record.action {
        b'A' | b'C' | b'M' if matches!(record.side, b'A' | b'B') => {}
        b'R' if record.side == b'N' => {}
        b'T' | b'F' if matches!(record.side, b'A' | b'B' | b'N') => {}
        b'A' | b'C' | b'M' | b'R' | b'T' | b'F' => {
            return Err(ReferenceSemanticErrorV1::UnsupportedSide(record.side))
        }
        action => return Err(ReferenceSemanticErrorV1::UnsupportedAction(action)),
    }
    if matches!(record.action, b'T' | b'F') && record.price == DBN_UNDEF_PRICE {
        return Err(ReferenceSemanticErrorV1::UndefinedExecutionPrice);
    }
    Ok(())
}

fn same_mbp10_payload(left: &RawMbp10RecordV1, right: &RawMbp10RecordV1) -> bool {
    left.rtype == right.rtype
        && left.publisher_id == right.publisher_id
        && left.instrument_id == right.instrument_id
        && left.ts_event == right.ts_event
        && left.price == right.price
        && left.size == right.size
        && left.action == right.action
        && left.side == right.side
        && left.flags == right.flags
        && left.depth == right.depth
        && left.ts_recv == right.ts_recv
        && left.ts_in_delta == right.ts_in_delta
        && left.sequence == right.sequence
        && left.levels == right.levels
}

#[cfg(test)]
mod mbp10_tests {
    use super::*;

    const INSTRUMENT: u32 = 11_667;
    const OTHER_INSTRUMENT: u32 = 22_334;

    fn ordinal(value: u64) -> SourceOrdinal {
        SourceOrdinal::new(value).expect("test ordinal is one-based")
    }

    fn levels(bid_size: u32) -> [Mbp10LevelV1; 10] {
        std::array::from_fn(|index| Mbp10LevelV1 {
            bid_px: 100_000_000_000 - index as i64 * 1_000_000,
            ask_px: 100_010_000_000 + index as i64 * 1_000_000,
            bid_sz: if index == 0 {
                bid_size
            } else {
                10 + index as u32
            },
            ask_sz: 20 + index as u32,
            bid_ct: 1 + index as u32,
            ask_ct: 2 + index as u32,
        })
    }

    fn record(
        ordinal_value: u64,
        instrument_id: u32,
        sequence: u32,
        ts_event: u64,
        ts_recv: u64,
        action: u8,
        flags: u8,
        endpoint_levels: [Mbp10LevelV1; 10],
    ) -> RawMbp10RecordV1 {
        RawMbp10RecordV1 {
            source_ordinal: ordinal(ordinal_value),
            rtype: DBN_RTYPE_MBP_10,
            publisher_id: XNAS_ITCH_PUBLISHER_ID,
            instrument_id,
            ts_event,
            price: 100_000_000_000,
            size: 7,
            action,
            side: match action {
                b'R' => b'N',
                b'T' | b'F' => b'A',
                _ => b'B',
            },
            flags,
            depth: 0,
            ts_recv,
            ts_in_delta: 10,
            sequence,
            levels: endpoint_levels,
        }
    }

    fn reducer(instruments: &[u32]) -> ReferenceMbp10ReducerV1 {
        ReferenceMbp10ReducerV1::new(
            instruments
                .iter()
                .map(|instrument_id| XnasIdentityV1::new(XNAS_ITCH_PUBLISHER_ID, *instrument_id))
                .collect(),
        )
    }

    #[test]
    fn repeated_last_uses_last_book_bearing_terminal_record_and_excludes_witness() {
        let mut reference = reducer(&[INSTRUMENT]);
        reference
            .push(record(
                1,
                INSTRUMENT,
                348_451,
                900,
                1_000,
                b'T',
                DBN_FLAG_LAST,
                levels(999),
            ))
            .unwrap();
        reference
            .push(record(
                2,
                INSTRUMENT,
                348_451,
                900,
                1_000,
                b'C',
                DBN_FLAG_LAST,
                levels(93),
            ))
            .unwrap();
        let endpoint = reference
            .push(record(
                3,
                INSTRUMENT,
                348_803,
                1_050,
                1_100,
                b'A',
                DBN_FLAG_LAST,
                levels(94),
            ))
            .unwrap()
            .expect("later clean sequence witnesses the prior terminal block");

        assert_eq!(endpoint.levels[0].bid_sz, 93);
        assert_eq!(endpoint.terminal_source_ordinal, ordinal(2));
        assert_eq!(endpoint.witness_source_ordinal, ordinal(3));
        assert_eq!(endpoint.ordered_distinct_sequence_vector, vec![348_451]);
        assert_eq!(endpoint.terminal_sequence, 348_451);
        assert_eq!(endpoint.endpoint_ns, 1_000);
        assert_eq!(endpoint.witness_ts_recv, 1_100);
        assert_eq!(endpoint.effective_available_ns, 1_100);
        assert_eq!(endpoint.closure_confirmation_delay_ns, 100);
        assert_eq!(reference.counts().completed_member_record_count, 2);
        assert_eq!(reference.counts().pending_record_count, 1);
        assert_eq!(reference.counts().completed_endpoint_count, 1);
        assert_eq!(reference.counts().sequence_block_count, 1);
        assert!(reference.population_reconciles());
    }

    #[test]
    fn identity_projection_and_global_watermark_are_independent_concerns() {
        let mut reference = reducer(&[INSTRUMENT, OTHER_INSTRUMENT]);
        reference
            .push(record(1, INSTRUMENT, 10, 90, 100, b'A', 0, levels(100)))
            .unwrap();
        reference
            .push(record(
                2,
                OTHER_INSTRUMENT,
                500,
                900,
                1_000,
                b'A',
                DBN_FLAG_LAST,
                levels(200),
            ))
            .unwrap();
        reference
            .push(record(
                3,
                INSTRUMENT,
                20,
                91,
                100,
                b'C',
                DBN_FLAG_LAST,
                levels(90),
            ))
            .unwrap();
        let endpoint = reference
            .push(record(
                4,
                INSTRUMENT,
                50,
                190,
                200,
                b'A',
                DBN_FLAG_LAST,
                levels(91),
            ))
            .unwrap()
            .expect("same-identity record is the witness");

        assert_eq!(endpoint.ordered_distinct_sequence_vector, vec![10, 20]);
        assert_eq!(endpoint.terminal_source_ordinal, ordinal(3));
        assert_eq!(endpoint.witness_source_ordinal, ordinal(4));
        assert_eq!(endpoint.levels[0].bid_sz, 90);
        assert_eq!(endpoint.effective_available_ns, 1_000);
        assert_eq!(endpoint.closure_confirmation_delay_ns, 900);
        assert_eq!(reference.global_watermark(), Some(1_000));
        assert_eq!(reference.counts().completed_member_record_count, 2);
        assert_eq!(reference.counts().pending_record_count, 2);
        assert_eq!(reference.counts().sequence_block_count, 2);
        assert!(reference.population_reconciles());
    }

    #[test]
    fn rejected_finite_receive_clock_still_lifts_global_watermark() {
        let mut reference = reducer(&[INSTRUMENT, OTHER_INSTRUMENT]);
        reference
            .push(record(
                1,
                INSTRUMENT,
                10,
                90,
                100,
                b'A',
                DBN_FLAG_LAST,
                levels(100),
            ))
            .unwrap();
        assert_eq!(
            reference
                .push(record(
                    2,
                    OTHER_INSTRUMENT,
                    500,
                    900,
                    1_000,
                    b'A',
                    DBN_FLAG_MAYBE_BAD_BOOK,
                    levels(200),
                ))
                .unwrap_err(),
            ReferenceSemanticErrorV1::MaybeBadBook
        );
        let endpoint = reference
            .push(record(
                3,
                INSTRUMENT,
                20,
                190,
                200,
                b'A',
                DBN_FLAG_LAST,
                levels(101),
            ))
            .unwrap()
            .expect("the clean same-identity record witnesses the prior endpoint");

        assert_eq!(reference.global_watermark(), Some(1_000));
        assert_eq!(endpoint.witness_ts_recv, 200);
        assert_eq!(endpoint.effective_available_ns, 1_000);
        assert_eq!(endpoint.closure_confirmation_delay_ns, 900);
        assert_eq!(
            reference.counts().quarantined_by_reason["MAYBE_BAD_BOOK"].record_count,
            1
        );
        assert!(reference.population_reconciles());
    }

    #[test]
    fn last_to_non_last_quarantines_exact_population_with_stable_code() {
        let mut reference = reducer(&[INSTRUMENT]);
        reference
            .push(record(
                1,
                INSTRUMENT,
                10,
                90,
                100,
                b'A',
                DBN_FLAG_LAST,
                levels(100),
            ))
            .unwrap();
        let error = reference
            .push(record(2, INSTRUMENT, 10, 90, 100, b'C', 0, levels(90)))
            .unwrap_err();

        assert_eq!(error, ReferenceSemanticErrorV1::LastToNonLast);
        assert_eq!(error.code(), "LAST_TO_NON_LAST");
        assert_eq!(reference.counts().raw_record_count, 2);
        assert_eq!(reference.counts().quarantined_record_count, 2);
        assert_eq!(
            reference.counts().quarantined_by_reason["LAST_TO_NON_LAST"],
            ReferenceQuarantinePopulationV1 {
                incident_count: 1,
                open_candidate_count: 1,
                record_count: 2,
            }
        );
        assert!(reference.population_reconciles());

        let report = reference.finish();
        assert_eq!(
            report.terminal_error,
            Some(ReferenceSemanticErrorV1::InvalidState)
        );
        assert!(report.counts.population_reconciles());
    }

    #[test]
    fn terminal_block_without_book_record_fails_closed() {
        let mut reference = reducer(&[INSTRUMENT]);
        reference
            .push(record(1, INSTRUMENT, 10, 90, 100, b'A', 0, levels(100)))
            .unwrap();
        reference
            .push(record(
                2,
                INSTRUMENT,
                20,
                91,
                100,
                b'T',
                DBN_FLAG_LAST,
                levels(999),
            ))
            .unwrap();
        let error = reference
            .push(record(
                3,
                INSTRUMENT,
                30,
                190,
                200,
                b'A',
                DBN_FLAG_LAST,
                levels(101),
            ))
            .unwrap_err();

        assert_eq!(error, ReferenceSemanticErrorV1::NoBookBearingTerminalRecord);
        assert_eq!(error.code(), "NO_BOOK_BEARING_TERMINAL_RECORD");
        assert_eq!(reference.counts().quarantined_record_count, 3);
        assert_eq!(
            reference.counts().quarantined_by_reason["NO_BOOK_BEARING_TERMINAL_RECORD"]
                .record_count,
            3
        );
        assert!(reference.population_reconciles());
    }

    #[test]
    fn source_boundary_requires_a_clean_witnessed_reset() {
        let mut reference = reducer(&[INSTRUMENT]);
        reference
            .push(record(
                1,
                INSTRUMENT,
                10,
                90,
                100,
                b'A',
                DBN_FLAG_LAST,
                levels(100),
            ))
            .unwrap();
        assert_eq!(
            reference.invalidate_boundary(ReferenceSemanticErrorV1::SourceGap),
            ReferenceSemanticErrorV1::SourceGap
        );
        assert_eq!(
            reference
                .push(record(
                    2,
                    INSTRUMENT,
                    20,
                    190,
                    200,
                    b'A',
                    DBN_FLAG_LAST,
                    levels(101),
                ))
                .unwrap_err(),
            ReferenceSemanticErrorV1::InvalidState
        );
        reference
            .push(record(
                3,
                INSTRUMENT,
                30,
                290,
                300,
                b'R',
                DBN_FLAG_LAST,
                levels(0),
            ))
            .unwrap();
        let reset_endpoint = reference
            .push(record(
                4,
                INSTRUMENT,
                40,
                390,
                400,
                b'A',
                DBN_FLAG_LAST,
                levels(102),
            ))
            .unwrap()
            .expect("clean later sequence witnesses reset recovery");
        assert_eq!(reset_endpoint.terminal_sequence, 30);
        assert_eq!(reset_endpoint.levels, levels(0));
        assert_eq!(
            reference.counts().quarantined_by_reason["SOURCE_GAP"].record_count,
            1
        );
        assert_eq!(
            reference.counts().quarantined_by_reason["INVALID_STATE"].record_count,
            1
        );
        assert!(reference.population_reconciles());

        let report = reference.finish();
        assert_eq!(report.terminal_error, None);
        assert_eq!(
            report.counts.quarantined_by_reason["TERMINAL_AT_EOF"].record_count,
            1
        );
        assert_eq!(report.counts.pending_record_count, 0);
        assert!(report.counts.population_reconciles());
    }
}
