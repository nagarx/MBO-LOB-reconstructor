use super::envelope::{ReadyEnvelopeCommitmentV2, ReadyEnvelopeTxnV1};
use super::XnasIdentityV1;
use ahash::AHashMap;
use hft_mbo_event_contract::{BookCommandV1, RestingSideV1, Sha256DigestV1};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RestingOrderV1 {
    side: RestingSideV1,
    price_raw: i64,
    size: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct LevelAggregateV1 {
    size: u64,
    order_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum LevelSideV1 {
    Bid,
    Ask,
}

impl From<RestingSideV1> for LevelSideV1 {
    fn from(value: RestingSideV1) -> Self {
        match value {
            RestingSideV1::Bid => Self::Bid,
            RestingSideV1::Ask => Self::Ask,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct XnasBookLevelV1 {
    price_raw: i64,
    aggregate_size: u64,
    order_count: u64,
}

impl XnasBookLevelV1 {
    pub const fn price_raw(&self) -> i64 {
        self.price_raw
    }

    pub const fn aggregate_size(&self) -> u64 {
        self.aggregate_size
    }

    pub const fn order_count(&self) -> u64 {
        self.order_count
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct XnasBookSnapshotV1 {
    bids: Vec<XnasBookLevelV1>,
    asks: Vec<XnasBookLevelV1>,
    live_orders: u64,
}

impl XnasBookSnapshotV1 {
    pub fn bids(&self) -> &[XnasBookLevelV1] {
        &self.bids
    }

    pub fn asks(&self) -> &[XnasBookLevelV1] {
        &self.asks
    }

    pub const fn live_orders(&self) -> u64 {
        self.live_orders
    }

    pub fn best_bid(&self) -> Option<&XnasBookLevelV1> {
        self.bids.first()
    }

    pub fn best_ask(&self) -> Option<&XnasBookLevelV1> {
        self.asks.first()
    }
}

/// Result of one fully prepared and atomically committed XNAS update envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct XnasBookCommitV1 {
    commit_index: u64,
    reset_epoch: u64,
    book_commands_committed: u64,
    exact_endpoint_state_changed: bool,
    transition_chain_sha256: Sha256DigestV1,
    snapshot: XnasBookSnapshotV1,
}

impl XnasBookCommitV1 {
    pub const fn commit_index(&self) -> u64 {
        self.commit_index
    }

    /// One-based reset epoch of the exact private book after this commit. It is
    /// deliberately distinct from replay validity epochs, which count only
    /// intervals that actually qualified for observation.
    pub const fn reset_epoch(&self) -> u64 {
        self.reset_epoch
    }

    pub const fn book_commands_committed(&self) -> u64 {
        self.book_commands_committed
    }

    /// Whether the exact resting-order state changed, including a recovery
    /// reset that advances the reset epoch. This is deliberately independent
    /// of command count and visible snapshot equality: an execution-only or
    /// add-then-cancel envelope is `false`, while a deeper-than-exported-depth
    /// order change is `true`.
    pub const fn exact_endpoint_state_changed(&self) -> bool {
        self.exact_endpoint_state_changed
    }

    pub const fn transition_chain_sha256(&self) -> Sha256DigestV1 {
        self.transition_chain_sha256
    }

    pub const fn snapshot(&self) -> &XnasBookSnapshotV1 {
        &self.snapshot
    }
}

/// Exact private book. It is intentionally not an adapter over the legacy
/// reconstructor: the strict path has different failure and quantity semantics.
pub(crate) struct ExactBookProjectorV1 {
    source_digest: Sha256DigestV1,
    identity: XnasIdentityV1,
    snapshot_depth: usize,
    reset_epoch: u64,
    commit_index: u64,
    transition_chain: Sha256DigestV1,
    orders: AHashMap<u64, RestingOrderV1>,
    bids: BTreeMap<i64, LevelAggregateV1>,
    asks: BTreeMap<i64, LevelAggregateV1>,
}

impl ExactBookProjectorV1 {
    pub(crate) fn new(
        source_digest: Sha256DigestV1,
        identity: XnasIdentityV1,
        snapshot_depth: usize,
    ) -> Result<Self, BookTransactionErrorV1> {
        if snapshot_depth == 0 {
            return Err(BookTransactionErrorV1::ZeroSnapshotDepth);
        }
        let transition_chain = initial_chain(source_digest, identity, snapshot_depth);
        Ok(Self {
            source_digest,
            identity,
            snapshot_depth,
            reset_epoch: 1,
            commit_index: 0,
            transition_chain,
            orders: AHashMap::new(),
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
        })
    }

    /// Transactional for every represented `Result::Err`: validation and all
    /// checked arithmetic complete before live mutation. Allocator failure,
    /// process abort, and panic unwinding are not recoverable transaction modes;
    /// the owning replay exposes no success receipt in those cases.
    pub(crate) fn apply_envelope_precommitted(
        &mut self,
        envelope: &ReadyEnvelopeTxnV1,
        envelope_commitment: ReadyEnvelopeCommitmentV2,
    ) -> Result<XnasBookCommitV1, BookTransactionErrorV1> {
        self.validate_envelope_binding(envelope)?;
        let prepared = PreparedBookTxnV1::prepare(self, envelope)?;
        self.commit(prepared, envelope_commitment)
    }

    #[cfg(test)]
    pub(crate) fn apply_envelope(
        &mut self,
        envelope: &ReadyEnvelopeTxnV1,
    ) -> Result<XnasBookCommitV1, BookTransactionErrorV1> {
        let envelope_commitment = envelope.commitment(self.source_digest);
        self.apply_envelope_precommitted(envelope, envelope_commitment)
    }

    /// Digest of exact resting state scoped to the current reset epoch. Two
    /// visibly identical books reached across different reset histories are
    /// deliberately distinct; batching alone is not.
    pub(crate) fn state_digest(&self) -> Sha256DigestV1 {
        canonical_state_digest(
            self.source_digest,
            self.identity,
            self.reset_epoch,
            &self.orders,
            &self.bids,
            &self.asks,
        )
    }

    pub(crate) const fn commit_index(&self) -> u64 {
        self.commit_index
    }

    pub(crate) const fn transition_chain_sha256(&self) -> Sha256DigestV1 {
        self.transition_chain
    }

    /// Independently rebuild every price-level aggregate from the authoritative
    /// order map. This intentionally does not share the transaction-delta path:
    /// it is the slow terminal reconciliation used before an EOF receipt can be
    /// minted.
    pub(crate) fn validate_internal_consistency(&self) -> Result<(), BookTransactionErrorV1> {
        let mut expected_bids = BTreeMap::<i64, LevelAggregateV1>::new();
        let mut expected_asks = BTreeMap::<i64, LevelAggregateV1>::new();
        for order in self.orders.values() {
            if order.size == 0 {
                return Err(BookTransactionErrorV1::ZeroRestingOrder);
            }
            let expected = match order.side {
                RestingSideV1::Bid => &mut expected_bids,
                RestingSideV1::Ask => &mut expected_asks,
            };
            let aggregate = expected.entry(order.price_raw).or_default();
            aggregate.size = aggregate
                .size
                .checked_add(order.size)
                .ok_or(BookTransactionErrorV1::InternalReconciliationOverflow)?;
            aggregate.order_count = aggregate
                .order_count
                .checked_add(1)
                .ok_or(BookTransactionErrorV1::InternalReconciliationOverflow)?;
        }
        if expected_bids != self.bids || expected_asks != self.asks {
            return Err(BookTransactionErrorV1::InternalLevelStateMismatch);
        }
        if self
            .bids
            .last_key_value()
            .zip(self.asks.first_key_value())
            .is_some_and(|((bid, _), (ask, _))| bid >= ask)
        {
            return Err(BookTransactionErrorV1::InternalLockedOrCrossedState);
        }
        Ok(())
    }

    fn validate_envelope_binding(
        &self,
        envelope: &ReadyEnvelopeTxnV1,
    ) -> Result<(), BookTransactionErrorV1> {
        if envelope.identity() != self.identity {
            return Err(BookTransactionErrorV1::IdentityMismatch);
        }
        let mut previous_ordinal = None;
        for event in envelope.events() {
            let raw = event.event().raw();
            if raw.source_object_sha256 != self.source_digest {
                return Err(BookTransactionErrorV1::SourceMismatch);
            }
            if XnasIdentityV1::new(raw.publisher_id, raw.instrument_id) != self.identity {
                return Err(BookTransactionErrorV1::IdentityMismatch);
            }
            if previous_ordinal.is_some_and(|previous| raw.raw_ordinal <= previous) {
                return Err(BookTransactionErrorV1::MemberOrdinalNotIncreasing);
            }
            previous_ordinal = Some(raw.raw_ordinal);
        }
        Ok(())
    }

    fn commit(
        &mut self,
        prepared: PreparedBookTxnV1,
        envelope_commitment: ReadyEnvelopeCommitmentV2,
    ) -> Result<XnasBookCommitV1, BookTransactionErrorV1> {
        let next_commit_index = self
            .commit_index
            .checked_add(1)
            .ok_or(BookTransactionErrorV1::CommitIndexOverflow)?;
        let next_reset_epoch = self
            .reset_epoch
            .checked_add(u64::from(prepared.cleared))
            .ok_or(BookTransactionErrorV1::ResetEpochOverflow)?;
        let command_count = u64::try_from(prepared.command_count)
            .map_err(|_| BookTransactionErrorV1::CountOverflow)?;
        let exact_endpoint_state_changed = prepared.exact_endpoint_state_changed;
        let transaction_digest = prepared.digest();

        if prepared.cleared {
            self.orders.clear();
            self.bids.clear();
            self.asks.clear();
        }

        for (order_id, final_order) in prepared.order_changes {
            match final_order {
                Some(order) => {
                    self.orders.insert(order_id, order);
                }
                None => {
                    self.orders.remove(&order_id);
                }
            }
        }
        for ((side, price), aggregate) in prepared.final_levels {
            let levels = levels_mut(&mut self.bids, &mut self.asks, side);
            if aggregate.order_count == 0 {
                levels.remove(&price);
            } else {
                levels.insert(price, aggregate);
            }
        }

        self.reset_epoch = next_reset_epoch;
        self.commit_index = next_commit_index;
        self.transition_chain = next_transition_chain(
            self.transition_chain,
            envelope_commitment.sha256(),
            transaction_digest,
            next_commit_index,
            command_count,
        );
        Ok(XnasBookCommitV1 {
            commit_index: next_commit_index,
            reset_epoch: next_reset_epoch,
            book_commands_committed: command_count,
            exact_endpoint_state_changed,
            transition_chain_sha256: self.transition_chain,
            snapshot: self.snapshot(),
        })
    }

    fn snapshot(&self) -> XnasBookSnapshotV1 {
        let bids = self
            .bids
            .iter()
            .rev()
            .take(self.snapshot_depth)
            .map(|(&price_raw, aggregate)| XnasBookLevelV1 {
                price_raw,
                aggregate_size: aggregate.size,
                order_count: aggregate.order_count,
            })
            .collect();
        let asks = self
            .asks
            .iter()
            .take(self.snapshot_depth)
            .map(|(&price_raw, aggregate)| XnasBookLevelV1 {
                price_raw,
                aggregate_size: aggregate.size,
                order_count: aggregate.order_count,
            })
            .collect();
        let live_orders =
            u64::try_from(self.orders.len()).expect("usize always fits u64 on supported targets");
        XnasBookSnapshotV1 {
            bids,
            asks,
            live_orders,
        }
    }
}

struct PreparedBookTxnV1 {
    cleared: bool,
    command_count: usize,
    exact_endpoint_state_changed: bool,
    order_changes: BTreeMap<u64, Option<RestingOrderV1>>,
    final_levels: BTreeMap<(LevelSideV1, i64), LevelAggregateV1>,
}

impl PreparedBookTxnV1 {
    fn prepare(
        base: &ExactBookProjectorV1,
        envelope: &ReadyEnvelopeTxnV1,
    ) -> Result<Self, BookTransactionErrorV1> {
        let commands = envelope.book_commands().collect::<Vec<_>>();
        validate_clear_policy(envelope, &commands)?;

        let mut prepared = Self {
            cleared: false,
            command_count: commands.len(),
            exact_endpoint_state_changed: false,
            order_changes: BTreeMap::new(),
            final_levels: BTreeMap::new(),
        };
        for command in commands {
            prepared.apply_command(base, command)?;
        }
        prepared.derive_final_levels(base)?;
        prepared.validate_endpoint(base)?;
        Ok(prepared)
    }

    fn lookup_order(&self, base: &ExactBookProjectorV1, order_id: u64) -> Option<RestingOrderV1> {
        match self.order_changes.get(&order_id) {
            Some(value) => *value,
            None if self.cleared => None,
            None => base.orders.get(&order_id).copied(),
        }
    }

    fn apply_command(
        &mut self,
        base: &ExactBookProjectorV1,
        command: &BookCommandV1,
    ) -> Result<(), BookTransactionErrorV1> {
        let raw_ordinal = command.event().raw().raw_ordinal;
        match command {
            BookCommandV1::Add(command) => {
                if self.lookup_order(base, command.order_id()).is_some() {
                    return Err(BookTransactionErrorV1::DuplicateAdd {
                        order_id: command.order_id(),
                        raw_ordinal,
                    });
                }
                self.order_changes.insert(
                    command.order_id(),
                    Some(RestingOrderV1 {
                        side: command.resting_side(),
                        price_raw: command.price_raw(),
                        size: u64::from(command.size_raw()),
                    }),
                );
            }
            BookCommandV1::Modify(command) => {
                let current = self.lookup_order(base, command.order_id()).ok_or(
                    BookTransactionErrorV1::MissingModify {
                        order_id: command.order_id(),
                        raw_ordinal,
                    },
                )?;
                if current.side != command.resting_side() {
                    return Err(BookTransactionErrorV1::ModifySideMismatch {
                        order_id: command.order_id(),
                        raw_ordinal,
                    });
                }
                self.order_changes.insert(
                    command.order_id(),
                    Some(RestingOrderV1 {
                        side: current.side,
                        price_raw: command.price_raw(),
                        size: u64::from(command.size_raw()),
                    }),
                );
            }
            BookCommandV1::Cancel(command) => {
                let current = self.lookup_order(base, command.order_id()).ok_or(
                    BookTransactionErrorV1::MissingCancel {
                        order_id: command.order_id(),
                        raw_ordinal,
                    },
                )?;
                if current.side != command.resting_side()
                    || current.price_raw != command.price_raw()
                {
                    return Err(BookTransactionErrorV1::CancelIdentityMismatch {
                        order_id: command.order_id(),
                        raw_ordinal,
                    });
                }
                let cancel_size = u64::from(command.size_raw());
                if cancel_size > current.size {
                    return Err(BookTransactionErrorV1::OverCancel {
                        order_id: command.order_id(),
                        raw_ordinal,
                        resting: current.size,
                        cancelled: cancel_size,
                    });
                }
                let remaining = current.size - cancel_size;
                self.order_changes.insert(
                    command.order_id(),
                    (remaining != 0).then_some(RestingOrderV1 {
                        size: remaining,
                        ..current
                    }),
                );
            }
            BookCommandV1::Clear(_) => {
                self.cleared = true;
                self.order_changes.clear();
            }
        }
        Ok(())
    }

    fn derive_final_levels(
        &mut self,
        base: &ExactBookProjectorV1,
    ) -> Result<(), BookTransactionErrorV1> {
        let mut touched = BTreeSet::new();
        if self.cleared {
            // A qualified recovery clear establishes a new exact-state epoch
            // even when both pre- and post-reset visible books are empty.
            self.exact_endpoint_state_changed = true;
            for final_order in self.order_changes.values().flatten() {
                touched.insert((LevelSideV1::from(final_order.side), final_order.price_raw));
            }
        } else {
            for (&order_id, final_order) in &self.order_changes {
                let original = base.orders.get(&order_id).copied();
                self.exact_endpoint_state_changed |= original != *final_order;
                if let Some(original) = original {
                    touched.insert((LevelSideV1::from(original.side), original.price_raw));
                }
                if let Some(order) = final_order {
                    touched.insert((LevelSideV1::from(order.side), order.price_raw));
                }
            }
        }

        for key in touched {
            self.final_levels.insert(
                key,
                if self.cleared {
                    LevelAggregateV1::default()
                } else {
                    base_level(base, key)
                },
            );
        }
        for (&order_id, final_order) in &self.order_changes {
            if !self.cleared {
                if let Some(original) = base.orders.get(&order_id) {
                    let aggregate = self
                        .final_levels
                        .get_mut(&(LevelSideV1::from(original.side), original.price_raw))
                        .expect("original level was inserted into touched set");
                    aggregate.size = aggregate
                        .size
                        .checked_sub(original.size)
                        .ok_or(BookTransactionErrorV1::LevelAggregateUnderflow)?;
                    aggregate.order_count = aggregate
                        .order_count
                        .checked_sub(1)
                        .ok_or(BookTransactionErrorV1::LevelAggregateUnderflow)?;
                }
            }
            if let Some(order) = final_order {
                let aggregate = self
                    .final_levels
                    .get_mut(&(LevelSideV1::from(order.side), order.price_raw))
                    .expect("final level was inserted into touched set");
                aggregate.size = aggregate
                    .size
                    .checked_add(order.size)
                    .ok_or(BookTransactionErrorV1::LevelArithmeticOverflow)?;
                aggregate.order_count = aggregate
                    .order_count
                    .checked_add(1)
                    .ok_or(BookTransactionErrorV1::LevelArithmeticOverflow)?;
            }
        }
        for aggregate in self.final_levels.values() {
            if (aggregate.size == 0) != (aggregate.order_count == 0) {
                return Err(BookTransactionErrorV1::LevelPopulationMismatch);
            }
        }
        Ok(())
    }

    fn validate_endpoint(&self, base: &ExactBookProjectorV1) -> Result<(), BookTransactionErrorV1> {
        let best_bid = best_after(
            &base.bids,
            &self.final_levels,
            LevelSideV1::Bid,
            self.cleared,
        );
        let best_ask = best_after(
            &base.asks,
            &self.final_levels,
            LevelSideV1::Ask,
            self.cleared,
        );
        if best_bid.zip(best_ask).is_some_and(|(bid, ask)| bid >= ask) {
            return Err(BookTransactionErrorV1::LockedOrCrossedEndpoint {
                best_bid: best_bid.expect("zip proved bid"),
                best_ask: best_ask.expect("zip proved ask"),
            });
        }
        Ok(())
    }

    fn digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"hft.xnas_prepared_book_transaction.v1\0");
        hasher.update([u8::from(self.cleared)]);
        hasher.update(
            u64::try_from(self.command_count)
                .expect("usize always fits u64 on supported targets")
                .to_le_bytes(),
        );
        hasher.update(
            u64::try_from(self.order_changes.len())
                .expect("usize always fits u64 on supported targets")
                .to_le_bytes(),
        );
        for (&order_id, final_order) in &self.order_changes {
            hasher.update(order_id.to_le_bytes());
            match final_order {
                Some(order) => {
                    hasher.update([1, resting_side_byte(order.side)]);
                    hasher.update(order.price_raw.to_le_bytes());
                    hasher.update(order.size.to_le_bytes());
                }
                None => hasher.update([0]),
            }
        }
        hasher.update(
            u64::try_from(self.final_levels.len())
                .expect("usize always fits u64 on supported targets")
                .to_le_bytes(),
        );
        for (&(side, price), aggregate) in &self.final_levels {
            hasher.update([level_side_byte(side)]);
            hasher.update(price.to_le_bytes());
            hasher.update(aggregate.size.to_le_bytes());
            hasher.update(aggregate.order_count.to_le_bytes());
        }
        hasher.finalize().into()
    }
}

fn validate_clear_policy(
    envelope: &ReadyEnvelopeTxnV1,
    commands: &[&BookCommandV1],
) -> Result<(), BookTransactionErrorV1> {
    let clear_positions = commands
        .iter()
        .enumerate()
        .filter_map(|(index, command)| matches!(command, BookCommandV1::Clear(_)).then_some(index))
        .collect::<Vec<_>>();
    if envelope.is_recovery() {
        if clear_positions.as_slice() != [0] {
            return Err(BookTransactionErrorV1::InvalidRecoveryClear);
        }
        if !matches!(
            envelope.events().first(),
            Some(hft_mbo_event_contract::EventDispositionV1::Book(
                BookCommandV1::Clear(_)
            ))
        ) {
            return Err(BookTransactionErrorV1::InvalidRecoveryClear);
        }
    } else if !clear_positions.is_empty() {
        return Err(BookTransactionErrorV1::UnexpectedClear);
    }
    Ok(())
}

fn base_level(base: &ExactBookProjectorV1, key: (LevelSideV1, i64)) -> LevelAggregateV1 {
    levels(&base.bids, &base.asks, key.0)
        .get(&key.1)
        .copied()
        .unwrap_or_default()
}

fn best_after(
    base: &BTreeMap<i64, LevelAggregateV1>,
    final_levels: &BTreeMap<(LevelSideV1, i64), LevelAggregateV1>,
    side: LevelSideV1,
    cleared: bool,
) -> Option<i64> {
    let untouched_base = (!cleared)
        .then(|| match side {
            LevelSideV1::Bid => base
                .iter()
                .rev()
                .find(|(price, _)| !final_levels.contains_key(&(side, **price)))
                .map(|(&price, _)| price),
            LevelSideV1::Ask => base
                .iter()
                .find(|(price, _)| !final_levels.contains_key(&(side, **price)))
                .map(|(&price, _)| price),
        })
        .flatten();
    let touched = {
        let candidates = final_levels
            .iter()
            .filter_map(|(&(candidate_side, price), aggregate)| {
                (candidate_side == side && aggregate.order_count != 0).then_some(price)
            });
        match side {
            LevelSideV1::Bid => candidates.max(),
            LevelSideV1::Ask => candidates.min(),
        }
    };
    match side {
        LevelSideV1::Bid => untouched_base.into_iter().chain(touched).max(),
        LevelSideV1::Ask => untouched_base.into_iter().chain(touched).min(),
    }
}

fn levels<'a>(
    bids: &'a BTreeMap<i64, LevelAggregateV1>,
    asks: &'a BTreeMap<i64, LevelAggregateV1>,
    side: LevelSideV1,
) -> &'a BTreeMap<i64, LevelAggregateV1> {
    match side {
        LevelSideV1::Bid => bids,
        LevelSideV1::Ask => asks,
    }
}

fn levels_mut<'a>(
    bids: &'a mut BTreeMap<i64, LevelAggregateV1>,
    asks: &'a mut BTreeMap<i64, LevelAggregateV1>,
    side: LevelSideV1,
) -> &'a mut BTreeMap<i64, LevelAggregateV1> {
    match side {
        LevelSideV1::Bid => bids,
        LevelSideV1::Ask => asks,
    }
}

