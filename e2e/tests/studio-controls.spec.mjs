import { test, expect } from "@playwright/test";

import { getStack } from "./helpers.mjs";

const ZONE = "22222222-2222-4222-8222-222222222222";
const EFFECT = "44444444-4444-4444-8444-444444444444";
const LAYER = "55555555-5555-4555-8555-555555555555";
const replacementId = (revision) => `55555555-5555-4555-8555-${String(revision).padStart(12, "0")}`;
const float = (value) => ({ kind: "float", value });

async function openControls(page, { rejectControls = false } = {}) {
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
  await page.goto(`${getStack().appOrigin}/studio`, { waitUntil: "networkidle" });
  await expect.poll(() => Boolean(socket)).toBe(true);
  await page.getByTitle("Open the composition panel", { exact: true }).click();
  await page.getByRole("button", { name: "Control lights 0 devices", exact: true }).click();
  const inspector = page.getByRole("complementary");
  const control = (name) => inspector.locator(`div:has(> label:text-is("Fixture ${name}")) > input[type="range"]`);
  await expect(control("lower")).toHaveValue("0.4");
  await page.waitForLoadState("networkidle");
  return { scene, inspector, control, reads, writes, unexpectedWrites, controlEvent, sendEvent };
}

async function settle(page) {
  await page.waitForLoadState("networkidle");
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
}

