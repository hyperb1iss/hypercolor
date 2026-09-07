import { test, expect } from "@playwright/test";

import { getStack } from "./helpers.mjs";

const OUTPUT_COUNT = 80;
const FIRST_ZONE = "11111111-1111-4111-8111-111111111111";
const SECOND_ZONE = "22222222-2222-4222-8222-222222222222";
const outputSelector = "[data-zone-id]";
const handleSelector = ":scope > div.w-3.h-3";

function studioScene() {
  const members = Array.from({ length: OUTPUT_COUNT }, (_, index) => ({
    id: `studio-output-${index}`,
    device_id: `studio-fixture:device-${index}`,
    name: `Fixture light ${index + 1}`,
  }));
  const placements = members.map((member, index) => ({
    member: member.id,
    position: { x: 0.075 + (index % 10) * 0.09, y: 0.08 + Math.floor(index / 10) * 0.115 },
    size: { x: 0.065, y: 0.075 },
    topology: { type: "strip", count: 12, direction: "left_to_right" },
  }));
  const zone = (id, name, role, zoneMembers, zonePlacements) => ({
    id,
    name,
    role,
    enabled: true,
    brightness: 1,
    members: zoneMembers,
    layout: { placements: zonePlacements },
    layers: [],
  });
  return {
    id: "33333333-3333-4333-8333-333333333333",
    name: "Studio performance fixture",
    kind: "named",
    is_default: false,
    revision: 42,
    zones: [
      zone(FIRST_ZONE, "Fixture lights", "primary", members, placements),
      zone(SECOND_ZONE, "Empty lights", "custom", [], []),
    ],
  };
}

async function openStudio(page, scene = studioScene()) {
  let sceneRequests = 0;
  let socket;
  await page.setViewportSize({ width: 1600, height: 1000 });
  // Keep browser interactions entirely local: no live events can replace the
  // fixture, and unexpected writes must never reach connected hardware.
  await page.routeWebSocket(/\/api\/v1\/ws(?:\?|$)/, (route) => { socket = route; });
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    if (request.method() !== "GET" && request.method() !== "HEAD") {
      await route.abort("blockedbyclient");
      return;
    }
    if (new URL(request.url()).pathname === "/api/v1/scene") {
      sceneRequests += 1;
      await route.fulfill({ json: {
        data: scene,
        meta: {
          api_version: "v1",
          request_id: `req_studio_fixture_${sceneRequests}`,
          timestamp: "2026-09-06T00:00:00Z",
        },
      } });
      return;
    }
    await route.continue();
  });
  await page.goto(`${getStack().appOrigin}/studio`, { waitUntil: "networkidle" });
  await expect(page.locator(outputSelector)).toHaveCount(OUTPUT_COUNT);
  return {
    sceneRequests: () => sceneRequests,
    sendEvent: async (event) => {
      await expect.poll(() => Boolean(socket)).toBe(true);
      socket.send(JSON.stringify(event));
      await nextPaint(page);
    },
  };
}

async function nextPaint(page) {
  await page.evaluate(() => new Promise((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(resolve));
  }));
}

test("Studio commits a drag released before animation frame and can undo it", async ({ page }) => {
  await openStudio(page);
  const output = page.locator('[data-zone-id="studio-output-22"]');
  const original = await output.evaluate((element) => ({
    left: element.style.left,
    top: element.style.top,
  }));

  // Dispatch the entire gesture in one JavaScript task. Playwright mouse
  // commands yield between events and cannot guarantee release before RAF.
  await output.evaluate((element) => {
    const bounds = element.getBoundingClientRect();
    const clientX = Math.round(bounds.x + bounds.width / 2);
    const clientY = Math.round(bounds.y + bounds.height / 2);
    const dispatch = (type, dx, dy, buttons) => element.dispatchEvent(new MouseEvent(type, {
      bubbles: true,
      cancelable: true,
      button: 0,
      buttons,
      clientX: clientX + dx,
      clientY: clientY + dy,
    }));
    dispatch("mousedown", 0, 0, 1);
    dispatch("mousemove", 30, 15, 1);
    dispatch("mouseup", 30, 15, 0);
  });

  const undo = page.getByRole("button", { name: "Undo (Ctrl+Z)", exact: true });
  await expect(undo).toBeEnabled();
  await nextPaint(page);
  expect(await output.evaluate((element) => ({
    left: element.style.left,
    top: element.style.top,
  }))).not.toEqual(original);
  await undo.click();
  await expect.poll(() => output.evaluate((element) => ({
    left: element.style.left,
    top: element.style.top,
  }))).toEqual(original);
});

