"""Loopback-only throughput comparison for two PeerCastStation Windows builds."""

from __future__ import annotations

import argparse
import hashlib
import http.client
import json
import os
import platform
import socket
import statistics
import subprocess
import sys
import threading
import time
import ctypes
from ctypes import wintypes
from pathlib import Path


CHANNEL_ID = "0123456789abcdef0123456789abcdef"
CHUNK_SIZE = 64 * 1024
RATE_BYTES_PER_SECOND = 16 * 1024 * 1024
STREAM_SECONDS = 4.0
VIEWER_COUNT = 8
PING_COUNT = 30


def free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def make_config(port: int, push_port: int) -> dict:
    return {
        "schemaVersion": 2,
        "server": {
            "port": port,
            "bindAddress": "127.0.0.1",
            "allowRemoteAdmin": False,
            "authToken": None,
            "maxRelays": 16,
            "maxRelaysPerChannel": 8,
            "maxDirects": max(16, VIEWER_COUNT),
            "maxDirectsPerChannel": max(8, VIEWER_COUNT),
            "maxUpstreamRate": 100000,
            "maxUpstreamRateIpv6": 100000,
        },
        "logging": {
            "level": "off",
            "maxLogBytes": 65536,
            "maxBackups": 0,
            "maxInMemoryEntries": 64,
            "logFile": None,
        },
        "lifecycle": {"headless": True, "shutdownTimeoutSecs": 5},
        "runtime": {
            "portMapping": {"enabled": False},
            "channels": [
                {
                    "channelId": CHANNEL_ID,
                    "name": "loopback benchmark",
                    "containerType": "RAW",
                    "source": {
                        "type": "httpPush",
                        "bindAddress": f"127.0.0.1:{push_port}",
                        "authToken": None,
                    },
                }
            ],
        },
    }


def process_cpu_seconds(process: subprocess.Popen) -> float:
    class FileTime(ctypes.Structure):
        _fields_ = [("dwLowDateTime", wintypes.DWORD), ("dwHighDateTime", wintypes.DWORD)]

        def ticks(self) -> int:
            return (self.dwHighDateTime << 32) | self.dwLowDateTime

    kernel, user, created, exited = FileTime(), FileTime(), FileTime(), FileTime()
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    get_times = kernel32.GetProcessTimes
    get_times.argtypes = [
        wintypes.HANDLE,
        ctypes.POINTER(FileTime),
        ctypes.POINTER(FileTime),
        ctypes.POINTER(FileTime),
        ctypes.POINTER(FileTime),
    ]
    get_times.restype = wintypes.BOOL
    handle = wintypes.HANDLE(process._handle)
    if not get_times(
        handle,
        ctypes.byref(created),
        ctypes.byref(exited),
        ctypes.byref(kernel),
        ctypes.byref(user),
    ):
        raise ctypes.WinError(ctypes.get_last_error())
    return (kernel.ticks() + user.ticks()) / 10_000_000.0


def read_http_headers(sock: socket.socket, limit: int = 65536) -> tuple[int, bytes]:
    data = bytearray()
    while b"\r\n\r\n" not in data:
        part = sock.recv(4096)
        if not part:
            raise RuntimeError("HTTP peer closed before response headers")
        data.extend(part)
        if len(data) > limit:
            raise RuntimeError("HTTP response headers exceeded the benchmark limit")
    end = data.index(b"\r\n\r\n") + 4
    head, remainder = bytes(data[:end]), bytes(data[end:])
    try:
        status = int(head.split(b"\r\n", 1)[0].split()[1])
    except (IndexError, ValueError) as exc:
        raise RuntimeError("invalid HTTP response status line") from exc
    return status, remainder


