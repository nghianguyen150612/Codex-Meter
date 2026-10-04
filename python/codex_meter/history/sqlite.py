"""Strictly read-only access to the Rust-owned Observation SQLite database."""

from __future__ import annotations

import hashlib
import json
import math
import re
import sqlite3
from pathlib import Path
from typing import Any

from .models import (
    ConfigurationKey,
    EvidenceValidity,
    ObservationDataset,
    ObservationLifecycle,
    ObservationRecord,
    ObservationSelector,
    QualityGrade,
    QuotaEvidenceRecord,
    QuotaMeterType,
    ResetStatus,
)

SUPPORTED_MIGRATION_VERSION = 4
DEFAULT_BUSY_TIMEOUT_SECONDS = 5.0
MAXIMUM_OBSERVATIONS_SAFETY_LIMIT = 100_000

MIGRATIONS = (
    (
        1,
        "0001_storage_metadata",
        "1ffa336dcdc5abc63fdf74276c354c82a7b8f157af9412625723a7d8fe20c5aa",
    ),
    (
        2,
        "0002_runtime_checkpoints",
        "ae4a25ab39f865e7d1d5f02c9d03cc95a2786b72a48733025db1a0abe01d7199",
    ),
    (
        3,
        "0003_observations",
        "1ed907c9f124697b7f5620799672e49128dde7110860d513b0efaa9f57cd305c",
    ),
    (
        4,
        "0004_observation_time_keys",
        "97aaebf9ca9c856b42cd88089ee17f84cbe3010ad4e37246ecd7118417d14f6c",
    ),
)

_TIMESTAMP_PATTERN = re.compile(
    r"^(?P<base>\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})(?:\.(?P<fraction>\d{1,9}))?Z$"
)
_TERMINAL_LIFECYCLES = tuple(
    lifecycle.value
    for lifecycle in (
        ObservationLifecycle.FINALIZED,
        ObservationLifecycle.INCOMPLETE,
        ObservationLifecycle.INVALID,
    )
)
_CONFIGURATION_COLUMNS = (
    "plan",
    "model",
    "reasoning_level",
    "speed_mode",
    "codex_version",
)
_REQUIRED_OBSERVATION_COLUMNS = {
    "observation_id",
    "schema_version",
    "lifecycle_state",
    "started_at",
    "ended_at",
    "finalized_at",
    "summary_quality",
    "plan",
    "model",
    "reasoning_level",
    "speed_mode",
    "codex_version",
    "token_validity",
    "token_quality",
    "raw_total",
    "five_hour_validity",
    "five_hour_quality",
    "five_hour_delta",
    "five_hour_reset_status",
    "weekly_validity",
    "weekly_quality",
    "weekly_delta",
    "weekly_reset_status",
    "observation_json",
    "observation_sha256",
}


class AnalyticsDataError(Exception):
    """Base class for safe, fail-closed analytics data errors."""


class DatabaseNotFound(AnalyticsDataError):
    pass


class DatabaseOpenError(AnalyticsDataError):
    pass


class StorageCompatibilityError(AnalyticsDataError):
    pass


class MigrationDriftError(StorageCompatibilityError):
    pass


class ObservationCorruptError(AnalyticsDataError):
    pass


class QueryValidationError(AnalyticsDataError):
    pass


def canonical_time_key(timestamp: str) -> str:
    """Return the fixed-width UTC key used by migration 0004."""

    if not isinstance(timestamp, str):
        raise QueryValidationError("timestamp must be a string")
    match = _TIMESTAMP_PATTERN.fullmatch(timestamp)
    if match is None:
        raise QueryValidationError(
            "timestamp must be UTC RFC3339 with at most nine fractional digits"
        )
    base = match.group("base")
    fraction = (match.group("fraction") or "").ljust(9, "0")
    try:
        from datetime import datetime

        datetime.strptime(base, "%Y-%m-%dT%H:%M:%S")
    except ValueError as exc:
        raise QueryValidationError("timestamp is not a valid UTC date-time") from exc
    return f"{base}.{fraction}Z"


