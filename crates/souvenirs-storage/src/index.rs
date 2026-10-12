use std::error::Error;
use std::fmt;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OpenFlags};
use souvenirs_domain::names::PortableComponent;
use souvenirs_domain::plan::{ContentDigest, ManifestFingerprint};

use crate::durability::{sync_directory, sync_file};
use crate::{read_manifest_entries, ManifestReadError, COLLECTION_MANIFEST_FILE_NAME};

const INDEX_SCHEMA_VERSION: i64 = 2;

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
        catalog_id: &str,
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
            let snapshot = read_catalog_snapshot(catalog_root)?;
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
                transaction
                    .execute(
                        "INSERT INTO catalog_metadata (catalog_id) VALUES (?1)",
                        [catalog_id],
                    )
                    .map_err(|source| CatalogIndexError::Sqlite {
                        path: rebuild_path.clone(),
                        source,
                    })?;
                let mut insert_manifest = transaction
                    .prepare("INSERT INTO catalog_manifests (path, digest) VALUES (?1, ?2)")
                    .map_err(|source| CatalogIndexError::Sqlite {
                        path: rebuild_path.clone(),
                        source,
                    })?;
                for manifest in &snapshot.manifests {
                    let path = serde_json::to_string(&manifest.path)
                        .map_err(CatalogIndexError::SerializePath)?;
                    insert_manifest
                        .execute(params![path, manifest.digest.as_str()])
                        .map_err(|source| CatalogIndexError::Sqlite {
                            path: rebuild_path.clone(),
                            source,
                        })?;
                }
                for original in snapshot.originals {
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
            Self::open(index_path, catalog_id)
        })();
        if result.is_err() {
            let _ = remove_if_present(&rebuild_path);
        }
        result
    }

    pub fn open(
        index_path: impl AsRef<Path>,
        expected_catalog_id: &str,
    ) -> Result<Self, CatalogIndexError> {
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
        let catalog_id = connection
            .query_row("SELECT catalog_id FROM catalog_metadata", [], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|source| CatalogIndexError::Sqlite {
                path: index_path.to_path_buf(),
                source,
            })?;
        if catalog_id != expected_catalog_id {
            return Err(CatalogIndexError::CatalogIdentityMismatch {
                path: index_path.to_path_buf(),
                expected: expected_catalog_id.to_owned(),
                actual: catalog_id,
            });
        }
        Ok(Self { connection })
    }

    pub fn manifest_snapshot(&self) -> Result<Vec<ManifestFingerprint>, CatalogIndexError> {
        let mut statement = self
            .connection
            .prepare("SELECT path, digest FROM catalog_manifests ORDER BY path")
            .map_err(CatalogIndexError::Query)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(CatalogIndexError::Query)?;
        rows.map(|row| {
            let (path, digest) = row.map_err(CatalogIndexError::Query)?;
            Ok(ManifestFingerprint {
                path: serde_json::from_str(&path).map_err(CatalogIndexError::DeserializePath)?,
                digest: ContentDigest::parse(digest)
                    .map_err(|_| CatalogIndexError::InvalidIndexDigest)?,
            })
        })
        .collect()
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
            PRAGMA user_version = 2;
            CREATE TABLE catalog_metadata (
                catalog_id TEXT NOT NULL
            );
            CREATE TABLE catalog_manifests (
                path TEXT NOT NULL PRIMARY KEY,
                digest TEXT NOT NULL
            );
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

struct CatalogSnapshot {
    manifests: Vec<ManifestFingerprint>,
    originals: Vec<IndexedOriginal>,
}

pub fn snapshot_catalog_manifests(
    root: impl AsRef<Path>,
) -> Result<Vec<ManifestFingerprint>, CatalogIndexError> {
    let root = root.as_ref();
    let mut manifests = Vec::new();
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
            let contents = fs::read(&manifest_path).map_err(|source| CatalogIndexError::Io {
                action: "read catalog manifest fingerprint",
                path: manifest_path,
                source,
            })?;
            manifests.push(ManifestFingerprint {
                path: vec![year_name.clone(), collection_name, "SHA256SUMS".to_owned()],
                digest: ContentDigest::from_bytes(&contents),
            });
        }
    }
    manifests.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(manifests)
}

