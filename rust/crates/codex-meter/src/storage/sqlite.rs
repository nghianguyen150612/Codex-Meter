use std::error::Error;
use std::fmt;
use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;

use super::migrations::{migrate, validate_registry, Migration, MIGRATIONS};

pub const DEFAULT_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub enum StorageError {
    Open {
        source: rusqlite::Error,
    },
    ConnectionConfiguration {
        operation: &'static str,
        detail: &'static str,
        source: Option<rusqlite::Error>,
    },
    MigrationRegistryInvalid {
        detail: &'static str,
    },
    MigrationHistoryCorrupt {
        detail: &'static str,
        source: Option<rusqlite::Error>,
    },
    MigrationDrift {
        version: u32,
        expected_name: &'static str,
        actual_name: String,
        expected_checksum: String,
        actual_checksum: String,
    },
    DatabaseTooNew {
        database_version: u32,
        latest_supported_migration: u32,
    },
    MigrationFailed {
        version: u32,
        name: &'static str,
        source: rusqlite::Error,
    },
    Sqlite {
        source: rusqlite::Error,
    },
}

impl StorageError {
    pub(crate) fn sqlite(source: rusqlite::Error) -> Self {
        Self::Sqlite { source }
    }
}

impl fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open { .. } => write!(formatter, "could not open the SQLite database"),
            Self::ConnectionConfiguration {
                operation, detail, ..
            } => write!(formatter, "SQLite connection configuration failed during {operation}: {detail}"),
            Self::MigrationRegistryInvalid { detail } => {
                write!(formatter, "compiled migration registry is invalid: {detail}")
            }
            Self::MigrationHistoryCorrupt { detail, .. } => {
                write!(formatter, "SQLite migration history is corrupt: {detail}")
            }
            Self::MigrationDrift { version, .. } => {
                write!(formatter, "SQLite migration {version} differs from recorded history")
            }
            Self::DatabaseTooNew {
                database_version,
                latest_supported_migration,
            } => write!(
                formatter,
                "database migration version {database_version} is newer than supported version {latest_supported_migration}"
            ),
            Self::MigrationFailed { version, name, .. } => {
                write!(formatter, "SQLite migration {version} ({name}) failed")
            }
            Self::Sqlite { .. } => write!(formatter, "SQLite operation failed"),
        }
    }
}

impl Error for StorageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Open { source }
            | Self::MigrationFailed { source, .. }
            | Self::Sqlite { source } => Some(source),
            Self::ConnectionConfiguration { source, .. }
            | Self::MigrationHistoryCorrupt { source, .. } => {
                source.as_ref().map(|source| source as &dyn Error)
            }
            Self::MigrationRegistryInvalid { .. }
            | Self::MigrationDrift { .. }
            | Self::DatabaseTooNew { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StorageInfo {
    pub latest_supported_migration: u32,
    pub latest_applied_migration: u32,
}

#[derive(Debug)]
pub struct SqliteStore {
    connection: Connection,
    info: StorageInfo,
}

impl SqliteStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let path = path.as_ref();
        open_with_migrations(path, path == Path::new(":memory:"), MIGRATIONS)
    }

    pub fn open_in_memory() -> Result<Self, StorageError> {
        open_with_migrations(Path::new(":memory:"), true, MIGRATIONS)
    }

    pub fn info(&self) -> StorageInfo {
        self.info
    }

    pub fn foreign_keys_enabled(&self) -> Result<bool, StorageError> {
        let foreign_keys: i64 = self
            .connection
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .map_err(StorageError::sqlite)?;
        Ok(foreign_keys == 1)
    }

    pub fn journal_mode(&self) -> Result<String, StorageError> {
        self.connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .map_err(StorageError::sqlite)
    }

    #[cfg(test)]
    pub(crate) fn connection(&self) -> &Connection {
        &self.connection
    }
}

pub(crate) fn open_with_migrations(
    path: &Path,
    in_memory: bool,
    migrations: &[Migration],
) -> Result<SqliteStore, StorageError> {
    validate_registry(migrations)?;
    let mut connection = if in_memory {
        Connection::open_in_memory().map_err(|source| StorageError::Open { source })?
    } else {
        Connection::open(path).map_err(|source| StorageError::Open { source })?
    };

    configure_connection(&connection, in_memory)?;
    let info = migrate(&mut connection, migrations)?;
    Ok(SqliteStore { connection, info })
}

