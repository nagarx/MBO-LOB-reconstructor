//! Exact catalog-membership and no-symlink physical-open boundary.

use hft_mbo_event_contract::{LogicalSourceV1, Sha256DigestV1, SourceIdentityErrorV1};
use rustix::fs::{open, openat, Mode, OFlags};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CatalogSelectionErrorV1 {
    #[error(transparent)]
    SourceIdentity(#[from] SourceIdentityErrorV1),
    #[error("custody projection path is not valid UTF-8: {0:?}")]
    NonUtf8ProjectionPath(PathBuf),
    #[error("failed to read custody projection {path}: {source}")]
    ProjectionRead {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("custody projection file SHA-256 mismatch: expected={expected}, actual={actual}")]
    ProjectionFileDigestMismatch {
        expected: Sha256DigestV1,
        actual: Sha256DigestV1,
    },
    #[error("custody projection is not valid schema-v1 JSON: {0}")]
    ProjectionDecode(#[from] serde_json::Error),
    #[error("custody projection release authority differs from the logical source")]
    ProjectionReleaseMismatch,
    #[error("custody projection contains {matches} exact path members for {relative_path}")]
    ProjectionObjectCardinality {
        relative_path: String,
        matches: usize,
    },
    #[error("custody projection object/provider identity differs from the logical source")]
    ProjectionObjectMismatch,
    #[error("storage root must be one absolute UTF-8 directory path: {0:?}")]
    InvalidStorageRoot(PathBuf),
    #[error("failed no-symlink open of catalog object {path}: {source}")]
    PhysicalOpen {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("opened catalog object is not a regular file: {0}")]
    NotRegularFile(String),
}

#[derive(Debug)]
pub(crate) struct OpenedCatalogObjectV1 {
    pub file: File,
    pub opened_path: String,
    pub device_id: u64,
    pub inode: u64,
    pub metadata_bytes: u64,
    pub modified_seconds: i64,
    pub modified_nanoseconds: i64,
    pub changed_seconds: i64,
    pub changed_nanoseconds: i64,
}

#[derive(Debug, Deserialize)]
struct CustodyEnvelopeV1 {
    schema: String,
    content_sha256: String,
    content: CustodyContentV1,
}

#[derive(Debug, Deserialize)]
struct CustodyContentV1 {
    release: CustodyReleaseV1,
    groups: BTreeMap<String, CustodyGroupV1>,
}

#[derive(Debug, Deserialize)]
struct CustodyReleaseV1 {
    release_id: String,
    storage_root_id: String,
    observed_root_path: String,
    canonical_profile_sha256: String,
    embedded_per_file_tsv_sha256: String,
    evidence_manifest_sha256: String,
    terminal_validation_receipt_sha256: String,
    terminal_validation_status: String,
}

#[derive(Debug, Deserialize)]
struct CustodyGroupV1 {
    identity: CustodyGroupIdentityV1,
    provider_receipt: CustodyProviderReceiptV1,
    objects: Vec<CustodyObjectV1>,
}

#[derive(Debug, Deserialize)]
struct CustodyGroupIdentityV1 {
    dataset: String,
    schema: String,
    dbn_version: u8,
    requested_symbols_preview: String,
    requested_symbols_sha256: String,
    symbols_n: u64,
    active_instruments_n: u64,
}

#[derive(Debug, Deserialize)]
struct CustodyProviderReceiptV1 {
    relative_path: String,
    sha256: String,
    job_id: String,
    declared_data_file_count: u64,
}

#[derive(Debug, Deserialize)]
struct CustodyObjectV1 {
    relative_path: String,
    compressed_sha256: String,
    compressed_bytes: u64,
    records: u64,
    dataset: String,
    schema: String,
    dbn_version: u8,
    metadata_start_ns: u64,
    metadata_end_ns: u64,
    requested_symbols_preview: String,
    requested_symbols_sha256: String,
    symbols_n: u64,
    active_instruments_n: u64,
    provenance_tier: String,
}

pub(crate) fn verify_catalog_membership(
    logical: &LogicalSourceV1,
    custody_projection_path: &Path,
    storage_root_path: &Path,
) -> Result<(), CatalogSelectionErrorV1> {
    logical.validate_strict()?;
    let projection_path_text = custody_projection_path.to_str().ok_or_else(|| {
        CatalogSelectionErrorV1::NonUtf8ProjectionPath(custody_projection_path.to_path_buf())
    })?;
    let projection_bytes = std::fs::read(custody_projection_path).map_err(|source| {
        CatalogSelectionErrorV1::ProjectionRead {
            path: projection_path_text.to_owned(),
            source,
        }
    })?;
    let projection_file_sha256 =
        Sha256DigestV1::from_bytes(Sha256::digest(&projection_bytes).into());
    if projection_file_sha256 != logical.custody_projection_file_sha256 {
        return Err(CatalogSelectionErrorV1::ProjectionFileDigestMismatch {
            expected: logical.custody_projection_file_sha256,
            actual: projection_file_sha256,
        });
    }
    let envelope: CustodyEnvelopeV1 = serde_json::from_slice(&projection_bytes)?;
    let root_text = storage_root_path.to_str().ok_or_else(|| {
        CatalogSelectionErrorV1::InvalidStorageRoot(storage_root_path.to_path_buf())
    })?;
    let release = &envelope.content.release;
    if envelope.schema != logical.custody_projection_schema
        || envelope.content_sha256 != logical.custody_projection_content_sha256.to_hex()
        || release.release_id != logical.catalog_release_id
        || release.storage_root_id != logical.catalog_storage_root_id
        || release.observed_root_path != root_text
        || release.canonical_profile_sha256 != logical.canonical_profile_sha256.to_hex()
        || release.embedded_per_file_tsv_sha256 != logical.embedded_per_file_tsv_sha256.to_hex()
        || release.evidence_manifest_sha256 != logical.evidence_manifest_sha256.to_hex()
        || release.terminal_validation_receipt_sha256
            != logical.terminal_validation_receipt_sha256.to_hex()
        || release.terminal_validation_status != logical.terminal_validation_status
    {
        return Err(CatalogSelectionErrorV1::ProjectionReleaseMismatch);
    }

    let matches = envelope
        .content
        .groups
        .values()
        .flat_map(|group| {
            group
                .objects
                .iter()
                .filter(|object| object.relative_path == logical.relative_path)
                .map(move |object| (group, object))
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(CatalogSelectionErrorV1::ProjectionObjectCardinality {
            relative_path: logical.relative_path.clone(),
            matches: matches.len(),
        });
    }
    let (group, object) = matches[0];
    let identity = &group.identity;
    let provider = &group.provider_receipt;
    let object_matches = object.compressed_sha256 == logical.compressed_sha256.to_hex()
        && object.compressed_bytes == logical.compressed_bytes
        && object.records == logical.expected_records
        && object.dataset == logical.dataset
        && object.schema.to_ascii_lowercase() == logical.schema
        && object.dbn_version == logical.dbn_version
        && object.metadata_start_ns == logical.metadata_start_ns
        && object.metadata_end_ns == logical.metadata_end_ns
        && object.requested_symbols_preview == logical.requested_symbols_preview
        && object.requested_symbols_sha256 == logical.requested_symbols_sha256.to_hex()
        && object.symbols_n == logical.symbols_n
        && object.active_instruments_n == logical.active_instruments_n
        && object.provenance_tier == logical.provenance_tier
        && identity.dataset == logical.dataset
        && identity.schema == object.schema
        && identity.dbn_version == logical.dbn_version
        && identity.requested_symbols_preview == logical.requested_symbols_preview
        && identity.requested_symbols_sha256 == logical.requested_symbols_sha256.to_hex()
        && identity.symbols_n == logical.symbols_n
        && identity.active_instruments_n == logical.active_instruments_n
        && provider.relative_path == logical.provider_manifest_relative_path
        && provider.sha256 == logical.provider_manifest_sha256.to_hex()
        && provider.job_id == logical.provider_job_id
        && provider.declared_data_file_count == logical.provider_declared_data_file_count;
    if !object_matches {
        return Err(CatalogSelectionErrorV1::ProjectionObjectMismatch);
    }
    Ok(())
}

pub(crate) fn open_catalog_object_no_symlinks(
    storage_root_path: &Path,
    relative_path: &str,
) -> Result<OpenedCatalogObjectV1, CatalogSelectionErrorV1> {
    let root_text = storage_root_path.to_str().ok_or_else(|| {
        CatalogSelectionErrorV1::InvalidStorageRoot(storage_root_path.to_path_buf())
    })?;
    if !storage_root_path.is_absolute() {
        return Err(CatalogSelectionErrorV1::InvalidStorageRoot(
            storage_root_path.to_path_buf(),
        ));
    }
    let directory_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let file_flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut directory = open("/", directory_flags, Mode::empty()).map_err(|source| {
        CatalogSelectionErrorV1::PhysicalOpen {
            path: root_text.to_owned(),
            source: source.into(),
        }
    })?;
    for component in storage_root_path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(component) => {
                directory = openat(&directory, component, directory_flags, Mode::empty()).map_err(
                    |source| CatalogSelectionErrorV1::PhysicalOpen {
                        path: root_text.to_owned(),
                        source: source.into(),
                    },
                )?;
            }
            _ => {
                return Err(CatalogSelectionErrorV1::InvalidStorageRoot(
                    storage_root_path.to_path_buf(),
                ));
            }
        }
    }

    let components = relative_path.split('/').collect::<Vec<_>>();
    for component in &components[..components.len() - 1] {
        directory =
            openat(&directory, *component, directory_flags, Mode::empty()).map_err(|source| {
                CatalogSelectionErrorV1::PhysicalOpen {
                    path: storage_root_path.join(relative_path).display().to_string(),
                    source: source.into(),
                }
            })?;
    }
    let descriptor = openat(
        &directory,
        components[components.len() - 1],
        file_flags,
        Mode::empty(),
    )
    .map_err(|source| CatalogSelectionErrorV1::PhysicalOpen {
        path: storage_root_path.join(relative_path).display().to_string(),
        source: source.into(),
    })?;
    let file = File::from(descriptor);
    let metadata = file
        .metadata()
        .map_err(|source| CatalogSelectionErrorV1::PhysicalOpen {
            path: storage_root_path.join(relative_path).display().to_string(),
            source,
        })?;
    let opened_path = storage_root_path.join(relative_path);
    let opened_path_text = opened_path
        .to_str()
        .ok_or_else(|| CatalogSelectionErrorV1::InvalidStorageRoot(opened_path.clone()))?
        .to_owned();
    if !metadata.file_type().is_file() {
        return Err(CatalogSelectionErrorV1::NotRegularFile(opened_path_text));
    }
    Ok(OpenedCatalogObjectV1 {
        file,
        opened_path: opened_path_text,
        device_id: metadata.dev(),
        inode: metadata.ino(),
        metadata_bytes: metadata.len(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
        changed_seconds: metadata.ctime(),
        changed_nanoseconds: metadata.ctime_nsec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn physical_open_rejects_leaf_and_intermediate_symlinks() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let actual_dir = root.join("actual");
        std::fs::create_dir(&actual_dir).unwrap();
        std::fs::write(actual_dir.join("source.dbn.zst"), b"source").unwrap();

        symlink(actual_dir.join("source.dbn.zst"), root.join("leaf.dbn.zst")).unwrap();
        assert!(matches!(
            open_catalog_object_no_symlinks(&root, "leaf.dbn.zst"),
            Err(CatalogSelectionErrorV1::PhysicalOpen { .. })
        ));

        symlink(&actual_dir, root.join("alias")).unwrap();
        assert!(matches!(
            open_catalog_object_no_symlinks(&root, "alias/source.dbn.zst"),
            Err(CatalogSelectionErrorV1::PhysicalOpen { .. })
        ));

        let opened = open_catalog_object_no_symlinks(&root, "actual/source.dbn.zst").unwrap();
        assert_eq!(opened.metadata_bytes, 6);
    }
}
