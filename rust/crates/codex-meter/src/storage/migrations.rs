use std::collections::{BTreeSet, HashMap};
use std::fmt;

use rusqlite::{params, Connection, Row, Transaction};
use sha2::{Digest, Sha256};

use super::sqlite::{StorageError, StorageInfo};

const HISTORY_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER NOT NULL PRIMARY KEY CHECK (version > 0),
    name TEXT NOT NULL UNIQUE CHECK (length(name) > 0),
    checksum_sha256 TEXT NOT NULL CHECK (
        length(checksum_sha256) = 64
        AND checksum_sha256 NOT GLOB '*[^0-9a-f]*'
    )
);"#;

pub(crate) const STORAGE_METADATA_MIGRATION: &str =
    include_str!("../../migrations/0001_storage_metadata.sql");
pub(crate) const RUNTIME_CHECKPOINTS_MIGRATION: &str =
    include_str!("../../migrations/0002_runtime_checkpoints.sql");
pub(crate) const OBSERVATIONS_MIGRATION: &str =
    include_str!("../../migrations/0003_observations.sql");
pub(crate) const OBSERVATION_TIME_KEYS_MIGRATION: &str =
    include_str!("../../migrations/0004_observation_time_keys.sql");

pub(crate) const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "0001_storage_metadata",
        sql: STORAGE_METADATA_MIGRATION,
    },
    Migration {
        version: 2,
        name: "0002_runtime_checkpoints",
        sql: RUNTIME_CHECKPOINTS_MIGRATION,
    },
    Migration {
        version: 3,
        name: "0003_observations",
        sql: OBSERVATIONS_MIGRATION,
    },
    Migration {
        version: 4,
        name: "0004_observation_time_keys",
        sql: OBSERVATION_TIME_KEYS_MIGRATION,
    },
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Migration {
    pub(crate) version: u32,
    pub(crate) name: &'static str,
    pub(crate) sql: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AppliedMigration {
    version: u32,
    name: String,
    checksum_sha256: String,
}

impl fmt::Display for AppliedMigration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "version {} ({})", self.version, self.name)
    }
}

pub(crate) fn validate_registry(migrations: &[Migration]) -> Result<(), StorageError> {
    if migrations.is_empty() {
        return Err(StorageError::MigrationRegistryInvalid {
            detail: "migration registry must not be empty",
        });
    }

    let mut versions = BTreeSet::new();
    let mut names = BTreeSet::new();
    for (index, migration) in migrations.iter().enumerate() {
        let expected_version = index as u32 + 1;
        if migration.version == 0 {
            return Err(StorageError::MigrationRegistryInvalid {
                detail: "migration versions must be positive",
            });
        }
        if migration.version != expected_version {
            return Err(StorageError::MigrationRegistryInvalid {
                detail: "migration versions must be contiguous starting at 1",
            });
        }
        if !versions.insert(migration.version) {
            return Err(StorageError::MigrationRegistryInvalid {
                detail: "migration versions must be unique",
            });
        }
        if migration.name.is_empty() {
            return Err(StorageError::MigrationRegistryInvalid {
                detail: "migration names must not be empty",
            });
        }
        if !names.insert(migration.name) {
            return Err(StorageError::MigrationRegistryInvalid {
                detail: "migration names must be unique",
            });
        }
    }

    Ok(())
}

