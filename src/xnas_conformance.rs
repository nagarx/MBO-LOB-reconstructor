//! Exact authority and source-image binding for the bounded DECISION-031 lane.
//!
//! This module deliberately separates two objects that are easy to conflate:
//! the immutable Git authority that names expected evidence, and the exact
//! local byte images actually consumed by conformance.  A source or manifest
//! path is opened once, read to completion, hashed in memory, and never
//! reopened by the authoritative path.  Both primary and reference decoders
//! must consume clones of the resulting [`Arc`] rather than the path.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Cursor, Read, Write};
use std::mem::size_of;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use dbn::decode::{DbnMetadata, DecodeRecordRef, DynDecoder};
use dbn::{MboMsg, Mbp10Msg, Record, SType, Schema, VersionUpgradePolicy};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::xnas_semantics::{
    xnas_mbo_watermark_contribution, xnas_mbp10_watermark_contribution, CausalBinnedMidpointV1,
    MboCausalInvalidationScopeV1, MboIngestDispositionV1, MboIngestOutcomeV1,
    MboSemanticsFinishReportV1, Mbp10CompletedEndpointV1, Mbp10SemanticsFinishReportV1,
    PublishedMboBookV1, RawMboRecordV1, RawMbp10RecordV1, SourceOrdinal, XnasBoundaryV1,
    XnasDailySourceQualificationV1, XnasIdentityV1, XnasMboStreamV1, XnasMbp10StreamV1,
    XnasSchemaV1, XnasSemanticsError, XNAS_ITCH_PUBLISHER_ID,
};

mod runner;

pub use runner::{
    run_and_persist_xnas_semantics_conformance_v1, XnasConformanceArtifactDispositionV1,
};

pub(crate) const BLOCKER_COMMIT_V1: &str = "f3bc6ff58bbfd2342f36ee31cb67860cb3b52b58";
pub(crate) const BLOCKER_PATH_V1: &str = "results/MBO_SEMANTICS_BLOCKER_V1.json";
pub(crate) const BLOCKER_SHA256_V1: &str =
    "4b56c89b5e2b8796e9badc1a6af01ad78208accbbe02fb74bcee0ebf08b51e51";

const BLOCKER_SCHEMA_VERSION_V1: &str = "1.0";
const BLOCKER_ARTIFACT_ID_V1: &str = "MBO_SEMANTICS_BLOCKER_V1";
const BLOCKER_ARTIFACT_KIND_V1: &str = "terminal_blocker";
const BLOCKER_STATUS_V1: &str = "BLOCKED_SPECIFICATION_CONTRADICTION";

/// Fail-closed errors for authority and immutable-byte qualification.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum XnasConformanceError {
    #[error("Git object resolution failed: {0}")]
    GitObject(String),
    #[error("authority digest mismatch")]
    AuthorityDigestMismatch,
    #[error("authority JSON is malformed: {0}")]
    AuthorityJson(String),
    #[error("authority invariant failed: {0}")]
    AuthorityInvariant(String),
    #[error("unsafe authority path: {0}")]
    UnsafeAuthorityPath(String),
    #[error("source-image I/O failed for {subject}: {kind}")]
    SourceIo { subject: String, kind: String },
    #[error("source-image evidence mismatch for {0}")]
    SourceEvidenceMismatch(String),
    #[error("DBN decode failed: {0}")]
    DbnDecode(String),
    #[error("reference executable qualification failed: {0}")]
    ReferenceExecutable(String),
    #[error("reference NDJSON decode failed: {0}")]
    ReferenceNdjson(String),
    #[error("reference process failed: {0}")]
    ReferenceProcess(String),
    #[error("reference {phase} timed out after {timeout_ms} ms")]
    ReferenceTimeout {
        phase: &'static str,
        timeout_ms: u64,
    },
    #[error("DBN metadata qualification failed: {0}")]
    Metadata(String),
    #[error("XNAS session qualification failed: {0}")]
    Session(String),
    #[error("XNAS session cursor already closed at {rth_close_ns}")]
    SessionClosed { rth_close_ns: u64 },
    #[error("decision {decision_ns} reached source EOF without a lifting lookahead record")]
    DecisionPrefixEndedAtEof { decision_ns: u64 },
    #[error("conformance invariant failed: {0}")]
    ConformanceInvariant(String),
    #[error("conformance artifact I/O failed: {0}")]
    ArtifactIo(String),
    #[error("conformance artifact already exists: {0}")]
    ArtifactExists(String),
    #[error(transparent)]
    Semantics(#[from] XnasSemanticsError),
}

/// The two byte images named by the accepted blocker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockerSourceRoleV1 {
    Mbo,
    Mbp10,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourceQueryAuthorityV1 {
    ExactIntervalNs { start_ns: u64, end_ns: u64 },
    ManifestSessionDate { yyyymmdd: String },
}

/// Exact source fields consumed from the sealed blocker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlockerSourceAuthorityV1 {
    role: BlockerSourceRoleV1,
    workspace_path: String,
    resolved_storage_root: String,
    size_bytes: u64,
    sha256: String,
    manifest_path: String,
    manifest_sha256: String,
    dataset: String,
    schema: XnasSchemaV1,
    dbn_version: u8,
    query: SourceQueryAuthorityV1,
    raw_symbol: String,
    publisher_id: u16,
    instrument_id: Option<u32>,
}

impl BlockerSourceAuthorityV1 {
    pub(crate) const fn role(&self) -> BlockerSourceRoleV1 {
        self.role
    }

    pub(crate) fn source_path(&self) -> Result<PathBuf, XnasConformanceError> {
        join_authority_path(&self.resolved_storage_root, &self.workspace_path)
    }

    pub(crate) fn manifest_path(&self) -> Result<PathBuf, XnasConformanceError> {
        join_authority_path(&self.resolved_storage_root, &self.manifest_path)
    }

    pub(crate) const fn schema(&self) -> XnasSchemaV1 {
        self.schema
    }

    pub(crate) const fn dbn_version(&self) -> u8 {
        self.dbn_version
    }

    pub(crate) fn dataset(&self) -> &str {
        &self.dataset
    }

    pub(crate) fn query(&self) -> &SourceQueryAuthorityV1 {
        &self.query
    }

    pub(crate) fn raw_symbol(&self) -> &str {
        &self.raw_symbol
    }

    pub(crate) const fn expected_identity(&self) -> Option<XnasIdentityV1> {
        match self.instrument_id {
            Some(instrument_id) => Some(XnasIdentityV1::new(self.publisher_id, instrument_id)),
            None => None,
        }
    }
}

/// Reference executable fields consumed from the sealed blocker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReferenceDecoderAuthorityV1 {
    executable: String,
    version: String,
    sha256: String,
    output_encoding: String,
    timestamps_and_prices: String,
    source_ordinal_definition: String,
}

impl ReferenceDecoderAuthorityV1 {
    pub(crate) fn executable(&self) -> &str {
        &self.executable
    }

    pub(crate) fn version(&self) -> &str {
        &self.version
    }

    pub(crate) fn sha256(&self) -> &str {
        &self.sha256
    }
}

/// Exact executable bytes retained once from the blocker-named path.
///
/// Every version/decode spawn receives a fresh private staging directory. The
/// staged image is rehashed before and after execution and retained only until
/// that child is reaped; neither the authority path nor a long-lived staged
/// pathname is executed later.
#[derive(Debug)]
pub(crate) struct QualifiedReferenceExecutableV1 {
    authority: ReferenceDecoderAuthorityV1,
    executable_bytes: Arc<[u8]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReferenceDecodeRunV1 {
    pub(crate) executable_sha256: String,
    pub(crate) executable_version: String,
    pub(crate) source_sha256: String,
    pub(crate) source_schema: XnasSchemaV1,
    pub(crate) decoded_body_record_count: u64,
}

const REFERENCE_VERSION_TIMEOUT_V1: Duration = Duration::from_secs(10);
const REFERENCE_DECODE_TIMEOUT_V1: Duration = Duration::from_secs(30 * 60);
const REFERENCE_PROCESS_POLL_V1: Duration = Duration::from_millis(10);

/// Owns both the child and its one-spawn staging directory. Drop is a
/// fail-safe: unwinding or an early return cannot orphan the reference
/// process, and the staged image remains present for shebang interpreters
/// until the child is reaped.
struct StagedReferenceChildV1 {
    child: Option<std::process::Child>,
    staging_directory: tempfile::TempDir,
    staged_path: PathBuf,
    expected_sha256: String,
}

impl StagedReferenceChildV1 {
    fn child_mut(&mut self) -> &mut std::process::Child {
        self.child.as_mut().expect("staged child has not been reaped")
    }

    fn take_stdin(&mut self) -> Option<std::process::ChildStdin> {
        self.child_mut().stdin.take()
    }

    fn take_stdout(&mut self) -> Option<std::process::ChildStdout> {
        self.child_mut().stdout.take()
    }

    fn take_stderr(&mut self) -> Option<std::process::ChildStderr> {
        self.child_mut().stderr.take()
    }

    fn wait_until(
        &mut self,
        deadline: Instant,
        phase: &'static str,
        timeout: Duration,
    ) -> Result<std::process::ExitStatus, XnasConformanceError> {
        loop {
            match self.child_mut().try_wait() {
                Ok(Some(status)) => {
                    self.child.take();
                    return Ok(status);
                }
                Ok(None) => {}
                Err(error) => {
                    self.kill_and_reap();
                    return Err(XnasConformanceError::ReferenceProcess(format!(
                        "{phase} wait: {}",
                        error.kind()
                    )));
                }
            }
            if Instant::now() >= deadline {
                self.kill_and_reap();
                return Err(reference_timeout(phase, timeout));
            }
            std::thread::sleep(REFERENCE_PROCESS_POLL_V1);
        }
    }

    fn kill_and_reap(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn verify_staged_image(&self) -> Result<(), XnasConformanceError> {
        let bytes = read_one_image(&self.staged_path, "staged reference executable")
            .map_err(|error| XnasConformanceError::ReferenceExecutable(error.to_string()))?;
        if sha256_bytes(&bytes) != self.expected_sha256 {
            return Err(XnasConformanceError::ReferenceExecutable(
                "staged executable changed during execution".to_owned(),
            ));
        }
        Ok(())
    }
}

impl Drop for StagedReferenceChildV1 {
    fn drop(&mut self) {
        self.kill_and_reap();
        // Keep the directory field observably live until after child cleanup.
        let _ = self.staging_directory.path();
    }
}

enum ReferenceStdoutItemV1 {
    Line(String),
    Eof,
    Error(std::io::ErrorKind),
}

fn reference_timeout(phase: &'static str, timeout: Duration) -> XnasConformanceError {
    XnasConformanceError::ReferenceTimeout {
        phase,
        timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
    }
}

impl QualifiedReferenceExecutableV1 {
    pub(crate) fn read_once(
        authority: &ReferenceDecoderAuthorityV1,
    ) -> Result<Self, XnasConformanceError> {
        let authority_path = Path::new(authority.executable());
        if !authority_path.is_absolute() {
            return Err(XnasConformanceError::ReferenceExecutable(
                "authority path is not absolute".to_owned(),
            ));
        }
        let executable_bytes = read_one_image(authority_path, "reference executable")
            .map_err(|error| XnasConformanceError::ReferenceExecutable(error.to_string()))?;
        if sha256_bytes(&executable_bytes) != authority.sha256() {
            return Err(XnasConformanceError::ReferenceExecutable(
                "executable digest mismatch".to_owned(),
            ));
        }

        let qualified = Self {
            authority: authority.clone(),
            executable_bytes,
        };
        let version = qualified.execute_version()?;
        if version != qualified.authority.version() {
            return Err(XnasConformanceError::ReferenceExecutable(format!(
                "version mismatch: expected {}, observed {version}",
                qualified.authority.version()
            )));
        }
        Ok(qualified)
    }

    pub(crate) fn executable_bytes(&self) -> Arc<[u8]> {
        Arc::clone(&self.executable_bytes)
    }

    pub(crate) fn decode_mbo_source<F>(
        &self,
        image: &QualifiedSourceImageV1,
        mut consume: F,
    ) -> Result<ReferenceDecodeRunV1, XnasConformanceError>
    where
        F: FnMut(RawMboRecordV1) -> Result<(), XnasConformanceError>,
    {
        if image.authority().schema() != XnasSchemaV1::Mbo {
            return Err(XnasConformanceError::ReferenceNdjson(
                "typed MBO decode received a non-MBO source".to_owned(),
            ));
        }
        self.decode_source_with_timeout(
            image,
            REFERENCE_DECODE_TIMEOUT_V1,
            move |source_ordinal, line| {
                consume(parse_reference_mbo_line(source_ordinal, line)?)
            },
        )
    }

    pub(crate) fn decode_mbp10_source<F>(
        &self,
        image: &QualifiedSourceImageV1,
        mut consume: F,
    ) -> Result<ReferenceDecodeRunV1, XnasConformanceError>
    where
        F: FnMut(RawMbp10RecordV1) -> Result<(), XnasConformanceError>,
    {
        if image.authority().schema() != XnasSchemaV1::Mbp10 {
            return Err(XnasConformanceError::ReferenceNdjson(
                "typed MBP-10 decode received a non-MBP-10 source".to_owned(),
            ));
        }
        self.decode_source_with_timeout(
            image,
            REFERENCE_DECODE_TIMEOUT_V1,
            move |source_ordinal, line| {
                consume(parse_reference_mbp10_line(source_ordinal, line)?)
            },
        )
    }

