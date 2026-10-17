"""Version checks and deterministic, unpublished P030 candidate bundles."""

from __future__ import annotations

import argparse
import ast
import base64
import csv
import email.parser
import gzip
import hashlib
import io
import json
import os
import platform
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import zipfile
from pathlib import Path, PurePosixPath
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
VERSION_FILE = ROOT / "VERSION"
RUST_PACKAGE = ROOT / "rust" / "crates" / "codex-meter" / "Cargo.toml"
PYTHON_PROJECT = ROOT / "python" / "pyproject.toml"
PYTHON_INIT = ROOT / "python" / "codex_meter" / "__init__.py"
REPOSITORY_URL = "https://github.com/nghianguyen150612/Codex-Meter"
PRODUCT = "codex-meter"
WHEEL_DISTRIBUTION = "codex-meter-analytics"
WHEEL_PROJECT = "codex_meter_analytics"
WHEEL_TAG = "py3-none-any"
PYTHON_REQUIRES = ">=3.11"
SOURCE_DATE_EPOCH = "1792137600"
VERSION_PATTERN = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
COMMIT_PATTERN = re.compile(r"^[0-9a-f]{40}$")
SUPPORTED_PYTHON_VERSIONS = ("3.11", "3.12", "3.13")

TARGETS: dict[str, dict[str, str]] = {
    "x86_64-unknown-linux-gnu": {
        "slug": "linux-x86_64",
        "os": "Linux",
        "architecture": "x86_64",
        "archive_suffix": "tar.gz",
        "executable": "codex-meter",
    },
    "x86_64-pc-windows-msvc": {
        "slug": "windows-x86_64",
        "os": "Windows",
        "architecture": "x86_64",
        "archive_suffix": "zip",
        "executable": "codex-meter.exe",
    },
    "x86_64-apple-darwin": {
        "slug": "macos-x86_64",
        "os": "macOS",
        "architecture": "x86_64",
        "archive_suffix": "tar.gz",
        "executable": "codex-meter",
    },
    "aarch64-apple-darwin": {
        "slug": "macos-arm64",
        "os": "macOS",
        "architecture": "arm64",
        "archive_suffix": "tar.gz",
        "executable": "codex-meter",
    },
}

METHOD_VERSION_EXPECTATIONS = {
    "python/codex_meter/estimation/result.py": ("SCHEMA_VERSION", "1.0.0"),
    "python/codex_meter/estimation/robust.py": ("ESTIMATOR_METHOD_VERSION", "1.0.0"),
    "python/codex_meter/integration/worker.py": (
        "ANALYTICS_PROCESS_PROTOCOL_VERSION",
        "1.0.0",
    ),
    "python/codex_meter/analysis/workload.py": ("WORKLOAD_METHOD_VERSION", "1.0.0"),
    "python/codex_meter/analysis/regime_models.py": (
        "REGIME_SIGNAL_METHOD_VERSION",
        "1.0.0",
    ),
    "python/codex_meter/analysis/benchmark_models.py": (
        "BENCHMARK_ANALYSIS_METHOD_VERSION",
        "1.0.0",
    ),
    "rust/crates/codex-meter/src/benchmark/model.rs": (
        "BENCHMARK_FRAMEWORK_VERSION",
        "1.0.0",
    ),
    "rust/crates/codex-meter/src/benchmark/runtime.rs": (
        "BENCHMARK_RUNTIME_VERSION",
        "1.0.0",
    ),
    "rust/crates/codex-meter/src/analytics/process.rs": (
        "ANALYTICS_PROCESS_PROTOCOL_VERSION",
        "1.0.0",
    ),
}

SCHEMA_VERSION_GUARDS = {
    "rust/crates/codex-meter/src/analytics/request.rs": r'schema_version != "1\.0\.0"',
    "rust/crates/codex-meter/src/analytics/result.rs": r'schema_version != "1\.0\.0"',
    "python/codex_meter/integration/worker.py": r'document\["schema_version"\] != "1\.0\.0"',
}


class ReleaseError(RuntimeError):
    """A release policy or artifact validation failure."""


def _read_python_version(path: Path) -> str:
    module = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    for statement in module.body:
        if isinstance(statement, ast.Assign):
            names = [
                target.id
                for target in statement.targets
                if isinstance(target, ast.Name)
            ]
            if "__version__" in names and isinstance(statement.value, ast.Constant):
                if isinstance(statement.value.value, str):
                    return statement.value.value
    raise ReleaseError(
        "Python package __version__ is missing or is not a string constant"
    )


def verify_method_versions() -> None:
    for relative, (constant, expected) in METHOD_VERSION_EXPECTATIONS.items():
        source = (ROOT / relative).read_text(encoding="utf-8")
        pattern = re.compile(
            rf"\b{re.escape(constant)}\b\s*(?::[^=\n]+)?=\s*\"([^\"]+)\""
        )
        match = pattern.search(source)
        if match is None or match.group(1) != expected:
            found = "missing" if match is None else match.group(1)
            raise ReleaseError(
                f"{constant} in {relative} must remain {expected}; found {found}"
            )
    for relative, guard in SCHEMA_VERSION_GUARDS.items():
        source = (ROOT / relative).read_text(encoding="utf-8")
        if re.search(guard, source) is None:
            raise ReleaseError(f"Analytics v1 schema guard is missing from {relative}")