def _corrupt(
    observation_id: str | None = None,
    detail: str = "observation row is corrupt",
) -> ObservationCorruptError:
    suffix = f" ({observation_id})" if observation_id else ""
    return ObservationCorruptError(f"{detail}{suffix}")


class ObservationDatabase:
    """Owned read-only connection to a Rust-managed SQLite database."""

    def __init__(self, connection: sqlite3.Connection, migration_version: int) -> None:
        self._connection = connection
        self.migration_version = migration_version
        self._closed = False

    @classmethod
    def open(
        cls,
        path: str | Path,
        *,
        busy_timeout_seconds: float = DEFAULT_BUSY_TIMEOUT_SECONDS,
    ) -> ObservationDatabase:
        database_path = Path(path).expanduser()
        if not database_path.exists() or not database_path.is_file():
            raise DatabaseNotFound("analytics database does not exist")
        if not math.isfinite(busy_timeout_seconds) or busy_timeout_seconds <= 0:
            raise QueryValidationError("busy timeout must be finite and positive")
        try:
            uri = f"{database_path.resolve().as_uri()}?mode=ro"
            connection = sqlite3.connect(
                uri,
                uri=True,
                timeout=busy_timeout_seconds,
                isolation_level=None,
            )
            connection.row_factory = sqlite3.Row
        except (OSError, sqlite3.Error) as exc:
            raise DatabaseOpenError("could not open analytics database read-only") from exc

        try:
            connection.execute("PRAGMA query_only = ON")
            query_only = connection.execute("PRAGMA query_only").fetchone()[0]
            if query_only != 1:
                raise DatabaseOpenError("SQLite query-only mode could not be enabled")
            migration_version = _validate_storage(connection)
        except AnalyticsDataError:
            connection.close()
            raise
        except sqlite3.Error as exc:
            connection.close()
            raise DatabaseOpenError("could not validate analytics database") from exc
        return cls(connection, migration_version)

    @property
    def query_only(self) -> bool:
        self._ensure_open()
        return self._connection.execute("PRAGMA query_only").fetchone()[0] == 1

    def close(self) -> None:
        if not self._closed:
            self._connection.close()
            self._closed = True

    def __enter__(self) -> ObservationDatabase:
        self._ensure_open()
        return self

    def __exit__(self, exc_type: object, exc_value: object, traceback: object) -> None:
        self.close()

    def load_dataset(self, selector: ObservationSelector | None = None) -> ObservationDataset:
        self._ensure_open()
        selector = selector or ObservationSelector()
        params: list[Any] = []
        clauses = [
            "lifecycle_state IN ('finalized', 'incomplete', 'invalid')",
        ]
        if selector.configuration is not None:
            for column in _CONFIGURATION_COLUMNS:
                value = getattr(selector.configuration, column)
                if value is None:
                    clauses.append(f"{column} IS NULL")
                else:
                    clauses.append(f"{column} = ?")
                    params.append(value)

        observed_from = _canonical_selector_time(selector.observed_from)
        observed_through = _canonical_selector_time(selector.observed_through)
        if observed_from is not None or observed_through is not None:
            clauses.append("COALESCE(finalized_at, ended_at, started_at, '') <> ''")
        if observed_from is not None:
            clauses.append("COALESCE(finalized_at, ended_at, started_at, '') >= ?")
            params.append(observed_from)
        if observed_through is not None:
            clauses.append("COALESCE(finalized_at, ended_at, started_at, '') <= ?")
            params.append(observed_through)
        if (
            observed_from is not None
            and observed_through is not None
            and observed_from > observed_through
        ):
            raise QueryValidationError("observed_from must not be after observed_through")

        maximum = selector.maximum_observations
        if maximum is not None:
            if isinstance(maximum, bool) or not isinstance(maximum, int) or maximum < 1:
                raise QueryValidationError("maximum_observations must be at least one")
            if maximum > MAXIMUM_OBSERVATIONS_SAFETY_LIMIT:
                raise QueryValidationError("maximum_observations exceeds the reader safety limit")

        columns = (
            "observation_id, schema_version, lifecycle_state, started_at, ended_at, finalized_at, "
            "summary_quality, plan, model, reasoning_level, speed_mode, codex_version, "
            "token_validity, token_quality, raw_total, five_hour_validity, five_hour_quality, "
            "five_hour_delta, five_hour_reset_status, weekly_validity, weekly_quality, "
            "weekly_delta, weekly_reset_status, observation_json, observation_sha256"
        )
        order = "DESC, observation_id DESC" if maximum is not None else "ASC, observation_id ASC"
        sql = f"SELECT {columns} FROM observations WHERE {' AND '.join(clauses)} "
        sql += f"ORDER BY COALESCE(finalized_at, ended_at, started_at, '') {order}"
        if maximum is not None:
            sql += " LIMIT ?"
            params.append(maximum)

        rows: list[ObservationRecord] = []
        try:
            self._connection.execute("BEGIN")
            cursor = self._connection.execute(sql, params)
            while True:
                batch = cursor.fetchmany(256)
                if not batch:
                    break
                rows.extend(_record_from_row(row) for row in batch)
            self._connection.execute("COMMIT")
        except (AnalyticsDataError, sqlite3.Error) as exc:
            try:
                self._connection.execute("ROLLBACK")
            except sqlite3.Error:
                pass
            if isinstance(exc, AnalyticsDataError):
                raise
            raise DatabaseOpenError("could not read analytics observations") from exc

        rows.sort(key=lambda row: (_observation_time(row), row.observation_id))
        return ObservationDataset(
            observations=tuple(rows),
            migration_version=self.migration_version,
            selected_configuration=selector.configuration,
        )

    def load(self, selector: ObservationSelector | None = None) -> ObservationDataset:
        return self.load_dataset(selector)

    def _ensure_open(self) -> None:
        if self._closed:
            raise DatabaseOpenError("analytics database is closed")


