"""Tests for the strict local Rust/Python analytics boundary."""

import hashlib
import json
import sqlite3
from pathlib import Path

import pytest
from jsonschema import Draft202012Validator, FormatChecker
from referencing import Registry, Resource

from codex_meter.history import canonical_time_key
from codex_meter.history.sqlite import MIGRATIONS
from codex_meter.integration.worker import (
    LocalExecutionContext,
    WorkerError,
    execute_request,
    parse_request,
)

ROOT = Path(__file__).parents[2]
MIGRATION_DIR = ROOT / "rust" / "crates" / "codex-meter" / "migrations"
OBSERVATION_FIXTURE = ROOT / "fixtures" / "contracts" / "v1" / "observation-finalized.json"
GENERATED_AT = "2026-10-16T07:00:00Z"


def _request(*, targets: list[str] | None = None, options: dict | None = None) -> dict:
    return {
        "schema_version": "1.0.0",
        "request_id": "worker-test-1",
        "analysis_type": "current_capacity_estimate",
        "dataset_selector": {
            "configuration": {
                "plan": {"availability": "available", "value": "plus", "provenance": "observed"},
                "model": {
                    "availability": "available",
                    "value": "gpt-5.6-sol",
                    "provenance": "observed",
                },
                "reasoning_level": {
                    "availability": "available",
                    "value": "high",
                    "provenance": "observed",
                },
                "speed_mode": {
                    "availability": "available",
                    "value": "standard",
                    "provenance": "derived",
                },
                "codex_version": {"availability": "unavailable", "provenance": "unavailable"},
            },
            "observed_from": "2026-10-01T00:00:00Z",
            "observed_through": "2026-10-02T00:00:00Z",
        },
        "target_quota_windows": targets if targets is not None else ["five_hour"],
        "options": options or {},
    }


def _create_database(path: Path) -> None:
    connection = sqlite3.connect(path)
    connection.execute(
        "CREATE TABLE schema_migrations (version INTEGER NOT NULL PRIMARY KEY, "
        "name TEXT NOT NULL UNIQUE, checksum_sha256 TEXT NOT NULL)"
    )
    for version, name, checksum in MIGRATIONS:
        migration_path = MIGRATION_DIR / f"{version:04d}_{name[5:]}.sql"
        connection.executescript(migration_path.read_text(encoding="utf-8"))
        connection.execute(
            "INSERT INTO schema_migrations VALUES (?, ?, ?)", (version, name, checksum)
        )
    document = json.loads(OBSERVATION_FIXTURE.read_text(encoding="utf-8"))
    for index in range(2):
        row_document = json.loads(json.dumps(document))
        row_document["observation_id"] = f"worker-observation-{index}"
        row_document["task_id"] = f"worker-task-{index}"
        row_document["session_id"] = f"worker-session-{index}"
        hour = 2 + index
        row_document["timing"]["started_at"] = f"2026-10-01T{hour:02}:00:00Z"
        row_document["timing"]["ended_at"] = f"2026-10-01T{hour:02}:15:00Z"
        row_document["timing"]["finalized_at"] = f"2026-10-01T{hour:02}:20:00Z"
        payload = json.dumps(row_document, separators=(",", ":"))
        configuration = row_document["configuration"]
        values = {
            "observation_id": row_document["observation_id"],
            "schema_version": row_document["schema_version"],
            "lifecycle_state": row_document["lifecycle_state"],
            "started_at": canonical_time_key(row_document["timing"]["started_at"]),
            "ended_at": canonical_time_key(row_document["timing"]["ended_at"]),
            "finalized_at": canonical_time_key(row_document["timing"]["finalized_at"]),
            "summary_quality": row_document["summary_quality"],
            "plan": configuration["plan"]["value"],
            "model": configuration["model"]["value"],
            "reasoning_level": configuration["reasoning_level"]["value"],
            "speed_mode": configuration["speed_mode"]["value"],
            "codex_version": None,
            "token_validity": "valid",
            "token_quality": "B",
            "raw_total": 14500,
            "five_hour_validity": "valid",
            "five_hour_quality": "B",
            "five_hour_delta": 4.0,
            "five_hour_reset_status": "not_detected",
            "weekly_validity": "valid",
            "weekly_quality": "B",
            "weekly_delta": 1.0,
            "weekly_reset_status": "not_detected",
            "observation_json": payload,
            "observation_sha256": hashlib.sha256(payload.encode("utf-8")).hexdigest(),
            "storage_revision": 1,
        }
        columns = ", ".join(values)
        placeholders = ", ".join("?" for _ in values)
        connection.execute(
            f"INSERT INTO observations ({columns}) VALUES ({placeholders})", tuple(values.values())
        )
    connection.commit()
    connection.close()


def _context(path: Path, *, five_hour: float | None = 24.0) -> LocalExecutionContext:
    return LocalExecutionContext(
        database_path=path,
        generated_at=GENERATED_AT,
        five_hour_used_percent=five_hour,
        weekly_used_percent=None,
    )


def test_request_parser_is_closed_and_rejects_unimplemented_policies() -> None:
    raw = json.dumps(_request()).encode()
    parsed, identity = parse_request(raw)
    assert parsed["request_id"] == "worker-test-1"
    assert identity.speed_mode.provenance.value == "derived"
    assert identity.codex_version.available is False

    for document, code in [
        ({**_request(), "secret": "no"}, "invalid_request"),
        ({**_request(), "analysis_type": "history_summary"}, "unsupported_analysis_type"),
        ({**_request(options={"minimum_quality": "C"})}, "unsupported_estimator_option"),
        ({**_request(), "schema_version": "2.0.0"}, "invalid_request"),
        (
            {
                **_request(),
                "dataset_selector": {**_request()["dataset_selector"], "observed_from": None},
            },
            "invalid_request",
        ),
        ({**_request(options={"maximum_observations": None})}, "invalid_request"),
    ]:
        with pytest.raises(WorkerError) as error:
            parse_request(json.dumps(document).encode())
        assert error.value.code == code

    with pytest.raises(WorkerError, match="duplicate JSON"):
        parse_request(b'{"schema_version":"1.0.0","schema_version":"1.0.0"}')