fn initial_chain(
    source_digest: Sha256DigestV1,
    identity: XnasIdentityV1,
    snapshot_depth: usize,
) -> Sha256DigestV1 {
    let mut hasher = Sha256::new();
    hasher.update(b"hft.xnas_book_transition_chain.seed.v2\0");
    hasher.update(source_digest.as_bytes());
    hasher.update(identity.publisher_id().to_le_bytes());
    hasher.update(identity.instrument_id().to_le_bytes());
    hasher.update(
        u64::try_from(snapshot_depth)
            .expect("usize always fits u64 on supported targets")
            .to_le_bytes(),
    );
    Sha256DigestV1::from_bytes(hasher.finalize().into())
}

fn next_transition_chain(
    prior: Sha256DigestV1,
    envelope_digest: Sha256DigestV1,
    transaction_digest: [u8; 32],
    commit_index: u64,
    command_count: u64,
) -> Sha256DigestV1 {
    let mut hasher = Sha256::new();
    hasher.update(b"hft.xnas_book_transition_chain.v2\0");
    hasher.update(prior.as_bytes());
    hasher.update(envelope_digest.as_bytes());
    hasher.update(transaction_digest);
    hasher.update(commit_index.to_le_bytes());
    hasher.update(command_count.to_le_bytes());
    Sha256DigestV1::from_bytes(hasher.finalize().into())
}

