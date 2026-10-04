import hashlib
import json
import sqlite3
from pathlib import Path

import pytest

from codex_meter.history import (
    ConfigurationKey,
    DatabaseNotFound,
    EvidenceValidity,
    MigrationDriftError,
    ObservationCorruptError,
    ObservationDatabase,
    ObservationLifecycle,
    ObservationSelector,
    QualityGrade,
    QueryValidationError,
    QuotaMeterType,
    StorageCompatibilityError,
    canonical_time_key,
)
from codex_meter.history.sqlite import MIGRATIONS

ROOT = Path(__file__).parents[2]
MIGRATION_DIR = ROOT / "rust" / "crates" / "codex-meter" / "migrations"
FIXTURE = ROOT / "fixtures" / "contracts" / "v1" / "observation-finalized.json"


def _migration_sql(version: int, name: str) -> str:
    path = MIGRATION_DIR / f"{version:04d}_{name[5:]}.sql"
    return path.read_text()


def _create_database(path: Path, latest: int = 4) -> None:
    connection = sqlite3.connect(path)
    connection.executescript(
        """
        CREATE TABLE schema_migrations (
            version INTEGER NOT NULL PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            checksum_sha256 TEXT NOT NULL
        );
        """
    )
    for version, name, checksum in MIGRATIONS[:latest]:
        connection.executescript(_migration_sql(version, name))
        connection.execute(
            "INSERT INTO schema_migrations VALUES (?, ?, ?)",
            (version, name, checksum),
        )
    connection.commit()
    connection.close()


def _document(**changes: object) -> dict:
    document = json.loads(FIXTURE.read_text())
    for path, value in changes.items():
        target = document
        parts = path.split(".")
        for part in parts[:-1]:
            target = target[part]
        if value is _DELETE:
            del target[parts[-1]]
        else:
            target[parts[-1]] = value
    return document


class _Delete:
    pass


_DELETE = _Delete()


def _insert_document(connection: sqlite3.Connection, document: dict, **overrides: object) -> None:
    def value(path: str) -> object:
        target = document
        for part in path.split("."):
            target = target[part]
        return target

    def configuration(name: str) -> str | None:
        item = value(f"configuration.{name}")
        return item.get("value") if item["availability"] == "available" else None

    def delta(name: str) -> float | None:
        return value(f"quota_evidence.{name}").get("delta_percentage_points")

    def time_value(name: str) -> str | None:
        timestamp = value(f"timing.{name}") if name in value("timing") else None
        return canonical_time_key(timestamp) if timestamp is not None else None

    row = {
        "observation_id": value("observation_id"),
        "schema_version": value("schema_version"),
        "lifecycle_state": value("lifecycle_state"),
        "started_at": time_value("started_at"),
        "ended_at": time_value("ended_at"),
        "finalized_at": time_value("finalized_at"),
        "summary_quality": value("summary_quality"),
        "plan": configuration("plan"),
        "model": configuration("model"),
        "reasoning_level": configuration("reasoning_level"),
        "speed_mode": configuration("speed_mode"),
        "codex_version": configuration("codex_version"),
        "token_validity": value("token_evidence.status.validity"),
        "token_quality": value("token_evidence.status.quality"),
        "raw_total": value("token_evidence.raw_token_counters.raw_total").get("value"),
        "five_hour_validity": value("quota_evidence.five_hour.status.validity"),
        "five_hour_quality": value("quota_evidence.five_hour.status.quality"),
        "five_hour_delta": delta("five_hour"),
        "five_hour_reset_status": value("quota_evidence.five_hour.reset_status"),
        "weekly_validity": value("quota_evidence.weekly.status.validity"),
        "weekly_quality": value("quota_evidence.weekly.status.quality"),
        "weekly_delta": delta("weekly"),
        "weekly_reset_status": value("quota_evidence.weekly.reset_status"),
        "observation_json": json.dumps(document, separators=(",", ":")),
        "observation_sha256": "",
        "storage_revision": 1,
    }
    row["raw_total"] = (
        row["raw_total"]
        if value("token_evidence.raw_token_counters.raw_total")["availability"] == "available"
        else None
    )
    row["observation_sha256"] = hashlib.sha256(row["observation_json"].encode()).hexdigest()
    row.update(overrides)
    columns = ", ".join(row)
    placeholders = ", ".join("?" for _ in row)
    connection.execute(
        f"INSERT INTO observations ({columns}) VALUES ({placeholders})",
        tuple(row.values()),
    )


@pytest.fixture
def database_path(tmp_path: Path) -> Path:
    path = tmp_path / "analytics # history" / "данные.sqlite"
    path.parent.mkdir()
    _create_database(path)
    connection = sqlite3.connect(path)
    _insert_document(connection, json.loads(FIXTURE.read_text()))
    connection.commit()
    connection.close()
    return path


