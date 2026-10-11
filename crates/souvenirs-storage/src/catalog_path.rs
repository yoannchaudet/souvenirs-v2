use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};

use souvenirs_domain::names::{
    CollisionKey, PathCollisionKey, PortableComponent, PortableRelativePath,
};

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

pub struct OutputPathObservation {
    resolved_path: PathBuf,
    catalog_relative: Option<PathBuf>,
}

impl OutputPathObservation {
    pub fn resolved_path(&self) -> &Path {
        &self.resolved_path
    }

    pub fn is_in_catalog(&self) -> bool {
        self.catalog_relative.is_some()
    }

    pub fn overlaps_catalog_path(&self, relative_path: &Path) -> Result<bool, CatalogPathError> {
        let Some(candidate) = &self.catalog_relative else {
            return Ok(false);
        };
        if candidate.as_os_str().is_empty() {
            return Ok(false);
        }

        let candidate = portable_relative_path(candidate)?;
        let planned = portable_relative_path(relative_path)?;
        Ok(collision_keys_overlap(
            &candidate.collision_key(),
            &planned.collision_key(),
        ))
    }
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

    pub fn observe_output_path(
        &self,
        output_path: &Path,
    ) -> Result<OutputPathObservation, CatalogPathError> {
        let file_name = output_path
            .file_name()
            .ok_or_else(|| CatalogPathError::Invalid(output_path.to_path_buf()))?;
        let parent = output_path.parent().unwrap_or_else(|| Path::new("."));
        let canonical_parent = fs::canonicalize(parent).map_err(|source| CatalogPathError::Io {
            action: "resolve output directory",
            path: parent.to_path_buf(),
            source,
        })?;
        let unresolved_path = canonical_parent.join(file_name);
        let resolved_path = match fs::symlink_metadata(output_path) {
            Ok(_) => match fs::canonicalize(output_path) {
                Ok(path) => path,
                Err(source) if source.kind() == io::ErrorKind::NotFound => {
                    return Err(CatalogPathError::UnresolvableOutput(
                        output_path.to_path_buf(),
                    ));
                }
                Err(source) => {
                    return Err(CatalogPathError::Io {
                        action: "resolve existing output path",
                        path: output_path.to_path_buf(),
                        source,
                    });
                }
            },
            Err(source) if source.kind() == io::ErrorKind::NotFound => unresolved_path,
            Err(source) => {
                return Err(CatalogPathError::Io {
                    action: "inspect output path",
                    path: output_path.to_path_buf(),
                    source,
                });
            }
        };
        let catalog_relative = resolved_path
            .strip_prefix(&self.canonical_root)
            .ok()
            .map(Path::to_path_buf);

        Ok(OutputPathObservation {
            resolved_path,
            catalog_relative,
        })
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

fn portable_relative_path(path: &Path) -> Result<PortableRelativePath, CatalogPathError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(CatalogPathError::Invalid(path.to_path_buf()));
    }

    PortableRelativePath::new(
        path.components()
            .map(|component| {
                component
                    .as_os_str()
                    .to_str()
                    .ok_or_else(|| CatalogPathError::Invalid(path.to_path_buf()))
            })
            .collect::<Result<Vec<_>, _>>()?,
    )
    .map_err(|_| CatalogPathError::Invalid(path.to_path_buf()))
}

