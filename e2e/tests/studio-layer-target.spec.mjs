import { test, expect } from "@playwright/test";

import { settle } from "./control-fixture.mjs";
import { createApi, findRunnableEffect, readEnvelope } from "./helpers.mjs";
import { KEYS, MOUSE, openLayerTargets } from "./studio-layer-target-fixture.mjs";

let effect;

test.beforeAll(async ({ playwright }) => {
  const api = await createApi(playwright);
  const effects = await readEnvelope(await api.get("/api/v1/effects"));
  effect = findRunnableEffect(effects.items, ["Rainbow", "Breathing"]);
  await api.dispose();
});

const scopes = (fixture) => fixture.zone.layers.map((layer) => layer.scope?.outputs ?? null);

// A dropdown closes when anything under it scrolls or shifts: the panel
// sliding in, a row's controls arriving after it mounts. The stack settles
// before one is opened.
async function stackSettled(page, rows) {
  await expect(page.locator("[data-layer-row]")).toHaveCount(rows);
  await expect(page.getByText("Loading controls…", { exact: true })).toHaveCount(0);
  await page.locator("#studio-composition-panel")
    .evaluate((panel) => Promise.all(panel.getAnimations({ subtree: true }).map((motion) => motion.finished)));
  await settle(page);
}

async function blendOfTopRow(page, name) {
  const blend = page.locator("[data-layer-row]").first()
    .getByRole("combobox", { name: "Blend mode", exact: true });
  await blend.scrollIntoViewIfNeeded();
  await settle(page);
  await blend.click();
  await page.getByRole("option", { name, exact: true }).click();
}

test("Studio adds a layer to one output from the canvas, retargets it, and undoes that", async ({ page }) => {
  const fixture = await openLayerTargets(page, { effectId: effect.id });

  // From the canvas: select the keyboard's box, then "+ Layer".
  await page.locator(`[data-zone-id="${KEYS}"]`).click();
  await page.getByRole("button", { name: "Add layer to the selection", exact: true }).click();
  const picker = page.getByRole("dialog", { name: "Add layer", exact: true });
  await expect(picker.getByText("Pick what plays on Keyboard", { exact: true })).toBeVisible();
  await picker.getByRole("button", { name: new RegExp(`^${effect.name}`) }).first().click();

  await expect.poll(() => scopes(fixture)).toEqual([null, [KEYS]]);
  const created = fixture.layerWrites().at(-1);
  expect(created.method).toBe("POST");
  expect(created.body.scope).toEqual({ outputs: [KEYS] });
  await expect(page.getByText("Layer added to Keyboard", { exact: true })).toBeVisible();

  // The stack names what each layer covers, and puts the keyboard's first.
  const chip = page.getByRole("button", { name: `${effect.name} applies to Keyboard`, exact: true });
  await expect(chip).toBeVisible();
  await expect(page.getByRole("button", { name: "Base applies to the whole zone", exact: true })).toBeVisible();
  await expect(page.locator("[data-studio-showing]")).toContainText("Keyboard");

  // Retarget from the chip: the checklist commits once, on Done.
  await chip.click();
  const editor = page.getByRole("dialog", { name: `${effect.name} applies to`, exact: true });
  await expect(editor.getByRole("checkbox", { name: "Keyboard", exact: true })).toBeChecked();
  await editor.getByRole("checkbox", { name: "Mouse", exact: true }).click();
  expect(fixture.layerWrites().at(-1)).toBe(created);
  await editor.getByRole("button", { name: "Done", exact: true }).click();

  await expect.poll(() => scopes(fixture)).toEqual([null, [KEYS, MOUSE]]);
  const retargeted = fixture.layerWrites().at(-1);
  expect(retargeted.method).toBe("PUT");
  expect(retargeted.body.scope).toEqual({ outputs: [KEYS, MOUSE] });
  await expect(page.getByRole("button", { name: `${effect.name} applies to Keyboard and Mouse`, exact: true })).toBeVisible();

  // One step back, one step forward.
  await expect(fixture.undo).toBeEnabled();
  await fixture.undo.click();
  await expect.poll(() => scopes(fixture)).toEqual([null, [KEYS]]);
  await expect(chip).toBeVisible();
  await expect(fixture.redo).toBeEnabled();
  await fixture.redo.click();
  await expect.poll(() => scopes(fixture)).toEqual([null, [KEYS, MOUSE]]);
  expect(fixture.unexpected).toEqual([]);
});