def _canonical_selector_time(timestamp: str | None) -> str | None:
    if timestamp is None:
        return None
    try:
        return canonical_time_key(timestamp)
    except QueryValidationError:
        raise


def _validate_storage(connection: sqlite3.Connection) -> int:
    tables = {
        row[0]
        for row in connection.execute(
            "SELECT name FROM sqlite_master "
            "WHERE type = 'table' AND name IN ('schema_migrations', 'observations')"
        )
    }
    missing_tables = {"schema_migrations", "observations"} - tables
    if missing_tables:
        raise StorageCompatibilityError("required analytics storage tables are missing")

    try:
        migration_rows = connection.execute(
            "SELECT version, name, checksum_sha256 FROM schema_migrations ORDER BY version"
        ).fetchall()
    except sqlite3.Error as exc:
        raise StorageCompatibilityError("schema migration history is unreadable") from exc
    expected_by_version = {version: (name, checksum) for version, name, checksum in MIGRATIONS}
    if any(not isinstance(row[0], int) for row in migration_rows):
        raise MigrationDriftError("migration history contains an invalid version")
    if any(row[0] > SUPPORTED_MIGRATION_VERSION for row in migration_rows):
        raise StorageCompatibilityError("database migration is newer than the analytics reader")
    for index, row in enumerate(migration_rows, start=1):
        version, name, checksum = row
        expected = expected_by_version.get(index)
        if version != index or expected is None or name != expected[0] or checksum != expected[1]:
            raise MigrationDriftError(f"migration history drift at version {version}")
    latest = migration_rows[-1][0] if migration_rows else 0
    if latest != SUPPORTED_MIGRATION_VERSION:
        raise StorageCompatibilityError("database migration version is not supported")

    try:
        observation_columns = {
            row[1] for row in connection.execute("PRAGMA table_info('observations')")
        }
    except sqlite3.Error as exc:
        raise StorageCompatibilityError("Observation storage projection is unreadable") from exc
    if not _REQUIRED_OBSERVATION_COLUMNS <= observation_columns:
        raise StorageCompatibilityError("Observation storage projection is incomplete")
    return latest


