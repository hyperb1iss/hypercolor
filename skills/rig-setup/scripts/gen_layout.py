#!/usr/bin/env python3
"""Case spec + rig spec -> Hypercolor attachment bindings, spatial layout, and scene.

    gen_layout.py plan  --case CASE.json --rig RIG.json [--out DIR]
    gen_layout.py apply --case CASE.json --rig RIG.json [--out DIR]

`plan` validates the bindings against the daemon (creating any custom templates the
rig needs, which is additive), writes layout.json plus preview.svg into --out, and
touches nothing else. `apply` also PUTs the bindings, creates or updates the layout
and the scene named in the rig, and re-applies the layout when it is the active one.
Both modes are idempotent: rerun after every rig-spec edit.

Coordinate model (see references/case-spec.md):
  case frame   d = mm from the front outer face, h = mm above the bottom outer face
  board frame  u = mm from the I/O edge toward the front, v = mm from the CPU-end edge
  view         standard = glass on the left (front on the viewer's right)
               reversed = glass on the right (front on the left, board flipped 180)
"""
from __future__ import annotations

import argparse
import copy
import json
import math
import shutil
import subprocess
import sys
import urllib.error
import urllib.request
from pathlib import Path

PI = math.pi
DEFAULT_BASE = "http://localhost:9420/api/v1"


# ── HTTP ─────────────────────────────────────────────────────────────────────
class Api:
    def __init__(self, base: str):
        self.base = base

    def __call__(self, method: str, path: str, body=None):
        data = json.dumps(body).encode() if body is not None else None
        req = urllib.request.Request(self.base + path, data=data, method=method)
        req.add_header("Content-Type", "application/json")
        try:
            with urllib.request.urlopen(req, timeout=30) as resp:
                return json.load(resp)
        except urllib.error.HTTPError as err:
            raise SystemExit(f"{method} {path} -> {err.code}: {err.read().decode()[:800]}") from err
        except urllib.error.URLError as err:
            raise SystemExit(f"{method} {path}: daemon unreachable at {self.base} ({err.reason})") from err