def test_open_is_read_only_and_query_only(database_path: Path) -> None:
    with ObservationDatabase.open(database_path) as database:
        assert database.query_only is True
        with pytest.raises(sqlite3.OperationalError, match="readonly|read-only"):
            database._connection.execute("DELETE FROM observations")


def test_missing_database_does_not_create_file(tmp_path: Path) -> None:
    path = tmp_path / "does-not-exist.sqlite"
    with pytest.raises(DatabaseNotFound):
        ObservationDatabase.open(path)
    assert not path.exists()


def test_v4_observation_loads_as_typed_record(database_path: Path) -> None:
    with ObservationDatabase.open(database_path) as database:
        dataset = database.load_dataset()
    record = dataset.observations[0]
    assert dataset.loaded_rows == 1
    assert record.lifecycle is ObservationLifecycle.FINALIZED
    assert record.configuration.plan == "plus"
    assert record.raw_total == 14500
    assert record.quota(QuotaMeterType.FIVE_HOUR).delta_percentage_points == 4.0
    assert record.quota(QuotaMeterType.WEEKLY).delta_percentage_points == 1.0


def test_old_database_is_rejected(tmp_path: Path) -> None:
    path = tmp_path / "old.sqlite"
    _create_database(path, latest=3)
    with pytest.raises(StorageCompatibilityError):
        ObservationDatabase.open(path)


def test_new_database_is_rejected(database_path: Path) -> None:
    connection = sqlite3.connect(database_path)
    connection.execute(
        "INSERT INTO schema_migrations VALUES (5, '0005_future', '" + "0" * 64 + "')"
    )
    connection.commit()
    connection.close()
    with pytest.raises(StorageCompatibilityError):
        ObservationDatabase.open(database_path)


def test_migration_checksum_drift_is_rejected(database_path: Path) -> None:
    connection = sqlite3.connect(database_path)
    connection.execute(
        "UPDATE schema_migrations SET checksum_sha256 = ? WHERE version = 4",
        ("0" * 64,),
    )
    connection.commit()
    connection.close()
    with pytest.raises(MigrationDriftError):
        ObservationDatabase.open(database_path)


def test_observation_checksum_corruption_is_rejected(database_path: Path) -> None:
    connection = sqlite3.connect(database_path)
    connection.execute("UPDATE observations SET observation_json = ?", ('{"corrupt":true}',))
    connection.commit()
    connection.close()
    with ObservationDatabase.open(database_path) as database:
        with pytest.raises(ObservationCorruptError):
            database.load_dataset()


def test_projection_corruption_is_rejected(database_path: Path) -> None:
    connection = sqlite3.connect(database_path)
    connection.execute("UPDATE observations SET model = 'other-model'")
    connection.commit()
    connection.close()
    with ObservationDatabase.open(database_path) as database:
        with pytest.raises(ObservationCorruptError):
            database.load_dataset()


def test_json_identity_mismatch_is_rejected(database_path: Path) -> None:
    connection = sqlite3.connect(database_path)
    document = _document(observation_id="obs-other")
    payload = json.dumps(document, separators=(",", ":"))
    connection.execute(
        "UPDATE observations SET observation_json = ?, observation_sha256 = ?",
        (payload, hashlib.sha256(payload.encode()).hexdigest()),
    )
    connection.commit()
    connection.close()
    with ObservationDatabase.open(database_path) as database:
        with pytest.raises(ObservationCorruptError):
            database.load_dataset()


def test_null_and_zero_quota_delta_are_distinct(tmp_path: Path) -> None:
    path = tmp_path / "null-zero.sqlite"
    _create_database(path)
    connection = sqlite3.connect(path)
    zero = _document(
        **{"observation_id": "obs-zero", "quota_evidence.five_hour.delta_percentage_points": 0}
    )
    _insert_document(connection, zero)
    unavailable = _document(
        **{
            "observation_id": "obs-null",
            "summary_quality": "D",
            "quota_evidence.five_hour.status.validity": "incomplete",
            "quota_evidence.five_hour.status.quality": "D",
            "quota_evidence.five_hour.reset_status": "unavailable",
            "quota_evidence.five_hour.delta_percentage_points": _DELETE,
        }
    )
    _insert_document(connection, unavailable)
    connection.commit()
    connection.close()
    with ObservationDatabase.open(path) as database:
        records = {record.observation_id: record for record in database.load_dataset().observations}
    assert records["obs-zero"].five_hour.delta_percentage_points == 0.0
    assert records["obs-null"].five_hour.delta_percentage_points is None


