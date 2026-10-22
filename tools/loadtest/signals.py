#!/usr/bin/env python3
"""Load check for app signal ingest (POST /ingest/v1/signals).

    python3 tools/loadtest/signals.py --url http://kestrel:3002 --setup --rate 3000 --seconds 30

Open loop: requests are scheduled at a fixed rate and their latency is
measured from when they were due, so a slow server can't hide behind the
generator. Each request is a batch of 50-100 signals built like KestrelKit's
(sorted keys, `sdk` first, gzip-compressed) from 20,000 installs on five app
versions, with sessions, 20 signal types, payloads on a third of them and
5 % TestFlight signals, and a random client address in X-Forwarded-For
(start Kestrel with KK_TRUST_PROXY=1 and KK_INGEST_IP_RATE_LIMIT=0). `--rate`
counts signals per second.

`--setup` makes the admin account and a project on a fresh instance and uses
its key (otherwise pass `--key`); afterwards, once the rollup has run, it
prints what the App tab counts, to compare with what was accepted. Python
standard library only; HTTP/1.1 keep-alive connections over several
processes.
"""

from __future__ import annotations

import argparse
import asyncio
import gzip
import http.cookiejar
import json
import multiprocessing
import random
import re
import time
import urllib.parse
import urllib.request
import uuid
from datetime import datetime, timezone

TYPES = ["App.launched", "Session.started", "Paywall.shown", "Paywall.closed", "Export.finished",
         "Document.opened", "Document.saved", "Settings.opened", "Search.used", "Share.tapped",
         "Onboarding.step", "Onboarding.done", "Purchase.started", "Purchase.done", "Sync.failed",
         "Widget.added", "Theme.changed", "Feedback.sent", "Tip.shown", "Tip.closed"]
VERSIONS = [("1.4.0", "57"), ("1.4.0", "58"), ("1.3.2", "51"), ("1.3.1", "49"), ("1.2.0", "40")]
MODELS = ["iPhone16,2", "iPhone15,3", "iPhone14,7", "iPad13,4", "Mac14,2", "iPhone17,1"]
LOCALES = ["nb_NO", "en_US", "de_DE", "sv_SE", "en_GB", "fr_FR", "ja_JP"]
INSTALLS = [str(uuid.UUID(int=random.Random(7).getrandbits(128) ^ i)).upper() for i in range(20_000)]


def stamp(t: float) -> str:
    return datetime.fromtimestamp(t, timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.") + f"{int(t * 1000) % 1000:03d}Z"


def signal(now: float) -> dict:
    i = random.randrange(len(INSTALLS))
    version, build = VERSIONS[i % len(VERSIONS)]
    model = MODELS[i % len(MODELS)]
    kind = random.choice(TYPES)
    payload = {}
    if random.random() < 0.33:
        payload = {"plan": random.choice(["free", "pro", "team"]), "source": random.choice(["onboarding", "settings", "deeplink"]),
                   "count": str(random.randrange(50))}
    return {
        "app": {"build": build, "bundle_id": "com.example.load", "version": version},
        "device": {"locale": LOCALES[i % len(LOCALES)], "model": model,
                   "os_version": "18.2" if model.startswith("iP") else "15.1",
                   "platform": "iPadOS" if model.startswith("iPad") else "macOS" if model.startswith("Mac") else "iOS"},
        "flags": {"debug": False, "simulator": False, "testflight": random.random() < 0.05},
        "install_id": INSTALLS[i],
        "payload": payload,
        "session_id": f"{INSTALLS[i][:8]}-SESS-{int(now) // 1800}",
        "time": stamp(now - random.random() * 30),
        "type": kind,
    }


def body() -> tuple[bytes, int]:
    now = time.time()
    n = random.randint(50, 100)
    signals = ",".join(json.dumps(signal(now), sort_keys=True, separators=(",", ":")) for _ in range(n))
    raw = ('{"sdk":{"name":"kestrelkit","version":"0.1.0"},"sent_at":"' + stamp(now) + '","signals":[' + signals + "]}").encode()
    return gzip.compress(raw, 6), n


async def worker(host: str, port: int, key: str, due: asyncio.Queue, out: list):
    reader = writer = None
    while True:
        t = await due.get()
        if t is None:
            break
        ip = f"10.{random.randrange(256)}.{random.randrange(256)}.{random.randrange(1, 255)}"
        b, n = body()
        req = (f"POST /ingest/v1/signals HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\n"
               f"Content-Encoding: gzip\r\nX-Kestrel-Key: {key}\r\nUser-Agent: KestrelKit/0.1.0\r\n"
               f"X-Forwarded-For: {ip}\r\nContent-Length: {len(b)}\r\n\r\n").encode() + b
        try:
            if writer is None:
                reader, writer = await asyncio.open_connection(host, port)
            writer.write(req)
            await writer.drain()
            head = await reader.readuntil(b"\r\n\r\n")
            status = int(head.split(b" ", 2)[1])
            length = 0
            for line in head.split(b"\r\n"):
                if line.lower().startswith(b"content-length:"):
                    length = int(line.split(b":", 1)[1])
            accepted = 0
            if length:
                data = await reader.readexactly(length)
                if status == 202:
                    accepted = json.loads(data).get("accepted", 0)
            out.append((status, time.monotonic() - t, n, accepted))
        except Exception:  # noqa: BLE001 - a broken connection is an error, then a new one
            out.append((0, time.monotonic() - t, n, 0))
            writer = None


async def run(args, rate: float, seed: int) -> list:
    random.seed(seed)
    u = urllib.parse.urlparse(args.url)
    due: asyncio.Queue = asyncio.Queue()
    out: list = []
    workers = [asyncio.create_task(worker(u.hostname, u.port or 80, args.key, due, out))
               for _ in range(args.connections)]
    start = time.monotonic()
    requests_per_second = rate / 75  # batches of 50-100 signals
    n = int(requests_per_second * args.seconds)
    for i in range(n):
        t = start + i / requests_per_second
        delay = t - time.monotonic()
        if delay > 0:
            await asyncio.sleep(delay)
        due.put_nowait(t)
    for _ in workers:
        due.put_nowait(None)
    await asyncio.gather(*workers)
    return out


def child(args, rate, seed, q):
    q.put(asyncio.run(run(args, rate, seed)))


class Browser:
    """A signed-in session for --setup and the check afterwards."""

    def __init__(self, url: str):
        self.url = url.rstrip("/")
        self.opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))

    def get(self, path: str) -> str:
        return self.opener.open(self.url + path, timeout=30).read().decode()

    def post(self, path: str, form: dict) -> str:
        req = urllib.request.Request(self.url + path, data=urllib.parse.urlencode(form).encode(),
                                     headers={"Origin": self.url, "Content-Type": "application/x-www-form-urlencoded"})
        return self.opener.open(req, timeout=30).read().decode()


