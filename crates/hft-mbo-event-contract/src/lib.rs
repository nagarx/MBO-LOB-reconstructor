//! Lossless canonical MBO event and typed disposition contract.
//!
//! This crate is intentionally lightweight. It owns raw event identity,
//! structural validation, and responsibility-specific dispositions. It does
//! not decode DBN, mutate a book, sample observations, or calculate features.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use thiserror::Error;

/// Descriptor generated from the authoritative raw-field table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawEventFieldDescriptor {
    pub name: &'static str,
    pub rust_type: &'static str,
    pub unit: &'static str,
    pub origin: &'static str,
    pub clock: Option<&'static str>,
    pub wire_encoding: Option<&'static str>,
}

/// Build-time admitted catalog-release authority.
///
/// The root contract plane owns these values. Runtime source selection must
/// prove exact membership in the corresponding SHA-256-pinned custody
/// projection before any market-data object is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcceptedCatalogReleaseBindingDescriptorV1 {
    pub release_id: &'static str,
    pub storage_root_id: &'static str,
    pub custody_projection_schema: &'static str,
    pub custody_projection_file_sha256: &'static str,
    pub custody_projection_content_sha256: &'static str,
    pub canonical_profile_sha256: &'static str,
    pub embedded_per_file_tsv_sha256: &'static str,
    pub evidence_manifest_sha256: &'static str,
    pub terminal_validation_receipt_sha256: &'static str,
    pub terminal_validation_status: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/mbo_event_contract_generated.rs"));

/// Exact SHA-256 bytes for a logical source object or receipt.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sha256DigestV1([u8; 32]);

impl Sha256DigestV1 {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub const fn is_zero(&self) -> bool {
        let mut index = 0;
        while index < self.0.len() {
            if self.0[index] != 0 {
                return false;
            }
            index += 1;
        }
        true
    }

    pub fn from_hex(value: &str) -> Result<Self, DigestParseErrorV1> {
        if value.len() != 64 {
            return Err(DigestParseErrorV1::WrongLength(value.len()));
        }
        let mut bytes = [0_u8; 32];
        for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
            let high = decode_lower_hex(pair[0]).ok_or(DigestParseErrorV1::InvalidHex {
                index: index * 2,
                byte: pair[0],
            })?;
            let low = decode_lower_hex(pair[1]).ok_or(DigestParseErrorV1::InvalidHex {
                index: index * 2 + 1,
                byte: pair[1],
            })?;
            bytes[index] = (high << 4) | low;
        }
        Ok(Self(bytes))
    }

    pub fn to_hex(self) -> String {
        let mut result = String::with_capacity(64);
        for byte in self.0 {
            use std::fmt::Write as _;
            write!(&mut result, "{byte:02x}").expect("write into String");
        }
        result
    }
}

fn decode_lower_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

impl fmt::Debug for Sha256DigestV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Sha256DigestV1")
            .field(&self.to_hex())
            .finish()
    }
}

impl fmt::Display for Sha256DigestV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl Serialize for Sha256DigestV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Sha256DigestV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(&value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DigestParseErrorV1 {
    #[error("SHA-256 must contain 64 lowercase hexadecimal characters, found {0}")]
    WrongLength(usize),
    #[error("invalid lowercase hexadecimal byte {byte:#04x} at index {index}")]
    InvalidHex { index: usize, byte: u8 },
}

/// Catalog-release and exact object identity whose row identity remains stable.
///
/// Every authority coordinate is copied from a SHA-256-pinned custody
/// projection. `relative_path`, not an absolute filesystem path, participates
/// in the stable object key `(release_id, relative_path, compressed_sha256)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogicalSourceV1 {
    pub catalog_release_id: String,
    pub catalog_storage_root_id: String,
    pub custody_projection_schema: String,
    pub custody_projection_file_sha256: Sha256DigestV1,
    pub custody_projection_content_sha256: Sha256DigestV1,
    pub canonical_profile_sha256: Sha256DigestV1,
    pub embedded_per_file_tsv_sha256: Sha256DigestV1,
    pub evidence_manifest_sha256: Sha256DigestV1,
    pub terminal_validation_receipt_sha256: Sha256DigestV1,
    pub terminal_validation_status: String,
    pub relative_path: String,
    pub compressed_sha256: Sha256DigestV1,
    pub compressed_bytes: u64,
    pub expected_records: u64,
    pub metadata_start_ns: u64,
    pub metadata_end_ns: u64,
    pub requested_symbols_preview: String,
    pub requested_symbols_sha256: Sha256DigestV1,
    pub symbols_n: u64,
    pub active_instruments_n: u64,
    pub provenance_tier: String,
    pub provider_manifest_relative_path: String,
    pub provider_manifest_sha256: Sha256DigestV1,
    pub provider_job_id: String,
    pub provider_declared_data_file_count: u64,
    pub dbn_version: u8,
    pub dbn_ts_out: bool,
    pub dataset: String,
    pub schema: String,
}

/// Representation model reserved across versions.
///
/// Strict v1 accepts only `CanonicalObject`. `DerivedReplica` is a future
/// receipt carrier and is not accepted merely because it contains a digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OpenedRepresentationV1 {
    CanonicalObject,
    DerivedReplica {
        derivation_receipt_sha256: Sha256DigestV1,
    },
}

/// The exact bytes, derived locator, and runtime file identity opened by the decoder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenedReplicaV1 {
    pub custody_projection_path: String,
    pub storage_root_path: String,
    pub relative_path: String,
    pub opened_path: String,
    pub representation: OpenedRepresentationV1,
    pub opened_sha256: Sha256DigestV1,
    pub opened_bytes: u64,
    pub device_id: u64,
    pub inode: u64,
    pub metadata_bytes: u64,
    pub modified_seconds: i64,
    pub modified_nanoseconds: i64,
    pub changed_seconds: i64,
    pub changed_nanoseconds: i64,
}

/// Source identity carried once per stream and bound into every row identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceDescriptorV1 {
    pub logical: LogicalSourceV1,
    pub opened: OpenedReplicaV1,
}

