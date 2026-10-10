use std::collections::BTreeMap;

use souvenirs_domain::datetime::{resolve_datetime, DateTimeResolution};
use souvenirs_domain::relationships::{
    LogicalItem, RelationshipFinding, RelationshipResolution, RelationshipStatus,
};
use souvenirs_domain::source::{SourceFile, SourceFileId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceDateItem {
    pub(crate) logical_item: LogicalItem,
    pub(crate) datetime: Option<DateTimeResolution>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceDateResolution {
    pub(crate) items: Vec<SourceDateItem>,
    pub(crate) findings: Vec<RelationshipFinding>,
}

pub(crate) fn assemble_source_datetimes(
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

#[cfg(test)]
mod tests {
    use souvenirs_domain::datetime::{
        AcceptedDateTimeSource, ConfirmationReason, DateTimeCandidate, DateTimeResolution,
        DateTimeSource, DateTimeValue,
    };
    use souvenirs_domain::source::{
        CameraIdentity, OriginalRole, SidecarNamingForm, SourceFile, SourceFileId, SourceFileKind,
        SourceMetadata, SourcePath,
    };
    use time::macros::datetime;

    use super::assemble_source_datetimes;

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
