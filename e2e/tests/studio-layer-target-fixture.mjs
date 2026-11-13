import { randomUUID } from "node:crypto";

import { expect } from "@playwright/test";
import { daemonError, daemonHeaders, getStack, refuseFence, writeFence } from "./helpers.mjs";

export const ZONE = "11111111-1111-4111-8111-111111111111";
export const KEYS = "keys-output";
export const MOUSE = "mouse-output";
const KEYBOARD = "fixture:keyboard";
const POINTER = "fixture:mouse";

const output = (id, device, name, x) => ({
  id, device_id: device, name, zone_name: null,
  position: { x, y: 0.5 }, size: { x: 0.24, y: 0.16 },
  rotation: 0, scale: 1, display_order: 0,
  topology: { type: "strip", count: 12, direction: "left_to_right" },
  orientation: null, sampling_mode: null, edge_behavior: null,
  shape: null, shape_preset: null,
});
const member = (output) => ({ id: output.id, device_id: output.device_id, name: output.name, segment: output.zone_name });
const placement = (output) => ({
  member: output.id, position: output.position, size: output.size,
  rotation: output.rotation, scale: output.scale, orientation: output.orientation,
  topology: output.topology,
});
const device = (id, name) => ({
  id, layout_device_id: id, name,
  origin: { driver_id: "fixture", backend_id: "usb", transport: "usb", protocol_id: "fixture/test" },
  presentation: { label: "Fixture" }, status: "connected", brightness: 100, total_leds: 12, segments: [],
});
const layer = (body) => ({
  id: randomUUID(),
  name: body.name ?? null,
  source: body.source,
  blend: body.blend ?? "alpha",
  opacity: body.opacity ?? 1,
  transform: body.transform ?? { anchor: { x: 0.5, y: 0.5 }, scale: [1, 1], rotation: 0, fit: "cover" },
  adjust: body.adjust ?? {
    brightness: 1, saturation: 1, hue_shift: 0, tint: [1, 1, 1, 1], tint_strength: 0, contrast: 0,
  },
  enabled: body.enabled ?? true,
  ...(body.scope ? { scope: { outputs: [...new Set(body.scope.outputs)].sort() } } : {}),
});

