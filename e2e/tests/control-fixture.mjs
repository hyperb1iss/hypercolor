import { expect } from "@playwright/test";
import { getStack } from "./helpers.mjs";

export const ZONE = "22222222-2222-4222-8222-222222222222";
export const EFFECT = "44444444-4444-4444-8444-444444444444";
export const LAYER = "55555555-5555-4555-8555-555555555555";
export const replacementId = (revision) => `55555555-5555-4555-8555-${String(revision).padStart(12, "0")}`;
export const float = (value) => ({ kind: "float", value });

export async function openControls(page, { rejectControls = false, surface = "studio", beforeControlReply } = {}) {
  const zone = (id, name, role, layers = []) => ({
    id, name, role, enabled: true, brightness: 1,
    members: [], layout: { placements: [] }, layers,
  });
  const scene = {
    id: "33333333-3333-4333-8333-333333333333",
    name: "Layer controls fixture", kind: "named", is_default: false, revision: 42,
    zones: [
      zone("11111111-1111-4111-8111-111111111111", "Primary lights", "primary", [{
        id: "77777777-7777-4777-8777-777777777777", name: "Primary effect layer",
        source: { type: "effect", effect_id: "88888888-8888-4888-8888-888888888888", controls: {} },
      }]),
      zone(ZONE, "Control lights", "custom", [{
        id: LAYER, name: "Fixture layer", opacity: 1, blend: "replace",
        source: { type: "effect", effect_id: EFFECT, controls: {
          upper: float(0.2), lower: float(0.4),
        } },
      }]),
    ],
  };
  let socket;
  const reads = [];
  const writes = [];
  const unexpectedWrites = [];
  const envelope = (data) => ({ data, meta: {
    api_version: "v1", request_id: "req_controls_fixture", timestamp: "2026-09-09T00:00:00Z",
  } });
  const sendEvent = (event, data) => socket.send(JSON.stringify({ type: "event", event, data }));
  const controlEvent = (controlId, value, oldValue = 0.4) => sendEvent("effect_control_changed", {
    effect_id: scene.zones[1].layers[0].source.effect_id,
    zone_id: ZONE, layer_id: scene.zones[1].layers[0].id,
    control_id: controlId, old_value: float(oldValue), new_value: float(value), trigger: "api",
  });
  await page.setViewportSize({ width: 1440, height: 800 });
  await page.routeWebSocket(/\/api\/v1\/ws(?:\?|$)/, (route) => { socket = route; });
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const pathname = new URL(request.url()).pathname;
    if (["GET", "HEAD"].includes(request.method())) {
      reads.push(pathname);
      if (pathname === "/api/v1/scene") {
        await route.fulfill({ json: envelope(scene) });
        return;
      }
      if (pathname.startsWith("/api/v1/effects/") && !pathname.endsWith("/active")) {
        const id = pathname.split("/").at(-1);
        await route.fulfill({ json: envelope({
          id, name: "Fixture effect", description: "Scrollable effect controls", author: "Hypercolor tests",
          category: "ambient", source: "native", runnable: true, tags: [], version: "1.0.0",
          audio_reactive: false,
          controls: ["upper", ...Array.from({ length: 18 }, (_, i) => `middle${i}`), "lower"]
            .map((id) => ({ id, name: `Fixture ${id}`, control_type: "slider",
              default_value: float(0.2), min: 0, max: 1, step: 0.01 })),
        }) });
        return;
      }
      await route.continue();
      return;
    }
    const layerPath = `/api/v1/scene/zones/${ZONE}/layers/${scene.zones[1].layers[0].id}`;
    const body = request.postDataJSON();
    writes.push({ method: request.method(), pathname, body, revision: request.headers()["if-match"] });
    if (request.method() === "PATCH" && pathname === `${layerPath}/controls`) {
      if (rejectControls) {
        await route.fulfill({ status: 400, json: { error: { code: "invalid_control", message: "Fixture rejects this value" } } });
        return;
      }
      if (beforeControlReply) await beforeControlReply({ scene, body, sendEvent });
      Object.assign(scene.zones[1].layers[0].source.controls, body.values);
      scene.revision += 1;
      await route.fulfill({ json: envelope(scene.zones[1]) });
      sendEvent("zone_changed", { scene_id: scene.id, zone_id: ZONE, role: "custom", kind: "controls_patched" });
      for (const [id, value] of Object.entries(body.values)) controlEvent(id, value.value);
      return;
    }
    if (request.method() === "PUT" && pathname === layerPath) {
      if (request.headers()["if-match"] !== String(scene.revision)) {
        await route.fulfill({ status: 412, json: { error: { code: "stale_revision", message: "Stale revision" } } });
        return;
      }
      scene.revision += 1;
      scene.zones[1].layers[0] = { ...body, id: replacementId(scene.revision) };
      await route.fulfill({ json: envelope(scene.zones[1]) });
      sendEvent("zone_changed", { scene_id: scene.id, zone_id: ZONE, role: "custom", kind: "updated" });
      return;
    }
    unexpectedWrites.push(`${request.method()} ${pathname}`);
    await route.abort("blockedbyclient");
  });
  await page.goto(getStack().appOrigin + "/" + surface, { waitUntil: "networkidle" });
  await expect.poll(() => Boolean(socket)).toBe(true);
  let inspector;
  if (surface === "studio") {
    await page.getByTitle("Open the composition panel", { exact: true }).click();
    await page.getByRole("button", { name: "Control lights 0 devices", exact: true }).click();
    inspector = page.getByRole("complementary");
  } else {
    await page.getByRole("tab", { name: "Control lights", exact: true }).click();
    inspector = page.locator("body");
  }
  const control = (name) => inspector.locator(`div:has(> label:text-is("Fixture ${name}")) > input[type="range"]`);
  await expect(control("lower")).toHaveValue("0.4");
  await page.waitForLoadState("networkidle");
  return { scene, inspector, control, reads, writes, unexpectedWrites, controlEvent, sendEvent };
}

export async function settle(page) {
  await page.waitForLoadState("networkidle");
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
}

export async function scrollState(input) {
  return input.evaluate((element) => {
    const scrollers = [];
    for (let parent = element.parentElement; parent; parent = parent.parentElement) {
      if (parent.scrollHeight > parent.clientHeight && /auto|scroll/.test(getComputedStyle(parent).overflowY)) {
        scrollers.push(parent.scrollTop);
      }
    }
    return scrollers;
  });
}

