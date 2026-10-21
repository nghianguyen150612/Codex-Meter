"""Small JSONL App Server used only by the native Rust transport tests."""

import json
import os
import sys
import time


mode = sys.argv[1]
initialized = False
if mode == "startup_failure":
    raise SystemExit(3)


def read_message():
    line = sys.stdin.buffer.readline()
    if not line:
        return None
    message = json.loads(line)
    if "jsonrpc" in message:
        raise SystemExit(11)
    return message


def send(message):
    sys.stdout.write(json.dumps(message, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def thread(thread_id, status="idle", created_at=100, archived=False, forked_from=None):
    return {
        "id": thread_id,
        "sessionId": "session-tree-root",
        "name": "Fixture session",
        "createdAt": created_at,
        "updatedAt": created_at + 10,
        "cwd": "/private/path/that/must/not/escape",
        "ephemeral": False,
        "cliVersion": "0.159.2",
        "model": "fixture-model",
        "modelProvider": "fixture-provider",
        "reasoningEffort": "medium",
        "preview": "SENSITIVE_PROMPT_SENTINEL",
        "projectId": "opaque-project-id",
        "forkedFromId": forked_from,
        "parentThreadId": None,
        "source": "cli",
        "status": {"type": status},
        "turns": [],
        "archived": archived,
    }


def model(model_id, hidden=False, default=False, modalities=None):
    return {
        "id": model_id,
        "model": model_id,
        "displayName": "Fixture " + model_id,
        "description": "not displayed",
        "hidden": hidden,
        "isDefault": default,
        "supportedReasoningEfforts": [
            {"reasoningEffort": "low", "description": "Lower effort"},
            {"reasoningEffort": "medium", "description": "Balanced"},
        ],
        "defaultReasoningEffort": "medium",
        "inputModalities": modalities,
    }


while True:
    message = read_message()
    if message is None:
        raise SystemExit(0)
    method = message.get("method")
    if method == "initialize":
        if mode == "unsupported_initialize":
            send({"id": message["id"], "error": {"code": -32601, "message": "unsupported"}})
            continue
        if mode == "initialization_failure":
            send({"id": message["id"], "error": {"code": -32000, "message": "initialization failed"}})
            continue
        if mode == "authentication_unavailable":
            send({"id": message["id"], "error": {"code": -32001, "message": "request denied", "data": {"codexErrorInfo": "unauthorized"}}})
            continue
        client = message.get("params", {}).get("clientInfo", {})
        if client.get("name") != "codex-meter":
            raise SystemExit(12)
        result = {
                "id": message["id"],
                "result": {
                    "userAgent": "codex_cli_rs/test",
                    "codexHome": "/not-retained",
                    "platformFamily": "unix",
                    "platformOs": "linux",
                },
            }
        if mode == "initialization_malformed":
            result["result"].pop("platformOs")
        send(result)
    elif method == "initialized":
        if "params" in message:
            raise SystemExit(13)
        initialized = True
        if mode == "no_read":
            time.sleep(30)
    elif not initialized:
        raise SystemExit(14)
    elif method in ("thread/start", "turn/start", "turn/steer"):
        # Preparation-only tests must fail loudly if a thread or model turn is started.
        raise SystemExit(30)
    elif mode == "unsupported":
        send({"id": message["id"], "error": {"code": -32601, "message": "unsupported"}})
    elif mode == "remote_sensitive":
        send({"id": message["id"], "error": {"code": -32000, "message": "SENSITIVE_ERROR_SENTINEL /private/path"}})
    elif mode in ("sessions", "unknown_thread", "loaded_unsupported", "empty_sessions", "unknown_status") and method == "thread/list":
        params = message.get("params", {})
        if params.get("useStateDbOnly") is not True or "sourceKinds" not in params:
            raise SystemExit(18)
        if mode == "empty_sessions":
            data = []
            cursor = None
        elif params.get("archived") is True:
            data = [thread("thread-archived", created_at=105, archived=True)]
            cursor = None
        elif params.get("cursor") is None:
            status = "futureStatus" if mode == "unknown_status" else "active"
            data = [thread("thread-active", status, 100)]
            cursor = "sessions-page-2"
        else:
            if params["cursor"] != "sessions-page-2":
                raise SystemExit(19)
            data = [thread("thread-idle", "idle", 90)]
            cursor = None
        send({"id": message["id"], "result": {"data": data, "nextCursor": cursor}})
    elif mode == "loaded_unsupported" and method == "thread/loaded/list":
        send({"id": message["id"], "error": {"code": -32601, "message": "unsupported"}})
    elif mode in ("sessions", "unknown_thread") and method == "thread/loaded/list":
        send({"id": message["id"], "result": {"data": ["thread-active"], "nextCursor": None}})
    elif mode == "unknown_thread" and method == "thread/read":
        send({"id": message["id"], "error": {"code": -32004, "message": "thread not found"}})
    elif mode == "sessions" and method == "thread/read":
        params = message.get("params", {})
        if params.get("includeTurns") is not False:
            raise SystemExit(20)
        send({"id": message["id"], "result": {"thread": thread(params["threadId"])}})
    elif mode == "sessions" and method == "thread/resume":
        params = message.get("params", {})
        if set(params) != {"threadId", "excludeTurns"} or params["excludeTurns"] is not True:
            raise SystemExit(21)
        send({"id": message["id"], "result": {"thread": thread(params["threadId"])}})
    elif mode == "sessions" and method == "thread/fork":
        params = message.get("params", {})
        if set(params) != {"threadId", "excludeTurns"} or params["excludeTurns"] is not True:
            raise SystemExit(22)
        send({"id": message["id"], "result": {"thread": thread("thread-forked", forked_from=params["threadId"])}})
    elif mode == "sessions" and method == "thread/turns/list":
        params = message.get("params", {})
        if params.get("itemsView") != "notLoaded":
            raise SystemExit(23)
        if params.get("cursor") is None:
            data = [{"id": "turn-1", "status": "completed", "startedAt": 100, "completedAt": 101, "items": []}]
            cursor = "turns-page-2"
        else:
            if params["cursor"] != "turns-page-2":
                raise SystemExit(24)
            data = [{"id": "turn-2", "status": "interrupted", "startedAt": 110, "completedAt": 111, "items": []}]
            cursor = None
        send({"id": message["id"], "result": {"data": data, "nextCursor": cursor}})
    elif mode in ("models", "empty_models", "models_optional") and method == "model/list":
        params = message.get("params", {})
        if params.get("includeHidden") is not True:
            raise SystemExit(25)
        if mode == "empty_models":
            data = []
            cursor = None
        elif mode == "models_optional":
            data = [{
                "id": "model-minimal",
                "displayName": "Minimal model",
                "hidden": False,
                "isDefault": False,
                "supportedReasoningEfforts": [],
                "defaultReasoningEffort": "medium",
            }]
            cursor = None
        elif params.get("cursor") is None:
            data = [model("model-visible", default=True, modalities=["text", "image"])]
            cursor = "models-page-2"
        else:
            if params["cursor"] != "models-page-2":
                raise SystemExit(26)
            data = [model("model-hidden", hidden=True, modalities=None)]
            cursor = None
        send({"id": message["id"], "result": {"data": data, "nextCursor": cursor}})
    elif method == "probe":
        if mode == "exit":
            raise SystemExit(17)
        if mode == "broken_pipe":
            os.close(0)
            send({"method": "ready-to-break"})
            time.sleep(2)
            continue
        elif mode == "server_shutdown":
            os.close(1)
            time.sleep(2)
            raise SystemExit(0)
        if mode == "timeout":
            time.sleep(2)
        elif mode == "malformed":
            sys.stdout.write("{malformed\n")
            sys.stdout.flush()
            raise SystemExit(0)
        elif mode == "oversized":
            sys.stdout.write("x" * (4 * 1024 * 1024 + 2) + "\n")
            sys.stdout.flush()
            raise SystemExit(0)
        elif mode == "notification":
            send({"method": "thread/status/changed", "params": {"sensitive": "ignored"}})
            send({"id": message["id"], "result": {"ok": True}})
        elif mode == "notification_flood":
            for _ in range(256):
                send({"method": "thread/status/changed", "params": {"sensitive": "ignored"}})
            send({"id": message["id"], "result": {"ok": True}})
        elif mode == "server_request":
            send({"id": "server-request-1", "method": "commandExecution/requestApproval", "params": {"sensitive": "ignored"}})
            response = read_message()
            send({"id": message["id"], "result": {"approved": "error" not in response, "code": response.get("error", {}).get("code")}})
        elif mode == "stderr_flood":
            sys.stderr.write("x" * (1024 * 1024))
            sys.stderr.flush()
            send({"id": message["id"], "result": {"ok": True}})
        elif mode == "large_echo":
            payload = message.get("params", {}).get("payload", "")
            send({"id": message["id"], "result": {"length": len(payload)}})
        else:
            send({"id": message["id"], "result": {"ok": True}})
    elif mode == "concurrent" and method in ("first", "second"):
        other = read_message()
        if other is None:
            raise SystemExit(15)
        send({"id": other["id"], "result": {"method": other["method"]}})
        send({"id": message["id"], "result": {"method": method}})
    else:
        send({"id": message["id"], "error": {"code": -32601, "message": "unsupported"}})
