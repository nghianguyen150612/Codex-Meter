"""Build a local wheel and exercise a relocated CLI in a fresh Python venv."""

from __future__ import annotations

import concurrent.futures
import hashlib
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import venv
import zipfile
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
PYTHON_PROJECT = ROOT / "python"
MIGRATION_DIR = ROOT / "rust" / "crates" / "codex-meter" / "migrations"
OBSERVATION_FIXTURE = (
    ROOT / "fixtures" / "contracts" / "v1" / "observation-finalized.json"
)
GENERATED_AT = "2026-10-16T07:00:00Z"

sys.path.insert(0, str(PYTHON_PROJECT))
from codex_meter.history.sqlite import MIGRATIONS, canonical_time_key  # noqa: E402


def run(
    command: list[str], *, cwd: Path | None = None, env: dict[str, str] | None = None
) -> str:
    completed = subprocess.run(
        command,
        cwd=cwd,
        env=env,
        check=False,
        capture_output=True,
        timeout=90,
    )
    if completed.returncode:
        raise RuntimeError(
            f"command failed ({completed.returncode}): {Path(command[0]).name}\n"
            f"stdout: {completed.stdout.decode('utf-8', errors='replace')[-4000:]}\n"
            f"stderr: {completed.stderr.decode('utf-8', errors='replace')[-4000:]}"
        )
    return completed.stdout.decode("utf-8")


def build_wheel(directory: Path) -> Path:
    directory.mkdir(parents=True)
    environment = os.environ.copy()
    environment["SOURCE_DATE_EPOCH"] = "1792137600"
    run(
        [
            "uv",
            "build",
            "--project",
            str(PYTHON_PROJECT),
            "--wheel",
            "--out-dir",
            str(directory),
        ],
        cwd=ROOT,
        env=environment,
    )
    wheels = list(directory.glob("*.whl"))
    if len(wheels) != 1:
        raise AssertionError("local build must produce exactly one wheel")
    return wheels[0]


def validate_wheel(wheel: Path) -> None:
    with zipfile.ZipFile(wheel) as archive:
        names = sorted(archive.namelist())
        entries = set(names)
        expected_modules = {
            path.relative_to(PYTHON_PROJECT).as_posix()
            for path in (PYTHON_PROJECT / "codex_meter").rglob("*.py")
        }
        actual_modules = {name for name in names if name.startswith("codex_meter/")}
        if actual_modules != expected_modules:
            raise AssertionError(
                "wheel must contain every package module and no extra package files"
            )
        unexpected_roots = [
            name
            for name in names
            if not name.startswith("codex_meter/") and ".dist-info/" not in name
        ]
        if unexpected_roots:
            raise AssertionError(
                f"wheel contains files outside package and metadata roots: {unexpected_roots}"
            )
        required = {
            "codex_meter/integration/worker.py",
            "codex_meter/history/sqlite.py",
            "codex_meter/estimation/result.py",
            "codex_meter/analysis/historical.py",
        }
        if not required <= entries:
            raise AssertionError("wheel is missing a worker or analytics module")
        forbidden = (
            ".env",
            "credential",
            "secret",
            ".sqlite",
            ".db",
            ".jsonl",
            "__pycache__",
            ".pyc",
            ".git/",
            ".venv/",
            "/tests/",
            "private",
        )
        for name in names:
            lowered = name.lower()
            if any(item in lowered for item in forbidden):
                raise AssertionError(f"wheel contains a prohibited file: {name}")
        metadata_files = [
            name for name in names if name.endswith(".dist-info/METADATA")
        ]
        if len(metadata_files) != 1:
            raise AssertionError("wheel must contain one distribution metadata file")
        metadata = archive.read(metadata_files[0]).decode("utf-8")
        if "Name: codex-meter-analytics\n" not in metadata:
            raise AssertionError("wheel distribution name changed")
        if "Requires-Python: >=3.11\n" not in metadata:
            raise AssertionError("wheel Python compatibility metadata changed")
        if "Requires-Dist:" in metadata:
            raise AssertionError("wheel unexpectedly declares runtime dependencies")


