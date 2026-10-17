"""Narrow stdin/stdout worker for the Rust current-capacity CLI."""

from __future__ import annotations

import argparse
import json
import math
import re
import sys
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any

from codex_meter.estimation import (
    ConfigurationProvenance,
    CurrentCapacityResultContext,
    CurrentQuotaPosition,
    ResultConfigurationIdentity,
    ResultConfigurationValue,
    build_current_capacity_result,
    derive_capacity_candidates,
)
from codex_meter.history import (
    ConfigurationKey,
    ObservationDatabase,
    ObservationSelector,
    QuotaMeterType,
    canonical_time_key,
)
from codex_meter.history.sqlite import (
    AnalyticsDataError,
    DatabaseNotFound,
    DatabaseOpenError,
    MigrationDriftError,
    ObservationCorruptError,
    StorageCompatibilityError,
)

ANALYTICS_PROCESS_PROTOCOL_VERSION = "1.0.0"
MAXIMUM_REQUEST_BYTES = 64 * 1024
MAXIMUM_OBSERVATIONS = 100_000
_OPAQUE_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$")
_UTC_TIMESTAMP = re.compile(
    r"^(?P<base>\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})(?:\.(?P<fraction>\d{1,9}))?Z$"
)
_CONFIGURATION_FIELDS = ("plan", "model", "reasoning_level", "speed_mode", "codex_version")
_REQUEST_FIELDS = {
    "schema_version",
    "request_id",
    "analysis_type",
    "dataset_selector",
    "target_quota_windows",
    "options",
}