fn collision_keys_overlap(left: &PathCollisionKey, right: &PathCollisionKey) -> bool {
    let common_length = left.components().len().min(right.components().len());
    left.components()[..common_length] == right.components()[..common_length]
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
    UnresolvableOutput(PathBuf),
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
            Self::UnresolvableOutput(path) => write!(
                formatter,
                "existing output path cannot be resolved: {}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for CatalogPathError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::EscapesCatalog(_)
            | Self::Invalid(_)
            | Self::Reserved(_)
            | Self::UnresolvableOutput(_) => None,
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

    #[test]
    fn observes_a_nonexistent_output_without_creating_it() {
        let temporary = tempfile::tempdir().unwrap();
        let catalog = temporary.path().join("catalog");
        let output_directory = temporary.path().join("output");
        fs::create_dir(&catalog).unwrap();
        fs::create_dir(&output_directory).unwrap();
        let layout = CatalogLayout::new(&catalog);
        let observer = PortableDestinationObserver::new(&layout).unwrap();
        let output = output_directory.join("progress.jsonl");

        let observation = observer.observe_output_path(&output).unwrap();

        assert_eq!(
            observation.resolved_path(),
            fs::canonicalize(&output_directory)
                .unwrap()
                .join("progress.jsonl")
        );
        assert!(!observation.is_in_catalog());
        assert!(!output.exists());
    }

    #[test]
    fn observes_portable_overlap_for_catalog_outputs() {
        let temporary = tempfile::tempdir().unwrap();
        let layout = CatalogLayout::new(temporary.path());
        fs::create_dir(temporary.path().join("2026")).unwrap();
        fs::create_dir(temporary.path().join("2026").join("Café")).unwrap();
        let observer = PortableDestinationObserver::new(&layout).unwrap();
        let output = temporary.path().join("2026").join("Café").join("PHOTO.jpg");

        let observation = observer.observe_output_path(&output).unwrap();

        assert!(observation.is_in_catalog());
        assert!(observation
            .overlaps_catalog_path(Path::new("2026/CAFE\u{301}/photo.JPG"))
            .unwrap());
        assert!(observation
            .overlaps_catalog_path(Path::new("2026/café"))
            .unwrap());
        assert!(!observation
            .overlaps_catalog_path(Path::new("2026/another/photo.jpg"))
            .unwrap());
        assert!(!output.exists());
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

    #[cfg(unix)]
    #[test]
    fn resolves_an_output_parent_alias_into_the_catalog() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let catalog = temporary.path().join("catalog");
        fs::create_dir(&catalog).unwrap();
        fs::create_dir(catalog.join("2026")).unwrap();
        symlink(catalog.join("2026"), temporary.path().join("alias")).unwrap();
        let layout = CatalogLayout::new(&catalog);
        let observer = PortableDestinationObserver::new(&layout).unwrap();
        let output = temporary.path().join("alias").join("progress.jsonl");

        let observation = observer.observe_output_path(&output).unwrap();

        assert!(observation.is_in_catalog());
        assert_eq!(
            observation.resolved_path(),
            fs::canonicalize(catalog.join("2026"))
                .unwrap()
                .join("progress.jsonl")
        );
        assert!(!output.exists());
    }

    #[cfg(unix)]
    #[test]
    fn resolves_an_existing_output_symlink_to_its_target() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let catalog = temporary.path().join("catalog");
        let outside = temporary.path().join("outside");
        fs::create_dir(&catalog).unwrap();
        fs::create_dir(&outside).unwrap();
        let target = outside.join("plan.json");
        fs::write(&target, b"plan").unwrap();
        let output = outside.join("progress.jsonl");
        symlink(&target, &output).unwrap();
        let layout = CatalogLayout::new(&catalog);
        let observer = PortableDestinationObserver::new(&layout).unwrap();

        let observation = observer.observe_output_path(&output).unwrap();

        assert_eq!(
            observation.resolved_path(),
            fs::canonicalize(target).unwrap()
        );
        assert!(!observation.is_in_catalog());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_an_unresolvable_existing_output_symlink() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let catalog = temporary.path().join("catalog");
        let outside = temporary.path().join("outside");
        fs::create_dir(&catalog).unwrap();
        fs::create_dir(&outside).unwrap();
        let output = outside.join("progress.jsonl");
        symlink(outside.join("missing.jsonl"), &output).unwrap();
        let layout = CatalogLayout::new(&catalog);
        let observer = PortableDestinationObserver::new(&layout).unwrap();

        assert!(matches!(
            observer.observe_output_path(&output),
            Err(CatalogPathError::UnresolvableOutput(path)) if path == output
        ));
    }
}
