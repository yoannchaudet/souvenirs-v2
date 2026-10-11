use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OpenFlags};
use souvenirs_domain::names::PortableComponent;
use souvenirs_domain::plan::ContentDigest;

use crate::durability::{sync_directory, sync_file};

const INDEX_SCHEMA_VERSION: i64 = 1;

pub struct CatalogIndex {
    connection: Connection,
}

impl CatalogIndex {
    /// Rebuilds the disposable index from canonical manifests.
    ///
    /// The caller owns index placement and must prevent concurrent catalog mutation while the
    /// manifests are read. Media bytes are never hashed or otherwise inspected.
    pub fn rebuild(
        index_path: impl AsRef<Path>,
        catalog_root: impl AsRef<Path>,
    ) -> Result<Self, CatalogIndexError> {
        let index_path = index_path.as_ref();
        let catalog_root = catalog_root.as_ref();
        let parent = index_parent(index_path);
        if !parent.is_dir() {
            return Err(CatalogIndexError::MissingIndexDirectory(
                parent.to_path_buf(),
            ));
        }

        let rebuild_path = rebuild_path(index_path)?;
        remove_if_present(&rebuild_path)?;
        let result = (|| {
            let mut connection =
                Connection::open(&rebuild_path).map_err(|source| CatalogIndexError::Sqlite {
                    path: rebuild_path.clone(),
                    source,
                })?;
            initialize_schema(&connection, &rebuild_path)?;
            let originals = read_catalog_originals(catalog_root)?;
            let transaction =
                connection
                    .transaction()
                    .map_err(|source| CatalogIndexError::Sqlite {
                        path: rebuild_path.clone(),
                        source,
                    })?;
            {
                let mut insert = transaction
                    .prepare("INSERT INTO catalog_originals (digest, path) VALUES (?1, ?2)")
                    .map_err(|source| CatalogIndexError::Sqlite {
                        path: rebuild_path.clone(),
                        source,
                    })?;
                for original in originals {
                    let path = serde_json::to_string(&original.path)
                        .map_err(CatalogIndexError::SerializePath)?;
                    insert
                        .execute(params![original.digest.as_str(), path])
                        .map_err(|source| CatalogIndexError::Sqlite {
                            path: rebuild_path.clone(),
                            source,
                        })?;
                }
            }
            transaction
                .commit()
                .map_err(|source| CatalogIndexError::Sqlite {
                    path: rebuild_path.clone(),
                    source,
                })?;
            connection
                .close()
                .map_err(|(_, source)| CatalogIndexError::Sqlite {
                    path: rebuild_path.clone(),
                    source,
                })?;
            let rebuilt = File::open(&rebuild_path).map_err(|source| CatalogIndexError::Io {
                action: "open rebuilt catalog index for persistence",
                path: rebuild_path.clone(),
                source,
            })?;
            sync_file(&rebuilt).map_err(|source| CatalogIndexError::Io {
                action: "persist rebuilt catalog index",
                path: rebuild_path.clone(),
                source,
            })?;
            replace_index(&rebuild_path, index_path)?;
            Self::open(index_path)
        })();
        if result.is_err() {
            let _ = remove_if_present(&rebuild_path);
        }
        result
    }

    pub fn open(index_path: impl AsRef<Path>) -> Result<Self, CatalogIndexError> {
        let index_path = index_path.as_ref();
        let connection = Connection::open_with_flags(index_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|source| CatalogIndexError::Sqlite {
                path: index_path.to_path_buf(),
                source,
            })?;
        let version = connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .map_err(|source| CatalogIndexError::Sqlite {
                path: index_path.to_path_buf(),
                source,
            })?;
        if version != INDEX_SCHEMA_VERSION {
            return Err(CatalogIndexError::UnsupportedSchemaVersion {
                path: index_path.to_path_buf(),
                version,
            });
        }
        Ok(Self { connection })
    }

