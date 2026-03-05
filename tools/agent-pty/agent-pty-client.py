#!/usr/bin/env python3
"""Local PTY client for pika-cli agent mode.

Spawns a local marmotd daemon, invites the remote bot into a data call over MoQ,
then forwards local terminal input/output over encrypted call data frames.
"""

from __future__ import annotations

import base64
from collections import deque
import json
import os
import queue
import select
import signal
import subprocess
import sys
import termios
import threading
import tty
import uuid
from shutil import get_terminal_size
from typing import Any


def required_env(name: str) -> str:
    value = os.environ.get(name, "").strip()
    if not value:
        raise RuntimeError(f"missing required env var {name}")
    return value


def parse_json_list(name: str) -> list[str]:
    raw = os.environ.get(name, "").strip()
    if not raw:
        return []
    try:
        value = json.loads(raw)
        if not isinstance(value, list):
            return []
        return [str(item).strip() for item in value if str(item).strip()]
    except Exception:
        return []


def env_bool(name: str) -> bool:
    value = os.environ.get(name, "").strip().lower()
    return value in {"1", "true", "yes", "on"}


def env_float(name: str, default: float) -> float:
    raw = os.environ.get(name, "").strip()
    if not raw:
        return default
    try:
        value = float(raw)
        if value > 0:
            return value
    except Exception:
        pass
    return default


MARMOTD_BIN = required_env("PIKA_AGENT_MARMOTD_BIN")
STATE_DIR = required_env("PIKA_AGENT_STATE_DIR")
GROUP_ID = required_env("PIKA_AGENT_GROUP_ID")
BOT_PUBKEY = required_env("PIKA_AGENT_BOT_PUBKEY").lower()
RELAYS = parse_json_list("PIKA_AGENT_RELAYS_JSON")
MOQ_URLS = parse_json_list("PIKA_AGENT_MOQ_URLS_JSON")

if not MOQ_URLS:
    MOQ_URLS = [
        "https://us-east.moq.pikachat.org/anon",
        "https://eu.moq.pikachat.org/anon",
    ]

TEST_MODE = env_bool("PIKA_AGENT_TEST_MODE")
TEST_TIMEOUT_SEC = env_float("PIKA_AGENT_TEST_TIMEOUT_SEC", 25.0)
CAPTURE_STDOUT_PATH = os.environ.get("PIKA_AGENT_CAPTURE_STDOUT_PATH", "").strip()
EXPECT_REPLAY_FILE = os.environ.get("PIKA_AGENT_EXPECT_REPLAY_FILE", "").strip()
try:
    MAX_PREFIX_DROP_BYTES = int(
        os.environ.get("PIKA_AGENT_MAX_PREFIX_DROP_BYTES", "32") or "32"
    )
except Exception:
    MAX_PREFIX_DROP_BYTES = 32
try:
    MAX_SUFFIX_DROP_BYTES = int(
        os.environ.get("PIKA_AGENT_MAX_SUFFIX_DROP_BYTES", "32") or "32"
    )
except Exception:
    MAX_SUFFIX_DROP_BYTES = 32

STREAM_DAEMON_LOGS = env_bool("PIKA_AGENT_STREAM_DAEMON_LOGS")

WRITE_LOCK = threading.Lock()
EVENTS: "queue.Queue[dict[str, Any]]" = queue.Queue()
DEFERRED_EVENTS: "deque[dict[str, Any]]" = deque()
STOP_EVENT = threading.Event()
RESIZE_PENDING = threading.Event()

daemon: subprocess.Popen[str] | None = None
daemon_stderr_thread: threading.Thread | None = None


def tty_print(text: str) -> None:
    sys.stdout.write(text)
    sys.stdout.flush()


def send_cmd(cmd: dict[str, Any]) -> None:
    global daemon
    if daemon is None or daemon.stdin is None:
        return
    line = json.dumps(cmd, separators=(",", ":")) + "\n"
    with WRITE_LOCK:
        daemon.stdin.write(line)
        daemon.stdin.flush()