pub(crate) fn checksum(sql: &str) -> String {
    let digest = Sha256::digest(sql.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn migrate(
    connection: &mut Connection,
    migrations: &[Migration],
) -> Result<StorageInfo, StorageError> {
    validate_registry(migrations)?;

    let Some(latest_migration) = migrations.last() else {
        return Err(StorageError::MigrationRegistryInvalid {
            detail: "migration registry must not be empty",
        });
    };
    let latest_supported_migration = latest_migration.version;

    loop {
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(StorageError::sqlite)?;

        if let Err(source) = transaction.execute_batch(HISTORY_SCHEMA) {
            return Err(StorageError::sqlite(source));
        }

        let history = read_history(&transaction)?;
        let latest_applied_migration = validate_history(&history, migrations)?;

        let Some(migration) = migrations.iter().find(|migration| {
            !history.iter().any(|applied| {
                applied.version == migration.version
                    && applied.name == migration.name
                    && applied.checksum_sha256 == checksum(migration.sql)
            })
        }) else {
            transaction.commit().map_err(StorageError::sqlite)?;
            return Ok(StorageInfo {
                latest_supported_migration,
                latest_applied_migration,
            });
        };

        if migration.version != latest_applied_migration + 1 {
            return Err(StorageError::MigrationHistoryCorrupt {
                detail: "migration history does not identify the next contiguous migration",
                source: None,
            });
        }

        if let Err(source) = transaction.execute_batch(migration.sql) {
            return Err(StorageError::MigrationFailed {
                version: migration.version,
                name: migration.name,
                source,
            });
        }

        if let Err(source) = transaction.execute(
            "INSERT INTO schema_migrations (version, name, checksum_sha256) VALUES (?1, ?2, ?3)",
            params![migration.version, migration.name, checksum(migration.sql)],
        ) {
            return Err(StorageError::MigrationFailed {
                version: migration.version,
                name: migration.name,
                source,
            });
        }

        if let Err(source) = transaction.commit() {
            return Err(StorageError::MigrationFailed {
                version: migration.version,
                name: migration.name,
                source,
            });
        }
    }
}

fn read_history(transaction: &Transaction<'_>) -> Result<Vec<AppliedMigration>, StorageError> {
    let mut statement = transaction
        .prepare("SELECT version, name, checksum_sha256 FROM schema_migrations ORDER BY version")
        .map_err(|source| StorageError::MigrationHistoryCorrupt {
            detail: "schema_migrations has an unreadable shape",
            source: Some(source),
        })?;

    let rows = statement
        .query_map([], migration_from_row)
        .map_err(|source| StorageError::MigrationHistoryCorrupt {
            detail: "schema_migrations rows could not be read",
            source: Some(source),
        })?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|source| StorageError::MigrationHistoryCorrupt {
            detail: "schema_migrations contains an unreadable row",
            source: Some(source),
        })
}

fn migration_from_row(row: &Row<'_>) -> rusqlite::Result<AppliedMigration> {
    Ok(AppliedMigration {
        version: row.get(0)?,
        name: row.get(1)?,
        checksum_sha256: row.get(2)?,
    })
}

fn validate_history(
    history: &[AppliedMigration],
    migrations: &[Migration],
) -> Result<u32, StorageError> {
    let Some(latest_migration) = migrations.last() else {
        return Err(StorageError::MigrationRegistryInvalid {
            detail: "migration registry must not be empty",
        });
    };
    let latest_supported_migration = latest_migration.version;
    let compiled_by_version: HashMap<u32, &Migration> = migrations
        .iter()
        .map(|migration| (migration.version, migration))
        .collect();
    let mut names = BTreeSet::new();

    for (index, applied) in history.iter().enumerate() {
        let expected_version = index as u32 + 1;
        if applied.version > latest_supported_migration {
            return Err(StorageError::DatabaseTooNew {
                database_version: applied.version,
                latest_supported_migration,
            });
        }
        if applied.version == 0 || applied.version != expected_version {
            return Err(StorageError::MigrationHistoryCorrupt {
                detail: "migration history versions must be contiguous starting at 1",
                source: None,
            });
        }
        if !names.insert(applied.name.as_str()) {
            return Err(StorageError::MigrationHistoryCorrupt {
                detail: "migration history names must be unique",
                source: None,
            });
        }

        let migration = compiled_by_version.get(&applied.version).ok_or(
            StorageError::MigrationHistoryCorrupt {
                detail: "migration history references an unknown migration",
                source: None,
            },
        )?;
        let expected_checksum = checksum(migration.sql);
        if applied.name != migration.name || applied.checksum_sha256 != expected_checksum {
            return Err(StorageError::MigrationDrift {
                version: applied.version,
                expected_name: migration.name,
                actual_name: applied.name.clone(),
                expected_checksum,
                actual_checksum: applied.checksum_sha256.clone(),
            });
        }
    }

    Ok(history.last().map_or(0, |migration| migration.version))
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_SQL: &str = "CREATE TABLE test_one (value TEXT NOT NULL);";

    #[test]
    fn registry_rejects_empty_registry() {
        assert!(matches!(
            validate_registry(&[]),
            Err(StorageError::MigrationRegistryInvalid { .. })
        ));
    }

    #[test]
    fn registry_rejects_zero_version() {
        assert!(matches!(
            validate_registry(&[Migration {
                version: 0,
                name: "zero",
                sql: VALID_SQL,
            }]),
            Err(StorageError::MigrationRegistryInvalid { .. })
        ));
    }

    #[test]
    fn registry_rejects_duplicate_version() {
        assert!(matches!(
            validate_registry(&[
                Migration {
                    version: 1,
                    name: "one",
                    sql: VALID_SQL,
                },
                Migration {
                    version: 1,
                    name: "two",
                    sql: VALID_SQL,
                },
            ]),
            Err(StorageError::MigrationRegistryInvalid { .. })
        ));
    }

    #[test]
    fn registry_rejects_duplicate_name() {
        assert!(matches!(
            validate_registry(&[
                Migration {
                    version: 1,
                    name: "same",
                    sql: VALID_SQL,
                },
                Migration {
                    version: 2,
                    name: "same",
                    sql: VALID_SQL,
                },
            ]),
            Err(StorageError::MigrationRegistryInvalid { .. })
        ));
    }

    #[test]
    fn registry_rejects_gap() {
        assert!(matches!(
            validate_registry(&[
                Migration {
                    version: 1,
                    name: "one",
                    sql: VALID_SQL,
                },
                Migration {
                    version: 3,
                    name: "three",
                    sql: VALID_SQL,
                },
            ]),
            Err(StorageError::MigrationRegistryInvalid { .. })
        ));
    }

    #[test]
    fn migration_one_checksum_is_regression_stable() {
        assert_eq!(
            checksum(STORAGE_METADATA_MIGRATION),
            "1ffa336dcdc5abc63fdf74276c354c82a7b8f157af9412625723a7d8fe20c5aa"
        );
    }

    #[test]
    fn migration_two_checksum_is_regression_stable() {
        assert_eq!(
            checksum(RUNTIME_CHECKPOINTS_MIGRATION),
            "ae4a25ab39f865e7d1d5f02c9d03cc95a2786b72a48733025db1a0abe01d7199"
        );
    }

    #[test]
    fn migration_three_checksum_is_regression_stable() {
        assert_eq!(
            checksum(OBSERVATIONS_MIGRATION),
            "1ed907c9f124697b7f5620799672e49128dde7110860d513b0efaa9f57cd305c"
        );
    }

    #[test]
    fn migration_four_checksum_is_regression_stable() {
        assert_eq!(
            checksum(OBSERVATION_TIME_KEYS_MIGRATION),
            "97aaebf9ca9c856b42cd88089ee17f84cbe3010ad4e37246ecd7118417d14f6c"
        );
    }
}