def _record_from_row(row: sqlite3.Row) -> ObservationRecord:
    observation_id = row["observation_id"]
    if not isinstance(observation_id, str) or not observation_id:
        raise _corrupt(detail="observation ID is empty or invalid")
    if row["schema_version"] != "1.0.0":
        raise _corrupt(observation_id, "unsupported Observation schema version")
    payload = row["observation_json"]
    stored_checksum = row["observation_sha256"]
    if not isinstance(payload, str) or not isinstance(stored_checksum, str):
        raise _corrupt(observation_id)
    checksum = hashlib.sha256(payload.encode("utf-8")).hexdigest()
    if checksum != stored_checksum:
        raise _corrupt(observation_id, "Observation checksum mismatch")
    try:
        document = json.loads(payload)
    except json.JSONDecodeError as exc:
        raise _corrupt(observation_id, "Observation JSON is invalid") from exc
    _validate_json_projection(document, row, observation_id)

    lifecycle = _enum_value(
        ObservationLifecycle, row["lifecycle_state"], observation_id, "lifecycle"
    )
    summary_quality = _enum_value(
        QualityGrade, row["summary_quality"], observation_id, "summary quality"
    )
    token_validity = _enum_value(
        EvidenceValidity, row["token_validity"], observation_id, "token validity"
    )
    token_quality = _enum_value(QualityGrade, row["token_quality"], observation_id, "token quality")
    _validate_status(token_validity, token_quality, observation_id, "token")
    raw_total = row["raw_total"]
    if raw_total is not None and (
        isinstance(raw_total, bool) or not isinstance(raw_total, int) or raw_total < 0
    ):
        raise _corrupt(observation_id, "raw total is invalid")
    five_hour = _quota_from_row(row, QuotaMeterType.FIVE_HOUR, observation_id)
    weekly = _quota_from_row(row, QuotaMeterType.WEEKLY, observation_id)
    expected_summary = max(
        (token_quality, five_hour.quality, weekly.quality), key=lambda quality: quality.severity
    )
    if summary_quality is not expected_summary:
        raise _corrupt(observation_id, "summary quality is inconsistent")
    configuration = ConfigurationKey(*(row[column] for column in _CONFIGURATION_COLUMNS))
    for value in (
        configuration.plan,
        configuration.model,
        configuration.reasoning_level,
        configuration.speed_mode,
        configuration.codex_version,
    ):
        if value is not None and (not isinstance(value, str) or not value):
            raise _corrupt(observation_id, "configuration projection is invalid")
    started_at = _validate_sql_time(row["started_at"], observation_id)
    ended_at = _validate_sql_time(row["ended_at"], observation_id)
    finalized_at = _validate_sql_time(row["finalized_at"], observation_id)
    return ObservationRecord(
        observation_id=observation_id,
        lifecycle=lifecycle,
        started_at=started_at,
        ended_at=ended_at,
        finalized_at=finalized_at,
        configuration=configuration,
        summary_quality=summary_quality,
        token_validity=token_validity,
        token_quality=token_quality,
        raw_total=raw_total,
        five_hour=five_hour,
        weekly=weekly,
    )


def _enum_value(enum_type: type[Any], value: Any, observation_id: str, field: str) -> Any:
    try:
        return enum_type(value)
    except (TypeError, ValueError) as exc:
        raise _corrupt(observation_id, f"unknown {field}") from exc