// A Studio page over one zone holding a keyboard and a mouse, with a daemon
// stand-in that keeps the zone's layer stack: creates, replaces (which mint
// a new id and read `scope` as missing, null, or a value), and reorders.
// Reads other than the scene, the devices, and the layouts reach the real
// harness daemon, which lists the effects and advertises layer-scope.
// `olderDaemon` takes that advertisement out of its status, as a daemon
// from before layers could be limited answers, and refuses a layer write
// that names `scope` at all: that daemon denies fields it does not know,
// `null` included.
export async function openLayerTargets(page, { effectId, olderDaemon = false }) {
  const keys = output(KEYS, KEYBOARD, "Keyboard", 0.3);
  const mouse = output(MOUSE, POINTER, "Mouse", 0.7);
  const scene = {
    id: "33333333-3333-4333-8333-333333333333", name: "Layer target fixture",
    kind: "named", is_default: false, revision: 7,
    run_epoch: "0b6f2b57-6e0c-4d7c-9a51-7c1d1f0e2a10",
    zones: [{
      id: ZONE, name: "Desk lights", role: "primary", enabled: true, brightness: 1,
      members: [keys, mouse].map(member),
      layout: { placements: [keys, mouse].map(placement) },
      layers: [layer({ name: "Base", source: { type: "effect", effect_id: effectId, controls: {} } })],
    }],
  };
  const zone = scene.zones[0];
  let socket;
  const writes = [];
  const unexpected = [];
  const envelope = (data) => ({ data, meta: { api_version: "v1", request_id: "layer_target_fixture", timestamp: "2026-09-10T00:00:00Z" } });
  const notify = () => socket.send(JSON.stringify({ type: "event", event: "zone_changed", data: {
    scene_id: scene.id, zone_id: ZONE, role: "primary", kind: "updated",
  } }));
  const settle = async (route) => {
    scene.revision += 1;
    await route.fulfill({ json: envelope(zone), headers: daemonHeaders(scene, { fence: true }) });
    notify();
  };
  await page.setViewportSize({ width: 1600, height: 1000 });
  await page.routeWebSocket(/\/api\/v1\/ws(?:\?|$)/, (route) => { socket = route; });
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (["GET", "HEAD"].includes(request.method())) {
      if (path === "/api/v1/scene") {
        return route.fulfill({ json: envelope(scene), headers: daemonHeaders(scene, { fence: true }) });
      }
      if (path === "/api/v1/devices") {
        const items = [device(KEYBOARD, "Keyboard"), device(POINTER, "Mouse")];
        return route.fulfill({ json: envelope({ items, total: items.length }), headers: daemonHeaders(scene) });
      }
      if (path === "/api/v1/layouts") {
        return route.fulfill({ json: envelope({ items: [], total: 0 }), headers: daemonHeaders(scene) });
      }
      if (path === "/api/v1/layouts/active") {
        return route.fulfill({ json: envelope({
          id: "fixture-rig", name: "Fixture rig", description: null,
          canvas_width: 640, canvas_height: 480, zones: [], version: 1,
        }), headers: daemonHeaders(scene) });
      }
      if (olderDaemon && path === "/api/v1/system") {
        const response = await route.fetch();
        const status = await response.json();
        status.data.status.capabilities = status.data.status.capabilities
          .filter((capability) => capability !== "layer-scope");
        return route.fulfill({ response, json: status });
      }
      return route.continue();
    }
    const body = request.postDataJSON();
    const ifMatch = request.headers()["if-match"];
    const write = { path, method: request.method(), body, ...writeFence(ifMatch) };
    writes.push(write);
    const refusal = refuseFence(scene, ifMatch);
    if (refusal) return route.fulfill(refusal);
    if (olderDaemon && body && "scope" in body) {
      return route.fulfill(daemonError(scene, 422, "validation_error", "unknown field `scope`"));
    }
    if (path === `/api/v1/scene/zones/${ZONE}/layers` && request.method() === "POST") {
      zone.layers.push(layer(body));
      return settle(route);
    }
    const replaced = path.match(new RegExp(`^/api/v1/scene/zones/${ZONE}/layers/([^/]+)$`));
    if (replaced && request.method() === "PUT") {
      const index = zone.layers.findIndex((stored) => stored.id === replaced[1]);
      expect(index, `the replaced layer ${replaced[1]} is in the stack`).toBeGreaterThanOrEqual(0);
      // Missing keeps the stored scope, null clears it, a value sets it.
      const scope = "scope" in body ? body.scope : zone.layers[index].scope;
      zone.layers[index] = layer({ ...body, scope: scope ?? undefined });
      return settle(route);
    }
    unexpected.push(write);
    await route.abort("blockedbyclient");
  });
  await page.goto(getStack().appOrigin + "/studio", { waitUntil: "networkidle" });
  await expect.poll(() => Boolean(socket)).toBe(true);
  await expect(page.locator(`[data-zone-id="${KEYS}"]`)).toBeVisible();
  return {
    scene, zone, writes, unexpected,
    layerWrites: () => writes.filter((write) => write.path.includes("/layers")),
    // Another client takes an output out of the zone.
    removeOutput: (id) => {
      zone.members = zone.members.filter((member) => member.id !== id);
      zone.layout.placements = zone.layout.placements.filter((placement) => placement.member !== id);
      scene.revision += 1;
      notify();
    },
    undo: page.getByRole("button", { name: "Undo (Ctrl+Z)", exact: true }),
    redo: page.getByRole("button", { name: "Redo (Ctrl+Shift+Z)", exact: true }),
  };
}
