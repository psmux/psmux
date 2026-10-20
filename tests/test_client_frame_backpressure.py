"""Windows client receive-queue regression; no real psmux servers or panes."""

import argparse
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import time
import uuid


ROOT = Path(tempfile.gettempdir()) / f"psmux-backpressure-{uuid.uuid4().hex}"
DEFAULT = Path.home() / ".psmux"
NAMESPACE = f"memprobe-{uuid.uuid4().hex}"
LIMIT = 384 * 1024 * 1024


class MemoryCounters(ctypes.Structure):
    _fields_ = [
        ("cb", wintypes.DWORD),
        ("PageFaultCount", wintypes.DWORD),
        *[(name, ctypes.c_size_t) for name in (
            "PeakWorkingSetSize", "WorkingSetSize", "QuotaPeakPagedPoolUsage",
            "QuotaPagedPoolUsage", "QuotaPeakNonPagedPoolUsage",
            "QuotaNonPagedPoolUsage", "PagefileUsage", "PeakPagefileUsage",
            "PrivateUsage",
        )],
    ]


kernel = ctypes.WinDLL("kernel32", use_last_error=True)
psapi = ctypes.WinDLL("psapi", use_last_error=True)
kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
kernel.OpenProcess.restype = wintypes.HANDLE
kernel.CloseHandle.argtypes = [wintypes.HANDLE]
psapi.GetProcessMemoryInfo.argtypes = [
    wintypes.HANDLE, ctypes.POINTER(MemoryCounters), wintypes.DWORD,
]


def private_bytes(pid):
    handle = kernel.OpenProcess(0x410, False, pid)
    if not handle:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        counters = MemoryCounters()
        counters.cb = ctypes.sizeof(counters)
        if not psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
            raise ctypes.WinError(ctypes.get_last_error())
        return counters.PrivateUsage
    finally:
        kernel.CloseHandle(handle)


def default_sessions():
    return sorted(p.stem for p in DEFAULT.glob("*.port"))


