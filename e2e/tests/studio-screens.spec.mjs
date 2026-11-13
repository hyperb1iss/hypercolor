import { test, expect } from "@playwright/test";

import { settle } from "./control-fixture.mjs";
import { createApi, findRunnableEffect, getStack, readEnvelope, uniqueName } from "./helpers.mjs";

// Screens in Studio, against the real daemon: two simulated panels placed
// in one zone of a scene of their own, one of them with a face. Every
// assertion about what a gesture wrote reads the daemon's live scene.

const LCD_1 = "e2e-lcd-1";
const LCD_2 = "e2e-lcd-2";
const NAME_1 = "E2E LCD 1";
const NAME_2 = "E2E LCD 2";

let api;
let effect;
let sceneId;
let zoneId;
const simulators = [];

const face = (name, outputs, blend = "alpha") => ({
  name,
  source: { type: "effect", effect_id: effect.id, controls: {} },
  blend,
  opacity: 1,
  enabled: true,
  ...(outputs ? { scope: { outputs } } : {}),
});

async function liveScene() {
  return readEnvelope(await api.get("/api/v1/scene"));
}

// The owning zone's layers that are limited to outputs, as `[name, outputs]`.
async function limitedLayers() {
  const scene = await liveScene();
  const zone = scene.zones.find((candidate) => candidate.id === zoneId);
  return zone.layers
    .filter((layer) => layer.scope)
    .map((layer) => [layer.name, layer.scope.outputs]);
}

// Put the scene back to one whole-zone layer and one face on the first panel.
async function resetScene() {
  const read = await api.get(`/api/v1/scenes/${sceneId}`);
  const stored = await readEnvelope(read);
  const zone = stored.zones.find((candidate) => candidate.role === "primary");
  const replacement = { ...stored };
  delete replacement.is_default;
  delete replacement.revision;
  const screen = (id, device, name, x) => ({
    member: { id, device_id: device, segment: "Display", name },
    placement: {
      member: id,
      position: { x, y: 0.5 },
      size: { x: 0.2, y: 0.2667 },
      rotation: 0,
      scale: 1,
      topology: { type: "matrix", width: 1, height: 1, serpentine: false, start_corner: "top_left" },
    },
  });
  const screens = [
    screen(LCD_1, simulators[0].layout, NAME_1, 0.3),
    screen(LCD_2, simulators[1].layout, NAME_2, 0.7),
  ];
  replacement.unassigned_behavior = "off";
  // The primary zone follows the rig's layout, which knows nothing of
  // these panels, so they go in a zone of their own.
  const blank = (id, name, role) => ({
    ...(id ? { id } : {}),
    name,
    description: null,
    role,
    enabled: true,
    brightness: 1,
    color: null,
    display_target: null,
    members: [],
    layout: { placements: [] },
    layers: [],
  });
  const kept = stored.zones.find((candidate) => candidate.name === "Case");
  replacement.zones = [
    blank(zone.id, "Main", "primary"),
    {
      ...blank(kept?.id, "Case", "custom"),
      members: screens.map((entry) => entry.member),
      layout: { placements: screens.map((entry) => entry.placement) },
      layers: [face("Glow", null), face("Clock", [LCD_1])],
    },
  ];
  await readEnvelope(await api.put(`/api/v1/scenes/${sceneId}`, {
    headers: { "If-Match": read.headers().etag },
    data: replacement,
  }));
  await readEnvelope(await api.post(`/api/v1/scenes/${sceneId}/activate`));
  zoneId = (await liveScene()).zones.find((candidate) => candidate.name === "Case").id;
}

test.beforeAll(async ({ playwright }) => {
  api = await createApi(playwright);
  const effects = await readEnvelope(await api.get("/api/v1/effects"));
  effect = findRunnableEffect(effects.items, ["Rainbow", "Breathing"]);
  for (const [name, circular] of [[NAME_1, true], [NAME_2, false]]) {
    const simulator = await readEnvelope(await api.post("/api/v1/simulators/displays", {
      data: { name: uniqueName(name), width: 240, height: 240, circular, enabled: true },
    }));
    simulators.push({ id: simulator.id });
  }
  await expect.poll(async () => {
    const devices = await readEnvelope(await api.get("/api/v1/devices?limit=200"));
    for (const simulator of simulators) {
      simulator.layout = devices.items.find((device) => device.id === simulator.id)?.layout_device_id;
    }
    return simulators.every((simulator) => simulator.layout);
  }).toBe(true);
  const scene = await readEnvelope(await api.post("/api/v1/scenes", {
    data: { name: uniqueName("e2e-screens"), mutation_mode: "live" },
  }));
  sceneId = scene.id;
});

test.beforeEach(async ({ page }) => {
  await resetScene();
  await page.setViewportSize({ width: 1600, height: 1000 });
});

