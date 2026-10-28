#!/usr/bin/env python3
"""A JSON-RPC proxy in front of the sequencer that fails on demand, for testing the app's failure
handling: `faultproxy.py [LISTEN_PORT] [UPSTREAM]` (default 3041 → http://127.0.0.1:3040).

Point a test wallet's `wallet_config.json` at http://127.0.0.1:3041, then write a mode to
`/tmp/bc-test/proxy.mode` (re-read on every request; missing = pass):

  pass         forward everything
  down         close every connection at once (connection refused/reset)
  hang         accept, never answer (until the mode changes)
  eat-send     forward sendTransaction to the sequencer, then drop the reply: the tx is in, the
               client sees an error (the case a retry turns into a double spend)
  reject-send  answer sendTransaction with HTTP 502 without forwarding it (nothing sent)
  slow:MS      forward everything after MS milliseconds

Each request's method and the mode applied go to /tmp/bc-test/proxy.log.
"""
import http.client
import json
import socket
import sys
import threading
import time
import urllib.parse

MODE_FILE = "/tmp/bc-test/proxy.mode"
LOG_FILE = "/tmp/bc-test/proxy.log"
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 3041
UPSTREAM = urllib.parse.urlparse(sys.argv[2] if len(sys.argv) > 2 else "http://127.0.0.1:3040")


def mode():
    try:
        with open(MODE_FILE) as f:
            return f.read().strip() or "pass"
    except OSError:
        return "pass"


def log(line):
    with open(LOG_FILE, "a") as f:
        f.write(time.strftime("%H:%M:%S ") + line + "\n")


def read_request(conn):
    data = b""
    while b"\r\n\r\n" not in data:
        chunk = conn.recv(65536)
        if not chunk:
            return None
        data += chunk
    head, body = data.split(b"\r\n\r\n", 1)
    lines = head.decode("latin-1").split("\r\n")
    headers = {}
    for line in lines[1:]:
        k, _, v = line.partition(":")
        headers[k.strip().lower()] = v.strip()
    length = int(headers.get("content-length", "0"))
    while len(body) < length:
        chunk = conn.recv(65536)
        if not chunk:
            return None
        body += chunk
    return lines[0], headers, body


def method_of(body):
    try:
        j = json.loads(body)
        return ",".join(r.get("method", "?") for r in j) if isinstance(j, list) else j.get("method", "?")
    except Exception:
        return "?"


def forward(path, headers, body):
    up = http.client.HTTPConnection(UPSTREAM.hostname, UPSTREAM.port, timeout=120)
    up.request("POST", path, body=body, headers={"Content-Type": headers.get("content-type", "application/json")})
    r = up.getresponse()
    payload = r.read()
    up.close()
    return r.status, payload


def handle(conn):
    try:
        while True:
            req = read_request(conn)
            if req is None:
                return
            line, headers, body = req
            path = line.split(" ")[1] if " " in line else "/"
            m, rpc = mode(), method_of(body)
            log(f"{rpc} [{m}]")
            if m == "down":
                return
            if m == "hang":
                while mode() == "hang":
                    time.sleep(0.2)
                return
            if m.startswith("slow:"):
                time.sleep(int(m[5:]) / 1000)
            if rpc == "sendTransaction" and m == "reject-send":
                conn.sendall(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                continue
            status, payload = forward(path, headers, body)
            if rpc == "sendTransaction" and m == "eat-send":
                log(f"  sent upstream ({status}: {payload[:120]!r}), reply dropped")
                return
            conn.sendall(f"HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {len(payload)}\r\n\r\n".encode() + payload)
    except Exception as e:
        log(f"  error: {e}")
    finally:
        try:
            conn.close()
        except Exception:
            pass


def main():
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("127.0.0.1", PORT))
    s.listen(64)
    log(f"listening on {PORT} → {UPSTREAM.geturl()}")
    while True:
        conn, _ = s.accept()
        if mode() == "down":
            conn.close()
            continue
        threading.Thread(target=handle, args=(conn,), daemon=True).start()


if __name__ == "__main__":
    main()
