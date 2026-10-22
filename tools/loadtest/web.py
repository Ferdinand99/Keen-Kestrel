#!/usr/bin/env python3
"""Load check for web analytics ingest (POST /ingest/v1/event).

    python3 tools/loadtest/web.py --url http://kestrel:3002 --key <public key> --rate 3000 --seconds 30

Open loop: requests are scheduled at a fixed rate and their latency is
measured from when they were due, so a slow server can't hide behind the
generator. Each request is a k.js-style text/plain pageview (now and then a
custom event) with one of 400 paths, 10 referrers, 12 user agents and a
random client address in X-Forwarded-For (start Kestrel with
KK_TRUST_PROXY=1 and KK_INGEST_IP_RATE_LIMIT=0 so they count as many
visitors). Python standard library only; HTTP/1.1 keep-alive connections,
spread over several processes.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import multiprocessing
import random
import time
import urllib.parse

UAS = [
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15",
    "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0",
    "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1",
    "Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Mobile Safari/537.36",
    "Mozilla/5.0 (iPad; CPU OS 17_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.6 Mobile/15E148 Safari/604.1",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36 Edg/141.0.0.0",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:131.0) Gecko/20100101 Firefox/131.0",
    "Mozilla/5.0 (Linux; Android 14; SAMSUNG SM-S921B) AppleWebKit/537.36 (KHTML, like Gecko) SamsungBrowser/26.0 Chrome/122.0.0.0 Mobile Safari/537.36",
    "Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36",
    "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/141.0 Mobile/15E148 Safari/604.1",
]
REFERRERS = [None, None, None, "https://www.google.com/", "https://news.ycombinator.com/item?id=1",
             "https://duckduckgo.com/", "https://www.reddit.com/r/selfhosted/", "https://github.com/x/y",
             "https://t.co/abc", "https://mastodon.social/@x"]
PATHS = [f"/docs/page-{i}" for i in range(380)] + ["/", "/pricing", "/blog", "/about"] * 5
WIDTHS = [390, 414, 768, 1280, 1440, 1920, 2560]


def body(key: str, site: str) -> bytes:
    path = random.choice(PATHS)
    if random.random() < 0.05:
        path += "?utm_source=newsletter&utm_medium=email&utm_campaign=autumn"
    event = {"k": key, "n": "pageview", "u": site + path, "r": random.choice(REFERRERS),
             "w": random.choice(WIDTHS), "h": 0}
    if random.random() < 0.03:
        event["n"] = "Signup"
        event["p"] = {"plan": random.choice(["free", "pro"])}
    return json.dumps(event).encode()


async def worker(host: str, port: int, path: str, key: str, site: str, due: asyncio.Queue, out: list):
    reader = writer = None
    while True:
        t = await due.get()
        if t is None:
            break
        ip = f"10.{random.randrange(256)}.{random.randrange(256)}.{random.randrange(1, 255)}"
        b = body(key, site)
        req = (f"POST {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: text/plain;charset=UTF-8\r\n"
               f"User-Agent: {random.choice(UAS)}\r\nOrigin: {site}\r\nX-Forwarded-For: {ip}\r\n"
               f"Content-Length: {len(b)}\r\n\r\n").encode() + b
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
            if length:
                await reader.readexactly(length)
            out.append((status, time.monotonic() - t))
        except Exception:  # noqa: BLE001 - a broken connection is an error, then a new one
            out.append((0, time.monotonic() - t))
            writer = None


async def run(args, rate: float, seed: int) -> list:
    random.seed(seed)
    u = urllib.parse.urlparse(args.url)
    due: asyncio.Queue = asyncio.Queue()
    out: list = []
    workers = [asyncio.create_task(worker(u.hostname, u.port or 80, "/ingest/v1/event", args.key, args.site, due, out))
               for _ in range(args.connections)]
    start = time.monotonic()
    n = int(rate * args.seconds)
    for i in range(n):
        t = start + i / rate
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


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--url", required=True)
    p.add_argument("--key", required=True)
    p.add_argument("--site", default="https://load.example")
    p.add_argument("--rate", type=float, default=3000)
    p.add_argument("--seconds", type=float, default=30)
    p.add_argument("--connections", type=int, default=48, help="per process")
    p.add_argument("--processes", type=int, default=3)
    args = p.parse_args()
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
    for s, _ in results:
        statuses[s] = statuses.get(s, 0) + 1
    lat = sorted(l for s, l in results if s == 202)
    pct = lambda f: lat[min(len(lat) - 1, int(f * len(lat)))] * 1000 if lat else float("nan")  # noqa: E731
    print(json.dumps({
        "target_rate": args.rate, "seconds": args.seconds, "sent": len(results),
        "achieved_rate": round(len(results) / elapsed, 1), "statuses": statuses,
        "p50_ms": round(pct(0.50), 2), "p99_ms": round(pct(0.99), 2),
        "max_ms": round(lat[-1] * 1000, 2) if lat else None,
    }))


if __name__ == "__main__":
    main()
