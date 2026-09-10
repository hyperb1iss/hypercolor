import { test, expect } from "@playwright/test";
import { openControls, settle, scrollState, ZONE, EFFECT, replacementId, float } from "./control-fixture.mjs";

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
