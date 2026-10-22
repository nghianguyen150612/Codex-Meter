"""Small JSONL App Server used only by the native Rust transport tests."""

import json
import os
import sys
import time


mode = sys.argv[1]
execution_workspace = sys.argv[2] if len(sys.argv) > 2 else None
initialized = False
config_read_count = 0
thread_read_count = 0
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
                    "userAgent": (
                        "codex_cli_rs/0.162.0"
                        if mode == "execution_unsupported_version"
                        else "codex_cli_rs/0.163.0"
                        if mode.startswith("execution_")
                        else "codex_cli_rs/test"
                    ),
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
    elif mode in ("interactive_request", "interactive_notification_flood", "interactive_oversized_event", "server_request") and method == "thread/read":
        if message.get("params") != {"threadId": "thread-test", "includeTurns": False}:
            raise SystemExit(50)
        if mode == "interactive_request":
            send({"method": "turn/started", "params": {"threadId": "thread-test", "turn": {"id": "turn-test", "status": "inProgress", "items": []}}})
            send({"id": "approval-test", "method": "item/commandExecution/requestApproval", "params": {
                "threadId": "thread-test", "turnId": "turn-test", "itemId": "item-test",
                "startedAtMs": 1, "command": "printf safe", "cwd": "/tmp"
            }})
            approval = read_message()
            accepted = approval is not None and approval.get("id") == "approval-test" and approval.get("result") == {"decision": "accept"}
            send({"id": message["id"], "result": {"approved": accepted}})
        elif mode == "interactive_notification_flood":
            for _ in range(256):
                send({"method": "turn/completed", "params": {"threadId": "thread-test", "turn": {"id": "turn-test", "status": "completed", "items": []}}})
            send({"id": message["id"], "result": {"ok": True}})
        elif mode == "interactive_oversized_event":
            send({"method": "turn/completed", "params": {"threadId": "thread-test", "turn": {"id": "turn-test", "status": "completed", "items": []}, "padding": "x" * (128 * 1024)}})
            send({"id": message["id"], "result": {"ok": True}})
        else:
            send({"id": "unknown-server-request", "method": "item/unknown/requestApproval", "params": {"sensitive": "ignored"}})
            response = read_message()
            send({"id": message["id"], "result": {"approved": "error" not in response, "code": response.get("error", {}).get("code")}})
    elif mode.startswith("execution_") and method == "model/list":
        params = message.get("params", {})
        if params != {"limit": 100, "includeHidden": True}:
            raise SystemExit(40)
        send({"id": message["id"], "result": {"data": [model("fixture-model", default=True)], "nextCursor": None}})
    elif mode.startswith("execution_") and method == "config/read":
        config_read_count += 1
        params = message.get("params", {})
        if params != {"cwd": execution_workspace, "includeLayers": False}:
            raise SystemExit(41)
        config = {
            "approval_policy": "on-request",
            "sandbox_mode": "workspace-write",
            "sandbox_workspace_write": {"network_access": False, "writable_roots": []},
        }
        if mode == "execution_new_model_changed":
            config["model"] = "fixture-model" if config_read_count == 1 else "changed-model"
            config["model_reasoning_effort"] = "medium"
        send({"id": message["id"], "result": {"config": config, "origins": {}}})
    elif mode.startswith("execution_") and method == "thread/read":
        thread_read_count += 1
        params = message.get("params", {})
        if params != {"threadId": "thread-existing", "includeTurns": False}:
            raise SystemExit(42)
        status = "active" if mode == "execution_active_thread" else "idle"
        send({"id": message["id"], "result": {"thread": {
            "id": "thread-existing",
            "cwd": execution_workspace,
            "model": "changed-model" if mode == "execution_existing_model_changed" and thread_read_count >= 2 else "fixture-model",
            "status": {"type": status},
        }}})
    elif mode.startswith("execution_") and method == "thread/resume":
        params = message.get("params", {})
        expected_thread_id = "thread-created" if mode == "execution_reconcile" else "thread-existing"
        if params != {"threadId": expected_thread_id, "excludeTurns": True}:
            raise SystemExit(43)
        if mode == "execution_reconcile":
            send({"id": message["id"], "result": {
                "thread": {"id": "thread-created", "cwd": execution_workspace, "model": "fixture-model", "status": {"type": "idle"}},
                "cwd": execution_workspace,
                "model": "fixture-model",
                "approvalPolicy": "on-request",
                "approvalsReviewer": "user",
                "sandbox": {"type": "workspaceWrite", "networkAccess": False, "writableRoots": []},
            }})
            continue
        resumed_model = "changed-model" if mode == "execution_existing_model_changed" and thread_read_count >= 2 else "fixture-model"
        send({"id": message["id"], "result": {
            "thread": {"id": "thread-existing", "cwd": execution_workspace, "model": resumed_model, "status": {"type": "idle"}},
            "cwd": execution_workspace,
            "model": "fixture-model",
            "approvalPolicy": "on-request",
            "approvalsReviewer": "user",
            "sandbox": {"type": "workspaceWrite", "networkAccess": False, "writableRoots": []},
        }})
    elif mode.startswith("execution_") and method == "thread/start":
        params = message.get("params", {})
        expected = {
            "cwd": execution_workspace,
            "sandbox": "workspace-write",
            "approvalPolicy": "on-request",
            "approvalsReviewer": "user",
            "config": {"sandbox_workspace_write": {"network_access": False}},
        }
        if params != expected:
            raise SystemExit(44)
        if mode == "execution_lost_thread_start":
            raise SystemExit(45)
        send({"id": message["id"], "result": {
            "thread": {"id": "thread-created", "cwd": execution_workspace, "model": "fixture-model", "status": {"type": "idle"}},
            "cwd": execution_workspace,
            "model": "fixture-model",
            "approvalPolicy": "on-request",
            "approvalsReviewer": "user",
            "sandbox": {"type": "workspaceWrite", "networkAccess": False, "writableRoots": []},
        }})
    elif mode.startswith("execution_") and method == "turn/start":
        params = message.get("params", {})
        if params not in (
            {"threadId": "thread-created", "input": [{"type": "text", "text": "P036_SYNTHETIC_PROMPT"}]},
            {"threadId": "thread-existing", "input": [{"type": "text", "text": "P036_SYNTHETIC_PROMPT"}]},
        ):
            raise SystemExit(46)
        if mode == "execution_lost_turn_start":
            raise SystemExit(47)
        if mode == "execution_reconcile":
            send({"id": message["id"], "result": {"turn": {"id": "turn-p036", "status": "inProgress", "items": []}}})
            raise SystemExit(53)
        if mode == "execution_terminal_response":
            send({"id": message["id"], "result": {"turn": {
                "id": "turn-p036", "status": "completed", "items": [
                    {"id": "reasoning-item", "type": "reasoning", "text": "INTERNAL_REASONING_SENTINEL"},
                    {"id": "final-item", "type": "agentMessage", "text": "Authoritative final response"},
                ]
            }}})
            continue
        send({"id": message["id"], "result": {"turn": {"id": "turn-p036", "status": "inProgress", "items": []}}})
        if mode == "execution_interrupt":
            send({"method": "turn/started", "params": {"threadId": params["threadId"], "turn": {"id": "turn-p036", "status": "inProgress", "items": []}}})
            continue
        send({"method": "turn/started", "params": {"threadId": params["threadId"], "turn": {"id": "turn-p036", "status": "inProgress", "items": []}}})
        if mode in ("execution_approval", "execution_approval_interrupt"):
            send({"id": "approval-p036", "method": "item/commandExecution/requestApproval", "params": {
                "threadId": params["threadId"], "turnId": "turn-p036", "itemId": "item-command",
                "startedAtMs": 1, "cwd": execution_workspace, "command": "cargo test",
            }})
            approval = read_message()
            expected = {"decision": "decline"} if mode == "execution_approval_interrupt" else {"decision": "accept"}
            if approval is None or approval.get("id") != "approval-p036" or approval.get("result") != expected:
                raise SystemExit(48)
            send({"method": "serverRequest/resolved", "params": {"threadId": params["threadId"], "requestId": "approval-p036"}})
            if mode == "execution_approval":
                send({"method": "turn/completed", "params": {"threadId": params["threadId"], "turn": {"id": "turn-p036", "status": "completed", "items": []}}})
        elif mode in ("execution_file_approval", "execution_permission_approval"):
            send({"method": "turn/diff/updated", "params": {"threadId": params["threadId"], "turnId": "turn-p036", "diff": "--- a/generated.txt\n+++ b/generated.txt\n+approved content"}})
            if mode == "execution_file_approval":
                send({"id": "approval-p036", "method": "item/fileChange/requestApproval", "params": {
                    "threadId": params["threadId"], "turnId": "turn-p036", "itemId": "item-file",
                    "startedAtMs": 1, "reason": "Write generated.txt",
                }})
                approval = read_message()
                expected = {"decision": "accept"}
            else:
                permissions = {"fileSystem": {"entries": [{"access": "write", "path": {"type": "path", "path": "/repo/generated"}}]}}
                send({"id": "approval-p036", "method": "item/permissions/requestApproval", "params": {
                    "threadId": params["threadId"], "turnId": "turn-p036", "itemId": "item-permission",
                    "startedAtMs": 1, "cwd": "/repo", "permissions": permissions,
                }})
                approval = read_message()
                expected = {"permissions": permissions, "scope": "turn"}
            if approval is None or approval.get("id") != "approval-p036" or approval.get("result") != expected:
                raise SystemExit(51)
            send({"method": "serverRequest/resolved", "params": {"threadId": params["threadId"], "requestId": "approval-p036"}})
            send({"method": "turn/completed", "params": {"threadId": params["threadId"], "turn": {"id": "turn-p036", "status": "completed", "items": []}}})
        elif mode == "execution_missing_output":
            send({"method": "turn/completed", "params": {"threadId": params["threadId"], "turn": {"id": "turn-p036", "status": "completed", "items": []}}})
        else:
            send({"method": "item/agentMessage/delta", "params": {"threadId": params["threadId"], "turnId": "turn-p036", "itemId": "item-agent", "delta": "Streaming response"}})
            send({"method": "item/completed", "params": {"threadId": params["threadId"], "turnId": "turn-p036", "completedAtMs": 2, "item": {"id": "item-agent", "type": "agentMessage", "text": "Final response from fake Codex."}}})
            send({"method": "turn/completed", "params": {"threadId": params["threadId"], "turn": {"id": "turn-p036", "status": "completed", "items": []}}})
    elif mode in ("execution_interrupt", "execution_approval_interrupt") and method == "turn/interrupt":
        if message.get("params") != {"threadId": "thread-created", "turnId": "turn-p036"}:
            raise SystemExit(49)
        send({"id": message["id"], "result": {}})
        send({"method": "turn/completed", "params": {"threadId": "thread-created", "turn": {"id": "turn-p036", "status": "interrupted", "items": []}}})
    elif mode == "execution_reconcile" and method == "thread/turns/list":
        expected = {"threadId": "thread-created", "limit": 50, "itemsView": "notLoaded", "sortDirection": "desc"}
        if message.get("params") != expected:
            raise SystemExit(54)
        send({"id": message["id"], "result": {"data": [
            {"id": "turn-p036", "status": "completed", "items": []}
        ], "nextCursor": None}})
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
