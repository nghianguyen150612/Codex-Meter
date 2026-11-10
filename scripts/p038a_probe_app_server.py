#!/usr/bin/env python3
"""Read-only Codex App Server initialize probe for P038A qualification."""

from __future__ import annotations

import argparse
import asyncio
import json
import re
import shutil
import sys
from pathlib import Path
from typing import Any

INITIALIZE_ID = 1
MAX_LINE_BYTES = 4 * 1024 * 1024
FIELD_RE = re.compile(r"[A-Za-z0-9._/-]{1,256}")


async def drain(stream: asyncio.StreamReader) -> None:
    while await stream.read(8192):
        pass


AUDITED_RUNTIME = "codex_cli_rs/0.163.0"


def classify_initialize(response: Any) -> tuple[str, dict[str, str]]:
    """Validate metadata without returning paths or the full user-agent suffix."""
    if not isinstance(response, dict) or "error" in response:
        return "invalid_initialize_response", {}
    result = response.get("result")
    if not isinstance(result, dict):
        return "invalid_initialize_response", {}
    limits = {
        "codexHome": None,
        "userAgent": 1024,
        "platformFamily": 128,
        "platformOs": 128,
    }
    for field, limit in limits.items():
        if field not in result:
            return "missing_required_metadata_" + field, {}
        value = result[field]
        if (
            not isinstance(value, str)
            or not value
            or "\0" in value
            or (limit is not None and len(value.encode()) > limit)
        ):
            return "invalid_required_metadata_" + field, {}
    agent = result["userAgent"]
    # Match execution.rs exactly; do not broaden the audited runtime policy.
    suffix = agent.removeprefix(AUDITED_RUNTIME)
    supported = agent.startswith(AUDITED_RUNTIME) and (not suffix or suffix[0] in " /+")
    identity = re.match(r"[A-Za-z0-9._-]+/[A-Za-z0-9._+-]+", agent)
    metadata = {
        "app_server_user_agent": identity.group(0) if identity else "withheld",
        "app_server_user_agent_suffix": "withheld" if " " in agent else "none",
        "codex_home_path": "validated_and_discarded",
    }
    for field, key in (
        ("platformFamily", "app_server_platform_family"),
        ("platformOs", "app_server_platform_os"),
    ):
        value = result[field]
        metadata[key] = value if FIELD_RE.fullmatch(value) else "withheld"
    return (
        "supported_audited_runtime" if supported else "unsupported_runtime"
    ), metadata


async def probe(program: str, client_version: str) -> int:
    try:
        process = await asyncio.create_subprocess_exec(
            program,
            "app-server",
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            limit=MAX_LINE_BYTES,
        )
    except FileNotFoundError:
        print("app_server_probe=UNVERIFIED")
        print("diagnostic=app_server_executable_missing")
        return 2
    except (OSError, ValueError):
        print("app_server_probe=UNVERIFIED")
        print("diagnostic=app_server_start_failed")
        return 2

    assert process.stdin is not None
    assert process.stdout is not None
    assert process.stderr is not None
    stderr_task = asyncio.create_task(drain(process.stderr))
    try:
        request = {
            "id": INITIALIZE_ID,
            "method": "initialize",
            "params": {
                "clientInfo": {
                    "name": "codex-meter",
                    "title": "Codex Meter",
                    "version": client_version,
                }
            },
        }
        process.stdin.write(
            (json.dumps(request, separators=(",", ":")) + "\n").encode()
        )
        await asyncio.wait_for(process.stdin.drain(), timeout=10)

        deadline = asyncio.get_running_loop().time() + 10
        response: dict[str, Any] | None = None
        while asyncio.get_running_loop().time() < deadline:
            remaining = deadline - asyncio.get_running_loop().time()
            try:
                line = await asyncio.wait_for(
                    process.stdout.readline(), timeout=remaining
                )
            except (asyncio.TimeoutError, asyncio.LimitOverrunError, ValueError):
                break
            if not line:
                break
            try:
                message = json.loads(line)
            except (UnicodeDecodeError, json.JSONDecodeError):
                continue
            if (
                isinstance(message, dict)
                and type(message.get("id")) is int
                and message["id"] == INITIALIZE_ID
            ):
                response = message
                break

        if response is None:
            print("app_server_probe=FAIL")
            print("diagnostic=initialization_transport_failed")
            return 1
        classification, metadata = classify_initialize(response)
        supported = classification == "supported_audited_runtime"
        if metadata:
            process.stdin.write(b'{"method":"initialized","params":{}}\n')
            await asyncio.wait_for(process.stdin.drain(), timeout=5)
        print("app_server_probe=" + ("PASS" if supported else "FAIL"))
        print("diagnostic=" + classification)
        for key, value in metadata.items():
            print(f"{key}={value}")
        print("audited_runtime_compatibility=" + ("PASS" if supported else "FAIL"))
        return 0 if supported else 1
    except (asyncio.TimeoutError, BrokenPipeError, OSError):
        print("app_server_probe=FAIL")
        print("diagnostic=Initialize exchange ended before a validated response")
        return 1
    finally:
        if not process.stdin.is_closing():
            process.stdin.close()
        if process.returncode is None:
            try:
                await asyncio.wait_for(process.wait(), timeout=5)
            except asyncio.TimeoutError:
                process.kill()
                await process.wait()
        if not stderr_task.done():
            stderr_task.cancel()
        await asyncio.gather(stderr_task, return_exceptions=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--program",
        help="Codex executable to probe; defaults to codex found on PATH",
    )
    args = parser.parse_args()
    program = args.program or shutil.which("codex")
    if not program:
        print("app_server_probe=UNVERIFIED")
        print("diagnostic=app_server_executable_missing")
        return 2
    manifest = (
        Path(__file__).resolve().parents[1] / "rust/crates/codex-meter/Cargo.toml"
    )
    try:
        manifest_text = manifest.read_text(encoding="utf-8")
        package_section = re.search(
            r"(?ms)^\[package\]\s*(.*?)(?=^\[|\Z)", manifest_text
        )
        version_field = (
            re.search(r'(?m)^\s*version\s*=\s*"([^"]+)"', package_section.group(1))
            if package_section
            else None
        )
        client_version = version_field.group(1) if version_field else None
    except OSError:
        print("app_server_probe=UNVERIFIED")
        print("diagnostic=Codex Meter client version could not be identified")
        return 2
    if not isinstance(client_version, str) or not FIELD_RE.fullmatch(client_version):
        print("app_server_probe=UNVERIFIED")
        print("diagnostic=Codex Meter client version is malformed")
        return 2
    return asyncio.run(probe(program, client_version))


if __name__ == "__main__":
    sys.exit(main())
