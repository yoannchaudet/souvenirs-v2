use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use souvenirs_domain::catalog::{
    allocate_collection_counters, CanonicalFilename, CanonicalStem, NewItemCounterRequest,
};
use souvenirs_domain::names::{CatalogLabel, PortableComponent};
use souvenirs_domain::plan::PlanFileRole;
use souvenirs_domain::relationships::{
    resolve_directory_relationships, sidecar_suffix_for_item, RelationshipStatus,
};
use souvenirs_domain::source::{
    CameraIdentity, SourceFile, SourceFileId, SourceMetadata, SourcePath,
};
use time::{macros::format_description, Date, PrimitiveDateTime, Time};

use crate::execution::{
    digest_bytes, digest_file, encode_manifest, existing_counters, existing_manifest,
};
use crate::{
    CatalogService, DestinationMode, ImportPlanningError, ImportPlanningService, ImportReview,
    ImportReviewRequest,
};

#[derive(Clone, Debug)]
pub(crate) struct PlannedImport {
    pub(crate) review: ImportReview,
    pub(crate) catalog_path: PathBuf,
    pub(crate) source_path: PathBuf,
    pub(crate) collection_name: String,
    pub(crate) directories: Vec<PlannedDirectory>,
    pub(crate) items: Vec<PlannedItem>,
    pub(crate) manifest: PlannedManifest,
}