test.afterAll(async () => {
  await api.post("/api/v1/scene/deactivate");
  if (sceneId) await api.delete(`/api/v1/scenes/${sceneId}`);
  for (const simulator of simulators) {
    await api.delete(`/api/v1/simulators/displays/${simulator.id}`);
  }
  await api.dispose();
});

async function openStudio(page) {
  await page.goto(getStack().appOrigin + "/studio", { waitUntil: "networkidle" });
  await page.getByRole("tab", { name: "Case", exact: true }).click();
  await expect(page.locator(`[data-zone-id="${LCD_1}"]`)).toBeVisible();
}

// A dropdown closes when anything under it scrolls or shifts, so the panel
// settles before one is opened.
async function panelSettled(page) {
  await expect(page.getByText("Loading controls…", { exact: true })).toHaveCount(0);
  await page.locator("#studio-composition-panel")
    .evaluate((panel) => Promise.all(panel.getAnimations({ subtree: true }).map((motion) => motion.finished)));
  await settle(page);
}

test("Studio opens a screen's face from its box and changes how it meets what is behind", async ({ page }) => {
  await openStudio(page);
  const chip = page.locator("#studio-composition-trigger");
  await expect(chip).toContainText("Layers");

  // Select the box: the chip names the face and what a click opens.
  await page.locator(`[data-zone-id="${LCD_1}"]`).click();
  await expect(chip).toContainText("Clock");
  await expect(chip).toContainText("Face settings");
  await chip.click();

  // The panel is the screen's: its own layers, then what is behind it.
  const panel = page.locator("#studio-composition-panel");
  await expect(panel.locator("[data-composition-subject]")).toHaveText(NAME_1);
  await expect(panel.locator("[data-screen-heading]")).toHaveText("On this screen");
  await expect(panel.locator("[data-screen-behind]")).toHaveText("Behind it, from Case");
  await expect(panel.locator("[data-layer-row]")).toHaveCount(1);
  await expect(panel.getByRole("button", { name: "Clock applies to " + NAME_1, exact: true })).toBeVisible();
  await expect(panel.getByRole("button", { name: "Show Glow in the zone's layers", exact: true })).toBeVisible();

  // The bottom layer's blend is how the stack meets what is behind it.
  // The write goes to the zone that places the screen and restates the
  // screen the layer is on.
  const written = page.waitForRequest((request) =>
    request.method() === "PUT" && request.url().includes(`/api/v1/scene/zones/${zoneId}/layers/`));
  await panelSettled(page);
  // The face's own settings lead the row; how it blends waits under them,
  // closed until asked for.
  const row = panel.locator("[data-layer-row]").first();
  await expect(row.getByText("Face settings", { exact: true })).toBeVisible();
  const disclosure = row.locator("[data-blend-disclosure]");
  await expect(disclosure).toHaveJSProperty("open", false);
  await expect(disclosure.locator("summary")).toContainText("Blend with what is behind");
  await disclosure.locator("summary").click();
  await expect(disclosure).toHaveJSProperty("open", true);
  const blend = panel.getByRole("combobox", { name: "Blend with what is behind", exact: true });
  await blend.scrollIntoViewIfNeeded();
  await settle(page);
  await blend.click();
  await page.getByRole("option", { name: "Screen", exact: true }).click();
  expect((await written).postDataJSON().scope).toEqual({ outputs: [LCD_1] });
  await expect.poll(async () => {
    const scene = await liveScene();
    return scene.zones.find((zone) => zone.id === zoneId).layers
      .find((layer) => layer.name === "Clock")?.blend;
  }).toBe("screen");
  await expect.poll(limitedLayers).toEqual([["Clock", [LCD_1]]]);
  // The replace minted a new layer, and its row keeps the disclosure open.
  await expect(panel.locator("[data-blend-disclosure]").first()).toHaveJSProperty("open", true);

  // A layer behind it leads back to the zone's own list, which names the
  // face on one line that leads to the screen again.
  await panel.getByRole("button", { name: "Show Glow in the zone's layers", exact: true }).click();
  await expect(panel.locator("[data-composition-subject]")).toHaveText("Case");
  await expect(chip).toContainText("Layers");
  await panel.getByRole("button", { name: "Clock on " + NAME_1, exact: true }).click();
  await expect(panel.locator("[data-composition-subject]")).toHaveText(NAME_1);
});

