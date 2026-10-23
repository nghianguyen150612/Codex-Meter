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


def safe_field(value: Any) -> str | None:
    if not isinstance(value, str) or not FIELD_RE.fullmatch(value):
        return None
    return value


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
    except (OSError, ValueError):
        print("app_server_probe=UNVERIFIED")
        print("diagnostic=Codex App Server could not be started")
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
            if isinstance(message, dict) and message.get("id") == INITIALIZE_ID:
                response = message
                break

        if response is None or "result" not in response:
            print("app_server_probe=FAIL")
            print("diagnostic=No valid initialize response")
            return 1

        result = response["result"]
        if (
            not isinstance(result, dict)
            or not isinstance(result.get("codexHome"), str)
            or not result["codexHome"]
        ):
            print("app_server_probe=FAIL")
            print(
                "diagnostic=Initialize response did not satisfy Codex Meter requirements"
            )
            return 1

        user_agent = safe_field(result.get("userAgent"))
        platform_family = safe_field(result.get("platformFamily"))
        platform_os = safe_field(result.get("platformOs"))
        if not all((user_agent, platform_family, platform_os)):
            print("app_server_probe=FAIL")
            print("diagnostic=Initialize response metadata was missing or malformed")
            return 1

        process.stdin.write(b'{"method":"initialized"}\n')
        await asyncio.wait_for(process.stdin.drain(), timeout=5)
        print("app_server_probe=PASS")
        print(f"app_server_user_agent={user_agent}")
        print(f"app_server_platform_family={platform_family}")
        print(f"app_server_platform_os={platform_os}")
        print("codex_home_path=validated_and_discarded")
        return 0
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
        print("diagnostic=Codex executable was not found")
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
