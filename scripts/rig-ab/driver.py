#!/usr/bin/env python3
"""Drive one isolated daemon into a one-zone scene with faceless panels and
sample its per-frame timing. Started by side.sh; see ab.sh for usage.

Runs inside the daemon's network namespace. Builds the rig over REST, holds
a display_preview subscription per simulated panel (a simulator is only a
display target while something previews it), then polls /api/v1/system and
keeps one record per rendered frame, keyed by its frame token.
"""

import argparse
import asyncio
import http.client
import json
import socket
import sys
import threading
import time

HOST = "127.0.0.1"
PORT = 9420


class Api:
    def __init__(self):
        self.conn = http.client.HTTPConnection(HOST, PORT, timeout=10)

    def call(self, method, path, body=None):
        payload = None if body is None else json.dumps(body)
        headers = {"content-type": "application/json"} if payload is not None else {}
        try:
            self.conn.request(method, "/api/v1" + path, body=payload, headers=headers)
            response = self.conn.getresponse()
            raw = response.read()
        except (http.client.HTTPException, OSError):
            self.conn.close()
            self.conn = http.client.HTTPConnection(HOST, PORT, timeout=10)
            raise
        if response.status >= 400:
            raise RuntimeError(f"{method} {path} -> {response.status}: {raw[:400]!r}")
        return json.loads(raw)["data"] if raw else None


def wait_for_api(deadline):
    while time.time() < deadline:
        try:
            Api().call("GET", "/system")
            return
        except Exception:
            time.sleep(0.2)
    raise RuntimeError("daemon API never came up")


def status(api):
    data = api.call("GET", "/system")
    return data.get("status", data)


def build_rig(api, args):
    sinks = []
    for index in range(args.led_devices):
        # A bound socket nobody reads: the strip's datagrams are dropped in
        # the kernel instead of bouncing back as ICMP errors.
        sink = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        sink.bind((HOST, 4048 + index))
        sinks.append(sink)

    if args.led_devices:
        api.call("POST", "/devices/discover", {"targets": ["pixel"], "wait": True, "timeout_ms": 5000})

    panels = []
    for index in range(args.panels):
        panel = api.call(
            "POST",
            "/simulators/displays",
            {"name": f"Bench LCD {index + 1}", "width": args.panel_size, "height": args.panel_size, "circular": False},
        )
        panels.append(panel["id"])

    deadline = time.time() + 20
    while True:
        devices = api.call("GET", "/devices")["items"]
        strips = [d for d in devices if d["total_leds"] > 0]
        screens = [d for d in devices if d["id"] in panels and d["status"] in ("connected", "active")]
        if len(strips) >= args.led_devices and len(screens) == len(panels):
            break
        if time.time() > deadline:
            raise RuntimeError(f"rig never connected: {[(d['name'], d['status']) for d in devices]}")
        time.sleep(0.25)

    outputs = []
    by_id = {d["id"]: d for d in devices}
    for index, panel in enumerate(panels):
        outputs.append(
            {
                "id": f"bench-lcd-{index}",
                "name": f"Bench LCD {index + 1}",
                "device_id": by_id[panel]["layout_device_id"],
                "zone_name": "Display",
                "position": {"x": (index + 0.5) / len(panels), "y": 0.3},
                "size": {"x": 0.8 / len(panels), "y": 0.4},
                "rotation": 0.0,
                "scale": 1.0,
                "display_order": index,
                "topology": {"type": "point"},
            }
        )
    for index, strip in enumerate(strips):
        outputs.append(
            {
                "id": f"bench-strip-{index}",
                "name": strip["name"],
                "device_id": strip["layout_device_id"],
                "zone_name": None,
                "position": {"x": 0.5, "y": 0.6 + 0.35 * (index + 0.5) / max(len(strips), 1)},
                "size": {"x": 0.9, "y": 0.04},
                "rotation": 0.0,
                "scale": 1.0,
                "display_order": len(panels) + index,
                "topology": {"type": "strip", "count": strip["total_leds"], "direction": "left_to_right"},
            }
        )
    api.call("PUT", "/layouts/default", {"zones": outputs})
    api.call("POST", "/layouts/default/apply")

    # A strip connects once a layout maps it; a second scan picks up any the
    # first left known.
    deadline = time.time() + 20
    rescanned = False
    while args.led_devices:
        devices = api.call("GET", "/devices")["items"]
        live = [d for d in devices if d["total_leds"] > 0 and d["status"] in ("connected", "active")]
        if len(live) >= args.led_devices:
            break
        if time.time() > deadline:
            raise RuntimeError(f"strips never connected: {[(d['name'], d['status']) for d in devices]}")
        if not rescanned and time.time() > deadline - 17:
            api.call("POST", "/devices/discover", {"targets": ["pixel"], "wait": True, "timeout_ms": 5000})
            rescanned = True
        time.sleep(0.25)

    effects = api.call("GET", "/effects")["items"]
    effect = next(e for e in effects if e["name"] == args.effect)
    api.call("POST", f"/effects/{effect['id']}/apply", {})
    return panels, sinks