    fn decode_source_with_timeout<F>(
        &self,
        image: &QualifiedSourceImageV1,
        timeout: Duration,
        mut consume: F,
    ) -> Result<ReferenceDecodeRunV1, XnasConformanceError>
    where
        F: FnMut(SourceOrdinal, &str) -> Result<(), XnasConformanceError>,
    {
        let source_bytes = image.source_bytes();
        let schema = image.authority().schema();
        let mut staged =
            self.spawn_staged(&["-J", "-"], Stdio::piped(), Stdio::piped(), Stdio::piped())?;
        let mut stdin = staged.take_stdin().ok_or_else(|| {
            XnasConformanceError::ReferenceProcess("stdin pipe is absent".to_owned())
        })?;
        let stdout = staged.take_stdout().ok_or_else(|| {
            XnasConformanceError::ReferenceProcess("stdout pipe is absent".to_owned())
        })?;
        let mut stderr = staged.take_stderr().ok_or_else(|| {
            XnasConformanceError::ReferenceProcess("stderr pipe is absent".to_owned())
        })?;

        let writer_bytes = Arc::clone(&source_bytes);
        let writer = std::thread::spawn(move || -> std::io::Result<()> {
            stdin.write_all(&writer_bytes)?;
            stdin.flush()
        });
        let stderr_reader = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes)?;
            Ok(bytes)
        });
        let (stdout_sender, stdout_receiver) = mpsc::sync_channel(256);
        let stdout_reader = std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => {
                        let _ = stdout_sender.send(ReferenceStdoutItemV1::Eof);
                        break;
                    }
                    Ok(_) => {
                        while line.ends_with('\n') || line.ends_with('\r') {
                            line.pop();
                        }
                        if stdout_sender
                            .send(ReferenceStdoutItemV1::Line(line))
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ =
                            stdout_sender.send(ReferenceStdoutItemV1::Error(error.kind()));
                        break;
                    }
                }
            }
        });

        let mut decoded_body_record_count = 0_u64;
        let mut callback_error = None;
        let mut process_error = None;
        let deadline = Instant::now() + timeout;
        let mut saw_stdout_eof = false;
        while callback_error.is_none() && process_error.is_none() && !saw_stdout_eof {
            let now = Instant::now();
            if now >= deadline {
                process_error = Some(reference_timeout("decode", timeout));
                break;
            }
            let remaining = deadline.saturating_duration_since(now);
            let poll = remaining.min(Duration::from_millis(100));
            match stdout_receiver.recv_timeout(poll) {
                Ok(ReferenceStdoutItemV1::Line(line)) => {
                    if line.is_empty() {
                        callback_error = Some(XnasConformanceError::ReferenceNdjson(
                            "empty body line".to_owned(),
                        ));
                        continue;
                    }
                    decoded_body_record_count =
                        match decoded_body_record_count.checked_add(1) {
                            Some(value) => value,
                            None => {
                                callback_error =
                                    Some(XnasConformanceError::ReferenceNdjson(
                                        "source ordinal overflow".to_owned(),
                                    ));
                                continue;
                            }
                        };
                    let source_ordinal = match SourceOrdinal::new(decoded_body_record_count) {
                        Ok(value) => value,
                        Err(error) => {
                            callback_error = Some(error.into());
                            continue;
                        }
                    };
                    if let Err(error) = consume(source_ordinal, &line) {
                        callback_error = Some(error);
                    }
                }
                Ok(ReferenceStdoutItemV1::Eof) => saw_stdout_eof = true,
                Ok(ReferenceStdoutItemV1::Error(kind)) => {
                    callback_error =
                        Some(XnasConformanceError::ReferenceProcess(kind.to_string()));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    callback_error = Some(XnasConformanceError::ReferenceProcess(
                        "stdout reader disconnected before EOF".to_owned(),
                    ));
                }
            }
        }

        let status = if callback_error.is_some() || process_error.is_some() {
            staged.kill_and_reap();
            None
        } else {
            match staged.wait_until(deadline, "decode", timeout) {
                Ok(status) => Some(status),
                Err(error) => {
                    process_error = Some(error);
                    None
                }
            }
        };
        drop(stdout_receiver);
        let writer_result = writer.join();
        let stdout_result = stdout_reader.join();
        let stderr_result = stderr_reader.join();
        let staged_image_result = staged.verify_staged_image();

        // Executable-image integrity is non-negotiable and must be checked
        // after every child exit, including timeout and callback-abort paths.
        staged_image_result?;
        if let Some(error) = callback_error {
            return Err(error);
        }
        if let Some(error) = process_error {
            return Err(error);
        }
        let status = status.expect("normal decode path reaps the child");
        let stderr_bytes = match stderr_result {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(error)) => {
                return Err(XnasConformanceError::ReferenceProcess(format!(
                    "stderr reader: {}",
                    error.kind()
                )));
            }
            Err(_) => {
                return Err(XnasConformanceError::ReferenceProcess(
                    "stderr reader panicked".to_owned(),
                ));
            }
        };
        if !status.success() {
            return Err(XnasConformanceError::ReferenceProcess(format!(
                "exit {:?}: {}",
                status.code(),
                String::from_utf8_lossy(&stderr_bytes).trim()
            )));
        }
        stdout_result.map_err(|_| {
            XnasConformanceError::ReferenceProcess("stdout reader panicked".to_owned())
        })?;
        if !stderr_bytes.is_empty() {
            return Err(XnasConformanceError::ReferenceProcess(format!(
                "unexpected stderr: {}",
                String::from_utf8_lossy(&stderr_bytes).trim()
            )));
        }
        writer_result
            .map_err(|_| {
                XnasConformanceError::ReferenceProcess("stdin writer panicked".to_owned())
            })?
            .map_err(|error| {
                XnasConformanceError::ReferenceProcess(format!("stdin: {}", error.kind()))
            })?;
        Ok(ReferenceDecodeRunV1 {
            executable_sha256: self.authority.sha256().to_owned(),
            executable_version: self.authority.version().to_owned(),
            source_sha256: image.authority().sha256.clone(),
            source_schema: schema,
            decoded_body_record_count,
        })
    }

    fn execute_version(&self) -> Result<String, XnasConformanceError> {
        self.execute_version_with_timeout(REFERENCE_VERSION_TIMEOUT_V1)
    }

    fn execute_version_with_timeout(
        &self,
        timeout: Duration,
    ) -> Result<String, XnasConformanceError> {
        let mut staged = self.spawn_staged(
            &["--version"],
            Stdio::null(),
            Stdio::piped(),
            Stdio::piped(),
        )?;
        let mut stdout = staged.take_stdout().ok_or_else(|| {
            XnasConformanceError::ReferenceExecutable("stdout pipe is absent".to_owned())
        })?;
        let mut stderr = staged.take_stderr().ok_or_else(|| {
            XnasConformanceError::ReferenceExecutable("stderr pipe is absent".to_owned())
        })?;
        let stdout_reader = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes)?;
            Ok(bytes)
        });
        let stderr_reader = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes)?;
            Ok(bytes)
        });
        let status_result =
            staged.wait_until(Instant::now() + timeout, "version probe", timeout);
        let stdout_result = stdout_reader.join();
        let stderr_result = stderr_reader.join();
        let staged_image_result = staged.verify_staged_image();

        staged_image_result?;
        let stdout = stdout_result
            .map_err(|_| {
                XnasConformanceError::ReferenceExecutable("stdout reader panicked".to_owned())
            })?
            .map_err(|error| {
                XnasConformanceError::ReferenceExecutable(format!(
                    "stdout reader: {}",
                    error.kind()
                ))
            })?;
        let stderr = stderr_result
            .map_err(|_| {
                XnasConformanceError::ReferenceExecutable("stderr reader panicked".to_owned())
            })?
            .map_err(|error| {
                XnasConformanceError::ReferenceExecutable(format!(
                    "stderr reader: {}",
                    error.kind()
                ))
            })?;
        let status = status_result?;
        if !status.success() {
            return Err(XnasConformanceError::ReferenceExecutable(format!(
                "exit {:?}: {}",
                status.code(),
                String::from_utf8_lossy(&stderr).trim()
            )));
        }
        if !stderr.is_empty() {
            return Err(XnasConformanceError::ReferenceExecutable(format!(
                "unexpected stderr: {}",
                String::from_utf8_lossy(&stderr).trim()
            )));
        }
        String::from_utf8(stdout)
            .map(|value| value.trim().to_owned())
            .map_err(|error| XnasConformanceError::ReferenceExecutable(error.to_string()))
    }

    /// Stage the retained executable image for exactly one spawn. The private
    /// path is verified before execution and held only for that child.
    fn spawn_staged(
        &self,
        args: &[&str],
        stdin: Stdio,
        stdout: Stdio,
        stderr: Stdio,
    ) -> Result<StagedReferenceChildV1, XnasConformanceError> {
        let staging_directory = tempfile::Builder::new()
            .prefix("xnas-reference-executable-")
            .tempdir()
            .map_err(|error| XnasConformanceError::ReferenceExecutable(error.kind().to_string()))?;
        set_private_directory_permissions(staging_directory.path())?;
        let staged_path = staging_directory.path().join("dbn-reference");
        std::fs::write(&staged_path, &self.executable_bytes)
            .map_err(|error| XnasConformanceError::ReferenceExecutable(error.kind().to_string()))?;
        set_executable_permissions(&staged_path)?;
        let staged_bytes = read_one_image(&staged_path, "staged reference executable")
            .map_err(|error| XnasConformanceError::ReferenceExecutable(error.to_string()))?;
        if sha256_bytes(&staged_bytes) != self.authority.sha256() {
            return Err(XnasConformanceError::ReferenceExecutable(
                "staged executable digest mismatch".to_owned(),
            ));
        }

        let child = Command::new(&staged_path)
            .args(args)
            .stdin(stdin)
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .map_err(|error| XnasConformanceError::ReferenceProcess(error.kind().to_string()))?;
        Ok(StagedReferenceChildV1 {
            child: Some(child),
            staging_directory,
            staged_path,
            expected_sha256: self.authority.sha256().to_owned(),
        })
    }
}

#[cfg(unix)]
fn set_executable_permissions(path: &Path) -> Result<(), XnasConformanceError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o500))
        .map_err(|error| XnasConformanceError::ReferenceExecutable(error.kind().to_string()))
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<(), XnasConformanceError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| XnasConformanceError::ReferenceExecutable(error.kind().to_string()))
}

#[cfg(not(unix))]
fn set_executable_permissions(_path: &Path) -> Result<(), XnasConformanceError> {
    Err(XnasConformanceError::ReferenceExecutable(
        "reference staging requires Unix executable permissions".to_owned(),
    ))
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<(), XnasConformanceError> {
    Err(XnasConformanceError::ReferenceExecutable(
        "reference staging requires Unix directory permissions".to_owned(),
    ))
}

/// Verified immutable blocker bytes and the exact evidence fields they name.
#[derive(Debug)]
pub(crate) struct ResolvedBlockerV1 {
    bytes: Arc<[u8]>,
    sha256: String,
    mbo: BlockerSourceAuthorityV1,
    mbp10: BlockerSourceAuthorityV1,
    reference_decoder: ReferenceDecoderAuthorityV1,
    full_source_file_measurements: serde_json::Value,
}

impl ResolvedBlockerV1 {
    pub(crate) fn source(&self, role: BlockerSourceRoleV1) -> &BlockerSourceAuthorityV1 {
        match role {
            BlockerSourceRoleV1::Mbo => &self.mbo,
            BlockerSourceRoleV1::Mbp10 => &self.mbp10,
        }
    }

    pub(crate) fn reference_decoder(&self) -> &ReferenceDecoderAuthorityV1 {
        &self.reference_decoder
    }

    pub(crate) fn bytes(&self) -> Arc<[u8]> {
        Arc::clone(&self.bytes)
    }

    pub(crate) fn sha256(&self) -> &str {
        &self.sha256
    }

    pub(crate) fn full_source_file_measurements(&self) -> &serde_json::Value {
        &self.full_source_file_measurements
    }
}

/// One-open immutable source and manifest images.
#[derive(Debug)]
pub(crate) struct QualifiedSourceImageV1 {
    authority: BlockerSourceAuthorityV1,
    source_path: PathBuf,
    manifest_path: PathBuf,
    source_bytes: Arc<[u8]>,
    manifest_bytes: Arc<[u8]>,
}

impl QualifiedSourceImageV1 {
    /// Open each blocker-named file once and bind every later consumer to the
    /// resulting immutable in-memory image.
    pub(crate) fn read_once(
        authority: &BlockerSourceAuthorityV1,
    ) -> Result<Self, XnasConformanceError> {
        let source_path = authority.source_path()?;
        let manifest_path = authority.manifest_path()?;
        let source_bytes = read_one_image(&source_path, "source")?;
        let manifest_bytes = read_one_image(&manifest_path, "manifest")?;

        let observed_source_size = u64::try_from(source_bytes.len())
            .map_err(|_| XnasConformanceError::SourceEvidenceMismatch("source size".to_owned()))?;
        if observed_source_size != authority.size_bytes
            || sha256_bytes(&source_bytes) != authority.sha256
        {
            return Err(XnasConformanceError::SourceEvidenceMismatch(
                "source".to_owned(),
            ));
        }
        if sha256_bytes(&manifest_bytes) != authority.manifest_sha256 {
            return Err(XnasConformanceError::SourceEvidenceMismatch(
                "manifest".to_owned(),
            ));
        }

        Ok(Self {
            authority: authority.clone(),
            source_path,
            manifest_path,
            source_bytes,
            manifest_bytes,
        })
    }

    pub(crate) fn authority(&self) -> &BlockerSourceAuthorityV1 {
        &self.authority
    }

    pub(crate) fn source_path(&self) -> &Path {
        &self.source_path
    }

    pub(crate) fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    pub(crate) fn source_bytes(&self) -> Arc<[u8]> {
        Arc::clone(&self.source_bytes)
    }

    pub(crate) fn manifest_bytes(&self) -> Arc<[u8]> {
        Arc::clone(&self.manifest_bytes)
    }

    /// Derive the exact requested daily MBO receipt from the already-verified
    /// native corpus manifest. No filename, size, or source digest is supplied
    /// by the caller.
    pub(crate) fn derive_and_read_manifest_mbo_source(
        &self,
        yyyymmdd: &str,
    ) -> Result<Self, XnasConformanceError> {
        let authority = self.derive_manifest_mbo_authority(yyyymmdd)?;
        let source_path = authority.source_path()?;
        let source_bytes = read_one_image(&source_path, "manifest-derived source")?;
        let observed_source_size = u64::try_from(source_bytes.len()).map_err(|_| {
            XnasConformanceError::SourceEvidenceMismatch("manifest-derived source size".to_owned())
        })?;
        if observed_source_size != authority.size_bytes
            || sha256_bytes(&source_bytes) != authority.sha256
        {
            return Err(XnasConformanceError::SourceEvidenceMismatch(
                "manifest-derived source".to_owned(),
            ));
        }

        Ok(Self {
            authority,
            source_path,
            manifest_path: self.manifest_path.clone(),
            source_bytes,
            manifest_bytes: Arc::clone(&self.manifest_bytes),
        })
    }

    fn derive_manifest_mbo_authority(
        &self,
        yyyymmdd: &str,
    ) -> Result<BlockerSourceAuthorityV1, XnasConformanceError> {
        if self.authority.role != BlockerSourceRoleV1::Mbo || !valid_yyyymmdd(yyyymmdd) {
            return Err(XnasConformanceError::AuthorityInvariant(
                "manifest-derived MBO request".to_owned(),
            ));
        }
        let manifest: NativeManifestV1 =
            serde_json::from_slice(&self.manifest_bytes).map_err(|error| {
                XnasConformanceError::AuthorityJson(format!("native manifest: {error}"))
            })?;
        let expected_prefix = format!("xnas-itch-{yyyymmdd}.");
        let matches = manifest
            .files
            .iter()
            .filter(|entry| {
                entry.filename.starts_with(&expected_prefix)
                    && entry.filename.ends_with(".mbo.dbn.zst")
            })
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(XnasConformanceError::AuthorityInvariant(format!(
                "manifest receipt population for {yyyymmdd}"
            )));
        }
        let receipt = matches[0];
        if receipt.size == 0
            || Path::new(&receipt.filename).components().count() != 1
            || !receipt
                .filename
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-._".contains(&byte))
        {
            return Err(XnasConformanceError::AuthorityInvariant(
                "manifest receipt filename or size".to_owned(),
            ));
        }
        let source_sha256 = receipt.hash.strip_prefix("sha256:").ok_or_else(|| {
            XnasConformanceError::AuthorityInvariant("manifest receipt hash prefix".to_owned())
        })?;
        if !is_lower_hex_sha256(source_sha256) {
            return Err(XnasConformanceError::AuthorityInvariant(
                "manifest receipt hash".to_owned(),
            ));
        }
        let parent = Path::new(&self.authority.workspace_path)
            .parent()
            .ok_or_else(|| {
                XnasConformanceError::AuthorityInvariant(
                    "blocker source workspace parent".to_owned(),
                )
            })?;
        let workspace_path = parent
            .join(&receipt.filename)
            .to_str()
            .ok_or_else(|| {
                XnasConformanceError::AuthorityInvariant(
                    "manifest receipt workspace path".to_owned(),
                )
            })?
            .to_owned();

        let derived = BlockerSourceAuthorityV1 {
            role: BlockerSourceRoleV1::Mbo,
            workspace_path,
            resolved_storage_root: self.authority.resolved_storage_root.clone(),
            size_bytes: receipt.size,
            sha256: source_sha256.to_owned(),
            manifest_path: self.authority.manifest_path.clone(),
            manifest_sha256: self.authority.manifest_sha256.clone(),
            dataset: self.authority.dataset.clone(),
            schema: XnasSchemaV1::Mbo,
            dbn_version: self.authority.dbn_version,
            query: SourceQueryAuthorityV1::ManifestSessionDate {
                yyyymmdd: yyyymmdd.to_owned(),
            },
            raw_symbol: self.authority.raw_symbol.clone(),
            publisher_id: self.authority.publisher_id,
            // A manifest receipt proves bytes, not a historical instrument
            // mapping. The derived daily DBN metadata must independently bind
            // the instrument for this session.
            instrument_id: None,
        };
        let _ = derived.source_path()?;
        Ok(derived)
    }
}

type InMemoryDbnDecoderV1 = DynDecoder<'static, Cursor<Arc<[u8]>>>;

const UTC_DAY_NS: u64 = 86_400_000_000_000;