impl LogicalSourceV1 {
    /// Validate the complete external catalog authority before filesystem I/O.
    pub fn validate_strict(&self) -> Result<(), SourceIdentityErrorV1> {
        for (field, value) in [
            ("catalog_release_id", self.catalog_release_id.as_str()),
            (
                "catalog_storage_root_id",
                self.catalog_storage_root_id.as_str(),
            ),
            (
                "custody_projection_schema",
                self.custody_projection_schema.as_str(),
            ),
            (
                "terminal_validation_status",
                self.terminal_validation_status.as_str(),
            ),
            ("relative_path", self.relative_path.as_str()),
            (
                "requested_symbols_preview",
                self.requested_symbols_preview.as_str(),
            ),
            ("provenance_tier", self.provenance_tier.as_str()),
            (
                "provider_manifest_relative_path",
                self.provider_manifest_relative_path.as_str(),
            ),
            ("provider_job_id", self.provider_job_id.as_str()),
            ("dataset", self.dataset.as_str()),
            ("schema", self.schema.as_str()),
        ] {
            if value.is_empty() {
                return Err(SourceIdentityErrorV1::EmptyField(field));
            }
        }
        if self.schema != "mbo" {
            return Err(SourceIdentityErrorV1::WrongSchema(self.schema.clone()));
        }
        if !(1..=3).contains(&self.dbn_version) {
            return Err(SourceIdentityErrorV1::UnsupportedDbnVersion(
                self.dbn_version,
            ));
        }
        if self.dbn_ts_out {
            return Err(SourceIdentityErrorV1::TsOutUnsupported);
        }
        if self.compressed_bytes == 0 {
            return Err(SourceIdentityErrorV1::EmptyObject);
        }
        if self.custody_projection_file_sha256.is_zero()
            || self.custody_projection_content_sha256.is_zero()
            || self.canonical_profile_sha256.is_zero()
            || self.embedded_per_file_tsv_sha256.is_zero()
            || self.evidence_manifest_sha256.is_zero()
            || self.terminal_validation_receipt_sha256.is_zero()
            || self.compressed_sha256.is_zero()
            || self.requested_symbols_sha256.is_zero()
            || self.provider_manifest_sha256.is_zero()
        {
            return Err(SourceIdentityErrorV1::PlaceholderDigest);
        }
        if self.terminal_validation_status != "PASS" {
            return Err(SourceIdentityErrorV1::CatalogValidationNotPassed(
                self.terminal_validation_status.clone(),
            ));
        }
        if self.metadata_start_ns >= self.metadata_end_ns {
            return Err(SourceIdentityErrorV1::InvalidMetadataBounds {
                start_ns: self.metadata_start_ns,
                end_ns: self.metadata_end_ns,
            });
        }
        if self.symbols_n == 0
            || self.active_instruments_n == 0
            || self.active_instruments_n > self.symbols_n
            || self.provider_declared_data_file_count == 0
        {
            return Err(SourceIdentityErrorV1::InvalidPopulationIdentity);
        }
        validate_catalog_relative_path_v1(&self.relative_path)?;
        validate_catalog_relative_path_v1(&self.provider_manifest_relative_path)?;
        Ok(())
    }
}

impl SourceDescriptorV1 {
    /// Strict v1 accepts only the catalog-selected canonical byte object itself.
    pub fn validate_strict(&self) -> Result<(), SourceIdentityErrorV1> {
        self.logical.validate_strict()?;
        for (field, value) in [
            (
                "custody_projection_path",
                self.opened.custody_projection_path.as_str(),
            ),
            ("storage_root_path", self.opened.storage_root_path.as_str()),
            ("opened_relative_path", self.opened.relative_path.as_str()),
            ("opened_path", self.opened.opened_path.as_str()),
        ] {
            if value.is_empty() {
                return Err(SourceIdentityErrorV1::EmptyField(field));
            }
        }
        if self.opened.opened_bytes == 0 || self.opened.opened_sha256.is_zero() {
            return Err(SourceIdentityErrorV1::EmptyObject);
        }
        if !matches!(
            self.opened.representation,
            OpenedRepresentationV1::CanonicalObject
        ) {
            return Err(SourceIdentityErrorV1::DerivedReplicaNotAllowed);
        }
        let expected_path =
            std::path::Path::new(&self.opened.storage_root_path).join(&self.logical.relative_path);
        if self.opened.relative_path != self.logical.relative_path
            || expected_path.to_str() != Some(self.opened.opened_path.as_str())
        {
            return Err(SourceIdentityErrorV1::PathSubstitution {
                expected: expected_path.to_string_lossy().into_owned(),
                opened: self.opened.opened_path.clone(),
            });
        }
        if self.opened.opened_sha256 != self.logical.compressed_sha256 {
            return Err(SourceIdentityErrorV1::DigestMismatch {
                expected: self.logical.compressed_sha256,
                opened: self.opened.opened_sha256,
            });
        }
        if self.opened.opened_bytes != self.logical.compressed_bytes
            || self.opened.metadata_bytes != self.logical.compressed_bytes
        {
            return Err(SourceIdentityErrorV1::ByteLengthMismatch {
                expected: self.logical.compressed_bytes,
                opened: self.opened.opened_bytes,
            });
        }
        Ok(())
    }
}

/// Require an externally supplied source to match one release admitted by the
/// root event-contract authority. Low-level synthetic tests may validate a
/// self-consistent projection without this admission; every production probe
/// calls this function before opening bytes.
pub fn validate_accepted_catalog_release_v1(
    logical: &LogicalSourceV1,
) -> Result<(), SourceIdentityErrorV1> {
    logical.validate_strict()?;
    let accepted = ACCEPTED_CATALOG_RELEASE_BINDINGS_V1.iter().any(|binding| {
        binding.release_id == logical.catalog_release_id
            && binding.storage_root_id == logical.catalog_storage_root_id
            && binding.custody_projection_schema == logical.custody_projection_schema
            && binding.custody_projection_file_sha256
                == logical.custody_projection_file_sha256.to_hex()
            && binding.custody_projection_content_sha256
                == logical.custody_projection_content_sha256.to_hex()
            && binding.canonical_profile_sha256 == logical.canonical_profile_sha256.to_hex()
            && binding.embedded_per_file_tsv_sha256 == logical.embedded_per_file_tsv_sha256.to_hex()
            && binding.evidence_manifest_sha256 == logical.evidence_manifest_sha256.to_hex()
            && binding.terminal_validation_receipt_sha256
                == logical.terminal_validation_receipt_sha256.to_hex()
            && binding.terminal_validation_status == logical.terminal_validation_status
    });
    if !accepted {
        return Err(SourceIdentityErrorV1::CatalogReleaseNotAccepted(
            logical.catalog_release_id.clone(),
        ));
    }
    Ok(())
}

