import { test, expect } from "@playwright/test";

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
  const stackSettled = async (rows) => {
    // A row's controls load after it mounts and shift the stack as they
    // arrive, which closes an open dropdown, so the stack settles first.
    await expect(page.locator("[data-layer-row]")).toHaveCount(rows);
    await expect(page.getByText("Loading controls…", { exact: true })).toHaveCount(0);
  };
  const blendOfTopRow = async (name) => {
    await page.locator("[data-layer-row]").first()
      .getByRole("combobox", { name: "Blend mode", exact: true }).click();
    await page.getByRole("option", { name, exact: true }).click();
  };

  // A whole-zone layer says so, rather than leaving the key out for the
  // daemon to read as "keep whatever you hold".
  await page.locator("#studio-composition-trigger").click();
  await stackSettled(1);
  await blendOfTopRow("Screen");
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
  await stackSettled(2);
  await blendOfTopRow("Add");
  await expect.poll(() => fixture.zone.layers.at(-1).blend).toBe("add");
  const limited = fixture.layerWrites().at(-1);
  expect(limited.method).toBe("PUT");
  expect(limited.body.scope).toEqual({ outputs: [MOUSE] });
  expect(scopes(fixture)).toEqual([null, [MOUSE]]);
  expect(fixture.unexpected).toEqual([]);
});