def encode_payload_hex(obj: dict[str, Any]) -> str:
    return json.dumps(obj, separators=(",", ":")).encode("utf-8").hex()


def send_call_payload(call_id: str, payload_obj: dict[str, Any]) -> None:
    send_cmd(
        {
            "cmd": "send_call_data",
            "call_id": call_id,
            "payload_hex": encode_payload_hex(payload_obj),
        }
    )


def daemon_reader(proc: subprocess.Popen[str]) -> None:
    assert proc.stdout is not None
    for line in proc.stdout:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except Exception:
            continue
        EVENTS.put(msg)
    STOP_EVENT.set()


def daemon_stderr_log_path() -> str:
    configured = os.environ.get("PIKA_AGENT_DAEMON_LOG_PATH", "").strip()
    if configured:
        return configured
    return os.path.join(STATE_DIR, "agent-daemon.log")


def daemon_stderr_reader(proc: subprocess.Popen[str]) -> None:
    assert proc.stderr is not None
    log_path = daemon_stderr_log_path()
    os.makedirs(os.path.dirname(log_path), exist_ok=True)
    with open(log_path, "a", encoding="utf-8") as logf:
        for line in proc.stderr:
            line = line.rstrip("\n")
            if not line:
                continue
            logf.write(line + "\n")
            logf.flush()
            if STREAM_DAEMON_LOGS:
                sys.stderr.write(line + "\n")
                sys.stderr.flush()


def spawn_daemon() -> subprocess.Popen[str]:
    env = os.environ.copy()
    env.setdefault("RUST_LOG", "error")
    cmd = [MARMOTD_BIN, "daemon", "--state-dir", STATE_DIR, "--allow-pubkey", BOT_PUBKEY]
    for relay in RELAYS:
        cmd.extend(["--relay", relay])
    proc = subprocess.Popen(
        cmd,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
        text=True,
        bufsize=1,
    )
    return proc


def decode_call_payload(msg: dict[str, Any]) -> dict[str, Any] | None:
    payload_hex = str(msg.get("payload_hex", "")).strip()
    if not payload_hex:
        return None
    try:
        raw = bytes.fromhex(payload_hex)
        payload = json.loads(raw.decode("utf-8"))
        if isinstance(payload, dict):
            return payload
    except Exception:
        return None
    return None


def load_expected_bytes_from_replay(path: str) -> bytes:
    with open(path, "r", encoding="utf-8") as fh:
        doc = json.load(fh)
    frames_raw = doc.get("frames")
    if not isinstance(frames_raw, list):
        raise RuntimeError("replay fixture missing frames array")
    out = bytearray()
    for idx, frame in enumerate(frames_raw):
        if not isinstance(frame, dict):
            raise RuntimeError(f"replay frame {idx} is not an object")
        data_b64 = str(frame.get("stdout_b64", "")).strip()
        if not data_b64:
            raise RuntimeError(f"replay frame {idx} missing stdout_b64")
        out.extend(base64.b64decode(data_b64))
    return bytes(out)


def decode_stdout_data(payload: dict[str, Any]) -> bytes:
    data_b64 = str(payload.get("d", ""))
    if not data_b64:
        return b""
    return base64.b64decode(data_b64)


class StdoutReorderBuffer:
    """Best-effort in-order stdout reconstruction for sequenced frames."""

    def __init__(self, allow_gap_recovery: bool = True, gap_timeout_sec: float = 0.35) -> None:
        self.next_seq: int | None = None
        self.pending: dict[int, bytes] = {}
        self.gap_since: float | None = None
        self.allow_gap_recovery = allow_gap_recovery
        self.gap_timeout_sec = gap_timeout_sec

    def push(self, payload: dict[str, Any]) -> list[bytes]:
        seq_raw = payload.get("s")
        if not isinstance(seq_raw, int):
            data = decode_stdout_data(payload)
            return [data] if data else []

        seq = seq_raw
        data = decode_stdout_data(payload)
        if not data:
            return []
        self.pending[seq] = data

        if self.next_seq is None:
            self.next_seq = min(self.pending)

        out: list[bytes] = []
        while self.next_seq in self.pending:
            out.append(self.pending.pop(self.next_seq))
            self.next_seq += 1
            self.gap_since = None

        if self.pending:
            if self.gap_since is None:
                self.gap_since = time_now()
            # If a frame appears lost, skip ahead to avoid freezing output.
            if (
                self.allow_gap_recovery
                and self.gap_since is not None
                and (time_now() - self.gap_since) > self.gap_timeout_sec
            ):
                self.next_seq = min(self.pending)
                self.gap_since = None
                while self.next_seq in self.pending:
                    out.append(self.pending.pop(self.next_seq))
                    self.next_seq += 1

        return out