def run(binary: Path, output: Path) -> dict:
    if os.name != "nt":
        raise RuntimeError("benchmark_win64.py requires Windows")
    if not binary.is_file():
        raise FileNotFoundError(binary)
    output.mkdir(parents=True, exist_ok=False)

    server_port, push_port = free_port(), free_port()
    while push_port == server_port:
        push_port = free_port()
    config_path = output / "benchmark.config.json"
    config_path.write_text(json.dumps(make_config(server_port, push_port)), encoding="utf-8")
    child_log = (output / "child.log").open("wb")
    creation_flags = getattr(subprocess, "CREATE_NO_WINDOW", 0x08000000)
    process = subprocess.Popen(
        [str(binary), "--headless", "--config", str(config_path), "--multi"],
        stdin=subprocess.DEVNULL,
        stdout=child_log,
        stderr=subprocess.STDOUT,
        creationflags=creation_flags,
    )
    sockets: list[socket.socket] = []
    viewer_threads: list[threading.Thread] = []
    errors: list[str] = []
    errors_lock = threading.Lock()
    producer_finished = threading.Event()
    ready_events = [threading.Event() for _ in range(VIEWER_COUNT)]
    viewer_results = [{"bytes": 0, "first_byte_ms": None} for _ in range(VIEWER_COUNT)]
    try:
        process_start_cpu = process_cpu_seconds(process)
        startup_deadline = time.monotonic() + 20.0
        while True:
            if process.poll() is not None:
                raise RuntimeError(f"benchmark child exited during startup: {process.returncode}")
            try:
                connection = http.client.HTTPConnection("127.0.0.1", server_port, timeout=0.5)
                connection.request("GET", "/", headers={"Connection": "close"})
                response = connection.getresponse()
                response.read()
                connection.close()
                if response.status != 200:
                    raise RuntimeError(f"startup HTTP GET returned {response.status}")
                break
            except (OSError, http.client.HTTPException):
                if time.monotonic() >= startup_deadline:
                    raise RuntimeError("timed out waiting for loopback HTTP server")
                time.sleep(0.1)

        ping_ms = []
        for _ in range(PING_COUNT):
            begin = time.perf_counter()
            connection = http.client.HTTPConnection("127.0.0.1", server_port, timeout=2.0)
            connection.request("GET", "/", headers={"Connection": "close"})
            response = connection.getresponse()
            body = response.read()
            connection.close()
            if response.status != 200 or not body:
                raise RuntimeError(f"benchmark GET / failed: HTTP {response.status}")
            ping_ms.append((time.perf_counter() - begin) * 1000.0)

        def wait_until_stream_available() -> None:
            deadline = time.monotonic() + 5.0
            last_status = None
            while time.monotonic() < deadline:
                connection = http.client.HTTPConnection("127.0.0.1", server_port, timeout=0.5)
                try:
                    connection.request(
                        "HEAD", f"/stream/{CHANNEL_ID}", headers={"Connection": "close"}
                    )
                    response = connection.getresponse()
                    response.read()
                    last_status = response.status
                    if last_status == 200:
                        return
                    if last_status != 503:
                        raise RuntimeError(f"warmup HEAD received HTTP {last_status}")
                except (OSError, http.client.HTTPException):
                    if process.poll() is not None:
                        raise RuntimeError(
                            f"benchmark child exited while waiting for stream: {process.returncode}"
                        )
                finally:
                    connection.close()
                time.sleep(0.05)
            raise RuntimeError(f"stream did not become available after warmup (last HTTP {last_status})")

        def read_viewer(index: int) -> None:
            sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            sock.settimeout(1.0)
            sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            sockets.append(sock)
            connected_at = time.perf_counter()
            try:
                sock.connect(("127.0.0.1", server_port))
                sock.sendall(
                    f"GET /stream/{CHANNEL_ID} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n".encode("ascii")
                )
                response = http.client.HTTPResponse(sock)
                response.begin()
                if response.status != 200:
                    raise RuntimeError(f"viewer {index} received HTTP {response.status}")
                if not response.chunked:
                    raise RuntimeError(f"viewer {index} response was not HTTP chunked")
                ready_events[index].set()
                result = viewer_results[index]
                producer_deadline = None
                while True:
                    try:
                        block = response.read(CHUNK_SIZE)
                    except socket.timeout:
                        if producer_finished.is_set():
                            if producer_deadline is None:
                                producer_deadline = time.monotonic() + 5.0
                            elif time.monotonic() >= producer_deadline:
                                raise RuntimeError(f"viewer {index} timed out waiting for stream end")
                        continue
                    if not block:
                        if not producer_finished.is_set():
                            raise RuntimeError(f"viewer {index} disconnected before producer completion")
                        if response.fp is not None:
                            raise RuntimeError(f"viewer {index} did not receive the chunk terminator")
                        break
                    if result["first_byte_ms"] is None:
                        result["first_byte_ms"] = (time.perf_counter() - connected_at) * 1000.0
                    result["bytes"] += len(block)
                    if block != b"B" * len(block):
                        raise RuntimeError(f"viewer {index} received unexpected payload")
            except Exception as exc:  # surfaced in JSON after every worker is joined
                with errors_lock:
                    errors.append(str(exc))
                ready_events[index].set()
            finally:
                try:
                    sock.close()
                except OSError:
                    pass

        push = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        sockets.append(push)
        push.settimeout(3.0)
        push.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        push.connect(("127.0.0.1", push_port))
        push.sendall(
            b"POST / HTTP/1.1\r\nHost: 127.0.0.1\r\n"
            b"Content-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\n\r\n"
        )
        status, _ = read_http_headers(push)
        if status != 200:
            raise RuntimeError(f"push source received HTTP {status}")
        payload = b"B" * CHUNK_SIZE
        warmup_wire = f"{CHUNK_SIZE:x}\r\n".encode("ascii") + payload + b"\r\n"
        push.sendall(warmup_wire)
        wait_until_stream_available()

        for index in range(VIEWER_COUNT):
            thread = threading.Thread(target=read_viewer, args=(index,), name=f"viewer-{index}")
            thread.start()
            viewer_threads.append(thread)
        for event in ready_events:
            if not event.wait(5.0):
                raise RuntimeError("timed out waiting for viewer response headers")
        if errors:
            raise RuntimeError("; ".join(errors))

        cpu_before_stream = process_cpu_seconds(process)
        stream_cpu_start = cpu_before_stream
        stream_start = time.perf_counter()
        duration = STREAM_SECONDS
        end_at = stream_start + duration
        next_at = stream_start
        sent_bytes = 0
        while next_at < end_at:
            now = time.perf_counter()
            if now < next_at:
                time.sleep(next_at - now)
            push.sendall(warmup_wire)
            sent_bytes += CHUNK_SIZE
            next_at = stream_start + sent_bytes / RATE_BYTES_PER_SECOND
            if process.poll() is not None:
                raise RuntimeError(f"benchmark child exited during streaming: {process.returncode}")
        producer_finished.set()
        push.sendall(b"0\r\n\r\n")
        push.shutdown(socket.SHUT_WR)
        push.close()
        elapsed = time.perf_counter() - stream_start
        for thread in viewer_threads:
            thread.join(timeout=5.0)
        if any(thread.is_alive() for thread in viewer_threads):
            raise RuntimeError("viewer worker did not stop within timeout")
        if errors:
            raise RuntimeError("; ".join(errors))
        expected_bytes = sent_bytes + CHUNK_SIZE
        for index, result in enumerate(viewer_results):
            if result["bytes"] != expected_bytes:
                raise RuntimeError(
                    f"viewer {index} received {result['bytes']} of {expected_bytes} expected bytes"
                )

        stream_cpu_end = process_cpu_seconds(process)
        return {
            "windows": {"release": platform.release(), "version": platform.version(), "machine": platform.machine()},
            "binary": str(binary),
            "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
            "viewers": VIEWER_COUNT,
            "ping_count": PING_COUNT,
            "http_get_latency_ms": {
                "p50": statistics.median(ping_ms),
                "p95": sorted(ping_ms)[max(0, int(len(ping_ms) * 0.95 + 0.5) - 1)],
            },
            "stream": {
                "target_bytes_per_second": RATE_BYTES_PER_SECOND,
                "chunk_bytes": CHUNK_SIZE,
                "elapsed_seconds": elapsed,
                "warmup_bytes": CHUNK_SIZE,
                "source_bytes": sent_bytes,
                "viewers": viewer_results,
            },
            "child_process_cpu_seconds": {
                "startup": max(0.0, cpu_before_stream - process_start_cpu),
                "stream": max(0.0, stream_cpu_end - stream_cpu_start),
            },
            "worker_errors": errors,
        }
    finally:
        producer_finished.set()
        for sock in sockets:
            try:
                sock.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            try:
                sock.close()
            except OSError:
                pass
        for thread in viewer_threads:
            thread.join(timeout=2.0)
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5.0)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5.0)
        child_log.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path, help="absolute path to the PeerCastStation executable")
    parser.add_argument("--output", required=True, type=Path, help="new output directory for config, log, and result.json")
    args = parser.parse_args()
    try:
        result = run(args.binary.resolve(), args.output.resolve())
        (args.output.resolve() / "result.json").write_text(
            json.dumps(result, indent=2), encoding="utf-8"
        )
        print(json.dumps(result, indent=2))
        return 0
    except Exception as exc:
        print(json.dumps({"error": str(exc), "worker_errors": []}, indent=2), file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