def _validate_status(
    validity: EvidenceValidity,
    quality: QualityGrade,
    observation_id: str,
    field: str,
) -> None:
    if validity is EvidenceValidity.INVALID and quality is not QualityGrade.X:
        raise _corrupt(observation_id, f"{field} validity and quality are inconsistent")
    if validity is not EvidenceValidity.VALID and quality.severity < QualityGrade.D.severity:
        raise _corrupt(observation_id, f"{field} quality is too strong for non-valid evidence")
    if validity is EvidenceValidity.VALID and quality is QualityGrade.X:
        raise _corrupt(observation_id, f"{field} validity and quality are inconsistent")


def _quota_from_row(
    row: sqlite3.Row,
    meter_type: QuotaMeterType,
    observation_id: str,
) -> QuotaEvidenceRecord:
    prefix = meter_type.value
    validity = _enum_value(
        EvidenceValidity,
        row[f"{prefix}_validity"],
        observation_id,
        f"{prefix} validity",
    )
    quality = _enum_value(
        QualityGrade,
        row[f"{prefix}_quality"],
        observation_id,
        f"{prefix} quality",
    )
    reset = _enum_value(
        ResetStatus,
        row[f"{prefix}_reset_status"],
        observation_id,
        f"{prefix} reset status",
    )
    _validate_status(validity, quality, observation_id, prefix)
    delta = row[f"{prefix}_delta"]
    if delta is not None and (isinstance(delta, bool) or not isinstance(delta, (int, float))):
        raise _corrupt(observation_id, f"{prefix} delta is invalid")
    if delta is not None:
        delta = float(delta)
        if not math.isfinite(delta) or not 0 <= delta <= 100:
            raise _corrupt(observation_id, f"{prefix} delta is outside the permitted range")
    if validity is EvidenceValidity.VALID and delta is None:
        raise _corrupt(observation_id, f"valid {prefix} evidence has no delta")
    if validity is not EvidenceValidity.VALID and delta is not None:
        raise _corrupt(observation_id, f"non-valid {prefix} evidence has a delta")
    if reset is ResetStatus.DETECTED and (
        validity is not EvidenceValidity.INVALID
        or quality is not QualityGrade.X
        or delta is not None
    ):
        raise _corrupt(observation_id, f"{prefix} reset evidence is inconsistent")
    return QuotaEvidenceRecord(meter_type, validity, quality, delta, reset)


