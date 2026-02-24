#!/usr/bin/env python3
"""Bridge between marmotd JSONL (stdin/stdout) and Pi.

Modes:
- PTY call mode (default): auto-accept data calls and attach remote Pi TUI over MoQ call data.
- RPC parity call mode: set PI_BRIDGE_CALL_MODE=rpc to forward raw Pi RPC JSON lines over framed call data.
- Legacy chat mode (optional): set PI_BRIDGE_ENABLE_CHAT=1 to keep old RPC text reply behavior.
"""

from __future__ import annotations

import base64
import json
import os
import pty
import select
import signal
import shlex
import struct
import subprocess
import sys
import termios
import threading
import time


EVENT_PREFIX = "__PI_EVT__"
RPC_MESSAGE_PREFIX = "__PI_RPC__"
FRAMED_PROTOCOL_VERSION = 1
RPC_STREAMS = {"rpc_event", "rpc_response", "rpc_request", "control"}
SEND_LOCK = threading.Lock()
PTY_LOCK = threading.Lock()
RPC_LOCK = threading.Lock()

my_pubkey: str | None = None
pi_proc: subprocess.Popen[bytes] | None = None

active_call_id: str | None = None
active_pty_pid: int | None = None
active_pty_fd: int | None = None
pty_reader_thread: threading.Thread | None = None
active_replay_thread: threading.Thread | None = None
active_replay_stop = threading.Event()
active_replay_peer_ready = threading.Event()

rpc_call_id: str | None = None
rpc_proc: subprocess.Popen[bytes] | None = None
rpc_reader_thread: threading.Thread | None = None
rpc_session_id: str | None = None
rpc_message_group_id: str | None = None
rpc_out_seq: dict[str, int] = {}
rpc_in_expected_seq: dict[str, int] = {}
rpc_in_pending: dict[str, dict[int, bytes]] = {}
rpc_in_fragments: dict[tuple[str, int], dict[str, object]] = {}
rpc_seen: set[tuple[str, str, int]] = set()

legacy_chat_enabled = os.environ.get("PI_BRIDGE_ENABLE_CHAT", "").strip() == "1"
call_mode = os.environ.get("PI_BRIDGE_CALL_MODE", "pty").strip().lower() or "pty"
if call_mode not in {"pty", "rpc"}:
    call_mode = "pty"
rpc_transport = os.environ.get("PI_BRIDGE_RPC_TRANSPORT", "call").strip().lower() or "call"
if rpc_transport not in {"call", "nostr"}:
    rpc_transport = "call"
replay_file = os.environ.get("PI_BRIDGE_REPLAY_FILE", "").strip()
replay_speed_raw = os.environ.get("PI_BRIDGE_REPLAY_SPEED", "1").strip()
replay_initial_delay_ms_raw = os.environ.get("PI_BRIDGE_REPLAY_INITIAL_DELAY_MS", "1000").strip()
replay_wait_for_peer_sec_raw = os.environ.get("PI_BRIDGE_REPLAY_WAIT_FOR_PEER_SEC", "3").strip()

try:
    max_rpc_fragment_bytes = int(os.environ.get("PI_BRIDGE_RPC_FRAGMENT_BYTES", "6000"))
except ValueError:
    max_rpc_fragment_bytes = 6000
if max_rpc_fragment_bytes < 256:
    max_rpc_fragment_bytes = 256

max_rpc_reorder_window = 4096


def log(msg: str) -> None:
    print(f"[pi-bridge] {msg}", file=sys.stderr, flush=True)


def send_to_marmotd(cmd: dict) -> None:
    line = json.dumps(cmd, separators=(",", ":"))
    with SEND_LOCK:
        print(line, flush=True)


def send_to_pi(proc: subprocess.Popen[bytes], msg: dict) -> None:
    line = json.dumps(msg) + "\n"
    assert proc.stdin is not None
    proc.stdin.write(line.encode())
    proc.stdin.flush()


def emit_pi_event(group_id: str, payload: dict) -> None:
    send_to_marmotd(
        {
            "cmd": "send_message",
            "nostr_group_id": group_id,
            "content": EVENT_PREFIX + json.dumps(payload, separators=(",", ":")),
        }
    )