fn canonical_state_digest(
    source_digest: Sha256DigestV1,
    identity: XnasIdentityV1,
    reset_epoch: u64,
    orders: &AHashMap<u64, RestingOrderV1>,
    bids: &BTreeMap<i64, LevelAggregateV1>,
    asks: &BTreeMap<i64, LevelAggregateV1>,
) -> Sha256DigestV1 {
    let mut hasher = Sha256::new();
    hasher.update(b"hft.xnas_canonical_book_state.v1\0");
    hasher.update(source_digest.as_bytes());
    hasher.update(identity.publisher_id().to_le_bytes());
    hasher.update(identity.instrument_id().to_le_bytes());
    hasher.update(reset_epoch.to_le_bytes());
    let mut sorted_orders = orders.iter().collect::<Vec<_>>();
    sorted_orders.sort_unstable_by_key(|(order_id, _)| **order_id);
    hasher.update((sorted_orders.len() as u64).to_le_bytes());
    for (&order_id, order) in sorted_orders {
        hasher.update(order_id.to_le_bytes());
        hasher.update([resting_side_byte(order.side)]);
        hasher.update(order.price_raw.to_le_bytes());
        hasher.update(order.size.to_le_bytes());
    }
    hash_levels(&mut hasher, b'B', bids);
    hash_levels(&mut hasher, b'A', asks);
    Sha256DigestV1::from_bytes(hasher.finalize().into())
}