def test_reset_invalid_meter_evidence_is_retained(tmp_path: Path) -> None:
    path = tmp_path / "reset.sqlite"
    _create_database(path)
    connection = sqlite3.connect(path)
    document = json.loads(
        (ROOT / "fixtures" / "contracts" / "v1" / "observation-reset-crossing.json").read_text()
    )
    _insert_document(connection, document)
    connection.commit()
    connection.close()
    with ObservationDatabase.open(path) as database:
        record = database.load_dataset().observations[0]
    assert record.five_hour.validity is EvidenceValidity.INVALID
    assert record.five_hour.quality is QualityGrade.X
    assert record.five_hour.delta_percentage_points is None
    assert record.weekly.delta_percentage_points == 1.0


def test_exact_configuration_filter_preserves_missing_values(database_path: Path) -> None:
    connection = sqlite3.connect(database_path)
    missing_speed = _document(
        **{
            "observation_id": "obs-missing-speed",
            "configuration.speed_mode": _DELETE,
        }
    )
    missing_speed["configuration"]["speed_mode"] = {
        "availability": "unavailable",
        "provenance": "unavailable",
    }
    _insert_document(connection, missing_speed)
    connection.commit()
    connection.close()
    with ObservationDatabase.open(database_path) as database:
        exact = ConfigurationKey("plus", "gpt-5.6-sol", "high", None, None)
        fast = ConfigurationKey("plus", "gpt-5.6-sol", "high", "fast", None)
        exact_ids = [
            r.observation_id for r in database.load_dataset(ObservationSelector(exact)).observations
        ]
        assert exact_ids == ["obs-missing-speed"]
        assert database.load_dataset(ObservationSelector(fast)).observations == ()


def test_mixed_precision_times_order_chronologically(tmp_path: Path) -> None:
    path = tmp_path / "times.sqlite"
    _create_database(path)
    connection = sqlite3.connect(path)
    timestamps = (
        ("obs-later", "2026-10-01T02:00:00.123456789Z"),
        ("obs-earlier", "2026-10-01T02:00:00.1Z"),
    )
    for observation_id, timestamp in timestamps:
        document = _document(**{"observation_id": observation_id, "timing.started_at": timestamp})
        _insert_document(connection, document)
    connection.commit()
    connection.close()
    with ObservationDatabase.open(path) as database:
        assert [r.observation_id for r in database.load_dataset().observations] == [
            "obs-earlier",
            "obs-later",
        ]


def test_recent_limit_is_stable_and_chronological(tmp_path: Path) -> None:
    path = tmp_path / "recent.sqlite"
    _create_database(path)
    connection = sqlite3.connect(path)
    for index, timestamp in enumerate(("02:00:00", "03:00:00", "04:00:00", "05:00:00")):
        document = _document(
            **{
                "observation_id": f"obs-{index}",
                "timing.started_at": f"2026-10-01T{timestamp}Z",
                "timing.ended_at": f"2026-10-01T{timestamp}Z",
                "timing.finalized_at": f"2026-10-01T{timestamp}Z",
                "timing.duration_ms": 0,
            }
        )
        _insert_document(connection, document)
    connection.commit()
    connection.close()
    with ObservationDatabase.open(path) as database:
        records = database.load_dataset(ObservationSelector(maximum_observations=2)).observations
    assert [record.observation_id for record in records] == ["obs-2", "obs-3"]


def test_terminal_loader_excludes_provisional_rows(database_path: Path) -> None:
    connection = sqlite3.connect(database_path)
    for lifecycle in ("reconciling", "active"):
        document = _document(**{"observation_id": f"obs-{lifecycle}", "lifecycle_state": lifecycle})
        document["timing"].pop("finalized_at")
        _insert_document(
            connection,
            document,
            finalized_at=None,
        )
    connection.commit()
    connection.close()
    with ObservationDatabase.open(database_path) as database:
        ids = [record.observation_id for record in database.load_dataset().observations]
        assert ids == ["obs-001"]


def test_time_range_is_inclusive(database_path: Path) -> None:
    selector = ObservationSelector(
        observed_from="2026-10-01T02:20:00Z",
        observed_through="2026-10-01T02:20:00.000000000Z",
    )
    with ObservationDatabase.open(database_path) as database:
        assert database.load_dataset(selector).loaded_rows == 1


def test_time_range_requires_valid_order(database_path: Path) -> None:
    with ObservationDatabase.open(database_path) as database:
        with pytest.raises(QueryValidationError):
            database.load_dataset(
                ObservationSelector(
                    observed_from="2026-10-02T00:00:00Z",
                    observed_through="2026-10-01T00:00:00Z",
                )
            )


def test_canonical_time_key_rejects_non_utc_and_preserves_nanos() -> None:
    assert canonical_time_key("2026-10-01T02:00:00.1Z") == "2026-10-01T02:00:00.100000000Z"
    assert canonical_time_key("2026-10-01T02:00:00Z") == "2026-10-01T02:00:00.000000000Z"
    with pytest.raises(QueryValidationError):
        canonical_time_key("2026-10-01T04:00:00+02:00")