def build_publish_keypackage_cmd():
    raw = os.environ.get("PI_RELAYS_JSON", "").strip()
    if not raw:
        return {"cmd": "publish_keypackage"}
    try:
        parsed = json.loads(raw)
    except Exception as err:
        log(f"invalid PI_RELAYS_JSON, ignoring: {err}")
        return {"cmd": "publish_keypackage"}
    if not isinstance(parsed, list):
        return {"cmd": "publish_keypackage"}
    relays = [str(item).strip() for item in parsed if str(item).strip()]
    if not relays:
        return {"cmd": "publish_keypackage"}
    return {"cmd": "publish_keypackage", "relays": relays}


def spawn_pi_rpc(label: str) -> subprocess.Popen[bytes]:
    env = os.environ.copy()
    cmd = shlex.split(
        os.environ.get(
            "PI_CMD",
            "pi --mode rpc --no-session --provider anthropic",
        )
    )
    model = os.environ.get("PI_MODEL")
    if model:
        cmd.extend(["--model", model])
    log(f"starting pi rpc ({label}): {' '.join(cmd)}")
    return subprocess.Popen(
        cmd,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=sys.stderr,
        env=env,
        bufsize=0,
    )


def collect_pi_response(proc: subprocess.Popen[bytes], group_id: str) -> str:
    text_parts: list[str] = []
    pending_delta_parts: list[str] = []
    last_delta_emit = time.time()
    assert proc.stdout is not None
    for raw in proc.stdout:
        raw = raw.decode().strip()
        if not raw:
            continue
        try:
            event = json.loads(raw)
        except json.JSONDecodeError:
            continue

        etype = event.get("type")
        if etype == "message_update":
            delta_event = event.get("assistantMessageEvent", {})
            delta_type = delta_event.get("type")
            if delta_type == "text_delta":
                delta = delta_event.get("delta", "")
                text_parts.append(delta)
                pending_delta_parts.append(delta)
                pending_joined = "".join(pending_delta_parts)
                now = time.time()
                should_emit = (
                    len(pending_joined) >= 120
                    or "\n" in delta
                    or now - last_delta_emit >= 0.5
                )
                if should_emit and pending_joined:
                    emit_pi_event(group_id, {"kind": "text_delta", "text": pending_joined})
                    pending_delta_parts = []
                    last_delta_emit = now
            else:
                payload = {
                    "kind": "assistant_event",
                    "event_type": str(delta_type or "unknown"),
                }
                for key in (
                    "name",
                    "tool_name",
                    "toolName",
                    "toolCallId",
                    "callId",
                    "id",
                ):
                    value = delta_event.get(key)
                    if isinstance(value, str) and value:
                        payload[key] = value
                if isinstance(delta_event.get("arguments"), str):
                    payload["arguments"] = delta_event["arguments"][:300]
                if isinstance(delta_event.get("text"), str):
                    payload["text"] = delta_event["text"][:300]
                emit_pi_event(group_id, payload)
        elif etype == "agent_end":
            break
        elif etype == "response" and not event.get("success"):
            emit_pi_event(
                group_id,
                {"kind": "error", "message": str(event.get("error") or "pi response failed")},
            )
            break

    if pending_delta_parts:
        emit_pi_event(group_id, {"kind": "text_delta", "text": "".join(pending_delta_parts)})
    return "".join(text_parts)


def encode_call_payload_hex(obj: dict) -> str:
    return json.dumps(obj, separators=(",", ":")).encode("utf-8").hex()


def decode_call_payload(msg: dict) -> dict | None:
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


def send_call_payload(call_id: str, payload_obj: dict) -> None:
    send_to_marmotd(
        {
            "cmd": "send_call_data",
            "call_id": call_id,
            "payload_hex": encode_call_payload_hex(payload_obj),
        }
    )


def parse_replay_speed(raw: str) -> float:
    try:
        speed = float(raw)
        if speed > 0:
            return speed
    except Exception:
        pass
    return 1.0


