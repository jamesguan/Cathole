"""Black-box tests, standard library only. All sockets are loopback-only.
Run: python tests/e2e.py [path/to/cathole] [--loss 0.01] [--ipv6]
"""
import concurrent.futures
import json
import os
from pathlib import Path
import random
import select
import socket
import subprocess
import sys
import tempfile
import threading
import time
import argparse
import re
import base64

parser = argparse.ArgumentParser()
parser.add_argument("binary", nargs="?", default="target/debug/cathole.exe" if os.name == "nt" else "target/debug/cathole")
parser.add_argument("--loss", type=float, default=0)
parser.add_argument("--ipv6", action="store_true")
args = parser.parse_args()
binary = str(Path(args.binary).resolve())
host = "::1" if args.ipv6 else "127.0.0.1"
family = socket.AF_INET6 if args.ipv6 else socket.AF_INET

def address(port):
    return f"[{host}]:{port}" if args.ipv6 else f"{host}:{port}"

def sock(kind):
    s = socket.socket(family, kind)
    s.bind((host, 0))
    return s

def free_port():
    with sock(socket.SOCK_STREAM) as s:
        return s.getsockname()[1]

stop = threading.Event()
tcp_echo = sock(socket.SOCK_STREAM)
tcp_echo.listen()
tcp_echo.settimeout(0.2)
udp_echo = sock(socket.SOCK_DGRAM)
udp_echo.settimeout(0.2)

def echo_tcp(conn):
    with conn:
        conn.settimeout(20)
        try:
            while True:
                chunk = conn.recv(65536)
                if not chunk:
                    # Response after half-close tests directional FIN propagation.
                    conn.sendall(b"END")
                    return
                conn.sendall(chunk)
        except OSError:
            pass

def serve_tcp():
    while not stop.is_set():
        try:
            conn, _ = tcp_echo.accept()
            threading.Thread(target=echo_tcp, args=(conn,), daemon=True).start()
        except socket.timeout:
            pass

def serve_udp():
    while not stop.is_set():
        try:
            data, source = udp_echo.recvfrom(65536)
            udp_echo.sendto(data, source)
        except socket.timeout:
            pass

threading.Thread(target=serve_tcp, daemon=True).start()
threading.Thread(target=serve_udp, daemon=True).start()

class Relay:
    """User-space UDP impairment, reproducible loss/reordering in both directions."""
    def __init__(self, server_port):
        self.front = sock(socket.SOCK_DGRAM)
        self.back = sock(socket.SOCK_DGRAM)
        self.target = (host, server_port)
        self.peer = None
        self.running = True
        self.rng = random.Random(721)
        self.dropped = 0
        self.rebind_requested = False
        self.thread = threading.Thread(target=self.run, daemon=True)
        self.thread.start()

    def run(self):
        delayed = []
        while self.running:
            if self.rebind_requested:
                self.back.close()
                self.back = sock(socket.SOCK_DGRAM)
                self.rebind_requested = False
            readable, _, _ = select.select([self.front, self.back], [], [], 0.002)
            for s in readable:
                try:
                    data, source = s.recvfrom(65536)
                except ConnectionResetError:
                    continue  # Windows reports ICMP from the intentionally killed server.
                if s is self.front:
                    self.peer = source
                    target, output = self.target, self.back
                else:
                    target, output = self.peer, self.front
                if target is None:
                    continue
                if self.rng.random() < args.loss:
                    self.dropped += 1
                    continue
                # Delay a few packets to exercise reordering when loss is enabled.
                wait = 0.015 if args.loss and self.rng.random() < 0.05 else 0
                delayed.append((time.monotonic() + wait, output, data, target))
            now = time.monotonic()
            pending = []
            for at, output, data, target in delayed:
                if at <= now:
                    try:
                        output.sendto(data, target)
                    except OSError:
                        pass  # Old mapping or a peer deliberately stopped by this test.
                else:
                    pending.append((at, output, data, target))
            delayed = pending

    def close(self):
        self.running = False
        self.thread.join(2)
        self.front.close()
        self.back.close()

processes = []
logs = []

def launch(config):
    log = open(str(config) + f".{len(logs)}.log", "w+")
    logs.append(log)
    p = subprocess.Popen([binary, str(config)], stdout=log, stderr=subprocess.STDOUT, env={**os.environ, "RUST_LOG": "info"})
    processes.append(p)
    return p

def terminate(p):
    p.terminate()
    p.wait(timeout=10)

def tcp_roundtrip(port, payload=b"hello", timeout=20):
    with socket.create_connection((host, port), timeout=timeout) as s:
        s.settimeout(timeout)
        # Read concurrently to avoid a test-side send/receive deadlock for bulk echo.
        def send():
            s.sendall(payload)
            s.shutdown(socket.SHUT_WR)
        t = threading.Thread(target=send)
        t.start()
        data = bytearray()
        while True:
            part = s.recv(65536)
            if not part:
                break
            data.extend(part)
        t.join(timeout)
        assert data == payload + b"END", (len(data), len(payload))

def ready(port, timeout=45):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        try:
            tcp_roundtrip(port, timeout=2)
            return
        except (OSError, AssertionError):
            time.sleep(0.15)
    raise AssertionError("tunnel did not become ready")

def udp_roundtrip(port, payload):
    with sock(socket.SOCK_DGRAM) as s:
        s.settimeout(0.7 if args.loss else 3)
        s.sendto(payload, (host, port))
        try:
            data, _ = s.recvfrom(65536)
        except socket.timeout:
            assert args.loss, "UDP response lost on lossless test"
            return False
        assert data == payload
        return True

