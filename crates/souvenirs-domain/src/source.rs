use std::fmt;

use crate::catalog::SourcePathOrderKey;
use crate::datetime::DateTimeCandidate;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SourceFileId(String);

impl SourceFileId {
    pub fn new(value: impl Into<String>) -> Result<Self, SourceError> {
        let value = value.into();
        if value.is_empty() {
            return Err(SourceError::EmptyFileId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SourcePath(Vec<String>);

impl SourcePath {
    pub fn new<I, S>(components: I) -> Result<Self, SourceError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let components: Vec<_> = components.into_iter().map(Into::into).collect();
        if components.is_empty() || components.last().is_some_and(String::is_empty) {
            return Err(SourceError::MissingFilename);
        }
        Ok(Self(components))
    }

    pub fn components(&self) -> &[String] {
        &self.0
    }

    pub fn directory(&self) -> &[String] {
        &self.0[..self.0.len() - 1]
    }

    pub fn filename(&self) -> &str {
        self.0.last().expect("a source path always has a filename")
    }

    pub fn order_key(&self) -> SourcePathOrderKey {
        SourcePathOrderKey::new(self.0.clone())
    }
}

impl fmt::Display for SourcePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0.join("/"))
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CameraIdentity {
    pub make: Option<String>,
    pub model: Option<String>,
    pub serial_number: Option<String>,
}

impl CameraIdentity {
    pub fn is_empty(&self) -> bool {
        self.make.is_none() && self.model.is_none() && self.serial_number.is_none()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceMetadata {
    pub datetime_candidates: Vec<DateTimeCandidate>,
    pub camera: CameraIdentity,
    pub apple_content_identifier: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OriginalRole {
    Jpeg,
    Raw,
    Still,
    Video,
    CanonicalStill,
    CanonicalVideo,
}

impl OriginalRole {
    pub fn is_still(self) -> bool {
        matches!(
            self,
            Self::Jpeg | Self::Raw | Self::Still | Self::CanonicalStill
        )
    }

    pub fn is_video(self) -> bool {
        matches!(self, Self::Video | Self::CanonicalVideo)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SidecarNamingForm {
    Basename,
    AssetFilename,
    NumberedAssetFilename,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceFileKind {
    Original(OriginalRole),
    Sidecar {
        naming_forms: Vec<SidecarNamingForm>,
    },
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceFile {
    pub id: SourceFileId,
    pub path: SourcePath,
    pub basename: String,
    pub extension: String,
    pub kind: SourceFileKind,
    pub metadata: SourceMetadata,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceError {
    EmptyFileId,
    MissingFilename,
}

impl fmt::Display for SourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyFileId => formatter.write_str("source file identifier must not be empty"),
            Self::MissingFilename => formatter.write_str("source path must contain a filename"),
        }
    }
}

impl std::error::Error for SourceError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_paths_preserve_nonportable_names() {
        let path = SourcePath::new(["card", "CON: photo?.jpg"]).unwrap();

        assert_eq!(path.directory(), ["card"]);
        assert_eq!(path.filename(), "CON: photo?.jpg");
        assert_eq!(path.order_key().components(), ["card", "CON: photo?.jpg"]);
    }
}