def load_replay_frames(path: str) -> list[tuple[float, bytes]]:
    with open(path, "r", encoding="utf-8") as fh:
        doc = json.load(fh)
    if not isinstance(doc, dict):
        raise ValueError("replay file must contain an object")
    frames_raw = doc.get("frames")
    if not isinstance(frames_raw, list) or not frames_raw:
        raise ValueError("replay file has no frames")
    out: list[tuple[float, bytes]] = []
    for idx, frame in enumerate(frames_raw):
        if not isinstance(frame, dict):
            raise ValueError(f"frame {idx} must be an object")
        data_b64 = str(frame.get("stdout_b64", "")).strip()
        if not data_b64:
            raise ValueError(f"frame {idx} missing stdout_b64")
        delay_ms = float(frame.get("delay_ms", 0) or 0)
        if delay_ms < 0:
            delay_ms = 0
        try:
            payload = base64.b64decode(data_b64)
        except Exception as err:
            raise ValueError(f"frame {idx} invalid stdout_b64: {err}") from err
        out.append((delay_ms / 1000.0, payload))
    return out


REPLAY_SPEED = parse_replay_speed(replay_speed_raw)
REPLAY_INITIAL_DELAY_MS = 1000.0
REPLAY_WAIT_FOR_PEER_SEC = 3.0
try:
    REPLAY_INITIAL_DELAY_MS = max(float(replay_initial_delay_ms_raw), 0.0)
except Exception:
    REPLAY_INITIAL_DELAY_MS = 1000.0
try:
    REPLAY_WAIT_FOR_PEER_SEC = max(float(replay_wait_for_peer_sec_raw), 0.0)
except Exception:
    REPLAY_WAIT_FOR_PEER_SEC = 3.0
REPLAY_FRAMES: list[tuple[float, bytes]] = []
if replay_file:
    try:
        REPLAY_FRAMES = load_replay_frames(replay_file)
        log(
            "replay mode enabled: "
            f"file={replay_file} frames={len(REPLAY_FRAMES)} "
            f"speed={REPLAY_SPEED:.2f}x initial_delay_ms={REPLAY_INITIAL_DELAY_MS:.0f} "
            f"wait_for_peer_sec={REPLAY_WAIT_FOR_PEER_SEC:.1f}"
        )
    except Exception as err:
        log(f"invalid PI_BRIDGE_REPLAY_FILE={replay_file}: {err}")
        sys.exit(2)


def set_pty_size(fd: int, cols: int, rows: int) -> None:
    winsz = struct.pack("HHHH", rows, cols, 0, 0)
    try:
        import fcntl

        fcntl.ioctl(fd, termios.TIOCSWINSZ, winsz)
    except Exception as err:
        log(f"set pty size failed: {err}")


def decode_wait_status(status: int) -> int | None:
    if os.WIFEXITED(status):
        return os.WEXITSTATUS(status)
    if os.WIFSIGNALED(status):
        return 128 + os.WTERMSIG(status)
    return None


def reap_child_exit_code(pid: int, timeout_sec: float = 1.0) -> int | None:
    deadline = time.monotonic() + max(0.0, timeout_sec)
    sent_term = False
    while True:
        try:
            waited_pid, status = os.waitpid(pid, os.WNOHANG)
        except ChildProcessError:
            return None
        if waited_pid == pid:
            return decode_wait_status(status)

        now = time.monotonic()
        if now < deadline:
            time.sleep(0.02)
            continue

        if not sent_term:
            sent_term = True
            deadline = now + 0.5
            try:
                os.kill(pid, signal.SIGTERM)
            except Exception:
                pass
            continue

        try:
            os.kill(pid, signal.SIGKILL)
        except Exception:
            pass
        try:
            _, status = os.waitpid(pid, 0)
            return decode_wait_status(status)
        except ChildProcessError:
            return None


def pty_reader_loop(call_id: str, pid: int, fd: int) -> None:
    global active_call_id, active_pty_fd, active_pty_pid
    flush_interval_sec = 0.03
    max_chunk_bytes = 32768
    pending = bytearray()
    last_flush = time.monotonic()
    next_seq = 0

    def flush_pending() -> None:
        nonlocal pending, last_flush, next_seq
        if not pending:
            return
        send_call_payload(
            call_id,
            {
                "t": "stdout",
                "s": next_seq,
                "d": base64.b64encode(bytes(pending)).decode("ascii"),
            },
        )
        pending.clear()
        last_flush = time.monotonic()
        next_seq += 1

    try:
        while True:
            with PTY_LOCK:
                if active_call_id != call_id or active_pty_fd != fd:
                    break
            rlist, _, _ = select.select([fd], [], [], 0.02)
            if fd in rlist:
                try:
                    chunk = os.read(fd, 8192)
                except OSError:
                    break
                if not chunk:
                    break
                pending.extend(chunk)
            now = time.monotonic()
            if pending and (
                len(pending) >= max_chunk_bytes or now - last_flush >= flush_interval_sec
            ):
                flush_pending()
    finally:
        flush_pending()
        exit_code = reap_child_exit_code(pid)
        send_call_payload(call_id, {"t": "exit", "code": exit_code})
        send_to_marmotd({"cmd": "end_call", "call_id": call_id, "reason": "pty_exit"})
        with PTY_LOCK:
            if active_call_id == call_id:
                active_call_id = None
            if active_pty_fd == fd:
                active_pty_fd = None
            if active_pty_pid == pid:
                active_pty_pid = None