def create_synthetic_database(path: Path) -> None:
    connection = sqlite3.connect(path)
    connection.execute(
        "CREATE TABLE schema_migrations (version INTEGER NOT NULL PRIMARY KEY, "
        "name TEXT NOT NULL UNIQUE, checksum_sha256 TEXT NOT NULL)"
    )
    for version, name, checksum in MIGRATIONS:
        migration = MIGRATION_DIR / f"{version:04d}_{name[5:]}.sql"
        connection.executescript(migration.read_text(encoding="utf-8"))
        connection.execute(
            "INSERT INTO schema_migrations VALUES (?, ?, ?)", (version, name, checksum)
        )
    fixture = json.loads(OBSERVATION_FIXTURE.read_text(encoding="utf-8"))
    for index in range(2):
        document = json.loads(json.dumps(fixture))
        document["observation_id"] = f"p029-observation-{index}"
        document["task_id"] = f"p029-task-{index}"
        document["session_id"] = f"p029-session-{index}"
        hour = index + 2
        document["timing"]["started_at"] = f"2026-10-01T{hour:02}:00:00Z"
        document["timing"]["ended_at"] = f"2026-10-01T{hour:02}:15:00Z"
        document["timing"]["finalized_at"] = f"2026-10-01T{hour:02}:20:00Z"
        raw_total = 14500 + index * 1500
        document["token_evidence"]["raw_token_counters"]["raw_total"]["value"] = (
            raw_total
        )
        payload = json.dumps(document, separators=(",", ":"))
        configuration = document["configuration"]
        values: dict[str, Any] = {
            "observation_id": document["observation_id"],
            "schema_version": document["schema_version"],
            "lifecycle_state": document["lifecycle_state"],
            "started_at": canonical_time_key(document["timing"]["started_at"]),
            "ended_at": canonical_time_key(document["timing"]["ended_at"]),
            "finalized_at": canonical_time_key(document["timing"]["finalized_at"]),
            "summary_quality": document["summary_quality"],
            "plan": configuration["plan"]["value"],
            "model": configuration["model"]["value"],
            "reasoning_level": configuration["reasoning_level"]["value"],
            "speed_mode": configuration["speed_mode"]["value"],
            "codex_version": None,
            "token_validity": "valid",
            "token_quality": "B",
            "raw_total": raw_total,
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
            f"INSERT INTO observations ({columns}) VALUES ({placeholders})",
            tuple(values.values()),
        )
    connection.commit()
    connection.close()


def request_document(request_id: str, *, insufficient: bool = False) -> dict[str, Any]:
    selector: dict[str, Any] = {
        "configuration": {
            "plan": {
                "availability": "available",
                "value": "plus",
                "provenance": "observed",
            },
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
                "provenance": "observed",
            },
            "codex_version": {
                "availability": "unavailable",
                "provenance": "unavailable",
            },
        },
        "observed_through": "2026-10-15T23:59:59Z",
    }
    if insufficient:
        selector["observed_from"] = "2026-10-03T00:00:00Z"
    else:
        selector["observed_from"] = "2026-10-01T00:00:00Z"
    return {
        "schema_version": "1.0.0",
        "request_id": request_id,
        "analysis_type": "current_capacity_estimate",
        "dataset_selector": selector,
        "target_quota_windows": ["five_hour"],
        "options": {"maximum_observations": 10},
    }


def write_request(path: Path, request_id: str, *, insufficient: bool = False) -> None:
    path.write_text(
        json.dumps(request_document(request_id, insufficient=insufficient)),
        encoding="utf-8",
    )


