import { test, expect } from "@playwright/test";

import { settle } from "./control-fixture.mjs";
import { createApi, findRunnableEffect, readEnvelope } from "./helpers.mjs";
import { openLayerTargets } from "./studio-layer-target-fixture.mjs";

let effect;

test.beforeAll(async ({ playwright }) => {
  const api = await createApi(playwright);
  const effects = await readEnvelope(await api.get("/api/v1/effects"));
  effect = findRunnableEffect(effects.items, ["Rainbow", "Breathing"]);
  await api.dispose();
});

// The stack as the daemon holds it, bottom first.
const names = (fixture) => fixture.zone.layers.map((layer) => layer.name);
const ids = (fixture) => fixture.zone.layers.map((layer) => layer.id);
const layerWrite = (fixture) => {
  const write = fixture.layerWrites().at(-1);
  return `${write.method} ${write.path.replace(/^.*\/layers/, "")}`;
};

async function openStack(page, rows) {
  await page.locator("#studio-composition-trigger").click();
  await stackSettled(page, rows);
}

// A row that leaves the stack takes its entrance animation with it, and a
// cancelled animation rejects: settled is all that is waited for.
async function stackSettled(page, rows) {
  await expect(page.locator("[data-layer-row]")).toHaveCount(rows);
  await expect(page.getByText("Loading controls…", { exact: true })).toHaveCount(0);
  await page.locator("#studio-composition-panel").evaluate(async (panel) => {
    await Promise.allSettled(panel.getAnimations({ subtree: true }).map((motion) => motion.finished));
  });
  await settle(page);
}

async function addFromPanel(page) {
  await page.getByRole("button", { name: "Add layer", exact: true }).click();
  await page.getByRole("dialog", { name: "Add layer", exact: true })
    .getByRole("button", { name: new RegExp(`^${effect.name}`) }).first().click();
}

// Rows list the stack top first.
const row = (page, index) => page.locator("[data-layer-row]").nth(index);

test("Studio undoes and redoes adding, reordering, and removing a zone's layers", async ({ page }) => {
  const fixture = await openLayerTargets(page, { effectId: effect.id });
  await openStack(page, 1);
  await expect(fixture.undo).toBeDisabled();

  // Adding a layer is one step: back, and forward again.
  await addFromPanel(page);
  await expect.poll(() => names(fixture)).toEqual(["Base", effect.name]);
  await expect(page.getByText("Layer added", { exact: true })).toBeVisible();
  await expect(fixture.undo).toBeEnabled();
  await fixture.undo.click();
  await expect.poll(() => names(fixture)).toEqual(["Base"]);
  expect(layerWrite(fixture)).toMatch(/^DELETE \//);
  await stackSettled(page, 1);
  await expect(fixture.redo).toBeEnabled();
  await fixture.redo.click();
  await expect.poll(() => names(fixture)).toEqual(["Base", effect.name]);
  await stackSettled(page, 2);

  // A blend somebody chose for it. The replace mints the layer a new id,
  // which the add recorded above has to follow.
  const blend = row(page, 0).getByRole("combobox", { name: "Blend mode", exact: true });
  await blend.scrollIntoViewIfNeeded();
  await settle(page);
  await blend.click();
  await page.getByRole("option", { name: "Add", exact: true }).click();
  await expect.poll(() => fixture.zone.layers[1].blend).toBe("add");
  await stackSettled(page, 2);

  // Moving it under the base layer is one step too.
  const [base, added] = ids(fixture);
  await row(page, 0).getByTitle("Move layer down", { exact: true }).click();
  await expect.poll(() => ids(fixture)).toEqual([added, base]);
  expect(layerWrite(fixture)).toBe("PATCH /order");
  await fixture.undo.click();
  await expect.poll(() => ids(fixture)).toEqual([base, added]);
  await fixture.redo.click();
  await expect.poll(() => ids(fixture)).toEqual([added, base]);
  await stackSettled(page, 2);

  // Removing it from the bottom of the stack.
  const { id: _, ...stood } = fixture.zone.layers[0];
  expect(stood.blend).toBe("add");
  await row(page, 1).locator("[data-layer-remove]").click();
  await expect.poll(() => ids(fixture)).toEqual([base]);
  await expect(page.getByText("Layer removed", { exact: true })).toBeVisible();

  // Undo makes it again where it sat, as it stood: the daemon makes a
  // layer on top, so the undo moves it back down.
  await fixture.undo.click();
  await expect.poll(() => names(fixture)).toEqual([effect.name, "Base"]);
  expect(layerWrite(fixture)).toBe("PATCH /order");
  const { id: restored, ...back } = fixture.zone.layers[0];
  expect(back).toEqual(stood);
  expect(restored).not.toBe(added);
  await stackSettled(page, 2);

  await fixture.redo.click();
  await expect.poll(() => ids(fixture)).toEqual([base]);
  expect(layerWrite(fixture)).toBe(`DELETE /${restored}`);
  await fixture.undo.click();
  await expect.poll(() => names(fixture)).toEqual([effect.name, "Base"]);
  const again = ids(fixture)[0];
  expect(again).not.toBe(restored);

  // The earlier steps named the layer under ids it no longer has, and
  // still find it: the reorder goes back, then the add.
  await fixture.undo.click();
  await expect.poll(() => ids(fixture)).toEqual([base, again]);
  await fixture.undo.click();
  await expect.poll(() => ids(fixture)).toEqual([base]);
  expect(layerWrite(fixture)).toBe(`DELETE /${again}`);
  await expect(fixture.undo).toBeDisabled();
  await expect(page.getByText(/removed from history/)).toHaveCount(0);
  expect(fixture.unexpected).toEqual([]);
});

test("Studio drops a reorder it can no longer undo and goes on to the step before it", async ({ page }) => {
  const fixture = await openLayerTargets(page, { effectId: effect.id });
  await openStack(page, 1);
  await addFromPanel(page);
  await expect.poll(() => names(fixture)).toEqual(["Base", effect.name]);
  await stackSettled(page, 2);
  await addFromPanel(page);
  await expect.poll(() => fixture.zone.layers.length).toBe(3);
  await stackSettled(page, 3);
  const [base, first, second] = ids(fixture);
  await row(page, 0).getByTitle("Move layer down", { exact: true }).click();
  await expect.poll(() => ids(fixture)).toEqual([base, second, first]);

  // Another client takes the base layer out: the stack is no longer the
  // one that was put in order.
  fixture.removeLayer(base);
  await stackSettled(page, 2);
  const sent = fixture.layerWrites().length;
  await fixture.undo.click();
  await expect(page.getByText(/can no longer be undone, so it was removed from history/)).toBeVisible();
  expect(fixture.layerWrites().length, "nothing was sent for the dropped step").toBe(sent);
  expect(ids(fixture)).toEqual([second, first]);

  // The journal is not stuck behind it: the add before it still undoes.
  await expect(fixture.undo).toBeEnabled();
  await fixture.undo.click();
  await expect.poll(() => ids(fixture)).toEqual([first]);
  expect(fixture.unexpected).toEqual([]);
});