def replay_sleep(seconds: float) -> bool:
    if seconds <= 0:
        return True
    deadline = time.time() + seconds
    while True:
        if active_replay_stop.wait(timeout=0.02):
            return False
        if time.time() >= deadline:
            return True


def replay_loop(call_id: str) -> None:
    global active_call_id, active_replay_thread
    next_seq = 0
    try:
        if REPLAY_WAIT_FOR_PEER_SEC > 0:
            active_replay_peer_ready.wait(timeout=REPLAY_WAIT_FOR_PEER_SEC)
        if REPLAY_INITIAL_DELAY_MS > 0:
            if not replay_sleep((REPLAY_INITIAL_DELAY_MS / 1000.0) / REPLAY_SPEED):
                return
        for delay_sec, chunk in REPLAY_FRAMES:
            with PTY_LOCK:
                if active_call_id != call_id:
                    break
            if not replay_sleep(delay_sec / REPLAY_SPEED):
                break
            send_call_payload(
                call_id,
                {
                    "t": "stdout",
                    "s": next_seq,
                    "d": base64.b64encode(chunk).decode("ascii"),
                },
            )
            next_seq += 1
    finally:
        send_call_payload(call_id, {"t": "exit", "code": 0})
        send_to_marmotd({"cmd": "end_call", "call_id": call_id, "reason": "replay_done"})
        with PTY_LOCK:
            if active_call_id == call_id:
                active_call_id = None
            active_replay_thread = None


def start_pi_pty(call_id: str) -> None:
    global active_call_id, active_pty_pid, active_pty_fd, pty_reader_thread, active_replay_thread
    need_stop = False
    with PTY_LOCK:
        if active_call_id and active_call_id != call_id:
            need_stop = True
        elif active_call_id == call_id and active_pty_fd is not None:
            return
    if need_stop:
        stop_pi_pty("superseded")
    with PTY_LOCK:
        active_call_id = call_id

    if REPLAY_FRAMES:
        active_replay_stop.clear()
        active_replay_peer_ready.clear()
        log(f"starting replay PTY stream for call {call_id}")
        t = threading.Thread(target=replay_loop, args=(call_id,), daemon=True)
        with PTY_LOCK:
            active_replay_thread = t
            active_pty_pid = None
            active_pty_fd = None
        t.start()
        return

    env = os.environ.copy()
    cmd = ["pi", "--no-session", "--provider", "anthropic"]
    model = os.environ.get("PI_MODEL")
    if model:
        cmd.extend(["--model", model])
    log(f"starting pi pty for call {call_id}: {' '.join(cmd)}")
    pid, fd = pty.fork()
    if pid == 0:
        os.execvpe(cmd[0], cmd, env)
        os._exit(1)

    with PTY_LOCK:
        active_pty_pid = pid
        active_pty_fd = fd
        t = threading.Thread(target=pty_reader_loop, args=(call_id, pid, fd), daemon=True)
        pty_reader_thread = t
        t.start()


def stop_pi_pty(reason: str) -> None:
    global active_call_id, active_pty_fd, active_pty_pid, active_replay_thread
    with PTY_LOCK:
        call_id = active_call_id
        pid = active_pty_pid
        fd = active_pty_fd
        replay_thread = active_replay_thread
        active_call_id = None
        active_pty_fd = None
        active_pty_pid = None
        active_replay_thread = None
    active_replay_stop.set()
    if fd is not None:
        try:
            os.close(fd)
        except Exception:
            pass
    if pid is not None:
        try:
            os.kill(pid, signal.SIGTERM)
        except Exception:
            pass
    if replay_thread is not None and replay_thread.is_alive():
        replay_thread.join(timeout=0.5)
    if call_id:
        log(f"stopped pty call {call_id}: {reason}")


