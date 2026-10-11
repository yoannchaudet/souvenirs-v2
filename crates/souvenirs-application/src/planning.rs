use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use souvenirs_domain::catalog::{
    allocate_collection_counters, ongoing_collection_directory, year_directory, CanonicalFilename,
    CanonicalStem, CollectionIdentity, NewItemCounterRequest,
};
use souvenirs_domain::names::{CatalogLabel, PortableComponent};
use souvenirs_domain::plan::PlanFileRole;
use souvenirs_domain::relationships::sidecar_suffix_for_item;
use souvenirs_domain::source::SourceFile;
use souvenirs_storage::{validate_directory, CatalogLayout, StorageCapabilities};

use crate::execution::{
    digest_bytes, digest_file, encode_manifest, encode_reviewed_plan, existing_counters,
    existing_manifest, validate_reviewed_plan,
};
use crate::{
    dated_collection_components, directory_identity, inspect_source, scan_summary, CatalogService,
    ExistingCollectionIdentity, ImportDestination, ImportPlanningError, ImportPreflightCheck,
    ImportPreflightEvidence, ImportReview, ImportReviewRequest, SourceSnapshotEntry,
};

#[derive(Clone, Debug)]
pub(crate) struct PlannedImport {
    pub(crate) review: ImportReview,
    pub(crate) catalog_path: PathBuf,
    pub(crate) source_path: PathBuf,
    pub(crate) collection_name: String,
    pub(crate) source_snapshot: Vec<SourceSnapshotEntry>,
    pub(crate) directories: Vec<PlannedDirectory>,
    pub(crate) items: Vec<PlannedItem>,
    pub(crate) manifest: PlannedManifest,
}