/// Metadata facts admitted by the exact blocker/manifest authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QualifiedDbnMetadataV1 {
    pub(crate) version: u8,
    pub(crate) dataset: String,
    pub(crate) schema: XnasSchemaV1,
    pub(crate) query_start_ns: u64,
    pub(crate) query_end_ns: u64,
    pub(crate) session_date_yyyymmdd: String,
    pub(crate) raw_symbol: String,
    pub(crate) identity: XnasIdentityV1,
}

/// XNAS RTH decision scope supplied by the corrected row owner together with
/// that owner's calendar digest.
///
/// This module does not parse or independently qualify the calendar. It binds
/// the supplied scope to the verified daily source and structurally owns its
/// close transition so later cursor calls cannot omit the boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct XnasSessionScopeV1 {
    pub(crate) trading_date_yyyymmdd: String,
    pub(crate) rth_open_ns: u64,
    pub(crate) rth_close_ns: u64,
    pub(crate) calendar_sha256: String,
}

impl XnasSessionScopeV1 {
    pub(crate) fn new(
        trading_date_yyyymmdd: String,
        rth_open_ns: u64,
        rth_close_ns: u64,
        calendar_sha256: String,
    ) -> Result<Self, XnasConformanceError> {
        if !valid_yyyymmdd(&trading_date_yyyymmdd)
            || rth_open_ns >= rth_close_ns
            || !is_lower_hex_sha256(&calendar_sha256)
        {
            return Err(XnasConformanceError::Session(
                "date, interval, or calendar digest".to_owned(),
            ));
        }
        Ok(Self {
            trading_date_yyyymmdd,
            rth_open_ns,
            rth_close_ns,
            calendar_sha256,
        })
    }

    fn qualify_against(
        &self,
        metadata: &QualifiedDbnMetadataV1,
    ) -> Result<(), XnasConformanceError> {
        if self.trading_date_yyyymmdd != metadata.session_date_yyyymmdd
            || self.rth_open_ns < metadata.query_start_ns
            || self.rth_close_ns >= metadata.query_end_ns
        {
            return Err(XnasConformanceError::Session(
                "session does not lie inside the qualified daily source".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Auditable proof that one causal decision used exactly the stopping prefix
/// through N(t), with exactly one future record decoded and held as the
/// stopping certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CausalMidpointObservationV1 {
    pub(crate) decision_ns: u64,
    pub(crate) consumed_prefix_ordinal: u64,
    pub(crate) observed_watermark_ns: Option<u64>,
    pub(crate) held_source_ordinal: SourceOrdinal,
    pub(crate) held_watermark_contribution_ns: u64,
    pub(crate) projected_watermark_ns: u64,
    pub(crate) midpoint: Option<CausalBinnedMidpointV1>,
}

/// Full-source scanner populations. The decoder EOF flag is distinct from a
/// market-data closure witness: EOF never closes an update envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct XnasMboSourceScanV1 {
    pub(crate) metadata: QualifiedDbnMetadataV1,
    pub(crate) decoded_body_record_count: u64,
    pub(crate) consumed_body_record_count: u64,
    pub(crate) decoder_eof: bool,
    pub(crate) terminal_cursor_error: Option<XnasConformanceError>,
    pub(crate) publications: Vec<PublishedMboBookV1>,
    pub(crate) semantics: MboSemanticsFinishReportV1,
}

/// Prefix-only report produced after P(close) and the atomically owned session
/// boundary. It is deliberately distinct from a full-source scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct XnasMboSessionSealV1 {
    pub(crate) metadata: QualifiedDbnMetadataV1,
    pub(crate) session: XnasSessionScopeV1,
    pub(crate) decoded_body_record_count: u64,
    pub(crate) consumed_body_record_count: u64,
    pub(crate) held_source_ordinal: SourceOrdinal,
    pub(crate) publications: Vec<PublishedMboBookV1>,
    pub(crate) semantics: MboSemanticsFinishReportV1,
}

/// The only DECISION-031 MBO P(t) path. It owns the exact verified byte image,
/// the DBN decoder, one source-order lookahead, the semantic reducer, and every
/// causal midpoint series.
pub(crate) struct XnasMboCausalCursorV1 {
    image: QualifiedSourceImageV1,
    metadata: QualifiedDbnMetadataV1,
    session: XnasSessionScopeV1,
    decoder: InMemoryDbnDecoderV1,
    stream: Option<XnasMboStreamV1>,
    terminal_semantics: Option<MboSemanticsFinishReportV1>,
    terminal_cursor_error: Option<XnasConformanceError>,
    lookahead: Option<RawMboRecordV1>,
    decoder_eof: bool,
    decoded_body_record_count: u64,
    consumed_body_record_count: u64,
    last_decision_ns: Option<u64>,
    publications: Vec<PublishedMboBookV1>,
}

impl XnasMboCausalCursorV1 {
    pub(crate) fn open(
        image: QualifiedSourceImageV1,
        session: XnasSessionScopeV1,
    ) -> Result<Self, XnasConformanceError> {
        let source_bytes = image.source_bytes();
        let decoder = DynDecoder::inferred_with_buffer(
            Cursor::new(Arc::clone(&source_bytes)),
            VersionUpgradePolicy::AsIs,
        )
        .map_err(|error| XnasConformanceError::DbnDecode(error.to_string()))?;
        let metadata = qualify_mbo_metadata(&image, decoder.metadata())?;
        session.qualify_against(&metadata)?;
        let source_path = utf8_path(image.source_path(), "source")?;
        let manifest_path = utf8_path(image.manifest_path(), "manifest")?;
        let qualification = XnasDailySourceQualificationV1::from_verified_images(
            XnasSchemaV1::Mbo,
            BTreeSet::from([metadata.identity]),
            source_path,
            image.authority.sha256.clone(),
            manifest_path,
            image.authority.manifest_sha256.clone(),
        )?;
        let stream = XnasMboStreamV1::new(qualification);

        Ok(Self {
            image,
            metadata,
            session,
            decoder,
            stream: Some(stream),
            terminal_semantics: None,
            terminal_cursor_error: None,
            lookahead: None,
            decoder_eof: false,
            decoded_body_record_count: 0,
            consumed_body_record_count: 0,
            last_decision_ns: None,
            publications: Vec::new(),
        })
    }

    /// Advance through the exact global source-order prefix N(t), retain the
    /// first record whose prospective valid-receive watermark exceeds t, and
    /// only then emit P(t).
    pub(crate) fn midpoint_at(
        &mut self,
        decision_ns: u64,
    ) -> Result<CausalMidpointObservationV1, XnasConformanceError> {
        if let Some(error) = &self.terminal_cursor_error {
            return Err(error.clone());
        }
        if self.terminal_semantics.is_some() {
            return Err(XnasConformanceError::SessionClosed {
                rth_close_ns: self.session.rth_close_ns,
            });
        }
        if decision_ns < self.session.rth_open_ns || decision_ns > self.session.rth_close_ns {
            return Err(XnasConformanceError::Session(
                "decision outside qualified RTH interval".to_owned(),
            ));
        }
        if let Some(previous) = self.last_decision_ns {
            if decision_ns <= previous {
                return Err(XnasSemanticsError::DecisionTimeNotStrictlyIncreasing {
                    previous,
                    observed: decision_ns,
                }
                .into());
            }
        }

        loop {
            self.fill_lookahead()?;
            let Some(record) = self.lookahead.as_ref() else {
                break;
            };
            let prospective_watermark = match (
                self.stream_ref()?.global_watermark(),
                xnas_mbo_watermark_contribution(record),
            ) {
                (Some(prior), Some(observed)) => Some(prior.max(observed)),
                (None, Some(observed)) => Some(observed),
                (prior, None) => prior,
            };
            if prospective_watermark.is_some_and(|watermark| watermark > decision_ns) {
                break;
            }
            let _ = self.consume_lookahead()?;
        }
        if self.decoder_eof && self.lookahead.is_none() {
            let error = XnasConformanceError::DecisionPrefixEndedAtEof { decision_ns };
            self.terminalize_eof();
            self.terminal_cursor_error = Some(error.clone());
            return Err(error);
        }

        let held = self
            .lookahead
            .as_ref()
            .expect("non-EOF stopping prefix holds one lifting record");
        let held_source_ordinal = held.source_ordinal;
        let held_watermark_contribution_ns =
            xnas_mbo_watermark_contribution(held).expect("lifting record contributes a watermark");
        let observed_watermark_ns = self.stream_ref()?.global_watermark();
        let projected_watermark_ns = observed_watermark_ns
            .map_or(held_watermark_contribution_ns, |prior| {
                prior.max(held_watermark_contribution_ns)
            });
        debug_assert!(projected_watermark_ns > decision_ns);

        let identity = self.metadata.identity;
        let midpoint = self
            .stream_mut()?
            .emit_causal_midpoint_after_complete_prefix(identity, decision_ns)?;
        self.last_decision_ns = Some(decision_ns);
        let observation = CausalMidpointObservationV1 {
            decision_ns,
            consumed_prefix_ordinal: self.consumed_body_record_count,
            observed_watermark_ns,
            held_source_ordinal,
            held_watermark_contribution_ns,
            projected_watermark_ns,
            midpoint,
        };
        if decision_ns == self.session.rth_close_ns {
            self.terminalize_session_boundary();
        }
        Ok(observation)
    }

    /// Consume the complete remaining source in original order and quarantine
    /// every unwitnessed tail at EOF. A session-sealed row cursor cannot be
    /// mislabeled as this whole-file scan.
    pub(crate) fn finish_source_scan(
        mut self,
    ) -> Result<XnasMboSourceScanV1, XnasConformanceError> {
        if self.terminal_semantics.is_some()
            && self.terminal_cursor_error.is_none()
            && !self.decoder_eof
        {
            return Err(XnasConformanceError::SessionClosed {
                rth_close_ns: self.session.rth_close_ns,
            });
        }
        while self.terminal_cursor_error.is_none()
            && (!self.decoder_eof || self.lookahead.is_some())
        {
            if let Err(error) = self.fill_lookahead() {
                debug_assert_eq!(self.terminal_cursor_error.as_ref(), Some(&error));
                break;
            }
            if self.lookahead.is_some() {
                if let Err(error) = self.consume_lookahead() {
                    debug_assert_eq!(self.terminal_cursor_error.as_ref(), Some(&error));
                    break;
                }
            }
        }
        let metadata = self.metadata.clone();
        let decoded_body_record_count = self.decoded_body_record_count;
        let consumed_body_record_count = self.consumed_body_record_count;
        let decoder_eof = self.decoder_eof;
        if self.terminal_cursor_error.is_none() {
            self.terminalize_eof();
        }
        let terminal_cursor_error = self.terminal_cursor_error.take();
        let semantics = self
            .terminal_semantics
            .take()
            .expect("every source-scan termination stores the semantic report");
        Ok(XnasMboSourceScanV1 {
            metadata,
            decoded_body_record_count,
            consumed_body_record_count,
            decoder_eof,
            terminal_cursor_error,
            publications: self.publications,
            semantics,
        })
    }

    /// Return the deliberately prefix-only session report after P(close).
    pub(crate) fn seal_session(mut self) -> Result<XnasMboSessionSealV1, XnasConformanceError> {
        if self.terminal_cursor_error.is_some()
            || self.last_decision_ns != Some(self.session.rth_close_ns)
            || self.decoder_eof
        {
            return Err(XnasConformanceError::Session(
                "cursor was not atomically sealed by P(close)".to_owned(),
            ));
        }
        let held_source_ordinal = self
            .lookahead
            .as_ref()
            .map(|record| record.source_ordinal)
            .ok_or_else(|| {
                XnasConformanceError::Session(
                    "sealed cursor lacks its post-close lifting record".to_owned(),
                )
            })?;
        let semantics = self
            .terminal_semantics
            .take()
            .expect("P(close) stores the session-boundary report");
        Ok(XnasMboSessionSealV1 {
            metadata: self.metadata,
            session: self.session,
            decoded_body_record_count: self.decoded_body_record_count,
            consumed_body_record_count: self.consumed_body_record_count,
            held_source_ordinal,
            publications: self.publications,
            semantics,
        })
    }

    fn fill_lookahead(&mut self) -> Result<(), XnasConformanceError> {
        if let Some(error) = &self.terminal_cursor_error {
            return Err(error.clone());
        }
        if self.lookahead.is_some() || self.decoder_eof {
            return Ok(());
        }
        let decoded = match self.decoder.decode_record_ref() {
            Ok(decoded) => decoded,
            Err(error) => {
                let error = XnasConformanceError::DbnDecode(error.to_string());
                if let Some(stream) = self.stream.as_mut() {
                    stream.invalidate_boundary_causally(XnasBoundaryV1::DecodeGap);
                }
                self.terminalize_cursor_error(error.clone());
                return Err(error);
            }
        };
        let Some(record) = decoded else {
            self.decoder_eof = true;
            return Ok(());
        };

        self.decoded_body_record_count = self
            .decoded_body_record_count
            .checked_add(1)
            .ok_or_else(|| XnasConformanceError::DbnDecode("source ordinal overflow".to_owned()))?;
        let source_ordinal = SourceOrdinal::new(self.decoded_body_record_count)?;
        if !record.has::<MboMsg>() || record.record_size() < size_of::<MboMsg>() {
            let error = XnasConformanceError::DbnDecode(format!(
                "body record {} is not a complete MBO record",
                source_ordinal.get()
            ));
            if let Some(stream) = self.stream.as_mut() {
                stream.invalidate_boundary_causally(XnasBoundaryV1::DecodeGap);
            }
            self.terminalize_cursor_error(error.clone());
            return Err(error);
        }
        let message = record
            .get::<MboMsg>()
            .expect("rtype and encoded length were checked");
        self.lookahead = Some(RawMboRecordV1::from_dbn(source_ordinal, message));
        Ok(())
    }

    fn consume_lookahead(&mut self) -> Result<MboIngestOutcomeV1, XnasConformanceError> {
        if let Some(error) = &self.terminal_cursor_error {
            return Err(error.clone());
        }
        let record = self
            .lookahead
            .take()
            .expect("consume is called only with a held record");
        self.consumed_body_record_count = self
            .consumed_body_record_count
            .checked_add(1)
            .ok_or_else(|| {
                XnasConformanceError::DbnDecode("consumed ordinal overflow".to_owned())
            })?;
        debug_assert_eq!(record.source_ordinal.get(), self.consumed_body_record_count);
        let outcome = match self.stream_mut()?.push_causally(record) {
            Ok(outcome) => outcome,
            Err(error) => {
                let error = XnasConformanceError::Semantics(error);
                self.terminalize_cursor_error(error.clone());
                return Err(error);
            }
        };
        if let MboIngestOutcomeV1::Rejected(rejection) = &outcome {
            if matches!(
                rejection.invalidation_scope,
                MboCausalInvalidationScopeV1::All
            ) {
                let error = XnasConformanceError::Semantics(rejection.error.clone());
                self.terminalize_cursor_error(error.clone());
                return Err(error);
            }
        }
        if let MboIngestOutcomeV1::Accepted {
            disposition: MboIngestDispositionV1::Published(publication),
            ..
        } = &outcome
        {
            self.publications.push((**publication).clone());
        }
        Ok(outcome)
    }

    #[cfg(test)]
    fn source_bytes(&self) -> Arc<[u8]> {
        self.image.source_bytes()
    }

    fn stream_ref(&self) -> Result<&XnasMboStreamV1, XnasConformanceError> {
        self.stream.as_ref().ok_or_else(|| {
            XnasConformanceError::DbnDecode("MBO cursor is already terminal".to_owned())
        })
    }

    fn stream_mut(&mut self) -> Result<&mut XnasMboStreamV1, XnasConformanceError> {
        self.stream.as_mut().ok_or_else(|| {
            XnasConformanceError::DbnDecode("MBO cursor is already terminal".to_owned())
        })
    }

    fn terminalize_eof(&mut self) {
        if self.terminal_semantics.is_none() {
            let stream = self
                .stream
                .take()
                .expect("EOF terminalization occurs exactly once");
            self.terminal_semantics = Some(stream.finish_report());
        }
    }

    fn terminalize_session_boundary(&mut self) {
        if self.terminal_semantics.is_none() {
            let mut stream = self
                .stream
                .take()
                .expect("session terminalization occurs exactly once");
            stream.invalidate_boundary_causally(XnasBoundaryV1::SessionBoundary);
            self.terminal_semantics = Some(stream.finish_report());
        }
    }

    fn terminalize_cursor_error(&mut self, error: XnasConformanceError) {
        if self.terminal_cursor_error.is_none() {
            self.terminal_cursor_error = Some(error);
        }
        if self.terminal_semantics.is_none() {
            let stream = self
                .stream
                .take()
                .expect("source-error terminalization occurs exactly once");
            self.terminal_semantics = Some(stream.finish_report());
        }
    }
}

/// One nonterminal identity-local MBP semantic rejection retained while the
/// source cursor continues toward an accepted reset/recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mbp10SemanticRejectionV1 {
    pub(crate) source_ordinal: SourceOrdinal,
    pub(crate) error: XnasSemanticsError,
}

/// Auditable proof that one MBP comparator decision consumed exactly
/// N_MBP(t) and held the first source-order record lifting its watermark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mbp10PrefixObservationV1 {
    pub(crate) decision_ns: u64,
    pub(crate) consumed_prefix_ordinal: u64,
    pub(crate) observed_watermark_ns: Option<u64>,
    pub(crate) held_source_ordinal: SourceOrdinal,
    pub(crate) held_watermark_contribution_ns: u64,
    pub(crate) projected_watermark_ns: u64,
    pub(crate) completed_endpoint_count: u64,
}

