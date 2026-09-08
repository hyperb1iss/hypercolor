"""Offline regression coverage for physical LED maps and radiator placement."""

import copy
import importlib.util
import json
import math
import re
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch as mock_patch
from xml.etree import ElementTree

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "gen_layout", ROOT / "scripts/gen_layout.py"
)
gen = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gen)


class CustomGeometryTests(unittest.TestCase):
    # Fixture pins preserve the current rig hypothesis, not hardware verification.
    def setUp(self):
        self.case = json.loads(
            (ROOT / "references/cases/lian-li-o11d-evo-rgb.json").read_text()
        )
        self.rig = json.loads(
            (ROOT / "references/rigs/o11d-evo-rgb-reversed-example.json").read_text()
        )
        self.geo = gen.Geometry(self.case, self.rig)

    def test_corsair_maps_preserve_wire_order_and_mount_rotation(self):
        zones = {zone["name"]: zone for zone in gen.raw_zones(self.geo, self.rig)}
        aio = zones["Corsair AIO Ring"]
        expected_aio = [
            (6, 4),
            (5, 5),
            (4, 6),
            (3, 6),
            (2, 6),
            (1, 5),
            (0, 4),
            (0, 3),
            (0, 2),
            (1, 1),
            (2, 0),
            (3, 0),
            (4, 0),
            (5, 1),
            (6, 2),
            (6, 3),
            (3, 2),
            (4, 3),
            (3, 4),
            (2, 3),
        ]
        lcd = zones["Corsair Pump Ring"]
        expected_lcd = [
            (6, 16),
            (4, 15),
            (2, 14),
            (1, 12),
            (0, 10),
            (0, 8),
            (0, 6),
            (1, 4),
            (2, 2),
            (4, 1),
            (6, 0),
            (8, 0),
            (10, 0),
            (12, 1),
            (14, 2),
            (15, 4),
            (16, 6),
            (16, 8),
            (16, 10),
            (15, 12),
            (14, 14),
            (12, 15),
            (10, 16),
            (8, 16),
        ]
        for zone, coords, denominator in (
            (aio, expected_aio, 6),
            (lcd, expected_lcd, 16),
        ):
            self.assertEqual(zone["topology"]["type"], "custom")
            self.assertEqual(gen.led_total(zone["topology"]), len(coords))
            self.assertEqual(
                gen.led_points(zone["topology"]),
                [(x / denominator, y / denominator) for x, y in coords],
            )
            self.assertAlmostEqual(zone["rotation"], math.pi / 2, places=6)
            self.assertEqual(zone["position"], zones["Corsair Pump LCD"]["position"])
            self.assertEqual(zone["shape"], {"shape_type": "ring"})
        # The LCD image itself must retain its independent, upright orientation.
        self.assertEqual(zones["Corsair Pump LCD"]["rotation"], 0)

    def test_svg_places_rotated_leds_in_daemon_canvas_direction(self):
        # Resolve actual emitted SVG transforms to canvas points. Positive quarter
        # turns move the off-axis LED from above-right to below-right in y-down
        # coordinates, matching the daemon sampler, while 0/pi remain unchanged.
        raw = {
            "layout_device_id": "test",
            "segment": "LED",
            "name": "test LED",
            "kind": "custom",
            "count": 1,
            "positions": [{"x": 1, "y": 0.25}],
            "size_mm": [50, 50],
            "case": {"d": 50, "h": 50, "w": 50, "hgt": 50},
        }
        case = {"envelope": {"depth": 100, "height": 100}, "mounts": {}}
        rig = {
            "name": "test",
            "canvas": {"width": 100, "height": 100},
            "raw_zones": [raw],
        }
        geo = gen.Geometry(case, rig)
        expected = [
            (0, (150, 75)),
            (math.pi / 2, (125, 150)),
            (math.pi, (50, 125)),
            (3 * math.pi / 2, (75, 50)),
        ]
        with (
            tempfile.TemporaryDirectory() as directory,
            mock_patch.object(gen.shutil, "which", return_value=None),
        ):
            path = Path(directory) / "preview.svg"
            for rotation, target in expected:
                with self.subTest(rotation=rotation):
                    raw["rot"] = rotation
                    gen.write_preview(geo, rig, gen.raw_zones(geo, rig), path)
                    svg = ElementTree.parse(path).getroot()
                    group = svg.find("{http://www.w3.org/2000/svg}g")
                    point = group.find("{http://www.w3.org/2000/svg}circle")
                    tx, ty, angle = map(
                        float,
                        re.fullmatch(
                            r"translate\(([^,]+),([^\)]+)\) rotate\(([^\)]+)\)",
                            group.attrib["transform"],
                        ).groups(),
                    )
                    x, y = float(point.attrib["cx"]), float(point.attrib["cy"])
                    radians = math.radians(angle)
                    canvas_x = tx + x * math.cos(radians) - y * math.sin(radians)
                    canvas_y = ty + x * math.sin(radians) + y * math.cos(radians)
                    self.assertAlmostEqual(canvas_x, target[0])
                    self.assertAlmostEqual(canvas_y, target[1])

    def test_custom_mirrors_do_not_mutate_source_positions(self):
        raw = copy.deepcopy(
            next(
                raw
                for raw in self.rig["raw_zones"]
                if raw["name"] == "Corsair AIO Ring"
            )
        )
        original = copy.deepcopy(raw["positions"])
        raw.update(mirror=True, mirror_y=True)
        zone = gen.raw_zones(self.geo, {"raw_zones": [raw]})[0]
        self.assertEqual(
            zone["topology"]["positions"],
            [{"x": round(1 - p["x"], 4), "y": round(1 - p["y"], 4)} for p in original],
        )
        self.assertEqual(raw["positions"], original)

    def test_custom_positions_reject_invalid_shape_count_and_coordinates(self):
        valid = {"name": "test map", "count": 1, "positions": [{"x": 0, "y": 1}]}
        invalid = [
            {"count": 0},
            {"count": True},
            {"count": 1.0},
            {"count": 2},
            {"positions": []},
            {"positions": None},
            {"positions": [[0, 1]]},
            {"positions": [{"x": 0}]},
            {"positions": [{"x": 0, "y": 1, "z": 0}]},
        ]
        invalid += [
            {"positions": [{"x": value, "y": 0}]}
            for value in (-0.1, 1.1, float("nan"), float("inf"), True, "0", None)
        ]
        for patch in invalid:
            with (
                self.subTest(patch=patch),
                self.assertRaisesRegex(SystemExit, "test map:"),
            ):
                gen.raw_zones(
                    self.geo,
                    {
                        "raw_zones": [
                            {
                                **valid,
                                "kind": "custom",
                                "anchor": "cpu_socket",
                                "size_mm": [10, 10],
                                **patch,
                            }
                        ]
                    },
                )

    def test_radiator_leds_share_named_lcd_centers(self):
        displays = {zone["name"]: zone for zone in gen.raw_zones(self.geo, self.rig)}
        ctrl = self.rig["controllers"]["lwireless"]
        suggested = [
            {
                "slot_id": binding["slot"],
                "template_id": binding["template"],
                "instance": 0,
                "template_name": "TL Fan",
                "led_start": index * 26,
                "led_count": 26,
                "topology": {
                    "type": "ring",
                    "count": 26,
                    "start_angle": 0,
                    "direction": "clockwise",
                },
            }
            for index, binding in enumerate(ctrl["bindings"])
        ]
        fans = gen.attachment_zones(self.geo, "lwireless", ctrl, suggested)
        for index, fan in enumerate(fans, 1):
            self.assertEqual(fan["position"], displays[f"TL LCD {index}"]["position"])
        self.assertLess(fans[1]["position"]["y"], fans[0]["position"]["y"])
        self.assertLess(fans[0]["position"]["y"], fans[2]["position"]["y"])
        self.assertEqual(
            [fan["position"] for fan in fans],
            [
                {"x": 0.19874, "y": 0.50106},
                {"x": 0.19874, "y": 0.24628},
                {"x": 0.19874, "y": 0.75584},
            ],
        )


if __name__ == "__main__":
    unittest.main()