# ── geometry ─────────────────────────────────────────────────────────────────
class Geometry:
    def __init__(self, case: dict, rig: dict):
        self.case = case
        self.rig = rig
        self.D = case["envelope"]["depth"]
        self.H = case["envelope"]["height"]
        self.mounts = case["mounts"]
        self.board = next((m for m in self.mounts.values() if m.get("kind") == "motherboard"), None)
        self.reversed = rig.get("view", "standard") == "reversed"

    # board-local -> case-native
    def board_to_case(self, u: float, v: float) -> tuple[float, float]:
        if self.board is None:
            raise SystemExit("rig uses board coordinates but the case spec has no motherboard mount")
        d = self.board["d_io_edge"] - u
        h = self.board["h_low"] + v if self.reversed else self.board["h_high"] - v
        return d, h

    def to_canvas(self, d: float, h: float) -> dict:
        x = d / self.D if self.reversed else (self.D - d) / self.D
        return {"x": round(x, 5), "y": round((self.H - h) / self.H, 5)}

    def size(self, w: float, hgt: float) -> dict:
        return {"x": round(w / self.D, 5), "y": round(hgt / self.H, 5)}

    def flip(self, rot: float) -> float:
        """Board-mounted parts turn with the board when the view is reversed."""
        return (rot + PI) % (2 * PI) if self.reversed else rot

    def feature(self, name: str) -> dict:
        features = (self.board or {}).get("features", {})
        if name not in features:
            raise SystemExit(f"unknown board feature '{name}' (have: {', '.join(features)})")
        return features[name]

    # Resolve one placement into (d, h, w, hgt, rotation) in mm / radians.
    def place(self, spec: dict, instance: int = 0, instances: int = 1) -> tuple[float, float, float, float, float]:
        if "case" in spec:
            c = spec["case"]
            return c["d"], c["h"], c["w"], c["hgt"], float(c.get("rot", 0.0))
        if "board" in spec:
            b = spec["board"]
            d, h = self.board_to_case(b["u"], b["v"])
            return d, h, b["w"], b["hgt"], self.flip(float(b.get("rot", 0.0)))
        if "anchor" in spec:
            f = self.feature(spec["anchor"])
            off = spec.get("offset", {})
            u = f.get("u", 0) + off.get("du", 0)
            v = f.get("v", 0) + off.get("dv", 0)
            d, h = self.board_to_case(u, v)
            w, hgt = spec.get("size_mm", [f.get("w", 30), f.get("hgt", 30)])
            return d, h, w, hgt, self.flip(float(spec.get("rot", 0.0)))
        name = spec.get("place")
        if name is None:
            raise SystemExit(f"placement needs one of place/board/anchor/case: {json.dumps(spec)[:120]}")
        if name == "motherboard":
            bm = self.board["board_mm"]
            d, h = self.board_to_case(bm["u"] / 2, bm["v"] / 2)
            return d, h, bm["v"], bm["u"], self.flip(PI / 2)
        if name not in self.mounts:
            raise SystemExit(f"unknown mount '{name}' (have: {', '.join(self.mounts)})")
        m = self.mounts[name]
        kind = m["kind"]
        chain = spec.get("chain")
        index = spec.get("index")
        if kind == "fan_row":
            centers = m["d_centers_rear_to_front"]
            if index is None:
                index = (len(centers) - 1 - instance) if chain == "front_to_rear" else instance
            edge = m.get("edge_thickness_mm", 28) + 12
            return centers[index], m["h_center"], m["fan_mm"], edge, float(m.get("edge_rotation", 0.0))
        if kind == "fan_column":
            centers = m["h_centers_top_to_bottom"]
            if index is None:
                index = (len(centers) - 1 - instance) if chain == "bottom_to_top" else instance
            if m.get("faces_viewer", True):
                return m["d_center"], centers[index], m["fan_mm"], m["fan_mm"], 0.0
            edge = m.get("edge_thickness_mm", 28) + 12
            return m["d_center"], centers[index], m["fan_mm"], edge, float(m.get("edge_rotation", PI / 2))
        if kind == "fan_single":
            if "h_center" in m:
                h = m["h_center"]
            else:
                _, h = self.board_to_case(0, m["h_from_board"]["v"])
            edge = m.get("edge_thickness_mm", 28) + 12
            rot = float(m.get("edge_rotation", 3 * PI / 2))
            if m.get("rotate_with_view", True) and not self.reversed:
                rot = (rot + PI) % (2 * PI)
            return m["d_center"], h, m["fan_mm"], edge, rot
        if kind == "l_strip":
            run, leg = m["run"], m["leg"]
            w = run["d_to"] - run["d_from"]
            hgt = abs(leg["h_to"] - leg["h_from"])
            d = (run["d_from"] + run["d_to"]) / 2
            h = (leg["h_from"] + leg["h_to"]) / 2
            return d, h, w, hgt, 0.0 if self.reversed else PI
        raise SystemExit(f"unsupported mount kind '{kind}' for '{name}'")


# ── templates the catalog lacks ──────────────────────────────────────────────
def l_strip_template(case: dict, mount_name: str, geo: Geometry) -> dict:
    """One custom-topology template per L-shaped case strip: leg first, then the run.

    Positions are template-local. The leg sits on the x=0 edge, which is the front
    side in the reversed view; the standard view rotates the zone by pi, so the
    top strip's leg lands on the right where the front is."""
    m = case["mounts"][mount_name]
    leg_n, run_n = m["leg"]["leds"], m["run"]["leds"]
    top = m["run"]["h"] > geo.H / 2
    positions = []
    for i in range(leg_n):
        t = i / max(leg_n - 1, 1)
        y = (1.0 - t * 0.95) if top else (t * 0.95)
        positions.append({"x": 0.0, "y": round(y, 4)})
    for i in range(run_n):
        t = i / max(run_n - 1, 1)
        positions.append({"x": round(0.02 + t * 0.98, 4), "y": 0.0 if top else 1.0})
    w = m["run"]["d_to"] - m["run"]["d_from"]
    hgt = abs(m["leg"]["h_to"] - m["leg"]["h_from"])
    return {
        "id": f"{case['id']}-l-strip-{mount_name}",
        "name": f"{case['name']} {mount_name.replace('_', ' ')} L-strip - {leg_n + run_n} LED",
        "category": "case",
        "origin": "user",
        "description": f"{leg_n}-LED pillar leg then {run_n}-LED run ({m.get('chain_order', 'chain order assumed')})",
        "vendor": case.get("vendor", ""),
        "default_size": {"width": round(w / geo.D, 4), "height": round(hgt / geo.H, 4)},
        "topology": {"type": "custom", "positions": positions},
        "compatible_slots": [],
        "tags": ["case", "l-strip", case["id"]],
        "physical_size_mm": [w, hgt],
    }


