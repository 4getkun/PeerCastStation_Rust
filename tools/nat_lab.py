#!/usr/bin/env python3
"""Opt-in Linux network-namespace lab for the real Rust binary; requires root."""
import argparse
import http.client
import http.server
import json
import math
import os
from pathlib import Path
import signal
import socket
import struct
import subprocess
import sys
import threading
import time
import uuid
import xml.etree.ElementTree as ET

LAN = "10.203.0.2"
GATEWAY = "10.203.0.1"
WAN = "198.18.0.1"
OUTSIDE = "198.18.0.2"
PORT = 17144
CHANNEL = "0123456789abcdef0123456789abcdef"
MARKER = b"PEERCAST_ISOLATED_NAT_LAB\n"
SERVICE = "urn:schemas-upnp-org:service:WANIPConnection:1"
SOAP = "http://schemas.xmlsoap.org/soap/envelope/"


def command(*args, **kwargs):
    result = subprocess.run([str(a) for a in args], capture_output=True, text=True, **kwargs)
    if result.returncode:
        raise RuntimeError(f"command {args!r} failed ({result.returncode}): {result.stderr[-3000:]}")
    return result


def verify_gateway_namespace(namespace):
    assert namespace.startswith("pcs-router-") and "/" not in namespace
    assert os.stat("/proc/self/ns/net").st_ino == os.stat("/run/netns/" + namespace).st_ino
    assert os.stat("/proc/self/ns/net").st_ino != os.stat("/proc/1/ns/net").st_ino, "refusing to modify the host network namespace"