test("Studio leaves geometry and undo untouched for an unmoved output or resize click", async ({ page }) => {
  await openStudio(page);
  const output = page.locator('[data-zone-id="studio-output-22"]');
  const geometry = (element) => ({
    left: element.style.left,
    top: element.style.top,
    width: element.style.width,
    height: element.style.height,
  });
  const original = await output.evaluate(geometry);
  const undo = page.getByRole("button", { name: "Undo (Ctrl+Z)", exact: true });
  await output.click();
  await nextPaint(page);
  expect(await output.evaluate(geometry)).toEqual(original);
  await expect(undo).toBeDisabled();
  await output.locator(handleSelector).last().click();
  await nextPaint(page);
  expect(await output.evaluate(geometry)).toEqual(original);
  await expect(undo).toBeDisabled();
});

test("Studio preserves selected resize handles when unrelated outputs are hovered", async ({ page }) => {
  await openStudio(page);
  const selected = page.locator('[data-zone-id="studio-output-22"]');
  await selected.click();
  await expect(selected.locator(handleSelector)).toHaveCount(4);
  const handles = await selected.locator(handleSelector).elementHandles();

  for (const index of [23, 24, 25, 23]) {
    await page.locator(`[data-zone-id="studio-output-${index}"]`).hover();
    await nextPaint(page);
    for (const handle of handles) {
      expect(await handle.evaluate((element) => element.isConnected)).toBe(true);
    }
  }
  const other = page.locator('[data-zone-id="studio-output-23"]');
  for (let toggle = 0; toggle < 2; toggle += 1) {
    await other.click({ modifiers: ["Shift"] });
    await nextPaint(page);
    for (const handle of handles) {
      expect(await handle.evaluate((element) => element.isConnected)).toBe(true);
    }
  }
  await expect(selected.locator(handleSelector)).toHaveCount(4);
});

test("Studio preserves all compound positions when a member is hovered during drag", async ({ page }) => {
  const scene = studioScene();
  scene.zones[0].members[23].device_id = scene.zones[0].members[22].device_id;
  await openStudio(page, scene);
  const output = page.locator('[data-zone-id="studio-output-22"]');
  const member = page.locator('[data-zone-id="studio-output-23"]');
  const geometry = (element) => ({
    left: element.style.left,
    top: element.style.top,
    width: element.style.width,
    height: element.style.height,
  });
  const original = await member.evaluate(geometry);
  await output.evaluate((element) => {
    const bounds = element.getBoundingClientRect();
    const clientX = Math.round(bounds.x + bounds.width / 2);
    const clientY = Math.round(bounds.y + bounds.height / 2);
    element.dispatchEvent(new MouseEvent("mouseenter", { clientX, clientY }));
    element.dispatchEvent(new MouseEvent("mousedown", {
      bubbles: true, cancelable: true, button: 0, buttons: 1, clientX, clientY,
    }));
    element.dispatchEvent(new MouseEvent("mousemove", {
      bubbles: true, cancelable: true, buttons: 1, clientX: clientX + 40, clientY: clientY + 20,
    }));
  });
  await nextPaint(page);
  const painted = await member.evaluate(geometry);
  const paintedPrimary = await output.evaluate(geometry);
  expect(painted).not.toEqual(original);
  // Hovering another member must not replace its direct drag paint with the
  // committed layout, which intentionally stays unchanged until release.
  await member.dispatchEvent("mouseenter", { bubbles: false });
  await nextPaint(page);
  expect(await member.evaluate(geometry)).toEqual(painted);
  expect(await output.evaluate(geometry)).toEqual(paintedPrimary);
});

test("Studio output boxes do not create individual backdrop filters", async ({ page }) => {
  await openStudio(page);
  const filtered = await page.locator(outputSelector).evaluateAll((elements) => elements
    .filter((element) => getComputedStyle(element).backdropFilter !== "none")
    .map((element) => element.dataset.zoneId));
  expect(filtered).toEqual([]);
});

test("Studio hover keeps smaller overlapping outputs reachable", async ({ page }) => {
  const scene = studioScene();
  const placements = scene.zones[0].layout.placements;
  placements[0].position = { x: 0.15, y: 0.15 };
  placements[0].size = { x: 0.2, y: 0.2 };
  placements[1].position = { x: 0.15, y: 0.15 };
  placements[1].size = { x: 0.05, y: 0.05 };
  await openStudio(page, scene);
  const background = page.locator('[data-zone-id="studio-output-0"]');
  const foreground = page.locator('[data-zone-id="studio-output-1"]');
  const originalRank = await background.evaluate((element) => element.style.zIndex);
  const foregroundRank = await foreground.evaluate((element) => Number(element.style.zIndex));
  await background.hover({ position: { x: 4, y: 4 } });
  await nextPaint(page);
  expect(await background.evaluate((element) => element.style.zIndex)).toBe(originalRank);
  const bounds = await foreground.boundingBox();
  await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2);
  expect(await foreground.evaluate((element) => {
    const bounds = element.getBoundingClientRect();
    return document.elementFromPoint(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2)
      ?.closest("[data-zone-id]") === element;
  })).toBe(true);
  await foreground.click();
  expect(await foreground.evaluate((element) => Number(element.style.zIndex)))
    .toBeGreaterThan(foregroundRank);
});

