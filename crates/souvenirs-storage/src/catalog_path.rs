use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::CatalogLayout;

pub(crate) struct CatalogEntry {
    pub path: PathBuf,
    pub resolved_path: PathBuf,
    pub directory_path: PathBuf,
    pub directory: File,
    pub file_name: OsString,
}

pub(crate) fn resolve_catalog_entry(
    layout: &CatalogLayout,
    relative_path: &Path,
) -> Result<CatalogEntry, CatalogPathError> {
    validate_catalog_relative(relative_path)?;

    let destination = layout.root().join(relative_path);
    let parent = destination
        .parent()
        .expect("non-empty relative destination has a parent");
    let canonical_root =
        fs::canonicalize(layout.root()).map_err(|source| CatalogPathError::Io {
            action: "resolve catalog root",
            path: layout.root().to_path_buf(),
            source,
        })?;
    let canonical_parent = fs::canonicalize(parent).map_err(|source| CatalogPathError::Io {
        action: "resolve catalog destination directory",
        path: parent.to_path_buf(),
        source,
    })?;
    if !canonical_parent.starts_with(&canonical_root) {
        return Err(CatalogPathError::EscapesCatalog(
            relative_path.to_path_buf(),
        ));
    }

    let directory = File::open(&canonical_parent).map_err(|source| CatalogPathError::Io {
        action: "open catalog destination directory",
        path: canonical_parent.clone(),
        source,
    })?;
    let file_name = destination
        .file_name()
        .expect("validated destination has a file name")
        .to_os_string();
    Ok(CatalogEntry {
        path: destination,
        resolved_path: canonical_parent.join(&file_name),
        directory_path: canonical_parent,
        directory,
        file_name,
    })
}

pub(crate) fn validate_catalog_relative(path: &Path) -> Result<(), CatalogPathError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(CatalogPathError::Invalid(path.to_path_buf()));
    }

    let first_component = path
        .components()
        .next()
        .expect("validated destination has a component")
        .as_os_str();
    if first_component == ".souvenirs" || path == Path::new(".souvenirs.yml") {
        return Err(CatalogPathError::Reserved(path.to_path_buf()));
    }
    Ok(())
}

#[derive(Debug)]
pub enum CatalogPathError {
    EscapesCatalog(PathBuf),
    Invalid(PathBuf),
    Io {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Reserved(PathBuf),
}

impl fmt::Display for CatalogPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EscapesCatalog(path) => write!(
                formatter,
                "catalog path escapes through a symbolic link: {}",
                path.display()
            ),
            Self::Invalid(path) => write!(
                formatter,
                "catalog path must be relative without traversal: {}",
                path.display()
            ),
            Self::Io {
                action,
                path,
                source,
            } => write!(formatter, "{action} at {}: {source}", path.display()),
            Self::Reserved(path) => {
                write!(formatter, "catalog path is reserved: {}", path.display())
            }
        }
    }
}

impl std::error::Error for CatalogPathError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::EscapesCatalog(_) | Self::Invalid(_) | Self::Reserved(_) => None,
        }
    }
}
