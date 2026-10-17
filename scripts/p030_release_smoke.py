"""Extract and exercise one unpublished candidate using only synthetic history."""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import venv
from pathlib import Path

import p029_runtime_smoke as p029
import p030_release_candidate as release


def run(command: list[str], *, cwd: Path, env: dict[str, str] | None = None) -> bytes:
    result = subprocess.run(
        command,
        cwd=cwd,
        env=env,
        check=False,
        capture_output=True,
        timeout=90,
    )
    if result.returncode:
        raise AssertionError(
            f"command failed ({result.returncode}): {Path(command[0]).name}; "
            f"stderr={result.stderr.decode('utf-8', errors='replace')[-3000:]}"
        )
    return result.stdout


def expect_incompatible_worker(
    binary: Path, python: Path, database: Path, request: Path, cwd: Path
) -> None:
    result = subprocess.run(
        p029.cli_command(binary, python, database, request, extra=()),
        cwd=cwd,
        check=False,
        capture_output=True,
        timeout=90,
    )
    diagnostic = result.stderr.decode("utf-8", errors="replace")
    if (
        result.returncode == 0
        or result.stdout
        or "python_worker_resolution_failed" not in diagnostic
        or str(python) in diagnostic
        or str(database) in diagnostic
    ):
        raise AssertionError(
            "a mismatched analytics wheel must fail closed without paths"
        )