class WorkerError(Exception):
    """A privacy-safe, categorized worker failure."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.message = message


@dataclass(frozen=True, slots=True)
class LocalExecutionContext:
    """Versioned launch metadata kept outside AnalyticsRequest v1."""

    database_path: Path
    generated_at: str
    five_hour_used_percent: float | None
    weekly_used_percent: float | None
    protocol_version: str = ANALYTICS_PROCESS_PROTOCOL_VERSION


def _object_without_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise WorkerError("invalid_request", "request contains a duplicate JSON field")
        result[key] = value
    return result


def _reject_json_constant(_value: str) -> None:
    raise WorkerError("invalid_request", "request contains a non-finite JSON number")


def _utc_timestamp(value: Any, label: str) -> str:
    if not isinstance(value, str):
        raise WorkerError("invalid_request", f"{label} must be a UTC timestamp ending in Z")
    match = _UTC_TIMESTAMP.fullmatch(value)
    if match is None:
        raise WorkerError("invalid_request", f"{label} must be a UTC timestamp ending in Z")
    try:
        datetime.strptime(match.group("base"), "%Y-%m-%dT%H:%M:%S")
    except ValueError as exc:
        raise WorkerError("invalid_request", f"{label} is not a valid UTC timestamp") from exc
    return value


def _configuration_value(value: Any, name: str) -> ResultConfigurationValue:
    if not isinstance(value, dict):
        raise WorkerError("invalid_request", f"configuration field {name} is invalid")
    availability = value.get("availability")
    provenance = value.get("provenance")
    if availability == "available":
        if set(value) != {"availability", "value", "provenance"}:
            raise WorkerError("invalid_request", f"configuration field {name} is invalid")
        if provenance not in {"observed", "derived"}:
            raise WorkerError("invalid_request", f"configuration field {name} is invalid")
        item = value["value"]
        if not isinstance(item, str) or not item or len(item) > 128:
            raise WorkerError("invalid_request", f"configuration field {name} is invalid")
        return ResultConfigurationValue(item, ConfigurationProvenance(provenance))
    if availability == "unavailable":
        if set(value) != {"availability", "provenance"} or provenance != "unavailable":
            raise WorkerError("invalid_request", f"configuration field {name} is invalid")
        return ResultConfigurationValue(None, ConfigurationProvenance.UNAVAILABLE)
    raise WorkerError("invalid_request", f"configuration field {name} is invalid")


def validate_request(document: Any) -> tuple[dict[str, Any], ResultConfigurationIdentity]:
    """Validate the closed AnalyticsRequest v1 structure and P027 execution policy."""

    if not isinstance(document, dict) or set(document) != _REQUEST_FIELDS:
        raise WorkerError(
            "invalid_request", "request must contain exactly the AnalyticsRequest v1 fields"
        )
    if document["schema_version"] != "1.0.0":
        raise WorkerError("invalid_request", "unsupported AnalyticsRequest schema version")
    request_id = document["request_id"]
    if not isinstance(request_id, str) or _OPAQUE_ID.fullmatch(request_id) is None:
        raise WorkerError("invalid_request", "request_id does not match the v1 opaque ID format")
    if document["analysis_type"] != "current_capacity_estimate":
        raise WorkerError(
            "unsupported_analysis_type", "only current_capacity_estimate is supported"
        )

    selector = document["dataset_selector"]
    if not isinstance(selector, dict) or not {"configuration"} <= set(selector):
        raise WorkerError("invalid_request", "dataset_selector is invalid")
    if set(selector) - {"configuration", "observed_from", "observed_through"}:
        raise WorkerError("invalid_request", "dataset_selector contains an unknown field")
    configuration = selector["configuration"]
    if not isinstance(configuration, dict) or set(configuration) != set(_CONFIGURATION_FIELDS):
        raise WorkerError("invalid_request", "configuration identity is invalid")
    typed_configuration = ResultConfigurationIdentity(
        *(_configuration_value(configuration[name], name) for name in _CONFIGURATION_FIELDS)
    )

    observed_from = selector.get("observed_from")
    observed_through = selector.get("observed_through")
    if "observed_from" in selector:
        _utc_timestamp(observed_from, "observed_from")
    if "observed_through" in selector:
        _utc_timestamp(observed_through, "observed_through")
    if observed_from is not None and observed_through is not None:
        if canonical_time_key(observed_from) > canonical_time_key(observed_through):
            raise WorkerError("invalid_request", "observed_from must not be after observed_through")

    targets = document["target_quota_windows"]
    if not isinstance(targets, list) or any(
        not isinstance(target, str) or target not in {"five_hour", "weekly"} for target in targets
    ):
        raise WorkerError("invalid_request", "target_quota_windows contains an invalid meter")
    if len(targets) != len(set(targets)):
        raise WorkerError("invalid_request", "target_quota_windows must be unique")

    options = document["options"]
    if not isinstance(options, dict) or set(options) - {"minimum_quality", "maximum_observations"}:
        raise WorkerError("invalid_request", "options contains an unsupported field")
    if "minimum_quality" in options:
        raise WorkerError(
            "unsupported_estimator_option",
            "minimum_quality is not supported by the P027 P018 eligibility policy",
        )
    maximum = options.get("maximum_observations")
    if "maximum_observations" in options and (
        isinstance(maximum, bool)
        or not isinstance(maximum, int)
        or not 1 <= maximum <= min(MAXIMUM_OBSERVATIONS, 9_007_199_254_740_991)
    ):
        raise WorkerError(
            "invalid_request", "maximum_observations exceeds the supported reader limit"
        )
    return document, typed_configuration


def parse_request(raw: bytes) -> tuple[dict[str, Any], ResultConfigurationIdentity]:
    """Decode one bounded UTF-8 request document and apply strict validation."""

    if len(raw) > MAXIMUM_REQUEST_BYTES:
        raise WorkerError("invalid_request", "request exceeds the 64 KiB limit")
    try:
        document = json.loads(
            raw.decode("utf-8"),
            object_pairs_hook=_object_without_duplicates,
            parse_constant=_reject_json_constant,
        )
    except WorkerError:
        raise
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise WorkerError(
            "invalid_request", "request must be one valid UTF-8 JSON document"
        ) from exc
    return validate_request(document)


def _validate_context(context: LocalExecutionContext) -> dict[QuotaMeterType, CurrentQuotaPosition]:
    if context.protocol_version != ANALYTICS_PROCESS_PROTOCOL_VERSION:
        raise WorkerError(
            "unsupported_protocol_version", "unsupported analytics process protocol version"
        )
    _utc_timestamp(context.generated_at, "generated_at")
    positions: dict[QuotaMeterType, CurrentQuotaPosition] = {}
    for meter, value in (
        (QuotaMeterType.FIVE_HOUR, context.five_hour_used_percent),
        (QuotaMeterType.WEEKLY, context.weekly_used_percent),
    ):
        if value is None:
            continue
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            raise WorkerError(
                "invalid_manual_meter_percentage", "manual meter percentage must be finite"
            )
        try:
            finite = math.isfinite(float(value))
        except (OverflowError, ValueError):
            finite = False
        if not finite:
            raise WorkerError(
                "invalid_manual_meter_percentage", "manual meter percentage must be finite"
            )
        if not 0 <= value <= 100:
            raise WorkerError(
                "invalid_manual_meter_percentage",
                "manual meter percentage must be between 0 and 100",
            )
        positions[meter] = CurrentQuotaPosition(meter, float(value))
    return positions


def _configuration_key(identity: ResultConfigurationIdentity) -> ConfigurationKey:
    return identity.configuration_key


def execute_request(raw: bytes, context: LocalExecutionContext) -> str:
    """Run the P017/P018/P019/P020 pipeline and return one AnalyticsResult v1 JSON string."""

    request, identity = parse_request(raw)
    positions = _validate_context(context)
    targets = tuple(QuotaMeterType(value) for value in request["target_quota_windows"])
    if not set(positions) <= set(targets):
        raise WorkerError(
            "unrequested_meter_position", "manual meter position targets an unrequested meter"
        )

    options = request["options"]
    selector_document = request["dataset_selector"]
    selector = ObservationSelector(
        configuration=_configuration_key(identity),
        observed_from=selector_document.get("observed_from"),
        observed_through=selector_document.get("observed_through"),
        maximum_observations=options.get("maximum_observations"),
    )
    try:
        with ObservationDatabase.open(context.database_path) as database:
            dataset = database.load_dataset(selector)
    except DatabaseNotFound as exc:
        raise WorkerError("database_missing", "analytics database does not exist") from exc
    except MigrationDriftError as exc:
        raise WorkerError(
            "migration_checksum_drift", "analytics database migration checksums are incompatible"
        ) from exc
    except StorageCompatibilityError as exc:
        raise WorkerError(
            "database_schema_incompatible", "analytics database schema is incompatible"
        ) from exc
    except ObservationCorruptError as exc:
        raise WorkerError(
            "corrupt_observation", "analytics database contains a corrupt observation"
        ) from exc
    except DatabaseOpenError as exc:
        raise WorkerError(
            "database_open_failed", "analytics database could not be opened read-only"
        ) from exc
    except AnalyticsDataError as exc:
        raise WorkerError("database_read_failed", "analytics database could not be read") from exc

    try:
        candidate_sets = {meter: derive_capacity_candidates(dataset, meter) for meter in targets}
        result = build_current_capacity_result(
            context=CurrentCapacityResultContext(
                request_id=request["request_id"],
                dataset_configuration=identity,
                target_quota_windows=targets,
                generated_at=context.generated_at,
            ),
            candidate_sets=candidate_sets,
            current_positions=positions,
        )
    except Exception as exc:
        from codex_meter.estimation import CurrentQuotaPositionRequiredError

        if isinstance(exc, CurrentQuotaPositionRequiredError):
            raise WorkerError(
                "current_quota_position_required",
                "a current meter percentage is required for sufficient historical evidence",
            ) from exc
        raise WorkerError(
            "analytics_failed", "the empirical capacity estimate could not be finalized"
        ) from exc
    return result.to_json()


def _arguments(argv: list[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(prog="python -m codex_meter.integration.worker")
    parser.add_argument("--database", required=True)
    parser.add_argument("--runtime-context-version", required=True)
    parser.add_argument("--generated-at", required=True)
    parser.add_argument("--five-hour-used-percent", type=float)
    parser.add_argument("--weekly-used-percent", type=float)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    try:
        arguments = _arguments(argv)
        context = LocalExecutionContext(
            database_path=Path(arguments.database),
            generated_at=arguments.generated_at,
            five_hour_used_percent=arguments.five_hour_used_percent,
            weekly_used_percent=arguments.weekly_used_percent,
            protocol_version=arguments.runtime_context_version,
        )
        raw = sys.stdin.buffer.read(MAXIMUM_REQUEST_BYTES + 1)
        output = execute_request(raw, context)
    except WorkerError as exc:
        print(f"{exc.code}: {exc.message}", file=sys.stderr)
        return 2
    except SystemExit as exc:
        return int(exc.code or 0)
    except Exception:
        print("worker_failure: analytics worker failed", file=sys.stderr)
        return 1
    sys.stdout.write(output)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