test("Studio switches light zones without fetching the scene again", async ({ page }) => {
  const fixture = await openStudio(page);
  const before = fixture.sceneRequests();
  await page.getByRole("button", { name: "Empty lights 0 devices", exact: true }).click();
  await expect(page.locator(outputSelector)).toHaveCount(0);
  await page.getByRole("button", { name: "Fixture lights 80 devices", exact: true }).click();
  await expect(page.locator(outputSelector)).toHaveCount(OUTPUT_COUNT);
  await nextPaint(page);
  await page.waitForLoadState("networkidle");
  expect(fixture.sceneRequests()).toBe(before);
});

test("Studio preserves the inspector during control events and refreshes values on selection", async ({ page }) => {
  const effectId = "44444444-4444-4444-8444-444444444444";
  const layerId = "55555555-5555-4555-8555-555555555555";
  const scene = studioScene();
  scene.zones[1].layers = [{
    id: layerId,
    name: "Fixture effect layer",
    source: {
      type: "effect",
      effect_id: effectId,
      controls: { speed: { kind: "float", value: 0.2 } },
    },
  }];
  const fixture = await openStudio(page, scene);
  // This schema belongs to the synthetic effect and is the only additional
  // API fixture needed to inspect the stored control value in a real widget.
  await page.route(`**/api/v1/effects/${effectId}`, async (route) => {
    await route.fulfill({ json: {
      data: {
        id: effectId,
        name: "Fixture effect",
        description: "Control freshness fixture",
        author: "Hypercolor tests",
        category: "ambient",
        source: "native",
        runnable: true,
        tags: [],
        version: "1.0.0",
        audio_reactive: false,
        controls: [{
          id: "speed",
          name: "Fixture speed",
          control_type: "slider",
          default_value: { kind: "float", value: 0.2 },
          min: 0,
          max: 1,
          step: 0.01,
        }],
      },
      meta: { api_version: "v1", request_id: "req_fixture_effect", timestamp: "2026-09-06T00:00:00Z" },
    } });
  });
  await page.getByTitle("Open the composition panel", { exact: true }).click();
  const selectEmpty = page.getByRole("button", { name: "Empty lights 0 devices", exact: true });
  await selectEmpty.click();
  const inspector = page.getByRole("complementary");
  const speed = inspector.locator('div:has(> label:text-is("Fixture speed")) > input[type="range"]');
  await expect(speed).toHaveValue("0.2");
  const originalControl = await speed.elementHandle();
  scene.revision = 43;
  scene.zones[1].layers[0].source.controls.speed.value = 0.8;
  for (let index = 0; index < 3; index += 1) {
    await fixture.sendEvent({
      type: "event",
      event: "zone_changed",
      data: { scene_id: scene.id, zone_id: SECOND_ZONE, role: "custom", kind: "controls_patched" },
    });
    expect(await originalControl.evaluate((element) => element.isConnected)).toBe(true);
  }
  await page.waitForLoadState("networkidle");
  // Other app contexts may fetch their own primary-effect state; control
  // events must leave this inspector mounted while the user is editing.
  expect(await originalControl.evaluate((element) => element.isConnected)).toBe(true);
  const before = fixture.sceneRequests();
  await page.getByRole("button", { name: "Fixture lights 80 devices", exact: true }).click();
  await expect(page.locator(outputSelector)).toHaveCount(OUTPUT_COUNT);
  await selectEmpty.click();
  await expect(speed).toHaveValue("0.8");
  expect(fixture.sceneRequests()).toBeGreaterThan(before);

  // The write is aborted by openStudio's route before reaching the daemon;
  // inspecting it proves the refreshed revision reached mutation callbacks.
  const pendingWrite = page.waitForRequest((request) => request.method() === "PUT"
    && new URL(request.url()).pathname.endsWith(`/layers/${layerId}`));
  await inspector.locator("article input[type=range]").first().evaluate((element) => {
    element.value = "0.5";
    element.dispatchEvent(new Event("change", { bubbles: true }));
  });
  expect((await pendingWrite).headers()["if-match"]).toBe("43");
});
