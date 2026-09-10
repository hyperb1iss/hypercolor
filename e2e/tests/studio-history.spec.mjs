import { test, expect } from "@playwright/test";
import { openHistory, addStand, dragOutput, FIRST, SECOND } from "./studio-history-fixture.mjs";

const standCard = (page) => page.locator('div.group\\/card').filter({ has: page.getByText("Laptop stand", { exact: true }) });
const geometry = (element) => ({ left: element.style.left, top: element.style.top });

test("Studio device assignment saves immediately and supports undo and redo", async ({ page }) => {
  const fixture = await openHistory(page);
  await expect(fixture.undo).toBeDisabled();
  await addStand(page);
  await expect.poll(() => fixture.assigned().length).toBe(1);
  const assignment = fixture.assigned();
  expect(assignment[0].zone).toBe(FIRST);
  await expect(page.getByText("Assignment saved", { exact: true })).toBeVisible();
  await expect(fixture.undo).toBeEnabled();
  await fixture.undo.click();
  await expect.poll(() => fixture.assigned()).toEqual([]);
  await expect(fixture.redo).toBeEnabled();
  await fixture.redo.click();
  await expect.poll(() => fixture.assigned()).toEqual(assignment);
  await expect(fixture.redo).toBeDisabled();
  expect(fixture.writes.map((write) => write.path)).toEqual(Array(3).fill("/api/v1/scene/members/edit"));
  expect(fixture.rejected).toEqual([]);
  expect(fixture.unexpected).toEqual([]);
});

test("Studio moving a device restores its original zone and placement on undo", async ({ page }) => {
  const fixture = await openHistory(page, { placedStand: true });
  const original = structuredClone(fixture.scene.zones[0]);
  await standCard(page).getByTitle("Device options").click();
  await page.getByRole("button", { name: "Move to Accent lights", exact: true }).click();
  await expect.poll(() => fixture.assigned()).toEqual([{ zone: SECOND, id: "stand-output" }]);
  await expect(fixture.undo).toBeEnabled();
  await fixture.undo.click();
  await expect.poll(() => fixture.scene.zones[0].members).toEqual(original.members);
  expect(fixture.scene.zones[0].layout).toEqual(original.layout);
  await expect(fixture.redo).toBeEnabled();
  await fixture.redo.click();
  await expect.poll(() => fixture.assigned()).toEqual([{ zone: SECOND, id: "stand-output" }]);
  expect(fixture.rejected).toEqual([]);
  expect(fixture.unexpected).toEqual([]);
});

test("Studio can undo removing an offline device without requiring its registry entry", async ({ page }) => {
  const fixture = await openHistory(page, { placedStand: true, offline: true });
  const original = structuredClone(fixture.scene.zones[0]);
  await page.getByRole("button", { name: "Remove from this zone", exact: true }).click();
  await expect.poll(() => fixture.assigned()).toEqual([]);
  await expect(fixture.undo).toBeEnabled();
  await fixture.undo.click();
  await expect.poll(() => fixture.scene.zones[0].members).toEqual(original.members);
  expect(fixture.scene.zones[0].layout).toEqual(original.layout);
  await expect(page.getByRole("button", { name: "Remove from this zone", exact: true })).toBeVisible();
  expect(fixture.rejected).toEqual([]);
  expect(fixture.unexpected).toEqual([]);
});

test("Studio undo follows geometry and assignment edits in chronological order", async ({ page }) => {
  const fixture = await openHistory(page);
  const anchor = page.locator('[data-zone-id="anchor-output"]');
  const original = await anchor.evaluate(geometry);
  await dragOutput(page, "anchor-output");
  await expect.poll(() => anchor.evaluate(geometry)).not.toEqual(original);
  const moved = await anchor.evaluate(geometry);
  await addStand(page);
  await expect.poll(() => fixture.assigned().length).toBe(1);
  await expect(fixture.undo).toBeEnabled();
  await fixture.undo.click();
  await expect.poll(() => fixture.assigned()).toEqual([]);
  await expect.poll(() => anchor.evaluate(geometry)).toEqual(moved);
  await expect(fixture.undo).toBeEnabled();
  await fixture.undo.click();
  await expect.poll(() => anchor.evaluate(geometry)).toEqual(original);
  await expect(fixture.redo).toBeEnabled();
  await fixture.redo.click();
  await expect.poll(() => anchor.evaluate(geometry)).toEqual(moved);
  await fixture.redo.click();
  await expect.poll(() => fixture.assigned().length).toBe(1);
  expect(fixture.rejected).toEqual([]);
  expect(fixture.unexpected).toEqual([]);
});