def cli_command(
    binary: Path,
    python: Path,
    database: Path,
    request: Path,
    *,
    mode: str = "installed",
    extra: tuple[str, ...] = ("--five-hour-used-percent", "24"),
) -> list[str]:
    return [
        str(binary),
        "estimate",
        "--database",
        str(database),
        "--request",
        str(request),
        "--generated-at",
        GENERATED_AT,
        "--python",
        str(python),
        "--worker-mode",
        mode,
        *extra,
        "--json",
    ]


def invoke(
    command: list[str], *, cwd: Path, env: dict[str, str] | None = None
) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(
        command,
        cwd=cwd,
        env=env,
        check=False,
        capture_output=True,
        timeout=70,
    )


def expect_resolution_error(
    binary: Path,
    python: Path,
    database: Path,
    request: Path,
    cwd: Path,
    *,
    mode: str = "installed",
) -> None:
    result = invoke(
        cli_command(binary, python, database, request, mode=mode, extra=()), cwd=cwd
    )
    diagnostic = result.stderr.decode("utf-8", errors="replace")
    if (
        result.returncode == 0
        or result.stdout
        or "python_worker_resolution_failed" not in diagnostic
    ):
        raise AssertionError("missing or ambiguous installed workers must fail closed")
    if str(python) in diagnostic or str(database) in diagnostic:
        raise AssertionError("worker-resolution errors must not disclose local paths")


