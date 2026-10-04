"""Validate every v1 schema and explicitly inventoried contract fixture locally."""

from __future__ import annotations

import json
import re
import sys
from datetime import datetime
from pathlib import Path
from typing import Any

from jsonschema import Draft202012Validator, FormatChecker, SchemaError
from referencing import Registry, Resource

ROOT = Path(__file__).resolve().parents[1]
SCHEMA_DIR = ROOT / "schemas" / "v1"
FIXTURE_DIR = ROOT / "fixtures" / "contracts" / "v1"

FIXTURES = {
    "analytics-estimate-request.json": "analytics-request.schema.json",
    "analytics-result-insufficient.json": "analytics-result.schema.json",
    "analytics-result-success.json": "analytics-result.schema.json",
    "normalized-token-event.json": "normalized-event.schema.json",
    "normalized-token-event-codex-rollout.json": "normalized-event.schema.json",
    "normalized-session-detected-codex-rollout.json": "normalized-event.schema.json",
    "normalized-configuration-event-codex-rollout.json": "normalized-event.schema.json",
    "observation-finalized.json": "observation.schema.json",
    "observation-incomplete.json": "observation.schema.json",
    "observation-reset-crossing.json": "observation.schema.json",
    "observation-codex-finalized.json": "observation.schema.json",
    "observation-codex-reset.json": "observation.schema.json",
    "observation-codex-incomplete.json": "observation.schema.json",
    "quota-sample.json": "quota-sample.schema.json",
    "quota-sample-codex-five-hour.json": "quota-sample.schema.json",
    "quota-sample-codex-weekly.json": "quota-sample.schema.json",
    "invalid/event-session-started-token-payload.json": "normalized-event.schema.json",
    "invalid/event-token-update-lifecycle-payload.json": "normalized-event.schema.json",
    "invalid/observation-five-hour-as-weekly.json": "observation.schema.json",
    "invalid/observation-weekly-as-five-hour.json": "observation.schema.json",
    "invalid/quota-sample-reset-meter-mismatch.json": "quota-sample.schema.json",
}

FORMAT_CHECKER = FormatChecker()
RFC3339_UTC = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z$")


@FORMAT_CHECKER.checks("date-time", raises=ValueError)
def is_rfc3339_datetime(value: object) -> bool:
    """Check RFC 3339-shaped timestamps without optional network-installed extras."""
    if not isinstance(value, str):
        return True
    if RFC3339_UTC.fullmatch(value) is None:
        return False
    datetime.fromisoformat(value.replace("Z", "+00:00"))
    return True


def load_json(path: Path) -> Any:
    with path.open(encoding="utf-8") as stream:
        return json.load(stream)


def main() -> int:
    schema_paths = sorted(SCHEMA_DIR.glob("*.schema.json"))
    schemas = {path.name: load_json(path) for path in schema_paths}
    failures: list[str] = []

    expected_schemas = set(schemas)
    mapped_schemas = set(FIXTURES.values())
    if expected_schemas != mapped_schemas | {"common.schema.json"}:
        failures.append("fixture inventory does not cover exactly the six v1 schemas")

    for name, schema in schemas.items():
        try:
            Draft202012Validator.check_schema(schema)
        except SchemaError as error:
            failures.append(f"invalid schema {name}: {error}")

    base_uri = SCHEMA_DIR.resolve().as_uri().rstrip("/") + "/"
    resources = [
        (base_uri + name, Resource.from_contents(schema))
        for name, schema in schemas.items()
    ]
    registry = Registry().with_resources(resources)

    discovered = {
        path.relative_to(FIXTURE_DIR).as_posix() for path in FIXTURE_DIR.rglob("*.json")
    }
    inventoried = set(FIXTURES)
    for name in sorted(discovered - inventoried):
        failures.append(f"unclassified fixture: fixtures/contracts/v1/{name}")
    for name in sorted(inventoried - discovered):
        failures.append(f"inventoried fixture is missing: fixtures/contracts/v1/{name}")

    positive_count = 0
    negative_count = 0
    for fixture_name, schema_name in FIXTURES.items():
        path = FIXTURE_DIR / Path(fixture_name)
        if not path.is_file():
            continue
        schema = {"$id": base_uri + schema_name, **schemas[schema_name]}
        validator = Draft202012Validator(
            schema, registry=registry, format_checker=FORMAT_CHECKER
        )
        errors = list(validator.iter_errors(load_json(path)))
        is_negative = Path(fixture_name).parts[0] == "invalid"
        if is_negative:
            negative_count += 1
            if not errors:
                failures.append(f"negative fixture unexpectedly passed: {fixture_name}")
        else:
            positive_count += 1
            if errors:
                details = "; ".join(error.message for error in errors[:3])
                failures.append(f"positive fixture failed: {fixture_name}: {details}")

    summary = (
        f"Contract validation: {len(schemas)} schemas, {positive_count} positive fixtures, "
        f"{negative_count} negative fixtures"
    )
    if failures:
        print(summary + " — FAILED", file=sys.stderr)
        for failure in failures:
            print(f"- {failure}", file=sys.stderr)
        return 1
    print(summary + " — passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