def handle_call_data(msg: dict) -> None:
    call_id = str(msg.get("call_id", ""))
    with PTY_LOCK:
        if call_id == active_call_id:
            active_replay_peer_ready.set()

    payload = decode_call_payload(msg)
    if not payload:
        return
    msg_type = str(payload.get("t", ""))
    if msg_type == "exit":
        send_to_marmotd({"cmd": "end_call", "call_id": call_id, "reason": "peer_exit"})
        return

    with PTY_LOCK:
        if call_id != active_call_id:
            return
        fd = active_pty_fd

    if fd is None:
        return

    if msg_type == "stdin":
        data_b64 = str(payload.get("d", ""))
        if not data_b64:
            return
        try:
            data = base64.b64decode(data_b64)
            os.write(fd, data)
        except Exception as err:
            log(f"stdin payload write failed: {err}")
    elif msg_type == "resize":
        cols = int(payload.get("cols", 0) or 0)
        rows = int(payload.get("rows", 0) or 0)
        if cols > 0 and rows > 0:
            set_pty_size(fd, cols, rows)


def reset_rpc_session_state(session_id: str) -> None:
    global rpc_session_id, rpc_out_seq, rpc_in_expected_seq, rpc_in_pending, rpc_in_fragments, rpc_seen
    rpc_session_id = session_id
    rpc_out_seq = {stream: 0 for stream in RPC_STREAMS}
    rpc_in_expected_seq = {stream: 0 for stream in RPC_STREAMS}
    rpc_in_pending = {stream: {} for stream in RPC_STREAMS}
    rpc_in_fragments = {}
    rpc_seen = set()


def classify_pi_rpc_stream(raw_line: bytes) -> str:
    try:
        parsed = json.loads(raw_line.decode("utf-8").strip())
        if isinstance(parsed, dict) and parsed.get("type") == "response":
            return "rpc_response"
    except Exception:
        pass
    return "rpc_event"


def send_rpc_framed_payload(call_id: str, stream: str, payload: bytes) -> None:
    global rpc_message_group_id
    if stream not in RPC_STREAMS:
        return
    with RPC_LOCK:
        if rpc_transport == "call":
            if call_id != rpc_call_id:
                return
            session_id = rpc_session_id or call_id
            target_group_id = None
        else:
            if rpc_proc is None:
                return
            session_id = rpc_session_id
            target_group_id = rpc_message_group_id
            if not session_id or not target_group_id:
                return
        seq = rpc_out_seq.get(stream, 0)
        rpc_out_seq[stream] = seq + 1

    frag_count = max(1, (len(payload) + max_rpc_fragment_bytes - 1) // max_rpc_fragment_bytes)
    for frag_index in range(frag_count):
        start = frag_index * max_rpc_fragment_bytes
        end = start + max_rpc_fragment_bytes
        frag = payload[start:end]
        envelope = {
            "v": FRAMED_PROTOCOL_VERSION,
            "session_id": session_id,
            "stream": stream,
            "seq": seq,
            "frag_index": frag_index,
            "frag_count": frag_count,
            "payload_b64": base64.b64encode(frag).decode("ascii"),
        }
        if rpc_transport == "call":
            send_call_payload(call_id, envelope)
        else:
            send_to_marmotd(
                {
                    "cmd": "send_message",
                    "nostr_group_id": target_group_id,
                    "content": RPC_MESSAGE_PREFIX + json.dumps(envelope, separators=(",", ":")),
                }
            )


def send_rpc_control(call_id: str, payload_obj: dict) -> None:
    payload = json.dumps(payload_obj, separators=(",", ":")).encode("utf-8")
    send_rpc_framed_payload(call_id, "control", payload)


def rpc_reader_loop(call_id: str, proc: subprocess.Popen[bytes]) -> None:
    global rpc_call_id, rpc_proc, rpc_message_group_id, rpc_session_id
    exit_code: int | None = None
    try:
        assert proc.stdout is not None
        while True:
            with RPC_LOCK:
                if rpc_transport == "call":
                    if rpc_call_id != call_id or rpc_proc is not proc:
                        break
                elif rpc_proc is not proc:
                    break
            raw = proc.stdout.readline()
            if not raw:
                break
            stream = classify_pi_rpc_stream(raw)
            send_rpc_framed_payload(call_id, stream, raw)

        poll = proc.poll()
        if poll is None:
            proc.terminate()
            try:
                proc.wait(timeout=2.0)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=2.0)
        exit_code = proc.poll()
    except Exception as err:
        log(f"rpc reader loop failed: {err}")
    finally:
        send_rpc_control(
            call_id,
            {
                "type": "close",
                "reason": "remote_exit",
                "code": exit_code,
            },
        )
        if rpc_transport == "call":
            send_to_marmotd({"cmd": "end_call", "call_id": call_id, "reason": "rpc_exit"})
        with RPC_LOCK:
            if rpc_transport == "call" and rpc_call_id == call_id:
                rpc_call_id = None
            if rpc_proc is proc:
                rpc_proc = None
            if rpc_transport == "nostr":
                rpc_message_group_id = None
                rpc_session_id = None


