"""Qualification probe metadata validation never exports private suffixes or paths."""

import importlib.util
from pathlib import Path

import pytest

spec = importlib.util.spec_from_file_location(
    "probe", Path(__file__).resolve().parents[2] / "scripts/p038a_probe_app_server.py"
)
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


def response(agent="codex_cli_rs/0.163.0"):
    return {
        "result": {
            "userAgent": agent,
            "codexHome": "/PRIVATE_HOME",
            "platformFamily": "unix",
            "platformOs": "linux",
        }
    }


@pytest.mark.parametrize(
    "agent,supported",
    [
        ("codex_cli_rs/0.163.0", True),
        ("codex_cli_rs/0.163.0 (Linux; PRIVATE_SUFFIX)", True),
        ("codex_cli_rs/0.163.0+build", True),
        ("codex_cli_rs/0.163.0-dev", False),
        ("codex_cli_rs/0.164.0 (Linux)", False),
        ("codex-meter/0.164.0 (Linux)", False),
    ],
)
def test_runtime_policy(agent, supported):
    status, metadata = probe.classify_initialize(response(agent))
    assert (status == "supported_audited_runtime") == supported
    assert "PRIVATE" not in str(metadata)


@pytest.mark.parametrize("field", ["userAgent", "codexHome", "platformFamily", "platformOs"])
@pytest.mark.parametrize("value", [None, "", 123, "bad\0value"])
def test_required_field_types(field, value):
    message = response()
    message["result"][field] = value
    assert probe.classify_initialize(message) == ("invalid_required_metadata_" + field, {})


def test_missing_and_invalid_response():
    assert probe.classify_initialize({"result": {}})[0] == "missing_required_metadata_codexHome"
    for value in [None, [], {"error": {}}, {"result": []}]:
        assert probe.classify_initialize(value) == ("invalid_initialize_response", {})


@pytest.mark.parametrize(
    "mode,expected",
    [
        ("supported", 0),
        ("unsupported", 1),
        ("invalid", 1),
        ("eof", 1),
    ],
)
def test_probe_transport(tmp_path, monkeypatch, capsys, mode, expected):
    import asyncio
    import sys

    fixture = tmp_path / "server.py"
    fixture.write_text(
        "import sys,json\n"
        "request=json.loads(sys.stdin.readline())\n"
        "assert request['method']=='initialize'\n"
        + (
            "sys.exit(0)\n"
            if mode == "eof"
            else "print("
            + repr(
                __import__("json").dumps(
                    {
                        "id": 1,
                        **(
                            response("codex_cli_rs/0.163.0 (PRIVATE_SUFFIX)")
                            if mode == "supported"
                            else response("codex-meter/0.164.0 (PRIVATE_SUFFIX)")
                            if mode == "unsupported"
                            else {"result": []}
                        ),
                    }
                )
            )
            + ",flush=True)\n"
            + (
                "assert json.loads(sys.stdin.readline())['method']=='initialized'\n"
                if mode in ("supported", "unsupported")
                else ""
            )
        ),
        encoding="utf-8",
    )
    original = asyncio.create_subprocess_exec

    async def start(*args, **kwargs):
        return await original(sys.executable, str(fixture), **kwargs)

    monkeypatch.setattr(asyncio, "create_subprocess_exec", start)
    assert asyncio.run(probe.probe("fixture", "0.1.0")) == expected
    output = capsys.readouterr().out
    assert "PRIVATE" not in output
    assert str(tmp_path) not in output
    assert "app_server_probe=" in output


def test_missing_executable(capsys):
    import asyncio

    assert asyncio.run(probe.probe("/nonexistent-p038a-executable", "0.1.0")) == 2
    assert "app_server_executable_missing" in capsys.readouterr().out
