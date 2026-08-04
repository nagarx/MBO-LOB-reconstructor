use super::XnasIdentityV1;
use ahash::AHashSet;
use hft_mbo_event_contract::{
    AggressorSideV1, BookCommandV1, EventDispositionV1, ExecutionCarrierV1, RawMboEventV1,
    RestingSideV1, Sha256DigestV1, FLAG_LAST,
};
use sha2::{Digest, Sha256};

#[derive(Debug)]
pub(crate) struct OpenEnvelopeV1 {
    source_digest: Sha256DigestV1,
    identity: XnasIdentityV1,
    channel_id: u8,
    common_ts_recv: u64,
    current_sequence: u32,
    current_ts_event: u64,
    current_saw_last: bool,
    current_has_execution: bool,
    completed_execution_blocks: u64,
    sequences: Vec<u32>,
    events: Vec<EventDispositionV1>,
    provider_payloads: AHashSet<ProviderPayloadKeyV1>,
    max_members: usize,
    max_sequence_blocks: usize,
    recovery: bool,
}

impl OpenEnvelopeV1 {
    pub(crate) fn new(
        event: EventDispositionV1,
        recovery: bool,
        max_members: usize,
        max_sequence_blocks: usize,
    ) -> Self {
        let raw = event.event().raw();
        let mut provider_payloads = AHashSet::new();
        provider_payloads.insert(ProviderPayloadKeyV1::from(raw));
        Self {
            source_digest: raw.source_object_sha256,
            identity: XnasIdentityV1::new(raw.publisher_id, raw.instrument_id),
            channel_id: raw.channel_id,
            common_ts_recv: raw.ts_recv,
            current_sequence: raw.sequence,
            current_ts_event: raw.ts_event,
            current_saw_last: is_last(raw),
            current_has_execution: matches!(event, EventDispositionV1::Execution(_)),
            completed_execution_blocks: 0,
            sequences: vec![raw.sequence],
            events: vec![event],
            provider_payloads,
            max_members,
            max_sequence_blocks,
            recovery,
        }
    }

    pub(crate) const fn current_sequence(&self) -> u32 {
        self.current_sequence
    }

    pub(crate) const fn current_saw_last(&self) -> bool {
        self.current_saw_last
    }

    pub(crate) const fn channel_id(&self) -> u8 {
        self.channel_id
    }

    pub(crate) fn len(&self) -> usize {
        self.events.len()
    }

    pub(crate) fn first_source_ordinal(&self) -> u64 {
        self.events
            .first()
            .expect("an open envelope is nonempty")
            .event()
            .raw()
            .raw_ordinal
    }

    pub(crate) fn last_source_ordinal(&self) -> u64 {
        self.events
            .last()
            .expect("an open envelope is nonempty")
            .event()
            .raw()
            .raw_ordinal
    }

