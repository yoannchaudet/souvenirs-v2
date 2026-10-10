//! Media format classification and normalized metadata values.

use std::path::Path;

use souvenirs_domain::source::{OriginalRole, SidecarNamingForm, SourceFileKind};

pub const SUPPORTED_ORIGINAL_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "heic", "dng", "cr3", "nef", "arw", "mov", "mp4",
];
pub const SUPPORTED_SIDECAR_EXTENSIONS: &[&str] = &["xmp", "aae", "dop", "pp3"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaKind {
    Photo,
    Video,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssetKind {
    Original {
        media: MediaKind,
        role: OriginalRole,
    },
    Sidecar {
        naming_forms: Vec<SidecarNamingForm>,
    },
    Unsupported,
}

impl AssetKind {
    pub fn source_file_kind(&self) -> SourceFileKind {
        match self {
            Self::Original { role, .. } => SourceFileKind::Original(*role),
            Self::Sidecar { naming_forms } => SourceFileKind::Sidecar {
                naming_forms: naming_forms.clone(),
            },
            Self::Unsupported => SourceFileKind::Unknown,
        }
    }
}

pub fn classify_path(path: &Path) -> AssetKind {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(classify_extension)
        .unwrap_or(AssetKind::Unsupported)
}

pub fn classify_extension(extension: &str) -> AssetKind {
    let extension = extension.to_ascii_lowercase();
    match extension.as_str() {
        "jpg" | "jpeg" => original(MediaKind::Photo, OriginalRole::Jpeg),
        "heic" => original(MediaKind::Photo, OriginalRole::Still),
        "dng" | "cr3" | "nef" | "arw" => original(MediaKind::Photo, OriginalRole::Raw),
        "mov" | "mp4" => original(MediaKind::Video, OriginalRole::Video),
        "xmp" => AssetKind::Sidecar {
            naming_forms: vec![
                SidecarNamingForm::Basename,
                SidecarNamingForm::AssetFilename,
                SidecarNamingForm::NumberedAssetFilename,
            ],
        },
        "aae" => AssetKind::Sidecar {
            naming_forms: vec![SidecarNamingForm::Basename],
        },
        "dop" | "pp3" => AssetKind::Sidecar {
            naming_forms: vec![SidecarNamingForm::AssetFilename],
        },
        _ => AssetKind::Unsupported,
    }
}

fn original(media: MediaKind, role: OriginalRole) -> AssetKind {
    AssetKind::Original { media, role }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_is_case_insensitive() {
        assert_eq!(
            classify_extension("JpG"),
            AssetKind::Original {
                media: MediaKind::Photo,
                role: OriginalRole::Jpeg,
            }
        );
        assert_eq!(classify_extension("LRV"), AssetKind::Unsupported);
    }

    #[test]
    fn xmp_supports_all_declared_naming_forms() {
        assert_eq!(
            classify_extension("xmp"),
            AssetKind::Sidecar {
                naming_forms: vec![
                    SidecarNamingForm::Basename,
                    SidecarNamingForm::AssetFilename,
                    SidecarNamingForm::NumberedAssetFilename,
                ],
            }
        );
    }
}
