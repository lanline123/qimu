#!/usr/bin/env python3
"""Probe qimu-server LAN discovery: broadcast QIMU_DISCOVER_V1 on UDP 8731
(plus a directed send to 127.0.0.1) and print every JSON hello received."""
import json
import socket
import sys
import time

MAGIC = b"QIMU_DISCOVER_V1"
PORT = 8731

sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
sock.bind(("0.0.0.0", 0))
sock.settimeout(0.3)
for target in ("255.255.255.255", "127.0.0.1"):
    try:
        sock.sendto(MAGIC, (target, PORT))
    except OSError as e:
        print(f"send to {target} failed: {e}", file=sys.stderr)

seen = {}
deadline = time.time() + 2.0
while time.time() < deadline:
    try:
        data, addr = sock.recvfrom(2048)
    except socket.timeout:
        continue
    try:
        hello = json.loads(data.decode("utf-8"))
    except Exception:
        continue
    if hello.get("app") == "qimu":
        seen[(addr[0], hello.get("port"))] = hello

for (ip, _port), hello in sorted(seen.items()):
    print(f"{ip}: {json.dumps(hello, ensure_ascii=False)}")
print(f"total: {len(seen)} server(s)")
sys.exit(0 if seen else 1)
