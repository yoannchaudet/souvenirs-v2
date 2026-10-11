use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use schemars::{
    schema::{InstanceType, RootSchema, Schema, SchemaObject, StringValidation},
    schema_for, JsonSchema,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use time::Date;

use crate::names::{PathCollisionKey, PortableRelativePath};
use crate::source::SourcePath;

/// Versions 1 and 2 hash the normalized `PlanDocument` compact JSON representation.
pub const IMPORT_PLAN_SCHEMA_VERSION: u32 = 2;

#[derive(
    Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize, JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
#[schemars(transparent)]
pub struct PlanId(String);

impl PlanId {
    pub fn new(value: impl Into<String>) -> Result<Self, PlanError> {
        let value = value.into();
        if value.is_empty() {
            return Err(PlanError::EmptyIdentifier("plan"));
        }
        if value
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
        {
            return Err(PlanError::InvalidPlanId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for PlanId {
    type Error = PlanError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<PlanId> for String {
    fn from(value: PlanId) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ContentDigest(String);

impl JsonSchema for ContentDigest {
    fn schema_name() -> String {
        "ContentDigest".into()
    }

    fn json_schema(_: &mut schemars::gen::SchemaGenerator) -> Schema {
        Schema::Object(SchemaObject {
            instance_type: Some(InstanceType::String.into()),
            string: Some(Box::new(StringValidation {
                pattern: Some("^[0-9a-f]{64}$".into()),
                ..StringValidation::default()
            })),
            ..SchemaObject::default()
        })
    }
}

impl ContentDigest {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let digest = Sha256::digest(bytes);
        Self(hex_encode(&digest))
    }

    pub fn parse(value: impl Into<String>) -> Result<Self, PlanError> {
        let value = value.into();
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(PlanError::InvalidDigest);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ContentDigest {
    type Error = PlanError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<ContentDigest> for String {
    fn from(value: ContentDigest) -> Self {
        value.0
    }
}

#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize, JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct PlanTimestamp {
    pub unix_seconds: i64,
    pub nanosecond: u32,
}

impl PlanTimestamp {
    pub fn new(unix_seconds: i64, nanosecond: u32) -> Result<Self, PlanError> {
        if nanosecond >= 1_000_000_000 {
            return Err(PlanError::InvalidNanosecond);
        }
        Ok(Self {
            unix_seconds,
            nanosecond,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CatalogSnapshot {
    pub catalog_id: String,
    pub manifests: Vec<ManifestFingerprint>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceSnapshot {
    pub source_id: String,
    pub fingerprint: ContentDigest,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManifestFingerprint {
    pub path: Vec<String>,
    pub digest: ContentDigest,
}

#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum PlanFileRole {
    Primary,
    Companion,
    Sidecar,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanFile {
    pub source_file_id: String,
    pub source_path: Vec<String>,
    pub digest: ContentDigest,
    pub size_bytes: u64,
    pub role: PlanFileRole,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanDateTime {
    pub year: i32,
    pub ordinal: u16,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub recorded_offset_seconds: Option<i32>,
    pub provenance: PlanDateTimeProvenance,
}

#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum PlanDateTimeSource {
    ExifDateTimeOriginal,
    ExifDateTimeDigitized,
    QuickTimeCreationDate,
    XmpDateCreated,
    XmpExifDateTimeOriginal,
    ContainerCreationTime,
    GoogleTakeoutPhotoTakenTime,
    FilenamePattern,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PlanDateTimeProvenance {
    Metadata {
        source: PlanDateTimeSource,
    },
    Confirmed {
        source: PlanDateTimeSource,
    },
    UserAssigned,
    Corrected {
        original: Box<PlanDateTimeProvenance>,
        offset_seconds: i64,
    },
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanItemFinding {
    MissingDate,
    PendingDateConfirmation,
    AmbiguousRawJpeg,
    AmbiguousLivePhoto,
    AmbiguousCanonicalCompound,
    ConflictingCompound,
    AmbiguousSidecar,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PlanItemDisposition {
    Transfer,
    Excluded { reason: String },
    Unresolved { finding: PlanItemFinding },
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanItem {
    pub item_id: String,
    pub files: Vec<PlanFile>,
    pub accepted_datetime: Option<PlanDateTime>,
    pub disposition: PlanItemDisposition,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum UnhandledContentFinding {
    UnknownContent,
    OrphanSidecar,
    AmbiguousSidecar { candidate_item_ids: Vec<String> },
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnhandledContent {
    pub source_file_id: String,
    pub source_path: Vec<String>,
    pub digest: ContentDigest,
    pub size_bytes: u64,
    pub finding: UnhandledContentFinding,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PlanStepKind {
    CreateDirectory {
        path: Vec<String>,
    },
    CopyFile {
        source_file_id: String,
        destination: Vec<String>,
        expected_digest: ContentDigest,
        size_bytes: u64,
    },
    WriteManifest {
        path: Vec<String>,
        previous_digest: Option<ContentDigest>,
        resulting_digest: ContentDigest,
        resulting_size_bytes: u64,
    },
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanStep {
    pub sequence: u32,
    pub action: PlanStepKind,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
/// Field order is part of the version 1 digest contract.
pub struct PlanDocument {
    #[schemars(range(min = 1, max = 1))]
    pub schema_version: u32,
    pub plan_id: PlanId,
    pub created_at: PlanTimestamp,
    pub catalog: CatalogSnapshot,
    pub source: SourceSnapshot,
    pub items: Vec<PlanItem>,
    pub unhandled_content: Vec<UnhandledContent>,
    pub steps: Vec<PlanStep>,
    pub required_space_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportPlan {
    document: PlanDocument,
    digest: ContentDigest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawImportPlan {
    document: PlanDocument,
    digest: ContentDigest,
}

impl<'de> Deserialize<'de> for ImportPlan {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = RawImportPlan::deserialize(deserializer)?;
        Self::from_raw(raw).map_err(serde::de::Error::custom)
    }
}

impl ImportPlan {
    pub fn new(mut document: PlanDocument) -> Result<Self, PlanError> {
        normalize_document(&mut document);
        validate_document(&document)?;
        let digest = document_digest(&document)?;
        Ok(Self { document, digest })
    }

    pub fn to_json(&self) -> Result<Vec<u8>, PlanError> {
        self.verify()?;
        Ok(serde_json::to_vec(self)?)
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, PlanError> {
        let raw: RawImportPlan = serde_json::from_slice(bytes)?;
        Self::from_raw(raw)
    }

    fn from_raw(raw: RawImportPlan) -> Result<Self, PlanError> {
        let plan = Self {
            document: raw.document,
            digest: raw.digest,
        };
        validate_document(&plan.document)?;
        plan.verify()?;
        Ok(plan)
    }

    pub fn document(&self) -> &PlanDocument {
        &self.document
    }

    pub fn digest(&self) -> &ContentDigest {
        &self.digest
    }

    pub fn verify(&self) -> Result<(), PlanError> {
        let actual = document_digest(&self.document)?;
        if actual != self.digest {
            return Err(PlanError::DigestMismatch {
                expected: self.digest.clone(),
                actual,
            });
        }
        Ok(())
    }

    pub fn validate_drift(
        &self,
        observed: &ObservedPlanState,
    ) -> Result<Vec<DriftFinding>, PlanError> {
        let mut findings = Vec::new();
        if self.document.catalog.catalog_id != observed.catalog_id {
            findings.push(DriftFinding::CatalogIdentityChanged);
        }
        if self.document.source.source_id != observed.source_id {
            findings.push(DriftFinding::SourceIdentityChanged);
        }
        if self.document.source.fingerprint != observed.source_fingerprint {
            findings.push(DriftFinding::SourceFingerprintChanged);
        }

        let expected: BTreeMap<_, _> = self
            .document
            .catalog
            .manifests
            .iter()
            .map(|manifest| (manifest.path.clone(), &manifest.digest))
            .collect();
        let mut actual = BTreeMap::new();
        for manifest in &observed.manifests {
            portable_path(&manifest.path)?;
            if actual
                .insert(manifest.path.clone(), &manifest.digest)
                .is_some()
            {
                return Err(PlanError::DuplicateObservedManifest(manifest.path.clone()));
            }
        }
        for (path, digest) in &expected {
            match actual.get(path) {
                None => findings.push(DriftFinding::ManifestMissing { path: path.clone() }),
                Some(actual) if *actual != *digest => {
                    findings.push(DriftFinding::ManifestChanged { path: path.clone() });
                }
                Some(_) => {}
            }
        }
        for path in actual.keys().filter(|path| !expected.contains_key(*path)) {
            findings.push(DriftFinding::UnexpectedManifest { path: path.clone() });
        }

        let occupied: BTreeSet<PathCollisionKey> = observed
            .occupied_destinations
            .iter()
            .map(|path| portable_path(path).map(|path| path.collision_key()))
            .collect::<Result<_, _>>()?;
        for destination in self.destinations_requiring_free() {
            if portable_path(destination).is_ok_and(|path| occupied.contains(&path.collision_key()))
            {
                findings.push(DriftFinding::DestinationOccupied {
                    path: destination.to_vec(),
                });
            }
        }
        findings.sort();
        findings.dedup();
        Ok(findings)
    }

    pub fn destinations_requiring_free(&self) -> impl Iterator<Item = &[String]> {
        self.document
            .steps
            .iter()
            .filter_map(|step| match &step.action {
                PlanStepKind::CreateDirectory { path } => Some(path.as_slice()),
                PlanStepKind::CopyFile { destination, .. } => Some(destination.as_slice()),
                PlanStepKind::WriteManifest {
                    path,
                    previous_digest: None,
                    ..
                } => Some(path.as_slice()),
                PlanStepKind::WriteManifest {
                    previous_digest: Some(_),
                    ..
                } => None,
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedPlanState {
    pub catalog_id: String,
    pub source_id: String,
    pub source_fingerprint: ContentDigest,
    pub manifests: Vec<ManifestFingerprint>,
    pub occupied_destinations: Vec<Vec<String>>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum DriftFinding {
    CatalogIdentityChanged,
    SourceIdentityChanged,
    SourceFingerprintChanged,
    ManifestMissing { path: Vec<String> },
    ManifestChanged { path: Vec<String> },
    UnexpectedManifest { path: Vec<String> },
    DestinationOccupied { path: Vec<String> },
}

pub fn import_plan_schema() -> RootSchema {
    import_plan_schema_for_version(IMPORT_PLAN_SCHEMA_VERSION)
        .expect("current import plan schema version is supported")
}

pub fn import_plan_schema_for_version(version: u32) -> Result<RootSchema, PlanError> {
    let mut schema = schema_for!(ImportPlan);
    constrain_import_plan_schema(&mut schema, version)?;
    Ok(schema)
}

pub fn constrain_import_plan_schema(
    schema: &mut RootSchema,
    version: u32,
) -> Result<(), PlanError> {
    if !(1..=IMPORT_PLAN_SCHEMA_VERSION).contains(&version) {
        return Err(PlanError::UnsupportedSchemaVersion(version));
    }
    let mut value = serde_json::to_value(&*schema)?;
    value["definitions"]["PlanDocument"]["properties"]["schema_version"]["minimum"] =
        serde_json::json!(f64::from(version));
    value["definitions"]["PlanDocument"]["properties"]["schema_version"]["maximum"] =
        serde_json::json!(f64::from(version));
    if version == 1 {
        let values = value["definitions"]["PlanItemFinding"]["enum"]
            .as_array_mut()
            .expect("plan item finding schema has enum values");
        values.retain(|value| value != "ambiguous_sidecar");
    } else {
        let variants = value["definitions"]["UnhandledContentFinding"]["oneOf"]
            .as_array_mut()
            .expect("unhandled content schema has variants");
        let variant = variants
            .iter_mut()
            .find(|variant| {
                variant["properties"]["kind"]["enum"] == serde_json::json!(["ambiguous_sidecar"])
            })
            .expect("ambiguous sidecar schema variant exists");
        variant["properties"]["candidate_item_ids"]["minItems"] = serde_json::json!(2);
        variant["properties"]["candidate_item_ids"]["uniqueItems"] = serde_json::json!(true);
    }
    *schema = serde_json::from_value(value)?;
    Ok(())
}

fn normalize_document(document: &mut PlanDocument) {
    document
        .catalog
        .manifests
        .sort_by(|left, right| left.path.cmp(&right.path));
    document
        .items
        .sort_by(|left, right| left.item_id.cmp(&right.item_id));
    for item in &mut document.items {
        item.files.sort_by(|left, right| {
            (&left.source_path, &left.source_file_id)
                .cmp(&(&right.source_path, &right.source_file_id))
        });
    }
    document.unhandled_content.sort_by(|left, right| {
        (&left.source_path, &left.source_file_id).cmp(&(&right.source_path, &right.source_file_id))
    });
    for unhandled in &mut document.unhandled_content {
        if let UnhandledContentFinding::AmbiguousSidecar { candidate_item_ids } =
            &mut unhandled.finding
        {
            candidate_item_ids.sort();
        }
    }
    document.steps.sort_by_key(|step| step.sequence);
}

fn validate_document(document: &PlanDocument) -> Result<(), PlanError> {
    if !(1..=IMPORT_PLAN_SCHEMA_VERSION).contains(&document.schema_version) {
        return Err(PlanError::UnsupportedSchemaVersion(document.schema_version));
    }
    if document.catalog.catalog_id.is_empty() {
        return Err(PlanError::EmptyIdentifier("catalog"));
    }
    if document.source.source_id.is_empty() {
        return Err(PlanError::EmptyIdentifier("source"));
    }
    if document.created_at.nanosecond >= 1_000_000_000 {
        return Err(PlanError::InvalidNanosecond);
    }
    ensure_strictly_sorted(
        document
            .catalog
            .manifests
            .iter()
            .map(|manifest| &manifest.path),
        "manifest paths",
    )?;
    for manifest in &document.catalog.manifests {
        portable_path(&manifest.path)?;
    }

    ensure_strictly_sorted(
        document.items.iter().map(|item| &item.item_id),
        "item identifiers",
    )?;
    let item_ids: BTreeSet<_> = document
        .items
        .iter()
        .map(|item| item.item_id.as_str())
        .collect();
    let mut source_ids = BTreeSet::new();
    let mut source_paths = BTreeSet::new();
    let mut transferable_files = BTreeMap::new();
    let mut ambiguous_sidecar_items = BTreeSet::new();
    for item in &document.items {
        if item.item_id.is_empty() {
            return Err(PlanError::EmptyIdentifier("item"));
        }
        if item.files.is_empty() {
            return Err(PlanError::ItemWithoutFiles(item.item_id.clone()));
        }
        ensure_strictly_sorted(
            item.files
                .iter()
                .map(|file| (&file.source_path, &file.source_file_id)),
            "item files",
        )?;
        let primary_count = item
            .files
            .iter()
            .filter(|file| file.role == PlanFileRole::Primary)
            .count();
        if primary_count != 1 {
            return Err(PlanError::InvalidPrimaryCount {
                item_id: item.item_id.clone(),
                count: primary_count,
            });
        }
        if matches!(item.disposition, PlanItemDisposition::Transfer)
            && item.accepted_datetime.is_none()
        {
            return Err(PlanError::TransferWithoutDate(item.item_id.clone()));
        }
        if document.schema_version == 1
            && matches!(
                item.disposition,
                PlanItemDisposition::Unresolved {
                    finding: PlanItemFinding::AmbiguousSidecar
                }
            )
        {
            return Err(PlanError::FindingRequiresSchemaVersion {
                item_id: item.item_id.clone(),
                finding: "ambiguous_sidecar",
                minimum_version: 2,
            });
        }
        if matches!(
            item.disposition,
            PlanItemDisposition::Unresolved {
                finding: PlanItemFinding::AmbiguousSidecar
            }
        ) {
            ambiguous_sidecar_items.insert(item.item_id.as_str());
        }
        for file in &item.files {
            if file.source_file_id.is_empty() {
                return Err(PlanError::EmptyIdentifier("source file"));
            }
            if file.source_path.is_empty() || file.source_path.last().is_some_and(String::is_empty)
            {
                return Err(PlanError::InvalidSourcePath);
            }
            SourcePath::new(file.source_path.clone()).map_err(|_| PlanError::InvalidSourcePath)?;
            if !source_ids.insert(&file.source_file_id) {
                return Err(PlanError::DuplicateSourceFileId(
                    file.source_file_id.clone(),
                ));
            }
            if !source_paths.insert(&file.source_path) {
                return Err(PlanError::DuplicateSourcePath(file.source_path.clone()));
            }
            if matches!(item.disposition, PlanItemDisposition::Transfer) {
                transferable_files.insert(
                    file.source_file_id.clone(),
                    (file.digest.clone(), file.size_bytes, file.role),
                );
            }
        }
        if let Some(datetime) = &item.accepted_datetime {
            validate_datetime(datetime)?;
        }
    }

    ensure_strictly_sorted(
        document
            .unhandled_content
            .iter()
            .map(|file| (&file.source_path, &file.source_file_id)),
        "unhandled content",
    )?;
    let mut ambiguous_sidecar_evidence = BTreeSet::new();
    for file in &document.unhandled_content {
        if file.source_file_id.is_empty() {
            return Err(PlanError::EmptyIdentifier("source file"));
        }
        SourcePath::new(file.source_path.clone()).map_err(|_| PlanError::InvalidSourcePath)?;
        if !source_ids.insert(&file.source_file_id) {
            return Err(PlanError::DuplicateSourceFileId(
                file.source_file_id.clone(),
            ));
        }
        if !source_paths.insert(&file.source_path) {
            return Err(PlanError::DuplicateSourcePath(file.source_path.clone()));
        }
        if let UnhandledContentFinding::AmbiguousSidecar { candidate_item_ids } = &file.finding {
            ensure_strictly_sorted(candidate_item_ids, "ambiguous sidecar candidates")?;
            if candidate_item_ids.len() < 2
                || candidate_item_ids
                    .iter()
                    .any(|candidate| !item_ids.contains(candidate.as_str()))
            {
                return Err(PlanError::InvalidAmbiguousSidecar(
                    file.source_file_id.clone(),
                ));
            }
            ambiguous_sidecar_evidence.extend(candidate_item_ids.iter().map(String::as_str));
        }
    }
    if let Some(item_id) = ambiguous_sidecar_items
        .difference(&ambiguous_sidecar_evidence)
        .next()
    {
        return Err(PlanError::InvalidAmbiguousSidecar((*item_id).to_owned()));
    }

    ensure_strictly_sorted(
        document.steps.iter().map(|step| step.sequence),
        "plan steps",
    )?;
    let manifest_fingerprints: BTreeMap<_, _> = document
        .catalog
        .manifests
        .iter()
        .map(|manifest| (&manifest.path, &manifest.digest))
        .collect();
    let mut destinations = BTreeMap::new();
    let mut copied_files = BTreeSet::new();
    let mut required_manifest_paths = BTreeSet::new();
    let mut written_manifest_paths = BTreeSet::new();
    let mut required_bytes = 0_u64;
    for step in &document.steps {
        match &step.action {
            PlanStepKind::CreateDirectory { path } => {
                let path_value = portable_path(path)?;
                if let Some(first) = destinations.insert(path_value.collision_key(), path.clone()) {
                    return Err(PlanError::DestinationCollision {
                        first,
                        second: path.clone(),
                    });
                }
            }
            PlanStepKind::CopyFile {
                source_file_id,
                destination,
                expected_digest,
                size_bytes,
            } => {
                let Some((file_digest, file_size, role)) = transferable_files.get(source_file_id)
                else {
                    return Err(PlanError::SourceFileNotTransferable(source_file_id.clone()));
                };
                if !copied_files.insert(source_file_id) {
                    return Err(PlanError::DuplicateCopyStep(source_file_id.clone()));
                }
                if expected_digest != file_digest || size_bytes != file_size {
                    return Err(PlanError::CopyStepMismatch(source_file_id.clone()));
                }
                let path = portable_path(destination)?;
                if let Some(first) = destinations.insert(path.collision_key(), destination.clone())
                {
                    return Err(PlanError::DestinationCollision {
                        first,
                        second: destination.clone(),
                    });
                }
                required_bytes = required_bytes
                    .checked_add(*size_bytes)
                    .ok_or(PlanError::RequiredSpaceOverflow)?;
                if *role != PlanFileRole::Sidecar {
                    let mut manifest_path = destination[..destination.len() - 1].to_vec();
                    manifest_path.push("SHA256SUMS".into());
                    required_manifest_paths.insert(manifest_path);
                }
            }
            PlanStepKind::WriteManifest {
                path,
                previous_digest,
                resulting_size_bytes,
                ..
            } => {
                if path.last().map(String::as_str) != Some("SHA256SUMS") {
                    return Err(PlanError::InvalidManifestPath(path.clone()));
                }
                let path_value = portable_path(path)?;
                if let Some(first) = destinations.insert(path_value.collision_key(), path.clone()) {
                    return Err(PlanError::DestinationCollision {
                        first,
                        second: path.clone(),
                    });
                }
                let expected_previous = manifest_fingerprints.get(path).copied();
                if previous_digest.as_ref() != expected_previous {
                    return Err(PlanError::ManifestBaselineMismatch(path.clone()));
                }
                written_manifest_paths.insert(path.clone());
                required_bytes = required_bytes
                    .checked_add(*resulting_size_bytes)
                    .ok_or(PlanError::RequiredSpaceOverflow)?;
            }
        }
    }
    for source_file_id in transferable_files.keys() {
        if !copied_files.contains(source_file_id) {
            return Err(PlanError::MissingCopyStep(source_file_id.clone()));
        }
    }
    if let Some(path) = required_manifest_paths
        .difference(&written_manifest_paths)
        .next()
    {
        return Err(PlanError::MissingManifestWrite(path.clone()));
    }
    if required_bytes != document.required_space_bytes {
        return Err(PlanError::RequiredSpaceMismatch {
            declared: document.required_space_bytes,
            calculated: required_bytes,
        });
    }
    Ok(())
}

fn validate_datetime(datetime: &PlanDateTime) -> Result<(), PlanError> {
    let valid = (0..=9999).contains(&datetime.year)
        && Date::from_ordinal_date(datetime.year, datetime.ordinal).is_ok()
        && datetime.hour <= 23
        && datetime.minute <= 59
        && datetime.second <= 59
        && datetime
            .recorded_offset_seconds
            .map_or(true, |offset| (-86_399..=86_399).contains(&offset))
        && provenance_is_valid(&datetime.provenance);
    if valid {
        Ok(())
    } else {
        Err(PlanError::InvalidAcceptedDateTime)
    }
}

fn provenance_is_valid(provenance: &PlanDateTimeProvenance) -> bool {
    match provenance {
        PlanDateTimeProvenance::Corrected {
            original,
            offset_seconds,
        } => *offset_seconds != 0 && provenance_is_valid(original),
        PlanDateTimeProvenance::Metadata { .. }
        | PlanDateTimeProvenance::Confirmed { .. }
        | PlanDateTimeProvenance::UserAssigned => true,
    }
}

fn portable_path(components: &[String]) -> Result<PortableRelativePath, PlanError> {
    PortableRelativePath::new(components).map_err(|error| PlanError::InvalidPortablePath {
        path: components.to_vec(),
        reason: error.to_string(),
    })
}

fn ensure_strictly_sorted<T, I>(values: I, field: &'static str) -> Result<(), PlanError>
where
    T: Ord,
    I: IntoIterator<Item = T>,
{
    let mut previous = None;
    for value in values {
        if previous.is_some_and(|previous| previous >= value) {
            return Err(PlanError::NonCanonicalOrder(field));
        }
        previous = Some(value);
    }
    Ok(())
}

fn document_digest(document: &PlanDocument) -> Result<ContentDigest, PlanError> {
    Ok(ContentDigest::from_bytes(&canonical_document_bytes(
        document,
    )?))
}

fn canonical_document_bytes(document: &PlanDocument) -> Result<Vec<u8>, PlanError> {
    Ok(serde_json::to_vec(document)?)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(char::from(HEX[usize::from(byte >> 4)]));
        result.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    result
}

#[derive(Debug)]
pub enum PlanError {
    EmptyIdentifier(&'static str),
    InvalidPlanId,
    InvalidDigest,
    InvalidNanosecond,
    UnsupportedSchemaVersion(u32),
    NonCanonicalOrder(&'static str),
    InvalidPortablePath {
        path: Vec<String>,
        reason: String,
    },
    InvalidSourcePath,
    InvalidAcceptedDateTime,
    DuplicateSourceFileId(String),
    DuplicateSourcePath(Vec<String>),
    ItemWithoutFiles(String),
    InvalidPrimaryCount {
        item_id: String,
        count: usize,
    },
    FindingRequiresSchemaVersion {
        item_id: String,
        finding: &'static str,
        minimum_version: u32,
    },
    TransferWithoutDate(String),
    SourceFileNotTransferable(String),
    DuplicateCopyStep(String),
    MissingCopyStep(String),
    CopyStepMismatch(String),
    DuplicateObservedManifest(Vec<String>),
    InvalidAmbiguousSidecar(String),
    ManifestBaselineMismatch(Vec<String>),
    InvalidManifestPath(Vec<String>),
    MissingManifestWrite(Vec<String>),
    DestinationCollision {
        first: Vec<String>,
        second: Vec<String>,
    },
    RequiredSpaceOverflow,
    RequiredSpaceMismatch {
        declared: u64,
        calculated: u64,
    },
    DigestMismatch {
        expected: ContentDigest,
        actual: ContentDigest,
    },
    Serialization(serde_json::Error),
}

impl fmt::Display for PlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyIdentifier(kind) => write!(formatter, "{kind} identifier must not be empty"),
            Self::InvalidPlanId => {
                formatter.write_str("plan identifier contains a control or path separator")
            }
            Self::InvalidDigest => formatter.write_str("digest must be 64 lowercase hexadecimal digits"),
            Self::InvalidNanosecond => formatter.write_str("nanosecond must be less than one billion"),
            Self::UnsupportedSchemaVersion(version) => {
                write!(formatter, "unsupported import plan schema version {version}")
            }
            Self::NonCanonicalOrder(field) => write!(formatter, "{field} are not in canonical order"),
            Self::InvalidPortablePath { path, reason } => {
                write!(formatter, "invalid portable path '{}': {reason}", path.join("/"))
            }
            Self::InvalidSourcePath => formatter.write_str("source path must contain a filename"),
            Self::InvalidAcceptedDateTime => formatter.write_str("accepted datetime is invalid"),
            Self::DuplicateSourceFileId(id) => write!(formatter, "duplicate source file identifier '{id}'"),
            Self::DuplicateSourcePath(path) => write!(formatter, "duplicate source path '{}'", path.join("/")),
            Self::ItemWithoutFiles(id) => write!(formatter, "plan item '{id}' has no files"),
            Self::InvalidPrimaryCount { item_id, count } => {
                write!(formatter, "plan item '{item_id}' has {count} primary files")
            }
            Self::FindingRequiresSchemaVersion {
                item_id,
                finding,
                minimum_version,
            } => write!(
                formatter,
                "plan item '{item_id}' finding '{finding}' requires schema version {minimum_version}"
            ),
            Self::TransferWithoutDate(id) => {
                write!(formatter, "transferred plan item '{id}' has no accepted datetime")
            }
            Self::SourceFileNotTransferable(id) => {
                write!(formatter, "copy step references non-transferable source file '{id}'")
            }
            Self::DuplicateCopyStep(id) => {
                write!(formatter, "source file '{id}' has more than one copy step")
            }
            Self::MissingCopyStep(id) => {
                write!(formatter, "transferred source file '{id}' has no copy step")
            }
            Self::CopyStepMismatch(id) => {
                write!(formatter, "copy step does not match source file '{id}'")
            }
            Self::DuplicateObservedManifest(path) => {
                write!(formatter, "observed manifest '{}' is duplicated", path.join("/"))
            }
            Self::InvalidAmbiguousSidecar(id) => {
                write!(
                    formatter,
                    "ambiguous sidecar evidence for '{id}' must reference at least two plan items and cover every ambiguous item"
                )
            }
            Self::ManifestBaselineMismatch(path) => {
                write!(
                    formatter,
                    "manifest step baseline does not match snapshot for '{}'",
                    path.join("/")
                )
            }
            Self::InvalidManifestPath(path) => {
                write!(
                    formatter,
                    "manifest step path '{}' must end in SHA256SUMS",
                    path.join("/")
                )
            }
            Self::MissingManifestWrite(path) => {
                write!(
                    formatter,
                    "copied original requires manifest write '{}'",
                    path.join("/")
                )
            }
            Self::DestinationCollision { first, second } => write!(
                formatter,
                "planned destinations '{}' and '{}' collide",
                first.join("/"),
                second.join("/")
            ),
            Self::RequiredSpaceOverflow => formatter.write_str("required space exceeds the supported range"),
            Self::RequiredSpaceMismatch { declared, calculated } => write!(
                formatter,
                "declared required space {declared} does not match calculated copy size {calculated}"
            ),
            Self::DigestMismatch { expected, actual } => write!(
                formatter,
                "plan digest mismatch: expected {}, calculated {}",
                expected.as_str(),
                actual.as_str()
            ),
            Self::Serialization(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for PlanError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialization(error) => Some(error),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for PlanError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn digest(value: &str) -> ContentDigest {
        ContentDigest::from_bytes(value.as_bytes())
    }

    fn document() -> PlanDocument {
        PlanDocument {
            schema_version: IMPORT_PLAN_SCHEMA_VERSION,
            plan_id: PlanId::new("plan-1").unwrap(),
            created_at: PlanTimestamp::new(1_725_000_000, 123).unwrap(),
            catalog: CatalogSnapshot {
                catalog_id: "catalog-1".into(),
                manifests: vec![
                    ManifestFingerprint {
                        path: vec!["2024".into(), "collection".into(), "SHA256SUMS".into()],
                        digest: digest("manifest"),
                    },
                    ManifestFingerprint {
                        path: vec!["2023".into(), "collection".into(), "SHA256SUMS".into()],
                        digest: digest("older manifest"),
                    },
                ],
            },
            source: SourceSnapshot {
                source_id: "source-1".into(),
                fingerprint: digest("source snapshot"),
            },
            items: vec![
                PlanItem {
                    item_id: "item-b".into(),
                    files: vec![PlanFile {
                        source_file_id: "file-b".into(),
                        source_path: vec!["card".into(), "b.jpg".into()],
                        digest: digest("b"),
                        size_bytes: 20,
                        role: PlanFileRole::Primary,
                    }],
                    accepted_datetime: None,
                    disposition: PlanItemDisposition::Excluded {
                        reason: "duplicate".into(),
                    },
                },
                PlanItem {
                    item_id: "item-a".into(),
                    files: vec![
                        PlanFile {
                            source_file_id: "file-sidecar".into(),
                            source_path: vec!["card".into(), "a.xmp".into()],
                            digest: digest("sidecar"),
                            size_bytes: 5,
                            role: PlanFileRole::Sidecar,
                        },
                        PlanFile {
                            source_file_id: "file-a".into(),
                            source_path: vec!["card".into(), "a.jpg".into()],
                            digest: digest("a"),
                            size_bytes: 10,
                            role: PlanFileRole::Primary,
                        },
                    ],
                    accepted_datetime: Some(PlanDateTime {
                        year: 2024,
                        ordinal: 221,
                        hour: 12,
                        minute: 34,
                        second: 56,
                        recorded_offset_seconds: Some(3_600),
                        provenance: PlanDateTimeProvenance::Corrected {
                            original: Box::new(PlanDateTimeProvenance::Confirmed {
                                source: PlanDateTimeSource::ExifDateTimeOriginal,
                            }),
                            offset_seconds: 120,
                        },
                    }),
                    disposition: PlanItemDisposition::Transfer,
                },
            ],
            unhandled_content: vec![UnhandledContent {
                source_file_id: "unknown".into(),
                source_path: vec!["card".into(), "notes.txt".into()],
                digest: digest("unknown"),
                size_bytes: 7,
                finding: UnhandledContentFinding::UnknownContent,
            }],
            steps: vec![
                PlanStep {
                    sequence: 3,
                    action: PlanStepKind::WriteManifest {
                        path: vec!["2024".into(), "collection".into(), "SHA256SUMS".into()],
                        previous_digest: Some(digest("manifest")),
                        resulting_digest: digest("result"),
                        resulting_size_bytes: 100,
                    },
                },
                PlanStep {
                    sequence: 1,
                    action: PlanStepKind::CopyFile {
                        source_file_id: "file-a".into(),
                        destination: vec![
                            "2024".into(),
                            "collection".into(),
                            "2024-08-08 12-34-56 001.jpg".into(),
                        ],
                        expected_digest: digest("a"),
                        size_bytes: 10,
                    },
                },
                PlanStep {
                    sequence: 2,
                    action: PlanStepKind::CopyFile {
                        source_file_id: "file-sidecar".into(),
                        destination: vec![
                            "2024".into(),
                            "collection".into(),
                            "2024-08-08 12-34-56 001.jpg.xmp".into(),
                        ],
                        expected_digest: digest("sidecar"),
                        size_bytes: 5,
                    },
                },
            ],
            required_space_bytes: 115,
        }
    }

    #[test]
    fn serialization_round_trip_preserves_the_complete_plan() {
        let plan = ImportPlan::new(document()).unwrap();
        let json = plan.to_json().unwrap();
        let restored = ImportPlan::from_json(&json).unwrap();

        assert_eq!(restored, plan);
        assert_eq!(restored.to_json().unwrap(), json);
    }

    #[test]
    fn equivalent_traversal_orders_produce_identical_plans() {
        let forward = ImportPlan::new(document()).unwrap();
        let mut reversed = document();
        reversed.catalog.manifests.reverse();
        reversed.items.reverse();
        for item in &mut reversed.items {
            item.files.reverse();
        }
        reversed.unhandled_content.reverse();
        reversed.steps.reverse();
        let reverse = ImportPlan::new(reversed).unwrap();

        assert_eq!(forward, reverse);
        assert_eq!(forward.to_json().unwrap(), reverse.to_json().unwrap());
    }

    proptest! {
        #[test]
        fn serialization_is_independent_of_input_collection_order(
            reverse_manifests in any::<bool>(),
            reverse_items in any::<bool>(),
            reverse_files in any::<bool>(),
            reverse_steps in any::<bool>(),
        ) {
            let expected = ImportPlan::new(document()).unwrap();
            let mut reordered = document();
            if reverse_manifests {
                reordered.catalog.manifests.reverse();
            }
            if reverse_items {
                reordered.items.reverse();
                reordered.unhandled_content.reverse();
            }
            if reverse_files {
                for item in &mut reordered.items {
                    item.files.reverse();
                }
            }
            if reverse_steps {
                reordered.steps.reverse();
            }

            let actual = ImportPlan::new(reordered).unwrap();

            prop_assert_eq!(actual.to_json().unwrap(), expected.to_json().unwrap());
        }
    }

    #[test]
    fn tampering_is_rejected_by_the_complete_plan_digest() {
        let plan = ImportPlan::new(document()).unwrap();
        let mut value = serde_json::to_value(plan).unwrap();
        value["document"]["required_space_bytes"] = serde_json::json!(11);
        let json = serde_json::to_vec(&value).unwrap();

        assert!(matches!(
            ImportPlan::from_json(&json),
            Err(PlanError::RequiredSpaceMismatch { .. } | PlanError::DigestMismatch { .. })
        ));
    }

    #[test]
    fn every_top_level_document_field_is_inside_the_digest_boundary() {
        let original = ImportPlan::new(document()).unwrap();
        let expected = original.digest().clone();
        let mut variants = Vec::new();

        let mut changed = document();
        changed.schema_version = 2;
        variants.push(changed);
        let mut changed = document();
        changed.plan_id = PlanId::new("other-plan").unwrap();
        variants.push(changed);
        let mut changed = document();
        changed.created_at = PlanTimestamp::new(-1, 0).unwrap();
        variants.push(changed);
        let mut changed = document();
        changed.catalog.catalog_id = "other-catalog".into();
        variants.push(changed);
        let mut changed = document();
        changed.source.source_id = "other-source".into();
        variants.push(changed);
        let mut changed = document();
        changed.items[0].disposition = PlanItemDisposition::Excluded {
            reason: "other reason".into(),
        };
        variants.push(changed);
        let mut changed = document();
        changed.unhandled_content[0].size_bytes += 1;
        variants.push(changed);
        let mut changed = document();
        changed.steps[0].sequence = 4;
        variants.push(changed);
        let mut changed = document();
        changed.required_space_bytes += 1;
        variants.push(changed);

        for changed in variants {
            assert_ne!(document_digest(&changed).unwrap(), expected);
        }
    }

    #[test]
    fn version_one_canonical_bytes_cover_json_encoding_rules() {
        let mut document = document();
        document.schema_version = 1;
        document.plan_id = PlanId::new("plan-\"café\"").unwrap();
        document.created_at = PlanTimestamp::new(-1, 0).unwrap();
        document.catalog.catalog_id = "catalog-雪".into();
        document.catalog.manifests.clear();
        document.source.source_id = "source-card".into();
        document.items = vec![PlanItem {
            item_id: "item-1".into(),
            files: vec![PlanFile {
                source_file_id: "file-1".into(),
                source_path: vec!["DCIM".into(), "café \"one\".jpg".into()],
                digest: digest("file"),
                size_bytes: 1,
                role: PlanFileRole::Primary,
            }],
            accepted_datetime: None,
            disposition: PlanItemDisposition::Excluded {
                reason: "not selected\nline".into(),
            },
        }];
        document.unhandled_content.clear();
        document.steps.clear();
        document.required_space_bytes = 0;
        let plan = ImportPlan::new(document).unwrap();

        let bytes = canonical_document_bytes(plan.document()).unwrap();

        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            concat!(
                "{\"schema_version\":1,\"plan_id\":\"plan-\\\"café\\\"\",",
                "\"created_at\":{\"unix_seconds\":-1,\"nanosecond\":0},",
                "\"catalog\":{\"catalog_id\":\"catalog-雪\",\"manifests\":[]},",
                "\"source\":{\"source_id\":\"source-card\",\"fingerprint\":",
                "\"41637c7875947b50775c0ab2bb30a28b292be748376a5713eb8f527d9953433c\"},",
                "\"items\":[{\"item_id\":\"item-1\",\"files\":[{\"source_file_id\":",
                "\"file-1\",\"source_path\":[\"DCIM\",\"café \\\"one\\\".jpg\"],",
                "\"digest\":\"3b9c358f36f0a31b6ad3e14f309c7cf198ac9246e8316f9ce543d5b19ac02b80\",",
                "\"size_bytes\":1,\"role\":\"primary\"}],\"accepted_datetime\":null,",
                "\"disposition\":{\"kind\":\"excluded\",\"reason\":\"not selected\\nline\"}}],",
                "\"unhandled_content\":[],\"steps\":[],\"required_space_bytes\":0}"
            )
        );
        assert_eq!(
            plan.digest().as_str(),
            "c9df6ebdc9a50f2b21ebcfa128bbc29586f36f2755f7609098e4b09f935ad9e3"
        );
    }

    #[test]
    fn version_one_rich_document_digest_is_stable() {
        let mut document = document();
        document.schema_version = 1;
        let plan = ImportPlan::new(document).unwrap();

        assert_eq!(
            plan.digest().as_str(),
            "8169e208a2d7301f2727c1cae13abf7dd0a3f27dafe4993a293e95d7819314d5"
        );
    }

    #[test]
    fn deserialization_rejects_noncanonical_step_order_even_with_a_matching_digest() {
        let mut plan = ImportPlan::new(document()).unwrap();
        plan.document.steps.reverse();
        plan.digest = document_digest(&plan.document).unwrap();
        let json = serde_json::to_vec(&plan).unwrap();

        assert!(matches!(
            ImportPlan::from_json(&json),
            Err(PlanError::NonCanonicalOrder("plan steps"))
        ));
    }

    #[test]
    fn drift_reports_identity_manifest_source_and_destination_changes() {
        let plan = ImportPlan::new(document()).unwrap();
        let observed = ObservedPlanState {
            catalog_id: "other-catalog".into(),
            source_id: "other-source".into(),
            source_fingerprint: digest("changed source"),
            manifests: vec![
                ManifestFingerprint {
                    path: vec!["2024".into(), "collection".into(), "SHA256SUMS".into()],
                    digest: digest("changed manifest"),
                },
                ManifestFingerprint {
                    path: vec!["new".into(), "collection".into(), "SHA256SUMS".into()],
                    digest: digest("unexpected"),
                },
            ],
            occupied_destinations: vec![vec![
                "2024".into(),
                "COLLECTION".into(),
                "2024-08-08 12-34-56 001.JPG".into(),
            ]],
        };

        let findings = plan.validate_drift(&observed).unwrap();

        assert!(findings.contains(&DriftFinding::CatalogIdentityChanged));
        assert!(findings.contains(&DriftFinding::SourceIdentityChanged));
        assert!(findings.contains(&DriftFinding::SourceFingerprintChanged));
        assert!(findings.iter().any(
            |finding| matches!(finding, DriftFinding::ManifestChanged { path } if path[0] == "2024")
        ));
        assert!(findings.iter().any(
            |finding| matches!(finding, DriftFinding::ManifestMissing { path } if path[0] == "2023")
        ));
        assert!(findings
            .iter()
            .any(|finding| matches!(finding, DriftFinding::UnexpectedManifest { .. })));
        assert!(findings
            .iter()
            .any(|finding| matches!(finding, DriftFinding::DestinationOccupied { .. })));
    }

    #[test]
    fn drift_reports_portably_occupied_directory_destinations() {
        let mut document = document();
        document.steps.push(PlanStep {
            sequence: 4,
            action: PlanStepKind::CreateDirectory {
                path: vec!["2024".into(), "Café".into()],
            },
        });
        let plan = ImportPlan::new(document).unwrap();
        let observed = ObservedPlanState {
            catalog_id: plan.document().catalog.catalog_id.clone(),
            source_id: plan.document().source.source_id.clone(),
            source_fingerprint: plan.document().source.fingerprint.clone(),
            manifests: plan.document().catalog.manifests.clone(),
            occupied_destinations: vec![vec!["2024".into(), "CAFE\u{301}".into()]],
        };

        assert_eq!(
            plan.validate_drift(&observed).unwrap(),
            [DriftFinding::DestinationOccupied {
                path: vec!["2024".into(), "Café".into()],
            }]
        );
    }

    #[test]
    fn drift_reports_an_occupied_new_manifest_destination() {
        let mut document = document();
        document.catalog.manifests.clear();
        let manifest_path = vec!["2024".into(), "collection".into(), "SHA256SUMS".into()];
        let manifest_step = document
            .steps
            .iter_mut()
            .find(|step| matches!(step.action, PlanStepKind::WriteManifest { .. }))
            .unwrap();
        let PlanStepKind::WriteManifest {
            previous_digest, ..
        } = &mut manifest_step.action
        else {
            unreachable!();
        };
        *previous_digest = None;
        let plan = ImportPlan::new(document).unwrap();
        let observed = ObservedPlanState {
            catalog_id: plan.document().catalog.catalog_id.clone(),
            source_id: plan.document().source.source_id.clone(),
            source_fingerprint: plan.document().source.fingerprint.clone(),
            manifests: Vec::new(),
            occupied_destinations: vec![vec![
                "2024".into(),
                "COLLECTION".into(),
                "sha256sums".into(),
            ]],
        };

        assert_eq!(
            plan.validate_drift(&observed).unwrap(),
            [DriftFinding::DestinationOccupied {
                path: manifest_path,
            }]
        );
    }

    #[test]
    fn colliding_destinations_are_rejected() {
        let mut document = document();
        let sidecar_step = document
            .steps
            .iter_mut()
            .find(|step| step.sequence == 2)
            .unwrap();
        let PlanStepKind::CopyFile { destination, .. } = &mut sidecar_step.action else {
            panic!("step 2 should copy the sidecar");
        };
        *destination = vec![
            "2024".into(),
            "COLLECTION".into(),
            "2024-08-08 12-34-56 001.JPG".into(),
        ];

        assert!(matches!(
            ImportPlan::new(document),
            Err(PlanError::DestinationCollision { .. })
        ));
    }

    #[test]
    fn directory_and_file_destinations_cannot_collide() {
        let mut document = document();
        document.steps.push(PlanStep {
            sequence: 4,
            action: PlanStepKind::CreateDirectory {
                path: vec![
                    "2024".into(),
                    "COLLECTION".into(),
                    "2024-08-08 12-34-56 001.JPG".into(),
                ],
            },
        });

        assert!(matches!(
            ImportPlan::new(document),
            Err(PlanError::DestinationCollision { .. })
        ));
    }

    #[test]
    fn manifest_steps_must_match_the_snapshot_baseline() {
        let mut document = document();
        let step = document
            .steps
            .iter_mut()
            .find(|step| step.sequence == 3)
            .unwrap();
        let PlanStepKind::WriteManifest {
            previous_digest, ..
        } = &mut step.action
        else {
            panic!("step 3 should write the manifest");
        };
        *previous_digest = None;

        assert!(matches!(
            ImportPlan::new(document),
            Err(PlanError::ManifestBaselineMismatch(path)) if path.last().unwrap() == "SHA256SUMS"
        ));
    }

    #[test]
    fn copied_originals_require_canonical_manifest_steps() {
        let mut missing = document();
        missing.steps.retain(|step| step.sequence != 3);
        missing.required_space_bytes = 15;
        assert!(matches!(
            ImportPlan::new(missing),
            Err(PlanError::MissingManifestWrite(path)) if path.last().unwrap() == "SHA256SUMS"
        ));

        let mut invalid = document();
        let step = invalid
            .steps
            .iter_mut()
            .find(|step| step.sequence == 3)
            .unwrap();
        let PlanStepKind::WriteManifest {
            path,
            previous_digest,
            ..
        } = &mut step.action
        else {
            panic!("step 3 should write the manifest");
        };
        *path = vec!["2024".into(), "collection".into(), "checksums.txt".into()];
        *previous_digest = None;
        assert!(matches!(
            ImportPlan::new(invalid),
            Err(PlanError::InvalidManifestPath(path)) if path.last().unwrap() == "checksums.txt"
        ));
    }

    #[test]
    fn excluded_files_cannot_be_copied() {
        let mut document = document();
        document.steps.push(PlanStep {
            sequence: 4,
            action: PlanStepKind::CopyFile {
                source_file_id: "file-b".into(),
                destination: vec!["2024".into(), "collection".into(), "excluded.jpg".into()],
                expected_digest: digest("b"),
                size_bytes: 20,
            },
        });
        document.required_space_bytes = 135;

        assert!(matches!(
            ImportPlan::new(document),
            Err(PlanError::SourceFileNotTransferable(id)) if id == "file-b"
        ));
    }

    #[test]
    fn every_transferred_file_requires_one_matching_copy_step() {
        let mut missing = document();
        missing.steps.retain(|step| step.sequence != 2);
        missing.required_space_bytes = 110;
        assert!(matches!(
            ImportPlan::new(missing),
            Err(PlanError::MissingCopyStep(id)) if id == "file-sidecar"
        ));

        let mut mismatched = document();
        let step = mismatched
            .steps
            .iter_mut()
            .find(|step| step.sequence == 1)
            .unwrap();
        let PlanStepKind::CopyFile { size_bytes, .. } = &mut step.action else {
            panic!("step 1 should copy the primary file");
        };
        *size_bytes = 11;
        mismatched.required_space_bytes = 116;
        assert!(matches!(
            ImportPlan::new(mismatched),
            Err(PlanError::CopyStepMismatch(id)) if id == "file-a"
        ));
    }

    #[test]
    fn invalid_observed_paths_are_reported_instead_of_ignored() {
        let plan = ImportPlan::new(document()).unwrap();
        let observed = ObservedPlanState {
            catalog_id: "catalog-1".into(),
            source_id: "source-1".into(),
            source_fingerprint: digest("source snapshot"),
            manifests: plan.document().catalog.manifests.clone(),
            occupied_destinations: vec![vec!["CON".into()]],
        };

        assert!(matches!(
            plan.validate_drift(&observed),
            Err(PlanError::InvalidPortablePath { .. })
        ));
    }

    #[test]
    fn ambiguous_sidecars_require_two_existing_item_references() {
        let mut missing_evidence = document();
        missing_evidence.items[0].disposition = PlanItemDisposition::Unresolved {
            finding: PlanItemFinding::AmbiguousSidecar,
        };
        assert!(matches!(
            ImportPlan::new(missing_evidence),
            Err(PlanError::InvalidAmbiguousSidecar(id)) if id == "item-b"
        ));

        let mut single_candidate = document();
        single_candidate.unhandled_content[0].finding = UnhandledContentFinding::AmbiguousSidecar {
            candidate_item_ids: vec!["item-a".into()],
        };
        assert!(matches!(
            ImportPlan::new(single_candidate),
            Err(PlanError::InvalidAmbiguousSidecar(id)) if id == "unknown"
        ));

        let mut dangling_candidate = document();
        dangling_candidate.unhandled_content[0].finding =
            UnhandledContentFinding::AmbiguousSidecar {
                candidate_item_ids: vec!["item-a".into(), "missing".into()],
            };
        assert!(matches!(
            ImportPlan::new(dangling_candidate),
            Err(PlanError::InvalidAmbiguousSidecar(id)) if id == "unknown"
        ));
    }

    #[test]
    fn plan_identifiers_reject_controls_and_path_separators() {
        assert!(matches!(
            PlanId::new("folder/plan"),
            Err(PlanError::InvalidPlanId)
        ));
        assert!(matches!(
            PlanId::new("plan\nnext"),
            Err(PlanError::InvalidPlanId)
        ));
    }

    #[test]
    fn generated_schema_describes_the_plan_envelope() {
        let schema = serde_json::to_value(import_plan_schema()).unwrap();

        assert_eq!(
            schema["required"],
            serde_json::json!(["digest", "document"])
        );
        assert!(schema["definitions"]["PlanDocument"].is_object());
        assert_eq!(
            schema["properties"]["digest"]["$ref"],
            serde_json::json!("#/definitions/ContentDigest")
        );
        assert_eq!(
            schema["definitions"]["ContentDigest"]["pattern"],
            serde_json::json!("^[0-9a-f]{64}$")
        );
    }
}
