use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};

use souvenirs_domain::names::{CollisionKey, PortableComponent};

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

pub fn validate_directory(layout: &CatalogLayout, path: &Path) -> Result<(), CatalogPathError> {
    resolve_catalog_entry(layout, &path.join(".containment-check")).map(|_| ())
}

#[derive(Clone)]
struct ObservedEntry {
    collision_key: CollisionKey,
    path: PathBuf,
    is_directory: bool,
}

pub struct PortableDestinationObserver {
    canonical_root: PathBuf,
    directories: BTreeMap<PathBuf, Vec<ObservedEntry>>,
}

impl PortableDestinationObserver {
    pub fn new(layout: &CatalogLayout) -> Result<Self, CatalogPathError> {
        let canonical_root =
            fs::canonicalize(layout.root()).map_err(|source| CatalogPathError::Io {
                action: "resolve catalog root",
                path: layout.root().to_path_buf(),
                source,
            })?;
        Ok(Self {
            canonical_root,
            directories: BTreeMap::new(),
        })
    }

    pub fn is_occupied(&mut self, relative_path: &Path) -> Result<bool, CatalogPathError> {
        validate_catalog_relative(relative_path)?;
        let planned_components = relative_path
            .components()
            .map(|component| {
                let Component::Normal(component) = component else {
                    unreachable!("catalog-relative path was validated");
                };
                let component = component
                    .to_str()
                    .ok_or_else(|| CatalogPathError::Invalid(relative_path.to_path_buf()))?;
                PortableComponent::new(component)
                    .map_err(|_| CatalogPathError::Invalid(relative_path.to_path_buf()))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut candidate_directories = vec![self.canonical_root.clone()];
        for (index, planned_component) in planned_components.iter().enumerate() {
            let is_destination = index + 1 == planned_components.len();
            let mut next_directories = Vec::new();
            for directory in candidate_directories {
                for entry in self.entries(&directory, relative_path)? {
                    if entry.collision_key != planned_component.collision_key() {
                        continue;
                    }
                    if is_destination {
                        return Ok(true);
                    }
                    if !entry.is_directory {
                        return Ok(true);
                    }
                    next_directories.push(entry.path);
                }
            }
            if next_directories.is_empty() {
                return Ok(false);
            }
            candidate_directories = next_directories;
        }
        Ok(false)
    }

    fn entries(
        &mut self,
        directory: &Path,
        relative_path: &Path,
    ) -> Result<Vec<ObservedEntry>, CatalogPathError> {
        let canonical_directory =
            fs::canonicalize(directory).map_err(|source| CatalogPathError::Io {
                action: "resolve catalog destination directory",
                path: directory.to_path_buf(),
                source,
            })?;
        if !canonical_directory.starts_with(&self.canonical_root) {
            return Err(CatalogPathError::EscapesCatalog(
                relative_path.to_path_buf(),
            ));
        }
        if let Some(entries) = self.directories.get(&canonical_directory) {
            return Ok(entries.clone());
        }

        let mut observed = Vec::new();
        let entries =
            fs::read_dir(&canonical_directory).map_err(|source| CatalogPathError::Io {
                action: "read catalog destination directory",
                path: canonical_directory.clone(),
                source,
            })?;
        for entry in entries {
            let entry = entry.map_err(|source| CatalogPathError::Io {
                action: "read catalog destination entry",
                path: canonical_directory.clone(),
                source,
            })?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Ok(component) = PortableComponent::new(name) else {
                continue;
            };
            let metadata = entry.metadata().map_err(|source| CatalogPathError::Io {
                action: "read catalog destination entry metadata",
                path: entry.path(),
                source,
            })?;
            observed.push(ObservedEntry {
                collision_key: component.collision_key(),
                path: entry.path(),
                is_directory: metadata.is_dir(),
            });
        }
        self.directories
            .insert(canonical_directory, observed.clone());
        Ok(observed)
    }
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use crate::{CatalogLayout, PortableDestinationObserver};

    use super::CatalogPathError;

    #[test]
    fn observes_portable_destination_collisions_across_components() {
        let temporary = tempfile::tempdir().unwrap();
        let layout = CatalogLayout::new(temporary.path());
        fs::create_dir(temporary.path().join("2026")).unwrap();
        fs::create_dir(temporary.path().join("2026").join("Café")).unwrap();
        fs::write(
            temporary.path().join("2026").join("Café").join("Photo.JPG"),
            b"occupied",
        )
        .unwrap();
        let mut observer = PortableDestinationObserver::new(&layout).unwrap();

        assert!(observer
            .is_occupied(Path::new("2026/CAFE\u{301}/photo.jpg"))
            .unwrap());
        assert!(observer.is_occupied(Path::new("2026/cafe\u{301}")).unwrap());
        assert!(!observer
            .is_occupied(Path::new("2026/Café/another.jpg"))
            .unwrap());
    }

    #[test]
    fn treats_an_intermediate_file_as_an_occupied_destination() {
        let temporary = tempfile::tempdir().unwrap();
        let layout = CatalogLayout::new(temporary.path());
        fs::write(temporary.path().join("2026"), b"occupied").unwrap();
        let mut observer = PortableDestinationObserver::new(&layout).unwrap();

        assert!(observer.is_occupied(Path::new("2026/collection")).unwrap());
    }

    #[test]
    fn rejects_nonportable_planned_paths() {
        let temporary = tempfile::tempdir().unwrap();
        let layout = CatalogLayout::new(temporary.path());
        let mut observer = PortableDestinationObserver::new(&layout).unwrap();

        assert!(matches!(
            observer.is_occupied(Path::new("collection.")),
            Err(CatalogPathError::Invalid(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn traverses_an_intermediate_symlink_only_inside_the_catalog() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let layout = CatalogLayout::new(temporary.path());
        fs::create_dir(temporary.path().join("actual")).unwrap();
        fs::write(
            temporary.path().join("actual").join("Photo.JPG"),
            b"occupied",
        )
        .unwrap();
        symlink("actual", temporary.path().join("alias")).unwrap();
        let mut observer = PortableDestinationObserver::new(&layout).unwrap();

        assert!(observer.is_occupied(Path::new("ALIAS/photo.jpg")).unwrap());
    }
}
