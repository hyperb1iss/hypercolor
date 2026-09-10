import { test, expect } from "@playwright/test";
import { openControls, settle, scrollState, ZONE, replacementId, float } from "./control-fixture.mjs";

test("Effects zone control edits and remote snapshots preserve the scrolled, focused widget", async ({ page }) => {
  const fixture = await openControls(page, { surface: "effects" });
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

test("Effects zone source replacement retargets effect controls to the new layer", async ({ page }) => {
  const fixture = await openControls(page, { surface: "effects" });
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

test("Effects zone rejected control edits restore the canonical value without remounting", async ({ page }) => {
  const fixture = await openControls(page, { rejectControls: true, surface: "effects" });
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


test("Effects zone snapshots preserve in-flight and queued edits", async ({ page }) => {
  let releaseFirst;
  let requests = 0;
  const firstReply = new Promise((resolve) => { releaseFirst = resolve; });
  const fixture = await openControls(page, {
    surface: "effects",
    beforeControlReply: async () => { if (++requests === 1) await firstReply; },
  });
  const lower = fixture.control("lower");
  await lower.focus();
  await lower.press("ArrowRight");
  await expect.poll(() => fixture.writes.length).toBe(1);
  fixture.scene.zones[1].layers[0].source.controls.lower = float(0.12);
  fixture.scene.zones[1].layers[0].source.controls.upper = float(0.82);
  fixture.scene.revision += 1;
  fixture.sendEvent("zone_changed", {
    scene_id: fixture.scene.id, zone_id: ZONE, role: "custom", kind: "controls_patched",
  });
  await expect(fixture.control("upper")).toHaveValue("0.82");
  await expect(lower).toHaveValue("0.41");
  await lower.press("ArrowRight");
  await expect(lower).toHaveValue("0.42");
  releaseFirst();
  await expect.poll(() => fixture.writes.length).toBe(2);
  await settle(page);
  await expect(lower).toHaveValue("0.42");
  expect(fixture.writes[0].body.values.lower.value).toBeCloseTo(0.41, 6);
  expect(fixture.writes[1].body.values.lower.value).toBeCloseTo(0.42, 6);
  expect(fixture.unexpectedWrites).toEqual([]);
});