const fn resting_side_byte(side: RestingSideV1) -> u8 {
    match side {
        RestingSideV1::Ask => b'A',
        RestingSideV1::Bid => b'B',
    }
}

const fn level_side_byte(side: LevelSideV1) -> u8 {
    match side {
        LevelSideV1::Ask => b'A',
        LevelSideV1::Bid => b'B',
    }
}

fn hash_levels(hasher: &mut Sha256, side: u8, levels: &BTreeMap<i64, LevelAggregateV1>) {
    hasher.update([side]);
    hasher.update((levels.len() as u64).to_le_bytes());
    for (&price, aggregate) in levels {
        hasher.update(price.to_le_bytes());
        hasher.update(aggregate.size.to_le_bytes());
        hasher.update(aggregate.order_count.to_le_bytes());
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
pub enum BookTransactionErrorV1 {
    #[error("snapshot depth must be nonzero")]
    ZeroSnapshotDepth,
    #[error("envelope source digest does not match its book")]
    SourceMismatch,
    #[error("envelope identity does not match its book")]
    IdentityMismatch,
    #[error("envelope member ordinals are not strictly increasing")]
    MemberOrdinalNotIncreasing,
    #[error("duplicate add for resting order {order_id} at raw ordinal {raw_ordinal}")]
    DuplicateAdd { order_id: u64, raw_ordinal: u64 },
    #[error("modify references absent resting order {order_id} at raw ordinal {raw_ordinal}")]
    MissingModify { order_id: u64, raw_ordinal: u64 },
    #[error("modify changes the resting side of order {order_id} at raw ordinal {raw_ordinal}")]
    ModifySideMismatch { order_id: u64, raw_ordinal: u64 },
    #[error("cancel references absent resting order {order_id} at raw ordinal {raw_ordinal}")]
    MissingCancel { order_id: u64, raw_ordinal: u64 },
    #[error(
        "cancel side or price does not match resting order {order_id} at raw ordinal {raw_ordinal}"
    )]
    CancelIdentityMismatch { order_id: u64, raw_ordinal: u64 },
    #[error("cancel exceeds resting quantity for order {order_id} at raw ordinal {raw_ordinal}: resting={resting}, cancelled={cancelled}")]
    OverCancel {
        order_id: u64,
        raw_ordinal: u64,
        resting: u64,
        cancelled: u64,
    },
    #[error("ordinary envelope contains a clear command")]
    UnexpectedClear,
    #[error("recovery envelope must begin with exactly one clear command")]
    InvalidRecoveryClear,
    #[error("level aggregate arithmetic overflow")]
    LevelArithmeticOverflow,
    #[error("level aggregate arithmetic underflow")]
    LevelAggregateUnderflow,
    #[error("level size and order-count populations disagree")]
    LevelPopulationMismatch,
    #[error("a zero-sized resting order survived into committed state")]
    ZeroRestingOrder,
    #[error("terminal whole-book reconciliation overflowed")]
    InternalReconciliationOverflow,
    #[error("terminal order population does not exactly reconcile to stored price levels")]
    InternalLevelStateMismatch,
    #[error("terminal committed book is locked or crossed")]
    InternalLockedOrCrossedState,
    #[error("locked or crossed endpoint: best_bid={best_bid}, best_ask={best_ask}")]
    LockedOrCrossedEndpoint { best_bid: i64, best_ask: i64 },
    #[error("commit index overflow")]
    CommitIndexOverflow,
    #[error("reset epoch overflow")]
    ResetEpochOverflow,
    #[error("population counter cannot be represented as u64")]
    CountOverflow,
}

