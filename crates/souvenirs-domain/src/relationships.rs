use std::collections::{BTreeMap, BTreeSet};

use crate::source::{OriginalRole, SidecarNamingForm, SourceFile, SourceFileId, SourceFileKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompoundKind {
    RawJpeg,
    LivePhoto,
    ExistingCanonical,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelationshipStatus {
    Resolved,
    NeedsDecision,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogicalItem {
    pub primary: SourceFileId,
    pub companion: Option<SourceFileId>,
    pub sidecars: Vec<SourceFileId>,
    pub compound_kind: Option<CompoundKind>,
    pub status: RelationshipStatus,
}

impl LogicalItem {
    fn original_ids(&self) -> impl Iterator<Item = &SourceFileId> {
        std::iter::once(&self.primary).chain(self.companion.iter())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RelationshipFinding {
    AmbiguousRawJpeg(Vec<SourceFileId>),
    AmbiguousLivePhoto {
        content_identifier: String,
        files: Vec<SourceFileId>,
    },
    AmbiguousCanonicalCompound(Vec<SourceFileId>),
    ConflictingCompound(Vec<SourceFileId>),
    OrphanSidecar(SourceFileId),
    AmbiguousSidecar {
        sidecar: SourceFileId,
        candidate_items: Vec<SourceFileId>,
    },
    UnknownContent(SourceFileId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelationshipResolution {
    pub items: Vec<LogicalItem>,
    pub findings: Vec<RelationshipFinding>,
}

pub fn resolve_directory_relationships(files: &[SourceFile]) -> RelationshipResolution {
    let mut originals: Vec<_> = files
        .iter()
        .filter(|file| matches!(file.kind, SourceFileKind::Original(_)))
        .collect();
    originals.sort_by(|left, right| left.path.cmp(&right.path));

    let by_id: BTreeMap<_, _> = files.iter().map(|file| (file.id.clone(), file)).collect();
    let mut assigned = BTreeSet::new();
    let mut blocked = BTreeSet::new();
    let mut pairs = Vec::new();
    let mut findings = Vec::new();

    pair_raw_jpeg(
        &originals,
        &mut assigned,
        &mut blocked,
        &mut pairs,
        &mut findings,
    );
    pair_live_photos(
        &originals,
        &mut assigned,
        &mut blocked,
        &mut pairs,
        &mut findings,
    );
    pair_canonical_compounds(
        &originals,
        &mut assigned,
        &mut blocked,
        &mut pairs,
        &mut findings,
    );

    let mut items: Vec<_> = pairs
        .into_iter()
        .map(|(primary, companion, compound_kind)| LogicalItem {
            status: if blocked.contains(&primary) || blocked.contains(&companion) {
                RelationshipStatus::NeedsDecision
            } else {
                RelationshipStatus::Resolved
            },
            primary,
            companion: Some(companion),
            sidecars: Vec::new(),
            compound_kind: Some(compound_kind),
        })
        .collect();

    for original in originals {
        if !assigned.contains(&original.id) {
            items.push(LogicalItem {
                primary: original.id.clone(),
                companion: None,
                sidecars: Vec::new(),
                compound_kind: None,
                status: if blocked.contains(&original.id) {
                    RelationshipStatus::NeedsDecision
                } else {
                    RelationshipStatus::Resolved
                },
            });
        }
    }

    items.sort_by(|left, right| by_id[&left.primary].path.cmp(&by_id[&right.primary].path));

    attach_sidecars(files, &by_id, &mut items, &mut findings);

    findings.extend(
        files
            .iter()
            .filter(|file| matches!(file.kind, SourceFileKind::Unknown))
            .map(|file| RelationshipFinding::UnknownContent(file.id.clone())),
    );
    findings.sort_by(|left, right| {
        by_id[finding_file_id(left)]
            .path
            .cmp(&by_id[finding_file_id(right)].path)
            .then_with(|| finding_kind(left).cmp(&finding_kind(right)))
    });

    RelationshipResolution { items, findings }
}

fn pair_raw_jpeg(
    originals: &[&SourceFile],
    assigned: &mut BTreeSet<SourceFileId>,
    blocked: &mut BTreeSet<SourceFileId>,
    pairs: &mut Vec<(SourceFileId, SourceFileId, CompoundKind)>,
    findings: &mut Vec<RelationshipFinding>,
) {
    let mut by_basename: BTreeMap<(Vec<String>, &str), Vec<&SourceFile>> = BTreeMap::new();
    for file in originals {
        by_basename
            .entry((file.path.directory().to_vec(), &file.basename))
            .or_default()
            .push(file);
    }

    for files in by_basename.into_values() {
        let jpegs: Vec<_> = files
            .iter()
            .filter(|file| matches!(file.kind, SourceFileKind::Original(OriginalRole::Jpeg)))
            .collect();
        let raws: Vec<_> = files
            .iter()
            .filter(|file| matches!(file.kind, SourceFileKind::Original(OriginalRole::Raw)))
            .collect();
        if jpegs.len() == 1 && raws.len() == 1 {
            assigned.insert(jpegs[0].id.clone());
            assigned.insert(raws[0].id.clone());
            pairs.push((
                jpegs[0].id.clone(),
                raws[0].id.clone(),
                CompoundKind::RawJpeg,
            ));
        } else if !jpegs.is_empty() && !raws.is_empty() {
            let ids = sorted_ids(jpegs.into_iter().copied().chain(raws.into_iter().copied()));
            blocked.extend(ids.iter().cloned());
            findings.push(RelationshipFinding::AmbiguousRawJpeg(ids));
        }
    }
}

fn pair_live_photos(
    originals: &[&SourceFile],
    assigned: &mut BTreeSet<SourceFileId>,
    blocked: &mut BTreeSet<SourceFileId>,
    pairs: &mut Vec<(SourceFileId, SourceFileId, CompoundKind)>,
    findings: &mut Vec<RelationshipFinding>,
) {
    let mut by_identifier: BTreeMap<(Vec<String>, &str), Vec<&SourceFile>> = BTreeMap::new();
    for file in originals {
        if let Some(identifier) = file.metadata.apple_content_identifier.as_deref() {
            by_identifier
                .entry((file.path.directory().to_vec(), identifier))
                .or_default()
                .push(file);
        }
    }

    for ((_, identifier), files) in by_identifier {
        let stills: Vec<_> = files
            .iter()
            .filter(|file| original_role(file).is_some_and(OriginalRole::is_still))
            .collect();
        let videos: Vec<_> = files
            .iter()
            .filter(|file| original_role(file).is_some_and(OriginalRole::is_video))
            .collect();
        if stills.len() == 1 && videos.len() == 1 {
            if assigned.contains(&stills[0].id) || assigned.contains(&videos[0].id) {
                let mut ids = sorted_ids(files.iter().copied());
                loop {
                    let previous_len = ids.len();
                    for (primary, companion, _) in pairs.iter() {
                        if ids.contains(primary) || ids.contains(companion) {
                            ids.push(primary.clone());
                            ids.push(companion.clone());
                        }
                    }
                    ids.sort();
                    ids.dedup();
                    if ids.len() == previous_len {
                        break;
                    }
                }
                ids.sort_by(|left, right| {
                    originals
                        .iter()
                        .find(|file| file.id == *left)
                        .expect("paired files are originals")
                        .path
                        .cmp(
                            &originals
                                .iter()
                                .find(|file| file.id == *right)
                                .expect("paired files are originals")
                                .path,
                        )
                });
                blocked.extend(ids.iter().cloned());
                findings.push(RelationshipFinding::ConflictingCompound(ids));
            } else {
                let compound_kind = if matches!(
                    stills[0].kind,
                    SourceFileKind::Original(OriginalRole::CanonicalStill)
                ) && matches!(
                    videos[0].kind,
                    SourceFileKind::Original(OriginalRole::CanonicalVideo)
                ) && stills[0].basename == videos[0].basename
                {
                    CompoundKind::ExistingCanonical
                } else {
                    CompoundKind::LivePhoto
                };
                assigned.insert(stills[0].id.clone());
                assigned.insert(videos[0].id.clone());
                pairs.push((stills[0].id.clone(), videos[0].id.clone(), compound_kind));
            }
        } else if !stills.is_empty() && !videos.is_empty() {
            let ids = sorted_ids(files.iter().copied());
            blocked.extend(ids.iter().cloned());
            findings.push(RelationshipFinding::AmbiguousLivePhoto {
                content_identifier: identifier.to_owned(),
                files: ids,
            });
        }
    }
}

fn pair_canonical_compounds(
    originals: &[&SourceFile],
    assigned: &mut BTreeSet<SourceFileId>,
    blocked: &mut BTreeSet<SourceFileId>,
    pairs: &mut Vec<(SourceFileId, SourceFileId, CompoundKind)>,
    findings: &mut Vec<RelationshipFinding>,
) {
    let mut by_stem: BTreeMap<(Vec<String>, &str), Vec<&SourceFile>> = BTreeMap::new();
    for file in originals.iter().filter(|file| !assigned.contains(&file.id)) {
        if matches!(
            file.kind,
            SourceFileKind::Original(OriginalRole::CanonicalStill | OriginalRole::CanonicalVideo)
        ) {
            by_stem
                .entry((file.path.directory().to_vec(), &file.basename))
                .or_default()
                .push(file);
        }
    }

    for files in by_stem.into_values() {
        let stills: Vec<_> = files
            .iter()
            .filter(|file| {
                matches!(
                    file.kind,
                    SourceFileKind::Original(OriginalRole::CanonicalStill)
                )
            })
            .collect();
        let videos: Vec<_> = files
            .iter()
            .filter(|file| {
                matches!(
                    file.kind,
                    SourceFileKind::Original(OriginalRole::CanonicalVideo)
                )
            })
            .collect();
        if stills.len() == 1 && videos.len() == 1 {
            assigned.insert(stills[0].id.clone());
            assigned.insert(videos[0].id.clone());
            pairs.push((
                stills[0].id.clone(),
                videos[0].id.clone(),
                CompoundKind::ExistingCanonical,
            ));
        } else if !stills.is_empty() && !videos.is_empty() {
            let ids = sorted_ids(files.iter().copied());
            blocked.extend(ids.iter().cloned());
            findings.push(RelationshipFinding::AmbiguousCanonicalCompound(ids));
        }
    }
}

fn attach_sidecars(
    files: &[SourceFile],
    by_id: &BTreeMap<SourceFileId, &SourceFile>,
    items: &mut [LogicalItem],
    findings: &mut Vec<RelationshipFinding>,
) {
    let mut sidecars: Vec<_> = files
        .iter()
        .filter(|file| matches!(file.kind, SourceFileKind::Sidecar { .. }))
        .collect();
    sidecars.sort_by(|left, right| left.path.cmp(&right.path));

    for sidecar in sidecars {
        let SourceFileKind::Sidecar { naming_forms } = &sidecar.kind else {
            continue;
        };
        let mut candidates = Vec::new();
        for (index, item) in items.iter().enumerate() {
            if item
                .original_ids()
                .any(|id| sidecar_matches_original(sidecar, by_id[id], naming_forms.as_slice()))
            {
                candidates.push(index);
            }
        }

        match candidates.as_slice() {
            [] => findings.push(RelationshipFinding::OrphanSidecar(sidecar.id.clone())),
            [index] => items[*index].sidecars.push(sidecar.id.clone()),
            _ => {
                let candidate_items: Vec<_> = candidates
                    .iter()
                    .map(|index| items[*index].primary.clone())
                    .collect();
                for index in candidates {
                    items[index].status = RelationshipStatus::NeedsDecision;
                }
                findings.push(RelationshipFinding::AmbiguousSidecar {
                    sidecar: sidecar.id.clone(),
                    candidate_items,
                });
            }
        }
    }
}

fn sidecar_matches_original(
    sidecar: &SourceFile,
    original: &SourceFile,
    forms: &[SidecarNamingForm],
) -> bool {
    if sidecar.path.directory() != original.path.directory() {
        return false;
    }

    let sidecar_name = sidecar.path.filename();
    let Some((without_sidecar_extension, actual_extension)) = sidecar_name.rsplit_once('.') else {
        return false;
    };
    if !actual_extension.eq_ignore_ascii_case(&sidecar.extension) {
        return false;
    }

    forms.iter().any(|form| match form {
        SidecarNamingForm::Basename => {
            without_sidecar_extension.eq_ignore_ascii_case(&original.basename)
        }
        SidecarNamingForm::AssetFilename => {
            without_sidecar_extension.eq_ignore_ascii_case(original.path.filename())
        }
        SidecarNamingForm::NumberedAssetFilename => {
            let Some((numbered_basename, asset_extension)) =
                without_sidecar_extension.rsplit_once('.')
            else {
                return false;
            };
            if !asset_extension.eq_ignore_ascii_case(&original.extension) {
                return false;
            }
            numbered_basename
                .rsplit_once('_')
                .is_some_and(|(basename, number)| {
                    basename.eq_ignore_ascii_case(&original.basename)
                        && number.len() == 2
                        && number.bytes().all(|byte| byte.is_ascii_digit())
                })
        }
    })
}

fn original_role(file: &SourceFile) -> Option<OriginalRole> {
    match file.kind {
        SourceFileKind::Original(role) => Some(role),
        SourceFileKind::Sidecar { .. } | SourceFileKind::Unknown => None,
    }
}

fn sorted_ids<'a>(files: impl IntoIterator<Item = &'a SourceFile>) -> Vec<SourceFileId> {
    let mut files: Vec<_> = files.into_iter().collect();
    files.sort_by(|left, right| left.path.cmp(&right.path));
    files.into_iter().map(|file| file.id.clone()).collect()
}

fn finding_file_id(finding: &RelationshipFinding) -> &SourceFileId {
    match finding {
        RelationshipFinding::AmbiguousRawJpeg(files)
        | RelationshipFinding::AmbiguousCanonicalCompound(files)
        | RelationshipFinding::ConflictingCompound(files) => &files[0],
        RelationshipFinding::AmbiguousLivePhoto { files, .. } => &files[0],
        RelationshipFinding::OrphanSidecar(file) | RelationshipFinding::UnknownContent(file) => {
            file
        }
        RelationshipFinding::AmbiguousSidecar { sidecar, .. } => sidecar,
    }
}

fn finding_kind(finding: &RelationshipFinding) -> u8 {
    match finding {
        RelationshipFinding::AmbiguousRawJpeg(_) => 0,
        RelationshipFinding::AmbiguousLivePhoto { .. } => 1,
        RelationshipFinding::AmbiguousCanonicalCompound(_) => 2,
        RelationshipFinding::ConflictingCompound(_) => 3,
        RelationshipFinding::OrphanSidecar(_) => 4,
        RelationshipFinding::AmbiguousSidecar { .. } => 5,
        RelationshipFinding::UnknownContent(_) => 6,
    }
}

#[cfg(test)]
mod tests {
    use crate::datetime::DateTimeCandidate;
    use crate::source::{CameraIdentity, SourceMetadata, SourcePath};

    use super::*;

    fn file(
        id: &str,
        filename: &str,
        basename: &str,
        extension: &str,
        kind: SourceFileKind,
        content_identifier: Option<&str>,
    ) -> SourceFile {
        SourceFile {
            id: SourceFileId::new(id).unwrap(),
            path: SourcePath::new(["card", filename]).unwrap(),
            basename: basename.into(),
            extension: extension.into(),
            kind,
            metadata: SourceMetadata {
                datetime_candidates: Vec::<DateTimeCandidate>::new(),
                camera: CameraIdentity {
                    make: None,
                    model: None,
                    serial_number: None,
                },
                apple_content_identifier: content_identifier.map(str::to_owned),
            },
        }
    }

    #[test]
    fn pairs_raw_jpeg_live_photos_and_sidecars() {
        let files = vec![
            file(
                "jpeg",
                "IMG_1.JPG",
                "IMG_1",
                "JPG",
                SourceFileKind::Original(OriginalRole::Jpeg),
                None,
            ),
            file(
                "raw",
                "IMG_1.CR3",
                "IMG_1",
                "CR3",
                SourceFileKind::Original(OriginalRole::Raw),
                None,
            ),
            file(
                "still",
                "IMG_2.HEIC",
                "IMG_2",
                "HEIC",
                SourceFileKind::Original(OriginalRole::Still),
                Some("live-1"),
            ),
            file(
                "video",
                "IMG_2.MOV",
                "IMG_2",
                "MOV",
                SourceFileKind::Original(OriginalRole::Video),
                Some("live-1"),
            ),
            file(
                "xmp",
                "IMG_1.JPG.xmp",
                "IMG_1.JPG",
                "xmp",
                SourceFileKind::Sidecar {
                    naming_forms: vec![
                        SidecarNamingForm::Basename,
                        SidecarNamingForm::AssetFilename,
                        SidecarNamingForm::NumberedAssetFilename,
                    ],
                },
                None,
            ),
        ];

        let resolution = resolve_directory_relationships(&files);

        assert!(resolution.findings.is_empty());
        assert_eq!(resolution.items.len(), 2);
        assert_eq!(
            resolution.items[0].compound_kind,
            Some(CompoundKind::RawJpeg)
        );
        assert_eq!(
            resolution.items[0].sidecars,
            [SourceFileId::new("xmp").unwrap()]
        );
        assert_eq!(
            resolution.items[1].compound_kind,
            Some(CompoundKind::LivePhoto)
        );
    }

    #[test]
    fn matching_basename_does_not_pair_other_still_and_video() {
        let files = vec![
            file(
                "still",
                "clip.jpg",
                "clip",
                "jpg",
                SourceFileKind::Original(OriginalRole::Still),
                None,
            ),
            file(
                "video",
                "clip.mov",
                "clip",
                "mov",
                SourceFileKind::Original(OriginalRole::Video),
                None,
            ),
        ];

        let resolution = resolve_directory_relationships(&files);

        assert_eq!(resolution.items.len(), 2);
        assert!(resolution.items.iter().all(|item| item.companion.is_none()));
    }

    #[test]
    fn canonical_compounds_keep_their_kind_with_content_identifiers() {
        let files = vec![
            file(
                "still",
                "2024-07-09 05-02-03 001.jpg",
                "2024-07-09 05-02-03 001",
                "jpg",
                SourceFileKind::Original(OriginalRole::CanonicalStill),
                Some("live"),
            ),
            file(
                "video",
                "2024-07-09 05-02-03 001.mov",
                "2024-07-09 05-02-03 001",
                "mov",
                SourceFileKind::Original(OriginalRole::CanonicalVideo),
                Some("live"),
            ),
        ];

        let resolution = resolve_directory_relationships(&files);

        assert_eq!(
            resolution.items[0].compound_kind,
            Some(CompoundKind::ExistingCanonical)
        );
    }

    #[test]
    fn ambiguous_relationships_do_not_depend_on_input_order() {
        let files = vec![
            file(
                "still-a",
                "a.heic",
                "a",
                "heic",
                SourceFileKind::Original(OriginalRole::Still),
                Some("duplicate"),
            ),
            file(
                "still-b",
                "b.heic",
                "b",
                "heic",
                SourceFileKind::Original(OriginalRole::Still),
                Some("duplicate"),
            ),
            file(
                "video",
                "c.mov",
                "c",
                "mov",
                SourceFileKind::Original(OriginalRole::Video),
                Some("duplicate"),
            ),
        ];
        let reversed: Vec<_> = files.iter().cloned().rev().collect();

        assert_eq!(
            resolve_directory_relationships(&files),
            resolve_directory_relationships(&reversed)
        );
    }

    #[test]
    fn sidecars_never_match_across_directories() {
        let original = file(
            "original",
            "image.jpg",
            "image",
            "jpg",
            SourceFileKind::Original(OriginalRole::Jpeg),
            None,
        );
        let mut sidecar = file(
            "sidecar",
            "image.xmp",
            "image",
            "xmp",
            SourceFileKind::Sidecar {
                naming_forms: vec![SidecarNamingForm::Basename],
            },
            None,
        );
        sidecar.path = SourcePath::new(["other", "image.xmp"]).unwrap();

        let resolution = resolve_directory_relationships(&[original, sidecar]);

        assert!(matches!(
            resolution.findings.as_slice(),
            [RelationshipFinding::OrphanSidecar(_)]
        ));
    }

    #[test]
    fn numbered_sidecar_asset_extensions_are_case_insensitive() {
        let original = file(
            "original",
            "image.jpg",
            "image",
            "jpg",
            SourceFileKind::Original(OriginalRole::Jpeg),
            None,
        );
        let sidecar = file(
            "sidecar",
            "image_01.JPG.XMP",
            "image_01.JPG",
            "xmp",
            SourceFileKind::Sidecar {
                naming_forms: vec![SidecarNamingForm::NumberedAssetFilename],
            },
            None,
        );

        let resolution = resolve_directory_relationships(&[original, sidecar]);

        assert!(resolution.findings.is_empty());
        assert_eq!(
            resolution.items[0].sidecars,
            [SourceFileId::new("sidecar").unwrap()]
        );
    }

    #[test]
    fn sidecar_asset_names_are_case_insensitive() {
        for naming_form in [
            SidecarNamingForm::Basename,
            SidecarNamingForm::AssetFilename,
        ] {
            let original = file(
                "original",
                "image.jpg",
                "image",
                "jpg",
                SourceFileKind::Original(OriginalRole::Jpeg),
                None,
            );
            let sidecar = file(
                "sidecar",
                match naming_form {
                    SidecarNamingForm::Basename => "IMAGE.XMP",
                    SidecarNamingForm::AssetFilename => "IMAGE.JPG.XMP",
                    SidecarNamingForm::NumberedAssetFilename => unreachable!(),
                },
                "sidecar",
                "xmp",
                SourceFileKind::Sidecar {
                    naming_forms: vec![naming_form],
                },
                None,
            );

            let resolution = resolve_directory_relationships(&[original, sidecar]);

            assert!(resolution.findings.is_empty());
            assert_eq!(
                resolution.items[0].sidecars,
                [SourceFileId::new("sidecar").unwrap()]
            );
        }
    }

    #[test]
    fn overlapping_compound_relationships_require_a_decision() {
        let files = vec![
            file(
                "jpeg",
                "image.jpg",
                "image",
                "jpg",
                SourceFileKind::Original(OriginalRole::Jpeg),
                Some("live"),
            ),
            file(
                "raw",
                "image.raw",
                "image",
                "raw",
                SourceFileKind::Original(OriginalRole::Raw),
                None,
            ),
            file(
                "video",
                "image.mov",
                "image",
                "mov",
                SourceFileKind::Original(OriginalRole::Video),
                Some("live"),
            ),
        ];

        let resolution = resolve_directory_relationships(&files);

        let conflicting = resolution
            .findings
            .iter()
            .find_map(|finding| match finding {
                RelationshipFinding::ConflictingCompound(ids) => Some(ids),
                _ => None,
            })
            .expect("overlapping compounds should be reported");
        assert_eq!(
            conflicting,
            &[
                SourceFileId::new("jpeg").unwrap(),
                SourceFileId::new("video").unwrap(),
                SourceFileId::new("raw").unwrap(),
            ]
        );
        assert!(resolution
            .items
            .iter()
            .all(|item| item.status == RelationshipStatus::NeedsDecision));
    }

    #[test]
    fn originals_never_pair_across_directories() {
        let first = file(
            "jpeg",
            "image.jpg",
            "image",
            "jpg",
            SourceFileKind::Original(OriginalRole::Jpeg),
            None,
        );
        let mut second = file(
            "raw",
            "image.raw",
            "image",
            "raw",
            SourceFileKind::Original(OriginalRole::Raw),
            None,
        );
        second.path = SourcePath::new(["other", "image.raw"]).unwrap();

        let resolution = resolve_directory_relationships(&[first, second]);

        assert_eq!(resolution.items.len(), 2);
        assert!(resolution.items.iter().all(|item| item.companion.is_none()));
    }
}