def strip_template(spec: dict) -> dict:
    """Rig-declared simple strip template, e.g. a light bar of unknown provenance."""
    return {
        "id": spec["id"],
        "name": spec.get("name", f"Custom strip - {spec['leds']} LED"),
        "category": spec.get("category", "strip"),
        "origin": "user",
        "description": spec.get("description", "rig-declared strip"),
        "vendor": spec.get("vendor", "Custom"),
        "default_size": {"width": round(spec.get("physical_size_mm", [200, 12])[0] / 478, 4), "height": 0.03},
        "topology": {"type": "strip", "count": spec["leds"], "direction": "left_to_right"},
        "compatible_slots": [],
        "tags": spec.get("tags", ["strip"]),
        "physical_size_mm": spec.get("physical_size_mm", [200, 12]),
    }


# ── topology edits driven by rig flags ───────────────────────────────────────
def mirror_x(topo: dict) -> dict:
    t = topo["type"]
    if t == "ring":
        topo["direction"] = "counter_clockwise" if topo.get("direction") == "clockwise" else "clockwise"
    elif t == "strip":
        flip = {"left_to_right": "right_to_left", "right_to_left": "left_to_right",
                "top_to_bottom": "bottom_to_top", "bottom_to_top": "top_to_bottom"}
        topo["direction"] = flip[topo["direction"]]
    elif t == "custom":
        topo["positions"] = [{"x": round(1.0 - p["x"], 4), "y": p["y"]} for p in topo["positions"]]
    elif t == "matrix":
        topo["start_corner"] = {"top_left": "top_right", "top_right": "top_left",
                                "bottom_left": "bottom_right", "bottom_right": "bottom_left"}[topo["start_corner"]]
    elif t == "perimeter_loop":
        topo["direction"] = "counter_clockwise" if topo.get("direction") == "clockwise" else "clockwise"
    return topo


def mirror_y(topo: dict) -> dict:
    t = topo["type"]
    if t == "matrix":
        topo["start_corner"] = {"top_left": "bottom_left", "bottom_left": "top_left",
                                "top_right": "bottom_right", "bottom_right": "top_right"}[topo["start_corner"]]
    elif t == "custom":
        topo["positions"] = [{"x": p["x"], "y": round(1.0 - p["y"], 4)} for p in topo["positions"]]
    elif t == "strip" and topo["direction"] in ("top_to_bottom", "bottom_to_top"):
        topo["direction"] = "bottom_to_top" if topo["direction"] == "top_to_bottom" else "top_to_bottom"
    elif t in ("ring", "perimeter_loop"):
        topo["direction"] = "counter_clockwise" if topo.get("direction") == "clockwise" else "clockwise"
    return topo


def led_total(topo: dict) -> int:
    t = topo["type"]
    if t in ("strip", "ring"):
        return topo["count"]
    if t == "matrix":
        return topo["width"] * topo["height"]
    if t == "custom":
        return len(topo["positions"])
    if t == "perimeter_loop":
        return sum(topo[k] for k in ("top", "right", "bottom", "left"))
    if t == "point":
        return 1
    return 0


def sanitize(raw: str) -> str:
    return "".join(c.lower() if c.isalnum() else "_" for c in raw)


def orientation_for(topo: dict):
    t = topo["type"]
    if t == "strip":
        return "vertical" if topo.get("direction") in ("top_to_bottom", "bottom_to_top") else "horizontal"
    if t in ("ring", "point", "concentric_rings"):
        return "radial"
    return None


def shape_for(topo: dict) -> dict:
    return {"shape_type": "ring"} if topo["type"] in ("ring", "concentric_rings", "point") else {"shape_type": "rectangle"}