def terminal_size() -> tuple[int, int]:
    size = get_terminal_size(fallback=(120, 40))
    return size.columns, size.lines


def handle_sigwinch(_signum: int, _frame: Any) -> None:
    RESIZE_PENDING.set()


def wait_for_ready(timeout_sec: float = 15.0) -> None:
    deadline = time_now() + timeout_sec
    while time_now() < deadline and not STOP_EVENT.is_set():
        msg = next_event(timeout=0.25)
        if not msg:
            continue
        msg_type = str(msg.get("type", ""))
        if msg_type == "ready":
            return
    raise RuntimeError("timed out waiting for marmotd ready")


def next_event(timeout: float = 0.0) -> dict[str, Any] | None:
    if DEFERRED_EVENTS:
        return DEFERRED_EVENTS.popleft()
    try:
        return EVENTS.get(timeout=timeout)
    except queue.Empty:
        return None


def time_now() -> float:
    import time

    return time.time()


def invite_call(moq_url: str) -> str:
    call_id = str(uuid.uuid4())
    send_cmd(
        {
            "cmd": "invite_call",
            "call_id": call_id,
            "nostr_group_id": GROUP_ID,
            "peer_pubkey": BOT_PUBKEY,
            "moq_url": moq_url,
            "broadcast_base": f"pika/pty/{call_id}",
            "track_name": "pty0",
            "track_codec": "bytes",
        }
    )
    return call_id


def wait_for_call_start(
    candidate_call_ids: set[str], timeout_sec: float = 20.0
) -> tuple[str | None, set[str]]:
    deadline = time_now() + timeout_sec
    ended_call_ids: set[str] = set()
    deferred: list[dict[str, Any]] = []
    try:
        while time_now() < deadline and not STOP_EVENT.is_set():
            msg = next_event(timeout=0.25)
            if not msg:
                continue
            msg_type = str(msg.get("type", ""))
            if msg_type == "error":
                message = str(msg.get("message", "unknown error"))
                tty_print(f"\n[agent] daemon error: {message}\n")
                continue
            if msg_type == "call_session_started":
                msg_call_id = str(msg.get("call_id", ""))
                if msg_call_id in candidate_call_ids:
                    return msg_call_id, ended_call_ids
                continue
            if msg_type == "call_session_ended":
                msg_call_id = str(msg.get("call_id", ""))
                if msg_call_id in candidate_call_ids:
                    ended_call_ids.add(msg_call_id)
                    candidate_call_ids.discard(msg_call_id)
                    if not candidate_call_ids:
                        return None, ended_call_ids
                    continue
                continue
            deferred.append(msg)
    finally:
        # Preserve ordering for non-call lifecycle events.
        for msg in reversed(deferred):
            DEFERRED_EVENTS.appendleft(msg)
    return None, ended_call_ids


def end_call(call_id: str, reason: str) -> None:
    send_cmd({"cmd": "end_call", "call_id": call_id, "reason": reason})