test("Studio rejected assignment leaves existing history and device membership intact", async ({ page }) => {
  const fixture = await openHistory(page);
  const anchor = page.locator('[data-zone-id="anchor-output"]');
  const original = await anchor.evaluate(geometry);
  await dragOutput(page, "anchor-output");
  await expect.poll(() => anchor.evaluate(geometry)).not.toEqual(original);
  fixture.rejectNext();
  await addStand(page);
  await expect.poll(() => fixture.rejected.length).toBe(1);
  await expect(fixture.undo).toBeEnabled();
  expect(fixture.assigned()).toEqual([]);
  await expect(fixture.redo).toBeDisabled();
  await fixture.undo.click();
  await expect.poll(() => anchor.evaluate(geometry)).toEqual(original);
  await expect(fixture.undo).toBeDisabled();
  await expect(fixture.redo).toBeEnabled();
  expect(fixture.unexpected).toEqual([]);
});

test("Studio assignment completion after navigation does not access a disposed editor", async ({ page }) => {
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  const fixture = await openHistory(page, { beforeMemberReply: () => gate });
  await addStand(page);
  await expect.poll(() => fixture.writes.length).toBe(1);
  await page.getByRole("link", { name: "Effects", exact: true }).click();
  await expect(page).toHaveURL(/\/effects$/);
  release();
  await expect.poll(() => fixture.assigned().length).toBe(1);
  await page.waitForLoadState("networkidle");
  expect(errors).toEqual([]);
  expect(fixture.unexpected).toEqual([]);
});

test("Studio geometry undo preserves unrelated placement fields changed remotely", async ({ page }) => {
  const fixture = await openHistory(page);
  const anchor = page.locator('[data-zone-id="anchor-output"]');
  const original = await anchor.evaluate(geometry);
  await dragOutput(page, "anchor-output");
  await expect.poll(() => anchor.evaluate(geometry)).not.toEqual(original);
  const oldWidth = await anchor.evaluate((element) => element.style.width);
  fixture.remotePlacement("anchor-output", { size: { x: 0.3, y: 0.15 } });
  await expect.poll(() => anchor.evaluate((element) => element.style.width)).not.toBe(oldWidth);
  const remoteSize = await anchor.evaluate((element) => ({ width: element.style.width, height: element.style.height }));
  await fixture.undo.click();
  await expect.poll(() => anchor.evaluate(geometry)).toEqual(original);
  expect(await anchor.evaluate((element) => ({ width: element.style.width, height: element.style.height }))).toEqual(remoteSize);
  await fixture.redo.click();
  await expect.poll(() => anchor.evaluate(geometry)).not.toEqual(original);
  expect(await anchor.evaluate((element) => ({ width: element.style.width, height: element.style.height }))).toEqual(remoteSize);
  expect(fixture.unexpected).toEqual([]);
});

