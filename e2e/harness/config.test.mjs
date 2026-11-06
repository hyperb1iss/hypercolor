import assert from "node:assert/strict";
import test from "node:test";
import { buildConfigToml } from "./global-setup.mjs";

test("browser fixtures disable every hardware and network driver", () => {
  const config = buildConfigToml();
  const drivers = [...config.matchAll(/\[drivers\.([^\]]+)\]\nenabled = (true|false)/g)];
  // The daemon's startup tests prove this list covers every compiled driver;
  // here the rule is only that none of them may start enabled.
  assert.ok(drivers.length > 0);
  assert.ok(drivers.every(([, , enabled]) => enabled === "false"));
  assert.match(config, /mdns_enabled = false/);
  assert.match(config, /background_enabled = false/);
  assert.match(config, /blocks_scan = false/);
});