work = Path(tempfile.mkdtemp(prefix="e2e-", dir="target"))
identity = work / "identity"
subprocess.run([binary, "--init", str(identity)], check=True, stdout=subprocess.DEVNULL)
server_file, client_file = identity / "server.toml", identity / "client.toml"
server_port, public_port = free_port(), free_port()
relay = Relay(server_port)
server_text = server_file.read_text().replace("127.0.0.1:2333", address(server_port)).replace("127.0.0.1:5202", address(public_port)).replace("keep_alive_interval = 15", "keep_alive_interval = 2").replace("max_idle_timeout = 60", "max_idle_timeout = 8")
client_text = client_file.read_text().replace("127.0.0.1:2333", address(relay.front.getsockname()[1])).replace("keep_alive_interval = 15", "keep_alive_interval = 2").replace("max_idle_timeout = 60", "max_idle_timeout = 8")
client_text = client_text.replace('local_addr = "127.0.0.1:5201"', f'local_addr = "{address(tcp_echo.getsockname()[1])}"', 1)
client_text = client_text.replace('local_addr = "127.0.0.1:5201"', f'local_addr = "{address(udp_echo.getsockname()[1])}"')
server_file.write_text(server_text)
client_file.write_text(client_text)
started = time.monotonic()
try:
    for p in [server_file, client_file]:
        subprocess.run([binary, str(p), "--check"], check=True, stdout=subprocess.DEVNULL)
    server = launch(server_file)
    # No listener may open for an invalid token or an incorrect Noise identity.
    alternate = work / "alternate"
    subprocess.run([binary, "--init", str(alternate)], check=True, stdout=subprocess.DEVNULL)
    for field, replacement in [("default_token", "wrong-token"), ("remote_public_key", base64.b64encode(os.urandom(32)).decode()), ("trusted_root", (alternate / "server.der").resolve().as_posix())]:
        bad_file = identity / f"bad-{field}.toml"
        bad_file.write_text(re.sub(rf'{field} = "[^"]+"', f'{field} = "{replacement}"', client_text))
        bad = launch(bad_file)
        bad_log = logs[-1]
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            bad_log.seek(0)
            if "tunnel disconnected" in bad_log.read():
                break
            time.sleep(0.1)
        else:
            raise AssertionError(f"bad {field} handshake did not terminate")
        try:
            probe = socket.create_connection((host, public_port), timeout=0.2)
        except OSError:
            pass
        else:
            probe.close()
            raise AssertionError("unauthorized client opened public service")
        terminate(bad)
    client = launch(client_file)
    ready(public_port)
    relay.rebind_requested = True
    time.sleep(0.1)
    tcp_roundtrip(public_port, b"same session after NAT port rebinding")
    tcp_roundtrip(public_port, os.urandom(2 * 1024 * 1024))
    with concurrent.futures.ThreadPoolExecutor(max_workers=24) as pool:
        list(pool.map(lambda i: tcp_roundtrip(public_port, bytes([i]) * 32768), range(24)))
    udp_delivered = sum(udp_roundtrip(public_port, os.urandom(n)) for n in [0, 1, 999, 1000, 1001, 2000, 8192, 65507])
    assert udp_delivered >= (3 if args.loss else 8)
    # Concurrent source endpoints must never receive each other's packets.
    with concurrent.futures.ThreadPoolExecutor(max_workers=16) as pool:
        list(pool.map(lambda i: udp_roundtrip(public_port, bytes([i]) * 200), range(16)))
    # Invalid hot reload keeps the running config.
    client_file.write_text(client_text + '\ninvalid_setting = true\n')
    time.sleep(1.3)
    tcp_roundtrip(public_port)
    client_file.write_text(client_text)
    time.sleep(1.3)
    ready(public_port)
    # Server restart forces fresh TLS + Noise and service registration.
    terminate(server)
    server = launch(server_file)
    ready(public_port)
    tcp_roundtrip(public_port, b"after reconnect")
    # Both sides add a new service through watched config files.
    extra_port = free_port()
    server_file.write_text(server_text + f'\n[server.services.extra]\nbind_addr = "{address(extra_port)}"\n')
    client_file.write_text(client_text + f'\n[client.services.extra]\nlocal_addr = "{address(tcp_echo.getsockname()[1])}"\n')
    ready(extra_port)
    tcp_roundtrip(extra_port, b"hot added service")
    server_file.write_text(server_text)
    client_file.write_text(client_text)
    time.sleep(2)
    ready(public_port)
    try:
        probe = socket.create_connection((host, extra_port), timeout=.5)
    except OSError:
        pass
    else:
        probe.close()
        raise AssertionError("removed service remains exposed")
    print(json.dumps({"result": "PASS", "loss": args.loss, "ipv6": args.ipv6, "seconds": round(time.monotonic()-started, 2), "relay_dropped": relay.dropped, "udp_sizes_delivered": udp_delivered, "checks": ["bulk TCP integrity", "TCP half-close", "24 concurrent streams", "UDP boundaries/fragmentation/empty packets", "UDP source isolation", "wrong token/Noise key/TLS root", "NAT port rebinding", "invalid reload", "reconnect", "hot add/remove"]}))
except BaseException:
    for log in logs:
        log.flush()
        log.seek(0)
        print(log.read()[-6000:], file=sys.stderr)
    raise
finally:
    for p in processes:
        if p.poll() is None:
            terminate(p)
    relay.close()
    stop.set()
    for log in logs:
        log.close()