test("Studio Save commits geometry and undo redo track the saved baseline", async ({ page }) => {
  const fixture = await openHistory(page);
  const anchor = page.locator('[data-zone-id="anchor-output"]');
  const original = await anchor.evaluate(geometry);
  const save = page.getByTitle("Save layout changes", { exact: true });
  const status = page.getByRole("status").filter({ hasText: /^(Saved|Unsaved layout|Saving…)$/ });
  await dragOutput(page, "anchor-output");
  await expect.poll(() => anchor.evaluate(geometry)).not.toEqual(original);
  const moved = await anchor.evaluate(geometry);
  await expect(status).toHaveText("Unsaved layout");
  await expect(save).toBeEnabled();
  await save.click();
  await expect.poll(() => fixture.writes.filter((write) => write.method === "PUT").length).toBe(1);
  await expect(status).toHaveText("Saved");
  await expect(save).toBeDisabled();
  const persisted = structuredClone(fixture.scene.zones[0].layout);
  expect(fixture.writes[0].path).toBe(`/api/v1/scene/zones/${FIRST}/layout`);
  expect(persisted.placements[0].position.x).not.toBe(0.35);
  await fixture.undo.click();
  await expect.poll(() => anchor.evaluate(geometry)).toEqual(original);
  await expect(status).toHaveText("Unsaved layout");
  await expect(save).toBeEnabled();
  expect(fixture.scene.zones[0].layout).toEqual(persisted);
  await fixture.redo.click();
  await expect.poll(() => anchor.evaluate(geometry)).toEqual(moved);
  await expect(status).toHaveText("Saved");
  await expect(save).toBeDisabled();
  expect(fixture.writes).toHaveLength(1);
  expect(fixture.unexpected).toEqual([]);
});

test("Studio removing and restoring a device retains its unsaved placement draft", async ({ page }) => {
  const fixture = await openHistory(page, { placedStand: true });
  const stand = page.locator('[data-zone-id="stand-output"]');
  const original = await stand.evaluate(geometry);
  await dragOutput(page, "stand-output");
  await expect.poll(() => stand.evaluate(geometry)).not.toEqual(original);
  const draft = await stand.evaluate(geometry);
  await standCard(page).getByTitle("Device options").click();
  await page.getByRole("button", { name: "Remove from zone", exact: true }).click();
  await expect.poll(() => fixture.assigned()).toEqual([]);
  await expect(stand).toHaveCount(0);
  await fixture.undo.click();
  await expect.poll(() => stand.evaluate(geometry)).toEqual(draft);
  await expect(page.getByTitle("Save layout changes", { exact: true })).toBeEnabled();
  await fixture.undo.click();
  await expect.poll(() => stand.evaluate(geometry)).toEqual(original);
  expect(fixture.unexpected).toEqual([]);
});

test("Studio saving assignment blocks canvas edits without changing history order", async ({ page }) => {
  let release;
  let replyCount = 0;
  const gate = new Promise((resolve) => { release = resolve; });
  const fixture = await openHistory(page, { beforeMemberReply: () => ++replyCount === 1 ? gate : undefined });
  const anchor = page.locator('[data-zone-id="anchor-output"]');
  const original = await anchor.evaluate(geometry);
  await dragOutput(page, "anchor-output");
  await expect.poll(() => anchor.evaluate(geometry)).not.toEqual(original);
  const moved = await anchor.evaluate(geometry);
  await addStand(page);
  await expect.poll(() => fixture.writes.length).toBe(1);
  await expect(page.getByRole("status").filter({ hasText: "Saving…" })).toBeVisible();
  await expect(fixture.undo).toBeDisabled();
  await expect(fixture.redo).toBeDisabled();
  expect(await anchor.evaluate((element) => Boolean(element.closest("[inert]")))).toBe(true);
  const bounds = await anchor.boundingBox();
  await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2);
  await page.mouse.down();
  await page.mouse.move(bounds.x + bounds.width / 2 + 35, bounds.y + bounds.height / 2 + 20);
  await page.mouse.up();
  await page.keyboard.press("Control+z");
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(resolve)));
  expect(await anchor.evaluate(geometry)).toEqual(moved);
  release();
  await expect.poll(() => fixture.assigned().length).toBe(1);
  await expect(fixture.undo).toBeEnabled();
  await fixture.undo.click();
  await expect.poll(() => fixture.assigned()).toEqual([]);
  await expect.poll(() => anchor.evaluate(geometry)).toEqual(moved);
  await fixture.undo.click();
  await expect.poll(() => anchor.evaluate(geometry)).toEqual(original);
  await expect(fixture.undo).toBeDisabled();
  expect(fixture.unexpected).toEqual([]);
});