def _validate_json_projection(document: Any, row: sqlite3.Row, observation_id: str) -> None:
    if not isinstance(document, dict):
        raise _corrupt(observation_id, "Observation JSON is not an object")
    required = {
        "schema_version",
        "observation_id",
        "lifecycle_state",
        "timing",
        "configuration",
        "summary_quality",
        "token_evidence",
        "quota_evidence",
    }
    if not required <= document.keys():
        raise _corrupt(observation_id, "Observation JSON is missing required evidence")
    for field in ("schema_version", "observation_id", "lifecycle_state", "summary_quality"):
        if document[field] != row[field if field != "lifecycle_state" else "lifecycle_state"]:
            raise _corrupt(observation_id, f"{field} projection mismatch")

    timing = document["timing"]
    if not isinstance(timing, dict):
        raise _corrupt(observation_id, "timing JSON is invalid")
    for field in ("started_at", "ended_at", "finalized_at"):
        value = timing.get(field)
        if value is not None:
            try:
                value = canonical_time_key(value)
            except QueryValidationError as exc:
                raise _corrupt(observation_id, f"{field} timestamp is invalid") from exc
        if value != row[field]:
            raise _corrupt(observation_id, f"{field} projection mismatch")

    configuration = document["configuration"]
    if not isinstance(configuration, dict):
        raise _corrupt(observation_id, "configuration JSON is invalid")
    for field in _CONFIGURATION_COLUMNS:
        value = configuration.get(field)
        if not isinstance(value, dict):
            raise _corrupt(observation_id, f"{field} configuration JSON is invalid")
        availability = value.get("availability")
        sql_value = row[field]
        if availability == "available":
            json_value = value.get("value")
            if not isinstance(json_value, str) or not json_value or sql_value != json_value:
                raise _corrupt(observation_id, f"{field} configuration projection mismatch")
        elif availability == "unavailable":
            if sql_value is not None:
                raise _corrupt(observation_id, f"{field} unavailable projection mismatch")
        else:
            raise _corrupt(observation_id, f"{field} configuration availability is invalid")

    token_evidence = document["token_evidence"]
    if not isinstance(token_evidence, dict) or not isinstance(token_evidence.get("status"), dict):
        raise _corrupt(observation_id, "token evidence JSON is invalid")
    token_status = token_evidence["status"]
    if (
        token_status.get("validity") != row["token_validity"]
        or token_status.get("quality") != row["token_quality"]
    ):
        raise _corrupt(observation_id, "token status projection mismatch")
    counters = token_evidence.get("raw_token_counters")
    if not isinstance(counters, dict) or not isinstance(counters.get("raw_total"), dict):
        raise _corrupt(observation_id, "raw token counter JSON is invalid")
    raw_total = counters["raw_total"]
    if raw_total.get("availability") == "available":
        if raw_total.get("value") != row["raw_total"]:
            raise _corrupt(observation_id, "raw total projection mismatch")
    elif raw_total.get("availability") == "unavailable":
        if row["raw_total"] is not None:
            raise _corrupt(observation_id, "unavailable raw total projection mismatch")
    else:
        raise _corrupt(observation_id, "raw total availability is invalid")

    quota_evidence = document["quota_evidence"]
    if not isinstance(quota_evidence, dict):
        raise _corrupt(observation_id, "quota evidence JSON is invalid")
    for meter_type in QuotaMeterType:
        quota = quota_evidence.get(meter_type.value)
        if not isinstance(quota, dict) or not isinstance(quota.get("status"), dict):
            raise _corrupt(observation_id, f"{meter_type.value} evidence JSON is invalid")
        prefix = meter_type.value
        status = quota["status"]
        if (
            status.get("validity") != row[f"{prefix}_validity"]
            or status.get("quality") != row[f"{prefix}_quality"]
        ):
            raise _corrupt(observation_id, f"{prefix} status projection mismatch")
        if (
            quota.get("meter_type") != prefix
            or quota.get("reset_status") != row[f"{prefix}_reset_status"]
        ):
            raise _corrupt(observation_id, f"{prefix} identity projection mismatch")
        json_delta = quota.get("delta_percentage_points")
        if json_delta != row[f"{prefix}_delta"]:
            if not (json_delta is None and row[f"{prefix}_delta"] is None):
                raise _corrupt(observation_id, f"{prefix} delta projection mismatch")


def _validate_sql_time(value: Any, observation_id: str) -> str | None:
    if value is None:
        return None
    if not isinstance(value, str):
        raise _corrupt(observation_id, "SQL time projection is invalid")
    try:
        canonical = canonical_time_key(value)
    except QueryValidationError as exc:
        raise _corrupt(observation_id, "SQL time projection is invalid") from exc
    if canonical != value:
        raise _corrupt(observation_id, "SQL time projection is not canonical")
    return value


def _observation_time(record: ObservationRecord) -> str:
    return record.finalized_at or record.ended_at or record.started_at or ""


__all__ = [
    "AnalyticsDataError",
    "DatabaseNotFound",
    "DatabaseOpenError",
    "DEFAULT_BUSY_TIMEOUT_SECONDS",
    "MAXIMUM_OBSERVATIONS_SAFETY_LIMIT",
    "MigrationDriftError",
    "ObservationCorruptError",
    "ObservationDatabase",
    "QueryValidationError",
    "StorageCompatibilityError",
    "SUPPORTED_MIGRATION_VERSION",
    "canonical_time_key",
]