def setup(url: str) -> tuple[Browser, int, str]:
    b = Browser(url)
    password = "Quiet-Harbor-Lamp-7319!"
    b.post("/setup", {"username": "bench", "password": password, "confirm": password, "site_name": "Load", "timezone": "UTC"})
    b.post("/login", {"username": "bench", "password": password})
    b.post("/projects", {"name": "Load app", "platform": "apple"})
    projects = b.get("/projects")
    pid = int(re.findall(r"/projects/(\d+)", projects)[-1])
    key = re.search(r"\b([0-9a-f]{32})\b", b.get(f"/projects/{pid}/settings")).group(1)
    return b, pid, key


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--url", required=True)
    p.add_argument("--key")
    p.add_argument("--setup", action="store_true", help="make the admin and a project on a fresh instance")
    p.add_argument("--rate", type=float, default=3000, help="signals per second")
    p.add_argument("--seconds", type=float, default=30)
    p.add_argument("--connections", type=int, default=16, help="per process")
    p.add_argument("--processes", type=int, default=3)
    p.add_argument("--check-after", type=float, default=75, help="seconds to wait for the rollup before checking")
    args = p.parse_args()
    browser = pid = None
    if args.setup:
        browser, pid, args.key = setup(args.url)
    if not args.key:
        p.error("--key or --setup")
    q = multiprocessing.Queue()
    procs = [multiprocessing.Process(target=child, args=(args, args.rate / args.processes, i, q))
             for i in range(args.processes)]
    started = time.monotonic()
    for pr in procs:
        pr.start()
    results = []
    for _ in procs:
        results += q.get()
    for pr in procs:
        pr.join()
    elapsed = time.monotonic() - started
    statuses: dict = {}
    for s, *_ in results:
        statuses[s] = statuses.get(s, 0) + 1
    lat = sorted(l for s, l, *_ in results if s == 202)
    pct = lambda f: lat[min(len(lat) - 1, int(f * len(lat)))] * 1000 if lat else float("nan")  # noqa: E731
    signals = sum(n for *_, n, _ in results)
    accepted = sum(a for *_, a in results)
    report = {
        "target_signals_per_s": args.rate, "seconds": args.seconds, "requests": len(results),
        "signals_sent": signals, "signals_accepted": accepted,
        "achieved_signals_per_s": round(signals / elapsed, 1), "statuses": statuses,
        "p50_ms": round(pct(0.50), 2), "p99_ms": round(pct(0.99), 2),
        "max_ms": round(lat[-1] * 1000, 2) if lat else None,
    }
    print(json.dumps(report), flush=True)
    if browser is not None and args.check_after > 0:
        time.sleep(args.check_after)
        ts = json.loads(browser.get(f"/api/v1/projects/{pid}/analytics/app/timeseries?period=today&test=1"))
        rolled = json.loads(browser.get(f"/api/v1/projects/{pid}/analytics/app/timeseries?period=today"))
        print(json.dumps({"raw_signals_today": ts["totals"]["signals"],
                          "rolled_up_signals_today": rolled["totals"]["signals"],
                          "rolled_up_dau": rolled["totals"]["dau"]}), flush=True)


if __name__ == "__main__":
    main()