def start_pi_rpc_call(call_id: str) -> None:
    global rpc_call_id, rpc_proc, rpc_reader_thread
    if rpc_transport != "call":
        return
    need_stop = False
    with RPC_LOCK:
        if rpc_call_id and rpc_call_id != call_id:
            need_stop = True
        elif rpc_call_id == call_id and rpc_proc is not None and rpc_proc.poll() is None:
            return
    if need_stop:
        stop_pi_rpc_call("superseded")

    proc = spawn_pi_rpc("call")
    with RPC_LOCK:
        rpc_call_id = call_id
        rpc_proc = proc
        reset_rpc_session_state(call_id)
        t = threading.Thread(target=rpc_reader_loop, args=(call_id, proc), daemon=True)
        rpc_reader_thread = t
        t.start()

    send_rpc_control(
        call_id,
        {
            "type": "open_ack",
            "session_id": call_id,
            "server_version": FRAMED_PROTOCOL_VERSION,
        },
    )


def stop_pi_rpc_call(reason: str) -> None:
    global rpc_call_id, rpc_proc, rpc_session_id
    if rpc_transport != "call":
        return
    with RPC_LOCK:
        call_id = rpc_call_id
        proc = rpc_proc
        rpc_call_id = None
        rpc_proc = None
        rpc_session_id = None
    if proc is not None and proc.poll() is None:
        try:
            proc.terminate()
        except Exception:
            pass
    if call_id:
        log(f"stopped rpc call {call_id}: {reason}")


def start_pi_rpc_nostr(group_id: str) -> None:
    global rpc_proc, rpc_reader_thread, rpc_message_group_id
    if rpc_transport != "nostr":
        return
    gid = str(group_id or "").strip()
    if not gid:
        return
    with RPC_LOCK:
        if rpc_proc is not None and rpc_proc.poll() is None:
            rpc_message_group_id = gid
            return
    proc = spawn_pi_rpc("nostr")
    with RPC_LOCK:
        rpc_proc = proc
        rpc_message_group_id = gid
        t = threading.Thread(target=rpc_reader_loop, args=("", proc), daemon=True)
        rpc_reader_thread = t
        t.start()


def stop_pi_rpc_nostr(reason: str) -> None:
    global rpc_proc, rpc_session_id, rpc_message_group_id
    if rpc_transport != "nostr":
        return
    with RPC_LOCK:
        proc = rpc_proc
        rpc_proc = None
        rpc_session_id = None
        rpc_message_group_id = None
    if proc is not None and proc.poll() is None:
        try:
            proc.terminate()
        except Exception:
            pass
    log(f"stopped rpc nostr session: {reason}")


def parse_rpc_envelope(payload: dict) -> tuple[str, str, int, int, int, bytes] | None:
    try:
        version = int(payload.get("v", 0))
    except Exception:
        return None
    if version != FRAMED_PROTOCOL_VERSION:
        return None
    session_id = str(payload.get("session_id", "")).strip()
    stream = str(payload.get("stream", "")).strip()
    if not session_id or stream not in RPC_STREAMS:
        return None

    try:
        seq = int(payload.get("seq", -1))
        frag_index = int(payload.get("frag_index", -1))
        frag_count = int(payload.get("frag_count", -1))
    except Exception:
        return None

    if seq < 0 or frag_count <= 0 or frag_index < 0 or frag_index >= frag_count:
        return None

    payload_b64 = str(payload.get("payload_b64", ""))
    try:
        raw = base64.b64decode(payload_b64, validate=True)
    except Exception:
        return None

    return session_id, stream, seq, frag_index, frag_count, raw