test("Studio restates what a layer applies to when another of its settings changes", async ({ page }) => {
  const fixture = await openLayerTargets(page, { effectId: effect.id });
  // A whole-zone layer says so, rather than leaving the key out for the
  // daemon to read as "keep whatever you hold".
  await page.locator("#studio-composition-trigger").click();
  await stackSettled(page, 1);
  await blendOfTopRow(page, "Screen");
  await expect.poll(() => fixture.zone.layers[0].blend).toBe("screen");
  const whole = fixture.layerWrites().at(-1);
  expect(whole.method).toBe("PUT");
  expect(whole.body).toHaveProperty("scope", null);

  // A layer limited to the mouse carries the mouse through its replace.
  await page.locator(`[data-zone-id="${MOUSE}"]`).click();
  await page.getByRole("button", { name: "Add layer to the selection", exact: true }).click();
  await page.getByRole("dialog", { name: "Add layer", exact: true })
    .getByRole("button", { name: new RegExp(`^${effect.name}`) }).first().click();
  await expect.poll(() => scopes(fixture)).toEqual([null, [MOUSE]]);
  await stackSettled(page, 2);
  await blendOfTopRow(page, "Add");
  await expect.poll(() => fixture.zone.layers.at(-1).blend).toBe("add");
  const limited = fixture.layerWrites().at(-1);
  expect(limited.method).toBe("PUT");
  expect(limited.body.scope).toEqual({ outputs: [MOUSE] });
  expect(scopes(fixture)).toEqual([null, [MOUSE]]);
  expect(fixture.unexpected).toEqual([]);
});

test("Studio stops an add whose chosen outputs left the zone instead of adding to the whole zone", async ({ page }) => {
  const fixture = await openLayerTargets(page, { effectId: effect.id });

  // Choose the keyboard for the next layer, then think again.
  await page.locator(`[data-zone-id="${KEYS}"]`).click();
  await page.getByRole("button", { name: "Add layer to the selection", exact: true }).click();
  const picker = page.getByRole("dialog", { name: "Add layer", exact: true });
  await expect(picker.getByText("Pick what plays on Keyboard", { exact: true })).toBeVisible();
  await picker.getByRole("button", { name: "Close", exact: true }).click();
  await expect(picker).toHaveCount(0);
  await expect(page.getByRole("button", { name: "New layer applies to Keyboard", exact: true })).toBeVisible();

  // The keyboard leaves the zone from somewhere else, which also leaves
  // the zone with one output and nothing to choose between.
  fixture.removeOutput(KEYS);
  await expect(page.locator(`[data-zone-id="${KEYS}"]`)).toHaveCount(0);

  // The target does not quietly become the whole zone: it says where it
  // stands, and Add layer says why it will not add.
  const stale = page.getByRole("button", { name: "The new layer's outputs are not in this zone", exact: true });
  await expect(stale).toBeVisible();
  await expect(stale).toContainText("Not in this zone");
  await page.getByRole("button", { name: "Add layer", exact: true }).click();
  await expect(page.getByText("The outputs chosen for the new layer have left this zone, so nothing was added. Choose what it applies to first.", { exact: true })).toBeVisible();
  await expect(picker).toHaveCount(0);
  expect(fixture.layerWrites()).toEqual([]);

  // Choosing again is one click away, and then the add goes through.
  await stale.click();
  const editor = page.getByRole("dialog", { name: "New layer applies to", exact: true });
  await editor.getByRole("radio", { name: /^Whole zone/ }).click();
  await editor.getByRole("button", { name: "Done", exact: true }).click();
  await page.getByRole("button", { name: "Add layer", exact: true }).click();
  await picker.getByRole("button", { name: new RegExp(`^${effect.name}`) }).first().click();
  await expect.poll(() => scopes(fixture)).toEqual([null, null]);
  expect(fixture.unexpected).toEqual([]);
});

test("Studio offers no layer target on a daemon that cannot limit layers", async ({ page }) => {
  const fixture = await openLayerTargets(page, { effectId: effect.id, olderDaemon: true });

  // The properties bar keeps "Move to" and gains nothing beside it.
  await page.locator(`[data-zone-id="${KEYS}"]`).click();
  await expect(page.getByRole("button", { name: "Move to", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Add layer to the selection", exact: true })).toHaveCount(0);

  // Neither does the device card's menu.
  const card = page.locator('div.group\\/card').filter({ has: page.getByText("Keyboard", { exact: true }) });
  await card.getByTitle("Device options").click();
  await expect(card.getByRole("button", { name: "Remove from this zone", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Add layer to this device", exact: true })).toHaveCount(0);

  // The stack adds to the zone, and its rows name no target.
  await page.locator("#studio-composition-trigger").click();
  await expect(page.locator("[data-layer-row]")).toHaveCount(1);
  await expect(page.getByRole("button", { name: /applies to/ })).toHaveCount(0);
  await expect(page.locator("[data-studio-showing]")).toHaveCount(0);
  expect(fixture.layerWrites()).toEqual([]);

  // Its layers still edit: the replace leaves out the field that daemon
  // does not know, rather than naming it as null.
  await stackSettled(page, 1);
  await blendOfTopRow(page, "Screen");
  await expect.poll(() => fixture.zone.layers[0].blend).toBe("screen");
  const replace = fixture.layerWrites().at(-1);
  expect(replace.method).toBe("PUT");
  expect(replace.body).not.toHaveProperty("scope");
  await expect(page.getByText(/Layer update failed/)).toHaveCount(0);

  // So does adding one, to the whole zone.
  await page.getByRole("button", { name: "Add layer", exact: true }).click();
  await page.getByRole("dialog", { name: "Add layer", exact: true })
    .getByRole("button", { name: new RegExp(`^${effect.name}`) }).first().click();
  await expect.poll(() => fixture.zone.layers.length).toBe(2);
  expect(fixture.layerWrites().at(-1).body).not.toHaveProperty("scope");
  expect(fixture.unexpected).toEqual([]);
});