def verify_version_policy() -> str:
    version = VERSION_FILE.read_text(encoding="utf-8").strip()
    if not VERSION_PATTERN.fullmatch(version) or version == "0.0.0":
        raise ReleaseError(
            "VERSION must contain a stable semantic version, not a placeholder"
        )

    cargo_version = tomllib.loads(RUST_PACKAGE.read_text(encoding="utf-8"))["package"][
        "version"
    ]
    python_project = tomllib.loads(PYTHON_PROJECT.read_text(encoding="utf-8"))[
        "project"
    ]
    python_version = _read_python_version(PYTHON_INIT)
    versions = {
        "VERSION": version,
        "Cargo package": cargo_version,
        "Python project": python_project["version"],
        "Python import package": python_version,
    }
    mismatches = {name: value for name, value in versions.items() if value != version}
    if mismatches:
        details = ", ".join(f"{name}={value}" for name, value in mismatches.items())
        raise ReleaseError(
            f"release component versions must all equal {version}: {details}"
        )
    if python_project.get("requires-python") != PYTHON_REQUIRES:
        raise ReleaseError(f"Python requirement must remain {PYTHON_REQUIRES}")
    if python_project.get("dependencies") != []:
        raise ReleaseError(
            "the analytics distribution must have no runtime dependencies"
        )
    verify_method_versions()
    return version


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def expected_wheel_name(version: str | None = None) -> str:
    return f"{WHEEL_PROJECT}-{version or verify_version_policy()}-{WHEEL_TAG}.whl"


def _wheel_record_rows(
    archive: zipfile.ZipFile, record_name: str
) -> dict[str, tuple[str, str]]:
    rows = csv.reader(io.StringIO(archive.read(record_name).decode("utf-8")))
    result: dict[str, tuple[str, str]] = {}
    for row in rows:
        if len(row) != 3 or row[0] in result:
            raise ReleaseError("wheel RECORD must contain unique three-column entries")
        result[row[0]] = (row[1], row[2])
    return result