    pub fn paths_for_digest(
        &self,
        digest: &ContentDigest,
    ) -> Result<Vec<Vec<String>>, CatalogIndexError> {
        let mut statement = self
            .connection
            .prepare("SELECT path FROM catalog_originals WHERE digest = ?1")
            .map_err(CatalogIndexError::Query)?;
        let rows = statement
            .query_map([digest.as_str()], |row| row.get::<_, String>(0))
            .map_err(CatalogIndexError::Query)?;
        let mut paths = rows
            .map(|row| {
                row.map_err(CatalogIndexError::Query).and_then(|path| {
                    serde_json::from_str::<Vec<String>>(&path)
                        .map_err(CatalogIndexError::DeserializePath)
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        paths.sort();
        Ok(paths)
    }
}

#[derive(Debug)]
struct IndexedOriginal {
    digest: ContentDigest,
    path: Vec<String>,
}

fn initialize_schema(connection: &Connection, path: &Path) -> Result<(), CatalogIndexError> {
    connection
        .execute_batch(
            "
            PRAGMA journal_mode = OFF;
            PRAGMA user_version = 1;
            CREATE TABLE catalog_originals (
                digest TEXT NOT NULL,
                path TEXT NOT NULL UNIQUE
            );
            CREATE INDEX catalog_originals_digest ON catalog_originals (digest);
            ",
        )
        .map_err(|source| CatalogIndexError::Sqlite {
            path: path.to_path_buf(),
            source,
        })
}

fn read_catalog_originals(root: &Path) -> Result<Vec<IndexedOriginal>, CatalogIndexError> {
    let mut originals = Vec::new();
    for year in sorted_directories(root, true)? {
        let year_name = portable_entry_name(&year)?;
        for collection in sorted_directories(&year.path(), false)? {
            let collection_name = portable_entry_name(&collection)?;
            let manifest_path = collection.path().join("SHA256SUMS");
            match fs::symlink_metadata(&manifest_path) {
                Ok(metadata) if metadata.file_type().is_file() => {}
                Ok(_) => {
                    return Err(CatalogIndexError::InvalidManifest {
                        path: manifest_path,
                        line: None,
                        reason: "SHA256SUMS is not a regular file".to_owned(),
                    });
                }
                Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
                Err(source) => {
                    return Err(CatalogIndexError::Io {
                        action: "inspect catalog manifest",
                        path: manifest_path,
                        source,
                    });
                }
            }
            originals.extend(read_manifest(&manifest_path, &year_name, &collection_name)?);
        }
    }
    originals.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(originals)
}

fn read_manifest(
    manifest_path: &Path,
    year: &str,
    collection: &str,
) -> Result<Vec<IndexedOriginal>, CatalogIndexError> {
    let file = File::open(manifest_path).map_err(|source| CatalogIndexError::Io {
        action: "open catalog manifest",
        path: manifest_path.to_path_buf(),
        source,
    })?;
    let mut filenames = BTreeSet::new();
    let mut previous_filename: Option<String> = None;
    let mut originals = Vec::new();
    for (line_index, line) in BufReader::new(file).lines().enumerate() {
        let line_number = line_index + 1;
        let line = line.map_err(|source| CatalogIndexError::Io {
            action: "read catalog manifest",
            path: manifest_path.to_path_buf(),
            source,
        })?;
        let Some((digest, filename)) = line.split_once("  ") else {
            return Err(invalid_manifest(
                manifest_path,
                line_number,
                "expected '<sha256>  <filename>'",
            ));
        };
        let digest = ContentDigest::parse(digest.to_owned())
            .map_err(|_| invalid_manifest(manifest_path, line_number, "invalid SHA-256 digest"))?;
        let filename = PortableComponent::new(filename).map_err(|error| {
            invalid_manifest(
                manifest_path,
                line_number,
                format!("invalid filename: {error}"),
            )
        })?;
        if !filenames.insert(filename.as_str().to_owned()) {
            return Err(invalid_manifest(
                manifest_path,
                line_number,
                "duplicate filename",
            ));
        }
        if previous_filename
            .as_deref()
            .is_some_and(|previous| previous.as_bytes() >= filename.as_str().as_bytes())
        {
            return Err(invalid_manifest(
                manifest_path,
                line_number,
                "filenames are not in strict byte order",
            ));
        }
        previous_filename = Some(filename.as_str().to_owned());
        originals.push(IndexedOriginal {
            digest,
            path: vec![
                year.to_owned(),
                collection.to_owned(),
                filename.as_str().to_owned(),
            ],
        });
    }
    Ok(originals)
}

fn sorted_directories(
    path: &Path,
    skip_hidden: bool,
) -> Result<Vec<fs::DirEntry>, CatalogIndexError> {
    let entries = fs::read_dir(path)
        .map_err(|source| CatalogIndexError::Io {
            action: "read catalog directory",
            path: path.to_path_buf(),
            source,
        })?
        .map(|entry| {
            entry.map_err(|source| CatalogIndexError::Io {
                action: "read catalog entry",
                path: path.to_path_buf(),
                source,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut directories = Vec::new();
    for entry in entries {
        if skip_hidden && entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let file_type = entry.file_type().map_err(|source| CatalogIndexError::Io {
            action: "inspect catalog entry",
            path: entry.path(),
            source,
        })?;
        if file_type.is_dir() {
            directories.push(entry);
        }
    }
    directories.sort_by_key(fs::DirEntry::file_name);
    Ok(directories)
}

fn portable_entry_name(entry: &fs::DirEntry) -> Result<String, CatalogIndexError> {
    let value = entry
        .file_name()
        .into_string()
        .map_err(|_| CatalogIndexError::InvalidCatalogPath(entry.path()))?;
    PortableComponent::new(&value)
        .map_err(|_| CatalogIndexError::InvalidCatalogPath(entry.path()))?;
    Ok(value)
}

fn rebuild_path(index_path: &Path) -> Result<PathBuf, CatalogIndexError> {
    let filename = index_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| CatalogIndexError::InvalidIndexPath(index_path.to_path_buf()))?;
    Ok(index_path.with_file_name(format!("{filename}.rebuild")))
}

fn replace_index(rebuild_path: &Path, index_path: &Path) -> Result<(), CatalogIndexError> {
    fs::rename(rebuild_path, index_path).map_err(|source| CatalogIndexError::Io {
        action: "publish rebuilt catalog index",
        path: index_path.to_path_buf(),
        source,
    })?;
    sync_directory(index_parent(index_path)).map_err(|source| CatalogIndexError::Io {
        action: "persist rebuilt catalog index publication",
        path: index_parent(index_path).to_path_buf(),
        source,
    })
}

fn index_parent(index_path: &Path) -> &Path {
    index_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn remove_if_present(path: &Path) -> Result<(), CatalogIndexError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(CatalogIndexError::Io {
            action: "remove stale rebuilt catalog index",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn invalid_manifest(path: &Path, line: usize, reason: impl Into<String>) -> CatalogIndexError {
    CatalogIndexError::InvalidManifest {
        path: path.to_path_buf(),
        line: Some(line),
        reason: reason.into(),
    }
}

#[derive(Debug)]
pub enum CatalogIndexError {
    DeserializePath(serde_json::Error),
    InvalidCatalogPath(PathBuf),
    InvalidIndexPath(PathBuf),
    InvalidManifest {
        path: PathBuf,
        line: Option<usize>,
        reason: String,
    },
    Io {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    MissingIndexDirectory(PathBuf),
    Query(rusqlite::Error),
    SerializePath(serde_json::Error),
    Sqlite {
        path: PathBuf,
        source: rusqlite::Error,
    },
    UnsupportedSchemaVersion {
        path: PathBuf,
        version: i64,
    },
}

impl fmt::Display for CatalogIndexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeserializePath(source) => {
                write!(
                    formatter,
                    "catalog index contains an invalid path: {source}"
                )
            }
            Self::InvalidCatalogPath(path) => {
                write!(
                    formatter,
                    "catalog path is not portable: {}",
                    path.display()
                )
            }
            Self::InvalidIndexPath(path) => {
                write!(
                    formatter,
                    "catalog index path has no filename: {}",
                    path.display()
                )
            }
            Self::InvalidManifest { path, line, reason } => {
                if let Some(line) = line {
                    write!(
                        formatter,
                        "invalid catalog manifest {} at line {line}: {reason}",
                        path.display()
                    )
                } else {
                    write!(
                        formatter,
                        "invalid catalog manifest {}: {reason}",
                        path.display()
                    )
                }
            }
            Self::Io {
                action,
                path,
                source,
            } => write!(formatter, "{action} at {}: {source}", path.display()),
            Self::MissingIndexDirectory(path) => {
                write!(
                    formatter,
                    "catalog index directory does not exist: {}",
                    path.display()
                )
            }
            Self::Query(source) => write!(formatter, "query catalog index: {source}"),
            Self::SerializePath(source) => {
                write!(formatter, "serialize catalog index path: {source}")
            }
            Self::Sqlite { path, source } => {
                write!(formatter, "catalog index {}: {source}", path.display())
            }
            Self::UnsupportedSchemaVersion { path, version } => write!(
                formatter,
                "catalog index {} uses unsupported schema version {version}",
                path.display()
            ),
        }
    }
}

impl Error for CatalogIndexError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::DeserializePath(source) | Self::SerializePath(source) => Some(source),
            Self::Io { source, .. } => Some(source),
            Self::Query(source) => Some(source),
            Self::Sqlite { source, .. } => Some(source),
            Self::InvalidCatalogPath(_)
            | Self::InvalidIndexPath(_)
            | Self::InvalidManifest { .. }
            | Self::MissingIndexDirectory(_)
            | Self::UnsupportedSchemaVersion { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use souvenirs_domain::plan::{
        catalog_duplicate_findings, PlanFile, PlanFileRole, PlanItem, PlanItemDisposition,
    };

    #[test]
    fn rebuild_indexes_exact_digests_and_component_sorted_paths() {
        let temporary = tempfile::tempdir().unwrap();
        let catalog = temporary.path().join("catalog");
        let cache = temporary.path().join("cache");
        fs::create_dir_all(&cache).unwrap();
        write_collection(
            &catalog,
            "2025",
            "2025-01-01 First",
            &[("photo.JPG", "a".repeat(64)), ("video.mov", "b".repeat(64))],
        );
        write_collection(
            &catalog,
            "2024",
            "2024-12-31 Second",
            &[("raw.CR3", "a".repeat(64))],
        );

        let index = CatalogIndex::rebuild(cache.join("index.sqlite"), &catalog).unwrap();

        assert_eq!(
            index
                .paths_for_digest(&ContentDigest::parse("a".repeat(64)).unwrap())
                .unwrap(),
            vec![
                vec![
                    "2024".to_owned(),
                    "2024-12-31 Second".to_owned(),
                    "raw.CR3".to_owned()
                ],
                vec![
                    "2025".to_owned(),
                    "2025-01-01 First".to_owned(),
                    "photo.JPG".to_owned()
                ],
            ]
        );
        assert!(index
            .paths_for_digest(&ContentDigest::parse("c".repeat(64)).unwrap())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn rebuild_replaces_stale_rows_without_rehashing_originals() {
        let temporary = tempfile::tempdir().unwrap();
        let catalog = temporary.path().join("catalog");
        let cache = temporary.path().join("cache");
        fs::create_dir_all(&cache).unwrap();
        write_collection(
            &catalog,
            "2025",
            "2025-01-01 First",
            &[("photo.jpg", "a".repeat(64))],
        );
        let index_path = cache.join("index.sqlite");
        CatalogIndex::rebuild(&index_path, &catalog).unwrap();

        let collection = catalog.join("2025/2025-01-01 First");
        fs::write(collection.join("photo.jpg"), b"contents changed").unwrap();
        fs::write(
            collection.join("SHA256SUMS"),
            format!("{}  photo.jpg\n", "b".repeat(64)),
        )
        .unwrap();
        let index = CatalogIndex::rebuild(&index_path, &catalog).unwrap();

        assert!(index
            .paths_for_digest(&ContentDigest::parse("a".repeat(64)).unwrap())
            .unwrap()
            .is_empty());
        assert_eq!(
            index
                .paths_for_digest(&ContentDigest::parse("b".repeat(64)).unwrap())
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn failed_rebuild_preserves_the_published_index_and_removes_the_stage() {
        let temporary = tempfile::tempdir().unwrap();
        let catalog = temporary.path().join("catalog");
        let cache = temporary.path().join("cache");
        fs::create_dir_all(&cache).unwrap();
        write_collection(
            &catalog,
            "2025",
            "2025-01-01 First",
            &[("a.jpg", "a".repeat(64)), ("b.jpg", "b".repeat(64))],
        );
        let index_path = cache.join("index.sqlite");
        CatalogIndex::rebuild(&index_path, &catalog).unwrap();
        fs::write(
            catalog.join("2025/2025-01-01 First/SHA256SUMS"),
            format!("{}  b.jpg\n{}  a.jpg\n", "b".repeat(64), "a".repeat(64)),
        )
        .unwrap();

        assert!(matches!(
            CatalogIndex::rebuild(&index_path, &catalog),
            Err(CatalogIndexError::InvalidManifest { reason, .. })
                if reason.contains("strict byte order")
        ));
        assert!(!cache.join("index.sqlite.rebuild").exists());
        let index = CatalogIndex::open(&index_path).unwrap();
        assert_eq!(
            index
                .paths_for_digest(&ContentDigest::parse("a".repeat(64)).unwrap())
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn lookup_feeds_structured_plan_catalog_findings() {
        let temporary = tempfile::tempdir().unwrap();
        let catalog = temporary.path().join("catalog");
        let cache = temporary.path().join("cache");
        fs::create_dir_all(&cache).unwrap();
        write_collection(
            &catalog,
            "2025",
            "2025-01-01 First",
            &[("photo.JPG", "a".repeat(64))],
        );
        let index = CatalogIndex::rebuild(cache.join("index.sqlite"), &catalog).unwrap();
        let items = vec![
            plan_item("source-b", "b.mov", "b".repeat(64)),
            plan_item("source-a", "a.cr3", "a".repeat(64)),
        ];

        let findings =
            catalog_duplicate_findings(&items, |digest| index.paths_for_digest(digest)).unwrap();

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].source_file_id, "source-a");
        assert_eq!(
            findings[0].catalog_paths,
            [vec![
                "2025".to_owned(),
                "2025-01-01 First".to_owned(),
                "photo.JPG".to_owned()
            ]]
        );
    }

    #[test]
    fn bare_index_filename_uses_the_current_directory() {
        assert_eq!(index_parent(Path::new("index.sqlite")), Path::new("."));
    }

    fn plan_item(source_file_id: &str, filename: &str, digest: String) -> PlanItem {
        PlanItem {
            item_id: source_file_id.to_owned(),
            files: vec![PlanFile {
                source_file_id: source_file_id.to_owned(),
                source_path: vec![filename.to_owned()],
                digest: ContentDigest::parse(digest).unwrap(),
                size_bytes: 1,
                role: PlanFileRole::Primary,
                duplicate_disposition: None,
            }],
            accepted_datetime: None,
            datetime_disagreements: None,
            disposition: PlanItemDisposition::Excluded {
                reason: "test".to_owned(),
            },
        }
    }

    fn write_collection(root: &Path, year: &str, collection: &str, originals: &[(&str, String)]) {
        let path = root.join(year).join(collection);
        fs::create_dir_all(&path).unwrap();
        let mut manifest = String::new();
        for (filename, digest) in originals {
            fs::write(path.join(filename), b"not rehashed").unwrap();
            manifest.push_str(digest);
            manifest.push_str("  ");
            manifest.push_str(filename);
            manifest.push('\n');
        }
        fs::write(path.join("SHA256SUMS"), manifest).unwrap();
    }
}