# ── zone builders ────────────────────────────────────────────────────────────
def make_zone(geo: Geometry, zid, name, device_id, zone_name, mm, topo, *, shape=None, preset=None,
              attachment=None, order=0, orientation=None) -> dict:
    d, h, w, hgt, rot = mm
    return {
        "id": zid,
        "name": name,
        "device_id": device_id,
        "zone_name": zone_name,
        "position": geo.to_canvas(d, h),
        "size": geo.size(w, hgt),
        "rotation": round(rot, 6),
        "scale": 1.0,
        "display_order": order,
        "orientation": orientation if orientation is not None else orientation_for(topo),
        "topology": topo,
        "sampling_mode": None,
        "edge_behavior": None,
        "shape": shape or shape_for(topo),
        "shape_preset": preset,
        "attachment": attachment,
    }


def attachment_zones(geo: Geometry, ctrl_key: str, ctrl: dict, suggested: list[dict]) -> list[dict]:
    by_slot: dict[str, list[dict]] = {}
    for s in suggested:
        by_slot.setdefault(s["slot_id"], []).append(s)
    out = []
    for binding in ctrl["bindings"]:
        slot = binding["slot"]
        matches = [s for s in by_slot.get(slot, []) if s["template_id"] == binding["template"]]
        if len(matches) != binding.get("instances", 1):
            raise SystemExit(
                f"{ctrl_key}/{slot}: daemon suggested {len(matches)} zones for {binding['template']}, "
                f"rig expects {binding.get('instances', 1)}")
        for sug in matches:
            inst = sug["instance"]
            mm = geo.place(binding, instance=inst, instances=binding.get("instances", 1))
            if binding.get("rotate"):
                d, h, w, hgt, r = mm
                mm = (d, h, w, hgt, (r + float(binding["rotate"])) % (2 * PI))
            topo = copy.deepcopy(sug["topology"])
            if binding.get("mirror"):
                topo = mirror_x(topo)
            if binding.get("mirror_y"):
                topo = mirror_y(topo)
            led_start = binding.get("led_start_override", sug["led_start"])
            att = {
                "template_id": sug["template_id"],
                "slot_id": slot,
                "instance": inst,
                "led_start": led_start,
                "led_count": sug["led_count"],
                "led_mapping": sug.get("led_mapping"),
            }
            zid = f"attachment-{sanitize(ctrl['layout_device_id'])}-{sanitize(slot)}-{led_start}-{inst}"
            label = binding.get("label") or binding.get("place") or binding.get("anchor") or slot
            suffix = f" {inst + 1}" if binding.get("instances", 1) > 1 else ""
            name = f"{sug['template_name'].split(' - ')[0]} ({label}{suffix})"
            out.append(make_zone(geo, zid, name, ctrl["layout_device_id"], slot, mm, topo, attachment=att))
    return out


def raw_zones(geo: Geometry, rig: dict) -> list[dict]:
    out = []
    for raw in rig.get("raw_zones", []):
        d, h, _, _, _ = geo.place(raw, instance=raw.get("index", 0) or 0)
        w, hgt = raw["size_mm"]
        kind = raw["kind"]
        if kind == "display":
            px_w, px_h = raw["px"]
            topo = {"type": "matrix", "width": px_w, "height": px_h, "serpentine": False, "start_corner": "top_left"}
            shape, preset, orient = {"shape_type": "ring" if raw.get("circular", True) else "rectangle"}, "lcd-display", "horizontal"
        elif kind == "ring":
            topo = {"type": "ring", "count": raw["count"], "start_angle": float(raw.get("start_angle", -PI / 2)),
                    "direction": raw.get("direction", "clockwise")}
            shape, preset, orient = {"shape_type": "ring"}, None, "radial"
        elif kind in ("vstrip", "hstrip"):
            if kind == "vstrip":
                direction = raw.get("direction", "top_to_bottom" if geo.reversed else "bottom_to_top")
            else:
                direction = raw.get("direction", "left_to_right")
            topo = {"type": "strip", "count": raw["count"], "direction": direction}
            shape, preset, orient = {"shape_type": "rectangle"}, None, ("vertical" if kind == "vstrip" else "horizontal")
        elif kind == "point":
            topo = {"type": "point"}
            shape, preset, orient = {"shape_type": "ring"}, None, "radial"
        else:
            raise SystemExit(f"unknown raw zone kind '{kind}'")
        if raw.get("mirror"):
            topo = mirror_x(topo)
        if raw.get("mirror_y"):
            topo = mirror_y(topo)
        rot = float(raw.get("rot", 0.0))
        zid = f"case-{sanitize(raw['layout_device_id'])}-{sanitize(raw['segment'])}"
        out.append(make_zone(geo, zid, raw["name"], raw["layout_device_id"], raw["segment"], (d, h, w, hgt, rot),
                             topo, shape=shape, preset=preset, orientation=orient, order=5 if kind == "display" else 0))
    return out