def test_real_p017_to_p020_pipeline_is_read_only_and_deterministic(tmp_path: Path) -> None:
    database = tmp_path / "synthetic.sqlite"
    _create_database(database)
    before = hashlib.sha256(database.read_bytes()).hexdigest()
    request = json.dumps(_request()).encode()
    first = execute_request(request, _context(database))
    replay = execute_request(request, _context(database))
    assert first == replay
    result = json.loads(first)
    assert result["result_status"] == "succeeded"
    assert result["generated_at"] == GENERATED_AT
    assert result["dataset_configuration"]["speed_mode"]["provenance"] == "derived"
    assert result["quota_capacity_estimates"][0]["sample_accounting"]["used_observations"] == 2
    assert hashlib.sha256(database.read_bytes()).hexdigest() == before

    schema_dir = ROOT / "schemas" / "v1"
    schema = json.loads((schema_dir / "analytics-result.schema.json").read_text(encoding="utf-8"))
    common = json.loads((schema_dir / "common.schema.json").read_text(encoding="utf-8"))
    base_uri = schema_dir.resolve().as_uri().rstrip("/") + "/"
    registry = Registry().with_resource(
        base_uri + "common.schema.json", Resource.from_contents(common)
    )
    validator = Draft202012Validator(
        {"$id": base_uri + "analytics-result.schema.json", **schema},
        registry=registry,
        format_checker=FormatChecker(),
    )
    validator.validate(result)


def test_zero_selected_observations_is_insufficient_without_a_position(tmp_path: Path) -> None:
    database = tmp_path / "synthetic.sqlite"
    _create_database(database)
    request = _request()
    request["dataset_selector"]["observed_from"] = "2026-10-02T00:00:00Z"
    result = json.loads(
        execute_request(json.dumps(request).encode(), _context(database, five_hour=None))
    )
    assert result["result_status"] == "insufficient_evidence"
    estimate = result["quota_capacity_estimates"][0]
    assert estimate["confidence"] == "insufficient"
    assert "estimated_full_capacity_raw_tokens" not in estimate


def test_sufficient_history_requires_manual_position_and_observation_limit_is_applied(
    tmp_path: Path,
) -> None:
    database = tmp_path / "synthetic.sqlite"
    _create_database(database)
    request = _request()
    with pytest.raises(WorkerError) as missing_position:
        execute_request(json.dumps(request).encode(), _context(database, five_hour=None))
    assert missing_position.value.code == "current_quota_position_required"

    request["options"]["maximum_observations"] = 1
    result = json.loads(
        execute_request(
            json.dumps(request).encode(),
            _context(database, five_hour=None),
        )
    )
    assert result["result_status"] == "insufficient_evidence"
    assert result["quota_capacity_estimates"][0]["sample_accounting"]["candidate_observations"] == 1


def test_history_timestamp_bounds_compare_chronologically() -> None:
    request = _request()
    request["dataset_selector"]["observed_from"] = "2026-10-01T00:00:00Z"
    request["dataset_selector"]["observed_through"] = "2026-10-01T00:00:00.1Z"
    assert parse_request(json.dumps(request).encode())[0]["request_id"] == "worker-test-1"
    request["dataset_selector"]["observed_from"] = "2026-10-01T00:00:00.1Z"
    request["dataset_selector"]["observed_through"] = "2026-10-01T00:00:00Z"
    with pytest.raises(WorkerError, match="must not be after"):
        parse_request(json.dumps(request).encode())


@pytest.mark.parametrize(
    ("targets", "five_hour", "weekly"),
    [
        (["five_hour"], 24.0, None),
        (["weekly"], None, 12.5),
        (["five_hour", "weekly"], 24.0, 12.5),
    ],
)
def test_each_requested_meter_uses_only_its_manual_position(
    tmp_path: Path,
    targets: list[str],
    five_hour: float | None,
    weekly: float | None,
) -> None:
    database = tmp_path / "synthetic.sqlite"
    _create_database(database)
    request = _request(targets=targets)
    context = LocalExecutionContext(
        database_path=database,
        generated_at=GENERATED_AT,
        five_hour_used_percent=five_hour,
        weekly_used_percent=weekly,
    )

    result = json.loads(execute_request(json.dumps(request).encode(), context))

    assert result["result_status"] == "succeeded"
    assert [item["meter_type"] for item in result["quota_capacity_estimates"]] == targets
    for estimate in result["quota_capacity_estimates"]:
        expected_position = five_hour if estimate["meter_type"] == "five_hour" else weekly
        assert estimate["estimated_used_raw_tokens"] == pytest.approx(
            estimate["estimated_full_capacity_raw_tokens"] * expected_position / 100
        )
    assert result["generated_at"] == GENERATED_AT


def test_manual_position_for_unrequested_meter_is_rejected(tmp_path: Path) -> None:
    database = tmp_path / "synthetic.sqlite"
    _create_database(database)
    with pytest.raises(WorkerError) as error:
        execute_request(
            json.dumps(_request(targets=["five_hour"])).encode(),
            LocalExecutionContext(
                database_path=database,
                generated_at=GENERATED_AT,
                five_hour_used_percent=24.0,
                weekly_used_percent=10.0,
            ),
        )
    assert error.value.code == "unrequested_meter_position"