def gateway_main(events, namespace):
    verify_gateway_namespace(namespace)
    stop = threading.Event()
    lock = threading.RLock()
    mapping = {}
    rule = ["iptables", "-t", "nat", "-A", "PREROUTING", "-i", "wan0", "-d", WAN,
            "-p", "tcp", "--dport", str(PORT), "-j", "DNAT", "--to-destination", f"{LAN}:{PORT}"]

    def event(name, **fields):
        with open(events, "a", encoding="utf8") as file:
            file.write(json.dumps({"time": time.time(), "event": name, **fields}) + "\n")

    def remove(reason):
        if mapping:
            delete = list(rule)
            delete[3] = "-D"
            command(*delete)
            event(reason, description=mapping["description"])
            mapping.clear()

    def expire():
        if mapping and time.monotonic() >= mapping["expires"]:
            remove("expired")

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def respond(self, body, status=200):
            encoded = body.encode()
            self.send_response(status)
            self.send_header("Content-Type", "text/xml")
            self.send_header("Content-Length", str(len(encoded)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(encoded)

        def do_GET(self):
            if self.path == "/root.xml":
                self.respond(f'''<root xmlns="urn:schemas-upnp-org:device-1-0"><specVersion><major>1</major><minor>0</minor></specVersion><device><deviceType>urn:schemas-upnp-org:device:InternetGatewayDevice:1</deviceType><friendlyName>Isolated test gateway</friendlyName><manufacturer>Local test fixture</manufacturer><modelName>NAT lab</modelName><UDN>uuid:pcs-nat-lab</UDN><serviceList><service><serviceType>{SERVICE}</serviceType><serviceId>urn:upnp-org:serviceId:WANIPConn1</serviceId><SCPDURL>/schema.xml</SCPDURL><controlURL>/control</controlURL><eventSubURL>/events</eventSubURL></service></serviceList></device></root>''')
            elif self.path == "/schema.xml":
                fields = {
                    "AddPortMapping": ["NewRemoteHost", "NewExternalPort", "NewProtocol", "NewInternalPort", "NewInternalClient", "NewEnabled", "NewPortMappingDescription", "NewLeaseDuration"],
                    "DeletePortMapping": ["NewRemoteHost", "NewExternalPort", "NewProtocol"],
                    "GetSpecificPortMappingEntry": ["NewRemoteHost", "NewExternalPort", "NewProtocol"],
                    "GetGenericPortMappingEntry": ["NewPortMappingIndex"],
                    "GetExternalIPAddress": [],
                }
                actions = "".join("<action><name>" + name + "</name><argumentList>" + "".join(
                    f"<argument><name>{field}</name><direction>in</direction><relatedStateVariable>Dummy</relatedStateVariable></argument>" for field in arguments
                ) + "</argumentList></action>" for name, arguments in fields.items())
                self.respond(f'<scpd xmlns="urn:schemas-upnp-org:service-1-0"><specVersion><major>1</major><minor>0</minor></specVersion><actionList>{actions}</actionList><serviceStateTable><stateVariable sendEvents="no"><name>Dummy</name><dataType>string</dataType></stateVariable></serviceStateTable></scpd>')
            else:
                self.respond("not found", 404)

        def do_POST(self):
            length = int(self.headers.get("Content-Length", "0"))
            if not 0 < length < 16384:
                return self.respond("invalid length", 400)
            try:
                root = ET.fromstring(self.rfile.read(length))
                body = root.find(f"{{{SOAP}}}Body")
                action_node = list(body)[0]
                action = action_node.tag.split("}")[-1]
                values = {node.tag.split("}")[-1]: node.text or "" for node in action_node}
                result = ""
                with lock:
                    expire()
                    code = None
                    if action == "AddPortMapping":
                        valid = values.get("NewInternalClient") == LAN and values.get("NewInternalPort") == str(PORT) and values.get("NewExternalPort") == str(PORT) and values.get("NewProtocol") == "TCP"
                        lease = int(values.get("NewLeaseDuration", "0"))
                        if not valid or not 1 <= lease <= 60:
                            code = 402
                        elif mapping and mapping["description"] != values["NewPortMappingDescription"]:
                            code = 718
                        else:
                            renewing = bool(mapping)
                            if not renewing:
                                command(*rule)
                            mapping.update(description=values["NewPortMappingDescription"], expires=time.monotonic() + lease, lease=lease)
                            event("renewed" if renewing else "added", lease=lease, description=mapping["description"])
                    elif action == "DeletePortMapping":
                        if mapping:
                            remove("deleted")
                        else:
                            code = 714
                    elif action == "GetExternalIPAddress":
                        result = f"<NewExternalIPAddress>{WAN}</NewExternalIPAddress>"
                    elif action in ("GetSpecificPortMappingEntry", "GetGenericPortMappingEntry"):
                        if not mapping:
                            code = 713 if action == "GetGenericPortMappingEntry" else 714
                        elif action == "GetGenericPortMappingEntry" and values.get("NewPortMappingIndex") != "0":
                            code = 713
                        elif action == "GetSpecificPortMappingEntry" and (values.get("NewExternalPort") != str(PORT) or values.get("NewProtocol") != "TCP"):
                            code = 714
                        else:
                            result = f"<NewInternalPort>{PORT}</NewInternalPort><NewInternalClient>{LAN}</NewInternalClient><NewEnabled>1</NewEnabled><NewPortMappingDescription>{mapping['description']}</NewPortMappingDescription><NewLeaseDuration>{max(1, math.ceil(mapping['expires']-time.monotonic()))}</NewLeaseDuration>"
                            if action == "GetGenericPortMappingEntry":
                                result = f"<NewRemoteHost></NewRemoteHost><NewExternalPort>{PORT}</NewExternalPort><NewProtocol>TCP</NewProtocol>" + result
                    else:
                        code = 401
                    if code:
                        response = f'<s:Envelope xmlns:s="{SOAP}"><s:Body><s:Fault><detail><UPnPError xmlns="urn:schemas-upnp-org:control-1-0"><errorCode>{code}</errorCode><errorDescription>Lab error</errorDescription></UPnPError></detail></s:Fault></s:Body></s:Envelope>'
                    else:
                        response = f'<s:Envelope xmlns:s="{SOAP}"><s:Body><u:{action}Response xmlns:u="{SERVICE}">{result}</u:{action}Response></s:Body></s:Envelope>'
                self.respond(response, 500 if code else 200)
            except (ValueError, ET.ParseError, IndexError, TypeError):
                self.respond("invalid SOAP", 400)

    def ssdp():
        udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        udp.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        udp.bind(("0.0.0.0", 1900))
        udp.setsockopt(socket.IPPROTO_IP, socket.IP_ADD_MEMBERSHIP, socket.inet_aton("239.255.255.250") + socket.inet_aton(GATEWAY))
        udp.settimeout(0.2)
        while not stop.is_set():
            try:
                packet, peer = udp.recvfrom(4096)
                if packet.startswith(b"M-SEARCH"):
                    reply = f"HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=60\r\nEXT:\r\nLOCATION: http://{GATEWAY}:1901/root.xml\r\nSERVER: Linux/1.0 UPnP/1.0 NAT-lab/1.0\r\nST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\nUSN: uuid:pcs-nat-lab::urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\n\r\n"
                    udp.sendto(reply.encode(), peer)
            except socket.timeout:
                pass
            with lock:
                expire()
        udp.close()

    httpd = http.server.ThreadingHTTPServer((GATEWAY, 1901), Handler)
    threading.Thread(target=ssdp, daemon=True).start()
    signal.signal(signal.SIGTERM, lambda *_: (stop.set(), threading.Thread(target=httpd.shutdown, daemon=True).start()))
    event("ready")
    try:
        httpd.serve_forever(poll_interval=0.1)
    finally:
        with lock:
            remove("fixture-stopped")


def atom(name, value):
    return name + struct.pack("<I", len(value)) + value


def helo():
    children = atom(b"sid\0", uuid.uuid4().bytes) + atom(b"agnt", b"isolated-NAT-client\0") + atom(b"ver\0", struct.pack("<I", 1218))
    return b"helo" + struct.pack("<I", 0x80000003) + children


def outside_client(expected):
    if expected == "closed":
        try:
            with socket.create_connection((WAN, PORT), 0.6):
                raise AssertionError("unexpected externally reachable port")
        except (ConnectionRefusedError, TimeoutError, OSError):
            print(json.dumps({"reachable": False}))
            return
    with socket.create_connection((WAN, PORT), 2) as sock:
        sock.settimeout(2)
        sock.sendall(b"GET /api/1 HTTP/1.0\r\n\r\n")
        response = sock.recv(4096)
        assert b"200 OK" in response, response[:200]
    connection = http.client.HTTPConnection(WAN, PORT, timeout=2)
    connection.request("POST", "/api/1", json.dumps({"jsonrpc": "2.0", "id": 1, "method": "getLog", "params": {}}), {"Content-Type": "application/json"})
    denied = json.loads(connection.getresponse().read())
    connection.close()
    assert denied["error"]["code"] == -32001, "external viewer must not read diagnostic logs"
    with socket.create_connection((WAN, PORT), 2) as sock:
        sock.settimeout(2)
        sock.sendall(b"pcp\n" + struct.pack("<II", 4, 1) + helo())
        pong = sock.recv(4096)
        assert pong.startswith(b"oleh") and b"sid\0" in pong, pong[:100]
    with socket.create_connection((WAN, PORT), 2) as sock:
        sock.settimeout(3)
        sock.sendall(f"GET /channel/{CHANNEL} HTTP/1.0\r\nx-peercast-pcp: 1\r\n\r\n".encode())
        data = b""
        while b"\r\n\r\n" not in data:
            part = sock.recv(4096)
            assert part
            data += part
        assert b"200 OK" in data[:200], data[:200]
        sock.sendall(helo())
        while MARKER not in data:
            part = sock.recv(16384)
            assert part, "relay ended before media"
            data += part
            assert len(data) < 1024 * 1024
        print(json.dumps({"reachable": True, "http": 200, "pong": True, "pcpMedia": True, "bytes": len(data), "unauthenticatedLogDenied": True}))


def read_logs(config_path):
    config = json.loads(Path(config_path).read_text())
    token = config["server"]["authToken"]
    connection = http.client.HTTPConnection("127.0.0.1", PORT, timeout=3)
    connection.request("POST", "/api/1", json.dumps({"jsonrpc": "2.0", "id": 1, "method": "getLog", "params": {"from": 0, "maxLines": 1000}}), {"Content-Type": "application/json", "Authorization": "Bearer " + token})
    result = json.loads(connection.getresponse().read())
    connection.close()
    assert "error" not in result, result
    assert token not in json.dumps(result)
    entries = result["result"]["entries"]
    assert any(row["target"] == "port-mapping" for row in entries)
    assert any(f"peer={OUTSIDE}:" in row["message"] for row in entries)
    print(json.dumps({"records": len(entries), "localAdminOnly": True, "credentialsRedacted": True}))


def injector():
    while True:
        try:
            with socket.create_connection(("127.0.0.1", 17146), 1) as sock:
                sock.sendall(b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n")
                while True:
                    sock.sendall(f"{len(MARKER):x}\r\n".encode() + MARKER + b"\r\n")
                    time.sleep(0.1)
        except OSError:
            time.sleep(0.1)


def run_lab(binary, output):
    assert sys.platform.startswith("linux") and os.geteuid() == 0, "Linux root required"
    binary, output = Path(binary).resolve(), Path(output).resolve()
    assert binary.is_file()
    output.mkdir(parents=True, exist_ok=True)
    assert not any(output.iterdir()), "use a fresh output directory for each lab run"
    script = Path(__file__).resolve()
    suffix = uuid.uuid4().hex[:6]
    names = {part: f"pcs-{part}-{suffix}" for part in ("lan", "router", "outside")}
    created, processes, checks = [], [], []
    events = output / "gateway-events.jsonl"
    token = uuid.uuid4().hex

    def ns(which, *args):
        return command("ip", "netns", "exec", names[which], *args)

    def spawn(which, *args, name):
        logfile = open(output / (name + ".stderr.log"), "w", encoding="utf8")
        process = subprocess.Popen(["ip", "netns", "exec", names[which], *map(str, args)], stdout=logfile, stderr=logfile)
        processes.append((process, logfile))
        return process

    def wait_for(predicate, timeout=10):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            if predicate():
                return
            time.sleep(0.1)
        raise AssertionError("lab condition timed out")

    def history():
        return [json.loads(line) for line in events.read_text().splitlines() if line] if events.exists() else []

    def check(name, expected):
        result = json.loads(ns("outside", sys.executable, script, "client", "--expected", expected).stdout)
        checks.append({"name": name, **result})
        print(json.dumps(checks[-1]), flush=True)

    def app_config(enabled, name):
        config = json.loads((output / "defaults.json").read_text())
        config["server"].update(bindAddress="0.0.0.0", port=PORT, allowRemoteAdmin=False, authToken=token)
        config["logging"].update(level="debug", logFile=str(output / (name + ".jsonl")), maxLogBytes=65536, maxBackups=2)
        config["runtime"]["portMapping"].update(enabled=enabled, leaseDurationSecs=8, renewalMarginSecs=3, discoveryTimeoutMs=2000, requestTimeoutMs=1000, retryBaseSecs=1, retryMaxSecs=2)
        config["runtime"]["channels"] = [{"channelId": CHANNEL, "name": "Isolated NAT lab", "containerType": "RAW", "source": {"type": "httpPush", "bindAddress": "127.0.0.1:17146", "authToken": None}}]
        filename = output / (name + ".config.json")
        filename.write_text(json.dumps(config))
        return filename

    def start_app(enabled, name):
        process = spawn("lan", binary, "--headless", "--config", app_config(enabled, name), name=name)
        time.sleep(0.6)
        assert process.poll() is None
        return process

    def stop_app(process, graceful=True):
        process.send_signal(signal.SIGINT if graceful else signal.SIGKILL)
        code = process.wait(timeout=6)
        assert code == (0 if graceful else -signal.SIGKILL), code

    try:
        command(binary, "--headless", "--config", output / "defaults.json", "--kill")
        for name in names.values():
            command("ip", "netns", "add", name)
            created.append(name)
            command("ip", "-n", name, "link", "set", "lo", "up")
        for first, second, first_ns, second_ns, first_name, second_name in [
            ("la" + suffix, "lr" + suffix, "lan", "router", "eth0", "lan0"),
            ("wa" + suffix, "wr" + suffix, "outside", "router", "eth0", "wan0"),
        ]:
            command("ip", "link", "add", first, "type", "veth", "peer", "name", second)
            command("ip", "link", "set", first, "netns", names[first_ns])
            command("ip", "link", "set", second, "netns", names[second_ns])
            ns(first_ns, "ip", "link", "set", first, "name", first_name)
            ns(second_ns, "ip", "link", "set", second, "name", second_name)
        for which, interface, address in [("lan", "eth0", LAN + "/24"), ("router", "lan0", GATEWAY + "/24"), ("router", "wan0", WAN + "/24"), ("outside", "eth0", OUTSIDE + "/24")]:
            ns(which, "ip", "addr", "add", address, "dev", interface)
            ns(which, "ip", "link", "set", interface, "up")
        ns("lan", "ip", "route", "add", "default", "via", GATEWAY)
        ns("outside", "ip", "route", "add", "default", "via", WAN)
        ns("router", "sysctl", "-w", "net.ipv4.ip_forward=1")
        ns("router", "iptables", "-P", "FORWARD", "DROP")
        ns("router", "iptables", "-A", "FORWARD", "-m", "conntrack", "--ctstate", "ESTABLISHED,RELATED", "-j", "ACCEPT")
        ns("router", "iptables", "-A", "FORWARD", "-i", "lan0", "-o", "wan0", "-j", "ACCEPT")
        ns("router", "iptables", "-A", "FORWARD", "-i", "wan0", "-o", "lan0", "-p", "tcp", "-d", LAN, "--dport", str(PORT), "-m", "conntrack", "--ctstate", "DNAT", "-j", "ACCEPT")
        ns("router", "iptables", "-t", "nat", "-A", "POSTROUTING", "-s", "10.203.0.0/24", "-o", "wan0", "-j", "MASQUERADE")
        spawn("router", sys.executable, script, "gateway", "--events", events, "--namespace", names["router"], name="gateway")
        wait_for(lambda: any(row["event"] == "ready" for row in history()))
        spawn("lan", sys.executable, script, "inject", name="inject")
        app = start_app(False, "unmapped")
        check("before-registration", "closed")
        stop_app(app)
        app = start_app(True, "mapped")
        wait_for(lambda: any(row["event"] == "added" for row in history()))
        check("after-registration-http-pong-pcp", "open")
        wait_for(lambda: sum(row["event"] == "renewed" for row in history()) >= 2, 15)
        check("after-two-renewals", "open")
        debug = json.loads(ns("lan", sys.executable, script, "logs", "--config", output / "mapped.config.json").stdout)
        checks.append({"name": "local-admin-debug-log-retrieval", **debug})
        print(json.dumps(checks[-1]), flush=True)
        stop_app(app)
        wait_for(lambda: any(row["event"] == "deleted" for row in history()))
        app = start_app(False, "after-clean-shutdown")
        check("after-clean-shutdown-unmapped-server-running", "closed")
        stop_app(app)
        added_before = sum(row["event"] == "added" for row in history())
        app = start_app(True, "crash")
        wait_for(lambda: sum(row["event"] == "added" for row in history()) > added_before)
        check("before-crash", "open")
        stop_app(app, False)
        app = start_app(False, "after-crash")
        wait_for(lambda: any(row["event"] == "expired" for row in history()), 12)
        check("finite-lease-expired-server-still-running", "closed")
        stop_app(app)
        for log in output.glob("*.jsonl"):
            assert token not in log.read_text(), "credential leaked in log"
        result = {"passed": True, "topology": {"lan": LAN, "routerLan": GATEWAY, "routerWan": WAN, "outside": OUTSIDE}, "checks": checks, "events": history(), "scope": "isolated Linux network namespaces and kernel NAT; no physical-router or Windows Firewall changes"}
        (output / "results.json").write_text(json.dumps(result, indent=2))
        print(json.dumps({"passed": True, "checks": len(checks)}), flush=True)
    finally:
        for process, logfile in reversed(processes):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=3)
            logfile.close()
        for name in reversed(created):
            command("ip", "netns", "delete", name)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="mode", required=True)
    run = sub.add_parser("run")
    run.add_argument("--binary", required=True)
    run.add_argument("--output", required=True)
    gateway = sub.add_parser("gateway")
    gateway.add_argument("--events", required=True)
    gateway.add_argument("--namespace", required=True)
    client = sub.add_parser("client")
    client.add_argument("--expected", choices=["open", "closed"], required=True)
    logs = sub.add_parser("logs")
    logs.add_argument("--config", required=True)
    sub.add_parser("inject")
    args = parser.parse_args()
    if args.mode == "run":
        run_lab(args.binary, args.output)
    elif args.mode == "gateway":
        gateway_main(args.events, args.namespace)
    elif args.mode == "client":
        outside_client(args.expected)
    elif args.mode == "logs":
        read_logs(args.config)
    else:
        injector()