# ── preview ──────────────────────────────────────────────────────────────────
def led_points(topo: dict) -> list[tuple[float, float]]:
    t = topo["type"]
    if t == "custom":
        return [(p["x"], p["y"]) for p in topo["positions"]]
    if t == "strip":
        n = topo["count"]
        pts = [(i / max(n - 1, 1), 0.5) for i in range(n)]
        d = topo["direction"]
        if d == "right_to_left":
            pts = [(1 - x, y) for x, y in pts]
        elif d == "top_to_bottom":
            pts = [(0.5, x) for x, _ in pts]
        elif d == "bottom_to_top":
            pts = [(0.5, 1 - x) for x, _ in pts]
        return pts
    if t == "ring":
        n = topo["count"]
        a0 = topo.get("start_angle", 0.0)
        sgn = 1 if topo.get("direction") == "clockwise" else -1
        return [(0.5 + 0.5 * math.cos(a0 + sgn * 2 * PI * i / n), 0.5 + 0.5 * math.sin(a0 + sgn * 2 * PI * i / n)) for i in range(n)]
    if t == "matrix":
        w, h = topo["width"], topo["height"]
        if w * h > 400:
            return []
        pts = [((c + 0.5) / w, (r + 0.5) / h) for r in range(h) for c in range(w)]
        sc = topo.get("start_corner", "top_left")
        if "right" in sc:
            pts = [(1 - x, y) for x, y in pts]
        if "bottom" in sc:
            pts = [(x, 1 - y) for x, y in pts]
        return pts
    if t == "perimeter_loop":
        top, right, bottom, left = topo["top"], topo["right"], topo["bottom"], topo["left"]
        pts = [((i + 0.5) / top, 0.0) for i in range(top)]
        pts += [(1.0, (i + 0.5) / right) for i in range(right)]
        pts += [(1 - (i + 0.5) / bottom, 1.0) for i in range(bottom)]
        pts += [(0.0, 1 - (i + 0.5) / left) for i in range(left)]
        return pts
    if t == "point":
        return [(0.5, 0.5)]
    return []