#[derive(Clone, Debug)]
pub(crate) struct PlannedDirectory {
    pub(crate) path: PathBuf,
    pub(crate) create: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct PlannedItem {
    pub(crate) files: Vec<PlannedFile>,
}

#[derive(Clone, Debug)]
pub(crate) struct PlannedFile {
    pub(crate) source: PathBuf,
    pub(crate) destination: PathBuf,
    pub(crate) digest: Option<String>,
    pub(crate) size_bytes: u64,
    pub(crate) role: PlanFileRole,
}

#[derive(Clone, Debug)]
pub(crate) struct PlannedManifest {
    pub(crate) path: PathBuf,
    pub(crate) previous_digest: Option<String>,
    pub(crate) resulting_digest: String,
    pub(crate) contents: Vec<u8>,
}

pub(crate) fn plan_import(
    request: ImportReviewRequest,
) -> Result<PlannedImport, ImportPlanningError> {
    build_import(request, PlanningPurpose::Execution)
}

pub(crate) fn review_import(
    request: ImportReviewRequest,
) -> Result<ImportReview, ImportPlanningError> {
    build_import(request, PlanningPurpose::Review).map(|plan| plan.review)
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum PlanningPurpose {
    Review,
    Execution,
}

fn build_import(
    request: ImportReviewRequest,
    purpose: PlanningPurpose,
) -> Result<PlannedImport, ImportPlanningError> {
    CatalogService::open(&request.catalog_path).map_err(ImportPlanningError::Catalog)?;
    let scan = ImportPlanningService::scan(&request.source_path)?;
    if scan.media_count == 0 {
        return Err(ImportPlanningError::NoSupportedMedia);
    }

    let collection = CatalogLabel::new(&request.collection_name)
        .map_err(|error| ImportPlanningError::InvalidCollection(error.to_string()))?;
    let accepted_date = Date::parse(
        &request.accepted_date,
        format_description!("[year]-[month]-[day]"),
    )
    .map_err(|_| ImportPlanningError::InvalidDate(request.accepted_date.clone()))?;
    let accepted_datetime = PrimitiveDateTime::new(
        accepted_date,
        Time::from_hms(12, 0, 0).expect("noon is valid"),
    );
    let year = accepted_date.year().to_string();
    let collection_directory = format!("{accepted_date} {collection}");
    let collection_path = request.catalog_path.join(&year).join(&collection_directory);
    match request.destination_mode {
        DestinationMode::Create if collection_path.exists() => {
            return Err(ImportPlanningError::DestinationAlreadyExists(
                collection_path,
            ));
        }
        DestinationMode::Existing if !collection_path.is_dir() => {
            return Err(ImportPlanningError::DestinationMissing(collection_path));
        }
        DestinationMode::Create | DestinationMode::Existing => {}
    }

    let mut paths = Vec::new();
    super::collect_source_files(&request.source_path, &mut paths)?;
    paths.sort();
    let scanned = paths
        .into_iter()
        .filter(|path| {
            !matches!(
                souvenirs_metadata::classify_path(path),
                souvenirs_metadata::AssetKind::Unsupported
            )
        })
        .map(|path| scan_file(&request.source_path, path, purpose))
        .collect::<Result<Vec<_>, _>>()?;
    let source_files = scanned
        .iter()
        .map(|file| file.source_file.clone())
        .collect::<Vec<_>>();
    let by_id = scanned
        .iter()
        .map(|file| (file.source_file.id.clone(), file))
        .collect::<BTreeMap<_, _>>();
    let relationships = resolve_directory_relationships(&source_files);

    let allocations = allocate_collection_counters(
        &BTreeMap::from([(
            accepted_date,
            existing_counters(&collection_path, accepted_date)
                .map_err(|error| ImportPlanningError::InvalidPlan(error.to_string()))?,
        )]),
        relationships
            .items
            .iter()
            .filter(|item| item.status == RelationshipStatus::Resolved)
            .map(|item| NewItemCounterRequest {
                item_id: item.primary.as_str().to_owned(),
                source_path_order_key: by_id[&item.primary].source_file.path.order_key(),
                accepted_datetime,
            }),
    )
    .map_err(|error| ImportPlanningError::InvalidPlan(error.to_string()))?;
    let counters = allocations
        .into_iter()
        .map(|allocation| (allocation.item_id, allocation.counter))
        .collect::<BTreeMap<_, _>>();

    let manifest_path = PathBuf::from(&year)
        .join(&collection_directory)
        .join("SHA256SUMS");
    let mut manifest = existing_manifest(&request.catalog_path.join(&manifest_path))
        .map_err(|error| ImportPlanningError::InvalidPlan(error.to_string()))?;
    let mut items = Vec::new();
    let mut transferred_originals = 0_usize;
    let mut transferred_files = 0_usize;
    let mut required_space_bytes = 0_u64;

    for item in relationships
        .items
        .into_iter()
        .filter(|item| item.status == RelationshipStatus::Resolved)
    {
        let stem = CanonicalStem::new(accepted_datetime, counters[item.primary.as_str()])
            .map_err(|error| ImportPlanningError::InvalidPlan(error.to_string()))?;
        let mut files = Vec::new();
        for (file_id, role) in std::iter::once((&item.primary, PlanFileRole::Primary)).chain(
            item.companion
                .iter()
                .map(|id| (id, PlanFileRole::Companion)),
        ) {
            let source = by_id[file_id];
            let filename = CanonicalFilename::new(&stem, &source.source_file.extension)
                .map_err(|error| ImportPlanningError::InvalidPlan(error.to_string()))?
                .to_string();
            if let Some(digest) = &source.digest {
                manifest.insert(filename.clone(), digest.clone());
            }
            files.push(planned_file(
                source,
                PathBuf::from(&year)
                    .join(&collection_directory)
                    .join(filename),
                role,
            ));
            transferred_originals += 1;
        }
        for sidecar_id in &item.sidecars {
            let sidecar = by_id[sidecar_id];
            let suffix = sidecar_suffix_for_item(
                &sidecar.source_file,
                std::iter::once(&item.primary)
                    .chain(item.companion.iter())
                    .map(|id| &by_id[id].source_file),
            )
            .ok_or_else(|| {
                ImportPlanningError::InvalidPlan(format!(
                    "resolved sidecar has no unambiguous preserved suffix: {}",
                    sidecar.source_file.path
                ))
            })?;
            let filename = PortableComponent::new(format!("{stem}{suffix}"))
                .map_err(|error| ImportPlanningError::InvalidPlan(error.to_string()))?;
            files.push(planned_file(
                sidecar,
                PathBuf::from(&year)
                    .join(&collection_directory)
                    .join(filename.as_str()),
                PlanFileRole::Sidecar,
            ));
        }
        transferred_files += files.len();
        for file in &files {
            required_space_bytes = required_space_bytes
                .checked_add(file.size_bytes)
                .ok_or(ImportPlanningError::SourceTooLarge)?;
        }
        items.push(PlannedItem { files });
    }

    let contents = encode_manifest(&manifest);
    let previous_digest = if purpose == PlanningPurpose::Execution
        && request.catalog_path.join(&manifest_path).is_file()
    {
        Some(
            digest_file(&request.catalog_path.join(&manifest_path))
                .map_err(|error| ImportPlanningError::InvalidPlan(error.to_string()))?
                .0,
        )
    } else {
        None
    };
    let required_decision_count =
        usize::from(scan.uncertain_time_count > 0 && request.date_resolution.is_none());
    let review = ImportReview {
        collection_name: collection.to_string(),
        destination_mode: request.destination_mode,
        destination_path: collection_path,
        accepted_date: request.accepted_date,
        date_resolution: request.date_resolution,
        required_decision_count,
        transfer_count: transferred_originals,
        skipped_count: 0,
        left_on_source_count: scan.file_count.saturating_sub(transferred_files),
        operation_count: transferred_files + 2,
        required_space_bytes,
        can_start: required_decision_count == 0 && transferred_files > 0,
        scan,
    };
    let directories = match request.destination_mode {
        DestinationMode::Create => vec![
            PlannedDirectory {
                path: PathBuf::from(&year),
                create: !request.catalog_path.join(&year).exists(),
            },
            PlannedDirectory {
                path: PathBuf::from(&year).join(&collection_directory),
                create: true,
            },
        ],
        DestinationMode::Existing => Vec::new(),
    };

    Ok(PlannedImport {
        review,
        catalog_path: request.catalog_path,
        source_path: request.source_path,
        collection_name: request.collection_name,
        directories,
        items,
        manifest: PlannedManifest {
            path: manifest_path,
            previous_digest,
            resulting_digest: digest_bytes(&contents),
            contents,
        },
    })
}

struct ScannedPlanningFile {
    source_file: SourceFile,
    absolute_path: PathBuf,
    digest: Option<String>,
    size_bytes: u64,
}

fn scan_file(
    root: &Path,
    path: PathBuf,
    purpose: PlanningPurpose,
) -> Result<ScannedPlanningFile, ImportPlanningError> {
    let relative = path
        .strip_prefix(root)
        .expect("collected source remains below root");
    let components = relative
        .components()
        .map(|component| {
            component
                .as_os_str()
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| {
                    ImportPlanningError::InvalidPlan(format!(
                        "source path is not valid UTF-8: {}",
                        path.display()
                    ))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let source_path = SourcePath::new(components)
        .map_err(|error| ImportPlanningError::InvalidPlan(error.to_string()))?;
    let filename = source_path.filename();
    let filename_path = Path::new(filename);
    let basename = filename_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(filename)
        .to_owned();
    let extension = filename_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_owned();
    let relationship_metadata = souvenirs_metadata::relationship_metadata(&path)
        .map_err(|error| ImportPlanningError::InvalidPlan(error.to_string()))?;
    let file_metadata = std::fs::metadata(&path).map_err(|source| ImportPlanningError::Io {
        path: path.clone(),
        source,
    })?;
    let digest = if purpose == PlanningPurpose::Execution {
        Some(
            digest_file(&path)
                .map_err(|error| ImportPlanningError::InvalidPlan(error.to_string()))?
                .0,
        )
    } else {
        None
    };
    Ok(ScannedPlanningFile {
        source_file: SourceFile {
            id: SourceFileId::new(source_path.to_string())
                .map_err(|error| ImportPlanningError::InvalidPlan(error.to_string()))?,
            path: source_path,
            basename,
            extension: extension.clone(),
            kind: souvenirs_metadata::classify_extension(&extension).source_file_kind(),
            metadata: SourceMetadata {
                datetime_candidates: Vec::new(),
                camera: CameraIdentity {
                    make: None,
                    model: None,
                    serial_number: None,
                },
                apple_content_identifier: relationship_metadata.apple_content_identifier,
            },
        },
        absolute_path: path,
        digest,
        size_bytes: file_metadata.len(),
    })
}

fn planned_file(
    source: &ScannedPlanningFile,
    destination: PathBuf,
    role: PlanFileRole,
) -> PlannedFile {
    PlannedFile {
        source: source.absolute_path.clone(),
        destination,
        digest: source.digest.clone(),
        size_bytes: source.size_bytes,
        role,
    }
}