class PreviewHold(threading.Thread):
    """Hold a display_preview subscription per panel and count its frames."""

    def __init__(self, panels):
        super().__init__(daemon=True)
        self.panels = panels
        self.frames = 0
        self.ready = threading.Event()
        self.stop = threading.Event()
        self.error = None

    def run(self):
        try:
            asyncio.run(self.hold())
        except Exception as error:  # noqa: BLE001
            self.error = error
            self.ready.set()

    async def hold(self):
        import websockets

        async with websockets.connect(f"ws://{HOST}:{PORT}/api/v1/ws", max_size=None) as ws:
            await ws.send(
                json.dumps(
                    {
                        "type": "subscribe",
                        "topics": [{"topic": "display_preview", "key": panel} for panel in self.panels],
                    }
                )
            )
            self.ready.set()
            while not self.stop.is_set():
                try:
                    message = await asyncio.wait_for(ws.recv(), timeout=0.5)
                except asyncio.TimeoutError:
                    continue
                if isinstance(message, (bytes, bytearray)):
                    self.frames += 1


FRAME_FIELDS = (
    "total_ms",
    "composition_ms",
    "wake_late_ms",
    "jitter_ms",
    "producer_ms",
    "spatial_sampling_ms",
    "device_output_ms",
    "event_bus_ms",
    "coordination_overhead_ms",
    "input_sampling_ms",
)


def sample(api, seconds, interval):
    frames = {}
    polls = 0
    end = time.time() + seconds
    next_poll = time.time()
    while True:
        now = time.time()
        if now >= end:
            break
        if now < next_poll:
            time.sleep(next_poll - now)
        next_poll = max(next_poll + interval, time.time())
        frame = status(api).get("latest_frame")
        polls += 1
        if not frame:
            continue
        token = frame["frame_token"]
        if token not in frames:
            frames[token] = [frame.get(field, 0.0) for field in FRAME_FIELDS]
    return frames, polls


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--label", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--start-epoch", type=float, required=True)
    parser.add_argument("--seconds", type=float, required=True)
    parser.add_argument("--panels", type=int, default=3)
    parser.add_argument("--panel-size", type=int, default=480)
    parser.add_argument("--led-devices", type=int, default=6)
    parser.add_argument("--led-pixels", type=int, default=150)
    parser.add_argument("--effect", default="Rainbow")
    parser.add_argument("--interval-ms", type=float, default=5.0)
    args = parser.parse_args()

    wait_for_api(time.time() + 30)
    api = Api()
    panels, sinks = build_rig(api, args)
    hold = PreviewHold(panels)
    hold.start()
    hold.ready.wait(10)
    if hold.error:
        raise hold.error

    wait = args.start_epoch - time.time()
    if wait < 3:
        print(f"[{args.label}] warning: only {wait:.1f}s of warmup before the window", file=sys.stderr)
    if wait > 0:
        time.sleep(wait)

    before = status(api)
    frames_before = hold.frames
    frames, polls = sample(api, args.seconds, args.interval_ms / 1000.0)
    after = status(api)
    preview_frames = hold.frames - frames_before

    hold.stop.set()

    rendered = after["render_loop"]["total_frames"] - before["render_loop"]["total_frames"]
    result = {
        "label": args.label,
        "fields": FRAME_FIELDS,
        "frames": list(frames.values()),
        "polls": polls,
        "rendered_frames": rendered,
        "preview_frames": preview_frames,
        "render_loop_before": before["render_loop"],
        "render_loop_after": after["render_loop"],
        "latest_frame": after.get("latest_frame"),
        "panels": args.panels,
        "panel_size": args.panel_size,
        "led_devices": args.led_devices,
        "led_pixels": args.led_pixels,
        "effect": args.effect,
        "seconds": args.seconds,
    }
    with open(args.out, "w") as handle:
        json.dump(result, handle)
    print(
        f"[{args.label}] sampled {len(frames)} of {rendered} frames, "
        f"{preview_frames} preview frames, tier {after['render_loop']['fps_tier']}"
    )


if __name__ == "__main__":
    main()