def write_preview(geo: Geometry, rig: dict, zones: list[dict], path: Path) -> None:
    cw, ch = rig["canvas"]["width"], rig["canvas"]["height"]
    scale = 2
    W, Hh = cw * scale, ch * scale
    els = [f'<rect width="{W}" height="{Hh}" fill="#0b0a14"/>',
           f'<rect x="1" y="1" width="{W - 2}" height="{Hh - 2}" fill="none" stroke="#3a355c" stroke-width="2"/>']
    board = geo.board
    if board and "gpu_bay" in board.get("features", {}):
        f = board["features"]["gpu_bay"]
        p0 = geo.to_canvas(*geo.board_to_case(f["u_bracket"], f["v_from"]))
        p1 = geo.to_canvas(*geo.board_to_case(f["u_bracket"] + f["length"], f["v_to"]))
        x0, x1 = sorted([p0["x"], p1["x"]])
        y0, y1 = sorted([p0["y"], p1["y"]])
        els.append(f'<rect x="{x0 * W:.1f}" y="{y0 * Hh:.1f}" width="{(x1 - x0) * W:.1f}" height="{(y1 - y0) * Hh:.1f}" fill="#1a1830" stroke="#2c2a4a"/>')
        els.append(f'<text x="{(x0 + x1) / 2 * W:.1f}" y="{(y0 + y1) / 2 * Hh + 4:.1f}" fill="#4a4670" font-size="14" text-anchor="middle" font-family="sans-serif">GPU (ghost)</text>')
    palette = {"display": "#bd93f9", "matrix": "#f1fa8c", "ring": "#ff6ac1", "custom": "#80ffea",
               "perimeter_loop": "#80ffea", "strip": "#ffb86c", "point": "#50fa7b"}
    for z in zones:
        cx, cy = z["position"]["x"] * W, z["position"]["y"] * Hh
        w, h = z["size"]["x"] * W, z["size"]["y"] * Hh
        rot_deg = -math.degrees(z["rotation"])
        kind = "display" if z.get("shape_preset") == "lcd-display" else z["topology"]["type"]
        color = palette.get(kind, "#e8e6f5")
        els.append(f'<g transform="translate({cx:.1f},{cy:.1f}) rotate({rot_deg:.1f})">')
        if z["shape"]["shape_type"] == "ring":
            els.append(f'<ellipse rx="{w / 2:.1f}" ry="{h / 2:.1f}" fill="{color}" fill-opacity="0.12" stroke="{color}" stroke-width="1.5"/>')
        else:
            els.append(f'<rect x="{-w / 2:.1f}" y="{-h / 2:.1f}" width="{w:.1f}" height="{h:.1f}" fill="{color}" fill-opacity="0.12" stroke="{color}" stroke-width="1.5"/>')
        for i, p in enumerate(led_points(z["topology"])):
            px, py = (p[0] - 0.5) * w, (p[1] - 0.5) * h
            els.append(f'<circle cx="{px:.1f}" cy="{py:.1f}" r="{3.6 if i == 0 else 2.2}" fill="{"#ffffff" if i == 0 else color}"/>')
        els.append("</g>")
        els.append(f'<text x="{cx:.1f}" y="{cy + 4:.1f}" fill="#e8e6f5" font-size="11" text-anchor="middle" font-family="sans-serif" opacity="0.9">{z["name"]}</text>')
    front = "FRONT (left)" if geo.reversed else "REAR (left)"
    rear = "REAR (right)" if geo.reversed else "FRONT (right)"
    els.append(f'<text x="12" y="{Hh - 12}" fill="#8f8ab8" font-size="13" font-family="sans-serif">{front}  ·  {rig["name"]}  ·  {geo.D}×{geo.H} mm at {cw}×{ch}  ·  {rear}</text>')
    path.write_text(f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{Hh}" viewBox="0 0 {W} {Hh}">' + "".join(els) + "</svg>")
    if shutil.which("rsvg-convert"):
        subprocess.run(["rsvg-convert", "-o", str(path.with_suffix(".png")), str(path)], check=False)


# ── main ─────────────────────────────────────────────────────────────────────
def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("mode", choices=["plan", "apply"])
    ap.add_argument("--case", required=True, type=Path)
    ap.add_argument("--rig", required=True, type=Path)
    ap.add_argument("--out", type=Path, default=None, help="output dir (default: the rig file's directory)")
    ap.add_argument("--base", default=DEFAULT_BASE)
    args = ap.parse_args()

    case = json.loads(args.case.read_text())
    rig = json.loads(args.rig.read_text())
    if rig.get("case") and rig["case"] != case["id"]:
        raise SystemExit(f"rig targets case '{rig['case']}' but --case is '{case['id']}'")
    out = args.out or args.rig.parent
    out.mkdir(parents=True, exist_ok=True)
    api = Api(args.base)
    geo = Geometry(case, rig)

    # 1. custom templates (additive, needed even for a plan so bindings validate)
    templates = {t["id"]: t for t in api("GET", "/attachments/templates?limit=200")["data"]["items"]}
    wanted = [l_strip_template(case, name, geo) for name, m in case["mounts"].items() if m.get("kind") == "l_strip"]
    wanted += [strip_template(t) for t in rig.get("custom_templates", [])]
    for tpl in wanted:
        if tpl["id"] not in templates:
            created = api("POST", "/attachments/templates", tpl)["data"]
            templates[created["id"]] = created
            print(f"  + template {created['id']} ({created['led_count']} LEDs)")

    # 2. bindings -> suggested zones (validate_only on plan)
    zones: list[dict] = []
    for key, ctrl in rig["controllers"].items():
        bindings = [{"slot_id": b["slot"], "template_id": b["template"], "instances": b.get("instances", 1),
                     "led_offset": b.get("led_offset", 0), "enabled": True} for b in ctrl["bindings"]]
        resp = api("PUT", f"/devices/{ctrl['device']}/attachments",
                   {"bindings": bindings, "validate_only": args.mode != "apply"})["data"]
        print(f"  {key}: {len(resp['bindings'])} bindings, {len(resp['suggested_zones'])} suggested zones")
        zones += attachment_zones(geo, key, ctrl, resp["suggested_zones"])
    zones += raw_zones(geo, rig)

    description = rig.get("description") or f"{case['name']} in {rig.get('view', 'standard')} view, generated from case spec {case['id']}."
    layout_body = {"name": rig["name"], "description": description,
                   "canvas_width": rig["canvas"]["width"], "canvas_height": rig["canvas"]["height"], "zones": zones}
    (out / "layout.json").write_text(json.dumps(layout_body, indent=1))
    write_preview(geo, rig, zones, out / "preview.svg")
    addressable = sum(led_total(z["topology"]) for z in zones if z.get("shape_preset") != "lcd-display")
    print(f"  {len(zones)} zones, {addressable} addressable LEDs (displays excluded) -> {out / 'layout.json'}, {out / 'preview.svg'}")
    if args.mode != "apply":
        return

    # 3. layout (create or update by name), re-apply if it is live
    existing = [l for l in api("GET", "/layouts?limit=200")["data"]["items"] if l["name"] == rig["name"]]
    layout_id = existing[0]["id"] if existing else api("POST", "/layouts", {
        "name": rig["name"], "description": description,
        "canvas_width": rig["canvas"]["width"], "canvas_height": rig["canvas"]["height"]})["data"]["id"]
    summary = api("PUT", f"/layouts/{layout_id}", {"description": description, "canvas_width": rig["canvas"]["width"],
                                                   "canvas_height": rig["canvas"]["height"], "zones": zones})["data"]
    print(f"  layout {layout_id}: {summary['zone_count']} zones @ {summary['canvas_width']}x{summary['canvas_height']}")
    if summary.get("is_active"):
        applied = api("POST", f"/layouts/{layout_id}/apply", {})["data"]
        print(f"  re-applied active layout: applied={applied['applied']}")

    # 4. scene (create or update by name), one primary zone holding every output
    scenes = [s for s in api("GET", "/scenes?limit=200")["data"]["items"] if s["name"] == rig["name"]]
    scene_id = scenes[0]["id"] if scenes else api("POST", "/scenes", {"name": rig["name"], "description": description})["data"]["id"]
    scene = api("GET", f"/scenes/{scene_id}")["data"]
    members = [{"id": z["id"], "device_id": z["device_id"], "segment": z["zone_name"], "name": z["name"]} for z in zones]
    placements = [{"member": z["id"], "position": z["position"], "size": z["size"], "rotation": z["rotation"],
                   "scale": 1.0, "topology": z["topology"]} for z in zones]
    primary = scene["zones"][0]
    layers = primary.get("layers") or [{"source": {"type": "effect", "effect_id": rig.get("effect_id", "08d75535-1bd9-48e1-a970-2ef5406bf561"), "controls": {}},
                                        "blend": "replace", "opacity": 1.0, "enabled": True}]
    replace = {
        "id": scene["id"], "name": scene["name"], "description": description, "kind": scene["kind"],
        "unassigned_behavior": scene["unassigned_behavior"], "layout_id": layout_id,
        "activation_brightness": scene.get("activation_brightness"), "transition": scene["transition"],
        "priority": scene["priority"], "enabled": True,
        "metadata": {"case": case["id"], "view": rig.get("view", "standard")}, "mutation_mode": scene["mutation_mode"],
        "zones": [{"id": primary["id"], "name": "Case", "description": "Every case-mounted output", "role": "primary",
                   "enabled": True, "brightness": 1.0, "members": members, "layout": {"placements": placements},
                   "layers": [{k: v for k, v in layer.items() if k in ("id", "name", "source", "blend", "opacity", "transform", "adjust", "bindings", "enabled")} for layer in layers]}],
    }
    saved = api("PUT", f"/scenes/{scene_id}", replace)["data"]
    print(f"  scene {saved['id']} '{saved['name']}' layout_id={saved.get('layout_id')} members={len(saved['zones'][0]['members'])}")
    (out / "receipt.json").write_text(json.dumps({"layout_id": layout_id, "scene_id": saved["id"]}, indent=1))
    print("  activate with: POST /api/v1/scenes/{scene_id}/activate (or the activate_scene MCP tool)")


if __name__ == "__main__":
    main()
