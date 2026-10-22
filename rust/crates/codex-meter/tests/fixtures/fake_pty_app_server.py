"""Stateful synthetic App Server used by Unix PTY subprocess tests only."""

import json
import os
import sys
import time


mode = sys.argv[1]
workspace = sys.argv[2]
state_path = os.environ["CODEX_METER_TEST_STATE"]
request_log = os.environ["CODEX_METER_TEST_REQUEST_LOG"]
continue_path = os.environ["CODEX_METER_TEST_CONTINUE"]
expected_prompt = os.environ.get("CODEX_METER_TEST_PROMPT", "P037_λ_SYNTHETIC_PROMPT")


def read_message():
    line = sys.stdin.buffer.readline()
    if not line:
        return None
    message = json.loads(line)
    if "jsonrpc" in message:
        raise SystemExit(21)
    return message


def send(message):
    sys.stdout.write(json.dumps(message, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def log(method):
    with open(request_log, "a", encoding="utf-8") as output:
        output.write(method + "\n")
        output.flush()


def load_state():
    try:
        with open(state_path, encoding="utf-8") as source:
            return json.load(source)
    except FileNotFoundError:
        return {"threadId": "thread-pty", "turnId": None, "status": "idle"}


def save_state(state):
    temporary = state_path + ".tmp"
    with open(temporary, "w", encoding="utf-8") as output:
        json.dump(state, output, separators=(",", ":"))
        output.flush()
        os.fsync(output.fileno())
    os.replace(temporary, state_path)


def wait_for(path):
    deadline = time.monotonic() + 20
    while not os.path.exists(path) and time.monotonic() < deadline:
        time.sleep(0.01)
    return os.path.exists(path)


def model():
    return {
        "id": "fixture-model",
        "model": "fixture-model",
        "displayName": "Fixture model",
        "description": "Synthetic test model",
        "hidden": False,
        "isDefault": True,
        "supportedReasoningEfforts": [{"reasoningEffort": "medium", "description": "Balanced"}],
        "defaultReasoningEffort": "medium",
        "inputModalities": ["text"],
    }


def thread_status(state):
    return "active" if state.get("status") == "inProgress" else "idle"


def thread_record(state):
    return {
        "id": state["threadId"],
        "cwd": workspace,
        "model": "fixture-model",
        "status": {"type": thread_status(state)},
    }


while True:
    message = read_message()
    if message is None:
        raise SystemExit(0)
    method = message.get("method")
    if method is None:
        raise SystemExit(22)
    log(method)

    if method == "initialize":
        send({"id": message["id"], "result": {
            "userAgent": "codex_cli_rs/0.163.0",
            "codexHome": "/test-only/not-retained",
            "platformFamily": "unix",
            "platformOs": "linux",
        }})
    elif method == "initialized":
        continue
    elif method == "model/list":
        send({"id": message["id"], "result": {"data": [model()], "nextCursor": None}})
    elif method == "config/read":
        if message.get("params") != {"cwd": workspace, "includeLayers": False}:
            raise SystemExit(23)
        send({"id": message["id"], "result": {"config": {
            "model": "fixture-model",
            "model_reasoning_effort": "medium",
            "approval_policy": "on-request",
            "sandbox_mode": "workspace-write",
            "sandbox_workspace_write": {"network_access": False, "writable_roots": []},
        }, "origins": {}}})
    elif method == "thread/list":
        send({"id": message["id"], "result": {"data": [], "nextCursor": None}})
    elif method == "thread/loaded/list":
        send({"id": message["id"], "result": {"data": [], "nextCursor": None}})
    elif method == "thread/start":
        params = message.get("params", {})
        if params != {
            "cwd": workspace,
            "sandbox": "workspace-write",
            "approvalPolicy": "on-request",
            "approvalsReviewer": "user",
            "config": {"sandbox_workspace_write": {"network_access": False}},
        }:
            raise SystemExit(24)
        state = {"threadId": "thread-pty", "turnId": None, "status": "idle"}
        save_state(state)
        if mode == "thread_start_crash":
            while True:
                time.sleep(0.25)
        send({"id": message["id"], "result": {
            "thread": {"id": "thread-pty", "cwd": workspace, "model": "fixture-model", "status": {"type": "idle"}},
            "cwd": workspace,
            "model": "fixture-model",
            "approvalPolicy": "on-request",
            "approvalsReviewer": "user",
            "sandbox": {"type": "workspaceWrite", "networkAccess": False, "writableRoots": []},
        }})
    elif method == "turn/start":
        params = message.get("params", {})
        if params.get("threadId") != "thread-pty" or params.get("input") != [{"type": "text", "text": expected_prompt}]:
            raise SystemExit(25)
        state = {"threadId": "thread-pty", "turnId": "turn-pty", "status": "inProgress"}
        save_state(state)
        if mode == "turn_start_crash":
            while True:
                time.sleep(0.25)
        send({"id": message["id"], "result": {"turn": {"id": "turn-pty", "status": "inProgress", "items": []}}})
        send({"method": "turn/started", "params": {"threadId": "thread-pty", "turn": {"id": "turn-pty", "status": "inProgress", "items": []}}})
        if mode == "success":
            if not wait_for(continue_path):
                raise SystemExit(26)
            state["status"] = "completed"
            save_state(state)
            send({"method": "turn/completed", "params": {"threadId": "thread-pty", "turn": {"id": "turn-pty", "status": "completed", "items": []}}})
        elif mode in ("approval_allow", "approval_deny"):
            send({"id": "approval-pty", "method": "item/commandExecution/requestApproval", "params": {
                "threadId": "thread-pty", "turnId": "turn-pty", "itemId": "item-pty",
                "startedAtMs": 1, "cwd": workspace, "command": "printf synthetic",
            }})
            decision = read_message()
            expected_decision = {"decision": "accept" if mode == "approval_allow" else "decline"}
            if decision is None or decision.get("id") != "approval-pty" or decision.get("result") != expected_decision:
                raise SystemExit(27)
            with open(state_path + ".approval", "w", encoding="utf-8") as output:
                json.dump(expected_decision, output)
            send({"method": "serverRequest/resolved", "params": {"threadId": "thread-pty", "requestId": "approval-pty"}})
            state["status"] = "completed"
            save_state(state)
            send({"method": "turn/completed", "params": {"threadId": "thread-pty", "turn": {"id": "turn-pty", "status": "completed", "items": []}}})
        elif mode == "interrupt":
            while True:
                followup = read_message()
                if followup is None:
                    raise SystemExit(0)
                log(followup.get("method", ""))
                if followup.get("method") == "turn/interrupt":
                    if followup.get("params") != {"threadId": "thread-pty", "turnId": "turn-pty"}:
                        raise SystemExit(28)
                    send({"id": followup["id"], "result": {}})
                    if not wait_for(continue_path):
                        raise SystemExit(29)
                    state["status"] = "interrupted"
                    save_state(state)
                    send({"method": "turn/completed", "params": {"threadId": "thread-pty", "turn": {"id": "turn-pty", "status": "interrupted", "items": []}}})
                    break
        elif mode == "connection_loss":
            sys.stdout.flush()
            raise SystemExit(0)
        elif mode in ("crash", "recovery"):
            while True:
                time.sleep(0.25)
        else:
            state["status"] = "completed"
            save_state(state)
            send({"method": "turn/completed", "params": {"threadId": "thread-pty", "turn": {"id": "turn-pty", "status": "completed", "items": []}}})
    elif method == "turn/interrupt":
        if message.get("params") != {"threadId": "thread-pty", "turnId": "turn-pty"}:
            raise SystemExit(30)
        send({"id": message["id"], "result": {}})
        if mode in ("interrupt", "connection_loss") and wait_for(continue_path):
            state = load_state()
            state["status"] = "interrupted"
            save_state(state)
            send({"method": "turn/completed", "params": {"threadId": "thread-pty", "turn": {"id": "turn-pty", "status": "interrupted", "items": []}}})
    elif method == "thread/read":
        state = load_state()
        if message.get("params") != {"threadId": state["threadId"], "includeTurns": False}:
            raise SystemExit(31)
        send({"id": message["id"], "result": {"thread": thread_record(state)}})
    elif method == "thread/turns/list":
        state = load_state()
        params = message.get("params", {})
        if params.get("threadId") != state["threadId"]:
            raise SystemExit(32)
        turns = []
        if state.get("turnId"):
            turns.append({"id": state["turnId"], "status": state["status"], "items": []})
        send({"id": message["id"], "result": {"data": turns, "nextCursor": None}})
    else:
        send({"id": message["id"], "error": {"code": -32601, "message": "unsupported"}})