/// Whole-source MBP-10 corroboration report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct XnasMbp10SourceScanV1 {
    pub(crate) metadata: QualifiedDbnMetadataV1,
    pub(crate) decoded_body_record_count: u64,
    pub(crate) consumed_body_record_count: u64,
    pub(crate) decoder_eof: bool,
    pub(crate) terminal_cursor_error: Option<XnasConformanceError>,
    pub(crate) semantic_rejections: Vec<Mbp10SemanticRejectionV1>,
    pub(crate) endpoints: Vec<Mbp10CompletedEndpointV1>,
    pub(crate) semantics: Mbp10SemanticsFinishReportV1,
}

/// Prefix-only MBP report after N_MBP(close) and the owned session boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct XnasMbp10SessionSealV1 {
    pub(crate) metadata: QualifiedDbnMetadataV1,
    pub(crate) session: XnasSessionScopeV1,
    pub(crate) decoded_body_record_count: u64,
    pub(crate) consumed_body_record_count: u64,
    pub(crate) held_source_ordinal: SourceOrdinal,
    pub(crate) semantic_rejections: Vec<Mbp10SemanticRejectionV1>,
    pub(crate) endpoints: Vec<Mbp10CompletedEndpointV1>,
    pub(crate) semantics: Mbp10SemanticsFinishReportV1,
}

/// Source-owned DECISION-031 MBP-10 stopping-prefix cursor. It is
/// structurally channel-less and cannot be fed caller-selected records.
pub(crate) struct XnasMbp10CausalCursorV1 {
    image: QualifiedSourceImageV1,
    metadata: QualifiedDbnMetadataV1,
    session: XnasSessionScopeV1,
    decoder: InMemoryDbnDecoderV1,
    stream: Option<XnasMbp10StreamV1>,
    terminal_semantics: Option<Mbp10SemanticsFinishReportV1>,
    terminal_cursor_error: Option<XnasConformanceError>,
    lookahead: Option<RawMbp10RecordV1>,
    decoder_eof: bool,
    decoded_body_record_count: u64,
    consumed_body_record_count: u64,
    last_decision_ns: Option<u64>,
    semantic_rejections: Vec<Mbp10SemanticRejectionV1>,
    endpoints: Vec<Mbp10CompletedEndpointV1>,
}

impl XnasMbp10CausalCursorV1 {
    pub(crate) fn open(
        image: QualifiedSourceImageV1,
        session: XnasSessionScopeV1,
    ) -> Result<Self, XnasConformanceError> {
        let source_bytes = image.source_bytes();
        let decoder = DynDecoder::inferred_with_buffer(
            Cursor::new(Arc::clone(&source_bytes)),
            VersionUpgradePolicy::AsIs,
        )
        .map_err(|error| XnasConformanceError::DbnDecode(error.to_string()))?;
        let metadata = qualify_mbp10_metadata(&image, decoder.metadata())?;
        session.qualify_against(&metadata)?;
        let source_path = utf8_path(image.source_path(), "source")?;
        let manifest_path = utf8_path(image.manifest_path(), "manifest")?;
        let qualification = XnasDailySourceQualificationV1::from_verified_images(
            XnasSchemaV1::Mbp10,
            BTreeSet::from([metadata.identity]),
            source_path,
            image.authority.sha256.clone(),
            manifest_path,
            image.authority.manifest_sha256.clone(),
        )?;

        Ok(Self {
            image,
            metadata,
            session,
            decoder,
            stream: Some(XnasMbp10StreamV1::new(qualification)),
            terminal_semantics: None,
            terminal_cursor_error: None,
            lookahead: None,
            decoder_eof: false,
            decoded_body_record_count: 0,
            consumed_body_record_count: 0,
            last_decision_ns: None,
            semantic_rejections: Vec::new(),
            endpoints: Vec::new(),
        })
    }

    pub(crate) fn observe_prefix_at(
        &mut self,
        decision_ns: u64,
    ) -> Result<Mbp10PrefixObservationV1, XnasConformanceError> {
        if let Some(error) = &self.terminal_cursor_error {
            return Err(error.clone());
        }
        if self.terminal_semantics.is_some() {
            return Err(XnasConformanceError::SessionClosed {
                rth_close_ns: self.session.rth_close_ns,
            });
        }
        if decision_ns < self.session.rth_open_ns || decision_ns > self.session.rth_close_ns {
            return Err(XnasConformanceError::Session(
                "decision outside qualified RTH interval".to_owned(),
            ));
        }
        if let Some(previous) = self.last_decision_ns {
            if decision_ns <= previous {
                return Err(XnasSemanticsError::DecisionTimeNotStrictlyIncreasing {
                    previous,
                    observed: decision_ns,
                }
                .into());
            }
        }

        loop {
            self.fill_lookahead()?;
            let Some(record) = self.lookahead.as_ref() else {
                break;
            };
            let prospective_watermark = match (
                self.stream_ref()?.global_watermark(),
                xnas_mbp10_watermark_contribution(record),
            ) {
                (Some(prior), Some(observed)) => Some(prior.max(observed)),
                (None, Some(observed)) => Some(observed),
                (prior, None) => prior,
            };
            if prospective_watermark.is_some_and(|watermark| watermark > decision_ns) {
                break;
            }
            self.consume_lookahead()?;
        }
        if self.decoder_eof && self.lookahead.is_none() {
            let error = XnasConformanceError::DecisionPrefixEndedAtEof { decision_ns };
            self.terminalize_eof();
            self.terminal_cursor_error = Some(error.clone());
            return Err(error);
        }

        let held = self
            .lookahead
            .as_ref()
            .expect("non-EOF MBP stopping prefix holds one lifting record");
        let held_source_ordinal = held.source_ordinal;
        let held_watermark_contribution_ns = xnas_mbp10_watermark_contribution(held)
            .expect("MBP lifting record contributes a watermark");
        let observed_watermark_ns = self.stream_ref()?.global_watermark();
        let projected_watermark_ns = observed_watermark_ns
            .map_or(held_watermark_contribution_ns, |prior| {
                prior.max(held_watermark_contribution_ns)
            });
        debug_assert!(projected_watermark_ns > decision_ns);
        self.last_decision_ns = Some(decision_ns);
        let observation = Mbp10PrefixObservationV1 {
            decision_ns,
            consumed_prefix_ordinal: self.consumed_body_record_count,
            observed_watermark_ns,
            held_source_ordinal,
            held_watermark_contribution_ns,
            projected_watermark_ns,
            completed_endpoint_count: self.endpoints.len() as u64,
        };
        if decision_ns == self.session.rth_close_ns {
            self.terminalize_session_boundary();
        }
        Ok(observation)
    }

    pub(crate) fn finish_source_scan(
        mut self,
    ) -> Result<XnasMbp10SourceScanV1, XnasConformanceError> {
        if self.terminal_semantics.is_some()
            && self.terminal_cursor_error.is_none()
            && !self.decoder_eof
        {
            return Err(XnasConformanceError::SessionClosed {
                rth_close_ns: self.session.rth_close_ns,
            });
        }
        while self.terminal_cursor_error.is_none()
            && (!self.decoder_eof || self.lookahead.is_some())
        {
            if let Err(error) = self.fill_lookahead() {
                debug_assert_eq!(self.terminal_cursor_error.as_ref(), Some(&error));
                break;
            }
            if self.lookahead.is_some() {
                if let Err(error) = self.consume_lookahead() {
                    debug_assert_eq!(self.terminal_cursor_error.as_ref(), Some(&error));
                    break;
                }
            }
        }
        let metadata = self.metadata.clone();
        let decoded_body_record_count = self.decoded_body_record_count;
        let consumed_body_record_count = self.consumed_body_record_count;
        let decoder_eof = self.decoder_eof;
        if self.terminal_cursor_error.is_none() {
            self.terminalize_eof();
        }
        let terminal_cursor_error = self.terminal_cursor_error.take();
        let semantics = self
            .terminal_semantics
            .take()
            .expect("every MBP source-scan termination stores the semantic report");
        Ok(XnasMbp10SourceScanV1 {
            metadata,
            decoded_body_record_count,
            consumed_body_record_count,
            decoder_eof,
            terminal_cursor_error,
            semantic_rejections: self.semantic_rejections,
            endpoints: self.endpoints,
            semantics,
        })
    }

    pub(crate) fn seal_session(mut self) -> Result<XnasMbp10SessionSealV1, XnasConformanceError> {
        if self.terminal_cursor_error.is_some()
            || self.last_decision_ns != Some(self.session.rth_close_ns)
            || self.decoder_eof
        {
            return Err(XnasConformanceError::Session(
                "MBP cursor was not atomically sealed at close".to_owned(),
            ));
        }
        let held_source_ordinal = self
            .lookahead
            .as_ref()
            .map(|record| record.source_ordinal)
            .ok_or_else(|| {
                XnasConformanceError::Session(
                    "sealed MBP cursor lacks its post-close lifting record".to_owned(),
                )
            })?;
        let semantics = self
            .terminal_semantics
            .take()
            .expect("MBP close stores the session-boundary report");
        Ok(XnasMbp10SessionSealV1 {
            metadata: self.metadata,
            session: self.session,
            decoded_body_record_count: self.decoded_body_record_count,
            consumed_body_record_count: self.consumed_body_record_count,
            held_source_ordinal,
            semantic_rejections: self.semantic_rejections,
            endpoints: self.endpoints,
            semantics,
        })
    }

    fn fill_lookahead(&mut self) -> Result<(), XnasConformanceError> {
        if let Some(error) = &self.terminal_cursor_error {
            return Err(error.clone());
        }
        if self.lookahead.is_some() || self.decoder_eof {
            return Ok(());
        }
        let decoded = match self.decoder.decode_record_ref() {
            Ok(decoded) => decoded,
            Err(error) => {
                let error = XnasConformanceError::DbnDecode(error.to_string());
                if let Some(stream) = self.stream.as_mut() {
                    stream.invalidate_boundary(XnasBoundaryV1::DecodeGap);
                }
                self.terminalize_cursor_error(error.clone());
                return Err(error);
            }
        };
        let Some(record) = decoded else {
            self.decoder_eof = true;
            return Ok(());
        };

        self.decoded_body_record_count = self
            .decoded_body_record_count
            .checked_add(1)
            .ok_or_else(|| XnasConformanceError::DbnDecode("source ordinal overflow".to_owned()))?;
        let source_ordinal = SourceOrdinal::new(self.decoded_body_record_count)?;
        if !record.has::<Mbp10Msg>() || record.record_size() < size_of::<Mbp10Msg>() {
            let error = XnasConformanceError::DbnDecode(format!(
                "body record {} is not a complete MBP-10 record",
                source_ordinal.get()
            ));
            if let Some(stream) = self.stream.as_mut() {
                stream.invalidate_boundary(XnasBoundaryV1::DecodeGap);
            }
            self.terminalize_cursor_error(error.clone());
            return Err(error);
        }
        let message = record
            .get::<Mbp10Msg>()
            .expect("rtype and encoded length were checked");
        self.lookahead = Some(RawMbp10RecordV1::from_dbn(source_ordinal, message));
        Ok(())
    }

    fn consume_lookahead(&mut self) -> Result<(), XnasConformanceError> {
        if let Some(error) = &self.terminal_cursor_error {
            return Err(error.clone());
        }
        let record = self
            .lookahead
            .take()
            .expect("MBP consume is called only with a held record");
        let source_ordinal = record.source_ordinal;
        self.consumed_body_record_count = self
            .consumed_body_record_count
            .checked_add(1)
            .ok_or_else(|| {
                XnasConformanceError::DbnDecode("consumed ordinal overflow".to_owned())
            })?;
        debug_assert_eq!(source_ordinal.get(), self.consumed_body_record_count);
        match self.stream_mut()?.push(record) {
            Ok(Some(endpoint)) => self.endpoints.push(endpoint),
            Ok(None) => {}
            Err(error) => {
                if self
                    .stream_ref()?
                    .terminal_error()
                    .is_some_and(|terminal| terminal == &error)
                {
                    let error = XnasConformanceError::Semantics(error);
                    self.terminalize_cursor_error(error.clone());
                    return Err(error);
                }
                self.semantic_rejections.push(Mbp10SemanticRejectionV1 {
                    source_ordinal,
                    error,
                });
            }
        }
        Ok(())
    }

    #[cfg(test)]
    fn source_bytes(&self) -> Arc<[u8]> {
        self.image.source_bytes()
    }

    fn stream_ref(&self) -> Result<&XnasMbp10StreamV1, XnasConformanceError> {
        self.stream.as_ref().ok_or_else(|| {
            XnasConformanceError::DbnDecode("MBP cursor is already terminal".to_owned())
        })
    }

    fn stream_mut(&mut self) -> Result<&mut XnasMbp10StreamV1, XnasConformanceError> {
        self.stream.as_mut().ok_or_else(|| {
            XnasConformanceError::DbnDecode("MBP cursor is already terminal".to_owned())
        })
    }

    fn terminalize_eof(&mut self) {
        if self.terminal_semantics.is_none() {
            let stream = self
                .stream
                .take()
                .expect("MBP EOF terminalization occurs exactly once");
            self.terminal_semantics = Some(stream.finish_report());
        }
    }

    fn terminalize_session_boundary(&mut self) {
        if self.terminal_semantics.is_none() {
            let mut stream = self
                .stream
                .take()
                .expect("MBP session terminalization occurs exactly once");
            stream.invalidate_boundary(XnasBoundaryV1::SessionBoundary);
            self.terminal_semantics = Some(stream.finish_report());
        }
    }

    fn terminalize_cursor_error(&mut self, error: XnasConformanceError) {
        if self.terminal_cursor_error.is_none() {
            self.terminal_cursor_error = Some(error);
        }
        if self.terminal_semantics.is_none() {
            let stream = self
                .stream
                .take()
                .expect("MBP source-error terminalization occurs exactly once");
            self.terminal_semantics = Some(stream.finish_report());
        }
    }
}

fn qualify_mbo_metadata(
    image: &QualifiedSourceImageV1,
    metadata: &dbn::Metadata,
) -> Result<QualifiedDbnMetadataV1, XnasConformanceError> {
    qualify_dbn_metadata(
        image,
        metadata,
        BlockerSourceRoleV1::Mbo,
        XnasSchemaV1::Mbo,
        Schema::Mbo,
    )
}