fn configure_connection(connection: &Connection, in_memory: bool) -> Result<(), StorageError> {
    connection
        .busy_timeout(DEFAULT_BUSY_TIMEOUT)
        .map_err(|source| StorageError::ConnectionConfiguration {
            operation: "busy timeout",
            detail: "could not set a finite busy timeout",
            source: Some(source),
        })?;
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|source| StorageError::ConnectionConfiguration {
            operation: "foreign keys",
            detail: "could not enable foreign-key enforcement",
            source: Some(source),
        })?;
    let foreign_keys: i64 = connection
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .map_err(|source| StorageError::ConnectionConfiguration {
            operation: "foreign keys",
            detail: "could not verify foreign-key enforcement",
            source: Some(source),
        })?;
    if foreign_keys != 1 {
        return Err(StorageError::ConnectionConfiguration {
            operation: "foreign keys",
            detail: "SQLite did not enable foreign-key enforcement",
            source: None,
        });
    }

    if !in_memory {
        let journal_mode: String = connection
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
            .map_err(|source| StorageError::ConnectionConfiguration {
                operation: "journal mode",
                detail: "could not configure WAL",
                source: Some(source),
            })?;
        if !journal_mode.eq_ignore_ascii_case("wal") {
            return Err(StorageError::ConnectionConfiguration {
                operation: "journal mode",
                detail: "SQLite did not activate WAL",
                source: None,
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    const MIGRATION_TWO_SQL: &str = "CREATE TABLE storage_test_two (value TEXT NOT NULL);";
    const FAILING_MIGRATION_TWO_SQL: &str =
        "CREATE TABLE storage_test_atomic (value TEXT NOT NULL); INVALID SQL;";
    const VALID_MIGRATION_TWO_SQL: &str = "CREATE TABLE storage_test_atomic (value TEXT NOT NULL);";

    const ONE_MIGRATION: &[Migration] = &[Migration {
        version: 1,
        name: "0001_storage_metadata",
        sql: super::super::migrations::STORAGE_METADATA_MIGRATION,
    }];
    const TWO_MIGRATIONS: &[Migration] = &[
        Migration {
            version: 1,
            name: "0001_storage_metadata",
            sql: super::super::migrations::STORAGE_METADATA_MIGRATION,
        },
        Migration {
            version: 2,
            name: "0002_storage_test",
            sql: MIGRATION_TWO_SQL,
        },
    ];
    const THREE_MIGRATIONS: &[Migration] = &[
        Migration {
            version: 1,
            name: "0001_storage_metadata",
            sql: super::super::migrations::STORAGE_METADATA_MIGRATION,
        },
        Migration {
            version: 2,
            name: "0002_storage_test",
            sql: MIGRATION_TWO_SQL,
        },
        Migration {
            version: 3,
            name: "0003_storage_test_three",
            sql: "CREATE TABLE storage_test_three (value TEXT NOT NULL);",
        },
    ];
    const FAILING_MIGRATIONS: &[Migration] = &[
        Migration {
            version: 1,
            name: "0001_storage_metadata",
            sql: super::super::migrations::STORAGE_METADATA_MIGRATION,
        },
        Migration {
            version: 2,
            name: "0002_storage_test_atomic",
            sql: FAILING_MIGRATION_TWO_SQL,
        },
    ];
    const VALID_MIGRATIONS: &[Migration] = &[
        Migration {
            version: 1,
            name: "0001_storage_metadata",
            sql: super::super::migrations::STORAGE_METADATA_MIGRATION,
        },
        Migration {
            version: 2,
            name: "0002_storage_test_atomic",
            sql: VALID_MIGRATION_TWO_SQL,
        },
    ];

    static NEXT_PATH: AtomicU64 = AtomicU64::new(0);

    fn database_path() -> std::path::PathBuf {
        let sequence = NEXT_PATH.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "codex-meter-storage-{}-{sequence}.sqlite",
            std::process::id()
        ))
    }

    fn remove_database(path: &std::path::Path) {
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite-shm"));
    }

    #[test]
    fn new_database_is_migrated_without_domain_tables() {
        let store = SqliteStore::open_in_memory().unwrap();
        assert_eq!(
            store.info(),
            StorageInfo {
                latest_supported_migration: 1,
                latest_applied_migration: 1,
            }
        );
        let tables = store
            .connection()
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(tables, vec!["schema_migrations", "storage_metadata"]);
        let migration_count: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(migration_count, 1);
    }

    #[test]
    fn reopen_is_idempotent_and_preserves_metadata() {
        let path = database_path();
        {
            let store = SqliteStore::open(&path).unwrap();
            store
                .connection()
                .execute(
                    "INSERT INTO storage_metadata (key, value) VALUES (?1, ?2)",
                    ("test-key", "test-value"),
                )
                .unwrap();
        }
        {
            let store = SqliteStore::open(&path).unwrap();
            let migration_count: i64 = store
                .connection()
                .query_row("SELECT count(*) FROM schema_migrations", [], |row| {
                    row.get(0)
                })
                .unwrap();
            let value: String = store
                .connection()
                .query_row(
                    "SELECT value FROM storage_metadata WHERE key = 'test-key'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(migration_count, 1);
            assert_eq!(value, "test-value");
        }
        remove_database(&path);
    }

    #[test]
    fn foreign_keys_are_enabled() {
        let store = SqliteStore::open_in_memory().unwrap();
        let foreign_keys: i64 = store
            .connection()
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(foreign_keys, 1);
    }

    #[test]
    fn explicit_memory_path_uses_in_memory_configuration() {
        let store = SqliteStore::open(":memory:").unwrap();
        let journal_mode: String = store
            .connection()
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_ne!(journal_mode.to_ascii_lowercase(), "wal");
    }

    #[test]
    fn file_backed_databases_use_wal() {
        let path = database_path();
        let store = SqliteStore::open(&path).unwrap();
        let journal_mode: String = store
            .connection()
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
        drop(store);
        remove_database(&path);
    }

    #[test]
    fn migration_drift_is_rejected() {
        let path = database_path();
        {
            let store = SqliteStore::open(&path).unwrap();
            store
                .connection()
                .execute(
                    "UPDATE schema_migrations SET checksum_sha256 = ?1 WHERE version = 1",
                    ["0".repeat(64)],
                )
                .unwrap();
        }
        let error = SqliteStore::open(&path).unwrap_err();
        assert!(matches!(
            error,
            StorageError::MigrationDrift { version: 1, .. }
        ));
        remove_database(&path);
    }

    #[test]
    fn migration_name_drift_is_rejected() {
        let path = database_path();
        {
            let store = SqliteStore::open(&path).unwrap();
            store
                .connection()
                .execute(
                    "UPDATE schema_migrations SET name = 'renamed' WHERE version = 1",
                    [],
                )
                .unwrap();
        }
        let error = SqliteStore::open(&path).unwrap_err();
        assert!(matches!(
            error,
            StorageError::MigrationDrift { version: 1, .. }
        ));
        remove_database(&path);
    }

    #[test]
    fn newer_database_is_rejected_without_mutation() {
        let path = database_path();
        {
            let store = SqliteStore::open(&path).unwrap();
            store
                .connection()
                .execute(
                    "INSERT INTO schema_migrations (version, name, checksum_sha256) VALUES (999, 'future', ?1)",
                    ["a".repeat(64)],
                )
                .unwrap();
        }
        let error = SqliteStore::open(&path).unwrap_err();
        assert!(matches!(
            error,
            StorageError::DatabaseTooNew {
                database_version: 999,
                latest_supported_migration: 1
            }
        ));
        let connection = Connection::open(&path).unwrap();
        let count: i64 = connection
            .query_row("SELECT count(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 2);
        drop(connection);
        remove_database(&path);
    }

    #[test]
    fn history_gap_is_rejected() {
        let path = database_path();
        {
            let store = open_with_migrations(&path, false, THREE_MIGRATIONS).unwrap();
            store
                .connection()
                .execute("DELETE FROM schema_migrations", [])
                .unwrap();
            store
                .connection()
                .execute(
                    "INSERT INTO schema_migrations (version, name, checksum_sha256) VALUES (1, '0001_storage_metadata', ?1)",
                    [super::super::migrations::checksum(
                        super::super::migrations::STORAGE_METADATA_MIGRATION,
                    )],
                )
                .unwrap();
            store
                .connection()
                .execute(
                    "INSERT INTO schema_migrations (version, name, checksum_sha256) VALUES (3, '0003_missing', ?1)",
                    [super::super::migrations::checksum(MIGRATION_TWO_SQL)],
                )
                .unwrap();
        }
        let error = open_with_migrations(&path, false, THREE_MIGRATIONS).unwrap_err();
        assert!(matches!(
            error,
            StorageError::MigrationHistoryCorrupt { .. }
        ));
        remove_database(&path);
    }

    #[test]
    fn failed_migration_rolls_back_schema_and_history_and_can_resume() {
        let path = database_path();
        let error = open_with_migrations(&path, false, FAILING_MIGRATIONS).unwrap_err();
        assert!(matches!(
            error,
            StorageError::MigrationFailed {
                version: 2,
                name: "0002_storage_test_atomic",
                ..
            }
        ));

        let connection = Connection::open(&path).unwrap();
        let history_count: i64 = connection
            .query_row("SELECT count(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        let atomic_table_count: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'storage_test_atomic'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(history_count, 1);
        assert_eq!(atomic_table_count, 0);
        drop(connection);

        let store = open_with_migrations(&path, false, VALID_MIGRATIONS).unwrap();
        assert_eq!(store.info().latest_applied_migration, 2);
        remove_database(&path);
    }

    #[test]
    fn pending_migration_applies_only_the_new_version() {
        let path = database_path();
        let first = open_with_migrations(&path, false, ONE_MIGRATION).unwrap();
        assert_eq!(first.info().latest_applied_migration, 1);
        drop(first);

        let second = open_with_migrations(&path, false, TWO_MIGRATIONS).unwrap();
        assert_eq!(second.info().latest_applied_migration, 2);
        let history_count: i64 = second
            .connection()
            .query_row("SELECT count(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        let table_count: i64 = second
            .connection()
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'storage_test_two'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(history_count, 2);
        assert_eq!(table_count, 1);
        remove_database(&path);
    }
}
