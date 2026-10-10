use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use souvenirs_domain::catalog::{
    allocate_collection_counters, CanonicalFilename, CanonicalStem, NewItemCounterRequest,
};
use souvenirs_domain::names::{CatalogLabel, PortableComponent};
use souvenirs_domain::plan::PlanFileRole;
use souvenirs_domain::relationships::sidecar_suffix_for_item;
use souvenirs_domain::source::SourceFile;

use crate::execution::{
    digest_bytes, digest_file, encode_manifest, existing_counters, existing_manifest,
};
use crate::{
    dated_collection_components, inspect_source, scan_summary, CatalogService, DateResolution,
    DestinationMode, ImportPlanningError, ImportReview, ImportReviewRequest,
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
    if request.date_resolution == Some(DateResolution::ConvertFromUtc) {
        return Err(ImportPlanningError::UnsupportedDateResolution);
    }
    CatalogService::open(&request.catalog_path).map_err(ImportPlanningError::Catalog)?;
    let inspected = inspect_source(&request.source_path, request.date_resolution)?;
    let scan = scan_summary(&request.source_path, &inspected)?;
    if inspected.photo_count + inspected.video_count == 0 {
        return Err(ImportPlanningError::NoSupportedMedia);
    }

    let collection = CatalogLabel::new(&request.collection_name)
        .map_err(|error| ImportPlanningError::InvalidCollection(error.to_string()))?;
    let required_decision_count = match request.date_resolution {
        Some(DateResolution::AsShown) => 0,
        Some(DateResolution::ConvertFromUtc) | None => {
            inspected.pending_item_count + inspected.unresolved_item_count
        }
    };
    let Some(range_start) = inspected
        .items
        .iter()
        .map(|item| item.accepted.wall_clock.date())
        .min()
    else {
        return Ok(PlannedImport {
            review: ImportReview {
                collection_name: collection.to_string(),
                destination_mode: request.destination_mode,
                destination_path: request.catalog_path.clone(),
                accepted_date: String::new(),
                date_resolution: request.date_resolution,
                required_decision_count,
                transfer_count: 0,
                skipped_count: 0,
                left_on_source_count: scan.file_count,
                operation_count: 0,
                required_space_bytes: 0,
                can_start: false,
                scan,
            },
            catalog_path: request.catalog_path,
            source_path: request.source_path,
            collection_name: request.collection_name,
            directories: Vec::new(),
            items: Vec::new(),
            manifest: PlannedManifest {
                path: PathBuf::new(),
                previous_digest: None,
                resulting_digest: digest_bytes(&[]),
                contents: Vec::new(),
            },
        });
    };
    let range_end = inspected
        .items
        .iter()
        .map(|item| item.accepted.wall_clock.date())
        .max()
        .expect("a range start has a range end");
    let (year, collection_directory) =
        dated_collection_components(range_start, range_end, &collection)?;
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

    let dates = inspected
        .items
        .iter()
        .map(|item| item.accepted.wall_clock.date())
        .collect::<BTreeSet<_>>();
    let existing = dates
        .iter()
        .map(|date| {
            existing_counters(&collection_path, *date)
                .map(|counters| (*date, counters))
                .map_err(|error| ImportPlanningError::InvalidPlan(error.to_string()))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let allocations = allocate_collection_counters(
        &existing,
        inspected.items.iter().map(|item| NewItemCounterRequest {
            item_id: item.primary.id.as_str().to_owned(),
            source_path_order_key: item.primary.path.order_key(),
            accepted_datetime: item.accepted.wall_clock,
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

    for item in &inspected.items {
        let stem = CanonicalStem::new(item.accepted.wall_clock, counters[item.primary.id.as_str()])
            .map_err(|error| ImportPlanningError::InvalidPlan(error.to_string()))?;
        let mut files = Vec::new();
        for (source, role) in std::iter::once((&item.primary, PlanFileRole::Primary)).chain(
            item.companion
                .iter()
                .map(|source| (source, PlanFileRole::Companion)),
        ) {
            let filename = CanonicalFilename::new(&stem, &source.extension)
                .map_err(|error| ImportPlanningError::InvalidPlan(error.to_string()))?
                .to_string();
            let planned = planned_file(
                source,
                &request.source_path,
                PathBuf::from(&year)
                    .join(&collection_directory)
                    .join(&filename),
                role,
                purpose,
            )?;
            if let Some(digest) = &planned.digest {
                manifest.insert(filename, digest.clone());
            }
            files.push(planned);
        }
        for sidecar in &item.sidecars {
            let suffix = sidecar_suffix_for_item(
                sidecar,
                std::iter::once(&item.primary).chain(item.companion.iter()),
            )
            .ok_or_else(|| {
                ImportPlanningError::InvalidPlan(format!(
                    "resolved sidecar has no unambiguous preserved suffix: {}",
                    sidecar.path
                ))
            })?;
            let filename = PortableComponent::new(format!("{stem}{suffix}"))
                .map_err(|error| ImportPlanningError::InvalidPlan(error.to_string()))?;
            files.push(planned_file(
                sidecar,
                &request.source_path,
                PathBuf::from(&year)
                    .join(&collection_directory)
                    .join(filename.as_str()),
                PlanFileRole::Sidecar,
                purpose,
            )?);
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
    let review = ImportReview {
        collection_name: collection.to_string(),
        destination_mode: request.destination_mode,
        destination_path: collection_path,
        accepted_date: range_start.to_string(),
        date_resolution: request.date_resolution,
        required_decision_count,
        transfer_count: inspected.transfer_media_count,
        skipped_count: 0,
        left_on_source_count: inspected.excluded_file_count,
        operation_count: inspected.transfer_file_count + 2,
        required_space_bytes: inspected.transfer_bytes,
        can_start: required_decision_count == 0 && inspected.transfer_file_count > 0,
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

fn planned_file(
    source: &SourceFile,
    root: &Path,
    destination: PathBuf,
    role: PlanFileRole,
    purpose: PlanningPurpose,
) -> Result<PlannedFile, ImportPlanningError> {
    let absolute_path = source
        .path
        .components()
        .iter()
        .fold(root.to_path_buf(), |path, component| path.join(component));
    let size_bytes = std::fs::metadata(&absolute_path)
        .map_err(|source| ImportPlanningError::Io {
            path: absolute_path.clone(),
            source,
        })?
        .len();
    let digest = if purpose == PlanningPurpose::Execution {
        Some(
            digest_file(&absolute_path)
                .map_err(|error| ImportPlanningError::InvalidPlan(error.to_string()))?
                .0,
        )
    } else {
        None
    };
    Ok(PlannedFile {
        source: absolute_path,
        destination,
        digest,
        size_bytes,
        role,
    })
}
