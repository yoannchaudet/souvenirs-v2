use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Deserialize;
use souvenirs_domain::catalog::{
    allocate_collection_counters, CanonicalFilename, CanonicalStem, NewItemCounterRequest,
};
use souvenirs_domain::datetime::{
    resolve_datetime, AcceptedDateTimeSource, ConfirmationReason, DateTimeCandidate,
    DateTimeResolution, DateTimeSource, DateTimeValue,
};
use souvenirs_domain::relationships::{
    resolve_directory_relationships, CompoundKind, RelationshipFinding, RelationshipStatus,
};
use souvenirs_domain::source::{
    CameraIdentity, SourceFile, SourceFileId, SourceMetadata, SourcePath,
};
use time::{PrimitiveDateTime, UtcOffset};

#[derive(Clone, Debug, Deserialize)]
struct Corpus {
    files: Vec<FixtureFile>,
}

#[derive(Clone, Debug, Deserialize)]
struct FixtureFile {
    id: String,
    path: Vec<String>,
    device: [Option<String>; 3],
    content_identifier: Option<String>,
    candidates: Vec<[String; 2]>,
    disposition: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CorpusOutcome {
    relationships: Vec<String>,
    findings: Vec<String>,
    accepted_sources: Vec<String>,
    disagreements: Vec<String>,
    canonical_files: Vec<String>,
    confirmations: Vec<String>,
    excluded: Vec<String>,
}

#[test]
fn representative_corpus_has_deterministic_relationships_and_plan_results() {
    let corpus = corpus();
    let forward = evaluate(&corpus.files);
    let mut reversed = corpus.files.clone();
    reversed.reverse();

    assert_eq!(forward, evaluate(&reversed));
    assert_eq!(
        forward.relationships,
        [
            "android-mp4 [] single",
            "canon-jpeg+canon-raw [canon-xmp] raw+jpeg",
            "iphone-still+iphone-video [iphone-aae] live-photo",
            "legacy-jpeg [legacy-xmp] single",
            "nikon-raw [nikon-dop] single",
            "pixel-dng [] single",
            "sony-raw [sony-pp3] single",
            "standalone-mov [] single",
        ]
    );
    assert_eq!(
        forward.findings,
        ["orphan-xmp: orphan-sidecar", "unsupported: unknown"]
    );
    assert_eq!(
        forward.accepted_sources,
        [
            "canon-jpeg: exif-original 2024-07-09 10:11:12.0",
            "iphone-still: exif-original 2024-08-03 15:30:45.0",
            "legacy-jpeg: xmp-exif-original 2024-07-09 10:11:12.0",
            "nikon-raw: exif-digitized 2024-09-10 8:00:00.0",
            "standalone-mov: quicktime-creation-date 2024-08-03 15:30:45.0",
        ]
    );
    assert_eq!(
        forward.disagreements,
        [
            "canon-jpeg: exif-digitized 2024-07-09 10:11:13.0",
            "canon-jpeg: xmp-date-created 2024-07-09 10:11:14.0",
            "legacy-jpeg: google-takeout-photo-taken-time 2024-07-09 11:12:13.0",
        ]
    );
    assert_eq!(
        forward.canonical_files,
        [
            "2024-07-09 10-11-12 001.CR3",
            "2024-07-09 10-11-12 001.JPG",
            "2024-07-09 10-11-12 001.xmp",
            "2024-07-09 10-11-12 002.jpeg",
            "2024-07-09 10-11-12 002.jpeg.xmp",
            "2024-08-03 15-30-45 001.AAE",
            "2024-08-03 15-30-45 001.HEIC",
            "2024-08-03 15-30-45 001.MOV",
            "2024-08-03 15-30-45 002.mov",
            "2024-09-10 08-00-00 001.NEF",
            "2024-09-10 08-00-00 001.NEF.dop",
        ]
    );
    assert_eq!(
        forward.confirmations,
        ["android-mp4: ambiguous-metadata", "pixel-dng: proposal"]
    );
    assert_eq!(forward.excluded, ["sony-raw"]);
}

#[test]
fn corpus_covers_supported_formats_devices_and_date_sources() {
    let corpus = corpus();
    let extensions: BTreeSet<_> = corpus
        .files
        .iter()
        .filter_map(|file| Path::new(file.path.last().unwrap()).extension())
        .filter_map(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .collect();
    let expected_extensions: BTreeSet<_> = souvenirs_metadata::SUPPORTED_ORIGINAL_EXTENSIONS
        .iter()
        .chain(souvenirs_metadata::SUPPORTED_SIDECAR_EXTENSIONS)
        .map(|extension| (*extension).to_owned())
        .collect();
    assert!(extensions.is_superset(&expected_extensions));

    let devices: BTreeSet<_> = corpus
        .files
        .iter()
        .filter_map(|file| file.device[0].as_deref())
        .collect();
    assert_eq!(
        devices,
        ["Apple", "Canon", "Google", "Nikon", "Sony"].into()
    );

    let sources: BTreeSet<_> = corpus
        .files
        .iter()
        .flat_map(|file| {
            file.candidates
                .iter()
                .map(|candidate| candidate[0].as_str())
        })
        .collect();
    assert_eq!(
        sources,
        [
            "container_creation_time",
            "exif_datetime_digitized",
            "exif_datetime_original",
            "filename_pattern",
            "google_takeout_photo_taken_time",
            "quick_time_creation_date",
            "xmp_date_created",
            "xmp_exif_datetime_original",
        ]
        .into()
    );
}

fn corpus() -> Corpus {
    serde_json::from_str(include_str!("fixtures/corpus.json")).expect("valid metadata corpus")
}

fn evaluate(fixtures: &[FixtureFile]) -> CorpusOutcome {
    let files: Vec<_> = fixtures.iter().map(source_file).collect();
    let by_id: BTreeMap<_, _> = files.iter().map(|file| (file.id.clone(), file)).collect();
    let fixture_by_id: BTreeMap<_, _> = fixtures
        .iter()
        .map(|file| (file.id.as_str(), file))
        .collect();
    let resolution = resolve_directory_relationships(&files);

    let mut relationships = Vec::new();
    let mut accepted_sources = Vec::new();
    let mut disagreements = Vec::new();
    let mut accepted = Vec::new();
    let mut confirmations = Vec::new();
    let mut excluded = Vec::new();
    for item in &resolution.items {
        assert_eq!(item.status, RelationshipStatus::Resolved);
        let primary = by_id[&item.primary];
        let fixture = fixture_by_id[primary.id.as_str()];
        let companion = item
            .companion
            .as_ref()
            .map_or_else(String::new, |id| format!("+{}", id.as_str()));
        let sidecars = item
            .sidecars
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let kind = match item.compound_kind {
            Some(CompoundKind::RawJpeg) => "raw+jpeg",
            Some(CompoundKind::LivePhoto) => "live-photo",
            Some(CompoundKind::ExistingCanonical) => "canonical",
            None => "single",
        };
        relationships.push(format!(
            "{}{} [{}] {kind}",
            item.primary.as_str(),
            companion,
            sidecars
        ));

        let datetime_candidates = std::iter::once(&item.primary)
            .chain(item.sidecars.iter())
            .flat_map(|id| by_id[id].metadata.datetime_candidates.iter().cloned());
        match (
            fixture.disposition.as_str(),
            resolve_datetime(datetime_candidates),
        ) {
            (
                "transfer",
                DateTimeResolution::Accepted {
                    accepted: value,
                    disagreements: item_disagreements,
                },
            ) => {
                let source = match &value.source {
                    AcceptedDateTimeSource::Metadata(source) => source_label(*source),
                    source => panic!("unexpected accepted source: {source:?}"),
                };
                accepted_sources.push(format!(
                    "{}: {source} {}",
                    item.primary.as_str(),
                    value.wall_clock
                ));
                disagreements.extend(item_disagreements.iter().map(|candidate| {
                    format!(
                        "{}: {} {}",
                        item.primary.as_str(),
                        source_label(candidate.source),
                        candidate_value_label(candidate)
                    )
                }));
                accepted.push((item, value.wall_clock));
            }
            ("missing", DateTimeResolution::Missing) => {
                excluded.push(item.primary.as_str().to_owned());
            }
            ("confirm_proposal", DateTimeResolution::NeedsConfirmation(pending))
                if pending.reason == ConfirmationReason::Proposal =>
            {
                confirmations.push(format!("{}: proposal", item.primary.as_str()));
            }
            ("confirm_ambiguous", DateTimeResolution::NeedsConfirmation(pending))
                if pending.reason == ConfirmationReason::AmbiguousMetadata =>
            {
                confirmations.push(format!("{}: ambiguous-metadata", item.primary.as_str()));
            }
            (disposition, datetime) => {
                panic!("unexpected {disposition} datetime resolution: {datetime:?}")
            }
        }
    }

    let allocations = allocate_collection_counters(
        &BTreeMap::new(),
        accepted
            .iter()
            .map(|(item, datetime)| NewItemCounterRequest {
                item_id: item.primary.as_str().to_owned(),
                source_path_order_key: by_id[&item.primary].path.order_key(),
                accepted_datetime: *datetime,
            }),
    )
    .unwrap();
    let counters: BTreeMap<_, _> = allocations
        .into_iter()
        .map(|allocation| (allocation.item_id, allocation.counter))
        .collect();

    let mut canonical_files = Vec::new();
    for (item, datetime) in accepted {
        let counter = counters[item.primary.as_str()];
        let stem = CanonicalStem::new(datetime, counter).unwrap();
        for id in std::iter::once(&item.primary).chain(item.companion.iter()) {
            let extension = &by_id[id].extension;
            canonical_files.push(
                CanonicalFilename::new(&stem, extension)
                    .unwrap()
                    .to_string(),
            );
        }
        for sidecar in &item.sidecars {
            canonical_files.push(canonical_sidecar_filename(
                &stem,
                by_id[sidecar],
                std::iter::once(&item.primary)
                    .chain(item.companion.iter())
                    .map(|id| by_id[id]),
            ));
        }
    }

    let mut findings = resolution
        .findings
        .iter()
        .map(|finding| match finding {
            RelationshipFinding::OrphanSidecar(id) => {
                format!("{}: orphan-sidecar", id.as_str())
            }
            RelationshipFinding::UnknownContent(id) => format!("{}: unknown", id.as_str()),
            finding => panic!("unexpected corpus relationship finding: {finding:?}"),
        })
        .collect::<Vec<_>>();
    relationships.sort();
    findings.sort();
    accepted_sources.sort();
    disagreements.sort();
    canonical_files.sort();
    confirmations.sort();
    excluded.sort();
    CorpusOutcome {
        relationships,
        findings,
        accepted_sources,
        disagreements,
        canonical_files,
        confirmations,
        excluded,
    }
}

fn canonical_sidecar_filename<'a>(
    stem: &CanonicalStem,
    sidecar: &SourceFile,
    originals: impl IntoIterator<Item = &'a SourceFile>,
) -> String {
    let filename = sidecar.path.filename();
    let without_sidecar_extension = filename
        .strip_suffix(&format!(".{}", sidecar.extension))
        .expect("fixture sidecar extension matches its filename");
    for original in originals {
        if without_sidecar_extension.eq_ignore_ascii_case(&original.basename) {
            return format!("{stem}.{}", sidecar.extension);
        }
        if without_sidecar_extension.len() >= original.basename.len()
            && without_sidecar_extension[..original.basename.len()]
                .eq_ignore_ascii_case(&original.basename)
        {
            let preserved_suffix = &without_sidecar_extension[original.basename.len()..];
            return format!("{stem}{preserved_suffix}.{}", sidecar.extension);
        }
    }
    panic!("fixture sidecar has no owner")
}

fn source_file(fixture: &FixtureFile) -> SourceFile {
    let filename = fixture.path.last().expect("fixture path has filename");
    let (basename, extension) = filename.rsplit_once('.').unwrap_or((filename, ""));
    SourceFile {
        id: SourceFileId::new(&fixture.id).unwrap(),
        path: SourcePath::new(fixture.path.clone()).unwrap(),
        basename: basename.to_owned(),
        extension: extension.to_owned(),
        kind: souvenirs_metadata::classify_extension(extension).source_file_kind(),
        metadata: SourceMetadata {
            datetime_candidates: fixture
                .candidates
                .iter()
                .map(|candidate| datetime_candidate(&candidate[0], &candidate[1]))
                .collect(),
            camera: CameraIdentity {
                make: fixture.device[0].clone(),
                model: fixture.device[1].clone(),
                serial_number: fixture.device[2].clone(),
            },
            apple_content_identifier: fixture.content_identifier.clone(),
        },
    }
}

fn datetime_candidate(source: &str, value: &str) -> DateTimeCandidate {
    let source = match source {
        "exif_datetime_original" => DateTimeSource::ExifDateTimeOriginal,
        "exif_datetime_digitized" => DateTimeSource::ExifDateTimeDigitized,
        "quick_time_creation_date" => DateTimeSource::QuickTimeCreationDate,
        "xmp_date_created" => DateTimeSource::XmpDateCreated,
        "xmp_exif_datetime_original" => DateTimeSource::XmpExifDateTimeOriginal,
        "container_creation_time" => DateTimeSource::ContainerCreationTime,
        "google_takeout_photo_taken_time" => DateTimeSource::GoogleTakeoutPhotoTakenTime,
        "filename_pattern" => DateTimeSource::FilenamePattern,
        source => panic!("unknown fixture datetime source: {source}"),
    };
    let value = PrimitiveDateTime::parse(
        value,
        &time::format_description::well_known::Iso8601::DEFAULT,
    )
    .expect("valid fixture datetime");
    let value = match source {
        DateTimeSource::QuickTimeCreationDate => DateTimeValue::LocalWithOffset {
            local: value,
            offset: UtcOffset::UTC,
        },
        DateTimeSource::ContainerCreationTime => DateTimeValue::AmbiguousContainer(value),
        _ => DateTimeValue::Local(value),
    };
    DateTimeCandidate { source, value }
}

fn source_label(source: DateTimeSource) -> &'static str {
    match source {
        DateTimeSource::ExifDateTimeOriginal => "exif-original",
        DateTimeSource::ExifDateTimeDigitized => "exif-digitized",
        DateTimeSource::QuickTimeCreationDate => "quicktime-creation-date",
        DateTimeSource::XmpDateCreated => "xmp-date-created",
        DateTimeSource::XmpExifDateTimeOriginal => "xmp-exif-original",
        DateTimeSource::ContainerCreationTime => "container-creation-time",
        DateTimeSource::GoogleTakeoutPhotoTakenTime => "google-takeout-photo-taken-time",
        DateTimeSource::FilenamePattern => "filename-pattern",
    }
}

fn candidate_value_label(candidate: &DateTimeCandidate) -> String {
    match &candidate.value {
        DateTimeValue::Local(value) | DateTimeValue::LocalWithOffset { local: value, .. } => {
            value.to_string()
        }
        DateTimeValue::UtcWithoutLocalContext(value) => format!("{value} UTC"),
        DateTimeValue::AmbiguousContainer(value) => format!("{value} ambiguous"),
        DateTimeValue::DateOnly(value) => value.to_string(),
    }
}