fn qualify_mbp10_metadata(
    image: &QualifiedSourceImageV1,
    metadata: &dbn::Metadata,
) -> Result<QualifiedDbnMetadataV1, XnasConformanceError> {
    qualify_dbn_metadata(
        image,
        metadata,
        BlockerSourceRoleV1::Mbp10,
        XnasSchemaV1::Mbp10,
        Schema::Mbp10,
    )
}

fn qualify_dbn_metadata(
    image: &QualifiedSourceImageV1,
    metadata: &dbn::Metadata,
    source_role: BlockerSourceRoleV1,
    xnas_schema: XnasSchemaV1,
    dbn_schema: Schema,
) -> Result<QualifiedDbnMetadataV1, XnasConformanceError> {
    let authority = image.authority();
    if authority.role != source_role
        || authority.schema != xnas_schema
        || metadata.version != authority.dbn_version
        || metadata.dataset != authority.dataset
        || metadata.schema != Some(dbn_schema)
        || metadata.ts_out
        || metadata.limit.is_some()
        || metadata.stype_in != Some(SType::RawSymbol)
        || metadata.stype_out != SType::InstrumentId
        || !metadata.partial.is_empty()
        || !metadata.not_found.is_empty()
        || metadata.symbols.len() != 1
        || metadata.symbols[0] != authority.raw_symbol
    {
        return Err(XnasConformanceError::Metadata(
            "header, schema, symbology, or completeness".to_owned(),
        ));
    }
    let query_end_ns = metadata.end.map(|value| value.get()).ok_or_else(|| {
        XnasConformanceError::Metadata("query end is absent from metadata".to_owned())
    })?;
    if metadata.start >= query_end_ns {
        return Err(XnasConformanceError::Metadata(
            "nonpositive query interval".to_owned(),
        ));
    }
    match &authority.query {
        SourceQueryAuthorityV1::ExactIntervalNs { start_ns, end_ns }
            if metadata.start == *start_ns && query_end_ns == *end_ns => {}
        SourceQueryAuthorityV1::ManifestSessionDate { yyyymmdd }
            if metadata.start % UTC_DAY_NS == 0
                && query_end_ns
                    .checked_sub(metadata.start)
                    .is_some_and(|span| span == UTC_DAY_NS)
                && metadata_date_yyyymmdd(metadata) == *yyyymmdd => {}
        SourceQueryAuthorityV1::ExactIntervalNs { .. }
        | SourceQueryAuthorityV1::ManifestSessionDate { .. } => {
            return Err(XnasConformanceError::Metadata(
                "query interval does not match authority".to_owned(),
            ));
        }
    }

    let session_date = metadata.start().date();
    if metadata.mappings.len() != 1 || metadata.mappings[0].raw_symbol != authority.raw_symbol {
        return Err(XnasConformanceError::Metadata(
            "raw-symbol mapping population".to_owned(),
        ));
    }
    let active_mappings = metadata.mappings[0]
        .intervals
        .iter()
        .filter(|interval| interval.start_date <= session_date && session_date < interval.end_date)
        .collect::<Vec<_>>();
    if active_mappings.len() != 1 {
        return Err(XnasConformanceError::Metadata(
            "instrument mapping".to_owned(),
        ));
    }
    let mapped_instrument_id = active_mappings[0].symbol.parse::<u32>().map_err(|_| {
        XnasConformanceError::Metadata("instrument mapping is not numeric".to_owned())
    })?;
    if authority
        .instrument_id
        .is_some_and(|expected| mapped_instrument_id != expected)
    {
        return Err(XnasConformanceError::Metadata(
            "instrument mapping differs from exact authority".to_owned(),
        ));
    }

    Ok(QualifiedDbnMetadataV1 {
        version: metadata.version,
        dataset: metadata.dataset.clone(),
        schema: xnas_schema,
        query_start_ns: metadata.start,
        query_end_ns,
        session_date_yyyymmdd: metadata_date_yyyymmdd(metadata),
        raw_symbol: authority.raw_symbol.clone(),
        identity: XnasIdentityV1::new(authority.publisher_id, mapped_instrument_id),
    })
}

fn metadata_date_yyyymmdd(metadata: &dbn::Metadata) -> String {
    metadata.start().date().to_string().replace('-', "")
}

fn utf8_path(path: &Path, subject: &str) -> Result<String, XnasConformanceError> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| XnasConformanceError::SourceEvidenceMismatch(subject.to_owned()))
}