def verify_wheel(path: Path, *, source_root: Path | None = None) -> dict[str, Any]:
    version = verify_version_policy()
    if path.name != expected_wheel_name(version):
        raise ReleaseError(f"wheel filename must be {expected_wheel_name(version)}")
    if not path.is_file() or path.is_symlink():
        raise ReleaseError("wheel must be a regular file")

    try:
        archive = zipfile.ZipFile(path)
    except (OSError, zipfile.BadZipFile) as error:
        raise ReleaseError("wheel is not a readable ZIP archive") from error
    with archive:
        infos = archive.infolist()
        names = [item.filename for item in infos]
        if len(names) != len(set(names)):
            raise ReleaseError("wheel contains duplicate file names")
        for info in infos:
            name = info.filename
            pure = PurePosixPath(name)
            mode = (info.external_attr >> 16) & 0o170000
            if (
                pure.is_absolute()
                or re.match(r"^[A-Za-z]:", name) is not None
                or ".." in pure.parts
                or "\\" in name
                or not name
                or mode not in {0, stat.S_IFREG}
            ):
                raise ReleaseError(f"wheel contains an unsafe path: {name}")
            lowered = name.lower()
            if any(
                marker in lowered
                for marker in (
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
            ):
                raise ReleaseError(f"wheel contains a prohibited file: {name}")

        dist_info = f"{WHEEL_PROJECT}-{version}.dist-info/"
        metadata_name = dist_info + "METADATA"
        wheel_name = dist_info + "WHEEL"
        record_name = dist_info + "RECORD"
        allowed_metadata = {metadata_name, wheel_name, record_name}
        package_names = {name for name in names if name.startswith("codex_meter/")}
        unexpected = set(names) - package_names - allowed_metadata
        if unexpected:
            raise ReleaseError(
                f"wheel contains files outside its allowlist: {sorted(unexpected)}"
            )
        if not {metadata_name, wheel_name, record_name}.issubset(names):
            raise ReleaseError("wheel is missing required .dist-info metadata")
        if not package_names or any(not name.endswith(".py") for name in package_names):
            raise ReleaseError(
                "wheel package inventory must contain Python source files only"
            )

        if source_root is not None:
            source_package = source_root / "codex_meter"
            expected = {
                path.relative_to(source_root).as_posix()
                for path in source_package.rglob("*.py")
                if path.is_file() and not path.is_symlink()
            }
            if package_names != expected:
                missing = sorted(expected - package_names)
                extra = sorted(package_names - expected)
                raise ReleaseError(
                    f"wheel package inventory mismatch; missing={missing}, extra={extra}"
                )

        metadata = email.parser.BytesParser().parsebytes(archive.read(metadata_name))
        if (
            metadata.get("Name") != WHEEL_DISTRIBUTION
            or metadata.get("Version") != version
            or metadata.get("Requires-Python") != PYTHON_REQUIRES
            or metadata.get_all("Requires-Dist")
        ):
            raise ReleaseError(
                "wheel metadata has an unexpected name, version, Python, or dependency"
            )
        wheel_metadata = archive.read(wheel_name).decode("utf-8")
        if f"Tag: {WHEEL_TAG}\n" not in wheel_metadata:
            raise ReleaseError("wheel must be platform-independent py3-none-any")

        records = _wheel_record_rows(archive, record_name)
        if set(records) != set(names):
            raise ReleaseError("wheel RECORD does not enumerate every packaged file")
        for name in names:
            digest, size = records[name]
            contents = archive.read(name)
            if name == record_name:
                if digest or size:
                    raise ReleaseError(
                        "wheel RECORD must leave its own digest and size blank"
                    )
                continue
            expected_digest = "sha256=" + base64.urlsafe_b64encode(
                hashlib.sha256(contents).digest()
            ).decode("ascii").rstrip("=")
            if digest != expected_digest or size != str(len(contents)):
                raise ReleaseError(f"wheel RECORD integrity mismatch for {name}")

    return {
        "filename": path.name,
        "version": version,
        "sha256": sha256_file(path),
        "size_bytes": path.stat().st_size,
        "package_files": sorted(package_names),
    }


def _read_command(arguments: list[str]) -> str:
    try:
        result = subprocess.run(
            arguments,
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
            timeout=30,
        )
    except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        raise ReleaseError(f"could not run {Path(arguments[0]).name}") from error
    return result.stdout.strip()


def _commit_sha() -> str:
    sha = _read_command(["git", "rev-parse", "HEAD"])
    if not COMMIT_PATTERN.fullmatch(sha):
        raise ReleaseError("source commit must be a full 40-character Git SHA")
    return sha


def _ensure_clean_source() -> None:
    status = _read_command(["git", "status", "--porcelain", "--untracked-files=all"])
    if status:
        raise ReleaseError("candidate packaging requires a clean source checkout")


def _normalize_architecture(value: str) -> str:
    normalized = value.lower()
    if normalized in {"amd64", "x64", "x86_64"}:
        return "x86_64"
    if normalized in {"arm64", "aarch64"}:
        return "arm64"
    return normalized


def verify_native_target(target_triple: str) -> dict[str, str]:
    try:
        target = TARGETS[target_triple]
    except KeyError as error:
        raise ReleaseError(f"unsupported release target: {target_triple}") from error
    host_system = platform.system()
    expected_system = target["os"]
    same_system = host_system == expected_system or (
        host_system == "Darwin" and expected_system == "macOS"
    )
    host_architecture = _normalize_architecture(platform.machine())
    expected_architecture = _normalize_architecture(target["architecture"])
    if not same_system or host_architecture != expected_architecture:
        raise ReleaseError(
            f"{target_triple} requires a native {expected_system} {target['architecture']} runner; "
            f"found {host_system} {platform.machine()}"
        )
    return target


def _flat_regular_files(directory: Path) -> dict[str, Path]:
    if not directory.is_dir() or directory.is_symlink():
        raise ReleaseError("candidate directory must be a real directory")
    files: dict[str, Path] = {}
    for path in directory.iterdir():
        if path.is_symlink() or not path.is_file():
            raise ReleaseError(
                f"candidate directory contains a non-regular entry: {path.name}"
            )
        files[path.name] = path
    return files


def _json_bytes(document: Any) -> bytes:
    return (json.dumps(document, indent=2, sort_keys=True) + "\n").encode("utf-8")


def _bundle_readme(version: str) -> str:
    return f"""# Codex Meter {version}

Codex Meter is an independent, local-first current-capacity estimation CLI. This candidate
contains a native `codex-meter` executable and the `codex-meter-analytics` Python wheel. It is not
a standalone binary.

## Requirements

- Python 3.11, 3.12, or 3.13 in a local environment.
- The matching `codex-meter-analytics` wheel installed in that environment.
- An existing compatible Codex Meter SQLite database and an AnalyticsRequest v1 JSON file.

See [INSTALLATION.md](INSTALLATION.md) for setup, checksum verification, interpreter selection,
and example commands. Check the executable with `codex-meter --version` and
`codex-meter estimate --help`.

The `estimate` command reads SQLite history in read-only mode. It does not create or migrate a
database, call provider APIs, start a tracker, or run benchmarks. Current meter percentages are
entered manually and are unverified.

Codex Meter is independent and unofficial. Its empirical estimates are not documented provider
quota allowances.
"""


def _build_manifest(
    *,
    target_triple: str,
    binary: Path,
    wheel: Path,
    wheel_info: dict[str, Any],
    run_id: str,
) -> dict[str, Any]:
    target = TARGETS[target_triple]
    source_sha = _commit_sha()
    if run_id != "local" and not run_id.isdigit():
        raise ReleaseError(
            "workflow run ID must be numeric, or 'local' for a local verification"
        )
    binary_version = _read_command([str(binary), "--version"])
    version = verify_version_policy()
    if binary_version != f"codex-meter {version}":
        raise ReleaseError(
            f"native executable reports an unexpected version: {binary_version}"
        )
    rustc = _read_command(["rustc", "--version", "--verbose"])
    cargo = _read_command(["cargo", "--version"])
    python_version = platform.python_version()
    if ".".join(python_version.split(".")[:2]) not in SUPPORTED_PYTHON_VERSIONS:
        raise ReleaseError(
            f"candidate packaging requires supported Python {SUPPORTED_PYTHON_VERSIONS}"
        )
    event = (
        "local" if run_id == "local" else os.environ.get("GITHUB_EVENT_NAME", "unknown")
    )
    ref = (
        "local"
        if run_id == "local"
        else os.environ.get("GITHUB_HEAD_REF")
        or os.environ.get("GITHUB_REF", "unknown")
    )
    server = os.environ.get("GITHUB_SERVER_URL", "https://github.com").rstrip("/")
    repository = os.environ.get("GITHUB_REPOSITORY", "nghianguyen150612/Codex-Meter")
    if run_id != "local" and (
        event == "unknown"
        or ref == "unknown"
        or repository != "nghianguyen150612/Codex-Meter"
    ):
        raise ReleaseError("GitHub workflow provenance environment is incomplete")
    workflow_run_url = (
        "local" if run_id == "local" else f"{server}/{repository}/actions/runs/{run_id}"
    )
    return {
        "format_version": 1,
        "product": PRODUCT,
        "product_version": version,
        "source": {
            "repository": REPOSITORY_URL,
            "commit_sha": source_sha,
            "ref": ref,
            "event": event,
            "workflow_file": ".github/workflows/release-candidate.yml",
            "workflow_run_id": run_id,
            "workflow_run_url": workflow_run_url,
        },
        "build": {
            "rustc": rustc,
            "cargo": cargo,
            "python": python_version,
            "target_triple": target_triple,
            "configuration": [
                "cargo build --manifest-path rust/Cargo.toml --workspace --release --locked "
                f"--target {target_triple} --features codex-meter/release-artifact",
                f"uv build --project python --wheel (SOURCE_DATE_EPOCH={SOURCE_DATE_EPOCH})",
            ],
            "worker_mode": "installed",
        },
        "artifacts": {
            "binary": {
                "filename": target["executable"],
                "sha256": sha256_file(binary),
                "size_bytes": binary.stat().st_size,
            },
            "wheel": {
                "filename": wheel_info["filename"],
                "sha256": wheel_info["sha256"],
                "size_bytes": wheel_info["size_bytes"],
            },
        },
        "reproducibility": {
            "wheel": "Two builds with the same source, backend, and SOURCE_DATE_EPOCH are byte-compared in CI.",
            "archive": "Entry order and timestamps are normalized; repacking identical inputs is deterministic.",
            "native_binary": "Toolchain and target are recorded; bit-for-bit reproducibility across builds is not claimed.",
        },
    }


def _archive_member_names(path: Path) -> dict[str, bytes]:
    members: dict[str, bytes] = {}
    if path.name.endswith(".zip"):
        try:
            with zipfile.ZipFile(path) as archive:
                infos = archive.infolist()
                for info in infos:
                    name = info.filename
                    if name in members:
                        raise ReleaseError(
                            f"candidate archive contains a duplicate entry: {name}"
                        )
                    if info.is_dir():
                        raise ReleaseError(
                            "candidate ZIP must not contain directory entries"
                        )
                    mode = (info.external_attr >> 16) & 0o170000
                    if mode not in {0, stat.S_IFREG}:
                        raise ReleaseError(
                            f"candidate ZIP contains a non-regular entry: {name}"
                        )
                    members[name] = archive.read(info)
        except (OSError, zipfile.BadZipFile) as error:
            raise ReleaseError("candidate ZIP archive is not readable") from error
    elif path.name.endswith(".tar.gz"):
        try:
            with tarfile.open(path, mode="r:gz") as archive:
                for member in archive.getmembers():
                    if member.name in members:
                        raise ReleaseError(
                            f"candidate archive contains a duplicate entry: {member.name}"
                        )
                    if not member.isfile() or member.issym() or member.islnk():
                        raise ReleaseError(
                            "candidate tar archive may contain regular files only"
                        )
                    stream = archive.extractfile(member)
                    if stream is None:
                        raise ReleaseError("candidate tar entry could not be read")
                    members[member.name] = stream.read()
        except (OSError, tarfile.TarError) as error:
            raise ReleaseError("candidate tar archive is not readable") from error
    else:
        raise ReleaseError(f"unsupported candidate archive type: {path.name}")
    if len(members) != len(set(members)):
        raise ReleaseError("candidate archive contains duplicate entries")
    for name in members:
        pure = PurePosixPath(name)
        if (
            pure.is_absolute()
            or ".." in pure.parts
            or re.match(r"^[A-Za-z]:", name) is not None
            or "\\" in name
            or "\x00" in name
            or not name
        ):
            raise ReleaseError(f"candidate archive contains an unsafe path: {name}")
    return members


def _write_archive(stage: Path, archive_path: Path, target: dict[str, str]) -> None:
    files = sorted(
        path for path in stage.iterdir() if path.is_file() and not path.is_symlink()
    )
    if len(files) != len(list(stage.iterdir())):
        raise ReleaseError(
            "candidate staging directory contains an unexpected file type"
        )
    if archive_path.name.endswith(".zip"):
        with zipfile.ZipFile(
            archive_path, mode="w", compression=zipfile.ZIP_DEFLATED, compresslevel=9
        ) as archive:
            for path in files:
                info = zipfile.ZipInfo(
                    f"{stage.name}/{path.name}", date_time=(1980, 1, 1, 0, 0, 0)
                )
                info.compress_type = zipfile.ZIP_DEFLATED
                info.create_system = 3
                mode = 0o755 if path.name == target["executable"] else 0o644
                info.external_attr = (stat.S_IFREG | mode) << 16
                archive.writestr(
                    info, path.read_bytes(), compress_type=zipfile.ZIP_DEFLATED
                )
        return

    with archive_path.open("wb") as output:
        with gzip.GzipFile(
            fileobj=output, mode="wb", filename="", mtime=0, compresslevel=9
        ) as gz:
            with tarfile.open(
                fileobj=gz, mode="w", format=tarfile.USTAR_FORMAT
            ) as archive:
                for path in files:
                    contents = path.read_bytes()
                    info = tarfile.TarInfo(f"{stage.name}/{path.name}")
                    info.size = len(contents)
                    info.mtime = 0
                    info.uid = 0
                    info.gid = 0
                    info.uname = ""
                    info.gname = ""
                    info.mode = 0o755 if path.name == target["executable"] else 0o644
                    archive.addfile(info, io.BytesIO(contents))


def package_candidate(
    *,
    binary: Path,
    wheel: Path,
    target_triple: str,
    output_directory: Path,
    run_id: str,
) -> Path:
    _ensure_clean_source()
    version = verify_version_policy()
    target = verify_native_target(target_triple)
    if not binary.is_file() or binary.is_symlink():
        raise ReleaseError("candidate executable must be a regular file")
    if binary.name != target["executable"]:
        raise ReleaseError(f"candidate executable must be named {target['executable']}")
    wheel_info = verify_wheel(wheel, source_root=ROOT / "python")
    output_directory.mkdir(parents=True, exist_ok=True)
    archive_name = f"{PRODUCT}-{version}-{target['slug']}.{target['archive_suffix']}"
    archive_path = output_directory / archive_name
    if archive_path.exists():
        raise ReleaseError(
            f"refusing to overwrite existing candidate archive: {archive_path.name}"
        )

    root_name = f"{PRODUCT}-{version}-{target['slug']}"
    with tempfile.TemporaryDirectory(prefix="codex-meter-candidate-") as temporary:
        temporary_path = Path(temporary)
        stage = temporary_path / root_name
        stage.mkdir()
        shutil.copyfile(binary, stage / target["executable"])
        shutil.copyfile(wheel, stage / wheel_info["filename"])
        (stage / "README.md").write_text(
            _bundle_readme(version), encoding="utf-8", newline="\n"
        )
        shutil.copyfile(ROOT / "docs" / "INSTALLATION.md", stage / "INSTALLATION.md")
        shutil.copyfile(ROOT / "LICENSE", stage / "LICENSE")
        manifest = _build_manifest(
            target_triple=target_triple,
            binary=binary,
            wheel=wheel,
            wheel_info=wheel_info,
            run_id=run_id,
        )
        (stage / "build-manifest.json").write_bytes(_json_bytes(manifest))
        _write_archive(stage, archive_path, target)

    return archive_path


def verify_candidate_archive(
    path: Path, *, expected_source_sha: str | None = None
) -> dict[str, Any]:
    version = verify_version_policy()
    if not path.is_file() or path.is_symlink():
        raise ReleaseError("candidate archive must be a regular file")
    target_match = None
    target_triple = None
    for triple, target in TARGETS.items():
        expected = f"{PRODUCT}-{version}-{target['slug']}.{target['archive_suffix']}"
        if path.name == expected:
            target_match = target
            target_triple = triple
            break
    if target_match is None or target_triple is None:
        raise ReleaseError(f"unexpected candidate archive name: {path.name}")
    root_name = f"{PRODUCT}-{version}-{target_match['slug']}"
    members = _archive_member_names(path)
    prefix = f"{root_name}/"
    expected_names = {
        prefix + target_match["executable"],
        prefix + expected_wheel_name(version),
        prefix + "README.md",
        prefix + "INSTALLATION.md",
        prefix + "LICENSE",
        prefix + "build-manifest.json",
    }
    if set(members) != expected_names:
        missing = sorted(expected_names - set(members))
        extra = sorted(set(members) - expected_names)
        raise ReleaseError(
            f"candidate archive inventory mismatch; missing={missing}, extra={extra}"
        )
    manifest = json.loads(members[prefix + "build-manifest.json"])
    if not isinstance(manifest, dict):
        raise ReleaseError("candidate build manifest must be a JSON object")
    build = manifest.get("build", {})
    if not isinstance(build, dict):
        raise ReleaseError("candidate build metadata must be a JSON object")
    if (
        manifest.get("format_version") != 1
        or manifest.get("product") != PRODUCT
        or manifest.get("product_version") != version
        or build.get("target_triple") != target_triple
        or build.get("worker_mode") != "installed"
    ):
        raise ReleaseError(
            "candidate build manifest has inconsistent product or target metadata"
        )
    source = manifest.get("source", {})
    if not isinstance(source, dict):
        raise ReleaseError("candidate source provenance must be a JSON object")
    source_sha = source.get("commit_sha", "")
    run_id = source.get("workflow_run_id", "")
    source_event = source.get("event")
    source_ref = source.get("ref")
    if not isinstance(source_sha, str) or not isinstance(run_id, str):
        raise ReleaseError("candidate source SHA and workflow run ID must be strings")
    if not isinstance(source_event, str) or not isinstance(source_ref, str):
        raise ReleaseError("candidate source event and ref must be strings")
    if not COMMIT_PATTERN.fullmatch(source_sha) or not (
        run_id.isdigit() or (run_id == "local" and source_event == "local")
    ):
        raise ReleaseError(
            "candidate manifest is missing exact source or workflow identity"
        )
    if (
        source.get("repository") != REPOSITORY_URL
        or source.get("workflow_file") != ".github/workflows/release-candidate.yml"
        or not source_ref
        or not source_event
        or source.get("workflow_run_url")
        != (
            "local"
            if run_id == "local"
            else f"https://github.com/nghianguyen150612/Codex-Meter/actions/runs/{run_id}"
        )
    ):
        raise ReleaseError("candidate manifest has inconsistent repository provenance")
    python_version = str(build.get("python", ""))
    if (
        ".".join(python_version.split(".")[:2]) not in SUPPORTED_PYTHON_VERSIONS
        or not build.get("rustc")
        or not build.get("cargo")
    ):
        raise ReleaseError("candidate manifest is missing validated toolchain versions")
    if expected_source_sha is not None and source_sha != expected_source_sha:
        raise ReleaseError(
            "candidate source SHA does not match the requested source commit"
        )
    artifact_metadata = manifest.get("artifacts", {})
    if not isinstance(artifact_metadata, dict):
        raise ReleaseError("candidate artifact metadata must be a JSON object")
    binary_info = artifact_metadata.get("binary", {})
    wheel_info = artifact_metadata.get("wheel", {})
    if not isinstance(binary_info, dict) or not isinstance(wheel_info, dict):
        raise ReleaseError("candidate component metadata must be JSON objects")
    binary_bytes = members[prefix + target_match["executable"]]
    wheel_bytes = members[prefix + expected_wheel_name(version)]
    if (
        binary_info.get("filename") != target_match["executable"]
        or binary_info.get("sha256") != sha256_bytes(binary_bytes)
        or binary_info.get("size_bytes") != len(binary_bytes)
        or wheel_info.get("filename") != expected_wheel_name(version)
        or wheel_info.get("sha256") != sha256_bytes(wheel_bytes)
        or wheel_info.get("size_bytes") != len(wheel_bytes)
    ):
        raise ReleaseError("candidate manifest hashes do not match packaged bytes")

    with tempfile.TemporaryDirectory(prefix="codex-meter-wheel-check-") as temporary:
        wheel_path = Path(temporary) / expected_wheel_name(version)
        wheel_path.write_bytes(wheel_bytes)
        verified_wheel = verify_wheel(wheel_path)
    if verified_wheel["sha256"] != wheel_info["sha256"]:
        raise ReleaseError("packaged wheel failed independent integrity validation")
    return {
        "filename": path.name,
        "target_triple": target_triple,
        "product_version": version,
        "source_commit_sha": source_sha,
        "workflow_run_id": run_id,
        "sha256": sha256_file(path),
        "size_bytes": path.stat().st_size,
        "wheel_sha256": wheel_info["sha256"],
        "manifest": manifest,
    }


def _checksum_text(files: list[Path], directory: Path) -> str:
    return "".join(
        f"{sha256_file(path)}  {path.relative_to(directory).as_posix()}\n"
        for path in sorted(files)
    )


def verify_checksums(directory: Path) -> None:
    files = _flat_regular_files(directory)
    sums_path = directory / "SHA256SUMS.txt"
    if not sums_path.is_file():
        raise ReleaseError("SHA256SUMS.txt is missing")
    lines = sums_path.read_text(encoding="utf-8").splitlines()
    records: dict[str, str] = {}
    for line in lines:
        match = re.fullmatch(r"([0-9a-f]{64})  ([A-Za-z0-9._-]+)", line)
        if match is None or match.group(2) in records:
            raise ReleaseError("SHA256SUMS.txt contains an invalid or duplicate record")
        records[match.group(2)] = match.group(1)
    files.pop(sums_path.name, None)
    if set(records) != set(files):
        raise ReleaseError(
            "SHA256SUMS.txt must cover every candidate file except itself"
        )
    for name, path in files.items():
        if records[name] != sha256_file(path):
            raise ReleaseError(f"checksum mismatch for {name}")


def aggregate_candidates(directory: Path) -> dict[str, Any]:
    version = verify_version_policy()
    files = _flat_regular_files(directory)
    expected_archives = {
        f"{PRODUCT}-{version}-{target['slug']}.{target['archive_suffix']}"
        for target in TARGETS.values()
    }
    wheel_name = expected_wheel_name(version)
    actual = set(files)
    if actual != expected_archives | {wheel_name}:
        raise ReleaseError(
            f"candidate directory inventory mismatch; expected={sorted(expected_archives | {wheel_name})}, "
            f"found={sorted(actual)}"
        )
    wheel_info = verify_wheel(directory / wheel_name, source_root=ROOT / "python")
    candidates = [
        verify_candidate_archive(directory / name) for name in sorted(expected_archives)
    ]
    source_shas = {candidate["source_commit_sha"] for candidate in candidates}
    run_ids = {candidate["workflow_run_id"] for candidate in candidates}
    source_events = {
        candidate["manifest"]["source"]["event"] for candidate in candidates
    }
    source_refs = {candidate["manifest"]["source"]["ref"] for candidate in candidates}
    wheel_hashes = {candidate["wheel_sha256"] for candidate in candidates}
    if (
        len(source_shas) != 1
        or len(run_ids) != 1
        or len(source_events) != 1
        or len(source_refs) != 1
        or wheel_hashes != {wheel_info["sha256"]}
    ):
        raise ReleaseError(
            "candidate builds must share one source SHA, workflow run, and wheel"
        )
    artifact_manifest = {
        "format_version": 1,
        "product": PRODUCT,
        "product_version": version,
        "repository": REPOSITORY_URL,
        "source_commit_sha": next(iter(source_shas)),
        "workflow_run_id": next(iter(run_ids)),
        "source_event": next(iter(source_events)),
        "source_ref": next(iter(source_refs)),
        "python_versions_supported": list(SUPPORTED_PYTHON_VERSIONS),
        "target_triples_supported": sorted(TARGETS),
        "wheel": {
            "filename": wheel_info["filename"],
            "sha256": wheel_info["sha256"],
            "size_bytes": wheel_info["size_bytes"],
        },
        "artifacts": [
            {
                "filename": item["filename"],
                "target_triple": item["target_triple"],
                "product_version": item["product_version"],
                "source_commit_sha": item["source_commit_sha"],
                "workflow_run_id": item["workflow_run_id"],
                "source_event": item["manifest"]["source"]["event"],
                "source_ref": item["manifest"]["source"]["ref"],
                "size_bytes": item["size_bytes"],
                "sha256": item["sha256"],
                "wheel_sha256": item["wheel_sha256"],
            }
            for item in candidates
        ],
        "publishing": "unpublished CI candidate; no tag, GitHub Release, or registry publication",
    }
    manifest_path = directory / "artifact-manifest.json"
    manifest_path.write_bytes(_json_bytes(artifact_manifest))
    hashed_files = [
        path
        for name, path in _flat_regular_files(directory).items()
        if name != "SHA256SUMS.txt"
    ]
    (directory / "SHA256SUMS.txt").write_text(
        _checksum_text(hashed_files, directory), encoding="utf-8", newline="\n"
    )
    verify_candidate_directory(directory)
    return artifact_manifest


def verify_candidate_directory(directory: Path) -> dict[str, Any]:
    version = verify_version_policy()
    files = _flat_regular_files(directory)
    artifact_manifest_path = directory / "artifact-manifest.json"
    if not artifact_manifest_path.is_file():
        raise ReleaseError("artifact-manifest.json is missing")
    try:
        artifact_manifest = json.loads(
            artifact_manifest_path.read_text(encoding="utf-8")
        )
    except (OSError, json.JSONDecodeError) as error:
        raise ReleaseError("artifact-manifest.json is invalid") from error
    if not isinstance(artifact_manifest, dict):
        raise ReleaseError("artifact manifest must be a JSON object")
    if (
        artifact_manifest.get("format_version") != 1
        or artifact_manifest.get("product") != PRODUCT
        or artifact_manifest.get("product_version") != version
        or artifact_manifest.get("repository") != REPOSITORY_URL
        or artifact_manifest.get("python_versions_supported")
        != list(SUPPORTED_PYTHON_VERSIONS)
        or artifact_manifest.get("target_triples_supported") != sorted(TARGETS)
        or artifact_manifest.get("publishing")
        != "unpublished CI candidate; no tag, GitHub Release, or registry publication"
    ):
        raise ReleaseError(
            "artifact manifest has an unexpected product or publication status"
        )

    versioned_names = {
        f"{PRODUCT}-{version}-{target['slug']}.{target['archive_suffix']}"
        for target in TARGETS.values()
    } | {expected_wheel_name(version), "artifact-manifest.json", "SHA256SUMS.txt"}
    actual = set(files)
    if actual != versioned_names:
        raise ReleaseError(
            "final candidate directory contains missing or unexpected files"
        )
    source_sha = artifact_manifest.get("source_commit_sha")
    run_id = artifact_manifest.get("workflow_run_id")
    source_event = artifact_manifest.get("source_event")
    source_ref = artifact_manifest.get("source_ref")
    if (
        not isinstance(source_sha, str)
        or not COMMIT_PATTERN.fullmatch(source_sha)
        or not isinstance(run_id, str)
        or not isinstance(source_event, str)
        or not isinstance(source_ref, str)
        or not source_ref
        or not (run_id.isdigit() or (run_id == "local" and source_event == "local"))
    ):
        raise ReleaseError("artifact manifest is missing source or workflow identity")
    expected_hashes: dict[str, str] = {}
    artifacts = artifact_manifest.get("artifacts", [])
    if not isinstance(artifacts, list) or len(artifacts) != len(TARGETS):
        raise ReleaseError("artifact manifest must list all four native targets")
    observed_targets: set[str] = set()
    for item in artifacts:
        if not isinstance(item, dict):
            raise ReleaseError("artifact manifest entries must be JSON objects")
        target_triple = item.get("target_triple")
        if not isinstance(target_triple, str) or target_triple not in TARGETS:
            raise ReleaseError("artifact manifest contains an unsupported target")
        target = TARGETS[target_triple]
        expected_filename = (
            f"{PRODUCT}-{version}-{target['slug']}.{target['archive_suffix']}"
        )
        if item.get("filename") != expected_filename:
            raise ReleaseError("artifact manifest contains an unexpected archive name")
        if (
            item.get("source_commit_sha") != source_sha
            or item.get("workflow_run_id") != run_id
            or item.get("source_event") != source_event
            or item.get("source_ref") != artifact_manifest.get("source_ref")
        ):
            raise ReleaseError(
                "artifact manifest mixes source commits or workflow runs"
            )
        archive = directory / expected_filename
        verified = verify_candidate_archive(archive, expected_source_sha=source_sha)
        if (
            item.get("target_triple") != verified["target_triple"]
            or verified["manifest"]["source"]["event"] != source_event
            or verified["manifest"]["source"]["ref"]
            != artifact_manifest.get("source_ref")
            or item.get("product_version") != version
            or item.get("sha256") != verified["sha256"]
            or item.get("size_bytes") != verified["size_bytes"]
            or item.get("wheel_sha256") != verified["wheel_sha256"]
        ):
            raise ReleaseError(f"artifact manifest disagrees with {archive.name}")
        observed_targets.add(verified["target_triple"])
        expected_hashes[archive.name] = verified["sha256"]
    if observed_targets != set(TARGETS) or len(expected_hashes) != len(TARGETS):
        raise ReleaseError(
            "artifact manifest must include all four native distribution targets"
        )

    wheel_path = directory / expected_wheel_name(version)
    wheel_info = verify_wheel(wheel_path, source_root=ROOT / "python")
    wheel_manifest = artifact_manifest.get("wheel", {})
    if not isinstance(wheel_manifest, dict):
        raise ReleaseError("artifact wheel metadata must be a JSON object")
    if (
        wheel_manifest.get("filename") != wheel_info["filename"]
        or wheel_manifest.get("sha256") != wheel_info["sha256"]
        or wheel_manifest.get("size_bytes") != wheel_info["size_bytes"]
    ):
        raise ReleaseError("artifact manifest disagrees with the shared wheel")
    verify_checksums(directory)
    return artifact_manifest


def _command_check_version(_: argparse.Namespace) -> None:
    version = verify_version_policy()
    print(f"release version policy verified: {version}")


def _command_verify_wheel(arguments: argparse.Namespace) -> None:
    print(
        json.dumps(
            verify_wheel(arguments.wheel, source_root=ROOT / "python"), sort_keys=True
        )
    )


def _command_package(arguments: argparse.Namespace) -> None:
    path = package_candidate(
        binary=arguments.binary,
        wheel=arguments.wheel,
        target_triple=arguments.target_triple,
        output_directory=arguments.output_directory,
        run_id=arguments.workflow_run_id,
    )
    print(path)


def _command_aggregate(arguments: argparse.Namespace) -> None:
    document = aggregate_candidates(arguments.directory)
    print(
        f"validated {len(document['artifacts'])} platform bundles and shared wheel "
        f"for source {document['source_commit_sha']}"
    )


def _command_verify(arguments: argparse.Namespace) -> None:
    document = verify_candidate_directory(arguments.directory)
    print(
        f"verified {len(document['artifacts'])} platform bundles and shared wheel "
        f"for source {document['source_commit_sha']}"
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(required=True)
    check = subparsers.add_parser(
        "check-version", help="check release and method versions"
    )
    check.set_defaults(function=_command_check_version)
    wheel = subparsers.add_parser(
        "verify-wheel", help="validate a Python analytics wheel"
    )
    wheel.add_argument("--wheel", type=Path, required=True)
    wheel.set_defaults(function=_command_verify_wheel)
    package = subparsers.add_parser("package", help="build one native platform bundle")
    package.add_argument("--binary", type=Path, required=True)
    package.add_argument("--wheel", type=Path, required=True)
    package.add_argument("--target-triple", required=True, choices=sorted(TARGETS))
    package.add_argument("--output-directory", type=Path, required=True)
    package.add_argument("--workflow-run-id", required=True)
    package.set_defaults(function=_command_package)
    aggregate = subparsers.add_parser(
        "aggregate", help="create aggregate hashes and manifest"
    )
    aggregate.add_argument("--directory", type=Path, required=True)
    aggregate.set_defaults(function=_command_aggregate)
    verify = subparsers.add_parser(
        "verify", help="verify a complete candidate directory"
    )
    verify.add_argument("--directory", type=Path, required=True)
    verify.set_defaults(function=_command_verify)
    arguments = parser.parse_args()
    try:
        arguments.function(arguments)
    except (OSError, ReleaseError, ValueError, KeyError) as error:
        print(f"release validation error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