def prune_rpc_seen(session_id: str, stream: str, next_expected_seq: int) -> None:
    global rpc_seen
    if len(rpc_seen) < max_rpc_reorder_window * 2:
        return
    cutoff = next_expected_seq - max_rpc_reorder_window
    if cutoff <= 0:
        return
    rpc_seen = {
        key
        for key in rpc_seen
        if not (
            key[0] == session_id
            and key[1] == stream
            and isinstance(key[2], int)
            and key[2] < cutoff
        )
    }


def handle_complete_rpc_frame(call_id: str, session_id: str, stream: str, payload: bytes) -> None:
    if stream == "rpc_request":
        with RPC_LOCK:
            if rpc_transport == "call":
                proc = rpc_proc if rpc_call_id == call_id else None
            else:
                proc = rpc_proc
        if proc is None or proc.stdin is None:
            return
        data = payload if payload.endswith(b"\n") else payload + b"\n"
        try:
            proc.stdin.write(data)
            proc.stdin.flush()
        except Exception as err:
            log(f"forward rpc request failed: {err}")
        return

    if stream != "control":
        return

    try:
        ctrl = json.loads(payload.decode("utf-8"))
    except Exception:
        return
    ctrl_type = str(ctrl.get("type", "")).strip()
    if ctrl_type == "ping":
        resp = {"type": "pong"}
        if "ts" in ctrl:
            resp["ts"] = ctrl["ts"]
        send_rpc_control(call_id, resp)
    elif ctrl_type == "open":
        send_rpc_control(
            call_id,
            {
                "type": "open_ack",
                "session_id": session_id,
                "server_version": FRAMED_PROTOCOL_VERSION,
            },
        )
    elif ctrl_type == "close":
        if rpc_transport == "call":
            send_to_marmotd({"cmd": "end_call", "call_id": call_id, "reason": "peer_close"})
        else:
            stop_pi_rpc_nostr("peer_close")


def handle_rpc_envelope(call_id: str, envelope: dict, group_id: str | None = None) -> None:
    global rpc_message_group_id
    parsed = parse_rpc_envelope(envelope)
    if parsed is None:
        return
    session_id, stream, seq, frag_index, frag_count, frag_payload = parsed

    with RPC_LOCK:
        if rpc_transport == "call":
            if call_id != rpc_call_id or rpc_proc is None:
                return
        else:
            if rpc_proc is None:
                return
            if group_id:
                gid = str(group_id).strip()
                if not gid:
                    return
                if rpc_message_group_id and rpc_message_group_id != gid:
                    return
                rpc_message_group_id = gid

        if rpc_session_id and session_id != rpc_session_id:
            return

        if rpc_session_id is None:
            reset_rpc_session_state(session_id)

        dedupe_key = (session_id, stream, seq)
        if dedupe_key in rpc_seen:
            return

        expected = rpc_in_expected_seq.get(stream, 0)
        if seq < expected:
            rpc_seen.add(dedupe_key)
            return
        if seq - expected > max_rpc_reorder_window:
            return

        frag_key = (stream, seq)
        entry = rpc_in_fragments.get(frag_key)
        if entry is None:
            entry = {"frag_count": frag_count, "parts": {}}
            rpc_in_fragments[frag_key] = entry
        elif int(entry.get("frag_count", -1)) != frag_count:
            del rpc_in_fragments[frag_key]
            return

        parts = entry.get("parts")
        if not isinstance(parts, dict):
            rpc_in_fragments[frag_key] = {"frag_count": frag_count, "parts": {frag_index: frag_payload}}
            return
        if frag_index in parts:
            return
        parts[frag_index] = frag_payload

        if len(parts) < frag_count:
            return

        assembled: list[bytes] = []
        for idx in range(frag_count):
            part = parts.get(idx)
            if not isinstance(part, (bytes, bytearray)):
                return
            assembled.append(bytes(part))
        del rpc_in_fragments[frag_key]

        stream_pending = rpc_in_pending.setdefault(stream, {})
        if seq not in stream_pending:
            stream_pending[seq] = b"".join(assembled)

        drain_expected = rpc_in_expected_seq.get(stream, 0)
        ready: list[tuple[int, bytes]] = []
        while True:
            payload = stream_pending.pop(drain_expected, None)
            if payload is None:
                break
            ready.append((drain_expected, payload))
            rpc_seen.add((session_id, stream, drain_expected))
            drain_expected += 1
        rpc_in_expected_seq[stream] = drain_expected
        prune_rpc_seen(session_id, stream, drain_expected)

    for _, payload in ready:
        handle_complete_rpc_frame(call_id, session_id, stream, payload)