async function scrollState(input) {
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

test("Studio control edits and remote snapshots preserve the scrolled, focused widget", async ({ page }) => {
  const fixture = await openControls(page);
  const lower = fixture.control("lower");
  await lower.scrollIntoViewIfNeeded();
  await lower.focus();
  const original = await lower.elementHandle();
  const beforeScroll = await scrollState(lower);
  expect(beforeScroll.some((offset) => offset > 100)).toBe(true);
  const beforeReads = fixture.reads.length;
  await lower.press("ArrowRight");
  await expect.poll(() => fixture.writes.length).toBe(1);
  await settle(page);
  expect(fixture.writes[0].body.values.lower.value).toBeCloseTo(0.41, 6);
  expect(await original.evaluate((element) => element.isConnected && document.activeElement === element)).toBe(true);
  expect(await scrollState(lower)).toEqual(beforeScroll);
  const editReads = fixture.reads.slice(beforeReads);
  expect(editReads.filter((path) => path.startsWith("/api/v1/effects/"))).toEqual([]);
  expect(editReads.filter((path) => path === "/api/v1/scene").length).toBeLessThanOrEqual(1);

  // Another client changes the control while this inspector stays selected.
  fixture.scene.zones[1].layers[0].source.controls.lower = float(0.73);
  fixture.scene.revision += 1;
  fixture.sendEvent("zone_changed", { scene_id: fixture.scene.id, zone_id: ZONE, role: "custom", kind: "controls_patched" });
  fixture.controlEvent("lower", 0.73, 0.41);
  await expect(lower).toHaveValue("0.73");
  await settle(page);
  expect(await original.evaluate((element) => element.isConnected && document.activeElement === element)).toBe(true);
  expect(await scrollState(lower)).toEqual(beforeScroll);
  expect(fixture.unexpectedWrites).toEqual([]);
});

test("Studio layer edits retain current controls and use each replacement's revision and identity", async ({ page }) => {
  const fixture = await openControls(page);
  // A refreshed snapshot changes both control data and revision before a
  // compositing edit. The subsequent PUT must preserve those remote values.
  fixture.scene.zones[1].layers[0].source.controls.lower = float(0.81);
  fixture.scene.revision = 47;
  fixture.sendEvent("zone_changed", { scene_id: fixture.scene.id, zone_id: ZONE, role: "custom", kind: "controls_patched" });
  fixture.controlEvent("lower", 0.81);
  await expect(fixture.control("lower")).toHaveValue("0.81");
  await fixture.control("lower").scrollIntoViewIfNeeded();
  const beforeScroll = await scrollState(fixture.control("lower"));
  expect(beforeScroll.some((offset) => offset > 100)).toBe(true);
  const opacity = fixture.inspector.locator("article input[type=range]").first();
  await opacity.evaluate((element) => {
    element.value = "0.5";
    element.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await expect.poll(() => fixture.writes.length).toBe(1);
  await settle(page);
  expect(fixture.writes[0].revision).toBe("47");
  expect(fixture.writes[0].body.source.controls.lower.value).toBeCloseTo(0.81, 6);
  await expect(opacity).toHaveValue("0.5");
  expect(await scrollState(fixture.control("lower"))).toEqual(beforeScroll);
  const blend = fixture.inspector.locator("article").getByRole("button", { name: "Replace", exact: true });
  await blend.scrollIntoViewIfNeeded();
  await settle(page);
  await blend.click();
  await page.getByRole("option", { name: "Add", exact: true }).click();
  await expect.poll(() => fixture.writes.length).toBe(2);
  await settle(page);
  expect(fixture.writes[1].revision).toBe("48");
  expect(fixture.writes[1].pathname).toContain(replacementId(48));
  expect(fixture.writes[1].body.opacity).toBe(0.5);
  expect(fixture.writes[1].body.source.controls.lower.value).toBeCloseTo(0.81, 6);
  await expect(fixture.inspector.locator("article").getByRole("button", { name: "Add", exact: true })).toBeVisible();
  expect(fixture.unexpectedWrites).toEqual([]);
});

test("Studio source replacement retargets effect controls to the new layer", async ({ page }) => {
  const fixture = await openControls(page);
  const oldControl = await fixture.control("lower").elementHandle();
  const nextEffect = "66666666-6666-4666-8666-666666666666";
  fixture.scene.revision += 1;
  fixture.scene.zones[1].layers[0] = {
    ...fixture.scene.zones[1].layers[0],
    id: replacementId(fixture.scene.revision),
    source: { type: "effect", effect_id: nextEffect, controls: { lower: float(0.6) } },
  };
  fixture.sendEvent("zone_changed", {
    scene_id: fixture.scene.id, zone_id: ZONE, role: "custom", kind: "updated",
  });
  await expect(fixture.control("lower")).toHaveValue("0.6");
  expect(await oldControl.evaluate((element) => element.isConnected)).toBe(false);
  expect(fixture.reads).toContain(`/api/v1/effects/${nextEffect}`);
  await fixture.control("lower").focus();
  await fixture.control("lower").press("ArrowRight");
  await expect.poll(() => fixture.writes.length).toBe(1);
  expect(fixture.writes[0].pathname).toBe(`/api/v1/scene/zones/${ZONE}/layers/${replacementId(43)}/controls`);
  expect(fixture.writes[0].body.values.lower.value).toBeCloseTo(0.61, 6);
  expect(fixture.unexpectedWrites).toEqual([]);
});

test("Studio rejected control edits restore the canonical value without remounting", async ({ page }) => {
  const fixture = await openControls(page, { rejectControls: true });
  const lower = fixture.control("lower");
  await lower.scrollIntoViewIfNeeded();
  await lower.focus();
  const original = await lower.elementHandle();
  const beforeScroll = await scrollState(lower);
  await lower.press("ArrowRight");
  await expect.poll(() => fixture.writes.length).toBe(1);
  await expect(lower).toHaveValue("0.4");
  await settle(page);
  expect(fixture.scene.zones[1].layers[0].source.controls.lower).toEqual(float(0.4));
  expect(await original.evaluate((element) => element.isConnected && document.activeElement === element)).toBe(true);
  expect(await scrollState(lower)).toEqual(beforeScroll);
  expect(fixture.unexpectedWrites).toEqual([]);
});

test("Studio advanced layer edits keep the disclosure open and preserve scrolling", async ({ page }) => {
  const fixture = await openControls(page);
  const disclosure = fixture.inspector.locator("article details");
  await disclosure.locator("summary").click();
  const scaleY = disclosure.locator('label:has(> span:text-is("Scale Y")) > input[type="range"]');
  await scaleY.scrollIntoViewIfNeeded();
  await expect.poll(async () => Number(await scaleY.inputValue())).toBe(1);
  const beforeScroll = await scrollState(scaleY);
  expect(beforeScroll.some((offset) => offset > 100)).toBe(true);
  const beforeReads = fixture.reads.length;
  await scaleY.evaluate((element) => {
    element.value = "1.5";
    element.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await expect.poll(() => fixture.writes.length).toBe(1);
  await settle(page);
  expect(fixture.writes[0].revision).toBe("42");
  expect(fixture.writes[0].body.transform.scale).toEqual([1, 1.5]);
  expect(fixture.scene.zones[1].layers[0].id).toBe(replacementId(43));
  await expect(disclosure).toHaveAttribute("open", "");
  await expect.poll(async () => Number(await scaleY.inputValue())).toBe(1.5);
  expect(await scrollState(scaleY)).toEqual(beforeScroll);
  expect(fixture.reads.slice(beforeReads).filter((path) => path === `/api/v1/effects/${EFFECT}`)).toEqual([]);
  expect(fixture.unexpectedWrites).toEqual([]);
});