test("One face on two screens is one layer, and one undo takes it back", async ({ page }) => {
  await openStudio(page);
  await page.locator(`[data-zone-id="${LCD_1}"]`).click();
  await page.locator(`[data-zone-id="${LCD_2}"]`).click({ modifiers: ["Shift"] });
  await expect(page.locator("#studio-composition-trigger")).toContainText("Face settings");

  await page.getByRole("button", { name: "Add layer to the selection", exact: true }).click();
  const picker = page.getByRole("dialog", { name: "Add layer", exact: true });
  await expect(picker.getByText(`Pick what plays on ${NAME_1} and ${NAME_2}`, { exact: true })).toBeVisible();
  // Screens are all it will play on, so the picker offers faces.
  await expect(picker.getByRole("button", { name: "Face", exact: true })).toBeVisible();
  await picker.getByRole("button", { name: new RegExp(`^${effect.name}`) }).first().click();

  await expect.poll(limitedLayers).toEqual([
    ["Clock", [LCD_1]],
    [effect.name, [LCD_1, LCD_2]],
  ]);
  const panel = page.locator("#studio-composition-panel");
  await expect(panel.locator("[data-composition-subject]")).toHaveText(`${NAME_1} and ${NAME_2}`);
  await expect(panel.locator("[data-screen-heading]")).toHaveText("On these screens");
  await expect(panel.getByRole("button", {
    name: `${effect.name} applies to ${NAME_1} and ${NAME_2}`, exact: true,
  })).toBeVisible();

  const undo = page.getByRole("button", { name: "Undo (Ctrl+Z)", exact: true });
  await expect(undo).toBeEnabled();
  await undo.click();
  await expect.poll(limitedLayers).toEqual([["Clock", [LCD_1]]]);
  const redo = page.getByRole("button", { name: "Redo (Ctrl+Shift+Z)", exact: true });
  await expect(redo).toBeEnabled();
  await redo.click();
  await expect.poll(limitedLayers).toEqual([
    ["Clock", [LCD_1]],
    [effect.name, [LCD_1, LCD_2]],
  ]);
});

test("A screen shows its mounting in the properties bar and fills the Stage in Focus", async ({ page }) => {
  await openStudio(page);
  await page.locator(`[data-zone-id="${LCD_1}"]`).click();

  const facts = page.locator("[data-screen-facts]");
  const mounting = facts.getByRole("combobox", { name: "Mounting", exact: true });
  await expect(mounting).toBeVisible();
  await expect(facts.getByRole("link", { name: "Open full-screen preview", exact: true }))
    .toHaveAttribute("href", new RegExp(`/preview\\?display=${simulators[0].id}$`));
  await mounting.click();
  await page.getByRole("option", { name: "Upside down", exact: true }).click();
  await expect.poll(async () => {
    const displays = await readEnvelope(await api.get("/api/v1/displays"));
    return displays.items.find((display) => display.id === simulators[0].id)?.rotation;
  }).toBe("deg180");

  const focus = page.getByRole("button", { name: "Focus", exact: true });
  await focus.click();
  const focused = page.locator("[data-studio-focus]");
  await expect(focused).toBeVisible();
  await expect(focused.getByRole("combobox", { name: "Mounting", exact: true })).toBeVisible();
  await expect(page.locator(`[data-zone-id="${LCD_1}"]`)).toBeHidden();
  await page.keyboard.press("Escape");
  await expect(focused).toHaveCount(0);
  await expect(page.locator(`[data-zone-id="${LCD_1}"]`)).toBeVisible();
});

test("A daemon from before screen layers still takes a face, through the screen's own zone", async ({ page }) => {
  // That daemon advertises no screen-layers and lists each screen as a
  // zone of its own, with nothing saying which zone it is a view of.
  await page.route("**/api/v1/system", async (route) => {
    const response = await route.fetch();
    const status = await response.json();
    status.data.status.capabilities = status.data.status.capabilities
      .filter((capability) => capability !== "screen-layers");
    await route.fulfill({ response, json: status });
  });
  let views = {};
  await page.route("**/api/v1/scene", async (route) => {
    if (route.request().method() !== "GET") return route.continue();
    const response = await route.fetch();
    const scene = await response.json();
    for (const zone of scene.data.zones) {
      if (zone.view_of) views[zone.view_of.member] = zone.id;
      delete zone.view_of;
    }
    await route.fulfill({ response, json: scene });
  });
  await openStudio(page);

  // The second panel shows nothing, so its chip offers a face.
  await page.locator(`[data-zone-id="${LCD_2}"]`).click();
  const chip = page.locator("#studio-composition-trigger");
  await expect(chip).toHaveText(/^\s*Face\s*$/);
  const created = page.waitForRequest((request) =>
    request.method() === "POST" && /\/api\/v1\/scene\/zones\/[^/]+\/layers$/.test(request.url()));
  await chip.click();
  const picker = page.getByRole("dialog", { name: "Add layer", exact: true });
  await picker.getByRole("button", { name: new RegExp(`^${effect.name}`) }).first().click();

  // The layer is written to the screen's own zone, with nothing said
  // about outputs, and the screen shows it.
  const request = await created;
  expect(new URL(request.url()).pathname).toBe(`/api/v1/scene/zones/${views[LCD_2]}/layers`);
  expect(request.postDataJSON()).not.toHaveProperty("scope");
  await expect.poll(limitedLayers).toEqual([
    ["Clock", [LCD_1]],
    [effect.name, [LCD_2]],
  ]);
  await expect(chip).toContainText(effect.name);
  await expect(chip).toContainText("Face settings");
});