def handle_rpc_call_data(msg: dict) -> None:
    call_id = str(msg.get("call_id", ""))
    payload = decode_call_payload(msg)
    if not payload:
        return
    handle_rpc_envelope(call_id, payload)


def handle_rpc_message(msg: dict) -> bool:
    if rpc_transport != "nostr":
        return False
    content = str(msg.get("content", ""))
    if not content.startswith(RPC_MESSAGE_PREFIX):
        return False
    group_id = str(msg.get("nostr_group_id", "")).strip()
    if not group_id:
        return True
    start_pi_rpc_nostr(group_id)
    raw_payload = content[len(RPC_MESSAGE_PREFIX) :]
    try:
        payload = json.loads(raw_payload)
    except Exception:
        log("invalid rpc message payload")
        return True
    handle_rpc_envelope("", payload, group_id=group_id)
    return True


def main() -> None:
    global my_pubkey, pi_proc
    log(f"bridge call mode={call_mode} rpc_transport={rpc_transport} legacy_chat={legacy_chat_enabled}")
    if legacy_chat_enabled:
        pi_proc = spawn_pi_rpc("legacy_chat")

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue

        msg_type = msg.get("type")
        if msg_type == "ready":
            my_pubkey = msg.get("pubkey")
            log(f"marmotd ready, pubkey={my_pubkey}")
            send_to_marmotd(build_publish_keypackage_cmd())

        if msg_type == "call_invite_received":
            call_id = str(msg.get("call_id", ""))
            if call_mode == "pty":
                with PTY_LOCK:
                    busy = active_call_id is not None
            else:
                with RPC_LOCK:
                    busy = rpc_call_id is not None
            if busy:
                send_to_marmotd(
                    {"cmd": "reject_call", "call_id": call_id, "reason": "busy"}
                )
            else:
                send_to_marmotd({"cmd": "accept_call", "call_id": call_id})
            continue

        if msg_type == "call_session_started":
            call_id = str(msg.get("call_id", ""))
            if call_mode == "pty":
                start_pi_pty(call_id)
            else:
                start_pi_rpc_call(call_id)
            continue

        if msg_type == "call_data":
            if call_mode == "pty":
                handle_call_data(msg)
            else:
                handle_rpc_call_data(msg)
            continue

        if msg_type == "call_session_ended":
            call_id = str(msg.get("call_id", ""))
            if call_mode == "pty":
                should_stop = False
                with PTY_LOCK:
                    should_stop = call_id == active_call_id
                if should_stop:
                    stop_pi_pty("remote_end")
            else:
                should_stop = False
                with RPC_LOCK:
                    should_stop = call_id == rpc_call_id
                if should_stop:
                    stop_pi_rpc_call("remote_end")
            continue

        if msg_type == "message_received":
            if msg.get("from_pubkey") == my_pubkey:
                continue
            if handle_rpc_message(msg):
                continue
            if legacy_chat_enabled and pi_proc is not None:
                content = msg.get("content", "")
                group_id = msg.get("nostr_group_id", "")
                send_to_pi(pi_proc, {"type": "prompt", "message": content})
                emit_pi_event(group_id, {"kind": "status", "message": "processing"})
                response = collect_pi_response(pi_proc, group_id)
                if response.strip():
                    send_to_marmotd(
                        {
                            "cmd": "send_message",
                            "nostr_group_id": group_id,
                            "content": response,
                        }
                    )
                emit_pi_event(group_id, {"kind": "status", "message": "done"})

    stop_pi_pty("stdin_closed")
    stop_pi_rpc_call("stdin_closed")
    stop_pi_rpc_nostr("stdin_closed")
    if pi_proc and pi_proc.poll() is None:
        pi_proc.terminate()


if __name__ == "__main__":
    main()
