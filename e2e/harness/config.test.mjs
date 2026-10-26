import assert from "node:assert/strict";
import test from "node:test";
import { buildConfigToml } from "./global-setup.mjs";

test("browser fixtures disable every hardware and network driver", () => {
  const config = buildConfigToml();
  const drivers = [...config.matchAll(/\[drivers\.([^\]]+)\]\nenabled = (true|false)/g)];
  assert.deepEqual(drivers.map(([, id]) => id).sort(), [
    "asus", "blocks", "corsair", "dygma", "govee", "hue", "lianli", "nanoleaf",
    "nollie", "openrgb", "prismrgb", "push2", "qmk", "razer", "smbus", "usb", "wled",
  ]);
  assert.ok(drivers.every(([, , enabled]) => enabled === "false"));
  assert.match(config, /mdns_enabled = false/);
  assert.match(config, /background_enabled = false/);
  assert.match(config, /blocks_scan = false/);
});
