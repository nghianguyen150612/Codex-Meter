"""Focused regression coverage for unpublished P030 release artifacts."""

from __future__ import annotations

import base64
import csv
import hashlib
import io
import sys
import zipfile
from pathlib import Path

import pytest

ROOT = Path(__file__).parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import p030_release_candidate as release  # noqa: E402


def _make_wheel(
    path: Path,
    *,
    version: str = "0.1.0",
    requires_python: str = ">=3.11",
    dependency: str | None = None,
) -> Path:
    dist_info = f"codex_meter_analytics-{version}.dist-info"
    metadata = (
        "Metadata-Version: 2.3\n"
        "Name: codex-meter-analytics\n"
        f"Version: {version}\n"
        f"Requires-Python: {requires_python}\n"
    )
    if dependency:
        metadata += f"Requires-Dist: {dependency}\n"
    files = {
        "codex_meter/__init__.py": b'__version__ = "0.1.0"\n',
        f"{dist_info}/METADATA": metadata.encode("utf-8"),
        f"{dist_info}/WHEEL": (
            b"Wheel-Version: 1.0\nGenerator: p030-test\nRoot-Is-Purelib: true\nTag: py3-none-any\n"
        ),
    }
    rows = []
    for name, contents in sorted(files.items()):
        digest = base64.urlsafe_b64encode(hashlib.sha256(contents).digest())
        rows.append([name, f"sha256={digest.decode('ascii').rstrip('=')}", str(len(contents))])
    record = io.StringIO(newline="")
    writer = csv.writer(record, lineterminator="\n")
    writer.writerows(rows)
    writer.writerow([f"{dist_info}/RECORD", "", ""])
    files[f"{dist_info}/RECORD"] = record.getvalue().encode("utf-8")
    with zipfile.ZipFile(path, mode="w", compression=zipfile.ZIP_DEFLATED) as archive:
        for name, contents in sorted(files.items()):
            archive.writestr(name, contents)
    return path


def test_release_versions_are_synchronized_and_methods_stay_at_v1() -> None:
    assert release.verify_version_policy() == "0.1.0"
    assert release.expected_wheel_name() == "codex_meter_analytics-0.1.0-py3-none-any.whl"
    release.verify_method_versions()