impl BookTransactionErrorV1 {
    /// Exact source ordinal of the book command that established a
    /// data-derived transaction failure. Endpoint failures are properties of
    /// the complete candidate and therefore intentionally have no single
    /// offending member.
    pub const fn offending_raw_ordinal(&self) -> Option<u64> {
        match self {
            Self::DuplicateAdd { raw_ordinal, .. }
            | Self::MissingModify { raw_ordinal, .. }
            | Self::ModifySideMismatch { raw_ordinal, .. }
            | Self::MissingCancel { raw_ordinal, .. }
            | Self::CancelIdentityMismatch { raw_ordinal, .. }
            | Self::OverCancel { raw_ordinal, .. } => Some(*raw_ordinal),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hft_mbo_event_contract::{
        classify_full_order_book, validate_raw_event, BoundPublisherPolicyV1, EventDispositionV1,
        LogicalSourceV1, OpenedReplicaV1, OpenedRepresentationV1, PublisherPolicyIdV1,
        RawMboEventV1, SourceDescriptorV1, ACTION_ADD, ACTION_CANCEL, ACTION_CLEAR, ACTION_FILL,
        ACTION_MODIFY, ACTION_TRADE, EXPECTED_MBO_RECORD_SIZE_BYTES, EXPECTED_MBO_RTYPE, FLAG_LAST,
        SIDE_ASK, SIDE_BID, SIDE_NONE, UNDEF_PRICE,
    };

    const PUBLISHER: u16 = 2;
    const INSTRUMENT: u32 = 11;
    const BID: i64 = 100_000_000_000;
    const ASK: i64 = 101_000_000_000;

    fn digest() -> Sha256DigestV1 {
        Sha256DigestV1::from_bytes([7; 32])
    }

    fn policy() -> BoundPublisherPolicyV1 {
        let source = SourceDescriptorV1 {
            logical: LogicalSourceV1 {
                catalog_release_id: "test-release".to_owned(),
                catalog_object_id: "test-object".to_owned(),
                canonical_path: "/test/source.dbn.zst".to_owned(),
                canonical_sha256: digest(),
                canonical_bytes: 1,
                dbn_version: 1,
                dbn_ts_out: false,
                dataset: "XNAS.ITCH".to_owned(),
                schema: "mbo".to_owned(),
            },
            opened: OpenedReplicaV1 {
                configured_path: "/test/source.dbn.zst".to_owned(),
                opened_path: "/test/source.dbn.zst".to_owned(),
                representation: OpenedRepresentationV1::CanonicalObject,
                opened_sha256: digest(),
                opened_bytes: 1,
            },
        };
        BoundPublisherPolicyV1::bind(PublisherPolicyIdV1::XnasItchHistorical, &source).unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    fn event(
        action: u8,
        side: u8,
        order_id: u64,
        price_raw: i64,
        size_raw: u32,
        raw_ordinal: u64,
        sequence: u32,
    ) -> EventDispositionV1 {
        classify_full_order_book(
            validate_raw_event(RawMboEventV1 {
                source_object_sha256: digest(),
                raw_ordinal,
                subordinal: 0,
                rtype: EXPECTED_MBO_RTYPE,
                record_size_bytes: EXPECTED_MBO_RECORD_SIZE_BYTES,
                publisher_id: PUBLISHER,
                instrument_id: INSTRUMENT,
                ts_event: 1_000 + raw_ordinal,
                ts_recv: 2_000,
                ts_in_delta: 0,
                channel_id: 0,
                sequence,
                order_id,
                price_raw,
                size_raw,
                flags_raw: FLAG_LAST,
                action_raw: action,
                side_raw: side,
            })
            .unwrap(),
            &policy(),
        )
        .unwrap()
    }

    fn txn(events: Vec<EventDispositionV1>, recovery: bool) -> ReadyEnvelopeTxnV1 {
        let last = events.last().expect("test envelope is nonempty");
        let last_raw = last.event().raw();
        let witness = event(
            ACTION_TRADE,
            SIDE_BID,
            0,
            ASK,
            1,
            last_raw.raw_ordinal + 1,
            last_raw.sequence + 1,
        );
        ReadyEnvelopeTxnV1::synthetic_for_book_test(events, witness, recovery)
    }

    fn projector() -> ExactBookProjectorV1 {
        ExactBookProjectorV1::new(digest(), XnasIdentityV1::new(PUBLISHER, INSTRUMENT), 10).unwrap()
    }

    fn add(
        book: &mut ExactBookProjectorV1,
        order_id: u64,
        side: u8,
        price: i64,
        size: u32,
        ordinal: u64,
    ) -> XnasBookCommitV1 {
        book.apply_envelope(&txn(
            vec![event(
                ACTION_ADD,
                side,
                order_id,
                price,
                size,
                ordinal,
                ordinal as u32,
            )],
            false,
        ))
        .unwrap()
    }

    #[test]
    fn trade_and_fill_are_retained_but_cannot_mutate_the_book() {
        let mut book = projector();
        add(&mut book, 1, SIDE_BID, BID, 100, 1);
        let before = book.state_digest();

        let trade = txn(vec![event(ACTION_TRADE, SIDE_ASK, 0, BID, 40, 3, 3)], false);
        assert_eq!(trade.execution_carriers().count(), 1);
        let trade_commit = book.apply_envelope(&trade).unwrap();
        assert_eq!(trade_commit.book_commands_committed(), 0);
        assert!(!trade_commit.exact_endpoint_state_changed());
        assert_eq!(trade_commit.reset_epoch(), 1);
        assert_eq!(book.state_digest(), before);

        let fill = txn(vec![event(ACTION_FILL, SIDE_BID, 1, BID, 40, 5, 5)], false);
        assert_eq!(fill.execution_carriers().count(), 1);
        let fill_commit = book.apply_envelope(&fill).unwrap();
        assert_eq!(fill_commit.book_commands_committed(), 0);
        assert!(!fill_commit.exact_endpoint_state_changed());
        assert_eq!(fill_commit.reset_epoch(), 1);
        assert_eq!(book.state_digest(), before);
        assert_eq!(
            fill_commit.snapshot().best_bid().unwrap().aggregate_size(),
            100
        );
    }

    #[test]
    fn exact_state_change_is_not_command_count_or_visible_snapshot_change() {
        let mut book = projector();
        let before_flow_chain = book.transition_chain_sha256();
        let add_then_cancel = txn(
            vec![
                event(ACTION_ADD, SIDE_BID, 99, BID - 1, 10, 1, 1),
                event(ACTION_CANCEL, SIDE_BID, 99, BID - 1, 10, 2, 1),
            ],
            false,
        );
        let net_zero = book.apply_envelope(&add_then_cancel).unwrap();
        assert_eq!(net_zero.book_commands_committed(), 2);
        assert!(!net_zero.exact_endpoint_state_changed());
        assert_eq!(net_zero.snapshot().live_orders(), 0);
        assert_ne!(net_zero.transition_chain_sha256(), before_flow_chain);

        for order_id in 1..=11 {
            let commit = add(
                &mut book,
                order_id,
                SIDE_BID,
                BID - i64::try_from(order_id).unwrap() * 1_000_000_000,
                10,
                order_id * 2 + 1,
            );
            assert!(commit.exact_endpoint_state_changed());
        }
        let before_visible = book.snapshot();
        let deeper = book
            .apply_envelope(&txn(
                vec![event(
                    ACTION_MODIFY,
                    SIDE_BID,
                    11,
                    BID - 20_000_000_000,
                    20,
                    31,
                    31,
                )],
                false,
            ))
            .unwrap();
        assert!(deeper.exact_endpoint_state_changed());
        assert_eq!(deeper.snapshot(), &before_visible);
    }

    #[test]
    fn cancel_is_the_only_mutation_in_fill_cancel_envelope() {
        let mut book = projector();
        add(&mut book, 1, SIDE_BID, BID, 100, 1);
        let update = txn(
            vec![
                event(ACTION_FILL, SIDE_BID, 1, BID, 40, 3, 3),
                event(ACTION_CANCEL, SIDE_BID, 1, BID, 40, 4, 3),
            ],
            false,
        );
        assert_eq!(update.execution_carriers().count(), 1);
        let commit = book.apply_envelope(&update).unwrap();
        assert_eq!(commit.book_commands_committed(), 1);
        assert_eq!(commit.snapshot().best_bid().unwrap().aggregate_size(), 60);
    }

    #[test]
    fn strict_order_anomalies_roll_back_every_live_field() {
        let cases = [
            (
                txn(vec![event(ACTION_ADD, SIDE_BID, 1, BID, 1, 3, 3)], false),
                BookTransactionErrorV1::DuplicateAdd {
                    order_id: 1,
                    raw_ordinal: 3,
                },
            ),
            (
                txn(
                    vec![event(ACTION_MODIFY, SIDE_BID, 99, BID, 1, 5, 5)],
                    false,
                ),
                BookTransactionErrorV1::MissingModify {
                    order_id: 99,
                    raw_ordinal: 5,
                },
            ),
            (
                txn(
                    vec![event(ACTION_CANCEL, SIDE_BID, 99, BID, 1, 7, 7)],
                    false,
                ),
                BookTransactionErrorV1::MissingCancel {
                    order_id: 99,
                    raw_ordinal: 7,
                },
            ),
            (
                txn(
                    vec![event(ACTION_CANCEL, SIDE_BID, 1, BID, 101, 9, 9)],
                    false,
                ),
                BookTransactionErrorV1::OverCancel {
                    order_id: 1,
                    raw_ordinal: 9,
                    resting: 100,
                    cancelled: 101,
                },
            ),
            (
                txn(
                    vec![event(ACTION_CANCEL, SIDE_BID, 1, BID + 1, 1, 11, 11)],
                    false,
                ),
                BookTransactionErrorV1::CancelIdentityMismatch {
                    order_id: 1,
                    raw_ordinal: 11,
                },
            ),
            (
                txn(
                    vec![event(ACTION_MODIFY, SIDE_ASK, 1, ASK, 1, 13, 13)],
                    false,
                ),
                BookTransactionErrorV1::ModifySideMismatch {
                    order_id: 1,
                    raw_ordinal: 13,
                },
            ),
        ];

        for (candidate, expected) in cases {
            let mut book = projector();
            add(&mut book, 1, SIDE_BID, BID, 100, 1);
            let before_state = book.state_digest();
            let before_chain = book.transition_chain;
            let before_index = book.commit_index;
            assert_eq!(book.apply_envelope(&candidate), Err(expected));
            assert_eq!(book.state_digest(), before_state);
            assert_eq!(book.transition_chain, before_chain);
            assert_eq!(book.commit_index, before_index);
        }
    }

    #[test]
    fn valid_prefix_and_clear_do_not_survive_later_failure() {
        let mut book = projector();
        add(&mut book, 1, SIDE_BID, BID, 100, 1);
        let before = book.state_digest();
        let before_chain = book.transition_chain;
        let before_index = book.commit_index;
        let prefix_then_failure = txn(
            vec![
                event(ACTION_ADD, SIDE_ASK, 2, ASK, 10, 3, 3),
                event(ACTION_CANCEL, SIDE_ASK, 404, ASK, 1, 4, 3),
            ],
            false,
        );
        assert!(matches!(
            book.apply_envelope(&prefix_then_failure),
            Err(BookTransactionErrorV1::MissingCancel {
                order_id: 404,
                raw_ordinal: 4
            })
        ));
        assert_eq!(book.state_digest(), before);
        assert_eq!(book.transition_chain, before_chain);
        assert_eq!(book.commit_index, before_index);
        book.validate_internal_consistency().unwrap();

        let clear_then_failure = txn(
            vec![
                event(ACTION_CLEAR, SIDE_NONE, 0, UNDEF_PRICE, 0, 6, 6),
                event(ACTION_CANCEL, SIDE_BID, 404, BID, 1, 7, 6),
            ],
            true,
        );
        assert!(matches!(
            book.apply_envelope(&clear_then_failure),
            Err(BookTransactionErrorV1::MissingCancel {
                order_id: 404,
                raw_ordinal: 7
            })
        ));
        assert_eq!(book.state_digest(), before);
        assert_eq!(book.transition_chain, before_chain);
        assert_eq!(book.commit_index, before_index);
        book.validate_internal_consistency().unwrap();
    }

    #[test]
    fn endpoint_validation_is_atomic_not_per_command() {
        let mut book = projector();
        add(&mut book, 1, SIDE_BID, BID, 10, 1);
        add(&mut book, 2, SIDE_ASK, ASK, 10, 3);

        let transient_cross_resolved = txn(
            vec![
                event(ACTION_MODIFY, SIDE_BID, 1, ASK + 1, 10, 5, 5),
                event(ACTION_MODIFY, SIDE_ASK, 2, ASK + 2, 10, 6, 5),
            ],
            false,
        );
        let commit = book.apply_envelope(&transient_cross_resolved).unwrap();
        assert_eq!(commit.snapshot().best_bid().unwrap().price_raw(), ASK + 1);
        assert_eq!(commit.snapshot().best_ask().unwrap().price_raw(), ASK + 2);

        let before = book.state_digest();
        let locked = txn(
            vec![event(ACTION_MODIFY, SIDE_ASK, 2, ASK + 1, 10, 8, 8)],
            false,
        );
        assert!(matches!(
            book.apply_envelope(&locked),
            Err(BookTransactionErrorV1::LockedOrCrossedEndpoint { .. })
        ));
        assert_eq!(book.state_digest(), before);
    }

    #[test]
    fn level_quantity_is_exact_above_u32_max() {
        let mut book = projector();
        let first = u32::MAX - 1;
        add(&mut book, 1, SIDE_BID, BID, first, 1);
        let commit = add(&mut book, 2, SIDE_BID, BID, 2, 3);
        assert_eq!(
            commit.snapshot().best_bid().unwrap().aggregate_size(),
            u64::from(u32::MAX) + 1
        );
        assert_eq!(commit.snapshot().best_bid().unwrap().order_count(), 2);
        book.validate_internal_consistency().unwrap();
    }

    #[test]
    fn terminal_reconciliation_independently_detects_untouched_level_corruption() {
        let mut book = projector();
        add(&mut book, 1, SIDE_BID, BID, 10, 1);
        add(&mut book, 2, SIDE_ASK, ASK, 20, 3);
        book.validate_internal_consistency().unwrap();

        book.bids.get_mut(&BID).unwrap().size += 1;
        let execution_only = txn(vec![event(ACTION_TRADE, SIDE_BID, 0, ASK, 1, 5, 5)], false);
        book.apply_envelope(&execution_only).unwrap();
        assert_eq!(
            book.validate_internal_consistency(),
            Err(BookTransactionErrorV1::InternalLevelStateMismatch)
        );
    }

    #[test]
    fn deleting_the_best_level_reveals_the_next_untouched_level() {
        let mut book = projector();
        add(&mut book, 1, SIDE_BID, BID, 10, 1);
        add(&mut book, 2, SIDE_BID, BID - 1_000_000_000, 20, 3);
        add(&mut book, 3, SIDE_ASK, ASK, 30, 5);
        let commit = book
            .apply_envelope(&txn(
                vec![event(ACTION_CANCEL, SIDE_BID, 1, BID, 10, 7, 7)],
                false,
            ))
            .unwrap();
        assert_eq!(
            commit.snapshot().best_bid().unwrap().price_raw(),
            BID - 1_000_000_000
        );
        book.validate_internal_consistency().unwrap();
    }

    #[test]
    fn recovery_requires_one_leading_clear_and_replaces_prior_state() {
        let mut book = projector();
        add(&mut book, 1, SIDE_BID, BID, 100, 1);
        let ordinary_clear = txn(
            vec![event(ACTION_CLEAR, SIDE_NONE, 0, UNDEF_PRICE, 0, 3, 3)],
            false,
        );
        assert_eq!(
            book.apply_envelope(&ordinary_clear),
            Err(BookTransactionErrorV1::UnexpectedClear)
        );

        let recovery = txn(
            vec![
                event(ACTION_CLEAR, SIDE_NONE, 0, UNDEF_PRICE, 0, 5, 5),
                event(ACTION_ADD, SIDE_ASK, 2, ASK, 25, 6, 5),
            ],
            true,
        );
        let commit = book.apply_envelope(&recovery).unwrap();
        assert!(commit.exact_endpoint_state_changed());
        assert_eq!(commit.reset_epoch(), 2);
        assert!(commit.snapshot().best_bid().is_none());
        assert_eq!(commit.snapshot().best_ask().unwrap().aggregate_size(), 25);
        assert_eq!(commit.snapshot().live_orders(), 1);

        let mut empty = projector();
        let before_empty = empty.snapshot();
        let empty_reset = empty
            .apply_envelope(&txn(
                vec![event(ACTION_CLEAR, SIDE_NONE, 0, UNDEF_PRICE, 0, 9, 9)],
                true,
            ))
            .unwrap();
        assert_eq!(empty_reset.snapshot(), &before_empty);
        assert!(empty_reset.exact_endpoint_state_changed());
        assert_eq!(empty_reset.reset_epoch(), 2);
    }

    #[test]
    fn canonical_state_digest_is_batching_independent_within_one_reset_epoch() {
        let mut one_envelope = projector();
        let combined = txn(
            vec![
                event(ACTION_ADD, SIDE_BID, 1, BID, 10, 1, 1),
                event(ACTION_ADD, SIDE_ASK, 2, ASK, 20, 2, 1),
            ],
            false,
        );
        one_envelope.apply_envelope(&combined).unwrap();

        let mut two_envelopes = projector();
        add(&mut two_envelopes, 1, SIDE_BID, BID, 10, 1);
        add(&mut two_envelopes, 2, SIDE_ASK, ASK, 20, 3);

        assert_eq!(one_envelope.state_digest(), two_envelopes.state_digest());
        assert_ne!(
            one_envelope.transition_chain,
            two_envelopes.transition_chain
        );
    }

    #[test]
    fn digest_encodings_have_fixed_golden_vectors() {
        let mut book = projector();
        book.apply_envelope(&txn(
            vec![
                event(ACTION_ADD, SIDE_BID, 1, BID, 10, 1, 1),
                event(ACTION_ADD, SIDE_ASK, 2, ASK, 20, 2, 1),
            ],
            false,
        ))
        .unwrap();
        assert_eq!(
            book.state_digest().to_hex(),
            "9c18def2c8ca902fa1a72d2fa4018eee59457489a00505fe14b40603aeb8bc56"
        );
        assert_eq!(
            book.transition_chain_sha256().to_hex(),
            "188ce03ecf4c8507564c5973d369921a0644feb6cf14e641a5ba712e2f5282a2"
        );
    }

    #[test]
    fn state_digest_binds_values_and_reset_epoch_not_only_visible_levels() {
        let mut size_25 = projector();
        add(&mut size_25, 2, SIDE_ASK, ASK, 25, 1);
        let mut size_26 = projector();
        add(&mut size_26, 2, SIDE_ASK, ASK, 26, 1);
        assert_ne!(size_25.state_digest(), size_26.state_digest());

        let mut recovered_same_visible_state = projector();
        recovered_same_visible_state
            .apply_envelope(&txn(
                vec![
                    event(ACTION_CLEAR, SIDE_NONE, 0, UNDEF_PRICE, 0, 1, 1),
                    event(ACTION_ADD, SIDE_ASK, 2, ASK, 25, 2, 1),
                ],
                true,
            ))
            .unwrap();
        assert_eq!(size_25.orders, recovered_same_visible_state.orders);
        assert_eq!(size_25.bids, recovered_same_visible_state.bids);
        assert_eq!(size_25.asks, recovered_same_visible_state.asks);
        assert_ne!(
            size_25.state_digest(),
            recovered_same_visible_state.state_digest()
        );
    }
}