#[derive(Clone, Debug)]
pub(crate) struct PlannedDirectory {
    pub(crate) path: PathBuf,
    pub(crate) create: bool,
    pub(crate) identity: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct PlannedItem {
    pub(crate) accepted_datetime: String,
    pub(crate) date_source: crate::ImportDateSource,
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

enum ResolvedDestination {
    Create {
        label: CatalogLabel,
    },
    Dated {
        label: CatalogLabel,
        year: String,
        directory: String,
        start_date: time::Date,
        end_date: time::Date,
    },
    Ongoing {
        label: CatalogLabel,
        parts: Vec<crate::CatalogCollectionPart>,
    },
}

struct ResolvedTarget {
    year: String,
    collection_directory: String,
    create_collection: bool,
    existing_selection: bool,
}

impl ResolvedDestination {
    fn collection_name(&self) -> &str {
        match self {
            Self::Create { label } | Self::Dated { label, .. } | Self::Ongoing { label, .. } => {
                label.as_str()
            }
        }
    }

    fn target(
        &self,
        range_start: time::Date,
        range_end: time::Date,
        dates: &BTreeSet<time::Date>,
    ) -> Result<ResolvedTarget, ImportPlanningError> {
        match self {
            Self::Create { label } => {
                let (year, collection_directory) =
                    dated_collection_components(range_start, range_end, label)?;
                Ok(ResolvedTarget {
                    year,
                    collection_directory,
                    create_collection: true,
                    existing_selection: false,
                })
            }
            Self::Dated {
                year,
                directory,
                start_date,
                end_date,
                ..
            } => {
                if dates
                    .iter()
                    .any(|date| date < start_date || date > end_date)
                {
                    return Err(ImportPlanningError::DestinationUnavailable(format!(
                        "The selected collection covers {start_date} through {end_date}. Choose a collection that includes every import date."
                    )));
                }
                Ok(ResolvedTarget {
                    year: year.clone(),
                    collection_directory: directory.clone(),
                    create_collection: false,
                    existing_selection: true,
                })
            }
            Self::Ongoing { label, parts } => {
                let years = dates
                    .iter()
                    .map(|date| date.year())
                    .collect::<BTreeSet<_>>();
                let Some(year) = years.first().copied() else {
                    return Err(ImportPlanningError::DestinationUnavailable(
                        "The selected ongoing collection has no resolved import year.".to_owned(),
                    ));
                };
                if years.len() != 1 {
                    return Err(ImportPlanningError::DestinationUnavailable(
                        "This import spans more than one year. Import one year at a time into an ongoing collection."
                            .to_owned(),
                    ));
                }
                if let Some(part) = parts.iter().find(|part| part.year == year) {
                    return Ok(ResolvedTarget {
                        year: part.year_directory.clone(),
                        collection_directory: part.collection_directory.clone(),
                        create_collection: false,
                        existing_selection: true,
                    });
                }
                let date = *dates.first().expect("an ongoing target has a date");
                Ok(ResolvedTarget {
                    year: year_directory(date)
                        .map_err(|error| ImportPlanningError::InvalidCollection(error.to_string()))?
                        .to_string(),
                    collection_directory: ongoing_collection_directory(label)
                        .map_err(|error| ImportPlanningError::InvalidCollection(error.to_string()))?
                        .to_string(),
                    create_collection: true,
                    existing_selection: true,
                })
            }
        }
    }
}

fn resolve_destination(
    catalog_path: &Path,
    destination: &ImportDestination,
) -> Result<ResolvedDestination, ImportPlanningError> {
    match destination {
        ImportDestination::Create { collection_name } => Ok(ResolvedDestination::Create {
            label: CatalogLabel::new(collection_name)
                .map_err(|error| ImportPlanningError::InvalidCollection(error.to_string()))?,
        }),
        ImportDestination::Existing { identity } => {
            let collections =
                CatalogService::collections(catalog_path).map_err(ImportPlanningError::Catalog)?;
            collections
                .into_iter()
                .find_map(|collection| match (identity, collection) {
                    (
                        ExistingCollectionIdentity::Dated {
                            year: requested_year,
                            directory: requested_directory,
                        },
                        crate::CatalogCollection::Dated {
                            identity: CollectionIdentity::Dated { year, directory },
                            label,
                            start_date,
                            end_date,
                        },
                    ) if year.as_str() == requested_year
                        && directory.as_str() == requested_directory =>
                    {
                        Some(
                            CatalogLabel::new(&label)
                                .map(|label| ResolvedDestination::Dated {
                                    label,
                                    year: year.to_string(),
                                    directory: directory.to_string(),
                                    start_date,
                                    end_date,
                                })
                                .map_err(|error| {
                                    ImportPlanningError::InvalidCollection(error.to_string())
                                }),
                        )
                    }
                    (
                        ExistingCollectionIdentity::Ongoing { key: requested_key },
                        crate::CatalogCollection::Ongoing {
                            identity: CollectionIdentity::Ongoing { key },
                            label,
                            parts,
                        },
                    ) if key.as_str() == requested_key => Some(
                        CatalogLabel::new(&label)
                            .map(|label| ResolvedDestination::Ongoing { label, parts })
                            .map_err(|error| {
                                ImportPlanningError::InvalidCollection(error.to_string())
                            }),
                    ),
                    _ => None,
                })
                .transpose()?
                .ok_or_else(|| {
                    ImportPlanningError::DestinationUnavailable(
                        "The selected collection is no longer available. Choose it again."
                            .to_owned(),
                    )
                })
        }
    }
}

pub(crate) fn review_import(
    request: ImportReviewRequest,
) -> Result<ImportReview, ImportPlanningError> {
    build_import(request).map(|plan| plan.review)
}

fn build_import(request: ImportReviewRequest) -> Result<PlannedImport, ImportPlanningError> {
    CatalogService::open(&request.catalog_path).map_err(ImportPlanningError::Catalog)?;
    let utc_offset =
        time::UtcOffset::from_whole_seconds(request.utc_offset_seconds).map_err(|error| {
            ImportPlanningError::InvalidMetadata(format!("the UTC offset is invalid: {error}"))
        })?;
    let inspected = inspect_source(
        &request.source_path,
        request.date_resolution,
        utc_offset,
        true,
    )?;
    let scan = scan_summary(&request.source_path, &inspected)?;
    if inspected.photo_count + inspected.video_count == 0 {
        return Err(ImportPlanningError::NoSupportedMedia);
    }

    let resolved_destination = resolve_destination(&request.catalog_path, &request.destination)?;
    let collection_name = resolved_destination.collection_name().to_owned();
    let required_decision_count = inspected.pending_item_count;
    let Some(range_start) = inspected
        .items
        .iter()
        .map(|item| item.accepted.wall_clock.date())
        .min()
    else {
        return Ok(PlannedImport {
            review: ImportReview {
                collection_name: collection_name.clone(),
                destination: request.destination.clone(),
                destination_path: request.catalog_path.clone(),
                accepted_date: String::new(),
                date_resolution: request.date_resolution,
                required_decision_count,
                transfer_count: 0,
                skipped_count: inspected.pending_item_count,
                left_on_source_count: scan.file_count,
                operation_count: 0,
                preflight: ImportPreflightEvidence {
                    representative_file_names: Vec::new(),
                    required_bytes: 0,
                    available_bytes: None,
                    available_space: ImportPreflightCheck::NotRun,
                    write_probe: ImportPreflightCheck::NotRun,
                    portable_names: ImportPreflightCheck::NotRun,
                    destination_conflicts: ImportPreflightCheck::NotRun,
                },
                can_start: false,
                reviewed_plan: None,
                scan,
            },
            catalog_path: request.catalog_path,
            source_path: request.source_path,
            collection_name,
            source_snapshot: inspected.source_snapshot,
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
    let dates = inspected
        .items
        .iter()
        .map(|item| item.accepted.wall_clock.date())
        .collect::<BTreeSet<_>>();
    let target = resolved_destination.target(range_start, range_end, &dates)?;
    let year = target.year;
    let collection_directory = target.collection_directory;
    let collection_path = request.catalog_path.join(&year).join(&collection_directory);
    if target.create_collection {
        if collection_path.exists() {
            return Err(if target.existing_selection {
                ImportPlanningError::DestinationUnavailable(
                    "The selected collection's annual destination is unavailable. Choose the collection again."
                        .to_owned(),
                )
            } else {
                ImportPlanningError::DestinationAlreadyExists(collection_path)
            });
        }
    } else {
        if !collection_path.is_dir() {
            return Err(ImportPlanningError::DestinationUnavailable(
                "The selected collection is no longer available. Choose it again.".to_owned(),
            ));
        }
        let layout = CatalogLayout::new(&request.catalog_path);
        validate_directory(&layout, &PathBuf::from(&year).join(&collection_directory)).map_err(
            |error| {
                ImportPlanningError::InvalidPlan(format!(
                    "The selected collection cannot be used: {error}"
                ))
            },
        )?;
    }

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
            )?);
        }
        items.push(PlannedItem {
            accepted_datetime: item.accepted.wall_clock.to_string(),
            date_source: crate::accepted_source(&item.accepted.source),
            files,
        });
    }

    let contents = encode_manifest(&manifest);
    let previous_digest = if request.catalog_path.join(&manifest_path).is_file() {
        Some(
            digest_file(&request.catalog_path.join(&manifest_path))
                .map_err(|error| ImportPlanningError::InvalidPlan(error.to_string()))?
                .0,
        )
    } else {
        None
    };
    let review = ImportReview {
        collection_name: collection_name.clone(),
        destination: request.destination.clone(),
        destination_path: collection_path.clone(),
        accepted_date: range_start.to_string(),
        date_resolution: request.date_resolution,
        required_decision_count,
        transfer_count: inspected.transfer_media_count,
        skipped_count: inspected.pending_item_count,
        left_on_source_count: inspected.excluded_file_count,
        operation_count: inspected.transfer_file_count + 2,
        preflight: ImportPreflightEvidence {
            representative_file_names: Vec::new(),
            required_bytes: inspected.transfer_bytes,
            available_bytes: None,
            available_space: ImportPreflightCheck::NotRun,
            write_probe: ImportPreflightCheck::NotRun,
            portable_names: ImportPreflightCheck::NotRun,
            destination_conflicts: ImportPreflightCheck::NotRun,
        },
        can_start: inspected.transfer_file_count > 0,
        reviewed_plan: None,
        scan,
    };
    let directories = if target.create_collection {
        vec![
            PlannedDirectory {
                path: PathBuf::from(&year),
                create: !request.catalog_path.join(&year).exists(),
                identity: None,
            },
            PlannedDirectory {
                path: PathBuf::from(&year).join(&collection_directory),
                create: true,
                identity: None,
            },
        ]
    } else {
        vec![PlannedDirectory {
            path: PathBuf::from(&year).join(&collection_directory),
            create: false,
            identity: Some(directory_identity(&collection_path.metadata().map_err(
                |error| ImportPlanningError::InvalidPlan(error.to_string()),
            )?)),
        }]
    };

    let mut planned = PlannedImport {
        review,
        catalog_path: request.catalog_path,
        source_path: request.source_path,
        collection_name,
        source_snapshot: inspected.source_snapshot,
        directories,
        items,
        manifest: PlannedManifest {
            path: manifest_path,
            previous_digest,
            resulting_digest: digest_bytes(&contents),
            contents,
        },
    };
    if planned.review.can_start {
        let reviewed_plan = encode_reviewed_plan(&planned)
            .map_err(|error| ImportPlanningError::InvalidPlan(error.to_string()))?;
        validate_reviewed_plan(&reviewed_plan)
            .map_err(|error| ImportPlanningError::DestinationPreflight(error.to_string()))?;

        let capabilities = StorageCapabilities::detect(&planned.catalog_path)
            .map_err(|error| ImportPlanningError::DestinationPreflight(error.to_string()))?;
        capabilities
            .ensure_writable(&planned.catalog_path)
            .map_err(|error| ImportPlanningError::DestinationPreflight(error.to_string()))?;
        let available_bytes = capabilities
            .available_space_bytes(&planned.catalog_path)
            .map_err(|error| ImportPlanningError::DestinationPreflight(error.to_string()))?;
        let available_space =
            available_space_check(planned.review.preflight.required_bytes, available_bytes);

        planned.review.preflight = ImportPreflightEvidence {
            representative_file_names: planned
                .items
                .iter()
                .flat_map(|item| &item.files)
                .filter_map(|file| {
                    file.destination
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_owned)
                })
                .take(3)
                .collect(),
            required_bytes: planned.review.preflight.required_bytes,
            available_bytes: Some(available_bytes),
            available_space,
            write_probe: ImportPreflightCheck::Passed,
            portable_names: ImportPreflightCheck::Passed,
            destination_conflicts: ImportPreflightCheck::Passed,
        };
        if available_space == ImportPreflightCheck::Passed {
            planned.review.reviewed_plan = Some(reviewed_plan);
        } else {
            planned.review.can_start = false;
        }
    }
    Ok(planned)
}

fn available_space_check(required_bytes: u64, available_bytes: u64) -> ImportPreflightCheck {
    if available_bytes >= required_bytes {
        ImportPreflightCheck::Passed
    } else {
        ImportPreflightCheck::Failed
    }
}

fn planned_file(
    source: &SourceFile,
    root: &Path,
    destination: PathBuf,
    role: PlanFileRole,
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
    let digest = Some(
        digest_file(&absolute_path)
            .map_err(|error| ImportPlanningError::InvalidPlan(error.to_string()))?
            .0,
    );
    Ok(PlannedFile {
        source: absolute_path,
        destination,
        digest,
        size_bytes,
        role,
    })
}

#[cfg(test)]
mod tests {
    use super::available_space_check;
    use crate::ImportPreflightCheck;

    #[test]
    fn available_space_check_reports_passed_and_failed_evidence() {
        assert_eq!(
            available_space_check(100, 100),
            ImportPreflightCheck::Passed
        );
        assert_eq!(
            available_space_check(101, 100),
            ImportPreflightCheck::Failed
        );
    }
}
