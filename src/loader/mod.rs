//! Strict, source-bound DBN ingestion.
//!
//! The v1 public surface has no pathname-only file iterator. Opening a stream
//! requires an expected source digest, byte length, metadata contract, record
//! count, and publisher policy. Reusable custody is minted only after verified
//! EOF and source revalidation.

pub mod canonical;
mod catalog;

pub use canonical::{
    CanonicalReadReceiptV1, CanonicalSourceExpectationV1, StrictBoundaryErrorV1, StrictDbnLoaderV1,
    StrictMboEventIteratorV1, VerifiedRejectedStreamEventV1, VerifiedRejectionStageV1,
    VerifiedStreamEventV1, VerifiedStreamRecordV1, XnasDailyMetadataBindingV1,
    XnasDailyMetadataExpectationV1, XnasExpectedInstrumentIdentityV1,
    XnasPolicyBoundInstrumentIdentityV1,
};
pub use catalog::CatalogSelectionErrorV1;

use std::io::{BufRead, Read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Read buffer used by the strict decoder.
pub const IO_BUFFER_SIZE: usize = 1024 * 1024;

/// Counts bytes consumed from the already opened source object.
pub(crate) struct CountingReader<R: Read> {
    inner: R,
    bytes_read: Arc<AtomicU64>,
}

impl<R: Read> CountingReader<R> {
    pub(crate) fn new(inner: R, bytes_read: Arc<AtomicU64>) -> Self {
        Self { inner, bytes_read }
    }
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes_read.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

impl<R: BufRead> BufRead for CountingReader<R> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        self.inner.fill_buf()
    }

    fn consume(&mut self, amount: usize) {
        self.bytes_read.fetch_add(amount as u64, Ordering::Relaxed);
        self.inner.consume(amount);
    }
}
