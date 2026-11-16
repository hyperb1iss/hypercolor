#!/usr/bin/env python3
"""Capture what each simulated panel shows for a still scene.

A stand-in for driver.py: RIG_AB_DRIVER=parity.py with
RIG_AB_DRIVER_ARGS="--effect Gradient". With a still effect the encoded
panel frame is a pure function of the build, so two builds can be compared
byte for byte: the digests land in samples.json and the JPEGs beside it.
"""

import argparse
import hashlib
import http.client
import json
import sys
import time
from pathlib import Path

import driver


def panel_frame(panel):
    conn = http.client.HTTPConnection(driver.HOST, driver.PORT, timeout=10)
    conn.request("GET", f"/api/v1/simulators/displays/{panel}/frame")
    response = conn.getresponse()
    body = response.read()
    conn.close()
    return response.status, body


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--label", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--start-epoch", type=float, default=0)
    parser.add_argument("--seconds", type=float, default=0)
    parser.add_argument("--panels", type=int, default=3)
    parser.add_argument("--panel-size", type=int, default=480)
    parser.add_argument("--led-devices", type=int, default=6)
    parser.add_argument("--led-pixels", type=int, default=150)
    parser.add_argument("--effect", default="Gradient")
    args = parser.parse_args()

    driver.wait_for_api(time.time() + 30)
    api = driver.Api()
    panels, _sinks = driver.build_rig(api, args)
    hold = driver.PreviewHold(panels)
    hold.start()
    hold.ready.wait(10)
    if hold.error:
        raise hold.error

    out = Path(args.out).parent / "panels"
    out.mkdir(parents=True, exist_ok=True)
    digests = []
    # Two reads a few seconds apart: a still scene must not change.
    for attempt in range(2):
        time.sleep(5)
        row = []
        for index, panel in enumerate(panels):
            status, body = panel_frame(panel)
            if status != 200:
                raise RuntimeError(f"panel {index} frame -> {status}")
            (out / f"panel-{index}-read-{attempt}.jpg").write_bytes(body)
            row.append(hashlib.sha256(body).hexdigest())
        digests.append(row)
    status = driver.status(api)
    hold.stop.set()
    result = {"label": args.label, "effect": args.effect, "digests": digests, "latest_frame": status.get("latest_frame")}
    Path(args.out).write_text(json.dumps(result))
    print(f"[{args.label}] {args.effect}: " + " ".join(d[:16] for d in digests[-1]) + (" stable" if digests[0] == digests[1] else " CHANGED BETWEEN READS"))


if __name__ == "__main__":
    main()
