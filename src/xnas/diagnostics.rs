use serde::Serialize;

/// Stable, unit-specific populations for the historical XNAS replay.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct XnasReplayCountsV1 {
    pub raw_records_ingested: u64,
    pub decoded_semantic_rejections: u64,
    pub initial_clear_controls: u64,
    /// Fresh-book acknowledgements plus successfully committed recovery clears.
    pub private_book_resets: u64,
    pub completed_envelope_members: u64,
    pub pending_members: u64,
    pub quarantined_records: u64,
    pub completed_update_envelopes: u64,
    pub venue_sequence_blocks: u64,
    pub execution_sequence_blocks: u64,
    pub execution_envelopes: u64,
    pub execution_carriers: u64,
    pub book_commands_committed: u64,
    pub staged_book_updates: u64,
    pub reset_recovery_candidates: u64,
    pub reset_boundary_quarantined_records: u64,
    pub reset_boundary_quarantine_incidents: u64,
    pub semantic_quarantined_records: u64,
    pub semantic_candidate_quarantined_records: u64,
    pub semantic_while_invalid_quarantined_records: u64,
    pub semantic_quarantine_incidents: u64,
    pub eof_tail_quarantined_records: u64,
    pub tail_quarantine_incidents: u64,
}

impl XnasReplayCountsV1 {
    pub fn population_reconciles(&self) -> bool {
        self.initial_clear_controls
            .checked_add(self.completed_envelope_members)
            .and_then(|value| value.checked_add(self.pending_members))
            .and_then(|value| value.checked_add(self.quarantined_records))
            == Some(self.raw_records_ingested)
    }

    pub fn quarantine_reasons_reconcile(&self) -> bool {
        self.reset_boundary_quarantined_records
            .checked_add(self.semantic_quarantined_records)
            .and_then(|value| value.checked_add(self.eof_tail_quarantined_records))
            == Some(self.quarantined_records)
    }

    pub(crate) fn admit_pending(&mut self) -> Result<(), &'static str> {
        self.raw_records_ingested = self
            .raw_records_ingested
            .checked_add(1)
            .ok_or("raw record count overflow")?;
        self.pending_members = self
            .pending_members
            .checked_add(1)
            .ok_or("pending record count overflow")?;
        Ok(())
    }

    pub(crate) fn mark_decoded_semantic_rejection(&mut self) -> Result<(), &'static str> {
        self.decoded_semantic_rejections = self
            .decoded_semantic_rejections
            .checked_add(1)
            .ok_or("decoded semantic rejection count overflow")?;
        Ok(())
    }

    pub(crate) fn consume_initial_control(&mut self) -> Result<(), &'static str> {
        self.pending_members = self
            .pending_members
            .checked_sub(1)
            .ok_or("initial control was not pending")?;
        self.initial_clear_controls = self
            .initial_clear_controls
            .checked_add(1)
            .ok_or("initial control count overflow")?;
        self.private_book_resets = self
            .private_book_resets
            .checked_add(1)
            .ok_or("private-book reset count overflow")?;
        Ok(())
    }

    pub(crate) fn commit_recovery_reset(&mut self) -> Result<(), &'static str> {
        self.private_book_resets = self
            .private_book_resets
            .checked_add(1)
            .ok_or("private-book reset count overflow")?;
        Ok(())
    }

    pub(crate) fn complete_members(&mut self, members: u64) -> Result<(), &'static str> {
        self.pending_members = self
            .pending_members
            .checked_sub(members)
            .ok_or("completed member population exceeds pending population")?;
        self.completed_envelope_members = self
            .completed_envelope_members
            .checked_add(members)
            .ok_or("completed member count overflow")?;
        Ok(())
    }

    pub(crate) fn quarantine_pending(&mut self, records: u64) -> Result<(), &'static str> {
        self.pending_members = self
            .pending_members
            .checked_sub(records)
            .ok_or("quarantine population exceeds pending population")?;
        self.quarantined_records = self
            .quarantined_records
            .checked_add(records)
            .ok_or("quarantined record count overflow")?;
        Ok(())
    }

    pub(crate) fn quarantine_reset_boundary(&mut self, records: u64) -> Result<(), &'static str> {
        self.quarantine_pending(records)?;
        self.reset_boundary_quarantined_records = self
            .reset_boundary_quarantined_records
            .checked_add(records)
            .ok_or("reset-boundary quarantined-record count overflow")?;
        self.reset_boundary_quarantine_incidents = self
            .reset_boundary_quarantine_incidents
            .checked_add(1)
            .ok_or("reset-boundary quarantine incident count overflow")?;
        Ok(())
    }

    pub(crate) fn quarantine_semantic_candidate(
        &mut self,
        records: u64,
    ) -> Result<(), &'static str> {
        self.quarantine_pending(records)?;
        self.semantic_quarantined_records = self
            .semantic_quarantined_records
            .checked_add(records)
            .ok_or("semantic quarantined-record count overflow")?;
        self.semantic_candidate_quarantined_records = self
            .semantic_candidate_quarantined_records
            .checked_add(records)
            .ok_or("semantic candidate quarantined-record count overflow")?;
        self.semantic_quarantine_incidents = self
            .semantic_quarantine_incidents
            .checked_add(1)
            .ok_or("semantic quarantine incident count overflow")?;
        Ok(())
    }

    pub(crate) fn quarantine_while_invalid(&mut self) -> Result<(), &'static str> {
        self.quarantine_pending(1)?;
        self.semantic_quarantined_records = self
            .semantic_quarantined_records
            .checked_add(1)
            .ok_or("semantic quarantined-record count overflow")?;
        self.semantic_while_invalid_quarantined_records = self
            .semantic_while_invalid_quarantined_records
            .checked_add(1)
            .ok_or("invalid-state quarantined-record count overflow")?;
        Ok(())
    }

    pub fn semantic_population_reconciles(&self) -> bool {
        self.semantic_candidate_quarantined_records
            .checked_add(self.semantic_while_invalid_quarantined_records)
            == Some(self.semantic_quarantined_records)
    }

    pub(crate) fn quarantine_eof_tail(&mut self, records: u64) -> Result<(), &'static str> {
        self.quarantine_pending(records)?;
        self.eof_tail_quarantined_records = self
            .eof_tail_quarantined_records
            .checked_add(records)
            .ok_or("EOF-tail quarantined-record count overflow")?;
        self.tail_quarantine_incidents = self
            .tail_quarantine_incidents
            .checked_add(1)
            .ok_or("tail quarantine incident count overflow")?;
        Ok(())
    }
}
