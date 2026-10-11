use std::collections::{BTreeMap, BTreeSet};

use souvenirs_domain::datetime::{
    camera_correction_batches, pending_confirmation_batches, resolve_datetime,
    AcceptedDateTimeItem, CameraCorrectionBatch, CameraCorrectionItem, DateTimeError,
    DateTimeResolution, PendingConfirmationBatch, PendingConfirmationItem,
};
use souvenirs_domain::relationships::{
    LogicalItem, RelationshipFinding, RelationshipResolution, RelationshipStatus,
};
use souvenirs_domain::source::{CameraIdentity, SourceFile, SourceFileId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceDateItem {
    pub logical_item: LogicalItem,
    pub camera: CameraIdentity,
    pub datetime: Option<DateTimeResolution>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceDateResolution {
    pub items: Vec<SourceDateItem>,
    pub findings: Vec<RelationshipFinding>,
}

pub fn assemble_source_datetimes(
    files: &[SourceFile],
    relationships: RelationshipResolution,
) -> SourceDateResolution {
    let by_id = files
        .iter()
        .map(|file| (file.id.clone(), file))
        .collect::<BTreeMap<_, _>>();
    let items = relationships
        .items
        .into_iter()
        .map(|logical_item| SourceDateItem {
            datetime: resolve_item_datetime(&logical_item, &by_id),
            camera: by_id[&logical_item.primary].metadata.camera.clone(),
            logical_item,
        })
        .collect();
    SourceDateResolution {
        items,
        findings: relationships.findings,
    }
}

fn resolve_item_datetime(
    item: &LogicalItem,
    by_id: &BTreeMap<SourceFileId, &SourceFile>,
) -> Option<DateTimeResolution> {
    (item.status == RelationshipStatus::Resolved).then(|| {
        resolve_datetime(
            std::iter::once(&item.primary)
                .chain(item.sidecars.iter())
                .flat_map(|id| by_id[id].metadata.datetime_candidates.iter().cloned()),
        )
    })
}

pub fn source_confirmation_batches(
    items: &[SourceDateItem],
) -> Result<Vec<PendingConfirmationBatch>, DateTimeError> {
    pending_confirmation_batches(items.iter().filter_map(|item| {
        let Some(DateTimeResolution::NeedsConfirmation(pending)) = &item.datetime else {
            return None;
        };
        Some(PendingConfirmationItem {
            item_id: item.logical_item.primary.as_str().to_owned(),
            pending: pending.clone(),
        })
    }))
}

pub fn source_camera_correction_batches(
    items: &[SourceDateItem],
    confirmed: impl IntoIterator<Item = AcceptedDateTimeItem>,
) -> Result<Vec<CameraCorrectionBatch>, DateTimeError> {
    let mut confirmed_by_item = BTreeMap::new();
    for item in confirmed {
        if confirmed_by_item
            .insert(item.item_id.clone(), item.accepted)
            .is_some()
        {
            return Err(DateTimeError::DuplicateItemId(item.item_id));
        }
    }
    let known_item_ids = items
        .iter()
        .map(|item| item.logical_item.primary.as_str())
        .collect::<BTreeSet<_>>();
    if let Some(item_id) = confirmed_by_item
        .keys()
        .find(|item_id| !known_item_ids.contains(item_id.as_str()))
    {
        return Err(DateTimeError::UnknownItemId(item_id.clone()));
    }
    camera_correction_batches(items.iter().filter_map(|item| {
        let item_id = item.logical_item.primary.as_str();
        let accepted = match confirmed_by_item.get(item_id) {
            Some(accepted) => accepted,
            None => {
                let Some(DateTimeResolution::Accepted { accepted, .. }) = &item.datetime else {
                    return None;
                };
                accepted
            }
        };
        Some(CameraCorrectionItem {
            item_id: item_id.to_owned(),
            camera: item.camera.clone(),
            accepted: accepted.clone(),
        })
    }))
}

#[cfg(test)]
mod tests {
    use souvenirs_domain::datetime::{
        AcceptedDateTimeSource, ConfirmationReason, ConfirmationRequirement, DateTimeCandidate,
        DateTimeResolution, DateTimeSource, DateTimeValue,
    };
    use souvenirs_domain::source::{
        CameraIdentity, OriginalRole, SidecarNamingForm, SourceFile, SourceFileId, SourceFileKind,
        SourceMetadata, SourcePath,
    };
    use time::macros::datetime;

    use super::{
        assemble_source_datetimes, source_camera_correction_batches, source_confirmation_batches,
    };

    #[test]
    fn resolves_candidates_only_after_sidecar_ownership() {
        let primary = original(
            "IMG_0001.JPG",
            vec![candidate(
                DateTimeSource::FilenamePattern,
                DateTimeValue::Local(datetime!(2030-01-02 03:04:05)),
            )],
        );
        let owned = sidecar(
            "IMG_0001.JPG.xmp",
            "IMG_0001.JPG",
            candidate(
                DateTimeSource::XmpDateCreated,
                DateTimeValue::Local(datetime!(2001-02-03 04:05:06)),
            ),
        );
        let orphan = sidecar(
            "OTHER.JPG.xmp",
            "OTHER.JPG",
            candidate(
                DateTimeSource::ExifDateTimeOriginal,
                DateTimeValue::Local(datetime!(1999-01-01 00:00:01)),
            ),
        );

        let files = [primary, owned, orphan];
        let relationships =
            souvenirs_domain::relationships::resolve_directory_relationships(&files);
        let result = assemble_source_datetimes(&files, relationships);
        let DateTimeResolution::Accepted { accepted, .. } =
            result.items[0].datetime.as_ref().unwrap()
        else {
            panic!("owned XMP metadata should resolve automatically");
        };
        assert_eq!(accepted.wall_clock, datetime!(2001-02-03 04:05:06));
        assert_eq!(
            accepted.source,
            AcceptedDateTimeSource::Metadata(DateTimeSource::XmpDateCreated)
        );
        assert_eq!(result.items[0].logical_item.sidecars.len(), 1);
        assert_eq!(result.findings.len(), 1);
    }

    #[test]
    fn preserves_pending_and_missing_states_without_a_clock() {
        let filename = original(
            "IMG_20240203_040506.JPG",
            vec![candidate(
                DateTimeSource::FilenamePattern,
                DateTimeValue::Local(datetime!(2024-02-03 04:05:06)),
            )],
        );
        let container = original(
            "clip.mov",
            vec![candidate(
                DateTimeSource::ContainerCreationTime,
                DateTimeValue::AmbiguousContainer(datetime!(2020-06-07 08:09:10)),
            )],
        );
        let missing = original("undated.png", Vec::new());

        let files = [filename, container, missing];
        let relationships =
            souvenirs_domain::relationships::resolve_directory_relationships(&files);
        let result = assemble_source_datetimes(&files, relationships);
        assert!(matches!(
            &result.items[0].datetime,
            Some(DateTimeResolution::NeedsConfirmation(pending))
                if pending.reason == ConfirmationReason::Proposal
        ));
        assert!(matches!(
            &result.items[1].datetime,
            Some(DateTimeResolution::NeedsConfirmation(pending))
                if pending.reason == ConfirmationReason::AmbiguousMetadata
        ));
        assert_eq!(result.items[2].datetime, Some(DateTimeResolution::Missing));
    }

    #[test]
    fn compound_candidates_come_from_the_primary_not_the_companion() {
        let primary = source_file(
            "IMG_0002.JPG",
            "IMG_0002",
            SourceFileKind::Original(OriginalRole::Jpeg),
            vec![candidate(
                DateTimeSource::FilenamePattern,
                DateTimeValue::Local(datetime!(2024-02-03 04:05:06)),
            )],
        );
        let companion = source_file(
            "IMG_0002.CR3",
            "IMG_0002",
            SourceFileKind::Original(OriginalRole::Raw),
            vec![candidate(
                DateTimeSource::ExifDateTimeOriginal,
                DateTimeValue::Local(datetime!(2001-02-03 04:05:06)),
            )],
        );
        let files = [primary, companion];
        let relationships =
            souvenirs_domain::relationships::resolve_directory_relationships(&files);
        let result = assemble_source_datetimes(&files, relationships);

        assert!(matches!(
            &result.items[0].datetime,
            Some(DateTimeResolution::NeedsConfirmation(pending))
                if pending.candidate.source == DateTimeSource::FilenamePattern
        ));
    }

    #[test]
    fn application_batches_pending_items_by_primary_item_id() {
        let first = original(
            "z-first.jpg",
            vec![candidate(
                DateTimeSource::FilenamePattern,
                DateTimeValue::Local(datetime!(2024-02-03 04:05:06)),
            )],
        );
        let second = original(
            "a-second.jpg",
            vec![candidate(
                DateTimeSource::FilenamePattern,
                DateTimeValue::Local(datetime!(2024-02-03 04:05:06)),
            )],
        );
        let files = [first, second];
        let relationships =
            souvenirs_domain::relationships::resolve_directory_relationships(&files);
        let result = assemble_source_datetimes(&files, relationships);
        let mut reversed = result.items.clone();
        reversed.reverse();

        let forward = source_confirmation_batches(&result.items).unwrap();
        let reverse = source_confirmation_batches(&reversed).unwrap();

        assert_eq!(forward, reverse);
        assert_eq!(forward.len(), 1);
        assert_eq!(
            forward[0].key.requirement,
            ConfirmationRequirement::AcceptCandidate
        );
        assert_eq!(
            forward[0]
                .items
                .iter()
                .map(|item| item.item_id.as_str())
                .collect::<Vec<_>>(),
            ["a-second.jpg", "z-first.jpg"]
        );
    }

    #[test]
    fn application_uses_only_the_primary_complete_camera_identity() {
        let mut complete = original(
            "complete.jpg",
            vec![candidate(
                DateTimeSource::ExifDateTimeOriginal,
                DateTimeValue::Local(datetime!(2024-02-03 04:05:06)),
            )],
        );
        complete.metadata.camera = CameraIdentity {
            make: Some("Canon".into()),
            model: Some("R5".into()),
            serial_number: Some("123".into()),
        };
        let mut incomplete = original(
            "incomplete.jpg",
            vec![candidate(
                DateTimeSource::ExifDateTimeOriginal,
                DateTimeValue::Local(datetime!(2024-02-03 04:05:06)),
            )],
        );
        incomplete.metadata.camera = CameraIdentity {
            make: Some("Canon".into()),
            model: Some("R5".into()),
            serial_number: None,
        };
        let files = [incomplete, complete];
        let relationships =
            souvenirs_domain::relationships::resolve_directory_relationships(&files);
        let result = assemble_source_datetimes(&files, relationships);

        let batches = source_camera_correction_batches(&result.items, std::iter::empty()).unwrap();

        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].items[0].item_id, "complete.jpg");
    }

    #[test]
    fn confirmed_items_join_camera_correction_batches() {
        let mut source = original(
            "confirmed.jpg",
            vec![candidate(
                DateTimeSource::FilenamePattern,
                DateTimeValue::Local(datetime!(2024-02-03 04:05:06)),
            )],
        );
        source.metadata.camera = CameraIdentity {
            make: Some("Canon".into()),
            model: Some("R5".into()),
            serial_number: Some("123".into()),
        };
        let files = [source];
        let relationships =
            souvenirs_domain::relationships::resolve_directory_relationships(&files);
        let result = assemble_source_datetimes(&files, relationships);
        let confirmation = souvenirs_domain::datetime::confirm_batch(
            source_confirmation_batches(&result.items)
                .unwrap()
                .remove(0),
            souvenirs_domain::datetime::ConfirmationDecision::AcceptCandidate,
        )
        .unwrap();

        let batches = source_camera_correction_batches(&result.items, confirmation).unwrap();

        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].items[0].item_id, "confirmed.jpg");
        assert_eq!(
            batches[0].items[0].accepted.source,
            AcceptedDateTimeSource::Confirmed(DateTimeSource::FilenamePattern)
        );
    }

    #[test]
    fn camera_batches_reject_confirmations_for_unknown_items() {
        let files = [original(
            "known.jpg",
            vec![candidate(
                DateTimeSource::ExifDateTimeOriginal,
                DateTimeValue::Local(datetime!(2024-02-03 04:05:06)),
            )],
        )];
        let relationships =
            souvenirs_domain::relationships::resolve_directory_relationships(&files);
        let result = assemble_source_datetimes(&files, relationships);
        let accepted = souvenirs_domain::datetime::AcceptedDateTime::assigned(datetime!(
            2024-02-03 04:05:06
        ));

        assert!(matches!(
            source_camera_correction_batches(
                &result.items,
                [souvenirs_domain::datetime::AcceptedDateTimeItem {
                    item_id: "unknown.jpg".into(),
                    accepted,
                }],
            ),
            Err(souvenirs_domain::datetime::DateTimeError::UnknownItemId(item_id))
                if item_id == "unknown.jpg"
        ));
    }

    fn original(name: &str, candidates: Vec<DateTimeCandidate>) -> SourceFile {
        source_file(
            name,
            name.rsplit_once('.').map_or(name, |(basename, _)| basename),
            SourceFileKind::Original(OriginalRole::Still),
            candidates,
        )
    }

    fn sidecar(name: &str, basename: &str, candidate: DateTimeCandidate) -> SourceFile {
        source_file(
            name,
            basename,
            SourceFileKind::Sidecar {
                naming_forms: vec![SidecarNamingForm::AssetFilename],
            },
            vec![candidate],
        )
    }

    fn candidate(source: DateTimeSource, value: DateTimeValue) -> DateTimeCandidate {
        DateTimeCandidate { source, value }
    }

    fn source_file(
        name: &str,
        basename: &str,
        kind: SourceFileKind,
        datetime_candidates: Vec<DateTimeCandidate>,
    ) -> SourceFile {
        SourceFile {
            id: SourceFileId::new(name).unwrap(),
            path: SourcePath::new([name]).unwrap(),
            basename: basename.to_owned(),
            extension: name
                .rsplit_once('.')
                .map_or("", |(_, value)| value)
                .to_owned(),
            kind,
            metadata: SourceMetadata {
                datetime_candidates,
                camera: CameraIdentity {
                    make: None,
                    model: None,
                    serial_number: None,
                },
                apple_content_identifier: None,
            },
        }
    }
}