fn read_catalog_snapshot(root: &Path) -> Result<CatalogSnapshot, CatalogIndexError> {
    let manifests = snapshot_catalog_manifests(root)?;
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
    Ok(CatalogSnapshot {
        manifests,
        originals,
    })
}

fn read_manifest(
    manifest_path: &Path,
    year: &str,
    collection: &str,
) -> Result<Vec<IndexedOriginal>, CatalogIndexError> {
    Ok(read_manifest_entries(manifest_path)
        .map_err(|error| match error {
            ManifestReadError::Invalid { path, line, reason } => {
                CatalogIndexError::InvalidManifest {
                    path,
                    line: Some(line),
                    reason,
                }
            }
            ManifestReadError::Io { path, source } => CatalogIndexError::Io {
                action: "read catalog manifest",
                path,
                source,
            },
            ManifestReadError::NotRegular { path } => CatalogIndexError::InvalidManifest {
                path,
                line: None,
                reason: format!("{COLLECTION_MANIFEST_FILE_NAME} is not a regular file"),
            },
        })?
        .into_iter()
        .map(|entry| IndexedOriginal {
            digest: entry.digest,
            path: vec![
                year.to_owned(),
                collection.to_owned(),
                entry.filename.as_str().to_owned(),
            ],
        })
        .collect())
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

#[derive(Debug)]
pub enum CatalogIndexError {
    CatalogIdentityMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    DeserializePath(serde_json::Error),
    InvalidIndexDigest,
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
            Self::CatalogIdentityMismatch {
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "catalog index {} belongs to catalog {actual}, expected {expected}",
                path.display()
            ),
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
            Self::InvalidIndexDigest => {
                formatter.write_str("catalog index contains an invalid manifest digest")
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
            Self::CatalogIdentityMismatch { .. }
            | Self::InvalidCatalogPath(_)
            | Self::InvalidIndexDigest
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

        let index =
            CatalogIndex::rebuild(cache.join("index.sqlite"), &catalog, "catalog-a").unwrap();

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
        CatalogIndex::rebuild(&index_path, &catalog, "catalog-a").unwrap();

        let collection = catalog.join("2025/2025-01-01 First");
        fs::write(collection.join("photo.jpg"), b"contents changed").unwrap();
        fs::write(
            collection.join("SHA256SUMS"),
            format!("{}  photo.jpg\n", "b".repeat(64)),
        )
        .unwrap();
        let index = CatalogIndex::rebuild(&index_path, &catalog, "catalog-a").unwrap();

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
        CatalogIndex::rebuild(&index_path, &catalog, "catalog-a").unwrap();
        fs::write(
            catalog.join("2025/2025-01-01 First/SHA256SUMS"),
            format!("{}  b.jpg\n{}  a.jpg\n", "b".repeat(64), "a".repeat(64)),
        )
        .unwrap();

        assert!(matches!(
            CatalogIndex::rebuild(&index_path, &catalog, "catalog-a"),
            Err(CatalogIndexError::InvalidManifest { reason, .. })
                if reason.contains("strict byte order")
        ));
        assert!(!cache.join("index.sqlite.rebuild").exists());
        let index = CatalogIndex::open(&index_path, "catalog-a").unwrap();
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
        let index =
            CatalogIndex::rebuild(cache.join("index.sqlite"), &catalog, "catalog-a").unwrap();
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

    #[test]
    fn index_is_bound_to_catalog_identity_and_complete_manifest_snapshot() {
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
        write_collection(&catalog, "2026", "2026-01-01 Empty", &[]);
        let index_path = cache.join("index.sqlite");

        let index = CatalogIndex::rebuild(&index_path, &catalog, "catalog-a").unwrap();

        assert!(matches!(
            CatalogIndex::open(&index_path, "catalog-b"),
            Err(CatalogIndexError::CatalogIdentityMismatch { .. })
        ));
        assert_eq!(
            index
                .manifest_snapshot()
                .unwrap()
                .into_iter()
                .map(|manifest| manifest.path)
                .collect::<Vec<_>>(),
            vec![
                vec![
                    "2025".to_owned(),
                    "2025-01-01 First".to_owned(),
                    "SHA256SUMS".to_owned(),
                ],
                vec![
                    "2026".to_owned(),
                    "2026-01-01 Empty".to_owned(),
                    "SHA256SUMS".to_owned(),
                ],
            ]
        );
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