/// Reject every path spelling that could escape or ambiguously alias the
/// catalog's portable POSIX-relative object coordinate.
pub fn validate_catalog_relative_path_v1(value: &str) -> Result<(), SourceIdentityErrorV1> {
    if value.is_empty()
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains('\\')
        || value.bytes().any(|byte| !(0x21..=0x7e).contains(&byte))
        || value
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(SourceIdentityErrorV1::InvalidRelativePath(value.to_owned()));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SourceIdentityErrorV1 {
    #[error("source identity field {0} is empty")]
    EmptyField(&'static str),
    #[error("strict canonical MBO source schema must be mbo, found {0}")]
    WrongSchema(String),
    #[error("unsupported DBN version {0}; v1 supports versions 1 through 3")]
    UnsupportedDbnVersion(u8),
    #[error("canonical MBO event v1 rejects DBN metadata with ts_out=true")]
    TsOutUnsupported,
    #[error("catalog terminal validation status must be PASS, found {0}")]
    CatalogValidationNotPassed(String),
    #[error("catalog release authority is not accepted by this event-contract build: {0}")]
    CatalogReleaseNotAccepted(String),
    #[error("invalid catalog-relative path {0:?}")]
    InvalidRelativePath(String),
    #[error("invalid catalog metadata bounds [{start_ns}, {end_ns})")]
    InvalidMetadataBounds { start_ns: u64, end_ns: u64 },
    #[error("catalog symbol/provider population identity is empty or inconsistent")]
    InvalidPopulationIdentity,
    #[error("source object must not be empty")]
    EmptyObject,
    #[error("catalog, source, and opened SHA-256 values must not be all-zero placeholders")]
    PlaceholderDigest,
    #[error("strict canonical MBO event v1 rejects every derived replica")]
    DerivedReplicaNotAllowed,
    #[error("strict source path substitution: expected={expected}, opened={opened}")]
    PathSubstitution { expected: String, opened: String },
    #[error("opened source digest mismatch: expected={expected}, opened={opened}")]
    DigestMismatch {
        expected: Sha256DigestV1,
        opened: Sha256DigestV1,
    },
    #[error("opened source length mismatch: expected={expected}, opened={opened}")]
    ByteLengthMismatch { expected: u64, opened: u64 },
}

/// One-to-one, lossless projection of a successfully decoded DBN MBO record.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RawMboEventV1 {
    pub source_object_sha256: Sha256DigestV1,
    pub raw_ordinal: u64,
    pub subordinal: u16,
    pub rtype: u8,
    pub record_size_bytes: u16,
    pub publisher_id: u16,
    pub instrument_id: u32,
    pub ts_event: u64,
    pub ts_recv: u64,
    pub ts_in_delta: i32,
    pub channel_id: u8,
    pub sequence: u32,
    pub order_id: u64,
    pub price_raw: i64,
    pub size_raw: u32,
    pub flags_raw: u8,
    pub action_raw: u8,
    pub side_raw: u8,
}

impl RawMboEventV1 {
    pub const fn price_state(&self) -> FixedPriceStateV1 {
        if self.price_raw == UNDEF_PRICE {
            FixedPriceStateV1::Undefined
        } else {
            FixedPriceStateV1::Present(self.price_raw)
        }
    }

    pub const fn size_state(&self) -> QuantityStateV1 {
        if self.size_raw == UNDEF_ORDER_SIZE {
            QuantityStateV1::Undefined
        } else {
            QuantityStateV1::Present(self.size_raw)
        }
    }

    pub const fn ts_event_state(&self) -> TimestampStateV1 {
        if self.ts_event == UNDEF_TIMESTAMP {
            TimestampStateV1::Undefined
        } else {
            TimestampStateV1::Present(self.ts_event)
        }
    }

    pub const fn ts_recv_state(&self) -> TimestampStateV1 {
        if self.ts_recv == UNDEF_TIMESTAMP {
            TimestampStateV1::Undefined
        } else {
            TimestampStateV1::Present(self.ts_recv)
        }
    }

    pub const fn flags(&self) -> RawFlagsV1 {
        RawFlagsV1(self.flags_raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum FixedPriceStateV1 {
    Present(i64),
    Undefined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum QuantityStateV1 {
    Present(u32),
    Undefined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum TimestampStateV1 {
    Present(u64),
    Undefined,
}

/// Raw flag bitset. Every one of the 256 values round-trips unchanged.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RawFlagsV1(pub u8);

impl RawFlagsV1 {
    pub const fn raw(self) -> u8 {
        self.0
    }

    pub const fn contains(self, mask: u8) -> bool {
        self.0 & mask != 0
    }

    pub const fn is_last(self) -> bool {
        self.contains(FLAG_LAST)
    }

    pub const fn is_snapshot(self) -> bool {
        self.contains(FLAG_SNAPSHOT)
    }

    pub const fn has_bad_ts_recv(self) -> bool {
        self.contains(FLAG_BAD_TS_RECV)
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum KnownActionV1 {
    Add = ACTION_ADD,
    Modify = ACTION_MODIFY,
    Cancel = ACTION_CANCEL,
    Clear = ACTION_CLEAR,
    Trade = ACTION_TRADE,
    Fill = ACTION_FILL,
    None = ACTION_NONE,
}

impl KnownActionV1 {
    pub const fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            ACTION_ADD => Some(Self::Add),
            ACTION_MODIFY => Some(Self::Modify),
            ACTION_CANCEL => Some(Self::Cancel),
            ACTION_CLEAR => Some(Self::Clear),
            ACTION_TRADE => Some(Self::Trade),
            ACTION_FILL => Some(Self::Fill),
            ACTION_NONE => Some(Self::None),
            _ => None,
        }
    }

    pub const fn raw_byte(self) -> u8 {
        match self {
            Self::Add => ACTION_ADD,
            Self::Modify => ACTION_MODIFY,
            Self::Cancel => ACTION_CANCEL,
            Self::Clear => ACTION_CLEAR,
            Self::Trade => ACTION_TRADE,
            Self::Fill => ACTION_FILL,
            Self::None => ACTION_NONE,
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum KnownSideV1 {
    Ask = SIDE_ASK,
    Bid = SIDE_BID,
    None = SIDE_NONE,
}

impl KnownSideV1 {
    pub const fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            SIDE_ASK => Some(Self::Ask),
            SIDE_BID => Some(Self::Bid),
            SIDE_NONE => Some(Self::None),
            _ => None,
        }
    }

    pub const fn raw_byte(self) -> u8 {
        match self {
            Self::Ask => SIDE_ASK,
            Self::Bid => SIDE_BID,
            Self::None => SIDE_NONE,
        }
    }
}

/// Structurally valid event. Fields are private to prevent unchecked creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct ValidatedMboEventV1 {
    raw: RawMboEventV1,
    action: KnownActionV1,
    side: KnownSideV1,
}

impl ValidatedMboEventV1 {
    pub const fn raw(&self) -> &RawMboEventV1 {
        &self.raw
    }

    pub const fn action(&self) -> KnownActionV1 {
        self.action
    }

    pub const fn side(&self) -> KnownSideV1 {
        self.side
    }

    /// `ts_recv` remains raw, but a flagged value is not offered as trusted time.
    pub const fn trusted_ts_recv(&self) -> Option<u64> {
        if self.raw.flags().has_bad_ts_recv() {
            None
        } else {
            match self.raw.ts_recv_state() {
                TimestampStateV1::Present(value) => Some(value),
                TimestampStateV1::Undefined => None,
            }
        }
    }
}

/// Validate universal DBN MBO structure without choosing publisher book policy.
pub fn validate_raw_event(raw: RawMboEventV1) -> Result<ValidatedMboEventV1, ValidationFailureV1> {
    let fail = |reason| ValidationFailureV1::new(&raw, reason);
    if raw.source_object_sha256.is_zero() {
        return Err(fail(ValidationReasonV1::PlaceholderSourceDigest));
    }
    if raw.raw_ordinal == 0 {
        return Err(fail(ValidationReasonV1::ZeroRawOrdinal));
    }
    if raw.rtype != EXPECTED_MBO_RTYPE {
        return Err(fail(ValidationReasonV1::WrongRtype(raw.rtype)));
    }
    if raw.record_size_bytes != EXPECTED_MBO_RECORD_SIZE_BYTES {
        return Err(fail(ValidationReasonV1::WrongRecordSize(
            raw.record_size_bytes,
        )));
    }
    if raw.subordinal != 0 {
        return Err(fail(ValidationReasonV1::NonzeroSubordinal(raw.subordinal)));
    }
    let action = KnownActionV1::from_raw(raw.action_raw)
        .ok_or_else(|| fail(ValidationReasonV1::UnknownAction(raw.action_raw)))?;
    let side = KnownSideV1::from_raw(raw.side_raw)
        .ok_or_else(|| fail(ValidationReasonV1::UnknownSide(raw.side_raw)))?;
    if matches!(raw.ts_event_state(), TimestampStateV1::Undefined) {
        return Err(fail(ValidationReasonV1::UndefinedTsEvent));
    }
    if matches!(raw.ts_recv_state(), TimestampStateV1::Undefined) {
        return Err(fail(ValidationReasonV1::UndefinedTsRecv));
    }

    let side_valid = match action {
        KnownActionV1::Add | KnownActionV1::Modify | KnownActionV1::Cancel => {
            matches!(side, KnownSideV1::Ask | KnownSideV1::Bid)
        }
        KnownActionV1::Clear => side == KnownSideV1::None,
        KnownActionV1::Trade | KnownActionV1::Fill | KnownActionV1::None => true,
    };
    if !side_valid {
        return Err(fail(ValidationReasonV1::InvalidSideForAction {
            action,
            side,
        }));
    }

    let requires_order = matches!(
        action,
        KnownActionV1::Add | KnownActionV1::Modify | KnownActionV1::Cancel
    );
    let requires_price_and_size =
        requires_order || matches!(action, KnownActionV1::Trade | KnownActionV1::Fill);
    if requires_order && raw.order_id == 0 {
        return Err(fail(ValidationReasonV1::ZeroOrderId));
    }
    if requires_price_and_size && matches!(raw.price_state(), FixedPriceStateV1::Undefined) {
        return Err(fail(ValidationReasonV1::UndefinedPrice));
    }
    if requires_price_and_size && matches!(raw.size_state(), QuantityStateV1::Undefined) {
        return Err(fail(ValidationReasonV1::UndefinedSize));
    }
    if requires_price_and_size && raw.size_raw == 0 {
        return Err(fail(ValidationReasonV1::ZeroSize));
    }

    Ok(ValidatedMboEventV1 { raw, action, side })
}

#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize)]
#[error("canonical MBO validation failed at ordinal {raw_ordinal}:{subordinal}: {reason}")]
pub struct ValidationFailureV1 {
    pub source_object_sha256: Sha256DigestV1,
    pub raw_ordinal: u64,
    pub subordinal: u16,
    pub reason: ValidationReasonV1,
}

impl ValidationFailureV1 {
    fn new(raw: &RawMboEventV1, reason: ValidationReasonV1) -> Self {
        Self {
            source_object_sha256: raw.source_object_sha256,
            raw_ordinal: raw.raw_ordinal,
            subordinal: raw.subordinal,
            reason,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize)]
pub enum ValidationReasonV1 {
    #[error("source SHA-256 is an all-zero placeholder")]
    PlaceholderSourceDigest,
    #[error("raw source ordinal is zero; canonical source order is one-based")]
    ZeroRawOrdinal,
    #[error("unexpected rtype {0}; expected MBO rtype 160")]
    WrongRtype(u8),
    #[error("unexpected decoded MBO record size {0}; expected 56 bytes")]
    WrongRecordSize(u16),
    #[error("v1 one-to-one DBN projection requires subordinal 0, found {0}")]
    NonzeroSubordinal(u16),
    #[error("unknown raw action byte {0:#04x}")]
    UnknownAction(u8),
    #[error("unknown raw side byte {0:#04x}")]
    UnknownSide(u8),
    #[error("side {side:?} is invalid for action {action:?}")]
    InvalidSideForAction {
        action: KnownActionV1,
        side: KnownSideV1,
    },
    #[error("ts_event is UNDEF_TIMESTAMP")]
    UndefinedTsEvent,
    #[error("ts_recv is UNDEF_TIMESTAMP")]
    UndefinedTsRecv,
    #[error("required order_id is zero")]
    ZeroOrderId,
    #[error("required price is UNDEF_PRICE")]
    UndefinedPrice,
    #[error("required size is UNDEF_ORDER_SIZE")]
    UndefinedSize,
    #[error("required size is zero")]
    ZeroSize,
    #[error("MAYBE_BAD_BOOK invalidates strict reusable book output")]
    MaybeBadBook,
    #[error("TOB projection is unsupported by the full-order-book profile")]
    UnsupportedTopOfBook,
    #[error("MBP projection is unsupported by the full-order-book profile")]
    UnsupportedMarketByPrice,
    #[error("publisher-specific flag has no registered policy for publisher {0}")]
    PublisherSpecificPolicyRequired(u16),
    #[error("snapshot replay requires a separately registered projection policy")]
    SnapshotPolicyRequired,
    #[error("unassigned DBN flag bit 0 cannot be interpreted by this registered policy")]
    UnassignedFlagPolicyRequired,
    #[error("event source digest does not match the source-bound publisher policy")]
    SourcePolicyBindingMismatch,
    #[error("publisher {publisher_id} is not allowed by policy {policy_id}")]
    PublisherPolicyMismatch {
        policy_id: &'static str,
        publisher_id: u16,
    },
}

/// Custody boundary owned by the canonical event contract. A source-fatal
/// reason means exact stream attribution or policy binding is not trustworthy;
/// a rejected record remains exactly attributable and can be handed to a
/// publisher-specific quarantine owner without calling it accepted semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationBoundaryClassV1 {
    SourceStreamFatal,
    DecodedRecordRejected,
}

impl ValidationReasonV1 {
    pub const fn boundary_class(&self) -> ValidationBoundaryClassV1 {
        match self {
            Self::PlaceholderSourceDigest
            | Self::ZeroRawOrdinal
            | Self::WrongRtype(_)
            | Self::WrongRecordSize(_)
            | Self::NonzeroSubordinal(_)
            | Self::SourcePolicyBindingMismatch
            | Self::PublisherPolicyMismatch { .. } => ValidationBoundaryClassV1::SourceStreamFatal,
            Self::UnknownAction(_)
            | Self::UnknownSide(_)
            | Self::InvalidSideForAction { .. }
            | Self::UndefinedTsEvent
            | Self::UndefinedTsRecv
            | Self::ZeroOrderId
            | Self::UndefinedPrice
            | Self::UndefinedSize
            | Self::ZeroSize
            | Self::MaybeBadBook
            | Self::UnsupportedTopOfBook
            | Self::UnsupportedMarketByPrice
            | Self::PublisherSpecificPolicyRequired(_)
            | Self::SnapshotPolicyRequired
            | Self::UnassignedFlagPolicyRequired => {
                ValidationBoundaryClassV1::DecodedRecordRejected
            }
        }
    }
}

/// Closed identifiers for publisher policy rows registered by the authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PublisherPolicyIdV1 {
    RejectAll,
    XnasItchHistorical,
}

impl Serialize for PublisherPolicyIdV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl PublisherPolicyIdV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RejectAll => PUBLISHER_POLICY_REJECT_ALL_V1_ID,
            Self::XnasItchHistorical => PUBLISHER_POLICY_XNAS_ITCH_HISTORICAL_V1_ID,
        }
    }
}

/// A registered policy bound to one strict source identity.
///
/// Fields are private: callers select an authority row, but cannot invent an
/// ad-hoc acceptance callback or detach the policy from its source object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundPublisherPolicyV1 {
    id: PublisherPolicyIdV1,
    source_object_sha256: Sha256DigestV1,
}

impl BoundPublisherPolicyV1 {
    pub fn bind(
        id: PublisherPolicyIdV1,
        source: &SourceDescriptorV1,
    ) -> Result<Self, PublisherPolicyBindingErrorV1> {
        source.validate_strict()?;
        match id {
            PublisherPolicyIdV1::RejectAll => {}
            PublisherPolicyIdV1::XnasItchHistorical => {
                if source.logical.dataset != "XNAS.ITCH" {
                    return Err(PublisherPolicyBindingErrorV1::WrongDataset {
                        policy_id: id.as_str(),
                        actual: source.logical.dataset.clone(),
                    });
                }
                if source.logical.dbn_version != 1 {
                    return Err(PublisherPolicyBindingErrorV1::WrongDbnVersion {
                        policy_id: id.as_str(),
                        actual: source.logical.dbn_version,
                    });
                }
            }
        }
        Ok(Self {
            id,
            source_object_sha256: source.logical.compressed_sha256,
        })
    }

    pub const fn id(&self) -> PublisherPolicyIdV1 {
        self.id
    }

    pub const fn source_object_sha256(&self) -> Sha256DigestV1 {
        self.source_object_sha256
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PublisherPolicyBindingErrorV1 {
    #[error(transparent)]
    InvalidSource(#[from] SourceIdentityErrorV1),
    #[error("publisher policy {policy_id} requires dataset XNAS.ITCH, found {actual}")]
    WrongDataset {
        policy_id: &'static str,
        actual: String,
    },
    #[error("publisher policy {policy_id} requires DBN version 1, found {actual}")]
    WrongDbnVersion { policy_id: &'static str, actual: u8 },
}

/// Classify one validated full-order-book event into a responsibility-specific lane.
pub fn classify_full_order_book(
    event: ValidatedMboEventV1,
    publisher_policy: &BoundPublisherPolicyV1,
) -> Result<EventDispositionV1, ValidationFailureV1> {
    let raw = event.raw();
    let flags = raw.flags();
    let fail = |reason| ValidationFailureV1::new(raw, reason);
    if flags.contains(FLAG_MAYBE_BAD_BOOK) {
        return Err(fail(ValidationReasonV1::MaybeBadBook));
    }
    if flags.contains(FLAG_TOB) {
        return Err(fail(ValidationReasonV1::UnsupportedTopOfBook));
    }
    if flags.contains(FLAG_MBP) {
        return Err(fail(ValidationReasonV1::UnsupportedMarketByPrice));
    }
    if flags.contains(FLAG_SNAPSHOT) {
        return Err(fail(ValidationReasonV1::SnapshotPolicyRequired));
    }
    if flags.contains(FLAG_RESERVED) {
        return Err(fail(ValidationReasonV1::UnassignedFlagPolicyRequired));
    }
    if raw.source_object_sha256 != publisher_policy.source_object_sha256 {
        return Err(fail(ValidationReasonV1::SourcePolicyBindingMismatch));
    }
    match publisher_policy.id {
        PublisherPolicyIdV1::RejectAll => {
            if flags.contains(FLAG_PUBLISHER_SPECIFIC) {
                return Err(fail(ValidationReasonV1::PublisherSpecificPolicyRequired(
                    raw.publisher_id,
                )));
            }
        }
        PublisherPolicyIdV1::XnasItchHistorical => {
            if !XNAS_ITCH_HISTORICAL_PUBLISHER_IDS_V1.contains(&raw.publisher_id) {
                return Err(fail(ValidationReasonV1::PublisherPolicyMismatch {
                    policy_id: publisher_policy.id.as_str(),
                    publisher_id: raw.publisher_id,
                }));
            }
            // Flag 0x02 remains losslessly present but opaque. This policy does
            // not assign it a semantic meaning or use it for reconstruction.
        }
    }

    let disposition =
        match event.action() {
            KnownActionV1::Add => {
                EventDispositionV1::Book(BookCommandV1::Add(BookOrderCommandV1::new(event)))
            }
            KnownActionV1::Modify => {
                EventDispositionV1::Book(BookCommandV1::Modify(BookOrderCommandV1::new(event)))
            }
            KnownActionV1::Cancel => {
                EventDispositionV1::Book(BookCommandV1::Cancel(BookOrderCommandV1::new(event)))
            }
            KnownActionV1::Clear => {
                EventDispositionV1::Book(BookCommandV1::Clear(ClearBookCommandV1 { event }))
            }
            KnownActionV1::Trade => match event.side() {
                KnownSideV1::Ask => EventDispositionV1::Execution(
                    ExecutionCarrierV1::AggressorTrade(AggressorTradeV1 {
                        event,
                        aggressor: AggressorSideV1::Seller,
                    }),
                ),
                KnownSideV1::Bid => EventDispositionV1::Execution(
                    ExecutionCarrierV1::AggressorTrade(AggressorTradeV1 {
                        event,
                        aggressor: AggressorSideV1::Buyer,
                    }),
                ),
                KnownSideV1::None => EventDispositionV1::Execution(
                    ExecutionCarrierV1::UnsignedTrade(UnsignedTradeV1 { event }),
                ),
            },
            KnownActionV1::Fill => {
                EventDispositionV1::Execution(ExecutionCarrierV1::RestingFill(RestingFillV1 {
                    resting_side: RestingSideV1::from_known(event.side()),
                    event,
                }))
            }
            KnownActionV1::None => EventDispositionV1::Control(ControlEventV1 { event }),
        };
    Ok(disposition)
}

/// Accepted event lanes. Quarantine is deliberately not a variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum EventDispositionV1 {
    Book(BookCommandV1),
    Execution(ExecutionCarrierV1),
    Control(ControlEventV1),
}

impl EventDispositionV1 {
    pub const fn event(&self) -> &ValidatedMboEventV1 {
        match self {
            Self::Book(command) => command.event(),
            Self::Execution(carrier) => carrier.event(),
            Self::Control(control) => control.event(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum BookCommandV1 {
    Add(BookOrderCommandV1),
    Modify(BookOrderCommandV1),
    Cancel(BookOrderCommandV1),
    Clear(ClearBookCommandV1),
}

impl BookCommandV1 {
    pub const fn event(&self) -> &ValidatedMboEventV1 {
        match self {
            Self::Add(command) | Self::Modify(command) | Self::Cancel(command) => command.event(),
            Self::Clear(command) => command.event(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BookOrderCommandV1 {
    event: ValidatedMboEventV1,
    resting_side: RestingSideV1,
}

impl BookOrderCommandV1 {
    fn new(event: ValidatedMboEventV1) -> Self {
        let resting_side = RestingSideV1::from_known(event.side())
            .expect("validated A/M/C event always has a resting side");
        Self {
            event,
            resting_side,
        }
    }

    pub const fn event(&self) -> &ValidatedMboEventV1 {
        &self.event
    }

    pub const fn resting_side(&self) -> RestingSideV1 {
        self.resting_side
    }

    pub const fn order_id(&self) -> u64 {
        self.event.raw.order_id
    }

    pub const fn price_raw(&self) -> i64 {
        self.event.raw.price_raw
    }

    pub const fn size_raw(&self) -> u32 {
        self.event.raw.size_raw
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ClearBookCommandV1 {
    event: ValidatedMboEventV1,
}

impl ClearBookCommandV1 {
    pub const fn event(&self) -> &ValidatedMboEventV1 {
        &self.event
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ExecutionCarrierV1 {
    AggressorTrade(AggressorTradeV1),
    UnsignedTrade(UnsignedTradeV1),
    RestingFill(RestingFillV1),
}

impl ExecutionCarrierV1 {
    pub const fn event(&self) -> &ValidatedMboEventV1 {
        match self {
            Self::AggressorTrade(value) => value.event(),
            Self::UnsignedTrade(value) => value.event(),
            Self::RestingFill(value) => value.event(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AggressorTradeV1 {
    event: ValidatedMboEventV1,
    aggressor: AggressorSideV1,
}

impl AggressorTradeV1 {
    pub const fn event(&self) -> &ValidatedMboEventV1 {
        &self.event
    }

    pub const fn aggressor(&self) -> AggressorSideV1 {
        self.aggressor
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct UnsignedTradeV1 {
    event: ValidatedMboEventV1,
}

impl UnsignedTradeV1 {
    pub const fn event(&self) -> &ValidatedMboEventV1 {
        &self.event
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RestingFillV1 {
    event: ValidatedMboEventV1,
    resting_side: Option<RestingSideV1>,
}

impl RestingFillV1 {
    pub const fn event(&self) -> &ValidatedMboEventV1 {
        &self.event
    }

    pub const fn resting_side(&self) -> Option<RestingSideV1> {
        self.resting_side
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ControlEventV1 {
    event: ValidatedMboEventV1,
}

impl ControlEventV1 {
    pub const fn event(&self) -> &ValidatedMboEventV1 {
        &self.event
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum RestingSideV1 {
    Ask,
    Bid,
}

impl RestingSideV1 {
    const fn from_known(side: KnownSideV1) -> Option<Self> {
        match side {
            KnownSideV1::Ask => Some(Self::Ask),
            KnownSideV1::Bid => Some(Self::Bid),
            KnownSideV1::None => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum AggressorSideV1 {
    Seller,
    Buyer,
}

/// Explicit non-success output for repair/diagnostic workflows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QuarantineRecordV1 {
    pub raw: RawMboEventV1,
    pub failure: ValidationFailureV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ProcessingOutcomeV1 {
    Accepted(EventDispositionV1),
    Quarantined(QuarantineRecordV1),
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    const DIGEST: Sha256DigestV1 = Sha256DigestV1::from_bytes([0xAB; 32]);

    fn raw(action: u8, side: u8) -> RawMboEventV1 {
        RawMboEventV1 {
            source_object_sha256: DIGEST,
            raw_ordinal: 17,
            subordinal: 0,
            rtype: EXPECTED_MBO_RTYPE,
            record_size_bytes: EXPECTED_MBO_RECORD_SIZE_BYTES,
            publisher_id: 2,
            instrument_id: 10_001,
            ts_event: 1_750_000_000_000_000_123,
            ts_recv: 1_750_000_000_000_100_456,
            ts_in_delta: 100_333,
            channel_id: 3,
            sequence: 987_654,
            order_id: 42,
            price_raw: 123_456_789_012,
            size_raw: 77,
            flags_raw: FLAG_LAST,
            action_raw: action,
            side_raw: side,
        }
    }

    fn disposition(action: u8, side: u8) -> EventDispositionV1 {
        let validated = validate_raw_event(raw(action, side)).unwrap();
        let policy =
            BoundPublisherPolicyV1::bind(PublisherPolicyIdV1::RejectAll, &source_descriptor())
                .unwrap();
        classify_full_order_book(validated, &policy).unwrap()
    }

    #[test]
    fn crate_local_snapshot_and_sidecar_match_generated_identity() {
        let bytes = include_bytes!("../contracts/mbo_event_contract.toml");
        let actual = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let declared = include_str!("../contracts/mbo_event_contract.sha256").trim();
        assert_eq!(actual, declared);
        assert_eq!(actual, CANONICAL_MBO_EVENT_CONTRACT_SHA256);
        assert_eq!(CANONICAL_MBO_EVENT_SCHEMA_VERSION, "1.0.0");
    }

    #[test]
    fn digest_json_is_lowercase_hex_and_round_trips() {
        let encoded = serde_json::to_string(&DIGEST).unwrap();
        assert_eq!(encoded, format!("\"{}\"", "ab".repeat(32)));
        let decoded: Sha256DigestV1 = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, DIGEST);
        assert!(Sha256DigestV1::from_hex(&"AB".repeat(32)).is_err());
    }

    #[test]
    fn publisher_policy_wire_identity_matches_the_authority_row() {
        assert_eq!(
            serde_json::to_string(&PublisherPolicyIdV1::RejectAll).unwrap(),
            format!("\"{PUBLISHER_POLICY_REJECT_ALL_V1_ID}\"")
        );
        assert_eq!(
            serde_json::to_string(&PublisherPolicyIdV1::XnasItchHistorical).unwrap(),
            format!("\"{PUBLISHER_POLICY_XNAS_ITCH_HISTORICAL_V1_ID}\"")
        );
        assert_eq!(XNAS_ITCH_HISTORICAL_PUBLISHER_IDS_V1, &[2]);
    }

    #[test]
    fn all_raw_fields_round_trip_without_interpretive_loss() {
        let event = raw(ACTION_ADD, SIDE_BID);
        let bytes = serde_json::to_vec(&event).unwrap();
        let decoded: RawMboEventV1 = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded, event);
        assert_eq!(RAW_EVENT_FIELD_DESCRIPTORS.len(), 18);
    }

    #[test]
    fn all_256_flag_values_round_trip() {
        for value in u8::MIN..=u8::MAX {
            let flags = RawFlagsV1(value);
            assert_eq!(flags.raw(), value);
            let encoded = serde_json::to_string(&flags).unwrap();
            let decoded: RawFlagsV1 = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded.raw(), value);
        }
    }

    #[test]
    fn trade_and_fill_are_distinct_non_book_lanes() {
        assert!(matches!(
            disposition(ACTION_TRADE, SIDE_BID),
            EventDispositionV1::Execution(ExecutionCarrierV1::AggressorTrade(value))
                if value.aggressor() == AggressorSideV1::Buyer
        ));
        assert!(matches!(
            disposition(ACTION_TRADE, SIDE_NONE),
            EventDispositionV1::Execution(ExecutionCarrierV1::UnsignedTrade(_))
        ));
        assert!(matches!(
            disposition(ACTION_FILL, SIDE_ASK),
            EventDispositionV1::Execution(ExecutionCarrierV1::RestingFill(value))
                if value.resting_side() == Some(RestingSideV1::Ask)
        ));
    }

    #[test]
    fn only_amcr_create_book_commands() {
        for (action, expected) in [
            (ACTION_ADD, "add"),
            (ACTION_MODIFY, "modify"),
            (ACTION_CANCEL, "cancel"),
            (ACTION_CLEAR, "clear"),
        ] {
            let side = if action == ACTION_CLEAR {
                SIDE_NONE
            } else {
                SIDE_ASK
            };
            let actual = disposition(action, side);
            assert!(matches!(actual, EventDispositionV1::Book(_)), "{expected}");
        }
        for action in [ACTION_TRADE, ACTION_FILL, ACTION_NONE] {
            assert!(!matches!(
                disposition(action, SIDE_NONE),
                EventDispositionV1::Book(_)
            ));
        }
    }

    #[test]
    fn timestamp_zero_is_present_but_true_sentinel_fails() {
        let mut event = raw(ACTION_ADD, SIDE_BID);
        event.ts_event = 0;
        event.ts_recv = 0;
        assert!(validate_raw_event(event).is_ok());

        event.ts_event = UNDEF_TIMESTAMP;
        assert_eq!(
            validate_raw_event(event).unwrap_err().reason,
            ValidationReasonV1::UndefinedTsEvent
        );
    }

    #[test]
    fn negative_fixed_price_is_not_silently_rejected() {
        let mut event = raw(ACTION_ADD, SIDE_BID);
        event.price_raw = -1_000_000_000;
        assert!(validate_raw_event(event).is_ok());
        event.price_raw = UNDEF_PRICE;
        assert_eq!(
            validate_raw_event(event).unwrap_err().reason,
            ValidationReasonV1::UndefinedPrice
        );
    }

    #[test]
    fn trade_order_id_zero_is_valid_and_not_a_system_taxonomy() {
        let mut event = raw(ACTION_TRADE, SIDE_NONE);
        event.order_id = 0;
        assert!(matches!(
            classify_full_order_book(
                validate_raw_event(event).unwrap(),
                &BoundPublisherPolicyV1::bind(
                    PublisherPolicyIdV1::RejectAll,
                    &source_descriptor(),
                )
                .unwrap()
            )
            .unwrap(),
            EventDispositionV1::Execution(ExecutionCarrierV1::UnsignedTrade(_))
        ));
    }

    #[test]
    fn unknown_and_legacy_sell_side_fail_without_coercion() {
        let error = validate_raw_event(raw(ACTION_ADD, b'S')).unwrap_err();
        assert_eq!(error.reason, ValidationReasonV1::UnknownSide(b'S'));
        let error = validate_raw_event(raw(b'X', SIDE_BID)).unwrap_err();
        assert_eq!(error.reason, ValidationReasonV1::UnknownAction(b'X'));
    }

    #[test]
    fn validation_boundary_class_is_exhaustive_and_custody_preserving() {
        let source_fatal = [
            ValidationReasonV1::PlaceholderSourceDigest,
            ValidationReasonV1::ZeroRawOrdinal,
            ValidationReasonV1::WrongRtype(1),
            ValidationReasonV1::WrongRecordSize(1),
            ValidationReasonV1::NonzeroSubordinal(1),
            ValidationReasonV1::SourcePolicyBindingMismatch,
            ValidationReasonV1::PublisherPolicyMismatch {
                policy_id: "test",
                publisher_id: 9,
            },
        ];
        assert!(source_fatal.iter().all(|reason| {
            reason.boundary_class() == ValidationBoundaryClassV1::SourceStreamFatal
        }));

        let rejected = [
            ValidationReasonV1::UnknownAction(b'X'),
            ValidationReasonV1::UnknownSide(b'X'),
            ValidationReasonV1::InvalidSideForAction {
                action: KnownActionV1::Add,
                side: KnownSideV1::None,
            },
            ValidationReasonV1::UndefinedTsEvent,
            ValidationReasonV1::UndefinedTsRecv,
            ValidationReasonV1::ZeroOrderId,
            ValidationReasonV1::UndefinedPrice,
            ValidationReasonV1::UndefinedSize,
            ValidationReasonV1::ZeroSize,
            ValidationReasonV1::MaybeBadBook,
            ValidationReasonV1::UnsupportedTopOfBook,
            ValidationReasonV1::UnsupportedMarketByPrice,
            ValidationReasonV1::PublisherSpecificPolicyRequired(2),
            ValidationReasonV1::SnapshotPolicyRequired,
            ValidationReasonV1::UnassignedFlagPolicyRequired,
        ];
        assert!(rejected.iter().all(|reason| {
            reason.boundary_class() == ValidationBoundaryClassV1::DecodedRecordRejected
        }));
    }

    #[test]
    fn book_quality_and_unsupported_projection_flags_fail() {
        for (flag, reason) in [
            (FLAG_MAYBE_BAD_BOOK, ValidationReasonV1::MaybeBadBook),
            (FLAG_TOB, ValidationReasonV1::UnsupportedTopOfBook),
            (FLAG_MBP, ValidationReasonV1::UnsupportedMarketByPrice),
        ] {
            let mut event = raw(ACTION_ADD, SIDE_BID);
            event.flags_raw = flag;
            let validated = validate_raw_event(event).unwrap();
            assert_eq!(
                classify_full_order_book(
                    validated,
                    &BoundPublisherPolicyV1::bind(
                        PublisherPolicyIdV1::RejectAll,
                        &source_descriptor(),
                    )
                    .unwrap(),
                )
                .unwrap_err()
                .reason,
                reason
            );
        }
    }

    #[test]
    fn publisher_specific_requires_a_registered_source_bound_policy() {
        let mut event = raw(ACTION_ADD, SIDE_BID);
        event.flags_raw = FLAG_PUBLISHER_SPECIFIC;
        let validated = validate_raw_event(event).unwrap();
        let reject =
            BoundPublisherPolicyV1::bind(PublisherPolicyIdV1::RejectAll, &source_descriptor())
                .unwrap();
        assert!(matches!(
            classify_full_order_book(validated, &reject)
                .unwrap_err()
                .reason,
            ValidationReasonV1::PublisherSpecificPolicyRequired(2)
        ));
        let xnas = BoundPublisherPolicyV1::bind(
            PublisherPolicyIdV1::XnasItchHistorical,
            &source_descriptor(),
        )
        .unwrap();
        assert!(classify_full_order_book(validated, &xnas).is_ok());

        let mut wrong_publisher = raw(ACTION_ADD, SIDE_BID);
        wrong_publisher.publisher_id = 7;
        assert!(matches!(
            classify_full_order_book(validate_raw_event(wrong_publisher).unwrap(), &xnas)
                .unwrap_err()
                .reason,
            ValidationReasonV1::PublisherPolicyMismatch {
                publisher_id: 7,
                ..
            }
        ));
    }

    fn source_descriptor() -> SourceDescriptorV1 {
        let accepted = ACCEPTED_CATALOG_RELEASE_BINDINGS_V1[0];
        let parse = |value: &str| Sha256DigestV1::from_hex(value).unwrap();
        SourceDescriptorV1 {
            logical: LogicalSourceV1 {
                catalog_release_id: accepted.release_id.into(),
                catalog_storage_root_id: accepted.storage_root_id.into(),
                custody_projection_schema: accepted.custody_projection_schema.into(),
                custody_projection_file_sha256: parse(accepted.custody_projection_file_sha256),
                custody_projection_content_sha256: parse(
                    accepted.custody_projection_content_sha256,
                ),
                canonical_profile_sha256: parse(accepted.canonical_profile_sha256),
                embedded_per_file_tsv_sha256: parse(accepted.embedded_per_file_tsv_sha256),
                evidence_manifest_sha256: parse(accepted.evidence_manifest_sha256),
                terminal_validation_receipt_sha256: parse(
                    accepted.terminal_validation_receipt_sha256,
                ),
                terminal_validation_status: accepted.terminal_validation_status.into(),
                relative_path: "object.dbn.zst".into(),
                compressed_sha256: DIGEST,
                compressed_bytes: 123,
                expected_records: 1,
                metadata_start_ns: 1,
                metadata_end_ns: 2,
                requested_symbols_preview: "NVDA".into(),
                requested_symbols_sha256: Sha256DigestV1::from_bytes([7; 32]),
                symbols_n: 1,
                active_instruments_n: 1,
                provenance_tier: "test_only".into(),
                provider_manifest_relative_path: "manifest.json".into(),
                provider_manifest_sha256: Sha256DigestV1::from_bytes([8; 32]),
                provider_job_id: "test-job".into(),
                provider_declared_data_file_count: 1,
                dbn_version: 1,
                dbn_ts_out: false,
                dataset: "XNAS.ITCH".into(),
                schema: "mbo".into(),
            },
            opened: OpenedReplicaV1 {
                custody_projection_path: "/data/custody.json".into(),
                storage_root_path: "/data".into(),
                relative_path: "object.dbn.zst".into(),
                opened_path: "/data/object.dbn.zst".into(),
                representation: OpenedRepresentationV1::CanonicalObject,
                opened_sha256: DIGEST,
                opened_bytes: 123,
                device_id: 1,
                inode: 1,
                metadata_bytes: 123,
                modified_seconds: 1,
                modified_nanoseconds: 0,
                changed_seconds: 1,
                changed_nanoseconds: 0,
            },
        }
    }

    #[test]
    fn strict_source_identity_rejects_substitution_and_derived_replica() {
        let source = source_descriptor();
        assert!(source.validate_strict().is_ok());

        let mut substituted = source.clone();
        substituted.opened.opened_path = "/hot/object.dbn".into();
        assert!(matches!(
            substituted.validate_strict(),
            Err(SourceIdentityErrorV1::PathSubstitution { .. })
        ));

        let mut derived = source;
        derived.opened.representation = OpenedRepresentationV1::DerivedReplica {
            derivation_receipt_sha256: Sha256DigestV1::from_bytes([0xCD; 32]),
        };
        assert_eq!(
            derived.validate_strict().unwrap_err(),
            SourceIdentityErrorV1::DerivedReplicaNotAllowed
        );
    }

    #[test]
    fn zero_digest_placeholders_fail_before_accepted_identity_or_event() {
        let mut source = source_descriptor();
        source.logical.compressed_sha256 = Sha256DigestV1::from_bytes([0; 32]);
        assert_eq!(
            source.validate_strict().unwrap_err(),
            SourceIdentityErrorV1::PlaceholderDigest
        );

        let mut event = raw(ACTION_ADD, SIDE_BID);
        event.source_object_sha256 = Sha256DigestV1::from_bytes([0; 32]);
        assert_eq!(
            validate_raw_event(event).unwrap_err().reason,
            ValidationReasonV1::PlaceholderSourceDigest
        );
    }

    #[test]
    fn zero_raw_ordinal_fails_before_event_acceptance() {
        let mut event = raw(ACTION_ADD, SIDE_BID);
        event.raw_ordinal = 0;
        assert_eq!(
            validate_raw_event(event).unwrap_err().reason,
            ValidationReasonV1::ZeroRawOrdinal
        );
    }

    #[test]
    fn source_policy_binding_and_unsupported_flags_fail_closed() {
        let source = source_descriptor();
        let policy =
            BoundPublisherPolicyV1::bind(PublisherPolicyIdV1::XnasItchHistorical, &source).unwrap();

        let mut wrong_source = raw(ACTION_ADD, SIDE_BID);
        wrong_source.source_object_sha256 = Sha256DigestV1::from_bytes([0xCD; 32]);
        assert_eq!(
            classify_full_order_book(validate_raw_event(wrong_source).unwrap(), &policy)
                .unwrap_err()
                .reason,
            ValidationReasonV1::SourcePolicyBindingMismatch
        );

        for (flag, reason) in [
            (FLAG_SNAPSHOT, ValidationReasonV1::SnapshotPolicyRequired),
            (
                FLAG_RESERVED,
                ValidationReasonV1::UnassignedFlagPolicyRequired,
            ),
        ] {
            let mut event = raw(ACTION_ADD, SIDE_BID);
            event.flags_raw = flag;
            assert_eq!(
                classify_full_order_book(validate_raw_event(event).unwrap(), &policy)
                    .unwrap_err()
                    .reason,
                reason
            );
        }
    }

    #[test]
    fn every_flag_byte_has_a_closed_policy_outcome() {
        let source = source_descriptor();
        let reject = BoundPublisherPolicyV1::bind(PublisherPolicyIdV1::RejectAll, &source).unwrap();
        let xnas =
            BoundPublisherPolicyV1::bind(PublisherPolicyIdV1::XnasItchHistorical, &source).unwrap();
        let mut reject_accepted = Vec::new();
        let mut xnas_accepted = Vec::new();
        for flags in u8::MIN..=u8::MAX {
            let mut event = raw(ACTION_ADD, SIDE_BID);
            event.flags_raw = flags;
            let validated = validate_raw_event(event).unwrap();
            if classify_full_order_book(validated, &reject).is_ok() {
                reject_accepted.push(flags);
            }
            if classify_full_order_book(validated, &xnas).is_ok() {
                xnas_accepted.push(flags);
            }
        }
        assert_eq!(
            reject_accepted,
            [0, FLAG_BAD_TS_RECV, FLAG_LAST, FLAG_LAST | FLAG_BAD_TS_RECV]
        );
        assert_eq!(
            xnas_accepted,
            [
                0,
                FLAG_PUBLISHER_SPECIFIC,
                FLAG_BAD_TS_RECV,
                FLAG_BAD_TS_RECV | FLAG_PUBLISHER_SPECIFIC,
                FLAG_LAST,
                FLAG_LAST | FLAG_PUBLISHER_SPECIFIC,
                FLAG_LAST | FLAG_BAD_TS_RECV,
                FLAG_LAST | FLAG_BAD_TS_RECV | FLAG_PUBLISHER_SPECIFIC,
            ]
        );
    }

    #[test]
    fn ts_out_and_wrong_xnas_policy_identity_are_rejected() {
        let mut source = source_descriptor();
        source.logical.dbn_ts_out = true;
        assert_eq!(
            source.validate_strict().unwrap_err(),
            SourceIdentityErrorV1::TsOutUnsupported
        );

        let mut source = source_descriptor();
        source.logical.dataset = "GLBX.MDP3".into();
        assert!(matches!(
            BoundPublisherPolicyV1::bind(PublisherPolicyIdV1::XnasItchHistorical, &source),
            Err(PublisherPolicyBindingErrorV1::WrongDataset { .. })
        ));
    }
}