    pub(crate) fn source_ordinals(&self) -> impl Iterator<Item = u64> + '_ {
        self.events
            .iter()
            .map(|event| event.event().raw().raw_ordinal)
    }

    pub(crate) const fn is_recovery(&self) -> bool {
        self.recovery
    }

    pub(crate) fn contains_provider_payload(&self, raw: &RawMboEventV1) -> bool {
        self.provider_payloads
            .contains(&ProviderPayloadKeyV1::from(raw))
    }

    pub(crate) fn append_same_block(
        &mut self,
        event: EventDispositionV1,
    ) -> Result<(), EnvelopeAssemblyErrorV1> {
        let raw = event.event().raw();
        self.validate_identity_source_and_ordinal(raw)?;
        if raw.sequence != self.current_sequence {
            return Err(EnvelopeAssemblyErrorV1::WrongSameBlockSequence);
        }
        if raw.channel_id != self.channel_id {
            return Err(EnvelopeAssemblyErrorV1::ChannelChange);
        }
        if raw.ts_event != self.current_ts_event || raw.ts_recv != self.common_ts_recv {
            return Err(EnvelopeAssemblyErrorV1::BlockTimestampMismatch);
        }
        if self.events.len() >= self.max_members {
            return Err(EnvelopeAssemblyErrorV1::MemberLimit {
                limit: self.max_members,
            });
        }
        let payload = ProviderPayloadKeyV1::from(raw);
        if self.provider_payloads.contains(&payload) {
            return Err(EnvelopeAssemblyErrorV1::ExactDuplicate);
        }
        if self.current_saw_last && !is_last(raw) {
            return Err(EnvelopeAssemblyErrorV1::LastToNonLast);
        }
        self.current_saw_last |= is_last(raw);
        self.current_has_execution |= matches!(event, EventDispositionV1::Execution(_));
        self.provider_payloads.insert(payload);
        self.events.push(event);
        Ok(())
    }

    pub(crate) fn append_next_block(
        &mut self,
        event: EventDispositionV1,
    ) -> Result<(), EnvelopeAssemblyErrorV1> {
        let raw = event.event().raw();
        self.validate_identity_source_and_ordinal(raw)?;
        if raw.channel_id != self.channel_id {
            return Err(EnvelopeAssemblyErrorV1::ChannelChange);
        }
        let payload = ProviderPayloadKeyV1::from(raw);
        if self.provider_payloads.contains(&payload) {
            return Err(EnvelopeAssemblyErrorV1::ExactDuplicate);
        }
        if raw.sequence <= self.current_sequence {
            return Err(EnvelopeAssemblyErrorV1::SequenceRegressionOrReuse);
        }
        if raw.ts_recv != self.common_ts_recv {
            return Err(EnvelopeAssemblyErrorV1::ReceiveTimeChangedBeforeTerminal);
        }
        if self.events.len() >= self.max_members {
            return Err(EnvelopeAssemblyErrorV1::MemberLimit {
                limit: self.max_members,
            });
        }
        if self.sequences.len() >= self.max_sequence_blocks {
            return Err(EnvelopeAssemblyErrorV1::SequenceBlockLimit {
                limit: self.max_sequence_blocks,
            });
        }
        if self.current_has_execution {
            self.completed_execution_blocks = self
                .completed_execution_blocks
                .checked_add(1)
                .ok_or(EnvelopeAssemblyErrorV1::CountOverflow)?;
        }
        self.sequences.push(raw.sequence);
        self.current_sequence = raw.sequence;
        self.current_ts_event = raw.ts_event;
        self.current_saw_last = is_last(raw);
        self.current_has_execution = matches!(event, EventDispositionV1::Execution(_));
        self.provider_payloads.insert(payload);
        self.events.push(event);
        Ok(())
    }

    pub(crate) fn close(
        self,
        witness: EventDispositionV1,
        effective_available_ns: u64,
    ) -> Result<ReadyEnvelopeTxnV1, EnvelopeAssemblyErrorV1> {
        let witness_channel_id = witness.event().raw().channel_id;
        let witness_sequence = witness.event().raw().sequence;
        let witness_ts_recv = witness.event().raw().ts_recv;
        self.validate_identity_source_and_ordinal(witness.event().raw())?;
        if witness_channel_id != self.channel_id {
            return Err(EnvelopeAssemblyErrorV1::ChannelChange);
        }
        if witness_sequence <= self.current_sequence {
            return Err(EnvelopeAssemblyErrorV1::SequenceRegressionOrReuse);
        }
        if !self.current_saw_last {
            return Err(EnvelopeAssemblyErrorV1::NotTerminal);
        }
        let terminal_source_ordinal = self
            .events
            .last()
            .expect("an open envelope is nonempty")
            .event()
            .raw()
            .raw_ordinal;
        let execution_sequence_blocks = self
            .completed_execution_blocks
            .checked_add(u64::from(self.current_has_execution))
            .ok_or(EnvelopeAssemblyErrorV1::CountOverflow)?;
        let execution_carriers = u64::try_from(
            self.events
                .iter()
                .filter(|event| matches!(event, EventDispositionV1::Execution(_)))
                .count(),
        )
        .map_err(|_| EnvelopeAssemblyErrorV1::CountOverflow)?;
        let endpoint_ns = self.common_ts_recv;
        let closure_confirmation_delay_ns = effective_available_ns
            .checked_sub(endpoint_ns)
            .ok_or(EnvelopeAssemblyErrorV1::AvailabilityBeforeEndpoint)?;
        Ok(ReadyEnvelopeTxnV1 {
            identity: self.identity,
            channel_id: self.channel_id,
            ordered_distinct_sequences: self.sequences,
            terminal_sequence: self.current_sequence,
            events: self.events,
            terminal_source_ordinal,
            witness,
            endpoint_ns,
            witness_ts_recv,
            effective_available_ns,
            closure_confirmation_delay_ns,
            execution_sequence_blocks,
            execution_carriers,
            recovery: self.recovery,
        })
    }

    fn validate_identity_source_and_ordinal(
        &self,
        raw: &RawMboEventV1,
    ) -> Result<(), EnvelopeAssemblyErrorV1> {
        if raw.source_object_sha256 != self.source_digest {
            return Err(EnvelopeAssemblyErrorV1::SourceChange);
        }
        if XnasIdentityV1::new(raw.publisher_id, raw.instrument_id) != self.identity {
            return Err(EnvelopeAssemblyErrorV1::IdentityChange);
        }
        if self
            .events
            .last()
            .is_some_and(|event| raw.raw_ordinal <= event.event().raw().raw_ordinal)
        {
            return Err(EnvelopeAssemblyErrorV1::NonIncreasingSourceOrdinal);
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct ReadyEnvelopeTxnV1 {
    identity: XnasIdentityV1,
    channel_id: u8,
    ordered_distinct_sequences: Vec<u32>,
    terminal_sequence: u32,
    events: Vec<EventDispositionV1>,
    terminal_source_ordinal: u64,
    witness: EventDispositionV1,
    endpoint_ns: u64,
    witness_ts_recv: u64,
    effective_available_ns: u64,
    closure_confirmation_delay_ns: u64,
    execution_sequence_blocks: u64,
    execution_carriers: u64,
    recovery: bool,
}

impl ReadyEnvelopeTxnV1 {
    pub(crate) const fn identity(&self) -> XnasIdentityV1 {
        self.identity
    }

    pub(crate) fn sequences(&self) -> &[u32] {
        &self.ordered_distinct_sequences
    }

    pub(crate) fn events(&self) -> &[EventDispositionV1] {
        &self.events
    }

    pub(crate) const fn terminal_sequence(&self) -> u32 {
        self.terminal_sequence
    }

    pub(crate) const fn terminal_source_ordinal(&self) -> u64 {
        self.terminal_source_ordinal
    }

    pub(crate) const fn witness(&self) -> &EventDispositionV1 {
        &self.witness
    }

    pub(crate) const fn endpoint_ns(&self) -> u64 {
        self.endpoint_ns
    }

    pub(crate) const fn witness_ts_recv(&self) -> u64 {
        self.witness_ts_recv
    }

    pub(crate) const fn effective_available_ns(&self) -> u64 {
        self.effective_available_ns
    }

    pub(crate) const fn closure_confirmation_delay_ns(&self) -> u64 {
        self.closure_confirmation_delay_ns
    }

    pub(crate) const fn execution_sequence_blocks(&self) -> u64 {
        self.execution_sequence_blocks
    }

    pub(crate) const fn execution_carrier_count(&self) -> u64 {
        self.execution_carriers
    }

    pub(crate) const fn is_recovery(&self) -> bool {
        self.recovery
    }

    #[cfg(test)]
    pub(crate) fn synthetic_for_book_test(
        events: Vec<EventDispositionV1>,
        witness: EventDispositionV1,
        recovery: bool,
    ) -> Self {
        let first = events.first().expect("test envelope is nonempty");
        let last = events.last().expect("test envelope is nonempty");
        let first_raw = first.event().raw();
        let last_raw = last.event().raw();
        Self {
            identity: XnasIdentityV1::new(first_raw.publisher_id, first_raw.instrument_id),
            channel_id: first_raw.channel_id,
            ordered_distinct_sequences: vec![first_raw.sequence],
            terminal_sequence: last_raw.sequence,
            terminal_source_ordinal: last_raw.raw_ordinal,
            endpoint_ns: first_raw.ts_recv,
            witness_ts_recv: witness.event().raw().ts_recv,
            effective_available_ns: witness.event().raw().ts_recv,
            closure_confirmation_delay_ns: witness
                .event()
                .raw()
                .ts_recv
                .checked_sub(first_raw.ts_recv)
                .expect("test witness follows endpoint"),
            execution_sequence_blocks: 0,
            execution_carriers: u64::try_from(
                events
                    .iter()
                    .filter(|event| matches!(event, EventDispositionV1::Execution(_)))
                    .count(),
            )
            .expect("usize always fits u64 on supported targets"),
            events,
            witness,
            recovery,
        }
    }
}

impl ReadyEnvelopeTxnV1 {
    pub(crate) fn book_commands(&self) -> impl Iterator<Item = &BookCommandV1> {
        self.events.iter().filter_map(|event| match event {
            EventDispositionV1::Book(command) => Some(command),
            EventDispositionV1::Execution(_) | EventDispositionV1::Control(_) => None,
        })
    }

    /// Move the two variable-length public observation populations without a
    /// second allocation or per-envelope clone in the revalidation hot path.
    pub(crate) fn into_sequences_and_events(self) -> (Vec<u32>, Vec<EventDispositionV1>) {
        (self.ordered_distinct_sequences, self.events)
    }

    #[cfg(test)]
    pub(crate) fn execution_carriers(&self) -> impl Iterator<Item = &ExecutionCarrierV1> {
        self.events.iter().filter_map(|event| match event {
            EventDispositionV1::Execution(carrier) => Some(carrier),
            EventDispositionV1::Book(_) | EventDispositionV1::Control(_) => None,
        })
    }

    pub(crate) fn commitment(&self, source_digest: Sha256DigestV1) -> ReadyEnvelopeCommitmentV2 {
        let mut hasher = Sha256::new();
        hasher.update(b"hft.xnas_ready_envelope.v2\0");
        hasher.update(source_digest.as_bytes());
        hasher.update(self.identity.publisher_id().to_le_bytes());
        hasher.update(self.identity.instrument_id().to_le_bytes());
        hasher.update([self.channel_id]);
        let sequence_count = u64::try_from(self.ordered_distinct_sequences.len())
            .expect("usize always fits u64 on supported targets");
        hasher.update(sequence_count.to_le_bytes());
        for sequence in &self.ordered_distinct_sequences {
            hasher.update(sequence.to_le_bytes());
        }
        hasher.update(self.terminal_sequence.to_le_bytes());
        hasher.update(self.terminal_source_ordinal.to_le_bytes());
        hasher.update(self.witness.event().raw().raw_ordinal.to_le_bytes());
        hasher.update(self.endpoint_ns.to_le_bytes());
        hasher.update(self.witness_ts_recv.to_le_bytes());
        hasher.update(self.effective_available_ns.to_le_bytes());
        hasher.update([u8::from(self.recovery)]);
        hasher.update(
            u64::try_from(self.events.len())
                .expect("usize always fits u64 on supported targets")
                .to_le_bytes(),
        );
        for event in &self.events {
            hash_event_semantics(&mut hasher, event);
            hash_raw_event(&mut hasher, event.event().raw());
        }
        ReadyEnvelopeCommitmentV2(Sha256DigestV1::from_bytes(hasher.finalize().into()))
    }
}

/// Private, typed custody for the canonical ready-envelope v2 encoding.
///
/// Construction is deliberately confined to `ReadyEnvelopeTxnV1`, so the
/// book transition and exported observation must consume the exact same
/// commitment rather than independently re-hashing or accepting arbitrary
/// bytes from a caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReadyEnvelopeCommitmentV2(Sha256DigestV1);

impl ReadyEnvelopeCommitmentV2 {
    pub(crate) const fn sha256(self) -> Sha256DigestV1 {
        self.0
    }
}

/// Bind the interpreted public event lane separately from provider bytes.
/// The encoding belongs to ready-envelope digest v2 and is intentionally
/// exhaustive, so a new semantic variant cannot silently inherit an identity.
pub(crate) fn hash_event_semantics(hasher: &mut Sha256, event: &EventDispositionV1) {
    hasher.update(event_semantic_tag(event));
}

pub(crate) const fn event_semantic_tag(event: &EventDispositionV1) -> [u8; 3] {
    match event {
        EventDispositionV1::Book(BookCommandV1::Add(command)) => {
            [0, 0, resting_side_code(command.resting_side())]
        }
        EventDispositionV1::Book(BookCommandV1::Modify(command)) => {
            [0, 1, resting_side_code(command.resting_side())]
        }
        EventDispositionV1::Book(BookCommandV1::Cancel(command)) => {
            [0, 2, resting_side_code(command.resting_side())]
        }
        EventDispositionV1::Book(BookCommandV1::Clear(_)) => [0, 3, 0],
        EventDispositionV1::Execution(ExecutionCarrierV1::AggressorTrade(trade)) => {
            let aggressor = match trade.aggressor() {
                AggressorSideV1::Seller => 1,
                AggressorSideV1::Buyer => 2,
            };
            [1, 0, aggressor]
        }
        EventDispositionV1::Execution(ExecutionCarrierV1::UnsignedTrade(_)) => [1, 1, 0],
        EventDispositionV1::Execution(ExecutionCarrierV1::RestingFill(fill)) => {
            let side = match fill.resting_side() {
                None => 0,
                Some(side) => resting_side_code(side),
            };
            [1, 2, side]
        }
        EventDispositionV1::Control(_) => [2, 0, 0],
    }
}

const fn resting_side_code(side: RestingSideV1) -> u8 {
    match side {
        RestingSideV1::Ask => 1,
        RestingSideV1::Bid => 2,
    }
}

fn is_last(raw: &RawMboEventV1) -> bool {
    raw.flags_raw & FLAG_LAST != 0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ProviderPayloadKeyV1 {
    rtype: u8,
    record_size_bytes: u16,
    publisher_id: u16,
    instrument_id: u32,
    ts_event: u64,
    ts_recv: u64,
    ts_in_delta: i32,
    channel_id: u8,
    sequence: u32,
    order_id: u64,
    price_raw: i64,
    size_raw: u32,
    flags_raw: u8,
    action_raw: u8,
    side_raw: u8,
}

impl From<&RawMboEventV1> for ProviderPayloadKeyV1 {
    fn from(raw: &RawMboEventV1) -> Self {
        Self {
            rtype: raw.rtype,
            record_size_bytes: raw.record_size_bytes,
            publisher_id: raw.publisher_id,
            instrument_id: raw.instrument_id,
            ts_event: raw.ts_event,
            ts_recv: raw.ts_recv,
            ts_in_delta: raw.ts_in_delta,
            channel_id: raw.channel_id,
            sequence: raw.sequence,
            order_id: raw.order_id,
            price_raw: raw.price_raw,
            size_raw: raw.size_raw,
            flags_raw: raw.flags_raw,
            action_raw: raw.action_raw,
            side_raw: raw.side_raw,
        }
    }
}

pub(crate) fn hash_raw_event(hasher: &mut Sha256, raw: &RawMboEventV1) {
    hasher.update(raw.source_object_sha256.as_bytes());
    hasher.update(raw.raw_ordinal.to_le_bytes());
    hasher.update(raw.subordinal.to_le_bytes());
    hasher.update([raw.rtype]);
    hasher.update(raw.record_size_bytes.to_le_bytes());
    hasher.update(raw.publisher_id.to_le_bytes());
    hasher.update(raw.instrument_id.to_le_bytes());
    hasher.update(raw.ts_event.to_le_bytes());
    hasher.update(raw.ts_recv.to_le_bytes());
    hasher.update(raw.ts_in_delta.to_le_bytes());
    hasher.update([raw.channel_id]);
    hasher.update(raw.sequence.to_le_bytes());
    hasher.update(raw.order_id.to_le_bytes());
    hasher.update(raw.price_raw.to_le_bytes());
    hasher.update(raw.size_raw.to_le_bytes());
    hasher.update([raw.flags_raw, raw.action_raw, raw.side_raw]);
}

/// Append the canonical raw-event encoding without hashing it. This is the
/// same byte sequence as `hash_raw_event` and allows callers to batch many
/// small fields into one SHA-256 update without changing the digest contract.
pub(crate) fn encode_raw_event(output: &mut Vec<u8>, raw: &RawMboEventV1) {
    output.extend_from_slice(raw.source_object_sha256.as_bytes());
    output.extend_from_slice(&raw.raw_ordinal.to_le_bytes());
    output.extend_from_slice(&raw.subordinal.to_le_bytes());
    output.push(raw.rtype);
    output.extend_from_slice(&raw.record_size_bytes.to_le_bytes());
    output.extend_from_slice(&raw.publisher_id.to_le_bytes());
    output.extend_from_slice(&raw.instrument_id.to_le_bytes());
    output.extend_from_slice(&raw.ts_event.to_le_bytes());
    output.extend_from_slice(&raw.ts_recv.to_le_bytes());
    output.extend_from_slice(&raw.ts_in_delta.to_le_bytes());
    output.push(raw.channel_id);
    output.extend_from_slice(&raw.sequence.to_le_bytes());
    output.extend_from_slice(&raw.order_id.to_le_bytes());
    output.extend_from_slice(&raw.price_raw.to_le_bytes());
    output.extend_from_slice(&raw.size_raw.to_le_bytes());
    output.extend_from_slice(&[raw.flags_raw, raw.action_raw, raw.side_raw]);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, thiserror::Error)]
pub enum EnvelopeAssemblyErrorV1 {
    #[error("records in one sequence block disagree on event or receive timestamp")]
    BlockTimestampMismatch,
    #[error("exact duplicate provider payload")]
    ExactDuplicate,
    #[error("source digest changed inside one envelope or at its witness")]
    SourceChange,
    #[error("publisher/instrument identity changed inside one envelope or at its witness")]
    IdentityChange,
    #[error("source ordinal did not increase inside one identity projection")]
    NonIncreasingSourceOrdinal,
    #[error("same-block append used a different sequence")]
    WrongSameBlockSequence,
    #[error("open envelope exceeds its configured member limit {limit}")]
    MemberLimit { limit: usize },
    #[error("open envelope exceeds its configured sequence-block limit {limit}")]
    SequenceBlockLimit { limit: usize },
    #[error("LAST-to-non-LAST transition inside one sequence block")]
    LastToNonLast,
    #[error("channel changed inside an open envelope or at its witness")]
    ChannelChange,
    #[error("sequence regressed or was reused")]
    SequenceRegressionOrReuse,
    #[error("receive time changed before terminality")]
    ReceiveTimeChangedBeforeTerminal,
    #[error("attempted to close a nonterminal envelope")]
    NotTerminal,
    #[error("causal availability precedes the envelope endpoint")]
    AvailabilityBeforeEndpoint,
    #[error("envelope counter overflow")]
    CountOverflow,
}

#[cfg(test)]
mod tests {
    use super::*;
    use hft_mbo_event_contract::{
        classify_full_order_book, validate_raw_event, BoundPublisherPolicyV1, LogicalSourceV1,
        OpenedReplicaV1, OpenedRepresentationV1, PublisherPolicyIdV1, SourceDescriptorV1,
        ACTION_ADD, ACTION_CANCEL, ACTION_CLEAR, ACTION_FILL, ACTION_MODIFY, ACTION_NONE,
        ACTION_TRADE, EXPECTED_MBO_RECORD_SIZE_BYTES, EXPECTED_MBO_RTYPE, SIDE_ASK, SIDE_BID,
        SIDE_NONE, UNDEF_PRICE,
    };

    fn digest(byte: u8) -> Sha256DigestV1 {
        Sha256DigestV1::from_bytes([byte; 32])
    }

    fn policy(source_digest: Sha256DigestV1) -> BoundPublisherPolicyV1 {
        let source = SourceDescriptorV1 {
            logical: LogicalSourceV1 {
                catalog_release_id: "test".into(),
                catalog_object_id: "test".into(),
                canonical_path: "/test.dbn".into(),
                canonical_sha256: source_digest,
                canonical_bytes: 1,
                dbn_version: 1,
                dbn_ts_out: false,
                dataset: "XNAS.ITCH".into(),
                schema: "mbo".into(),
            },
            opened: OpenedReplicaV1 {
                configured_path: "/test.dbn".into(),
                opened_path: "/test.dbn".into(),
                representation: OpenedRepresentationV1::CanonicalObject,
                opened_sha256: source_digest,
                opened_bytes: 1,
            },
        };
        BoundPublisherPolicyV1::bind(PublisherPolicyIdV1::XnasItchHistorical, &source).unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    fn event(
        source_digest: Sha256DigestV1,
        ordinal: u64,
        instrument: u32,
        channel: u8,
        sequence: u32,
        ts_event: u64,
        ts_recv: u64,
        flags: u8,
        order_id: u64,
    ) -> EventDispositionV1 {
        classify_full_order_book(
            validate_raw_event(RawMboEventV1 {
                source_object_sha256: source_digest,
                raw_ordinal: ordinal,
                subordinal: 0,
                rtype: EXPECTED_MBO_RTYPE,
                record_size_bytes: EXPECTED_MBO_RECORD_SIZE_BYTES,
                publisher_id: 2,
                instrument_id: instrument,
                ts_event,
                ts_recv,
                ts_in_delta: 0,
                channel_id: channel,
                sequence,
                order_id,
                price_raw: 100_000_000_000,
                size_raw: 1,
                flags_raw: flags,
                action_raw: ACTION_ADD,
                side_raw: SIDE_BID,
            })
            .unwrap(),
            &policy(source_digest),
        )
        .unwrap()
    }

    fn semantic_event(action_raw: u8, side_raw: u8) -> EventDispositionV1 {
        let order_required = matches!(action_raw, ACTION_ADD | ACTION_MODIFY | ACTION_CANCEL);
        let price_required = order_required || matches!(action_raw, ACTION_TRADE | ACTION_FILL);
        classify_full_order_book(
            validate_raw_event(RawMboEventV1 {
                source_object_sha256: digest(1),
                raw_ordinal: 1,
                subordinal: 0,
                rtype: EXPECTED_MBO_RTYPE,
                record_size_bytes: EXPECTED_MBO_RECORD_SIZE_BYTES,
                publisher_id: 2,
                instrument_id: 101,
                ts_event: 1_000,
                ts_recv: 2_000,
                ts_in_delta: 0,
                channel_id: 0,
                sequence: 10,
                order_id: u64::from(order_required),
                price_raw: if price_required {
                    100_000_000_000
                } else {
                    UNDEF_PRICE
                },
                size_raw: u32::from(price_required),
                flags_raw: FLAG_LAST,
                action_raw,
                side_raw,
            })
            .unwrap(),
            &policy(digest(1)),
        )
        .unwrap()
    }

    #[test]
    fn semantic_tags_are_stable_distinct_and_exhaustive_for_current_lanes() {
        let cases = [
            (ACTION_ADD, SIDE_ASK, [0, 0, 1]),
            (ACTION_ADD, SIDE_BID, [0, 0, 2]),
            (ACTION_MODIFY, SIDE_ASK, [0, 1, 1]),
            (ACTION_MODIFY, SIDE_BID, [0, 1, 2]),
            (ACTION_CANCEL, SIDE_ASK, [0, 2, 1]),
            (ACTION_CANCEL, SIDE_BID, [0, 2, 2]),
            (ACTION_CLEAR, SIDE_NONE, [0, 3, 0]),
            (ACTION_TRADE, SIDE_ASK, [1, 0, 1]),
            (ACTION_TRADE, SIDE_BID, [1, 0, 2]),
            (ACTION_TRADE, SIDE_NONE, [1, 1, 0]),
            (ACTION_FILL, SIDE_ASK, [1, 2, 1]),
            (ACTION_FILL, SIDE_BID, [1, 2, 2]),
            (ACTION_FILL, SIDE_NONE, [1, 2, 0]),
            (ACTION_NONE, SIDE_NONE, [2, 0, 0]),
        ];
        let mut tags = AHashSet::new();
        for (action, side, expected) in cases {
            let tag = event_semantic_tag(&semantic_event(action, side));
            assert_eq!(tag, expected);
            assert!(tags.insert(tag));
        }
    }

    #[test]
    fn ready_envelope_cannot_be_minted_across_source_identity_or_ordinal() {
        let first = event(digest(1), 1, 101, 0, 10, 1_000, 2_000, 0, 1);
        let mut open = OpenEnvelopeV1::new(first, false, 8, 8);
        assert_eq!(
            open.append_same_block(event(digest(2), 2, 101, 0, 10, 1_000, 2_000, 0, 2)),
            Err(EnvelopeAssemblyErrorV1::SourceChange)
        );
        assert_eq!(
            open.append_same_block(event(digest(1), 2, 202, 0, 10, 1_000, 2_000, 0, 2)),
            Err(EnvelopeAssemblyErrorV1::IdentityChange)
        );
        assert_eq!(
            open.append_same_block(event(digest(1), 1, 101, 0, 10, 1_000, 2_000, 0, 2)),
            Err(EnvelopeAssemblyErrorV1::NonIncreasingSourceOrdinal)
        );
    }

    #[test]
    fn same_block_checks_sequence_clock_channel_duplicate_and_last_suffix() {
        let first = event(digest(1), 1, 101, 0, 10, 1_000, 2_000, FLAG_LAST, 1);
        let mut open = OpenEnvelopeV1::new(first, false, 8, 8);
        assert_eq!(
            open.append_same_block(event(digest(1), 2, 101, 0, 11, 1_000, 2_000, FLAG_LAST, 2)),
            Err(EnvelopeAssemblyErrorV1::WrongSameBlockSequence)
        );
        assert_eq!(
            open.append_same_block(event(digest(1), 2, 101, 1, 10, 1_000, 2_000, FLAG_LAST, 2)),
            Err(EnvelopeAssemblyErrorV1::ChannelChange)
        );
        assert_eq!(
            open.append_same_block(event(digest(1), 2, 101, 0, 10, 1_001, 2_000, FLAG_LAST, 2)),
            Err(EnvelopeAssemblyErrorV1::BlockTimestampMismatch)
        );
        assert_eq!(
            open.append_same_block(event(digest(1), 2, 101, 0, 10, 1_000, 2_000, FLAG_LAST, 1)),
            Err(EnvelopeAssemblyErrorV1::ExactDuplicate)
        );
        assert_eq!(
            open.append_same_block(event(digest(1), 2, 101, 0, 10, 1_000, 2_000, 0, 2)),
            Err(EnvelopeAssemblyErrorV1::LastToNonLast)
        );
    }

    #[test]
    fn cross_block_and_closure_checks_are_fail_loud() {
        let first = event(digest(1), 1, 101, 0, 10, 1_000, 2_000, 0, 1);
        let mut open = OpenEnvelopeV1::new(first, false, 8, 8);
        assert_eq!(
            open.append_next_block(event(digest(1), 2, 101, 0, 9, 1_001, 2_000, 0, 2)),
            Err(EnvelopeAssemblyErrorV1::SequenceRegressionOrReuse)
        );
        assert_eq!(
            open.append_next_block(event(digest(1), 2, 101, 0, 11, 1_001, 2_001, 0, 2)),
            Err(EnvelopeAssemblyErrorV1::ReceiveTimeChangedBeforeTerminal)
        );
        assert!(matches!(
            open.close(
                event(digest(1), 2, 101, 0, 11, 1_001, 2_001, FLAG_LAST, 2),
                2_001,
            ),
            Err(EnvelopeAssemblyErrorV1::NotTerminal)
        ));

        let terminal = event(digest(1), 1, 101, 0, 10, 1_000, 2_000, FLAG_LAST, 1);
        let open = OpenEnvelopeV1::new(terminal, false, 8, 8);
        assert!(matches!(
            open.close(
                event(digest(1), 2, 101, 0, 11, 1_001, 2_001, FLAG_LAST, 2),
                1_999,
            ),
            Err(EnvelopeAssemblyErrorV1::AvailabilityBeforeEndpoint)
        ));
    }
}