def run_terminal_loop(call_id: str) -> None:
    reorder = StdoutReorderBuffer(allow_gap_recovery=True, gap_timeout_sec=0.35)
    stdin_fd = sys.stdin.fileno()
    old_attrs = termios.tcgetattr(stdin_fd)
    signal.signal(signal.SIGWINCH, handle_sigwinch)
    tty.setraw(stdin_fd)
    RESIZE_PENDING.set()

    try:
        tty_print("\r\n[agent] connected. Ctrl-C to exit.\r\n")
        while not STOP_EVENT.is_set():
            # Drain daemon events first.
            while True:
                msg = next_event(timeout=0.0)
                if msg is None:
                    break
                msg_type = str(msg.get("type", ""))
                if msg_type == "call_data" and str(msg.get("call_id", "")) == call_id:
                    payload = decode_call_payload(msg)
                    if not payload:
                        continue
                    event_type = str(payload.get("t", ""))
                    if event_type == "stdout":
                        try:
                            for data in reorder.push(payload):
                                os.write(sys.stdout.fileno(), data)
                        except Exception:
                            pass
                    elif event_type == "exit":
                        code = payload.get("code")
                        tty_print(f"\r\n[agent] remote session ended (code={code}).\r\n")
                        return
                elif msg_type == "call_session_ended" and str(msg.get("call_id", "")) == call_id:
                    reason = str(msg.get("reason", "ended"))
                    tty_print(f"\r\n[agent] call ended: {reason}\r\n")
                    return
                elif msg_type == "error":
                    message = str(msg.get("message", "unknown error"))
                    tty_print(f"\r\n[agent] daemon error: {message}\r\n")

            if RESIZE_PENDING.is_set():
                RESIZE_PENDING.clear()
                cols, rows = terminal_size()
                send_call_payload(call_id, {"t": "resize", "cols": cols, "rows": rows})

            rlist, _, _ = select.select([stdin_fd], [], [], 0.02)
            if stdin_fd not in rlist:
                continue
            data = os.read(stdin_fd, 4096)
            if not data:
                continue

            # Ctrl-C exits the session cleanly.
            if b"\x03" in data:
                send_call_payload(call_id, {"t": "exit"})
                send_cmd({"cmd": "end_call", "call_id": call_id, "reason": "user_exit"})
                return

            send_call_payload(
                call_id,
                {"t": "stdin", "d": base64.b64encode(data).decode("ascii")},
            )
    finally:
        termios.tcsetattr(stdin_fd, termios.TCSADRAIN, old_attrs)


def run_test_capture_loop(call_id: str) -> int:
    reorder = StdoutReorderBuffer(allow_gap_recovery=False)
    captured = bytearray()
    tty_print(f"[agent] test mode: capturing stdout for up to {TEST_TIMEOUT_SEC:.1f}s...\n")
    cols, rows = terminal_size()
    send_call_payload(call_id, {"t": "resize", "cols": cols, "rows": rows})
    deadline = time_now() + TEST_TIMEOUT_SEC
    exit_seen = False
    exit_drain_deadline: float | None = None
    while time_now() < deadline and not STOP_EVENT.is_set():
        msg = next_event(timeout=0.25)
        if msg is None:
            if exit_drain_deadline is not None and time_now() >= exit_drain_deadline:
                break
            continue
        msg_type = str(msg.get("type", ""))
        if msg_type == "call_data" and str(msg.get("call_id", "")) == call_id:
            payload = decode_call_payload(msg)
            if not payload:
                continue
            event_type = str(payload.get("t", ""))
            if event_type == "stdout":
                try:
                    for data in reorder.push(payload):
                        captured.extend(data)
                except Exception:
                    pass
            elif event_type == "exit":
                exit_seen = True
                exit_drain_deadline = time_now() + 0.5
        elif msg_type == "call_session_ended" and str(msg.get("call_id", "")) == call_id:
            exit_seen = True
            if exit_drain_deadline is None:
                exit_drain_deadline = time_now() + 0.5
        elif msg_type == "error":
            message = str(msg.get("message", "unknown error"))
            tty_print(f"[agent] daemon error: {message}\n")
        if exit_drain_deadline is not None and time_now() >= exit_drain_deadline:
            break

    if CAPTURE_STDOUT_PATH:
        with open(CAPTURE_STDOUT_PATH, "wb") as fh:
            fh.write(captured)
        tty_print(f"[agent] wrote capture: {CAPTURE_STDOUT_PATH} ({len(captured)} bytes)\n")

    if not exit_seen:
        tty_print("[agent] test mode timed out before remote exit.\n")
        return 1

    if EXPECT_REPLAY_FILE:
        expected = load_expected_bytes_from_replay(EXPECT_REPLAY_FILE)
        if captured == expected:
            tty_print(
                "[agent] capture matches replay fixture "
                f"({len(captured)} bytes).\n"
            )
            return 0

        max_prefix_drop = max(0, MAX_PREFIX_DROP_BYTES)
        max_suffix_drop = max(0, MAX_SUFFIX_DROP_BYTES)
        max_prefix = min(max_prefix_drop, len(expected))
        max_suffix = min(max_suffix_drop, len(expected))

        matched_trim: tuple[int, int] | None = None
        for prefix in range(0, max_prefix + 1):
            for suffix in range(0, max_suffix + 1):
                end = len(expected) - suffix
                if end < prefix:
                    continue
                if captured == expected[prefix:end]:
                    matched_trim = (prefix, suffix)
                    break
            if matched_trim is not None:
                break

        if matched_trim is not None:
            prefix, suffix = matched_trim
            tty_print(
                "[agent] capture matches replay fixture with "
                f"leading_drop={prefix} bytes "
                f"trailing_drop={suffix} bytes "
                f"(got={len(captured)} expected={len(expected)}).\n"
            )
            return 0

        if captured != expected:
            min_len = min(len(captured), len(expected))
            mismatch = next(
                (idx for idx in range(min_len) if captured[idx] != expected[idx]),
                min_len,
            )
            tty_print(
                "[agent] capture mismatch: "
                f"got={len(captured)} expected={len(expected)} first_diff={mismatch}\n"
            )
            return 1
    return 0