def run_probe(executable, drain_output):
    folder = ROOT / ("drained" if drain_output else "blocked")
    folder.mkdir(parents=True, exist_ok=True)
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen()
    listener.settimeout(0.2)
    base = f"{NAMESPACE}__probe"
    for suffix, value in (
        ("port", str(listener.getsockname()[1])),
        ("key", "isolated-memory-probe"),
        ("sid", "98765"),
    ):
        (folder / f"{base}.{suffix}").write_text(value)
    stop = threading.Event()
    attached = threading.Event()
    detach = threading.Event()
    errors = []
    sent = [0]
    drained = [0]
    sockets = []
    rows = [
        {"runs": [{
            "text": chr(65 + r % 26) * 160, "fg": "white",
            "bg": "blue" if r % 2 else "red", "flags": 0, "width": 160,
        }]}
        for r in range(59)
    ]
    frame = {
        "layout": {
            "type": "leaf", "id": 1, "rows": 59, "cols": 160,
            "cursor_row": 0, "cursor_col": 0, "active": True,
            "copy_mode": False, "scroll_offset": 0, "rows_v2": rows,
        },
        "windows": [],
    }
    wire_frame = json.dumps(frame, separators=(",", ":")).encode() + b"\n"

    def connection(sock):
        sockets.append(sock)
        sock.settimeout(15)
        persistent = False
        try:
            reader = sock.makefile("rb")
            if not reader.readline().startswith(b"AUTH "):
                raise RuntimeError("Unexpected authentication handshake")
            sock.sendall(b"OK\n")
            while not stop.is_set():
                line = reader.readline()
                if not line:
                    break
                if line == b"PERSISTENT\n":
                    persistent = True
                if line == b"session-info\n":
                    sock.sendall(b"OK\n")
                if persistent and line.startswith(b"dump-state"):
                    attached.set()
                    for n in range(3200):
                        if stop.is_set():
                            break
                        sock.sendall(wire_frame)
                        sent[0] = n + 1
                        time.sleep(0.002)
                    if detach.wait(20) and not stop.is_set():
                        sock.sendall(b"NC\nDETACH\n")
                    break
        except (ConnectionResetError, ConnectionAbortedError):
            # Short-lived registry/auth probes may close without a TCP drain.
            if persistent and not stop.is_set():
                errors.append("Persistent connection reset before test cleanup")
        except (OSError, ValueError) as exc:
            if not stop.is_set():
                errors.append(repr(exc))
        finally:
            sock.close()

    def serve():
        while not stop.is_set():
            try:
                sock, _ = listener.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            threading.Thread(target=connection, args=(sock,), daemon=True).start()

    threading.Thread(target=serve, daemon=True).start()
    env = os.environ.copy()
    for name in list(env):
        if name.startswith("PSMUX_") or name in ("TMUX", "TMUX_PANE"):
            del env[name]
    env.update({
        "PSMUX_DATA_DIR": str(folder), "PSMUX_PIPE_VT": "1",
        "TERM": "xterm-256color",
    })
    process = None
    result = {"drain_output": drain_output, "frame_bytes": len(wire_frame)}
    try:
        process = subprocess.Popen(
            [str(executable), "-L", NAMESPACE, "-f", "NUL", "attach", "-t", "probe"],
            env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, creationflags=subprocess.CREATE_NO_WINDOW,
        )
        process.stdin.write(b"\x1b[8;60;160t")
        process.stdin.flush()
        def drain():
            while True:
                chunk = process.stdout.read(4096)
                if not chunk:
                    return
                drained[0] += len(chunk)
        if drain_output:
            threading.Thread(target=drain, daemon=True).start()
        if not attached.wait(10):
            raise RuntimeError(f"Client did not reach dump-state; exit={process.poll()}")
        samples = []
        started = time.monotonic()
        while time.monotonic() - started < 12:
            if process.poll() is not None:
                raise RuntimeError(f"Client exited unexpectedly: {process.returncode}")
            memory = private_bytes(process.pid)
            samples.append({
                "seconds": round(time.monotonic() - started, 2),
                "frames_sent": sent[0], "private_mb": round(memory / 1048576, 2),
            })
            if memory > LIMIT:
                raise RuntimeError(f"Safety memory cap reached: {memory}")
            if sent[0] == 3200:
                break
            time.sleep(0.5)
        result.update({
            "pid": process.pid, "samples": samples,
            "frames_sent": sent[0], "stdout_bytes_drained": drained[0],
            "connection_errors": errors,
        })
        if not drain_output:
            threading.Thread(target=drain, daemon=True).start()
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                memory = private_bytes(process.pid)
                if memory < 10 * 1048576 and sent[0] == 3200:
                    break
                time.sleep(0.1)
            result["private_mb_after_terminal_resumes"] = round(memory / 1048576, 2)
            if sent[0] != 3200:
                raise AssertionError("Queued frames did not finish after output resumed")
            if memory >= 10 * 1048576:
                raise AssertionError("Client did not return below 10 MiB after output resumed")
        detach.set()
        process.wait(timeout=5)
        if process.returncode != 0:
            raise AssertionError(f"Server-directed detach failed: {process.returncode}")
        result["server_directed_detach_exit_code"] = process.returncode
        result["passed"] = (
            max(sample["private_mb"] for sample in samples) < 16
            and not errors
        )
    finally:
        stop.set()
        detach.set()
        listener.close()
        for sock in sockets:
            try:
                sock.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
        if process is not None:
            if process.poll() is None:
                process.terminate()
            process.wait(timeout=5)
            result["stderr"] = process.stderr.read().decode(errors="replace")[:1500]
            if result["stderr"]:
                print(json.dumps({"probe_stderr": result["stderr"]}), flush=True)
        for file in folder.iterdir():
            if file.is_file():
                file.unlink()
        folder.rmdir()
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", type=Path, required=True)
    args = parser.parse_args()
    executable = args.exe.resolve(strict=True)
    before = default_sessions()
    results = []
    try:
        for drain_output in (True, False):
            result = run_probe(executable, drain_output)
            results.append(result)
            print(json.dumps(result), flush=True)
    finally:
        after = default_sessions()
        print(json.dumps({
            "default_sessions_before": before, "default_sessions_after": after,
            "default_registry_unchanged": before == after,
        }), flush=True)
        if ROOT.exists() and not any(ROOT.iterdir()):
            ROOT.rmdir()
    if before != after:
        raise AssertionError("Default session registry changed")
    if not all(result["passed"] for result in results):
        raise AssertionError("Client memory exceeded 16 MiB or a connection failed")