/// Resolve the exact sealed blocker from the local MBO repository.
pub(crate) fn resolve_accepted_blocker(
    repository: &Path,
) -> Result<ResolvedBlockerV1, XnasConformanceError> {
    let spec = format!("{BLOCKER_COMMIT_V1}:{BLOCKER_PATH_V1}");
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["cat-file", "blob"])
        .arg(spec)
        .output()
        .map_err(|error| XnasConformanceError::GitObject(error.kind().to_string()))?;
    if !output.status.success() {
        return Err(XnasConformanceError::GitObject(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    resolve_blocker_bytes(output.stdout, BLOCKER_SHA256_V1)
}

fn resolve_blocker_bytes(
    bytes: Vec<u8>,
    expected_sha256: &str,
) -> Result<ResolvedBlockerV1, XnasConformanceError> {
    let observed_sha256 = sha256_bytes(&bytes);
    if observed_sha256 != expected_sha256 {
        return Err(XnasConformanceError::AuthorityDigestMismatch);
    }
    let raw: RawBlockerV1 = serde_json::from_slice(&bytes)
        .map_err(|error| XnasConformanceError::AuthorityJson(error.to_string()))?;
    validate_blocker_header(&raw)?;

    let mbo = raw
        .sources
        .mbo
        .try_into_authority(BlockerSourceRoleV1::Mbo)?;
    let mbp10 = raw
        .sources
        .mbp10
        .try_into_authority(BlockerSourceRoleV1::Mbp10)?;
    validate_cross_source_authority(&mbo, &mbp10)?;
    validate_reference_decoder(&raw.reference_decoder)?;

    Ok(ResolvedBlockerV1 {
        bytes: Arc::from(bytes),
        sha256: observed_sha256,
        mbo,
        mbp10,
        reference_decoder: raw.reference_decoder.into(),
        full_source_file_measurements: raw.full_source_file_measurements,
    })
}

fn validate_blocker_header(raw: &RawBlockerV1) -> Result<(), XnasConformanceError> {
    if raw.schema_version != BLOCKER_SCHEMA_VERSION_V1
        || raw.artifact_id != BLOCKER_ARTIFACT_ID_V1
        || raw.artifact_kind != BLOCKER_ARTIFACT_KIND_V1
        || raw.status != BLOCKER_STATUS_V1
        || !raw.terminal_for_current_authority
    {
        return Err(XnasConformanceError::AuthorityInvariant(
            "blocker header".to_owned(),
        ));
    }
    Ok(())
}

fn validate_cross_source_authority(
    mbo: &BlockerSourceAuthorityV1,
    mbp10: &BlockerSourceAuthorityV1,
) -> Result<(), XnasConformanceError> {
    if mbo.schema != XnasSchemaV1::Mbo
        || mbp10.schema != XnasSchemaV1::Mbp10
        || mbo.dataset != "XNAS.ITCH"
        || mbp10.dataset != "XNAS.ITCH"
        || mbo.dbn_version != 1
        || mbp10.dbn_version != 1
        || mbo.raw_symbol != "NVDA"
        || mbp10.raw_symbol != "NVDA"
        || mbo.publisher_id != XNAS_ITCH_PUBLISHER_ID
        || mbp10.publisher_id != XNAS_ITCH_PUBLISHER_ID
        || mbo.publisher_id != mbp10.publisher_id
        || mbo.instrument_id.is_none()
        || mbo.instrument_id != mbp10.instrument_id
        || mbo.query != mbp10.query
    {
        return Err(XnasConformanceError::AuthorityInvariant(
            "cross-source identity or query".to_owned(),
        ));
    }
    Ok(())
}

fn validate_reference_decoder(
    authority: &RawReferenceDecoderV1,
) -> Result<(), XnasConformanceError> {
    if authority.executable.is_empty()
        || authority.version.is_empty()
        || !is_lower_hex_sha256(&authority.sha256)
        || authority.output_encoding != "NDJSON"
        || authority.timestamps_and_prices != "raw integer strings"
        || !authority
            .source_ordinal_definition
            .starts_with("One-based body-record position")
    {
        return Err(XnasConformanceError::AuthorityInvariant(
            "reference decoder".to_owned(),
        ));
    }
    Ok(())
}

fn join_authority_path(root: &str, relative: &str) -> Result<PathBuf, XnasConformanceError> {
    let root = Path::new(root);
    let relative_path = Path::new(relative);
    let storage_relative = relative_path.strip_prefix("data").map_err(|_| {
        XnasConformanceError::UnsafeAuthorityPath(format!(
            "{relative} does not begin with the workspace data mount"
        ))
    })?;
    if !root.is_absolute()
        || relative_path.is_absolute()
        || storage_relative.as_os_str().is_empty()
        || storage_relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(XnasConformanceError::UnsafeAuthorityPath(
            relative.to_owned(),
        ));
    }
    Ok(root.join(storage_relative))
}

fn read_one_image(path: &Path, subject: &str) -> Result<Arc<[u8]>, XnasConformanceError> {
    std::fs::read(path)
        .map(Arc::from)
        .map_err(|error| XnasConformanceError::SourceIo {
            subject: subject.to_owned(),
            kind: error.kind().to_string(),
        })
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Debug, Deserialize)]
struct RawBlockerV1 {
    schema_version: String,
    artifact_id: String,
    artifact_kind: String,
    status: String,
    terminal_for_current_authority: bool,
    sources: RawSourcesV1,
    reference_decoder: RawReferenceDecoderV1,
    full_source_file_measurements: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct RawSourcesV1 {
    mbo: RawSourceAuthorityV1,
    mbp10: RawSourceAuthorityV1,
}

#[derive(Debug, Deserialize)]
struct RawSourceAuthorityV1 {
    workspace_path: String,
    resolved_storage_root: String,
    size_bytes: u64,
    sha256: String,
    manifest_path: String,
    manifest_sha256: String,
    manifest_size_and_hash_match: bool,
    dataset: String,
    schema: String,
    dbn_version: u8,
    start_ns: String,
    end_ns: String,
    raw_symbol: String,
    instrument_id: u32,
    publisher_id: u16,
}

impl RawSourceAuthorityV1 {
    fn try_into_authority(
        self,
        role: BlockerSourceRoleV1,
    ) -> Result<BlockerSourceAuthorityV1, XnasConformanceError> {
        let expected_schema = match role {
            BlockerSourceRoleV1::Mbo => ("mbo", XnasSchemaV1::Mbo),
            BlockerSourceRoleV1::Mbp10 => ("mbp-10", XnasSchemaV1::Mbp10),
        };
        if self.schema != expected_schema.0
            || !self.manifest_size_and_hash_match
            || self.size_bytes == 0
            || !is_lower_hex_sha256(&self.sha256)
            || !is_lower_hex_sha256(&self.manifest_sha256)
        {
            return Err(XnasConformanceError::AuthorityInvariant(
                "source evidence".to_owned(),
            ));
        }
        let start_ns = self
            .start_ns
            .parse()
            .map_err(|_| XnasConformanceError::AuthorityInvariant("source start_ns".to_owned()))?;
        let end_ns = self
            .end_ns
            .parse()
            .map_err(|_| XnasConformanceError::AuthorityInvariant("source end_ns".to_owned()))?;
        if start_ns >= end_ns {
            return Err(XnasConformanceError::AuthorityInvariant(
                "source interval".to_owned(),
            ));
        }
        let _ = join_authority_path(&self.resolved_storage_root, &self.workspace_path)?;
        let _ = join_authority_path(&self.resolved_storage_root, &self.manifest_path)?;
        Ok(BlockerSourceAuthorityV1 {
            role,
            workspace_path: self.workspace_path,
            resolved_storage_root: self.resolved_storage_root,
            size_bytes: self.size_bytes,
            sha256: self.sha256,
            manifest_path: self.manifest_path,
            manifest_sha256: self.manifest_sha256,
            dataset: self.dataset,
            schema: expected_schema.1,
            dbn_version: self.dbn_version,
            query: SourceQueryAuthorityV1::ExactIntervalNs { start_ns, end_ns },
            raw_symbol: self.raw_symbol,
            publisher_id: self.publisher_id,
            instrument_id: Some(self.instrument_id),
        })
    }
}

#[derive(Debug, Deserialize)]
struct RawReferenceDecoderV1 {
    executable: String,
    version: String,
    sha256: String,
    output_encoding: String,
    timestamps_and_prices: String,
    source_ordinal_definition: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReferenceHeaderJsonV1 {
    ts_event: String,
    rtype: u8,
    publisher_id: u16,
    instrument_id: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReferenceMboJsonV1 {
    ts_recv: String,
    hd: ReferenceHeaderJsonV1,
    action: String,
    side: String,
    price: String,
    size: u32,
    channel_id: u8,
    order_id: String,
    flags: u8,
    ts_in_delta: i32,
    sequence: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReferenceMbp10JsonV1 {
    ts_recv: String,
    hd: ReferenceHeaderJsonV1,
    action: String,
    side: String,
    depth: u8,
    price: String,
    size: u32,
    flags: u8,
    ts_in_delta: i32,
    sequence: u32,
    levels: Vec<ReferenceMbp10LevelJsonV1>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReferenceMbp10LevelJsonV1 {
    bid_px: String,
    ask_px: String,
    bid_sz: u32,
    ask_sz: u32,
    bid_ct: u32,
    ask_ct: u32,
}

fn parse_reference_mbo_line(
    source_ordinal: SourceOrdinal,
    line: &str,
) -> Result<RawMboRecordV1, XnasConformanceError> {
    let value: ReferenceMboJsonV1 = serde_json::from_str(line)
        .map_err(|error| XnasConformanceError::ReferenceNdjson(error.to_string()))?;
    Ok(RawMboRecordV1 {
        source_ordinal,
        rtype: value.hd.rtype,
        publisher_id: value.hd.publisher_id,
        instrument_id: value.hd.instrument_id,
        ts_event: parse_reference_integer(&value.hd.ts_event, "MBO ts_event")?,
        order_id: parse_reference_integer(&value.order_id, "MBO order_id")?,
        price: parse_reference_integer(&value.price, "MBO price")?,
        size: value.size,
        flags: value.flags,
        channel_id: value.channel_id,
        action: parse_reference_char(&value.action, "MBO action")?,
        side: parse_reference_char(&value.side, "MBO side")?,
        ts_recv: parse_reference_integer(&value.ts_recv, "MBO ts_recv")?,
        ts_in_delta: value.ts_in_delta,
        sequence: value.sequence,
    })
}

fn parse_reference_mbp10_line(
    source_ordinal: SourceOrdinal,
    line: &str,
) -> Result<RawMbp10RecordV1, XnasConformanceError> {
    let value: ReferenceMbp10JsonV1 = serde_json::from_str(line)
        .map_err(|error| XnasConformanceError::ReferenceNdjson(error.to_string()))?;
    let levels = value
        .levels
        .into_iter()
        .map(|level| {
            Ok(crate::xnas_semantics::Mbp10LevelV1 {
                bid_px: parse_reference_integer(&level.bid_px, "MBP bid_px")?,
                ask_px: parse_reference_integer(&level.ask_px, "MBP ask_px")?,
                bid_sz: level.bid_sz,
                ask_sz: level.ask_sz,
                bid_ct: level.bid_ct,
                ask_ct: level.ask_ct,
            })
        })
        .collect::<Result<Vec<_>, XnasConformanceError>>()?
        .try_into()
        .map_err(|levels: Vec<_>| {
            XnasConformanceError::ReferenceNdjson(format!(
                "MBP level population is {}, expected 10",
                levels.len()
            ))
        })?;
    Ok(RawMbp10RecordV1 {
        source_ordinal,
        rtype: value.hd.rtype,
        publisher_id: value.hd.publisher_id,
        instrument_id: value.hd.instrument_id,
        ts_event: parse_reference_integer(&value.hd.ts_event, "MBP ts_event")?,
        price: parse_reference_integer(&value.price, "MBP price")?,
        size: value.size,
        action: parse_reference_char(&value.action, "MBP action")?,
        side: parse_reference_char(&value.side, "MBP side")?,
        flags: value.flags,
        depth: value.depth,
        ts_recv: parse_reference_integer(&value.ts_recv, "MBP ts_recv")?,
        ts_in_delta: value.ts_in_delta,
        sequence: value.sequence,
        levels,
    })
}

fn parse_reference_integer<T>(value: &str, field: &str) -> Result<T, XnasConformanceError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value.parse::<T>().map_err(|error| {
        XnasConformanceError::ReferenceNdjson(format!("{field} is not an integer: {error}"))
    })
}

fn parse_reference_char(value: &str, field: &str) -> Result<u8, XnasConformanceError> {
    let bytes = value.as_bytes();
    if bytes.len() != 1 || !bytes[0].is_ascii() {
        return Err(XnasConformanceError::ReferenceNdjson(format!(
            "{field} is not one ASCII byte"
        )));
    }
    Ok(bytes[0])
}

#[derive(Debug, Deserialize)]
struct NativeManifestV1 {
    files: Vec<NativeManifestFileV1>,
}

#[derive(Debug, Deserialize)]
struct NativeManifestFileV1 {
    filename: String,
    size: u64,
    hash: String,
}

impl From<RawReferenceDecoderV1> for ReferenceDecoderAuthorityV1 {
    fn from(value: RawReferenceDecoderV1) -> Self {
        Self {
            executable: value.executable,
            version: value.version,
            sha256: value.sha256,
            output_encoding: value.output_encoding,
            timestamps_and_prices: value.timestamps_and_prices,
            source_ordinal_definition: value.source_ordinal_definition,
        }
    }
}

fn valid_yyyymmdd(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::num::NonZeroU64;

    use dbn::encode::{dbn::Encoder, EncodeDbn, EncodeRecord};
    use serde_json::json;
    use tempfile::TempDir;

    use crate::xnas_semantics::{DBN_FLAG_BAD_TS_RECV, DBN_FLAG_LAST, DBN_FLAG_MAYBE_BAD_BOOK};

    use super::*;

    fn blocker_bytes(root: &Path, source: &[u8], manifest: &[u8]) -> Vec<u8> {
        let source_sha256 = sha256_bytes(source);
        let manifest_sha256 = sha256_bytes(manifest);
        serde_json::to_vec(&json!({
            "schema_version": BLOCKER_SCHEMA_VERSION_V1,
            "artifact_id": BLOCKER_ARTIFACT_ID_V1,
            "artifact_kind": BLOCKER_ARTIFACT_KIND_V1,
            "status": BLOCKER_STATUS_V1,
            "terminal_for_current_authority": true,
            "sources": {
                "mbo": {
                    "workspace_path": "data/mbo.dbn.zst",
                    "resolved_storage_root": root,
                    "size_bytes": source.len(),
                    "sha256": source_sha256,
                    "manifest_path": "data/manifest.json",
                    "manifest_sha256": manifest_sha256,
                    "manifest_size_and_hash_match": true,
                    "dataset": "XNAS.ITCH",
                    "schema": "mbo",
                    "dbn_version": 1,
                    "start_ns": "1",
                    "end_ns": "2",
                    "raw_symbol": "NVDA",
                    "instrument_id": 11667,
                    "publisher_id": 2
                },
                "mbp10": {
                    "workspace_path": "data/mbp10.dbn.zst",
                    "resolved_storage_root": root,
                    "size_bytes": source.len(),
                    "sha256": source_sha256,
                    "manifest_path": "data/manifest.json",
                    "manifest_sha256": manifest_sha256,
                    "manifest_size_and_hash_match": true,
                    "dataset": "XNAS.ITCH",
                    "schema": "mbp-10",
                    "dbn_version": 1,
                    "start_ns": "1",
                    "end_ns": "2",
                    "raw_symbol": "NVDA",
                    "instrument_id": 11667,
                    "publisher_id": 2
                }
            },
            "reference_decoder": {
                "executable": "/verified/dbn",
                "version": "dbn-cli 0.20.1",
                "sha256": "a".repeat(64),
                "output_encoding": "NDJSON",
                "timestamps_and_prices": "raw integer strings",
                "source_ordinal_definition":
                    "One-based body-record position in reference CLI NDJSON output."
            },
            "full_source_file_measurements": {"retained": true}
        }))
        .unwrap()
    }

    fn resolved_fixture(temp: &TempDir, source: &[u8], manifest: &[u8]) -> ResolvedBlockerV1 {
        let bytes = blocker_bytes(temp.path(), source, manifest);
        let digest = sha256_bytes(&bytes);
        resolve_blocker_bytes(bytes, &digest).unwrap()
    }

    fn resolved_fixture_with_interval(
        temp: &TempDir,
        source: &[u8],
        manifest: &[u8],
        start_ns: u64,
        end_ns: u64,
    ) -> ResolvedBlockerV1 {
        let bytes = blocker_bytes(temp.path(), source, manifest);
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        for role in ["mbo", "mbp10"] {
            value["sources"][role]["start_ns"] = json!(start_ns.to_string());
            value["sources"][role]["end_ns"] = json!(end_ns.to_string());
        }
        let bytes = serde_json::to_vec(&value).unwrap();
        let digest = sha256_bytes(&bytes);
        resolve_blocker_bytes(bytes, &digest).unwrap()
    }

    fn mbo_message(
        ts_event: u64,
        ts_recv: u64,
        sequence: u32,
        flags: u8,
        order_id: u64,
        action: u8,
        side: u8,
    ) -> MboMsg {
        MboMsg {
            hd: dbn::RecordHeader::new::<MboMsg>(
                dbn::rtype::MBO,
                XNAS_ITCH_PUBLISHER_ID,
                11_667,
                ts_event,
            ),
            order_id,
            price: if action == b'R' {
                dbn::UNDEF_PRICE
            } else {
                100_000_000_000
            },
            size: if action == b'R' { 0 } else { 10 },
            flags: dbn::FlagSet::new(flags),
            channel_id: 0,
            action: action as _,
            side: side as _,
            ts_recv,
            ts_in_delta: 0,
            sequence,
        }
    }

    fn mbo_message_for_instrument(
        instrument_id: u32,
        ts_event: u64,
        ts_recv: u64,
        sequence: u32,
        flags: u8,
        order_id: u64,
        action: u8,
        side: u8,
    ) -> MboMsg {
        let mut message = mbo_message(ts_event, ts_recv, sequence, flags, order_id, action, side);
        message.hd.instrument_id = instrument_id;
        message
    }

    fn encoded_mbo_source(
        start_ns: u64,
        end_ns: u64,
        instrument_id: u32,
        records: &[MboMsg],
    ) -> (Vec<u8>, String) {
        let mut metadata = dbn::Metadata::builder()
            .version(1)
            .dataset("XNAS.ITCH".to_owned())
            .schema(Some(Schema::Mbo))
            .start(start_ns)
            .end(NonZeroU64::new(end_ns))
            .stype_in(Some(SType::RawSymbol))
            .stype_out(SType::InstrumentId)
            .symbols(vec!["NVDA".to_owned()])
            .build();
        let session_date = metadata.start().date();
        let session_date_yyyymmdd = session_date.to_string().replace('-', "");
        metadata.mappings = vec![dbn::SymbolMapping {
            raw_symbol: "NVDA".to_owned(),
            intervals: vec![dbn::MappingInterval {
                start_date: session_date,
                end_date: session_date.next_day().unwrap(),
                symbol: instrument_id.to_string(),
            }],
        }];
        let mut bytes = Vec::new();
        let mut encoder = Encoder::new(&mut bytes, &metadata).unwrap();
        encoder.encode_records(records).unwrap();
        encoder.flush().unwrap();
        (bytes, session_date_yyyymmdd)
    }

    fn mbp10_message(
        ts_event: u64,
        ts_recv: u64,
        sequence: u32,
        flags: u8,
        action: u8,
        side: u8,
        level_offset: i64,
    ) -> Mbp10Msg {
        let levels = std::array::from_fn(|idx| dbn::BidAskPair {
            bid_px: 100_000_000_000 - idx as i64 * 1_000_000 + level_offset,
            ask_px: 100_010_000_000 + idx as i64 * 1_000_000 + level_offset,
            bid_sz: 100 + idx as u32,
            ask_sz: 200 + idx as u32,
            bid_ct: 10 + idx as u32,
            ask_ct: 20 + idx as u32,
        });
        Mbp10Msg {
            hd: dbn::RecordHeader::new::<Mbp10Msg>(
                dbn::rtype::MBP_10,
                XNAS_ITCH_PUBLISHER_ID,
                11_667,
                ts_event,
            ),
            price: if action == b'R' {
                dbn::UNDEF_PRICE
            } else {
                100_000_000_000 + level_offset
            },
            size: if action == b'R' { 0 } else { 10 },
            action: action as _,
            side: side as _,
            flags: dbn::FlagSet::new(flags),
            depth: 0,
            ts_recv,
            ts_in_delta: 0,
            sequence,
            levels,
        }
    }

    fn encoded_mbp10_source(start_ns: u64, end_ns: u64, records: &[Mbp10Msg]) -> (Vec<u8>, String) {
        let mut metadata = dbn::Metadata::builder()
            .version(1)
            .dataset("XNAS.ITCH".to_owned())
            .schema(Some(Schema::Mbp10))
            .start(start_ns)
            .end(NonZeroU64::new(end_ns))
            .stype_in(Some(SType::RawSymbol))
            .stype_out(SType::InstrumentId)
            .symbols(vec!["NVDA".to_owned()])
            .build();
        let session_date = metadata.start().date();
        let session_date_yyyymmdd = session_date.to_string().replace('-', "");
        metadata.mappings = vec![dbn::SymbolMapping {
            raw_symbol: "NVDA".to_owned(),
            intervals: vec![dbn::MappingInterval {
                start_date: session_date,
                end_date: session_date.next_day().unwrap(),
                symbol: "11667".to_owned(),
            }],
        }];
        let mut bytes = Vec::new();
        let mut encoder = Encoder::new(&mut bytes, &metadata).unwrap();
        encoder.encode_records(records).unwrap();
        encoder.flush().unwrap();
        (bytes, session_date_yyyymmdd)
    }

    fn cursor_fixture(
        records: &[MboMsg],
        start_ns: u64,
        end_ns: u64,
    ) -> (TempDir, XnasMboCausalCursorV1) {
        cursor_fixture_with_scope(records, start_ns, end_ns, start_ns, end_ns - 1)
    }

    fn cursor_fixture_with_scope(
        records: &[MboMsg],
        start_ns: u64,
        end_ns: u64,
        rth_open_ns: u64,
        rth_close_ns: u64,
    ) -> (TempDir, XnasMboCausalCursorV1) {
        let temp = TempDir::new().unwrap();
        let (source, session_date_yyyymmdd) = encoded_mbo_source(start_ns, end_ns, 11_667, records);
        let manifest = b"exact-manifest";
        fs::write(temp.path().join("mbo.dbn.zst"), &source).unwrap();
        fs::write(temp.path().join("mbp10.dbn.zst"), &source).unwrap();
        fs::write(temp.path().join("manifest.json"), manifest).unwrap();
        let blocker = resolved_fixture_with_interval(&temp, &source, manifest, start_ns, end_ns);
        let image =
            QualifiedSourceImageV1::read_once(blocker.source(BlockerSourceRoleV1::Mbo)).unwrap();
        let session = XnasSessionScopeV1::new(
            session_date_yyyymmdd,
            rth_open_ns,
            rth_close_ns,
            "b".repeat(64),
        )
        .unwrap();
        let cursor = XnasMboCausalCursorV1::open(image, session).unwrap();
        (temp, cursor)
    }

    fn mbp10_cursor_fixture_with_scope(
        records: &[Mbp10Msg],
        start_ns: u64,
        end_ns: u64,
        rth_open_ns: u64,
        rth_close_ns: u64,
    ) -> (TempDir, XnasMbp10CausalCursorV1) {
        let temp = TempDir::new().unwrap();
        let (source, session_date_yyyymmdd) = encoded_mbp10_source(start_ns, end_ns, records);
        let manifest = b"exact-manifest";
        fs::write(temp.path().join("mbo.dbn.zst"), &source).unwrap();
        fs::write(temp.path().join("mbp10.dbn.zst"), &source).unwrap();
        fs::write(temp.path().join("manifest.json"), manifest).unwrap();
        let blocker = resolved_fixture_with_interval(&temp, &source, manifest, start_ns, end_ns);
        let image =
            QualifiedSourceImageV1::read_once(blocker.source(BlockerSourceRoleV1::Mbp10)).unwrap();
        let session = XnasSessionScopeV1::new(
            session_date_yyyymmdd,
            rth_open_ns,
            rth_close_ns,
            "d".repeat(64),
        )
        .unwrap();
        let cursor = XnasMbp10CausalCursorV1::open(image, session).unwrap();
        (temp, cursor)
    }

    #[cfg(unix)]
    #[test]
    fn reference_executable_and_source_are_consumed_from_qualified_images() {
        let temp = TempDir::new().unwrap();
        let source = b"reference-source";
        let manifest = b"exact-manifest";
        fs::write(temp.path().join("mbo.dbn.zst"), source).unwrap();
        fs::write(temp.path().join("mbp10.dbn.zst"), source).unwrap();
        fs::write(temp.path().join("manifest.json"), manifest).unwrap();
        let blocker = resolved_fixture(&temp, source, manifest);
        let image =
            QualifiedSourceImageV1::read_once(blocker.source(BlockerSourceRoleV1::Mbo)).unwrap();

        let script = br#"#!/bin/sh
if [ "$1" = "--version" ]; then
    printf '%s\n' 'fake-dbn 0.20.1'
    exit 0
fi
payload=$(cat)
if [ "$payload" != "reference-source" ]; then
    printf '%s\n' 'stdin did not contain the qualified source image' >&2
    exit 9
fi
printf '%s\n' '{"ts_recv":"10","hd":{"ts_event":"9","rtype":160,"publisher_id":2,"instrument_id":11667},"action":"A","side":"B","price":"100000000000","size":10,"channel_id":0,"order_id":"77","flags":128,"ts_in_delta":1,"sequence":5}'
"#;
        let executable_path = temp.path().join("fake-dbn");
        fs::write(&executable_path, script).unwrap();
        let authority = ReferenceDecoderAuthorityV1 {
            executable: executable_path.to_str().unwrap().to_owned(),
            version: "fake-dbn 0.20.1".to_owned(),
            sha256: sha256_bytes(script),
            output_encoding: "NDJSON".to_owned(),
            timestamps_and_prices: "raw integer strings".to_owned(),
            source_ordinal_definition:
                "One-based body-record position in reference CLI NDJSON output.".to_owned(),
        };
        let executable = QualifiedReferenceExecutableV1::read_once(&authority).unwrap();
        let first_executable_image = executable.executable_bytes();
        let second_executable_image = executable.executable_bytes();
        assert!(Arc::ptr_eq(
            &first_executable_image,
            &second_executable_image
        ));
        assert_eq!(&*first_executable_image, script);

        // Neither blocker-named path may influence execution after the
        // qualification reads have completed.
        fs::write(&executable_path, b"not-the-qualified-executable").unwrap();
        fs::write(temp.path().join("mbo.dbn.zst"), b"not-the-qualified-source").unwrap();

        let mut records = Vec::new();
        let run = executable
            .decode_mbo_source(&image, |record| {
                records.push(record);
                Ok(())
            })
            .unwrap();
        assert_eq!(
            records,
            vec![RawMboRecordV1 {
                source_ordinal: SourceOrdinal::new(1).unwrap(),
                rtype: 160,
                publisher_id: 2,
                instrument_id: 11_667,
                ts_event: 9,
                order_id: 77,
                price: 100_000_000_000,
                size: 10,
                flags: 128,
                channel_id: 0,
                action: b'A',
                side: b'B',
                ts_recv: 10,
                ts_in_delta: 1,
                sequence: 5,
            }]
        );
        assert_eq!(run.executable_sha256, authority.sha256);
        assert_eq!(run.executable_version, authority.version);
        assert_eq!(run.source_sha256, image.authority().sha256);
        assert_eq!(run.source_schema, XnasSchemaV1::Mbo);
        assert_eq!(run.decoded_body_record_count, 1);
    }

    #[cfg(unix)]
    #[test]
    fn reference_child_failure_outranks_stdin_broken_pipe() {
        let temp = TempDir::new().unwrap();
        let source = vec![b'x'; 8 * 1024 * 1024];
        let manifest = b"exact-manifest";
        fs::write(temp.path().join("mbo.dbn.zst"), &source).unwrap();
        fs::write(temp.path().join("mbp10.dbn.zst"), &source).unwrap();
        fs::write(temp.path().join("manifest.json"), manifest).unwrap();
        let blocker = resolved_fixture(&temp, &source, manifest);
        let image =
            QualifiedSourceImageV1::read_once(blocker.source(BlockerSourceRoleV1::Mbo)).unwrap();

        let script = br#"#!/bin/sh
if [ "$1" = "--version" ]; then
    printf '%s\n' 'failing-dbn 0.20.1'
    exit 0
fi
printf '%s\n' 'authoritative decode failure' >&2
exit 17
"#;
        let executable_path = temp.path().join("failing-dbn");
        fs::write(&executable_path, script).unwrap();
        let authority = ReferenceDecoderAuthorityV1 {
            executable: executable_path.to_str().unwrap().to_owned(),
            version: "failing-dbn 0.20.1".to_owned(),
            sha256: sha256_bytes(script),
            output_encoding: "NDJSON".to_owned(),
            timestamps_and_prices: "raw integer strings".to_owned(),
            source_ordinal_definition:
                "One-based body-record position in reference CLI NDJSON output.".to_owned(),
        };
        let executable = QualifiedReferenceExecutableV1::read_once(&authority).unwrap();
        assert_eq!(
            executable
                .decode_mbo_source(&image, |_| Ok(()))
                .unwrap_err(),
            XnasConformanceError::ReferenceProcess(
                "exit Some(17): authoritative decode failure".to_owned()
            )
        );
    }

    #[test]
    fn reference_ndjson_is_lossless_and_schema_strict() {
        let mbo = r#"{"ts_recv":"10","hd":{"ts_event":"9","rtype":160,"publisher_id":2,"instrument_id":11667},"action":"A","side":"B","price":"100000000000","size":10,"channel_id":3,"order_id":"77","flags":128,"ts_in_delta":-2,"sequence":5}"#;
        let parsed_mbo = parse_reference_mbo_line(SourceOrdinal::new(1).unwrap(), mbo).unwrap();
        assert_eq!(parsed_mbo.channel_id, 3);
        assert_eq!(parsed_mbo.ts_in_delta, -2);
        assert_eq!(parsed_mbo.order_id, 77);

        let level = |index: u32| {
            json!({
                "bid_px": (100_000_000_000_i64 - i64::from(index)).to_string(),
                "ask_px": (100_010_000_000_i64 + i64::from(index)).to_string(),
                "bid_sz": 100 + index,
                "ask_sz": 200 + index,
                "bid_ct": 10 + index,
                "ask_ct": 20 + index
            })
        };
        let mbp = json!({
            "ts_recv": "11",
            "hd": {
                "ts_event": "10",
                "rtype": 10,
                "publisher_id": 2,
                "instrument_id": 11667
            },
            "action": "C",
            "side": "A",
            "depth": 4,
            "price": "100010000000",
            "size": 7,
            "flags": 128,
            "ts_in_delta": 3,
            "sequence": 6,
            "levels": (0..10).map(level).collect::<Vec<_>>()
        })
        .to_string();
        let parsed_mbp = parse_reference_mbp10_line(SourceOrdinal::new(2).unwrap(), &mbp).unwrap();
        assert_eq!(parsed_mbp.source_ordinal, SourceOrdinal::new(2).unwrap());
        assert_eq!(parsed_mbp.depth, 4);
        assert_eq!(parsed_mbp.levels[9].bid_ct, 19);
        assert_eq!(parsed_mbp.levels[9].ask_ct, 29);

        let mut unknown: serde_json::Value = serde_json::from_str(mbo).unwrap();
        unknown["unexpected"] = json!(true);
        assert!(matches!(
            parse_reference_mbo_line(
                SourceOrdinal::new(1).unwrap(),
                &serde_json::to_string(&unknown).unwrap()
            ),
            Err(XnasConformanceError::ReferenceNdjson(_))
        ));

        let mut nine_levels: serde_json::Value = serde_json::from_str(&mbp).unwrap();
        nine_levels["levels"].as_array_mut().unwrap().pop();
        assert_eq!(
            parse_reference_mbp10_line(
                SourceOrdinal::new(1).unwrap(),
                &serde_json::to_string(&nine_levels).unwrap()
            )
            .unwrap_err(),
            XnasConformanceError::ReferenceNdjson(
                "MBP level population is 9, expected 10".to_owned()
            )
        );
    }

    #[test]
    #[ignore = "requires the accepted local reference executable named by the immutable blocker"]
    fn accepted_reference_cli_decodes_synthetic_mbo_and_mbp10_exactly() {
        const START_NS: u64 = 1_751_500_800_000_000_000;
        const END_NS: u64 = START_NS + UTC_DAY_NS;
        let repository = Path::new(env!("CARGO_MANIFEST_DIR"));
        let blocker = resolve_accepted_blocker(repository).unwrap();
        let executable =
            QualifiedReferenceExecutableV1::read_once(blocker.reference_decoder()).unwrap();

        let mbo_records = vec![
            mbo_message(
                START_NS,
                START_NS + 1,
                0,
                DBN_FLAG_BAD_TS_RECV,
                0,
                b'R',
                b'N',
            ),
            mbo_message(START_NS + 2, START_NS + 3, 1, DBN_FLAG_LAST, 1, b'A', b'B'),
        ];
        let (mbo_source, _) = encoded_mbo_source(START_NS, END_NS, 11_667, &mbo_records);
        let mbo_temp = TempDir::new().unwrap();
        fs::write(mbo_temp.path().join("mbo.dbn.zst"), &mbo_source).unwrap();
        fs::write(mbo_temp.path().join("mbp10.dbn.zst"), &mbo_source).unwrap();
        fs::write(mbo_temp.path().join("manifest.json"), b"manifest").unwrap();
        let mbo_fixture =
            resolved_fixture_with_interval(&mbo_temp, &mbo_source, b"manifest", START_NS, END_NS);
        let mbo_image =
            QualifiedSourceImageV1::read_once(mbo_fixture.source(BlockerSourceRoleV1::Mbo))
                .unwrap();
        let mut decoded_mbo = Vec::new();
        let mbo_run = executable
            .decode_mbo_source(&mbo_image, |record| {
                decoded_mbo.push(record);
                Ok(())
            })
            .unwrap();
        assert_eq!(mbo_run.decoded_body_record_count, 2);
        assert_eq!(
            decoded_mbo,
            mbo_records
                .iter()
                .enumerate()
                .map(|(index, record)| {
                    RawMboRecordV1::from_dbn(
                        SourceOrdinal::new(index as u64 + 1).unwrap(),
                        record,
                    )
                })
                .collect::<Vec<_>>()
        );

        let mbp_records = vec![
            mbp10_message(START_NS, START_NS + 1, 1, DBN_FLAG_LAST, b'A', b'B', 0),
            mbp10_message(
                START_NS + 2,
                START_NS + 3,
                2,
                DBN_FLAG_LAST,
                b'C',
                b'A',
                1_000,
            ),
        ];
        let (mbp_source, _) = encoded_mbp10_source(START_NS, END_NS, &mbp_records);
        let mbp_temp = TempDir::new().unwrap();
        fs::write(mbp_temp.path().join("mbo.dbn.zst"), &mbp_source).unwrap();
        fs::write(mbp_temp.path().join("mbp10.dbn.zst"), &mbp_source).unwrap();
        fs::write(mbp_temp.path().join("manifest.json"), b"manifest").unwrap();
        let mbp_fixture =
            resolved_fixture_with_interval(&mbp_temp, &mbp_source, b"manifest", START_NS, END_NS);
        let mbp_image =
            QualifiedSourceImageV1::read_once(mbp_fixture.source(BlockerSourceRoleV1::Mbp10))
                .unwrap();
        let mut decoded_mbp = Vec::new();
        let mbp_run = executable
            .decode_mbp10_source(&mbp_image, |record| {
                decoded_mbp.push(record);
                Ok(())
            })
            .unwrap();
        assert_eq!(mbp_run.decoded_body_record_count, 2);
        assert_eq!(
            decoded_mbp,
            mbp_records
                .iter()
                .enumerate()
                .map(|(index, record)| {
                    RawMbp10RecordV1::from_dbn(
                        SourceOrdinal::new(index as u64 + 1).unwrap(),
                        record,
                    )
                })
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn blocker_digest_and_header_are_fail_closed() {
        let temp = TempDir::new().unwrap();
        let bytes = blocker_bytes(temp.path(), b"source", b"manifest");
        assert_eq!(
            resolve_blocker_bytes(bytes.clone(), &"0".repeat(64)).unwrap_err(),
            XnasConformanceError::AuthorityDigestMismatch
        );

        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value["status"] = json!("PASS");
        let mutated = serde_json::to_vec(&value).unwrap();
        let digest = sha256_bytes(&mutated);
        assert_eq!(
            resolve_blocker_bytes(mutated, &digest).unwrap_err(),
            XnasConformanceError::AuthorityInvariant("blocker header".to_owned())
        );
    }

    #[test]
    fn authority_paths_reject_parent_traversal() {
        assert!(matches!(
            join_authority_path("/absolute/root", "data/../escape"),
            Err(XnasConformanceError::UnsafeAuthorityPath(_))
        ));
        assert!(matches!(
            join_authority_path("relative/root", "data/source"),
            Err(XnasConformanceError::UnsafeAuthorityPath(_))
        ));
    }

    #[test]
    fn exact_source_and_manifest_images_are_read_once_and_retained() {
        let temp = TempDir::new().unwrap();
        let source = b"exact-source";
        let manifest = b"exact-manifest";
        fs::write(temp.path().join("mbo.dbn.zst"), source).unwrap();
        fs::write(temp.path().join("mbp10.dbn.zst"), source).unwrap();
        fs::write(temp.path().join("manifest.json"), manifest).unwrap();
        let blocker = resolved_fixture(&temp, source, manifest);

        let image =
            QualifiedSourceImageV1::read_once(blocker.source(BlockerSourceRoleV1::Mbo)).unwrap();
        let first = image.source_bytes();
        let second = image.source_bytes();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(&*first, source);
        assert_eq!(&*image.manifest_bytes(), manifest);

        fs::write(temp.path().join("mbo.dbn.zst"), b"later-path-mutation").unwrap();
        assert_eq!(&*image.source_bytes(), source);
    }

    #[test]
    fn source_or_manifest_mismatch_never_yields_an_image() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("mbo.dbn.zst"), b"wrong").unwrap();
        fs::write(temp.path().join("mbp10.dbn.zst"), b"source").unwrap();
        fs::write(temp.path().join("manifest.json"), b"manifest").unwrap();
        let blocker = resolved_fixture(&temp, b"source", b"manifest");
        assert_eq!(
            QualifiedSourceImageV1::read_once(blocker.source(BlockerSourceRoleV1::Mbo))
                .unwrap_err(),
            XnasConformanceError::SourceEvidenceMismatch("source".to_owned())
        );

        fs::write(temp.path().join("mbo.dbn.zst"), b"source").unwrap();
        fs::write(temp.path().join("manifest.json"), b"wrong").unwrap();
        assert_eq!(
            QualifiedSourceImageV1::read_once(blocker.source(BlockerSourceRoleV1::Mbo))
                .unwrap_err(),
            XnasConformanceError::SourceEvidenceMismatch("manifest".to_owned())
        );
    }

    #[test]
    fn blocker_fields_are_consumed_without_retyping_evidence() {
        let temp = TempDir::new().unwrap();
        let bytes = blocker_bytes(temp.path(), b"source", b"manifest");
        let digest = sha256_bytes(&bytes);
        let blocker = resolve_blocker_bytes(bytes.clone(), &digest).unwrap();
        assert_eq!(&*blocker.bytes(), bytes.as_slice());
        assert_eq!(blocker.sha256(), digest);
        assert_eq!(
            blocker.source(BlockerSourceRoleV1::Mbo).expected_identity(),
            Some(XnasIdentityV1::new(2, 11667))
        );
        assert_eq!(
            blocker.source(BlockerSourceRoleV1::Mbp10).schema(),
            XnasSchemaV1::Mbp10
        );
        assert_eq!(
            blocker.full_source_file_measurements(),
            &json!({"retained": true})
        );
        assert_eq!(blocker.reference_decoder().version(), "dbn-cli 0.20.1");
    }

    #[test]
    fn verified_manifest_derives_exact_february_source_receipt() {
        let temp = TempDir::new().unwrap();
        let feb_source = b"feb-source";
        let manifest = serde_json::to_vec(&json!({
            "job_id": "pinned-job",
            "files": [{
                "filename": "xnas-itch-20250203.mbo.dbn.zst",
                "size": feb_source.len(),
                "hash": format!("sha256:{}", sha256_bytes(feb_source))
            }]
        }))
        .unwrap();
        fs::write(temp.path().join("mbo.dbn.zst"), b"july-source").unwrap();
        fs::write(temp.path().join("mbp10.dbn.zst"), b"july-source").unwrap();
        fs::write(temp.path().join("manifest.json"), &manifest).unwrap();
        fs::write(
            temp.path().join("xnas-itch-20250203.mbo.dbn.zst"),
            feb_source,
        )
        .unwrap();
        let blocker = resolved_fixture(&temp, b"july-source", &manifest);
        let july =
            QualifiedSourceImageV1::read_once(blocker.source(BlockerSourceRoleV1::Mbo)).unwrap();

        let feb = july
            .derive_and_read_manifest_mbo_source("20250203")
            .unwrap();
        assert_eq!(
            feb.authority().query(),
            &SourceQueryAuthorityV1::ManifestSessionDate {
                yyyymmdd: "20250203".to_owned()
            }
        );
        assert_eq!(feb.authority().expected_identity(), None);
        assert_eq!(&*feb.source_bytes(), feb_source);
        assert!(Arc::ptr_eq(&july.manifest_bytes(), &feb.manifest_bytes()));
    }

    #[test]
    fn manifest_derived_source_binds_its_own_metadata_instrument() {
        const START_NS: u64 = 1_738_540_800_000_000_000;
        const END_NS: u64 = START_NS + UTC_DAY_NS;
        const FEB_INSTRUMENT_ID: u32 = 22_222;

        let temp = TempDir::new().unwrap();
        let records = vec![
            mbo_message_for_instrument(
                FEB_INSTRUMENT_ID,
                START_NS,
                START_NS + 1,
                0,
                DBN_FLAG_BAD_TS_RECV,
                0,
                b'R',
                b'N',
            ),
            mbo_message_for_instrument(
                FEB_INSTRUMENT_ID,
                START_NS + 2,
                START_NS + 2,
                1,
                DBN_FLAG_LAST,
                1,
                b'A',
                b'B',
            ),
        ];
        let (feb_source, session_date) =
            encoded_mbo_source(START_NS, END_NS, FEB_INSTRUMENT_ID, &records);
        assert_eq!(session_date, "20250203");
        let manifest = serde_json::to_vec(&json!({
            "files": [{
                "filename": "xnas-itch-20250203.mbo.dbn.zst",
                "size": feb_source.len(),
                "hash": format!("sha256:{}", sha256_bytes(&feb_source))
            }]
        }))
        .unwrap();
        fs::write(temp.path().join("mbo.dbn.zst"), b"july-source").unwrap();
        fs::write(temp.path().join("mbp10.dbn.zst"), b"july-source").unwrap();
        fs::write(temp.path().join("manifest.json"), &manifest).unwrap();
        fs::write(
            temp.path().join("xnas-itch-20250203.mbo.dbn.zst"),
            &feb_source,
        )
        .unwrap();
        let blocker = resolved_fixture(&temp, b"july-source", &manifest);
        let july =
            QualifiedSourceImageV1::read_once(blocker.source(BlockerSourceRoleV1::Mbo)).unwrap();
        let feb = july
            .derive_and_read_manifest_mbo_source("20250203")
            .unwrap();
        let session =
            XnasSessionScopeV1::new(session_date, START_NS + 1, END_NS - 1, "c".repeat(64))
                .unwrap();
        let cursor = XnasMboCausalCursorV1::open(feb, session).unwrap();

        assert_eq!(
            cursor.metadata.identity,
            XnasIdentityV1::new(XNAS_ITCH_PUBLISHER_ID, FEB_INSTRUMENT_ID)
        );
    }

    #[test]
    fn causal_cursor_consumes_all_ties_and_holds_first_lifting_record() {
        let records = vec![
            mbo_message(1, 999, 0, DBN_FLAG_BAD_TS_RECV, 0, b'R', b'N'),
            mbo_message(2, 10, 1, DBN_FLAG_LAST, 1, b'A', b'B'),
            mbo_message(3, 10, 2, DBN_FLAG_LAST, 2, b'A', b'B'),
            mbo_message(4, 11, 3, DBN_FLAG_LAST, 3, b'A', b'B'),
            // This t-stamped record is hidden behind ordinal 4 and must not
            // be decoded or observed at t.
            mbo_message(5, 10, 4, DBN_FLAG_LAST, 4, b'A', b'B'),
        ];
        let (_temp, mut cursor) = cursor_fixture(&records, 1, 100);
        let source = cursor.source_bytes();
        assert!(!source.is_empty());

        let observation = cursor.midpoint_at(10).unwrap();
        assert_eq!(observation.consumed_prefix_ordinal, 3);
        assert_eq!(observation.observed_watermark_ns, Some(10));
        assert_eq!(
            observation.held_source_ordinal,
            SourceOrdinal::new(4).unwrap()
        );
        assert_eq!(observation.held_watermark_contribution_ns, 11);
        assert_eq!(observation.projected_watermark_ns, 11);
        assert!(observation.midpoint.is_none());
        assert_eq!(cursor.decoded_body_record_count, 4);
    }

    #[test]
    fn cursor_cutoff_is_invariant_to_payload_beyond_the_held_prefix() {
        let prefix = [
            mbo_message(1, 999, 0, DBN_FLAG_BAD_TS_RECV, 0, b'R', b'N'),
            mbo_message(2, 10, 1, DBN_FLAG_LAST, 1, b'A', b'B'),
            mbo_message(3, 10, 2, DBN_FLAG_LAST, 2, b'A', b'B'),
        ];
        let mut first_records = prefix.to_vec();
        first_records.push(mbo_message(4, 11, 3, DBN_FLAG_LAST, 3, b'A', b'B'));
        let mut second_records = prefix.to_vec();
        second_records.push(mbo_message(40, 11, 300, 0, 999, b'C', b'A'));

        let (_first_temp, mut first) = cursor_fixture(&first_records, 1, 100);
        let (_second_temp, mut second) = cursor_fixture(&second_records, 1, 100);
        assert_eq!(
            first.midpoint_at(10).unwrap(),
            second.midpoint_at(10).unwrap()
        );
    }

    #[test]
    fn cursor_rejects_nonincreasing_or_out_of_interval_decisions_before_mutation() {
        let records = vec![
            mbo_message(1, 999, 0, DBN_FLAG_BAD_TS_RECV, 0, b'R', b'N'),
            mbo_message(2, 10, 1, DBN_FLAG_LAST, 1, b'A', b'B'),
            mbo_message(3, 11, 2, DBN_FLAG_LAST, 2, b'A', b'B'),
        ];
        let (_temp, mut cursor) = cursor_fixture(&records, 1, 100);
        assert_eq!(
            cursor.midpoint_at(0).unwrap_err(),
            XnasConformanceError::Session("decision outside qualified RTH interval".to_owned())
        );
        cursor.midpoint_at(10).unwrap();
        assert_eq!(
            cursor.midpoint_at(10).unwrap_err(),
            XnasConformanceError::Semantics(
                XnasSemanticsError::DecisionTimeNotStrictlyIncreasing {
                    previous: 10,
                    observed: 10
                }
            )
        );
        assert_eq!(cursor.consumed_body_record_count, 2);
    }

    #[test]
    fn all_scope_rejection_is_sticky_and_preserves_terminal_audit() {
        let records = vec![
            mbo_message(1, 999, 0, DBN_FLAG_BAD_TS_RECV, 0, b'R', b'N'),
            mbo_message_for_instrument(99_999, 2, 10, 1, DBN_FLAG_LAST, 1, b'A', b'B'),
            mbo_message(3, 11, 2, DBN_FLAG_LAST, 2, b'A', b'B'),
        ];
        let (_temp, mut cursor) = cursor_fixture(&records, 1, 100);
        let expected = XnasConformanceError::Semantics(XnasSemanticsError::UnexpectedIdentity {
            publisher_id: XNAS_ITCH_PUBLISHER_ID,
            instrument_id: 99_999,
        });

        assert_eq!(cursor.midpoint_at(10).unwrap_err(), expected);
        assert_eq!(cursor.midpoint_at(11).unwrap_err(), expected);
        let scan = cursor.finish_source_scan().unwrap();
        assert_eq!(scan.terminal_cursor_error, Some(expected));
        assert!(!scan.decoder_eof);
        assert!(scan.semantics.counts.population_reconciles());
    }

    #[test]
    fn midpoint_eof_is_sticky_and_never_becomes_a_witness() {
        let records = vec![
            mbo_message(1, 999, 0, DBN_FLAG_BAD_TS_RECV, 0, b'R', b'N'),
            mbo_message(2, 10, 1, DBN_FLAG_LAST, 1, b'A', b'B'),
            mbo_message(3, 11, 2, DBN_FLAG_LAST, 2, b'A', b'B'),
        ];
        let (_temp, mut cursor) = cursor_fixture(&records, 1, 100);
        let expected = XnasConformanceError::DecisionPrefixEndedAtEof { decision_ns: 11 };

        assert_eq!(cursor.midpoint_at(11).unwrap_err(), expected);
        assert_eq!(cursor.midpoint_at(12).unwrap_err(), expected);
        let scan = cursor.finish_source_scan().unwrap();
        assert!(scan.decoder_eof);
        assert_eq!(scan.terminal_cursor_error, Some(expected));
        assert_eq!(
            scan.semantics
                .counts
                .quarantined_by_reason
                .get("TERMINAL_AT_EOF")
                .unwrap()
                .record_count,
            1
        );
        assert!(scan.semantics.counts.population_reconciles());
    }

    #[test]
    fn close_equality_emits_then_atomically_seals_the_session() {
        let records = vec![
            mbo_message(1, 999, 0, DBN_FLAG_BAD_TS_RECV, 0, b'R', b'N'),
            mbo_message(2, 10, 1, DBN_FLAG_LAST, 1, b'A', b'B'),
            mbo_message(3, 11, 2, DBN_FLAG_LAST, 2, b'A', b'B'),
            mbo_message(4, 101, 3, DBN_FLAG_LAST, 3, b'A', b'B'),
        ];
        let (_temp, mut cursor) = cursor_fixture_with_scope(&records, 1, 200, 1, 100);

        let close = cursor.midpoint_at(100).unwrap();
        assert_eq!(close.held_source_ordinal, SourceOrdinal::new(4).unwrap());
        assert_eq!(close.held_watermark_contribution_ns, 101);
        assert_eq!(
            cursor.midpoint_at(101).unwrap_err(),
            XnasConformanceError::SessionClosed { rth_close_ns: 100 }
        );
        let seal = cursor.seal_session().unwrap();
        assert_eq!(seal.session.rth_close_ns, 100);
        assert_eq!(seal.decoded_body_record_count, 4);
        assert_eq!(seal.consumed_body_record_count, 3);
        assert_eq!(seal.held_source_ordinal, SourceOrdinal::new(4).unwrap());
        assert_eq!(
            seal.semantics
                .counts
                .quarantined_by_reason
                .get("SESSION_BOUNDARY")
                .unwrap()
                .record_count,
            1
        );
        assert!(seal.semantics.counts.population_reconciles());
    }

    #[test]
    fn mbp_cursor_owns_n_t_and_excludes_witness_payload_from_endpoint() {
        let records = vec![
            mbp10_message(1, 10, 1, DBN_FLAG_LAST, b'A', b'B', 0),
            mbp10_message(2, 11, 2, DBN_FLAG_LAST, b'A', b'B', 1_000),
            mbp10_message(3, 12, 3, DBN_FLAG_LAST, b'A', b'B', 2_000),
        ];
        let expected_levels =
            RawMbp10RecordV1::from_dbn(SourceOrdinal::new(1).unwrap(), &records[0]).levels;
        let (_temp, mut cursor) = mbp10_cursor_fixture_with_scope(&records, 1, 100, 1, 99);

        let observation = cursor.observe_prefix_at(11).unwrap();
        assert_eq!(observation.consumed_prefix_ordinal, 2);
        assert_eq!(observation.observed_watermark_ns, Some(11));
        assert_eq!(
            observation.held_source_ordinal,
            SourceOrdinal::new(3).unwrap()
        );
        assert_eq!(observation.held_watermark_contribution_ns, 12);
        assert_eq!(observation.projected_watermark_ns, 12);
        assert_eq!(observation.completed_endpoint_count, 1);
        assert_eq!(cursor.endpoints[0].witness_source_ordinal.get(), 2);
        assert_eq!(cursor.endpoints[0].levels, expected_levels);
        assert_eq!(cursor.decoded_body_record_count, 3);
    }

    #[test]
    fn mbp_cutoff_is_invariant_to_held_future_payload() {
        let prefix = [
            mbp10_message(1, 10, 1, DBN_FLAG_LAST, b'A', b'B', 0),
            mbp10_message(2, 11, 2, DBN_FLAG_LAST, b'A', b'B', 1_000),
        ];
        let mut first_records = prefix.to_vec();
        first_records.push(mbp10_message(3, 12, 3, DBN_FLAG_LAST, b'A', b'B', 2_000));
        let mut second_records = prefix.to_vec();
        second_records.push(mbp10_message(300, 12, 300, 0, b'C', b'A', 999_000));
        let (_first_temp, mut first) =
            mbp10_cursor_fixture_with_scope(&first_records, 1, 100, 1, 99);
        let (_second_temp, mut second) =
            mbp10_cursor_fixture_with_scope(&second_records, 1, 100, 1, 99);

        assert_eq!(
            first.observe_prefix_at(11).unwrap(),
            second.observe_prefix_at(11).unwrap()
        );
        assert_eq!(first.endpoints, second.endpoints);
    }

    #[test]
    fn mbp_identity_quarantine_can_recover_without_breaking_source_cursor() {
        let records = vec![
            mbp10_message(1, 10, 1, DBN_FLAG_MAYBE_BAD_BOOK, b'A', b'B', 0),
            mbp10_message(2, 11, 2, DBN_FLAG_LAST, b'R', b'N', 1_000),
            mbp10_message(3, 12, 3, DBN_FLAG_LAST, b'A', b'B', 2_000),
            mbp10_message(4, 13, 4, DBN_FLAG_LAST, b'A', b'B', 3_000),
        ];
        let (_temp, mut cursor) = mbp10_cursor_fixture_with_scope(&records, 1, 100, 1, 99);

        let observation = cursor.observe_prefix_at(12).unwrap();
        assert_eq!(observation.completed_endpoint_count, 1);
        assert_eq!(cursor.semantic_rejections.len(), 1);
        assert_eq!(
            cursor.semantic_rejections[0].error,
            XnasSemanticsError::MaybeBadBook
        );
        assert!(cursor.terminal_cursor_error.is_none());
    }

    #[test]
    fn mbp_source_scan_and_session_seal_are_distinct() {
        let records = vec![
            mbp10_message(1, 10, 1, DBN_FLAG_LAST, b'A', b'B', 0),
            mbp10_message(2, 11, 2, DBN_FLAG_LAST, b'A', b'B', 1_000),
            mbp10_message(3, 12, 3, DBN_FLAG_LAST, b'A', b'B', 2_000),
        ];
        let (_scan_temp, scan_cursor) = mbp10_cursor_fixture_with_scope(&records, 1, 100, 1, 99);
        let scan = scan_cursor.finish_source_scan().unwrap();
        assert!(scan.decoder_eof);
        assert_eq!(scan.decoded_body_record_count, 3);
        assert_eq!(scan.consumed_body_record_count, 3);
        assert_eq!(scan.endpoints.len(), 2);
        assert_eq!(scan.terminal_cursor_error, None);
        assert!(scan.semantics.counts.population_reconciles());

        let (_seal_temp, mut session_cursor) =
            mbp10_cursor_fixture_with_scope(&records, 1, 100, 1, 11);
        let close = session_cursor.observe_prefix_at(11).unwrap();
        assert_eq!(close.completed_endpoint_count, 1);
        assert_eq!(
            session_cursor.observe_prefix_at(12).unwrap_err(),
            XnasConformanceError::SessionClosed { rth_close_ns: 11 }
        );
        let seal = session_cursor.seal_session().unwrap();
        assert_eq!(seal.consumed_body_record_count, 2);
        assert_eq!(seal.held_source_ordinal, SourceOrdinal::new(3).unwrap());
        assert_eq!(seal.endpoints.len(), 1);
        assert!(seal.semantics.counts.population_reconciles());
    }

    #[test]
    fn watermark_helper_excludes_only_initial_or_bad_clocks() {
        let initial = RawMboRecordV1::from_dbn(
            SourceOrdinal::new(1).unwrap(),
            &mbo_message(1, 999, 0, DBN_FLAG_BAD_TS_RECV, 0, b'R', b'N'),
        );
        let bad = RawMboRecordV1::from_dbn(
            SourceOrdinal::new(2).unwrap(),
            &mbo_message(2, 55, 1, DBN_FLAG_BAD_TS_RECV, 1, b'A', b'B'),
        );
        let maybe_bad = RawMboRecordV1::from_dbn(
            SourceOrdinal::new(3).unwrap(),
            &mbo_message(3, 77, 2, DBN_FLAG_MAYBE_BAD_BOOK, 2, b'A', b'B'),
        );
        assert_eq!(xnas_mbo_watermark_contribution(&initial), None);
        assert_eq!(xnas_mbo_watermark_contribution(&bad), None);
        assert_eq!(xnas_mbo_watermark_contribution(&maybe_bad), Some(77));
    }

    #[test]
    fn cursor_finish_decodes_every_record_but_never_uses_eof_as_witness() {
        let records = vec![
            mbo_message(1, 999, 0, DBN_FLAG_BAD_TS_RECV, 0, b'R', b'N'),
            mbo_message(2, 10, 1, DBN_FLAG_LAST, 1, b'A', b'B'),
            mbo_message(3, 11, 2, DBN_FLAG_LAST, 2, b'A', b'B'),
        ];
        let (_temp, cursor) = cursor_fixture(&records, 1, 100);
        let finish = cursor.finish_source_scan().unwrap();
        assert!(finish.decoder_eof);
        assert_eq!(finish.decoded_body_record_count, 3);
        assert_eq!(finish.consumed_body_record_count, 3);
        assert_eq!(
            finish
                .semantics
                .counts
                .quarantined_by_reason
                .get("TERMINAL_AT_EOF")
                .unwrap()
                .record_count,
            1
        );
        assert!(finish.semantics.counts.population_reconciles());
    }
}