def main() -> None:
    executable_name = "codex-meter.exe" if os.name == "nt" else "codex-meter"
    binary = ROOT / "rust" / "target" / "debug" / executable_name
    if not binary.is_file():
        raise FileNotFoundError("build rust/Cargo.toml before running this smoke test")

    with tempfile.TemporaryDirectory(prefix="codex-meter P029 雪 ") as temporary:
        base = Path(temporary)
        wheel_one = build_wheel(base / "wheel output one")
        wheel_two = build_wheel(base / "wheel output two")
        validate_wheel(wheel_one)
        validate_wheel(wheel_two)
        if (
            hashlib.sha256(wheel_one.read_bytes()).digest()
            != hashlib.sha256(wheel_two.read_bytes()).digest()
        ):
            raise AssertionError(
                "repeated local wheel builds must be byte-for-byte reproducible"
            )

        installed_environment = base / "installed 雪 environment with spaces"
        venv.EnvBuilder(with_pip=True).create(installed_environment)
        if os.name == "nt":
            python = installed_environment / "Scripts" / "python.exe"
            package_root = installed_environment / "Lib" / "site-packages"
            if not python.is_file():
                raise AssertionError("Windows venv must use Scripts/python.exe")
        else:
            python = installed_environment / "bin" / "python"
            lib = installed_environment / "lib"
            package_root = next(lib.glob("python*/site-packages"))
            if not python.is_file():
                raise AssertionError("POSIX venv must use bin/python")
        run(
            [
                str(python),
                "-m",
                "pip",
                "install",
                "--disable-pip-version-check",
                "--no-index",
                "--no-deps",
                str(wheel_one),
            ],
            cwd=base,
        )

        relocated_directory = base / "relocated CLI 雪 with spaces"
        relocated_directory.mkdir()
        relocated_binary = relocated_directory / executable_name
        shutil.copy2(binary, relocated_binary)
        caller_directory = base / "unrelated caller directory 雪 space"
        caller_directory.mkdir()
        database = base / "synthetic history 雪 data.sqlite"
        request = base / "analytics request 雪 input.json"
        create_synthetic_database(database)
        write_request(request, "p029-installed-smoke")
        original_hash = hashlib.sha256(database.read_bytes()).digest()

        shadow = caller_directory / "codex_meter"
        shadow.mkdir()
        (shadow / "__init__.py").write_text("raise SystemExit(97)\n", encoding="utf-8")
        shadow_integration = shadow / "integration"
        shadow_integration.mkdir()
        (shadow_integration / "__init__.py").write_text("", encoding="utf-8")
        (shadow_integration / "worker.py").write_text(
            "raise SystemExit(98)\n", encoding="utf-8"
        )
        shadow_environment = os.environ.copy()
        shadow_environment["PYTHONPATH"] = str(caller_directory)
        shadow_environment["PYTHONHOME"] = "invalid-python-home-marker"

        command = cli_command(relocated_binary, python, database, request)
        first = invoke(command, cwd=caller_directory, env=shadow_environment)
        if first.returncode:
            raise AssertionError(first.stderr.decode("utf-8", errors="replace"))
        result = json.loads(first.stdout)
        if (
            result["result_status"] != "succeeded"
            or result["request_id"] != "p029-installed-smoke"
        ):
            raise AssertionError(
                "installed worker returned an unexpected AnalyticsResult v1"
            )
        estimate = result["quota_capacity_estimates"][0]
        full_capacity = estimate["estimated_full_capacity_raw_tokens"]
        used_capacity = estimate["estimated_used_raw_tokens"]
        if abs(used_capacity - full_capacity * 0.24) > max(
            1e-8, abs(full_capacity) * 1e-12
        ):
            raise AssertionError(
                "manual used percentage must be applied to the validated result"
            )
        if (
            invoke(command, cwd=caller_directory, env=shadow_environment).stdout
            != first.stdout
        ):
            raise AssertionError(
                "identical installed-worker inputs must be deterministic"
            )

        with tempfile.TemporaryDirectory(
            prefix="codex-meter P029 home caller 雪 ", dir=Path.home()
        ) as home_caller:
            from_home = invoke(command, cwd=Path(home_caller), env=shadow_environment)
            if from_home.returncode or from_home.stdout != first.stdout:
                raise AssertionError(
                    "the installed worker must be independent of the home working directory"
                )

        def concurrent_run(index: int) -> dict[str, Any]:
            concurrent_request = caller_directory / f"request-{index}.json"
            write_request(concurrent_request, f"p029-concurrent-{index}")
            output = invoke(
                cli_command(relocated_binary, python, database, concurrent_request),
                cwd=caller_directory,
                env=shadow_environment,
            )
            if output.returncode:
                raise AssertionError(output.stderr.decode("utf-8", errors="replace"))
            return json.loads(output.stdout)

        with concurrent.futures.ThreadPoolExecutor(max_workers=4) as executor:
            concurrent_results = list(executor.map(concurrent_run, range(4)))
        if {item["request_id"] for item in concurrent_results} != {
            f"p029-concurrent-{index}" for index in range(4)
        }:
            raise AssertionError(
                "concurrent requests must preserve independent request IDs"
            )
        if hashlib.sha256(database.read_bytes()).digest() != original_hash:
            raise AssertionError(
                "successful and concurrent analyses must not modify SQLite"
            )

        insufficient_request = caller_directory / "insufficient.json"
        write_request(insufficient_request, "p029-insufficient", insufficient=True)
        insufficient = invoke(
            cli_command(
                relocated_binary,
                python,
                database,
                insufficient_request,
                extra=(),
            ),
            cwd=caller_directory,
            env=shadow_environment,
        )
        if insufficient.returncode or json.loads(insufficient.stdout)[
            "result_status"
        ] != ("insufficient_evidence"):
            raise AssertionError(
                "synthetic sparse history must retain insufficient-evidence output"
            )

        relative_database = caller_directory / "relative.sqlite"
        relative_request = caller_directory / "relative.json"
        shutil.copy2(database, relative_database)
        write_request(relative_request, "p029-relative-paths")
        relative = invoke(
            cli_command(
                relocated_binary,
                python,
                Path("relative.sqlite"),
                Path("relative.json"),
            ),
            cwd=caller_directory,
            env=shadow_environment,
        )
        if (
            relative.returncode
            or json.loads(relative.stdout)["request_id"] != "p029-relative-paths"
        ):
            raise AssertionError(
                "relative paths must remain relative to the caller's working directory"
            )

        linked_database = caller_directory / "linked.sqlite"
        try:
            os.symlink(database, linked_database)
        except (OSError, NotImplementedError):
            linked_database = Path()
        if linked_database != Path():
            linked = invoke(
                cli_command(relocated_binary, python, linked_database, request),
                cwd=caller_directory,
                env=shadow_environment,
            )
            if linked.returncode:
                raise AssertionError(
                    "supported database symlinks must retain read-only behavior"
                )

        missing_database = caller_directory / "missing database.sqlite"
        missing = invoke(
            cli_command(relocated_binary, python, missing_database, request),
            cwd=caller_directory,
        )
        if (
            missing.returncode == 0
            or missing_database.exists()
            or b"database_missing" not in missing.stderr
        ):
            raise AssertionError("a missing database must fail before creating a file")

        unusable_python = invoke(
            cli_command(
                relocated_binary, base / "missing Python interpreter", database, request
            ),
            cwd=caller_directory,
        )
        if (
            unusable_python.returncode == 0
            or b"python_unavailable" not in unusable_python.stderr
        ):
            raise AssertionError(
                "an unusable selected interpreter must fail without fallback"
            )

        empty_environment = base / "uninstalled environment"
        venv.EnvBuilder(with_pip=False).create(empty_environment)
        if os.name == "nt":
            missing_python = empty_environment / "Scripts" / "python.exe"
        else:
            missing_python = empty_environment / "bin" / "python"
        expect_resolution_error(
            relocated_binary, missing_python, database, request, caller_directory
        )

        dist_infos = list(package_root.glob("codex_meter_analytics-*.dist-info"))
        if len(dist_infos) != 1:
            raise AssertionError(
                "fresh installation must have one distribution metadata directory"
            )
        duplicate = package_root / "codex_meter_analytics_duplicate.dist-info"
        shutil.copytree(dist_infos[0], duplicate)
        expect_resolution_error(
            relocated_binary, python, database, request, caller_directory
        )
        shutil.rmtree(duplicate)

        source_mode = invoke(
            cli_command(
                binary,
                Path(sys.executable),
                database,
                request,
                mode="source",
            ),
            cwd=ROOT,
        )
        if source_mode.returncode:
            raise AssertionError("source checkout mode must remain available")
        if hashlib.sha256(database.read_bytes()).digest() != original_hash:
            raise AssertionError(
                "source and installed modes must leave SQLite unchanged"
            )

        source_root = ROOT / "python"
        hidden_source_root = ROOT / f".p029-python-source-{os.getpid()}"
        if hidden_source_root.exists():
            raise AssertionError("temporary source-path test directory already exists")
        source_root.rename(hidden_source_root)
        try:
            without_source = invoke(
                cli_command(relocated_binary, python, database, request, mode="auto"),
                cwd=caller_directory,
                env=shadow_environment,
            )
            if without_source.returncode or without_source.stdout != first.stdout:
                raise AssertionError(
                    "auto mode must use the installed worker when the source tree is absent: "
                    + without_source.stderr.decode("utf-8", errors="replace")
                )
            expect_resolution_error(
                relocated_binary,
                missing_python,
                database,
                request,
                caller_directory,
                mode="auto",
            )
        finally:
            hidden_source_root.rename(source_root)

        if os.name == "nt":
            extended_database = Path("\\\\?\\" + str(database.resolve()))
            extended_request = Path("\\\\?\\" + str(request.resolve()))
            extended = invoke(
                cli_command(
                    relocated_binary, python, extended_database, extended_request
                ),
                cwd=caller_directory,
                env=shadow_environment,
            )
            if extended.returncode:
                raise AssertionError(
                    "Windows extended-length local paths must work when enabled; "
                    f"exit={extended.returncode}, stderr="
                    f"{extended.stderr.decode('utf-8', errors='replace')}"
                )

    print(
        "P029 runtime smoke passed: reproducible wheel, isolated installation, relocation, "
        "source mode, independent cwd, paths, determinism, concurrency, and read-only SQLite"
    )


if __name__ == "__main__":
    main()