def test_version_gate_rejects_prereleases_and_unresolved_placeholders(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    assert release.VERSION_PATTERN.fullmatch("0.1.0")
    for invalid in ("0.1.0-rc.1", "v0.1.0", "0.1"):
        assert not release.VERSION_PATTERN.fullmatch(invalid)
    placeholder = tmp_path / "VERSION"
    placeholder.write_text("0.0.0\n", encoding="utf-8")
    monkeypatch.setattr(release, "VERSION_FILE", placeholder)
    with pytest.raises(release.ReleaseError, match="placeholder"):
        release.verify_version_policy()


def test_wheel_allowlist_metadata_and_record_are_verified(tmp_path: Path) -> None:
    wheel = _make_wheel(tmp_path / release.expected_wheel_name())

    result = release.verify_wheel(wheel)

    assert result["filename"] == "codex_meter_analytics-0.1.0-py3-none-any.whl"
    assert result["version"] == "0.1.0"
    assert result["sha256"] == release.sha256_file(wheel)


@pytest.mark.parametrize(
    ("version", "requires_python", "dependency"),
    [
        ("0.2.0", ">=3.11", None),
        ("0.1.0", ">=3.10", None),
        ("0.1.0", ">=3.11", "requests>=2"),
    ],
)
def test_wheel_gate_rejects_mismatched_metadata(
    tmp_path: Path, version: str, requires_python: str, dependency: str | None
) -> None:
    wheel = _make_wheel(
        tmp_path / release.expected_wheel_name(),
        version=version,
        requires_python=requires_python,
        dependency=dependency,
    )

    with pytest.raises(release.ReleaseError):
        release.verify_wheel(wheel)


def test_wheel_gate_rejects_traversal_and_extra_package_files(tmp_path: Path) -> None:
    wheel = _make_wheel(tmp_path / release.expected_wheel_name())
    with zipfile.ZipFile(wheel, mode="a") as archive:
        archive.writestr("../outside.txt", b"outside")

    with pytest.raises(release.ReleaseError):
        release.verify_wheel(wheel)


def test_candidate_archives_are_byte_reproducible_for_identical_inputs(
    tmp_path: Path,
) -> None:
    stage = tmp_path / "codex-meter-0.1.0-linux-x86_64"
    stage.mkdir()
    (stage / "LICENSE").write_text("synthetic license\n", encoding="utf-8")
    (stage / "codex-meter").write_bytes(b"synthetic executable")
    target = release.TARGETS["x86_64-unknown-linux-gnu"]

    first_tar = tmp_path / "first.tar.gz"
    second_tar = tmp_path / "second.tar.gz"
    release._write_archive(stage, first_tar, target)
    release._write_archive(stage, second_tar, target)
    assert first_tar.read_bytes() == second_tar.read_bytes()

    first_zip = tmp_path / "first.zip"
    second_zip = tmp_path / "second.zip"
    release._write_archive(stage, first_zip, target)
    release._write_archive(stage, second_zip, target)
    assert first_zip.read_bytes() == second_zip.read_bytes()


def test_candidate_archive_reader_rejects_parent_traversal(tmp_path: Path) -> None:
    archive_path = tmp_path / "unsafe.zip"
    with zipfile.ZipFile(archive_path, mode="w") as archive:
        archive.writestr("../escape", b"no")

    with pytest.raises(release.ReleaseError, match="unsafe path"):
        release._archive_member_names(archive_path)


def test_candidate_archive_reader_rejects_windows_absolute_paths(
    tmp_path: Path,
) -> None:
    archive_path = tmp_path / "windows-absolute.zip"
    with zipfile.ZipFile(archive_path, mode="w") as archive:
        archive.writestr("C:/outside.txt", b"no")

    with pytest.raises(release.ReleaseError, match="unsafe path"):
        release._archive_member_names(archive_path)


def test_candidate_directory_rejects_extra_directories_and_symlinks(
    tmp_path: Path,
) -> None:
    unexpected_directory = tmp_path / "nested"
    unexpected_directory.mkdir()
    with pytest.raises(release.ReleaseError, match="non-regular"):
        release._flat_regular_files(tmp_path)

    unexpected_directory.rmdir()
    target = tmp_path / "payload"
    target.write_text("synthetic", encoding="utf-8")
    link = tmp_path / "linked-payload"
    try:
        link.symlink_to(target)
    except OSError:
        pytest.skip("symlink creation is unavailable")
    with pytest.raises(release.ReleaseError, match="non-regular"):
        release._flat_regular_files(tmp_path)


def test_checksum_gate_covers_every_file_and_detects_changes(tmp_path: Path) -> None:
    payload = tmp_path / "candidate.tar.gz"
    payload.write_bytes(b"synthetic artifact")
    (tmp_path / "SHA256SUMS.txt").write_text(
        f"{release.sha256_file(payload)}  {payload.name}\n", encoding="utf-8"
    )

    release.verify_checksums(tmp_path)

    payload.write_bytes(b"changed artifact")
    with pytest.raises(release.ReleaseError, match="checksum mismatch"):
        release.verify_checksums(tmp_path)


def test_release_workflow_only_builds_and_uploads_candidate_artifacts() -> None:
    workflow = (ROOT / ".github" / "workflows" / "release-candidate.yml").read_text(
        encoding="utf-8"
    )
    lowered = workflow.lower()
    assert "contents: read" in lowered
    assert "upload-artifact" in lowered
    assert "github.release" not in lowered
    assert "gh release" not in lowered
    assert "pypi" not in lowered
    assert "cargo publish" not in lowered