test("Studio assignment from the selected Unassigned bucket opens the destination undo toolbar", async ({ page }) => {
  const fixture = await openHistory(page);
  await page.getByRole("button", { name: /^Unassigned Hardware in no zone/ }).click();
  await expect(fixture.undo).toHaveCount(0);
  await addStand(page);
  await expect.poll(() => fixture.assigned().length).toBe(1);
  await expect(fixture.undo).toBeVisible();
  await expect(fixture.undo).toBeEnabled();
  await fixture.undo.click();
  await expect.poll(() => fixture.assigned()).toEqual([]);
  await expect(fixture.redo).toBeEnabled();
  expect(fixture.unexpected).toEqual([]);
});

test("Studio assignment completion respects a surface selected while saving", async ({ page }) => {
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  const fixture = await openHistory(page, { beforeMemberReply: () => gate });
  await page.getByRole("button", { name: /^Unassigned Hardware in no zone/ }).click();
  await addStand(page);
  await expect.poll(() => fixture.writes.length).toBe(1);
  await page.getByRole("button", { name: "Accent lights 0 devices", exact: true }).click();
  release();
  await expect.poll(() => fixture.assigned().length).toBe(1);
  await expect(fixture.undo).toBeEnabled();
  await expect(page.locator('[data-zone-id="stand-output"]')).toHaveCount(0);
  await expect(page.locator('[data-zone-id="anchor-output"]')).toHaveCount(0);
  expect(fixture.unexpected).toEqual([]);
});

test("Studio rejected assignment keeps the Unassigned bucket selected", async ({ page }) => {
  const fixture = await openHistory(page);
  await page.getByRole("button", { name: /^Unassigned Hardware in no zone/ }).click();
  fixture.rejectNext();
  await addStand(page);
  await expect.poll(() => fixture.rejected.length).toBe(1);
  await page.waitForLoadState("networkidle");
  await expect(fixture.undo).toHaveCount(0);
  expect(fixture.assigned()).toEqual([]);
  expect(fixture.unexpected).toEqual([]);
});

test("Studio pending assignment disables open device menus and the add picker", async ({ page }) => {
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  const fixture = await openHistory(page, { beforeMemberReply: () => gate });
  await page.getByTitle("Device options", { exact: true }).click();
  const move = page.getByRole("button", { name: "Move to Accent lights", exact: true });
  const remove = page.getByRole("button", { name: "Remove from zone", exact: true });
  await expect(move).toBeEnabled();
  await page.getByRole("button", { name: "Add device", exact: true }).last().click();
  const picker = page.getByRole("button", { name: "Pick a device…", exact: true });
  await expect(picker).toBeEnabled();
  await addStand(page);
  await expect.poll(() => fixture.writes.length).toBe(1);
  // The rail may close its picker when the operation starts. Either state
  // must make another assignment unavailable until the reply arrives.
  if (await picker.count()) {
    await expect(picker).toBeDisabled();
  } else {
    await expect(page.getByRole("button", { name: "Add device", exact: true }).last()).toBeDisabled();
  }
  if (!(await move.isVisible())) await page.getByTitle("Device options", { exact: true }).click();
  await expect(move).toBeDisabled();
  await expect(remove).toBeDisabled();
  await expect(page.getByTitle("Add to a zone", { exact: true })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Accent lights 0 devices", exact: true })).toBeEnabled();
  release();
  await expect.poll(() => fixture.assigned().length).toBe(1);
  if (!(await picker.count())) await page.getByRole("button", { name: "Add device", exact: true }).last().click();
  await expect(picker).toBeEnabled();
  if (!(await move.isVisible())) {
    await page.locator('div.group\\/card').filter({ has: page.getByText("Anchor light", { exact: true }) })
      .getByTitle("Device options", { exact: true }).click();
  }
  await expect(move).toBeEnabled();
  await expect(remove).toBeEnabled();
  expect(fixture.writes).toHaveLength(1);
  expect(fixture.unexpected).toEqual([]);
});