def shutdown() -> None:
    global daemon
    STOP_EVENT.set()
    if daemon is not None:
        try:
            send_cmd({"cmd": "shutdown"})
        except Exception:
            pass
        try:
            daemon.terminate()
        except Exception:
            pass
        daemon = None


def maybe_stop_test_machine() -> None:
    # Provider-specific machine lifecycle hooks were removed; keep a no-op for compatibility.
    return


def main() -> int:
    global daemon, daemon_stderr_thread
    tty_print("\n")
    tty_print("Launching PTY agent session...\n")
    tty_print("MoQ candidates:\n")
    for url in MOQ_URLS:
        tty_print(f"  - {url}\n")
    tty_print("\n")

    daemon = spawn_daemon()
    reader = threading.Thread(target=daemon_reader, args=(daemon,), daemon=True)
    reader.start()
    stderr_reader = threading.Thread(target=daemon_stderr_reader, args=(daemon,), daemon=True)
    stderr_reader.start()
    daemon_stderr_thread = stderr_reader

    try:
        wait_for_ready()
        started_call_id: str | None = None
        pending_call_ids: set[str] = set()
        for moq_url in MOQ_URLS:
            tty_print(f"[agent] inviting call via {moq_url}...\n")
            call_id = invite_call(moq_url)
            pending_call_ids.add(call_id)
            matched_call_id, ended_call_ids = wait_for_call_start(
                set(pending_call_ids)
            )
            if ended_call_ids:
                pending_call_ids.difference_update(ended_call_ids)
            if matched_call_id is not None:
                started_call_id = matched_call_id
                break
            tty_print(f"[agent] no start for {moq_url}, trying next relay...\n")

        if started_call_id is not None:
            for pending_call_id in list(pending_call_ids):
                if pending_call_id != started_call_id:
                    end_call(pending_call_id, "superseded")

        if not started_call_id:
            for pending_call_id in list(pending_call_ids):
                end_call(pending_call_id, "no_start")
            tty_print("[agent] failed to start PTY call on available MoQ relays.\n")
            return 1

        if TEST_MODE:
            return run_test_capture_loop(started_call_id)

        run_terminal_loop(started_call_id)
        return 0
    finally:
        shutdown()
        maybe_stop_test_machine()


if __name__ == "__main__":
    sys.exit(main())