def main() -> int:
    if len(sys.argv) != 2:
        raise SystemExit(
            "usage: python scripts/p030_release_smoke.py CANDIDATE_ARCHIVE"
        )
    archive_path = Path(sys.argv[1]).resolve(strict=True)
    verified = release.verify_candidate_archive(archive_path)
    manifest = verified["manifest"]
    runtime_version = tuple(sys.version_info[:2])
    if runtime_version not in {(3, 11), (3, 12), (3, 13)}:
        raise AssertionError(
            "release smoke must use a supported Python 3.11, 3.12, or 3.13"
        )

    members = release._archive_member_names(archive_path)
    root_name = next(iter(members)).split("/", maxsplit=1)[0]
    with tempfile.TemporaryDirectory(
        prefix="codex-meter-extracted-candidate-"
    ) as temporary:
        base = Path(temporary).resolve()
        if base == release.ROOT or base.is_relative_to(release.ROOT):
            raise AssertionError(
                "candidate extraction must be outside the source checkout"
            )
        extraction = base / "fresh extraction"
        extraction.mkdir()
        for name, contents in members.items():
            root, relative = name.split("/", maxsplit=1)
            if root != root_name:
                raise AssertionError(
                    "candidate archive must have one top-level directory"
                )
            destination = extraction / relative
            if destination.parent != extraction:
                raise AssertionError(
                    "candidate archive inventory must contain direct children"
                )
            destination.write_bytes(contents)
        expected_binary = manifest["artifacts"]["binary"]["filename"]
        extracted_binary = extraction / expected_binary
        if os.name != "nt":
            extracted_binary.chmod(0o755)
        relocated_directory = base / "relocated executable"
        relocated_directory.mkdir()
        binary = relocated_directory / expected_binary
        shutil.copy2(extracted_binary, binary)
        binary_bytes = binary.read_bytes()
        if str(release.ROOT.resolve()).encode("utf-8") in binary_bytes:
            raise AssertionError("release binary embeds its source-checkout path")

        version_output = run([str(binary), "--version"], cwd=base)
        if (
            version_output.decode("utf-8").strip()
            != f"codex-meter {release.verify_version_policy()}"
        ):
            raise AssertionError(
                "relocated executable reports the wrong product version"
            )
        help_output = run([str(binary), "estimate", "--help"], cwd=base)
        if (
            b"--worker-mode MODE" not in help_output
            or b"--python PATH" not in help_output
        ):
            raise AssertionError(
                "estimate help must document installed worker selection"
            )

        environment = base / "fresh Python environment"
        venv.EnvBuilder(with_pip=True).create(environment)
        if os.name == "nt":
            python = environment / "Scripts" / "python.exe"
            site_packages = environment / "Lib" / "site-packages"
        else:
            python = environment / "bin" / "python"
            site_packages = next((environment / "lib").glob("python*/site-packages"))
        wheel = extraction / release.expected_wheel_name()
        run(
            [
                str(python),
                "-m",
                "pip",
                "install",
                "--disable-pip-version-check",
                "--no-index",
                "--no-deps",
                str(wheel),
            ],
            cwd=base,
        )

        caller = base / "unrelated caller directory"
        caller.mkdir()
        database = base / "synthetic history.sqlite"
        request = base / "synthetic request.json"
        p029.create_synthetic_database(database)
        p029.write_request(request, "p030-installed-candidate")
        original_database_hash = hashlib.sha256(database.read_bytes()).digest()
        cli_environment = os.environ.copy()
        cli_environment["PYTHONPATH"] = str(caller)
        cli_environment["PYTHONHOME"] = "invalid-python-home-marker"

        command = p029.cli_command(binary, python, database, request)
        first = subprocess.run(
            command,
            cwd=caller,
            env=cli_environment,
            check=False,
            capture_output=True,
            timeout=90,
        )
        if first.returncode:
            raise AssertionError(first.stderr.decode("utf-8", errors="replace"))
        result = json.loads(first.stdout)
        estimate = result["quota_capacity_estimates"][0]
        if (
            result.get("schema_version") != "1.0.0"
            or result.get("estimator_method_version") != "1.0.0"
            or result.get("result_status") != "succeeded"
            or result.get("request_id") != "p030-installed-candidate"
            or estimate.get("metric_kind") != "estimated_raw_tokens"
        ):
            raise AssertionError(
                "installed candidate returned an unexpected AnalyticsResult v1"
            )
        # The native CLI performs strict AnalyticsResult v1 schema and semantic
        # validation before it writes this output to stdout.
        full = estimate["estimated_full_capacity_raw_tokens"]
        used = estimate["estimated_used_raw_tokens"]
        if abs(used - full * 0.24) > max(1e-8, abs(full) * 1e-12):
            raise AssertionError("manual current-meter arithmetic changed")
        replay = subprocess.run(
            command,
            cwd=caller,
            env=cli_environment,
            check=False,
            capture_output=True,
            timeout=90,
        )
        if replay.returncode or replay.stdout != first.stdout:
            raise AssertionError("installed candidate output must be deterministic")

        insufficient_request = base / "insufficient request.json"
        p029.write_request(insufficient_request, "p030-insufficient", insufficient=True)
        insufficient = subprocess.run(
            p029.cli_command(binary, python, database, insufficient_request, extra=()),
            cwd=caller,
            env=cli_environment,
            check=False,
            capture_output=True,
            timeout=90,
        )
        if (
            insufficient.returncode
            or json.loads(insufficient.stdout).get("result_status")
            != "insufficient_evidence"
        ):
            raise AssertionError(
                "sparse synthetic history must preserve insufficient evidence"
            )

        if hashlib.sha256(database.read_bytes()).digest() != original_database_hash:
            raise AssertionError("installed candidate modified its SQLite database")

        dist_infos = list(site_packages.glob("codex_meter_analytics-*.dist-info"))
        if len(dist_infos) != 1:
            raise AssertionError(
                "fresh venv must contain exactly one analytics distribution"
            )
        metadata_path = dist_infos[0] / "METADATA"
        original_metadata = metadata_path.read_bytes()
        expected_metadata = f"Version: {release.verify_version_policy()}\n".encode()
        if expected_metadata not in original_metadata:
            raise AssertionError("installed wheel metadata version is inconsistent")
        metadata_path.write_bytes(
            original_metadata.replace(expected_metadata, b"Version: 0.0.0\n", 1)
        )
        expect_incompatible_worker(binary, python, database, request, caller)
        metadata_path.write_bytes(original_metadata)

        for marker in (
            b"SYNTHETIC_SECRET_MARKER",
            b"SYNTHETIC_PROMPT_MARKER",
            b"SYNTHETIC_RESPONSE_MARKER",
            b"SYNTHETIC_SOURCE_MARKER",
        ):
            if any(marker in value for value in members.values()):
                raise AssertionError(
                    "candidate archive contains a privacy-canary marker"
                )

    print(
        f"P030 extracted candidate smoke passed: {verified['target_triple']}, "
        f"source {verified['source_commit_sha']}, installed worker, sufficient and "
        "insufficient estimates, deterministic JSON, version compatibility, and read-only SQLite"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
